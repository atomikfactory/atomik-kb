//! The local tables of comments, notifications and person mentions (schema v18, M13 slice 2,
//! ADR-0048 and ADR-0049, `docs/development/milestone-13.md` sections 2.1 and 2.2): the records of
//! each page's comment stream, the words of comments for the text index, this device's inbox of
//! notifications, and the derived table of person mentions. Blobs (the encoded signed records) are
//! opaque here: `kb-model` encodes them and `kb-core` verifies them.
//!
//! Same conventions as [`crate::sync`] and [`crate::sharing`]: the methods exist on [`Db`] and [`Tx`]
//! through the traits below, both running the same private functions over a connection; the
//! multi-statement methods of a [`Db`] run as one atomic unit; ids are bound as 16-byte blobs; rows
//! are read as raw column values and validated afterwards, so a damaged row is a typed `InvalidData`
//! error; record bytes, comment text and names never appear in an error.
//!
//! # `comments`
//!
//! One row per verified record, whether the fold counts it or not (an `Edit` that arrives before its
//! `Post` counts once the `Post` is here). The core computes threads from the rows of a page with
//! `kb_model::CommentStream`; nothing here interprets a record. `cursor` is the feed cursor of the
//! item that carried the record (what snapshot dominance counts: the records seen at cursors up to a
//! snapshot's `covers`), `NULL` for a record this device made and has not pushed yet.
//!
//! # `comment_fts`
//!
//! One row per comment whose `Post` counts and that is not deleted: its latest valid body's text. A
//! search ([`CommentQueries::comment_fts_search`]) reads it with the rules of the page text index
//! (words matched by prefix, case and accents folded, the same snippet markers) and joins `pages_index`
//! for the trash, the space and the page's edit time.
//!
//! # `notifications`
//!
//! This device's inbox, bounded to `kb_types::comments::NOTIFICATIONS_MAX_ROWS` rows: past it the
//! oldest read rows go first, then the oldest unread ones, never the row just added. A row stores ids
//! only (the actor's name and the page's title are read when it is listed).

use kb_types::comments::NOTIFICATIONS_MAX_ROWS;
use kb_types::{
    AccountId, BlockId, CommentId, CommentRecordId, DeviceId, DocId, MemberRole, NotificationKind,
    PageId, SpaceId, ThreadId,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::docs::{atomically, id_from};
use crate::sync::{count_from, int_from};
use crate::text::{
    HIGHLIGHT_CLOSE_SQL, HIGHLIGHT_OPEN_SQL, MAX_SEARCH_LIMIT, SNIPPET_ELLIPSIS, decode_snippet,
    indexable, match_expression,
};
use crate::{Db, SearchParams, StorageError, Tx};

/// A `comments` row: one record of a page's comment stream, as the core verified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRow {
    /// The record's id (the primary key; a record id is stored once).
    pub record_id: CommentRecordId,
    /// The page's space (follows the page when it moves, [`CommentQueries::comment_records_set_space`]).
    pub space_id: SpaceId,
    /// The page whose stream carries the record.
    pub page_id: PageId,
    /// The record's thread.
    pub thread_id: ThreadId,
    /// The record's comment.
    pub comment_id: CommentId,
    /// The op byte of the record: 1 `Post`, 2 `Edit`, 3 `Delete`, 4 `Resolve`
    /// (`kb_model::CommentOp::code`).
    pub op: u8,
    /// The author's account.
    pub author: AccountId,
    /// The signing device.
    pub device: DeviceId,
    /// The record's revision.
    pub rev: u32,
    /// The record's time, Unix ms (the author's clock).
    pub at: i64,
    /// The feed cursor of the item that carried the record; `None` for a record this device made
    /// and has not pushed yet.
    pub cursor: Option<u64>,
    /// The author's role at that cursor (what the fold judges the record by).
    pub author_role: MemberRole,
    /// `kb_model::CommentRecord::encode`: the whole signed record.
    pub record: Vec<u8>,
}

/// What [`CommentQueries::comment_fts_replace`] writes: the words of one comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentTextRow {
    /// The comment.
    pub comment_id: CommentId,
    /// Its text: the text items of its latest valid body, joined with one space.
    pub text: String,
}

/// One hit of [`CommentQueries::comment_fts_search`]: a comment that holds the words.
#[derive(Debug, Clone, PartialEq)]
pub struct CommentFtsHit {
    /// The page the comment is on.
    pub page_id: PageId,
    /// The comment.
    pub comment_id: CommentId,
    /// Text around the match, plain, possibly shortened with an ellipsis.
    pub snippet: String,
    /// Match positions inside `snippet` as UTF-16 code unit ranges `(start, end)`.
    pub highlights: Vec<(u32, u32)>,
    /// FTS5 rank (`bm25`, more negative is better).
    pub rank: f64,
}

/// What [`NotificationQueries::notification_add`] stores (the id is the table's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotification {
    /// Why.
    pub kind: NotificationKind,
    /// The space.
    pub space_id: SpaceId,
    /// The page.
    pub page_id: Option<PageId>,
    /// The block.
    pub block_id: Option<BlockId>,
    /// The comment, for the two comment kinds.
    pub comment_id: Option<CommentId>,
    /// Who did it.
    pub actor: AccountId,
    /// When this device saw the change, Unix ms.
    pub at: i64,
}

/// A `notifications` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRow {
    /// The row id.
    pub id: i64,
    /// Why.
    pub kind: NotificationKind,
    /// The space.
    pub space_id: SpaceId,
    /// The page.
    pub page_id: Option<PageId>,
    /// The block.
    pub block_id: Option<BlockId>,
    /// The comment.
    pub comment_id: Option<CommentId>,
    /// Who did it.
    pub actor: AccountId,
    /// When this device saw the change, Unix ms.
    pub at: i64,
    /// Marked read.
    pub read: bool,
}

/// A `person_mentions` row without its document (the document is the argument of the methods).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PersonMentionRow {
    /// The block that holds the mention.
    pub block_id: BlockId,
    /// The mentioned account.
    pub account_id: AccountId,
}

/// Queries of the `comments` and `comment_fts` tables, available on a connection or inside a
/// transaction. On a [`Db`] every method is atomic by itself.
pub trait CommentQueries {
    /// Stores a record (an insert that keeps the row already there when the record id exists).
    /// Answers whether the record was new. A cursor above `i64::MAX` is `InvalidData` and nothing
    /// is written.
    fn comment_record_put(&self, row: &CommentRow) -> Result<bool, StorageError>;

    /// Every record of a page's stream, ordered by `(at, record_id)`; empty for a page without any.
    fn comment_records_of_page(&self, page_id: PageId) -> Result<Vec<CommentRow>, StorageError>;

    /// Fills in the feed cursor of a record this device made once the push got its entry's cursor.
    /// Answers whether a row was found (an unknown record is not an error).
    fn comment_record_set_cursor(
        &self,
        record_id: CommentRecordId,
        cursor: u64,
    ) -> Result<bool, StorageError>;

    /// Removes a page's records (purge; the stream document goes with the page). Answers how many.
    fn comment_records_delete_page(&self, page_id: PageId) -> Result<u64, StorageError>;

    /// Every page that has at least one record, with the space its records name, ordered by page
    /// id (what a rebuild of the comment index walks).
    fn comment_pages(&self) -> Result<Vec<(SpaceId, PageId)>, StorageError>;

    /// Points a page's records at another space (the page moved, ADR-0038: the stream moves with
    /// it) and nothing else. Answers how many rows changed.
    fn comment_records_set_space(
        &self,
        page_id: PageId,
        space_id: SpaceId,
    ) -> Result<u64, StorageError>;

    /// Replaces the `comment_fts` rows of a page with one row per `(comment, text)` given. A row
    /// whose text is blank is not written. The two private-use characters of snippet markers are
    /// stored as U+FFFD like in the main text index.
    fn comment_fts_replace(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        rows: &[CommentTextRow],
    ) -> Result<(), StorageError>;

    /// Removes a page's `comment_fts` rows. Succeeds when it has none.
    fn comment_fts_delete_page(&self, page_id: PageId) -> Result<(), StorageError>;

    /// Removes every `comment_fts` row (before a full rebuild).
    fn comment_fts_clear(&self) -> Result<(), StorageError>;

    /// Searches the words of comments with the same query rules as `fts_search` (`params.query`
    /// is split into words matched by prefix; `any_word` ORs them; `titles_only` finds nothing).
    /// Pages that are trashed, in a template or missing from `pages_index` are excluded; `space_id` and
    /// `edited_after` filter through it. Ordered by `bm25` then by comment id, at most
    /// `params.limit` hits.
    fn comment_fts_search(
        &self,
        params: &SearchParams<'_>,
    ) -> Result<Vec<CommentFtsHit>, StorageError>;
}

/// Queries of the `notifications` table (this device's inbox), available on a connection or inside
/// a transaction. On a [`Db`] every method is atomic by itself.
pub trait NotificationQueries {
    /// Adds a row (unread) and answers its id. The table keeps at most
    /// `kb_types::comments::NOTIFICATIONS_MAX_ROWS` rows: past that the oldest read rows go first,
    /// then the oldest unread ones, never the row just added.
    fn notification_add(&self, row: &NewNotification) -> Result<i64, StorageError>;

    /// One row, `None` when it does not exist.
    fn notification_get(&self, id: i64) -> Result<Option<NotificationRow>, StorageError>;

    /// Rows newest first (by id), at most `limit`, only those with an id below `before_id` when
    /// given, only unread ones when `unread_only`.
    fn notification_list(
        &self,
        limit: u32,
        before_id: Option<i64>,
        unread_only: bool,
    ) -> Result<Vec<NotificationRow>, StorageError>;

    /// Marks the listed rows read, or every row when `ids` is `None`. Answers how many rows
    /// changed (a row that was read already, or does not exist, does not count).
    fn notification_mark_read(&self, ids: Option<&[i64]>) -> Result<u32, StorageError>;

    /// How many rows are unread.
    fn notification_unread_count(&self) -> Result<u32, StorageError>;

    /// Removes the rows of a space (the space was left or deleted on this device). Answers how many.
    fn notification_delete_space(&self, space_id: SpaceId) -> Result<u64, StorageError>;
}

/// Queries of the derived `person_mentions` table, available on a connection or inside a
/// transaction. On a [`Db`] every method is atomic by itself.
pub trait PersonMentionQueries {
    /// Replaces the rows of a page document with `rows` (the same `(block, account)` twice keeps one
    /// row). The page does not have to be in `pages_index`.
    fn person_mentions_replace(
        &self,
        doc_id: DocId,
        rows: &[PersonMentionRow],
    ) -> Result<(), StorageError>;

    /// The rows of a page document, ordered by `(block, account)`; what a remote change is diffed
    /// against.
    fn person_mentions_of(&self, doc_id: DocId) -> Result<Vec<PersonMentionRow>, StorageError>;

    /// Removes the rows of a page document (purge). Succeeds when it has none.
    fn person_mentions_delete(&self, doc_id: DocId) -> Result<(), StorageError>;

    /// Removes every row (before a full rebuild).
    fn person_mentions_clear(&self) -> Result<(), StorageError>;
}

impl CommentQueries for Db {
    fn comment_record_put(&self, row: &CommentRow) -> Result<bool, StorageError> {
        comment_record_put_on(&self.conn, row)
    }
    fn comment_records_of_page(&self, page_id: PageId) -> Result<Vec<CommentRow>, StorageError> {
        comment_records_of_page_on(&self.conn, page_id)
    }
    fn comment_record_set_cursor(
        &self,
        record_id: CommentRecordId,
        cursor: u64,
    ) -> Result<bool, StorageError> {
        comment_record_set_cursor_on(&self.conn, record_id, cursor)
    }
    fn comment_records_delete_page(&self, page_id: PageId) -> Result<u64, StorageError> {
        comment_records_delete_page_on(&self.conn, page_id)
    }
    fn comment_pages(&self) -> Result<Vec<(SpaceId, PageId)>, StorageError> {
        comment_pages_on(&self.conn)
    }
    fn comment_records_set_space(
        &self,
        page_id: PageId,
        space_id: SpaceId,
    ) -> Result<u64, StorageError> {
        comment_records_set_space_on(&self.conn, page_id, space_id)
    }
    fn comment_fts_replace(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        rows: &[CommentTextRow],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            comment_fts_replace_on(conn, space_id, page_id, rows)
        })
    }
    fn comment_fts_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        comment_fts_delete_page_on(&self.conn, page_id)
    }
    fn comment_fts_clear(&self) -> Result<(), StorageError> {
        comment_fts_clear_on(&self.conn)
    }
    fn comment_fts_search(
        &self,
        params: &SearchParams<'_>,
    ) -> Result<Vec<CommentFtsHit>, StorageError> {
        comment_fts_search_on(&self.conn, params)
    }
}

impl CommentQueries for Tx<'_> {
    fn comment_record_put(&self, row: &CommentRow) -> Result<bool, StorageError> {
        comment_record_put_on(&self.tx, row)
    }
    fn comment_records_of_page(&self, page_id: PageId) -> Result<Vec<CommentRow>, StorageError> {
        comment_records_of_page_on(&self.tx, page_id)
    }
    fn comment_record_set_cursor(
        &self,
        record_id: CommentRecordId,
        cursor: u64,
    ) -> Result<bool, StorageError> {
        comment_record_set_cursor_on(&self.tx, record_id, cursor)
    }
    fn comment_records_delete_page(&self, page_id: PageId) -> Result<u64, StorageError> {
        comment_records_delete_page_on(&self.tx, page_id)
    }
    fn comment_pages(&self) -> Result<Vec<(SpaceId, PageId)>, StorageError> {
        comment_pages_on(&self.tx)
    }
    fn comment_records_set_space(
        &self,
        page_id: PageId,
        space_id: SpaceId,
    ) -> Result<u64, StorageError> {
        comment_records_set_space_on(&self.tx, page_id, space_id)
    }
    fn comment_fts_replace(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        rows: &[CommentTextRow],
    ) -> Result<(), StorageError> {
        comment_fts_replace_on(&self.tx, space_id, page_id, rows)
    }
    fn comment_fts_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        comment_fts_delete_page_on(&self.tx, page_id)
    }
    fn comment_fts_clear(&self) -> Result<(), StorageError> {
        comment_fts_clear_on(&self.tx)
    }
    fn comment_fts_search(
        &self,
        params: &SearchParams<'_>,
    ) -> Result<Vec<CommentFtsHit>, StorageError> {
        comment_fts_search_on(&self.tx, params)
    }
}

impl NotificationQueries for Db {
    fn notification_add(&self, row: &NewNotification) -> Result<i64, StorageError> {
        atomically(&self.conn, |conn| notification_add_on(conn, row))
    }
    fn notification_get(&self, id: i64) -> Result<Option<NotificationRow>, StorageError> {
        notification_get_on(&self.conn, id)
    }
    fn notification_list(
        &self,
        limit: u32,
        before_id: Option<i64>,
        unread_only: bool,
    ) -> Result<Vec<NotificationRow>, StorageError> {
        notification_list_on(&self.conn, limit, before_id, unread_only)
    }
    fn notification_mark_read(&self, ids: Option<&[i64]>) -> Result<u32, StorageError> {
        atomically(&self.conn, |conn| notification_mark_read_on(conn, ids))
    }
    fn notification_unread_count(&self) -> Result<u32, StorageError> {
        notification_unread_count_on(&self.conn)
    }
    fn notification_delete_space(&self, space_id: SpaceId) -> Result<u64, StorageError> {
        notification_delete_space_on(&self.conn, space_id)
    }
}

impl NotificationQueries for Tx<'_> {
    fn notification_add(&self, row: &NewNotification) -> Result<i64, StorageError> {
        notification_add_on(&self.tx, row)
    }
    fn notification_get(&self, id: i64) -> Result<Option<NotificationRow>, StorageError> {
        notification_get_on(&self.tx, id)
    }
    fn notification_list(
        &self,
        limit: u32,
        before_id: Option<i64>,
        unread_only: bool,
    ) -> Result<Vec<NotificationRow>, StorageError> {
        notification_list_on(&self.tx, limit, before_id, unread_only)
    }
    fn notification_mark_read(&self, ids: Option<&[i64]>) -> Result<u32, StorageError> {
        notification_mark_read_on(&self.tx, ids)
    }
    fn notification_unread_count(&self) -> Result<u32, StorageError> {
        notification_unread_count_on(&self.tx)
    }
    fn notification_delete_space(&self, space_id: SpaceId) -> Result<u64, StorageError> {
        notification_delete_space_on(&self.tx, space_id)
    }
}

impl PersonMentionQueries for Db {
    fn person_mentions_replace(
        &self,
        doc_id: DocId,
        rows: &[PersonMentionRow],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            person_mentions_replace_on(conn, doc_id, rows)
        })
    }
    fn person_mentions_of(&self, doc_id: DocId) -> Result<Vec<PersonMentionRow>, StorageError> {
        person_mentions_of_on(&self.conn, doc_id)
    }
    fn person_mentions_delete(&self, doc_id: DocId) -> Result<(), StorageError> {
        person_mentions_delete_on(&self.conn, doc_id)
    }
    fn person_mentions_clear(&self) -> Result<(), StorageError> {
        person_mentions_clear_on(&self.conn)
    }
}

impl PersonMentionQueries for Tx<'_> {
    fn person_mentions_replace(
        &self,
        doc_id: DocId,
        rows: &[PersonMentionRow],
    ) -> Result<(), StorageError> {
        person_mentions_replace_on(&self.tx, doc_id, rows)
    }
    fn person_mentions_of(&self, doc_id: DocId) -> Result<Vec<PersonMentionRow>, StorageError> {
        person_mentions_of_on(&self.tx, doc_id)
    }
    fn person_mentions_delete(&self, doc_id: DocId) -> Result<(), StorageError> {
        person_mentions_delete_on(&self.tx, doc_id)
    }
    fn person_mentions_clear(&self) -> Result<(), StorageError> {
        person_mentions_clear_on(&self.tx)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// The columns of `comments` in the order [`CommentColumns::read`] expects.
const COMMENT_COLUMNS: &str = "record_id, space_id, page_id, thread_id, comment_id, op, author, \
     device, rev, at, cursor, author_role, record";

/// Raw column values of a `comments` row.
struct CommentColumns {
    record_id: Vec<u8>,
    space_id: Vec<u8>,
    page_id: Vec<u8>,
    thread_id: Vec<u8>,
    comment_id: Vec<u8>,
    op: i64,
    author: Vec<u8>,
    device: Vec<u8>,
    rev: i64,
    at: i64,
    cursor: Option<i64>,
    author_role: i64,
    record: Vec<u8>,
}

impl CommentColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            record_id: row.get(0)?,
            space_id: row.get(1)?,
            page_id: row.get(2)?,
            thread_id: row.get(3)?,
            comment_id: row.get(4)?,
            op: row.get(5)?,
            author: row.get(6)?,
            device: row.get(7)?,
            rev: row.get(8)?,
            at: row.get(9)?,
            cursor: row.get(10)?,
            author_role: row.get(11)?,
            record: row.get(12)?,
        })
    }

    fn decode(self) -> Result<CommentRow, StorageError> {
        Ok(CommentRow {
            record_id: CommentRecordId::from_bytes(id_from(&self.record_id, "comment record id")?),
            space_id: SpaceId::from_bytes(id_from(&self.space_id, "comment space id")?),
            page_id: PageId::from_bytes(id_from(&self.page_id, "comment page id")?),
            thread_id: ThreadId::from_bytes(id_from(&self.thread_id, "comment thread id")?),
            comment_id: CommentId::from_bytes(id_from(&self.comment_id, "comment id")?),
            op: u8::try_from(self.op)
                .ok()
                .filter(|op| (1..=4).contains(op))
                .ok_or_else(|| {
                    StorageError::InvalidData("a comment op is not 1 to 4".to_owned())
                })?,
            author: AccountId::from_bytes(id_from(&self.author, "comment author id")?),
            device: DeviceId::from_bytes(id_from(&self.device, "comment device id")?),
            rev: u32::try_from(self.rev).map_err(|_| {
                StorageError::InvalidData("a comment revision is out of range".to_owned())
            })?,
            at: self.at,
            cursor: self
                .cursor
                .map(|cursor| count_from(cursor, "comment cursor"))
                .transpose()?,
            author_role: u8::try_from(self.author_role)
                .ok()
                .and_then(MemberRole::from_byte)
                .ok_or_else(|| {
                    StorageError::InvalidData("a comment author role is not 1 to 4".to_owned())
                })?,
            record: self.record,
        })
    }
}

fn comment_record_put_on(conn: &Connection, row: &CommentRow) -> Result<bool, StorageError> {
    // The cursor is converted before the statement runs: a refusal writes nothing.
    let cursor = row
        .cursor
        .map(|cursor| int_from(cursor, "comment cursor"))
        .transpose()?;
    let inserted = conn.execute(
        "INSERT INTO comments (record_id, space_id, page_id, thread_id, comment_id, op, author, \
            device, rev, at, cursor, author_role, record) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
         ON CONFLICT (record_id) DO NOTHING",
        params![
            row.record_id.to_bytes(),
            row.space_id.to_bytes(),
            row.page_id.to_bytes(),
            row.thread_id.to_bytes(),
            row.comment_id.to_bytes(),
            i64::from(row.op),
            row.author.to_bytes(),
            row.device.to_bytes(),
            i64::from(row.rev),
            row.at,
            cursor,
            i64::from(row.author_role.to_byte()),
            row.record,
        ],
    )?;
    Ok(inserted > 0)
}

fn comment_records_of_page_on(
    conn: &Connection,
    page_id: PageId,
) -> Result<Vec<CommentRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COMMENT_COLUMNS} FROM comments WHERE page_id = ?1 ORDER BY at, record_id"
    ))?;
    let rows = stmt
        .query_map([page_id.to_bytes()], CommentColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(CommentColumns::decode).collect()
}

fn comment_record_set_cursor_on(
    conn: &Connection,
    record_id: CommentRecordId,
    cursor: u64,
) -> Result<bool, StorageError> {
    let cursor = int_from(cursor, "comment cursor")?;
    let changed = conn.execute(
        "UPDATE comments SET cursor = ?2 WHERE record_id = ?1",
        params![record_id.to_bytes(), cursor],
    )?;
    Ok(changed > 0)
}

fn comment_records_delete_page_on(conn: &Connection, page_id: PageId) -> Result<u64, StorageError> {
    let removed = conn.execute(
        "DELETE FROM comments WHERE page_id = ?1",
        [page_id.to_bytes()],
    )?;
    Ok(removed as u64)
}

fn comment_pages_on(conn: &Connection) -> Result<Vec<(SpaceId, PageId)>, StorageError> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT page_id, space_id FROM comments ORDER BY page_id, space_id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(page, space)| {
            Ok((
                SpaceId::from_bytes(id_from(&space, "comment space id")?),
                PageId::from_bytes(id_from(&page, "comment page id")?),
            ))
        })
        .collect()
}

fn comment_records_set_space_on(
    conn: &Connection,
    page_id: PageId,
    space_id: SpaceId,
) -> Result<u64, StorageError> {
    let changed = conn.execute(
        "UPDATE comments SET space_id = ?2 WHERE page_id = ?1 AND space_id <> ?2",
        params![page_id.to_bytes(), space_id.to_bytes()],
    )?;
    Ok(changed as u64)
}

fn comment_fts_replace_on(
    conn: &Connection,
    space_id: SpaceId,
    page_id: PageId,
    rows: &[CommentTextRow],
) -> Result<(), StorageError> {
    comment_fts_delete_page_on(conn, page_id)?;
    let mut insert = conn.prepare(
        "INSERT INTO comment_fts (body, page_id, comment_id, space_id) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for row in rows.iter().filter(|row| !row.text.trim().is_empty()) {
        insert.execute(params![
            indexable(&row.text),
            page_id.to_bytes(),
            row.comment_id.to_bytes(),
            space_id.to_bytes(),
        ])?;
    }
    Ok(())
}

fn comment_fts_delete_page_on(conn: &Connection, page_id: PageId) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM comment_fts WHERE page_id = ?1",
        [page_id.to_bytes()],
    )?;
    Ok(())
}

fn comment_fts_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM comment_fts", [])?;
    Ok(())
}

/// The comments that hold the words, with their live pages. `comment_fts` drives through its
/// word index; each hit looks its page up by primary key in `pages_index`.
const COMMENT_SEARCH_SQL: &str = r"
SELECT comment_fts.page_id,
       comment_fts.comment_id,
       snippet(comment_fts, 0, ?3, ?4, ?5, 12),
       bm25(comment_fts) AS score
  FROM comment_fts
 CROSS JOIN pages_index AS p ON p.page_id = comment_fts.page_id
 WHERE comment_fts MATCH ?1
   AND p.trashed_at IS NULL
   AND p.in_template = 0
   AND (?2 IS NULL OR p.space_id = ?2)
   AND (?7 IS NULL OR p.updated_at >= ?7)
 ORDER BY score, comment_fts.comment_id
 LIMIT ?6";

fn comment_fts_search_on(
    conn: &Connection,
    search: &SearchParams<'_>,
) -> Result<Vec<CommentFtsHit>, StorageError> {
    if search.titles_only {
        return Ok(Vec::new());
    }
    let Some(expression) = match_expression(search.query, search.any_word) else {
        return Ok(Vec::new());
    };
    let limit = search.limit.clamp(1, MAX_SEARCH_LIMIT);
    let mut stmt = conn.prepare(COMMENT_SEARCH_SQL)?;
    let rows = stmt
        .query_map(
            params![
                expression,
                search.space_id.map(SpaceId::to_bytes),
                HIGHLIGHT_OPEN_SQL,
                HIGHLIGHT_CLOSE_SQL,
                SNIPPET_ELLIPSIS,
                i64::from(limit),
                search.edited_after,
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(page, comment, snippet, rank)| {
            let (snippet, highlights) = decode_snippet(snippet.as_deref().unwrap_or_default());
            Ok(CommentFtsHit {
                page_id: PageId::from_bytes(id_from(&page, "indexed comment page id")?),
                comment_id: CommentId::from_bytes(id_from(&comment, "indexed comment id")?),
                snippet,
                highlights,
                rank,
            })
        })
        .collect()
}

// ---- notifications ----

/// The stored number of a kind (on disk: never renumber).
const fn kind_code(kind: NotificationKind) -> i64 {
    match kind {
        NotificationKind::ThreadReply => 1,
        NotificationKind::CommentMention => 2,
        NotificationKind::PageMention => 3,
        NotificationKind::PersonAssigned => 4,
    }
}

fn kind_from(code: i64) -> Result<NotificationKind, StorageError> {
    match code {
        1 => Ok(NotificationKind::ThreadReply),
        2 => Ok(NotificationKind::CommentMention),
        3 => Ok(NotificationKind::PageMention),
        4 => Ok(NotificationKind::PersonAssigned),
        _ => Err(StorageError::InvalidData(
            "a notification kind is not 1 to 4".to_owned(),
        )),
    }
}

/// The columns of `notifications` in the order [`NotificationColumns::read`] expects.
const NOTIFICATION_COLUMNS: &str =
    "id, kind, space_id, page_id, block_id, comment_id, actor, at, read";

/// Raw column values of a `notifications` row.
struct NotificationColumns {
    id: i64,
    kind: i64,
    space_id: Vec<u8>,
    page_id: Option<Vec<u8>>,
    block_id: Option<Vec<u8>>,
    comment_id: Option<Vec<u8>>,
    actor: Vec<u8>,
    at: i64,
    read: i64,
}

impl NotificationColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            kind: row.get(1)?,
            space_id: row.get(2)?,
            page_id: row.get(3)?,
            block_id: row.get(4)?,
            comment_id: row.get(5)?,
            actor: row.get(6)?,
            at: row.get(7)?,
            read: row.get(8)?,
        })
    }

    fn decode(self) -> Result<NotificationRow, StorageError> {
        Ok(NotificationRow {
            id: self.id,
            kind: kind_from(self.kind)?,
            space_id: SpaceId::from_bytes(id_from(&self.space_id, "notification space id")?),
            page_id: self
                .page_id
                .as_deref()
                .map(|bytes| id_from(bytes, "notification page id").map(PageId::from_bytes))
                .transpose()?,
            block_id: self
                .block_id
                .as_deref()
                .map(|bytes| id_from(bytes, "notification block id").map(BlockId::from_bytes))
                .transpose()?,
            comment_id: self
                .comment_id
                .as_deref()
                .map(|bytes| id_from(bytes, "notification comment id").map(CommentId::from_bytes))
                .transpose()?,
            actor: AccountId::from_bytes(id_from(&self.actor, "notification actor id")?),
            at: self.at,
            read: match self.read {
                0 => false,
                1 => true,
                _ => {
                    return Err(StorageError::InvalidData(
                        "a notification read flag is neither 0 nor 1".to_owned(),
                    ));
                }
            },
        })
    }
}

fn notification_add_on(conn: &Connection, row: &NewNotification) -> Result<i64, StorageError> {
    conn.execute(
        "INSERT INTO notifications (kind, space_id, page_id, block_id, comment_id, actor, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            kind_code(row.kind),
            row.space_id.to_bytes(),
            row.page_id.map(PageId::to_bytes),
            row.block_id.map(BlockId::to_bytes),
            row.comment_id.map(CommentId::to_bytes),
            row.actor.to_bytes(),
            row.at,
        ],
    )?;
    let id = conn.last_insert_rowid();
    // Keep the table to its bound: the oldest read rows go first, then the oldest unread ones, and
    // never the row just added.
    let total: i64 = conn.query_one("SELECT count(*) FROM notifications", [], |row| row.get(0))?;
    let bound = i64::try_from(NOTIFICATIONS_MAX_ROWS).unwrap_or(i64::MAX);
    if total > bound {
        conn.execute(
            "DELETE FROM notifications WHERE id IN ( \
               SELECT id FROM notifications WHERE id <> ?1 ORDER BY read DESC, id LIMIT ?2)",
            params![id, total - bound],
        )?;
    }
    Ok(id)
}

fn notification_get_on(
    conn: &Connection,
    id: i64,
) -> Result<Option<NotificationRow>, StorageError> {
    conn.query_row(
        &format!("SELECT {NOTIFICATION_COLUMNS} FROM notifications WHERE id = ?1"),
        [id],
        NotificationColumns::read,
    )
    .optional()?
    .map(NotificationColumns::decode)
    .transpose()
}

fn notification_list_on(
    conn: &Connection,
    limit: u32,
    before_id: Option<i64>,
    unread_only: bool,
) -> Result<Vec<NotificationRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NOTIFICATION_COLUMNS} FROM notifications \
          WHERE (?2 IS NULL OR id < ?2) AND (?3 = 0 OR read = 0) \
          ORDER BY id DESC LIMIT ?1"
    ))?;
    let rows = stmt
        .query_map(
            params![i64::from(limit), before_id, unread_only],
            NotificationColumns::read,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(NotificationColumns::decode).collect()
}

fn notification_mark_read_on(conn: &Connection, ids: Option<&[i64]>) -> Result<u32, StorageError> {
    let changed = match ids {
        None => conn.execute("UPDATE notifications SET read = 1 WHERE read = 0", [])?,
        Some(ids) => {
            let mut update =
                conn.prepare("UPDATE notifications SET read = 1 WHERE id = ?1 AND read = 0")?;
            let mut changed = 0;
            for id in ids {
                changed += update.execute([id])?;
            }
            changed
        }
    };
    u32::try_from(changed)
        .map_err(|_| StorageError::InvalidData("a row count is out of range".to_owned()))
}

fn notification_unread_count_on(conn: &Connection) -> Result<u32, StorageError> {
    let count: i64 = conn.query_one(
        "SELECT count(*) FROM notifications WHERE read = 0",
        [],
        |row| row.get(0),
    )?;
    u32::try_from(count)
        .map_err(|_| StorageError::InvalidData("a row count is out of range".to_owned()))
}

fn notification_delete_space_on(conn: &Connection, space_id: SpaceId) -> Result<u64, StorageError> {
    let removed = conn.execute(
        "DELETE FROM notifications WHERE space_id = ?1",
        [space_id.to_bytes()],
    )?;
    Ok(removed as u64)
}

// ---- person_mentions ----

fn person_mentions_replace_on(
    conn: &Connection,
    doc_id: DocId,
    rows: &[PersonMentionRow],
) -> Result<(), StorageError> {
    person_mentions_delete_on(conn, doc_id)?;
    let mut insert = conn.prepare(
        "INSERT INTO person_mentions (doc_id, block_id, account_id) VALUES (?1, ?2, ?3) \
         ON CONFLICT DO NOTHING",
    )?;
    for row in rows {
        insert.execute(params![
            doc_id.to_bytes(),
            row.block_id.to_bytes(),
            row.account_id.to_bytes()
        ])?;
    }
    Ok(())
}

fn person_mentions_of_on(
    conn: &Connection,
    doc_id: DocId,
) -> Result<Vec<PersonMentionRow>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT block_id, account_id FROM person_mentions WHERE doc_id = ?1 \
          ORDER BY block_id, account_id",
    )?;
    let rows = stmt
        .query_map([doc_id.to_bytes()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(block, account)| {
            Ok(PersonMentionRow {
                block_id: BlockId::from_bytes(id_from(&block, "person mention block id")?),
                account_id: AccountId::from_bytes(id_from(&account, "person mention account id")?),
            })
        })
        .collect()
}

fn person_mentions_delete_on(conn: &Connection, doc_id: DocId) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM person_mentions WHERE doc_id = ?1",
        [doc_id.to_bytes()],
    )?;
    Ok(())
}

fn person_mentions_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM person_mentions", [])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::{DocQueries, PageIndexRow, migrations};

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut statement = conn
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

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn migration_18_creates_the_comment_notification_and_mention_tables() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut conn).unwrap();
        const { assert!(crate::SCHEMA_VERSION >= 18) };
        assert_eq!(
            columns(&conn, "comments"),
            [
                "record_id",
                "space_id",
                "page_id",
                "thread_id",
                "comment_id",
                "op",
                "author",
                "device",
                "rev",
                "at",
                "cursor",
                "author_role",
                "record"
            ]
        );
        assert_eq!(
            columns(&conn, "notifications"),
            [
                "id",
                "kind",
                "space_id",
                "page_id",
                "block_id",
                "comment_id",
                "actor",
                "at",
                "read"
            ]
        );
        assert_eq!(
            columns(&conn, "person_mentions"),
            ["doc_id", "block_id", "account_id"]
        );
        assert_eq!(
            columns(&conn, "comment_fts"),
            ["body", "page_id", "comment_id", "space_id"]
        );
        // The full-text table takes a row and finds its words, diacritics folded.
        conn.execute(
            "INSERT INTO comment_fts (body, page_id, comment_id, space_id) \
             VALUES ('Café menu looks good', x'01', x'02', x'03')",
            [],
        )
        .unwrap();
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM comment_fts WHERE comment_fts MATCH 'cafe'"
            ),
            1
        );
        // An op or a kind outside the documented numbers is refused.
        assert!(
            conn.execute(
                "INSERT INTO notifications (kind, space_id, actor, at) VALUES (9, x'01', x'02', 0)",
                []
            )
            .is_err()
        );
        conn.execute(
            "INSERT INTO notifications (kind, space_id, actor, at) VALUES (2, x'01', x'02', 5)",
            [],
        )
        .unwrap();
        assert_eq!(count(&conn, "SELECT read FROM notifications"), 0);
    }

    #[test]
    fn migrating_from_v17_keeps_the_property_and_date_mention_rows_and_adds_the_person_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 17).unwrap();
        conn.execute(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, value_text) \
             VALUES (x'01', x'02', x'03', 'text', 'Quarterly')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, related_page_id) \
             VALUES (x'01', x'02', x'04', 'relation', x'05')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO date_mentions (doc_id, page_id, block_id, node_id, date, time, \
             reminder_minutes, fire_at_local) VALUES (x'02', x'02', x'06', x'07', '2026-10-07', \
             '14:30', 15, 1000)",
            [],
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();

        // The rows are what they were, in the order they were written, and the new column is empty.
        assert_eq!(count(&conn, "SELECT count(*) FROM page_properties"), 2);
        assert_eq!(
            conn.query_row(
                "SELECT value_text FROM page_properties ORDER BY rowid LIMIT 1",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "Quarterly"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM page_properties WHERE account_id IS NOT NULL"
            ),
            0
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM date_mentions"), 1);
        assert_eq!(
            count(
                &conn,
                "SELECT reminder_for IS NULL FROM date_mentions WHERE reminder_minutes = 15"
            ),
            1
        );
        // `person` is a kind now, with one row per account and one account once per page property.
        let person = |account: &str| {
            conn.execute(
                &format!(
                    "INSERT INTO page_properties (space_id, page_id, property_id, kind, account_id) \
                     VALUES (x'01', x'02', x'08', 'person', {account})"
                ),
                [],
            )
        };
        person("x'0a'").unwrap();
        person("x'0b'").unwrap();
        assert!(person("x'0a'").is_err(), "the same account twice");
        assert!(
            conn.execute(
                "INSERT INTO page_properties (space_id, page_id, property_id, kind) \
                 VALUES (x'01', x'02', x'09', 'nonsense')",
                []
            )
            .is_err()
        );
        // The scalar rows are still unique per property (the index that v13 had, now also without an account).
        assert!(
            conn.execute(
                "INSERT INTO page_properties (space_id, page_id, property_id, kind, value_text) \
                 VALUES (x'01', x'02', x'03', 'text', 'again')",
                []
            )
            .is_err()
        );
    }

    // ---- the queries ----

    /// What a scenario needs of a connection or a transaction: both implement all of it.
    trait Queries: DocQueries + CommentQueries + NotificationQueries + PersonMentionQueries {}
    impl<T: DocQueries + CommentQueries + NotificationQueries + PersonMentionQueries> Queries for T {}

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that both
    /// implementations of the traits are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&Db::open_in_memory().unwrap());
        Db::open_in_memory()
            .unwrap()
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

    const fn account(n: u8) -> AccountId {
        AccountId::from_bytes([n; 16])
    }

    fn add_page(q: &dyn Queries, n: u8, space_id: SpaceId, trashed_at: Option<i64>, updated: i64) {
        q.pages_index_upsert(&[PageIndexRow {
            page_id: page(n),
            space_id,
            parent_id: None,
            title: "page".to_owned(),
            trashed_at,
            updated_at: updated,
            in_template: false,
        }])
        .unwrap();
    }

    fn record(n: u8, page_n: u8, at: i64) -> CommentRow {
        CommentRow {
            record_id: CommentRecordId::from_bytes([n; 16]),
            space_id: space(1),
            page_id: page(page_n),
            thread_id: ThreadId::from_bytes([n; 16]),
            comment_id: CommentId::from_bytes([n; 16]),
            op: 1,
            author: account(9),
            device: DeviceId::from_bytes([8; 16]),
            rev: 0,
            at,
            cursor: Some(u64::from(n)),
            author_role: MemberRole::Commenter,
            record: vec![n, 0x00, 0xFF],
        }
    }

    fn search<'a>(query: &'a str, space_id: Option<SpaceId>) -> SearchParams<'a> {
        SearchParams {
            query,
            space_id,
            titles_only: false,
            limit: 20,
            any_word: false,
            edited_after: None,
        }
    }

    #[test]
    fn records_are_stored_once_in_order_and_follow_their_page() {
        on_db_and_tx(|q| {
            assert!(q.comment_records_of_page(page(1)).unwrap().is_empty());
            // Three records of page 1 written out of order, one of page 2.
            assert!(q.comment_record_put(&record(3, 1, 300)).unwrap());
            assert!(q.comment_record_put(&record(1, 1, 100)).unwrap());
            assert!(q.comment_record_put(&record(2, 1, 100)).unwrap());
            assert!(q.comment_record_put(&record(4, 2, 50)).unwrap());
            // The same record id again is kept as it was: the first row wins, and it says so.
            let again = CommentRow {
                at: 999,
                record: vec![0],
                ..record(1, 1, 100)
            };
            assert!(!q.comment_record_put(&again).unwrap());
            let rows = q.comment_records_of_page(page(1)).unwrap();
            // Ordered by (at, record id).
            assert_eq!(
                rows.iter()
                    .map(|row| row.record_id.to_bytes()[0])
                    .collect::<Vec<_>>(),
                [1, 2, 3]
            );
            assert_eq!(rows[0], record(1, 1, 100));
            assert_eq!(q.comment_records_of_page(page(2)).unwrap().len(), 1);

            // A record this device made has no cursor until the push gives it one.
            let local = CommentRow {
                cursor: None,
                ..record(5, 1, 400)
            };
            assert!(q.comment_record_put(&local).unwrap());
            let stored = q.comment_records_of_page(page(1)).unwrap();
            assert_eq!(stored.last().unwrap().cursor, None);
            assert!(q.comment_record_set_cursor(local.record_id, 77).unwrap());
            assert_eq!(
                q.comment_records_of_page(page(1))
                    .unwrap()
                    .last()
                    .unwrap()
                    .cursor,
                Some(77)
            );
            assert!(
                !q.comment_record_set_cursor(CommentRecordId::from_bytes([0xEE; 16]), 1)
                    .unwrap(),
                "an unknown record is not an error"
            );
            // A cursor beyond SQLite's integers is refused and nothing is written.
            let huge = CommentRow {
                cursor: Some(u64::MAX),
                ..record(6, 1, 500)
            };
            assert!(matches!(
                q.comment_record_put(&huge),
                Err(StorageError::InvalidData(_))
            ));
            assert!(matches!(
                q.comment_record_set_cursor(local.record_id, u64::MAX),
                Err(StorageError::InvalidData(_))
            ));
            assert_eq!(q.comment_records_of_page(page(1)).unwrap().len(), 4);

            // The page moves to another space: only the space column changes.
            assert_eq!(q.comment_records_set_space(page(1), space(2)).unwrap(), 4);
            assert_eq!(q.comment_records_set_space(page(1), space(2)).unwrap(), 0);
            let moved = q.comment_records_of_page(page(1)).unwrap();
            assert!(moved.iter().all(|row| row.space_id == space(2)));
            assert_eq!(moved[0].record, record(1, 1, 100).record);
            assert_eq!(
                q.comment_records_of_page(page(2)).unwrap()[0].space_id,
                space(1)
            );

            // The pages that have records, with their space.
            assert_eq!(
                q.comment_pages().unwrap(),
                [(space(2), page(1)), (space(1), page(2))]
            );

            // A purge removes a page's records only.
            assert_eq!(q.comment_records_delete_page(page(1)).unwrap(), 4);
            assert_eq!(q.comment_records_delete_page(page(1)).unwrap(), 0);
            assert!(q.comment_records_of_page(page(1)).unwrap().is_empty());
            assert_eq!(q.comment_records_of_page(page(2)).unwrap().len(), 1);
        });
    }

    #[test]
    fn a_damaged_record_row_is_a_typed_error() {
        let db = Db::open_in_memory().unwrap();
        db.comment_record_put(&record(1, 1, 100)).unwrap();
        db.connection()
            .execute("UPDATE comments SET author_role = 9", [])
            .unwrap();
        assert!(matches!(
            db.comment_records_of_page(page(1)),
            Err(StorageError::InvalidData(_))
        ));
        db.connection()
            .execute("UPDATE comments SET author_role = 2, rev = -1", [])
            .unwrap();
        assert!(matches!(
            db.comment_records_of_page(page(1)),
            Err(StorageError::InvalidData(_))
        ));
    }

    #[test]
    fn comment_words_are_found_by_prefix_with_accents_folded_and_never_in_the_trash() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1), None, 1_000);
            add_page(q, 2, space(2), None, 2_000);
            add_page(q, 3, space(1), Some(5), 3_000);
            let comment = |n: u8| CommentId::from_bytes([n; 16]);
            let rows = |texts: &[(u8, &str)]| -> Vec<CommentTextRow> {
                texts
                    .iter()
                    .map(|(n, text)| CommentTextRow {
                        comment_id: comment(*n),
                        text: (*text).to_owned(),
                    })
                    .collect()
            };
            q.comment_fts_replace(
                space(1),
                page(1),
                &rows(&[
                    (1, "The café menu looks good"),
                    (2, "   "),
                    (3, "Ship the roadmap"),
                ]),
            )
            .unwrap();
            q.comment_fts_replace(space(2), page(2), &rows(&[(4, "Menu for the other space")]))
                .unwrap();
            q.comment_fts_replace(space(1), page(3), &rows(&[(5, "Menu in the trash")]))
                .unwrap();

            // The blank row is not written; the words are matched by prefix, case and accents folded.
            let hits = q.comment_fts_search(&search("CAFE men", None)).unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].page_id, page(1));
            assert_eq!(hits[0].comment_id, comment(1));
            assert_eq!(hits[0].snippet, "The café menu looks good");
            assert_eq!(hits[0].highlights, [(4, 8), (9, 13)]);

            // `menu` is in two live pages and one trashed one: the trashed page never answers.
            let hits = q.comment_fts_search(&search("menu", None)).unwrap();
            assert_eq!(
                hits.iter().map(|hit| hit.page_id).collect::<Vec<_>>().len(),
                2
            );
            assert!(hits.iter().all(|hit| hit.page_id != page(3)));
            // The space filter goes through the page index.
            let hits = q
                .comment_fts_search(&search("menu", Some(space(2))))
                .unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].page_id, page(2));
            // The edit time filter too.
            let mut recent = search("menu", None);
            recent.edited_after = Some(1_500);
            let hits = q.comment_fts_search(&recent).unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].page_id, page(2));
            // Any word ORs the words (and drops the short ones); titles-only finds nothing.
            let mut any = search("roadmap other", None);
            any.any_word = true;
            assert_eq!(q.comment_fts_search(&any).unwrap().len(), 2);
            assert_eq!(
                q.comment_fts_search(&search("roadmap other", None))
                    .unwrap()
                    .len(),
                0
            );
            let mut titles = search("menu", None);
            titles.titles_only = true;
            assert!(q.comment_fts_search(&titles).unwrap().is_empty());
            // A query without a word matches nothing, whatever it holds.
            assert!(
                q.comment_fts_search(&search(" \"()* ", None))
                    .unwrap()
                    .is_empty()
            );
            // The limit is honoured.
            let mut one = search("menu", None);
            one.limit = 1;
            assert_eq!(q.comment_fts_search(&one).unwrap().len(), 1);

            // Replacing a page's rows drops the old words; the other pages keep theirs.
            q.comment_fts_replace(space(1), page(1), &rows(&[(1, "Edited words")]))
                .unwrap();
            assert!(
                q.comment_fts_search(&search("café", None))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                q.comment_fts_search(&search("edited", None)).unwrap().len(),
                1
            );
            assert_eq!(
                q.comment_fts_search(&search("menu", None)).unwrap().len(),
                1
            );
            q.comment_fts_delete_page(page(1)).unwrap();
            q.comment_fts_delete_page(page(1)).unwrap();
            assert!(
                q.comment_fts_search(&search("edited", None))
                    .unwrap()
                    .is_empty()
            );
            q.comment_fts_clear().unwrap();
            assert!(
                q.comment_fts_search(&search("menu", None))
                    .unwrap()
                    .is_empty()
            );
        });
    }

    #[test]
    fn the_snippet_markers_of_the_index_never_come_from_a_comment() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1), None, 1);
            q.comment_fts_replace(
                space(1),
                page(1),
                &[CommentTextRow {
                    comment_id: CommentId::from_bytes([1; 16]),
                    text: "before \u{E000}fake\u{E001} real notes".to_owned(),
                }],
            )
            .unwrap();
            let hits = q.comment_fts_search(&search("notes", None)).unwrap();
            assert_eq!(hits[0].snippet, "before \u{FFFD}fake\u{FFFD} real notes");
            assert_eq!(hits[0].highlights, [(19, 24)]);
        });
    }

    fn notification(kind: NotificationKind, space_n: u8, at: i64) -> NewNotification {
        NewNotification {
            kind,
            space_id: space(space_n),
            page_id: Some(page(1)),
            block_id: None,
            comment_id: Some(CommentId::from_bytes([3; 16])),
            actor: account(4),
            at,
        }
    }

    #[test]
    fn the_inbox_lists_newest_first_pages_by_id_and_marks_rows_read() {
        on_db_and_tx(|q| {
            assert_eq!(q.notification_unread_count().unwrap(), 0);
            assert!(q.notification_list(10, None, false).unwrap().is_empty());
            let mut ids = Vec::new();
            for (i, kind) in [
                NotificationKind::ThreadReply,
                NotificationKind::CommentMention,
                NotificationKind::PageMention,
                NotificationKind::PersonAssigned,
            ]
            .into_iter()
            .enumerate()
            {
                ids.push(
                    q.notification_add(&notification(kind, 1, i as i64))
                        .unwrap(),
                );
            }
            assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "ids grow");
            let row = q.notification_get(ids[1]).unwrap().unwrap();
            assert_eq!(row.kind, NotificationKind::CommentMention);
            assert_eq!(row.page_id, Some(page(1)));
            assert_eq!(row.block_id, None);
            assert_eq!(row.comment_id, Some(CommentId::from_bytes([3; 16])));
            assert_eq!(row.actor, account(4));
            assert!(!row.read);
            assert!(q.notification_get(ids[3] + 100).unwrap().is_none());

            let all = q.notification_list(10, None, false).unwrap();
            assert_eq!(
                all.iter().map(|row| row.id).collect::<Vec<_>>(),
                ids.iter().rev().copied().collect::<Vec<_>>()
            );
            // Paging by the id of the last row seen.
            let first = q.notification_list(2, None, false).unwrap();
            assert_eq!(first.len(), 2);
            let second = q.notification_list(2, Some(first[1].id), false).unwrap();
            assert_eq!(
                second.iter().map(|row| row.id).collect::<Vec<_>>(),
                [ids[1], ids[0]]
            );

            assert_eq!(q.notification_unread_count().unwrap(), 4);
            assert_eq!(
                q.notification_mark_read(Some(&[ids[0], ids[0], ids[2], 12_345]))
                    .unwrap(),
                2,
                "a row counts once, an unknown id not at all"
            );
            assert_eq!(q.notification_unread_count().unwrap(), 2);
            let unread = q.notification_list(10, None, true).unwrap();
            assert_eq!(
                unread.iter().map(|row| row.id).collect::<Vec<_>>(),
                [ids[3], ids[1]]
            );
            assert_eq!(q.notification_mark_read(Some(&[ids[0]])).unwrap(), 0);
            assert_eq!(q.notification_mark_read(None).unwrap(), 2);
            assert_eq!(q.notification_mark_read(None).unwrap(), 0);
            assert_eq!(q.notification_unread_count().unwrap(), 0);
            assert_eq!(q.notification_list(10, None, false).unwrap().len(), 4);
        });
    }

    #[test]
    fn the_inbox_forgets_the_rows_of_a_space() {
        on_db_and_tx(|q| {
            q.notification_add(&notification(NotificationKind::ThreadReply, 1, 1))
                .unwrap();
            q.notification_add(&notification(NotificationKind::ThreadReply, 2, 2))
                .unwrap();
            q.notification_add(&notification(NotificationKind::PageMention, 2, 3))
                .unwrap();
            assert_eq!(q.notification_delete_space(space(2)).unwrap(), 2);
            assert_eq!(q.notification_delete_space(space(2)).unwrap(), 0);
            let left = q.notification_list(10, None, false).unwrap();
            assert_eq!(left.len(), 1);
            assert_eq!(left[0].space_id, space(1));
        });
    }

    #[test]
    fn the_inbox_is_bounded_read_rows_go_first_and_the_new_row_stays() {
        let db = Db::open_in_memory().unwrap();
        // Fill the table with rows already (in one statement: the bound is about what add keeps).
        db.connection()
            .execute(
                &format!(
                    "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {}) \
                     INSERT INTO notifications (kind, space_id, actor, at, read) \
                     SELECT 1, zeroblob(16), zeroblob(16), i, 0 FROM n",
                    NOTIFICATIONS_MAX_ROWS
                ),
                [],
            )
            .unwrap();
        assert_eq!(
            count(db.connection(), "SELECT count(*) FROM notifications") as usize,
            NOTIFICATIONS_MAX_ROWS
        );
        let total = || {
            usize::try_from(count(db.connection(), "SELECT count(*) FROM notifications")).unwrap()
        };
        // Rows 5 and 6 are read: they are the ones to go, oldest first, before the older unread rows
        // 1 to 4.
        db.notification_mark_read(Some(&[5, 6])).unwrap();
        let first = db
            .notification_add(&notification(NotificationKind::PageMention, 1, 5_000))
            .unwrap();
        assert_eq!(total(), NOTIFICATIONS_MAX_ROWS);
        assert!(db.notification_get(5).unwrap().is_none());
        assert!(db.notification_get(6).unwrap().is_some());
        assert!(db.notification_get(1).unwrap().is_some());
        let second = db
            .notification_add(&notification(NotificationKind::PageMention, 1, 5_001))
            .unwrap();
        assert_eq!(total(), NOTIFICATIONS_MAX_ROWS);
        assert!(db.notification_get(6).unwrap().is_none());
        assert!(db.notification_get(1).unwrap().is_some());
        // No read row is left: the oldest unread one goes, and the row just added never does.
        let third = db
            .notification_add(&notification(NotificationKind::PageMention, 1, 5_002))
            .unwrap();
        assert_eq!(total(), NOTIFICATIONS_MAX_ROWS);
        assert!(db.notification_get(1).unwrap().is_none());
        assert!(db.notification_get(2).unwrap().is_some());
        for id in [first, second, third] {
            assert!(db.notification_get(id).unwrap().is_some(), "{id}");
        }
    }

    #[test]
    fn person_mentions_are_replaced_per_document_and_read_in_order() {
        on_db_and_tx(|q| {
            let doc = DocId::from_bytes([1; 16]);
            let other = DocId::from_bytes([2; 16]);
            let row = |block: u8, account_n: u8| PersonMentionRow {
                block_id: BlockId::from_bytes([block; 16]),
                account_id: account(account_n),
            };
            assert!(q.person_mentions_of(doc).unwrap().is_empty());
            q.person_mentions_replace(doc, &[row(9, 1), row(3, 2), row(3, 1), row(9, 1)])
                .unwrap();
            q.person_mentions_replace(other, &[row(5, 7)]).unwrap();
            assert_eq!(
                q.person_mentions_of(doc).unwrap(),
                [row(3, 1), row(3, 2), row(9, 1)],
                "ordered, a pair once"
            );
            // A second replacement is a replacement, not an addition.
            q.person_mentions_replace(doc, &[row(4, 1)]).unwrap();
            assert_eq!(q.person_mentions_of(doc).unwrap(), [row(4, 1)]);
            q.person_mentions_replace(doc, &[]).unwrap();
            assert!(q.person_mentions_of(doc).unwrap().is_empty());
            assert_eq!(q.person_mentions_of(other).unwrap(), [row(5, 7)]);
            q.person_mentions_delete(other).unwrap();
            q.person_mentions_delete(other).unwrap();
            q.person_mentions_replace(doc, &[row(1, 1)]).unwrap();
            q.person_mentions_clear().unwrap();
            assert!(q.person_mentions_of(doc).unwrap().is_empty());
        });
    }
}
