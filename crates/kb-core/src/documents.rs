//! Document manager: keeps the account and structure documents in memory, persists
//! their updates with the derived index rows in one transaction, compacts the
//! update log, and implements the Milestone 2 handlers
//! (`docs/development/milestone-02.md`).

use std::collections::{HashMap, HashSet};

use kb_doc::{Doc, DocError, DocKind, Version};
use kb_model::formula::FormulaEnv;
use kb_model::{AccountDoc, ModelError, PageDoc, SpaceMeta, StructureDoc};
use kb_storage::{
    Db, DocKindCode, DocQueries, DocRow, IconRefQueries, PageIndexRow, PagePropertyQueries,
    StorageError, UpdateOrigin, UpdateStats,
};
use kb_types::{
    AccountId, ApiError, DocId, ErrorCode, Event, MemberRole, PageFont, PageId, PageInfo, Response,
    Rng, RngExt, SpaceId, SpaceInfo, SpaceShareState, TreeView, UndoState,
};

use crate::properties::PropsScope;
use crate::{Core, map_storage_error, short_id, validate};

/// When a document's update log gets a new snapshot (`docs/sync/crdt-strategy.md`,
/// "History and compaction policy").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPolicy {
    /// Snapshot once more than this many updates follow the last snapshot.
    pub max_updates: u64,
    /// Snapshot once the updates since the last snapshot exceed this many bytes.
    pub max_bytes: u64,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            max_updates: 1_000,
            max_bytes: 4 * 1024 * 1024,
        }
    }
}

pub(crate) struct OpenAccount {
    pub(crate) doc_id: DocId,
    pub(crate) model: AccountDoc,
    pub(crate) last_persisted: Version,
}

pub(crate) struct OpenStructure {
    pub(crate) doc_id: DocId,
    pub(crate) model: StructureDoc,
    pub(crate) last_persisted: Version,
}

/// A page document an editor has open (`pages.rs`).
pub(crate) struct OpenPage {
    pub(crate) doc_id: DocId,
    pub(crate) model: PageDoc,
    pub(crate) last_persisted: Version,
}

/// Every document the core has open. Page documents are loaded on demand and dropped
/// when the editor closes them.
#[derive(Default)]
pub(crate) struct Documents {
    pub(crate) account: Option<OpenAccount>,
    pub(crate) structures: HashMap<SpaceId, OpenStructure>,
    pub(crate) pages: HashMap<PageId, OpenPage>,
}

/// Which open document a compaction concerns.
#[derive(Clone, Copy)]
pub(crate) enum DocRef {
    Account,
    Structure(SpaceId),
    Page(PageId),
}

/// Loads every persisted document (snapshot + updates) into memory.
pub(crate) fn load_documents(db: &Db, rng: &dyn Rng) -> Result<Documents, ApiError> {
    let mut docs = Documents::default();

    for row in db
        .docs_by_kind(DocKindCode::Account)
        .map_err(map_storage_error)?
    {
        if docs.account.is_some() {
            tracing::warn!(doc_id = %short_id(&row.doc_id), "extra account document ignored");
            continue;
        }
        let doc = load_doc(db, &row, DocKind::Account, rng)?;
        let model = AccountDoc::from_doc(doc).map_err(map_model_error)?;
        let last_persisted = model.doc().version();
        docs.account = Some(OpenAccount {
            doc_id: row.doc_id,
            model,
            last_persisted,
        });
    }

    for row in db
        .docs_by_kind(DocKindCode::Structure)
        .map_err(map_storage_error)?
    {
        let Some(space_id) = row.space_id else {
            tracing::warn!(doc_id = %short_id(&row.doc_id), "structure document without a space ignored");
            continue;
        };
        let doc = load_doc(db, &row, DocKind::Structure, rng)?;
        let model = StructureDoc::from_doc(doc).map_err(map_model_error)?;
        let last_persisted = model.doc().version();
        docs.structures.insert(
            space_id,
            OpenStructure {
                doc_id: row.doc_id,
                model,
                last_persisted,
            },
        );
    }

    tracing::debug!(
        spaces = docs.structures.len(),
        account = docs.account.is_some(),
        "documents loaded"
    );
    Ok(docs)
}

pub(crate) fn load_doc(
    db: &Db,
    row: &DocRow,
    kind: DocKind,
    rng: &dyn Rng,
) -> Result<Doc, ApiError> {
    let snapshot = db.snapshot_get(row.doc_id).map_err(map_storage_error)?;
    let updates = db
        .updates_after_snapshot(row.doc_id)
        .map_err(map_storage_error)?;
    Doc::load(
        kind,
        new_peer(rng),
        snapshot.as_ref().map(|(blob, _version)| blob.as_slice()),
        &updates,
    )
    .map_err(map_doc_error)
}

/// A fresh random peer id for one opened document (never derived from device ids).
pub(crate) fn new_peer(rng: &dyn Rng) -> u64 {
    u64::from_le_bytes(rng.random_array())
}

/// Re-indexes spaces whose `pages_index` disagrees with their document.
pub(crate) fn verify_index(db: &mut Db, docs: &Documents) -> Result<(), ApiError> {
    for (space_id, open) in &docs.structures {
        let indexed = db
            .pages_index_live_count(*space_id)
            .map_err(map_storage_error)?;
        let actual = open.model.live_page_count();
        // The pages in a template are counted too (a template is a live page): the rows that say a
        // page is in one (ADR-0056) must be as many as the document has.
        let indexed_templates = db
            .pages_index_template_count(*space_id)
            .map_err(map_storage_error)?;
        let actual_templates = count_of(open.model.pages_in_templates().len());
        if indexed != actual || indexed_templates != actual_templates {
            tracing::warn!(space = %short_id(&space_id), indexed, actual, indexed_templates, actual_templates, "page index out of sync; rebuilding");
            rebuild_space_index(db, *space_id, &open.model)?;
        }
    }
    Ok(())
}

pub(crate) fn rebuild_space_index(
    db: &mut Db,
    space_id: SpaceId,
    model: &StructureDoc,
) -> Result<(), ApiError> {
    let rows = all_rows(model);
    db.transaction(|tx| {
        tx.pages_index_clear_space(space_id)?;
        tx.pages_index_upsert(&rows)?;
        Ok(())
    })
    .map_err(map_storage_error)
}

fn all_rows(model: &StructureDoc) -> Vec<PageIndexRow> {
    let templates = model.pages_in_templates();
    model
        .all_page_ids()
        .into_iter()
        .filter_map(|id| model.page(id))
        .map(|info| {
            let in_template = templates.contains(&info.id);
            row_from_info(&info, in_template)
        })
        .collect()
}

/// The index row of a page. `in_template` says the page is a template or inside one (the page or an
/// ancestor has the flag, ADR-0056): only the structure knows, so the caller asks it.
fn row_from_info(info: &PageInfo, in_template: bool) -> PageIndexRow {
    PageIndexRow {
        page_id: info.id,
        space_id: info.space_id,
        parent_id: info.parent_id,
        title: info.title.clone(),
        trashed_at: info.trashed_at,
        updated_at: info.updated_at,
        in_template,
    }
}

fn count_of(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub(crate) fn new_doc_row(
    doc_id: DocId,
    kind: DocKindCode,
    space_id: Option<SpaceId>,
    now_ms: i64,
) -> DocRow {
    DocRow {
        doc_id,
        kind,
        space_id,
        created_at: now_ms,
        format_version: kb_doc::FORMAT_VERSION,
        min_reader_version: kb_doc::MIN_READER_VERSION,
    }
}

pub(crate) fn map_model_error(err: ModelError) -> ApiError {
    match err {
        ModelError::PageNotFound => ApiError::not_found("page not found"),
        ModelError::SpaceNotFound => ApiError::not_found("space not found"),
        ModelError::PageTrashed => ApiError::invalid("pageId", "the page is in the trash"),
        ModelError::PageNotTrashed => ApiError::invalid("pageId", "the page is not in the trash"),
        ModelError::ParentUnavailable => ApiError::invalid(
            "parentId",
            "the parent page does not exist or is in the trash",
        ),
        ModelError::CyclicMove => ApiError::invalid(
            "parentId",
            "a page cannot be moved under its own descendant",
        ),
        ModelError::InvalidOrder => ApiError::invalid(
            "order",
            "the order is not the current children of the parent",
        ),
        ModelError::DuplicatePage => ApiError::internal("page id collision"),
        ModelError::BlockNotFound => ApiError::not_found("block not found"),
        ModelError::PropertyNotFound => ApiError::not_found("property not found"),
        ModelError::OptionNotFound => ApiError::not_found("option not found"),
        ModelError::InvalidProperty { field, message } => ApiError::invalid(field, message),
        ModelError::InvalidFavorite { field, message } => ApiError::invalid(field, message),
        ModelError::DuplicateBlock => ApiError::internal("block id collision"),
        ModelError::InvalidBlock(msg) => ApiError::invalid("blockId", msg),
        ModelError::Shape(msg) => ApiError::storage(format!("document shape violated: {msg}")),
        ModelError::Doc(e) => map_doc_error(e),
    }
}

pub(crate) fn map_doc_error(err: DocError) -> ApiError {
    match err {
        DocError::NeedsNewerReader { needed, supported } => ApiError::new(
            ErrorCode::Storage,
            format!(
                "this document needs a newer app version (reader {needed}, this build reads {supported})"
            ),
        ),
        other => ApiError::storage(format!("document error: {other}")),
    }
}

impl Core {
    /// Changes when the update logs are compacted (tests use tiny thresholds).
    pub fn set_compaction_policy(&mut self, policy: CompactionPolicy) {
        self.policy = policy;
    }

    /// Creates the account document if the workspace has none yet (databases from
    /// Milestone 1 have a workspace but no documents).
    pub(crate) fn ensure_account(&mut self) -> Result<(), ApiError> {
        if self.docs.account.is_some() {
            return Ok(());
        }
        let now = self.clock.now_ms();
        let doc_id = DocId::generate(self.rng.as_ref());
        let model = AccountDoc::new(Doc::new(DocKind::Account, new_peer(self.rng.as_ref())));
        model
            .set_device_name(self.device.device_id, &self.device.name)
            .map_err(map_model_error)?;
        let blob = model
            .doc()
            .export_updates_since(None)
            .map_err(map_doc_error)?;
        let row = new_doc_row(doc_id, DocKindCode::Account, None, now);
        self.db
            .transaction(|tx| {
                tx.doc_insert(&row)?;
                if let Some(blob) = &blob {
                    tx.update_append(doc_id, blob, UpdateOrigin::Local, now)?;
                }
                Ok(())
            })
            .map_err(map_storage_error)?;
        let last_persisted = model.doc().version();
        self.docs.account = Some(OpenAccount {
            doc_id,
            model,
            last_persisted,
        });
        tracing::info!("created the account document");
        Ok(())
    }

    // ----- spaces -------------------------------------------------------------------

    /// A space as the list shows it: with what sharing says (`shareState`, `role`, ...); `None` for
    /// a space this account is leaving (it is gone from the list at once, its data goes when the
    /// `Leave` is on the server).
    fn listed_space(
        &self,
        rows: &HashMap<SpaceId, kb_storage::SyncFeedRow>,
        space_id: SpaceId,
        model: &StructureDoc,
    ) -> Option<SpaceInfo> {
        if rows
            .get(&space_id)
            .is_some_and(|row| self.space_is_leaving(row))
        {
            return None;
        }
        Some(self.decorate_space(rows, space_info_of(space_id, model)))
    }

    pub(crate) fn space_list(&self) -> Result<Vec<SpaceInfo>, ApiError> {
        let rows = self.shared_rows();
        let ordered: Vec<SpaceId> = self
            .docs
            .account
            .as_ref()
            .map(|a| a.model.spaces())
            .unwrap_or_default();
        let mut infos = Vec::with_capacity(self.docs.structures.len());
        let mut seen = HashSet::new();
        for space_id in ordered {
            match self.docs.structures.get(&space_id) {
                Some(open) => {
                    infos.extend(self.listed_space(&rows, space_id, &open.model));
                    seen.insert(space_id);
                }
                None => {
                    tracing::warn!(space = %short_id(&space_id), "space in account order has no document")
                }
            }
        }
        // Spaces whose document exists but that the order does not mention yet
        // (concurrent creation on another device, or a damaged account document).
        let mut extra: Vec<(i64, SpaceId)> = self
            .docs
            .structures
            .iter()
            .filter(|(id, _)| !seen.contains(*id))
            .map(|(id, open)| (open.model.space().created_at, *id))
            .collect();
        extra.sort_unstable();
        for (_, space_id) in extra {
            if let Some(open) = self.docs.structures.get(&space_id) {
                infos.extend(self.listed_space(&rows, space_id, &open.model));
            }
        }
        Ok(infos)
    }

    pub(crate) fn space_create(
        &mut self,
        name: &str,
        icon: Option<&str>,
    ) -> Result<SpaceInfo, ApiError> {
        let name = validate::display_name(name, 100, "name")?;
        let icon = validate::space_icon(icon)?;
        self.ensure_account()?;

        let now = self.clock.now_ms();
        let space_id = SpaceId::generate(self.rng.as_ref());
        let doc_id = DocId::generate(self.rng.as_ref());
        let model = StructureDoc::new(
            Doc::new(DocKind::Structure, new_peer(self.rng.as_ref())),
            SpaceMeta {
                id: space_id,
                name,
                icon,
                created_at: now,
            },
        );
        let structure_blob = model
            .doc()
            .export_updates_since(None)
            .map_err(map_doc_error)?
            .ok_or_else(|| ApiError::internal("new structure document exported nothing"))?;

        let account = self
            .docs
            .account
            .as_mut()
            .ok_or_else(|| ApiError::internal("account document missing"))?;
        account.model.add_space(space_id).map_err(map_model_error)?;
        let account_blob = account
            .model
            .doc()
            .export_updates_since(Some(&account.last_persisted))
            .map_err(map_doc_error)?;
        let account_doc_id = account.doc_id;

        let row = new_doc_row(doc_id, DocKindCode::Structure, Some(space_id), now);
        self.db
            .transaction(|tx| {
                tx.doc_insert(&row)?;
                tx.update_append(doc_id, &structure_blob, UpdateOrigin::Local, now)?;
                if let Some(blob) = &account_blob {
                    tx.update_append(account_doc_id, blob, UpdateOrigin::Local, now)?;
                }
                Ok(())
            })
            .map_err(map_storage_error)?;
        account.last_persisted = account.model.doc().version();

        let last_persisted = model.doc().version();
        let info = space_info_of(space_id, &model);
        self.docs.structures.insert(
            space_id,
            OpenStructure {
                doc_id,
                model,
                last_persisted,
            },
        );
        self.ensure_space_key(space_id)?;
        self.ensure_doc_key(space_id, doc_id)?;
        self.events.push(Event::SpacesChanged);
        Ok(info)
    }

    pub(crate) fn space_rename(
        &mut self,
        space_id: SpaceId,
        name: &str,
    ) -> Result<SpaceInfo, ApiError> {
        let name = validate::display_name(name, 100, "name")?;
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        self.structure(space_id)?
            .model
            .set_name(&name)
            .map_err(map_model_error)?;
        self.persist_structure(space_id, Some(&[]), UpdateOrigin::Local)?;
        self.events.push(Event::SpacesChanged);
        self.events.push(Event::StructureChanged { space_id });
        self.space_info(space_id)
    }

    pub(crate) fn space_set_icon(
        &mut self,
        space_id: SpaceId,
        icon: Option<&str>,
    ) -> Result<SpaceInfo, ApiError> {
        let icon = validate::space_icon(icon)?;
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        self.structure(space_id)?
            .model
            .set_icon(icon.as_deref())
            .map_err(map_model_error)?;
        self.persist_structure(space_id, Some(&[]), UpdateOrigin::Local)?;
        self.events.push(Event::SpacesChanged);
        self.space_info(space_id)
    }

    pub(crate) fn space_move(
        &mut self,
        space_id: SpaceId,
        index: u32,
    ) -> Result<Vec<SpaceInfo>, ApiError> {
        if !self.docs.structures.contains_key(&space_id) {
            return Err(ApiError::not_found("space not found"));
        }
        let account = self
            .docs
            .account
            .as_mut()
            .ok_or_else(|| ApiError::internal("account document missing"))?;
        account
            .model
            .move_space(space_id, index as usize)
            .map_err(map_model_error)?;
        self.persist_account()?;
        self.events.push(Event::SpacesChanged);
        self.space_list()
    }

    pub(crate) fn space_delete(&mut self, space_id: SpaceId) -> Result<(), ApiError> {
        let open = self.structure(space_id)?;
        // A shared space is left or closed (an archive is removed from this device with
        // `SpaceLeave`), never deleted (ADR-0047).
        if self.is_shared_space(space_id) {
            return Err(ApiError::invalid(
                "spaceId",
                "leave or close a shared space instead of deleting it",
            ));
        }
        // A personal space goes with all its pages, live and trashed, for good: the same clean-up
        // as a purge from the trash (view links, history, index rows, comments, attachment refs).
        let doc_id = open.doc_id;
        let pages = open.model.all_page_ids();
        if !pages.is_empty() {
            self.view_links_after_purge(space_id, &pages);
            self.history_after_purge(&pages)?;
            self.text_after_remove(&pages)?;
        }
        let now = self.clock.now_ms();
        // The space's attachments go with it (keys shredded, chunks deleted).
        self.purge_space_attachments(space_id)?;

        let account = self
            .docs
            .account
            .as_mut()
            .ok_or_else(|| ApiError::internal("account document missing"))?;
        account
            .model
            .remove_space(space_id)
            .map_err(map_model_error)?;
        let account_blob = account
            .model
            .doc()
            .export_updates_since(Some(&account.last_persisted))
            .map_err(map_doc_error)?;
        let account_doc_id = account.doc_id;

        self.db
            .transaction(|tx| {
                tx.pages_index_clear_space(space_id)?;
                tx.page_properties_clear_space(space_id)?;
                tx.icon_refs_replace_space(space_id, &[])?;
                for page in &pages {
                    tx.doc_delete(DocId::for_page(*page))?;
                }
                tx.doc_delete(doc_id)?;
                if let Some(blob) = &account_blob {
                    tx.update_append(account_doc_id, blob, UpdateOrigin::Local, now)?;
                }
                Ok(())
            })
            .map_err(map_storage_error)?;
        account.last_persisted = account.model.doc().version();
        self.docs.structures.remove(&space_id);
        self.keyring_forget_space(space_id)?;
        self.events.push(Event::SpacesChanged);
        Ok(())
    }

    // ----- pages --------------------------------------------------------------------

    pub(crate) fn tree_get(&self, space_id: SpaceId) -> Result<TreeView, ApiError> {
        let open = self.structure(space_id)?;
        // A shared space's favourites are this account's own (ADR-0047): the account document keeps
        // them, and the live pages among them show.
        let favorites = if self.is_shared_space(space_id) {
            self.docs
                .account
                .as_ref()
                .map(|account| account.model.favorites(space_id))
                .unwrap_or_default()
                .into_iter()
                .filter(|page| {
                    open.model
                        .page(*page)
                        .is_some_and(|info| info.trashed_at.is_none())
                })
                .collect()
        } else {
            open.model.favorites()
        };
        Ok(TreeView {
            space_id,
            roots: open.model.live_tree(),
            trashed: open.model.trashed_pages(),
            undo: UndoState {
                can_undo: open.model.can_undo(),
                can_redo: open.model.can_redo(),
            },
            favorites,
        })
    }

    pub(crate) fn page_get(&self, page_id: PageId) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.structure(space_id)?
            .model
            .page(page_id)
            .ok_or_else(|| ApiError::not_found("page not found"))
    }

    pub(crate) fn page_create(
        &mut self,
        space_id: SpaceId,
        parent_id: Option<PageId>,
        index: Option<u32>,
        title: &str,
    ) -> Result<PageInfo, ApiError> {
        let title = validate::page_title(title)?;
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let page_id = PageId::generate(self.rng.as_ref());
        self.mutate_structure(space_id, |model, now| {
            model.create_page(page_id, parent_id, index.map(|i| i as usize), &title, now)
        })?;
        self.text_after_create(page_id, &title)?;
        // The content document and its key record exist from the start, so every
        // device opening the page later finds one key (lazy creation on two devices
        // would race for the document key).
        self.load_or_create_page(page_id, space_id)?;
        self.page_get(page_id)
    }

    pub(crate) fn page_rename(
        &mut self,
        page_id: PageId,
        title: &str,
    ) -> Result<PageInfo, ApiError> {
        let title = validate::page_title(title)?;
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        self.mutate_structure(space_id, |model, now| {
            model.rename_page(page_id, &title, now)
        })?;
        self.text_after_rename(page_id, &title)?;
        self.page_get(page_id)
    }

    pub(crate) fn page_set_icon(
        &mut self,
        page_id: PageId,
        icon: Option<&str>,
    ) -> Result<PageInfo, ApiError> {
        let icon = validate::icon(icon)?;
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        // An uploaded image must be an image attachment of this page's space (ADR-0029).
        self.check_page_icon_attachment(space_id, icon.as_deref())?;
        self.mutate_structure(space_id, |model, now| {
            model.set_page_icon(page_id, icon.as_deref(), now)
        })?;
        self.page_get(page_id)
    }

    /// Sets, repositions or clears the cover of a page (ADR-0040; see `Request::PageSetCover`).
    pub(crate) fn page_set_cover(
        &mut self,
        page_id: PageId,
        cover: Option<&str>,
        cover_y: Option<u8>,
    ) -> Result<PageInfo, ApiError> {
        let cover = validate::cover(cover)?;
        let cover_y = validate::cover_y(cover_y)?;
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let current = self
            .structure(space_id)?
            .model
            .page(page_id)
            .ok_or_else(|| ApiError::not_found("page not found"))?;
        let (cover, cover_y) = match (cover, cover_y) {
            // Neither: clear the cover and its position.
            (None, None) => (None, None),
            // A position alone moves the image of the cover the page has.
            (None, Some(y)) => match current.cover.clone() {
                Some(existing) => (Some(existing), Some(y)),
                None => {
                    return Err(ApiError::invalid(
                        "coverY",
                        "the page has no cover to reposition",
                    ));
                }
            },
            // A cover, with its position (the default when none is given).
            (Some(cover), y) => (Some(cover), y),
        };
        // An uploaded image must be an image attachment of this page's space (ADR-0040). The cover the
        // page already has is not checked again: moving its image around names nothing new.
        if cover != current.cover {
            self.check_page_cover_attachment(space_id, cover.as_deref())?;
        }
        self.mutate_structure(space_id, |model, now| {
            model.set_page_cover(page_id, cover.as_deref(), cover_y, now)
        })?;
        self.page_get(page_id)
    }

    /// Sets or clears the font, the small text and the lock of a page (ADR-0040; see
    /// `Request::PageSetAppearance`). The lock is a guard the UI applies: nothing here or elsewhere in
    /// the core refuses a write to a locked page.
    pub(crate) fn page_set_appearance(
        &mut self,
        page_id: PageId,
        font: Option<PageFont>,
        small_text: Option<bool>,
        locked: Option<bool>,
    ) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        self.mutate_structure(space_id, |model, now| {
            model.set_page_appearance(page_id, font, small_text, locked, now)
        })?;
        self.page_get(page_id)
    }

    pub(crate) fn page_move(
        &mut self,
        page_id: PageId,
        parent_id: Option<PageId>,
        index: u32,
    ) -> Result<(), ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        if let Some(parent) = parent_id
            && self.space_of_page(parent)? != space_id
        {
            return Err(ApiError::invalid(
                "parentId",
                "the parent is in another space: use PageMoveToSpace",
            ));
        }
        let was_in_template = self.structure(space_id)?.model.in_template(page_id);
        let affected = self.mutate_structure(space_id, |model, now| {
            model.move_page(page_id, parent_id, index as usize, now)
        })?;
        // A move into or out of a template changes what the subtree is in (ADR-0056); its index rows
        // were rewritten with the move.
        let in_template = self.structure(space_id)?.model.in_template(page_id);
        if !affected.is_empty() && in_template != was_in_template {
            self.template_pages_moved(&affected, in_template)?;
        }
        Ok(())
    }

    /// Puts the live children of `parent_id` (the live top-level pages when `None`) into `order`,
    /// in one structural undo step (ADR-0052; see `Request::PageReorderChildren`). Persists one
    /// update and emits one `StructureChanged`; when the children already stand in that order
    /// nothing is written and nothing is emitted. No page is stamped as edited.
    pub(crate) fn page_reorder_children(
        &mut self,
        space_id: SpaceId,
        parent_id: Option<PageId>,
        order: &[PageId],
    ) -> Result<(), ApiError> {
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let moved = self
            .structure_mut(space_id)?
            .model
            .reorder_children(parent_id, order)
            .map_err(map_model_error)?;
        if moved.is_empty() {
            return Ok(());
        }
        // The positions are not in the page index rows (and no page was stamped), so no row of
        // the index changes: the update alone is persisted, as for a favourite.
        self.persist_structure(space_id, Some(&[]), UpdateOrigin::Local)?;
        self.events.push(Event::StructureChanged { space_id });
        Ok(())
    }

    pub(crate) fn page_trash(&mut self, page_id: PageId) -> Result<(), ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let affected =
            self.mutate_structure(space_id, |model, now| model.trash_page(page_id, now))?;
        self.backlinks_changed_for_sources(&affected)
    }

    pub(crate) fn page_restore(&mut self, page_id: PageId) -> Result<PageInfo, ApiError> {
        let space_id = self.space_of_page(page_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let affected =
            self.mutate_structure(space_id, |model, now| model.restore_page(page_id, now))?;
        self.backlinks_changed_for_sources(&affected)?;
        self.page_get(page_id)
    }

    pub(crate) fn page_purge(&mut self, page_id: PageId) -> Result<(), ApiError> {
        let space_id = self.space_of_page(page_id)?;
        // Tombstones and purge are Owner only in a shared space (ADR-0047).
        self.require_role(space_id, MemberRole::Owner)?;
        let affected =
            self.mutate_structure(space_id, |model, now| model.purge_page(page_id, now))?;
        // View links (M13 slice 4): the publication of a purged page goes with it.
        self.view_links_after_purge(space_id, &affected);
        // The history of the pages goes with them (ADR-0057): the journal is not derived data, so
        // nothing else would remove it before the documents are collected.
        self.history_after_purge(&affected)?;
        self.text_after_remove(&affected)
    }

    /// Adds a page to its space's favourites or removes it (ADR-0029). Adding needs a live page
    /// (`InvalidInput` on `pageId` for one in the trash); removing works for any known page.
    pub(crate) fn page_favorite_set(
        &mut self,
        page_id: PageId,
        favorite: bool,
    ) -> Result<(), ApiError> {
        let space_id = self.space_of_page(page_id)?;
        if self.is_shared_space(space_id) {
            if favorite {
                let live = self
                    .structure(space_id)?
                    .model
                    .page(page_id)
                    .ok_or_else(|| ApiError::not_found("page not found"))?
                    .trashed_at
                    .is_none();
                if !live {
                    return Err(map_model_error(ModelError::PageTrashed));
                }
            }
            return self.change_shared_favorites(space_id, |account| {
                account.set_favorite(space_id, page_id, favorite)
            });
        }
        self.change_favorites(space_id, |model| model.set_favorite(page_id, favorite))
    }

    /// Moves a favourite in front of `before` (another favourite of the same space) or last.
    pub(crate) fn page_favorite_move(
        &mut self,
        page_id: PageId,
        before: Option<PageId>,
    ) -> Result<(), ApiError> {
        let space_id = self.space_of_page(page_id)?;
        if self.is_shared_space(space_id) {
            return self.change_shared_favorites(space_id, |account| {
                account.move_favorite(space_id, page_id, before)
            });
        }
        self.change_favorites(space_id, |model| model.move_favorite(page_id, before))
    }

    /// A change of this account's favourites of a shared space (the account document, which no role
    /// restricts: every member, Viewers and archives included, keeps their own list).
    fn change_shared_favorites(
        &mut self,
        space_id: SpaceId,
        op: impl FnOnce(&AccountDoc) -> Result<bool, ModelError>,
    ) -> Result<(), ApiError> {
        self.ensure_account()?;
        let changed = match self.docs.account.as_ref() {
            Some(account) => op(&account.model).map_err(map_model_error)?,
            None => false,
        };
        if changed {
            self.persist_account()?;
        }
        self.events.push(Event::StructureChanged { space_id });
        Ok(())
    }

    /// A change of a space's favourites: applies `op`, persists the update when something was
    /// written (a favourite touches no page row of the index) and emits `StructureChanged`
    /// (also for an idempotent no-op, like every structure request). Favourites are not an undo
    /// step.
    fn change_favorites(
        &mut self,
        space_id: SpaceId,
        op: impl FnOnce(&mut StructureDoc) -> Result<bool, ModelError>,
    ) -> Result<(), ApiError> {
        let open = self.structure_mut(space_id)?;
        let changed = op(&mut open.model).map_err(map_model_error)?;
        if changed {
            self.persist_structure(space_id, Some(&[]), UpdateOrigin::Local)?;
        }
        self.events.push(Event::StructureChanged { space_id });
        Ok(())
    }

    pub(crate) fn structure_undo(
        &mut self,
        space_id: SpaceId,
        redo: bool,
    ) -> Result<TreeView, ApiError> {
        self.structure(space_id)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let open = self.structure_mut(space_id)?;
        let live_before = open.model.live_page_count();
        let ids_before: HashSet<PageId> = open.model.all_page_ids().into_iter().collect();
        let outcome = if redo {
            open.model.redo()
        } else {
            open.model.undo()
        }
        .map_err(map_model_error)?;
        if outcome.is_some() {
            let live_after = open.model.live_page_count();
            let ids_after: HashSet<PageId> = open.model.all_page_ids().into_iter().collect();
            // Undo can touch any page; re-sync the whole space index.
            self.persist_structure(space_id, None, UpdateOrigin::Core)?;
            self.text_after_undo(space_id, &ids_before, &ids_after)?;
            self.events.push(Event::StructureChanged { space_id });
            if live_after != live_before {
                self.events.push(Event::SpacesChanged);
            }
        }
        self.tree_get(space_id)
    }

    // ----- plumbing -----------------------------------------------------------------

    /// The open structure document of a space, for a read. Every way to the document tells it the
    /// time first ([`Self::formula_env`]), so the formulas of the page values it reads compute with
    /// the time of this request (`now()`, the local day) without any reader passing a clock.
    pub(crate) fn structure(&self, space_id: SpaceId) -> Result<&OpenStructure, ApiError> {
        let open = self
            .docs
            .structures
            .get(&space_id)
            .ok_or_else(|| ApiError::not_found("space not found"))?;
        open.model.set_formula_env(self.formula_env());
        // The members' names for `name(person)`: rebuilt only when the membership or a name changed.
        if open.model.formula_people_revision() != Some(self.shared.names_rev) {
            open.model
                .set_formula_people(self.shared.names_rev, self.formula_people(space_id));
        }
        Ok(open)
    }

    /// The members of a space and their names, for the formulas of its pages (`name(person)`). The core
    /// knows display names only: the e-mail of a member is empty. A former member is not listed.
    fn formula_people(&self, space_id: SpaceId) -> HashMap<AccountId, (String, String)> {
        self.space_people_names(space_id)
            .into_iter()
            .map(|(account, name)| (account, (name, String::new())))
            .collect()
    }

    /// The time formulas read: the clock's `now_ms` and the device's offset from UTC (ADR-0055).
    fn formula_env(&self) -> FormulaEnv {
        FormulaEnv {
            now_ms: self.clock.now_ms(),
            offset_seconds: self.clock.local_offset_seconds(),
        }
    }

    /// The open structure document of a space, for a local write: the edits are attributed to
    /// this device (`created_by`, `edited_by`, ADR-0026 amendment of 2026-10-08).
    pub(crate) fn structure_mut(
        &mut self,
        space_id: SpaceId,
    ) -> Result<&mut OpenStructure, ApiError> {
        let author = self.device.device_id;
        let env = self.formula_env();
        let revision = self.shared.names_rev;
        let people = self
            .docs
            .structures
            .get(&space_id)
            .is_some_and(|open| open.model.formula_people_revision() != Some(revision))
            .then(|| self.formula_people(space_id));
        let open = self
            .docs
            .structures
            .get_mut(&space_id)
            .ok_or_else(|| ApiError::not_found("space not found"))?;
        open.model.set_author(Some(author));
        open.model.set_formula_env(env);
        if let Some(people) = people {
            open.model.set_formula_people(revision, people);
        }
        Ok(open)
    }

    fn space_info(&self, space_id: SpaceId) -> Result<SpaceInfo, ApiError> {
        Ok(self.decorated_space(space_info_of(space_id, &self.structure(space_id)?.model)))
    }

    /// A local edit of a page's content document: stamps the page's last edit in the structure
    /// document (`updated_at` and `edited_by`), at most once per
    /// [`kb_model::EDIT_STAMP_INTERVAL_MS`] per device. A stamp is a structure change like any
    /// other: persisted with its page index row, announced with `StructureChanged`, not an undo step.
    pub(crate) fn page_content_edited(&mut self, page_id: PageId) -> Result<(), ApiError> {
        let space_id = self.space_of_page(page_id)?;
        let now = self.clock.now_ms();
        let open = self.structure_mut(space_id)?;
        let affected = open
            .model
            .touch_edited(page_id, now)
            .map_err(map_model_error)?;
        if affected.is_empty() {
            return Ok(());
        }
        self.persist_structure(space_id, Some(&affected), UpdateOrigin::Local)?;
        self.events.push(Event::StructureChanged { space_id });
        Ok(())
    }

    pub(crate) fn space_of_page(&self, page_id: PageId) -> Result<SpaceId, ApiError> {
        self.db
            .pages_index_get(page_id)
            .map_err(map_storage_error)?
            .map(|row| row.space_id)
            .ok_or_else(|| ApiError::not_found("page not found"))
    }

    /// Applies `op` to a structure document, persists the update with the index
    /// rows of the affected pages, emits `StructureChanged` and returns the affected
    /// page ids.
    pub(crate) fn mutate_structure(
        &mut self,
        space_id: SpaceId,
        op: impl FnOnce(&mut StructureDoc, i64) -> Result<Vec<PageId>, ModelError>,
    ) -> Result<Vec<PageId>, ApiError> {
        let now = self.clock.now_ms();
        let open = self.structure_mut(space_id)?;
        let live_before = open.model.live_page_count();
        let affected = op(&mut open.model, now).map_err(map_model_error)?;
        let live_after = open.model.live_page_count();
        self.persist_structure(space_id, Some(&affected), UpdateOrigin::Local)?;
        self.events.push(Event::StructureChanged { space_id });
        if live_after != live_before {
            // `SpaceInfo::page_count` changed.
            self.events.push(Event::SpacesChanged);
        }
        // View links (M13 slice 4): a title, icon or cover edit changes a publication too. Best effort.
        let _ = self.view_links_pages_changed(space_id, &affected);
        Ok(affected)
    }

    /// Appends the pending update of a structure document and refreshes the index
    /// rows of `affected` (`None` = every page of the space) in one transaction,
    /// then compacts if the policy says so. The derived property rows are left alone
    /// (except the rows of pages that left the document); see
    /// [`Self::persist_structure_with`].
    pub(crate) fn persist_structure(
        &mut self,
        space_id: SpaceId,
        affected: Option<&[PageId]>,
        origin: UpdateOrigin,
    ) -> Result<(), ApiError> {
        self.persist_structure_with(space_id, affected, origin, PropsScope::Nothing)
    }

    /// [`Self::persist_structure`] that also rewrites the derived property rows (`page_properties`
    /// and the property rows of the text index, ADR-0026) of `props` in the same transaction.
    pub(crate) fn persist_structure_with(
        &mut self,
        space_id: SpaceId,
        affected: Option<&[PageId]>,
        origin: UpdateOrigin,
        props: PropsScope<'_>,
    ) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        // The names the text rows of Person values are made of (read before the document is borrowed).
        let people = self.space_people_names(space_id);
        let open = self
            .docs
            .structures
            .get_mut(&space_id)
            .ok_or_else(|| ApiError::not_found("space not found"))?;
        let blob = open
            .model
            .doc()
            .export_updates_since(Some(&open.last_persisted))
            .map_err(map_doc_error)?;
        let (rows, gone, clear_all, affected) = match affected {
            Some(ids) => {
                let mut rows = Vec::with_capacity(ids.len());
                let mut gone = Vec::new();
                let mut kept = Vec::with_capacity(ids.len());
                for id in ids {
                    match open.model.page(*id) {
                        Some(info) => {
                            rows.push(row_from_info(&info, open.model.in_template(*id)));
                            kept.push(*id);
                        }
                        None => {
                            // A page that left this document for another space (it moved,
                            // ADR-0038) keeps the rows of the space it lives in now.
                            let elsewhere = self
                                .db
                                .pages_index_get(*id)
                                .map_err(map_storage_error)?
                                .is_some_and(|row| row.space_id != space_id);
                            if !elsewhere {
                                gone.push(*id);
                                kept.push(*id);
                            }
                        }
                    }
                }
                (rows, gone, false, Some(kept))
            }
            None => (all_rows(&open.model), Vec::new(), true, None),
        };
        let doc_id = open.doc_id;
        let mut writes = crate::properties::property_writes(&open.model, props, &people);
        writes
            .keep_rows_of_moved_pages(&self.db, space_id)
            .map_err(map_storage_error)?;
        // The files the property rows attach: those about to be replaced or removed (with the
        // pages that leave the document) and those the new rows attach. Their references may
        // change, so the retention bookkeeping looks at them once the transaction is in.
        let mut attachments = writes.attachments();
        attachments.extend(
            writes
                .replaced_attachments(&self.db, space_id, &gone)
                .map_err(map_storage_error)?,
        );
        // The image icons of the pages (ADR-0029, `icons.rs`): the rows of the affected pages, or of
        // the whole space, are rewritten in the same transaction; the attachments the icons held
        // before and hold after go through the same bookkeeping.
        let icons = crate::icons::IconWrites::of(&open.model, affected.as_deref());
        attachments.extend(
            icons
                .touched(&self.db, space_id)
                .map_err(map_storage_error)?,
        );

        let stats = self
            .db
            .transaction(|tx| {
                if let Some(blob) = &blob {
                    tx.update_append(doc_id, blob, origin, now)?;
                }
                if clear_all {
                    tx.pages_index_clear_space(space_id)?;
                }
                if !rows.is_empty() {
                    tx.pages_index_upsert(&rows)?;
                }
                if !gone.is_empty() {
                    tx.pages_index_delete(&gone)?;
                    tx.page_properties_delete_pages(&gone)?;
                }
                crate::properties::write_property_rows(tx, space_id, &writes)?;
                icons.write(tx, space_id)?;
                tx.update_stats(doc_id)
            })
            .map_err(map_storage_error)?;
        open.last_persisted = open.model.doc().version();
        if origin != UpdateOrigin::Remote {
            self.sync_note_local_change();
        }
        if !attachments.is_empty() {
            self.attachment_refs_changed(attachments, now)?;
        }
        self.maybe_compact(DocRef::Structure(space_id), doc_id, stats)
    }

    pub(crate) fn persist_account(&mut self) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let account = self
            .docs
            .account
            .as_mut()
            .ok_or_else(|| ApiError::internal("account document missing"))?;
        let blob = account
            .model
            .doc()
            .export_updates_since(Some(&account.last_persisted))
            .map_err(map_doc_error)?;
        let doc_id = account.doc_id;
        let stats = self
            .db
            .transaction(|tx| {
                if let Some(blob) = &blob {
                    tx.update_append(doc_id, blob, UpdateOrigin::Local, now)?;
                }
                tx.update_stats(doc_id)
            })
            .map_err(map_storage_error)?;
        account.last_persisted = account.model.doc().version();
        self.sync_note_local_change();
        self.maybe_compact(DocRef::Account, doc_id, stats)
    }

    pub(crate) fn maybe_compact(
        &mut self,
        which: DocRef,
        doc_id: DocId,
        stats: UpdateStats,
    ) -> Result<(), ApiError> {
        if stats.count <= self.policy.max_updates && stats.bytes <= self.policy.max_bytes {
            return Ok(());
        }
        let Some(last_seq) = stats.last_seq else {
            return Ok(());
        };
        // Sync guard: updates the server has not received stay in the log (the
        // outbox reads them); only the pushed prefix is folded into the snapshot.
        // The newest row always stays: `local_seq` is one more than the highest row,
        // so an emptied log would restart at 1 and hide new updates behind the sync
        // marker `pushed_up_to_seq`.
        let cut = match self.sync_pushed_up_to(doc_id)? {
            Some(pushed) => pushed.min(last_seq - 1),
            None => last_seq - 1,
        };
        let first_seq = last_seq - i64::try_from(stats.count).unwrap_or(0) + 1;
        if cut < first_seq {
            return Ok(());
        }
        let (snapshot, version) = match which {
            DocRef::Account => {
                let doc = self
                    .docs
                    .account
                    .as_ref()
                    .map(|a| a.model.doc())
                    .ok_or_else(|| ApiError::internal("account document missing"))?;
                (doc.export_snapshot().map_err(map_doc_error)?, doc.version())
            }
            DocRef::Structure(space_id) => {
                let doc = self.structure(space_id)?.model.doc();
                (doc.export_snapshot().map_err(map_doc_error)?, doc.version())
            }
            DocRef::Page(page_id) => {
                let doc = self
                    .docs
                    .pages
                    .get(&page_id)
                    .map(|p| p.model.doc())
                    .ok_or_else(|| ApiError::internal("page document missing"))?;
                (doc.export_snapshot().map_err(map_doc_error)?, doc.version())
            }
        };
        let now = self.clock.now_ms();
        self.db
            .transaction(|tx| {
                tx.snapshot_put(doc_id, &version.0, &snapshot, now)?;
                tx.updates_delete_up_to(doc_id, cut)?;
                Ok::<(), StorageError>(())
            })
            .map_err(map_storage_error)?;
        tracing::info!(doc_id = %short_id(&doc_id), updates = stats.count, bytes = stats.bytes, "compacted document");
        Ok(())
    }

    /// Dispatch entry for the Milestone 2 requests.
    pub(crate) fn dispatch_structure(
        &mut self,
        request: kb_types::Request,
    ) -> Result<Response, ApiError> {
        use kb_types::Request as R;
        match request {
            R::SpaceList => Ok(Response::Spaces(self.space_list()?)),
            R::SpaceCreate { name, icon } => {
                Ok(Response::Space(self.space_create(&name, icon.as_deref())?))
            }
            R::SpaceRename { space_id, name } => {
                Ok(Response::Space(self.space_rename(space_id, &name)?))
            }
            R::SpaceSetIcon { space_id, icon } => Ok(Response::Space(
                self.space_set_icon(space_id, icon.as_deref())?,
            )),
            R::SpaceMove { space_id, index } => {
                Ok(Response::Spaces(self.space_move(space_id, index)?))
            }
            R::SpaceDelete { space_id } => {
                self.space_delete(space_id)?;
                Ok(Response::Ok)
            }
            R::TreeGet { space_id } => Ok(Response::Tree(self.tree_get(space_id)?)),
            R::PageGet { page_id } => Ok(Response::Page(self.page_get(page_id)?)),
            R::PageCreate {
                space_id,
                parent_id,
                index,
                title,
            } => Ok(Response::Page(
                self.page_create(space_id, parent_id, index, &title)?,
            )),
            R::PageRename { page_id, title } => {
                Ok(Response::Page(self.page_rename(page_id, &title)?))
            }
            R::PageSetIcon { page_id, icon } => Ok(Response::Page(
                self.page_set_icon(page_id, icon.as_deref())?,
            )),
            R::PageSetCover {
                page_id,
                cover,
                cover_y,
            } => Ok(Response::Page(self.page_set_cover(
                page_id,
                cover.as_deref(),
                cover_y,
            )?)),
            R::PageSetAppearance {
                page_id,
                font,
                small_text,
                locked,
            } => Ok(Response::Page(
                self.page_set_appearance(page_id, font, small_text, locked)?,
            )),
            R::PageMove {
                page_id,
                parent_id,
                index,
            } => {
                self.page_move(page_id, parent_id, index)?;
                Ok(Response::Ok)
            }
            R::PageReorderChildren {
                space_id,
                parent_id,
                order,
            } => {
                self.page_reorder_children(space_id, parent_id, &order)?;
                Ok(Response::Ok)
            }
            R::PageDuplicate { page_id } => Ok(Response::Page(self.page_duplicate(page_id)?)),
            R::PageBulk {
                space_id,
                page_ids,
                action,
            } => Ok(Response::PageBulk(
                self.page_bulk(space_id, &page_ids, action)?,
            )),
            R::PageSetTemplate { page_id, template } => {
                Ok(Response::Page(self.page_set_template(page_id, template)?))
            }
            R::PageSetTemplatePlaceholder {
                page_id,
                property_id,
                placeholder,
            } => Ok(Response::Page(self.page_set_template_placeholder(
                page_id,
                property_id,
                placeholder,
            )?)),
            R::PageCreateFromTemplate {
                template_id,
                parent_id,
                index,
                values,
            } => Ok(Response::Page(self.page_create_from_template(
                template_id,
                parent_id,
                index,
                values,
            )?)),
            R::PageTrash { page_id } => {
                self.page_trash(page_id)?;
                Ok(Response::Ok)
            }
            R::PageRestore { page_id } => Ok(Response::Page(self.page_restore(page_id)?)),
            R::PagePurge { page_id } => {
                self.page_purge(page_id)?;
                Ok(Response::Ok)
            }
            R::PageFavoriteSet { page_id, favorite } => {
                self.page_favorite_set(page_id, favorite)?;
                Ok(Response::Ok)
            }
            R::PageFavoriteMove { page_id, before } => {
                self.page_favorite_move(page_id, before)?;
                Ok(Response::Ok)
            }
            R::StructureUndo { space_id } => {
                Ok(Response::Tree(self.structure_undo(space_id, false)?))
            }
            R::StructureRedo { space_id } => {
                Ok(Response::Tree(self.structure_undo(space_id, true)?))
            }
            other => Err(ApiError::internal(format!(
                "request {} is not a structure request",
                other.kind()
            ))),
        }
    }
}

fn space_info_of(space_id: SpaceId, model: &StructureDoc) -> SpaceInfo {
    let meta = model.space();
    SpaceInfo {
        id: space_id,
        name: meta.name,
        icon: meta.icon,
        created_at: meta.created_at,
        page_count: model.live_page_count(),
        // M13: a personal space until the sharing code (kb-core `sharing.rs`) says otherwise.
        share_state: SpaceShareState::Personal,
        role: None,
        archive_reason: None,
        member_count: 0,
        ai_enabled: true,
    }
}
