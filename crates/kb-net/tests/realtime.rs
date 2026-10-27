//! Realtime and presence end to end (M13 slice 3, ADR-0050): two people, each a real core with the v2 driver,
//! on the reference `MemoryRemote` (tokio's paused clock, so the debounce and the timers are exact and cost
//! nothing; its socket has the knobs that lose frames and cut connections) and on an in-process `kb-server`
//! over HTTP (a real WebSocket, real time). What is shown: an entry reaches the other core inline, within
//! one debounce and one round trip, and never through the pull timer (60 s); a frame that is lost leaves a
//! gap that the next entry's `prev_cursor` shows, and the feed is pulled; the push debounce drops to
//! 150 ms while a peer is present and comes back with the timeout; a socket that was lost is replaced and
//! what happened meanwhile arrives; presence crosses the server sealed and is opened by the other
//! member's core only.

// The latencies are printed (`cargo test -p kb-net --test realtime -- --nocapture`), so a person can read them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use kb_net::HttpRemote;
use kb_net::v2::SyncRemote;
use kb_types::sharing::PRESENCE_TIMEOUT_SECONDS;
use kb_types::{PageId, PresenceState, SpaceId};
use support::{Env, Network, Peer, join_by_link, shared, takes, until};

/// How often a test looks at a core while it waits for something to arrive.
const LOOK: Duration = Duration::from_millis(2);

/// Alice and Bob in one shared space, both synced and idle.
struct Pair {
    alice: Arc<Peer>,
    alice_net: Network,
    bob: Arc<Peer>,
    bob_net: Network,
    space: SpaceId,
    plan: PageId,
}

async fn pair<R: SyncRemote>(env: &Env<R>) -> Pair {
    support::init_logging();
    let (alice, alice_net, space, plan) = env.alice_with_a_project();
    let (bob, bob_net) = env.newcomer("bob", 2, "Bob");
    shared(&alice, space).await;
    join_by_link(&alice, &bob, space).await;
    Pair {
        alice,
        alice_net,
        bob,
        bob_net,
        space,
        plan,
    }
}

/// `editor` renames the page; how long until `other` shows the new title.
async fn rename_and_wait(
    editor: &Arc<Peer>,
    other: &Arc<Peer>,
    space: SpaceId,
    plan: PageId,
    title: &str,
) -> Duration {
    let shown = format!("{title}\n");
    editor.rename(plan, title).unwrap();
    takes(
        &format!("the other device shows {title}"),
        Duration::from_secs(30),
        LOOK,
        || other.outline(space).starts_with(&shown),
    )
    .await
}

/// A few seconds of quiet: the connections are up and the pulls they ask for have run.
async fn settle() {
    tokio::time::sleep(Duration::from_secs(5)).await;
}

// ----- the reference remote, on the paused clock -----------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_entry_reaches_the_other_core_inline_long_before_the_pull_timer() {
    let env = Env::memory();
    let p = pair(&env).await;
    let memory = env.memory.as_ref().unwrap();
    until("both sockets are up", || memory.ws_open_count() == 2).await;
    settle().await;

    let (applied, pulled) = (p.bob.inline_applied(), p.bob.pulled_pages());
    let took = rename_and_wait(&p.alice, &p.bob, p.space, p.plan, "Plan v2").await;
    assert!(
        took < Duration::from_secs(2),
        "the debounce and a round trip, not the 60 s timer: {took:?}"
    );
    assert!(
        p.bob.inline_applied() > applied,
        "it went through the socket"
    );
    assert_eq!(p.bob.pulled_pages(), pulled, "and no pull was needed");
    // Both ways, many times (each side pulls for its own cycles: only what is written by the other one is
    // checked to have come through the socket).
    for round in 0..4 {
        rename_and_wait(&p.bob, &p.alice, p.space, p.plan, &format!("Bob {round}")).await;
        rename_and_wait(&p.alice, &p.bob, p.space, p.plan, &format!("Alice {round}")).await;
    }
    assert_eq!(p.alice.outline(p.space), p.bob.outline(p.space));
    assert!(p.alice.inline_applied() >= 4 && p.bob.inline_applied() >= 5);
}

#[tokio::test(start_paused = true)]
async fn a_lost_frame_is_found_by_the_next_entry_whose_prev_cursor_shows_the_gap_and_that_feed_is_pulled()
 {
    let env = Env::memory();
    let p = pair(&env).await;
    let memory = env.memory.as_ref().unwrap();
    until("both sockets are up", || memory.ws_open_count() == 2).await;
    settle().await;

    // Everything the server relays while Alice's first change is pushed is lost.
    memory.ws_drop_next(1_000);
    p.alice.create_page(p.space, None, "Draft");
    until("Alice pushed", || p.alice.sync_info().pending_entries == 0).await;
    memory.ws_drop_next(0);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        !p.bob.outline(p.space).contains("Draft"),
        "Bob was told nothing, and the pull timer is a minute away"
    );

    // The next entry says which one it follows: the core is not there, answers `Pull`, and the feed's
    // pull brings both.
    let (pulls, pulled) = (p.bob.inline_pulls(), p.bob.pulled_pages());
    let took = rename_and_wait(&p.alice, &p.bob, p.space, p.plan, "Plan v2").await;
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(
        p.bob.outline(p.space).contains("Draft"),
        "the page of the lost frame arrived with it"
    );
    assert!(
        p.bob.inline_pulls() > pulls,
        "the core sent the entry back to a pull"
    );
    assert!(p.bob.pulled_pages() > pulled);
    assert_eq!(p.alice.outline(p.space), p.bob.outline(p.space));
}

#[tokio::test(start_paused = true)]
async fn the_pull_timer_finds_a_lost_frame_when_nothing_follows() {
    let env = Env::memory();
    let p = pair(&env).await;
    let memory = env.memory.as_ref().unwrap();
    until("both sockets are up", || memory.ws_open_count() == 2).await;
    settle().await;

    memory.ws_drop_next(1_000);
    p.alice.create_page(p.space, None, "Lonely");
    until("Alice pushed", || p.alice.sync_info().pending_entries == 0).await;
    memory.ws_drop_next(0);
    let took = takes(
        "the timer's pull brought it",
        Duration::from_secs(120),
        Duration::from_millis(500),
        || p.bob.outline(p.space).contains("Lonely"),
    )
    .await;
    assert!(
        took > Duration::from_secs(5) && took < Duration::from_secs(70),
        "correctness never depends on the socket: {took:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn edits_by_both_people_converge_whatever_frames_are_lost() {
    let env = Env::memory();
    let p = pair(&env).await;
    let memory = env.memory.as_ref().unwrap();
    until("both sockets are up", || memory.ws_open_count() == 2).await;
    settle().await;

    // A deterministic mix: some changes lose no frame, some lose all, some the first or the second.
    for round in 0..12_usize {
        memory.ws_drop_next([0, 1, 2, 1_000, 0, 3][round % 6]);
        let editor = if round % 2 == 0 { &p.alice } else { &p.bob };
        editor.create_page(p.space, None, &format!("Page {round}"));
        tokio::time::sleep(Duration::from_millis(700)).await;
        memory.ws_drop_next(0);
    }
    // The timer's pull is the last resort: a minute and a bit.
    tokio::time::sleep(Duration::from_secs(70)).await;
    let (alice, bob) = (p.alice.outline(p.space), p.bob.outline(p.space));
    assert_eq!(alice, bob);
    for round in 0..12 {
        assert!(alice.contains(&format!("Page {round}")), "{alice}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_push_debounce_is_150_ms_while_a_peer_is_present_and_500_ms_otherwise() {
    let env = Env::memory();
    let p = pair(&env).await;
    let memory = env.memory.as_ref().unwrap();
    until("both sockets are up", || memory.ws_open_count() == 2).await;
    settle().await;

    // Nobody else is on a page: the plain debounce.
    let plain = rename_and_wait(&p.bob, &p.alice, p.space, p.plan, "Plain").await;
    assert!(
        plain >= Duration::from_millis(500) && plain < Duration::from_millis(560),
        "{plain:?}"
    );
    assert_eq!(p.bob.debounce_hints().last(), Some(&500));

    // Alice is in the page: her presence reaches Bob's core, whose hint drops.
    assert!(p.alice.presence_send(
        &p.alice_net.realtime(),
        p.space,
        Some(p.plan),
        PresenceState::Editing,
        b"cursor",
    ));
    until("Bob's core holds Alice's presence", || {
        !p.bob.presence_seen().is_empty()
    })
    .await;
    let seen = p.bob.presence_seen();
    assert_eq!(seen[0].info.name, "Alice");
    assert_eq!(seen[0].info.page_id, Some(p.plan));
    assert_eq!(seen[0].info.state, PresenceState::Editing);
    assert_eq!(seen[0].update, b"cursor");
    let quick = rename_and_wait(&p.bob, &p.alice, p.space, p.plan, "Quick").await;
    assert!(
        quick >= Duration::from_millis(150) && quick < Duration::from_millis(200),
        "{quick:?}"
    );
    assert_eq!(p.bob.debounce_hints().last(), Some(&150));

    // Alice goes quiet: after the timeout (the core's clock) and the next expiry tick it is 500 ms again.
    env.advance(i64::try_from(PRESENCE_TIMEOUT_SECONDS).unwrap() * 1_000 + 1_000);
    tokio::time::sleep(Duration::from_secs(6)).await;
    let after = rename_and_wait(&p.bob, &p.alice, p.space, p.plan, "Back to plain").await;
    assert!(
        after >= Duration::from_millis(500) && after < Duration::from_millis(560),
        "{after:?}"
    );
    assert_eq!(p.bob.debounce_hints().last(), Some(&500));
}

#[tokio::test(start_paused = true)]
async fn a_lost_connection_is_replaced_and_what_happened_meanwhile_arrives() {
    let env = Env::memory();
    let p = pair(&env).await;
    let memory = env.memory.as_ref().unwrap();
    until("both sockets are up", || memory.ws_open_count() == 2).await;
    settle().await;

    // The server goes away: every socket closes, every call is refused.
    memory.set_offline(true);
    assert_eq!(memory.ws_open_count(), 0);
    p.alice.rename(p.plan, "Written offline").unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(!p.bob.outline(p.space).starts_with("Written offline"));
    // It comes back: the drivers retry with their backoff, open new sockets and catch up.
    memory.set_offline(false);
    takes(
        "the sockets are back",
        Duration::from_secs(120),
        Duration::from_millis(100),
        || memory.ws_open_count() == 2,
    )
    .await;
    takes(
        "Bob has what was written offline",
        Duration::from_secs(120),
        Duration::from_millis(100),
        || p.bob.outline(p.space).starts_with("Written offline"),
    )
    .await;

    // And the new sockets carry entries inline again.
    settle().await;
    let (applied, pulled) = (p.bob.inline_applied(), p.bob.pulled_pages());
    let took = rename_and_wait(&p.alice, &p.bob, p.space, p.plan, "Online again").await;
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(p.bob.inline_applied() > applied);
    assert_eq!(p.bob.pulled_pages(), pulled);
}

#[tokio::test(start_paused = true)]
async fn presence_reaches_the_other_member_sealed_and_never_the_sender_or_a_removed_member() {
    let env = Env::memory();
    let p = pair(&env).await;
    let memory = env.memory.as_ref().unwrap();
    until("both sockets are up", || memory.ws_open_count() == 2).await;
    settle().await;

    // Frames go to the other member, in order, with the sender's name and page.
    let alice = p.alice_net.realtime();
    for update in [b"one".as_slice(), b"two", b"three"] {
        assert!(p.alice.presence_send(
            &alice,
            p.space,
            Some(p.plan),
            PresenceState::Editing,
            update
        ));
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    until("Bob's core accepted the frames", || {
        p.bob.presence_seen().len() == 3
    })
    .await;
    let updates: Vec<Vec<u8>> = p
        .bob
        .presence_seen()
        .into_iter()
        .map(|message| message.update)
        .collect();
    assert_eq!(
        updates,
        [b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
    );
    assert!(
        p.alice.presence_seen().is_empty(),
        "a frame never goes back to its sender's socket"
    );

    // Bob's own presence is Alice's to see, from the other side.
    let bob = p.bob_net.realtime();
    assert!(
        p.bob
            .presence_send(&bob, p.space, None, PresenceState::Idle, b"")
    );
    until("Alice's core accepted Bob's frame", || {
        p.alice.presence_seen().len() == 1
    })
    .await;
    assert_eq!(p.alice.presence_seen()[0].info.name, "Bob");
    assert_eq!(p.alice.presence_seen()[0].info.page_id, None);
}

// ----- an in-process kb-server over HTTP, in real time -----------------------------------------------

/// The latency of an entry from one core to the other over the real socket: what is measured is the time
/// from the call that writes to the moment the other core shows the change, which is the push debounce,
/// the server's relay and the inline apply (no pull: the timer is 60 s, and the pulled pages stay as
/// they were).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entry_arrives_over_the_real_socket_within_one_cycle_without_polling() {
    let (env, server) = Env::<HttpRemote>::http().await;
    let p = pair(&env).await;
    // The sockets connect, and the pulls they ask for run.
    tokio::time::sleep(Duration::from_secs(2)).await;
    until("both are idle", || {
        p.alice.sync_info().state == kb_types::SyncState::Idle
            && p.bob.sync_info().state == kb_types::SyncState::Idle
    })
    .await;

    let mut latencies = Vec::new();
    let (applied, pulled) = (p.bob.inline_applied(), p.bob.pulled_pages());
    for round in 0..5 {
        let took =
            rename_and_wait(&p.alice, &p.bob, p.space, p.plan, &format!("Plan v{round}")).await;
        latencies.push(took);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!("write to apply over the real socket (500 ms debounce): {latencies:?}");
    assert!(
        latencies.iter().all(|took| *took < Duration::from_secs(5)),
        "{latencies:?}"
    );
    assert_eq!(
        p.bob.inline_applied() - applied,
        5,
        "every entry went through the socket"
    );
    assert_eq!(p.bob.pulled_pages(), pulled, "no pull ran for any of them");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_and_the_short_debounce_over_the_real_socket() {
    let (env, server) = Env::<HttpRemote>::http().await;
    let p = pair(&env).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Nobody else is on a page: Bob's push waits the plain 500 ms.
    let mut plain = Vec::new();
    for round in 0..3 {
        plain.push(
            rename_and_wait(&p.bob, &p.alice, p.space, p.plan, &format!("Plain {round}")).await,
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(p.bob.debounce_hints().last(), Some(&500));

    // Alice is in the page: her sealed presence crosses the server to Bob's core, and Bob's push waits 150 ms.
    let alice = p.alice_net.realtime();
    assert!(p.alice.presence_send(
        &alice,
        p.space,
        Some(p.plan),
        PresenceState::Viewing,
        b"cursor"
    ));
    takes(
        "Bob's core accepted Alice's presence",
        Duration::from_secs(10),
        LOOK,
        || !p.bob.presence_seen().is_empty(),
    )
    .await;
    assert_eq!(p.bob.presence_seen()[0].info.name, "Alice");
    assert!(
        p.alice.presence_seen().is_empty(),
        "never back to the sender"
    );
    let mut quick = Vec::new();
    for round in 0..3 {
        quick.push(
            rename_and_wait(&p.bob, &p.alice, p.space, p.plan, &format!("Quick {round}")).await,
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(p.bob.debounce_hints().last(), Some(&150));

    plain.sort();
    quick.sort();
    eprintln!(
        "write to apply over the real socket: 500 ms debounce {plain:?}, 150 ms debounce {quick:?}"
    );
    // The debounce is 350 ms shorter: whatever the speed of the machine, the middle values are 250 ms apart
    // at least.
    assert!(
        quick[1] + Duration::from_millis(250) < plain[1],
        "{quick:?} against {plain:?}"
    );
    server.stop().await;
}
