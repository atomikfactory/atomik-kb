//! The feed registry and the per-feed sync methods (M13 slice 1, ADR-0045,
//! `docs/development/milestone-13.md` section 1.5): the feeds this device syncs (`sync_feeds` table),
//! the routing of documents and keyring items to feeds, the per-feed forms of the v1 host methods
//! (outbox, apply, snapshots, heads, chunk work), the co-members' device logs, and the snapshot marks
//! that drive the dominance check. Every method mirrors one of `kb-net`'s `SyncHost` (v2) one to one,
//! as v1's do.
//!
//! **Feed routing.** A document's feed is its space's feed (a personal space and the account
//! document: the personal feed); the device log, `Space(epoch)` records and the personal keyring: the
//! personal feed; `Doc`, `Attachment` and tombstone records of a shared space: that feed's keyring.
//! The outbox, `device_seq` and the snapshot bookkeeping are per feed.
//!
//! **Apply path** for a shared feed's items, in order (ADR-0043): the chain; membership at the item's
//! cursor (not a member: quarantine, flag the server); the header signature by a device of the header
//! account (the core's own log or `foreign_device_logs`); the role gating of ADR-0043 (quarantine,
//! the account counted in `refused_changes`); decrypt with the document key resolved from the key
//! epoch; the decrypted kind must agree with the document id; apply; update `sync_entry_marks`; verify
//! snapshot dominance before acknowledging past `covers`. The chain advances in every case.
//!
//! **Names.** The v1 methods of the personal feed keep their names; the per-feed forms are
//! `sync_feed_*` (a leading `feed: FeedId`), the co-members' logs `sync_needed_logs` and
//! `sync_foreign_log_received`. When v1 is retired the architect may drop the `feed_` infix.
//!
//! **Where the code is**: this file holds the registry, the heads and the chunk work; `feeds/push.rs`
//! the outbox, the snapshots made here and the snapshot marks; `feeds/pull.rs` the apply path.
//!
//! **The driver's cycle** (what a host does with these methods, for one shared feed; the personal
//! feed skips the membership steps):
//!
//! 1. `sync_feeds` lists the feeds (personal first); `sync_needed_logs`, `logs_batch` and
//!    `sync_foreign_log_received` bring the co-members' logs before the shared feeds are worked.
//! 2. `sync_membership_pending`: post the entries (the first with `seq` 0 through `feeds_create`,
//!    the rest through `membership_append`), then `sync_membership_posted`; `Conflict` on either
//!    is `sync_membership_conflict`. `sync_membership_wanted` asks for `membership_get`, answered
//!    through `sync_membership_fetched`.
//! 3. `sync_feed_outbox`: `append` every entry, `sync_feed_pushed`; `RekeyNeeded` is
//!    `sync_feed_rekey_needed` and ends the push; `StaleEpoch` ends the push (the pull teaches the
//!    new epoch, then the outbox is rebuilt and resealed); `Forbidden` is `sync_feed_refused` and the
//!    pull still runs (it teaches a demotion); `Closed` ends the push.
//! 4. Snapshots: `sync_feed_snapshot_candidates`, `sync_feed_make_snapshot`, `put_snapshot`,
//!    `sync_feed_snapshot_published`.
//! 5. The pull: `changes` pages through `sync_feed_apply`. A page whose `new_cursor` stays below
//!    its last cursor stalled for a device log: fetch `sync_needed_logs` again and offer the same
//!    page once more (items at or below the cursor are skipped). `gaps` and a cursor of 0 ask for
//!    `snapshots`, applied through `sync_feed_apply_snapshots` (a shared feed judges them with the
//!    membership known after the pull: they are parked until it is).
//! 6. Marks: `sync_snapshot_marks_wanted`, `snapshot_marks`, `sync_snapshot_marks_received`,
//!    `get_snapshot` of each document, `sync_feed_apply_snapshots`, a `snapshot_dispute` for each
//!    `disputed` answer, `sync_snapshot_verified`.
//! 7. Chunks (`sync_feed_chunk_*`) and the head (`sync_feed_head`, `put_head`, `heads`,
//!    `sync_feed_heads_received`).
//!
//! A `404` on a feed is `sync_feed_lost`.

mod pull;
mod push;

use std::collections::HashMap;

use kb_storage::{
    AttachmentQueries, ChunkSyncQueries, DocKindCode, DocQueries, DocRow, FeedQueries,
    SecurityQueries, SyncFeedRow, SyncHeadRow, SyncQueries,
};
use kb_sync::v2::{FeedChunkKind, FeedChunkWork, FeedWork};
use kb_types::sync::FeedKind;
use kb_types::{
    AccountId, ApiError, AttachmentId, ChunkId, DeviceId, DocId, FeedId, MemberRole, PageId,
    SpaceId,
};

use crate::security::{decode_hex, hex};
use crate::transfers::{COPY, DELETE, DOWNLOAD, UPLOAD};
use crate::{Core, map_storage_error};

/// How many `(cursor, chain hash)` pairs a feed keeps for head comparisons.
const CHAIN_HISTORY_LEN: usize = 256;

/// The most chunk rows one call of `sync_feed_chunk_work` reads while it looks for a feed's own.
const CHUNK_WINDOW_MAX: u32 = 16_384;

/// A shared space of this device: its feed and whether it is an archive.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SharedSpace {
    pub(crate) feed: FeedId,
    pub(crate) archived: bool,
}

/// The `(cursor, chain hash)` pairs a feed row keeps (the text format of v1).
pub(crate) fn chain_history_of(row: &SyncFeedRow) -> Vec<(u64, [u8; 32])> {
    let Some(bytes) = &row.chain_history else {
        return Vec::new();
    };
    String::from_utf8_lossy(bytes)
        .split(',')
        .filter_map(|pair| {
            let (cursor, hash) = pair.split_once(':')?;
            let hash: [u8; 32] = decode_hex(hash)?.try_into().ok()?;
            Some((cursor.parse().ok()?, hash))
        })
        .collect()
}

/// Records the chain hash at a cursor in a feed row and in its bounded history.
pub(crate) fn store_chain(row: &mut SyncFeedRow, cursor: u64, chain: &[u8; 32]) {
    row.chain_hash = Some(*chain);
    let mut history = chain_history_of(row);
    history.retain(|(c, _)| *c != cursor);
    history.push((cursor, *chain));
    if history.len() > CHAIN_HISTORY_LEN {
        let drop = history.len() - CHAIN_HISTORY_LEN;
        history.drain(..drop);
    }
    let encoded: Vec<String> = history
        .iter()
        .map(|(c, h)| format!("{c}:{}", hex(h)))
        .collect();
    row.chain_history = Some(encoded.join(",").into_bytes());
}

impl Core {
    // ----- the registry ---------------------------------------------------------------------

    /// The row of a feed (`NotFound` when this device does not sync it).
    pub(crate) fn feed_row(&self, feed: FeedId) -> Result<SyncFeedRow, ApiError> {
        self.db
            .sync_feed_get(feed)
            .map_err(map_storage_error)?
            .ok_or_else(|| ApiError::not_found("the feed is not known on this device"))
    }

    pub(crate) fn feed_row_put(&self, row: &SyncFeedRow) -> Result<(), ApiError> {
        self.db.sync_feed_put(row).map_err(map_storage_error)
    }

    /// The personal feed (the account id), once the kit exists.
    pub(crate) fn personal_feed(&self) -> Result<Option<FeedId>, ApiError> {
        Ok(self
            .sync_identity()?
            .map(|(account, _)| FeedId::from(account)))
    }

    /// A fresh row for a feed this device starts to sync.
    fn new_feed_row(
        &self,
        feed: FeedId,
        kind: FeedKind,
        space_id: Option<SpaceId>,
    ) -> Result<SyncFeedRow, ApiError> {
        Ok(SyncFeedRow {
            feed,
            kind,
            space_id,
            cursor: 0,
            chain_hash: None,
            max_covers: 0,
            chain_history: None,
            fork: false,
            server_epoch: self.server_epoch()?,
        })
    }

    /// The personal feed has a row once the account id exists.
    pub(crate) fn ensure_personal_feed_row(&self) -> Result<Option<FeedId>, ApiError> {
        let Some(feed) = self.personal_feed()? else {
            return Ok(None);
        };
        if self
            .db
            .sync_feed_get(feed)
            .map_err(map_storage_error)?
            .is_none()
        {
            let row = self.new_feed_row(feed, FeedKind::Personal, None)?;
            self.feed_row_put(&row)?;
        }
        Ok(Some(feed))
    }

    /// Starts to sync a shared feed (idempotent). `space_id` is `None` for a feed whose space is
    /// not known yet (the link flow's invitee): `feed_bind_space` completes it.
    pub(crate) fn feed_register_shared(
        &mut self,
        feed: FeedId,
        space_id: Option<SpaceId>,
    ) -> Result<(), ApiError> {
        if let Some(mut row) = self.db.sync_feed_get(feed).map_err(map_storage_error)? {
            if row.space_id.is_none() && space_id.is_some() {
                row.space_id = space_id;
                self.feed_row_put(&row)?;
            }
            return Ok(());
        }
        let row = self.new_feed_row(feed, FeedKind::Shared, space_id)?;
        self.feed_row_put(&row)?;
        self.shared.timelines.remove(&feed);
        Ok(())
    }

    /// The account document says which spaces are shared (`sharedSpaces`): the device that did not
    /// share them starts to sync their feeds.
    pub(crate) fn reconcile_shared_spaces(&mut self) -> Result<(), ApiError> {
        let Some(personal) = self.personal_feed()? else {
            return Ok(());
        };
        let listed: Vec<(SpaceId, FeedId)> = self
            .docs
            .account
            .as_ref()
            .map(|a| a.model.shared_spaces())
            .unwrap_or_default();
        for (space_id, feed) in listed {
            if feed == personal {
                continue;
            }
            if self
                .db
                .sync_feed_get(feed)
                .map_err(map_storage_error)?
                .is_none()
            {
                self.feed_register_shared(feed, Some(space_id))?;
                self.push_event_once(kb_types::Event::SpacesChanged);
            }
        }
        Ok(())
    }

    /// The feeds the driver works this cycle: the personal feed first (once the account id exists),
    /// then one per shared space that is not an archive, with the cursor each is at.
    pub fn sync_feeds(&mut self) -> Result<Vec<FeedWork>, ApiError> {
        self.shared_load()?;
        // What a `Leave` the server took leaves behind goes now, before any feed is worked.
        self.leave_finalize_all()?;
        if self.ensure_personal_feed_row()?.is_none() {
            return Ok(Vec::new());
        }
        self.reconcile_shared_spaces()?;
        // View links (M13 slice 4): an Owner's device tombstones the `Link` records a non-Owner creator
        // abandoned. Best effort: a failure here never stops the cycle.
        let _ = self.view_links_maintain();
        let mut out = Vec::new();
        for row in self.db.sync_feed_list().map_err(map_storage_error)? {
            if row.kind == FeedKind::Shared && self.feed_archived(row.feed)?.is_some() {
                continue;
            }
            let closed = row.kind == FeedKind::Shared
                && self
                    .shared
                    .timelines
                    .get(&row.feed)
                    .is_some_and(|t| t.state().closed.is_some());
            out.push(FeedWork {
                feed: row.feed,
                kind: row.kind,
                space_id: row.space_id,
                cursor: row.cursor,
                closed,
                forked: row.fork,
            });
        }
        Ok(out)
    }

    /// The shared spaces of this device by space id (archives included, flagged).
    pub(crate) fn shared_space_map(&self) -> Result<HashMap<SpaceId, SharedSpace>, ApiError> {
        let mut map = HashMap::new();
        for row in self.db.sync_feed_list().map_err(map_storage_error)? {
            if row.kind != FeedKind::Shared {
                continue;
            }
            if let Some(space_id) = row.space_id {
                map.insert(
                    space_id,
                    SharedSpace {
                        feed: row.feed,
                        archived: self.feed_archived(row.feed)?.is_some(),
                    },
                );
            }
        }
        Ok(map)
    }

    /// The feed a document travels in: the personal feed for the account document and for the
    /// documents of personal spaces, the space's feed for a shared one, `None` for an archive.
    pub(crate) fn feed_of_doc(
        row: &DocRow,
        shared: &HashMap<SpaceId, SharedSpace>,
        personal: FeedId,
    ) -> Option<FeedId> {
        if row.kind == DocKindCode::Account {
            return Some(personal);
        }
        match row.space_id.and_then(|space| shared.get(&space)) {
            Some(space) if space.archived => None,
            Some(space) => Some(space.feed),
            None => Some(personal),
        }
    }

    /// The feed a chunk travels in, from the space of its attachment (the personal feed when the
    /// attachment is unknown here). The chunks of a purged attachment of a shared space have no
    /// index row to ask any more: the purge noted the feed they are deleted from.
    fn feed_of_chunk(
        &self,
        attachment_id: AttachmentId,
        shared: &HashMap<SpaceId, SharedSpace>,
        personal: FeedId,
    ) -> Result<Option<FeedId>, ApiError> {
        let row = self
            .db
            .attachment_get(attachment_id)
            .map_err(map_storage_error)?;
        if row.is_none()
            && let Some(feed) = self
                .db
                .sync_config_get(&chunk_delete_key(attachment_id))
                .map_err(map_storage_error)?
                .and_then(|text| text.parse::<FeedId>().ok())
        {
            return Ok(Some(feed));
        }
        let space = row.and_then(|row| shared.get(&row.space_id).copied());
        Ok(match space {
            Some(space) if space.archived => None,
            Some(space) => Some(space.feed),
            None => Some(personal),
        })
    }

    /// This account's role in a shared feed, as the folded timeline says (`None` for the personal
    /// feed).
    pub(crate) fn feed_role(&mut self, feed: FeedId) -> Result<Option<MemberRole>, ApiError> {
        let me = self.me()?;
        Ok(self
            .shared
            .timelines
            .get(&feed)
            .and_then(|t| t.state().role_of(me)))
    }

    /// The page a document is, when it is a page document of this device.
    pub(crate) fn page_of_doc(&self, doc_id: DocId) -> Option<PageId> {
        let page_id = PageId::from_bytes(doc_id.to_bytes());
        self.space_of_page(page_id)
            .ok()
            .map(|_| page_id)
            // A comment stream is "about" its page.
            .or_else(|| self.comment_stream_page(None, doc_id).map(|(_, page)| page))
    }

    // ----- the snapshot disputes of a feed (Owners see them) --------------------------------

    /// The open disputes of a feed: `(document, account that published the snapshot, when)`.
    pub(crate) fn disputes_of(
        &self,
        feed: FeedId,
    ) -> Result<Vec<(DocId, AccountId, i64)>, ApiError> {
        Ok(self
            .fcfg(feed, "disputes")?
            .unwrap_or_default()
            .split(';')
            .filter_map(|item| {
                let mut parts = item.split(':');
                let doc: [u8; 16] = decode_hex(parts.next()?)?.try_into().ok()?;
                let account: [u8; 16] = decode_hex(parts.next()?)?.try_into().ok()?;
                let at: i64 = parts.next()?.parse().ok()?;
                Some((DocId::from_bytes(doc), AccountId::from_bytes(account), at))
            })
            .collect())
    }

    pub(crate) fn dispute_add(
        &self,
        feed: FeedId,
        doc_id: DocId,
        account: AccountId,
        at: i64,
    ) -> Result<(), ApiError> {
        let mut all = self.disputes_of(feed)?;
        all.retain(|(doc, who, _)| !(*doc == doc_id && *who == account));
        all.push((doc_id, account, at));
        if all.len() > 50 {
            let drop = all.len() - 50;
            all.drain(..drop);
        }
        let encoded: Vec<String> = all
            .iter()
            .map(|(doc, who, at)| format!("{}:{}:{at}", hex(doc.as_bytes()), hex(who.as_bytes())))
            .collect();
        self.fcfg_set(feed, "disputes", &encoded.join(";"))
    }

    // ----- heads ----------------------------------------------------------------------------

    /// The signed head of this device for the feed. While a snapshot of a shared feed waits to be
    /// verified, the head states a cursor below its `covers`: the server prunes behind the covered
    /// entries only when every device's head is past them, so acknowledging comes after the check.
    pub fn sync_feed_head(&mut self, feed: FeedId) -> Result<Vec<u8>, ApiError> {
        let row = self.feed_row(feed)?;
        let (account, device) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let epoch = self
            .server_epoch()?
            .ok_or_else(|| ApiError::storage("the server epoch is not known yet"))?;
        let mut cursor = row.cursor;
        let mut chain = row
            .chain_hash
            .unwrap_or_else(|| kb_sync::v2::chain_genesis(&epoch));
        if let Some(floor) = self.unverified_floor(feed)?
            && floor <= cursor
        {
            let history = chain_history_of(&row);
            match history.iter().rev().find(|(c, _)| *c < floor) {
                Some((c, h)) => {
                    cursor = *c;
                    chain = *h;
                }
                None => {
                    cursor = 0;
                    chain = kb_sync::v2::chain_genesis(&epoch);
                }
            }
        }
        let head = kb_sync::v2::Head {
            feed,
            account_id: account,
            device_id: device,
            cursor,
            chain_hash: chain,
            server_epoch: epoch,
            at_ms: self.clock.now_ms(),
        };
        Ok(kb_sync::v2::sign_head(&head, self.signer()))
    }

    /// The signing key of an active device of an account: this account's own log or a co-member's
    /// copy (`None` when the log is unknown or the device is revoked).
    fn active_device_key(
        &self,
        account: AccountId,
        device: DeviceId,
    ) -> Option<kb_crypto::VerifyingKey> {
        let me = self.sync_identity().ok().flatten().map(|(a, _)| a);
        if Some(account) == me {
            return self
                .security
                .log_state
                .as_ref()
                .and_then(|s| s.active(device))
                .map(|d| d.keys.signing);
        }
        self.shared
            .logs
            .get(&account)
            .and_then(|log| log.verify().ok())
            .and_then(|state| state.active(device).map(|d| d.keys.signing))
    }

    /// The heads of the other devices in the feed; `true` when a fork or rollback was detected.
    pub fn sync_feed_heads_received(
        &mut self,
        feed: FeedId,
        heads: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        self.shared_load()?;
        let row = self.feed_row(feed)?;
        let (me, my_device) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let now = self.clock.now_ms();
        let epoch = self.server_epoch()?;
        let shared = row.kind == FeedKind::Shared;
        let members = if shared {
            self.members_of(feed)?
        } else {
            Vec::new()
        };
        let mut known: Vec<(DeviceId, u64, [u8; 32])> = self
            .db
            .sync_heads_all_in(feed)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|h| (h.device_id, h.cursor, h.chain_hash))
            .collect();
        let history = chain_history_of(&row);
        let mut fork = None;
        for bytes in heads {
            let peeked = kb_sync::v2::peek_head(bytes)
                .map_err(|e| ApiError::storage(format!("malformed head: {e}")))?;
            if peeked.feed != feed || Some(peeked.server_epoch) != epoch {
                continue;
            }
            if peeked.account_id != me && !(shared && members.contains(&peeked.account_id)) {
                continue;
            }
            let Some(verifier) = self.active_device_key(peeked.account_id, peeked.device_id) else {
                tracing::debug!("head of a device that is not active here ignored");
                continue;
            };
            let head = kb_sync::v2::verify_head(bytes, &verifier)
                .map_err(|_| ApiError::crypto("a head does not verify"))?;
            if head.device_id == my_device {
                continue;
            }
            if let Some((_, old_cursor, _)) = known.iter().find(|(d, _, _)| *d == head.device_id)
                && head.cursor < *old_cursor
            {
                // A device that holds back its acknowledgement of a snapshot states a lower cursor
                // than before; that is not a rollback of the feed.
                if !shared {
                    fork = Some(format!(
                        "a device's position in the feed went backwards ({} to {})",
                        old_cursor, head.cursor
                    ));
                }
            }
            if history
                .iter()
                .any(|(c, h)| *c == head.cursor && *h != head.chain_hash)
            {
                fork = Some(format!(
                    "another device saw a different history at position {}",
                    head.cursor
                ));
            }
            self.db
                .sync_heads_put_in(
                    feed,
                    &SyncHeadRow {
                        device_id: head.device_id,
                        cursor: head.cursor,
                        chain_hash: head.chain_hash,
                        at_ms: head.at_ms,
                        signed: bytes.clone(),
                        seen_at: now,
                    },
                )
                .map_err(map_storage_error)?;
            known.retain(|(d, _, _)| *d != head.device_id);
            known.push((head.device_id, head.cursor, head.chain_hash));
        }
        if let Some(epoch) = epoch {
            known.retain(|(d, _, _)| *d != my_device);
            known.push((
                my_device,
                row.cursor,
                row.chain_hash
                    .unwrap_or_else(|| kb_sync::v2::chain_genesis(&epoch)),
            ));
        }
        for (i, (da, ca, ha)) in known.iter().enumerate() {
            for (db_, cb, hb) in known.iter().skip(i + 1) {
                if ca == cb && ha != hb {
                    fork = Some(format!(
                        "two devices disagree about the feed at position {ca} ({da} and {db_})"
                    ));
                }
            }
        }
        if let Some(why) = fork {
            self.mark_fork_in(feed, &why)?;
            return Ok(true);
        }
        Ok(false)
    }

    // ----- chunks ---------------------------------------------------------------------------

    /// Up to `limit` chunk transfers of the feed due now: deletes, then uploads and copies, then
    /// downloads. Deletes and copies only on devices and in feeds where the role allows them (a
    /// shared feed: deletes Owner, uploads and copies Editor, downloads any member; a closed feed
    /// only downloads).
    pub fn sync_feed_chunk_work(
        &mut self,
        feed: FeedId,
        limit: usize,
    ) -> Result<Vec<FeedChunkWork>, ApiError> {
        self.shared_load()?;
        let row = self.feed_row(feed)?;
        let Some(personal) = self.personal_feed()? else {
            return Ok(Vec::new());
        };
        let shared_map = self.shared_space_map()?;
        let (can_delete, can_write) = if row.kind == FeedKind::Shared {
            let closed = self
                .shared
                .timelines
                .get(&feed)
                .is_some_and(|t| t.state().closed.is_some());
            let role = self.feed_role(feed)?;
            (
                !closed && role.is_some_and(MemberRole::is_owner),
                !closed && role.is_some_and(|r| r >= MemberRole::Editor),
            )
        } else {
            (true, true)
        };
        let now = self.clock.now_ms();
        let mut out = Vec::new();
        for (state, kind) in [
            (DELETE, FeedChunkKind::Delete),
            (UPLOAD, FeedChunkKind::Upload),
            (COPY, FeedChunkKind::Copy),
            (DOWNLOAD, FeedChunkKind::Download),
        ] {
            let allowed = match kind {
                FeedChunkKind::Delete => can_delete,
                FeedChunkKind::Upload | FeedChunkKind::Copy => can_write,
                FeedChunkKind::Download => true,
            };
            if !allowed {
                continue;
            }
            let room = limit.saturating_sub(out.len());
            if room == 0 {
                break;
            }
            // The due rows are not sorted by feed: a window of them is read and filtered, and a
            // window that held too few of this feed's chunks grows (a library with one shared
            // space among many personal chunks).
            let mut window = u32::try_from(room.saturating_mul(4).max(64)).unwrap_or(u32::MAX);
            let found = loop {
                let rows = self
                    .db
                    .chunk_sync_due(state, now, window)
                    .map_err(map_storage_error)?;
                let exhausted = rows.len() < usize::try_from(window).unwrap_or(usize::MAX);
                let mut found = Vec::new();
                for chunk in rows {
                    if found.len() >= room {
                        break;
                    }
                    if self.feed_of_chunk(chunk.attachment_id, &shared_map, personal)? == Some(feed)
                    {
                        found.push(chunk);
                    }
                }
                if found.len() >= room || exhausted || window >= CHUNK_WINDOW_MAX {
                    break found;
                }
                window = window.saturating_mul(4).min(CHUNK_WINDOW_MAX);
            };
            out.extend(found.into_iter().map(|chunk| FeedChunkWork {
                feed,
                hash: chunk.hash,
                attachment_id: chunk.attachment_id,
                kind,
            }));
        }
        Ok(out)
    }

    /// The sealed chunk to upload; `None` when it is not here any more.
    pub fn sync_feed_chunk_bytes(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        let _ = feed;
        self.chunk_upload_bytes(hash)
    }

    /// The feed's namespace took the chunk (an upload or a copy).
    pub fn sync_feed_chunk_uploaded(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<(), ApiError> {
        let _ = feed;
        self.chunk_uploaded(hash)
    }

    /// A downloaded chunk, verified against its hash before it is stored (`Crypto` otherwise).
    pub fn sync_feed_chunk_received(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError> {
        let _ = feed;
        self.chunk_received(hash, bytes)
    }

    /// The feed's namespace does not hold the chunk yet: the core asks again later.
    pub fn sync_feed_chunk_unavailable(
        &mut self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<(), ApiError> {
        let _ = feed;
        self.chunk_unavailable(hash)
    }

    /// The feed's namespace no longer holds the chunk (deleted now, or absent already).
    pub fn sync_feed_chunk_deleted(&mut self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        let _ = feed;
        self.chunk_deleted(hash)
    }

    /// Every chunk of the space's attachments that the feed's namespace lacks is copied from the
    /// personal one (the share step, ADR-0045 section 10): the rows that said "both have it" now say
    /// "copy". Rows that wait for an upload stay: they upload into the feed's namespace now.
    #[allow(dead_code)] // the share request (agent D2) calls it
    pub(crate) fn chunks_follow_space_to_feed(
        &mut self,
        space_id: SpaceId,
    ) -> Result<(), ApiError> {
        let attachments: Vec<AttachmentId> = self
            .docs
            .structures
            .get(&space_id)
            .map(|open| {
                open.model
                    .attachments()
                    .into_iter()
                    .map(|manifest| manifest.attachment_id)
                    .collect()
            })
            .unwrap_or_default();
        for attachment_id in attachments {
            for chunk in self
                .db
                .chunk_sync_by_attachment(attachment_id)
                .map_err(map_storage_error)?
            {
                if chunk.state == crate::transfers::SYNCED {
                    self.db
                        .chunk_sync_set(chunk.hash, COPY, 0, None)
                        .map_err(map_storage_error)?;
                }
            }
        }
        Ok(())
    }

    // ----- counts for `SyncInfo` ------------------------------------------------------------

    /// Documents of the feed with local updates the feed does not hold (and the keyring when its
    /// items wait), as `SyncInfo` counts them.
    pub(crate) fn feed_pending_count(&self, feed: FeedId) -> Result<u32, ApiError> {
        let Some(personal) = self.personal_feed()? else {
            return Ok(0);
        };
        let shared = self.shared_space_map()?;
        let states: HashMap<DocId, i64> = self
            .db
            .sync_doc_state_all_in(feed)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|s| (s.doc_id, s.pushed_up_to_seq))
            .collect();
        let mut pending = 0usize;
        for row in self.db.docs_all().map_err(map_storage_error)? {
            if Self::feed_of_doc(&row, &shared, personal) != Some(feed) {
                continue;
            }
            let pushed = states.get(&row.doc_id).copied().unwrap_or(0);
            let last = self
                .db
                .update_stats(row.doc_id)
                .map_err(map_storage_error)?
                .last_seq
                .unwrap_or(0);
            if last > pushed {
                pending += 1;
            }
        }
        if !self.keyring_unpublished_for(feed, &shared)?.is_empty() {
            pending += 1;
        }
        pending += self.unconfirmed_membership_rows(feed)?.len();
        Ok(u32::try_from(pending).unwrap_or(u32::MAX))
    }

    /// The unpublished keyring rows that travel in this feed.
    pub(crate) fn keyring_unpublished_for(
        &self,
        feed: FeedId,
        shared: &HashMap<SpaceId, SharedSpace>,
    ) -> Result<Vec<kb_storage::KeyringRow>, ApiError> {
        let personal = self.personal_feed()?;
        let mut out = Vec::new();
        for row in self.db.keyring_unpublished().map_err(map_storage_error)? {
            let item = kb_keys::KeyringItem::decode(&row.record).map_err(crate::map_keys_error)?;
            if Self::feed_of_keyring_item(&item, shared, personal) == Some(feed) {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// The feed a keyring item travels in: `Space` records and everything of a personal space in the
    /// personal feed, the other records and the tombstones of a shared space in that space's feed
    /// (`None` for an archive).
    pub(crate) fn feed_of_keyring_item(
        item: &kb_keys::KeyringItem,
        shared: &HashMap<SpaceId, SharedSpace>,
        personal: Option<FeedId>,
    ) -> Option<FeedId> {
        if matches!(item.subject(), kb_keys::KeySubject::Space { .. }) {
            return personal;
        }
        match shared.get(&item.space_id()) {
            Some(space) if space.archived => None,
            Some(space) => Some(space.feed),
            None => personal,
        }
    }

    /// Whether the feed has anything this device would send: updates of its documents the feed
    /// lacks (a `Local` or `Core` update) or keyring items that wait.
    pub(crate) fn feed_has_unsent(&self, feed: FeedId) -> Result<bool, ApiError> {
        let Some(personal) = self.personal_feed()? else {
            return Ok(false);
        };
        let shared = self.shared_space_map()?;
        for row in self.db.docs_all().map_err(map_storage_error)? {
            if Self::feed_of_doc(&row, &shared, personal) != Some(feed) {
                continue;
            }
            if !self.doc_unsent_updates(feed, row.doc_id)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(!self.keyring_unpublished_for(feed, &shared)?.is_empty())
    }

    /// The updates of a document this device wrote and the feed does not hold yet.
    pub(crate) fn doc_unsent_updates(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<Vec<(i64, Vec<u8>, kb_storage::UpdateOrigin)>, ApiError> {
        let pushed = self
            .db
            .sync_doc_state_get_in(feed, doc_id)
            .map_err(map_storage_error)?
            .map_or(0, |s| s.pushed_up_to_seq);
        Ok(self
            .db
            .updates_from(doc_id, pushed)
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|(_, _, origin)| *origin != kb_storage::UpdateOrigin::Remote)
            .collect())
    }

    /// One `FeedSyncInfo` per feed of this device (the personal feed first).
    pub(crate) fn feeds_info(&self) -> Result<Vec<kb_types::FeedSyncInfo>, ApiError> {
        let mut out = Vec::new();
        for row in self.db.sync_feed_list().map_err(map_storage_error)? {
            let refused: u32 = self
                .db
                .rejected_count_by_account(row.feed)
                .map_err(map_storage_error)?
                .into_iter()
                .map(|(_, n)| n)
                .fold(0, u32::saturating_add);
            out.push(kb_types::FeedSyncInfo {
                feed_id: row.feed,
                space_id: row.space_id,
                pending_entries: self.feed_pending_count(row.feed)?,
                refused,
                forked: row.fork,
                closed: row.kind == FeedKind::Shared
                    && self
                        .shared
                        .timelines
                        .get(&row.feed)
                        .is_some_and(|t| t.state().closed.is_some()),
                server_flagged: self.fcfg(row.feed, "flagged")?.as_deref() == Some("1"),
            });
        }
        Ok(out)
    }

    // ----- a new server epoch ---------------------------------------------------------------

    /// The server lost its data (a new epoch) or another server is configured: every feed's cursor,
    /// chain, heads and per-document state start over. A shared feed keeps its membership entries
    /// and posts them again (`sync_membership_pending` answers every entry), a personal feed uploads
    /// everything again as v1 does.
    pub(crate) fn feeds_forget_remote_state(&mut self) -> Result<(), ApiError> {
        let epoch = self.server_epoch()?;
        let rows = self.db.sync_feed_list().map_err(map_storage_error)?;
        for row in rows {
            let membership = if row.kind == FeedKind::Shared {
                self.db
                    .membership_entries_list(row.feed, None)
                    .map_err(map_storage_error)?
                    .into_iter()
                    .map(|entry| kb_storage::MembershipEntryRow { cursor: 0, ..entry })
                    .collect()
            } else {
                Vec::new()
            };
            let fresh = SyncFeedRow {
                cursor: 0,
                chain_hash: None,
                max_covers: 0,
                chain_history: None,
                fork: false,
                server_epoch: epoch,
                ..row.clone()
            };
            // The device's own sequence numbers (and what its snapshots covered) are not the
            // server's: `sync_feed_delete` takes the per-document state with the feed, so it goes
            // back as it is (`sync_doc_state_reset_all` already cleared what concerns the server),
            // or the next entry of a document would repeat a `device_seq` its peers have seen.
            let doc_states = self
                .db
                .sync_doc_state_all_in(row.feed)
                .map_err(map_storage_error)?;
            self.db
                .transaction(|tx| {
                    tx.sync_feed_delete(row.feed)?;
                    tx.sync_feed_put(&fresh)?;
                    for state in &doc_states {
                        tx.sync_doc_state_put_in(row.feed, state)?;
                    }
                    if !membership.is_empty() {
                        tx.membership_entries_put(&membership)?;
                    }
                    Ok::<(), kb_storage::StorageError>(())
                })
                .map_err(map_storage_error)?;
            for name in [
                "posted",
                "marks",
                "deferred",
                "unverified",
                "refetch",
                "rekey",
                "refused",
                "bootstrapped",
            ] {
                self.fcfg_del(row.feed, name)?;
            }
            // Entries found before the reset are re-posted; what was only pending stays pending.
            self.shared.timelines.remove(&row.feed);
            self.shared.metas.remove(&row.feed);
            self.shared.keyring_in_flight.remove(&row.feed);
            self.shared.snapshots_checked_at.remove(&row.feed);
            self.shared.stalled.remove(&row.feed);
        }
        Ok(())
    }
}

impl Core {
    /// The share step's re-wrap (ADR-0045 section 10, step 3): every attachment key of the space is
    /// wrapped again under the space key of the feed's current epoch, which the members hold (the
    /// record that wraps it under the personal epoch stays in the owner's personal feed). Chunks
    /// follow through `chunks_follow_space_to_feed`.
    #[allow(dead_code)] // the share request (agent D2) calls it
    pub(crate) fn share_rewrap_attachment_keys(
        &mut self,
        space_id: SpaceId,
    ) -> Result<(), ApiError> {
        let (space_key, epoch) = self.attachment_wrap_key(space_id)?;
        let ids: Vec<AttachmentId> = self
            .docs
            .structures
            .get(&space_id)
            .map(|open| {
                open.model
                    .attachments()
                    .into_iter()
                    .map(|manifest| manifest.attachment_id)
                    .collect()
            })
            .unwrap_or_default();
        let now = self.clock.now_ms();
        for id in ids {
            let subject = kb_keys::KeySubject::Attachment { attachment_id: id };
            let Some(record) = self.security.keyring.record(space_id, &subject).cloned() else {
                continue;
            };
            if record.wrap_epoch >= epoch {
                continue;
            }
            let key = self.attachment_key(space_id, id)?;
            let rewrapped = kb_keys::keyring::wrap_attachment_key(
                &key,
                &space_key,
                space_id,
                id,
                epoch,
                &self.security.device_keys,
                self.device.device_id,
                now,
                self.rng.as_ref(),
            );
            self.put_keyring_item(kb_keys::KeyringItem::Record(rewrapped))?;
        }
        Ok(())
    }
}

/// The `sync_config` key under which a purge of a shared space's attachment notes the feed its chunks
/// are deleted from (the attachment's index row is gone by the time the deletes run).
pub(crate) fn chunk_delete_key(attachment_id: AttachmentId) -> String {
    format!("chunkdel.{attachment_id}")
}
