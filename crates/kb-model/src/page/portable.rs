//! Portable JSON of a page (`docs/development/milestone-03.md`, "Portable JSON").
//!
//! Output: ProseMirror JSON with these normalisations, so that the Rust core and the
//! TypeScript side produce equal values for equal documents:
//!
//! - `attrs` holds the non-null attributes only and is omitted when empty; `content`
//!   is omitted when empty; marks are `{type, attrs?}` with the same rule.
//! - Text: consecutive text runs with the same marks form one text node, across
//!   `LoroText` boundaries too (ProseMirror joins adjacent equal text nodes); empty
//!   runs disappear. Marks are listed in canonical order: the v1 marks in table order
//!   (`bold, italic, underline, strike, code, link, textColor, highlight`), then unknown
//!   marks by name.
//! - Numbers: integral values are integers (JavaScript has one number type and
//!   loro-crdt stores integral numbers as `I64`); objects are built with their keys
//!   in sorted order. Keys of a node are inserted as `type, attrs, content, marks,
//!   text` (the order is visible only where `serde_json` preserves insertion order).
//!
//! Input ([`parse_doc`]): the normalised form and raw ProseMirror JSON (null
//! attributes, `"attrs": {}`, `"marks": []`, `"content": []`, empty text nodes).
//! Marks on element nodes are accepted and dropped, as the binding drops them. Any
//! other structure is a [`ModelError::Shape`] whose message never quotes the input.

use loro::LoroValue;
use serde_json::{Map, Number, Value};

use super::read::{Child, Element, MAX_DEPTH, Mark, Segment, integral};
use super::shape;
use super::write::{NewChild, NewNode, NewRun};
use crate::ModelError;

/// Deepest nesting of attribute values converted in either direction.
const MAX_VALUE_DEPTH: usize = 64;

/// JSON node type of text runs.
const TEXT: &str = "text";

// ----- Loro -> JSON ---------------------------------------------------------------------

/// The portable JSON of a document (`{"type": "doc"}` while it has no root).
pub(super) fn document(root: Option<&Element>) -> Value {
    match root {
        Some(root) => node(root),
        None => {
            let mut obj = Map::new();
            obj.insert("type".into(), shape::DOC.into());
            Value::Object(obj)
        }
    }
}

/// The valid block ids of the nodes named `name` in portable JSON.
pub(super) fn node_ids(json: &Value, name: &str) -> std::collections::HashSet<kb_types::BlockId> {
    fn visit(json: &Value, name: &str, out: &mut std::collections::HashSet<kb_types::BlockId>) {
        if json.get("type").and_then(Value::as_str) == Some(name)
            && let Some(id) = json
                .get("attrs")
                .and_then(|attrs| attrs.get(shape::ID))
                .and_then(Value::as_str)
                .and_then(|id| id.parse().ok())
        {
            out.insert(id);
        }
        if let Some(Value::Array(content)) = json.get("content") {
            for child in content {
                visit(child, name, out);
            }
        }
    }
    let mut out = std::collections::HashSet::new();
    visit(json, name, &mut out);
    out
}

fn node(element: &Element) -> Value {
    let mut obj = Map::new();
    obj.insert("type".into(), element.name.clone().into());
    if !element.attrs.is_empty() {
        obj.insert("attrs".into(), attrs(&element.attrs));
    }
    let content = content(&element.children);
    if !content.is_empty() {
        obj.insert("content".into(), Value::Array(content));
    }
    Value::Object(obj)
}

fn content(children: &[Child]) -> Vec<Value> {
    let mut out = Vec::with_capacity(children.len());
    let mut run: Option<(String, &[Mark])> = None;
    for child in children {
        match child {
            Child::Text(segments) => {
                for Segment { text, marks } in segments {
                    let same_marks =
                        matches!(&run, Some((_, pending)) if *pending == marks.as_slice());
                    if same_marks {
                        if let Some((pending, _)) = run.as_mut() {
                            pending.push_str(text);
                        }
                    } else {
                        if let Some((pending, pending_marks)) = run.take() {
                            out.push(text_node(pending, pending_marks));
                        }
                        run = Some((text.clone(), marks.as_slice()));
                    }
                }
            }
            Child::Element(element) => {
                if let Some((pending, pending_marks)) = run.take() {
                    out.push(text_node(pending, pending_marks));
                }
                out.push(node(element));
            }
            // Invisible to every reader, so it does not separate text runs either.
            Child::Skipped => {}
        }
    }
    if let Some((pending, pending_marks)) = run {
        out.push(text_node(pending, pending_marks));
    }
    out
}

fn text_node(text: String, marks: &[Mark]) -> Value {
    let mut obj = Map::new();
    obj.insert("type".into(), TEXT.into());
    if !marks.is_empty() {
        let marks = marks
            .iter()
            .map(|mark| {
                let mut obj = Map::new();
                obj.insert("type".into(), mark.name.clone().into());
                if !mark.attrs.is_empty() {
                    obj.insert("attrs".into(), attrs(&mark.attrs));
                }
                Value::Object(obj)
            })
            .collect();
        obj.insert("marks".into(), Value::Array(marks));
    }
    obj.insert("text".into(), text.into());
    Value::Object(obj)
}

/// Attributes (already non-null and sorted) as a JSON object.
fn attrs(attrs: &[(String, LoroValue)]) -> Value {
    let mut obj = Map::new();
    for (key, value) in attrs {
        obj.insert(key.clone(), json_value(value, 0));
    }
    Value::Object(obj)
}

/// A plain Loro value as JSON (nested nulls are kept: only top-level null attributes
/// mean "absent").
pub(super) fn json_value(value: &LoroValue, depth: usize) -> Value {
    if depth > MAX_VALUE_DEPTH {
        return Value::Null;
    }
    match value {
        LoroValue::Null | LoroValue::Container(_) => Value::Null,
        LoroValue::Bool(b) => Value::Bool(*b),
        LoroValue::I64(i) => Value::from(*i),
        LoroValue::Double(f) => match integral(*f) {
            Some(i) => Value::from(i),
            None => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        },
        LoroValue::String(s) => Value::String(s.as_str().to_owned()),
        LoroValue::Binary(bytes) => Value::Array(bytes.iter().map(|b| Value::from(*b)).collect()),
        LoroValue::List(items) => Value::Array(
            items
                .iter()
                .map(|item| json_value(item, depth + 1))
                .collect(),
        ),
        LoroValue::Map(map) => {
            let mut entries: Vec<(&String, &LoroValue)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut obj = Map::new();
            for (key, value) in entries {
                obj.insert(key.clone(), json_value(value, depth + 1));
            }
            Value::Object(obj)
        }
    }
}

// ----- JSON -> Loro ---------------------------------------------------------------------

fn shape_error(message: &str) -> ModelError {
    ModelError::Shape(format!("portable JSON: {message}"))
}

enum Parsed {
    Node(NewNode),
    Text(NewRun),
}

/// Validates `json` as a page and converts it into the nodes to write.
pub(super) fn parse_doc(json: &Value) -> Result<NewNode, ModelError> {
    match parse_node(json, 0)? {
        Parsed::Node(node) if node.name == shape::DOC => Ok(node),
        _ => Err(shape_error("the top-level node must be a doc")),
    }
}

fn parse_node(json: &Value, depth: usize) -> Result<Parsed, ModelError> {
    if depth > MAX_DEPTH {
        return Err(shape_error("nodes are nested too deeply"));
    }
    let Value::Object(obj) = json else {
        return Err(shape_error("a node is not an object"));
    };
    if obj.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type" | "attrs" | "content" | "marks" | "text"
        ) && !(depth == 0 && key == shape::WHITEBOARDS)
    }) {
        return Err(shape_error("unexpected key in a node"));
    }
    let name = match obj.get("type") {
        Some(Value::String(name)) if !name.is_empty() => name.clone(),
        _ => return Err(shape_error("a node has no type")),
    };
    let attrs = parse_attrs(obj.get("attrs"))?;
    let marks = parse_marks(obj.get("marks"))?;
    let content: &[Value] = match obj.get("content") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(items)) => items,
        Some(_) => return Err(shape_error("content is not an array")),
    };

    if name == TEXT {
        let Some(Value::String(text)) = obj.get("text") else {
            return Err(shape_error("a text node has no text"));
        };
        if !attrs.is_empty() || !content.is_empty() {
            return Err(shape_error("a text node has attributes or content"));
        }
        return Ok(Parsed::Text(NewRun {
            text: text.clone(),
            marks,
        }));
    }
    if obj.contains_key("text") {
        return Err(shape_error("only text nodes have text"));
    }
    // Marks on element nodes (a bold page reference) are valid ProseMirror, but the
    // binding does not store them; neither do we.
    let mut children = Vec::with_capacity(content.len());
    let mut runs: Vec<NewRun> = Vec::new();
    for item in content {
        match parse_node(item, depth + 1)? {
            Parsed::Text(run) => {
                if !run.text.is_empty() {
                    runs.push(run);
                }
            }
            Parsed::Node(node) => {
                if !runs.is_empty() {
                    children.push(NewChild::Text(std::mem::take(&mut runs)));
                }
                children.push(NewChild::Node(node));
            }
        }
    }
    if !runs.is_empty() {
        children.push(NewChild::Text(runs));
    }
    Ok(Parsed::Node(NewNode {
        name,
        attrs,
        content: children,
    }))
}

/// `attrs`: absent, null or an object; null entries are dropped (not stored).
fn parse_attrs(json: Option<&Value>) -> Result<Vec<(String, LoroValue)>, ModelError> {
    match json {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(obj)) => {
            let mut attrs = Vec::with_capacity(obj.len());
            for (key, value) in obj {
                if !value.is_null() {
                    attrs.push((key.clone(), loro_value(value, 0)?));
                }
            }
            attrs.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(attrs)
        }
        Some(_) => Err(shape_error("attrs is not an object")),
    }
}

/// `marks`: absent, null or an array of `{type, attrs?}`. Each mark becomes a Loro
/// text mark whose value is the map of its non-null attributes (`{}` without any).
fn parse_marks(json: Option<&Value>) -> Result<Vec<(String, LoroValue)>, ModelError> {
    let items = match json {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(shape_error("marks is not an array")),
    };
    let mut marks = Vec::with_capacity(items.len());
    for item in items {
        let Value::Object(obj) = item else {
            return Err(shape_error("a mark is not an object"));
        };
        if obj
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "attrs"))
        {
            return Err(shape_error("unexpected key in a mark"));
        }
        let name = match obj.get("type") {
            Some(Value::String(name)) if !name.is_empty() => name.clone(),
            _ => return Err(shape_error("a mark has no type")),
        };
        let attrs = parse_attrs(obj.get("attrs"))?;
        marks.push((name, LoroValue::Map(attrs.into())));
    }
    Ok(marks)
}

/// A JSON value as a Loro value, with loro-crdt's number rule (integral numbers are
/// `I64`).
pub(super) fn loro_value(json: &Value, depth: usize) -> Result<LoroValue, ModelError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(shape_error("an attribute value is nested too deeply"));
    }
    Ok(match json {
        Value::Null => LoroValue::Null,
        Value::Bool(b) => LoroValue::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => LoroValue::I64(i),
            None => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                integral(f).map_or(LoroValue::Double(f), LoroValue::I64)
            }
        },
        Value::String(s) => LoroValue::from(s.as_str()),
        Value::Array(items) => {
            let mut list = Vec::with_capacity(items.len());
            for item in items {
                list.push(loro_value(item, depth + 1)?);
            }
            LoroValue::List(list.into())
        }
        Value::Object(obj) => {
            let mut map = Vec::with_capacity(obj.len());
            for (key, value) in obj {
                map.push((key.clone(), loro_value(value, depth + 1)?));
            }
            LoroValue::Map(map.into())
        }
    })
}
