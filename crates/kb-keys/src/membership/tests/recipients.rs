//! What only a recipient can check (real boxes, real keys) and the planning of a rekey.

use super::*;
use crate::space_keys::{open_space_key_box, seal_space_key_for, unwrap_prev, wrap_prev};
use kb_crypto::CryptoError;

fn real(people: usize) -> Space {
    Space::new(people, false, 41)
}

fn is_mismatch<T: core::fmt::Debug>(result: Result<T, KeysError>) -> bool {
    matches!(result, Err(KeysError::Crypto(CryptoError::Authentication)))
}

/// `known_sks` over a map.
fn known(space: &Space, epochs: &[u32]) -> Box<dyn Fn(u32) -> Option<SpaceKey>> {
    let keys: Vec<(u32, SpaceKey)> = epochs.iter().map(|e| (*e, space.keys[e].clone())).collect();
    Box::new(move |epoch| {
        keys.iter()
            .find(|(e, _)| *e == epoch)
            .map(|(_, k)| k.clone())
    })
}

fn dev(space: &Space, person: usize, device: usize) -> &Dev {
    &space.world.people[person].devices[device]
}

// ----- planning the boxes --------------------------------------------------------------------

#[test]
fn a_rekey_is_planned_for_the_active_devices_as_of_each_log_head() {
    let mut space = real(3);
    space.start(0, &[(1, MemberRole::Editor), (2, MemberRole::Viewer)]);
    // Person 1 adds a device and revokes its first; person 2 stays as it is.
    space.world.people[1].add_device(7);
    space.world.people[1].revoke_device(0);
    let members: Vec<AccountId> = (0..3).map(|p| space.account(p)).collect();
    let plan = rekey_recipients(&members, &space.world).unwrap();

    assert_eq!(plan.member_heads.len(), 3);
    for (head, person) in plan.member_heads.iter().zip(&space.world.people) {
        assert_eq!(head.account, person.account);
        assert_eq!(head.head, person.head());
        assert_eq!(head.head.hash, person.latest().head_hash().unwrap());
    }
    // Member by member, active devices only, with their public keys.
    let expected: Vec<(AccountId, DeviceId)> = vec![
        (space.account(0), dev(&space, 0, 0).id),
        (space.account(1), dev(&space, 1, 1).id),
        (space.account(2), dev(&space, 2, 0).id),
    ];
    let got: Vec<(AccountId, DeviceId)> = plan.devices.iter().map(|(a, d, _)| (*a, *d)).collect();
    assert_eq!(got, expected);
    for (_, device, keys) in &plan.devices {
        let owner = space
            .world
            .people
            .iter()
            .flat_map(|p| p.devices.iter())
            .find(|d| d.id == *device)
            .unwrap();
        assert_eq!(*keys, owner.keys.public());
    }
    // The order follows the members given.
    let reversed = [members[2], members[0]];
    let plan = rekey_recipients(&reversed, &space.world).unwrap();
    assert_eq!(plan.member_heads[0].account, members[2]);
    assert_eq!(plan.devices[1].0, members[0]);
    // And the fold takes exactly this plan.
    let rekey = space.rekey(0);
    space.push(rekey);
}

#[test]
fn a_rekey_cannot_be_planned_from_logs_that_are_missing_or_named_twice() {
    let mut space = real(2);
    space.start(0, &[(1, MemberRole::Editor)]);
    let (a0, a1) = (space.account(0), space.account(1));
    assert!(matches!(
        rekey_recipients(&[a0, account(0x99)], &space.world),
        Err(KeysError::Corrupted(_))
    ));
    assert!(matches!(
        rekey_recipients(&[a0, a1, a0], &space.world),
        Err(KeysError::Corrupted(_))
    ));
    assert!(matches!(
        rekey_recipients(&[a0], &NoLogs),
        Err(KeysError::Corrupted(_))
    ));
    let none = rekey_recipients(&[], &space.world).unwrap();
    assert!(none.member_heads.is_empty() && none.devices.is_empty());
}

// ----- opening what was boxed -----------------------------------------------------------------

#[test]
fn a_create_opens_for_the_creators_devices_only() {
    let mut space = real(2);
    space.world.people[0].add_device(5);
    let create = space.create(0);
    let key = space.keys[&1].clone();
    for device in &space.world.people[0].devices {
        // Device 0 is still active in this log (nothing was revoked), device 1 too.
        let opened = verify_for_recipient(&create, device.id, &device.keys, &|_| None).unwrap();
        assert_eq!(opened, Some(key.clone()));
    }
    let outsider = dev(&space, 1, 0);
    assert_eq!(
        verify_for_recipient(&create, outsider.id, &outsider.keys, &|_| None).unwrap(),
        None
    );
    // A device with someone else's id and keys: the box is for the other keys.
    let wrong = dev(&space, 0, 0);
    assert!(is_mismatch(verify_for_recipient(
        &create,
        space.world.people[0].devices[1].id,
        &wrong.keys,
        &|_| None
    )));
}

#[test]
fn a_rekey_is_opened_checked_against_its_commitment_and_chained_to_the_previous_key() {
    let mut space = real(3);
    space.start(0, &[(1, MemberRole::Editor), (2, MemberRole::Viewer)]);
    space.world.people[1].add_device(9);
    let rekey = space.rekey(1);
    space.push(rekey.clone());
    let new_key = space.keys[&2].clone();
    let old = known(&space, &[1]);

    for person in 0..3 {
        for device in &space.world.people[person].devices {
            let active = space.world.people[person]
                .log()
                .verify()
                .unwrap()
                .active(device.id)
                .is_some();
            let result = verify_for_recipient(&rekey, device.id, &device.keys, &old).unwrap();
            assert_eq!(result, active.then(|| new_key.clone()), "person {person}");
        }
    }
    // The new key matches what the state recorded; the old one does not.
    assert!(space.state().current_key_matches(&new_key));
    assert!(!space.state().current_key_matches(&space.keys[&1]));
}

#[test]
fn a_box_for_other_keys_than_the_commitment_names_is_caught_by_the_member_it_cheats() {
    // The Rekey gives Alice (person 0) the committed key and Bob (person 1) another one. The
    // fold cannot see inside a box, so it folds; Bob's check catches it.
    let mut space = real(2);
    space.start(0, &[(1, MemberRole::Editor)]);
    let rekey = space.rekey(0);
    let feed = space.feed;
    let bob = dev(&space, 1, 0).clone();
    let other = SpaceKey::from_secret(Secret32::from_bytes([0x66; 32]));
    let cheating = space.rebuild(&rekey, |h| {
        let EntryBody::Rekey(f) = &mut h.body else {
            panic!("a rekey")
        };
        for key_box in &mut f.boxes {
            if key_box.device == bob.id {
                key_box.bytes =
                    seal_space_key_for(&other, feed, f.new_epoch, &bob.keys.public(), &rng(3));
            }
        }
    });
    let alice = dev(&space, 0, 0).clone();
    let old = known(&space, &[1]);
    space.push(cheating.clone());

    let for_alice = verify_for_recipient(&cheating, alice.id, &alice.keys, &old).unwrap();
    assert_eq!(for_alice, Some(space.keys[&2].clone()));
    assert!(is_mismatch(verify_for_recipient(
        &cheating, bob.id, &bob.keys, &old
    )));
    // Bob's key is a key, just not the committed one: the box itself opens.
    assert_eq!(
        open_space_key_box(&bob.keys, space.feed, 2, &box_of(&cheating, bob.id)).unwrap(),
        other
    );
    assert!(!space.state().current_key_matches(&other));
}

fn box_of(entry: &MembershipEntry, device: DeviceId) -> Vec<u8> {
    let EntryBody::Rekey(fields) = &entry.header.body else {
        panic!("a rekey")
    };
    fields
        .boxes
        .iter()
        .find(|b| b.device == device)
        .unwrap()
        .bytes
        .clone()
}

#[test]
fn prev_wrapped_must_open_under_the_new_key_to_the_previous_one() {
    let mut space = real(2);
    space.start(0, &[(1, MemberRole::Editor)]);
    let rekey = space.rekey(0);
    let alice = dev(&space, 0, 0).clone();
    let new_key = space.keys[&2].clone();
    let old_key = space.keys[&1].clone();
    let feed = space.feed;
    let with_prev = |wrapped: WrappedKey| {
        space.rebuild(&rekey, |h| {
            let EntryBody::Rekey(f) = &mut h.body else {
                panic!("a rekey")
            };
            f.prev_wrapped = wrapped;
        })
    };
    let check = |entry: &MembershipEntry, known: &[(u32, SpaceKey)]| {
        let known = known.to_vec();
        verify_for_recipient(entry, alice.id, &alice.keys, &move |e| {
            known.iter().find(|(x, _)| *x == e).map(|(_, k)| k.clone())
        })
    };
    let knows_old = [(1u32, old_key.clone())];

    // The genuine one.
    assert_eq!(check(&rekey, &knows_old).unwrap(), Some(new_key.clone()));
    assert_eq!(check(&rekey, &[]).unwrap(), Some(new_key.clone()));

    // Another previous key under the new key: it opens, but it is not the committed one.
    let wrong_prev = wrap_prev(
        &SpaceKey::from_secret(Secret32::from_bytes([3; 32])),
        &new_key,
        feed,
        1,
        &rng(1),
    );
    assert!(is_mismatch(check(
        &with_prev(wrong_prev.clone()),
        &knows_old
    )));
    // A recipient that never held the previous key can only check that it opens.
    assert_eq!(
        check(&with_prev(wrong_prev), &[]).unwrap(),
        Some(new_key.clone())
    );

    // The previous key wrapped under another key, for another epoch or another feed: it does
    // not open under the new key at all, with or without knowledge of the old one.
    let under_other = wrap_prev(&old_key, &old_key, feed, 1, &rng(2));
    let other_epoch = wrap_prev(&old_key, &new_key, feed, 0, &rng(3));
    let other_feed = wrap_prev(&old_key, &new_key, FeedId::from_bytes([9; 16]), 1, &rng(4));
    for wrapped in [under_other, other_epoch, other_feed] {
        let entry = with_prev(wrapped);
        assert!(is_mismatch(check(&entry, &knows_old)));
        assert!(is_mismatch(check(&entry, &[])));
    }
    // And a recipient that knows a different previous key than the one the chain opens to.
    let liar = [(1u32, SpaceKey::from_secret(Secret32::from_bytes([4; 32])))];
    assert!(is_mismatch(check(&rekey, &liar)));
    // The chain does open to the old key.
    let EntryBody::Rekey(fields) = &rekey.header.body else {
        panic!("a rekey")
    };
    assert_eq!(
        unwrap_prev(&new_key, &fields.prev_wrapped, feed, 1).unwrap(),
        old_key
    );
}

#[test]
fn a_box_sealed_for_another_epoch_or_feed_does_not_open() {
    let mut space = real(2);
    space.start(0, &[(1, MemberRole::Editor)]);
    let rekey = space.rekey(0);
    let alice = dev(&space, 0, 0).clone();
    let key = space.keys[&2].clone();
    for (feed, epoch) in [
        (FeedId::from_bytes([7; 16]), 2),
        (space.feed, 3),
        (space.feed, 1),
    ] {
        let forged = space.rebuild(&rekey, |h| {
            let EntryBody::Rekey(f) = &mut h.body else {
                panic!("a rekey")
            };
            for key_box in &mut f.boxes {
                if key_box.device == alice.id {
                    key_box.bytes =
                        seal_space_key_for(&key, feed, epoch, &alice.keys.public(), &rng(1));
                }
            }
        });
        assert!(is_mismatch(verify_for_recipient(
            &forged,
            alice.id,
            &alice.keys,
            &|_| None
        )));
    }
}

#[test]
fn a_remove_is_a_rekey_for_the_members_that_stay() {
    let mut space = real(3);
    space.start(0, &[(1, MemberRole::Editor), (2, MemberRole::Viewer)]);
    let remove = space.remove(0, 2);
    space.push(remove.clone());
    let new_key = space.keys[&2].clone();
    let old = known(&space, &[1]);
    for person in [0, 1] {
        let d = dev(&space, person, 0);
        assert_eq!(
            verify_for_recipient(&remove, d.id, &d.keys, &old).unwrap(),
            Some(new_key.clone())
        );
    }
    // The removed member has no box, and nobody else's box opens for them.
    let removed = dev(&space, 2, 0);
    assert_eq!(
        verify_for_recipient(&remove, removed.id, &removed.keys, &old).unwrap(),
        None
    );
    let EntryBody::Remove { rekey, .. } = &remove.header.body else {
        panic!("a remove")
    };
    for key_box in &rekey.boxes {
        assert!(open_space_key_box(&removed.keys, space.feed, 2, &key_box.bytes).is_err());
    }
    // What they hold (epoch 1) does not derive the new membership-meta key.
    let next = space.profile(0, "after the removal");
    let mk_old = crate::space_keys::membership_meta_key(&space.keys[&1], space.feed);
    assert!(next.open_private(&mk_old).is_err());
    let mk_new = crate::space_keys::membership_meta_key(&new_key, space.feed);
    assert!(next.open_private(&mk_new).is_ok());
}

#[test]
fn a_grant_is_checked_against_the_known_key_or_the_timeline() {
    let mut space = real(3);
    space.start(0, &[(1, MemberRole::Editor), (2, MemberRole::Viewer)]);
    let rekey = space.rekey(0);
    space.push(rekey);
    let grant = space.grant(0, 2);
    space.push(grant.clone());
    let viewer = dev(&space, 2, 0).clone();
    let key = space.keys[&2].clone();

    // The viewer (who has an older bundle) does not know epoch 2: it gets the key and the
    // timeline vouches for it.
    let opened = verify_for_recipient(&grant, viewer.id, &viewer.keys, &|_| None)
        .unwrap()
        .unwrap();
    assert_eq!(opened, key);
    assert!(space.state().current_key_matches(&opened));
    // One that knows the epoch compares.
    let knows = known(&space, &[2]);
    assert_eq!(
        verify_for_recipient(&grant, viewer.id, &viewer.keys, &knows).unwrap(),
        Some(key.clone())
    );
    let other_key = SpaceKey::from_secret(Secret32::from_bytes([5; 32]));
    assert!(is_mismatch(verify_for_recipient(
        &grant,
        viewer.id,
        &viewer.keys,
        &move |e| (e == 2).then(|| other_key.clone())
    )));
    // A Grant that boxes another key opens for the viewer all the same, and the timeline refuses it.
    let cheat = SpaceKey::from_secret(Secret32::from_bytes([6; 32]));
    let feed = space.feed;
    let boxed_wrong = space.rebuild(&grant, |h| {
        let EntryBody::Grant { boxes, .. } = &mut h.body else {
            panic!("a grant")
        };
        boxes[0].bytes = seal_space_key_for(&cheat, feed, 2, &viewer.keys.public(), &rng(8));
    });
    let opened = verify_for_recipient(&boxed_wrong, viewer.id, &viewer.keys, &|_| None)
        .unwrap()
        .unwrap();
    assert_eq!(opened, cheat);
    assert!(!space.state().current_key_matches(&opened));
    // Someone else's devices get nothing from this Grant.
    let editor = dev(&space, 1, 0);
    assert_eq!(
        verify_for_recipient(&grant, editor.id, &editor.keys, &|_| None).unwrap(),
        None
    );
}

#[test]
fn entries_without_boxes_hold_nothing_for_a_recipient() {
    let mut space = real(3);
    space.start(0, &[(1, MemberRole::Editor)]);
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    let join = {
        space.push(invite.clone());
        space.join(2, &secret)
    };
    let entries = [
        invite,
        join,
        space.profile(0, "x"),
        space.leave(1),
        space.close(0),
        space.settings(0, SpaceSettings::default()),
        space.role_set(0, 1, MemberRole::Viewer),
    ];
    let alice = dev(&space, 0, 0).clone();
    for entry in &entries {
        assert_eq!(
            verify_for_recipient(entry, alice.id, &alice.keys, &|_| None).unwrap(),
            None,
            "{:?}",
            entry.kind()
        );
    }
    // A Rekey with no box for this device.
    let rekey = space.rekey(0);
    let missing = space.rebuild(&rekey, |h| {
        let EntryBody::Rekey(f) = &mut h.body else {
            panic!("a rekey")
        };
        f.boxes.retain(|b| b.device != alice.id);
    });
    assert_eq!(
        verify_for_recipient(&missing, alice.id, &alice.keys, &|_| None).unwrap(),
        None
    );
}
