//! Log hygiene of the realtime socket (`milestone-13.md` 3.1, 3.5: "the server log has no presence
//! content and no page ids"): while entries are pushed and presence frames are relayed, refused and
//! rate-limited over real WebSockets, the log holds paths, the 8-character prefixes of the request's
//! account and device, and one line of counts per closed connection; no feed, account or device id in
//! full, no frame, no token. Its own test binary because it installs the global tracing subscriber.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};

use kb_net::HttpRemote;
use kb_sync::v2::RemoteStore;
use kb_sync::v2::conformance::{self, Fixture, WsConnection};
use kb_types::{FeedId, MemberRole};
use support::realtime::ServerWs;
use support::{TestServer, config};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Runs of at least `min` characters that satisfy `keep`.
fn runs(text: &str, min: usize, keep: impl Fn(char) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars().chain(std::iter::once('\n')) {
        if keep(c) {
            current.push(c);
        } else {
            if current.len() >= min {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_of_the_realtime_socket_holds_counts_and_no_ids_or_frames() {
    let capture = Capture::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .finish(),
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(&config(dir.path())).await;
    let fixture = Fixture::generate(96);
    let remote = HttpRemote::new(&server.base).unwrap();
    let ws = ServerWs { server: &server };
    let [alice, bob] = conformance::enroll_n::<HttpRemote, 2>(&remote, &fixture).await;
    let mut space = fixture.space("Secret presence space");
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
    let alice_ws = ws.connect(&alice).await.unwrap();
    alice_ws.subscribe_and_wait(&[feed]).await.unwrap();
    let bob_ws = ws.connect(&bob).await.unwrap();
    bob_ws.subscribe_and_wait(&[feed]).await.unwrap();

    // Presence, relayed.
    let mut frames = Vec::new();
    for counter in 1..=3u64 {
        let frame = space.presence_frame(&alice, 0, space.epoch, counter, b"PRIVATE-CURSOR-TEXT");
        alice_ws.send_presence(&frame).await.unwrap();
        assert!(bob_ws.recv(5_000).await.is_some());
        frames.push(frame);
    }
    // Presence refused (a spoofed account) and dropped (over the rate).
    let mut spoof = space.presence_header(&bob, 0, space.epoch, 1);
    spoof.account_id = alice.account;
    bob_ws
        .send_presence(&space.presence_frame_with(&spoof, &bob, 0, b"SPOOFED"))
        .await
        .unwrap();
    for counter in 1..=14u64 {
        let frame = space.presence_frame(&bob, 0, space.epoch, counter, b"MORE-PRIVATE-TEXT");
        bob_ws.send_presence(&frame).await.unwrap();
        frames.push(frame);
    }
    // An entry, pushed inline.
    conformance::login(&remote, &alice).await;
    let doc = fixture.doc();
    let entry = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"private page text");
    let (cursor, _) = remote.append(feed, &entry).await.unwrap();
    let mut pushed = None;
    while let Some(frame) = bob_ws.recv(2_000).await {
        if let kb_sync::v2::ServerFrame::Entry(entry) = frame {
            pushed = Some(entry);
            break;
        }
    }
    assert_eq!(pushed.unwrap().cursor, cursor);
    // Both connections end.
    drop(alice_ws);
    drop(bob_ws);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    server.stop().await;

    let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("realtime connection closed"),
        "a closed connection logs its counts:\n{}",
        &log[..log.len().min(2000)]
    );
    assert!(log.contains("presence_relayed="), "counts, not frames");
    assert!(log.contains("/v2/ws"), "the path is logged");
    // No long hex or UUID run, whatever it names, and no base64 body.
    let long_ids = runs(&log, 32, |c| c.is_ascii_hexdigit() || c == '-');
    assert!(
        long_ids.is_empty(),
        "a long id or hash reached the log: {long_ids:?}"
    );
    let bodies = runs(&log, 48, |c| {
        c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='
    });
    assert!(
        bodies.is_empty(),
        "something that looks like a body reached the log: {bodies:?}"
    );
    // Not one of the full ids, plaintexts or frames the flow used.
    let mut forbidden: Vec<String> = vec![
        alice.account.to_string(),
        bob.account.to_string(),
        alice.device_id(0).to_string(),
        bob.device_id(0).to_string(),
        feed.to_string(),
        FeedId::from_account(alice.account).to_string(),
        doc.to_string(),
        "Secret presence space".to_owned(),
        "PRIVATE-CURSOR-TEXT".to_owned(),
        "MORE-PRIVATE-TEXT".to_owned(),
        "SPOOFED".to_owned(),
        "private page text".to_owned(),
    ];
    for frame in &frames {
        forbidden.push(hex(&frame.envelope[..40]));
    }
    for needle in &forbidden {
        assert!(!log.contains(needle.as_str()), "the log holds {needle}");
    }
    assert!(
        log.contains(&feed.short_hex()),
        "the feed's 8 hex characters are what is logged"
    );
    assert!(
        !log.contains('?'),
        "no query string (the token) reaches the log"
    );
}
