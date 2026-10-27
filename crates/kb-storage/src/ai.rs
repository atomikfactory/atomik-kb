//! The semantic index (schema v15, ADR-0037, `docs/architecture/ai.md` sections 4 and 5): the
//! tables `ai_pages`, `ai_chunks` and `ai_dirty`. Derived data, device-local, never synced, never
//! in a backup (a restore or a new device rebuilds it). Same conventions as [`crate::icon_refs`]:
//! every method exists on [`Db`] and [`Tx`] through [`AiIndexQueries`], both running the same
//! private functions over a connection, so a method behaves identically in either place; the
//! multi-statement methods of a [`Db`] run as one atomic unit. Ids are bound as 16-byte blobs and
//! hashes as 32-byte blobs. Rows are read as raw column values first and validated afterwards, so
//! a damaged row is a typed `InvalidData` error and never a panic. Page text never appears in an
//! error.
//!
//! # The tables
//!
//! - `ai_pages(page_id PRIMARY KEY, source_hash, model, chunks, indexed_at)`: one row per page whose
//!   chunks are stored. `source_hash` is the hash of the chunk hashes: equal to the one computed
//!   from the page now means there is nothing to embed again.
//! - `ai_chunks(page_id, seq, block_ids, hash, text, vector)`: the chunks of a page. `block_ids`
//!   is the blocks a chunk covers, 16 bytes each, concatenated (empty: the title alone); `hash` is
//!   the hash of the embedded text (a vector is reused by hash, `ai_chunks_hash`); `vector` is the
//!   unit-length vector as little-endian `f32`.
//! - `ai_dirty(page_id)`: the pages whose text changed since their chunks were stored. The three
//!   triggers of the migration add a page on every insert, update and delete of `fts_blocks`
//!   (the title rows and the property rows included), and the core removes a page once it is
//!   indexed, unchanged, trashed or gone.
//!
//! There is deliberately no foreign key to `pages_index`: a page that is trashed or purged keeps
//! its rows until the core's walk of `ai_dirty` reaches it, and every reader that cares joins
//! `pages_index` to leave the trashed pages out.

use kb_types::{BlockId, PageId, SpaceId};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::docs::{atomically, id_from};
use crate::text::{FtsBlock, FtsHit, SearchParams, fts_search_on};
use crate::{Db, StorageError, Tx};

/// Bytes of a hash (blake3) in `ai_pages.source_hash` and `ai_chunks.hash`.
pub const AI_HASH_BYTES: usize = 32;

/// A row of `ai_pages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiPageRow {
    /// The page.
    pub page_id: PageId,
    /// Hash of the page's chunk hashes, in order.
    pub source_hash: [u8; AI_HASH_BYTES],
    /// The embedding model of the vectors.
    pub model: String,
    /// How many chunk rows the page has.
    pub chunks: u32,
    /// When the chunks were stored, Unix ms.
    pub indexed_at: i64,
}

/// A row of `ai_chunks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiChunkRow {
    /// The page.
    pub page_id: PageId,
    /// Position in the page, from 0.
    pub seq: u32,
    /// The blocks the chunk covers, in order (empty: the title alone).
    pub block_ids: Vec<BlockId>,
    /// Hash of the embedded text.
    pub hash: [u8; AI_HASH_BYTES],
    /// The embedded text (the title line, then the blocks).
    pub text: String,
    /// The vector as little-endian `f32` bytes (a non-empty multiple of 4 bytes).
    pub vector: Vec<u8>,
}

/// What [`AiIndexQueries::ai_index_counts`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiIndexCounts {
    /// The model of the stored vectors (`None` when nothing is stored).
    pub model: Option<String>,
    /// Live pages that have an `ai_pages` row (of the model asked for, when one was) and are not dirty.
    pub pages_indexed: u32,
    /// Live (not trashed) pages.
    pub pages_total: u32,
    /// Stored chunk rows, of every page, trashed ones included.
    pub chunks: u32,
    /// Live pages waiting in `ai_dirty`.
    pub dirty: u32,
    /// The length of a stored vector, in floats.
    pub dimensions: Option<u32>,
}

/// Queries of the semantic index, available on a connection or inside a transaction. On a [`Db`]
/// every method is atomic by itself.
pub trait AiIndexQueries {
    // ---- the pages waiting (`ai_dirty`) ----

    /// Up to `limit` dirty pages with an id greater than `after` (all of them from the start when
    /// `None`), ordered by page id. Trashed and unknown pages are listed too: the core's walk
    /// drops them.
    fn ai_dirty_after(
        &self,
        after: Option<PageId>,
        limit: u32,
    ) -> Result<Vec<PageId>, StorageError>;
    /// How many rows `ai_dirty` has.
    fn ai_dirty_count(&self) -> Result<u32, StorageError>;
    /// Removes pages from `ai_dirty`. Pages that are not there and an empty list succeed. One
    /// atomic unit.
    fn ai_dirty_remove(&self, pages: &[PageId]) -> Result<(), StorageError>;
    /// Marks every page of `pages_index` dirty (a page already dirty stays so). Returns how many
    /// pages were added.
    fn ai_dirty_mark_all(&self) -> Result<u32, StorageError>;
    /// Marks dirty every live page of `pages_index` that has no `ai_pages` row of `model` (a page
    /// that was trashed while it was waiting and has been restored, a page that never had a text
    /// row). Returns how many pages were added.
    fn ai_dirty_mark_unindexed(&self, model: &str) -> Result<u32, StorageError>;

    // ---- the stored pages and chunks ----

    /// The `ai_pages` row of a page.
    fn ai_page_get(&self, page_id: PageId) -> Result<Option<AiPageRow>, StorageError>;
    /// Makes the page's rows exactly `page` and `chunks`: the old chunk rows go, the new ones come
    /// and the `ai_pages` row is written or replaced. `page.chunks` must equal `chunks.len()` and
    /// every chunk must be of `page.page_id` (`InvalidData` otherwise, nothing written), and every
    /// vector must be a non-empty multiple of 4 bytes. One atomic unit.
    fn ai_page_replace(&self, page: &AiPageRow, chunks: &[AiChunkRow]) -> Result<(), StorageError>;
    /// Removes the `ai_pages` row and the chunks of the given pages (trashed or purged pages).
    /// Unknown pages and an empty list succeed. One atomic unit.
    fn ai_pages_delete(&self, pages: &[PageId]) -> Result<(), StorageError>;
    /// Whether some `ai_pages` row is of another model than `model`.
    fn ai_has_other_model(&self, model: &str) -> Result<bool, StorageError>;
    /// Removes every `ai_pages` and `ai_chunks` row (`ai_dirty` stays). One atomic unit.
    fn ai_clear_index(&self) -> Result<(), StorageError>;
    /// Removes every row of the three tables. One atomic unit.
    fn ai_clear(&self) -> Result<(), StorageError>;
    /// For each hash, whether some stored chunk has it (`ai_chunks_hash`), in the order given.
    fn ai_hashes_stored(&self, hashes: &[[u8; AI_HASH_BYTES]]) -> Result<Vec<bool>, StorageError>;
    /// For each hash, the vector bytes of a stored chunk that has it, in the order given.
    fn ai_vectors_by_hash(
        &self,
        hashes: &[[u8; AI_HASH_BYTES]],
    ) -> Result<Vec<Option<Vec<u8>>>, StorageError>;
    /// Hands every chunk row to `visit`, ordered by page and `seq`, one at a time (the cache of the
    /// core is built from this without holding the whole table twice).
    fn ai_chunks_scan(&self, visit: &mut dyn FnMut(AiChunkRow)) -> Result<(), StorageError>;
    /// The chunks of a page, ordered by `seq`.
    fn ai_chunks_of(&self, page_id: PageId) -> Result<Vec<AiChunkRow>, StorageError>;
    /// The text of one chunk.
    fn ai_chunk_text(&self, page_id: PageId, seq: u32) -> Result<Option<String>, StorageError>;

    // ---- views for the core ----

    /// The numbers of `AiIndexStatus`. `current_model` is the model the index should be of
    /// (`pages_indexed` counts only the pages stored with it); `None` counts a row of any model.
    fn ai_index_counts(&self, current_model: Option<&str>) -> Result<AiIndexCounts, StorageError>;
    /// The live (not trashed) pages that are not in a template (ADR-0056) as (page, space),
    /// ordered by page id; only those of `space_id` when one is given.
    fn ai_live_pages(
        &self,
        space_id: Option<SpaceId>,
    ) -> Result<Vec<(PageId, SpaceId)>, StorageError>;
    /// The pages that are in a template (per `pages_index.in_template`) and still have an
    /// `ai_pages` row, ordered by page id: templates are not indexed, so the core drops what a
    /// page became a template with.
    fn ai_template_pages(&self) -> Result<Vec<PageId>, StorageError>;
    /// The body rows of a page's text index (`kind = 0`, with a block: no title row, no property
    /// row) in the order they were written, which is the order of the blocks in the page.
    fn ai_page_body(&self, page_id: PageId) -> Result<Vec<FtsBlock>, StorageError>;
    /// [`TextIndexQueries::fts_search`](crate::TextIndexQueries::fts_search) restricted to the
    /// rows of one page (live pages only, as for every search): the page scope of retrieval.
    /// `params.space_id` still applies.
    fn ai_search_in_page(
        &self,
        page_id: PageId,
        params: &SearchParams<'_>,
    ) -> Result<Vec<FtsHit>, StorageError>;
}

impl AiIndexQueries for Db {
    fn ai_dirty_after(
        &self,
        after: Option<PageId>,
        limit: u32,
    ) -> Result<Vec<PageId>, StorageError> {
        ai_dirty_after_on(&self.conn, after, limit)
    }
    fn ai_dirty_count(&self) -> Result<u32, StorageError> {
        ai_dirty_count_on(&self.conn)
    }
    fn ai_dirty_remove(&self, pages: &[PageId]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| ai_dirty_remove_on(conn, pages))
    }
    fn ai_dirty_mark_all(&self) -> Result<u32, StorageError> {
        ai_dirty_mark_all_on(&self.conn)
    }
    fn ai_dirty_mark_unindexed(&self, model: &str) -> Result<u32, StorageError> {
        ai_dirty_mark_unindexed_on(&self.conn, model)
    }
    fn ai_page_get(&self, page_id: PageId) -> Result<Option<AiPageRow>, StorageError> {
        ai_page_get_on(&self.conn, page_id)
    }
    fn ai_page_replace(&self, page: &AiPageRow, chunks: &[AiChunkRow]) -> Result<(), StorageError> {
        check_replacement(page, chunks)?;
        atomically(&self.conn, |conn| ai_page_replace_on(conn, page, chunks))
    }
    fn ai_pages_delete(&self, pages: &[PageId]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| ai_pages_delete_on(conn, pages))
    }
    fn ai_has_other_model(&self, model: &str) -> Result<bool, StorageError> {
        ai_has_other_model_on(&self.conn, model)
    }
    fn ai_clear_index(&self) -> Result<(), StorageError> {
        atomically(&self.conn, ai_clear_index_on)
    }
    fn ai_clear(&self) -> Result<(), StorageError> {
        atomically(&self.conn, ai_clear_on)
    }
    fn ai_hashes_stored(&self, hashes: &[[u8; AI_HASH_BYTES]]) -> Result<Vec<bool>, StorageError> {
        ai_hashes_stored_on(&self.conn, hashes)
    }
    fn ai_vectors_by_hash(
        &self,
        hashes: &[[u8; AI_HASH_BYTES]],
    ) -> Result<Vec<Option<Vec<u8>>>, StorageError> {
        ai_vectors_by_hash_on(&self.conn, hashes)
    }
    fn ai_chunks_scan(&self, visit: &mut dyn FnMut(AiChunkRow)) -> Result<(), StorageError> {
        ai_chunks_scan_on(&self.conn, visit)
    }
    fn ai_chunks_of(&self, page_id: PageId) -> Result<Vec<AiChunkRow>, StorageError> {
        ai_chunks_of_on(&self.conn, page_id)
    }
    fn ai_chunk_text(&self, page_id: PageId, seq: u32) -> Result<Option<String>, StorageError> {
        ai_chunk_text_on(&self.conn, page_id, seq)
    }
    fn ai_index_counts(&self, current_model: Option<&str>) -> Result<AiIndexCounts, StorageError> {
        ai_index_counts_on(&self.conn, current_model)
    }
    fn ai_live_pages(
        &self,
        space_id: Option<SpaceId>,
    ) -> Result<Vec<(PageId, SpaceId)>, StorageError> {
        ai_live_pages_on(&self.conn, space_id)
    }
    fn ai_template_pages(&self) -> Result<Vec<PageId>, StorageError> {
        ai_template_pages_on(&self.conn)
    }
    fn ai_page_body(&self, page_id: PageId) -> Result<Vec<FtsBlock>, StorageError> {
        ai_page_body_on(&self.conn, page_id)
    }
    fn ai_search_in_page(
        &self,
        page_id: PageId,
        params: &SearchParams<'_>,
    ) -> Result<Vec<FtsHit>, StorageError> {
        fts_search_on(&self.conn, params, Some(page_id))
    }
}

impl AiIndexQueries for Tx<'_> {
    fn ai_dirty_after(
        &self,
        after: Option<PageId>,
        limit: u32,
    ) -> Result<Vec<PageId>, StorageError> {
        ai_dirty_after_on(&self.tx, after, limit)
    }
    fn ai_dirty_count(&self) -> Result<u32, StorageError> {
        ai_dirty_count_on(&self.tx)
    }
    fn ai_dirty_remove(&self, pages: &[PageId]) -> Result<(), StorageError> {
        ai_dirty_remove_on(&self.tx, pages)
    }
    fn ai_dirty_mark_all(&self) -> Result<u32, StorageError> {
        ai_dirty_mark_all_on(&self.tx)
    }
    fn ai_dirty_mark_unindexed(&self, model: &str) -> Result<u32, StorageError> {
        ai_dirty_mark_unindexed_on(&self.tx, model)
    }
    fn ai_page_get(&self, page_id: PageId) -> Result<Option<AiPageRow>, StorageError> {
        ai_page_get_on(&self.tx, page_id)
    }
    fn ai_page_replace(&self, page: &AiPageRow, chunks: &[AiChunkRow]) -> Result<(), StorageError> {
        check_replacement(page, chunks)?;
        ai_page_replace_on(&self.tx, page, chunks)
    }
    fn ai_pages_delete(&self, pages: &[PageId]) -> Result<(), StorageError> {
        ai_pages_delete_on(&self.tx, pages)
    }
    fn ai_has_other_model(&self, model: &str) -> Result<bool, StorageError> {
        ai_has_other_model_on(&self.tx, model)
    }
    fn ai_clear_index(&self) -> Result<(), StorageError> {
        ai_clear_index_on(&self.tx)
    }
    fn ai_clear(&self) -> Result<(), StorageError> {
        ai_clear_on(&self.tx)
    }
    fn ai_hashes_stored(&self, hashes: &[[u8; AI_HASH_BYTES]]) -> Result<Vec<bool>, StorageError> {
        ai_hashes_stored_on(&self.tx, hashes)
    }
    fn ai_vectors_by_hash(
        &self,
        hashes: &[[u8; AI_HASH_BYTES]],
    ) -> Result<Vec<Option<Vec<u8>>>, StorageError> {
        ai_vectors_by_hash_on(&self.tx, hashes)
    }
    fn ai_chunks_scan(&self, visit: &mut dyn FnMut(AiChunkRow)) -> Result<(), StorageError> {
        ai_chunks_scan_on(&self.tx, visit)
    }
    fn ai_chunks_of(&self, page_id: PageId) -> Result<Vec<AiChunkRow>, StorageError> {
        ai_chunks_of_on(&self.tx, page_id)
    }
    fn ai_chunk_text(&self, page_id: PageId, seq: u32) -> Result<Option<String>, StorageError> {
        ai_chunk_text_on(&self.tx, page_id, seq)
    }
    fn ai_index_counts(&self, current_model: Option<&str>) -> Result<AiIndexCounts, StorageError> {
        ai_index_counts_on(&self.tx, current_model)
    }
    fn ai_live_pages(
        &self,
        space_id: Option<SpaceId>,
    ) -> Result<Vec<(PageId, SpaceId)>, StorageError> {
        ai_live_pages_on(&self.tx, space_id)
    }
    fn ai_template_pages(&self) -> Result<Vec<PageId>, StorageError> {
        ai_template_pages_on(&self.tx)
    }
    fn ai_page_body(&self, page_id: PageId) -> Result<Vec<FtsBlock>, StorageError> {
        ai_page_body_on(&self.tx, page_id)
    }
    fn ai_search_in_page(
        &self,
        page_id: PageId,
        params: &SearchParams<'_>,
    ) -> Result<Vec<FtsHit>, StorageError> {
        fts_search_on(&self.tx, params, Some(page_id))
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// The first page of the dirty walk: the primary key in order.
const DIRTY_FIRST_SQL: &str = "SELECT page_id FROM ai_dirty ORDER BY page_id LIMIT ?1";

/// The next pages of the dirty walk: a range of the primary key.
const DIRTY_AFTER_SQL: &str =
    "SELECT page_id FROM ai_dirty WHERE page_id > ?1 ORDER BY page_id LIMIT ?2";

/// By primary key.
const DIRTY_DELETE_SQL: &str = "DELETE FROM ai_dirty WHERE page_id = ?1";

/// Every page of the index of pages.
const DIRTY_MARK_ALL_SQL: &str =
    "INSERT OR IGNORE INTO ai_dirty (page_id) SELECT page_id FROM pages_index";

/// The live pages that no `ai_pages` row of the model covers (each one looks its row up by primary key).
const DIRTY_MARK_UNINDEXED_SQL: &str = "INSERT OR IGNORE INTO ai_dirty (page_id) \
     SELECT p.page_id FROM pages_index AS p \
      WHERE p.trashed_at IS NULL AND p.in_template = 0 \
        AND NOT EXISTS (SELECT 1 FROM ai_pages AS a WHERE a.page_id = p.page_id AND a.model = ?1)";

/// By primary key.
const PAGE_GET_SQL: &str =
    "SELECT source_hash, model, chunks, indexed_at FROM ai_pages WHERE page_id = ?1";

/// By primary key.
const PAGE_UPSERT_SQL: &str = "INSERT INTO ai_pages (page_id, source_hash, model, chunks, indexed_at) \
     VALUES (?1, ?2, ?3, ?4, ?5) \
     ON CONFLICT (page_id) DO UPDATE SET \
       source_hash = excluded.source_hash, model = excluded.model, \
       chunks = excluded.chunks, indexed_at = excluded.indexed_at";

/// A range of the primary key.
const CHUNKS_DELETE_PAGE_SQL: &str = "DELETE FROM ai_chunks WHERE page_id = ?1";

/// By primary key.
const PAGE_DELETE_SQL: &str = "DELETE FROM ai_pages WHERE page_id = ?1";

const CHUNK_INSERT_SQL: &str = "INSERT INTO ai_chunks (page_id, seq, block_ids, hash, text, vector) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

/// The columns of `ai_chunks` in the order [`ChunkColumns::read`] expects.
const CHUNK_COLUMNS: &str = "page_id, seq, block_ids, hash, text, vector";

/// Through `ai_chunks_hash` (the index covers the question).
const HASH_STORED_SQL: &str = "SELECT EXISTS (SELECT 1 FROM ai_chunks WHERE hash = ?1)";

/// Through `ai_chunks_hash`.
const VECTOR_BY_HASH_SQL: &str = "SELECT vector FROM ai_chunks WHERE hash = ?1 LIMIT 1";

/// By primary key.
const CHUNK_TEXT_SQL: &str = "SELECT text FROM ai_chunks WHERE page_id = ?1 AND seq = ?2";

/// The body rows of a page through `fts_blocks_page`, in the order they were written.
const PAGE_BODY_SQL: &str = "SELECT block_id, body FROM fts_blocks \
      WHERE page_id = ?1 AND kind = 0 AND block_id IS NOT NULL ORDER BY id";

/// The live pages of a space that are not in a template (`pages_index_space_trashed`).
const LIVE_PAGES_OF_SPACE_SQL: &str = "SELECT page_id, space_id FROM pages_index \
      WHERE space_id = ?1 AND trashed_at IS NULL AND in_template = 0 ORDER BY page_id";

const LIVE_PAGES_SQL: &str = "SELECT page_id, space_id FROM pages_index \
      WHERE trashed_at IS NULL AND in_template = 0 ORDER BY page_id";

/// Live pages (templates are not indexed, ADR-0056) that are stored (with the model when `?1` is not
/// NULL) and not waiting.
const PAGES_INDEXED_SQL: &str = "SELECT count(*) FROM pages_index AS p \
      JOIN ai_pages AS a ON a.page_id = p.page_id \
     WHERE p.trashed_at IS NULL AND p.in_template = 0 \
       AND (?1 IS NULL OR a.model = ?1) \
       AND NOT EXISTS (SELECT 1 FROM ai_dirty AS d WHERE d.page_id = p.page_id)";

/// Live pages that wait (not those in a template).
const PAGES_DIRTY_SQL: &str = "SELECT count(*) FROM ai_dirty AS d \
      JOIN pages_index AS p ON p.page_id = d.page_id \
     WHERE p.trashed_at IS NULL AND p.in_template = 0";

/// The pages that have an `ai_pages` row and are in a template (ordered by the primary key of
/// `ai_pages`, each one looks its page up by primary key in `pages_index`).
const TEMPLATE_PAGES_SQL: &str = "SELECT a.page_id FROM ai_pages AS a \
      JOIN pages_index AS p ON p.page_id = a.page_id \
     WHERE p.in_template = 1 ORDER BY a.page_id";

fn count_to_u32(value: i64, what: &str) -> Result<u32, StorageError> {
    u32::try_from(value).map_err(|_| StorageError::InvalidData(format!("{what} is out of range")))
}

fn hash_from(bytes: &[u8], what: &str) -> Result<[u8; AI_HASH_BYTES], StorageError> {
    <[u8; AI_HASH_BYTES]>::try_from(bytes)
        .map_err(|_| StorageError::InvalidData(format!("{what} is not {AI_HASH_BYTES} bytes long")))
}

/// A vector is little-endian `f32`s: a non-empty multiple of 4 bytes.
fn check_vector(bytes: &[u8]) -> Result<(), StorageError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return Err(StorageError::InvalidData(
            "a stored vector is not a whole number of floats".to_owned(),
        ));
    }
    Ok(())
}

fn block_ids_from(bytes: &[u8]) -> Result<Vec<BlockId>, StorageError> {
    let (ids, rest) = bytes.as_chunks::<16>();
    if !rest.is_empty() {
        return Err(StorageError::InvalidData(
            "chunk block ids are not a whole number of 16-byte ids".to_owned(),
        ));
    }
    Ok(ids.iter().map(|id| BlockId::from_bytes(*id)).collect())
}

fn block_ids_to_bytes(ids: &[BlockId]) -> Vec<u8> {
    ids.iter().flat_map(|id| id.to_bytes()).collect()
}

/// A replacement names one page, and its row counts the chunks that come with it.
fn check_replacement(page: &AiPageRow, chunks: &[AiChunkRow]) -> Result<(), StorageError> {
    if usize::try_from(page.chunks).ok() != Some(chunks.len()) {
        return Err(StorageError::InvalidData(
            "the page row does not count its chunk rows".to_owned(),
        ));
    }
    if chunks.iter().any(|chunk| chunk.page_id != page.page_id) {
        return Err(StorageError::InvalidData(
            "a chunk row belongs to another page".to_owned(),
        ));
    }
    chunks
        .iter()
        .try_for_each(|chunk| check_vector(&chunk.vector))
}

fn ai_dirty_after_on(
    conn: &Connection,
    after: Option<PageId>,
    limit: u32,
) -> Result<Vec<PageId>, StorageError> {
    let rows = match after {
        None => conn
            .prepare(DIRTY_FIRST_SQL)?
            .query_map([i64::from(limit)], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?,
        Some(after) => conn
            .prepare(DIRTY_AFTER_SQL)?
            .query_map(params![after.to_bytes(), i64::from(limit)], |row| {
                row.get::<_, Vec<u8>>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?,
    };
    rows.iter()
        .map(|bytes| Ok(PageId::from_bytes(id_from(bytes, "dirty page id")?)))
        .collect()
}

fn ai_dirty_count_on(conn: &Connection) -> Result<u32, StorageError> {
    let count: i64 = conn.query_one("SELECT count(*) FROM ai_dirty", [], |row| row.get(0))?;
    count_to_u32(count, "dirty page count")
}

fn ai_dirty_remove_on(conn: &Connection, pages: &[PageId]) -> Result<(), StorageError> {
    let mut delete = conn.prepare(DIRTY_DELETE_SQL)?;
    for page_id in pages {
        delete.execute([page_id.to_bytes()])?;
    }
    Ok(())
}

fn ai_dirty_mark_all_on(conn: &Connection) -> Result<u32, StorageError> {
    let added = conn.execute(DIRTY_MARK_ALL_SQL, [])?;
    u32::try_from(added)
        .map_err(|_| StorageError::InvalidData("added dirty page count is out of range".to_owned()))
}

fn ai_dirty_mark_unindexed_on(conn: &Connection, model: &str) -> Result<u32, StorageError> {
    let added = conn.execute(DIRTY_MARK_UNINDEXED_SQL, [model])?;
    u32::try_from(added)
        .map_err(|_| StorageError::InvalidData("added dirty page count is out of range".to_owned()))
}

fn ai_page_get_on(conn: &Connection, page_id: PageId) -> Result<Option<AiPageRow>, StorageError> {
    let raw: Option<(Vec<u8>, String, i64, i64)> = conn
        .query_row(PAGE_GET_SQL, [page_id.to_bytes()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    raw.map(|(source_hash, model, chunks, indexed_at)| {
        Ok(AiPageRow {
            page_id,
            source_hash: hash_from(&source_hash, "page source hash")?,
            model,
            chunks: count_to_u32(chunks, "page chunk count")?,
            indexed_at,
        })
    })
    .transpose()
}

fn ai_page_replace_on(
    conn: &Connection,
    page: &AiPageRow,
    chunks: &[AiChunkRow],
) -> Result<(), StorageError> {
    conn.execute(CHUNKS_DELETE_PAGE_SQL, [page.page_id.to_bytes()])?;
    let mut insert = conn.prepare(CHUNK_INSERT_SQL)?;
    for chunk in chunks {
        insert.execute(params![
            chunk.page_id.to_bytes(),
            i64::from(chunk.seq),
            block_ids_to_bytes(&chunk.block_ids),
            chunk.hash,
            chunk.text,
            chunk.vector,
        ])?;
    }
    conn.execute(
        PAGE_UPSERT_SQL,
        params![
            page.page_id.to_bytes(),
            page.source_hash,
            page.model,
            i64::from(page.chunks),
            page.indexed_at,
        ],
    )?;
    Ok(())
}

fn ai_pages_delete_on(conn: &Connection, pages: &[PageId]) -> Result<(), StorageError> {
    let mut delete_chunks = conn.prepare(CHUNKS_DELETE_PAGE_SQL)?;
    let mut delete_page = conn.prepare(PAGE_DELETE_SQL)?;
    for page_id in pages {
        delete_chunks.execute([page_id.to_bytes()])?;
        delete_page.execute([page_id.to_bytes()])?;
    }
    Ok(())
}

fn ai_has_other_model_on(conn: &Connection, model: &str) -> Result<bool, StorageError> {
    Ok(conn.query_one(
        "SELECT EXISTS (SELECT 1 FROM ai_pages WHERE model <> ?1)",
        [model],
        |row| row.get(0),
    )?)
}

fn ai_clear_index_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM ai_chunks", [])?;
    conn.execute("DELETE FROM ai_pages", [])?;
    Ok(())
}

fn ai_clear_on(conn: &Connection) -> Result<(), StorageError> {
    ai_clear_index_on(conn)?;
    conn.execute("DELETE FROM ai_dirty", [])?;
    Ok(())
}

fn ai_hashes_stored_on(
    conn: &Connection,
    hashes: &[[u8; AI_HASH_BYTES]],
) -> Result<Vec<bool>, StorageError> {
    let mut stmt = conn.prepare(HASH_STORED_SQL)?;
    hashes
        .iter()
        .map(|hash| Ok(stmt.query_row([hash], |row| row.get::<_, bool>(0))?))
        .collect()
}

fn ai_vectors_by_hash_on(
    conn: &Connection,
    hashes: &[[u8; AI_HASH_BYTES]],
) -> Result<Vec<Option<Vec<u8>>>, StorageError> {
    let mut stmt = conn.prepare(VECTOR_BY_HASH_SQL)?;
    hashes
        .iter()
        .map(|hash| {
            let vector: Option<Vec<u8>> = stmt.query_row([hash], |row| row.get(0)).optional()?;
            if let Some(vector) = &vector {
                check_vector(vector)?;
            }
            Ok(vector)
        })
        .collect()
}

/// Raw column values of an `ai_chunks` row.
struct ChunkColumns {
    page_id: Vec<u8>,
    seq: i64,
    block_ids: Vec<u8>,
    hash: Vec<u8>,
    text: String,
    vector: Vec<u8>,
}

impl ChunkColumns {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            page_id: row.get(0)?,
            seq: row.get(1)?,
            block_ids: row.get(2)?,
            hash: row.get(3)?,
            text: row.get(4)?,
            vector: row.get(5)?,
        })
    }

    fn decode(self) -> Result<AiChunkRow, StorageError> {
        check_vector(&self.vector)?;
        Ok(AiChunkRow {
            page_id: PageId::from_bytes(id_from(&self.page_id, "chunk page id")?),
            seq: count_to_u32(self.seq, "chunk position")?,
            block_ids: block_ids_from(&self.block_ids)?,
            hash: hash_from(&self.hash, "chunk hash")?,
            text: self.text,
            vector: self.vector,
        })
    }
}

fn ai_chunks_scan_on(
    conn: &Connection,
    visit: &mut dyn FnMut(AiChunkRow),
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CHUNK_COLUMNS} FROM ai_chunks ORDER BY page_id, seq"
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        visit(ChunkColumns::read(row)?.decode()?);
    }
    Ok(())
}

fn ai_chunks_of_on(conn: &Connection, page_id: PageId) -> Result<Vec<AiChunkRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CHUNK_COLUMNS} FROM ai_chunks WHERE page_id = ?1 ORDER BY seq"
    ))?;
    let rows = stmt
        .query_map([page_id.to_bytes()], ChunkColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(ChunkColumns::decode).collect()
}

fn ai_chunk_text_on(
    conn: &Connection,
    page_id: PageId,
    seq: u32,
) -> Result<Option<String>, StorageError> {
    Ok(conn
        .query_row(
            CHUNK_TEXT_SQL,
            params![page_id.to_bytes(), i64::from(seq)],
            |row| row.get(0),
        )
        .optional()?)
}

fn ai_index_counts_on(
    conn: &Connection,
    current_model: Option<&str>,
) -> Result<AiIndexCounts, StorageError> {
    let count = |sql: &str, what: &str| -> Result<u32, StorageError> {
        count_to_u32(conn.query_one(sql, [], |row| row.get::<_, i64>(0))?, what)
    };
    let pages_indexed: i64 =
        conn.query_one(PAGES_INDEXED_SQL, [current_model], |row| row.get(0))?;
    let model: Option<String> = conn
        .query_row(
            "SELECT model FROM ai_pages ORDER BY page_id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let dimensions: Option<i64> = conn
        .query_row(
            "SELECT length(vector) / 4 FROM ai_chunks LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(AiIndexCounts {
        model,
        pages_indexed: count_to_u32(pages_indexed, "indexed page count")?,
        pages_total: count(
            "SELECT count(*) FROM pages_index WHERE trashed_at IS NULL AND in_template = 0",
            "live page count",
        )?,
        chunks: count("SELECT count(*) FROM ai_chunks", "chunk count")?,
        dirty: count(PAGES_DIRTY_SQL, "dirty page count")?,
        dimensions: dimensions
            .map(|d| count_to_u32(d, "vector dimension"))
            .transpose()?,
    })
}

fn ai_live_pages_on(
    conn: &Connection,
    space_id: Option<SpaceId>,
) -> Result<Vec<(PageId, SpaceId)>, StorageError> {
    let read = |row: &Row<'_>| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?));
    let rows = match space_id {
        Some(space_id) => conn
            .prepare(LIVE_PAGES_OF_SPACE_SQL)?
            .query_map([space_id.to_bytes()], read)?
            .collect::<Result<Vec<_>, _>>()?,
        None => conn
            .prepare(LIVE_PAGES_SQL)?
            .query_map([], read)?
            .collect::<Result<Vec<_>, _>>()?,
    };
    rows.iter()
        .map(|(page, space)| {
            Ok((
                PageId::from_bytes(id_from(page, "indexed page id")?),
                SpaceId::from_bytes(id_from(space, "indexed page space id")?),
            ))
        })
        .collect()
}

fn ai_template_pages_on(conn: &Connection) -> Result<Vec<PageId>, StorageError> {
    let rows = conn
        .prepare(TEMPLATE_PAGES_SQL)?
        .query_map([], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.iter()
        .map(|page| Ok(PageId::from_bytes(id_from(page, "indexed page id")?)))
        .collect()
}

fn ai_page_body_on(conn: &Connection, page_id: PageId) -> Result<Vec<FtsBlock>, StorageError> {
    let rows = conn
        .prepare(PAGE_BODY_SQL)?
        .query_map([page_id.to_bytes()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(block, text)| {
            Ok(FtsBlock {
                block_id: BlockId::from_bytes(id_from(&block, "indexed block id")?),
                text,
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{DocQueries, PageIndexRow, TextIndexQueries};

    /// What a scenario needs of a connection or a transaction: both implement every trait.
    trait Queries: AiIndexQueries + DocQueries + TextIndexQueries {}
    impl<T: AiIndexQueries + DocQueries + TextIndexQueries> Queries for T {}

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

    const fn hash(n: u8) -> [u8; AI_HASH_BYTES] {
        [n; AI_HASH_BYTES]
    }

    /// A vector of `floats` floats, every byte `n`.
    fn vector(n: u8, floats: usize) -> Vec<u8> {
        vec![n; floats * 4]
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that both
    /// impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries, &Connection)) {
        let db = new_db();
        scenario(&db, db.connection());
        let mut db = new_db();
        db.transaction(|tx| {
            scenario(tx, &tx.tx);
            Ok(())
        })
        .unwrap();
    }

    fn page_row(n: u8, space_id: SpaceId, trashed_at: Option<i64>) -> PageIndexRow {
        PageIndexRow {
            page_id: page(n),
            space_id,
            parent_id: None,
            title: format!("page {n}"),
            trashed_at,
            updated_at: 100 + i64::from(n),
            in_template: false,
        }
    }

    fn add_page(q: &dyn Queries, n: u8, space_id: SpaceId) {
        q.pages_index_upsert(&[page_row(n, space_id, None)])
            .unwrap();
    }

    fn trash_page(q: &dyn Queries, n: u8, space_id: SpaceId) {
        q.pages_index_upsert(&[page_row(n, space_id, Some(9))])
            .unwrap();
    }

    fn index(q: &dyn Queries, n: u8, blocks: &[(u8, &str)]) {
        let blocks: Vec<FtsBlock> = blocks
            .iter()
            .map(|(b, text)| FtsBlock {
                block_id: block(*b),
                text: (*text).to_owned(),
            })
            .collect();
        q.fts_replace_page(page(n), &format!("page {n}"), &blocks)
            .unwrap();
    }

    fn dirty(q: &dyn Queries) -> Vec<PageId> {
        q.ai_dirty_after(None, 1_000).unwrap()
    }

    fn chunk(n: u8, seq: u32, blocks: &[u8], text: &str, floats: usize) -> AiChunkRow {
        AiChunkRow {
            page_id: page(n),
            seq,
            block_ids: blocks.iter().map(|b| block(*b)).collect(),
            hash: hash(n.wrapping_mul(16).wrapping_add(u8::try_from(seq).unwrap())),
            text: text.to_owned(),
            vector: vector(n, floats),
        }
    }

    fn store(q: &dyn Queries, n: u8, model: &str, chunks: &[AiChunkRow]) {
        q.ai_page_replace(
            &AiPageRow {
                page_id: page(n),
                source_hash: hash(200 + n),
                model: model.to_owned(),
                chunks: u32::try_from(chunks.len()).unwrap(),
                indexed_at: 1_000 + i64::from(n),
            },
            chunks,
        )
        .unwrap();
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    // ---- ai_dirty ----

    #[test]
    fn a_page_becomes_dirty_with_every_change_of_its_text_rows() {
        on_db_and_tx(|q, _| {
            assert!(dirty(q).is_empty());
            index(q, 2, &[(1, "text")]);
            assert_eq!(dirty(q), [page(2)], "a page indexed");
            q.ai_dirty_remove(&[page(2)]).unwrap();
            assert!(dirty(q).is_empty());

            q.fts_set_title(page(2), "renamed").unwrap();
            assert_eq!(dirty(q), [page(2)], "a title row updated");
            q.ai_dirty_remove(&[page(2)]).unwrap();

            q.fts_replace_properties(page(2), &[]).unwrap();
            assert!(dirty(q).is_empty(), "no row inserted or removed");
            q.fts_replace_properties(
                page(2),
                &[FtsBlock {
                    block_id: block(50),
                    text: "urgent".to_owned(),
                }],
            )
            .unwrap();
            assert_eq!(dirty(q), [page(2)], "a property row inserted");
            q.ai_dirty_remove(&[page(2)]).unwrap();

            q.fts_delete_page(page(2)).unwrap();
            assert_eq!(dirty(q), [page(2)], "rows deleted (the page is gone)");
        });
    }

    #[test]
    fn the_dirty_pages_come_in_page_id_order_after_a_cursor() {
        on_db_and_tx(|q, _| {
            for n in [5, 1, 9, 3, 7] {
                index(q, n, &[(1, "text")]);
            }
            assert_eq!(
                dirty(q),
                [page(1), page(3), page(5), page(7), page(9)],
                "ordered by page id however they were written"
            );
            assert_eq!(q.ai_dirty_count().unwrap(), 5);
            assert_eq!(q.ai_dirty_after(None, 2).unwrap(), [page(1), page(3)]);
            assert_eq!(
                q.ai_dirty_after(Some(page(3)), 2).unwrap(),
                [page(5), page(7)]
            );
            assert_eq!(q.ai_dirty_after(Some(page(7)), 2).unwrap(), [page(9)]);
            assert!(q.ai_dirty_after(Some(page(9)), 2).unwrap().is_empty());
            assert!(
                q.ai_dirty_after(Some(PageId::from_bytes([2; 16])), 0)
                    .unwrap()
                    .is_empty(),
                "a limit of 0 is an empty list"
            );

            q.ai_dirty_remove(&[page(3), page(4), page(3)]).unwrap();
            q.ai_dirty_remove(&[]).unwrap();
            assert_eq!(q.ai_dirty_count().unwrap(), 4);
            assert_eq!(
                q.ai_dirty_after(Some(page(1)), 10).unwrap(),
                [page(5), page(7), page(9)]
            );
        });
    }

    #[test]
    fn marking_every_page_dirty_covers_the_index_of_pages_and_keeps_what_waits() {
        on_db_and_tx(|q, _| {
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            trash_page(q, 3, space(1));
            index(q, 2, &[(1, "text")]);
            assert_eq!(dirty(q), [page(2)]);
            assert_eq!(q.ai_dirty_mark_all().unwrap(), 2, "pages 1 and 3 are added");
            assert_eq!(dirty(q), [page(1), page(2), page(3)]);
            assert_eq!(q.ai_dirty_mark_all().unwrap(), 0, "nothing more to add");
        });
    }

    #[test]
    fn marking_the_unindexed_pages_dirty_leaves_the_live_pages_that_are_stored() {
        on_db_and_tx(|q, _| {
            for n in [1, 2, 3, 4] {
                add_page(q, n, space(1));
            }
            trash_page(q, 4, space(1));
            store(q, 1, "model-a", &[chunk(1, 0, &[1], "one", 4)]);
            store(q, 2, "model-b", &[chunk(2, 0, &[1], "two", 4)]);
            assert_eq!(
                q.ai_dirty_mark_unindexed("model-a").unwrap(),
                2,
                "page 2 is stored with another model, page 3 is not stored; 4 is trashed"
            );
            assert_eq!(dirty(q), [page(2), page(3)]);
            assert_eq!(q.ai_dirty_mark_unindexed("model-a").unwrap(), 0);
        });
    }

    // ---- ai_pages and ai_chunks ----

    #[test]
    fn a_page_is_stored_replaced_and_read_back() {
        on_db_and_tx(|q, conn| {
            assert_eq!(q.ai_page_get(page(1)).unwrap(), None);
            let first = [
                chunk(1, 0, &[1, 2], "page 1\nfirst", 4),
                chunk(1, 1, &[], "page 1", 4),
            ];
            store(q, 1, "nomic", &first);
            let row = q.ai_page_get(page(1)).unwrap().unwrap();
            assert_eq!(
                row,
                AiPageRow {
                    page_id: page(1),
                    source_hash: hash(201),
                    model: "nomic".to_owned(),
                    chunks: 2,
                    indexed_at: 1_001,
                }
            );
            assert_eq!(q.ai_chunks_of(page(1)).unwrap(), first);
            assert_eq!(
                q.ai_chunk_text(page(1), 0).unwrap().as_deref(),
                Some("page 1\nfirst")
            );
            assert_eq!(q.ai_chunk_text(page(1), 2).unwrap(), None);
            assert_eq!(q.ai_chunk_text(page(9), 0).unwrap(), None);

            // Another page, then the first one again with one chunk fewer: the old rows are gone.
            store(q, 2, "nomic", &[chunk(2, 0, &[3], "page 2\nother", 4)]);
            let second = [chunk(1, 0, &[1], "page 1\nchanged", 4)];
            store(q, 1, "nomic2", &second);
            assert_eq!(q.ai_chunks_of(page(1)).unwrap(), second);
            let row = q.ai_page_get(page(1)).unwrap().unwrap();
            assert_eq!((row.model.as_str(), row.chunks), ("nomic2", 1));
            assert_eq!(q.ai_chunks_of(page(2)).unwrap().len(), 1, "page 2 stays");
            assert_eq!(count(conn, "ai_pages"), 2);
            assert_eq!(count(conn, "ai_chunks"), 2);

            // A page without chunks has a row and nothing else.
            store(q, 3, "nomic", &[]);
            assert_eq!(q.ai_page_get(page(3)).unwrap().unwrap().chunks, 0);
            assert!(q.ai_chunks_of(page(3)).unwrap().is_empty());
        });
    }

    #[test]
    fn a_replacement_that_does_not_add_up_changes_nothing() {
        on_db_and_tx(|q, conn| {
            store(q, 1, "m", &[chunk(1, 0, &[1], "kept", 4)]);
            let row = |chunks| AiPageRow {
                page_id: page(1),
                source_hash: hash(9),
                model: "m".to_owned(),
                chunks,
                indexed_at: 5,
            };
            let mut other_page = chunk(1, 0, &[1], "x", 4);
            other_page.page_id = page(2);
            let mut empty_vector = chunk(1, 0, &[1], "x", 4);
            empty_vector.vector = Vec::new();
            let mut odd_vector = chunk(1, 0, &[1], "x", 4);
            odd_vector.vector = vec![0; 5];
            for (what, page_row, chunks) in [
                ("count", row(2), vec![chunk(1, 0, &[1], "x", 4)]),
                ("page", row(1), vec![other_page]),
                ("empty vector", row(1), vec![empty_vector]),
                ("odd vector", row(1), vec![odd_vector]),
            ] {
                let err = q.ai_page_replace(&page_row, &chunks).unwrap_err();
                assert!(matches!(err, StorageError::InvalidData(_)), "{what}: {err}");
            }
            assert_eq!(
                q.ai_chunk_text(page(1), 0).unwrap().as_deref(),
                Some("kept")
            );
            assert_eq!(count(conn, "ai_chunks"), 1);
        });
    }

    #[test]
    fn a_bare_db_replaces_a_page_all_or_nothing() {
        let db = new_db();
        store(&db, 1, "m", &[chunk(1, 0, &[1], "kept", 4)]);
        db.connection()
            .execute_batch(
                "CREATE TRIGGER no_boom BEFORE INSERT ON ai_chunks
                 WHEN NEW.seq = 7
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db
            .ai_page_replace(
                &AiPageRow {
                    page_id: page(1),
                    source_hash: hash(9),
                    model: "other".to_owned(),
                    chunks: 2,
                    indexed_at: 5,
                },
                &[chunk(1, 0, &[1], "new", 4), chunk(1, 7, &[2], "boom", 4)],
            )
            .unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(
            db.ai_chunk_text(page(1), 0).unwrap().as_deref(),
            Some("kept")
        );
        assert_eq!(db.ai_page_get(page(1)).unwrap().unwrap().model, "m");
        assert!(db.connection().is_autocommit());
    }

    #[test]
    fn pages_are_deleted_with_their_chunks_and_the_index_is_cleared() {
        on_db_and_tx(|q, conn| {
            for n in [1, 2, 3] {
                store(q, n, "m", &[chunk(n, 0, &[1], "text", 4)]);
            }
            q.ai_pages_delete(&[page(2), page(9), page(2)]).unwrap();
            q.ai_pages_delete(&[]).unwrap();
            assert_eq!(q.ai_page_get(page(2)).unwrap(), None);
            assert!(q.ai_chunks_of(page(2)).unwrap().is_empty());
            assert_eq!(count(conn, "ai_pages"), 2);
            assert_eq!(count(conn, "ai_chunks"), 2);

            index(q, 1, &[(1, "text")]);
            q.ai_clear_index().unwrap();
            assert_eq!(count(conn, "ai_pages") + count(conn, "ai_chunks"), 0);
            assert_eq!(
                dirty(q),
                [page(1)],
                "clearing the index leaves the dirty pages"
            );
            store(q, 1, "m", &[chunk(1, 0, &[1], "text", 4)]);
            q.ai_clear().unwrap();
            assert_eq!(
                count(conn, "ai_pages") + count(conn, "ai_chunks") + count(conn, "ai_dirty"),
                0
            );
            q.ai_clear().unwrap();
        });
    }

    #[test]
    fn another_model_is_found_among_the_stored_pages() {
        on_db_and_tx(|q, _| {
            assert!(!q.ai_has_other_model("a").unwrap());
            store(q, 1, "a", &[]);
            assert!(!q.ai_has_other_model("a").unwrap());
            assert!(q.ai_has_other_model("b").unwrap());
            store(q, 2, "b", &[]);
            assert!(q.ai_has_other_model("a").unwrap());
        });
    }

    #[test]
    fn vectors_are_found_by_the_hash_of_their_text() {
        on_db_and_tx(|q, _| {
            let mut shared = chunk(1, 0, &[1], "shared", 4);
            shared.hash = hash(77);
            store(q, 1, "m", &[shared, chunk(1, 1, &[2], "own", 4)]);
            let hashes = [hash(77), hash(5), hash(16 + 1)];
            assert_eq!(
                q.ai_hashes_stored(&hashes).unwrap(),
                [true, false, true],
                "the hash of the chunk written as seq 1 of page 1 is 16 + 1"
            );
            let vectors = q.ai_vectors_by_hash(&hashes).unwrap();
            assert_eq!(vectors[0].as_deref(), Some(vector(1, 4).as_slice()));
            assert_eq!(vectors[1], None);
            assert_eq!(vectors[2].as_deref(), Some(vector(1, 4).as_slice()));
            assert!(q.ai_hashes_stored(&[]).unwrap().is_empty());
            assert!(q.ai_vectors_by_hash(&[]).unwrap().is_empty());
        });
    }

    #[test]
    fn every_chunk_is_handed_to_a_scan_in_page_and_position_order() {
        on_db_and_tx(|q, _| {
            store(
                q,
                2,
                "m",
                &[chunk(2, 1, &[2], "b", 4), chunk(2, 0, &[1], "a", 4)],
            );
            store(q, 1, "m", &[chunk(1, 0, &[3, 4, 5], "c", 4)]);
            let mut seen = Vec::new();
            q.ai_chunks_scan(&mut |row| seen.push((row.page_id, row.seq, row.text.clone())))
                .unwrap();
            assert_eq!(
                seen,
                [
                    (page(1), 0, "c".to_owned()),
                    (page(2), 0, "a".to_owned()),
                    (page(2), 1, "b".to_owned()),
                ]
            );
            assert_eq!(
                q.ai_chunks_of(page(1)).unwrap()[0].block_ids,
                [block(3), block(4), block(5)]
            );
        });
    }

    // ---- views ----

    #[test]
    fn the_status_counts_live_pages_chunks_and_waiting_pages() {
        on_db_and_tx(|q, _| {
            assert_eq!(
                q.ai_index_counts(Some("m")).unwrap(),
                AiIndexCounts::default()
            );
            for n in [1, 2, 3, 4] {
                add_page(q, n, space(1));
            }
            trash_page(q, 4, space(1));
            for n in [1, 2, 4] {
                store(
                    q,
                    n,
                    "m",
                    &[chunk(n, 0, &[1], "text", 6), chunk(n, 1, &[2], "t2", 6)],
                );
            }
            store(q, 3, "old", &[chunk(3, 0, &[1], "text", 6)]);
            index(q, 2, &[(1, "changed")]);
            // Page 4 (trashed) and page 3 (waiting for nothing: no text row yet) do not count.
            let counts = q.ai_index_counts(Some("m")).unwrap();
            assert_eq!(
                counts,
                AiIndexCounts {
                    model: Some("m".to_owned()),
                    pages_indexed: 1,
                    pages_total: 3,
                    chunks: 7,
                    dirty: 1,
                    dimensions: Some(6),
                },
                "page 1 is current; page 2 waits; page 3 is another model; page 4 is trashed"
            );
            assert_eq!(
                q.ai_index_counts(Some("old")).unwrap().pages_indexed,
                1,
                "only page 3 is stored with `old` and is live"
            );
            assert_eq!(
                q.ai_index_counts(None).unwrap().pages_indexed,
                2,
                "no model asked for: any model counts (pages 1 and 3)"
            );
            assert_eq!(q.ai_index_counts(Some("other")).unwrap().pages_indexed, 0);

            // A trashed page that waits is not counted as waiting.
            index(q, 4, &[(1, "text")]);
            assert_eq!(q.ai_index_counts(Some("m")).unwrap().dirty, 1);
        });
    }

    #[test]
    fn the_live_pages_are_listed_with_their_space() {
        on_db_and_tx(|q, _| {
            add_page(q, 3, space(2));
            add_page(q, 1, space(1));
            add_page(q, 2, space(1));
            trash_page(q, 4, space(1));
            assert_eq!(
                q.ai_live_pages(None).unwrap(),
                [
                    (page(1), space(1)),
                    (page(2), space(1)),
                    (page(3), space(2))
                ]
            );
            assert_eq!(
                q.ai_live_pages(Some(space(1))).unwrap(),
                [(page(1), space(1)), (page(2), space(1))]
            );
            assert!(q.ai_live_pages(Some(space(9))).unwrap().is_empty());
        });
    }

    #[test]
    fn the_body_of_a_page_is_its_block_rows_in_the_order_written() {
        on_db_and_tx(|q, _| {
            index(q, 1, &[(9, "nine"), (1, "one"), (5, "five")]);
            index(q, 2, &[(1, "other page")]);
            q.fts_replace_properties(
                page(1),
                &[FtsBlock {
                    block_id: block(50),
                    text: "a property".to_owned(),
                }],
            )
            .unwrap();
            let body = q.ai_page_body(page(1)).unwrap();
            assert_eq!(
                body.iter()
                    .map(|b| (b.block_id, b.text.as_str()))
                    .collect::<Vec<_>>(),
                [(block(9), "nine"), (block(1), "one"), (block(5), "five")],
                "no title row, no property row"
            );
            assert!(q.ai_page_body(page(7)).unwrap().is_empty());
            index(q, 3, &[]);
            assert!(q.ai_page_body(page(3)).unwrap().is_empty(), "a title only");
        });
    }

    #[test]
    fn a_search_in_one_page_finds_that_pages_rows_only() {
        on_db_and_tx(|q, _| {
            for n in [1, 2] {
                add_page(q, n, space(1));
                index(q, n, &[(1, "alpha"), (2, "beta")]);
            }
            let params = SearchParams {
                query: "alpha beta",
                space_id: None,
                titles_only: false,
                limit: 10,
                any_word: true,
                edited_after: None,
            };
            let hits = q.ai_search_in_page(page(2), &params).unwrap();
            assert_eq!(hits.len(), 2);
            assert!(hits.iter().all(|h| h.page_id == page(2)));
        });
    }

    // ---- conventions ----

    #[test]
    fn ids_and_hashes_are_stored_as_blobs_of_their_width() {
        on_db_and_tx(|q, conn| {
            store(q, 1, "m", &[chunk(1, 0, &[1, 2], "text", 4)]);
            index(q, 1, &[(1, "text")]);
            let widths: Vec<String> = conn
                .prepare(
                    "SELECT typeof(page_id) || length(page_id) FROM ai_pages \
                     UNION ALL SELECT typeof(source_hash) || length(source_hash) FROM ai_pages \
                     UNION ALL SELECT typeof(page_id) || length(page_id) FROM ai_chunks \
                     UNION ALL SELECT typeof(hash) || length(hash) FROM ai_chunks \
                     UNION ALL SELECT typeof(block_ids) || length(block_ids) FROM ai_chunks \
                     UNION ALL SELECT typeof(page_id) || length(page_id) FROM ai_dirty",
                )
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(
                widths,
                ["blob16", "blob32", "blob16", "blob32", "blob32", "blob16"]
            );
        });
    }

    #[test]
    fn a_damaged_row_is_invalid_data_and_never_a_panic() {
        let db = new_db();
        let conn = db.connection();
        let invalid = |what: &str, err: StorageError| {
            assert!(matches!(err, StorageError::InvalidData(_)), "{what}: {err}");
        };
        // A short source hash.
        conn.execute(
            "INSERT INTO ai_pages VALUES (x'01010101010101010101010101010101', zeroblob(31), 'm', 0, 0)",
            [],
        )
        .unwrap();
        invalid("source hash", db.ai_page_get(page(1)).unwrap_err());
        // A negative chunk count.
        conn.execute(
            "INSERT INTO ai_pages VALUES (x'02020202020202020202020202020202', zeroblob(32), 'm', -1, 0)",
            [],
        )
        .unwrap();
        invalid("chunk count", db.ai_page_get(page(2)).unwrap_err());
        // Chunk rows: a block id list of 15 bytes, a hash of 31, a vector of 5 bytes, an empty one.
        let blob = |n: u8, bytes: usize| format!("x'{}'", format!("{n:02x}").repeat(bytes));
        for (n, columns) in [
            (3, "zeroblob(15), zeroblob(32), 't', zeroblob(16)"),
            (4, "zeroblob(16), zeroblob(31), 't', zeroblob(16)"),
            (5, "zeroblob(16), zeroblob(32), 't', zeroblob(5)"),
            (6, "zeroblob(16), zeroblob(32), 't', zeroblob(0)"),
        ] {
            conn.execute(
                &format!(
                    "INSERT INTO ai_chunks VALUES ({}, 0, {columns})",
                    blob(n, 16)
                ),
                [],
            )
            .unwrap();
            invalid(&format!("chunk {n}"), db.ai_chunks_of(page(n)).unwrap_err());
        }
        invalid("scan", db.ai_chunks_scan(&mut |_| {}).unwrap_err());
        conn.execute("DELETE FROM ai_chunks", []).unwrap();
        conn.execute(
            "INSERT INTO ai_chunks VALUES (zeroblob(15), 0, zeroblob(0), zeroblob(32), 't', zeroblob(16))",
            [],
        )
        .unwrap();
        invalid("chunk page id", db.ai_chunks_scan(&mut |_| {}).unwrap_err());
        conn.execute("DELETE FROM ai_chunks", []).unwrap();
        // A stored vector that is not whole floats is not handed out for reuse.
        conn.execute(
            &format!(
                "INSERT INTO ai_chunks VALUES (zeroblob(16), 0, zeroblob(0), {}, 't', zeroblob(7))",
                blob(9, 32)
            ),
            [],
        )
        .unwrap();
        invalid(
            "vector by hash",
            db.ai_vectors_by_hash(&[hash(9)]).unwrap_err(),
        );
        // A dirty page id of the wrong width.
        conn.execute("INSERT INTO ai_dirty VALUES (zeroblob(15))", [])
            .unwrap();
        invalid("dirty page", db.ai_dirty_after(None, 5).unwrap_err());
    }

    #[test]
    fn the_statements_use_the_indexes_the_migration_and_the_text_index_create() {
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
        for (sql, expected) in [
            (DIRTY_AFTER_SQL, "USING PRIMARY KEY (page_id>?)"),
            (DIRTY_DELETE_SQL, "USING PRIMARY KEY (page_id=?)"),
            (PAGE_GET_SQL, "USING PRIMARY KEY (page_id=?)"),
            (PAGE_DELETE_SQL, "USING PRIMARY KEY (page_id=?)"),
            (CHUNKS_DELETE_PAGE_SQL, "USING PRIMARY KEY (page_id=?)"),
            (CHUNK_TEXT_SQL, "USING PRIMARY KEY (page_id=? AND seq=?)"),
            (VECTOR_BY_HASH_SQL, "ai_chunks_hash (hash=?)"),
            (PAGE_BODY_SQL, "fts_blocks_page (page_id=?"),
            (
                LIVE_PAGES_OF_SPACE_SQL,
                "pages_index_space_trashed (space_id=? AND trashed_at=?)",
            ),
        ] {
            let p = plan(sql);
            assert!(p.contains(expected), "{expected} in {p} for {sql}");
        }
        let exists = plan("SELECT 1 FROM ai_chunks WHERE hash = ?1");
        assert!(
            exists.contains("USING COVERING INDEX ai_chunks_hash"),
            "a hash is found in the index alone: {exists}"
        );
    }
}
