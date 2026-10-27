//! Reading `.excalidraw` files (ADR-0036, section 3.7): the live elements of a drawing, keyed by
//! element id, ready for the `whiteboards` map of a page document (ADR-0033).

use serde_json::{Map, Value};

/// Most elements kept from one drawing.
const MAX_ELEMENTS: usize = 5_000;
/// The longest element id the page document accepts.
const MAX_ELEMENT_ID: usize = 256;
/// The deepest nesting of one element's JSON that the page document accepts (it stops at 64).
const MAX_ELEMENT_DEPTH: usize = 32;

/// The elements of a drawing.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    /// Live elements by id.
    pub elements: Map<String, Value>,
    /// The drawing holds pictures (their files are not imported, so those elements are left out).
    pub images: bool,
}

/// The live elements of an Excalidraw file: `None` when the text is not an Excalidraw document.
/// Deleted elements, pictures, elements without a usable id and elements nested too deeply are
/// left out; at most 5,000 are kept.
pub(crate) fn elements(text: &str) -> Option<Parsed> {
    let root: Value = serde_json::from_str(text).ok()?;
    let list = root.get("elements")?.as_array()?;
    let mut parsed = Parsed::default();
    for element in list {
        if parsed.elements.len() >= MAX_ELEMENTS {
            break;
        }
        let Some(object) = element.as_object() else {
            continue;
        };
        if object.get("isDeleted").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(id) = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= MAX_ELEMENT_ID)
        else {
            continue;
        };
        if object.get("type").and_then(Value::as_str) == Some("image") {
            parsed.images = true;
            continue;
        }
        if !depth_within(element, MAX_ELEMENT_DEPTH) || parsed.elements.contains_key(id) {
            continue;
        }
        parsed.elements.insert(id.to_owned(), element.clone());
    }
    Some(parsed)
}

/// Whether `value` nests at most `max` levels.
fn depth_within(value: &Value, max: usize) -> bool {
    match value {
        Value::Array(items) => max > 0 && items.iter().all(|item| depth_within(item, max - 1)),
        Value::Object(map) => max > 0 && map.values().all(|item| depth_within(item, max - 1)),
        _ => true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn live_elements_are_kept() {
        let text = json!({
            "type": "excalidraw",
            "version": 2,
            "elements": [
                {"id": "a", "type": "rectangle", "index": "a0"},
                {"id": "b", "type": "text", "text": "hi", "isDeleted": false},
                {"id": "c", "type": "ellipse", "isDeleted": true},
                {"id": "d", "type": "image", "fileId": "f"},
                {"type": "line"},
                {"id": "", "type": "line"},
                {"id": "a", "type": "duplicate"},
                "not an object"
            ],
            "files": {"f": {"dataURL": "data:image/png;base64,AAAA"}}
        })
        .to_string();
        let parsed = elements(&text).expect("a drawing");
        assert_eq!(parsed.elements.keys().collect::<Vec<_>>(), ["a", "b"]);
        assert!(parsed.images);
        assert_eq!(parsed.elements["a"]["type"], "rectangle");
    }

    #[test]
    fn other_documents_are_refused() {
        assert!(elements("").is_none());
        assert!(elements("[]").is_none());
        assert!(elements("{\"elements\": 3}").is_none());
        assert!(elements("{\"type\": \"excalidraw\"}").is_none());
        assert!(elements("{").is_none());
    }

    #[test]
    fn deep_elements_and_floods_are_limited() {
        let mut deep = json!(1);
        for _ in 0..40 {
            deep = json!([deep]);
        }
        let text = json!({"elements": [
            {"id": "deep", "type": "line", "points": deep},
            {"id": "ok", "type": "line", "points": [[0, 0], [1, 1]]}
        ]})
        .to_string();
        let parsed = elements(&text).expect("a drawing");
        assert_eq!(parsed.elements.keys().collect::<Vec<_>>(), ["ok"]);

        let many: Vec<Value> = (0..6_000)
            .map(|n| json!({"id": format!("e{n}"), "type": "line"}))
            .collect();
        let text = json!({ "elements": many }).to_string();
        assert_eq!(elements(&text).expect("a drawing").elements.len(), 5_000);
    }
}
