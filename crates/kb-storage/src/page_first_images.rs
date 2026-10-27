//! The first picture of each page (schema v19, ADR-0054, `docs/architecture/database-views.md` section 9). Derived
//! data, device-local: the core rewrites the row of a page in the same transaction as the rest of the page's
//! derived rows (the text index, the links, the attachment references), clears the table with `IndexRebuild` of the
//! whole workspace, and scans the pages that have no row when a gallery asks for them (`Request::PageFirstImages`).
//! The table is neither synced nor part of a backup.
//!
//! Same conventions as [`crate::icon_refs`]: every method exists on [`Db`] and [`Tx`] through
//! [`PageFirstImageQueries`], both running the same private functions over a connection (a transaction derefs to
//! one), so a method behaves identically in either place; the multi-statement methods of a [`Db`] run as one atomic
//! unit. Ids are bound as 16-byte blobs. Rows are read as raw column values first and validated afterwards, so a
//! damaged row is a typed `InvalidData` error and never a panic.
//!
//! # The table
//!
//! `page_first_images(page_id PRIMARY KEY, space_id, attachment_id NULL)`: one row per page that was scanned, trashed
//! pages included. A null `attachment_id` is "scanned, the page has no picture"; no row is "not scanned yet". There is
//! deliberately no foreign key and no index by attachment: the attachment is already referenced by its block, so the
//! table adds no retention, and a row may name an attachment that was purged since (the card then shows the page's icon,
//! and the next write of the page rewrites the row).

use kb_types::{AttachmentId, PageId, SpaceId};
use rusqlite::{Connection, params};

use crate::docs::{atomically, id_from};
use crate::{Db, StorageError, Tx};

/// Queries of the `page_first_images` table, available on a connection or inside a transaction. On a [`Db`] every
/// method is atomic by itself.
pub trait PageFirstImageQueries {
    /// Makes the row of each given page exactly the attachment given (`None`: scanned, no picture), in the space
    /// given. Pages not in the list are not touched; a page given twice keeps its last entry. One atomic unit.
    fn first_images_set(
        &self,
        space_id: SpaceId,
        pages: &[(PageId, Option<AttachmentId>)],
    ) -> Result<(), StorageError>;
    /// The rows of the given pages that have one, ordered by page id: the attachment of the page's first picture, or
    /// `None` for a page that was scanned and has none. A page with no row is not in the answer (and neither is an
    /// unknown page).
    fn first_images_get(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<(PageId, Option<AttachmentId>)>, StorageError>;
    /// Removes the rows of the given pages (purge). Unknown pages and an empty list succeed. One atomic unit.
    fn first_images_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError>;
    /// Removes every row (`IndexRebuild` of the whole workspace; the pages are scanned again when asked about).
    fn first_images_clear(&self) -> Result<(), StorageError>;
}

impl PageFirstImageQueries for Db {
    fn first_images_set(
        &self,
        space_id: SpaceId,
        pages: &[(PageId, Option<AttachmentId>)],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            first_images_set_on(conn, space_id, pages)
        })
    }
    fn first_images_get(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<(PageId, Option<AttachmentId>)>, StorageError> {
        first_images_get_on(&self.conn, pages)
    }
    fn first_images_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| first_images_delete_pages_on(conn, pages))
    }
    fn first_images_clear(&self) -> Result<(), StorageError> {
        first_images_clear_on(&self.conn)
    }
}

impl PageFirstImageQueries for Tx<'_> {
    fn first_images_set(
        &self,
        space_id: SpaceId,
        pages: &[(PageId, Option<AttachmentId>)],
    ) -> Result<(), StorageError> {
        first_images_set_on(&self.tx, space_id, pages)
    }
    fn first_images_get(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<(PageId, Option<AttachmentId>)>, StorageError> {
        first_images_get_on(&self.tx, pages)
    }
    fn first_images_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        first_images_delete_pages_on(&self.tx, pages)
    }
    fn first_images_clear(&self) -> Result<(), StorageError> {
        first_images_clear_on(&self.tx)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// Through the primary key (the page).
const UPSERT_SQL: &str = "INSERT INTO page_first_images (page_id, space_id, attachment_id) \
     VALUES (?1, ?2, ?3) \
     ON CONFLICT (page_id) DO UPDATE SET space_id = excluded.space_id, attachment_id = excluded.attachment_id";

/// Through the primary key.
const GET_SQL: &str = "SELECT attachment_id FROM page_first_images WHERE page_id = ?1";

/// Through the primary key.
const DELETE_SQL: &str = "DELETE FROM page_first_images WHERE page_id = ?1";

fn first_images_set_on(
    conn: &Connection,
    space_id: SpaceId,
    pages: &[(PageId, Option<AttachmentId>)],
) -> Result<(), StorageError> {
    let mut upsert = conn.prepare(UPSERT_SQL)?;
    for (page_id, attachment_id) in pages {
        upsert.execute(params![
            page_id.to_bytes(),
            space_id.to_bytes(),
            attachment_id.map(AttachmentId::to_bytes)
        ])?;
    }
    Ok(())
}

fn first_images_get_on(
    conn: &Connection,
    pages: &[PageId],
) -> Result<Vec<(PageId, Option<AttachmentId>)>, StorageError> {
    let mut stmt = conn.prepare(GET_SQL)?;
    let mut found: Vec<(PageId, Option<Vec<u8>>)> = Vec::new();
    for page_id in pages {
        let rows = stmt
            .query_map([page_id.to_bytes()], |row| row.get::<_, Option<Vec<u8>>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(attachment) = rows.into_iter().next() {
            found.push((*page_id, attachment));
        }
    }
    found.sort_unstable_by_key(|(page_id, _)| *page_id);
    found.dedup_by_key(|(page_id, _)| *page_id);
    found
        .into_iter()
        .map(|(page_id, attachment)| {
            let attachment = attachment
                .map(|bytes| {
                    id_from(&bytes, "first image attachment id").map(AttachmentId::from_bytes)
                })
                .transpose()?;
            Ok((page_id, attachment))
        })
        .collect()
}

fn first_images_delete_pages_on(conn: &Connection, pages: &[PageId]) -> Result<(), StorageError> {
    let mut delete = conn.prepare(DELETE_SQL)?;
    for page_id in pages {
        delete.execute([page_id.to_bytes()])?;
    }
    Ok(())
}

fn first_images_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM page_first_images", [])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
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

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that both
    /// `PageFirstImageQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn PageFirstImageQueries, &Connection)) {
        let db = new_db();
        scenario(&db, db.connection());
        let mut db = new_db();
        db.transaction(|tx| {
            scenario(tx, &tx.tx);
            Ok(())
        })
        .unwrap();
    }

    fn rows(q: &dyn PageFirstImageQueries, pages: &[u8]) -> Vec<(u8, Option<u8>)> {
        let asked: Vec<PageId> = pages.iter().map(|n| page(*n)).collect();
        q.first_images_get(&asked)
            .unwrap()
            .into_iter()
            .map(|(p, a)| (p.to_bytes()[0], a.map(|a| a.to_bytes()[0])))
            .collect()
    }

    #[test]
    fn set_writes_and_replaces_the_rows_of_the_given_pages_only() {
        on_db_and_tx(|q, _| {
            q.first_images_set(
                space(1),
                &[
                    (page(3), Some(attachment(30))),
                    (page(1), Some(attachment(10))),
                    (page(2), None),
                ],
            )
            .unwrap();
            assert_eq!(
                rows(q, &[1, 2, 3]),
                [(1, Some(10)), (2, None), (3, Some(30))],
                "ordered by page id; a null attachment is a scanned page without a picture"
            );

            // A row is replaced whole (the space too), the others stay; a page given twice keeps its last entry.
            q.first_images_set(
                space(2),
                &[
                    (page(1), None),
                    (page(2), Some(attachment(20))),
                    (page(4), Some(attachment(1))),
                    (page(4), Some(attachment(40))),
                ],
            )
            .unwrap();
            assert_eq!(
                rows(q, &[1, 2, 3, 4]),
                [(1, None), (2, Some(20)), (3, Some(30)), (4, Some(40))]
            );

            // An empty list changes nothing.
            q.first_images_set(space(1), &[]).unwrap();
            assert_eq!(rows(q, &[1, 2, 3, 4]).len(), 4);
        });
    }

    #[test]
    fn get_answers_the_rows_that_exist_for_the_pages_asked_once_each() {
        on_db_and_tx(|q, _| {
            q.first_images_set(
                space(1),
                &[(page(1), Some(attachment(10))), (page(2), None)],
            )
            .unwrap();
            // An unknown page and a page with no row are not in the answer; a page asked twice is answered once.
            assert_eq!(
                rows(q, &[9, 2, 1, 1, 8]),
                [(1, Some(10)), (2, None)],
                "nothing for 8 and 9"
            );
            assert!(rows(q, &[]).is_empty());
        });
    }

    #[test]
    fn delete_removes_the_rows_of_the_given_pages_and_clear_removes_every_row() {
        on_db_and_tx(|q, _| {
            q.first_images_set(
                space(1),
                &[
                    (page(1), Some(attachment(10))),
                    (page(2), None),
                    (page(3), Some(attachment(30))),
                ],
            )
            .unwrap();
            q.first_images_delete_pages(&[page(1), page(2), page(77)])
                .unwrap();
            assert_eq!(rows(q, &[1, 2, 3]), [(3, Some(30))]);
            q.first_images_delete_pages(&[]).unwrap();
            assert_eq!(rows(q, &[3]), [(3, Some(30))]);
            q.first_images_clear().unwrap();
            assert!(rows(q, &[1, 2, 3]).is_empty());
            q.first_images_clear().unwrap();
        });
    }

    #[test]
    fn a_damaged_row_is_a_typed_error_and_never_a_panic() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO page_first_images VALUES (?1, ?2, x'0102')",
                params![page(1).to_bytes(), space(1).to_bytes()],
            )
            .unwrap();
        assert!(matches!(
            db.first_images_get(&[page(1)]),
            Err(StorageError::InvalidData(_))
        ));
    }

    #[test]
    fn the_lookup_goes_through_the_primary_key() {
        let db = new_db();
        let plan: Vec<String> = db
            .connection()
            .prepare(&format!("EXPLAIN QUERY PLAN {GET_SQL}"))
            .unwrap()
            .query_map([page(1).to_bytes()], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("PRIMARY KEY") || step.contains("autoindex")),
            "{plan:?}"
        );
    }
}
