//! HTTP errors: a status, a stable machine-readable code and a short message, answered
//! as `{"error": code, "message": message}`. Messages name what is wrong, never a body,
//! a token, a nonce or a key.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// An error answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl HttpError {
    fn new(status: StatusCode, code: &'static str, message: &str) -> Self {
        Self {
            status,
            code,
            message: message.to_owned(),
        }
    }

    /// 400 with a specific code (`bad_request`, `encoding`, `invalid_log`, ...).
    pub(crate) fn bad_request(code: &'static str, message: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    /// 403: a signature that does not verify under the device's key (clients map
    /// 401/403 to `Unauthorized`; 403 does not make them open a new session first).
    pub(crate) fn bad_signature() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "authentication",
            "the signature does not verify",
        )
    }

    /// 401: no session, or a session that is expired, unknown or of a device that is no
    /// longer active.
    pub(crate) fn unauthorized(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    /// 403: the session's account or device does not match the request.
    pub(crate) fn forbidden(message: &str) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub(crate) fn not_found(message: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub(crate) fn conflict(message: &str) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    /// 413: over `MAX_WIRE_BYTES`.
    pub(crate) fn too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "the body exceeds the server's size limit",
        )
    }

    /// 401 `idle`: the device has not opened a session for longer than the server's
    /// idle limit (`KB_SERVER_IDLE_DEVICE_DAYS`).
    pub(crate) fn idle() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "idle",
            "this device was idle too long; pair it again",
        )
    }

    /// 409 `rekey_needed` (v2): the feed waits for a `Rekey` before it takes content.
    pub(crate) fn rekey_needed() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "rekey_needed",
            "the space needs a new key before it accepts changes",
        )
    }

    /// 409 `stale_epoch` (v2): the entry was sealed under a key epoch below the feed's.
    pub(crate) fn stale_epoch() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "stale_epoch",
            "the entry was sealed under an old key epoch",
        )
    }

    /// 409 `feed_closed` (v2): an Owner closed the feed; it takes no more writes.
    pub(crate) fn feed_closed() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "feed_closed",
            "the space was closed and takes no more changes",
        )
    }

    /// 403 `forbidden` (v2): the account's role (or its removal) does not allow the request.
    pub(crate) fn role_forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "this account may not do that in this space",
        )
    }

    /// 403 `signup_limit` (v2): the share signup limits are used up, or the invite already created
    /// an account, or the invite is bound to another account.
    pub(crate) fn signup_limit() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "signup_limit",
            "this invite cannot create another account",
        )
    }

    /// 429: a rate limit (per client address or per pairing mailbox).
    pub(crate) fn rate_limited() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests; try again later",
        )
    }

    /// 507: the account quota is exhausted.
    pub(crate) fn quota() -> Self {
        Self::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "quota",
            "the account quota is exceeded",
        )
    }

    /// 500 for a storage failure; the detail goes to the log, not to the client.
    pub(crate) fn storage(error: &rusqlite::Error) -> Self {
        tracing::error!(%error, "storage failure");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "storage failure",
        )
    }

    pub(crate) fn internal(message: &str) -> Self {
        tracing::error!(message, "internal failure");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        (self.status, Json(body)).into_response()
    }
}

/// The answer for a protocol-level refusal of `kb-sync`'s shared rules ([`kb_sync::SyncError`]):
/// the table of `docs/development/milestone-13.md` 1.1, read in the server's direction. The
/// messages of `Encoding`, `Protocol` and `Conflict` name a rule or a field, never data.
impl From<kb_sync::SyncError> for HttpError {
    fn from(error: kb_sync::SyncError) -> Self {
        use kb_sync::SyncError;
        match error {
            SyncError::Encoding(message) => Self::bad_request("encoding", &message),
            SyncError::Authentication => Self::bad_signature(),
            SyncError::Protocol(message) => Self::bad_request("bad_request", &message),
            SyncError::Quota => Self::quota(),
            SyncError::NotFound => Self::not_found("not found"),
            SyncError::Unauthorized => Self::unauthorized("unauthorized"),
            SyncError::Idle => Self::idle(),
            // The only thing the shared rules answer with this is the Rekey rate limit.
            SyncError::Unavailable(_) => Self::rate_limited(),
            SyncError::Conflict(message) => Self::conflict(&message),
            SyncError::RekeyNeeded => Self::rekey_needed(),
            SyncError::StaleEpoch => Self::stale_epoch(),
            SyncError::Closed => Self::feed_closed(),
            SyncError::Forbidden => Self::role_forbidden(),
        }
    }
}
