//! The apply path of a feed (ADR-0043, ADR-0045): the chain, then for each item the checks that make
//! it acceptable (a shared feed: membership at the item's cursor, the signature by a device of the
//! header account, the role of ADR-0043), the decryption under the document key of its epoch, the
//! checks that the decrypted kind agrees with the document id, the apply, the entry marks of the
//! dominance check. The chain advances in every case, quarantined items included.
//!
//! The personal feed goes through the same functions with none of the shared checks.

use kb_crypto::{Secret32, VerifyingKey};
use kb_doc::Doc;
use kb_keys::KeyringItem;
use kb_keys::space_keys::space_keyring_key;
use kb_storage::{
    DocKindCode, DocQueries, EntryMarkRow, FeedQueries, RejectReason, RejectedRow, SecurityQueries,
    SyncPendingRow, SyncQueries,
};
use kb_sync::v2::{
    ItemKind, SnapshotsApplied, SyncApplied, chain_genesis, chain_next, feed_kind, open_entry,
    open_keyring, open_snapshot, peek_entry, peek_keyring, peek_snapshot, verify_signature,
};
use kb_sync::{ChangesPage, SnapshotsPage, SyncError};
use kb_types::sync::{FeedKind, KEYRING_DOC_ID};
use kb_types::{AccountId, ApiError, DeviceId, DocId, Event, FeedId, MemberRole, PageId, SpaceId};

use super::store_chain;
use crate::comments::{Carried, CommentApply, StreamStatus};
use crate::documents::{map_doc_error, map_model_error};
use crate::security::keyring_row;
use crate::sync::{
    PENDING_HISTORY, PENDING_KEY, PENDING_SNAPSHOT, PENDING_STRUCTURE, doc_kind_code, engine_kind,
    map_sync_error,
};
use crate::{Core, map_keys_error, map_storage_error, short_id};

/// A decrypted item: its document kind byte and the blobs to import.
type Opened = (u8, Vec<Vec<u8>>);

/// What one item or snapshot did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Applied,
    Own,
    Pending,
    Rejected,
    Covered,
    /// A device log the item needs is not here: the page stops before the item.
    Stalled,
    /// A snapshot that omits entries it covers.
    Disputed(DocId, u64),
}

/// The role an item needs (ADR-0043).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Editor,
    Commenter,
}

impl Need {
    const fn role(self) -> MemberRole {
        match self {
            Self::Editor => MemberRole::Editor,
            Self::Commenter => MemberRole::Commenter,
        }
    }
}

/// What applying the content of a decrypted item did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Content {
    /// Applied.
    Done,
    /// Parked: it waits for a structure, a key or history.
    Parked,
    /// A device log it needs is not here: the page stops before the item.
    Stalled,
    /// Quarantined by the content's own checks (a comment record that fails them).
    Rejected,
}

/// What a keyring item of a shared feed came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemVerdict {
    /// Not allowed.
    Refused,
    /// Allowed; whether the keyring changed.
    Merged(bool),
    /// A Commenter's `Doc` record for a document this device cannot place yet (a page whose
    /// structure has not arrived): the entry waits.
    Deferred,
}

/// Opens an item with the first of `keys` that authenticates it. `Ok(None)` when none does although
/// the item's signature is valid: the key that sealed it has not arrived yet, so the caller parks it.
fn open_with_keys<T>(
    keys: &[Secret32],
    bytes: &[u8],
    verifier: &VerifyingKey,
    open: impl Fn(&Secret32) -> Result<T, SyncError>,
) -> Result<Option<T>, ApiError> {
    for key in keys {
        match open(key) {
            Ok(opened) => return Ok(Some(opened)),
            Err(SyncError::Authentication) => {}
            Err(err) => return Err(map_sync_error(err)),
        }
    }
    if !keys.is_empty() {
        verify_signature(bytes, verifier).map_err(map_sync_error)?;
    }
    Ok(None)
}

impl Core {
    // ----- the page ------------------------------------------------------------------------

    /// A page of the feed's changes (the apply path above). The driver stops on `Authentication`
    /// (`Crypto`). A page that stalls for a co-member's device log answers a `new_cursor` below its
    /// last cursor: fetch `sync_needed_logs` and offer the page again.
    pub fn sync_feed_apply(
        &mut self,
        feed: FeedId,
        page: &ChangesPage,
    ) -> Result<SyncApplied, ApiError> {
        self.shared_load()?;
        let row = self.feed_row(feed)?;
        let kind = row.kind;
        if kind == FeedKind::Shared && !self.feed_is_live(feed)? {
            return Ok(SyncApplied {
                new_cursor: row.cursor,
                ..SyncApplied::default()
            });
        }
        let Some(epoch) = self.server_epoch()? else {
            return Err(ApiError::storage("the server epoch is not known yet"));
        };
        if page.server_epoch != epoch {
            return Err(ApiError::storage(
                "the page belongs to another server epoch",
            ));
        }
        let mut cursor = row.cursor;
        let mut report = SyncApplied {
            new_cursor: cursor,
            ..SyncApplied::default()
        };
        // Already applied items (a page replayed by the driver) are ignored.
        let items: Vec<&(u64, Vec<u8>)> =
            page.changes.iter().filter(|(c, _)| *c > cursor).collect();
        let mut chain = row.chain_hash.unwrap_or_else(|| chain_genesis(&epoch));
        let mut after_item = Vec::with_capacity(items.len());
        for (_, bytes) in &items {
            chain = chain_next(&chain, bytes);
            after_item.push(chain);
        }
        let reanchor = if chain == page.chain_hash_after {
            false
        } else if items.is_empty() {
            // Nothing new, yet the server's hash at our cursor differs: a rollback.
            self.mark_fork_in(feed, "the server's history does not match this device's")?;
            return Err(ApiError::crypto(
                "the server's history does not match this device's",
            ));
        } else if self.shared.snapshots_checked_at.get(&feed) == Some(&cursor) {
            if row.max_covers > cursor {
                tracing::warn!(
                    cursor,
                    "feed entries were pruned under a snapshot; re-anchoring the chain"
                );
                report.gaps += 1;
                true
            } else {
                self.mark_fork_in(feed, "the server left entries out of the feed")?;
                return Err(ApiError::crypto("the server left entries out of the feed"));
            }
        } else {
            tracing::warn!(
                cursor,
                "chain mismatch: asking for the snapshots before deciding"
            );
            report.gaps += 1;
            return Ok(report);
        };
        if kind == FeedKind::Shared {
            self.want_logs_of_page(feed, &items)?;
        }
        let mut completed = true;
        for (index, (item_cursor, bytes)) in items.iter().enumerate() {
            let outcome = self.apply_feed_item(feed, kind, *item_cursor, bytes)?;
            match outcome {
                Outcome::Stalled => {
                    completed = false;
                    break;
                }
                Outcome::Applied => report.applied += 1,
                Outcome::Own => report.skipped_own += 1,
                Outcome::Pending => report.pending += 1,
                Outcome::Rejected => report.rejected += 1,
                Outcome::Covered | Outcome::Disputed(..) => {}
            }
            cursor = *item_cursor;
            // The item may have changed the row (a fork, a fork-out): the cursor is written on the
            // row as it is now.
            let mut fresh = self.feed_row(feed)?;
            fresh.cursor = cursor;
            if !reanchor {
                store_chain(&mut fresh, cursor, &after_item[index]);
            }
            self.feed_row_put(&fresh)?;
            if kind == FeedKind::Shared && !self.feed_is_live(feed)? {
                completed = false;
                break;
            }
        }
        if reanchor && completed {
            let mut fresh = self.feed_row(feed)?;
            store_chain(&mut fresh, cursor, &page.chain_hash_after);
            self.feed_row_put(&fresh)?;
        }
        report.new_cursor = cursor;
        if kind == FeedKind::Shared {
            self.confirm_records_up_to(feed, cursor)?;
        }
        if completed {
            self.shared.fresh_logs.clear();
        }
        if report.applied > 0 {
            self.retry_pending_in(feed)?;
        }
        if !page.more && completed {
            self.forkout_check(feed)?;
            // The first full read of a shared feed is over: what comes now is news (notifications).
            if kind == FeedKind::Shared && self.fcfg(feed, "bootstrapped")?.is_none() {
                self.fcfg_set(feed, "bootstrapped", "1")?;
            }
        }
        Ok(report)
    }

    /// The device logs the membership entries of a page name and this device lacks are asked for at
    /// once, so that one round trip brings them all.
    fn want_logs_of_page(
        &mut self,
        feed: FeedId,
        items: &[&(u64, Vec<u8>)],
    ) -> Result<(), ApiError> {
        let me = self.me()?;
        for (_, bytes) in items {
            if feed_kind(bytes).ok() != Some(ItemKind::Membership) {
                continue;
            }
            let Ok(entry) = kb_keys::membership::MembershipEntry::decode(bytes) else {
                continue;
            };
            for account in crate::membership::entry_accounts(&entry) {
                if account != me && !self.shared.logs.contains_key(&account) {
                    self.shared.wanted_logs.insert(account);
                }
            }
        }
        let _ = feed;
        Ok(())
    }

    /// One feed item, fully processed.
    fn apply_feed_item(
        &mut self,
        feed: FeedId,
        kind: FeedKind,
        cursor: u64,
        bytes: &[u8],
    ) -> Result<Outcome, ApiError> {
        match feed_kind(bytes).map_err(map_sync_error)? {
            ItemKind::Membership => {
                if kind != FeedKind::Shared {
                    tracing::warn!(cursor, "a membership entry in the personal feed rejected");
                    return Ok(Outcome::Rejected);
                }
                Ok(match self.membership_ingest(feed, cursor, bytes)? {
                    crate::membership::Ingest::Applied => Outcome::Applied,
                    crate::membership::Ingest::Already => Outcome::Covered,
                    crate::membership::Ingest::Stalled => Outcome::Stalled,
                })
            }
            ItemKind::Keyring => self.apply_feed_keyring(feed, kind, cursor, bytes),
            ItemKind::Entry => self.apply_feed_entry(feed, kind, cursor, bytes),
        }
    }

    // ----- membership and role checks -------------------------------------------------------

    /// The signing key of a device of an account, revoked or not (items keep their signatures after
    /// their author's revocation): this account's own log or a co-member's copy. `Ok(None)` when the
    /// log is not held or is older than the device (the caller asks for it); a device that a freshly fetched
    /// log does not know is `Crypto`.
    pub(crate) fn member_device_key(
        &mut self,
        account: AccountId,
        device: DeviceId,
    ) -> Result<Option<VerifyingKey>, ApiError> {
        let (me, _) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let log = if account == me {
            self.security.log.as_ref()
        } else {
            self.shared.logs.get(&account)
        };
        let Some(log) = log else {
            self.shared.wanted_logs.insert(account);
            return Ok(None);
        };
        if let Some(keys) = log.device_keys(device) {
            return Ok(Some(keys.signing));
        }
        // A device the held log does not list: the account may have added it since the log was
        // fetched, so the log is asked for again first; a log fetched since that still does not
        // list it is a forged claim.
        if self.shared.fresh_logs.contains(&account) {
            return Err(ApiError::crypto(
                "the entry was signed by a device the log does not know",
            ));
        }
        self.shared.wanted_logs.insert(account);
        Ok(None)
    }

    /// The role of an account in a shared feed at a cursor; `None` when it was not a member then.
    pub(crate) fn role_at(
        &self,
        feed: FeedId,
        account: AccountId,
        cursor: u64,
    ) -> Option<MemberRole> {
        self.shared
            .timelines
            .get(&feed)
            .and_then(|t| t.role_at(account, cursor))
    }

    /// The role a snapshot's writer had when it made it, judged at the cursor the snapshot covers. A device
    /// that made its first snapshots before it pulled its own `Create` or `Join` covers a cursor from
    /// before its membership: it is judged from the cursor it became a member at.
    fn snapshot_writer_role(
        &self,
        feed: FeedId,
        account: AccountId,
        covers: u64,
    ) -> Option<MemberRole> {
        let timeline = self.shared.timelines.get(&feed)?;
        timeline.role_at(account, covers).or_else(|| {
            let joined = timeline.state().member(account)?.joined_cursor;
            timeline.role_at(account, covers.max(joined))
        })
    }

    /// Whether a document id is the comment stream of a page of the space
    /// ([`crate::comments::comment_stream_doc_id`], ADR-0048): comment-stream entries need the
    /// Commenter role. A document this device holds is what its row says; otherwise the id must be the
    /// derived stream of a page of the space's structure, and not the id of the structure or a page.
    fn is_comment_stream(&self, space: Option<SpaceId>, doc_id: DocId) -> bool {
        self.comment_stream_status(space, doc_id) == StreamStatus::Is
    }

    /// The role an entry of a shared feed needs before it is decrypted: a stream's is Commenter's, a
    /// known page's or structure's is Editor's, and a document this device cannot place yet needs the
    /// lower one (the decrypted kind decides, in `apply_entry_in`: a page entry of a Commenter is
    /// refused there).
    fn need_for(&self, space: Option<SpaceId>, doc_id: DocId) -> Need {
        match self.comment_stream_status(space, doc_id) {
            StreamStatus::Is | StreamStatus::Unknown => Need::Commenter,
            StreamStatus::Other => Need::Editor,
        }
    }

    /// Quarantines an item: it is rejected, kept in `sync_rejected` and counted against its writer;
    /// an item of a non-member also flags the feed as one the server misbehaved on.
    #[allow(clippy::too_many_arguments)]
    fn quarantine(
        &mut self,
        feed: FeedId,
        cursor: u64,
        doc_id: DocId,
        account: AccountId,
        device: DeviceId,
        reason: RejectReason,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let at = self.clock.now_ms();
        self.db
            .rejected_put(&RejectedRow {
                feed,
                cursor,
                doc_id,
                account_id: account,
                device_id: device,
                reason,
                bytes: bytes.to_vec(),
                at,
            })
            .map_err(map_storage_error)?;
        if reason == RejectReason::NotAMember {
            self.fcfg_set(feed, "flagged", "1")?;
        }
        tracing::warn!(feed = %feed.short_hex(), cursor, reason = reason.to_byte(), "an item was quarantined");
        if let Some(space_id) = self.feed_space(feed)? {
            self.push_event_once(Event::SpaceMembersChanged { space_id });
        }
        Ok(())
    }

    // ----- continuity -----------------------------------------------------------------------

    /// `device_seq` must continue the last one seen from that device for that document, in this
    /// feed (the rule of v1).
    fn continuity_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
        device_seq: u64,
    ) -> Result<bool, ApiError> {
        let Some(last) = self
            .db
            .sync_seen_get_in(feed, doc_id, device_id)
            .map_err(map_storage_error)?
        else {
            return Ok(true);
        };
        if device_seq <= last {
            tracing::debug!(doc = %short_id(&doc_id), device_seq, last, "repeated entry skipped");
            return Ok(false);
        }
        if device_seq != last + 1 {
            let snapshotted = doc_id != KEYRING_DOC_ID
                && self
                    .doc_state_in(feed, doc_id)?
                    .remote_snapshot_covers
                    .is_some();
            if !snapshotted {
                return Err(ApiError::storage(format!(
                    "the feed skipped entries of a device (expected {}, got {device_seq})",
                    last + 1
                )));
            }
            tracing::debug!(doc = %short_id(&doc_id), device_seq, last, "sequence gap under a snapshot");
        }
        Ok(true)
    }

    fn seen_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
        device_seq: u64,
    ) -> Result<(), ApiError> {
        self.db
            .sync_seen_put_in(feed, doc_id, device_id, device_seq)
            .map_err(map_storage_error)
    }

    fn park_in(
        &self,
        feed: FeedId,
        cursor: u64,
        doc_id: DocId,
        bytes: &[u8],
        reason: u8,
    ) -> Result<(), ApiError> {
        tracing::debug!(cursor, doc = %short_id(&doc_id), reason, "item parked");
        self.db
            .sync_pending_put_in(
                feed,
                &SyncPendingRow {
                    cursor,
                    doc_id,
                    bytes: bytes.to_vec(),
                    reason,
                    received_at: self.clock.now_ms(),
                },
            )
            .map_err(map_storage_error)
    }

    /// Documents being rebuilt after a fork-out: their own earlier entries are applied again.
    pub(crate) fn rebuilding(&self, feed: FeedId, doc_id: DocId) -> Result<bool, ApiError> {
        Ok(self.fcfg(feed, "rebuild")?.is_some_and(|list| {
            list.split(',')
                .any(|d| d == crate::security::hex(doc_id.as_bytes()))
        }))
    }

    // ----- a document entry -----------------------------------------------------------------

    fn apply_feed_entry(
        &mut self,
        feed: FeedId,
        kind: FeedKind,
        cursor: u64,
        bytes: &[u8],
    ) -> Result<Outcome, ApiError> {
        let header = peek_entry(bytes).map_err(map_sync_error)?;
        if header.feed != feed {
            tracing::warn!(cursor, "an entry of another feed rejected");
            return Ok(Outcome::Rejected);
        }
        let (me, my_device) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let shared = kind == FeedKind::Shared;
        let space = self.feed_space(feed)?;
        let verifier;
        if shared {
            // Membership at the cursor first: an account that was not a member then needs no log.
            let Some(role) = self.role_at(feed, header.account_id, cursor) else {
                self.quarantine(
                    feed,
                    cursor,
                    header.doc_id,
                    header.account_id,
                    header.device_id,
                    RejectReason::NotAMember,
                    bytes,
                )?;
                self.seen_in(feed, header.doc_id, header.device_id, header.device_seq)?;
                return Ok(Outcome::Rejected);
            };
            let Some(key) = self.member_device_key(header.account_id, header.device_id)? else {
                return Ok(Outcome::Stalled);
            };
            verify_signature(bytes, &key).map_err(map_sync_error)?;
            verifier = key;
            if !self.continuity_in(feed, header.doc_id, header.device_id, header.device_seq)? {
                return Ok(Outcome::Rejected);
            }
            let need = self.need_for(space, header.doc_id);
            if role < need.role() {
                self.quarantine(
                    feed,
                    cursor,
                    header.doc_id,
                    header.account_id,
                    header.device_id,
                    RejectReason::RoleForbids,
                    bytes,
                )?;
                self.seen_in(feed, header.doc_id, header.device_id, header.device_seq)?;
                if header.account_id == me && header.device_id == my_device {
                    self.forkout_note_refused_own(feed, header.doc_id)?;
                }
                return Ok(Outcome::Rejected);
            }
        } else {
            if header.account_id != me {
                tracing::warn!(cursor, "entry of another account rejected");
                return Ok(Outcome::Rejected);
            }
            verifier = self.device_verifier(header.device_id)?;
            if !self.continuity_in(feed, header.doc_id, header.device_id, header.device_seq)? {
                return Ok(Outcome::Rejected);
            }
        }
        if header.device_id == my_device && !self.rebuilding(feed, header.doc_id)? {
            self.seen_in(feed, header.doc_id, header.device_id, header.device_seq)?;
            return Ok(Outcome::Own);
        }
        let state = self.doc_state_in(feed, header.doc_id)?;
        let outcome = if state
            .remote_snapshot_covers
            .is_some_and(|covers| cursor <= covers)
        {
            Outcome::Covered
        } else {
            self.apply_entry_in(feed, kind, cursor, bytes, &header, &verifier)?
        };
        // A stalled entry is offered again: its sequence number is not seen yet.
        if outcome != Outcome::Rejected && outcome != Outcome::Stalled {
            self.seen_in(feed, header.doc_id, header.device_id, header.device_seq)?;
        }
        Ok(outcome)
    }

    /// Decrypts and imports a document entry, or parks it.
    fn apply_entry_in(
        &mut self,
        feed: FeedId,
        kind: FeedKind,
        cursor: u64,
        bytes: &[u8],
        header: &kb_sync::v2::EntryHeader,
        verifier: &VerifyingKey,
    ) -> Result<Outcome, ApiError> {
        let now = self.clock.now_ms();
        let existing = self.db.doc_get(header.doc_id).map_err(map_storage_error)?;
        let (doc_kind, blobs) = match &existing {
            Some(row) => {
                let keys = self.reading_keys(row, header.key_epoch)?;
                let opened = open_with_keys(&keys, bytes, verifier, |key| {
                    open_entry(bytes, key, verifier).map(|(_, body)| body)
                })?;
                match opened {
                    Some(body) => {
                        if body.min_reader_version > kb_doc::FORMAT_VERSION {
                            tracing::warn!(doc = %short_id(&header.doc_id), "entry needs a newer app version; stored as is");
                        }
                        (body.doc_kind, body.updates)
                    }
                    None => {
                        self.park_in(feed, cursor, header.doc_id, bytes, PENDING_KEY)?;
                        return Ok(Outcome::Pending);
                    }
                }
            }
            None => match self.open_unknown_v2(
                bytes,
                header.doc_id,
                header.key_epoch,
                verifier,
                false,
            )? {
                Some(opened) => opened,
                None => {
                    self.park_in(feed, cursor, header.doc_id, bytes, PENDING_KEY)?;
                    return Ok(Outcome::Pending);
                }
            },
        };
        let Some(dk) = doc_kind_code(doc_kind) else {
            tracing::warn!(cursor, "entry with an unknown document kind rejected");
            return Ok(Outcome::Rejected);
        };
        if kind == FeedKind::Shared {
            // The role the entry needed is the one its decrypted kind says (ADR-0043): a comment
            // stream's is Commenter's, every other document's is Editor's.
            let needed = if dk == DocKindCode::Comments {
                MemberRole::Commenter
            } else {
                MemberRole::Editor
            };
            let role = self.role_at(feed, header.account_id, cursor);
            let acceptable = if role.is_none_or(|role| role < needed) {
                false
            } else if dk == DocKindCode::Comments {
                // The stream of a page this device does not know yet waits for the page.
                let space = self.feed_space(feed)?;
                match self.comment_stream_status(space, header.doc_id) {
                    StreamStatus::Is => true,
                    StreamStatus::Other => false,
                    StreamStatus::Unknown => {
                        self.park_in(feed, cursor, header.doc_id, bytes, PENDING_STRUCTURE)?;
                        return Ok(Outcome::Pending);
                    }
                }
            } else {
                self.shared_doc_acceptable(feed, header.doc_id, dk, &blobs)?
            };
            if !acceptable {
                self.quarantine(
                    feed,
                    cursor,
                    header.doc_id,
                    header.account_id,
                    header.device_id,
                    RejectReason::RoleForbids,
                    bytes,
                )?;
                return Ok(Outcome::Rejected);
            }
        }
        let carried = Carried {
            writer: header.account_id,
            device: header.device_id,
            snapshot: false,
        };
        match self.apply_blobs_in(feed, cursor, header.doc_id, dk, &blobs, bytes, now, carried)? {
            Content::Done => {}
            Content::Parked => return Ok(Outcome::Pending),
            Content::Stalled => return Ok(Outcome::Stalled),
            Content::Rejected => {
                self.quarantine(
                    feed,
                    cursor,
                    header.doc_id,
                    header.account_id,
                    header.device_id,
                    RejectReason::RoleForbids,
                    bytes,
                )?;
                return Ok(Outcome::Rejected);
            }
        }
        let mut state = self.doc_state_in(feed, header.doc_id)?;
        state.entries_since_snapshot += 1;
        state.bytes_since_snapshot += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.db
            .sync_doc_state_put_in(feed, &state)
            .map_err(map_storage_error)?;
        if kind == FeedKind::Shared {
            self.note_entry_mark(header.doc_id, header.device_id, cursor, &blobs)?;
        }
        Ok(Outcome::Applied)
    }

    /// Records where the entry left the document, for the dominance check of later snapshots
    /// (`sync_entry_marks`): the end version vector of its last update blob.
    fn note_entry_mark(
        &mut self,
        doc_id: DocId,
        device_id: DeviceId,
        cursor: u64,
        blobs: &[Vec<u8>],
    ) -> Result<(), ApiError> {
        let Some(end_vv) = blobs
            .iter()
            .rev()
            .find_map(|blob| kb_doc::update_end_vv(blob).ok())
        else {
            return Ok(());
        };
        self.db
            .entry_mark_put(&EntryMarkRow {
                doc_id,
                device_id,
                end_vv: end_vv.0,
                cursor,
            })
            .map_err(map_storage_error)
    }

    /// The checks of a shared feed that need the decrypted body: its kind must agree with the
    /// document id, and it must not reach into another space or the account document.
    fn shared_doc_acceptable(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        dk: DocKindCode,
        blobs: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        let space = self.feed_space(feed)?;
        match dk {
            DocKindCode::Account => Ok(false),
            DocKindCode::Structure => {
                let Some(space) = space else {
                    return Ok(false);
                };
                if let Some(open) = self.docs.structures.get(&space) {
                    return Ok(open.doc_id == doc_id);
                }
                // A structure document this device does not have: it must be the space's own.
                let doc =
                    Doc::load(kb_doc::DocKind::Structure, 1, None, blobs).map_err(map_doc_error)?;
                let model = kb_model::StructureDoc::from_doc(doc).map_err(map_model_error)?;
                Ok(model.space().id == space)
            }
            DocKindCode::Page => {
                if self.is_comment_stream(space, doc_id) {
                    return Ok(false);
                }
                let page = PageId::from_bytes(doc_id.to_bytes());
                match (self.space_of_page(page), space) {
                    (Ok(found), Some(space)) => Ok(found == space),
                    (Ok(_), None) => Ok(false),
                    // The structure has not told of the page yet: the item waits for it.
                    (Err(_), _) => Ok(true),
                }
            }
            // A comment stream is acceptable when its document id is the derived id of a page of
            // this space: the decrypted kind and the id agree (ADR-0043, ADR-0048). The callers park an
            // item whose page they cannot place yet before they ask.
            DocKindCode::Comments => Ok(self.is_comment_stream(space, doc_id)),
        }
    }

    /// Opens an entry or snapshot of a document this device does not know: under the keyring record
    /// that names it, or under the account document key. `None` when no key opens it yet.
    fn open_unknown_v2(
        &mut self,
        bytes: &[u8],
        doc_id: DocId,
        key_epoch: u32,
        verifier: &VerifyingKey,
        snapshot: bool,
    ) -> Result<Option<Opened>, ApiError> {
        let mut candidates = self.guess_keys(doc_id, key_epoch)?;
        if let Some(key) = self.account_doc_key_for(key_epoch) {
            candidates.push(key);
        }
        for key in &candidates {
            let opened = if snapshot {
                open_snapshot(bytes, key, verifier)
                    .map(|(_, body)| (body.doc_kind, vec![body.snapshot]))
            } else {
                open_entry(bytes, key, verifier).map(|(_, body)| (body.doc_kind, body.updates))
            };
            match opened {
                Ok(opened) => return Ok(Some(opened)),
                Err(SyncError::Authentication) => continue,
                Err(err) => return Err(map_sync_error(err)),
            }
        }
        Ok(None)
    }

    /// Imports blobs into the document of `kind`: `Parked` when the item was parked because the
    /// page's structure is unknown or its history is not here, `Stalled` or `Rejected` for a comment
    /// stream's content that needs a device log or fails a check (`carried` says who wrote the
    /// item and whether it is a snapshot). Only an entry notifies about a page or a structure: it
    /// has an author to name.
    #[allow(clippy::too_many_arguments)]
    fn apply_blobs_in(
        &mut self,
        feed: FeedId,
        cursor: u64,
        doc_id: DocId,
        kind: DocKindCode,
        blobs: &[Vec<u8>],
        raw: &[u8],
        now: i64,
        carried: Carried,
    ) -> Result<Content, ApiError> {
        let me = self.sync_identity()?.map(|(account, _)| account);
        let notify = (!carried.snapshot
            && Some(carried.writer) != me
            && self.comment_notifications_allowed(feed)?)
        .then_some(carried.writer);
        let complete = match kind {
            DocKindCode::Account => self.apply_account_updates(blobs, now)?,
            DocKindCode::Structure => self.apply_structure_updates(doc_id, blobs, now, notify)?,
            DocKindCode::Page => {
                let page_id = PageId::from_bytes(doc_id.to_bytes());
                if self.space_of_page(page_id).is_err() {
                    self.park_in(feed, cursor, doc_id, raw, PENDING_STRUCTURE)?;
                    return Ok(Content::Parked);
                }
                // An entry is a session of its writer; a snapshot is history of many hands (no author).
                let writer = (!carried.snapshot).then_some(crate::history::Writer::Remote {
                    account: carried.writer,
                    device: carried.device,
                });
                self.apply_page_updates(page_id, blobs, now, notify, writer)?
            }
            DocKindCode::Comments => {
                match self.apply_comment_updates(feed, cursor, doc_id, blobs, carried, now)? {
                    CommentApply::Applied => true,
                    CommentApply::Quarantined => return Ok(Content::Rejected),
                    CommentApply::NeedsLog => {
                        if carried.snapshot {
                            self.park_in(feed, cursor, doc_id, raw, PENDING_SNAPSHOT)?;
                            return Ok(Content::Parked);
                        }
                        return Ok(Content::Stalled);
                    }
                    CommentApply::PageUnknown => {
                        self.park_in(feed, cursor, doc_id, raw, PENDING_STRUCTURE)?;
                        return Ok(Content::Parked);
                    }
                }
            }
        };
        if !complete {
            self.park_in(feed, cursor, doc_id, raw, PENDING_HISTORY)?;
            return Ok(Content::Parked);
        }
        Ok(Content::Done)
    }

    // ----- the keyring entry ----------------------------------------------------------------

    fn apply_feed_keyring(
        &mut self,
        feed: FeedId,
        kind: FeedKind,
        cursor: u64,
        bytes: &[u8],
    ) -> Result<Outcome, ApiError> {
        let header = peek_keyring(bytes).map_err(map_sync_error)?;
        if header.feed != feed {
            tracing::warn!(cursor, "a keyring entry of another feed rejected");
            return Ok(Outcome::Rejected);
        }
        let (me, my_device) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let shared = kind == FeedKind::Shared;
        let verifier = if shared {
            if self.role_at(feed, header.account_id, cursor).is_none() {
                self.quarantine(
                    feed,
                    cursor,
                    KEYRING_DOC_ID,
                    header.account_id,
                    header.device_id,
                    RejectReason::NotAMember,
                    bytes,
                )?;
                self.seen_in(feed, KEYRING_DOC_ID, header.device_id, header.device_seq)?;
                return Ok(Outcome::Rejected);
            }
            let Some(key) = self.member_device_key(header.account_id, header.device_id)? else {
                return Ok(Outcome::Stalled);
            };
            verify_signature(bytes, &key).map_err(map_sync_error)?;
            key
        } else {
            if header.account_id != me {
                tracing::warn!(cursor, "keyring entry of another account rejected");
                return Ok(Outcome::Rejected);
            }
            self.device_verifier(header.device_id)?
        };
        if !self.continuity_in(feed, KEYRING_DOC_ID, header.device_id, header.device_seq)? {
            return Ok(Outcome::Rejected);
        }
        if header.device_id == my_device {
            self.seen_in(feed, KEYRING_DOC_ID, header.device_id, header.device_seq)?;
            return Ok(Outcome::Own);
        }
        let outcome = self.apply_keyring_entry_in(feed, kind, cursor, bytes, &header, &verifier)?;
        if outcome != Outcome::Rejected {
            self.seen_in(feed, KEYRING_DOC_ID, header.device_id, header.device_seq)?;
        }
        Ok(outcome)
    }

    /// Opens a keyring entry (the root key's keyring key for the personal feed, the space key's for a
    /// shared one), verifies every item and merges it.
    fn apply_keyring_entry_in(
        &mut self,
        feed: FeedId,
        kind: FeedKind,
        cursor: u64,
        bytes: &[u8],
        header: &kb_sync::v2::KeyringHeader,
        verifier: &VerifyingKey,
    ) -> Result<Outcome, ApiError> {
        let keys: Vec<Secret32> = if kind == FeedKind::Shared {
            match self.feed_sk(feed, header.key_epoch)? {
                Some(sk) => vec![space_keyring_key(&sk, feed)],
                None => Vec::new(),
            }
        } else {
            self.keyring_keys()
        };
        let opened = if keys.is_empty() {
            None
        } else {
            open_with_keys(&keys, bytes, verifier, |key| {
                open_keyring(bytes, key, verifier)
            })?
        };
        let Some((_, items)) = opened else {
            self.park_in(feed, cursor, KEYRING_DOC_ID, bytes, PENDING_KEY)?;
            return Ok(Outcome::Pending);
        };
        if kind == FeedKind::Shared {
            // An item signed by a device the held log of the account does not list yet waits for the
            // log (the account may have added the device since it was fetched).
            for item in &items {
                let signer = match item {
                    KeyringItem::Record(r) => r.created_by,
                    KeyringItem::Tombstone(t) => t.purged_by,
                };
                if matches!(self.member_device_key(header.account_id, signer), Ok(None)) {
                    return Ok(Outcome::Stalled);
                }
            }
        }
        let mut changed = false;
        let mut accepted = 0u32;
        let mut refused = 0u32;
        let mut deferred = 0u32;
        // The spaces whose view-link records this entry carries (M13 slice 4): their list is re-read.
        let link_spaces: std::collections::BTreeSet<kb_types::SpaceId> = items
            .iter()
            .filter(|item| matches!(item.subject(), kb_keys::KeySubject::Link { .. }))
            .map(KeyringItem::space_id)
            .collect();
        for item in items {
            let signer = match &item {
                KeyringItem::Record(r) => r.created_by,
                KeyringItem::Tombstone(t) => t.purged_by,
            };
            if kind == FeedKind::Personal {
                let key = self.device_verifier(signer)?;
                item.verify(&key).map_err(map_keys_error)?;
                let row = keyring_row(&item);
                if self.merge_keyring_item(item)? {
                    changed = true;
                    self.db
                        .transaction(|tx| {
                            tx.keyring_put(&row)?;
                            tx.keyring_mark_published(std::slice::from_ref(&row))
                        })
                        .map_err(map_storage_error)?;
                }
                accepted += 1;
                continue;
            }
            match self.accept_shared_keyring_item(feed, cursor, header, signer, item)? {
                ItemVerdict::Refused => refused += 1,
                ItemVerdict::Deferred => deferred += 1,
                ItemVerdict::Merged(applied) => {
                    accepted += 1;
                    changed |= applied;
                }
            }
        }
        if changed {
            self.security.space_keys.clear();
            self.sync.known_spaces = None;
            self.events.push(Event::SecurityChanged);
            for space_id in link_spaces {
                self.push_event_once(Event::ViewLinksChanged { space_id });
            }
            self.retry_pending_all()?;
        }
        if refused > 0 {
            self.quarantine(
                feed,
                cursor,
                KEYRING_DOC_ID,
                header.account_id,
                header.device_id,
                RejectReason::RoleForbids,
                bytes,
            )?;
        }
        if accepted == 0 && refused > 0 {
            return Ok(Outcome::Rejected);
        }
        if deferred > 0 && refused == 0 {
            // A Commenter's key record for a document nobody here can place yet (a late joiner reads
            // the keyring before the structure that tells of the page): the entry is read again when
            // the structure has arrived (what it already merged stays merged).
            self.park_in(feed, cursor, KEYRING_DOC_ID, bytes, PENDING_STRUCTURE)?;
            return Ok(Outcome::Pending);
        }
        Ok(Outcome::Applied)
    }

    /// One keyring item of a shared feed: its signer must be a device of the entry's account, its
    /// signature must verify, it must be about the feed's space and the account's role at the cursor
    /// must allow it (Editor for document and attachment records, Commenter for those of a comment
    /// stream, Owner for a tombstone, never a `Space` record). A Commenter's `Doc` record for a
    /// document this device cannot place yet is `Deferred`: it could be a stream, and it could be a
    /// page's key made before its Editors made it (which a Commenter may not), so it waits for the
    /// structure. `Refused` when not allowed; otherwise whether the keyring changed.
    fn accept_shared_keyring_item(
        &mut self,
        feed: FeedId,
        cursor: u64,
        header: &kb_sync::v2::KeyringHeader,
        signer: DeviceId,
        item: KeyringItem,
    ) -> Result<ItemVerdict, ApiError> {
        let Some(role) = self.role_at(feed, header.account_id, cursor) else {
            return Ok(ItemVerdict::Refused);
        };
        let Some(key) = self
            .member_device_key(header.account_id, signer)
            .ok()
            .flatten()
        else {
            return Ok(ItemVerdict::Refused);
        };
        if item.verify(&key).is_err() {
            return Ok(ItemVerdict::Refused);
        }
        let space = match self.feed_space(feed)? {
            Some(space) => space,
            None => {
                // The first key record of a feed whose space was not known names it.
                if !self.feed_bind_space(feed, item.space_id())? {
                    return Ok(ItemVerdict::Refused);
                }
                item.space_id()
            }
        };
        if item.space_id() != space {
            return Ok(ItemVerdict::Refused);
        }
        let allowed = match &item {
            KeyringItem::Record(record) => match record.subject {
                kb_keys::KeySubject::Space { .. } => false,
                kb_keys::KeySubject::Doc { doc_id, .. } => {
                    match self.comment_stream_status(Some(space), doc_id) {
                        StreamStatus::Is => role >= MemberRole::Commenter,
                        StreamStatus::Other => role >= MemberRole::Editor,
                        StreamStatus::Unknown => {
                            if role >= MemberRole::Editor {
                                true
                            } else if role >= MemberRole::Commenter {
                                return Ok(ItemVerdict::Deferred);
                            } else {
                                false
                            }
                        }
                    }
                }
                kb_keys::KeySubject::Attachment { .. } => role >= MemberRole::Editor,
                // A view link's key (M13 slice 4): written by whoever may create the link, an Editor or
                // an Owner (the setting "Editors may create view links" is enforced by clients before
                // the record is made, ADR-0051); a Viewer or Commenter writes none.
                kb_keys::KeySubject::Link { .. } => role >= MemberRole::Editor,
            },
            KeyringItem::Tombstone(_) => role.is_owner(),
        };
        if !allowed {
            return Ok(ItemVerdict::Refused);
        }
        let row = keyring_row(&item);
        let is_owner = role.is_owner();
        let changed = self.merge_keyring_item_ordered(feed, cursor, item, &|_| is_owner)?;
        if changed {
            self.db
                .transaction(|tx| {
                    tx.keyring_put(&row)?;
                    tx.keyring_mark_published(std::slice::from_ref(&row))
                })
                .map_err(map_storage_error)?;
        }
        Ok(ItemVerdict::Merged(changed))
    }

    // ----- parked items ---------------------------------------------------------------------

    /// Retries the parked items of every feed.
    pub(crate) fn retry_pending_all(&mut self) -> Result<(), ApiError> {
        for row in self.db.sync_feed_list().map_err(map_storage_error)? {
            self.retry_pending_in(row.feed)?;
        }
        Ok(())
    }

    /// Retries the parked items of a feed until a pass applies nothing more.
    pub(crate) fn retry_pending_in(&mut self, feed: FeedId) -> Result<(), ApiError> {
        let Some(feed_row) = self.db.sync_feed_get(feed).map_err(map_storage_error)? else {
            return Ok(());
        };
        let kind = feed_row.kind;
        loop {
            let rows = self
                .db
                .sync_pending_all_in(feed)
                .map_err(map_storage_error)?;
            if rows.is_empty() {
                return Ok(());
            }
            let mut progressed = false;
            for row in rows {
                self.db
                    .sync_pending_delete_in(feed, row.cursor, row.doc_id)
                    .map_err(map_storage_error)?;
                let outcome = if peek_snapshot(&row.bytes).is_ok() {
                    self.apply_feed_snapshot(feed, kind, &row.bytes)?
                } else if row.doc_id == KEYRING_DOC_ID {
                    let header = peek_keyring(&row.bytes).map_err(map_sync_error)?;
                    let verifier =
                        self.parked_verifier(kind, header.account_id, header.device_id)?;
                    match verifier {
                        Some(verifier) => self.apply_keyring_entry_in(
                            feed, kind, row.cursor, &row.bytes, &header, &verifier,
                        )?,
                        None => Outcome::Stalled,
                    }
                } else {
                    let header = peek_entry(&row.bytes).map_err(map_sync_error)?;
                    let verifier =
                        self.parked_verifier(kind, header.account_id, header.device_id)?;
                    match verifier {
                        Some(verifier) => self.apply_entry_in(
                            feed, kind, row.cursor, &row.bytes, &header, &verifier,
                        )?,
                        None => Outcome::Stalled,
                    }
                };
                match outcome {
                    Outcome::Applied => {
                        tracing::debug!(cursor = row.cursor, "parked item applied");
                        progressed = true;
                    }
                    Outcome::Stalled => {
                        // Not decidable yet: it waits as it did.
                        self.db
                            .sync_pending_put_in(feed, &row)
                            .map_err(map_storage_error)?;
                    }
                    _ => {}
                }
            }
            if !progressed {
                return Ok(());
            }
        }
    }

    /// The key that verifies a parked item's signature.
    fn parked_verifier(
        &mut self,
        kind: FeedKind,
        account: AccountId,
        device: DeviceId,
    ) -> Result<Option<VerifyingKey>, ApiError> {
        if kind == FeedKind::Personal {
            return self.device_verifier(device).map(Some);
        }
        self.member_device_key(account, device)
    }

    // ----- snapshots ------------------------------------------------------------------------

    /// Snapshots fetched for bootstrapping or for verification: each is verified (signature, key,
    /// and snapshot dominance for a shared feed) before it is applied; the ones that fail are
    /// answered in `disputed` and never applied.
    pub fn sync_feed_apply_snapshots(
        &mut self,
        feed: FeedId,
        page: &SnapshotsPage,
    ) -> Result<SnapshotsApplied, ApiError> {
        self.shared_load()?;
        let row = self.feed_row(feed)?;
        if let Some(epoch) = self.server_epoch()?
            && page.server_epoch != epoch
        {
            return Err(ApiError::storage(
                "the snapshots belong to another server epoch",
            ));
        }
        let mut out = SnapshotsApplied::default();
        for bytes in &page.snapshots {
            match self.apply_feed_snapshot(feed, row.kind, bytes)? {
                Outcome::Applied => out.applied += 1,
                Outcome::Disputed(doc_id, covers) => out.disputed.push((doc_id, covers)),
                _ => {}
            }
        }
        self.shared
            .snapshots_checked_at
            .insert(feed, self.feed_row(feed)?.cursor);
        if out.applied > 0 {
            self.retry_pending_in(feed)?;
            self.events.push(Event::SpacesChanged);
            self.events.push(Event::SyncChanged);
        }
        Ok(out)
    }

    fn note_covers_in(&mut self, feed: FeedId, covers: u64) -> Result<(), ApiError> {
        let mut row = self.feed_row(feed)?;
        if covers > row.max_covers {
            row.max_covers = covers;
            self.feed_row_put(&row)?;
        }
        Ok(())
    }

    /// One snapshot of a feed, fully processed.
    fn apply_feed_snapshot(
        &mut self,
        feed: FeedId,
        kind: FeedKind,
        bytes: &[u8],
    ) -> Result<Outcome, ApiError> {
        let now = self.clock.now_ms();
        let header = peek_snapshot(bytes).map_err(map_sync_error)?;
        if header.feed != feed {
            return Ok(Outcome::Rejected);
        }
        let (me, my_device) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let shared = kind == FeedKind::Shared;
        let verifier;
        if shared {
            let Some(key) = self.member_device_key(header.account_id, header.device_id)? else {
                // The writer's log is not here: the snapshot waits for it.
                self.park_in(
                    feed,
                    header.covers_cursor,
                    header.doc_id,
                    bytes,
                    PENDING_SNAPSHOT,
                )?;
                return Ok(Outcome::Pending);
            };
            verify_signature(bytes, &key).map_err(map_sync_error)?;
            verifier = key;
            self.note_covers_in(feed, header.covers_cursor)?;
            // The role is judged at the cursor the snapshot covers, which the pull has to have
            // reached for the membership at that cursor to be known.
            if self.feed_row(feed)?.cursor < header.covers_cursor {
                self.park_in(
                    feed,
                    header.covers_cursor,
                    header.doc_id,
                    bytes,
                    PENDING_SNAPSHOT,
                )?;
                return Ok(Outcome::Pending);
            }
            match self.snapshot_writer_role(feed, header.account_id, header.covers_cursor) {
                None => {
                    self.quarantine(
                        feed,
                        header.covers_cursor,
                        header.doc_id,
                        header.account_id,
                        header.device_id,
                        RejectReason::NotAMember,
                        bytes,
                    )?;
                    return Ok(Outcome::Rejected);
                }
                Some(role) => {
                    let space = self.feed_space(feed)?;
                    let need = self.need_for(space, header.doc_id).role();
                    if role < need {
                        self.quarantine(
                            feed,
                            header.covers_cursor,
                            header.doc_id,
                            header.account_id,
                            header.device_id,
                            RejectReason::RoleForbids,
                            bytes,
                        )?;
                        return Ok(Outcome::Rejected);
                    }
                }
            }
        } else {
            if header.account_id != me {
                return Ok(Outcome::Rejected);
            }
            self.note_covers_in(feed, header.covers_cursor)?;
            verifier = self.device_verifier(header.device_id)?;
        }
        if header.device_id == my_device && !self.rebuilding(feed, header.doc_id)? {
            return Ok(Outcome::Own);
        }
        let existing = self.db.doc_get(header.doc_id).map_err(map_storage_error)?;
        let state = self.doc_state_in(feed, header.doc_id)?;
        if existing.is_some()
            && state
                .remote_snapshot_covers
                .is_some_and(|covers| header.covers_cursor <= covers)
        {
            self.drop_superseded_in(feed, header.doc_id, header.covers_cursor)?;
            return Ok(Outcome::Covered);
        }
        let (doc_kind, blobs) = match &existing {
            Some(row) => {
                let keys = self.reading_keys(row, header.key_epoch)?;
                let opened = open_with_keys(&keys, bytes, &verifier, |key| {
                    open_snapshot(bytes, key, &verifier).map(|(_, body)| body)
                })?;
                match opened {
                    Some(body) => (body.doc_kind, vec![body.snapshot]),
                    None => {
                        self.park_in(
                            feed,
                            header.covers_cursor,
                            header.doc_id,
                            bytes,
                            PENDING_SNAPSHOT,
                        )?;
                        return Ok(Outcome::Pending);
                    }
                }
            }
            None => match self.open_unknown_v2(
                bytes,
                header.doc_id,
                header.key_epoch,
                &verifier,
                true,
            )? {
                Some(opened) => opened,
                None => {
                    self.park_in(
                        feed,
                        header.covers_cursor,
                        header.doc_id,
                        bytes,
                        PENDING_SNAPSHOT,
                    )?;
                    return Ok(Outcome::Pending);
                }
            },
        };
        let Some(dk) = doc_kind_code(doc_kind) else {
            return Ok(Outcome::Rejected);
        };
        if shared {
            // As for an entry, the role the snapshot needed is the one its decrypted kind says.
            let needed = if dk == DocKindCode::Comments {
                MemberRole::Commenter
            } else {
                MemberRole::Editor
            };
            let role = self.snapshot_writer_role(feed, header.account_id, header.covers_cursor);
            let space = self.feed_space(feed)?;
            let acceptable = if role.is_none_or(|role| role < needed) {
                false
            } else if dk == DocKindCode::Comments {
                match self.comment_stream_status(space, header.doc_id) {
                    StreamStatus::Is => true,
                    StreamStatus::Other => false,
                    StreamStatus::Unknown => {
                        self.park_in(
                            feed,
                            header.covers_cursor,
                            header.doc_id,
                            bytes,
                            PENDING_SNAPSHOT,
                        )?;
                        return Ok(Outcome::Pending);
                    }
                }
            } else {
                self.shared_doc_acceptable(feed, header.doc_id, dk, &blobs)?
            };
            if !acceptable {
                self.quarantine(
                    feed,
                    header.covers_cursor,
                    header.doc_id,
                    header.account_id,
                    header.device_id,
                    RejectReason::RoleForbids,
                    bytes,
                )?;
                return Ok(Outcome::Rejected);
            }
            if let Some(blob) = blobs.first() {
                let dominates = if dk == DocKindCode::Comments {
                    match self.comment_stream_page(space, header.doc_id) {
                        Some((_, page)) => {
                            self.comment_snapshot_dominates(page, blob, header.covers_cursor)?
                        }
                        None => true,
                    }
                } else {
                    self.snapshot_dominates(header.doc_id, header.covers_cursor, blob, dk)?
                };
                if !dominates {
                    return self.snapshot_disputed(feed, &header, bytes);
                }
            }
        }
        let carried = Carried {
            writer: header.account_id,
            device: header.device_id,
            snapshot: true,
        };
        match self.apply_blobs_in(
            feed,
            header.covers_cursor,
            header.doc_id,
            dk,
            &blobs,
            bytes,
            now,
            carried,
        )? {
            Content::Done => {}
            Content::Parked | Content::Stalled => return Ok(Outcome::Pending),
            Content::Rejected => {
                self.quarantine(
                    feed,
                    header.covers_cursor,
                    header.doc_id,
                    header.account_id,
                    header.device_id,
                    RejectReason::RoleForbids,
                    bytes,
                )?;
                return Ok(Outcome::Rejected);
            }
        }
        let mut state = self.doc_state_in(feed, header.doc_id)?;
        state.remote_snapshot_covers = Some(
            state
                .remote_snapshot_covers
                .map_or(header.covers_cursor, |c| c.max(header.covers_cursor)),
        );
        self.db
            .sync_doc_state_put_in(feed, &state)
            .map_err(map_storage_error)?;
        self.drop_superseded_in(feed, header.doc_id, header.covers_cursor)?;
        Ok(Outcome::Applied)
    }

    /// Whether a snapshot covering `covers` contains every entry this device applied for the document
    /// at a cursor up to that (snapshot dominance, ADR-0045): the snapshot's version vector must
    /// include the end version vector of the last entry of every writer device.
    fn snapshot_dominates(
        &self,
        doc_id: DocId,
        covers: u64,
        blob: &[u8],
        dk: DocKindCode,
    ) -> Result<bool, ApiError> {
        let marks: Vec<EntryMarkRow> = self
            .db
            .entry_marks_for_doc(doc_id)
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|mark| mark.cursor <= covers)
            .collect();
        if marks.is_empty() {
            return Ok(true);
        }
        let doc = Doc::load(engine_kind(dk), 1, Some(blob), &[]).map_err(map_doc_error)?;
        let version = doc.version();
        for mark in marks {
            let end = kb_doc::Version(mark.end_vv);
            if !version.includes(&end).map_err(map_doc_error)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// A snapshot that omits entries it covers: never applied, quarantined, the Owners told, a full
    /// snapshot republished by this device (ADR-0045).
    fn snapshot_disputed(
        &mut self,
        feed: FeedId,
        header: &kb_sync::v2::SnapshotHeader,
        bytes: &[u8],
    ) -> Result<Outcome, ApiError> {
        tracing::warn!(feed = %feed.short_hex(), covers = header.covers_cursor, "a snapshot omits entries it covers");
        self.quarantine(
            feed,
            header.covers_cursor,
            header.doc_id,
            header.account_id,
            header.device_id,
            RejectReason::SnapshotNotDominant,
            bytes,
        )?;
        let now = self.clock.now_ms();
        self.dispute_add(feed, header.doc_id, header.account_id, now)?;
        self.feed_mark_for_snapshot(feed, header.doc_id)?;
        if let Some(space_id) = self.feed_space(feed)? {
            self.events.push(Event::SnapshotDisputed {
                space_id,
                page_id: self.page_of_doc(header.doc_id),
                account_id: header.account_id,
            });
        }
        self.sync_note_local_change();
        Ok(Outcome::Disputed(header.doc_id, header.covers_cursor))
    }

    /// Parked items a snapshot supersedes (its document, at or below the cursor it covers) are
    /// dropped: the snapshot carries what they hold.
    fn drop_superseded_in(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        covers: u64,
    ) -> Result<(), ApiError> {
        let rows = self
            .db
            .sync_pending_take_for_doc_in(feed, doc_id)
            .map_err(map_storage_error)?;
        for row in rows {
            if row.cursor > covers {
                self.db
                    .sync_pending_put_in(feed, &row)
                    .map_err(map_storage_error)?;
            }
        }
        Ok(())
    }
}
