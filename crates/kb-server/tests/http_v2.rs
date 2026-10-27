//! The `/v2` HTTP surface of `kb-server` beyond the conformance suite: status codes and error codes
//! per route and role, the adversarial cases of the slice (a non-member reading, a Viewer appending,
//! a removed member appending, a forged or replayed `Join`, a dead invite, the signup limit, stale
//! epochs, a truncating snapshot that is disputed and never pruned, Owner-only chunk deletes), the
//! rules surviving a restart, the refusal of a database of protocol v1's days, the `/join` page and
//! the WebSocket; and, on the personal feed, what the retired `/v1` tests covered (the epoch across
//! restarts, accounts and the device log, authentication edge cases, entries, snapshots, chunks,
//! heads, pruning).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;

use kb_keys::invite::{InviteSecret, invite_id};
use kb_sync::v2::conformance::{
    DAY_MS, Person, Space, WsConnection, sealed_entry, sealed_snapshot, signed_head,
};
use kb_types::sharing::SpaceSettings;
use kb_types::testing::{FixedClock, SeededRng};
use kb_types::{AccountId, DocId, FeedId, MemberRole};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{Value, json};
use support::{T0, TestServer, base64, config, hex, unbase64, unhex};

const SEED: u64 = 700;

/// A server, its clock and a counter of people.
struct World {
    _dir: tempfile::TempDir,
    server: TestServer,
    clock: Arc<FixedClock>,
    next_person: std::cell::Cell<u64>,
}

impl World {
    async fn start(adjust: impl FnOnce(&mut kb_server::Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config(dir.path());
        adjust(&mut config);
        let clock = Arc::new(FixedClock::new(T0));
        let server = TestServer::start_with_clock(&config, clock.clone()).await;
        Self {
            _dir: dir,
            server,
            clock,
            next_person: std::cell::Cell::new(0),
        }
    }

    fn person(&self) -> Person {
        let index = self.next_person.get();
        self.next_person.set(index + 1);
        Person::generate(SEED * 1000 + index, &format!("Person {index}"), T0)
    }

    fn space(&self, name: &str) -> Space {
        let index = self.next_person.get();
        self.next_person.set(index + 1);
        Space::new(SEED * 77 + index, name, self.clock_ms())
    }

    fn clock_ms(&self) -> i64 {
        kb_types::Clock::now_ms(self.clock.as_ref())
    }

    fn url(&self, path: &str) -> String {
        self.server.url(path)
    }

    /// Creates the person's account with the operator token; answers the status.
    async fn enroll(&self, person: &Person) -> StatusCode {
        let log: Vec<String> = person.encoded_log().iter().map(|e| base64(e)).collect();
        self.server
            .http
            .post(self.url("/v2/accounts"))
            .json(&json!({ "inviteToken": "dev", "log": log }))
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn enrolled(&self) -> Person {
        let person = self.person();
        assert_eq!(self.enroll(&person).await, StatusCode::CREATED);
        person
    }

    /// Challenge and session on the `/v2` endpoints; the bearer token.
    async fn login(&self, person: &Person) -> String {
        let dev = person.active_device();
        let challenge: Value = self
            .server
            .http
            .post(self.url("/v2/auth/challenge"))
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
        let session: Value = self
            .server
            .http
            .post(self.url("/v2/auth/session"))
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

    /// A request with a bearer token; the status and the JSON body (`Null` when there is none).
    async fn json(&self, request: RequestBuilder, token: Option<&str>) -> (StatusCode, Value) {
        let request = match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        let response = request.send().await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.json(self.server.http.get(self.url(path)), Some(token))
            .await
    }

    async fn delete(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.json(self.server.http.delete(self.url(path)), Some(token))
            .await
    }

    async fn post_bytes(&self, path: &str, token: &str, bytes: &[u8]) -> (StatusCode, Value) {
        self.json(
            self.server
                .http
                .post(self.url(path))
                .header("content-type", "application/octet-stream")
                .body(bytes.to_vec()),
            Some(token),
        )
        .await
    }

    async fn put_bytes(&self, path: &str, token: &str, bytes: &[u8]) -> (StatusCode, Value) {
        self.json(
            self.server
                .http
                .put(self.url(path))
                .header("content-type", "application/octet-stream")
                .body(bytes.to_vec()),
            Some(token),
        )
        .await
    }

    /// Posts membership entries; the status and body.
    async fn membership(
        &self,
        feed: FeedId,
        token: &str,
        entries: &[Vec<u8>],
    ) -> (StatusCode, Value) {
        let entries: Vec<String> = entries.iter().map(|e| base64(e)).collect();
        self.json(
            self.server
                .http
                .post(self.url(&format!("/v2/feeds/{feed}/membership")))
                .json(&json!({ "entries": entries })),
            Some(token),
        )
        .await
    }

    async fn create_feed(&self, token: &str, create: &[u8]) -> (StatusCode, Value) {
        self.post_bytes("/v2/feeds", token, create).await
    }

    /// Founds the space; answers the creator's token.
    async fn found(&self, space: &mut Space, creator: &Person) -> String {
        let token = self.login(creator).await;
        let create = space.create(creator, 1);
        let (status, body) = self.create_feed(&token, &create.encode()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        space.accept(&create);
        token
    }

    /// `owner` invites `joiner` (a link invite, a week) and `joiner` joins; answers the joiner's
    /// token.
    async fn add(
        &self,
        space: &mut Space,
        owner: &Person,
        owner_token: &str,
        joiner: &Person,
        role: MemberRole,
    ) -> (String, InviteSecret) {
        space.now = self.clock_ms();
        let (invite, secret) = space.invite(owner, role, self.clock_ms() + 7 * DAY_MS, None);
        let (status, body) = self
            .membership(space.feed, owner_token, &[invite.encode()])
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        space.accept(&invite);
        let token = self.login(joiner).await;
        let join = space.join(joiner, &secret);
        let (status, body) = self.membership(space.feed, &token, &[join.encode()]).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        space.accept(&join);
        (token, secret)
    }

    async fn changes_status(&self, feed: FeedId, token: &str, after: u64) -> StatusCode {
        self.json(
            self.server
                .http
                .get(self.url(&format!("/v2/feeds/{feed}/changes?after={after}&limit=50"))),
            Some(token),
        )
        .await
        .0
    }

    async fn server_epoch(&self) -> [u8; 16] {
        self.server.epoch_v2().await
    }
}

trait EpochV2 {
    async fn epoch_v2(&self) -> [u8; 16];
}

impl EpochV2 for TestServer {
    async fn epoch_v2(&self) -> [u8; 16] {
        let info: Value = self
            .http
            .get(self.url("/v2/server"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        unhex(info["serverEpoch"].as_str().unwrap())
    }
}

fn error_code(body: &Value) -> &str {
    body["error"].as_str().unwrap_or("")
}

// ----- server info, accounts and logs --------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn server_info_reports_version_2_the_epoch_and_the_limits() {
    let world = World::start(|config| {
        config.account_quota_bytes = 12_345;
        config.share_signups = 7;
        config.share_signups_per_account = 3;
        config.idle_device_days = Some(30);
    })
    .await;
    let info: Value = world
        .server
        .http
        .get(world.url("/v2/server"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        info,
        json!({
            "protocolVersion": 2,
            "serverEpoch": hex(&world.server.server.server_epoch()),
            "maxWireBytes": kb_types::sync::MAX_WIRE_BYTES,
            "accountQuotaBytes": 12_345,
            "idleDeviceDays": 30,
            "shareSignups": 7,
            "shareSignupsPerAccount": 3,
        })
    );
    // Protocol v1 is retired: its routes are gone.
    for path in ["/v1/server", "/v1/accounts", "/v1/entries", "/v1/pair"] {
        let status = world
            .server
            .http
            .get(world.url(path))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn accounts_logs_and_the_batch_endpoint() {
    let world = World::start(|_| {}).await;
    let mut alice = world.person();
    let log: Vec<String> = alice.encoded_log().iter().map(|e| base64(e)).collect();
    let post = |body: Value| {
        world
            .server
            .http
            .post(world.url("/v2/accounts"))
            .json(&body)
    };
    let (status, _) = world.json(post(json!({ "log": log })), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no token and no claim");
    let (status, body) = world
        .json(post(json!({ "inviteToken": "wrong", "log": log })), None)
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    let (status, _) = world
        .json(post(json!({ "inviteToken": "dev", "log": [] })), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = world
        .json(post(json!({ "inviteToken": "dev", "log": log })), None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["accountId"], json!(alice.account));
    let (status, body) = world
        .json(post(json!({ "inviteToken": "dev", "log": log })), None)
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "conflict")
    );

    // A device id is unique across accounts, at creation and when a log grows.
    let twin = Person::generate_with_device_id(SEED + 1, "Twin", T0, Some(alice.device_id(0)));
    let twin_log: Vec<String> = twin.encoded_log().iter().map(|e| base64(e)).collect();
    let (status, body) = world
        .json(post(json!({ "inviteToken": "dev", "log": twin_log })), None)
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "conflict")
    );
    let mut bob = world.enrolled().await;
    let bob_device = bob.add_device(T0);
    assert_eq!(bob_device, 1);
    // A second device of Bob's whose id is Alice's first device: not through the kit's API, so
    // check that a normal addition works and the registry allows it.
    let (status, _) = world
        .json(
            world
                .server
                .http
                .post(world.url(&format!("/v2/accounts/{}/log", bob.account)))
                .json(&json!({ "entries": bob.encoded_log_after(0).iter().map(|e| base64(e)).collect::<Vec<_>>() })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The log, whole and after a sequence number; an unknown account.
    alice.add_device(T0 + 1);
    let (status, _) = world
        .json(
            world
                .server
                .http
                .post(world.url(&format!("/v2/accounts/{}/log", alice.account)))
                .json(&json!({ "entries": alice.encoded_log_after(0).iter().map(|e| base64(e)).collect::<Vec<_>>() })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, whole) = world
        .json(
            world
                .server
                .http
                .get(world.url(&format!("/v2/accounts/{}/log", alice.account))),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(whole["entries"].as_array().unwrap().len(), 2);
    let (_, tail) = world
        .json(
            world
                .server
                .http
                .get(world.url(&format!("/v2/accounts/{}/log?after=0", alice.account))),
            None,
        )
        .await;
    assert_eq!(tail["entries"].as_array().unwrap().len(), 1);
    let unknown = world.person();
    let (status, _) = world
        .json(
            world
                .server
                .http
                .get(world.url(&format!("/v2/accounts/{}/log", unknown.account))),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The batch leaves unknown accounts out and honours `after`.
    let (status, batch) = world
        .json(
            world
                .server
                .http
                .post(world.url("/v2/accounts/logs"))
                .json(&json!({ "accounts": [
                    { "id": alice.account },
                    { "id": unknown.account },
                    { "id": alice.account, "after": 0 },
                    { "id": bob.account, "after": 5 },
                ] })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let logs = batch["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 3);
    assert_eq!(logs[0]["entries"].as_array().unwrap().len(), 2);
    assert_eq!(logs[1]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(logs[2]["entries"].as_array().unwrap().len(), 0);
    let too_many: Vec<Value> = (0..300).map(|_| json!({ "id": alice.account })).collect();
    let (status, _) = world
        .json(
            world
                .server
                .http
                .post(world.url("/v2/accounts/logs"))
                .json(&json!({ "accounts": too_many })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    world.server.stop().await;
}

// ----- the shapes of the feed routes -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn feeds_membership_and_entries_answer_their_documented_shapes() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let mut space = world.space("Shapes");
    let alice_token = world.found(&mut space, &alice).await;
    let feed = space.feed;

    // Without a session every feed route is 401.
    for (method, path) in [
        ("GET", "/v2/feeds".to_owned()),
        ("GET", format!("/v2/feeds/{feed}/changes")),
        ("GET", format!("/v2/feeds/{feed}/membership")),
        ("GET", format!("/v2/feeds/{feed}/heads")),
        ("GET", format!("/v2/feeds/{feed}/snapshots")),
        ("GET", format!("/v2/feeds/{feed}/snapshot-marks")),
    ] {
        let request = world
            .server
            .http
            .request(method.parse().unwrap(), world.url(&path));
        let (status, body) = world.json(request, None).await;
        assert_eq!(
            (status, error_code(&body)),
            (StatusCode::UNAUTHORIZED, "unauthorized"),
            "{path}"
        );
    }

    let (status, listing) = world.get("/v2/feeds", &alice_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listing,
        json!({ "feeds": [
            { "feedId": FeedId::from_account(alice.account), "kind": "personal", "role": null,
              "removed": false, "closed": false },
            { "feedId": feed, "kind": "shared", "role": "owner", "removed": false, "closed": false },
        ] })
    );
    let (status, page) = world
        .get(&format!("/v2/feeds/{feed}/membership"), &alice_token)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["more"], false);
    assert_eq!(page["head"]["seq"], 0);
    assert_eq!(page["head"]["hash"].as_str().unwrap().len(), 64);

    // An entry answers its cursor and chain hash; the page is binary and carries the epoch.
    let doc = kb_types::DocId::from_bytes([7; 16]);
    let entry = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"payload");
    let (status, appended) = world
        .post_bytes(&format!("/v2/feeds/{feed}/entries"), &alice_token, &entry)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(appended["cursor"].as_u64().unwrap() > 0);
    assert_eq!(appended["chainHash"].as_str().unwrap().len(), 64);
    let response = world
        .server
        .http
        .get(world.url(&format!("/v2/feeds/{feed}/changes?after=0&limit=10")))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    let page = kb_sync::ChangesPage::decode(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(page.server_epoch, world.server_epoch().await);
    assert_eq!(page.changes.len(), 2, "the Create and the entry");

    // A snapshot or a chunk a member's feed lacks is 204, not 404.
    let (status, _) = world
        .get(&format!("/v2/feeds/{feed}/snapshots/{doc}"), &alice_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (id, bytes) = space.random_chunk(64);
    let (status, _) = world
        .get(
            &format!("/v2/feeds/{feed}/chunks/{}", id.to_hex()),
            &alice_token,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/chunks/{}", id.to_hex()),
            &alice_token,
            &bytes,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/chunks/{}", id.to_hex()),
            &alice_token,
            b"other",
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "bytes that do not hash to the path"
    );

    // Malformed ids and objects.
    let (status, _) = world
        .get("/v2/feeds/not-a-feed/changes", &alice_token)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &alice_token,
            b"garbage",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &alice_token,
            &space.profile(&alice, "A").encode(),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a membership entry through /entries"
    );
    let (status, body) = world.membership(feed, &alice_token, &[]).await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::BAD_REQUEST, "bad_request")
    );
    let (status, _) = world
        .json(
            world
                .server
                .http
                .post(world.url(&format!("/v2/feeds/{feed}/membership")))
                .json(&json!({ "entries": ["!!!"] })),
            Some(&alice_token),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Bob is nobody here.
    let bob_token = world.login(&bob).await;
    assert_eq!(
        world.changes_status(feed, &bob_token, 0).await,
        StatusCode::NOT_FOUND
    );
    world.server.stop().await;
}

// ----- the error codes -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn roles_and_refusals_carry_the_documented_codes() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let carol = world.enrolled().await;
    let erin = world.enrolled().await;
    let mut space = world.space("Codes");
    let alice_token = world.found(&mut space, &alice).await;
    let (bob_token, _) = world
        .add(&mut space, &alice, &alice_token, &bob, MemberRole::Viewer)
        .await;
    let (carol_token, _) = world
        .add(&mut space, &alice, &alice_token, &carol, MemberRole::Editor)
        .await;
    let erin_token = world.login(&erin).await;
    let feed = space.feed;
    let doc = kb_types::DocId::from_bytes([9; 16]);

    // A Viewer's append: 403 `forbidden`; a stranger's: 404 `not_found`.
    let viewer_entry = space.doc_entry(&bob, 0, doc, 1, space.epoch, b"x");
    let (status, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &bob_token,
            &viewer_entry,
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    let rng = SeededRng::new(5);
    let stranger_entry =
        kb_sync::v2::conformance::sealed_entry(feed, &erin, 0, doc, 1, space.epoch, b"x", &rng);
    let (status, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &erin_token,
            &stranger_entry,
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::NOT_FOUND, "not_found")
    );
    // A bad signature: 403 `authentication`.
    let mut forged = space.doc_entry(&carol, 0, doc, 1, space.epoch, b"x");
    if let Some(last) = forged.last_mut() {
        *last ^= 1;
    }
    let (status, body) = world
        .post_bytes(&format!("/v2/feeds/{feed}/entries"), &carol_token, &forged)
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "authentication")
    );
    // An object of another account or another feed.
    let foreign = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"x");
    let (status, body) = world
        .post_bytes(&format!("/v2/feeds/{feed}/entries"), &carol_token, &foreign)
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    let other_feed = space.random_feed();
    let (status, _) = world
        .post_bytes(
            &format!("/v2/feeds/{other_feed}/entries"),
            &carol_token,
            &space.doc_entry(&carol, 0, doc, 1, space.epoch, b"x"),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the entry names another feed than the path"
    );

    // A conflict: 409 `conflict`.
    let stale = space.profile(&alice, "Alice");
    let (status, _) = world
        .membership(feed, &alice_token, &[stale.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = world
        .membership(feed, &alice_token, &[stale.encode()])
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "conflict")
    );
    space.accept(&stale);

    // A role violation by a member: 403 `forbidden`; a fold violation: 400.
    let by_viewer = space.settings(&bob, SpaceSettings::default());
    let (status, body) = world
        .membership(feed, &bob_token, &[by_viewer.encode()])
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    let owner_invite = space.invite_with(
        &alice,
        &InviteSecret::generate(&SeededRng::new(9)),
        MemberRole::Owner,
        world.clock_ms() + DAY_MS,
        None,
    );
    let (status, body) = world
        .membership(feed, &alice_token, &[owner_invite.encode()])
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::BAD_REQUEST, "bad_request")
    );

    // A Leave: 409 `rekey_needed`; after the Rekey a stale epoch: 409 `stale_epoch`.
    space.now = world.clock_ms();
    let leave = space.leave(&bob);
    let (status, _) = world.membership(feed, &bob_token, &[leave.encode()]).await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&leave);
    let entry = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"x");
    let (status, body) = world
        .post_bytes(&format!("/v2/feeds/{feed}/entries"), &alice_token, &entry)
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "rekey_needed")
    );
    let old_epoch = space.epoch;
    let rekey = space.rekey(&alice, &[&alice, &carol]);
    let (status, _) = world
        .membership(feed, &alice_token, &[rekey.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&rekey);
    let stale_entry = space.doc_entry(&alice, 0, doc, 1, old_epoch, b"x");
    let (status, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &alice_token,
            &stale_entry,
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "stale_epoch")
    );
    // Bob left: reading past his Leave is 403, writing 404.
    let (status, page) = world
        .get(&format!("/v2/feeds/{feed}/membership"), &bob_token)
        .await;
    assert_eq!(status, StatusCode::OK);
    let last = page["entries"].as_array().unwrap().last().unwrap();
    assert_eq!(unbase64(last["bytes"].as_str().unwrap()), leave.encode());
    let leave_cursor = last["cursor"].as_u64().unwrap();
    let (status, body) = world
        .get(
            &format!("/v2/feeds/{feed}/changes?after={leave_cursor}"),
            &bob_token,
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    let (status, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &bob_token,
            &space.doc_entry(&bob, 0, doc, 2, space.epoch, b"x"),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::NOT_FOUND, "not_found")
    );

    // A closed feed: 409 `feed_closed`.
    let close = space.close(&alice);
    let (status, _) = world
        .membership(feed, &alice_token, &[close.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &carol_token,
            &space.doc_entry(&carol, 0, doc, 5, space.epoch, b"x"),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "feed_closed")
    );
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forged_replayed_or_dead_invite_gets_the_same_404() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let carol = world.enrolled().await;
    let erin = world.enrolled().await;
    let mut space = world.space("Dead invites");
    let alice_token = world.found(&mut space, &alice).await;
    let feed = space.feed;
    let epoch = world.server_epoch().await;
    let week = world.clock_ms() + 7 * DAY_MS;
    let open = |secret: &InviteSecret| {
        let sig = Space::open_sig(secret, epoch);
        world
            .server
            .http
            .post(world.url(&format!("/v2/invites/{}/open", invite_id(secret))))
            .json(&json!({ "signature": hex(&sig.to_bytes()) }))
    };

    let (invite, secret) = space.invite(&alice, MemberRole::Editor, week, None);
    let (status, _) = world
        .membership(feed, &alice_token, &[invite.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&invite);
    let (status, opened) = world.json(open(&secret), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opened["feedId"], json!(feed));
    assert_eq!(opened["log"].as_array().unwrap().len(), 2);

    // The dead-invite answer is one body for every kind of dead.
    let (status_unknown, unknown_body) = world
        .json(open(&InviteSecret::generate(&SeededRng::new(11))), None)
        .await;
    assert_eq!(status_unknown, StatusCode::NOT_FOUND);

    // A forged Join (proof signed with another secret) by a non-member: 404.
    let bob_token = world.login(&bob).await;
    let other = InviteSecret::generate(&SeededRng::new(12));
    let forged = space.join_forged(&bob, invite_id(&secret), &other);
    let (status, body) = world.membership(feed, &bob_token, &[forged.encode()]).await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::NOT_FOUND, "not_found")
    );
    // The real Join works once; its replay by another account is the same 404, and the open too.
    let join = space.join(&bob, &secret);
    let (status, _) = world.membership(feed, &bob_token, &[join.encode()]).await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&join);
    let carol_token = world.login(&carol).await;
    let replay = space.join(&carol, &secret);
    let (status, body) = world
        .membership(feed, &carol_token, &[replay.encode()])
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::NOT_FOUND, "not_found")
    );
    let (status, used_body) = world.json(open(&secret), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        used_body, unknown_body,
        "a used invite answers like an unknown one"
    );
    // A stranger may only post its own Joins: anything else is 404 too.
    let erin_token = world.login(&erin).await;
    let profile = space.profile(&erin, "Erin");
    let (status, _) = world
        .membership(feed, &erin_token, &[profile.encode()])
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = world.membership(feed, &erin_token, &[join.encode()]).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a Join signed by someone else"
    );

    // Revoked, expired.
    let (invite, revoked_secret) = space.invite(&alice, MemberRole::Viewer, week, None);
    let (status, _) = world
        .membership(feed, &alice_token, &[invite.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&invite);
    let revoke = space.invite_revoke(&alice, invite_id(&revoked_secret));
    let (status, _) = world
        .membership(feed, &alice_token, &[revoke.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&revoke);
    let (status, revoked_body) = world.json(open(&revoked_secret), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(revoked_body, unknown_body);
    let (invite, soon_secret) = space.invite(
        &alice,
        MemberRole::Viewer,
        world.clock_ms() + 3_600_000,
        None,
    );
    let (status, _) = world
        .membership(feed, &alice_token, &[invite.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&invite);
    assert_eq!(world.json(open(&soon_secret), None).await.0, StatusCode::OK);
    world.clock.advance(2 * 3_600_000);
    let (status, expired_body) = world.json(open(&soon_secret), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(expired_body, unknown_body);
    // Malformed.
    let bad = world
        .server
        .http
        .post(world.url(&format!("/v2/invites/{}/open", invite_id(&secret))))
        .json(&json!({ "signature": "zz" }));
    assert_eq!(world.json(bad, None).await.0, StatusCode::BAD_REQUEST);
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_signup_beyond_the_limit_is_403_signup_limit() {
    let world = World::start(|config| {
        config.share_signups = 1;
        config.share_signups_per_account = 5;
    })
    .await;
    let alice = world.enrolled().await;
    let mut space = world.space("Signups");
    let alice_token = world.found(&mut space, &alice).await;
    let epoch = world.server_epoch().await;
    let week = world.clock_ms() + 7 * DAY_MS;
    let signup = |newcomer: &Person, secret: &InviteSecret| {
        let sig = Space::signup_sig(secret, newcomer.account, epoch);
        let log: Vec<String> = newcomer.encoded_log().iter().map(|e| base64(e)).collect();
        world
            .server
            .http
            .post(world.url("/v2/accounts"))
            .json(&json!({
                "share": { "inviteId": invite_id(secret), "signature": hex(&sig.to_bytes()) },
                "log": log,
            }))
    };
    let mut results = Vec::new();
    for _ in 0..2 {
        let (invite, secret) = space.invite(&alice, MemberRole::Viewer, week, None);
        let (status, _) = world
            .membership(space.feed, &alice_token, &[invite.encode()])
            .await;
        assert_eq!(status, StatusCode::OK);
        space.accept(&invite);
        let newcomer = world.person();
        let (status, body) = world.json(signup(&newcomer, &secret), None).await;
        results.push((status, error_code(&body).to_owned()));
    }
    assert_eq!(results[0].0, StatusCode::CREATED);
    assert_eq!(
        results[1],
        (StatusCode::FORBIDDEN, "signup_limit".to_owned())
    );
    world.server.stop().await;
}

// ----- snapshots, pruning and chunk deletes ---------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_truncating_snapshot_that_is_disputed_is_never_pruned() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let mut space = world.space("Truncation");
    let alice_token = world.found(&mut space, &alice).await;
    let (bob_token, _) = world
        .add(&mut space, &alice, &alice_token, &bob, MemberRole::Editor)
        .await;
    let feed = space.feed;
    let doc = kb_types::DocId::from_bytes([3; 16]);
    for seq in 1..=3 {
        let entry = space.doc_entry(&alice, 0, doc, seq, space.epoch, &[seq as u8; 32]);
        let (status, _) = world
            .post_bytes(&format!("/v2/feeds/{feed}/entries"), &alice_token, &entry)
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (_, page_bytes) = {
        let response = world
            .server
            .http
            .get(world.url(&format!("/v2/feeds/{feed}/changes?after=0&limit=50")))
            .bearer_auth(&alice_token)
            .send()
            .await
            .unwrap();
        (response.status(), response.bytes().await.unwrap())
    };
    let page = kb_sync::ChangesPage::decode(&page_bytes).unwrap();
    let tip = page.changes.last().unwrap().0;
    // Bob, a hostile Editor, publishes a snapshot that claims the whole feed and holds none of it.
    let truncating = space.doc_snapshot(&bob, 0, doc, tip, space.epoch, b"nothing");
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/snapshots/{doc}"),
            &bob_token,
            &truncating,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // The marks announce it; Alice finds it incomplete and disputes it.
    let (_, marks) = world
        .get(
            &format!("/v2/feeds/{feed}/snapshot-marks?after=0"),
            &alice_token,
        )
        .await;
    assert_eq!(marks["marks"].as_array().unwrap().len(), 1);
    assert_eq!(marks["marks"][0]["coversCursor"], tip);
    assert_eq!(marks["marks"][0]["accountId"], json!(bob.account));
    let (status, _) = world
        .json(
            world
                .server
                .http
                .post(world.url(&format!("/v2/feeds/{feed}/snapshots/{doc}/dispute")))
                .json(&json!({ "coversCursor": tip })),
            Some(&alice_token),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Everyone acknowledges; a month passes; the dispute keeps the entries.
    let epoch = world.server_epoch().await;
    let chain = page.chain_hash_after;
    for (person, token) in [(&alice, &alice_token), (&bob, &bob_token)] {
        let head = space.head_of(person, 0, tip, chain, epoch);
        let (status, _) = world
            .put_bytes(&format!("/v2/feeds/{feed}/heads"), token, &head)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    world.clock.advance(30 * DAY_MS);
    let report = world.server.server.prune_now().unwrap();
    assert_eq!(report.entries, 0, "nothing is pruned under a dispute");
    let alice_token = world.login(&alice).await;
    let still = world.changes_status(feed, &alice_token, 0).await;
    assert_eq!(still, StatusCode::OK);
    // Alice republishes a full snapshot: the dispute is over and the entries go after the grace period.
    let full = space.doc_snapshot(&alice, 0, doc, tip, space.epoch, b"the whole document");
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/snapshots/{doc}"),
            &alice_token,
            &full,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    world.clock.advance(8 * DAY_MS);
    let bob_token = world.login(&bob).await;
    let alice_token = world.login(&alice).await;
    let _ = bob_token;
    let report = world.server.server.prune_now().unwrap();
    assert_eq!(report.entries, 3, "the three covered entries are pruned");
    // A dispute of a document without a snapshot is 404; of a replaced snapshot a no-op.
    let (status, _) = world
        .json(
            world
                .server
                .http
                .post(world.url(&format!(
                    "/v2/feeds/{feed}/snapshots/{}/dispute",
                    kb_types::DocId::from_bytes([4; 16])
                )))
                .json(&json!({ "coversCursor": 1 })),
            Some(&alice_token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_owners_delete_chunks_and_a_regression_of_a_snapshot_conflicts() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let mut space = world.space("Chunks");
    let alice_token = world.found(&mut space, &alice).await;
    let (bob_token, _) = world
        .add(&mut space, &alice, &alice_token, &bob, MemberRole::Editor)
        .await;
    let feed = space.feed;
    let (id, bytes) = space.random_chunk(100);
    let path = format!("/v2/feeds/{feed}/chunks/{}", id.to_hex());
    let (status, _) = world.put_bytes(&path, &bob_token, &bytes).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = world.delete(&path, &bob_token).await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    let (status, _) = world.delete(&path, &alice_token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = world.delete(&path, &alice_token).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "deleting a chunk the feed lacks is fine"
    );
    // Copy from the personal namespace.
    let (mine, mine_bytes) = space.random_chunk(50);
    let personal = FeedId::from_account(bob.account);
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{personal}/chunks/{}", mine.to_hex()),
            &bob_token,
            &mine_bytes,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let copy = format!("/v2/feeds/{feed}/chunks/{}/copy", mine.to_hex());
    let (status, _) = world
        .json(world.server.http.post(world.url(&copy)), Some(&bob_token))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = world
        .json(world.server.http.post(world.url(&copy)), Some(&alice_token))
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Alice's personal namespace lacks the chunk"
    );

    // A snapshot that covers less than the stored one is 409.
    let doc = kb_types::DocId::from_bytes([5; 16]);
    let newer = space.doc_snapshot(&alice, 0, doc, 5, space.epoch, b"newer");
    let older = space.doc_snapshot(&alice, 0, doc, 4, space.epoch, b"older");
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/snapshots/{doc}"),
            &alice_token,
            &newer,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/snapshots/{doc}"),
            &alice_token,
            &older,
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "conflict")
    );
    // A snapshot of another document than the path names is 400.
    let other = kb_types::DocId::from_bytes([6; 16]);
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/snapshots/{other}"),
            &alice_token,
            &newer,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    world.server.stop().await;
}

// ----- restart, upgrade, notifications, the page ------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn the_rules_of_a_feed_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FixedClock::new(T0));
    let first = TestServer::start_with_clock(&config(dir.path()), clock.clone()).await;
    let world = World {
        _dir: tempfile::tempdir().unwrap(),
        server: first,
        clock: clock.clone(),
        next_person: std::cell::Cell::new(0),
    };
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let carol = world.enrolled().await;
    let mut space = world.space("Restart");
    let alice_token = world.found(&mut space, &alice).await;
    let (_, _) = world
        .add(&mut space, &alice, &alice_token, &bob, MemberRole::Viewer)
        .await;
    let (carol_token, _) = world
        .add(&mut space, &alice, &alice_token, &carol, MemberRole::Editor)
        .await;
    let leave = space.leave(&carol);
    let (status, _) = world
        .membership(space.feed, &carol_token, &[leave.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&leave);
    let feed = space.feed;
    world.server.stop().await;

    // The same data directory, a new process's state: the rules are rebuilt from the stored log.
    let again = TestServer::start_with_clock(&config(dir.path()), clock.clone()).await;
    let world = World {
        _dir: tempfile::tempdir().unwrap(),
        server: again,
        clock,
        next_person: std::cell::Cell::new(100),
    };
    let alice_token = world.login(&alice).await;
    let bob_token = world.login(&bob).await;
    let doc = kb_types::DocId::from_bytes([8; 16]);
    // Bob is still a Viewer, Carol still departed, Alice still an Owner and a Rekey is still due.
    let (status, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &bob_token,
            &space.doc_entry(&bob, 0, doc, 1, space.epoch, b"x"),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    let carol_token = world.login(&carol).await;
    assert_eq!(
        world.changes_status(feed, &carol_token, 0).await,
        StatusCode::OK,
        "a departed account reads up to its Leave"
    );
    let (status, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &alice_token,
            &space.doc_entry(&alice, 0, doc, 1, space.epoch, b"x"),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (StatusCode::CONFLICT, "rekey_needed")
    );
    let rekey = space.rekey(&alice, &[&alice, &bob]);
    let (status, _) = world
        .membership(feed, &alice_token, &[rekey.encode()])
        .await;
    assert_eq!(status, StatusCode::OK);
    space.accept(&rekey);
    let (status, _) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &alice_token,
            &space.doc_entry(&alice, 0, doc, 1, space.epoch, b"x"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_database_of_protocol_v1_days_is_refused_with_a_clear_message() {
    let dir = tempfile::tempdir().unwrap();
    {
        let server = TestServer::start(&config(dir.path())).await;
        server.stop().await;
    }
    // Make it look like a file of protocol v1's days: its tables, schema version 2.
    {
        let conn = rusqlite::Connection::open(dir.path().join("server.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (cursor INTEGER PRIMARY KEY AUTOINCREMENT);
             PRAGMA user_version=2;",
        )
        .unwrap();
    }
    let Err(error) = kb_server::Server::open(&config(dir.path())) else {
        panic!("a database of protocol v1's days must be refused");
    };
    let text = error.to_string();
    assert!(text.contains("protocol v1"), "{text}");
    assert!(text.contains("fresh"), "{text}");
    // The data directory is left as it was: nothing was dropped.
    let version: i64 = rusqlite::Connection::open(dir.path().join("server.db"))
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    // A fresh data directory starts at schema version 3.
    let fresh = tempfile::tempdir().unwrap();
    let server = TestServer::start(&config(fresh.path())).await;
    server.stop().await;
    let version: i64 = rusqlite::Connection::open(fresh.path().join("server.db"))
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_websocket_pushes_the_entry_to_every_subscribed_member_and_to_no_stranger() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let erin = world.enrolled().await;
    let mut space = world.space("Pushes");
    let alice_token = world.found(&mut space, &alice).await;
    let (bob_token, _) = world
        .add(&mut space, &alice, &alice_token, &bob, MemberRole::Editor)
        .await;
    let erin_token = world.login(&erin).await;
    let ws = support::realtime::ServerWs {
        server: &world.server,
    };
    let bob_socket = ws.connect_with(&bob_token).await.unwrap();
    bob_socket.subscribe_and_wait(&[space.feed]).await.unwrap();
    // A stranger may subscribe, and hears nothing.
    let erin_socket = ws.connect_with(&erin_token).await.unwrap();
    erin_socket.subscribe_and_wait(&[space.feed]).await.unwrap();
    let doc = kb_types::DocId::from_bytes([2; 16]);
    let entry = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"hello");
    let (status, appended) = world
        .post_bytes(
            &format!("/v2/feeds/{}/entries", space.feed),
            &alice_token,
            &entry,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let cursor = appended["cursor"].as_u64().unwrap();
    let pushed = match bob_socket.recv(10_000).await {
        Some(kb_sync::v2::ServerFrame::Entry(pushed)) => pushed,
        other => panic!("an entry frame within 10 s, got {other:?}"),
    };
    assert_eq!(
        (pushed.feed, pushed.cursor, pushed.entry.as_slice()),
        (space.feed, cursor, &entry[..])
    );
    assert_eq!(
        hex(&pushed.chain_after),
        appended["chainHash"].as_str().unwrap()
    );
    assert!(pushed.prev_cursor < cursor);
    assert_eq!(erin_socket.recv(300).await, None, "a stranger got a frame");
    drop(bob_socket);
    drop(erin_socket);
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_join_page_is_static_and_strict() {
    let world = World::start(|_| {}).await;
    for (path, content_type, needle) in [
        ("/join", "text/html", "Open in atomik-kb"),
        ("/join.js", "text/javascript", "atomik-kb://invite?server="),
        ("/join.css", "text/css", "button"),
    ] {
        let response = world.server.http.get(world.url(path)).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let headers = response.headers().clone();
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with(content_type)
        );
        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'"));
        assert!(!csp.contains("unsafe-inline"));
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        let text = response.text().await.unwrap();
        assert!(text.contains(needle), "{path} lacks {needle}");
        if path == "/join" {
            assert!(!text.contains("<script>"), "no inline script");
            assert!(text.contains("src=\"/join.js\""));
        }
    }
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_of_another_account_cannot_use_a_feed_it_does_not_belong_to() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let bob = world.enrolled().await;
    let mut space = world.space("Isolation");
    let _ = world.found(&mut space, &alice).await;
    let bob_token = world.login(&bob).await;
    let alice_personal = FeedId::from_account(alice.account);
    for path in [
        format!("/v2/feeds/{}/changes", space.feed),
        format!("/v2/feeds/{alice_personal}/changes"),
        format!("/v2/feeds/{}/heads", space.feed),
        format!("/v2/feeds/{alice_personal}/snapshots"),
    ] {
        let (status, body) = world.get(&path, &bob_token).await;
        assert_eq!(
            (status, error_code(&body)),
            (StatusCode::NOT_FOUND, "not_found"),
            "{path}"
        );
    }
    let _ = (AccountId::from_bytes([0; 16]),);
    world.server.stop().await;
}

// ----- the personal feed and the account endpoints (what the retired `/v1` tests covered) --------

fn rng(seed: u64) -> SeededRng {
    SeededRng::new(seed)
}

/// A person with two devices in the log (genesis and one `DeviceAdded`), and the entry that
/// revokes the second device (the next valid entry of the log).
fn two_devices(seed: u64) -> (Person, Vec<u8>) {
    let mut person = Person::generate(seed, "Pat", T0);
    person.add_device(T0);
    let mut later = person.clone();
    later.revoke_device(1, T0 + 1);
    let revocation = later.encoded_log_after(person.log_seq()).remove(0);
    (person, revocation)
}

async fn status_of(request: RequestBuilder) -> StatusCode {
    request.send().await.unwrap().status()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_epoch_survives_a_restart_and_a_new_data_directory_gets_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let alice = Person::generate(SEED + 10, "Alice", T0);
    let feed = FeedId::from_account(alice.account);
    let doc = DocId::from_bytes([7; 16]);
    let first = TestServer::start(&config(dir.path())).await;
    let epoch = first.epoch().await;
    assert_eq!(first.create_account(&alice).await, StatusCode::CREATED);
    let token = first.login(&alice, 0).await;
    let entry = sealed_entry(feed, &alice, 0, doc, 1, 0, b"kept", &rng(1));
    let appended: Value = first
        .http
        .post(first.url(&format!("/v2/feeds/{feed}/entries")))
        .bearer_auth(&token)
        .header("content-type", "application/octet-stream")
        .body(entry.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    first.stop().await;

    let again = TestServer::start(&config(dir.path())).await;
    assert_eq!(again.epoch().await, epoch);
    // The data and the session survived too.
    assert_eq!(again.create_account(&alice).await, StatusCode::CONFLICT);
    let response = again
        .http
        .get(again.url(&format!("/v2/feeds/{feed}/changes?after=0&limit=10")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = kb_sync::ChangesPage::decode(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(page.server_epoch, epoch);
    assert_eq!(
        page.changes,
        vec![(appended["cursor"].as_u64().unwrap(), entry)]
    );
    again.stop().await;

    let other_dir = tempfile::tempdir().unwrap();
    let other = TestServer::start(&config(other_dir.path())).await;
    assert_ne!(other.epoch().await, epoch);
    other.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn accounts_refuse_malformed_and_forged_logs_and_a_server_without_tokens_creates_none() {
    let world = World::start(|_| {}).await;
    let (person, _) = two_devices(SEED + 11);
    let log: Vec<String> = person.encoded_log().iter().map(|e| base64(e)).collect();
    let post = |body: Value| {
        world
            .server
            .http
            .post(world.url("/v2/accounts"))
            .json(&body)
    };
    let garbage = post(json!({ "inviteToken": "dev", "log": ["not base64!"] }));
    assert_eq!(status_of(garbage).await, StatusCode::BAD_REQUEST);
    let not_json = world.server.http.post(world.url("/v2/accounts")).body("{");
    assert_eq!(status_of(not_json).await, StatusCode::BAD_REQUEST);
    // Without its genesis the log does not verify.
    let headless = post(json!({ "inviteToken": "dev", "log": [log[1].clone()] }));
    assert_eq!(status_of(headless).await, StatusCode::BAD_REQUEST);
    // A forged signature on the genesis.
    let mut forged = person.encoded_log();
    let last = forged[0].len() - 1;
    forged[0][last] ^= 1;
    let forged: Vec<String> = forged.iter().map(|e| base64(e)).collect();
    let forged = post(json!({ "inviteToken": "dev", "log": forged }));
    assert_eq!(status_of(forged).await, StatusCode::FORBIDDEN);
    // Nothing of it was kept: the real log is created now.
    let response = post(json!({ "inviteToken": "dev", "log": log }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    world.server.stop().await;

    // A server without invite tokens creates no account at all.
    let closed_dir = tempfile::tempdir().unwrap();
    let mut closed = config(closed_dir.path());
    closed.invite_tokens.clear();
    let closed = TestServer::start(&closed).await;
    let refused = closed
        .http
        .post(closed.url("/v2/accounts"))
        .json(&json!({ "inviteToken": "", "log": log }));
    assert_eq!(status_of(refused).await, StatusCode::UNAUTHORIZED);
    closed.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_grows_by_verified_continuations_without_a_session() {
    let world = World::start(|_| {}).await;
    let (alice, revocation) = two_devices(SEED + 12);
    assert_eq!(world.enroll(&alice).await, StatusCode::CREATED);
    let log_url = world.url(&format!("/v2/accounts/{}/log", alice.account));
    let revocation = base64(&revocation);

    // No bearer needed: the entries carry their own signatures.
    for _ in 0..2 {
        let append = world
            .server
            .http
            .post(&log_url)
            .json(&json!({ "entries": [revocation] }));
        assert_eq!(status_of(append).await, StatusCode::NO_CONTENT);
    }
    // `log` is accepted as the field name too; re-sending stored entries is fine.
    let mut whole: Vec<String> = alice.encoded_log().iter().map(|e| base64(e)).collect();
    whole.push(revocation.clone());
    let resend = world
        .server
        .http
        .post(&log_url)
        .json(&json!({ "log": whole }));
    assert_eq!(status_of(resend).await, StatusCode::NO_CONTENT);
    let fetched: Value = world
        .server
        .http
        .get(&log_url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let entries: Vec<String> = fetched["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(entries, whole);

    // Another entry at sequence 2 forks the log.
    let mut fork = alice.clone();
    fork.add_device(T0 + 5);
    let forked = base64(&fork.encoded_log_after(1).remove(0));
    let conflict = world
        .server
        .http
        .post(&log_url)
        .json(&json!({ "entries": [forked] }));
    assert_eq!(status_of(conflict).await, StatusCode::CONFLICT);
    // A gap does not continue the chain either: sequence 4 before 3.
    let mut ahead = alice.clone();
    ahead.revoke_device(1, T0 + 1);
    ahead.add_device(T0 + 6);
    ahead.add_device(T0 + 7);
    let gap = base64(&ahead.encoded_log_after(3).remove(0));
    let gap = world
        .server
        .http
        .post(&log_url)
        .json(&json!({ "entries": [gap] }));
    assert_eq!(status_of(gap).await, StatusCode::CONFLICT);
    let bad = world
        .server
        .http
        .post(&log_url)
        .json(&json!({ "entries": ["%%%"] }));
    assert_eq!(status_of(bad).await, StatusCode::BAD_REQUEST);

    let unknown = AccountId::from_bytes([9; 16]);
    let unknown_url = world.url(&format!("/v2/accounts/{unknown}/log"));
    assert_eq!(
        status_of(world.server.http.get(&unknown_url)).await,
        StatusCode::NOT_FOUND
    );
    let to_unknown = world
        .server
        .http
        .post(&unknown_url)
        .json(&json!({ "entries": [revocation] }));
    assert_eq!(status_of(to_unknown).await, StatusCode::NOT_FOUND);
    let malformed = world
        .server
        .http
        .get(world.url("/v2/accounts/not-an-id/log"));
    assert_eq!(status_of(malformed).await, StatusCode::BAD_REQUEST);
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn authentication_refuses_every_bad_attempt() {
    let world = World::start(|_| {}).await;
    let (person, _) = two_devices(SEED + 13);
    assert_eq!(world.enroll(&person).await, StatusCode::CREATED);
    let server = &world.server;
    let (account, device) = (person.account, person.device_id(0));
    let stranger = Person::generate(SEED + 14, "Stranger", T0);

    let (status, _) = server
        .challenge(AccountId::from_bytes([9; 16]), device)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown account");
    let (status, _) = server.challenge(account, stranger.device_id(0)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "device outside the log");
    let malformed = server
        .http
        .post(server.url("/v2/auth/challenge"))
        .json(&json!({ "accountId": "x", "deviceId": device }));
    assert_eq!(status_of(malformed).await, StatusCode::BAD_REQUEST);

    let sign = |nonce: &str, epoch: &str, device: usize| {
        person.signer(device)(&kb_sync::auth_message(&unhex(nonce), &unhex(epoch)))
    };
    let fresh = || async move {
        let (status, body) = server.challenge(account, device).await;
        assert_eq!(status, StatusCode::OK);
        (
            body["nonce"].as_str().unwrap().to_owned(),
            body["serverEpoch"].as_str().unwrap().to_owned(),
        )
    };

    // Another device's signature.
    let (nonce, epoch) = fresh().await;
    assert_eq!(epoch, hex(&server.server.server_epoch()));
    let (status, _) = server
        .session(account, device, &nonce, &sign(&nonce, &epoch, 1))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "wrong signature");
    // The challenge was used up by the failed attempt.
    let (status, _) = server
        .session(account, device, &nonce, &sign(&nonce, &epoch, 0))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "used challenge");
    // A challenge for device 0 does not open a session for device 1.
    let (nonce, epoch) = fresh().await;
    let (status, _) = server
        .session(
            account,
            person.device_id(1),
            &nonce,
            &sign(&nonce, &epoch, 1),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "challenge of another device"
    );
    // A signature over another epoch.
    let (nonce, _) = fresh().await;
    let other_epoch = hex(&[0u8; 16]);
    let (status, _) = server
        .session(account, device, &nonce, &sign(&nonce, &other_epoch, 0))
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "signature bound to another epoch"
    );
    // Expired after two minutes.
    let (nonce, epoch) = fresh().await;
    server.clock.advance(120_001);
    let (status, _) = server
        .session(account, device, &nonce, &sign(&nonce, &epoch, 0))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "expired challenge");
    // Malformed fields.
    let (nonce, epoch) = fresh().await;
    let (status, _) = server
        .session(account, device, "abc", &sign(&nonce, &epoch, 0))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "malformed nonce");
    let short = server
        .http
        .post(server.url("/v2/auth/session"))
        .json(&json!({
            "accountId": account, "deviceId": device, "nonce": nonce, "signature": "00",
        }));
    assert_eq!(
        status_of(short).await,
        StatusCode::BAD_REQUEST,
        "malformed signature"
    );

    // A good session works until it expires 24 hours later.
    let token = server.login(&person, 0).await;
    let feeds = |token: &str| server.http.get(server.url("/v2/feeds")).bearer_auth(token);
    assert_eq!(status_of(feeds(&token)).await, StatusCode::OK);
    assert_eq!(
        status_of(server.http.get(server.url("/v2/feeds"))).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(feeds("not-a-token")).await,
        StatusCode::UNAUTHORIZED
    );
    let basic = server
        .http
        .get(server.url("/v2/feeds"))
        .header("authorization", format!("Basic {token}"));
    assert_eq!(status_of(basic).await, StatusCode::UNAUTHORIZED);
    server.clock.advance(86_400_000 - 1);
    assert_eq!(status_of(feeds(&token)).await, StatusCode::OK);
    server.clock.advance(1);
    assert_eq!(
        status_of(feeds(&token)).await,
        StatusCode::UNAUTHORIZED,
        "expired session"
    );
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_device_loses_its_session_and_its_entries_are_refused() {
    let world = World::start(|_| {}).await;
    let (alice, revocation) = two_devices(SEED + 15);
    assert_eq!(world.enroll(&alice).await, StatusCode::CREATED);
    let server = &world.server;
    let feed = FeedId::from_account(alice.account);
    let revoked = server.login(&alice, 1).await;
    let kept = server.login(&alice, 0).await;
    let append = server
        .http
        .post(world.url(&format!("/v2/accounts/{}/log", alice.account)))
        .json(&json!({ "entries": [base64(&revocation)] }));
    assert_eq!(status_of(append).await, StatusCode::NO_CONTENT);

    let feeds = |token: &str| server.http.get(world.url("/v2/feeds")).bearer_auth(token);
    assert_eq!(status_of(feeds(&revoked)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status_of(feeds(&kept)).await, StatusCode::OK);
    let (status, _) = server.challenge(alice.account, alice.device_id(1)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Its entries are refused even through another device's session.
    let late = sealed_entry(
        feed,
        &alice,
        1,
        DocId::from_bytes([1; 16]),
        1,
        0,
        b"late",
        &rng(2),
    );
    let (status, _) = world
        .post_bytes(&format!("/v2/feeds/{feed}/entries"), &kept, &late)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn entries_of_the_personal_feed_are_checked_deduplicated_chained_and_paged() {
    let world = World::start(|_| {}).await;
    let (alice, _) = two_devices(SEED + 16);
    assert_eq!(world.enroll(&alice).await, StatusCode::CREATED);
    let token = world.server.login(&alice, 0).await;
    let feed = FeedId::from_account(alice.account);
    let epoch = world.server_epoch().await;
    let entries = format!("/v2/feeds/{feed}/entries");
    let doc = DocId::from_bytes([7; 16]);
    let other_doc = DocId::from_bytes([8; 16]);

    let first = sealed_entry(feed, &alice, 0, doc, 1, 0, b"one", &rng(3));
    let (status, body) = world.post_bytes(&entries, &token, &first).await;
    assert_eq!(status, StatusCode::OK);
    let cursor = body["cursor"].as_u64().unwrap();
    let chain = kb_sync::v2::chain_next(&kb_sync::v2::chain_genesis(&epoch), &first);
    assert_eq!(body["chainHash"], json!(hex(&chain)));
    let resealed = sealed_entry(feed, &alice, 0, doc, 1, 0, b"resealed", &rng(4));
    let (status, again) = world.post_bytes(&entries, &token, &resealed).await;
    assert_eq!((status, again), (StatusCode::OK, body.clone()));
    let second = sealed_entry(feed, &alice, 0, other_doc, 1, 0, b"two", &rng(5));
    let (status, body) = world.post_bytes(&entries, &token, &second).await;
    assert_eq!(status, StatusCode::OK);
    let second_cursor = body["cursor"].as_u64().unwrap();
    assert!(second_cursor > cursor);

    // Refusals.
    let theirs = sealed_entry(feed, &alice, 1, doc, 1, 0, b"theirs", &rng(6));
    let (status, _) = world.post_bytes(&entries, &token, &theirs).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "signed by another device than the session's"
    );
    let mut tampered = sealed_entry(feed, &alice, 0, doc, 2, 0, b"tampered", &rng(7));
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    let (status, body) = world.post_bytes(&entries, &token, &tampered).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "broken signature");
    assert_eq!(error_code(&body), "authentication");
    let (status, _) = world.post_bytes(&entries, &token, b"not an envelope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let snapshot = sealed_snapshot(feed, &alice, 0, doc, 1, 0, b"not a feed item", &rng(8));
    let (status, _) = world.post_bytes(&entries, &token, &snapshot).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a snapshot is not a feed item"
    );
    let oversized = vec![0u8; usize::try_from(kb_types::sync::MAX_WIRE_BYTES).unwrap() + 1];
    let (status, _) = world.post_bytes(&entries, &token, &oversized).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = world.post_bytes(&entries, "bad", &first).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The page: binary, in cursor order, chained.
    let page_of = |query: &str| {
        world
            .server
            .http
            .get(world.url(&format!("/v2/feeds/{feed}/changes{query}")))
            .bearer_auth(&token)
    };
    let response = page_of("?after=0&limit=1000").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    let page = kb_sync::ChangesPage::decode(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(page.server_epoch, epoch);
    assert_eq!(
        page.changes,
        vec![(cursor, first.clone()), (second_cursor, second.clone())]
    );
    assert_eq!(
        page.chain_hash_after,
        kb_sync::v2::chain_next(&chain, &second)
    );
    assert!(!page.more);
    let decode = |bytes: &[u8]| kb_sync::ChangesPage::decode(bytes).unwrap();
    let one = decode(
        &page_of("?after=0&limit=1")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    );
    assert_eq!(
        (one.changes.len(), one.more, one.chain_hash_after),
        (1, true, chain)
    );
    let clamped = decode(
        &page_of("?after=0&limit=0")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    );
    assert_eq!(
        (clamped.changes.len(), clamped.more),
        (1, true),
        "limit 0 counts as 1"
    );
    let default = decode(&page_of("").send().await.unwrap().bytes().await.unwrap());
    assert_eq!(
        default.changes.len(),
        2,
        "no query: after 0, the default limit"
    );
    assert_eq!(
        status_of(page_of("?after=minus")).await,
        StatusCode::BAD_REQUEST
    );
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshots_of_the_personal_feed_replace_check_their_header_and_refuse_regressions() {
    let world = World::start(|_| {}).await;
    let (alice, _) = two_devices(SEED + 17);
    assert_eq!(world.enroll(&alice).await, StatusCode::CREATED);
    let token = world.server.login(&alice, 0).await;
    let feed = FeedId::from_account(alice.account);
    let doc = DocId::from_bytes([7; 16]);
    let elsewhere = DocId::from_bytes([8; 16]);
    let entry = sealed_entry(feed, &alice, 0, doc, 1, 0, b"x", &rng(9));
    let (_, appended) = world
        .post_bytes(&format!("/v2/feeds/{feed}/entries"), &token, &entry)
        .await;
    let cursor = appended["cursor"].as_u64().unwrap();
    let put = |doc: DocId, bytes: Vec<u8>| {
        world
            .server
            .http
            .put(world.url(&format!("/v2/feeds/{feed}/snapshots/{doc}")))
            .bearer_auth(&token)
            .body(bytes)
    };
    let get = |doc: DocId| {
        world
            .server
            .http
            .get(world.url(&format!("/v2/feeds/{feed}/snapshots/{doc}")))
            .bearer_auth(&token)
    };

    assert_eq!(
        status_of(get(doc)).await,
        StatusCode::NO_CONTENT,
        "a member's feed holds nothing under that key: 204"
    );
    let first = sealed_snapshot(feed, &alice, 0, doc, cursor, 0, b"first", &rng(10));
    assert_eq!(
        status_of(put(doc, first.clone())).await,
        StatusCode::NO_CONTENT
    );
    let fetched = get(doc).send().await.unwrap();
    assert_eq!(
        fetched.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(fetched.bytes().await.unwrap().to_vec(), first);

    assert_eq!(
        status_of(put(elsewhere, first.clone())).await,
        StatusCode::BAD_REQUEST,
        "the path names another document"
    );
    let theirs = sealed_snapshot(feed, &alice, 1, doc, cursor, 0, b"theirs", &rng(11));
    assert_eq!(status_of(put(doc, theirs)).await, StatusCode::FORBIDDEN);
    let mut tampered = sealed_snapshot(feed, &alice, 0, doc, cursor, 0, b"tampered", &rng(12));
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert_eq!(status_of(put(doc, tampered)).await, StatusCode::FORBIDDEN);
    assert_eq!(
        status_of(put(doc, b"garbage".to_vec())).await,
        StatusCode::BAD_REQUEST
    );
    let older = sealed_snapshot(feed, &alice, 0, doc, cursor - 1, 0, b"older", &rng(13));
    assert_eq!(
        status_of(put(doc, older)).await,
        StatusCode::CONFLICT,
        "a snapshot covering less than the stored one"
    );
    let same = sealed_snapshot(
        feed,
        &alice,
        0,
        doc,
        cursor,
        0,
        b"same cover, newer",
        &rng(14),
    );
    assert_eq!(
        status_of(put(doc, same.clone())).await,
        StatusCode::NO_CONTENT
    );
    let other = sealed_snapshot(feed, &alice, 0, elsewhere, 0, 0, b"other", &rng(15));
    assert_eq!(
        status_of(put(elsewhere, other.clone())).await,
        StatusCode::NO_CONTENT
    );

    let all = world
        .server
        .http
        .get(world.url(&format!("/v2/feeds/{feed}/snapshots")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(all.headers()["content-type"], "application/octet-stream");
    let all = kb_sync::SnapshotsPage::decode(&all.bytes().await.unwrap()).unwrap();
    assert_eq!(all.server_epoch, world.server_epoch().await);
    assert_eq!(all.snapshots.len(), 2);
    assert!(all.snapshots.contains(&same) && all.snapshots.contains(&other));
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chunks_are_content_addressed_idempotent_deleted_with_their_bytes_and_private_to_the_feed()
{
    let world = World::start(|_| {}).await;
    let (alice, revocation) = two_devices(SEED + 18);
    let bob = world.enrolled().await;
    assert_eq!(world.enroll(&alice).await, StatusCode::CREATED);
    let token = world.server.login(&alice, 0).await;
    let revoked = world.server.login(&alice, 1).await;
    let bob_token = world.login(&bob).await;
    let feed = FeedId::from_account(alice.account);
    let bytes = vec![0x5a; 4096];
    let hash = hex(&kb_crypto::hash(&bytes));
    let url = world.url(&format!("/v2/feeds/{feed}/chunks/{hash}"));
    let put = |url: &str, token: &str, body: Vec<u8>| {
        world.server.http.put(url).bearer_auth(token).body(body)
    };
    let used = |account: AccountId| world.server.server.account_bytes_used(account).unwrap();

    assert_eq!(
        status_of(world.server.http.get(&url).bearer_auth(&token)).await,
        StatusCode::NO_CONTENT
    );
    let before = used(alice.account);
    assert_eq!(
        status_of(put(&url, &token, bytes.clone())).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status_of(put(&url, &token, bytes.clone())).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        used(alice.account),
        before + 4096,
        "a chunk stored twice counts once"
    );
    let got = world
        .server
        .http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(got.bytes().await.unwrap().to_vec(), bytes);
    assert_eq!(
        status_of(put(&url, &token, b"other bytes".to_vec())).await,
        StatusCode::BAD_REQUEST,
        "the body does not hash to the path"
    );
    let malformed = world.url(&format!("/v2/feeds/{feed}/chunks/xyz"));
    assert_eq!(
        status_of(put(&malformed, &token, bytes.clone())).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status_of(world.server.http.put(&url).body(bytes.clone())).await,
        StatusCode::UNAUTHORIZED
    );

    // Deleting: a session is needed; another account's session finds no such feed; a revoked
    // device is refused (ADR-0023); the owner's delete gives the bytes back.
    assert_eq!(
        status_of(world.server.http.delete(&url)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(world.server.http.delete(&url).bearer_auth("not-a-session")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(world.server.http.delete(&url).bearer_auth(&bob_token)).await,
        StatusCode::NOT_FOUND,
        "another account's session has no such feed"
    );
    assert_eq!(used(alice.account), before + 4096);
    let revoke = world
        .server
        .http
        .post(world.url(&format!("/v2/accounts/{}/log", alice.account)))
        .json(&json!({ "entries": [base64(&revocation)] }));
    assert_eq!(status_of(revoke).await, StatusCode::NO_CONTENT);
    let held = used(alice.account);
    assert_eq!(
        status_of(world.server.http.delete(&url).bearer_auth(&revoked)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        used(alice.account),
        held,
        "the revoked device deleted nothing"
    );

    let delete = |url: &str| world.server.http.delete(url).bearer_auth(&token);
    assert_eq!(status_of(delete(&url)).await, StatusCode::NO_CONTENT);
    assert_eq!(
        status_of(world.server.http.get(&url).bearer_auth(&token)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        used(alice.account),
        before + (held - before - 4096),
        "the chunk's bytes are given back"
    );
    // Idempotent: again, or a hash never stored, is 204 and frees nothing.
    assert_eq!(status_of(delete(&url)).await, StatusCode::NO_CONTENT);
    let unknown = world.url(&format!("/v2/feeds/{feed}/chunks/{}", "ab".repeat(32)));
    assert_eq!(status_of(delete(&unknown)).await, StatusCode::NO_CONTENT);
    assert_eq!(
        status_of(delete(&world.url(&format!("/v2/feeds/{feed}/chunks/xyz")))).await,
        StatusCode::BAD_REQUEST
    );
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn heads_of_the_personal_feed_are_verified_and_replaced_per_device() {
    let world = World::start(|_| {}).await;
    let (alice, _) = two_devices(SEED + 19);
    assert_eq!(world.enroll(&alice).await, StatusCode::CREATED);
    let token = world.server.login(&alice, 0).await;
    let feed = FeedId::from_account(alice.account);
    let epoch = world.server_epoch().await;
    let mut cursors = Vec::new();
    for seq in 1..=3 {
        let entry = sealed_entry(
            feed,
            &alice,
            0,
            DocId::from_bytes([7; 16]),
            seq,
            0,
            b"e",
            &rng(20 + seq),
        );
        let (_, body) = world
            .post_bytes(&format!("/v2/feeds/{feed}/entries"), &token, &entry)
            .await;
        cursors.push(body["cursor"].as_u64().unwrap());
    }
    let url = world.url(&format!("/v2/feeds/{feed}/heads"));
    let put = |bytes: Vec<u8>| world.server.http.put(&url).bearer_auth(&token).body(bytes);
    let old = signed_head(feed, &alice, 0, cursors[0], [1; 32], epoch, 1);
    let new = signed_head(feed, &alice, 0, cursors[1], [2; 32], epoch, 2);
    assert_eq!(status_of(put(old)).await, StatusCode::NO_CONTENT);
    assert_eq!(status_of(put(new.clone())).await, StatusCode::NO_CONTENT);
    let theirs = signed_head(feed, &alice, 1, cursors[1], [2; 32], epoch, 2);
    assert_eq!(status_of(put(theirs)).await, StatusCode::FORBIDDEN);
    let mut tampered = signed_head(feed, &alice, 0, cursors[2], [3; 32], epoch, 3);
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert_eq!(status_of(put(tampered)).await, StatusCode::FORBIDDEN);
    assert_eq!(
        status_of(put(b"short".to_vec())).await,
        StatusCode::BAD_REQUEST
    );
    let ahead = signed_head(feed, &alice, 0, cursors[2] + 1000, [4; 32], epoch, 4);
    assert_eq!(
        status_of(put(ahead)).await,
        StatusCode::BAD_REQUEST,
        "a head names a cursor the feed has reached"
    );

    let heads: Value = world
        .server
        .http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(heads, json!({ "heads": [base64(&new)] }));
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_personal_feed_prunes_covered_entries_after_the_grace_period() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let token = world.login(&alice).await;
    let feed = FeedId::from_account(alice.account);
    let (doc, other) = (DocId::from_bytes([7; 16]), DocId::from_bytes([8; 16]));
    let mut cursors = Vec::new();
    for seq in 1..=3u64 {
        let entry = sealed_entry(
            feed,
            &alice,
            0,
            doc,
            seq,
            0,
            &[seq as u8; 100],
            &rng(30 + seq),
        );
        let (_, body) = world
            .post_bytes(&format!("/v2/feeds/{feed}/entries"), &token, &entry)
            .await;
        cursors.push(body["cursor"].as_u64().unwrap());
    }
    let unrelated_entry = sealed_entry(feed, &alice, 0, other, 1, 0, b"unrelated", &rng(40));
    let (_, body) = world
        .post_bytes(
            &format!("/v2/feeds/{feed}/entries"),
            &token,
            &unrelated_entry,
        )
        .await;
    let unrelated = body["cursor"].as_u64().unwrap();
    let running = body["chainHash"].as_str().unwrap().to_owned();
    let snapshot = sealed_snapshot(feed, &alice, 0, doc, cursors[1], 0, b"covers two", &rng(41));
    let (status, _) = world
        .put_bytes(
            &format!("/v2/feeds/{feed}/snapshots/{doc}"),
            &token,
            &snapshot,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let before = world
        .server
        .server
        .account_bytes_used(alice.account)
        .unwrap();

    // Not before the grace period (the personal feed has one account's devices only: no
    // acknowledgement of other members is waited for, as for a shared feed).
    world.clock.advance(7 * DAY_MS - 1);
    assert_eq!(
        world.server.server.prune_now().unwrap().entries,
        0,
        "inside the grace period"
    );
    world.clock.advance(1);
    let report = world.server.server.prune_now().unwrap();
    assert_eq!(report.entries, 2);
    assert!(report.bytes > 200);
    assert!(
        world
            .server
            .server
            .account_bytes_used(alice.account)
            .unwrap()
            < before
    );
    // The page shows what is left; pruned entries stay in the chain, so the page answers the true
    // running hash.
    let token = world.login(&alice).await;
    let response = world
        .server
        .http
        .get(world.url(&format!("/v2/feeds/{feed}/changes?after=0&limit=200")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    let page = kb_sync::ChangesPage::decode(&response.bytes().await.unwrap()).unwrap();
    let left: Vec<u64> = page.changes.iter().map(|(c, _)| *c).collect();
    assert_eq!(left, vec![cursors[2], unrelated]);
    assert_eq!(hex(&page.chain_hash_after), running);
    // A pruned entry pushed again is still a duplicate.
    let again = sealed_entry(feed, &alice, 0, doc, 1, 0, &[1u8; 100], &rng(31));
    let (status, body) = world
        .post_bytes(&format!("/v2/feeds/{feed}/entries"), &token, &again)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cursor"].as_u64().unwrap(), cursors[0]);
    assert_eq!(world.server.server.prune_now().unwrap().entries, 0);
    world.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_refuses_a_missing_token_a_bad_token_and_a_plain_request() {
    let world = World::start(|_| {}).await;
    let alice = world.enrolled().await;
    let token = world.login(&alice).await;
    let ws_url = world.url("/v2/ws").replace("http://", "ws://");
    // By header and by query: both open.
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
            ws_url.as_str(),
        )
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (by_header, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let (by_query, _) = tokio_tungstenite::connect_async(format!("{ws_url}?token={token}"))
        .await
        .unwrap();
    // No token, a bad token, or no upgrade.
    assert_eq!(
        status_of(world.server.http.get(world.url("/v2/ws"))).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        tokio_tungstenite::connect_async(format!("{ws_url}?token=wrong"))
            .await
            .is_err()
    );
    assert_eq!(
        status_of(
            world
                .server
                .http
                .get(world.url("/v2/ws"))
                .bearer_auth(&token)
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    drop(by_header);
    drop(by_query);
    world.server.stop().await;
}
