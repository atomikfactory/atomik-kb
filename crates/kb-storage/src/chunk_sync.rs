//! The per-chunk transfer state of attachment sync (schema v8,
//! `docs/development/milestone-08.md`, "Storage"; ADR-0023). One row per chunk of every
//! manifest this device knows: which attachment it belongs to, what the sync owes it
//! (upload, download, delete) or that both sides have it, how many tries did not finish and
//! the earliest time of the next one. The chunks themselves are not here: they live in the
//! [`crate::BlobStore`], and the manifests in the structure documents.
//!
//! Derived bookkeeping like `attachments_index`: no foreign keys (a purge removes the
//! attachment and leaves the `Delete` rows that tell the server to drop its chunks), not
//! part of a backup, and the core rebuilds it from the manifests and the blob store when it
//! is missing. A new server epoch does **not** reset it here: `sync_doc_state_reset_all`
//! and the `mark_all_unpublished` calls of [`crate::SyncQueries`] leave it alone, and the
//! core calls [`ChunkSyncQueries::chunk_sync_reset`] itself.
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`] through
//! [`ChunkSyncQueries`], both running the same private functions over a connection (a
//! transaction derefs to one), so a method behaves identically in either place. Every method
//! is a single statement, hence atomic by itself on a bare [`Db`]. Rows are read as raw
//! column values first and validated afterwards, so a damaged row is a typed `InvalidData`
//! error and never a panic or a silently wrong value. Chunk ids and attachment ids never
//! appear in an error.
//!
//! # `state`
//!
//! The column holds a small integer, and **the core defines its meaning**; this crate stores
//! any value from 0 to 255 as given and never interprets it, with one exception:
//! [`ChunkSyncQueries::chunk_sync_count_waiting`] counts the rows of state 2. The values the
//! core uses (ADR-0023):
//!
//! | state | name       | meaning                                                          |
//! |-------|------------|------------------------------------------------------------------|
//! | 1     | `Upload`   | the chunk is here, the server lacks it                           |
//! | 2     | `Download` | the chunk is wanted here, the server should have it              |
//! | 3     | `Synced`   | both have it                                                     |
//! | 4     | `Delete`   | a purge here asks the server to drop it                          |
//!
//! # `attempts` and `next_at`
//!
//! `attempts` counts the tries that did not finish (the server did not hold the chunk yet,
//! the network failed) and orders the work: the chunks with the fewest attempts come first.
//! `next_at` (Unix ms) is the earliest time of the next try; NULL means now. The core
//! computes the back-off; this crate stores both as given, `next_at` as any `i64`.
//!
//! # `waiting`
//!
//! A chunk is *waiting* when it is wanted (state 2) and a try already failed (`attempts >
//! 0`): the server answered that it does not hold it yet because the uploading device has
//! not finished.

use kb_types::{AttachmentId, ChunkId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::docs::id_from;
use crate::{Db, StorageError, Tx};

/// The state whose rows with `attempts > 0` are *waiting*: `Download`.
const STATE_DOWNLOAD: u8 = 2;

/// A `chunk_sync` row: what the sync owes one chunk of an attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSyncRow {
    /// The chunk: the BLAKE3 hash of its ciphertext. Unique in the table.
    pub hash: ChunkId,
    /// The attachment whose manifest lists the chunk (exactly one, by construction: the
    /// hash is of ciphertext under a per-attachment key).
    pub attachment_id: AttachmentId,
    /// 1 = upload, 2 = download, 3 = synced, 4 = delete. This crate stores any value from 0
    /// to 255; the core defines the meaning.
    pub state: u8,
    /// Tries that did not finish.
    pub attempts: u32,
    /// Not before this time (Unix ms); `None` means now.
    pub next_at: Option<i64>,
}

/// Queries of the `chunk_sync` table, available on a connection or inside a transaction.
/// On a [`Db`] every method is atomic by itself.
pub trait ChunkSyncQueries {
    /// Inserts the row or replaces every column of the one with the same hash (an upsert).
    /// Nothing is merged: `attachment_id`, `state`, `attempts` and `next_at` are whatever the
    /// caller writes.
    fn chunk_sync_put(&self, row: &ChunkSyncRow) -> Result<(), StorageError>;

    /// One row, `None` for a chunk the table does not know.
    fn chunk_sync_get(&self, hash: ChunkId) -> Result<Option<ChunkSyncRow>, StorageError>;

    /// Every row of an attachment, in hash order (the bytes compared as unsigned, which is
    /// the order of [`ChunkId`]). Empty for an attachment with none.
    fn chunk_sync_by_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Vec<ChunkSyncRow>, StorageError>;

    /// At most `limit` rows of `state` that are due at `now` (Unix ms): `next_at` is NULL or
    /// `<= now`. Fewest `attempts` first, then hash order. A `limit` of 0 gives none.
    fn chunk_sync_due(
        &self,
        state: u8,
        now: i64,
        limit: u32,
    ) -> Result<Vec<ChunkSyncRow>, StorageError>;

    /// Sets `state`, `attempts` and `next_at` of the row with this hash and leaves its
    /// attachment alone. Succeeds when there is no such row (nothing happens: this is not an
    /// insert).
    fn chunk_sync_set(
        &self,
        hash: ChunkId,
        state: u8,
        attempts: u32,
        next_at: Option<i64>,
    ) -> Result<(), StorageError>;

    /// Removes one row. Succeeds when there is none.
    fn chunk_sync_delete(&self, hash: ChunkId) -> Result<(), StorageError>;

    /// Removes every row of an attachment. Succeeds when there is none.
    fn chunk_sync_delete_attachment(&self, attachment_id: AttachmentId)
    -> Result<(), StorageError>;

    /// Number of rows in `state`, due or not.
    fn chunk_sync_count(&self, state: u8) -> Result<u32, StorageError>;

    /// Number of chunks that are wanted and were not served yet: rows of state 2 with
    /// `attempts > 0`, whatever their `next_at`.
    fn chunk_sync_count_waiting(&self) -> Result<u32, StorageError>;

    /// Number of rows of one attachment whose state is in `states` (a state listed twice
    /// counts once; an empty list counts nothing).
    fn chunk_sync_count_attachment(
        &self,
        attachment_id: AttachmentId,
        states: &[u8],
    ) -> Result<u32, StorageError>;

    /// Moves every row of state `from` to state `to` with `attempts = 0` and `next_at =
    /// NULL`, and returns how many rows moved (a new server epoch: everything local goes up
    /// again). With `from == to` the rows keep their state and lose their back-off.
    fn chunk_sync_reset(&self, from: u8, to: u8) -> Result<u32, StorageError>;

    /// Removes every row (the core rebuilds the table from the manifests).
    fn chunk_sync_clear(&self) -> Result<(), StorageError>;
}

impl ChunkSyncQueries for Db {
    fn chunk_sync_put(&self, row: &ChunkSyncRow) -> Result<(), StorageError> {
        chunk_sync_put_on(&self.conn, row)
    }
    fn chunk_sync_get(&self, hash: ChunkId) -> Result<Option<ChunkSyncRow>, StorageError> {
        chunk_sync_get_on(&self.conn, hash)
    }
    fn chunk_sync_by_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Vec<ChunkSyncRow>, StorageError> {
        chunk_sync_by_attachment_on(&self.conn, attachment_id)
    }
    fn chunk_sync_due(
        &self,
        state: u8,
        now: i64,
        limit: u32,
    ) -> Result<Vec<ChunkSyncRow>, StorageError> {
        chunk_sync_due_on(&self.conn, state, now, limit)
    }
    fn chunk_sync_set(
        &self,
        hash: ChunkId,
        state: u8,
        attempts: u32,
        next_at: Option<i64>,
    ) -> Result<(), StorageError> {
        chunk_sync_set_on(&self.conn, hash, state, attempts, next_at)
    }
    fn chunk_sync_delete(&self, hash: ChunkId) -> Result<(), StorageError> {
        chunk_sync_delete_on(&self.conn, hash)
    }
    fn chunk_sync_delete_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<(), StorageError> {
        chunk_sync_delete_attachment_on(&self.conn, attachment_id)
    }
    fn chunk_sync_count(&self, state: u8) -> Result<u32, StorageError> {
        chunk_sync_count_on(&self.conn, state)
    }
    fn chunk_sync_count_waiting(&self) -> Result<u32, StorageError> {
        chunk_sync_count_waiting_on(&self.conn)
    }
    fn chunk_sync_count_attachment(
        &self,
        attachment_id: AttachmentId,
        states: &[u8],
    ) -> Result<u32, StorageError> {
        chunk_sync_count_attachment_on(&self.conn, attachment_id, states)
    }
    fn chunk_sync_reset(&self, from: u8, to: u8) -> Result<u32, StorageError> {
        chunk_sync_reset_on(&self.conn, from, to)
    }
    fn chunk_sync_clear(&self) -> Result<(), StorageError> {
        chunk_sync_clear_on(&self.conn)
    }
}

impl ChunkSyncQueries for Tx<'_> {
    fn chunk_sync_put(&self, row: &ChunkSyncRow) -> Result<(), StorageError> {
        chunk_sync_put_on(&self.tx, row)
    }
    fn chunk_sync_get(&self, hash: ChunkId) -> Result<Option<ChunkSyncRow>, StorageError> {
        chunk_sync_get_on(&self.tx, hash)
    }
    fn chunk_sync_by_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Vec<ChunkSyncRow>, StorageError> {
        chunk_sync_by_attachment_on(&self.tx, attachment_id)
    }
    fn chunk_sync_due(
        &self,
        state: u8,
        now: i64,
        limit: u32,
    ) -> Result<Vec<ChunkSyncRow>, StorageError> {
        chunk_sync_due_on(&self.tx, state, now, limit)
    }
    fn chunk_sync_set(
        &self,
        hash: ChunkId,
        state: u8,
        attempts: u32,
        next_at: Option<i64>,
    ) -> Result<(), StorageError> {
        chunk_sync_set_on(&self.tx, hash, state, attempts, next_at)
    }
    fn chunk_sync_delete(&self, hash: ChunkId) -> Result<(), StorageError> {
        chunk_sync_delete_on(&self.tx, hash)
    }
    fn chunk_sync_delete_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<(), StorageError> {
        chunk_sync_delete_attachment_on(&self.tx, attachment_id)
    }
    fn chunk_sync_count(&self, state: u8) -> Result<u32, StorageError> {
        chunk_sync_count_on(&self.tx, state)
    }
    fn chunk_sync_count_waiting(&self) -> Result<u32, StorageError> {
        chunk_sync_count_waiting_on(&self.tx)
    }
    fn chunk_sync_count_attachment(
        &self,
        attachment_id: AttachmentId,
        states: &[u8],
    ) -> Result<u32, StorageError> {
        chunk_sync_count_attachment_on(&self.tx, attachment_id, states)
    }
    fn chunk_sync_reset(&self, from: u8, to: u8) -> Result<u32, StorageError> {
        chunk_sync_reset_on(&self.tx, from, to)
    }
    fn chunk_sync_clear(&self) -> Result<(), StorageError> {
        chunk_sync_clear_on(&self.tx)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// The columns of `chunk_sync` in the order [`ChunkSyncColumns::read`] expects.
const COLUMNS: &str = "hash, attachment_id, state, attempts, next_at";

const PUT_SQL: &str = "INSERT INTO chunk_sync (hash, attachment_id, state, attempts, next_at) \
     VALUES (?1, ?2, ?3, ?4, ?5) \
     ON CONFLICT (hash) DO UPDATE SET attachment_id = excluded.attachment_id, \
       state = excluded.state, attempts = excluded.attempts, next_at = excluded.next_at";

/// By primary key.
const SET_SQL: &str = "UPDATE chunk_sync SET state = ?2, attempts = ?3, next_at = ?4 \
     WHERE hash = ?1";

/// By the `chunk_sync_work (state, next_at)` index; the table that is due is the size of
/// one cycle's work, so the sort on `(attempts, hash)` is over few rows.
const DUE_SQL: &str = "SELECT hash, attachment_id, state, attempts, next_at FROM chunk_sync \
     WHERE state = ?1 AND (next_at IS NULL OR next_at <= ?2) \
     ORDER BY attempts, hash LIMIT ?3";

/// By the `chunk_sync_attachment (attachment_id)` index.
const BY_ATTACHMENT_SQL: &str = "SELECT hash, attachment_id, state, attempts, next_at \
     FROM chunk_sync WHERE attachment_id = ?1 ORDER BY hash";

const RESET_SQL: &str = "UPDATE chunk_sync SET state = ?2, attempts = 0, next_at = NULL \
     WHERE state = ?1";

/// A count read from SQLite as the `u32` of the API.
fn count_u32(value: i64, what: &str) -> Result<u32, StorageError> {
    u32::try_from(value).map_err(|_| StorageError::InvalidData(format!("{what} is out of range")))
}

/// Raw column values of a `chunk_sync` row.
struct ChunkSyncColumns {
    hash: Vec<u8>,
    attachment_id: Vec<u8>,
    state: i64,
    attempts: i64,
    next_at: Option<i64>,
}

impl ChunkSyncColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            hash: row.get(0)?,
            attachment_id: row.get(1)?,
            state: row.get(2)?,
            attempts: row.get(3)?,
            next_at: row.get(4)?,
        })
    }

    fn decode(self) -> Result<ChunkSyncRow, StorageError> {
        let hash = <[u8; 32]>::try_from(self.hash.as_slice()).map_err(|_| {
            StorageError::InvalidData("chunk sync hash is not 32 bytes long".to_owned())
        })?;
        Ok(ChunkSyncRow {
            hash: ChunkId::from_bytes(hash),
            attachment_id: AttachmentId::from_bytes(id_from(
                &self.attachment_id,
                "chunk sync attachment id",
            )?),
            state: u8::try_from(self.state).map_err(|_| {
                StorageError::InvalidData("chunk sync state is out of range".to_owned())
            })?,
            attempts: count_u32(self.attempts, "chunk sync attempts")?,
            next_at: self.next_at,
        })
    }
}

fn rows_from(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<ChunkSyncRow>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params, ChunkSyncColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(ChunkSyncColumns::decode).collect()
}

fn chunk_sync_put_on(conn: &Connection, row: &ChunkSyncRow) -> Result<(), StorageError> {
    conn.execute(
        PUT_SQL,
        params![
            &row.hash.as_bytes()[..],
            row.attachment_id.to_bytes(),
            i64::from(row.state),
            i64::from(row.attempts),
            row.next_at,
        ],
    )?;
    Ok(())
}

fn chunk_sync_get_on(
    conn: &Connection,
    hash: ChunkId,
) -> Result<Option<ChunkSyncRow>, StorageError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM chunk_sync WHERE hash = ?1"),
        [&hash.as_bytes()[..]],
        ChunkSyncColumns::read,
    )
    .optional()?
    .map(ChunkSyncColumns::decode)
    .transpose()
}

fn chunk_sync_by_attachment_on(
    conn: &Connection,
    attachment_id: AttachmentId,
) -> Result<Vec<ChunkSyncRow>, StorageError> {
    rows_from(conn, BY_ATTACHMENT_SQL, [attachment_id.to_bytes()])
}

fn chunk_sync_due_on(
    conn: &Connection,
    state: u8,
    now: i64,
    limit: u32,
) -> Result<Vec<ChunkSyncRow>, StorageError> {
    rows_from(
        conn,
        DUE_SQL,
        params![i64::from(state), now, i64::from(limit)],
    )
}

fn chunk_sync_set_on(
    conn: &Connection,
    hash: ChunkId,
    state: u8,
    attempts: u32,
    next_at: Option<i64>,
) -> Result<(), StorageError> {
    conn.execute(
        SET_SQL,
        params![
            &hash.as_bytes()[..],
            i64::from(state),
            i64::from(attempts),
            next_at
        ],
    )?;
    Ok(())
}

fn chunk_sync_delete_on(conn: &Connection, hash: ChunkId) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM chunk_sync WHERE hash = ?1",
        [&hash.as_bytes()[..]],
    )?;
    Ok(())
}

fn chunk_sync_delete_attachment_on(
    conn: &Connection,
    attachment_id: AttachmentId,
) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM chunk_sync WHERE attachment_id = ?1",
        [attachment_id.to_bytes()],
    )?;
    Ok(())
}

fn chunk_sync_count_on(conn: &Connection, state: u8) -> Result<u32, StorageError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM chunk_sync WHERE state = ?1",
        [i64::from(state)],
        |row| row.get(0),
    )?;
    count_u32(count, "chunk sync count")
}

fn chunk_sync_count_waiting_on(conn: &Connection) -> Result<u32, StorageError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM chunk_sync WHERE state = ?1 AND attempts > 0",
        [i64::from(STATE_DOWNLOAD)],
        |row| row.get(0),
    )?;
    count_u32(count, "chunk sync count")
}

fn chunk_sync_count_attachment_on(
    conn: &Connection,
    attachment_id: AttachmentId,
    states: &[u8],
) -> Result<u32, StorageError> {
    if states.is_empty() {
        return Ok(0);
    }
    // One grouped statement over the attachment's rows (the table is indexed by
    // attachment), summed over the listed states here: no SQL text is built from the list,
    // and a state named twice cannot count its rows twice.
    let mut stmt = conn.prepare(
        "SELECT state, count(*) FROM chunk_sync WHERE attachment_id = ?1 GROUP BY state",
    )?;
    let per_state = stmt
        .query_map([attachment_id.to_bytes()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut total: i64 = 0;
    for (state, count) in per_state {
        if states.iter().any(|wanted| i64::from(*wanted) == state) {
            total = total.saturating_add(count);
        }
    }
    count_u32(total, "chunk sync count")
}

fn chunk_sync_reset_on(conn: &Connection, from: u8, to: u8) -> Result<u32, StorageError> {
    let moved = conn.execute(RESET_SQL, params![i64::from(from), i64::from(to)])?;
    u32::try_from(moved)
        .map_err(|_| StorageError::InvalidData("chunk sync count is out of range".to_owned()))
}

fn chunk_sync_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM chunk_sync", [])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{AttachmentQueries, SyncDocState, SyncQueries, migrations};

    /// What a scenario needs of a connection or a transaction: both implement all three.
    trait Queries: ChunkSyncQueries + SyncQueries + AttachmentQueries {}
    impl<T: ChunkSyncQueries + SyncQueries + AttachmentQueries> Queries for T {}

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that
    /// both `ChunkSyncQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
    }

    /// A chunk id that starts with `first`, so that ids order by it; the other bytes make
    /// every position differ and include NUL and 0xFF.
    fn chunk(first: u8) -> ChunkId {
        ChunkId::from_bytes(std::array::from_fn(|i| {
            if i == 0 {
                first
            } else {
                first.wrapping_mul(3).wrapping_add(u8::try_from(i).unwrap())
            }
        }))
    }

    const fn attachment(n: u8) -> AttachmentId {
        AttachmentId::from_bytes([n; 16])
    }

    fn row(first: u8, attachment_n: u8, state: u8, attempts: u32) -> ChunkSyncRow {
        ChunkSyncRow {
            hash: chunk(first),
            attachment_id: attachment(attachment_n),
            state,
            attempts,
            next_at: None,
        }
    }

    fn at(row: ChunkSyncRow, next_at: i64) -> ChunkSyncRow {
        ChunkSyncRow {
            next_at: Some(next_at),
            ..row
        }
    }

    fn hashes(rows: &[ChunkSyncRow]) -> Vec<u8> {
        rows.iter().map(|row| row.hash.as_bytes()[0]).collect()
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

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

    /// The explicit indexes of a table: `(name, columns in order)`, by name.
    fn explicit_indexes(conn: &Connection, table: &str) -> Vec<(String, Vec<String>)> {
        let names: Vec<String> = conn
            .prepare("SELECT name FROM pragma_index_list(?1) WHERE origin = 'c' ORDER BY name")
            .unwrap()
            .query_map([table], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        names
            .into_iter()
            .map(|name| {
                let indexed: Vec<String> = conn
                    .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                    .unwrap()
                    .query_map([&name], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                (name, indexed)
            })
            .collect()
    }

    #[test]
    fn schema_v8_matches_the_documented_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 7).unwrap();
        let v7_tables = table_names(&conn);
        migrations().to_version(&mut conn, 8).unwrap();
        let conn = &conn;

        // One new table, nothing else.
        let new: Vec<String> = table_names(conn)
            .into_iter()
            .filter(|name| !v7_tables.contains(name))
            .collect();
        assert_eq!(new, ["chunk_sync"]);

        // SQLite does not mark a BLOB primary key NOT NULL, hence `false` for it.
        assert_eq!(
            columns(conn, "chunk_sync"),
            [
                col("hash", "BLOB", false, 1, None),
                col("attachment_id", "BLOB", true, 0, None),
                col("state", "INTEGER", true, 0, None),
                col("attempts", "INTEGER", true, 0, Some("0")),
                col("next_at", "INTEGER", false, 0, None),
            ]
        );
        // Derived bookkeeping: no foreign key, so a purge leaves the rows that tell the
        // server to delete the chunks.
        let foreign: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_foreign_key_list('chunk_sync')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(foreign, 0);
        assert_eq!(
            explicit_indexes(conn, "chunk_sync"),
            [
                (
                    "chunk_sync_attachment".to_owned(),
                    vec!["attachment_id".to_owned()]
                ),
                (
                    "chunk_sync_work".to_owned(),
                    vec!["state".to_owned(), "next_at".to_owned()]
                ),
            ]
        );
        assert_eq!(count(conn, "chunk_sync"), 0);
    }

    #[test]
    fn migrating_from_v7_adds_only_the_empty_chunk_sync_table() {
        // A v7 database with a row in every table that exists at v7 that can take one
        // without ceremony, migrated to v8 in place: the rows stay, the new table starts
        // empty.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 7).unwrap();
        conn.execute_batch(
            "INSERT INTO meta VALUES ('device_id', 'x');
             INSERT INTO attachments_index VALUES (zeroblob(16), zeroblob(16), 'n', 'm', 1, 1, 2, NULL);
             INSERT INTO attachment_refs VALUES (zeroblob(16), zeroblob(16), zeroblob(16));
             INSERT INTO sync_config VALUES ('server_url', 'u');
             INSERT INTO sync_pending VALUES (1, zeroblob(16), x'01', 1, 2);",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, crate::SCHEMA_VERSION);
        for (table, rows) in [
            ("meta", 1),
            ("attachments_index", 1),
            ("attachment_refs", 1),
            ("sync_config", 1),
            ("sync_pending", 1),
            ("chunk_sync", 0),
        ] {
            assert_eq!(count(&conn, table), rows, "{table}");
        }
    }

    // ---- put and get ----------------------------------------------------------------

    #[test]
    fn an_empty_table_knows_nothing() {
        on_db_and_tx(|q| {
            assert_eq!(q.chunk_sync_get(chunk(1)).unwrap(), None);
            assert!(
                q.chunk_sync_by_attachment(attachment(1))
                    .unwrap()
                    .is_empty()
            );
            assert!(q.chunk_sync_due(1, i64::MAX, 100).unwrap().is_empty());
            for state in [0, 1, 2, 3, 4, 255] {
                assert_eq!(q.chunk_sync_count(state).unwrap(), 0);
            }
            assert_eq!(q.chunk_sync_count_waiting().unwrap(), 0);
            assert_eq!(
                q.chunk_sync_count_attachment(attachment(1), &[1, 2, 3, 4])
                    .unwrap(),
                0
            );
            assert_eq!(q.chunk_sync_reset(1, 2).unwrap(), 0);
            q.chunk_sync_clear().unwrap();
        });
    }

    #[test]
    fn put_stores_a_row_and_replaces_it_by_hash() {
        on_db_and_tx(|q| {
            let first = at(row(7, 1, 1, 2), 1_000);
            q.chunk_sync_put(&first).unwrap();
            assert_eq!(q.chunk_sync_get(chunk(7)).unwrap(), Some(first.clone()));

            // The same hash again replaces every column, including the attachment and a
            // next_at that goes back to NULL: nothing is merged.
            let second = row(7, 9, 3, 0);
            q.chunk_sync_put(&second).unwrap();
            assert_eq!(q.chunk_sync_get(chunk(7)).unwrap(), Some(second.clone()));
            assert_eq!(q.chunk_sync_count(1).unwrap(), 0);
            assert_eq!(q.chunk_sync_count(3).unwrap(), 1);
            assert!(
                q.chunk_sync_by_attachment(attachment(1))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(q.chunk_sync_by_attachment(attachment(9)).unwrap(), [second]);

            // Another hash is another row.
            q.chunk_sync_put(&row(8, 9, 1, 0)).unwrap();
            assert_eq!(
                q.chunk_sync_count(3).unwrap() + q.chunk_sync_count(1).unwrap(),
                2
            );
        });
    }

    #[test]
    fn the_hash_is_stored_byte_for_byte() {
        on_db_and_tx(|q| {
            // NUL, 0xFF and every byte value in a chunk id survive, and a hash that
            // differs in the last byte only is another row.
            let a = ChunkSyncRow {
                hash: ChunkId::from_bytes(std::array::from_fn(|i| u8::try_from(i * 8).unwrap())),
                ..row(0, 1, 1, 0)
            };
            let mut other = a.hash.to_bytes();
            other[31] ^= 0x01;
            let b = ChunkSyncRow {
                hash: ChunkId::from_bytes(other),
                ..a.clone()
            };
            q.chunk_sync_put(&a).unwrap();
            q.chunk_sync_put(&b).unwrap();
            assert_eq!(q.chunk_sync_get(a.hash).unwrap(), Some(a.clone()));
            assert_eq!(q.chunk_sync_get(b.hash).unwrap(), Some(b.clone()));
            assert_eq!(q.chunk_sync_count(1).unwrap(), 2);
            let zeros = ChunkId::from_bytes([0; 32]);
            let ones = ChunkId::from_bytes([0xFF; 32]);
            for hash in [zeros, ones] {
                q.chunk_sync_put(&ChunkSyncRow { hash, ..a.clone() })
                    .unwrap();
                assert_eq!(q.chunk_sync_get(hash).unwrap().unwrap().hash, hash);
            }
        });
    }

    #[test]
    fn states_attempts_and_times_are_stored_as_given() {
        on_db_and_tx(|q| {
            // The state is the core's: any byte is stored; the times are any i64.
            for (first, state) in [(1u8, 0u8), (2, 1), (3, 2), (4, 3), (5, 4), (6, 255)] {
                q.chunk_sync_put(&row(first, 1, state, 0)).unwrap();
            }
            let states: Vec<u8> = q
                .chunk_sync_by_attachment(attachment(1))
                .unwrap()
                .iter()
                .map(|row| row.state)
                .collect();
            assert_eq!(states, [0, 1, 2, 3, 4, 255]);

            for (first, attempts, next_at) in [
                (10u8, u32::MAX, Some(i64::MAX)),
                (11, 0, Some(i64::MIN)),
                (12, 1, Some(-1)),
                (13, 0, Some(0)),
            ] {
                let stored = ChunkSyncRow {
                    attempts,
                    next_at,
                    ..row(first, 2, 1, 0)
                };
                q.chunk_sync_put(&stored).unwrap();
                assert_eq!(q.chunk_sync_get(chunk(first)).unwrap(), Some(stored));
            }
        });
    }

    // ---- by_attachment --------------------------------------------------------------

    #[test]
    fn by_attachment_lists_one_attachment_in_hash_order() {
        on_db_and_tx(|q| {
            // Inserted out of order; 0x80 and above sort after the low bytes (the bytes are
            // compared as unsigned, like ChunkId).
            for (first, attachment_n, state) in [
                (0xC3u8, 1u8, 3u8),
                (0x21, 1, 1),
                (0x5A, 2, 2),
                (0x84, 1, 1),
                (0x00, 1, 4),
                (0xFF, 1, 2),
                (0xE7, 3, 4),
            ] {
                q.chunk_sync_put(&row(first, attachment_n, state, 0))
                    .unwrap();
            }
            let of_one = q.chunk_sync_by_attachment(attachment(1)).unwrap();
            assert_eq!(hashes(&of_one), [0x00, 0x21, 0x84, 0xC3, 0xFF]);
            assert!(of_one.iter().all(|row| row.attachment_id == attachment(1)));
            assert_eq!(
                hashes(&q.chunk_sync_by_attachment(attachment(2)).unwrap()),
                [0x5A]
            );
            assert_eq!(
                hashes(&q.chunk_sync_by_attachment(attachment(3)).unwrap()),
                [0xE7]
            );
            assert!(
                q.chunk_sync_by_attachment(attachment(4))
                    .unwrap()
                    .is_empty()
            );
        });
    }

    #[test]
    fn the_sql_hash_order_is_the_order_of_chunk_ids() {
        on_db_and_tx(|q| {
            let mut ids: Vec<ChunkId> = (0..=255u8).step_by(5).map(chunk).collect();
            ids.push(ChunkId::from_bytes([0; 32]));
            ids.push(ChunkId::from_bytes([0xFF; 32]));
            // A pair that differs in the last byte only.
            let mut near = chunk(100).to_bytes();
            near[31] = near[31].wrapping_add(1);
            ids.push(ChunkId::from_bytes(near));
            for id in ids.iter().rev() {
                q.chunk_sync_put(&ChunkSyncRow {
                    hash: *id,
                    ..row(0, 1, 1, 0)
                })
                .unwrap();
            }
            ids.sort();
            ids.dedup();
            let listed: Vec<ChunkId> = q
                .chunk_sync_by_attachment(attachment(1))
                .unwrap()
                .iter()
                .map(|row| row.hash)
                .collect();
            assert_eq!(listed, ids);
        });
    }

    // ---- due ------------------------------------------------------------------------

    #[test]
    fn due_returns_the_rows_of_one_state_whose_time_has_come() {
        on_db_and_tx(|q| {
            for stored in [
                row(1, 1, 1, 0),               // upload, now
                at(row(2, 1, 1, 0), 500),      // upload, in the past
                at(row(3, 1, 1, 0), 1_000),    // upload, exactly now
                at(row(4, 1, 1, 0), 1_001),    // upload, one ms ahead
                at(row(5, 1, 1, 0), i64::MAX), // upload, far ahead
                row(6, 1, 2, 0),               // download, now
                at(row(7, 1, 2, 3), 2_000),    // download, ahead
                row(8, 1, 3, 0),               // synced
                row(9, 1, 4, 0),               // delete
            ] {
                q.chunk_sync_put(&stored).unwrap();
            }
            let now = 1_000;
            assert_eq!(
                hashes(&q.chunk_sync_due(1, now, 100).unwrap()),
                [1, 2, 3],
                "NULL, past and equal are due; ahead is not"
            );
            assert_eq!(hashes(&q.chunk_sync_due(2, now, 100).unwrap()), [6]);
            assert_eq!(hashes(&q.chunk_sync_due(2, 2_000, 100).unwrap()), [6, 7]);
            assert_eq!(hashes(&q.chunk_sync_due(3, now, 100).unwrap()), [8]);
            assert_eq!(hashes(&q.chunk_sync_due(4, now, 100).unwrap()), [9]);
            assert!(q.chunk_sync_due(5, now, 100).unwrap().is_empty());
            // A time before every row: only the NULLs are due.
            assert_eq!(hashes(&q.chunk_sync_due(1, i64::MIN, 100).unwrap()), [1]);
            // The whole row comes back, not only the hash.
            assert_eq!(
                q.chunk_sync_due(2, 2_000, 100).unwrap()[1],
                at(row(7, 1, 2, 3), 2_000)
            );
            assert_eq!(
                hashes(&q.chunk_sync_due(1, i64::MAX, 100).unwrap()),
                [1, 2, 3, 4, 5]
            );
        });
    }

    #[test]
    fn due_orders_by_attempts_then_hash_and_cuts_at_the_limit() {
        on_db_and_tx(|q| {
            // (hash, attempts): the hash order and the attempts order disagree.
            for (first, attempts) in [
                (0x10u8, 5u32),
                (0xA0, 0),
                (0x20, 2),
                (0x90, 2),
                (0x30, 0),
                (0xF0, 1),
                (0x05, 5),
            ] {
                q.chunk_sync_put(&row(first, 1, 2, attempts)).unwrap();
            }
            // Rows of another state never mix in.
            q.chunk_sync_put(&row(0x01, 1, 1, 0)).unwrap();
            let all = q.chunk_sync_due(2, 0, 100).unwrap();
            assert_eq!(
                hashes(&all),
                [0x30, 0xA0, 0xF0, 0x20, 0x90, 0x05, 0x10],
                "fewest attempts first, then hash (unsigned bytes)"
            );
            assert_eq!(
                q.chunk_sync_due(2, 0, 3)
                    .unwrap()
                    .iter()
                    .map(|row| row.attempts)
                    .collect::<Vec<_>>(),
                [0, 0, 1]
            );
            assert_eq!(
                hashes(&q.chunk_sync_due(2, 0, 4).unwrap()),
                [0x30, 0xA0, 0xF0, 0x20]
            );
            assert_eq!(hashes(&q.chunk_sync_due(2, 0, 1).unwrap()), [0x30]);
            assert!(q.chunk_sync_due(2, 0, 0).unwrap().is_empty(), "limit 0");
            assert_eq!(q.chunk_sync_due(2, 0, u32::MAX).unwrap().len(), 7);
        });
    }

    #[test]
    fn a_row_that_is_not_due_does_not_count_against_the_limit() {
        on_db_and_tx(|q| {
            // The earliest hashes are not due yet; the limit applies to what is due.
            for first in 1..=4u8 {
                q.chunk_sync_put(&at(row(first, 1, 1, 0), 9_000)).unwrap();
            }
            for first in 10..=12u8 {
                q.chunk_sync_put(&row(first, 1, 1, 0)).unwrap();
            }
            assert_eq!(hashes(&q.chunk_sync_due(1, 100, 2).unwrap()), [10, 11]);
        });
    }

    // ---- set ------------------------------------------------------------------------

    #[test]
    fn set_updates_the_three_columns_and_leaves_the_attachment() {
        on_db_and_tx(|q| {
            q.chunk_sync_put(&row(1, 4, 2, 0)).unwrap();
            q.chunk_sync_put(&row(2, 4, 2, 0)).unwrap();
            q.chunk_sync_set(chunk(1), 2, 3, Some(1_234)).unwrap();
            assert_eq!(
                q.chunk_sync_get(chunk(1)).unwrap(),
                Some(at(row(1, 4, 2, 3), 1_234))
            );
            q.chunk_sync_set(chunk(1), 3, 0, None).unwrap();
            assert_eq!(q.chunk_sync_get(chunk(1)).unwrap(), Some(row(1, 4, 3, 0)));
            assert_eq!(
                q.chunk_sync_get(chunk(2)).unwrap(),
                Some(row(2, 4, 2, 0)),
                "another row is untouched"
            );
            q.chunk_sync_set(chunk(1), 255, u32::MAX, Some(i64::MIN))
                .unwrap();
            assert_eq!(
                q.chunk_sync_get(chunk(1)).unwrap(),
                Some(at(row(1, 4, 255, u32::MAX), i64::MIN))
            );
        });
    }

    #[test]
    fn set_on_a_missing_row_succeeds_and_inserts_nothing() {
        on_db_and_tx(|q| {
            q.chunk_sync_set(chunk(1), 2, 1, Some(5)).unwrap();
            assert_eq!(q.chunk_sync_get(chunk(1)).unwrap(), None);
            assert_eq!(q.chunk_sync_count(2).unwrap(), 0);
            q.chunk_sync_put(&row(2, 1, 1, 0)).unwrap();
            q.chunk_sync_set(chunk(1), 2, 1, Some(5)).unwrap();
            assert_eq!(
                q.chunk_sync_by_attachment(attachment(1)).unwrap(),
                [row(2, 1, 1, 0)]
            );
        });
    }

    // ---- delete ---------------------------------------------------------------------

    #[test]
    fn delete_removes_one_row_and_a_missing_one_is_fine() {
        on_db_and_tx(|q| {
            q.chunk_sync_delete(chunk(1)).unwrap(); // none yet
            for first in [1u8, 2, 3] {
                q.chunk_sync_put(&row(first, 1, 1, 0)).unwrap();
            }
            q.chunk_sync_delete(chunk(2)).unwrap();
            q.chunk_sync_delete(chunk(2)).unwrap();
            assert_eq!(
                hashes(&q.chunk_sync_by_attachment(attachment(1)).unwrap()),
                [1, 3]
            );
            assert_eq!(q.chunk_sync_get(chunk(2)).unwrap(), None);
        });
    }

    #[test]
    fn delete_attachment_removes_the_rows_of_that_attachment_only() {
        on_db_and_tx(|q| {
            q.chunk_sync_delete_attachment(attachment(1)).unwrap(); // none yet
            for (first, attachment_n, state) in
                [(1u8, 1u8, 1u8), (2, 1, 3), (3, 2, 1), (4, 1, 4), (5, 3, 2)]
            {
                q.chunk_sync_put(&row(first, attachment_n, state, 0))
                    .unwrap();
            }
            q.chunk_sync_delete_attachment(attachment(1)).unwrap();
            q.chunk_sync_delete_attachment(attachment(1)).unwrap();
            assert!(
                q.chunk_sync_by_attachment(attachment(1))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                q.chunk_sync_by_attachment(attachment(2)).unwrap(),
                [row(3, 2, 1, 0)]
            );
            assert_eq!(
                q.chunk_sync_by_attachment(attachment(3)).unwrap(),
                [row(5, 3, 2, 0)]
            );
            assert_eq!(q.chunk_sync_count(1).unwrap(), 1);
            assert_eq!(q.chunk_sync_count(2).unwrap(), 1);
            assert_eq!(q.chunk_sync_count(3).unwrap(), 0);
            assert_eq!(q.chunk_sync_count(4).unwrap(), 0);
        });
    }

    // ---- counts ---------------------------------------------------------------------

    #[test]
    fn count_counts_one_state_whether_due_or_not() {
        on_db_and_tx(|q| {
            for stored in [
                row(1, 1, 1, 0),
                at(row(2, 1, 1, 4), i64::MAX),
                row(3, 2, 1, 0),
                row(4, 1, 2, 0),
                row(5, 1, 3, 0),
                row(6, 1, 3, 0),
                row(7, 1, 3, 0),
            ] {
                q.chunk_sync_put(&stored).unwrap();
            }
            assert_eq!(q.chunk_sync_count(1).unwrap(), 3);
            assert_eq!(q.chunk_sync_count(2).unwrap(), 1);
            assert_eq!(q.chunk_sync_count(3).unwrap(), 3);
            assert_eq!(q.chunk_sync_count(4).unwrap(), 0);
        });
    }

    #[test]
    fn count_waiting_is_the_downloads_that_were_tried() {
        on_db_and_tx(|q| {
            for stored in [
                row(1, 1, 2, 0),               // wanted, not tried yet
                row(2, 1, 2, 1),               // wanted, tried: waiting
                at(row(3, 1, 2, 5), i64::MAX), // waiting, not due
                at(row(4, 2, 2, 2), 10),       // waiting, due
                row(5, 1, 1, 3),               // an upload that failed: not waiting
                row(6, 1, 3, 1),               // synced
                row(7, 1, 4, 9),               // delete that failed: not waiting
            ] {
                q.chunk_sync_put(&stored).unwrap();
            }
            assert_eq!(q.chunk_sync_count_waiting().unwrap(), 3);
            assert_eq!(q.chunk_sync_count(2).unwrap(), 4);
            // A download that succeeds leaves the state; one that is reset has no attempts.
            q.chunk_sync_set(chunk(2), 3, 0, None).unwrap();
            q.chunk_sync_set(chunk(3), 2, 0, None).unwrap();
            assert_eq!(q.chunk_sync_count_waiting().unwrap(), 1);
        });
    }

    #[test]
    fn count_attachment_sums_the_listed_states_of_one_attachment() {
        on_db_and_tx(|q| {
            // attachment 1: two uploads, one synced; attachment 2: a download that waits
            // and a delete; attachment 3: nothing.
            for stored in [
                row(1, 1, 1, 0),
                row(2, 1, 1, 2),
                row(3, 1, 3, 0),
                at(row(4, 2, 2, 1), 77),
                row(5, 2, 4, 0),
            ] {
                q.chunk_sync_put(&stored).unwrap();
            }
            let count = |attachment_n: u8, states: &[u8]| {
                q.chunk_sync_count_attachment(attachment(attachment_n), states)
                    .unwrap()
            };
            assert_eq!(count(1, &[1]), 2);
            assert_eq!(count(1, &[3]), 1);
            assert_eq!(count(1, &[1, 3]), 3);
            assert_eq!(
                count(1, &[3, 1]),
                3,
                "the order of the list does not matter"
            );
            assert_eq!(count(1, &[1, 2, 3, 4]), 3);
            assert_eq!(count(1, &[2, 4]), 0);
            assert_eq!(count(1, &[]), 0, "an empty list counts nothing");
            assert_eq!(count(1, &[1, 1, 1]), 2, "a state listed twice counts once");
            assert_eq!(count(1, &[0, 5, 255]), 0, "states nobody uses");
            assert_eq!(count(2, &[2]), 1);
            assert_eq!(count(2, &[2, 4]), 2);
            assert_eq!(count(2, &[1, 3]), 0);
            assert_eq!(count(3, &[1, 2, 3, 4]), 0);
        });
    }

    // ---- reset ----------------------------------------------------------------------

    #[test]
    fn reset_moves_one_state_to_another_with_a_clean_back_off() {
        on_db_and_tx(|q| {
            for stored in [
                at(row(1, 1, 3, 4), 5_000), // synced -> upload
                row(2, 1, 3, 0),            // synced -> upload
                at(row(3, 2, 3, 1), 9),     // synced -> upload
                at(row(4, 1, 2, 6), 7_000), // download: stays
                at(row(5, 1, 1, 2), 8_000), // already an upload: stays as it is
                at(row(6, 1, 4, 1), 9_000), // delete: stays
            ] {
                q.chunk_sync_put(&stored).unwrap();
            }
            assert_eq!(q.chunk_sync_reset(3, 1).unwrap(), 3);
            for first in [1, 2, 3] {
                let moved = q.chunk_sync_get(chunk(first)).unwrap().unwrap();
                assert_eq!((moved.state, moved.attempts, moved.next_at), (1, 0, None));
            }
            assert_eq!(
                q.chunk_sync_get(chunk(1)).unwrap().unwrap().attachment_id,
                attachment(1),
                "the attachment stays"
            );
            assert_eq!(
                q.chunk_sync_get(chunk(3)).unwrap().unwrap().attachment_id,
                attachment(2)
            );
            assert_eq!(
                q.chunk_sync_get(chunk(4)).unwrap(),
                Some(at(row(4, 1, 2, 6), 7_000))
            );
            assert_eq!(
                q.chunk_sync_get(chunk(5)).unwrap(),
                Some(at(row(5, 1, 1, 2), 8_000))
            );
            assert_eq!(
                q.chunk_sync_get(chunk(6)).unwrap(),
                Some(at(row(6, 1, 4, 1), 9_000))
            );
            assert_eq!(q.chunk_sync_count(3).unwrap(), 0);
            assert_eq!(q.chunk_sync_count(1).unwrap(), 4);

            // Nothing left in that state: nothing moves.
            assert_eq!(q.chunk_sync_reset(3, 1).unwrap(), 0);
            assert_eq!(q.chunk_sync_reset(200, 1).unwrap(), 0);
        });
    }

    #[test]
    fn reset_to_the_same_state_only_clears_the_back_off() {
        on_db_and_tx(|q| {
            q.chunk_sync_put(&at(row(1, 1, 2, 5), 123)).unwrap();
            q.chunk_sync_put(&at(row(2, 1, 1, 5), 456)).unwrap();
            assert_eq!(q.chunk_sync_reset(2, 2).unwrap(), 1);
            assert_eq!(q.chunk_sync_get(chunk(1)).unwrap(), Some(row(1, 1, 2, 0)));
            assert_eq!(
                q.chunk_sync_get(chunk(2)).unwrap(),
                Some(at(row(2, 1, 1, 5), 456))
            );
        });
    }

    #[test]
    fn a_reset_row_is_due_at_once() {
        on_db_and_tx(|q| {
            q.chunk_sync_put(&at(row(1, 1, 3, 2), i64::MAX)).unwrap();
            assert!(q.chunk_sync_due(1, 0, 10).unwrap().is_empty());
            q.chunk_sync_reset(3, 1).unwrap();
            assert_eq!(hashes(&q.chunk_sync_due(1, i64::MIN, 10).unwrap()), [1]);
        });
    }

    // ---- clear ----------------------------------------------------------------------

    #[test]
    fn clear_removes_every_row() {
        on_db_and_tx(|q| {
            for (first, state) in [(1u8, 1u8), (2, 2), (3, 3), (4, 4)] {
                q.chunk_sync_put(&row(first, first, state, 1)).unwrap();
            }
            q.chunk_sync_clear().unwrap();
            for state in 1..=4 {
                assert_eq!(q.chunk_sync_count(state).unwrap(), 0);
            }
            assert_eq!(q.chunk_sync_get(chunk(1)).unwrap(), None);
            q.chunk_sync_clear().unwrap();
            // The table takes rows again.
            q.chunk_sync_put(&row(1, 1, 1, 0)).unwrap();
            assert_eq!(q.chunk_sync_count(1).unwrap(), 1);
        });
    }

    // ---- what it is not tied to -----------------------------------------------------

    #[test]
    fn the_table_is_independent_of_the_attachment_index_and_of_the_epoch_reset() {
        on_db_and_tx(|q| {
            q.chunk_sync_put(&at(row(1, 1, 1, 2), 99)).unwrap();
            q.chunk_sync_put(&row(2, 1, 4, 0)).unwrap();

            // No foreign key and no cascade: removing the attachment's index row (a purge)
            // leaves the rows that ask the server to delete its chunks.
            q.attachment_delete(attachment(1)).unwrap();
            assert_eq!(q.chunk_sync_count(1).unwrap(), 1);
            assert_eq!(q.chunk_sync_count(4).unwrap(), 1);

            // The reset of the sync tables for a new server epoch does not reach the
            // chunk table: the core resets it with `chunk_sync_reset`.
            q.sync_doc_state_put(&SyncDocState {
                doc_id: kb_types::DocId::from_bytes([1; 16]),
                pushed_up_to_seq: 5,
                next_device_seq: 2,
                snapshot_covers: None,
                entries_since_snapshot: 1,
                bytes_since_snapshot: 1,
                remote_snapshot_covers: None,
            })
            .unwrap();
            q.sync_doc_state_reset_all().unwrap();
            q.keyring_mark_all_unpublished().unwrap();
            q.device_log_mark_all_unpublished().unwrap();
            q.sync_seen_clear().unwrap();
            q.sync_heads_clear().unwrap();
            q.sync_pending_clear().unwrap();
            assert_eq!(
                q.chunk_sync_get(chunk(1)).unwrap(),
                Some(at(row(1, 1, 1, 2), 99))
            );
            assert_eq!(q.chunk_sync_get(chunk(2)).unwrap(), Some(row(2, 1, 4, 0)));
        });
    }

    #[test]
    fn a_transaction_that_rolls_back_leaves_the_table_as_it_was() {
        let mut db = new_db();
        db.chunk_sync_put(&row(1, 1, 1, 0)).unwrap();
        let result: Result<(), StorageError> = db.transaction(|tx| {
            tx.chunk_sync_put(&row(2, 1, 1, 0))?;
            tx.chunk_sync_set(chunk(1), 3, 0, None)?;
            tx.chunk_sync_delete_attachment(attachment(9))?;
            Err(StorageError::InvalidData("stop".to_owned()))
        });
        assert!(result.is_err());
        assert_eq!(
            db.chunk_sync_by_attachment(attachment(1)).unwrap(),
            [row(1, 1, 1, 0)]
        );
    }

    // ---- damaged rows and errors ----------------------------------------------------

    /// `true` when the read failed with an `InvalidData` naming `what`; `false` when it
    /// found nothing; anything else is a test failure.
    fn damaged<T: std::fmt::Debug>(result: Result<T, StorageError>, what: &str) -> bool {
        match result {
            Err(StorageError::InvalidData(message)) if message.contains(what) => true,
            Err(other) => panic!("{what}: unexpected error {other}"),
            Ok(_) => false,
        }
    }

    #[test]
    fn a_damaged_row_is_a_typed_error_and_never_a_wrong_value() {
        for (what, insert) in [
            (
                "chunk sync hash",
                "INSERT INTO chunk_sync VALUES (x'0102', zeroblob(16), 1, 0, NULL)",
            ),
            (
                "chunk sync attachment id",
                "INSERT INTO chunk_sync VALUES (zeroblob(32), x'01', 1, 0, NULL)",
            ),
            (
                "chunk sync state",
                "INSERT INTO chunk_sync VALUES (zeroblob(32), zeroblob(16), 256, 0, NULL)",
            ),
            (
                "chunk sync state",
                "INSERT INTO chunk_sync VALUES (zeroblob(32), zeroblob(16), -1, 0, NULL)",
            ),
            (
                "chunk sync attempts",
                "INSERT INTO chunk_sync VALUES (zeroblob(32), zeroblob(16), 1, -1, NULL)",
            ),
            (
                "chunk sync attempts",
                "INSERT INTO chunk_sync VALUES (zeroblob(32), zeroblob(16), 1, 4294967296, NULL)",
            ),
        ] {
            let db = new_db();
            db.connection().execute(insert, []).unwrap();
            let zero_hash = ChunkId::from_bytes([0; 32]);
            let zero_attachment = AttachmentId::from_bytes([0; 16]);
            // Every read that reaches the row refuses it; the ones that cannot reach it
            // (the key is the damaged part) find nothing.
            let refused = [
                damaged(db.chunk_sync_get(zero_hash), what),
                damaged(db.chunk_sync_by_attachment(zero_attachment), what),
                damaged(db.chunk_sync_due(1, i64::MAX, 10), what),
            ];
            assert!(refused.contains(&true), "{what}: no read refused the row");
            // The writes that do not read it still work, and the row can be removed.
            db.chunk_sync_put(&row(1, 1, 1, 0)).unwrap();
            assert_eq!(db.chunk_sync_get(chunk(1)).unwrap(), Some(row(1, 1, 1, 0)));
            db.chunk_sync_clear().unwrap();
            assert_eq!(count(db.connection(), "chunk_sync"), 0, "{what}");
        }
    }

    #[test]
    fn errors_never_contain_ids() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO chunk_sync VALUES (x'DEADBEEF', x'C0FFEE', 1, 0, NULL)",
                [],
            )
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO chunk_sync VALUES (zeroblob(32), x'C0FFEE', 1, 0, NULL)",
                [],
            )
            .unwrap();
        for text in [
            db.chunk_sync_due(1, 0, 10).unwrap_err().to_string(),
            db.chunk_sync_get(ChunkId::from_bytes([0; 32]))
                .unwrap_err()
                .to_string(),
        ] {
            let text = text.to_lowercase();
            assert!(
                !text.contains("deadbeef") && !text.contains("c0ffee"),
                "{text}"
            );
        }
    }
}
