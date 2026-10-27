//! Tests of `resolve`: ids, fallbacks, views, and the page documents the results make.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use kb_doc::{Doc, DocKind};
use kb_model::PageDoc;
use kb_types::testing::SeededRng;
use kb_types::{AttachmentId, ImportWarning, ImportWarningCode, OptionId, PageId, PropertyId};
use serde_json::{Map, Value, json};

use crate::plan_tests::{HOME, file, notion_files, run};
use crate::{PlannedPage, Resolver, SourceFile, resolve};

/// A resolver with what a test sets up.
#[derive(Default)]
pub(crate) struct Fake {
    pub pages: Vec<PageId>,
    pub titles: HashMap<String, PageId>,
    pub existing_pages: HashSet<PageId>,
    pub attachments: HashMap<String, AttachmentId>,
    pub existing_attachments: HashSet<AttachmentId>,
    pub properties: HashMap<String, PropertyId>,
    pub options: HashMap<(String, String), OptionId>,
}

impl Resolver for Fake {
    fn page(&self, index: usize) -> Option<PageId> {
        self.pages.get(index).copied()
    }
    fn page_by_title(&self, title: &str) -> Option<PageId> {
        self.titles.get(title).copied()
    }
    fn existing_page(&self, page: PageId) -> bool {
        self.existing_pages.contains(&page)
    }
    fn attachment(&self, path: &str) -> Option<AttachmentId> {
        self.attachments.get(path).copied()
    }
    fn existing_attachment(&self, attachment: AttachmentId) -> bool {
        self.existing_attachments.contains(&attachment)
    }
    fn property(&self, name: &str) -> Option<PropertyId> {
        self.properties.get(&name.to_lowercase()).copied()
    }
    fn option(&self, property: &str, option: &str) -> Option<OptionId> {
        self.options
            .get(&(property.to_lowercase(), option.to_owned()))
            .copied()
    }
}

fn page_id(n: u8) -> PageId {
    PageId::from_random_bytes([n; 16])
}

fn attachment_id(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([n; 16])
}

/// Resolves a page, loads the result into a page document and checks it.
pub(crate) fn resolve_and_load(
    page: &PlannedPage,
    resolver: &dyn Resolver,
) -> (Value, Vec<ImportWarning>, PageDoc) {
    // Every call gets its own seed, as every import page would draw other random bytes.
    static SEED: AtomicU64 = AtomicU64::new(1);
    let rng = SeededRng::new(SEED.fetch_add(1, Ordering::SeqCst));
    let (json, warnings) = resolve(page, resolver, &rng);
    let doc = PageDoc::from_portable(Doc::new(DocKind::Page, 7), &json)
        .unwrap_or_else(|err| panic!("the result does not load: {err}\n{json:#}"));
    let problems = doc.check_invariants();
    assert!(problems.is_empty(), "invariants: {problems:#?}\n{json:#}");
    (json, warnings, doc)
}

fn planned(content: Value) -> PlannedPage {
    PlannedPage {
        parent: None,
        title: "T".into(),
        icon: None,
        cover: None,
        cover_y: None,
        template: false,
        template_placeholders: Vec::new(),
        source_path: "Source/T.md".into(),
        content: json!({"type": "doc", "content": content}),
        properties: Vec::new(),
    }
}

/// Every id of a resolved page.
fn ids(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(id) = map
                .get("attrs")
                .and_then(|a| a.get("id"))
                .and_then(Value::as_str)
            {
                out.push(id.to_owned());
            }
            for child in map.values() {
                ids(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| ids(item, out)),
        _ => {}
    }
}

#[test]
fn every_id_bearing_node_gets_a_fresh_valid_id() {
    let page = planned(json!([
        {"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "T"}]},
        {"type": "bulletList", "content": [
            {"type": "listItem", "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "a "},
                    {"type": "dateMention", "attrs": {"date": "2026-10-07", "time": "14:30", "reminder": 15}},
                    {"type": "mathInline", "attrs": {"latex": "x"}}
                ]}]}]},
        {"type": "table", "content": [{"type": "tableRow", "content": [
            {"type": "tableHeader", "attrs": {"colspan": 1, "rowspan": 1}, "content": [{"type": "paragraph"}]}]}]},
        {"type": "details", "attrs": {"open": true}, "content": [
            {"type": "detailsSummary", "content": [{"type": "text", "text": "s"}]},
            {"type": "detailsContent", "content": [{"type": "paragraph"}]}]},
        {"type": "callout", "attrs": {"icon": "💡", "color": "blue"}, "content": [{"type": "paragraph"}]},
        {"type": "columnList", "content": [
            {"type": "column", "content": [{"type": "paragraph"}]},
            {"type": "column", "content": [{"type": "paragraph"}]}]},
        {"type": "tableOfContents"},
        {"type": "breadcrumb"},
        {"type": "horizontalRule"},
        {"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "x"}]},
        {"type": "mathBlock", "content": [{"type": "text", "text": "x"}]}
    ]));
    let (json, warnings, _) = resolve_and_load(&page, &Fake::default());
    assert!(warnings.is_empty());
    let mut all = Vec::new();
    ids(&json, &mut all);
    let unique: HashSet<_> = all.iter().collect();
    assert_eq!(unique.len(), all.len(), "ids repeat");
    assert!(all.len() >= 22, "{}", all.len());
    for id in &all {
        assert!(kb_types::BlockId::from_str(id).is_ok());
    }
    // The same page resolved again gets other ids.
    let rng = SeededRng::new(43);
    let (again, _) = resolve(&page, &Fake::default(), &rng);
    let mut other = Vec::new();
    ids(&again, &mut other);
    assert!(all.iter().all(|id| !other.contains(id)));
    // Nothing placeholder-like is left.
    assert!(!serde_json::to_string(&json).unwrap().contains('$'));
}

#[test]
fn an_empty_page_has_a_block() {
    let (json, _, _) = resolve_and_load(&planned(json!([])), &Fake::default());
    assert_eq!(json["content"][0]["type"], "paragraph");
    assert!(json["content"][0]["attrs"]["id"].is_string());
}

#[test]
fn page_references_resolve_or_fall_back_to_text() {
    let mut resolver = Fake::default();
    resolver.pages = vec![page_id(1), page_id(2)];
    resolver.titles.insert("Existing note".into(), page_id(9));
    resolver.existing_pages.insert(page_id(7));
    let block = kb_types::BlockId::from_random_bytes([8; 16]);
    let page = planned(json!([
        {"type": "paragraph", "content": [
            {"type": "text", "text": "to "},
            {"type": "pageRef", "attrs": {"$page": {"planned": 1}, "$label": "x"}},
            {"type": "pageRef", "attrs": {"$page": {"title": "Existing note"}}},
            {"type": "pageRef", "attrs": {"$page": {"title": "Nobody"}}},
            {"type": "pageRef", "attrs": {"$page": {"existing": page_id(7).to_string(), "block": block.to_string()}, "$label": "label"}},
            {"type": "pageRef", "attrs": {"$page": {"existing": page_id(6).to_string()}, "$label": "gone page"}},
            {"type": "pageRef", "attrs": {"$page": {"planned": 99}}}
        ]},
        {"type": "pageLink", "attrs": {"$page": {"planned": 0}}},
        {"type": "pageLink", "attrs": {"$page": {"title": "Missing card"}}}
    ]));
    let (json, warnings, doc) = resolve_and_load(&page, &resolver);
    let inline = json["content"][0]["content"].as_array().unwrap();
    assert_eq!(inline[1]["attrs"]["pageId"], page_id(2).to_string());
    assert_eq!(inline[2]["attrs"]["pageId"], page_id(9).to_string());
    assert_eq!(inline[3]["text"], "[[Nobody]]");
    assert_eq!(inline[4]["attrs"]["pageId"], page_id(7).to_string());
    assert_eq!(inline[4]["attrs"]["blockId"], block.to_string());
    assert_eq!(inline[5]["text"], "gone page");
    // A reference to a planned page has no block: the block ids of new pages are new.
    assert!(inline[1]["attrs"].get("blockId").is_none());
    assert_eq!(json["content"][1]["type"], "pageLink");
    assert_eq!(
        json["content"][1]["attrs"]["pageId"],
        page_id(1).to_string()
    );
    assert_eq!(json["content"][2]["type"], "paragraph");
    assert_eq!(json["content"][2]["content"][0]["text"], "[[Missing card]]");
    let missing: Vec<_> = warnings
        .iter()
        .map(|w| (w.code, w.detail.clone().unwrap()))
        .collect();
    assert_eq!(
        missing,
        vec![
            (ImportWarningCode::UnresolvedLink, "Nobody".to_owned()),
            (ImportWarningCode::UnresolvedLink, page_id(6).to_string()),
            (
                ImportWarningCode::UnresolvedLink,
                "a page of the import".to_owned()
            ),
            (ImportWarningCode::UnresolvedLink, "Missing card".to_owned()),
        ]
    );
    assert!(warnings.iter().all(|w| w.path == "Source/T.md"));
    // The links the document holds are the ones that resolved.
    assert_eq!(doc.links().len(), 4);
}

#[test]
fn attachments_resolve_or_leave_text() {
    let mut resolver = Fake::default();
    resolver
        .attachments
        .insert("Export/a.png".into(), attachment_id(1));
    resolver
        .attachments
        .insert("Export/r.pdf".into(), attachment_id(2));
    resolver.existing_attachments.insert(attachment_id(3));
    let page = planned(json!([
        {"type": "image", "attrs": {"$file": "Export/a.png", "alt": "A", "caption": "Cap", "widthPercent": 60, "align": "right", "width": 640}},
        {"type": "image", "attrs": {"$file": "Export/missing.png", "alt": "Lost picture"}},
        {"type": "image", "attrs": {"$attachment": attachment_id(3).to_string(), "alt": ""}},
        {"type": "image", "attrs": {"$attachment": attachment_id(4).to_string(), "alt": "", "caption": "Only a caption"}},
        {"type": "file", "attrs": {"$file": "Export/r.pdf", "$label": "report", "view": "preview"}},
        {"type": "file", "attrs": {"$file": "Export/dir/gone.pdf", "$label": "the report"}},
        {"type": "file", "attrs": {"$file": "Export/dir/gone2.pdf"}},
        {"type": "file", "attrs": {"$attachment": "not an id"}}
    ]));
    let (json, warnings, doc) = resolve_and_load(&page, &resolver);
    let blocks = json["content"].as_array().unwrap();
    assert_eq!(blocks[0]["type"], "image");
    assert_eq!(
        blocks[0]["attrs"]["attachmentId"],
        attachment_id(1).to_string()
    );
    assert_eq!(blocks[0]["attrs"]["alt"], "A");
    assert_eq!(blocks[0]["attrs"]["caption"], "Cap");
    assert_eq!(blocks[0]["attrs"]["widthPercent"], 60);
    assert_eq!(blocks[0]["attrs"]["align"], "right");
    assert_eq!(blocks[0]["attrs"]["width"], 640);
    assert_eq!(blocks[1]["content"][0]["text"], "Lost picture");
    assert_eq!(
        blocks[2]["attrs"]["attachmentId"],
        attachment_id(3).to_string()
    );
    assert_eq!(blocks[2]["attrs"]["alt"], "");
    assert_eq!(blocks[3]["content"][0]["text"], "Only a caption");
    assert_eq!(
        blocks[4]["attrs"]["attachmentId"],
        attachment_id(2).to_string()
    );
    assert_eq!(blocks[4]["attrs"]["view"], "preview");
    assert_eq!(blocks[5]["content"][0]["text"], "the report");
    assert_eq!(blocks[6]["content"][0]["text"], "gone2.pdf");
    assert_eq!(blocks[7]["content"][0]["text"], "not an id");
    assert_eq!(blocks.len(), 8);
    assert_eq!(warnings.len(), 5);
    assert!(
        warnings
            .iter()
            .all(|w| w.code == ImportWarningCode::MissingFile)
    );
    assert_eq!(doc.attachment_refs().len(), 3);
}

#[test]
fn galleries_keep_the_items_that_resolve() {
    let mut resolver = Fake::default();
    resolver
        .attachments
        .insert("Export/1.png".into(), attachment_id(1));
    resolver.existing_attachments.insert(attachment_id(2));
    let page = planned(json!([
        {"type": "gallery", "attrs": {"columns": 2, "items": [
            {"$file": "Export/1.png", "caption": "one"},
            {"$file": "Export/none.png"},
            {"$attachment": attachment_id(2).to_string()},
            {"$attachment": attachment_id(5).to_string()}
        ]}},
        {"type": "gallery", "attrs": {"items": [{"$file": "Export/none.png"}]}}
    ]));
    let (json, warnings, doc) = resolve_and_load(&page, &resolver);
    assert_eq!(json["content"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        json["content"][0]["attrs"]["items"],
        json!([
            {"attachmentId": attachment_id(1).to_string(), "caption": "one"},
            {"attachmentId": attachment_id(2).to_string()}
        ])
    );
    assert_eq!(json["content"][0]["attrs"]["columns"], 2);
    assert_eq!(warnings.len(), 2);
    assert_eq!(doc.attachment_refs().len(), 2);
}

#[test]
fn whiteboards_move_their_elements_to_the_root_map() {
    let page = planned(json!([
        {"type": "whiteboard", "attrs": {"height": 480, "$elements": {
            "a": {"id": "a", "type": "rectangle", "index": "a0"},
            "b": {"id": "b", "type": "text", "text": "hi"},
            "wrong": {"id": "other", "type": "line"}
        }}},
        {"type": "whiteboard"}
    ]));
    let (json, _, doc) = resolve_and_load(&page, &Fake::default());
    let first = json["content"][0]["attrs"]["id"].as_str().unwrap();
    assert_eq!(json["content"][0]["attrs"]["height"], 480);
    let board = &json["whiteboards"][first];
    assert_eq!(board.as_object().map(Map::len), Some(2));
    assert_eq!(board["b"]["text"], "hi");
    // The empty whiteboard has no entry.
    assert_eq!(json["whiteboards"].as_object().map(Map::len), Some(1));
    let block = kb_types::BlockId::from_str(first).unwrap();
    assert_eq!(doc.whiteboard_elements(block).len(), 2);
}

fn views_resolver() -> Fake {
    let mut resolver = Fake::default();
    for (n, name) in [(1u8, "tags"), (2, "status"), (3, "due")] {
        resolver
            .properties
            .insert(name.into(), PropertyId::from_random_bytes([n; 16]));
    }
    resolver.options.insert(
        ("tags".into(), "work".into()),
        OptionId::from_random_bytes([10; 16]),
    );
    resolver.options.insert(
        ("status".into(), "Done".into()),
        OptionId::from_random_bytes([11; 16]),
    );
    resolver.existing_pages.insert(page_id(1));
    resolver
}

fn pid(n: u8) -> String {
    PropertyId::from_random_bytes([n; 16]).to_string()
}

fn oid(n: u8) -> String {
    OptionId::from_random_bytes([n; 16]).to_string()
}

#[test]
fn views_get_their_names_back_as_ids() {
    let page = planned(json!([
        {"type": "pageList", "attrs": {
            "$names": true,
            "layout": "table",
            "properties": ["Status", "Gone", "Due", "STATUS"],
            "sort": {"by": "Due", "dir": "desc"},
            "source": {"kind": "tag", "option": "work"},
            "filters": [
                {"property": "Status", "op": "hasOption", "value": "Done"},
                {"property": "Status", "op": "hasOption", "value": "Missing option"},
                {"property": "Gone", "op": "isSet"},
                {"property": "Due", "op": "before", "value": "2026-10-07"},
                {"property": "Due", "op": "relatesTo", "value": page_id(1).to_string()},
                {"property": "Due", "op": "relatesTo", "value": page_id(2).to_string()}
            ],
            "groupBy": "Status",
            "hiddenGroups": ["none", "Done", "Nope"],
            "columnWidths": {"title": 300, "Due": 120, "Gone": 99},
            "calculations": {"title": "count_all", "Due": "sum", "Gone": "count_all", "Status": "a-function-of-a-newer-version"}
        }},
        {"type": "pageList", "attrs": {"$names": true, "layout": "list", "properties": [], "sort": {"by": "updated", "dir": "asc"}, "source": {"kind": "children"}}},
        {"type": "pageList", "attrs": {"$names": true, "sort": {"by": "Gone", "dir": "asc"}, "source": {"kind": "tag", "option": "unknown"}, "groupBy": "Gone", "hiddenGroups": ["x"]}}
    ]));
    let (json, warnings, _) = resolve_and_load(&page, &views_resolver());
    assert!(warnings.is_empty());
    let first = &json["content"][0]["attrs"];
    assert_eq!(first["layout"], "table");
    assert_eq!(first["properties"], json!([pid(2), pid(3)]));
    assert_eq!(first["sort"], json!({"by": pid(3), "dir": "desc"}));
    assert_eq!(first["source"], json!({"kind": "tag", "optionId": oid(10)}));
    assert_eq!(
        first["filters"],
        json!([
            {"propertyId": pid(2), "op": "hasOption", "value": oid(11)},
            {"propertyId": pid(3), "op": "before", "value": "2026-10-07"},
            {"propertyId": pid(3), "op": "relatesTo", "value": page_id(1).to_string()}
        ])
    );
    assert_eq!(first["groupBy"], pid(2));
    assert_eq!(first["hiddenGroups"], json!(["none", oid(11)]));
    assert_eq!(first["columnWidths"], json!({"title": 300, pid(3): 120}));
    assert_eq!(
        first["calculations"],
        json!({"title": "count_all", pid(3): "sum", pid(2): "a-function-of-a-newer-version"})
    );
    let second = &json["content"][1]["attrs"];
    assert_eq!(second["properties"], json!([]));
    assert_eq!(second["sort"], json!({"by": "updated", "dir": "asc"}));
    assert_eq!(second["source"], json!({"kind": "children"}));
    // What has no id is dropped; the view stays.
    let third = &json["content"][2]["attrs"];
    assert!(
        third.get("sort").is_none()
            && third.get("source").is_none()
            && third.get("groupBy").is_none()
    );
    assert!(third.get("hiddenGroups").is_none());
    assert!(third.get("calculations").is_none());
    assert_eq!(third["properties"], json!([]));
}

#[test]
fn every_view_of_a_block_gets_its_names_back_as_ids_and_the_first_one_is_mirrored() {
    let mut resolver = views_resolver();
    resolver
        .properties
        .insert("end".into(), PropertyId::from_random_bytes([4; 16]));
    let page = planned(json!([
        {"type": "pageList", "attrs": {
            "$names": true,
            "source": {"kind": "children"},
            // The block's own attributes say something else than its first view: the view wins.
            "layout": "list",
            "properties": ["Gone"],
            "views": [
                {"id": "k3f9a2bc", "name": "", "layout": "table", "properties": ["Status", "Gone", "Due"],
                 "sort": {"by": "Due", "dir": "desc"},
                 "filter": {"op": "and", "items": [
                     {"property": "Status", "op": "hasOption", "value": "Done"},
                     {"property": "Gone", "op": "isSet"},
                     {"op": "or", "items": [{"property": "Due", "op": "isSet"}, {"property": "Gone", "op": "isSet"}]},
                     {"op": "or", "items": [{"property": "Gone", "op": "isSet"}]},
                     {"op": "or", "items": []}]},
                 "columnWidths": {"title": 320, "Due": 120, "Gone": 90},
                 "calculations": {"title": "count_all", "Due": "sum", "Gone": "sum"}},
                {"id": "b81d00e4", "name": "By status", "layout": "board", "groupBy": "Status",
                 "hiddenGroups": ["Done", "Nope", "none"], "openIn": "center"},
                {"id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "dateBy": "Due", "endBy": "End"},
                // A repeated id and an unusable one get new ids; a calendar whose dates are gone loses them.
                {"id": "7c0a55d1", "name": "Copy", "layout": "calendar", "dateBy": "Gone", "endBy": "Gone"},
                {"name": "No id", "layout": "gallery", "dateBy": "created", "cardSize": "l"},
                {"id": "e2a9b6f0"}
            ]
        }},
        // An OR first view cannot be a flat list: the mirror carries no filters at all.
        {"type": "pageList", "attrs": {
            "$names": true,
            "filters": [{"property": "Status", "op": "isSet"}],
            "views": [{"id": "o", "layout": "table",
                       "filter": {"op": "or", "items": [{"property": "Status", "op": "isSet"}, {"property": "Due", "op": "isSet"}]}}]
        }},
        // No usable view: the block stays what its own attributes say.
        {"type": "pageList", "attrs": {"$names": true, "layout": "board", "views": ["x", 3]}}
    ]));
    let (json, warnings, _) = resolve_and_load(&page, &resolver);
    assert!(warnings.is_empty());

    let first = &json["content"][0]["attrs"];
    let views = first["views"].as_array().unwrap();
    assert_eq!(views.len(), 6);
    let tree = json!({"op": "and", "items": [
        {"propertyId": pid(2), "op": "hasOption", "value": oid(11)},
        {"op": "or", "items": [{"propertyId": pid(3), "op": "isSet"}]},
        {"op": "or", "items": []}
    ]});
    assert_eq!(
        views[0],
        json!({"id": "k3f9a2bc", "name": "", "layout": "table", "properties": [pid(2), pid(3)],
               "sort": {"by": pid(3), "dir": "desc"}, "filter": tree,
               "columnWidths": {"title": 320, pid(3): 120},
               "calculations": {"title": "count_all", pid(3): "sum"}})
    );
    assert_eq!(
        views[1],
        json!({"id": "b81d00e4", "name": "By status", "layout": "board", "properties": [],
               "sort": {"by": "title", "dir": "asc"}, "groupBy": pid(2),
               "hiddenGroups": [oid(11), "none"], "openIn": "center"})
    );
    assert_eq!(
        views[2],
        json!({"id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "properties": [],
               "sort": {"by": "title", "dir": "asc"}, "dateBy": pid(3), "endBy": pid(4)})
    );
    // The second "7c0a55d1" and the view with no id: new ids of 8 hexadecimal characters, all different.
    let ids: Vec<&str> = views
        .iter()
        .map(|view| view["id"].as_str().unwrap())
        .collect();
    let unique: std::collections::HashSet<&&str> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    for at in [3, 4] {
        let id = ids[at];
        assert!(
            id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()),
            "{id}"
        );
    }
    assert_eq!(
        (
            views[3]["name"].clone(),
            views[3].get("dateBy"),
            views[3].get("endBy")
        ),
        (json!("Copy"), None, None)
    );
    assert_eq!(
        (
            views[4]["layout"].clone(),
            views[4]["dateBy"].clone(),
            views[4]["cardSize"].clone()
        ),
        (json!("gallery"), json!("created"), json!("l"))
    );
    // A view that said nothing is complete: the editor reads the same defaults.
    assert_eq!(
        views[5],
        json!({"id": "e2a9b6f0", "name": "", "layout": "list", "properties": [],
               "sort": {"by": "title", "dir": "asc"}})
    );

    // The mirror: the block's legacy attributes are the first view's, whatever the annotation held.
    assert_eq!(first["layout"], "table");
    assert_eq!(first["properties"], views[0]["properties"]);
    assert_eq!(first["sort"], views[0]["sort"]);
    assert_eq!(
        first["filters"],
        json!([{"propertyId": pid(2), "op": "hasOption", "value": oid(11)}]),
        "the plain filters of the AND group"
    );
    assert_eq!(first["columnWidths"], views[0]["columnWidths"]);
    assert_eq!(first["calculations"], views[0]["calculations"]);
    assert!(first.get("groupBy").is_none() && first.get("hiddenGroups").is_none());
    assert_eq!(first["source"], json!({"kind": "children"}));

    // An OR group: the older build is told nothing about filters.
    let or = &json["content"][1]["attrs"];
    assert!(or.get("filters").is_none(), "{or}");
    assert_eq!(or["views"][0]["filter"]["op"], "or");
    assert_eq!(
        or["views"][0]["filter"]["items"].as_array().map(Vec::len),
        Some(2)
    );

    // Nothing usable in `views`: no `views` attribute, the legacy attributes as annotated.
    let none = &json["content"][2]["attrs"];
    assert!(none.get("views").is_none());
    assert_eq!(none["layout"], "board");
}

#[test]
fn the_widgets_of_a_dashboard_get_their_names_back_as_ids() {
    let mut resolver = views_resolver();
    resolver
        .properties
        .insert("cost".into(), PropertyId::from_random_bytes([5; 16]));
    let page = planned(json!([
        {"type": "pageList", "attrs": {
            "$names": true,
            "views": [
                {"id": "d", "name": "Overview", "layout": "dashboard", "widgets": [
                    {"id": "w0000001", "kind": "number", "title": "", "w": 1, "h": "s", "calc": {"by": "Due", "fn": "sum"}},
                    {"id": "w0000002", "kind": "number", "w": 1, "h": "s", "calc": {"by": "title", "fn": "count_all"},
                     "filter": {"op": "and", "items": [
                         {"property": "Status", "op": "hasOption", "value": "Done"},
                         {"property": "Gone", "op": "isSet"}]}},
                    {"id": "w0000003", "kind": "chart", "w": 2, "h": "m",
                     "chart": {"type": "bar", "groupBy": "Status", "measure": {"fn": "sum", "propertyId": "Cost"}, "bucket": "week"}},
                    {"id": "w0000004", "kind": "chart",
                     "chart": {"type": "line", "groupBy": "created", "measure": {"fn": "count"}}},
                    // What has no property in the space: the number counts again, the chart asks and counts.
                    {"id": "w0000005", "kind": "number", "calc": {"by": "Gone", "fn": "sum"},
                     "filter": {"op": "or", "items": [{"property": "Gone", "op": "isSet"}]}},
                    {"id": "w0000006", "kind": "chart",
                     "chart": {"type": "donut", "groupBy": "Gone", "measure": {"fn": "average", "propertyId": "Gone"}}},
                    {"id": "w0000007", "kind": "view", "viewId": "k3f9a2bc"}
                ]}
            ]
        }}
    ]));
    let (json, warnings, _) = resolve_and_load(&page, &resolver);
    assert!(warnings.is_empty());
    let views = json["content"][0]["attrs"]["views"].as_array().unwrap();
    let widgets = views[0]["widgets"].as_array().unwrap();
    assert_eq!(widgets.len(), 7);
    assert_eq!(widgets[0]["calc"], json!({"by": pid(3), "fn": "sum"}));
    assert_eq!(
        widgets[1]["calc"],
        json!({"by": "title", "fn": "count_all"})
    );
    assert_eq!(
        widgets[1]["filter"],
        json!({"op": "and", "items": [{"propertyId": pid(2), "op": "hasOption", "value": oid(11)}]})
    );
    assert_eq!(
        widgets[2]["chart"],
        json!({"type": "bar", "groupBy": pid(2), "measure": {"fn": "sum", "propertyId": pid(5)}, "bucket": "week"})
    );
    assert_eq!(
        widgets[3]["chart"],
        json!({"type": "line", "groupBy": "created", "measure": {"fn": "count"}})
    );
    assert_eq!(
        widgets[4]["calc"],
        json!({"by": "title", "fn": "count_all"})
    );
    assert!(widgets[4].get("filter").is_none());
    assert_eq!(
        widgets[5]["chart"],
        json!({"type": "donut", "groupBy": null, "measure": {"fn": "count"}})
    );
    assert_eq!(
        widgets[6],
        json!({"id": "w0000007", "kind": "view", "viewId": "k3f9a2bc"})
    );
    assert_eq!(
        (widgets[0]["id"].as_str(), views[0]["layout"].as_str()),
        (Some("w0000001"), Some("dashboard"))
    );
}

#[test]
fn a_block_without_views_resolves_exactly_as_before() {
    let page = planned(json!([
        {"type": "pageList", "attrs": {
            "$names": true, "layout": "table", "properties": ["Status"],
            "filters": [{"property": "Status", "op": "hasOption", "value": ["Done", "Missing"]},
                        {"property": "Status", "op": "hasOption", "value": ["Missing"]}]
        }}
    ]));
    let (json, warnings, _) = resolve_and_load(&page, &views_resolver());
    assert!(warnings.is_empty());
    let attrs = &json["content"][0]["attrs"];
    assert!(attrs.get("views").is_none());
    assert_eq!(attrs["properties"], json!([pid(2)]));
    // A list of options keeps the ones that exist; none left, no filter.
    assert_eq!(
        attrs["filters"],
        json!([{"propertyId": pid(2), "op": "hasOption", "value": [oid(11)]}])
    );
}

#[test]
fn the_sorts_of_a_view_the_new_operators_and_the_linked_sources_get_their_ids() {
    let page = planned(json!([
        {"type": "pageList", "attrs": {
            "$names": true,
            "source": {"kind": "page", "pageId": page_id(7).to_string()},
            "views": [
                {"id": "a", "layout": "table",
                 "sort": {"by": "Gone", "dir": "asc"},
                 "sorts": [
                     {"by": "Gone", "dir": "asc"}, {"by": "Due", "dir": "desc"}, "junk",
                     {"by": "created", "dir": "asc"}, {"by": "STATUS"}, {"by": "due", "dir": "asc"},
                     {"by": "title", "dir": "desc"}, {"by": "updated", "dir": "asc"},
                     {"by": "Tags", "dir": "asc"}
                 ],
                 "filter": {"op": "and", "items": [
                     {"property": "Status", "op": "lacksOption", "value": "Done"},
                     {"property": "Status", "op": "lacksOption", "value": "Missing option"},
                     {"property": "Status", "op": "lacksOption", "value": ["Done", "Missing option"]},
                     {"property": "Due", "op": "notRelatesTo", "value": page_id(1).to_string()},
                     {"property": "Due", "op": "notRelatesTo", "value": page_id(2).to_string()},
                     {"property": "Due", "op": "within", "value": {"relative": "pastDays", "n": 7}},
                     {"property": "Due", "op": "onOrBefore", "value": {"relative": "today"}}
                 ]}}
            ]
        }},
        // No key of `sorts` has an id: `sort` is what it was; a space source passes.
        {"type": "pageList", "attrs": {
            "$names": true, "source": {"kind": "space"},
            "views": [{"id": "b", "layout": "table", "sort": {"by": "Due", "dir": "asc"},
                       "sorts": [{"by": "Gone", "dir": "asc"}]}]
        }}
    ]));
    let (json, warnings, _) = resolve_and_load(&page, &views_resolver());
    assert!(warnings.is_empty());

    let first = &json["content"][0]["attrs"];
    assert_eq!(
        first["source"],
        json!({"kind": "page", "pageId": page_id(7).to_string()})
    );
    let view = &first["views"][0];
    // A key with no id goes, a repeated one is dropped, at most five are kept, and `sort` is the first key
    // left (here the one `sort` named has no id), in the view and in the block's mirror.
    assert_eq!(
        view["sorts"],
        json!([
            {"by": pid(3), "dir": "desc"}, {"by": "created", "dir": "asc"},
            {"by": pid(2), "dir": "asc"}, {"by": "title", "dir": "desc"},
            {"by": "updated", "dir": "asc"}
        ])
    );
    assert_eq!(view["sort"], json!({"by": pid(3), "dir": "desc"}));
    assert_eq!(first["sort"], view["sort"]);
    assert!(
        first.get("sorts").is_none(),
        "the block mirrors `sort` only"
    );
    assert_eq!(
        view["filter"],
        json!({"op": "and", "items": [
            {"propertyId": pid(2), "op": "lacksOption", "value": oid(11)},
            {"propertyId": pid(2), "op": "lacksOption", "value": [oid(11)]},
            {"propertyId": pid(3), "op": "notRelatesTo", "value": page_id(1).to_string()},
            {"propertyId": pid(3), "op": "within", "value": {"relative": "pastDays", "n": 7}},
            {"propertyId": pid(3), "op": "onOrBefore", "value": {"relative": "today"}}
        ]})
    );

    let second = &json["content"][1]["attrs"];
    assert_eq!(second["source"], json!({"kind": "space"}));
    let view = &second["views"][0];
    assert!(view.get("sorts").is_none(), "{view}");
    assert_eq!(view["sort"], json!({"by": pid(3), "dir": "asc"}));
    assert_eq!(second["sort"], view["sort"]);
}

#[test]
fn a_notion_plan_resolves_to_valid_documents() {
    let files: Vec<SourceFile> = notion_files();
    let plan = run(
        files,
        &[&format!("Export-1234/Home {HOME}/diagram.png")],
        Some("Export-1234"),
    );
    let mut resolver = Fake::default();
    resolver.pages = (0..plan.pages.len())
        .map(|n| page_id(u8::try_from(n + 1).unwrap()))
        .collect();
    resolver.attachments.insert(
        format!("Export-1234/Home {HOME}/diagram.png"),
        attachment_id(1),
    );
    for property in &plan.properties {
        resolver.properties.insert(
            property.name.to_lowercase(),
            PropertyId::from_random_bytes([77; 16]),
        );
    }
    let mut all_ids: HashSet<String> = HashSet::new();
    let mut warnings = Vec::new();
    for page in &plan.pages {
        let (json, page_warnings, _) = resolve_and_load(page, &resolver);
        warnings.extend(page_warnings);
        let mut found = Vec::new();
        ids(&json, &mut found);
        for id in found {
            assert!(all_ids.insert(id), "an id is used twice");
        }
    }
    assert!(warnings.is_empty(), "{warnings:?}");
    // The Home page links the sub page, the database and shows the diagram.
    let (home, _, doc) = resolve_and_load(&plan.pages[1], &resolver);
    assert_eq!(home["content"][0]["type"], "callout");
    let linked: Vec<_> = doc.links().iter().map(|l| l.to_page).collect();
    assert!(linked.contains(&page_id(3)) && linked.contains(&page_id(4)));
    assert_eq!(doc.attachment_refs().len(), 1);
    // The database page's view lists the columns by id.
    let (tasks, _, _) = resolve_and_load(&plan.pages[3], &resolver);
    assert_eq!(tasks["content"][0]["type"], "pageList");
    assert_eq!(
        tasks["content"][0]["attrs"]["properties"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

/// The golden file of atomik-kb's own Markdown, imported, is the page it was exported from, up to
/// ids and what the Markdown cannot hold (the targets of block links, text colours, person mentions).
#[test]
fn the_golden_file_flavour_comes_back_as_its_page() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/doc/");
    let markdown = std::fs::read_to_string(format!("{root}page-v1.file.md")).expect("golden file");
    let portable: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{root}page-v1.portable.json")).expect("portable sample"),
    )
    .expect("portable JSON");

    // The files of the archive the golden was written for.
    let live_elements: Map<String, Value> = portable["whiteboards"]
        .as_object()
        .and_then(|boards| boards.values().next())
        .and_then(Value::as_object)
        .map(|elements| {
            elements
                .iter()
                .filter(|(_, e)| e["isDeleted"] != true)
                .map(|(id, e)| (id.clone(), e.clone()))
                .collect()
        })
        .unwrap_or_default();
    let excalidraw = json!({
        "type": "excalidraw", "version": 2, "source": "atomik-kb",
        "elements": live_elements.values().collect::<Vec<_>>(),
        "appState": {"viewBackgroundColor": "#ffffff"}, "files": {}
    })
    .to_string();
    let plan = run(
        vec![
            file("Welcome.md", &markdown),
            file("Linked page.md", "linked"),
            file("Welcome/Whiteboard.excalidraw", &excalidraw),
        ],
        &[
            "Welcome/diagram.png",
            "Welcome/Quarterly report.pdf",
            "Welcome/photo (2).jpg",
        ],
        None,
    );
    assert_eq!(plan.pages.len(), 2);
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    assert_eq!(
        plan.needed_files,
        [
            "Welcome/diagram.png",
            "Welcome/Quarterly report.pdf",
            "Welcome/photo (2).jpg"
        ]
    );

    let fixture_page = |text: &str| PageId::from_str(text).unwrap();
    let linked = fixture_page("7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6");
    let mut resolver = Fake::default();
    resolver.pages = vec![page_id(1), linked];
    resolver.attachments.insert(
        "Welcome/diagram.png".into(),
        AttachmentId::from_str("5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5").unwrap(),
    );
    resolver.attachments.insert(
        "Welcome/Quarterly report.pdf".into(),
        AttachmentId::from_str("6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6").unwrap(),
    );
    resolver.attachments.insert(
        "Welcome/photo (2).jpg".into(),
        AttachmentId::from_str("7b2e3f4a-5c6d-4e7f-8091-a2b3c4d5e6f7").unwrap(),
    );
    resolver
        .existing_attachments
        .insert(AttachmentId::from_str("8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708").unwrap());
    let tags = PropertyId::from_str("0000000a-0000-4000-8000-000000000001").unwrap();
    let priority = PropertyId::from_str("5b2a7c14-9e30-4d68-8a15-3f6e0c9d2b74").unwrap();
    resolver.properties.insert("tags".into(), tags);
    resolver.properties.insert("priority".into(), priority);
    resolver.options.insert(
        ("tags".into(), "work".into()),
        OptionId::from_str("3c9d1e52-6a47-4b8e-9f20-5d1c7a8b4e61").unwrap(),
    );

    let welcome = plan
        .pages
        .iter()
        .position(|p| p.title == "Welcome")
        .unwrap();
    // `resolve` is given the index of "Linked page" as 1 or 0, whichever sorts first.
    resolver.pages = (0..plan.pages.len())
        .map(|n| {
            if plan.pages[n].title == "Linked page" {
                linked
            } else {
                page_id(1)
            }
        })
        .collect();
    let (json, warnings, _) = resolve_and_load(&plan.pages[welcome], &resolver);
    assert!(warnings.is_empty(), "{warnings:?}");

    let expected = normalise(&portable);
    let got = normalise(&json);
    assert_eq!(
        serde_json::to_string_pretty(&got["content"]).unwrap(),
        serde_json::to_string_pretty(&expected["content"]).unwrap()
    );
    // The whiteboard's live elements.
    let board = |doc: &Value| -> Value {
        doc["whiteboards"]
            .as_object()
            .and_then(|b| b.values().next())
            .cloned()
            .unwrap_or(Value::Null)
    };
    let expected_board: Map<String, Value> = board(&portable)
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(_, e)| e["isDeleted"] != true)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(board(&json), Value::Object(expected_board));
}

/// A portable page without ids, block targets and text colours, with adjacent equal text joined.
fn normalise(value: &Value) -> Value {
    match value {
        Value::Array(items) => {
            let mut out: Vec<Value> = Vec::new();
            for item in items {
                let item = normalise(item);
                if let Some(last) = out.last_mut()
                    && last["type"] == "text"
                    && item["type"] == "text"
                    && last.get("marks") == item.get("marks")
                    && let (Some(Value::String(a)), Some(Value::String(b))) =
                        (last.get("text").cloned(), item.get("text").cloned())
                {
                    last["text"] = Value::String(format!("{a}{b}"));
                    continue;
                }
                out.push(item);
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            // A person mention is its `@name` as text in Markdown and is not read back as a mention (ADR-0049).
            if map.get("type").and_then(Value::as_str) == Some("personMention") {
                let label = map
                    .get("attrs")
                    .and_then(|attrs| attrs.get("label"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return json!({"type": "text", "text": format!("@{label}")});
            }
            let mut out = Map::new();
            for (key, child) in map {
                match key.as_str() {
                    "whiteboards" => {}
                    "attrs" => {
                        let mut attrs = child.as_object().cloned().unwrap_or_default();
                        attrs.remove("id");
                        let kind = map.get("type").and_then(Value::as_str).unwrap_or_default();
                        if kind == "pageRef" || kind == "pageLink" {
                            attrs.remove("blockId");
                        }
                        if !attrs.is_empty() {
                            out.insert(key.clone(), Value::Object(attrs));
                        }
                    }
                    "marks" => {
                        let kept: Vec<Value> = child
                            .as_array()
                            .map(|marks| {
                                marks
                                    .iter()
                                    .filter(|m| {
                                        m["type"] != "textColor" && m["type"] != "highlight"
                                    })
                                    .cloned()
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !kept.is_empty() {
                            out.insert(key.clone(), Value::Array(kept));
                        }
                    }
                    _ => {
                        out.insert(key.clone(), normalise(child));
                    }
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The clipboard flavour of the same sample (`attachment://` links, `[[Title]]` wiki links) also
/// imports into a sound page: its attachments belong to the space, its link is to a planned page.
#[test]
fn the_clipboard_flavour_imports_into_a_sound_page() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/doc/");
    let markdown = std::fs::read_to_string(format!("{root}page-v1.md")).expect("golden file");
    let plan = run(
        vec![
            file("Welcome.md", &markdown),
            file("Linked page.md", "linked"),
        ],
        &[],
        None,
    );
    assert_eq!(plan.pages.len(), 2);
    let mut resolver = Fake::default();
    resolver.pages = (0..2).map(|n| page_id(n + 1)).collect();
    for id in [
        "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5",
        "6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6",
        "7b2e3f4a-5c6d-4e7f-8091-a2b3c4d5e6f7",
        "8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708",
    ] {
        resolver
            .existing_attachments
            .insert(AttachmentId::from_str(id).unwrap());
    }
    let welcome = plan
        .pages
        .iter()
        .position(|p| p.title == "Welcome")
        .unwrap();
    let (json, warnings, doc) = resolve_and_load(&plan.pages[welcome], &resolver);
    assert!(warnings.is_empty(), "{warnings:?}");
    let blocks = json["content"].as_array().unwrap();
    let types: Vec<_> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
    // Everything of the sample that Markdown can say comes back; the whiteboard is a quote.
    for expected in [
        "heading",
        "paragraph",
        "bulletList",
        "orderedList",
        "taskList",
        "blockquote",
        "codeBlock",
        "mathBlock",
        "horizontalRule",
        "details",
        "callout",
        "table",
        "image",
        "file",
        "pageLink",
    ] {
        assert!(types.contains(&expected), "no {expected} in {types:?}");
    }
    assert_eq!(doc.attachment_refs().len(), 5);
    assert!(!doc.links().is_empty());
}
