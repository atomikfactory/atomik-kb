//! Milestone 13, slice 1, the request side, scenarios of a team: roles, removal, leaving, closing,
//! owners, link invites, the grant after a rekey. Helpers: `requests`.

use kb_types::{InviteSessionState, InviteStatus, MembersInfo, Request, Response, SpaceShareState};

use super::requests::*;
use super::*;

// ----- a team of two ----------------------------------------------------------------------------

/// Alice's shared project with Bob in it (a code invite), everybody settled.
pub(super) struct Team {
    pub(super) alice: Dev,
    pub(super) bob: Dev,
    pub(super) space: SpaceId,
    pub(super) plan: PageId,
    pub(super) notes: PageId,
    pub(super) feed: FeedId,
    pub(super) server: MemoryRemote,
}

pub(super) fn team(role: MemberRole) -> Team {
    let server = server();
    let Project {
        mut alice,
        space,
        plan,
        notes,
        ..
    } = project(&server);
    share_space(&mut alice, space);
    let mut bob = newcomer("bob", 2, &server, "Bob");
    code_invite(&mut alice, &mut bob, space, role);
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let feed = feed_of(&mut alice, space);
    alice.events();
    bob.events();
    Team {
        alice,
        bob,
        space,
        plan,
        notes,
        feed,
        server,
    }
}

fn account_of(info: &MembersInfo, name: &str) -> kb_types::AccountId {
    member_of(info, name).account_id
}

pub(super) fn forbidden_role(error: &ApiError, required: &str) {
    assert_eq!(error.code, ErrorCode::Forbidden, "{error:?}");
    assert_eq!(
        error.details,
        Some(serde_json::json!({ "reason": "role", "required": required })),
        "{error:?}"
    );
}

pub(super) fn forbidden_archived(error: &ApiError) {
    assert_eq!(error.code, ErrorCode::Forbidden, "{error:?}");
    assert_eq!(
        error.details,
        Some(serde_json::json!({ "reason": "archived" }))
    );
}

pub(super) fn invalid_field(error: &ApiError, field: &str) {
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(
        error.details,
        Some(serde_json::json!({ "field": field })),
        "{error:?}"
    );
}

fn members_response(response: Response) -> MembersInfo {
    match response {
        Response::Members(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn editors_edit_both_ways_and_the_requests_see_the_same_people() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        notes,
        ..
    } = team(MemberRole::Editor);
    bob.write(plan, "bob added a line");
    alice.write(notes, "alice again");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(alice.texts(plan).contains(&"bob added a line".to_owned()));
    assert!(bob.texts(notes).contains(&"alice again".to_owned()));
    assert_eq!(bob.texts(plan), alice.texts(plan));
    // Every request reads the same people on both sides.
    let (a, b) = (members(&mut alice, space), members(&mut bob, space));
    assert_eq!(
        a.members.iter().map(|m| m.account_id).collect::<Vec<_>>(),
        b.members.iter().map(|m| m.account_id).collect::<Vec<_>>()
    );
    let person = match call(
        &mut alice,
        Request::PersonGet {
            account_id: account_of(&a, "Bob"),
        },
    )
    .unwrap()
    {
        Response::Person(person) => person,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        (person.name.as_str(), person.verified, person.is_me),
        ("Bob", true, false)
    );
    assert_eq!(person.safety_number, member_of(&a, "Bob").safety_number);
    // A person the account does not know is NotFound.
    assert_eq!(
        fails(
            &mut alice,
            Request::PersonGet {
                account_id: kb_types::AccountId::from_bytes([9; 16])
            }
        )
        .code,
        ErrorCode::NotFound
    );
}

#[test]
fn an_editor_reorders_the_children_in_one_step_and_the_other_member_sees_the_order() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = team(MemberRole::Editor);
    let zeta = create_page(&mut bob.core, space, None, "Zeta").id;
    let alpha = create_page(&mut bob.core, space, None, "Alpha").id;
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(
        outline(&tree(&mut alice.core, space)),
        "Plan\n  Notes\nZeta\nAlpha\n"
    );

    call(
        &mut bob,
        Request::PageReorderChildren {
            space_id: space,
            parent_id: None,
            order: vec![alpha, plan, zeta],
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(
        outline(&tree(&mut alice.core, space)),
        "Alpha\nPlan\n  Notes\nZeta\n"
    );
    assert_eq!(
        outline(&tree(&mut bob.core, space)),
        outline(&tree(&mut alice.core, space))
    );

    // One structural undo takes the order back, and the undo reaches Alice.
    call(&mut bob, Request::StructureUndo { space_id: space }).unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(
        outline(&tree(&mut alice.core, space)),
        "Plan\n  Notes\nZeta\nAlpha\n"
    );
}

#[test]
fn a_viewer_reads_and_every_write_is_refused_by_the_request_and_by_the_receivers() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = team(MemberRole::Viewer);
    assert_eq!(space_info(&mut bob, space).role, Some(MemberRole::Viewer));
    let writes = vec![
        Request::PageCreate {
            space_id: space,
            parent_id: None,
            index: None,
            title: "New".into(),
        },
        Request::PageRename {
            page_id: plan,
            title: "Renamed".into(),
        },
        Request::PageSetIcon {
            page_id: plan,
            icon: Some("📝".into()),
        },
        Request::PageMove {
            page_id: plan,
            parent_id: None,
            index: 0,
        },
        Request::PageReorderChildren {
            space_id: space,
            parent_id: None,
            order: vec![plan],
        },
        Request::PageTrash { page_id: plan },
        Request::PageDuplicate { page_id: plan },
        Request::SpaceRename {
            space_id: space,
            name: "Renamed".into(),
        },
        Request::SpaceSetIcon {
            space_id: space,
            icon: Some("💼".into()),
        },
        Request::StructureUndo { space_id: space },
        Request::AttachmentBegin {
            space_id: space,
            name: "a.txt".into(),
            mime: "text/plain".into(),
        },
        Request::PropertyCreate {
            space_id: space,
            name: "Mood".into(),
            kind: kb_types::PropertyKind::Text,
            config: None,
        },
    ];
    for write in writes {
        let kind = write.kind();
        let error = fails(&mut bob, write);
        assert_eq!(error.code, ErrorCode::Forbidden, "{kind}");
        forbidden_role(&error, "editor");
    }
    forbidden_role(
        &bob.core.page_doc_import(plan, &[1, 2, 3]).unwrap_err(),
        "editor",
    );
    // The Owner-only requests.
    let bob_account = bob.account();
    let alice_account = alice.account();
    let owner_only = vec![
        Request::PagePurge { page_id: plan },
        Request::SpaceSettingsSet {
            space_id: space,
            trash_retention_days: Some(7),
            editors_may_link: None,
        },
        Request::MemberRoleSet {
            space_id: space,
            account_id: bob_account,
            role: MemberRole::Editor,
        },
        Request::MemberRemove {
            space_id: space,
            account_id: alice_account,
        },
        Request::InviteLinkCreate {
            space_id: space,
            role: MemberRole::Viewer,
            expires_in_days: None,
        },
        Request::InviteCodeStart {
            space_id: space,
            role: MemberRole::Viewer,
        },
        Request::SpaceClose { space_id: space },
    ];
    for request in owner_only {
        let kind = request.kind();
        let error = fails(&mut bob, request);
        assert_eq!(error.code, ErrorCode::Forbidden, "{kind}");
        forbidden_role(&error, "owner");
    }
    // Reads, search and favourites stay open.
    assert_eq!(tree(&mut bob.core, space).roots.len(), 1);
    assert!(!bob.texts(plan).is_empty());
    call(
        &mut bob,
        Request::PageFavoriteSet {
            page_id: plan,
            favorite: false,
        },
    )
    .unwrap();
    call(
        &mut bob,
        Request::PageFavoriteSet {
            page_id: plan,
            favorite: true,
        },
    )
    .unwrap();
    assert_eq!(tree(&mut bob.core, space).favorites, vec![plan]);
    assert!(
        members(&mut bob, space).invites.is_empty(),
        "a Viewer sees no invites"
    );

    // Forced past the request (a hostile or out-of-date client), the entry still reaches the server
    // only through a server that lets anything in, and the receivers refuse it.
    bob.core.shared.force_push = true;
    bob.write(plan, "a viewer should not type this");
    server_adversary(&bob, Some(Adversary::AcceptAll));
    bob.cycle().unwrap();
    server_adversary(&bob, None);
    bob.core.shared.force_push = false;
    alice.events();
    alice.cycle().unwrap();
    assert!(
        !alice
            .texts(plan)
            .contains(&"a viewer should not type this".to_owned())
    );
    let panel = members(&mut alice, space);
    assert_eq!(panel.refused_total, 1);
    assert_eq!(member_of(&panel, "Bob").refused_changes, 1);
    assert!(
        alice
            .events()
            .contains(&Event::SpaceMembersChanged { space_id: space })
    );
}

#[test]
fn a_role_change_reaches_the_member_and_the_gate_follows() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = team(MemberRole::Editor);
    let bob_account = bob.account();
    let panel = members_response(
        call(
            &mut alice,
            Request::MemberRoleSet {
                space_id: space,
                account_id: bob_account,
                role: MemberRole::Viewer,
            },
        )
        .unwrap(),
    );
    assert_eq!(
        member_of(&panel, "Bob").role,
        MemberRole::Viewer,
        "shown at once"
    );
    // The same role again changes nothing.
    call(
        &mut alice,
        Request::MemberRoleSet {
            space_id: space,
            account_id: bob_account,
            role: MemberRole::Viewer,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let events = bob.events();
    assert!(events.contains(&Event::SpaceRoleChanged {
        space_id: space,
        role: MemberRole::Viewer,
        previous: MemberRole::Editor
    }));
    assert!(events.contains(&Event::SpacesChanged));
    assert_eq!(space_info(&mut bob, space).role, Some(MemberRole::Viewer));
    forbidden_role(
        &fails(
            &mut bob,
            Request::PageRename {
                page_id: plan,
                title: "x".into(),
            },
        ),
        "editor",
    );
    // Back to Editor: the same request works and Alice sees the title.
    call(
        &mut alice,
        Request::MemberRoleSet {
            space_id: space,
            account_id: bob_account,
            role: MemberRole::Editor,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    call(
        &mut bob,
        Request::PageRename {
            page_id: plan,
            title: "Bob's plan".into(),
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert!(outline_of(&mut alice, space).contains("Bob's plan"));
    // A person who is not a member, and the last Owner.
    invalid_field(
        &fails(
            &mut alice,
            Request::MemberRoleSet {
                space_id: space,
                account_id: kb_types::AccountId::from_bytes([7; 16]),
                role: MemberRole::Viewer,
            },
        ),
        "accountId",
    );
    let alice_account = alice.account();
    invalid_field(
        &fails(
            &mut alice,
            Request::MemberRoleSet {
                space_id: space,
                account_id: alice_account,
                role: MemberRole::Editor,
            },
        ),
        "role",
    );
    invalid_field(
        &fails(&mut alice, Request::SpaceLeave { space_id: space }),
        "role",
    );
    invalid_field(
        &fails(
            &mut alice,
            Request::MemberRemove {
                space_id: space,
                account_id: alice_account,
            },
        ),
        "accountId",
    );
}

#[test]
fn removing_a_member_locks_them_out_of_what_comes_after_and_they_keep_an_archive() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        feed,
        server,
        ..
    } = team(MemberRole::Editor);
    bob.write(plan, "bob before the removal");
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    let bob_account = bob.account();
    let panel = members_response(
        call(
            &mut alice,
            Request::MemberRemove {
                space_id: space,
                account_id: bob_account,
            },
        )
        .unwrap(),
    );
    assert_eq!(panel.members.len(), 1);
    assert_eq!(panel.epoch, 2, "the removal carries its Rekey");
    assert!(!panel.pending_rekey);
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    // Bob: an archive with the banner's data, the server refuses him, nothing later opens.
    let events = bob.events();
    assert!(events.contains(&Event::SpaceRemoved {
        space_id: space,
        reason: ArchiveReason::Removed
    }));
    let shown = space_info(&mut bob, space);
    assert_eq!(shown.share_state, SpaceShareState::Archived);
    assert_eq!(shown.archive_reason, Some(ArchiveReason::Removed));
    assert_eq!(shown.role, Some(MemberRole::Editor), "the role it had");
    forbidden_archived(&fails(
        &mut bob,
        Request::PageRename {
            page_id: plan,
            title: "x".into(),
        },
    ));
    forbidden_archived(&fails(
        &mut bob,
        Request::SpaceSettingsSet {
            space_id: space,
            trash_retention_days: Some(1),
            editors_may_link: None,
        },
    ));
    alice.write(plan, "written after the removal");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(
        bob.texts(plan)
            .contains(&"bob before the removal".to_owned()),
        "the archive still reads"
    );
    assert!(
        !bob.texts(plan)
            .contains(&"written after the removal".to_owned()),
        "nothing written after the Rekey reaches the removed member"
    );
    assert!(
        bob.core.feed_sk(feed, 2).unwrap().is_none(),
        "never given the new key"
    );
    // The server lets him read up to his own removal and nothing after it.
    let cursor = bob.core.feed_row(feed).unwrap().cursor;
    let later = bob.remote.changes_sync(feed, cursor, 100);
    assert!(
        later.is_err() || later.unwrap().changes.is_empty(),
        "the server gives him nothing past his removal"
    );
    assert!(
        bob.remote.append_sync(feed, &[0u8; 8]).is_err(),
        "and takes nothing from him"
    );
    assert!(
        server.fetch_log_sync(bob_account, None).is_ok(),
        "the account itself stays"
    );

    // A removed account may be invited again: the archive comes back to life.
    code_invite(&mut alice, &mut bob, space, MemberRole::Viewer);
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let back = space_info(&mut bob, space);
    assert_eq!(back.share_state, SpaceShareState::Shared);
    assert_eq!(back.role, Some(MemberRole::Viewer));
    assert!(
        bob.texts(plan)
            .contains(&"written after the removal".to_owned()),
        "the new epoch's key came with the invite"
    );
    assert!(
        bob.texts(plan)
            .contains(&"bob before the removal".to_owned())
    );
    assert_eq!(members(&mut alice, space).members.len(), 2);
    // The archive's leftovers did not fork anything out.
    assert!(!outline_of(&mut bob, space).contains("Unsent changes"));
}

#[test]
fn leaving_hides_the_space_at_once_and_the_data_goes_when_the_entry_is_on_the_server() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = team(MemberRole::Editor);
    call(&mut bob, Request::SpaceLeave { space_id: space }).unwrap();
    assert!(
        crate::test_support::spaces(&mut bob.core)
            .iter()
            .all(|info| info.id != space),
        "hidden at once"
    );
    // The local data goes once the Leave is on the server.
    assert!(bob.core.structure(space).is_ok());
    bob.cycle().unwrap();
    assert!(
        bob.core.structure(space).is_ok(),
        "the Leave was posted in that cycle: the data goes at the start of the next one"
    );
    bob.cycle().unwrap();
    assert!(bob.core.structure(space).is_err(), "the local copy is gone");
    assert!(bob.core.shared_feed_of(space).unwrap().is_none());
    assert_eq!(
        bob.core
            .docs
            .account
            .as_ref()
            .unwrap()
            .model
            .shared_space(space),
        None
    );
    // Alice sees the Leave and the server asks for a Rekey, which the next writer mints.
    alice.cycle().unwrap();
    let panel = members(&mut alice, space);
    assert_eq!(panel.members.len(), 1);
    assert!(panel.pending_rekey);
    alice.write(plan, "after the leave");
    Dev::settle(&mut [&mut alice], 3);
    let panel = members(&mut alice, space);
    assert_eq!(panel.epoch, 2);
    assert!(!panel.pending_rekey);
}

#[test]
fn closing_a_space_makes_an_archive_for_everybody_and_a_shared_space_is_never_deleted() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = team(MemberRole::Editor);
    invalid_field(
        &fails(&mut alice, Request::SpaceDelete { space_id: space }),
        "spaceId",
    );
    let closed = match call(&mut alice, Request::SpaceClose { space_id: space }).unwrap() {
        Response::Space(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(closed.share_state, SpaceShareState::Archived);
    assert_eq!(closed.archive_reason, Some(ArchiveReason::Closed));
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    for dev in [&mut alice, &mut bob] {
        let events = dev.events();
        assert!(
            events.contains(&Event::SpaceRemoved {
                space_id: space,
                reason: ArchiveReason::Closed
            }),
            "{}: {events:?}",
            dev.name
        );
        let info = space_info(dev, space);
        assert_eq!(info.share_state, SpaceShareState::Archived, "{}", dev.name);
        assert_eq!(info.archive_reason, Some(ArchiveReason::Closed));
        assert!(members(dev, space).closed_at.is_some());
        forbidden_archived(&fails(
            dev,
            Request::PageRename {
                page_id: plan,
                title: "x".into(),
            },
        ));
        invalid_field(
            &fails(dev, Request::SpaceDelete { space_id: space }),
            "spaceId",
        );
        assert!(!dev.texts(plan).is_empty(), "the archive reads");
    }
    forbidden_archived(&fails(&mut alice, Request::SpaceClose { space_id: space }));
    // An archive is removed from the device with SpaceLeave, which only deletes the local copy.
    call(&mut bob, Request::SpaceLeave { space_id: space }).unwrap();
    assert!(bob.core.structure(space).is_err());
    assert!(
        crate::test_support::spaces(&mut bob.core)
            .iter()
            .all(|info| info.id != space)
    );
    // Alice's copy is untouched.
    assert!(alice.core.structure(space).is_ok());
}

#[test]
fn two_owners_removing_each_other_leave_the_first_and_the_loser_is_the_one_removed() {
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = team(MemberRole::Editor);
    let (alice_account, bob_account) = (alice.account(), bob.account());
    call(
        &mut alice,
        Request::MemberRoleSet {
            space_id: space,
            account_id: bob_account,
            role: MemberRole::Owner,
        },
    )
    .unwrap();
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(space_info(&mut bob, space).role, Some(MemberRole::Owner));
    // Both act before either hears of the other.
    call(
        &mut alice,
        Request::MemberRemove {
            space_id: space,
            account_id: bob_account,
        },
    )
    .unwrap();
    call(
        &mut bob,
        Request::MemberRemove {
            space_id: space,
            account_id: alice_account,
        },
    )
    .unwrap();
    alice.cycle().unwrap();
    Dev::settle(&mut [&mut bob, &mut alice], 4);
    let panel = members(&mut alice, space);
    assert_eq!(panel.members.len(), 1);
    assert_eq!(panel.members[0].name, "Alice");
    assert_eq!(
        panel.epoch, 2,
        "one Rekey: the loser's removal never happened"
    );
    let shown = space_info(&mut bob, space);
    assert_eq!(shown.share_state, SpaceShareState::Archived);
    assert_eq!(shown.archive_reason, Some(ArchiveReason::Removed));
    assert!(!alice.texts(plan).is_empty());
}

#[test]
fn joining_after_a_rekey_gets_a_grant_for_the_current_epoch() {
    let server = server();
    let Project {
        mut alice,
        space,
        plan,
        ..
    } = project(&server);
    share_space(&mut alice, space);
    let mut bob = newcomer("bob", 2, &server, "Bob");
    code_invite(&mut alice, &mut bob, space, MemberRole::Editor);
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    // A link made at epoch 1 ...
    let link = link_invite(&mut alice, space, MemberRole::Editor);
    // ... and a removal raises the epoch before Carol uses it.
    let bob_account = bob.account();
    call(
        &mut alice,
        Request::MemberRemove {
            space_id: space,
            account_id: bob_account,
        },
    )
    .unwrap();
    alice.write(plan, "written at epoch two");
    Dev::settle(&mut [&mut alice], 3);
    assert_eq!(members(&mut alice, space).epoch, 2);
    let mut carol = newcomer("carol", 3, &server, "Carol");
    let preview = open_link(&mut carol, &link.link);
    assert_eq!(preview.state, InviteSessionState::Previewing);
    accept_link(&mut carol);
    Dev::settle(&mut [&mut alice, &mut carol], 5);
    // Carol joined with the epoch-1 bundle; any member device posts the Grant.
    assert!(
        carol
            .texts(plan)
            .contains(&"written at epoch two".to_owned()),
        "carol reads the current epoch"
    );
    assert_eq!(space_info(&mut carol, space).role, Some(MemberRole::Editor));
    carol.write(plan, "carol writes at epoch two");
    Dev::settle(&mut [&mut carol, &mut alice], 3);
    assert!(
        alice
            .texts(plan)
            .contains(&"carol writes at epoch two".to_owned())
    );
}

#[test]
fn a_link_invite_joins_unverified_and_the_safety_number_is_the_same_on_both_sides() {
    let server = server();
    let Project {
        mut alice, space, ..
    } = project(&server);
    share_space(&mut alice, space);
    let mut carol = newcomer("carol", 3, &server, "Carol");
    let link = link_invite(&mut alice, space, MemberRole::Commenter);
    assert!(
        link.link
            .starts_with("atomik-kb://invite?server=https%3A%2F%2Fsync.example.test#")
    );
    let secret = link.link.split('#').nth(1).unwrap().to_owned();
    assert_eq!(secret.len(), 52);
    assert_eq!(link.web_link, format!("{SERVER_URL}/join#{secret}"));
    assert_eq!(link.role, MemberRole::Commenter);
    assert_eq!(link.expires_at, alice.clock.now_ms() + 7 * DAY_MS);
    let panel = members(&mut alice, space);
    assert_eq!(panel.invites.len(), 1);
    assert_eq!(panel.invites[0].kind, kb_types::InviteKind::Link);
    assert_eq!(panel.invites[0].status, InviteStatus::Open);
    assert!(
        !format!("{panel:?}").contains(&secret),
        "the secret is shown once and not listed"
    );

    // The preview: not verified, and nothing is registered until the user accepts.
    let preview = open_link(&mut carol, &link.link);
    assert_eq!(preview.state, InviteSessionState::Previewing);
    assert_eq!(preview.space_name.as_deref(), Some("Project"));
    assert_eq!(preview.inviter_name.as_deref(), Some("Alice"));
    assert_eq!(preview.role, Some(MemberRole::Commenter));
    assert!(!preview.verified);
    assert!(carol.core.shared_feed_of(space).unwrap().is_none());
    accept_link(&mut carol);
    assert!(!invite_info(&mut carol).verified);
    Dev::settle(&mut [&mut alice, &mut carol], 4);

    let (a, c) = (members(&mut alice, space), members(&mut carol, space));
    assert!(!member_of(&a, "Carol").verified);
    assert!(!member_of(&c, "Alice").verified);
    assert_eq!(
        member_of(&a, "Carol").safety_number,
        member_of(&c, "Alice").safety_number
    );
    assert_eq!(member_of(&a, "Carol").role, MemberRole::Commenter);
    assert_eq!(a.invites[0].status, InviteStatus::Used);
    assert_eq!(a.invites[0].used_by, Some(carol.account()));
    // "Mark verified" is per person and written to the account document.
    let carol_account = carol.account();
    let alice_account = alice.account();
    let person = match call(
        &mut alice,
        Request::PersonVerifiedSet {
            account_id: carol_account,
            verified: true,
        },
    )
    .unwrap()
    {
        Response::Person(person) => person,
        other => panic!("unexpected {other:?}"),
    };
    assert!(person.verified);
    assert!(member_of(&members(&mut alice, space), "Carol").verified);
    assert!(
        !member_of(&members(&mut carol, space), "Alice").verified,
        "per account"
    );
    call(
        &mut carol,
        Request::PersonVerifiedSet {
            account_id: alice_account,
            verified: true,
        },
    )
    .unwrap();
    assert!(member_of(&members(&mut carol, space), "Alice").verified);
    // Unmarking, and never this account.
    let person = match call(
        &mut alice,
        Request::PersonVerifiedSet {
            account_id: carol_account,
            verified: false,
        },
    )
    .unwrap()
    {
        Response::Person(person) => person,
        other => panic!("unexpected {other:?}"),
    };
    assert!(!person.verified);
    invalid_field(
        &fails(
            &mut alice,
            Request::PersonVerifiedSet {
                account_id: alice_account,
                verified: true,
            },
        ),
        "accountId",
    );
    // The link is single use: a second person finds one answer for a dead invite.
    let mut dave = newcomer("dave", 4, &server, "Dave");
    call(
        &mut dave,
        Request::InviteLinkOpen {
            link: link.link.clone(),
        },
    )
    .unwrap();
    let epoch = dave.remote.server_epoch();
    let request = dave.core.invite_open_request(epoch).unwrap().unwrap();
    assert!(matches!(
        dave.remote
            .invite_open_sync(request.invite_id, &request.signature),
        Err(kb_sync::SyncError::NotFound)
    ));
    dave.core
        .invite_failed("this invite is invalid, expired or already used")
        .unwrap();
    let failed = invite_info(&mut dave);
    assert_eq!(failed.state, InviteSessionState::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("this invite is invalid, expired or already used")
    );
    // The web form of the link reads the same.
    call(
        &mut dave,
        Request::InviteLinkOpen {
            link: link.web_link.clone(),
        },
    )
    .unwrap();
    assert_eq!(invite_info(&mut dave).state, InviteSessionState::Starting);
}

#[test]
fn a_member_s_core_opens_again_with_the_keys_that_other_members_devices_signed() {
    // Found by the real-app pass (drive-m13-share, the restart step): the key records of the documents Alice wrote are signed by
    // Alice's devices and are stored in Bob's keyring table; Bob's core refused to open at the next start ("a keyring item is
    // signed by an unknown device") because only Bob's own devices were known at that point.
    let Team {
        mut alice,
        mut bob,
        space,
        plan,
        notes,
        ..
    } = team(MemberRole::Editor);
    alice.write(notes, "alice wrote after bob joined");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    let now = bob.clock.now_ms();
    bob.reopen(91, now + 60_000);
    // It opened. The shared space is there, Bob still reads what Alice wrote and can write himself.
    assert!(
        crate::test_support::spaces(&mut bob.core)
            .iter()
            .any(|info| info.id == space)
    );
    bob.cycle().unwrap();
    assert!(
        bob.texts(notes)
            .contains(&"alice wrote after bob joined".to_owned())
    );
    bob.write(plan, "bob after the restart");
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert!(
        alice
            .texts(plan)
            .contains(&"bob after the restart".to_owned())
    );
}

// ----- the log -------------------------------------------------------------------------------------

/// The buffer the global subscriber of the log test writes into.
static LOG: std::sync::OnceLock<LogBuffer> = std::sync::OnceLock::new();

/// A buffer the formatter of `tracing` writes the log into.
#[derive(Clone, Default)]
struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut bytes) = self.0.lock() {
            bytes.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Whether a line holds a full id: a UUID (`8-4-...`) or a run of 12 or more hex digits (a 16-byte id, a hash, a key).
fn has_full_id(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if bytes[i].is_ascii_hexdigit() && starts_word {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                j += 1;
            }
            let run = &bytes[i..j];
            let word_goes_on = j < bytes.len() && bytes[j].is_ascii_alphanumeric();
            let mixed =
                run.iter().any(u8::is_ascii_alphabetic) && run.iter().any(u8::is_ascii_digit);
            if !word_goes_on && run.len() >= 12 && mixed {
                return true;
            }
            let uuid_start = run.len() == 8
                && j + 5 <= bytes.len()
                && bytes[j] == b'-'
                && bytes[j + 1..j + 5].iter().all(u8::is_ascii_hexdigit);
            if uuid_start {
                return true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}

#[test]
fn the_matcher_of_full_ids_finds_a_uuid_and_a_long_hex_run_and_leaves_short_ids_alone() {
    assert!(has_full_id(
        "doc=0abaadc7-1bc2-4b7b-a29d-494bdd1a360b device_seq=1"
    ));
    assert!(has_full_id("feed=aad1880b9f0e4c7d8a55e3b1c2d3e4f5"));
    assert!(!has_full_id("feed=aad1880b entries=3 cursor=1700000000000"));
    assert!(!has_full_id("a made-up line with nothing in it"));
}

/// The global subscriber of the log tests, installed once. A scoped subscriber is not reliable here: the other tests of this
/// binary run at the same time on other threads and register the callsites with no subscriber of their own. This one shows
/// thread names, and a test reads the lines of its own thread (the test's name) with `own_log`.
fn log_buffer() -> &'static LogBuffer {
    LOG.get_or_init(|| {
        let buffer = LogBuffer::default();
        let writer = buffer.clone();
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_ansi(false)
                .with_thread_names(true)
                .with_writer(move || writer.clone())
                .finish(),
        );
        buffer
    })
}

/// What the thread of the running test logged so far, one event per line.
fn own_log(buffer: &LogBuffer) -> String {
    let thread = std::thread::current().name().unwrap_or_default().to_owned();
    assert!(!thread.is_empty(), "the test runs on a named thread");
    let all = String::from_utf8_lossy(&buffer.0.lock().unwrap().clone()).into_owned();
    all.lines()
        .filter(|line| line.contains(thread.as_str()))
        .map(|line| {
            format!(
                "{line}
"
            )
        })
        .collect()
}

#[test]
fn the_log_of_a_shared_space_s_life_holds_ids_cut_to_eight_characters_and_nothing_longer() {
    // The rule of milestone 13 (and its real-app pass): logs carry paths and ids cut to 8 hex characters, never a full space,
    // page, document, device, account or feed id. A team shares, joins, edits both ways and one member restarts, with the
    // log captured at every level.
    let buffer = log_buffer();
    {
        let Team {
            mut alice,
            mut bob,
            plan,
            notes,
            ..
        } = team(MemberRole::Editor);
        bob.write(plan, "bob added a line");
        alice.write(notes, "alice again");
        Dev::settle(&mut [&mut alice, &mut bob], 3);
        let now = bob.clock.now_ms();
        bob.reopen(93, now + 60_000);
        bob.cycle().unwrap();
    }
    let log = own_log(buffer);
    assert!(
        log.lines().count() > 20,
        "the capture holds the events of the run ({} lines)",
        log.lines().count()
    );
    let offenders: Vec<&str> = log.lines().filter(|line| has_full_id(line)).collect();
    assert!(offenders.is_empty(), "full ids in the log: {offenders:#?}");
}

#[test]
fn an_attachment_that_arrives_through_the_feed_is_indexed_as_it_arrives_with_no_repair_to_warn_of()
{
    // Found by the real-app pass: every device that received a shared space with an attachment, and every one that saw a file
    // added or removed, logged WARN "attachment index out of sync; rebuilding". The index follows the manifests as they arrive; only
    // a difference at open or after a restore is a repair.
    let buffer = log_buffer();
    let Team {
        mut alice,
        mut bob,
        space,
        ..
    } = team(MemberRole::Editor);
    // The space came with its first file (the project's); a second one is added after Bob joined.
    alice.upload_attachment(space, "second.bin", &[7u8; 5_000]);
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let names = |dev: &mut Dev| -> Vec<String> {
        match call(dev, Request::AttachmentList { space_id: space }).unwrap() {
            Response::Attachments(list) => list.into_iter().map(|a| a.name).collect(),
            other => panic!("unexpected {other:?}"),
        }
    };
    let on_bob = names(&mut bob);
    assert!(
        on_bob.contains(&"diagram.bin".to_owned()) && on_bob.contains(&"second.bin".to_owned()),
        "both files are indexed on Bob's device: {on_bob:?}"
    );
    // Bob restarts: the check at open has nothing to repair either.
    let now = bob.clock.now_ms();
    bob.reopen(95, now + 60_000);
    bob.cycle().unwrap();
    assert_eq!(names(&mut bob), on_bob);
    let log = own_log(buffer);
    assert!(
        log.lines().count() > 20,
        "the capture holds the events of the run"
    );
    let warned: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("attachment index out of sync"))
        .collect();
    assert!(
        warned.is_empty(),
        "the index was repaired when it should have followed the manifests: {warned:#?}"
    );
}

// ----- a member's backup -------------------------------------------------------------------------

#[test]
fn a_member_s_backup_restores_on_a_fresh_device_which_then_reads_and_writes_the_shared_space() {
    // The backup of a member holds the key records of the documents Alice wrote, signed by Alice's devices. Restoring checks the
    // signers against the device log the backup carries, which is Bob's own: it refused the file ("a keyring item is signed by an
    // unknown device"). Those records are not Bob's to vouch for: they are left out of the restore and reach the new device
    // through the shared feed, where they are checked against the member's own device log.
    let server = server();
    let Project {
        mut alice,
        space,
        plan,
        notes,
        ..
    } = project(&server);
    share_space(&mut alice, space);
    let (mut bob, kit) = Dev::first("bob", 2, &server);
    set_profile(&mut bob, "Bob");
    code_invite(&mut alice, &mut bob, space, MemberRole::Editor);
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    alice.write(notes, "alice wrote after bob joined");
    bob.write(plan, "bob wrote before the backup");
    Dev::settle(&mut [&mut alice, &mut bob], 3);

    let mut bob2 = Dev::from_backup("bob2", 7, &server, &mut bob, &kit);
    bob2.configure();
    Dev::settle(&mut [&mut bob2, &mut alice, &mut bob], 4);
    // The new device is a member's device: it reads everything and writes.
    let shared = crate::test_support::spaces(&mut bob2.core)
        .into_iter()
        .find(|info| info.id == space)
        .expect("the shared space is on the restored device");
    assert_eq!(shared.share_state, SpaceShareState::Shared);
    assert_eq!(shared.role, Some(MemberRole::Editor));
    assert!(
        bob2.texts(notes)
            .contains(&"alice wrote after bob joined".to_owned())
    );
    assert!(
        bob2.texts(plan)
            .contains(&"bob wrote before the backup".to_owned())
    );
    bob2.write(notes, "bob's second device wrote");
    Dev::settle(&mut [&mut bob2, &mut alice, &mut bob], 3);
    assert!(
        alice
            .texts(notes)
            .contains(&"bob's second device wrote".to_owned())
    );
    // And a restart of it opens (the key records it holds are checked at open as they were at restore).
    let now = bob2.clock.now_ms();
    bob2.reopen(94, now + 60_000);
    bob2.cycle().unwrap();
    assert!(
        bob2.texts(notes)
            .contains(&"alice wrote after bob joined".to_owned())
    );
}
