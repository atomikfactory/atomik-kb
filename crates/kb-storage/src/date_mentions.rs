//! The date mentions of the pages (schema v9, ADR-0024). Derived data: the mentions live in the
//! page documents, so the core rewrites the rows of a page in the same transaction as its
//! update row on every save (exactly like [`crate::TextIndexQueries::links_replace_from`]),
//! `IndexRebuild` recreates the table from scratch, and it is neither synced nor part of a
//! backup.
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`] through
//! [`DateMentionQueries`], both running the same private functions over a connection (a
//! transaction derefs to one), so a method behaves identically in either place; the
//! multi-statement methods of a [`Db`] run as one atomic unit. Ids are bound as 16-byte blobs.
//! Rows are read as raw column values first and validated afterwards, so a damaged row is a
//! typed `InvalidData` error and never a panic or a silently wrong value. Dates and times
//! (which are the user's content, if only a little of it) never appear in an error.
//!
//! # The table
//!
//! One row per `dateMention` node with a valid date: the page, the block that holds it, the
//! node's own id, its `date` (`YYYY-MM-DD`) and `time` (`HH:mm`, or NULL), and for a mention
//! with a reminder `reminder_minutes` (minutes before the moment) and `fire_at_local`, the
//! fire time in seconds since 1970-01-01T00:00:00 as the device's local wall clock reads it
//! (the calendar fields counted as if they were UTC: a mention is a floating local date, so
//! no time zone data is stored or needed). A mention without a reminder has both NULL, which a
//! `CHECK` constraint enforces. This crate stores what it is given; the core computes the
//! fire time (`kb_model::PageDoc::date_mentions`).
//!
//! `doc_id` is the page's document id (the same bytes as the page id, ADR-0016); this crate
//! writes it, callers only give the page.

use kb_types::{AccountId, BlockId, DocId, PageId, SpaceId};
use rusqlite::{Connection, params};

use crate::docs::{atomically, id_from};
use crate::{Db, StorageError, Tx};

/// A `date_mentions` row without its page (the page is the argument of the methods).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateMentionRow {
    /// The block that holds the mention.
    pub block_id: BlockId,
    /// The mention itself (the `id` of the `dateMention` node). Unique within a page: when
    /// two rows of one replacement share it, the first wins.
    pub node_id: BlockId,
    /// The day, `YYYY-MM-DD`.
    pub date: String,
    /// The time of day, `HH:mm`.
    pub time: Option<String>,
    /// Minutes before the moment that the reminder fires; `None` for no reminder.
    pub reminder_minutes: Option<u32>,
    /// When it fires, in seconds since 1970-01-01T00:00:00 on the local wall clock; `Some`
    /// exactly when `reminder_minutes` is.
    pub fire_at_local: Option<i64>,
    /// `reminderFor` (ADR-0047, schema v18): the account whose devices fire the reminder in a
    /// shared space; `None` when the node has no such attribute.
    pub reminder_for: Option<AccountId>,
}

/// A reminder that falls in a window ([`DateMentionQueries::date_mentions_due`]): a row with
/// a reminder, with the page and its space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueReminderRow {
    /// The page that holds the mention.
    pub page_id: PageId,
    /// The page's space (from `pages_index`).
    pub space_id: SpaceId,
    /// The block that holds the mention.
    pub block_id: BlockId,
    /// The mention.
    pub node_id: BlockId,
    /// The day, `YYYY-MM-DD`.
    pub date: String,
    /// The time of day, `HH:mm`.
    pub time: Option<String>,
    /// Minutes before the moment.
    pub reminder_minutes: u32,
    /// The fire time, in seconds on the local wall clock.
    pub fire_at_local: i64,
    /// `reminderFor`: the account whose devices fire it in a shared space (`None`: the Owners').
    pub reminder_for: Option<AccountId>,
}

/// Queries of the `date_mentions` table, available on a connection or inside a transaction.
/// On a [`Db`] every method is atomic by itself.
pub trait DateMentionQueries {
    /// Replaces the rows of a page with `rows`, in the order given (a page's rows are read
    /// back in that order). Two rows with the same `node_id` keep the first. The page does
    /// not have to be in `pages_index`.
    fn date_mentions_replace(
        &self,
        page_id: PageId,
        rows: &[DateMentionRow],
    ) -> Result<(), StorageError>;

    /// The rows of a page in the order they were written (document order); empty for a page
    /// without mentions.
    fn date_mentions_of(&self, page_id: PageId) -> Result<Vec<DateMentionRow>, StorageError>;

    /// Removes the rows of a page (purge). Succeeds when it has none.
    fn date_mentions_delete_page(&self, page_id: PageId) -> Result<(), StorageError>;

    /// Removes every row (before a full rebuild).
    fn date_mentions_clear(&self) -> Result<(), StorageError>;

    /// The reminders whose `fire_at_local` is after `after` and not after `until`, of live
    /// pages only (not trashed, not in a template, and present in `pages_index`), oldest first, then by page and
    /// mention; at most `limit` (0 gives none). A mention without a reminder is never due.
    fn date_mentions_due(
        &self,
        after: i64,
        until: i64,
        limit: u32,
    ) -> Result<Vec<DueReminderRow>, StorageError>;
}

impl DateMentionQueries for Db {
    fn date_mentions_replace(
        &self,
        page_id: PageId,
        rows: &[DateMentionRow],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            date_mentions_replace_on(conn, page_id, rows)
        })
    }
    fn date_mentions_of(&self, page_id: PageId) -> Result<Vec<DateMentionRow>, StorageError> {
        date_mentions_of_on(&self.conn, page_id)
    }
    fn date_mentions_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        date_mentions_delete_page_on(&self.conn, page_id)
    }
    fn date_mentions_clear(&self) -> Result<(), StorageError> {
        date_mentions_clear_on(&self.conn)
    }
    fn date_mentions_due(
        &self,
        after: i64,
        until: i64,
        limit: u32,
    ) -> Result<Vec<DueReminderRow>, StorageError> {
        date_mentions_due_on(&self.conn, after, until, limit)
    }
}

impl DateMentionQueries for Tx<'_> {
    fn date_mentions_replace(
        &self,
        page_id: PageId,
        rows: &[DateMentionRow],
    ) -> Result<(), StorageError> {
        date_mentions_replace_on(&self.tx, page_id, rows)
    }
    fn date_mentions_of(&self, page_id: PageId) -> Result<Vec<DateMentionRow>, StorageError> {
        date_mentions_of_on(&self.tx, page_id)
    }
    fn date_mentions_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        date_mentions_delete_page_on(&self.tx, page_id)
    }
    fn date_mentions_clear(&self) -> Result<(), StorageError> {
        date_mentions_clear_on(&self.tx)
    }
    fn date_mentions_due(
        &self,
        after: i64,
        until: i64,
        limit: u32,
    ) -> Result<Vec<DueReminderRow>, StorageError> {
        date_mentions_due_on(&self.tx, after, until, limit)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// The rows of a page (the primary key `(page_id, node_id)` starts with the page).
const DELETE_PAGE_SQL: &str = "DELETE FROM date_mentions WHERE page_id = ?1";

/// A page's rows in the order they were written: `rowid` grows with every insert.
const OF_PAGE_SQL: &str = "SELECT block_id, node_id, date, time, reminder_minutes, fire_at_local, \
            reminder_for \
     FROM date_mentions WHERE page_id = ?1 ORDER BY rowid";

/// The window of fire times. `date_mentions` drives, through the partial index
/// `date_mentions_fire` (the range implies `fire_at_local IS NOT NULL`); each hit looks its
/// page up by primary key in `pages_index` (`CROSS JOIN` pins the order).
const DUE_SQL: &str = "SELECT d.page_id, p.space_id, d.block_id, d.node_id, d.date, d.time, \
            d.reminder_minutes, d.fire_at_local, d.reminder_for \
       FROM date_mentions AS d \
      CROSS JOIN pages_index AS p ON p.page_id = d.page_id \
      WHERE d.fire_at_local > ?1 AND d.fire_at_local <= ?2 AND p.trashed_at IS NULL \
        AND p.in_template = 0 \
      ORDER BY d.fire_at_local, d.page_id, d.node_id \
      LIMIT ?3";

fn date_mentions_replace_on(
    conn: &Connection,
    page_id: PageId,
    rows: &[DateMentionRow],
) -> Result<(), StorageError> {
    conn.execute(DELETE_PAGE_SQL, [page_id.to_bytes()])?;
    if rows.is_empty() {
        return Ok(());
    }
    // A node id twice in one page (a damaged document) has one row: the first wins.
    let mut insert = conn.prepare(
        "INSERT INTO date_mentions \
           (doc_id, page_id, block_id, node_id, date, time, reminder_minutes, fire_at_local, \
            reminder_for) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT (page_id, node_id) DO NOTHING",
    )?;
    for row in rows {
        insert.execute(params![
            DocId::for_page(page_id).to_bytes(),
            page_id.to_bytes(),
            row.block_id.to_bytes(),
            row.node_id.to_bytes(),
            row.date,
            row.time,
            row.reminder_minutes.map(i64::from),
            row.fire_at_local,
            row.reminder_for.map(AccountId::to_bytes),
        ])?;
    }
    Ok(())
}

fn date_mentions_of_on(
    conn: &Connection,
    page_id: PageId,
) -> Result<Vec<DateMentionRow>, StorageError> {
    let mut stmt = conn.prepare(OF_PAGE_SQL)?;
    let rows = stmt
        .query_map([page_id.to_bytes()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<Vec<u8>>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(block, node, date, time, minutes, fire, reminder_for)| {
            let reminder_minutes = minutes.map(minutes_from).transpose()?;
            if reminder_minutes.is_some() != fire.is_some() {
                return Err(StorageError::InvalidData(
                    "a date mention has a reminder without a fire time, or the reverse".to_owned(),
                ));
            }
            Ok(DateMentionRow {
                block_id: BlockId::from_bytes(id_from(&block, "date mention block id")?),
                node_id: BlockId::from_bytes(id_from(&node, "date mention node id")?),
                date,
                time,
                reminder_minutes,
                fire_at_local: fire,
                reminder_for: reminder_for_from(reminder_for.as_deref())?,
            })
        })
        .collect()
}

/// A `reminder_for` column read as the account it names.
fn reminder_for_from(bytes: Option<&[u8]>) -> Result<Option<AccountId>, StorageError> {
    bytes
        .map(|bytes| id_from(bytes, "date mention reminder account id").map(AccountId::from_bytes))
        .transpose()
}

fn date_mentions_delete_page_on(conn: &Connection, page_id: PageId) -> Result<(), StorageError> {
    conn.execute(DELETE_PAGE_SQL, [page_id.to_bytes()])?;
    Ok(())
}

fn date_mentions_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM date_mentions", [])?;
    Ok(())
}

fn date_mentions_due_on(
    conn: &Connection,
    after: i64,
    until: i64,
    limit: u32,
) -> Result<Vec<DueReminderRow>, StorageError> {
    let mut stmt = conn.prepare(DUE_SQL)?;
    let rows = stmt
        .query_map(params![after, until, i64::from(limit)], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<Vec<u8>>>(8)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(page, space, block, node, date, time, minutes, fire, reminder_for)| {
                Ok(DueReminderRow {
                    page_id: PageId::from_bytes(id_from(&page, "date mention page id")?),
                    space_id: SpaceId::from_bytes(id_from(&space, "date mention space id")?),
                    block_id: BlockId::from_bytes(id_from(&block, "date mention block id")?),
                    node_id: BlockId::from_bytes(id_from(&node, "date mention node id")?),
                    date,
                    time,
                    reminder_minutes: minutes_from(minutes)?,
                    fire_at_local: fire,
                    reminder_for: reminder_for_from(reminder_for.as_deref())?,
                })
            },
        )
        .collect()
}

/// A reminder's minutes read from SQLite as the `u32` of the API.
fn minutes_from(value: i64) -> Result<u32, StorageError> {
    u32::try_from(value)
        .map_err(|_| StorageError::InvalidData("a reminder is out of range".to_owned()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{DocQueries, PageIndexRow, TextIndexQueries};

    /// What a scenario needs of a connection or a transaction: both implement all three.
    trait Queries: DocQueries + DateMentionQueries + TextIndexQueries {}
    impl<T: DocQueries + DateMentionQueries + TextIndexQueries> Queries for T {}

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that
    /// both `DateMentionQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
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

    fn add_page(q: &dyn Queries, n: u8, space_id: SpaceId, trashed_at: Option<i64>) {
        q.pages_index_upsert(&[PageIndexRow {
            page_id: page(n),
            space_id,
            parent_id: None,
            title: "page".to_owned(),
            trashed_at,
            updated_at: 100,
            in_template: false,
        }])
        .unwrap();
    }

    /// A mention without a reminder.
    fn plain(block_n: u8, node_n: u8, date: &str) -> DateMentionRow {
        DateMentionRow {
            block_id: block(block_n),
            node_id: block(node_n),
            date: date.to_owned(),
            time: None,
            reminder_minutes: None,
            fire_at_local: None,
            reminder_for: None,
        }
    }

    /// A mention with a reminder that fires at `fire`.
    fn reminder(block_n: u8, node_n: u8, minutes: u32, fire: i64) -> DateMentionRow {
        DateMentionRow {
            block_id: block(block_n),
            node_id: block(node_n),
            date: "2026-10-07".to_owned(),
            time: Some("14:30".to_owned()),
            reminder_minutes: Some(minutes),
            fire_at_local: Some(fire),
            reminder_for: None,
        }
    }

    fn nodes(rows: &[DueReminderRow]) -> Vec<(u8, u8)> {
        rows.iter()
            .map(|row| (row.page_id.to_bytes()[0], row.node_id.to_bytes()[0]))
            .collect()
    }

    fn count(db: &Db, from: &str) -> i64 {
        db.connection()
            .query_row(&format!("SELECT count(*) FROM {from}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn rows_round_trip_in_the_order_they_were_written() {
        on_db_and_tx(|q| {
            let rows = vec![
                reminder(2, 9, 15, 1_000),
                plain(2, 3, "2026-10-08"),
                reminder(1, 5, 0, 500),
            ];
            q.date_mentions_replace(page(1), &rows).unwrap();
            assert_eq!(q.date_mentions_of(page(1)).unwrap(), rows);
            assert!(q.date_mentions_of(page(2)).unwrap().is_empty());
            q.date_mentions_replace(page(1), &[]).unwrap();
            assert!(q.date_mentions_of(page(1)).unwrap().is_empty());
        });
    }

    #[test]
    fn ids_are_stored_as_16_byte_blobs_and_the_document_is_the_page() {
        let db = new_db();
        db.date_mentions_replace(page(7), &[reminder(8, 9, 15, 1_000)])
            .unwrap();
        let (doc, page_id, block_id, node_id): (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) = db
            .connection()
            .query_row(
                "SELECT doc_id, page_id, block_id, node_id FROM date_mentions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(doc, vec![7; 16]);
        assert_eq!(page_id, vec![7; 16]);
        assert_eq!(block_id, vec![8; 16]);
        assert_eq!(node_id, vec![9; 16]);
    }

    #[test]
    fn replacing_the_rows_of_a_page_leaves_the_other_pages_alone() {
        on_db_and_tx(|q| {
            q.date_mentions_replace(
                page(1),
                &[plain(1, 1, "2026-10-07"), plain(1, 2, "2026-10-08")],
            )
            .unwrap();
            q.date_mentions_replace(page(2), &[plain(1, 1, "2026-11-01")])
                .unwrap();
            q.date_mentions_replace(page(1), &[plain(3, 3, "2027-01-01")])
                .unwrap();
            assert_eq!(
                q.date_mentions_of(page(1)).unwrap(),
                [plain(3, 3, "2027-01-01")]
            );
            assert_eq!(
                q.date_mentions_of(page(2)).unwrap(),
                [plain(1, 1, "2026-11-01")],
                "the same node id on another page is another row"
            );
        });
    }

    #[test]
    fn a_node_id_twice_in_a_page_keeps_the_first_row() {
        on_db_and_tx(|q| {
            q.date_mentions_replace(
                page(1),
                &[plain(1, 4, "2026-10-07"), plain(2, 4, "2026-10-09")],
            )
            .unwrap();
            assert_eq!(
                q.date_mentions_of(page(1)).unwrap(),
                [plain(1, 4, "2026-10-07")]
            );
        });
    }

    #[test]
    fn delete_page_and_clear_remove_rows() {
        on_db_and_tx(|q| {
            q.date_mentions_replace(page(1), &[plain(1, 1, "2026-10-07")])
                .unwrap();
            q.date_mentions_replace(page(2), &[plain(1, 1, "2026-10-07")])
                .unwrap();
            q.date_mentions_delete_page(page(1)).unwrap();
            q.date_mentions_delete_page(page(9)).unwrap();
            assert!(q.date_mentions_of(page(1)).unwrap().is_empty());
            assert_eq!(q.date_mentions_of(page(2)).unwrap().len(), 1);
            q.date_mentions_clear().unwrap();
            assert!(q.date_mentions_of(page(2)).unwrap().is_empty());
            q.date_mentions_replace(page(2), &[plain(1, 1, "2026-10-07")])
                .unwrap();
            assert_eq!(q.date_mentions_of(page(2)).unwrap().len(), 1);
        });
    }

    #[test]
    fn a_reminder_without_a_fire_time_or_the_reverse_is_refused_by_the_table() {
        let db = new_db();
        let mut half = reminder(1, 1, 15, 1_000);
        half.fire_at_local = None;
        assert!(db.date_mentions_replace(page(1), &[half]).is_err());
        let mut other_half = reminder(1, 1, 15, 1_000);
        other_half.reminder_minutes = None;
        assert!(db.date_mentions_replace(page(1), &[other_half]).is_err());
        assert_eq!(
            count(&db, "date_mentions"),
            0,
            "a failed replacement leaves nothing"
        );
    }

    #[test]
    fn a_failed_replacement_keeps_the_old_rows() {
        let db = new_db();
        db.date_mentions_replace(page(1), &[plain(1, 1, "2026-10-07")])
            .unwrap();
        let mut bad = reminder(1, 2, 15, 1_000);
        bad.fire_at_local = None;
        assert!(
            db.date_mentions_replace(page(1), &[plain(1, 3, "2026-10-09"), bad])
                .is_err()
        );
        assert_eq!(
            db.date_mentions_of(page(1)).unwrap(),
            [plain(1, 1, "2026-10-07")]
        );
    }

    #[test]
    fn due_is_a_window_open_below_and_closed_above() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1), None);
            q.date_mentions_replace(
                page(1),
                &[
                    reminder(1, 1, 5, 100),
                    reminder(1, 2, 5, 200),
                    reminder(1, 3, 5, 300),
                    plain(1, 4, "2026-10-07"),
                ],
            )
            .unwrap();
            assert_eq!(
                nodes(&q.date_mentions_due(100, 300, 10).unwrap()),
                [(1, 2), (1, 3)]
            );
            assert_eq!(nodes(&q.date_mentions_due(99, 100, 10).unwrap()), [(1, 1)]);
            assert_eq!(
                nodes(&q.date_mentions_due(0, 1_000, 10).unwrap()),
                [(1, 1), (1, 2), (1, 3)]
            );
            assert!(q.date_mentions_due(300, 1_000, 10).unwrap().is_empty());
            assert!(q.date_mentions_due(200, 200, 10).unwrap().is_empty());
            assert!(
                q.date_mentions_due(500, 100, 10).unwrap().is_empty(),
                "an inverted window"
            );
            assert_eq!(
                nodes(&q.date_mentions_due(i64::MIN, i64::MAX, 10).unwrap()),
                [(1, 1), (1, 2), (1, 3)],
                "mentions without a reminder are never due"
            );
        });
    }

    #[test]
    fn due_rows_carry_the_page_the_space_and_the_mention() {
        on_db_and_tx(|q| {
            add_page(q, 4, space(8), None);
            q.date_mentions_replace(page(4), &[reminder(5, 6, 60, 1_000)])
                .unwrap();
            assert_eq!(
                q.date_mentions_due(0, 2_000, 10).unwrap(),
                [DueReminderRow {
                    page_id: page(4),
                    space_id: space(8),
                    block_id: block(5),
                    node_id: block(6),
                    date: "2026-10-07".to_owned(),
                    time: Some("14:30".to_owned()),
                    reminder_minutes: 60,
                    fire_at_local: 1_000,
                    reminder_for: None,
                }]
            );
        });
    }

    #[test]
    fn due_is_ordered_by_fire_time_then_page_then_mention_and_limited() {
        on_db_and_tx(|q| {
            for n in 1..=3 {
                add_page(q, n, space(1), None);
            }
            q.date_mentions_replace(page(2), &[reminder(1, 7, 0, 50), reminder(1, 3, 0, 10)])
                .unwrap();
            q.date_mentions_replace(page(1), &[reminder(1, 9, 0, 50), reminder(1, 1, 0, 70)])
                .unwrap();
            q.date_mentions_replace(page(3), &[reminder(1, 2, 0, 5)])
                .unwrap();
            assert_eq!(
                nodes(&q.date_mentions_due(0, 100, 10).unwrap()),
                [(3, 2), (2, 3), (1, 9), (2, 7), (1, 1)]
            );
            assert_eq!(
                nodes(&q.date_mentions_due(0, 100, 2).unwrap()),
                [(3, 2), (2, 3)]
            );
            assert!(q.date_mentions_due(0, 100, 0).unwrap().is_empty());
        });
    }

    #[test]
    fn trashed_pages_and_pages_missing_from_the_index_are_not_due() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1), None);
            add_page(q, 2, space(1), Some(9));
            for n in 1..=3 {
                q.date_mentions_replace(page(n), &[reminder(1, n, 0, 10)])
                    .unwrap();
            }
            assert_eq!(nodes(&q.date_mentions_due(0, 100, 10).unwrap()), [(1, 1)]);
            // Restoring the page brings its reminder back.
            add_page(q, 2, space(1), None);
            assert_eq!(
                nodes(&q.date_mentions_due(0, 100, 10).unwrap()),
                [(1, 1), (2, 2)]
            );
        });
    }

    #[test]
    fn a_reminder_of_any_size_up_to_a_u32_is_stored() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1), None);
            q.date_mentions_replace(page(1), &[reminder(1, 1, u32::MAX, -5_000_000_000_000)])
                .unwrap();
            let due = q.date_mentions_due(i64::MIN, i64::MAX, 10).unwrap();
            assert_eq!(due[0].reminder_minutes, u32::MAX);
            assert_eq!(due[0].fire_at_local, -5_000_000_000_000);
        });
    }

    #[test]
    fn the_reminder_account_is_stored_and_comes_back_in_both_reads() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1), None);
            let who = AccountId::from_bytes([0xA1; 16]);
            let rows = [
                DateMentionRow {
                    reminder_for: Some(who),
                    ..reminder(1, 1, 15, 1_000)
                },
                reminder(2, 2, 0, 2_000),
            ];
            q.date_mentions_replace(page(1), &rows).unwrap();
            assert_eq!(q.date_mentions_of(page(1)).unwrap(), rows);
            let due = q.date_mentions_due(0, 10_000, 10).unwrap();
            assert_eq!(
                due.iter().map(|row| row.reminder_for).collect::<Vec<_>>(),
                [Some(who), None]
            );
        });
    }

    #[test]
    fn damaged_rows_are_invalid_data_and_never_a_panic() {
        let db = new_db();
        add_page(&db, 1, space(1), None);
        db.connection()
            .execute_batch(
                "INSERT INTO date_mentions VALUES (zeroblob(16), zeroblob(16), x'01', zeroblob(16), '2026-10-07', NULL, NULL, NULL, NULL);",
            )
            .unwrap();
        let err = db.date_mentions_of(page(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err:?}");

        db.connection()
            .execute_batch("DELETE FROM date_mentions;")
            .unwrap();
        db.connection()
            .execute_batch(
                "INSERT INTO date_mentions VALUES (x'01010101010101010101010101010101', x'01010101010101010101010101010101', zeroblob(16), zeroblob(16), '2026-10-07', NULL, -1, 5, NULL);",
            )
            .unwrap();
        let err = db.date_mentions_of(page(1)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err:?}");
        let err = db.date_mentions_due(0, 10, 10).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err:?}");
        assert!(!err.to_string().contains("2026"), "{err}");

        // A reminder account that is not an account id is damage too.
        db.connection()
            .execute_batch(
                "DELETE FROM date_mentions; \
                 INSERT INTO date_mentions VALUES (x'01010101010101010101010101010101', x'01010101010101010101010101010101', zeroblob(16), zeroblob(16), '2026-10-07', NULL, NULL, NULL, x'0102');",
            )
            .unwrap();
        let err = db.date_mentions_of(page(1)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err:?}");
    }

    fn plan_of(db: &Db, sql: &str, args: &[i64]) -> String {
        let mut stmt = db
            .connection()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.join(" | ")
    }

    #[test]
    fn the_statements_are_driven_by_their_indexes() {
        let db = new_db();
        let due = plan_of(&db, DUE_SQL, &[0, 10, 10]);
        assert!(
            due.contains("USING INDEX date_mentions_fire"),
            "the window of fire times is read through the partial index: {due}"
        );
        assert!(
            due.contains("SEARCH p USING INDEX sqlite_autoindex_pages_index_1 (page_id=?)"),
            "each hit finds its page by primary key: {due}"
        );
    }
}
