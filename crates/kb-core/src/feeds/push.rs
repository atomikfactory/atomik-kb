//! What a feed's outbox carries (v2 headers): the keyring entry and one entry per document with local
//! updates the feed lacks, sealed under the current epoch of the feed (a shared feed: under the space
//! key's derivatives); the snapshots this device makes; the snapshot marks that bring other devices'
//! snapshots in for verification.

use std::collections::HashMap;

use kb_keys::KeyringItem;
use kb_keys::space_keys::{SpaceKey, space_keyring_key};
use kb_storage::{
    DocKindCode, DocQueries, SecurityQueries, SyncDocState, SyncFeedRow, SyncQueries, UpdateOrigin,
};
use kb_sync::v2::{
    EntryHeader, KeyringHeader, OutboundEntry, SnapshotHeader, SnapshotMark, seal_entry,
    seal_keyring, seal_snapshot,
};
use kb_sync::{EntryBody, SnapshotBody};
use kb_types::sync::{FeedKind, KEYRING_DOC_ID, SNAPSHOT_AFTER_BYTES, SNAPSHOT_AFTER_ENTRIES};
use kb_types::{ApiError, DocId, FeedId, MemberRole};

use crate::sync::{code_of, doc_state_default, engine_of};
use crate::{Core, map_keys_error, map_storage_error, short_id};

/// What may be pushed into a shared feed right now: the epoch and the key it is sealed under.
struct PushGate {
    epoch: u32,
    key: SpaceKey,
    /// A Commenter's device: it may send the records of comment streams (and the key records of
    /// those streams) and nothing else (ADR-0043, ADR-0048).
    comments_only: bool,
}

impl Core {
    /// `Some` when this device may write into the shared feed now: it is an Editor or an Owner, or a
    /// Commenter who writes comment streams only (counting the entries it minted and the server has
    /// not confirmed), the feed is open and the key of the feed's epoch is here.
    fn push_gate(&mut self, feed: FeedId) -> Result<Option<PushGate>, ApiError> {
        if !self.feed_is_live(feed)? {
            return Ok(None);
        }
        let me = self.me()?;
        let Some(state) = self.tentative_state(feed)? else {
            return Ok(None);
        };
        if state.closed.is_some() {
            return Ok(None);
        }
        let editor = state.role_of(me).is_some_and(|r| r >= MemberRole::Editor);
        #[cfg(test)]
        let editor = editor || (self.shared.force_push && state.role_of(me).is_some());
        let commenter = state.role_of(me).is_some_and(MemberRole::can_comment);
        if !editor && !commenter {
            return Ok(None);
        }
        let Some(key) = self.feed_sk(feed, state.epoch)? else {
            return Ok(None);
        };
        Ok(Some(PushGate {
            epoch: state.epoch,
            key,
            comments_only: !editor,
        }))
    }

    /// The sealed entries of the feed ready to upload (document and keyring entries; membership
    /// entries go through `sync_membership_pending`). Resealed under the current epoch after a
    /// `StaleEpoch`: the outbox is rebuilt from the local state on every call, so the entry the
    /// server refused comes back under the epoch the pull taught.
    pub fn sync_feed_outbox(&mut self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError> {
        self.shared_load()?;
        let row = self.feed_row(feed)?;
        if row.fork {
            return Ok(Vec::new());
        }
        let (account_id, device_id) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let Some(personal) = self.personal_feed()? else {
            return Ok(Vec::new());
        };
        let shared_map = self.shared_space_map()?;
        let gate = if row.kind == FeedKind::Shared {
            match self.push_gate(feed)? {
                Some(gate) => Some(gate),
                None => return Ok(Vec::new()),
            }
        } else {
            None
        };
        let comments_only = gate.as_ref().is_some_and(|gate| gate.comments_only);
        let docs: Vec<kb_storage::DocRow> = self
            .db
            .docs_all()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|doc| Self::feed_of_doc(doc, &shared_map, personal) == Some(feed))
            .filter(|doc| !comments_only || doc.kind == DocKindCode::Comments)
            .collect();
        // A document gets the key record of the feed's current epoch on its next write (lazy
        // rotation, ADR-0044): made before the keyring entry is built, so that it travels with it.
        for doc in &docs {
            if doc.kind != DocKindCode::Account
                && doc.space_id.is_some()
                && self.doc_has_unsent(feed, doc)?
            {
                self.ensure_doc_key_of(doc)?;
            }
        }
        let mut out = Vec::new();

        let mut unpublished = self.keyring_unpublished_for(feed, &shared_map)?;
        if gate.is_some() {
            // The records of the epochs before the space was shared are wrapped under keys the members
            // never get (ADR-0045 section 10): they stay in the owner's personal feed.
            unpublished = self.drop_pre_share_records(feed, unpublished)?;
        }
        if comments_only {
            // A Commenter sends the key records of comment streams only; anything else of its
            // keyring stays where it is (an Editor's device sends it).
            unpublished = self.keep_comment_stream_records(unpublished)?;
        }
        if !unpublished.is_empty() {
            let items: Vec<KeyringItem> = unpublished
                .iter()
                .map(|row| KeyringItem::decode(&row.record).map_err(map_keys_error))
                .collect::<Result<_, _>>()?;
            let state = self.doc_state_in(feed, KEYRING_DOC_ID)?;
            let (key, key_epoch) = match &gate {
                Some(gate) => (space_keyring_key(&gate.key, feed), gate.epoch),
                None => (
                    kb_sync::keyring_key(&self.ark),
                    self.security.ark_generation,
                ),
            };
            let header = KeyringHeader {
                feed,
                account_id,
                device_id,
                device_seq: state.next_device_seq,
                key_epoch,
            };
            let bytes = seal_keyring(&header, &items, &key, self.signer(), self.rng.as_ref());
            self.shared.keyring_in_flight.insert(feed, unpublished);
            out.push(OutboundEntry {
                doc_id: KEYRING_DOC_ID,
                device_seq: state.next_device_seq,
                bytes,
                up_to_seq: -1,
            });
        }

        let now = self.clock.now_ms();
        for doc in docs {
            if let Some(entry) = self.doc_entry(feed, &doc, account_id, device_id, now)? {
                out.push(entry);
            }
        }
        Ok(out)
    }

    /// Whether a document has anything this device would send into the feed.
    fn doc_has_unsent(&self, feed: FeedId, doc: &kb_storage::DocRow) -> Result<bool, ApiError> {
        Ok(!self.doc_unsent_updates(feed, doc.doc_id)?.is_empty())
    }

    /// The per-feed push state of a document.
    pub(crate) fn doc_state_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<SyncDocState, ApiError> {
        Ok(self
            .db
            .sync_doc_state_get_in(feed, doc_id)
            .map_err(map_storage_error)?
            .unwrap_or_else(|| doc_state_default(doc_id)))
    }

    /// One sealed entry for a document (the port of the v1 outbox, per feed).
    fn doc_entry(
        &mut self,
        feed: FeedId,
        row: &kb_storage::DocRow,
        account_id: kb_types::AccountId,
        device_id: kb_types::DeviceId,
        now: i64,
    ) -> Result<Option<OutboundEntry>, ApiError> {
        let mut state = self.doc_state_in(feed, row.doc_id)?;
        // A log that no longer starts where the feed left off (compacted before its first push, or
        // after a reset) cannot be replayed from its tail: the entry carries the whole document as one
        // snapshot blob instead.
        let stats = self
            .db
            .update_stats(row.doc_id)
            .map_err(map_storage_error)?;
        let compacted = self
            .db
            .snapshot_get(row.doc_id)
            .map_err(map_storage_error)?
            .is_some();
        let first_seq = stats
            .last_seq
            .map(|last| last - i64::try_from(stats.count).unwrap_or(0) + 1);
        let needs_full = compacted
            && (stats.count == 0
                || first_seq.is_some_and(|first| state.pushed_up_to_seq < first - 1));
        let (updates, last_seq): (Vec<Vec<u8>>, i64) = if needs_full {
            let blob = self.full_snapshot_blob(row)?;
            let last_seq = match stats.last_seq {
                Some(last) => last,
                None => self
                    .db
                    .update_append(row.doc_id, &blob, UpdateOrigin::Core, now)
                    .map_err(map_storage_error)?,
            };
            (vec![blob], last_seq)
        } else {
            let all = self
                .db
                .updates_from(row.doc_id, state.pushed_up_to_seq)
                .map_err(map_storage_error)?;
            let Some((last_seq, _, _)) = all.last() else {
                return Ok(None);
            };
            let updates: Vec<Vec<u8>> = all
                .iter()
                .filter(|(_, _, origin)| *origin != UpdateOrigin::Remote)
                .map(|(_, blob, _)| blob.clone())
                .collect();
            if updates.is_empty() {
                state.pushed_up_to_seq = *last_seq;
                self.db
                    .sync_doc_state_put_in(feed, &state)
                    .map_err(map_storage_error)?;
                return Ok(None);
            }
            (updates, *last_seq)
        };
        let Some((key, key_epoch)) = self.current_doc_key(row)? else {
            tracing::debug!(doc = %short_id(&row.doc_id), "no document key yet; entry deferred");
            return Ok(None);
        };
        let version_vector = self
            .open_doc(row)
            .map(|d| d.version().0)
            .unwrap_or_default();
        let header = EntryHeader {
            feed,
            account_id,
            doc_id: row.doc_id,
            device_id,
            device_seq: state.next_device_seq,
            key_epoch,
        };
        let body = EntryBody {
            engine: engine_of(row.kind),
            format_version: kb_doc::FORMAT_VERSION,
            min_reader_version: kb_doc::MIN_READER_VERSION,
            doc_kind: code_of(row.kind),
            updates,
            version_vector,
        };
        let bytes = seal_entry(&header, &body, &key, self.signer(), self.rng.as_ref());
        Ok(Some(OutboundEntry {
            doc_id: row.doc_id,
            device_seq: state.next_device_seq,
            bytes,
            up_to_seq: last_seq,
        }))
    }

    /// The server took an entry: its feed cursor and the local update sequence it covered.
    pub fn sync_feed_pushed(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        device_seq: u64,
        cursor: u64,
        up_to_seq: i64,
    ) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let mut state = self.doc_state_in(feed, doc_id)?;
        if device_seq >= state.next_device_seq {
            state.next_device_seq = device_seq + 1;
        }
        if doc_id == KEYRING_DOC_ID {
            let rows = self
                .shared
                .keyring_in_flight
                .remove(&feed)
                .unwrap_or_default();
            self.db
                .keyring_mark_published(&rows)
                .map_err(map_storage_error)?;
            if self.feed_row(feed)?.kind == FeedKind::Shared {
                self.note_records_pushed(feed, &rows, cursor)?;
            }
        } else {
            state.pushed_up_to_seq = state.pushed_up_to_seq.max(up_to_seq);
            state.entries_since_snapshot += 1;
            // The comment records of the entry now have the cursor snapshot dominance counts from.
            if self
                .db
                .doc_get(doc_id)
                .map_err(map_storage_error)?
                .is_some_and(|row| row.kind == DocKindCode::Comments)
            {
                self.comment_records_pushed(feed, doc_id, cursor)?;
            }
        }
        self.db
            .sync_doc_state_put_in(feed, &state)
            .map_err(map_storage_error)?;
        self.db
            .sync_config_set("last_push_at", &now.to_string())
            .map_err(map_storage_error)?;
        tracing::debug!(doc = %short_id(&doc_id), device_seq, cursor, "entry pushed");
        Ok(())
    }

    // ----- snapshots ------------------------------------------------------------------------

    /// Documents of the feed due a new remote snapshot.
    pub fn sync_feed_snapshot_candidates(&mut self, feed: FeedId) -> Result<Vec<DocId>, ApiError> {
        self.shared_load()?;
        let row = self.feed_row(feed)?;
        if row.fork {
            return Ok(Vec::new());
        }
        let gate = if row.kind == FeedKind::Shared {
            match self.push_gate(feed)? {
                Some(gate) => Some(gate),
                None => return Ok(Vec::new()),
            }
        } else {
            None
        };
        let comments_only = gate.as_ref().is_some_and(|gate| gate.comments_only);
        let Some(personal) = self.personal_feed()? else {
            return Ok(Vec::new());
        };
        let shared_map = self.shared_space_map()?;
        let states: HashMap<DocId, SyncDocState> = self
            .db
            .sync_doc_state_all_in(feed)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|s| (s.doc_id, s))
            .collect();
        // A document with parked items is incomplete here: a snapshot of it would claim to cover the
        // feed without them.
        let incomplete: std::collections::HashSet<DocId> = self
            .db
            .sync_pending_all_in(feed)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|row| row.doc_id)
            .collect();
        let mut out = Vec::new();
        for doc in self.db.docs_all().map_err(map_storage_error)? {
            if Self::feed_of_doc(&doc, &shared_map, personal) != Some(feed)
                || incomplete.contains(&doc.doc_id)
                || (comments_only && doc.kind != DocKindCode::Comments)
            {
                continue;
            }
            let state = states.get(&doc.doc_id);
            let never = state.is_none_or(|s| s.remote_snapshot_covers.is_none());
            let over = state.is_some_and(|s| {
                s.entries_since_snapshot >= SNAPSHOT_AFTER_ENTRIES
                    || s.bytes_since_snapshot >= SNAPSHOT_AFTER_BYTES
            });
            let has_content = self
                .db
                .update_stats(doc.doc_id)
                .map_err(map_storage_error)?
                .last_seq
                .is_some()
                || self
                    .db
                    .snapshot_get(doc.doc_id)
                    .map_err(map_storage_error)?
                    .is_some();
            if (never || over) && has_content {
                out.push(doc.doc_id);
            }
        }
        Ok(out)
    }

    /// Makes the sealed snapshot of a document of the feed and the cursor it covers (everything this
    /// device has applied from the feed).
    pub fn sync_feed_make_snapshot(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(Vec<u8>, u64), ApiError> {
        let row = self.feed_row(feed)?;
        let doc = self
            .db
            .doc_get(doc_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| ApiError::not_found("document not found"))?;
        let (snapshot, version) = self.document_snapshot(&doc)?;
        self.seal_snapshot_of(feed, &row, &doc, snapshot, version, row.cursor)
    }

    /// The whole document as one snapshot blob and its version vector.
    fn document_snapshot(&self, row: &kb_storage::DocRow) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
        use crate::documents::{load_doc, map_doc_error};
        if row.kind == DocKindCode::Comments {
            // A comment stream is not a Loro document: its snapshot is the set of records the fold
            // counts, and it has no version vector (dominance is record-set inclusion).
            return Ok((self.comment_stream_snapshot_of_doc(row)?, Vec::new()));
        }
        match self.open_doc(row) {
            Some(doc) => Ok((
                doc.export_snapshot().map_err(map_doc_error)?,
                doc.version().0,
            )),
            None => {
                let doc = load_doc(
                    &self.db,
                    row,
                    crate::sync::engine_kind(row.kind),
                    self.rng.as_ref(),
                )?;
                Ok((
                    doc.export_snapshot().map_err(map_doc_error)?,
                    doc.version().0,
                ))
            }
        }
    }

    /// Seals a snapshot body for a document under its current document key.
    fn seal_snapshot_of(
        &mut self,
        feed: FeedId,
        feed_row: &SyncFeedRow,
        doc: &kb_storage::DocRow,
        snapshot: Vec<u8>,
        version: Vec<u8>,
        covers: u64,
    ) -> Result<(Vec<u8>, u64), ApiError> {
        let (account_id, device_id) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let Some((key, key_epoch)) =
            self.snapshot_doc_key(doc, feed_row.kind == FeedKind::Shared)?
        else {
            return Err(ApiError::crypto("the document has no key record yet"));
        };
        let header = SnapshotHeader {
            feed,
            account_id,
            doc_id: doc.doc_id,
            device_id,
            covers_cursor: covers,
            key_epoch,
        };
        let body = SnapshotBody {
            engine: engine_of(doc.kind),
            format_version: kb_doc::FORMAT_VERSION,
            min_reader_version: kb_doc::MIN_READER_VERSION,
            doc_kind: code_of(doc.kind),
            snapshot,
            version_vector: version,
        };
        Ok((
            seal_snapshot(&header, &body, &key, self.signer(), self.rng.as_ref()),
            covers,
        ))
    }

    /// Seals an arbitrary snapshot body of a document as this device (a hostile or broken writer):
    /// what the adversarial tests publish.
    #[cfg(test)]
    pub(crate) fn test_seal_snapshot(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        snapshot: Vec<u8>,
        covers: u64,
    ) -> Result<Vec<u8>, ApiError> {
        let row = self.feed_row(feed)?;
        let doc = self
            .db
            .doc_get(doc_id)
            .map_err(map_storage_error)?
            .ok_or_else(|| ApiError::not_found("document not found"))?;
        let (_, version) = self.document_snapshot(&doc)?;
        Ok(self
            .seal_snapshot_of(feed, &row, &doc, snapshot, version, covers)?
            .0)
    }

    /// The server took the snapshot of the document covering `covers`.
    pub fn sync_feed_snapshot_published(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        covers: u64,
    ) -> Result<(), ApiError> {
        let mut row = self.feed_row(feed)?;
        if covers > row.max_covers {
            row.max_covers = covers;
            self.feed_row_put(&row)?;
        }
        let mut state = self.doc_state_in(feed, doc_id)?;
        state.remote_snapshot_covers = Some(covers);
        state.snapshot_covers = Some(covers);
        state.entries_since_snapshot = 0;
        state.bytes_since_snapshot = 0;
        self.db
            .sync_doc_state_put_in(feed, &state)
            .map_err(map_storage_error)
    }

    /// Marks a document of a feed for a snapshot at the next cycle (a republication after a dispute,
    /// a document whose key changed): its remote snapshot counts as unknown.
    pub(crate) fn feed_mark_for_snapshot(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(), ApiError> {
        let mut state = self.doc_state_in(feed, doc_id)?;
        state.remote_snapshot_covers = None;
        state.entries_since_snapshot = state.entries_since_snapshot.max(SNAPSHOT_AFTER_ENTRIES);
        self.db
            .sync_doc_state_put_in(feed, &state)
            .map_err(map_storage_error)
    }

    // ----- the marks of other devices' snapshots --------------------------------------------

    /// Whether this device verifies snapshots in the feed (an Editor-and-up device of a shared feed
    /// holding documents), and if so the mark `after` to ask `snapshot_marks` from.
    pub fn sync_snapshot_marks_wanted(&mut self, feed: FeedId) -> Result<Option<u64>, ApiError> {
        self.shared_load()?;
        let row = self.feed_row(feed)?;
        if row.kind != FeedKind::Shared || !self.feed_is_live(feed)? {
            return Ok(None);
        }
        let closed = self
            .shared
            .timelines
            .get(&feed)
            .is_none_or(|t| t.state().closed.is_some());
        let editor = self
            .feed_role(feed)?
            .is_some_and(|role| role >= MemberRole::Editor);
        if closed || !editor {
            return Ok(None);
        }
        Ok(Some(self.fcfg_u64(feed, "marks")?.unwrap_or(0)))
    }

    /// The marks the server announced: the documents whose latest snapshot this device holds and must
    /// fetch and verify (it fetches them with `get_snapshot` and hands them to
    /// `sync_feed_apply_snapshots`). The core remembers the highest mark. A snapshot that covers more
    /// than this device has pulled yet waits until the pull is there.
    pub fn sync_snapshot_marks_received(
        &mut self,
        feed: FeedId,
        marks: &[SnapshotMark],
    ) -> Result<Vec<DocId>, ApiError> {
        let row = self.feed_row(feed)?;
        let (_, my_device) = self
            .sync_identity()?
            .ok_or_else(|| ApiError::invalid("kit", "create your recovery kit before syncing"))?;
        let space_id = row.space_id;
        let mut deferred = self.snapshot_list(feed, "deferred")?;
        let mut highest = self.fcfg_u64(feed, "marks")?.unwrap_or(0);
        for mark in marks {
            highest = highest.max(mark.mark);
            if mark.device_id == my_device {
                continue;
            }
            let Some(doc) = self.db.doc_get(mark.doc_id).map_err(map_storage_error)? else {
                continue;
            };
            if doc.space_id != space_id {
                continue;
            }
            let state = self.doc_state_in(feed, mark.doc_id)?;
            if state
                .remote_snapshot_covers
                .is_some_and(|covers| covers >= mark.covers_cursor)
            {
                continue;
            }
            deferred.retain(|(doc_id, _)| *doc_id != mark.doc_id);
            deferred.push((mark.doc_id, mark.covers_cursor));
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = deferred
            .into_iter()
            .partition(|(_, covers)| *covers <= row.cursor);
        self.snapshot_list_set(feed, "deferred", &waiting)?;
        let mut unverified = self.snapshot_list(feed, "unverified")?;
        for (doc_id, covers) in &ready {
            unverified.retain(|(doc, _)| doc != doc_id);
            unverified.push((*doc_id, *covers));
        }
        self.snapshot_list_set(feed, "unverified", &unverified)?;
        self.fcfg_set(feed, "marks", &highest.to_string())?;
        Ok(ready.into_iter().map(|(doc_id, _)| doc_id).collect())
    }

    /// The driver finished checking a document's snapshot: `ok` is `false` when it posted a dispute
    /// (the core republishes a full snapshot and tells the Owners: `SnapshotDisputed`), `true` when the
    /// snapshot was honest (the core may acknowledge past its `covers`).
    pub fn sync_snapshot_verified(
        &mut self,
        feed: FeedId,
        doc_id: DocId,
        ok: bool,
    ) -> Result<(), ApiError> {
        let mut unverified = self.snapshot_list(feed, "unverified")?;
        unverified.retain(|(doc, _)| *doc != doc_id);
        self.snapshot_list_set(feed, "unverified", &unverified)?;
        if !ok {
            self.feed_mark_for_snapshot(feed, doc_id)?;
            self.sync_note_local_change();
        }
        Ok(())
    }

    /// The lowest `covers` of a snapshot this device was told of and has not verified yet.
    pub(crate) fn unverified_floor(&self, feed: FeedId) -> Result<Option<u64>, ApiError> {
        Ok(self
            .snapshot_list(feed, "unverified")?
            .into_iter()
            .map(|(_, covers)| covers)
            .min())
    }

    /// A small persisted list of `(document, covers)` pairs.
    fn snapshot_list(&self, feed: FeedId, name: &str) -> Result<Vec<(DocId, u64)>, ApiError> {
        Ok(self
            .fcfg(feed, name)?
            .unwrap_or_default()
            .split(';')
            .filter_map(|item| {
                let (doc, covers) = item.split_once(':')?;
                let doc: [u8; 16] = crate::security::decode_hex(doc)?.try_into().ok()?;
                Some((DocId::from_bytes(doc), covers.parse().ok()?))
            })
            .collect())
    }

    fn snapshot_list_set(
        &self,
        feed: FeedId,
        name: &str,
        list: &[(DocId, u64)],
    ) -> Result<(), ApiError> {
        if list.is_empty() {
            return self.fcfg_del(feed, name);
        }
        let encoded: Vec<String> = list
            .iter()
            .map(|(doc, covers)| format!("{}:{covers}", crate::security::hex(doc.as_bytes())))
            .collect();
        self.fcfg_set(feed, name, &encoded.join(";"))
    }
}

impl Core {
    /// The keyring rows that are `Doc` records of comment streams (documents this device holds as
    /// such): what a Commenter's device may send.
    fn keep_comment_stream_records(
        &self,
        rows: Vec<kb_storage::KeyringRow>,
    ) -> Result<Vec<kb_storage::KeyringRow>, ApiError> {
        let mut keep = Vec::with_capacity(rows.len());
        for row in rows {
            let KeyringItem::Record(record) =
                KeyringItem::decode(&row.record).map_err(map_keys_error)?
            else {
                continue;
            };
            let kb_keys::KeySubject::Doc { doc_id, .. } = record.subject else {
                continue;
            };
            let is_stream = self
                .db
                .doc_get(doc_id)
                .map_err(map_storage_error)?
                .is_some_and(|doc| doc.kind == DocKindCode::Comments);
            if is_stream {
                keep.push(row);
            }
        }
        Ok(keep)
    }

    /// Takes out of a shared feed's keyring rows the records made before the feed's first epoch
    /// (they are wrapped under keys the members never get) and marks them sent, so they neither
    /// travel nor wait.
    fn drop_pre_share_records(
        &mut self,
        feed: FeedId,
        rows: Vec<kb_storage::KeyringRow>,
    ) -> Result<Vec<kb_storage::KeyringRow>, ApiError> {
        let Some(first) = self
            .shared
            .metas
            .get(&feed)
            .and_then(|meta| meta.commitments.keys().next().copied())
        else {
            return Ok(rows);
        };
        let mut keep = Vec::with_capacity(rows.len());
        let mut old = Vec::new();
        for row in rows {
            let before = match KeyringItem::decode(&row.record).map_err(map_keys_error)? {
                KeyringItem::Record(record) => match record.subject {
                    kb_keys::KeySubject::Doc { epoch, .. } => epoch < first,
                    _ => record.wrap_epoch < first,
                },
                KeyringItem::Tombstone(_) => false,
            };
            if before {
                old.push(row);
            } else {
                keep.push(row);
            }
        }
        if !old.is_empty() {
            self.db
                .keyring_mark_published(&old)
                .map_err(map_storage_error)?;
        }
        Ok(keep)
    }
}

// ----- the records this device sent and the pull has not passed yet ------------------------------

impl Core {
    /// Notes the key records of an entry the server took at `cursor`: until the pull passes that
    /// cursor they are not "in the feed" from this device's point of view, so a record of another
    /// device for the same subject that sits earlier in the feed displaces them (`first in feed
    /// order`, ADR-0044).
    fn note_records_pushed(
        &self,
        feed: FeedId,
        rows: &[kb_storage::KeyringRow],
        cursor: u64,
    ) -> Result<(), ApiError> {
        let mut list = self.unconfirmed_records(feed)?;
        for row in rows {
            if row.tombstone {
                continue;
            }
            list.push((row.space_id, row.subject.clone(), cursor));
        }
        self.unconfirmed_records_set(feed, &list)
    }

    fn unconfirmed_records(
        &self,
        feed: FeedId,
    ) -> Result<Vec<(kb_types::SpaceId, Vec<u8>, u64)>, ApiError> {
        Ok(self
            .fcfg(feed, "unconfirmed")?
            .unwrap_or_default()
            .split(';')
            .filter_map(|item| {
                let mut parts = item.split(':');
                let space: [u8; 16] = crate::security::decode_hex(parts.next()?)?
                    .try_into()
                    .ok()?;
                let subject = crate::security::decode_hex(parts.next()?)?;
                let cursor: u64 = parts.next()?.parse().ok()?;
                Some((kb_types::SpaceId::from_bytes(space), subject, cursor))
            })
            .collect())
    }

    fn unconfirmed_records_set(
        &self,
        feed: FeedId,
        list: &[(kb_types::SpaceId, Vec<u8>, u64)],
    ) -> Result<(), ApiError> {
        if list.is_empty() {
            return self.fcfg_del(feed, "unconfirmed");
        }
        let encoded: Vec<String> = list
            .iter()
            .map(|(space, subject, cursor)| {
                format!(
                    "{}:{}:{cursor}",
                    crate::security::hex(space.as_bytes()),
                    crate::security::hex(subject)
                )
            })
            .collect();
        self.fcfg_set(feed, "unconfirmed", &encoded.join(";"))
    }

    /// Whether a record this device sent for the subject is in an entry of the feed that comes after
    /// `cursor` (the pull has not passed it yet).
    pub(crate) fn own_record_pushed_after(
        &self,
        feed: FeedId,
        space_id: kb_types::SpaceId,
        subject: &[u8],
        cursor: u64,
    ) -> Result<bool, ApiError> {
        Ok(self
            .unconfirmed_records(feed)?
            .iter()
            .any(|(space, other, at)| *space == space_id && other == subject && *at > cursor))
    }

    /// The pull passed `cursor`: the records sent up to it are in the feed.
    pub(crate) fn confirm_records_up_to(&self, feed: FeedId, cursor: u64) -> Result<(), ApiError> {
        let list = self.unconfirmed_records(feed)?;
        if list.is_empty() {
            return Ok(());
        }
        let kept: Vec<_> = list.into_iter().filter(|(_, _, at)| *at > cursor).collect();
        self.unconfirmed_records_set(feed, &kept)
    }

    /// The key a snapshot of a document of a shared feed is sealed under: the newest record the
    /// document has, whatever epoch the feed is at (a snapshot is not epoch-gated, and a device that
    /// did not write the document makes no record of the new epoch for it: that is the writers'
    /// business). A document without any record gets the current one.
    fn snapshot_doc_key(
        &mut self,
        row: &kb_storage::DocRow,
        shared: bool,
    ) -> Result<Option<(kb_crypto::Secret32, u32)>, ApiError> {
        if !shared || row.kind == DocKindCode::Account {
            return self.current_doc_key(row);
        }
        let Some(space_id) = row.space_id else {
            return Ok(None);
        };
        let mut epochs: Vec<u32> = self
            .security
            .keyring
            .doc_records(row.doc_id)
            .iter()
            .filter(|record| record.space_id == space_id)
            .filter_map(|record| match record.subject {
                kb_keys::KeySubject::Doc { epoch, .. } => Some(epoch),
                _ => None,
            })
            .collect();
        epochs.sort_unstable_by(|a, b| b.cmp(a));
        for epoch in epochs {
            if let Some(key) = self.doc_key_in(space_id, row.doc_id, epoch)? {
                return Ok(Some((key, epoch)));
            }
        }
        self.current_doc_key(row)
    }
}
