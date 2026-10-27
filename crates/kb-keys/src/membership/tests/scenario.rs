//! A whole life of a space with the real cryptography end to end: real signatures, real boxes,
//! real proofs and bundles, folded by a server that holds no key and by members that do.

use super::*;
use crate::invite::{
    CodeBundle, InviteSecret, LinkBundle, invite_id, open_link_bundle, seal_link_bundle,
};
use crate::space_keys::membership_meta_key;

fn meta(space: &Space, epoch: u32) -> Secret32 {
    membership_meta_key(&space.keys[&epoch], space.feed)
}

/// The names the members see: each entry's private part opened with the key of its epoch.
fn names(space: &Space) -> Vec<String> {
    space
        .entries
        .iter()
        .filter_map(|(_, entry)| {
            let key = meta(space, entry.header.meta_epoch);
            match entry.open_private(&key).ok()? {
                Private::Create { profile_name, .. } | Private::Join { profile_name } => {
                    Some(profile_name)
                }
                Private::Profile { display_name } => Some(display_name),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn the_whole_story_with_real_cryptography() {
    // Alice 0, Bob 1, Carol 2, Dave 3.
    let mut space = Space::new(4, false, 51);
    let feed = space.feed;

    // Alice creates the space: epoch 1.
    let create = space.create(0);
    space.push(create);
    let key1 = space.keys[&1].clone();

    // A link invite for Bob (Editor): the bundle seals SK[1] and the names under the secret.
    let secret_bob = InviteSecret::generate(&rng(1));
    let invite = space.invite_with(0, &secret_bob, MemberRole::Editor, None, 7 * DAY);
    let bundle = LinkBundle {
        epoch: 1,
        space_key: key1.clone(),
        space_name: "Space".to_owned(),
        inviter_name: "Person 0".to_owned(),
    };
    let sealed = seal_link_bundle(&bundle, &secret_bob, &rng(2));
    let invite = space.rebuild(&invite, |h| {
        if let EntryBody::Invite { enc_bundle, .. } = &mut h.body {
            *enc_bundle = Some(sealed.clone());
        }
    });
    space.push(invite);

    // Bob, who has the link and nothing else, fetches the log from the server (invite open), checks
    // it, opens the bundle and reads what the space is called.
    let log_for_bob = space.entries.clone();
    let seen_by_bob = fold(feed, &log_for_bob, &space.world).unwrap();
    let record = seen_by_bob.state().invite(invite_id(&secret_bob)).unwrap();
    assert_eq!(record.created_by, space.account(0));
    assert_eq!(
        seen_by_bob.state().role_of(record.created_by),
        Some(MemberRole::Owner)
    );
    let opened = open_link_bundle(record.enc_bundle.as_ref().unwrap(), &secret_bob).unwrap();
    assert!(seen_by_bob.state().current_key_matches(&opened.space_key));
    let create_entry = &log_for_bob[0].1;
    match create_entry
        .open_private(&membership_meta_key(&opened.space_key, feed))
        .unwrap()
    {
        Private::Create { space_name, .. } => assert_eq!(space_name, opened.space_name),
        other => panic!("{other:?}"),
    }
    // Bob joins: his device signs the entry and the invite key signs the proof.
    let join = space.join(1, &secret_bob);
    space.push(join);

    // Alice invites Carol by code (bound to Carol's account); the secret travels in the code bundle.
    let secret_carol = InviteSecret::generate(&rng(3));
    let invite = space.invite_with(0, &secret_carol, MemberRole::Viewer, Some(2), 3600 * 1000);
    space.push(invite);
    let head = space.state().head.unwrap();
    let code = CodeBundle {
        feed_id: feed,
        space_id: kb_types::SpaceId::from_bytes([4; 16]),
        inviter: space.account(0),
        secret: secret_carol.clone(),
        epoch: 1,
        space_key: key1.clone(),
        membership_head: head,
        space_name: "Space".to_owned(),
    };
    let code = CodeBundle::decode(&code.encode()).unwrap();
    assert_eq!(code.membership_head, head);
    let join = space.join(2, &code.secret);
    space.push(join);

    // Roles and names.
    let role = space.role_set(0, 2, MemberRole::Commenter);
    space.push(role);
    let profile = space.profile(1, "Bobby");
    space.push(profile);

    // Carol adds a device; the next Rekey boxes it.
    space.world.people[2].add_device(77);
    let rekey = space.rekey(1);
    space.push(rekey.clone());
    let carol_new = space.world.people[2].devices[1].clone();
    let key2 = space.keys[&2].clone();
    let known_1 = {
        let k = key1.clone();
        move |e: u32| (e == 1).then(|| k.clone())
    };
    assert_eq!(
        verify_for_recipient(&rekey, carol_new.id, &carol_new.keys, &known_1).unwrap(),
        Some(key2.clone())
    );

    // Alice removes Bob: one entry, one Rekey, atomic.
    let remove = space.remove(0, 1);
    space.push(remove.clone());
    let key3 = space.keys[&3].clone();
    for person in [0usize, 2] {
        for device in &space.world.people[person].devices {
            let known = {
                let k = key2.clone();
                move |e: u32| (e == 2).then(|| k.clone())
            };
            assert_eq!(
                verify_for_recipient(&remove, device.id, &device.keys, &known).unwrap(),
                Some(key3.clone()),
                "person {person}"
            );
        }
    }
    let bob = space.world.people[1].devices[0].clone();
    assert_eq!(
        verify_for_recipient(&remove, bob.id, &bob.keys, &|_| None).unwrap(),
        None,
        "Bob has no box"
    );
    // What Alice writes next is sealed under a key Bob never gets.
    let after = space.profile(0, "Alice after");
    space.push(after.clone());
    assert!(after.open_private(&meta(&space, 1)).is_err());
    assert!(after.open_private(&meta(&space, 2)).is_err());
    assert_eq!(
        after.open_private(&meta(&space, 3)).unwrap(),
        Private::Profile {
            display_name: "Alice after".to_owned()
        }
    );
    // Bob's later entries are refused.
    let late = space.profile(1, "Bobby again");
    refuse(&mut space, late, FoldRule::Rule(rules::SIGNER_NOT_MEMBER));

    // Dave joins the space through a new link; Carol leaves; Alice closes.
    let (invite, secret_dave) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(invite);
    let join = space.join(3, &secret_dave);
    space.push(join);
    let leave = space.leave(2);
    space.push(leave);
    assert!(space.state().pending_rekey);
    let rekey = space.rekey(3);
    space.push(rekey);
    assert!(!space.state().pending_rekey);
    let close = space.close(0);
    space.push(close);

    // The server (no key at all) and a member (who folds from the very start) agree on everything.
    let final_log = space.entries.clone();
    let server = fold(feed, &final_log, &space.world).unwrap();
    assert_eq!(server, space.timeline);
    assert_eq!(server.state().closed, Some(space.cursor));
    assert_eq!(server.state().epoch, 4);
    assert_eq!(
        space.roster(),
        [(0, MemberRole::Owner), (3, MemberRole::Editor)]
    );
    assert_eq!(
        names(&space),
        [
            "Person 0",
            "Person 1",
            "Person 2",
            "Bobby",
            "Alice after",
            "Person 3"
        ]
        .map(str::to_owned)
    );
}

#[test]
fn the_server_and_a_member_fold_the_same_entries_from_their_bytes() {
    // What a server stores and a client downloads are the encoded entries: decoding them gives
    // entries that fold exactly like the ones that were built.
    let mut space = Space::new(3, false, 52);
    space.start(0, &[(1, MemberRole::Editor), (2, MemberRole::Viewer)]);
    let remove = space.remove(0, 2);
    space.push(remove);
    let stored: Vec<(u64, Vec<u8>)> = space
        .entries
        .iter()
        .map(|(cursor, entry)| (*cursor, entry.encode()))
        .collect();
    let decoded: Vec<(u64, MembershipEntry)> = stored
        .iter()
        .map(|(cursor, bytes)| (*cursor, MembershipEntry::decode(bytes).unwrap()))
        .collect();
    assert_eq!(decoded, space.entries);
    let timeline = fold(space.feed, &decoded, &space.world).unwrap();
    assert_eq!(timeline, space.timeline);
    // The server routes on the header alone.
    for (_, bytes) in &stored {
        let header = MembershipEntry::peek(bytes).unwrap();
        assert_eq!(header.feed, space.feed);
    }
}
