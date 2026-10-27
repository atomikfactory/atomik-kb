//! Page content: document sessions for the editor (binary API), the text and link
//! indexes, search, backlinks, portable export and Markdown
//! (`docs/development/milestone-03.md`, "Core").
//!
//! A page's document id is its page id (ADR-0016). Documents are created lazily, the
//! first time anything looks at a page's content, with one empty paragraph; the
//! editor always starts from the core's snapshot. Open documents stay in
//! [`Documents::pages`](crate::documents::Documents) until the editor closes them;
//! everything else (indexing, export) loads a page transiently.
//!
//! The text (`fts`), link (`links`), date-mention (`date_mentions`, reminders,
//! ADR-0024) and first-picture (`page_first_images`, ADR-0054) tables are derived
//! data: every import rewrites the page's rows in the same transaction as the update
//! row, `IndexRebuild` recreates them from scratch, and the open sequence repairs a
//! space whose title-row count disagrees with its pages.

use std::collections::{BTreeSet, HashMap, HashSet};

use kb_doc::{BlobKind, Doc, DocError, DocKind, Version, decode_blob, encode_blob};
use kb_model::{Inline, PageDoc};
use kb_storage::{
    AttachmentQueries, CommentQueries, DateMentionQueries, DateMentionRow, Db, DocKindCode,
    DocQueries, FtsBlock, FuzzyQuery, IconRefQueries, JournalKind, LinkRow, PageFirstImageQueries,
    PageIndexRow, PagePropertyQueries, PersonMentionQueries, PersonMentionRow, SearchParams,
    StorageError, TextIndexQueries, UpdateOrigin, rank_fuzzy_titles,
};
use kb_types::{
    ApiError, AttachmentId, Backlink, BlockId, ChangeSource, DocId, ErrorCode, Event, MemberRole,
    PageContent, PageExcerpt, PageFirstImage, PageId, Request, Response, SearchHit, SearchScope,
    SpaceId, TextRange,
};

use crate::documents::{
    DocRef, Documents, OpenPage, load_doc, map_doc_error, map_model_error, new_doc_row, new_peer,
    rebuild_space_index,
};
use crate::history::{JournalWrite, Writer, record_journal};
use crate::properties::PropsScope;
use crate::{Core, map_storage_error, short_id, validate};

/// Largest update the editor may send in one call.
pub const MAX_DOC_UPDATE_BYTES: usize = 16 * 1024 * 1024;

/// Default number of search hits.
const DEFAULT_SEARCH_LIMIT: u32 = 20;
/// Longest block excerpt returned with a backlink, in characters.
const BACKLINK_TEXT_CHARS: usize = 200;
/// Most pages one `PageExcerpts` request asks about (ADR-0058).
pub(crate) const MAX_EXCERPT_PAGES: usize = 500;

/// Most characters of one excerpt, before the `…` of a cut (ADR-0058).
pub(crate) const EXCERPT_CHARS: usize = 280;

/// Most pages one `PageFirstImages` request asks about (ADR-0054).
pub(crate) const MAX_FIRST_IMAGE_PAGES: usize = 500;

impl Core {
    // ----- binary API ---------------------------------------------------------------

    /// Opens a page's document for an editor: loads it (or creates it with one empty
    /// paragraph when the page has none yet), keeps it in memory until
    /// [`Self::page_doc_close`], and returns the raw Loro snapshot the editor imports
    /// into its own replica. Unknown or purged pages are `NotFound`.
    pub fn page_doc_open(&mut self, page_id: PageId) -> Result<Vec<u8>, ApiError> {
        let open = self.open_page(page_id)?;
        let blob = open.model.doc().export_snapshot().map_err(map_doc_error)?;
        Ok(payload_of(&blob)?.to_vec())
    }

    /// Imports raw Loro update bytes produced by the editor replica, persists them with
    /// the derived index rows in one transaction and emits the content and backlink
    /// events. Updates above [`MAX_DOC_UPDATE_BYTES`] are `InvalidInput`.
    pub fn page_doc_import(&mut self, page_id: PageId, update: &[u8]) -> Result<(), ApiError> {
        if update.is_empty() {
            return Err(ApiError::invalid("update", "must not be empty"));
        }
        if update.len() > MAX_DOC_UPDATE_BYTES {
            return Err(ApiError::invalid(
                "update",
                format!("must be at most {MAX_DOC_UPDATE_BYTES} bytes"),
            ));
        }
        // A Viewer's or Commenter's edit, and any edit of an archive, is refused (ADR-0043).
        if let Ok(space_id) = self.space_of_page(page_id) {
            self.require_role(space_id, MemberRole::Editor)?;
        }
        let blob = encode_blob(BlobKind::Update, update);
        let open = self.open_page(page_id)?;
        open.model.doc().import(&blob).map_err(|err| match err {
            DocError::Engine(msg) => ApiError::invalid("update", format!("rejected: {msg}")),
            other => map_doc_error(other),
        })?;
        // The editor's own bytes are what gets persisted: they also carry operations
        // whose dependencies have not arrived yet, which an export would not include.
        self.persist_page(
            page_id,
            Some(blob),
            UpdateOrigin::Local,
            ChangeSource::Editor,
        )?;
        // The page's last edit moves with its content (throttled, ADR-0026 amendment).
        self.page_content_edited(page_id)
    }

    /// Raw Loro update with everything the editor replica lacks, given its encoded
    /// version vector (`None` = everything). Empty when there is nothing new.
    pub fn page_doc_export(
        &mut self,
        page_id: PageId,
        since: Option<&[u8]>,
    ) -> Result<Vec<u8>, ApiError> {
        let since = since
            .filter(|bytes| !bytes.is_empty())
            .map(|bytes| Version(bytes.to_vec()));
        let open = self.open_page(page_id)?;
        let exported = open
            .model
            .doc()
            .export_updates_since(since.as_ref())
            .map_err(|err| match (&err, &since) {
                (DocError::Corrupt(_), Some(_)) => {
                    ApiError::invalid("since", "not a valid version vector")
                }
                _ => map_doc_error(err),
            })?;
        match exported {
            Some(blob) => Ok(payload_of(&blob)?.to_vec()),
            None => Ok(Vec::new()),
        }
    }

    /// Drops the in-memory replica of a page (its content stays persisted). Closing a
    /// page that is not open is fine.
    pub fn page_doc_close(&mut self, page_id: PageId) -> Result<(), ApiError> {
        self.docs.pages.remove(&page_id);
        Ok(())
    }

    // ----- requests -----------------------------------------------------------------

    /// Dispatch entry for the Milestone 3 requests.
    pub(crate) fn dispatch_pages(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::Search {
                query,
                space_id,
                scope,
                limit,
                edited_after,
            } => Ok(Response::Search(self.search_pages(
                &query,
                space_id,
                scope.unwrap_or_default(),
                limit,
                edited_after,
                true,
            )?)),
            Request::BacklinksGet { page_id } => Ok(Response::Backlinks(self.backlinks(page_id)?)),
            Request::PageContentGet { page_id } => {
                let content = self.with_page_model(page_id, |model, _| model.to_portable())?;
                Ok(Response::PageContent(PageContent { page_id, content }))
            }
            Request::PageFirstImages { space_id, page_ids } => Ok(Response::PageFirstImages(
                self.page_first_images(space_id, &page_ids)?,
            )),
            Request::PageExcerpts { space_id, page_ids } => Ok(Response::PageExcerpts(
                self.page_excerpts(space_id, &page_ids)?,
            )),
            Request::PageExportMarkdown { page_id } => {
                let markdown = self.with_page_model(page_id, |model, db| {
                    model.to_markdown(&|id| title_of(db, id))
                })?;
                Ok(Response::Markdown(markdown))
            }
            Request::IndexRebuild { space_id } => {
                self.index_rebuild(space_id)?;
                Ok(Response::Ok)
            }
            other => Err(ApiError::internal(format!(
                "request {} is not a page request",
                other.kind()
            ))),
        }
    }

    /// `Request::PageFirstImages` (ADR-0054, `docs/architecture/database-views.md` section 9): the attachment of the
    /// first picture of each of `page_ids` that has one, from the device-local `page_first_images` table, in the
    /// order asked and each page once. `InvalidInput` (`pageIds`) for more than [`MAX_FIRST_IMAGE_PAGES`] ids,
    /// `NotFound` for an unknown space; pages that have no picture, are unknown or belong to another space are left
    /// out, live and trashed pages are both answered. A page that has no row yet (every page of a database from
    /// before schema v19, and any page nobody has written since) is **scanned here**: its document is loaded
    /// transiently, the first picture computed and the row inserted, so there is no backfill pass at open and the
    /// first gallery of an old database pays once for its own pages (at most one scan per page asked about, and a
    /// request asks about at most [`MAX_FIRST_IMAGE_PAGES`]). A page whose document cannot be read now is skipped
    /// and not recorded: it is asked about again.
    pub(crate) fn page_first_images(
        &mut self,
        space_id: SpaceId,
        page_ids: &[PageId],
    ) -> Result<Vec<PageFirstImage>, ApiError> {
        if page_ids.len() > MAX_FIRST_IMAGE_PAGES {
            return Err(ApiError::invalid(
                "pageIds",
                format!("at most {MAX_FIRST_IMAGE_PAGES} pages at a time"),
            ));
        }
        if !self.docs.structures.contains_key(&space_id) {
            return Err(ApiError::not_found("space not found"));
        }
        // The pages of this space among those asked, once each, in the order asked. The page index decides
        // the space (a page that moved is answered in its new space at once).
        let mut seen: HashSet<PageId> = HashSet::new();
        let mut asked: Vec<PageId> = Vec::new();
        for page_id in page_ids {
            if !seen.insert(*page_id) {
                continue;
            }
            let row = self
                .db
                .pages_index_get(*page_id)
                .map_err(map_storage_error)?;
            if row.is_some_and(|row| row.space_id == space_id) {
                asked.push(*page_id);
            }
        }
        let known: HashMap<PageId, Option<AttachmentId>> = self
            .db
            .first_images_get(&asked)
            .map_err(map_storage_error)?
            .into_iter()
            .collect();
        let mut scanned: Vec<(PageId, Option<AttachmentId>)> = Vec::new();
        let mut answer: Vec<PageFirstImage> = Vec::new();
        for page_id in asked {
            let first = match known.get(&page_id) {
                Some(first) => *first,
                None => match self.scan_first_image(page_id) {
                    Ok(first) => {
                        scanned.push((page_id, first));
                        first
                    }
                    Err(err) => {
                        tracing::debug!(page = %short_id(&page_id), error = %err, "first image not scanned");
                        continue;
                    }
                },
            };
            if let Some(attachment_id) = first {
                answer.push(PageFirstImage {
                    page_id,
                    attachment_id,
                });
            }
        }
        if !scanned.is_empty() {
            self.db
                .first_images_set(space_id, &scanned)
                .map_err(map_storage_error)?;
        }
        Ok(answer)
    }

    /// The first picture of a page from its document: the open one, else a transient load (a page that has no
    /// document yet has no picture). Nothing is created or kept.
    fn scan_first_image(&mut self, page_id: PageId) -> Result<Option<AttachmentId>, ApiError> {
        if let Some(open) = self.docs.pages.get(&page_id) {
            return Ok(open.model.first_image());
        }
        let Some(row) = self
            .db
            .doc_get(DocId::for_page(page_id))
            .map_err(map_storage_error)?
        else {
            return Ok(None);
        };
        let doc = load_doc(&self.db, &row, DocKind::Page, self.rng.as_ref())?;
        let model = PageDoc::from_doc(doc).map_err(map_model_error)?;
        Ok(model.first_image())
    }

    /// `Request::PageExcerpts` (ADR-0058, `docs/architecture/database-power-ups.md` section 6.2): the first
    /// lines of the text of each of `page_ids`, in the order asked and each page once, read from the text index
    /// (`fts_blocks`, the body rows in document order) with no document opened and nothing written. The text
    /// rule is [`excerpt_row`] and [`cut_excerpt`]. `InvalidInput` (`pageIds`) for more than
    /// [`MAX_EXCERPT_PAGES`] ids, `NotFound` for an unknown space; a page that is not a live page of the space
    /// (unknown, trashed, of another space), a template or a page inside one, and a page without body text are
    /// left out of the answer. A read: any member may ask. A page the index has not seen yet has no rows, so it
    /// answers no excerpt.
    pub(crate) fn page_excerpts(
        &mut self,
        space_id: SpaceId,
        page_ids: &[PageId],
    ) -> Result<Vec<PageExcerpt>, ApiError> {
        if page_ids.len() > MAX_EXCERPT_PAGES {
            return Err(ApiError::invalid(
                "pageIds",
                format!("at most {MAX_EXCERPT_PAGES} pages at a time"),
            ));
        }
        if !self.docs.structures.contains_key(&space_id) {
            return Err(ApiError::not_found("space not found"));
        }
        let mut seen: HashSet<PageId> = HashSet::new();
        let mut answer: Vec<PageExcerpt> = Vec::new();
        for page_id in page_ids {
            if !seen.insert(*page_id) {
                continue;
            }
            let row = self
                .db
                .pages_index_get(*page_id)
                .map_err(map_storage_error)?;
            let readable = row.is_some_and(|row| {
                row.space_id == space_id && row.trashed_at.is_none() && !row.in_template
            });
            if !readable {
                continue;
            }
            let mut text = String::new();
            let mut chars = 0usize;
            self.db
                .fts_page_body(*page_id, &mut |body| {
                    let Some(row) = excerpt_row(body) else {
                        return true;
                    };
                    if chars > 0 {
                        text.push('\n');
                        chars += 1;
                    }
                    chars += row.chars().count();
                    text.push_str(&row);
                    // Past the limit the rest of the page cannot change the excerpt.
                    chars <= EXCERPT_CHARS
                })
                .map_err(map_storage_error)?;
            if chars > 0 {
                answer.push(PageExcerpt {
                    page_id: *page_id,
                    text: cut_excerpt(text),
                });
            }
        }
        Ok(answer)
    }

    /// `Request::Search`: the full-text pass of `kb-storage` (a page matches when every word
    /// matches somewhere in it, ADR-0040), then, when that leaves room under the limit and `fuzzy_titles`
    /// is set, the fuzzy pass over the titles of the live pages. The MCP server asks without the fuzzy
    /// pass: what it lists contains the words it was asked for.
    pub(crate) fn search_pages(
        &self,
        query: &str,
        space_id: Option<SpaceId>,
        scope: SearchScope,
        limit: Option<u32>,
        edited_after: Option<i64>,
        fuzzy_titles: bool,
    ) -> Result<Vec<SearchHit>, ApiError> {
        let query = validate::search_query(query)?;
        if let Some(space_id) = space_id
            && !self.docs.structures.contains_key(&space_id)
        {
            return Err(ApiError::not_found("space not found"));
        }
        let limit = validate::search_limit(limit.unwrap_or(DEFAULT_SEARCH_LIMIT));
        let params = SearchParams {
            query: &query,
            space_id,
            titles_only: scope == SearchScope::Titles,
            limit,
            any_word: false,
            edited_after,
        };
        let hits = self.db.fts_search(&params).map_err(map_storage_error)?;
        let mut out = Vec::with_capacity(hits.len());
        for hit in hits {
            let Some(row) = self
                .db
                .pages_index_get(hit.page_id)
                .map_err(map_storage_error)?
            else {
                continue; // indexed text of a page that is gone; the rebuild cleans it up
            };
            out.push(self.search_hit(
                &row,
                // A hit on a property value (ADR-0026) is on the page, like a title hit: the
                // id the index keeps for it is the property's, not a block of the page.
                if hit.property { None } else { hit.block_id },
                hit.snippet,
                hit.highlights,
            ));
        }
        // The words of comments (ADR-0048), labelled "in comment" by the hit's `comment_id`: the
        // interactive search only (the fuzzy pass is what marks it), so the MCP server and the local AI
        // never list what people said in comments.
        if fuzzy_titles && scope != SearchScope::Titles {
            self.add_comment_hits(&params, limit, &mut out)?;
        }
        let room = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .saturating_sub(out.len());
        if room > 0 && fuzzy_titles {
            self.add_fuzzy_title_hits(&query, space_id, edited_after, room, &mut out)?;
        }
        Ok(out)
    }

    /// Adds the comments that hold the query's words after the hits already in `out`, best first, up to
    /// the limit: one hit per comment, on its page (no block), with the comment's text as the snippet.
    fn add_comment_hits(
        &self,
        params: &SearchParams<'_>,
        limit: u32,
        out: &mut Vec<SearchHit>,
    ) -> Result<(), ApiError> {
        let room = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .saturating_sub(out.len());
        if room == 0 {
            return Ok(());
        }
        for hit in self
            .db
            .comment_fts_search(params)
            .map_err(map_storage_error)?
            .into_iter()
            .take(room)
        {
            let Some(row) = self
                .db
                .pages_index_get(hit.page_id)
                .map_err(map_storage_error)?
            else {
                continue;
            };
            let mut found = self.search_hit(&row, None, hit.snippet, hit.highlights);
            found.comment_id = Some(hit.comment_id);
            out.push(found);
        }
        Ok(())
    }

    /// A search hit on a page, with the title and icon the structure document has now.
    fn search_hit(
        &self,
        row: &PageIndexRow,
        block_id: Option<BlockId>,
        snippet: String,
        highlights: Vec<(u32, u32)>,
    ) -> SearchHit {
        let info = self
            .docs
            .structures
            .get(&row.space_id)
            .and_then(|open| open.model.page(row.page_id));
        SearchHit {
            page_id: row.page_id,
            space_id: row.space_id,
            title: info
                .as_ref()
                .map_or_else(|| row.title.clone(), |p| p.title.clone()),
            icon: info.and_then(|p| p.icon),
            block_id,
            comment_id: None,
            snippet,
            highlights: highlights
                .into_iter()
                .map(|(start, end)| TextRange { start, end })
                .collect(),
        }
    }

    /// The fuzzy pass of a search (ADR-0040): the live pages that are not templates nor inside one
    /// (ADR-0056) (of `space_id`, edited at or after `edited_after`) whose title has the query's letters in order, or a word with one typo, best
    /// first, after the hits already in `out`; a page already hit is not repeated. At most `room` are
    /// added. The hit is the title: no block, the title as the snippet, the matched parts highlighted.
    fn add_fuzzy_title_hits(
        &self,
        query: &str,
        space_id: Option<SpaceId>,
        edited_after: Option<i64>,
        room: usize,
        out: &mut Vec<SearchHit>,
    ) -> Result<(), ApiError> {
        if FuzzyQuery::new(query).is_none() {
            return Ok(()); // fewer than two letters: nothing fuzzy to look for
        }
        let spaces: BTreeSet<SpaceId> = match space_id {
            Some(space_id) => BTreeSet::from([space_id]),
            None => self.docs.structures.keys().copied().collect(),
        };
        let already: HashSet<PageId> = out.iter().map(|hit| hit.page_id).collect();
        let mut rows: Vec<PageIndexRow> = Vec::new();
        for space in spaces {
            rows.extend(
                self.db
                    .pages_index_by_space(space)
                    .map_err(map_storage_error)?
                    .into_iter()
                    .filter(|row| {
                        row.trashed_at.is_none()
                            && !row.in_template
                            && !already.contains(&row.page_id)
                            && edited_after.is_none_or(|after| row.updated_at >= after)
                    }),
            );
        }
        let ranked = rank_fuzzy_titles(
            query,
            rows.iter().map(|row| (row, row.title.as_str())),
            room,
        );
        for (row, matched) in ranked {
            out.push(self.search_hit(row, None, row.title.clone(), matched.highlights));
        }
        Ok(())
    }

    fn backlinks(&self, page_id: PageId) -> Result<Vec<Backlink>, ApiError> {
        self.space_of_page(page_id)?;
        let sources = self.db.links_to(page_id).map_err(map_storage_error)?;
        let mut out = Vec::with_capacity(sources.len());
        for source in sources {
            let Some(row) = self
                .db
                .pages_index_get(source.from_page)
                .map_err(map_storage_error)?
            else {
                continue;
            };
            let info = self
                .docs
                .structures
                .get(&row.space_id)
                .and_then(|open| open.model.page(source.from_page));
            let text = self
                .db
                .fts_block_text(source.from_page, source.from_block)
                .map_err(map_storage_error)?
                .unwrap_or_default();
            out.push(Backlink {
                page_id: source.from_page,
                space_id: row.space_id,
                title: info.as_ref().map_or(row.title, |p| p.title.clone()),
                icon: info.and_then(|p| p.icon),
                block_id: source.from_block,
                text: truncate_chars(&text, BACKLINK_TEXT_CHARS),
            });
        }
        Ok(out)
    }

    /// Drops and rebuilds the text and link rows of one space or of every space.
    pub(crate) fn index_rebuild(&mut self, space_id: Option<SpaceId>) -> Result<(), ApiError> {
        let spaces: Vec<SpaceId> = match space_id {
            Some(id) => {
                if !self.docs.structures.contains_key(&id) {
                    return Err(ApiError::not_found("space not found"));
                }
                vec![id]
            }
            None => {
                self.db
                    .transaction(|tx| {
                        tx.fts_clear()?;
                        tx.date_mentions_clear()?;
                        tx.person_mentions_clear()?;
                        tx.comment_fts_clear()?;
                        tx.page_properties_clear()?;
                        tx.icon_refs_clear()?;
                        tx.first_images_clear()?;
                        tx.links_clear()
                    })
                    .map_err(map_storage_error)?;
                self.docs.structures.keys().copied().collect()
            }
        };
        for space in spaces {
            // The page index first: its rows say which pages are in a template (ADR-0056), which the
            // lists below leave out, and the text rebuild reads them.
            let open = self
                .docs
                .structures
                .get(&space)
                .ok_or_else(|| ApiError::not_found("space not found"))?;
            rebuild_space_index(&mut self.db, space, &open.model)?;
            self.rebuild_space_text_index(space)?;
            // The words of the comments of its pages (the records stay, the index follows them).
            self.comment_fts_rebuild_space(space)?;
        }
        if space_id.is_none() {
            // The property rows were cleared with the rest (and refilled from the documents):
            // the references of every attachment are looked at again.
            let all: Vec<AttachmentId> = self
                .db
                .attachments_all()
                .map_err(map_storage_error)?
                .into_iter()
                .map(|row| row.attachment_id)
                .collect();
            let now = self.clock.now_ms();
            self.attachment_refs_changed(all, now)?;
        }
        Ok(())
    }

    /// Rewrites the title, body, link and property rows of every page of a space from the
    /// documents (used by `IndexRebuild`, by the open-time check and after undo).
    pub(crate) fn rebuild_space_text_index(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        let rows = self
            .db
            .pages_index_by_space(space_id)
            .map_err(map_storage_error)?;
        let mut touched: BTreeSet<AttachmentId> = BTreeSet::new();
        for row in rows {
            touched.extend(
                self.db
                    .attachment_ids_referenced_by(row.page_id)
                    .map_err(map_storage_error)?,
            );
            let entry = self.text_entry(row.page_id, &row.title)?;
            touched.extend(entry.refs.iter().map(|(_, id)| *id));
            self.db
                .transaction(|tx| {
                    tx.fts_replace_page(row.page_id, &row.title, &entry.blocks)?;
                    tx.links_replace_from(row.page_id, &entry.links)?;
                    tx.date_mentions_replace(row.page_id, &entry.date_mentions)?;
                    tx.person_mentions_replace(
                        DocId::for_page(row.page_id),
                        &entry.person_mentions,
                    )?;
                    tx.first_images_set(space_id, &[(row.page_id, entry.first_image)])?;
                    tx.attachment_refs_replace(row.page_id, &entry.refs)
                })
                .map_err(map_storage_error)?;
        }
        // The property values of the space's pages (ADR-0026), from its structure document.
        self.refresh_property_rows(space_id, PropsScope::Space)?;
        // The image icons of its pages (ADR-0029), compared with the rows and rewritten if they differ.
        touched.extend(self.refresh_icon_refs(space_id)?);
        let now = self.clock.now_ms();
        self.attachment_refs_changed(touched, now)
    }

    // ----- hooks for the structure handlers -------------------------------------------

    /// After a page was created: its title becomes searchable right away.
    pub(crate) fn text_after_create(
        &mut self,
        page_id: PageId,
        title: &str,
    ) -> Result<(), ApiError> {
        self.db
            .fts_set_title(page_id, title)
            .map_err(map_storage_error)
    }

    /// After a rename: the title row changes, and the body rows of pages that
    /// reference this page (their text contains the title) are refreshed.
    pub(crate) fn text_after_rename(
        &mut self,
        page_id: PageId,
        title: &str,
    ) -> Result<(), ApiError> {
        self.db
            .fts_set_title(page_id, title)
            .map_err(map_storage_error)?;
        let sources: BTreeSet<PageId> = self
            .db
            .links_to(page_id)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|s| s.from_page)
            .collect();
        for source in sources {
            self.refresh_page_text(source)?;
        }
        // The pages whose relations list this page hold its title in their property text rows.
        let listing = self
            .db
            .page_properties_pages_relating_to(page_id)
            .map_err(map_storage_error)?;
        if !listing.is_empty() {
            let space_id = self.space_of_page(page_id)?;
            self.refresh_property_rows(space_id, PropsScope::Pages(&listing))?;
        }
        Ok(())
    }

    /// Pages referenced from `pages` see their inbound link set change when those
    /// pages are trashed or restored (trashed sources are hidden from backlinks).
    pub(crate) fn backlinks_changed_for_sources(
        &mut self,
        pages: &[PageId],
    ) -> Result<(), ApiError> {
        let mut targets = BTreeSet::new();
        for page in pages {
            for link in self.db.links_from(*page).map_err(map_storage_error)? {
                targets.insert(link.to_page);
            }
        }
        for target in targets {
            self.events
                .push(Event::BacklinksChanged { page_id: target });
        }
        Ok(())
    }

    /// Removes the derived rows and the in-memory replica of pages that left the
    /// structure document (purge, or an undone create). The document rows stay until
    /// the next open (`gc_page_documents`), so an undo that brings the page back finds
    /// its content again.
    pub(crate) fn text_after_remove(&mut self, pages: &[PageId]) -> Result<(), ApiError> {
        let mut targets = BTreeSet::new();
        for page in pages {
            for link in self.db.links_from(*page).map_err(map_storage_error)? {
                targets.insert(link.to_page);
            }
        }
        let referenced = self.attachments_referenced_by(pages);
        // Pages whose relations list a page that is going: their rows and search text change.
        let mut listing: BTreeSet<PageId> = BTreeSet::new();
        for page in pages {
            listing.extend(
                self.db
                    .page_properties_pages_relating_to(*page)
                    .map_err(map_storage_error)?,
            );
        }
        listing.retain(|page| !pages.contains(page));
        self.db
            .transaction(|tx| {
                for page in pages {
                    tx.fts_delete_page(*page)?;
                    tx.links_delete_page(*page)?;
                    tx.date_mentions_delete_page(*page)?;
                    tx.person_mentions_delete(DocId::for_page(*page))?;
                }
                tx.page_properties_delete_pages(pages)?;
                tx.first_images_delete_pages(pages)?;
                tx.attachment_refs_delete_pages(pages)
            })
            .map_err(map_storage_error)?;
        for page in pages {
            self.docs.pages.remove(page);
        }
        // The comment streams of the pages go with them (their key records are tombstoned with the
        // pages' below, and the rows of the stream are crypto-erased the same way).
        let streams = self.comments_forget_pages(pages)?;
        let mut by_space_listing: std::collections::HashMap<SpaceId, Vec<PageId>> =
            std::collections::HashMap::new();
        for page in listing {
            if let Ok(space) = self.space_of_page(page) {
                by_space_listing.entry(space).or_default().push(page);
            }
        }
        for (space, listed) in by_space_listing {
            self.refresh_property_rows(space, PropsScope::Pages(&listed))?;
        }
        let now = self.clock.now_ms();
        self.attachment_refs_changed(referenced, now)?;
        // Crypto-erase: the documents' key records are tombstoned (the rows go at the
        // next open, so an undo can still bring the content back).
        let mut by_space: std::collections::HashMap<SpaceId, Vec<DocId>> =
            std::collections::HashMap::new();
        for page in pages {
            if let Ok(Some(row)) = self.db.doc_get(DocId::for_page(*page))
                && let Some(space) = row.space_id
            {
                by_space.entry(space).or_default().push(row.doc_id);
            }
        }
        for (space, doc_id) in streams {
            by_space.entry(space).or_default().push(doc_id);
        }
        for (space, docs) in by_space {
            // A tombstone of a shared space is an Owner's (the others' would be refused): a page
            // another device purged is only forgotten here (ADR-0047).
            if self.may_purge_in(space) {
                self.keyring_tombstone_docs(space, &docs)?;
            }
        }
        for target in targets {
            if !pages.contains(&target) {
                self.events
                    .push(Event::BacklinksChanged { page_id: target });
            }
        }
        Ok(())
    }

    /// After undo/redo in a space: titles may have changed, pages may have come back
    /// (re-index their content) or gone (drop their rows).
    pub(crate) fn text_after_undo(
        &mut self,
        space_id: SpaceId,
        before: &HashSet<PageId>,
        after: &HashSet<PageId>,
    ) -> Result<(), ApiError> {
        let rows = self
            .db
            .pages_index_by_space(space_id)
            .map_err(map_storage_error)?;
        self.db
            .transaction(|tx| {
                for row in &rows {
                    tx.fts_set_title(row.page_id, &row.title)?;
                }
                Ok(())
            })
            .map_err(map_storage_error)?;
        let returned: Vec<PageId> = after.difference(before).copied().collect();
        if !returned.is_empty() {
            // A page that comes back brings its values with it.
            self.refresh_property_rows(space_id, PropsScope::Pages(&returned))?;
        }
        for page in returned {
            self.refresh_page_text(page)?;
        }
        let gone: Vec<PageId> = before.difference(after).copied().collect();
        if !gone.is_empty() {
            self.text_after_remove(&gone)?;
        }
        Ok(())
    }

    /// Deletes the documents of pages that no longer exist (purged before the last
    /// close). Runs at open, after the page index was verified. A page without an index row is
    /// not necessarily gone: one that is residue in any structure moved to another space and
    /// may not have arrived on this device yet (ADR-0038), and one hidden below residue is
    /// about to be re-rooted; their documents stay.
    pub(crate) fn gc_page_documents(db: &mut Db, docs: &Documents) -> Result<(), ApiError> {
        let mut in_transit: HashSet<PageId> = HashSet::new();
        for open in docs.structures.values() {
            in_transit.extend(open.model.moved_out().into_iter().map(|(page, _, _)| page));
            in_transit.extend(open.model.orphans());
        }
        let rows = db
            .docs_by_kind(DocKindCode::Page)
            .map_err(map_storage_error)?;
        let mut orphans = Vec::new();
        for row in rows {
            let page_id = PageId::from_bytes(row.doc_id.to_bytes());
            if in_transit.contains(&page_id) {
                continue;
            }
            if db
                .pages_index_get(page_id)
                .map_err(map_storage_error)?
                .is_none()
            {
                orphans.push((row.doc_id, page_id));
            }
        }
        if orphans.is_empty() {
            return Ok(());
        }
        db.transaction(|tx| {
            for (doc_id, page_id) in &orphans {
                tx.doc_delete(*doc_id)?;
                tx.fts_delete_page(*page_id)?;
                tx.links_delete_page(*page_id)?;
                tx.date_mentions_delete_page(*page_id)?;
                tx.person_mentions_delete(DocId::from_bytes(page_id.to_bytes()))?;
                tx.page_properties_delete_pages(&[*page_id])?;
                tx.first_images_delete_pages(&[*page_id])?;
            }
            Ok(())
        })
        .map_err(map_storage_error)?;
        tracing::info!(count = orphans.len(), "removed documents of purged pages");
        Ok(())
    }

    /// The spaces whose text index holds a number of pages other than the structure's, as
    /// `(space, indexed, pages)`. Every page of a space has a title row, whatever its state: the live
    /// pages, the trashed ones and the sub-pages of a trashed page (they stay in the trashed page's
    /// subtree), so `pages` counts all of them.
    pub(crate) fn spaces_with_stale_text_index(
        &self,
    ) -> Result<Vec<(SpaceId, u32, u32)>, ApiError> {
        let mut stale = Vec::new();
        for (space_id, open) in &self.docs.structures {
            let pages = u32::try_from(open.model.all_page_ids().len()).unwrap_or(u32::MAX);
            let indexed = self
                .db
                .fts_page_count(*space_id)
                .map_err(map_storage_error)?;
            if indexed != pages {
                stale.push((*space_id, indexed, pages));
            }
        }
        Ok(stale)
    }

    /// Repairs the text index of spaces whose title-row count disagrees with their
    /// pages (databases from before Milestone 3, interrupted rebuilds).
    pub(crate) fn verify_text_index(&mut self) -> Result<(), ApiError> {
        for (space_id, indexed, pages) in self.spaces_with_stale_text_index()? {
            tracing::warn!(space = %short_id(&space_id), indexed, pages, "text index out of sync; rebuilding");
            self.rebuild_space_text_index(space_id)?;
        }
        // A database from before schema v9 has no date-mention rows yet.
        self.verify_date_mentions()?;
        // Nor does one from before schema v10 have property rows.
        self.verify_page_properties()
    }

    // ----- plumbing -----------------------------------------------------------------

    /// The open document of a page, loading or creating it first.
    pub(crate) fn open_page(&mut self, page_id: PageId) -> Result<&OpenPage, ApiError> {
        if !self.docs.pages.contains_key(&page_id) {
            let space_id = self.space_of_page(page_id)?;
            let open = self.load_or_create_page(page_id, space_id)?;
            self.docs.pages.insert(page_id, open);
        }
        self.docs
            .pages
            .get(&page_id)
            .ok_or_else(|| ApiError::internal("page document vanished"))
    }

    /// Loads a page's document, or creates it (one empty paragraph) with its rows.
    pub(crate) fn load_or_create_page(
        &mut self,
        page_id: PageId,
        space_id: SpaceId,
    ) -> Result<OpenPage, ApiError> {
        let doc_id = DocId::for_page(page_id);
        if let Some(row) = self.db.doc_get(doc_id).map_err(map_storage_error)? {
            let doc = load_doc(&self.db, &row, DocKind::Page, self.rng.as_ref())?;
            let model = PageDoc::from_doc(doc).map_err(map_model_error)?;
            let last_persisted = model.doc().version();
            return Ok(OpenPage {
                doc_id,
                model,
                last_persisted,
            });
        }

        let now = self.clock.now_ms();
        let first_block = BlockId::generate(self.rng.as_ref());
        let model = PageDoc::new(
            Doc::new(DocKind::Page, new_peer(self.rng.as_ref())),
            first_block,
        )
        .map_err(map_model_error)?;
        let blob = model
            .doc()
            .export_updates_since(None)
            .map_err(map_doc_error)?
            .ok_or_else(|| ApiError::internal("new page document exported nothing"))?;
        let title = self.page_title(page_id)?;
        let row = new_doc_row(doc_id, DocKindCode::Page, Some(space_id), now);
        // The page's history starts here (ADR-0057): the editing that follows within the session gap
        // continues this row, so a page whose journal starts at its creation has no undated versions.
        // It rides in the transaction that creates the document.
        let journal = self.journal_plan_new(page_id, &model.doc().frontiers(), Some(title.clone()));
        self.db
            .transaction(|tx| {
                tx.doc_insert(&row)?;
                tx.update_append(doc_id, &blob, UpdateOrigin::Core, now)?;
                record_journal(tx, page_id, journal.as_ref());
                tx.fts_replace_page(page_id, &title, &[])?;
                tx.date_mentions_replace(page_id, &[])?;
                tx.links_replace_from(page_id, &[])
            })
            .map_err(map_storage_error)?;
        let last_persisted = model.doc().version();
        self.ensure_doc_key(space_id, doc_id)?;
        tracing::debug!(page = %short_id(&page_id), "created page document");
        Ok(OpenPage {
            doc_id,
            model,
            last_persisted,
        })
    }

    /// Runs `f` on a page's model: the open one, or a transient load (the document is
    /// created first when the page has none yet).
    fn with_page_model<T>(
        &mut self,
        page_id: PageId,
        f: impl FnOnce(&PageDoc, &Db) -> T,
    ) -> Result<T, ApiError> {
        if let Some(open) = self.docs.pages.get(&page_id) {
            return Ok(f(&open.model, &self.db));
        }
        let space_id = self.space_of_page(page_id)?;
        let open = self.load_or_create_page(page_id, space_id)?;
        Ok(f(&open.model, &self.db))
    }

    /// The text and link rows of a page from its document (empty when it has none).
    pub(crate) fn text_entry(
        &mut self,
        page_id: PageId,
        title: &str,
    ) -> Result<TextEntry, ApiError> {
        let _ = title;
        if let Some(open) = self.docs.pages.get(&page_id) {
            return Ok(text_entry_of(&open.model, &self.db));
        }
        let doc_id = DocId::for_page(page_id);
        let Some(row) = self.db.doc_get(doc_id).map_err(map_storage_error)? else {
            return Ok(TextEntry::default());
        };
        let doc = load_doc(&self.db, &row, DocKind::Page, self.rng.as_ref())?;
        let model = PageDoc::from_doc(doc).map_err(map_model_error)?;
        Ok(text_entry_of(&model, &self.db))
    }

    /// Rewrites the body rows of one page (its title row is left as it is).
    pub(crate) fn refresh_page_text(&mut self, page_id: PageId) -> Result<(), ApiError> {
        let Some(row) = self
            .db
            .pages_index_get(page_id)
            .map_err(map_storage_error)?
        else {
            return Ok(());
        };
        let entry = self.text_entry(page_id, &row.title)?;
        let old_refs = self
            .db
            .attachment_ids_referenced_by(page_id)
            .map_err(map_storage_error)?;
        self.db
            .transaction(|tx| {
                tx.fts_replace_page(page_id, &row.title, &entry.blocks)?;
                tx.links_replace_from(page_id, &entry.links)?;
                tx.date_mentions_replace(page_id, &entry.date_mentions)?;
                tx.person_mentions_replace(DocId::for_page(page_id), &entry.person_mentions)?;
                tx.first_images_set(row.space_id, &[(page_id, entry.first_image)])?;
                tx.attachment_refs_replace(page_id, &entry.refs)
            })
            .map_err(map_storage_error)?;
        let now = self.clock.now_ms();
        let touched: BTreeSet<AttachmentId> = old_refs
            .into_iter()
            .chain(entry.refs.iter().map(|(_, id)| *id))
            .collect();
        self.attachment_refs_changed(touched, now)
    }

    /// Persists what changed in an open page document: the update row (the given blob,
    /// or an export since the last persisted version), the page's text rows and its
    /// outgoing links, in one transaction; then the events and the compaction check.
    pub(crate) fn persist_page(
        &mut self,
        page_id: PageId,
        blob: Option<Vec<u8>>,
        origin: UpdateOrigin,
        source: ChangeSource,
    ) -> Result<(), ApiError> {
        self.persist_page_as(page_id, blob, origin, source, JournalKind::Edit)
    }

    /// [`Self::persist_page`] that says what the page's history records the write as (ADR-0057): an
    /// edit, which continues the editing session of this device when there is one, or a restore,
    /// which always starts its own row. The write is journaled after it is stored, as this device's;
    /// a write that changed nothing in the document is not.
    pub(crate) fn persist_page_as(
        &mut self,
        page_id: PageId,
        blob: Option<Vec<u8>>,
        origin: UpdateOrigin,
        source: ChangeSource,
        journal: JournalKind,
    ) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let title = self.page_title(page_id)?;
        let space_id = self.space_of_page(page_id)?;
        let old_targets: BTreeSet<PageId> = self
            .db
            .links_from(page_id)
            .map_err(map_storage_error)?
            .into_iter()
            .map(|l| l.to_page)
            .collect();

        let open = self
            .docs
            .pages
            .get(&page_id)
            .ok_or_else(|| ApiError::not_found("page document is not open"))?;
        let blob = match blob {
            Some(blob) => Some(blob),
            None => open
                .model
                .doc()
                .export_updates_since(Some(&open.last_persisted))
                .map_err(map_doc_error)?,
        };
        let entry = text_entry_of(&open.model, &self.db);
        let doc_id = open.doc_id;
        // The versions around the write, for the page history journal: what was persisted before
        // (the document was changed in memory first) and what the document is now.
        let before = open.model.doc().frontiers_at(&open.last_persisted).ok();
        let after = open.model.doc().frontiers();
        // The journal's write is decided now (it only reads) and rides in the transaction below.
        let journal_op = before.as_ref().and_then(|before| {
            self.journal_plan(
                &JournalWrite {
                    page_id,
                    before,
                    after: &after,
                    writer: Writer::ThisDevice,
                    kind: journal,
                },
                Some(title.clone()),
            )
        });
        let new_targets: BTreeSet<PageId> = entry.links.iter().map(|l| l.to_page).collect();
        let old_refs = self
            .db
            .attachment_ids_referenced_by(page_id)
            .map_err(map_storage_error)?;

        let stats = self
            .db
            .transaction(|tx| {
                if let Some(blob) = &blob {
                    tx.update_append(doc_id, blob, origin, now)?;
                }
                tx.fts_replace_page(page_id, &title, &entry.blocks)?;
                tx.links_replace_from(page_id, &entry.links)?;
                tx.date_mentions_replace(page_id, &entry.date_mentions)?;
                tx.person_mentions_replace(DocId::for_page(page_id), &entry.person_mentions)?;
                tx.first_images_set(space_id, &[(page_id, entry.first_image)])?;
                tx.attachment_refs_replace(page_id, &entry.refs)?;
                record_journal(tx, page_id, journal_op.as_ref());
                tx.update_stats(doc_id)
            })
            .map_err(map_storage_error)?;
        if let Some(open) = self.docs.pages.get_mut(&page_id) {
            open.last_persisted = open.model.doc().version();
        }
        let touched: BTreeSet<AttachmentId> = old_refs
            .into_iter()
            .chain(entry.refs.iter().map(|(_, id)| *id))
            .collect();
        self.attachment_refs_changed(touched, now)?;
        if origin != UpdateOrigin::Remote {
            self.sync_note_local_change();
        }

        self.events
            .push(Event::PageContentChanged { page_id, source });
        // View links (M13 slice 4): a link of this page is refreshed a minute after its first
        // unpublished change. Best effort: a failure here never fails the edit.
        let _ = self.view_links_page_changed(page_id, source);
        for target in old_targets.symmetric_difference(&new_targets) {
            self.events
                .push(Event::BacklinksChanged { page_id: *target });
        }
        self.maybe_compact(DocRef::Page(page_id), doc_id, stats)
    }

    /// Current title of a page from the index (`NotFound` for unknown pages).
    pub(crate) fn page_title(&self, page_id: PageId) -> Result<String, ApiError> {
        self.db
            .pages_index_get(page_id)
            .map_err(map_storage_error)?
            .map(|row| row.title)
            .ok_or_else(|| ApiError::not_found("page not found"))
    }
}

/// Text, link, date-mention and person-mention rows of one page.
#[derive(Default)]
pub(crate) struct TextEntry {
    pub(crate) blocks: Vec<FtsBlock>,
    pub(crate) links: Vec<LinkRow>,
    /// The page's date mentions (the derived `date_mentions` table, ADR-0024).
    pub(crate) date_mentions: Vec<DateMentionRow>,
    /// The page's person mentions (the derived `person_mentions` table, ADR-0049).
    pub(crate) person_mentions: Vec<PersonMentionRow>,
    /// Blocks referencing attachments (`image`, `file`), in document order.
    pub(crate) refs: Vec<(BlockId, AttachmentId)>,
    /// The attachment of the page's first picture, for the gallery (the derived `page_first_images` table,
    /// ADR-0054).
    pub(crate) first_image: Option<AttachmentId>,
}

/// Extracts the indexable text (page references resolved to titles) and the links. A
/// `pageLink` block (the card, 2026-10-06) reads as the one reference it stands for
/// (`PageDoc::blocks`) and counts as a link of its own block (`PageDoc::links`), so a card
/// is indexed by the title of its page and listed in its backlinks exactly like a chip. A date
/// mention (`dateMention`, 2026-10-07) needs no case of its own: `PageDoc::blocks` already folds
/// its ISO text (`2026-10-07`, `2026-10-07 14:30`) into the text run around it, so the index and the
/// backlinks' text hold the date as typed.
pub(crate) fn text_entry_of(model: &PageDoc, db: &Db) -> TextEntry {
    let refs = model.attachment_refs();
    // The content of a template button is template material (ADR-0056): not even a file's name in it is searched.
    let template_material = model.template_blocks();
    // File blocks carry no text of their own: the attachment's name is what people
    // search for (ADR-0020), so it comes from the index.
    let file_names: std::collections::HashMap<BlockId, String> = refs
        .iter()
        .filter(|(block, _)| !template_material.contains(block))
        .filter_map(|(block, attachment)| {
            db.attachment_get(*attachment)
                .ok()
                .flatten()
                .map(|row| (*block, row.name))
        })
        .collect();
    let blocks = model
        .blocks()
        .into_iter()
        .filter_map(|block| {
            let mut text = String::new();
            if block.kind == "file"
                && let Some(name) = file_names.get(&block.id)
            {
                text.push_str(name);
            }
            for inline in &block.inlines {
                match inline {
                    Inline::Text(s) => text.push_str(s),
                    Inline::PageRef { page_id, .. } => {
                        if let Some(title) = title_of(db, *page_id) {
                            text.push_str(&title);
                        }
                    }
                    Inline::Break => text.push(' '),
                }
            }
            if text.trim().is_empty() {
                None
            } else {
                Some(FtsBlock {
                    block_id: block.id,
                    text,
                })
            }
        })
        .collect();
    let links = model
        .links()
        .into_iter()
        .map(|link| LinkRow {
            from_block: link.from_block,
            to_page: link.to_page,
            to_block: link.to_block,
        })
        .collect();
    TextEntry {
        blocks,
        links,
        date_mentions: crate::reminders::date_mention_rows(model),
        person_mentions: crate::mentions::person_mention_rows(model),
        refs,
        first_image: model.first_image(),
    }
}

/// A page's title from the index, for Markdown and text extraction.
pub(crate) fn title_of(db: &Db, page_id: PageId) -> Option<String> {
    db.pages_index_get(page_id)
        .ok()
        .flatten()
        .map(|row| row.title)
}

/// Strips the `KBD1` envelope; the editor only ever sees raw engine bytes.
fn payload_of(blob: &[u8]) -> Result<&[u8], ApiError> {
    decode_blob(blob)
        .map(|(_, payload)| payload)
        .map_err(map_doc_error)
}

/// The first `max` characters of `text`, on a character boundary.
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// One indexed body row as a line of an excerpt (ADR-0058, section 6.2): the `U+FFFD` the index puts for
/// inline atoms removed, every run of whitespace one space, trimmed; `None` when nothing is left. The
/// twin is `excerptRow` in `content.ts`.
pub(crate) fn excerpt_row(row: &str) -> Option<String> {
    let mut line = String::with_capacity(row.len().min(2 * EXCERPT_CHARS));
    let mut gap = false;
    for ch in row.chars().filter(|ch| *ch != '\u{FFFD}') {
        if ch.is_whitespace() {
            gap = !line.is_empty();
        } else {
            if gap {
                line.push(' ');
                gap = false;
            }
            line.push(ch);
        }
    }
    (!line.is_empty()).then_some(line)
}

/// The excerpt of the lines of a page joined with `\n`: the text itself when it has at most
/// [`EXCERPT_CHARS`] Unicode scalar values, else cut at the last whitespace among the first
/// [`EXCERPT_CHARS`] (at the limit when there is none) with `…` appended. The twin is `cutExcerpt` in
/// `content.ts`.
pub(crate) fn cut_excerpt(text: String) -> String {
    let Some((limit, _)) = text.char_indices().nth(EXCERPT_CHARS) else {
        return text;
    };
    let head = &text[..limit];
    let end = head
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(limit, |(at, _)| at);
    // A cut at the very start would leave only the mark: keep the limit then.
    let end = if end == 0 { limit } else { end };
    format!("{}…", &text[..end])
}

/// Maps storage failures of the derived indexes; a missing FTS5 is a storage error.
#[allow(dead_code)]
fn map_index_error(err: StorageError) -> ApiError {
    ApiError::new(ErrorCode::Storage, format!("text index error: {err}"))
}
