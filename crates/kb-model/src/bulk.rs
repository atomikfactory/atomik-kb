//! Structure changes of many pages in one step (ADR-0058, `docs/architecture/database-power-ups.md`
//! section 5.4): moving several pages under one parent and moving several pages to the trash.
//! (Duplicating several is [`StructureDoc::duplicate_subtrees`]; changing one property of
//! several is [`StructureDoc::set_pages_property`].)
//!
//! Each write here is **one commit** that carries no special origin, so structural undo records it
//! as **one step** and one undo puts every page back, as for [`StructureDoc::reorder_children`].
//! Everything that can be checked is checked before anything is written.
//!
//! The pages must be **tops**: none of them may have an ancestor in the list (a page goes with its
//! subtree, so listing both is a mistake). The caller drops the descendants first with
//! [`StructureDoc::without_descendants`]; a list that still holds one is refused with
//! [`ModelError::InvalidOrder`].

use std::collections::HashSet;

use kb_types::PageId;
use loro::{LoroError, LoroTreeError, TreeID, TreeParentId};

use crate::meta::{self, engine};
use crate::{ModelError, StructureDoc};

impl StructureDoc {
    /// Whether `page_id` is a live page: known, not purged, not in the trash and not below a
    /// trashed page.
    #[must_use]
    pub fn is_live_page(&self, page_id: PageId) -> bool {
        self.live_node(page_id).is_some()
    }

    /// The title of a page (live or trashed), `None` when it is unknown. A cheap read of the page's
    /// node, for the title of a copy (the full [`Self::page`] computes every value).
    #[must_use]
    pub fn page_title(&self, page_id: PageId) -> Option<String> {
        let node = self.index.node(page_id)?;
        Some(meta::string(&self.meta_of(node)?, meta::TITLE).unwrap_or_default())
    }

    /// The pages of `pages` that have no ancestor in `pages`, in the order given (a page listed
    /// twice stays listed twice). A page that is not in the document is kept.
    #[must_use]
    pub fn without_descendants(&self, pages: &[PageId]) -> Vec<PageId> {
        let listed: HashSet<PageId> = pages.iter().copied().collect();
        pages
            .iter()
            .copied()
            .filter(|page| !self.has_ancestor_in(*page, &listed))
            .collect()
    }

    /// Whether a strict ancestor of `page_id` is in `listed`.
    fn has_ancestor_in(&self, page_id: PageId, listed: &HashSet<PageId>) -> bool {
        let Some(node) = self.index.node(page_id) else {
            return false;
        };
        let mut current = node;
        // A tree has no cycle; the bound keeps a damaged document from looping.
        for _ in 0..=self.index.pages.len() + 1 {
            match self.tree.parent(current) {
                Some(TreeParentId::Node(up)) => {
                    if self.index.page(up).is_some_and(|id| listed.contains(&id)) {
                        return true;
                    }
                    current = up;
                }
                _ => return false,
            }
        }
        false
    }

    /// The nodes of live `pages` that are tops, each listed once, in order.
    fn live_tops(&self, pages: &[PageId]) -> Result<Vec<TreeID>, ModelError> {
        let mut seen: HashSet<PageId> = HashSet::with_capacity(pages.len());
        let mut nodes = Vec::with_capacity(pages.len());
        for page in pages {
            let node = self.index.node(*page).ok_or(ModelError::PageNotFound)?;
            if !self.is_live(node) {
                return Err(ModelError::PageTrashed);
            }
            if !seen.insert(*page) {
                return Err(ModelError::InvalidOrder);
            }
            nodes.push(node);
        }
        if self.without_descendants(pages).len() != pages.len() {
            return Err(ModelError::InvalidOrder);
        }
        Ok(nodes)
    }

    /// Moves live `pages` (tops, see the module documentation) to the end of the live children of
    /// `parent` (`None` = the live roots), in the order given, each with its subtree, in **one
    /// commit**. Each page gets the edit stamp [`Self::move_page`] gives. Returns the pages whose
    /// index rows may have changed (every moved page and its subtree, pre-order); empty, with
    /// nothing written, when the pages already stand last under `parent` in that order.
    ///
    /// Errors, all before anything is written: [`ModelError::PageNotFound`],
    /// [`ModelError::PageTrashed`], [`ModelError::InvalidOrder`] (a page listed twice or with an
    /// ancestor in the list), [`ModelError::ParentUnavailable`] (`parent` is unknown or not live),
    /// [`ModelError::CyclicMove`] (`parent` is one of the pages or below one).
    pub fn move_pages_to_end(
        &mut self,
        pages: &[PageId],
        parent: Option<PageId>,
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let nodes = self.live_tops(pages)?;
        let new_parent = match parent {
            None => TreeParentId::Root,
            Some(parent) => TreeParentId::Node(
                self.live_node(parent)
                    .ok_or(ModelError::ParentUnavailable)?,
            ),
        };
        if let TreeParentId::Node(parent_node) = new_parent
            && nodes
                .iter()
                .any(|node| self.is_ancestor(*node, parent_node))
        {
            return Err(ModelError::CyclicMove);
        }
        // Already last, in this order: nothing to write.
        let current = self.visible_children(new_parent);
        if current.len() >= pages.len()
            && current
                .iter()
                .skip(current.len() - pages.len())
                .map(|(_, id)| id)
                .eq(pages.iter())
        {
            return Ok(Vec::new());
        }

        self.close_group();
        self.commit();
        let written = self.write_moves(&nodes, new_parent, now_ms);
        self.commit();
        match written {
            Ok(()) => {
                let affected = nodes.iter().flat_map(|node| self.preorder(*node)).collect();
                self.settle_index();
                Ok(affected)
            }
            Err(err) => {
                // Some moves may be in; the index follows the document.
                self.rebuild_index();
                Err(err)
            }
        }
    }

    /// Moves each node, in order, behind the last child of `parent`, and stamps it.
    fn write_moves(
        &self,
        nodes: &[TreeID],
        parent: TreeParentId,
        now_ms: i64,
    ) -> Result<(), ModelError> {
        for node in nodes {
            let position = self.engine_position(parent, Some(*node), None);
            self.tree
                .mov_to(*node, parent, position)
                .map_err(|err| match err {
                    LoroError::TreeError(LoroTreeError::CyclicMoveError) => ModelError::CyclicMove,
                    other => engine(other),
                })?;
            let meta_map = self.tree.get_meta(*node).map_err(engine)?;
            self.stamp_edit(&meta_map, now_ms)?;
        }
        Ok(())
    }

    /// Moves live `pages` (tops, see the module documentation) with their subtrees to the trash,
    /// in the order given, in **one commit**: what [`Self::trash_page`] does for each. Returns the
    /// pages affected (every trashed page and its subtree, pre-order).
    ///
    /// Errors, all before anything is written: [`ModelError::PageNotFound`],
    /// [`ModelError::PageTrashed`], [`ModelError::InvalidOrder`] (a page listed twice or with an
    /// ancestor in the list).
    pub fn trash_pages(
        &mut self,
        pages: &[PageId],
        now_ms: i64,
    ) -> Result<Vec<PageId>, ModelError> {
        let nodes = self.live_tops(pages)?;
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        self.close_group();
        self.commit();
        let written = self.write_trash(&nodes, now_ms);
        self.commit();
        match written {
            Ok(()) => {
                let affected = nodes.iter().flat_map(|node| self.preorder(*node)).collect();
                self.settle_index();
                Ok(affected)
            }
            Err(err) => {
                self.rebuild_index();
                Err(err)
            }
        }
    }

    /// What [`Self::trash_page`] writes, for each node.
    fn write_trash(&mut self, nodes: &[TreeID], now_ms: i64) -> Result<(), ModelError> {
        let trash = self.trash_root_for_write()?;
        for node in nodes {
            let placement = self.placement(*node).ok_or(ModelError::PageNotFound)?;
            let meta_map = self.tree.get_meta(*node).map_err(engine)?;
            meta::set_int(&meta_map, meta::TRASHED_AT, now_ms)?;
            let from = placement.parent.map(|p| p.to_string());
            meta::set_opt_string(&meta_map, meta::TRASHED_FROM, from.as_deref())?;
            self.stamp_edit(&meta_map, now_ms)?;
            self.tree.mov(*node, trash).map_err(engine)?;
        }
        Ok(())
    }
}
