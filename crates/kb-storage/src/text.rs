//! Link index and full-text search (schema v3, `docs/development/milestone-03.md`,
//! "Storage"). Derived data: the core rebuilds both tables from the documents.
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`] through
//! [`TextIndexQueries`], both running the same private functions over a connection; the
//! multi-statement methods of a [`Db`] run as one atomic unit.
//!
//! # The text index: `fts_blocks` and `fts`
//!
//! The text lives in the regular table `fts_blocks`: a page has one **title row**
//! (`block_id` NULL, `title` set, `body` empty) and one **block row** per text-bearing
//! block (`title` empty). Ids are the usual 16-byte blobs, and the index
//! `fts_blocks_page (page_id, block_id)` finds the rows of a page or a block, so replacing a
//! page, renaming it, reading a block or counting the title rows costs a few index lookups
//! however large the workspace is.
//!
//! **Property rows** (schema v10, ADR-0026) are a third kind of row, marked `kind = 1`: one per
//! property of a page that has searchable text (option names, or a text, URL, email or phone
//! value), with the property's id as `block_id`, the text as `body` and an empty `title`. They
//! are written and replaced on their own ([`TextIndexQueries::fts_replace_properties`]) while
//! [`TextIndexQueries::fts_replace_page`] leaves them alone, and [`TextIndexQueries::fts_delete_page`]
//! removes every kind. A search finds them like a block row (a hit with `property` set); a
//! titles-only search and the page count never see them.
//!
//! `fts` is an FTS5 table (`unicode61 remove_diacritics 2`) over the `title` and `body`
//! columns of `fts_blocks` (external content: it keeps no text of its own, only the word
//! index). Three triggers keep it in step with every insert, update and delete of
//! `fts_blocks`, so this module only ever writes `fts_blocks`. A search (`MATCH`) uses the
//! word index and reads `page_id` and `block_id` of each hit from `fts_blocks` by rowid.
//!
//! # Searching
//!
//! [`TextIndexQueries::fts_search`] never hands user text to FTS5 as it is: every word
//! becomes a quoted prefix phrase (see `match_expression`), so no input can be read as FTS5
//! syntax or fail to parse. Matches are marked in the `snippet()` output with two
//! private-use characters, which are turned into UTF-16 ranges over the marker-free text.
//! To keep those markers unambiguous, the two characters are replaced by U+FFFD in the
//! text that is indexed.
//!
//! A query matches a **page** when every word is somewhere in it (ADR-0040, which reopened open
//! question 19). The statement above finds the rows that hold every word; when they do not fill the
//! answer, the page-level pass (`spread`) adds one hit for each other page whose words are spread
//! over its rows. The fuzzy matching of titles is in the `fuzzy` module and runs in the core, on the
//! titles it knows.

mod spread;

use std::borrow::Cow;
use std::collections::HashSet;

use kb_types::{BlockId, PageId, SpaceId};
use rusqlite::{Connection, OptionalExtension, ToSql, params};

use crate::docs::{atomically, id_from};
use crate::fuzzy::is_word_char;
use crate::{Db, StorageError, Tx};

/// Longest search query the index accepts, in characters (the core validates first).
/// [`TextIndexQueries::fts_search`] ignores whatever comes after.
pub const MAX_QUERY_CHARS: usize = 200;
/// Largest `limit` of a search.
pub const MAX_SEARCH_LIMIT: u32 = 100;

/// Opens a match inside `snippet()` output (a private-use character, see the module docs).
const HIGHLIGHT_OPEN: char = '\u{E000}';
/// Closes a match inside `snippet()` output.
const HIGHLIGHT_CLOSE: char = '\u{E001}';
/// [`HIGHLIGHT_OPEN`] as the SQL argument of `snippet()`.
pub(crate) const HIGHLIGHT_OPEN_SQL: &str = "\u{E000}";
/// [`HIGHLIGHT_CLOSE`] as the SQL argument of `snippet()`.
pub(crate) const HIGHLIGHT_CLOSE_SQL: &str = "\u{E001}";
/// What `snippet()` puts where it cut the text.
pub(crate) const SNIPPET_ELLIPSIS: &str = "\u{2026}";

/// The characters of FTS5's query syntax that user text must not carry into a query. Each
/// one separates words, so `well-known` searches for `well` and `known`.
const QUERY_SYNTAX: [char; 8] = ['"', '*', '(', ')', ':', '^', '-', '+'];
/// In any-word mode a word must be longer than this many characters to be searched.
const ANY_WORD_MIN_EXCLUSIVE: usize = 2;
/// The FTS5 operators, which are only operators in capitals. A bare one is dropped from
/// the query (the words around it are searched together anyway).
const QUERY_OPERATORS: [&str; 4] = ["AND", "OR", "NOT", "NEAR"];

/// The search statement. The columns of `fts` count from 0 in `snippet()` and `bm25()`:
/// `title` 0, `body` 1.
///
/// `?1` the FTS5 query, `?2` the space id or NULL, `?3` 1 for titles only, `?4` the limit,
/// `?5` and `?6` the highlight markers, `?7` the ellipsis, `?8` the page id or NULL (only the
/// semantic index's page scope sets it, see `fts_search_on`), `?9` the oldest `updated_at` of a
/// page (Unix ms) or NULL. A title row has its text in column 0, a block row in column 1; the title
/// weighs five times the body in the rank.
///
/// `CROSS JOIN` pins the join order: the full-text match drives, each hit reads its ids from
/// `fts_blocks` by rowid, and its page is looked up by primary key in `pages_index`.
const SEARCH_SQL: &str = r"
SELECT b.page_id,
       b.block_id,
       snippet(fts, CASE WHEN b.block_id IS NULL THEN 0 ELSE 1 END, ?5, ?6, ?7, 12),
       bm25(fts, 5.0, 1.0) AS score,
       b.kind
  FROM fts
 CROSS JOIN fts_blocks AS b ON b.id = fts.rowid
 CROSS JOIN pages_index AS p ON p.page_id = b.page_id
 WHERE fts MATCH ?1
   AND p.trashed_at IS NULL
   AND p.in_template = 0
   AND (?2 IS NULL OR p.space_id = ?2)
   AND (?3 = 0 OR b.block_id IS NULL)
   AND (?8 IS NULL OR b.page_id = ?8)
   AND (?9 IS NULL OR p.updated_at >= ?9)
 ORDER BY score, b.page_id, b.block_id
 LIMIT ?4";

/// Every row of a page, of every kind. The index `fts_blocks_page` finds them; the delete
/// trigger takes each one out of the word index.
const FTS_DELETE_PAGE_SQL: &str = "DELETE FROM fts_blocks WHERE page_id = ?1";

/// The title row and the block rows of a page (`kind = 0`), which a replacement of the page's
/// text rewrites; the property rows stay.
const FTS_DELETE_TEXT_SQL: &str = "DELETE FROM fts_blocks WHERE page_id = ?1 AND kind = 0";

/// The property rows of a page (`kind = 1`).
const FTS_DELETE_PROPERTIES_SQL: &str = "DELETE FROM fts_blocks WHERE page_id = ?1 AND kind = 1";

/// The property rows of every page of a space (`fts_blocks_property_rows`, the pages of the
/// space from `pages_index_space_trashed`).
const FTS_CLEAR_PROPERTIES_SQL: &str = "DELETE FROM fts_blocks WHERE kind = 1 \
     AND page_id IN (SELECT page_id FROM pages_index WHERE space_id = ?1)";

/// A property row: the property's id as the block, its text as the body.
const FTS_INSERT_PROPERTY_SQL: &str =
    "INSERT INTO fts_blocks (page_id, block_id, title, body, kind) VALUES (?1, ?2, '', ?3, 1)";

/// The title row of a page.
const FTS_INSERT_TITLE_SQL: &str =
    "INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES (?1, NULL, ?2, '')";

/// Rows per `INSERT` statement of [`fts_replace_page_on`]. SQLite opens a statement
/// savepoint for a statement that fires triggers, and FTS5 writes out its pending words at
/// every savepoint, so a page goes in with a few statements and not with one per block.
const INSERT_ROWS_PER_STATEMENT: usize = 100;

/// Renames the title row of a page (`block_id IS NULL`, by `fts_blocks_page`).
const FTS_SET_TITLE_SQL: &str =
    "UPDATE fts_blocks SET title = ?2 WHERE page_id = ?1 AND block_id IS NULL";

/// The text of one block (`fts_blocks_page`).
const FTS_BLOCK_TEXT_SQL: &str =
    "SELECT body FROM fts_blocks WHERE page_id = ?1 AND block_id = ?2 AND kind = 0 LIMIT 1";

/// The body rows of a page in the order they were written, which is document order
/// (`fts_blocks_page`): the block rows, not the title row (`block_id` NULL) nor the property rows
/// (`kind = 1`).
const FTS_PAGE_BODY_SQL: &str = "SELECT body FROM fts_blocks \
     WHERE page_id = ?1 AND kind = 0 AND block_id IS NOT NULL ORDER BY id";

/// The pages of a space that have a title row: the pages of the space come from
/// `pages_index_space_trashed`, each one looks for its title row in `fts_blocks_page`.
const FTS_PAGE_COUNT_SQL: &str = "SELECT count(*) FROM pages_index AS p \
     WHERE p.space_id = ?1 \
       AND EXISTS (SELECT 1 FROM fts_blocks AS b WHERE b.page_id = p.page_id AND b.block_id IS NULL)";

/// Every row of every page (the delete trigger takes each one out of the word index).
const FTS_CLEAR_SQL: &str = "DELETE FROM fts_blocks";

/// Outgoing references of a page. The primary key `(from_page, from_block, to_page)` is the
/// order asked for, so no sort is needed.
const LINKS_FROM_SQL: &str = "SELECT from_block, to_page, to_block FROM links \
     WHERE from_page = ?1 ORDER BY from_block, to_page";

/// Incoming references from live pages: the `links_to` index finds the rows of the target,
/// then each source page is looked up by primary key in `pages_index`.
const LINKS_TO_SQL: &str = "SELECT l.from_page, l.from_block, l.to_block \
       FROM links AS l JOIN pages_index AS p ON p.page_id = l.from_page \
      WHERE l.to_page = ?1 AND p.trashed_at IS NULL AND p.in_template = 0 \
      ORDER BY l.from_page, l.from_block";

/// The outgoing references of a page (primary key index).
const LINKS_DELETE_FROM_SQL: &str = "DELETE FROM links WHERE from_page = ?1";

/// Every reference from and to a page, in one statement: both directions go together even
/// outside a transaction. SQLite uses the primary key for one side and `links_to` for the
/// other.
const LINKS_DELETE_PAGE_SQL: &str = "DELETE FROM links WHERE from_page = ?1 OR to_page = ?1";

/// One text-bearing block of a page, as the core extracts it from the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsBlock {
    /// Block id.
    pub block_id: BlockId,
    /// Plain text of the block (page references already resolved to titles).
    pub text: String,
}

/// A search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchParams<'a> {
    /// The user's text. The crate turns it into an FTS5 query: the first
    /// [`MAX_QUERY_CHARS`] characters are split into words at whitespace, control
    /// characters and the characters of FTS5's query syntax (`"`, `*`, `(`, `)`, `:`, `^`,
    /// `-`, `+`); a bare `AND`, `OR`, `NOT` or `NEAR` is dropped; every word is matched
    /// by prefix, ignoring case and accents. A page matches when every word matches somewhere in
    /// it (implicit AND, unless [`Self::any_word`] or [`Self::titles_only`]): see
    /// [`TextIndexQueries::fts_search`]. A query without a word matches nothing.
    pub query: &'a str,
    /// Restrict to the pages of one space.
    pub space_id: Option<SpaceId>,
    /// Match titles only.
    pub titles_only: bool,
    /// Maximum hits, 1..=[`MAX_SEARCH_LIMIT`].
    pub limit: u32,
    /// Any-word mode (ADR-0037, retrieval for the local AI): the words are OR-ed instead of AND-ed
    /// and words of 2 or fewer characters are dropped, so a question in a sentence finds the rows
    /// that share some of its words. Every word is still matched by prefix. A query with no word
    /// of 3 characters left matches nothing. The matching is by row, as it always was: the
    /// page-level pass is not run.
    pub any_word: bool,
    /// Only pages whose `updated_at` (Unix ms, `pages_index`) is this or later (ADR-0040).
    pub edited_after: Option<i64>,
}

/// One hit of [`TextIndexQueries::fts_search`].
#[derive(Debug, Clone, PartialEq)]
pub struct FtsHit {
    /// Page.
    pub page_id: PageId,
    /// Block, or `None` for a title hit. For a property hit ([`Self::property`]) the id of the
    /// property.
    pub block_id: Option<BlockId>,
    /// The hit is a property row (ADR-0026): `block_id` names a property, not a block.
    pub property: bool,
    /// Text around the match, plain, possibly shortened with an ellipsis.
    pub snippet: String,
    /// Match positions inside `snippet` as UTF-16 code unit ranges `(start, end)`.
    pub highlights: Vec<(u32, u32)>,
    /// FTS5 rank (`bm25`, more negative is better). The hits of one row that holds every word come
    /// sorted by it; the hits of a page whose words are spread over its rows follow them (see
    /// [`TextIndexQueries::fts_search`]) and carry the row's rank over the words it holds.
    pub rank: f64,
}

/// An outgoing reference of a page (`links` row without `from_page`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRow {
    /// Referencing block.
    pub from_block: BlockId,
    /// Referenced page.
    pub to_page: PageId,
    /// Referenced block, if the reference points inside the page.
    pub to_block: Option<BlockId>,
}

/// An incoming reference (`links` row seen from the referenced page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSource {
    /// Referencing page.
    pub from_page: PageId,
    /// Referencing block.
    pub from_block: BlockId,
    /// Referenced block, if any.
    pub to_block: Option<BlockId>,
}

/// Queries of the text and link indexes, available on a connection or inside a
/// transaction. On a [`Db`] every method is atomic by itself.
pub trait TextIndexQueries {
    /// Replaces the text-index rows of a page that come from its document: the title row plus
    /// one row per block, in the order given (the property rows stay, see
    /// [`Self::fts_replace_properties`]). The title row exists even when the title is empty.
    /// The two characters U+E000 and U+E001 are stored as U+FFFD (see the module docs).
    fn fts_replace_page(
        &self,
        page_id: PageId,
        title: &str,
        blocks: &[FtsBlock],
    ) -> Result<(), StorageError>;
    /// Replaces the property rows of a page (`kind = 1`) with one row per `(property id as a
    /// block id, text)` given, in that order; the title row and the block rows are untouched. A
    /// row whose text is blank is not written. The two characters U+E000 and U+E001 are stored
    /// as U+FFFD (see the module docs).
    fn fts_replace_properties(
        &self,
        page_id: PageId,
        rows: &[FtsBlock],
    ) -> Result<(), StorageError>;
    /// Removes the property rows of every page of a space (per `pages_index`), before they
    /// are written again. The rows of the documents stay.
    fn fts_clear_properties(&self, space_id: SpaceId) -> Result<(), StorageError>;
    /// Updates (or creates) the title row of a page; body rows are untouched.
    fn fts_set_title(&self, page_id: PageId, title: &str) -> Result<(), StorageError>;
    /// Removes every text-index row of a page, property rows included. Succeeds when the page
    /// has none.
    fn fts_delete_page(&self, page_id: PageId) -> Result<(), StorageError>;
    /// The indexed plain text of one block (`None` for an unknown block; a title row is
    /// not a block).
    fn fts_block_text(
        &self,
        page_id: PageId,
        block_id: BlockId,
    ) -> Result<Option<String>, StorageError>;
    /// Hands the indexed text of the blocks of a page to `visit`, one block row at a time in
    /// document order (the order the rows were written; the title row and the property rows are
    /// not blocks), until `visit` answers `false` or the rows end. The rows are read as stored,
    /// with U+FFFD where the text held a private-use highlight marker. A page with no row gets
    /// no call. The feed's excerpt (`PageExcerpts`, ADR-0058) reads the first few rows of each
    /// page this way, so a long page is not read to its end.
    fn fts_page_body(
        &self,
        page_id: PageId,
        visit: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), StorageError>;
    /// Searches titles and bodies. Trashed pages and templates (a template or a page inside one,
    /// `in_template`, ADR-0056) are excluded (per `pages_index`), and so are pages missing from it; `space_id` and `edited_after` filter through `pages_index`.
    /// `limit` is clamped to `1..=`[`MAX_SEARCH_LIMIT`]. A query that has no searchable word left
    /// after stripping returns no hits.
    ///
    /// **Page-level match** (ADR-0040; not with `any_word` or `titles_only`): a page matches when
    /// every word matches, by prefix, somewhere in the page: its title row, a block row or a property
    /// row. The hits come in two groups.
    ///
    /// 1. The rows that hold every word, as the search always found them, ordered by
    ///    `bm25(fts, 5.0, 1.0)` (a title hit weighs five body hits), then by page and block id. A page
    ///    can have several of them.
    /// 2. After them, one hit for each other page that matches, for the row of that page that holds
    ///    the most of the words (then the best `bm25` over its words, then the first in the index),
    ///    with the highlights of the words that row holds. These come ordered by the number of words in
    ///    that row (more first), then by that rank, then by page id. They only come when the first
    ///    group does not fill `limit`, and never for a page that is in it.
    ///
    /// The second group reads, for each word, the rows that match it (at most 250,000), so it costs
    /// about one full-text query per word.
    fn fts_search(&self, params: &SearchParams<'_>) -> Result<Vec<FtsHit>, StorageError>;
    /// Number of pages of a space that have a title row, trashed pages included: the pages
    /// of the space per `pages_index` (for the open-time check).
    fn fts_page_count(&self, space_id: SpaceId) -> Result<u32, StorageError>;
    /// Removes every text-index row (before a full rebuild).
    fn fts_clear(&self) -> Result<(), StorageError>;

    /// Replaces the outgoing references of a page. A block that references the same page
    /// more than once keeps the first of its rows.
    fn links_replace_from(&self, page_id: PageId, links: &[LinkRow]) -> Result<(), StorageError>;
    /// Outgoing references of a page, ordered by (block, target page).
    fn links_from(&self, page_id: PageId) -> Result<Vec<LinkRow>, StorageError>;
    /// Incoming references whose source page is live (not trashed) and not in a template per
    /// `pages_index`, ordered by (source page, block).
    fn links_to(&self, page_id: PageId) -> Result<Vec<LinkSource>, StorageError>;
    /// Removes every reference from and to a page (purge).
    fn links_delete_page(&self, page_id: PageId) -> Result<(), StorageError>;
    /// Removes every `links` row (before a full rebuild).
    fn links_clear(&self) -> Result<(), StorageError>;
}

impl TextIndexQueries for Db {
    fn fts_replace_page(
        &self,
        page_id: PageId,
        title: &str,
        blocks: &[FtsBlock],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            fts_replace_page_on(conn, page_id, title, blocks)
        })
    }
    fn fts_replace_properties(
        &self,
        page_id: PageId,
        rows: &[FtsBlock],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            fts_replace_properties_on(conn, page_id, rows)
        })
    }
    fn fts_clear_properties(&self, space_id: SpaceId) -> Result<(), StorageError> {
        fts_clear_properties_on(&self.conn, space_id)
    }
    fn fts_set_title(&self, page_id: PageId, title: &str) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| fts_set_title_on(conn, page_id, title))
    }
    fn fts_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        fts_delete_page_on(&self.conn, page_id)
    }
    fn fts_block_text(
        &self,
        page_id: PageId,
        block_id: BlockId,
    ) -> Result<Option<String>, StorageError> {
        fts_block_text_on(&self.conn, page_id, block_id)
    }
    fn fts_page_body(
        &self,
        page_id: PageId,
        visit: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), StorageError> {
        fts_page_body_on(&self.conn, page_id, visit)
    }
    fn fts_search(&self, params: &SearchParams<'_>) -> Result<Vec<FtsHit>, StorageError> {
        fts_search_on(&self.conn, params, None)
    }
    fn fts_page_count(&self, space_id: SpaceId) -> Result<u32, StorageError> {
        fts_page_count_on(&self.conn, space_id)
    }
    fn fts_clear(&self) -> Result<(), StorageError> {
        fts_clear_on(&self.conn)
    }
    fn links_replace_from(&self, page_id: PageId, links: &[LinkRow]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            links_replace_from_on(conn, page_id, links)
        })
    }
    fn links_from(&self, page_id: PageId) -> Result<Vec<LinkRow>, StorageError> {
        links_from_on(&self.conn, page_id)
    }
    fn links_to(&self, page_id: PageId) -> Result<Vec<LinkSource>, StorageError> {
        links_to_on(&self.conn, page_id)
    }
    fn links_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        links_delete_page_on(&self.conn, page_id)
    }
    fn links_clear(&self) -> Result<(), StorageError> {
        links_clear_on(&self.conn)
    }
}

impl TextIndexQueries for Tx<'_> {
    fn fts_replace_page(
        &self,
        page_id: PageId,
        title: &str,
        blocks: &[FtsBlock],
    ) -> Result<(), StorageError> {
        fts_replace_page_on(&self.tx, page_id, title, blocks)
    }
    fn fts_replace_properties(
        &self,
        page_id: PageId,
        rows: &[FtsBlock],
    ) -> Result<(), StorageError> {
        fts_replace_properties_on(&self.tx, page_id, rows)
    }
    fn fts_clear_properties(&self, space_id: SpaceId) -> Result<(), StorageError> {
        fts_clear_properties_on(&self.tx, space_id)
    }
    fn fts_set_title(&self, page_id: PageId, title: &str) -> Result<(), StorageError> {
        fts_set_title_on(&self.tx, page_id, title)
    }
    fn fts_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        fts_delete_page_on(&self.tx, page_id)
    }
    fn fts_block_text(
        &self,
        page_id: PageId,
        block_id: BlockId,
    ) -> Result<Option<String>, StorageError> {
        fts_block_text_on(&self.tx, page_id, block_id)
    }
    fn fts_page_body(
        &self,
        page_id: PageId,
        visit: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), StorageError> {
        fts_page_body_on(&self.tx, page_id, visit)
    }
    fn fts_search(&self, params: &SearchParams<'_>) -> Result<Vec<FtsHit>, StorageError> {
        fts_search_on(&self.tx, params, None)
    }
    fn fts_page_count(&self, space_id: SpaceId) -> Result<u32, StorageError> {
        fts_page_count_on(&self.tx, space_id)
    }
    fn fts_clear(&self) -> Result<(), StorageError> {
        fts_clear_on(&self.tx)
    }
    fn links_replace_from(&self, page_id: PageId, links: &[LinkRow]) -> Result<(), StorageError> {
        links_replace_from_on(&self.tx, page_id, links)
    }
    fn links_from(&self, page_id: PageId) -> Result<Vec<LinkRow>, StorageError> {
        links_from_on(&self.tx, page_id)
    }
    fn links_to(&self, page_id: PageId) -> Result<Vec<LinkSource>, StorageError> {
        links_to_on(&self.tx, page_id)
    }
    fn links_delete_page(&self, page_id: PageId) -> Result<(), StorageError> {
        links_delete_page_on(&self.tx, page_id)
    }
    fn links_clear(&self) -> Result<(), StorageError> {
        links_clear_on(&self.tx)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------
//
// Ids are bound as 16-byte blobs. Rows are read as raw column values first and validated
// afterwards, so a damaged row is a typed `InvalidData` error and never a panic or a
// silently wrong value. Content (titles, texts, queries) never appears in an error.

/// The text as it is stored in `fts_blocks`: the two private-use characters that `snippet()`
/// uses as highlight markers become U+FFFD, so a marker in a snippet always came from a
/// match.
pub(crate) fn indexable(text: &str) -> Cow<'_, str> {
    if text.contains([HIGHLIGHT_OPEN, HIGHLIGHT_CLOSE]) {
        Cow::Owned(text.replace([HIGHLIGHT_OPEN, HIGHLIGHT_CLOSE], "\u{FFFD}"))
    } else {
        Cow::Borrowed(text)
    }
}

fn fts_replace_page_on(
    conn: &Connection,
    page_id: PageId,
    title: &str,
    blocks: &[FtsBlock],
) -> Result<(), StorageError> {
    conn.execute(FTS_DELETE_TEXT_SQL, [page_id.to_bytes()])?;
    let page = page_id.to_bytes();
    let title = indexable(title);
    let ids: Vec<[u8; 16]> = blocks.iter().map(|b| b.block_id.to_bytes()).collect();
    let texts: Vec<Cow<'_, str>> = blocks.iter().map(|b| indexable(&b.text)).collect();

    // The title row leads the first statement, the blocks follow in the order given.
    let mut next = 0;
    let mut first = true;
    loop {
        let room = INSERT_ROWS_PER_STATEMENT - usize::from(first);
        let end = blocks.len().min(next + room);
        let mut values: Vec<&dyn ToSql> = vec![&page];
        if first {
            values.push(&title);
        }
        for (id, text) in ids[next..end].iter().zip(&texts[next..end]) {
            values.push(id);
            values.push(text);
        }
        conn.execute(&insert_rows_sql(first, end - next), values.as_slice())?;
        next = end;
        first = false;
        if next == blocks.len() {
            return Ok(());
        }
    }
}

/// `INSERT INTO fts_blocks ... VALUES` of an optional title row and `blocks` block rows.
/// `?1` is the page id, `?2` the title when there is one, then a (block id, text) pair per
/// block.
fn insert_rows_sql(with_title: bool, blocks: usize) -> String {
    let mut rows = Vec::with_capacity(blocks + 1);
    let mut param = 2;
    if with_title {
        rows.push("(?1, NULL, ?2, '')".to_owned());
        param = 3;
    }
    for _ in 0..blocks {
        rows.push(format!("(?1, ?{param}, '', ?{})", param + 1));
        param += 2;
    }
    format!(
        "INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES {}",
        rows.join(", ")
    )
}

fn fts_replace_properties_on(
    conn: &Connection,
    page_id: PageId,
    rows: &[FtsBlock],
) -> Result<(), StorageError> {
    conn.execute(FTS_DELETE_PROPERTIES_SQL, [page_id.to_bytes()])?;
    let mut insert = conn.prepare(FTS_INSERT_PROPERTY_SQL)?;
    for row in rows.iter().filter(|row| !row.text.trim().is_empty()) {
        insert.execute(params![
            page_id.to_bytes(),
            row.block_id.to_bytes(),
            indexable(&row.text)
        ])?;
    }
    Ok(())
}

fn fts_clear_properties_on(conn: &Connection, space_id: SpaceId) -> Result<(), StorageError> {
    conn.execute(FTS_CLEAR_PROPERTIES_SQL, [space_id.to_bytes()])?;
    Ok(())
}

fn fts_set_title_on(conn: &Connection, page_id: PageId, title: &str) -> Result<(), StorageError> {
    let title = indexable(title);
    let updated = conn.execute(FTS_SET_TITLE_SQL, params![page_id.to_bytes(), title])?;
    if updated == 0 {
        conn.execute(FTS_INSERT_TITLE_SQL, params![page_id.to_bytes(), title])?;
    }
    Ok(())
}

fn fts_delete_page_on(conn: &Connection, page_id: PageId) -> Result<(), StorageError> {
    conn.execute(FTS_DELETE_PAGE_SQL, [page_id.to_bytes()])?;
    Ok(())
}

fn fts_block_text_on(
    conn: &Connection,
    page_id: PageId,
    block_id: BlockId,
) -> Result<Option<String>, StorageError> {
    Ok(conn
        .query_row(
            FTS_BLOCK_TEXT_SQL,
            params![page_id.to_bytes(), block_id.to_bytes()],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

fn fts_page_body_on(
    conn: &Connection,
    page_id: PageId,
    visit: &mut dyn FnMut(&str) -> bool,
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(FTS_PAGE_BODY_SQL)?;
    let mut rows = stmt.query([page_id.to_bytes()])?;
    while let Some(row) = rows.next()? {
        let body: String = row.get(0)?;
        if !visit(&body) {
            break;
        }
    }
    Ok(())
}

fn fts_page_count_on(conn: &Connection, space_id: SpaceId) -> Result<u32, StorageError> {
    let count: i64 = conn.query_one(FTS_PAGE_COUNT_SQL, [space_id.to_bytes()], |row| row.get(0))?;
    u32::try_from(count)
        .map_err(|_| StorageError::InvalidData("indexed page count is out of range".to_owned()))
}

fn fts_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute(FTS_CLEAR_SQL, [])?;
    Ok(())
}

/// Raw column values of one search hit.
struct HitColumns {
    page_id: Vec<u8>,
    block_id: Option<Vec<u8>>,
    snippet: Option<String>,
    score: f64,
    kind: i64,
}

impl HitColumns {
    fn decode(self) -> Result<FtsHit, StorageError> {
        let (snippet, highlights) = decode_snippet(self.snippet.as_deref().unwrap_or_default());
        Ok(FtsHit {
            page_id: PageId::from_bytes(id_from(&self.page_id, "indexed page id")?),
            block_id: self
                .block_id
                .as_deref()
                .map(|bytes| id_from(bytes, "indexed block id").map(BlockId::from_bytes))
                .transpose()?,
            property: self.kind == 1,
            snippet,
            highlights,
            rank: self.score,
        })
    }
}

/// The search of [`TextIndexQueries::fts_search`], optionally restricted to the rows of one page
/// (`page`, used by the page scope of the semantic index's retrieval; the page must still be live,
/// as for every search).
pub(crate) fn fts_search_on(
    conn: &Connection,
    search: &SearchParams<'_>,
    page: Option<PageId>,
) -> Result<Vec<FtsHit>, StorageError> {
    let Some(expression) = match_expression(search.query, search.any_word) else {
        return Ok(Vec::new());
    };
    let limit = search.limit.clamp(1, MAX_SEARCH_LIMIT);
    let mut stmt = conn.prepare(SEARCH_SQL)?;
    let rows = stmt
        .query_map(
            params![
                expression,
                search.space_id.map(SpaceId::to_bytes),
                search.titles_only,
                i64::from(limit),
                HIGHLIGHT_OPEN_SQL,
                HIGHLIGHT_CLOSE_SQL,
                SNIPPET_ELLIPSIS,
                page.map(PageId::to_bytes),
                search.edited_after,
            ],
            |row| {
                Ok(HitColumns {
                    page_id: row.get(0)?,
                    block_id: row.get(1)?,
                    snippet: row.get(2)?,
                    score: row.get(3)?,
                    kind: row.get(4)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut hits = rows
        .into_iter()
        .map(HitColumns::decode)
        .collect::<Result<Vec<_>, _>>()?;

    // The rows that hold every word did not fill the answer: add the pages whose words are spread
    // over several rows (ADR-0040). A titles-only search has one row per page, and the any-word
    // mode (the AI's retrieval) is by row on purpose.
    let room = usize::try_from(limit)
        .unwrap_or(usize::MAX)
        .saturating_sub(hits.len());
    if room > 0 && !search.any_word && !search.titles_only {
        let words = distinct_words(query_words(search.query, false));
        if words.len() >= 2 {
            let found: HashSet<[u8; 16]> = hits.iter().map(|hit| hit.page_id.to_bytes()).collect();
            hits.extend(spread::spread_hits(
                conn, &words, search, page, &found, room,
            )?);
        }
    }
    Ok(hits)
}

/// The words of a query that the page-level pass searches one by one: a repeated word (any case) is
/// left out, and so is a word with nothing the tokenizer keeps in it (`&`, an emoji), which the
/// statement of the first pass ignores as well.
fn distinct_words(words: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    words
        .into_iter()
        .filter(|word| word.chars().any(is_word_char) && seen.insert(word.to_lowercase()))
        .collect()
}

/// The words of a query as the index searches them: the text is cut at [`MAX_QUERY_CHARS`]
/// characters, whitespace, control characters and the characters of FTS5's query syntax
/// ([`QUERY_SYNTAX`]) separate words, a bare operator in capitals ([`QUERY_OPERATORS`]) is dropped,
/// and in `any_word` mode (ADR-0037) so is every word of 2 or fewer characters. A control character
/// is removed because FTS5 reads the query as a C string: a NUL would cut it short, in the middle
/// of a quoted phrase.
pub(crate) fn query_words(query: &str, any_word: bool) -> Vec<String> {
    let separated: String = query
        .chars()
        .take(MAX_QUERY_CHARS)
        .map(|c| {
            if c.is_whitespace() || c.is_control() || QUERY_SYNTAX.contains(&c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    separated
        .split(' ')
        .filter(|word| {
            !word.is_empty()
                && !QUERY_OPERATORS.contains(word)
                && (!any_word || word.chars().count() > ANY_WORD_MIN_EXCLUSIVE)
        })
        .map(str::to_owned)
        .collect()
}

/// Turns the user's text into an FTS5 query, or `None` when no word is left.
///
/// The words are those of [`query_words`]. Every one becomes the prefix phrase `"word"*`, and the
/// phrases are joined by a space, which FTS5 reads as AND. A word that the tokenizer splits further
/// (`foo.bar`) is searched as a phrase.
///
/// In `any_word` mode (ADR-0037) the words of 2 or fewer characters are dropped and the phrases
/// are joined by `OR`, so a row that has any of the remaining words matches.
///
/// Inside double quotes FTS5 reads every character literally except the quote, so the
/// result cannot be anything but a list of phrases.
pub(crate) fn match_expression(query: &str, any_word: bool) -> Option<String> {
    let mut expression = String::new();
    for word in query_words(query, any_word) {
        if !expression.is_empty() {
            expression.push_str(if any_word { " OR " } else { " " });
        }
        push_prefix_phrase(&mut expression, &word);
    }
    (!expression.is_empty()).then_some(expression)
}

/// Appends `"word"*` to an FTS5 query, doubling any quote inside the word.
fn push_prefix_phrase(expression: &mut String, word: &str) {
    expression.push('"');
    for c in word.chars() {
        if c == '"' {
            expression.push('"');
        }
        expression.push(c);
    }
    expression.push_str("\"*");
}

/// Splits `snippet()` output into the plain snippet and the highlighted ranges, as UTF-16
/// code unit offsets into that plain text (what JavaScript indexes by).
///
/// Never fails: a closing marker without an opening one is ignored, a second opening
/// marker inside a highlight is ignored, and a highlight that is still open at the end
/// runs to the end of the text. Empty highlights are dropped.
pub(crate) fn decode_snippet(raw: &str) -> (String, Vec<(u32, u32)>) {
    let mut text = String::with_capacity(raw.len());
    let mut highlights = Vec::new();
    let mut offset: u32 = 0;
    let mut open: Option<u32> = None;
    for c in raw.chars() {
        match c {
            HIGHLIGHT_OPEN => {
                open.get_or_insert(offset);
            }
            HIGHLIGHT_CLOSE => {
                if let Some(start) = open.take().filter(|&start| start < offset) {
                    highlights.push((start, offset));
                }
            }
            _ => {
                text.push(c);
                offset = offset.saturating_add(utf16_len(c));
            }
        }
    }
    if let Some(start) = open.filter(|&start| start < offset) {
        highlights.push((start, offset));
    }
    (text, highlights)
}

/// Number of UTF-16 code units of a character: 2 outside the basic multilingual plane.
fn utf16_len(c: char) -> u32 {
    if c.len_utf16() == 2 { 2 } else { 1 }
}

fn links_replace_from_on(
    conn: &Connection,
    page_id: PageId,
    links: &[LinkRow],
) -> Result<(), StorageError> {
    conn.execute(LINKS_DELETE_FROM_SQL, [page_id.to_bytes()])?;
    if links.is_empty() {
        return Ok(());
    }
    // A block that mentions one page twice has one row: the first reference wins.
    let mut insert = conn.prepare(
        "INSERT INTO links (from_page, from_block, to_page, to_block) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (from_page, from_block, to_page) DO NOTHING",
    )?;
    for link in links {
        insert.execute(params![
            page_id.to_bytes(),
            link.from_block.to_bytes(),
            link.to_page.to_bytes(),
            link.to_block.map(BlockId::to_bytes),
        ])?;
    }
    Ok(())
}

fn links_from_on(conn: &Connection, page_id: PageId) -> Result<Vec<LinkRow>, StorageError> {
    let mut stmt = conn.prepare(LINKS_FROM_SQL)?;
    let rows = stmt
        .query_map([page_id.to_bytes()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(from_block, to_page, to_block)| {
            Ok(LinkRow {
                from_block: BlockId::from_bytes(id_from(&from_block, "link source block id")?),
                to_page: PageId::from_bytes(id_from(&to_page, "link target page id")?),
                to_block: to_block
                    .as_deref()
                    .map(|bytes| id_from(bytes, "link target block id").map(BlockId::from_bytes))
                    .transpose()?,
            })
        })
        .collect()
}

fn links_to_on(conn: &Connection, page_id: PageId) -> Result<Vec<LinkSource>, StorageError> {
    let mut stmt = conn.prepare(LINKS_TO_SQL)?;
    let rows = stmt
        .query_map([page_id.to_bytes()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(from_page, from_block, to_block)| {
            Ok(LinkSource {
                from_page: PageId::from_bytes(id_from(&from_page, "link source page id")?),
                from_block: BlockId::from_bytes(id_from(&from_block, "link source block id")?),
                to_block: to_block
                    .as_deref()
                    .map(|bytes| id_from(bytes, "link target block id").map(BlockId::from_bytes))
                    .transpose()?,
            })
        })
        .collect()
}

fn links_delete_page_on(conn: &Connection, page_id: PageId) -> Result<(), StorageError> {
    conn.execute(LINKS_DELETE_PAGE_SQL, [page_id.to_bytes()])?;
    Ok(())
}

fn links_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM links", [])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{DocQueries, PageIndexRow};

    /// What a scenario needs of a connection or a transaction: both implement both traits.
    trait Queries: DocQueries + TextIndexQueries {}
    impl<T: DocQueries + TextIndexQueries> Queries for T {}

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
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

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so
    /// that both `TextIndexQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
    }

    fn page_row(n: u8, space_id: SpaceId, title: &str, trashed_at: Option<i64>) -> PageIndexRow {
        PageIndexRow {
            page_id: page(n),
            space_id,
            parent_id: None,
            title: title.to_owned(),
            trashed_at,
            updated_at: 100 + i64::from(n),
            in_template: false,
        }
    }

    /// A live page of `space_id` in `pages_index` (search joins against it).
    fn add_page(q: &dyn Queries, n: u8, space_id: SpaceId) {
        q.pages_index_upsert(&[page_row(n, space_id, "page", None)])
            .unwrap();
    }

    fn trash_page(q: &dyn Queries, n: u8, space_id: SpaceId) {
        q.pages_index_upsert(&[page_row(n, space_id, "page", Some(9))])
            .unwrap();
    }

    fn blocks(list: &[(u8, &str)]) -> Vec<FtsBlock> {
        list.iter()
            .map(|(n, text)| FtsBlock {
                block_id: block(*n),
                text: (*text).to_owned(),
            })
            .collect()
    }

    /// Indexes page `n`: its title row and the blocks `(block number, text)`.
    fn index(q: &dyn Queries, n: u8, title: &str, list: &[(u8, &str)]) {
        q.fts_replace_page(page(n), title, &blocks(list)).unwrap();
    }

    fn params(query: &str) -> SearchParams<'_> {
        SearchParams {
            query,
            space_id: None,
            titles_only: false,
            limit: 50,
            any_word: false,
            edited_after: None,
        }
    }

    fn search(q: &dyn Queries, query: &str) -> Vec<FtsHit> {
        q.fts_search(&params(query)).unwrap()
    }

    /// `(page, block)` of every hit, in the order of the hits.
    fn ids(hits: &[FtsHit]) -> Vec<(PageId, Option<BlockId>)> {
        hits.iter().map(|h| (h.page_id, h.block_id)).collect()
    }

    /// `(page, block)` of every hit, sorted (for results whose order is not the point).
    fn sorted_ids(hits: &[FtsHit]) -> Vec<(PageId, Option<BlockId>)> {
        let mut found = ids(hits);
        found.sort();
        found
    }

    /// The highlighted parts of a hit's snippet, cut by UTF-16 offsets the way a
    /// JavaScript client indexes a string.
    fn highlighted(hit: &FtsHit) -> Vec<String> {
        let units: Vec<u16> = hit.snippet.encode_utf16().collect();
        hit.highlights
            .iter()
            .map(|&(start, end)| {
                let (start, end) = (
                    usize::try_from(start).unwrap(),
                    usize::try_from(end).unwrap(),
                );
                String::from_utf16(&units[start..end]).unwrap()
            })
            .collect()
    }

    /// Number of rows of a raw table or query source.
    fn count(db: &Db, from: &str) -> i64 {
        db.connection()
            .query_row(&format!("SELECT count(*) FROM {from}"), [], |r| r.get(0))
            .unwrap()
    }

    fn link(from_block: u8, to_page: u8, to_block: Option<u8>) -> LinkRow {
        LinkRow {
            from_block: block(from_block),
            to_page: page(to_page),
            to_block: to_block.map(block),
        }
    }

    fn source(from_page: u8, from_block: u8, to_block: Option<u8>) -> LinkSource {
        LinkSource {
            from_page: page(from_page),
            from_block: block(from_block),
            to_block: to_block.map(block),
        }
    }

    // ---- property rows (schema v10, ADR-0026) ---------------------------------------------

    /// A property row for the property with the id bytes `n`.
    fn property_rows(list: &[(u8, &str)]) -> Vec<FtsBlock> {
        blocks(list)
    }

    #[test]
    fn property_rows_are_replaced_without_touching_the_rows_of_the_document() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Plan", &[(1, "block text")]);
            q.fts_replace_properties(
                page(1),
                &property_rows(&[(50, "urgent, work"), (51, "Ada")]),
            )
            .unwrap();
            assert_eq!(
                sorted_ids(&search(q, "urgent")),
                [(page(1), Some(block(50)))]
            );
            assert!(search(q, "urgent")[0].property);
            assert_eq!(search(q, "ada").len(), 1);

            // Rewriting the document's rows keeps the property rows.
            index(q, 1, "Plan 2", &[(2, "another block")]);
            assert_eq!(search(q, "urgent").len(), 1, "still found");
            assert_eq!(search(q, "another").len(), 1);
            assert!(!search(q, "another")[0].property);
            assert!(
                search(q, "block").len() == 1,
                "the old block is gone: {:?}",
                search(q, "block")
            );

            // Replacing the property rows keeps the document's.
            q.fts_replace_properties(page(1), &property_rows(&[(50, "later")]))
                .unwrap();
            assert!(search(q, "urgent").is_empty());
            assert!(search(q, "ada").is_empty());
            assert_eq!(search(q, "later").len(), 1);
            assert_eq!(search(q, "another").len(), 1);
            assert_eq!(search(q, "plan").len(), 1, "the title row stays");

            // No rows, blank rows: nothing left.
            q.fts_replace_properties(page(1), &property_rows(&[(50, "  ")]))
                .unwrap();
            assert!(search(q, "later").is_empty());
            q.fts_replace_properties(page(1), &[]).unwrap();
        });
    }

    /// The body rows `fts_page_body` hands over for page `n`, all of them.
    fn body_of(q: &dyn Queries, n: u8) -> Vec<String> {
        let mut rows = Vec::new();
        q.fts_page_body(page(n), &mut |text| {
            rows.push(text.to_owned());
            true
        })
        .unwrap();
        rows
    }

    #[test]
    fn the_page_body_is_the_block_rows_in_document_order() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            // Block ids do not sort in document order: the order is the order written.
            index(
                q,
                1,
                "Plan",
                &[(9, "first"), (1, "second"), (5, ""), (3, "third")],
            );
            index(q, 2, "Other", &[(1, "elsewhere")]);
            q.fts_replace_properties(page(1), &property_rows(&[(50, "urgent")]))
                .unwrap();
            assert_eq!(
                body_of(q, 1),
                ["first", "second", "", "third"],
                "no title row, no property row, the empty block row stays for the caller to skip"
            );
            assert_eq!(body_of(q, 2), ["elsewhere"]);
            assert!(body_of(q, 3).is_empty(), "an unknown page has no row");

            // A replacement rewrites the rows and keeps the new order.
            index(q, 1, "Plan", &[(2, "only")]);
            assert_eq!(body_of(q, 1), ["only"]);
        });
    }

    #[test]
    fn the_page_body_stops_when_the_visitor_says_so() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Plan", &[(1, "a"), (2, "b"), (3, "c"), (4, "d")]);
            let mut seen = Vec::new();
            q.fts_page_body(page(1), &mut |text| {
                seen.push(text.to_owned());
                seen.len() < 2
            })
            .unwrap();
            assert_eq!(seen, ["a", "b"], "the third row is never handed over");
        });
    }

    #[test]
    fn property_rows_never_count_as_titles_or_pages_or_blocks() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Plan", &[(1, "block text")]);
            q.fts_replace_properties(page(1), &property_rows(&[(50, "urgent")]))
                .unwrap();
            let titles = q
                .fts_search(&SearchParams {
                    titles_only: true,
                    ..params("urgent")
                })
                .unwrap();
            assert!(
                titles.is_empty(),
                "a titles-only search ignores property rows"
            );
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 1);
            assert_eq!(
                q.fts_block_text(page(1), block(50)).unwrap(),
                None,
                "a property is not a block"
            );
            // A page with property rows only still has no title row until it gets one.
            add_page(q, 2, space(1));
            q.fts_replace_properties(page(2), &property_rows(&[(50, "orphan")]))
                .unwrap();
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 1);
            q.fts_set_title(page(2), "Two").unwrap();
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 2);
            assert_eq!(search(q, "orphan").len(), 1);
            assert_eq!(search(q, "two").len(), 1);
        });
    }

    #[test]
    fn property_hits_have_a_snippet_highlights_and_follow_the_page_rules() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(2));
            add_page(q, 3, space(1));
            for n in [1, 2, 3] {
                index(q, n, "page", &[]);
                q.fts_replace_properties(page(n), &property_rows(&[(50, "Project urgent, work")]))
                    .unwrap();
            }
            trash_page(q, 3, space(1));
            let hits = search(q, "urg");
            assert_eq!(
                sorted_ids(&hits),
                [(page(1), Some(block(50))), (page(2), Some(block(50)))],
                "a prefix finds it, a trashed page does not"
            );
            assert_eq!(hits[0].snippet, "Project urgent, work");
            assert_eq!(highlighted(&hits[0]), ["urgent"]);
            let scoped = q
                .fts_search(&SearchParams {
                    space_id: Some(space(2)),
                    ..params("urgent")
                })
                .unwrap();
            assert_eq!(ids(&scoped), [(page(2), Some(block(50)))]);
            // Two properties are two rows: no row has both words, but the page does (one hit).
            q.fts_replace_properties(page(1), &property_rows(&[(50, "urgent"), (51, "work")]))
                .unwrap();
            let found = search(q, "urgent work");
            assert_eq!(
                found.iter().map(|hit| hit.page_id).collect::<Vec<_>>(),
                [page(2), page(1)],
                "page 2 has a row with both words, page 1 has them in two rows: after it, one hit"
            );
            assert!(found[1].property);
            assert_eq!(
                highlighted(&found[1]).len(),
                1,
                "only the word the row holds"
            );
        });
    }

    #[test]
    fn delete_page_removes_property_rows_and_clear_removes_everything() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            for n in [1, 2] {
                index(q, n, "page", &[(1, "text")]);
                q.fts_replace_properties(page(n), &property_rows(&[(50, "tagged")]))
                    .unwrap();
            }
            q.fts_delete_page(page(1)).unwrap();
            assert_eq!(ids(&search(q, "tagged")), [(page(2), Some(block(50)))]);
            q.fts_clear().unwrap();
            assert!(search(q, "tagged").is_empty());
        });
    }

    #[test]
    fn clear_properties_removes_the_property_rows_of_one_space_only() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(2));
            for n in [1, 2] {
                index(q, n, "page", &[(1, "text")]);
                q.fts_replace_properties(page(n), &property_rows(&[(50, "tagged")]))
                    .unwrap();
            }
            q.fts_clear_properties(space(1)).unwrap();
            assert_eq!(ids(&search(q, "tagged")), [(page(2), Some(block(50)))]);
            assert_eq!(search(q, "text").len(), 2, "the rows of the documents stay");
            q.fts_clear_properties(space(2)).unwrap();
            assert!(search(q, "tagged").is_empty());
        });
    }

    #[test]
    fn marker_characters_in_a_property_text_are_replaced() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            q.fts_replace_properties(
                page(1),
                &property_rows(&[(50, "fake \u{E000}marker\u{E001} urgent")]),
            )
            .unwrap();
            let hits = search(q, "urgent");
            assert_eq!(
                highlighted(&hits[0]),
                ["urgent"],
                "only the match is marked"
            );
            assert!(hits[0].snippet.contains('\u{FFFD}'));
        });
    }

    // ---- fts_replace_page ----------------------------------------------------------

    #[test]
    fn replace_page_writes_a_title_row_then_one_row_per_block() {
        let db = new_db();
        index(
            &db,
            1,
            "My title",
            &[(7, "first block"), (3, "second block")],
        );

        type Row = (String, i64, String, Option<i64>, String, String);
        let rows: Vec<Row> = db
            .connection()
            .prepare(
                "SELECT typeof(page_id), length(page_id), typeof(block_id), length(block_id), \
                        title, body FROM fts_blocks ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let row = |block_type: &str, block_len, title: &str, body: &str| -> Row {
            (
                "blob".to_owned(),
                16,
                block_type.to_owned(),
                block_len,
                title.to_owned(),
                body.to_owned(),
            )
        };
        assert_eq!(
            rows,
            [
                row("null", None, "My title", ""),
                row("blob", Some(16), "", "first block"),
                row("blob", Some(16), "", "second block"),
            ]
        );

        // The blobs are the id bytes themselves, so ids read back as they were written.
        let ids: Vec<(Vec<u8>, Option<Vec<u8>>)> = db
            .connection()
            .prepare("SELECT page_id, block_id FROM fts_blocks ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            ids,
            [
                (vec![1; 16], None),
                (vec![1; 16], Some(vec![7; 16])),
                (vec![1; 16], Some(vec![3; 16])),
            ]
        );
    }

    #[test]
    fn replace_page_removes_every_old_row_of_that_page_and_only_that_page() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Old title", &[(1, "alpha one"), (2, "alpha two")]);
            index(q, 2, "Other", &[(3, "alpha three")]);
            assert_eq!(search(q, "alpha").len(), 3);

            index(q, 1, "New title", &[(4, "beta four")]);

            assert_eq!(q.fts_block_text(page(1), block(1)).unwrap(), None);
            assert_eq!(q.fts_block_text(page(1), block(2)).unwrap(), None);
            assert_eq!(
                q.fts_block_text(page(1), block(4)).unwrap().as_deref(),
                Some("beta four")
            );
            assert_eq!(
                q.fts_block_text(page(2), block(3)).unwrap().as_deref(),
                Some("alpha three"),
                "the other page is untouched"
            );
            assert_eq!(ids(&search(q, "alpha")), [(page(2), Some(block(3)))]);
            assert!(search(q, "old").is_empty(), "the old title row is gone");
            assert_eq!(ids(&search(q, "new")), [(page(1), None)]);
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 2, "one title row each");
        });
    }

    #[test]
    fn replacing_a_page_twice_with_the_same_content_leaves_the_same_rows() {
        let db = new_db();
        for _ in 0..3 {
            index(&db, 1, "Title", &[(1, "one"), (2, "two")]);
            assert_eq!(count(&db, "fts_blocks"), 3);
        }
    }

    #[test]
    fn replace_page_with_no_blocks_or_an_empty_title_still_writes_the_title_row() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Only a title", &[]);
            index(q, 2, "", &[(1, "body of an untitled page")]);
            assert_eq!(
                q.fts_page_count(space(1)).unwrap(),
                2,
                "the untitled page counts as indexed"
            );
            assert_eq!(ids(&search(q, "title")), [(page(1), None)]);
            assert_eq!(ids(&search(q, "untitled")), [(page(2), Some(block(1)))]);
            assert_eq!(
                q.fts_block_text(page(2), block(1)).unwrap().as_deref(),
                Some("body of an untitled page")
            );
        });
    }

    #[test]
    fn replace_page_keeps_the_block_order_of_the_argument() {
        let db = new_db();
        index(&db, 1, "T", &[(9, "nine"), (1, "one"), (5, "five")]);
        let bodies: Vec<String> = db
            .connection()
            .prepare("SELECT body FROM fts_blocks WHERE block_id IS NOT NULL ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(bodies, ["nine", "one", "five"]);
    }

    /// A block id for any index, not only the 255 of `block(n)`.
    fn wide_block(i: usize) -> BlockId {
        let mut bytes = [7u8; 16];
        bytes[..8].copy_from_slice(&u64::try_from(i).unwrap().to_be_bytes());
        BlockId::from_bytes(bytes)
    }

    fn numbered_blocks(count: usize) -> Vec<FtsBlock> {
        (0..count)
            .map(|i| FtsBlock {
                block_id: wide_block(i),
                text: format!("text number {i}"),
            })
            .collect()
    }

    #[test]
    fn a_page_of_any_size_is_written_whole_across_the_insert_statements() {
        // A page goes in with one INSERT per `INSERT_ROWS_PER_STATEMENT` rows (the title row
        // leads the first one): every size around the chunk edges, growing and shrinking.
        assert_eq!(
            INSERT_ROWS_PER_STATEMENT, 100,
            "the sizes below are for 100"
        );
        let sizes = [
            0usize, 1, 2, 98, 99, 100, 101, 198, 199, 200, 201, 299, 300, 1_000, 300, 101, 100, 99,
            1, 0, 250,
        ];
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 2, "Neighbour", &[(1, "text of the neighbour")]);
            for count in sizes {
                q.fts_replace_page(page(1), "A title", &numbered_blocks(count))
                    .unwrap();
                for i in 0..count {
                    assert_eq!(
                        q.fts_block_text(page(1), wide_block(i)).unwrap().as_deref(),
                        Some(format!("text number {i}").as_str()),
                        "{count} blocks, block {i}"
                    );
                }
                assert_eq!(
                    q.fts_block_text(page(1), wide_block(count)).unwrap(),
                    None,
                    "{count} blocks: no row beyond the last, whatever the page held before"
                );
                assert_eq!(q.fts_page_count(space(1)).unwrap(), 2, "{count} blocks");
                assert_eq!(
                    ids(&search(q, "title")),
                    [(page(1), None)],
                    "{count} blocks: one title row"
                );
            }
            assert_eq!(
                q.fts_block_text(page(2), block(1)).unwrap().as_deref(),
                Some("text of the neighbour"),
                "the other page is untouched"
            );
        });
    }

    #[test]
    fn the_rows_of_a_large_page_are_stored_in_the_order_given() {
        let db = new_db();
        db.fts_replace_page(page(1), "Title", &numbered_blocks(250))
            .unwrap();
        let bodies: Vec<String> = db
            .connection()
            .prepare("SELECT body FROM fts_blocks ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let expected: Vec<String> = std::iter::once(String::new())
            .chain((0..250).map(|i| format!("text number {i}")))
            .collect();
        assert_eq!(
            bodies, expected,
            "the title row first, then every block in order"
        );
    }

    #[test]
    fn the_insert_statement_of_a_page_numbers_its_parameters() {
        let head = "INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES ";
        assert_eq!(
            insert_rows_sql(true, 0),
            format!("{head}(?1, NULL, ?2, '')")
        );
        assert_eq!(
            insert_rows_sql(true, 2),
            format!("{head}(?1, NULL, ?2, ''), (?1, ?3, '', ?4), (?1, ?5, '', ?6)")
        );
        assert_eq!(
            insert_rows_sql(false, 2),
            format!("{head}(?1, ?2, '', ?3), (?1, ?4, '', ?5)")
        );
    }

    #[test]
    fn private_use_markers_in_content_are_replaced_so_they_cannot_fake_a_highlight() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(
                q,
                1,
                "Title \u{E000}x\u{E001}",
                &[(1, "before \u{E000}fake\u{E001} real notes")],
            );
            assert_eq!(
                q.fts_block_text(page(1), block(1)).unwrap().as_deref(),
                Some("before \u{FFFD}fake\u{FFFD} real notes")
            );
            let hits = search(q, "notes");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].snippet, "before \u{FFFD}fake\u{FFFD} real notes");
            assert_eq!(highlighted(&hits[0]), ["notes"], "only the real match");

            let title_hits = search(q, "title");
            assert_eq!(title_hits[0].snippet, "Title \u{FFFD}x\u{FFFD}");
            assert_eq!(highlighted(&title_hits[0]), ["Title"]);

            // Same for a title that arrives through `fts_set_title`.
            q.fts_set_title(page(1), "Again \u{E001}\u{E000} again")
                .unwrap();
            let again = search(q, "again");
            assert_eq!(again[0].snippet, "Again \u{FFFD}\u{FFFD} again");
            assert_eq!(highlighted(&again[0]), ["Again", "again"]);
        });
    }

    #[test]
    fn odd_text_is_stored_and_read_back_unchanged() {
        on_db_and_tx(|q| {
            let text = "line one\nline two\ttab \u{0} nul \u{1F389} \u{202E}rtl\u{202C} \
                        caf\u{e9} \u{65e5}\u{672c}\u{8a9e} \u{1D518}";
            add_page(q, 1, space(1));
            index(q, 1, "T", &[(1, text), (2, "")]);
            assert_eq!(
                q.fts_block_text(page(1), block(1)).unwrap().as_deref(),
                Some(text)
            );
            assert_eq!(
                q.fts_block_text(page(1), block(2)).unwrap().as_deref(),
                Some(""),
                "an empty block is stored as given"
            );
        });
    }

    // ---- fts_set_title -------------------------------------------------------------

    #[test]
    fn set_title_updates_the_title_row_and_leaves_the_block_rows() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Old name", &[(1, "keep me")]);
            index(q, 2, "Old name", &[(2, "other page")]);

            q.fts_set_title(page(1), "New name").unwrap();

            assert_eq!(ids(&search(q, "new")), [(page(1), None)]);
            assert_eq!(
                ids(&search(q, "old")),
                [(page(2), None)],
                "only the other page has the old title"
            );
            assert_eq!(
                q.fts_block_text(page(1), block(1)).unwrap().as_deref(),
                Some("keep me")
            );
            assert_eq!(
                q.fts_page_count(space(1)).unwrap(),
                2,
                "the title row was updated, not added"
            );
            assert_eq!(search(q, "name").len(), 2);
        });
    }

    #[test]
    fn set_title_creates_the_title_row_when_it_is_missing() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 0);
            q.fts_set_title(page(1), "Brand new").unwrap();
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 1);
            assert_eq!(ids(&search(q, "brand")), [(page(1), None)]);

            // Setting it again updates it.
            q.fts_set_title(page(1), "Renamed").unwrap();
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 1);
            assert!(search(q, "brand").is_empty());
            assert_eq!(ids(&search(q, "renamed")), [(page(1), None)]);
        });
    }

    #[test]
    fn set_title_adds_the_title_row_to_a_page_that_has_only_block_rows() {
        let db = new_db();
        add_page(&db, 0, space(1));
        db.connection()
            .execute(
                "INSERT INTO fts_blocks (page_id, block_id, title, body) \
                 VALUES (zeroblob(16), zeroblob(16), '', 'a lonely block')",
                [],
            )
            .unwrap();
        assert_eq!(
            db.fts_page_count(space(1)).unwrap(),
            0,
            "a block row is not a title row: the page does not count as indexed"
        );
        db.fts_set_title(PageId::from_bytes([0; 16]), "Fresh title")
            .unwrap();
        assert_eq!(count(&db, "fts_blocks"), 2);
        assert_eq!(db.fts_page_count(space(1)).unwrap(), 1);
        assert_eq!(
            db.fts_block_text(PageId::from_bytes([0; 16]), BlockId::from_bytes([0; 16]))
                .unwrap()
                .as_deref(),
            Some("a lonely block")
        );
    }

    // ---- fts_delete_page and fts_clear ---------------------------------------------

    #[test]
    fn delete_page_removes_the_title_and_block_rows_of_that_page_only() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(
                q,
                1,
                "Gone soon",
                &[(1, "doomed text"), (2, "more doomed text")],
            );
            index(q, 2, "Stays", &[(3, "doomed but kept")]);

            q.fts_delete_page(page(1)).unwrap();
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 1);
            assert_eq!(q.fts_block_text(page(1), block(1)).unwrap(), None);
            assert!(search(q, "gone").is_empty());
            assert_eq!(ids(&search(q, "doomed")), [(page(2), Some(block(3)))]);

            // Deleting again, or deleting a page that was never indexed, succeeds.
            q.fts_delete_page(page(1)).unwrap();
            q.fts_delete_page(page(99)).unwrap();
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 1);
        });
    }

    #[test]
    fn clear_removes_every_row_and_the_index_can_be_filled_again() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(2));
            index(q, 1, "One", &[(1, "word")]);
            index(q, 2, "Two", &[(2, "word")]);
            assert_eq!(search(q, "word").len(), 2);

            q.fts_clear().unwrap();
            assert!(search(q, "word").is_empty());
            assert!(search(q, "one").is_empty());
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 0);
            assert_eq!(q.fts_page_count(space(2)).unwrap(), 0);
            assert_eq!(q.fts_block_text(page(1), block(1)).unwrap(), None);
            q.fts_clear().unwrap();

            index(q, 1, "One again", &[(1, "word again")]);
            assert_eq!(ids(&search(q, "word")), [(page(1), Some(block(1)))]);
        });
    }

    // ---- fts_block_text ------------------------------------------------------------

    #[test]
    fn block_text_is_the_indexed_text_of_exactly_that_block() {
        on_db_and_tx(|q| {
            index(q, 1, "T1", &[(1, "first"), (2, "second")]);
            index(q, 2, "T2", &[(3, "third")]);
            let text = |p, b| q.fts_block_text(page(p), block(b)).unwrap();
            assert_eq!(text(1, 1).as_deref(), Some("first"));
            assert_eq!(text(1, 2).as_deref(), Some("second"));
            assert_eq!(text(2, 3).as_deref(), Some("third"));
            assert_eq!(text(1, 3), None, "block 3 belongs to page 2");
            assert_eq!(text(2, 1), None, "block 1 belongs to page 1");
            assert_eq!(text(1, 99), None, "unknown block");
            assert_eq!(text(99, 1), None, "unknown page");
        });
    }

    // ---- fts_page_count ------------------------------------------------------------

    #[test]
    fn page_count_counts_the_title_rows_of_the_pages_of_that_space() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            add_page(q, 3, space(1)); // trashed below
            add_page(q, 4, space(2));
            add_page(q, 5, space(1)); // never indexed
            trash_page(q, 3, space(1));
            index(q, 1, "One", &[(1, "a"), (2, "b"), (3, "c")]);
            index(q, 2, "Two", &[]);
            index(q, 3, "Three", &[(4, "d")]);
            index(q, 4, "Four", &[(5, "e")]);
            index(q, 6, "Six", &[(6, "f")]); // indexed, but not in pages_index

            assert_eq!(
                q.fts_page_count(space(1)).unwrap(),
                3,
                "trashed included, body rows and missing pages not"
            );
            assert_eq!(q.fts_page_count(space(2)).unwrap(), 1);
            assert_eq!(q.fts_page_count(space(3)).unwrap(), 0);

            q.fts_delete_page(page(2)).unwrap();
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 2);
            q.fts_clear().unwrap();
            assert_eq!(q.fts_page_count(space(2)).unwrap(), 0);
        });
    }

    #[test]
    fn page_count_follows_the_page_to_the_space_it_moved_to() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Mover", &[]);
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 1);
            add_page(q, 1, space(2));
            assert_eq!(q.fts_page_count(space(1)).unwrap(), 0);
            assert_eq!(q.fts_page_count(space(2)).unwrap(), 1);
        });
    }

    // ---- fts_search ----------------------------------------------------------------

    #[test]
    fn search_finds_titles_and_blocks_with_a_snippet_and_highlights() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Meeting notes", &[(1, "Remember the milk")]);
            index(
                q,
                2,
                "Groceries",
                &[(2, "Buy milk and eggs"), (3, "Call mum")],
            );

            let hits = search(q, "milk");
            assert_eq!(
                sorted_ids(&hits),
                [(page(1), Some(block(1))), (page(2), Some(block(2)))]
            );
            for hit in &hits {
                assert_eq!(highlighted(hit), ["milk"]);
                assert!(hit.rank <= 0.0, "bm25 is never positive: {hit:?}");
            }
            let first = hits.iter().find(|h| h.block_id == Some(block(1))).unwrap();
            assert_eq!(first.snippet, "Remember the milk");
            assert_eq!(first.highlights, [(13, 17)]);

            let titles = search(q, "meeting");
            assert_eq!(titles.len(), 1);
            assert_eq!(titles[0].page_id, page(1));
            assert_eq!(titles[0].block_id, None, "a title hit has no block");
            assert_eq!(titles[0].snippet, "Meeting notes");
            assert_eq!(titles[0].highlights, [(0, 7)]);

            assert!(search(q, "absent").is_empty());
        });
    }

    #[test]
    fn search_matches_prefixes_ignoring_case() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Meeting Notes", &[(1, "Keep the NOTEBOOK handy")]);

            for query in [
                "not", "NoT", "Notes", "notes", "note", "n", "NoTeBoOk", "notebook",
            ] {
                let hits = search(q, query);
                assert!(!hits.is_empty(), "{query}");
            }
            assert_eq!(highlighted(&search(q, "not")[0]).len(), 1);
            assert!(search(q, "otes").is_empty(), "prefix, not substring");
            assert!(search(q, "notess").is_empty());

            // The prefix is on every word.
            assert_eq!(
                sorted_ids(&search(q, "meet no")),
                [(page(1), None)],
                "only the title row has both"
            );
        });
    }

    #[test]
    fn a_row_that_holds_every_word_is_a_hit_of_its_own() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(
                q,
                1,
                "Apple pie",
                &[
                    (1, "a red apple"),
                    (2, "a green apple"),
                    (3, "red wine"),
                    (4, "pie"),
                ],
            );

            assert_eq!(
                ids(&search(q, "red apple")),
                [(page(1), Some(block(1)))],
                "both words, one row: the page has no other hit"
            );
            assert_eq!(
                ids(&search(q, "apple red")),
                [(page(1), Some(block(1)))],
                "in any order"
            );
            assert_eq!(search(q, "apple").len(), 3, "title and two blocks");
            assert_eq!(
                ids(&search(q, "apple pie")),
                [(page(1), None)],
                "the title row holds both"
            );
        });
    }

    #[test]
    fn a_title_hit_ranks_above_a_body_hit_for_the_same_word() {
        on_db_and_tx(|q| {
            // Enough other rows for the word to be rare (bm25 floors the weight of a word
            // that is in more than half of the rows).
            for n in 10..30 {
                add_page(q, n, space(1));
                index(
                    q,
                    n,
                    &format!("Filler {n}"),
                    &[(n, "unrelated gardening text")],
                );
            }
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            // The body hit has the lower page id, was indexed first and has a row as short
            // as the title: only the weight of the title can put the title hit first.
            index(q, 1, "Savanna", &[(1, "zebra herd")]);
            index(q, 2, "Zebra crossing", &[]);

            let hits = search(q, "zebra");
            assert_eq!(
                ids(&hits),
                [(page(2), None), (page(1), Some(block(1)))],
                "the title hit comes first"
            );
            assert!(hits[0].rank < hits[1].rank, "{hits:?}");
            assert!(hits[0].rank < 0.0);
        });
    }

    #[test]
    fn hits_with_equal_rank_are_ordered_by_page_and_then_block() {
        on_db_and_tx(|q| {
            for n in 10..30 {
                add_page(q, n, space(1));
                index(
                    q,
                    n,
                    &format!("Filler {n}"),
                    &[(n, "unrelated gardening text")],
                );
            }
            for n in [3, 1, 2] {
                add_page(q, n, space(1));
            }
            // Identical rows have identical ranks; insertion order is scrambled.
            index(q, 3, "Same", &[(9, "needle"), (1, "needle"), (5, "needle")]);
            index(q, 1, "Same", &[(2, "needle")]);
            index(q, 2, "Same", &[(2, "needle")]);

            let hits = search(q, "needle");
            assert_eq!(
                ids(&hits),
                [
                    (page(1), Some(block(2))),
                    (page(2), Some(block(2))),
                    (page(3), Some(block(1))),
                    (page(3), Some(block(5))),
                    (page(3), Some(block(9))),
                ]
            );
            assert!(hits.windows(2).all(|w| w[0].rank == w[1].rank));

            let titles = search(q, "same");
            assert_eq!(
                ids(&titles),
                [(page(1), None), (page(2), None), (page(3), None)]
            );
        });
    }

    #[test]
    fn hits_are_ordered_best_rank_first() {
        on_db_and_tx(|q| {
            for n in 10..40 {
                add_page(q, n, space(1));
                index(
                    q,
                    n,
                    &format!("Filler {n}"),
                    &[(n, "unrelated gardening text")],
                );
            }
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            add_page(q, 3, space(1));
            index(q, 1, "A", &[(1, "kiwi")]); // short row, one occurrence
            index(q, 2, "B", &[(2, "kiwi kiwi kiwi kiwi")]); // many occurrences
            index(
                q,
                3,
                "C",
                &[(
                    3,
                    "a long row that mentions kiwi once among many other words",
                )],
            );

            let hits = search(q, "kiwi");
            assert_eq!(hits.len(), 3);
            assert!(
                hits.windows(2).all(|w| w[0].rank <= w[1].rank),
                "sorted by rank: {hits:?}"
            );
            assert_eq!(hits[0].page_id, page(2), "more occurrences, better rank");
            assert_eq!(hits[2].page_id, page(3), "the longest row ranks last");
        });
    }

    #[test]
    fn trashed_pages_are_never_hit_and_come_back_when_restored() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Live needle", &[(1, "a needle")]);
            index(q, 2, "Trashed needle", &[(2, "another needle")]);
            assert_eq!(search(q, "needle").len(), 4);

            trash_page(q, 2, space(1));
            assert_eq!(
                sorted_ids(&search(q, "needle")),
                [(page(1), None), (page(1), Some(block(1)))],
                "only the live page"
            );
            assert!(
                q.fts_search(&SearchParams {
                    titles_only: true,
                    ..params("trashed")
                })
                .unwrap()
                .is_empty()
            );
            assert_eq!(
                q.fts_page_count(space(1)).unwrap(),
                2,
                "its rows are still there"
            );

            add_page(q, 2, space(1)); // restored
            assert_eq!(search(q, "needle").len(), 4, "no re-indexing needed");
        });
    }

    #[test]
    fn pages_missing_from_the_page_index_are_not_hit() {
        on_db_and_tx(|q| {
            index(q, 1, "Orphan needle", &[(1, "needle")]);
            assert!(search(q, "needle").is_empty());
            add_page(q, 1, space(1));
            assert_eq!(search(q, "needle").len(), 2);
            q.pages_index_delete(&[page(1)]).unwrap();
            assert!(search(q, "needle").is_empty());
        });
    }

    #[test]
    fn the_space_filter_restricts_hits_to_the_pages_of_one_space() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(2));
            add_page(q, 3, space(2));
            index(q, 1, "Alpha", &[(1, "shared word")]);
            index(q, 2, "Beta", &[(2, "shared word")]);
            index(q, 3, "Gamma", &[(3, "shared word")]);

            let in_space = |space_id| {
                sorted_ids(
                    &q.fts_search(&SearchParams {
                        space_id,
                        ..params("shared")
                    })
                    .unwrap(),
                )
            };
            assert_eq!(in_space(Some(space(1))), [(page(1), Some(block(1)))]);
            assert_eq!(
                in_space(Some(space(2))),
                [(page(2), Some(block(2))), (page(3), Some(block(3)))]
            );
            assert!(in_space(Some(space(3))).is_empty());
            assert_eq!(in_space(None).len(), 3, "no filter, every space");

            // The filter works on title hits too.
            let titles = q
                .fts_search(&SearchParams {
                    space_id: Some(space(2)),
                    titles_only: true,
                    ..params("beta gamma")
                })
                .unwrap();
            assert!(titles.is_empty(), "one title never has both words");
            let titles = q
                .fts_search(&SearchParams {
                    space_id: Some(space(1)),
                    titles_only: true,
                    ..params("beta")
                })
                .unwrap();
            assert!(titles.is_empty(), "Beta is in space 2");
        });
    }

    #[test]
    fn titles_only_returns_title_rows_only() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Roadmap", &[(1, "the roadmap for next year")]);
            index(q, 2, "Other", &[(2, "see the roadmap")]);

            assert_eq!(search(q, "roadmap").len(), 3);
            let titles = q
                .fts_search(&SearchParams {
                    titles_only: true,
                    ..params("roadmap")
                })
                .unwrap();
            assert_eq!(ids(&titles), [(page(1), None)]);
            assert_eq!(highlighted(&titles[0]), ["Roadmap"]);
        });
    }

    #[test]
    fn limit_is_clamped_to_one_through_the_maximum() {
        on_db_and_tx(|q| {
            for n in 1..=120u8 {
                add_page(q, n, space(1));
                index(q, n, &format!("match {n}"), &[]);
            }
            let found = |limit| {
                q.fts_search(&SearchParams {
                    limit,
                    ..params("match")
                })
                .unwrap()
                .len()
            };
            assert_eq!(found(0), 1, "zero is raised to one");
            assert_eq!(found(1), 1);
            assert_eq!(found(7), 7);
            assert_eq!(found(100), 100);
            assert_eq!(found(101), 100, "the maximum caps it");
            assert_eq!(found(1_000), 100);
            assert_eq!(found(u32::MAX), 100);
            assert_eq!(usize::try_from(MAX_SEARCH_LIMIT).unwrap(), 100);
        });
    }

    #[test]
    fn a_limited_search_returns_the_best_hits_not_the_first_inserted() {
        on_db_and_tx(|q| {
            for n in 10..40 {
                add_page(q, n, space(1));
                index(
                    q,
                    n,
                    &format!("Filler {n}"),
                    &[(n, "unrelated gardening text")],
                );
            }
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Body only", &[(1, "mango")]);
            index(q, 2, "Mango", &[]); // inserted last, ranks first
            let best = q
                .fts_search(&SearchParams {
                    limit: 1,
                    ..params("mango")
                })
                .unwrap();
            assert_eq!(ids(&best), [(page(2), None)]);
        });
    }

    #[test]
    fn diacritics_are_folded_in_both_directions() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(
                q,
                1,
                "Caf\u{e9}",
                &[
                    (1, "un caf\u{e9} noir"),
                    (2, "CAF\u{c9} au lait"),
                    (3, "Cafe table"),
                ],
            );
            index(q, 2, "R\u{e9}sum\u{e9}", &[(4, "my resume")]);

            for query in ["cafe", "caf\u{e9}", "CAFE", "CAF\u{c9}", "cafe\u{301}"] {
                let hits = search(q, query);
                assert_eq!(
                    sorted_ids(&hits),
                    [
                        (page(1), None),
                        (page(1), Some(block(1))),
                        (page(1), Some(block(2))),
                        (page(1), Some(block(3))),
                    ],
                    "{query:?}"
                );
            }
            assert_eq!(search(q, "resume").len(), 2, "title and block");
            assert_eq!(search(q, "r\u{e9}sum\u{e9}").len(), 2);

            // The highlight covers the accented word as it is written in the text.
            let block_one = search(q, "cafe")
                .into_iter()
                .find(|h| h.block_id == Some(block(1)))
                .unwrap();
            assert_eq!(block_one.snippet, "un caf\u{e9} noir");
            assert_eq!(block_one.highlights, [(3, 7)]);
            assert_eq!(highlighted(&block_one), ["caf\u{e9}"]);
        });
    }

    #[test]
    fn highlights_are_utf16_code_unit_offsets_over_the_marker_free_snippet() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            // Two emoji (2 UTF-16 units each), an astral letter, CJK (3 bytes, 1 unit each)
            // before and between the matches.
            let text =
                "\u{1F389}\u{1F389} \u{1D518}nicode notes and \u{65e5}\u{672c}\u{8a9e} notes";
            index(q, 1, "\u{1F389} party \u{1D518}", &[(1, text)]);

            let hits = search(q, "notes");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].snippet, text, "short text: no ellipsis, no markers");
            // 4 (emoji) + 1 + 8 (astral letter + "nicode") + 1 = 14; "notes" is 5 units.
            // " and " = 5 -> 24, then 3 CJK units + 1 space -> 28.
            assert_eq!(hits[0].highlights, [(14, 19), (28, 33)]);
            assert_eq!(highlighted(&hits[0]), ["notes", "notes"]);

            // A match on the astral letter itself: it is part of the word, so the range
            // starts at 5 (after the emoji pair and the space) and covers 8 units.
            let astral = search(q, "\u{1D518}nic");
            assert_eq!(astral.len(), 1);
            assert_eq!(astral[0].highlights, [(5, 13)]);
            assert_eq!(highlighted(&astral[0]), ["\u{1D518}nicode"]);

            // The title has astral text before the match as well.
            let title = search(q, "party");
            assert_eq!(title.len(), 1);
            assert_eq!(title[0].block_id, None);
            assert_eq!(title[0].snippet, "\u{1F389} party \u{1D518}");
            assert_eq!(title[0].highlights, [(3, 8)]);
            assert_eq!(highlighted(&title[0]), ["party"]);
        });
    }

    #[test]
    fn a_long_row_is_cut_to_a_snippet_around_the_match() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            let words: Vec<String> = (1..=60).map(|i| format!("word{i}")).collect();
            let text = format!("{} needle {}", words[..30].join(" "), words[30..].join(" "));
            index(q, 1, "Long", &[(1, &text)]);

            let hits = search(q, "needle");
            assert_eq!(hits.len(), 1);
            let hit = &hits[0];
            assert!(hit.snippet.starts_with('\u{2026}'), "{}", hit.snippet);
            assert!(hit.snippet.ends_with('\u{2026}'), "{}", hit.snippet);
            assert!(hit.snippet.contains("needle"));
            assert!(
                hit.snippet.chars().count() < text.chars().count() / 2,
                "shortened: {}",
                hit.snippet
            );
            assert!(
                !hit.snippet.contains([HIGHLIGHT_OPEN, HIGHLIGHT_CLOSE]),
                "no markers are left"
            );
            assert_eq!(highlighted(hit), ["needle"]);
            // The ellipsis in front of the text is one UTF-16 unit; the range starts after it.
            assert!(hit.highlights[0].0 > 1, "{hit:?}");
        });
    }

    #[test]
    fn non_latin_text_is_searchable() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            // "Japanese page"; "Hello world" in Cyrillic; "welcome" in Arabic.
            index(
                q,
                1,
                "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30da}\u{30fc}\u{30b8}",
                &[
                    (
                        1,
                        "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442} \u{43c}\u{438}\u{440}",
                    ),
                    (2, "\u{645}\u{631}\u{62d}\u{628}\u{627}"),
                ],
            );
            // Cyrillic is case folded; a run of CJK letters is one word, matched by prefix.
            let hello = "\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}";
            assert_eq!(ids(&search(q, hello)), [(page(1), Some(block(1)))]);
            assert_eq!(
                ids(&search(q, "\u{65e5}\u{672c}")),
                [(page(1), None)],
                "a prefix of the CJK word"
            );
            assert_eq!(
                ids(&search(q, "\u{645}\u{631}")),
                [(page(1), Some(block(2)))]
            );
        });
    }

    /// The query plan SQLite reports for a statement, as one string, with every parameter
    /// bound to a dummy value (`EXPLAIN` does not run the statement).
    fn plan_of(db: &Db, sql: &str) -> String {
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
    fn the_search_statement_starts_from_the_full_text_match() {
        // The word index cannot be searched by page: if the planner started from
        // `pages_index` or `fts_blocks`, the whole match would run again for every row.
        let plan = plan_of(&new_db(), SEARCH_SQL);
        assert!(
            plan.starts_with("SCAN fts VIRTUAL TABLE INDEX"),
            "the match drives: {plan}"
        );
        assert!(
            plan.contains("SEARCH b USING INTEGER PRIMARY KEY (rowid=?)"),
            "each hit reads its ids from the content row by rowid: {plan}"
        );
        assert!(
            plan.contains("SEARCH p USING INDEX sqlite_autoindex_pages_index_1 (page_id=?)"),
            "and finds its page by key: {plan}"
        );
    }

    #[test]
    fn the_by_id_statements_use_the_page_index_and_never_scan_the_text() {
        // The point of `fts_blocks_page`: replacing, renaming, deleting, reading a block and
        // counting title rows cost a few index lookups, not a pass over every indexed row.
        let db = new_db();
        for (name, sql) in [
            ("delete page", FTS_DELETE_PAGE_SQL),
            ("delete text rows", FTS_DELETE_TEXT_SQL),
            ("rename", FTS_SET_TITLE_SQL),
            ("block text", FTS_BLOCK_TEXT_SQL),
            ("page body", FTS_PAGE_BODY_SQL),
            ("page count", FTS_PAGE_COUNT_SQL),
        ] {
            let plan = plan_of(&db, sql);
            assert!(!plan.contains("SCAN"), "{name} scans: {plan}");
            assert!(
                plan.contains("fts_blocks_page (page_id=?"),
                "{name}: {plan}"
            );
        }
        // The property rows have an index of their own (`kind = 1`), so replacing one page's
        // and clearing a space's never pass over the rows of the documents.
        let properties = plan_of(&db, FTS_DELETE_PROPERTIES_SQL);
        assert!(
            properties.contains("fts_blocks_property_rows (page_id=?"),
            "{properties}"
        );
        let clear = plan_of(&db, FTS_CLEAR_PROPERTIES_SQL);
        assert!(
            !clear.contains("SCAN fts_blocks") && clear.contains("fts_blocks_"),
            "clearing a space does not scan the text index: {clear}"
        );
        let block_text = plan_of(&db, FTS_BLOCK_TEXT_SQL);
        assert!(
            block_text.contains("fts_blocks_page (page_id=? AND block_id=?)"),
            "{block_text}"
        );
        let rename = plan_of(&db, FTS_SET_TITLE_SQL);
        assert!(
            rename.contains("fts_blocks_page (page_id=? AND block_id=?)"),
            "the title row (block_id IS NULL) is an index lookup too: {rename}"
        );
        let count = plan_of(&db, FTS_PAGE_COUNT_SQL);
        assert!(
            count.contains("SEARCH p USING INDEX pages_index_space_trashed (space_id=?)"),
            "the pages of the space come from their index: {count}"
        );
    }

    #[test]
    fn the_link_statements_are_driven_by_their_indexes() {
        let db = new_db();
        let from = plan_of(&db, LINKS_FROM_SQL);
        assert!(
            from.contains("USING INDEX sqlite_autoindex_links_1 (from_page=?)"),
            "{from}"
        );
        assert!(
            !from.contains("TEMP B-TREE"),
            "the primary key order is the order asked for: {from}"
        );

        let to = plan_of(&db, LINKS_TO_SQL);
        assert!(
            to.contains("SEARCH l USING INDEX links_to (to_page=?)"),
            "{to}"
        );
        assert!(
            to.contains("SEARCH p USING INDEX sqlite_autoindex_pages_index_1 (page_id=?)"),
            "{to}"
        );

        let delete_from = plan_of(&db, LINKS_DELETE_FROM_SQL);
        assert!(delete_from.contains("(from_page=?)"), "{delete_from}");

        let delete_page = plan_of(&db, LINKS_DELETE_PAGE_SQL);
        assert!(delete_page.contains("MULTI-INDEX OR"), "{delete_page}");
        assert!(delete_page.contains("(from_page=?)"), "{delete_page}");
        assert!(
            delete_page.contains("links_to (to_page=?)"),
            "{delete_page}"
        );
    }

    // ---- page-level match (ADR-0040, item 9) ----------------------------------------

    /// The pages of the hits, in the order of the hits.
    fn pages_of(hits: &[FtsHit]) -> Vec<PageId> {
        hits.iter().map(|hit| hit.page_id).collect()
    }

    /// The pages of the hits, sorted.
    fn sorted_pages(hits: &[FtsHit]) -> Vec<PageId> {
        let mut pages = pages_of(hits);
        pages.sort();
        pages
    }

    fn search_in(
        q: &dyn Queries,
        query: &str,
        space_id: Option<SpaceId>,
        limit: u32,
    ) -> Vec<FtsHit> {
        q.fts_search(&SearchParams {
            space_id,
            limit,
            ..params(query)
        })
        .unwrap()
    }

    #[test]
    fn words_spread_over_the_title_and_a_block_match_the_page_with_one_hit() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(
                q,
                1,
                "Roadmap",
                &[(1, "quarter plan notes"), (2, "unrelated words")],
            );
            // The title holds one word, the block two: the block is the page's hit.
            let hits = search(q, "roadmap quarter plan");
            assert_eq!(ids(&hits), [(page(1), Some(block(1)))]);
            assert_eq!(hits[0].snippet, "quarter plan notes");
            assert_eq!(
                highlighted(&hits[0]),
                ["quarter", "plan"],
                "the words the row holds, not the one the title holds"
            );
            assert!(hits[0].rank < 0.0);

            // Two words, one in the title and one in a block: still one hit.
            let hits = search(q, "roadmap unrelated");
            assert_eq!(hits.len(), 1, "{hits:?}");
            assert_eq!(hits[0].page_id, page(1));
            assert_eq!(hits[0].highlights.len(), 1);

            // A word that is nowhere in the page: no match.
            assert!(search(q, "roadmap quarter zebra").is_empty());
        });
    }

    #[test]
    fn words_spread_over_two_blocks_or_a_property_and_a_block_match_the_page() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(
                q,
                1,
                "Notes",
                &[(1, "alpha beta"), (2, "gamma"), (3, "alpha")],
            );
            let hits = search(q, "alpha beta gamma");
            assert_eq!(
                ids(&hits),
                [(page(1), Some(block(1)))],
                "the row with the most words, once"
            );
            assert_eq!(highlighted(&hits[0]), ["alpha", "beta"]);

            let hits = search(q, "beta gamma");
            assert_eq!(hits.len(), 1, "one word in each of two blocks: {hits:?}");

            // A property and a block.
            add_page(q, 2, space(1));
            index(q, 2, "Plan", &[(1, "budget review")]);
            q.fts_replace_properties(page(2), &property_rows(&[(50, "urgent, work")]))
                .unwrap();
            let hits = search(q, "urgent budget");
            assert_eq!(hits.len(), 1, "{hits:?}");
            assert_eq!(hits[0].page_id, page(2));
            let hits = search(q, "work review plan");
            assert_eq!(hits.len(), 1, "title, block and property: {hits:?}");
        });
    }

    #[test]
    fn a_page_with_a_row_that_holds_every_word_gets_no_second_hit() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(
                q,
                1,
                "Zebra",
                &[(1, "zebra crossing"), (2, "crossing guard")],
            );
            assert_eq!(
                ids(&search(q, "zebra crossing")),
                [(page(1), Some(block(1)))],
                "the title and block 2 each hold one word, block 1 both"
            );
            // Several full-row hits of one page are kept, as before.
            index(
                q,
                1,
                "Zebra",
                &[(1, "zebra crossing"), (2, "crossing the zebra")],
            );
            assert_eq!(
                sorted_ids(&search(q, "zebra crossing")),
                [(page(1), Some(block(1))), (page(1), Some(block(2)))]
            );
        });
    }

    #[test]
    fn rows_that_hold_every_word_rank_before_pages_whose_words_are_spread() {
        on_db_and_tx(|q| {
            for n in 10..30 {
                add_page(q, n, space(1));
                index(
                    q,
                    n,
                    &format!("Filler {n}"),
                    &[(n, "unrelated gardening text")],
                );
            }
            // Page 1 has the lower id and its words in the title and a block; page 2 has one row.
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            index(q, 1, "Zebra", &[(1, "crossing")]);
            index(q, 2, "Other", &[(2, "a zebra crossing")]);
            let hits = search(q, "zebra crossing");
            assert_eq!(
                pages_of(&hits),
                [page(2), page(1)],
                "the full row first: {hits:?}"
            );
            assert_eq!(hits[0].block_id, Some(block(2)));
            // The spread page is one hit, on its title (the weight of a title makes it the best row).
            assert_eq!(hits[1].block_id, None);
        });
    }

    #[test]
    fn spread_hits_are_ordered_by_the_words_their_row_holds_and_fill_what_the_limit_leaves() {
        on_db_and_tx(|q| {
            for n in 1..=6 {
                add_page(q, n, space(1));
            }
            // Page 6 holds all three words in one row.
            index(q, 6, "Six", &[(1, "red green blue")]);
            // Page 5: two words in a row, one in another. Page 4: one word per block, the lowest id.
            index(q, 5, "Five", &[(1, "red green"), (2, "blue")]);
            index(q, 4, "Four", &[(1, "red"), (2, "green"), (3, "blue")]);
            // Page 3 only has two of the three words.
            index(q, 3, "Three", &[(1, "red"), (2, "green")]);

            let all = search(q, "red green blue");
            assert_eq!(
                pages_of(&all),
                [page(6), page(5), page(4)],
                "the full row, then two words, then one word: {all:?}"
            );
            assert_eq!(all[1].block_id, Some(block(1)));
            assert_eq!(highlighted(&all[1]), ["red", "green"]);

            // The limit takes the first ones: the spread hits only fill what the full rows leave.
            let two = search_in(q, "red green blue", None, 2);
            assert_eq!(pages_of(&two), [page(6), page(5)]);
            let one = search_in(q, "red green blue", None, 1);
            assert_eq!(pages_of(&one), [page(6)]);
        });
    }

    #[test]
    fn a_spread_page_adds_one_hit_even_when_several_rows_hold_the_best_count() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(
                q,
                1,
                "Notes",
                &[
                    (1, "red green"),
                    (2, "red green"),
                    (3, "blue"),
                    (4, "green blue"),
                ],
            );
            let hits = search(q, "red green blue");
            assert_eq!(hits.len(), 1, "{hits:?}");
            assert!(hits[0].block_id.is_some());
        });
    }

    #[test]
    fn spread_hits_leave_out_trashed_pages_and_follow_the_space_filter() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(2));
            add_page(q, 3, space(1));
            for n in [1, 2, 3] {
                index(q, n, "Pending", &[(1, "alpha"), (2, "omega")]);
            }
            trash_page(q, 3, space(1));
            assert_eq!(
                sorted_pages(&search(q, "alpha omega")),
                [page(1), page(2)],
                "the trashed page is left out"
            );
            assert_eq!(
                pages_of(&search_in(q, "alpha omega", Some(space(2)), 50)),
                [page(2)]
            );
            assert_eq!(
                pages_of(&search_in(q, "alpha omega", Some(space(1)), 50)),
                [page(1)]
            );
            assert!(search_in(q, "alpha omega", Some(space(3)), 50).is_empty());
            // A page that moves to another space is found there.
            add_page(q, 1, space(2));
            assert_eq!(
                sorted_pages(&search_in(q, "alpha omega", Some(space(2)), 50)),
                [page(1), page(2)]
            );
        });
    }

    #[test]
    fn words_of_a_spread_page_need_every_word_and_ignore_what_the_tokenizer_drops() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Roadmap", &[(1, "quarter")]);
            assert_eq!(search(q, "roadmap quarter").len(), 1);
            assert!(search(q, "roadmap quarter zebra").is_empty());
            // A word with nothing in it (`&`) does not stop the others, and a repeated word is one word.
            assert_eq!(search(q, "roadmap & quarter").len(), 1);
            assert_eq!(search(q, "Roadmap roadmap quarter").len(), 1);
            // Prefixes, as everywhere.
            assert_eq!(search(q, "road quart").len(), 1);
            assert!(
                search(q, "oadmap quart").is_empty(),
                "prefix, not substring"
            );
            // Accents and case are folded across rows too.
            index(q, 1, "Caf\u{e9}", &[(1, "Cr\u{c8}ME")]);
            assert_eq!(search(q, "cafe creme").len(), 1);
        });
    }

    #[test]
    fn titles_only_and_any_word_searches_stay_by_row() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Apple", &[(1, "wine list")]);
            let titles = |query| {
                q.fts_search(&SearchParams {
                    titles_only: true,
                    ..params(query)
                })
                .unwrap()
            };
            assert!(titles("apple wine").is_empty(), "one title row, one title");
            assert_eq!(titles("apple").len(), 1);
            // Any-word: every row that has some word is a hit of its own, as it always was.
            let any = q
                .fts_search(&SearchParams {
                    any_word: true,
                    ..params("apple wine")
                })
                .unwrap();
            assert_eq!(
                sorted_ids(&any),
                [(page(1), None), (page(1), Some(block(1)))],
                "{any:?}"
            );
            // The default search of the same words is one hit.
            assert_eq!(search(q, "apple wine").len(), 1);
        });
    }

    #[test]
    fn a_search_restricted_to_a_page_also_finds_its_spread_words() {
        let db = new_db();
        add_page(&db, 1, space(1));
        add_page(&db, 2, space(1));
        for n in [1, 2] {
            index(&db, n, "Notes", &[(1, "alpha"), (2, "beta")]);
        }
        let found = |n: u8| {
            fts_search_on(db.connection(), &params("alpha beta"), Some(page(n)))
                .unwrap()
                .iter()
                .map(|hit| hit.page_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(found(1), [page(1)]);
        assert_eq!(found(2), [page(2)]);
        assert!(found(3).is_empty());
    }

    #[test]
    fn edited_after_keeps_the_pages_edited_at_or_after_it() {
        on_db_and_tx(|q| {
            // The page index rows say page n was last edited at 100 + n.
            for n in 1..=4 {
                add_page(q, n, space(1));
                index(
                    q,
                    n,
                    "Pending",
                    &[(1, "alpha"), (2, "omega"), (3, "alpha omega")],
                );
            }
            let after = |query: &str, edited_after: Option<i64>, titles_only: bool| {
                let mut pages = sorted_pages(
                    &q.fts_search(&SearchParams {
                        edited_after,
                        titles_only,
                        ..params(query)
                    })
                    .unwrap(),
                );
                pages.dedup();
                pages
            };
            assert_eq!(after("alpha", None, false).len(), 4);
            assert_eq!(
                after("alpha", Some(103), false),
                [page(3), page(4)],
                "at or after"
            );
            assert_eq!(after("alpha", Some(104), false), [page(4)]);
            assert!(after("alpha", Some(105), false).is_empty());
            assert_eq!(
                after("pending", Some(102), true),
                [page(2), page(3), page(4)]
            );

            // Spread words: no row holds both.
            for n in 1..=4 {
                index(q, n, "Pending", &[(1, "alpha"), (2, "omega")]);
            }
            assert_eq!(after("alpha omega", None, false).len(), 4);
            assert_eq!(after("alpha omega", Some(103), false), [page(3), page(4)]);
            assert!(after("alpha omega", Some(200), false).is_empty());
        });
    }

    /// The id of page number `i`, for more pages than the 255 of `page(n)`.
    fn wide_page(i: usize) -> PageId {
        let mut bytes = [3u8; 16];
        bytes[..8].copy_from_slice(&u64::try_from(i).unwrap().to_be_bytes());
        PageId::from_bytes(bytes)
    }

    /// A workspace of `pages` pages with `rows` blocks each. Every page has "alpha" in one block, "beta"
    /// in another and "gamma" in a third; the middle page also has a block with "zeta eta".
    fn big_workspace(pages: usize, rows: usize) -> Db {
        let mut db = new_db();
        db.transaction(|tx| {
            let index_rows: Vec<PageIndexRow> = (0..pages)
                .map(|i| PageIndexRow {
                    page_id: wide_page(i),
                    space_id: space(1),
                    parent_id: None,
                    title: format!("Project {i}"),
                    trashed_at: None,
                    updated_at: i64::try_from(i).unwrap(),
                    in_template: false,
                })
                .collect();
            tx.pages_index_upsert(&index_rows)?;
            for i in 0..pages {
                let mut blocks = vec![
                    FtsBlock {
                        block_id: wide_block(0),
                        text: "shared alpha notes".to_owned(),
                    },
                    FtsBlock {
                        block_id: wide_block(1),
                        text: "shared beta notes".to_owned(),
                    },
                    FtsBlock {
                        block_id: wide_block(2),
                        text: "gamma delta".to_owned(),
                    },
                ];
                for r in 3..rows {
                    blocks.push(FtsBlock {
                        block_id: wide_block(r),
                        text: format!("filler row {r} of page {i} with ordinary words"),
                    });
                }
                if i == pages / 2 {
                    blocks.push(FtsBlock {
                        block_id: wide_block(rows + 1),
                        text: "zeta eta".to_owned(),
                    });
                }
                tx.fts_replace_page(wide_page(i), &format!("Project {i}"), &blocks)?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn a_page_level_search_over_thousands_of_pages_completes() {
        let db = big_workspace(3_000, 6);
        // Every page has "alpha" and "gamma" in different blocks: all 3,000 match, the limit cuts.
        let hits = db
            .fts_search(&SearchParams {
                limit: 100,
                ..params("alpha gamma")
            })
            .unwrap();
        assert_eq!(hits.len(), 100);
        let pages: std::collections::HashSet<PageId> = hits.iter().map(|hit| hit.page_id).collect();
        assert_eq!(pages.len(), 100, "one hit per page");
        // Words that no page has together, and words one page has in one row.
        assert!(db.fts_search(&params("alpha omega")).unwrap().is_empty());
        assert_eq!(db.fts_search(&params("zeta eta")).unwrap().len(), 1);
        // A word in every title and a rare one in a block: found through the title and the block.
        let rare = db.fts_search(&params("project 1500 gamma")).unwrap();
        assert_eq!(pages_of(&rare), [wide_page(1_500)]);
    }

    #[test]
    #[ignore = "a measurement: cargo test -p kb-storage --lib -- --ignored --nocapture ten_thousand"]
    #[allow(clippy::print_stderr)]
    fn ten_thousand_pages_page_level_search() {
        let started = std::time::Instant::now();
        let db = big_workspace(10_000, 20);
        eprintln!("built 10,000 pages of 20 blocks in {:?}", started.elapsed());
        for query in [
            "alpha gamma",
            "alpha beta gamma",
            "shared notes",
            "project 4000 gamma",
        ] {
            let started = std::time::Instant::now();
            let hits = db
                .fts_search(&SearchParams {
                    limit: 30,
                    ..params(query)
                })
                .unwrap();
            eprintln!("{query:?}: {} hits in {:?}", hits.len(), started.elapsed());
        }
    }

    // ---- query sanitising ----------------------------------------------------------

    #[test]
    fn user_syntax_is_read_as_plain_words() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(
                q,
                1,
                "Notes and ideas",
                &[
                    (1, "Ideas about notes"),
                    (2, "well-known facts"),
                    (3, "2024-10-03 meeting"),
                    (4, "e-mail me at a@b.com"),
                    (5, "just ideas"),
                ],
            );
            // `None` stands for the title row, `Some(n)` for block n.
            let found = |query: &str| -> Vec<Option<u8>> {
                let mut rows: Vec<(Option<u8>, PageId)> = search(q, query)
                    .iter()
                    .map(|hit| {
                        let n = hit.block_id.map(|b| b.as_bytes()[0]);
                        (n, hit.page_id)
                    })
                    .collect();
                rows.sort();
                assert!(rows.iter().all(|(_, p)| *p == page(1)));
                rows.into_iter().map(|(n, _)| n).collect()
            };
            let notes = [None, Some(1)];

            // Quotes, stars and the other characters of FTS5's syntax are word separators.
            for query in [
                "notes",
                "\"notes\"",
                "'notes'",
                "notes*",
                "-notes",
                "+notes",
                "^notes",
                "(notes)",
                "notes:",
                "  notes  ",
                "notes\t",
                "\nnotes",
            ] {
                assert_eq!(found(query), notes, "{query:?}");
            }
            // A phrase in quotes is searched as its words (in any order).
            assert_eq!(found("\"ideas about\""), [Some(1)]);
            assert_eq!(found("about ideas"), [Some(1)]);

            // Bare capital operators are dropped; the words around them are all required.
            assert_eq!(found("ideas"), [None, Some(1), Some(5)]);
            for query in [
                "notes AND ideas",
                "notes OR ideas",
                "NOT notes ideas",
                "NEAR(notes ideas)",
                "AND notes OR ideas NOT",
            ] {
                assert_eq!(found(query), notes, "{query:?}");
            }
            // Lowercase operators are ordinary words: "and" is a word of the title, and "not"
            // is the start of "notes".
            assert_eq!(found("and"), [None]);
            assert_eq!(found("not"), notes);
            assert!(found("or").is_empty());
            assert!(found("near").is_empty());

            // Separators inside a word split it: well-known facts, dates, addresses.
            assert_eq!(found("well-known"), [Some(2)]);
            assert_eq!(found("well known"), [Some(2)]);
            assert_eq!(found("2024-10-03"), [Some(3)]);
            assert_eq!(found("e-mail"), [Some(4)]);
            assert_eq!(found("a@b.com"), [Some(4)], "searched as a phrase");

            // A column filter is not a column filter, just the words "title" and "notes".
            assert!(found("title:notes").is_empty());

            // Every kind of space separates words.
            assert_eq!(found("notes\u{a0}ideas"), notes);
            assert_eq!(found("notes\u{3000}ideas"), notes);
            assert_eq!(found("notes\u{2003}ideas"), notes);
            // So does a control character; a NUL must not cut the query short.
            assert_eq!(found("notes\0ideas"), notes);
            assert_eq!(
                found("not\u{1}es"),
                Vec::<Option<u8>>::new(),
                "two words, not and es"
            );
        });
    }

    #[test]
    fn a_query_without_words_returns_no_hits_without_touching_the_index() {
        const NOTHING: &[&str] = &[
            "",
            " ",
            "\t\n\r",
            "AND",
            "OR NOT",
            "NEAR AND OR NOT",
            "\"",
            "\"\"",
            "\"\" \"\"",
            "*",
            "()",
            "-+^:",
            "(\"*\")",
            "\0",
            "\u{1}\u{2}\u{7f}\u{9f}",
            "\u{a0}\u{2003}\u{3000}\u{2028}",
            "AND\0OR",
        ];
        // With the table gone, any statement that reaches it fails: these queries only
        // succeed because they never get that far.
        let db = new_db();
        db.connection().execute_batch("DROP TABLE fts;").unwrap();
        for query in NOTHING {
            assert!(
                db.fts_search(&params(query)).unwrap().is_empty(),
                "{query:?}"
            );
        }
        let err = db.fts_search(&params("word")).unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");

        let mut db = new_db();
        db.connection().execute_batch("DROP TABLE fts;").unwrap();
        db.transaction(|tx| {
            for query in NOTHING {
                assert!(
                    tx.fts_search(&params(query)).unwrap().is_empty(),
                    "{query:?}"
                );
            }
            assert!(tx.fts_search(&params("word")).is_err());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_word_that_is_only_punctuation_matches_nothing_and_is_not_an_error() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "Title", &[(1, "plain words & more...")]);
            for query in ["&", "&&", "...", "?!", "@", "$%", "\u{2026}", "\u{1F389}"] {
                assert!(search(q, query).is_empty(), "{query:?}");
            }
            // A word the tokenizer drops does not stop the others from matching.
            assert_eq!(
                ids(&search(q, "plain & words")),
                [(page(1), Some(block(1)))]
            );
        });
    }

    #[test]
    fn the_query_is_cut_at_the_maximum_length() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            index(q, 1, "T", &[(1, "needle")]);
            let gap = MAX_QUERY_CHARS - "needle".len();

            // The word that cannot match is the 201st character on: never read.
            let beyond = format!("needle{}zzzz", " ".repeat(gap));
            assert_eq!(beyond.chars().count(), MAX_QUERY_CHARS + 4);
            assert_eq!(search(q, &beyond).len(), 1);

            // Inside the limit it is a required word.
            let inside = format!("needle{}zzzz", " ".repeat(gap - 4));
            assert_eq!(inside.chars().count(), MAX_QUERY_CHARS);
            assert!(search(q, &inside).is_empty());

            // The limit counts characters, not bytes: with three-byte spaces, the word is
            // still inside the first 200 characters.
            let wide = format!("needle{}zzzz", "\u{3000}".repeat(gap - 4));
            assert_eq!(wide.chars().count(), MAX_QUERY_CHARS);
            assert!(wide.len() > 2 * MAX_QUERY_CHARS);
            assert!(search(q, &wide).is_empty());
        });
    }

    /// Strings that have broken query parsers before.
    const HOSTILE: &[&str] = &[
        "\"",
        "\"\"\"",
        "\" OR 1=1 --",
        "'; DROP TABLE fts; --",
        "a\" OR \"b",
        "\"unterminated",
        "unbalanced)",
        "(((",
        "*",
        "**",
        "* *",
        "a**",
        "-",
        "--",
        "+-+-",
        "^",
        "^^a",
        ":",
        "::",
        "a:b:c",
        "title:",
        "body: notes",
        "{title}: notes",
        "{title body}: notes",
        "title : \"notes\"",
        "NEAR",
        "NEAR(",
        "NEAR(a b, 2)",
        "NEAR/2",
        "a NEAR b",
        "a AND",
        "AND a",
        "a AND AND b",
        "a OR OR b",
        "NOT",
        "a NOT",
        "NOT NOT",
        "AND OR NOT NEAR",
        "a AND (b OR c) NOT d",
        "\0",
        "a\0b",
        "\"a\0b\"",
        "\u{1}\u{7f}\u{9f}",
        "%",
        "_",
        "\\",
        "\\\"",
        "/*",
        "*/",
        "<script>alert(1)</script>",
        "?",
        "$1",
        "?1",
        ":name",
        "@p",
        "\u{e000}",
        "\u{e001}",
        "\u{e000}notes\u{e001}",
        "\u{fffd}",
        "\u{301}",
        "\u{200d}",
        "\u{202e}notes",
        "\u{feff}notes",
        "\u{1F389}",
        "\u{1D518}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "caf\u{e9}",
        "\u{df}",
        "\u{130}",
        "\u{1c5}",
        "\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467}",
    ];

    /// Pieces the fuzz test glues together: syntax, operators, whitespace and odd text.
    const PIECES: &[&str] = &[
        "a",
        "b",
        "notes",
        "ideas",
        "caf\u{e9}",
        "cafe\u{301}",
        " ",
        "  ",
        "\t",
        "\n",
        "\0",
        "\"",
        "'",
        "*",
        "(",
        ")",
        ":",
        "^",
        "-",
        "+",
        "{",
        "}",
        "[",
        "]",
        "\\",
        "/",
        ".",
        ",",
        ";",
        "!",
        "?",
        "%",
        "_",
        "~",
        "|",
        "<",
        ">",
        "=",
        "@",
        "#",
        "$",
        "&",
        "AND",
        "OR",
        "NOT",
        "NEAR",
        "NEAR/2",
        "title:",
        "body:",
        "{title}",
        "\u{e000}",
        "\u{e001}",
        "\u{fffd}",
        "\u{301}",
        "\u{a0}",
        "\u{2003}",
        "\u{3000}",
        "\u{85}",
        "\u{1F389}",
        "\u{1D518}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "\u{200d}",
        "\u{202e}",
        "\u{feff}",
    ];

    /// A small deterministic generator (xorshift64), so a failure can be replayed.
    struct Xorshift(u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
        }
    }

    /// Everything a caller may rely on in a hit, whatever the query was.
    fn assert_well_formed(hits: &[FtsHit], limit: u32, context: &str) {
        let cap = usize::try_from(limit.clamp(1, MAX_SEARCH_LIMIT)).unwrap();
        assert!(
            hits.len() <= cap,
            "{context}: {} hits for limit {limit}",
            hits.len()
        );
        for hit in hits {
            assert!(
                !hit.snippet.contains([HIGHLIGHT_OPEN, HIGHLIGHT_CLOSE]),
                "{context}: marker left in {:?}",
                hit.snippet
            );
            let units = u32::try_from(hit.snippet.encode_utf16().count()).unwrap();
            let mut previous_end = 0;
            for &(start, end) in &hit.highlights {
                assert!(
                    previous_end <= start && start < end && end <= units,
                    "{context}: bad highlights {:?} in {:?}",
                    hit.highlights,
                    hit.snippet
                );
                previous_end = end;
            }
        }
    }

    fn fuzz_database(q: &dyn Queries) {
        add_page(q, 1, space(1));
        add_page(q, 2, space(2));
        index(
            q,
            1,
            "Notes and ideas",
            &[
                (1, "Ideas about notes"),
                (
                    2,
                    "caf\u{e9} \u{65e5}\u{672c}\u{8a9e} \u{1F389} \u{1D518}nicode",
                ),
                (
                    3,
                    "AND OR NOT NEAR title: body: \"quoted\" (nested) *star* -minus +plus",
                ),
            ],
        );
        index(q, 2, "Second page", &[(4, "more notes and ideas")]);
    }

    #[test]
    fn hostile_queries_never_fail() {
        on_db_and_tx(|q| {
            fuzz_database(q);
            let long_quotes = "\"".repeat(5_000);
            let long_words = "a ".repeat(500);
            let long_parens = "(".repeat(1_000);
            let queries = HOSTILE.iter().copied().chain([
                long_quotes.as_str(),
                long_words.as_str(),
                long_parens.as_str(),
            ]);
            for query in queries {
                for titles_only in [false, true] {
                    for space_id in [None, Some(space(1)), Some(space(3))] {
                        let search = SearchParams {
                            space_id,
                            titles_only,
                            ..params(query)
                        };
                        let context = format!("{query:?}");
                        let hits = q.fts_search(&search).unwrap_or_else(|e| {
                            panic!("{context}: {e}");
                        });
                        assert_well_formed(&hits, search.limit, &context);
                    }
                }
            }
        });
    }

    #[test]
    fn random_queries_never_fail_and_always_give_well_formed_hits() {
        on_db_and_tx(|q| {
            fuzz_database(q);
            let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
            for round in 0..2_500 {
                let mut query = String::new();
                for _ in 0..rng.below(14) {
                    query.push_str(PIECES[rng.below(PIECES.len())]);
                }
                let search = SearchParams {
                    space_id: [None, Some(space(1)), Some(space(2))][rng.below(3)],
                    titles_only: rng.below(2) == 1,
                    limit: [0, 1, 5, 50, 1_000][rng.below(5)],
                    ..params(&query)
                };
                let context = format!("round {round}, query {query:?}");
                let hits = q.fts_search(&search).unwrap_or_else(|e| {
                    panic!("{context}: {e}");
                });
                assert_well_formed(&hits, search.limit, &context);
            }
        });
    }

    #[test]
    fn match_expression_turns_words_into_prefix_phrases() {
        let expr = |query: &str| match_expression(query, false);
        assert_eq!(expr("notes").as_deref(), Some("\"notes\"*"));
        assert_eq!(
            expr("  Hello \t World\n").as_deref(),
            Some("\"Hello\"* \"World\"*"),
            "case is left to the tokenizer"
        );
        assert_eq!(
            expr("caf\u{e9} \u{65e5}\u{672c}").as_deref(),
            Some("\"caf\u{e9}\"* \"\u{65e5}\u{672c}\"*")
        );
        assert_eq!(
            expr("\"quoted phrase\"").as_deref(),
            Some("\"quoted\"* \"phrase\"*")
        );
        assert_eq!(
            expr("-a +b ^c d*").as_deref(),
            Some("\"a\"* \"b\"* \"c\"* \"d\"*")
        );
        assert_eq!(
            expr("title:foo").as_deref(),
            Some("\"title\"* \"foo\"*"),
            "a colon separates words"
        );
        assert_eq!(expr("(a OR b)").as_deref(), Some("\"a\"* \"b\"*"));
        assert_eq!(
            expr("a AND b NOT c").as_deref(),
            Some("\"a\"* \"b\"* \"c\"*")
        );
        assert_eq!(
            expr("and or not near").as_deref(),
            Some("\"and\"* \"or\"* \"not\"* \"near\"*"),
            "only capitals are operators"
        );
        assert_eq!(
            expr("ANDROID NOTE ORDER").as_deref(),
            Some("\"ANDROID\"* \"NOTE\"* \"ORDER\"*"),
            "a word that starts with an operator is a word"
        );
        assert_eq!(expr("NEAR/2").as_deref(), Some("\"NEAR/2\"*"));
        assert_eq!(expr("a.b").as_deref(), Some("\"a.b\"*"));
        assert_eq!(expr("a\0b").as_deref(), Some("\"a\"* \"b\"*"));
        assert_eq!(
            expr("a\u{a0}b\u{3000}c").as_deref(),
            Some("\"a\"* \"b\"* \"c\"*")
        );

        for nothing in [
            "",
            "   ",
            "AND",
            "OR NOT NEAR",
            "\"\"",
            "*",
            "()",
            "-+^:",
            "\0",
        ] {
            assert_eq!(expr(nothing), None, "{nothing:?}");
        }

        // Cut at the maximum length, counted in characters.
        let long = expr(&"a".repeat(500)).unwrap();
        assert_eq!(long.chars().count(), MAX_QUERY_CHARS + 3);
        let accents = expr(&"\u{e9}".repeat(500)).unwrap();
        assert_eq!(accents.chars().count(), MAX_QUERY_CHARS + 3);
    }

    #[test]
    fn any_word_mode_ors_the_words_and_drops_the_short_ones() {
        let any = |query: &str| match_expression(query, true);
        assert_eq!(
            any("when is the launch").as_deref(),
            Some("\"when\"* OR \"the\"* OR \"launch\"*"),
            "words of 2 or fewer characters (is) are dropped, the others are OR-ed"
        );
        assert_eq!(any("notes").as_deref(), Some("\"notes\"*"));
        assert_eq!(
            any("ab abc").as_deref(),
            Some("\"abc\"*"),
            "3 characters stay"
        );
        assert_eq!(
            any("\u{65e5}\u{672c} \u{65e5}\u{672c}\u{8a9e}").as_deref(),
            Some("\"\u{65e5}\u{672c}\u{8a9e}\"*"),
            "length counts characters, not bytes"
        );
        assert_eq!(any("a AND ab OR b").as_deref(), None, "nothing is left");
        assert_eq!(any("").as_deref(), None);
        assert_eq!(
            any("(abc OR def)").as_deref(),
            Some("\"abc\"* OR \"def\"*"),
            "the syntax characters still separate words and bare operators are dropped"
        );
        // The same text in the default mode keeps every word, AND-ed.
        assert_eq!(
            match_expression("when is the launch", false).as_deref(),
            Some("\"when\"* \"is\"* \"the\"* \"launch\"*")
        );
    }

    #[test]
    fn any_word_search_finds_rows_that_have_some_of_the_words() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(2));
            add_page(q, 3, space(1));
            index(
                q,
                1,
                "Plan",
                &[
                    (1, "the launch of the product is planned for spring"),
                    (2, "budget numbers"),
                    (3, "launch"),
                ],
            );
            index(q, 2, "Other", &[(1, "product roadmap and launch dates")]);
            index(q, 3, "Gone", &[(1, "launch of something else")]);
            trash_page(q, 3, space(1));

            let any = |query: &str, space_id: Option<SpaceId>| {
                q.fts_search(&SearchParams {
                    space_id,
                    any_word: true,
                    ..params(query)
                })
                .unwrap()
            };
            // AND finds nothing for the whole sentence, OR finds the rows with some of the words.
            assert!(search(q, "when does the product launch happen").is_empty());
            let hits = any("when does the product launch happen", None);
            assert_eq!(
                sorted_ids(&hits),
                [
                    (page(1), Some(block(1))),
                    (page(1), Some(block(3))),
                    (page(2), Some(block(1))),
                ],
                "the trashed page is never found; \"budget numbers\" has none of the words"
            );
            // A prefix of a word still matches (plann: planned).
            let prefixes = any("plann", None);
            assert_eq!(ids(&prefixes), [(page(1), Some(block(1)))]);
            // Scoped to one space.
            assert_eq!(
                sorted_ids(&any("launch product", Some(space(2)))),
                [(page(2), Some(block(1)))]
            );
            // Every word is 2 characters or fewer: nothing to search.
            assert!(any("is of to", None).is_empty());
        });
    }

    #[test]
    fn a_search_can_be_restricted_to_one_page() {
        let db = new_db();
        for n in [1, 2, 3] {
            add_page(&db, n, space(1));
            index(&db, n, "Notes", &[(1, "alpha beta"), (2, "alpha gamma")]);
        }
        trash_page(&db, 3, space(1));
        let search = SearchParams {
            any_word: true,
            ..params("alpha")
        };
        let in_page = |n: u8| {
            fts_search_on(db.connection(), &search, Some(page(n)))
                .unwrap()
                .iter()
                .map(|h| (h.page_id, h.block_id))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            in_page(1),
            [(page(1), Some(block(1))), (page(1), Some(block(2)))]
        );
        assert_eq!(
            in_page(2),
            [(page(2), Some(block(1))), (page(2), Some(block(2)))]
        );
        assert!(in_page(3).is_empty(), "a trashed page has no hits");
        assert!(in_page(9).is_empty(), "nor an unknown one");
        assert_eq!(
            fts_search_on(db.connection(), &search, None).unwrap().len(),
            4,
            "without a page the search covers every live page"
        );
    }

    #[test]
    fn a_quote_inside_a_word_is_doubled() {
        // `match_expression` never lets a quote through, but the quoting must be right on
        // its own: a stray quote would end the phrase early.
        let mut out = String::new();
        push_prefix_phrase(&mut out, "a\"b\"\"c");
        assert_eq!(out, "\"a\"\"b\"\"\"\"c\"*");
        let mut out = String::from("x ");
        push_prefix_phrase(&mut out, "plain");
        assert_eq!(out, "x \"plain\"*");
    }

    #[test]
    fn indexable_text_changes_only_for_the_two_markers() {
        assert!(matches!(
            indexable("plain caf\u{e9} \u{1F389}"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(indexable(""), Cow::Borrowed("")));
        assert_eq!(
            indexable("a\u{E000}b\u{E001}c\u{E000}"),
            "a\u{FFFD}b\u{FFFD}c\u{FFFD}"
        );
        assert_eq!(
            indexable("\u{E002}\u{F8FF}"),
            "\u{E002}\u{F8FF}",
            "other private use stays"
        );
    }

    // ---- snippet decoding ----------------------------------------------------------

    fn decoded(raw: &str) -> (String, Vec<(u32, u32)>) {
        decode_snippet(raw)
    }

    #[test]
    fn decode_snippet_removes_the_markers_and_reports_utf16_ranges() {
        assert_eq!(decoded(""), (String::new(), vec![]));
        assert_eq!(decoded("plain text"), ("plain text".to_owned(), vec![]));
        assert_eq!(
            decoded("a\u{E000}b\u{E001}c"),
            ("abc".to_owned(), vec![(1, 2)])
        );
        assert_eq!(
            decoded("\u{E000}ab\u{E001} \u{E000}c\u{E001}"),
            ("ab c".to_owned(), vec![(0, 2), (3, 4)]),
            "adjacent matches stay separate"
        );
        assert_eq!(
            decoded("\u{2026}\u{E000}x\u{E001}\u{2026}"),
            ("\u{2026}x\u{2026}".to_owned(), vec![(1, 2)])
        );
    }

    #[test]
    fn decode_snippet_counts_utf16_code_units_not_bytes_or_characters() {
        // An emoji is 4 bytes, 1 character and 2 UTF-16 units.
        assert_eq!(
            decoded("\u{1F389}\u{E000}x\u{E001}"),
            ("\u{1F389}x".to_owned(), vec![(2, 3)])
        );
        // A highlight can contain astral characters.
        assert_eq!(
            decoded("\u{E000}\u{1D518}x\u{E001}"),
            ("\u{1D518}x".to_owned(), vec![(0, 3)])
        );
        // Three-byte characters of the basic plane are one unit each.
        assert_eq!(
            decoded("\u{65e5}\u{672c}\u{E000}\u{8a9e}\u{E001}"),
            ("\u{65e5}\u{672c}\u{8a9e}".to_owned(), vec![(2, 3)])
        );
        assert_eq!(
            decoded("\u{e9}\u{E000}\u{e9}\u{E001}"),
            ("\u{e9}\u{e9}".to_owned(), vec![(1, 2)])
        );
        // Several pairs and astral text in between.
        assert_eq!(
            decoded("\u{E000}a\u{E001}\u{1F389}\u{1F389}\u{E000}b\u{E001}"),
            ("a\u{1F389}\u{1F389}b".to_owned(), vec![(0, 1), (5, 6)])
        );
    }

    #[test]
    fn decode_snippet_survives_unbalanced_markers() {
        // A close without an open is ignored.
        assert_eq!(decoded("a\u{E001}b"), ("ab".to_owned(), vec![]));
        // An open without a close runs to the end.
        assert_eq!(decoded("a\u{E000}bc"), ("abc".to_owned(), vec![(1, 3)]));
        // A second open inside a highlight is ignored.
        assert_eq!(
            decoded("\u{E000}a\u{E000}b\u{E001}"),
            ("ab".to_owned(), vec![(0, 2)])
        );
        // Empty highlights are dropped, even at the very end.
        assert_eq!(decoded("a\u{E000}\u{E001}b"), ("ab".to_owned(), vec![]));
        assert_eq!(decoded("ab\u{E000}"), ("ab".to_owned(), vec![]));
        // Only markers.
        assert_eq!(
            decoded("\u{E000}\u{E001}\u{E001}\u{E000}"),
            (String::new(), vec![])
        );
    }

    // ---- damaged rows ----------------------------------------------------------------

    #[test]
    fn damaged_rows_are_invalid_data_and_never_a_panic() {
        // A search hit with an id that is not 16 bytes long.
        for (page_id, block_id) in [("x'010203'", "NULL"), ("zeroblob(16)", "x'01'")] {
            let db = new_db();
            db.connection()
                .execute_batch(&format!(
                    "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES ({page_id}, zeroblob(16), NULL, 't', NULL, 1);
                     INSERT INTO fts_blocks (page_id, block_id, title, body)
                       VALUES ({page_id}, {block_id}, 'damaged', 'damaged');"
                ))
                .unwrap();
            let err = db.fts_search(&params("damaged")).unwrap_err();
            assert!(
                matches!(err, StorageError::InvalidData(_)),
                "{page_id}: {err}"
            );
            assert!(!err.to_string().contains("damaged"), "no content in errors");
        }

        // Link rows with ids of the wrong length.
        let db = new_db();
        db.connection()
            .execute_batch(
                "INSERT INTO links VALUES (zeroblob(16), x'01', zeroblob(16), NULL);
                 INSERT INTO links VALUES (zeroblob(16), zeroblob(16), x'0102', NULL);
                 INSERT INTO links VALUES (zeroblob(16), zeroblob(16), zeroblob(16), x'03');",
            )
            .unwrap();
        let err = db.links_from(page(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");

        let db = new_db();
        db.connection()
            .execute_batch(
                "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (x'0102', zeroblob(16), NULL, 't', NULL, 1);
                 INSERT INTO links VALUES (x'0102', zeroblob(16), zeroblob(16), NULL);",
            )
            .unwrap();
        let err = db.links_to(page(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");

        let db = new_db();
        db.connection()
            .execute_batch(
                "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (zeroblob(16), zeroblob(16), NULL, 't', NULL, 1);
                 INSERT INTO links VALUES (zeroblob(16), zeroblob(16), zeroblob(16), x'0405');",
            )
            .unwrap();
        let err = db.links_to(page(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    // ---- links ---------------------------------------------------------------------

    #[test]
    fn link_rows_roundtrip_with_and_without_a_target_block() {
        on_db_and_tx(|q| {
            let rows = [link(1, 2, None), link(2, 3, Some(9))];
            q.links_replace_from(page(1), &rows).unwrap();
            assert_eq!(q.links_from(page(1)).unwrap(), rows);
            assert!(q.links_from(page(2)).unwrap().is_empty());
            assert!(q.links_from(page(99)).unwrap().is_empty());
        });
    }

    #[test]
    fn links_are_stored_as_16_byte_blobs() {
        let db = new_db();
        db.links_replace_from(page(1), &[link(2, 3, Some(4)), link(5, 6, None)])
            .unwrap();
        type Row = (String, i64, String, String, String, Option<i64>);
        let rows: Vec<Row> = db
            .connection()
            .prepare(
                "SELECT typeof(from_page), length(from_page), typeof(from_block), \
                        typeof(to_page), typeof(to_block), length(to_block) \
                   FROM links ORDER BY from_block",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let row = |to_block: &str, len| -> Row {
            (
                "blob".to_owned(),
                16,
                "blob".to_owned(),
                "blob".to_owned(),
                to_block.to_owned(),
                len,
            )
        };
        assert_eq!(rows, [row("blob", Some(16)), row("null", None)]);
    }

    #[test]
    fn replacing_links_replaces_the_rows_of_that_page_only() {
        on_db_and_tx(|q| {
            q.links_replace_from(page(1), &[link(1, 2, None), link(2, 3, None)])
                .unwrap();
            q.links_replace_from(page(2), &[link(5, 1, None)]).unwrap();

            q.links_replace_from(page(1), &[link(7, 4, Some(8))])
                .unwrap();
            assert_eq!(q.links_from(page(1)).unwrap(), [link(7, 4, Some(8))]);
            assert_eq!(q.links_from(page(2)).unwrap(), [link(5, 1, None)]);

            // The same rows again change nothing; no rows at all removes them.
            q.links_replace_from(page(1), &[link(7, 4, Some(8))])
                .unwrap();
            assert_eq!(q.links_from(page(1)).unwrap(), [link(7, 4, Some(8))]);
            q.links_replace_from(page(1), &[]).unwrap();
            assert!(q.links_from(page(1)).unwrap().is_empty());
            assert_eq!(q.links_from(page(2)).unwrap().len(), 1);
        });
    }

    #[test]
    fn links_from_is_ordered_by_block_then_target_page() {
        on_db_and_tx(|q| {
            q.links_replace_from(
                page(1),
                &[
                    link(5, 3, None),
                    link(2, 9, None),
                    link(5, 1, Some(7)),
                    link(2, 4, None),
                ],
            )
            .unwrap();
            assert_eq!(
                q.links_from(page(1)).unwrap(),
                [
                    link(2, 4, None),
                    link(2, 9, None),
                    link(5, 1, Some(7)),
                    link(5, 3, None),
                ]
            );
        });
    }

    #[test]
    fn a_block_that_references_a_page_twice_keeps_its_first_row() {
        on_db_and_tx(|q| {
            q.links_replace_from(
                page(1),
                &[
                    link(1, 2, Some(5)),
                    link(1, 2, Some(6)),
                    link(1, 2, None),
                    link(1, 3, None),
                    link(2, 2, None),
                ],
            )
            .unwrap();
            assert_eq!(
                q.links_from(page(1)).unwrap(),
                [link(1, 2, Some(5)), link(1, 3, None), link(2, 2, None)],
                "one row per (block, page); another block may reference the page again"
            );
        });
    }

    #[test]
    fn links_to_lists_the_live_sources_ordered_by_page_then_block() {
        on_db_and_tx(|q| {
            for n in [1, 3, 4, 5] {
                add_page(q, n, space(1));
            }
            // Page 6 has links but no `pages_index` row.
            q.links_replace_from(page(4), &[link(2, 1, Some(7))])
                .unwrap();
            q.links_replace_from(page(3), &[link(9, 1, None), link(1, 1, None)])
                .unwrap();
            q.links_replace_from(page(5), &[link(1, 1, None)]).unwrap();
            q.links_replace_from(page(6), &[link(1, 1, None)]).unwrap();
            q.links_replace_from(page(1), &[link(1, 1, None), link(2, 3, None)])
                .unwrap();
            trash_page(q, 5, space(1));

            assert_eq!(
                q.links_to(page(1)).unwrap(),
                [
                    source(1, 1, None), // a page may link to itself
                    source(3, 1, None),
                    source(3, 9, None),
                    source(4, 2, Some(7)),
                ],
                "trashed and unindexed sources are left out"
            );
            assert_eq!(q.links_to(page(3)).unwrap(), [source(1, 2, None)]);
            assert!(q.links_to(page(99)).unwrap().is_empty());

            // The filter is applied when asked: restore, trash and remove change the answer.
            add_page(q, 5, space(1));
            assert_eq!(q.links_to(page(1)).unwrap().len(), 5);
            trash_page(q, 3, space(1));
            assert_eq!(q.links_to(page(1)).unwrap().len(), 3);
            q.pages_index_delete(&[page(4)]).unwrap();
            assert_eq!(q.links_to(page(1)).unwrap().len(), 2);
        });
    }

    #[test]
    fn links_to_a_trashed_page_are_still_listed() {
        on_db_and_tx(|q| {
            add_page(q, 1, space(1));
            trash_page(q, 2, space(1));
            q.links_replace_from(page(1), &[link(1, 2, None)]).unwrap();
            assert_eq!(
                q.links_to(page(2)).unwrap(),
                [source(1, 1, None)],
                "only the source has to be live"
            );
        });
    }

    #[test]
    fn links_delete_page_removes_references_in_both_directions() {
        on_db_and_tx(|q| {
            q.links_replace_from(page(1), &[link(1, 2, None), link(2, 1, None)])
                .unwrap();
            q.links_replace_from(page(2), &[link(3, 1, Some(4)), link(4, 3, None)])
                .unwrap();
            q.links_replace_from(page(3), &[link(5, 2, None)]).unwrap();
            q.links_replace_from(page(4), &[link(6, 5, None)]).unwrap();

            q.links_delete_page(page(1)).unwrap();
            assert!(q.links_from(page(1)).unwrap().is_empty(), "outgoing");
            assert_eq!(
                q.links_from(page(2)).unwrap(),
                [link(4, 3, None)],
                "the incoming link from page 2 went, its other link stayed"
            );
            assert_eq!(q.links_from(page(3)).unwrap(), [link(5, 2, None)]);
            assert_eq!(q.links_from(page(4)).unwrap(), [link(6, 5, None)]);

            // Again, and for a page without links, nothing changes.
            q.links_delete_page(page(1)).unwrap();
            q.links_delete_page(page(99)).unwrap();
            assert_eq!(q.links_from(page(3)).unwrap().len(), 1);
        });
    }

    #[test]
    fn links_delete_page_removes_a_self_link_too() {
        on_db_and_tx(|q| {
            q.links_replace_from(page(1), &[link(1, 1, None)]).unwrap();
            q.links_delete_page(page(1)).unwrap();
            assert!(q.links_from(page(1)).unwrap().is_empty());
        });
    }

    #[test]
    fn links_clear_removes_every_row() {
        on_db_and_tx(|q| {
            q.links_replace_from(page(1), &[link(1, 2, None)]).unwrap();
            q.links_replace_from(page(3), &[link(2, 1, Some(5))])
                .unwrap();
            q.links_clear().unwrap();
            assert!(q.links_from(page(1)).unwrap().is_empty());
            assert!(q.links_from(page(3)).unwrap().is_empty());
            q.links_clear().unwrap();
            q.links_replace_from(page(1), &[link(1, 2, None)]).unwrap();
            assert_eq!(q.links_from(page(1)).unwrap().len(), 1);
        });
    }

    // ---- transactions and atomicity --------------------------------------------------

    #[test]
    fn a_committed_transaction_keeps_the_index_writes() {
        let mut db = new_db();
        let value = db
            .transaction(|tx| {
                add_page(tx, 1, space(1));
                add_page(tx, 2, space(1));
                index(tx, 1, "Committed", &[(1, "kept text")]);
                tx.links_replace_from(page(1), &[link(1, 2, None)])?;
                tx.fts_set_title(page(2), "Second")?;
                Ok(7)
            })
            .unwrap();
        assert_eq!(value, 7, "the closure's value is passed through");

        assert_eq!(ids(&search(&db, "kept")), [(page(1), Some(block(1)))]);
        assert_eq!(db.links_from(page(1)).unwrap(), [link(1, 2, None)]);
        assert_eq!(db.links_to(page(2)).unwrap(), [source(1, 1, None)]);
        assert_eq!(db.fts_page_count(space(1)).unwrap(), 2);
        assert!(db.connection().is_autocommit(), "the transaction is over");
    }

    #[test]
    fn a_failing_transaction_rolls_back_every_index_write() {
        let mut db = new_db();
        add_page(&db, 1, space(1));
        add_page(&db, 2, space(1));
        index(&db, 1, "Before", &[(1, "old text")]);
        index(&db, 2, "Second", &[(2, "second text")]);
        db.links_replace_from(page(1), &[link(1, 2, None)]).unwrap();
        db.links_replace_from(page(2), &[link(2, 1, None)]).unwrap();

        let err = db
            .transaction(|tx| {
                index(tx, 1, "After", &[(3, "new text")]);
                tx.fts_set_title(page(2), "Renamed")?;
                tx.fts_delete_page(page(2))?;
                tx.links_replace_from(page(1), &[link(3, 2, Some(4))])?;
                tx.links_delete_page(page(2))?;
                // The transaction sees its own writes.
                assert_eq!(ids(&search(tx, "new")), [(page(1), Some(block(3)))]);
                assert!(tx.links_from(page(2)).unwrap().is_empty());
                Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m == "boom"),
            "{err}"
        );

        assert!(db.connection().is_autocommit(), "the transaction is over");
        assert_eq!(sorted_ids(&search(&db, "old")), [(page(1), Some(block(1)))]);
        assert!(search(&db, "after").is_empty());
        assert!(search(&db, "renamed").is_empty());
        assert_eq!(
            sorted_ids(&search(&db, "before")),
            [(page(1), None)],
            "the old title row is back"
        );
        assert_eq!(
            db.fts_block_text(page(2), block(2)).unwrap().as_deref(),
            Some("second text")
        );
        assert_eq!(db.fts_page_count(space(1)).unwrap(), 2);
        assert_eq!(db.links_from(page(1)).unwrap(), [link(1, 2, None)]);
        assert_eq!(db.links_from(page(2)).unwrap(), [link(2, 1, None)]);
        assert_eq!(db.links_to(page(2)).unwrap(), [source(1, 1, None)]);
        // The word index still agrees with the text rows after the rollback.
        check_word_index(db.connection()).unwrap();
    }

    #[test]
    fn a_failing_transaction_rolls_back_the_clears_too() {
        let mut db = new_db();
        add_page(&db, 1, space(1));
        index(&db, 1, "Kept", &[(1, "text")]);
        db.links_replace_from(page(1), &[link(1, 2, None)]).unwrap();

        db.transaction(|tx| {
            tx.fts_clear()?;
            tx.links_clear()?;
            assert_eq!(tx.fts_page_count(space(1))?, 0);
            Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
        })
        .unwrap_err();

        assert_eq!(db.fts_page_count(space(1)).unwrap(), 1);
        assert_eq!(search(&db, "text").len(), 1);
        check_word_index(db.connection()).unwrap();
        assert_eq!(db.links_from(page(1)).unwrap(), [link(1, 2, None)]);
    }

    #[test]
    fn an_index_statement_that_fails_inside_a_transaction_rolls_the_whole_transaction_back() {
        let mut db = new_db();
        add_page(&db, 1, space(1));
        index(&db, 1, "Old title", &[(1, "old text")]);
        let err = db
            .transaction(|tx| {
                tx.fts_set_title(page(1), "New title")?;
                // With the table gone, the next statement fails.
                tx.tx.execute_batch("DROP TABLE links;")?;
                tx.links_clear()?;
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(ids(&search(&db, "old")).len(), 2);
        assert!(search(&db, "new").is_empty());
        // `links` is back too: DDL is transactional in SQLite.
        assert!(db.links_from(page(1)).unwrap().is_empty());
    }

    #[test]
    fn links_replace_from_on_a_bare_db_is_all_or_nothing() {
        let db = new_db();
        db.links_replace_from(page(1), &[link(1, 2, None)]).unwrap();
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE INSERT ON links
                 WHEN NEW.from_block = x'09090909090909090909090909090909'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();

        let err = db
            .links_replace_from(
                page(1),
                &[link(3, 4, None), link(9, 4, None), link(5, 4, None)],
            )
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            db.links_from(page(1)).unwrap(),
            [link(1, 2, None)],
            "the old rows are back and none of the new ones is there"
        );
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn fts_replace_page_on_a_bare_db_is_all_or_nothing() {
        let db = new_db();
        add_page(&db, 1, space(1));
        index(&db, 1, "Old title", &[(1, "old text")]);
        // Leave no room to grow: the delete and the title row fit, the huge block does not.
        let pages: i64 = db
            .connection()
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap();
        db.connection()
            .execute_batch(&format!("PRAGMA max_page_count = {pages};"))
            .unwrap();

        let huge = "x ".repeat(200_000);
        let err = db
            .fts_replace_page(page(1), "New title", &blocks(&[(2, &huge)]))
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");

        assert!(db.connection().is_autocommit());
        assert_eq!(
            db.fts_block_text(page(1), block(1)).unwrap().as_deref(),
            Some("old text"),
            "the old rows are back"
        );
        assert_eq!(db.fts_block_text(page(1), block(2)).unwrap(), None);
        assert_eq!(search(&db, "old").len(), 2, "old title row and block row");
        assert!(search(&db, "new").is_empty(), "the new title row is gone");
        assert_eq!(db.fts_page_count(space(1)).unwrap(), 1);
        check_word_index(db.connection()).unwrap();
    }

    #[test]
    fn the_index_works_through_a_trait_object() {
        // The core holds `&dyn`-friendly handles: the trait has to stay object safe.
        fn count_hits(q: &dyn TextIndexQueries) -> usize {
            q.fts_search(&params("needle")).unwrap().len()
        }
        let db = new_db();
        add_page(&db, 1, space(1));
        index(&db, 1, "Needle", &[(1, "a needle")]);
        assert_eq!(count_hits(&db), 2);
    }

    // ---- a random workload against a naive model ---------------------------------------

    /// What the index should hold, kept the naive way: page number to (title, blocks).
    type Model = BTreeMap<u8, (String, Vec<(u8, String)>)>;

    /// A row matches when every query word is the start of one of its words.
    fn row_matches(text: &str, query: &str) -> bool {
        query
            .split(' ')
            .all(|word| text.split(' ').any(|t| t.starts_with(word)))
    }

    /// `(page, block)` of every row of the model that matches the query.
    fn model_hits(model: &Model, query: &str) -> Vec<(PageId, Option<BlockId>)> {
        let mut hits = Vec::new();
        for (&n, (title, blocks)) in model {
            if row_matches(title, query) {
                hits.push((page(n), None));
            }
            for (b, text) in blocks {
                if row_matches(text, query) {
                    hits.push((page(n), Some(block(*b))));
                }
            }
        }
        hits.sort();
        hits
    }

    /// The pages that match a query as a whole: every word starts some word of the title or of a block.
    fn model_pages(model: &Model, query: &str) -> std::collections::BTreeSet<PageId> {
        model
            .iter()
            .filter(|(_, (title, blocks))| {
                query.split(' ').all(|word| {
                    row_matches(title, word)
                        || blocks.iter().any(|(_, text)| row_matches(text, word))
                })
            })
            .map(|(&n, _)| page(n))
            .collect()
    }

    fn assert_index_matches_model(q: &dyn Queries, model: &Model, step: usize) {
        for query in [
            "al",
            "alpha",
            "alphabet",
            "be",
            "del",
            "delta",
            "echo",
            "x",
            "alpha beta",
            "delta echo",
            "al be",
            "juliet india golf",
        ] {
            // The rows that hold every word come first, as the model has them; then one hit for each
            // other page whose words are spread, on a row of that page.
            let hits = search(q, query);
            let rows = model_hits(model, query);
            assert!(
                hits.len() >= rows.len(),
                "step {step}, query {query:?}: {hits:?}"
            );
            assert_eq!(
                sorted_ids(&hits[..rows.len()]),
                rows,
                "step {step}, query {query:?}"
            );
            let with_rows: std::collections::BTreeSet<PageId> =
                rows.iter().map(|(page_id, _)| *page_id).collect();
            let spread_pages: Vec<PageId> = sorted_pages(&hits[rows.len()..]);
            let expected: Vec<PageId> = model_pages(model, query)
                .into_iter()
                .filter(|page_id| !with_rows.contains(page_id))
                .collect();
            assert_eq!(spread_pages, expected, "step {step}, query {query:?}");
            for hit in &hits[rows.len()..] {
                let n = hit.page_id.as_bytes()[0];
                let on_page = model.get(&n).is_some_and(|(_, blocks)| {
                    hit.block_id
                        .is_none_or(|b| blocks.iter().any(|(id, _)| block(*id) == b))
                });
                assert!(on_page, "step {step}, query {query:?}: {hit:?}");
            }
        }
        for (&n, (_, blocks)) in model {
            for (b, text) in blocks {
                assert_eq!(
                    q.fts_block_text(page(n), block(*b)).unwrap().as_deref(),
                    Some(text.as_str()),
                    "step {step}"
                );
            }
        }
        assert_eq!(
            usize::try_from(q.fts_page_count(space(1)).unwrap()).unwrap(),
            model.len(),
            "step {step}"
        );
    }

    /// Replaces, renames and deletes pages at random, checking against the model as it
    /// goes.
    fn run_workload(q: &dyn Queries) {
        const WORDS: [&str; 12] = [
            "alpha", "alphabet", "beta", "gamma", "delta", "delight", "echo", "foxtrot", "golf",
            "hotel", "india", "juliet",
        ];
        let mut rng = Xorshift(0x1234_5678_9ABC_DEF1);
        let phrase = |rng: &mut Xorshift| {
            (0..1 + rng.below(5))
                .map(|_| WORDS[rng.below(WORDS.len())])
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut model = Model::new();
        for n in 1..=8 {
            add_page(q, n, space(1));
        }
        for step in 1..=400 {
            let n = u8::try_from(1 + rng.below(8)).unwrap();
            match rng.below(6) {
                0..=2 => {
                    let title = phrase(&mut rng);
                    let list: Vec<(u8, String)> = (0..rng.below(5))
                        .map(|i| (n * 10 + u8::try_from(i).unwrap(), phrase(&mut rng)))
                        .collect();
                    let refs: Vec<(u8, &str)> =
                        list.iter().map(|(b, t)| (*b, t.as_str())).collect();
                    index(q, n, &title, &refs);
                    model.insert(n, (title, list));
                }
                3 | 4 => {
                    let title = phrase(&mut rng);
                    q.fts_set_title(page(n), &title).unwrap();
                    model.entry(n).or_default().0 = title;
                }
                _ => {
                    q.fts_delete_page(page(n)).unwrap();
                    model.remove(&n);
                }
            }
            if step % 40 == 0 {
                assert_index_matches_model(q, &model, step);
            }
        }
        assert_index_matches_model(q, &model, 400);
    }

    /// FTS5 reads `fts_blocks` again, tokenizes it and compares the words with its own index;
    /// it fails when they differ (a stale or a missing entry).
    fn check_word_index(conn: &Connection) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
    }

    #[test]
    fn a_random_workload_keeps_the_index_equal_to_a_naive_model_and_consistent() {
        let db = new_db();
        run_workload(&db);
        check_word_index(db.connection()).unwrap();

        let mut db = new_db();
        db.transaction(|tx| {
            run_workload(tx);
            check_word_index(&tx.tx)?;
            Ok(())
        })
        .unwrap();
        check_word_index(db.connection()).unwrap();
    }

    #[test]
    fn the_triggers_keep_the_word_index_in_step_with_every_write_to_fts_blocks() {
        // The module only writes `fts_blocks`, and the triggers do the rest. Raw writes show
        // the contract: an insert, an update (of either text column) and a delete.
        let db = new_db();
        add_page(&db, 1, space(1));
        let conn = db.connection();
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body) \
             VALUES (?1, ?2, '', 'first words')",
            params![[1u8; 16], [7u8; 16]],
        )
        .unwrap();
        assert_eq!(ids(&search(&db, "first")), [(page(1), Some(block(7)))]);
        check_word_index(conn).unwrap();

        conn.execute(
            "UPDATE fts_blocks SET body = 'second words' WHERE block_id = ?1",
            [[7u8; 16]],
        )
        .unwrap();
        assert!(search(&db, "first").is_empty(), "the old words are gone");
        assert_eq!(ids(&search(&db, "second")), [(page(1), Some(block(7)))]);
        check_word_index(conn).unwrap();

        conn.execute(
            "UPDATE fts_blocks SET title = 'moved title' WHERE block_id = ?1",
            [[7u8; 16]],
        )
        .unwrap();
        assert_eq!(
            ids(&search(&db, "moved")),
            [(page(1), Some(block(7)))],
            "a title column of a block row is indexed too"
        );
        check_word_index(conn).unwrap();

        conn.execute("DELETE FROM fts_blocks WHERE block_id = ?1", [[7u8; 16]])
            .unwrap();
        assert!(search(&db, "second").is_empty());
        assert!(search(&db, "moved").is_empty());
        check_word_index(conn).unwrap();

        // Emptying the whole table fires the delete trigger once per row, and the word index
        // is empty afterwards.
        index(
            &db,
            1,
            "Many rows",
            &[(1, "alpha"), (2, "beta"), (3, "gamma")],
        );
        conn.execute("DELETE FROM fts_blocks", []).unwrap();
        assert!(search(&db, "alpha").is_empty());
        assert!(search(&db, "many").is_empty());
        check_word_index(conn).unwrap();
        let words: i64 = conn
            .query_row(
                "SELECT count(*) FROM fts WHERE fts MATCH 'alpha OR many'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(words, 0);
    }

    #[test]
    fn the_word_index_stores_no_text_of_its_own() {
        // External content: no `fts_content` table, so each text is stored once.
        let db = new_db();
        let tables: Vec<String> = db
            .connection()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'fts%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            tables,
            [
                "fts",
                "fts_blocks",
                "fts_config",
                "fts_data",
                "fts_docsize",
                "fts_idx"
            ]
        );
    }
}
