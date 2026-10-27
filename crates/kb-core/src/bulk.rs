//! Bulk edit of database rows (ADR-0058, `docs/architecture/database-power-ups.md` section 5.4):
//! one action applied to a set of pages of one space, all or nothing, a structural action in one
//! undo step.
//!
//! `PageBulk { spaceId, pageIds, action }` checks everything first and writes nothing until it
//! has (the role, the pages, the action's own arguments), then:
//!
//! - **`SetValue` / `SetValues`** are one properties change like a single `PagePropertySet`, for
//!   every page: one commit, one persist with the derived rows of the pages written, one
//!   `StructureChanged`. Property values are not in structural undo (their origin is excluded), so
//!   the UI's undo sends the previous values back with `SetValues`.
//! - **`Move`, `Duplicate` and `Trash`** first drop the pages whose ancestor is in the list (they
//!   go with it), then write all the pages in one commit that carries no origin, so structural
//!   undo records **one step** and one `StructureUndo` puts everything back
//!   (`kb_model::StructureDoc::move_pages_to_end`, `duplicate_subtrees`, `trash_pages`). Their
//!   persists and events are those of `PageMove`, `PageDuplicate` and `PageTrash`, once.
//!
//! The answer is the top copies, in order, for a duplicate and the pages acted on otherwise.
//! Copying the pages themselves is the machinery of `PageDuplicate` (`duplicate.rs`).

use std::collections::HashSet;

use kb_types::{ApiError, BulkAction, MemberRole, PageId, PropertyValue, SpaceId};

use crate::Core;
use crate::documents::map_model_error;
use crate::duplicate::{PreparedCopies, copy_title};

/// Most pages one `PageBulk` request acts on, and most pages one bulk duplicate copies (ADR-0058).
pub(crate) const MAX_BULK_PAGES: usize = 1_000;

impl Core {
    /// `Request::PageBulk`: applies `action` to `page_ids` of `space_id` (section 5.4 of the design
    /// doc). Answers the top copies for a duplicate, else the pages acted on.
    pub(crate) fn page_bulk(
        &mut self,
        space_id: SpaceId,
        page_ids: &[PageId],
        action: BulkAction,
    ) -> Result<Vec<PageId>, ApiError> {
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        self.check_bulk_pages(space_id, page_ids)?;
        match action {
            BulkAction::SetValue { property_id, value } => {
                let entries: Vec<(PageId, Option<PropertyValue>)> =
                    page_ids.iter().map(|page| (*page, value.clone())).collect();
                self.pages_property_set(space_id, property_id, &entries)?;
                Ok(page_ids.to_vec())
            }
            BulkAction::SetValues {
                property_id,
                values,
            } => {
                if values.len() != page_ids.len() {
                    return Err(ApiError::invalid(
                        "values",
                        "there must be one value for each page",
                    ));
                }
                let entries: Vec<(PageId, Option<PropertyValue>)> =
                    page_ids.iter().copied().zip(values).collect();
                self.pages_property_set(space_id, property_id, &entries)?;
                Ok(page_ids.to_vec())
            }
            BulkAction::Move { parent_id } => self.bulk_move(space_id, page_ids, parent_id),
            BulkAction::Duplicate => self.bulk_duplicate(space_id, page_ids),
            BulkAction::Trash => self.bulk_trash(space_id, page_ids),
        }
    }

    /// The pages of a request: 1 to [`MAX_BULK_PAGES`], none twice, each a live page of the space
    /// (`InvalidInput`, `pageIds`).
    fn check_bulk_pages(&self, space_id: SpaceId, page_ids: &[PageId]) -> Result<(), ApiError> {
        if page_ids.is_empty() {
            return Err(ApiError::invalid("pageIds", "name at least one page"));
        }
        if page_ids.len() > MAX_BULK_PAGES {
            return Err(ApiError::invalid(
                "pageIds",
                format!("at most {MAX_BULK_PAGES} pages at a time"),
            ));
        }
        let model = &self.structure(space_id)?.model;
        let mut seen: HashSet<PageId> = HashSet::with_capacity(page_ids.len());
        for page in page_ids {
            if !seen.insert(*page) {
                return Err(ApiError::invalid("pageIds", "a page is named twice"));
            }
            if !model.is_live_page(*page) {
                return Err(ApiError::invalid(
                    "pageIds",
                    "a page is not a live page of this space",
                ));
            }
        }
        Ok(())
    }

    /// `Move`: the pages without those that go with an ancestor, to the end of `parent_id`'s
    /// children (the top level for `None`) in the order given, as one commit and one undo step.
    /// `InvalidInput` (`parentId`) when the parent is not a live page of the space, or is one of
    /// the pages or below one. A move into or out of a template is announced like `PageMove`.
    fn bulk_move(
        &mut self,
        space_id: SpaceId,
        page_ids: &[PageId],
        parent_id: Option<PageId>,
    ) -> Result<Vec<PageId>, ApiError> {
        let model = &self.structure(space_id)?.model;
        let tops = model.without_descendants(page_ids);
        let was_in_template: Vec<bool> = tops.iter().map(|page| model.in_template(*page)).collect();
        self.mutate_structure(space_id, |model, now| {
            model.move_pages_to_end(&tops, parent_id, now)
        })?;
        // A move into or out of a template changes what the subtree is in (ADR-0056): its index rows
        // were rewritten with the move; the backlinks and the semantic index follow.
        for (top, was) in tops.iter().zip(was_in_template) {
            let model = &self.structure(space_id)?.model;
            let now_in_template = model.in_template(*top);
            if now_in_template != was {
                let subtree = model.subtree(*top).map_err(map_model_error)?;
                self.template_pages_moved(&subtree, now_in_template)?;
            }
        }
        Ok(tops)
    }

    /// `Trash`: the pages without those that go with an ancestor, each with its subtree, as one
    /// commit and one undo step.
    fn bulk_trash(
        &mut self,
        space_id: SpaceId,
        page_ids: &[PageId],
    ) -> Result<Vec<PageId>, ApiError> {
        let tops = self
            .structure(space_id)?
            .model
            .without_descendants(page_ids);
        let affected =
            self.mutate_structure(space_id, |model, now| model.trash_pages(&tops, now))?;
        self.backlinks_changed_for_sources(&affected)?;
        Ok(tops)
    }

    /// `Duplicate`: each page without those that go with an ancestor, with its sub-pages, as
    /// `PageDuplicate` copies one, each copy right after its page; all the structure in one commit
    /// and one undo step. At most [`MAX_BULK_PAGES`] pages are copied in all (`InvalidInput`,
    /// `pageIds`). Answers the copy of each page, in the order given.
    fn bulk_duplicate(
        &mut self,
        space_id: SpaceId,
        page_ids: &[PageId],
    ) -> Result<Vec<PageId>, ApiError> {
        let model = &self.structure(space_id)?.model;
        let tops = model.without_descendants(page_ids);
        // The live subtrees, pre-order, and the title of each top copy.
        let mut sources: Vec<PageId> = Vec::new();
        let mut titled: Vec<(PageId, String)> = Vec::with_capacity(tops.len());
        for top in &tops {
            let title = model
                .page_title(*top)
                .ok_or_else(|| ApiError::not_found("page not found"))?;
            titled.push((*top, copy_title(&title)));
            sources.extend(model.subtree(*top).map_err(map_model_error)?);
            if sources.len() > MAX_BULK_PAGES {
                return Err(ApiError::invalid(
                    "pageIds",
                    format!("at most {MAX_BULK_PAGES} pages are duplicated at a time"),
                ));
            }
        }

        // Everything that can fail is done before anything is written: the new ids, the content of
        // every page read and rebuilt with them.
        let PreparedCopies { ids, models } = self.prepare_copies(&sources, None)?;

        let now = self.clock.now_ms();
        let duplicated = self
            .structure_mut(space_id)?
            .model
            .duplicate_subtrees(&titled, &ids, now)
            .map_err(map_model_error)?;
        self.finish_copies(space_id, &duplicated, models, now)?;
        tracing::info!(pages = duplicated.pages.len(), "pages duplicated");

        tops.iter()
            .map(|top| {
                ids.get(top)
                    .copied()
                    .ok_or_else(|| ApiError::internal("the page has no copy"))
            })
            .collect()
    }
}
