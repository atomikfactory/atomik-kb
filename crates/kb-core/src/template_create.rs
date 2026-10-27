//! Making a page from a template (ADR-0056, `docs/architecture/templates.md` section 5): a copy of
//! the template's subtree through the machinery of `PageDuplicate` (`duplicate.rs`), placed under
//! another parent, with the template flag cleared and the placeholders resolved.
//!
//! `PageCreateFromTemplate { templateId, parentId?, index?, values? }` checks first and writes
//! nothing until every check has passed: the template exists, is live and is a template
//! (`templateId`); the parent, when given, is a live page of the template's space that is not
//! inside the template (`parentId`); the subtree has at most [`MAX_DUPLICATED_PAGES`] live pages
//! (`templateId`); every value of `values` would be accepted by `PagePropertySet` (`values`). The
//! role is Editor.
//!
//! Then, as the duplicate does: new ids, the documents rebuilt (new block ids, links and cards
//! into the subtree remapped) with the relative mentions resolved to the local day, time and
//! account of the request ([`TemplateFill`]), one structure update with the pages (one structural
//! undo step), their values, the placeholders resolved into values and the request's values on
//! the top copy, the titles' tokens resolved, the documents written, and the same events.
//!
//! Time is the clock's `now_ms` and `local_offset_seconds` at the request; "me" is [`Core::me`]
//! (the account of the device; in a personal space, the owner) with the display name the space
//! gives it. Before the recovery kit exists there is no account: a `me` placeholder gives no
//! value, and a relative person mention keeps the account and label it was written with.

use kb_model::{LocalMoment, ModelError, TemplateCopy, TemplateFill, TemplateMe};
use kb_types::{ApiError, MemberRole, PageId, PageInfo, PropertyId, PropertyValue};

use crate::Core;
use crate::documents::map_model_error;
use crate::duplicate::{MAX_DUPLICATED_PAGES, PreparedCopies};

/// A model refusal of one of the request's values, as `PagePropertySet` would answer it but
/// naming `values`.
fn value_error(err: ModelError) -> ApiError {
    match err {
        ModelError::InvalidProperty { message, .. } => ApiError::invalid("values", message),
        ModelError::PropertyNotFound => ApiError::invalid("values", "the property does not exist"),
        other => map_model_error(other),
    }
}

impl Core {
    /// `PageCreateFromTemplate`: a new page (and sub-pages) from a template.
    pub(crate) fn page_create_from_template(
        &mut self,
        template_id: PageId,
        parent_id: Option<PageId>,
        index: Option<u32>,
        values: Vec<(PropertyId, PropertyValue)>,
    ) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(template_id)?;
        self.require_role(space_id, MemberRole::Editor)?;

        // The checks: nothing is written until they have all passed.
        let sources = {
            let open = self.structure(space_id)?;
            let info = open
                .model
                .page(template_id)
                .ok_or_else(|| ApiError::not_found("page not found"))?;
            if info.trashed_at.is_some() {
                return Err(ApiError::invalid(
                    "templateId",
                    "the template is in the trash",
                ));
            }
            if !info.template {
                return Err(ApiError::invalid(
                    "templateId",
                    "the page is not a template",
                ));
            }
            let sources = open.model.subtree(template_id).map_err(map_model_error)?;
            if let Some(parent) = parent_id {
                match open.model.page(parent) {
                    Some(parent_info) if parent_info.trashed_at.is_none() => {}
                    // Unknown, in the trash, or a page of another space (this document has none).
                    _ => {
                        return Err(ApiError::invalid(
                            "parentId",
                            "the parent page does not exist, is in the trash or is in another space",
                        ));
                    }
                }
                if sources.contains(&parent) {
                    return Err(ApiError::invalid(
                        "parentId",
                        "a page cannot be made inside the template it is copied from",
                    ));
                }
            }
            if sources.len() > MAX_DUPLICATED_PAGES {
                return Err(ApiError::invalid(
                    "templateId",
                    format!("a template is copied with at most {MAX_DUPLICATED_PAGES} pages"),
                ));
            }
            for (property, value) in &values {
                open.model
                    .check_page_property(*property, value)
                    .map_err(value_error)?;
            }
            sources
        };
        for (_, value) in &values {
            if let PropertyValue::Person(accounts) = value {
                // Members of the space (ADR-0049), as `PagePropertySet` checks them.
                self.person_value_check(space_id, accounts, &[])
                    .map_err(|err| ApiError::invalid("values", err.message))?;
            }
        }

        // The moment and the person the placeholders stand for.
        let now = self.clock.now_ms();
        let moment = LocalMoment::new(now, self.clock.local_offset_seconds());
        let me = self.me().ok();
        let me_label = me.and_then(|account| self.space_people_names(space_id).remove(&account));
        let template_me = me.map(|account| TemplateMe {
            account,
            label: me_label,
        });
        let fill = TemplateFill {
            moment: &moment,
            me: template_me.as_ref(),
        };

        // Everything that can fail is done before anything is written: the new ids, the content
        // of every page read and rebuilt with them.
        let PreparedCopies { ids, models } = self.prepare_copies(&sources, Some(&fill))?;

        // 1. The pages and their values, one persisted update.
        let copy = TemplateCopy {
            parent: parent_id,
            index: index.map(|at| usize::try_from(at).unwrap_or(usize::MAX)),
            moment: &moment,
            me,
            values: &values,
        };
        let created = self
            .structure_mut(space_id)?
            .model
            .create_from_template(template_id, &ids, &copy, now)
            .map_err(map_model_error)?;
        self.finish_copies(space_id, &created, models, now)?;
        tracing::info!(pages = created.pages.len(), "page made from a template");

        let top = *ids
            .get(&template_id)
            .ok_or_else(|| ApiError::internal("the template has no copy"))?;
        self.page_get(top)
    }
}
