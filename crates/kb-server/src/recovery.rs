//! Recovery blobs (ADR-0022 §5): `wrap(ARK, HKDF(RcK, "recovery-wrap"))` with the
//! account id, stored under `recovery_id = HKDF(RcK, "recovery-id")`. The id is a
//! 256-bit secret lookup key and the blob opens only with the kit, so reading needs no
//! session (a device that lost everything has none) but is rate-limited per address;
//! writing and deleting need a session of the owning account. The id is never logged
//! in full.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use kb_types::devices::RECOVERY_BLOB_MAX_BYTES;

use crate::api::{AppState, Authed, binary};
use crate::codec::unhex;
use crate::error::HttpError;
use crate::limits::{ClientAddr, LimitKey, RECOVERY_READ_WINDOW_MS, RECOVERY_READS_PER_ADDRESS};

fn parse_id(text: &str) -> Result<[u8; 32], HttpError> {
    unhex::<32>(text)
        .ok_or_else(|| HttpError::bad_request("bad_request", "the recovery id is malformed"))
}

/// `PUT /v2/recovery/{id}` (bearer) → 204: stores or replaces the session account's blob.
pub(crate) async fn put(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, HttpError> {
    let id = parse_id(&id)?;
    let bytes = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            HttpError::too_large()
        } else {
            HttpError::bad_request("bad_request", "the body could not be read")
        }
    })?;
    if bytes.len() > RECOVERY_BLOB_MAX_BYTES {
        return Err(HttpError::too_large());
    }
    if bytes.is_empty() {
        return Err(HttpError::bad_request("bad_request", "the blob is empty"));
    }
    let now = state.now_ms();
    let account = authed.account;
    state
        .db(move |store| store.put_recovery(&id, account, &bytes, now))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v2/recovery/{id}` (no session; 5 per minute per address) → 200 bytes or 404.
pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    client: ClientAddr,
    Path(id): Path<String>,
) -> Result<Response, HttpError> {
    let allowed = state.limiter.allow(
        LimitKey::RecoveryRead(client),
        RECOVERY_READS_PER_ADDRESS,
        RECOVERY_READ_WINDOW_MS,
        state.now_ms(),
    );
    if !allowed {
        return Err(HttpError::rate_limited());
    }
    let id = parse_id(&id)?;
    state
        .db(move |store| store.recovery(&id))
        .await?
        .map(binary)
        .ok_or_else(|| HttpError::not_found("no recovery blob under this id"))
}

/// `DELETE /v2/recovery/{id}` (bearer of the owning account) → 204, also when the id
/// holds nothing; another account's blob: 403.
pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    let id = parse_id(&id)?;
    let account = authed.account;
    state
        .db(move |store| store.delete_recovery(&id, account))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
