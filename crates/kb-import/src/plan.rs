//! `plan` (ADR-0036, section 3.3): source files into a tree of pages with placeholder content, the
//! properties they need and the files they use.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use kb_types::{ImportWarningCode, PropertyKind, TemplatePlaceholder};
use serde_json::Value;

use crate::ctx::{PAGE_EXTENSIONS, PageCtx, SourceIndex, Warnings};
use crate::frontmatter;
use crate::json::{self, kind, plain_text};
use crate::markdown;
use crate::model::{Node, NodeKind, PendingTarget, RawValue};
use crate::notion;
use crate::paths::{self, natural_cmp, truncate_chars};
use crate::placeholder::{self, Target};
use crate::props::Aggregator;
use crate::sidecar::{self, Sidecar};
use crate::table::{self, Table};
use crate::text;
use crate::values;
use crate::yaml;
use crate::{
    ImportOptions, ImportValue, PageTarget, Plan, PlannedCover, PlannedPage, PlannedProperty,
    SourceFile,
};

/// The longest page title, in characters (the core's limit).
const MAX_TITLE_CHARS: usize = 500;
/// File name of the sidecar of an atomik-kb export.
const SIDECAR_NAME: &str = "atomik-export.json";

/// A text file of the source.
struct TextFile {
    orig: String,
    norm: String,
    ext: String,
    text: String,
}

/// What a file's content gave.
struct Converted {
    title: Option<String>,
    icon: Option<String>,
    cover: Option<PlannedCover>,
    cover_y: Option<u8>,
    /// The front matter says `template: true` (ADR-0056).
    template: bool,
    /// The placeholders the front matter lists, by property name.
    template_placeholders: Vec<(String, TemplatePlaceholder)>,
    blocks: Vec<Value>,
    props: Vec<(String, RawValue)>,
    /// Property lines were removed from the top of the page.
    stripped: bool,
}

pub(crate) fn run(
    files: Vec<SourceFile>,
    binary_paths: &[String],
    options: &ImportOptions,
) -> Plan {
    let mut planner = Planner {
        warnings: Warnings::default(),
        texts: Vec::new(),
        nodes: Vec::new(),
        node_text: Vec::new(),
        index: SourceIndex::default(),
        tables: HashMap::new(),
        sidecar: None,
        hints: HashMap::new(),
        strip_name: None,
        max_pages: options.max_pages,
    };
    planner.ingest(files, binary_paths);
    planner.hints = planner
        .sidecar
        .as_ref()
        .map(|s| {
            s.properties
                .iter()
                .map(|p| (p.name.to_lowercase(), p.kind))
                .collect()
        })
        .unwrap_or_default();
    planner.build_tree();
    planner.read_tables();
    planner.convert_pages();
    planner.build_rows();
    planner.finish(options)
}

struct Planner {
    warnings: Warnings,
    texts: Vec<TextFile>,
    nodes: Vec<Node>,
    /// The text file of each node.
    node_text: Vec<Option<usize>>,
    index: SourceIndex,
    /// The CSV files read, by the node of their database.
    tables: HashMap<usize, Table>,
    sidecar: Option<Sidecar>,
    /// The kinds the sidecar gives to property names (lowercase).
    hints: HashMap<String, PropertyKind>,
    /// The name of a folder that held the whole source and was stripped.
    strip_name: Option<String>,
    max_pages: usize,
}

/// A directory of the source that holds pages.
#[derive(Default)]
struct Dir {
    /// Page nodes directly in it.
    files: Vec<usize>,
    /// Page-bearing folders directly in it (full paths).
    subdirs: BTreeSet<String>,
}

impl Planner {
    // ----- 1. the files ----------------------------------------------------------------------

    fn ingest(&mut self, files: Vec<SourceFile>, binary_paths: &[String]) {
        let mut seen: HashSet<String> = HashSet::new();
        let mut texts: Vec<(String, String)> = Vec::new();
        for file in files {
            if !paths::is_safe_path(&file.path)
                || paths::is_hidden(&file.path)
                || !seen.insert(file.path.clone())
            {
                self.warnings
                    .push(&file.path, ImportWarningCode::UnsupportedFile, None);
                continue;
            }
            texts.push((file.path, file.text));
        }
        let mut binaries: Vec<String> = Vec::new();
        let mut seen_binary: HashSet<&str> = HashSet::new();
        for path in binary_paths {
            if paths::is_safe_path(path)
                && !paths::is_hidden(path)
                && !seen.contains(path)
                && seen_binary.insert(path)
            {
                binaries.push(path.clone());
            }
        }

        // One top-level folder around everything is stripped.
        let all = texts
            .iter()
            .map(|(path, _)| path.as_str())
            .chain(binaries.iter().map(String::as_str));
        let mut top: Option<&str> = None;
        let mut strip = true;
        for path in all {
            match path.split_once('/') {
                Some((folder, _)) if top.is_none_or(|t| t == folder) => top = Some(folder),
                _ => {
                    strip = false;
                    break;
                }
            }
        }
        let prefix = if strip {
            top.map(|t| format!("{t}/"))
        } else {
            None
        };
        if let Some(prefix) = &prefix {
            self.strip_name = Some(paths::clean_name(prefix.trim_end_matches('/'), false));
        }
        let norm_of = |orig: &str| -> String {
            match &prefix {
                Some(prefix) => orig
                    .strip_prefix(prefix.as_str())
                    .unwrap_or(orig)
                    .to_owned(),
                None => orig.to_owned(),
            }
        };

        for orig in &binaries {
            let norm = norm_of(orig);
            let name = paths::file_name(&norm).to_lowercase();
            self.index
                .binary_lower
                .entry(norm.to_lowercase())
                .or_default()
                .push(norm.clone());
            self.index
                .binary_names
                .entry(name)
                .or_default()
                .push(norm.clone());
            self.index.binary.insert(norm, orig.clone());
        }
        let mut sidecar_text: Option<(String, String)> = None;
        for (orig, text) in texts {
            let norm = norm_of(&orig);
            let ext = paths::ext_of(&norm);
            if PAGE_EXTENSIONS.contains(&ext.as_str()) {
                self.texts.push(TextFile {
                    orig,
                    norm,
                    ext,
                    text,
                });
            } else if ext == "excalidraw" {
                self.index.whiteboards.insert(norm, text);
            } else if norm == SIDECAR_NAME {
                sidecar_text = Some((orig, text));
            } else {
                self.warnings
                    .push(&orig, ImportWarningCode::UnsupportedFile, None);
            }
        }
        if let Some((orig, text)) = sidecar_text {
            match sidecar::parse(&text) {
                Some(sidecar) => {
                    // Their names come back as text (ADR-0049): say so, once per property.
                    for name in &sidecar.people {
                        self.warnings.push(
                            &orig,
                            ImportWarningCode::UnsupportedContent,
                            Some(&format!("{name} (people as text)")),
                        );
                    }
                    self.sidecar = Some(sidecar);
                }
                None => self.warnings.push(
                    &orig,
                    ImportWarningCode::UnsupportedContent,
                    Some(SIDECAR_NAME),
                ),
            }
        }
        // A deterministic order: by path.
        self.texts.sort_by(|a, b| a.norm.cmp(&b.norm));
    }

    // ----- 2. the tree -----------------------------------------------------------------------

    /// The stem the folder of a page's children has.
    fn folder_key(&self, node: usize) -> String {
        let path = &self.nodes[node].path;
        let stem = paths::split_ext(paths::file_name(path)).0;
        if self.nodes[node].kind == NodeKind::Csv {
            stem.strip_suffix("_all").unwrap_or(stem).to_owned()
        } else {
            stem.to_owned()
        }
    }

    fn build_tree(&mut self) {
        // Pages: one node per Markdown, text, HTML or CSV file. A Notion database exported twice
        // (`X.csv` and `X_all.csv`) keeps the complete one.
        let complete: HashSet<String> = self
            .texts
            .iter()
            .filter(|t| t.ext == "csv")
            .map(|t| t.norm.to_lowercase())
            .collect();
        for (text_index, file) in self.texts.iter().enumerate() {
            let (stem, _) = paths::split_ext(paths::file_name(&file.norm));
            let is_csv = file.ext == "csv";
            if is_csv && !stem.ends_with("_all") {
                let all = paths::join(paths::dir_of(&file.norm), &format!("{stem}_all.csv"));
                if complete.contains(&all.to_lowercase()) {
                    continue;
                }
            }
            let mut node = Node::new(if is_csv {
                NodeKind::Csv
            } else {
                NodeKind::File
            });
            node.orig = file.orig.clone();
            node.path = file.norm.clone();
            node.dir = paths::dir_of(&file.norm).to_owned();
            node.display = paths::clean_name(stem, is_csv);
            node.notion_id = paths::split_notion_id(
                stem.strip_suffix("_all").filter(|_| is_csv).unwrap_or(stem),
            )
            .map(|(_, id)| id.to_owned());
            node.title = node.display.clone();
            let id = self.nodes.len();
            self.index.pages.insert(file.norm.clone(), id);
            self.index
                .pages_lower
                .entry(file.norm.to_lowercase())
                .or_default()
                .push(id);
            if let Some(notion) = &node.notion_id {
                self.index.notion.entry(notion.clone()).or_insert(id);
            }
            self.nodes.push(node);
            self.node_text.push(Some(text_index));
        }

        // Directories that hold pages.
        let mut dirs: BTreeMap<String, Dir> = BTreeMap::new();
        dirs.entry(String::new()).or_default();
        let ids: Vec<usize> = (0..self.nodes.len()).collect();
        for id in ids {
            let dir = self.nodes[id].dir.clone();
            dirs.entry(dir.clone()).or_default().files.push(id);
            let mut current = dir;
            while !current.is_empty() {
                let parent = paths::dir_of(&current).to_owned();
                dirs.entry(parent.clone())
                    .or_default()
                    .subdirs
                    .insert(current.clone());
                dirs.entry(current).or_default();
                current = parent;
            }
        }
        // The page that owns a folder is the first file with its stem, in the order of the
        // extensions: Markdown, HTML, text, CSV.
        let priority = |node: &Node| -> usize {
            match paths::ext_of(&node.path).as_str() {
                "md" | "markdown" => 0,
                "html" | "htm" => 1,
                "txt" => 2,
                _ => 3,
            }
        };
        for info in dirs.values_mut() {
            info.files.sort_by(|a, b| {
                priority(&self.nodes[*a])
                    .cmp(&priority(&self.nodes[*b]))
                    .then_with(|| self.nodes[*a].path.cmp(&self.nodes[*b].path))
            });
        }

        // Attach, from the top: a page's children are the pages of the folder with its stem; a
        // folder without a page of its own is a page.
        let mut stack: Vec<(String, Option<usize>)> = vec![(String::new(), None)];
        while let Some((dir, parent)) = stack.pop() {
            let Some(info) = dirs.get(&dir) else {
                continue;
            };
            let files = info.files.clone();
            let subdirs: Vec<String> = info.subdirs.iter().cloned().collect();
            let mut consumed: HashSet<String> = HashSet::new();
            for node in files {
                self.nodes[node].parent = parent;
                if let Some(parent) = parent {
                    self.nodes[parent].children.push(node);
                }
                let folder = paths::join(&dir, &self.folder_key(node));
                if subdirs.contains(&folder) && consumed.insert(folder.clone()) {
                    stack.push((folder, Some(node)));
                }
            }
            for sub in subdirs {
                if consumed.contains(&sub) {
                    continue;
                }
                let name = paths::file_name(&sub);
                let mut folder = Node::new(NodeKind::Folder);
                folder.display = paths::clean_name(name, false);
                folder.title = folder.display.clone();
                folder.notion_id = paths::split_notion_id(name).map(|(_, id)| id.to_owned());
                folder.dir = sub.clone();
                folder.parent = parent;
                let id = self.nodes.len();
                self.nodes.push(folder);
                self.node_text.push(None);
                if let Some(parent) = parent {
                    self.nodes[parent].children.push(id);
                }
                stack.push((sub, Some(id)));
            }
        }
    }

    // ----- 3. CSV files ----------------------------------------------------------------------

    fn read_tables(&mut self) {
        let ids: Vec<usize> = (0..self.nodes.len())
            .filter(|id| self.nodes[*id].kind == NodeKind::Csv)
            .collect();
        for id in ids {
            let Some(text_index) = self.node_text[id] else {
                continue;
            };
            let text = std::mem::take(&mut self.texts[text_index].text);
            let mut table = table::parse(&text, self.max_pages.saturating_add(1));
            let dir = self.nodes[id].dir.clone();
            {
                let resolve = table::resolver(&self.index, &dir);
                table::infer(&mut table, &resolve);
            }
            let orig = self.nodes[id].orig.clone();
            for (code, detail) in table::cut_warnings(&table) {
                self.warnings.push(&orig, code, Some(detail));
            }
            self.tables.insert(id, table);
        }
    }

    // ----- 4. the content of the files -------------------------------------------------------

    fn convert_pages(&mut self) {
        for id in 0..self.nodes.len() {
            if self.nodes[id].kind == NodeKind::File {
                self.convert_node(id, true);
            }
        }
    }

    /// Reads one file's content into its node. A page in the folder of a Notion database loses
    /// the property lines at its top when `strip` is set (its row has the values); its text is
    /// kept until the rows are matched.
    fn convert_node(&mut self, id: usize, strip: bool) {
        let Some(text_index) = self.node_text[id] else {
            return;
        };
        let columns: Option<HashSet<String>> = self.nodes[id]
            .parent
            .and_then(|parent| self.tables.get(&parent))
            .map(|table| {
                table
                    .columns
                    .iter()
                    .map(|c| c.name.to_lowercase())
                    .collect()
            });
        let in_database = columns.is_some();
        let text = if in_database {
            self.texts[text_index].text.clone()
        } else {
            std::mem::take(&mut self.texts[text_index].text)
        };
        let ext = self.texts[text_index].ext.clone();
        let orig = self.nodes[id].orig.clone();
        let dir = self.nodes[id].dir.clone();
        let columns = columns.filter(|_| strip);
        let mut ctx = PageCtx::new(&self.index, &mut self.warnings, &orig, &dir, id);
        let converted = convert_file(&mut ctx, &ext, &text, columns.as_ref(), &self.hints);
        let PageCtx { needed, links, .. } = ctx;

        let node = &mut self.nodes[id];
        let mut blocks = converted.blocks;
        let title = match converted.title {
            Some(title) => title,
            None => take_heading_title(&mut blocks, &node.display, node.notion_id.is_some())
                .unwrap_or_else(|| node.display.clone()),
        };
        node.title = clean_title(&title, &node.display);
        node.icon = converted.icon;
        node.cover = converted.cover;
        node.cover_y = converted.cover_y;
        node.template = converted.template;
        node.template_placeholders = converted.template_placeholders;
        node.blocks = blocks;
        node.links = links;
        node.needed = needed;
        node.props = converted.props;
        node.stripped = converted.stripped;
    }

    // ----- 5. database rows ------------------------------------------------------------------

    fn build_rows(&mut self) {
        let ids: Vec<usize> = (0..self.nodes.len())
            .filter(|id| self.nodes[*id].kind == NodeKind::Csv)
            .collect();
        for id in ids {
            let Some(table) = self.tables.get(&id) else {
                continue;
            };
            let dir = self.nodes[id].dir.clone();
            let rows: Vec<(String, Vec<(String, RawValue)>)> = {
                let resolve = table::resolver(&self.index, &dir);
                (0..table.rows.len())
                    .map(|row| (table.title(row), table::row_values(table, row, &resolve)))
                    .collect()
            };
            let names: Vec<Value> = table
                .property_names()
                .map(|name| Value::String(name.to_owned()))
                .collect();

            // Row pages of a Notion export, by title (then by the name of the file).
            let files: Vec<usize> = self.nodes[id]
                .children
                .iter()
                .copied()
                .filter(|child| self.nodes[*child].kind == NodeKind::File)
                .collect();
            let mut by_title: HashMap<String, VecDeque<usize>> = HashMap::new();
            let mut by_folded: HashMap<String, VecDeque<usize>> = HashMap::new();
            let mut by_name: HashMap<String, VecDeque<usize>> = HashMap::new();
            for file in &files {
                let node = &self.nodes[*file];
                let title = node.title.trim().to_owned();
                by_title.entry(title.clone()).or_default().push_back(*file);
                by_folded
                    .entry(title.to_lowercase())
                    .or_default()
                    .push_back(*file);
                by_name
                    .entry(node.display.trim().to_lowercase())
                    .or_default()
                    .push_back(*file);
            }
            let mut taken: HashSet<usize> = HashSet::new();
            let mut new_rows: Vec<Node> = Vec::new();
            for (row, (title, values)) in rows.into_iter().enumerate() {
                let wanted = title.trim();
                let found = [
                    (&mut by_title, wanted.to_owned()),
                    (&mut by_folded, wanted.to_lowercase()),
                    (&mut by_name, wanted.to_lowercase()),
                ]
                .into_iter()
                .find_map(|(map, key)| {
                    let queue = map.get_mut(&key)?;
                    while let Some(candidate) = queue.pop_front() {
                        if !taken.contains(&candidate) {
                            return Some(candidate);
                        }
                    }
                    None
                });
                match found {
                    Some(file) => {
                        taken.insert(file);
                        let node = &mut self.nodes[file];
                        node.row_index = Some(row);
                        node.row_props = values;
                        node.title = clean_title(&title, &node.display);
                    }
                    None => {
                        let mut node = Node::new(NodeKind::Row);
                        node.title = clean_title(&title, "Untitled");
                        node.display = node.title.clone();
                        node.row_props = values;
                        node.row_index = Some(row);
                        new_rows.push(node);
                    }
                }
            }
            // A page that matches no row keeps its property lines: nothing carries its values.
            for file in &files {
                if !taken.contains(file) && self.nodes[*file].stripped {
                    self.convert_node(*file, false);
                }
            }
            for file in &files {
                if let Some(text_index) = self.node_text[*file] {
                    self.texts[text_index].text = String::new();
                }
            }
            for mut node in new_rows {
                node.parent = Some(id);
                let new_id = self.nodes.len();
                self.nodes.push(node);
                self.node_text.push(None);
                self.nodes[id].children.push(new_id);
            }

            // The database page shows its rows as a table.
            let mut view = json::attrs([
                (placeholder::NAMES, Value::Bool(true)),
                ("source", serde_json::json!({"kind": "children"})),
                ("layout", Value::String("table".into())),
                ("properties", Value::Array(names)),
            ]);
            view.insert(placeholder::NAMES.into(), Value::Bool(true));
            self.nodes[id].blocks = vec![json::element("pageList", view, Vec::new())];
        }
    }

    // ----- 6. the plan -----------------------------------------------------------------------

    fn finish(mut self, options: &ImportOptions) -> Plan {
        let order = self.order(options);
        let kept = order.len().min(self.max_pages);
        if order.len() > kept {
            let first_left_out = &self.nodes[order[kept]];
            let path = first_left_out.orig.clone();
            self.warnings.push(
                &path,
                ImportWarningCode::Truncated,
                Some(&format!("{} pages left out", order.len() - kept)),
            );
        }
        let order: Vec<usize> = order.into_iter().take(kept).collect();
        let mut index_of: Vec<Option<usize>> = vec![None; self.nodes.len()];
        for (position, node) in order.iter().enumerate() {
            index_of[*node] = Some(position);
        }

        // Titles, for wiki links and for text values.
        let mut titles: HashMap<String, Vec<usize>> = HashMap::new();
        for node in &order {
            let n = &self.nodes[*node];
            titles
                .entry(n.title.to_lowercase())
                .or_default()
                .push(*node);
            if n.display.to_lowercase() != n.title.to_lowercase() {
                titles
                    .entry(n.display.to_lowercase())
                    .or_default()
                    .push(*node);
            }
        }

        // Properties.
        let sidecar_properties: Vec<PlannedProperty> = self
            .sidecar
            .as_ref()
            .map(|s| s.properties.clone())
            .unwrap_or_default();
        let mut aggregator = Aggregator::new(&sidecar_properties);
        for node in &order {
            if let Some(table) = self.tables.get(node) {
                for (position, column) in table.columns.iter().enumerate() {
                    if position == table.title_col {
                        continue;
                    }
                    aggregator.observe(&column.name, column.kind.property_kind(), &column.options);
                }
            }
            for (name, value) in &self.nodes[*node].props {
                match value {
                    RawValue::Plain(value) => aggregator.observe_value(name, value),
                    RawValue::Pages(_) => {
                        aggregator.observe(name, Some(PropertyKind::Relation), &[]);
                    }
                }
            }
        }

        // The blocks move into the plan, they are not copied.
        let mut contents: Vec<Vec<Value>> = order
            .iter()
            .map(|id| std::mem::take(&mut self.nodes[*id].blocks))
            .collect();
        let resolver = Links {
            nodes: &self.nodes,
            index_of: &index_of,
            node_of: &order,
            titles: &titles,
            index: &self.index,
        };

        let mut pages = Vec::with_capacity(order.len());
        let mut needed: Vec<String> = Vec::new();
        let mut needed_seen: HashSet<String> = HashSet::new();
        for (position, node_id) in order.iter().enumerate() {
            let node = &self.nodes[*node_id];
            let parent = node.parent.and_then(|p| index_of[p]);
            let mut blocks = std::mem::take(&mut contents[position]);
            if blocks.is_empty() {
                blocks.push(json::paragraph(Vec::new()));
            }
            let mut content = serde_json::json!({"type": "doc"});
            if let Value::Object(object) = &mut content {
                object.insert("content".into(), Value::Array(blocks));
            }
            remap_content(&mut content, &resolver, *node_id, 0);

            // The page's values: its own, then its row's (the row wins).
            let mut values: Vec<(String, ImportValue)> = Vec::new();
            for (name, raw) in node.props.iter().chain(node.row_props.iter()) {
                let Some(canonical) = aggregator.canonical(name) else {
                    continue;
                };
                let kind = aggregator.kind_of(name).unwrap_or(PropertyKind::Text);
                let Some(value) = resolver.value(raw, *node_id) else {
                    continue;
                };
                let value = fit(value, kind, &resolver);
                if let Some(existing) = values
                    .iter_mut()
                    .find(|(n, _)| n.eq_ignore_ascii_case(canonical))
                {
                    existing.1 = value;
                } else {
                    values.push((canonical.to_owned(), value));
                }
            }
            for path in &node.needed {
                if needed_seen.insert(path.clone()) {
                    needed.push(path.clone());
                }
            }
            for (_, value) in &values {
                if let ImportValue::Files(files) = value {
                    for path in files {
                        if needed_seen.insert(path.clone()) {
                            needed.push(path.clone());
                        }
                    }
                }
            }
            pages.push(PlannedPage {
                parent,
                title: node.title.clone(),
                icon: node.icon.clone(),
                cover: node.cover.clone(),
                cover_y: node.cover_y,
                template: node.template,
                template_placeholders: node.template_placeholders.clone(),
                source_path: node.orig.clone(),
                content,
                properties: values,
            });
        }

        Plan {
            pages,
            properties: aggregator.finish(),
            needed_files: needed,
            needed_file_names: Vec::new(),
            warnings: std::mem::take(&mut self.warnings).into_vec(),
            space_roots: Vec::new(),
            remote_files: Vec::new(),
        }
    }

    /// The pages in creation order: a parent before its children, siblings sorted.
    fn order(&mut self, options: &ImportOptions) -> Vec<usize> {
        let roots: Vec<usize> = (0..self.nodes.len())
            .filter(|id| self.nodes[*id].parent.is_none())
            .collect();
        let rank: HashMap<String, usize> = self
            .sidecar
            .as_ref()
            .map(|s| {
                s.pages
                    .iter()
                    .enumerate()
                    .map(|(position, path)| (path.clone(), position))
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        let has_sidecar = self.sidecar.is_some();

        let mut roots = roots;
        self.sort_siblings(&mut roots, None, &rank, has_sidecar);
        let top: Vec<usize> = match (&options.source_name, roots.len()) {
            (Some(name), count) if count > 1 => {
                let title = self
                    .strip_name
                    .clone()
                    .unwrap_or_else(|| paths::clean_name(name, false));
                let mut container = Node::new(NodeKind::Container);
                container.title = clean_title(&title, "Imported");
                container.display = container.title.clone();
                let id = self.nodes.len();
                self.nodes.push(container);
                self.node_text.push(None);
                for root in &roots {
                    self.nodes[*root].parent = Some(id);
                }
                self.nodes[id].children = roots;
                vec![id]
            }
            _ => roots,
        };

        let mut order = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<usize> = top.into_iter().rev().collect();
        while let Some(id) = stack.pop() {
            order.push(id);
            let mut children = std::mem::take(&mut self.nodes[id].children);
            self.sort_siblings(&mut children, Some(id), &rank, has_sidecar);
            self.nodes[id].children.clone_from(&children);
            stack.extend(children.into_iter().rev());
        }
        order
    }

    fn sort_siblings(
        &self,
        siblings: &mut [usize],
        parent: Option<usize>,
        rank: &HashMap<String, usize>,
        has_sidecar: bool,
    ) {
        let link_rank: HashMap<usize, usize> = match parent {
            Some(parent) if !has_sidecar => self.nodes[parent]
                .links
                .iter()
                .enumerate()
                .map(|(position, id)| (*id, position))
                .collect(),
            _ => HashMap::new(),
        };
        siblings.sort_by(|a, b| {
            let key = |id: usize| {
                let node = &self.nodes[id];
                (
                    node.row_index.unwrap_or(usize::MAX),
                    rank.get(&node.path).copied().unwrap_or(usize::MAX),
                    link_rank.get(&id).copied().unwrap_or(usize::MAX),
                )
            };
            key(*a)
                .cmp(&key(*b))
                .then_with(|| natural_cmp(&self.nodes[*a].display, &self.nodes[*b].display))
                .then_with(|| self.nodes[*a].path.cmp(&self.nodes[*b].path))
                .then_with(|| a.cmp(b))
        });
    }
}

/// Reads one file's content.
fn convert_file(
    ctx: &mut PageCtx<'_>,
    ext: &str,
    source: &str,
    db_columns: Option<&HashSet<String>>,
    hints: &HashMap<String, PropertyKind>,
) -> Converted {
    match ext {
        "txt" => Converted {
            title: None,
            icon: None,
            cover: None,
            cover_y: None,
            template: false,
            template_placeholders: Vec::new(),
            blocks: text::convert(source),
            props: Vec::new(),
            stripped: false,
        },
        "html" | "htm" => {
            let page = crate::html::convert_document(ctx, source);
            Converted {
                title: page.title,
                icon: page.icon,
                cover: None,
                cover_y: None,
                template: false,
                template_placeholders: Vec::new(),
                blocks: page.blocks,
                props: Vec::new(),
                stripped: false,
            }
        }
        _ => {
            let split = yaml::split(source);
            let mut converted = Converted {
                title: None,
                icon: None,
                cover: None,
                cover_y: None,
                template: false,
                template_placeholders: Vec::new(),
                blocks: Vec::new(),
                props: Vec::new(),
                stripped: false,
            };
            if let Some(front) = split.yaml {
                match yaml::parse(front) {
                    Some(pairs) => {
                        let front = frontmatter::extract(pairs, ctx, hints);
                        converted.title = front.title;
                        converted.icon = front.icon;
                        converted.cover = front.cover;
                        converted.cover_y = front.cover_y;
                        converted.template = front.template;
                        converted.template_placeholders = front.template_placeholders;
                        converted.props = front.props;
                    }
                    None => ctx.warn(ImportWarningCode::UnsupportedContent, Some("front matter")),
                }
            }
            let body: Cow<'_, str> = match db_columns {
                Some(columns) => notion::strip_property_lines(split.body, columns),
                None => Cow::Borrowed(split.body),
            };
            converted.stripped = matches!(body, Cow::Owned(_));
            converted.blocks = markdown::convert(ctx, &body);
            converted
        }
    }
}

/// The title a first level-1 heading gives: removed from the blocks when its text is the name of
/// the file or the file has a Notion id.
fn take_heading_title(blocks: &mut Vec<Value>, display: &str, notion: bool) -> Option<String> {
    let first = blocks.first()?;
    if kind(first) != "heading" || json::attr(first, "level").and_then(Value::as_i64) != Some(1) {
        return None;
    }
    let text = plain_text(first);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if notion || text.to_lowercase() == display.trim().to_lowercase() {
        let text = text.to_owned();
        blocks.remove(0);
        return Some(text);
    }
    None
}

/// A title as the core accepts it: trimmed, on one line, at most 500 characters; `fallback` when
/// nothing is left.
fn clean_title(title: &str, fallback: &str) -> String {
    let one_line: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = truncate_chars(one_line.trim(), MAX_TITLE_CHARS);
    if title.is_empty() {
        fallback.to_owned()
    } else {
        title
    }
}

// ----- turning arena references into plan references -------------------------------------------

/// Where links and values may point, once the plan's pages and their order are known.
struct Links<'a> {
    nodes: &'a [Node],
    index_of: &'a [Option<usize>],
    /// The arena index of each page of the plan.
    node_of: &'a [usize],
    titles: &'a HashMap<String, Vec<usize>>,
    index: &'a SourceIndex,
}

impl Links<'_> {
    /// The plan index of an arena page that was kept.
    fn kept(&self, node: usize) -> Option<usize> {
        self.index_of.get(node).copied().flatten()
    }

    /// The page a wiki link names: by path when it has one, by title or name otherwise; among
    /// several, one in the folder of the page that holds the link, else the first.
    fn wiki(&self, name: &str, from: usize) -> Option<usize> {
        let name = name.trim();
        let from_dir = self.nodes.get(from).map_or("", |n| n.dir.as_str());
        if name.contains('/') {
            for base in [from_dir, ""] {
                if let Some(found) = self
                    .index
                    .find_page(&paths::join(base, name))
                    .and_then(|id| self.kept(id).map(|_| id))
                {
                    return Some(found);
                }
            }
        }
        let last = name.rsplit('/').next().unwrap_or(name);
        let (stem, ext) = paths::split_ext(last);
        let lookup = if PAGE_EXTENSIONS.contains(&ext.as_str()) {
            stem
        } else {
            last
        };
        let candidates = self.titles.get(&lookup.trim().to_lowercase())?;
        let mut best: Option<usize> = None;
        for candidate in candidates {
            if self.kept(*candidate).is_none() {
                continue;
            }
            if self.nodes[*candidate].dir == from_dir {
                return Some(*candidate);
            }
            best.get_or_insert(*candidate);
        }
        best
    }

    /// The target of a placeholder in a plan reference.
    fn target(&self, target: &Target, from: usize) -> Target {
        match target {
            Target::Planned(node) => match self.kept(*node) {
                Some(index) => Target::Planned(index),
                None => Target::Title(
                    self.nodes
                        .get(*node)
                        .map(|n| n.title.clone())
                        .unwrap_or_default(),
                ),
            },
            Target::Wiki { name, .. } => match self.wiki(name, from).and_then(|id| self.kept(id)) {
                Some(index) => Target::Planned(index),
                None => {
                    let last = name.rsplit('/').next().unwrap_or(name);
                    let (stem, ext) = paths::split_ext(last);
                    Target::Title(
                        if PAGE_EXTENSIONS.contains(&ext.as_str()) {
                            stem
                        } else {
                            last
                        }
                        .trim()
                        .to_owned(),
                    )
                }
            },
            other => other.clone(),
        }
    }

    /// A value of a page as an import value.
    fn value(&self, raw: &RawValue, from: usize) -> Option<ImportValue> {
        match raw {
            RawValue::Plain(value) => Some(value.clone()),
            RawValue::Pages(targets) => {
                let mut out: Vec<PageTarget> = Vec::new();
                for target in targets {
                    let resolved = match target {
                        PendingTarget::Planned(node) => {
                            match self.target(&Target::Planned(*node), from) {
                                Target::Planned(index) => PageTarget::Planned(index),
                                Target::Title(title) => PageTarget::Title(title),
                                _ => continue,
                            }
                        }
                        PendingTarget::Wiki(name) => match self.target(
                            &Target::Wiki {
                                name: name.clone(),
                                from,
                            },
                            from,
                        ) {
                            Target::Planned(index) => PageTarget::Planned(index),
                            Target::Title(title) => PageTarget::Title(title),
                            _ => continue,
                        },
                        PendingTarget::Title(title) => PageTarget::Title(title.clone()),
                    };
                    if !out.contains(&resolved) {
                        out.push(resolved);
                    }
                }
                (!out.is_empty()).then_some(ImportValue::Pages(out))
            }
        }
    }

    /// The title of a page of the plan, by index.
    fn title_of(&self, target: &PageTarget) -> String {
        match target {
            PageTarget::Title(title) => title.clone(),
            PageTarget::Planned(index) => self
                .node_of
                .get(*index)
                .and_then(|node| self.nodes.get(*node))
                .map(|n| n.title.clone())
                .unwrap_or_default(),
        }
    }
}

/// A value as text, for a property whose kind (after merging several observations) is text.
fn fit(value: ImportValue, kind: PropertyKind, links: &Links<'_>) -> ImportValue {
    if kind != PropertyKind::Text {
        return value;
    }
    let text = match &value {
        ImportValue::Text(_) => return value,
        ImportValue::Number(n) => values::number_text(*n),
        ImportValue::Bool(b) => if *b { "Yes" } else { "No" }.to_owned(),
        ImportValue::Date { date, time } => match time {
            Some(time) => format!("{date} {time}"),
            None => date.clone(),
        },
        ImportValue::List(items) => items.join(", "),
        ImportValue::Pages(targets) => targets
            .iter()
            .map(|t| links.title_of(t))
            .collect::<Vec<_>>()
            .join(", "),
        ImportValue::Files(files) => files
            .iter()
            .map(|f| paths::file_name(f))
            .collect::<Vec<_>>()
            .join(", "),
        ImportValue::Checklist(items) => items
            .iter()
            .map(|(done, text)| format!("[{}] {text}", if *done { "x" } else { " " }))
            .collect::<Vec<_>>()
            .join("; "),
        ImportValue::Place { name, .. } => name.clone(),
    };
    ImportValue::Text(text)
}

/// Rewrites the page references of a page's content from arena references to plan references,
/// and resolves wiki links.
fn remap_content(value: &mut Value, links: &Links<'_>, from: usize, depth: usize) {
    if depth > 200 {
        return;
    }
    if let Some(Value::Object(attrs)) = value.get_mut("attrs")
        && let Some(slot) = attrs.get_mut(placeholder::PAGE)
        && let Some(target) = Target::from_json(slot)
    {
        *slot = links.target(&target, from).to_json();
    }
    if let Some(Value::Array(children)) = value.get_mut("content") {
        for child in children {
            remap_content(child, links, from, depth + 1);
        }
    }
}
