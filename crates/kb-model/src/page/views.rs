//! The views of a `pageList` block (ADR-0054, amending ADR-0016; `docs/architecture/database-views.md`).
//!
//! A `pageList` block holds a list of views in its attribute `views`, a plain JSON list set whole like
//! every attribute of the block (so [`PageDoc::page_lists`](super::PageDoc::page_lists) and
//! [`PageDoc::set_page_list_attrs`](super::PageDoc::set_page_list_attrs) already carry it: they read and
//! write attributes as plain JSON and know no names). This module is the **typed form of what is in
//! that list**, for the Rust code that has to read inside a view: the move to another space remaps the
//! property, option and page ids a view mentions, the Markdown annotation swaps ids for names and back,
//! the HTML export may read the layout. The TypeScript twin is `packages/ui/src/editor/page-list-views.ts`
//! (the filter tree is `page-list-filter-tree.ts`); the two must agree and the JSON examples of the
//! design document are the contract.
//!
//! **A view is "the legacy attributes of the block, one per view"** plus new keys:
//!
//! | Key | Type | Notes |
//! |---|---|---|
//! | `id` | text, 1 to 64 characters | unique in the block; `"default"` for the view a block without `views` reads as |
//! | `name` | text, at most 80 characters | empty means the layout's own name |
//! | `layout` | `list`, `table`, `board`, `calendar`, `gallery` | another value reads as `list` and is kept |
//! | `properties` | list of property ids | the shown properties (the columns of a table) |
//! | `sort` | `{ by, dir }` | `by` is `title`, `updated`, `created` or a property id; the first of `sorts` when there are several (ADR-0058) |
//! | `sorts` | list of `{ by, dir }`, 1 to 5 | the sort keys in order, written only when there are several (ADR-0058); a reader uses it when it holds a usable key, else `[sort]` |
//! | `filter` | [`FilterGroup`] | the filter tree; absent means an empty AND group; replaces the flat `filters` |
//! | `columnWidths` | `{ <property id or "title">: px }` | |
//! | `groupBy` | a property id | the board's |
//! | `hiddenGroups` | option ids or `"none"` | the board's |
//! | `calculations` | `{ <property id or "title">: function }` | the table footer's |
//! | `openIn` | `side`, `center`, `page` | absent means `side` |
//! | `dateBy` | a property id, `created` or `updated` | the calendar's date source |
//! | `endBy` | a property id | the calendar's optional end date |
//! | `cardSize` | `s`, `m`, `l` | the gallery's; absent means `m` |
//!
//! The block's `source` is shared by all its views and stays a top-level attribute. The **first view is
//! mirrored** into the block's legacy attributes by every writer (TypeScript `writeViews`), so an older
//! build shows it; a block without `views` reads as one view made from the legacy attributes.
//!
//! Everything a version does not understand is kept: [`PageListView::extra`] holds the keys this module
//! does not name, so a remap of ids rewrites a view without dropping what a newer version added.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

/// The attribute of a `pageList` block that holds its views.
pub const VIEWS_ATTR: &str = "views";
/// Most views a block holds (a reader ignores the rest).
pub const MAX_VIEWS: usize = 20;
/// Longest view name, in characters.
pub const MAX_VIEW_NAME_CHARS: usize = 80;
/// Longest view id, in characters.
pub const MAX_VIEW_ID_CHARS: usize = 64;
/// Most sort keys a view holds in `sorts` (a reader ignores the rest).
pub const MAX_SORTS: usize = 5;
/// Most filters a view holds, nested groups included.
pub const MAX_FILTERS: usize = 50;
/// Most groups nested in a view's top-level filter group.
pub const MAX_FILTER_GROUPS: usize = 10;
/// The id of the view a block without `views` reads as.
pub const DEFAULT_VIEW_ID: &str = "default";
/// The layouts this version knows, in the order the pickers list them.
pub const LAYOUTS: [&str; 5] = ["list", "table", "board", "calendar", "gallery"];

/// How a filter group combines its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupOp {
    /// Every applicable item passes.
    And,
    /// Some applicable item passes.
    Or,
}

/// A group of filters, one level deep: the top-level group of a view holds filters and nested groups, a
/// nested group holds filters only. An empty group passes; an item that cannot be applied is skipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilterGroup {
    /// AND or OR.
    pub op: GroupOp,
    /// The filters and (in the top-level group) the nested groups, in order.
    pub items: Vec<FilterNode>,
}

/// One filter as stored: `{ propertyId, op, value? }` (the flat filters of ADR-0016, 2026-10-08).
/// `value` depends on the operator and the property's kind (an option id for `hasOption`, a page id for
/// `relatesTo`, a day for `before`, `{from, to}` for `between`, ...). A JSON `null` value reads as absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewFilter {
    /// The property id (kept as text: a stored id may be anything).
    pub property_id: String,
    /// The operator (`isSet`, `hasOption`, ...; a name this version does not know is kept).
    pub op: String,
    /// The argument of the operator, when it takes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// An item of a [`FilterGroup`]: a nested group (it has `items`) or a filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterNode {
    /// A nested group.
    Group(FilterGroup),
    /// A filter.
    Filter(ViewFilter),
}

impl FilterGroup {
    /// The number of filters of the group, nested groups included.
    #[must_use]
    pub fn count_filters(&self) -> usize {
        self.items
            .iter()
            .map(|item| match item {
                FilterNode::Group(group) => group.count_filters(),
                FilterNode::Filter(_) => 1,
            })
            .sum()
    }

    /// Calls `visit` on every filter of the tree (top level first, nested groups in place), so that an id
    /// rewrite (`propertyId`, and the option or page ids inside `value`) is one traversal.
    pub fn for_each_filter_mut(&mut self, visit: &mut dyn FnMut(&mut ViewFilter)) {
        for item in &mut self.items {
            match item {
                FilterNode::Group(group) => group.for_each_filter_mut(visit),
                FilterNode::Filter(filter) => visit(filter),
            }
        }
    }
}

/// `{ by, dir }`: `by` is `title`, `updated`, `created` or a property id; `dir` is `asc` or `desc`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSort {
    /// What the rows sort by.
    pub by: String,
    /// `asc` or `desc`.
    pub dir: String,
}

/// One view of a `pageList` block, as stored (see the module documentation for the keys).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageListView {
    /// Unique in the block.
    pub id: String,
    /// The tab's label; empty means the layout's own name.
    #[serde(default)]
    pub name: String,
    /// The layout, as stored (a value this version does not know is kept).
    pub layout: String,
    /// The shown properties, in order.
    #[serde(default)]
    pub properties: Vec<String>,
    /// The sort; absent reads as title ascending. With several keys it is the first of [`PageListView::sorts`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<ViewSort>,
    /// The sort keys in order (ADR-0058): at most [`MAX_SORTS`], no two with the same `by`. Empty when the
    /// view stores none (`sort` then is the only key); written back only when not empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sorts: Vec<ViewSort>,
    /// The filter tree; absent reads as an empty AND group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<FilterGroup>,
    /// Column widths by property id or `title`, as stored (an integral width stays an integer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_widths: Option<BTreeMap<String, Number>>,
    /// The board's group property id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    /// The board's hidden columns (option ids or `none`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_groups: Option<Vec<String>>,
    /// The table footer's functions by column key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calculations: Option<BTreeMap<String, String>>,
    /// Where a row opens: `side` (the default), `center` or `page`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_in: Option<String>,
    /// The calendar's date source: a property id, `created` or `updated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_by: Option<String>,
    /// The calendar's optional end-date property id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_by: Option<String>,
    /// The gallery's card size: `s`, `m` (the default) or `l`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_size: Option<String>,
    /// Every other key of the stored view (a newer version's), kept as read and written back.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The views a block stores, read leniently: the entries of the `views` attribute that parse (an entry
/// that is no object, has no usable `id` of 1 to [`MAX_VIEW_ID_CHARS`] characters or repeats an earlier id
/// is skipped; at most [`MAX_VIEWS`]; a name is cut to [`MAX_VIEW_NAME_CHARS`]; the filter tree is limited
/// to [`MAX_FILTERS`] filters and [`MAX_FILTER_GROUPS`] groups, one level deep, in reading order). `None`
/// when the attribute is absent or holds no usable entry: the block then reads as one view made from its
/// legacy attributes, which the caller handles as it handled them before ADR-0054. Never panics.
///
/// The rules are those of the TypeScript `parseViews` (`page-list-views.ts`): a part that cannot be used
/// reads as its default (an empty list, no sort, no filter), the keys this version does not know and the
/// `openIn` or `cardSize` it cannot use are kept in [`PageListView::extra`], and `__proto__` is never a key.
#[must_use]
pub fn read_views(attrs: &Map<String, Value>) -> Option<Vec<PageListView>> {
    let Some(Value::Array(entries)) = attrs.get(VIEWS_ATTR) else {
        return None;
    };
    let mut views: Vec<PageListView> = Vec::new();
    for entry in entries {
        let Some(view) = read_view(entry) else {
            continue;
        };
        if views.iter().any(|seen| seen.id == view.id) {
            continue;
        }
        views.push(view);
        if views.len() >= MAX_VIEWS {
            break;
        }
    }
    (!views.is_empty()).then_some(views)
}

/// The keys of a stored view this version understands (everything else goes to [`PageListView::extra`]).
const KNOWN_KEYS: [&str; 15] = [
    "id",
    "name",
    "layout",
    "properties",
    "sort",
    "sorts",
    "filter",
    "columnWidths",
    "groupBy",
    "hiddenGroups",
    "calculations",
    "openIn",
    "dateBy",
    "endBy",
    "cardSize",
];

/// The values of `openIn`.
const OPEN_IN_VALUES: [&str; 3] = ["side", "center", "page"];
/// The values of `cardSize`.
const CARD_SIZES: [&str; 3] = ["s", "m", "l"];

/// One stored view, read leniently (see [`read_views`]); `None` for an entry that is no object or has no
/// usable id.
#[must_use]
pub fn read_view(entry: &Value) -> Option<PageListView> {
    let object = entry.as_object()?;
    let id = non_empty_text(object.get("id"))?;
    if id.chars().count() > MAX_VIEW_ID_CHARS {
        return None;
    }
    let mut extra = Map::new();
    for (key, value) in object {
        if !KNOWN_KEYS.contains(&key.as_str()) && key != "__proto__" {
            extra.insert(key.clone(), value.clone());
        }
    }
    let open_in = keyword(object.get("openIn"), &OPEN_IN_VALUES, "openIn", &mut extra);
    let card_size = keyword(object.get("cardSize"), &CARD_SIZES, "cardSize", &mut extra);
    Some(PageListView {
        id: id.to_owned(),
        name: object
            .get("name")
            .and_then(Value::as_str)
            .map(|name| name.chars().take(MAX_VIEW_NAME_CHARS).collect())
            .unwrap_or_default(),
        layout: non_empty_text(object.get("layout"))
            .unwrap_or("list")
            .to_owned(),
        properties: read_text_list(object.get("properties")),
        sort: read_sort(object.get("sort")),
        sorts: read_sorts(object.get("sorts")),
        filter: object.get("filter").and_then(read_filter_group),
        column_widths: read_widths(object.get("columnWidths")),
        group_by: non_empty_text(object.get("groupBy")).map(str::to_owned),
        hidden_groups: Some(read_text_list(object.get("hiddenGroups")))
            .filter(|list| !list.is_empty()),
        calculations: read_calculations(object.get("calculations")),
        open_in,
        date_by: non_empty_text(object.get("dateBy")).map(str::to_owned),
        end_by: non_empty_text(object.get("endBy")).map(str::to_owned),
        card_size,
        extra,
    })
}

/// A non-empty text, `None` for anything else.
fn non_empty_text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// A value out of a fixed set of keywords. Anything else that is present (and not null) is kept in `extra`
/// under `key` as it was, so that a newer version's value survives an edit made here.
fn keyword(
    value: Option<&Value>,
    allowed: &[&str],
    key: &str,
    extra: &mut Map<String, Value>,
) -> Option<String> {
    let value = value?;
    match value.as_str() {
        Some(text) if allowed.contains(&text) => Some(text.to_owned()),
        _ => {
            if !value.is_null() {
                extra.insert(key.to_owned(), value.clone());
            }
            None
        }
    }
}

/// The non-empty texts of a list, without repeats, in order.
fn read_text_list(value: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if let Some(text) = non_empty_text(Some(item))
                && !out.iter().any(|seen| seen == text)
            {
                out.push(text.to_owned());
            }
        }
    }
    out
}

/// The direction of a stored sort: `desc`, anything else is ascending.
fn read_dir(object: &Map<String, Value>) -> String {
    if object.get("dir").and_then(Value::as_str) == Some("desc") {
        "desc"
    } else {
        "asc"
    }
    .to_owned()
}

/// `{ by, dir }` with the defaults of an unusable part (`title`, ascending); `None` for no object.
fn read_sort(value: Option<&Value>) -> Option<ViewSort> {
    let object = value?.as_object()?;
    Some(ViewSort {
        by: non_empty_text(object.get("by"))
            .unwrap_or("title")
            .to_owned(),
        dir: read_dir(object),
    })
}

/// The usable keys of `sorts` (ADR-0058), in order: an object with a non-empty `by`; the first of a repeated
/// `by` wins; at most [`MAX_SORTS`] are read. Empty when the value is no list or holds no usable key (the
/// reader then falls back to `sort`).
fn read_sorts(value: Option<&Value>) -> Vec<ViewSort> {
    let mut out: Vec<ViewSort> = Vec::new();
    let Some(Value::Array(items)) = value else {
        return out;
    };
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(by) = non_empty_text(object.get("by")) else {
            continue;
        };
        if out.iter().any(|seen| seen.by == by) {
            continue;
        }
        out.push(ViewSort {
            by: by.to_owned(),
            dir: read_dir(object),
        });
        if out.len() >= MAX_SORTS {
            break;
        }
    }
    out
}

/// The widths above zero by key; `None` when none is left.
fn read_widths(value: Option<&Value>) -> Option<BTreeMap<String, Number>> {
    let widths: BTreeMap<String, Number> = value?
        .as_object()?
        .iter()
        .filter(|(key, _)| key.as_str() != "__proto__")
        .filter_map(|(key, width)| {
            let number = width.as_number()?;
            number
                .as_f64()
                .is_some_and(|w| w.is_finite() && w > 0.0)
                .then(|| (key.clone(), number.clone()))
        })
        .collect();
    (!widths.is_empty()).then_some(widths)
}

/// The footer's functions: non-empty keys and non-empty texts; `None` when none is left.
fn read_calculations(value: Option<&Value>) -> Option<BTreeMap<String, String>> {
    let calculations: BTreeMap<String, String> = value?
        .as_object()?
        .iter()
        .filter(|(key, _)| !key.is_empty() && key.as_str() != "__proto__")
        .filter_map(|(key, function)| {
            non_empty_text(Some(function)).map(|f| (key.clone(), f.to_owned()))
        })
        .collect();
    (!calculations.is_empty()).then_some(calculations)
}

/// One stored filter: an object with a non-empty `propertyId` and a text `op` (a filter this version cannot
/// apply is kept; anything else reads as none). A JSON `null` value reads as absent.
fn read_filter(item: &Value) -> Option<ViewFilter> {
    let object = item.as_object()?;
    let property_id = non_empty_text(object.get("propertyId"))?;
    let op = object.get("op")?.as_str()?;
    Some(ViewFilter {
        property_id: property_id.to_owned(),
        op: op.to_owned(),
        value: object
            .get("value")
            .filter(|value| !value.is_null())
            .cloned(),
    })
}

/// A stored group: an object with an `items` list and the operator `and` or `or`.
fn group_parts(value: &Value) -> Option<(GroupOp, &Vec<Value>)> {
    let object = value.as_object()?;
    let op = match object.get("op").and_then(Value::as_str)? {
        "and" => GroupOp::And,
        "or" => GroupOp::Or,
        _ => return None,
    };
    Some((op, object.get("items")?.as_array()?))
}

/// What a stored filter tree becomes (the TypeScript `normaliseFilterGroup`): a group stays a group; a list
/// (the flat `filters` of a block written before ADR-0054) becomes an AND group of its filters; anything else,
/// and the empty AND group that is the default, is `None`. Entries that are no filter and no group are dropped,
/// a group inside a nested group is dropped (one level only) and the limits apply in reading order: the first
/// [`MAX_FILTERS`] filters and the first [`MAX_FILTER_GROUPS`] groups are kept.
#[must_use]
pub fn read_filter_group(value: &Value) -> Option<FilterGroup> {
    let group = if let Value::Array(list) = value {
        FilterGroup {
            op: GroupOp::And,
            items: list
                .iter()
                .filter_map(read_filter)
                .take(MAX_FILTERS)
                .map(FilterNode::Filter)
                .collect(),
        }
    } else {
        let (op, entries) = group_parts(value)?;
        let (mut filters, mut groups) = (0_usize, 0_usize);
        let mut items: Vec<FilterNode> = Vec::new();
        for entry in entries {
            if let Some((inner_op, inner_entries)) = group_parts(entry) {
                if groups >= MAX_FILTER_GROUPS {
                    continue;
                }
                groups += 1;
                let mut inner: Vec<FilterNode> = Vec::new();
                for entry in inner_entries
                    .iter()
                    .filter(|entry| group_parts(entry).is_none())
                {
                    let Some(filter) = read_filter(entry) else {
                        continue;
                    };
                    if filters >= MAX_FILTERS {
                        break;
                    }
                    filters += 1;
                    inner.push(FilterNode::Filter(filter));
                }
                items.push(FilterNode::Group(FilterGroup {
                    op: inner_op,
                    items: inner,
                }));
            } else if let Some(filter) = read_filter(entry)
                && filters < MAX_FILTERS
            {
                filters += 1;
                items.push(FilterNode::Filter(filter));
            }
        }
        FilterGroup { op, items }
    };
    (!(group.op == GroupOp::And && group.items.is_empty())).then_some(group)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example() -> Value {
        json!({
            "id": "k3f9a2bc",
            "name": "Open tasks",
            "layout": "calendar",
            "properties": ["p-status"],
            "sort": { "by": "p-due", "dir": "asc" },
            "filter": {
                "op": "and",
                "items": [
                    { "propertyId": "p-status", "op": "hasOption", "value": "o-todo" },
                    { "op": "or", "items": [
                        { "propertyId": "p-prio", "op": "hasOption", "value": "o-high" },
                        { "propertyId": "p-due", "op": "isSet" }
                    ] }
                ]
            },
            "openIn": "center",
            "dateBy": "p-due",
            "endBy": "p-end",
            "cardSize": "l",
            "somethingNew": { "kept": true }
        })
    }

    #[test]
    fn a_view_round_trips_with_its_filter_tree_and_the_keys_it_does_not_know() {
        let view: PageListView = serde_json::from_value(example()).unwrap();
        assert_eq!(view.id, "k3f9a2bc");
        assert_eq!(view.layout, "calendar");
        assert_eq!(view.date_by.as_deref(), Some("p-due"));
        assert_eq!(view.end_by.as_deref(), Some("p-end"));
        assert_eq!(view.extra["somethingNew"], json!({ "kept": true }));
        assert_eq!(serde_json::to_value(&view).unwrap(), example());
    }

    #[test]
    fn a_filter_tree_tells_groups_from_filters_and_counts_and_visits_them() {
        let mut view: PageListView = serde_json::from_value(example()).unwrap();
        let mut filter = view.filter.take().unwrap();
        assert_eq!(filter.op, GroupOp::And);
        assert!(matches!(filter.items[0], FilterNode::Filter(_)));
        assert!(matches!(&filter.items[1], FilterNode::Group(group) if group.op == GroupOp::Or));
        assert_eq!(filter.count_filters(), 3);
        let mut seen = Vec::new();
        filter.for_each_filter_mut(&mut |f| {
            seen.push(f.property_id.clone());
            f.property_id = format!("moved-{}", f.property_id);
        });
        assert_eq!(seen, ["p-status", "p-prio", "p-due"]);
        assert_eq!(filter.count_filters(), 3);
        assert!(
            matches!(&filter.items[0], FilterNode::Filter(f) if f.property_id == "moved-p-status")
        );
    }

    #[test]
    fn an_older_view_without_the_new_keys_reads_and_writes_without_adding_any() {
        let stored = json!({ "id": "default", "name": "", "layout": "table", "properties": [], "sort": { "by": "title", "dir": "asc" } });
        let view: PageListView = serde_json::from_value(stored.clone()).unwrap();
        assert!(view.filter.is_none() && view.open_in.is_none() && view.extra.is_empty());
        assert_eq!(serde_json::to_value(&view).unwrap(), stored);
    }

    /// The `views` of the design document's example (section 2.2), with plain ids.
    fn golden_views() -> Value {
        json!([
            { "id": "k3f9a2bc", "name": "", "layout": "table", "properties": ["p-status", "p-due"],
              "sort": { "by": "p-due", "dir": "asc" },
              "filter": { "op": "and", "items": [
                  { "propertyId": "p-status", "op": "hasOption", "value": "o-todo" },
                  { "op": "or", "items": [
                      { "propertyId": "p-prio", "op": "hasOption", "value": "o-high" },
                      { "propertyId": "p-due", "op": "isSet" } ] } ] },
              "columnWidths": { "title": 320 } },
            { "id": "b81d00e4", "name": "By status", "layout": "board", "properties": ["p-due"],
              "sort": { "by": "title", "dir": "asc" }, "groupBy": "p-status", "hiddenGroups": ["none"],
              "openIn": "center" },
            { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "properties": ["p-status"],
              "sort": { "by": "title", "dir": "asc" }, "dateBy": "p-due", "endBy": "p-end" },
            { "id": "e2a9b6f0", "name": "", "layout": "gallery", "properties": [],
              "sort": { "by": "created", "dir": "desc" }, "cardSize": "l" }
        ])
    }

    fn attrs_with(views: Value) -> Map<String, Value> {
        let mut attrs = Map::new();
        attrs.insert("views".to_owned(), views);
        attrs
    }

    fn read(views: Value) -> Vec<PageListView> {
        read_views(&attrs_with(views)).unwrap_or_default()
    }

    #[test]
    fn the_golden_views_of_the_design_document_read_and_write_back_unchanged() {
        let views = read(golden_views());
        assert_eq!(views.len(), 4);
        assert_eq!(views[0].layout, "table");
        assert_eq!(
            views[0].column_widths.as_ref().unwrap()["title"].as_u64(),
            Some(320)
        );
        assert_eq!(views[1].open_in.as_deref(), Some("center"));
        assert_eq!(views[2].date_by.as_deref(), Some("p-due"));
        assert_eq!(views[3].card_size.as_deref(), Some("l"));
        let written = serde_json::to_value(&views).unwrap();
        assert_eq!(written, golden_views());
    }

    #[test]
    fn a_block_without_usable_views_reads_as_none() {
        assert_eq!(read_views(&Map::new()), None);
        for stored in [
            json!(null),
            json!("views"),
            json!(7),
            json!({}),
            json!([]),
            json!([1, "x", null, [], {}, { "id": "" }, { "id": 5 }, { "id": "a".repeat(65) }]),
        ] {
            assert_eq!(read_views(&attrs_with(stored.clone())), None, "{stored}");
        }
    }

    #[test]
    fn unusable_entries_are_skipped_the_first_of_a_repeated_id_wins_and_twenty_are_read() {
        let views = read(json!([
            "nope",
            { "id": "a", "name": "first", "layout": "list" },
            { "id": "a", "name": "second", "layout": "table" },
            { "name": "no id" },
            { "id": "b", "layout": "board" }
        ]));
        let ids: Vec<&str> = views.iter().map(|view| view.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(views[0].name, "first");

        let many: Vec<Value> = (0..30).map(|n| json!({ "id": format!("v{n}") })).collect();
        let views = read(Value::Array(many));
        assert_eq!(views.len(), MAX_VIEWS);
        assert_eq!(views[19].id, "v19");
        // An id of exactly 64 characters is usable.
        assert_eq!(read(json!([{ "id": "x".repeat(64) }])).len(), 1);
    }

    #[test]
    fn a_part_that_cannot_be_used_reads_as_its_default_and_the_rest_is_kept() {
        let long_name = "é".repeat(100);
        let views = read(json!([{
            "id": "v",
            "name": long_name,
            "layout": "timeline",
            "properties": ["a", "", 3, "a", "b"],
            "sort": { "by": "", "dir": "sideways" },
            "filter": "not a group",
            "columnWidths": { "title": 200, "x": -1, "y": "wide", "__proto__": 5 },
            "groupBy": "",
            "hiddenGroups": ["none", "none", 4, ""],
            "calculations": { "title": "count_all", "x": 3, "": "sum", "__proto__": "sum", "y": "" },
            "openIn": "sideways",
            "dateBy": 9,
            "endBy": "",
            "cardSize": 7,
            "somethingNew": { "kept": true },
            "__proto__": { "evil": true }
        }]));
        let view = &views[0];
        assert_eq!(view.name.chars().count(), MAX_VIEW_NAME_CHARS);
        assert_eq!(
            view.layout, "timeline",
            "an unknown layout is kept as stored"
        );
        assert_eq!(view.properties, ["a", "b"]);
        let sort = view.sort.as_ref().unwrap();
        assert_eq!((sort.by.as_str(), sort.dir.as_str()), ("title", "asc"));
        assert_eq!(view.filter, None);
        assert_eq!(view.column_widths.as_ref().unwrap().len(), 1);
        assert_eq!(view.group_by, None);
        assert_eq!(
            view.hidden_groups.as_deref(),
            Some(&["none".to_owned()][..])
        );
        assert_eq!(view.calculations.as_ref().unwrap().len(), 1);
        assert_eq!(view.open_in, None);
        assert_eq!(
            (view.date_by.as_deref(), view.end_by.as_deref()),
            (None, None)
        );
        assert_eq!(view.card_size, None);
        // What this version cannot use of a known key, and what it does not know, is kept.
        assert_eq!(view.extra["openIn"], json!("sideways"));
        assert_eq!(view.extra["cardSize"], json!(7));
        assert_eq!(view.extra["somethingNew"], json!({ "kept": true }));
        assert!(!view.extra.contains_key("__proto__"));
        assert!(
            !view.extra.contains_key("dateBy"),
            "a known key with no usable value is not kept"
        );
    }

    #[test]
    fn a_stored_openin_and_cardsize_of_this_version_are_typed() {
        let views = read(json!([{ "id": "v", "openIn": "page", "cardSize": "s" }]));
        assert_eq!(views[0].open_in.as_deref(), Some("page"));
        assert_eq!(views[0].card_size.as_deref(), Some("s"));
        assert!(views[0].extra.is_empty());
        // `null` is no value: it is not kept either.
        let views = read(json!([{ "id": "v", "openIn": null, "cardSize": null }]));
        assert!(views[0].extra.is_empty());
    }

    #[test]
    fn several_sort_keys_are_typed_and_written_back_as_read() {
        let stored = json!({
            "id": "v", "name": "", "layout": "table", "properties": [],
            "sort": { "by": "p-due", "dir": "desc" },
            "sorts": [
                { "by": "p-due", "dir": "desc" },
                { "by": "title", "dir": "asc" },
                { "by": "created", "dir": "desc" }
            ]
        });
        let view = read_view(&stored).unwrap();
        let keys: Vec<(&str, &str)> = view
            .sorts
            .iter()
            .map(|key| (key.by.as_str(), key.dir.as_str()))
            .collect();
        assert_eq!(
            keys,
            [("p-due", "desc"), ("title", "asc"), ("created", "desc")]
        );
        assert!(
            !view.extra.contains_key("sorts"),
            "sorts is a known key, not a kept one"
        );
        assert_eq!(serde_json::to_value(&view).unwrap(), stored);
        // The serde form (the one the golden test uses) agrees with the lenient reader.
        let typed: PageListView = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(typed, view);
    }

    #[test]
    fn a_view_without_sorts_reads_none_and_writes_none() {
        let stored =
            json!({ "id": "v", "layout": "list", "sort": { "by": "title", "dir": "asc" } });
        let view = read_view(&stored).unwrap();
        assert!(view.sorts.is_empty());
        let written = serde_json::to_value(&view).unwrap();
        assert!(written.get("sorts").is_none());
        assert_eq!(written["sort"], stored["sort"]);
    }

    #[test]
    fn sorts_read_usable_keys_only_the_first_of_a_repeated_by_and_at_most_five() {
        let view = read_view(&json!({
            "id": "v",
            "sorts": [
                "junk", 4, null, [],
                { "by": "" },
                { "dir": "desc" },
                { "by": 3 },
                { "by": "a", "dir": "sideways" },
                { "by": "a", "dir": "desc" },
                { "by": "b", "dir": "desc" },
                { "by": "c" },
                { "by": "d", "dir": "desc" },
                { "by": "e" },
                { "by": "f" },
                { "by": "g" }
            ]
        }))
        .unwrap();
        let keys: Vec<(&str, &str)> = view
            .sorts
            .iter()
            .map(|key| (key.by.as_str(), key.dir.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("a", "asc"),
                ("b", "desc"),
                ("c", "asc"),
                ("d", "desc"),
                ("e", "asc")
            ],
            "a repeated by keeps the first, an unusable dir is ascending, five at most"
        );
        assert_eq!(view.sorts.len(), MAX_SORTS);
        // Skipped entries do not use up the five.
        let view =
            read_view(&json!({ "id": "v", "sorts": [{ "by": "" }, { "by": "x" }] })).unwrap();
        assert_eq!(view.sorts.len(), 1);
    }

    #[test]
    fn sorts_that_cannot_be_used_read_as_none_and_are_not_kept() {
        for stored in [
            json!("title"),
            json!({ "by": "title" }),
            json!([]),
            json!([{ "by": "" }, 5]),
            json!(null),
        ] {
            let view = read_view(
                &json!({ "id": "v", "sort": { "by": "p-due", "dir": "asc" }, "sorts": stored }),
            )
            .unwrap();
            assert!(view.sorts.is_empty(), "{stored}");
            assert!(!view.extra.contains_key("sorts"), "{stored}");
            assert_eq!(view.sort.as_ref().unwrap().by, "p-due", "{stored}");
        }
    }

    fn filter(property: &str) -> Value {
        json!({ "propertyId": property, "op": "isSet" })
    }

    #[test]
    fn the_new_filter_operators_and_relative_values_pass_through_untouched() {
        let stored = json!({
            "id": "v", "layout": "table",
            "filter": { "op": "and", "items": [
                { "propertyId": "p-status", "op": "lacksOption", "value": "o-done" },
                { "propertyId": "p-rel", "op": "notRelatesTo", "value": "page-1" },
                { "propertyId": "p-due", "op": "onOrBefore", "value": { "relative": "today" } },
                { "propertyId": "p-due", "op": "within", "value": { "relative": "pastDays", "n": 7 } }
            ] }
        });
        let view = read_view(&stored).unwrap();
        assert_eq!(view.filter.as_ref().unwrap().count_filters(), 4);
        assert_eq!(
            serde_json::to_value(&view).unwrap()["filter"],
            stored["filter"]
        );
    }

    #[test]
    fn a_filter_tree_reads_like_the_typescript_normalise() {
        // A flat list (the filters of a block before ADR-0054) is an AND group; the empty one is the default.
        assert_eq!(
            read_filter_group(&json!([filter("a"), { "op": "x" }, 4])),
            Some(FilterGroup {
                op: GroupOp::And,
                items: vec![FilterNode::Filter(read_filter(&filter("a")).unwrap())],
            })
        );
        assert_eq!(read_filter_group(&json!([])), None);
        assert_eq!(
            read_filter_group(&json!({ "op": "and", "items": [] })),
            None
        );
        assert_eq!(
            read_filter_group(&json!({ "op": "xor", "items": [] })),
            None
        );
        assert_eq!(read_filter_group(&json!({ "op": "and" })), None);
        assert_eq!(read_filter_group(&json!(null)), None);
        // An empty OR group is a value: only the empty AND group is the default.
        assert!(read_filter_group(&json!({ "op": "or", "items": [] })).is_some());

        // A group inside a nested group is dropped, bad entries go, unknown operators stay.
        let group = read_filter_group(&json!({ "op": "and", "items": [
            { "propertyId": "a", "op": "someNewOperator", "value": { "n": 1 } },
            { "op": "or", "items": [
                filter("b"),
                { "op": "and", "items": [filter("deep")] },
                { "propertyId": "", "op": "isSet" },
                "junk"
            ] },
            7
        ] }))
        .unwrap();
        assert_eq!(group.count_filters(), 2);
        let FilterNode::Filter(first) = &group.items[0] else {
            panic!("a filter first");
        };
        assert_eq!(first.op, "someNewOperator");
        assert_eq!(first.value, Some(json!({ "n": 1 })));
        let FilterNode::Group(nested) = &group.items[1] else {
            panic!("a group second");
        };
        assert_eq!((nested.op, nested.items.len()), (GroupOp::Or, 1));
    }

    #[test]
    fn the_limits_of_fifty_filters_and_ten_groups_apply_in_reading_order() {
        let many: Vec<Value> = (0..70).map(|n| filter(&format!("p{n}"))).collect();
        let group = read_filter_group(&json!({ "op": "and", "items": many })).unwrap();
        assert_eq!(group.count_filters(), MAX_FILTERS);

        // Filters of the top level and of nested groups count together.
        let mut items: Vec<Value> = (0..30).map(|n| filter(&format!("p{n}"))).collect();
        items.push(json!({ "op": "or", "items": (0..30).map(|n| filter(&format!("q{n}"))).collect::<Vec<_>>() }));
        let group = read_filter_group(&json!({ "op": "and", "items": items })).unwrap();
        assert_eq!(group.count_filters(), MAX_FILTERS);

        let groups: Vec<Value> = (0..15)
            .map(|n| json!({ "op": "or", "items": [filter(&format!("g{n}"))] }))
            .collect();
        let group = read_filter_group(&json!({ "op": "and", "items": groups })).unwrap();
        assert_eq!(group.items.len(), MAX_FILTER_GROUPS);
    }

    #[test]
    fn reading_never_panics_on_whatever_is_stored() {
        let weird = [
            json!(null),
            json!(true),
            json!(1.5),
            json!("text"),
            json!([]),
            json!({}),
            json!([null, [null], { "id": [] }]),
            json!({ "id": "v", "properties": { "a": 1 }, "sort": [], "filter": [[], {}, null], "columnWidths": [1] }),
            json!({ "id": "v", "filter": { "op": "or", "items": [{ "op": "or", "items": "x" }] } }),
            json!({ "id": "v", "hiddenGroups": "none", "calculations": [], "layout": 4, "name": 5 }),
        ];
        for entry in weird {
            let _ = read_view(&entry);
            let _ = read_filter_group(&entry);
            let _ = read_views(&attrs_with(entry.clone()));
            let _ = read_views(&attrs_with(json!([entry])));
        }
    }
}
