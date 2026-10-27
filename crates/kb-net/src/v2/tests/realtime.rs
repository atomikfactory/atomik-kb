//! The driver and the realtime socket (M13 slice 3, ADR-0050), against the scripted host and remote of
//! the parent module, whose remote hands out sockets the test holds the other end of: the subscription
//! (every feed, the personal one included, before any pull, again when the list changes), the frames the
//! socket delivers (an inline entry the core applies, one it cannot apply and a gap that pulls one feed,
//! `changed` pings, presence), the connection changes, the expiry tick, the push debounce that follows the
//! host's hint, a socket that ends and the shell's handle. Tokio's clock is paused, so every delay is
//! exact. The reference remote's socket and a real server are in `tests/realtime.rs`.

use kb_sync::SyncError;
use kb_sync::v2::PresenceFrame;
use kb_types::sharing::PRESENCE_MAX_FRAME_BYTES;

use super::*;
use crate::realtime::{
    COMMAND_QUEUE, EVENT_QUEUE, EXPIRE_INTERVAL, RealtimeCommand, RealtimeEvent,
};

/// The test's ends of a socket the fake remote hands the driver.
struct Wire {
    events: mpsc::Sender<RealtimeEvent>,
    commands: mpsc::Receiver<RealtimeCommand>,
}

impl Wire {
    fn new() -> (Realtime, Self) {
        let (events, event_receiver) = mpsc::channel(EVENT_QUEUE);
        let (command_sender, commands) = mpsc::channel(COMMAND_QUEUE);
        (
            Realtime {
                events: event_receiver,
                commands: command_sender,
            },
            Self { events, commands },
        )
    }

    async fn send(&self, event: RealtimeEvent) {
        self.events.send(event).await.unwrap();
    }

    /// What the driver sent the socket so far.
    fn commands(&mut self) -> Vec<RealtimeCommand> {
        let mut sent = Vec::new();
        while let Ok(command) = self.commands.try_recv() {
            sent.push(command);
        }
        sent
    }
}

/// A rig whose remote has one socket to give.
fn rig_with_socket(feeds: Vec<FeedWork>) -> (Rig, Wire) {
    let rig = Rig::new(feeds);
    let (realtime, wire) = Wire::new();
    rig.remote.with(|data| data.sockets.push_back(realtime));
    (rig, wire)
}

fn entry(feed: FeedId, prev_cursor: u64, cursor: u64) -> InlineEntry {
    InlineEntry {
        feed,
        prev_cursor,
        cursor,
        chain_after: [9; 32],
        entry: vec![cursor as u8; 16],
    }
}

fn applied(cursor: u64) -> SyncApplied {
    SyncApplied {
        applied: 1,
        new_cursor: cursor,
        ..SyncApplied::default()
    }
}

/// A rig of the personal feed and one shared feed whose first cycle has run, the shared feed at cursor 20.
async fn running() -> (Rig, Wire) {
    let (mut rig, wire) = rig_with_socket(vec![personal_work(), shared_work(shared_a())]);
    rig.remote.with(|data| {
        data.changes
            .insert(shared_a(), VecDeque::from([page(&[20], false)]));
    });
    rig.one_cycle().await;
    assert_eq!(rig.host.with(|data| data.cursors[&shared_a()]), 20);
    (rig, wire)
}

// ----- the subscription ------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_socket_opens_after_the_session_and_subscribes_to_every_feed_before_any_pull() {
    let (mut rig, mut wire) = rig_with_socket(vec![personal_work(), shared_work(shared_a())]);
    rig.one_cycle().await;
    let steps = rig.calls(&["authenticate", "realtime", "sync_feeds", "changes"]);
    assert_eq!(steps[..3], ["r:authenticate", "r:realtime", "h:sync_feeds"]);
    assert!(
        steps[3].starts_with("r:changes"),
        "no feed was pulled before the subscription was queued: {steps:?}"
    );
    // The personal feed is on the list too (the realtime path serves a person's own devices), sorted.
    assert_eq!(
        wire.commands(),
        vec![RealtimeCommand::Subscribe(vec![personal(), shared_a()])]
    );
    assert_eq!(rig.count("r:realtime"), 1);
}

#[tokio::test(start_paused = true)]
async fn the_subscription_is_sent_again_when_the_feed_list_changes_and_only_then() {
    let (mut rig, mut wire) = rig_with_socket(vec![personal_work(), shared_work(shared_a())]);
    let first = vec![personal_work(), shared_work(shared_a())];
    let second = vec![
        personal_work(),
        shared_work(shared_a()),
        shared_work(shared_b()),
    ];
    // Two listings per cycle: the first cycle sees the first list twice, the later ones the second.
    rig.host.with(|data| {
        data.feed_lists = VecDeque::from([first.clone(), first, second]);
    });
    rig.start();
    rig.until_cycles(1).await;
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(3).await;
    assert_eq!(
        wire.commands(),
        vec![
            RealtimeCommand::Subscribe(vec![personal(), shared_a()]),
            RealtimeCommand::Subscribe(vec![personal(), shared_a(), shared_b()]),
        ],
        "one message per change of the set"
    );
}

#[tokio::test(start_paused = true)]
async fn a_cycle_that_posted_a_membership_entry_subscribes_again_for_the_server_checks_the_access_list()
 {
    let (create, profile) = membership_entries();
    let feed = kb_sync::v2::peek_membership(&create).unwrap().feed;
    let (mut rig, mut wire) = rig_with_socket(vec![personal_work(), shared_work(feed)]);
    rig.host
        .with(|data| data.membership.insert(feed, vec![create, profile]));
    rig.one_cycle().await;
    let set = vec![personal(), feed];
    let mut expected = set.clone();
    expected.sort_unstable();
    assert_eq!(
        wire.commands(),
        vec![
            RealtimeCommand::Subscribe(expected.clone()),
            RealtimeCommand::Subscribe(expected),
        ],
        "before the feed existed on the server, and once the Create was posted"
    );
    let steps = rig.calls(&["realtime", "feeds_create", "sync_membership_posted"]);
    let tag = tag(feed);
    assert_eq!(
        steps,
        vec![
            "r:realtime".to_owned(),
            format!("r:feeds_create#{tag}"),
            format!("h:sync_membership_posted#{tag}"),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_remote_without_a_socket_runs_as_before() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host.with(|data| data.debounce_ms = 150);
    rig.one_cycle().await;
    assert_eq!(rig.count("r:realtime"), 1, "asked once per link");
    rig.host.emit(&Event::SyncRequested);
    rig.remote.with(|data| data.times.clear());
    let at = Instant::now();
    rig.until_cycles(2).await;
    assert_eq!(
        rig.remote.times("server_info")[0] - at,
        Duration::from_millis(500)
    );
    assert_eq!(
        rig.count("h:sync_push_debounce_ms"),
        0,
        "no socket, no presence, nothing to ask"
    );
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(rig.count("h:presence_expire"), 0);
}

// ----- inline entries, gaps and pings -----------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_inline_entry_the_core_applies_costs_no_pull_and_moves_the_feeds_cursor() {
    let (rig, wire) = running().await;
    rig.host
        .with(|data| data.inline_outcomes = VecDeque::from([InlineOutcome::Applied(applied(21))]));
    wire.send(RealtimeEvent::Entry(entry(shared_a(), 20, 21)))
        .await;
    until("the entry reached the core", || {
        rig.count("h:sync_feed_apply_inline#2") == 1
    })
    .await;
    assert_eq!(
        rig.host.with(|data| data.inline_calls.clone()),
        [(shared_a(), 21)]
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        rig.count("r:changes#2"),
        1,
        "no pull: the entry was the next one"
    );
    assert_eq!(rig.count("r:server_info"), 1, "no cycle either");
    assert_eq!(rig.host.cycles(), 1);
    // The cursor moved: the `changed` frame of the same entry has nothing to pull.
    wire.send(RealtimeEvent::Changed {
        feed: shared_a(),
        cursor: 21,
    })
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(rig.count("r:changes#2"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_gap_makes_the_core_answer_pull_and_that_one_feed_is_pulled_outside_the_cycle() {
    let (rig, wire) = running().await;
    let states = rig.host.with(|data| data.states.len());
    rig.remote.with(|data| {
        data.changes
            .insert(shared_a(), VecDeque::from([page(&[21, 22, 23, 24], false)]));
    });
    // The core answers `Pull` by default (it is not at the entry's `prev_cursor`).
    wire.send(RealtimeEvent::Entry(entry(shared_a(), 23, 24)))
        .await;
    until("the feed was pulled", || rig.count("r:changes#2") == 2).await;
    until("the pulled page reached the core", || {
        rig.count("h:sync_feed_apply#2") == 2
    })
    .await;
    assert_eq!(rig.host.with(|data| data.cursors[&shared_a()]), 24);
    assert_eq!(
        rig.count("r:changes#1"),
        1,
        "the personal feed was not pulled"
    );
    assert_eq!(rig.count("r:server_info"), 1, "it was not a cycle");
    assert_eq!(
        rig.host.with(|data| data.states.len()),
        states,
        "no state was reported"
    );
    // The feed's membership log was looked at first, like a cycle's pull does.
    assert_eq!(rig.count("h:sync_membership_pending#2"), 2);
}

#[tokio::test(start_paused = true)]
async fn an_entry_the_core_already_has_does_nothing() {
    let (rig, wire) = running().await;
    rig.host
        .with(|data| data.inline_outcomes = VecDeque::from([InlineOutcome::Known]));
    wire.send(RealtimeEvent::Entry(entry(shared_a(), 18, 19)))
        .await;
    until("the entry reached the core", || {
        rig.count("h:sync_feed_apply_inline#2") == 1
    })
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(rig.count("r:changes#2"), 1);
    assert_eq!(rig.host.cycles(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_applied_entry_that_parked_items_pulls_the_feed_for_the_snapshots() {
    let (rig, wire) = running().await;
    rig.host.with(|data| {
        data.inline_outcomes = VecDeque::from([InlineOutcome::Applied(SyncApplied {
            pending: 1,
            ..applied(21)
        })]);
    });
    wire.send(RealtimeEvent::Entry(entry(shared_a(), 20, 21)))
        .await;
    until("the feed was pulled", || rig.count("r:changes#2") == 2).await;
}

#[tokio::test(start_paused = true)]
async fn an_entry_for_a_feed_the_socket_was_not_subscribed_to_never_reaches_the_core() {
    let (rig, wire) = running().await;
    wire.send(RealtimeEvent::Entry(entry(shared_b(), 0, 1)))
        .await;
    wire.send(RealtimeEvent::Changed {
        feed: shared_b(),
        cursor: 5,
    })
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(rig.count("h:sync_feed_apply_inline#3"), 0);
    assert_eq!(rig.host.cycles(), 1);
    assert_eq!(rig.count("r:changes#3"), 0);
}

#[tokio::test(start_paused = true)]
async fn a_changed_frame_pulls_its_feed_only_when_it_is_above_the_feeds_cursor_and_news() {
    let (rig, wire) = running().await;
    let changed = |feed, cursor| RealtimeEvent::Changed { feed, cursor };
    // At or below what the pull reached: nothing to fetch.
    wire.send(changed(shared_a(), 20)).await;
    wire.send(changed(shared_a(), 7)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(rig.count("r:changes#2"), 1);
    // Above it: that feed (not the personal one) is pulled, at once, long before the timer.
    let at = Instant::now();
    wire.send(changed(shared_a(), 21)).await;
    until("the feed was pulled", || rig.count("r:changes#2") == 2).await;
    assert!(Instant::now() - at < Duration::from_secs(1));
    assert_eq!(rig.count("r:changes#1"), 1);
    assert_eq!(rig.count("r:server_info"), 1, "not a cycle");
    // The same announcement again (the server repeats itself) costs nothing, a higher one pulls.
    wire.send(changed(shared_a(), 21)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(rig.count("r:changes#2"), 2);
    wire.send(changed(shared_a(), 22)).await;
    until("the next growth was pulled", || {
        rig.count("r:changes#2") == 3
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn an_entry_the_core_cannot_take_for_a_failure_of_its_own_falls_back_to_a_pull_cycle() {
    let (rig, wire) = running().await;
    rig.host
        .fail("sync_feed_apply_inline", ApiError::internal("disk is full"));
    wire.send(RealtimeEvent::Entry(entry(shared_a(), 20, 21)))
        .await;
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:server_info"), 2, "a whole pull cycle ran");
}

#[tokio::test(start_paused = true)]
async fn a_pull_of_one_feed_that_cannot_run_becomes_a_pull_cycle() {
    // The feed is not the core's (a space it has not learned of yet): the cycle's pull of the personal
    // feed may teach it.
    let (rig, wire) = running().await;
    rig.host.with(|data| {
        data.feed_lists = VecDeque::from([vec![personal_work()]]);
    });
    wire.send(RealtimeEvent::Entry(entry(shared_a(), 23, 24)))
        .await;
    rig.until_cycles(2).await;
    assert_eq!(rig.count("r:server_info"), 2);
}

// ----- the connection --------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_connected_socket_runs_a_pull_cycle_and_a_lost_one_leaves_the_timer_alone() {
    let (rig, wire) = running().await;
    let at = Instant::now();
    // The first connection, or a reconnection: what was relayed meanwhile was lost.
    wire.send(RealtimeEvent::Connected).await;
    rig.until_cycles(2).await;
    assert!(
        Instant::now() - at < Duration::from_secs(1),
        "not the timer"
    );
    assert_eq!(
        rig.count("h:sync_feed_outbox#2"),
        1,
        "a pull cycle: only the first, full cycle pushed"
    );
    // A lost socket asks for nothing; the timer's pull is what is left.
    wire.send(RealtimeEvent::Disconnected).await;
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(rig.host.cycles(), 2);
    tokio::time::sleep(Duration::from_millis(PULL_INTERVAL_MS)).await;
    assert_eq!(rig.host.cycles(), 3, "the pull timer still runs");
}

#[tokio::test(start_paused = true)]
async fn a_socket_that_ends_is_replaced_by_the_next_cycle_after_a_growing_pause() {
    let (mut rig, wire) = rig_with_socket(vec![personal_work(), shared_work(shared_a())]);
    let (second, mut second_wire) = Wire::new();
    let (third, third_wire) = Wire::new();
    rig.remote.with(|data| {
        data.sockets.push_back(second);
        data.sockets.push_back(third);
    });
    rig.one_cycle().await;
    assert_eq!(rig.count("r:realtime"), 1);

    // The first one ends: another is opened by a pull a second later, and subscribed.
    let ended = Instant::now();
    drop(wire);
    rig.until_cycles(2).await;
    let waited = Instant::now() - ended;
    assert!(
        waited >= BACKOFF_MIN && waited < BACKOFF_MIN + Duration::from_millis(50),
        "{waited:?}"
    );
    assert_eq!(rig.count("r:realtime"), 2);
    assert_eq!(
        second_wire.commands(),
        vec![RealtimeCommand::Subscribe(vec![personal(), shared_a()])]
    );

    // It ends before it ever connected: the pause doubles.
    let ended = Instant::now();
    drop(second_wire);
    rig.until_cycles(3).await;
    let waited = Instant::now() - ended;
    assert!(
        waited >= 2 * BACKOFF_MIN && waited < 2 * BACKOFF_MIN + Duration::from_millis(50),
        "{waited:?}"
    );
    assert_eq!(rig.count("r:realtime"), 3);

    // One that connected resets it.
    third_wire.send(RealtimeEvent::Connected).await;
    rig.until_cycles(4).await;
    let ended = Instant::now();
    drop(third_wire);
    // No socket is left to hand out: the cycle asks and gets none, and goes on without.
    rig.until_cycles(5).await;
    let waited = Instant::now() - ended;
    assert!(
        waited >= BACKOFF_MIN && waited < BACKOFF_MIN + Duration::from_millis(50),
        "{waited:?}"
    );
    assert_eq!(rig.count("r:realtime"), 4);
}

#[tokio::test(start_paused = true)]
async fn a_socket_the_server_refused_runs_a_cycle_that_authenticates_again_or_reports_why_not() {
    let (rig, wire) = running().await;
    let at = Instant::now();
    wire.send(RealtimeEvent::Refused).await;
    rig.until_cycles(2).await;
    assert!(
        Instant::now() - at < Duration::from_secs(1),
        "not the timer"
    );
    // The remote's session is gone (a 401 dropped it): the cycle opens a new one.
    rig.remote.with(|data| data.authenticated = false);
    wire.send(RealtimeEvent::Refused).await;
    rig.until_cycles(3).await;
    assert_eq!(rig.remote.with(|data| data.authentications), 2);
    // A device the server turned away: the cycle says so and the driver does not loop.
    rig.remote.with(|data| data.authenticated = false);
    rig.remote
        .fail_always("authenticate", &SyncError::Unauthorized);
    wire.send(RealtimeEvent::Refused).await;
    rig.until_cycles(4).await;
    assert_eq!(rig.host.last_state().0, SyncState::Error);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(
        rig.host.cycles(),
        4,
        "nothing runs until the timer or an event"
    );
}

// ----- presence --------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_presence_frame_goes_to_the_host_as_it_came() {
    let (rig, wire) = running().await;
    wire.send(RealtimeEvent::Presence(PresenceFrame {
        feed: shared_a(),
        envelope: vec![7; 100],
    }))
    .await;
    wire.send(RealtimeEvent::Presence(PresenceFrame {
        feed: shared_a(),
        envelope: vec![8; 50],
    }))
    .await;
    until("both frames reached the host", || {
        rig.host.with(|data| data.presence.len()) == 2
    })
    .await;
    assert_eq!(
        rig.host.with(|data| data.presence.clone()),
        vec![(shared_a(), vec![7; 100]), (shared_a(), vec![8; 50])]
    );
    assert_eq!(rig.host.cycles(), 1, "presence never starts a cycle");
}

#[tokio::test(start_paused = true)]
async fn the_presence_table_expires_every_five_seconds_while_there_is_a_socket() {
    let (rig, wire) = running().await;
    assert_eq!(rig.count("h:presence_expire"), 0);
    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(
        rig.count("h:presence_expire"),
        4,
        "at 5, 10, 15 and 20 seconds"
    );
    assert_eq!(EXPIRE_INTERVAL, Duration::from_secs(5));
    // A lost socket still expires what timed out (a peer's avatar goes when this device lost its link).
    wire.send(RealtimeEvent::Disconnected).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(rig.count("h:presence_expire"), 6);
}

#[tokio::test(start_paused = true)]
async fn a_host_that_fails_to_expire_or_open_presence_does_not_stop_the_driver() {
    let (rig, wire) = running().await;
    rig.host.fail("presence_expire", ApiError::internal("x"));
    rig.host.fail("presence_received", ApiError::internal("x"));
    wire.send(RealtimeEvent::Presence(PresenceFrame {
        feed: shared_a(),
        envelope: vec![1],
    }))
    .await;
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert_eq!(rig.count("h:presence_expire"), 2);
    // Still serving: a `changed` frame pulls.
    wire.send(RealtimeEvent::Changed {
        feed: shared_a(),
        cursor: 30,
    })
    .await;
    until("the feed was pulled", || rig.count("r:changes#2") == 2).await;
}

// ----- the push debounce -----------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_push_debounce_follows_the_hosts_hint_while_there_is_a_socket() {
    let (rig, _wire) = running().await;
    // 150 ms with a remote presence, 500 ms without, and never more than the plain debounce.
    for (hint, waited) in [(150u64, 150u64), (500, 500), (5_000, 500), (0, 0)] {
        rig.host.with(|data| data.debounce_ms = hint);
        rig.remote.with(|data| data.times.clear());
        let cycles = rig.host.cycles();
        let at = Instant::now();
        rig.host.emit(&Event::SyncRequested);
        rig.until_cycles(cycles + 1).await;
        let started = rig.remote.times("server_info");
        assert_eq!(started.len(), 1);
        assert_eq!(
            started[0] - at,
            Duration::from_millis(waited),
            "hint {hint} ms"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_burst_under_the_short_debounce_is_one_cycle_that_waits_the_hint_after_the_last_request()
{
    let (rig, _wire) = running().await;
    rig.host.with(|data| data.debounce_ms = 150);
    rig.remote.with(|data| data.times.clear());
    let mut last = Instant::now();
    for _ in 0..4 {
        rig.host.emit(&Event::SyncRequested);
        last = Instant::now();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    rig.until_cycles(2).await;
    let started = rig.remote.times("server_info");
    assert_eq!(started.len(), 1, "one cycle for the whole burst");
    assert_eq!(started[0] - last, Duration::from_millis(150));
}

#[tokio::test(start_paused = true)]
async fn a_host_that_cannot_tell_the_debounce_gets_the_plain_one() {
    let (rig, _wire) = running().await;
    rig.host
        .fail("sync_push_debounce_ms", ApiError::internal("locked up"));
    rig.host.with(|data| data.debounce_ms = 150);
    rig.remote.with(|data| data.times.clear());
    let at = Instant::now();
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(
        rig.remote.times("server_info")[0] - at,
        Duration::from_millis(PUSH_DEBOUNCE_MS)
    );
}

// ----- the shell's handle ----------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_handle_sends_presence_through_the_socket_while_there_is_one() {
    let (mut rig, mut wire) = rig_with_socket(vec![personal_work(), shared_work(shared_a())]);
    rig.start();
    let handle = rig._handle.as_ref().unwrap().realtime();
    assert!(
        !handle.send_presence(shared_a(), vec![1; 10]),
        "no socket yet"
    );
    assert!(!handle.is_attached());
    rig.until_cycles(1).await;
    assert!(handle.is_attached());
    assert!(handle.send_presence(shared_a(), vec![1; 10]));
    let sent = wire.commands();
    assert_eq!(sent.len(), 2, "the subscription, then the frame");
    assert_eq!(
        sent[1],
        RealtimeCommand::Presence(PresenceFrame {
            feed: shared_a(),
            envelope: vec![1; 10],
        })
    );
    // The server refuses a frame over its limit: it is not even sent.
    assert!(!handle.send_presence(shared_a(), vec![0; PRESENCE_MAX_FRAME_BYTES]));
    assert!(handle.send_presence(shared_a(), vec![0; PRESENCE_MAX_FRAME_BYTES - 17]));
    assert_eq!(wire.commands().len(), 1);
    // A socket that is not read (or a UI that sends too fast) fills the queue: the excess is dropped.
    let accepted = (0..COMMAND_QUEUE + 10)
        .filter(|_| handle.send_presence(shared_a(), vec![2; 10]))
        .count();
    assert_eq!(accepted, COMMAND_QUEUE);
}

#[tokio::test(start_paused = true)]
async fn the_handle_stops_sending_when_the_driver_stops_and_closes_the_socket() {
    let (mut rig, mut wire) = rig_with_socket(vec![personal_work(), shared_work(shared_a())]);
    rig.one_cycle().await;
    let handle = rig._handle.as_ref().unwrap().realtime();
    let other = handle.clone();
    assert!(handle.send_presence(shared_a(), vec![1; 10]));
    drop(rig._handle.take());
    assert!(!handle.is_attached());
    assert!(!other.send_presence(shared_a(), vec![1; 10]));
    tokio::time::sleep(Duration::from_secs(1)).await;
    // Nothing keeps the socket's commands open (the remote's task would end), and nobody reads its events.
    let _ = wire.commands();
    assert!(matches!(
        wire.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    assert!(wire.events.is_closed());
}

#[tokio::test(start_paused = true)]
async fn a_new_socket_takes_over_the_handle_and_an_old_one_leaves_it_alone() {
    let (mut rig, wire) = rig_with_socket(vec![personal_work()]);
    let (second, mut second_wire) = Wire::new();
    rig.remote.with(|data| data.sockets.push_back(second));
    rig.one_cycle().await;
    let handle = rig._handle.as_ref().unwrap().realtime();
    assert!(handle.is_attached());
    drop(wire);
    rig.until_cycles(2).await;
    assert!(handle.is_attached(), "the new socket is attached");
    let _ = second_wire.commands();
    assert!(handle.send_presence(personal(), vec![3; 10]));
    assert_eq!(second_wire.commands().len(), 1);
}
