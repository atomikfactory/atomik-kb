//! The sync state (schema v6, `docs/development/milestone-06.md`, "Storage"; ADR-0021; per feed
//! since schema v17, ADR-0045). Local bookkeeping of this device's view of the server: the client
//! settings, the per-document push and snapshot state, the continuity table, the signed heads of the
//! other devices, the entries that wait for a key or a structure document, the `published` markers
//! of the keyring and the device log, and the outbox read over `doc_updates`. Blobs (signed heads,
//! pending entries, keyring records) are opaque here: `kb-sync` and `kb-keys` encode and verify
//! them.
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`] through
//! [`SyncQueries`], both running the same private functions over a connection (a
//! transaction derefs to one), so a method behaves identically in either place. The
//! methods that take more than one statement run as one atomic unit on a bare [`Db`].
//! Rows are read as raw column values first and validated afterwards, so a damaged row is
//! a typed `InvalidData` error and never a panic or a silently wrong value. Blob contents
//! and configuration values never appear in an error.
//!
//! # The tables
//!
//! - `sync_config`: text pairs (server URL, epoch, cursor, ...). The keys belong to the
//!   core; this crate stores them. Not per feed.
//! - `sync_doc_state`: one row per `(feed, document)` the sync has touched, **including the
//!   keyring**, whose feed document id is the all-zero id (`kb_types::sync::KEYRING_DOC_ID`)
//!   and is not a row of `docs` (it exists once per feed). So there are no foreign keys in the v6
//!   tables: deleting a document does not delete its sync row, and a pending entry may name a
//!   document this device does not have yet.
//! - `sync_seen`: the last `device_seq` applied per `(feed, document, device)`.
//! - `sync_heads`: the latest signed head of each `(feed, device)`.
//! - `sync_pending`: received entries that cannot be applied yet, by feed, feed cursor and
//!   document (a feed cursor is unique; parked snapshots share the cursor they cover).
//!
//! # Feeds
//!
//! Since schema v17 a device syncs several feeds (its personal one and one per shared space,
//! ADR-0045), and the four tables above carry the `feed`. Every query has a form that names the
//! feed (`..._in`: [`SyncQueries::sync_doc_state_get_in`] and the others). The forms without a feed
//! are those of schema v6 and work on the **personal feed**, whose id is the account id (read from
//! `meta`, [`SyncQueries::sync_personal_feed`]; [`UNRESOLVED_PERSONAL_FEED`] while there is no
//! account): their behaviour for the personal feed is exactly what it was, so a caller that does not
//! know about shared spaces works unchanged. The two exceptions are the resets of a new server
//! epoch, [`SyncQueries::sync_doc_state_reset_all`] and the `..._clear` methods without a feed: the
//! server is the same for every feed, so they reset **all** feeds. A feed that is forgotten takes
//! its rows with it ([`crate::FeedQueries::sync_feed_delete`]).
//!
//! # Numbers
//!
//! Cursors, device sequences and snapshot coverage are `u64` in the API and 63-bit
//! integers in SQLite. A value above `i64::MAX` is refused (`InvalidData`) and nothing is
//! written; a negative stored value is `InvalidData` when read.
//!
//! # `published`
//!
//! `keyring` and `device_log` rows carry `published` (0 = not sent to the server yet, 1 =
//! sent). Existing rows migrate to 0, a row written by [`crate::SecurityQueries`]
//! (`keyring_put`, `device_log_append`) starts at 0, and **replacing** a keyring row with
//! `keyring_put` makes it 0 again, which is what a changed record needs. A row that a
//! device received from the server is stored and then marked published in the same
//! transaction, so it does not echo back. Since schema v17 a keyring row's destination feed follows
//! its space (the `Space` records and the device log go to the personal feed, the other records of a
//! shared space to that space's feed): the marker means "sent to that feed". The rows are
//! unchanged, the core decides the destination of each.

use kb_types::{AccountId, DeviceId, DocId, FeedId, SpaceId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::docs::{atomically, id_from};
use crate::{Db, DeviceLogRow, KeyringRow, StorageError, Tx, UpdateOrigin};

/// The `meta` key the core stores the account id under, as a UUID string.
const META_ACCOUNT_ID: &str = "account_id";

/// The personal feed of a database that has no account id yet: the all-zero id, which no feed has.
/// Rows written while there is no account (and the rows a migration finds without one) belong to
/// it, and no feed of a real account finds them.
pub const UNRESOLVED_PERSONAL_FEED: FeedId = FeedId::from_bytes([0; 16]);

/// A `sync_doc_state` row: what this device knows about the sync of one document (or of
/// the keyring, under the all-zero document id) in one feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncDocState {
    /// The document; the all-zero id for the keyring.
    pub doc_id: DocId,
    /// The highest `doc_updates.local_seq` of the document that is in the feed (0: none).
    pub pushed_up_to_seq: i64,
    /// The `device_seq` of this device's next entry for the document (1 for the first).
    pub next_device_seq: u64,
    /// What the snapshot this device last made covers (a feed cursor), if it made one.
    pub snapshot_covers: Option<u64>,
    /// Entries pushed since the last snapshot of the document.
    pub entries_since_snapshot: u64,
    /// Bytes pushed since the last snapshot of the document.
    pub bytes_since_snapshot: u64,
    /// What the latest snapshot **on the server** covers (a feed cursor); `None` until one
    /// is known.
    pub remote_snapshot_covers: Option<u64>,
}

/// A `sync_heads` row: the latest signed head seen for one device in one feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncHeadRow {
    /// The device that signed it.
    pub device_id: DeviceId,
    /// The feed cursor the head states.
    pub cursor: u64,
    /// The chain hash at that cursor.
    pub chain_hash: [u8; 32],
    /// The time the device signed it, Unix ms.
    pub at_ms: i64,
    /// The signed head exactly as received.
    pub signed: Vec<u8>,
    /// When this device received it, Unix ms.
    pub seen_at: i64,
}

/// A `sync_pending` row: an entry received and verified but not applicable yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPendingRow {
    /// The entry's feed cursor, or the cursor a parked snapshot covers.
    pub cursor: u64,
    /// The document it belongs to.
    pub doc_id: DocId,
    /// The entry exactly as received.
    pub bytes: Vec<u8>,
    /// Why it waits: 1 = the document key is missing, 2 = the structure document is
    /// missing, 3 = a snapshot whose key is missing, 4 = the document's history is
    /// missing. This crate stores any value from 0 to 255; the core defines the meaning.
    pub reason: u8,
    /// When this device received it, Unix ms.
    pub received_at: i64,
}

/// Queries of the sync tables, the `published` markers and the outbox, available on a
/// connection or inside a transaction. On a [`Db`] every method is atomic by itself.
///
/// The methods without `_in` work on the personal feed (see the module documentation); those with
/// `_in` take the feed.
pub trait SyncQueries {
    // ---- sync_config ----

    /// One value, `None` for an unknown key.
    fn sync_config_get(&self, key: &str) -> Result<Option<String>, StorageError>;
    /// Writes a value (upsert).
    fn sync_config_set(&self, key: &str, value: &str) -> Result<(), StorageError>;
    /// Removes a key. Succeeds when the key is unknown.
    fn sync_config_delete(&self, key: &str) -> Result<(), StorageError>;
    /// Every pair, ordered by key.
    fn sync_config_all(&self) -> Result<Vec<(String, String)>, StorageError>;

    // ---- the personal feed ----

    /// The personal feed of this database: the account id (`meta` key `account_id`) as a feed id,
    /// or [`UNRESOLVED_PERSONAL_FEED`] while there is no account. A stored account id that is not an
    /// id is `InvalidData`.
    fn sync_personal_feed(&self) -> Result<FeedId, StorageError>;

    // ---- sync_doc_state ----

    /// One row of the personal feed, `None` for a document the sync has not touched.
    fn sync_doc_state_get(&self, doc_id: DocId) -> Result<Option<SyncDocState>, StorageError> {
        self.sync_doc_state_get_in(self.sync_personal_feed()?, doc_id)
    }
    /// One row of `feed`, `None` for a document the sync has not touched in it.
    fn sync_doc_state_get_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<Option<SyncDocState>, StorageError>;
    /// Inserts the row or replaces every column of the one with the same document id (an
    /// upsert), in the personal feed. Nothing is merged: `next_device_seq` is whatever the caller
    /// writes. A number above `i64::MAX` is `InvalidData` and nothing is written.
    fn sync_doc_state_put(&self, row: &SyncDocState) -> Result<(), StorageError> {
        self.sync_doc_state_put_in(self.sync_personal_feed()?, row)
    }
    /// [`Self::sync_doc_state_put`] in `feed`.
    fn sync_doc_state_put_in(&self, feed: FeedId, row: &SyncDocState) -> Result<(), StorageError>;
    /// Every row of the personal feed, ordered by document id.
    fn sync_doc_state_all(&self) -> Result<Vec<SyncDocState>, StorageError> {
        self.sync_doc_state_all_in(self.sync_personal_feed()?)
    }
    /// Every row of `feed`, ordered by document id.
    fn sync_doc_state_all_in(&self, feed: FeedId) -> Result<Vec<SyncDocState>, StorageError>;
    /// Forgets what is known about the server, for a new server epoch, **in every feed**: in every
    /// row `pushed_up_to_seq` becomes 0, `remote_snapshot_covers` NULL and the two counters
    /// since the last snapshot 0. `next_device_seq` (the device's sequence is its own, and
    /// the continuity of its entries must not restart) and `snapshot_covers` stay.
    fn sync_doc_state_reset_all(&self) -> Result<(), StorageError>;
    /// [`Self::sync_doc_state_reset_all`] for the rows of one feed (a shared feed that was lost).
    fn sync_doc_state_reset_in(&self, feed: FeedId) -> Result<(), StorageError>;

    // ---- sync_seen ----

    /// The last `device_seq` applied for `(doc_id, device_id)` in the personal feed, `None` when
    /// none was.
    fn sync_seen_get(
        &self,
        doc_id: DocId,
        device_id: DeviceId,
    ) -> Result<Option<u64>, StorageError> {
        self.sync_seen_get_in(self.sync_personal_feed()?, doc_id, device_id)
    }
    /// The last `device_seq` applied for `(doc_id, device_id)` in `feed`.
    fn sync_seen_get_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
    ) -> Result<Option<u64>, StorageError>;
    /// Writes the last applied `device_seq` in the personal feed (upsert; it is stored as given,
    /// also when lower than the stored one).
    fn sync_seen_put(
        &self,
        doc_id: DocId,
        device_id: DeviceId,
        last_seq: u64,
    ) -> Result<(), StorageError> {
        self.sync_seen_put_in(self.sync_personal_feed()?, doc_id, device_id, last_seq)
    }
    /// [`Self::sync_seen_put`] in `feed`.
    fn sync_seen_put_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
        last_seq: u64,
    ) -> Result<(), StorageError>;
    /// Removes every row **of every feed** (a new server epoch).
    fn sync_seen_clear(&self) -> Result<(), StorageError>;
    /// Removes the rows of one feed.
    fn sync_seen_clear_in(&self, feed: FeedId) -> Result<(), StorageError>;

    // ---- sync_heads ----

    /// Stores the head of a device in the personal feed, replacing the one it had (one row per
    /// device and feed).
    fn sync_heads_put(&self, row: &SyncHeadRow) -> Result<(), StorageError> {
        self.sync_heads_put_in(self.sync_personal_feed()?, row)
    }
    /// [`Self::sync_heads_put`] in `feed`.
    fn sync_heads_put_in(&self, feed: FeedId, row: &SyncHeadRow) -> Result<(), StorageError>;
    /// Every head of the personal feed, ordered by device id.
    fn sync_heads_all(&self) -> Result<Vec<SyncHeadRow>, StorageError> {
        self.sync_heads_all_in(self.sync_personal_feed()?)
    }
    /// Every head of `feed`, ordered by device id.
    fn sync_heads_all_in(&self, feed: FeedId) -> Result<Vec<SyncHeadRow>, StorageError>;
    /// Removes the head of one device in the personal feed. Succeeds when there is none.
    fn sync_heads_delete(&self, device_id: DeviceId) -> Result<(), StorageError> {
        self.sync_heads_delete_in(self.sync_personal_feed()?, device_id)
    }
    /// Removes the head of one device in `feed`. Succeeds when there is none.
    fn sync_heads_delete_in(&self, feed: FeedId, device_id: DeviceId) -> Result<(), StorageError>;
    /// Removes every head **of every feed** (a new server epoch).
    fn sync_heads_clear(&self) -> Result<(), StorageError>;
    /// Removes the heads of one feed.
    fn sync_heads_clear_in(&self, feed: FeedId) -> Result<(), StorageError>;

    // ---- sync_pending ----

    /// Stores a pending entry in the personal feed; one with the same cursor and document is
    /// replaced (a feed cursor is unique; parked snapshots share the cursor they cover).
    fn sync_pending_put(&self, row: &SyncPendingRow) -> Result<(), StorageError> {
        self.sync_pending_put_in(self.sync_personal_feed()?, row)
    }
    /// [`Self::sync_pending_put`] in `feed`.
    fn sync_pending_put_in(&self, feed: FeedId, row: &SyncPendingRow) -> Result<(), StorageError>;
    /// Returns **and deletes** every pending entry of a document in the personal feed, in cursor
    /// order. The read and the delete are one atomic unit: if a row cannot be read nothing is
    /// deleted. Empty for a document with none.
    fn sync_pending_take_for_doc(
        &self,
        doc_id: DocId,
    ) -> Result<Vec<SyncPendingRow>, StorageError> {
        self.sync_pending_take_for_doc_in(self.sync_personal_feed()?, doc_id)
    }
    /// [`Self::sync_pending_take_for_doc`] in `feed`.
    fn sync_pending_take_for_doc_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<Vec<SyncPendingRow>, StorageError>;
    /// Every pending entry of the personal feed, in cursor order.
    fn sync_pending_all(&self) -> Result<Vec<SyncPendingRow>, StorageError> {
        self.sync_pending_all_in(self.sync_personal_feed()?)
    }
    /// Every pending entry of `feed`, in cursor order.
    fn sync_pending_all_in(&self, feed: FeedId) -> Result<Vec<SyncPendingRow>, StorageError>;
    /// Removes one pending entry of the personal feed by cursor and document. Succeeds when there
    /// is none.
    fn sync_pending_delete(&self, cursor: u64, doc_id: DocId) -> Result<(), StorageError> {
        self.sync_pending_delete_in(self.sync_personal_feed()?, cursor, doc_id)
    }
    /// [`Self::sync_pending_delete`] in `feed`.
    fn sync_pending_delete_in(
        &self,
        feed: FeedId,
        cursor: u64,
        doc_id: DocId,
    ) -> Result<(), StorageError>;
    /// Removes every pending entry **of every feed** (a new server epoch).
    fn sync_pending_clear(&self) -> Result<(), StorageError>;
    /// Removes the pending entries of one feed.
    fn sync_pending_clear_in(&self, feed: FeedId) -> Result<(), StorageError>;
    /// Number of pending entries of the personal feed.
    fn sync_pending_count(&self) -> Result<u32, StorageError> {
        self.sync_pending_count_in(self.sync_personal_feed()?)
    }
    /// Number of pending entries of `feed`.
    fn sync_pending_count_in(&self, feed: FeedId) -> Result<u32, StorageError>;

    // ---- published markers ----

    /// The keyring rows not sent to the server yet, ordered like
    /// [`crate::SecurityQueries::keyring_all`] (space, then subject bytes).
    fn keyring_unpublished(&self) -> Result<Vec<KeyringRow>, StorageError>;
    /// Marks keyring rows as sent. A row is identified by its **content**, not only by its
    /// key: `(space_id, subject)` **and** `record` **and** `tombstone` must all still match.
    /// A row that was replaced after the caller read it (a tombstone over a record, say)
    /// is therefore left unpublished, so a push that is in flight while the user edits
    /// cannot swallow the newer row. Rows that are unknown, or already published, are
    /// ignored. All rows or none on a bare [`Db`].
    fn keyring_mark_published(&self, rows: &[KeyringRow]) -> Result<(), StorageError>;
    /// Marks every keyring row as not sent (a new server epoch).
    fn keyring_mark_all_unpublished(&self) -> Result<(), StorageError>;
    /// The device log entries not sent to the server yet, in sequence order (always a
    /// suffix of the log: marking is by prefix and appends start unpublished).
    fn device_log_unpublished(&self) -> Result<Vec<DeviceLogRow>, StorageError>;
    /// Marks every entry with `seq <= up_to_seq` as sent. A number above `i64::MAX` is
    /// `InvalidData`.
    fn device_log_mark_published(&self, up_to_seq: u64) -> Result<(), StorageError>;
    /// Marks every log entry as not sent (a new server epoch).
    fn device_log_mark_all_unpublished(&self) -> Result<(), StorageError>;

    // ---- the outbox ----

    /// The update log of a document after `after_seq` (**exclusive**): `(local_seq, blob,
    /// origin)` in `local_seq` order, whatever the origin (the caller filters
    /// [`UpdateOrigin::Remote`] out). Empty for an unknown document. Compaction deletes
    /// the rows a snapshot covers (`updates_delete_up_to`), so this reads what is left of
    /// the log, not the whole history.
    fn updates_from(
        &self,
        doc_id: DocId,
        after_seq: i64,
    ) -> Result<Vec<(i64, Vec<u8>, UpdateOrigin)>, StorageError>;
}

impl SyncQueries for Db {
    fn sync_config_get(&self, key: &str) -> Result<Option<String>, StorageError> {
        sync_config_get_on(&self.conn, key)
    }
    fn sync_config_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        sync_config_set_on(&self.conn, key, value)
    }
    fn sync_config_delete(&self, key: &str) -> Result<(), StorageError> {
        sync_config_delete_on(&self.conn, key)
    }
    fn sync_config_all(&self) -> Result<Vec<(String, String)>, StorageError> {
        sync_config_all_on(&self.conn)
    }
    fn sync_personal_feed(&self) -> Result<FeedId, StorageError> {
        personal_feed_on(&self.conn)
    }
    fn sync_doc_state_get_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<Option<SyncDocState>, StorageError> {
        sync_doc_state_get_on(&self.conn, feed, doc_id)
    }
    fn sync_doc_state_put_in(&self, feed: FeedId, row: &SyncDocState) -> Result<(), StorageError> {
        sync_doc_state_put_on(&self.conn, feed, row)
    }
    fn sync_doc_state_all_in(&self, feed: FeedId) -> Result<Vec<SyncDocState>, StorageError> {
        sync_doc_state_all_on(&self.conn, feed)
    }
    fn sync_doc_state_reset_all(&self) -> Result<(), StorageError> {
        sync_doc_state_reset_on(&self.conn, None)
    }
    fn sync_doc_state_reset_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_doc_state_reset_on(&self.conn, Some(feed))
    }
    fn sync_seen_get_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
    ) -> Result<Option<u64>, StorageError> {
        sync_seen_get_on(&self.conn, feed, doc_id, device_id)
    }
    fn sync_seen_put_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
        last_seq: u64,
    ) -> Result<(), StorageError> {
        sync_seen_put_on(&self.conn, feed, doc_id, device_id, last_seq)
    }
    fn sync_seen_clear(&self) -> Result<(), StorageError> {
        sync_seen_clear_on(&self.conn, None)
    }
    fn sync_seen_clear_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_seen_clear_on(&self.conn, Some(feed))
    }
    fn sync_heads_put_in(&self, feed: FeedId, row: &SyncHeadRow) -> Result<(), StorageError> {
        sync_heads_put_on(&self.conn, feed, row)
    }
    fn sync_heads_all_in(&self, feed: FeedId) -> Result<Vec<SyncHeadRow>, StorageError> {
        sync_heads_all_on(&self.conn, feed)
    }
    fn sync_heads_delete_in(&self, feed: FeedId, device_id: DeviceId) -> Result<(), StorageError> {
        sync_heads_delete_on(&self.conn, feed, device_id)
    }
    fn sync_heads_clear(&self) -> Result<(), StorageError> {
        sync_heads_clear_on(&self.conn, None)
    }
    fn sync_heads_clear_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_heads_clear_on(&self.conn, Some(feed))
    }
    fn sync_pending_put_in(&self, feed: FeedId, row: &SyncPendingRow) -> Result<(), StorageError> {
        sync_pending_put_on(&self.conn, feed, row)
    }
    fn sync_pending_take_for_doc_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<Vec<SyncPendingRow>, StorageError> {
        atomically(&self.conn, |conn| {
            sync_pending_take_for_doc_on(conn, feed, doc_id)
        })
    }
    fn sync_pending_all_in(&self, feed: FeedId) -> Result<Vec<SyncPendingRow>, StorageError> {
        sync_pending_all_on(&self.conn, feed)
    }
    fn sync_pending_delete_in(
        &self,
        feed: FeedId,
        cursor: u64,
        doc_id: DocId,
    ) -> Result<(), StorageError> {
        sync_pending_delete_on(&self.conn, feed, cursor, doc_id)
    }
    fn sync_pending_clear(&self) -> Result<(), StorageError> {
        sync_pending_clear_on(&self.conn, None)
    }
    fn sync_pending_clear_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_pending_clear_on(&self.conn, Some(feed))
    }
    fn sync_pending_count_in(&self, feed: FeedId) -> Result<u32, StorageError> {
        sync_pending_count_on(&self.conn, feed)
    }
    fn keyring_unpublished(&self) -> Result<Vec<KeyringRow>, StorageError> {
        keyring_unpublished_on(&self.conn)
    }
    fn keyring_mark_published(&self, rows: &[KeyringRow]) -> Result<(), StorageError> {
        if rows.len() > 1 {
            atomically(&self.conn, |conn| keyring_mark_published_on(conn, rows))
        } else {
            keyring_mark_published_on(&self.conn, rows)
        }
    }
    fn keyring_mark_all_unpublished(&self) -> Result<(), StorageError> {
        keyring_mark_all_unpublished_on(&self.conn)
    }
    fn device_log_unpublished(&self) -> Result<Vec<DeviceLogRow>, StorageError> {
        device_log_unpublished_on(&self.conn)
    }
    fn device_log_mark_published(&self, up_to_seq: u64) -> Result<(), StorageError> {
        device_log_mark_published_on(&self.conn, up_to_seq)
    }
    fn device_log_mark_all_unpublished(&self) -> Result<(), StorageError> {
        device_log_mark_all_unpublished_on(&self.conn)
    }
    fn updates_from(
        &self,
        doc_id: DocId,
        after_seq: i64,
    ) -> Result<Vec<(i64, Vec<u8>, UpdateOrigin)>, StorageError> {
        updates_from_on(&self.conn, doc_id, after_seq)
    }
}

impl SyncQueries for Tx<'_> {
    fn sync_config_get(&self, key: &str) -> Result<Option<String>, StorageError> {
        sync_config_get_on(&self.tx, key)
    }
    fn sync_config_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        sync_config_set_on(&self.tx, key, value)
    }
    fn sync_config_delete(&self, key: &str) -> Result<(), StorageError> {
        sync_config_delete_on(&self.tx, key)
    }
    fn sync_config_all(&self) -> Result<Vec<(String, String)>, StorageError> {
        sync_config_all_on(&self.tx)
    }
    fn sync_personal_feed(&self) -> Result<FeedId, StorageError> {
        personal_feed_on(&self.tx)
    }
    fn sync_doc_state_get_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<Option<SyncDocState>, StorageError> {
        sync_doc_state_get_on(&self.tx, feed, doc_id)
    }
    fn sync_doc_state_put_in(&self, feed: FeedId, row: &SyncDocState) -> Result<(), StorageError> {
        sync_doc_state_put_on(&self.tx, feed, row)
    }
    fn sync_doc_state_all_in(&self, feed: FeedId) -> Result<Vec<SyncDocState>, StorageError> {
        sync_doc_state_all_on(&self.tx, feed)
    }
    fn sync_doc_state_reset_all(&self) -> Result<(), StorageError> {
        sync_doc_state_reset_on(&self.tx, None)
    }
    fn sync_doc_state_reset_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_doc_state_reset_on(&self.tx, Some(feed))
    }
    fn sync_seen_get_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
    ) -> Result<Option<u64>, StorageError> {
        sync_seen_get_on(&self.tx, feed, doc_id, device_id)
    }
    fn sync_seen_put_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_id: DeviceId,
        last_seq: u64,
    ) -> Result<(), StorageError> {
        sync_seen_put_on(&self.tx, feed, doc_id, device_id, last_seq)
    }
    fn sync_seen_clear(&self) -> Result<(), StorageError> {
        sync_seen_clear_on(&self.tx, None)
    }
    fn sync_seen_clear_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_seen_clear_on(&self.tx, Some(feed))
    }
    fn sync_heads_put_in(&self, feed: FeedId, row: &SyncHeadRow) -> Result<(), StorageError> {
        sync_heads_put_on(&self.tx, feed, row)
    }
    fn sync_heads_all_in(&self, feed: FeedId) -> Result<Vec<SyncHeadRow>, StorageError> {
        sync_heads_all_on(&self.tx, feed)
    }
    fn sync_heads_delete_in(&self, feed: FeedId, device_id: DeviceId) -> Result<(), StorageError> {
        sync_heads_delete_on(&self.tx, feed, device_id)
    }
    fn sync_heads_clear(&self) -> Result<(), StorageError> {
        sync_heads_clear_on(&self.tx, None)
    }
    fn sync_heads_clear_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_heads_clear_on(&self.tx, Some(feed))
    }
    fn sync_pending_put_in(&self, feed: FeedId, row: &SyncPendingRow) -> Result<(), StorageError> {
        sync_pending_put_on(&self.tx, feed, row)
    }
    fn sync_pending_take_for_doc_in(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<Vec<SyncPendingRow>, StorageError> {
        sync_pending_take_for_doc_on(&self.tx, feed, doc_id)
    }
    fn sync_pending_all_in(&self, feed: FeedId) -> Result<Vec<SyncPendingRow>, StorageError> {
        sync_pending_all_on(&self.tx, feed)
    }
    fn sync_pending_delete_in(
        &self,
        feed: FeedId,
        cursor: u64,
        doc_id: DocId,
    ) -> Result<(), StorageError> {
        sync_pending_delete_on(&self.tx, feed, cursor, doc_id)
    }
    fn sync_pending_clear(&self) -> Result<(), StorageError> {
        sync_pending_clear_on(&self.tx, None)
    }
    fn sync_pending_clear_in(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_pending_clear_on(&self.tx, Some(feed))
    }
    fn sync_pending_count_in(&self, feed: FeedId) -> Result<u32, StorageError> {
        sync_pending_count_on(&self.tx, feed)
    }
    fn keyring_unpublished(&self) -> Result<Vec<KeyringRow>, StorageError> {
        keyring_unpublished_on(&self.tx)
    }
    fn keyring_mark_published(&self, rows: &[KeyringRow]) -> Result<(), StorageError> {
        keyring_mark_published_on(&self.tx, rows)
    }
    fn keyring_mark_all_unpublished(&self) -> Result<(), StorageError> {
        keyring_mark_all_unpublished_on(&self.tx)
    }
    fn device_log_unpublished(&self) -> Result<Vec<DeviceLogRow>, StorageError> {
        device_log_unpublished_on(&self.tx)
    }
    fn device_log_mark_published(&self, up_to_seq: u64) -> Result<(), StorageError> {
        device_log_mark_published_on(&self.tx, up_to_seq)
    }
    fn device_log_mark_all_unpublished(&self) -> Result<(), StorageError> {
        device_log_mark_all_unpublished_on(&self.tx)
    }
    fn updates_from(
        &self,
        doc_id: DocId,
        after_seq: i64,
    ) -> Result<Vec<(i64, Vec<u8>, UpdateOrigin)>, StorageError> {
        updates_from_on(&self.tx, doc_id, after_seq)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

const CONFIG_GET_SQL: &str = "SELECT value FROM sync_config WHERE key = ?1";
const CONFIG_SET_SQL: &str = "INSERT INTO sync_config (key, value) VALUES (?1, ?2) \
     ON CONFLICT (key) DO UPDATE SET value = excluded.value";
const CONFIG_DELETE_SQL: &str = "DELETE FROM sync_config WHERE key = ?1";
const CONFIG_ALL_SQL: &str = "SELECT key, value FROM sync_config ORDER BY key";

const DOC_STATE_GET_SQL: &str = "SELECT doc_id, pushed_up_to_seq, next_device_seq, \
     snapshot_covers, entries_since_snapshot, bytes_since_snapshot, remote_snapshot_covers \
     FROM sync_doc_state WHERE feed = ?1 AND doc_id = ?2";
const DOC_STATE_ALL_SQL: &str = "SELECT doc_id, pushed_up_to_seq, next_device_seq, \
     snapshot_covers, entries_since_snapshot, bytes_since_snapshot, remote_snapshot_covers \
     FROM sync_doc_state WHERE feed = ?1 ORDER BY doc_id";
const DOC_STATE_PUT_SQL: &str = "INSERT INTO sync_doc_state \
     (feed, doc_id, pushed_up_to_seq, next_device_seq, snapshot_covers, entries_since_snapshot, \
      bytes_since_snapshot, remote_snapshot_covers) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
     ON CONFLICT (feed, doc_id) DO UPDATE SET \
       pushed_up_to_seq = excluded.pushed_up_to_seq, next_device_seq = excluded.next_device_seq, \
       snapshot_covers = excluded.snapshot_covers, \
       entries_since_snapshot = excluded.entries_since_snapshot, \
       bytes_since_snapshot = excluded.bytes_since_snapshot, \
       remote_snapshot_covers = excluded.remote_snapshot_covers";
const DOC_STATE_RESET_SQL: &str = "UPDATE sync_doc_state SET pushed_up_to_seq = 0, \
     remote_snapshot_covers = NULL, entries_since_snapshot = 0, bytes_since_snapshot = 0";
const DOC_STATE_RESET_IN_SQL: &str = "UPDATE sync_doc_state SET pushed_up_to_seq = 0, \
     remote_snapshot_covers = NULL, entries_since_snapshot = 0, bytes_since_snapshot = 0 \
     WHERE feed = ?1";

const SEEN_GET_SQL: &str =
    "SELECT last_seq FROM sync_seen WHERE feed = ?1 AND doc_id = ?2 AND device_id = ?3";
const SEEN_PUT_SQL: &str = "INSERT INTO sync_seen (feed, doc_id, device_id, last_seq) \
     VALUES (?1, ?2, ?3, ?4) \
     ON CONFLICT (feed, doc_id, device_id) DO UPDATE SET last_seq = excluded.last_seq";

const HEAD_ALL_SQL: &str = "SELECT device_id, cursor, chain_hash, at_ms, signed, seen_at \
     FROM sync_heads WHERE feed = ?1 ORDER BY device_id";
const HEAD_PUT_SQL: &str = "INSERT INTO sync_heads \
     (feed, device_id, cursor, chain_hash, at_ms, signed, seen_at) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
     ON CONFLICT (feed, device_id) DO UPDATE SET cursor = excluded.cursor, \
       chain_hash = excluded.chain_hash, at_ms = excluded.at_ms, signed = excluded.signed, \
       seen_at = excluded.seen_at";

const PENDING_ALL_SQL: &str = "SELECT cursor, doc_id, bytes, reason, received_at \
     FROM sync_pending WHERE feed = ?1 ORDER BY cursor, doc_id";
const PENDING_PUT_SQL: &str = "INSERT INTO sync_pending \
     (feed, cursor, doc_id, bytes, reason, received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
     ON CONFLICT (feed, cursor, doc_id) DO UPDATE SET bytes = excluded.bytes, \
       reason = excluded.reason, received_at = excluded.received_at";
/// The table is the size of the entries that wait, which is small, so the feed's rows are scanned.
const PENDING_OF_DOC_SQL: &str = "SELECT cursor, doc_id, bytes, reason, received_at \
     FROM sync_pending WHERE feed = ?1 AND doc_id = ?2 ORDER BY cursor";
const PENDING_DELETE_OF_DOC_SQL: &str = "DELETE FROM sync_pending WHERE feed = ?1 AND doc_id = ?2";

const KEYRING_UNPUBLISHED_SQL: &str = "SELECT space_id, subject, record, tombstone \
     FROM keyring WHERE published = 0 ORDER BY space_id, subject";
/// By primary key, and only while the content is the one the caller read.
const KEYRING_MARK_SQL: &str = "UPDATE keyring SET published = 1 \
     WHERE space_id = ?1 AND subject = ?2 AND record = ?3 AND tombstone = ?4";

const DEVICE_LOG_UNPUBLISHED_SQL: &str =
    "SELECT seq, entry, hash FROM device_log WHERE published = 0 ORDER BY seq";

/// By primary key and a range of it: `doc_updates (doc_id, local_seq)`.
const UPDATES_FROM_SQL: &str = "SELECT local_seq, blob, origin FROM doc_updates \
     WHERE doc_id = ?1 AND local_seq > ?2 ORDER BY local_seq";

/// A `u64` as SQLite stores it (63 bits).
pub(crate) fn int_from(value: u64, what: &str) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::InvalidData(format!("{what} is out of range")))
}

/// A number read from SQLite that must not be negative.
pub(crate) fn count_from(value: i64, what: &str) -> Result<u64, StorageError> {
    u64::try_from(value).map_err(|_| StorageError::InvalidData(format!("{what} is out of range")))
}

fn opt_int_from(value: Option<u64>, what: &str) -> Result<Option<i64>, StorageError> {
    value.map(|value| int_from(value, what)).transpose()
}

fn opt_count_from(value: Option<i64>, what: &str) -> Result<Option<u64>, StorageError> {
    value.map(|value| count_from(value, what)).transpose()
}

pub(crate) fn hash_from(bytes: &[u8], what: &str) -> Result<[u8; 32], StorageError> {
    <[u8; 32]>::try_from(bytes)
        .map_err(|_| StorageError::InvalidData(format!("{what} is not 32 bytes long")))
}

fn sync_config_get_on(conn: &Connection, key: &str) -> Result<Option<String>, StorageError> {
    Ok(conn
        .query_row(CONFIG_GET_SQL, [key], |row| row.get(0))
        .optional()?)
}

fn sync_config_set_on(conn: &Connection, key: &str, value: &str) -> Result<(), StorageError> {
    conn.execute(CONFIG_SET_SQL, params![key, value])?;
    Ok(())
}

fn sync_config_delete_on(conn: &Connection, key: &str) -> Result<(), StorageError> {
    conn.execute(CONFIG_DELETE_SQL, [key])?;
    Ok(())
}

fn sync_config_all_on(conn: &Connection) -> Result<Vec<(String, String)>, StorageError> {
    let mut stmt = conn.prepare(CONFIG_ALL_SQL)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The account id of `meta` as the personal feed (see [`SyncQueries::sync_personal_feed`]).
fn personal_feed_on(conn: &Connection) -> Result<FeedId, StorageError> {
    match crate::meta_get_on(conn, META_ACCOUNT_ID)? {
        None => Ok(UNRESOLVED_PERSONAL_FEED),
        Some(text) => text
            .parse::<AccountId>()
            .map(FeedId::from_account)
            .map_err(|_| {
                StorageError::InvalidData("the stored account id is not an id".to_owned())
            }),
    }
}

/// Raw column values of a `sync_doc_state` row.
struct DocStateColumns {
    doc_id: Vec<u8>,
    pushed_up_to_seq: i64,
    next_device_seq: i64,
    snapshot_covers: Option<i64>,
    entries_since_snapshot: i64,
    bytes_since_snapshot: i64,
    remote_snapshot_covers: Option<i64>,
}

impl DocStateColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            doc_id: row.get(0)?,
            pushed_up_to_seq: row.get(1)?,
            next_device_seq: row.get(2)?,
            snapshot_covers: row.get(3)?,
            entries_since_snapshot: row.get(4)?,
            bytes_since_snapshot: row.get(5)?,
            remote_snapshot_covers: row.get(6)?,
        })
    }

    fn decode(self) -> Result<SyncDocState, StorageError> {
        Ok(SyncDocState {
            doc_id: DocId::from_bytes(id_from(&self.doc_id, "sync state document id")?),
            pushed_up_to_seq: self.pushed_up_to_seq,
            next_device_seq: count_from(self.next_device_seq, "sync next device sequence")?,
            snapshot_covers: opt_count_from(self.snapshot_covers, "sync snapshot coverage")?,
            entries_since_snapshot: count_from(
                self.entries_since_snapshot,
                "sync entries since snapshot",
            )?,
            bytes_since_snapshot: count_from(
                self.bytes_since_snapshot,
                "sync bytes since snapshot",
            )?,
            remote_snapshot_covers: opt_count_from(
                self.remote_snapshot_covers,
                "sync remote snapshot coverage",
            )?,
        })
    }
}

fn doc_states_from(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<SyncDocState>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params, DocStateColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(DocStateColumns::decode).collect()
}

fn sync_doc_state_get_on(
    conn: &Connection,
    feed: FeedId,
    doc_id: DocId,
) -> Result<Option<SyncDocState>, StorageError> {
    Ok(doc_states_from(
        conn,
        DOC_STATE_GET_SQL,
        [feed.to_bytes(), doc_id.to_bytes()],
    )?
    .pop())
}

fn sync_doc_state_put_on(
    conn: &Connection,
    feed: FeedId,
    row: &SyncDocState,
) -> Result<(), StorageError> {
    // Every number is converted before the statement runs: a refusal writes nothing.
    let next_device_seq = int_from(row.next_device_seq, "sync next device sequence")?;
    let snapshot_covers = opt_int_from(row.snapshot_covers, "sync snapshot coverage")?;
    let entries = int_from(row.entries_since_snapshot, "sync entries since snapshot")?;
    let bytes = int_from(row.bytes_since_snapshot, "sync bytes since snapshot")?;
    let remote_covers = opt_int_from(row.remote_snapshot_covers, "sync remote snapshot coverage")?;
    conn.execute(
        DOC_STATE_PUT_SQL,
        params![
            feed.to_bytes(),
            row.doc_id.to_bytes(),
            row.pushed_up_to_seq,
            next_device_seq,
            snapshot_covers,
            entries,
            bytes,
            remote_covers,
        ],
    )?;
    Ok(())
}

fn sync_doc_state_all_on(
    conn: &Connection,
    feed: FeedId,
) -> Result<Vec<SyncDocState>, StorageError> {
    doc_states_from(conn, DOC_STATE_ALL_SQL, [feed.to_bytes()])
}

/// Resets the push state of one feed's rows, or of every feed's (`None`).
fn sync_doc_state_reset_on(conn: &Connection, feed: Option<FeedId>) -> Result<(), StorageError> {
    match feed {
        None => conn.execute(DOC_STATE_RESET_SQL, [])?,
        Some(feed) => conn.execute(DOC_STATE_RESET_IN_SQL, [feed.to_bytes()])?,
    };
    Ok(())
}

fn sync_seen_get_on(
    conn: &Connection,
    feed: FeedId,
    doc_id: DocId,
    device_id: DeviceId,
) -> Result<Option<u64>, StorageError> {
    conn.query_row(
        SEEN_GET_SQL,
        [feed.to_bytes(), doc_id.to_bytes(), device_id.to_bytes()],
        |row| row.get::<_, i64>(0),
    )
    .optional()?
    .map(|seq| count_from(seq, "sync last sequence"))
    .transpose()
}

fn sync_seen_put_on(
    conn: &Connection,
    feed: FeedId,
    doc_id: DocId,
    device_id: DeviceId,
    last_seq: u64,
) -> Result<(), StorageError> {
    conn.execute(
        SEEN_PUT_SQL,
        params![
            feed.to_bytes(),
            doc_id.to_bytes(),
            device_id.to_bytes(),
            int_from(last_seq, "sync last sequence")?
        ],
    )?;
    Ok(())
}

fn sync_seen_clear_on(conn: &Connection, feed: Option<FeedId>) -> Result<(), StorageError> {
    match feed {
        None => conn.execute("DELETE FROM sync_seen", [])?,
        Some(feed) => conn.execute("DELETE FROM sync_seen WHERE feed = ?1", [feed.to_bytes()])?,
    };
    Ok(())
}

/// Raw column values of a `sync_heads` row.
struct HeadColumns {
    device_id: Vec<u8>,
    cursor: i64,
    chain_hash: Vec<u8>,
    at_ms: i64,
    signed: Vec<u8>,
    seen_at: i64,
}

impl HeadColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            device_id: row.get(0)?,
            cursor: row.get(1)?,
            chain_hash: row.get(2)?,
            at_ms: row.get(3)?,
            signed: row.get(4)?,
            seen_at: row.get(5)?,
        })
    }

    fn decode(self) -> Result<SyncHeadRow, StorageError> {
        Ok(SyncHeadRow {
            device_id: DeviceId::from_bytes(id_from(&self.device_id, "sync head device id")?),
            cursor: count_from(self.cursor, "sync head cursor")?,
            chain_hash: hash_from(&self.chain_hash, "sync head chain hash")?,
            at_ms: self.at_ms,
            signed: self.signed,
            seen_at: self.seen_at,
        })
    }
}

fn sync_heads_put_on(
    conn: &Connection,
    feed: FeedId,
    row: &SyncHeadRow,
) -> Result<(), StorageError> {
    conn.execute(
        HEAD_PUT_SQL,
        params![
            feed.to_bytes(),
            row.device_id.to_bytes(),
            int_from(row.cursor, "sync head cursor")?,
            &row.chain_hash[..],
            row.at_ms,
            row.signed,
            row.seen_at,
        ],
    )?;
    Ok(())
}

fn sync_heads_all_on(conn: &Connection, feed: FeedId) -> Result<Vec<SyncHeadRow>, StorageError> {
    let mut stmt = conn.prepare(HEAD_ALL_SQL)?;
    let rows = stmt
        .query_map([feed.to_bytes()], HeadColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(HeadColumns::decode).collect()
}

fn sync_heads_delete_on(
    conn: &Connection,
    feed: FeedId,
    device_id: DeviceId,
) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM sync_heads WHERE feed = ?1 AND device_id = ?2",
        [feed.to_bytes(), device_id.to_bytes()],
    )?;
    Ok(())
}

fn sync_heads_clear_on(conn: &Connection, feed: Option<FeedId>) -> Result<(), StorageError> {
    match feed {
        None => conn.execute("DELETE FROM sync_heads", [])?,
        Some(feed) => conn.execute("DELETE FROM sync_heads WHERE feed = ?1", [feed.to_bytes()])?,
    };
    Ok(())
}

/// Raw column values of a `sync_pending` row.
struct PendingColumns {
    cursor: i64,
    doc_id: Vec<u8>,
    bytes: Vec<u8>,
    reason: i64,
    received_at: i64,
}

impl PendingColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            cursor: row.get(0)?,
            doc_id: row.get(1)?,
            bytes: row.get(2)?,
            reason: row.get(3)?,
            received_at: row.get(4)?,
        })
    }

    fn decode(self) -> Result<SyncPendingRow, StorageError> {
        Ok(SyncPendingRow {
            cursor: count_from(self.cursor, "pending entry cursor")?,
            doc_id: DocId::from_bytes(id_from(&self.doc_id, "pending entry document id")?),
            bytes: self.bytes,
            reason: u8::try_from(self.reason).map_err(|_| {
                StorageError::InvalidData("pending entry reason is out of range".to_owned())
            })?,
            received_at: self.received_at,
        })
    }
}

fn pending_from(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<SyncPendingRow>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params, PendingColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(PendingColumns::decode).collect()
}

fn sync_pending_put_on(
    conn: &Connection,
    feed: FeedId,
    row: &SyncPendingRow,
) -> Result<(), StorageError> {
    conn.execute(
        PENDING_PUT_SQL,
        params![
            feed.to_bytes(),
            int_from(row.cursor, "pending entry cursor")?,
            row.doc_id.to_bytes(),
            row.bytes,
            i64::from(row.reason),
            row.received_at,
        ],
    )?;
    Ok(())
}

fn sync_pending_take_for_doc_on(
    conn: &Connection,
    feed: FeedId,
    doc_id: DocId,
) -> Result<Vec<SyncPendingRow>, StorageError> {
    // Read and validate every row first: one that cannot be read fails the call before
    // anything is deleted.
    let key = [feed.to_bytes(), doc_id.to_bytes()];
    let rows = pending_from(conn, PENDING_OF_DOC_SQL, key)?;
    if !rows.is_empty() {
        conn.execute(PENDING_DELETE_OF_DOC_SQL, key)?;
    }
    Ok(rows)
}

fn sync_pending_all_on(
    conn: &Connection,
    feed: FeedId,
) -> Result<Vec<SyncPendingRow>, StorageError> {
    pending_from(conn, PENDING_ALL_SQL, [feed.to_bytes()])
}

fn sync_pending_delete_on(
    conn: &Connection,
    feed: FeedId,
    cursor: u64,
    doc_id: DocId,
) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM sync_pending WHERE feed = ?1 AND cursor = ?2 AND doc_id = ?3",
        params![
            feed.to_bytes(),
            int_from(cursor, "pending entry cursor")?,
            doc_id.to_bytes()
        ],
    )?;
    Ok(())
}

fn sync_pending_clear_on(conn: &Connection, feed: Option<FeedId>) -> Result<(), StorageError> {
    match feed {
        None => conn.execute("DELETE FROM sync_pending", [])?,
        Some(feed) => conn.execute(
            "DELETE FROM sync_pending WHERE feed = ?1",
            [feed.to_bytes()],
        )?,
    };
    Ok(())
}

fn sync_pending_count_on(conn: &Connection, feed: FeedId) -> Result<u32, StorageError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sync_pending WHERE feed = ?1",
        [feed.to_bytes()],
        |row| row.get(0),
    )?;
    u32::try_from(count)
        .map_err(|_| StorageError::InvalidData("pending entry count is out of range".to_owned()))
}

fn keyring_unpublished_on(conn: &Connection) -> Result<Vec<KeyringRow>, StorageError> {
    let mut stmt = conn.prepare(KEYRING_UNPUBLISHED_SQL)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(space_id, subject, record, tombstone)| {
            Ok(KeyringRow {
                space_id: SpaceId::from_bytes(id_from(&space_id, "keyring space id")?),
                subject,
                record,
                tombstone: match tombstone {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(StorageError::InvalidData(
                            "keyring tombstone flag is neither 0 nor 1".to_owned(),
                        ));
                    }
                },
            })
        })
        .collect()
}

fn keyring_mark_published_on(conn: &Connection, rows: &[KeyringRow]) -> Result<(), StorageError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(KEYRING_MARK_SQL)?;
    for row in rows {
        stmt.execute(params![
            row.space_id.to_bytes(),
            row.subject,
            row.record,
            i64::from(row.tombstone)
        ])?;
    }
    Ok(())
}

fn keyring_mark_all_unpublished_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("UPDATE keyring SET published = 0", [])?;
    Ok(())
}

fn device_log_unpublished_on(conn: &Connection) -> Result<Vec<DeviceLogRow>, StorageError> {
    let mut stmt = conn.prepare(DEVICE_LOG_UNPUBLISHED_SQL)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(seq, entry, hash)| {
            Ok(DeviceLogRow {
                seq: count_from(seq, "device log sequence number")?,
                entry,
                hash: hash_from(&hash, "device log hash")?,
            })
        })
        .collect()
}

fn device_log_mark_published_on(conn: &Connection, up_to_seq: u64) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE device_log SET published = 1 WHERE seq <= ?1",
        [int_from(up_to_seq, "device log sequence number")?],
    )?;
    Ok(())
}

fn device_log_mark_all_unpublished_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("UPDATE device_log SET published = 0", [])?;
    Ok(())
}

fn origin_from(code: i64) -> Result<UpdateOrigin, StorageError> {
    // The numbers are on disk (`doc_updates.origin`): see `UpdateOrigin`.
    match code {
        0 => Ok(UpdateOrigin::Local),
        1 => Ok(UpdateOrigin::Remote),
        2 => Ok(UpdateOrigin::Core),
        other => Err(StorageError::InvalidData(format!(
            "unknown update origin {other}"
        ))),
    }
}

fn updates_from_on(
    conn: &Connection,
    doc_id: DocId,
    after_seq: i64,
) -> Result<Vec<(i64, Vec<u8>, UpdateOrigin)>, StorageError> {
    let mut stmt = conn.prepare(UPDATES_FROM_SQL)?;
    let rows = stmt
        .query_map(params![doc_id.to_bytes(), after_seq], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(seq, blob, origin)| Ok((seq, blob, origin_from(origin)?)))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;
    use crate::{DocKindCode, DocQueries, DocRow, SecurityQueries, migrations};

    /// What a scenario needs of a connection or a transaction: both implement all three.
    trait Queries: SyncQueries + DocQueries + SecurityQueries {}
    impl<T: SyncQueries + DocQueries + SecurityQueries> Queries for T {}

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that
    /// both `SyncQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
    }

    const fn doc(n: u8) -> DocId {
        DocId::from_bytes([n; 16])
    }

    const fn device(n: u8) -> DeviceId {
        DeviceId::from_bytes([n; 16])
    }

    const fn space(n: u8) -> SpaceId {
        SpaceId::from_bytes([n; 16])
    }

    /// A deterministic blob that cycles through every byte value, NUL included.
    fn blob(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| seed.wrapping_add(u8::try_from(i % 256).unwrap()))
            .collect()
    }

    fn pair(key: &str, value: &str) -> (String, String) {
        (key.to_owned(), value.to_owned())
    }

    /// A state that differs from every other in each column.
    fn state(n: u8) -> SyncDocState {
        SyncDocState {
            doc_id: doc(n),
            pushed_up_to_seq: i64::from(n) * 10,
            next_device_seq: u64::from(n) * 10 + 1,
            snapshot_covers: Some(u64::from(n) * 10 + 2),
            entries_since_snapshot: u64::from(n) * 10 + 3,
            bytes_since_snapshot: u64::from(n) * 10 + 4,
            remote_snapshot_covers: Some(u64::from(n) * 10 + 5),
        }
    }

    fn head(n: u8) -> SyncHeadRow {
        SyncHeadRow {
            device_id: device(n),
            cursor: u64::from(n) * 100,
            chain_hash: std::array::from_fn(|i| n.wrapping_add(u8::try_from(i).unwrap())),
            at_ms: i64::from(n) * 1_000,
            signed: blob(60 + usize::from(n), n),
            seen_at: i64::from(n) * 1_000 + 1,
        }
    }

    fn pending(cursor: u8, doc_n: u8, reason: u8) -> SyncPendingRow {
        SyncPendingRow {
            cursor: u64::from(cursor),
            doc_id: doc(doc_n),
            bytes: blob(30 + usize::from(cursor), cursor),
            reason,
            received_at: i64::from(cursor) * 10,
        }
    }

    fn key_row(space_n: u8, subject: &[u8], record: &[u8], tombstone: bool) -> KeyringRow {
        KeyringRow {
            space_id: space(space_n),
            subject: subject.to_vec(),
            record: record.to_vec(),
            tombstone,
        }
    }

    fn log_row(seq: u8) -> DeviceLogRow {
        DeviceLogRow {
            seq: u64::from(seq),
            entry: blob(20 + usize::from(seq), seq),
            hash: [seq + 1; 32],
        }
    }

    fn doc_row(n: u8) -> DocRow {
        DocRow {
            doc_id: doc(n),
            kind: DocKindCode::Page,
            space_id: Some(space(1)),
            created_at: 10,
            format_version: 1,
            min_reader_version: 1,
        }
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    fn assert_invalid(err: StorageError, what: &str) {
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m.contains(what)),
            "{what}: {err}"
        );
    }

    const SYNC_TABLES: [&str; 5] = [
        "sync_config",
        "sync_doc_state",
        "sync_heads",
        "sync_pending",
        "sync_seen",
    ];

    // ---- the schema -----------------------------------------------------------------

    /// `(name, type, notnull, primary key position, default)` of a column.
    type Column = (String, String, bool, i64, Option<String>);

    fn columns(conn: &Connection, table: &str) -> Vec<Column> {
        conn.prepare(
            "SELECT name, type, \"notnull\", pk, dflt_value FROM pragma_table_info(?1) \
             ORDER BY cid",
        )
        .unwrap()
        .query_map([table], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    fn col(name: &str, ty: &str, notnull: bool, pk: i64, default: Option<&str>) -> Column {
        (
            name.to_owned(),
            ty.to_owned(),
            notnull,
            pk,
            default.map(str::to_owned),
        )
    }

    fn table_names(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn schema_v6_matches_the_documented_tables() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 5).unwrap();
        let v5_tables = table_names(&conn);
        let v5_keyring = columns(&conn, "keyring");
        let v5_log = columns(&conn, "device_log");
        migrations().to_version(&mut conn, 6).unwrap();
        let conn = &conn;

        // Five new tables, nothing else.
        let new: Vec<String> = table_names(conn)
            .into_iter()
            .filter(|name| !v5_tables.contains(name))
            .collect();
        assert_eq!(new, SYNC_TABLES);

        assert_eq!(
            columns(conn, "sync_config"),
            [
                col("key", "TEXT", false, 1, None),
                col("value", "TEXT", true, 0, None)
            ]
        );
        assert_eq!(
            columns(conn, "sync_doc_state"),
            [
                col("doc_id", "BLOB", false, 1, None),
                col("pushed_up_to_seq", "INTEGER", true, 0, Some("0")),
                col("next_device_seq", "INTEGER", true, 0, Some("1")),
                col("snapshot_covers", "INTEGER", false, 0, None),
                col("entries_since_snapshot", "INTEGER", true, 0, Some("0")),
                col("bytes_since_snapshot", "INTEGER", true, 0, Some("0")),
                col("remote_snapshot_covers", "INTEGER", false, 0, None),
            ]
        );
        assert_eq!(
            columns(conn, "sync_seen"),
            [
                col("doc_id", "BLOB", true, 1, None),
                col("device_id", "BLOB", true, 2, None),
                col("last_seq", "INTEGER", true, 0, None),
            ]
        );
        assert_eq!(
            columns(conn, "sync_heads"),
            [
                col("device_id", "BLOB", false, 1, None),
                col("cursor", "INTEGER", true, 0, None),
                col("chain_hash", "BLOB", true, 0, None),
                col("at_ms", "INTEGER", true, 0, None),
                col("signed", "BLOB", true, 0, None),
                col("seen_at", "INTEGER", true, 0, None),
            ]
        );
        assert_eq!(
            columns(conn, "sync_pending"),
            [
                col("cursor", "INTEGER", false, 1, None),
                col("doc_id", "BLOB", true, 0, None),
                col("bytes", "BLOB", true, 0, None),
                col("reason", "INTEGER", true, 0, None),
                col("received_at", "INTEGER", true, 0, None),
            ]
        );

        // The two old tables gained exactly one column each, at the end, 0 by default.
        for (table, old) in [("keyring", v5_keyring), ("device_log", v5_log)] {
            let now = columns(conn, table);
            assert_eq!(now[..old.len()], old[..], "{table}: the old columns");
            assert_eq!(
                now[old.len()..],
                [col("published", "INTEGER", true, 0, Some("0"))],
                "{table}: the new column"
            );
        }

        // Local bookkeeping: no foreign keys (the keyring's pseudo-document is not in docs).
        for table in SYNC_TABLES {
            let foreign: i64 = conn
                .query_row(
                    "SELECT count(*) FROM pragma_foreign_key_list(?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(foreign, 0, "{table}");
        }
    }

    #[test]
    fn migrating_from_v5_keeps_the_rows_and_publishes_nothing() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 5).unwrap();
        conn.execute_batch(
            "INSERT INTO device_log VALUES (0, x'04', zeroblob(32));
             INSERT INTO device_log VALUES (1, x'05', zeroblob(32));
             INSERT INTO keyring VALUES (zeroblob(16), x'05', x'06', 0);
             INSERT INTO keyring VALUES (zeroblob(16), x'07', x'08', 1);",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();

        for table in ["device_log", "keyring"] {
            assert_eq!(count(&conn, table), 2, "{table}");
            let published: i64 = conn
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE published = 0"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(published, 2, "{table}: published is 0 everywhere");
        }
        for table in SYNC_TABLES {
            assert_eq!(count(&conn, table), 0, "{table}");
        }
        // A row written without the new column starts at 0 too.
        conn.execute(
            "INSERT INTO device_log (seq, entry, hash) VALUES (2, x'09', zeroblob(32))",
            [],
        )
        .unwrap();
        let published: i64 = conn
            .query_row(
                "SELECT published FROM device_log WHERE seq = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(published, 0);
    }

    #[test]
    fn a_default_row_of_sync_doc_state_reads_back_as_a_document_never_pushed() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO sync_doc_state (feed, doc_id) VALUES (zeroblob(16), ?1)",
                [doc(1).to_bytes()],
            )
            .unwrap();
        assert_eq!(
            db.sync_doc_state_get(doc(1)).unwrap(),
            Some(SyncDocState {
                doc_id: doc(1),
                pushed_up_to_seq: 0,
                next_device_seq: 1,
                snapshot_covers: None,
                entries_since_snapshot: 0,
                bytes_since_snapshot: 0,
                remote_snapshot_covers: None,
            })
        );
    }

    // ---- sync_config ----------------------------------------------------------------

    #[test]
    fn config_is_a_set_of_text_pairs_with_upsert_and_delete() {
        on_db_and_tx(|q| {
            assert_eq!(q.sync_config_get("server_url").unwrap(), None);
            assert!(q.sync_config_all().unwrap().is_empty());

            q.sync_config_set("server_url", "https://a.example")
                .unwrap();
            q.sync_config_set("enabled", "1").unwrap();
            q.sync_config_set("remote_cursor", "").unwrap();
            assert_eq!(
                q.sync_config_get("server_url").unwrap().as_deref(),
                Some("https://a.example")
            );
            assert_eq!(
                q.sync_config_get("remote_cursor").unwrap().as_deref(),
                Some(""),
                "an empty value is a value"
            );

            // Setting again replaces: one row per key.
            q.sync_config_set("server_url", "https://b.example \u{1F389}")
                .unwrap();
            assert_eq!(
                q.sync_config_all().unwrap(),
                [
                    pair("enabled", "1"),
                    pair("remote_cursor", ""),
                    pair("server_url", "https://b.example \u{1F389}")
                ],
                "ordered by key"
            );

            q.sync_config_delete("enabled").unwrap();
            q.sync_config_delete("enabled").unwrap();
            q.sync_config_delete("never_set").unwrap();
            assert_eq!(q.sync_config_get("enabled").unwrap(), None);
            assert_eq!(q.sync_config_all().unwrap().len(), 2);
        });
    }

    #[test]
    fn config_keys_are_exact_and_case_sensitive() {
        on_db_and_tx(|q| {
            q.sync_config_set("Key", "upper").unwrap();
            q.sync_config_set("key", "lower").unwrap();
            assert_eq!(q.sync_config_get("Key").unwrap().as_deref(), Some("upper"));
            assert_eq!(q.sync_config_get("key").unwrap().as_deref(), Some("lower"));
            assert_eq!(q.sync_config_get("KEY").unwrap(), None);
        });
    }

    // ---- sync_doc_state -------------------------------------------------------------

    #[test]
    fn doc_state_round_trips_every_column() {
        on_db_and_tx(|q| {
            assert_eq!(q.sync_doc_state_get(doc(1)).unwrap(), None);
            assert!(q.sync_doc_state_all().unwrap().is_empty());

            // Written in neither id order.
            for n in [3, 1, 2] {
                q.sync_doc_state_put(&state(n)).unwrap();
            }
            for n in 1..=3 {
                assert_eq!(q.sync_doc_state_get(doc(n)).unwrap(), Some(state(n)));
            }
            assert_eq!(
                q.sync_doc_state_all().unwrap(),
                [state(1), state(2), state(3)],
                "ordered by document id"
            );

            // Absent options come back absent, the extremes come back exact.
            let row = SyncDocState {
                snapshot_covers: None,
                remote_snapshot_covers: None,
                pushed_up_to_seq: -1,
                next_device_seq: u64::try_from(i64::MAX).unwrap(),
                entries_since_snapshot: 0,
                bytes_since_snapshot: u64::try_from(i64::MAX).unwrap(),
                ..state(9)
            };
            q.sync_doc_state_put(&row).unwrap();
            assert_eq!(q.sync_doc_state_get(doc(9)).unwrap(), Some(row));
        });
    }

    #[test]
    fn doc_state_put_is_an_upsert_that_replaces_every_column() {
        on_db_and_tx(|q| {
            q.sync_doc_state_put(&state(1)).unwrap();
            q.sync_doc_state_put(&state(2)).unwrap();
            let replacement = SyncDocState {
                doc_id: doc(1),
                pushed_up_to_seq: 1,
                next_device_seq: 2,
                snapshot_covers: None,
                entries_since_snapshot: 0,
                bytes_since_snapshot: 0,
                remote_snapshot_covers: None,
            };
            q.sync_doc_state_put(&replacement).unwrap();
            assert_eq!(
                q.sync_doc_state_get(doc(1)).unwrap(),
                Some(replacement.clone()),
                "the optional columns went back to NULL"
            );
            assert_eq!(
                q.sync_doc_state_all().unwrap(),
                [replacement, state(2)],
                "one row per document, the other untouched"
            );
        });
    }

    #[test]
    fn doc_state_covers_the_keyring_pseudo_document_without_a_docs_row() {
        on_db_and_tx(|q| {
            let keyring = SyncDocState {
                doc_id: DocId::from_bytes([0; 16]),
                ..state(5)
            };
            q.sync_doc_state_put(&keyring).unwrap();
            assert!(q.doc_get(keyring.doc_id).unwrap().is_none());
            assert_eq!(q.sync_doc_state_get(keyring.doc_id).unwrap(), Some(keyring));
        });
    }

    #[test]
    fn doc_state_reset_all_forgets_the_server_and_keeps_the_device_sequence() {
        on_db_and_tx(|q| {
            q.sync_doc_state_reset_all().unwrap(); // nothing to reset
            for n in 1..=3 {
                q.sync_doc_state_put(&state(n)).unwrap();
            }
            q.sync_doc_state_put(&SyncDocState {
                snapshot_covers: None,
                remote_snapshot_covers: None,
                ..state(4)
            })
            .unwrap();

            q.sync_doc_state_reset_all().unwrap();

            let all = q.sync_doc_state_all().unwrap();
            assert_eq!(all.len(), 4, "no row is removed");
            for row in all {
                let n = row.doc_id.to_bytes()[0];
                let before = state(n);
                assert_eq!(row.pushed_up_to_seq, 0, "{n}");
                assert_eq!(row.remote_snapshot_covers, None, "{n}");
                assert_eq!(row.entries_since_snapshot, 0, "{n}");
                assert_eq!(row.bytes_since_snapshot, 0, "{n}");
                assert_eq!(row.next_device_seq, before.next_device_seq, "{n}: kept");
                if n < 4 {
                    assert_eq!(row.snapshot_covers, before.snapshot_covers, "{n}: kept");
                } else {
                    assert_eq!(row.snapshot_covers, None, "{n}");
                }
            }
        });
    }

    #[test]
    fn doc_state_numbers_that_do_not_fit_sqlite_are_refused_and_nothing_is_written() {
        let too_big = u64::try_from(i64::MAX).unwrap() + 1;
        on_db_and_tx(|q| {
            for (what, row) in [
                (
                    "next device sequence",
                    SyncDocState {
                        next_device_seq: too_big,
                        ..state(1)
                    },
                ),
                (
                    "snapshot coverage",
                    SyncDocState {
                        snapshot_covers: Some(too_big),
                        ..state(1)
                    },
                ),
                (
                    "entries since snapshot",
                    SyncDocState {
                        entries_since_snapshot: u64::MAX,
                        ..state(1)
                    },
                ),
                (
                    "bytes since snapshot",
                    SyncDocState {
                        bytes_since_snapshot: too_big,
                        ..state(1)
                    },
                ),
                (
                    "remote snapshot coverage",
                    SyncDocState {
                        remote_snapshot_covers: Some(u64::MAX),
                        ..state(1)
                    },
                ),
            ] {
                assert_invalid(q.sync_doc_state_put(&row).unwrap_err(), what);
            }
            assert!(q.sync_doc_state_all().unwrap().is_empty());
        });
    }

    // ---- sync_seen ------------------------------------------------------------------

    #[test]
    fn seen_is_kept_per_document_and_device_with_upsert() {
        on_db_and_tx(|q| {
            assert_eq!(q.sync_seen_get(doc(1), device(1)).unwrap(), None);
            q.sync_seen_put(doc(1), device(1), 5).unwrap();
            q.sync_seen_put(doc(1), device(2), 6).unwrap();
            q.sync_seen_put(doc(2), device(1), 7).unwrap();
            assert_eq!(q.sync_seen_get(doc(1), device(1)).unwrap(), Some(5));
            assert_eq!(q.sync_seen_get(doc(1), device(2)).unwrap(), Some(6));
            assert_eq!(q.sync_seen_get(doc(2), device(1)).unwrap(), Some(7));
            assert_eq!(q.sync_seen_get(doc(2), device(2)).unwrap(), None);

            // Upsert: the value is replaced, as given (also a lower one, also 0).
            q.sync_seen_put(doc(1), device(1), 9).unwrap();
            assert_eq!(q.sync_seen_get(doc(1), device(1)).unwrap(), Some(9));
            q.sync_seen_put(doc(1), device(1), 0).unwrap();
            assert_eq!(q.sync_seen_get(doc(1), device(1)).unwrap(), Some(0));
            let max = u64::try_from(i64::MAX).unwrap();
            q.sync_seen_put(doc(1), device(1), max).unwrap();
            assert_eq!(q.sync_seen_get(doc(1), device(1)).unwrap(), Some(max));
        });
    }

    #[test]
    fn seen_clear_removes_every_row_and_a_number_above_i64_max_is_refused() {
        on_db_and_tx(|q| {
            q.sync_seen_clear().unwrap(); // nothing to clear
            q.sync_seen_put(doc(1), device(1), 5).unwrap();
            q.sync_seen_put(doc(2), device(2), 6).unwrap();
            q.sync_seen_clear().unwrap();
            assert_eq!(q.sync_seen_get(doc(1), device(1)).unwrap(), None);
            assert_eq!(q.sync_seen_get(doc(2), device(2)).unwrap(), None);

            let err = q.sync_seen_put(doc(1), device(1), u64::MAX).unwrap_err();
            assert_invalid(err, "out of range");
            assert_eq!(q.sync_seen_get(doc(1), device(1)).unwrap(), None);
        });
    }

    // ---- sync_heads -----------------------------------------------------------------

    #[test]
    fn heads_are_one_row_per_device_and_a_put_replaces_it() {
        on_db_and_tx(|q| {
            assert!(q.sync_heads_all().unwrap().is_empty());
            for n in [3, 1, 2] {
                q.sync_heads_put(&head(n)).unwrap();
            }
            assert_eq!(
                q.sync_heads_all().unwrap(),
                [head(1), head(2), head(3)],
                "ordered by device id"
            );

            let newer = SyncHeadRow {
                cursor: 999,
                chain_hash: [0xEE; 32],
                at_ms: 5,
                signed: vec![0, 0xFF, 0x80],
                seen_at: 6,
                ..head(2)
            };
            q.sync_heads_put(&newer).unwrap();
            assert_eq!(
                q.sync_heads_all().unwrap(),
                [head(1), newer, head(3)],
                "device 2 was replaced, not added to"
            );
        });
    }

    #[test]
    fn heads_delete_and_clear() {
        on_db_and_tx(|q| {
            q.sync_heads_delete(device(1)).unwrap(); // none yet
            for n in 1..=3 {
                q.sync_heads_put(&head(n)).unwrap();
            }
            q.sync_heads_delete(device(2)).unwrap();
            q.sync_heads_delete(device(2)).unwrap();
            assert_eq!(q.sync_heads_all().unwrap(), [head(1), head(3)]);
            q.sync_heads_clear().unwrap();
            assert!(q.sync_heads_all().unwrap().is_empty());
            q.sync_heads_clear().unwrap();
        });
    }

    #[test]
    fn a_head_cursor_above_i64_max_is_refused() {
        on_db_and_tx(|q| {
            let err = q
                .sync_heads_put(&SyncHeadRow {
                    cursor: u64::MAX,
                    ..head(1)
                })
                .unwrap_err();
            assert_invalid(err, "out of range");
            assert!(q.sync_heads_all().unwrap().is_empty());
        });
    }

    // ---- sync_pending ---------------------------------------------------------------

    #[test]
    fn pending_entries_come_back_in_cursor_order_and_a_cursor_is_unique_per_document() {
        on_db_and_tx(|q| {
            assert!(q.sync_pending_all().unwrap().is_empty());
            assert_eq!(q.sync_pending_count().unwrap(), 0);
            for (cursor, doc_n, reason) in [(30, 1, 1), (10, 2, 2), (20, 1, 1)] {
                q.sync_pending_put(&pending(cursor, doc_n, reason)).unwrap();
            }
            assert_eq!(
                q.sync_pending_all().unwrap(),
                [pending(10, 2, 2), pending(20, 1, 1), pending(30, 1, 1)]
            );
            assert_eq!(q.sync_pending_count().unwrap(), 3);

            // The same cursor and document again replaces the entry (a cursor names one
            // entry of the feed).
            let replacement = SyncPendingRow {
                bytes: vec![1, 2, 3],
                ..pending(20, 1, 2)
            };
            q.sync_pending_put(&replacement).unwrap();
            assert_eq!(
                q.sync_pending_all().unwrap(),
                [pending(10, 2, 2), replacement.clone(), pending(30, 1, 1)]
            );
            assert_eq!(q.sync_pending_count().unwrap(), 3);

            // The same cursor for another document is another row: parked snapshots are
            // stored under the cursor they cover, which several of them share.
            let sibling = pending(20, 5, 3);
            q.sync_pending_put(&sibling).unwrap();
            assert_eq!(q.sync_pending_count().unwrap(), 4);
            let at_20: Vec<SyncPendingRow> = q
                .sync_pending_all()
                .unwrap()
                .into_iter()
                .filter(|row| row.cursor == 20)
                .collect();
            assert_eq!(at_20.len(), 2);
            assert!(at_20.contains(&replacement) && at_20.contains(&sibling));
        });
    }

    #[test]
    fn take_for_doc_returns_the_entries_of_one_document_in_cursor_order_and_deletes_them() {
        on_db_and_tx(|q| {
            for (cursor, doc_n, reason) in
                [(30, 1, 1), (10, 2, 2), (20, 1, 1), (5, 3, 1), (40, 1, 2)]
            {
                q.sync_pending_put(&pending(cursor, doc_n, reason)).unwrap();
            }
            assert_eq!(
                q.sync_pending_take_for_doc(doc(1)).unwrap(),
                [pending(20, 1, 1), pending(30, 1, 1), pending(40, 1, 2)]
            );
            assert_eq!(
                q.sync_pending_all().unwrap(),
                [pending(5, 3, 1), pending(10, 2, 2)],
                "only the document's entries went"
            );
            assert_eq!(q.sync_pending_count().unwrap(), 2);
            assert!(
                q.sync_pending_take_for_doc(doc(1)).unwrap().is_empty(),
                "taken once"
            );
            assert!(q.sync_pending_take_for_doc(doc(9)).unwrap().is_empty());
            assert_eq!(q.sync_pending_count().unwrap(), 2);
        });
    }

    #[test]
    fn pending_delete_and_clear() {
        on_db_and_tx(|q| {
            q.sync_pending_delete(1, doc(1)).unwrap(); // none yet
            for cursor in [1, 2, 3] {
                q.sync_pending_put(&pending(cursor, 1, 1)).unwrap();
            }
            // Another document under the same cursor (parked snapshots share the cursor
            // they cover) is its own row; a delete names the cursor and the document.
            q.sync_pending_put(&pending(2, 2, 3)).unwrap();
            assert_eq!(q.sync_pending_count().unwrap(), 4);
            q.sync_pending_delete(2, doc(1)).unwrap();
            q.sync_pending_delete(2, doc(1)).unwrap();
            assert_eq!(
                q.sync_pending_all().unwrap(),
                [pending(1, 1, 1), pending(2, 2, 3), pending(3, 1, 1)]
            );
            q.sync_pending_clear().unwrap();
            assert!(q.sync_pending_all().unwrap().is_empty());
            assert_eq!(q.sync_pending_count().unwrap(), 0);
            q.sync_pending_clear().unwrap();
        });
    }

    #[test]
    fn pending_reasons_are_stored_as_given() {
        on_db_and_tx(|q| {
            for reason in [0u8, 1, 2, 3, 255] {
                q.sync_pending_put(&pending(reason, 1, reason)).unwrap();
            }
            let reasons: Vec<u8> = q
                .sync_pending_all()
                .unwrap()
                .iter()
                .map(|row| row.reason)
                .collect();
            assert_eq!(reasons, [0, 1, 2, 3, 255]);
        });
    }

    #[test]
    fn a_pending_cursor_above_i64_max_is_refused() {
        on_db_and_tx(|q| {
            for cursor in [u64::MAX, u64::try_from(i64::MAX).unwrap() + 1] {
                let row = SyncPendingRow {
                    cursor,
                    ..pending(1, 1, 1)
                };
                assert_invalid(q.sync_pending_put(&row).unwrap_err(), "out of range");
                assert_invalid(
                    q.sync_pending_delete(cursor, doc(1)).unwrap_err(),
                    "out of range",
                );
            }
            assert_eq!(q.sync_pending_count().unwrap(), 0);
        });
    }

    // ---- damaged rows ---------------------------------------------------------------

    #[test]
    fn damaged_rows_are_invalid_data_and_never_a_panic() {
        // (statement, what reads it, what is wrong)
        type Read = fn(&Db) -> Result<(), StorageError>;
        let cases: [(&str, Read, &str); 9] = [
            (
                "INSERT INTO sync_doc_state (feed, doc_id) VALUES (zeroblob(16), x'0102')",
                |db| db.sync_doc_state_all().map(drop),
                "a document id of 2 bytes",
            ),
            (
                "INSERT INTO sync_doc_state (feed, doc_id, next_device_seq) \
                 VALUES (zeroblob(16), zeroblob(16), -1)",
                |db| db.sync_doc_state_get(doc(0)).map(drop),
                "a negative device sequence",
            ),
            (
                "INSERT INTO sync_doc_state (feed, doc_id, remote_snapshot_covers) \
                 VALUES (zeroblob(16), zeroblob(16), -5)",
                |db| db.sync_doc_state_all().map(drop),
                "a negative remote coverage",
            ),
            (
                "INSERT INTO sync_seen VALUES (zeroblob(16), zeroblob(16), zeroblob(16), -1)",
                |db| db.sync_seen_get(doc(0), device(0)).map(drop),
                "a negative last sequence",
            ),
            (
                "INSERT INTO sync_heads VALUES (zeroblob(16), zeroblob(15), 1, zeroblob(32), 0, x'01', 0)",
                |db| db.sync_heads_all().map(drop),
                "a device id of 15 bytes",
            ),
            (
                "INSERT INTO sync_heads VALUES (zeroblob(16), zeroblob(16), 1, zeroblob(31), 0, x'01', 0)",
                |db| db.sync_heads_all().map(drop),
                "a chain hash of 31 bytes",
            ),
            (
                "INSERT INTO sync_pending VALUES (zeroblob(16), -3, zeroblob(16), x'01', 1, 0)",
                |db| db.sync_pending_all().map(drop),
                "a negative cursor",
            ),
            (
                "INSERT INTO sync_pending VALUES (zeroblob(16), 3, zeroblob(16), x'01', 256, 0)",
                |db| db.sync_pending_all().map(drop),
                "a reason above 255",
            ),
            (
                "INSERT INTO sync_pending VALUES (zeroblob(16), 3, zeroblob(8), x'01', 1, 0)",
                |db| db.sync_pending_all().map(drop),
                "a pending document id of 8 bytes",
            ),
        ];
        for (sql, read, what) in cases {
            let db = new_db();
            db.connection().execute(sql, []).unwrap();
            let err = read(&db).unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{what}: {err}");
        }
    }

    #[test]
    fn a_damaged_pending_row_is_not_deleted_by_take_for_doc() {
        let db = new_db();
        db.sync_pending_put(&pending(1, 1, 1)).unwrap();
        db.connection()
            .execute(
                "INSERT INTO sync_pending VALUES (zeroblob(16), 2, ?1, x'01', 999, 0)",
                [doc(1).to_bytes()],
            )
            .unwrap();
        let err = db.sync_pending_take_for_doc(doc(1)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        assert_eq!(
            count(db.connection(), "sync_pending"),
            2,
            "nothing was deleted"
        );
    }

    #[test]
    fn errors_never_contain_blob_contents() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO sync_heads VALUES (zeroblob(16), zeroblob(16), 1, x'DEADBEEF', 0, x'C0FFEE', 0)",
                [],
            )
            .unwrap();
        let text = db.sync_heads_all().unwrap_err().to_string().to_lowercase();
        assert!(
            !text.contains("deadbeef") && !text.contains("c0ffee"),
            "{text}"
        );
    }

    // ---- keyring published ----------------------------------------------------------

    #[test]
    fn a_new_keyring_row_is_unpublished_and_marking_is_by_content() {
        on_db_and_tx(|q| {
            assert!(q.keyring_unpublished().unwrap().is_empty());
            let a = key_row(2, b"a", b"record a", false);
            let b = key_row(1, b"b", b"record b", false);
            let c = key_row(2, b"\x00\xFF", b"tombstone c", true);
            for row in [&a, &b, &c] {
                q.keyring_put(row).unwrap();
            }
            // Ordered like keyring_all: space, then subject bytes.
            assert_eq!(
                q.keyring_unpublished().unwrap(),
                [b.clone(), c.clone(), a.clone()]
            );
            assert_eq!(q.keyring_unpublished().unwrap(), q.keyring_all().unwrap());

            q.keyring_mark_published(&[]).unwrap();
            assert_eq!(q.keyring_unpublished().unwrap().len(), 3);

            q.keyring_mark_published(std::slice::from_ref(&c)).unwrap();
            assert_eq!(q.keyring_unpublished().unwrap(), [b.clone(), a.clone()]);

            // Marking an unknown row or one already marked changes nothing.
            q.keyring_mark_published(&[key_row(9, b"x", b"y", false), c.clone()])
                .unwrap();
            assert_eq!(q.keyring_unpublished().unwrap(), [b.clone(), a.clone()]);

            q.keyring_mark_published(&[a.clone(), b.clone()]).unwrap();
            assert!(q.keyring_unpublished().unwrap().is_empty());
            assert_eq!(
                q.keyring_all().unwrap().len(),
                3,
                "the rows are all still there"
            );

            q.keyring_mark_all_unpublished().unwrap();
            assert_eq!(q.keyring_unpublished().unwrap().len(), 3);
            q.keyring_mark_all_unpublished().unwrap();
        });
    }

    #[test]
    fn a_row_that_changed_since_it_was_read_stays_unpublished() {
        on_db_and_tx(|q| {
            let record = key_row(1, b"s", b"the record", false);
            q.keyring_put(&record).unwrap();
            let read = q.keyring_unpublished().unwrap();

            // Between the read and the mark a tombstone replaces the record.
            let tombstone = key_row(1, b"s", b"the tombstone", true);
            q.keyring_put(&tombstone).unwrap();
            q.keyring_mark_published(&read).unwrap();
            assert_eq!(
                q.keyring_unpublished().unwrap(),
                std::slice::from_ref(&tombstone),
                "the newer row was not in the push"
            );

            // The same record bytes with a different flag are another row too.
            let flagged = key_row(1, b"s", b"the tombstone", false);
            q.keyring_mark_published(&[flagged]).unwrap();
            assert_eq!(
                q.keyring_unpublished().unwrap(),
                std::slice::from_ref(&tombstone)
            );

            q.keyring_mark_published(&[tombstone]).unwrap();
            assert!(q.keyring_unpublished().unwrap().is_empty());
        });
    }

    #[test]
    fn a_keyring_put_is_unpublished_even_when_it_writes_the_same_row() {
        on_db_and_tx(|q| {
            let row = key_row(1, b"s", b"record", false);
            q.keyring_put(&row).unwrap();
            q.keyring_mark_published(std::slice::from_ref(&row))
                .unwrap();
            assert!(q.keyring_unpublished().unwrap().is_empty());

            // `keyring_put` is a plain replace: whatever it writes is unpublished. A caller
            // that stores a row it received puts and marks in one transaction.
            q.keyring_put(&row).unwrap();
            assert_eq!(q.keyring_unpublished().unwrap(), std::slice::from_ref(&row));
            q.keyring_mark_published(std::slice::from_ref(&row))
                .unwrap();
            assert!(q.keyring_unpublished().unwrap().is_empty());
        });
    }

    #[test]
    fn keyring_rows_are_removed_with_their_space_whatever_their_marker() {
        on_db_and_tx(|q| {
            let (a, b) = (key_row(1, b"a", b"1", false), key_row(2, b"b", b"2", false));
            q.keyring_put(&a).unwrap();
            q.keyring_put(&b).unwrap();
            q.keyring_mark_published(std::slice::from_ref(&a)).unwrap();
            q.keyring_delete_space(space(1)).unwrap();
            assert_eq!(q.keyring_unpublished().unwrap(), [b]);
        });
    }

    // ---- device log published -------------------------------------------------------

    #[test]
    fn device_log_entries_start_unpublished_and_are_marked_by_prefix() {
        on_db_and_tx(|q| {
            assert!(q.device_log_unpublished().unwrap().is_empty());
            q.device_log_mark_published(5).unwrap(); // nothing to mark
            for seq in 0..4 {
                q.device_log_append(&log_row(seq)).unwrap();
            }
            assert_eq!(
                q.device_log_unpublished().unwrap(),
                (0..4).map(log_row).collect::<Vec<_>>()
            );

            q.device_log_mark_published(1).unwrap();
            assert_eq!(
                q.device_log_unpublished().unwrap(),
                [log_row(2), log_row(3)]
            );
            q.device_log_mark_published(0).unwrap(); // already marked: no change
            assert_eq!(q.device_log_unpublished().unwrap().len(), 2);

            // An appended entry starts unpublished, after the marked ones.
            q.device_log_append(&log_row(4)).unwrap();
            assert_eq!(
                q.device_log_unpublished().unwrap(),
                [log_row(2), log_row(3), log_row(4)]
            );

            // Beyond the last entry marks them all.
            q.device_log_mark_published(100).unwrap();
            assert!(q.device_log_unpublished().unwrap().is_empty());
            assert_eq!(q.device_log_all().unwrap().len(), 5, "no entry was removed");

            q.device_log_mark_all_unpublished().unwrap();
            assert_eq!(q.device_log_unpublished().unwrap().len(), 5);
            assert_eq!(
                q.device_log_unpublished().unwrap(),
                q.device_log_all().unwrap()
            );
            q.device_log_mark_all_unpublished().unwrap();
        });
    }

    #[test]
    fn a_log_sequence_above_i64_max_cannot_be_marked() {
        on_db_and_tx(|q| {
            q.device_log_append(&log_row(0)).unwrap();
            assert_invalid(
                q.device_log_mark_published(u64::MAX).unwrap_err(),
                "out of range",
            );
            assert_eq!(q.device_log_unpublished().unwrap().len(), 1);
        });
    }

    // ---- the outbox -----------------------------------------------------------------

    fn append(q: &dyn Queries, doc_n: u8, blob: &[u8], origin: UpdateOrigin) -> i64 {
        q.update_append(doc(doc_n), blob, origin, 1).unwrap()
    }

    #[test]
    fn updates_from_reads_the_log_after_a_sequence_number_with_every_origin() {
        on_db_and_tx(|q| {
            q.doc_insert(&doc_row(1)).unwrap();
            q.doc_insert(&doc_row(2)).unwrap();
            assert!(q.updates_from(doc(1), 0).unwrap().is_empty());
            assert!(
                q.updates_from(doc(77), 0).unwrap().is_empty(),
                "unknown document"
            );

            let origins = [
                UpdateOrigin::Local,
                UpdateOrigin::Remote,
                UpdateOrigin::Core,
                UpdateOrigin::Local,
            ];
            for (i, origin) in origins.into_iter().enumerate() {
                let seq = append(q, 1, &blob(5 + i, u8::try_from(i).unwrap()), origin);
                assert_eq!(seq, i64::try_from(i).unwrap() + 1);
            }
            append(q, 2, b"other", UpdateOrigin::Local);

            let expected: Vec<(i64, Vec<u8>, UpdateOrigin)> = origins
                .into_iter()
                .enumerate()
                .map(|(i, origin)| {
                    (
                        i64::try_from(i).unwrap() + 1,
                        blob(5 + i, u8::try_from(i).unwrap()),
                        origin,
                    )
                })
                .collect();
            assert_eq!(q.updates_from(doc(1), 0).unwrap(), expected);
            assert_eq!(
                q.updates_from(doc(1), -5).unwrap(),
                expected,
                "below the log"
            );
            assert_eq!(
                q.updates_from(doc(1), 1).unwrap(),
                expected[1..],
                "exclusive"
            );
            assert_eq!(q.updates_from(doc(1), 3).unwrap(), expected[3..]);
            assert!(q.updates_from(doc(1), 4).unwrap().is_empty());
            assert!(q.updates_from(doc(1), i64::MAX).unwrap().is_empty());
            assert_eq!(
                q.updates_from(doc(2), 0).unwrap(),
                [(1, b"other".to_vec(), UpdateOrigin::Local)],
                "only the document's own updates"
            );
        });
    }

    #[test]
    fn updates_from_follows_a_compaction() {
        on_db_and_tx(|q| {
            q.doc_insert(&doc_row(1)).unwrap();
            for i in 0..5u8 {
                append(q, 1, &[i, 0, 0xFF], UpdateOrigin::Local);
            }
            q.updates_delete_up_to(doc(1), 3).unwrap();
            let seqs: Vec<i64> = q
                .updates_from(doc(1), 0)
                .unwrap()
                .into_iter()
                .map(|(seq, _, _)| seq)
                .collect();
            assert_eq!(seqs, [4, 5], "the covered rows are gone");
            assert_eq!(
                append(q, 1, b"x", UpdateOrigin::Core),
                6,
                "numbering goes on"
            );
            assert_eq!(
                q.updates_from(doc(1), 5).unwrap(),
                [(6, b"x".to_vec(), UpdateOrigin::Core)]
            );
        });
    }

    #[test]
    fn an_update_with_an_unknown_origin_is_invalid_data() {
        let db = new_db();
        db.doc_insert(&doc_row(1)).unwrap();
        db.connection()
            .execute(
                "INSERT INTO doc_updates VALUES (?1, 1, x'01', 9, 0)",
                [doc(1).to_bytes()],
            )
            .unwrap();
        assert_invalid(
            db.updates_from(doc(1), 0).unwrap_err(),
            "unknown update origin",
        );
    }

    // ---- transactions ---------------------------------------------------------------

    /// Writes one row of every kind through `q`.
    fn write_everything(q: &dyn Queries) {
        q.sync_config_set("server_url", "https://x.example")
            .unwrap();
        q.sync_doc_state_put(&state(1)).unwrap();
        q.sync_seen_put(doc(1), device(1), 3).unwrap();
        q.sync_heads_put(&head(1)).unwrap();
        q.sync_pending_put(&pending(7, 1, 1)).unwrap();
        q.keyring_put(&key_row(1, b"s", b"r", false)).unwrap();
        q.device_log_append(&log_row(0)).unwrap();
    }

    #[test]
    fn writes_commit_with_the_transaction_and_reads_see_them_before_that() {
        let mut db = new_db();
        db.transaction(|tx| {
            write_everything(tx);
            // The transaction reads its own writes.
            assert_eq!(tx.sync_pending_count()?, 1);
            assert_eq!(tx.sync_config_all()?.len(), 1);
            assert_eq!(tx.keyring_unpublished()?.len(), 1);
            assert_eq!(tx.device_log_unpublished()?.len(), 1);
            tx.keyring_mark_published(&tx.keyring_unpublished()?)?;
            tx.device_log_mark_published(0)
        })
        .unwrap();

        assert!(db.connection().is_autocommit(), "the transaction is over");
        assert_eq!(db.sync_doc_state_all().unwrap(), [state(1)]);
        assert_eq!(db.sync_seen_get(doc(1), device(1)).unwrap(), Some(3));
        assert_eq!(db.sync_heads_all().unwrap(), [head(1)]);
        assert_eq!(db.sync_pending_all().unwrap(), [pending(7, 1, 1)]);
        assert!(db.keyring_unpublished().unwrap().is_empty());
        assert!(db.device_log_unpublished().unwrap().is_empty());
    }

    #[test]
    fn writes_roll_back_with_a_transaction_that_fails() {
        let mut db = new_db();
        // Some committed state first.
        write_everything(&db);
        let keyring = db.keyring_all().unwrap();
        db.keyring_mark_published(&keyring).unwrap();
        db.device_log_mark_published(0).unwrap();

        let err = db
            .transaction(|tx| {
                tx.sync_config_set("server_url", "https://changed.example")?;
                tx.sync_config_set("more", "x")?;
                tx.sync_doc_state_put(&state(2))?;
                tx.sync_doc_state_reset_all()?;
                tx.sync_seen_put(doc(1), device(1), 99)?;
                tx.sync_heads_put(&head(2))?;
                tx.sync_heads_clear()?;
                tx.sync_pending_put(&pending(8, 1, 2))?;
                assert_eq!(tx.sync_pending_take_for_doc(doc(1))?.len(), 2);
                tx.keyring_mark_all_unpublished()?;
                tx.device_log_mark_all_unpublished()?;
                tx.sync_seen_clear()?;
                Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m == "boom"),
            "{err}"
        );

        assert_eq!(
            db.sync_config_all().unwrap(),
            [pair("server_url", "https://x.example")]
        );
        assert_eq!(db.sync_doc_state_all().unwrap(), [state(1)]);
        assert_eq!(db.sync_seen_get(doc(1), device(1)).unwrap(), Some(3));
        assert_eq!(db.sync_heads_all().unwrap(), [head(1)]);
        assert_eq!(db.sync_pending_all().unwrap(), [pending(7, 1, 1)]);
        assert!(db.keyring_unpublished().unwrap().is_empty());
        assert!(db.device_log_unpublished().unwrap().is_empty());
        assert!(db.connection().is_autocommit(), "the transaction is over");

        // The connection is fine afterwards.
        db.transaction(|tx| tx.sync_config_set("after", "1"))
            .unwrap();
        assert_eq!(db.sync_config_all().unwrap().len(), 2);
    }

    #[test]
    fn a_panic_inside_the_transaction_rolls_the_sync_writes_back() {
        let mut db = new_db();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = db.transaction::<()>(|tx| {
                write_everything(tx);
                panic!("boom");
            });
        }));
        assert!(outcome.is_err());
        assert!(db.sync_config_all().unwrap().is_empty());
        assert!(db.sync_doc_state_all().unwrap().is_empty());
        assert!(db.sync_heads_all().unwrap().is_empty());
        assert_eq!(db.sync_pending_count().unwrap(), 0);
        assert!(db.keyring_all().unwrap().is_empty());
        assert!(db.device_log_all().unwrap().is_empty());
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_bare_db_takes_the_pending_entries_of_a_document_all_or_nothing() {
        let db = new_db();
        for cursor in [1, 2, 3] {
            db.sync_pending_put(&pending(cursor, 1, 1)).unwrap();
        }
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE DELETE ON sync_pending
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db.sync_pending_take_for_doc(doc(1)).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(db.sync_pending_count().unwrap(), 3, "nothing was taken");
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_bare_db_marks_several_keyring_rows_all_or_nothing() {
        let db = new_db();
        let rows: Vec<KeyringRow> = (1..=3u8).map(|n| key_row(n, b"s", &[n], false)).collect();
        for row in &rows {
            db.keyring_put(row).unwrap();
        }
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE UPDATE ON keyring
                 WHEN OLD.record = x'03'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db.keyring_mark_published(&rows).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            db.keyring_unpublished().unwrap().len(),
            3,
            "rows 1 and 2 were marked first and are back"
        );
        assert!(db.connection().is_autocommit());
    }

    // ---- query plans ----------------------------------------------------------------

    /// The query plan SQLite reports for a statement, as one string, with every parameter
    /// bound to a dummy value (`EXPLAIN` does not run the statement).
    fn plan_of(sql: &str) -> String {
        let db = new_db();
        let mut stmt = db
            .connection()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let dummies = vec![[1u8; 16]; stmt.parameter_count()];
        let rows: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(dummies.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.join(" | ")
    }

    #[test]
    fn the_statements_use_the_primary_keys() {
        let outbox = plan_of(UPDATES_FROM_SQL);
        assert!(
            outbox.contains("(doc_id=? AND local_seq>?)"),
            "the outbox reads a range of the primary key: {outbox}"
        );
        assert!(!outbox.contains("SCAN"), "{outbox}");

        let mark = plan_of(KEYRING_MARK_SQL);
        assert!(
            mark.contains("(space_id=? AND subject=?)"),
            "marking goes by primary key: {mark}"
        );
        for sql in [DOC_STATE_PUT_SQL, SEEN_GET_SQL, CONFIG_GET_SQL] {
            let plan = plan_of(sql);
            assert!(!plan.contains("SCAN"), "{plan} for {sql}");
        }
    }

    // ---- feeds (schema v17) ---------------------------------------------------------

    const ACCOUNT_TEXT: &str = "123e4567-e89b-42d3-a456-426614174000";

    /// The personal feed of [`ACCOUNT_TEXT`]: the same 16 bytes.
    fn personal() -> FeedId {
        FeedId::from_bytes([
            0x12, 0x3e, 0x45, 0x67, 0xe8, 0x9b, 0x42, 0xd3, 0xa4, 0x56, 0x42, 0x66, 0x14, 0x17,
            0x40, 0x00,
        ])
    }

    const fn feed(n: u8) -> FeedId {
        FeedId::from_bytes([n; 16])
    }

    /// Gives the database its account, so that the personal feed is `personal()`.
    fn with_account(q: &dyn Queries) {
        q.meta_set("account_id", ACCOUNT_TEXT).unwrap();
    }

    #[test]
    fn the_personal_feed_is_the_account_id_and_the_zero_feed_while_there_is_none() {
        on_db_and_tx(|q| {
            assert_eq!(q.sync_personal_feed().unwrap(), UNRESOLVED_PERSONAL_FEED);
            assert_eq!(UNRESOLVED_PERSONAL_FEED, FeedId::from_bytes([0; 16]));
            with_account(q);
            assert_eq!(q.sync_personal_feed().unwrap(), personal());
            assert_eq!(
                q.sync_personal_feed().unwrap(),
                FeedId::from_account(ACCOUNT_TEXT.parse::<kb_types::AccountId>().unwrap())
            );
            // The account id may change (a restore): the personal feed follows it.
            q.meta_set("account_id", "00000000-0000-4000-8000-000000000001")
                .unwrap();
            assert_ne!(q.sync_personal_feed().unwrap(), personal());
        });
    }

    #[test]
    fn a_stored_account_id_that_is_not_an_id_is_invalid_data() {
        let db = new_db();
        db.meta_set("account_id", "not an account id").unwrap();
        assert_invalid(db.sync_personal_feed().unwrap_err(), "account id");
        // The forms without a feed need it, and refuse rather than guess.
        assert_invalid(db.sync_doc_state_all().unwrap_err(), "account id");
        assert_invalid(db.sync_heads_put(&head(1)).unwrap_err(), "account id");
        // The forms that name the feed do not.
        assert!(db.sync_doc_state_all_in(feed(1)).unwrap().is_empty());
    }

    #[test]
    fn the_forms_without_a_feed_are_the_forms_of_the_personal_feed() {
        on_db_and_tx(|q| {
            with_account(q);
            // Written without a feed, read with it.
            q.sync_doc_state_put(&state(1)).unwrap();
            q.sync_seen_put(doc(1), device(1), 5).unwrap();
            q.sync_heads_put(&head(1)).unwrap();
            q.sync_pending_put(&pending(7, 1, 1)).unwrap();
            assert_eq!(
                q.sync_doc_state_get_in(personal(), doc(1)).unwrap(),
                Some(state(1))
            );
            assert_eq!(q.sync_doc_state_all_in(personal()).unwrap(), [state(1)]);
            assert_eq!(
                q.sync_seen_get_in(personal(), doc(1), device(1)).unwrap(),
                Some(5)
            );
            assert_eq!(q.sync_heads_all_in(personal()).unwrap(), [head(1)]);
            assert_eq!(
                q.sync_pending_all_in(personal()).unwrap(),
                [pending(7, 1, 1)]
            );
            assert_eq!(q.sync_pending_count_in(personal()).unwrap(), 1);
            // Written with the feed, read without it.
            q.sync_doc_state_put_in(personal(), &state(2)).unwrap();
            q.sync_seen_put_in(personal(), doc(2), device(2), 6)
                .unwrap();
            q.sync_heads_put_in(personal(), &head(2)).unwrap();
            q.sync_pending_put_in(personal(), &pending(8, 2, 1))
                .unwrap();
            assert_eq!(q.sync_doc_state_all().unwrap(), [state(1), state(2)]);
            assert_eq!(q.sync_seen_get(doc(2), device(2)).unwrap(), Some(6));
            assert_eq!(q.sync_heads_all().unwrap(), [head(1), head(2)]);
            assert_eq!(q.sync_pending_count().unwrap(), 2);
            // Another feed sees none of it.
            assert!(q.sync_doc_state_all_in(feed(1)).unwrap().is_empty());
            assert_eq!(
                q.sync_seen_get_in(feed(1), doc(1), device(1)).unwrap(),
                None
            );
            assert!(q.sync_heads_all_in(feed(1)).unwrap().is_empty());
            assert_eq!(q.sync_pending_count_in(feed(1)).unwrap(), 0);
            // Deleting without a feed deletes in the personal one.
            q.sync_heads_delete(device(1)).unwrap();
            q.sync_pending_delete(7, doc(1)).unwrap();
            assert_eq!(q.sync_heads_all().unwrap(), [head(2)]);
            assert_eq!(q.sync_pending_all().unwrap(), [pending(8, 2, 1)]);
            assert_eq!(
                q.sync_pending_take_for_doc(doc(2)).unwrap(),
                [pending(8, 2, 1)]
            );
        });
    }

    #[test]
    fn rows_written_before_there_was_an_account_belong_to_the_zero_feed() {
        let db = new_db();
        db.sync_doc_state_put(&state(1)).unwrap();
        assert_eq!(
            db.sync_doc_state_get_in(UNRESOLVED_PERSONAL_FEED, doc(1))
                .unwrap(),
            Some(state(1))
        );
        // Once the account exists the personal feed is another feed: those rows are not its own.
        with_account(&db);
        assert!(db.sync_doc_state_all().unwrap().is_empty());
        assert_eq!(
            db.sync_doc_state_all_in(UNRESOLVED_PERSONAL_FEED)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn every_feed_has_its_own_document_states_and_the_keyring_exists_once_per_feed() {
        on_db_and_tx(|q| {
            let keyring = DocId::from_bytes([0; 16]);
            let in_a = SyncDocState {
                doc_id: keyring,
                ..state(5)
            };
            let in_b = SyncDocState {
                doc_id: keyring,
                pushed_up_to_seq: 99,
                ..state(6)
            };
            q.sync_doc_state_put_in(feed(1), &in_a).unwrap();
            q.sync_doc_state_put_in(feed(2), &in_b).unwrap();
            q.sync_doc_state_put_in(feed(1), &state(3)).unwrap();
            q.sync_doc_state_put_in(feed(2), &state(3)).unwrap();
            q.sync_doc_state_put_in(feed(2), &state(4)).unwrap();

            assert_eq!(
                q.sync_doc_state_get_in(feed(1), keyring).unwrap(),
                Some(in_a.clone())
            );
            assert_eq!(
                q.sync_doc_state_get_in(feed(2), keyring).unwrap(),
                Some(in_b.clone())
            );
            assert_eq!(q.sync_doc_state_get_in(feed(3), keyring).unwrap(), None);
            assert_eq!(q.sync_doc_state_all_in(feed(1)).unwrap(), [in_a, state(3)]);
            assert_eq!(
                q.sync_doc_state_all_in(feed(2)).unwrap(),
                [in_b, state(3), state(4)]
            );

            // A put replaces the row of that feed only.
            let moved = SyncDocState {
                next_device_seq: 77,
                ..state(3)
            };
            q.sync_doc_state_put_in(feed(1), &moved).unwrap();
            assert_eq!(
                q.sync_doc_state_get_in(feed(1), doc(3)).unwrap(),
                Some(moved)
            );
            assert_eq!(
                q.sync_doc_state_get_in(feed(2), doc(3)).unwrap(),
                Some(state(3))
            );
            assert_invalid(
                q.sync_doc_state_put_in(
                    feed(1),
                    &SyncDocState {
                        next_device_seq: u64::MAX,
                        ..state(1)
                    },
                )
                .unwrap_err(),
                "out of range",
            );
        });
    }

    #[test]
    fn a_feed_is_reset_alone_or_every_feed_at_once() {
        on_db_and_tx(|q| {
            for f in [1, 2] {
                for n in 1..=2 {
                    q.sync_doc_state_put_in(feed(f), &state(n)).unwrap();
                }
            }
            let pushed = |q: &dyn Queries, f: u8| -> Vec<i64> {
                q.sync_doc_state_all_in(feed(f))
                    .unwrap()
                    .iter()
                    .map(|row| row.pushed_up_to_seq)
                    .collect()
            };
            assert_eq!(pushed(q, 1), [10, 20]);

            q.sync_doc_state_reset_in(feed(1)).unwrap();
            assert_eq!(pushed(q, 1), [0, 0]);
            assert_eq!(pushed(q, 2), [10, 20], "the other feed is untouched");
            for row in q.sync_doc_state_all_in(feed(1)).unwrap() {
                let n = row.doc_id.to_bytes()[0];
                assert_eq!(row.next_device_seq, state(n).next_device_seq, "kept");
                assert_eq!(row.snapshot_covers, state(n).snapshot_covers, "kept");
                assert_eq!(row.remote_snapshot_covers, None);
                assert_eq!(row.entries_since_snapshot, 0);
            }
            q.sync_doc_state_reset_in(feed(9)).unwrap();

            // A new server epoch resets every feed.
            q.sync_doc_state_reset_all().unwrap();
            assert_eq!(pushed(q, 2), [0, 0]);
            assert_eq!(
                q.sync_doc_state_all_in(feed(2)).unwrap().len(),
                2,
                "no row is removed"
            );
        });
    }

    #[test]
    fn every_feed_has_its_own_continuity() {
        on_db_and_tx(|q| {
            q.sync_seen_put_in(feed(1), doc(1), device(1), 5).unwrap();
            q.sync_seen_put_in(feed(2), doc(1), device(1), 9).unwrap();
            q.sync_seen_put_in(feed(2), doc(2), device(1), 3).unwrap();
            assert_eq!(
                q.sync_seen_get_in(feed(1), doc(1), device(1)).unwrap(),
                Some(5)
            );
            assert_eq!(
                q.sync_seen_get_in(feed(2), doc(1), device(1)).unwrap(),
                Some(9)
            );
            assert_eq!(
                q.sync_seen_get_in(feed(1), doc(2), device(1)).unwrap(),
                None
            );

            q.sync_seen_put_in(feed(1), doc(1), device(1), 0).unwrap();
            assert_eq!(
                q.sync_seen_get_in(feed(1), doc(1), device(1)).unwrap(),
                Some(0)
            );
            assert_eq!(
                q.sync_seen_get_in(feed(2), doc(1), device(1)).unwrap(),
                Some(9)
            );
            assert_invalid(
                q.sync_seen_put_in(feed(1), doc(1), device(1), u64::MAX)
                    .unwrap_err(),
                "out of range",
            );

            q.sync_seen_clear_in(feed(1)).unwrap();
            q.sync_seen_clear_in(feed(9)).unwrap();
            assert_eq!(
                q.sync_seen_get_in(feed(1), doc(1), device(1)).unwrap(),
                None
            );
            assert_eq!(
                q.sync_seen_get_in(feed(2), doc(2), device(1)).unwrap(),
                Some(3)
            );
            q.sync_seen_clear().unwrap();
            assert_eq!(
                q.sync_seen_get_in(feed(2), doc(2), device(1)).unwrap(),
                None
            );
        });
    }

    #[test]
    fn every_feed_has_its_own_head_of_a_device() {
        on_db_and_tx(|q| {
            let other = SyncHeadRow {
                cursor: 999,
                signed: vec![9, 9],
                ..head(1)
            };
            q.sync_heads_put_in(feed(1), &head(1)).unwrap();
            q.sync_heads_put_in(feed(2), &other).unwrap();
            q.sync_heads_put_in(feed(2), &head(2)).unwrap();
            assert_eq!(q.sync_heads_all_in(feed(1)).unwrap(), [head(1)]);
            assert_eq!(
                q.sync_heads_all_in(feed(2)).unwrap(),
                [other.clone(), head(2)],
                "the same device has a head in each feed"
            );

            // A put replaces the head of that device in that feed.
            let newer = SyncHeadRow {
                cursor: 5,
                ..head(1)
            };
            q.sync_heads_put_in(feed(1), &newer).unwrap();
            assert_eq!(q.sync_heads_all_in(feed(1)).unwrap(), [newer]);
            assert_eq!(q.sync_heads_all_in(feed(2)).unwrap(), [other, head(2)]);
            assert_invalid(
                q.sync_heads_put_in(
                    feed(1),
                    &SyncHeadRow {
                        cursor: u64::MAX,
                        ..head(3)
                    },
                )
                .unwrap_err(),
                "out of range",
            );

            q.sync_heads_delete_in(feed(2), device(1)).unwrap();
            q.sync_heads_delete_in(feed(2), device(1)).unwrap();
            assert_eq!(q.sync_heads_all_in(feed(2)).unwrap(), [head(2)]);
            assert_eq!(q.sync_heads_all_in(feed(1)).unwrap().len(), 1);
            q.sync_heads_clear_in(feed(2)).unwrap();
            assert!(q.sync_heads_all_in(feed(2)).unwrap().is_empty());
            assert_eq!(q.sync_heads_all_in(feed(1)).unwrap().len(), 1);
            q.sync_heads_clear().unwrap();
            assert!(q.sync_heads_all_in(feed(1)).unwrap().is_empty());
        });
    }

    #[test]
    fn every_feed_has_its_own_pending_entries() {
        on_db_and_tx(|q| {
            // The same cursor and document in two feeds (two servers' histories, or a move) are
            // two entries.
            q.sync_pending_put_in(feed(1), &pending(10, 1, 1)).unwrap();
            q.sync_pending_put_in(feed(2), &pending(10, 1, 2)).unwrap();
            q.sync_pending_put_in(feed(2), &pending(5, 3, 1)).unwrap();
            q.sync_pending_put_in(feed(1), &pending(20, 1, 1)).unwrap();
            assert_eq!(
                q.sync_pending_all_in(feed(1)).unwrap(),
                [pending(10, 1, 1), pending(20, 1, 1)]
            );
            assert_eq!(
                q.sync_pending_all_in(feed(2)).unwrap(),
                [pending(5, 3, 1), pending(10, 1, 2)]
            );
            assert_eq!(q.sync_pending_count_in(feed(1)).unwrap(), 2);
            assert_eq!(q.sync_pending_count_in(feed(2)).unwrap(), 2);
            assert_eq!(q.sync_pending_count_in(feed(3)).unwrap(), 0);

            // Taking a document's entries takes those of that feed only.
            assert_eq!(
                q.sync_pending_take_for_doc_in(feed(1), doc(1)).unwrap(),
                [pending(10, 1, 1), pending(20, 1, 1)]
            );
            assert!(
                q.sync_pending_take_for_doc_in(feed(1), doc(1))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(q.sync_pending_count_in(feed(2)).unwrap(), 2);
            assert_eq!(
                q.sync_pending_take_for_doc_in(feed(2), doc(1)).unwrap(),
                [pending(10, 1, 2)]
            );

            q.sync_pending_delete_in(feed(2), 5, doc(3)).unwrap();
            q.sync_pending_delete_in(feed(2), 5, doc(3)).unwrap();
            assert_eq!(q.sync_pending_count_in(feed(2)).unwrap(), 0);

            q.sync_pending_put_in(feed(1), &pending(1, 1, 1)).unwrap();
            q.sync_pending_put_in(feed(2), &pending(1, 1, 1)).unwrap();
            q.sync_pending_clear_in(feed(1)).unwrap();
            assert_eq!(q.sync_pending_count_in(feed(1)).unwrap(), 0);
            assert_eq!(q.sync_pending_count_in(feed(2)).unwrap(), 1);
            q.sync_pending_clear().unwrap();
            assert_eq!(q.sync_pending_count_in(feed(2)).unwrap(), 0);
            for cursor in [u64::MAX, u64::try_from(i64::MAX).unwrap() + 1] {
                let row = SyncPendingRow {
                    cursor,
                    ..pending(1, 1, 1)
                };
                assert_invalid(
                    q.sync_pending_put_in(feed(1), &row).unwrap_err(),
                    "out of range",
                );
                assert_invalid(
                    q.sync_pending_delete_in(feed(1), cursor, doc(1))
                        .unwrap_err(),
                    "out of range",
                );
            }
        });
    }

    #[test]
    fn a_bare_db_takes_the_pending_entries_of_a_feed_all_or_nothing() {
        let db = new_db();
        for f in [1, 2] {
            for cursor in [1, 2] {
                db.sync_pending_put_in(feed(f), &pending(cursor, 1, 1))
                    .unwrap();
            }
        }
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE DELETE ON sync_pending
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db
            .sync_pending_take_for_doc_in(feed(1), doc(1))
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(db.sync_pending_count_in(feed(1)).unwrap(), 2);
        assert_eq!(db.sync_pending_count_in(feed(2)).unwrap(), 2);
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn feed_writes_roll_back_with_a_transaction_that_fails() {
        let mut db = new_db();
        db.sync_doc_state_put_in(feed(1), &state(1)).unwrap();
        db.sync_heads_put_in(feed(1), &head(1)).unwrap();
        let err = db
            .transaction(|tx| {
                tx.sync_doc_state_put_in(feed(2), &state(2))?;
                tx.sync_doc_state_reset_in(feed(1))?;
                tx.sync_seen_put_in(feed(2), doc(2), device(2), 1)?;
                tx.sync_heads_clear()?;
                tx.sync_pending_put_in(feed(2), &pending(1, 1, 1))?;
                Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m == "boom"),
            "{err}"
        );
        assert_eq!(db.sync_doc_state_all_in(feed(1)).unwrap(), [state(1)]);
        assert!(db.sync_doc_state_all_in(feed(2)).unwrap().is_empty());
        assert_eq!(db.sync_heads_all_in(feed(1)).unwrap(), [head(1)]);
        assert_eq!(db.sync_pending_count_in(feed(2)).unwrap(), 0);
    }

    #[test]
    fn the_feed_statements_use_the_primary_keys() {
        for sql in [
            DOC_STATE_GET_SQL,
            DOC_STATE_ALL_SQL,
            DOC_STATE_RESET_IN_SQL,
            SEEN_GET_SQL,
            HEAD_ALL_SQL,
            PENDING_ALL_SQL,
            PENDING_OF_DOC_SQL,
            PENDING_DELETE_OF_DOC_SQL,
        ] {
            let plan = plan_of(sql);
            assert!(!plan.contains("SCAN"), "{plan} for {sql}");
            assert!(
                plan.contains("feed=?"),
                "the feed is the key's first column: {plan}"
            );
        }
    }
}
