//! View links (M13 slice 4, ADR-0051, `docs/security/sharing.md` sections 6 and 10,
//! `docs/development/milestone-13.md` 4.1 and its contract notes): the server's half of a link, a row it
//! stores and serves without being able to read it.
//!
//! ```text
//! PUT    /v2/links/{id}                 bearer   body: kb_sync::v2::encode_link_put      -> 200 {"pubSeq": N}
//! GET    /v2/links/{id}                 public   If-None-Match: "N"                      -> 200 octet-stream + ETag: "N" | 304 | 404 | 429
//! DELETE /v2/links/{id}                 bearer                                           -> 204 | 403 | 404
//! GET    /v2/links/{id}/chunks/{hash}   public                                           -> 200 octet-stream | 404 | 429
//! ```
//!
//! The rules are those of the "View links" bullet of `kb_sync::v2`'s module documentation and of
//! `RemoteStore::link_put`, `link_get`, `link_delete` and `link_chunk_get`, which `MemoryRemote` and this
//! server share (the conformance suite checks both):
//!
//! - **Tables** (in `store/v2.rs`'s schema, all `IF NOT EXISTS`, so a v3 database of slices 1 to 3 opens and
//!   gains them; `user_version` stays 3): `links(link_id BLOB PRIMARY KEY, feed BLOB, created_by BLOB,
//!   publication BLOB, pub_seq INTEGER, expires_at INTEGER NULL)` and `link_chunks(link_id BLOB, hash
//!   BLOB, PRIMARY KEY (link_id, hash))`, the declared hashes. A chunk is served when the link declares its
//!   hash **and** the link's feed holds it (`feed_chunks`): the declaration is not a copy.
//! - **`PUT`**: the body is `decode_link_put` (any failure is 400 `bad_request`/`encoding`, a
//!   declaration over `LINK_MAX_CHUNKS` 400, a publication over `MAX_WIRE_BYTES` 413: the route's body
//!   limit leaves room for the framing and the declared hashes, and `HttpRemote` reports the 413 as
//!   `SyncError::Quota`, as `RemoteStore::link_put` says); the publication must
//!   parse as an unsigned envelope of kind `Publication` whose header names `{id}`
//!   (`kb_sync::v2::peek_publication`; the server cannot and does not try to open it). The session's
//!   account must be an Editor or Owner of `put.feed` (a shared feed's folded membership state, the
//!   materialised `feed_members`), or the account itself for its personal feed (`feed == account id`): a
//!   stranger gets 404 `not_found`, a Viewer or Commenter 403 `forbidden`, a closed feed 409
//!   `feed_closed`. The **first** upload creates the row (`created_by` the session's account, `expires_at`
//!   the upload's, `pub_seq` 1); a later one needs the same `feed` (409 `conflict` otherwise), keeps the
//!   row's `created_by` and `expires_at`, replaces the publication and the declared chunks and
//!   increments `pub_seq`. An expiry more than `LINK_EXPIRY_MAX_DAYS` days (plus an hour of clock skew)
//!   ahead of the server's clock is 400 `bad_request`. A declared hash need not be held yet: it is served
//!   once the feed holds it, never before. The publication's bytes count against the feed's billing
//!   account (507 `quota`) until the link goes. The setting "Editors may create view links" is
//!   encrypted, so the server cannot check it and does not (honest clients do). The answer is
//!   `{"pubSeq": N}`.
//! - **`GET`** (no session): 404 `not_found` for a link that is unknown, expired (`expires_at <= now` on
//!   the server's clock) or deleted, one answer for all three (and for a malformed id); `If-None-Match`
//!   holding `"N"` (quotes and a `W/` prefix tolerated; a list or `*` too) equal to the stored `pub_seq`
//!   gives 304 with the `ETag` and no body; otherwise
//!   200, `application/octet-stream`, `ETag: "N"`, `Cache-Control: no-store`, the publication as stored. 120
//!   reads (publication and chunks together) per client address per minute
//!   (`LINK_GETS_PER_MINUTE_PER_IP`), 429 `rate_limited` beyond, counted per address like `invite_open`
//!   (`LimitKey`), whatever the answer.
//! - **`DELETE`**: the row, the publication and the declared chunks go. The session's account must be the
//!   link's `created_by` **while still a current member** of its feed, or an Owner of it: another member is
//!   403 `forbidden`, a stranger or an unknown link 404. Idempotent for the client: a second delete is a
//!   404, which `HttpRemote` reports as `NotFound` and the driver counts as done.
//! - **Close and expiry**: closing a feed deletes its links; the hourly maintenance pass deletes the rows
//!   whose `expires_at` has passed (the pass `PruneReport::expired` counts); a feed deleted after its
//!   close-read window takes its links with it.
//! - **Logs**: paths only with every id cut to 8 hex characters (`loggable_path` already cuts the
//!   36-character link id and the 64-character chunk hash), never a body, a publication or an `ETag` value
//!   beyond what the status says; `feed` in the request span is the 8-character form.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use kb_sync::v2::publication::LINK_PUT_PREFIX_LEN;
use kb_sync::v2::{decode_link_put, peek_publication};
use kb_types::sharing::{LINK_GETS_PER_MINUTE_PER_IP, LINK_MAX_CHUNKS};
use kb_types::sync::MAX_WIRE_BYTES;
use kb_types::{ChunkId, LinkId};
use serde_json::json;

use crate::api::{AppState, Authed, binary, short_id};
use crate::error::HttpError;
use crate::limits::{ClientAddr, LINK_GET_WINDOW_MS, LimitKey};
use crate::store::LinkRead;

/// The `Cache-Control` of every link answer: nothing of a link is stored by an intermediary or a cache.
const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

/// The body limit of `PUT /v2/links/{id}`: a publication of `MAX_WIRE_BYTES` plus the framing and the most
/// declared hashes.
pub(crate) fn upload_limit() -> usize {
    usize::try_from(MAX_WIRE_BYTES)
        .unwrap_or(usize::MAX)
        .saturating_add(LINK_PUT_PREFIX_LEN)
        .saturating_add(LINK_MAX_CHUNKS.saturating_mul(32))
}

/// The id of a bearer route's path; malformed is the caller's mistake (400).
fn parse_link(text: &str) -> Result<LinkId, HttpError> {
    text.parse()
        .map_err(|_| HttpError::bad_request("bad_request", "the link id is malformed"))
}

/// The id of a public route's path; malformed is one more way not to be a link (404).
fn parse_public(text: &str) -> Result<LinkId, HttpError> {
    text.parse().map_err(|_| HttpError::not_found("not found"))
}

/// Counts one public read for the client's address; refuses it past `LINK_GETS_PER_MINUTE_PER_IP`.
fn admit(state: &AppState, client: ClientAddr) -> Result<(), HttpError> {
    let limit = usize::try_from(LINK_GETS_PER_MINUTE_PER_IP).unwrap_or(usize::MAX);
    if state.limiter.allow(
        LimitKey::LinkGet(client),
        limit,
        LINK_GET_WINDOW_MS,
        state.now_ms(),
    ) {
        Ok(())
    } else {
        Err(HttpError::rate_limited())
    }
}

/// The `pub_seq` values an `If-None-Match` names (`"N"`, `W/"N"`, a list of them) and whether it is `*`.
#[derive(Default)]
struct Validators {
    any: bool,
    seqs: Vec<u64>,
}

impl Validators {
    fn from_headers(headers: &HeaderMap) -> Self {
        let mut out = Self::default();
        for value in headers.get_all(header::IF_NONE_MATCH) {
            let Ok(text) = value.to_str() else {
                continue;
            };
            for part in text.split(',') {
                let part = part.trim();
                if part == "*" {
                    out.any = true;
                    continue;
                }
                let part = part.strip_prefix("W/").unwrap_or(part);
                if let Ok(seq) = part.trim_matches('"').parse::<u64>() {
                    out.seqs.push(seq);
                }
            }
        }
        out
    }

    fn matches(&self, seq: u64) -> bool {
        self.any || self.seqs.contains(&seq)
    }
}

fn etag(pub_seq: u64) -> Result<HeaderValue, HttpError> {
    HeaderValue::from_str(&format!("\"{pub_seq}\""))
        .map_err(|_| HttpError::internal("an ETag could not be made"))
}

/// `PUT /v2/links/{id}` (bearer) with the body of `kb_sync::v2::encode_link_put`: creates or refreshes
/// the link, answers `{"pubSeq": N}`.
pub(crate) async fn put(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let link = parse_link(&id)?;
    // The route's body limit leaves room for the framing; the publication itself has the usual one.
    let bytes = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            HttpError::too_large()
        } else {
            HttpError::bad_request("bad_request", "the body could not be read")
        }
    })?;
    let put = decode_link_put(&bytes).map_err(HttpError::from)?;
    drop(bytes);
    if u64::try_from(put.publication.len()).map_or(true, |size| size > MAX_WIRE_BYTES) {
        return Err(HttpError::too_large());
    }
    let header = peek_publication(&put.publication).map_err(HttpError::from)?;
    if header.link_id != link {
        return Err(HttpError::bad_request(
            "bad_request",
            "the publication names another link than the path",
        ));
    }
    tracing::Span::current().record("feed", short_id(&put.feed).as_str());
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let account = authed.account;
    let pub_seq = state
        .db(move |store| store.link_put(account, link, &put, quota, now))
        .await?;
    tracing::info!(link = %link.short_hex(), pub_seq, "link published");
    Ok(Json(json!({ "pubSeq": pub_seq })).into_response())
}

/// `GET /v2/links/{id}` (no session): the publication, 304 for a matching `If-None-Match`, one 404 for a
/// link that is unknown, expired or deleted, 429 past the read limit.
pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    client: ClientAddr,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, HttpError> {
    admit(&state, client)?;
    let link = parse_public(&id)?;
    let validators = Validators::from_headers(&headers);
    let now = state.now_ms();
    let read = state
        .db(move |store| store.link_get(link, now, |seq| validators.matches(seq)))
        .await?;
    match read {
        LinkRead::Missing => Err(HttpError::not_found("not found")),
        LinkRead::NotModified(pub_seq) => Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag(pub_seq)?),
                (header::CACHE_CONTROL, NO_STORE),
            ],
        )
            .into_response()),
        LinkRead::Found { pub_seq, bytes } => {
            let mut response = binary(bytes);
            let headers = response.headers_mut();
            headers.insert(header::ETAG, etag(pub_seq)?);
            headers.insert(header::CACHE_CONTROL, NO_STORE);
            Ok(response)
        }
    }
}

/// `DELETE /v2/links/{id}` (bearer): revokes the link (its creator while a member, or an Owner).
pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    let link = parse_link(&id)?;
    let account = authed.account;
    let now = state.now_ms();
    state
        .db(move |store| store.link_delete(account, link, now))
        .await?;
    tracing::info!(link = %link.short_hex(), "link revoked");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v2/links/{id}/chunks/{hash}` (no session): a chunk the link declares and its feed holds.
pub(crate) async fn chunk(
    State(state): State<Arc<AppState>>,
    client: ClientAddr,
    Path((id, hash)): Path<(String, String)>,
) -> Result<Response, HttpError> {
    admit(&state, client)?;
    let link = parse_public(&id)?;
    let hash = ChunkId::from_hex(&hash)
        .map_err(|_| HttpError::not_found("not found"))?
        .to_bytes();
    let now = state.now_ms();
    let bytes = state
        .db(move |store| store.link_chunk(link, &hash, now))
        .await?;
    let mut response = binary(bytes.ok_or_else(|| HttpError::not_found("not found"))?);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, NO_STORE);
    Ok(response)
}
