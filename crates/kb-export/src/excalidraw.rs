//! Whiteboards as `.excalidraw` files (`docs/architecture/import-export.md`, "Whiteboards").
//!
//! A whiteboard block holds no elements: they live in the page's `whiteboards` map, one entry
//! per element under `<blockId>/<elementId>`, which the portable JSON shows as
//! `{"whiteboards": {"<blockId>": {"<elementId>": element}}}`. An Excalidraw scene is that block's
//! live elements (not `isDeleted`) in z-order: the elements' fractional `index`, then their id.

use std::collections::HashSet;

use kb_types::BlockId;
use serde_json::Value;

use crate::json::Node;

/// One `.excalidraw` scene per whiteboard block of the page, in document order: the live
/// elements of the block from the portable JSON's top-level `whiteboards` value, in z-order.
/// A block without elements still gets a scene (with no elements), so the block survives a
/// re-import. A block whose `id` is not a block id has none.
///
/// The scene is `{"type":"excalidraw","version":2,"source":"atomik-kb","elements":[...],
/// "appState":{"viewBackgroundColor":"#ffffff"},"files":{}}`, pretty-printed (two-space indent,
/// those keys in that order, the keys of each element sorted) and ends with a newline.
#[must_use]
pub fn whiteboard_files(content: &Value) -> Vec<(BlockId, String)> {
    let whiteboards = content.get("whiteboards").unwrap_or(&Value::Null);
    let mut seen: HashSet<BlockId> = HashSet::new();
    let mut files = Vec::new();
    for key in whiteboard_blocks(content) {
        let Ok(block) = key.parse::<BlockId>() else {
            continue;
        };
        if !seen.insert(block) {
            continue;
        }
        let elements = live_elements(whiteboards, key);
        files.push((block, scene(&elements)));
    }
    files
}

/// The `id` attribute of every `whiteboard` node under `content`, in document order.
fn whiteboard_blocks(content: &Value) -> Vec<&str> {
    let mut ids = Vec::new();
    let mut stack = vec![content];
    while let Some(node) = stack.pop() {
        if node.get("type").and_then(Value::as_str) == Some("whiteboard")
            && let Some(id) = node
                .get("attrs")
                .and_then(|attrs| attrs.get("id"))
                .and_then(Value::as_str)
        {
            ids.push(id);
        }
        if let Some(children) = node.get("content").and_then(Value::as_array) {
            stack.extend(children.iter().rev());
        }
    }
    ids
}

/// The live elements of the whiteboard `block` (its `id` as stored) in z-order, with their ids.
pub(crate) fn live_elements<'a>(whiteboards: &'a Value, block: &str) -> Vec<(&'a str, &'a Value)> {
    let Some(elements) = whiteboards.get(block).and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut live: Vec<(&str, &Value)> = elements
        .iter()
        .filter(|(_, element)| element.get("isDeleted").and_then(Value::as_bool) != Some(true))
        .map(|(id, element)| (id.as_str(), element))
        .collect();
    live.sort_by(|(a_id, a), (b_id, b)| index(a).cmp(index(b)).then_with(|| a_id.cmp(b_id)));
    live
}

/// The fractional z-order index of an element (empty when it has none).
fn index(element: &Value) -> &str {
    element
        .get("index")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The text of the live text elements of a whiteboard in z-order, each with its white space
/// collapsed to single spaces (`originalText` when it is a string, else `text`); empty ones are
/// left out.
pub(crate) fn texts(whiteboards: &Value, block: &str) -> Vec<String> {
    live_elements(whiteboards, block)
        .into_iter()
        .filter(|(_, element)| element.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|(_, element)| {
            element
                .get("originalText")
                .and_then(Value::as_str)
                .or_else(|| element.get("text").and_then(Value::as_str))
        })
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty())
        .collect()
}

/// The scene file of `elements`.
fn scene(elements: &[(&str, &Value)]) -> String {
    let document = Node::object([
        ("type", Node::string("excalidraw")),
        ("version", Node::Scalar(2.into())),
        ("source", Node::string("atomik-kb")),
        (
            "elements",
            Node::Array(
                elements
                    .iter()
                    .map(|(_, element)| Node::sorted(element))
                    .collect(),
            ),
        ),
        (
            "appState",
            Node::object([("viewBackgroundColor", Node::string("#ffffff"))]),
        ),
        ("files", Node::Object(Vec::new())),
    ]);
    let mut text = document.pretty();
    text.push('\n');
    text
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn bid(n: u8) -> BlockId {
        BlockId::from_random_bytes([n; 16])
    }

    fn board(n: u8) -> Value {
        json!({ "type": "whiteboard", "attrs": { "id": bid(n).to_string(), "height": 360 } })
    }

    fn element(id: &str, index: &str, deleted: Option<bool>) -> Value {
        let mut value = json!({ "id": id, "type": "rectangle", "index": index, "x": 1, "y": 2.5 });
        if let Some(deleted) = deleted {
            value["isDeleted"] = json!(deleted);
        }
        value
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_scene_has_the_excalidraw_envelope() {
        let content = json!({
            "type": "doc",
            "content": [board(1)],
            "whiteboards": { bid(1).to_string(): { "a": element("a", "a0", None) } }
        });
        let files = whiteboard_files(&content);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, bid(1));
        let text = &files[0].1;
        assert!(text.ends_with("}\n"));
        assert_eq!(
            parse(text),
            json!({
                "type": "excalidraw",
                "version": 2,
                "source": "atomik-kb",
                "elements": [{ "id": "a", "type": "rectangle", "index": "a0", "x": 1, "y": 2.5 }],
                "appState": { "viewBackgroundColor": "#ffffff" },
                "files": {}
            })
        );
        // Pretty, in the documented order, the keys of an element sorted.
        assert_eq!(
            text,
            "{\n  \"type\": \"excalidraw\",\n  \"version\": 2,\n  \"source\": \"atomik-kb\",\n  \"elements\": [\n    {\n      \"id\": \"a\",\n      \"index\": \"a0\",\n      \"type\": \"rectangle\",\n      \"x\": 1,\n      \"y\": 2.5\n    }\n  ],\n  \"appState\": {\n    \"viewBackgroundColor\": \"#ffffff\"\n  },\n  \"files\": {}\n}\n"
        );
    }

    #[test]
    fn elements_are_in_z_order_and_the_deleted_are_gone() {
        let content = json!({
            "type": "doc",
            "content": [board(1)],
            "whiteboards": { bid(1).to_string(): {
                "z": element("z", "a1", None),
                "b": element("b", "a1", Some(false)),
                "gone": element("gone", "a0", Some(true)),
                "top": element("top", "b0", None),
                "bottom": element("bottom", "Zz", None),
                "noindex": { "id": "noindex", "type": "line" },
                "string": element("string", "a2", None),
            } }
        });
        let files = whiteboard_files(&content);
        let scene = parse(&files[0].1);
        let ids: Vec<&str> = scene["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|element| element["id"].as_str().unwrap())
            .collect();
        // Fractional indices compare as strings (`Zz` before `a0`), ties by id; no index first.
        assert_eq!(ids, ["noindex", "bottom", "b", "z", "string", "top"]);
        // Only a boolean `true` is a tombstone.
        let odd = json!({
            "type": "doc",
            "content": [board(1)],
            "whiteboards": { bid(1).to_string(): { "x": { "id": "x", "index": "a0", "isDeleted": "true" } } }
        });
        assert_eq!(
            parse(&whiteboard_files(&odd)[0].1)["elements"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_block_without_elements_still_gets_a_scene() {
        let none = json!({ "type": "doc", "content": [board(1)] });
        let files = whiteboard_files(&none);
        assert_eq!(files.len(), 1);
        let scene = parse(&files[0].1);
        assert_eq!(scene["elements"], json!([]));
        assert!(files[0].1.contains("\"elements\": [],"));
        // All its elements deleted is the same.
        let deleted = json!({
            "type": "doc",
            "content": [board(1)],
            "whiteboards": { bid(1).to_string(): { "a": element("a", "a0", Some(true)) } }
        });
        assert_eq!(
            parse(&whiteboard_files(&deleted)[0].1)["elements"],
            json!([])
        );
    }

    #[test]
    fn scenes_come_in_document_order_at_any_depth() {
        let content = json!({
            "type": "doc",
            "content": [
                board(3),
                { "type": "columnList", "content": [
                    { "type": "column", "content": [board(1)] },
                    { "type": "column", "content": [
                        { "type": "callout", "content": [board(2)] }
                    ] }
                ] },
                board(4),
            ],
            "whiteboards": {
                bid(1).to_string(): { "a": element("a", "a0", None) },
                bid(2).to_string(): { "b": element("b", "a0", None) },
                bid(9).to_string(): { "orphan": element("orphan", "a0", None) },
            }
        });
        let files = whiteboard_files(&content);
        let blocks: Vec<BlockId> = files.iter().map(|(block, _)| *block).collect();
        assert_eq!(blocks, [bid(3), bid(1), bid(2), bid(4)]);
        // Each scene holds its own block's elements; the orphan is nobody's.
        let ids = |at: usize| -> Vec<String> {
            parse(&files[at].1)["elements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|element| element["id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert!(ids(0).is_empty());
        assert_eq!(ids(1), ["a"]);
        assert_eq!(ids(2), ["b"]);
        assert!(ids(3).is_empty());
    }

    #[test]
    fn blocks_without_a_usable_id_and_repeats_are_skipped() {
        let content = json!({
            "type": "doc",
            "content": [
                { "type": "whiteboard" },
                { "type": "whiteboard", "attrs": {} },
                { "type": "whiteboard", "attrs": { "id": "not an id" } },
                { "type": "whiteboard", "attrs": { "id": 5 } },
                board(1),
                board(1),
                { "type": "paragraph", "content": [{ "type": "text", "text": "whiteboard" }] },
            ]
        });
        let files = whiteboard_files(&content);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, bid(1));
        assert!(whiteboard_files(&json!({ "type": "doc" })).is_empty());
        assert!(whiteboard_files(&json!(null)).is_empty());
    }

    #[test]
    fn the_texts_of_a_whiteboard_follow_the_markdown_rules() {
        let whiteboards = json!({
            "block": {
                "a": { "id": "a", "type": "text", "index": "a1", "originalText": "Plan A\nship  it", "text": "ignored" },
                "b": { "id": "b", "type": "text", "index": "a0", "text": "first" },
                "c": { "id": "c", "type": "text", "index": "a2", "originalText": "  \n ", "text": "blank" },
                "d": { "id": "d", "type": "rectangle", "index": "a3", "text": "not text" },
                "e": { "id": "e", "type": "text", "index": "a4", "isDeleted": true, "text": "gone" },
            }
        });
        assert_eq!(texts(&whiteboards, "block"), ["first", "Plan A ship it"]);
        assert!(texts(&whiteboards, "other").is_empty());
        assert!(texts(&Value::Null, "block").is_empty());
    }
}
