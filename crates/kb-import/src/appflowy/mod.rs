//! The reader of an AppFlowy "Back up your workspace" archive (ADR-0039,
//! `docs/architecture/appflowy-import.md`).
//!
//! Clean room (ADR-0010): the format was learnt from an archive and public knowledge, never from
//! AppFlowy's code. Only the JSON copies are read (`relation_map.json`, `metadata.json`,
//! `EXPORT_WARNINGS.json`, `collab_jsons/documents/*.json`, `collab_jsons/databases/*.json`);
//! `collab_objects/` is ignored. The view tree becomes the page tree, each space a space root,
//! documents portable content with the crate's placeholders, a database a page with one
//! `pageList` block and a child page per row (the row's document is the one whose id is
//! `uuid_v5(row id, "document_id")`), fields planned properties, `local_files/*` needed files
//! and `https` images remote files.
//!
//! Modules: `archive` (the files and the view table), `blocks` (documents), `delta` (text),
//! `colors`, `database` (fields, cells, the view), `env` (what the conversion shares).

mod archive;
mod blocks;
mod colors;
mod database;
mod delta;
mod env;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_blocks;
#[cfg(test)]
mod tests_db;
#[cfg(test)]
mod tests_hostile;
#[cfg(test)]
mod tests_survey;

use std::collections::{HashMap, HashSet};

use kb_types::{ImportWarningCode, PropertyKind};
use serde_json::{Value, json};

use self::archive::{Archive, View, row_document_id};
use self::blocks::DocData;
use self::database::{
    Database, Props, cell_value, page_list, property_kind, property_name, value_text,
};
use self::env::Env;
use crate::paths::truncate_chars;
use crate::placeholder::{self, Target};
use crate::{
    ImportOptions, ImportValue, Plan, PlannedCover, PlannedPage, SourceFile, emoji, json as pj,
};

/// The longest page title, in characters (the core's limit).
const MAX_TITLE_CHARS: usize = 500;
/// The title of the folder that holds the views outside any space.
const OTHER_PAGES: &str = "Other pages";

/// Plans an AppFlowy backup, or gives the files back when the source is not one: `relation_map.json`
/// and `metadata.json` at its root (after the usual stripping of one wrapping folder) and a
/// `views` object in the relation map. Never fails: what it cannot read becomes a warning.
pub(crate) fn plan(
    files: Vec<SourceFile>,
    binary_paths: &[String],
    options: &ImportOptions,
) -> Result<Plan, Vec<SourceFile>> {
    let archive = Archive::open(files, binary_paths)?;
    Ok(Reader::new(archive, options.max_pages, options.source_name.as_deref()).run())
}

/// What a planned page is.
enum Kind {
    /// A space: an empty page that stands for it.
    Space,
    /// The folder of the views outside any space (also a space root).
    Other,
    /// A document, by its id.
    Document(String),
    /// A view whose document or database the archive lacks.
    Missing,
    /// A database view whose file is not a database.
    Unreadable,
    /// A view of a layout this reader does not know and has no document for.
    Unsupported(i64),
    /// A database view that holds the rows: the database (an index into `Reader::dbs`) and the
    /// layout of the view.
    Database(usize, i64),
    /// Another view of a database whose rows are held elsewhere: the holder's plan index.
    Linked(usize),
    /// A row of a database: the database and the row (indexes).
    Row(usize, usize),
}

/// One planned page before it is flattened into the plan.
struct Node {
    parent: Option<usize>,
    title: String,
    icon: Option<String>,
    path: String,
    kind: Kind,
}

/// How the fields of the databases map to the planned properties.
#[derive(Default)]
struct FieldPlan {
    /// The planned name of a field, by database and field id.
    names: HashMap<(usize, String), String>,
    /// The relation fields that stay relations (every target is planned).
    relations: HashSet<(usize, String)>,
    /// The final kind of each property, by lowercase name.
    kinds: HashMap<String, PropertyKind>,
}

struct Reader {
    a: Archive,
    max_pages: usize,
    env: Env,
    nodes: Vec<Node>,
    dbs: Vec<Database>,
    /// The database of a database view: its index and the id of the view whose file holds it
    /// (the view itself, or the embedded view of a container).
    db_of_view: HashMap<String, (usize, String)>,
    /// Embedded views of database containers, by the container that shows them: they are not
    /// pages of their own.
    absorbed: HashMap<String, String>,
    /// The title of the folder of the views outside any space.
    other_title: String,
    /// Ids of the views that are the documents of database rows: not pages.
    row_docs: HashSet<String>,
    /// Row id to the id of the document that holds its content.
    row_doc_of: HashMap<String, String>,
    /// Database id to the plan index of the view that holds its rows.
    held: HashMap<String, usize>,
    visited: HashSet<String>,
    /// The nodes that are spaces or the folder of the other views, in order.
    space_roots: Vec<usize>,
    /// The cover of each planned page that has one (ADR-0040), by plan index.
    covers: HashMap<usize, PlannedCover>,
}

/// A title as the core accepts it: on one line without control characters, at most 500
/// characters, `Untitled` when empty.
fn clean_title(title: &str) -> String {
    let spaced: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one_line: String = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = truncate_chars(one_line.trim(), MAX_TITLE_CHARS);
    if title.is_empty() {
        "Untitled".to_owned()
    } else {
        title
    }
}

/// What a view's `extra.cover` is, by its type: AppFlowy's cover types are none, colour, file and
/// asset (its own built-in pictures), written as a number or as a name (`color`, `CoverType.file`).
enum CoverKind {
    /// A colour (a value such as `0xFFB8D2F4`).
    Color,
    /// An uploaded or linked picture: a `local_files` name or a URL.
    File,
    /// A built-in picture, a gradient or a type this reader does not know.
    Other,
}

fn cover_kind(ty: &str) -> CoverKind {
    let ty = ty.trim().to_ascii_lowercase();
    match ty.rsplit('.').next().unwrap_or_default() {
        "1" | "color" | "purecolor" => CoverKind::Color,
        "2" | "file" | "customimage" | "unsplashimage" | "unsplash" => CoverKind::File,
        _ => CoverKind::Other,
    }
}

/// Whether text is an icon an emoji field can hold.
fn is_emoji(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.len() <= 32 && emoji::is_icon_word(text)
}

impl Reader {
    fn new(a: Archive, max_pages: usize, source_name: Option<&str>) -> Self {
        let mut env = Env::default();
        // The views outside any space go to one more space, named after the source.
        let other_title = match source_name.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => clean_title(name),
            None => OTHER_PAGES.to_owned(),
        };
        env.local.clone_from(&a.local);
        Self {
            a,
            max_pages,
            env,
            nodes: Vec::new(),
            dbs: Vec::new(),
            db_of_view: HashMap::new(),
            absorbed: HashMap::new(),
            other_title,
            row_docs: HashSet::new(),
            row_doc_of: HashMap::new(),
            held: HashMap::new(),
            visited: HashSet::new(),
            space_roots: Vec::new(),
            covers: HashMap::new(),
        }
    }

    fn run(mut self) -> Plan {
        self.read_databases();
        self.build_tree();
        let (properties, mut fields) = self.properties();
        fields.kinds = properties.kinds();
        let pages = self.convert(&fields);
        Plan {
            pages,
            properties: properties.finish(),
            needed_files: std::mem::take(&mut self.env.needed),
            needed_file_names: std::mem::take(&mut self.env.needed_names),
            warnings: std::mem::take(&mut self.env.warnings).into_vec(),
            space_roots: self.space_roots,
            remote_files: std::mem::take(&mut self.env.remote),
        }
    }

    // ----- 1. databases and row documents ----------------------------------------------------

    fn read_databases(&mut self) {
        let mut ids: Vec<String> = self
            .a
            .views
            .values()
            .filter(|v| v.is_database())
            .map(|v| v.id.clone())
            .collect();
        ids.sort();
        for id in &ids {
            let Some((text, _)) = self.a.database(id) else {
                continue;
            };
            let Some(db) = Database::parse(text, id) else {
                continue;
            };
            self.db_of_view
                .insert(id.clone(), (self.dbs.len(), id.clone()));
            self.dbs.push(db);
        }
        // A database container has no file of its own: its embedded view holds the database. The
        // container is the page; the embedded view is not one.
        for id in &ids {
            let Some(container) = self.a.views.get(id) else {
                continue;
            };
            if !container.is_container || self.db_of_view.contains_key(id) {
                continue;
            }
            let mut candidates: Vec<&View> = container
                .children
                .iter()
                .filter_map(|c| self.a.views.get(c))
                .collect();
            let mut unlisted: Vec<&View> = self
                .a
                .views
                .values()
                .filter(|v| v.parent == *id && !container.children.contains(&v.id))
                .collect();
            unlisted.sort_by(|x, y| (x.created, &x.id).cmp(&(y.created, &y.id)));
            candidates.extend(unlisted);
            let embedded = candidates.into_iter().find(|v| {
                v.embedded
                    && !self.absorbed.contains_key(&v.id)
                    && self.db_of_view.contains_key(&v.id)
            });
            if let Some(embedded) = embedded
                && let Some((index, _)) = self.db_of_view.get(&embedded.id).cloned()
            {
                let child = embedded.id.clone();
                self.db_of_view.insert(id.clone(), (index, child.clone()));
                self.absorbed.insert(child, id.clone());
            }
        }
        for db in &self.dbs {
            for row in &db.rows {
                let derived = row_document_id(&row.id);
                let hint = self.a.row_document_hints.get(&row.id).cloned();
                for id in derived.iter().chain(hint.iter()) {
                    self.row_docs.insert(id.clone());
                }
                let chosen = match (derived, hint) {
                    (Some(d), _) if self.a.has_document(&d) => Some(d),
                    (_, Some(h)) if self.a.has_document(&h) => Some(h),
                    (Some(d), _) => Some(d),
                    (None, hint) => hint,
                };
                if let Some(id) = chosen {
                    self.row_doc_of.insert(row.id.clone(), id);
                }
            }
        }
    }

    // ----- 2. the tree -----------------------------------------------------------------------

    /// Adds a page; `None` when the plan is full.
    fn add(&mut self, node: Node) -> Option<usize> {
        if self.nodes.len() >= self.max_pages {
            return None;
        }
        self.env.titles.push(node.title.clone());
        self.nodes.push(node);
        Some(self.nodes.len() - 1)
    }

    /// The icon of a view: an emoji, else nothing (and one warning per import).
    fn view_icon(&mut self, view: &View) -> Option<String> {
        let tree = self.a.tree_path();
        if let Some((ty, value)) = &view.icon {
            if *ty == 0 && is_emoji(value) {
                return Some(value.trim().to_owned());
            }
            if !value.is_empty() {
                self.env
                    .warn(&tree, ImportWarningCode::UnsupportedContent, Some("icon"));
            }
        }
        if view.is_space
            && let Some(icon) = &view.space_icon
        {
            if is_emoji(icon) {
                return Some(icon.trim().to_owned());
            }
            self.env
                .warn(&tree, ImportWarningCode::UnsupportedContent, Some("icon"));
        }
        None
    }

    /// The children of a view in order: the ones it lists, then views that name it as their
    /// parent without being listed. The embedded view of a database container is replaced by its
    /// own children: it is not a page.
    fn children_of(&self, view: &View, unlisted: &HashMap<String, Vec<String>>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let add = |id: &String, out: &mut Vec<String>| {
            let Some(child) = self.a.views.get(id) else {
                return;
            };
            if self.absorbed.contains_key(id) {
                out.extend(
                    child
                        .children
                        .iter()
                        .filter(|c| self.a.views.contains_key(*c))
                        .cloned(),
                );
                if let Some(more) = unlisted.get(id) {
                    out.extend(more.iter().cloned());
                }
            } else {
                out.push(id.clone());
            }
        };
        for id in &view.children {
            add(id, &mut out);
        }
        if let Some(more) = unlisted.get(&view.id) {
            for id in more {
                add(id, &mut out);
            }
        }
        out
    }

    /// A view that is not a page of its own: the document of a database row, a template of a
    /// database's rows, the embedded view of a database container.
    fn is_hidden(&self, view: &View) -> bool {
        view.template
            || self.absorbed.contains_key(&view.id)
            || self.row_docs.contains(&view.id)
            || self.row_docs.contains(&view.object)
    }

    fn build_tree(&mut self) {
        // Views that name a parent without that parent listing them.
        let mut unlisted: HashMap<String, Vec<String>> = HashMap::new();
        let mut sorted: Vec<&View> = self.a.views.values().collect();
        sorted.sort_by(|x, y| (x.created, &x.id).cmp(&(y.created, &y.id)));
        let listed: HashMap<&str, HashSet<&str>> = self
            .a
            .views
            .values()
            .map(|v| {
                (
                    v.id.as_str(),
                    v.children.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        for view in &sorted {
            if let Some(parent) = self.a.views.get(&view.parent)
                && !listed
                    .get(parent.id.as_str())
                    .is_some_and(|children| children.contains(view.id.as_str()))
            {
                unlisted
                    .entry(parent.id.clone())
                    .or_default()
                    .push(view.id.clone());
            }
        }
        let by_age: Vec<String> = sorted.iter().map(|v| v.id.clone()).collect();
        // Templates of database rows are not pages.
        if sorted.iter().any(|v| v.template) {
            let tree = self.a.tree_path();
            self.env.warn(
                &tree,
                ImportWarningCode::UnsupportedContent,
                Some("template"),
            );
        }

        // The spaces, oldest first.
        let mut spaces: Vec<&View> = self.a.views.values().filter(|v| v.is_space).collect();
        spaces.sort_by(|x, y| {
            let key = |v: &View| {
                v.space_created
                    .unwrap_or_else(|| v.created.saturating_mul(1000))
            };
            (key(x), x.created, &x.id).cmp(&(key(y), y.created, &y.id))
        });
        let spaces: Vec<String> = spaces.iter().map(|v| v.id.clone()).collect();
        for space in spaces {
            self.plan_views(&space, None, &unlisted);
        }

        // Everything else that is a page: views with an unknown parent, then what a cycle kept
        // out of reach.
        let is_page = |this: &Self, id: &str| {
            !this.visited.contains(id) && this.a.views.get(id).is_some_and(|v| !this.is_hidden(v))
        };
        let orphans: Vec<String> = by_age
            .iter()
            .filter(|id| is_page(self, id))
            .filter(|id| {
                self.a
                    .views
                    .get(*id)
                    .is_some_and(|v| !self.a.views.contains_key(&v.parent))
            })
            .cloned()
            .collect();
        let mut other: Option<usize> = None;
        let mut pending: Vec<String> = orphans;
        let mut second_pass = false;
        loop {
            for id in pending {
                if !is_page(self, &id) {
                    continue;
                }
                if other.is_none() {
                    other = self.add_other();
                    if other.is_none() {
                        break;
                    }
                }
                self.plan_views(&id, other, &unlisted);
            }
            if second_pass {
                break;
            }
            second_pass = true;
            pending = by_age
                .iter()
                .filter(|id| is_page(self, id))
                .cloned()
                .collect();
        }
        self.truncation_warning(other.is_some());
    }

    /// The cover of a view (ADR-0040), when it can be one: a colour of AppFlowy's (by hue, to the
    /// nearest of the nine palette colours), an image of the archive's `local_files` or an `https`
    /// image (a remote file, downloaded once by the shell). AppFlowy's built-in covers and
    /// anything unreadable are dropped with one warning per import.
    fn view_cover(&mut self, view: &View) -> Option<PlannedCover> {
        if !view.has_cover {
            return None;
        }
        let mapped = view
            .cover
            .as_ref()
            .and_then(|(ty, value)| self.map_cover(ty, value));
        if mapped.is_none() {
            let tree = self.a.tree_path();
            self.env
                .warn(&tree, ImportWarningCode::UnsupportedContent, Some("cover"));
        }
        mapped
    }

    fn map_cover(&mut self, ty: &str, value: &str) -> Option<PlannedCover> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        match cover_kind(ty) {
            CoverKind::Color => colors::palette(value, true)
                .map(|name| PlannedCover::Named(format!("color:{name}"))),
            CoverKind::File => self
                .env
                .local_file(value, None)
                .or_else(|| self.env.remote_key(value))
                .map(PlannedCover::File),
            CoverKind::Other => None,
        }
    }

    fn add_other(&mut self) -> Option<usize> {
        let path = self.a.tree_path();
        let index = self.add(Node {
            parent: None,
            title: self.other_title.clone(),
            icon: None,
            path,
            kind: Kind::Other,
        })?;
        self.space_roots.push(index);
        Some(index)
    }

    /// Plans a view and everything under it, in order.
    fn plan_views(
        &mut self,
        root: &str,
        parent: Option<usize>,
        unlisted: &HashMap<String, Vec<String>>,
    ) {
        let mut stack: Vec<(String, Option<usize>)> = vec![(root.to_owned(), parent)];
        while let Some((id, parent)) = stack.pop() {
            if self.visited.contains(&id) {
                continue;
            }
            let Some(view) = self.a.views.get(&id).cloned() else {
                continue;
            };
            if self.is_hidden(&view) {
                self.visited.insert(id);
                continue;
            }
            self.visited.insert(id.clone());
            let icon = self.view_icon(&view);
            let cover = self.view_cover(&view);
            let title = clean_title(&view.name);
            let (kind, path) = self.kind_of(&view);
            let is_space = matches!(kind, Kind::Space);
            let holder = matches!(kind, Kind::Database(..));
            let Some(index) = self.add(Node {
                parent: if is_space { None } else { parent },
                title,
                icon,
                path,
                kind,
            }) else {
                continue;
            };
            self.env.views.insert(view.id.clone(), index);
            if let Some(cover) = cover {
                self.covers.insert(index, cover);
            }
            if matches!(self.nodes[index].kind, Kind::Database(..) | Kind::Linked(_)) {
                self.env.db_pages.insert(index);
            }
            // The embedded view of a container is the container's page for every link.
            if let Some((_, effective)) = self.db_of_view.get(&view.id)
                && *effective != view.id
            {
                self.env.views.insert(effective.clone(), index);
            }
            if is_space {
                self.space_roots.push(index);
            }
            if holder {
                self.add_rows(index);
            }
            let children = self.children_of(&view, unlisted);
            for child in children.into_iter().rev() {
                stack.push((child, Some(index)));
            }
        }
    }

    /// The kind and the source path of the page of a view; a database view that is not the first
    /// of its database becomes a linked view.
    fn kind_of(&mut self, view: &View) -> (Kind, String) {
        let tree = self.a.tree_path();
        if view.is_space {
            return (Kind::Space, tree);
        }
        if view.is_database() {
            let effective = self
                .db_of_view
                .get(&view.id)
                .map_or_else(|| view.id.clone(), |(_, id)| id.clone());
            let wanted = self
                .a
                .orig(&format!("collab_jsons/databases/{effective}.json"));
            let Some((db_index, _)) = self.db_of_view.get(&view.id).cloned() else {
                // The file is not there, or it is not a database.
                let kind = if self.a.database(&view.id).is_some() {
                    Kind::Unreadable
                } else {
                    Kind::Missing
                };
                return (kind, wanted);
            };
            let path = wanted;
            let key = if self.dbs[db_index].id.is_empty() {
                view.database_id.clone().unwrap_or_else(|| view.id.clone())
            } else {
                self.dbs[db_index].id.clone()
            };
            // The first view of a database in tree order holds its rows; its page is the next
            // one added.
            return match self.held.get(&key) {
                Some(holder) => (Kind::Linked(*holder), path),
                None => {
                    let here = self.nodes.len();
                    self.held.insert(key.clone(), here);
                    self.env.databases.insert(key, here);
                    if let Some(named) = &view.database_id {
                        self.env.databases.entry(named.clone()).or_insert(here);
                    }
                    // The layout of the view table: its numbering is the one that counts (the
                    // database file counts its layouts from 0), the file's only fills in.
                    let layout = self
                        .a
                        .views
                        .get(&effective)
                        .map_or(view.layout, |v| v.layout);
                    let layout = if (1..=3).contains(&layout) {
                        layout
                    } else {
                        self.dbs[db_index].layout_hint.unwrap_or(layout)
                    };
                    (Kind::Database(db_index, layout), path)
                }
            };
        }
        let document = if self.a.has_document(&view.object) {
            Some(view.object.clone())
        } else if self.a.has_document(&view.id) {
            Some(view.id.clone())
        } else {
            None
        };
        match document {
            Some(id) => {
                let path = self.a.document_path(&id);
                (Kind::Document(id), path)
            }
            None => {
                let path = self.a.document_path(&view.object);
                if view.layout == 0 {
                    (Kind::Missing, path)
                } else {
                    (Kind::Unsupported(view.layout), path)
                }
            }
        }
    }

    /// The row pages of a database page, in the view's order.
    fn add_rows(&mut self, holder: usize) {
        let Kind::Database(db_index, _) = self.nodes[holder].kind else {
            return;
        };
        let path = self.nodes[holder].path.clone();
        for row_index in 0..self.dbs[db_index].rows.len() {
            let db = &self.dbs[db_index];
            let row = &db.rows[row_index];
            let title = clean_title(&db.title(row));
            let icon = match db.metas.get(&row.id).and_then(|m| m.icon.clone()) {
                Some(icon) if is_emoji(&icon) => Some(icon.trim().to_owned()),
                Some(_) => {
                    let tree = self.a.tree_path();
                    self.env
                        .warn(&tree, ImportWarningCode::UnsupportedContent, Some("icon"));
                    None
                }
                None => None,
            };
            let row_id = row.id.clone();
            let Some(index) = self.add(Node {
                parent: Some(holder),
                title,
                icon,
                path: path.clone(),
                kind: Kind::Row(db_index, row_index),
            }) else {
                return;
            };
            self.env.rows.insert(row_id.clone(), index);
            if let Some(doc) = self.row_doc_of.get(&row_id) {
                self.env.rows.entry(doc.clone()).or_insert(index);
            }
        }
    }

    fn truncation_warning(&mut self, other: bool) {
        let rows: usize = {
            let mut seen: HashSet<&str> = HashSet::new();
            self.dbs
                .iter()
                .filter(|db| db.id.is_empty() || seen.insert(db.id.as_str()))
                .map(|db| db.rows.len())
                .sum()
        };
        let pages = self.a.views.values().filter(|v| !self.is_hidden(v)).count();
        let total = pages + rows + usize::from(other);
        if total > self.nodes.len() && self.nodes.len() >= self.max_pages {
            let tree = self.a.tree_path();
            self.env.warn(
                &tree,
                ImportWarningCode::Truncated,
                Some(&format!("{} pages left out", total - self.nodes.len())),
            );
        }
    }

    // ----- 3. properties ---------------------------------------------------------------------

    /// The planned properties of every database that holds rows, with how each field maps to
    /// them.
    fn properties(&mut self) -> (Props, FieldPlan) {
        let mut props = Props::default();
        let mut plan = FieldPlan::default();
        let holders: Vec<usize> = self
            .nodes
            .iter()
            .filter_map(|n| match n.kind {
                Kind::Database(db, _) => Some(db),
                _ => None,
            })
            .collect();
        for db_index in holders {
            let db = &self.dbs[db_index];
            for field in db.property_fields() {
                let position = db.fields.iter().position(|f| f.id == field.id).unwrap_or(0);
                let name = property_name(field, position);
                let relation = field.ty == 10 && db.relation_complete(field, &self.env);
                let kind = if field.ty == 10 && !relation {
                    PropertyKind::Text
                } else {
                    property_kind(field.ty).unwrap_or(PropertyKind::Text)
                };
                let options = if kind.has_options() {
                    field.options.as_slice()
                } else {
                    &[]
                };
                props.observe(&name, kind, options);
                let planned = props.canonical(&name).unwrap_or(&name).to_owned();
                plan.names.insert((db_index, field.id.clone()), planned);
                if relation {
                    plan.relations.insert((db_index, field.id.clone()));
                }
            }
        }
        (props, plan)
    }

    // ----- 4. content and values -------------------------------------------------------------

    fn convert(&mut self, fields: &FieldPlan) -> Vec<PlannedPage> {
        let mut pages = Vec::with_capacity(self.nodes.len());
        for index in 0..self.nodes.len() {
            let (blocks, properties, path) = self.convert_node(index, fields);
            let node = &self.nodes[index];
            let blocks = if blocks.is_empty() {
                vec![pj::paragraph(Vec::new())]
            } else {
                blocks
            };
            pages.push(PlannedPage {
                parent: node.parent,
                title: node.title.clone(),
                icon: node.icon.clone(),
                cover: self.covers.get(&index).cloned(),
                cover_y: None,
                template: false,
                template_placeholders: Vec::new(),
                source_path: path,
                content: json!({"type": "doc", "content": blocks}),
                properties,
            });
        }
        pages
    }

    fn convert_node(
        &mut self,
        index: usize,
        fields: &FieldPlan,
    ) -> (Vec<Value>, Vec<(String, ImportValue)>, String) {
        let path = self.nodes[index].path.clone();
        let name = self.nodes[index].title.clone();
        match self.nodes[index].kind {
            Kind::Space | Kind::Other => (Vec::new(), Vec::new(), path),
            Kind::Missing => {
                self.env
                    .warn(&path, ImportWarningCode::MissingFile, Some(&name));
                (Vec::new(), Vec::new(), path)
            }
            Kind::Unreadable => {
                self.env.warn(
                    &path,
                    ImportWarningCode::UnsupportedContent,
                    Some("database"),
                );
                (Vec::new(), Vec::new(), path)
            }
            Kind::Unsupported(layout) => {
                self.env.warn(
                    &path,
                    ImportWarningCode::UnsupportedContent,
                    Some(&format!("layout {layout}")),
                );
                (Vec::new(), Vec::new(), path)
            }
            Kind::Document(_) => {
                let blocks = self.document_blocks(index);
                (blocks, Vec::new(), path)
            }
            Kind::Database(db_index, layout) => {
                let mut by_field: HashMap<String, String> = HashMap::new();
                for ((db, field), name) in &fields.names {
                    if *db == db_index {
                        by_field.insert(field.clone(), name.clone());
                    }
                }
                let view = page_list(&self.dbs[db_index], layout, &by_field, &mut self.env, &path);
                (vec![view], Vec::new(), path)
            }
            Kind::Linked(holder) => {
                self.env.warn(
                    &path,
                    ImportWarningCode::UnsupportedContent,
                    Some("linked view"),
                );
                let title = self.env.title(holder).to_owned();
                let card = placeholder::page_card(&Target::Planned(holder), &title);
                (vec![card], Vec::new(), path)
            }
            Kind::Row(db_index, row_index) => self.convert_row(index, db_index, row_index, fields),
        }
    }

    /// The blocks of the document of a page.
    fn document_blocks(&mut self, index: usize) -> Vec<Value> {
        let Kind::Document(id) = &self.nodes[index].kind else {
            return Vec::new();
        };
        let id = id.clone();
        let path = self.nodes[index].path.clone();
        self.blocks_of_document(&id, &path)
    }

    fn blocks_of_document(&mut self, id: &str, path: &str) -> Vec<Value> {
        let Some((text, _)) = self.a.document(id) else {
            return Vec::new();
        };
        match DocData::parse(text) {
            Some(doc) => blocks::convert(&doc, &mut self.env, path),
            None => {
                self.env.warn(
                    path,
                    ImportWarningCode::UnsupportedContent,
                    Some("document"),
                );
                Vec::new()
            }
        }
    }

    fn convert_row(
        &mut self,
        index: usize,
        db_index: usize,
        row_index: usize,
        fields: &FieldPlan,
    ) -> (Vec<Value>, Vec<(String, ImportValue)>, String) {
        let mut path = self.nodes[index].path.clone();
        let db = &self.dbs[db_index];
        let row = &db.rows[row_index];

        // The values, by the properties of the database's fields.
        let mut values: Vec<(String, ImportValue)> = Vec::new();
        for field in db.property_fields() {
            let Some(name) = fields.names.get(&(db_index, field.id.clone())) else {
                continue;
            };
            let Some(cell) = row.cells.get(&field.id) else {
                continue;
            };
            let relation = fields.relations.contains(&(db_index, field.id.clone()));
            let Some(value) = cell_value(field, cell, relation, &mut self.env, &path) else {
                continue;
            };
            let value = match (fields.kinds.get(&name.to_lowercase()), &value) {
                (Some(PropertyKind::Text), ImportValue::Text(_)) | (None, _) => value,
                (Some(PropertyKind::Text), other) => {
                    ImportValue::Text(value_text(other, &self.env))
                }
                _ => value,
            };
            match values
                .iter_mut()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
            {
                Some(existing) => existing.1 = value,
                None => values.push((name.clone(), value)),
            }
        }

        // The content: the row's document, unless the row says it is empty.
        let empty = db.metas.get(&row.id).is_some_and(|m| m.empty_document);
        let document = self.row_doc_of.get(&row.id).cloned();
        let mut blocks = Vec::new();
        if let Some(id) = document
            && !empty
            && self.a.has_document(&id)
        {
            path = self.a.document_path(&id);
            blocks = self.blocks_of_document(&id, &path);
        }
        (blocks, values, path)
    }
}
