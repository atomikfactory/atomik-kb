//! The invite task where invitations go wrong, with real cores on the reference remote (tokio's paused
//! clock): a cancellation reaches the other side and closes the mailbox, a code typed too late or for a
//! mailbox that was closed, a revoked, an expired and a used link (one message for every dead invite), a
//! preview nobody answers, and a preview the invitee cancels. The paths that go right are in `fleet.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use kb_net::devices::{CODE_EXPIRED, PEER_LEFT};
use kb_net::invite::{INVALID_INVITE, PREVIEW_TIMEOUT_MESSAGE};
use kb_sync::v2::MemoryRemote;
use kb_types::{InviteSessionState, InviteStatus, MemberRole, Request};
use support::{Env, link_on_the_server, shared, until};

const DAY_MS: i64 = 86_400_000;
const MINUTE_MS: i64 = 60_000;

#[tokio::test(start_paused = true)]
async fn cancelling_a_code_invite_tells_the_other_side_and_closes_the_mailbox() {
    support::init_logging();
    let env = Env::memory();
    let (alice, _alice_net, space, _) = env.alice_with_a_project();
    let (bob, _bob_net) = env.newcomer("bob", 2, "Bob");
    shared(&alice, space).await;

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
    bob.request(Request::InviteCodeJoin {
        server_url: env.url.clone(),
        code: alice.invite().code.unwrap(),
    })
    .unwrap();
    until("both show the digits", || {
        alice.invite_state() == InviteSessionState::Confirming
            && bob.invite_state() == InviteSessionState::Confirming
    })
    .await;

    alice.request(Request::InviteCancel).unwrap();
    // Bob hears of it: through the abort (Cancelled) when the core numbered it as the mailbox does, and
    // in any case through the mailbox the task closed behind it ("the other device gave up").
    until("Bob hears of it", || {
        matches!(
            bob.invite_state(),
            InviteSessionState::Cancelled | InviteSessionState::Failed
        )
    })
    .await;
    if bob.invite_state() == InviteSessionState::Failed {
        assert_eq!(bob.invite().error.as_deref(), Some(PEER_LEFT));
    }
    assert_eq!(alice.invite_state(), InviteSessionState::Cancelled);
    // The task closed the mailbox behind the abort.
    let mailbox = alice
        .with(|core| core.invite_mailbox_id())
        .unwrap()
        .expect("the session has a mailbox");
    let memory = env.memory.as_ref().unwrap();
    until("the mailbox is closed", || {
        memory
            .pair_messages_sync(mailbox, 0, 0)
            .is_ok_and(|page| page.closed)
    })
    .await;
    assert!(
        !env.account_on_server(bob.account()).await,
        "a cancelled code invite creates no account"
    );
}

#[tokio::test(start_paused = true)]
async fn a_code_typed_after_its_mailbox_expired_is_the_code_expired() {
    support::init_logging();
    let env = Env::memory();
    let (alice, _alice_net, space, _) = env.alice_with_a_project();
    let (bob, _bob_net) = env.newcomer("bob", 2, "Bob");
    shared(&alice, space).await;

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
    // A pairing mailbox lives ten minutes on the server's clock.
    env.advance(11 * MINUTE_MS);
    bob.request(Request::InviteCodeJoin {
        server_url: env.url.clone(),
        code,
    })
    .unwrap();
    until("Bob's session fails", || {
        bob.invite_state() == InviteSessionState::Failed
    })
    .await;
    assert_eq!(bob.invite().error.as_deref(), Some(CODE_EXPIRED));
}

#[tokio::test(start_paused = true)]
async fn a_code_whose_mailbox_was_closed_says_the_other_device_gave_up() {
    support::init_logging();
    let env = Env::memory();
    let (alice, _alice_net, space, _) = env.alice_with_a_project();
    let (bob, _bob_net) = env.newcomer("bob", 2, "Bob");
    shared(&alice, space).await;

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
    alice.request(Request::InviteCancel).unwrap();
    let mailbox = alice
        .with(|core| core.invite_mailbox_id())
        .unwrap()
        .unwrap();
    let memory = env.memory.as_ref().unwrap();
    until("the mailbox is closed", || {
        memory
            .pair_messages_sync(mailbox, 0, 0)
            .is_ok_and(|page| page.closed)
    })
    .await;

    bob.request(Request::InviteCodeJoin {
        server_url: env.url.clone(),
        code,
    })
    .unwrap();
    until("Bob's session fails", || {
        bob.invite_state() == InviteSessionState::Failed
    })
    .await;
    assert_eq!(bob.invite().error.as_deref(), Some(PEER_LEFT));
}

#[tokio::test(start_paused = true)]
async fn a_revoked_an_expired_and_a_used_link_read_the_same() {
    support::init_logging();
    let env = Env::memory();
    let (alice, _alice_net, space, _) = env.alice_with_a_project();
    shared(&alice, space).await;

    // Revoked.
    let (carol, _carol_net) = env.newcomer("carol", 3, "Carol");
    let revoked = link_on_the_server(&alice, space, MemberRole::Viewer, None).await;
    alice
        .request(Request::InviteRevoke {
            space_id: space,
            invite_id: revoked.invite_id,
        })
        .unwrap();
    until("the revocation is on the server", || {
        alice.sync_info().pending_entries == 0
            && alice.members(space).invites.iter().any(|invite| {
                invite.invite_id == revoked.invite_id && invite.status == InviteStatus::Revoked
            })
    })
    .await;
    carol
        .request(Request::InviteLinkOpen {
            link: revoked.link.clone(),
        })
        .unwrap();
    until("Carol's session fails", || {
        carol.invite_state() == InviteSessionState::Failed
    })
    .await;
    assert_eq!(carol.invite().error.as_deref(), Some(INVALID_INVITE));

    // Used: Carol joins with a fresh link, Dave tries the same one.
    let once = link_on_the_server(&alice, space, MemberRole::Viewer, None).await;
    carol
        .request(Request::InviteLinkOpen {
            link: once.link.clone(),
        })
        .unwrap();
    until("Carol sees the preview", || {
        carol.invite_state() == InviteSessionState::Previewing
    })
    .await;
    carol.request(Request::InviteConfirm).unwrap();
    until("Carol joined", || {
        carol.invite_state() == InviteSessionState::Done
    })
    .await;
    let (dave, _dave_net) = env.newcomer("dave", 4, "Dave");
    dave.request(Request::InviteLinkOpen {
        link: once.link.clone(),
    })
    .unwrap();
    until("Dave's session fails", || {
        dave.invite_state() == InviteSessionState::Failed
    })
    .await;
    assert_eq!(dave.invite().error.as_deref(), Some(INVALID_INVITE));
    assert!(
        !env.account_on_server(dave.account()).await,
        "a dead invite creates no account"
    );

    // Expired: a one-day link, opened two days later.
    let (erin, _erin_net) = env.newcomer("erin", 5, "Erin");
    let short = link_on_the_server(&alice, space, MemberRole::Viewer, Some(1)).await;
    env.advance(2 * DAY_MS);
    erin.request(Request::InviteLinkOpen {
        link: short.link.clone(),
    })
    .unwrap();
    until("Erin's session fails", || {
        erin.invite_state() == InviteSessionState::Failed
    })
    .await;
    assert_eq!(erin.invite().error.as_deref(), Some(INVALID_INVITE));
}

#[tokio::test(start_paused = true)]
async fn a_preview_nobody_answers_ends_as_failed_after_half_an_hour() {
    support::init_logging();
    let env = Env::memory();
    let (alice, _alice_net, space, _) = env.alice_with_a_project();
    let (carol, _carol_net) = env.newcomer("carol", 3, "Carol");
    shared(&alice, space).await;
    let link = link_on_the_server(&alice, space, MemberRole::Viewer, None).await;
    carol
        .request(Request::InviteLinkOpen { link: link.link })
        .unwrap();
    until("Carol sees the preview", || {
        carol.invite_state() == InviteSessionState::Previewing
    })
    .await;

    tokio::time::sleep(Duration::from_secs(29 * 60)).await;
    assert_eq!(
        carol.invite_state(),
        InviteSessionState::Previewing,
        "still waiting after 29 minutes"
    );
    tokio::time::sleep(Duration::from_secs(2 * 60)).await;
    until("the preview times out", || {
        carol.invite_state() == InviteSessionState::Failed
    })
    .await;
    assert_eq!(
        carol.invite().error.as_deref(),
        Some(PREVIEW_TIMEOUT_MESSAGE)
    );
    assert!(
        carol.space(space).is_none(),
        "nothing of the space stayed on this device"
    );
}

#[tokio::test(start_paused = true)]
async fn the_invitee_can_cancel_the_preview_and_the_invite_stays_open() {
    support::init_logging();
    let env = Env::memory();
    let (alice, _alice_net, space, _) = env.alice_with_a_project();
    let (carol, _carol_net) = env.newcomer("carol", 3, "Carol");
    shared(&alice, space).await;
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
    carol.request(Request::InviteCancel).unwrap();
    assert_eq!(carol.invite_state(), InviteSessionState::Cancelled);
    assert!(carol.space(space).is_none());
    assert!(!env.account_on_server(carol.account()).await);
    // The invitation was not used: she can open it again and join.
    carol
        .request(Request::InviteLinkOpen { link: link.link })
        .unwrap();
    until("Carol sees the preview again", || {
        carol.invite_state() == InviteSessionState::Previewing
    })
    .await;
    carol.request(Request::InviteConfirm).unwrap();
    until("Carol joined", || {
        carol.invite_state() == InviteSessionState::Done
    })
    .await;
    until("Carol has the pages", || {
        carol.outline(space) == alice.outline(space)
    })
    .await;
}

/// Keeps the type of the reference remote in view for the helpers above.
#[allow(dead_code)]
type Memory = MemoryRemote;
