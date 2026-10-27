//! Three people, each a real core with the v2 driver and the invite tasks, on one server: the reference
//! `MemoryRemote` (on tokio's paused clock, so the debounce and the pull timer cost nothing) and an
//! in-process `kb-server` over HTTP (real time). One scenario runs against both: Alice shares a space,
//! Bob joins with a typed code and the six digits both show (his account on the server made by the share
//! signup), Bob's edit reaches Alice, a role change reaches Bob, Carol (no account on the server yet)
//! joins through a link, and Alice removes Bob.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use kb_net::HttpRemote;
use kb_net::v2::SyncRemote;
use kb_sync::v2::MemoryRemote;
use kb_types::{
    ErrorCode, InviteSessionState, InviteStatus, MemberRole, Request, SpaceShareState, SyncState,
};
use support::{Env, link_on_the_server, shared, until};

/// The whole scenario of the file.
async fn scenario<R: SyncRemote>(env: Env<R>) {
    support::init_logging();
    let (alice, _alice_net, space, plan) = env.alice_with_a_project();
    let (bob, _bob_net) = env.newcomer("bob", 2, "Bob");
    let (carol, _carol_net) = env.newcomer("carol", 3, "Carol");
    shared(&alice, space).await;

    // ----- Bob joins with a typed code ---------------------------------------------------------------
    assert!(
        !env.account_on_server(bob.account()).await,
        "Bob has no account on the server yet"
    );
    alice
        .request(Request::InviteCodeStart {
            space_id: space,
            role: MemberRole::Editor,
        })
        .unwrap();
    until("the inviter shows the code", || {
        alice.invite().code.is_some()
    })
    .await;
    let code = alice.invite().code.unwrap();
    bob.request(Request::InviteCodeJoin {
        server_url: env.url.clone(),
        code,
    })
    .unwrap();
    until("both show the digits", || {
        alice.invite_state() == InviteSessionState::Confirming
            && bob.invite_state() == InviteSessionState::Confirming
    })
    .await;
    let digits = alice.invite().sas.expect("the inviter shows digits");
    assert_eq!(digits.len(), 6);
    assert_eq!(bob.invite().sas, Some(digits), "the same six digits");

    bob.request(Request::InviteConfirm).unwrap();
    until("the inviter sees who is asking", || {
        alice.invite_state() == InviteSessionState::PeerConfirming
    })
    .await;
    assert_eq!(alice.invite().peer_name.as_deref(), Some("Bob"));
    alice.request(Request::InviteConfirm).unwrap();
    until("Bob joined", || {
        bob.invite_state() == InviteSessionState::Done
    })
    .await;
    until("Alice's session saw the Join", || {
        alice.invite_state() == InviteSessionState::Done
    })
    .await;
    assert!(
        env.account_on_server(bob.account()).await,
        "the share signup made Bob's account"
    );

    // The whole space reaches Bob: the tree, his role.
    until("Bob has the pages", || {
        bob.space(space)
            .is_some_and(|info| info.role == Some(MemberRole::Editor))
            && bob.outline(space) == alice.outline(space)
    })
    .await;
    assert_eq!(bob.outline(space), "Plan\n  Notes\n");
    let info = bob.sync_info();
    assert!(info.configured && !info.needs_invite_token, "{info:?}");
    until("Bob's sync is idle", || {
        bob.sync_info().state == SyncState::Idle
    })
    .await;
    let bob_view = bob.members(space);
    let alice_view = alice.members(space);
    assert_eq!(bob_view.members.len(), 2);
    assert_eq!(alice_view.members.len(), 2);
    let alice_sees_bob = alice_view
        .members
        .iter()
        .find(|member| member.name == "Bob")
        .expect("Bob is listed");
    let bob_sees_alice = bob_view
        .members
        .iter()
        .find(|member| member.name == "Alice")
        .expect("Alice is listed");
    assert!(
        alice_sees_bob.verified && bob_sees_alice.verified,
        "the digits verified both"
    );
    assert_eq!(alice_sees_bob.safety_number, bob_sees_alice.safety_number);
    assert!(
        alice_view
            .invites
            .iter()
            .any(|invite| invite.status == InviteStatus::Used)
    );

    // ----- an edit of Bob's reaches Alice, and one of Alice's reaches Bob ---------------------------
    bob.rename(plan, "Plan v2").unwrap();
    until("Alice sees Bob's rename", || {
        alice.outline(space).starts_with("Plan v2\n")
    })
    .await;
    alice.rename(plan, "Plan v3").unwrap();
    until("Bob sees Alice's rename", || {
        bob.outline(space).starts_with("Plan v3\n")
    })
    .await;

    // ----- a role change reaches Bob and the Viewer cannot write ------------------------------------
    alice.role_set(space, bob.account(), MemberRole::Viewer);
    until("Bob's role is Viewer", || {
        bob.space(space)
            .is_some_and(|info| info.role == Some(MemberRole::Viewer))
    })
    .await;
    let refused = bob.rename(plan, "Plan by a Viewer").unwrap_err();
    assert_eq!(refused.code, ErrorCode::Forbidden);
    alice.role_set(space, bob.account(), MemberRole::Editor);
    until("Bob is an Editor again", || {
        bob.space(space)
            .is_some_and(|info| info.role == Some(MemberRole::Editor))
    })
    .await;

    // ----- Carol joins through a link, with no account on the server yet ----------------------------
    assert!(!env.account_on_server(carol.account()).await);
    let link = link_on_the_server(&alice, space, MemberRole::Viewer, None).await;
    carol
        .request(Request::InviteLinkOpen {
            link: link.link.clone(),
        })
        .unwrap();
    until("Carol sees the preview", || {
        carol.invite_state() == InviteSessionState::Previewing
    })
    .await;
    assert!(
        !env.account_on_server(carol.account()).await,
        "looking at an invitation creates no account"
    );
    let preview = carol.invite();
    assert_eq!(preview.inviter_name.as_deref(), Some("Alice"));
    assert_eq!(preview.space_name.as_deref(), Some("Project"));
    assert_eq!(preview.role, Some(MemberRole::Viewer));
    assert!(!preview.verified, "a link verifies nobody");
    carol.request(Request::InviteConfirm).unwrap();
    until("Carol joined", || {
        carol.invite_state() == InviteSessionState::Done
    })
    .await;
    assert!(
        env.account_on_server(carol.account()).await,
        "the share signup made Carol's account"
    );
    until("Carol has the pages", || {
        carol
            .space(space)
            .is_some_and(|info| info.role == Some(MemberRole::Viewer))
            && carol.outline(space) == alice.outline(space)
    })
    .await;
    let carol_info = carol.sync_info();
    assert!(
        carol_info.configured && !carol_info.needs_invite_token,
        "{carol_info:?}"
    );
    until("everybody sees three members", || {
        [&alice, &bob, &carol]
            .iter()
            .all(|peer| peer.members(space).members.len() == 3)
    })
    .await;

    // ----- Alice removes Bob: his copy becomes an archive, Carol keeps syncing ----------------------
    alice
        .request(Request::MemberRemove {
            space_id: space,
            account_id: bob.account(),
        })
        .unwrap();
    // A removed member is told nothing by the server's socket (it is no member any more when the
    // `Remove` is announced): the periodic pull finds it, and the test asks for that cycle.
    until("the Remove is on the server", || {
        alice.sync_info().pending_entries == 0
    })
    .await;
    bob.request(Request::SyncNow).unwrap();
    until("Bob's copy is an archive", || {
        bob.space(space)
            .is_some_and(|info| info.share_state == SpaceShareState::Archived)
    })
    .await;
    alice.create_page(space, None, "After Bob");
    until("Carol sees what Alice wrote after the removal", || {
        carol.outline(space).contains("After Bob")
    })
    .await;
    assert!(
        !bob.outline(space).contains("After Bob"),
        "a removed member never receives what is written afterwards"
    );
}

#[tokio::test(start_paused = true)]
async fn three_people_share_a_space_through_the_reference_remote() {
    scenario::<MemoryRemote>(Env::memory()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_people_share_a_space_through_kb_server_over_http() {
    let (env, server) = Env::<HttpRemote>::http().await;
    scenario::<HttpRemote>(env).await;
    server.stop().await;
}
