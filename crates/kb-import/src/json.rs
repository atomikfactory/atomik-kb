//! Builders for the portable JSON nodes the converters produce, and the marks of text runs.
//!
//! The nodes carry no `id` attributes (`resolve` assigns them) and may carry placeholder
//! attributes whose names start with `$` (see [`crate::placeholder`]).

use serde_json::{Map, Value};

/// A node: `{"type": kind, "attrs": {...}?, "content": [...]?}`.
pub(crate) fn element(kind: &str, attrs: Map<String, Value>, content: Vec<Value>) -> Value {
    let mut node = Map::new();
    node.insert("type".into(), Value::String(kind.to_owned()));
    if !attrs.is_empty() {
        node.insert("attrs".into(), Value::Object(attrs));
    }
    if !content.is_empty() {
        node.insert("content".into(), Value::Array(content));
    }
    Value::Object(node)
}

/// Attributes from pairs.
pub(crate) fn attrs<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// A node without attributes.
pub(crate) fn plain(kind: &str, content: Vec<Value>) -> Value {
    element(kind, Map::new(), content)
}

/// A paragraph of inline nodes.
pub(crate) fn paragraph(content: Vec<Value>) -> Value {
    plain("paragraph", content)
}

/// A paragraph of one unmarked text (empty text: an empty paragraph).
pub(crate) fn text_paragraph(text: &str) -> Value {
    if text.is_empty() {
        paragraph(Vec::new())
    } else {
        paragraph(vec![text_node(text, &MarkSet::default())])
    }
}

/// A text node with its marks (the marks are listed in the canonical order of the schema).
pub(crate) fn text_node(text: &str, marks: &MarkSet) -> Value {
    let mut node = Map::new();
    node.insert("type".into(), Value::String("text".into()));
    let marks = marks.to_json();
    if !marks.is_empty() {
        node.insert("marks".into(), Value::Array(marks));
    }
    node.insert("text".into(), Value::String(text.to_owned()));
    Value::Object(node)
}

/// A hard break.
pub(crate) fn hard_break() -> Value {
    plain("hardBreak", Vec::new())
}

/// A heading of level 1 to 3.
pub(crate) fn heading(level: u8, content: Vec<Value>) -> Value {
    element(
        "heading",
        attrs([("level", Value::from(level.clamp(1, 3)))]),
        content,
    )
}

/// The summary line of a toggle: its text unmarked (no text node when empty) and, for a toggle
/// heading (ADR-0040), its `level`, 1 to 3.
pub(crate) fn summary(text: &str, level: Option<u8>) -> Value {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![text_node(text, &MarkSet::default())]
    };
    let mut map = Map::new();
    if let Some(level) = level {
        map.insert("level".into(), Value::from(level.clamp(1, 3)));
    }
    element("detailsSummary", map, content)
}

/// The type name of a node.
pub(crate) fn kind(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

/// An attribute of a node.
pub(crate) fn attr<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    node.get("attrs")?.get(key)
}

/// A string attribute of a node.
pub(crate) fn str_attr<'a>(node: &'a Value, key: &str) -> Option<&'a str> {
    attr(node, key).and_then(Value::as_str)
}

/// Sets an attribute of a node (creating `attrs` when needed).
pub(crate) fn set_attr(node: &mut Value, key: &str, value: Value) {
    if let Value::Object(object) = node {
        let slot = object
            .entry("attrs")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(attrs) = slot {
            attrs.insert(key.to_owned(), value);
        }
    }
}

/// The children of a node.
pub(crate) fn content(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The children of a node, mutably.
pub(crate) fn content_mut(node: &mut Value) -> Option<&mut Vec<Value>> {
    node.get_mut("content").and_then(Value::as_array_mut)
}

/// The text of a node and everything inside it, without marks; a hard break is a space.
pub(crate) fn plain_text(node: &Value) -> String {
    let mut out = String::new();
    collect_text(node, &mut out, 0);
    out
}

fn collect_text(node: &Value, out: &mut String, depth: usize) {
    if depth > 128 {
        return;
    }
    match kind(node) {
        "text" => {
            if let Some(text) = node.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }
        "hardBreak" => out.push(' '),
        "mathInline" => {
            if let Some(latex) = str_attr(node, "latex") {
                out.push_str(latex);
            }
        }
        _ => {
            for child in content(node) {
                collect_text(child, out, depth + 1);
            }
        }
    }
}

/// Whether a text node is only white space.
pub(crate) fn is_blank_text(node: &Value) -> bool {
    kind(node) == "text"
        && node
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.trim().is_empty())
}

/// The marks active on a run of text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MarkSet {
    /// `bold`.
    pub bold: bool,
    /// `italic`.
    pub italic: bool,
    /// `underline`.
    pub underline: bool,
    /// `strike`.
    pub strike: bool,
    /// `code` (excludes every other mark, as in the schema).
    pub code: bool,
    /// `link` with this `href`.
    pub link: Option<String>,
    /// `textColor` with this colour name.
    pub text_color: Option<String>,
    /// `highlight` with this colour name.
    pub highlight: Option<String>,
}

impl MarkSet {
    /// The marks as JSON, in the canonical order: `bold, italic, underline, strike, code, link,
    /// textColor, highlight`. Code takes no other mark.
    pub(crate) fn to_json(&self) -> Vec<Value> {
        let simple = |name: &str| {
            let mut mark = Map::new();
            mark.insert("type".into(), Value::String(name.to_owned()));
            Value::Object(mark)
        };
        if self.code {
            return vec![simple("code")];
        }
        let mut marks = Vec::new();
        if self.bold {
            marks.push(simple("bold"));
        }
        if self.italic {
            marks.push(simple("italic"));
        }
        if self.underline {
            marks.push(simple("underline"));
        }
        if self.strike {
            marks.push(simple("strike"));
        }
        if let Some(href) = &self.link {
            let mut mark = Map::new();
            mark.insert("type".into(), Value::String("link".into()));
            mark.insert(
                "attrs".into(),
                Value::Object(attrs([("href", href.clone().into())])),
            );
            marks.push(Value::Object(mark));
        }
        if let Some(color) = &self.text_color {
            let mut mark = Map::new();
            mark.insert("type".into(), Value::String("textColor".into()));
            mark.insert(
                "attrs".into(),
                Value::Object(attrs([("color", color.clone().into())])),
            );
            marks.push(Value::Object(mark));
        }
        if let Some(color) = &self.highlight {
            let mut mark = Map::new();
            mark.insert("type".into(), Value::String("highlight".into()));
            mark.insert(
                "attrs".into(),
                Value::Object(attrs([("color", color.clone().into())])),
            );
            marks.push(Value::Object(mark));
        }
        marks
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn marks_are_canonical_and_code_excludes_the_rest() {
        let marks = MarkSet {
            italic: true,
            bold: true,
            link: Some("https://example.com".into()),
            ..MarkSet::default()
        };
        assert_eq!(
            Value::Array(marks.to_json()),
            json!([
                {"type": "bold"},
                {"type": "italic"},
                {"type": "link", "attrs": {"href": "https://example.com"}}
            ])
        );
        let code = MarkSet {
            code: true,
            bold: true,
            ..MarkSet::default()
        };
        assert_eq!(Value::Array(code.to_json()), json!([{"type": "code"}]));
    }

    #[test]
    fn plain_text_reads_through_marks_and_breaks() {
        let node = paragraph(vec![
            text_node("a", &MarkSet::default()),
            hard_break(),
            text_node(
                "b",
                &MarkSet {
                    bold: true,
                    ..MarkSet::default()
                },
            ),
        ]);
        assert_eq!(plain_text(&node), "a b");
    }
}
