//! Log hygiene of the `/v2` surface (`milestone-13.md` 1.3, "Logging"): the log holds paths only, ids
//! cut to their first 8 hex characters (feeds, accounts, devices, invites, documents, chunk hashes),
//! no query strings, no bodies, no tokens, and membership changes as counts. The test captures what
//! the server logs while a whole sharing flow runs and greps it for every full id and every body it
//! used. It is its own test binary because it installs the global tracing subscriber.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};

use kb_sync::v2::conformance::{DAY_MS, Person, Space};
use kb_types::testing::FixedClock;
use kb_types::{DocId, FeedId, MemberRole};
use reqwest::StatusCode;
use serde_json::{Value, json};
use support::{T0, TestServer, base64, config, hex, unhex};
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

#[tokio::test(flavor = "multi_thread")]
async fn the_log_of_a_sharing_flow_holds_no_full_ids_bodies_or_tokens() {
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
    let clock = Arc::new(FixedClock::new(T0));
    let server = TestServer::start_with_clock(&config(dir.path()), clock.clone()).await;
    let url = |path: &str| server.url(path);
    let people: Vec<Person> = (0..4u64)
        .map(|i| Person::generate(9_000 + i, &format!("Hygiene {i}"), T0))
        .collect();
    let [alice, bob, carol, newcomer] = <[Person; 4]>::try_from(people).unwrap();
    let mut secrets: Vec<String> = Vec::new();
    for person in [&alice, &bob, &carol] {
        let log: Vec<String> = person.encoded_log().iter().map(|e| base64(e)).collect();
        let status = server
            .http
            .post(url("/v2/accounts"))
            .json(&json!({ "inviteToken": "dev", "log": log }))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::CREATED);
    }
    // Sessions.
    let login = |person: &Person| {
        let person = person.clone();
        let http = server.http.clone();
        let challenge_url = url("/v2/auth/challenge");
        let session_url = url("/v2/auth/session");
        async move {
            let dev = person.active_device();
            let challenge: Value = http
                .post(challenge_url)
                .json(&json!({ "accountId": person.account, "deviceId": person.device_id(dev) }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let nonce_hex = challenge["nonce"].as_str().unwrap().to_owned();
            let nonce: [u8; 32] = unhex(&nonce_hex);
            let epoch: [u8; 16] = unhex(challenge["serverEpoch"].as_str().unwrap());
            let signature = person.signer(dev)(&kb_sync::auth_message(&nonce, &epoch));
            let session: Value = http
                .post(session_url)
                .json(&json!({
                    "accountId": person.account,
                    "deviceId": person.device_id(dev),
                    "nonce": nonce_hex,
                    "signature": hex(&signature.to_bytes()),
                }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            session["token"].as_str().unwrap().to_owned()
        }
    };
    let alice_token = login(&alice).await;
    let bob_token = login(&bob).await;
    secrets.push(alice_token.clone());
    secrets.push(bob_token.clone());

    // A feed, an invite, a join, content, a snapshot, a head, a chunk, a signup.
    let mut space = Space::new(77, "Secret space name", T0);
    let create = space.create(&alice, 1);
    let post_bytes = |path: String, token: String, bytes: Vec<u8>| {
        let request = server
            .http
            .post(url(&path))
            .bearer_auth(token)
            .header("content-type", "application/octet-stream")
            .body(bytes);
        async move { request.send().await.unwrap().status() }
    };
    assert_eq!(
        post_bytes("/v2/feeds".to_owned(), alice_token.clone(), create.encode()).await,
        StatusCode::CREATED
    );
    space.accept(&create);
    let feed = space.feed;
    let membership = |token: &str, entries: Vec<Vec<u8>>| {
        let entries: Vec<String> = entries.iter().map(|e| base64(e)).collect();
        let request = server
            .http
            .post(url(&format!("/v2/feeds/{feed}/membership")))
            .bearer_auth(token)
            .json(&json!({ "entries": entries }));
        async move { request.send().await.unwrap().status() }
    };
    let (invite, secret) = space.invite(&alice, MemberRole::Editor, T0 + 7 * DAY_MS, None);
    assert_eq!(
        membership(&alice_token, vec![invite.encode()]).await,
        StatusCode::OK
    );
    space.accept(&invite);
    let join = space.join(&bob, &secret);
    assert_eq!(
        membership(&bob_token, vec![join.encode()]).await,
        StatusCode::OK
    );
    space.accept(&join);
    let doc = DocId::from_bytes([0x5d; 16]);
    let entry = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"private page text");
    assert_eq!(
        post_bytes(
            format!("/v2/feeds/{feed}/entries"),
            alice_token.clone(),
            entry.clone()
        )
        .await,
        StatusCode::OK
    );
    let (chunk, chunk_bytes) = space.random_chunk(80);
    let chunk_path = format!("/v2/feeds/{feed}/chunks/{}", chunk.to_hex());
    let put = server
        .http
        .put(url(&chunk_path))
        .bearer_auth(&alice_token)
        .body(chunk_bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);
    let snapshot = space.doc_snapshot(&alice, 0, doc, 2, space.epoch, b"private snapshot text");
    let put = server
        .http
        .put(url(&format!("/v2/feeds/{feed}/snapshots/{doc}")))
        .bearer_auth(&alice_token)
        .body(snapshot.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);
    for path in [
        format!("/v2/feeds/{feed}/changes?after=0&limit=10"),
        format!("/v2/feeds/{feed}/membership?after=0"),
        format!("/v2/feeds/{feed}/snapshot-marks?after=0"),
        "/v2/feeds".to_owned(),
    ] {
        let response = server
            .http
            .get(url(&path))
            .bearer_auth(&bob_token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    // A stranger, a dead invite, a share signup, the log endpoints.
    let epoch: [u8; 16] = server.server.server_epoch();
    let sig = Space::open_sig(&secret, epoch);
    let dead = server
        .http
        .post(url(&format!(
            "/v2/invites/{}/open",
            kb_keys::invite::invite_id(&secret)
        )))
        .json(&json!({ "signature": hex(&sig.to_bytes()) }))
        .send()
        .await
        .unwrap();
    assert_eq!(dead.status(), StatusCode::NOT_FOUND);
    let (invite, signup_secret) = space.invite(&alice, MemberRole::Viewer, T0 + DAY_MS, None);
    assert_eq!(
        membership(&alice_token, vec![invite.encode()]).await,
        StatusCode::OK
    );
    space.accept(&invite);
    let signup_sig = Space::signup_sig(&signup_secret, newcomer.account, epoch);
    let log: Vec<String> = newcomer.encoded_log().iter().map(|e| base64(e)).collect();
    let signup = server
        .http
        .post(url("/v2/accounts"))
        .json(&json!({
            "share": {
                "inviteId": kb_keys::invite::invite_id(&signup_secret),
                "signature": hex(&signup_sig.to_bytes()),
            },
            "log": log,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(signup.status(), StatusCode::CREATED);
    let carol_token = login(&carol).await;
    let stranger = server
        .http
        .get(url(&format!("/v2/feeds/{feed}/changes?after=0")))
        .bearer_auth(&carol_token)
        .send()
        .await
        .unwrap();
    assert_eq!(stranger.status(), StatusCode::NOT_FOUND);
    for account in [alice.account, newcomer.account] {
        let status = server
            .http
            .get(url(&format!("/v2/accounts/{account}/log?after=0")))
            .send()
            .await
            .unwrap()
            .status();
        assert!(status == StatusCode::OK || status == StatusCode::NOT_FOUND);
    }
    server.stop().await;

    let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(log.lines().count() > 20, "the server logged the requests");
    assert!(
        log.contains("/v2/feeds/"),
        "paths are logged:\n{}",
        &log[..log.len().min(2000)]
    );
    assert!(log.contains("membership entries appended"));
    assert!(log.contains("entries=1"), "membership changes are counts");
    // No long hex or UUID run, whatever it names.
    let long_ids = runs(&log, 32, |c| c.is_ascii_hexdigit() || c == '-');
    assert!(
        long_ids.is_empty(),
        "a long id or hash reached the log: {long_ids:?}"
    );
    // No base64 body.
    let bodies = runs(&log, 48, |c| {
        c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='
    });
    assert!(
        bodies.is_empty(),
        "something that looks like a body reached the log: {bodies:?}"
    );
    // Not one of the full ids, bodies or tokens the flow used.
    let mut forbidden: Vec<String> = vec![
        alice.account.to_string(),
        bob.account.to_string(),
        carol.account.to_string(),
        newcomer.account.to_string(),
        alice.device_id(0).to_string(),
        bob.device_id(0).to_string(),
        feed.to_string(),
        FeedId::from_account(alice.account).to_string(),
        kb_keys::invite::invite_id(&secret).to_string(),
        kb_keys::invite::invite_id(&signup_secret).to_string(),
        doc.to_string(),
        chunk.to_hex(),
        "Secret space name".to_owned(),
        "private page text".to_owned(),
        "private snapshot text".to_owned(),
        "Hygiene".to_owned(),
        base64(&entry),
        base64(&snapshot),
        base64(&chunk_bytes),
    ];
    forbidden.extend(secrets);
    for needle in &forbidden {
        assert!(!log.contains(needle.as_str()), "the log holds {needle}");
    }
    // The short forms are what is there.
    assert!(
        log.contains(&feed.short_hex()),
        "the feed's 8 hex characters are logged"
    );
    assert!(!log.contains('?'), "no query string reaches the log");
}
