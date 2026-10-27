//! The elements of whiteboard blocks (ADR-0016, amendment of 2026-10-10; ADR-0033).
//!
//! A `whiteboard` node holds no elements. They live in a **separate root map** of the same
//! Loro document, `whiteboards`, as one plain map value per element under the flat key
//! `<block id>/<element id>` (the block id is a 36-character UUID, so the first `/` after
//! it splits the key). The value is the Excalidraw element as JSON (`id`, `type`, `index`,
//! `version`, `versionNonce`, `isDeleted`, ...). Two devices that edit different elements
//! write different keys and merge without conflict; two that edit the same element keep one
//! value (last writer wins per element, the editor reconciles by `version`). No container
//! is ever created concurrently (a flat key space in an always-existing root), which is why
//! there is no mergeable container here.
//!
//! Readers of shape v1 go by the root `doc` and never look at `whiteboards`, so an older
//! client leaves the map alone. Orphans (elements whose block is gone) are kept so that
//! undoing the deletion of the block brings the drawing back;
//! [`super::PageDoc::prune_orphan_whiteboards`] removes them and is called by nothing yet.

use std::collections::{BTreeMap, HashSet};

use kb_types::BlockId;
use loro::{LoroValue, ValueOrContainer};
use serde_json::Value;

use super::portable::{json_value, loro_value};
use super::{Mode, PageDoc, portable, shape};
use crate::ModelError;
use crate::meta::engine;

/// Longest element id accepted in a key.
const MAX_ELEMENT_ID: usize = 256;
/// Length of a canonical UUID string.
const UUID_LEN: usize = 36;

/// One element of a whiteboard: its Excalidraw id and the stored value.
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteboardElement {
    /// The element's id (the part of the key after the block id).
    pub id: String,
    /// The element as stored, as JSON (keys sorted, integral numbers as integers).
    pub value: Value,
}

impl WhiteboardElement {
    /// Excalidraw's `isDeleted` tombstone flag.
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.value.get("isDeleted").and_then(Value::as_bool) == Some(true)
    }

    /// The text of a live `text` element: `originalText` when it is a string, else `text`.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        if self.is_deleted() || self.value.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        self.value
            .get("originalText")
            .and_then(Value::as_str)
            .or_else(|| self.value.get("text").and_then(Value::as_str))
    }

    fn index(&self) -> &str {
        self.value
            .get("index")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// Splits `<block id>/<element id>`.
fn split_key(key: &str) -> Option<(BlockId, &str)> {
    let (block, rest) = key.split_at_checked(UUID_LEN)?;
    let element = rest.strip_prefix('/')?;
    if element.is_empty() || element.len() > MAX_ELEMENT_ID {
        return None;
    }
    Some((block.parse().ok()?, element))
}

/// The key of an element.
#[must_use]
fn element_key(block: BlockId, element: &str) -> String {
    format!("{block}/{element}")
}

/// The text of a whiteboard as the text index and the Markdown export read it: the live text
/// elements in z-order, each with its whitespace collapsed to single spaces, joined by one
/// space (the Markdown export keeps them apart, one per list item).
pub(super) fn texts(elements: &[WhiteboardElement]) -> Vec<String> {
    collapse(elements.iter().filter_map(WhiteboardElement::text))
}

/// [`texts`] for the portable form: the elements of one block as `{ id: element }`, sorted here.
pub(super) fn texts_of_portable(elements: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut list: Vec<WhiteboardElement> = elements
        .iter()
        .map(|(id, value)| WhiteboardElement {
            id: id.clone(),
            value: value.clone(),
        })
        .collect();
    sort_z(&mut list);
    texts(&list)
}

fn collapse<'a>(texts: impl Iterator<Item = &'a str>) -> Vec<String> {
    texts
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty())
        .collect()
}

impl PageDoc {
    /// Every plain entry of the `whiteboards` root map, by key.
    fn whiteboard_entries(&self) -> BTreeMap<String, LoroValue> {
        let map = self.doc.inner().get_map(shape::WHITEBOARDS);
        let mut out = BTreeMap::new();
        map.for_each(|key, value| {
            if let ValueOrContainer::Value(value) = value {
                out.insert(key.to_owned(), value);
            }
        });
        out
    }

    /// The elements of the whiteboard `block` (tombstones included), in z-order: by the
    /// fractional `index` string, then by id. Entries whose value is not a map with a string
    /// `id` equal to the key's element id are left out (`check_invariants` reports them).
    #[must_use]
    pub fn whiteboard_elements(&self, block: BlockId) -> Vec<WhiteboardElement> {
        let mut out: Vec<WhiteboardElement> = self
            .whiteboard_entries()
            .into_iter()
            .filter_map(|(key, value)| {
                let (owner, id) = split_key(&key)?;
                (owner == block).then_some((id.to_owned(), value))
            })
            .filter_map(|(id, value)| well_formed(&id, &value))
            .collect();
        sort_z(&mut out);
        out
    }

    /// The elements of every whiteboard block that has a node in the page, as a portable
    /// value `{ "<block id>": { "<element id>": element } }` (blocks without elements left
    /// out). Orphans are not part of it.
    pub(super) fn whiteboards_portable(
        &self,
        live: &HashSet<BlockId>,
    ) -> serde_json::Map<String, Value> {
        let mut out: serde_json::Map<String, Value> = serde_json::Map::new();
        for (key, value) in self.whiteboard_entries() {
            let Some((owner, id)) = split_key(&key) else {
                continue;
            };
            if !live.contains(&owner) {
                continue;
            }
            let Some(element) = well_formed(id, &value) else {
                continue;
            };
            let slot = out
                .entry(owner.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(slot) = slot {
                slot.insert(element.id, element.value);
            }
        }
        out
    }

    /// Ids of the `whiteboard` nodes of the page.
    pub(super) fn whiteboard_blocks(&self) -> HashSet<BlockId> {
        portable::node_ids(
            &portable::document(self.read(Mode::Structure).root.as_ref()),
            shape::WHITEBOARD,
        )
    }

    /// Stores (or replaces) one element of the whiteboard `block`. `element` must be a JSON
    /// object with a string `id`; the editor's writer does the same in TypeScript.
    pub fn set_whiteboard_element(
        &self,
        block: BlockId,
        element: &Value,
    ) -> Result<(), ModelError> {
        let Some(id) = element
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= MAX_ELEMENT_ID)
        else {
            return Err(ModelError::Shape("a whiteboard element has no id".into()));
        };
        let value = loro_value(element, 0)?;
        self.doc
            .inner()
            .get_map(shape::WHITEBOARDS)
            .insert(&element_key(block, id), value)
            .map_err(engine)?;
        self.commit();
        Ok(())
    }

    /// Keys of the elements whose block has no node in the page (any more).
    #[must_use]
    pub fn orphan_whiteboard_keys(&self) -> Vec<String> {
        let live = self.whiteboard_blocks();
        self.whiteboard_entries()
            .into_keys()
            .filter(|key| split_key(key).is_none_or(|(owner, _)| !live.contains(&owner)))
            .collect()
    }

    /// Deletes the elements of blocks that have no node in the page, and returns how many.
    ///
    /// Not called by the core in v1: deleting a block must stay undoable, and a replica
    /// cannot know that no other device still holds an undo step or an offline edit for it
    /// (ADR-0033, "orphans").
    pub fn prune_orphan_whiteboards(&self) -> Result<usize, ModelError> {
        let keys = self.orphan_whiteboard_keys();
        let map = self.doc.inner().get_map(shape::WHITEBOARDS);
        for key in &keys {
            map.delete(key).map_err(engine)?;
        }
        if !keys.is_empty() {
            self.commit();
        }
        Ok(keys.len())
    }

    /// Problems of the `whiteboards` root map, for `check_invariants`: a key that is not
    /// `<block id>/<element id>`, a value that is not a map, an `id` that differs from the
    /// key's. Messages hold block ids only, never element content.
    pub(super) fn whiteboard_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut malformed_keys = 0usize;
        for (key, value) in self.whiteboard_entries() {
            match split_key(&key) {
                None => malformed_keys += 1,
                Some((owner, id)) => {
                    if well_formed(id, &value).is_none() {
                        problems.push(format!("whiteboard {owner} has a malformed element"));
                    }
                }
            }
        }
        if malformed_keys > 0 {
            problems.push(format!(
                "whiteboards has {malformed_keys} entries with a malformed key"
            ));
        }
        problems
    }

    /// Writes the elements of a portable `whiteboards` value into the document. Anything that
    /// is not `{ block id: { element id: element with that id } }` is a shape error.
    pub(super) fn import_whiteboards(
        &self,
        whiteboards: &serde_json::Map<String, Value>,
    ) -> Result<(), ModelError> {
        let mut writes = Vec::new();
        for (block, elements) in whiteboards {
            let block: BlockId = block
                .parse()
                .map_err(|_| shape_error("a whiteboard key is not a block id"))?;
            let Value::Object(elements) = elements else {
                return Err(shape_error("a whiteboard is not an object"));
            };
            for (id, element) in elements {
                let key = element_key(block, id);
                if element.get("id").and_then(Value::as_str) != Some(id.as_str())
                    || split_key(&key).is_none()
                {
                    return Err(shape_error("a whiteboard element has a wrong id"));
                }
                writes.push((key, loro_value(element, 0)?));
            }
        }
        let map = self.doc.inner().get_map(shape::WHITEBOARDS);
        for (key, value) in writes {
            map.insert(&key, value).map_err(engine)?;
        }
        Ok(())
    }
}

/// Z-order: the fractional `index` string, then the id.
fn sort_z(elements: &mut [WhiteboardElement]) {
    elements.sort_by(|a, b| a.index().cmp(b.index()).then_with(|| a.id.cmp(&b.id)));
}

fn shape_error(message: &str) -> ModelError {
    ModelError::Shape(format!("portable JSON: {message}"))
}

/// The element when `value` is a map whose `id` is `id`.
fn well_formed(id: &str, value: &LoroValue) -> Option<WhiteboardElement> {
    let LoroValue::Map(map) = value else {
        return None;
    };
    match map.get("id") {
        Some(LoroValue::String(own)) if own.as_str() == id => {}
        _ => return None,
    }
    Some(WhiteboardElement {
        id: id.to_owned(),
        value: json_value(value, 0),
    })
}
