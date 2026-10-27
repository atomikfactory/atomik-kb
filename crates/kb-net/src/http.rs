//! [`HttpRemote`]: `kb-server` over HTTP(S) (reqwest) and its WebSocket (tokio-tungstenite, `ws`).
//!
//! Wire shapes (protocol v2, ADR-0045, [`v2`]): metadata is camelCase JSON ([`crate::codec`]),
//! payloads are `application/octet-stream`. The bearer token from `POST /v2/auth/session` stays
//! inside the remote and goes out in `Authorization` only. A 401 drops it: the call
//! answers `Unauthorized` and [`HttpRemote::is_authenticated`] turns false, which is how
//! the driver knows to authenticate again and retry once. A 403 (another account or
//! device, a role that is too low, a signature that does not verify) keeps the session:
//! authenticating again would not help.
//!
//! The device log endpoints, `POST /v2/accounts` and `GET /v2/server` need no session
//! (a device that joined through the kit publishes its own `DeviceAdded` before it can
//! authenticate); the token is sent with the log calls when one is held.
//!
//! The status mapping is [`status_error`] (the table of `milestone-13.md` 1.1). Every request has a
//! 30 s timeout; no answer is read past its cap (`MAX_WIRE_BYTES` for single objects) and no
//! upload larger than `MAX_WIRE_BYTES` leaves the machine.
//!
//! Nothing here logs a token, a nonce, a signature, a body or a server message.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use kb_crypto::Signature;
use kb_sync::SyncError;
use kb_sync::v2::RemoteStore;
use kb_types::devices::{PAIRING_MAX_MESSAGE_BYTES, RECOVERY_BLOB_MAX_BYTES};
use kb_types::sync::MAX_WIRE_BYTES;
use kb_types::{AccountId, DeviceId};
use reqwest::{RequestBuilder, Response, StatusCode};

use crate::NetError;
use crate::codec::{
    ChallengeJson, ChallengeRequestJson, SessionJson, SessionRequestJson, hex_decode, hex_encode,
};

/// Signs an authentication message ([`kb_sync::auth_message`]) with the device key;
/// `None` when the device cannot sign right now (the workspace is locked, no key).
pub type AuthSigner = Arc<dyn Fn(&[u8]) -> Option<Signature> + Send + Sync>;

/// Every request, connection included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Establishing a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A page of changes: `MAX_WIRE_BYTES` of entries plus the page framing. The server keeps
/// a page within this (always at least one entry).
pub const MAX_CHANGES_PAGE_BYTES: u64 = MAX_WIRE_BYTES + 1024 * 1024;
/// Every document's latest snapshot in one answer (bootstrap). ADR-0021 has no paging for
/// it, so the cap is far above one object; see the milestone retrospective.
pub const MAX_SNAPSHOTS_PAGE_BYTES: u64 = 512 * 1024 * 1024;
/// JSON answers (logs, heads, challenges, sessions).
const MAX_JSON_BYTES: u64 = MAX_WIRE_BYTES;
/// What is read of an error answer, and what is kept of its message.
const MAX_ERROR_BYTES: u64 = 4 * 1024;
const MAX_ERROR_MESSAGE: usize = 200;
/// Reconnection delays of the WebSocket (`ws`).
const WS_BACKOFF_MIN: Duration = Duration::from_secs(1);
const WS_BACKOFF_MAX: Duration = Duration::from_secs(60);
const OCTET_STREAM: &str = "application/octet-stream";

/// The session state shared with the WebSocket task.
#[derive(Default)]
struct Session {
    /// The bearer token. Never logged, never put in a URL.
    token: Option<String>,
    /// Who the token was issued to.
    identity: Option<(AccountId, DeviceId)>,
}

type SharedSession = Arc<Mutex<Session>>;

fn lock(session: &SharedSession) -> std::sync::MutexGuard<'_, Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `kb-server` over HTTP(S) and a WebSocket.
pub struct HttpRemote {
    base_url: String,
    client: reqwest::Client,
    session: SharedSession,
}

impl HttpRemote {
    /// A client for the server at `base_url` (`http(s)://host[:port]`, an optional path
    /// prefix allowed; a trailing slash is ignored).
    pub fn new(base_url: &str) -> Result<Self, NetError> {
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err(NetError::Transport(
                "the server address must start with http:// or https://".to_owned(),
            ));
        }
        let parsed = reqwest::Url::parse(base_url)
            .map_err(|_| NetError::Transport("the server address is not a valid URL".to_owned()))?;
        if parsed.host_str().is_none_or(str::is_empty) {
            return Err(NetError::Transport(
                "the server address has no host".to_owned(),
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("atomik-kb/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| NetError::Transport("the HTTP client could not be created".to_owned()))?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            client,
            session: SharedSession::default(),
        })
    }

    /// The configured server address.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Whether a session for this account and device is held. It may still have expired:
    /// the next call finds out, answers `Unauthorized` and drops it.
    #[must_use]
    pub fn is_authenticated(&self, account: AccountId, device: DeviceId) -> bool {
        let session = lock(&self.session);
        session.token.is_some() && session.identity == Some((account, device))
    }

    /// Opens a session (challenge, signature, token), replacing any previous one. The
    /// signer runs on tokio's blocking pool (it may wait for the core's lock), so the
    /// future is `Send`, unlike [`RemoteStore::authenticate`]'s with its borrowed signer.
    pub async fn authenticate_with(
        &self,
        account: AccountId,
        device: DeviceId,
        signer: AuthSigner,
    ) -> Result<(), SyncError> {
        self.forget_session();
        let (nonce, epoch) = self.challenge(account, device).await?;
        let message = kb_sync::auth_message(&nonce, &epoch);
        let signature = tokio::task::spawn_blocking(move || signer(&message))
            .await
            .ok()
            .flatten()
            .ok_or(SyncError::Unauthorized)?;
        self.session(account, device, &nonce, &signature).await
    }

    // ----- plumbing ---------------------------------------------------------------------------

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn token(&self) -> Option<String> {
        lock(&self.session).token.clone()
    }

    /// Sends a request with the bearer token when one is held.
    async fn send(&self, request: RequestBuilder) -> Result<Response, SyncError> {
        let request = match self.token() {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        request.send().await.map_err(transport_error)
    }

    /// Sends a request that needs the session; a 401 drops the token (the session is
    /// gone), a 403 does not.
    async fn send_authed(
        &self,
        build: impl FnOnce() -> RequestBuilder,
    ) -> Result<Response, SyncError> {
        let response = self.send(build()).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            tracing::debug!("session refused");
            self.forget_session();
        }
        Ok(response)
    }

    fn forget_session(&self) {
        let mut session = lock(&self.session);
        session.token = None;
        session.identity = None;
    }

    /// `POST /v2/auth/challenge`: the nonce and the server epoch it is bound to.
    async fn challenge(
        &self,
        account: AccountId,
        device: DeviceId,
    ) -> Result<([u8; 32], [u8; 16]), SyncError> {
        let body = ChallengeRequestJson {
            account_id: account,
            device_id: device,
        };
        let response = self
            .client
            .post(self.url("/v2/auth/challenge"))
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        let challenge: ChallengeJson = read_json(success(response).await?).await?;
        let nonce = hex_decode::<32>("nonce", &challenge.nonce)?;
        let epoch = match challenge.server_epoch {
            Some(epoch) => hex_decode::<16>("serverEpoch", &epoch)?,
            None => RemoteStore::server_info(self).await?.server_epoch,
        };
        Ok((nonce, epoch))
    }

    /// `POST /v2/auth/session`: exchanges the signed nonce for a token and keeps it.
    async fn session(
        &self,
        account: AccountId,
        device: DeviceId,
        nonce: &[u8; 32],
        signature: &Signature,
    ) -> Result<(), SyncError> {
        let body = SessionRequestJson {
            account_id: account,
            device_id: device,
            nonce: hex_encode(nonce),
            signature: hex_encode(&signature.to_bytes()),
        };
        let response = self
            .client
            .post(self.url("/v2/auth/session"))
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        let session: SessionJson = read_json(success(response).await?).await?;
        if session.token.is_empty() {
            return Err(SyncError::Protocol(
                "the server issued an empty token".to_owned(),
            ));
        }
        let mut shared = lock(&self.session);
        shared.token = Some(session.token);
        shared.identity = Some((account, device));
        Ok(())
    }
}

// ----- answers ---------------------------------------------------------------------------------

/// Reads a body, refusing one larger than `cap` (announced or actual).
pub(crate) async fn read_capped(mut response: Response, cap: u64) -> Result<Vec<u8>, SyncError> {
    let too_large =
        || SyncError::Protocol(format!("the server's answer is larger than {cap} bytes"));
    if response.content_length().is_some_and(|length| length > cap) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if (body.len() as u64).saturating_add(chunk.len() as u64) > cap {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn read_json<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, SyncError> {
    let bytes = read_capped(response, MAX_JSON_BYTES).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| SyncError::Encoding("the server's answer is not the expected JSON".to_owned()))
}

fn check_pairing_message(bytes: &[u8]) -> Result<(), SyncError> {
    if bytes.len() > PAIRING_MAX_MESSAGE_BYTES {
        return Err(SyncError::Protocol(format!(
            "a pairing message of {} bytes is larger than the {PAIRING_MAX_MESSAGE_BYTES}-byte limit",
            bytes.len()
        )));
    }
    Ok(())
}

fn check_recovery_blob(bytes: &[u8]) -> Result<(), SyncError> {
    if bytes.len() > RECOVERY_BLOB_MAX_BYTES {
        return Err(SyncError::Protocol(format!(
            "a recovery blob of {} bytes is larger than the {RECOVERY_BLOB_MAX_BYTES}-byte limit",
            bytes.len()
        )));
    }
    Ok(())
}

fn check_upload(bytes: &[u8]) -> Result<(), SyncError> {
    if bytes.len() as u64 > MAX_WIRE_BYTES {
        return Err(SyncError::Protocol(format!(
            "an object of {} bytes is larger than the {MAX_WIRE_BYTES}-byte limit",
            bytes.len()
        )));
    }
    Ok(())
}

/// A transport failure, described without the URL (or anything else from the request).
fn transport_error(error: reqwest::Error) -> SyncError {
    let error = error.without_url();
    let what = if error.is_timeout() {
        "the server did not answer in time"
    } else if error.is_connect() {
        "the server cannot be reached"
    } else if error.is_body() || error.is_decode() {
        "the server's answer was cut short"
    } else {
        "network error"
    };
    SyncError::Unavailable(what.to_owned())
}

// ----- WebSocket ---------------------------------------------------------------------------------

/// `http(s)://host/path` → `ws(s)://host/path/v2/ws`.
fn ws_url(base_url: &str) -> Result<String, NetError> {
    let rest = |scheme: &str| base_url.strip_prefix(scheme);
    if let Some(rest) = rest("https://") {
        Ok(format!("wss://{rest}/v2/ws"))
    } else if let Some(rest) = rest("http://") {
        Ok(format!("ws://{rest}/v2/ws"))
    } else {
        Err(NetError::Transport(
            "the server address must start with http:// or https://".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests;

mod v2;
mod ws;
pub use v2::status_error;
use v2::success;
