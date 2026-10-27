//! Moves between spaces (ADR-0038, `docs/architecture/space-move.md` section 3).
//!
//! A page that leaves a space leaves a **residue** node under a hidden root of kind `moved`: its
//! id, `movedTo` and `movedAt`, nothing else. Residue is neither live nor trashed, is never a
//! tombstone, and is invisible to every reader of the tree. A page that arrives gets a fresh node
//! with its metadata and `arrivedAt`/`arrivedFrom`. Every write here is committed with
//! [`MOVE_ORIGIN`], which structural undo excludes.
//!
//! - **Residue** is any node with a `movedTo` stamp. It normally sits under a `moved` root (created
//!   on first use, like the trash root; a merge can leave several), with the shape of the moved
//!   subtree kept, but it is residue wherever a merge leaves it.
//! - An **orphan** is a page node without a stamp that hangs below residue (another device
//!   created it under a page that moved away). It is hidden, with its subtree, until
//!   [`StructureDoc::rehome_orphans`] puts it at the end of the live root; [`StructureDoc::move_in`]
//!   and [`StructureDoc::evict`] do that in their own commit.
//! - A moved page keeps its id. The source node is turned into residue (title, icon, cover, font, small
//!   text, lock, template flag and placeholders, values, edit stamps and every other key removed), the target gets fresh nodes carrying
//!   the metadata.
//!   A rename or a value edit another device makes to the page while it moves lands on the residue
//!   and is lost; a child it creates is re-rooted.
//! - Favourites follow: removed in the source, appended in the target (favourites origin).
//! - Residue stamped with the document's **own** space as `movedTo` is not a move: it is a copy of
//!   a page that two devices created at the same time (both moved the same subtree in), merged
//!   away by [`StructureDoc::merge_duplicates`]. It is hidden like any residue and left out of
//!   [`StructureDoc::moved_out`].

use std::collections::HashSet;

use kb_types::{DeviceId, PageFont, PageId, PropertyId, SpaceId, TemplatePlaceholder};
use loro::{CommitOptions, TreeID, TreeParentId};

use crate::index::{node_label, tombstoned};
use crate::meta::{self, engine};
use crate::properties::write_props_json;
use crate::{ModelError, StructureDoc};

/// Commit origin of moves between spaces; the structure document's undo manager excludes it.
pub const MOVE_ORIGIN: &str = "kb-model/move";

/// The most passes [`StructureDoc::rehome_orphans`] makes (each pass handles one level of orphans
/// below residue below orphans; real documents need one).
const MAX_REHOME_PASSES: usize = 64;

/// One page of a subtree that moves to another space, as [`StructureDoc::moved_pages`] and
/// [`StructureDoc::move_out`] read it and as [`StructureDoc::move_in`] writes it (the core maps
/// `props` to the target's schema between the two).
#[derive(Debug, Clone, PartialEq)]
pub struct MovedPage {
    /// The page (its id is kept).
    pub page_id: PageId,
    /// Its parent as an index into the same list; `None` for the moved page itself.
    pub parent: Option<usize>,
    /// Title.
    pub title: String,
    /// Icon, as stored.
    pub icon: Option<String>,
    /// Cover, as stored (ADR-0040).
    pub cover: Option<String>,
    /// The cover's vertical position in percent, as the page reports it: absent for the default.
    pub cover_y: Option<u8>,
    /// Font, as the page reports it (ADR-0040): absent for the default.
    pub font: Option<PageFont>,
    /// Smaller text (ADR-0040).
    pub small_text: bool,
    /// Locked in the UI (ADR-0040).
    pub locked: bool,
    /// The page is a template (ADR-0056).
    pub template: bool,
    /// The template placeholders the page stores, by property id of the space it leaves (as
    /// written, sorted by id; the core maps them to the target's schema with the values and drops
    /// the ones whose property has no counterpart).
    pub template_values: Vec<(PropertyId, TemplatePlaceholder)>,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// The device that created it, when recorded.
    pub created_by: Option<DeviceId>,
    /// Last edit time, Unix ms.
    pub updated_at: i64,
    /// The device that edited it last, when recorded.
    pub edited_by: Option<DeviceId>,
    /// Stored property values by property id (raw JSON as the `props` map holds them).
    pub props: Vec<(PropertyId, serde_json::Value)>,
    /// Whether it was a favourite of the space it leaves.
    pub favorite: bool,
}

/// What an undo or redo must not take from a move (see [`StructureDoc::repair_after_undo`]).
pub(crate) struct MoveGuard {
    /// Every residue: page, `movedTo`, `movedAt`.
    residue: Vec<(PageId, String, i64)>,
    /// The pages that arrived from another space and the topmost orphans, with their nodes.
    nodes: Vec<(PageId, TreeID)>,
}

impl StructureDoc {
    /// Reads a live page and its live subtree as [`MovedPage`]s, pre-order, without writing:
    /// the reading half of [`Self::move_out`] (the plan of a move uses it, and `move_out` itself,
    /// so both always agree).
    ///
    /// `props` are the **raw** stored values exactly as the page node's `props` map holds them
    /// (a select is the option id string, a number a JSON number, ...), keyed by this space's
    /// property ids, sorted by id; computed kinds store nothing and `null` values are left out.
    /// `PageNotFound` (unknown, purged or residue), `PageTrashed`.
    pub fn moved_pages(&self, page_id: PageId) -> Result<Vec<MovedPage>, ModelError> {
        Ok(self
            .moved_subtree(page_id)?
            .into_iter()
            .map(|(_, page)| page)
            .collect())
    }

    /// Moves a live page and its live subtree out of this space towards `to`: reads them
    /// (pre-order, see [`Self::moved_pages`]) and turns their nodes into residue in one commit.
    /// The page and its subtree leave the favourites. `PageNotFound`, `PageTrashed`.
    pub fn move_out(
        &mut self,
        page_id: PageId,
        to: SpaceId,
        now_ms: i64,
    ) -> Result<Vec<MovedPage>, ModelError> {
        let subtree = self.moved_subtree(page_id)?;
        if to == self.space_id {
            return Err(ModelError::Shape(
                "a page cannot move to its own space".into(),
            ));
        }
        let nodes: Vec<TreeID> = subtree.iter().map(|(node, _)| *node).collect();
        self.move_write(|doc| {
            let root = doc.moved_root_for_write()?;
            for node in &nodes {
                doc.make_residue(*node, to, now_ms)?;
            }
            if let Some(top) = nodes.first() {
                doc.tree.mov(*top, root).map_err(engine)?;
            }
            Ok(())
        })?;
        let pages: Vec<MovedPage> = subtree.into_iter().map(|(_, page)| page).collect();
        let ids: Vec<PageId> = pages.iter().map(|page| page.page_id).collect();
        self.forget_favorites(&ids)?;
        Ok(pages)
    }

    /// Creates the pages of a subtree that arrives from `from`: the first under `parent` at
    /// `index` (`None` = last; the live position convention of [`Self::create_page`]), the others
    /// appended under their listed parent. The first page has no `parent` in the list, every other
    /// one a parent earlier in the list.
    ///
    /// The nodes carry the id, title, icon, cover, creation and edit stamps and `props` as given, plus
    /// `arrivedAt = now_ms` and `arrivedFrom = from`. Residue of these ids is removed first (its
    /// stamped children stay residue, under the moved root); every orphan of the document is
    /// re-rooted first (see [`Self::rehome_orphans`]). Pages with `favorite` are appended to this
    /// space's favourites.
    ///
    /// Everything is checked before anything is written: an id that is live, trashed, purged,
    /// hidden below residue or repeated is `DuplicatePage`; a parent that is not live or a list
    /// that does not form one subtree is `ParentUnavailable`. Returns the ids created.
    pub fn move_in(
        &mut self,
        pages: &[MovedPage],
        parent: Option<PageId>,
        index: Option<usize>,
        from: SpaceId,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }
        if from == self.space_id {
            return Err(ModelError::Shape(
                "a page cannot arrive from its own space".into(),
            ));
        }
        let hidden: HashSet<PageId> = self.orphan_ids().into_iter().collect();
        let tombstones = tombstoned(&self.tombstones);
        let mut seen: HashSet<PageId> = HashSet::with_capacity(pages.len());
        for (at, page) in pages.iter().enumerate() {
            if self.index.node(page.page_id).is_some()
                || tombstones.contains(&page.page_id)
                || hidden.contains(&page.page_id)
                || !seen.insert(page.page_id)
            {
                return Err(ModelError::DuplicatePage);
            }
            let well_formed = match (at, page.parent) {
                (0, None) => true,
                (0, Some(_)) | (_, None) => false,
                (_, Some(parent)) => parent < at,
            };
            if !well_formed {
                return Err(ModelError::ParentUnavailable);
            }
        }
        let target = match parent {
            None => TreeParentId::Root,
            Some(parent) => TreeParentId::Node(
                self.live_node(parent)
                    .ok_or(ModelError::ParentUnavailable)?,
            ),
        };

        let doomed: Vec<TreeID> = self
            .index
            .residue_nodes
            .iter()
            .filter(|(id, _)| seen.contains(id))
            .map(|(_, node)| *node)
            .collect();
        let created =
            self.move_write(|doc| doc.write_move_in(pages, target, index, from, now_ms, &doomed))?;
        for page in pages.iter().filter(|page| page.favorite) {
            self.set_favorite(page.page_id, true)?;
        }
        Ok(created)
    }

    /// When a live or trashed page arrived here (`arrivedAt`), else when it was created. `None`
    /// for a page this document does not hold (residue included).
    #[must_use]
    pub fn arrival(&self, page_id: PageId) -> Option<i64> {
        let node = self.index.node(page_id)?;
        let meta_map = self.meta_of(node)?;
        meta::int(&meta_map, meta::ARRIVED_AT).or_else(|| meta::int(&meta_map, meta::CREATED_AT))
    }

    /// Residue: page, the space it went to, when, in page id order. One entry per page (the first
    /// node found that says it went to another space); entries whose `movedTo` is not a space id
    /// are left out, and so is residue whose `movedTo` is **this document's own space**: that
    /// stamp is [`Self::merge_duplicates`]' (a copy of a page that is shown once, hidden for good),
    /// not a move. After a merge the page can also be live or trashed here (see
    /// [`Self::check_invariants`]).
    #[must_use]
    pub fn moved_out(&self) -> Vec<(PageId, SpaceId, i64)> {
        let mut seen: HashSet<PageId> = HashSet::new();
        let mut out: Vec<(PageId, SpaceId, i64)> = self
            .index
            .residue_nodes
            .iter()
            .filter_map(|(page, node)| {
                let meta_map = self.meta_of(*node)?;
                let to = meta::string(&meta_map, meta::MOVED_TO)?
                    .parse::<SpaceId>()
                    .ok()?;
                (to != self.space_id && seen.insert(*page))
                    .then(|| (*page, to, meta::int(&meta_map, meta::MOVED_AT).unwrap_or(0)))
            })
            .collect();
        out.sort_unstable_by_key(|(page, _, _)| *page);
        out
    }

    /// The pages hidden because they hang below residue without a stamp (another device created
    /// them under a page that moved away), the topmost ones first with their subtrees, pre-order.
    /// [`Self::rehome_orphans`] makes them live again.
    #[must_use]
    pub fn orphans(&self) -> Vec<PageId> {
        self.orphan_ids()
    }

    /// Turns the losing copies of a concurrent move into residue pointing at the winner's
    /// space, in one commit. Only pages that are live or trashed here are evicted (the others
    /// are skipped, as are entries that name this space); each node goes under the moved root,
    /// stripped like [`Self::move_out`] does, and leaves the favourites. Children that are not
    /// listed stay under their node and are re-rooted at the end of the live root by the same
    /// commit (the core refreshes the space's derived rows when anything was evicted).
    /// Returns the ids evicted, in the order listed.
    pub fn evict(
        &mut self,
        pages: &[(PageId, SpaceId)],
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let mut seen: HashSet<PageId> = HashSet::with_capacity(pages.len());
        let targets: Vec<(PageId, TreeID, SpaceId)> = pages
            .iter()
            .filter(|(page, to)| *to != self.space_id && seen.insert(*page))
            .filter_map(|(page, to)| Some((*page, self.index.node(*page)?, *to)))
            .collect();
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        self.move_write(|doc| {
            let root = doc.moved_root_for_write()?;
            for (_, node, to) in &targets {
                doc.make_residue(*node, *to, now_ms)?;
                doc.tree.mov(*node, root).map_err(engine)?;
            }
            doc.rebuild_index();
            doc.rehome_in_commit().map(|_| ())
        })?;
        let evicted: Vec<PageId> = targets.iter().map(|(page, _, _)| *page).collect();
        self.forget_favorites(&evicted)?;
        Ok(evicted)
    }

    /// Puts every unstamped page node found inside residue (a page another device created under
    /// a page that moved away) back at the end of the live root, in one commit; it takes its
    /// subtree along. A node without a page id, a tombstoned one and one whose id is already
    /// visible stay hidden. Writes nothing when there is none. Returns the ids that are live
    /// again: each orphan and every page below it, pre-order.
    pub fn rehome_orphans(&mut self, _now_ms: i64) -> Result<Vec<PageId>, ModelError> {
        if self.index.orphans.is_empty() {
            return Ok(Vec::new());
        }
        let tops = self.move_write(|doc| doc.rehome_in_commit())?;
        let mut seen: HashSet<PageId> = HashSet::new();
        Ok(tops
            .into_iter()
            .flat_map(|node| self.preorder(node))
            .filter(|page| seen.insert(*page))
            .collect())
    }

    /// Shows every page once: for each page id that more than one live or trashed node holds
    /// (two devices that moved the same subtree into this space at the same time both created
    /// its pages), the node the index already shows (the first visible one, the same on every
    /// replica) stays and every other node of that id becomes residue stamped `movedTo` = **this
    /// document's own space** and goes under the moved root.
    ///
    /// That stamp means "a copy that was merged away": the node is hidden for good (like any
    /// residue it is neither live nor trashed, never re-rooted, never a tombstone), and it is no
    /// move: [`Self::moved_out`] leaves it out, so nothing that detects moves sees it. Unstamped
    /// children of such a node are re-rooted like orphans (see [`Self::rehome_orphans`]), unless
    /// their id is itself already shown, in which case they are merged away too, down the subtree.
    /// A page that only one copy has under its parent (an extra child) stays live, at the end of
    /// the live root.
    ///
    /// One commit with [`MOVE_ORIGIN`]; writes nothing and answers an empty list when no page
    /// has a duplicate. Idempotent: replicas that all run it on the same state write the same
    /// stamps (the `movedAt` of two concurrent runs merges last-writer-wins). Returns the ids
    /// whose duplicates were merged away, in the order they were found; the pages that came back
    /// live through re-rooting are not listed, so the core refreshes the derived rows of the
    /// space when the answer is not empty.
    pub fn merge_duplicates(&mut self, now_ms: i64) -> Result<Vec<PageId>, ModelError> {
        if !self.has_duplicates() {
            return Ok(Vec::new());
        }
        let own = self.space_id;
        self.move_write(|doc| {
            let mut merged: Vec<PageId> = Vec::new();
            for _ in 0..MAX_REHOME_PASSES {
                // The nodes the index hides as duplicates, and the hidden orphans whose page is
                // shown already (the children of a node merged away, say).
                let mut targets: Vec<(PageId, TreeID)> = doc.index.duplicates.clone();
                targets.extend(
                    doc.index
                        .orphans
                        .iter()
                        .filter(|(page, _)| doc.index.pages.contains_key(page))
                        .copied(),
                );
                if targets.is_empty() {
                    break;
                }
                let root = doc.moved_root_for_write()?;
                for (page, node) in &targets {
                    doc.make_residue(*node, own, now_ms)?;
                    doc.tree.mov(*node, root).map_err(engine)?;
                    if !merged.contains(page) {
                        merged.push(*page);
                    }
                }
                doc.rebuild_index();
                // What hung below them and is not a duplicate comes back like an orphan.
                doc.rehome_in_commit()?;
            }
            Ok(merged)
        })
    }

    // ----- undo -------------------------------------------------------------------------------

    /// What a local undo or redo could take away from a move: the residue, and the pages that
    /// arrived or wait below residue (a creation the person undoes deletes the node, and with it
    /// whatever a move put there). Taken before the undo, used by [`Self::repair_after_undo`].
    pub(crate) fn move_guard(&self) -> MoveGuard {
        let residue = self
            .index
            .residue
            .iter()
            .filter_map(|(page, node)| {
                let meta_map = self.meta_of(*node)?;
                Some((
                    *page,
                    meta::string(&meta_map, meta::MOVED_TO)?,
                    meta::int(&meta_map, meta::MOVED_AT).unwrap_or(0),
                ))
            })
            .collect();
        let nodes = self
            .index
            .arrived
            .iter()
            .chain(self.index.orphans.iter())
            .copied()
            .collect();
        MoveGuard { residue, nodes }
    }

    /// After an undo or redo: puts back what it took from a move, in one commit with
    /// [`MOVE_ORIGIN`] (so it is no undo step either). A residue whose node the undo deleted
    /// (the person undid the creation of a page that has since moved away) is created again
    /// under the moved root; an arrived page or an orphan whose ancestor the undo deleted is
    /// moved to the end of the live root; residue gets rid of the title and edit stamps an undo
    /// of an earlier edit wrote back. Writes nothing when none of it applies.
    pub(crate) fn repair_after_undo(&mut self, guard: &MoveGuard) -> Result<(), ModelError> {
        let tombstones = tombstoned(&self.tombstones);
        let lost: Vec<&(PageId, String, i64)> = guard
            .residue
            .iter()
            .filter(|(page, _, _)| {
                !self.index.residue.contains_key(page) && !tombstones.contains(page)
            })
            .collect();
        let hidden: Vec<(PageId, TreeID)> = guard
            .nodes
            .iter()
            .filter(|(page, node)| {
                !self.index.pages.contains_key(page)
                    && !tombstones.contains(page)
                    && self.tree.is_node_deleted(node).unwrap_or(false)
                    && self
                        .meta_of(*node)
                        .is_some_and(|m| meta::page_id(&m, meta::ID) == Some(*page))
            })
            .copied()
            .collect();
        let hidden_nodes: HashSet<TreeID> = hidden.iter().map(|(_, node)| *node).collect();
        let tops: Vec<TreeID> = hidden
            .iter()
            .map(|(_, node)| *node)
            .filter(|node| {
                !matches!(
                    self.tree.parent(*node),
                    Some(TreeParentId::Node(up)) if hidden_nodes.contains(&up)
                )
            })
            .collect();
        let stray: Vec<TreeID> = self
            .index
            .residue_nodes
            .iter()
            .map(|(_, node)| *node)
            .filter(|node| !self.residue_is_clean(*node))
            .collect();
        if lost.is_empty() && tops.is_empty() && stray.is_empty() {
            return Ok(());
        }
        let lost: Vec<(PageId, String, i64)> = lost.into_iter().cloned().collect();
        self.move_write(|doc| {
            for node in &stray {
                doc.strip_to_residue(*node)?;
            }
            for node in &tops {
                doc.tree.mov(*node, TreeParentId::Root).map_err(engine)?;
            }
            if !lost.is_empty() {
                let root = doc.moved_root_for_write()?;
                for (page, to, at) in &lost {
                    let node = doc.tree.create(TreeParentId::Node(root)).map_err(engine)?;
                    let meta_map = doc.tree.get_meta(node).map_err(engine)?;
                    meta::set_string(&meta_map, meta::ID, &page.to_string())?;
                    meta::set_string(&meta_map, meta::MOVED_TO, to)?;
                    meta::set_int(&meta_map, meta::MOVED_AT, *at)?;
                }
            }
            Ok(())
        })
    }

    // ----- internals --------------------------------------------------------------------------

    /// Runs `write` as one commit with [`MOVE_ORIGIN`]: anything pending is committed first (and
    /// the open undo group closed), the commit happens whether or not the writes went through,
    /// and the index follows the document.
    fn move_write<T>(
        &mut self,
        write: impl FnOnce(&mut Self) -> Result<T, ModelError>,
    ) -> Result<T, ModelError> {
        self.close_group();
        self.commit();
        let result = write(self);
        self.doc
            .inner()
            .commit_with(CommitOptions::new().origin(MOVE_ORIGIN));
        self.rebuild_index();
        result
    }

    /// The nodes and pages of the live subtree of `page_id`, pre-order.
    fn moved_subtree(&self, page_id: PageId) -> Result<Vec<(TreeID, MovedPage)>, ModelError> {
        let node = self.index.node(page_id).ok_or(ModelError::PageNotFound)?;
        if !self.is_live(node) {
            return Err(ModelError::PageTrashed);
        }
        let favorites: HashSet<PageId> = self.favorites().into_iter().collect();
        let mut out: Vec<(TreeID, MovedPage)> = Vec::new();
        let mut stack: Vec<(TreeID, Option<usize>)> = vec![(node, None)];
        while let Some((node, parent)) = stack.pop() {
            let Some(id) = self.index.page(node) else {
                continue;
            };
            let at = out.len();
            out.push((
                node,
                self.read_moved_page(node, id, parent, favorites.contains(&id)),
            ));
            let children = self.visible_children(TreeParentId::Node(node));
            stack.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|(child, _)| (child, Some(at))),
            );
        }
        Ok(out)
    }

    fn read_moved_page(
        &self,
        node: TreeID,
        page_id: PageId,
        parent: Option<usize>,
        favorite: bool,
    ) -> MovedPage {
        let meta_map = self.meta_of(node);
        let text = |key| meta_map.as_ref().and_then(|m| meta::string(m, key));
        let int = |key| meta_map.as_ref().and_then(|m| meta::int(m, key));
        let device = |key| text(key).and_then(|raw| raw.parse::<DeviceId>().ok());
        let created_at = int(meta::CREATED_AT).unwrap_or(0);
        MovedPage {
            page_id,
            parent,
            title: text(meta::TITLE).unwrap_or_default(),
            icon: text(meta::ICON),
            cover: text(meta::COVER),
            cover_y: crate::cover_y_of(text(meta::COVER).is_some(), int(meta::COVER_Y)),
            font: text(meta::FONT).as_deref().and_then(PageFont::from_stored),
            small_text: meta_map
                .as_ref()
                .is_some_and(|m| meta::flag(m, meta::SMALL_TEXT)),
            locked: meta_map
                .as_ref()
                .is_some_and(|m| meta::flag(m, meta::LOCKED)),
            template: meta_map
                .as_ref()
                .is_some_and(|m| meta::flag(m, meta::TEMPLATE)),
            template_values: self.stored_placeholders(node),
            created_at,
            created_by: device(meta::CREATED_BY),
            updated_at: int(meta::UPDATED_AT).unwrap_or(created_at),
            edited_by: device(meta::EDITED_BY),
            props: self.props_json(node),
            favorite,
        }
    }

    /// The first moved root; created (and added to the index) when the document has none.
    fn moved_root_for_write(&mut self) -> Result<TreeID, ModelError> {
        if let Some(root) = self.index.moved_roots.first() {
            return Ok(*root);
        }
        let root = self.tree.create(TreeParentId::Root).map_err(engine)?;
        let meta_map = self.tree.get_meta(root).map_err(engine)?;
        meta::set_string(&meta_map, meta::KIND, meta::MOVED_KIND)?;
        self.index.moved_roots.push(root);
        Ok(root)
    }

    /// Turns a node into residue: every key but the id goes, then `movedTo` and `movedAt` are set.
    fn make_residue(&self, node: TreeID, to: SpaceId, now_ms: i64) -> Result<(), ModelError> {
        self.strip_to_residue(node)?;
        let meta_map = self.tree.get_meta(node).map_err(engine)?;
        meta::set_string(&meta_map, meta::MOVED_TO, &to.to_string())?;
        meta::set_int(&meta_map, meta::MOVED_AT, now_ms)
    }

    /// Removes every key of a node's meta but the id and the two stamps.
    fn strip_to_residue(&self, node: TreeID) -> Result<(), ModelError> {
        let meta_map = self.tree.get_meta(node).map_err(engine)?;
        let stale: Vec<String> = meta_map
            .keys()
            .map(|key| key.to_string())
            .filter(|key| !matches!(key.as_str(), meta::ID | meta::MOVED_TO | meta::MOVED_AT))
            .collect();
        for key in &stale {
            meta::remove(&meta_map, key)?;
        }
        Ok(())
    }

    /// Whether a residue node holds the id and the stamps and nothing else.
    fn residue_is_clean(&self, node: TreeID) -> bool {
        self.meta_of(node).is_none_or(|meta_map| {
            meta_map
                .keys()
                .all(|key| matches!(&*key, meta::ID | meta::MOVED_TO | meta::MOVED_AT))
        })
    }

    /// Moves every orphan (see the module documentation) to the end of the live root, pass after
    /// pass until none is left that can move; returns the nodes moved to the root. Not committed.
    fn rehome_in_commit(&mut self) -> Result<Vec<TreeID>, ModelError> {
        let mut moved: Vec<TreeID> = Vec::new();
        for _ in 0..MAX_REHOME_PASSES {
            let mut seen: HashSet<PageId> = HashSet::new();
            let movable: Vec<TreeID> = self
                .index
                .orphans
                .iter()
                .filter(|(page, _)| !self.index.pages.contains_key(page) && seen.insert(*page))
                .map(|(_, node)| *node)
                .collect();
            if movable.is_empty() {
                break;
            }
            for node in &movable {
                self.tree.mov(*node, TreeParentId::Root).map_err(engine)?;
            }
            moved.extend(movable);
            self.rebuild_index();
        }
        Ok(moved)
    }

    /// The writes of [`Self::move_in`] (already validated): re-roots the orphans, removes the
    /// residue of the incoming pages, creates the nodes.
    fn write_move_in(
        &mut self,
        pages: &[MovedPage],
        target: TreeParentId,
        index: Option<usize>,
        from: SpaceId,
        now_ms: i64,
        doomed: &[TreeID],
    ) -> Result<Vec<PageId>, ModelError> {
        self.rehome_in_commit()?;

        // The residue of the incoming pages goes; residue that hangs below it stays residue
        // (under the moved root), as the stamp says where those pages went.
        let doomed_set: HashSet<TreeID> = doomed.iter().copied().collect();
        for node in doomed {
            for child in self
                .tree
                .children(TreeParentId::Node(*node))
                .unwrap_or_default()
            {
                if doomed_set.contains(&child) {
                    continue;
                }
                if self.meta_of(child).is_some_and(|m| meta::is_residue(&m)) {
                    let root = self.moved_root_for_write()?;
                    self.tree.mov(child, root).map_err(engine)?;
                }
            }
        }
        for node in doomed {
            if !self.tree.is_node_deleted(node).unwrap_or(true) {
                self.tree.delete(*node).map_err(engine)?;
            }
        }

        let first_position = self.engine_position(target, None, index);
        let mut nodes: Vec<TreeID> = Vec::with_capacity(pages.len());
        // The next position under each created page (they start without children).
        let mut next: Vec<usize> = vec![0; pages.len()];
        for page in pages {
            let node = match page.parent {
                None => self.tree.create_at(target, first_position),
                Some(parent) => {
                    let parent_node = nodes
                        .get(parent)
                        .copied()
                        .ok_or(ModelError::ParentUnavailable)?;
                    let slot = next.get_mut(parent).ok_or(ModelError::ParentUnavailable)?;
                    let position = *slot;
                    *slot += 1;
                    self.tree
                        .create_at(TreeParentId::Node(parent_node), position)
                }
            }
            .map_err(engine)?;
            nodes.push(node);

            let meta_map = self.tree.get_meta(node).map_err(engine)?;
            meta::set_string(&meta_map, meta::ID, &page.page_id.to_string())?;
            meta::set_string(&meta_map, meta::TITLE, &page.title)?;
            meta::set_int(&meta_map, meta::CREATED_AT, page.created_at)?;
            meta::set_int(&meta_map, meta::UPDATED_AT, page.updated_at)?;
            if let Some(device) = page.created_by {
                meta::set_string(&meta_map, meta::CREATED_BY, &device.to_string())?;
            }
            if let Some(device) = page.edited_by {
                meta::set_string(&meta_map, meta::EDITED_BY, &device.to_string())?;
            }
            if let Some(icon) = page.icon.as_deref() {
                meta::set_string(&meta_map, meta::ICON, icon)?;
            }
            if let Some(cover) = page.cover.as_deref() {
                meta::set_string(&meta_map, meta::COVER, cover)?;
                if let Some(cover_y) = page.cover_y {
                    meta::set_int(&meta_map, meta::COVER_Y, i64::from(cover_y))?;
                }
            }
            meta::set_opt_string(&meta_map, meta::FONT, page.font.and_then(PageFont::stored))?;
            meta::set_flag(&meta_map, meta::SMALL_TEXT, page.small_text)?;
            meta::set_flag(&meta_map, meta::LOCKED, page.locked)?;
            meta::set_flag(&meta_map, meta::TEMPLATE, page.template)?;
            crate::templates::write_values(&meta_map, &page.template_values)?;
            write_props_json(&meta_map, &page.props)?;
            meta::set_int(&meta_map, meta::ARRIVED_AT, now_ms)?;
            meta::set_string(&meta_map, meta::ARRIVED_FROM, &from.to_string())?;
        }
        Ok(pages.iter().map(|page| page.page_id).collect())
    }

    /// Whether [`Self::merge_duplicates`] has anything to do: a node hidden as a duplicate, or a
    /// hidden orphan whose page id is shown already.
    fn has_duplicates(&self) -> bool {
        !self.index.duplicates.is_empty()
            || self
                .index
                .orphans
                .iter()
                .any(|(page, _)| self.index.pages.contains_key(page))
    }

    /// Ids of the orphans and of the pages below them (see [`Self::orphans`]).
    fn orphan_ids(&self) -> Vec<PageId> {
        if self.index.orphans.is_empty() {
            return Vec::new();
        }
        let tombstones = tombstoned(&self.tombstones);
        let mut out = Vec::new();
        for (_, top) in &self.index.orphans {
            let mut stack = vec![*top];
            while let Some(node) = stack.pop() {
                let Some(meta_map) = self.meta_of(node) else {
                    continue;
                };
                let Some(page) = meta::page_id(&meta_map, meta::ID) else {
                    continue;
                };
                if tombstones.contains(&page) || meta::is_residue(&meta_map) {
                    continue;
                }
                out.push(page);
                let children = self
                    .tree
                    .children(TreeParentId::Node(node))
                    .unwrap_or_default();
                stack.extend(children.into_iter().rev());
            }
        }
        out
    }

    /// Findings about residue for `check_invariants`: a root misclassified as moved, residue whose
    /// `movedTo` is not a space id, an orphan whose page id also appears in the tree (re-rooting
    /// it would duplicate the page). A residue id that is also live or trashed is *not* a finding:
    /// the stamp says the page left, and `move_in` removes the residue of a page that came back.
    pub(crate) fn residue_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for root in self.tree.roots() {
            let marked = self
                .meta_of(root)
                .is_some_and(|m| meta::is_moved_marker(&m));
            if marked != self.index.moved_roots.contains(&root) {
                problems.push(format!(
                    "root {} is misclassified as moved",
                    node_label(root)
                ));
            }
        }
        for (page, node) in &self.index.residue_nodes {
            let readable = self
                .meta_of(*node)
                .and_then(|m| meta::string(&m, meta::MOVED_TO))
                .is_some_and(|to| to.parse::<SpaceId>().is_ok());
            if !readable {
                problems.push(format!("residue of page {page} has no readable movedTo"));
            }
        }
        for (page, _) in &self.index.orphans {
            if self.index.pages.contains_key(page) {
                problems.push(format!(
                    "page {page} is hidden below residue without a movedTo stamp and is also in the tree"
                ));
            }
        }
        problems
    }

    /// The `moved` entry of the portable JSON: page id -> `{movedAt, movedTo}` of every residue
    /// (the first node of each page); `None` when there is none, so documents without moves export
    /// exactly as before.
    pub(crate) fn portable_moved(&self) -> Option<serde_json::Value> {
        if self.index.residue.is_empty() {
            return None;
        }
        let mut entries: Vec<(String, &TreeID)> = self
            .index
            .residue
            .iter()
            .map(|(page, node)| (page.to_string(), node))
            .collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut out = serde_json::Map::new();
        for (page, node) in entries {
            let mut obj = serde_json::Map::new();
            if let Some(meta_map) = self.meta_of(*node) {
                if let Some(at) = meta::int(&meta_map, meta::MOVED_AT) {
                    obj.insert("movedAt".into(), at.into());
                }
                if let Some(to) = meta::string(&meta_map, meta::MOVED_TO) {
                    obj.insert("movedTo".into(), to.into());
                }
            }
            out.insert(page, obj.into());
        }
        Some(out.into())
    }
}
