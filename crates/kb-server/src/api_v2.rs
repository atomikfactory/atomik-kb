//! The HTTP surface under `/v2` (M13, ADR-0045, `docs/security/sharing.md` section 6): feeds, their
//! membership log, entries, snapshots, heads, chunks, invites and share signups, with the device
//! log, authentication, pairing and recovery handlers of `api.rs`, `pairing.rs` and `recovery.rs`. Every handler
//! checks the plaintext header of what it stores (`kb_sync::v2::peek_*`) and the session against the
//! feed's access list (`store/v2.rs`); the server never reads past a header, and a log line never
//! holds more of an id than its first eight hex characters.
//!
//! Wire shapes: control is camelCase JSON (ids as UUID text, hashes and signatures as hex, log and
//! membership entries as base64), payloads (entries, snapshots, heads, chunks, pages) are
//! `application/octet-stream`. `204` answers a read of something a member's feed does not hold
//! (`GET .../snapshots/{doc}`, `GET .../chunks/{hash}`), so a `404` always means "not a member or no
//! such feed". Errors are `{"error": code, "message": ...}` with the codes of
//! `docs/development/milestone-13.md` 1.1.
//!
//! `GET /join`, `/join.js` and `/join.css` (outside `/v2`) are the static page of the link flow.
//! View links are `/v2/links/{id}` and `/v2/links/{id}/chunks/{hash}` (`links.rs`); `GET /view` and
//! `/view/{*path}` (outside `/v2`) serve the viewer (`viewer.rs`), both M13 slice 4.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use kb_crypto::Signature;
use kb_sync::v2::{FeedItemInfo, ItemKind, SnapshotMark};
use kb_sync::{ChangesPage, SnapshotsPage};
use kb_types::devices::RECOVERY_BLOB_MAX_BYTES;
use kb_types::sync::{CHANGES_PAGE_LIMIT, FeedKind, MAX_WIRE_BYTES, PROTOCOL_VERSION};
use kb_types::{AccountId, DeviceId, FeedId, InviteId};
use serde::Deserialize;
use serde_json::json;

use crate::api::{
    AppState, Authed, binary, body_bytes, decode_log, json_body, parse_account, parse_chunk,
    parse_doc, short_id, verify_envelope,
};
use crate::codec::{base64, hex, unbase64, unhex};
use crate::error::HttpError;
use crate::limits::{ClientAddr, INVITE_OPEN_WINDOW_MS, INVITE_OPENS_PER_ADDRESS, LimitKey};
use crate::store::SignupLimits;
use crate::ws::Position;

/// Largest JSON body of the small control calls (account creation, log batches, invite opens).
const JSON_BODY_LIMIT: usize = 1024 * 1024;
/// Entry payload bytes per page of changes (a page always carries at least one entry).
const PAGE_PAYLOAD_BUDGET: u64 = MAX_WIRE_BYTES;
/// Accounts one `POST /v2/accounts/logs` may ask for.
const LOG_BATCH_MAX: usize = 256;

fn parse_feed(text: &str) -> Result<FeedId, HttpError> {
    text.parse()
        .map_err(|_| HttpError::bad_request("bad_request", "the feed id is malformed"))
}

fn parse_invite(text: &str) -> Result<InviteId, HttpError> {
    text.parse()
        .map_err(|_| HttpError::bad_request("bad_request", "the invite id is malformed"))
}

/// Notes the feed in the request's log span (8 hex characters).
fn span_feed(feed: FeedId) {
    tracing::Span::current().record("feed", short_id(&feed).as_str());
}

/// The header of an object must name the feed of the path and the session's account and device.
fn check_owner(
    authed: &Authed,
    feed: FeedId,
    named_feed: FeedId,
    account: AccountId,
    device: DeviceId,
) -> Result<(), HttpError> {
    if named_feed != feed {
        return Err(HttpError::bad_request(
            "bad_request",
            "the object names another feed than the path",
        ));
    }
    if account != authed.account || device != authed.device {
        return Err(HttpError::role_forbidden());
    }
    Ok(())
}

fn role_name(role: Option<kb_types::MemberRole>) -> serde_json::Value {
    role.map_or(serde_json::Value::Null, |role| json!(role.as_str()))
}

fn head_json(head: kb_sync::v2::MembershipHead) -> serde_json::Value {
    json!({ "seq": head.seq, "hash": hex(&head.hash) })
}

// ----- server, accounts and logs ------------------------------------------------------------

async fn server_info(State(state): State<Arc<AppState>>) -> Response {
    Json(json!({
        "protocolVersion": PROTOCOL_VERSION,
        "serverEpoch": hex(&state.server_epoch),
        "maxWireBytes": MAX_WIRE_BYTES,
        "accountQuotaBytes": state.settings.account_quota_bytes,
        "idleDeviceDays": state.settings.idle_device_days,
        "shareSignups": state.settings.share_signups,
        "shareSignupsPerAccount": state.settings.share_signups_per_account,
    }))
    .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareClaim {
    invite_id: InviteId,
    /// The invite key's signature over `"signup" ‖ account ‖ server_epoch`, hex.
    signature: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAccountBody {
    /// The operator's invite token.
    invite_token: Option<String>,
    /// The claim of a share invite, instead of a token.
    share: Option<ShareClaim>,
    log: Vec<String>,
}

async fn create_account(
    State(state): State<Arc<AppState>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let body: CreateAccountBody = json_body(body)?;
    let entries = decode_log(&body.log)?;
    if entries.is_empty() {
        return Err(HttpError::bad_request(
            "invalid_log",
            "the log needs at least its genesis entry",
        ));
    }
    let now = state.now_ms();
    let account = match (body.share, body.invite_token) {
        (Some(claim), _) => {
            let signature = unhex::<64>(&claim.signature)
                .and_then(|bytes| Signature::from_bytes(&bytes).ok())
                .ok_or_else(|| {
                    HttpError::bad_request("bad_request", "the signature is malformed")
                })?;
            let limits = SignupLimits {
                total: state.settings.share_signups,
                per_account: state.settings.share_signups_per_account,
            };
            state
                .db(move |store| {
                    store.account_create_share(entries, claim.invite_id, &signature, limits, now)
                })
                .await?
        }
        (None, Some(token)) => {
            if !state.invite_ok(&token) {
                return Err(HttpError::unauthorized("the invite token is not valid"));
            }
            state
                .db(move |store| store.create_account(entries, now))
                .await?
        }
        (None, None) => {
            return Err(HttpError::bad_request(
                "bad_request",
                "an invite token or a share claim is required",
            ));
        }
    };
    tracing::Span::current().record("account", short_id(&account).as_str());
    tracing::info!(account = %short_id(&account), "account created");
    Ok((StatusCode::CREATED, Json(json!({ "accountId": account }))).into_response())
}

#[derive(Deserialize)]
struct AfterQuery {
    after: Option<u64>,
}

/// `GET /v2/accounts/{id}/log?after=seq`: the account's device log after `seq` (the whole log
/// without it). Public to whoever knows the account id: co-members fetch each other's.
async fn fetch_log(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    query: Result<Query<AfterQuery>, QueryRejection>,
) -> Result<Response, HttpError> {
    let account = parse_account(&id)?;
    let Query(query) =
        query.map_err(|_| HttpError::bad_request("bad_request", "after is an integer"))?;
    let entries = state.db(move |store| store.fetch_log(account)).await?;
    if entries.is_empty() {
        return Err(HttpError::not_found("unknown account"));
    }
    let skip = query.after.map_or(0, |after| {
        usize::try_from(after.saturating_add(1)).unwrap_or(usize::MAX)
    });
    let entries: Vec<String> = entries.iter().skip(skip).map(|e| base64(e)).collect();
    Ok(Json(json!({ "entries": entries })).into_response())
}

#[derive(Deserialize)]
struct LogsBody {
    accounts: Vec<LogsRequest>,
}

#[derive(Deserialize)]
struct LogsRequest {
    id: AccountId,
    after: Option<u64>,
}

/// `POST /v2/accounts/logs`: several accounts' logs in one call; unknown accounts are left out.
async fn logs_batch(
    State(state): State<Arc<AppState>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let body: LogsBody = json_body(body)?;
    if body.accounts.len() > LOG_BATCH_MAX {
        return Err(HttpError::bad_request(
            "bad_request",
            "too many accounts in one request",
        ));
    }
    let logs = state
        .db(move |store| {
            let mut out = Vec::new();
            for request in body.accounts {
                let entries = store.fetch_log(request.id)?;
                if entries.is_empty() {
                    continue;
                }
                let skip = request.after.map_or(0, |after| {
                    usize::try_from(after.saturating_add(1)).unwrap_or(usize::MAX)
                });
                let entries: Vec<String> = entries.iter().skip(skip).map(|e| base64(e)).collect();
                out.push(json!({ "account": request.id, "entries": entries }));
            }
            Ok(out)
        })
        .await?;
    Ok(Json(json!({ "logs": logs })).into_response())
}

// ----- feeds and membership -----------------------------------------------------------------

/// `POST /v2/feeds` (the `Create` entry as the body) → `{feedId, head: {seq, hash}, cursor}`.
async fn feeds_create(
    State(state): State<Arc<AppState>>,
    _authed: Authed,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let bytes = body_bytes(body)?;
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let created = state
        .db(move |store| store.feeds_create(&bytes, quota, now))
        .await?;
    span_feed(created.feed);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "feedId": created.feed,
            "head": head_json(created.head),
            "cursor": created.cursor,
        })),
    )
        .into_response())
}

/// `GET /v2/feeds`: the personal feed and the shared feeds the account belongs to or belonged to.
async fn feeds_list(
    State(state): State<Arc<AppState>>,
    authed: Authed,
) -> Result<Response, HttpError> {
    let account = authed.account;
    let feeds = state.db(move |store| store.feeds_list(account)).await?;
    let feeds: Vec<serde_json::Value> = feeds
        .iter()
        .map(|listing| {
            json!({
                "feedId": listing.feed,
                "kind": match listing.kind {
                    FeedKind::Personal => "personal",
                    FeedKind::Shared => "shared",
                },
                "role": role_name(listing.role),
                "removed": listing.removed,
                "closed": listing.closed,
            })
        })
        .collect();
    Ok(Json(json!({ "feeds": feeds })).into_response())
}

#[derive(Deserialize)]
struct EntriesBody {
    entries: Vec<String>,
}

/// `POST /v2/feeds/{f}/membership` `{entries: [base64]}` → `{seq, hash}`: a compare-and-swap, a batch
/// is atomic.
async fn membership_append(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let body: EntriesBody = json_body(body)?;
    if body.entries.is_empty() {
        return Err(HttpError::bad_request(
            "bad_request",
            "a batch holds no entries",
        ));
    }
    let entries = body
        .entries
        .iter()
        .map(|text| {
            unbase64(text)
                .ok_or_else(|| HttpError::bad_request("encoding", "an entry is not valid base64"))
        })
        .collect::<Result<Vec<Vec<u8>>, _>>()?;
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let session = authed.account;
    let hub = state.hub.clone();
    let appended = state
        .db(move |store| {
            let appended = store.membership_append(session, feed, &entries, quota, now)?;
            // Still under the store's lock: notices leave in the order of the cursors, one per entry.
            for (stored, bytes) in appended.stored_at.iter().zip(&entries) {
                let position = Position {
                    feed,
                    prev_cursor: stored.prev_cursor,
                    cursor: stored.cursor,
                    chain_after: stored.chain_hash,
                };
                hub.publish_item(
                    position,
                    appended.notify.clone(),
                    appended.ended.clone(),
                    bytes,
                );
            }
            Ok(appended)
        })
        .await?;
    tracing::info!(entries = appended.entries, "membership entries appended");
    Ok(Json(head_json(appended.head)).into_response())
}

/// `GET /v2/feeds/{f}/membership?after=seq` → `{entries: [{cursor, bytes}], head, more}`.
async fn membership_get(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
    query: Result<Query<AfterQuery>, QueryRejection>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let Query(query) =
        query.map_err(|_| HttpError::bad_request("bad_request", "after is an integer"))?;
    let account = authed.account;
    let now = state.now_ms();
    let page = state
        .db(move |store| store.membership_get(account, feed, query.after, now))
        .await?;
    let entries: Vec<serde_json::Value> = page
        .entries
        .iter()
        .map(|item| json!({ "cursor": item.cursor, "bytes": base64(&item.bytes) }))
        .collect();
    Ok(Json(json!({
        "entries": entries,
        "head": head_json(page.head),
        "more": page.more,
    }))
    .into_response())
}

// ----- entries and changes ------------------------------------------------------------------

/// `POST /v2/feeds/{f}/entries` (the sealed entry or keyring entry) → `{cursor, chainHash}`.
async fn append_entry(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let bytes = body_bytes(body)?;
    let info: FeedItemInfo = kb_sync::v2::peek_feed_item(&bytes).map_err(|error| {
        HttpError::from(match error {
            kb_sync::SyncError::Protocol(message) => kb_sync::SyncError::Protocol(message),
            _ => kb_sync::SyncError::Encoding("the body is not a feed item".to_owned()),
        })
    })?;
    if info.kind == ItemKind::Membership {
        return Err(HttpError::bad_request(
            "bad_request",
            "membership entries go through the membership endpoint",
        ));
    }
    check_owner(&authed, feed, info.feed, info.account_id, info.device_id)?;
    verify_envelope(&bytes, &authed.signing)?;
    let key_epoch = match info.kind {
        ItemKind::Entry => kb_sync::v2::peek_entry(&bytes).map(|h| h.key_epoch),
        _ => kb_sync::v2::peek_keyring(&bytes).map(|h| h.key_epoch),
    }
    .map_err(HttpError::from)?;
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let account = authed.account;
    let hub = state.hub.clone();
    let appended = state
        .db(move |store| {
            let appended =
                store.append_item(account, feed, &info, key_epoch, &bytes, quota, now)?;
            // Still under the store's lock: notices leave in the order of the cursors.
            if appended.new {
                let position = Position {
                    feed,
                    prev_cursor: appended.prev_cursor,
                    cursor: appended.cursor,
                    chain_after: appended.chain_hash,
                };
                hub.publish_item(position, appended.notify.clone(), Vec::new(), &bytes);
            }
            Ok(appended)
        })
        .await?;
    Ok(Json(json!({
        "cursor": appended.cursor,
        "chainHash": hex(&appended.chain_hash),
    }))
    .into_response())
}

#[derive(Deserialize)]
struct ChangesQuery {
    after: Option<u64>,
    limit: Option<u32>,
}

/// `GET /v2/feeds/{f}/changes?after=&limit=` → a binary page.
async fn changes(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
    query: Result<Query<ChangesQuery>, QueryRejection>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let Query(query) =
        query.map_err(|_| HttpError::bad_request("bad_request", "after and limit are integers"))?;
    let after = query.after.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(CHANGES_PAGE_LIMIT)
        .clamp(1, CHANGES_PAGE_LIMIT);
    let account = authed.account;
    let now = state.now_ms();
    let page = state
        .db(move |store| store.feed_changes(account, feed, after, limit, PAGE_PAYLOAD_BUDGET, now))
        .await?;
    let page = ChangesPage {
        server_epoch: state.server_epoch,
        changes: page.changes,
        chain_hash_after: page.chain_hash_after,
        more: page.more,
    };
    Ok(binary(page.encode()))
}

// ----- snapshots ----------------------------------------------------------------------------

async fn put_snapshot(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path((feed, doc)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let doc = parse_doc(&doc)?;
    let bytes = body_bytes(body)?;
    let header = kb_sync::v2::peek_snapshot(&bytes).map_err(|error| match error {
        kb_sync::SyncError::Protocol(message) => HttpError::bad_request("bad_request", &message),
        _ => HttpError::bad_request("encoding", "the snapshot header is malformed"),
    })?;
    check_owner(
        &authed,
        feed,
        header.feed,
        header.account_id,
        header.device_id,
    )?;
    if header.doc_id != doc {
        return Err(HttpError::bad_request(
            "bad_request",
            "the snapshot is of another document than the path names",
        ));
    }
    verify_envelope(&bytes, &authed.signing)?;
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let (account, device, covers) = (authed.account, authed.device, header.covers_cursor);
    state
        .db(move |store| {
            store.put_feed_snapshot(account, device, feed, doc, covers, &bytes, quota, now)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_snapshot(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path((feed, doc)): Path<(String, String)>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let doc = parse_doc(&doc)?;
    let account = authed.account;
    let now = state.now_ms();
    let snapshot = state
        .db(move |store| store.feed_snapshot(account, feed, doc, now))
        .await?;
    Ok(snapshot.map_or_else(|| StatusCode::NO_CONTENT.into_response(), binary))
}

async fn all_snapshots(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let account = authed.account;
    let now = state.now_ms();
    let snapshots = state
        .db(move |store| store.feed_snapshots(account, feed, now))
        .await?;
    let page = SnapshotsPage {
        server_epoch: state.server_epoch,
        snapshots,
    };
    Ok(binary(page.encode()))
}

/// `GET /v2/feeds/{f}/snapshot-marks?after=mark` → `{marks: [...], more}`.
async fn snapshot_marks(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
    query: Result<Query<AfterQuery>, QueryRejection>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let Query(query) =
        query.map_err(|_| HttpError::bad_request("bad_request", "after is an integer"))?;
    let account = authed.account;
    let now = state.now_ms();
    let page = state
        .db(move |store| store.feed_snapshot_marks(account, feed, query.after.unwrap_or(0), now))
        .await?;
    let marks: Vec<serde_json::Value> = page
        .marks
        .iter()
        .map(
            |SnapshotMark {
                 mark,
                 doc_id,
                 device_id,
                 account_id,
                 covers_cursor,
             }| {
                json!({
                    "mark": mark,
                    "docId": doc_id,
                    "deviceId": device_id,
                    "accountId": account_id,
                    "coversCursor": covers_cursor,
                })
            },
        )
        .collect();
    Ok(Json(json!({ "marks": marks, "more": page.more })).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisputeBody {
    covers_cursor: u64,
}

/// `POST /v2/feeds/{f}/snapshots/{doc}/dispute` `{coversCursor}` → 204.
async fn dispute_snapshot(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path((feed, doc)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let doc = parse_doc(&doc)?;
    let body: DisputeBody = json_body(body)?;
    let account = authed.account;
    let now = state.now_ms();
    state
        .db(move |store| store.feed_snapshot_dispute(account, feed, doc, body.covers_cursor, now))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- heads --------------------------------------------------------------------------------

async fn put_head(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let bytes = body_bytes(body)?;
    let head = kb_sync::v2::peek_head(&bytes).map_err(|error| match error {
        kb_sync::SyncError::Protocol(message) => HttpError::bad_request("bad_request", &message),
        _ => HttpError::bad_request("encoding", "the head is malformed"),
    })?;
    check_owner(&authed, feed, head.feed, head.account_id, head.device_id)?;
    let head = kb_sync::v2::verify_head(&bytes, &authed.signing)
        .map_err(|_| HttpError::bad_signature())?;
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let (account, device) = (authed.account, authed.device);
    state
        .db(move |store| {
            store.put_feed_head(account, device, feed, head.cursor, &bytes, quota, now)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_heads(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path(feed): Path<String>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let account = authed.account;
    let now = state.now_ms();
    let heads = state
        .db(move |store| store.feed_heads(account, feed, now))
        .await?;
    let heads: Vec<String> = heads.iter().map(|h| base64(h)).collect();
    Ok(Json(json!({ "heads": heads })).into_response())
}

// ----- chunks -------------------------------------------------------------------------------

async fn put_chunk(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path((feed, hash)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let hash = parse_chunk(&hash)?.to_bytes();
    let bytes = body_bytes(body)?;
    if kb_crypto::hash(&bytes) != hash {
        return Err(HttpError::bad_request(
            "bad_request",
            "the chunk does not hash to the path",
        ));
    }
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let account = authed.account;
    state
        .db(move |store| store.put_feed_chunk(account, feed, &hash, &bytes, quota, now))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_chunk(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path((feed, hash)): Path<(String, String)>,
) -> Result<Response, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let hash = parse_chunk(&hash)?.to_bytes();
    let account = authed.account;
    let now = state.now_ms();
    let chunk = state
        .db(move |store| store.feed_chunk(account, feed, &hash, now))
        .await?;
    Ok(chunk.map_or_else(|| StatusCode::NO_CONTENT.into_response(), binary))
}

async fn delete_chunk(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path((feed, hash)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let hash = parse_chunk(&hash)?.to_bytes();
    let account = authed.account;
    let now = state.now_ms();
    state
        .db(move |store| store.delete_feed_chunk(account, feed, &hash, now))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn copy_chunk(
    State(state): State<Arc<AppState>>,
    authed: Authed,
    Path((feed, hash)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    let feed = parse_feed(&feed)?;
    span_feed(feed);
    let hash = parse_chunk(&hash)?.to_bytes();
    let quota = state.settings.account_quota_bytes;
    let now = state.now_ms();
    let account = authed.account;
    state
        .db(move |store| store.copy_feed_chunk(account, feed, &hash, quota, now))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- invites ------------------------------------------------------------------------------

#[derive(Deserialize)]
struct OpenBody {
    /// The invite key's signature over `"open" ‖ invite_id ‖ server_epoch`, hex.
    signature: String,
}

/// `POST /v2/invites/{id}/open` `{signature}` → `{feedId, log: [{cursor, bytes}]}`; 30 per client
/// address per hour; a dead invite and a bad signature are the same `404`.
async fn invite_open(
    State(state): State<Arc<AppState>>,
    client: ClientAddr,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, HttpError> {
    let allowed = state.limiter.allow(
        LimitKey::InviteOpen(client),
        INVITE_OPENS_PER_ADDRESS,
        INVITE_OPEN_WINDOW_MS,
        state.now_ms(),
    );
    if !allowed {
        return Err(HttpError::rate_limited());
    }
    let invite = parse_invite(&id)?;
    let body: OpenBody = json_body(body)?;
    let signature = unhex::<64>(&body.signature)
        .and_then(|bytes| Signature::from_bytes(&bytes).ok())
        .ok_or_else(|| HttpError::bad_request("bad_request", "the signature is malformed"))?;
    let now = state.now_ms();
    let opened = state
        .db(move |store| store.invite_open(invite, &signature, now))
        .await?;
    span_feed(opened.feed);
    let log: Vec<serde_json::Value> = opened
        .log
        .iter()
        .map(|item| json!({ "cursor": item.cursor, "bytes": base64(&item.bytes) }))
        .collect();
    Ok(Json(json!({ "feedId": opened.feed, "log": log })).into_response())
}

// ----- the static page of the link flow -----------------------------------------------------

const JOIN_HTML: &str = include_str!("join.html");
const JOIN_JS: &str = include_str!("join.js");
const JOIN_CSS: &str = include_str!("join.css");

/// What every `/join` file is served with: nothing but its own script and style, no referrer, no
/// framing, no caching (the secret is in the fragment, which never reaches the server).
fn static_page(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'none'; script-src 'self'; style-src 'self'; base-uri 'none'; \
                     form-action 'none'; frame-ancestors 'none'",
                ),
            ),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response()
}

async fn join_page() -> Response {
    static_page("text/html; charset=utf-8", JOIN_HTML)
}

async fn join_script() -> Response {
    static_page("text/javascript; charset=utf-8", JOIN_JS)
}

async fn join_style() -> Response {
    static_page("text/css; charset=utf-8", JOIN_CSS)
}

// ----- router -------------------------------------------------------------------------------

/// The `/v2` routes and the `/join` page (the state and the tracing are the caller's).
pub(crate) fn routes() -> Router<Arc<AppState>> {
    let json_limit = DefaultBodyLimit::max(JSON_BODY_LIMIT);
    Router::new()
        .route("/v2/server", get(server_info))
        .route("/v2/accounts", post(create_account).layer(json_limit))
        .route("/v2/accounts/logs", post(logs_batch).layer(json_limit))
        .route(
            "/v2/accounts/{id}/log",
            post(crate::api::append_log)
                .layer(json_limit)
                .get(fetch_log),
        )
        .route(
            "/v2/auth/challenge",
            post(crate::api::challenge).layer(json_limit),
        )
        .route(
            "/v2/auth/session",
            post(crate::api::session).layer(json_limit),
        )
        .route("/v2/feeds", post(feeds_create).get(feeds_list))
        .route(
            "/v2/feeds/{feed}/membership",
            post(membership_append).get(membership_get),
        )
        .route("/v2/feeds/{feed}/entries", post(append_entry))
        .route("/v2/feeds/{feed}/changes", get(changes))
        .route("/v2/feeds/{feed}/snapshots", get(all_snapshots))
        .route(
            "/v2/feeds/{feed}/snapshots/{doc}",
            put(put_snapshot).get(get_snapshot),
        )
        .route(
            "/v2/feeds/{feed}/snapshots/{doc}/dispute",
            post(dispute_snapshot).layer(json_limit),
        )
        .route("/v2/feeds/{feed}/snapshot-marks", get(snapshot_marks))
        .route("/v2/feeds/{feed}/heads", put(put_head).get(get_heads))
        .route(
            "/v2/feeds/{feed}/chunks/{hash}",
            put(put_chunk).get(get_chunk).delete(delete_chunk),
        )
        .route("/v2/feeds/{feed}/chunks/{hash}/copy", post(copy_chunk))
        .route("/v2/invites/{id}/open", post(invite_open).layer(json_limit))
        // View links (M13 slice 4, `links.rs`) and the viewer's files (`viewer.rs`, outside `/v2`).
        .route(
            "/v2/links/{id}",
            put(crate::links::put)
                .layer(DefaultBodyLimit::max(crate::links::upload_limit()))
                .get(crate::links::get)
                .delete(crate::links::delete),
        )
        .route("/v2/links/{id}/chunks/{hash}", get(crate::links::chunk))
        .route("/view", get(crate::viewer::index))
        .route("/view/", get(crate::viewer::index))
        .route("/view/{*path}", get(crate::viewer::file))
        .route("/v2/ws", get(crate::api::websocket))
        .route("/v2/pair", post(crate::pairing::create).layer(json_limit))
        .route(
            "/v2/pair/{id}/messages",
            get(crate::pairing::messages)
                .post(crate::pairing::post)
                .layer(json_limit),
        )
        .route("/v2/pair/{id}", delete(crate::pairing::close))
        .route(
            "/v2/recovery/{id}",
            put(crate::recovery::put)
                .layer(DefaultBodyLimit::max(RECOVERY_BLOB_MAX_BYTES))
                .get(crate::recovery::get)
                .delete(crate::recovery::delete),
        )
        .route("/join", get(join_page))
        .route("/join.js", get(join_script))
        .route("/join.css", get(join_style))
}
