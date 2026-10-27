//! The realtime socket of `kb-server` (M13 slice 3, ADR-0050): the `check_ws_*` conformance checks of
//! `kb-sync` over a real WebSocket to the in-process server, and what only a server can show: that
//! presence is never written down, the limits and the abuse handling, that notices leave in the order
//! of the cursors under concurrent appends, and that a connection that fell behind is told where its
//! feeds stand.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::HashMap;
use std::path::Path;

use futures_util::future::join_all;
use kb_net::HttpRemote;
use kb_sync::v2::conformance::{self, Fixture, WsConnection, ok};
use kb_sync::v2::{RemoteStore, ServerFrame, chain_genesis, chain_next};
use kb_types::sharing::PRESENCE_MAX_FRAME_BYTES;
use kb_types::{FeedId, MemberRole};
use support::realtime::ServerWs;
use support::{TestServer, config};

async fn started(seed: u64) -> (tempfile::TempDir, TestServer, Fixture) {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(&config(dir.path())).await;
    (dir, server, Fixture::generate(seed))
}

// ----- the conformance checks over a real WebSocket ---------------------------------------------

macro_rules! conformance_check {
    ($name:ident, $check:path, $seed:expr) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $name() {
            let (_dir, server, fixture) = started($seed).await;
            let remote = HttpRemote::new(&server.base).unwrap();
            $check(&remote, &ServerWs { server: &server }, &fixture).await;
            server.stop().await;
        }
    };
}

conformance_check!(
    the_server_pushes_inline_entries_to_subscribed_members,
    conformance::check_ws_inline_entries,
    81
);
conformance_check!(
    the_server_announces_large_entries_with_changed,
    conformance::check_ws_large_entries_are_announced,
    82
);
conformance_check!(
    the_server_pushes_in_the_order_of_the_feed,
    conformance::check_ws_frames_follow_the_feeds_order,
    83
);
conformance_check!(
    the_server_stops_a_removed_members_socket_at_its_remove,
    conformance::check_ws_removed_member_stops_at_its_remove,
    84
);
conformance_check!(
    the_server_relays_presence_to_the_other_subscribed_members,
    conformance::check_ws_presence_relay,
    85
);
conformance_check!(
    the_server_drops_refused_presence_frames_and_keeps_the_socket,
    conformance::check_ws_presence_refusals,
    86
);
conformance_check!(
    the_server_drops_presence_over_the_rate_limit,
    conformance::check_ws_presence_rate_limit,
    87
);

// ----- what only a server can show ----------------------------------------------------------------

/// Two people in a space, each with a socket subscribed to it.
struct Duo {
    alice: conformance::Person,
    bob: conformance::Person,
    space: conformance::Space,
}

async fn duo(remote: &HttpRemote, fixture: &Fixture, name: &str) -> Duo {
    let [alice, bob] = conformance::enroll_n::<HttpRemote, 2>(remote, fixture).await;
    let mut space = fixture.space(name);
    conformance::found(remote, &mut space, &alice).await;
    conformance::invite_and_join(
        remote,
        fixture,
        &mut space,
        &alice,
        &bob,
        MemberRole::Editor,
    )
    .await;
    Duo { alice, bob, space }
}

/// Whether `needle` occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Every byte the server keeps in its data directory (the database, its journal and write-ahead log).
fn stored_bytes(dir: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            all.extend(std::fs::read(path).unwrap_or_default());
        }
    }
    all
}

fn feed_entry_count(dir: &Path) -> i64 {
    rusqlite::Connection::open(dir.join("server.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM feed_entries", [], |row| row.get(0))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn presence_is_relayed_and_never_written_down() {
    let (dir, server, fixture) = started(90).await;
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let Duo { alice, bob, space } = duo(&remote, &fixture, "Not stored").await;
    let feed = space.feed;
    let alice_ws = ws.connect(&alice).await.unwrap();
    alice_ws.subscribe_and_wait(&[feed]).await.unwrap();
    let bob_ws = ws.connect(&bob).await.unwrap();
    bob_ws.subscribe_and_wait(&[feed]).await.unwrap();
    let entries_before = feed_entry_count(dir.path());

    let mut frames = Vec::new();
    for counter in 1..=6u64 {
        let frame = space.presence_frame(&alice, 0, space.epoch, counter, &[counter as u8; 900]);
        alice_ws.send_presence(&frame).await.unwrap();
        assert_eq!(
            bob_ws.recv(5_000).await,
            Some(ServerFrame::Presence(frame.clone())),
            "the frame is relayed byte for byte"
        );
        frames.push(frame);
    }
    assert_eq!(
        feed_entry_count(dir.path()),
        entries_before,
        "a presence frame is no feed item"
    );
    // Nothing of the sealed content, not even its nonce, is in anything the server keeps.
    let kept = stored_bytes(dir.path());
    for frame in &frames {
        let header = kb_sync::v2::ws::PRESENCE_HEADER_LEN;
        let body_at = 3 + 4 + header;
        for slice in [
            &frame.envelope[body_at..body_at + 24],
            &frame.envelope[body_at + 60..body_at + 92],
            &frame.envelope[frame.envelope.len() - 64..],
        ] {
            assert!(
                !contains(&kept, slice),
                "a presence frame's bytes are in the server's files"
            );
        }
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frame_over_4_kib_is_ignored_and_a_message_over_the_client_limit_ends_the_connection() {
    let (_dir, server, fixture) = started(91).await;
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let Duo { alice, bob, space } = duo(&remote, &fixture, "Sizes").await;
    let feed = space.feed;
    let alice_ws = ws.connect(&alice).await.unwrap();
    alice_ws.subscribe_and_wait(&[feed]).await.unwrap();
    let bob_ws = ws.connect(&bob).await.unwrap();
    bob_ws.subscribe_and_wait(&[feed]).await.unwrap();

    let frame_of = |len: usize| {
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(feed.as_bytes());
        bytes.resize(len, 0);
        bytes
    };
    // One byte over 4 KiB, and 8 KiB: ignored, the connection stays.
    bob_ws
        .send_binary(&frame_of(PRESENCE_MAX_FRAME_BYTES + 1))
        .await
        .unwrap();
    bob_ws.send_binary(&frame_of(8 * 1024)).await.unwrap();
    let good = space.presence_frame(&bob, 0, space.epoch, 1, b"still here");
    bob_ws.send_presence(&good).await.unwrap();
    assert_eq!(
        alice_ws.recv(5_000).await,
        Some(ServerFrame::Presence(good)),
        "only the valid frame was relayed"
    );
    // A message over the limit the server reads from a client ends that connection, and only that one.
    bob_ws.send_binary(&frame_of(20 * 1024)).await.unwrap();
    assert!(
        bob_ws.closed_within(5_000).await,
        "a message over the client limit closes the connection"
    );
    conformance::login(&remote, &alice).await;
    let entry = space.doc_entry(&alice, 0, fixture.doc(), 1, space.epoch, b"unaffected");
    let (cursor, _) = ok(remote.append(feed, &entry).await, "append");
    assert!(matches!(
        alice_ws.recv(5_000).await,
        Some(ServerFrame::Entry(frame)) if frame.cursor == cursor
    ));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_sends_a_thousand_refused_frames_is_closed() {
    let (_dir, server, fixture) = started(92).await;
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let Duo { alice, bob, space } = duo(&remote, &fixture, "Abuse").await;
    let alice_ws = ws.connect(&alice).await.unwrap();
    alice_ws.subscribe_and_wait(&[space.feed]).await.unwrap();
    let bob_ws = ws.connect(&bob).await.unwrap();
    bob_ws.subscribe_and_wait(&[space.feed]).await.unwrap();
    for _ in 0..1_100 {
        if bob_ws.send_binary(&[9u8; 3]).await.is_err() {
            break;
        }
    }
    assert!(
        bob_ws.closed_within(10_000).await,
        "a thousand refused frames in a minute end the connection"
    );
    // Alice's connection never noticed.
    let from_alice = space.presence_frame(&alice, 0, space.epoch, 1, b"fine");
    alice_ws.send_presence(&from_alice).await.unwrap();
    assert_eq!(alice_ws.recv(300).await, None);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_appends_reach_every_socket_in_the_order_of_the_cursors() {
    let (_dir, server, fixture) = started(93).await;
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let [alice, bob, carol] = conformance::enroll_n::<HttpRemote, 3>(&remote, &fixture).await;
    let mut space = fixture.space("Race");
    conformance::found(&remote, &mut space, &alice).await;
    for joiner in [&bob, &carol] {
        conformance::invite_and_join(
            &remote,
            &fixture,
            &mut space,
            &alice,
            joiner,
            MemberRole::Editor,
        )
        .await;
    }
    let feed = space.feed;
    let people = [&alice, &bob, &carol];
    let mut sockets = Vec::new();
    for person in people {
        let socket = ws.connect(person).await.unwrap();
        socket.subscribe_and_wait(&[feed]).await.unwrap();
        sockets.push(socket);
    }
    // One handle per person, signed in once, so the appends can run at the same time.
    let mut remotes = Vec::new();
    for person in people {
        let handle = HttpRemote::new(&server.base).unwrap();
        conformance::login(&handle, person).await;
        remotes.push(handle);
    }
    conformance::login(&remote, &alice).await;
    let (tip, _) = conformance::feed_tip(&remote, feed).await;
    let doc = fixture.doc();
    let per_person = 4u64;
    let mut work = Vec::new();
    for (index, person) in people.iter().enumerate() {
        for seq in 1..=per_person {
            let bytes = space.doc_entry(person, 0, doc, seq, space.epoch, &[index as u8; 40]);
            work.push((index, bytes));
        }
    }
    let answers = join_all(
        work.iter()
            .map(|(index, bytes)| remotes[*index].append(feed, bytes)),
    )
    .await;
    let mut cursors: Vec<u64> = answers.into_iter().map(|a| ok(a, "append").0).collect();
    cursors.sort_unstable();
    let total = work.len();
    // The pull's view: items in cursor order and the chain after each.
    let items = conformance::read_feed(&remote, feed).await;
    let mut chain = chain_genesis(&server.epoch().await);
    let mut chain_at = HashMap::new();
    for (cursor, bytes) in &items {
        chain = chain_next(&chain, bytes);
        chain_at.insert(*cursor, chain);
    }
    for (who, socket) in ["alice", "bob", "carol"].iter().zip(&sockets) {
        let mut frames = Vec::new();
        for _ in 0..total {
            match socket.recv(5_000).await {
                Some(ServerFrame::Entry(entry)) => frames.push(entry),
                other => panic!("{who}: expected an entry frame, got {other:?}"),
            }
        }
        let got: Vec<u64> = frames.iter().map(|f| f.cursor).collect();
        assert_eq!(
            got, cursors,
            "{who} gets every entry in the order of the cursors"
        );
        let mut previous = tip;
        for frame in &frames {
            assert_eq!(
                frame.prev_cursor, previous,
                "{who}: the chain of cursors is contiguous"
            );
            assert_eq!(
                frame.chain_after, chain_at[&frame.cursor],
                "{who}: the chain hash is the running hash"
            );
            previous = frame.cursor;
        }
        assert_eq!(socket.recv(200).await, None);
    }
    server.stop().await;
}

/// The runtime is single-threaded so the flood below cannot be read while it is made: the connection
/// task only runs once the test awaits, deterministically after its receiver lagged.
#[tokio::test(flavor = "current_thread")]
async fn a_connection_that_fell_behind_is_told_where_its_feeds_stand() {
    let (_dir, server, fixture) = started(94).await;
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let Duo { alice, bob, space } = duo(&remote, &fixture, "Lag").await;
    let feed = space.feed;
    let home = FeedId::from_account(bob.account);
    let bob_ws = ws.connect(&bob).await.unwrap();
    bob_ws.subscribe_and_wait(&[feed, home]).await.unwrap();
    // Something to be told about: an entry the socket gets inline first.
    conformance::login(&remote, &alice).await;
    let entry = space.doc_entry(
        &alice,
        0,
        fixture.doc(),
        1,
        space.epoch,
        b"before the flood",
    );
    let (cursor, _) = ok(remote.append(feed, &entry).await, "append");
    assert!(matches!(
        bob_ws.recv(5_000).await,
        Some(ServerFrame::Entry(frame)) if frame.cursor == cursor
    ));
    // More notices than the channel keeps, published without the connection task ever running.
    server.server.realtime_flood(5_000);
    let told: HashMap<FeedId, u64> = bob_ws
        .drain(1_000)
        .await
        .into_iter()
        .map(|frame| match frame {
            ServerFrame::Changed { feed, cursor } => (feed, cursor),
            other => panic!("a changed frame per feed, got {other:?}"),
        })
        .collect();
    assert_eq!(
        told,
        HashMap::from([(feed, cursor), (home, 0)]),
        "the newest cursor of every subscribed feed"
    );
    // Back to normal afterwards: the next entry is inline again.
    let next = space.doc_entry(&alice, 0, fixture.doc(), 2, space.epoch, b"after the flood");
    let (next_cursor, _) = ok(remote.append(feed, &next).await, "append");
    assert!(matches!(
        bob_ws.recv(5_000).await,
        Some(ServerFrame::Entry(frame)) if frame.cursor == next_cursor && frame.prev_cursor == cursor
    ));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscribe_that_is_not_valid_changes_nothing_and_the_connection_stays() {
    let (_dir, server, fixture) = started(95).await;
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let Duo { alice, bob, space } = duo(&remote, &fixture, "Strict").await;
    let feed = space.feed;
    let bob_ws = ws.connect(&bob).await.unwrap();
    bob_ws.subscribe_and_wait(&[feed]).await.unwrap();
    // Invalid frames: not JSON, the wrong type, a malformed feed, and 101 feeds.
    let too_many: Vec<String> = (0..101)
        .map(|n| format!("\"{}\"", FeedId::from_bytes([n as u8 + 1; 16])))
        .collect();
    for text in [
        "not json".to_owned(),
        r#"{"type":"subscribe","feeds":["nope"]}"#.to_owned(),
        r#"{"type":"unsubscribe"}"#.to_owned(),
        format!(r#"{{"type":"subscribe","feeds":[{}]}}"#, too_many.join(",")),
    ] {
        bob_ws.send_text(&text).await.unwrap();
    }
    // Bob is subscribed as he was: the next entry reaches him.
    conformance::login(&remote, &alice).await;
    let entry = space.doc_entry(
        &alice,
        0,
        fixture.doc(),
        1,
        space.epoch,
        b"still subscribed",
    );
    let (cursor, _) = ok(remote.append(feed, &entry).await, "append");
    assert!(matches!(
        bob_ws.recv(5_000).await,
        Some(ServerFrame::Entry(frame)) if frame.cursor == cursor
    ));
    server.stop().await;
}

conformance_check!(
    the_server_closes_a_revoked_devices_socket_and_refuses_its_return,
    conformance::check_ws_revoked_device_is_closed_and_cannot_reconnect,
    88
);

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_devices_sessions_are_gone_and_its_old_token_is_refused_everywhere() {
    let (dir, server, fixture) = started(97).await;
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let [mut alice, bob] = conformance::enroll_n::<HttpRemote, 2>(&remote, &fixture).await;
    let mut space = fixture.space("Revocation");
    conformance::found(&remote, &mut space, &alice).await;
    conformance::invite_and_join(
        &remote,
        &fixture,
        &mut space,
        &alice,
        &bob,
        MemberRole::Editor,
    )
    .await;
    let feed = space.feed;
    let second = alice.add_device(fixture.now());
    conformance::login(&remote, &alice).await;
    ok(
        remote
            .append_log(alice.account, &alice.encoded_log_after(0))
            .await,
        "publish the second device",
    );
    let first_device = alice.device_id(0);
    let old_token = server.login(&alice, 0).await;
    let first_ws = ws.connect_with(&old_token).await.unwrap();
    first_ws.subscribe_and_wait(&[feed]).await.unwrap();
    let second_ws = ws.connect_device(&alice, second).await.unwrap();
    second_ws.subscribe_and_wait(&[feed]).await.unwrap();
    let sessions_of = |device: kb_types::DeviceId| -> i64 {
        rusqlite::Connection::open(dir.path().join("server.db"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE device_id = ?1",
                [device.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert!(sessions_of(first_device) >= 1);

    // The second device revokes the first.
    alice.revoke_device(0, fixture.now() + 1);
    ok(
        remote
            .authenticate(
                alice.account,
                alice.device_id(second),
                &alice.signer(second),
            )
            .await,
        "the second device signs in",
    );
    ok(
        remote
            .append_log(alice.account, &alice.encoded_log_after(1))
            .await,
        "publish the revocation",
    );
    assert!(
        first_ws.closed_within(5_000).await,
        "the open socket is closed"
    );
    assert_eq!(sessions_of(first_device), 0, "its sessions are deleted");
    // The old token is refused at the handshake and on the HTTP routes; a new sign-in is refused.
    assert_eq!(
        ws.connect_with(&old_token).await.err(),
        Some(kb_sync::SyncError::Unauthorized)
    );
    let status = server
        .http
        .get(server.url("/v2/feeds"))
        .bearer_auth(&old_token)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    let (status, _) = server.challenge(alice.account, first_device).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    // The account's other device keeps its socket and its session.
    assert!(sessions_of(alice.device_id(second)) >= 1);
    let bob_ws = ws.connect(&bob).await.unwrap();
    bob_ws.subscribe_and_wait(&[feed]).await.unwrap();
    let frame = space.presence_frame(&bob, 0, space.epoch, 1, b"after");
    bob_ws.send_presence(&frame).await.unwrap();
    assert_eq!(
        second_ws.recv(5_000).await,
        Some(ServerFrame::Presence(frame))
    );
    server.stop().await;
}
