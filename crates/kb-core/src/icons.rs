//! Page icons and covers that are uploaded images (ADR-0029, ADR-0040): the check of a request that
//! sets one, and the derived `icon_refs` rows.
//!
//! The icon and the cover of a page live in the page's meta map of the space's structure document,
//! as strings (`validate::icon` and `validate::cover` are their grammars). When one is
//! `attachment:<attachment id>` the page references that attachment like a block does, for as long
//! as the page exists (in the trash included): the retention of attachments (`unreferenced_since`,
//! the garbage collector) counts the rows of `icon_refs` next to the references of the blocks and of
//! the `files` values. A page holds one row per attachment, so the same image as icon and cover is
//! one reference.
//!
//! The rows are rewritten **whenever a structure document is persisted**
//! ([`Core::persist_structure_with`](crate::Core)): the pages that were affected (a local icon or
//! cover change, a purge, a trash or a move), or the whole space when the write may have touched any
//! page (undo and redo, a remote import). Nothing needs a hook of its own: a page that left the
//! document loses its rows, a page that came back by undo gets them again, and the attachments whose
//! reference count may have changed (the images before and after) go through the same bookkeeping as
//! a document change ([`Core::attachment_refs_changed`](crate::Core)). The rows are compared with
//! the documents at open and by `IndexRebuild` ([`Core::refresh_icon_refs`]), which also fills the
//! table for a database that was migrated to schema v14, and rewrites it for covers after v16.

use std::collections::BTreeSet;

use kb_model::StructureDoc;
use kb_storage::{AttachmentQueries, IconRefQueries, StorageError};
use kb_types::{ApiError, AttachmentId, PageId, SpaceId};

use crate::{Core, map_storage_error, short_id, validate};

/// The attachment a page icon references: `Some` for `attachment:<id>`, `None` for any other icon.
fn attachment_of(icon: &str) -> Option<AttachmentId> {
    validate::icon_attachment(icon)
}

/// The attachment a page cover references: `Some` for `attachment:<id>`, `None` for a colour or a
/// gradient.
fn cover_attachment_of(cover: &str) -> Option<AttachmentId> {
    validate::cover_attachment(cover)
}

/// The attachments a page references through its icon and its cover, each once, in id order.
fn page_attachments(icon: Option<&str>, cover: Option<&str>) -> Vec<AttachmentId> {
    let mut ids: Vec<AttachmentId> = icon
        .and_then(attachment_of)
        .into_iter()
        .chain(cover.and_then(cover_attachment_of))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// What a write of derived icon rows consists of, computed from the model before the transaction
/// opens.
pub(crate) enum IconWrites {
    /// These pages: the attachments of its image icon and image cover become its rows, none removes
    /// them (a page without an image, or one the document no longer has).
    Pages(Vec<(PageId, Vec<AttachmentId>)>),
    /// Every row of the space, rebuilt from its pages.
    Space(Vec<(PageId, AttachmentId)>),
}

impl IconWrites {
    /// The rows to write for a structure write that affected `affected` (`None`: any page).
    pub(crate) fn of(model: &StructureDoc, affected: Option<&[PageId]>) -> Self {
        match affected {
            Some(pages) => Self::Pages(
                pages
                    .iter()
                    .map(|page| {
                        let icon = model.page_icon(*page);
                        let cover = model.page_cover(*page);
                        (*page, page_attachments(icon.as_deref(), cover.as_deref()))
                    })
                    .collect(),
            ),
            None => Self::Space(space_refs(model)),
        }
    }

    /// The attachments the rows of this write reference.
    fn attachments(&self) -> BTreeSet<AttachmentId> {
        match self {
            Self::Pages(pages) => pages
                .iter()
                .flat_map(|(_, attachments)| attachments.iter().copied())
                .collect(),
            Self::Space(icons) => icons.iter().map(|(_, attachment)| *attachment).collect(),
        }
    }

    /// The attachments whose reference count this write may change: the ones its rows reference and
    /// the ones the stored rows it replaces or removes reference. Read before the write.
    pub(crate) fn touched<Q: IconRefQueries>(
        &self,
        q: &Q,
        space_id: SpaceId,
    ) -> Result<BTreeSet<AttachmentId>, StorageError> {
        let mut touched = self.attachments();
        match self {
            Self::Pages(pages) => {
                let ids: Vec<PageId> = pages.iter().map(|(page, _)| *page).collect();
                touched.extend(q.icon_refs_attachments_of(&ids)?);
            }
            Self::Space(_) => {
                touched.extend(
                    q.icon_refs_of_space(space_id)?
                        .into_iter()
                        .map(|(_, attachment)| attachment),
                );
            }
        }
        Ok(touched)
    }

    /// Writes the rows through a transaction or connection.
    pub(crate) fn write<Q: IconRefQueries>(
        &self,
        q: &Q,
        space_id: SpaceId,
    ) -> Result<(), StorageError> {
        match self {
            Self::Pages(pages) => {
                if pages.is_empty() {
                    return Ok(());
                }
                q.icon_refs_set(space_id, pages)
            }
            Self::Space(icons) => q.icon_refs_replace_space(space_id, icons),
        }
    }
}

/// The `(page, attachment)` pairs of every page of the space whose icon or cover is an uploaded
/// image, in page id order and then attachment id order, each pair once.
fn space_refs(model: &StructureDoc) -> Vec<(PageId, AttachmentId)> {
    let mut refs: Vec<(PageId, AttachmentId)> = model
        .page_icons()
        .into_iter()
        .filter_map(|(page, icon)| Some((page, attachment_of(&icon)?)))
        .chain(
            model
                .page_covers()
                .into_iter()
                .filter_map(|(page, cover)| Some((page, cover_attachment_of(&cover)?))),
        )
        .collect();
    refs.sort_unstable();
    refs.dedup();
    refs
}

impl Core {
    /// Whether `icon` (already through [`validate::icon`]) may be the icon of a page of `space_id`:
    /// an `attachment:<id>` must name an image that is among the space's manifests (so not a purged
    /// one, and not one of another space). Any other icon is fine here.
    pub(crate) fn check_page_icon_attachment(
        &self,
        space_id: SpaceId,
        icon: Option<&str>,
    ) -> Result<(), ApiError> {
        match icon.and_then(attachment_of) {
            Some(attachment_id) => self.check_page_image(space_id, "icon", attachment_id),
            None => Ok(()),
        }
    }

    /// Whether `cover` (already through [`validate::cover`]) may be the cover of a page of
    /// `space_id`: the same rule as for an icon (ADR-0040). A colour or a gradient is fine here.
    pub(crate) fn check_page_cover_attachment(
        &self,
        space_id: SpaceId,
        cover: Option<&str>,
    ) -> Result<(), ApiError> {
        match cover.and_then(cover_attachment_of) {
            Some(attachment_id) => self.check_page_image(space_id, "cover", attachment_id),
            None => Ok(()),
        }
    }

    /// The attachment must be an image among the space's manifests; the refusal is on `field`.
    fn check_page_image(
        &self,
        space_id: SpaceId,
        field: &str,
        attachment_id: AttachmentId,
    ) -> Result<(), ApiError> {
        let Some(manifest) = self.structure(space_id)?.model.attachment(attachment_id) else {
            // Said apart so that a shell (and a person) can tell a wrong space from a missing file.
            let elsewhere = self
                .db
                .attachment_get(attachment_id)
                .map_err(map_storage_error)?
                .is_some_and(|row| row.space_id != space_id);
            return Err(ApiError::invalid(
                field,
                if elsewhere {
                    "the image belongs to another space"
                } else {
                    "the image is not an attachment of this space (unknown or deleted)"
                },
            ));
        };
        if !manifest.mime.to_ascii_lowercase().starts_with("image/") {
            return Err(ApiError::invalid(
                field,
                "the attachment is not an image (its type must start with image/)",
            ));
        }
        Ok(())
    }

    /// Makes the `icon_refs` rows of a space agree with the icons and covers in its structure document, in a
    /// transaction of their own (at open, by `IndexRebuild`, after a repair), and returns the
    /// attachments whose references changed (empty when nothing differed) for the retention
    /// bookkeeping ([`Core::attachment_refs_changed`]). Also what fills the table for a database that
    /// was migrated to schema v14, which has none, and a cover that synced from a newer device.
    pub(crate) fn refresh_icon_refs(
        &mut self,
        space_id: SpaceId,
    ) -> Result<BTreeSet<AttachmentId>, ApiError> {
        let wanted = space_refs(&self.structure(space_id)?.model);
        let stored = self
            .db
            .icon_refs_of_space(space_id)
            .map_err(map_storage_error)?;
        if wanted == stored {
            return Ok(BTreeSet::new());
        }
        tracing::warn!(space = %short_id(&space_id), stored = stored.len(), wanted = wanted.len(), "icon and cover references out of sync; rewriting");
        let touched: BTreeSet<AttachmentId> = wanted
            .iter()
            .chain(&stored)
            .map(|(_, attachment)| *attachment)
            .collect();
        self.db
            .icon_refs_replace_space(space_id, &wanted)
            .map_err(map_storage_error)?;
        Ok(touched)
    }
}
