//! The semantic index and retrieval (ADR-0037, `docs/architecture/ai.md` section 5): the work the
//! shell's indexer embeds (`AiIndexNext`), the vectors it stores (`AiIndexStore`), the status, the
//! clearing, and the hybrid retrieval (`AiRetrieve`) over an in-memory vector cache. The chunking,
//! vector math and fusion are `kb-ai`'s; the tables are `kb-storage`'s `AiIndexQueries`.
//!
//! The index is derived data: the text of a page is its `fts_blocks` body rows, and the
//! `ai_dirty` triggers of the storage layer say which pages changed. Nothing here talks to a
//! model: the shell embeds the chunks `AiIndexNext` hands out and sends the vectors back.
//!
//! Two decisions beyond the contract's text, both so that a page never stays out of the index
//! without a reason:
//!
//! - A live page that no `ai_pages` row of the current model covers and that is not waiting
//!   (a page restored from the trash after the walk dropped its chunks, a page that never had a
//!   text row) is marked dirty by `AiIndexNext` when nothing else waits.
//! - A page whose chunks all have a stored vector still goes into the work, with no chunk to
//!   embed: the shell answers it with an `AiIndexStore` without vectors.

use std::collections::{BTreeMap, HashMap, HashSet};

use kb_ai::chunk::{Chunk, chunk_page, source_hash};
use kb_ai::fusion::{RRF_K, rrf};
use kb_ai::vector::{decode, encode, normalize, top_k};
use kb_ai::{hash_from_hex, hex};
use kb_storage::{
    AI_HASH_BYTES, AiChunkRow, AiIndexQueries, AiPageRow, DocQueries, FtsHit, SearchParams,
    StorageError, TextIndexQueries,
};
use kb_types::ai::{
    AI_SETTINGS_SETTING, AiChunkVector, AiIndexChunk, AiIndexInfo, AiIndexPage, AiIndexWork,
    AiScope, AiSettings, AiSource,
};
use kb_types::{ApiError, BlockId, Event, PageId, Request, Response, SpaceId};
use sha2::{Digest, Sha256};

use crate::{Core, map_storage_error};

/// `AiIndexNext`: the most chunks a caller may ask for.
const MAX_WORK_CHUNKS: u32 = 64;
/// `AiIndexNext`: the most pages in one answer (pages that need no embedding count too).
const MAX_WORK_PAGES: usize = 64;
/// `AiIndexNext`: dirty pages read from the table per step of the walk.
const DIRTY_BATCH: u32 = 64;
/// Shortest and longest vector the index stores, in floats.
const MIN_DIMENSIONS: usize = 16;
const MAX_DIMENSIONS: usize = 8_192;
/// `AiRetrieve`: the question's length, in characters.
const MAX_QUERY_CHARS: usize = 2_000;
/// `AiRetrieve`: the default and the largest number of passages.
const DEFAULT_PASSAGES: u32 = 8;
const MAX_PASSAGES: u32 = 20;
/// `AiRetrieve`: candidates of each list that goes into the fusion.
const SEMANTIC_CANDIDATES: usize = 40;
const KEYWORD_CANDIDATES: u32 = 40;
/// `AiRetrieve`: the cosine a chunk must reach to be a semantic candidate.
const MIN_SEMANTIC_SCORE: f32 = 0.25;
/// `AiRetrieve`: passages of one page in the answer.
const MAX_PASSAGES_PER_PAGE: usize = 3;
/// `AiRetrieve`: the length of a passage's text, in characters.
const MAX_PASSAGE_CHARS: usize = 1_500;

/// The in-memory part of the index (the vector cache), dropped with the core on lock.
#[derive(Debug, Default)]
pub(crate) struct AiState {
    /// The vectors of every stored chunk, loaded by the first `AiRetrieve` and kept in step by
    /// store, clear and the walk of `AiIndexNext`.
    cache: Option<VectorCache>,
}

/// The stored chunks by page (in page id order, so that ties break the same way every time).
#[derive(Debug, Default)]
struct VectorCache {
    pages: BTreeMap<PageId, Vec<CachedChunk>>,
}

/// A chunk in the cache: what retrieval needs to rank it and to find the chunk that contains a
/// block. The text stays in the table (it is read for the few passages that are returned).
#[derive(Debug)]
struct CachedChunk {
    seq: u32,
    block_ids: Vec<BlockId>,
    /// Unit length, as stored.
    vector: Vec<f32>,
}

/// A passage before it becomes an [`AiSource`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PassageKey {
    /// A stored chunk.
    Chunk(PageId, u32),
    /// A block of a page that no stored chunk covers (not indexed yet, or the index is stale),
    /// or `None` for the title.
    Block(PageId, Option<BlockId>),
}

impl PassageKey {
    const fn page_id(&self) -> PageId {
        match self {
            Self::Chunk(page, _) | Self::Block(page, _) => *page,
        }
    }
}

/// The hash of a page's chunks that `ai_pages.source_hash` stores and the work hands out as
/// `sourceHash`: `kb_ai`'s hash of the chunk hashes (text and model) followed by the block ids
/// of every chunk. A block deleted and written again with the same text has the same chunk but
/// another id, and the stored `block_ids` would point at a block that is gone; with the ids in
/// the hash that page is stale (and is stored again, reusing the vectors by chunk hash).
fn page_hash(chunks: &[Chunk]) -> [u8; AI_HASH_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(source_hash(chunks));
    for chunk in chunks {
        hasher.update(to_u32(chunk.block_ids.len()).to_le_bytes());
        for block_id in &chunk.block_ids {
            hasher.update(block_id.to_bytes());
        }
    }
    hasher.finalize().into()
}

fn to_u32(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

impl Core {
    pub(crate) fn dispatch_ai(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::AiIndexStatus => Ok(Response::AiIndex(self.ai_index_status()?)),
            Request::AiIndexNext { max_chunks } => {
                Ok(Response::AiIndexWork(self.ai_index_next(max_chunks)?))
            }
            Request::AiIndexStore {
                page_id,
                source_hash,
                model,
                vectors,
            } => {
                self.ai_index_store(page_id, &source_hash, &model, vectors)?;
                Ok(Response::Ok)
            }
            Request::AiIndexClear => {
                self.ai_index_clear()?;
                Ok(Response::Ok)
            }
            Request::AiRetrieve {
                query,
                vector,
                scope,
                limit,
            } => Ok(Response::AiPassages(
                self.ai_retrieve(&query, vector, scope, limit)?,
            )),
            other => Err(ApiError::internal(format!(
                "request {} is not a semantic index request",
                other.kind()
            ))),
        }
    }

    // ----- settings ---------------------------------------------------------------------

    /// The AI settings: absent or unreadable is the default, which is off.
    fn ai_settings(&self) -> Result<AiSettings, ApiError> {
        match self.db.setting_get(AI_SETTINGS_SETTING) {
            Ok(Some(value)) => Ok(serde_json::from_value(value).unwrap_or_default()),
            Ok(None) | Err(StorageError::InvalidData(_)) => Ok(AiSettings::default()),
            Err(err) => Err(map_storage_error(err)),
        }
    }

    /// The embedding model while the index is wanted: AI on, the semantic index on and a model
    /// chosen.
    fn ai_indexing_model(&self) -> Result<Option<String>, ApiError> {
        let settings = self.ai_settings()?;
        if !settings.enabled || !settings.semantic_index {
            return Ok(None);
        }
        Ok(settings
            .embedding_model
            .map(|model| model.trim().to_owned())
            .filter(|model| !model.is_empty()))
    }

    // ----- status and clearing -----------------------------------------------------------

    fn ai_index_status(&self) -> Result<AiIndexInfo, ApiError> {
        let model = self.ai_indexing_model()?;
        let counts = self
            .db
            .ai_index_counts(model.as_deref())
            .map_err(map_storage_error)?;
        // The pages of a space whose AI is off (a shared one until this account opts in) are never
        // indexed, so they do not count among the pages the index is waiting for (ADR-0047).
        let skipped = to_u32(
            self.db
                .ai_live_pages(None)
                .map_err(map_storage_error)?
                .into_iter()
                .filter(|(_, space)| !self.ai_space_enabled(*space))
                .count(),
        );
        Ok(AiIndexInfo {
            model: counts.model,
            pages_indexed: counts.pages_indexed,
            pages_total: counts.pages_total.saturating_sub(skipped),
            chunks: counts.chunks,
            dirty: counts.dirty,
            dimensions: counts.dimensions,
        })
    }

    fn ai_index_clear(&mut self) -> Result<(), ApiError> {
        self.db
            .transaction(|tx| {
                tx.ai_clear()?;
                tx.ai_dirty_mark_all()?;
                Ok(())
            })
            .map_err(map_storage_error)?;
        self.ai.cache = None;
        self.events.push(Event::AiIndexChanged);
        Ok(())
    }

    // ----- the work for the shell --------------------------------------------------------

    /// The chunks of a page now: its body rows from the text index, its title from the index of
    /// pages.
    fn ai_page_chunks(
        &self,
        model: &str,
        page_id: PageId,
        title: &str,
    ) -> Result<Vec<Chunk>, ApiError> {
        let body = self.db.ai_page_body(page_id).map_err(map_storage_error)?;
        let blocks: Vec<(BlockId, String)> = body
            .into_iter()
            .map(|row| (row.block_id, row.text))
            .collect();
        Ok(chunk_page(model, title, &blocks))
    }

    fn ai_index_next(&mut self, max_chunks: u32) -> Result<AiIndexWork, ApiError> {
        if !(1..=MAX_WORK_CHUNKS).contains(&max_chunks) {
            return Err(ApiError::invalid(
                "maxChunks",
                "must be between 1 and 64 chunks",
            ));
        }
        let Some(model) = self.ai_indexing_model()? else {
            return Ok(AiIndexWork::default());
        };
        let mut changed = false;
        if self
            .db
            .ai_has_other_model(&model)
            .map_err(map_storage_error)?
        {
            // The vectors of another model mean nothing to this one: start over.
            self.db
                .transaction(|tx| {
                    tx.ai_clear_index()?;
                    tx.ai_dirty_mark_all()?;
                    Ok(())
                })
                .map_err(map_storage_error)?;
            self.ai.cache = None;
            changed = true;
        } else if self.db.ai_dirty_count().map_err(map_storage_error)? == 0 {
            // Nothing waits: drop what a page that became a template (or moved into one) was indexed
            // with, then look for a live page that the index does not cover.
            self.ai_forget_templates()?;
            let added = self
                .db
                .ai_dirty_mark_unindexed(&model)
                .map_err(map_storage_error)?;
            // The pages of a space whose AI is off come back as dirty every time and go again
            // below: they are not a change.
            let off = to_u32(
                self.db
                    .ai_live_pages(None)
                    .map_err(map_storage_error)?
                    .into_iter()
                    .filter(|(_, space)| !self.ai_space_enabled(*space))
                    .count(),
            );
            changed = added > off;
        }

        let mut pages: Vec<AiIndexPage> = Vec::new();
        let mut needed: u32 = 0;
        let mut gone: Vec<PageId> = Vec::new();
        let mut unchanged: Vec<PageId> = Vec::new();
        // Pages of a space whose AI is off that the index holds nothing of: only the work list forgets
        // them.
        let mut quiet: Vec<PageId> = Vec::new();
        let mut after: Option<PageId> = None;
        'walk: loop {
            let batch = self
                .db
                .ai_dirty_after(after, DIRTY_BATCH)
                .map_err(map_storage_error)?;
            let Some(last) = batch.last().copied() else {
                break;
            };
            after = Some(last);
            for page_id in batch {
                let row = self
                    .db
                    .pages_index_get(page_id)
                    .map_err(map_storage_error)?;
                let Some(row) = row.filter(|row| row.trashed_at.is_none() && !row.in_template)
                else {
                    gone.push(page_id);
                    continue;
                };
                if !self.ai_space_enabled(row.space_id) {
                    if self
                        .db
                        .ai_page_get(page_id)
                        .map_err(map_storage_error)?
                        .is_some()
                    {
                        gone.push(page_id);
                    } else {
                        quiet.push(page_id);
                    }
                    continue;
                }
                let chunks = self.ai_page_chunks(&model, page_id, &row.title)?;
                let hash = page_hash(&chunks);
                let stored = self.db.ai_page_get(page_id).map_err(map_storage_error)?;
                if stored.is_some_and(|stored| stored.model == model && stored.source_hash == hash)
                {
                    unchanged.push(page_id);
                    continue;
                }
                let hashes: Vec<[u8; AI_HASH_BYTES]> = chunks.iter().map(|c| c.hash).collect();
                let present = self
                    .db
                    .ai_hashes_stored(&hashes)
                    .map_err(map_storage_error)?;
                let need = to_u32(present.iter().filter(|present| !**present).count());
                if !pages.is_empty()
                    && (needed.saturating_add(need) > max_chunks || pages.len() >= MAX_WORK_PAGES)
                {
                    break 'walk;
                }
                needed = needed.saturating_add(need);
                pages.push(AiIndexPage {
                    page_id,
                    source_hash: hex(&hash),
                    chunks: chunks
                        .into_iter()
                        .zip(present)
                        .map(|(chunk, present)| AiIndexChunk {
                            seq: chunk.seq,
                            hash: hex(&chunk.hash),
                            text: chunk.text,
                            needs_vector: !present,
                        })
                        .collect(),
                });
            }
        }

        if !gone.is_empty() || !unchanged.is_empty() || !quiet.is_empty() {
            self.db
                .transaction(|tx| {
                    tx.ai_pages_delete(&gone)?;
                    tx.ai_dirty_remove(&gone)?;
                    tx.ai_dirty_remove(&quiet)?;
                    tx.ai_dirty_remove(&unchanged)
                })
                .map_err(map_storage_error)?;
            if let Some(cache) = self.ai.cache.as_mut() {
                for page_id in &gone {
                    cache.pages.remove(page_id);
                }
            }
            changed |= !gone.is_empty() || !unchanged.is_empty();
        }
        if changed {
            self.events.push(Event::AiIndexChanged);
        }
        let waiting = self.db.ai_dirty_count().map_err(map_storage_error)?;
        Ok(AiIndexWork {
            model: Some(model),
            remaining: waiting.saturating_sub(to_u32(pages.len())),
            pages,
        })
    }

    /// `AiIndexStore`: writes the chunks of a page with their vectors, unless the page, its text
    /// or the model changed since the work was handed out.
    fn ai_index_store(
        &mut self,
        page_id: PageId,
        hash_hex: &str,
        model: &str,
        vectors: Vec<AiChunkVector>,
    ) -> Result<(), ApiError> {
        // The shape of the request first: a malformed one is a bug of the caller, stale or not.
        let mut by_seq: HashMap<u32, Vec<f32>> = HashMap::with_capacity(vectors.len());
        let mut dimensions: Option<usize> = None;
        for AiChunkVector { seq, vector } in vectors {
            if !(MIN_DIMENSIONS..=MAX_DIMENSIONS).contains(&vector.len()) {
                return Err(ApiError::invalid(
                    "vectors",
                    "a vector must have 16 to 8,192 values",
                ));
            }
            if *dimensions.get_or_insert(vector.len()) != vector.len() {
                return Err(ApiError::invalid(
                    "vectors",
                    "every vector must have the same length",
                ));
            }
            if vector.iter().any(|value| !value.is_finite()) {
                return Err(ApiError::invalid(
                    "vectors",
                    "a vector value is not a number",
                ));
            }
            if by_seq.insert(seq, vector).is_some() {
                return Err(ApiError::invalid(
                    "vectors",
                    "a chunk has more than one vector",
                ));
            }
        }

        // Stale: AI turned off or another model since, the page gone or trashed, its text changed.
        if self.ai_indexing_model()?.as_deref() != Some(model) {
            return Ok(());
        }
        let Some(row) = self
            .db
            .pages_index_get(page_id)
            .map_err(map_storage_error)?
            .filter(|row| {
                row.trashed_at.is_none() && !row.in_template && self.ai_space_enabled(row.space_id)
            })
        else {
            return Ok(());
        };
        let chunks = self.ai_page_chunks(model, page_id, &row.title)?;
        let hash = page_hash(&chunks);
        if hash_from_hex(hash_hex) != Some(hash) {
            return Ok(());
        }

        // A vector for every chunk: the request's, normalised, or one stored for the same text.
        if by_seq.keys().any(|seq| {
            usize::try_from(*seq)
                .ok()
                .is_none_or(|seq| seq >= chunks.len())
        }) {
            return Err(ApiError::invalid(
                "vectors",
                "a vector names a chunk the page does not have",
            ));
        }
        let lacking: Vec<&Chunk> = chunks
            .iter()
            .filter(|chunk| !by_seq.contains_key(&chunk.seq))
            .collect();
        let lacking_hashes: Vec<[u8; AI_HASH_BYTES]> = lacking.iter().map(|c| c.hash).collect();
        let stored = self
            .db
            .ai_vectors_by_hash(&lacking_hashes)
            .map_err(map_storage_error)?;
        let mut reused: HashMap<u32, Vec<f32>> = HashMap::new();
        for (chunk, bytes) in lacking.iter().zip(stored) {
            let Some(vector) = bytes.as_deref().and_then(decode) else {
                return Err(ApiError::invalid(
                    "vectors",
                    format!("chunk {} has no vector", chunk.seq),
                ));
            };
            reused.insert(chunk.seq, vector);
        }
        let mut resolved: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());
        for chunk in &chunks {
            let vector = match by_seq.remove(&chunk.seq) {
                Some(mut vector) => {
                    normalize(&mut vector);
                    vector
                }
                None => reused.remove(&chunk.seq).unwrap_or_default(),
            };
            if !(MIN_DIMENSIONS..=MAX_DIMENSIONS).contains(&vector.len())
                || resolved
                    .first()
                    .is_some_and(|first| first.len() != vector.len())
            {
                return Err(ApiError::invalid(
                    "vectors",
                    "every vector must have the same length, 16 to 8,192 values",
                ));
            }
            resolved.push(vector);
        }

        let rows: Vec<AiChunkRow> = chunks
            .iter()
            .zip(&resolved)
            .map(|(chunk, vector)| AiChunkRow {
                page_id,
                seq: chunk.seq,
                block_ids: chunk.block_ids.clone(),
                hash: chunk.hash,
                text: chunk.text.clone(),
                vector: encode(vector),
            })
            .collect();
        let page = AiPageRow {
            page_id,
            source_hash: hash,
            model: model.to_owned(),
            chunks: to_u32(rows.len()),
            indexed_at: self.clock.now_ms(),
        };
        self.db
            .transaction(|tx| {
                tx.ai_page_replace(&page, &rows)?;
                tx.ai_dirty_remove(&[page_id])
            })
            .map_err(map_storage_error)?;
        if let Some(cache) = self.ai.cache.as_mut() {
            if chunks.is_empty() {
                cache.pages.remove(&page_id);
            } else {
                cache.pages.insert(
                    page_id,
                    chunks
                        .into_iter()
                        .zip(resolved)
                        .map(|(chunk, vector)| CachedChunk {
                            seq: chunk.seq,
                            block_ids: chunk.block_ids,
                            vector,
                        })
                        .collect(),
                );
            }
        }
        self.events.push(Event::AiIndexChanged);
        Ok(())
    }

    // ----- retrieval ---------------------------------------------------------------------

    /// Loads the vector cache from the table, once.
    fn ai_load_cache(&mut self) -> Result<(), ApiError> {
        if self.ai.cache.is_some() {
            return Ok(());
        }
        let mut cache = VectorCache::default();
        self.db
            .ai_chunks_scan(&mut |row| {
                if let Some(vector) = decode(&row.vector) {
                    cache
                        .pages
                        .entry(row.page_id)
                        .or_default()
                        .push(CachedChunk {
                            seq: row.seq,
                            block_ids: row.block_ids,
                            vector,
                        });
                }
            })
            .map_err(map_storage_error)?;
        self.ai.cache = Some(cache);
        Ok(())
    }

    /// The live pages a scope covers, with their space, and the filters the full-text search
    /// takes for it. A trashed page is never live; a trashed page given as the scope has none.
    fn ai_scope_pages(&self, scope: AiScope) -> Result<ScopePages, ApiError> {
        match scope {
            AiScope::All => Ok(ScopePages {
                live: self.ai_live_pages(None)?,
                space: None,
                page: None,
            }),
            AiScope::Space { space_id } => {
                if !self.docs.structures.contains_key(&space_id) {
                    return Err(ApiError::not_found("space not found"));
                }
                Ok(ScopePages {
                    live: self.ai_live_pages(Some(space_id))?,
                    space: Some(space_id),
                    page: None,
                })
            }
            AiScope::Page { page_id } => {
                let row = self
                    .db
                    .pages_index_get(page_id)
                    .map_err(map_storage_error)?
                    .ok_or_else(|| ApiError::not_found("page not found"))?;
                let live = if row.trashed_at.is_none()
                    && !row.in_template
                    && self.ai_space_enabled(row.space_id)
                {
                    HashMap::from([(page_id, row.space_id)])
                } else {
                    HashMap::new()
                };
                Ok(ScopePages {
                    live,
                    space: None,
                    page: Some(page_id),
                })
            }
        }
    }

    fn ai_live_pages(
        &self,
        space_id: Option<SpaceId>,
    ) -> Result<HashMap<PageId, SpaceId>, ApiError> {
        // A space whose AI is off is neither indexed nor retrieved (ADR-0047).
        Ok(self
            .db
            .ai_live_pages(space_id)
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|(_, space)| self.ai_space_enabled(*space))
            .collect())
    }

    /// Forgets the semantic index rows of the pages that are templates or inside one (ADR-0056):
    /// templates are not indexed, so what a page was indexed with before it became one (or moved
    /// into one) goes. Nothing happens, and no event is sent, when there is none.
    pub(crate) fn ai_forget_templates(&mut self) -> Result<(), ApiError> {
        let pages = self.db.ai_template_pages().map_err(map_storage_error)?;
        self.ai_forget_pages(&pages)
    }

    /// Forgets the semantic index rows of pages (a space whose AI went off, a space that was
    /// dropped): their rows and their place on the work list.
    pub(crate) fn ai_forget_pages(&mut self, pages: &[PageId]) -> Result<(), ApiError> {
        if pages.is_empty() {
            return Ok(());
        }
        self.db
            .transaction(|tx| {
                tx.ai_pages_delete(pages)?;
                tx.ai_dirty_remove(pages)
            })
            .map_err(map_storage_error)?;
        if let Some(cache) = self.ai.cache.as_mut() {
            for page_id in pages {
                cache.pages.remove(page_id);
            }
        }
        self.events.push(Event::AiIndexChanged);
        Ok(())
    }

    fn ai_retrieve(
        &mut self,
        query: &str,
        vector: Option<Vec<f32>>,
        scope: AiScope,
        limit: Option<u32>,
    ) -> Result<Vec<AiSource>, ApiError> {
        if !(1..=MAX_QUERY_CHARS).contains(&query.chars().count()) {
            return Err(ApiError::invalid("query", "must be 1 to 2,000 characters"));
        }
        let limit = limit.unwrap_or(DEFAULT_PASSAGES);
        if !(1..=MAX_PASSAGES).contains(&limit) {
            return Err(ApiError::invalid("limit", "must be between 1 and 20"));
        }
        let question = match vector {
            Some(mut vector) if !vector.is_empty() => {
                if vector.len() > MAX_DIMENSIONS || vector.iter().any(|value| !value.is_finite()) {
                    return Err(ApiError::invalid("vector", "must be at most 8,192 numbers"));
                }
                normalize(&mut vector);
                Some(vector)
            }
            _ => None,
        };
        let scope = self.ai_scope_pages(scope)?;
        if scope.live.is_empty() {
            return Ok(Vec::new());
        }
        self.ai_load_cache()?;

        // The keyword list first (it reads the database), the semantic one from the cache.
        let params = SearchParams {
            query,
            space_id: scope.space,
            titles_only: false,
            limit: KEYWORD_CANDIDATES,
            any_word: true,
            edited_after: None,
        };
        let hits = match scope.page {
            Some(page_id) => self.db.ai_search_in_page(page_id, &params),
            None => self.db.fts_search(&params),
        }
        .map_err(map_storage_error)?;

        let Some(cache) = self.ai.cache.as_ref() else {
            return Ok(Vec::new());
        };
        let mut semantic: Vec<PassageKey> = Vec::new();
        if let Some(question) = &question {
            let mut keys: Vec<PassageKey> = Vec::new();
            let mut vectors: Vec<&[f32]> = Vec::new();
            for (page_id, chunks) in &cache.pages {
                if !scope.live.contains_key(page_id) {
                    continue;
                }
                for chunk in chunks.iter().filter(|c| c.vector.len() == question.len()) {
                    keys.push(PassageKey::Chunk(*page_id, chunk.seq));
                    vectors.push(&chunk.vector);
                }
            }
            for (index, score) in top_k(question, vectors.iter().copied(), SEMANTIC_CANDIDATES) {
                if score >= MIN_SEMANTIC_SCORE
                    && let Some(key) = keys.get(index)
                {
                    semantic.push(key.clone());
                }
            }
        }
        let mut keyword: Vec<PassageKey> = Vec::new();
        let mut seen: HashSet<PassageKey> = HashSet::new();
        for hit in &hits {
            if !scope.live.contains_key(&hit.page_id) {
                continue;
            }
            let key = key_of_hit(cache, hit);
            if seen.insert(key.clone()) {
                keyword.push(key);
            }
        }
        // The title alone of a page that has no chunk is only worth a passage when no block of
        // that page was found: with a block, it adds nothing.
        let with_block: HashSet<PageId> = keyword
            .iter()
            .filter(|key| !matches!(key, PassageKey::Block(_, None)))
            .map(PassageKey::page_id)
            .collect();
        keyword.retain(
            |key| !matches!(key, PassageKey::Block(page_id, None) if with_block.contains(page_id)),
        );

        let fused = rrf(&[semantic, keyword], RRF_K);
        self.ai_passages(&scope.live, &fused, limit)
    }

    /// Turns the fused list into passages: at most 3 per page, `limit` in all, numbered from 1.
    fn ai_passages(
        &self,
        live: &HashMap<PageId, SpaceId>,
        fused: &[(PassageKey, f32)],
        limit: u32,
    ) -> Result<Vec<AiSource>, ApiError> {
        let mut per_page: HashMap<PageId, usize> = HashMap::new();
        let mut out: Vec<AiSource> = Vec::new();
        for (key, score) in fused {
            if out.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
                break;
            }
            let page_id = key.page_id();
            let Some(space_id) = live.get(&page_id).copied() else {
                continue;
            };
            if per_page.get(&page_id).copied().unwrap_or(0) >= MAX_PASSAGES_PER_PAGE {
                continue;
            }
            let (title, icon) = self.ai_page_label(page_id, space_id)?;
            let Some((block_id, text)) = self.ai_passage_text(key, &title)? else {
                continue;
            };
            *per_page.entry(page_id).or_insert(0) += 1;
            out.push(AiSource {
                n: to_u32(out.len() + 1),
                page_id,
                space_id,
                block_id,
                title,
                icon,
                text,
                score: *score,
            });
        }
        Ok(out)
    }

    /// The title and the icon of a page, as the search gives them: from the structure document
    /// when it is open, else the index of pages.
    fn ai_page_label(
        &self,
        page_id: PageId,
        space_id: SpaceId,
    ) -> Result<(String, Option<String>), ApiError> {
        let info = self
            .docs
            .structures
            .get(&space_id)
            .and_then(|open| open.model.page(page_id));
        if let Some(info) = info {
            return Ok((info.title, info.icon));
        }
        let title = self
            .db
            .pages_index_get(page_id)
            .map_err(map_storage_error)?
            .map(|row| row.title)
            .unwrap_or_default();
        Ok((title, None))
    }

    /// The first block and the text of a passage, `None` when there is nothing to show (a block
    /// that left the text index since the hit).
    fn ai_passage_text(
        &self,
        key: &PassageKey,
        title: &str,
    ) -> Result<Option<(Option<BlockId>, String)>, ApiError> {
        let (block_id, text) = match key {
            PassageKey::Chunk(page_id, seq) => {
                let Some(text) = self
                    .db
                    .ai_chunk_text(*page_id, *seq)
                    .map_err(map_storage_error)?
                else {
                    return Ok(None);
                };
                let block_id = self
                    .ai
                    .cache
                    .as_ref()
                    .and_then(|cache| cache.pages.get(page_id))
                    .and_then(|chunks| chunks.iter().find(|chunk| chunk.seq == *seq))
                    .and_then(|chunk| chunk.block_ids.first().copied());
                (block_id, without_title_line(&text, title).to_owned())
            }
            PassageKey::Block(page_id, Some(block_id)) => {
                let Some(text) = self
                    .db
                    .fts_block_text(*page_id, *block_id)
                    .map_err(map_storage_error)?
                else {
                    return Ok(None);
                };
                (Some(*block_id), text)
            }
            PassageKey::Block(_, None) => (None, title.to_owned()),
        };
        if text.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some((
            block_id,
            text.chars().take(MAX_PASSAGE_CHARS).collect(),
        )))
    }
}

#[cfg(test)]
impl Core {
    /// Whether the vector cache is loaded (the tests of the lazy load).
    pub(crate) fn ai_cache_loaded(&self) -> bool {
        self.ai.cache.is_some()
    }

    /// The pages and chunks in the vector cache (the tests of its consistency).
    pub(crate) fn ai_cache_shape(&self) -> Vec<(PageId, Vec<u32>)> {
        self.ai.cache.as_ref().map_or_else(Vec::new, |cache| {
            cache
                .pages
                .iter()
                .map(|(page, chunks)| (*page, chunks.iter().map(|c| c.seq).collect()))
                .collect()
        })
    }
}

/// What a scope covers.
struct ScopePages {
    /// The live pages of the scope with their space.
    live: HashMap<PageId, SpaceId>,
    /// The space filter of the full-text search.
    space: Option<SpaceId>,
    /// The page filter of the full-text search.
    page: Option<PageId>,
}

/// The passage a full-text hit stands for: the stored chunk that contains its block, the first
/// chunk of the page for a title or property hit, or the block alone (the title alone for a
/// title hit) when no stored chunk covers it.
fn key_of_hit(cache: &VectorCache, hit: &FtsHit) -> PassageKey {
    let chunks = cache.pages.get(&hit.page_id);
    match hit.block_id.filter(|_| !hit.property) {
        Some(block_id) => chunks
            .and_then(|chunks| {
                chunks
                    .iter()
                    .find(|chunk| chunk.block_ids.contains(&block_id))
            })
            .map_or(PassageKey::Block(hit.page_id, Some(block_id)), |chunk| {
                PassageKey::Chunk(hit.page_id, chunk.seq)
            }),
        None => chunks
            .and_then(|chunks| chunks.first())
            .map_or(PassageKey::Block(hit.page_id, None), |chunk| {
                PassageKey::Chunk(hit.page_id, chunk.seq)
            }),
    }
}

/// The text of a chunk without its title line: a chunk is embedded as `"{title}\n{body}"`, and
/// the title is a field of the passage already. Only a line that is the page's title is removed;
/// a chunk of a page indexed under another title keeps its first line.
fn without_title_line<'a>(text: &'a str, title: &str) -> &'a str {
    let title = title.trim();
    let title: String = title.chars().take(kb_ai::chunk::TITLE_MAX_CHARS).collect();
    if title.is_empty() {
        return text;
    }
    text.strip_prefix(title.as_str())
        .and_then(|rest| rest.strip_prefix('\n'))
        .unwrap_or(text)
}
