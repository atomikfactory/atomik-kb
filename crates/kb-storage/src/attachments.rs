//! The attachment index and the attachment references (schema v5,
//! `docs/development/milestone-05.md`, "Storage"; ADR-0020). Derived data: the manifests
//! live in the structure document of the space and the references come from the page
//! documents, so the core can rebuild both tables. The chunks themselves are not here:
//! they live in the [`crate::BlobStore`].
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`] through
//! [`AttachmentQueries`], both running the same private functions over a connection (a
//! transaction derefs to one), so a method behaves identically in either place. The
//! methods that take more than one statement run as one atomic unit on a bare [`Db`].
//!
//! # The two tables
//!
//! - `attachments_index` has one row per attachment: a mirror of its manifest (name, MIME
//!   type, size, chunk count, creation time) for listing, plus `unreferenced_since`, which
//!   the garbage collector reads. It is NULL while a block references the attachment and
//!   the time the last reference went otherwise; this crate only stores it, the core decides
//!   when it changes.
//! - `attachment_refs` has one row per `(attachment, page, block)`. A block that shows the
//!   same attachment twice has one row. There is deliberately no foreign key to the index: a
//!   page can reference an attachment that was purged (the block then renders "deleted"), and
//!   its reference stays until the page is edited or purged.
//! - A page also references the attachments of its `files` property values: the
//!   `page_properties` rows with an `attachment_id` (schema v11, `page_properties` module). They
//!   are rewritten with the values of the page and removed with the page or the property, so
//!   they are not copied into `attachment_refs`; the reference count and the attachments a page
//!   references (below) read both tables, one reference per `(page, property)` for a property
//!   value.
//! - A page whose icon or cover is an uploaded image references that attachment too: the
//!   `icon_refs` rows (schema v14 and v16, `icon_refs` module). Same lifecycle as the property rows
//!   (rewritten with the structure document, removed with the page, not copied into
//!   `attachment_refs`), one reference per page and attachment (the same image as icon and cover
//!   is one).
//!
//! Ids are bound as 16-byte blobs. Rows are read as raw column values first and validated
//! afterwards, so a damaged row is a typed `InvalidData` error and never a panic or a
//! silently wrong value. Names and MIME types never appear in an error.

use kb_types::{AttachmentId, BlockId, PageId, SpaceId};
use rusqlite::{Connection, Row, params};

use crate::docs::{atomically, id_from};
use crate::{Db, StorageError, Tx};

/// An `attachments_index` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRow {
    /// Attachment.
    pub attachment_id: AttachmentId,
    /// Space that owns it.
    pub space_id: SpaceId,
    /// File name as the user knew it (at most 255 bytes; the core validates).
    pub name: String,
    /// MIME type (at most 127 bytes of printable ASCII; the core validates).
    pub mime: String,
    /// Plaintext size in bytes. SQLite stores 63 bits, so a value above `i64::MAX` is
    /// refused (`InvalidData`); attachments are limited to 2 GiB anyway.
    pub size: u64,
    /// Number of chunks in the manifest (at least 1: an empty file is one chunk of padding).
    pub chunk_count: u32,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// When the attachment lost its last reference, Unix ms; `None` while a block shows it.
    pub unreferenced_since: Option<i64>,
}

/// An `attachment_refs` row: block `block_id` of page `page_id` shows the attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AttachmentRef {
    /// The attachment shown.
    pub attachment_id: AttachmentId,
    /// The page whose document holds the block.
    pub page_id: PageId,
    /// The `image` or `file` block.
    pub block_id: BlockId,
}

/// Queries of the attachment index and references, available on a connection or inside a
/// transaction. On a [`Db`] every method is atomic by itself.
pub trait AttachmentQueries {
    /// Inserts the row, or replaces every column of the row with the same attachment id
    /// (an upsert; `unreferenced_since` is part of the row and is replaced with it). The
    /// references of the attachment are not touched. A `size` above `i64::MAX` is
    /// `InvalidData` and nothing is written.
    fn attachment_put(&self, row: &AttachmentRow) -> Result<(), StorageError>;
    /// One row, `None` for an unknown attachment.
    fn attachment_get(&self, id: AttachmentId) -> Result<Option<AttachmentRow>, StorageError>;
    /// The attachments of a space, newest first: by `created_at` descending, then by
    /// attachment id ascending (so equal times come out in a stable order).
    fn attachments_by_space(&self, space_id: SpaceId) -> Result<Vec<AttachmentRow>, StorageError>;
    /// Every row of every space, oldest first: by `created_at`, then by attachment id (the
    /// order of the manifests in the structure document, for the open-time check and the
    /// backup).
    fn attachments_all(&self) -> Result<Vec<AttachmentRow>, StorageError>;
    /// Removes the row and every reference to the attachment, whatever the page. Succeeds
    /// when the attachment is unknown. The two removals are one atomic unit.
    fn attachment_delete(&self, id: AttachmentId) -> Result<(), StorageError>;
    /// Makes the references of a page exactly `refs`, pairs of (block, attachment): the
    /// rows of the page that are not in the list go, the missing ones come, other pages
    /// are not touched. A pair given twice is one row. The attachments need not have an
    /// index row (see the module docs). One atomic unit.
    fn attachment_refs_replace(
        &self,
        page_id: PageId,
        refs: &[(BlockId, AttachmentId)],
    ) -> Result<(), StorageError>;
    /// Removes every reference held by the given pages (purge). Unknown pages and an empty
    /// list succeed. One atomic unit.
    fn attachment_refs_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError>;
    /// Number of references to an attachment, trashed pages included (0 for an unknown
    /// attachment): the `(page, block)` pairs that show it, the `(page, property)` values that
    /// attach it and the pages that have it as their icon.
    fn attachment_reference_count(&self, id: AttachmentId) -> Result<u32, StorageError>;
    /// Sets or clears `unreferenced_since` of one attachment and nothing else. An unknown
    /// attachment is not an error and changes nothing (a block can reference an attachment
    /// that was purged, and the core's bookkeeping does not tell them apart).
    fn attachment_set_unreferenced_since(
        &self,
        id: AttachmentId,
        since: Option<i64>,
    ) -> Result<(), StorageError>;
    /// Points an attachment at another space (it moved with its pages, ADR-0038) and nothing
    /// else: name, size, chunk count, `unreferenced_since` and the references stay. Answers
    /// whether a row was found; an unknown attachment is not an error.
    fn attachment_set_space(
        &self,
        id: AttachmentId,
        space_id: SpaceId,
    ) -> Result<bool, StorageError>;
    /// The rows whose `unreferenced_since` is set and **strictly** before `cutoff`, the
    /// longest unreferenced first (then by attachment id): what the garbage collector may
    /// purge. A row that is referenced (`None`) is never in the list.
    fn attachments_unreferenced_before(
        &self,
        cutoff: i64,
    ) -> Result<Vec<AttachmentRow>, StorageError>;
    /// The attachments a page references (blocks, `files` property values and its icon), each
    /// once, ordered by attachment id.
    fn attachment_ids_referenced_by(
        &self,
        page_id: PageId,
    ) -> Result<Vec<AttachmentId>, StorageError>;
    /// The pages that reference an attachment (blocks, `files` property values and icons), each
    /// once, ordered by page id: what moving pages to another space asks to know whether an
    /// attachment can follow them (ADR-0038).
    fn attachment_referencing_pages(&self, id: AttachmentId) -> Result<Vec<PageId>, StorageError>;
}

impl AttachmentQueries for Db {
    fn attachment_put(&self, row: &AttachmentRow) -> Result<(), StorageError> {
        attachment_put_on(&self.conn, row)
    }
    fn attachment_get(&self, id: AttachmentId) -> Result<Option<AttachmentRow>, StorageError> {
        attachment_get_on(&self.conn, id)
    }
    fn attachments_by_space(&self, space_id: SpaceId) -> Result<Vec<AttachmentRow>, StorageError> {
        attachments_by_space_on(&self.conn, space_id)
    }
    fn attachments_all(&self) -> Result<Vec<AttachmentRow>, StorageError> {
        attachments_all_on(&self.conn)
    }
    fn attachment_delete(&self, id: AttachmentId) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| attachment_delete_on(conn, id))
    }
    fn attachment_refs_replace(
        &self,
        page_id: PageId,
        refs: &[(BlockId, AttachmentId)],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            attachment_refs_replace_on(conn, page_id, refs)
        })
    }
    fn attachment_refs_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            attachment_refs_delete_pages_on(conn, pages)
        })
    }
    fn attachment_reference_count(&self, id: AttachmentId) -> Result<u32, StorageError> {
        attachment_reference_count_on(&self.conn, id)
    }
    fn attachment_set_unreferenced_since(
        &self,
        id: AttachmentId,
        since: Option<i64>,
    ) -> Result<(), StorageError> {
        attachment_set_unreferenced_since_on(&self.conn, id, since)
    }
    fn attachment_set_space(
        &self,
        id: AttachmentId,
        space_id: SpaceId,
    ) -> Result<bool, StorageError> {
        attachment_set_space_on(&self.conn, id, space_id)
    }
    fn attachments_unreferenced_before(
        &self,
        cutoff: i64,
    ) -> Result<Vec<AttachmentRow>, StorageError> {
        attachments_unreferenced_before_on(&self.conn, cutoff)
    }
    fn attachment_ids_referenced_by(
        &self,
        page_id: PageId,
    ) -> Result<Vec<AttachmentId>, StorageError> {
        attachment_ids_referenced_by_on(&self.conn, page_id)
    }
    fn attachment_referencing_pages(&self, id: AttachmentId) -> Result<Vec<PageId>, StorageError> {
        attachment_referencing_pages_on(&self.conn, id)
    }
}

impl AttachmentQueries for Tx<'_> {
    fn attachment_put(&self, row: &AttachmentRow) -> Result<(), StorageError> {
        attachment_put_on(&self.tx, row)
    }
    fn attachment_get(&self, id: AttachmentId) -> Result<Option<AttachmentRow>, StorageError> {
        attachment_get_on(&self.tx, id)
    }
    fn attachments_by_space(&self, space_id: SpaceId) -> Result<Vec<AttachmentRow>, StorageError> {
        attachments_by_space_on(&self.tx, space_id)
    }
    fn attachments_all(&self) -> Result<Vec<AttachmentRow>, StorageError> {
        attachments_all_on(&self.tx)
    }
    fn attachment_delete(&self, id: AttachmentId) -> Result<(), StorageError> {
        attachment_delete_on(&self.tx, id)
    }
    fn attachment_refs_replace(
        &self,
        page_id: PageId,
        refs: &[(BlockId, AttachmentId)],
    ) -> Result<(), StorageError> {
        attachment_refs_replace_on(&self.tx, page_id, refs)
    }
    fn attachment_refs_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        attachment_refs_delete_pages_on(&self.tx, pages)
    }
    fn attachment_reference_count(&self, id: AttachmentId) -> Result<u32, StorageError> {
        attachment_reference_count_on(&self.tx, id)
    }
    fn attachment_set_unreferenced_since(
        &self,
        id: AttachmentId,
        since: Option<i64>,
    ) -> Result<(), StorageError> {
        attachment_set_unreferenced_since_on(&self.tx, id, since)
    }
    fn attachment_set_space(
        &self,
        id: AttachmentId,
        space_id: SpaceId,
    ) -> Result<bool, StorageError> {
        attachment_set_space_on(&self.tx, id, space_id)
    }
    fn attachments_unreferenced_before(
        &self,
        cutoff: i64,
    ) -> Result<Vec<AttachmentRow>, StorageError> {
        attachments_unreferenced_before_on(&self.tx, cutoff)
    }
    fn attachment_ids_referenced_by(
        &self,
        page_id: PageId,
    ) -> Result<Vec<AttachmentId>, StorageError> {
        attachment_ids_referenced_by_on(&self.tx, page_id)
    }
    fn attachment_referencing_pages(&self, id: AttachmentId) -> Result<Vec<PageId>, StorageError> {
        attachment_referencing_pages_on(&self.tx, id)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// `SELECT` of every column of `attachments_index` in the order [`AttachmentColumns::read`]
/// expects, followed by the clauses given.
macro_rules! select_attachments {
    ($($clauses:literal),*) => {
        concat!(
            "SELECT attachment_id, space_id, name, mime, size, chunk_count, created_at, \
             unreferenced_since FROM attachments_index ",
            $($clauses),*
        )
    };
}

/// Inserts a row or replaces every column of the one with the same id.
const PUT_SQL: &str = "INSERT INTO attachments_index \
     (attachment_id, space_id, name, mime, size, chunk_count, created_at, unreferenced_since) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
     ON CONFLICT (attachment_id) DO UPDATE SET \
       space_id = excluded.space_id, name = excluded.name, mime = excluded.mime, \
       size = excluded.size, chunk_count = excluded.chunk_count, \
       created_at = excluded.created_at, unreferenced_since = excluded.unreferenced_since";

/// One row by primary key.
const GET_SQL: &str = select_attachments!("WHERE attachment_id = ?1");

/// The rows of a space, newest first: `attachments_index_space` finds them in that order,
/// the id only orders equal times.
const BY_SPACE_SQL: &str =
    select_attachments!("WHERE space_id = ?1 ORDER BY created_at DESC, attachment_id");

/// Every row, oldest first.
const ALL_SQL: &str = select_attachments!("ORDER BY created_at, attachment_id");

/// The rows that have been unreferenced for longer than the cutoff, longest first. The
/// table is the size of the user's attachment list, so it is scanned.
const UNREFERENCED_BEFORE_SQL: &str = select_attachments!(
    "WHERE unreferenced_since IS NOT NULL AND unreferenced_since < ?1 \
     ORDER BY unreferenced_since, attachment_id"
);

const DELETE_ROW_SQL: &str = "DELETE FROM attachments_index WHERE attachment_id = ?1";

/// Every reference to an attachment (the primary key starts with the attachment id).
const DELETE_REFS_OF_ATTACHMENT_SQL: &str = "DELETE FROM attachment_refs WHERE attachment_id = ?1";

/// The references of a page (`attachment_refs_page`).
const DELETE_REFS_OF_PAGE_SQL: &str = "DELETE FROM attachment_refs WHERE page_id = ?1";

/// A pair given twice is one row.
const INSERT_REF_SQL: &str = "INSERT INTO attachment_refs (attachment_id, page_id, block_id) \
     VALUES (?1, ?2, ?3) ON CONFLICT (attachment_id, page_id, block_id) DO NOTHING";

/// The blocks that show the attachment, the property values of pages that attach it and the pages
/// that have it as their icon or cover (each from an index: the primary key of `attachment_refs`,
/// `page_properties_by_attachment`, `icon_refs_by_attachment`).
const REF_COUNT_SQL: &str = "SELECT \
       (SELECT count(*) FROM attachment_refs WHERE attachment_id = ?1) \
     + (SELECT count(*) FROM page_properties WHERE attachment_id = ?1) \
     + (SELECT count(*) FROM icon_refs WHERE attachment_id = ?1)";

const SET_UNREFERENCED_SINCE_SQL: &str =
    "UPDATE attachments_index SET unreferenced_since = ?2 WHERE attachment_id = ?1";

/// The attachments of a page, once each: its blocks (`attachment_refs_page`), its property values
/// (`page_properties_page`) and its icon and cover (the primary key of `icon_refs`). `UNION` removes the
/// repeats.
const IDS_OF_PAGE_SQL: &str = "SELECT attachment_id FROM attachment_refs WHERE page_id = ?1 \
     UNION SELECT attachment_id FROM page_properties \
            WHERE page_id = ?1 AND attachment_id IS NOT NULL \
     UNION SELECT attachment_id FROM icon_refs WHERE page_id = ?1 \
     ORDER BY attachment_id";

/// The pages that reference an attachment, once each: through the primary key of
/// `attachment_refs`, `page_properties_by_attachment` and `icon_refs_by_attachment`.
const PAGES_OF_ATTACHMENT_SQL: &str = "SELECT page_id FROM attachment_refs WHERE attachment_id = ?1 \
     UNION SELECT page_id FROM page_properties WHERE attachment_id = ?1 \
     UNION SELECT page_id FROM icon_refs WHERE attachment_id = ?1 \
     ORDER BY page_id";

/// Raw column values of an `attachments_index` row.
struct AttachmentColumns {
    attachment_id: Vec<u8>,
    space_id: Vec<u8>,
    name: String,
    mime: String,
    size: i64,
    chunk_count: i64,
    created_at: i64,
    unreferenced_since: Option<i64>,
}

impl AttachmentColumns {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            attachment_id: row.get(0)?,
            space_id: row.get(1)?,
            name: row.get(2)?,
            mime: row.get(3)?,
            size: row.get(4)?,
            chunk_count: row.get(5)?,
            created_at: row.get(6)?,
            unreferenced_since: row.get(7)?,
        })
    }

    fn decode(self) -> Result<AttachmentRow, StorageError> {
        Ok(AttachmentRow {
            attachment_id: AttachmentId::from_bytes(id_from(&self.attachment_id, "attachment id")?),
            space_id: SpaceId::from_bytes(id_from(&self.space_id, "attachment space id")?),
            name: self.name,
            mime: self.mime,
            size: u64::try_from(self.size).map_err(|_| {
                StorageError::InvalidData("attachment size is out of range".to_owned())
            })?,
            chunk_count: u32::try_from(self.chunk_count).map_err(|_| {
                StorageError::InvalidData("attachment chunk count is out of range".to_owned())
            })?,
            created_at: self.created_at,
            unreferenced_since: self.unreferenced_since,
        })
    }
}

fn rows_from(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<AttachmentRow>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params, AttachmentColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(AttachmentColumns::decode).collect()
}

fn attachment_put_on(conn: &Connection, row: &AttachmentRow) -> Result<(), StorageError> {
    let size = i64::try_from(row.size)
        .map_err(|_| StorageError::InvalidData("attachment size is out of range".to_owned()))?;
    conn.execute(
        PUT_SQL,
        params![
            row.attachment_id.to_bytes(),
            row.space_id.to_bytes(),
            row.name,
            row.mime,
            size,
            row.chunk_count,
            row.created_at,
            row.unreferenced_since,
        ],
    )?;
    Ok(())
}

fn attachment_get_on(
    conn: &Connection,
    id: AttachmentId,
) -> Result<Option<AttachmentRow>, StorageError> {
    Ok(rows_from(conn, GET_SQL, [id.to_bytes()])?.pop())
}

fn attachments_by_space_on(
    conn: &Connection,
    space_id: SpaceId,
) -> Result<Vec<AttachmentRow>, StorageError> {
    rows_from(conn, BY_SPACE_SQL, [space_id.to_bytes()])
}

fn attachments_all_on(conn: &Connection) -> Result<Vec<AttachmentRow>, StorageError> {
    rows_from(conn, ALL_SQL, [])
}

fn attachment_set_space_on(
    conn: &Connection,
    id: AttachmentId,
    space_id: SpaceId,
) -> Result<bool, StorageError> {
    let changed = conn.execute(
        "UPDATE attachments_index SET space_id = ?2 WHERE attachment_id = ?1",
        params![id.to_bytes(), space_id.to_bytes()],
    )?;
    Ok(changed > 0)
}

fn attachment_delete_on(conn: &Connection, id: AttachmentId) -> Result<(), StorageError> {
    conn.execute(DELETE_REFS_OF_ATTACHMENT_SQL, [id.to_bytes()])?;
    conn.execute(DELETE_ROW_SQL, [id.to_bytes()])?;
    Ok(())
}

fn attachment_refs_replace_on(
    conn: &Connection,
    page_id: PageId,
    refs: &[(BlockId, AttachmentId)],
) -> Result<(), StorageError> {
    conn.execute(DELETE_REFS_OF_PAGE_SQL, [page_id.to_bytes()])?;
    if refs.is_empty() {
        return Ok(());
    }
    let mut insert = conn.prepare(INSERT_REF_SQL)?;
    for (block_id, attachment_id) in refs {
        insert.execute(params![
            attachment_id.to_bytes(),
            page_id.to_bytes(),
            block_id.to_bytes()
        ])?;
    }
    Ok(())
}

fn attachment_refs_delete_pages_on(
    conn: &Connection,
    pages: &[PageId],
) -> Result<(), StorageError> {
    if pages.is_empty() {
        return Ok(());
    }
    let mut delete = conn.prepare(DELETE_REFS_OF_PAGE_SQL)?;
    for page_id in pages {
        delete.execute([page_id.to_bytes()])?;
    }
    Ok(())
}

fn attachment_reference_count_on(conn: &Connection, id: AttachmentId) -> Result<u32, StorageError> {
    let count: i64 = conn.query_row(REF_COUNT_SQL, [id.to_bytes()], |row| row.get(0))?;
    u32::try_from(count)
        .map_err(|_| StorageError::InvalidData("attachment reference count is out of range".into()))
}

fn attachment_set_unreferenced_since_on(
    conn: &Connection,
    id: AttachmentId,
    since: Option<i64>,
) -> Result<(), StorageError> {
    conn.execute(SET_UNREFERENCED_SINCE_SQL, params![id.to_bytes(), since])?;
    Ok(())
}

fn attachments_unreferenced_before_on(
    conn: &Connection,
    cutoff: i64,
) -> Result<Vec<AttachmentRow>, StorageError> {
    rows_from(conn, UNREFERENCED_BEFORE_SQL, [cutoff])
}

fn attachment_referencing_pages_on(
    conn: &Connection,
    id: AttachmentId,
) -> Result<Vec<PageId>, StorageError> {
    let mut stmt = conn.prepare(PAGES_OF_ATTACHMENT_SQL)?;
    let pages = stmt
        .query_map([id.to_bytes()], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    pages
        .iter()
        .map(|bytes| Ok(PageId::from_bytes(id_from(bytes, "referencing page id")?)))
        .collect()
}

fn attachment_ids_referenced_by_on(
    conn: &Connection,
    page_id: PageId,
) -> Result<Vec<AttachmentId>, StorageError> {
    let mut stmt = conn.prepare(IDS_OF_PAGE_SQL)?;
    let ids = stmt
        .query_map([page_id.to_bytes()], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter()
        .map(|bytes| {
            Ok(AttachmentId::from_bytes(id_from(
                bytes,
                "referenced attachment id",
            )?))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    const fn attachment(n: u8) -> AttachmentId {
        AttachmentId::from_bytes([n; 16])
    }

    const fn space(n: u8) -> SpaceId {
        SpaceId::from_bytes([n; 16])
    }

    const fn page(n: u8) -> PageId {
        PageId::from_bytes([n; 16])
    }

    const fn block(n: u8) -> BlockId {
        BlockId::from_bytes([n; 16])
    }

    /// A row that differs from every other in each column.
    fn row(n: u8, space_n: u8, created_at: i64) -> AttachmentRow {
        AttachmentRow {
            attachment_id: attachment(n),
            space_id: space(space_n),
            name: format!("file-{n}.bin"),
            mime: "application/octet-stream".to_owned(),
            size: 1_000 * u64::from(n),
            chunk_count: u32::from(n),
            created_at,
            unreferenced_since: None,
        }
    }

    fn ids(rows: &[AttachmentRow]) -> Vec<AttachmentId> {
        rows.iter().map(|row| row.attachment_id).collect()
    }

    /// Every `attachment_refs` row, by attachment, page and block.
    fn raw_refs(conn: &Connection) -> Vec<AttachmentRef> {
        let rows = conn
            .prepare(
                "SELECT attachment_id, page_id, block_id FROM attachment_refs \
                 ORDER BY attachment_id, page_id, block_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, [u8; 16]>(0)?,
                    row.get::<_, [u8; 16]>(1)?,
                    row.get::<_, [u8; 16]>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.into_iter()
            .map(|(a, p, b)| AttachmentRef {
                attachment_id: AttachmentId::from_bytes(a),
                page_id: PageId::from_bytes(p),
                block_id: BlockId::from_bytes(b),
            })
            .collect()
    }

    const fn rf(attachment_n: u8, page_n: u8, block_n: u8) -> AttachmentRef {
        AttachmentRef {
            attachment_id: attachment(attachment_n),
            page_id: page(page_n),
            block_id: block(block_n),
        }
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that
    /// both `AttachmentQueries` impls are held to the same behaviour. The scenario gets the
    /// connection too, to look at the rows the way SQLite sees them.
    fn on_db_and_tx(scenario: impl Fn(&dyn AttachmentQueries, &Connection)) {
        let db = new_db();
        scenario(&db, db.connection());
        let mut db = new_db();
        db.transaction(|tx| {
            scenario(tx, &tx.tx);
            Ok(())
        })
        .unwrap();
    }

    // ---- the index rows -------------------------------------------------------------

    #[test]
    fn a_row_round_trips_with_every_column() {
        on_db_and_tx(|q, _| {
            assert_eq!(q.attachment_get(attachment(1)).unwrap(), None);
            assert!(q.attachments_all().unwrap().is_empty());

            let plain = row(1, 1, 1_000);
            let rich = AttachmentRow {
                attachment_id: attachment(2),
                space_id: space(2),
                name: "Café photo \u{1F389} (été).png".to_owned(),
                mime: "image/png".to_owned(),
                size: 4 * 1024 * 1024 + 1,
                chunk_count: 2,
                created_at: 1_760_000_200_000,
                unreferenced_since: Some(1_760_000_300_000),
            };
            let empty_file = AttachmentRow {
                attachment_id: attachment(3),
                space_id: space(2),
                name: String::new(),
                mime: String::new(),
                size: 0,
                chunk_count: 1,
                created_at: -5,
                unreferenced_since: Some(0),
            };
            for r in [&plain, &rich, &empty_file] {
                q.attachment_put(r).unwrap();
            }
            assert_eq!(q.attachment_get(attachment(1)).unwrap().unwrap(), plain);
            assert_eq!(q.attachment_get(attachment(2)).unwrap().unwrap(), rich);
            assert_eq!(
                q.attachment_get(attachment(3)).unwrap().unwrap(),
                empty_file
            );
            assert_eq!(q.attachment_get(attachment(4)).unwrap(), None);
            assert_eq!(q.attachments_all().unwrap().len(), 3);
        });
    }

    #[test]
    fn put_is_an_upsert_that_replaces_every_column_and_keeps_the_references() {
        on_db_and_tx(|q, conn| {
            q.attachment_put(&AttachmentRow {
                unreferenced_since: Some(50),
                ..row(1, 1, 1_000)
            })
            .unwrap();
            q.attachment_put(&row(2, 1, 2_000)).unwrap();
            q.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
                .unwrap();

            let replacement = AttachmentRow {
                attachment_id: attachment(1),
                space_id: space(9),
                name: "renamed".to_owned(),
                mime: "text/plain".to_owned(),
                size: 7,
                chunk_count: 1,
                created_at: 3_000,
                unreferenced_since: None,
            };
            q.attachment_put(&replacement).unwrap();

            assert_eq!(
                q.attachment_get(attachment(1)).unwrap().unwrap(),
                replacement
            );
            assert_eq!(
                q.attachment_get(attachment(2)).unwrap().unwrap(),
                row(2, 1, 2_000),
                "another row is not touched"
            );
            assert_eq!(q.attachments_all().unwrap().len(), 2, "no second row");
            assert_eq!(raw_refs(conn), [rf(1, 1, 1)], "the references stay");
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 1);
        });
    }

    #[test]
    fn set_space_moves_the_row_and_keeps_every_other_column_and_the_references() {
        on_db_and_tx(|q, conn| {
            q.attachment_put(&AttachmentRow {
                unreferenced_since: Some(50),
                ..row(1, 1, 1_000)
            })
            .unwrap();
            q.attachment_put(&row(2, 1, 2_000)).unwrap();
            q.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
                .unwrap();

            assert!(q.attachment_set_space(attachment(1), space(9)).unwrap());
            assert_eq!(
                q.attachment_get(attachment(1)).unwrap().unwrap(),
                AttachmentRow {
                    space_id: space(9),
                    unreferenced_since: Some(50),
                    ..row(1, 1, 1_000)
                },
                "only the space changed"
            );
            assert_eq!(
                q.attachment_get(attachment(2)).unwrap().unwrap(),
                row(2, 1, 2_000),
                "another row is not touched"
            );
            assert_eq!(
                ids(&q.attachments_by_space(space(9)).unwrap()),
                [attachment(1)]
            );
            assert_eq!(
                ids(&q.attachments_by_space(space(1)).unwrap()),
                [attachment(2)]
            );
            assert_eq!(raw_refs(conn), [rf(1, 1, 1)], "the references stay");

            assert!(!q.attachment_set_space(attachment(7), space(9)).unwrap());
            assert_eq!(q.attachment_get(attachment(7)).unwrap(), None);
        });
    }

    #[test]
    fn ids_and_numbers_are_stored_as_16_byte_blobs_and_integers() {
        on_db_and_tx(|q, conn| {
            q.attachment_put(&row(7, 8, 9)).unwrap();
            q.attachment_refs_replace(page(5), &[(block(6), attachment(7))])
                .unwrap();
            let (a, s, size, chunks, created): (String, String, String, String, String) = conn
                .query_row(
                    "SELECT typeof(attachment_id) || length(attachment_id), \
                            typeof(space_id) || length(space_id), typeof(size), \
                            typeof(chunk_count), typeof(created_at) \
                       FROM attachments_index",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .unwrap();
            assert_eq!((a.as_str(), s.as_str()), ("blob16", "blob16"));
            assert_eq!(
                (size.as_str(), chunks.as_str(), created.as_str()),
                ("integer", "integer", "integer")
            );
            let widths: (String, String, String) = conn
                .query_row(
                    "SELECT typeof(attachment_id) || length(attachment_id), \
                            typeof(page_id) || length(page_id), \
                            typeof(block_id) || length(block_id) FROM attachment_refs",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                widths,
                (
                    "blob16".to_owned(),
                    "blob16".to_owned(),
                    "blob16".to_owned()
                )
            );
        });
    }

    #[test]
    fn sizes_up_to_the_sqlite_integer_range_round_trip_and_larger_ones_are_refused() {
        on_db_and_tx(|q, _| {
            for size in [
                0,
                1,
                2 * 1024 * 1024 * 1024,
                u64::try_from(i64::MAX).unwrap(),
            ] {
                let r = AttachmentRow {
                    size,
                    ..row(1, 1, 1)
                };
                q.attachment_put(&r).unwrap();
                assert_eq!(q.attachment_get(attachment(1)).unwrap().unwrap(), r);
            }

            let too_big = AttachmentRow {
                size: u64::try_from(i64::MAX).unwrap() + 1,
                ..row(2, 1, 1)
            };
            let err = q.attachment_put(&too_big).unwrap_err();
            assert!(
                matches!(&err, StorageError::InvalidData(m) if m.contains("size")),
                "{err}"
            );
            assert_eq!(q.attachment_get(attachment(2)).unwrap(), None);
            assert_eq!(q.attachments_all().unwrap().len(), 1, "nothing was written");
        });
    }

    // ---- listings -------------------------------------------------------------------

    #[test]
    fn a_space_lists_newest_first_and_breaks_ties_by_id() {
        on_db_and_tx(|q, _| {
            // Written in neither the order of the times nor the order of the ids.
            for r in [
                row(5, 1, 2_000),
                row(2, 1, 3_000),
                row(9, 2, 9_999),
                row(1, 1, 2_000),
                row(7, 1, 1_000),
                row(3, 1, 2_000),
            ] {
                q.attachment_put(&r).unwrap();
            }
            assert_eq!(
                ids(&q.attachments_by_space(space(1)).unwrap()),
                [
                    attachment(2),
                    attachment(1),
                    attachment(3),
                    attachment(5),
                    attachment(7)
                ]
            );
            assert_eq!(
                ids(&q.attachments_by_space(space(2)).unwrap()),
                [attachment(9)]
            );
            assert!(q.attachments_by_space(space(3)).unwrap().is_empty());
        });
    }

    #[test]
    fn all_lists_every_space_oldest_first_and_breaks_ties_by_id() {
        on_db_and_tx(|q, _| {
            assert!(q.attachments_all().unwrap().is_empty());
            for r in [
                row(5, 1, 2_000),
                row(2, 1, 3_000),
                row(9, 2, 500),
                row(1, 2, 2_000),
                row(7, 1, 1_000),
            ] {
                q.attachment_put(&r).unwrap();
            }
            assert_eq!(
                ids(&q.attachments_all().unwrap()),
                [
                    attachment(9),
                    attachment(7),
                    attachment(1),
                    attachment(5),
                    attachment(2)
                ]
            );
        });
    }

    // ---- delete ---------------------------------------------------------------------

    #[test]
    fn delete_removes_the_row_and_its_references_and_nothing_else() {
        on_db_and_tx(|q, conn| {
            q.attachment_put(&row(1, 1, 1)).unwrap();
            q.attachment_put(&row(2, 1, 2)).unwrap();
            q.attachment_refs_replace(
                page(1),
                &[(block(1), attachment(1)), (block(2), attachment(2))],
            )
            .unwrap();
            q.attachment_refs_replace(page(2), &[(block(1), attachment(1))])
                .unwrap();

            q.attachment_delete(attachment(1)).unwrap();
            assert_eq!(q.attachment_get(attachment(1)).unwrap(), None);
            assert_eq!(
                q.attachment_get(attachment(2)).unwrap().unwrap(),
                row(2, 1, 2)
            );
            assert_eq!(raw_refs(conn), [rf(2, 1, 2)]);
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 0);

            // Deleting what is not there succeeds and changes nothing.
            q.attachment_delete(attachment(1)).unwrap();
            q.attachment_delete(attachment(9)).unwrap();
            assert_eq!(q.attachments_all().unwrap().len(), 1);
            assert_eq!(raw_refs(conn), [rf(2, 1, 2)]);
        });
    }

    #[test]
    fn delete_removes_references_of_an_attachment_that_has_no_row() {
        on_db_and_tx(|q, conn| {
            q.attachment_refs_replace(page(1), &[(block(1), attachment(4))])
                .unwrap();
            q.attachment_delete(attachment(4)).unwrap();
            assert!(raw_refs(conn).is_empty());
        });
    }

    // ---- references -----------------------------------------------------------------

    #[test]
    fn refs_replace_makes_the_references_of_the_page_exactly_the_given_set() {
        on_db_and_tx(|q, conn| {
            q.attachment_refs_replace(
                page(1),
                &[(block(1), attachment(1)), (block(2), attachment(2))],
            )
            .unwrap();
            q.attachment_refs_replace(page(2), &[(block(3), attachment(1))])
                .unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 1, 1), rf(1, 2, 3), rf(2, 1, 2)]);

            // One stays, one goes, one comes; the other page keeps its row.
            q.attachment_refs_replace(
                page(1),
                &[(block(2), attachment(2)), (block(4), attachment(3))],
            )
            .unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 2, 3), rf(2, 1, 2), rf(3, 1, 4)]);

            // The same set again changes nothing.
            q.attachment_refs_replace(
                page(1),
                &[(block(4), attachment(3)), (block(2), attachment(2))],
            )
            .unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 2, 3), rf(2, 1, 2), rf(3, 1, 4)]);

            // An empty set clears the page and only the page.
            q.attachment_refs_replace(page(1), &[]).unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 2, 3)]);
            q.attachment_refs_replace(page(1), &[]).unwrap();
            q.attachment_refs_replace(page(9), &[]).unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 2, 3)]);
        });
    }

    #[test]
    fn refs_replace_collapses_a_pair_given_twice_and_keeps_distinct_blocks_and_attachments() {
        on_db_and_tx(|q, conn| {
            q.attachment_refs_replace(
                page(1),
                &[
                    (block(1), attachment(1)),
                    (block(1), attachment(1)),
                    (block(2), attachment(1)),
                    (block(1), attachment(2)),
                ],
            )
            .unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 1, 1), rf(1, 1, 2), rf(2, 1, 1)]);
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 2);
        });
    }

    #[test]
    fn refs_replace_does_not_need_an_index_row_and_does_not_create_one() {
        on_db_and_tx(|q, conn| {
            q.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
                .unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 1, 1)]);
            assert!(q.attachments_all().unwrap().is_empty());
        });
    }

    #[test]
    fn refs_delete_pages_removes_the_references_of_those_pages_only() {
        on_db_and_tx(|q, conn| {
            for p in 1..=4 {
                q.attachment_refs_replace(
                    page(p),
                    &[(block(1), attachment(1)), (block(2), attachment(p))],
                )
                .unwrap();
            }
            q.attachment_refs_delete_pages(&[page(2), page(4), page(9)])
                .unwrap();
            // Page p holds block 1 -> attachment 1 and block 2 -> attachment p.
            assert_eq!(
                raw_refs(conn),
                [rf(1, 1, 1), rf(1, 1, 2), rf(1, 3, 1), rf(3, 3, 2)]
            );
        });
    }

    #[test]
    fn refs_delete_pages_with_no_pages_changes_nothing() {
        on_db_and_tx(|q, conn| {
            q.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
                .unwrap();
            q.attachment_refs_delete_pages(&[]).unwrap();
            assert_eq!(raw_refs(conn), [rf(1, 1, 1)]);
            q.attachment_refs_delete_pages(&[page(1), page(1)]).unwrap();
            assert!(raw_refs(conn).is_empty());
        });
    }

    #[test]
    fn the_reference_count_counts_pages_and_blocks_and_is_zero_for_the_unknown() {
        on_db_and_tx(|q, _| {
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 0);
            q.attachment_refs_replace(
                page(1),
                &[(block(1), attachment(1)), (block(2), attachment(1))],
            )
            .unwrap();
            q.attachment_refs_replace(page(2), &[(block(1), attachment(1))])
                .unwrap();
            q.attachment_refs_replace(page(3), &[(block(1), attachment(2))])
                .unwrap();
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 3);
            assert_eq!(q.attachment_reference_count(attachment(2)).unwrap(), 1);
            assert_eq!(q.attachment_reference_count(attachment(3)).unwrap(), 0);

            q.attachment_refs_delete_pages(&[page(1)]).unwrap();
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 1);
        });
    }

    #[test]
    fn the_attachments_of_a_page_are_listed_once_each_ordered_by_id() {
        on_db_and_tx(|q, _| {
            assert!(q.attachment_ids_referenced_by(page(1)).unwrap().is_empty());
            q.attachment_refs_replace(
                page(1),
                &[
                    (block(1), attachment(3)),
                    (block(2), attachment(1)),
                    (block(3), attachment(3)),
                ],
            )
            .unwrap();
            q.attachment_refs_replace(page(2), &[(block(1), attachment(2))])
                .unwrap();
            assert_eq!(
                q.attachment_ids_referenced_by(page(1)).unwrap(),
                [attachment(1), attachment(3)]
            );
            assert_eq!(
                q.attachment_ids_referenced_by(page(2)).unwrap(),
                [attachment(2)]
            );
            assert!(q.attachment_ids_referenced_by(page(3)).unwrap().is_empty());
        });
    }

    #[test]
    fn the_pages_that_reference_an_attachment_are_listed_once_each_ordered_by_id() {
        on_db_and_tx(|q, conn| {
            assert!(
                q.attachment_referencing_pages(attachment(1))
                    .unwrap()
                    .is_empty()
            );
            // Page 3: two blocks; page 1: a block and a property value; page 2: a property value
            // and an icon; page 4 references another attachment.
            q.attachment_refs_replace(
                page(3),
                &[(block(1), attachment(1)), (block(2), attachment(1))],
            )
            .unwrap();
            q.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
                .unwrap();
            q.attachment_refs_replace(page(4), &[(block(1), attachment(2))])
                .unwrap();
            attach(conn, 1, 1, &[1]);
            attach(conn, 2, 1, &[1]);
            conn.execute(
                "INSERT INTO icon_refs (page_id, space_id, attachment_id) VALUES (?1, ?2, ?3)",
                params![[2_u8; 16], [1_u8; 16], [1_u8; 16]],
            )
            .unwrap();

            assert_eq!(
                q.attachment_referencing_pages(attachment(1)).unwrap(),
                [page(1), page(2), page(3)]
            );
            assert_eq!(
                q.attachment_referencing_pages(attachment(2)).unwrap(),
                [page(4)]
            );
            assert!(
                q.attachment_referencing_pages(attachment(3))
                    .unwrap()
                    .is_empty()
            );
        });
    }

    /// Makes the attachments a property of a page attaches exactly `attachments`: a `files` row
    /// each, written straight into `page_properties` (the core writes them with the page's values).
    fn attach(conn: &Connection, page_n: u8, property_n: u8, attachments: &[u8]) {
        conn.execute(
            "DELETE FROM page_properties WHERE page_id = ?1 AND property_id = ?2",
            params![[page_n; 16], [property_n; 16]],
        )
        .unwrap();
        for n in attachments {
            conn.execute(
                "INSERT INTO page_properties (space_id, page_id, property_id, kind, attachment_id) \
                 VALUES (?1, ?2, ?3, 'files', ?4)",
                params![[1_u8; 16], [page_n; 16], [property_n; 16], [*n; 16]],
            )
            .unwrap();
        }
    }

    #[test]
    fn a_file_attached_by_a_property_value_is_a_reference_like_a_block() {
        on_db_and_tx(|q, conn| {
            q.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
                .unwrap();
            attach(conn, 2, 1, &[1, 2]);
            attach(conn, 3, 1, &[1]);
            attach(conn, 3, 2, &[1]);
            assert_eq!(
                q.attachment_reference_count(attachment(1)).unwrap(),
                4,
                "one block, and three (page, property) values"
            );
            assert_eq!(q.attachment_reference_count(attachment(2)).unwrap(), 1);
            assert_eq!(q.attachment_reference_count(attachment(3)).unwrap(), 0);
            assert_eq!(
                q.attachment_ids_referenced_by(page(3)).unwrap(),
                [attachment(1)],
                "once, although two properties attach it"
            );
            q.attachment_refs_replace(page(2), &[(block(5), attachment(3))])
                .unwrap();
            assert_eq!(
                q.attachment_ids_referenced_by(page(2)).unwrap(),
                [attachment(1), attachment(2), attachment(3)],
                "the blocks and the property values together, ordered by id"
            );

            // Clearing the values (what the next write of the page's values does) drops them.
            attach(conn, 2, 1, &[]);
            attach(conn, 3, 1, &[]);
            attach(conn, 3, 2, &[]);
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 1);
            assert_eq!(q.attachment_reference_count(attachment(2)).unwrap(), 0);
        });
    }

    /// Makes the page's icon the attachment (`None`: no icon), written straight into `icon_refs`
    /// (the core writes the rows with the structure document).
    fn set_icon(conn: &Connection, page_n: u8, attachment_n: Option<u8>) {
        conn.execute("DELETE FROM icon_refs WHERE page_id = ?1", [[page_n; 16]])
            .unwrap();
        if let Some(n) = attachment_n {
            conn.execute(
                "INSERT INTO icon_refs (page_id, space_id, attachment_id) VALUES (?1, ?2, ?3)",
                params![[page_n; 16], [1_u8; 16], [n; 16]],
            )
            .unwrap();
        }
    }

    #[test]
    fn a_page_icon_is_a_reference_like_a_block_and_a_file() {
        on_db_and_tx(|q, conn| {
            q.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
                .unwrap();
            attach(conn, 2, 1, &[1]);
            set_icon(conn, 3, Some(1));
            set_icon(conn, 4, Some(1));
            set_icon(conn, 5, Some(2));
            assert_eq!(
                q.attachment_reference_count(attachment(1)).unwrap(),
                4,
                "one block, one file value and two page icons"
            );
            assert_eq!(q.attachment_reference_count(attachment(2)).unwrap(), 1);
            assert_eq!(q.attachment_reference_count(attachment(3)).unwrap(), 0);

            assert_eq!(
                q.attachment_ids_referenced_by(page(3)).unwrap(),
                [attachment(1)]
            );
            assert!(q.attachment_ids_referenced_by(page(6)).unwrap().is_empty());
            q.attachment_refs_replace(
                page(3),
                &[(block(1), attachment(1)), (block(2), attachment(3))],
            )
            .unwrap();
            attach(conn, 3, 1, &[2]);
            assert_eq!(
                q.attachment_ids_referenced_by(page(3)).unwrap(),
                [attachment(1), attachment(2), attachment(3)],
                "blocks, file values and the icon together, each once, ordered by id"
            );

            // Another icon, or none, drops the reference.
            set_icon(conn, 3, Some(2));
            set_icon(conn, 4, None);
            assert_eq!(q.attachment_reference_count(attachment(1)).unwrap(), 3);
            assert_eq!(q.attachment_reference_count(attachment(2)).unwrap(), 3);
        });
    }

    // ---- unreferenced_since ---------------------------------------------------------

    #[test]
    fn unreferenced_since_is_set_and_cleared_without_touching_anything_else() {
        on_db_and_tx(|q, _| {
            q.attachment_put(&row(1, 1, 10)).unwrap();
            q.attachment_put(&row(2, 1, 20)).unwrap();

            q.attachment_set_unreferenced_since(attachment(1), Some(777))
                .unwrap();
            assert_eq!(
                q.attachment_get(attachment(1)).unwrap().unwrap(),
                AttachmentRow {
                    unreferenced_since: Some(777),
                    ..row(1, 1, 10)
                }
            );
            assert_eq!(
                q.attachment_get(attachment(2)).unwrap().unwrap(),
                row(2, 1, 20)
            );

            q.attachment_set_unreferenced_since(attachment(1), Some(-3))
                .unwrap();
            assert_eq!(
                q.attachment_get(attachment(1))
                    .unwrap()
                    .unwrap()
                    .unreferenced_since,
                Some(-3)
            );
            q.attachment_set_unreferenced_since(attachment(1), None)
                .unwrap();
            assert_eq!(
                q.attachment_get(attachment(1)).unwrap().unwrap(),
                row(1, 1, 10)
            );
        });
    }

    #[test]
    fn unreferenced_since_of_an_unknown_attachment_is_a_no_op() {
        on_db_and_tx(|q, _| {
            q.attachment_set_unreferenced_since(attachment(1), Some(5))
                .unwrap();
            q.attachment_set_unreferenced_since(attachment(1), None)
                .unwrap();
            assert!(q.attachments_all().unwrap().is_empty(), "no row appears");
        });
    }

    #[test]
    fn unreferenced_before_is_strict_skips_referenced_rows_and_lists_the_longest_first() {
        on_db_and_tx(|q, _| {
            for (n, since) in [
                (1, None),
                (2, Some(30)),
                (3, Some(10)),
                (4, Some(20)),
                (5, Some(20)),
                (6, Some(-1)),
            ] {
                q.attachment_put(&AttachmentRow {
                    unreferenced_since: since,
                    ..row(n, 1, i64::from(n))
                })
                .unwrap();
            }
            let before = |cutoff| ids(&q.attachments_unreferenced_before(cutoff).unwrap());

            assert!(before(i64::MIN).is_empty());
            assert!(
                before(-1).is_empty(),
                "strictly before: -1 is not before -1"
            );
            assert_eq!(before(0), [attachment(6)]);
            assert_eq!(before(10), [attachment(6)]);
            assert_eq!(before(11), [attachment(6), attachment(3)]);
            assert_eq!(before(20), [attachment(6), attachment(3)]);
            assert_eq!(
                before(21),
                [attachment(6), attachment(3), attachment(4), attachment(5)]
            );
            assert_eq!(
                before(i64::MAX),
                [
                    attachment(6),
                    attachment(3),
                    attachment(4),
                    attachment(5),
                    attachment(2)
                ],
                "the referenced row (no time) is never listed"
            );
            let listed = q.attachments_unreferenced_before(11).unwrap();
            assert_eq!(
                listed[1].unreferenced_since,
                Some(10),
                "whole rows come back"
            );
            assert_eq!(listed[1].name, "file-3.bin");
        });
    }

    // ---- damaged rows ---------------------------------------------------------------

    #[test]
    fn damaged_rows_are_invalid_data_and_never_a_panic_or_an_echo_of_the_name() {
        for (values, what) in [
            (
                "zeroblob(15), zeroblob(16), 'SECRET-NAME', 'a/b', 1, 1, 1, NULL",
                "a 15-byte attachment id",
            ),
            (
                "zeroblob(16), zeroblob(17), 'SECRET-NAME', 'a/b', 1, 1, 1, NULL",
                "a 17-byte space id",
            ),
            (
                "zeroblob(16), zeroblob(16), 'SECRET-NAME', 'a/b', -1, 1, 1, NULL",
                "a negative size",
            ),
            (
                "zeroblob(16), zeroblob(16), 'SECRET-NAME', 'a/b', 1, -1, 1, NULL",
                "a negative chunk count",
            ),
            (
                "zeroblob(16), zeroblob(16), 'SECRET-NAME', 'a/b', 1, 4294967296, 1, NULL",
                "a chunk count above u32",
            ),
        ] {
            let db = new_db();
            db.connection()
                .execute(
                    &format!("INSERT INTO attachments_index VALUES ({values})"),
                    [],
                )
                .unwrap();
            let err = db.attachments_all().unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{what}: {err}");
            assert!(!err.to_string().contains("SECRET-NAME"), "{what}: {err}");
        }
    }

    #[test]
    fn a_damaged_row_is_found_by_every_listing_that_reaches_it() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO attachments_index VALUES \
                 (zeroblob(16), zeroblob(16), 'n', 'a/b', -5, 1, 1, 100)",
                [],
            )
            .unwrap();
        let zero_attachment = AttachmentId::from_bytes([0; 16]);
        let zero_space = SpaceId::from_bytes([0; 16]);
        assert!(matches!(
            db.attachment_get(zero_attachment).unwrap_err(),
            StorageError::InvalidData(_)
        ));
        assert!(matches!(
            db.attachments_by_space(zero_space).unwrap_err(),
            StorageError::InvalidData(_)
        ));
        assert!(matches!(
            db.attachments_all().unwrap_err(),
            StorageError::InvalidData(_)
        ));
        assert!(matches!(
            db.attachments_unreferenced_before(i64::MAX).unwrap_err(),
            StorageError::InvalidData(_)
        ));
    }

    #[test]
    fn a_damaged_reference_id_is_invalid_data() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO attachment_refs VALUES (zeroblob(15), x'01010101010101010101010101010101', zeroblob(16))",
                [],
            )
            .unwrap();
        let err = db.attachment_ids_referenced_by(page(1)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    // ---- transactions ---------------------------------------------------------------

    #[test]
    fn writes_commit_with_the_transaction_and_reads_see_them_before_that() {
        let mut db = new_db();
        db.transaction(|tx| {
            tx.attachment_put(&row(1, 1, 10))?;
            tx.attachment_put(&row(2, 1, 20))?;
            tx.attachment_refs_replace(page(1), &[(block(1), attachment(1))])?;
            tx.attachment_set_unreferenced_since(attachment(2), Some(99))?;

            // The transaction reads its own writes.
            assert_eq!(tx.attachments_all()?.len(), 2);
            assert_eq!(tx.attachment_reference_count(attachment(1))?, 1);
            assert_eq!(tx.attachments_unreferenced_before(100)?.len(), 1);
            Ok(())
        })
        .unwrap();

        assert!(db.connection().is_autocommit(), "the transaction is over");
        assert_eq!(db.attachments_all().unwrap().len(), 2);
        assert_eq!(db.attachment_reference_count(attachment(1)).unwrap(), 1);
        assert_eq!(db.attachments_unreferenced_before(100).unwrap().len(), 1);
    }

    #[test]
    fn writes_roll_back_with_a_transaction_that_fails() {
        let mut db = new_db();
        // Some committed state first.
        db.attachment_put(&row(1, 1, 10)).unwrap();
        db.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
            .unwrap();

        let err = db
            .transaction(|tx| {
                tx.attachment_put(&row(2, 1, 20))?;
                tx.attachment_put(&AttachmentRow {
                    name: "replaced".to_owned(),
                    ..row(1, 1, 10)
                })?;
                tx.attachment_refs_replace(page(1), &[(block(5), attachment(2))])?;
                tx.attachment_refs_replace(page(2), &[(block(6), attachment(2))])?;
                tx.attachment_set_unreferenced_since(attachment(1), Some(5))?;
                tx.attachment_refs_delete_pages(&[page(1)])?;
                tx.attachment_delete(attachment(1))?;
                Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m == "boom"),
            "{err}"
        );

        assert_eq!(db.attachments_all().unwrap(), [row(1, 1, 10)]);
        assert_eq!(raw_refs(db.connection()), [rf(1, 1, 1)]);
        assert!(db.connection().is_autocommit(), "the transaction is over");

        // The connection is fine afterwards.
        db.transaction(|tx| tx.attachment_put(&row(2, 1, 20)))
            .unwrap();
        assert_eq!(db.attachments_all().unwrap().len(), 2);
    }

    #[test]
    fn a_panic_inside_the_transaction_rolls_the_attachment_writes_back() {
        let mut db = new_db();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = db.transaction::<()>(|tx| {
                tx.attachment_put(&row(1, 1, 10))?;
                tx.attachment_refs_replace(page(1), &[(block(1), attachment(1))])?;
                panic!("boom");
            });
        }));
        assert!(outcome.is_err());
        assert!(db.attachments_all().unwrap().is_empty());
        assert!(raw_refs(db.connection()).is_empty());
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_bare_db_replaces_the_references_of_a_page_all_or_nothing() {
        let db = new_db();
        db.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
            .unwrap();
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE INSERT ON attachment_refs
                 WHEN NEW.block_id = x'09090909090909090909090909090909'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();

        let err = db
            .attachment_refs_replace(
                page(1),
                &[
                    (block(3), attachment(3)),
                    (block(9), attachment(3)),
                    (block(5), attachment(3)),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            raw_refs(db.connection()),
            [rf(1, 1, 1)],
            "the old rows are back and none of the new ones is there"
        );
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_bare_db_deletes_an_attachment_and_its_references_all_or_nothing() {
        let db = new_db();
        db.attachment_put(&row(1, 1, 10)).unwrap();
        db.attachment_refs_replace(page(1), &[(block(1), attachment(1))])
            .unwrap();
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE DELETE ON attachments_index
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();

        let err = db.attachment_delete(attachment(1)).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(db.attachments_all().unwrap(), [row(1, 1, 10)]);
        assert_eq!(
            raw_refs(db.connection()),
            [rf(1, 1, 1)],
            "the references that were already deleted are back"
        );
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn a_bare_db_deletes_the_references_of_several_pages_all_or_nothing() {
        let db = new_db();
        for p in 1..=3 {
            db.attachment_refs_replace(page(p), &[(block(1), attachment(1))])
                .unwrap();
        }
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE DELETE ON attachment_refs
                 WHEN OLD.page_id = x'03030303030303030303030303030303'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();

        let err = db
            .attachment_refs_delete_pages(&[page(1), page(2), page(3)])
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            raw_refs(db.connection()),
            [rf(1, 1, 1), rf(1, 2, 1), rf(1, 3, 1)],
            "pages 1 and 2 were deleted first and are back"
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
    fn the_statements_use_the_indexes_the_migration_creates() {
        let by_space = plan_of(BY_SPACE_SQL);
        assert!(
            by_space.contains("USING INDEX attachments_index_space (space_id=?)"),
            "a space is listed through its index: {by_space}"
        );
        assert!(
            !by_space.contains("SCAN"),
            "no scan of the whole table: {by_space}"
        );
        for sql in [GET_SQL, DELETE_ROW_SQL, SET_UNREFERENCED_SINCE_SQL] {
            let plan = plan_of(sql);
            assert!(
                plan.contains("USING INDEX sqlite_autoindex_attachments_index_1 (attachment_id=?)"),
                "by primary key: {plan} for {sql}"
            );
        }
        for sql in [DELETE_REFS_OF_PAGE_SQL, IDS_OF_PAGE_SQL] {
            let plan = plan_of(sql);
            assert!(
                plan.contains("INDEX attachment_refs_page (page_id=?)"),
                "a page's references through its index: {plan} for {sql}"
            );
        }
        for sql in [REF_COUNT_SQL, DELETE_REFS_OF_ATTACHMENT_SQL] {
            let plan = plan_of(sql);
            assert!(
                plan.contains("INDEX sqlite_autoindex_attachment_refs_1 (attachment_id=?)"),
                "an attachment's references through the primary key: {plan} for {sql}"
            );
        }
        let count = plan_of(REF_COUNT_SQL);
        assert!(
            count.contains("INDEX icon_refs_by_attachment (attachment_id=?)"),
            "the icons of pages through their index: {count}"
        );
        let ids = plan_of(IDS_OF_PAGE_SQL);
        assert!(
            ids.contains("sqlite_autoindex_icon_refs_1 (page_id=?)"),
            "the icon of a page through the primary key: {ids}"
        );
    }
}
