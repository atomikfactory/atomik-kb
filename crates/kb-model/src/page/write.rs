//! Writing nodes in the binding's shape.
//!
//! Element nodes mirror loro-prosemirror's `createLoroMap` / `createLoroText`
//! operation by operation: an element node is a `LoroMap` inserted into its parent's
//! children list, then `nodeName` is set, then the `attributes` map is created as a
//! regular child container and filled with the non-null attributes, then the
//! `children` list is created the same way and filled. Consecutive text runs form one
//! `LoroText`, filled with one `apply_delta` whose inserts carry the marks (mark name
//! -> mark attributes map).
//!
//! The root differs in one point (shape v1, see [`super::shape`]): its `attributes`
//! and `children` are **mergeable** child containers (`ensure_mergeable_map`,
//! `ensure_mergeable_list`), whose ids derive from the root and the key, so two
//! devices that create the same page document concurrently write into the same list.
//! A root that already has these fields keeps them, whichever kind they are.

use loro::{Container, LoroList, LoroMap, LoroText, LoroValue, TextDelta, ValueOrContainer};

use super::shape;
use crate::ModelError;
use crate::meta::engine;

/// An element node to write.
pub(super) struct NewNode {
    /// `nodeName`.
    pub name: String,
    /// Non-null attributes.
    pub attrs: Vec<(String, LoroValue)>,
    /// Children in order.
    pub content: Vec<NewChild>,
}

/// A child to write.
pub(super) enum NewChild {
    /// An element node.
    Node(NewNode),
    /// One `LoroText` holding these runs.
    Text(Vec<NewRun>),
}

/// A run of text with its marks.
pub(super) struct NewRun {
    /// The text.
    pub text: String,
    /// Mark name -> mark value (the mark's attributes as a map).
    pub marks: Vec<(String, LoroValue)>,
}

/// A text run without marks (no child at all for empty text, as the binding does for
/// empty paragraphs).
pub(super) fn plain_text(text: &str) -> Vec<NewChild> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![NewChild::Text(vec![NewRun {
            text: text.to_owned(),
            marks: Vec::new(),
        }])]
    }
}

/// Initialises an empty root: `nodeName` first, then the `attributes` and `children`
/// fields (mergeable), like the binding's first sync, then the content.
pub(super) fn init_root(root: &LoroMap, node: &NewNode) -> Result<(), ModelError> {
    root.insert(shape::NODE_NAME, shape::DOC).map_err(engine)?;
    set_attrs(&root_attributes(root)?, &node.attrs)?;
    append_children(&root_list(root)?, &node.content)
}

/// The root's children list, creating the root first when the engine document has
/// none yet (semantic operations on an empty document).
pub(super) fn root_children(root: &LoroMap) -> Result<LoroList, ModelError> {
    if root.get(shape::NODE_NAME).is_none() {
        root.insert(shape::NODE_NAME, shape::DOC).map_err(engine)?;
    }
    root_attributes(root)?;
    root_list(root)
}

/// The root's attributes map: the existing one (mergeable, or regular in documents
/// written before the root became mergeable), or a new mergeable map.
fn root_attributes(root: &LoroMap) -> Result<LoroMap, ModelError> {
    match root.get(shape::ATTRIBUTES) {
        Some(ValueOrContainer::Container(Container::Map(map))) => Ok(map),
        None | Some(ValueOrContainer::Value(LoroValue::Null)) => {
            root.ensure_mergeable_map(shape::ATTRIBUTES).map_err(engine)
        }
        Some(_) => Err(ModelError::Shape(
            "root field attributes is not a map".into(),
        )),
    }
}

/// The root's children list: the existing one (mergeable or regular), or a new
/// mergeable list.
fn root_list(root: &LoroMap) -> Result<LoroList, ModelError> {
    match root.get(shape::CHILDREN) {
        Some(ValueOrContainer::Container(Container::List(list))) => Ok(list),
        None | Some(ValueOrContainer::Value(LoroValue::Null)) => {
            root.ensure_mergeable_list(shape::CHILDREN).map_err(engine)
        }
        Some(_) => Err(ModelError::Shape(
            "root field children is not a list".into(),
        )),
    }
}

/// Inserts `node` at `pos` of `list`.
pub(super) fn insert_node(
    list: &LoroList,
    pos: usize,
    node: &NewNode,
) -> Result<LoroMap, ModelError> {
    let map = list.insert_container(pos, LoroMap::new()).map_err(engine)?;
    map.insert(shape::NODE_NAME, node.name.as_str())
        .map_err(engine)?;
    fill(&map, node)?;
    Ok(map)
}

/// Inserts a `LoroText` holding `runs` at `pos` of `list`.
pub(super) fn insert_text(
    list: &LoroList,
    pos: usize,
    runs: &[NewRun],
) -> Result<LoroText, ModelError> {
    let text = list
        .insert_container(pos, LoroText::new())
        .map_err(engine)?;
    let delta: Vec<TextDelta> = runs
        .iter()
        .map(|run| TextDelta::Insert {
            insert: run.text.clone(),
            attributes: Some(run.marks.iter().cloned().collect()),
        })
        .collect();
    text.apply_delta(&delta).map_err(engine)?;
    Ok(text)
}

/// Writes the attributes and children of an element node whose `nodeName` is set
/// (regular child containers).
fn fill(map: &LoroMap, node: &NewNode) -> Result<(), ModelError> {
    set_attrs(&child_map(map, shape::ATTRIBUTES)?, &node.attrs)?;
    append_children(&child_list(map, shape::CHILDREN)?, &node.content)
}

/// Stores the non-null attributes.
fn set_attrs(attributes: &LoroMap, attrs: &[(String, LoroValue)]) -> Result<(), ModelError> {
    for (key, value) in attrs {
        if !matches!(value, LoroValue::Null) {
            attributes.insert(key, value.clone()).map_err(engine)?;
        }
    }
    Ok(())
}

/// Appends `content` to a children list.
fn append_children(children: &LoroList, content: &[NewChild]) -> Result<(), ModelError> {
    for child in content {
        let pos = children.len();
        match child {
            NewChild::Node(child) => {
                insert_node(children, pos, child)?;
            }
            NewChild::Text(runs) => {
                insert_text(children, pos, runs)?;
            }
        }
    }
    Ok(())
}

/// The map under `key`, created when absent (the binding's `getOrCreateContainer`).
pub(super) fn child_map(map: &LoroMap, key: &str) -> Result<LoroMap, ModelError> {
    match map.get(key) {
        Some(ValueOrContainer::Container(Container::Map(child))) => Ok(child),
        None | Some(ValueOrContainer::Value(LoroValue::Null)) => {
            map.insert_container(key, LoroMap::new()).map_err(engine)
        }
        Some(_) => Err(ModelError::Shape(format!("node field {key} is not a map"))),
    }
}

/// The list under `key`, created when absent (the binding's `getOrCreateContainer`).
pub(super) fn child_list(map: &LoroMap, key: &str) -> Result<LoroList, ModelError> {
    match map.get(key) {
        Some(ValueOrContainer::Container(Container::List(child))) => Ok(child),
        None | Some(ValueOrContainer::Value(LoroValue::Null)) => {
            map.insert_container(key, LoroList::new()).map_err(engine)
        }
        Some(_) => Err(ModelError::Shape(format!("node field {key} is not a list"))),
    }
}
