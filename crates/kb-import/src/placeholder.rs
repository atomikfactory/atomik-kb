//! The placeholder format inside [`crate::PlannedPage::content`], private to this crate.
//!
//! `plan` writes portable JSON whose nodes may carry attributes named `$...`; `resolve` replaces
//! them with ids (or with text when the target is missing). The format:
//!
//! | Node | Placeholder attributes |
//! |---|---|
//! | `pageRef`, `pageLink` | `$page` (a [`Target`]), `$label` (the text to show when the target is missing) |
//! | `image`, `file` | `$file` (a path of [`crate::Plan::needed_files`], or the key of a [`crate::RemoteFile`]) or `$attachment` (an `attachment://` id read from the source); `$label` for `file`; `$url` for an `image` on a remote file (the link it becomes when no attachment was made) |
//! | `gallery` | each of `items` holds `$file` or `$attachment` instead of `attachmentId` |
//! | `whiteboard` | `$elements`, an object of Excalidraw elements by element id |
//! | `pageList` | `$names: true`: `properties`, `sort`, `source`, `filters`, `groupBy`, `hiddenGroups`, `columnWidths` and `calculations` hold names instead of ids; so do the same keys of every entry of `views` (ADR-0054), whose `filter` tree and `dateBy` and `endBy` hold names too, and so do the `widgets` of a dashboard view (ADR-0055: their `filter`, `calc.by`, `chart.groupBy` and `chart.measure.propertyId`) |

use serde_json::{Map, Value};

use crate::json::{attrs, element};

/// Attribute of a page reference: the target.
pub(crate) const PAGE: &str = "$page";
/// Attribute: the text shown when a target is missing.
pub(crate) const LABEL: &str = "$label";
/// Attribute: a needed file's path.
pub(crate) const FILE: &str = "$file";
/// Attribute: an attachment id read from the source.
pub(crate) const ATTACHMENT: &str = "$attachment";
/// Attribute of an image on a remote file: the URL, which the image becomes a link to when the
/// core gets no attachment for it.
pub(crate) const URL: &str = "$url";
/// Attribute of a whiteboard: its elements.
pub(crate) const ELEMENTS: &str = "$elements";
/// Attribute of a page list: its schema references are names.
pub(crate) const NAMES: &str = "$names";

/// What a page reference points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A planned page, by its index in the arena (rewritten to the plan index at the end of
    /// planning).
    Planned(usize),
    /// A wiki link: a page found by name among the planned ones once every title is known, else
    /// an existing page of the space by title.
    Wiki {
        /// The link target as written (`Title`, `Folder/Title`).
        name: String,
        /// The page that holds the link.
        from: usize,
    },
    /// An existing page of the space, by exact title (resolved by the core).
    Title(String),
    /// An existing page of the space, by id (an `atomik-kb://page/...` link).
    Existing {
        /// The page id as written.
        page: String,
        /// The block id as written.
        block: Option<String>,
    },
}

impl Target {
    /// The JSON form stored under [`PAGE`].
    pub(crate) fn to_json(&self) -> Value {
        let mut object = Map::new();
        match self {
            Self::Planned(index) => {
                object.insert("planned".into(), Value::from(*index));
            }
            Self::Wiki { name, from } => {
                object.insert("wiki".into(), Value::String(name.clone()));
                object.insert("from".into(), Value::from(*from));
            }
            Self::Title(title) => {
                object.insert("title".into(), Value::String(title.clone()));
            }
            Self::Existing { page, block } => {
                object.insert("existing".into(), Value::String(page.clone()));
                if let Some(block) = block {
                    object.insert("block".into(), Value::String(block.clone()));
                }
            }
        }
        Value::Object(object)
    }

    /// Reads the JSON form back.
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if let Some(index) = object.get("planned").and_then(Value::as_u64) {
            return Some(Self::Planned(usize::try_from(index).ok()?));
        }
        if let Some(name) = object.get("wiki").and_then(Value::as_str) {
            let from = usize::try_from(object.get("from").and_then(Value::as_u64)?).ok()?;
            return Some(Self::Wiki {
                name: name.to_owned(),
                from,
            });
        }
        if let Some(title) = object.get("title").and_then(Value::as_str) {
            return Some(Self::Title(title.to_owned()));
        }
        let page = object.get("existing").and_then(Value::as_str)?;
        Some(Self::Existing {
            page: page.to_owned(),
            block: object
                .get("block")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

/// An inline page reference to be resolved.
pub(crate) fn page_ref(target: &Target, label: &str) -> Value {
    let mut map = attrs([(PAGE, target.to_json())]);
    if !label.is_empty() {
        map.insert(LABEL.into(), Value::String(label.to_owned()));
    }
    element("pageRef", map, Vec::new())
}

/// A page link card (a block) to be resolved.
pub(crate) fn page_card(target: &Target, label: &str) -> Value {
    let mut map = attrs([(PAGE, target.to_json())]);
    if !label.is_empty() {
        map.insert(LABEL.into(), Value::String(label.to_owned()));
    }
    element("pageLink", map, Vec::new())
}

/// An image block on a needed file.
pub(crate) fn image_file(path: &str, alt: &str, caption: &str) -> Value {
    image(FILE, path, alt, caption)
}

/// An image block on a remote file: `key` is the file's key in [`crate::Plan::remote_files`],
/// `url` its URL.
pub(crate) fn image_remote(key: &str, url: &str) -> Value {
    let mut node = image(FILE, key, "", "");
    crate::json::set_attr(&mut node, URL, Value::String(url.to_owned()));
    node
}

/// An image block on an attachment id read from the source.
pub(crate) fn image_attachment(id: &str, alt: &str, caption: &str) -> Value {
    image(ATTACHMENT, id, alt, caption)
}

fn image(key: &str, value: &str, alt: &str, caption: &str) -> Value {
    let mut map = attrs([(key, Value::String(value.to_owned())), ("alt", alt.into())]);
    if !caption.is_empty() {
        map.insert("caption".into(), caption.into());
    }
    element("image", map, Vec::new())
}

/// A file block on a needed file; `label` is the text shown if the file is missing.
pub(crate) fn file_path(path: &str, label: &str) -> Value {
    file(FILE, path, label)
}

/// A file block on an attachment id read from the source.
pub(crate) fn file_attachment(id: &str, label: &str) -> Value {
    file(ATTACHMENT, id, label)
}

fn file(key: &str, value: &str, label: &str) -> Value {
    let mut map = attrs([(key, Value::String(value.to_owned()))]);
    if !label.is_empty() {
        map.insert(LABEL.into(), label.into());
    }
    element("file", map, Vec::new())
}

/// A whiteboard block with its elements.
pub(crate) fn whiteboard(elements: Map<String, Value>, height: Option<i64>) -> Value {
    let mut map = attrs([(ELEMENTS, Value::Object(elements))]);
    if let Some(height) = height {
        map.insert("height".into(), Value::from(height));
    }
    element("whiteboard", map, Vec::new())
}

/// Deepest nesting the walkers below follow (planned content is already bounded by `plan`).
const MAX_WALK_DEPTH: usize = 256;

/// The `$file` placeholders (needed paths or remote keys) of planned content, in document order.
pub(crate) fn file_refs<'a>(node: &'a Value, out: &mut Vec<&'a str>, depth: usize) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    match node {
        Value::Object(map) => {
            for (key, value) in map {
                if key == FILE {
                    if let Some(path) = value.as_str() {
                        out.push(path);
                    }
                } else {
                    file_refs(value, out, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                file_refs(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// The property names the saved views of planned content use (`pageList` blocks whose schema
/// references are names, [`NAMES`]): the columns, the group, the sort (and the `sorts` of a view), the filters, the column
/// widths and the calculations of the block, and the same of every view of its `views` (ADR-0054),
/// with the filter tree, the calendar's date properties and the properties the widgets of a dashboard
/// read (ADR-0055).
pub(crate) fn view_property_names(
    node: &Value,
    out: &mut std::collections::BTreeSet<String>,
    depth: usize,
) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    match node {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("pageList")
                && let Some(attrs) = map.get("attrs").and_then(Value::as_object)
                && attrs.get(NAMES) == Some(&Value::Bool(true))
            {
                attrs_property_names(attrs, out);
                if let Some(Value::Array(views)) = attrs.get("views") {
                    for view in views.iter().filter_map(Value::as_object) {
                        attrs_property_names(view, out);
                    }
                }
            }
            for value in map.values() {
                view_property_names(value, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for item in items {
                view_property_names(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// The property names one set of attributes (a block's, or a view's) holds.
fn attrs_property_names(attrs: &Map<String, Value>, out: &mut std::collections::BTreeSet<String>) {
    if let Some(Value::Array(list)) = attrs.get("properties") {
        out.extend(list.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    if let Some(name) = attrs.get("groupBy").and_then(Value::as_str) {
        out.insert(name.to_owned());
    }
    if let Some(name) = attrs
        .get("sort")
        .and_then(|sort| sort.get("by"))
        .and_then(Value::as_str)
        .filter(|name| !matches!(*name, "title" | "updated" | "created"))
    {
        out.insert(name.to_owned());
    }
    // The other keys of a view's `sorts` (ADR-0058).
    if let Some(Value::Array(keys)) = attrs.get("sorts") {
        out.extend(
            keys.iter()
                .filter_map(|key| key.get("by").and_then(Value::as_str))
                .filter(|name| {
                    !name.is_empty() && !matches!(*name, "title" | "updated" | "created")
                })
                .map(str::to_owned),
        );
    }
    if let Some(Value::Array(filters)) = attrs.get("filters") {
        out.extend(
            filters
                .iter()
                .filter_map(|filter| filter.get("property").and_then(Value::as_str))
                .map(str::to_owned),
        );
    }
    if let Some(tree) = attrs.get("filter") {
        filter_tree_names(tree, out, 0);
    }
    if let Some(Value::Object(widths)) = attrs.get("columnWidths") {
        out.extend(widths.keys().filter(|key| *key != "title").cloned());
    }
    // (`title` is the page column, not a property.)
    if let Some(Value::Object(calculations)) = attrs.get("calculations") {
        out.extend(calculations.keys().filter(|key| *key != "title").cloned());
    }
    // The calendar's date properties (`created` and `updated` are the page's own times).
    for key in ["dateBy", "endBy"] {
        if let Some(name) = attrs
            .get(key)
            .and_then(Value::as_str)
            .filter(|name| !matches!(*name, "created" | "updated"))
        {
            out.insert(name.to_owned());
        }
    }
    // The widgets of a dashboard (ADR-0055): their filters, the column of a number, the grouping and the
    // measure of a chart.
    if let Some(Value::Array(widgets)) = attrs.get("widgets") {
        for widget in widgets {
            if let Some(tree) = widget.get("filter") {
                filter_tree_names(tree, out, 0);
            }
            let reads = [
                widget.get("calc").and_then(|calc| calc.get("by")),
                widget.get("chart").and_then(|chart| chart.get("groupBy")),
                widget
                    .get("chart")
                    .and_then(|chart| chart.get("measure"))
                    .and_then(|measure| measure.get("propertyId")),
            ];
            out.extend(
                reads
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|name| !matches!(*name, "title" | "created" | "updated"))
                    .map(str::to_owned),
            );
        }
    }
}

/// The property names of the filters of a filter tree (a group holds filters and, in the top-level one,
/// groups).
fn filter_tree_names(group: &Value, out: &mut std::collections::BTreeSet<String>, depth: usize) {
    let Some(Value::Array(items)) = group.get("items") else {
        return;
    };
    if depth > MAX_WALK_DEPTH {
        return;
    }
    for item in items {
        if item.get("items").is_some() {
            filter_tree_names(item, out, depth + 1);
        } else if let Some(name) = item.get("property").and_then(Value::as_str) {
            out.insert(name.to_owned());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn targets_round_trip() {
        for target in [
            Target::Planned(7),
            Target::Wiki {
                name: "A/B".into(),
                from: 3,
            },
            Target::Title("Notes".into()),
            Target::Existing {
                page: "x".into(),
                block: Some("y".into()),
            },
            Target::Existing {
                page: "x".into(),
                block: None,
            },
        ] {
            assert_eq!(Target::from_json(&target.to_json()), Some(target));
        }
        assert_eq!(Target::from_json(&Value::Null), None);
    }

    #[test]
    fn views_name_the_properties_of_their_calculations_too() {
        let blocks = serde_json::json!([
            {"type": "pageList", "attrs": {
                "$names": true,
                "properties": ["Status"],
                "columnWidths": {"Due": 120},
                "calculations": {"title": "count_all", "Cost": "sum"}
            }},
            // Without `$names` the keys are ids, not names to plan properties for.
            {"type": "pageList", "attrs": {"calculations": {"Other": "sum"}}}
        ]);
        let mut names = std::collections::BTreeSet::new();
        view_property_names(&blocks, &mut names, 0);
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            ["Cost", "Due", "Status"]
        );
    }

    #[test]
    fn the_widgets_of_a_dashboard_name_their_properties_too() {
        let blocks = serde_json::json!([
            {"type": "pageList", "attrs": {
                "$names": true,
                "views": [
                    {"id": "d", "layout": "dashboard", "widgets": [
                        {"id": "w1", "kind": "number", "calc": {"by": "Hours", "fn": "sum"}},
                        {"id": "w2", "kind": "number", "calc": {"by": "title", "fn": "count_all"}},
                        {"id": "w3", "kind": "chart",
                         "chart": {"type": "bar", "groupBy": "Stage", "measure": {"fn": "sum", "propertyId": "Cost"}},
                         "filter": {"op": "and", "items": [{"property": "Priority", "op": "isSet"}]}},
                        {"id": "w4", "kind": "chart", "chart": {"type": "line", "groupBy": "created", "measure": {"fn": "count"}}},
                        {"id": "w5", "kind": "chart", "chart": {"type": "donut", "groupBy": null}},
                        "not a widget"
                    ]}
                ]
            }}
        ]);
        let mut names = std::collections::BTreeSet::new();
        view_property_names(&blocks, &mut names, 0);
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            ["Cost", "Hours", "Priority", "Stage"]
        );
    }

    #[test]
    fn the_sort_keys_of_a_view_name_their_properties_too() {
        let blocks = serde_json::json!([
            {"type": "pageList", "attrs": {
                "$names": true,
                "views": [
                    {"id": "a", "layout": "table", "sort": {"by": "Due", "dir": "asc"},
                     "sorts": [
                         {"by": "Due", "dir": "asc"}, {"by": "title", "dir": "desc"},
                         {"by": "Rank", "dir": "asc"}, {"by": "created", "dir": "asc"},
                         {"by": "", "dir": "asc"}, {"dir": "asc"}, "junk", 3
                     ]},
                    {"id": "b", "sorts": "Rank"}
                ]
            }},
            // Without `$names` the keys are ids.
            {"type": "pageList", "attrs": {"views": [{"id": "x", "sorts": [{"by": "Hidden", "dir": "asc"}]}]}}
        ]);
        let mut names = std::collections::BTreeSet::new();
        view_property_names(&blocks, &mut names, 0);
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["Due", "Rank"]);
    }

    #[test]
    fn every_view_of_a_block_names_its_properties_too() {
        let blocks = serde_json::json!([
            {"type": "pageList", "attrs": {
                "$names": true,
                "properties": ["Status"],
                "views": [
                    {"id": "a", "name": "", "layout": "calendar", "properties": ["Owner"], "dateBy": "Due", "endBy": "End"},
                    // The page's own times are no property.
                    {"id": "b", "layout": "calendar", "dateBy": "created", "endBy": "updated", "sort": {"by": "updated", "dir": "asc"}},
                    {"id": "c", "layout": "table", "groupBy": "Stage", "sort": {"by": "Rank", "dir": "desc"},
                     "filter": {"op": "and", "items": [
                         {"property": "Priority", "op": "isSet"},
                         {"op": "or", "items": [{"property": "Cost", "op": "isSet"}]}]},
                     "columnWidths": {"Size": 100, "title": 300}, "calculations": {"Total": "sum", "title": "count_all"}},
                    "not a view",
                    4
                ]
            }},
            // Without `$names` the keys are ids, in the views too.
            {"type": "pageList", "attrs": {"views": [{"id": "x", "dateBy": "Hidden", "properties": ["Other"]}]}}
        ]);
        let mut names = std::collections::BTreeSet::new();
        view_property_names(&blocks, &mut names, 0);
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            [
                "Cost", "Due", "End", "Owner", "Priority", "Rank", "Size", "Stage", "Status",
                "Total"
            ]
        );
    }
}
