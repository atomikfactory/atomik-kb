//! The fold: roles, `Remove`, `Leave`, `Rekey`, `Grant`, `Profile`, `Settings` and `Close`.

use super::*;
use rules::*;

fn fast(people: usize) -> Space {
    Space::new(people, true, 21)
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

fn rekey_of(body: &mut EntryBody) -> &mut RekeyFields {
    match body {
        EntryBody::Remove { rekey, .. } | EntryBody::Rekey(rekey) => rekey,
        other => panic!("{other:?} carries no rekey"),
    }
}

fn dev(space: &Space, person: usize, device: usize) -> DeviceId {
    space.world.people[person].devices[device].id
}

// ----- RoleSet -------------------------------------------------------------------------------

#[test]
fn an_owner_sets_roles_and_all_owners_are_equal() {
    let mut space = trio();
    let promote = space.role_set(0, 1, MemberRole::Owner);
    let applied = space.push(promote);
    assert_eq!(
        applied,
        Applied {
            kind: EntryKind::RoleSet,
            signer: space.account(0),
            subject: Some(space.account(1)),
            previous_role: Some(MemberRole::Editor),
        }
    );
    // The promoted Owner demotes the creator, who then is no Owner at all.
    let demote = space.role_set(1, 0, MemberRole::Editor);
    space.push(demote);
    assert_eq!(
        space.roster(),
        [
            (0, MemberRole::Editor),
            (1, MemberRole::Owner),
            (2, MemberRole::Viewer)
        ]
    );
    let (invite, _) = space.invite(0, MemberRole::Viewer, None, DAY);
    refuse(&mut space, invite, rule(SIGNER_NOT_OWNER));
    let promote_back = space.role_set(0, 0, MemberRole::Owner);
    refuse(&mut space, promote_back, rule(SIGNER_NOT_OWNER));
}

#[test]
fn only_an_owner_sets_roles_and_only_for_members() {
    let mut space = trio();
    for (who, expected) in [
        (1, SIGNER_NOT_OWNER),
        (2, SIGNER_NOT_OWNER),
        (3, SIGNER_NOT_MEMBER),
    ] {
        // Including a self-promotion.
        let entry = space.role_set(who, who.min(2), MemberRole::Owner);
        refuse(&mut space, entry, rule(expected));
    }
    let outsider = space.role_set(0, 3, MemberRole::Editor);
    refuse(&mut space, outsider, rule(TARGET_NOT_MEMBER));
}

#[test]
fn the_last_owner_cannot_be_demoted_not_even_by_themself() {
    let mut space = trio();
    let demote_self = space.role_set(0, 0, MemberRole::Editor);
    refuse(&mut space, demote_self, rule(LAST_OWNER));
    // Two Owners: one may step down, the other is then the last.
    let promote = space.role_set(0, 1, MemberRole::Owner);
    space.push(promote);
    let step_down = space.role_set(0, 0, MemberRole::Viewer);
    space.push(step_down);
    assert_eq!(space.state().owners(), vec![space.account(1)]);
    let last = space.role_set(1, 1, MemberRole::Commenter);
    refuse(&mut space, last, rule(LAST_OWNER));
    // And nobody else can do it for them.
    let by_the_viewer = space.role_set(0, 1, MemberRole::Viewer);
    refuse(&mut space, by_the_viewer, rule(SIGNER_NOT_OWNER));
}

#[test]
fn setting_the_role_a_member_has_changes_nothing() {
    let mut space = trio();
    let before = space.roster();
    let same = space.role_set(0, 1, MemberRole::Editor);
    let applied = space.push(same);
    assert_eq!(applied.previous_role, Some(MemberRole::Editor));
    assert_eq!(space.roster(), before);
    // Also for the only Owner.
    let owner = space.role_set(0, 0, MemberRole::Owner);
    space.push(owner);
    assert_eq!(space.state().owners(), vec![space.account(0)]);
}

#[test]
fn a_demoted_owners_invites_are_void_and_the_others_stay() {
    let mut space = trio();
    let promote = space.role_set(0, 1, MemberRole::Owner);
    space.push(promote);
    let (by_zero, zero_secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(by_zero);
    let (by_one, one_secret) = space.invite(1, MemberRole::Editor, None, DAY);
    space.push(by_one);

    let demote = space.role_set(1, 0, MemberRole::Editor);
    space.push(demote);
    let state = space.state();
    assert_eq!(
        state
            .invite(crate::invite::invite_id(&zero_secret))
            .unwrap()
            .status,
        InviteRecordStatus::Revoked
    );
    assert_eq!(
        state
            .invite(crate::invite::invite_id(&one_secret))
            .unwrap()
            .status,
        InviteRecordStatus::Open
    );
    let join = space.join(3, &zero_secret);
    refuse(&mut space, join, rule(INVITE_REVOKED));
    let join = space.join(3, &one_secret);
    space.push(join);
}

#[test]
fn a_promotion_or_a_change_between_other_roles_leaves_invites_open() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(invite);
    for (target, role) in [
        (1, MemberRole::Owner),
        (1, MemberRole::Commenter),
        (2, MemberRole::Editor),
    ] {
        let entry = space.role_set(0, target, role);
        space.push(entry);
    }
    assert_eq!(space.state().open_invites().count(), 1);
    let join = space.join(3, &secret);
    space.push(join);
}

// ----- Remove --------------------------------------------------------------------------------

#[test]
fn an_owner_removes_a_member_with_the_rekey_that_locks_them_out() {
    let mut space = trio();
    let remove = space.remove(0, 2);
    let epoch_before = space.state().epoch;
    let applied = space.push(remove);
    assert_eq!(
        applied,
        Applied {
            kind: EntryKind::Remove,
            signer: space.account(0),
            subject: Some(space.account(2)),
            previous_role: Some(MemberRole::Viewer),
        }
    );
    let state = space.state();
    assert_eq!(state.role_of(space.account(2)), None);
    assert_eq!(state.epoch, epoch_before + 1);
    assert_eq!(
        state.sk_commitment,
        space.keys[&2].commitment(space.feed, 2)
    );
    assert!(!state.pending_rekey);
    assert_eq!(
        space.roster(),
        [(0, MemberRole::Owner), (1, MemberRole::Editor)]
    );
}

#[test]
fn only_an_owner_removes_and_never_themself() {
    let mut space = trio();
    for (who, target, expected) in [
        (1, 2, SIGNER_NOT_OWNER),
        (2, 1, SIGNER_NOT_OWNER),
        (3, 1, SIGNER_NOT_MEMBER),
        (0, 0, REMOVE_SELF),
        (0, 3, TARGET_NOT_MEMBER),
    ] {
        let entry = space.remove(who, target);
        refuse(&mut space, entry, rule(expected));
    }
}

#[test]
fn a_remove_carries_a_complete_rekey_for_the_members_that_remain() {
    let mut space = trio();
    let remove = space.remove(0, 2);
    let removed_device = dev(&space, 2, 0);
    let editor_device = dev(&space, 1, 0);
    let outsider_device = dev(&space, 3, 0);
    let remaining = [space.account(0), space.account(1)];
    let heads = |s: &Space| -> Vec<MemberHead> {
        remaining
            .iter()
            .map(|a| {
                let p = s.world.people.iter().find(|p| p.account == *a).unwrap();
                MemberHead {
                    account: *a,
                    head: p.head(),
                }
            })
            .collect()
    };
    let good_heads = heads(&space);

    type Edit = Box<dyn Fn(&mut MembershipHeader)>;
    let cases: Vec<(&str, Edit, FoldRule)> = vec![
        (
            "the epoch does not move",
            Box::new(|h| rekey_of(&mut h.body).new_epoch = 1),
            FoldRule::Completeness,
        ),
        (
            "the epoch skips one",
            Box::new(|h| rekey_of(&mut h.body).new_epoch = 3),
            FoldRule::Completeness,
        ),
        (
            "a box is missing",
            Box::new(|h| {
                rekey_of(&mut h.body).boxes.pop();
            }),
            FoldRule::Completeness,
        ),
        (
            "the removed member is boxed",
            Box::new(move |h| {
                rekey_of(&mut h.body).boxes.push(KeyBox {
                    device: removed_device,
                    bytes: vec![0; 104],
                });
            }),
            FoldRule::Completeness,
        ),
        (
            "an outsider is boxed",
            Box::new(move |h| {
                rekey_of(&mut h.body).boxes.push(KeyBox {
                    device: outsider_device,
                    bytes: vec![0; 104],
                });
            }),
            FoldRule::Completeness,
        ),
        (
            "a box twice",
            Box::new(move |h| {
                let again = rekey_of(&mut h.body).boxes[0].clone();
                rekey_of(&mut h.body).boxes.push(again);
            }),
            FoldRule::Completeness,
        ),
        (
            "a box for the wrong device",
            Box::new(move |h| rekey_of(&mut h.body).boxes[0].device = editor_device),
            FoldRule::Completeness,
        ),
        (
            "a head is missing",
            Box::new(|h| {
                rekey_of(&mut h.body).member_heads.pop();
            }),
            FoldRule::Completeness,
        ),
        (
            "the removed member has a head",
            Box::new({
                let removed = space.account(2);
                let head = space.world.people[2].head();
                move |h| {
                    rekey_of(&mut h.body).member_heads.push(MemberHead {
                        account: removed,
                        head,
                    });
                }
            }),
            FoldRule::Completeness,
        ),
        (
            "a head twice",
            Box::new({
                let first = good_heads[0];
                move |h| rekey_of(&mut h.body).member_heads[1] = first
            }),
            FoldRule::Completeness,
        ),
        (
            "a head that is not the log's",
            Box::new(|h| rekey_of(&mut h.body).member_heads[0].head.hash = [7; 32]),
            FoldRule::Heads,
        ),
        (
            "a head beyond the log",
            Box::new(|h| rekey_of(&mut h.body).member_heads[0].head.seq = 40),
            FoldRule::UnknownLog,
        ),
        (
            "a private part sealed under an epoch from the future",
            Box::new(|h| h.meta_epoch = 9),
            rule(META_EPOCH),
        ),
    ];
    for (what, edit, expected) in cases {
        let forged = space.rebuild(&remove, |h| edit(h));
        let before = space.timeline.clone();
        assert_refused(space.submit(forged), before.len(), expected);
        assert_eq!(space.timeline, before, "{what}");
    }
    space.push(remove);
}

#[test]
fn a_removed_member_is_gone_until_a_new_join() {
    let mut space = trio();
    let before_removal = space.cursor;
    let remove = space.remove(0, 2);
    space.push(remove);
    let after = space.cursor;
    let entries = [
        space.profile(2, "x"),
        space.leave(2),
        space.role_set(2, 2, MemberRole::Owner),
        space.grant(2, 1),
        space.close(2),
    ];
    for entry in entries {
        refuse(&mut space, entry, rule(SIGNER_NOT_MEMBER));
    }
    let rekey = space.rekey(2);
    refuse(&mut space, rekey, rule(SIGNER_NOT_MEMBER));
    // The timeline still knows what the member was when it was one.
    assert_eq!(
        space.timeline.role_at(space.account(2), before_removal),
        Some(MemberRole::Viewer)
    );
    assert_eq!(space.timeline.role_at(space.account(2), after), None);
}

#[test]
fn two_owners_removing_each_other_the_first_one_wins() {
    let mut space = trio();
    let promote = space.role_set(0, 1, MemberRole::Owner);
    space.push(promote);
    let a_removes_b = space.remove(0, 1);
    let b_removes_a = space.remove(1, 0);
    space.push(a_removes_b);
    // The other was written against the same head: it lost the compare-and-swap.
    refuse(&mut space, b_removes_a, FoldRule::Chain);
    // Written again against the new head it fails because its signer is no longer a member.
    let again = space.remove(1, 0);
    refuse(&mut space, again, rule(SIGNER_NOT_MEMBER));
    assert_eq!(space.state().owners(), vec![space.account(0)]);
    assert_eq!(
        space.roster(),
        [(0, MemberRole::Owner), (2, MemberRole::Viewer)]
    );
}

#[test]
fn a_removed_owners_invites_are_void_and_the_others_stay() {
    let mut space = trio();
    let promote = space.role_set(0, 1, MemberRole::Owner);
    space.push(promote);
    let (by_zero, zero_secret) = space.invite(0, MemberRole::Editor, None, DAY);
    space.push(by_zero);
    let (by_one, one_secret) = space.invite(1, MemberRole::Editor, None, DAY);
    space.push(by_one);
    let remove = space.remove(0, 1);
    space.push(remove);
    // The removed Owner still knows the secret of its link: it opens nothing.
    let join = space.join(3, &one_secret);
    refuse(&mut space, join, rule(INVITE_REVOKED));
    let join = space.join(3, &zero_secret);
    space.push(join);
}

#[test]
fn a_remove_answers_a_pending_rekey() {
    let mut space = trio();
    let leave = space.leave(2);
    space.push(leave);
    assert!(space.state().pending_rekey);
    let remove = space.remove(0, 1);
    space.push(remove);
    assert!(!space.state().pending_rekey);
}

// ----- Leave ---------------------------------------------------------------------------------

#[test]
fn a_member_leaves_and_a_rekey_becomes_due() {
    let mut space = trio();
    let leave = space.leave(2);
    let applied = space.push(leave);
    assert_eq!(
        applied,
        Applied {
            kind: EntryKind::Leave,
            signer: space.account(2),
            subject: Some(space.account(2)),
            previous_role: Some(MemberRole::Viewer),
        }
    );
    assert_eq!(
        space.roster(),
        [(0, MemberRole::Owner), (1, MemberRole::Editor)]
    );
    assert!(space.state().pending_rekey);
    assert_eq!(space.state().epoch, 1, "the next writer mints the Rekey");
    // The leaver cannot leave twice or write anything.
    let again = space.leave(2);
    refuse(&mut space, again, rule(SIGNER_NOT_MEMBER));
    let profile = space.profile(2, "x");
    refuse(&mut space, profile, rule(SIGNER_NOT_MEMBER));
    // Nor can an outsider.
    let outsider = space.leave(3);
    refuse(&mut space, outsider, rule(SIGNER_NOT_MEMBER));
}

#[test]
fn the_next_rekey_clears_the_pending_one() {
    let mut space = trio();
    let leave = space.leave(2);
    space.push(leave);
    // Any remaining member may mint it, a Viewer included once there is one.
    let rekey = space.rekey(1);
    space.push(rekey);
    assert!(!space.state().pending_rekey);
    assert_eq!(space.state().epoch, 2);
}

#[test]
fn the_last_owner_cannot_leave() {
    let mut space = trio();
    let leave = space.leave(0);
    refuse(&mut space, leave, rule(LAST_OWNER));
    let promote = space.role_set(0, 1, MemberRole::Owner);
    space.push(promote);
    let leave = space.leave(0);
    space.push(leave);
    // The remaining Owner is the last.
    let leave = space.leave(1);
    refuse(&mut space, leave, rule(LAST_OWNER));
}

#[test]
fn a_departing_owners_invites_are_void_and_the_member_may_return() {
    let mut space = trio();
    let promote = space.role_set(0, 1, MemberRole::Owner);
    space.push(promote);
    let (invite, secret) = space.invite(1, MemberRole::Editor, None, DAY);
    space.push(invite);
    let leave = space.leave(1);
    space.push(leave);
    let join = space.join(3, &secret);
    refuse(&mut space, join, rule(INVITE_REVOKED));
    // The departed account comes back with a new invite, as what that invite grants.
    let (invite, secret) = space.invite(0, MemberRole::Commenter, None, DAY);
    space.push(invite);
    let join = space.join(1, &secret);
    space.push(join);
    assert_eq!(
        space.state().role_of(space.account(1)),
        Some(MemberRole::Commenter)
    );
}

// ----- Rekey ---------------------------------------------------------------------------------

#[test]
fn any_member_may_rekey_a_viewer_included() {
    let mut space = trio();
    for who in [2, 1, 0] {
        let epoch = space.state().epoch;
        let rekey = space.rekey(who);
        let applied = space.push(rekey);
        assert_eq!(applied.kind, EntryKind::Rekey);
        assert_eq!(applied.signer, space.account(who));
        assert_eq!(space.state().epoch, epoch + 1);
        let key = &space.keys[&(epoch + 1)];
        assert_eq!(
            space.state().sk_commitment,
            key.commitment(space.feed, epoch + 1)
        );
    }
    // Not an outsider.
    let rekey = space.rekey(3);
    refuse(&mut space, rekey, rule(SIGNER_NOT_MEMBER));
}

#[test]
fn a_rekey_covers_the_active_devices_of_every_member_as_of_its_heads() {
    let mut space = trio();
    // Members' logs grow: the editor adds two devices, the viewer revokes its first.
    space.world.people[1].version = 2;
    space.world.people[2].version = 3;
    let rekey = space.rekey(0);
    let members: Vec<AccountId> = space.state().members.iter().map(|m| m.account).collect();
    let EntryBody::Rekey(fields) = &rekey.header.body else {
        panic!("a rekey")
    };
    let boxed: std::collections::BTreeSet<DeviceId> =
        fields.boxes.iter().map(|b| b.device).collect();
    assert_eq!(boxed, space.expected_devices(&members));
    assert_eq!(boxed.len(), 1 + 3 + 2, "d0; d0 d1 d2; d1 d2");
    space.push(rekey);
}

#[test]
fn a_rekey_that_leaves_a_device_out_or_boxes_a_revoked_one_is_refused() {
    let mut space = trio();
    space.world.people[2].version = 3; // device 0 revoked, devices 1 and 2 active
    let rekey = space.rekey(0);
    let revoked = dev(&space, 2, 0);
    let editor_device = dev(&space, 1, 0);
    let stranger = dev(&space, 3, 0);
    let viewer_boxes: Vec<usize> = match &rekey.header.body {
        EntryBody::Rekey(f) => f
            .boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.device == dev(&space, 2, 1))
            .map(|(i, _)| i)
            .collect(),
        _ => panic!("a rekey"),
    };
    assert_eq!(viewer_boxes.len(), 1);
    let at = viewer_boxes[0];

    type Edit = Box<dyn Fn(&mut RekeyFields)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "a device left out",
            Box::new(move |f| drop(f.boxes.remove(at))),
        ),
        (
            "a revoked device boxed instead",
            Box::new(move |f| f.boxes[at].device = revoked),
        ),
        (
            "a revoked device boxed as well",
            Box::new(move |f| {
                f.boxes.push(KeyBox {
                    device: revoked,
                    bytes: vec![0; 104],
                })
            }),
        ),
        (
            "a stranger boxed as well",
            Box::new(move |f| {
                f.boxes.push(KeyBox {
                    device: stranger,
                    bytes: vec![0; 104],
                })
            }),
        ),
        (
            "a device boxed twice",
            Box::new(move |f| {
                f.boxes.push(KeyBox {
                    device: editor_device,
                    bytes: vec![0; 104],
                })
            }),
        ),
        ("no boxes at all", Box::new(|f| f.boxes.clear())),
    ];
    for (what, edit) in cases {
        let forged = space.rebuild(&rekey, |h| edit(rekey_of(&mut h.body)));
        let before = space.timeline.clone();
        assert_refused(space.submit(forged), before.len(), FoldRule::Completeness);
        assert_eq!(space.timeline, before, "{what}");
    }
    space.push(rekey);
}

#[test]
fn a_rekey_names_every_member_once_and_the_next_epoch() {
    let mut space = trio();
    let rekey = space.rekey(0);
    let outsider_head = space.world.people[3].head();
    let outsider = space.account(3);
    type Edit = Box<dyn Fn(&mut RekeyFields)>;
    let cases: Vec<Edit> = vec![
        Box::new(|f| f.new_epoch = 1),
        Box::new(|f| f.new_epoch = 3),
        Box::new(|f| {
            f.member_heads.pop();
        }),
        Box::new(|f| f.member_heads.clear()),
        Box::new(move |f| {
            f.member_heads.push(MemberHead {
                account: outsider,
                head: outsider_head,
            })
        }),
        Box::new(|f| {
            let again = f.member_heads[0];
            f.member_heads[1] = again;
        }),
    ];
    for edit in cases {
        let forged = space.rebuild(&rekey, |h| edit(rekey_of(&mut h.body)));
        let before = space.timeline.clone();
        assert_refused(space.submit(forged), before.len(), FoldRule::Completeness);
        assert_eq!(space.timeline, before);
    }
    // A private part sealed under the new epoch is fine, a later one is not.
    let ahead = space.rebuild(&rekey, |h| h.meta_epoch = 3);
    refuse(&mut space, ahead, rule(META_EPOCH));
    let at_new = space.rebuild(&rekey, |h| h.meta_epoch = 2);
    space.push(at_new);
}

#[test]
fn heads_never_go_back_across_rekeys_and_removes() {
    let mut space = trio();
    space.world.people[1].version = 2;
    let first = space.rekey(0);
    space.push(first);
    assert_eq!(space.timeline.head_seq(space.account(1)), Some(2));
    assert_eq!(space.timeline.head_seq(space.account(0)), Some(0));
    assert_eq!(space.timeline.head_seq(space.account(3)), None);

    // A device whose copy of the editor's log is older than the one the first Rekey was made from
    // mints a Rekey from that older head: complete as of it, refused all the same.
    space.world.people[1].version = 1;
    let stale = space.rekey(0);
    refuse(&mut space, stale, FoldRule::Heads);
    // A Remove obeys the same rule.
    let remove = space.remove(0, 2);
    refuse(&mut space, remove, FoldRule::Heads);

    // The same head again is fine (monotone, not strictly increasing), and a newer one too.
    space.world.people[1].version = 2;
    let same = space.rekey(0);
    space.push(same);
    space.world.people[1].version = 4;
    let newer = space.rekey(0);
    space.push(newer);
    assert_eq!(space.timeline.head_seq(space.account(1)), Some(4));
    // Heads of the accounts that left the list are remembered: a member who is removed and comes
    // back cannot be boxed from an older log than before.
    let remove = space.remove(0, 1);
    space.push(remove);
    assert_eq!(space.timeline.head_seq(space.account(1)), Some(4));
}

#[test]
fn a_rekey_cannot_be_judged_without_the_logs_of_the_members() {
    let mut space = trio();
    let rekey = space.rekey(0);
    // A source that knows the signer but not the others.
    let signer_log = space.world.people[0].log().clone();
    let source = Logs(vec![(space.account(0), signer_log)]);
    let before = space.timeline.clone();
    let result = space
        .timeline
        .apply_checked(space.next_cursor(), &rekey, &source, &FAST);
    assert_refused(result, before.len(), FoldRule::UnknownLog);
    assert_eq!(space.timeline, before);
}

// ----- Grant ---------------------------------------------------------------------------------

#[test]
fn a_member_grants_the_current_key_to_a_member_who_joined_with_an_older_bundle() {
    let mut space = trio();
    let rekey = space.rekey(0);
    space.push(rekey);
    let grant = space.grant(0, 2);
    let applied = space.push(grant);
    assert_eq!(
        applied,
        Applied {
            kind: EntryKind::Grant,
            signer: space.account(0),
            subject: Some(space.account(2)),
            previous_role: Some(MemberRole::Viewer),
        }
    );
    // The state does not change: a Grant opens nothing for anyone else.
    assert_eq!(space.state().epoch, 2);
    assert_eq!(space.roster().len(), 3);
}

#[test]
fn a_grant_is_for_the_current_epoch_and_once_per_member_and_epoch() {
    let mut space = trio();
    let grant = space.grant(0, 2);
    space.push(grant);
    let again = space.grant(1, 2);
    refuse(&mut space, again, rule(GRANT_EXISTS));
    // Another member of the same epoch is fine.
    let other = space.grant(0, 1);
    space.push(other);
    // A new epoch allows a new Grant.
    let rekey = space.rekey(0);
    space.push(rekey);
    let again = space.grant(1, 2);
    space.push(again);
    // The wrong epoch, either way.
    let grant = space.grant(0, 0);
    let EntryBody::Grant { member, boxes, .. } = grant.header.body.clone() else {
        panic!("a grant")
    };
    for epoch in [1u32, 3] {
        let wrong = space.rebuild(&grant, |h| {
            h.body = EntryBody::Grant {
                member,
                epoch,
                boxes: boxes.clone(),
            };
        });
        refuse(&mut space, wrong, rule(GRANT_NOT_CURRENT_EPOCH));
    }
}

#[test]
fn a_grant_needs_a_member_a_signer_who_is_one_and_boxes() {
    let mut space = trio();
    let to_outsider = space.grant(0, 3);
    refuse(&mut space, to_outsider, rule(TARGET_NOT_MEMBER));
    let from_outsider = space.grant(3, 2);
    refuse(&mut space, from_outsider, rule(SIGNER_NOT_MEMBER));

    let grant = space.grant(0, 2);
    let empty = space.rebuild(&grant, |h| {
        if let EntryBody::Grant { boxes, .. } = &mut h.body {
            boxes.clear();
        }
    });
    refuse(&mut space, empty, rule(GRANT_BOXES_INVALID));
    let twice = space.rebuild(&grant, |h| {
        if let EntryBody::Grant { boxes, .. } = &mut h.body {
            let again = boxes[0].clone();
            boxes.push(again);
        }
    });
    refuse(&mut space, twice, rule(GRANT_BOXES_INVALID));
    space.push(grant);
}

#[test]
fn a_member_who_left_and_returned_may_be_granted_the_epoch_again() {
    let mut space = trio();
    let grant = space.grant(0, 2);
    space.push(grant);
    let leave = space.leave(2);
    space.push(leave);
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let join = space.join(2, &secret);
    space.push(join);
    let grant = space.grant(0, 2);
    space.push(grant);
}

// ----- Profile, Settings, Close ----------------------------------------------------------------

#[test]
fn every_member_publishes_a_profile_and_only_an_owner_changes_the_settings() {
    let mut space = trio();
    for (who, role) in [
        (0, MemberRole::Owner),
        (1, MemberRole::Editor),
        (2, MemberRole::Viewer),
    ] {
        let profile = space.profile(who, "New name");
        let applied = space.push(profile);
        assert_eq!(
            applied,
            Applied {
                kind: EntryKind::Profile,
                signer: space.account(who),
                subject: Some(space.account(who)),
                previous_role: Some(role),
            }
        );
    }
    let profile = space.profile(3, "x");
    refuse(&mut space, profile, rule(SIGNER_NOT_MEMBER));

    let settings = SpaceSettings {
        trash_retention_days: 7,
        editors_may_link: true,
    };
    for (who, expected) in [
        (1, SIGNER_NOT_OWNER),
        (2, SIGNER_NOT_OWNER),
        (3, SIGNER_NOT_MEMBER),
    ] {
        let entry = space.settings(who, settings);
        refuse(&mut space, entry, rule(expected));
    }
    let entry = space.settings(0, settings);
    let applied = space.push(entry);
    assert_eq!(applied.kind, EntryKind::Settings);
}

#[test]
fn the_private_part_is_sealed_under_an_epoch_that_exists() {
    let mut space = trio();
    let ahead = space.rebuild(&space.profile(0, "x"), |h| h.meta_epoch = 2);
    refuse(&mut space, ahead, rule(META_EPOCH));
    let rekey = space.rekey(0);
    space.push(rekey);
    // Epoch 1 (a member that has not seen the Rekey) and 2 are fine, 3 is not.
    for meta_epoch in [0u32, 1, 2] {
        let entry = space.rebuild(&space.profile(1, "x"), |h| h.meta_epoch = meta_epoch);
        space.push(entry);
    }
    let ahead = space.rebuild(&space.profile(0, "x"), |h| h.meta_epoch = 3);
    refuse(&mut space, ahead, rule(META_EPOCH));
}

#[test]
fn nothing_but_an_owner_does_what_an_owner_does() {
    let mut space = trio();
    let (invite, secret) = space.invite(0, MemberRole::Viewer, None, DAY);
    space.push(invite);
    let id = crate::invite::invite_id(&secret);
    for who in [1usize, 2] {
        let (invite, _) = space.invite(who, MemberRole::Viewer, None, DAY);
        let entries = [
            invite,
            space.revoke(who, id),
            space.role_set(who, 0, MemberRole::Viewer),
            space.remove(who, 0),
            space.settings(who, SpaceSettings::default()),
            space.close(who),
        ];
        for entry in entries {
            refuse(&mut space, entry, rule(SIGNER_NOT_OWNER));
        }
    }
    // What every member does, they do.
    for who in [1usize, 2] {
        let profile = space.profile(who, "x");
        space.push(profile);
        let rekey = space.rekey(who);
        space.push(rekey);
    }
}

#[test]
fn only_an_owner_closes_the_space() {
    let mut space = trio();
    for (who, expected) in [
        (1, SIGNER_NOT_OWNER),
        (2, SIGNER_NOT_OWNER),
        (3, SIGNER_NOT_MEMBER),
    ] {
        let close = space.close(who);
        refuse(&mut space, close, rule(expected));
    }
    let close = space.close(0);
    space.push(close);
    assert_eq!(space.state().closed, Some(space.cursor));
}
