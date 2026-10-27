//! Tests of the shapes a larger workspace brought: database containers with an embedded view,
//! row documents in the databases folder, templates, citations, speaker names, people,
//! files outside the archive, the space named after the source. Synthetic archives only.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::ImportWarningCode;
use serde_json::{Value, json};

use super::archive::row_document_id;
use super::tests::{Backup, Doc, WS, blocks, codes, index_of, titles, warnings_of};
use super::tests_db::{R1, R2, field, row, select_options};
use crate::json::plain_text;
use crate::resolve_tests::{Fake, resolve_and_load};
use crate::{ImportValue, Plan};

fn text_of(value: &Value) -> String {
    plain_text(value)
}

/// A small database: a title, a select and two rows (`R1` has a document).
fn small_db(database_id: &str, layout: i64) -> Value {
    json!({
        "database_id": database_id,
        "views": [{"id": "v", "layout": layout, "field_orders": [{"id": "p"}, {"id": "s"}],
                   "group_settings": [{"field_id": "s", "groups": []}],
                   "row_orders": [{"id": R1, "height": 60}, {"id": R2, "height": 60}]}],
        "fields": [field("p", "Name", 0, true, json!({})),
                   field("s", "Status", 3, false, select_options(3, &[("o1", "Open", "Blue")]))],
        "rows": [row(R1, &[("p", 0, json!("Row one")), ("s", 3, json!("o1"))]),
                 row(R2, &[("p", 0, json!("Row two"))])],
        "row_metas": {R1: {"is_document_empty": false}, R2: {"is_document_empty": true}},
    })
}

/// A container `cont` under `parent` and its embedded view `emb` that holds the database.
fn container(backup: &mut Backup, parent: &str, db: &Value) {
    backup.view(
        "cont",
        "Container title",
        1,
        parent,
        Some(json!({"database_id": db["database_id"], "is_database_container": true})),
        None,
    );
    backup.view(
        "emb",
        "Embedded view",
        1,
        "cont",
        Some(json!({"database_id": db["database_id"], "embedded": true})),
        None,
    );
    backup.text_file("collab_jsons/databases/emb.json", &db.to_string());
}

// ----- row documents in the databases folder ---------------------------------------------------

#[test]
fn a_row_document_in_the_databases_folder_is_read() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("dbv", "Tasks", 1, "sp1", &small_db("db1", 0));
    let mut doc = Doc::new();
    doc.para("document in the databases folder");
    let id = row_document_id(R1).unwrap();
    backup.text_file(
        &format!("collab_jsons/databases/dbv/row_documents/{id}.json"),
        &doc.json(),
    );
    // The view AppFlowy makes for the document is still a view of the workspace.
    backup.row_document(R1, None);
    let plan = backup.plan();
    let first = index_of(&plan, "Row one");
    assert_eq!(
        blocks(&plan, first)[0]["content"][0]["text"],
        "document in the databases folder"
    );
    assert_eq!(
        plan.pages[first].source_path,
        format!("collab_jsons/databases/dbv/row_documents/{id}.json")
    );
    assert_eq!(plan.pages.len(), 4);
}

#[test]
fn a_document_in_the_documents_folder_wins_over_a_row_document_copy() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("dbv", "Tasks", 1, "sp1", &small_db("db1", 0));
    let id = row_document_id(R1).unwrap();
    let mut copy = Doc::new();
    copy.para("the copy");
    backup.text_file(
        &format!("collab_jsons/databases/dbv/row_documents/{id}.json"),
        &copy.json(),
    );
    let mut original = Doc::new();
    original.para("the original");
    backup.row_document(R1, Some(&original));
    let plan = backup.plan();
    let first = index_of(&plan, "Row one");
    assert_eq!(
        blocks(&plan, first)[0]["content"][0]["text"],
        "the original"
    );
}

#[test]
fn row_documents_of_the_export_warnings_are_a_fallback_and_unknown_kinds_are_ignored() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("dbv", "Tasks", 1, "sp1", &small_db("db1", 0));
    let mut doc = Doc::new();
    doc.para("found through the warning");
    backup.document("custom-doc", &doc.json());
    backup.text_file(
        "EXPORT_WARNINGS.json",
        &json!({"omitted_item_count": 5, "samples": [
            {"kind": "unregistered_row_document", "database_id": "db1", "row_id": R1, "document_id": "custom-doc"},
            {"kind": "database_row_inventory", "database_id": "db1", "view_ids": ["dbv"], "reason": "x"},
            {"kind": "invalid_document_collab", "view_id": "v", "object_id": "o", "reason": "y"},
            {"kind": "a_kind_of_the_future", "anything": [1, 2, 3]},
            {"kind": 7}, "text", null, 5,
            {"kind": "unregistered_row_document", "row_id": 3, "document_id": null},
        ]})
        .to_string(),
    );
    let plan = backup.plan();
    assert_eq!(
        blocks(&plan, index_of(&plan, "Row one"))[0]["content"][0]["text"],
        "found through the warning"
    );
    // Whatever else the file holds is left alone.
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.text_file("EXPORT_WARNINGS.json", "[1, 2");
    backup.text_file("EXPORT_WARNINGS.json", r#"{"samples": "no"}"#);
    assert_eq!(backup.plan().pages.len(), 1);
}

// ----- database containers ---------------------------------------------------------------------

#[test]
fn a_container_and_its_embedded_view_are_one_database_page() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    let db = small_db("db1", 0);
    container(&mut backup, "sp1", &db);
    backup.page("after", "After", "sp1", &Doc::new());
    let mut doc = Doc::new();
    doc.para("document of row one");
    backup.row_document(R1, Some(&doc));
    let plan = backup.plan();
    // The container's title and place, the embedded view's rows, fields and view; the embedded
    // view is not a page.
    assert_eq!(
        titles(&plan),
        ["Space", "Container title", "Row one", "Row two", "After"]
    );
    let page = index_of(&plan, "Container title");
    assert_eq!(plan.pages[page].parent, Some(0));
    assert_eq!(plan.pages[index_of(&plan, "Row one")].parent, Some(page));
    assert_eq!(
        plan.pages[page].source_path,
        "collab_jsons/databases/emb.json"
    );
    let view = &blocks(&plan, page)[0];
    assert_eq!(view["type"], "pageList");
    assert_eq!(view["attrs"]["layout"], "table");
    assert_eq!(view["attrs"]["properties"], json!(["Status"]));
    assert_eq!(plan.properties.len(), 1);
    assert_eq!(
        plan.pages[index_of(&plan, "Row one")].properties,
        [("Status".to_owned(), ImportValue::Text("Open".into()))]
    );
    assert_eq!(
        blocks(&plan, index_of(&plan, "Row one"))[0]["content"][0]["text"],
        "document of row one"
    );
}

#[test]
fn links_to_either_view_or_to_the_database_reach_the_page() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    container(&mut backup, "sp1", &small_db("db1", 0));
    let mut doc = Doc::new();
    doc.add(
        "root",
        "grid",
        json!({"view_id": "cont", "parent_id": "p", "database_id": "db1"}),
        None,
    );
    doc.add(
        "root",
        "grid",
        json!({"view_id": "emb", "parent_id": "p"}),
        None,
    );
    doc.add(
        "root",
        "board",
        json!({"view_id": "gone", "view_ids": ["nope", "emb"], "parent_id": "p"}),
        None,
    );
    doc.add(
        "root",
        "grid",
        json!({"view_id": "gone", "database_id": "db1"}),
        None,
    );
    // The parent is a view of the database here (not the page that holds the block).
    doc.add("root", "grid", json!({"parent_id": "cont"}), None);
    doc.add(
        "root",
        "grid",
        json!({"view_id": "gone", "parent_id": "p"}),
        None,
    );
    doc.add(
        "root",
        "paragraph",
        json!({}),
        Some(
            json!([{"insert": "$", "attributes": {"mention": {"type": "page", "page_id": "emb"}}}]),
        ),
    );
    backup.page("p", "Source", "sp1", &doc);
    let plan = backup.plan();
    let page = index_of(&plan, "Container title");
    let content = blocks(&plan, index_of(&plan, "Source"));
    for (n, block) in content.iter().take(5).enumerate() {
        assert_eq!(block["type"], "pageLink", "block {n}");
        assert_eq!(block["attrs"]["$page"]["planned"], page, "block {n}");
    }
    // `parent_id` that is the page holding the block is not a database: it does not resolve.
    assert_eq!(content[5]["type"], "paragraph");
    assert_eq!(content[6]["content"][0]["attrs"]["$page"]["planned"], page);
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnresolvedLink),
        [Some("gone")]
    );
}

#[test]
fn a_container_without_its_database_is_a_missing_page_and_a_loose_embedded_view_a_database() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.view(
        "cont",
        "Empty container",
        1,
        "sp1",
        Some(json!({"is_database_container": true})),
        None,
    );
    backup.view(
        "emb",
        "Embedded without file",
        1,
        "cont",
        Some(json!({"embedded": true})),
        None,
    );
    // An embedded view whose container is not marked as one is an ordinary database view.
    backup.view("plain", "Plain parent", 0, "sp1", None, None);
    backup.view(
        "emb2",
        "Loose embedded",
        1,
        "plain",
        Some(json!({"embedded": true, "database_id": "db2"})),
        None,
    );
    backup.text_file(
        "collab_jsons/databases/emb2.json",
        &small_db("db2", 0).to_string(),
    );
    let plan = backup.plan();
    assert!(titles(&plan).contains(&"Empty container"));
    assert!(titles(&plan).contains(&"Loose embedded"));
    assert!(warnings_of(&plan, ImportWarningCode::MissingFile).contains(&Some("Empty container")));
    assert!(
        warnings_of(&plan, ImportWarningCode::MissingFile).contains(&Some("Embedded without file"))
    );
}

#[test]
fn children_of_an_embedded_view_move_to_the_container_and_other_views_stay() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    container(&mut backup, "sp1", &small_db("db1", 0));
    backup.page("under-emb", "Under the embedded view", "emb", &Doc::new());
    backup.page("under-cont", "Under the container", "cont", &Doc::new());
    // A second view of the database, a child of the container: a linked view.
    backup.view(
        "second",
        "Second view",
        2,
        "cont",
        Some(json!({"database_id": "db1"})),
        None,
    );
    backup.text_file(
        "collab_jsons/databases/second.json",
        &small_db("db1", 1).to_string(),
    );
    let plan = backup.plan();
    let page = index_of(&plan, "Container title");
    assert_eq!(
        plan.pages
            .iter()
            .filter(|p| p.title == "Embedded view")
            .count(),
        0
    );
    assert_eq!(
        plan.pages[index_of(&plan, "Under the embedded view")].parent,
        Some(page)
    );
    assert_eq!(
        plan.pages[index_of(&plan, "Under the container")].parent,
        Some(page)
    );
    let second = index_of(&plan, "Second view");
    assert_eq!(plan.pages[second].parent, Some(page));
    assert_eq!(blocks(&plan, second)[0]["type"], "pageLink");
    assert!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("linked view"))
    );
}

#[test]
fn the_layout_comes_from_the_view_table_not_from_the_database_file() {
    // The view table counts 1 grid, 2 board, 3 calendar; the database file counts 0 grid, 1
    // board, 2 calendar. A board is 2 in the table and 1 in the file.
    let layout_of = |table: i64, file: i64| -> String {
        let mut backup = Backup::new();
        backup.space("sp1", "Space");
        backup.database("dbv", "Db", table, "sp1", &small_db("db1", file));
        let plan = backup.plan();
        blocks(&plan, 1)[0]["attrs"]["layout"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(layout_of(2, 1), "board");
    assert_eq!(layout_of(1, 0), "table");
    // The file does not turn a grid into a board, or the other way round.
    assert_eq!(layout_of(1, 1), "table");
    assert_eq!(layout_of(2, 0), "board");
    // A container whose embedded view is a grid in the table and 0 in the file: a table.
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    container(&mut backup, "sp1", &small_db("db1", 0));
    let plan = backup.plan();
    assert_eq!(blocks(&plan, 1)[0]["attrs"]["layout"], "table");
    // A view with a layout number the table does not know falls back to the file's.
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.view(
        "dbv",
        "Odd",
        9,
        "sp1",
        Some(json!({"database_id": "db1"})),
        None,
    );
    backup.text_file(
        "collab_jsons/databases/dbv.json",
        &small_db("db1", 1).to_string(),
    );
    let plan = backup.plan();
    assert_eq!(blocks(&plan, 1)[0]["attrs"]["layout"], "board");
    assert_eq!(blocks(&plan, 1)[0]["attrs"]["groupBy"], "Status");
}

// ----- templates -------------------------------------------------------------------------------

#[test]
fn a_row_template_is_not_a_page() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("keep", "Keep", "sp1", &Doc::new());
    for (id, extra) in [
        (
            "t1",
            json!({"database_row_template": true, "database_view_id": "v", "template_id": "t"}),
        ),
        ("t2", json!({"template_id": "other"})),
        ("t3", json!({"database_view_id": "v"})),
    ] {
        backup.view(id, "Template", 0, "sp1", Some(extra), None);
        let mut doc = Doc::new();
        doc.para("template content");
        backup.document(id, &doc.json());
    }
    // An empty or false value does not make a template.
    backup.view(
        "real",
        "Real",
        0,
        "sp1",
        Some(json!({"database_row_template": false, "template_id": ""})),
        None,
    );
    let plan = backup.plan();
    assert_eq!(titles(&plan), ["Space", "Keep", "Real"]);
    // One warning for the import.
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent)
            .into_iter()
            .filter(|d| *d == Some("template"))
            .count(),
        1
    );
}

// ----- text ------------------------------------------------------------------------------------

#[test]
fn citations_are_numbers_in_brackets_and_people_are_at_names() {
    let mut doc = Doc::new();
    doc.add(
        "root",
        "paragraph",
        json!({}),
        Some(json!([
            {"insert": "A claim"},
            {"insert": "^", "attributes": {"reference": {"number": 3, "blockIds": ["b1", "b2"]}}},
            {"insert": "^", "attributes": {"reference": {"number": "12", "blockIds": []}, "bold": true}},
            {"insert": "^", "attributes": {"reference": {"blockIds": []}}},
            {"insert": "^", "attributes": {"reference": "nonsense"}},
            {"insert": " by "},
            {"insert": "$", "attributes": {"mention": {"type": "person", "person_id": "x", "person_name": "Ann Lee",
                                                       "page_id": "p", "block_id": "b"}}},
            {"insert": " and "},
            {"insert": "$", "attributes": {"mention": {"type": "person", "person_id": "y"}}},
            {"insert": " and "},
            {"insert": "$", "attributes": {"mention": {"type": "person", "person_name": "@Bo"}}},
        ])),
    );
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Page", "sp1", &doc);
    let plan = backup.plan();
    let inline = blocks(&plan, 1)[0]["content"].as_array().unwrap();
    let texts: Vec<&str> = inline.iter().filter_map(|n| n["text"].as_str()).collect();
    assert_eq!(
        texts,
        [
            "A claim", "[3]", "[12]", " by ", "@Ann Lee", " and ", "@person", " and ", "@Bo"
        ]
    );
    // No link, no mark on a citation (but a bold one stays bold).
    assert!(inline[1].get("marks").is_none());
    assert_eq!(inline[2]["marks"], json!([{"type": "bold"}]));
    assert!(!blocks(&plan, 1)[0].to_string().contains("\"link\""));
    assert!(!blocks(&plan, 1)[0].to_string().contains('^'));
}

#[test]
fn speakers_have_their_names_when_the_meeting_gives_them() {
    let mut doc = Doc::new();
    for (map, label) in [
        (
            json!({"0": {"name": "Ann"}, "1": {"name": "  Bo   Li "}, "2": {"name": ""}, "3": 5}),
            "object",
        ),
        (
            json!(r#"{"0": {"name": "Ann"}, "1": {"name": "Bo Li"}}"#),
            "string",
        ),
        (json!("not json"), "broken"),
    ] {
        let meeting = doc.add("root", "ai_meeting", json!({"speaker_info_map": map}), None);
        let transcript = doc.add(&meeting, "ai_meeting_transcription", json!({}), None);
        for (speaker, at) in [("0", 5), ("1", 70), ("2", 71), ("3", 72), ("9", 73)] {
            let turn = doc.add(
                &transcript,
                "ai_meeting_speaker",
                json!({"speaker_id": speaker, "timestamp": at}),
                None,
            );
            doc.text(&turn, "paragraph", json!({}), label);
        }
    }
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Page", "sp1", &doc);
    let plan = backup.plan();
    let turns = |block: usize| -> Vec<String> {
        blocks(&plan, 1)[block]["content"][1]["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(text_of)
            .collect()
    };
    assert_eq!(
        turns(0),
        [
            "[00:05] Ann: object",
            "[01:10] Bo Li: object",
            "[01:11] Speaker 2: object",
            "[01:12] Speaker 3: object",
            "[01:13] Speaker 9: object"
        ]
    );
    assert_eq!(turns(1)[0], "[00:05] Ann: string");
    assert_eq!(turns(1)[1], "[01:10] Bo Li: string");
    assert_eq!(turns(1)[2], "[01:11] Speaker 2: string");
    assert_eq!(turns(2)[0], "[00:05] Speaker 0: broken");
}

#[test]
fn layout_data_that_has_no_equivalent_is_ignored_without_a_warning() {
    let mut doc = Doc::new();
    doc.text(
        "root",
        "paragraph",
        json!({"align": "center", "level": 2}),
        "aligned",
    );
    let toggle = doc.text(
        "root",
        "toggle_list",
        json!({"level": 2, "collapsed": false}),
        "A toggle heading",
    );
    doc.text(&toggle, "paragraph", json!({}), "inside");
    doc.add("root", "image", json!({"url": "https://h.example/a.png", "align": "right", "height": 200, "width": 300, "image_type": 1}), None);
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Page", "sp1", &doc);
    let plan = backup.plan();
    let b = blocks(&plan, 1);
    assert_eq!(b[0]["type"], "paragraph");
    assert_eq!(b[1]["type"], "details");
    assert_eq!(text_of(&b[1]["content"][0]), "A toggle heading");
    assert_eq!(b[2]["attrs"]["align"], "right");
    assert!(b[2]["attrs"].get("height").is_none());
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent),
        [Some("icon")],
        "only the space's icon"
    );
}

// ----- images outside the archive --------------------------------------------------------------

#[test]
fn images_by_relative_url_are_local_files_and_outside_paths_are_never_followed() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.local_file("here.png");
    backup.local_file("also.png");
    backup.local_file("shared.png");
    let mut doc = Doc::new();
    for (url, image_type) in [
        ("p1/here.png", Some(1)),
        ("p1/also.png", Some(2)),
        ("also.png", None),
        ("p1/gone.png", Some(1)),
        ("\\\\nas\\share\\shared.png", None),
        ("//nas/share/shared.png", None),
        ("file:///C:/Users/me/shared.png", None),
        ("FILE:shared.png", None),
        ("C:\\Users\\me\\shared.png", None),
        ("/home/me/shared.png", None),
        ("", None),
    ] {
        let mut data = json!({"url": url});
        if let Some(image_type) = image_type {
            data["image_type"] = json!(image_type);
        }
        doc.add("root", "image", data, None);
    }
    doc.add(
        "root",
        "file",
        json!({"url": "\\\\nas\\share\\report.pdf", "url_type": 2, "name": "Report.pdf"}),
        None,
    );
    doc.add(
        "root",
        "file",
        json!({"url": "file:///C:/x/other.pdf", "name": ""}),
        None,
    );
    backup.page("p1", "Page", "sp1", &doc);
    let plan = backup.plan();
    // Only the three relative references are files; the outside ones are not even looked up
    // by name (`shared.png` is in the archive and still not used).
    assert_eq!(
        plan.needed_files,
        ["local_files/here.png", "local_files/also.png"]
    );
    let b = blocks(&plan, 1);
    assert_eq!(b[0]["attrs"]["$file"], "local_files/here.png");
    assert_eq!(b[1]["attrs"]["$file"], "local_files/also.png");
    assert_eq!(b[2]["attrs"]["$file"], "local_files/also.png");
    // Missing or outside: the file's name as text, no image block.
    for (n, block) in b.iter().enumerate().take(10).skip(3) {
        assert_eq!(block["type"], "paragraph", "block {n}");
    }
    assert_eq!(text_of(&b[3]), "gone.png");
    assert_eq!(text_of(&b[4]), "shared.png");
    assert_eq!(text_of(&b[8]), "shared.png");
    assert_eq!(text_of(&b[10]), "Report.pdf");
    assert_eq!(text_of(&b[11]), "other.pdf");
    assert_eq!(b.len(), 12);
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::MissingFile),
        [
            Some("gone.png"),
            Some("network path"),
            Some("local path"),
            Some("image")
        ]
    );
    assert!(plan.remote_files.is_empty());
    // Nothing of the outside paths reaches a placeholder or a warning detail.
    let all = serde_json::to_string(&plan.pages[1].content).unwrap();
    assert!(!all.contains("nas"));
    assert!(!all.contains("Users"));
    assert!(
        plan.warnings
            .iter()
            .all(|w| !w.detail.as_deref().unwrap_or("").contains("nas"))
    );
}

// ----- the space named after the source --------------------------------------------------------

#[test]
fn the_views_outside_a_space_go_to_a_space_named_after_the_source() {
    let build = || {
        let mut backup = Backup::new();
        backup.space("sp1", "Space");
        backup.page("loose", "Loose page", WS, &Doc::new());
        backup
    };
    let plan = build().plan_named(Some("appflowy backup ben"));
    let other = index_of(&plan, "appflowy backup ben");
    assert_eq!(plan.space_roots, [0, other]);
    assert_eq!(
        plan.pages[index_of(&plan, "Loose page")].parent,
        Some(other)
    );
    // Without a name (or with one that is nothing), the folder is "Other pages".
    for name in [None, Some(""), Some("  \n ")] {
        let plan = build().plan_named(name);
        assert!(titles(&plan).contains(&"Other pages"), "{name:?}");
    }
    // The name is a title: one line, no control characters, cut.
    let plan = build().plan_named(Some(&format!("Backup\n\u{0}name {}", "x".repeat(600))));
    assert!(
        plan.pages[plan.space_roots[1]]
            .title
            .starts_with("Backup name x")
    );
    assert_eq!(plan.pages[plan.space_roots[1]].title.chars().count(), 500);
    // No page outside a space: the name is not used.
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    let plan = backup.plan_named(Some("zip name"));
    assert_eq!(titles(&plan), ["Space"]);
}

// ----- everything resolves ---------------------------------------------------------------------

#[test]
fn the_pages_of_a_container_resolve_to_documents() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    container(&mut backup, "sp1", &small_db("db1", 0));
    let mut doc = Doc::new();
    doc.add("root", "grid", json!({"view_id": "emb"}), None);
    backup.page("p", "Source", "sp1", &doc);
    let plan: Plan = backup.plan();
    let mut fake = Fake::default();
    for n in 0..plan.pages.len() {
        fake.pages.push(kb_types::PageId::from_random_bytes(
            [u8::try_from(n).unwrap() + 1; 16],
        ));
    }
    fake.properties.insert(
        "status".into(),
        kb_types::PropertyId::from_random_bytes([50; 16]),
    );
    for page in &plan.pages {
        let (json, _, _) = resolve_and_load(page, &fake);
        assert!(!json.to_string().contains('$'));
    }
    assert!(codes(&plan).is_empty() || codes(&plan) == [ImportWarningCode::UnsupportedContent]);
}
