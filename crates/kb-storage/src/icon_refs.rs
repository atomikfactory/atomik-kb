//! The image icons and the image covers of the pages (schema v14, ADR-0029, and v16, ADR-0040).
//! Derived data: the icon and the cover of a page live in the page's meta map of the structure
//! document (`icon` and `cover`, each `"attachment:<attachment id>"` when an uploaded image), so the
//! core rewrites the rows of the pages that changed in the same transaction as the update row of the
//! document (like [`crate::PagePropertyQueries`] for the property values), compares the whole table
//! with the documents at open, and the table is neither synced nor part of a backup.
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`] through
//! [`IconRefQueries`], both running the same private functions over a connection (a transaction
//! derefs to one), so a method behaves identically in either place; the multi-statement methods of
//! a [`Db`] run as one atomic unit. Ids are bound as 16-byte blobs. Rows are read as raw column
//! values first and validated afterwards, so a damaged row is a typed `InvalidData` error and never
//! a panic.
//!
//! # The table
//!
//! `icon_refs(page_id, space_id, attachment_id, PRIMARY KEY (page_id, attachment_id))`: one row per
//! page and uploaded image its icon or its cover uses, trashed pages included (a trashed page can be
//! restored, so it keeps them). The same image as icon and as cover is one row. There is
//! deliberately no foreign key to the attachment index: a page can name an attachment that was
//! purged (the icon or cover then shows as the default) and its row stays until the icon or cover
//! changes or the page goes. Before schema v16 the key was the page alone (a page had one icon); the
//! migration keeps every row.
//!
//! The rows are attachment references: [`crate::AttachmentQueries::attachment_reference_count`]
//! adds them to the blocks that show an attachment and to the `files` values, one reference per
//! page, and [`crate::AttachmentQueries::attachment_ids_referenced_by`] lists the attachments of the
//! icon and the cover with the others, so an image that only an icon or a cover holds is kept by
//! the retention of attachments.

use kb_types::{AttachmentId, PageId, SpaceId};
use rusqlite::{Connection, params};

use crate::docs::{atomically, id_from};
use crate::{Db, StorageError, Tx};

/// Queries of the `icon_refs` table, available on a connection or inside a transaction. On a [`Db`]
/// every method is atomic by itself.
pub trait IconRefQueries {
    /// Makes the references of each given page exactly the attachments given (its image icon and
    /// its image cover; none removes every row of the page). Pages not in the list are not touched.
    /// A page given twice keeps its last entry, an attachment given twice for a page is one row.
    /// One atomic unit.
    fn icon_refs_set(
        &self,
        space_id: SpaceId,
        pages: &[(PageId, Vec<AttachmentId>)],
    ) -> Result<(), StorageError>;
    /// Makes the rows of a space exactly `icons`, pairs of (page, attachment): the rows of the space
    /// that are not in the list go, the missing ones come, other spaces are not touched. A pair
    /// given twice is one row. One atomic unit.
    fn icon_refs_replace_space(
        &self,
        space_id: SpaceId,
        icons: &[(PageId, AttachmentId)],
    ) -> Result<(), StorageError>;
    /// Removes the rows of the given pages (purge). Unknown pages and an empty list succeed. One
    /// atomic unit.
    fn icon_refs_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError>;
    /// Removes every row (`IndexRebuild` of the whole workspace, which then refills the table).
    fn icon_refs_clear(&self) -> Result<(), StorageError>;
    /// The rows of a space as (page, attachment), ordered by page id, then attachment id.
    fn icon_refs_of_space(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<(PageId, AttachmentId)>, StorageError>;
    /// The attachments the icons and covers of the given pages reference now, each once, ordered by
    /// id (the attachments whose references may change when those pages' rows are rewritten or
    /// removed).
    fn icon_refs_attachments_of(&self, pages: &[PageId])
    -> Result<Vec<AttachmentId>, StorageError>;
}

impl IconRefQueries for Db {
    fn icon_refs_set(
        &self,
        space_id: SpaceId,
        pages: &[(PageId, Vec<AttachmentId>)],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| icon_refs_set_on(conn, space_id, pages))
    }
    fn icon_refs_replace_space(
        &self,
        space_id: SpaceId,
        icons: &[(PageId, AttachmentId)],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            icon_refs_replace_space_on(conn, space_id, icons)
        })
    }
    fn icon_refs_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| icon_refs_delete_pages_on(conn, pages))
    }
    fn icon_refs_clear(&self) -> Result<(), StorageError> {
        icon_refs_clear_on(&self.conn)
    }
    fn icon_refs_of_space(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<(PageId, AttachmentId)>, StorageError> {
        icon_refs_of_space_on(&self.conn, space_id)
    }
    fn icon_refs_attachments_of(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<AttachmentId>, StorageError> {
        icon_refs_attachments_of_on(&self.conn, pages)
    }
}

impl IconRefQueries for Tx<'_> {
    fn icon_refs_set(
        &self,
        space_id: SpaceId,
        pages: &[(PageId, Vec<AttachmentId>)],
    ) -> Result<(), StorageError> {
        icon_refs_set_on(&self.tx, space_id, pages)
    }
    fn icon_refs_replace_space(
        &self,
        space_id: SpaceId,
        icons: &[(PageId, AttachmentId)],
    ) -> Result<(), StorageError> {
        icon_refs_replace_space_on(&self.tx, space_id, icons)
    }
    fn icon_refs_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        icon_refs_delete_pages_on(&self.tx, pages)
    }
    fn icon_refs_clear(&self) -> Result<(), StorageError> {
        icon_refs_clear_on(&self.tx)
    }
    fn icon_refs_of_space(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<(PageId, AttachmentId)>, StorageError> {
        icon_refs_of_space_on(&self.tx, space_id)
    }
    fn icon_refs_attachments_of(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<AttachmentId>, StorageError> {
        icon_refs_attachments_of_on(&self.tx, pages)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// A pair given twice is one row (the space of a row can change when a page moves).
const UPSERT_SQL: &str = "INSERT INTO icon_refs (page_id, space_id, attachment_id) \
     VALUES (?1, ?2, ?3) \
     ON CONFLICT (page_id, attachment_id) DO UPDATE SET space_id = excluded.space_id";

/// Through the primary key (its first column).
const DELETE_PAGE_SQL: &str = "DELETE FROM icon_refs WHERE page_id = ?1";

/// Through `icon_refs_by_space`.
const DELETE_SPACE_SQL: &str = "DELETE FROM icon_refs WHERE space_id = ?1";

/// Through `icon_refs_by_space`.
const OF_SPACE_SQL: &str = "SELECT page_id, attachment_id FROM icon_refs WHERE space_id = ?1 \
     ORDER BY page_id, attachment_id";

/// Through the primary key (its first column).
const OF_PAGE_SQL: &str = "SELECT attachment_id FROM icon_refs WHERE page_id = ?1";

fn icon_refs_set_on(
    conn: &Connection,
    space_id: SpaceId,
    pages: &[(PageId, Vec<AttachmentId>)],
) -> Result<(), StorageError> {
    let mut delete = conn.prepare(DELETE_PAGE_SQL)?;
    let mut upsert = conn.prepare(UPSERT_SQL)?;
    for (page_id, attachments) in pages {
        delete.execute([page_id.to_bytes()])?;
        for attachment_id in attachments {
            upsert.execute(params![
                page_id.to_bytes(),
                space_id.to_bytes(),
                attachment_id.to_bytes()
            ])?;
        }
    }
    Ok(())
}

fn icon_refs_replace_space_on(
    conn: &Connection,
    space_id: SpaceId,
    icons: &[(PageId, AttachmentId)],
) -> Result<(), StorageError> {
    conn.execute(DELETE_SPACE_SQL, [space_id.to_bytes()])?;
    let mut upsert = conn.prepare(UPSERT_SQL)?;
    for (page_id, attachment_id) in icons {
        upsert.execute(params![
            page_id.to_bytes(),
            space_id.to_bytes(),
            attachment_id.to_bytes()
        ])?;
    }
    Ok(())
}

fn icon_refs_delete_pages_on(conn: &Connection, pages: &[PageId]) -> Result<(), StorageError> {
    let mut delete = conn.prepare(DELETE_PAGE_SQL)?;
    for page_id in pages {
        delete.execute([page_id.to_bytes()])?;
    }
    Ok(())
}

fn icon_refs_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM icon_refs", [])?;
    Ok(())
}

fn icon_refs_of_space_on(
    conn: &Connection,
    space_id: SpaceId,
) -> Result<Vec<(PageId, AttachmentId)>, StorageError> {
    let mut stmt = conn.prepare(OF_SPACE_SQL)?;
    let rows = stmt
        .query_map([space_id.to_bytes()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.iter()
        .map(|(page, attachment)| {
            Ok((
                PageId::from_bytes(id_from(page, "icon page id")?),
                AttachmentId::from_bytes(id_from(attachment, "icon attachment id")?),
            ))
        })
        .collect()
}

fn icon_refs_attachments_of_on(
    conn: &Connection,
    pages: &[PageId],
) -> Result<Vec<AttachmentId>, StorageError> {
    let mut stmt = conn.prepare(OF_PAGE_SQL)?;
    let mut ids = Vec::new();
    for page_id in pages {
        let rows = stmt
            .query_map([page_id.to_bytes()], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for bytes in &rows {
            ids.push(AttachmentId::from_bytes(id_from(
                bytes,
                "icon attachment id",
            )?));
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{AttachmentQueries, AttachmentRow};

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
    /// `IconRefQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn IconRefQueries, &Connection)) {
        let db = new_db();
        scenario(&db, db.connection());
        let mut db = new_db();
        db.transaction(|tx| {
            scenario(tx, &tx.tx);
            Ok(())
        })
        .unwrap();
    }

    fn rows(q: &dyn IconRefQueries, space_n: u8) -> Vec<(u8, u8)> {
        q.icon_refs_of_space(space(space_n))
            .unwrap()
            .into_iter()
            .map(|(p, a)| (p.to_bytes()[0], a.to_bytes()[0]))
            .collect()
    }

    #[test]
    fn set_writes_replaces_and_removes_the_rows_of_the_given_pages_only() {
        on_db_and_tx(|q, _| {
            q.icon_refs_set(
                space(1),
                &[
                    (page(3), vec![attachment(30)]),
                    (page(1), vec![attachment(10)]),
                    (page(2), vec![]),
                ],
            )
            .unwrap();
            assert_eq!(rows(q, 1), [(1, 10), (3, 30)], "ordered by page id");

            q.icon_refs_set(
                space(1),
                &[
                    (page(1), vec![attachment(11)]),
                    (page(3), vec![]),
                    (page(4), vec![attachment(40)]),
                ],
            )
            .unwrap();
            assert_eq!(rows(q, 1), [(1, 11), (4, 40)]);

            // A page given twice keeps its last entry; an empty list changes nothing.
            q.icon_refs_set(
                space(1),
                &[
                    (page(5), vec![attachment(1)]),
                    (page(5), vec![attachment(2)]),
                ],
            )
            .unwrap();
            q.icon_refs_set(space(1), &[]).unwrap();
            assert_eq!(rows(q, 1), [(1, 11), (4, 40), (5, 2)]);
        });
    }

    #[test]
    fn replace_space_makes_the_rows_of_the_space_exactly_the_given_set() {
        on_db_and_tx(|q, _| {
            q.icon_refs_replace_space(
                space(1),
                &[(page(1), attachment(10)), (page(2), attachment(20))],
            )
            .unwrap();
            q.icon_refs_replace_space(space(2), &[(page(9), attachment(90))])
                .unwrap();

            q.icon_refs_replace_space(
                space(1),
                &[(page(2), attachment(21)), (page(3), attachment(30))],
            )
            .unwrap();
            assert_eq!(rows(q, 1), [(2, 21), (3, 30)]);
            assert_eq!(rows(q, 2), [(9, 90)], "another space is not touched");

            q.icon_refs_replace_space(space(1), &[]).unwrap();
            assert!(rows(q, 1).is_empty());
            assert_eq!(rows(q, 2), [(9, 90)]);
        });
    }

    #[test]
    fn delete_pages_and_clear_remove_rows() {
        on_db_and_tx(|q, _| {
            q.icon_refs_replace_space(
                space(1),
                &[
                    (page(1), attachment(10)),
                    (page(2), attachment(20)),
                    (page(3), attachment(30)),
                ],
            )
            .unwrap();
            q.icon_refs_delete_pages(&[page(2), page(9), page(2)])
                .unwrap();
            q.icon_refs_delete_pages(&[]).unwrap();
            assert_eq!(rows(q, 1), [(1, 10), (3, 30)]);
            q.icon_refs_clear().unwrap();
            assert!(rows(q, 1).is_empty());
            q.icon_refs_clear().unwrap();
        });
    }

    #[test]
    fn the_attachments_of_pages_are_listed_once_each_ordered_by_id() {
        on_db_and_tx(|q, _| {
            q.icon_refs_replace_space(
                space(1),
                &[
                    (page(1), attachment(7)),
                    (page(2), attachment(3)),
                    (page(3), attachment(7)),
                ],
            )
            .unwrap();
            assert_eq!(
                q.icon_refs_attachments_of(&[page(1), page(2), page(3), page(9)])
                    .unwrap(),
                [attachment(3), attachment(7)]
            );
            assert_eq!(
                q.icon_refs_attachments_of(&[page(3)]).unwrap(),
                [attachment(7)]
            );
            assert!(q.icon_refs_attachments_of(&[]).unwrap().is_empty());
            assert!(q.icon_refs_attachments_of(&[page(9)]).unwrap().is_empty());
        });
    }

    #[test]
    fn ids_are_stored_as_16_byte_blobs() {
        on_db_and_tx(|q, conn| {
            q.icon_refs_set(space(2), &[(page(1), vec![attachment(3)])])
                .unwrap();
            let widths: (String, String, String) = conn
                .query_row(
                    "SELECT typeof(page_id) || length(page_id), \
                            typeof(space_id) || length(space_id), \
                            typeof(attachment_id) || length(attachment_id) FROM icon_refs",
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
    fn a_damaged_row_is_invalid_data_and_never_a_panic() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO icon_refs VALUES (zeroblob(15), x'01010101010101010101010101010101', zeroblob(16))",
                [],
            )
            .unwrap();
        let err = db.icon_refs_of_space(space(1)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        db.connection()
            .execute(
                "INSERT INTO icon_refs VALUES (x'02020202020202020202020202020202', zeroblob(16), zeroblob(17))",
                [],
            )
            .unwrap();
        let err = db.icon_refs_attachments_of(&[page(2)]).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    #[test]
    fn a_bare_db_replaces_a_space_all_or_nothing() {
        let db = new_db();
        db.icon_refs_replace_space(space(1), &[(page(1), attachment(10))])
            .unwrap();
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE INSERT ON icon_refs
                 WHEN NEW.page_id = x'09090909090909090909090909090909'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db
            .icon_refs_replace_space(
                space(1),
                &[
                    (page(3), attachment(30)),
                    (page(9), attachment(90)),
                    (page(5), attachment(50)),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            rows(&db, 1),
            [(1, 10)],
            "the old row is back, no new one is there"
        );
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn the_statements_use_the_indexes_the_migration_creates() {
        let db = new_db();
        let plan = |sql: &str| {
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
        };
        for sql in [DELETE_PAGE_SQL, OF_PAGE_SQL] {
            let p = plan(sql);
            assert!(
                p.contains("sqlite_autoindex_icon_refs_1 (page_id=?)"),
                "by primary key: {p} for {sql}"
            );
        }
        for sql in [DELETE_SPACE_SQL, OF_SPACE_SQL] {
            let p = plan(sql);
            assert!(
                p.contains("icon_refs_by_space (space_id=?)"),
                "a space through its index: {p} for {sql}"
            );
        }
        let count = plan("SELECT count(*) FROM icon_refs WHERE attachment_id = ?1");
        assert!(
            count.contains("icon_refs_by_attachment (attachment_id=?)"),
            "{count}"
        );
    }

    #[test]
    fn the_reference_count_and_the_attachments_of_a_page_include_the_rows() {
        // The reference count and the page's attachment list read this table (the other sources
        // are exercised in the `attachments` module).
        let db = new_db();
        db.attachment_put(&AttachmentRow {
            attachment_id: attachment(1),
            space_id: space(1),
            name: "icon.png".to_owned(),
            mime: "image/png".to_owned(),
            size: 10,
            chunk_count: 1,
            created_at: 5,
            unreferenced_since: None,
        })
        .unwrap();
        assert_eq!(db.attachment_reference_count(attachment(1)).unwrap(), 0);
        db.icon_refs_set(
            space(1),
            &[
                (page(1), vec![attachment(1)]),
                (page(2), vec![attachment(1)]),
            ],
        )
        .unwrap();
        assert_eq!(db.attachment_reference_count(attachment(1)).unwrap(), 2);
        assert_eq!(
            db.attachment_ids_referenced_by(page(1)).unwrap(),
            [attachment(1)]
        );
        db.icon_refs_set(space(1), &[(page(1), vec![])]).unwrap();
        assert_eq!(db.attachment_reference_count(attachment(1)).unwrap(), 1);
        assert!(db.attachment_ids_referenced_by(page(1)).unwrap().is_empty());
    }

    #[test]
    fn a_page_holds_an_icon_and_a_cover_as_two_rows_and_the_same_image_as_one() {
        on_db_and_tx(|q, _| {
            // The icon and the cover are two images: two rows of the page.
            q.icon_refs_set(
                space(1),
                &[
                    (page(1), vec![attachment(10), attachment(11)]),
                    (page(2), vec![attachment(20), attachment(20)]),
                ],
            )
            .unwrap();
            assert_eq!(
                rows(q, 1),
                [(1, 10), (1, 11), (2, 20)],
                "ordered by page, then attachment; the same image twice is one row"
            );

            // A write of a page replaces all its rows: the cover goes, the icon stays.
            q.icon_refs_set(space(1), &[(page(1), vec![attachment(10)])])
                .unwrap();
            assert_eq!(rows(q, 1), [(1, 10), (2, 20)]);
            // Both change at once.
            q.icon_refs_set(space(1), &[(page(1), vec![attachment(12), attachment(13)])])
                .unwrap();
            assert_eq!(rows(q, 1), [(1, 12), (1, 13), (2, 20)]);

            // A space is rewritten from pairs, a page may appear twice, a pair twice is one row.
            q.icon_refs_replace_space(
                space(1),
                &[
                    (page(4), attachment(41)),
                    (page(3), attachment(31)),
                    (page(3), attachment(30)),
                    (page(3), attachment(31)),
                ],
            )
            .unwrap();
            assert_eq!(rows(q, 1), [(3, 30), (3, 31), (4, 41)]);

            // Purging a page removes both of its rows; the attachments list each once.
            assert_eq!(
                q.icon_refs_attachments_of(&[page(3), page(4)]).unwrap(),
                [attachment(30), attachment(31), attachment(41)]
            );
            q.icon_refs_delete_pages(&[page(3)]).unwrap();
            assert_eq!(rows(q, 1), [(4, 41)]);
        });
    }

    #[test]
    fn a_page_that_moves_to_another_space_takes_its_rows_along() {
        on_db_and_tx(|q, _| {
            q.icon_refs_set(space(1), &[(page(1), vec![attachment(10), attachment(11)])])
                .unwrap();
            // The target space writes the same pairs under its own id (the page keeps its id).
            q.icon_refs_set(space(2), &[(page(1), vec![attachment(10), attachment(11)])])
                .unwrap();
            assert!(rows(q, 1).is_empty());
            assert_eq!(rows(q, 2), [(1, 10), (1, 11)]);
            // The pair is the key: writing a pair that exists under another space moves the row.
            q.icon_refs_replace_space(space(1), &[(page(1), attachment(10))])
                .unwrap();
            assert_eq!(rows(q, 1), [(1, 10)]);
            assert_eq!(rows(q, 2), [(1, 11)]);
        });
    }

    #[test]
    fn the_same_image_as_icon_and_as_cover_is_one_reference_of_the_page() {
        let db = new_db();
        db.attachment_put(&AttachmentRow {
            attachment_id: attachment(1),
            space_id: space(1),
            name: "banner.png".to_owned(),
            mime: "image/png".to_owned(),
            size: 10,
            chunk_count: 1,
            created_at: 5,
            unreferenced_since: None,
        })
        .unwrap();
        db.icon_refs_set(space(1), &[(page(1), vec![attachment(1), attachment(1)])])
            .unwrap();
        assert_eq!(db.attachment_reference_count(attachment(1)).unwrap(), 1);
        db.icon_refs_set(
            space(1),
            &[
                (page(1), vec![attachment(1)]),
                (page(2), vec![attachment(1), attachment(2)]),
            ],
        )
        .unwrap();
        assert_eq!(
            db.attachment_reference_count(attachment(1)).unwrap(),
            2,
            "one reference per page"
        );
        assert_eq!(
            db.attachment_ids_referenced_by(page(2)).unwrap(),
            [attachment(1), attachment(2)],
            "the icon and the cover of a page are listed with its blocks"
        );
        assert_eq!(
            db.attachment_referencing_pages(attachment(1)).unwrap(),
            [page(1), page(2)]
        );
    }
}
