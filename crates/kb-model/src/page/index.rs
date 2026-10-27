//! The block-id index of a page document.
//!
//! The semantic operations and [`super::PageDoc::block_ids`] need to find blocks by
//! id; reading the whole document for every call made appending N blocks cost
//! O(N^2) engine calls (the core's tests append 2,000 paragraphs, calling
//! `block_ids` before each insert). The index caches the element structure of the
//! document, without text, keyed by the engine's state frontiers: it is valid
//! exactly while the document is unchanged. Local operations of [`super::PageDoc`]
//! patch it after their commit (re-reading only the top-level blocks they touched);
//! anything else (an import, a raw edit) changes the frontiers and the next use
//! rebuilds it from the document. It is never persisted.

use kb_types::BlockId;
use loro::{Frontiers, LoroMap};

use super::read::{self, Child, Element, Mode};

/// A node with a valid id.
#[derive(Clone)]
pub(super) struct Entry {
    /// Its block id.
    pub id: BlockId,
    /// Its `nodeName`.
    pub name: String,
    /// Its engine container.
    pub map: LoroMap,
}

/// One entry of the root's children list.
pub(super) struct Top {
    /// The node's container; `None` for a text run or a foreign entry.
    pub map: Option<LoroMap>,
    /// The node's own id, when it has a valid one.
    pub own: Option<BlockId>,
    /// Nodes with a valid id in this subtree, pre-order (the node itself first).
    pub nodes: Vec<Entry>,
}

/// Element structure of a document at one version.
pub(super) struct Index {
    /// The state the index describes.
    pub frontiers: Frontiers,
    /// The root's own id entry (the root normally has no id).
    pub root: Option<Entry>,
    /// One entry per entry of the root's children list, in order.
    pub tops: Vec<Top>,
}

impl Top {
    /// Indexes one entry of the root's children list.
    pub fn of(child: &Child) -> Self {
        match child {
            Child::Element(element) => {
                let mut nodes = Vec::new();
                collect(element, &mut nodes);
                Self {
                    map: Some(element.map.clone()),
                    own: element.block_id(),
                    nodes,
                }
            }
            Child::Text(_) | Child::Skipped => Self {
                map: None,
                own: None,
                nodes: Vec::new(),
            },
        }
    }

    /// Re-reads a top-level node from the engine.
    pub fn read(map: &LoroMap) -> Self {
        match read::read_element(map, 1, Mode::Structure) {
            Some(element) => Self::of(&Child::Element(element)),
            None => Self::of(&Child::Skipped),
        }
    }
}

/// Pre-order entries of `element`'s subtree (recursion is bounded by the reader's
/// depth limit).
fn collect(element: &Element, out: &mut Vec<Entry>) {
    if let Some(id) = element.block_id() {
        out.push(Entry {
            id,
            name: element.name.clone(),
            map: element.map.clone(),
        });
    }
    for child in &element.children {
        if let Child::Element(child) = child {
            collect(child, out);
        }
    }
}

impl Index {
    /// Indexes the document under `root` as it is at `frontiers`.
    pub fn build(root: &LoroMap, frontiers: Frontiers) -> Self {
        let tree = read::read(root, Mode::Structure);
        let mut index = Self {
            frontiers,
            root: None,
            tops: Vec::new(),
        };
        if let Some(root) = &tree.root {
            index.root = root.block_id().map(|id| Entry {
                id,
                name: root.name.clone(),
                map: root.map.clone(),
            });
            index.tops = root.children.iter().map(Top::of).collect();
        }
        index
    }

    /// Every valid id, pre-order (duplicates included).
    pub fn ids(&self) -> Vec<BlockId> {
        self.root
            .iter()
            .chain(self.tops.iter().flat_map(|top| top.nodes.iter()))
            .map(|entry| entry.id)
            .collect()
    }

    /// The first node with `id` in document order, with the position of the
    /// top-level entry holding it (`None` for the root itself).
    pub fn find(&self, id: BlockId) -> Option<(Option<usize>, &Entry)> {
        if let Some(root) = self.root.as_ref().filter(|root| root.id == id) {
            return Some((None, root));
        }
        self.tops.iter().enumerate().find_map(|(pos, top)| {
            top.nodes
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| (Some(pos), entry))
        })
    }

    /// Whether any node has `id`.
    pub fn contains(&self, id: BlockId) -> bool {
        self.find(id).is_some()
    }

    /// Position of the first top-level node whose own id is `id`.
    pub fn top_position(&self, id: BlockId) -> Option<usize> {
        self.tops.iter().position(|top| top.own == Some(id))
    }

    /// Number of top-level element nodes.
    pub fn top_elements(&self) -> usize {
        self.tops.iter().filter(|top| top.map.is_some()).count()
    }
}
