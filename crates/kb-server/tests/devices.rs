//! Milestone 7 endpoints (ADR-0022): pairing mailboxes (lifecycle, long-poll, limits),
//! recovery blobs (ownership, size, read rate) and idle-device refusal.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use kb_sync::v2::conformance::Person;
use kb_types::devices::{
    PAIRING_MAX_MESSAGE_BYTES, PAIRING_MAX_MESSAGES, PAIRING_TTL_SECONDS, RECOVERY_BLOB_MAX_BYTES,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use support::{DAY_MS, T0, TestServer, base64, config, hex, unbase64};

async fn server() -> (tempfile::TempDir, TestServer) {
    let dir = tempfile::tempdir().unwrap();
    let server = TestServer::start(&config(dir.path())).await;
    (dir, server)
}

async fn status_of(request: reqwest::RequestBuilder) -> StatusCode {
    request.send().await.unwrap().status()
}

async fn json_of(request: reqwest::RequestBuilder) -> (StatusCode, Value) {
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

impl TestServer {
    async fn open_mailbox(&self, offer: &[u8]) -> (String, i64) {
        let (status, body) = json_of(
            self.http
                .post(self.url("/v2/pair"))
                .json(&json!({ "offer": base64(offer) })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        (
            body["mailboxId"].as_str().unwrap().to_owned(),
            body["expiresAt"].as_i64().unwrap(),
        )
    }

    fn messages(&self, id: &str, after: u32, wait: u32) -> reqwest::RequestBuilder {
        self.http
            .get(self.url(&format!("/v2/pair/{id}/messages?after={after}&wait={wait}")))
    }

    fn post_message(&self, id: &str, role: &str, bytes: &[u8]) -> reqwest::RequestBuilder {
        self.http
            .post(self.url(&format!("/v2/pair/{id}/messages")))
            .json(&json!({ "role": role, "bytes": base64(bytes) }))
    }

    fn close_mailbox(&self, id: &str) -> reqwest::RequestBuilder {
        self.http.delete(self.url(&format!("/v2/pair/{id}")))
    }
}

fn seqs(page: &Value) -> Vec<u64> {
    page["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["seq"].as_u64().unwrap())
        .collect()
}

// ----- pairing mailboxes --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_mailbox_relays_messages_in_order_and_stays_readable_when_closed() {
    let (_dir, server) = server().await;
    let (id, expires_at) = server.open_mailbox(b"offer").await;
    assert_eq!(id.len(), 12);
    assert!(
        id.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(
        expires_at,
        T0 + i64::try_from(PAIRING_TTL_SECONDS).unwrap() * 1000
    );

    let (status, page) = json_of(server.messages(&id, 0, 0)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        page,
        json!({
            "messages": [{ "seq": 0, "role": "joiner", "bytes": base64(b"offer") }],
            "expiresAt": expires_at,
            "closed": false,
        })
    );
    // Without query parameters: from message 0, no wait.
    let (_, page) = json_of(
        server
            .http
            .get(server.url(&format!("/v2/pair/{id}/messages"))),
    )
    .await;
    assert_eq!(seqs(&page), vec![0]);

    assert_eq!(
        status_of(server.post_message(&id, "sponsor", b"answer")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status_of(server.post_message(&id, "joiner", b"identity")).await,
        StatusCode::NO_CONTENT
    );
    let (_, page) = json_of(server.messages(&id, 1, 0)).await;
    assert_eq!(seqs(&page), vec![1, 2]);
    assert_eq!(page["messages"][0]["role"], "sponsor");
    assert_eq!(
        unbase64(page["messages"][1]["bytes"].as_str().unwrap()),
        b"identity"
    );
    let (_, page) = json_of(server.messages(&id, 3, 0)).await;
    assert_eq!(
        (seqs(&page), page["closed"].clone()),
        (vec![], json!(false))
    );

    assert_eq!(
        status_of(server.close_mailbox(&id)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status_of(server.close_mailbox(&id)).await,
        StatusCode::NO_CONTENT
    );
    let (_, page) = json_of(server.messages(&id, 0, 0)).await;
    assert_eq!(
        (seqs(&page), page["closed"].clone()),
        (vec![0, 1, 2], json!(true))
    );
    assert_eq!(
        status_of(server.post_message(&id, "sponsor", b"late")).await,
        StatusCode::CONFLICT
    );

    // Expired: gone for readers and writers, then purged by the maintenance pass.
    server
        .clock
        .advance(i64::try_from(PAIRING_TTL_SECONDS).unwrap() * 1000);
    assert_eq!(
        status_of(server.messages(&id, 0, 0)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(server.close_mailbox(&id)).await,
        StatusCode::NOT_FOUND
    );
    let (live, _) = server.open_mailbox(b"another").await;
    assert_eq!(
        status_of(server.post_message(&id, "sponsor", b"x")).await,
        StatusCode::NOT_FOUND
    );
    server.clock.advance(1000 * 1000);
    assert_eq!(server.server.prune_now().unwrap().mailboxes, 1);
    assert_eq!(
        status_of(server.messages(&live, 0, 0)).await,
        StatusCode::NOT_FOUND
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_poll_returns_on_a_message_on_close_or_when_the_wait_elapses() {
    let (_dir, server) = server().await;
    let (id, _) = server.open_mailbox(b"offer").await;

    // A message posted while the poll waits.
    let started = std::time::Instant::now();
    let poll = tokio::spawn(server.messages(&id, 1, 20).send());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        status_of(server.post_message(&id, "sponsor", b"answer")).await,
        StatusCode::NO_CONTENT
    );
    let page: Value = poll.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!(seqs(&page), vec![1]);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "returned at once"
    );

    // An existing message answers without waiting.
    let started = std::time::Instant::now();
    let (_, page) = json_of(server.messages(&id, 0, 20)).await;
    assert_eq!(seqs(&page), vec![0, 1]);
    assert!(started.elapsed() < Duration::from_secs(5));

    // The wait elapses with nothing new.
    let started = std::time::Instant::now();
    let (status, page) = json_of(server.messages(&id, 2, 1)).await;
    assert_eq!((status, seqs(&page)), (StatusCode::OK, vec![]));
    assert!(started.elapsed() >= Duration::from_millis(900));

    // Closing wakes the poll.
    let started = std::time::Instant::now();
    let poll = tokio::spawn(server.messages(&id, 2, 20).send());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        status_of(server.close_mailbox(&id)).await,
        StatusCode::NO_CONTENT
    );
    let page: Value = poll.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!((seqs(&page), page["closed"].clone()), (vec![], json!(true)));
    assert!(started.elapsed() < Duration::from_secs(10));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mailboxes_refuse_bad_input_and_enforce_their_limits() {
    let (_dir, server) = server().await;
    let (id, _) = server.open_mailbox(b"offer").await;

    let too_big = vec![1u8; PAIRING_MAX_MESSAGE_BYTES + 1];
    assert_eq!(
        status_of(server.post_message(&id, "sponsor", &too_big)).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let create_too_big = server
        .http
        .post(server.url("/v2/pair"))
        .json(&json!({ "offer": base64(&too_big) }));
    assert_eq!(
        status_of(create_too_big).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        status_of(server.post_message(&id, "observer", b"x")).await,
        StatusCode::BAD_REQUEST
    );
    let bad_base64 = server
        .http
        .post(server.url(&format!("/v2/pair/{id}/messages")))
        .json(&json!({ "role": "sponsor", "bytes": "%%%" }));
    assert_eq!(status_of(bad_base64).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        status_of(server.messages("not-hex-id!!", 0, 0)).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_of(server.messages("000000000000", 0, 0)).await,
        StatusCode::NOT_FOUND
    );
    // An unknown mailbox is answered before the message is judged.
    assert_eq!(
        status_of(server.post_message("000000000000", "observer", &too_big)).await,
        StatusCode::NOT_FOUND
    );

    // At most PAIRING_MAX_MESSAGES messages (the offer included); the per-mailbox
    // request limit means the clock has to move along.
    let mut requests = 4; // the four refusals above that named this mailbox
    for seq in 1..PAIRING_MAX_MESSAGES {
        if requests == 60 {
            server.clock.advance(60_000);
            requests = 0;
        }
        let largest = vec![u8::try_from(seq % 256).unwrap(); PAIRING_MAX_MESSAGE_BYTES];
        assert_eq!(
            status_of(server.post_message(&id, "sponsor", &largest)).await,
            StatusCode::NO_CONTENT,
            "message {seq}"
        );
        requests += 1;
    }
    server.clock.advance(60_000);
    assert_eq!(
        status_of(server.post_message(&id, "sponsor", b"one too many")).await,
        StatusCode::CONFLICT
    );

    // 60 requests per minute per mailbox.
    for _ in 1..60 {
        assert_eq!(status_of(server.messages(&id, 64, 0)).await, StatusCode::OK);
    }
    let (status, body) = json_of(server.messages(&id, 64, 0)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate_limited");
    server.clock.advance(60_000);
    assert_eq!(status_of(server.messages(&id, 64, 0)).await, StatusCode::OK);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_opens_at_most_ten_mailboxes_per_ten_minutes() {
    let (_dir, server) = server().await;
    for _ in 0..10 {
        server.open_mailbox(b"offer").await;
    }
    let create = || {
        server
            .http
            .post(server.url("/v2/pair"))
            .json(&json!({ "offer": base64(b"offer") }))
    };
    assert_eq!(status_of(create()).await, StatusCode::TOO_MANY_REQUESTS);
    server.clock.advance(10 * 60 * 1000);
    assert_eq!(status_of(create()).await, StatusCode::CREATED);
    server.stop().await;
}

// ----- recovery blobs -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn recovery_blobs_belong_to_the_account_that_stored_them() {
    let (_dir, server) = server().await;
    let owner = Person::generate(41, "Owner", T0);
    let other = Person::generate(42, "Other", T0);
    for person in [&owner, &other] {
        assert_eq!(server.create_account(person).await, StatusCode::CREATED);
    }
    let owner_token = server.login(&owner, 0).await;
    let other_token = server.login(&other, 0).await;
    let id = hex(&[0xab; 32]);
    let url = server.url(&format!("/v2/recovery/{id}"));
    let put = |token: &str, bytes: Vec<u8>| server.http.put(&url).bearer_auth(token).body(bytes);

    assert_eq!(
        status_of(server.http.get(&url)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(server.http.put(&url).body(vec![1u8; 10])).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(put(&owner_token, vec![1u8; RECOVERY_BLOB_MAX_BYTES + 1])).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        status_of(put(&owner_token, Vec::new())).await,
        StatusCode::BAD_REQUEST
    );
    let malformed = server.http.get(server.url("/v2/recovery/abcd"));
    assert_eq!(status_of(malformed).await, StatusCode::BAD_REQUEST);

    let used = server.server.account_bytes_used(owner.account).unwrap();
    assert_eq!(
        status_of(put(&owner_token, vec![1u8; 100])).await,
        StatusCode::NO_CONTENT
    );
    let blob = vec![2u8; RECOVERY_BLOB_MAX_BYTES];
    assert_eq!(
        status_of(put(&owner_token, blob.clone())).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        server.server.account_bytes_used(owner.account).unwrap(),
        used,
        "recovery blobs are outside the quota"
    );
    // Read without a session.
    let fetched = server.http.get(&url).send().await.unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        fetched.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(fetched.bytes().await.unwrap().to_vec(), blob);

    // Another account can neither replace nor delete it.
    assert_eq!(
        status_of(put(&other_token, vec![3u8; 10])).await,
        StatusCode::FORBIDDEN
    );
    let delete = |token: &str| server.http.delete(&url).bearer_auth(token);
    assert_eq!(status_of(delete(&other_token)).await, StatusCode::FORBIDDEN);
    assert_eq!(
        status_of(server.http.delete(&url)).await,
        StatusCode::UNAUTHORIZED
    );

    // The owner deletes it; deleting again is fine.
    assert_eq!(
        status_of(delete(&owner_token)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status_of(delete(&owner_token)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        server.server.account_bytes_used(owner.account).unwrap(),
        used
    );
    server.clock.advance(60_000);
    assert_eq!(
        status_of(server.http.get(&url)).await,
        StatusCode::NOT_FOUND
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_reads_are_limited_to_five_per_minute_per_address() {
    let (_dir, server) = server().await;
    let url = |byte: u8| server.url(&format!("/v2/recovery/{}", hex(&[byte; 32])));
    for byte in 0..5 {
        assert_eq!(
            status_of(server.http.get(url(byte))).await,
            StatusCode::NOT_FOUND
        );
    }
    let (status, body) = json_of(server.http.get(url(9))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate_limited");
    server.clock.advance(60_000);
    assert_eq!(
        status_of(server.http.get(url(9))).await,
        StatusCode::NOT_FOUND
    );
    server.stop().await;
}

// ----- trusted proxies ----------------------------------------------------------------------

/// Six recovery reads, the first five as `first`, the last as `last` (as named by
/// `X-Forwarded-For`): answers the status of the sixth.
async fn sixth_read_status(server: &TestServer, first: &str, last: &str) -> StatusCode {
    let url = server.url(&format!("/v2/recovery/{}", hex(&[7; 32])));
    for _ in 0..5 {
        let read = server.http.get(&url).header("x-forwarded-for", first);
        assert_eq!(status_of(read).await, StatusCode::NOT_FOUND);
    }
    status_of(server.http.get(&url).header("x-forwarded-for", last)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_a_trusted_proxy_limits_count_the_forwarded_client() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.trusted_proxies = vec!["127.0.0.0/8".parse().unwrap()];
    let server = TestServer::start(&config).await;
    // Another client behind the same proxy has its own allowance.
    assert_eq!(
        sixth_read_status(&server, "203.0.113.1", "203.0.113.2").await,
        StatusCode::NOT_FOUND
    );
    // The first client is limited; a spoofed entry before the proxy's does not help.
    let url = server.url(&format!("/v2/recovery/{}", hex(&[7; 32])));
    let spoofed = server
        .http
        .get(&url)
        .header("x-forwarded-for", "198.51.100.99, 203.0.113.1");
    assert_eq!(status_of(spoofed).await, StatusCode::TOO_MANY_REQUESTS);
    // X-Real-IP when there is no X-Forwarded-For.
    let real = server.http.get(&url).header("x-real-ip", "203.0.113.1");
    assert_eq!(status_of(real).await, StatusCode::TOO_MANY_REQUESTS);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forwarding_headers_from_an_untrusted_peer_are_ignored() {
    let (_dir, server) = server().await;
    // Without trusted proxies every request counts as the TCP peer.
    assert_eq!(
        sixth_read_status(&server, "203.0.113.1", "203.0.113.2").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    server.stop().await;

    // A trusted list that does not include this peer changes nothing either.
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.trusted_proxies = vec!["10.0.0.0/8".parse().unwrap()];
    let server = TestServer::start(&config).await;
    assert_eq!(
        sixth_read_status(&server, "203.0.113.1", "203.0.113.2").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    server.stop().await;
}

// ----- idle devices -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn idle_devices_are_refused_at_challenge_and_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.idle_device_days = Some(30);
    let server = TestServer::start(&config).await;
    let info: Value = server
        .http
        .get(server.url("/v2/server"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["idleDeviceDays"], 30);

    let mut fixture = Person::generate(43, "Idle", T0);
    fixture.add_device(T0);
    assert_eq!(server.create_account(&fixture).await, StatusCode::CREATED);
    let (account, device) = (fixture.account, fixture.device_id(0));
    server.login(&fixture, 0).await;

    // Exactly at the limit is still fine; a challenge taken now is answered later.
    server.clock.advance(30 * DAY_MS);
    let (status, challenge) = server.challenge(account, device).await;
    assert_eq!(status, StatusCode::OK);
    server.clock.advance(1);
    let (status, body) = server.challenge(account, device).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "idle");
    let nonce = challenge["nonce"].as_str().unwrap();
    let signature = fixture.signer(0)(&kb_sync::auth_message(
        &support::unhex(nonce),
        &support::unhex(challenge["serverEpoch"].as_str().unwrap()),
    ));
    let (status, body) = server.session(account, device, nonce, &signature).await;
    assert_eq!(
        (status, body["error"].clone()),
        (StatusCode::UNAUTHORIZED, json!("idle"))
    );

    // A device never seen counts as seen now; its session starts its count.
    server.login(&fixture, 1).await;
    server.clock.advance(30 * DAY_MS);
    server.login(&fixture, 1).await;
    server.stop().await;
}
