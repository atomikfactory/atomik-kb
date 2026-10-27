//! The timeline: what the state was at a feed cursor, folding in chunks, errors, small accessors.

use super::*;

fn fast(people: usize) -> Space {
    Space::new(people, true, 31)
}

/// A story with explicit cursors (every entry five apart, from 5): a Create, an invite and the Join
/// of person 1, a promotion, a second invite and the Join of person 2, a Rekey, a demotion, a Remove
/// and a Leave.
fn story() -> (Space, Vec<(&'static str, u64)>) {
    let mut space = fast(4);
    space.gap = 5;
    let mut marks = Vec::new();
    let mut push = |space: &mut Space, name: &'static str, entry: MembershipEntry| {
        space.push(entry);
        marks.push((name, space.cursor));
    };
    let create = space.create(0);
    push(&mut space, "create", create);
    let (invite, secret_1) = space.invite(0, MemberRole::Editor, None, DAY);
    push(&mut space, "invite 1", invite);
    let join = space.join(1, &secret_1);
    push(&mut space, "join 1", join);
    let promote = space.role_set(0, 1, MemberRole::Owner);
    push(&mut space, "promote 1", promote);
    let (invite, secret_2) = space.invite(1, MemberRole::Viewer, None, DAY);
    push(&mut space, "invite 2", invite);
    let join = space.join(2, &secret_2);
    push(&mut space, "join 2", join);
    let rekey = space.rekey(2);
    push(&mut space, "rekey", rekey);
    let demote = space.role_set(1, 0, MemberRole::Editor);
    push(&mut space, "demote 0", demote);
    let remove = space.remove(1, 2);
    push(&mut space, "remove 2", remove);
    let leave = space.leave(0);
    push(&mut space, "leave 0", leave);
    (space, marks)
}

fn cursor_of(marks: &[(&str, u64)], name: &str) -> u64 {
    marks.iter().find(|(n, _)| *n == name).unwrap().1
}

#[test]
fn roles_are_answered_as_of_every_entry_up_to_a_cursor() {
    let (space, marks) = story();
    let t = &space.timeline;
    let (a0, a1, a2, a3) = (
        space.account(0),
        space.account(1),
        space.account(2),
        space.account(3),
    );
    let at = |name: &str| cursor_of(&marks, name);

    // Before the Create nobody is anything.
    assert_eq!(t.role_at(a0, at("create") - 1), None);
    assert_eq!(t.role_at(a0, 0), None);
    // Entry by entry, and between the entries (the state after the last one before the cursor).
    let table: Vec<(u64, [Option<MemberRole>; 3])> = vec![
        (at("create"), [Some(MemberRole::Owner), None, None]),
        (at("invite 1"), [Some(MemberRole::Owner), None, None]),
        (at("join 1") - 1, [Some(MemberRole::Owner), None, None]),
        (
            at("join 1"),
            [Some(MemberRole::Owner), Some(MemberRole::Editor), None],
        ),
        (
            at("promote 1") - 1,
            [Some(MemberRole::Owner), Some(MemberRole::Editor), None],
        ),
        (
            at("promote 1"),
            [Some(MemberRole::Owner), Some(MemberRole::Owner), None],
        ),
        (
            at("join 2"),
            [
                Some(MemberRole::Owner),
                Some(MemberRole::Owner),
                Some(MemberRole::Viewer),
            ],
        ),
        (
            at("demote 0"),
            [
                Some(MemberRole::Editor),
                Some(MemberRole::Owner),
                Some(MemberRole::Viewer),
            ],
        ),
        (
            at("remove 2") - 1,
            [
                Some(MemberRole::Editor),
                Some(MemberRole::Owner),
                Some(MemberRole::Viewer),
            ],
        ),
        (
            at("remove 2"),
            [Some(MemberRole::Editor), Some(MemberRole::Owner), None],
        ),
        (at("leave 0"), [None, Some(MemberRole::Owner), None]),
        (at("leave 0") + 1000, [None, Some(MemberRole::Owner), None]),
    ];
    for (cursor, expected) in table {
        for (person, want) in [a0, a1, a2].into_iter().zip(expected) {
            assert_eq!(t.role_at(person, cursor), want, "cursor {cursor}");
            assert_eq!(t.is_member_at(person, cursor), want.is_some());
        }
    }
    assert_eq!(
        t.role_at(a3, u64::MAX),
        None,
        "an outsider is never anything"
    );
}

#[test]
fn epochs_and_owners_are_answered_the_same_way() {
    let (space, marks) = story();
    let t = &space.timeline;
    let at = |name: &str| cursor_of(&marks, name);
    let (a0, a1) = (space.account(0), space.account(1));

    assert_eq!(t.epoch_at(at("create") - 1), None);
    assert_eq!(t.epoch_at(at("create")), Some(1));
    assert_eq!(t.epoch_at(at("rekey") - 1), Some(1));
    assert_eq!(t.epoch_at(at("rekey")), Some(2));
    assert_eq!(t.epoch_at(at("remove 2") - 1), Some(2));
    assert_eq!(t.epoch_at(at("remove 2")), Some(3));
    assert_eq!(t.epoch_at(u64::MAX), Some(3));
    assert_eq!(t.state().epoch, 3);

    assert_eq!(t.owners_at(at("create") - 1), Vec::<AccountId>::new());
    assert_eq!(t.owners_at(at("create")), vec![a0]);
    assert_eq!(t.owners_at(at("promote 1") - 1), vec![a0]);
    assert_eq!(t.owners_at(at("promote 1")), vec![a0, a1], "in join order");
    assert_eq!(t.owners_at(at("demote 0")), vec![a1]);
    assert_eq!(t.owners_at(u64::MAX), vec![a1]);
    assert_eq!(t.state().owners(), vec![a1]);
}

#[test]
fn owners_keep_their_place_in_the_join_order_through_role_changes() {
    let mut space = fast(4);
    space.start(0, &[(1, MemberRole::Editor), (2, MemberRole::Editor)]);
    // 2 becomes Owner, then 1: the order is the join order (0, 1, 2), not the promotion order.
    for target in [2, 1] {
        let promote = space.role_set(0, target, MemberRole::Owner);
        space.push(promote);
    }
    let (a0, a1, a2) = (space.account(0), space.account(1), space.account(2));
    assert_eq!(space.timeline.owners_at(u64::MAX), vec![a0, a1, a2]);
    assert_eq!(space.state().owners(), vec![a0, a1, a2]);
    // Removed and back, a member joins again at the end.
    let remove = space.remove(0, 1);
    space.push(remove);
    let (invite, secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(invite);
    let join = space.join(1, &secret);
    space.push(join);
    let promote = space.role_set(0, 1, MemberRole::Owner);
    space.push(promote);
    assert_eq!(space.timeline.owners_at(u64::MAX), vec![a0, a2, a1]);
    assert_eq!(space.state().owners(), vec![a0, a2, a1]);
}

#[test]
fn entries_that_share_a_cursor_are_all_applied_at_it() {
    // A batch whose cursors are not assigned yet: every entry has cursor 7.
    let mut space = fast(3);
    space.gap = 0;
    space.cursor = 7;
    space.start(0, &[(1, MemberRole::Editor)]);
    let a1 = space.account(1);
    assert_eq!(space.timeline.role_at(a1, 7), Some(MemberRole::Editor));
    assert_eq!(space.timeline.role_at(a1, 6), None);
    assert_eq!(space.timeline.epoch_at(7), Some(1));
}

#[test]
fn folding_all_at_once_one_by_one_or_in_chunks_gives_the_same_timeline() {
    let (space, _) = story();
    let log = space.entries.clone();
    let batch = space.fold(&log).unwrap();
    assert_eq!(batch, space.timeline);
    assert_eq!(batch.len(), log.len());

    for split in 0..=log.len() {
        let mut timeline = space.fold(&log[..split]).unwrap();
        for (cursor, entry) in &log[split..] {
            space.apply_to(&mut timeline, *cursor, entry).unwrap();
        }
        assert_eq!(timeline, batch, "split at {split}");
    }
    // A clone of a timeline goes on independently.
    let mut early = space.fold(&log[..3]).unwrap();
    let snapshot = early.clone();
    let (cursor, entry) = &log[3];
    space.apply_to(&mut early, *cursor, entry).unwrap();
    assert_ne!(early, snapshot);
    assert_eq!(snapshot.len(), 3);
}

#[test]
fn an_empty_log_is_an_empty_timeline() {
    let feed = FeedId::from_bytes([5; 16]);
    let timeline = fold(feed, &[], &NoLogs).unwrap();
    assert!(timeline.is_empty());
    assert_eq!(timeline, Timeline::new(feed));
    assert_eq!(timeline.len(), 0);
    assert_eq!(timeline.role_at(account(1), 100), None);
    assert!(!timeline.is_member_at(account(1), 100));
    assert_eq!(timeline.epoch_at(100), None);
    assert!(timeline.owners_at(100).is_empty());
    let state = timeline.state();
    assert_eq!(state, &MembershipState::empty(feed));
    assert_eq!((state.epoch, state.head, state.closed), (0, None, None));
    assert!(state.members.is_empty() && state.invites.is_empty());
    assert!(!state.current_key_matches(&SpaceKey::from_secret(Secret32::from_bytes([1; 32]))));
}

#[test]
fn an_error_names_the_entry_and_a_fold_the_index_in_the_slice() {
    let (mut space, _) = story();
    // The next entry is a Profile by someone who left.
    let bad = space.profile(0, "x");
    let before = space.timeline.clone();
    let index = space.timeline.len();
    let result = space.submit(bad.clone());
    assert_refused(result, index, FoldRule::Rule(rules::SIGNER_NOT_MEMBER));
    assert_eq!(space.timeline, before);

    // `fold` counts from the start of the slice it was given.
    let mut log = space.entries.clone();
    log.push((space.next_cursor(), bad));
    let error = space.fold(&log).unwrap_err();
    assert_eq!(error.index, log.len() - 1);
    assert_eq!(error.rule, FoldRule::Rule(rules::SIGNER_NOT_MEMBER));
    assert!(error.to_string().contains(&(log.len() - 1).to_string()));
    // The error carries no data of the entry.
    let text = format!("{error} {error:?}");
    assert!(!text.contains("Person") && text.len() < 200, "{text}");
    // A log that does not start with a Create fails at 0.
    let error = space.fold(&space.entries[1..]).unwrap_err();
    assert_eq!((error.index, error.rule), (0, FoldRule::Chain));
}

#[test]
fn the_state_answers_for_invites_keys_and_heads() {
    let mut space = fast(3);
    space.start(0, &[(1, MemberRole::Editor)]);
    let (invite, secret) = space.invite(0, MemberRole::Viewer, Some(2), DAY);
    space.push(invite);
    let id = crate::invite::invite_id(&secret);
    let state = space.state();
    assert_eq!(state.invite(id).unwrap().invite_id, id);
    assert!(state.invite(invite_id_of(0x77)).is_none());
    assert_eq!(state.open_invites().count(), 1);
    assert!(state.member(space.account(1)).is_some());
    assert!(state.member(space.account(2)).is_none());

    // The current key, by its commitment.
    assert!(state.current_key_matches(&space.keys[&1]));
    assert!(!state.current_key_matches(&SpaceKey::from_secret(Secret32::from_bytes([1; 32]))));
    let rekey = space.rekey(0);
    space.push(rekey);
    assert!(space.state().current_key_matches(&space.keys[&2]));
    assert!(
        !space.state().current_key_matches(&space.keys[&1]),
        "the old key is not current"
    );

    // The head names the last entry.
    let (cursor, last) = space.entries.last().unwrap();
    assert_eq!(space.state().head, Some(last.head()));
    assert_eq!(*cursor, space.cursor);
}
