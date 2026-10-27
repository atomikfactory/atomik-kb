//! The server's state, bearer authentication, the handlers the `/v2` routes share (the device log,
//! the challenge and the session, the notification socket) and the router (ADR-0021 §5-6,
//! ADR-0045). The feed routes themselves are in `api_v2.rs`. Every write is checked against the
//! wire object's plaintext header (`kb_sync::v2::peek_*`) and the device log (`kb_keys`); the server
//! never reads past a header.

use std::sync::{Arc, Mutex, PoisonError};

use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use kb_crypto::{Signature, VerifyingKey};
use kb_keys::LogEntry;
use kb_sync::SyncError;
use kb_types::sync::{CHALLENGE_TTL_SECONDS, MAX_WIRE_BYTES, SESSION_TTL_SECONDS};
use kb_types::{AccountId, ChunkId, Clock, DeviceId, DocId};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::watch;
use tower_http::LatencyUnit;
use tower_http::trace::{DefaultOnFailure, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::codec::{hex, unbase64, unhex};
use crate::error::HttpError;
use crate::limits::RateLimiter;
use crate::store::{ChallengeRow, SessionRow, Store};

fn max_wire_bytes() -> usize {
    usize::try_from(MAX_WIRE_BYTES).unwrap_or(usize::MAX)
}

/// Settings the handlers need.
pub(crate) struct Settings {
    pub(crate) invite_tokens: Vec<String>,
    pub(crate) account_quota_bytes: u64,
    /// `KB_SERVER_IDLE_DEVICE_DAYS`: a device whose last session is older is refused.
    pub(crate) idle_device_days: Option<u32>,
    /// `KB_SERVER_TRUSTED_PROXIES`: peers whose forwarding headers name the client.
    pub(crate) trusted_proxies: Vec<crate::limits::TrustedProxy>,
    /// `KB_SERVER_SHARE_SIGNUPS`: accounts share invites may create (0: none).
    pub(crate) share_signups: u32,
    /// `KB_SERVER_SHARE_SIGNUPS_PER_ACCOUNT`: accounts one inviter's invites may create.
    pub(crate) share_signups_per_account: u32,
    /// `KB_SERVER_VIEWER_DIR`: the viewer's build, served at `/view` (`viewer.rs`).
    pub(crate) viewer_dir: Option<std::path::PathBuf>,
}

impl Settings {
    /// The idle limit in ms, when one is set.
    pub(crate) fn idle_limit_ms(&self) -> Option<i64> {
        self.idle_device_days
            .map(|days| i64::from(days).saturating_mul(86_400_000))
    }
}

/// Shared state of one server process.
pub(crate) struct AppState {
    pub(crate) store: Arc<Mutex<Store>>,
    pub(crate) server_epoch: [u8; 16],
    pub(crate) settings: Settings,
    pub(crate) clock: Arc<dyn Clock>,
    /// The notices the WebSockets deliver (stored items, presence frames) and their counters.
    pub(crate) hub: crate::ws::Hub,
    /// Bumped on every pairing mailbox change; long-polls wait on it.
    pub(crate) mailbox_events: watch::Sender<u64>,
    /// Rate limits of the session-free pairing and recovery endpoints.
    pub(crate) limiter: RateLimiter,
}

impl AppState {
    pub(crate) fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Runs a storage closure on the blocking pool.
    pub(crate) async fn db<T, F>(&self, f: F) -> Result<T, HttpError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Store) -> Result<T, HttpError> + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let mut guard = store.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut guard)
        })
        .await
        .map_err(|_| HttpError::internal("a storage task failed"))?
    }

    pub(crate) fn invite_ok(&self, token: &str) -> bool {
        // Every configured token is compared in full, so timing says nothing about
        // which one (or how much of one) matched.
        self.settings
            .invite_tokens
            .iter()
            .fold(false, |found, valid| {
                found | ct_eq(valid.as_bytes(), token.as_bytes())
            })
    }

    /// Resolves a bearer token to its session, refusing expired or unknown sessions
    /// and devices that are no longer active in the account's log.
    pub(crate) async fn authenticate(&self, token: &str) -> Result<Authed, HttpError> {
        let token_hash = kb_crypto::hash(token.as_bytes());
        let now = self.now_ms();
        let authed = self
            .db(move |store| {
                let session = store
                    .session(&token_hash, now)?
                    .ok_or_else(|| HttpError::unauthorized("the session is unknown or expired"))?;
                let state = store
                    .log_state(session.account_id)?
                    .ok_or_else(|| HttpError::unauthorized("the account is unknown"))?;
                let record = state
                    .active(session.device_id)
                    .ok_or_else(|| HttpError::unauthorized("the device is not active"))?;
                Ok(Authed {
                    account: session.account_id,
                    device: session.device_id,
                    signing: record.keys.signing,
                    expires_at: session.expires_at,
                })
            })
            .await?;
        let span = tracing::Span::current();
        span.record("account", short_id(&authed.account).as_str());
        span.record("device", short_id(&authed.device).as_str());
        Ok(authed)
    }
}

/// Constant-time comparison for equal lengths (the length itself is not secret).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// An authenticated request: the session's account and device and the device's
/// current signing key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Authed {
    pub(crate) account: AccountId,
    pub(crate) device: DeviceId,
    pub(crate) signing: VerifyingKey,
    pub(crate) expires_at: i64,
}

/// The token of an `Authorization: Bearer <token>` header.
pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

impl FromRequestParts<Arc<AppState>> for Authed {
    type Rejection = HttpError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(&parts.headers)
            .ok_or_else(|| HttpError::unauthorized("a bearer token is required"))?;
        state.authenticate(token).await
    }
}

// ----- helpers ------------------------------------------------------------------------------

pub(crate) fn body_bytes(body: Result<Bytes, BytesRejection>) -> Result<Bytes, HttpError> {
    let bytes = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            HttpError::too_large()
        } else {
            HttpError::bad_request("bad_request", "the body could not be read")
        }
    })?;
    if bytes.len() > max_wire_bytes() {
        return Err(HttpError::too_large());
    }
    Ok(bytes)
}

pub(crate) fn json_body<T: serde::de::DeserializeOwned>(
    body: Result<Bytes, BytesRejection>,
) -> Result<T, HttpError> {
    let bytes = body_bytes(body)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| HttpError::bad_request("bad_request", "the body is not the expected JSON"))
}

pub(crate) fn decode_log(entries: &[String]) -> Result<Vec<LogEntry>, HttpError> {
    entries
        .iter()
        .map(|text| {
            let bytes = unbase64(text).ok_or_else(|| {
                HttpError::bad_request("encoding", "a log entry is not valid base64")
            })?;
            LogEntry::decode(&bytes)
                .map_err(|_| HttpError::bad_request("invalid_log", "a log entry is malformed"))
        })
        .collect()
}

pub(crate) fn parse_account(text: &str) -> Result<AccountId, HttpError> {
    text.parse()
        .map_err(|_| HttpError::bad_request("bad_request", "the account id is malformed"))
}

pub(crate) fn parse_doc(text: &str) -> Result<DocId, HttpError> {
    text.parse()
        .map_err(|_| HttpError::bad_request("bad_request", "the document id is malformed"))
}

pub(crate) fn parse_chunk(text: &str) -> Result<ChunkId, HttpError> {
    ChunkId::from_hex(text)
        .map_err(|_| HttpError::bad_request("bad_request", "the chunk hash is malformed"))
}

pub(crate) fn random<const N: usize>() -> Result<[u8; N], HttpError> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|_| HttpError::internal("no randomness available"))?;
    Ok(out)
}

pub(crate) fn binary(bytes: Vec<u8>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )],
        bytes,
    )
        .into_response()
}

pub(crate) fn ttl_ms(seconds: u64) -> i64 {
    i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX)
}

/// Checks the device signature of an entry, keyring entry or snapshot without any
/// decryption key (`kb_sync::v2::verify_signature`).
pub(crate) fn verify_envelope(bytes: &[u8], key: &VerifyingKey) -> Result<(), HttpError> {
    kb_sync::v2::verify_signature(bytes, key).map_err(|error| match error {
        SyncError::Authentication => HttpError::bad_signature(),
        _ => HttpError::bad_request("encoding", "the body is not a signed envelope"),
    })
}

// ----- accounts and the device log ---------------------------------------------------------

#[derive(Deserialize)]
struct LogBody {
    /// `log` is accepted too (the field name of account creation).
    #[serde(alias = "log")]
    entries: Vec<String>,
}

/// Appends device log entries. The entries authenticate themselves (each is signed by
/// an active device or the recovery key and chained to the stored log), so no session
/// is needed and a bearer, if sent, is not looked at: a device that joined through the
/// recovery kit must publish its `DeviceAdded` before it can open a session.
pub(crate) async fn append_log(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, HttpError> {
    let account = parse_account(&id)?;
    let body: LogBody = json_body(body)?;
    let entries = decode_log(&body.entries)?;
    let hub = state.hub.clone();
    state
        .db(move |store| {
            let revoked = store.append_log(account, entries)?;
            // Still under the store's lock, after the commit that deleted the devices' sessions:
            // their open sockets end now.
            for device in revoked {
                hub.publish_revoked(account, device);
            }
            Ok(())
        })
        .await?;
    tracing::Span::current().record("account", short_id(&account).as_str());
    Ok(StatusCode::NO_CONTENT)
}

// ----- authentication -----------------------------------------------------------------------

/// Refuses a device whose last session is older than the idle limit (a device never
/// seen counts as seen now).
fn check_idle(
    store: &Store,
    limit_ms: Option<i64>,
    account: AccountId,
    device: DeviceId,
    now: i64,
) -> Result<(), HttpError> {
    let Some(limit_ms) = limit_ms else {
        return Ok(());
    };
    match store.last_seen(account, device)? {
        Some(seen) if now.saturating_sub(seen) > limit_ms => Err(HttpError::idle()),
        _ => Ok(()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeBody {
    account_id: AccountId,
    device_id: DeviceId,
}

pub(crate) async fn challenge(
    State(state): State<Arc<AppState>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let body: ChallengeBody = json_body(body)?;
    let (account, device) = (body.account_id, body.device_id);
    let nonce: [u8; 32] = random()?;
    let now = state.now_ms();
    let row = ChallengeRow {
        account_id: account,
        device_id: device,
        expires_at: now.saturating_add(ttl_ms(CHALLENGE_TTL_SECONDS)),
    };
    let idle_limit = state.settings.idle_limit_ms();
    state
        .db(move |store| {
            let log = store
                .log_state(account)?
                .ok_or_else(|| HttpError::not_found("unknown account"))?;
            if log.active(device).is_none() {
                return Err(HttpError::unauthorized(
                    "the device is not active in the account's log",
                ));
            }
            check_idle(store, idle_limit, account, device, now)?;
            store.put_challenge(&nonce, row, now)
        })
        .await?;
    Ok(Json(json!({
        "nonce": hex(&nonce),
        "serverEpoch": hex(&state.server_epoch),
    }))
    .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionBody {
    account_id: AccountId,
    device_id: DeviceId,
    nonce: String,
    signature: String,
}

pub(crate) async fn session(
    State(state): State<Arc<AppState>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let body: SessionBody = json_body(body)?;
    let nonce = unhex::<32>(&body.nonce)
        .ok_or_else(|| HttpError::bad_request("bad_request", "the nonce is malformed"))?;
    let signature = unhex::<64>(&body.signature)
        .and_then(|bytes| Signature::from_bytes(&bytes).ok())
        .ok_or_else(|| HttpError::bad_request("bad_request", "the signature is malformed"))?;
    let (account, device) = (body.account_id, body.device_id);
    let token = hex(&random::<32>()?);
    let token_hash = kb_crypto::hash(token.as_bytes());
    let now = state.now_ms();
    let expires_at = now.saturating_add(ttl_ms(SESSION_TTL_SECONDS));
    let epoch = state.server_epoch;
    let idle_limit = state.settings.idle_limit_ms();
    state
        .db(move |store| {
            let challenge = store
                .take_challenge(&nonce)?
                .ok_or_else(|| HttpError::unauthorized("the challenge is unknown or used"))?;
            if challenge.expires_at <= now {
                return Err(HttpError::unauthorized("the challenge expired"));
            }
            if challenge.account_id != account || challenge.device_id != device {
                return Err(HttpError::unauthorized(
                    "the challenge was issued to another device",
                ));
            }
            let log = store
                .log_state(account)?
                .ok_or_else(|| HttpError::unauthorized("the account is unknown"))?;
            let record = log
                .active(device)
                .ok_or_else(|| HttpError::unauthorized("the device is not active"))?;
            kb_sync::verify_auth(
                &kb_sync::auth_message(&nonce, &epoch),
                &signature,
                &record.keys.signing,
            )
            .map_err(|_| HttpError::unauthorized("the signature does not verify"))?;
            check_idle(store, idle_limit, account, device, now)?;
            store.put_session(
                &token_hash,
                SessionRow {
                    account_id: account,
                    device_id: device,
                    expires_at,
                },
            )?;
            store.device_seen(account, device, now)
        })
        .await?;
    let span = tracing::Span::current();
    span.record("account", short_id(&account).as_str());
    span.record("device", short_id(&device).as_str());
    Ok(Json(json!({ "token": token, "expiresAt": expires_at })).into_response())
}

// ----- WebSocket ----------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct WsQuery {
    token: Option<String>,
}

pub(crate) async fn websocket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    query: Result<Query<WsQuery>, QueryRejection>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, HttpError> {
    let query_token = query.ok().and_then(|Query(q)| q.token);
    let token = bearer(&headers)
        .map(str::to_owned)
        .or(query_token)
        .ok_or_else(|| HttpError::unauthorized("a bearer token is required"))?;
    let authed = state.authenticate(&token).await?;
    let upgrade = upgrade.map_err(|_| {
        HttpError::bad_request("bad_request", "this endpoint needs a WebSocket upgrade")
    })?;
    // Listen before answering, so nothing appended during the handshake is missed. A client sends
    // `subscribe` frames and presence frames, which are small: a larger message ends the connection.
    let listen = state.hub.listen();
    let limit = kb_sync::v2::ws::CLIENT_MESSAGE_MAX_BYTES;
    Ok(upgrade
        .max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |socket| {
            crate::ws::serve(
                socket,
                listen,
                state,
                authed.account,
                authed.device,
                authed.expires_at,
            )
        }))
}

// ----- router -------------------------------------------------------------------------------

/// The request path as logged: every id in it (a feed, an account, an invite, a document, a chunk
/// hash, a recovery id: a segment of 32 or more hex digits and dashes) is cut to its first 8 hex
/// characters. Query strings never reach the log.
pub(crate) fn loggable_path(path: &str) -> std::borrow::Cow<'_, str> {
    let secret_like = |segment: &str| {
        segment.len() >= 32 && segment.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    };
    if !path.split('/').any(secret_like) {
        return std::borrow::Cow::Borrowed(path);
    }
    let cut: Vec<String> = path
        .split('/')
        .map(|segment| {
            if secret_like(segment) {
                let shown: String = segment.chars().take(8).collect();
                format!("{shown}...")
            } else {
                segment.to_owned()
            }
        })
        .collect();
    std::borrow::Cow::Owned(cut.join("/"))
}

/// The first 8 hex characters of an id (the only form of an id the log may hold).
pub(crate) fn short_id<T: core::fmt::Display>(id: &T) -> String {
    id.to_string().chars().take(8).collect()
}

/// The router (`/v2` and the join page) with request tracing (method, path, status, latency and, once
/// authenticated, account and device ids; never bodies, query strings or headers).
pub(crate) fn router(state: Arc<AppState>) -> Router {
    let trace = TraceLayer::new_for_http()
        .make_span_with(|request: &Request<Body>| {
            tracing::info_span!(
                "request",
                method = %request.method(),
                path = %loggable_path(request.uri().path()),
                account = tracing::field::Empty,
                device = tracing::field::Empty,
                feed = tracing::field::Empty,
            )
        })
        .on_request(())
        .on_response(
            DefaultOnResponse::new()
                .level(Level::INFO)
                .latency_unit(LatencyUnit::Millis),
        )
        .on_failure(DefaultOnFailure::new().level(Level::WARN));
    Router::new()
        .merge(crate::api_v2::routes())
        .layer(DefaultBodyLimit::max(max_wire_bytes()))
        .with_state(state)
        .layer(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_ids_and_chunk_hashes_are_logged_by_their_first_eight_hex_digits_only() {
        let id = "ab".repeat(32);
        assert_eq!(
            loggable_path(&format!("/v2/recovery/{id}")),
            "/v2/recovery/abababab..."
        );
        assert_eq!(loggable_path("/v2/recovery/short"), "/v2/recovery/short");
        assert_eq!(
            loggable_path(&format!(
                "/v2/feeds/{}/chunks/{}",
                "01".repeat(16),
                "0f".repeat(32)
            )),
            "/v2/feeds/01010101.../chunks/0f0f0f0f..."
        );
        assert_eq!(
            loggable_path("/v2/pair/0123456789ab"),
            "/v2/pair/0123456789ab"
        );
        assert_eq!(loggable_path("/v2/server"), "/v2/server");
    }
}
