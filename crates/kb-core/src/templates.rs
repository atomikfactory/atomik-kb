//! Templates (ADR-0056, `docs/architecture/templates.md`): the template flag of a page and the
//! placeholders of its properties. Making a page from a template is `template_create.rs`.
//!
//! A template is a page whose metadata has the flag; its sub-pages are part of it. The model keeps
//! the flag and the placeholders (`kb-model/src/templates.rs`); this module is the requests' side:
//! roles, the page index rows of the subtree (`pages_index.in_template`, which keeps templates out
//! of search, backlinks, reminders, the semantic index and MCP's lists, schema v20), the semantic
//! index rows a page leaves behind when it becomes a template, and the events.

use kb_storage::UpdateOrigin;
use kb_types::{ApiError, Event, MemberRole, PageId, PageInfo, PropertyId, TemplatePlaceholder};

use crate::Core;
use crate::documents::map_model_error;
use crate::properties::PropsScope;

impl Core {
    /// `PageSetTemplate`: makes a page a template or a page again (Editor; a page in the trash is
    /// `InvalidInput`). The page and its whole live subtree get their index rows rewritten (the
    /// flag changes what the subtree is in), and the backlinks the subtree gave (or gives back) are
    /// announced. A page that becomes a template loses what the semantic index held of it and of
    /// its sub-pages.
    pub(crate) fn page_set_template(
        &mut self,
        page_id: PageId,
        template: bool,
    ) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let affected = self.mutate_structure(space_id, |model, now| {
            model.set_page_template(page_id, template, now)
        })?;
        if !affected.is_empty() {
            self.template_flag_changed(&affected, template)?;
        }
        self.page_get(page_id)
    }

    /// `PageSetTemplatePlaceholder`: sets or clears the placeholder of a property on a template
    /// page, or on a page inside one (Editor). Setting one removes the page's stored value of the
    /// property, so the property rows the derived tables keep for the page are rewritten with the
    /// index row.
    pub(crate) fn page_set_template_placeholder(
        &mut self,
        page_id: PageId,
        property_id: PropertyId,
        placeholder: Option<TemplatePlaceholder>,
    ) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let now = self.clock.now_ms();
        let affected = self
            .structure_mut(space_id)?
            .model
            .set_template_placeholder(page_id, property_id, placeholder, now)
            .map_err(map_model_error)?;
        self.persist_structure_with(
            space_id,
            Some(&affected),
            UpdateOrigin::Local,
            PropsScope::Pages(&affected),
        )?;
        self.events.push(Event::StructureChanged { space_id });
        self.page_get(page_id)
    }

    /// What follows a change of the template flag of the pages `affected` (the page and its
    /// subtree): the backlinks their links gave to other pages come and go, and a template has no
    /// semantic index rows.
    fn template_flag_changed(
        &mut self,
        affected: &[PageId],
        template: bool,
    ) -> Result<(), ApiError> {
        self.backlinks_changed_for_sources(affected)?;
        if template {
            self.ai_forget_templates()?;
        }
        Ok(())
    }

    /// After a page moved into or out of a template inside its space (`affected` is the moved
    /// subtree, `in_template` where the moved page is now): the backlinks their links gave come or
    /// go, and a page that moved into a template has no semantic index rows (a move out of one is
    /// indexed again by the walk of the index).
    pub(crate) fn template_pages_moved(
        &mut self,
        affected: &[PageId],
        in_template: bool,
    ) -> Result<(), ApiError> {
        self.backlinks_changed_for_sources(affected)?;
        if in_template {
            self.ai_forget_templates()?;
        }
        Ok(())
    }
}
