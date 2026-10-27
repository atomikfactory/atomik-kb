//! Tests of the AppFlowy reader: recognition, the tree and the spaces, links, files, warnings.
//! The archives are synthetic (a builder below); no real archive is read.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::ImportWarningCode;
use serde_json::{Map, Value, json};

use super::archive::row_document_id;
use crate::{ImportOptions, Plan, PlannedCover, SourceFile, plan};

/// The id of the workspace: the parent of every space.
pub(super) const WS: &str = "ws-0001";

/// An AppFlowy document under construction: a block map, a children map and a text map.
pub(super) struct Doc {
    blocks: Map<String, Value>,
    children: Map<String, Value>,
    text: Map<String, Value>,
    n: usize,
}

impl Doc {
    pub(super) fn new() -> Self {
        let mut doc = Self {
            blocks: Map::new(),
            children: Map::new(),
            text: Map::new(),
            n: 0,
        };
        doc.blocks.insert(
            "root".into(),
            json!({"id": "root", "ty": "page", "parent": "", "children": "ch-root",
                   "external_id": null, "external_type": null, "data": {}}),
        );
        doc.children.insert("ch-root".into(), json!([]));
        doc
    }

    /// Adds a block under `parent`; its text is `delta` (kept in the text map) when given.
    pub(super) fn add(
        &mut self,
        parent: &str,
        ty: &str,
        data: Value,
        delta: Option<Value>,
    ) -> String {
        self.n += 1;
        let id = format!("b{}", self.n);
        let (external_id, external_type) = match delta {
            Some(delta) => {
                let key = format!("tx-{id}");
                self.text
                    .insert(key.clone(), Value::String(delta.to_string()));
                (Value::String(key), Value::String("text".into()))
            }
            None => (Value::Null, Value::Null),
        };
        self.blocks.insert(
            id.clone(),
            json!({"id": id, "ty": ty, "parent": parent, "children": format!("ch-{id}"),
                   "external_id": external_id, "external_type": external_type, "data": data}),
        );
        self.children.insert(format!("ch-{id}"), json!([]));
        let slot = self
            .children
            .get_mut(&format!("ch-{parent}"))
            .and_then(Value::as_array_mut)
            .unwrap();
        slot.push(Value::String(id.clone()));
        id
    }

    /// A block with plain text.
    pub(super) fn text(&mut self, parent: &str, ty: &str, data: Value, text: &str) -> String {
        self.add(parent, ty, data, Some(json!([{"insert": text}])))
    }

    /// A paragraph under the root.
    pub(super) fn para(&mut self, text: &str) -> String {
        self.text("root", "paragraph", json!({}), text)
    }

    pub(super) fn json(&self) -> String {
        json!({"page_id": "root", "blocks": self.blocks,
               "meta": {"children_map": self.children, "text_map": self.text}})
        .to_string()
    }
}

/// An archive under construction.
pub(super) struct Backup {
    views: Map<String, Value>,
    texts: Vec<SourceFile>,
    pub(super) binaries: Vec<String>,
    pub(super) prefix: String,
    clock: i64,
}

impl Backup {
    pub(super) fn new() -> Self {
        Self {
            views: Map::new(),
            texts: Vec::new(),
            binaries: Vec::new(),
            prefix: String::new(),
            clock: 1_700_000_000,
        }
    }

    /// A view; it is listed in its parent's children unless the parent is the workspace.
    pub(super) fn view(
        &mut self,
        id: &str,
        name: &str,
        layout: i64,
        parent: &str,
        extra: Option<Value>,
        icon: Option<(i64, &str)>,
    ) {
        self.clock += 10;
        let view = json!({
            "view_id": id, "name": name, "layout": layout, "parent_id": parent, "children": [],
            "collab_object_id": id, "created_at": self.clock, "updated_at": self.clock,
            "extra": extra.map(|e| e.to_string()),
            "icon": icon.map(|(ty, value)| json!({"ty": ty, "value": value})),
        });
        self.views.insert(id.to_owned(), view);
        self.list(parent, id);
    }

    /// Lists `child` among the children of `parent`.
    pub(super) fn list(&mut self, parent: &str, child: &str) {
        if let Some(Value::Array(list)) = self
            .views
            .get_mut(parent)
            .and_then(|v| v.get_mut("children"))
        {
            list.push(Value::String(child.to_owned()));
        }
    }

    /// Removes `child` from the children of its parent (a view the parent forgot to list).
    pub(super) fn unlist(&mut self, parent: &str, child: &str) {
        if let Some(Value::Array(list)) = self
            .views
            .get_mut(parent)
            .and_then(|v| v.get_mut("children"))
        {
            list.retain(|c| c != child);
        }
    }

    pub(super) fn space(&mut self, id: &str, name: &str) {
        let created = self.clock * 1000 + 11_000;
        self.view(
            id,
            name,
            0,
            WS,
            Some(json!({"is_space": true, "space_icon": "space_icon_3", "space_icon_color": "0xFF123456",
                        "space_permission": 0, "space_created_at": created})),
            None,
        );
    }

    /// A document page with its document.
    pub(super) fn page(&mut self, id: &str, name: &str, parent: &str, doc: &Doc) {
        self.view(id, name, 0, parent, None, None);
        self.document(id, &doc.json());
    }

    /// The file `collab_jsons/documents/<id>.json`.
    pub(super) fn document(&mut self, id: &str, text: &str) {
        self.text_file(&format!("collab_jsons/documents/{id}.json"), text);
    }

    /// A text file at a path under the archive's root.
    pub(super) fn text_file(&mut self, path: &str, text: &str) {
        self.texts.push(SourceFile {
            path: format!("{}{path}", self.prefix),
            text: text.to_owned(),
        });
    }

    /// A database view of `layout` (1 grid, 2 board, 3 calendar) with its file.
    pub(super) fn database(&mut self, id: &str, name: &str, layout: i64, parent: &str, db: &Value) {
        let database_id = match &db["database_id"] {
            Value::String(id) => id.clone(),
            other => other.to_string(),
        };
        self.view(
            id,
            name,
            layout,
            parent,
            Some(json!({"database_id": database_id})),
            None,
        );
        self.text_file(
            &format!("collab_jsons/databases/{id}.json"),
            &db.to_string(),
        );
    }

    /// The view that AppFlowy makes for the document of a row: at the workspace level, named
    /// "Untitled", with the derived id.
    pub(super) fn row_document(&mut self, row_id: &str, doc: Option<&Doc>) {
        let id = row_document_id(row_id).unwrap();
        self.view(&id, "Untitled", 0, WS, Some(json!({"cover": null})), None);
        if let Some(doc) = doc {
            self.document(&id, &doc.json());
        }
    }

    /// A file of `local_files`.
    pub(super) fn local_file(&mut self, name: &str) -> String {
        let path = format!("{}local_files/{name}", self.prefix);
        self.binaries.push(path.clone());
        path
    }

    /// The source files and the binary paths, with the metadata and the view table added.
    pub(super) fn build(mut self) -> (Vec<SourceFile>, Vec<String>) {
        let metadata = json!({"export_info": {"workspace_id": WS, "total_views": self.views.len(),
                                              "version": "1.0"}});
        let relation = json!({"workspace_id": WS, "views": self.views, "collab_objects": {},
                              "dependencies": [], "workspace_database_meta": []});
        self.text_file("metadata.json", &metadata.to_string());
        self.text_file("relation_map.json", &relation.to_string());
        (self.texts, self.binaries)
    }

    pub(super) fn plan(self) -> Plan {
        self.plan_with(10_000)
    }

    pub(super) fn plan_with(self, max_pages: usize) -> Plan {
        self.plan_as(None, max_pages)
    }

    /// Planned as a source of that name (the zip's stem).
    pub(super) fn plan_named(self, name: Option<&str>) -> Plan {
        self.plan_as(name, 10_000)
    }

    fn plan_as(self, name: Option<&str>, max_pages: usize) -> Plan {
        let (texts, binaries) = self.build();
        plan(
            texts,
            &binaries,
            &ImportOptions {
                source_name: name.map(str::to_owned),
                max_pages,
            },
        )
    }
}

// ----- helpers for assertions ------------------------------------------------------------------

pub(super) fn titles(plan: &Plan) -> Vec<&str> {
    plan.pages.iter().map(|p| p.title.as_str()).collect()
}

pub(super) fn blocks(plan: &Plan, page: usize) -> &Vec<Value> {
    plan.pages[page].content["content"].as_array().unwrap()
}

pub(super) fn index_of(plan: &Plan, title: &str) -> usize {
    plan.pages
        .iter()
        .position(|p| p.title == title)
        .unwrap_or_else(|| panic!("no page titled {title}: {:?}", titles(plan)))
}

pub(super) fn codes(plan: &Plan) -> Vec<ImportWarningCode> {
    plan.warnings.iter().map(|w| w.code).collect()
}

pub(super) fn warnings_of(plan: &Plan, code: ImportWarningCode) -> Vec<Option<&str>> {
    plan.warnings
        .iter()
        .filter(|w| w.code == code)
        .map(|w| w.detail.as_deref())
        .collect()
}

// ----- recognition -----------------------------------------------------------------------------

#[test]
fn a_backup_is_recognised_and_anything_else_is_not() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    let first = backup.plan();
    assert_eq!(first.space_roots, [0]);

    // Without the metadata (or with a view table that has no views) it is an ordinary source.
    let (texts, binaries) = {
        let mut backup = Backup::new();
        backup.space("sp1", "Space");
        backup.build()
    };
    let without_metadata: Vec<SourceFile> = texts
        .iter()
        .filter(|f| f.path != "metadata.json")
        .cloned()
        .collect();
    let options = ImportOptions {
        source_name: None,
        max_pages: 100,
    };
    let plan1 = plan(without_metadata, &binaries, &options);
    assert!(plan1.space_roots.is_empty());
    let no_views: Vec<SourceFile> = texts
        .iter()
        .map(|f| {
            if f.path == "relation_map.json" {
                SourceFile {
                    path: f.path.clone(),
                    text: r#"{"views": 3}"#.into(),
                }
            } else {
                f.clone()
            }
        })
        .collect();
    assert!(plan(no_views, &binaries, &options).space_roots.is_empty());
    // A Markdown source is untouched.
    let md = plan(
        vec![SourceFile {
            path: "Notes.md".into(),
            text: "Hello".into(),
        }],
        &[],
        &options,
    );
    assert_eq!(md.pages.len(), 1);
    assert!(md.space_roots.is_empty());
    assert!(md.remote_files.is_empty());
    assert!(md.needed_file_names.is_empty());
}

#[test]
fn one_wrapping_folder_is_stripped_and_kept_in_the_paths_as_given() {
    let mut backup = Backup::new();
    backup.prefix = "AppFlowy backup/".into();
    backup.space("sp1", "Space");
    let mut doc = Doc::new();
    doc.para("Hello");
    backup.page("p1", "Page", "sp1", &doc);
    let file = backup.local_file("abc.pdf");
    let plan = backup.plan();
    assert_eq!(plan.space_roots, [0]);
    assert_eq!(titles(&plan), ["Space", "Page"]);
    assert_eq!(
        plan.pages[1].source_path,
        "AppFlowy backup/collab_jsons/documents/p1.json"
    );
    // Binary paths keep the prefix they were given.
    assert_eq!(file, "AppFlowy backup/local_files/abc.pdf");
}

// ----- the tree --------------------------------------------------------------------------------

#[test]
fn spaces_are_roots_and_children_keep_their_order() {
    let mut backup = Backup::new();
    backup.space("sp1", "Work");
    backup.space("sp2", "Home");
    backup.page("a", "Zebra", "sp1", &Doc::new());
    backup.page("b", "Apple", "sp1", &Doc::new());
    backup.page("b1", "Apple child", "b", &Doc::new());
    backup.page("c", "Mango", "sp2", &Doc::new());
    let plan = backup.plan();
    // Children are in the order the view lists them, not alphabetical; a parent comes first.
    assert_eq!(
        titles(&plan),
        ["Work", "Zebra", "Apple", "Apple child", "Home", "Mango"]
    );
    assert_eq!(plan.space_roots, [0, 4]);
    let parents: Vec<Option<usize>> = plan.pages.iter().map(|p| p.parent).collect();
    assert_eq!(parents, [None, Some(0), Some(0), Some(2), None, Some(4)]);
    // A space root is an empty page.
    assert_eq!(blocks(&plan, 0), &vec![json!({"type": "paragraph"})]);
    // The only thing reported: the spaces' icons are names of AppFlowy's own set.
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent),
        [Some("icon")]
    );
    assert_eq!(plan.warnings.len(), 1);
}

#[test]
fn views_outside_a_space_go_to_other_pages_and_row_documents_are_not_pages() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("loose", "Loose page", WS, &Doc::new());
    backup.page("child", "Loose child", "loose", &Doc::new());
    // A view whose parent is unknown.
    backup.page("lost", "Lost page", "nowhere", &Doc::new());
    let row = "11111111-2222-4333-8444-555555555555";
    backup.row_document(row, Some(&Doc::new()));
    let db = json!({"database_id": "db1", "views": [], "fields": [], "rows": [{"id": row, "cells": {}}],
                    "row_metas": {}});
    backup.database("dbv", "Tasks", 1, "sp1", &db);
    let plan = backup.plan();
    assert_eq!(
        titles(&plan),
        [
            "Space",
            "Tasks",
            "Untitled",
            "Other pages",
            "Loose page",
            "Loose child",
            "Lost page"
        ]
    );
    // The "Untitled" is the row, a child of the database; the row document view is not planned.
    let other = index_of(&plan, "Other pages");
    assert_eq!(plan.space_roots, [0, other]);
    assert_eq!(plan.pages[other].parent, None);
    assert_eq!(
        plan.pages[index_of(&plan, "Loose page")].parent,
        Some(other)
    );
    assert_eq!(plan.pages[index_of(&plan, "Lost page")].parent, Some(other));
    assert_eq!(
        plan.pages[index_of(&plan, "Untitled")].parent,
        Some(index_of(&plan, "Tasks"))
    );
    assert_eq!(
        plan.pages.iter().filter(|p| p.title == "Untitled").count(),
        1
    );
}

#[test]
fn no_other_pages_when_everything_is_in_a_space() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("a", "A", "sp1", &Doc::new());
    let plan = backup.plan();
    assert!(!titles(&plan).contains(&"Other pages"));
    assert_eq!(plan.space_roots.len(), 1);
}

#[test]
fn an_unlisted_child_follows_the_listed_ones_a_view_listed_twice_is_planned_once() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("a", "A", "sp1", &Doc::new());
    backup.page("b", "B", "sp1", &Doc::new());
    backup.page("c", "C", "sp1", &Doc::new());
    backup.unlist("sp1", "b");
    backup.list("sp1", "a");
    backup.list("a", "c");
    let plan = backup.plan();
    // `b` names `sp1` as its parent but is not listed: it follows. `a` is listed twice. `c` is
    // listed under `sp1` and under `a`: planned once, where the walk of the tree meets it first.
    assert_eq!(titles(&plan), ["Space", "A", "C", "B"]);
    assert_eq!(plan.pages[2].parent, Some(1));
    assert_eq!(plan.pages[3].parent, Some(0));
}

#[test]
fn a_cycle_is_cut() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("a", "A", "sp1", &Doc::new());
    backup.page("b", "B", "a", &Doc::new());
    // `a` is also a child of `b`.
    backup.list("b", "a");
    backup.list("b", "sp1");
    // A cycle with no way in: x lists y, y lists x.
    backup.page("x", "X", "y", &Doc::new());
    backup.page("y", "Y", "x", &Doc::new());
    let plan = backup.plan();
    assert_eq!(plan.pages.iter().filter(|p| p.title == "A").count(), 1);
    assert_eq!(plan.pages.iter().filter(|p| p.title == "Space").count(), 1);
    // What the cycle kept out of reach is still imported, under "Other pages".
    let other = index_of(&plan, "Other pages");
    let x = index_of(&plan, "X");
    let y = index_of(&plan, "Y");
    assert!(
        plan.pages[x].parent == Some(other) || plan.pages[y].parent == Some(other),
        "{:?}",
        plan.pages.iter().map(|p| p.parent).collect::<Vec<_>>()
    );
}

#[test]
fn titles_and_icons() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.view("e", "", 0, "sp1", None, Some((0, "📝")));
    backup.view(
        "w",
        "  two\n  lines\t ",
        0,
        "sp1",
        None,
        Some((1, "icon_name")),
    );
    backup.view(
        "l",
        &"x".repeat(900),
        0,
        "sp1",
        None,
        Some((2, "https://example.com/i.png")),
    );
    backup.view(
        "c",
        "Cover",
        0,
        "sp1",
        Some(json!({"cover": {"type": 1, "value": "x"}})),
        None,
    );
    backup.view(
        "n",
        "No cover",
        0,
        "sp1",
        Some(json!({"cover": null})),
        None,
    );
    backup.view(
        "c2",
        "Cover 2",
        0,
        "sp1",
        Some(json!({"cover": {"type": 1, "value": "y"}})),
        None,
    );
    let plan = backup.plan();
    assert_eq!(plan.pages[1].title, "Untitled");
    assert_eq!(plan.pages[1].icon.as_deref(), Some("📝"));
    assert_eq!(plan.pages[2].title, "two lines");
    assert_eq!(plan.pages[2].icon, None);
    assert_eq!(plan.pages[3].title.chars().count(), 500);
    // Icons that are not emoji and covers are reported once per import.
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent),
        [Some("icon"), Some("cover")]
    );
    // The space's own icon is a name of AppFlowy's set: dropped too (already reported).
    assert_eq!(plan.pages[0].icon, None);
}

#[test]
fn covers_that_are_colours_or_pictures_become_the_pages_cover() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    let file = backup.local_file("abc123.png");
    let covered = |backup: &mut Backup, id: &str, name: &str, cover: Value| {
        backup.view(id, name, 0, "sp1", Some(json!({ "cover": cover })), None);
    };
    // A colour, by number and by name, `0xAARRGGBB` and `#RRGGBB`: the nearest of the nine.
    covered(
        &mut backup,
        "c1",
        "Blue",
        json!({"type": 1, "value": "0xFF3B82F6"}),
    );
    covered(
        &mut backup,
        "c2",
        "Red",
        json!({"type": "CoverType.color", "value": "#E5484D"}),
    );
    covered(
        &mut backup,
        "c3",
        "Green",
        json!({"type": "color", "value": "0xff2e8b57"}),
    );
    // A file of the archive, by the name `local_files` holds it under, with or without the folder,
    // and the object form held in a string as AppFlowy writes `extra`.
    covered(
        &mut backup,
        "f1",
        "Picture",
        json!({"type": 2, "value": "abc123.png"}),
    );
    covered(
        &mut backup,
        "f2",
        "Picture again",
        json!({"type": "CoverType.file", "value": "local_files/abc123.png"}),
    );
    // A picture on a server is a remote file, the same one for the same URL.
    covered(
        &mut backup,
        "r1",
        "Remote",
        json!({"type": 2, "value": "https://example.com/banner.jpg"}),
    );
    covered(
        &mut backup,
        "r2",
        "Remote again",
        json!({"type": "CoverType.file", "value": "https://example.com/banner.jpg"}),
    );
    backup.view("n", "Plain", 0, "sp1", Some(json!({"cover": null})), None);
    let plan = backup.plan();

    let cover = |title: &str| plan.pages[index_of(&plan, title)].cover.clone();
    assert_eq!(
        cover("Blue"),
        Some(PlannedCover::Named("color:blue".into()))
    );
    assert_eq!(cover("Red"), Some(PlannedCover::Named("color:red".into())));
    assert_eq!(
        cover("Green"),
        Some(PlannedCover::Named("color:green".into()))
    );
    assert_eq!(cover("Picture"), Some(PlannedCover::File(file.clone())));
    assert_eq!(
        cover("Picture again"),
        Some(PlannedCover::File(file.clone()))
    );
    assert_eq!(cover("Remote"), Some(PlannedCover::File("remote/0".into())));
    assert_eq!(
        cover("Remote again"),
        Some(PlannedCover::File("remote/0".into()))
    );
    assert_eq!(cover("Plain"), None);
    assert_eq!(plan.pages[index_of(&plan, "Blue")].cover_y, None);
    // The files are needed (once) and the page says which one is its cover.
    assert_eq!(plan.needed_files, std::slice::from_ref(&file));
    assert_eq!(plan.remote_files.len(), 1);
    assert_eq!(plan.remote_files[0].url, "https://example.com/banner.jpg");
    assert_eq!(
        plan.pages[index_of(&plan, "Picture")].cover_file(),
        Some(file.as_str())
    );
    assert_eq!(plan.pages[index_of(&plan, "Blue")].cover_file(), None);
    // Nothing to warn about the covers (the space's own icon is another warning).
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent),
        [Some("icon")]
    );
}

#[test]
fn covers_that_cannot_be_kept_are_reported_once_and_leave_no_cover() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    for (id, name, cover) in [
        // AppFlowy's own pictures and gradients, unknown types, a white colour, a missing file,
        // a path of the machine that made the backup, an `http` picture.
        (
            "a",
            "Built in",
            json!({"type": 3, "value": "assets/images/cover_1.jpg"}),
        ),
        (
            "b",
            "Gradient",
            json!({"type": "gradient", "value": "gradient_2"}),
        ),
        ("c", "Unknown", json!({"type": 9, "value": "x"})),
        ("d", "White", json!({"type": 1, "value": "0xFFFFFFFF"})),
        (
            "e",
            "Not in the archive",
            json!({"type": 2, "value": "gone.png"}),
        ),
        (
            "f",
            "Local path",
            json!({"type": 2, "value": r"C:\Users\me\a.png"}),
        ),
        (
            "g",
            "Plain http",
            json!({"type": 2, "value": "http://example.com/a.png"}),
        ),
        ("h", "No value", json!({"type": 1})),
        ("i", "Not an object", json!(5)),
    ] {
        backup.view(id, name, 0, "sp1", Some(json!({ "cover": cover })), None);
    }
    let plan = backup.plan();
    for page in &plan.pages {
        assert_eq!(page.cover, None, "{}", page.title);
    }
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent),
        [Some("icon"), Some("cover")],
        "the space's icon, then the covers, once per import"
    );
    assert!(plan.needed_files.is_empty() && plan.remote_files.is_empty());
}

#[test]
fn a_space_with_an_emoji_icon_keeps_it() {
    let mut backup = Backup::new();
    backup.view(
        "sp",
        "Space",
        0,
        WS,
        Some(json!({"is_space": true, "space_icon": "🚀"})),
        None,
    );
    let plan = backup.plan();
    assert_eq!(plan.pages[0].icon.as_deref(), Some("🚀"));
}

#[test]
fn a_missing_document_is_an_empty_page_with_a_warning() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.view("gone", "Gone page", 0, "sp1", None, None);
    backup.view("odd", "Odd layout", 9, "sp1", None, None);
    backup.view(
        "dbgone",
        "No data",
        1,
        "sp1",
        Some(json!({"database_id": "d"})),
        None,
    );
    let plan = backup.plan();
    assert_eq!(
        titles(&plan),
        ["Space", "Gone page", "Odd layout", "No data"]
    );
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::MissingFile),
        [Some("Gone page"), Some("No data")]
    );
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent),
        [Some("icon"), Some("layout 9")]
    );
    assert_eq!(blocks(&plan, 1), &vec![json!({"type": "paragraph"})]);
}

#[test]
fn the_page_limit_cuts_the_plan_with_one_warning() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    for n in 0..20 {
        backup.page(&format!("p{n}"), &format!("Page {n}"), "sp1", &Doc::new());
    }
    let plan = backup.plan_with(5);
    assert_eq!(plan.pages.len(), 5);
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::Truncated),
        [Some("16 pages left out")]
    );
}

// ----- links -----------------------------------------------------------------------------------

#[test]
fn mentions_and_page_links_point_at_planned_pages() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("target", "Target page", "sp1", &Doc::new());
    let row = "11111111-2222-4333-8444-555555555555";
    let db = json!({"database_id": "db1", "views": [],
        "fields": [{"id": "f1", "name": "Name", "field_type": 0, "is_primary": true, "type_options": {}}],
        "rows": [{"id": row, "cells": {"f1": {"field_type": 0, "data": "Row one"}}}], "row_metas": {}});
    backup.database("dbv", "Tasks", 1, "sp1", &db);
    backup.row_document(row, None);
    let row_doc = row_document_id(row).unwrap();

    let mut doc = Doc::new();
    doc.add(
        "root",
        "paragraph",
        json!({}),
        Some(json!([
            {"insert": "See "},
            {"insert": "$", "attributes": {"mention": {"type": "page", "page_id": "target", "block_id": "zz"}}},
            {"insert": " and "},
            {"insert": "link", "attributes": {"href": "target", "is_page_link": true}},
            {"insert": " and "},
            {"insert": "$", "attributes": {"mention": {"type": "page", "page_id": row_doc}}},
            {"insert": " and "},
            {"insert": "$", "attributes": {"mention": {"type": "page", "page_id": "ghost"}}},
            {"insert": " and "},
            {"insert": "dead", "attributes": {"href": "ghost2", "is_page_link": true}},
        ])),
    );
    doc.add("root", "sub_page", json!({"view_id": "target"}), None);
    doc.add(
        "root",
        "grid",
        json!({"view_id": "dbv", "parent_id": "p"}),
        None,
    );
    doc.add("root", "sub_page", json!({"view_id": "nope"}), None);
    backup.page("p", "Source", "sp1", &doc);
    let plan = backup.plan();
    let source = index_of(&plan, "Source");
    let target = index_of(&plan, "Target page");
    let row_page = index_of(&plan, "Row one");
    let tasks = index_of(&plan, "Tasks");
    let content = blocks(&plan, source);
    let inline = content[0]["content"].as_array().unwrap();
    let refs: Vec<usize> = inline
        .iter()
        .filter(|n| n["type"] == "pageRef")
        .map(|n| n["attrs"]["$page"]["planned"].as_u64().unwrap() as usize)
        .collect();
    // The mention (block id dropped), the page href, the row document id.
    assert_eq!(refs, [target, target, row_page]);
    // The unknown ones are their text; the label of a reference is the title of its target.
    let texts: Vec<&str> = inline.iter().filter_map(|n| n["text"].as_str()).collect();
    assert!(texts.contains(&"Unknown page"), "{texts:?}");
    assert!(texts.contains(&"dead"), "{texts:?}");
    assert_eq!(inline[1]["attrs"]["$label"], "Target page");
    // Blocks: sub_page and the inline grid are cards; the unknown view is a paragraph.
    assert_eq!(content[1]["type"], "pageLink");
    assert_eq!(content[1]["attrs"]["$page"]["planned"], target);
    assert_eq!(content[2]["type"], "pageLink");
    assert_eq!(content[2]["attrs"]["$page"]["planned"], tasks);
    assert_eq!(content[3]["type"], "paragraph");
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnresolvedLink),
        [Some("ghost"), Some("ghost2"), Some("nope")]
    );
}

// ----- files -----------------------------------------------------------------------------------

#[test]
fn local_files_are_needed_files_remote_images_are_remote_files() {
    let mut backup = Backup::new();
    backup.prefix = "Backup/".into();
    backup.space("sp1", "Space");
    let pdf = backup.local_file("aaaa.pdf");
    let png = backup.local_file("bbbb.png");
    backup.local_file("unused.7z");
    let mut doc = Doc::new();
    doc.add(
        "root",
        "file",
        json!({"url": "p1/aaaa.pdf", "url_type": 2, "name": "Report.pdf", "uploaded_at": 1}),
        None,
    );
    doc.add(
        "root",
        "file",
        json!({"url": "p1/zzzz.pdf", "url_type": 2, "name": "Lost.pdf"}),
        None,
    );
    doc.add(
        "root",
        "file",
        json!({"url": "https://example.com/doc.pdf", "url_type": 1, "name": "Remote"}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "bbbb.png", "align": "left", "width": 300}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "https://img.example.com/1.png", "align": "center"}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "https://img.example.com/2.png"}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "https://img.example.com/1.png"}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "http://insecure.example.com/3.png"}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "data:image/png;base64,AAAA"}),
        None,
    );
    doc.add("root", "image", json!({"url": "C:/Users/x/pic.png"}), None);
    backup.page("p1", "Files", "sp1", &doc);
    let plan = backup.plan();
    // Needed files by path as given, in order of first use; unused files are not needed.
    assert_eq!(plan.needed_files, [pdf, png.clone()]);
    // The original name of a file, from the block that names it; none for an image.
    assert_eq!(
        plan.needed_file_names,
        [Some("Report.pdf".to_owned()), None]
    );
    // One remote file per distinct https URL; keys count from 0 in plan order.
    let remote: Vec<(&str, &str, &str)> = plan
        .remote_files
        .iter()
        .map(|r| (r.key.as_str(), r.url.as_str(), r.host.as_str()))
        .collect();
    assert_eq!(
        remote,
        [
            (
                "remote/0",
                "https://img.example.com/1.png",
                "img.example.com"
            ),
            (
                "remote/1",
                "https://img.example.com/2.png",
                "img.example.com"
            ),
        ]
    );
    let content = blocks(&plan, 1);
    assert_eq!(content[0]["type"], "file");
    assert_eq!(content[0]["attrs"]["$file"], plan.needed_files[0].as_str());
    assert_eq!(content[0]["attrs"]["$label"], "Report.pdf");
    // A file that is not in the archive: its name as text. A web file: a link.
    assert_eq!(
        content[1],
        json!({"type": "paragraph", "content": [{"type": "text", "text": "Lost.pdf"}]})
    );
    assert_eq!(
        content[2]["content"][0]["marks"][0],
        json!({"type": "link", "attrs": {"href": "https://example.com/doc.pdf"}})
    );
    assert_eq!(content[3]["attrs"]["$file"], png.as_str());
    assert_eq!(content[3]["attrs"]["align"], "left");
    assert!(content[3]["attrs"].get("width").is_none());
    assert_eq!(content[4]["attrs"]["$file"], "remote/0");
    assert_eq!(content[4]["attrs"]["$url"], "https://img.example.com/1.png");
    assert_eq!(content[4]["attrs"]["align"], "center");
    assert_eq!(content[5]["attrs"]["$file"], "remote/1");
    assert_eq!(content[6]["attrs"]["$file"], "remote/0");
    // http: a link, not a remote file; data and a path outside the archive: text or nothing.
    assert_eq!(content[7]["type"], "paragraph");
    assert_eq!(content.len(), 9);
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::MissingFile),
        [Some("Lost.pdf"), Some("local path")]
    );
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::RemoteImage),
        [Some("http://insecure.example.com/3.png")]
    );
    assert_eq!(
        codes(&plan)
            .iter()
            .filter(|c| **c == ImportWarningCode::EmbeddedImage)
            .count(),
        1
    );
}

#[test]
fn remote_keys_run_across_pages_in_plan_order() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    for (n, url) in [
        "https://h.example/a.png",
        "https://h.example/b.png",
        "https://h.example/a.png",
    ]
    .into_iter()
    .enumerate()
    {
        let mut doc = Doc::new();
        doc.add("root", "image", json!({"url": url}), None);
        backup.page(&format!("p{n}"), &format!("P{n}"), "sp1", &doc);
    }
    let plan = backup.plan();
    let keys: Vec<&str> = (1..=3)
        .map(|i| blocks(&plan, i)[0]["attrs"]["$file"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["remote/0", "remote/1", "remote/0"]);
    assert_eq!(plan.remote_files.len(), 2);
}
