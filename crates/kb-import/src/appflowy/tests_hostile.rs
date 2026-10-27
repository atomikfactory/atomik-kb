//! Hostile archives: whatever the JSON holds, the plan is consistent, nothing panics, and every
//! page resolves into a document that loads and passes its invariants.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use kb_types::{AttachmentId, ImportWarningCode, OptionId, PageId, PropertyId};
use serde_json::{Map, Value, json};

use super::tests::{Backup, Doc, WS, blocks, titles, warnings_of};
use crate::resolve_tests::{Fake, resolve_and_load};
use crate::testing::node_depth;
use crate::{ImportOptions, Plan, SourceFile, plan};

/// A resolver that knows every page, most files and every property and option of the plan.
fn resolver_for(plan: &Plan) -> Fake {
    let mut fake = Fake::default();
    for n in 0..plan.pages.len() {
        fake.pages.push(PageId::from_random_bytes(
            [u8::try_from(n % 250).unwrap() + 1; 16],
        ));
    }
    for (n, path) in plan.needed_files.iter().enumerate() {
        fake.attachments.insert(
            path.clone(),
            AttachmentId::from_random_bytes([u8::try_from(n % 250).unwrap() + 1; 16]),
        );
    }
    for (n, remote) in plan.remote_files.iter().enumerate() {
        if n % 2 == 0 {
            fake.attachments.insert(
                remote.key.clone(),
                AttachmentId::from_random_bytes([77; 16]),
            );
        }
    }
    for (n, property) in plan.properties.iter().enumerate() {
        fake.properties.insert(
            property.name.to_lowercase(),
            PropertyId::from_random_bytes([u8::try_from(n % 250).unwrap() + 1; 16]),
        );
        for (m, option) in property.options.iter().enumerate() {
            fake.options.insert(
                (property.name.to_lowercase(), option.name.clone()),
                OptionId::from_random_bytes([u8::try_from(m % 250).unwrap() + 1; 16]),
            );
        }
    }
    fake
}

/// The plan is consistent and every page of it resolves into a loadable document.
fn check(plan: &Plan) {
    let mut roots = HashSet::new();
    for (index, page) in plan.pages.iter().enumerate() {
        if let Some(parent) = page.parent {
            assert!(parent < index, "a parent comes before its children");
        }
        assert!(!page.title.is_empty() && page.title.chars().count() <= 500);
        assert!(!page.title.contains('\n'));
        assert!(node_depth(&page.content) <= 70, "nesting is bounded");
    }
    for root in &plan.space_roots {
        assert!(*root < plan.pages.len());
        assert_eq!(plan.pages[*root].parent, None);
        assert!(roots.insert(*root), "a root once");
    }
    assert_eq!(plan.needed_file_names.len(), plan.needed_files.len());
    assert_eq!(
        plan.needed_files.iter().collect::<HashSet<_>>().len(),
        plan.needed_files.len()
    );
    for (n, remote) in plan.remote_files.iter().enumerate() {
        assert_eq!(remote.key, format!("remote/{n}"));
        assert!(remote.url.starts_with("https://"));
        assert!(!remote.host.is_empty());
    }
    let fake = resolver_for(plan);
    for page in &plan.pages {
        let (json, _, _) = resolve_and_load(page, &fake);
        assert!(!json.to_string().contains("\"$"), "no placeholder is left");
    }
}

fn plan_files(files: Vec<SourceFile>, binaries: &[String], max_pages: usize) -> Plan {
    plan(
        files,
        binaries,
        &ImportOptions {
            source_name: Some("x".into()),
            max_pages,
        },
    )
}

// ----- documents -------------------------------------------------------------------------------

#[test]
fn broken_documents_are_empty_pages() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    for (id, text) in [
        ("bad-json", "{not json"),
        ("empty", ""),
        ("array", "[1,2,3]"),
        ("no-blocks", r#"{"page_id": "root"}"#),
        ("no-root", r#"{"page_id": "x", "blocks": {}, "meta": {}}"#),
        (
            "blocks-array",
            r#"{"page_id": "root", "blocks": [], "meta": {}}"#,
        ),
        (
            "meta-string",
            r#"{"page_id": "root", "blocks": {"root": {"ty": "page", "parent": "", "children": "c"}}, "meta": "x"}"#,
        ),
    ] {
        backup.view(id, id, 0, "sp1", None, None);
        backup.document(id, text);
    }
    let plan = backup.plan();
    check(&plan);
    assert_eq!(plan.pages.len(), 8);
    for page in 1..8 {
        assert_eq!(
            blocks(&plan, page),
            &vec![json!({"type": "paragraph"})],
            "page {page}"
        );
    }
    // Unreadable documents are reported; a page whose root has no children is simply empty.
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent)
            .into_iter()
            .filter(|d| *d == Some("document"))
            .count(),
        6
    );
}

#[test]
fn cycles_duplicates_and_dangling_children_are_cut() {
    let mut doc = Doc::new();
    let a = doc.text("root", "bulleted_list", json!({}), "a");
    let b = doc.text(&a, "bulleted_list", json!({}), "b");
    let mut value: Value = serde_json::from_str(&doc.json()).unwrap();
    let children = value["meta"]["children_map"].as_object_mut().unwrap();
    // b lists a (a cycle) and itself; a lists b twice and a block that does not exist; the root
    // lists a again.
    children.insert(format!("ch-{b}"), json!([a, b, "ghost"]));
    children.insert(format!("ch-{a}"), json!([b, b, "ghost", 7, null]));
    children.insert("ch-root".into(), json!([a, a, "root"]));
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.view("p1", "Page", 0, "sp1", None, None);
    backup.document("p1", &value.to_string());
    let plan = backup.plan();
    check(&plan);
    let b = blocks(&plan, 1);
    assert_eq!(b.len(), 1);
    assert_eq!(b[0]["type"], "bulletList");
    // a, once, with b once inside it.
    assert_eq!(b[0]["content"].as_array().unwrap().len(), 1);
    assert_eq!(b[0]["content"][0]["content"].as_array().unwrap().len(), 2);
}

/// A document whose blocks nest `depth` levels deep through blocks of one kind.
fn deep(kind: &str, depth: usize) -> Doc {
    let mut doc = Doc::new();
    let mut parent = "root".to_owned();
    for n in 0..depth {
        parent = doc.text(&parent, kind, json!({}), &format!("level {n}"));
    }
    doc
}

#[test]
fn huge_nesting_is_flattened_not_followed() {
    for kind in [
        "bulleted_list",
        "numbered_list",
        "todo_list",
        "quote",
        "callout",
        "toggle_list",
        "paragraph",
        "heading",
    ] {
        let mut backup = Backup::new();
        backup.space("sp1", "Space");
        backup.page("p1", "Deep", "sp1", &deep(kind, 5_000));
        let plan = backup.plan();
        check(&plan);
        // The whole depth is not followed (the recursion is bounded), the first levels are kept.
        let text = plan.pages[1].content.to_string();
        assert!(text.contains("level 0"), "{kind}");
        assert!(text.contains("level 5"), "{kind}");
    }
}

#[test]
fn nested_tables_and_columns_are_bounded() {
    let mut doc = Doc::new();
    let mut parent = "root".to_owned();
    for _ in 0..300 {
        let table = doc.add(&parent, "simple_table", json!({}), None);
        let row = doc.add(&table, "simple_table_row", json!({}), None);
        let cell = doc.add(&row, "simple_table_cell", json!({}), None);
        let columns = doc.add(&cell, "simple_columns", json!({}), None);
        let c1 = doc.add(&columns, "simple_column", json!({}), None);
        doc.add(&columns, "simple_column", json!({}), None);
        parent = c1;
    }
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Deep", "sp1", &doc);
    check(&backup.plan());
}

#[test]
fn a_wide_and_long_table_is_cut() {
    let mut doc = Doc::new();
    let table = doc.add("root", "simple_table", json!({}), None);
    for r in 0..30 {
        let row = doc.add(&table, "simple_table_row", json!({}), None);
        for c in 0..150 {
            let cell = doc.add(&row, "simple_table_cell", json!({}), None);
            if r == 0 {
                doc.text(&cell, "paragraph", json!({}), &format!("c{c}"));
            }
        }
    }
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Wide", "sp1", &doc);
    let plan = backup.plan();
    check(&plan);
    let rows = blocks(&plan, 1)[0]["content"].as_array().unwrap();
    assert_eq!(rows.len(), 30);
    assert_eq!(rows[0]["content"].as_array().unwrap().len(), 100);
    assert!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("table rows"))
    );
}

#[test]
fn legacy_table_positions_are_bounded() {
    let mut doc = Doc::new();
    let table = doc.add("root", "table", json!({"rowsLen": 4_000_000_000_i64}), None);
    for (r, c) in [
        (0, 0),
        (4_000_000_000_i64, 0),
        (0, 4_000_000_000_i64),
        (-1, 3),
        (2, 2),
    ] {
        let cell = doc.add(
            &table,
            "table/cell",
            json!({"rowPosition": r, "colPosition": c}),
            None,
        );
        doc.text(&cell, "paragraph", json!({}), "x");
    }
    doc.add(&table, "table/cell", json!({"rowPosition": "a"}), None);
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "T", "sp1", &doc);
    let plan = backup.plan();
    check(&plan);
    let rows = blocks(&plan, 1)[0]["content"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn odd_block_data_never_panics() {
    let mut doc = Doc::new();
    for data in [
        json!(null),
        json!(7),
        json!("not json"),
        json!([1, 2]),
        json!({"level": "x", "number": -5, "checked": "maybe", "url": 5, "view_id": [], "formula": {}, "language": 3,
               "collapsed": {}, "icon": 12, "bgColor": 4, "align": {}, "name": [], "delta": 17}),
        json!({"level": 99999999999999_i64, "number": 1e300, "url": "https://", "delta": "[{\"insert\": 5}]"}),
    ] {
        for ty in [
            "paragraph",
            "heading",
            "bulleted_list",
            "numbered_list",
            "todo_list",
            "quote",
            "callout",
            "toggle_list",
            "toggle_heading",
            "code",
            "divider",
            "math_equation",
            "outline",
            "simple_table",
            "table",
            "simple_columns",
            "image",
            "file",
            "link_preview",
            "embed_link",
            "sub_page",
            "grid",
            "board",
            "calendar",
            "ai_meeting",
            "page",
            "unknown",
            "",
        ] {
            doc.add(
                "root",
                ty,
                data.clone(),
                Some(json!([{"insert": 5}, null, {"insert": "x", "attributes": 3}])),
            );
        }
    }
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Odd", "sp1", &doc);
    check(&backup.plan());
}

#[test]
fn hostile_deltas_never_panic() {
    let mut doc = Doc::new();
    for attributes in [
        json!({"mention": "not an object"}),
        json!({"mention": {"type": "page"}}),
        json!({"mention": {"type": "date", "date": 1e300}}),
        json!({"mention": {"type": "date", "date": -99999999999999999_i64, "include_time": true}}),
        json!({"mention": {"type": "date", "date": "99999999999999999999"}}),
        json!({"mention": "{\"type\":\"page\",\"page_id\":\"x\"}"}),
        json!({"formula": null, "bold": "yes", "href": 12}),
        json!({"href": "", "is_page_link": true}),
        json!({"href": format!("https://example.com/{}", "a".repeat(5000))}),
        json!({"font_color": "0xZZ", "bg_color": 77}),
        json!({"font_color": "0xFF", "bg_color": "0xFFFFFFFFFF"}),
    ] {
        doc.add(
            "root",
            "paragraph",
            json!({}),
            Some(json!([{"insert": "text", "attributes": attributes}, {"insert": {"embed": 1}}, {"insert": "\n\n\n"}])),
        );
    }
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.page("p1", "Odd", "sp1", &doc);
    check(&backup.plan());
}

// ----- the view table and the files ------------------------------------------------------------

#[test]
fn a_hostile_view_table_is_survived() {
    let weird = json!({
        "views": {
            "a": 5,
            "b": null,
            "c": {"view_id": "c", "name": 7, "layout": "x", "parent_id": "c", "children": "no", "extra": "{broken",
                  "icon": {"ty": "x", "value": 3}},
            "d": {"view_id": "d", "name": "Self parent", "layout": 0, "parent_id": "d", "children": ["d", "d", 4, null]},
            "e": {"view_id": "e", "name": "\u{0}\n\t\u{202e}Control\r\n", "layout": 0, "parent_id": 12,
                  "children": ["e"], "extra": {"is_space": "true", "cover": 5, "database_id": 9}},
            "f": {"name": "Missing id", "layout": 4.0},
            "g": {"view_id": "g", "name": "G", "layout": 0, "parent_id": "e", "children": [], "extra": "[1]"},
        }
    });
    let files = vec![
        SourceFile {
            path: "metadata.json".into(),
            text: "{}".into(),
        },
        SourceFile {
            path: "relation_map.json".into(),
            text: weird.to_string(),
        },
    ];
    let plan = plan_files(files, &[], 10_000);
    check(&plan);
    assert!(!plan.pages.is_empty());
    assert!(titles(&plan).contains(&"Self parent"));
    // Control characters do not reach a title.
    assert!(
        plan.pages
            .iter()
            .all(|p| !p.title.chars().any(char::is_control))
    );
}

#[test]
fn unsafe_and_odd_paths_are_ignored() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    let mut doc = Doc::new();
    doc.add(
        "root",
        "file",
        json!({"url": "p1/../../etc/passwd", "url_type": 2, "name": "x"}),
        None,
    );
    doc.add(
        "root",
        "file",
        json!({"url": "p1/ok.pdf", "url_type": 2, "name": "\u{0}../ok\n.pdf"}),
        None,
    );
    doc.add("root", "image", json!({"url": "p1/ok.pdf"}), None);
    // A multi-byte character where a scheme would end.
    doc.add(
        "root",
        "image",
        json!({"url": "dat\u{e9}\u{e9}\u{e9}.png"}),
        None,
    );
    doc.add(
        "root",
        "file",
        json!({"url": "\u{1F600}\u{1F600}", "name": "\u{1F600}"}),
        None,
    );
    backup.page("p1", "Files", "sp1", &doc);
    backup.local_file("ok.pdf");
    let (texts, mut binaries) = backup.build();
    binaries.push("local_files/../outside.pdf".into());
    binaries.push("/local_files/abs.pdf".into());
    binaries.push("local_files\\back.pdf".into());
    let plan = plan_files(texts, &binaries, 10_000);
    check(&plan);
    assert_eq!(plan.needed_files, ["local_files/ok.pdf"]);
    // The name is the last segment, without control characters.
    assert_eq!(plan.needed_file_names, [Some("ok.pdf".to_owned())]);
}

#[test]
fn the_page_limit_holds_for_every_limit() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.space("sp2", "Space 2");
    for n in 0..10 {
        backup.page(
            &format!("p{n}"),
            &format!("P{n}"),
            if n % 2 == 0 { "sp1" } else { "sp2" },
            &Doc::new(),
        );
    }
    backup.page("loose", "Loose", WS, &Doc::new());
    let (texts, binaries) = backup.build();
    for limit in 0..16 {
        let plan = plan_files(texts.clone(), &binaries, limit);
        assert!(plan.pages.len() <= limit);
        check(&plan);
    }
}

// ----- databases -------------------------------------------------------------------------------

#[test]
fn hostile_databases_are_survived() {
    let rows: Vec<Value> = (0..50)
        .map(|n| {
            json!({"id": if n % 5 == 0 { "dup".to_owned() } else { format!("not-a-uuid-{n}") },
                   "cells": {"f": {"field_type": 3, "data": "x,y,z,,,"}, "g": 5, "h": null,
                             "i": {"data": {"nested": [1, 2]}}, "j": {"field_type": 7, "data": "[1,2"},
                             "k": {"field_type": 14, "data": 12}, "l": {"field_type": 10, "data": [1, null, "a"]}}})
        })
        .collect();
    let db = json!({
        "database_id": 5,
        "views": [{"id": "dbv", "field_orders": "no", "field_settings": [], "group_settings": [{"field_id": 3, "groups": 4}],
                   "row_orders": [{"id": "dup"}, 5, null], "filters": "x", "sorts": {}}, 7, null],
        "fields": [
            {"id": "f", "name": "Pick", "field_type": 3, "is_primary": "true",
             "type_options": {"3": {"content": "{not json"}}},
            {"id": "g", "name": "", "field_type": "4", "type_options": "{\"4\":{\"content\":{\"options\":[{\"id\":1,\"name\":null,\"color\":5}]}}}"},
            {"id": "h", "name": "h", "field_type": 2},
            {"id": "i", "name": "i", "field_type": 6},
            {"id": "j", "name": "j", "field_type": 7},
            {"id": "k", "name": "k", "field_type": 14},
            {"id": "l", "name": "l", "field_type": 10},
            {"id": "f", "name": "Duplicate id", "field_type": 0},
            5, null, {"name": "no id"}
        ],
        "rows": rows,
        "row_metas": {"dup": {"icon_url": 5, "is_document_empty": "x"}, "not-a-uuid-1": null}
    });
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("dbv", "Db", 2, "sp1", &db);
    let plan = backup.plan();
    check(&plan);
    // The database file that is not JSON gives an empty page.
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.view(
        "dbv",
        "Broken db",
        1,
        "sp1",
        Some(json!({"database_id": "d"})),
        None,
    );
    backup.text_file("collab_jsons/databases/dbv.json", "{{{");
    let plan = backup.plan();
    check(&plan);
    assert_eq!(plan.pages.len(), 2);
    assert!(warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("database")));
}

#[test]
fn many_options_and_fields_are_capped() {
    let options: Vec<Value> = (0..800)
        .map(|n| json!({"id": format!("o{n}"), "name": format!("Option {n}"), "color": "Blue"}))
        .collect();
    let mut fields = vec![json!({"id": "p", "name": "Name", "field_type": 0, "is_primary": true})];
    fields.push(json!({"id": "s", "name": "Many", "field_type": 3,
                       "type_options": {"3": {"content": json!({"options": options}).to_string()}}}));
    for n in 0..400 {
        fields.push(json!({"id": format!("x{n}"), "name": format!("Field {n}"), "field_type": 0}));
    }
    let db = json!({"database_id": "d", "views": [], "fields": fields,
                    "rows": [{"id": "r", "cells": {"p": {"data": "row"}, "s": {"data": "o799", "field_type": 3}}}]});
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("dbv", "Db", 1, "sp1", &db);
    let plan = backup.plan();
    check(&plan);
    assert!(plan.properties.len() <= 200);
    let many = plan.properties.iter().find(|p| p.name == "Many").unwrap();
    assert!(many.options.len() <= 500);
}

// ----- random corruption -----------------------------------------------------------------------

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n as u64).max(1)).unwrap()
    }
}

/// Replaces some values of a JSON tree with others of any type.
fn corrupt(value: &mut Value, rng: &mut Lcg, rate: usize) {
    if rng.below(rate) == 0 {
        *value = match rng.below(12) {
            0 => Value::Null,
            1 => json!(0),
            2 => json!(-1),
            3 => json!(1e300),
            4 => json!(""),
            5 => json!("x".repeat(3_000)),
            6 => json!(true),
            7 => json!([]),
            8 => json!({}),
            9 => json!(["a", 1, null]),
            10 => json!({"a": 1, "ty": "paragraph"}),
            _ => json!("root"),
        };
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items.iter_mut() {
                corrupt(item, rng, rate);
            }
            if !items.is_empty() && rng.below(rate * 4) == 0 {
                items.remove(rng.below(items.len()));
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                corrupt(item, rng, rate);
            }
            if !map.is_empty() && rng.below(rate * 4) == 0 {
                let key = map.keys().nth(rng.below(map.len())).cloned().unwrap();
                map.remove(&key);
            }
        }
        Value::String(text) if rng.below(rate) == 0 && !text.is_empty() => {
            let cut = text
                .char_indices()
                .nth(rng.below(text.chars().count()))
                .map_or(0, |(i, _)| i);
            text.truncate(cut);
        }
        _ => {}
    }
}

/// An archive with a bit of everything.
fn rich_archive() -> (Vec<SourceFile>, Vec<String>) {
    let mut backup = Backup::new();
    backup.space("sp1", "Work");
    backup.space("sp2", "Home");
    backup.local_file("aaaa.pdf");
    let mut doc = Doc::new();
    let h = doc.text("root", "heading", json!({"level": 2}), "Title");
    doc.text(&h, "paragraph", json!({}), "child");
    let l = doc.text("root", "bulleted_list", json!({}), "item");
    doc.text(&l, "todo_list", json!({"checked": true}), "task");
    let q = doc.text("root", "quote", json!({}), "quote");
    doc.text(&q, "numbered_list", json!({"number": 2}), "one");
    doc.text(
        "root",
        "callout",
        json!({"icon": "💡", "bgColor": "0xFFFFE6E6"}),
        "callout",
    );
    doc.text("root", "toggle_list", json!({"collapsed": true}), "toggle");
    doc.text("root", "code", json!({"language": "rust"}), "let x = 1;");
    doc.add("root", "divider", json!({}), None);
    doc.add("root", "math_equation", json!({"formula": "x"}), None);
    doc.add(
        "root",
        "image",
        json!({"url": "https://h.example/1.png", "align": "left"}),
        None,
    );
    doc.add(
        "root",
        "file",
        json!({"url": "p/aaaa.pdf", "url_type": 2, "name": "A.pdf"}),
        None,
    );
    doc.add("root", "sub_page", json!({"view_id": "p2"}), None);
    let table = doc.add(
        "root",
        "simple_table",
        json!({"enable_header_row": true}),
        None,
    );
    let row = doc.add(&table, "simple_table_row", json!({}), None);
    let cell = doc.add(&row, "simple_table_cell", json!({}), None);
    doc.text(&cell, "paragraph", json!({}), "cell");
    let columns = doc.add("root", "simple_columns", json!({}), None);
    for n in 0..2 {
        let column = doc.add(&columns, "simple_column", json!({}), None);
        doc.text(&column, "paragraph", json!({}), &format!("col {n}"));
    }
    let meeting = doc.add("root", "ai_meeting", json!({}), None);
    let transcript = doc.add(&meeting, "ai_meeting_transcription", json!({}), None);
    let speaker = doc.add(
        &transcript,
        "ai_meeting_speaker",
        json!({"speaker_id": 1, "timestamp": 5}),
        None,
    );
    doc.text(&speaker, "paragraph", json!({}), "hello");
    doc.add(
        "root",
        "paragraph",
        json!({}),
        Some(json!([
            {"insert": "a", "attributes": {"bold": true, "href": "https://example.com", "font_color": "0xFFFF0000"}},
            {"insert": "$", "attributes": {"mention": {"type": "page", "page_id": "p2"}}},
            {"insert": "$", "attributes": {"mention": {"type": "date", "date": "2026-10-07"}}},
            {"insert": "$", "attributes": {"formula": "y"}},
        ])),
    );
    backup.page("p1", "Everything", "sp1", &doc);
    backup.page("p2", "Other", "sp2", &Doc::new());
    backup.page("p3", "Loose", WS, &Doc::new());
    let rows: Vec<Value> = (1..=3)
        .map(|n| {
            json!({"id": format!("20000000-0000-4000-8000-00000000000{n}"),
                   "cells": {"p": {"field_type": 0, "data": format!("Row {n}")},
                             "s": {"field_type": 3, "data": "o1"},
                             "r": {"field_type": 10, "data": "20000000-0000-4000-8000-000000000001"},
                             "d": {"field_type": 2, "data": "1791383400", "include_time": true}}})
        })
        .collect();
    let db = json!({
        "database_id": "d1",
        "views": [{"id": "dbv", "layout": 1, "field_orders": [{"id": "p"}, {"id": "s"}, {"id": "d"}],
                   "group_settings": [{"field_id": "s", "groups": [{"id": "o1", "visible": false}]}],
                   "row_orders": [{"id": "20000000-0000-4000-8000-000000000002"}]}],
        "fields": [
            {"id": "p", "name": "Name", "field_type": 0, "is_primary": true},
            {"id": "s", "name": "Status", "field_type": 3,
             "type_options": {"3": {"content": "{\"options\":[{\"id\":\"o1\",\"name\":\"Open\",\"color\":\"Blue\"}]}"}}},
            {"id": "r", "name": "Rel", "field_type": 10},
            {"id": "d", "name": "When", "field_type": 2},
        ],
        "rows": rows,
        "row_metas": {"20000000-0000-4000-8000-000000000001": {"icon_url": "🍎", "is_document_empty": false}},
    });
    backup.database("dbv", "Tasks", 2, "sp1", &db);
    // A container with its embedded view, a template, a row document in the databases folder.
    backup.view(
        "cont",
        "Container",
        1,
        "sp1",
        Some(json!({"database_id": "d2", "is_database_container": true})),
        None,
    );
    backup.view(
        "emb",
        "Embedded",
        1,
        "cont",
        Some(json!({"database_id": "d2", "embedded": true})),
        None,
    );
    let mut second = db.clone();
    second["database_id"] = json!("d2");
    backup.text_file("collab_jsons/databases/emb.json", &second.to_string());
    backup.view(
        "tpl",
        "Template",
        0,
        "sp1",
        Some(json!({"database_row_template": true, "template_id": "t"})),
        None,
    );
    let mut folder_doc = Doc::new();
    folder_doc.para("in the databases folder");
    backup.text_file(
        &format!(
            "collab_jsons/databases/emb/row_documents/{}.json",
            super::archive::row_document_id("20000000-0000-4000-8000-000000000003").unwrap()
        ),
        &folder_doc.json(),
    );
    let mut grid_doc = Doc::new();
    grid_doc.add(
        "root",
        "grid",
        json!({"view_id": "emb", "view_ids": ["cont"], "database_id": "d2", "parent_id": "cont"}),
        None,
    );
    backup.page("pg", "Inline", "sp1", &grid_doc);
    let mut row_doc = Doc::new();
    row_doc.para("row content");
    backup.row_document("20000000-0000-4000-8000-000000000001", Some(&row_doc));
    backup.build()
}

#[test]
fn the_rich_archive_is_consistent() {
    let (texts, binaries) = rich_archive();
    let plan = plan_files(texts, &binaries, 10_000);
    check(&plan);
    assert_eq!(plan.space_roots.len(), 3);
    assert!(plan.pages.len() > 8);
    // The container is one page; the embedded view and the template are none.
    assert!(titles(&plan).contains(&"Container"));
    assert!(!titles(&plan).contains(&"Embedded"));
    assert!(!titles(&plan).contains(&"Template"));
}

#[test]
fn random_corruption_never_breaks_the_plan_or_resolve() {
    let (texts, binaries) = rich_archive();
    for seed in 0..400_u64 {
        let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
        let rate = [6, 12, 25, 60, 150][usize::try_from(seed % 5).unwrap()];
        let files: Vec<SourceFile> = texts
            .iter()
            .map(|f| {
                if rng.below(10) == 0 {
                    // Sometimes a file is cut off in the middle.
                    let cut = f
                        .text
                        .char_indices()
                        .nth(rng.below(f.text.chars().count().max(1)))
                        .map_or(0, |(i, _)| i);
                    return SourceFile {
                        path: f.path.clone(),
                        text: f.text[..cut].to_owned(),
                    };
                }
                match serde_json::from_str::<Value>(&f.text) {
                    Ok(mut value) => {
                        // The two files that make the archive recognisable keep their shape at
                        // the top, so the reader is exercised, not skipped.
                        if f.path == "metadata.json" {
                            return f.clone();
                        }
                        let top_keys: Map<String, Value> =
                            value.as_object().cloned().unwrap_or_default();
                        corrupt(&mut value, &mut rng, rate);
                        if f.path == "relation_map.json"
                            && !value.get("views").is_some_and(Value::is_object)
                        {
                            value = Value::Object(top_keys);
                        }
                        SourceFile {
                            path: f.path.clone(),
                            text: value.to_string(),
                        }
                    }
                    Err(_) => f.clone(),
                }
            })
            .collect();
        let plan = plan_files(files, &binaries, 10_000);
        check(&plan);
    }
}
