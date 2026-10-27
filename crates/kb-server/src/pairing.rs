//! Pairing mailboxes (ADR-0022 §1): a rendezvous where a joiner and a sponsor exchange
//! opaque messages (the server cannot read them: everything after the two ephemeral
//! keys is a `Pairing` envelope under a key only the devices derive). No session; a
//! mailbox lives `PAIRING_TTL_SECONDS`, holds `PAIRING_MAX_MESSAGES` of at most
//! `PAIRING_MAX_MESSAGE_BYTES`; 10 mailboxes per client address per 10 minutes and 60
//! requests per mailbox per minute.
//!
//! Message 0 is the joiner's offer; sequences increase by one. A reader asks for the
//! messages with `seq >= after` (`after` counts the messages it already has: the
//! sponsor reads the offer with `after=0`, the joiner polls from 1) and long-polls with
//! `wait`: the answer comes as soon as such a message exists, the mailbox is closed, the
//! wait elapses, or the mailbox expires (then 404).

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kb_types::devices::{
    MAILBOX_ID_LEN, PAIRING_MAX_MESSAGE_BYTES, PAIRING_POLL_SECONDS, PAIRING_TTL_SECONDS,
};
use serde::Deserialize;
use serde_json::json;
use tokio::time::Instant;

use crate::api::{AppState, json_body, ttl_ms};
use crate::codec::{base64, hex, unbase64, unhex};
use crate::error::HttpError;
use crate::limits::{
    ClientAddr, LimitKey, MAILBOX_REQUEST_WINDOW_MS, MAILBOX_WINDOW_MS, MAILBOXES_PER_ADDRESS,
    REQUESTS_PER_MAILBOX,
};
use crate::store::{MailboxId, MailboxView, Role};

fn parse_id(text: &str) -> Result<MailboxId, HttpError> {
    unhex::<MAILBOX_ID_LEN>(text)
        .ok_or_else(|| HttpError::bad_request("bad_request", "the mailbox id is malformed"))
}

fn parse_role(text: &str) -> Result<Role, HttpError> {
    match text {
        "joiner" => Ok(Role::Joiner),
        "sponsor" => Ok(Role::Sponsor),
        _ => Err(HttpError::bad_request(
            "bad_request",
            "role is joiner or sponsor",
        )),
    }
}

/// A pairing message from base64, refused beyond the size limit.
fn message_bytes(text: &str) -> Result<Vec<u8>, HttpError> {
    let bytes = unbase64(text)
        .ok_or_else(|| HttpError::bad_request("encoding", "bytes is not valid base64"))?;
    if bytes.len() > PAIRING_MAX_MESSAGE_BYTES {
        return Err(HttpError::too_large());
    }
    if bytes.is_empty() {
        return Err(HttpError::bad_request(
            "bad_request",
            "the message is empty",
        ));
    }
    Ok(bytes)
}

/// Counts a request against the mailbox's limit.
fn mailbox_limit(state: &AppState, id: &MailboxId) -> Result<(), HttpError> {
    let allowed = state.limiter.allow(
        LimitKey::Mailbox(*id),
        REQUESTS_PER_MAILBOX,
        MAILBOX_REQUEST_WINDOW_MS,
        state.now_ms(),
    );
    if allowed {
        Ok(())
    } else {
        Err(HttpError::rate_limited())
    }
}

/// Wakes every long-poll so it re-reads its mailbox.
fn mailboxes_changed(state: &AppState) {
    state
        .mailbox_events
        .send_modify(|version| *version = version.wrapping_add(1));
}

#[derive(Deserialize)]
struct CreateBody {
    offer: String,
}

/// `POST /v2/pair` `{offer}` → 201 `{mailboxId, expiresAt}`.
pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    client: ClientAddr,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let allowed = state.limiter.allow(
        LimitKey::MailboxCreate(client),
        MAILBOXES_PER_ADDRESS,
        MAILBOX_WINDOW_MS,
        state.now_ms(),
    );
    if !allowed {
        return Err(HttpError::rate_limited());
    }
    let body: CreateBody = json_body(body)?;
    let offer = message_bytes(&body.offer)?;
    let now = state.now_ms();
    let ttl = ttl_ms(PAIRING_TTL_SECONDS);
    let (id, expires_at) = state
        .db(move |store| store.create_mailbox(&offer, now, ttl))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "mailboxId": hex(&id), "expiresAt": expires_at })),
    )
        .into_response())
}

#[derive(Deserialize)]
pub(crate) struct PollQuery {
    after: Option<u32>,
    wait: Option<u32>,
}

fn page(view: &MailboxView) -> Response {
    let messages: Vec<serde_json::Value> = view
        .messages
        .iter()
        .map(|m| {
            json!({
                "seq": m.seq,
                "role": m.role.as_str(),
                "bytes": base64(&m.bytes),
            })
        })
        .collect();
    Json(json!({
        "messages": messages,
        "expiresAt": view.expires_at,
        "closed": view.closed,
    }))
    .into_response()
}

/// `GET /v2/pair/{id}/messages?after=&wait=` → 200 `{messages, expiresAt, closed}`:
/// the messages with `seq >= after` (default 0), waiting up to `wait` seconds (default
/// 0, at most `PAIRING_POLL_SECONDS`) for one.
pub(crate) async fn messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    query: Result<Query<PollQuery>, QueryRejection>,
) -> Result<Response, HttpError> {
    let id = parse_id(&id)?;
    mailbox_limit(&state, &id)?;
    let Query(query) =
        query.map_err(|_| HttpError::bad_request("bad_request", "after and wait are integers"))?;
    let after = query.after.unwrap_or(0);
    let wait = query.wait.unwrap_or(0).min(PAIRING_POLL_SECONDS);
    let deadline = Instant::now() + Duration::from_secs(u64::from(wait));
    // Subscribed before the first read: a change between the read and the wait still
    // wakes this poll.
    let mut changes = state.mailbox_events.subscribe();
    loop {
        let now = state.now_ms();
        let view = state
            .db(move |store| store.read_mailbox(&id, after, now))
            .await?
            .ok_or_else(|| HttpError::not_found("unknown or expired mailbox"))?;
        let ready = !view.messages.is_empty() || view.closed;
        if ready || Instant::now() >= deadline {
            return Ok(page(&view));
        }
        let to_expiry = u64::try_from(view.expires_at.saturating_sub(now)).unwrap_or(0);
        let wake = deadline.min(Instant::now() + Duration::from_millis(to_expiry));
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    return Ok(page(&view));
                }
            }
            () = tokio::time::sleep_until(wake) => {}
        }
    }
}

#[derive(Deserialize)]
struct PostBody {
    role: String,
    bytes: String,
}

/// `POST /v2/pair/{id}/messages` `{role, bytes}` → 204. Refusals in this order: an
/// unknown or expired mailbox (404), a bad role or message (400, 413 beyond the size),
/// a closed or full mailbox (409).
pub(crate) async fn post(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, HttpError> {
    let id = parse_id(&id)?;
    mailbox_limit(&state, &id)?;
    let body: PostBody = json_body(body)?;
    let now = state.now_ms();
    if !state.db(move |store| store.mailbox_live(&id, now)).await? {
        return Err(HttpError::not_found("unknown or expired mailbox"));
    }
    let role = parse_role(&body.role)?;
    let bytes = message_bytes(&body.bytes)?;
    state
        .db(move |store| store.post_mailbox(&id, role, &bytes, now))
        .await?;
    mailboxes_changed(&state);
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v2/pair/{id}` → 204: closes the mailbox.
pub(crate) async fn close(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    let id = parse_id(&id)?;
    mailbox_limit(&state, &id)?;
    let now = state.now_ms();
    state.db(move |store| store.close_mailbox(&id, now)).await?;
    mailboxes_changed(&state);
    Ok(StatusCode::NO_CONTENT)
}
