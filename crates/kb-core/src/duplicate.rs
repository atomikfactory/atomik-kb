//! Duplicating a page with its sub-pages (ADR-0040, "Duplicate a page with its sub-pages").
//!
//! `PageDuplicate { pageId }` copies the live subtree of a page in the same space and answers the
//! copy of the page. The reading and the checks come first and write nothing; then:
//!
//! 1. **Structure** (`kb_model::StructureDoc::duplicate_subtree`): new page ids, the top copy
//!    right after the page among its siblings and titled `"<title> (copy)"`, the others with their
//!    titles, icons, and the property values through the normal value path (a relation that lists a
//!    page of the subtree lists its copy; a two-way pair stays paired). The creation is one
//!    structural undo step. The structure update is persisted with the index rows, the property
//!    rows and the icon references of the copies, and of the pages whose paired relation changed.
//! 2. **Documents**: each copy's content is the portable JSON of the original with new block ids,
//!    page references and cards remapped to the copies, and the drawings under the new block ids
//!    (`kb_model::remap_portable`), built into a new document that is written like an imported page
//!    (`Core::import_write_documents`): the document row, its key record under the page's space,
//!    the update (a local one, which the sync outbox pushes) and the derived rows in one
//!    transaction (text, links, date mentions, attachment references), then the attachment
//!    bookkeeping and the events. A page that has no document yet is copied as an empty page; the
//!    original is not given one.
//!
//! Attachments are never copied: the copies reference the same attachments by id, and the
//! references recorded for them keep the files alive when the original goes away. The copy
//! title's " (copy)" is English only, because the core has no translations.
//!
//! The same machinery makes a page from a template (ADR-0056, `template_create.rs`): the shared
//! steps are [`Core::prepare_copies`] (the new ids and the rebuilt documents) and
//! [`Core::finish_copies`] (the persisted structure, the titles, the events and the documents).
//! A duplicate copies the template flag and the placeholders of the pages (the duplicate of a
//! template is a template) and follows a view's `defaultTemplate` into the copies.

use std::collections::{HashMap, HashSet};

use kb_doc::{Doc, DocKind};
use kb_model::{
    CopyIds, Duplicated, PageDoc, TemplateFill, fill_template_content, portable_block_ids,
    remap_portable,
};
use kb_storage::{DocQueries, TextIndexQueries, UpdateOrigin};
use kb_types::{ApiError, BlockId, DocId, Event, MemberRole, PageId, PageInfo, SpaceId};
use serde_json::Value;

use crate::documents::{load_doc, map_doc_error, map_model_error, new_peer};
use crate::import::PreparedDoc;
use crate::pages::text_entry_of;
use crate::properties::PropsScope;
use crate::validate::MAX_PAGE_TITLE_CHARS;
use crate::{Core, map_storage_error};

/// Most pages one duplication copies, the page itself included.
pub const MAX_DUPLICATED_PAGES: usize = 1_000;

/// What the title of the copy of a page gets appended (English only: the core has no translations).
const COPY_SUFFIX: &str = " (copy)";

/// The title of the copy of a page: the title and `" (copy)"`, the title cut to fit the limit of
/// a page title when it would not. An untitled page's copy is titled `"(copy)"`.
pub(crate) fn copy_title(title: &str) -> String {
    let room = MAX_PAGE_TITLE_CHARS - COPY_SUFFIX.chars().count();
    let kept: String = title.chars().take(room).collect();
    format!("{}{COPY_SUFFIX}", kept.trim_end())
        .trim_start()
        .to_owned()
}

/// What a copy of a subtree needs before anything is written: the new ids and the rebuilt
/// documents.
pub(crate) struct PreparedCopies {
    /// The copy of every page of the subtree.
    pub ids: HashMap<PageId, PageId>,
    /// The new document of every copy, in the order of the subtree.
    pub models: Vec<(PageId, PageDoc)>,
}

impl Core {
    /// Duplicates a live page with its live sub-pages and answers the copy of the page.
    pub(crate) fn page_duplicate(&mut self, page_id: PageId) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let open = self.structure(space_id)?;
        let info = open
            .model
            .page(page_id)
            .ok_or_else(|| ApiError::not_found("page not found"))?;
        if info.trashed_at.is_some() {
            return Err(ApiError::invalid("pageId", "the page is in the trash"));
        }
        // The live subtree, pre-order (a trashed sub-page hangs under the trash root: not in it).
        let sources = open.model.subtree(page_id).map_err(map_model_error)?;
        if sources.len() > MAX_DUPLICATED_PAGES {
            return Err(ApiError::invalid(
                "pageId",
                format!("a page is duplicated with at most {MAX_DUPLICATED_PAGES} pages"),
            ));
        }

        // Everything that can fail is done before anything is written: the new ids, the content of
        // every page read and rebuilt with them.
        let PreparedCopies { ids, models } = self.prepare_copies(&sources, None)?;

        // 1. The pages and their values, one persisted update.
        let now = self.clock.now_ms();
        let title = copy_title(&info.title);
        let duplicated = self
            .structure_mut(space_id)?
            .model
            .duplicate_subtree(page_id, &ids, &title, now)
            .map_err(map_model_error)?;
        self.finish_copies(space_id, &duplicated, models, now)?;
        tracing::info!(pages = duplicated.pages.len(), "page duplicated");

        let top = *ids
            .get(&page_id)
            .ok_or_else(|| ApiError::internal("the page has no copy"))?;
        self.page_get(top)
    }

    /// The new ids and the new documents of the copies of `sources` (a subtree, pre-order): each
    /// document is the portable JSON of the original with new block ids and the links and cards
    /// into the subtree remapped, and, for a page made from a template, its relative mentions
    /// resolved with `fill`. A page that has no document yet is copied as an empty page. Reads
    /// and builds only; nothing is written.
    pub(crate) fn prepare_copies(
        &mut self,
        sources: &[PageId],
        fill: Option<&TemplateFill<'_>>,
    ) -> Result<PreparedCopies, ApiError> {
        let mut ids: HashMap<PageId, PageId> = HashMap::with_capacity(sources.len());
        for source in sources {
            ids.insert(*source, PageId::generate(self.rng.as_ref()));
        }
        let mut contents: Vec<Option<Value>> = Vec::with_capacity(sources.len());
        for source in sources {
            contents.push(self.portable_content(*source)?);
        }
        let mut copy_ids = CopyIds {
            pages: ids.clone(),
            blocks: HashMap::new(),
        };
        for content in contents.iter().flatten() {
            for block in portable_block_ids(content) {
                copy_ids
                    .blocks
                    .entry(block)
                    .or_insert_with(|| BlockId::generate(self.rng.as_ref()));
            }
        }
        let mut models: Vec<(PageId, PageDoc)> = Vec::with_capacity(sources.len());
        for (source, content) in sources.iter().zip(&contents) {
            let copy = *ids
                .get(source)
                .ok_or_else(|| ApiError::internal("a page of the subtree has no copy"))?;
            let doc = Doc::new(DocKind::Page, new_peer(self.rng.as_ref()));
            let model = match content {
                Some(content) => {
                    let remapped = remap_portable(content, &copy_ids);
                    let rebuilt = match fill {
                        Some(fill) => fill_template_content(&remapped, fill),
                        None => remapped,
                    };
                    PageDoc::from_portable(doc, &rebuilt)
                }
                None => PageDoc::new(doc, BlockId::generate(self.rng.as_ref())),
            }
            .map_err(map_model_error)?;
            models.push((copy, model));
        }
        Ok(PreparedCopies { ids, models })
    }

    /// Persists what a copy of a subtree wrote in the structure (the pages and their values, with
    /// the index rows, the property rows and the icon references of the copies and of the pages
    /// whose paired relation changed), makes the titles searchable, announces the structure, then
    /// writes the documents of the copies with their keys and derived rows
    /// ([`Core::import_write_documents`]) and their events.
    pub(crate) fn finish_copies(
        &mut self,
        space_id: SpaceId,
        duplicated: &Duplicated,
        models: Vec<(PageId, PageDoc)>,
        now: i64,
    ) -> Result<(), ApiError> {
        let copies: Vec<PageId> = duplicated.pages.iter().map(|(_, copy)| *copy).collect();
        let mut affected = copies.clone();
        let known: HashSet<PageId> = copies.iter().copied().collect();
        affected.extend(
            duplicated
                .touched
                .iter()
                .filter(|page| !known.contains(page))
                .copied(),
        );
        self.persist_structure_with(
            space_id,
            Some(&affected),
            UpdateOrigin::Local,
            PropsScope::Pages(&affected),
        )?;
        let titles: Vec<(PageId, String)> = {
            let model = &self.structure(space_id)?.model;
            copies
                .iter()
                .filter_map(|copy| Some((*copy, model.page(*copy)?.title)))
                .collect()
        };
        // The titles are searchable at once, whatever happens to the documents.
        self.db
            .transaction(|tx| {
                for (copy, title) in &titles {
                    tx.fts_set_title(*copy, title)?;
                }
                Ok(())
            })
            .map_err(map_storage_error)?;
        self.events.push(Event::StructureChanged { space_id });
        self.events.push(Event::SpacesChanged);

        // The documents with their keys and derived rows (the titles are in the index now, so
        // the text of a reference to a copy reads the copy's title).
        let titles: HashMap<PageId, String> = titles.into_iter().collect();
        let mut prepared: Vec<PreparedDoc> = Vec::with_capacity(models.len());
        for (copy, model) in models {
            let blob = model
                .doc()
                .export_updates_since(None)
                .map_err(map_doc_error)?
                .ok_or_else(|| ApiError::internal("a new page document exported nothing"))?;
            let entry = text_entry_of(&model, &self.db);
            prepared.push(PreparedDoc {
                page_id: copy,
                doc_id: DocId::for_page(copy),
                title: titles.get(&copy).cloned().unwrap_or_default(),
                blob,
                version: model.doc().frontiers(),
                entry,
            });
        }
        self.import_write_documents(space_id, prepared, now)?;
        Ok(())
    }

    /// The portable JSON of a page's content as it is now (the open document when an editor holds
    /// it, else the stored one); `None` for a page that has no document yet. Creates nothing.
    pub(crate) fn portable_content(&self, page_id: PageId) -> Result<Option<Value>, ApiError> {
        if let Some(open) = self.docs.pages.get(&page_id) {
            return Ok(Some(open.model.to_portable()));
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
        Ok(Some(model.to_portable()))
    }
}

#[cfg(test)]
mod tests {
    use super::copy_title;

    #[test]
    fn the_copy_title_appends_the_suffix_and_keeps_within_the_limit() {
        assert_eq!(copy_title("Plan"), "Plan (copy)");
        assert_eq!(copy_title(""), "(copy)");
        assert_eq!(copy_title("Plan (copy)"), "Plan (copy) (copy)");
        let exact = "a".repeat(493);
        assert_eq!(copy_title(&exact).chars().count(), 500);
        let long = "é".repeat(600);
        let cut = copy_title(&long);
        assert_eq!(cut.chars().count(), 500);
        assert!(cut.ends_with("é (copy)"));
        // A cut that ends on a space does not leave two before the suffix.
        let spaced = format!("{}  tail", "b".repeat(491));
        assert_eq!(copy_title(&spaced), format!("{} (copy)", "b".repeat(491)));
    }
}
