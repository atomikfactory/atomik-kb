//! The in-memory `PageId -> TreeID` index of a structure document.
//!
//! Only *visible* pages are indexed: nodes reachable from a tree root through
//! nodes that carry a valid, unique, non-tombstoned page id. Everything else is
//! hidden together with its subtree:
//!
//! - deleted nodes (purged pages, or pages whose creation was undone) are not
//!   reachable from a root in Loro's state;
//! - a purged page revived by a concurrent move (Loro's movable tree lets a move
//!   with a higher lamport win over a delete) stays purged: the tombstone wins;
//! - nodes without a valid page id, duplicate ids and misplaced trash markers are
//!   foreign or damaged data; they are reported as anomalies
//!   ([`crate::StructureDoc::check_invariants`]).
//!
//! Moves between spaces (ADR-0038, `moves.rs`) add a second kind of hidden node, **residue**: a
//! node that carries a `movedTo` stamp (wherever it sits; normally under a root whose meta has
//! `kind = "moved"`). Residue is neither live nor trashed: it is not in `pages`, it is listed in
//! `residue_nodes` (and, first one wins, in `residue`), and its subtree is walked only to find
//! **orphans**: page nodes without a stamp that another device created under a page that moved
//! away. An orphan and everything below it stays hidden until `rehome_orphans` puts it back on
//! the live root. An older build treats the moved root as a root without a page id, so it skips
//! the whole subtree.
//!
//! `TreeID`s are not stable across undo/redo (Loro re-creates deleted nodes under
//! new ids and copies their metadata), so the index is rebuilt from the `id` stored
//! in node metadata after every import, undo and redo.

use std::collections::{HashMap, HashSet};

use kb_types::PageId;
use loro::{LoroTree, TreeID, TreeParentId};

use crate::meta;

/// Where a walked node sits: in the live tree or the trash (`Visible`), or inside residue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Visible,
    Residue,
}

#[derive(Debug, Default)]
pub(crate) struct PageIndex {
    /// Visible pages.
    pub(crate) pages: HashMap<PageId, TreeID>,
    /// Reverse of `pages`.
    pub(crate) nodes: HashMap<TreeID, PageId>,
    /// Roots whose metadata has `kind = "trash"`, in root order.
    pub(crate) trash_roots: Vec<TreeID>,
    /// Roots whose metadata has `kind = "moved"`, in root order.
    pub(crate) moved_roots: Vec<TreeID>,
    /// Residue: page id -> node; the first one found wins (live roots, then the trash, then the
    /// moved roots, each in tree order).
    pub(crate) residue: HashMap<PageId, TreeID>,
    /// Every residue node found, duplicates included, in the order they were found.
    pub(crate) residue_nodes: Vec<(PageId, TreeID)>,
    /// The topmost page nodes hidden inside residue without a stamp (see the module
    /// documentation), in the order they were found.
    pub(crate) orphans: Vec<(PageId, TreeID)>,
    /// The visible pages that arrived from another space (`arrivedAt`), as of the last build (not
    /// maintained incrementally; `moves.rs` uses it to keep undo from losing them).
    pub(crate) arrived: Vec<(PageId, TreeID)>,
    /// The topmost visible-zone nodes whose page id another node (found earlier) already holds:
    /// the ones the index hides as duplicates, with their subtrees (`merge_duplicates`, `moves.rs`).
    pub(crate) duplicates: Vec<(PageId, TreeID)>,
    /// Damaged-data findings of the last build (not maintained incrementally).
    pub(crate) anomalies: Vec<String>,
}

impl PartialEq for PageIndex {
    fn eq(&self, other: &Self) -> bool {
        self.pages == other.pages
            && self.trash_roots == other.trash_roots
            && self.moved_roots == other.moved_roots
            && self.residue_nodes == other.residue_nodes
            && self.orphans == other.orphans
            && self.duplicates == other.duplicates
    }
}

impl PageIndex {
    /// Builds the index from the tree: live roots first, then the trash, then the residue.
    pub(crate) fn build(tree: &LoroTree, tombstones: &HashSet<PageId>) -> Self {
        let mut index = Self::default();
        let mut live_roots = Vec::new();
        for root in tree.roots() {
            let meta = tree.get_meta(root).ok();
            if meta.as_ref().is_some_and(meta::is_trash_marker) {
                index.trash_roots.push(root);
            } else if meta.as_ref().is_some_and(meta::is_moved_marker) {
                index.moved_roots.push(root);
            } else {
                live_roots.push(root);
            }
        }
        for root in live_roots {
            index.visit(tree, root, Zone::Visible, tombstones);
        }
        for trash in index.trash_roots.clone() {
            for child in tree.children(TreeParentId::Node(trash)).unwrap_or_default() {
                index.visit(tree, child, Zone::Visible, tombstones);
            }
        }
        for moved in index.moved_roots.clone() {
            for child in tree.children(TreeParentId::Node(moved)).unwrap_or_default() {
                index.visit(tree, child, Zone::Residue, tombstones);
            }
        }
        index
    }

    /// Indexes `start` and its subtree (pre-order, iterative).
    fn visit(&mut self, tree: &LoroTree, start: TreeID, zone: Zone, tombstones: &HashSet<PageId>) {
        let mut stack = vec![(start, zone)];
        while let Some((node, zone)) = stack.pop() {
            let Ok(meta) = tree.get_meta(node) else {
                self.anomalies
                    .push(format!("node {} has no metadata", node_label(node)));
                continue;
            };
            if meta::is_trash_marker(&meta) {
                self.anomalies.push(format!(
                    "trash marker on non-root node {}",
                    node_label(node)
                ));
                continue;
            }
            if meta::is_moved_marker(&meta) {
                self.anomalies.push(format!(
                    "moved marker on non-root node {}",
                    node_label(node)
                ));
                continue;
            }
            let Some(page_id) = meta::page_id(&meta, meta::ID) else {
                self.anomalies
                    .push(format!("node {} has no valid page id", node_label(node)));
                continue;
            };
            if tombstones.contains(&page_id) {
                continue;
            }
            let children = tree.children(TreeParentId::Node(node)).unwrap_or_default();
            if meta::is_residue(&meta) {
                // Residue, wherever it sits: hidden; what hangs below is looked at for orphans.
                self.residue_nodes.push((page_id, node));
                self.residue.entry(page_id).or_insert(node);
                stack.extend(children.into_iter().rev().map(|c| (c, Zone::Residue)));
                continue;
            }
            if zone == Zone::Residue {
                // A page created (or moved) under a page that moved away: it and its subtree
                // wait for `rehome_orphans`.
                self.orphans.push((page_id, node));
                continue;
            }
            if self.pages.contains_key(&page_id) {
                self.anomalies.push(format!("duplicate page id {page_id}"));
                self.duplicates.push((page_id, node));
                continue;
            }
            self.insert(page_id, node);
            if meta::int(&meta, meta::ARRIVED_AT).is_some() {
                self.arrived.push((page_id, node));
            }
            stack.extend(children.into_iter().rev().map(|c| (c, Zone::Visible)));
        }
    }

    pub(crate) fn insert(&mut self, page_id: PageId, node: TreeID) {
        self.pages.insert(page_id, node);
        self.nodes.insert(node, page_id);
    }

    pub(crate) fn remove(&mut self, page_id: PageId) {
        if let Some(node) = self.pages.remove(&page_id) {
            self.nodes.remove(&node);
        }
    }

    pub(crate) fn node(&self, page_id: PageId) -> Option<TreeID> {
        self.pages.get(&page_id).copied()
    }

    pub(crate) fn page(&self, node: TreeID) -> Option<PageId> {
        self.nodes.get(&node).copied()
    }

    pub(crate) fn is_trash_root(&self, node: TreeID) -> bool {
        self.trash_roots.contains(&node)
    }
}

/// Tombstoned page ids (keys of the `tombstones` map that parse as page ids).
pub(crate) fn tombstoned(tombstones: &loro::LoroMap) -> HashSet<PageId> {
    tombstones.keys().filter_map(|k| k.parse().ok()).collect()
}

/// A loggable label for a node: engine ids only (random peer + counter), never
/// user content.
pub(crate) fn node_label(node: TreeID) -> String {
    format!("{}@{:x}", node.counter, node.peer)
}
