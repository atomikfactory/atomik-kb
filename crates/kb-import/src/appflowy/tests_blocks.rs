//! Tests of the block mapping of documents (design, section 3) and of what `resolve` makes of
//! them: every mapped block loads into a page document.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::{AttachmentId, ImportWarningCode, PageId};
use serde_json::{Value, json};

use super::tests::{Backup, Doc, WS, blocks, warnings_of};
use crate::resolve_tests::{Fake, resolve_and_load};
use crate::{Plan, Resolver};

/// A plan of one space with one page whose document is `doc`; the page is index 1.
fn plan_of(doc: &Doc) -> Plan {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Page", "sp1", doc);
    backup.plan()
}

/// A resolver that knows every planned page and every file, so that nothing falls back to text.
fn knows_everything(plan: &Plan) -> Fake {
    let mut fake = Fake::default();
    for n in 0..plan.pages.len() {
        fake.pages.push(PageId::from_random_bytes(
            [u8::try_from(n % 200).unwrap() + 1; 16],
        ));
    }
    for (n, path) in plan.needed_files.iter().enumerate() {
        fake.attachments.insert(
            path.clone(),
            AttachmentId::from_random_bytes([u8::try_from(n % 200).unwrap() + 1; 16]),
        );
    }
    for (n, remote) in plan.remote_files.iter().enumerate() {
        fake.attachments.insert(
            remote.key.clone(),
            AttachmentId::from_random_bytes([u8::try_from(n % 200).unwrap() + 100; 16]),
        );
    }
    fake
}

/// The text of a node and everything inside it.
fn text_of(value: &Value) -> String {
    crate::json::plain_text(value)
}

fn kinds(value: &Value) -> Vec<&str> {
    value["content"]
        .as_array()
        .map(|c| c.iter().filter_map(|n| n["type"].as_str()).collect())
        .unwrap_or_default()
}

#[test]
fn a_toggle_heading_keeps_its_level_clamped_to_three() {
    let mut doc = Doc::new();
    let ids = [
        // Levels 1 to 3 as they are, a missing or unusable level is 1 (the default of AppFlowy),
        // a deeper one is 3; a toggle list never has a level, whatever its data says.
        doc.text("root", "toggle_heading", json!({"level": 1}), "One"),
        doc.text(
            "root",
            "toggle_heading",
            json!({"level": 2, "collapsed": true}),
            "Two",
        ),
        doc.text("root", "toggle_heading", json!({"level": 3}), "Three"),
        doc.text("root", "toggle_heading", json!({}), "None"),
        doc.text("root", "toggle_heading", json!({"level": "x"}), "Text"),
        doc.text("root", "toggle_heading", json!({"level": 9}), "Deep"),
        doc.text("root", "toggle_list", json!({"level": 2}), "A list"),
    ];
    doc.text(&ids[1], "paragraph", json!({}), "inside");
    let plan = plan_of(&doc);
    let b = blocks(&plan, 1);
    let levels: Vec<Option<i64>> = b
        .iter()
        .map(|toggle| toggle["content"][0]["attrs"]["level"].as_i64())
        .collect();
    assert_eq!(
        levels,
        [Some(1), Some(2), Some(3), Some(1), Some(1), Some(3), None]
    );
    // The open flag and the content are the toggle's, as for a toggle list.
    assert_eq!(b[0]["attrs"]["open"], true);
    assert!(b[1]["attrs"].is_null());
    assert_eq!(text_of(&b[1]["content"][0]), "Two");
    assert_eq!(text_of(&b[1]["content"][1]), "inside");
}

#[test]
fn text_blocks_lists_and_quotes() {
    let mut doc = Doc::new();
    doc.text("root", "paragraph", json!({}), "Plain");
    for (level, text) in [(1, "H1"), (2, "H2"), (3, "H3"), (6, "H6")] {
        doc.text("root", "heading", json!({"level": level}), text);
    }
    let a = doc.text("root", "bulleted_list", json!({}), "a");
    doc.text(&a, "bulleted_list", json!({}), "a1");
    doc.text("root", "bulleted_list", json!({}), "b");
    doc.text("root", "numbered_list", json!({"number": 3}), "n1");
    doc.text("root", "numbered_list", json!({}), "n2");
    doc.text("root", "paragraph", json!({}), "between");
    doc.text("root", "numbered_list", json!({"number": 1}), "m1");
    doc.text("root", "todo_list", json!({"checked": true}), "t1");
    doc.text("root", "todo_list", json!({"checked": false}), "t2");
    doc.text("root", "bulleted_list", json!({}), "after tasks");
    let q = doc.text("root", "quote", json!({}), "quoted");
    doc.text(&q, "paragraph", json!({}), "inside");
    let p = doc.text("root", "paragraph", json!({}), "parent");
    doc.text(&p, "paragraph", json!({}), "child");
    doc.add("root", "divider", json!({}), None);
    doc.add("root", "outline", json!({}), None);
    doc.add("root", "math_equation", json!({"formula": "E=mc^2"}), None);
    doc.text(
        "root",
        "code",
        json!({"language": "Rust"}),
        "fn main() {}\nlet x = 1;",
    );

    let plan = plan_of(&doc);
    let b = blocks(&plan, 1);
    let types: Vec<&str> = b.iter().filter_map(|n| n["type"].as_str()).collect();
    assert_eq!(
        types,
        [
            "paragraph",
            "heading",
            "heading",
            "heading",
            "heading",
            "bulletList",
            "orderedList",
            "paragraph",
            "orderedList",
            "taskList",
            "bulletList",
            "blockquote",
            "paragraph",
            "paragraph",
            "horizontalRule",
            "tableOfContents",
            "mathBlock",
            "codeBlock"
        ]
    );
    // Heading levels are clamped to the schema's three.
    let levels: Vec<i64> = b[1..5]
        .iter()
        .map(|h| h["attrs"]["level"].as_i64().unwrap())
        .collect();
    assert_eq!(levels, [1, 2, 3, 3]);
    // Consecutive items of one kind form one list; nested items nest.
    let items = b[5]["content"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(kinds(&items[0]), ["paragraph", "bulletList"]);
    assert_eq!(text_of(&items[0]["content"][1]), "a1");
    // An ordered list starts where its first item says; a later list starts at 1.
    assert_eq!(b[6]["attrs"]["start"], 3);
    assert_eq!(b[6]["content"].as_array().unwrap().len(), 2);
    assert!(b[8].get("attrs").is_none());
    let tasks = b[9]["content"].as_array().unwrap();
    assert_eq!(tasks[0]["type"], "taskItem");
    assert_eq!(tasks[0]["attrs"]["checked"], true);
    assert_eq!(tasks[1]["attrs"]["checked"], false);
    // A quote holds its text and its children.
    assert_eq!(kinds(&b[11]), ["paragraph", "paragraph"]);
    // A paragraph's children follow it.
    assert_eq!(text_of(&b[12]), "parent");
    assert_eq!(text_of(&b[13]), "child");
    assert_eq!(b[16]["content"][0]["text"], "E=mc^2");
    assert_eq!(b[17]["attrs"]["language"], "rust");
    assert_eq!(b[17]["content"][0]["text"], "fn main() {}\nlet x = 1;");

    let fake = knows_everything(&plan);
    let (_, warnings, _) = resolve_and_load(&plan.pages[1], &fake);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn callouts_toggles_and_page_nesting() {
    let mut doc = Doc::new();
    let c = doc.text(
        "root",
        "callout",
        json!({"icon": "🔥", "bgColor": "0xFFFFE6E6"}),
        "Heads up",
    );
    doc.text(&c, "paragraph", json!({}), "more");
    doc.text("root", "callout", json!({}), "plain callout");
    let open = doc.text(
        "root",
        "toggle_list",
        json!({"collapsed": false}),
        "Open toggle",
    );
    doc.text(&open, "paragraph", json!({}), "shown");
    let shut = doc.text(
        "root",
        "toggle_list",
        json!({"collapsed": true}),
        "Shut toggle",
    );
    doc.text(&shut, "paragraph", json!({}), "hidden");
    let heading = doc.text(
        "root",
        "toggle_heading",
        json!({"level": 2, "collapsed": false}),
        "Heading toggle",
    );
    doc.text(&heading, "bulleted_list", json!({}), "item");
    let page = doc.add("root", "page", json!({"selected_icon": null}), None);
    doc.text(&page, "paragraph", json!({}), "from a nested page");

    let plan = plan_of(&doc);
    let b = blocks(&plan, 1);
    assert_eq!(b[0]["type"], "callout");
    assert_eq!(b[0]["attrs"]["icon"], "🔥");
    assert_eq!(b[0]["attrs"]["color"], "red");
    assert_eq!(kinds(&b[0]), ["paragraph", "paragraph"]);
    // Without an icon the default one.
    assert_eq!(b[1]["attrs"]["icon"], "💡");
    assert!(b[1]["attrs"].get("color").is_none());
    // A toggle: a summary and its content; open unless collapsed.
    assert_eq!(b[2]["type"], "details");
    assert_eq!(b[2]["attrs"]["open"], true);
    assert_eq!(text_of(&b[2]["content"][0]), "Open toggle");
    assert_eq!(text_of(&b[2]["content"][1]), "shown");
    assert!(b[3].get("attrs").is_none());
    // A toggle list is a plain toggle; a toggle heading carries its level (ADR-0040).
    assert!(b[2]["content"][0].get("attrs").is_none());
    assert_eq!(b[4]["type"], "details");
    assert_eq!(b[4]["content"][0]["attrs"], json!({"level": 2}));
    assert_eq!(text_of(&b[4]["content"][0]), "Heading toggle");
    assert_eq!(kinds(&b[4]["content"][1]), ["bulletList"]);
    // A page nested in a document: its children, inlined.
    assert_eq!(b[5]["type"], "paragraph");
    assert_eq!(text_of(&b[5]), "from a nested page");
    assert_eq!(b.len(), 6);

    let (_, warnings, _) = resolve_and_load(&plan.pages[1], &knows_everything(&plan));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn tables_columns_and_links() {
    let mut doc = Doc::new();
    // A simple table with a header row and a short second row.
    let table = doc.add(
        "root",
        "simple_table",
        json!({"enable_header_row": true}),
        None,
    );
    let r1 = doc.add(&table, "simple_table_row", json!({}), None);
    for text in ["H1", "H2"] {
        let cell = doc.add(&r1, "simple_table_cell", json!({}), None);
        doc.text(&cell, "paragraph", json!({}), text);
    }
    let r2 = doc.add(&table, "simple_table_row", json!({}), None);
    let cell = doc.add(&r2, "simple_table_cell", json!({}), None);
    doc.text(&cell, "paragraph", json!({}), "c1");
    doc.text(&cell, "bulleted_list", json!({}), "list in a cell");
    // The older table, placed by position; one cell missing.
    let legacy = doc.add("root", "table", json!({"rowsLen": 2, "colsLen": 2}), None);
    for (row, col, text) in [(0, 0, "a"), (0, 1, "b"), (1, 1, "d")] {
        let cell = doc.add(
            &legacy,
            "table/cell",
            json!({"rowPosition": row, "colPosition": col}),
            None,
        );
        doc.text(&cell, "paragraph", json!({}), text);
    }
    // Columns: two, one, seven.
    for count in [2, 1, 7] {
        let columns = doc.add("root", "simple_columns", json!({}), None);
        for n in 0..count {
            let column = doc.add(&columns, "simple_column", json!({"width": 100}), None);
            doc.text(&column, "paragraph", json!({}), &format!("col {n}"));
        }
    }
    doc.add(
        "root",
        "link_preview",
        json!({"url": "https://example.com/a"}),
        None,
    );
    doc.add("root", "embed_link", json!({"url": "not a url"}), None);

    let plan = plan_of(&doc);
    let b = blocks(&plan, 1);
    // The simple table: the first row is the header, short rows are padded, cells hold blocks.
    assert_eq!(b[0]["type"], "table");
    let rows = b[0]["content"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(kinds(&rows[0]), ["tableHeader", "tableHeader"]);
    assert_eq!(kinds(&rows[1]), ["tableCell", "tableCell"]);
    assert_eq!(kinds(&rows[1]["content"][0]), ["paragraph", "bulletList"]);
    assert_eq!(
        rows[1]["content"][1]["content"][0],
        json!({"type": "paragraph"})
    );
    // The legacy table is placed by position and padded.
    let rows = b[1]["content"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(text_of(&rows[0]["content"][1]), "b");
    assert_eq!(text_of(&rows[1]["content"][0]), "");
    assert_eq!(text_of(&rows[1]["content"][1]), "d");
    assert_eq!(kinds(&rows[0]), ["tableCell", "tableCell"]);
    // Two columns are a column list; one column is just its blocks; seven become five.
    assert_eq!(b[2]["type"], "columnList");
    assert_eq!(b[2]["content"].as_array().unwrap().len(), 2);
    assert_eq!(b[3]["type"], "paragraph");
    assert_eq!(text_of(&b[3]), "col 0");
    assert_eq!(b[4]["type"], "columnList");
    assert_eq!(b[4]["content"].as_array().unwrap().len(), 5);
    assert_eq!(b[4]["content"][4]["content"].as_array().unwrap().len(), 3);
    // Link previews and embeds are paragraphs with the URL, linked when it can be.
    assert_eq!(
        b[5]["content"][0]["marks"][0],
        json!({"type": "link", "attrs": {"href": "https://example.com/a"}})
    );
    assert_eq!(
        b[6],
        json!({"type": "paragraph", "content": [{"type": "text", "text": "not a url"}]})
    );

    let (_, warnings, _) = resolve_and_load(&plan.pages[1], &knows_everything(&plan));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn ai_meetings_become_headings_and_a_transcript_toggle() {
    let mut doc = Doc::new();
    let meeting = doc.add(
        "root",
        "ai_meeting",
        json!({"title": "x", "created_at": 1}),
        None,
    );
    let summary = doc.add(&meeting, "ai_meeting_summary", json!({}), None);
    doc.text(&summary, "paragraph", json!({}), "The summary");
    let notes = doc.add(&meeting, "ai_meeting_notes", json!({}), None);
    doc.add(
        &notes,
        "paragraph",
        json!({"delta": [{"insert": "My notes"}]}),
        None,
    );
    let transcript = doc.add(&meeting, "ai_meeting_transcription", json!({}), None);
    for (speaker, at, text) in [("1", 75, "Hello there"), ("2", 3, "Hi")] {
        let turn = doc.add(
            &transcript,
            "ai_meeting_speaker",
            json!({"speaker_id": speaker, "timestamp": at}),
            None,
        );
        doc.text(&turn, "paragraph", json!({}), text);
    }
    // An empty meeting section adds nothing.
    let empty = doc.add("root", "ai_meeting", json!({}), None);
    doc.add(&empty, "ai_meeting_summary", json!({}), None);

    let plan = plan_of(&doc);
    let b = blocks(&plan, 1);
    let types: Vec<&str> = b.iter().filter_map(|n| n["type"].as_str()).collect();
    assert_eq!(
        types,
        ["heading", "paragraph", "heading", "paragraph", "details"]
    );
    assert_eq!(text_of(&b[0]), "Summary");
    assert_eq!(text_of(&b[1]), "The summary");
    assert_eq!(text_of(&b[2]), "Notes");
    assert_eq!(text_of(&b[3]), "My notes");
    assert_eq!(text_of(&b[4]["content"][0]), "Transcript");
    let turns: Vec<String> = b[4]["content"][1]["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(text_of)
        .collect();
    assert_eq!(
        turns,
        ["[01:15] Speaker 1: Hello there", "[00:03] Speaker 2: Hi"]
    );
    // The transcript starts closed.
    assert!(b[4].get("attrs").is_none());
    let (_, warnings, _) = resolve_and_load(&plan.pages[1], &knows_everything(&plan));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn unknown_blocks_are_text_with_one_warning_per_page_and_type() {
    let mut doc = Doc::new();
    let one = doc.text("root", "future_widget", json!({}), "kept text");
    doc.text(&one, "paragraph", json!({}), "its child");
    doc.text("root", "future_widget", json!({}), "second");
    doc.add("root", "other_widget", json!({}), None);
    let plan = plan_of(&doc);
    let b = blocks(&plan, 1);
    assert_eq!(b.len(), 3);
    assert_eq!(text_of(&b[0]), "kept text");
    assert_eq!(text_of(&b[1]), "its child");
    assert_eq!(text_of(&b[2]), "second");
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent),
        [Some("icon"), Some("future_widget"), Some("other_widget")]
    );
    // The same type on another page is reported again (once per page and type).
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    for id in ["p1", "p2"] {
        let mut doc = Doc::new();
        doc.text("root", "future_widget", json!({}), "x");
        backup.page(id, id, "sp1", &doc);
    }
    let plan = backup.plan();
    let widgets = warnings_of(&plan, ImportWarningCode::UnsupportedContent)
        .into_iter()
        .filter(|d| *d == Some("future_widget"))
        .count();
    assert_eq!(widgets, 2);
}

#[test]
fn text_attributes_become_marks_references_and_inline_nodes() {
    let mut doc = Doc::new();
    doc.add(
        "root",
        "paragraph",
        json!({}),
        Some(json!([
            {"insert": "bold", "attributes": {"bold": true}},
            {"insert": " "},
            {"insert": "all", "attributes": {"bold": true, "italic": true, "underline": true, "strikethrough": true}},
            {"insert": " "},
            {"insert": "code", "attributes": {"code": true, "bold": true}},
            {"insert": " "},
            {"insert": "site", "attributes": {"href": "https://example.com/x"}},
            {"insert": " "},
            {"insert": "bare", "attributes": {"href": "example.com/y"}},
            {"insert": " "},
            {"insert": "bad", "attributes": {"href": "javascript:alert(1)"}},
            {"insert": " "},
            {"insert": "red", "attributes": {"font_color": "0xFFFF0000", "bg_color": "0xFFFFFF00"}},
            {"insert": " "},
            {"insert": "black", "attributes": {"font_color": "0xFF000000"}},
            {"insert": " "},
            {"insert": "$", "attributes": {"formula": "x^2+1"}},
            {"insert": " "},
            {"insert": "$", "attributes": {"mention": {"type": "date", "date": "2026-10-07T00:00:00.000", "include_time": false}}},
            {"insert": "$", "attributes": {"mention": {"type": "date", "date": "2026-10-07T14:30:00.000", "include_time": true, "reminder_id": "r"}}},
            {"insert": "$", "attributes": {"mention": {"type": "date", "date": "never"}}},
            {"insert": " "},
            {"insert": "$", "attributes": {"mention": {"type": "externalLink", "url": "https://example.com/ext"}}},
            {"insert": " "},
            {"insert": "$", "attributes": {"mention": {"type": "person", "person_name": "Ann"}}},
            {"insert": "line one\nline two\r\nline three"},
            {"insert": "\n"}
        ])),
    );
    let plan = plan_of(&doc);
    let inline = blocks(&plan, 1)[0]["content"].as_array().unwrap().clone();
    let node = |text: &str| -> &Value {
        inline
            .iter()
            .find(|n| n["text"] == text)
            .unwrap_or_else(|| panic!("no text {text}: {inline:#?}"))
    };
    let marks = |text: &str| -> Vec<String> {
        node(text)["marks"]
            .as_array()
            .map(|m| {
                m.iter()
                    .map(|m| m["type"].as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(marks("bold"), ["bold"]);
    assert_eq!(marks("all"), ["bold", "italic", "underline", "strike"]);
    // Code excludes every other mark.
    assert_eq!(marks("code"), ["code"]);
    assert_eq!(
        node("site")["marks"][0],
        json!({"type": "link", "attrs": {"href": "https://example.com/x"}})
    );
    // A web address without a scheme is https; an unsafe one is plain text.
    assert_eq!(
        node("bare")["marks"][0],
        json!({"type": "link", "attrs": {"href": "https://example.com/y"}})
    );
    assert!(node("bad").get("marks").is_none());
    // Colours: the text colour and the highlight, by palette name; black is no colour.
    assert_eq!(
        node("red")["marks"],
        json!([{"type": "textColor", "attrs": {"color": "red"}}, {"type": "highlight", "attrs": {"color": "yellow"}}])
    );
    assert!(node("black").get("marks").is_none());
    let nodes_of =
        |kind: &str| -> Vec<&Value> { inline.iter().filter(|n| n["type"] == kind).collect() };
    assert_eq!(nodes_of("mathInline")[0]["attrs"]["latex"], "x^2+1");
    let dates = nodes_of("dateMention");
    assert_eq!(dates.len(), 2);
    assert_eq!(dates[0]["attrs"], json!({"date": "2026-10-07"}));
    assert_eq!(
        dates[1]["attrs"],
        json!({"date": "2026-10-07", "time": "14:30"})
    );
    // The unreadable date, the link and the person are their text.
    assert!(inline.iter().any(|n| n["text"] == "Date"));
    assert_eq!(
        node("https://example.com/ext")["marks"][0],
        json!({"type": "link", "attrs": {"href": "https://example.com/ext"}})
    );
    assert!(inline.iter().any(|n| n["text"] == "@Ann"));
    // Line breaks are hard breaks; the newline at the very end is dropped.
    assert_eq!(nodes_of("hardBreak").len(), 2);
    assert_eq!(inline.last().unwrap()["text"], "line three");

    let (_, warnings, _) = resolve_and_load(&plan.pages[1], &knows_everything(&plan));
    assert!(warnings.is_empty(), "{warnings:?}");
    // Unsafe link: reported.
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnresolvedLink),
        [Some("javascript")]
    );
}

#[test]
fn text_comes_from_the_text_map_or_inline_data() {
    let mut doc = Doc::new();
    doc.text("root", "paragraph", json!({}), "from the text map");
    doc.add(
        "root",
        "paragraph",
        json!({"delta": [{"insert": "inline"}]}),
        None,
    );
    // The delta as a string, as the text map keeps it.
    doc.add(
        "root",
        "paragraph",
        json!({"delta": "[{\"insert\":\"as a string\"}]"}),
        None,
    );
    // The data as a string, too.
    doc.add(
        "root",
        "heading",
        json!("{\"level\":2,\"delta\":[{\"insert\":\"h\"}]}"),
        None,
    );
    let plan = plan_of(&doc);
    let b = blocks(&plan, 1);
    let texts: Vec<String> = b.iter().map(text_of).collect();
    assert_eq!(texts, ["from the text map", "inline", "as a string", "h"]);
    assert_eq!(b[3]["attrs"]["level"], 2);
}

#[test]
fn resolve_turns_unresolved_files_and_pages_into_text_and_links() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.local_file("aaaa.pdf");
    let mut doc = Doc::new();
    doc.add(
        "root",
        "file",
        json!({"url": "p1/aaaa.pdf", "url_type": 2, "name": "Report.pdf"}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "https://h.example/one.png", "align": "right"}),
        None,
    );
    doc.add(
        "root",
        "image",
        json!({"url": "https://h.example/two.png"}),
        None,
    );
    doc.add("root", "sub_page", json!({"view_id": "p1"}), None);
    backup.page("p1", "Page", "sp1", &doc);
    let plan = backup.plan();

    // Everything resolves: the images are attachments, the file too.
    let fake = knows_everything(&plan);
    let (json, warnings, _) = resolve_and_load(&plan.pages[1], &fake);
    assert!(warnings.is_empty(), "{warnings:?}");
    let types: Vec<&str> = json["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["type"].as_str())
        .collect();
    assert_eq!(types, ["file", "image", "image", "pageLink"]);
    assert_eq!(json["content"][1]["attrs"]["align"], "right");
    // The `$` attributes are gone.
    assert!(!json.to_string().contains('$'));

    // Nobody downloaded the first image (no attachment for its key): a link and `RemoteImage`;
    // the file is missing: its name as text and `MissingFile`; the page link falls back too.
    let mut partial = Fake::default();
    partial.pages.push(PageId::from_random_bytes([1; 16]));
    partial.attachments.insert(
        plan.remote_files[1].key.clone(),
        AttachmentId::from_random_bytes([5; 16]),
    );
    let (json, warnings, _) = resolve_and_load(&plan.pages[1], &partial);
    let content = json["content"].as_array().unwrap();
    assert_eq!(content[0]["content"][0]["text"], "Report.pdf");
    assert_eq!(
        content[1]["content"][0],
        json!({"type": "text", "marks": [{"type": "link", "attrs": {"href": "https://h.example/one.png"}}],
               "text": "https://h.example/one.png"})
    );
    assert_eq!(content[2]["type"], "image");
    assert_eq!(content[3]["type"], "paragraph");
    let codes: Vec<(ImportWarningCode, Option<&str>)> = warnings
        .iter()
        .map(|w| (w.code, w.detail.as_deref()))
        .collect();
    assert!(
        codes.contains(&(ImportWarningCode::MissingFile, Some("aaaa.pdf")))
            || codes.iter().any(|c| c.0 == ImportWarningCode::MissingFile),
        "{codes:?}"
    );
    assert!(codes.contains(&(
        ImportWarningCode::RemoteImage,
        Some("https://h.example/one.png")
    )));
    assert!(
        codes
            .iter()
            .any(|c| c.0 == ImportWarningCode::UnresolvedLink)
    );
    // The resolver trait is object safe, as `resolve` needs.
    let _: &dyn Resolver = &partial;
    let _ = WS;
}
