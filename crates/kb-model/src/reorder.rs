//! Reordering the children of one parent in one step (ADR-0052, "Sorting subpages and lists").
//!
//! [`StructureDoc::reorder_children`] puts the live children of a page (or the live roots of the
//! space) into a given order with the fewest tree moves that do it, all in **one commit**: one
//! structural undo step, one persisted update. It is a general reorder, not a sort: the caller
//! (the UI, which owns the collation) says which order it wants.
//!
//! - The order must name exactly the live children of the parent, each once; anything else is
//!   [`ModelError::InvalidOrder`] and nothing is written. A concurrent change (a page added,
//!   moved away or trashed on another device) is how the UI's list can go stale; it asks again.
//! - A reorder is not an edit of the pages: no page gets an `updated_at` or `edited_by` stamp.
//! - Nothing is stored about the sort. The result is plain tree positions, and manual moves stay
//!   as free as before.

use std::collections::{HashMap, HashSet};

use kb_types::PageId;
use loro::{LoroError, LoroTreeError, TreeID, TreeParentId};

use crate::meta::engine;
use crate::{ModelError, StructureDoc};

impl StructureDoc {
    /// Puts the live children of `parent` (`None` = the live roots) in the order `order`.
    /// Returns the pages that were moved, in the order they were moved: empty when the children
    /// already stood in that order, in which case nothing is written.
    ///
    /// The pages that already stand in the right order relative to each other (a longest
    /// increasing run) stay where they are; each of the others is moved right behind the page
    /// that precedes it in `order`. All moves share one commit, which carries no special origin,
    /// so structural undo records the reorder as **one step** and one undo puts every page back.
    /// `updated_at` is not touched.
    ///
    /// Errors, all before anything is written: [`ModelError::ParentUnavailable`] (`parent` is
    /// unknown or in the trash), [`ModelError::InvalidOrder`] (`order` is not the live children
    /// of `parent`: a page missing, unknown, repeated or from elsewhere).
    pub fn reorder_children(
        &mut self,
        parent: Option<PageId>,
        order: &[PageId],
    ) -> Result<Vec<PageId>, ModelError> {
        let parent = match parent {
            None => TreeParentId::Root,
            Some(page) => {
                TreeParentId::Node(self.live_node(page).ok_or(ModelError::ParentUnavailable)?)
            }
        };
        let current = self.visible_children(parent);
        if order.len() != current.len() {
            return Err(ModelError::InvalidOrder);
        }
        let place: HashMap<PageId, (usize, TreeID)> = current
            .iter()
            .enumerate()
            .map(|(at, (node, id))| (*id, (at, *node)))
            .collect();
        let mut seen: HashSet<PageId> = HashSet::with_capacity(order.len());
        // The wanted children as (current position, node, id), in the wanted order.
        let mut wanted: Vec<(usize, TreeID, PageId)> = Vec::with_capacity(order.len());
        for id in order {
            let (at, node) = *place.get(id).ok_or(ModelError::InvalidOrder)?;
            if !seen.insert(*id) {
                return Err(ModelError::InvalidOrder);
            }
            wanted.push((at, node, *id));
        }
        if wanted
            .iter()
            .enumerate()
            .all(|(want, (at, _, _))| want == *at)
        {
            return Ok(Vec::new());
        }

        let positions: Vec<usize> = wanted.iter().map(|(at, _, _)| *at).collect();
        let stay: HashSet<usize> = longest_increasing_run(&positions).into_iter().collect();

        // One commit for all the moves; anything pending is committed first, and no undo group
        // is open while they are written.
        self.close_group();
        self.commit();
        let written = self.write_reorder(parent, &wanted, &stay);
        self.commit();
        match written {
            Ok(moved) => {
                self.settle_index();
                Ok(moved)
            }
            Err(err) => {
                // Some moves may be in; the index follows the document.
                self.rebuild_index();
                Err(err)
            }
        }
    }

    /// Moves every wanted child that is not in `stay` right behind its predecessor in `wanted`
    /// (the first one to the front).
    fn write_reorder(
        &self,
        parent: TreeParentId,
        wanted: &[(usize, TreeID, PageId)],
        stay: &HashSet<usize>,
    ) -> Result<Vec<PageId>, ModelError> {
        let mut moved = Vec::with_capacity(wanted.len().saturating_sub(stay.len()));
        for (want, (_, node, id)) in wanted.iter().enumerate() {
            if stay.contains(&want) {
                continue;
            }
            // The live position of the page that must come right before this one, among the
            // siblings without the page itself.
            let at = match want.checked_sub(1).and_then(|before| wanted.get(before)) {
                None => 0,
                Some((_, before, _)) => self
                    .visible_children(parent)
                    .iter()
                    .filter(|(sibling, _)| sibling != node)
                    .position(|(sibling, _)| sibling == before)
                    .map_or(0, |position| position + 1),
            };
            let position = self.engine_position(parent, Some(*node), Some(at));
            self.tree
                .mov_to(*node, parent, position)
                .map_err(|err| match err {
                    LoroError::TreeError(LoroTreeError::CyclicMoveError) => ModelError::CyclicMove,
                    other => engine(other),
                })?;
            moved.push(*id);
        }
        Ok(moved)
    }
}

/// The indexes (into `values`, ascending) of one longest strictly increasing subsequence of
/// `values`, found by patience sorting in O(n log n).
pub(crate) fn longest_increasing_run(values: &[usize]) -> Vec<usize> {
    // `tails[k]`: the index of the smallest value that ends an increasing run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = vec![None; values.len()];
    for (index, value) in values.iter().enumerate() {
        let length = tails.partition_point(|tail| values.get(*tail).is_some_and(|v| v < value));
        let predecessor = length.checked_sub(1).and_then(|k| tails.get(k)).copied();
        if let Some(entry) = before.get_mut(index) {
            *entry = predecessor;
        }
        match tails.get_mut(length) {
            Some(tail) => *tail = index,
            None => tails.push(index),
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut cursor = tails.last().copied();
    while let Some(index) = cursor {
        run.push(index);
        cursor = before.get(index).copied().flatten();
    }
    run.reverse();
    run
}
