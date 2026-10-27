//! The fold: the chain, the signer, `Create`, invites and `Join` (stand-in signatures, see the
//! testkit; the real signature path is covered by `entries` and `scenario`).

use super::*;
use crate::invite::{InviteSecret, invite_id, invite_signing_key};
use kb_types::sharing::{MAX_MEMBERS_PER_FEED, MAX_OPEN_INVITES};
use rules::*;

fn fast(people: usize) -> Space {
    Space::new(people, true, 11)
}

/// Person 0 Owner, 1 Editor, 2 Viewer; 3 and 4 are outsiders.
fn trio() -> Space {
    let mut space = fast(5);
    space.start(0, &[(1, MemberRole::Editor), (2, MemberRole::Viewer)]);
    space
}

fn rule(name: &'static str) -> FoldRule {
    FoldRule::Rule(name)
}

fn secret_of(seed: u64) -> InviteSecret {
    InviteSecret::generate(&rng(seed))
}

// ----- Create and the chain ------------------------------------------------------------------

#[test]
fn a_create_makes_its_signer_the_only_owner() {
    let mut space = fast(2);
    let create = space.create(0);
    let at = space.clock;
    let applied = space.push(create.clone());
    assert_eq!(
        applied,
        Applied {
            kind: EntryKind::Create,
            signer: space.account(0),
            subject: None,
            previous_role: None,
        }
    );
    let state = space.state();
    assert_eq!(
        state.members,
        vec![MemberRecord {
            account: space.account(0),
            role: MemberRole::Owner,
            joined_cursor: 1,
            joined_at: at,
        }]
    );
    assert_eq!(
        (state.epoch, state.pending_rekey, state.closed),
        (1, false, None)
    );
    assert_eq!(
        state.sk_commitment,
        space.keys[&1].commitment(space.feed, 1)
    );
    assert_eq!(state.head, Some(create.head()));
    assert!(state.invites.is_empty());
    assert_eq!(state.owners(), vec![space.account(0)]);
}

#[test]
fn the_log_starts_with_a_create_at_position_zero_and_has_one_only_there() {
    let mut space = fast(2);
    let create = space.create(0);
    let who = Space::who(0);

    // Anything else first.
    let leave = space.leave(0);
    refuse(&mut space, leave, FoldRule::Chain);
    // A Create at another position, or after another entry.
    let body = create.header.body.clone();
    let at = space.clock;
    let seq_one = space.entry_at(who, 1, [0; 32], at, 1, body.clone(), &Private::None);
    refuse(&mut space, seq_one, FoldRule::Chain);
    let prev_set = space.entry_at(who, 0, [1; 32], at, 1, body, &Private::None);
    refuse(&mut space, prev_set, FoldRule::Chain);

    space.push(create);
    let second = space.create(1);
    refuse(&mut space, second, FoldRule::Chain);
}

#[test]
fn a_create_boxes_exactly_the_creators_active_devices() {
    let mut space = fast(2);
    // Version 3 of person 0's log: device 0 revoked, devices 1 and 2 active.
    space.world.people[0].version = 3;
    let create = space.create(0);
    let ids: Vec<DeviceId> = space.world.people[0].devices.iter().map(|d| d.id).collect();
    let stranger = space.world.people[1].devices[0].id;
    let boxed = |bytes: usize| vec![0u8; bytes];

    type Edit = Box<dyn Fn(&mut EntryBody)>;
    let cases: Vec<(&str, Edit, FoldRule)> = vec![
        (
            "a device missing",
            Box::new(|b| {
                if let EntryBody::Create { boxes, .. } = b {
                    boxes.pop();
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "the revoked device instead of an active one",
            Box::new(move |b| {
                if let EntryBody::Create { boxes, .. } = b {
                    boxes[0].device = ids[0];
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "a device of another account",
            Box::new(move |b| {
                if let EntryBody::Create { boxes, .. } = b {
                    boxes.push(KeyBox {
                        device: stranger,
                        bytes: boxed(104),
                    });
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "a device twice",
            Box::new(|b| {
                if let EntryBody::Create { boxes, .. } = b {
                    let again = boxes[0].clone();
                    boxes.push(again);
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "no head",
            Box::new(|b| {
                if let EntryBody::Create { member_heads, .. } = b {
                    member_heads.clear();
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "two heads",
            Box::new(|b| {
                if let EntryBody::Create { member_heads, .. } = b {
                    let again = member_heads[0];
                    member_heads.push(again);
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "the head of another account",
            Box::new(|b| {
                if let EntryBody::Create { member_heads, .. } = b {
                    member_heads[0].account = AccountId::from_bytes([9; 16]);
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "a head hash that is not the log's",
            Box::new(|b| {
                if let EntryBody::Create { member_heads, .. } = b {
                    member_heads[0].head.hash = [9; 32];
                }
            }),
            FoldRule::Heads,
        ),
        (
            "a head beyond the log the source holds",
            Box::new(|b| {
                if let EntryBody::Create { member_heads, .. } = b {
                    member_heads[0].head.seq = 99;
                }
            }),
            FoldRule::UnknownLog,
        ),
    ];
    for (what, edit, expected) in cases {
        let forged = space.rebuild(&create, |h| edit(&mut h.body));
        let before = space.timeline.clone();
        assert_refused(space.submit(forged), 0, expected);
        assert_eq!(space.timeline, before, "{what}");
    }
    // A Create whose meta epoch is not its own epoch.
    let wrong_meta = space.rebuild(&create, |h| h.meta_epoch = 2);
    refuse(&mut space, wrong_meta, rule(META_EPOCH));
    // The genuine one folds.
    space.push(create);
}

// ----- the chain -----------------------------------------------------------------------------

#[test]
fn an_entry_must_continue_the_chain() {
    let mut space = trio();
    let head = space.state().head.unwrap();
    let who = Space::who(0);
    let at = space.clock;
    let body = EntryBody::Profile;
    let private = Private::Profile {
        display_name: "x".to_owned(),
    };
    // A gap, a repeat, a wrong prev (a fork), a zero prev.
    for (seq, prev) in [
        (head.seq + 2, head.hash),
        (head.seq, head.hash),
        (head.seq + 1, [3; 32]),
        (head.seq + 1, [0; 32]),
        (0, head.hash),
    ] {
        let entry = space.entry_at(who, seq, prev, at, 1, body.clone(), &private);
        refuse(&mut space, entry, FoldRule::Chain);
    }
    // The right position folds.
    let good = space.entry_at(who, head.seq + 1, head.hash, at, 1, body, &private);
    space.push(good);
}

#[test]
fn an_entry_of_another_feed_does_not_continue_the_chain() {
    let mut space = trio();
    let profile = space.profile(0, "x");
    let other = space.rebuild(&profile, |h| h.feed = FeedId::from_bytes([7; 16]));
    refuse(&mut space, other, FoldRule::Chain);
}

#[test]
fn reordered_duplicated_and_dropped_entries_break_the_chain_at_the_first_of_them() {
    let mut space = trio();
    for i in 0..4 {
        let entry = space.profile(i % 3, "x");
        space.push(entry);
    }
    let log: Vec<(u64, MembershipEntry)> = space.entries.clone();
    let broken = |entries: &[(u64, MembershipEntry)]| space.fold(entries);
    assert!(broken(&log).is_ok());
    for k in 0..log.len() - 1 {
        // Swapped with the next one.
        let mut swapped = log.clone();
        swapped.swap(k, k + 1);
        // Cursors stay in order: only the entries move.
        let cursors: Vec<u64> = log.iter().map(|(c, _)| *c).collect();
        for (i, c) in cursors.iter().enumerate() {
            swapped[i].0 = *c;
        }
        let error = broken(&swapped).unwrap_err();
        assert_eq!((error.index, error.rule), (k, FoldRule::Chain), "swap {k}");
        // The entry duplicated.
        let mut doubled = log.clone();
        let copy = (log[k].0, log[k].1.clone());
        doubled.insert(k + 1, copy);
        let error = broken(&doubled).unwrap_err();
        assert_eq!(
            (error.index, error.rule),
            (k + 1, FoldRule::Chain),
            "dup {k}"
        );
        // The entry dropped (not the last: a shorter log is a valid log).
        let mut dropped = log.clone();
        dropped.remove(k);
        if k + 1 < log.len() {
            let error = broken(&dropped).unwrap_err();
            assert_eq!((error.index, error.rule), (k, FoldRule::Chain), "drop {k}");
        }
    }
    // Without its tail the log still folds, to a shorter state.
    let shorter = broken(&log[..log.len() - 1]).unwrap();
    assert_eq!(shorter.len(), log.len() - 1);
}

#[test]
fn cursors_never_go_back_along_the_log() {
    let mut space = trio();
    let first = space.profile(0, "x");
    space.push(first);
    let cursor = space.cursor;
    // The same cursor is accepted (a batch whose cursors are not assigned yet), a lower one is not.
    let same = space.profile(1, "y");
    space.gap = 0;
    space.push(same);
    assert_eq!(space.cursor, cursor);
    let lower = space.profile(2, "z");
    let before = space.timeline.clone();
    let result = space.timeline.apply(cursor - 1, &lower, &space.world);
    assert_refused(result, space.timeline.len(), FoldRule::Chain);
    assert_eq!(space.timeline, before);
}

// ----- the signer ----------------------------------------------------------------------------

#[test]
fn only_a_device_of_the_header_account_signs() {
    let mut space = trio();
    let profile = space.profile(1, "x");
    let before = space.timeline.clone();

    // Signed by a device of another account while naming person 1.
    let forged = space.sign_header(profile.header.clone(), &Private::None, (0, 0));
    refuse(&mut space, forged, FoldRule::Signature);
    // Naming a device that is in nobody's log.
    let unknown = space.rebuild(&profile, |h| h.device = device(0x99));
    refuse(&mut space, unknown, FoldRule::Signature);
    // Naming a device of another account (person 0's) in person 1's header.
    let other_device = space.world.people[0].devices[0].id;
    let swapped = space.rebuild(&profile, |h| h.device = other_device);
    refuse(&mut space, swapped, FoldRule::Signature);
    // A flipped byte of the signature, of the ciphertext.
    let mut flipped = profile.clone();
    let mut signature = flipped.envelope.signature.unwrap().to_bytes();
    signature[3] ^= 1;
    flipped.envelope.signature = Some(Signature::from_bytes(&signature).unwrap());
    refuse(&mut space, flipped, FoldRule::Signature);
    let mut flipped = profile.clone();
    flipped.envelope.ciphertext[0] ^= 1;
    refuse(&mut space, flipped, FoldRule::Signature);
    assert_eq!(space.timeline, before);
    space.push(profile);
}

#[test]
fn an_account_whose_log_is_not_known_cannot_be_judged() {
    let mut space = trio();
    let profile = space.profile(1, "x");
    let before = space.timeline.clone();
    let result = space
        .timeline
        .apply_checked(space.next_cursor(), &profile, &NoLogs, &FAST);
    assert_refused(result, space.timeline.len(), FoldRule::UnknownLog);
    assert_eq!(space.timeline, before);
}

#[test]
fn a_device_revoked_later_still_signed_what_it_signed_before() {
    let mut space = trio();
    // Person 1's device 0 signs a Profile, then the log revokes it (and adds others).
    let before_revocation = space.profile(1, "before");
    space.push(before_revocation);
    space.world.people[1].version = 3;
    let revoked = space.world.people[1].devices[0].id;
    assert!(
        space.world.people[1]
            .log()
            .verify()
            .unwrap()
            .active(revoked)
            .is_none()
    );
    // Folding the log again from the start with the longer device log still works (the entry was
    // written while the device was active); the revoked device may even sign now: the server is the
    // one that refuses inactive devices at append time.
    let log = space.entries.clone();
    space.fold(&log).unwrap();
    let late = space.profile(1, "after");
    space.push(late);
}

#[test]
fn the_envelope_and_the_header_must_agree() {
    let mut space = trio();
    let profile = space.profile(0, "x");

    // Unsigned.
    let mut unsigned = profile.clone();
    unsigned.envelope.signature = None;
    refuse(&mut space, unsigned, FoldRule::Encoding);
    // Another envelope kind.
    let mut wrong_kind = profile.clone();
    wrong_kind.envelope.kind = EnvelopeKind::Entry;
    refuse(&mut space, wrong_kind, FoldRule::Encoding);
    // A header that is not the one the envelope carries.
    let mut other_header = profile.clone();
    other_header.header.at += 1;
    refuse(&mut space, other_header, FoldRule::Encoding);
    // More boxes than an entry may carry, or more heads than a space has members.
    let grant = space.grant(0, 1);
    let EntryBody::Grant { member, epoch, .. } = grant.header.body.clone() else {
        panic!("a grant")
    };
    let many = space.rebuild(&grant, |h| {
        h.body = EntryBody::Grant {
            member,
            epoch,
            boxes: (0..1025u16)
                .map(|i| {
                    let mut id = [0u8; 16];
                    id[..2].copy_from_slice(&i.to_le_bytes());
                    KeyBox {
                        device: DeviceId::from_bytes(id),
                        bytes: Vec::new(),
                    }
                })
                .collect(),
        };
    });
    refuse(&mut space, many, FoldRule::Encoding);
}

#[test]
fn nothing_follows_a_close() {
    let mut space = trio();
    let close = space.close(0);
    let applied = space.push(close);
    assert_eq!(applied.kind, EntryKind::Close);
    assert_eq!(space.state().closed, Some(space.cursor));
    let entries = [
        space.profile(0, "x"),
        space.leave(1),
        space.close(0),
        space.settings(0, SpaceSettings::default()),
    ];
    for entry in entries {
        refuse(&mut space, entry, rule(CLOSED));
    }
    let (invite, _) = space.invite(0, MemberRole::Viewer, None, DAY);
    refuse(&mut space, invite, rule(CLOSED));
}

// ----- invites ---------------------------------------------------------------------------------

#[test]
fn only_an_owner_writes_and_revokes_invites() {
    let mut space = trio();
    for (who, expected) in [
        (1, SIGNER_NOT_OWNER),
        (2, SIGNER_NOT_OWNER),
        (3, SIGNER_NOT_MEMBER),
    ] {
        let (invite, _) = space.invite(who, MemberRole::Viewer, None, DAY);
        refuse(&mut space, invite, rule(expected));
    }
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    let applied = space.push(invite);
    assert_eq!(applied.kind, EntryKind::Invite);
    assert_eq!(applied.subject, None);
    let id = invite_id(&secret);
    for (who, expected) in [
        (1, SIGNER_NOT_OWNER),
        (2, SIGNER_NOT_OWNER),
        (3, SIGNER_NOT_MEMBER),
    ] {
        let revoke = space.revoke(who, id);
        refuse(&mut space, revoke, rule(expected));
    }
    assert_eq!(space.state().open_invites().count(), 1);
    let revoke = space.revoke(0, id);
    space.push(revoke);
    assert_eq!(space.state().open_invites().count(), 0);
    assert_eq!(
        space.state().invite(id).unwrap().status,
        InviteRecordStatus::Revoked
    );
}

#[test]
fn an_invite_records_what_the_owner_wrote() {
    let mut space = trio();
    let at = space.clock;
    let (invite, secret) = space.invite(0, MemberRole::Commenter, Some(3), 2 * DAY);
    space.push(invite);
    let record = space.state().invite(invite_id(&secret)).unwrap();
    assert_eq!(record.role, MemberRole::Commenter);
    assert_eq!(record.expires_at, at + 2 * DAY);
    assert_eq!(record.bound_account, Some(space.account(3)));
    assert!(record.verified);
    assert_eq!(record.enc_bundle, None);
    assert_eq!(record.created_by, space.account(0));
    assert_eq!(record.created_at, at);
    assert_eq!(record.cursor, space.cursor);
    assert_eq!(
        record.invite_pub,
        invite_signing_key(&secret).verifying_key()
    );
    assert_eq!(record.status, InviteRecordStatus::Open);

    let (link, secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(link);
    let record = space.state().invite(invite_id(&secret)).unwrap();
    assert_eq!(record.bound_account, None);
    assert!(!record.verified);
    assert_eq!(record.enc_bundle, Some(vec![9; 40]));
}

#[test]
fn an_invite_cannot_grant_owner() {
    let mut space = trio();
    let (invite, _) = space.invite(0, MemberRole::Owner, None, DAY);
    refuse(&mut space, invite, rule(INVITE_GRANTS_OWNER));
    for role in ROLES {
        let (invite, _) = space.invite(0, role, None, DAY);
        space.push(invite);
    }
}

#[test]
fn an_invite_lives_at_most_thirty_days_after_its_entry() {
    let mut space = trio();
    let (too_far, _) = space.invite(0, MemberRole::Viewer, None, 30 * DAY + 1);
    refuse(&mut space, too_far, rule(INVITE_EXPIRY_TOO_FAR));
    let (limit, _) = space.invite(0, MemberRole::Viewer, None, 30 * DAY);
    space.push(limit);
    // An invite that expires before it was written is pointless but harmless.
    let (dead, secret) = space.invite(0, MemberRole::Viewer, None, -1);
    space.push(dead);
    assert!(
        space
            .state()
            .invite(invite_id(&secret))
            .unwrap()
            .is_expired(space.clock)
    );
}

#[test]
fn an_invite_id_is_used_once_in_the_log_whatever_became_of_it() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let again = space.invite_with(0, &secret, MemberRole::Editor, None, DAY);
    refuse(&mut space, again, rule(INVITE_ID_EXISTS));
    // Revoked.
    let revoke = space.revoke(0, invite_id(&secret));
    space.push(revoke);
    let again = space.invite_with(0, &secret, MemberRole::Editor, None, DAY);
    refuse(&mut space, again, rule(INVITE_ID_EXISTS));
    // Used.
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let join = space.join(3, &secret);
    space.push(join);
    let again = space.invite_with(0, &secret, MemberRole::Editor, None, DAY);
    refuse(&mut space, again, rule(INVITE_ID_EXISTS));
}

#[test]
fn at_most_twenty_invites_are_open_and_unexpired() {
    assert_eq!(MAX_OPEN_INVITES, 20);
    let mut space = trio();
    let mut secrets = Vec::new();
    for _ in 0..MAX_OPEN_INVITES {
        let (invite, secret) = space.invite(0, MemberRole::Viewer, None, 7 * DAY);
        space.push(invite);
        secrets.push(secret);
    }
    let (extra, _) = space.invite(0, MemberRole::Viewer, None, 7 * DAY);
    refuse(&mut space, extra, rule(TOO_MANY_OPEN_INVITES));

    // A revoked one makes room, a used one too.
    let revoke = space.revoke(0, invite_id(&secrets[0]));
    space.push(revoke);
    let join = space.join(3, &secrets[1]);
    space.push(join);
    let (one, _) = space.invite(0, MemberRole::Viewer, None, 7 * DAY);
    space.push(one);
    let (two, _) = space.invite(0, MemberRole::Viewer, None, 7 * DAY);
    space.push(two);
    let (three, _) = space.invite(0, MemberRole::Viewer, None, 7 * DAY);
    refuse(&mut space, three, rule(TOO_MANY_OPEN_INVITES));

    // Invites that are past their expiry as of the new entry no longer count.
    space.clock += 8 * DAY;
    let (late, _) = space.invite(0, MemberRole::Viewer, None, 7 * DAY);
    space.push(late);
}

#[test]
fn only_an_open_invite_is_revoked() {
    let mut space = trio();
    let unknown = space.revoke(0, invite_id_of(0x42));
    refuse(&mut space, unknown, rule(INVITE_UNKNOWN));
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let join = space.join(3, &secret);
    space.push(join);
    let used = space.revoke(0, invite_id(&secret));
    refuse(&mut space, used, rule(INVITE_USED));
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let revoke = space.revoke(0, invite_id(&secret));
    space.push(revoke);
    let twice = space.revoke(0, invite_id(&secret));
    refuse(&mut space, twice, rule(INVITE_REVOKED));
}

// ----- Join ----------------------------------------------------------------------------------

#[test]
fn a_join_adds_the_joiner_with_the_role_of_the_invite_and_consumes_it() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Commenter, None, DAY);
    space.push(invite);
    let join = space.join(3, &secret);
    let at = space.clock;
    let applied = space.push(join);
    assert_eq!(
        applied,
        Applied {
            kind: EntryKind::Join,
            signer: space.account(3),
            subject: Some(space.account(3)),
            previous_role: None,
        }
    );
    let state = space.state();
    assert_eq!(state.role_of(space.account(3)), Some(MemberRole::Commenter));
    let record = state.member(space.account(3)).unwrap();
    assert_eq!((record.joined_cursor, record.joined_at), (space.cursor, at));
    assert_eq!(
        state.members.last().unwrap().account,
        space.account(3),
        "members are in join order"
    );
    assert_eq!(
        state.invite(invite_id(&secret)).unwrap().status,
        InviteRecordStatus::Used {
            by: space.account(3)
        }
    );
    assert_eq!(state.open_invites().count(), 0);
}

#[test]
fn a_code_invite_is_bound_to_one_account() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Editor, Some(4), DAY);
    space.push(invite);
    let wrong = space.join(3, &secret);
    refuse(&mut space, wrong, rule(INVITE_BOUND_TO_OTHER_ACCOUNT));
    let right = space.join(4, &secret);
    space.push(right);
    assert_eq!(
        space.state().role_of(space.account(4)),
        Some(MemberRole::Editor)
    );
}

#[test]
fn a_forged_join_is_refused() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(invite);
    let id = invite_id(&secret);
    let join = space.join(3, &secret);
    let feed = space.feed;
    let account3 = space.account(3);
    let head = space.world.people[3].head();
    let other = secret_of(77);

    // The proof is signed by another invite key, or covers another feed, invite, account or head.
    let proofs = [
        space.proof_raw(&other, feed, id, account3, &head),
        space.proof_raw(&secret, FeedId::from_bytes([1; 16]), id, account3, &head),
        space.proof_raw(&secret, feed, invite_id_of(5), account3, &head),
        space.proof_raw(&secret, feed, id, space.account(4), &head),
        space.proof_raw(
            &secret,
            feed,
            id,
            account3,
            &ChainHead {
                seq: head.seq,
                hash: [8; 32],
            },
        ),
    ];
    for proof in proofs {
        let forged = space.rebuild(&join, |h| {
            if let EntryBody::Join { proof: p, .. } = &mut h.body {
                *p = proof;
            }
        });
        refuse(&mut space, forged, rule(JOIN_PROOF_INVALID));
    }
    // A zero proof.
    let zero = space.rebuild(&join, |h| {
        if let EntryBody::Join { proof, .. } = &mut h.body {
            *proof = Signature::from_bytes(&[0; 64]).unwrap();
        }
    });
    refuse(&mut space, zero, rule(JOIN_PROOF_INVALID));

    // A proof that is genuine for a head that is not a position of the joiner's log.
    let fake = ChainHead {
        seq: head.seq,
        hash: [8; 32],
    };
    let fake_head = space.rebuild(&join, |h| {
        if let EntryBody::Join { proof, log, .. } = &mut h.body {
            *log = fake;
            *proof = space.proof_raw(&secret, feed, id, account3, &fake);
        }
    });
    refuse(&mut space, fake_head, FoldRule::Heads);
    let beyond = ChainHead {
        seq: 9,
        hash: [8; 32],
    };
    let beyond_head = space.rebuild(&join, |h| {
        if let EntryBody::Join { proof, log, .. } = &mut h.body {
            *log = beyond;
            *proof = space.proof_raw(&secret, feed, id, account3, &beyond);
        }
    });
    refuse(&mut space, beyond_head, FoldRule::UnknownLog);

    // The account of the entry is not the account that signs.
    let other_account = space.account(4);
    let mismatch = space.rebuild(&join, |h| {
        if let EntryBody::Join { account, .. } = &mut h.body {
            *account = other_account;
        }
    });
    refuse(&mut space, mismatch, rule(JOIN_ACCOUNT_MISMATCH));

    // Signed by a device that is not in the joiner's log.
    let by_other_device = space.sign_header(join.header.clone(), &Private::None, (4, 0));
    refuse(&mut space, by_other_device, FoldRule::Signature);

    // The genuine one still folds.
    space.push(join);
}

#[test]
fn a_replayed_join_is_refused_every_way() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(invite);
    let join = space.join(3, &secret);
    space.push(join.clone());
    // The very same entry again: its position is taken.
    refuse(&mut space, join, FoldRule::Chain);
    // A fresh Join with the same invite: by the same account (already a member), by another (the
    // invite is used).
    let again = space.join(3, &secret);
    refuse(&mut space, again, rule(ALREADY_MEMBER));
    let other = space.join(4, &secret);
    refuse(&mut space, other, rule(INVITE_USED));
}

#[test]
fn a_join_needs_an_open_invite_of_the_log() {
    let mut space = trio();
    // Never written.
    let unknown = secret_of(5);
    let join = space.join(3, &unknown);
    refuse(&mut space, join, rule(INVITE_UNKNOWN));
    // Revoked.
    let (invite, secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(invite);
    let revoke = space.revoke(0, invite_id(&secret));
    space.push(revoke);
    let join = space.join(3, &secret);
    refuse(&mut space, join, rule(INVITE_REVOKED));
}

#[test]
fn clients_do_not_judge_expiry_the_server_does() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Editor, None, 1_000);
    space.push(invite);
    // Ten days later: the fold has no clock and folds the Join; the invite record says it expired,
    // which is what a server checks before it appends.
    space.clock += 10 * DAY;
    let join = space.join(3, &secret);
    let record = space.state().invite(invite_id(&secret)).unwrap();
    assert!(record.is_expired(join.header.at));
    assert!(!record.is_expired(record.created_at));
    assert!(
        record.is_expired(record.expires_at),
        "the instant itself is expired"
    );
    assert!(!record.is_expired(record.expires_at - 1));
    space.push(join);
}

#[test]
fn a_member_cannot_join_again_and_a_removed_one_can_with_a_new_invite() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(invite);
    let join = space.join(1, &secret);
    refuse(&mut space, join, rule(ALREADY_MEMBER));

    let remove = space.remove(0, 1);
    space.push(remove);
    assert_eq!(space.state().role_of(space.account(1)), None);
    // The removed account joins again with a new invite, as a new member at the end.
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let join = space.join(1, &secret);
    space.push(join);
    assert_eq!(
        space.state().role_of(space.account(1)),
        Some(MemberRole::Viewer)
    );
    assert_eq!(
        space.state().members.last().unwrap().account,
        space.account(1)
    );
}

#[test]
fn a_space_holds_at_most_fifty_members() {
    assert_eq!(MAX_MEMBERS_PER_FEED, 50);
    let mut space = Space::large(52, 3);
    let create = space.create(0);
    space.push(create);
    for person in 1..MAX_MEMBERS_PER_FEED {
        let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
        space.push(invite);
        let join = space.join(person, &secret);
        space.push(join);
    }
    assert_eq!(space.state().members.len(), MAX_MEMBERS_PER_FEED);
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let join = space.join(MAX_MEMBERS_PER_FEED, &secret);
    refuse(&mut space, join, rule(SPACE_FULL));
    // Someone leaving makes room.
    let leave = space.leave(7);
    space.push(leave);
    let join = space.join(MAX_MEMBERS_PER_FEED, &secret);
    space.push(join);
}
