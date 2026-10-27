//! An AppFlowy document (`collab_jsons/documents/<id>.json`: a block map, a children map and a
//! text map) into the blocks of the page shape (ADR-0016), with the crate's placeholders for
//! pages and files. The block table of `docs/architecture/appflowy-import.md`, section 3.
//!
//! Hostile documents are data: every block is read at most once (a cycle or a block listed twice
//! is cut), the JSON nesting of the result stays bounded (containers that would go deeper are
//! flattened into their parent) and so does the recursion.

use std::collections::{BTreeMap, HashSet};

use kb_types::ImportWarningCode;
use serde_json::{Map, Value};

use super::archive::{array_of, flag, int, object_of, text_of};
use super::colors;
use super::delta;
use super::env::{Env, clean_file_name, is_http, is_https, is_linkable, outside_path};
use crate::json::{self, MarkSet, attrs, element, plain};
use crate::paths::truncate_chars;
use crate::placeholder::{self, Target};

/// How much JSON nesting the result may use (the model's limit is 128).
const MAX_JSON_DEPTH: usize = 60;
/// How deep blocks are followed.
const MAX_RECURSION: usize = 64;
/// Most rows and columns kept of one table.
const MAX_ROWS: usize = crate::html::MAX_TABLE_ROWS;
const MAX_COLS: usize = crate::html::MAX_TABLE_COLS;
/// The longest detail of a warning about a block type.
const MAX_TYPE_CHARS: usize = 60;

/// A document as read from its JSON.
pub(super) struct DocData {
    blocks: Map<String, Value>,
    children: Map<String, Value>,
    text: Map<String, Value>,
    root: String,
}

impl DocData {
    /// Reads a document; `None` when the text is not JSON of the expected shape or has no root.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let Value::Object(mut top) = serde_json::from_str::<Value>(text).ok()? else {
            return None;
        };
        let Value::Object(blocks) = top.remove("blocks")? else {
            return None;
        };
        let (children, text_map) = match top.remove("meta") {
            Some(Value::Object(mut meta)) => (
                take_object(&mut meta, "children_map"),
                take_object(&mut meta, "text_map"),
            ),
            _ => (Map::new(), Map::new()),
        };
        let root = text_of(top.get("page_id"))
            .filter(|id| blocks.get(id).is_some_and(|b| ty(b) == "page"))
            .or_else(|| {
                // The page block whose parent is nothing (or not a block).
                blocks
                    .iter()
                    .find(|(_, b)| {
                        ty(b) == "page"
                            && text_of(b.get("parent"))
                                .is_none_or(|p| p.is_empty() || !blocks.contains_key(&p))
                    })
                    .map(|(id, _)| id.clone())
            })?;
        Some(Self {
            blocks,
            children,
            text: text_map,
            root,
        })
    }

    /// The ids of a block's children, in order.
    fn child_ids(&self, block: &Value) -> Vec<String> {
        let list = match block.get("children") {
            Some(Value::String(key)) => self.children.get(key).cloned(),
            Some(list @ Value::Array(_)) => Some(list.clone()),
            _ => None,
        };
        match list {
            Some(Value::Array(ids)) => ids
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The operations of a block's delta: the text map entry, else the inline `data.delta`.
    fn delta(&self, block: &Value, data: &Map<String, Value>) -> Vec<Value> {
        if text_of(block.get("external_type")).as_deref() == Some("text")
            && let Some(id) = text_of(block.get("external_id"))
            && let Some(ops) = self.text.get(&id).and_then(delta::ops)
        {
            return ops;
        }
        data.get("delta").and_then(delta::ops).unwrap_or_default()
    }
}

fn take_object(map: &mut Map<String, Value>, key: &str) -> Map<String, Value> {
    match map.remove(key) {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    }
}

fn ty(block: &Value) -> &str {
    block.get("ty").and_then(Value::as_str).unwrap_or_default()
}

fn data_of(block: &Value) -> Map<String, Value> {
    block.get("data").and_then(object_of).unwrap_or_default()
}

/// The blocks of a document's page.
pub(super) fn convert(doc: &DocData, env: &mut Env, path: &str) -> Vec<Value> {
    let mut conv = Conv {
        doc,
        env,
        path,
        seen: HashSet::new(),
    };
    let Some(root) = doc.blocks.get(&doc.root) else {
        return Vec::new();
    };
    conv.seen.insert(doc.root.clone());
    conv.kids(root, 1, 0)
}

struct Conv<'a> {
    doc: &'a DocData,
    env: &'a mut Env,
    path: &'a str,
    /// The blocks already read.
    seen: HashSet<String>,
}

/// Whether `levels` of nesting (the node itself down to its text) fit below `depth`.
fn fits(depth: usize, levels: usize) -> bool {
    depth + levels <= MAX_JSON_DEPTH
}

fn ensure_blocks(mut blocks: Vec<Value>) -> Vec<Value> {
    if blocks.is_empty() {
        blocks.push(json::paragraph(Vec::new()));
    }
    blocks
}

impl Conv<'_> {
    fn warn(&mut self, code: ImportWarningCode, detail: &str) {
        self.env.warn(self.path, code, Some(detail));
    }

    /// Claims a block: `false` when it was read already (a cycle, a block listed twice).
    fn claim(&mut self, id: &str) -> bool {
        self.seen.insert(id.to_owned())
    }

    fn inline(&mut self, block: &Value, data: &Map<String, Value>) -> Vec<Value> {
        let ops = self.doc.delta(block, data);
        delta::nodes(self.env, self.path, &ops)
    }

    /// The plain text of a block and everything under it, for the places that take only text.
    fn plain_of(&mut self, id: &str, depth: usize) -> String {
        let doc = self.doc;
        let Some(block) = doc.blocks.get(id) else {
            return String::new();
        };
        if depth > MAX_RECURSION || !self.claim(id) {
            return String::new();
        }
        let data = data_of(block);
        let inline = self.inline(block, &data);
        let mut text = json::plain_text(&json::paragraph(inline));
        for child in doc.child_ids(block) {
            let more = self.plain_of(&child, depth + 1);
            if !more.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&more);
            }
        }
        text
    }

    /// The children of a block as blocks, at JSON depth `jd`.
    fn kids(&mut self, block: &Value, jd: usize, rd: usize) -> Vec<Value> {
        let ids = self.doc.child_ids(block);
        self.run(&ids, jd, rd)
    }

    /// Blocks in order; consecutive items of one list kind form one list.
    fn run(&mut self, ids: &[String], jd: usize, rd: usize) -> Vec<Value> {
        let doc = self.doc;
        let mut out = Vec::new();
        let mut at = 0;
        while at < ids.len() {
            let Some(block) = doc.blocks.get(&ids[at]) else {
                at += 1;
                continue;
            };
            let kind = ty(block);
            if matches!(kind, "bulleted_list" | "numbered_list" | "todo_list") {
                let mut items: Vec<&str> = Vec::new();
                while at < ids.len() && doc.blocks.get(&ids[at]).is_some_and(|b| ty(b) == kind) {
                    items.push(&ids[at]);
                    at += 1;
                }
                out.extend(self.list(kind, &items, jd, rd));
            } else {
                out.extend(self.one(&ids[at], jd, rd));
                at += 1;
            }
        }
        out
    }

    /// The text of a block as a paragraph (when it has any) and its children after it, at the
    /// same depth: what a block becomes when its structure cannot be kept.
    fn flat(
        &mut self,
        block: &Value,
        data: &Map<String, Value>,
        jd: usize,
        rd: usize,
    ) -> Vec<Value> {
        let inline = self.inline(block, data);
        let mut out = Vec::new();
        if !inline.is_empty() {
            out.push(json::paragraph(inline));
        }
        out.extend(self.kids(block, jd, rd + 1));
        out
    }

    /// A block with its text in a paragraph and its children after the paragraph, in a container
    /// that holds blocks.
    fn container(
        &mut self,
        block: &Value,
        data: &Map<String, Value>,
        jd: usize,
        rd: usize,
        make: impl FnOnce(Vec<Value>) -> Value,
    ) -> Vec<Value> {
        if !fits(jd, 3) {
            return self.flat(block, data, jd, rd);
        }
        let inline = self.inline(block, data);
        let mut blocks = vec![json::paragraph(inline)];
        blocks.extend(self.kids(block, jd + 1, rd + 1));
        vec![make(blocks)]
    }

    fn one(&mut self, id: &str, jd: usize, rd: usize) -> Vec<Value> {
        let doc = self.doc;
        let Some(block) = doc.blocks.get(id) else {
            return Vec::new();
        };
        if !self.claim(id) {
            return Vec::new();
        }
        if rd > MAX_RECURSION {
            self.warn(ImportWarningCode::UnsupportedContent, "nesting");
            return Vec::new();
        }
        let data = data_of(block);
        let kind = ty(block);
        match kind {
            "paragraph" | "text" => {
                let mut out = vec![json::paragraph(self.inline(block, &data))];
                out.extend(self.kids(block, jd, rd + 1));
                out
            }
            "heading" => {
                let level = int(data.get("level")).unwrap_or(1).clamp(1, 3);
                let level = u8::try_from(level).unwrap_or(1);
                let mut out = vec![json::heading(level, self.inline(block, &data))];
                out.extend(self.kids(block, jd, rd + 1));
                out
            }
            "quote" => self.container(block, &data, jd, rd, |blocks| plain("blockquote", blocks)),
            "callout" => {
                let icon = data
                    .get("icon")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|icon| !icon.is_empty() && icon.len() <= 32)
                    .unwrap_or("💡")
                    .to_owned();
                let color = data
                    .get("bgColor")
                    .or_else(|| data.get("bg_color"))
                    .and_then(Value::as_str)
                    .and_then(|c| colors::palette(c, true));
                self.container(block, &data, jd, rd, |blocks| {
                    let mut map = attrs([("icon", Value::String(icon))]);
                    if let Some(color) = color {
                        map.insert("color".into(), Value::String(color.to_owned()));
                    }
                    element("callout", map, blocks)
                })
            }
            "toggle_list" => self.toggle(block, &data, jd, rd, None),
            "toggle_heading" => {
                // The level of a heading toggle, clamped as for headings (ADR-0040).
                let level = int(data.get("level")).unwrap_or(1).clamp(1, 3);
                let level = u8::try_from(level).unwrap_or(1);
                self.toggle(block, &data, jd, rd, Some(level))
            }
            "code" => {
                let text = delta::plain_text(&self.doc.delta(block, &data));
                let language = text_of(data.get("language"))
                    .map(|l| l.trim().to_lowercase())
                    .filter(|l| !l.is_empty());
                let mut map = Map::new();
                if let Some(language) = language {
                    map.insert(
                        "language".into(),
                        Value::String(truncate_chars(&language, 40)),
                    );
                }
                let content = if text.is_empty() {
                    Vec::new()
                } else {
                    vec![json::text_node(&text, &MarkSet::default())]
                };
                let mut out = vec![element("codeBlock", map, content)];
                out.extend(self.kids(block, jd, rd + 1));
                out
            }
            "divider" => {
                let mut out = vec![plain("horizontalRule", Vec::new())];
                out.extend(self.kids(block, jd, rd + 1));
                out
            }
            "math_equation" => {
                let latex = text_of(data.get("formula")).unwrap_or_default();
                let content = if latex.is_empty() {
                    Vec::new()
                } else {
                    vec![json::text_node(&latex, &MarkSet::default())]
                };
                vec![plain("mathBlock", content)]
            }
            "outline" => vec![plain("tableOfContents", Vec::new())],
            "simple_table" => self.simple_table(block, &data, jd, rd),
            "table" => self.legacy_table(block, jd, rd),
            "simple_columns" => self.columns(block, &data, jd, rd),
            "image" => self.image(&data),
            "file" => self.file(&data),
            "link_preview" | "embed_link" => {
                let url = text_of(data.get("url")).unwrap_or_default();
                link_paragraph(&url, &url).into_iter().collect()
            }
            "sub_page" | "grid" | "board" | "calendar" => self.view_link(&data),
            "ai_meeting" => self.meeting(block, jd, rd),
            // Containers whose children stand in the place of the container.
            "page"
            | "simple_table_row"
            | "simple_table_cell"
            | "simple_column"
            | "table/cell"
            | "ai_meeting_summary"
            | "ai_meeting_notes"
            | "ai_meeting_transcription"
            | "ai_meeting_speaker" => self.kids(block, jd, rd + 1),
            other => {
                let detail = truncate_chars(other, MAX_TYPE_CHARS);
                self.warn(ImportWarningCode::UnsupportedContent, &detail);
                self.flat(block, &data, jd, rd)
            }
        }
    }

    // ----- lists -----------------------------------------------------------------------------

    fn list(&mut self, kind: &str, items: &[&str], jd: usize, rd: usize) -> Vec<Value> {
        let doc = self.doc;
        let mut nodes: Vec<Value> = Vec::new();
        let mut start: Option<i64> = None;
        let mut first = true;
        let nested = fits(jd, 4);
        let mut flat: Vec<Value> = Vec::new();
        for id in items {
            let Some(block) = doc.blocks.get(*id) else {
                continue;
            };
            if !self.claim(id) {
                continue;
            }
            if rd > MAX_RECURSION {
                self.warn(ImportWarningCode::UnsupportedContent, "nesting");
                continue;
            }
            let data = data_of(block);
            if first {
                start = int(data.get("number")).filter(|n| *n >= 1);
                first = false;
            }
            if !nested {
                flat.extend(self.flat(block, &data, jd, rd));
                continue;
            }
            let inline = self.inline(block, &data);
            let mut blocks = vec![json::paragraph(inline)];
            blocks.extend(self.kids(block, jd + 2, rd + 1));
            nodes.push(match kind {
                "todo_list" => element(
                    "taskItem",
                    attrs([("checked", Value::Bool(flag(data.get("checked"))))]),
                    blocks,
                ),
                _ => plain("listItem", blocks),
            });
        }
        if !nested {
            return flat;
        }
        if nodes.is_empty() {
            return Vec::new();
        }
        vec![match kind {
            "todo_list" => plain("taskList", nodes),
            "numbered_list" => {
                let mut map = Map::new();
                if let Some(start) = start.filter(|s| *s != 1) {
                    map.insert("start".into(), Value::from(start));
                }
                element("orderedList", map, nodes)
            }
            _ => plain("bulletList", nodes),
        }]
    }

    fn toggle(
        &mut self,
        block: &Value,
        data: &Map<String, Value>,
        jd: usize,
        rd: usize,
        level: Option<u8>,
    ) -> Vec<Value> {
        if !fits(jd, 4) {
            return self.flat(block, data, jd, rd);
        }
        let inline = self.inline(block, data);
        let summary = json::plain_text(&json::paragraph(inline));
        let content = ensure_blocks(self.kids(block, jd + 2, rd + 1));
        vec![details(
            !flag(data.get("collapsed")),
            &summary,
            level,
            content,
        )]
    }

    // ----- tables and columns ----------------------------------------------------------------

    fn simple_table(
        &mut self,
        block: &Value,
        data: &Map<String, Value>,
        jd: usize,
        rd: usize,
    ) -> Vec<Value> {
        let doc = self.doc;
        let nested = fits(jd, 5);
        let header = flag(data.get("enable_header_row"));
        let mut rows: Vec<Vec<Vec<Value>>> = Vec::new();
        let mut flat: Vec<Value> = Vec::new();
        let mut cut = false;
        for row_id in doc.child_ids(block) {
            let Some(row) = doc.blocks.get(&row_id) else {
                continue;
            };
            if !self.claim(&row_id) {
                continue;
            }
            let mut cells: Vec<Vec<Value>> = Vec::new();
            for cell_id in doc.child_ids(row) {
                let Some(cell) = doc.blocks.get(&cell_id) else {
                    continue;
                };
                if !self.claim(&cell_id) {
                    continue;
                }
                if cells.len() >= MAX_COLS {
                    cut = true;
                    continue;
                }
                if nested {
                    cells.push(ensure_blocks(self.kids(cell, jd + 3, rd + 1)));
                } else {
                    flat.extend(self.kids(cell, jd, rd + 1));
                }
            }
            if rows.len() >= MAX_ROWS {
                cut = true;
            } else if nested {
                rows.push(cells);
            }
        }
        if cut {
            self.warn(ImportWarningCode::UnsupportedContent, "table rows");
        }
        if !nested {
            return flat;
        }
        table_node(rows, header)
    }

    /// The older table: `table` with `table/cell` children placed by `rowPosition` and
    /// `colPosition`.
    fn legacy_table(&mut self, block: &Value, jd: usize, rd: usize) -> Vec<Value> {
        let doc = self.doc;
        let nested = fits(jd, 5);
        let mut grid: BTreeMap<usize, BTreeMap<usize, Vec<Value>>> = BTreeMap::new();
        let mut flat: Vec<Value> = Vec::new();
        let mut cut = false;
        for cell_id in doc.child_ids(block) {
            let Some(cell) = doc.blocks.get(&cell_id) else {
                continue;
            };
            if !self.claim(&cell_id) {
                continue;
            }
            let data = data_of(cell);
            let row = int(data.get("rowPosition")).and_then(|n| usize::try_from(n).ok());
            let col = int(data.get("colPosition")).and_then(|n| usize::try_from(n).ok());
            let (Some(row), Some(col)) = (row, col) else {
                continue;
            };
            if row >= MAX_ROWS || col >= MAX_COLS {
                cut = true;
                continue;
            }
            if nested {
                let blocks = ensure_blocks(self.kids(cell, jd + 3, rd + 1));
                grid.entry(row).or_default().insert(col, blocks);
            } else {
                flat.extend(self.kids(cell, jd, rd + 1));
            }
        }
        if cut {
            self.warn(ImportWarningCode::UnsupportedContent, "table rows");
        }
        if !nested {
            return flat;
        }
        let width = grid
            .values()
            .filter_map(|row| row.keys().next_back())
            .max()
            .map_or(0, |last| last + 1);
        let height = grid.keys().next_back().map_or(0, |last| last + 1);
        let rows: Vec<Vec<Vec<Value>>> = (0..height)
            .map(|row| {
                let mut cells = grid.remove(&row).unwrap_or_default();
                (0..width)
                    .map(|col| cells.remove(&col).unwrap_or_default())
                    .collect()
            })
            .collect();
        table_node(rows, false)
    }

    fn columns(
        &mut self,
        block: &Value,
        data: &Map<String, Value>,
        jd: usize,
        rd: usize,
    ) -> Vec<Value> {
        let doc = self.doc;
        let mut columns: Vec<&Value> = Vec::new();
        for id in doc.child_ids(block) {
            if let Some(column) = doc.blocks.get(&id)
                && ty(column) == "simple_column"
                && self.claim(&id)
            {
                columns.push(column);
            }
        }
        if columns.is_empty() {
            return self.flat(block, data, jd, rd);
        }
        if columns.len() == 1 || !fits(jd, 4) {
            let mut out = Vec::new();
            for column in columns {
                out.extend(self.kids(column, jd, rd + 1));
            }
            return out;
        }
        let mut contents: Vec<Vec<Value>> = columns
            .into_iter()
            .map(|column| ensure_blocks(self.kids(column, jd + 2, rd + 1)))
            .collect();
        while contents.len() > 5 {
            if let Some(extra) = contents.pop()
                && let Some(last) = contents.last_mut()
            {
                last.extend(extra);
            }
        }
        vec![plain(
            "columnList",
            contents
                .into_iter()
                .map(|blocks| plain("column", blocks))
                .collect(),
        )]
    }

    // ----- media and links -------------------------------------------------------------------

    fn image(&mut self, data: &Map<String, Value>) -> Vec<Value> {
        let url = text_of(data.get("url")).unwrap_or_default();
        let url = url.trim();
        let align = data
            .get("align")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
            .filter(|a| matches!(a.as_str(), "left" | "center" | "right"));
        let with_align = |mut node: Value| {
            if let Some(align) = &align {
                json::set_attr(&mut node, "align", Value::String(align.clone()));
            }
            node
        };
        if url.is_empty() {
            self.warn(ImportWarningCode::MissingFile, "image");
            return Vec::new();
        }
        if is_https(url) {
            if let Some(key) = self.env.remote_key(url) {
                return vec![with_align(placeholder::image_remote(&key, url))];
            }
            self.warn(ImportWarningCode::RemoteImage, &truncate_chars(url, 100));
            return link_paragraph(url, url).into_iter().collect();
        }
        if is_http(url) {
            self.warn(ImportWarningCode::RemoteImage, &truncate_chars(url, 100));
            return link_paragraph(url, url).into_iter().collect();
        }
        if url
            .get(..5)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
        {
            self.warn(ImportWarningCode::EmbeddedImage, "");
            return Vec::new();
        }
        // A path on a network share or on the machine that made the backup is never followed.
        if let Some(outside) = outside_path(url) {
            self.warn(ImportWarningCode::MissingFile, outside);
            return text_paragraph(&clean_file_name(url).unwrap_or_default())
                .into_iter()
                .collect();
        }
        if let Some(path) = self.env.local_file(url, None) {
            return vec![with_align(placeholder::image_file(&path, "", ""))];
        }
        let name = clean_file_name(url).unwrap_or_default();
        self.warn(ImportWarningCode::MissingFile, &name);
        text_paragraph(&name).into_iter().collect()
    }

    fn file(&mut self, data: &Map<String, Value>) -> Vec<Value> {
        let url = text_of(data.get("url")).unwrap_or_default();
        let url = url.trim();
        let name = text_of(data.get("name")).unwrap_or_default();
        let name = name.trim();
        if is_linkable(url) {
            return link_paragraph(if name.is_empty() { url } else { name }, url)
                .into_iter()
                .collect();
        }
        let label = if name.is_empty() {
            clean_file_name(url).unwrap_or_default()
        } else {
            name.to_owned()
        };
        if let Some(outside) = outside_path(url) {
            self.warn(ImportWarningCode::MissingFile, outside);
            return text_paragraph(&label).into_iter().collect();
        }
        if let Some(path) = self.env.local_file(url, Some(name)) {
            return vec![placeholder::file_path(&path, name)];
        }
        self.warn(ImportWarningCode::MissingFile, &truncate_chars(&label, 100));
        text_paragraph(&label).into_iter().collect()
    }

    /// A `sub_page` or an inline database view: a card for the page of that view.
    ///
    /// The page is found through the view (`view_id`, then the views of `view_ids`), then the
    /// database (`database_id`), then `parent_id` when that is a database or one of its views
    /// (otherwise it is the page that holds the block).
    fn view_link(&mut self, data: &Map<String, Value>) -> Vec<Value> {
        let id = text_of(data.get("view_id")).unwrap_or_default();
        let mut found = self.env.page(&id);
        if found.is_none()
            && let Some(views) = data.get("view_ids").and_then(array_of)
        {
            found = views
                .iter()
                .filter_map(|v| text_of(Some(v)))
                .find_map(|view| self.env.page(&view));
        }
        if found.is_none() {
            found = text_of(data.get("database_id")).and_then(|d| self.env.database_page(&d));
        }
        if found.is_none() {
            found = text_of(data.get("parent_id")).and_then(|p| self.env.database_page(&p));
        }
        match found {
            Some(index) => vec![placeholder::page_card(
                &Target::Planned(index),
                self.env.title(index),
            )],
            None => {
                self.warn(ImportWarningCode::UnresolvedLink, &truncate_chars(&id, 100));
                vec![json::text_paragraph("Unknown page")]
            }
        }
    }

    // ----- AI meetings -----------------------------------------------------------------------

    fn meeting(&mut self, block: &Value, jd: usize, rd: usize) -> Vec<Value> {
        let doc = self.doc;
        let mut out: Vec<Value> = Vec::new();
        // The names of the speakers: `speaker_info_map` is `{speaker id: {name}}`, as an object or
        // as a string that holds one.
        let names = data_of(block)
            .get("speaker_info_map")
            .and_then(object_of)
            .unwrap_or_default();
        for id in doc.child_ids(block) {
            let Some(child) = doc.blocks.get(&id) else {
                continue;
            };
            let kind = ty(child);
            match kind {
                "ai_meeting_summary" | "ai_meeting_notes" if self.claim(&id) => {
                    let blocks = self.kids(child, jd, rd + 1);
                    if !blocks.is_empty() {
                        let title = if kind == "ai_meeting_summary" {
                            "Summary"
                        } else {
                            "Notes"
                        };
                        out.push(json::heading(
                            2,
                            vec![json::text_node(title, &MarkSet::default())],
                        ));
                        out.extend(blocks);
                    }
                }
                "ai_meeting_transcription" if self.claim(&id) => {
                    let mut turns: Vec<Value> = Vec::new();
                    for speaker_id in doc.child_ids(child) {
                        let Some(speaker) = doc.blocks.get(&speaker_id) else {
                            continue;
                        };
                        if !self.claim(&speaker_id) {
                            continue;
                        }
                        let data = data_of(speaker);
                        let id = text_of(data.get("speaker_id")).unwrap_or_else(|| "?".to_owned());
                        let who = names
                            .get(&id)
                            .and_then(|info| info.get("name"))
                            .and_then(Value::as_str)
                            .map(|name| {
                                truncate_chars(
                                    &name.split_whitespace().collect::<Vec<_>>().join(" "),
                                    80,
                                )
                            })
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| format!("Speaker {id}"));
                        let when = int(data.get("timestamp"))
                            .filter(|t| *t >= 0)
                            .map_or_else(String::new, |t| {
                                format!("[{:02}:{:02}] ", t / 60, t % 60)
                            });
                        let mut words: Vec<String> = Vec::new();
                        for part in doc.child_ids(speaker) {
                            let text = self.plain_of(&part, rd + 3);
                            if !text.is_empty() {
                                words.push(text);
                            }
                        }
                        turns.push(json::text_paragraph(&format!(
                            "{when}{who}: {}",
                            words.join(" ")
                        )));
                    }
                    if !turns.is_empty() {
                        if fits(jd, 4) {
                            out.push(details(false, "Transcript", None, turns));
                        } else {
                            out.extend(turns);
                        }
                    }
                }
                _ => out.extend(self.one(&id, jd, rd + 1)),
            }
        }
        out
    }
}

/// A paragraph holding `text` linked to `url` (plain text when the URL cannot be a link); `None`
/// when there is nothing to show.
fn link_paragraph(text: &str, url: &str) -> Option<Value> {
    let text = text.trim();
    let url = url.trim();
    let shown = if text.is_empty() { url } else { text };
    if shown.is_empty() {
        return None;
    }
    let marks = MarkSet {
        link: is_linkable(url).then(|| url.to_owned()),
        ..MarkSet::default()
    };
    Some(json::paragraph(vec![json::text_node(shown, &marks)]))
}

fn text_paragraph(text: &str) -> Option<Value> {
    let text = text.trim();
    (!text.is_empty()).then(|| json::text_paragraph(text))
}

/// A toggle: a summary line (with a `level` for a toggle heading) and the blocks it shows.
fn details(open: bool, summary: &str, level: Option<u8>, blocks: Vec<Value>) -> Value {
    let summary_node = json::summary(summary, level);
    let mut map = Map::new();
    if open {
        map.insert("open".into(), Value::Bool(true));
    }
    element(
        "details",
        map,
        vec![summary_node, plain("detailsContent", ensure_blocks(blocks))],
    )
}

/// A table of rows of cells of blocks; the first row is the header when `header` is set. Rows are
/// padded to the widest one. Nothing without cells.
fn table_node(rows: Vec<Vec<Vec<Value>>>, header: bool) -> Vec<Value> {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return Vec::new();
    }
    let rows: Vec<Value> = rows
        .into_iter()
        .enumerate()
        .map(|(row, mut cells)| {
            cells.resize_with(width, Vec::new);
            let name = if header && row == 0 {
                "tableHeader"
            } else {
                "tableCell"
            };
            plain(
                "tableRow",
                cells
                    .into_iter()
                    .map(|blocks| {
                        element(
                            name,
                            attrs([("colspan", Value::from(1)), ("rowspan", Value::from(1))]),
                            ensure_blocks(blocks),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    vec![plain("table", rows)]
}
