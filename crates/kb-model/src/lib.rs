//! Typed CRDT documents (`docs/development/milestone-02.md`, "Documents").
//!
//! - [`AccountDoc`]: one per workspace, the ordered list of spaces (and, since Milestone 13, the
//!   shared spaces, the profile, the per-space AI switches, the per-space favourites and the
//!   verified people: the `sharing` module).
//! - [`StructureDoc`]: one per space, the page tree with metadata, trash and tombstones,
//!   and the space's attachment manifests (Milestone 5, ADR-0020).
//! - [`PageDoc`]: one per page, the block tree with rich text (Milestone 3, ADR-0016).
//!
//! All wrap a [`kb_doc::Doc`]; persistence and sync only ever see the `Doc`. This
//! crate and `kb-doc` are the only crates that import `loro`.
//!
//! Every mutating method commits its Loro transaction before returning, so the
//! owner can export the change right away. Document shapes (container names,
//! metadata keys, value types) are frozen by the golden fixtures in `fixtures/doc/`.

#![forbid(unsafe_code)]

mod attachment;
mod bulk;
mod comments;
mod duplicate;
mod favorites;
pub mod formula;
mod index;
mod links;
mod meta;
mod moves;
mod page;
mod portable;
mod properties;
mod reorder;
mod sharing;
mod templates;

pub use attachment::{AttachmentManifest, MANIFEST_VERSION};
pub use comments::{
    ANCHOR_MAX_BYTES, CommentOp, CommentRecord, CommentRecordError, CommentStream, FoldedComment,
    FoldedThread, RECORD_VERSION, SIGNATURE_LEN, VerifiedRecord, snapshot_dominates,
    snapshot_record_ids, validate_anchor, validate_body,
};
pub use duplicate::{Duplicated, TemplateCopy};
pub use favorites::FAVORITES_ORIGIN;
pub use links::{LINK_ENTRY_VERSION, LINKS_MAP, LINKS_ORIGIN, ViewLinkRecord};
pub use moves::{MOVE_ORIGIN, MovedPage};
pub use page::{
    ANNOTATION_CLOSE, ANNOTATION_OPEN, BlockKind, BlockText, CopyIds, DateMention, ExportLinks,
    Inline, LocalMoment, PageDoc, PageLink, PersonMention, TemplateFill, TemplateMe,
    WhiteboardElement, fill_template_content, portable_block_ids, remap_portable,
    render_markdown_file, schema, shape, views,
};
pub use properties::{
    PROPERTIES_ORIGIN, PageIndexEntry, PropertyRow, normalise_value, person_filter_matches,
    person_text, valid_date, valid_url,
};
pub use sharing::{
    FAVORITES_MAP, PROFILE_MAP, PROFILE_NAME_KEY, SHARED_SPACES_MAP, SHARING_ORIGIN, SPACE_AI_KEY,
    SPACE_SETTINGS_MAP, VERIFIED_PEOPLE_MAP,
};
pub use templates::{ImportedTemplate, placeholder_fits};

use std::collections::{HashMap, HashSet};

use kb_doc::{Doc, DocKind};
use kb_types::{
    AttachmentId, DeviceId, PageFont, PageId, PageInfo, PageNode, SpaceId, TrashedPage,
};
use loro::{
    CommitOptions, LoroError, LoroMap, LoroMovableList, LoroTree, LoroTreeError, LoroValue, TreeID,
    TreeParentId, UndoManager,
};

use crate::index::{PageIndex, node_label, tombstoned};
use crate::meta::engine;

/// Errors from document operations. Mapped to `ApiError` by the core.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// No such page in this document.
    #[error("page not found")]
    PageNotFound,
    /// The page exists but is in the trash (or under a trashed page).
    #[error("page is in the trash")]
    PageTrashed,
    /// The page is live, so it cannot be restored.
    #[error("page is not in the trash")]
    PageNotTrashed,
    /// The target parent does not exist or is trashed.
    #[error("parent page not found or trashed")]
    ParentUnavailable,
    /// Moving a page under itself or one of its descendants.
    #[error("cannot move a page under its own descendant")]
    CyclicMove,
    /// A page with this id already exists in the document.
    #[error("page id already exists")]
    DuplicatePage,
    /// A reorder of the children of a page that does not name exactly its live children, each
    /// once (ADR-0052).
    #[error("the order is not the current children of the parent")]
    InvalidOrder,
    /// No such space in the account document.
    #[error("space not found")]
    SpaceNotFound,
    /// No block with this id in the page document (or none where the operation
    /// looks for it).
    #[error("block not found")]
    BlockNotFound,
    /// A block with this id already exists in the page document.
    #[error("block id already exists")]
    DuplicateBlock,
    /// A page operation that the block cannot take (fixed message, no content).
    #[error("invalid block operation: {0}")]
    InvalidBlock(&'static str),
    /// No such property in the space's schema.
    #[error("property not found")]
    PropertyNotFound,
    /// No such option in the property.
    #[error("option not found")]
    OptionNotFound,
    /// A property operation refused: the field at fault and a fixed message (no content).
    #[error("invalid property operation: {message}")]
    InvalidProperty {
        /// The API field at fault (`name`, `value`, `group`, `propertyId`, ...).
        field: &'static str,
        /// What is wrong, without any user content.
        message: String,
    },
    /// A favourite operation refused: the API field at fault and a fixed message (no content).
    #[error("invalid favourite operation: {message}")]
    InvalidFavorite {
        /// The API field at fault (`pageId` or `before`).
        field: &'static str,
        /// What is wrong, without any user content.
        message: &'static str,
    },
    /// The document content violates the expected shape (foreign or corrupt data).
    #[error("document shape violated: {0}")]
    Shape(String),
    /// Engine boundary failure.
    #[error(transparent)]
    Doc(#[from] kb_doc::DocError),
}

/// Space metadata stored in the structure document's `space` map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMeta {
    /// Space id.
    pub id: SpaceId,
    /// Display name.
    pub name: String,
    /// Optional icon.
    pub icon: Option<String>,
    /// Creation time, Unix ms.
    pub created_at: i64,
}

/// Version of the portable JSON produced by `to_portable`.
pub const PORTABLE_FORMAT: u32 = 1;

/// Undo stack depth of a structure document.
pub const UNDO_MAX_STEPS: usize = 100;

/// Consecutive edits of the same kind on the same target (title, icon or position
/// of one page) within this window of the injected clock form one undo step.
pub const UNDO_MERGE_WINDOW_MS: i64 = 300;

/// The largest vertical position of a page cover, in percent (ADR-0040).
pub const COVER_Y_MAX: u8 = 100;
/// The default vertical position of a page cover, in percent: stored as no value.
pub const COVER_Y_DEFAULT: u8 = 50;

/// The least time between two stamps of a page's last edit by [`StructureDoc::touch_edited`]
/// (a content edit moves `updated_at` at most once a minute per page and device).
pub const EDIT_STAMP_INTERVAL_MS: i64 = 60_000;

/// Commit origin of the bulk creation of imported pages ([`StructureDoc::create_pages`]). The
/// structure document's undo manager excludes this origin prefix, so an import never lands on the
/// structural undo stack page by page (and never pushes the person's earlier steps out of it): the
/// person undoes an import by trashing its container (ADR-0036).
pub const IMPORT_ORIGIN: &str = "kb-model/import";

/// A page for [`StructureDoc::create_pages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPage {
    /// The new page's id.
    pub page_id: PageId,
    /// Its parent: a live page of the document, a page created earlier in the same batch, or
    /// `None` for the root.
    pub parent: Option<PageId>,
    /// Its title.
    pub title: String,
    /// Its icon, when it has one.
    pub icon: Option<String>,
    /// Its cover, when it has one (ADR-0040; the caller validated it).
    pub cover: Option<String>,
    /// The cover's vertical position in percent (`None` and 50: the default); ignored without a cover.
    pub cover_y: Option<u8>,
}

/// Commit origin of attachment manifest writes. The structure document's undo manager
/// excludes this origin prefix, so structural undo never touches manifests (ADR-0020).
pub const ATTACHMENTS_ORIGIN: &str = "kb-model/attachments";

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

// ----- account document -----------------------------------------------------------------

/// Per-workspace document: the display order of spaces.
///
/// Container `spaces`: a movable list of space ids as UUID strings. Concurrent
/// additions of the same space may leave duplicates in the list; readers keep the
/// first occurrence and writers clean up the entries they touch.
pub struct AccountDoc {
    doc: Doc,
    spaces: LoroMovableList,
    /// `devices`: device id string -> display name (survives root key rotation,
    /// unlike the encrypted names in the device log).
    devices: LoroMap,
}

impl AccountDoc {
    /// A new, empty account document.
    #[must_use]
    pub fn new(doc: Doc) -> Self {
        let spaces = doc.inner().get_movable_list(meta::SPACES);
        let devices = doc.inner().get_map(meta::DEVICES);
        Self {
            doc,
            spaces,
            devices,
        }
    }

    /// Sets (or renames) a device's display name.
    pub fn set_device_name(
        &self,
        device_id: kb_types::DeviceId,
        name: &str,
    ) -> Result<(), ModelError> {
        self.devices
            .insert(&device_id.to_string(), name)
            .map_err(engine)?;
        self.doc.inner().commit();
        Ok(())
    }

    /// Every known device name, ordered by device id.
    #[must_use]
    pub fn device_names(&self) -> Vec<(kb_types::DeviceId, String)> {
        let mut names: Vec<(kb_types::DeviceId, String)> = match self.devices.get_value() {
            LoroValue::Map(map) => map
                .iter()
                .filter_map(|(key, value)| match value {
                    LoroValue::String(name) => Some((key.parse().ok()?, name.to_string())),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        names.sort();
        names
    }

    /// Wraps a loaded document, validating its shape.
    pub fn from_doc(doc: Doc) -> Result<Self, ModelError> {
        if doc.kind() != DocKind::Account {
            return Err(ModelError::Shape(format!(
                "expected an account document, got {:?}",
                doc.kind()
            )));
        }
        Ok(Self::new(doc))
    }

    /// Spaces in display order (duplicates removed, first occurrence wins).
    #[must_use]
    pub fn spaces(&self) -> Vec<SpaceId> {
        let mut seen = HashSet::new();
        self.entries()
            .into_iter()
            .flatten()
            .filter(|id| seen.insert(*id))
            .collect()
    }

    /// Appends a space (no-op if already present).
    pub fn add_space(&self, space_id: SpaceId) -> Result<(), ModelError> {
        if self.entries().contains(&Some(space_id)) {
            return Ok(());
        }
        self.spaces.push(space_id.to_string()).map_err(engine)?;
        self.doc.inner().commit();
        Ok(())
    }

    /// Removes a space from the order (every occurrence; no-op if absent).
    pub fn remove_space(&self, space_id: SpaceId) -> Result<(), ModelError> {
        let positions = self.positions_of(space_id);
        for pos in positions.into_iter().rev() {
            self.spaces.delete(pos, 1).map_err(engine)?;
        }
        self.doc.inner().commit();
        Ok(())
    }

    /// Moves a space to `index` (clamped).
    ///
    /// `index` is the position in [`Self::spaces`] after the move. Extra copies of
    /// the moved id are removed on the way.
    pub fn move_space(&self, space_id: SpaceId, index: usize) -> Result<(), ModelError> {
        let positions = self.positions_of(space_id);
        let Some((&from, duplicates)) = positions.split_first() else {
            return Err(ModelError::SpaceNotFound);
        };
        for pos in duplicates.iter().rev() {
            self.spaces.delete(*pos, 1).map_err(engine)?;
        }

        // First raw position of every distinct space, in display order.
        let entries = self.entries();
        let mut seen = HashSet::new();
        let order: Vec<(SpaceId, usize)> = entries
            .iter()
            .enumerate()
            .filter_map(|(pos, id)| id.map(|id| (id, pos)))
            .filter(|(id, _)| seen.insert(*id))
            .collect();
        let current = order.iter().position(|(id, _)| *id == space_id);
        let others: Vec<usize> = order
            .iter()
            .filter(|(id, _)| *id != space_id)
            .map(|(_, pos)| *pos)
            .collect();
        let target = index.min(others.len());
        if current != Some(target) {
            // Final raw index: just before the space now at `target`, or the end.
            let to = match others.get(target) {
                Some(&anchor) if from < anchor => anchor - 1,
                Some(&anchor) => anchor,
                None => entries.len().saturating_sub(1),
            };
            if to != from {
                self.spaces.mov(from, to).map_err(engine)?;
            }
        }
        self.doc.inner().commit();
        Ok(())
    }

    /// The underlying document, for persistence.
    #[must_use]
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// Must be called after the owner imported remote data into [`Self::doc`].
    pub fn refresh(&mut self) -> Result<(), ModelError> {
        // Nothing is cached: every read goes to the document.
        Ok(())
    }

    /// Engine-independent JSON (`{"format": 1, "spaces": ["<id>", ...]}`, plus
    /// `"devices": {"<device id>": "<name>"}` when any device name is known).
    #[must_use]
    pub fn to_portable(&self) -> serde_json::Value {
        let spaces: Vec<serde_json::Value> = self
            .spaces()
            .iter()
            .map(|id| serde_json::Value::String(id.to_string()))
            .collect();
        let mut out = serde_json::Map::new();
        let devices = self.device_names();
        if !devices.is_empty() {
            let mut map = serde_json::Map::new();
            for (id, name) in devices {
                map.insert(id.to_string(), name.into());
            }
            out.insert("devices".into(), map.into());
        }
        out.insert("format".into(), PORTABLE_FORMAT.into());
        out.insert("spaces".into(), spaces.into());
        out.into()
    }

    /// Raw list entries (`None` for anything that is not a space id string).
    fn entries(&self) -> Vec<Option<SpaceId>> {
        match self.spaces.get_value() {
            LoroValue::List(items) => items
                .iter()
                .map(|item| match item {
                    LoroValue::String(s) => s.parse().ok(),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn positions_of(&self, space_id: SpaceId) -> Vec<usize> {
        self.entries()
            .iter()
            .enumerate()
            .filter(|(_, id)| **id == Some(space_id))
            .map(|(pos, _)| pos)
            .collect()
    }
}

// ----- structure document ---------------------------------------------------------------

/// Which edits merge into one undo step (see [`UNDO_MERGE_WINDOW_MS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKey {
    Title(PageId),
    Icon(PageId),
    Cover(PageId),
    Appearance(PageId),
    Template(PageId),
    Move(PageId),
}

/// An undo group kept open while same-kind edits keep coming.
#[derive(Debug, Clone, Copy)]
struct OpenGroup {
    key: EditKey,
    started_at: i64,
}

/// Where a visible page sits.
#[derive(Debug, Clone, Copy)]
struct Placement {
    /// Parent page; `None` for live roots and top-level trashed pages.
    parent: Option<PageId>,
    /// The top-level trashed ancestor-or-self, when the page is in the trash.
    trashed_top: Option<TreeID>,
}

/// Per-space document: page tree, metadata, trash, tombstones.
///
/// Containers: `space` map (`id`, `name`, `icon`, `created_at`); `pages` tree whose
/// node metadata holds `id`, `title`, `icon`, `cover`, `coverY`, `font`, `smallText`, `locked`,
/// `template` and `template_values` (ADR-0056, see `templates.rs`), `created_at`, `updated_at`, `trashed_at`, `trashed_from` (pages) or `kind = "trash"` (trash roots);
/// `tombstones` map (`page id -> purged_at`); `attachments` map (`attachment id ->
/// manifest`, one plain map value per attachment, see [`AttachmentManifest`]).
/// Root containers are identified by their name, so two devices that write the same
/// root (the first manifest of a space, say) write into one map.
///
/// Undo: one Loro `UndoManager`, local changes only, [`UNDO_MAX_STEPS`] deep. Loro's
/// own merge interval is wall-clock based and merges edits of any kind, so it is
/// off; edits are grouped here instead, per kind of edit and target page and with
/// the `now_ms` the caller passes (see [`UNDO_MERGE_WINDOW_MS`]). Space name and icon
/// edits are not recorded: structural undo covers the page tree. Manifest writes are
/// committed with the origin [`ATTACHMENTS_ORIGIN`], which the undo manager excludes:
/// undo and redo never add, change or remove a manifest. Like the untracked space
/// edits, a manifest written between two edits that would merge into one undo step
/// leaves them as two steps (Loro groups contiguous local history only).
///
/// `favorites` map (`page id -> fractional position`, ADR-0029, see `favorites.rs`): the
/// space's favourite pages. Written with [`FAVORITES_ORIGIN`], which the undo manager excludes
/// (undo and redo never touch favourites); purging a page deletes its entry.
///
/// `links` map (`link id -> {page, created_by, created_at, expires_at}`, ADR-0051, see `links.rs`): the
/// space's view links. Written with [`LINKS_ORIGIN`], which the undo manager excludes (undo and redo
/// never create or revoke a link).
///
/// Imported pages ([`Self::create_pages`], ADR-0036) are committed with [`IMPORT_ORIGIN`], which the
/// undo manager excludes too: undo and redo never create or remove them.
pub struct StructureDoc {
    doc: Doc,
    space_id: SpaceId,
    space: LoroMap,
    tree: LoroTree,
    tombstones: LoroMap,
    attachments: LoroMap,
    /// The space's property schema (ADR-0026, `properties.rs`).
    properties: LoroMap,
    /// The space's favourite pages (ADR-0029, `favorites.rs`).
    favorites: LoroMap,
    /// The space's view links (ADR-0051, `links.rs`).
    links: LoroMap,
    undo: UndoManager,
    index: PageIndex,
    group: Option<OpenGroup>,
    /// The device that makes the local edits (ADR-0026, amendment of 2026-10-08): recorded as
    /// `created_by` on a new page and as `edited_by` with every `updated_at` this replica writes.
    author: Option<DeviceId>,
    /// The time the formulas of this space read as "now" and the device's offset (ADR-0055). Replica
    /// state like `author`: the owner sets it before reads ([`Self::set_formula_env`]); every read of
    /// the schema copies it, so one read of the tree or of a page sees one time.
    formula_time: properties::FormulaTime,
    /// The members' names for `name(person)` in formulas, told by the owner like the time
    /// ([`Self::set_formula_people`]).
    formula_people: std::sync::Mutex<properties::FormulaPeople>,
}

impl StructureDoc {
    /// Creates the document for a new space: writes the `space` map and the trash root.
    #[must_use]
    pub fn new(doc: Doc, meta: SpaceMeta) -> Self {
        if let Err(err) = write_new_space(&doc, &meta) {
            // Cannot happen on a fresh document; never panic.
            tracing::warn!(error = %err, "could not initialise the structure document");
        }
        doc.inner().commit();
        // The undo manager is created after the setup, so it can never be undone.
        Self::assemble(doc, meta.id)
    }

    /// Wraps a loaded document, validating its shape and building the id index.
    pub fn from_doc(doc: Doc) -> Result<Self, ModelError> {
        if doc.kind() != DocKind::Structure {
            return Err(ModelError::Shape(format!(
                "expected a structure document, got {:?}",
                doc.kind()
            )));
        }
        let space = doc.inner().get_map(meta::SPACE);
        let space_id = meta::string(&space, meta::ID)
            .and_then(|s| s.parse::<SpaceId>().ok())
            .ok_or_else(|| ModelError::Shape("space id missing or invalid".into()))?;
        Ok(Self::assemble(doc, space_id))
    }

    /// Sets the device the local edits are attributed to (`None`: no attribution is written, as
    /// before). The owner sets it once; it is replica state, not document state.
    pub fn set_author(&mut self, author: Option<DeviceId>) {
        self.author = author;
    }

    /// Writes `updated_at` (and, when an author is set, `edited_by`) of a page node.
    pub(crate) fn stamp_edit(&self, meta_map: &LoroMap, now_ms: i64) -> Result<(), ModelError> {
        meta::set_int(meta_map, meta::UPDATED_AT, now_ms)?;
        if let Some(author) = self.author {
            meta::set_string(meta_map, meta::EDITED_BY, &author.to_string())?;
        }
        Ok(())
    }

    /// Records that the page's **content document** was edited here: sets `updated_at` to `now_ms`
    /// and `edited_by` to the author, **throttled** to one write per [`EDIT_STAMP_INTERVAL_MS`]
    /// (an edit within the interval of the last stamp by the same device writes nothing; an edit
    /// by another device, or the first by this one, is stamped at once). Never moves
    /// `updated_at` backwards. Written with the origin [`PROPERTIES_ORIGIN`], so structural undo
    /// never undoes it. Returns the page when something was written.
    pub fn touch_edited(
        &mut self,
        page_id: PageId,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let meta_map = self.tree.get_meta(node).map_err(engine)?;
        let last = meta::int(&meta_map, meta::UPDATED_AT)
            .or_else(|| meta::int(&meta_map, meta::CREATED_AT))
            .unwrap_or(0);
        if now_ms <= last {
            return Ok(Vec::new());
        }
        let same_author = self.author.is_none_or(|author| {
            meta::string(&meta_map, meta::EDITED_BY).as_deref() == Some(author.to_string().as_str())
        });
        if same_author && now_ms - last < EDIT_STAMP_INTERVAL_MS {
            return Ok(Vec::new());
        }
        self.close_group();
        self.props_write(|| self.stamp_edit(&meta_map, now_ms))?;
        Ok(vec![page_id])
    }

    /// Space metadata.
    #[must_use]
    pub fn space(&self) -> SpaceMeta {
        SpaceMeta {
            id: self.space_id,
            name: meta::string(&self.space, meta::NAME).unwrap_or_default(),
            icon: meta::string(&self.space, meta::ICON),
            created_at: meta::int(&self.space, meta::CREATED_AT).unwrap_or(0),
        }
    }

    /// Renames the space.
    ///
    /// Not recorded on the undo stack (structural undo covers the page tree; the
    /// owner reports space changes separately).
    pub fn set_name(&self, name: &str) -> Result<(), ModelError> {
        self.untracked(|| meta::set_string(&self.space, meta::NAME, name))
    }

    /// Sets or clears the space icon. Not recorded on the undo stack.
    pub fn set_icon(&self, icon: Option<&str>) -> Result<(), ModelError> {
        self.untracked(|| meta::set_opt_string(&self.space, meta::ICON, icon))
    }

    /// Number of live pages (not in the trash).
    #[must_use]
    pub fn live_page_count(&self) -> u32 {
        count(
            self.visible_children(TreeParentId::Root)
                .iter()
                .map(|(node, _)| self.preorder(*node).len())
                .sum(),
        )
    }

    /// Number of top-level trashed pages.
    #[must_use]
    pub fn trashed_page_count(&self) -> u32 {
        count(self.top_trashed().len())
    }

    /// Live roots with their subtrees, in sibling order.
    #[must_use]
    pub fn live_tree(&self) -> Vec<PageNode> {
        let schema = self.read_schema();
        self.forest(
            self.visible_children(TreeParentId::Root),
            &mut |node, id, parent| PageNode {
                page: self.info(id, node, parent, None, &schema),
                children: Vec::new(),
            },
            &|parent, child| parent.children.push(child),
        )
    }

    /// Top-level trashed pages, most recently trashed first.
    ///
    /// Ties (same `trashed_at`) are ordered by page id so every replica agrees.
    #[must_use]
    pub fn trashed_pages(&self) -> Vec<TrashedPage> {
        let schema = self.read_schema();
        self.trash_entries()
            .into_iter()
            .map(|(node, id, trashed_at)| {
                let original_parent_id = self
                    .meta_of(node)
                    .and_then(|m| meta::page_id(&m, meta::TRASHED_FROM))
                    .filter(|parent| self.index.node(*parent).is_some());
                TrashedPage {
                    page: self.info(id, node, None, Some(trashed_at), &schema),
                    original_parent_id,
                    descendant_count: count(self.preorder(node).len().saturating_sub(1)),
                }
            })
            .collect()
    }

    /// Metadata of one page (live or trashed); `None` if unknown or purged.
    ///
    /// `parent_id` is `None` for roots and for a top-level trashed page; pages inside
    /// a trashed subtree report their real parent. `trashed_at` is the top-level
    /// trashed ancestor's time for every page of a trashed subtree.
    #[must_use]
    pub fn page(&self, page_id: PageId) -> Option<PageInfo> {
        let node = self.index.node(page_id)?;
        let placement = self.placement(node)?;
        let trashed_at = placement.trashed_top.map(|top| self.trashed_at_of(top));
        Some(self.info(
            page_id,
            node,
            placement.parent,
            trashed_at,
            &self.read_schema(),
        ))
    }

    /// Ids of every page in the subtree rooted at `page_id`, including itself.
    ///
    /// Pre-order: the page first, then each child's subtree in sibling order.
    pub fn subtree(&self, page_id: PageId) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        Ok(self.preorder(node))
    }

    /// Creates a page under `parent` (or the root) at `index` (`None` = last).
    /// Returns the ids whose index rows changed (the new page).
    ///
    /// `index` is the position among the live siblings, clamped. The parent must be
    /// live. Ids of existing, trashed or purged pages are refused.
    pub fn create_page(
        &mut self,
        page_id: PageId,
        parent: Option<PageId>,
        index: Option<usize>,
        title: &str,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        if self.index.node(page_id).is_some() || self.is_tombstoned(page_id) {
            return Err(ModelError::DuplicatePage);
        }
        let parent = match parent {
            None => TreeParentId::Root,
            Some(parent) => TreeParentId::Node(
                self.live_node(parent)
                    .ok_or(ModelError::ParentUnavailable)?,
            ),
        };
        let position = self.engine_position(parent, None, index);

        self.begin_edit(None, now_ms);
        let node = self.tree.create_at(parent, position).map_err(engine)?;
        let meta = self.tree.get_meta(node).map_err(engine)?;
        meta::set_string(&meta, meta::ID, &page_id.to_string())?;
        meta::set_string(&meta, meta::TITLE, title)?;
        meta::set_int(&meta, meta::CREATED_AT, now_ms)?;
        self.stamp_edit(&meta, now_ms)?;
        if let Some(author) = self.author {
            meta::set_string(&meta, meta::CREATED_BY, &author.to_string())?;
        }
        self.commit();
        self.index.insert(page_id, node);
        self.settle_index();
        Ok(vec![page_id])
    }

    /// Creates many pages in one commit (the import of ADR-0036). Each page is appended after the
    /// existing children of its parent, in the order given, so a parent may be a page created
    /// earlier in the same batch. Sets what [`Self::create_page`] sets (id, title, creation time,
    /// the edit stamp, the creating device) and the icon. Returns the ids created, in order.
    ///
    /// The positions come from a count per parent, kept while the batch is written, so a batch of
    /// `n` siblings costs `n` creations and not `n` scans of the siblings. The commit carries
    /// [`IMPORT_ORIGIN`]: the pages are not an undo step and do not push earlier steps off the
    /// undo stack. The index is settled once, at the end.
    ///
    /// Everything is checked before anything is written (a refusal writes nothing): an id that
    /// exists, is purged or repeats is [`ModelError::DuplicatePage`]; a parent that is not live, is
    /// unknown, comes later in the batch or is the page itself is [`ModelError::ParentUnavailable`].
    pub fn create_pages(
        &mut self,
        pages: &[NewPage],
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let mut seen: HashSet<PageId> = HashSet::with_capacity(pages.len());
        for page in pages {
            if self.index.node(page.page_id).is_some()
                || self.is_tombstoned(page.page_id)
                || seen.contains(&page.page_id)
            {
                return Err(ModelError::DuplicatePage);
            }
            if let Some(parent) = page.parent
                && !seen.contains(&parent)
                && self.live_node(parent).is_none()
            {
                return Err(ModelError::ParentUnavailable);
            }
            seen.insert(page.page_id);
        }
        if pages.is_empty() {
            return Ok(Vec::new());
        }

        // Whatever is pending is committed first, and the batch is committed with its own origin
        // whether or not the writes went through (as `props_write` does).
        self.close_group();
        self.commit();
        let written = self.write_new_pages(pages, now_ms);
        self.doc
            .inner()
            .commit_with(CommitOptions::new().origin(IMPORT_ORIGIN));
        match written {
            Ok(()) => {
                self.settle_index();
                Ok(pages.iter().map(|page| page.page_id).collect())
            }
            Err(err) => {
                // Some pages may be in; the index follows the document.
                self.rebuild_index();
                Err(err)
            }
        }
    }

    /// The writes of [`Self::create_pages`] (already validated).
    fn write_new_pages(&mut self, pages: &[NewPage], now_ms: i64) -> Result<(), ModelError> {
        let mut nodes: HashMap<PageId, TreeID> = HashMap::with_capacity(pages.len());
        // The next position under each parent (`None`: the root): all its children, hidden ones
        // included, as `engine_position` counts them.
        let mut next: HashMap<Option<PageId>, usize> = HashMap::new();
        for page in pages {
            let parent = match page.parent {
                None => TreeParentId::Root,
                Some(parent) => TreeParentId::Node(
                    nodes
                        .get(&parent)
                        .copied()
                        .or_else(|| self.live_node(parent))
                        .ok_or(ModelError::ParentUnavailable)?,
                ),
            };
            let position = match next.get(&page.parent) {
                Some(at) => *at,
                None => self.tree.children_num(parent).unwrap_or(0),
            };
            next.insert(page.parent, position + 1);

            let node = self.tree.create_at(parent, position).map_err(engine)?;
            let meta = self.tree.get_meta(node).map_err(engine)?;
            meta::set_string(&meta, meta::ID, &page.page_id.to_string())?;
            meta::set_string(&meta, meta::TITLE, &page.title)?;
            meta::set_int(&meta, meta::CREATED_AT, now_ms)?;
            self.stamp_edit(&meta, now_ms)?;
            if let Some(author) = self.author {
                meta::set_string(&meta, meta::CREATED_BY, &author.to_string())?;
            }
            if let Some(icon) = page.icon.as_deref() {
                meta::set_string(&meta, meta::ICON, icon)?;
            }
            if let Some(cover) = page.cover.as_deref() {
                meta::set_string(&meta, meta::COVER, cover)?;
                if let Some(y) = page
                    .cover_y
                    .map(|y| y.min(COVER_Y_MAX))
                    .filter(|y| *y != COVER_Y_DEFAULT)
                {
                    meta::set_int(&meta, meta::COVER_Y, i64::from(y))?;
                }
            }
            self.index.insert(page.page_id, node);
            nodes.insert(page.page_id, node);
        }
        Ok(())
    }

    /// Renames a page (live or trashed). Returns the affected ids.
    ///
    /// Renaming to the current title changes nothing and returns no ids.
    pub fn rename_page(
        &mut self,
        page_id: PageId,
        title: &str,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let meta = self.tree.get_meta(node).map_err(engine)?;
        if meta::string(&meta, meta::TITLE).as_deref() == Some(title) {
            return Ok(Vec::new());
        }
        self.begin_edit(Some(EditKey::Title(page_id)), now_ms);
        meta::set_string(&meta, meta::TITLE, title)?;
        self.stamp_edit(&meta, now_ms)?;
        self.commit();
        Ok(vec![page_id])
    }

    /// Sets or clears a page icon. Returns the affected ids.
    ///
    /// Setting the current icon changes nothing and returns no ids.
    pub fn set_page_icon(
        &mut self,
        page_id: PageId,
        icon: Option<&str>,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let meta = self.tree.get_meta(node).map_err(engine)?;
        if meta::string(&meta, meta::ICON).as_deref() == icon {
            return Ok(Vec::new());
        }
        self.begin_edit(Some(EditKey::Icon(page_id)), now_ms);
        meta::set_opt_string(&meta, meta::ICON, icon)?;
        self.stamp_edit(&meta, now_ms)?;
        self.commit();
        Ok(vec![page_id])
    }

    /// Sets, repositions or clears a page cover (ADR-0040). Returns the affected ids.
    ///
    /// `cover` is the new cover as stored (`attachment:<id>`, `color:<name>`, `gradient:<name>`; the
    /// caller validated it) and `cover_y` its vertical position in percent (`None` and 50 both mean
    /// the default and are stored as no value; a value above 100 is clamped). `cover` of `None`
    /// clears the cover and its position. One undo step with the icon's merge rule. Setting what the
    /// page already has changes nothing and returns no ids.
    pub fn set_page_cover(
        &mut self,
        page_id: PageId,
        cover: Option<&str>,
        cover_y: Option<u8>,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let meta = self.tree.get_meta(node).map_err(engine)?;
        let wanted_y = cover
            .and(cover_y)
            .map(|y| y.min(COVER_Y_MAX))
            .filter(|y| *y != COVER_Y_DEFAULT);
        let current_y = meta::int(&meta, meta::COVER_Y);
        if meta::string(&meta, meta::COVER).as_deref() == cover
            && current_y == wanted_y.map(i64::from)
        {
            return Ok(Vec::new());
        }
        self.begin_edit(Some(EditKey::Cover(page_id)), now_ms);
        meta::set_opt_string(&meta, meta::COVER, cover)?;
        match wanted_y {
            Some(y) => meta::set_int(&meta, meta::COVER_Y, i64::from(y))?,
            None => meta::remove(&meta, meta::COVER_Y)?,
        }
        self.stamp_edit(&meta, now_ms)?;
        self.commit();
        Ok(vec![page_id])
    }

    /// Sets or clears the appearance of a page (ADR-0040): its font, small text and lock. Returns the
    /// affected ids.
    ///
    /// Each argument is optional and `None` leaves that part as it is; [`PageFont::Default`] and
    /// `false` clear the font, the small text and the lock (stored as no value: `font` is a string
    /// only for the two fonts, `smallText` and `locked` the boolean `true`). The lock changes nothing
    /// here: it is a guard the UI applies, not a permission. The parts that change are written in one
    /// undo step with the icon's merge rule and stamp the page; setting what the page already has
    /// changes nothing and returns no ids.
    pub fn set_page_appearance(
        &mut self,
        page_id: PageId,
        font: Option<PageFont>,
        small_text: Option<bool>,
        locked: Option<bool>,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let meta = self.tree.get_meta(node).map_err(engine)?;
        // What actually changes: a part whose wanted value is what the page holds is left alone (the
        // font is compared as stored, so a name that is not a font is cleared by `Default` and
        // replaced by a font).
        let stored_font = meta::string(&meta, meta::FONT);
        let font_write = font
            .map(PageFont::stored)
            .filter(|wanted| *wanted != stored_font.as_deref());
        let small_write =
            small_text.filter(|wanted| *wanted != meta::flag(&meta, meta::SMALL_TEXT));
        let lock_write = locked.filter(|wanted| *wanted != meta::flag(&meta, meta::LOCKED));
        if font_write.is_none() && small_write.is_none() && lock_write.is_none() {
            return Ok(Vec::new());
        }
        self.begin_edit(Some(EditKey::Appearance(page_id)), now_ms);
        if let Some(wanted) = font_write {
            meta::set_opt_string(&meta, meta::FONT, wanted)?;
        }
        if let Some(on) = small_write {
            meta::set_flag(&meta, meta::SMALL_TEXT, on)?;
        }
        if let Some(on) = lock_write {
            meta::set_flag(&meta, meta::LOCKED, on)?;
        }
        self.stamp_edit(&meta, now_ms)?;
        self.commit();
        Ok(vec![page_id])
    }

    /// Moves a live page under `parent` (or the root) to `index` among its new
    /// siblings (clamped). Returns the affected ids (the page and its subtree).
    ///
    /// `index` is the page's position among its live siblings after the move, so a
    /// reorder within the same parent and a reparenting use the same convention.
    /// A move that changes nothing writes nothing and returns no ids.
    pub fn move_page(
        &mut self,
        page_id: PageId,
        parent: Option<PageId>,
        index: usize,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        if !self.is_live(node) {
            return Err(ModelError::PageTrashed);
        }
        let new_parent = match parent {
            None => TreeParentId::Root,
            Some(parent) if parent == page_id => return Err(ModelError::CyclicMove),
            Some(parent) => {
                let parent_node = self
                    .live_node(parent)
                    .ok_or(ModelError::ParentUnavailable)?;
                if self.is_ancestor(node, parent_node) {
                    return Err(ModelError::CyclicMove);
                }
                TreeParentId::Node(parent_node)
            }
        };

        if self.tree.parent(node) == Some(new_parent) {
            let live = self.visible_children(new_parent);
            let current = live.iter().position(|(n, _)| *n == node);
            let target = index.min(live.len().saturating_sub(1));
            if current == Some(target) {
                return Ok(Vec::new());
            }
        }
        let position = self.engine_position(new_parent, Some(node), Some(index));

        self.begin_edit(Some(EditKey::Move(page_id)), now_ms);
        self.tree
            .mov_to(node, new_parent, position)
            .map_err(|err| match err {
                LoroError::TreeError(LoroTreeError::CyclicMoveError) => ModelError::CyclicMove,
                other => engine(other),
            })?;
        let meta = self.tree.get_meta(node).map_err(engine)?;
        self.stamp_edit(&meta, now_ms)?;
        self.commit();
        let affected = self.preorder(node);
        self.settle_index();
        Ok(affected)
    }

    /// Moves a live page and its subtree to the trash. Returns the affected ids.
    ///
    /// Records `trashed_at = now_ms` and `trashed_from` (the parent page; absent for
    /// a root) on the page, and appends it under the trash root.
    pub fn trash_page(&mut self, page_id: PageId, now_ms: i64) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let placement = self.placement(node).ok_or(ModelError::PageNotFound)?;
        if placement.trashed_top.is_some() {
            return Err(ModelError::PageTrashed);
        }

        self.begin_edit(None, now_ms);
        let trash = self.trash_root_for_write()?;
        let meta = self.tree.get_meta(node).map_err(engine)?;
        meta::set_int(&meta, meta::TRASHED_AT, now_ms)?;
        let from = placement.parent.map(|p| p.to_string());
        meta::set_opt_string(&meta, meta::TRASHED_FROM, from.as_deref())?;
        self.stamp_edit(&meta, now_ms)?;
        self.tree.mov(node, trash).map_err(engine)?;
        self.commit();
        let affected = self.preorder(node);
        self.settle_index();
        Ok(affected)
    }

    /// Restores a top-level trashed page to its previous parent (or the root when
    /// that parent is gone or trashed). Returns the affected ids.
    ///
    /// The page is appended after its new live siblings; `trashed_at` and
    /// `trashed_from` are cleared and `updated_at` is set.
    pub fn restore_page(
        &mut self,
        page_id: PageId,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let is_top_trashed = matches!(
            self.tree.parent(node),
            Some(TreeParentId::Node(parent)) if self.index.is_trash_root(parent)
        );
        if !is_top_trashed {
            return Err(ModelError::PageNotTrashed);
        }
        let meta = self.tree.get_meta(node).map_err(engine)?;
        let target = meta::page_id(&meta, meta::TRASHED_FROM)
            .and_then(|parent| self.live_node(parent))
            .map_or(TreeParentId::Root, TreeParentId::Node);

        self.begin_edit(None, now_ms);
        meta::remove(&meta, meta::TRASHED_AT)?;
        meta::remove(&meta, meta::TRASHED_FROM)?;
        self.stamp_edit(&meta, now_ms)?;
        self.tree.mov(node, target).map_err(engine)?;
        self.commit();
        let affected = self.preorder(node);
        self.settle_index();
        Ok(affected)
    }

    /// Permanently deletes a page and its subtree and records tombstones. Returns
    /// the ids that disappeared.
    pub fn purge_page(&mut self, page_id: PageId, now_ms: i64) -> Result<Vec<PageId>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        let ids = self.preorder(node);

        self.begin_edit(None, now_ms);
        for id in &ids {
            meta::set_int(&self.tombstones, &id.to_string(), now_ms)?;
        }
        self.tree.delete(node).map_err(engine)?;
        self.commit();
        // A purged page leaves the favourites. Not an undo step, like every favourite write:
        // undoing the purge brings the page back, not its favourite.
        self.forget_favorites(&ids)?;
        for id in &ids {
            self.index.remove(*id);
        }
        self.settle_index();
        Ok(ids)
    }

    /// Purged page ids with their purge time.
    ///
    /// Sorted by page id.
    #[must_use]
    pub fn tombstones(&self) -> Vec<(PageId, i64)> {
        let mut out: Vec<(PageId, i64)> = self
            .tombstones
            .keys()
            .filter_map(|key| {
                let id = key.parse::<PageId>().ok()?;
                Some((id, meta::int(&self.tombstones, &key)?))
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// Writes (or replaces) the manifest of `manifest.attachment_id`.
    ///
    /// Not recorded on the undo stack (committed with [`ATTACHMENTS_ORIGIN`]).
    /// Errors with [`ModelError::Shape`] when the manifest cannot be encoded (see
    /// [`AttachmentManifest::to_loro`]); nothing is written then. Consistency (chunk
    /// count against size) is the caller's job and is reported by
    /// [`Self::check_invariants`].
    pub fn attachment_put(&self, manifest: &AttachmentManifest) -> Result<(), ModelError> {
        let value = manifest.to_loro()?;
        let key = manifest.attachment_id.to_string();
        self.manifest_write(|| self.attachments.insert(&key, value).map_err(engine))
    }

    /// The manifest of an attachment; `None` when absent or unreadable (another
    /// manifest version, damaged data: see [`Self::check_invariants`]).
    #[must_use]
    pub fn attachment(&self, attachment_id: AttachmentId) -> Option<AttachmentManifest> {
        let value = meta::value(&self.attachments, &attachment_id.to_string())?;
        AttachmentManifest::from_loro(attachment_id, &value).ok()
    }

    /// Every readable manifest, by `created_at`, then attachment id.
    #[must_use]
    pub fn attachments(&self) -> Vec<AttachmentManifest> {
        let mut out: Vec<AttachmentManifest> = self
            .manifest_entries()
            .into_iter()
            .filter_map(|(_, manifest)| manifest.ok())
            .collect();
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.attachment_id.cmp(&b.attachment_id))
        });
        out
    }

    /// Removes the manifest of an attachment (no-op when absent).
    ///
    /// Not recorded on the undo stack (committed with [`ATTACHMENTS_ORIGIN`]).
    pub fn attachment_remove(&self, attachment_id: AttachmentId) -> Result<(), ModelError> {
        let key = attachment_id.to_string();
        self.manifest_write(|| meta::remove(&self.attachments, &key))
    }

    /// Whether this device's last change can be undone / redone.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    /// See [`Self::can_undo`].
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Undoes the last local change. Returns `Ok(None)` when there was nothing to
    /// undo, otherwise the ids whose index rows may have changed (conservatively,
    /// every page: the owner re-syncs the whole space).
    pub fn undo(&mut self) -> Result<Option<Vec<PageId>>, ModelError> {
        self.undo_or_redo(false)
    }

    /// See [`Self::undo`].
    pub fn redo(&mut self) -> Result<Option<Vec<PageId>>, ModelError> {
        self.undo_or_redo(true)
    }

    /// All page ids known to the document (live and trashed), for index rebuilds.
    ///
    /// Live pages in tree pre-order, then the trashed subtrees.
    #[must_use]
    pub fn all_page_ids(&self) -> Vec<PageId> {
        let mut out = Vec::with_capacity(self.index.pages.len());
        for (node, _) in self.visible_children(TreeParentId::Root) {
            out.extend(self.preorder(node));
        }
        for (node, _) in self.top_trashed() {
            out.extend(self.preorder(node));
        }
        out
    }

    /// The icon of one page (live or trashed): `None` when it has none or the page is unknown.
    /// A cheap read of the page's meta map, for the derived icon references (ADR-0029).
    #[must_use]
    pub fn page_icon(&self, page_id: PageId) -> Option<String> {
        let node = self.index.node(page_id)?;
        meta::string(&self.meta_of(node)?, meta::ICON)
    }

    /// `(page id, icon)` of every page that has an icon, live and trashed, in page id order.
    #[must_use]
    pub fn page_icons(&self) -> Vec<(PageId, String)> {
        let mut icons: Vec<(PageId, String)> = self
            .index
            .pages
            .iter()
            .filter_map(|(page, node)| {
                let icon = meta::string(&self.meta_of(*node)?, meta::ICON)?;
                Some((*page, icon))
            })
            .collect();
        icons.sort_unstable_by_key(|(page, _)| *page);
        icons
    }

    /// The cover of one page (live or trashed): `None` when it has none or the page is unknown.
    #[must_use]
    pub fn page_cover(&self, page_id: PageId) -> Option<String> {
        let node = self.index.node(page_id)?;
        meta::string(&self.meta_of(node)?, meta::COVER)
    }

    /// `(page id, cover)` of every page that has a cover, live and trashed, in page id order.
    #[must_use]
    pub fn page_covers(&self) -> Vec<(PageId, String)> {
        let mut covers: Vec<(PageId, String)> = self
            .index
            .pages
            .iter()
            .filter_map(|(page, node)| {
                let cover = meta::string(&self.meta_of(*node)?, meta::COVER)?;
                Some((*page, cover))
            })
            .collect();
        covers.sort_unstable_by_key(|(page, _)| *page);
        covers
    }

    /// The underlying document, for persistence.
    #[must_use]
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// Must be called after the owner imported remote data into [`Self::doc`]:
    /// rebuilds the id index.
    pub fn refresh(&mut self) -> Result<(), ModelError> {
        self.close_group();
        self.rebuild_index();
        Ok(())
    }

    /// Engine-independent JSON of the whole document (space, live tree, trash,
    /// tombstones, and attachment manifests, properties and favourites when there
    /// are any). The migration escape
    /// hatch and the golden-test format.
    ///
    /// Deterministic: object keys are inserted in sorted order, the trash is listed
    /// most recently trashed first, tombstones are keyed by page id and manifests by
    /// attachment id.
    #[must_use]
    pub fn to_portable(&self) -> serde_json::Value {
        portable::structure(self)
    }

    /// Checks the invariants of `docs/data-model/domain-model.md`; returns the
    /// violations (empty = healthy). Used by tests and diagnostics.
    ///
    /// Attachments: every entry of the `attachments` map must be keyed by an attachment
    /// id and hold a readable version-1 manifest whose chunk count is
    /// `floor(size / chunk_size) + 1` (the final chunk is always a padded one, see
    /// [`AttachmentManifest::expected_chunk_count`]).
    ///
    /// Messages contain page ids, attachment ids and engine ids only, never titles,
    /// names or MIME types.
    #[must_use]
    pub fn check_invariants(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let fresh = PageIndex::build(&self.tree, &tombstoned(&self.tombstones));
        problems.extend(fresh.anomalies.iter().cloned());
        if fresh != self.index {
            problems.push("page index is out of date (refresh after import?)".into());
        }
        if self.index.trash_roots.is_empty() {
            problems.push("the document has no trash root".into());
        }

        let live: Vec<PageId> = self
            .visible_children(TreeParentId::Root)
            .into_iter()
            .flat_map(|(node, _)| self.preorder(node))
            .collect();
        let trashed: Vec<PageId> = self
            .top_trashed()
            .into_iter()
            .flat_map(|(node, _)| self.preorder(node))
            .collect();
        let live_set: HashSet<PageId> = live.iter().copied().collect();
        let tombstoned = tombstoned(&self.tombstones);

        let mut seen = HashSet::new();
        for id in live.iter().chain(trashed.iter()) {
            if !seen.insert(*id) {
                problems.push(format!("page {id} appears twice in the tree"));
            }
        }
        if seen.len() != self.index.pages.len() {
            problems.push(format!(
                "{} indexed pages but {} reachable",
                self.index.pages.len(),
                seen.len()
            ));
        }
        for id in &trashed {
            if live_set.contains(id) {
                problems.push(format!("trashed page {id} is in the live tree"));
            }
        }
        for id in &seen {
            if tombstoned.contains(id) {
                problems.push(format!("purged page {id} is still in the tree"));
            }
        }

        for (&id, &node) in &self.index.pages {
            // Acyclic, and every page reaches a root within the tree's size.
            if self.root_of(node).is_none() {
                problems.push(format!("page {id} does not reach a root (cycle?)"));
                continue;
            }
            match self.placement(node) {
                None => problems.push(format!("page {id} has an unindexed ancestor")),
                Some(placement) if placement.trashed_top.is_none() => {
                    if let Some(parent) = placement.parent
                        && !live_set.contains(&parent)
                    {
                        problems.push(format!("live page {id} has a parent that is not live"));
                    }
                }
                Some(_) => {}
            }
            match self.meta_of(node) {
                Some(m) => {
                    if meta::string(&m, meta::TITLE).is_none() {
                        problems.push(format!("page {id} has no title"));
                    }
                    if meta::int(&m, meta::CREATED_AT).is_none()
                        || meta::int(&m, meta::UPDATED_AT).is_none()
                    {
                        problems.push(format!("page {id} lacks timestamps"));
                    }
                }
                None => problems.push(format!("page {id} has no metadata")),
            }
        }

        for root in self.tree.roots() {
            let marked = self
                .meta_of(root)
                .is_some_and(|m| meta::is_trash_marker(&m));
            if marked != self.index.is_trash_root(root) {
                problems.push(format!(
                    "root {} is misclassified as trash",
                    node_label(root)
                ));
            }
        }

        for (key, manifest) in self.manifest_entries() {
            match (key, manifest) {
                (None, _) => problems.push("an attachments entry has a malformed key".into()),
                (Some(_), Ok(manifest)) => problems.extend(manifest.problems()),
                (Some(id), Err(err)) => problems.push(format!("attachment {id}: {err}")),
            }
        }
        problems.extend(self.residue_problems());
        problems.extend(self.property_problems());
        problems.extend(self.favorite_problems());
        problems.extend(self.link_problems());
        problems
    }

    // ----- internals ------------------------------------------------------------------

    fn assemble(doc: Doc, space_id: SpaceId) -> Self {
        let space = doc.inner().get_map(meta::SPACE);
        let tree = doc.inner().get_tree(meta::PAGES);
        let tombstones = doc.inner().get_map(meta::TOMBSTONES);
        let attachments = doc.inner().get_map(meta::ATTACHMENTS);
        let properties = doc.inner().get_map(meta::PROPERTIES);
        let favorites = doc.inner().get_map(meta::FAVORITES);
        let links = doc.inner().get_map(LINKS_MAP);
        let mut undo = UndoManager::new(doc.inner());
        undo.set_max_undo_steps(UNDO_MAX_STEPS);
        // Merging is done by `begin_edit` with the injected clock.
        undo.set_merge_interval(0);
        // Manifest writes are never undone (ADR-0020).
        undo.add_exclude_origin_prefix(ATTACHMENTS_ORIGIN);
        // Neither are schema and value writes (ADR-0026).
        undo.add_exclude_origin_prefix(PROPERTIES_ORIGIN);
        // Nor favourites (ADR-0029).
        undo.add_exclude_origin_prefix(FAVORITES_ORIGIN);
        // Nor imported pages (ADR-0036).
        undo.add_exclude_origin_prefix(IMPORT_ORIGIN);
        // Nor moves between spaces (ADR-0038).
        undo.add_exclude_origin_prefix(MOVE_ORIGIN);
        // Nor view links (ADR-0051).
        undo.add_exclude_origin_prefix(LINKS_ORIGIN);
        let index = PageIndex::build(&tree, &tombstoned(&tombstones));
        for anomaly in &index.anomalies {
            tracing::warn!(space = %space_id, %anomaly, "structure document anomaly");
        }
        Self {
            doc,
            space_id,
            space,
            tree,
            tombstones,
            attachments,
            properties,
            favorites,
            links,
            undo,
            index,
            group: None,
            author: None,
            formula_time: properties::FormulaTime::default(),
            formula_people: std::sync::Mutex::default(),
        }
    }

    fn commit(&self) {
        self.doc.inner().commit();
    }

    /// A manifest write, committed on its own with [`ATTACHMENTS_ORIGIN`] so that the
    /// undo manager leaves it out (anything pending is committed first, normally).
    fn manifest_write(
        &self,
        write: impl FnOnce() -> Result<(), ModelError>,
    ) -> Result<(), ModelError> {
        self.commit();
        let result = write();
        self.doc
            .inner()
            .commit_with(CommitOptions::new().origin(ATTACHMENTS_ORIGIN));
        result
    }

    /// Every entry of the `attachments` map, sorted by key: the attachment id (`None`
    /// when the key is not one) and the manifest read from the value.
    fn manifest_entries(
        &self,
    ) -> Vec<(Option<AttachmentId>, Result<AttachmentManifest, ModelError>)> {
        let LoroValue::Map(map) = self.attachments.get_value() else {
            return Vec::new();
        };
        let mut entries: Vec<(&String, &LoroValue)> = map.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        entries
            .into_iter()
            .map(|(key, value)| match key.parse::<AttachmentId>() {
                Ok(id) => (Some(id), AttachmentManifest::from_loro(id, value)),
                Err(_) => (
                    None,
                    Err(ModelError::Shape("malformed attachment key".into())),
                ),
            })
            .collect()
    }

    fn rebuild_index(&mut self) {
        let index = PageIndex::build(&self.tree, &tombstoned(&self.tombstones));
        if index.anomalies != self.index.anomalies {
            for anomaly in &index.anomalies {
                tracing::warn!(space = %self.space_id, %anomaly, "structure document anomaly");
            }
        }
        self.index = index;
    }

    /// After a structural edit. The incremental index update is exact on healthy
    /// documents; with damaged data (duplicate ids make visibility depend on tree
    /// order) the index is rebuilt instead.
    fn settle_index(&mut self) {
        if !self.index.anomalies.is_empty() {
            self.rebuild_index();
        }
    }

    /// A write that must not land on the undo stack.
    fn untracked(&self, write: impl FnOnce() -> Result<(), ModelError>) -> Result<(), ModelError> {
        self.undo.pause();
        let result = write();
        self.commit();
        self.undo.resume();
        result
    }

    /// Starts or continues an undo group before a local edit.
    fn begin_edit(&mut self, key: Option<EditKey>, now_ms: i64) {
        if let (Some(key), Some(open)) = (key, self.group) {
            let elapsed = now_ms.checked_sub(open.started_at);
            if open.key == key && elapsed.is_some_and(|d| (0..UNDO_MERGE_WINDOW_MS).contains(&d)) {
                return;
            }
        }
        self.close_group();
        if let Some(key) = key {
            match self.undo.group_start() {
                Ok(()) => {
                    self.group = Some(OpenGroup {
                        key,
                        started_at: now_ms,
                    });
                }
                Err(err) => tracing::debug!(error = %err, "undo group not started"),
            }
        }
    }

    fn close_group(&mut self) {
        if self.group.take().is_some() {
            self.undo.group_end();
        }
    }

    fn undo_or_redo(&mut self, redo: bool) -> Result<Option<Vec<PageId>>, ModelError> {
        self.close_group();
        let before = self.all_page_ids();
        let guard = self.move_guard();
        let changed = if redo {
            self.undo.redo()
        } else {
            self.undo.undo()
        }
        .map_err(engine)?;
        if !changed {
            return Ok(None);
        }
        self.commit();
        self.rebuild_index();
        // Undo never touches a move (ADR-0038): put back what it took from one.
        self.repair_after_undo(&guard)?;
        let mut ids = before;
        let mut seen: HashSet<PageId> = ids.iter().copied().collect();
        ids.extend(
            self.all_page_ids()
                .into_iter()
                .filter(|id| seen.insert(*id)),
        );
        Ok(Some(ids))
    }

    fn is_tombstoned(&self, page_id: PageId) -> bool {
        self.tombstones.get(&page_id.to_string()).is_some()
    }

    fn meta_of(&self, node: TreeID) -> Option<LoroMap> {
        self.tree.get_meta(node).ok()
    }

    fn info(
        &self,
        id: PageId,
        node: TreeID,
        parent: Option<PageId>,
        trashed_at: Option<i64>,
        schema: &properties::Schema,
    ) -> PageInfo {
        let meta = self.meta_of(node);
        let text = |key| meta.as_ref().and_then(|m| meta::string(m, key));
        let int = |key| meta.as_ref().and_then(|m| meta::int(m, key));
        let flag = |key| meta.as_ref().is_some_and(|m| meta::flag(m, key));
        let device = |key| text(key).and_then(|raw| raw.parse::<DeviceId>().ok());
        let created_at = int(meta::CREATED_AT).unwrap_or(0);
        PageInfo {
            id,
            space_id: self.space_id,
            parent_id: parent,
            title: text(meta::TITLE).unwrap_or_default(),
            icon: text(meta::ICON),
            cover: text(meta::COVER),
            cover_y: cover_y_of(text(meta::COVER).is_some(), int(meta::COVER_Y)),
            font: text(meta::FONT).as_deref().and_then(PageFont::from_stored),
            small_text: flag(meta::SMALL_TEXT),
            locked: flag(meta::LOCKED),
            created_at,
            updated_at: int(meta::UPDATED_AT).unwrap_or(created_at),
            trashed_at,
            created_by: device(meta::CREATED_BY),
            edited_by: device(meta::EDITED_BY),
            properties: self.values_of(node, schema),
            template: flag(meta::TEMPLATE),
            template_placeholders: self.placeholders_of(node, schema),
        }
    }

    /// `trashed_at` of a top-level trashed page. A concurrent restore and trash can
    /// leave the page in the trash without the field; `updated_at` stands in then.
    fn trashed_at_of(&self, top: TreeID) -> i64 {
        self.meta_of(top)
            .and_then(|m| {
                meta::int(&m, meta::TRASHED_AT).or_else(|| meta::int(&m, meta::UPDATED_AT))
            })
            .unwrap_or(0)
    }

    /// Visible pages directly under `parent`, in sibling order.
    fn visible_children(&self, parent: TreeParentId) -> Vec<(TreeID, PageId)> {
        self.tree
            .children(parent)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|node| self.index.page(node).map(|id| (node, id)))
            .collect()
    }

    /// Top-level trashed pages, in trash-root then sibling order.
    fn top_trashed(&self) -> Vec<(TreeID, PageId)> {
        self.index
            .trash_roots
            .iter()
            .flat_map(|root| self.visible_children(TreeParentId::Node(*root)))
            .collect()
    }

    /// Top-level trashed pages with their effective `trashed_at`, most recently
    /// trashed first, ties by page id.
    fn trash_entries(&self) -> Vec<(TreeID, PageId, i64)> {
        let mut entries: Vec<(TreeID, PageId, i64)> = self
            .top_trashed()
            .into_iter()
            .map(|(node, id)| (node, id, self.trashed_at_of(node)))
            .collect();
        entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
        entries
    }

    /// Visible subtree of `start`, pre-order, `start` first.
    fn preorder(&self, start: TreeID) -> Vec<PageId> {
        let mut out = Vec::new();
        let Some(id) = self.index.page(start) else {
            return out;
        };
        let mut stack = vec![(start, id)];
        while let Some((node, id)) = stack.pop() {
            out.push(id);
            let children = self.visible_children(TreeParentId::Node(node));
            stack.extend(children.into_iter().rev());
        }
        out
    }

    /// Builds nested values for the subtrees of `tops` (iteratively: no recursion
    /// limit on deep trees). `make` receives the node, its page id and its parent
    /// page (`None` for the tops).
    fn forest<T>(
        &self,
        tops: Vec<(TreeID, PageId)>,
        make: &mut dyn FnMut(TreeID, PageId, Option<PageId>) -> T,
        attach: &dyn Fn(&mut T, T),
    ) -> Vec<T> {
        struct Frame<T> {
            value: T,
            id: PageId,
            pending: std::vec::IntoIter<(TreeID, PageId)>,
        }
        let mut out = Vec::with_capacity(tops.len());
        for (node, id) in tops {
            let mut stack = vec![Frame {
                value: make(node, id, None),
                id,
                pending: self.visible_children(TreeParentId::Node(node)).into_iter(),
            }];
            while let Some(top) = stack.last_mut() {
                if let Some((child, child_id)) = top.pending.next() {
                    let parent = top.id;
                    let frame = Frame {
                        value: make(child, child_id, Some(parent)),
                        id: child_id,
                        pending: self.visible_children(TreeParentId::Node(child)).into_iter(),
                    };
                    stack.push(frame);
                } else if let Some(done) = stack.pop() {
                    match stack.last_mut() {
                        Some(parent) => attach(&mut parent.value, done.value),
                        None => out.push(done.value),
                    }
                }
            }
        }
        out
    }

    /// Where a visible node sits; `None` if it is not visible.
    fn placement(&self, node: TreeID) -> Option<Placement> {
        let mut parent = None;
        let mut current = node;
        for depth in 0..=self.index.pages.len() {
            match self.tree.parent(current)? {
                TreeParentId::Root => {
                    return Some(Placement {
                        parent,
                        trashed_top: None,
                    });
                }
                TreeParentId::Node(up) if self.index.is_trash_root(up) => {
                    return Some(Placement {
                        parent,
                        trashed_top: Some(current),
                    });
                }
                TreeParentId::Node(up) => {
                    let up_id = self.index.page(up)?;
                    if depth == 0 {
                        parent = Some(up_id);
                    }
                    current = up;
                }
                TreeParentId::Deleted | TreeParentId::Unexist => return None,
            }
        }
        None
    }

    /// The root above `node` (itself if it is a root), within a bounded walk.
    fn root_of(&self, node: TreeID) -> Option<TreeID> {
        let mut current = node;
        for _ in 0..=self.index.pages.len() + 1 {
            match self.tree.parent(current)? {
                TreeParentId::Root => return Some(current),
                TreeParentId::Node(up) => current = up,
                TreeParentId::Deleted | TreeParentId::Unexist => return None,
            }
        }
        None
    }

    fn is_live(&self, node: TreeID) -> bool {
        self.placement(node)
            .is_some_and(|p| p.trashed_top.is_none())
    }

    /// The node of a live page.
    fn live_node(&self, page_id: PageId) -> Option<TreeID> {
        self.index.node(page_id).filter(|node| self.is_live(*node))
    }

    /// Whether `ancestor` is `node` or one of its ancestors.
    fn is_ancestor(&self, ancestor: TreeID, node: TreeID) -> bool {
        let mut current = node;
        for _ in 0..=self.index.pages.len() + 1 {
            if current == ancestor {
                return true;
            }
            match self.tree.parent(current) {
                Some(TreeParentId::Node(up)) => current = up,
                _ => return false,
            }
        }
        false
    }

    /// Loro's index (among all children of `parent`, `moving` excluded) that puts a
    /// page at live position `index` (clamped; `None` = after the last one).
    ///
    /// Loro's `create_at`/`mov_to` take the final position among *all* children,
    /// which may include trash roots or hidden nodes; anchoring on the live sibling
    /// that must follow keeps the live order right whatever sits in between.
    fn engine_position(
        &self,
        parent: TreeParentId,
        moving: Option<TreeID>,
        index: Option<usize>,
    ) -> usize {
        let siblings: Vec<TreeID> = self
            .tree
            .children(parent)
            .unwrap_or_default()
            .into_iter()
            .filter(|node| Some(*node) != moving)
            .collect();
        let live: Vec<usize> = siblings
            .iter()
            .enumerate()
            .filter(|(_, node)| self.index.page(**node).is_some())
            .map(|(pos, _)| pos)
            .collect();
        match index.and_then(|i| live.get(i)) {
            Some(&pos) => pos,
            None => siblings.len(),
        }
    }

    /// The trash root new trash goes to (the first one; created if the document
    /// has none, which only damaged or foreign documents lack).
    fn trash_root_for_write(&mut self) -> Result<TreeID, ModelError> {
        if let Some(root) = self.index.trash_roots.first() {
            return Ok(*root);
        }
        let root = create_trash_root(&self.tree)?;
        self.index.trash_roots.push(root);
        Ok(root)
    }
}

/// The stored position of a cover as the page reports it: only with a cover, only a valid value
/// other than the default (anything else a foreign writer left is the default).
pub(crate) fn cover_y_of(has_cover: bool, stored: Option<i64>) -> Option<u8> {
    stored
        .filter(|_| has_cover)
        .and_then(|y| u8::try_from(y).ok())
        .filter(|y| *y <= COVER_Y_MAX && *y != COVER_Y_DEFAULT)
}

fn write_new_space(doc: &Doc, meta: &SpaceMeta) -> Result<(), ModelError> {
    let space = doc.inner().get_map(meta::SPACE);
    meta::set_string(&space, meta::ID, &meta.id.to_string())?;
    meta::set_string(&space, meta::NAME, &meta.name)?;
    meta::set_opt_string(&space, meta::ICON, meta.icon.as_deref())?;
    meta::set_int(&space, meta::CREATED_AT, meta.created_at)?;
    create_trash_root(&doc.inner().get_tree(meta::PAGES))?;
    Ok(())
}

fn create_trash_root(tree: &LoroTree) -> Result<TreeID, ModelError> {
    let root = tree.create(TreeParentId::Root).map_err(engine)?;
    let meta = tree.get_meta(root).map_err(engine)?;
    meta::set_string(&meta, meta::KIND, meta::TRASH_KIND)?;
    Ok(root)
}

#[cfg(test)]
mod tests;
