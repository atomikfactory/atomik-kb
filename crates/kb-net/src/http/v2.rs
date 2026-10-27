//! [`HttpRemote`] speaks protocol v2 (M13, ADR-0045, `docs/security/sharing.md` section 6):
//! `impl kb_sync::v2::RemoteStore for HttpRemote`, against the `/v2` routes of `kb-server`. The
//! session is a bearer token from `POST /v2/auth/session`, kept inside the remote (a 401 drops it).
//!
//! Wire shapes: control is camelCase JSON (ids as UUID text, hashes and signatures as hex, log and
//! membership entries as base64), payloads (entries, snapshots, heads, chunks, pages) are
//! `application/octet-stream`; `204` is "a member's feed holds nothing under that key"
//! (`get_snapshot`, `get_chunk`), so a `404` always means "not a member, or no such feed".
//!
//! Status mapping ([`status_error`], the table of `milestone-13.md` 1.1): 401 `Unauthorized` (code
//! `idle`: `Idle`), 403 `Forbidden` (code `authentication`: `Authentication`; `forbidden` and
//! `signup_limit`: `Forbidden`), 404 `NotFound`, 409 by code (`rekey_needed` `RekeyNeeded`,
//! `stale_epoch` `StaleEpoch`, `feed_closed` `Closed`, otherwise `Conflict`), 400/413/422 `Protocol`,
//! 429 `Unavailable` (never `Quota`), 507 `Quota`, 5xx and transport failures `Unavailable`. (`link_put` also
//! reads a 413 as `Quota`, the word of `RemoteStore::link_put` for a publication over the limit.)
//!
//! Nothing here logs a token, a nonce, a signature, a body or a server message.

use kb_crypto::Signature;
use kb_sync::v2::{
    FeedCreated, FeedListing, InviteOpened, LinkFetch, LinkPut, LogChunk, LogRequest, MarksPage,
    MembershipHead, MembershipItem, MembershipPage, RemoteStore, ServerInfo, SnapshotMark,
};
use kb_sync::{ChangesPage, PairingPage, SnapshotsPage, SyncError};
use kb_types::devices::{MAILBOX_ID_LEN, PAIRING_POLL_SECONDS, RECOVERY_BLOB_MAX_BYTES};
use kb_types::sync::{FeedKind, MAX_WIRE_BYTES};
use kb_types::{AccountId, ChunkId, DeviceId, DocId, FeedId, InviteId, LinkId, MemberRole};
use reqwest::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::{
    HttpRemote, MAX_CHANGES_PAGE_BYTES, MAX_ERROR_BYTES, MAX_ERROR_MESSAGE,
    MAX_SNAPSHOTS_PAGE_BYTES, OCTET_STREAM, check_pairing_message, check_recovery_blob,
    check_upload, read_capped, read_json, transport_error,
};
use crate::codec::{
    ErrorJson, PairCreateJson, PairCreatedJson, PairPageJson, PairPostJson, RoleJson,
    base64_decode, base64_encode, hex_decode, hex_encode,
};

/// The error a failed HTTP status stands for in protocol v2. `code` is the server's `error` field
/// (`rekey_needed`, `forbidden`, ...), `message` its explanation, kept for `Conflict` and `Protocol`
/// only.
#[must_use]
pub fn status_error(status: u16, code: Option<&str>, message: Option<String>) -> SyncError {
    let explained = |fallback: String| message.clone().unwrap_or(fallback);
    match status {
        401 => {
            if code == Some("idle") {
                SyncError::Idle
            } else {
                SyncError::Unauthorized
            }
        }
        403 => {
            if code == Some("authentication") {
                SyncError::Authentication
            } else {
                SyncError::Forbidden
            }
        }
        404 => SyncError::NotFound,
        409 => match code {
            Some("rekey_needed") => SyncError::RekeyNeeded,
            Some("stale_epoch") => SyncError::StaleEpoch,
            Some("feed_closed") => SyncError::Closed,
            _ => SyncError::Conflict(explained("the server refused a conflicting change".into())),
        },
        400 | 413 | 422 => SyncError::Protocol(explained(format!(
            "the server refused the request (status {status})"
        ))),
        429 => SyncError::Unavailable("the server asked to slow down".to_owned()),
        507 => SyncError::Quota,
        500..=599 => SyncError::Unavailable(format!("the server failed (status {status})")),
        other => SyncError::Protocol(format!(
            "unexpected answer from the server (status {other})"
        )),
    }
}

/// The response when its status is a success, else the error of [`status_error`].
pub(super) async fn success(response: Response) -> Result<Response, SyncError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = match read_capped(response, MAX_ERROR_BYTES).await {
        Ok(bytes) => serde_json::from_slice::<ErrorJson>(&bytes).unwrap_or_default(),
        Err(_) => ErrorJson::default(),
    };
    let message = body.message.map(|text| {
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_ERROR_MESSAGE)
            .collect();
        clean.trim().to_owned()
    });
    Err(status_error(
        status.as_u16(),
        body.error.as_deref(),
        message.filter(|text| !text.is_empty()),
    ))
}

// ----- JSON bodies ----------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerInfoJson {
    protocol_version: u8,
    server_epoch: String,
    max_wire_bytes: u64,
    #[serde(default)]
    account_quota_bytes: Option<u64>,
    #[serde(default)]
    idle_device_days: Option<u32>,
    #[serde(default)]
    share_signups: u32,
    #[serde(default)]
    share_signups_per_account: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAccountJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    invite_token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    share: Option<ShareClaimJson>,
    log: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareClaimJson {
    invite_id: InviteId,
    signature: String,
}

#[derive(Deserialize)]
struct LogJson {
    entries: Vec<String>,
}

#[derive(Serialize)]
struct LogBody {
    entries: Vec<String>,
}

#[derive(Serialize)]
struct LogsRequestJson {
    accounts: Vec<LogsAskJson>,
}

#[derive(Serialize)]
struct LogsAskJson {
    id: AccountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<u64>,
}

#[derive(Deserialize)]
struct LogsAnswerJson {
    logs: Vec<LogChunkJson>,
}

#[derive(Deserialize)]
struct LogChunkJson {
    account: AccountId,
    entries: Vec<String>,
}

#[derive(Deserialize)]
struct HeadJson {
    seq: u64,
    hash: String,
}

impl HeadJson {
    fn into_head(self) -> Result<MembershipHead, SyncError> {
        Ok(MembershipHead {
            seq: self.seq,
            hash: hex_decode::<32>("hash", &self.hash)?,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedCreatedJson {
    feed_id: FeedId,
    head: HeadJson,
    cursor: u64,
}

#[derive(Deserialize)]
struct FeedsJson {
    feeds: Vec<FeedJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedJson {
    feed_id: FeedId,
    kind: String,
    #[serde(default)]
    role: Option<String>,
    removed: bool,
    closed: bool,
}

#[derive(Deserialize)]
struct ItemJson {
    cursor: u64,
    bytes: String,
}

impl ItemJson {
    fn into_item(self) -> Result<MembershipItem, SyncError> {
        Ok(MembershipItem {
            cursor: self.cursor,
            bytes: base64_decode("bytes", &self.bytes)?,
        })
    }
}

#[derive(Deserialize)]
struct MembershipPageJson {
    entries: Vec<ItemJson>,
    head: HeadJson,
    more: bool,
}

#[derive(Deserialize)]
struct MarksJson {
    marks: Vec<MarkJson>,
    more: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkJson {
    mark: u64,
    doc_id: DocId,
    device_id: DeviceId,
    account_id: AccountId,
    covers_cursor: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DisputeJson {
    covers_cursor: u64,
}

#[derive(Serialize)]
struct OpenJson {
    signature: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenedJson {
    feed_id: FeedId,
    log: Vec<ItemJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppendedJson {
    cursor: u64,
    chain_hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PubSeqJson {
    pub_seq: u64,
}

#[derive(Deserialize)]
struct HeadsJson {
    heads: Vec<String>,
}

/// The `pub_seq` an `ETag` header carries: `"N"`, with or without a `W/` prefix.
fn etag_seq(text: &str) -> Option<u64> {
    let text = text.trim();
    let text = text.strip_prefix("W/").unwrap_or(text);
    text.trim_matches('"').parse().ok()
}

fn decode_list(field: &str, items: &[String]) -> Result<Vec<Vec<u8>>, SyncError> {
    items
        .iter()
        .map(|item| base64_decode(field, item))
        .collect()
}

fn role_of(name: &str) -> Result<MemberRole, SyncError> {
    match name {
        "owner" => Ok(MemberRole::Owner),
        "editor" => Ok(MemberRole::Editor),
        "commenter" => Ok(MemberRole::Commenter),
        "viewer" => Ok(MemberRole::Viewer),
        _ => Err(SyncError::Encoding(
            "a role of the server is unknown".to_owned(),
        )),
    }
}

impl HttpRemote {
    fn v2(&self, path: &str) -> String {
        self.url(&format!("/v2{path}"))
    }

    fn feed_url(&self, feed: FeedId, tail: &str) -> String {
        self.v2(&format!("/feeds/{feed}{tail}"))
    }

    /// A `GET` of a binary object a member's feed may not hold (`204`).
    async fn get_optional(&self, url: &str, cap: u64) -> Result<Option<Vec<u8>>, SyncError> {
        let response = self.send_authed(|| self.client.get(url)).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        read_capped(success(response).await?, cap).await.map(Some)
    }

    /// A `PUT` of a binary object, refused locally when it is too large.
    async fn put_binary(&self, url: &str, bytes: &[u8]) -> Result<(), SyncError> {
        check_upload(bytes)?;
        let response = self
            .send_authed(|| {
                self.client
                    .put(url)
                    .header(CONTENT_TYPE, OCTET_STREAM)
                    .body(bytes.to_vec())
            })
            .await?;
        success(response).await.map(drop)
    }

    /// A `POST` of JSON that needs the session and answers a body.
    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &impl Serialize,
    ) -> Result<T, SyncError> {
        let response = self
            .send_authed(|| self.client.post(url).json(body))
            .await?;
        read_json(success(response).await?).await
    }

    /// A `GET` of JSON that needs the session.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, SyncError> {
        let response = self.send_authed(|| self.client.get(url)).await?;
        read_json(success(response).await?).await
    }

    fn mailbox_url(&self, id: [u8; MAILBOX_ID_LEN], tail: &str) -> String {
        self.v2(&format!("/pair/{}{tail}", hex_encode(&id)))
    }

    fn recovery_url(&self, id: [u8; 32]) -> String {
        self.v2(&format!("/recovery/{}", hex_encode(&id)))
    }
}

impl RemoteStore for HttpRemote {
    async fn server_info(&self) -> Result<ServerInfo, SyncError> {
        let response = self
            .client
            .get(self.v2("/server"))
            .send()
            .await
            .map_err(transport_error)?;
        let info: ServerInfoJson = read_json(success(response).await?).await?;
        Ok(ServerInfo {
            protocol_version: info.protocol_version,
            server_epoch: hex_decode("serverEpoch", &info.server_epoch)?,
            max_wire_bytes: info.max_wire_bytes,
            account_quota_bytes: info.account_quota_bytes,
            idle_device_days: info.idle_device_days,
            share_signups: info.share_signups,
            share_signups_per_account: info.share_signups_per_account,
        })
    }

    async fn create_account(&self, invite_token: &str, log: &[Vec<u8>]) -> Result<(), SyncError> {
        let body = CreateAccountJson {
            invite_token: Some(invite_token),
            share: None,
            log: log.iter().map(|e| base64_encode(e)).collect(),
        };
        let response = self
            .client
            .post(self.v2("/accounts"))
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        success(response).await.map(drop)
    }

    async fn account_create_share(
        &self,
        log: &[Vec<u8>],
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<(), SyncError> {
        let body = CreateAccountJson {
            invite_token: None,
            share: Some(ShareClaimJson {
                invite_id,
                signature: hex_encode(&sig.to_bytes()),
            }),
            log: log.iter().map(|e| base64_encode(e)).collect(),
        };
        let response = self
            .client
            .post(self.v2("/accounts"))
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        success(response).await.map(drop)
    }

    async fn append_log(&self, account: AccountId, entries: &[Vec<u8>]) -> Result<(), SyncError> {
        // A device that joined through the kit publishes its own `DeviceAdded` here before it can
        // authenticate: the log is verified by its signatures, not by a session.
        let body = LogBody {
            entries: entries.iter().map(|e| base64_encode(e)).collect(),
        };
        let response = self
            .send(
                self.client
                    .post(self.v2(&format!("/accounts/{account}/log")))
                    .json(&body),
            )
            .await?;
        success(response).await.map(drop)
    }

    async fn fetch_log(
        &self,
        account: AccountId,
        after: Option<u64>,
    ) -> Result<Vec<Vec<u8>>, SyncError> {
        let query = after.map_or_else(String::new, |after| format!("?after={after}"));
        let response = self
            .send(
                self.client
                    .get(self.v2(&format!("/accounts/{account}/log{query}"))),
            )
            .await?;
        let log: LogJson = read_json(success(response).await?).await?;
        decode_list("entries", &log.entries)
    }

    async fn logs_batch(&self, requests: &[LogRequest]) -> Result<Vec<LogChunk>, SyncError> {
        let body = LogsRequestJson {
            accounts: requests
                .iter()
                .map(|request| LogsAskJson {
                    id: request.account,
                    after: request.after,
                })
                .collect(),
        };
        let response = self
            .send(self.client.post(self.v2("/accounts/logs")).json(&body))
            .await?;
        let answer: LogsAnswerJson = read_json(success(response).await?).await?;
        answer
            .logs
            .into_iter()
            .map(|chunk| {
                Ok(LogChunk {
                    account: chunk.account,
                    entries: decode_list("entries", &chunk.entries)?,
                })
            })
            .collect()
    }

    async fn authenticate(
        &self,
        account: AccountId,
        device: DeviceId,
        sign: &dyn Fn(&[u8]) -> Signature,
    ) -> Result<(), SyncError> {
        self.forget_session();
        let (nonce, epoch) = self.challenge(account, device).await?;
        let signature = sign(&kb_sync::auth_message(&nonce, &epoch));
        self.session(account, device, &nonce, &signature).await
    }

    async fn feeds_create(&self, create: &[u8]) -> Result<FeedCreated, SyncError> {
        check_upload(create)?;
        let url = self.v2("/feeds");
        let response = self
            .send_authed(|| {
                self.client
                    .post(&url)
                    .header(CONTENT_TYPE, OCTET_STREAM)
                    .body(create.to_vec())
            })
            .await?;
        let created: FeedCreatedJson = read_json(success(response).await?).await?;
        Ok(FeedCreated {
            feed: created.feed_id,
            head: created.head.into_head()?,
            cursor: created.cursor,
        })
    }

    async fn feeds_list(&self) -> Result<Vec<FeedListing>, SyncError> {
        let listing: FeedsJson = self.get_json(&self.v2("/feeds")).await?;
        listing
            .feeds
            .into_iter()
            .map(|feed| {
                Ok(FeedListing {
                    feed: feed.feed_id,
                    kind: match feed.kind.as_str() {
                        "personal" => FeedKind::Personal,
                        "shared" => FeedKind::Shared,
                        _ => {
                            return Err(SyncError::Encoding(
                                "a feed kind of the server is unknown".to_owned(),
                            ));
                        }
                    },
                    role: feed.role.as_deref().map(role_of).transpose()?,
                    removed: feed.removed,
                    closed: feed.closed,
                })
            })
            .collect()
    }

    async fn membership_append(
        &self,
        feed: FeedId,
        entries: &[Vec<u8>],
    ) -> Result<MembershipHead, SyncError> {
        let body = LogBody {
            entries: entries.iter().map(|e| base64_encode(e)).collect(),
        };
        // A joiner that is not a member yet posts its `Join` here too: the session is sent when
        // one is held (the server judges the entry, not the session's membership).
        let url = self.feed_url(feed, "/membership");
        let head: HeadJson = self.post_json(&url, &body).await?;
        head.into_head()
    }

    async fn membership_get(
        &self,
        feed: FeedId,
        after_seq: Option<u64>,
    ) -> Result<MembershipPage, SyncError> {
        let query = after_seq.map_or_else(String::new, |after| format!("?after={after}"));
        let page: MembershipPageJson = self
            .get_json(&self.feed_url(feed, &format!("/membership{query}")))
            .await?;
        Ok(MembershipPage {
            entries: page
                .entries
                .into_iter()
                .map(ItemJson::into_item)
                .collect::<Result<_, _>>()?,
            head: page.head.into_head()?,
            more: page.more,
        })
    }

    async fn append(&self, feed: FeedId, entry: &[u8]) -> Result<(u64, [u8; 32]), SyncError> {
        check_upload(entry)?;
        let url = self.feed_url(feed, "/entries");
        let response = self
            .send_authed(|| {
                self.client
                    .post(&url)
                    .header(CONTENT_TYPE, OCTET_STREAM)
                    .body(entry.to_vec())
            })
            .await?;
        let appended: AppendedJson = read_json(success(response).await?).await?;
        Ok((
            appended.cursor,
            hex_decode::<32>("chainHash", &appended.chain_hash)?,
        ))
    }

    async fn changes(
        &self,
        feed: FeedId,
        after: u64,
        limit: u32,
    ) -> Result<ChangesPage, SyncError> {
        let url = self.feed_url(feed, &format!("/changes?after={after}&limit={limit}"));
        let response = self.send_authed(|| self.client.get(&url)).await?;
        let bytes = read_capped(success(response).await?, MAX_CHANGES_PAGE_BYTES).await?;
        ChangesPage::decode(&bytes)
    }

    async fn put_snapshot(&self, feed: FeedId, snapshot: &[u8]) -> Result<(), SyncError> {
        let header = kb_sync::v2::peek_snapshot(snapshot)?;
        let url = self.feed_url(feed, &format!("/snapshots/{}", header.doc_id));
        self.put_binary(&url, snapshot).await
    }

    async fn get_snapshot(&self, feed: FeedId, doc: DocId) -> Result<Option<Vec<u8>>, SyncError> {
        let url = self.feed_url(feed, &format!("/snapshots/{doc}"));
        self.get_optional(&url, MAX_WIRE_BYTES).await
    }

    async fn snapshots(&self, feed: FeedId) -> Result<SnapshotsPage, SyncError> {
        let url = self.feed_url(feed, "/snapshots");
        let response = self.send_authed(|| self.client.get(&url)).await?;
        let bytes = read_capped(success(response).await?, MAX_SNAPSHOTS_PAGE_BYTES).await?;
        SnapshotsPage::decode(&bytes)
    }

    async fn snapshot_marks(&self, feed: FeedId, after: u64) -> Result<MarksPage, SyncError> {
        let page: MarksJson = self
            .get_json(&self.feed_url(feed, &format!("/snapshot-marks?after={after}")))
            .await?;
        Ok(MarksPage {
            marks: page
                .marks
                .into_iter()
                .map(|mark| SnapshotMark {
                    mark: mark.mark,
                    doc_id: mark.doc_id,
                    device_id: mark.device_id,
                    account_id: mark.account_id,
                    covers_cursor: mark.covers_cursor,
                })
                .collect(),
            more: page.more,
        })
    }

    async fn snapshot_dispute(
        &self,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
    ) -> Result<(), SyncError> {
        let url = self.feed_url(feed, &format!("/snapshots/{doc}/dispute"));
        let body = DisputeJson { covers_cursor };
        let response = self
            .send_authed(|| self.client.post(&url).json(&body))
            .await?;
        success(response).await.map(drop)
    }

    async fn put_head(&self, feed: FeedId, head: &[u8]) -> Result<(), SyncError> {
        self.put_binary(&self.feed_url(feed, "/heads"), head).await
    }

    async fn heads(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, SyncError> {
        let heads: HeadsJson = self.get_json(&self.feed_url(feed, "/heads")).await?;
        decode_list("heads", &heads.heads)
    }

    async fn put_chunk(&self, feed: FeedId, hash: ChunkId, bytes: &[u8]) -> Result<(), SyncError> {
        let url = self.feed_url(feed, &format!("/chunks/{}", hash.to_hex()));
        self.put_binary(&url, bytes).await
    }

    async fn get_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<Option<Vec<u8>>, SyncError> {
        let url = self.feed_url(feed, &format!("/chunks/{}", hash.to_hex()));
        self.get_optional(&url, MAX_WIRE_BYTES).await
    }

    async fn delete_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError> {
        let url = self.feed_url(feed, &format!("/chunks/{}", hash.to_hex()));
        let response = self.send_authed(|| self.client.delete(&url)).await?;
        success(response).await.map(drop)
    }

    async fn copy_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError> {
        let url = self.feed_url(feed, &format!("/chunks/{}/copy", hash.to_hex()));
        let response = self.send_authed(|| self.client.post(&url)).await?;
        success(response).await.map(drop)
    }

    async fn invite_open(
        &self,
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<InviteOpened, SyncError> {
        let body = OpenJson {
            signature: hex_encode(&sig.to_bytes()),
        };
        let response = self
            .client
            .post(self.v2(&format!("/invites/{invite_id}/open")))
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        let opened: OpenedJson = read_json(success(response).await?).await?;
        Ok(InviteOpened {
            feed: opened.feed_id,
            log: opened
                .log
                .into_iter()
                .map(ItemJson::into_item)
                .collect::<Result<_, _>>()?,
        })
    }

    // ---- view links (M13 slice 4): `PUT`, `GET` and `DELETE /v2/links/{id}` and `GET
    // /v2/links/{id}/chunks/{hash}`; the wire is in `kb_sync::v2::publication`'s module documentation
    // and `milestone-13.md` 4.1 (contract notes). `link_get` sends `If-None-Match: "<pub_seq>"` and reads
    // the `ETag`; both public reads send no bearer token (and the reads of a viewer must not get one). ----

    async fn link_put(&self, link_id: LinkId, put: &LinkPut) -> Result<u64, SyncError> {
        let body = kb_sync::v2::encode_link_put(put)?;
        // A publication over the server's limit is `Quota` (the trait's word for it), locally and when
        // the server's 413 says so.
        if put.publication.len() as u64 > MAX_WIRE_BYTES {
            return Err(SyncError::Quota);
        }
        let url = self.v2(&format!("/links/{link_id}"));
        let response = self
            .send_authed(|| {
                self.client
                    .put(&url)
                    .header(CONTENT_TYPE, OCTET_STREAM)
                    .body(body)
            })
            .await?;
        if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return Err(SyncError::Quota);
        }
        let answer: PubSeqJson = read_json(success(response).await?).await?;
        Ok(answer.pub_seq)
    }

    async fn link_get(
        &self,
        link_id: LinkId,
        if_none_match: Option<u64>,
    ) -> Result<LinkFetch, SyncError> {
        // No bearer token: the read is public, and a viewer has none.
        let mut request = self.client.get(self.v2(&format!("/links/{link_id}")));
        if let Some(pub_seq) = if_none_match {
            request = request.header(IF_NONE_MATCH, format!("\"{pub_seq}\""));
        }
        let response = request.send().await.map_err(transport_error)?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(LinkFetch::NotModified);
        }
        let response = success(response).await?;
        let pub_seq = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .and_then(etag_seq)
            .ok_or_else(|| SyncError::Protocol("the server sent no usable ETag".to_owned()))?;
        let bytes = read_capped(response, MAX_WIRE_BYTES).await?;
        Ok(LinkFetch::Publication { pub_seq, bytes })
    }

    async fn link_delete(&self, link_id: LinkId) -> Result<(), SyncError> {
        let url = self.v2(&format!("/links/{link_id}"));
        let response = self.send_authed(|| self.client.delete(&url)).await?;
        success(response).await.map(drop)
    }

    async fn link_chunk_get(&self, link_id: LinkId, hash: ChunkId) -> Result<Vec<u8>, SyncError> {
        let url = self.v2(&format!("/links/{link_id}/chunks/{}", hash.to_hex()));
        let response = self.client.get(url).send().await.map_err(transport_error)?;
        read_capped(success(response).await?, MAX_WIRE_BYTES).await
    }

    async fn pair_create(&self, offer: &[u8]) -> Result<([u8; 6], i64), SyncError> {
        check_pairing_message(offer)?;
        let body = PairCreateJson {
            offer: base64_encode(offer),
        };
        let response = self
            .client
            .post(self.v2("/pair"))
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        let created: PairCreatedJson = read_json(success(response).await?).await?;
        Ok((
            hex_decode::<MAILBOX_ID_LEN>("mailboxId", &created.mailbox_id)?,
            created.expires_at,
        ))
    }

    async fn pair_messages(
        &self,
        id: [u8; 6],
        after: u32,
        wait_seconds: u32,
    ) -> Result<PairingPage, SyncError> {
        let wait = wait_seconds.min(PAIRING_POLL_SECONDS);
        let url = self.mailbox_url(id, &format!("/messages?after={after}&wait={wait}"));
        let response = self.client.get(url).send().await.map_err(transport_error)?;
        let page: PairPageJson = read_json(success(response).await?).await?;
        page.into_page()
    }

    async fn pair_post(&self, id: [u8; 6], role: u8, bytes: &[u8]) -> Result<(), SyncError> {
        check_pairing_message(bytes)?;
        let body = PairPostJson {
            role: RoleJson::from_byte(role)?,
            bytes: base64_encode(bytes),
        };
        let response = self
            .client
            .post(self.mailbox_url(id, "/messages"))
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        success(response).await.map(drop)
    }

    async fn pair_close(&self, id: [u8; 6]) -> Result<(), SyncError> {
        let response = self
            .client
            .delete(self.mailbox_url(id, ""))
            .send()
            .await
            .map_err(transport_error)?;
        success(response).await.map(drop)
    }

    async fn recovery_put(&self, id: [u8; 32], bytes: &[u8]) -> Result<(), SyncError> {
        check_recovery_blob(bytes)?;
        self.put_binary(&self.recovery_url(id), bytes).await
    }

    async fn recovery_get(&self, id: [u8; 32]) -> Result<Option<Vec<u8>>, SyncError> {
        let response = self
            .client
            .get(self.recovery_url(id))
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        read_capped(success(response).await?, RECOVERY_BLOB_MAX_BYTES as u64)
            .await
            .map(Some)
    }

    async fn recovery_delete(&self, id: [u8; 32]) -> Result<(), SyncError> {
        let url = self.recovery_url(id);
        let response = self.send_authed(|| self.client.delete(&url)).await?;
        // Nothing stored under the id: deleted already.
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        success(response).await.map(drop)
    }
}
