//! `HttpRemote` (protocol v2) against an in-process axum stub of `kb-server`'s surface: the
//! status mapping, the JSON and binary shapes, sessions (bearer token, renewal on 401, a second
//! 401 is final), the caps, the realtime socket, the attachment chunks, the pairing mailboxes
//! and the recovery blobs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::to_bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kb_crypto::{Secret32, SigningKey};
use kb_sync::v2::{
    InlineEntry, PresenceFrame, RemoteStore, encode_changed, encode_presence_frame,
    encode_subscribe,
};
use kb_sync::{ChangesPage, SyncError};
use kb_types::sharing::INLINE_ENTRY_MAX_BYTES;
use kb_types::sync::MAX_WIRE_BYTES;
use kb_types::{AccountId, DeviceId, DocId, FeedId};

use super::*;
use crate::codec::{base64_encode, hex_encode};
use crate::realtime::{Realtime, RealtimeCommand, RealtimeEvent};

const EPOCH: [u8; 16] = [5; 16];
const MAILBOX: [u8; 6] = [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6];
const EXPIRES_AT: i64 = 1_790_000_600_000;
const NONCE: [u8; 32] = [3; 32];

fn account() -> AccountId {
    AccountId::from_bytes([1; 16])
}

fn device() -> DeviceId {
    DeviceId::from_bytes([2; 16])
}

/// The feed of the stub's account (its personal feed).
fn feed() -> FeedId {
    FeedId::from_account(account())
}

fn feed_path(tail: &str) -> String {
    format!("/v2/feeds/{}{tail}", feed())
}

fn device_key() -> SigningKey {
    SigningKey::from_seed(&Secret32::from_bytes([42; 32]))
}

fn signer() -> AuthSigner {
    let key = device_key();
    Arc::new(move |message: &[u8]| Some(kb_sync::sign_auth(message, &key)))
}

/// One request the stub received.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: Option<String>,
    authorization: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// The stub server's state.
#[derive(Default)]
struct Stub {
    /// Every request is answered with this status and body.
    forced: Mutex<Option<(u16, String)>>,
    /// The token the authenticated routes accept.
    valid: Mutex<Option<String>>,
    issued: AtomicUsize,
    challenges: AtomicUsize,
    account_known: AtomicBool,
    requests: Mutex<Vec<Seen>>,
    sockets_opened: AtomicUsize,
    sockets_closed: AtomicUsize,
    /// Upgrades asked for (refused ones included), and whether they are answered with 403.
    ws_attempts: AtomicUsize,
    ws_forbid: AtomicBool,
    /// What a socket sends right after it connects.
    ws_script: Mutex<Vec<WsMessage>>,
    /// What the clients of the sockets sent, in order.
    ws_received: Mutex<Vec<WsMessage>>,
    /// Wakes the socket that is open: it closes (the server hangs up).
    ws_kick: tokio::sync::Notify,
    /// The challenge refuses the device as idle.
    idle: AtomicBool,
    /// The one pairing mailbox the stub knows (`MAILBOX`) and its messages.
    mailbox: Mutex<Vec<(String, String)>>,
    mailbox_closed: AtomicBool,
    /// Recovery blobs by lookup id (hex).
    recovery: Mutex<Vec<(String, Vec<u8>)>>,
    /// Attachment chunks by hash (hex).
    chunks: Mutex<Vec<(String, Vec<u8>)>>,
}

impl Stub {
    fn seen(&self, path: &str) -> Vec<Seen> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.path == path)
            .cloned()
            .collect()
    }

    fn force(&self, status: u16, body: &str) {
        *self.forced.lock().unwrap() = Some((status, body.to_owned()));
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let expected = self
            .valid
            .lock()
            .unwrap()
            .as_ref()
            .map(|token| format!("Bearer {token}"));
        let given = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        expected.is_some() && given == expected.as_deref()
    }
}

fn json(value: &serde_json::Value) -> Response {
    (StatusCode::OK, axum::Json(value.clone())).into_response()
}

fn binary(bytes: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response()
}

async fn handle(State(stub): State<Arc<Stub>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 64 * 1024 * 1024).await.unwrap().to_vec();
    let header_text = |name| {
        parts
            .headers
            .get(name)
            .and_then(|value: &axum::http::HeaderValue| value.to_str().ok())
            .map(str::to_owned)
    };
    let seen = Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().map(str::to_owned),
        authorization: header_text(header::AUTHORIZATION),
        content_type: header_text(header::CONTENT_TYPE),
        body: body.clone(),
    };
    stub.requests.lock().unwrap().push(seen.clone());
    if let Some((status, text)) = stub.forced.lock().unwrap().clone() {
        return (StatusCode::from_u16(status).unwrap(), text).into_response();
    }
    let authed = stub.authorized(&parts.headers);
    let refuse = || StatusCode::UNAUTHORIZED.into_response();
    let account_path = format!("/v2/accounts/{}/log", account());
    let chunks = feed_path("/chunks/");
    match (seen.method.as_str(), seen.path.as_str()) {
        ("GET", "/v2/server") => json(&serde_json::json!({
            "protocolVersion": 2,
            "serverEpoch": hex_encode(&EPOCH),
            "maxWireBytes": MAX_WIRE_BYTES,
            "idleDeviceDays": 90,
        })),
        ("POST", "/v2/auth/challenge") if stub.idle.load(Ordering::SeqCst) => (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "idle", "message": "the device was idle" })),
        )
            .into_response(),
        ("POST", "/v2/pair") => {
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            *stub.mailbox.lock().unwrap() = vec![(
                "joiner".to_owned(),
                request["offer"].as_str().unwrap().to_owned(),
            )];
            (
                StatusCode::CREATED,
                axum::Json(serde_json::json!({
                    "mailboxId": hex_encode(&MAILBOX),
                    "expiresAt": EXPIRES_AT,
                })),
            )
                .into_response()
        }
        ("GET", path) if path == format!("/v2/pair/{}/messages", hex_encode(&MAILBOX)) => {
            let after: usize = seen
                .query
                .as_deref()
                .and_then(|query| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("after="))
                })
                .and_then(|after| after.parse().ok())
                .unwrap_or(0);
            let messages: Vec<serde_json::Value> = stub
                .mailbox
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .skip(after)
                .map(|(seq, (role, bytes))| serde_json::json!({ "seq": seq, "role": role, "bytes": bytes }))
                .collect();
            json(&serde_json::json!({
                "messages": messages,
                "expiresAt": EXPIRES_AT,
                "closed": stub.mailbox_closed.load(Ordering::SeqCst),
            }))
        }
        ("POST", path) if path == format!("/v2/pair/{}/messages", hex_encode(&MAILBOX)) => {
            if stub.mailbox_closed.load(Ordering::SeqCst) {
                return (StatusCode::CONFLICT, "closed").into_response();
            }
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            stub.mailbox.lock().unwrap().push((
                request["role"].as_str().unwrap().to_owned(),
                request["bytes"].as_str().unwrap().to_owned(),
            ));
            StatusCode::NO_CONTENT.into_response()
        }
        ("DELETE", path) if path == format!("/v2/pair/{}", hex_encode(&MAILBOX)) => {
            stub.mailbox_closed.store(true, Ordering::SeqCst);
            StatusCode::NO_CONTENT.into_response()
        }
        (_, path) if path.starts_with("/v2/pair/") => StatusCode::NOT_FOUND.into_response(),
        ("GET", path) if path.starts_with("/v2/recovery/") => {
            let id = path.trim_start_matches("/v2/recovery/");
            match stub
                .recovery
                .lock()
                .unwrap()
                .iter()
                .find(|(other, _)| other == id)
            {
                Some((_, bytes)) => binary(bytes.clone()),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        ("PUT", path) if path.starts_with("/v2/recovery/") && authed => {
            let id = path.trim_start_matches("/v2/recovery/").to_owned();
            let mut store = stub.recovery.lock().unwrap();
            store.retain(|(other, _)| *other != id);
            store.push((id, body.clone()));
            StatusCode::NO_CONTENT.into_response()
        }
        ("DELETE", path) if path.starts_with("/v2/recovery/") && authed => {
            let id = path.trim_start_matches("/v2/recovery/");
            let mut store = stub.recovery.lock().unwrap();
            let held = store.iter().any(|(other, _)| other == id);
            store.retain(|(other, _)| other != id);
            if held {
                StatusCode::NO_CONTENT.into_response()
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        ("PUT", path) if path.starts_with(&chunks) && authed => {
            let hash = path.trim_start_matches(chunks.as_str()).to_owned();
            let mut store = stub.chunks.lock().unwrap();
            store.retain(|(other, _)| *other != hash);
            store.push((hash, body.clone()));
            StatusCode::NO_CONTENT.into_response()
        }
        ("GET", path) if path.starts_with(&chunks) && authed => {
            let hash = path.trim_start_matches(chunks.as_str());
            match stub
                .chunks
                .lock()
                .unwrap()
                .iter()
                .find(|(other, _)| other == hash)
            {
                Some((_, bytes)) => binary(bytes.clone()),
                None => StatusCode::NO_CONTENT.into_response(),
            }
        }
        // `kb-server` answers 204 whether or not the feed held the chunk.
        ("DELETE", path) if path.starts_with(&chunks) && authed => {
            let hash = path.trim_start_matches(chunks.as_str());
            stub.chunks
                .lock()
                .unwrap()
                .retain(|(other, _)| other != hash);
            StatusCode::NO_CONTENT.into_response()
        }
        ("POST", "/v2/accounts") => {
            stub.account_known.store(true, Ordering::SeqCst);
            StatusCode::CREATED.into_response()
        }
        ("GET", path) if path == account_path => {
            if stub.account_known.load(Ordering::SeqCst) {
                json(&serde_json::json!({ "entries": [base64_encode(b"genesis")] }))
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        ("POST", path) if path == account_path => StatusCode::NO_CONTENT.into_response(),
        ("POST", "/v2/auth/challenge") => {
            stub.challenges.fetch_add(1, Ordering::SeqCst);
            json(&serde_json::json!({
                "nonce": hex_encode(&NONCE),
                "serverEpoch": hex_encode(&EPOCH),
            }))
        }
        ("POST", "/v2/auth/session") => {
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let signature =
                crate::codec::hex_decode::<64>("signature", request["signature"].as_str().unwrap())
                    .unwrap();
            let signature = kb_crypto::Signature::from_bytes(&signature).unwrap();
            let message = kb_sync::auth_message(&NONCE, &EPOCH);
            if kb_sync::verify_auth(&message, &signature, &device_key().verifying_key()).is_err() {
                return refuse();
            }
            let n = stub.issued.fetch_add(1, Ordering::SeqCst) + 1;
            let token = format!("token-{n}");
            *stub.valid.lock().unwrap() = Some(token.clone());
            json(&serde_json::json!({ "token": token, "expiresAt": 1_900_000_000 }))
        }
        ("POST", path) if path == feed_path("/entries") && authed => json(&serde_json::json!({
            "cursor": 42,
            "chainHash": hex_encode(&[8; 32]),
        })),
        ("GET", path) if path == feed_path("/changes") && authed => binary(
            ChangesPage {
                server_epoch: EPOCH,
                changes: vec![(7, b"entry".to_vec())],
                chain_hash_after: [1; 32],
                more: false,
            }
            .encode(),
        ),
        ("GET", path) if path == feed_path("/heads") && authed => json(&serde_json::json!({
            "heads": [base64_encode(b"head one"), base64_encode(b"head two")],
        })),
        ("PUT", path) if path == feed_path("/heads") && authed => {
            StatusCode::NO_CONTENT.into_response()
        }
        ("GET", "/v2/big") => binary(vec![0; 2048]),
        // A member's feed holds nothing under that key: 204 (a 404 means "not a member").
        ("GET", path) if path.starts_with(&feed_path("/snapshots/")) && authed => {
            StatusCode::NO_CONTENT.into_response()
        }
        (_, path) if path.starts_with("/v2/") => refuse(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn socket(
    State(stub): State<Arc<Stub>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    stub.ws_attempts.fetch_add(1, Ordering::SeqCst);
    if stub.ws_forbid.load(Ordering::SeqCst) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !stub.authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    upgrade.on_upgrade(move |socket| notify(stub, socket))
}

async fn notify(stub: Arc<Stub>, mut socket: WebSocket) {
    stub.sockets_opened.fetch_add(1, Ordering::SeqCst);
    let script = stub.ws_script.lock().unwrap().clone();
    for frame in script {
        if socket.send(frame).await.is_err() {
            break;
        }
    }
    // Record what the client says, until it goes away or the test hangs up.
    loop {
        tokio::select! {
            () = stub.ws_kick.notified() => break,
            message = socket.recv() => match message {
                Some(Ok(WsMessage::Close(_)) | Err(_)) | None => break,
                Some(Ok(message)) => stub.ws_received.lock().unwrap().push(message),
            },
        }
    }
    stub.sockets_closed.fetch_add(1, Ordering::SeqCst);
}

/// Starts the stub on a loopback port; answers its base URL.
async fn serve(stub: &Arc<Stub>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/v2/ws", get(socket))
        .fallback(handle)
        .with_state(Arc::clone(stub));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{address}")
}

async fn remote(stub: &Arc<Stub>) -> HttpRemote {
    HttpRemote::new(&serve(stub).await).unwrap()
}

async fn wait_for(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never happened: {what}");
}

// ----- addresses ------------------------------------------------------------------------------------

#[test]
fn only_http_and_https_addresses_with_a_host_are_accepted() {
    for bad in [
        "ftp://example.com",
        "example.com",
        "http://",
        "https://:80",
        "",
    ] {
        assert!(HttpRemote::new(bad).is_err(), "{bad}");
    }
    let remote = HttpRemote::new("https://sync.example.com/kb/").unwrap();
    assert_eq!(remote.base_url(), "https://sync.example.com/kb");
}

#[test]
fn the_socket_address_follows_the_scheme() {
    assert_eq!(
        ws_url("https://h.example/kb").unwrap(),
        "wss://h.example/kb/v2/ws"
    );
    assert_eq!(
        ws_url("http://127.0.0.1:8787").unwrap(),
        "ws://127.0.0.1:8787/v2/ws"
    );
}

// ----- status mapping ---------------------------------------------------------------------------------

#[test]
fn statuses_map_to_sync_errors() {
    assert_eq!(status_error(401, None, None), SyncError::Unauthorized);
    assert_eq!(status_error(401, Some("idle"), None), SyncError::Idle);
    assert_eq!(status_error(403, None, None), SyncError::Forbidden);
    assert_eq!(
        status_error(403, Some("forbidden"), None),
        SyncError::Forbidden
    );
    assert_eq!(
        status_error(403, Some("signup_limit"), None),
        SyncError::Forbidden
    );
    assert_eq!(
        status_error(403, Some("authentication"), None),
        SyncError::Authentication
    );
    assert_eq!(status_error(404, None, None), SyncError::NotFound);
    assert_eq!(
        status_error(409, Some("conflict"), Some("exists".into())),
        SyncError::Conflict("exists".into())
    );
    assert!(matches!(
        status_error(409, None, None),
        SyncError::Conflict(_)
    ));
    assert_eq!(
        status_error(409, Some("rekey_needed"), None),
        SyncError::RekeyNeeded
    );
    assert_eq!(
        status_error(409, Some("stale_epoch"), None),
        SyncError::StaleEpoch
    );
    assert_eq!(
        status_error(409, Some("feed_closed"), None),
        SyncError::Closed
    );
    for status in [400, 413, 422] {
        assert!(
            matches!(status_error(status, None, None), SyncError::Protocol(_)),
            "{status}"
        );
    }
    assert!(
        matches!(status_error(429, None, None), SyncError::Unavailable(_)),
        "a 429 is never `Quota`"
    );
    assert_eq!(status_error(507, None, None), SyncError::Quota);
    for status in [500, 502, 503, 504] {
        assert!(
            matches!(status_error(status, None, None), SyncError::Unavailable(_)),
            "{status}"
        );
    }
    assert!(matches!(
        status_error(418, None, None),
        SyncError::Protocol(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn statuses_from_the_server_map_to_sync_errors() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let cases: [(u16, &str, SyncError); 17] = [
        (401, "", SyncError::Unauthorized),
        (401, r#"{"error":"idle"}"#, SyncError::Idle),
        (403, "", SyncError::Forbidden),
        (
            403,
            r#"{"error":"authentication"}"#,
            SyncError::Authentication,
        ),
        (403, r#"{"error":"signup_limit"}"#, SyncError::Forbidden),
        (404, "", SyncError::NotFound),
        (
            409,
            r#"{"error":"conflict","message":"the account already exists"}"#,
            SyncError::Conflict("the account already exists".into()),
        ),
        (409, r#"{"error":"rekey_needed"}"#, SyncError::RekeyNeeded),
        (409, r#"{"error":"stale_epoch"}"#, SyncError::StaleEpoch),
        (409, r#"{"error":"feed_closed"}"#, SyncError::Closed),
        (
            400,
            r#"{"message":"the log does not verify"}"#,
            SyncError::Protocol("the log does not verify".into()),
        ),
        (
            413,
            r#"{"message":"too large"}"#,
            SyncError::Protocol("too large".into()),
        ),
        (
            422,
            r#"{"message":"bad header"}"#,
            SyncError::Protocol("bad header".into()),
        ),
        (
            429,
            "",
            SyncError::Unavailable("the server asked to slow down".into()),
        ),
        (507, "", SyncError::Quota),
        (
            500,
            "internal details",
            SyncError::Unavailable("the server failed (status 500)".into()),
        ),
        (
            503,
            "",
            SyncError::Unavailable("the server failed (status 503)".into()),
        ),
    ];
    for (status, body, expected) in cases {
        stub.force(status, body);
        let error = remote.server_info().await.unwrap_err();
        assert_eq!(error, expected, "status {status}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_messages_are_shortened_and_cleaned() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let long = format!("line one\nline two\u{7}{}", "x".repeat(500));
    stub.force(
        409,
        &serde_json::json!({ "error": "conflict", "message": long }).to_string(),
    );
    let SyncError::Conflict(message) = remote.server_info().await.unwrap_err() else {
        panic!("not a conflict");
    };
    assert!(message.starts_with("line oneline two"), "{message}");
    assert_eq!(message.chars().count(), 200);
    assert!(!message.chars().any(char::is_control));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_server_is_unavailable() {
    // A port that was just free: nothing listens there.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let remote = HttpRemote::new(&format!("http://{address}")).unwrap();
    let error = remote.server_info().await.unwrap_err();
    assert!(matches!(error, SyncError::Unavailable(_)), "{error:?}");
    assert!(
        !error.to_string().contains(&address.to_string()),
        "no URL in the message"
    );
}

// ----- shapes ---------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_info_reads_the_epoch_from_hex() {
    let stub = Arc::new(Stub::default());
    let info = remote(&stub).await.server_info().await.unwrap();
    assert_eq!(info.protocol_version, 2);
    assert_eq!(info.server_epoch, EPOCH);
    assert_eq!(info.max_wire_bytes, MAX_WIRE_BYTES);
    assert_eq!(info.account_quota_bytes, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accounts_are_created_and_their_logs_exchanged_as_base64_json() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    assert_eq!(
        remote.fetch_log(account(), None).await.unwrap_err(),
        SyncError::NotFound
    );

    remote
        .create_account("invite", &[b"genesis".to_vec(), vec![0, 255]])
        .await
        .unwrap();
    let created = &stub.seen("/v2/accounts")[0];
    assert_eq!(created.method, "POST");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&created.body).unwrap(),
        serde_json::json!({ "inviteToken": "invite", "log": ["Z2VuZXNpcw==", "AP8="] })
    );
    assert_eq!(created.authorization, None);

    assert_eq!(
        remote.fetch_log(account(), None).await.unwrap(),
        [b"genesis".to_vec()]
    );
    remote
        .append_log(account(), &[b"added".to_vec()])
        .await
        .unwrap();
    let path = format!("/v2/accounts/{}/log", account());
    let appended = stub
        .seen(&path)
        .into_iter()
        .find(|seen| seen.method == "POST")
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&appended.body).unwrap(),
        serde_json::json!({ "entries": ["YWRkZWQ="] })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_is_opened_with_a_signed_challenge_and_used_as_a_bearer_token() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    assert!(!remote.is_authenticated(account(), device()));
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    assert!(remote.is_authenticated(account(), device()));
    assert!(!remote.is_authenticated(account(), DeviceId::from_bytes([9; 16])));

    let challenge = &stub.seen("/v2/auth/challenge")[0];
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&challenge.body).unwrap(),
        serde_json::json!({
            "accountId": account().to_string(),
            "deviceId": device().to_string(),
        })
    );
    let session: serde_json::Value =
        serde_json::from_slice(&stub.seen("/v2/auth/session")[0].body).unwrap();
    assert_eq!(session["nonce"], hex_encode(&NONCE));
    assert_eq!(session["signature"].as_str().unwrap().len(), 128);

    let (cursor, chain) = remote.append(feed(), b"sealed entry").await.unwrap();
    assert_eq!((cursor, chain), (42, [8; 32]));
    let sent = &stub.seen(&feed_path("/entries"))[0];
    assert_eq!(sent.authorization.as_deref(), Some("Bearer token-1"));
    assert_eq!(
        sent.content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(sent.body, b"sealed entry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_trait_s_borrowed_signer_opens_a_session_too() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let key = device_key();
    RemoteStore::authenticate(&remote, account(), device(), &|message: &[u8]| {
        kb_sync::sign_auth(message, &key)
    })
    .await
    .unwrap();
    remote.put_head(feed(), b"head").await.unwrap();
    assert_eq!(
        stub.seen(&feed_path("/heads"))[0].authorization.as_deref(),
        Some("Bearer token-1")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_signature_is_unauthorized() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let stranger = SigningKey::from_seed(&Secret32::from_bytes([1; 32]));
    let wrong: AuthSigner =
        Arc::new(move |message: &[u8]| Some(kb_sync::sign_auth(message, &stranger)));
    assert_eq!(
        remote
            .authenticate_with(account(), device(), wrong)
            .await
            .unwrap_err(),
        SyncError::Unauthorized
    );
    assert!(!remote.is_authenticated(account(), device()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signer_that_cannot_sign_sends_nothing() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let unable: AuthSigner = Arc::new(|_: &[u8]| None);
    assert_eq!(
        remote
            .authenticate_with(account(), device(), unable)
            .await
            .unwrap_err(),
        SyncError::Unauthorized
    );
    assert!(stub.seen("/v2/auth/session").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_session_is_dropped_so_a_new_one_can_be_opened() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    // The server forgets the session (expiry, restart).
    *stub.valid.lock().unwrap() = None;

    assert_eq!(
        remote.changes(feed(), 5, 200).await.unwrap_err(),
        SyncError::Unauthorized
    );
    assert!(
        !remote.is_authenticated(account(), device()),
        "a 401 drops the token: the driver authenticates again"
    );
    assert_eq!(
        stub.challenges.load(Ordering::SeqCst),
        1,
        "no retry of its own"
    );

    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    let page = remote.changes(feed(), 5, 200).await.unwrap();
    assert_eq!(page.changes, [(7, b"entry".to_vec())]);
    let tries = stub.seen(&feed_path("/changes"));
    assert_eq!(tries.len(), 2);
    assert_eq!(tries[0].authorization.as_deref(), Some("Bearer token-1"));
    assert_eq!(tries[1].authorization.as_deref(), Some("Bearer token-2"));
    assert_eq!(tries[1].query.as_deref(), Some("after=5&limit=200"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forbidden_call_keeps_the_session() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    stub.force(
        403,
        r#"{"error":"forbidden","message":"another account's entry"}"#,
    );
    assert_eq!(
        remote.heads(feed()).await.unwrap_err(),
        SyncError::Forbidden
    );
    assert!(
        remote.is_authenticated(account(), device()),
        "a 403 is not about the session"
    );
    *stub.forced.lock().unwrap() = None;
    assert_eq!(remote.heads(feed()).await.unwrap().len(), 2);
    assert_eq!(stub.challenges.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_session_a_call_is_refused_and_nothing_else_is_tried() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    assert_eq!(
        remote.heads(feed()).await.unwrap_err(),
        SyncError::Unauthorized
    );
    assert_eq!(stub.seen(&feed_path("/heads")).len(), 1);
    assert_eq!(stub.challenges.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heads_travel_as_base64_json_and_snapshots_may_be_missing() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    assert_eq!(
        remote.heads(feed()).await.unwrap(),
        [b"head one".to_vec(), b"head two".to_vec()]
    );
    assert_eq!(
        remote
            .get_snapshot(feed(), DocId::from_bytes([4; 16]))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        stub.seen(&feed_path(&format!(
            "/snapshots/{}",
            DocId::from_bytes([4; 16])
        )))
        .len(),
        1
    );
}

// ----- attachment chunks (ADR-0023) ----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunks_are_stored_read_and_deleted_with_a_session() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let hash = kb_types::ChunkId::from_bytes([0xcd; 32]);
    let path = feed_path(&format!("/chunks/{}", "cd".repeat(32)));

    // Without a session the server refuses, and nothing else is tried.
    assert_eq!(
        remote.delete_chunk(feed(), hash).await.unwrap_err(),
        SyncError::Unauthorized
    );
    let refused = stub.seen(&path);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].method, "DELETE");
    assert_eq!(refused[0].authorization, None);
    assert_eq!(stub.challenges.load(Ordering::SeqCst), 0);

    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    remote
        .put_chunk(feed(), hash, b"sealed chunk")
        .await
        .unwrap();
    let put = stub
        .seen(&path)
        .into_iter()
        .rfind(|seen| seen.method == "PUT")
        .unwrap();
    assert_eq!(put.authorization.as_deref(), Some("Bearer token-1"));
    assert_eq!(
        put.content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(put.body, b"sealed chunk");
    assert_eq!(
        remote.get_chunk(feed(), hash).await.unwrap().as_deref(),
        Some(&b"sealed chunk"[..])
    );

    // `DELETE /v2/feeds/{feed}/chunks/{hex}`: a 204 is success.
    remote.delete_chunk(feed(), hash).await.unwrap();
    let deleted = stub
        .seen(&path)
        .into_iter()
        .rfind(|seen| seen.method == "DELETE")
        .unwrap();
    assert_eq!(deleted.authorization.as_deref(), Some("Bearer token-1"));
    assert!(deleted.body.is_empty());
    assert_eq!(remote.get_chunk(feed(), hash).await.unwrap(), None);
    // Nothing stored under the hash: deleted already.
    remote.delete_chunk(feed(), hash).await.unwrap();
    assert_eq!(
        stub.seen(&path)
            .iter()
            .filter(|seen| seen.method == "DELETE")
            .count(),
        3
    );

    // The usual status mapping.
    stub.force(507, "");
    assert_eq!(
        remote
            .put_chunk(feed(), hash, b"sealed chunk")
            .await
            .unwrap_err(),
        SyncError::Quota
    );
    stub.force(503, "");
    assert!(matches!(
        remote.delete_chunk(feed(), hash).await.unwrap_err(),
        SyncError::Unavailable(_)
    ));
    stub.force(403, "");
    assert_eq!(
        remote.delete_chunk(feed(), hash).await.unwrap_err(),
        SyncError::Forbidden
    );
    assert!(
        remote.is_authenticated(account(), device()),
        "a 403 keeps the session"
    );
    stub.force(401, "");
    assert_eq!(
        remote.delete_chunk(feed(), hash).await.unwrap_err(),
        SyncError::Unauthorized
    );
    assert!(
        !remote.is_authenticated(account(), device()),
        "a 401 drops it: the driver authenticates again"
    );
}

// ----- caps -------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_upload_never_leaves_the_machine() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let too_big = vec![0u8; usize::try_from(MAX_WIRE_BYTES).unwrap() + 1];
    assert!(matches!(
        remote.append(feed(), &too_big).await.unwrap_err(),
        SyncError::Protocol(_)
    ));
    assert!(matches!(
        remote
            .put_chunk(feed(), kb_types::ChunkId::from_bytes([1; 32]), &too_big)
            .await
            .unwrap_err(),
        SyncError::Protocol(_)
    ));
    assert!(stub.requests.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_beyond_its_cap_is_refused() {
    let stub = Arc::new(Stub::default());
    let base = serve(&stub).await;
    let client = reqwest::Client::new();
    let response = client.get(format!("{base}/v2/big")).send().await.unwrap();
    assert!(matches!(
        read_capped(response, 1024).await.unwrap_err(),
        SyncError::Protocol(_)
    ));
    let response = client.get(format!("{base}/v2/big")).send().await.unwrap();
    assert_eq!(read_capped(response, 4096).await.unwrap().len(), 2048);
}

// ----- the realtime socket ---------------------------------------------------------------------------

/// The binary frame `1` as the layout of `kb_sync::v2::ws` documents it, written out by hand so that the
/// stub does not lean on the encoders it tests the decoders of.
fn entry_frame(feed: FeedId, prev: u64, cursor: u64, entry: &[u8]) -> Vec<u8> {
    let mut frame = vec![1];
    frame.extend_from_slice(feed.as_bytes());
    frame.extend_from_slice(&prev.to_le_bytes());
    frame.extend_from_slice(&cursor.to_le_bytes());
    frame.extend_from_slice(&[7; 32]);
    frame.extend_from_slice(entry);
    frame
}

fn presence_frame_bytes(feed: FeedId, envelope: &[u8]) -> Vec<u8> {
    let mut frame = vec![2];
    frame.extend_from_slice(feed.as_bytes());
    frame.extend_from_slice(envelope);
    frame
}

fn other_feed() -> FeedId {
    FeedId::from_bytes([9; 16])
}

async fn authenticated(stub: &Arc<Stub>) -> HttpRemote {
    let remote = remote(stub).await;
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    remote
}

async fn next_event(realtime: &mut Realtime) -> RealtimeEvent {
    tokio::time::timeout(Duration::from_secs(10), realtime.events.recv())
        .await
        .expect("an event within ten seconds")
        .expect("the socket is open")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_socket_delivers_what_the_server_sends_in_order_and_skips_what_it_does_not_understand()
{
    let stub = Arc::new(Stub::default());
    *stub.ws_script.lock().unwrap() = vec![
        WsMessage::Text(encode_changed(feed(), 7).unwrap().into()),
        WsMessage::Text(r#"{"type":"hello"}"#.into()),
        WsMessage::Binary(entry_frame(feed(), 7, 9, b"entry bytes").into()),
        WsMessage::Binary(vec![9, 9, 9].into()),
        WsMessage::Binary(presence_frame_bytes(other_feed(), b"sealed").into()),
        WsMessage::Text(encode_changed(other_feed(), 12).unwrap().into()),
    ];
    let remote = authenticated(&stub).await;
    let mut realtime = remote.open_realtime().unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    assert_eq!(
        next_event(&mut realtime).await,
        RealtimeEvent::Changed {
            feed: feed(),
            cursor: 7
        }
    );
    assert_eq!(
        next_event(&mut realtime).await,
        RealtimeEvent::Entry(InlineEntry {
            feed: feed(),
            prev_cursor: 7,
            cursor: 9,
            chain_after: [7; 32],
            entry: b"entry bytes".to_vec(),
        }),
        "the unknown text frame was skipped"
    );
    assert_eq!(
        next_event(&mut realtime).await,
        RealtimeEvent::Presence(PresenceFrame {
            feed: other_feed(),
            envelope: b"sealed".to_vec(),
        }),
        "the undecodable binary frame was skipped"
    );
    assert_eq!(
        next_event(&mut realtime).await,
        RealtimeEvent::Changed {
            feed: other_feed(),
            cursor: 12
        }
    );

    // Nobody listens any more: the socket is closed.
    drop(realtime.commands);
    wait_for("the socket closed", || {
        stub.sockets_closed.load(Ordering::SeqCst) == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_largest_inline_entry_fits_in_one_message() {
    let stub = Arc::new(Stub::default());
    let entry = vec![0xab; INLINE_ENTRY_MAX_BYTES];
    *stub.ws_script.lock().unwrap() =
        vec![WsMessage::Binary(entry_frame(feed(), 0, 1, &entry).into())];
    let remote = authenticated(&stub).await;
    let mut realtime = remote.open_realtime().unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    match next_event(&mut realtime).await {
        RealtimeEvent::Entry(inline) => assert_eq!(inline.entry, entry),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_subscription_and_presence_go_out_as_the_frames_of_the_protocol() {
    let stub = Arc::new(Stub::default());
    let remote = authenticated(&stub).await;
    let mut realtime = remote.open_realtime().unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    let feeds = vec![feed(), other_feed()];
    realtime
        .commands
        .send(RealtimeCommand::Subscribe(feeds.clone()))
        .await
        .unwrap();
    let frame = PresenceFrame {
        feed: other_feed(),
        envelope: b"sealed by the core".to_vec(),
    };
    realtime
        .commands
        .send(RealtimeCommand::Presence(frame.clone()))
        .await
        .unwrap();
    wait_for("both frames arrived", || {
        stub.ws_received.lock().unwrap().len() == 2
    })
    .await;
    let received = stub.ws_received.lock().unwrap().clone();
    assert_eq!(
        received,
        vec![
            WsMessage::Text(encode_subscribe(&feeds).unwrap().into()),
            WsMessage::Binary(encode_presence_frame(&frame).unwrap().into()),
        ]
    );
    // A frame over the protocol's limit is not sent (the handle refuses it before, too).
    realtime
        .commands
        .send(RealtimeCommand::Presence(PresenceFrame {
            feed: other_feed(),
            envelope: vec![0; kb_types::sharing::PRESENCE_MAX_FRAME_BYTES],
        }))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stub.ws_received.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_socket_reconnects_after_the_server_hangs_up_and_subscribes_again_by_itself() {
    let stub = Arc::new(Stub::default());
    let remote = authenticated(&stub).await;
    let mut realtime = remote.open_realtime().unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    let feeds = vec![feed(), other_feed()];
    realtime
        .commands
        .send(RealtimeCommand::Subscribe(feeds.clone()))
        .await
        .unwrap();
    wait_for("the subscription arrived", || {
        stub.ws_received.lock().unwrap().len() == 1
    })
    .await;

    stub.ws_kick.notify_one();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Disconnected);
    // Backoff of a second, then the same connection again; the driver says nothing more.
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    assert!(stub.sockets_opened.load(Ordering::SeqCst) >= 2);
    wait_for("the subscription came again", || {
        stub.ws_received.lock().unwrap().len() == 2
    })
    .await;
    let received = stub.ws_received.lock().unwrap().clone();
    assert_eq!(
        received[0], received[1],
        "the latest subscription, sent after the reconnection"
    );

    // A new subscription replaces it for the connections after that.
    let only = vec![other_feed()];
    realtime
        .commands
        .send(RealtimeCommand::Subscribe(only.clone()))
        .await
        .unwrap();
    wait_for("the replacement arrived", || {
        stub.ws_received.lock().unwrap().len() == 3
    })
    .await;
    stub.ws_kick.notify_one();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Disconnected);
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    wait_for("the replacement came again", || {
        stub.ws_received.lock().unwrap().len() == 4
    })
    .await;
    assert_eq!(
        stub.ws_received.lock().unwrap()[3],
        WsMessage::Text(encode_subscribe(&only).unwrap().into())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscription_made_while_the_socket_was_down_is_sent_when_it_comes_up() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    // No session yet: no connection, but the driver may already say what it wants.
    let mut realtime = remote.open_realtime().unwrap();
    let feeds = vec![feed()];
    realtime
        .commands
        .send(RealtimeCommand::Subscribe(feeds.clone()))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), realtime.events.recv())
            .await
            .is_err()
    );
    assert_eq!(stub.sockets_opened.load(Ordering::SeqCst), 0);
    // With a session the next attempt connects, subscribing first and saying `Connected` after.
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    // The stub records what it reads a moment after the client sent it.
    wait_for("the subscription arrived", || {
        !stub.ws_received.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        stub.ws_received.lock().unwrap().clone(),
        vec![WsMessage::Text(encode_subscribe(&feeds).unwrap().into())]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_the_server_refuses_is_an_authentication_failure_not_a_retry_loop() {
    let stub = Arc::new(Stub::default());
    let remote = authenticated(&stub).await;
    // The server forgot the session: the upgrade is answered with 401.
    *stub.valid.lock().unwrap() = Some("another".to_owned());
    let mut realtime = remote.open_realtime().unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Refused);
    assert!(
        !remote.is_authenticated(account(), device()),
        "a 401 drops the session, so the driver authenticates again"
    );
    // Neither that token nor the lack of one is tried again: no attempt, no event, however long it waits
    // (a retry with backoff would have come back after one second, and again after three).
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert_eq!(stub.ws_attempts.load(Ordering::SeqCst), 1);
    assert!(realtime.events.try_recv().is_err());
    // A new session connects at once.
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    assert_eq!(stub.ws_attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_the_server_turns_away_is_told_so_once_and_the_token_is_not_tried_again() {
    let stub = Arc::new(Stub::default());
    let remote = authenticated(&stub).await;
    stub.ws_forbid.store(true, Ordering::SeqCst);
    let mut realtime = remote.open_realtime().unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Refused);
    // A 403 is no reason to forget the session (authenticating again would not help), and no reason to try
    // the same token again either.
    assert!(remote.is_authenticated(account(), device()));
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert_eq!(stub.ws_attempts.load(Ordering::SeqCst), 1);
    assert!(realtime.events.try_recv().is_err());
    // The server lets it in again with a new session.
    stub.ws_forbid.store(false, Ordering::SeqCst);
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_close_followed_by_a_refused_reconnection_ends_in_one_refusal() {
    // The server closes the socket of a device it revoked and refuses it when it comes back.
    let stub = Arc::new(Stub::default());
    let remote = authenticated(&stub).await;
    let mut realtime = remote.open_realtime().unwrap();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Connected);
    stub.ws_forbid.store(true, Ordering::SeqCst);
    stub.ws_kick.notify_one();
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Disconnected);
    assert_eq!(next_event(&mut realtime).await, RealtimeEvent::Refused);
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert_eq!(
        stub.ws_attempts.load(Ordering::SeqCst),
        2,
        "the open one and the refused one"
    );
    assert!(realtime.events.try_recv().is_err());
}

// ----- pairing mailboxes, recovery blobs, idle devices (ADR-0022) ------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mailbox_is_created_read_written_and_closed_without_a_session() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;

    let (id, expires_at) = remote.pair_create(b"offer").await.unwrap();
    assert_eq!((id, expires_at), (MAILBOX, EXPIRES_AT));
    let created = &stub.seen("/v2/pair")[0];
    assert_eq!(created.method, "POST");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&created.body).unwrap(),
        serde_json::json!({ "offer": "b2ZmZXI=" })
    );
    assert_eq!(created.authorization, None);

    remote
        .pair_post(id, kb_sync::PAIRING_ROLE_SPONSOR, b"answer")
        .await
        .unwrap();
    let path = format!("/v2/pair/{}/messages", hex_encode(&MAILBOX));
    let posted = stub
        .seen(&path)
        .into_iter()
        .find(|seen| seen.method == "POST")
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&posted.body).unwrap(),
        serde_json::json!({ "role": "sponsor", "bytes": "YW5zd2Vy" })
    );

    // `after` counts what the caller has: from 1, the answer only.
    let page = remote.pair_messages(id, 1, 25).await.unwrap();
    assert_eq!(
        page.messages,
        [kb_sync::PairingMessage {
            seq: 1,
            role: kb_sync::PAIRING_ROLE_SPONSOR,
            bytes: b"answer".to_vec(),
        }]
    );
    assert_eq!(page.expires_at, EXPIRES_AT);
    assert!(!page.closed);
    let polled = stub
        .seen(&path)
        .into_iter()
        .find(|seen| seen.method == "GET")
        .unwrap();
    assert_eq!(polled.query.as_deref(), Some("after=1&wait=25"));

    remote.pair_close(id).await.unwrap();
    let page = remote.pair_messages(id, 0, 99).await.unwrap();
    assert!(page.closed);
    assert_eq!(page.messages.len(), 2, "a closed mailbox stays readable");
    assert!(
        stub.seen(&path)
            .iter()
            .any(|seen| seen.query.as_deref() == Some("after=0&wait=25")),
        "the wait is capped at PAIRING_POLL_SECONDS"
    );
    assert!(matches!(
        remote
            .pair_post(id, kb_sync::PAIRING_ROLE_JOINER, b"late")
            .await
            .unwrap_err(),
        SyncError::Conflict(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_mailbox_is_not_found_and_oversized_messages_never_leave() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let other = [9; 6];
    assert_eq!(
        remote.pair_messages(other, 0, 1).await.unwrap_err(),
        SyncError::NotFound
    );
    assert_eq!(
        remote.pair_post(other, 0, b"x").await.unwrap_err(),
        SyncError::NotFound
    );
    let requests = stub.requests.lock().unwrap().len();
    let too_big = vec![0u8; kb_types::devices::PAIRING_MAX_MESSAGE_BYTES + 1];
    assert!(matches!(
        remote.pair_create(&too_big).await.unwrap_err(),
        SyncError::Protocol(_)
    ));
    assert!(matches!(
        remote.pair_post(MAILBOX, 0, &too_big).await.unwrap_err(),
        SyncError::Protocol(_)
    ));
    assert!(matches!(
        remote.pair_post(MAILBOX, 7, b"x").await.unwrap_err(),
        SyncError::Protocol(_)
    ));
    assert_eq!(stub.requests.lock().unwrap().len(), requests);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_blobs_are_stored_with_a_session_and_read_without_one() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    let id = [0xab; 32];
    assert_eq!(remote.recovery_get(id).await.unwrap(), None);
    assert_eq!(
        remote.recovery_put(id, b"blob").await.unwrap_err(),
        SyncError::Unauthorized,
        "storing needs a session"
    );

    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();
    remote.recovery_put(id, b"blob").await.unwrap();
    let path = format!("/v2/recovery/{}", "ab".repeat(32));
    let put = stub
        .seen(&path)
        .into_iter()
        .rfind(|seen| seen.method == "PUT")
        .unwrap();
    assert_eq!(put.authorization.as_deref(), Some("Bearer token-1"));
    assert_eq!(
        put.content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(put.body, b"blob");

    let fresh = HttpRemote::new(remote.base_url()).unwrap();
    assert_eq!(
        fresh.recovery_get(id).await.unwrap().as_deref(),
        Some(&b"blob"[..])
    );

    remote.recovery_delete(id).await.unwrap();
    assert_eq!(remote.recovery_get(id).await.unwrap(), None);
    // Nothing left under the id: deleted already.
    remote.recovery_delete(id).await.unwrap();

    let too_big = vec![0u8; kb_types::devices::RECOVERY_BLOB_MAX_BYTES + 1];
    assert!(matches!(
        remote.recovery_put(id, &too_big).await.unwrap_err(),
        SyncError::Protocol(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_device_is_refused_and_says_so() {
    let stub = Arc::new(Stub::default());
    let remote = remote(&stub).await;
    assert_eq!(
        remote.server_info().await.unwrap().idle_device_days,
        Some(90)
    );
    stub.idle.store(true, Ordering::SeqCst);
    assert_eq!(
        remote
            .authenticate_with(account(), device(), signer())
            .await
            .unwrap_err(),
        SyncError::Idle
    );
    assert!(!remote.is_authenticated(account(), device()));
    assert!(stub.seen("/v2/auth/session").is_empty());

    // Re-admitted: authentication works again.
    stub.idle.store(false, Ordering::SeqCst);
    remote
        .authenticate_with(account(), device(), signer())
        .await
        .unwrap();

    // An ordinary refusal is not an idle one.
    let stranger = SigningKey::from_seed(&Secret32::from_bytes([1; 32]));
    let wrong: AuthSigner =
        Arc::new(move |message: &[u8]| Some(kb_sync::sign_auth(message, &stranger)));
    assert_eq!(
        remote
            .authenticate_with(account(), device(), wrong)
            .await
            .unwrap_err(),
        SyncError::Unauthorized
    );
}
