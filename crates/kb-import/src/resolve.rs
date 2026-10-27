//! `resolve` (ADR-0036, section 3.1): a planned page's content into portable JSON the core can
//! write, once the new page ids, the uploaded attachments and the property ids exist.

use std::str::FromStr;

use kb_types::properties::TAGS_NAME;
use kb_types::{AttachmentId, BlockId, ImportWarning, ImportWarningCode, PageId, Rng};
use serde_json::{Map, Value};

use crate::json::{self, kind};
use crate::paths;
use crate::placeholder::{self, Target};
use crate::{PlannedPage, Resolver};
use kb_model::schema::ID_BEARING;

/// The most warnings one page reports.
const MAX_WARNINGS: usize = 100;

/// The deepest nesting followed (plans never get near it; hostile data cannot recurse deeper).
const MAX_DEPTH: usize = 200;

pub(crate) fn run(
    page: &PlannedPage,
    resolver: &dyn Resolver,
    rng: &dyn Rng,
) -> (Value, Vec<ImportWarning>) {
    let mut walk = Walk {
        resolver,
        rng,
        path: &page.source_path,
        warnings: Vec::new(),
        whiteboards: Map::new(),
    };
    let mut content: Vec<Value> = page
        .content
        .get("content")
        .and_then(Value::as_array)
        .map(|nodes| nodes.iter().flat_map(|node| walk.node(node, 1)).collect())
        .unwrap_or_default();
    if content.is_empty() {
        content.push(walk.with_id(json::paragraph(Vec::new())));
    }
    let mut doc = Map::new();
    doc.insert("type".into(), Value::String("doc".into()));
    doc.insert("content".into(), Value::Array(content));
    if !walk.whiteboards.is_empty() {
        doc.insert("whiteboards".into(), Value::Object(walk.whiteboards));
    }
    (Value::Object(doc), walk.warnings)
}

struct Walk<'r> {
    resolver: &'r dyn Resolver,
    rng: &'r dyn Rng,
    path: &'r str,
    warnings: Vec<ImportWarning>,
    /// `{ block id: { element id: element } }`.
    whiteboards: Map<String, Value>,
}

impl Walk<'_> {
    fn warn(&mut self, code: ImportWarningCode, detail: &str) {
        let detail = paths::truncate_chars(detail, 200);
        let warning = ImportWarning {
            path: self.path.to_owned(),
            code,
            detail: Some(detail),
        };
        // A page with thousands of broken links reports the first hundred.
        if self.warnings.len() < MAX_WARNINGS && !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    fn fresh_id(&self) -> String {
        BlockId::generate(self.rng).to_string()
    }

    /// Gives a node a fresh `id` attribute.
    fn with_id(&self, mut node: Value) -> Value {
        json::set_attr(&mut node, "id", Value::String(self.fresh_id()));
        node
    }

    /// The nodes a planned node becomes (none, one, or a replacement).
    fn node(&mut self, node: &Value, depth: usize) -> Vec<Value> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        match kind(node) {
            "text" => vec![node.clone()],
            "pageRef" | "pageLink" => self.page_ref(node),
            "image" | "file" => self.media(node),
            "gallery" => self.gallery(node),
            "whiteboard" => self.whiteboard(node),
            "pageList" => self.page_list(node),
            _ => vec![self.element(node, depth)],
        }
    }

    /// An ordinary element: its attributes without placeholders, its children resolved, a fresh
    /// id when the schema gives it one.
    fn element(&mut self, node: &Value, depth: usize) -> Value {
        let name = kind(node).to_owned();
        let mut attrs = plain_attrs(node);
        if ID_BEARING.contains(&name.as_str()) {
            attrs.insert("id".into(), Value::String(self.fresh_id()));
        }
        let children: Vec<Value> = json::content(node)
            .iter()
            .flat_map(|child| self.node(child, depth + 1))
            .collect();
        json::element(&name, attrs, children)
    }

    // ----- page references -------------------------------------------------------------------

    fn page_ref(&mut self, node: &Value) -> Vec<Value> {
        let is_card = kind(node) == "pageLink";
        let target = node
            .get("attrs")
            .and_then(|a| a.get(placeholder::PAGE))
            .and_then(Target::from_json);
        let label = json::str_attr(node, placeholder::LABEL).unwrap_or_default();
        let mut block: Option<String> = None;
        let (page, missing) = match &target {
            Some(Target::Planned(index)) => (self.resolver.page(*index), label.to_owned()),
            Some(Target::Title(title) | Target::Wiki { name: title, .. }) => (
                self.resolver.page_by_title(title),
                if label.is_empty() {
                    format!("[[{title}]]")
                } else {
                    label.to_owned()
                },
            ),
            Some(Target::Existing {
                page,
                block: to_block,
            }) => {
                let id = PageId::from_str(page)
                    .ok()
                    .filter(|id| self.resolver.existing_page(*id));
                if id.is_some() {
                    block = to_block
                        .as_deref()
                        .and_then(|b| BlockId::from_str(b).ok())
                        .map(|b| b.to_string());
                }
                (id, label.to_owned())
            }
            None => (None, label.to_owned()),
        };
        match page {
            Some(page) => {
                let mut attrs = Map::new();
                attrs.insert("id".into(), Value::String(self.fresh_id()));
                attrs.insert("pageId".into(), Value::String(page.to_string()));
                if let Some(block) = block {
                    attrs.insert("blockId".into(), Value::String(block));
                }
                vec![json::element(
                    if is_card { "pageLink" } else { "pageRef" },
                    attrs,
                    Vec::new(),
                )]
            }
            None => {
                let detail = match &target {
                    Some(Target::Title(title) | Target::Wiki { name: title, .. }) => title.clone(),
                    Some(Target::Existing { page, .. }) => page.clone(),
                    _ if !missing.is_empty() => missing.clone(),
                    _ => "a page of the import".to_owned(),
                };
                self.warn(ImportWarningCode::UnresolvedLink, &detail);
                self.text_in_place(&missing, is_card)
            }
        }
    }

    /// Text in the place of something that could not be resolved: a text node inline, a paragraph
    /// for a block. Nothing when the text is empty.
    fn text_in_place(&self, text: &str, block: bool) -> Vec<Value> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        let text_node = json::text_node(text, &json::MarkSet::default());
        if block {
            vec![self.with_id(json::paragraph(vec![text_node]))]
        } else {
            vec![text_node]
        }
    }

    // ----- attachments -----------------------------------------------------------------------

    /// The attachment a placeholder names: a needed file by path or an attachment of the space by
    /// id. `Err` carries what to say about it.
    fn attachment(&self, holder: &Value) -> Result<AttachmentId, String> {
        if let Some(path) = json::str_attr(holder, placeholder::FILE) {
            return self
                .resolver
                .attachment(path)
                .ok_or_else(|| path.to_owned());
        }
        let id = json::str_attr(holder, placeholder::ATTACHMENT).unwrap_or_default();
        AttachmentId::from_str(id)
            .ok()
            .filter(|id| self.resolver.existing_attachment(*id))
            .ok_or_else(|| id.to_owned())
    }

    fn media(&mut self, node: &Value) -> Vec<Value> {
        let is_image = kind(node) == "image";
        match self.attachment(node) {
            Ok(attachment) => {
                let mut attrs = Map::new();
                attrs.insert("id".into(), Value::String(self.fresh_id()));
                attrs.insert("attachmentId".into(), Value::String(attachment.to_string()));
                if is_image {
                    let alt = json::str_attr(node, "alt").unwrap_or_default();
                    attrs.insert("alt".into(), Value::String(alt.to_owned()));
                    for key in ["caption", "align"] {
                        if let Some(text) = json::str_attr(node, key).filter(|t| !t.is_empty()) {
                            attrs.insert(key.into(), Value::String(text.to_owned()));
                        }
                    }
                    for key in ["width", "widthPercent"] {
                        if let Some(number) = json::attr(node, key).filter(|v| v.is_number()) {
                            attrs.insert(key.into(), number.clone());
                        }
                    }
                } else if json::str_attr(node, "view") == Some("preview") {
                    attrs.insert("view".into(), Value::String("preview".into()));
                }
                vec![json::element(kind(node), attrs, Vec::new())]
            }
            // An image the source keeps on a server, that nobody downloaded: a link to it.
            Err(_) if is_image && json::str_attr(node, placeholder::URL).is_some() => {
                let url = json::str_attr(node, placeholder::URL).unwrap_or_default();
                self.warn(ImportWarningCode::RemoteImage, url);
                let text = json::str_attr(node, "alt")
                    .filter(|t| !t.is_empty())
                    .or_else(|| json::str_attr(node, "caption"))
                    .filter(|t| !t.is_empty())
                    .unwrap_or(url);
                let marks = json::MarkSet {
                    link: is_http_url(url).then(|| url.to_owned()),
                    ..json::MarkSet::default()
                };
                vec![self.with_id(json::paragraph(vec![json::text_node(text, &marks)]))]
            }
            Err(what) => {
                self.warn(ImportWarningCode::MissingFile, &what);
                let name = paths::file_name(&what);
                let text = if is_image {
                    json::str_attr(node, "alt")
                        .filter(|t| !t.is_empty())
                        .or_else(|| json::str_attr(node, "caption"))
                        .unwrap_or(name)
                } else {
                    json::str_attr(node, placeholder::LABEL)
                        .filter(|t| !t.is_empty())
                        .unwrap_or(name)
                };
                self.text_in_place(text, true)
            }
        }
    }

    fn gallery(&mut self, node: &Value) -> Vec<Value> {
        let mut items = Vec::new();
        let list = json::attr(node, "items")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for item in list {
            // The placeholders sit in the item itself, as attributes of a node.
            let holder = serde_json::json!({"attrs": item});
            match self.attachment(&holder) {
                Ok(attachment) => {
                    let mut entry = Map::new();
                    entry.insert("attachmentId".into(), Value::String(attachment.to_string()));
                    if let Some(caption) = item
                        .get("caption")
                        .and_then(Value::as_str)
                        .filter(|c| !c.is_empty())
                    {
                        entry.insert("caption".into(), Value::String(caption.to_owned()));
                    }
                    items.push(Value::Object(entry));
                }
                Err(what) => self.warn(ImportWarningCode::MissingFile, &what),
            }
        }
        if items.is_empty() {
            return Vec::new();
        }
        let mut attrs = Map::new();
        attrs.insert("id".into(), Value::String(self.fresh_id()));
        attrs.insert("items".into(), Value::Array(items));
        if let Some(columns) = json::attr(node, "columns")
            .and_then(Value::as_i64)
            .filter(|c| (2..=4).contains(c))
        {
            attrs.insert("columns".into(), Value::from(columns));
        }
        vec![json::element("gallery", attrs, Vec::new())]
    }

    // ----- whiteboards -----------------------------------------------------------------------

    fn whiteboard(&mut self, node: &Value) -> Vec<Value> {
        let id = self.fresh_id();
        let mut attrs = Map::new();
        attrs.insert("id".into(), Value::String(id.clone()));
        if let Some(height) = json::attr(node, "height")
            .and_then(Value::as_i64)
            .filter(|h| (160..=1200).contains(h))
        {
            attrs.insert("height".into(), Value::from(height));
        }
        if let Some(Value::Object(elements)) = json::attr(node, placeholder::ELEMENTS) {
            let mut kept = Map::new();
            for (element_id, element) in elements {
                if element.get("id").and_then(Value::as_str) == Some(element_id.as_str()) {
                    kept.insert(element_id.clone(), element.clone());
                }
            }
            if !kept.is_empty() {
                self.whiteboards.insert(id, Value::Object(kept));
            }
        }
        vec![json::element("whiteboard", attrs, Vec::new())]
    }

    // ----- views -----------------------------------------------------------------------------

    fn property(&self, name: &str) -> Option<String> {
        self.resolver.property(name).map(|id| id.to_string())
    }

    fn option(&self, property: &str, name: &str) -> Option<String> {
        self.resolver
            .option(property, name)
            .map(|id| id.to_string())
    }

    /// What a sort key sorts by, as stored: `title`, `updated` and `created` stay, a property name is its id
    /// (`None` when it has none).
    fn sort_by(&self, by: &str) -> Option<String> {
        if matches!(by, "title" | "updated" | "created") {
            Some(by.to_owned())
        } else {
            self.property(by)
        }
    }

    /// A saved view: names back to ids, what has no id dropped. A block that holds several views
    /// (ADR-0054) gets each of them resolved like its own attributes, and its legacy attributes are the
    /// mirror of the first view, so that the block and its view agree whatever the annotation said.
    fn page_list(&mut self, node: &Value) -> Vec<Value> {
        let source = json::attr(node, placeholder::NAMES).and_then(Value::as_bool) == Some(true);
        let mut attrs = Map::new();
        attrs.insert("id".into(), Value::String(self.fresh_id()));
        if !source {
            for (key, value) in plain_attrs(node) {
                if key != "id" {
                    attrs.insert(key, value);
                }
            }
            return vec![json::element("pageList", attrs, Vec::new())];
        }
        let named = node
            .get("attrs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        attrs.extend(self.resolve_attrs(&named, true));
        if let Some(Value::Array(entries)) = named.get("views") {
            let mut views: Vec<Map<String, Value>> = Vec::new();
            for entry in entries.iter().filter_map(Value::as_object) {
                if views.len() >= MAX_VIEWS {
                    break;
                }
                let mut view = self.resolve_attrs(entry, false);
                // A usable id is kept when no earlier view has it; else the view gets a new one.
                let id = match view.get("id").and_then(Value::as_str) {
                    Some(id) if !view_id_taken(&views, id) => id.to_owned(),
                    _ => self.fresh_view_id(&views),
                };
                view.insert("id".into(), Value::String(id));
                views.push(complete_view(view));
            }
            if let Some(first) = views.first() {
                mirror_first_view(&mut attrs, first);
                attrs.insert(
                    "views".into(),
                    Value::Array(views.into_iter().map(Value::Object).collect()),
                );
            }
        }
        vec![json::element("pageList", attrs, Vec::new())]
    }

    /// A view id nobody in `views` has: 8 hexadecimal characters of a fresh id, or the whole id.
    fn fresh_view_id(&self, views: &[Map<String, Value>]) -> String {
        let fresh = self.fresh_id();
        let short: String = fresh.chars().take(8).collect();
        if view_id_taken(views, &short) {
            fresh
        } else {
            short
        }
    }

    /// The attributes of a block (`is_block`) or of one of its views, names back to ids; what has no id
    /// goes. A view has no `source` of its own, and the keys of its own (`id`, `name`, `openIn`,
    /// `cardSize`, the `filter` tree, `sorts`, `dateBy` and `endBy`) were checked by the annotation reader.
    fn resolve_attrs(&self, named: &Map<String, Value>, is_block: bool) -> Map<String, Value> {
        let mut attrs = Map::new();
        let named = |key: &str| named.get(key);
        if !is_block {
            for key in ["id", "name", "openIn", "cardSize"] {
                if let Some(value) = named(key) {
                    attrs.insert(key.into(), value.clone());
                }
            }
            if let Some(tree) = named("filter").and_then(|tree| self.filter_tree(tree)) {
                attrs.insert("filter".into(), tree);
            }
            if let Some(by) = named("dateBy").and_then(Value::as_str) {
                // The page's own times stay; a name with no property goes (the calendar asks to choose).
                if matches!(by, "created" | "updated") {
                    attrs.insert("dateBy".into(), Value::String(by.to_owned()));
                } else if let Some(id) = self.property(by) {
                    attrs.insert("dateBy".into(), Value::String(id));
                }
            }
            if let Some(id) = named("endBy")
                .and_then(Value::as_str)
                .and_then(|name| self.property(name))
            {
                attrs.insert("endBy".into(), Value::String(id));
            }
            if let Some(Value::Array(widgets)) = named("widgets") {
                let resolved: Vec<Value> = widgets
                    .iter()
                    .filter_map(Value::as_object)
                    .take(MAX_WIDGETS)
                    .map(|widget| self.widget(widget))
                    .collect();
                if !resolved.is_empty() {
                    attrs.insert("widgets".into(), Value::Array(resolved));
                }
            }
        }
        if let Some(layout) = named("layout").and_then(Value::as_str) {
            attrs.insert("layout".into(), Value::String(layout.to_owned()));
        }
        let mut properties: Vec<Value> = Vec::new();
        if let Some(Value::Array(list)) = named("properties") {
            for name in list.iter().filter_map(Value::as_str) {
                if let Some(id) = self.property(name).map(Value::String)
                    && !properties.contains(&id)
                {
                    properties.push(id);
                }
            }
        }
        attrs.insert("properties".into(), Value::Array(properties));
        if let Some(Value::Object(sort)) = named("sort") {
            let by = sort.get("by").and_then(Value::as_str).unwrap_or_default();
            let dir = sort.get("dir").and_then(Value::as_str).unwrap_or("asc");
            if let Some(by) = self.sort_by(by) {
                attrs.insert("sort".into(), serde_json::json!({"by": by, "dir": dir}));
            }
        }
        // The sort keys of a view (ADR-0058), resolved like `sort`: a key whose property has no id goes, a
        // repeated key is dropped, at most five are kept, and `sort` is the first key left (the mirror an
        // older build orders by).
        if !is_block && let Some(Value::Array(keys)) = named("sorts") {
            let mut sorts: Vec<Value> = Vec::new();
            for key in keys.iter().filter_map(Value::as_object) {
                let by = key.get("by").and_then(Value::as_str).unwrap_or_default();
                let dir = key.get("dir").and_then(Value::as_str).unwrap_or("asc");
                let Some(by) = self.sort_by(by) else {
                    continue;
                };
                if sorts.iter().any(|seen| seen["by"] == by.as_str()) {
                    continue;
                }
                sorts.push(serde_json::json!({"by": by, "dir": dir}));
                if sorts.len() >= kb_model::views::MAX_SORTS {
                    break;
                }
            }
            if let Some(first) = sorts.first().cloned() {
                attrs.insert("sort".into(), first);
                attrs.insert("sorts".into(), Value::Array(sorts));
            }
        }
        if is_block && let Some(Value::Object(source)) = named("source") {
            match source.get("kind").and_then(Value::as_str) {
                Some("tag") => {
                    let option = source
                        .get("option")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(id) = self.option(TAGS_NAME, option) {
                        attrs.insert(
                            "source".into(),
                            serde_json::json!({"kind": "tag", "optionId": id}),
                        );
                    }
                }
                _ => {
                    attrs.insert("source".into(), Value::Object(source.clone()));
                }
            }
        }
        if let Some(Value::Array(list)) = named("filters") {
            let filters: Vec<Value> = list.iter().filter_map(|f| self.filter(f)).collect();
            if !filters.is_empty() {
                attrs.insert("filters".into(), Value::Array(filters));
            }
        }
        let group_name = named("groupBy").and_then(Value::as_str);
        if let Some(group_name) = group_name
            && let Some(id) = self.property(group_name)
        {
            attrs.insert("groupBy".into(), Value::String(id));
            if let Some(Value::Array(list)) = named("hiddenGroups") {
                let hidden: Vec<Value> = list
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|name| {
                        if name == "none" {
                            Some("none".to_owned())
                        } else {
                            self.option(group_name, name)
                        }
                    })
                    .map(Value::String)
                    .collect();
                if !hidden.is_empty() {
                    attrs.insert("hiddenGroups".into(), Value::Array(hidden));
                }
            }
        }
        if let Some(Value::Object(widths)) = named("columnWidths") {
            let mut kept = Map::new();
            for (name, width) in widths {
                if name == "title" {
                    kept.insert(name.clone(), width.clone());
                } else if let Some(id) = self.property(name) {
                    kept.insert(id, width.clone());
                }
            }
            if !kept.is_empty() {
                attrs.insert("columnWidths".into(), Value::Object(kept));
            }
        }
        if let Some(Value::Object(calculations)) = named("calculations") {
            let mut kept = Map::new();
            for (name, function) in calculations {
                if name == "title" {
                    kept.insert(name.clone(), function.clone());
                } else if let Some(id) = self.property(name) {
                    kept.insert(id, function.clone());
                }
            }
            if !kept.is_empty() {
                attrs.insert("calculations".into(), Value::Object(kept));
            }
        }
        attrs
    }

    /// A widget of a dashboard view with its property names as ids (ADR-0055, `dashboards.md` section
    /// 5): its own filter tree (one that is left with nothing goes), the column of a number (`title`
    /// stays; a name with no property makes the number count all pages again), the grouping of a chart
    /// (`created` and `updated` stay; a name with no property is `null`, the chart asks to choose) and its
    /// measure (a name with no property: it counts). Everything else is as the annotation had it.
    fn widget(&self, widget: &Map<String, Value>) -> Value {
        let mut out = widget.clone();
        if let Some(tree) = widget.get("filter") {
            match self.filter_tree(tree) {
                Some(tree) => {
                    out.insert("filter".into(), tree);
                }
                None => {
                    out.remove("filter");
                }
            }
        }
        if let Some(Value::Object(calc)) = widget.get("calc")
            && let Some(by) = calc.get("by").and_then(Value::as_str)
            && by != "title"
        {
            let mut resolved = calc.clone();
            match self.property(by) {
                Some(id) => {
                    resolved.insert("by".into(), Value::String(id));
                }
                None => {
                    resolved.insert("by".into(), Value::String("title".into()));
                    resolved.insert("fn".into(), Value::String("count_all".into()));
                }
            }
            out.insert("calc".into(), Value::Object(resolved));
        }
        if let Some(Value::Object(chart)) = widget.get("chart") {
            let mut resolved = chart.clone();
            if let Some(group) = chart.get("groupBy").and_then(Value::as_str)
                && !matches!(group, "created" | "updated")
            {
                resolved.insert(
                    "groupBy".into(),
                    self.property(group).map_or(Value::Null, Value::String),
                );
            }
            if let Some(Value::Object(measure)) = chart.get("measure")
                && let Some(by) = measure.get("propertyId").and_then(Value::as_str)
            {
                match self.property(by) {
                    Some(id) => {
                        let mut kept = measure.clone();
                        kept.insert("propertyId".into(), Value::String(id));
                        resolved.insert("measure".into(), Value::Object(kept));
                    }
                    None => {
                        resolved.insert("measure".into(), serde_json::json!({ "fn": "count" }));
                    }
                }
            }
            out.insert("chart".into(), Value::Object(resolved));
        }
        Value::Object(out)
    }

    /// A filter of a view with its property and options as ids; `None` when one has none.
    fn filter(&self, filter: &Value) -> Option<Value> {
        let property = filter.get("property")?.as_str()?;
        let op = filter.get("op")?.as_str()?;
        let id = self.property(property)?;
        let mut out = Map::new();
        out.insert("propertyId".into(), Value::String(id));
        out.insert("op".into(), Value::String(op.to_owned()));
        if let Some(value) = filter.get("value") {
            let value = match op {
                "hasOption" | "lacksOption" => match value {
                    Value::String(name) => Value::String(self.option(property, name)?),
                    Value::Array(names) => {
                        let ids: Vec<Value> = names
                            .iter()
                            .filter_map(Value::as_str)
                            .filter_map(|name| self.option(property, name))
                            .map(Value::String)
                            .collect();
                        if ids.is_empty() {
                            return None;
                        }
                        Value::Array(ids)
                    }
                    _ => return None,
                },
                "relatesTo" | "notRelatesTo" => {
                    let page = PageId::from_str(value.as_str()?).ok()?;
                    if !self.resolver.existing_page(page) {
                        return None;
                    }
                    Value::String(page.to_string())
                }
                _ => value.clone(),
            };
            out.insert("value".into(), value);
        }
        Some(Value::Object(out))
    }

    /// A filter tree of a view with ids: a filter that has none goes, a nested group that is left with no
    /// filter after it had some goes with them, a tree left with nothing is no tree.
    fn filter_tree(&self, group: &Value) -> Option<Value> {
        let op = group.get("op")?.as_str()?;
        let items = group.get("items")?.as_array()?;
        let kept: Vec<Value> = items
            .iter()
            .filter_map(|item| {
                if item.get("items").is_some() {
                    self.filter_tree(item)
                } else {
                    self.filter(item)
                }
            })
            .collect();
        if kept.is_empty() && !items.is_empty() {
            return None;
        }
        Some(serde_json::json!({ "op": op, "items": kept }))
    }
}

/// The most views a block holds.
const MAX_VIEWS: usize = kb_model::views::MAX_VIEWS;

/// The most widgets a dashboard view holds.
const MAX_WIDGETS: usize = 12;

/// Whether an earlier view of the block has this id.
fn view_id_taken(views: &[Map<String, Value>], id: &str) -> bool {
    views
        .iter()
        .any(|view| view.get("id").and_then(Value::as_str) == Some(id))
}

/// A resolved view as the editor writes one: `id`, `name`, `layout`, `properties` and `sort` are always
/// there (a missing layout is the list, a missing sort title ascending), the rest only when it was set.
fn complete_view(mut view: Map<String, Value>) -> Map<String, Value> {
    view.entry("name")
        .or_insert_with(|| Value::String(String::new()));
    view.entry("layout")
        .or_insert_with(|| Value::String("list".into()));
    view.entry("properties")
        .or_insert_with(|| Value::Array(Vec::new()));
    view.entry("sort")
        .or_insert_with(|| serde_json::json!({ "by": "title", "dir": "asc" }));
    view
}

/// Writes the first view into the block's legacy attributes (ADR-0054, the mirror an older build reads):
/// `layout`, `properties` and `sort` as they are (`sort` is the first of the view's `sorts` when it has
/// some, ADR-0058); `filters` the plain filters of the view's top-level group
/// when it is AND (nothing when it is OR: an OR cannot be a flat list); `columnWidths`, `groupBy`,
/// `hiddenGroups` and `calculations` as they are, removed when the view has none.
fn mirror_first_view(attrs: &mut Map<String, Value>, first: &Map<String, Value>) {
    for key in ["layout", "properties", "sort"] {
        if let Some(value) = first.get(key) {
            attrs.insert(key.into(), value.clone());
        }
    }
    if let Some(key) = first
        .get("sorts")
        .and_then(Value::as_array)
        .and_then(|keys| keys.first())
    {
        attrs.insert("sort".into(), key.clone());
    }
    let plain: Vec<Value> = match first.get("filter") {
        Some(tree) if tree.get("op").and_then(Value::as_str) == Some("and") => tree
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item.get("items").is_none())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if plain.is_empty() {
        attrs.remove("filters");
    } else {
        attrs.insert("filters".into(), Value::Array(plain));
    }
    for key in ["columnWidths", "groupBy", "hiddenGroups", "calculations"] {
        match first.get(key) {
            Some(value) => {
                attrs.insert(key.into(), value.clone());
            }
            None => {
                attrs.remove(key);
            }
        }
    }
}

/// Whether a URL can be the target of a `link` mark: `http` or `https`, short.
fn is_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && url.len() <= 2_048
        && !url.chars().any(char::is_whitespace)
}

/// The attributes of a node without the placeholders.
fn plain_attrs(node: &Value) -> Map<String, Value> {
    node.get("attrs")
        .and_then(Value::as_object)
        .map(|attrs| {
            attrs
                .iter()
                .filter(|(key, _)| !key.starts_with('$'))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}
