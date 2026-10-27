//! Milestone 13, slice 1, the request side: the scenarios drive the host methods and the requests the
//! way the shell and the UI do (`SpaceShare`, the invite sessions through the `InviteHost`
//! methods in the order `kb-net`'s task uses them, `MemberRoleSet`, `MemberRemove`, `SpaceLeave`,
//! `SpaceClose`, ...) over the people of the simulator in `super`: accounts with devices on one
//! `MemoryRemote`.

use kb_sync::v2::{PAIR_ROLE_OPENER, PAIR_ROLE_TYPIST};
use kb_types::{
    AttachmentInfo, InviteLinkInfo, InviteSessionInfo, InviteSessionState, InviteSide,
    InviteStatus, MembersInfo, ProfileInfo, Request, Response, SpaceInfo, SpaceSharePlan,
    SpaceShareState,
};

use super::*;

// ----- requests ---------------------------------------------------------------------------------

pub(super) fn call(dev: &mut Dev, request: Request) -> Result<Response, ApiError> {
    dev.core.dispatch(request)
}

pub(super) fn fails(dev: &mut Dev, request: Request) -> ApiError {
    match dev.core.dispatch(request) {
        Ok(response) => panic!("{}: expected an error, got {response:?}", dev.name),
        Err(error) => error,
    }
}

pub(super) fn set_profile(dev: &mut Dev, name: &str) -> ProfileInfo {
    match call(
        dev,
        Request::ProfileSet {
            display_name: name.into(),
        },
    )
    .unwrap()
    {
        Response::Profile(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

pub(super) fn space_info(dev: &mut Dev, space: SpaceId) -> SpaceInfo {
    crate::test_support::spaces(&mut dev.core)
        .into_iter()
        .find(|info| info.id == space)
        .unwrap_or_else(|| panic!("{}: the space is not listed", dev.name))
}

pub(super) fn members(dev: &mut Dev, space: SpaceId) -> MembersInfo {
    match call(dev, Request::SpaceMembers { space_id: space }).unwrap() {
        Response::Members(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

pub(super) fn plan(dev: &mut Dev, space: SpaceId) -> SpaceSharePlan {
    match call(dev, Request::SpaceSharePlan { space_id: space }).unwrap() {
        Response::SpaceSharePlan(plan) => plan,
        other => panic!("unexpected {other:?}"),
    }
}

pub(super) fn member_of<'a>(info: &'a MembersInfo, name: &str) -> &'a kb_types::MemberInfo {
    info.members
        .iter()
        .find(|member| member.name == name)
        .unwrap_or_else(|| panic!("no member named {name}: {:?}", info.members))
}

/// `SpaceShare` as the UI sends it, then the cycles that bring the `Create` and the documents to the
/// server.
pub(super) fn share_space(owner: &mut Dev, space: SpaceId) -> SpaceInfo {
    let info = match call(owner, Request::SpaceShare { space_id: space }).unwrap() {
        Response::Space(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(info.share_state, SpaceShareState::Pending);
    Dev::settle(&mut [owner], 3);
    info
}

pub(super) fn feed_of(dev: &mut Dev, space: SpaceId) -> FeedId {
    members(dev, space).feed_id
}

// ----- invites, the way kb-net's task drives them -----------------------------------------------

/// The messages of one side of a mailbox that it has handled.
#[derive(Default)]
pub(super) struct Cursor(pub(super) u32);

/// Moves what the two sides of a code session hold in their outboxes into the mailbox, and what
/// they find there into the other side, until nothing moves.
pub(super) fn pump(
    inviter: &mut Dev,
    invitee: &mut Dev,
    mailbox: [u8; 6],
    cursors: &mut [Cursor; 2],
) {
    for _ in 0..40 {
        let mut moved = false;
        for (index, (dev, role)) in [
            (&mut *inviter, PAIR_ROLE_OPENER),
            (&mut *invitee, PAIR_ROLE_TYPIST),
        ]
        .into_iter()
        .enumerate()
        {
            for (seq, bytes) in dev.core.invite_outbox().unwrap() {
                dev.remote.pair_post_sync(mailbox, role, &bytes).unwrap();
                dev.core.invite_posted(seq).unwrap();
                moved = true;
            }
            let page = dev
                .remote
                .pair_messages_sync(mailbox, cursors[index].0, 0)
                .unwrap();
            for message in page.messages {
                cursors[index].0 = message.seq + 1;
                if message.role != role {
                    let from = if message.role == PAIR_ROLE_OPENER {
                        InviteSide::Inviter
                    } else {
                        InviteSide::Invitee
                    };
                    dev.core
                        .invite_receive(message.seq, from, &message.bytes)
                        .unwrap();
                    moved = true;
                }
            }
        }
        if !moved {
            return;
        }
    }
    panic!("the invite session does not settle");
}

pub(super) fn invite_info(dev: &mut Dev) -> InviteSessionInfo {
    dev.core.invite_info().unwrap()
}

/// What a joining invitee's task does against the server once the invite is known: the share signup
/// when the account is not there, authentication, the open request, the log check (fetching the
/// device logs the core asks for), and, for the code flow, the `Join`.
pub(super) fn open_invite(member: &mut Dev) {
    let epoch = member.remote.server_epoch();
    if let Some(signup) = member.core.invite_signup(epoch).unwrap() {
        member
            .remote
            .account_create_share_sync(&signup.log, signup.invite_id, &signup.signature)
            .unwrap();
        member
            .core
            .sync_log_published(signup.log.len() as u64)
            .unwrap();
    }
    authenticate(member);
    let request = member
        .core
        .invite_open_request(epoch)
        .unwrap()
        .expect("an invitee has an open request");
    let opened = member
        .remote
        .invite_open_sync(request.invite_id, &request.signature)
        .unwrap();
    verify_opened(member, &opened);
}

pub(super) fn authenticate(member: &mut Dev) {
    if member.authed {
        return;
    }
    let (account, device) = member.core.sync_identity().unwrap().unwrap();
    member
        .remote
        .authenticate_sync(account, device, &|message| {
            member.core.sync_sign(message).unwrap()
        })
        .unwrap();
    member.authed = true;
}

/// `invite_opened` until the core needs no more device logs (it asks for them through
/// `sync_needed_logs`).
pub(super) fn verify_opened(member: &mut Dev, opened: &kb_sync::v2::InviteOpened) {
    for _ in 0..4 {
        member.core.invite_opened(opened).unwrap();
        let needed = member.core.sync_needed_logs().unwrap();
        if needed.is_empty() {
            return;
        }
        for chunk in member.remote.logs_batch_sync(&needed).unwrap() {
            member
                .core
                .sync_foreign_log_received(chunk.account, &chunk.entries)
                .unwrap();
        }
    }
}

/// The `Join`: minted, posted by compare-and-swap, reported.
pub(super) fn post_join(member: &mut Dev) {
    let (feed, entry) = member
        .core
        .invite_join()
        .unwrap()
        .expect("the invitee has a Join to post");
    let head = member
        .remote
        .membership_append_sync(feed, &[entry])
        .unwrap();
    member.core.invite_joined(head).unwrap();
}

/// A code invite from `inviter` to `invitee` for `space` with `role`, step by step like the UI and the
/// tasks do; answers the digits both showed.
pub(super) fn code_invite(
    inviter: &mut Dev,
    invitee: &mut Dev,
    space: SpaceId,
    role: MemberRole,
) -> String {
    call(
        inviter,
        Request::InviteCodeStart {
            space_id: space,
            role,
        },
    )
    .unwrap();
    let info = invite_info(inviter);
    assert_eq!(info.state, InviteSessionState::Starting);
    // The task opens the mailbox with the offer the core made.
    let (seq, offer) = inviter.core.invite_outbox().unwrap().remove(0);
    assert_eq!(seq, 0);
    let (mailbox, expires) = inviter.remote.pair_create_sync(&offer).unwrap();
    inviter
        .core
        .invite_mailbox_opened(mailbox, expires)
        .unwrap();
    inviter.core.invite_posted(0).unwrap();
    let info = invite_info(inviter);
    assert_eq!(info.state, InviteSessionState::Waiting);
    let code = info.code.expect("the inviter shows a code");
    assert_eq!(code.split(' ').count(), 3, "three groups of four");

    // The invitee types the server and the code.
    call(
        invitee,
        Request::InviteCodeJoin {
            server_url: SERVER_URL.into(),
            code,
        },
    )
    .unwrap();
    assert_eq!(invite_info(invitee).state, InviteSessionState::Starting);
    let found = invitee
        .core
        .invite_mailbox_id()
        .unwrap()
        .expect("the code names a mailbox");
    assert_eq!(found, mailbox);
    invitee.core.invite_mailbox_opened(found, expires).unwrap();
    let mut cursors = [Cursor::default(), Cursor::default()];
    cursors[0].0 = 1;
    pump(inviter, invitee, mailbox, &mut cursors);
    let (a, b) = (invite_info(inviter), invite_info(invitee));
    assert_eq!(a.state, InviteSessionState::Confirming);
    assert_eq!(b.state, InviteSessionState::Confirming);
    let digits = a.sas.clone().expect("digits");
    assert_eq!(a.sas, b.sas, "both sides show the same six digits");
    assert_eq!(digits.len(), 6);

    // The invitee confirms first; the inviter then sees who is asking.
    call(invitee, Request::InviteConfirm).unwrap();
    assert_eq!(
        invite_info(invitee).state,
        InviteSessionState::PeerConfirming
    );
    pump(inviter, invitee, mailbox, &mut cursors);
    let a = invite_info(inviter);
    assert_eq!(a.state, InviteSessionState::PeerConfirming);
    assert!(a.peer_name.is_some() && a.peer_safety_number.is_some());
    // The inviter's confirmation writes the Invite; it must be on the server before the bundle leaves.
    call(inviter, Request::InviteConfirm).unwrap();
    assert_eq!(invite_info(inviter).state, InviteSessionState::Transferring);
    assert!(
        inviter.core.invite_outbox().unwrap().is_empty(),
        "no bundle before the Invite is on the server"
    );
    let pending = inviter
        .core
        .invite_membership_pending()
        .unwrap()
        .expect("the Invite waits for the server");
    let head = inviter
        .remote
        .membership_append_sync(pending.feed, &pending.entries)
        .unwrap();
    inviter.core.invite_membership_posted(head).unwrap();
    pump(inviter, invitee, mailbox, &mut cursors);
    assert_eq!(invite_info(invitee).state, InviteSessionState::Joining);
    // The invitee finishes against the server.
    open_invite(invitee);
    post_join(invitee);
    assert_eq!(invite_info(invitee).state, InviteSessionState::Done);
    digits
}

/// A link invite: `InviteLinkCreate`, then the invitee opens it and previews.
pub(super) fn link_invite(owner: &mut Dev, space: SpaceId, role: MemberRole) -> InviteLinkInfo {
    match call(
        owner,
        Request::InviteLinkCreate {
            space_id: space,
            role,
            expires_in_days: None,
        },
    )
    .unwrap()
    {
        Response::InviteLink(link) => {
            // The driver reacts to `SyncRequested`: the `Invite` goes to the server.
            owner.cycle().unwrap();
            link
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// The invitee opens a link (the deep link of the shell) and gets to the preview.
pub(super) fn open_link(member: &mut Dev, link: &str) -> InviteSessionInfo {
    call(
        member,
        Request::InviteLinkOpen {
            link: link.to_owned(),
        },
    )
    .unwrap();
    assert_eq!(invite_info(member).state, InviteSessionState::Starting);
    // The open does not need an account on the server.
    let epoch = member.remote.server_epoch();
    let request = member
        .core
        .invite_open_request(epoch)
        .unwrap()
        .expect("an open request");
    let opened = member
        .remote
        .invite_open_sync(request.invite_id, &request.signature)
        .unwrap();
    verify_opened(member, &opened);
    invite_info(member)
}

/// Accepts a previewed link: the signup, the `Join`.
pub(super) fn accept_link(member: &mut Dev) {
    call(member, Request::InviteConfirm).unwrap();
    assert_eq!(invite_info(member).state, InviteSessionState::Joining);
    let epoch = member.remote.server_epoch();
    if let Some(signup) = member.core.invite_signup(epoch).unwrap() {
        member
            .remote
            .account_create_share_sync(&signup.log, signup.invite_id, &signup.signature)
            .unwrap();
        member
            .core
            .sync_log_published(signup.log.len() as u64)
            .unwrap();
    }
    authenticate(member);
    post_join(member);
    assert_eq!(invite_info(member).state, InviteSessionState::Done);
}

/// A newcomer: an account with a workspace and a kit, a profile name, and no sync configuration (the
/// server creates the account through the share signup when it joins).
pub(super) fn newcomer(name: &'static str, seed: u64, server: &MemoryRemote, profile: &str) -> Dev {
    let (mut dev, _) = Dev::first(name, seed, server);
    set_profile(&mut dev, profile);
    dev
}

/// Alice with a sync configuration, a profile name and a space `Project` with history: a page, a
/// sub-page, a trashed page, an attachment and a favourite.
pub(super) struct Project {
    pub(super) alice: Dev,
    pub(super) space: SpaceId,
    pub(super) plan: PageId,
    pub(super) notes: PageId,
    pub(super) trashed: PageId,
    pub(super) file: AttachmentInfo,
    pub(super) bytes: Vec<u8>,
}

pub(super) fn project(server: &MemoryRemote) -> Project {
    let (mut alice, _) = Dev::first("alice", 1, server);
    alice.configure();
    set_profile(&mut alice, "Alice");
    let space = create_space(&mut alice.core, "Project").id;
    let plan = create_page(&mut alice.core, space, None, "Plan").id;
    let notes = create_page(&mut alice.core, space, Some(plan), "Notes").id;
    let trashed = create_page(&mut alice.core, space, None, "Old ideas").id;
    alice.write(plan, "alice wrote the plan");
    alice.write(notes, "alice wrote notes");
    alice.write(trashed, "an idea nobody wanted");
    call(&mut alice, Request::PageTrash { page_id: trashed }).unwrap();
    call(
        &mut alice,
        Request::PageFavoriteSet {
            page_id: plan,
            favorite: true,
        },
    )
    .unwrap();
    let bytes: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
    let file = alice.upload_attachment(space, "diagram.bin", &bytes);
    alice.cycle().unwrap();
    alice.core.drain_events();
    Project {
        alice,
        space,
        plan,
        notes,
        trashed,
        file,
        bytes,
    }
}

// ----- scenarios --------------------------------------------------------------------------------

#[test]
pub(super) fn a_space_with_history_is_shared_and_a_code_invite_brings_a_newcomer_in() {
    let server = server();
    let Project {
        mut alice,
        space,
        plan,
        notes,
        trashed,
        file,
        bytes,
    } = project(&server);
    let mut bob = newcomer("bob", 2, &server, "Bob");
    let bob_account = bob.account();

    // The plan counts what travels: live pages, the trashed one, the attachment.
    let counted = plan_of(&mut alice, space);
    assert_eq!(counted.page_count, 2);
    assert_eq!(counted.trashed_page_count, 1);
    assert_eq!(counted.attachment_count, 1);
    assert_eq!(counted.attachment_bytes, bytes.len() as u64);
    assert!(counted.history_bytes > 0);
    assert!(counted.server_configured && counted.has_account && counted.has_profile_name);
    assert!(!counted.already_shared);
    assert_eq!(
        space_info(&mut alice, space).share_state,
        SpaceShareState::Personal
    );
    assert!(space_info(&mut alice, space).ai_enabled);

    let info = share_space(&mut alice, space);
    assert_eq!(info.role, Some(MemberRole::Owner));
    let shown = space_info(&mut alice, space);
    assert_eq!(
        shown.share_state,
        SpaceShareState::Shared,
        "the server took the Create"
    );
    assert_eq!(shown.role, Some(MemberRole::Owner));
    assert_eq!(shown.member_count, 1);
    assert!(
        !shown.ai_enabled,
        "a shared space starts with AI off (ADR-0047)"
    );
    assert!(plan_of(&mut alice, space).already_shared);
    // A second share answers AlreadyExists.
    assert_eq!(
        fails(&mut alice, Request::SpaceShare { space_id: space }).code,
        ErrorCode::AlreadyExists
    );
    // The favourite moved to the account document: the structure's map is empty, the tree shows it.
    assert_eq!(tree(&mut alice.core, space).favorites, vec![plan]);
    assert!(
        alice
            .core
            .structure(space)
            .unwrap()
            .model
            .favorites()
            .is_empty()
    );

    // Bob has no account on the server yet: the share signup will create it.
    assert!(matches!(
        server.fetch_log_sync(bob_account, None),
        Err(kb_sync::SyncError::NotFound)
    ));
    let digits = code_invite(&mut alice, &mut bob, space, MemberRole::Editor);
    assert_eq!(digits.len(), 6);
    assert!(
        server.fetch_log_sync(bob_account, None).is_ok(),
        "the signup created the account"
    );
    assert!(
        bob.core.sync_info().unwrap().configured,
        "joining points sync at the server"
    );

    // Both settle: the whole history arrives.
    Dev::settle(&mut [&mut alice, &mut bob], 4);
    let alice_view = members(&mut alice, space);
    let bob_view = members(&mut bob, space);
    assert_eq!(alice_view.members.len(), 2);
    assert_eq!(bob_view.members.len(), 2);
    for (view, other) in [(&alice_view, "Bob"), (&bob_view, "Alice")] {
        let member = member_of(view, other);
        assert!(member.verified, "the digits authenticated both accounts");
        assert!(member.safety_number.is_some());
        assert_eq!(
            member.role,
            if other == "Bob" {
                MemberRole::Editor
            } else {
                MemberRole::Owner
            }
        );
    }
    assert_eq!(
        member_of(&alice_view, "Bob").safety_number,
        member_of(&bob_view, "Alice").safety_number,
        "the safety number is the same on both sides"
    );
    assert_eq!(bob_view.my_role, MemberRole::Editor);
    assert_eq!(outline_of(&mut bob, space), outline_of(&mut alice, space));
    assert_eq!(bob.texts(plan), alice.texts(plan));
    assert_eq!(bob.texts(notes), vec!["alice wrote notes".to_owned()]);
    let bob_tree = tree(&mut bob.core, space);
    assert_eq!(bob_tree.trashed.len(), 1, "the trashed page travelled");
    assert_eq!(bob_tree.trashed[0].page.id, trashed);
    assert_eq!(
        bob_tree.favorites,
        Vec::<PageId>::new(),
        "favourites are per person: Alice's pin is not Bob's"
    );
    assert_eq!(bob.read_attachment(file.attachment_id).unwrap(), bytes);
    assert_eq!(space_info(&mut bob, space).role, Some(MemberRole::Editor));
    assert_eq!(space_info(&mut bob, space).member_count, 2);
    // The inviter's session ended when the Join folded.
    assert_eq!(invite_info(&mut alice).state, InviteSessionState::Done);
    assert_eq!(
        alice_view
            .invites
            .iter()
            .map(|invite| (invite.kind, invite.status, invite.bound_account))
            .collect::<Vec<_>>(),
        vec![(
            kb_types::InviteKind::Code,
            InviteStatus::Used,
            Some(bob_account)
        )]
    );
    // The panel's other fields.
    assert_eq!(alice_view.epoch, 1);
    assert!(!alice_view.pending_rekey && !alice_view.server_flagged);
    assert_eq!(alice_view.refused_total, 0);
    assert_eq!(alice_view.settings.trash_retention_days, 30);
    assert!(!alice_view.settings.editors_may_link);
    assert!(bob_view.invites.is_empty(), "only Owners see the invites");
}

pub(super) fn plan_of(dev: &mut Dev, space: SpaceId) -> SpaceSharePlan {
    plan(dev, space)
}
