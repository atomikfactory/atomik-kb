//! Moving a page and its subtree to another space (ADR-0038, `docs/architecture/space-move.md`).
//!
//! The page ids are kept. The source structure keeps a residue node per page (`kb_model`
//! `StructureDoc::move_out`), the target gets fresh nodes (`move_in`) with the values mapped by
//! property name; every moved document gets a new `Doc` key record under the target space while
//! the source's records stay readable; attachments used only by the moved pages are re-wrapped
//! under the target and their manifests moved. Remote moves are settled by [`Core::reconcile_moves`]
//! (residue is never a purge; a page live in two spaces keeps its latest arrival).
//!
//! Order of a local move, the first fallible step first so that an error leaves nothing half done:
//! validation and the plan (reads only), the key records (they only add readability), the target
//! schema changes, the target structure (`move_in`, persisted), the source structure (`move_out`,
//! persisted: a crash in between leaves the page live in both spaces, which the next open settles
//! by the same rule as a concurrent move), the relations left behind, the views, the attachments.

mod mapping;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use kb_keys::KeyringItem;
use kb_keys::keyring as kr;
use kb_model::{MovedPage, PageDoc, StructureDoc};
use kb_storage::{
    AttachmentQueries, ChunkSyncQueries, DocKindCode, DocQueries, SecurityQueries, UpdateOrigin,
};
use kb_types::{
    ApiError, AttachmentId, BlockId, ChangeSource, DocId, Event, MemberRole, PageId, PropertyId,
    PropertyKind, PropertyValue, Request, Response, SpaceId, SpaceMoveSummary,
};
use serde_json::{Map, Value};

use self::mapping::{
    Needs, SchemaMapping, SchemaOp, keep_members, map_placeholders, map_values, plan_schema,
    remap_view,
};
use crate::documents::{load_doc, map_model_error};
use crate::properties::PropsScope;
use crate::security::keyring_row;
use crate::{Core, map_storage_error};

/// Most pages one move may carry.
pub const MAX_MOVED_PAGES: usize = 10_000;

/// A `pageList` block of a moved page: its page, its id and its attributes.
type ViewBlock = (BlockId, Map<String, Value>);

/// Everything a move computed before it writes anything (the plan request stops here).
struct MoveCalc {
    source: SpaceId,
    target: SpaceId,
    /// The pages in pre-order, their stored values already mapped to the target schema.
    pages: Vec<MovedPage>,
    ids: Vec<PageId>,
    mapping: SchemaMapping,
    /// The `pageList` blocks of the moved pages that have a document here.
    views: Vec<(PageId, Vec<ViewBlock>)>,
    /// Pages that stay and store a relation that lists a moved page: (page, source property).
    relation_cleanup: Vec<(PageId, PropertyId)>,
    /// Attachments the moved pages use that do not move (pages that stay use them too, or they
    /// belong to a third space): a move that touches a shared space takes copies of them.
    keep_using: Vec<AttachmentId>,
    /// Attachments that move with the pages.
    attachments_moving: Vec<AttachmentId>,
    summary: SpaceMoveSummary,
}

impl Core {
    /// Dispatch entry for `PageMoveToSpacePlan` and `PageMoveToSpace`.
    pub(crate) fn dispatch_space_move(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::PageMoveToSpacePlan {
                page_id,
                space_id,
                parent_id,
            } => Ok(Response::SpaceMovePlanned(
                self.page_move_to_space_plan(page_id, space_id, parent_id)?,
            )),
            Request::PageMoveToSpace {
                page_id,
                space_id,
                parent_id,
                index,
            } => Ok(Response::PagesMovedToSpace(
                self.page_move_to_space(page_id, space_id, parent_id, index)?,
            )),
            other => Err(ApiError::internal(format!(
                "request {} is not a space move request",
                other.kind()
            ))),
        }
    }

    /// What `PageMoveToSpace` would do; writes nothing.
    pub(crate) fn page_move_to_space_plan(
        &mut self,
        page_id: PageId,
        space_id: SpaceId,
        parent_id: Option<PageId>,
    ) -> Result<SpaceMoveSummary, ApiError> {
        Ok(self.prepare_move(page_id, space_id, parent_id)?.summary)
    }

    /// Moves a live page and its live subtree to `space_id` under `parent_id` at `index`.
    pub(crate) fn page_move_to_space(
        &mut self,
        page_id: PageId,
        space_id: SpaceId,
        parent_id: Option<PageId>,
        index: Option<u32>,
    ) -> Result<SpaceMoveSummary, ApiError> {
        // Out of a shared space is an Owner's; into one needs an Editor (ADR-0047). The plan only
        // reads.
        let source = self.space_of_page(page_id)?;
        self.structure(space_id)?;
        self.require_role(source, MemberRole::Owner)?;
        self.require_role(space_id, MemberRole::Editor)?;
        let calc = self.prepare_move(page_id, space_id, parent_id)?;
        let now = self.clock.now_ms();
        let MoveCalc {
            source,
            target,
            mut pages,
            ids,
            mapping,
            views,
            relation_cleanup,
            attachments_moving,
            keep_using,
            summary,
        } = calc;

        // 1. The key records: a new `Doc` record under the target for every moved document that
        //    exists, and the document's row points at the target. The source's records stay.
        let doc_ids = self.move_doc_keys(target, &ids)?;

        // 1b. A move that touches a shared space cannot leave the pages reading files of another
        //     space across the line (the members cannot): the files the pages keep using elsewhere
        //     are copied under fresh keys into the target and the pages name the copies.
        if !keep_using.is_empty() && (self.is_shared_space(source) || self.is_shared_space(target))
        {
            let copies = self.copy_attachments_for_move(target, &keep_using, now)?;
            self.rewrite_attachment_refs(source, &mut pages, &ids, &copies)?;
        }

        // 2. The target structure: the schema it needs, then the pages (the first under
        //    `parent_id` at `index`).
        {
            let open = self.structure_mut(target)?;
            apply_schema_ops(&mut open.model, &mapping.ops, now)?;
            open.model
                .move_in(&pages, parent_id, index.map(|i| i as usize), source, now)
                .map_err(map_model_error)?;
        }
        self.persist_structure_with(
            target,
            Some(&ids),
            UpdateOrigin::Local,
            PropsScope::Pages(&ids),
        )?;

        // 3. The source structure: the pages become residue.
        self.structure_mut(source)?
            .model
            .move_out(page_id, target, now)
            .map_err(map_model_error)?;
        self.persist_structure_with(source, Some(&ids), UpdateOrigin::Local, PropsScope::Nothing)?;

        // 4. Relations of pages that stay lose the entries that list a moved page.
        self.drop_relation_links(source, &relation_cleanup, now)?;

        // 5. The documents are snapshotted under the new key at the next sync; the views inside
        //    them follow the property mapping.
        self.sync_mark_for_snapshot(&doc_ids)?;
        self.remap_views(&views, &mapping)?;

        // 6. Attachments only the moved pages use follow them.
        self.move_attachments(source, target, &attachments_moving, now)?;

        // 7. Events.
        if !mapping.ops.is_empty() {
            self.events
                .push(Event::PropertySchemaChanged { space_id: target });
        }
        self.events
            .push(Event::StructureChanged { space_id: source });
        self.events
            .push(Event::StructureChanged { space_id: target });
        self.events.push(Event::SpacesChanged);
        self.events.push(Event::PagesMovedSpace {
            from_space_id: source,
            to_space_id: target,
            page_ids: ids,
        });
        tracing::info!(
            pages = summary.page_ids.len(),
            attachments = summary.attachments_moved,
            "pages moved to another space"
        );
        // A move can leave a page in two spaces only through a crash; settling is cheap.
        self.reconcile_moves()?;
        Ok(summary)
    }

    // ----- planning -------------------------------------------------------------------------

    /// Validates the request and works out everything the move needs, without writing: the
    /// pages with their values mapped to the target schema, the schema changes, the relations
    /// and attachments involved, and the summary.
    fn prepare_move(
        &mut self,
        page_id: PageId,
        space_id: SpaceId,
        parent_id: Option<PageId>,
    ) -> Result<MoveCalc, ApiError> {
        let source = self.space_of_page(page_id)?;
        if space_id == source {
            return Err(ApiError::invalid(
                "spaceId",
                "the page is already in this space (use PageMove)",
            ));
        }
        self.structure(space_id)?;
        // The people a moved Person value may keep: the members of the target space.
        let target_members = self.space_member_accounts(space_id)?;
        let source_model = &self.structure(source)?.model;
        let target_model = &self.structure(space_id)?.model;
        let info = source_model
            .page(page_id)
            .ok_or_else(|| ApiError::not_found("page not found"))?;
        if info.trashed_at.is_some() {
            return Err(ApiError::invalid("pageId", "the page is in the trash"));
        }
        if let Some(parent) = parent_id {
            let live = self
                .space_of_page(parent)
                .is_ok_and(|space| space == space_id)
                && target_model
                    .page(parent)
                    .is_some_and(|parent| parent.trashed_at.is_none());
            if !live {
                return Err(ApiError::invalid(
                    "parentId",
                    "the parent must be a live page of the target space",
                ));
            }
        }
        let mut pages = source_model.moved_pages(page_id).map_err(map_model_error)?;
        if pages.len() > MAX_MOVED_PAGES {
            return Err(ApiError::invalid(
                "pageId",
                format!("a move carries at most {MAX_MOVED_PAGES} pages"),
            ));
        }
        if pages
            .iter()
            .any(|page| target_model.page(page.page_id).is_some())
        {
            return Err(ApiError::invalid(
                "pageId",
                "a page of the subtree already exists in the target space",
            ));
        }
        let ids: Vec<PageId> = pages.iter().map(|page| page.page_id).collect();
        let moved: HashSet<PageId> = ids.iter().copied().collect();
        let source_schema = source_model.property_schema();
        let target_schema = target_model.property_schema();

        // What the schema has to provide: the values of the pages, and what their views name.
        let mut needs = Needs::default();
        for page in &pages {
            needs.add_values(&source_schema, &page.props);
            needs.add_placeholders(&source_schema, &page.template_values);
        }
        let mut views: Vec<(PageId, Vec<ViewBlock>)> = Vec::new();
        for id in &ids {
            let blocks = self.page_view_blocks(*id)?;
            for (_, attrs) in &blocks {
                needs.add_view(attrs);
            }
            if !blocks.is_empty() {
                views.push((*id, blocks));
            }
        }
        // A formula a view names needs the properties it reads too (ADR-0055).
        needs.add_formula_reads(&source_schema);
        let mapping = plan_schema(&source_schema, &target_schema, &needs, self.rng.as_ref());

        let mut values_dropped = 0;
        for page in &mut pages {
            let outcome = map_values(&mapping, &source_schema, &moved, &page.props);
            values_dropped += outcome.dropped;
            let (props, dropped) = keep_members(outcome.props, &mapping.target, &target_members);
            values_dropped += dropped;
            page.props = props;
            // The placeholders follow the property mapping (ADR-0056).
            page.template_values = map_placeholders(&mapping, &page.template_values);
        }

        // Relations: entries of moved pages that list pages which stay, and entries of pages
        // that stay that list moved pages, are removed.
        let mut relation_links_removed = 0u32;
        let mut relation_cleanup: Vec<(PageId, PropertyId)> = Vec::new();
        for (page, entry) in self.structure(source)?.model.property_index_all() {
            let is_moved = moved.contains(&page);
            for row in &entry.rows {
                let Some(related) = row.related_page_id else {
                    continue;
                };
                if is_moved != moved.contains(&related) {
                    relation_links_removed += 1;
                    if !is_moved && !relation_cleanup.contains(&(page, row.property_id)) {
                        relation_cleanup.push((page, row.property_id));
                    }
                }
            }
        }

        let (attachments_moving, attachments_left, keep_using) =
            self.plan_attachments(source, &ids, &moved)?;

        let summary = SpaceMoveSummary {
            page_ids: ids.clone(),
            attachments_moved: u32::try_from(attachments_moving.len()).unwrap_or(u32::MAX),
            attachments_left: u32::try_from(attachments_left).unwrap_or(u32::MAX),
            properties_created: mapping.created,
            values_dropped,
            relation_links_removed,
        };
        Ok(MoveCalc {
            source,
            target: space_id,
            pages,
            ids,
            mapping,
            views,
            relation_cleanup,
            attachments_moving,
            keep_using,
            summary,
        })
    }

    /// The attachments the moved pages use whose every reference is a moved page (they move) and
    /// how many others of the source space they use that other pages use too (they stay).
    fn plan_attachments(
        &self,
        source: SpaceId,
        ids: &[PageId],
        moved: &HashSet<PageId>,
    ) -> Result<(Vec<AttachmentId>, usize, Vec<AttachmentId>), ApiError> {
        let mut used: BTreeSet<AttachmentId> = BTreeSet::new();
        for page in ids {
            used.extend(
                self.db
                    .attachment_ids_referenced_by(*page)
                    .map_err(map_storage_error)?,
            );
        }
        let mut moving = Vec::new();
        let mut left = 0usize;
        let mut kept: Vec<AttachmentId> = Vec::new();
        for attachment in used {
            // Only an attachment of the source space can move; one of a third space is read
            // across spaces already.
            let in_source = self
                .db
                .attachment_get(attachment)
                .map_err(map_storage_error)?
                .is_some_and(|row| row.space_id == source)
                && self
                    .structure(source)?
                    .model
                    .attachment(attachment)
                    .is_some();
            if !in_source {
                kept.push(attachment);
                continue;
            }
            let users = self
                .db
                .attachment_referencing_pages(attachment)
                .map_err(map_storage_error)?;
            if users.iter().all(|user| moved.contains(user)) {
                moving.push(attachment);
            } else {
                left += 1;
                kept.push(attachment);
            }
        }
        Ok((moving, left, kept))
    }

    // ----- keys -------------------------------------------------------------------------------

    /// Writes the `Doc` record of every moved document under `target` and points the document
    /// rows there, in one transaction. Answers the documents that exist (a page that never had
    /// its content opened has none yet).
    fn move_doc_keys(&mut self, target: SpaceId, pages: &[PageId]) -> Result<Vec<DocId>, ApiError> {
        let mut docs: Vec<DocId> = Vec::new();
        for page in pages {
            let doc_id = DocId::for_page(*page);
            if self
                .db
                .doc_get(doc_id)
                .map_err(map_storage_error)?
                .is_some()
            {
                docs.push(doc_id);
            }
        }
        // The comment stream of a page moves with it (ADR-0048): its own `Doc` record under the
        // target, its row pointed there, and (below) its records and words.
        docs.extend(self.comment_streams_of_pages(pages)?);
        let mut items: Vec<KeyringItem> = Vec::new();
        for doc_id in &docs {
            if let Some(item) = self.doc_key_item_for_move(target, *doc_id)? {
                items.push(item);
            }
        }
        let rows: Vec<kb_storage::KeyringRow> = items.iter().map(keyring_row).collect();
        self.db
            .transaction(|tx| {
                for row in &rows {
                    tx.keyring_put(row)?;
                }
                for doc_id in &docs {
                    tx.doc_set_space(*doc_id, target)?;
                }
                Ok(())
            })
            .map_err(map_storage_error)?;
        self.keyring_merge(items);
        self.comments_after_move(pages, target)?;
        Ok(docs)
    }

    // ----- relations --------------------------------------------------------------------------

    /// Removes the entries that list a moved page from the relations of pages that stay (the
    /// moved pages are residue by now, so the pages read their relations without them already:
    /// this rewrites what they store).
    fn drop_relation_links(
        &mut self,
        source: SpaceId,
        cleanup: &[(PageId, PropertyId)],
        now: i64,
    ) -> Result<(), ApiError> {
        if cleanup.is_empty() {
            return Ok(());
        }
        let mut changed: Vec<PageId> = Vec::new();
        for (page, property) in cleanup {
            let open = self.structure_mut(source)?;
            let current: Vec<PageId> = open
                .model
                .page_properties(*page)
                .map_err(map_model_error)?
                .into_iter()
                .find_map(|(id, value)| match value {
                    PropertyValue::Relation(pages) if id == *property => Some(pages),
                    _ => None,
                })
                .unwrap_or_default();
            let value = if current.is_empty() {
                None
            } else {
                Some(PropertyValue::Relation(current))
            };
            let written = open
                .model
                .set_page_property(*page, *property, value, now)
                .map_err(map_model_error)?;
            for page in written {
                if !changed.contains(&page) {
                    changed.push(page);
                }
            }
        }
        if !changed.is_empty() {
            self.persist_structure_with(
                source,
                Some(&changed),
                UpdateOrigin::Local,
                PropsScope::Pages(&changed),
            )?;
        }
        Ok(())
    }

    // ----- views ------------------------------------------------------------------------------

    /// The `pageList` blocks of a page's document (none when the page has no document yet).
    fn page_view_blocks(&mut self, page_id: PageId) -> Result<Vec<ViewBlock>, ApiError> {
        if let Some(open) = self.docs.pages.get(&page_id) {
            return Ok(open.model.page_lists());
        }
        let Some(row) = self
            .db
            .doc_get(DocId::for_page(page_id))
            .map_err(map_storage_error)?
        else {
            return Ok(Vec::new());
        };
        let doc = load_doc(&self.db, &row, kb_doc::DocKind::Page, self.rng.as_ref())?;
        let model = PageDoc::from_doc(doc).map_err(map_model_error)?;
        Ok(model.page_lists())
    }

    /// Rewrites the schema ids of the views of the moved pages through the property mapping,
    /// one local edit per document that changes.
    fn remap_views(
        &mut self,
        views: &[(PageId, Vec<ViewBlock>)],
        mapping: &SchemaMapping,
    ) -> Result<(), ApiError> {
        for (page_id, blocks) in views {
            let was_open = self.docs.pages.contains_key(page_id);
            let mut changed = false;
            {
                let open = self.open_page(*page_id)?;
                for (block, attrs) in blocks {
                    let changes = remap_view(attrs, mapping);
                    if changes.is_empty() {
                        continue;
                    }
                    changed |= open
                        .model
                        .set_page_list_attrs(*block, &changes)
                        .map_err(map_model_error)?;
                }
            }
            if changed {
                self.persist_page(*page_id, None, UpdateOrigin::Local, ChangeSource::Core)?;
            }
            if !was_open {
                self.docs.pages.remove(page_id);
            }
        }
        Ok(())
    }

    // ----- attachments ------------------------------------------------------------------------

    /// Moves attachments to `target`: a record that wraps the same attachment key under the
    /// target's space key, the manifest in the target structure before the source gives it up
    /// (so no chunk is ever unclaimed), the index row's space. The chunks and their transfer
    /// rows stay as they are.
    fn move_attachments(
        &mut self,
        source: SpaceId,
        target: SpaceId,
        attachments: &[AttachmentId],
        now: i64,
    ) -> Result<(), ApiError> {
        if attachments.is_empty() {
            return Ok(());
        }
        let (target_key, target_epoch) = self.attachment_wrap_key(target)?;
        let mut moved: Vec<AttachmentId> = Vec::new();
        for id in attachments {
            let Some(manifest) = self.structure(source)?.model.attachment(*id) else {
                continue;
            };
            let key = match self.attachment_key(source, *id) {
                Ok(key) => key,
                Err(err) => {
                    tracing::warn!(code = ?err.code, "an attachment could not be re-wrapped; it stays");
                    continue;
                }
            };
            let record = kr::wrap_attachment_key(
                &key,
                &target_key,
                target,
                *id,
                target_epoch,
                &self.security.device_keys,
                self.device.device_id,
                now,
                self.rng.as_ref(),
            );
            self.put_keyring_item(KeyringItem::Record(record))?;
            self.structure(target)?
                .model
                .attachment_put(&manifest)
                .map_err(map_model_error)?;
            moved.push(*id);
        }
        if moved.is_empty() {
            return Ok(());
        }
        self.persist_structure(target, Some(&[]), UpdateOrigin::Local)?;
        for id in &moved {
            self.structure(source)?
                .model
                .attachment_remove(*id)
                .map_err(map_model_error)?;
        }
        self.persist_structure(source, Some(&[]), UpdateOrigin::Local)?;
        for id in &moved {
            self.db
                .attachment_set_space(*id, target)
                .map_err(map_storage_error)?;
        }
        self.chunks_follow_move(source, &moved)?;
        self.events
            .push(Event::AttachmentsChanged { space_id: source });
        self.events
            .push(Event::AttachmentsChanged { space_id: target });
        Ok(())
    }

    // ----- attachments the moved pages keep using ----------------------------------------------

    /// Copies each attachment into `target` under a fresh key (`Core::attachment_copy`); answers
    /// `(original, copy)` for the ones copied. One that is not complete on this device cannot be
    /// copied: the pages keep naming the original (still readable on this account's devices).
    fn copy_attachments_for_move(
        &mut self,
        target: SpaceId,
        ids: &[AttachmentId],
        now: i64,
    ) -> Result<Vec<(AttachmentId, AttachmentId)>, ApiError> {
        let mut copies = Vec::new();
        for id in ids {
            match self.attachment_copy(*id, target, now)? {
                Some(copy) => copies.push((*id, copy)),
                None => tracing::warn!(
                    "a file the moved pages use is not complete here: it cannot be copied"
                ),
            }
        }
        Ok(copies)
    }

    /// Makes the moved pages name the copies: their icons, covers and `Files` values (in `pages`,
    /// which `move_in` writes) and the blocks of their documents (a local edit each).
    fn rewrite_attachment_refs(
        &mut self,
        source: SpaceId,
        pages: &mut [MovedPage],
        ids: &[PageId],
        copies: &[(AttachmentId, AttachmentId)],
    ) -> Result<(), ApiError> {
        if copies.is_empty() {
            return Ok(());
        }
        let as_text: Vec<(String, String)> = copies
            .iter()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .collect();
        let swap_id = |id: &str| -> Option<&str> {
            as_text
                .iter()
                .find(|(from, _)| from == id)
                .map(|(_, to)| to.as_str())
        };
        let swap_prefixed = |stored: &str| -> Option<String> {
            let rest = stored.strip_prefix(crate::validate::ICON_ATTACHMENT_PREFIX)?;
            Some(format!(
                "{}{}",
                crate::validate::ICON_ATTACHMENT_PREFIX,
                swap_id(rest)?
            ))
        };
        let file_properties: HashSet<PropertyId> = self
            .structure(source)?
            .model
            .property_schema()
            .into_iter()
            .filter(|def| def.kind == PropertyKind::Files)
            .map(|def| def.id)
            .collect();
        for page in pages.iter_mut() {
            if let Some(icon) = page.icon.as_deref().and_then(swap_prefixed) {
                page.icon = Some(icon);
            }
            if let Some(cover) = page.cover.as_deref().and_then(swap_prefixed) {
                page.cover = Some(cover);
            }
            for (property, value) in &mut page.props {
                if !file_properties.contains(property) {
                    continue;
                }
                if let Value::Array(items) = value {
                    for item in items {
                        if let Some(to) = item.as_str().and_then(swap_id) {
                            *item = Value::String(to.to_owned());
                        }
                    }
                }
            }
        }
        for page in ids {
            if self
                .db
                .doc_get(DocId::for_page(*page))
                .map_err(map_storage_error)?
                .is_none()
            {
                continue;
            }
            let was_open = self.docs.pages.contains_key(page);
            let mut changed = 0;
            {
                let open = self.open_page(*page)?;
                for (from, to) in copies {
                    changed += open
                        .model
                        .replace_attachment(*from, *to)
                        .map_err(map_model_error)?;
                }
            }
            if changed > 0 {
                self.persist_page(*page, None, UpdateOrigin::Local, ChangeSource::Core)?;
            }
            if !was_open {
                self.docs.pages.remove(page);
            }
        }
        Ok(())
    }

    /// The chunks of attachments that moved between a personal space and a shared one, or between
    /// two shared spaces, have to reach the namespace of their new space (each feed has its own,
    /// ADR-0045): a chunk the personal namespace holds is copied server-side into a shared feed's, any
    /// other is uploaded from this device. Moves between personal spaces change nothing (one
    /// namespace).
    fn chunks_follow_move(
        &mut self,
        source: SpaceId,
        moved: &[AttachmentId],
    ) -> Result<(), ApiError> {
        let source_shared = self.is_shared_space(source);
        for id in moved {
            let target = self
                .db
                .attachment_get(*id)
                .map_err(map_storage_error)?
                .map(|row| row.space_id);
            let Some(target) = target else {
                continue;
            };
            let target_shared = self.is_shared_space(target);
            if !source_shared && !target_shared {
                continue;
            }
            for chunk in self
                .db
                .chunk_sync_by_attachment(*id)
                .map_err(map_storage_error)?
            {
                if chunk.state != crate::transfers::SYNCED {
                    continue;
                }
                let state = if target_shared && !source_shared {
                    crate::transfers::COPY
                } else {
                    crate::transfers::UPLOAD
                };
                self.db
                    .chunk_sync_set(chunk.hash, state, 0, None)
                    .map_err(map_storage_error)?;
            }
        }
        self.request_sync()?;
        Ok(())
    }

    // ----- remote moves -----------------------------------------------------------------------

    /// Among `candidates` (pages that became visible in `space_id` with a structure update), the
    /// ones that arrive from another space, with that space: the page index still names it, a
    /// residue there points here, or the page's document does. Read before the update's rows are
    /// written, which replace the index rows.
    pub(crate) fn find_arrivals(
        &self,
        space_id: SpaceId,
        candidates: &[PageId],
    ) -> Result<Vec<(PageId, SpaceId)>, ApiError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut moved_here: HashMap<PageId, SpaceId> = HashMap::new();
        for (other, open) in &self.docs.structures {
            if *other == space_id {
                continue;
            }
            for (page, to, _) in open.model.moved_out() {
                if to == space_id {
                    moved_here.insert(page, *other);
                }
            }
        }
        let mut out = Vec::new();
        for page in candidates {
            let indexed = self
                .db
                .pages_index_get(*page)
                .map_err(map_storage_error)?
                .map(|row| row.space_id)
                .filter(|space| *space != space_id);
            let from = match indexed.or_else(|| moved_here.get(page).copied()) {
                Some(from) => Some(from),
                None => self
                    .db
                    .doc_get(DocId::for_page(*page))
                    .map_err(map_storage_error)?
                    .and_then(|row| row.space_id)
                    .filter(|space| *space != space_id),
            };
            if let Some(from) = from {
                out.push((*page, from));
            }
        }
        Ok(out)
    }

    /// The pages that arrived in `space_id` (see [`Self::find_arrivals`]): their documents point
    /// at it, and one `PagesMovedSpace` per space they came from tells the shells.
    pub(crate) fn settle_arrivals(
        &mut self,
        space_id: SpaceId,
        arrivals: &[(PageId, SpaceId)],
    ) -> Result<(), ApiError> {
        let mut by_origin: BTreeMap<SpaceId, Vec<PageId>> = BTreeMap::new();
        for (page, from) in arrivals {
            if let Some(row) = self
                .db
                .doc_get(DocId::for_page(*page))
                .map_err(map_storage_error)?
                && row.space_id != Some(space_id)
            {
                self.db
                    .doc_set_space(row.doc_id, space_id)
                    .map_err(map_storage_error)?;
            }
            by_origin.entry(*from).or_default().push(*page);
        }
        for (from, page_ids) in by_origin {
            self.events.push(Event::PagesMovedSpace {
                from_space_id: from,
                to_space_id: space_id,
                page_ids,
            });
        }
        Ok(())
    }

    /// Settles what remote updates and crashes can leave behind, the same way on every device
    /// (the writes are local edits like any other and sync; every device computes the same
    /// result, so they merge):
    ///
    /// 1. pages another device created under a page that moved away are re-rooted in their
    ///    space, and a page that two devices moved into the same space (two nodes of one
    ///    document) keeps one node;
    /// 2. a page that is live or trashed in more than one space (two devices moved it to
    ///    different spaces) stays where it arrived last (`arrivedAt`, then the greater space id);
    ///    the other copies become residue pointing at the winner;
    /// 3. an attachment manifest in more than one space stays in the space of the pages that use
    ///    it.
    ///
    /// Runs after every applied remote structure update and after a local move. It is cheap when
    /// nothing moved: the spaces have no residue.
    pub(crate) fn reconcile_moves(&mut self) -> Result<(), ApiError> {
        self.reconcile(false)
    }

    /// [`Self::reconcile_moves`] at open, which also finds pages that are live in two spaces
    /// without residue anywhere (a crash between the two writes of a move leaves that): every
    /// page of every space is looked at.
    pub(crate) fn reconcile_moves_at_open(&mut self) -> Result<(), ApiError> {
        self.reconcile(true)
    }

    fn reconcile(&mut self, full: bool) -> Result<(), ApiError> {
        let mut spaces: Vec<SpaceId> = self.docs.structures.keys().copied().collect();
        spaces.sort_unstable();
        let mut residue: BTreeMap<SpaceId, Vec<PageId>> = BTreeMap::new();
        let mut any_orphans = false;
        for space in &spaces {
            if let Some(open) = self.docs.structures.get(space) {
                let moved: Vec<PageId> = open
                    .model
                    .moved_out()
                    .into_iter()
                    .map(|(page, _, _)| page)
                    .collect();
                if !moved.is_empty() {
                    residue.insert(*space, moved);
                }
                any_orphans |= !open.model.orphans().is_empty();
            }
        }
        if !full && residue.is_empty() && !any_orphans {
            return Ok(());
        }
        let now = self.clock.now_ms();

        // 1. Orphans.
        if any_orphans {
            for space in &spaces {
                let has_orphans = self
                    .docs
                    .structures
                    .get(space)
                    .is_some_and(|open| !open.model.orphans().is_empty());
                if !has_orphans {
                    continue;
                }
                let rehomed = self
                    .structure_mut(*space)?
                    .model
                    .rehome_orphans(now)
                    .map_err(map_model_error)?;
                if rehomed.is_empty() {
                    continue;
                }
                self.persist_structure_with(
                    *space,
                    Some(&rehomed),
                    UpdateOrigin::Local,
                    PropsScope::Pages(&rehomed),
                )?;
                self.index_rehomed_pages(*space, &rehomed)?;
                self.events
                    .push(Event::StructureChanged { space_id: *space });
                self.events.push(Event::SpacesChanged);
            }
        }

        // 1b. A page that two devices moved into the same space is two nodes of one document:
        //     the copies the index does not show are merged away.
        for space in &spaces {
            let merged = self
                .structure_mut(*space)?
                .model
                .merge_duplicates(now)
                .map_err(map_model_error)?;
            if merged.is_empty() {
                continue;
            }
            // Pages may have been re-rooted too: the whole space's rows follow its document.
            self.persist_structure_with(*space, None, UpdateOrigin::Local, PropsScope::Space)?;
            for page in &merged {
                if self
                    .db
                    .doc_get(DocId::for_page(*page))
                    .map_err(map_storage_error)?
                    .is_some()
                {
                    self.refresh_page_text(*page)?;
                }
            }
            self.events
                .push(Event::StructureChanged { space_id: *space });
            self.events.push(Event::SpacesChanged);
        }

        // 2. A page in two spaces.
        let mut candidates: BTreeSet<PageId> = residue.values().flatten().copied().collect();
        if full {
            let mut seen: HashMap<PageId, u32> = HashMap::new();
            for space in &spaces {
                if let Some(open) = self.docs.structures.get(space) {
                    for page in open.model.all_page_ids() {
                        *seen.entry(page).or_default() += 1;
                    }
                }
            }
            candidates.extend(
                seen.into_iter()
                    .filter(|(_, count)| *count > 1)
                    .map(|(page, _)| page),
            );
        }
        let mut evictions: BTreeMap<SpaceId, Vec<(PageId, SpaceId)>> = BTreeMap::new();
        for page in candidates {
            let mut holders: Vec<(i64, SpaceId)> = spaces
                .iter()
                .filter_map(|space| {
                    let at = self.docs.structures.get(space)?.model.arrival(page)?;
                    Some((at, *space))
                })
                .collect();
            if holders.len() < 2 {
                continue;
            }
            holders.sort_unstable();
            let Some(&(_, winner)) = holders.last() else {
                continue;
            };
            for (_, loser) in holders.iter().filter(|(_, space)| *space != winner) {
                evictions.entry(*loser).or_default().push((page, winner));
            }
        }
        if !evictions.is_empty() {
            let mut touched: BTreeSet<SpaceId> = BTreeSet::new();
            let mut moved_events: BTreeMap<(SpaceId, SpaceId), Vec<PageId>> = BTreeMap::new();
            for (loser, list) in &evictions {
                let evicted = self
                    .structure_mut(*loser)?
                    .model
                    .evict(list, now)
                    .map_err(map_model_error)?;
                if evicted.is_empty() {
                    continue;
                }
                touched.insert(*loser);
                for (page, winner) in list {
                    if evicted.contains(page) {
                        touched.insert(*winner);
                        moved_events
                            .entry((*loser, *winner))
                            .or_default()
                            .push(*page);
                    }
                }
            }
            // The rows of every space the settling touched follow its documents (the index row of
            // a page that was in both names whichever structure was written last).
            for space in &touched {
                self.persist_structure_with(*space, None, UpdateOrigin::Local, PropsScope::Space)?;
                self.events
                    .push(Event::StructureChanged { space_id: *space });
            }
            if !touched.is_empty() {
                self.events.push(Event::SpacesChanged);
            }
            for ((from, to), mut page_ids) in moved_events {
                // The moved page first: the order of the winner's tree (pre-order).
                if let Some(open) = self.docs.structures.get(&to) {
                    let order: HashMap<PageId, usize> = open
                        .model
                        .all_page_ids()
                        .into_iter()
                        .enumerate()
                        .map(|(at, page)| (page, at))
                        .collect();
                    page_ids.sort_by_key(|page| order.get(page).copied().unwrap_or(usize::MAX));
                }
                self.events.push(Event::PagesMovedSpace {
                    from_space_id: from,
                    to_space_id: to,
                    page_ids,
                });
            }
        }

        // 3. A manifest in two spaces.
        self.reconcile_duplicate_manifests()
    }

    /// Title and body rows of pages that were hidden below residue and are live again (a page
    /// created on another device is known to this one only now).
    fn index_rehomed_pages(&mut self, space_id: SpaceId, pages: &[PageId]) -> Result<(), ApiError> {
        for page in pages {
            let Some(info) = self.structure(space_id)?.model.page(*page) else {
                continue;
            };
            self.text_after_create(*page, &info.title)?;
            if self
                .db
                .doc_get(DocId::for_page(*page))
                .map_err(map_storage_error)?
                .is_some()
            {
                self.refresh_page_text(*page)?;
            }
        }
        Ok(())
    }

    /// An attachment whose manifest is in several structures keeps it where the pages that use
    /// it are (when they are in one of those spaces and only one); the others give it up, and
    /// the index row follows. Anything less clear is left alone: a spare manifest costs nothing,
    /// a manifest removed from every space would lose the file.
    fn reconcile_duplicate_manifests(&mut self) -> Result<(), ApiError> {
        let mut holders: BTreeMap<AttachmentId, Vec<SpaceId>> = BTreeMap::new();
        for (space, open) in &self.docs.structures {
            for manifest in open.model.attachments() {
                holders
                    .entry(manifest.attachment_id)
                    .or_default()
                    .push(*space);
            }
        }
        for (attachment, mut spaces) in holders {
            if spaces.len() < 2 {
                continue;
            }
            spaces.sort_unstable();
            let users = self
                .db
                .attachment_referencing_pages(attachment)
                .map_err(map_storage_error)?;
            let mut keep: BTreeSet<SpaceId> = BTreeSet::new();
            for user in users {
                if let Some(row) = self.db.pages_index_get(user).map_err(map_storage_error)?
                    && spaces.contains(&row.space_id)
                {
                    keep.insert(row.space_id);
                }
            }
            let (Some(&winner), 1) = (keep.iter().next(), keep.len()) else {
                continue;
            };
            for space in spaces.iter().filter(|space| **space != winner) {
                self.structure_mut(*space)?
                    .model
                    .attachment_remove(attachment)
                    .map_err(map_model_error)?;
                self.persist_structure(*space, Some(&[]), UpdateOrigin::Local)?;
                self.events
                    .push(Event::AttachmentsChanged { space_id: *space });
            }
            self.db
                .attachment_set_space(attachment, winner)
                .map_err(map_storage_error)?;
            self.events
                .push(Event::AttachmentsChanged { space_id: winner });
        }
        Ok(())
    }

    /// Makes the space of every page document agree with the page index (a crash between the
    /// steps of a move, a backup): the index follows the structure documents.
    pub(crate) fn verify_doc_spaces(&mut self) -> Result<(), ApiError> {
        let rows = self
            .db
            .docs_by_kind(DocKindCode::Page)
            .map_err(map_storage_error)?;
        for row in rows {
            let page = PageId::from_bytes(row.doc_id.to_bytes());
            let Some(indexed) = self.db.pages_index_get(page).map_err(map_storage_error)? else {
                continue;
            };
            if row.space_id != Some(indexed.space_id) {
                self.db
                    .doc_set_space(row.doc_id, indexed.space_id)
                    .map_err(map_storage_error)?;
            }
        }
        // The comment streams follow their pages the same way.
        self.comments_verify_spaces()
    }
}

/// Replays the schema changes of a plan on the target structure document.
fn apply_schema_ops(model: &mut StructureDoc, ops: &[SchemaOp], now: i64) -> Result<(), ApiError> {
    for op in ops {
        match op {
            SchemaOp::Property {
                id,
                name,
                kind,
                config,
                spare_options,
            } => {
                let mut spare = spare_options.iter().copied();
                let made = model.create_property_with(
                    *id,
                    name,
                    *kind,
                    config.as_ref(),
                    &mut || {
                        spare
                            .next()
                            .unwrap_or_else(|| kb_types::OptionId::from_bytes([0; 16]))
                    },
                    now,
                );
                match (made, config) {
                    // A formula that does not check in the target (something it reads has no
                    // counterpart there) moves as written and stays broken: no value, and its
                    // settings show the error (ADR-0055).
                    (
                        Err(kb_model::ModelError::InvalidProperty {
                            field: "config", ..
                        }),
                        Some(kb_types::PropertyConfig::Formula { expression, .. }),
                    ) if *kind == kb_types::PropertyKind::Formula => {
                        model
                            .create_formula_unchecked(*id, name, expression, now)
                            .map_err(map_model_error)?;
                    }
                    (made, _) => {
                        made.map_err(map_model_error)?;
                    }
                }
            }
            SchemaOp::RelationPair {
                forward,
                back,
                name,
                back_name,
            } => {
                model
                    .create_relation_pair(*forward, *back, name, back_name, now)
                    .map_err(map_model_error)?;
            }
            SchemaOp::AddOption {
                property,
                id,
                name,
                color,
                group,
            } => {
                model
                    .add_option(*property, *id, name, *color, *group, now)
                    .map_err(map_model_error)?;
            }
            SchemaOp::DeleteOption { property, option } => {
                model
                    .delete_option(*property, *option, now)
                    .map_err(map_model_error)?;
            }
        }
    }
    Ok(())
}
