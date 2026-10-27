//! Document persistence and the derived page index (schema v2,
//! `docs/data-model/sqlite-schema.md`, `docs/development/milestone-02.md`).
//!
//! The tables (migration `002_documents.sql`): `docs` (one row per CRDT document),
//! `doc_snapshots` (at most one snapshot per document), `doc_updates` (the update log
//! since that snapshot, numbered by `local_seq`) and `pages_index` (the read model of
//! the structure documents). The snapshot and update rows of a document go with it
//! (`ON DELETE CASCADE`).
//!
//! Blobs are opaque to this crate (the `kb-doc` envelope is inside them). Every
//! method exists on both [`Db`] and [`Tx`] through [`DocQueries`], so the core can
//! group an update append with the index rows it affects in one transaction. Both
//! impls run the same private functions over a plain connection (a transaction
//! derefs to one), so a method behaves identically in either place.

use kb_types::{DocId, PageId, SpaceId};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use crate::{Db, StorageError};

/// Document kinds as stored in `docs.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DocKindCode {
    /// Per space: page tree.
    Structure = 0,
    /// Per page: content.
    Page = 1,
    /// Per workspace: space order.
    Account = 2,
    /// Per page: the signed record stream of its comments (M13 slice 2, ADR-0048). The document id
    /// derives from the page id (`kb_types::comments::COMMENT_STREAM_DOMAIN`), its updates are the
    /// records' blobs (`kb_doc::ENGINE_RECORDS`) and the `comments` table indexes them.
    Comments = 3,
}

impl DocKindCode {
    /// The number stored in `docs.kind`. It is on disk: never renumber.
    const fn code(self) -> i64 {
        match self {
            Self::Structure => 0,
            Self::Page => 1,
            Self::Account => 2,
            Self::Comments => 3,
        }
    }

    fn from_code(code: i64) -> Result<Self, StorageError> {
        match code {
            0 => Ok(Self::Structure),
            1 => Ok(Self::Page),
            2 => Ok(Self::Account),
            3 => Ok(Self::Comments),
            other => Err(StorageError::InvalidData(format!(
                "unknown document kind {other}"
            ))),
        }
    }
}

/// A `docs` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRow {
    /// Document id.
    pub doc_id: DocId,
    /// Kind.
    pub kind: DocKindCode,
    /// Owning space for structure and page documents; `None` for the account document.
    pub space_id: Option<SpaceId>,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// Envelope format version the document was created with.
    pub format_version: u16,
    /// Oldest reader able to interpret it.
    pub min_reader_version: u16,
}

/// Where an update originated (`doc_updates.origin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UpdateOrigin {
    /// This device's user action.
    Local = 0,
    /// Received through sync.
    Remote = 1,
    /// Produced by the core itself (migrations, repairs, undo).
    Core = 2,
}

impl UpdateOrigin {
    /// The number stored in `doc_updates.origin`. It is on disk: never renumber.
    const fn code(self) -> i64 {
        match self {
            Self::Local => 0,
            Self::Remote => 1,
            Self::Core => 2,
        }
    }
}

/// Update log statistics since the latest snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateStats {
    /// Number of update rows.
    pub count: u64,
    /// Total blob bytes.
    pub bytes: u64,
    /// Highest `local_seq`, if any.
    pub last_seq: Option<i64>,
}

/// A `pages_index` row: derived read model of a structure document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageIndexRow {
    /// Page id.
    pub page_id: PageId,
    /// Owning space.
    pub space_id: SpaceId,
    /// Live parent, `None` at the root or when trashed.
    pub parent_id: Option<PageId>,
    /// Title.
    pub title: String,
    /// Set for every page inside a trashed subtree.
    pub trashed_at: Option<i64>,
    /// Last metadata change, Unix ms.
    pub updated_at: i64,
    /// The page is a template or inside one (schema v20, ADR-0056): the lists that are not the tree
    /// (search, backlinks, reminders, the semantic index, MCP) leave it out.
    pub in_template: bool,
}

/// A snapshot as `(blob, version)`: the blob first, then its encoded version.
type SnapshotParts = (Vec<u8>, Vec<u8>);

/// Queries available on a connection or inside a transaction.
///
/// On a [`Db`] every method is atomic by itself (the two batch methods,
/// [`pages_index_upsert`](Self::pages_index_upsert) and
/// [`pages_index_delete`](Self::pages_index_delete), commit once, all rows or none);
/// to make several calls one unit, run them inside [`Db::transaction`].
pub trait DocQueries {
    /// Inserts a document row. Fails with `InvalidData` on a duplicate id; the existing
    /// row is left as it was.
    fn doc_insert(&self, row: &DocRow) -> Result<(), StorageError>;
    /// One document row.
    fn doc_get(&self, doc_id: DocId) -> Result<Option<DocRow>, StorageError>;
    /// All documents of a kind, oldest first (`created_at`, then `doc_id`).
    fn docs_by_kind(&self, kind: DocKindCode) -> Result<Vec<DocRow>, StorageError>;
    /// The document of `kind` that belongs to a space, if any (the structure document of
    /// a space for [`DocKindCode::Structure`]). The oldest one if several claim the
    /// space. The account document belongs to no space.
    fn doc_for_space(
        &self,
        space_id: SpaceId,
        kind: DocKindCode,
    ) -> Result<Option<DocRow>, StorageError>;
    /// Deletes a document with its snapshot and updates. Succeeds when the document
    /// does not exist.
    fn doc_delete(&self, doc_id: DocId) -> Result<(), StorageError>;
    /// Points a document at another space (a page that moved, ADR-0038) and nothing else:
    /// kind, times, snapshot and updates stay. Answers whether a row was found; an unknown
    /// document is not an error.
    fn doc_set_space(&self, doc_id: DocId, space_id: SpaceId) -> Result<bool, StorageError>;

    /// Latest snapshot as `(blob, version)`: the blob first, then its encoded version.
    fn snapshot_get(&self, doc_id: DocId) -> Result<Option<SnapshotParts>, StorageError>;
    /// Inserts or replaces the snapshot.
    fn snapshot_put(
        &self,
        doc_id: DocId,
        version: &[u8],
        blob: &[u8],
        now_ms: i64,
    ) -> Result<(), StorageError>;

    /// The update blobs that follow the snapshot, in `local_seq` order. Compaction
    /// deletes the updates a snapshot covers, so this is every update row of the
    /// document.
    fn updates_after_snapshot(&self, doc_id: DocId) -> Result<Vec<Vec<u8>>, StorageError>;
    /// Appends an update and returns its `local_seq`: one more than the highest sequence
    /// number the document has (1 for its first update).
    fn update_append(
        &self,
        doc_id: DocId,
        blob: &[u8],
        origin: UpdateOrigin,
        now_ms: i64,
    ) -> Result<i64, StorageError>;
    /// Deletes updates with `local_seq <= up_to` (after a snapshot covered them) and
    /// returns how many rows went.
    fn updates_delete_up_to(&self, doc_id: DocId, up_to: i64) -> Result<u64, StorageError>;
    /// Count, bytes and last sequence of the updates since the snapshot.
    fn update_stats(&self, doc_id: DocId) -> Result<UpdateStats, StorageError>;

    /// Inserts or replaces index rows (by page id).
    fn pages_index_upsert(&self, rows: &[PageIndexRow]) -> Result<(), StorageError>;
    /// Removes index rows; ids that are not indexed are ignored.
    fn pages_index_delete(&self, page_ids: &[PageId]) -> Result<(), StorageError>;
    /// One index row.
    fn pages_index_get(&self, page_id: PageId) -> Result<Option<PageIndexRow>, StorageError>;
    /// All rows of a space (live and trashed), ordered by page id.
    fn pages_index_by_space(&self, space_id: SpaceId) -> Result<Vec<PageIndexRow>, StorageError>;
    /// Live (not trashed) row count of a space.
    fn pages_index_live_count(&self, space_id: SpaceId) -> Result<u32, StorageError>;
    /// How many rows of a space (live or trashed) are in a template (`in_template = 1`).
    fn pages_index_template_count(&self, space_id: SpaceId) -> Result<u32, StorageError>;
    /// Removes every row of a space (before a rebuild or a space delete).
    fn pages_index_clear_space(&self, space_id: SpaceId) -> Result<(), StorageError>;
}

/// A transaction handle exposing [`DocQueries`]; commits when the closure given to
/// [`Db::transaction`] returns `Ok`, rolls back otherwise.
#[derive(Debug)]
pub struct Tx<'c> {
    pub(crate) tx: Transaction<'c>,
}

impl Db {
    /// Runs `f` inside one SQLite transaction.
    ///
    /// The transaction commits when `f` returns `Ok` (its value is passed through). It
    /// rolls back when `f` returns `Err`, which is passed through unchanged, and when
    /// `f` panics (the transaction is rolled back as it is dropped).
    ///
    /// It starts with `BEGIN IMMEDIATE`: the write lock is taken up front and held to
    /// the end, so a statement in the middle never fails because another writer got in
    /// first, and the caller that appends an update owns the log until it commits.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&Tx<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let tx = Tx {
            tx: self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?,
        };
        let value = f(&tx)?;
        tx.tx.commit()?;
        Ok(value)
    }
}

impl DocQueries for Db {
    fn doc_insert(&self, row: &DocRow) -> Result<(), StorageError> {
        doc_insert_on(&self.conn, row)
    }
    fn doc_get(&self, doc_id: DocId) -> Result<Option<DocRow>, StorageError> {
        doc_get_on(&self.conn, doc_id)
    }
    fn docs_by_kind(&self, kind: DocKindCode) -> Result<Vec<DocRow>, StorageError> {
        docs_by_kind_on(&self.conn, kind)
    }
    fn doc_for_space(
        &self,
        space_id: SpaceId,
        kind: DocKindCode,
    ) -> Result<Option<DocRow>, StorageError> {
        doc_for_space_on(&self.conn, space_id, kind)
    }
    fn doc_delete(&self, doc_id: DocId) -> Result<(), StorageError> {
        doc_delete_on(&self.conn, doc_id)
    }
    fn doc_set_space(&self, doc_id: DocId, space_id: SpaceId) -> Result<bool, StorageError> {
        doc_set_space_on(&self.conn, doc_id, space_id)
    }
    fn snapshot_get(&self, doc_id: DocId) -> Result<Option<SnapshotParts>, StorageError> {
        snapshot_get_on(&self.conn, doc_id)
    }
    fn snapshot_put(
        &self,
        doc_id: DocId,
        version: &[u8],
        blob: &[u8],
        now_ms: i64,
    ) -> Result<(), StorageError> {
        snapshot_put_on(&self.conn, doc_id, version, blob, now_ms)
    }
    fn updates_after_snapshot(&self, doc_id: DocId) -> Result<Vec<Vec<u8>>, StorageError> {
        updates_after_snapshot_on(&self.conn, doc_id)
    }
    fn update_append(
        &self,
        doc_id: DocId,
        blob: &[u8],
        origin: UpdateOrigin,
        now_ms: i64,
    ) -> Result<i64, StorageError> {
        update_append_on(&self.conn, doc_id, blob, origin, now_ms)
    }
    fn updates_delete_up_to(&self, doc_id: DocId, up_to: i64) -> Result<u64, StorageError> {
        updates_delete_up_to_on(&self.conn, doc_id, up_to)
    }
    fn update_stats(&self, doc_id: DocId) -> Result<UpdateStats, StorageError> {
        update_stats_on(&self.conn, doc_id)
    }
    fn pages_index_upsert(&self, rows: &[PageIndexRow]) -> Result<(), StorageError> {
        if rows.len() > 1 {
            atomically(&self.conn, |conn| pages_index_upsert_on(conn, rows))
        } else {
            pages_index_upsert_on(&self.conn, rows)
        }
    }
    fn pages_index_delete(&self, page_ids: &[PageId]) -> Result<(), StorageError> {
        if page_ids.len() > 1 {
            atomically(&self.conn, |conn| pages_index_delete_on(conn, page_ids))
        } else {
            pages_index_delete_on(&self.conn, page_ids)
        }
    }
    fn pages_index_get(&self, page_id: PageId) -> Result<Option<PageIndexRow>, StorageError> {
        pages_index_get_on(&self.conn, page_id)
    }
    fn pages_index_by_space(&self, space_id: SpaceId) -> Result<Vec<PageIndexRow>, StorageError> {
        pages_index_by_space_on(&self.conn, space_id)
    }
    fn pages_index_live_count(&self, space_id: SpaceId) -> Result<u32, StorageError> {
        pages_index_live_count_on(&self.conn, space_id)
    }
    fn pages_index_template_count(&self, space_id: SpaceId) -> Result<u32, StorageError> {
        pages_index_template_count_on(&self.conn, space_id)
    }
    fn pages_index_clear_space(&self, space_id: SpaceId) -> Result<(), StorageError> {
        pages_index_clear_space_on(&self.conn, space_id)
    }
}

impl DocQueries for Tx<'_> {
    fn doc_insert(&self, row: &DocRow) -> Result<(), StorageError> {
        doc_insert_on(&self.tx, row)
    }
    fn doc_get(&self, doc_id: DocId) -> Result<Option<DocRow>, StorageError> {
        doc_get_on(&self.tx, doc_id)
    }
    fn docs_by_kind(&self, kind: DocKindCode) -> Result<Vec<DocRow>, StorageError> {
        docs_by_kind_on(&self.tx, kind)
    }
    fn doc_for_space(
        &self,
        space_id: SpaceId,
        kind: DocKindCode,
    ) -> Result<Option<DocRow>, StorageError> {
        doc_for_space_on(&self.tx, space_id, kind)
    }
    fn doc_delete(&self, doc_id: DocId) -> Result<(), StorageError> {
        doc_delete_on(&self.tx, doc_id)
    }
    fn doc_set_space(&self, doc_id: DocId, space_id: SpaceId) -> Result<bool, StorageError> {
        doc_set_space_on(&self.tx, doc_id, space_id)
    }
    fn snapshot_get(&self, doc_id: DocId) -> Result<Option<SnapshotParts>, StorageError> {
        snapshot_get_on(&self.tx, doc_id)
    }
    fn snapshot_put(
        &self,
        doc_id: DocId,
        version: &[u8],
        blob: &[u8],
        now_ms: i64,
    ) -> Result<(), StorageError> {
        snapshot_put_on(&self.tx, doc_id, version, blob, now_ms)
    }
    fn updates_after_snapshot(&self, doc_id: DocId) -> Result<Vec<Vec<u8>>, StorageError> {
        updates_after_snapshot_on(&self.tx, doc_id)
    }
    fn update_append(
        &self,
        doc_id: DocId,
        blob: &[u8],
        origin: UpdateOrigin,
        now_ms: i64,
    ) -> Result<i64, StorageError> {
        update_append_on(&self.tx, doc_id, blob, origin, now_ms)
    }
    fn updates_delete_up_to(&self, doc_id: DocId, up_to: i64) -> Result<u64, StorageError> {
        updates_delete_up_to_on(&self.tx, doc_id, up_to)
    }
    fn update_stats(&self, doc_id: DocId) -> Result<UpdateStats, StorageError> {
        update_stats_on(&self.tx, doc_id)
    }
    fn pages_index_upsert(&self, rows: &[PageIndexRow]) -> Result<(), StorageError> {
        pages_index_upsert_on(&self.tx, rows)
    }
    fn pages_index_delete(&self, page_ids: &[PageId]) -> Result<(), StorageError> {
        pages_index_delete_on(&self.tx, page_ids)
    }
    fn pages_index_get(&self, page_id: PageId) -> Result<Option<PageIndexRow>, StorageError> {
        pages_index_get_on(&self.tx, page_id)
    }
    fn pages_index_by_space(&self, space_id: SpaceId) -> Result<Vec<PageIndexRow>, StorageError> {
        pages_index_by_space_on(&self.tx, space_id)
    }
    fn pages_index_live_count(&self, space_id: SpaceId) -> Result<u32, StorageError> {
        pages_index_live_count_on(&self.tx, space_id)
    }
    fn pages_index_template_count(&self, space_id: SpaceId) -> Result<u32, StorageError> {
        pages_index_template_count_on(&self.tx, space_id)
    }
    fn pages_index_clear_space(&self, space_id: SpaceId) -> Result<(), StorageError> {
        pages_index_clear_space_on(&self.tx, space_id)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------
//
// Ids are bound as 16-byte blobs; blobs are bound and read as opaque bytes. Rows are
// read as raw column values first and validated afterwards, so a damaged row is a
// typed `InvalidData` error and never a panic or a silently wrong value.

/// Runs `f` as one atomic unit on a bare connection: one `BEGIN IMMEDIATE ... COMMIT`,
/// rolled back if `f` fails. The `Db` impl uses it for the batch methods, so that a
/// failure half way leaves nothing behind and N rows cost one commit instead of N. A
/// [`Tx`] needs no such wrapper: the surrounding transaction already is the unit.
pub(crate) fn atomically<T>(
    conn: &Connection,
    f: impl FnOnce(&Connection) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let value = f(&tx)?;
    tx.commit()?;
    Ok(value)
}

/// The columns of `docs` in the order [`DocColumns::read`] expects.
const DOC_COLUMNS: &str = "doc_id, kind, space_id, created_at, format_version, min_reader_version";

/// The columns of `pages_index` in the order [`PageColumns::read`] expects.
const PAGE_COLUMNS: &str =
    "page_id, space_id, parent_id, title, trashed_at, updated_at, in_template";

/// Raw column values of a `docs` row.
struct DocColumns {
    doc_id: Vec<u8>,
    kind: i64,
    space_id: Option<Vec<u8>>,
    created_at: i64,
    format_version: i64,
    min_reader_version: i64,
}

impl DocColumns {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            doc_id: row.get(0)?,
            kind: row.get(1)?,
            space_id: row.get(2)?,
            created_at: row.get(3)?,
            format_version: row.get(4)?,
            min_reader_version: row.get(5)?,
        })
    }

    fn decode(self) -> Result<DocRow, StorageError> {
        Ok(DocRow {
            doc_id: DocId::from_bytes(id_from(&self.doc_id, "document id")?),
            kind: DocKindCode::from_code(self.kind)?,
            space_id: self
                .space_id
                .as_deref()
                .map(|bytes| id_from(bytes, "document space id").map(SpaceId::from_bytes))
                .transpose()?,
            created_at: self.created_at,
            format_version: version_from(self.format_version, "document format version")?,
            min_reader_version: version_from(
                self.min_reader_version,
                "document minimum reader version",
            )?,
        })
    }
}

/// Raw column values of a `pages_index` row.
struct PageColumns {
    page_id: Vec<u8>,
    space_id: Vec<u8>,
    parent_id: Option<Vec<u8>>,
    title: String,
    trashed_at: Option<i64>,
    updated_at: i64,
    in_template: i64,
}

impl PageColumns {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            page_id: row.get(0)?,
            space_id: row.get(1)?,
            parent_id: row.get(2)?,
            title: row.get(3)?,
            trashed_at: row.get(4)?,
            updated_at: row.get(5)?,
            in_template: row.get(6)?,
        })
    }

    fn decode(self) -> Result<PageIndexRow, StorageError> {
        Ok(PageIndexRow {
            page_id: PageId::from_bytes(id_from(&self.page_id, "indexed page id")?),
            space_id: SpaceId::from_bytes(id_from(&self.space_id, "indexed page space id")?),
            parent_id: self
                .parent_id
                .as_deref()
                .map(|bytes| id_from(bytes, "indexed page parent id").map(PageId::from_bytes))
                .transpose()?,
            title: self.title,
            trashed_at: self.trashed_at,
            updated_at: self.updated_at,
            in_template: self.in_template != 0,
        })
    }
}

pub(crate) fn id_from(bytes: &[u8], what: &str) -> Result<[u8; 16], StorageError> {
    <[u8; 16]>::try_from(bytes)
        .map_err(|_| StorageError::InvalidData(format!("{what} is not 16 bytes long")))
}

fn version_from(value: i64, what: &str) -> Result<u16, StorageError> {
    u16::try_from(value).map_err(|_| StorageError::InvalidData(format!("{what} is out of range")))
}

/// A count or size read from SQLite, which is never negative.
fn count_from(value: i64, what: &str) -> Result<u64, StorageError> {
    u64::try_from(value).map_err(|_| StorageError::InvalidData(format!("{what} is out of range")))
}

fn doc_insert_on(conn: &Connection, row: &DocRow) -> Result<(), StorageError> {
    // DO NOTHING rather than a failing INSERT: a duplicate id is reported as a typed
    // error below and the existing row is not touched.
    let inserted = conn.execute(
        "INSERT INTO docs (doc_id, kind, space_id, created_at, format_version, min_reader_version) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(doc_id) DO NOTHING",
        params![
            row.doc_id.to_bytes(),
            row.kind.code(),
            row.space_id.map(SpaceId::to_bytes),
            row.created_at,
            i64::from(row.format_version),
            i64::from(row.min_reader_version),
        ],
    )?;
    if inserted == 0 {
        return Err(StorageError::InvalidData(
            "a document with this id already exists".to_owned(),
        ));
    }
    Ok(())
}

fn doc_get_on(conn: &Connection, doc_id: DocId) -> Result<Option<DocRow>, StorageError> {
    conn.query_row(
        &format!("SELECT {DOC_COLUMNS} FROM docs WHERE doc_id = ?1"),
        [doc_id.to_bytes()],
        DocColumns::read,
    )
    .optional()?
    .map(DocColumns::decode)
    .transpose()
}

fn docs_by_kind_on(conn: &Connection, kind: DocKindCode) -> Result<Vec<DocRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOC_COLUMNS} FROM docs WHERE kind = ?1 ORDER BY created_at, doc_id"
    ))?;
    let rows = stmt
        .query_map([kind.code()], DocColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(DocColumns::decode).collect()
}

/// Every document row, oldest first (`created_at`, then `doc_id`) like
/// [`docs_by_kind_on`]. For the backup, through [`crate::SecurityQueries::docs_all`].
pub(crate) fn docs_all_on(conn: &Connection) -> Result<Vec<DocRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DOC_COLUMNS} FROM docs ORDER BY created_at, doc_id"
    ))?;
    let rows = stmt
        .query_map([], DocColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(DocColumns::decode).collect()
}

fn doc_for_space_on(
    conn: &Connection,
    space_id: SpaceId,
    kind: DocKindCode,
) -> Result<Option<DocRow>, StorageError> {
    conn.query_row(
        &format!(
            "SELECT {DOC_COLUMNS} FROM docs WHERE space_id = ?1 AND kind = ?2 \
             ORDER BY created_at, doc_id LIMIT 1"
        ),
        params![space_id.to_bytes(), kind.code()],
        DocColumns::read,
    )
    .optional()?
    .map(DocColumns::decode)
    .transpose()
}

fn doc_delete_on(conn: &Connection, doc_id: DocId) -> Result<(), StorageError> {
    // The snapshot and the updates follow through ON DELETE CASCADE (foreign keys are
    // switched on by `Db::new`).
    conn.execute("DELETE FROM docs WHERE doc_id = ?1", [doc_id.to_bytes()])?;
    Ok(())
}

fn doc_set_space_on(
    conn: &Connection,
    doc_id: DocId,
    space_id: SpaceId,
) -> Result<bool, StorageError> {
    let changed = conn.execute(
        "UPDATE docs SET space_id = ?2 WHERE doc_id = ?1",
        params![doc_id.to_bytes(), space_id.to_bytes()],
    )?;
    Ok(changed > 0)
}

fn snapshot_get_on(
    conn: &Connection,
    doc_id: DocId,
) -> Result<Option<SnapshotParts>, StorageError> {
    Ok(conn
        .query_row(
            "SELECT blob, version FROM doc_snapshots WHERE doc_id = ?1",
            [doc_id.to_bytes()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn snapshot_put_on(
    conn: &Connection,
    doc_id: DocId,
    version: &[u8],
    blob: &[u8],
    now_ms: i64,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO doc_snapshots (doc_id, version, blob, created_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(doc_id) DO UPDATE SET \
           version = excluded.version, blob = excluded.blob, created_at = excluded.created_at",
        params![doc_id.to_bytes(), version, blob, now_ms],
    )?;
    Ok(())
}

fn updates_after_snapshot_on(
    conn: &Connection,
    doc_id: DocId,
) -> Result<Vec<Vec<u8>>, StorageError> {
    let mut stmt =
        conn.prepare("SELECT blob FROM doc_updates WHERE doc_id = ?1 ORDER BY local_seq")?;
    let blobs = stmt
        .query_map([doc_id.to_bytes()], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(blobs)
}

fn update_append_on(
    conn: &Connection,
    doc_id: DocId,
    blob: &[u8],
    origin: UpdateOrigin,
    now_ms: i64,
) -> Result<i64, StorageError> {
    // One statement computes the next number and inserts, so it is atomic even outside
    // a transaction. `query_one` runs it to completion (not just to the first returned
    // row), which surfaces an error that only shows up when the statement finishes.
    let local_seq = conn.query_one(
        "INSERT INTO doc_updates (doc_id, local_seq, blob, origin, created_at) \
         SELECT ?1, COALESCE(MAX(local_seq), 0) + 1, ?2, ?3, ?4 \
         FROM doc_updates WHERE doc_id = ?1 \
         RETURNING local_seq",
        params![doc_id.to_bytes(), blob, origin.code(), now_ms],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(local_seq)
}

fn updates_delete_up_to_on(
    conn: &Connection,
    doc_id: DocId,
    up_to: i64,
) -> Result<u64, StorageError> {
    let deleted = conn.execute(
        "DELETE FROM doc_updates WHERE doc_id = ?1 AND local_seq <= ?2",
        params![doc_id.to_bytes(), up_to],
    )?;
    u64::try_from(deleted)
        .map_err(|_| StorageError::InvalidData("deleted row count is out of range".to_owned()))
}

fn update_stats_on(conn: &Connection, doc_id: DocId) -> Result<UpdateStats, StorageError> {
    let (count, bytes, last_seq) = conn.query_one(
        "SELECT count(*), COALESCE(SUM(length(blob)), 0), MAX(local_seq) \
         FROM doc_updates WHERE doc_id = ?1",
        [doc_id.to_bytes()],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        },
    )?;
    Ok(UpdateStats {
        count: count_from(count, "update count")?,
        bytes: count_from(bytes, "update size")?,
        last_seq,
    })
}

fn pages_index_upsert_on(conn: &Connection, rows: &[PageIndexRow]) -> Result<(), StorageError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "INSERT INTO pages_index \
           (page_id, space_id, parent_id, title, trashed_at, updated_at, in_template) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(page_id) DO UPDATE SET \
           space_id = excluded.space_id, parent_id = excluded.parent_id, \
           title = excluded.title, trashed_at = excluded.trashed_at, \
           updated_at = excluded.updated_at, in_template = excluded.in_template",
    )?;
    for row in rows {
        stmt.execute(params![
            row.page_id.to_bytes(),
            row.space_id.to_bytes(),
            row.parent_id.map(PageId::to_bytes),
            row.title,
            row.trashed_at,
            row.updated_at,
            i64::from(row.in_template),
        ])?;
    }
    Ok(())
}

fn pages_index_delete_on(conn: &Connection, page_ids: &[PageId]) -> Result<(), StorageError> {
    if page_ids.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare("DELETE FROM pages_index WHERE page_id = ?1")?;
    for page_id in page_ids {
        stmt.execute([page_id.to_bytes()])?;
    }
    Ok(())
}

fn pages_index_get_on(
    conn: &Connection,
    page_id: PageId,
) -> Result<Option<PageIndexRow>, StorageError> {
    conn.query_row(
        &format!("SELECT {PAGE_COLUMNS} FROM pages_index WHERE page_id = ?1"),
        [page_id.to_bytes()],
        PageColumns::read,
    )
    .optional()?
    .map(PageColumns::decode)
    .transpose()
}

fn pages_index_by_space_on(
    conn: &Connection,
    space_id: SpaceId,
) -> Result<Vec<PageIndexRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PAGE_COLUMNS} FROM pages_index WHERE space_id = ?1 ORDER BY page_id"
    ))?;
    let rows = stmt
        .query_map([space_id.to_bytes()], PageColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(PageColumns::decode).collect()
}

fn pages_index_live_count_on(conn: &Connection, space_id: SpaceId) -> Result<u32, StorageError> {
    let count: i64 = conn.query_one(
        "SELECT count(*) FROM pages_index WHERE space_id = ?1 AND trashed_at IS NULL",
        [space_id.to_bytes()],
        |row| row.get(0),
    )?;
    u32::try_from(count)
        .map_err(|_| StorageError::InvalidData("live page count is out of range".to_owned()))
}

fn pages_index_template_count_on(
    conn: &Connection,
    space_id: SpaceId,
) -> Result<u32, StorageError> {
    let count: i64 = conn.query_one(
        "SELECT count(*) FROM pages_index WHERE space_id = ?1 AND in_template = 1",
        [space_id.to_bytes()],
        |row| row.get(0),
    )?;
    u32::try_from(count)
        .map_err(|_| StorageError::InvalidData("template page count is out of range".to_owned()))
}

fn pages_index_clear_space_on(conn: &Connection, space_id: SpaceId) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM pages_index WHERE space_id = ?1",
        [space_id.to_bytes()],
    )?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    const fn doc(n: u8) -> DocId {
        DocId::from_bytes([n; 16])
    }

    const fn space(n: u8) -> SpaceId {
        SpaceId::from_bytes([n; 16])
    }

    const fn page(n: u8) -> PageId {
        PageId::from_bytes([n; 16])
    }

    fn doc_row(n: u8, kind: DocKindCode, space_id: Option<SpaceId>, created_at: i64) -> DocRow {
        DocRow {
            doc_id: doc(n),
            kind,
            space_id,
            created_at,
            format_version: 1,
            min_reader_version: 1,
        }
    }

    /// A live root page of `space_id`, updated at `100 + n`.
    fn index_row(n: u8, space_id: SpaceId, title: &str) -> PageIndexRow {
        PageIndexRow {
            page_id: page(n),
            space_id,
            parent_id: None,
            title: title.to_owned(),
            trashed_at: None,
            updated_at: 100 + i64::from(n),
            in_template: false,
        }
    }

    fn trashed(row: PageIndexRow, at: i64) -> PageIndexRow {
        PageIndexRow {
            trashed_at: Some(at),
            ..row
        }
    }

    /// A deterministic blob that cycles through every byte value, NUL included.
    fn blob(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| seed.wrapping_add(u8::try_from(i % 256).unwrap()))
            .collect()
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so
    /// that both `DocQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn DocQueries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
    }

    /// Adds a structure document (of `space(n)`, created at time `n`) that snapshots and
    /// updates can belong to.
    fn add_doc(q: &dyn DocQueries, n: u8) {
        let row = doc_row(n, DocKindCode::Structure, Some(space(n)), i64::from(n));
        q.doc_insert(&row).unwrap();
    }

    fn append(q: &dyn DocQueries, n: u8, bytes: &[u8]) -> i64 {
        q.update_append(doc(n), bytes, UpdateOrigin::Local, 1)
            .unwrap()
    }

    // ---- persisted codes and damaged rows -----------------------------------------

    #[test]
    fn persisted_codes_are_frozen() {
        // These numbers are on disk (`docs.kind`, `doc_updates.origin`): never renumber.
        for (kind, code) in [
            (DocKindCode::Structure, 0),
            (DocKindCode::Page, 1),
            (DocKindCode::Account, 2),
            (DocKindCode::Comments, 3),
        ] {
            assert_eq!(kind.code(), code);
            assert_eq!(i64::from(kind as u8), code, "agrees with the repr");
            assert_eq!(DocKindCode::from_code(code).unwrap(), kind);
        }
        for bad in [-1, 4, 255, i64::MAX] {
            let err = DocKindCode::from_code(bad).unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{bad}: {err}");
        }
        for (origin, code) in [
            (UpdateOrigin::Local, 0),
            (UpdateOrigin::Remote, 1),
            (UpdateOrigin::Core, 2),
        ] {
            assert_eq!(origin.code(), code);
            assert_eq!(i64::from(origin as u8), code, "agrees with the repr");
        }
    }

    #[test]
    fn damaged_document_rows_are_invalid_data_and_never_a_panic() {
        // `zeroblob(16)` is the id `doc(0)`.
        for values in [
            "zeroblob(16), 9, NULL, 1, 1, 1",     // unknown kind
            "zeroblob(16), 0, x'0102', 1, 1, 1",  // space id of 2 bytes
            "zeroblob(16), 0, NULL, 1, 70000, 1", // format version out of range
            "zeroblob(16), 0, NULL, 1, 1, -1",    // reader version out of range
        ] {
            let db = new_db();
            db.connection()
                .execute(&format!("INSERT INTO docs VALUES ({values})"), [])
                .unwrap();
            let err = db.doc_get(doc(0)).unwrap_err();
            assert!(
                matches!(err, StorageError::InvalidData(_)),
                "{values}: {err}"
            );
        }

        // An id of the wrong length cannot be asked for, but a listing meets it.
        let db = new_db();
        db.connection()
            .execute("INSERT INTO docs VALUES (x'01', 0, NULL, 1, 1, 1)", [])
            .unwrap();
        let err = db.docs_by_kind(DocKindCode::Structure).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    #[test]
    fn damaged_index_rows_are_invalid_data_and_never_a_panic() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (zeroblob(16), zeroblob(16), x'03', 't', NULL, 1)",
                [],
            )
            .unwrap();
        let err = db.pages_index_get(page(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");

        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (x'01', zeroblob(16), NULL, 't', NULL, 1)",
                [],
            )
            .unwrap();
        let err = db.pages_index_by_space(space(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    // ---- documents ----------------------------------------------------------------

    #[test]
    fn doc_rows_roundtrip_for_every_kind() {
        on_db_and_tx(|q| {
            let rows = [
                doc_row(1, DocKindCode::Account, None, 10),
                doc_row(2, DocKindCode::Structure, Some(space(1)), i64::MAX),
                DocRow {
                    format_version: u16::MAX,
                    min_reader_version: 7,
                    ..doc_row(3, DocKindCode::Page, Some(space(1)), 0)
                },
            ];
            for row in &rows {
                q.doc_insert(row).unwrap();
            }
            for row in &rows {
                assert_eq!(q.doc_get(row.doc_id).unwrap().as_ref(), Some(row));
            }
            assert_eq!(q.doc_get(doc(99)).unwrap(), None);
        });
    }

    #[test]
    fn inserting_a_duplicate_document_id_is_invalid_data_and_keeps_the_original() {
        on_db_and_tx(|q| {
            let original = doc_row(1, DocKindCode::Structure, Some(space(1)), 10);
            q.doc_insert(&original).unwrap();

            let other = DocRow {
                kind: DocKindCode::Account,
                space_id: None,
                created_at: 99,
                ..original.clone()
            };
            let err = q.doc_insert(&other).unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
            assert!(
                !err.to_string().contains(&doc(1).to_string()),
                "no ids in messages"
            );

            assert_eq!(q.doc_get(doc(1)).unwrap(), Some(original));
            assert!(q.docs_by_kind(DocKindCode::Account).unwrap().is_empty());
        });
    }

    #[test]
    fn docs_by_kind_lists_oldest_first_then_by_id() {
        on_db_and_tx(|q| {
            assert!(q.docs_by_kind(DocKindCode::Structure).unwrap().is_empty());
            // Inserted out of order; documents 1 and 9 share a creation time.
            for (n, created_at) in [(5, 300), (2, 100), (9, 200), (1, 200)] {
                let row = doc_row(n, DocKindCode::Structure, Some(space(n)), created_at);
                q.doc_insert(&row).unwrap();
            }
            q.doc_insert(&doc_row(7, DocKindCode::Account, None, 1))
                .unwrap();
            q.doc_insert(&doc_row(8, DocKindCode::Page, Some(space(1)), 2))
                .unwrap();

            let ids = |kind| -> Vec<DocId> {
                q.docs_by_kind(kind)
                    .unwrap()
                    .iter()
                    .map(|r| r.doc_id)
                    .collect()
            };
            assert_eq!(
                ids(DocKindCode::Structure),
                [doc(2), doc(1), doc(9), doc(5)]
            );
            assert_eq!(ids(DocKindCode::Account), [doc(7)]);
            assert_eq!(ids(DocKindCode::Page), [doc(8)]);
        });
    }

    #[test]
    fn doc_for_space_matches_both_the_space_and_the_kind() {
        on_db_and_tx(|q| {
            let s1 = doc_row(1, DocKindCode::Structure, Some(space(1)), 10);
            let s2 = doc_row(2, DocKindCode::Structure, Some(space(2)), 20);
            let p1 = doc_row(3, DocKindCode::Page, Some(space(1)), 30);
            let account = doc_row(4, DocKindCode::Account, None, 40);
            for row in [&s1, &s2, &p1, &account] {
                q.doc_insert(row).unwrap();
            }
            assert_eq!(
                q.doc_for_space(space(1), DocKindCode::Structure).unwrap(),
                Some(s1)
            );
            assert_eq!(
                q.doc_for_space(space(2), DocKindCode::Structure).unwrap(),
                Some(s2)
            );
            assert_eq!(
                q.doc_for_space(space(1), DocKindCode::Page).unwrap(),
                Some(p1)
            );
            assert_eq!(q.doc_for_space(space(2), DocKindCode::Page).unwrap(), None);
            assert_eq!(
                q.doc_for_space(space(1), DocKindCode::Account).unwrap(),
                None,
                "the account document belongs to no space"
            );
            assert_eq!(
                q.doc_for_space(space(9), DocKindCode::Structure).unwrap(),
                None
            );
        });
    }

    #[test]
    fn doc_for_space_prefers_the_oldest_when_two_documents_claim_a_space() {
        on_db_and_tx(|q| {
            let newer = doc_row(1, DocKindCode::Structure, Some(space(1)), 50);
            let older = doc_row(2, DocKindCode::Structure, Some(space(1)), 5);
            q.doc_insert(&newer).unwrap();
            q.doc_insert(&older).unwrap();
            assert_eq!(
                q.doc_for_space(space(1), DocKindCode::Structure).unwrap(),
                Some(older)
            );
        });
    }

    #[test]
    fn doc_delete_removes_the_snapshot_and_updates_of_that_document_only() {
        on_db_and_tx(|q| {
            add_doc(q, 1);
            add_doc(q, 2);
            for n in [1, 2] {
                q.snapshot_put(doc(n), b"version", b"snapshot", 5).unwrap();
                append(q, n, b"first");
                append(q, n, b"second");
            }

            q.doc_delete(doc(1)).unwrap();
            assert_eq!(q.doc_get(doc(1)).unwrap(), None);
            assert_eq!(q.snapshot_get(doc(1)).unwrap(), None, "snapshot cascades");
            assert!(q.updates_after_snapshot(doc(1)).unwrap().is_empty());
            assert_eq!(q.update_stats(doc(1)).unwrap(), UpdateStats::default());

            assert!(q.doc_get(doc(2)).unwrap().is_some());
            assert!(q.snapshot_get(doc(2)).unwrap().is_some());
            assert_eq!(q.update_stats(doc(2)).unwrap().count, 2);

            // Deleting again, or deleting what never existed, succeeds.
            q.doc_delete(doc(1)).unwrap();
            q.doc_delete(doc(99)).unwrap();

            // The id can be used again and starts clean.
            add_doc(q, 1);
            assert_eq!(q.snapshot_get(doc(1)).unwrap(), None);
            assert_eq!(append(q, 1, b"again"), 1);
        });
    }

    #[test]
    fn doc_set_space_changes_the_space_and_nothing_else() {
        on_db_and_tx(|q| {
            q.doc_insert(&doc_row(1, DocKindCode::Page, Some(space(1)), 7))
                .unwrap();
            q.snapshot_put(doc(1), b"version", b"snapshot", 5).unwrap();
            append(q, 1, b"first");
            q.doc_insert(&doc_row(2, DocKindCode::Page, Some(space(1)), 8))
                .unwrap();

            assert!(q.doc_set_space(doc(1), space(2)).unwrap());
            assert_eq!(
                q.doc_get(doc(1)).unwrap().unwrap(),
                doc_row(1, DocKindCode::Page, Some(space(2)), 7),
                "only the space changed"
            );
            assert!(q.snapshot_get(doc(1)).unwrap().is_some(), "snapshot stays");
            assert_eq!(q.update_stats(doc(1)).unwrap().count, 1, "updates stay");
            assert_eq!(
                q.doc_get(doc(2)).unwrap().unwrap().space_id,
                Some(space(1)),
                "another document is not touched"
            );
            // Setting the same space again finds the row; an unknown document is no error.
            assert!(q.doc_set_space(doc(1), space(2)).unwrap());
            assert!(!q.doc_set_space(doc(9), space(2)).unwrap());
            assert_eq!(q.doc_get(doc(9)).unwrap(), None);
        });
    }

    // ---- snapshots ----------------------------------------------------------------

    #[test]
    fn snapshot_put_inserts_then_replaces_and_get_returns_blob_then_version() {
        on_db_and_tx(|q| {
            add_doc(q, 1);
            add_doc(q, 2);
            assert_eq!(q.snapshot_get(doc(1)).unwrap(), None);

            q.snapshot_put(doc(1), b"version-1", b"blob-1", 10).unwrap();
            assert_eq!(
                q.snapshot_get(doc(1)).unwrap(),
                Some((b"blob-1".to_vec(), b"version-1".to_vec())),
                "blob first, version second"
            );

            q.snapshot_put(doc(1), b"version-2", b"blob-2", 20).unwrap();
            assert_eq!(
                q.snapshot_get(doc(1)).unwrap(),
                Some((b"blob-2".to_vec(), b"version-2".to_vec()))
            );
            assert_eq!(
                q.snapshot_get(doc(2)).unwrap(),
                None,
                "other documents are untouched"
            );
        });
    }

    #[test]
    fn snapshot_replacement_leaves_one_row_with_the_latest_time() {
        let db = new_db();
        add_doc(&db, 1);
        db.snapshot_put(doc(1), b"v1", b"b1", 10).unwrap();
        db.snapshot_put(doc(1), b"v2", b"b2", 20).unwrap();
        let (rows, created_at): (i64, i64) = db
            .connection()
            .query_row(
                "SELECT count(*), max(created_at) FROM doc_snapshots",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((rows, created_at), (1, 20));
    }

    // ---- updates ------------------------------------------------------------------

    #[test]
    fn update_append_numbers_each_document_independently() {
        on_db_and_tx(|q| {
            add_doc(q, 1);
            add_doc(q, 2);
            assert_eq!(append(q, 1, b"a1"), 1);
            assert_eq!(append(q, 2, b"b1"), 1);
            assert_eq!(append(q, 1, b"a2"), 2);
            assert_eq!(append(q, 1, b"a3"), 3);
            assert_eq!(append(q, 2, b"b2"), 2);

            let blobs = |n| q.updates_after_snapshot(doc(n)).unwrap();
            assert_eq!(blobs(1), [b"a1".to_vec(), b"a2".to_vec(), b"a3".to_vec()]);
            assert_eq!(blobs(2), [b"b1".to_vec(), b"b2".to_vec()]);
            assert!(blobs(3).is_empty(), "a document without updates");
        });
    }

    #[test]
    fn update_append_records_origin_and_time() {
        let db = new_db();
        add_doc(&db, 1);
        for (origin, now) in [
            (UpdateOrigin::Local, 10),
            (UpdateOrigin::Remote, 20),
            (UpdateOrigin::Core, 30),
        ] {
            db.update_append(doc(1), b"u", origin, now).unwrap();
        }
        let rows: Vec<(i64, i64, i64)> = db
            .connection()
            .prepare("SELECT local_seq, origin, created_at FROM doc_updates ORDER BY local_seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, [(1, 0, 10), (2, 1, 20), (3, 2, 30)]);
    }

    #[test]
    fn updates_come_back_in_local_seq_order_whatever_the_insertion_order() {
        let db = new_db();
        add_doc(&db, 1);
        db.connection()
            .execute_batch(&format!(
                "INSERT INTO doc_updates VALUES ({id}, 3, x'33', 0, 1);
                 INSERT INTO doc_updates VALUES ({id}, 1, x'11', 0, 1);
                 INSERT INTO doc_updates VALUES ({id}, 2, x'22', 0, 1);",
                id = "x'01010101010101010101010101010101'"
            ))
            .unwrap();
        assert_eq!(
            db.updates_after_snapshot(doc(1)).unwrap(),
            [vec![0x11], vec![0x22], vec![0x33]]
        );
        assert_eq!(
            append(&db, 1, b"next"),
            4,
            "numbering follows the highest row"
        );
    }

    #[test]
    fn local_seq_continues_after_the_highest_remaining_row() {
        on_db_and_tx(|q| {
            add_doc(q, 1);
            for expected in 1..=3 {
                assert_eq!(append(q, 1, b"u"), expected);
            }
            assert_eq!(q.updates_delete_up_to(doc(1), 2).unwrap(), 2);
            assert_eq!(append(q, 1, b"u"), 4, "row 3 is still there");
        });
    }

    #[test]
    fn local_seq_starts_again_at_one_when_every_update_was_deleted() {
        // The numbering is `MAX(local_seq) + 1`, so a document whose log was emptied by
        // compaction starts over. Nothing outside the log refers to a sequence number.
        on_db_and_tx(|q| {
            add_doc(q, 1);
            append(q, 1, b"u");
            append(q, 1, b"u");
            assert_eq!(q.updates_delete_up_to(doc(1), 2).unwrap(), 2);
            assert_eq!(append(q, 1, b"u"), 1);
        });
    }

    #[test]
    fn updates_delete_up_to_removes_exactly_the_covered_rows() {
        on_db_and_tx(|q| {
            add_doc(q, 1);
            add_doc(q, 2);
            // Document 1 gets blobs of 1, 2, 3, 4, 5 bytes; document 2 three of 10.
            for len in 1..=5 {
                append(q, 1, &blob(len, 0));
            }
            for _ in 0..3 {
                append(q, 2, &blob(10, 0));
            }

            assert_eq!(q.updates_delete_up_to(doc(1), 3).unwrap(), 3);
            assert_eq!(
                q.updates_after_snapshot(doc(1)).unwrap(),
                [blob(4, 0), blob(5, 0)],
                "rows 4 and 5 remain, in order"
            );
            assert_eq!(
                q.update_stats(doc(1)).unwrap(),
                UpdateStats {
                    count: 2,
                    bytes: 9,
                    last_seq: Some(5)
                }
            );
            assert_eq!(
                q.update_stats(doc(2)).unwrap().count,
                3,
                "other documents are untouched"
            );

            // Nothing left to delete at or below 3; zero, negative and unknown ids delete nothing.
            for (id, up_to) in [(1, 3), (1, 0), (1, -4), (99, 10)] {
                assert_eq!(
                    q.updates_delete_up_to(doc(id), up_to).unwrap(),
                    0,
                    "{id} {up_to}"
                );
            }

            // The bound is inclusive: row 4 goes with `up_to = 4`, row 5 stays.
            assert_eq!(q.updates_delete_up_to(doc(1), 4).unwrap(), 1);
            assert_eq!(q.update_stats(doc(1)).unwrap().last_seq, Some(5));

            assert_eq!(q.updates_delete_up_to(doc(1), i64::MAX).unwrap(), 1);
            assert_eq!(q.update_stats(doc(1)).unwrap(), UpdateStats::default());
            assert_eq!(q.update_stats(doc(2)).unwrap().count, 3);
        });
    }

    #[test]
    fn update_stats_counts_rows_bytes_and_the_last_sequence() {
        on_db_and_tx(|q| {
            add_doc(q, 1);
            let empty = UpdateStats {
                count: 0,
                bytes: 0,
                last_seq: None,
            };
            assert_eq!(q.update_stats(doc(1)).unwrap(), empty);
            assert_eq!(q.update_stats(doc(99)).unwrap(), empty, "unknown document");

            // Bytes are blob bytes: NULs and high bytes count, and an empty blob is a row.
            append(q, 1, &[0, 1, 2, 0, 255]);
            append(q, 1, &[]);
            append(q, 1, &[7; 1000]);
            assert_eq!(
                q.update_stats(doc(1)).unwrap(),
                UpdateStats {
                    count: 3,
                    bytes: 1005,
                    last_seq: Some(3)
                }
            );
        });
    }

    #[test]
    fn blobs_are_opaque_bytes() {
        on_db_and_tx(|q| {
            add_doc(q, 1);
            let every_byte = blob(256, 0);
            let large = blob(300_000, 17);
            let envelope_like = b"KBD1\x01\x01\x01\x00\x00\xff payload".to_vec();

            q.snapshot_put(doc(1), &envelope_like, &large, 1).unwrap();
            assert_eq!(
                q.snapshot_get(doc(1)).unwrap(),
                Some((large.clone(), envelope_like.clone()))
            );
            for bytes in [&every_byte, &large, &envelope_like, &Vec::new()] {
                append(q, 1, bytes);
            }
            assert_eq!(
                q.updates_after_snapshot(doc(1)).unwrap(),
                [every_byte, large, envelope_like, Vec::new()]
            );
        });
    }

    // ---- pages_index --------------------------------------------------------------

    #[test]
    fn pages_index_rows_roundtrip_and_upsert_replaces_in_place() {
        on_db_and_tx(|q| {
            let root = index_row(1, space(1), "Root");
            let child = PageIndexRow {
                parent_id: Some(page(1)),
                ..trashed(
                    index_row(
                        2,
                        space(1),
                        "Child \"\u{e9}\" \u{1F600} \u{65e5}\u{672c}\u{8a9e}",
                    ),
                    500,
                )
            };
            q.pages_index_upsert(&[root.clone(), child.clone()])
                .unwrap();
            assert_eq!(q.pages_index_get(page(1)).unwrap(), Some(root.clone()));
            assert_eq!(q.pages_index_get(page(2)).unwrap(), Some(child.clone()));
            assert_eq!(q.pages_index_get(page(3)).unwrap(), None);

            // Every column is replaced for an existing id; a new id is added.
            let changed = PageIndexRow {
                space_id: space(2),
                parent_id: Some(page(9)),
                title: "Renamed".to_owned(),
                trashed_at: None,
                updated_at: 777,
                in_template: false,
                ..root
            };
            let fresh = index_row(3, space(1), "New");
            q.pages_index_upsert(&[changed.clone(), fresh.clone()])
                .unwrap();
            assert_eq!(q.pages_index_get(page(1)).unwrap(), Some(changed.clone()));
            assert_eq!(q.pages_index_get(page(2)).unwrap(), Some(child.clone()));
            assert_eq!(q.pages_index_get(page(3)).unwrap(), Some(fresh.clone()));
            assert_eq!(q.pages_index_by_space(space(1)).unwrap(), [child, fresh]);
            assert_eq!(q.pages_index_by_space(space(2)).unwrap(), [changed]);

            q.pages_index_upsert(&[]).unwrap();
        });
    }

    #[test]
    fn pages_index_upsert_of_one_page_twice_keeps_the_last() {
        on_db_and_tx(|q| {
            let first = index_row(1, space(1), "First");
            let last = PageIndexRow {
                title: "Last".to_owned(),
                ..first.clone()
            };
            q.pages_index_upsert(&[first, last.clone()]).unwrap();
            assert_eq!(q.pages_index_by_space(space(1)).unwrap(), [last]);
        });
    }

    #[test]
    fn pages_index_by_space_lists_every_row_of_that_space_ordered_by_page_id() {
        on_db_and_tx(|q| {
            assert!(q.pages_index_by_space(space(1)).unwrap().is_empty());
            let rows = [
                index_row(5, space(1), "e"),
                trashed(index_row(2, space(1), "b"), 9),
                index_row(9, space(2), "elsewhere"),
                index_row(7, space(1), "g"),
                index_row(1, space(1), "a"),
            ];
            q.pages_index_upsert(&rows).unwrap();

            let titles = |space_id| -> Vec<String> {
                q.pages_index_by_space(space_id)
                    .unwrap()
                    .into_iter()
                    .map(|r| r.title)
                    .collect()
            };
            assert_eq!(
                titles(space(1)),
                ["a", "b", "e", "g"],
                "trashed rows included"
            );
            assert_eq!(titles(space(2)), ["elsewhere"]);
            assert!(titles(space(3)).is_empty());
        });
    }

    #[test]
    fn pages_index_live_count_excludes_trashed_rows() {
        on_db_and_tx(|q| {
            assert_eq!(q.pages_index_live_count(space(1)).unwrap(), 0);
            q.pages_index_upsert(&[
                index_row(1, space(1), "live"),
                index_row(2, space(1), "live too"),
                trashed(index_row(3, space(1), "trashed"), 5),
                // A trash time of 0 is still a trash time, not "not trashed".
                trashed(index_row(4, space(1), "trashed at zero"), 0),
                index_row(5, space(2), "other space"),
            ])
            .unwrap();
            assert_eq!(q.pages_index_live_count(space(1)).unwrap(), 2);
            assert_eq!(q.pages_index_live_count(space(2)).unwrap(), 1);
            assert_eq!(q.pages_index_live_count(space(3)).unwrap(), 0);

            // Restoring and trashing move the count.
            q.pages_index_upsert(&[index_row(3, space(1), "restored")])
                .unwrap();
            assert_eq!(q.pages_index_live_count(space(1)).unwrap(), 3);
            q.pages_index_upsert(&[trashed(index_row(1, space(1), "live"), 6)])
                .unwrap();
            assert_eq!(q.pages_index_live_count(space(1)).unwrap(), 2);
        });
    }

    #[test]
    fn pages_index_delete_removes_the_given_rows_and_ignores_unknown_ids() {
        on_db_and_tx(|q| {
            q.pages_index_upsert(&[
                index_row(1, space(1), "a"),
                index_row(2, space(1), "b"),
                index_row(3, space(1), "c"),
            ])
            .unwrap();

            q.pages_index_delete(&[]).unwrap();
            assert_eq!(q.pages_index_by_space(space(1)).unwrap().len(), 3);

            // An unknown id and a repeated id are no problem.
            q.pages_index_delete(&[page(1), page(99), page(1)]).unwrap();
            assert_eq!(q.pages_index_get(page(1)).unwrap(), None);
            assert!(q.pages_index_get(page(2)).unwrap().is_some());
            assert!(q.pages_index_get(page(3)).unwrap().is_some());
            assert_eq!(q.pages_index_live_count(space(1)).unwrap(), 2);

            q.pages_index_delete(&[page(99)]).unwrap();
            assert_eq!(q.pages_index_by_space(space(1)).unwrap().len(), 2);
        });
    }

    #[test]
    fn pages_index_clear_space_removes_only_that_space() {
        on_db_and_tx(|q| {
            q.pages_index_upsert(&[
                index_row(1, space(1), "a"),
                trashed(index_row(2, space(1), "b"), 3),
                index_row(3, space(2), "c"),
            ])
            .unwrap();

            q.pages_index_clear_space(space(1)).unwrap();
            assert!(
                q.pages_index_by_space(space(1)).unwrap().is_empty(),
                "trashed rows too"
            );
            assert_eq!(q.pages_index_live_count(space(1)).unwrap(), 0);
            assert_eq!(q.pages_index_by_space(space(2)).unwrap().len(), 1);

            q.pages_index_clear_space(space(1)).unwrap();
            q.pages_index_clear_space(space(9)).unwrap();
            assert_eq!(q.pages_index_by_space(space(2)).unwrap().len(), 1);
        });
    }

    // ---- transactions and atomicity -----------------------------------------------

    #[test]
    fn transaction_commits_when_the_closure_returns_ok() {
        let mut db = new_db();
        let seq = db
            .transaction(|tx| {
                tx.doc_insert(&doc_row(1, DocKindCode::Structure, Some(space(1)), 1))?;
                tx.pages_index_upsert(&[index_row(1, space(1), "A")])?;
                tx.update_append(doc(1), b"u", UpdateOrigin::Local, 5)
            })
            .unwrap();
        assert_eq!(seq, 1, "the closure's value is passed through");

        assert!(db.doc_get(doc(1)).unwrap().is_some());
        assert_eq!(db.updates_after_snapshot(doc(1)).unwrap(), [b"u".to_vec()]);
        assert_eq!(db.pages_index_live_count(space(1)).unwrap(), 1);
        assert!(db.connection().is_autocommit(), "the transaction is over");
    }

    #[test]
    fn transaction_rolls_back_every_statement_when_the_closure_returns_err() {
        let mut db = new_db();
        add_doc(&db, 1);
        let err = db
            .transaction(|tx| {
                tx.doc_insert(&doc_row(2, DocKindCode::Structure, Some(space(2)), 2))?;
                tx.update_append(doc(1), b"u", UpdateOrigin::Local, 3)?;
                tx.snapshot_put(doc(1), b"v", b"s", 4)?;
                tx.pages_index_upsert(&[index_row(1, space(1), "A")])?;
                Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m == "boom"),
            "{err}"
        );

        assert!(
            db.doc_get(doc(1)).unwrap().is_some(),
            "earlier state survives"
        );
        assert_eq!(db.doc_get(doc(2)).unwrap(), None);
        assert!(db.updates_after_snapshot(doc(1)).unwrap().is_empty());
        assert_eq!(db.snapshot_get(doc(1)).unwrap(), None);
        assert_eq!(db.pages_index_get(page(1)).unwrap(), None);
        assert!(db.connection().is_autocommit(), "the transaction is over");

        // The connection is fine afterwards and the rolled back append left no gap.
        let seq = db
            .transaction(|tx| tx.update_append(doc(1), b"u", UpdateOrigin::Local, 3))
            .unwrap();
        assert_eq!(seq, 1);
    }

    #[test]
    fn a_failing_statement_rolls_back_the_earlier_ones_of_its_transaction() {
        let mut db = new_db();
        add_doc(&db, 1);
        let err = db
            .transaction(|tx| {
                add_doc(tx, 2);
                tx.update_append(doc(2), b"u", UpdateOrigin::Local, 1)?;
                // Fails: this id exists already.
                tx.doc_insert(&doc_row(1, DocKindCode::Account, None, 9))?;
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        assert_eq!(db.doc_get(doc(2)).unwrap(), None);
        assert!(db.updates_after_snapshot(doc(2)).unwrap().is_empty());
        assert!(db.updates_after_snapshot(doc(1)).unwrap().is_empty());
    }

    #[test]
    fn transaction_rolls_back_when_the_closure_panics() {
        let mut db = new_db();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = db.transaction::<()>(|tx| {
                add_doc(tx, 1);
                panic!("the closure panics after writing");
            });
        }));
        assert!(outcome.is_err());
        assert_eq!(db.doc_get(doc(1)).unwrap(), None, "rolled back by drop");
        assert!(db.connection().is_autocommit());
        db.transaction(|tx| tx.doc_insert(&doc_row(1, DocKindCode::Account, None, 1)))
            .unwrap();
    }

    #[test]
    fn reads_inside_a_transaction_see_its_own_writes() {
        // How the core uses it: append an update and read the stats in one transaction.
        let mut db = new_db();
        add_doc(&db, 1);
        let stats = db
            .transaction(|tx| {
                tx.update_append(doc(1), b"abc", UpdateOrigin::Local, 1)?;
                tx.update_append(doc(1), b"de", UpdateOrigin::Remote, 2)?;
                tx.update_stats(doc(1))
            })
            .unwrap();
        assert_eq!(
            stats,
            UpdateStats {
                count: 2,
                bytes: 5,
                last_seq: Some(2)
            }
        );
        assert_eq!(db.update_stats(doc(1)).unwrap(), stats);
    }

    #[test]
    fn snapshots_and_updates_need_an_existing_document() {
        on_db_and_tx(|q| {
            let err = q.snapshot_put(doc(1), b"v", b"s", 1).unwrap_err();
            assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
            let err = q
                .update_append(doc(1), b"u", UpdateOrigin::Local, 1)
                .unwrap_err();
            assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
            assert_eq!(q.snapshot_get(doc(1)).unwrap(), None);
            assert!(q.updates_after_snapshot(doc(1)).unwrap().is_empty());

            // The failures did not spoil the connection or the surrounding transaction.
            add_doc(q, 1);
            assert_eq!(append(q, 1, b"u"), 1);
        });
    }

    #[test]
    fn batch_methods_on_the_bare_db_are_all_or_nothing() {
        let db = new_db();
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom_insert BEFORE INSERT ON pages_index
                 WHEN NEW.title = 'boom' BEGIN SELECT RAISE(ABORT, 'boom'); END;
                 CREATE TRIGGER no_boom_delete BEFORE DELETE ON pages_index
                 WHEN OLD.title = 'boom' BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();

        let rows = [
            index_row(1, space(1), "fine"),
            index_row(2, space(1), "boom"),
            index_row(3, space(1), "never reached"),
        ];
        let err = db.pages_index_upsert(&rows).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert!(
            db.pages_index_by_space(space(1)).unwrap().is_empty(),
            "row 1 went too"
        );
        assert!(db.connection().is_autocommit());

        db.connection()
            .execute_batch("DROP TRIGGER no_boom_insert;")
            .unwrap();
        db.pages_index_upsert(&rows).unwrap();
        let err = db
            .pages_index_delete(&[page(1), page(2), page(3)])
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            db.pages_index_by_space(space(1)).unwrap().len(),
            3,
            "row 1 came back"
        );
        assert!(db.connection().is_autocommit());
    }
}
