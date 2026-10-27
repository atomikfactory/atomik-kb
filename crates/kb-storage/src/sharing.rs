//! The local tables of shared spaces (schema v17, M13 slice 1, ADR-0043 to ADR-0045,
//! `docs/development/milestone-13.md` section 1.4): the feeds this device syncs, the device logs of
//! co-members, the quarantine of items a role does not allow, the end version vectors that snapshot
//! dominance checks against, and the membership log of each shared feed with the cache of its
//! fold. Local bookkeeping like the sync tables of [`crate::sync`]: not part of the account, never
//! backed up, rebuilt or reset by the core. Blobs (log entries, membership entries and states,
//! quarantined items) are opaque here: `kb-keys` and `kb-sync` encode and verify them.
//!
//! Same conventions as [`crate::sync`]: the methods exist on [`Db`] and [`Tx`] through
//! [`FeedQueries`]; rows are read as raw column values and validated afterwards, so a damaged row is
//! a typed `InvalidData` error; blob contents never appear in an error. The per-feed forms of the v6
//! sync tables (`sync_heads`, `sync_doc_state`, `sync_seen`, `sync_pending`) are the `..._in`
//! methods of [`crate::SyncQueries`].
//!
//! # The quarantine's bound
//!
//! `sync_rejected` keeps at most [`REJECTED_MAX_ROWS_PER_FEED`] items and [`REJECTED_MAX_BYTES_PER_FEED`]
//! bytes of them **per feed**. A put that goes past either drops the oldest rows of that feed (by
//! `at`, then cursor) until both hold again, and counts each dropped row for its writer in
//! `sync_rejected_dropped`, so [`FeedQueries::rejected_count_by_account`] keeps counting what is no
//! longer stored. The item just put is never the one dropped, however large it is or however old
//! its `at`: a hostile writer cannot fill the table with refused items and then hide the newest.
//!
//! # Pending membership entries
//!
//! A membership entry the owner appended locally and the server has not accepted yet is stored with
//! the cursor `0` ([`MEMBERSHIP_PENDING_CURSOR`]; feed cursors start at 1). Putting the same entry
//! again with the cursor it was found at fills the cursor in; a different entry from the server at
//! the `seq` of a pending one (a lost compare-and-swap) replaces it. An entry that was found in the
//! feed (a cursor above 0) is never replaced by a different one: that is a server rewriting history.

use kb_types::sync::FeedKind;
use kb_types::{AccountId, DeviceId, DocId, FeedId, SpaceId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::docs::{atomically, id_from};
use crate::sync::{count_from, hash_from, int_from};
use crate::{Db, StorageError, Tx};

/// Most items the quarantine of one feed keeps (see the module documentation).
pub const REJECTED_MAX_ROWS_PER_FEED: usize = 200;

/// Most bytes of items the quarantine of one feed keeps (see the module documentation).
pub const REJECTED_MAX_BYTES_PER_FEED: usize = 8 * 1024 * 1024;

/// The cursor of a membership entry that is stored but was not found in the feed yet.
pub const MEMBERSHIP_PENDING_CURSOR: u64 = 0;

/// A `sync_feeds` row: this device's view of one feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFeedRow {
    /// The feed: the account id for the personal feed.
    pub feed: FeedId,
    /// Personal or shared.
    pub kind: FeedKind,
    /// The space of a shared feed; `None` for the personal feed.
    pub space_id: Option<SpaceId>,
    /// The last feed cursor this device applied.
    pub cursor: u64,
    /// The chain hash at that cursor.
    pub chain_hash: Option<[u8; 32]>,
    /// The highest `covers_cursor` of a snapshot this device verified or made (what pruning may
    /// have removed below).
    pub max_covers: u64,
    /// The bounded `(cursor, chain hash)` pairs of v1, opaque (encoded by the core).
    pub chain_history: Option<Vec<u8>>,
    /// The server's history of this feed does not match this device's: uploads are paused.
    pub fork: bool,
    /// The server epoch the cursor belongs to.
    pub server_epoch: Option<[u8; 16]>,
}

/// A `foreign_device_logs` row: one encoded device log entry of a co-member's account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignLogRow {
    /// The account the log belongs to.
    pub account_id: AccountId,
    /// The entry's sequence number in that log (0 for the genesis).
    pub seq: u64,
    /// `LogEntry::encode`, exactly as served.
    pub entry: Vec<u8>,
    /// When this device fetched it, Unix ms.
    pub fetched_at: i64,
}

/// Why an item was quarantined (the `reason` column of `sync_rejected`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The writer was not a member at the item's cursor (a misbehaving server).
    NotAMember,
    /// The writer's role does not allow the item.
    RoleForbids,
    /// A snapshot that does not dominate the entries it claims to cover.
    SnapshotNotDominant,
}

impl RejectReason {
    /// The stored value: 1, 2 or 3. On disk: never renumber.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::NotAMember => 1,
            Self::RoleForbids => 2,
            Self::SnapshotNotDominant => 3,
        }
    }

    /// Parses [`Self::to_byte`]; any other value is `None`.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::NotAMember),
            2 => Some(Self::RoleForbids),
            3 => Some(Self::SnapshotNotDominant),
            _ => None,
        }
    }
}

/// A `sync_rejected` row: an item that was refused and kept for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedRow {
    /// The feed.
    pub feed: FeedId,
    /// The item's feed cursor.
    pub cursor: u64,
    /// The document; the all-zero id for keyring and membership items.
    pub doc_id: DocId,
    /// The account the item's header names.
    pub account_id: AccountId,
    /// The device that signed it.
    pub device_id: DeviceId,
    /// Why it was refused.
    pub reason: RejectReason,
    /// The item exactly as received.
    pub bytes: Vec<u8>,
    /// When it was refused, Unix ms.
    pub at: i64,
}

/// A `sync_entry_marks` row: where the last applied entry of a writer device left a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMarkRow {
    /// The document.
    pub doc_id: DocId,
    /// The writer device.
    pub device_id: DeviceId,
    /// `kb_doc::update_end_vv` of that entry's update, encoded.
    pub end_vv: Vec<u8>,
    /// The entry's feed cursor.
    pub cursor: u64,
}

/// A `membership_entries` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipEntryRow {
    /// The feed.
    pub feed: FeedId,
    /// The entry's `seq` in the membership log.
    pub seq: u64,
    /// The feed cursor the entry was found at ([`MEMBERSHIP_PENDING_CURSOR`] while it was not).
    pub cursor: u64,
    /// `MembershipEntry::encode`.
    pub bytes: Vec<u8>,
}

/// A `membership_state` row: the cache of the fold of a feed's membership entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipStateRow {
    /// The feed.
    pub feed: FeedId,
    /// The `seq` of the last entry folded.
    pub seq: u64,
    /// That entry's hash (a cache whose head does not match the entries is rebuilt).
    pub head_hash: [u8; 32],
    /// The encoded state (the core's format).
    pub state: Vec<u8>,
}

/// Queries of the shared-space tables, available on a connection or inside a transaction. On a
/// [`Db`] every method is atomic by itself.
pub trait FeedQueries {
    // ---- sync_feeds ----

    /// One feed, `None` when this device does not sync it.
    fn sync_feed_get(&self, feed: FeedId) -> Result<Option<SyncFeedRow>, StorageError>;

    /// Inserts the row or replaces every column of the one with the same feed (an upsert). A
    /// number above `i64::MAX` is `InvalidData` and nothing is written.
    fn sync_feed_put(&self, row: &SyncFeedRow) -> Result<(), StorageError>;

    /// Every feed: the personal feed first, then the shared ones ordered by feed id.
    fn sync_feed_list(&self) -> Result<Vec<SyncFeedRow>, StorageError>;

    /// Forgets a feed in one transaction: its `sync_feeds` row, its membership entries and state, its
    /// quarantine (and the count of what the quarantine dropped), its per-feed sync bookkeeping
    /// (`sync_doc_state`, `sync_seen`, `sync_heads`, `sync_pending`) and the entry marks of the
    /// documents it touched. The co-members' device logs stay (they belong to accounts, not to a
    /// feed). Succeeds when the feed is unknown.
    fn sync_feed_delete(&self, feed: FeedId) -> Result<(), StorageError>;

    // ---- foreign_device_logs ----

    /// Inserts the entry or replaces the stored one with the same `(account, seq)`.
    fn foreign_log_put(&self, row: &ForeignLogRow) -> Result<(), StorageError>;

    /// The account's stored entries with `seq > after` (all when `None`), in `seq` order.
    fn foreign_log_after(
        &self,
        account_id: AccountId,
        after: Option<u64>,
    ) -> Result<Vec<ForeignLogRow>, StorageError>;

    /// The accounts with at least one stored entry, ordered by account id.
    fn foreign_log_accounts(&self) -> Result<Vec<AccountId>, StorageError>;

    // ---- sync_rejected ----

    /// Quarantines an item (an upsert on `(feed, cursor, doc_id)`). The table is bounded per feed
    /// ([`REJECTED_MAX_ROWS_PER_FEED`] rows and [`REJECTED_MAX_BYTES_PER_FEED`] bytes): past the bound
    /// the oldest rows of the feed go, never the one just put, and
    /// [`Self::rejected_count_by_account`] keeps counting what was dropped.
    fn rejected_put(&self, row: &RejectedRow) -> Result<(), StorageError>;

    /// Items quarantined in the feed per writer account, dropped ones included, ordered by account
    /// id (the member panel's "N changes from Bob were refused").
    fn rejected_count_by_account(
        &self,
        feed: FeedId,
    ) -> Result<Vec<(AccountId, u32)>, StorageError>;

    /// At most `limit` quarantined items of the feed, newest first (by `at`, then cursor).
    fn rejected_list(&self, feed: FeedId, limit: u32) -> Result<Vec<RejectedRow>, StorageError>;

    // ---- sync_entry_marks ----

    /// Inserts the mark or replaces the one for the same `(document, device)`.
    fn entry_mark_put(&self, row: &EntryMarkRow) -> Result<(), StorageError>;

    /// The mark of a writer device for a document, `None` when no entry of it was applied.
    fn entry_mark_get(
        &self,
        doc_id: DocId,
        device_id: DeviceId,
    ) -> Result<Option<EntryMarkRow>, StorageError>;

    /// Every mark of a document, ordered by device id: what a snapshot of the document must
    /// dominate up to the cursor it covers (the dominance check filters by `cursor`). Added by the
    /// contract beyond the names of milestone-13.md, 1.4.
    fn entry_marks_for_doc(&self, doc_id: DocId) -> Result<Vec<EntryMarkRow>, StorageError>;

    // ---- membership_entries and membership_state ----

    /// Stores membership entries in one transaction (idempotent on `(feed, seq)`: an entry that is
    /// stored already is kept, a different entry at a stored `seq` is `InvalidData`; see the module
    /// documentation for the entries that are still pending, cursor 0, and nothing of the batch is
    /// stored when one row is refused).
    fn membership_entries_put(&self, rows: &[MembershipEntryRow]) -> Result<(), StorageError>;

    /// The feed's stored entries with `seq > after` (all when `None`), in `seq` order.
    fn membership_entries_list(
        &self,
        feed: FeedId,
        after: Option<u64>,
    ) -> Result<Vec<MembershipEntryRow>, StorageError>;

    /// Deletes the feed's entries that were never found in the feed (cursor 0: the ones appended
    /// locally and not accepted), for a core that rebuilds them after a lost compare-and-swap.
    /// Returns how many went. Added beyond the contract's names.
    fn membership_entries_discard_pending(&self, feed: FeedId) -> Result<u32, StorageError>;

    /// Stores the cache of the fold (an upsert).
    fn membership_state_put(&self, row: &MembershipStateRow) -> Result<(), StorageError>;

    /// The cache of the fold, `None` when there is none.
    fn membership_state_get(
        &self,
        feed: FeedId,
    ) -> Result<Option<MembershipStateRow>, StorageError>;
}

impl FeedQueries for Db {
    fn sync_feed_get(&self, feed: FeedId) -> Result<Option<SyncFeedRow>, StorageError> {
        sync_feed_get_on(&self.conn, feed)
    }
    fn sync_feed_put(&self, row: &SyncFeedRow) -> Result<(), StorageError> {
        sync_feed_put_on(&self.conn, row)
    }
    fn sync_feed_list(&self) -> Result<Vec<SyncFeedRow>, StorageError> {
        sync_feed_list_on(&self.conn)
    }
    fn sync_feed_delete(&self, feed: FeedId) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| sync_feed_delete_on(conn, feed))
    }
    fn foreign_log_put(&self, row: &ForeignLogRow) -> Result<(), StorageError> {
        foreign_log_put_on(&self.conn, row)
    }
    fn foreign_log_after(
        &self,
        account_id: AccountId,
        after: Option<u64>,
    ) -> Result<Vec<ForeignLogRow>, StorageError> {
        foreign_log_after_on(&self.conn, account_id, after)
    }
    fn foreign_log_accounts(&self) -> Result<Vec<AccountId>, StorageError> {
        foreign_log_accounts_on(&self.conn)
    }
    fn rejected_put(&self, row: &RejectedRow) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| rejected_put_on(conn, row))
    }
    fn rejected_count_by_account(
        &self,
        feed: FeedId,
    ) -> Result<Vec<(AccountId, u32)>, StorageError> {
        rejected_count_by_account_on(&self.conn, feed)
    }
    fn rejected_list(&self, feed: FeedId, limit: u32) -> Result<Vec<RejectedRow>, StorageError> {
        rejected_list_on(&self.conn, feed, limit)
    }
    fn entry_mark_put(&self, row: &EntryMarkRow) -> Result<(), StorageError> {
        entry_mark_put_on(&self.conn, row)
    }
    fn entry_mark_get(
        &self,
        doc_id: DocId,
        device_id: DeviceId,
    ) -> Result<Option<EntryMarkRow>, StorageError> {
        entry_mark_get_on(&self.conn, doc_id, device_id)
    }
    fn entry_marks_for_doc(&self, doc_id: DocId) -> Result<Vec<EntryMarkRow>, StorageError> {
        entry_marks_for_doc_on(&self.conn, doc_id)
    }
    fn membership_entries_put(&self, rows: &[MembershipEntryRow]) -> Result<(), StorageError> {
        if rows.len() > 1 {
            atomically(&self.conn, |conn| membership_entries_put_on(conn, rows))
        } else {
            membership_entries_put_on(&self.conn, rows)
        }
    }
    fn membership_entries_list(
        &self,
        feed: FeedId,
        after: Option<u64>,
    ) -> Result<Vec<MembershipEntryRow>, StorageError> {
        membership_entries_list_on(&self.conn, feed, after)
    }
    fn membership_entries_discard_pending(&self, feed: FeedId) -> Result<u32, StorageError> {
        membership_entries_discard_pending_on(&self.conn, feed)
    }
    fn membership_state_put(&self, row: &MembershipStateRow) -> Result<(), StorageError> {
        membership_state_put_on(&self.conn, row)
    }
    fn membership_state_get(
        &self,
        feed: FeedId,
    ) -> Result<Option<MembershipStateRow>, StorageError> {
        membership_state_get_on(&self.conn, feed)
    }
}

impl FeedQueries for Tx<'_> {
    fn sync_feed_get(&self, feed: FeedId) -> Result<Option<SyncFeedRow>, StorageError> {
        sync_feed_get_on(&self.tx, feed)
    }
    fn sync_feed_put(&self, row: &SyncFeedRow) -> Result<(), StorageError> {
        sync_feed_put_on(&self.tx, row)
    }
    fn sync_feed_list(&self) -> Result<Vec<SyncFeedRow>, StorageError> {
        sync_feed_list_on(&self.tx)
    }
    fn sync_feed_delete(&self, feed: FeedId) -> Result<(), StorageError> {
        sync_feed_delete_on(&self.tx, feed)
    }
    fn foreign_log_put(&self, row: &ForeignLogRow) -> Result<(), StorageError> {
        foreign_log_put_on(&self.tx, row)
    }
    fn foreign_log_after(
        &self,
        account_id: AccountId,
        after: Option<u64>,
    ) -> Result<Vec<ForeignLogRow>, StorageError> {
        foreign_log_after_on(&self.tx, account_id, after)
    }
    fn foreign_log_accounts(&self) -> Result<Vec<AccountId>, StorageError> {
        foreign_log_accounts_on(&self.tx)
    }
    fn rejected_put(&self, row: &RejectedRow) -> Result<(), StorageError> {
        rejected_put_on(&self.tx, row)
    }
    fn rejected_count_by_account(
        &self,
        feed: FeedId,
    ) -> Result<Vec<(AccountId, u32)>, StorageError> {
        rejected_count_by_account_on(&self.tx, feed)
    }
    fn rejected_list(&self, feed: FeedId, limit: u32) -> Result<Vec<RejectedRow>, StorageError> {
        rejected_list_on(&self.tx, feed, limit)
    }
    fn entry_mark_put(&self, row: &EntryMarkRow) -> Result<(), StorageError> {
        entry_mark_put_on(&self.tx, row)
    }
    fn entry_mark_get(
        &self,
        doc_id: DocId,
        device_id: DeviceId,
    ) -> Result<Option<EntryMarkRow>, StorageError> {
        entry_mark_get_on(&self.tx, doc_id, device_id)
    }
    fn entry_marks_for_doc(&self, doc_id: DocId) -> Result<Vec<EntryMarkRow>, StorageError> {
        entry_marks_for_doc_on(&self.tx, doc_id)
    }
    fn membership_entries_put(&self, rows: &[MembershipEntryRow]) -> Result<(), StorageError> {
        membership_entries_put_on(&self.tx, rows)
    }
    fn membership_entries_list(
        &self,
        feed: FeedId,
        after: Option<u64>,
    ) -> Result<Vec<MembershipEntryRow>, StorageError> {
        membership_entries_list_on(&self.tx, feed, after)
    }
    fn membership_entries_discard_pending(&self, feed: FeedId) -> Result<u32, StorageError> {
        membership_entries_discard_pending_on(&self.tx, feed)
    }
    fn membership_state_put(&self, row: &MembershipStateRow) -> Result<(), StorageError> {
        membership_state_put_on(&self.tx, row)
    }
    fn membership_state_get(
        &self,
        feed: FeedId,
    ) -> Result<Option<MembershipStateRow>, StorageError> {
        membership_state_get_on(&self.tx, feed)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

const FEED_COLUMNS: &str =
    "feed, kind, space_id, cursor, chain_hash, max_covers, chain_history, fork, server_epoch";
const FEED_GET_SQL: &str = "SELECT feed, kind, space_id, cursor, chain_hash, max_covers, \
     chain_history, fork, server_epoch FROM sync_feeds WHERE feed = ?1";
const FEED_PUT_SQL: &str = "INSERT INTO sync_feeds \
     (feed, kind, space_id, cursor, chain_hash, max_covers, chain_history, fork, server_epoch) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
     ON CONFLICT (feed) DO UPDATE SET kind = excluded.kind, space_id = excluded.space_id, \
       cursor = excluded.cursor, chain_hash = excluded.chain_hash, \
       max_covers = excluded.max_covers, chain_history = excluded.chain_history, \
       fork = excluded.fork, server_epoch = excluded.server_epoch";

const FOREIGN_PUT_SQL: &str = "INSERT INTO foreign_device_logs (account_id, seq, entry, fetched_at) \
     VALUES (?1, ?2, ?3, ?4) \
     ON CONFLICT (account_id, seq) DO UPDATE SET entry = excluded.entry, \
       fetched_at = excluded.fetched_at";
const FOREIGN_AFTER_SQL: &str = "SELECT account_id, seq, entry, fetched_at FROM foreign_device_logs \
     WHERE account_id = ?1 AND seq > ?2 ORDER BY seq";
const FOREIGN_ACCOUNTS_SQL: &str =
    "SELECT DISTINCT account_id FROM foreign_device_logs ORDER BY account_id";

const REJECTED_PUT_SQL: &str = "INSERT INTO sync_rejected \
     (feed, cursor, doc_id, account_id, device_id, reason, bytes, at) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
     ON CONFLICT (feed, cursor, doc_id) DO UPDATE SET account_id = excluded.account_id, \
       device_id = excluded.device_id, reason = excluded.reason, bytes = excluded.bytes, \
       at = excluded.at";
const REJECTED_TOTALS_SQL: &str =
    "SELECT count(*), coalesce(sum(length(bytes)), 0) FROM sync_rejected WHERE feed = ?1";
/// The oldest row of the feed that is not the one just put.
const REJECTED_OLDEST_SQL: &str = "SELECT cursor, doc_id, account_id FROM sync_rejected \
     WHERE feed = ?1 AND NOT (cursor = ?2 AND doc_id = ?3) \
     ORDER BY at, cursor, doc_id LIMIT 1";
const REJECTED_DELETE_SQL: &str =
    "DELETE FROM sync_rejected WHERE feed = ?1 AND cursor = ?2 AND doc_id = ?3";
const REJECTED_DROPPED_SQL: &str = "INSERT INTO sync_rejected_dropped (feed, account_id, dropped) \
     VALUES (?1, ?2, 1) \
     ON CONFLICT (feed, account_id) DO UPDATE SET dropped = dropped + 1";
const REJECTED_COUNTS_SQL: &str = "SELECT account_id, sum(n) FROM ( \
       SELECT account_id, count(*) AS n FROM sync_rejected WHERE feed = ?1 GROUP BY account_id \
       UNION ALL \
       SELECT account_id, dropped AS n FROM sync_rejected_dropped WHERE feed = ?1) \
     GROUP BY account_id ORDER BY account_id";
const REJECTED_LIST_SQL: &str = "SELECT feed, cursor, doc_id, account_id, device_id, reason, \
     bytes, at FROM sync_rejected WHERE feed = ?1 ORDER BY at DESC, cursor DESC, doc_id DESC \
     LIMIT ?2";

const MARK_PUT_SQL: &str = "INSERT INTO sync_entry_marks (doc_id, device_id, end_vv, cursor) \
     VALUES (?1, ?2, ?3, ?4) \
     ON CONFLICT (doc_id, device_id) DO UPDATE SET end_vv = excluded.end_vv, \
       cursor = excluded.cursor";
const MARK_GET_SQL: &str = "SELECT doc_id, device_id, end_vv, cursor FROM sync_entry_marks \
     WHERE doc_id = ?1 AND device_id = ?2";
const MARKS_OF_DOC_SQL: &str = "SELECT doc_id, device_id, end_vv, cursor FROM sync_entry_marks \
     WHERE doc_id = ?1 ORDER BY device_id";

const MEMBERSHIP_GET_SQL: &str = "SELECT bytes, cursor FROM membership_entries \
     WHERE feed = ?1 AND seq = ?2";
const MEMBERSHIP_INSERT_SQL: &str = "INSERT INTO membership_entries (feed, seq, cursor, bytes) \
     VALUES (?1, ?2, ?3, ?4)";
const MEMBERSHIP_REPLACE_SQL: &str = "UPDATE membership_entries SET cursor = ?3, bytes = ?4 \
     WHERE feed = ?1 AND seq = ?2";
const MEMBERSHIP_SET_CURSOR_SQL: &str =
    "UPDATE membership_entries SET cursor = ?3 WHERE feed = ?1 AND seq = ?2";
const MEMBERSHIP_LIST_SQL: &str = "SELECT feed, seq, cursor, bytes FROM membership_entries \
     WHERE feed = ?1 AND seq > ?2 ORDER BY seq";

const MEMBERSHIP_STATE_PUT_SQL: &str = "INSERT INTO membership_state (feed, seq, head_hash, state) \
     VALUES (?1, ?2, ?3, ?4) \
     ON CONFLICT (feed) DO UPDATE SET seq = excluded.seq, head_hash = excluded.head_hash, \
       state = excluded.state";
const MEMBERSHIP_STATE_GET_SQL: &str =
    "SELECT feed, seq, head_hash, state FROM membership_state WHERE feed = ?1";

/// [`MEMBERSHIP_PENDING_CURSOR`] as SQLite stores it (a test holds the two together).
const PENDING_CURSOR: i64 = 0;

fn opt_id_from(value: Option<&[u8]>, what: &str) -> Result<Option<[u8; 16]>, StorageError> {
    value.map(|bytes| id_from(bytes, what)).transpose()
}

// ---- sync_feeds ----

/// Raw column values of a `sync_feeds` row.
struct FeedColumns {
    feed: Vec<u8>,
    kind: i64,
    space_id: Option<Vec<u8>>,
    cursor: i64,
    chain_hash: Option<Vec<u8>>,
    max_covers: i64,
    chain_history: Option<Vec<u8>>,
    fork: i64,
    server_epoch: Option<Vec<u8>>,
}

impl FeedColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            feed: row.get(0)?,
            kind: row.get(1)?,
            space_id: row.get(2)?,
            cursor: row.get(3)?,
            chain_hash: row.get(4)?,
            max_covers: row.get(5)?,
            chain_history: row.get(6)?,
            fork: row.get(7)?,
            server_epoch: row.get(8)?,
        })
    }

    fn decode(self) -> Result<SyncFeedRow, StorageError> {
        Ok(SyncFeedRow {
            feed: FeedId::from_bytes(id_from(&self.feed, "feed id")?),
            kind: u8::try_from(self.kind)
                .ok()
                .and_then(FeedKind::from_byte)
                .ok_or_else(|| StorageError::InvalidData("feed kind is not 1 or 2".to_owned()))?,
            space_id: opt_id_from(self.space_id.as_deref(), "feed space id")?
                .map(SpaceId::from_bytes),
            cursor: count_from(self.cursor, "feed cursor")?,
            chain_hash: self
                .chain_hash
                .as_deref()
                .map(|bytes| hash_from(bytes, "feed chain hash"))
                .transpose()?,
            max_covers: count_from(self.max_covers, "feed snapshot coverage")?,
            chain_history: self.chain_history,
            fork: match self.fork {
                0 => false,
                1 => true,
                _ => {
                    return Err(StorageError::InvalidData(
                        "feed fork flag is neither 0 nor 1".to_owned(),
                    ));
                }
            },
            server_epoch: opt_id_from(self.server_epoch.as_deref(), "feed server epoch")?,
        })
    }
}

fn feeds_from(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<SyncFeedRow>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params, FeedColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(FeedColumns::decode).collect()
}

fn sync_feed_get_on(conn: &Connection, feed: FeedId) -> Result<Option<SyncFeedRow>, StorageError> {
    Ok(feeds_from(conn, FEED_GET_SQL, [feed.to_bytes()])?.pop())
}

fn sync_feed_put_on(conn: &Connection, row: &SyncFeedRow) -> Result<(), StorageError> {
    // Every number is converted before the statement runs: a refusal writes nothing.
    let cursor = int_from(row.cursor, "feed cursor")?;
    let max_covers = int_from(row.max_covers, "feed snapshot coverage")?;
    conn.execute(
        FEED_PUT_SQL,
        params![
            row.feed.to_bytes(),
            i64::from(row.kind.to_byte()),
            row.space_id.map(SpaceId::to_bytes),
            cursor,
            row.chain_hash.as_ref().map(|hash| &hash[..]),
            max_covers,
            row.chain_history,
            i64::from(row.fork),
            row.server_epoch.as_ref().map(|epoch| &epoch[..]),
        ],
    )?;
    Ok(())
}

fn sync_feed_list_on(conn: &Connection) -> Result<Vec<SyncFeedRow>, StorageError> {
    // The personal feed (kind 1) first, then the others by id.
    feeds_from(
        conn,
        &format!("SELECT {FEED_COLUMNS} FROM sync_feeds ORDER BY kind <> 1, feed"),
        [],
    )
}

fn sync_feed_delete_on(conn: &Connection, feed: FeedId) -> Result<(), StorageError> {
    let key = [feed.to_bytes()];
    // The marks of the documents the feed touched go first: the rows that name them go next.
    conn.execute(
        "DELETE FROM sync_entry_marks WHERE doc_id IN ( \
           SELECT doc_id FROM sync_doc_state WHERE feed = ?1 \
           UNION SELECT doc_id FROM sync_seen WHERE feed = ?1)",
        key,
    )?;
    for table in [
        "sync_doc_state",
        "sync_seen",
        "sync_heads",
        "sync_pending",
        "sync_rejected",
        "sync_rejected_dropped",
        "membership_entries",
        "membership_state",
        "sync_feeds",
    ] {
        conn.execute(&format!("DELETE FROM {table} WHERE feed = ?1"), key)?;
    }
    Ok(())
}

// ---- foreign_device_logs ----

fn foreign_log_put_on(conn: &Connection, row: &ForeignLogRow) -> Result<(), StorageError> {
    conn.execute(
        FOREIGN_PUT_SQL,
        params![
            row.account_id.to_bytes(),
            int_from(row.seq, "foreign log sequence number")?,
            row.entry,
            row.fetched_at,
        ],
    )?;
    Ok(())
}

fn foreign_log_after_on(
    conn: &Connection,
    account_id: AccountId,
    after: Option<u64>,
) -> Result<Vec<ForeignLogRow>, StorageError> {
    // `seq > i64::MIN` is every row (a damaged negative one included, which is refused when read).
    let after = after
        .map(|after| int_from(after, "foreign log sequence number"))
        .transpose()?
        .unwrap_or(i64::MIN);
    let mut stmt = conn.prepare(FOREIGN_AFTER_SQL)?;
    let rows = stmt
        .query_map(params![account_id.to_bytes(), after], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(account, seq, entry, fetched_at)| {
            Ok(ForeignLogRow {
                account_id: AccountId::from_bytes(id_from(&account, "foreign log account id")?),
                seq: count_from(seq, "foreign log sequence number")?,
                entry,
                fetched_at,
            })
        })
        .collect()
}

fn foreign_log_accounts_on(conn: &Connection) -> Result<Vec<AccountId>, StorageError> {
    let mut stmt = conn.prepare(FOREIGN_ACCOUNTS_SQL)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|account| {
            Ok(AccountId::from_bytes(id_from(
                &account,
                "foreign log account id",
            )?))
        })
        .collect()
}

// ---- sync_rejected ----

fn rejected_put_on(conn: &Connection, row: &RejectedRow) -> Result<(), StorageError> {
    let cursor = int_from(row.cursor, "quarantined item cursor")?;
    conn.execute(
        REJECTED_PUT_SQL,
        params![
            row.feed.to_bytes(),
            cursor,
            row.doc_id.to_bytes(),
            row.account_id.to_bytes(),
            row.device_id.to_bytes(),
            i64::from(row.reason.to_byte()),
            row.bytes,
            row.at,
        ],
    )?;
    // The bound: drop the oldest rows of the feed (never the one just put) until it holds.
    loop {
        let (count, bytes): (i64, i64) =
            conn.query_row(REJECTED_TOTALS_SQL, [row.feed.to_bytes()], |totals| {
                Ok((totals.get(0)?, totals.get(1)?))
            })?;
        let over_rows =
            usize::try_from(count).is_ok_and(|count| count > REJECTED_MAX_ROWS_PER_FEED);
        let over_bytes =
            usize::try_from(bytes).is_ok_and(|bytes| bytes > REJECTED_MAX_BYTES_PER_FEED);
        if !(over_rows || over_bytes) {
            return Ok(());
        }
        let oldest: Option<(i64, Vec<u8>, Vec<u8>)> = conn
            .query_row(
                REJECTED_OLDEST_SQL,
                params![row.feed.to_bytes(), cursor, row.doc_id.to_bytes()],
                |oldest| Ok((oldest.get(0)?, oldest.get(1)?, oldest.get(2)?)),
            )
            .optional()?;
        let Some((old_cursor, old_doc, old_account)) = oldest else {
            // Only the new item is left: it stays, whatever its size.
            return Ok(());
        };
        conn.execute(
            REJECTED_DELETE_SQL,
            params![row.feed.to_bytes(), old_cursor, old_doc],
        )?;
        conn.execute(
            REJECTED_DROPPED_SQL,
            params![row.feed.to_bytes(), old_account],
        )?;
    }
}

fn rejected_count_by_account_on(
    conn: &Connection,
    feed: FeedId,
) -> Result<Vec<(AccountId, u32)>, StorageError> {
    let mut stmt = conn.prepare(REJECTED_COUNTS_SQL)?;
    let rows = stmt
        .query_map([feed.to_bytes()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(account, count)| {
            Ok((
                AccountId::from_bytes(id_from(&account, "quarantined item account id")?),
                u32::try_from(count_from(count, "quarantined item count")?).unwrap_or(u32::MAX),
            ))
        })
        .collect()
}

fn rejected_list_on(
    conn: &Connection,
    feed: FeedId,
    limit: u32,
) -> Result<Vec<RejectedRow>, StorageError> {
    let mut stmt = conn.prepare(REJECTED_LIST_SQL)?;
    let rows = stmt
        .query_map(params![feed.to_bytes(), i64::from(limit)], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(feed, cursor, doc, account, device, reason, bytes, at)| {
            Ok(RejectedRow {
                feed: FeedId::from_bytes(id_from(&feed, "quarantined item feed id")?),
                cursor: count_from(cursor, "quarantined item cursor")?,
                doc_id: DocId::from_bytes(id_from(&doc, "quarantined item document id")?),
                account_id: AccountId::from_bytes(id_from(
                    &account,
                    "quarantined item account id",
                )?),
                device_id: DeviceId::from_bytes(id_from(&device, "quarantined item device id")?),
                reason: u8::try_from(reason)
                    .ok()
                    .and_then(RejectReason::from_byte)
                    .ok_or_else(|| {
                        StorageError::InvalidData(
                            "quarantined item reason is not 1, 2 or 3".to_owned(),
                        )
                    })?,
                bytes,
                at,
            })
        })
        .collect()
}

// ---- sync_entry_marks ----

fn mark_from(
    (doc, device, end_vv, cursor): (Vec<u8>, Vec<u8>, Vec<u8>, i64),
) -> Result<EntryMarkRow, StorageError> {
    Ok(EntryMarkRow {
        doc_id: DocId::from_bytes(id_from(&doc, "entry mark document id")?),
        device_id: DeviceId::from_bytes(id_from(&device, "entry mark device id")?),
        end_vv,
        cursor: count_from(cursor, "entry mark cursor")?,
    })
}

fn marks_from(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<EntryMarkRow>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params, |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(mark_from).collect()
}

fn entry_mark_put_on(conn: &Connection, row: &EntryMarkRow) -> Result<(), StorageError> {
    conn.execute(
        MARK_PUT_SQL,
        params![
            row.doc_id.to_bytes(),
            row.device_id.to_bytes(),
            row.end_vv,
            int_from(row.cursor, "entry mark cursor")?,
        ],
    )?;
    Ok(())
}

fn entry_mark_get_on(
    conn: &Connection,
    doc_id: DocId,
    device_id: DeviceId,
) -> Result<Option<EntryMarkRow>, StorageError> {
    Ok(marks_from(
        conn,
        MARK_GET_SQL,
        [doc_id.to_bytes(), device_id.to_bytes()],
    )?
    .pop())
}

fn entry_marks_for_doc_on(
    conn: &Connection,
    doc_id: DocId,
) -> Result<Vec<EntryMarkRow>, StorageError> {
    marks_from(conn, MARKS_OF_DOC_SQL, [doc_id.to_bytes()])
}

// ---- membership_entries and membership_state ----

fn membership_entries_put_on(
    conn: &Connection,
    rows: &[MembershipEntryRow],
) -> Result<(), StorageError> {
    // Every number is converted before the first statement runs.
    let numbers: Vec<(i64, i64)> = rows
        .iter()
        .map(|row| {
            Ok((
                int_from(row.seq, "membership entry sequence number")?,
                int_from(row.cursor, "membership entry cursor")?,
            ))
        })
        .collect::<Result<_, StorageError>>()?;
    for (row, (seq, cursor)) in rows.iter().zip(numbers) {
        let key = row.feed.to_bytes();
        let stored: Option<(Vec<u8>, i64)> = conn
            .query_row(MEMBERSHIP_GET_SQL, params![key, seq], |stored| {
                Ok((stored.get(0)?, stored.get(1)?))
            })
            .optional()?;
        match stored {
            None => {
                conn.execute(MEMBERSHIP_INSERT_SQL, params![key, seq, cursor, row.bytes])?;
            }
            Some((bytes, stored_cursor)) if bytes == row.bytes => {
                // The same entry: kept, and a cursor it was found at fills a pending one in.
                if stored_cursor == PENDING_CURSOR && cursor != PENDING_CURSOR {
                    conn.execute(MEMBERSHIP_SET_CURSOR_SQL, params![key, seq, cursor])?;
                }
            }
            Some((_, stored_cursor)) if stored_cursor == PENDING_CURSOR => {
                // A different entry over one that was never found in the feed (a lost
                // compare-and-swap): the newer one replaces it.
                conn.execute(MEMBERSHIP_REPLACE_SQL, params![key, seq, cursor, row.bytes])?;
            }
            Some(_) => {
                return Err(StorageError::InvalidData(
                    "a different membership entry is stored at that sequence number".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn membership_entries_list_on(
    conn: &Connection,
    feed: FeedId,
    after: Option<u64>,
) -> Result<Vec<MembershipEntryRow>, StorageError> {
    // `seq > i64::MIN` is every row (a damaged negative one included, which is refused when read).
    let after = after
        .map(|after| int_from(after, "membership entry sequence number"))
        .transpose()?
        .unwrap_or(i64::MIN);
    let mut stmt = conn.prepare(MEMBERSHIP_LIST_SQL)?;
    let rows = stmt
        .query_map(params![feed.to_bytes(), after], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(feed, seq, cursor, bytes)| {
            Ok(MembershipEntryRow {
                feed: FeedId::from_bytes(id_from(&feed, "membership entry feed id")?),
                seq: count_from(seq, "membership entry sequence number")?,
                cursor: count_from(cursor, "membership entry cursor")?,
                bytes,
            })
        })
        .collect()
}

fn membership_entries_discard_pending_on(
    conn: &Connection,
    feed: FeedId,
) -> Result<u32, StorageError> {
    let removed = conn.execute(
        "DELETE FROM membership_entries WHERE feed = ?1 AND cursor = ?2",
        params![feed.to_bytes(), PENDING_CURSOR],
    )?;
    Ok(u32::try_from(removed).unwrap_or(u32::MAX))
}

fn membership_state_put_on(
    conn: &Connection,
    row: &MembershipStateRow,
) -> Result<(), StorageError> {
    conn.execute(
        MEMBERSHIP_STATE_PUT_SQL,
        params![
            row.feed.to_bytes(),
            int_from(row.seq, "membership state sequence number")?,
            &row.head_hash[..],
            row.state,
        ],
    )?;
    Ok(())
}

/// Raw column values of a `membership_state` row.
struct StateColumns {
    feed: Vec<u8>,
    seq: i64,
    head_hash: Vec<u8>,
    state: Vec<u8>,
}

fn membership_state_get_on(
    conn: &Connection,
    feed: FeedId,
) -> Result<Option<MembershipStateRow>, StorageError> {
    let row = conn
        .query_row(MEMBERSHIP_STATE_GET_SQL, [feed.to_bytes()], |row| {
            Ok(StateColumns {
                feed: row.get(0)?,
                seq: row.get(1)?,
                head_hash: row.get(2)?,
                state: row.get(3)?,
            })
        })
        .optional()?;
    row.map(|row| {
        Ok(MembershipStateRow {
            feed: FeedId::from_bytes(id_from(&row.feed, "membership state feed id")?),
            seq: count_from(row.seq, "membership state sequence number")?,
            head_hash: hash_from(&row.head_hash, "membership state head hash")?,
            state: row.state,
        })
    })
    .transpose()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{SyncDocState, SyncHeadRow, SyncPendingRow, SyncQueries, migrations};

    /// What a scenario needs of a connection or a transaction: both implement both.
    trait Queries: FeedQueries + SyncQueries {}
    impl<T: FeedQueries + SyncQueries> Queries for T {}

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that both
    /// `FeedQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
    }

    const fn feed(n: u8) -> FeedId {
        FeedId::from_bytes([n; 16])
    }
    const fn account(n: u8) -> AccountId {
        AccountId::from_bytes([n; 16])
    }
    const fn device(n: u8) -> DeviceId {
        DeviceId::from_bytes([n; 16])
    }
    const fn doc(n: u8) -> DocId {
        DocId::from_bytes([n; 16])
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

    fn assert_invalid(err: StorageError, what: &str) {
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m.contains(what)),
            "{what}: {err}"
        );
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn columns(db: &Db, table: &str) -> Vec<String> {
        let mut statement = db
            .connection()
            .prepare(&format!(
                "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
            ))
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    // ---- the schema -----------------------------------------------------------------

    #[test]
    fn migration_17_creates_the_shared_space_tables() {
        let db = Db::open_in_memory().unwrap();
        const { assert!(crate::SCHEMA_VERSION >= 17) };
        assert_eq!(
            columns(&db, "sync_feeds"),
            [
                "feed",
                "kind",
                "space_id",
                "cursor",
                "chain_hash",
                "max_covers",
                "chain_history",
                "fork",
                "server_epoch"
            ]
        );
        assert_eq!(
            columns(&db, "foreign_device_logs"),
            ["account_id", "seq", "entry", "fetched_at"]
        );
        assert_eq!(
            columns(&db, "sync_rejected"),
            [
                "feed",
                "cursor",
                "doc_id",
                "account_id",
                "device_id",
                "reason",
                "bytes",
                "at"
            ]
        );
        assert_eq!(
            columns(&db, "sync_rejected_dropped"),
            ["feed", "account_id", "dropped"]
        );
        assert_eq!(
            columns(&db, "sync_entry_marks"),
            ["doc_id", "device_id", "end_vv", "cursor"]
        );
        assert_eq!(
            columns(&db, "membership_entries"),
            ["feed", "seq", "cursor", "bytes"]
        );
        assert_eq!(
            columns(&db, "membership_state"),
            ["feed", "seq", "head_hash", "state"]
        );
    }

    #[test]
    fn the_per_feed_sync_tables_have_the_feed_first_in_their_key() {
        let db = new_db();
        let key = |table: &str| -> Vec<String> {
            let mut stmt = db
                .connection()
                .prepare("SELECT name FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk")
                .unwrap();
            stmt.query_map([table], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(key("sync_doc_state"), ["feed", "doc_id"]);
        assert_eq!(key("sync_seen"), ["feed", "doc_id", "device_id"]);
        assert_eq!(key("sync_heads"), ["feed", "device_id"]);
        assert_eq!(key("sync_pending"), ["feed", "cursor", "doc_id"]);
        // Their other columns are those of v6, in the same order.
        assert_eq!(
            columns(&db, "sync_doc_state"),
            [
                "feed",
                "doc_id",
                "pushed_up_to_seq",
                "next_device_seq",
                "snapshot_covers",
                "entries_since_snapshot",
                "bytes_since_snapshot",
                "remote_snapshot_covers"
            ]
        );
        assert_eq!(
            columns(&db, "sync_heads"),
            [
                "feed",
                "device_id",
                "cursor",
                "chain_hash",
                "at_ms",
                "signed",
                "seen_at"
            ]
        );
        // No foreign keys, as in v6.
        for table in [
            "sync_doc_state",
            "sync_seen",
            "sync_heads",
            "sync_pending",
            "sync_feeds",
            "sync_rejected",
            "sync_entry_marks",
            "membership_entries",
            "membership_state",
            "foreign_device_logs",
            "sync_rejected_dropped",
        ] {
            let foreign: i64 = db
                .connection()
                .query_row(
                    "SELECT count(*) FROM pragma_foreign_key_list(?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(foreign, 0, "{table}");
        }
    }

    /// The rows of the four v6 tables as the v16 schema stored them, written raw.
    const V16_SYNC_ROWS: &str = "\
        INSERT INTO sync_doc_state VALUES (zeroblob(16), 5, 6, 7, 8, 9, 10);
        INSERT INTO sync_doc_state VALUES (x'01010101010101010101010101010101', 1, 2, NULL, 0, 0, NULL);
        INSERT INTO sync_seen VALUES (zeroblob(16), x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', 4);
        INSERT INTO sync_seen VALUES (x'01010101010101010101010101010101', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', 9);
        INSERT INTO sync_heads VALUES (x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', 77, x'bb', 1000, x'0102', 1001);
        INSERT INTO sync_pending VALUES (3, x'01010101010101010101010101010101', x'0201', 1, 5);
        INSERT INTO sync_pending VALUES (3, x'02020202020202020202020202020202', x'0302', 3, 6);";

    /// Migrates a v16 database holding [`V16_SYNC_ROWS`] and `meta` pairs to the latest schema.
    fn migrated_v16(meta: &[(&str, &str)]) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 16).unwrap();
        for (key, value) in meta {
            conn.execute("INSERT INTO meta VALUES (?1, ?2)", [key, value])
                .unwrap();
        }
        conn.execute_batch(V16_SYNC_ROWS).unwrap();
        migrations().to_latest(&mut conn).unwrap();
        conn
    }

    fn feed_of_rows(conn: &Connection, table: &str) -> Vec<Vec<u8>> {
        let mut stmt = conn
            .prepare(&format!("SELECT DISTINCT feed FROM {table}"))
            .unwrap();
        stmt.query_map([], |row| row.get::<_, Vec<u8>>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn migrating_from_v16_gives_every_row_the_personal_feed_and_changes_nothing_else() {
        let account_text = "123e4567-e89b-42d3-a456-426614174000";
        let personal: Vec<u8> = vec![
            0x12, 0x3e, 0x45, 0x67, 0xe8, 0x9b, 0x42, 0xd3, 0xa4, 0x56, 0x42, 0x66, 0x14, 0x17,
            0x40, 0x00,
        ];
        let conn = migrated_v16(&[("account_id", account_text)]);
        for (table, rows) in [
            ("sync_doc_state", 2),
            ("sync_seen", 2),
            ("sync_heads", 1),
            ("sync_pending", 2),
        ] {
            assert_eq!(count(&conn, &format!("SELECT count(*) FROM {table}")), rows);
            assert_eq!(
                feed_of_rows(&conn, table),
                std::slice::from_ref(&personal),
                "{table}"
            );
        }
        // The other columns are the ones that were written.
        let db_state: (i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT pushed_up_to_seq, next_device_seq, snapshot_covers, entries_since_snapshot, \
                 bytes_since_snapshot, remote_snapshot_covers FROM sync_doc_state \
                 WHERE doc_id = zeroblob(16)",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(db_state, (5, 6, 7, 8, 9, 10));
        assert_eq!(
            count(
                &conn,
                "SELECT snapshot_covers IS NULL FROM sync_doc_state WHERE doc_id <> zeroblob(16)"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT last_seq FROM sync_seen WHERE doc_id = zeroblob(16)"
            ),
            4
        );
        assert_eq!(
            count(&conn, "SELECT cursor FROM sync_heads"),
            77,
            "the head keeps its cursor"
        );
        // The two parked snapshots of one cursor stay two rows.
        assert_eq!(
            count(&conn, "SELECT count(*) FROM sync_pending WHERE cursor = 3"),
            2
        );
    }

    #[test]
    fn migrating_from_v16_without_a_usable_account_id_parks_the_rows_under_the_zero_feed() {
        for meta in [
            vec![],
            vec![("account_id", "not an account id")],
            vec![("account_id", "123e4567-e89b-42d3-a456")],
            vec![("account_id", "")],
        ] {
            let conn = migrated_v16(&meta);
            for table in ["sync_doc_state", "sync_seen", "sync_heads", "sync_pending"] {
                assert_eq!(
                    feed_of_rows(&conn, table),
                    [vec![0u8; 16]],
                    "{table} with meta {meta:?}"
                );
            }
        }
        // The upper case and the unhyphenated form of the same id are one id.
        let upper = migrated_v16(&[("account_id", "123E4567-E89B-42D3-A456-426614174000")]);
        let lower = migrated_v16(&[("account_id", "123e4567-e89b-42d3-a456-426614174000")]);
        assert_eq!(
            feed_of_rows(&upper, "sync_seen"),
            feed_of_rows(&lower, "sync_seen")
        );
    }

    // ---- sync_feeds -----------------------------------------------------------------

    fn feed_row(n: u8, kind: FeedKind) -> SyncFeedRow {
        SyncFeedRow {
            feed: feed(n),
            kind,
            space_id: (kind == FeedKind::Shared).then(|| space(n)),
            cursor: u64::from(n) * 100,
            chain_hash: Some([n; 32]),
            max_covers: u64::from(n) * 10,
            chain_history: Some(blob(40 + usize::from(n), n)),
            fork: n.is_multiple_of(2),
            server_epoch: Some([n.wrapping_add(1); 16]),
        }
    }

    #[test]
    fn feeds_round_trip_every_column_and_list_the_personal_feed_first() {
        on_db_and_tx(|q| {
            assert_eq!(q.sync_feed_get(feed(1)).unwrap(), None);
            assert!(q.sync_feed_list().unwrap().is_empty());

            // Written in no particular order; ids that sort below and above the personal one.
            let personal = feed_row(5, FeedKind::Personal);
            let shared_low = feed_row(2, FeedKind::Shared);
            let shared_high = feed_row(9, FeedKind::Shared);
            for row in [&shared_high, &personal, &shared_low] {
                q.sync_feed_put(row).unwrap();
            }
            for row in [&personal, &shared_low, &shared_high] {
                assert_eq!(q.sync_feed_get(row.feed).unwrap().as_ref(), Some(row));
            }
            assert_eq!(
                q.sync_feed_list().unwrap(),
                [personal, shared_low, shared_high],
                "personal first, then by feed id"
            );
        });
    }

    #[test]
    fn a_feed_put_replaces_every_column_and_absent_options_come_back_absent() {
        on_db_and_tx(|q| {
            q.sync_feed_put(&feed_row(2, FeedKind::Shared)).unwrap();
            q.sync_feed_put(&feed_row(3, FeedKind::Shared)).unwrap();
            let replacement = SyncFeedRow {
                feed: feed(2),
                kind: FeedKind::Shared,
                space_id: None,
                cursor: u64::try_from(i64::MAX).unwrap(),
                chain_hash: None,
                max_covers: 0,
                chain_history: None,
                fork: false,
                server_epoch: None,
            };
            q.sync_feed_put(&replacement).unwrap();
            assert_eq!(q.sync_feed_get(feed(2)).unwrap(), Some(replacement.clone()));
            assert_eq!(
                q.sync_feed_list().unwrap(),
                [replacement, feed_row(3, FeedKind::Shared)],
                "one row per feed, the other untouched"
            );
        });
    }

    #[test]
    fn feed_numbers_that_do_not_fit_sqlite_are_refused_and_nothing_is_written() {
        let too_big = u64::try_from(i64::MAX).unwrap() + 1;
        on_db_and_tx(|q| {
            for (what, row) in [
                (
                    "feed cursor",
                    SyncFeedRow {
                        cursor: too_big,
                        ..feed_row(1, FeedKind::Shared)
                    },
                ),
                (
                    "feed snapshot coverage",
                    SyncFeedRow {
                        max_covers: u64::MAX,
                        ..feed_row(1, FeedKind::Shared)
                    },
                ),
            ] {
                assert_invalid(q.sync_feed_put(&row).unwrap_err(), what);
            }
            assert!(q.sync_feed_list().unwrap().is_empty());
        });
    }

    /// Rows of every per-feed table for `feed(n)` (and the doc/device ids derived from `n`).
    fn fill_feed(q: &dyn Queries, n: u8) {
        let f = feed(n);
        q.sync_feed_put(&feed_row(n, FeedKind::Shared)).unwrap();
        q.sync_doc_state_put_in(
            f,
            &SyncDocState {
                doc_id: doc(n),
                pushed_up_to_seq: 1,
                next_device_seq: 2,
                snapshot_covers: None,
                entries_since_snapshot: 0,
                bytes_since_snapshot: 0,
                remote_snapshot_covers: None,
            },
        )
        .unwrap();
        q.sync_seen_put_in(f, doc(n), device(n), 4).unwrap();
        q.sync_heads_put_in(
            f,
            &SyncHeadRow {
                device_id: device(n),
                cursor: 9,
                chain_hash: [n; 32],
                at_ms: 1,
                signed: vec![n],
                seen_at: 2,
            },
        )
        .unwrap();
        q.sync_pending_put_in(
            f,
            &SyncPendingRow {
                cursor: 3,
                doc_id: doc(n),
                bytes: vec![n],
                reason: 1,
                received_at: 5,
            },
        )
        .unwrap();
        q.entry_mark_put(&EntryMarkRow {
            doc_id: doc(n),
            device_id: device(n),
            end_vv: vec![n, n],
            cursor: 7,
        })
        .unwrap();
        q.membership_entries_put(&[MembershipEntryRow {
            feed: f,
            seq: 1,
            cursor: 8,
            bytes: vec![n],
        }])
        .unwrap();
        q.membership_state_put(&MembershipStateRow {
            feed: f,
            seq: 1,
            head_hash: [n; 32],
            state: vec![n; 3],
        })
        .unwrap();
        q.rejected_put(&rejected(n, 1, 1, 1)).unwrap();
        q.foreign_log_put(&ForeignLogRow {
            account_id: account(n),
            seq: 0,
            entry: vec![n],
            fetched_at: 1,
        })
        .unwrap();
    }

    /// Whether any per-feed table has a row of `feed(n)`: the tables, then what they hold.
    fn rows_of_feed(q: &dyn Queries, n: u8) -> [usize; 9] {
        let f = feed(n);
        [
            usize::from(q.sync_feed_get(f).unwrap().is_some()),
            q.sync_doc_state_all_in(f).unwrap().len(),
            usize::from(q.sync_seen_get_in(f, doc(n), device(n)).unwrap().is_some()),
            q.sync_heads_all_in(f).unwrap().len(),
            q.sync_pending_all_in(f).unwrap().len(),
            q.membership_entries_list(f, None).unwrap().len(),
            usize::from(q.membership_state_get(f).unwrap().is_some()),
            q.rejected_list(f, 10).unwrap().len(),
            usize::from(q.entry_mark_get(doc(n), device(n)).unwrap().is_some()),
        ]
    }

    #[test]
    fn forgetting_a_feed_takes_its_rows_with_it_and_only_its_rows() {
        on_db_and_tx(|q| {
            for n in [2, 3] {
                fill_feed(q, n);
            }
            assert_eq!(rows_of_feed(q, 2), [1; 9]);
            q.sync_feed_delete(feed(2)).unwrap();
            assert_eq!(rows_of_feed(q, 2), [0; 9], "everything of feed 2 is gone");
            assert!(q.rejected_count_by_account(feed(2)).unwrap().is_empty());
            assert_eq!(rows_of_feed(q, 3), [1; 9], "feed 3 is untouched");
            assert_eq!(
                q.rejected_count_by_account(feed(3)).unwrap(),
                [(account(3), 1)]
            );
            // The co-members' device logs belong to accounts, not to a feed.
            assert_eq!(q.foreign_log_accounts().unwrap(), [account(2), account(3)]);
            // Forgetting an unknown or a forgotten feed succeeds.
            q.sync_feed_delete(feed(2)).unwrap();
            q.sync_feed_delete(feed(77)).unwrap();
            assert_eq!(rows_of_feed(q, 3), [1; 9]);
        });
    }

    #[test]
    fn forgetting_a_feed_also_forgets_what_its_quarantine_dropped() {
        let db = new_db();
        for i in 0..(REJECTED_MAX_ROWS_PER_FEED + 5) {
            db.rejected_put(&RejectedRow {
                cursor: u64::try_from(i).unwrap() + 1,
                ..rejected(1, 1, 1, 1)
            })
            .unwrap();
        }
        assert!(
            count(
                db.connection(),
                "SELECT count(*) FROM sync_rejected_dropped"
            ) > 0
        );
        db.sync_feed_delete(feed(1)).unwrap();
        assert_eq!(
            count(
                db.connection(),
                "SELECT count(*) FROM sync_rejected_dropped"
            ),
            0
        );
        assert_eq!(
            count(db.connection(), "SELECT count(*) FROM sync_rejected"),
            0
        );
    }

    #[test]
    fn forgetting_a_feed_on_a_bare_db_is_all_or_nothing() {
        let db = new_db();
        fill_feed(&db, 2);
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE DELETE ON membership_state
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db.sync_feed_delete(feed(2)).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(rows_of_feed(&db, 2), [1; 9], "nothing was deleted");
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn the_marks_of_documents_the_feed_never_touched_stay() {
        let db = new_db();
        fill_feed(&db, 2);
        db.entry_mark_put(&EntryMarkRow {
            doc_id: doc(50),
            device_id: device(2),
            end_vv: vec![1],
            cursor: 1,
        })
        .unwrap();
        db.sync_feed_delete(feed(2)).unwrap();
        assert!(db.entry_mark_get(doc(50), device(2)).unwrap().is_some());
        assert!(db.entry_mark_get(doc(2), device(2)).unwrap().is_none());
    }

    // ---- foreign_device_logs --------------------------------------------------------

    fn log_row(account_n: u8, seq: u64) -> ForeignLogRow {
        ForeignLogRow {
            account_id: account(account_n),
            seq,
            entry: blob(30 + usize::try_from(seq).unwrap(), account_n),
            fetched_at: i64::from(account_n) * 1_000 + i64::try_from(seq).unwrap(),
        }
    }

    #[test]
    fn foreign_logs_come_back_in_sequence_order_after_a_sequence_number() {
        on_db_and_tx(|q| {
            assert!(q.foreign_log_accounts().unwrap().is_empty());
            assert!(q.foreign_log_after(account(1), None).unwrap().is_empty());
            for (a, seq) in [(2, 1), (1, 2), (1, 0), (2, 0), (1, 1)] {
                q.foreign_log_put(&log_row(a, seq)).unwrap();
            }
            assert_eq!(
                q.foreign_log_after(account(1), None).unwrap(),
                [log_row(1, 0), log_row(1, 1), log_row(1, 2)]
            );
            assert_eq!(
                q.foreign_log_after(account(1), Some(0)).unwrap(),
                [log_row(1, 1), log_row(1, 2)],
                "exclusive"
            );
            assert_eq!(
                q.foreign_log_after(account(1), Some(2)).unwrap(),
                [],
                "past the end"
            );
            assert_eq!(
                q.foreign_log_after(account(2), Some(0)).unwrap(),
                [log_row(2, 1)],
                "only the account's own entries"
            );
            assert_eq!(
                q.foreign_log_accounts().unwrap(),
                [account(1), account(2)],
                "ordered by account id, each once"
            );
        });
    }

    #[test]
    fn a_foreign_log_put_replaces_the_entry_with_the_same_account_and_sequence() {
        on_db_and_tx(|q| {
            q.foreign_log_put(&log_row(1, 0)).unwrap();
            q.foreign_log_put(&log_row(1, 1)).unwrap();
            let replacement = ForeignLogRow {
                entry: vec![0, 0xFF, 0x80],
                fetched_at: 5,
                ..log_row(1, 0)
            };
            q.foreign_log_put(&replacement).unwrap();
            assert_eq!(
                q.foreign_log_after(account(1), None).unwrap(),
                [replacement, log_row(1, 1)]
            );
            assert_invalid(
                q.foreign_log_put(&ForeignLogRow {
                    seq: u64::MAX,
                    ..log_row(1, 0)
                })
                .unwrap_err(),
                "out of range",
            );
            assert_invalid(
                q.foreign_log_after(account(1), Some(u64::MAX)).unwrap_err(),
                "out of range",
            );
        });
    }

    // ---- sync_rejected --------------------------------------------------------------

    fn rejected(n: u8, cursor: u64, at: i64, reason: u8) -> RejectedRow {
        RejectedRow {
            feed: feed(n),
            cursor,
            doc_id: doc(0),
            account_id: account(n),
            device_id: device(n),
            reason: RejectReason::from_byte(reason).unwrap(),
            bytes: blob(20 + usize::try_from(cursor).unwrap(), n),
            at,
        }
    }

    #[test]
    fn quarantined_items_list_newest_first_and_keep_every_column() {
        on_db_and_tx(|q| {
            assert!(q.rejected_list(feed(1), 10).unwrap().is_empty());
            let rows = [
                rejected(1, 5, 100, 1),
                rejected(1, 6, 300, 2),
                rejected(1, 7, 200, 3),
                RejectedRow {
                    doc_id: doc(8),
                    ..rejected(1, 8, 200, 1)
                },
            ];
            for row in &rows {
                q.rejected_put(row).unwrap();
            }
            let listed = q.rejected_list(feed(1), 10).unwrap();
            assert_eq!(
                listed,
                [
                    rows[1].clone(),
                    rows[3].clone(),
                    rows[2].clone(),
                    rows[0].clone()
                ],
                "by time, newest first; the same time by cursor, newest first"
            );
            assert_eq!(q.rejected_list(feed(1), 2).unwrap(), listed[..2]);
            assert!(q.rejected_list(feed(1), 0).unwrap().is_empty());
            assert!(q.rejected_list(feed(2), 10).unwrap().is_empty(), "per feed");
        });
    }

    #[test]
    fn a_quarantine_put_is_an_upsert_on_feed_cursor_and_document() {
        on_db_and_tx(|q| {
            q.rejected_put(&rejected(1, 5, 100, 1)).unwrap();
            let again = RejectedRow {
                reason: RejectReason::SnapshotNotDominant,
                bytes: vec![9, 9],
                at: 500,
                account_id: account(7),
                ..rejected(1, 5, 100, 1)
            };
            q.rejected_put(&again).unwrap();
            assert_eq!(q.rejected_list(feed(1), 10).unwrap(), [again]);
            assert_eq!(
                q.rejected_count_by_account(feed(1)).unwrap(),
                [(account(7), 1)],
                "counted once"
            );
            // The same cursor for another document, or in another feed, is another row.
            q.rejected_put(&RejectedRow {
                doc_id: doc(3),
                ..rejected(1, 5, 100, 1)
            })
            .unwrap();
            q.rejected_put(&rejected(2, 5, 100, 1)).unwrap();
            assert_eq!(q.rejected_list(feed(1), 10).unwrap().len(), 2);
            assert_eq!(q.rejected_list(feed(2), 10).unwrap().len(), 1);
            assert_invalid(
                q.rejected_put(&RejectedRow {
                    cursor: u64::MAX,
                    ..rejected(1, 9, 1, 1)
                })
                .unwrap_err(),
                "out of range",
            );
        });
    }

    #[test]
    fn quarantined_items_are_counted_per_writer_account_and_feed() {
        on_db_and_tx(|q| {
            let by = |account_n: u8, cursor: u64| RejectedRow {
                account_id: account(account_n),
                ..rejected(1, cursor, 1, 2)
            };
            for (account_n, cursor) in [(3, 1), (1, 2), (3, 3), (3, 4), (2, 5)] {
                q.rejected_put(&by(account_n, cursor)).unwrap();
            }
            q.rejected_put(&rejected(2, 1, 1, 1)).unwrap();
            assert_eq!(
                q.rejected_count_by_account(feed(1)).unwrap(),
                [(account(1), 1), (account(2), 1), (account(3), 3)],
                "ordered by account id"
            );
            assert_eq!(
                q.rejected_count_by_account(feed(2)).unwrap(),
                [(account(2), 1)]
            );
            assert!(q.rejected_count_by_account(feed(9)).unwrap().is_empty());
        });
    }

    #[test]
    fn the_quarantine_keeps_a_bounded_number_of_rows_and_still_counts_the_rest() {
        let extra = 7usize;
        let total = REJECTED_MAX_ROWS_PER_FEED + extra;
        on_db_and_tx(|q| {
            for i in 0..total {
                // Two writers alternate; the time and the cursor grow together.
                let n = u8::try_from(1 + i % 2).unwrap();
                let at = i64::try_from(i).unwrap();
                q.rejected_put(&RejectedRow {
                    account_id: account(n),
                    ..rejected(1, u64::try_from(i).unwrap() + 1, at, 1)
                })
                .unwrap();
            }
            q.rejected_put(&rejected(2, 1, 0, 1)).unwrap();
            let kept = q
                .rejected_list(feed(1), u32::try_from(total).unwrap())
                .unwrap();
            assert_eq!(kept.len(), REJECTED_MAX_ROWS_PER_FEED);
            assert_eq!(
                kept[0].at,
                i64::try_from(total).unwrap() - 1,
                "the newest stays"
            );
            assert_eq!(
                kept.last().unwrap().at,
                i64::try_from(extra).unwrap(),
                "the oldest rows went"
            );
            // Nothing a writer was refused for is forgotten: stored plus dropped.
            let counts = q.rejected_count_by_account(feed(1)).unwrap();
            let sum: u32 = counts.iter().map(|(_, n)| *n).sum();
            assert_eq!(usize::try_from(sum).unwrap(), total);
            assert_eq!(counts.len(), 2);
            assert_eq!(
                q.rejected_list(feed(2), 10).unwrap().len(),
                1,
                "another feed has its own bound"
            );
        });
    }

    #[test]
    fn the_quarantine_keeps_a_bounded_number_of_bytes_and_never_drops_the_item_just_put() {
        let big = REJECTED_MAX_BYTES_PER_FEED / 2 + 1;
        on_db_and_tx(|q| {
            for (cursor, at) in [(1, 10), (2, 20), (3, 30)] {
                q.rejected_put(&RejectedRow {
                    bytes: vec![u8::try_from(cursor).unwrap(); big],
                    ..rejected(1, cursor, at, 1)
                })
                .unwrap();
            }
            // Two such items do not fit: only the newest stays, and the others are counted.
            let kept = q.rejected_list(feed(1), 10).unwrap();
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].cursor, 3);
            assert_eq!(
                q.rejected_count_by_account(feed(1)).unwrap(),
                [(account(1), 3)]
            );

            // An item larger than the whole bound is stored alone rather than refused.
            q.rejected_put(&RejectedRow {
                bytes: vec![7; REJECTED_MAX_BYTES_PER_FEED + 1],
                ..rejected(1, 4, 40, 1)
            })
            .unwrap();
            let kept = q.rejected_list(feed(1), 10).unwrap();
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].cursor, 4);
            assert_eq!(
                q.rejected_count_by_account(feed(1)).unwrap(),
                [(account(1), 4)]
            );

            // An old item (a back-dated time) put last is still the one that stays.
            q.rejected_put(&RejectedRow {
                bytes: vec![8; big],
                ..rejected(1, 5, -1_000, 1)
            })
            .unwrap();
            let kept = q.rejected_list(feed(1), 10).unwrap();
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].cursor, 5);
        });
    }

    #[test]
    fn a_damaged_quarantine_row_is_invalid_data() {
        for (sql, what) in [
            (
                "INSERT INTO sync_rejected VALUES (zeroblob(16), 1, zeroblob(16), zeroblob(16), \
                 zeroblob(16), 9, x'01', 0)",
                "a reason of 9",
            ),
            (
                "INSERT INTO sync_rejected VALUES (zeroblob(16), -1, zeroblob(16), zeroblob(16), \
                 zeroblob(16), 1, x'01', 0)",
                "a negative cursor",
            ),
            (
                "INSERT INTO sync_rejected VALUES (zeroblob(16), 1, zeroblob(15), zeroblob(16), \
                 zeroblob(16), 1, x'01', 0)",
                "a document id of 15 bytes",
            ),
        ] {
            let db = new_db();
            db.connection().execute(sql, []).unwrap();
            let err = db
                .rejected_list(FeedId::from_bytes([0; 16]), 10)
                .unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{what}: {err}");
        }
    }

    // ---- sync_entry_marks -----------------------------------------------------------

    fn mark(doc_n: u8, device_n: u8, cursor: u64) -> EntryMarkRow {
        EntryMarkRow {
            doc_id: doc(doc_n),
            device_id: device(device_n),
            end_vv: blob(12 + usize::from(device_n), device_n),
            cursor,
        }
    }

    #[test]
    fn a_mark_is_kept_per_document_and_device_with_upsert() {
        on_db_and_tx(|q| {
            assert_eq!(q.entry_mark_get(doc(1), device(1)).unwrap(), None);
            assert!(q.entry_marks_for_doc(doc(1)).unwrap().is_empty());
            for (d, w, cursor) in [(1, 3, 30), (1, 1, 10), (2, 1, 11), (1, 2, 20)] {
                q.entry_mark_put(&mark(d, w, cursor)).unwrap();
            }
            assert_eq!(
                q.entry_mark_get(doc(1), device(2)).unwrap(),
                Some(mark(1, 2, 20))
            );
            assert_eq!(q.entry_mark_get(doc(2), device(2)).unwrap(), None);
            assert_eq!(
                q.entry_marks_for_doc(doc(1)).unwrap(),
                [mark(1, 1, 10), mark(1, 2, 20), mark(1, 3, 30)],
                "every writer of the document, by device id"
            );
            assert_eq!(q.entry_marks_for_doc(doc(2)).unwrap(), [mark(2, 1, 11)]);

            // The last applied entry replaces the mark; the vector is stored as given.
            let newer = EntryMarkRow {
                end_vv: vec![0, 0xFF, 0x80],
                ..mark(1, 2, 99)
            };
            q.entry_mark_put(&newer).unwrap();
            assert_eq!(q.entry_mark_get(doc(1), device(2)).unwrap(), Some(newer));
            assert_eq!(q.entry_marks_for_doc(doc(1)).unwrap().len(), 3);

            assert_invalid(
                q.entry_mark_put(&mark(1, 1, u64::MAX)).unwrap_err(),
                "out of range",
            );
            assert_eq!(
                q.entry_mark_get(doc(1), device(1)).unwrap(),
                Some(mark(1, 1, 10)),
                "a refused put changed nothing"
            );
        });
    }

    // ---- membership_entries and membership_state ------------------------------------

    fn entry(feed_n: u8, seq: u64, cursor: u64) -> MembershipEntryRow {
        MembershipEntryRow {
            feed: feed(feed_n),
            seq,
            cursor,
            bytes: blob(
                50 + usize::try_from(seq).unwrap(),
                u8::try_from(seq).unwrap(),
            ),
        }
    }

    #[test]
    fn membership_entries_are_listed_in_sequence_order_after_a_sequence_number() {
        on_db_and_tx(|q| {
            assert!(q.membership_entries_list(feed(1), None).unwrap().is_empty());
            q.membership_entries_put(&[entry(1, 3, 30), entry(1, 1, 10), entry(1, 2, 20)])
                .unwrap();
            q.membership_entries_put(&[entry(2, 1, 11)]).unwrap();
            q.membership_entries_put(&[]).unwrap();
            assert_eq!(
                q.membership_entries_list(feed(1), None).unwrap(),
                [entry(1, 1, 10), entry(1, 2, 20), entry(1, 3, 30)]
            );
            assert_eq!(
                q.membership_entries_list(feed(1), Some(1)).unwrap(),
                [entry(1, 2, 20), entry(1, 3, 30)],
                "exclusive"
            );
            assert!(
                q.membership_entries_list(feed(1), Some(3))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(q.membership_entries_list(feed(2), None).unwrap().len(), 1);
            assert_invalid(
                q.membership_entries_put(&[MembershipEntryRow {
                    seq: u64::MAX,
                    ..entry(1, 9, 1)
                }])
                .unwrap_err(),
                "out of range",
            );
            assert_invalid(
                q.membership_entries_put(&[entry(1, 9, u64::MAX)])
                    .unwrap_err(),
                "out of range",
            );
            assert_eq!(q.membership_entries_list(feed(1), None).unwrap().len(), 3);
        });
    }

    #[test]
    fn storing_a_membership_entry_again_keeps_it() {
        on_db_and_tx(|q| {
            q.membership_entries_put(&[entry(1, 1, 10), entry(1, 2, 20)])
                .unwrap();
            // The same entries, in any batch: nothing changes, also with another cursor.
            q.membership_entries_put(&[entry(1, 2, 20), entry(1, 1, 10)])
                .unwrap();
            q.membership_entries_put(&[entry(1, 1, 99)]).unwrap();
            assert_eq!(
                q.membership_entries_list(feed(1), None).unwrap(),
                [entry(1, 1, 10), entry(1, 2, 20)],
                "an entry found in the feed keeps the cursor it was found at"
            );
        });
    }

    #[test]
    fn a_different_entry_at_a_stored_sequence_number_is_refused_and_the_batch_with_it() {
        let db = new_db();
        db.membership_entries_put(&[entry(1, 1, 10), entry(1, 2, 20)])
            .unwrap();
        let forged = MembershipEntryRow {
            bytes: vec![1, 2, 3],
            ..entry(1, 2, 20)
        };
        // The first row of the batch is new, the second contradicts a stored entry.
        let err = db
            .membership_entries_put(&[entry(1, 3, 30), forged.clone()])
            .unwrap_err();
        assert_invalid(err, "different membership entry");
        assert_eq!(
            db.membership_entries_list(feed(1), None).unwrap(),
            [entry(1, 1, 10), entry(1, 2, 20)],
            "nothing of the batch was stored"
        );
        // A pending put over an entry that was found in the feed is refused as well.
        let pending_over_found = MembershipEntryRow {
            cursor: MEMBERSHIP_PENDING_CURSOR,
            ..forged
        };
        assert_invalid(
            db.membership_entries_put(&[pending_over_found])
                .unwrap_err(),
            "different membership entry",
        );
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_pending_membership_entry_takes_its_cursor_or_is_replaced_by_the_servers() {
        assert_eq!(
            i64::try_from(MEMBERSHIP_PENDING_CURSOR).unwrap(),
            PENDING_CURSOR
        );
        on_db_and_tx(|q| {
            // Appended locally, not found in the feed yet.
            q.membership_entries_put(&[entry(1, 1, 10), entry(1, 2, MEMBERSHIP_PENDING_CURSOR)])
                .unwrap();
            assert_eq!(
                q.membership_entries_list(feed(1), Some(1)).unwrap()[0].cursor,
                MEMBERSHIP_PENDING_CURSOR
            );
            // The same entry found in the feed: the cursor fills in.
            q.membership_entries_put(&[entry(1, 2, 25)]).unwrap();
            assert_eq!(
                q.membership_entries_list(feed(1), Some(1)).unwrap(),
                [entry(1, 2, 25)]
            );

            // Another pending entry that lost the compare-and-swap: the server's replaces it.
            q.membership_entries_put(&[entry(1, 3, MEMBERSHIP_PENDING_CURSOR)])
                .unwrap();
            let servers = MembershipEntryRow {
                bytes: vec![7; 5],
                ..entry(1, 3, 31)
            };
            q.membership_entries_put(std::slice::from_ref(&servers))
                .unwrap();
            assert_eq!(
                q.membership_entries_list(feed(1), Some(2)).unwrap(),
                [servers]
            );

            // A pending entry may be rebuilt over another pending one.
            q.membership_entries_put(&[entry(1, 4, MEMBERSHIP_PENDING_CURSOR)])
                .unwrap();
            let rebuilt = MembershipEntryRow {
                bytes: vec![8; 5],
                ..entry(1, 4, MEMBERSHIP_PENDING_CURSOR)
            };
            q.membership_entries_put(std::slice::from_ref(&rebuilt))
                .unwrap();
            assert_eq!(
                q.membership_entries_list(feed(1), Some(3)).unwrap(),
                [rebuilt]
            );
        });
    }

    #[test]
    fn pending_membership_entries_can_be_discarded_per_feed() {
        on_db_and_tx(|q| {
            q.membership_entries_put(&[
                entry(1, 1, 10),
                entry(1, 2, MEMBERSHIP_PENDING_CURSOR),
                entry(1, 3, MEMBERSHIP_PENDING_CURSOR),
                entry(2, 1, MEMBERSHIP_PENDING_CURSOR),
            ])
            .unwrap();
            assert_eq!(q.membership_entries_discard_pending(feed(1)).unwrap(), 2);
            assert_eq!(
                q.membership_entries_list(feed(1), None).unwrap(),
                [entry(1, 1, 10)]
            );
            assert_eq!(
                q.membership_entries_list(feed(2), None).unwrap().len(),
                1,
                "another feed's pending entry stays"
            );
            assert_eq!(q.membership_entries_discard_pending(feed(1)).unwrap(), 0);
            assert_eq!(q.membership_entries_discard_pending(feed(9)).unwrap(), 0);
        });
    }

    #[test]
    fn the_membership_cache_is_an_upsert_per_feed() {
        on_db_and_tx(|q| {
            assert_eq!(q.membership_state_get(feed(1)).unwrap(), None);
            let first = MembershipStateRow {
                feed: feed(1),
                seq: 3,
                head_hash: [0xAB; 32],
                state: blob(3000, 7),
            };
            q.membership_state_put(&first).unwrap();
            q.membership_state_put(&MembershipStateRow {
                feed: feed(2),
                ..first.clone()
            })
            .unwrap();
            assert_eq!(
                q.membership_state_get(feed(1)).unwrap(),
                Some(first.clone())
            );
            let newer = MembershipStateRow {
                seq: 4,
                head_hash: [0xCD; 32],
                state: vec![0, 0xFF],
                ..first
            };
            q.membership_state_put(&newer).unwrap();
            assert_eq!(q.membership_state_get(feed(1)).unwrap(), Some(newer));
            assert_eq!(q.membership_state_get(feed(2)).unwrap().unwrap().seq, 3);
            assert_invalid(
                q.membership_state_put(&MembershipStateRow {
                    seq: u64::MAX,
                    ..q.membership_state_get(feed(2)).unwrap().unwrap()
                })
                .unwrap_err(),
                "out of range",
            );
        });
    }

    // ---- damaged rows ---------------------------------------------------------------

    #[test]
    fn damaged_rows_are_invalid_data_and_never_a_panic() {
        type Read = fn(&Db) -> Result<(), StorageError>;
        let cases: [(&str, Read, &str); 12] = [
            (
                "INSERT INTO sync_feeds (feed, kind) VALUES (zeroblob(16), 9)",
                |db| db.sync_feed_list().map(drop),
                "a feed kind of 9",
            ),
            (
                "INSERT INTO sync_feeds (feed, kind) VALUES (zeroblob(15), 1)",
                |db| db.sync_feed_list().map(drop),
                "a feed id of 15 bytes",
            ),
            (
                "INSERT INTO sync_feeds (feed, kind, chain_hash) VALUES (zeroblob(16), 1, zeroblob(31))",
                |db| db.sync_feed_get(FeedId::from_bytes([0; 16])).map(drop),
                "a chain hash of 31 bytes",
            ),
            (
                "INSERT INTO sync_feeds (feed, kind, server_epoch) VALUES (zeroblob(16), 1, zeroblob(15))",
                |db| db.sync_feed_list().map(drop),
                "a server epoch of 15 bytes",
            ),
            (
                "INSERT INTO sync_feeds (feed, kind, cursor) VALUES (zeroblob(16), 1, -1)",
                |db| db.sync_feed_list().map(drop),
                "a negative cursor",
            ),
            (
                "INSERT INTO sync_feeds (feed, kind, fork) VALUES (zeroblob(16), 1, 2)",
                |db| db.sync_feed_list().map(drop),
                "a fork flag of 2",
            ),
            (
                "INSERT INTO sync_feeds (feed, kind, space_id) VALUES (zeroblob(16), 2, zeroblob(3))",
                |db| db.sync_feed_list().map(drop),
                "a space id of 3 bytes",
            ),
            (
                "INSERT INTO foreign_device_logs VALUES (zeroblob(15), 0, x'01', 0)",
                |db| db.foreign_log_accounts().map(drop),
                "an account id of 15 bytes",
            ),
            (
                "INSERT INTO foreign_device_logs VALUES (zeroblob(16), -1, x'01', 0)",
                |db| {
                    db.foreign_log_after(AccountId::from_bytes([0; 16]), None)
                        .map(drop)
                },
                "a negative sequence number",
            ),
            (
                "INSERT INTO sync_entry_marks VALUES (zeroblob(16), zeroblob(16), x'01', -1)",
                |db| db.entry_marks_for_doc(DocId::from_bytes([0; 16])).map(drop),
                "a negative mark cursor",
            ),
            (
                "INSERT INTO membership_entries VALUES (zeroblob(16), -1, 0, x'01')",
                |db| {
                    db.membership_entries_list(FeedId::from_bytes([0; 16]), None)
                        .map(drop)
                },
                "a negative sequence number",
            ),
            (
                "INSERT INTO membership_state VALUES (zeroblob(16), 1, zeroblob(31), x'01')",
                |db| {
                    db.membership_state_get(FeedId::from_bytes([0; 16]))
                        .map(drop)
                },
                "a head hash of 31 bytes",
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
    fn errors_never_contain_blob_contents() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO sync_feeds (feed, kind, chain_hash) \
                 VALUES (zeroblob(16), 1, x'DEADBEEF')",
                [],
            )
            .unwrap();
        let text = db.sync_feed_list().unwrap_err().to_string().to_lowercase();
        assert!(!text.contains("deadbeef"), "{text}");
        db.membership_entries_put(&[MembershipEntryRow {
            bytes: vec![0xC0, 0xFF, 0xEE],
            ..entry(1, 1, 5)
        }])
        .unwrap();
        let text = db
            .membership_entries_put(&[MembershipEntryRow {
                bytes: vec![0xBA, 0xAD, 0xF0, 0x0D],
                ..entry(1, 1, 5)
            }])
            .unwrap_err()
            .to_string()
            .to_lowercase();
        assert!(
            !text.contains("c0ffee") && !text.contains("baadf00d"),
            "{text}"
        );
    }

    // ---- transactions ---------------------------------------------------------------

    #[test]
    fn writes_roll_back_with_a_transaction_that_fails() {
        let mut db = new_db();
        fill_feed(&db, 2);
        let err = db
            .transaction(|tx| {
                fill_feed(tx, 3);
                tx.sync_feed_delete(feed(2))?;
                tx.rejected_put(&rejected(2, 99, 1, 1))?;
                tx.membership_entries_put(&[entry(2, 9, 1)])?;
                Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m == "boom"),
            "{err}"
        );
        assert_eq!(rows_of_feed(&db, 2), [1; 9]);
        assert_eq!(rows_of_feed(&db, 3), [0; 9]);
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_bare_db_stores_a_batch_of_membership_entries_all_or_nothing() {
        let db = new_db();
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE INSERT ON membership_entries
                 WHEN NEW.seq = 3
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db
            .membership_entries_put(&[entry(1, 1, 1), entry(1, 2, 2), entry(1, 3, 3)])
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert!(
            db.membership_entries_list(feed(1), None)
                .unwrap()
                .is_empty()
        );
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_bare_db_drops_and_counts_a_quarantine_overflow_all_or_nothing() {
        let db = new_db();
        for i in 0..REJECTED_MAX_ROWS_PER_FEED {
            db.rejected_put(&rejected(
                1,
                u64::try_from(i).unwrap() + 1,
                i64::try_from(i).unwrap(),
                1,
            ))
            .unwrap();
        }
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE INSERT ON sync_rejected_dropped
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db.rejected_put(&rejected(1, 9_999, 9_999, 1)).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            db.rejected_list(feed(1), 1_000).unwrap().len(),
            REJECTED_MAX_ROWS_PER_FEED,
            "the new row is not there and no old one is gone"
        );
        assert!(db.rejected_list(feed(1), 1).unwrap()[0].cursor != 9_999);
        assert!(db.connection().is_autocommit());
    }

    // ---- query plans ----------------------------------------------------------------

    /// The query plan SQLite reports for a statement, with every parameter bound to a dummy value.
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
        for sql in [
            FEED_GET_SQL,
            FOREIGN_AFTER_SQL,
            REJECTED_TOTALS_SQL,
            REJECTED_OLDEST_SQL,
            REJECTED_DELETE_SQL,
            REJECTED_LIST_SQL,
            MARK_GET_SQL,
            MARKS_OF_DOC_SQL,
            MEMBERSHIP_GET_SQL,
            MEMBERSHIP_LIST_SQL,
            MEMBERSHIP_STATE_GET_SQL,
        ] {
            let plan = plan_of(sql);
            assert!(!plan.contains("SCAN"), "{plan} for {sql}");
        }
    }

    #[test]
    fn the_quarantine_reasons_have_their_stored_values() {
        for (reason, byte) in [
            (RejectReason::NotAMember, 1),
            (RejectReason::RoleForbids, 2),
            (RejectReason::SnapshotNotDominant, 3),
        ] {
            assert_eq!(reason.to_byte(), byte);
            assert_eq!(RejectReason::from_byte(byte), Some(reason));
        }
        assert_eq!(RejectReason::from_byte(0), None);
        assert_eq!(RejectReason::from_byte(4), None);
    }
}
