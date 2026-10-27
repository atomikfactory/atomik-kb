//! The annotations of atomik-kb's Markdown (ADR-0036, sections 2.3 and 3.7): HTML comments
//! `<!-- atomik:{json} -->` that carry what Markdown cannot hold. Unknown or malformed ones read
//! as nothing.

use kb_types::OptionColor;
use serde_json::{Map, Value};

/// The largest annotation read, in bytes.
const MAX_ANNOTATION_BYTES: usize = 256 * 1024;
/// The most images one gallery takes.
pub(crate) const MAX_GALLERY_ITEMS: usize = 50;
/// The longest label a template button keeps, in characters (`kb_model`'s rule, ADR-0056).
pub(crate) const MAX_BUTTON_LABEL_CHARS: usize = 80;

/// One annotation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Annotation {
    /// Attributes of the next image or file block.
    Attrs(Map<String, Value>),
    /// The next quote is a callout.
    Callout {
        /// Its icon.
        icon: Option<String>,
        /// Its colour.
        color: Option<String>,
    },
    /// The next image blocks are one gallery.
    Gallery {
        /// Columns of the grid.
        columns: Option<i64>,
        /// How many image lines follow.
        items: usize,
    },
    /// A whiteboard, drawn in a `.excalidraw` file; the quote that follows is its summary.
    Whiteboard {
        /// Height of the inline view.
        height: Option<i64>,
        /// The link to its file.
        file: Option<String>,
    },
    /// The columns of a layout start.
    ColumnList,
    /// A column starts.
    Column,
    /// The columns of a layout end.
    EndColumnList,
    /// A template button starts; the blocks up to its end are its content (ADR-0056).
    TemplateButton {
        /// Its label (at most 80 characters).
        label: Option<String>,
        /// `above` or `below`.
        position: Option<String>,
    },
    /// A template button ends.
    EndTemplateButton,
    /// A table of contents.
    TableOfContents,
    /// A breadcrumb.
    Breadcrumb,
    /// A saved view, its schema references as names.
    PageList(Map<String, Value>),
}

/// Reads the JSON of an annotation.
pub(crate) fn parse(json: &str) -> Option<Annotation> {
    if json.len() > MAX_ANNOTATION_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(json).ok()?;
    let object = value.as_object()?;
    if let Some(end) = object.get("end") {
        return match end.as_str() {
            Some("columnList") => Some(Annotation::EndColumnList),
            Some("templateButton") => Some(Annotation::EndTemplateButton),
            _ => None,
        };
    }
    let attrs = object.get("attrs").and_then(Value::as_object);
    let Some(node) = object.get("node") else {
        return Some(Annotation::Attrs(media_attrs(attrs?)));
    };
    match node.as_str()? {
        "callout" => Some(Annotation::Callout {
            icon: attrs
                .and_then(|a| a.get("icon"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|icon| !icon.is_empty() && icon.len() <= 32)
                .map(str::to_owned),
            color: attrs
                .and_then(|a| a.get("color"))
                .and_then(Value::as_str)
                .filter(|color| OptionColor::parse(color).is_some())
                .map(str::to_owned),
        }),
        "gallery" => Some(Annotation::Gallery {
            columns: attrs
                .and_then(|a| a.get("columns"))
                .and_then(integer)
                .filter(|columns| (2..=4).contains(columns)),
            items: object
                .get("items")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(0)
                .min(MAX_GALLERY_ITEMS),
        }),
        "whiteboard" => Some(Annotation::Whiteboard {
            height: attrs
                .and_then(|a| a.get("height"))
                .and_then(integer)
                .filter(|height| (160..=1200).contains(height)),
            file: object
                .get("file")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "columnList" => Some(Annotation::ColumnList),
        "column" => Some(Annotation::Column),
        "templateButton" => Some(Annotation::TemplateButton {
            label: attrs
                .and_then(|a| a.get("label"))
                .and_then(Value::as_str)
                .map(|label| {
                    label
                        .chars()
                        .take(MAX_BUTTON_LABEL_CHARS)
                        .collect::<String>()
                })
                .filter(|label| !label.is_empty()),
            position: attrs
                .and_then(|a| a.get("position"))
                .and_then(Value::as_str)
                .filter(|position| matches!(*position, "below" | "above"))
                .map(str::to_owned),
        }),
        "tableOfContents" => Some(Annotation::TableOfContents),
        "breadcrumb" => Some(Annotation::Breadcrumb),
        "pageList" => Some(Annotation::PageList(page_list_attrs(attrs?))),
        _ => None,
    }
}

/// An integer, accepting a float with no fraction (`50.0`).
fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        let float = value.as_f64()?;
        // The check makes the conversion exact.
        (float.fract() == 0.0 && float.abs() < 1e15).then_some(float as i64)
    })
}

/// The attributes an image or a file block may take from an annotation, checked.
fn media_attrs(attrs: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(percent) = attrs
        .get("widthPercent")
        .and_then(integer)
        .filter(|p| (10..=100).contains(p))
    {
        out.insert("widthPercent".into(), Value::from(percent));
    }
    if let Some(width) = attrs
        .get("width")
        .and_then(integer)
        .filter(|w| (1..=100_000).contains(w))
    {
        out.insert("width".into(), Value::from(width));
    }
    if let Some(align) = attrs
        .get("align")
        .and_then(Value::as_str)
        .filter(|a| matches!(*a, "left" | "center" | "right"))
    {
        out.insert("align".into(), Value::String(align.to_owned()));
    }
    if attrs.get("view").and_then(Value::as_str) == Some("preview") {
        out.insert("view".into(), Value::String("preview".into()));
    }
    out
}

/// The attributes of a page list, the parts that are not the right shape left out: the block's own and,
/// when it holds several views (ADR-0054), a `views` list of at most 20 views, each shaped like the block's
/// attributes plus what a view adds (`id`, `name`, `openIn`, `cardSize`, `dateBy`, `endBy`, `sorts` and the
/// `filter` tree).
fn page_list_attrs(attrs: &Map<String, Value>) -> Map<String, Value> {
    let mut out = shaped_attrs(attrs, true);
    if let Some(Value::Array(views)) = attrs.get("views") {
        let list: Vec<Value> = views
            .iter()
            .filter_map(Value::as_object)
            .take(MAX_VIEWS)
            .map(|view| Value::Object(shaped_attrs(view, false)))
            .collect();
        if !list.is_empty() {
            out.insert("views".into(), Value::Array(list));
        }
    }
    out
}

/// The most views a block holds, filters a view holds (nested groups included) and groups nested in a
/// view's top-level filter group (`kb_model::views`).
const MAX_VIEWS: usize = 20;
const MAX_FILTERS: usize = 50;
const MAX_FILTER_GROUPS: usize = 10;
const MAX_VIEW_ID_CHARS: usize = 64;
const MAX_VIEW_NAME_CHARS: usize = 80;

/// One set of attributes, a block's (`is_block`) or one of its views': the parts that are not the right
/// shape are left out. Only a block has a `source`; a view adds its own keys.
fn shaped_attrs(attrs: &Map<String, Value>, is_block: bool) -> Map<String, Value> {
    let mut out = Map::new();
    if !is_block {
        if let Some(id) = attrs
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.chars().count() <= MAX_VIEW_ID_CHARS)
        {
            out.insert("id".into(), Value::String(id.to_owned()));
        }
        if let Some(name) = attrs.get("name").and_then(Value::as_str) {
            let name: String = name.chars().take(MAX_VIEW_NAME_CHARS).collect();
            out.insert("name".into(), Value::String(name));
        }
        if let Some(open_in) = attrs
            .get("openIn")
            .and_then(Value::as_str)
            .filter(|open_in| matches!(*open_in, "side" | "center" | "page"))
        {
            out.insert("openIn".into(), Value::String(open_in.to_owned()));
        }
        if let Some(size) = attrs
            .get("cardSize")
            .and_then(Value::as_str)
            .filter(|size| matches!(*size, "s" | "m" | "l"))
        {
            out.insert("cardSize".into(), Value::String(size.to_owned()));
        }
        for key in ["dateBy", "endBy"] {
            if let Some(name) = attrs
                .get(key)
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            {
                out.insert(key.into(), Value::String(name.to_owned()));
            }
        }
        if let Some(tree) = attrs.get("filter").and_then(filter_tree) {
            out.insert("filter".into(), tree);
        }
        if let Some(sorts) = attrs.get("sorts").and_then(sort_keys) {
            out.insert("sorts".into(), sorts);
        }
        if let Some(Value::Array(widgets)) = attrs.get("widgets") {
            let list: Vec<Value> = widgets
                .iter()
                .filter_map(Value::as_object)
                .take(MAX_WIDGETS)
                .map(shaped_widget)
                .collect();
            if !list.is_empty() {
                out.insert("widgets".into(), Value::Array(list));
            }
        }
    }
    if let Some(layout) = attrs.get("layout").and_then(Value::as_str) {
        out.insert("layout".into(), Value::String(layout.to_owned()));
    }
    if let Some(Value::Array(list)) = attrs.get("properties") {
        let names: Vec<Value> = list
            .iter()
            .filter(|name| name.as_str().is_some_and(|s| !s.is_empty()))
            .cloned()
            .collect();
        out.insert("properties".into(), Value::Array(names));
    }
    if let Some(Value::Object(sort)) = attrs.get("sort")
        && sort.get("by").and_then(Value::as_str).is_some()
        && matches!(
            sort.get("dir").and_then(Value::as_str),
            Some("asc" | "desc")
        )
    {
        out.insert("sort".into(), Value::Object(sort.clone()));
    }
    if is_block
        && let Some(Value::Object(source)) = attrs.get("source")
        && source.get("kind").and_then(Value::as_str).is_some()
    {
        out.insert("source".into(), Value::Object(source.clone()));
    }
    if let Some(Value::Array(filters)) = attrs.get("filters") {
        let list: Vec<Value> = filters
            .iter()
            .filter(|filter| {
                filter.get("property").and_then(Value::as_str).is_some()
                    && filter.get("op").and_then(Value::as_str).is_some()
            })
            .cloned()
            .collect();
        out.insert("filters".into(), Value::Array(list));
    }
    if let Some(group_by) = attrs.get("groupBy").and_then(Value::as_str) {
        out.insert("groupBy".into(), Value::String(group_by.to_owned()));
    }
    if let Some(Value::Array(list)) = attrs.get("hiddenGroups") {
        let names: Vec<Value> = list
            .iter()
            .filter(|name| name.is_string())
            .cloned()
            .collect();
        out.insert("hiddenGroups".into(), Value::Array(names));
    }
    if let Some(Value::Object(widths)) = attrs.get("columnWidths") {
        let kept: Map<String, Value> = widths
            .iter()
            .filter(|(_, width)| width.as_f64().is_some_and(|w| w.is_finite() && w > 0.0))
            .map(|(name, width)| (name.clone(), width.clone()))
            .collect();
        out.insert("columnWidths".into(), Value::Object(kept));
    }
    if let Some(Value::Object(calculations)) = attrs.get("calculations") {
        let kept: Map<String, Value> = calculations
            .iter()
            .filter(|(_, function)| function.as_str().is_some_and(|s| !s.is_empty()))
            .map(|(name, function)| (name.clone(), function.clone()))
            .collect();
        out.insert("calculations".into(), Value::Object(kept));
    }
    out
}

/// The most widgets a dashboard view holds (`dashboards.md`).
const MAX_WIDGETS: usize = 12;

/// One widget of a dashboard view (ADR-0055), the parts that are not the right shape left out: its
/// `filter` is a filter tree like a view's, `calc` and `chart` must be objects (a `calc` also needs a
/// text `by`, a `chart` is the chart's own spec with names for its property ids), and `id`, `kind`,
/// `title`, `w`, `h` and `viewId` keep their JSON type. Keys this version does not know are dropped like
/// those of a view.
fn shaped_widget(widget: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    for key in ["id", "kind", "title", "viewId"] {
        if let Some(text) = widget.get(key).filter(|value| value.is_string()) {
            out.insert(key.into(), text.clone());
        }
    }
    for key in ["w", "h"] {
        if let Some(value) = widget
            .get(key)
            .filter(|value| value.is_u64() || value.is_string())
        {
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(tree) = widget.get("filter").and_then(filter_tree) {
        out.insert("filter".into(), tree);
    }
    if let Some(Value::Object(calc)) = widget.get("calc")
        && calc.get("by").is_some_and(Value::is_string)
    {
        out.insert("calc".into(), Value::Object(calc.clone()));
    }
    if let Some(Value::Object(chart)) = widget.get("chart") {
        out.insert("chart".into(), Value::Object(chart.clone()));
    }
    Value::Object(out)
}

/// The `sorts` of a view with names (ADR-0058): the entries that name a `by` and a direction (`asc` or
/// `desc`), as `{ "by", "dir" }`, in order; the first of a repeated `by` wins and at most
/// [`kb_model::views::MAX_SORTS`] are kept. `None` when none is left.
fn sort_keys(value: &Value) -> Option<Value> {
    let mut keys: Vec<Value> = Vec::new();
    for key in value.as_array()?.iter().filter_map(Value::as_object) {
        let Some(by) = key
            .get("by")
            .and_then(Value::as_str)
            .filter(|by| !by.is_empty())
        else {
            continue;
        };
        let Some(dir) = key
            .get("dir")
            .and_then(Value::as_str)
            .filter(|dir| matches!(*dir, "asc" | "desc"))
        else {
            continue;
        };
        if keys.iter().any(|seen| seen["by"] == by) {
            continue;
        }
        keys.push(serde_json::json!({ "by": by, "dir": dir }));
        if keys.len() >= kb_model::views::MAX_SORTS {
            break;
        }
    }
    (!keys.is_empty()).then_some(Value::Array(keys))
}

/// Whether a filter of an annotation names a property and an operator.
fn is_named_filter(filter: &Value) -> bool {
    filter.get("property").and_then(Value::as_str).is_some()
        && filter.get("op").and_then(Value::as_str).is_some()
}

/// A filter tree of a view with names: `{ "op": "and" | "or", "items": [filters and groups] }`. A group of the
/// top level may nest groups that hold filters only; entries of another shape are left out, and so are the filters
/// and groups past the limits (the first 50 filters and 10 groups, in reading order). The empty AND group (the
/// default) is no tree. `None` for what is no group.
fn filter_tree(value: &Value) -> Option<Value> {
    let (op, items) = group_parts(value)?;
    let (mut filters, mut groups) = (0_usize, 0_usize);
    let mut kept: Vec<Value> = Vec::new();
    for item in items {
        if let Some((inner_op, inner_items)) = group_parts(item) {
            if groups >= MAX_FILTER_GROUPS {
                continue;
            }
            groups += 1;
            let mut inner: Vec<Value> = Vec::new();
            for filter in inner_items
                .iter()
                .filter(|entry| group_parts(entry).is_none() && is_named_filter(entry))
            {
                if filters >= MAX_FILTERS {
                    break;
                }
                filters += 1;
                inner.push(filter.clone());
            }
            kept.push(serde_json::json!({ "op": inner_op, "items": inner }));
        } else if is_named_filter(item) && filters < MAX_FILTERS {
            filters += 1;
            kept.push(item.clone());
        }
    }
    (!(op == "and" && kept.is_empty())).then(|| serde_json::json!({ "op": op, "items": kept }))
}

/// A group of an annotation: an object with the operator `and` or `or` and a list of `items`.
fn group_parts(value: &Value) -> Option<(&'static str, &Vec<Value>)> {
    let object = value.as_object()?;
    let op = match object.get("op").and_then(Value::as_str)? {
        "and" => "and",
        "or" => "or",
        _ => return None,
    };
    Some((op, object.get("items")?.as_array()?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn each_form_of_the_spec_is_read() {
        assert_eq!(
            parse(r#"{"attrs":{"widthPercent":60,"align":"right","width":640}}"#),
            Some(Annotation::Attrs(
                json!({"widthPercent": 60, "align": "right", "width": 640})
                    .as_object()
                    .unwrap()
                    .clone()
            ))
        );
        assert_eq!(
            parse(r#"{"attrs":{"view":"preview"}}"#),
            Some(Annotation::Attrs(
                json!({"view": "preview"}).as_object().unwrap().clone()
            ))
        );
        assert_eq!(
            parse(r#"{"node":"callout","attrs":{"color":"blue","icon":"💡"}}"#),
            Some(Annotation::Callout {
                icon: Some("💡".into()),
                color: Some("blue".into())
            })
        );
        assert_eq!(
            parse(r#"{"node":"callout","attrs":{"icon":"💡"}}"#),
            Some(Annotation::Callout {
                icon: Some("💡".into()),
                color: None
            })
        );
        assert_eq!(
            parse(r#"{"attrs":{"columns":3},"items":4,"node":"gallery"}"#),
            Some(Annotation::Gallery {
                columns: Some(3),
                items: 4
            })
        );
        assert_eq!(
            parse(
                r#"{"attrs":{"height":480},"file":"Page/Whiteboard.excalidraw","node":"whiteboard"}"#
            ),
            Some(Annotation::Whiteboard {
                height: Some(480),
                file: Some("Page/Whiteboard.excalidraw".into())
            })
        );
        assert_eq!(
            parse(r#"{"node":"columnList"}"#),
            Some(Annotation::ColumnList)
        );
        assert_eq!(parse(r#"{"node":"column"}"#), Some(Annotation::Column));
        assert_eq!(
            parse(r#"{"end":"columnList"}"#),
            Some(Annotation::EndColumnList)
        );
        assert_eq!(
            parse(r#"{"attrs":{"label":"Add task","position":"above"},"node":"templateButton"}"#),
            Some(Annotation::TemplateButton {
                label: Some("Add task".into()),
                position: Some("above".into())
            })
        );
        assert_eq!(
            parse(r#"{"node":"templateButton"}"#),
            Some(Annotation::TemplateButton {
                label: None,
                position: None
            })
        );
        assert_eq!(
            parse(r#"{"end":"templateButton"}"#),
            Some(Annotation::EndTemplateButton)
        );
        assert_eq!(parse(r#"{"end":"table"}"#), None);
        assert_eq!(
            parse(r#"{"node":"tableOfContents"}"#),
            Some(Annotation::TableOfContents)
        );
        assert_eq!(
            parse(r#"{"node":"breadcrumb"}"#),
            Some(Annotation::Breadcrumb)
        );
    }

    #[test]
    fn template_button_values_are_checked() {
        let long = "x".repeat(200);
        let text = format!(
            r#"{{"node":"templateButton","attrs":{{"label":"{long}","position":"middle"}}}}"#
        );
        let Some(Annotation::TemplateButton { label, position }) = parse(&text) else {
            panic!("a template button");
        };
        assert_eq!(
            label.map(|label| label.chars().count()),
            Some(MAX_BUTTON_LABEL_CHARS)
        );
        assert_eq!(position, None);
        assert_eq!(
            parse(r#"{"node":"templateButton","attrs":{"label":3,"position":"below"}}"#),
            Some(Annotation::TemplateButton {
                label: None,
                position: Some("below".into())
            })
        );
    }

    #[test]
    fn the_double_hyphen_escape_decodes() {
        let text = format!(
            r#"{{"node":"callout","attrs":{{"icon":"a-{}d"}}}}"#,
            "\\u002d"
        );
        assert_eq!(
            parse(&text),
            Some(Annotation::Callout {
                icon: Some("a--d".into()),
                color: None
            })
        );
    }

    #[test]
    fn values_are_checked() {
        let Some(Annotation::Attrs(attrs)) = parse(
            r#"{"attrs":{"widthPercent":5,"align":"middle","width":-3,"view":"card","x":1}}"#,
        ) else {
            panic!("attrs");
        };
        assert!(attrs.is_empty());
        let Some(Annotation::Attrs(attrs)) = parse(r#"{"attrs":{"widthPercent":50.0}}"#) else {
            panic!("attrs");
        };
        assert_eq!(attrs["widthPercent"], 50);
        assert_eq!(
            parse(r#"{"node":"callout","attrs":{"color":"mauve","icon":""}}"#),
            Some(Annotation::Callout {
                icon: None,
                color: None
            })
        );
        assert_eq!(
            parse(r#"{"node":"gallery","attrs":{"columns":9},"items":500}"#),
            Some(Annotation::Gallery {
                columns: None,
                items: MAX_GALLERY_ITEMS
            })
        );
        assert_eq!(
            parse(r#"{"node":"whiteboard","attrs":{"height":5}}"#),
            Some(Annotation::Whiteboard {
                height: None,
                file: None
            })
        );
    }

    #[test]
    fn page_lists_keep_the_parts_that_fit() {
        let text = json!({
            "node": "pageList",
            "attrs": {
                "layout": "table",
                "properties": ["Status", "", 3],
                "sort": {"by": "Due", "dir": "desc"},
                "source": {"kind": "tag", "option": "work"},
                "filters": [{"property": "Status", "op": "hasOption", "value": "Done"}, {"op": "isSet"}],
                "groupBy": "Status",
                "hiddenGroups": ["none", 4],
                "columnWidths": {"title": 300, "Due": -1, "X": "wide"},
                "calculations": {"title": "count_all", "Due": "sum", "X": 3, "Y": ""},
                "unknown": true
            }
        })
        .to_string();
        let Some(Annotation::PageList(attrs)) = parse(&text) else {
            panic!("page list");
        };
        assert_eq!(attrs["layout"], "table");
        assert_eq!(attrs["properties"], json!(["Status"]));
        assert_eq!(attrs["sort"], json!({"by": "Due", "dir": "desc"}));
        assert_eq!(attrs["filters"].as_array().map(Vec::len), Some(1));
        assert_eq!(attrs["hiddenGroups"], json!(["none"]));
        assert_eq!(attrs["columnWidths"], json!({"title": 300}));
        assert_eq!(
            attrs["calculations"],
            json!({"title": "count_all", "Due": "sum"})
        );
        assert!(!attrs.contains_key("unknown"));
    }

    fn page_list(attrs: Value) -> Map<String, Value> {
        let text = json!({ "node": "pageList", "attrs": attrs }).to_string();
        let Some(Annotation::PageList(attrs)) = parse(&text) else {
            panic!("page list");
        };
        attrs
    }

    #[test]
    fn page_lists_carry_their_views_shaped_like_the_block() {
        let attrs = page_list(json!({
            "layout": "table",
            "views": [
                { "id": "k3f9a2bc", "name": "", "layout": "table", "properties": ["Status", 3, ""],
                  "sort": { "by": "Due", "dir": "asc" },
                  "filter": { "op": "and", "items": [
                      { "property": "Status", "op": "hasOption", "value": "Done" },
                      { "op": "isSet" },
                      { "op": "or", "items": [
                          { "property": "Due", "op": "isSet" },
                          { "propertyId": "not named", "op": "isSet" },
                          { "op": "and", "items": [{ "property": "deep", "op": "isSet" }] } ] },
                      "junk" ] },
                  "columnWidths": { "title": 320, "Due": -1 }, "calculations": { "Due": "sum", "X": "" },
                  "unknown": true, "source": { "kind": "children" } },
                { "id": "b81d00e4", "name": "By status", "layout": "board", "groupBy": "Status",
                  "hiddenGroups": ["Done", "none", 4], "openIn": "center" },
                { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "dateBy": "Due", "endBy": "End" },
                { "id": "e2a9b6f0", "layout": "gallery", "dateBy": "created", "cardSize": "l" },
                "not a view",
                7
            ]
        }));
        let views = attrs["views"].as_array().unwrap();
        assert_eq!(views.len(), 4);
        assert_eq!(
            views[0],
            json!({ "id": "k3f9a2bc", "name": "", "layout": "table", "properties": ["Status"],
                    "sort": { "by": "Due", "dir": "asc" },
                    "filter": { "op": "and", "items": [
                        { "property": "Status", "op": "hasOption", "value": "Done" },
                        { "op": "or", "items": [{ "property": "Due", "op": "isSet" }] } ] },
                    "columnWidths": { "title": 320 }, "calculations": { "Due": "sum" } })
        );
        assert_eq!(
            views[1],
            json!({ "id": "b81d00e4", "name": "By status", "layout": "board", "groupBy": "Status",
                    "hiddenGroups": ["Done", "none"], "openIn": "center" })
        );
        assert_eq!(
            views[2],
            json!({ "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "dateBy": "Due", "endBy": "End" })
        );
        assert_eq!(
            views[3],
            json!({ "id": "e2a9b6f0", "layout": "gallery", "dateBy": "created", "cardSize": "l" })
        );
        // The block's own attributes are as before, and a view has no source of its own.
        assert_eq!(attrs["layout"], "table");
        assert!(views[0].get("source").is_none() && views[0].get("unknown").is_none());
    }

    #[test]
    fn the_sorts_of_a_view_keep_the_keys_that_fit_and_the_new_operators_and_sources_pass() {
        let attrs = page_list(json!({
            "source": { "kind": "page", "pageId": "01010101-0101-4101-8101-010101010101" },
            "views": [
                { "id": "a", "layout": "table",
                  "sort": { "by": "Due", "dir": "desc" },
                  "sorts": [
                      { "by": "Due", "dir": "desc", "x": 1 }, "junk", 3, { "by": "", "dir": "asc" },
                      { "by": "Name", "dir": "sideways" }, { "dir": "asc" },
                      { "by": "Due", "dir": "asc" },
                      { "by": "Status", "dir": "asc" }, { "by": "created", "dir": "desc" },
                      { "by": "Rank", "dir": "asc" }, { "by": "Size", "dir": "asc" },
                      { "by": "Cost", "dir": "asc" }
                  ],
                  "filter": { "op": "and", "items": [
                      { "property": "Status", "op": "lacksOption", "value": "Done" },
                      { "property": "Tasks", "op": "notRelatesTo", "value": "01010101-0101-4101-8101-010101010101" },
                      { "property": "Due", "op": "within", "value": { "relative": "thisWeek" } } ] } },
                { "id": "b", "sorts": [{ "by": "Due" }, "junk"] },
                { "id": "c", "sorts": "Due" },
                { "id": "d", "sorts": [] }
            ]
        }));
        let views = attrs["views"].as_array().unwrap();
        // At most five keys, the first of a repeated `by`, `{ by, dir }` only, the order kept.
        assert_eq!(
            views[0]["sorts"],
            json!([
                { "by": "Due", "dir": "desc" },
                { "by": "Status", "dir": "asc" },
                { "by": "created", "dir": "desc" },
                { "by": "Rank", "dir": "asc" },
                { "by": "Size", "dir": "asc" }
            ])
        );
        assert_eq!(views[0]["sort"], json!({ "by": "Due", "dir": "desc" }));
        assert_eq!(
            views[0]["filter"],
            json!({ "op": "and", "items": [
                { "property": "Status", "op": "lacksOption", "value": "Done" },
                { "property": "Tasks", "op": "notRelatesTo", "value": "01010101-0101-4101-8101-010101010101" },
                { "property": "Due", "op": "within", "value": { "relative": "thisWeek" } } ] })
        );
        // No usable key is no key (a key needs a direction, as `sort` does).
        for view in &views[1..] {
            assert!(view.get("sorts").is_none(), "{view}");
        }
        // The linked sources pass as they are.
        assert_eq!(
            attrs["source"],
            json!({ "kind": "page", "pageId": "01010101-0101-4101-8101-010101010101" })
        );
        assert_eq!(
            page_list(json!({ "source": { "kind": "space" } }))["source"],
            json!({ "kind": "space" })
        );
    }

    #[test]
    fn the_widgets_of_a_dashboard_view_are_shaped_and_limited() {
        let widget =
            |n: usize| json!({ "id": format!("w{n:07}"), "kind": "number", "w": 1, "h": "s" });
        let many: Vec<Value> = (0..20).map(widget).collect();
        let attrs = page_list(json!({ "views": [
            { "id": "d", "name": "Overview", "layout": "dashboard", "widgets": [
                { "id": "w0000001", "kind": "number", "title": "Hours", "w": 1, "h": "s",
                  "calc": { "by": "Hours", "fn": "sum" },
                  "filter": { "op": "and", "items": [
                      { "property": "Status", "op": "hasOption", "value": "Done" },
                      { "propertyId": "an id", "op": "isSet" } ] },
                  "unknown": 1 },
                { "id": "w0000002", "kind": "chart", "chart": { "type": "bar", "groupBy": "Status",
                  "measure": { "fn": "sum", "propertyId": "Cost" }, "future": true } },
                { "id": "w0000003", "kind": "view", "viewId": "k3f9a2bc", "calc": { "fn": "sum" }, "chart": "bar" },
                "not a widget", 7
            ] },
            { "id": "e", "layout": "dashboard", "widgets": many },
            { "id": "f", "layout": "dashboard", "widgets": "x" },
            { "id": "g", "layout": "dashboard", "widgets": [] }
        ] }));
        let views = attrs["views"].as_array().unwrap();
        assert_eq!(
            views[0]["widgets"],
            json!([
                { "id": "w0000001", "kind": "number", "title": "Hours", "w": 1, "h": "s",
                  "calc": { "by": "Hours", "fn": "sum" },
                  "filter": { "op": "and", "items": [{ "property": "Status", "op": "hasOption", "value": "Done" }] } },
                { "id": "w0000002", "kind": "chart", "chart": { "type": "bar", "groupBy": "Status",
                  "measure": { "fn": "sum", "propertyId": "Cost" }, "future": true } },
                { "id": "w0000003", "kind": "view", "viewId": "k3f9a2bc" }
            ])
        );
        // At most twelve widgets; no usable list is no key.
        assert_eq!(views[1]["widgets"].as_array().map(Vec::len), Some(12));
        assert!(views[2].get("widgets").is_none() && views[3].get("widgets").is_none());
    }

    #[test]
    fn views_are_limited_and_the_unusable_parts_of_a_view_are_left_out() {
        // At most twenty views, entries that are no object skipped.
        let many: Vec<Value> = (0..30)
            .map(|n| json!({ "id": format!("v{n}"), "layout": "list" }))
            .collect();
        let attrs = page_list(json!({ "views": many }));
        assert_eq!(attrs["views"].as_array().map(Vec::len), Some(MAX_VIEWS));
        // No usable view: the key is not there at all.
        for views in [
            json!([]),
            json!("views"),
            json!([1, null, "x"]),
            json!({ "id": "a" }),
        ] {
            assert!(!page_list(json!({ "layout": "list", "views": views })).contains_key("views"));
        }
        // Wrong values of a view's own keys are dropped one by one.
        let attrs = page_list(json!({ "views": [{
            "id": "x".repeat(65), "name": "n".repeat(100), "openIn": "elsewhere", "cardSize": "xl",
            "dateBy": "", "endBy": 3, "filter": "no tree"
        }] }));
        assert_eq!(attrs["views"][0], json!({ "name": "n".repeat(80) }));
        // The empty AND group is no tree; an empty OR group is one; the limits count in reading order.
        let attrs = page_list(json!({ "views": [
            { "id": "a", "filter": { "op": "and", "items": [] } },
            { "id": "b", "filter": { "op": "or", "items": [] } },
            { "id": "c", "filter": { "op": "xor", "items": [{ "property": "A", "op": "isSet" }] } },
        ] }));
        assert_eq!(attrs["views"][0], json!({ "id": "a" }));
        assert_eq!(
            attrs["views"][1],
            json!({ "id": "b", "filter": { "op": "or", "items": [] } })
        );
        assert_eq!(attrs["views"][2], json!({ "id": "c" }));
        let filters: Vec<Value> = (0..70)
            .map(|n| json!({ "property": format!("P{n}"), "op": "isSet" }))
            .collect();
        let attrs = page_list(
            json!({ "views": [{ "id": "a", "filter": { "op": "and", "items": filters } }] }),
        );
        assert_eq!(
            attrs["views"][0]["filter"]["items"]
                .as_array()
                .map(Vec::len),
            Some(MAX_FILTERS)
        );
        let groups: Vec<Value> = (0..15)
            .map(|n| json!({ "op": "or", "items": [{ "property": format!("P{n}"), "op": "isSet" }] }))
            .collect();
        let attrs = page_list(
            json!({ "views": [{ "id": "a", "filter": { "op": "and", "items": groups } }] }),
        );
        assert_eq!(
            attrs["views"][0]["filter"]["items"]
                .as_array()
                .map(Vec::len),
            Some(MAX_FILTER_GROUPS)
        );
    }

    #[test]
    fn malformed_annotations_read_as_nothing() {
        for text in [
            "",
            "{",
            "[]",
            "{}",
            r#"{"node":"mystery"}"#,
            r#"{"node":3}"#,
            r#"{"end":"table"}"#,
            r#"{"node":"pageList"}"#,
            r#"{"attrs":3}"#,
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
        assert_eq!(
            parse(&format!(
                "{{\"node\":\"column\",\"x\":\"{}\"}}",
                "a".repeat(300_000)
            )),
            None
        );
    }
}
