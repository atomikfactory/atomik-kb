//! Helpers shared by the tests of several modules.

use serde_json::Value;

use crate::ctx::SourceIndex;

/// The id Notion puts at the end of the name of `Sub Page`.
pub(crate) const SUB_ID: &str = "0123456789abcdef0123456789abcdef";

/// A source with a few pages and files, as seen from `Notes/Page.md` (page 1):
/// `Other.md` is page 2, `Notes/Sub Page <id>.md` page 3.
pub(crate) fn sample_index() -> SourceIndex {
    let mut index = SourceIndex::default();
    for (path, id) in [
        ("Notes/Page.md", 1),
        ("Other.md", 2),
        (&format!("Notes/Sub Page {SUB_ID}.md"), 3),
    ] {
        index.pages.insert(path.to_owned(), id);
        index
            .pages_lower
            .entry(path.to_lowercase())
            .or_default()
            .push(id);
    }
    index.notion.insert(SUB_ID.to_owned(), 3);
    for name in ["diagram.png", "report.pdf", "photo.jpg"] {
        let norm = format!("Notes/{name}");
        index
            .binary_lower
            .entry(norm.to_lowercase())
            .or_default()
            .push(norm.clone());
        index
            .binary_names
            .entry(name.to_lowercase())
            .or_default()
            .push(norm.clone());
        index.binary.insert(norm.clone(), format!("Export/{norm}"));
    }
    index.whiteboards.insert(
        "Notes/Board.excalidraw".to_owned(),
        r#"{"type":"excalidraw","elements":[{"id":"a","type":"rectangle","index":"a0"}]}"#
            .to_owned(),
    );
    index
}

/// The nesting of nodes (what the page document limits), not of JSON containers.
pub(crate) fn node_depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => items.iter().map(node_depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.get("content").map_or(0, node_depth),
        _ => 0,
    }
}
