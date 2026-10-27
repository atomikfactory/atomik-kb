//! Tests of the databases: fields, cells, rows and the view block (design, section 4.3).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::{ImportWarningCode, OptionColor, OptionId, PageId, PropertyId, PropertyKind};
use serde_json::{Value, json};

use super::archive::row_document_id;
use super::tests::{Backup, Doc, WS, blocks, index_of, titles, warnings_of};
use crate::resolve_tests::{Fake, resolve_and_load};
use crate::{ImportValue, PageTarget, Plan, PlannedProperty};

pub(super) const R1: &str = "10000000-0000-4000-8000-000000000001";
pub(super) const R2: &str = "10000000-0000-4000-8000-000000000002";
const R3: &str = "10000000-0000-4000-8000-000000000003";
const R4: &str = "10000000-0000-4000-8000-000000000004";

pub(super) fn field(id: &str, name: &str, ty: i64, primary: bool, type_options: Value) -> Value {
    json!({"id": id, "name": name, "field_type": ty, "icon": "", "is_primary": primary,
           "type_options": type_options})
}

/// The type options of a select-like field: `content` is a JSON string, as AppFlowy writes it.
pub(super) fn select_options(ty: i64, options: &[(&str, &str, &str)]) -> Value {
    let list: Vec<Value> = options
        .iter()
        .map(|(id, name, color)| json!({"id": id, "name": name, "color": color}))
        .collect();
    json!({ ty.to_string(): {"content": json!({"options": list, "disable_color": false}).to_string()} })
}

pub(super) fn row(id: &str, cells: &[(&str, i64, Value)]) -> Value {
    let cells: serde_json::Map<String, Value> = cells
        .iter()
        .map(|(field, ty, data)| ((*field).to_owned(), json!({"field_type": ty, "data": data})))
        .collect();
    json!({"id": id, "database_id": "db1", "cells": cells, "visibility": true})
}

/// A database with a field of every kind and four rows.
fn everything() -> Value {
    let fields = vec![
        field("fp", "Name", 0, true, json!({"0": {}})),
        field("ft", "Notes", 0, false, json!({})),
        field("fn", "Amount", 1, false, json!({})),
        field("fd", "Due", 2, false, json!({})),
        field(
            "fs",
            "Status",
            3,
            false,
            select_options(
                3,
                &[
                    ("o1", "Todo", "Purple"),
                    ("o2", "Doing", "Orange"),
                    ("o3", "Done", "Lime"),
                ],
            ),
        ),
        field(
            "fm",
            "Labels",
            4,
            false,
            select_options(
                4,
                &[
                    ("l1", "Red thing", "LightPink"),
                    ("l2", "Blue thing", "Aqua"),
                ],
            ),
        ),
        field("fc", "Done?", 5, false, json!({})),
        field("fu", "Link", 6, false, json!({})),
        field("fk", "Steps", 7, false, json!({})),
        field("fe", "Edited", 8, false, json!({})),
        field("fcr", "Created", 9, false, json!({})),
        field("fr", "Related", 10, false, json!({})),
        field("fsu", "Summary", 11, false, json!({})),
        field("ftr", "Translated", 12, false, json!({})),
        field("ftm", "Spent", 13, false, json!({})),
        field("fmd", "Attachments", 14, false, json!({})),
        field("fx", "Odd", 99, false, json!({})),
        field("fh", "Hidden", 0, false, json!({})),
    ];
    let r1 = {
        let mut r = row(
            R1,
            &[
                ("fp", 0, json!("First row")),
                ("ft", 0, json!("some notes")),
                ("fn", 1, json!("12.5")),
                ("fd", 2, json!("1791383400")),
                ("fs", 3, json!("o2")),
                ("fm", 4, json!("l1,l2")),
                ("fc", 5, json!("Yes")),
                ("fu", 6, json!("https://example.com/a")),
                (
                    "fk",
                    7,
                    json!(
                        r#"{"options":[{"id":"a","name":"One"},{"id":"b","name":"Two"}],"selected_option_ids":["b"]}"#
                    ),
                ),
                ("fr", 10, json!(format!("{R2},{R3}"))),
                ("fsu", 11, json!("a summary")),
                ("ftr", 12, json!("une traduction")),
                ("ftm", 13, json!("90")),
                (
                    "fmd",
                    14,
                    json!(
                        r#"[{"url":"v/aaaa.pdf","upload_type":1,"name":"Doc.pdf","file_type":3},{"url":"https://x.example/y.png","upload_type":2,"name":"y","file_type":1}]"#
                    ),
                ),
                ("fx", 99, json!("odd")),
                ("fh", 0, json!("hidden text")),
            ],
        );
        // A date with a time, and the computed fields (never imported).
        r["cells"]["fd"]["include_time"] = json!(true);
        r["cells"]["fe"] = json!({"field_type": 8, "data": "1791383400"});
        r
    };
    let r2 = {
        let mut r = row(
            R2,
            &[
                ("fp", 0, json!("Second row")),
                ("fd", 2, json!("1791324000")),
                ("fs", 3, json!("zzz")),
                ("fc", 5, json!("No")),
                ("fn", 1, json!("not a number")),
            ],
        );
        r["cells"]["fd"]["include_time"] = json!(false);
        r
    };
    let r3 = row(R3, &[("fp", 0, json!("Third row")), ("fs", 3, json!("o3"))]);
    let r4 = row(R4, &[("fp", 0, json!(""))]);
    json!({
        "database_id": "db1",
        "views": [{
            "id": "dbv", "database_id": "db1", "name": "Board", "layout": 1,
            "layout_settings": {"1": {"hide_ungrouped_column": true}},
            "filters": [],
            "group_settings": [{"id": "g", "field_id": "fs", "ty": 3, "content": "",
                                "groups": [{"id": "o1", "visible": true}, {"id": "o3", "visible": false},
                                           {"id": "fs", "visible": false}]}],
            "sorts": [{"id": "s", "field_id": "fn"}],
            "row_orders": [{"id": R3, "height": 60}, {"id": R1, "height": 60}, {"id": R2, "height": 60}],
            "field_orders": [{"id": "fp"}, {"id": "fs"}, {"id": "fn"}, {"id": "fd"}, {"id": "fh"}, {"id": "fe"},
                             {"id": "fm"}, {"id": "ft"}],
            "field_settings": {"fh": {"visibility": 2}, "fn": {"visibility": 1}, "fs": {"visibility": 0}}
        }],
        "fields": fields,
        "rows": [r1, r2, r3, r4],
        "row_metas": {
            R1: {"icon_url": "🍎", "cover": null, "is_document_empty": false, "attachment_count": 0},
            R2: {"icon_url": null, "cover": null, "is_document_empty": true, "attachment_count": 0},
            R3: {"icon_url": "https://example.com/i.png", "cover": null, "is_document_empty": true},
        }
    })
}

/// The archive of `everything`, planned: a space, a board database, row documents for the rows.
fn everything_plan(layout: i64) -> Plan {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.local_file("aaaa.pdf");
    backup.database("dbv", "Tasks", layout, "sp1", &everything());
    let mut doc = Doc::new();
    doc.para("row one content");
    backup.row_document(R1, Some(&doc));
    let mut ignored = Doc::new();
    ignored.para("never shown");
    backup.row_document(R2, Some(&ignored));
    backup.row_document(R3, None);
    backup.plan()
}

fn property<'a>(plan: &'a Plan, name: &str) -> &'a PlannedProperty {
    plan.properties
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| {
            panic!(
                "no property {name}: {:?}",
                plan.properties.iter().map(|p| &p.name).collect::<Vec<_>>()
            )
        })
}

fn value<'a>(plan: &'a Plan, page: usize, name: &str) -> Option<&'a ImportValue> {
    plan.pages[page]
        .properties
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
}

#[test]
fn rows_are_child_pages_in_the_views_order_with_their_documents() {
    let plan = everything_plan(2);
    assert_eq!(
        titles(&plan),
        [
            "Space",
            "Tasks",
            "Third row",
            "First row",
            "Second row",
            "Untitled"
        ]
    );
    let tasks = index_of(&plan, "Tasks");
    for page in 2..6 {
        assert_eq!(plan.pages[page].parent, Some(tasks));
    }
    // The documents of rows are not pages of their own, whichever row they belong to.
    assert_eq!(plan.pages.len(), 6);
    // A row's document is the content of the row; one the row says is empty, or that is not
    // there, gives an empty page.
    let first = index_of(&plan, "First row");
    assert_eq!(
        blocks(&plan, first)[0]["content"][0]["text"],
        "row one content"
    );
    assert_eq!(
        plan.pages[first].source_path,
        format!(
            "collab_jsons/documents/{}.json",
            row_document_id(R1).unwrap()
        )
    );
    assert_eq!(
        blocks(&plan, index_of(&plan, "Second row")),
        &vec![json!({"type": "paragraph"})]
    );
    assert_eq!(
        blocks(&plan, index_of(&plan, "Third row")),
        &vec![json!({"type": "paragraph"})]
    );
    // The icon of a row is an emoji or nothing.
    assert_eq!(plan.pages[first].icon.as_deref(), Some("🍎"));
    assert_eq!(plan.pages[index_of(&plan, "Third row")].icon, None);
}

#[test]
fn fields_become_properties_of_the_right_kinds() {
    let plan = everything_plan(2);
    let kinds: Vec<(&str, PropertyKind)> = plan
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Notes", PropertyKind::Text),
            ("Amount", PropertyKind::Number),
            ("Due", PropertyKind::Date),
            ("Status", PropertyKind::Select),
            ("Labels", PropertyKind::MultiSelect),
            ("Done?", PropertyKind::Checkbox),
            ("Link", PropertyKind::Url),
            ("Steps", PropertyKind::Checklist),
            ("Related", PropertyKind::Relation),
            ("Summary", PropertyKind::Text),
            ("Translated", PropertyKind::Text),
            ("Spent", PropertyKind::Number),
            ("Attachments", PropertyKind::Files),
            ("Odd", PropertyKind::Text),
            ("Hidden", PropertyKind::Text),
        ],
        "the primary field is the title, the computed fields are not imported"
    );
    // Options keep their order and their colours, by AppFlowy's names.
    let status = property(&plan, "Status");
    let options: Vec<(&str, Option<OptionColor>)> = status
        .options
        .iter()
        .map(|o| (o.name.as_str(), o.color))
        .collect();
    assert_eq!(
        options,
        [
            ("Todo", Some(OptionColor::Purple)),
            ("Doing", Some(OptionColor::Orange)),
            ("Done", Some(OptionColor::Green))
        ]
    );
    let labels: Vec<Option<OptionColor>> = property(&plan, "Labels")
        .options
        .iter()
        .map(|o| o.color)
        .collect();
    assert_eq!(labels, [Some(OptionColor::Pink), Some(OptionColor::Blue)]);
    assert!(property(&plan, "Notes").options.is_empty());
    assert!(property(&plan, "Related").config.is_some());
}

#[test]
fn cells_become_values() {
    let plan = everything_plan(2);
    let first = index_of(&plan, "First row");
    let get = |name: &str| value(&plan, first, name).unwrap_or_else(|| panic!("no value {name}"));
    assert_eq!(get("Notes"), &ImportValue::Text("some notes".into()));
    assert_eq!(get("Amount"), &ImportValue::Number(12.5));
    assert_eq!(
        get("Due"),
        &ImportValue::Date {
            date: "2026-10-07".into(),
            time: Some("14:30".into())
        }
    );
    assert_eq!(get("Status"), &ImportValue::Text("Doing".into()));
    assert_eq!(
        get("Labels"),
        &ImportValue::List(vec!["Red thing".into(), "Blue thing".into()])
    );
    assert_eq!(get("Done?"), &ImportValue::Bool(true));
    assert_eq!(
        get("Link"),
        &ImportValue::Text("https://example.com/a".into())
    );
    assert_eq!(
        get("Steps"),
        &ImportValue::Checklist(vec![(false, "One".into()), (true, "Two".into())])
    );
    assert_eq!(get("Summary"), &ImportValue::Text("a summary".into()));
    assert_eq!(
        get("Translated"),
        &ImportValue::Text("une traduction".into())
    );
    assert_eq!(get("Spent"), &ImportValue::Number(90.0));
    assert_eq!(get("Odd"), &ImportValue::Text("odd".into()));
    assert_eq!(get("Hidden"), &ImportValue::Text("hidden text".into()));
    // The relation points at planned rows, in order; computed fields give no value.
    let second = index_of(&plan, "Second row");
    let third = index_of(&plan, "Third row");
    assert_eq!(
        get("Related"),
        &ImportValue::Pages(vec![
            PageTarget::Planned(second),
            PageTarget::Planned(third)
        ])
    );
    assert!(value(&plan, first, "Edited").is_none() && value(&plan, first, "Created").is_none());
    // The media cell: the file of the archive (with its name), the remote one dropped.
    assert_eq!(plan.needed_files, ["local_files/aaaa.pdf"]);
    assert_eq!(plan.needed_file_names, [Some("Doc.pdf".to_owned())]);
    assert_eq!(
        get("Attachments"),
        &ImportValue::Files(vec!["local_files/aaaa.pdf".into()])
    );
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent)
            .into_iter()
            .filter(|d| *d == Some("Attachments"))
            .count(),
        1
    );
    // A date without a time that AppFlowy stored as the evening before (local midnight east of
    // Greenwich) is the next day.
    assert_eq!(
        value(&plan, second, "Due"),
        Some(&ImportValue::Date {
            date: "2026-10-07".into(),
            time: None
        })
    );
    assert_eq!(
        value(&plan, second, "Done?"),
        Some(&ImportValue::Bool(false))
    );
    // Values that do not fit are left out and reported: an option the field lacks, a number
    // that is not one.
    assert!(value(&plan, second, "Status").is_none());
    assert!(value(&plan, second, "Amount").is_none());
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::InvalidValue),
        [Some("Amount"), Some("Status")]
    );
    assert_eq!(
        value(&plan, third, "Status"),
        Some(&ImportValue::Text("Done".into()))
    );
    // The row without a title is Untitled, and has no values.
    let last = index_of(&plan, "Untitled");
    assert!(plan.pages[last].properties.is_empty());
}

#[test]
fn a_board_is_grouped_by_the_group_field_with_the_visible_fields_as_columns() {
    let plan = everything_plan(2);
    let tasks = index_of(&plan, "Tasks");
    let view = &blocks(&plan, tasks)[0];
    assert_eq!(view["type"], "pageList");
    let attrs = &view["attrs"];
    assert_eq!(attrs["$names"], true);
    assert_eq!(attrs["layout"], "board");
    assert_eq!(attrs["source"], json!({"kind": "children"}));
    assert_eq!(attrs["groupBy"], "Status");
    // The hidden columns: the option that is hidden, and the column of the rows without a value.
    assert_eq!(attrs["hiddenGroups"], json!(["Done", "none"]));
    // Visible fields in the view's order: the title is not a column, a hidden field and the
    // computed ones are not, fields the order does not list follow.
    assert_eq!(
        attrs["properties"],
        json!([
            "Status",
            "Amount",
            "Due",
            "Labels",
            "Notes",
            "Done?",
            "Link",
            "Steps",
            "Related",
            "Summary",
            "Translated",
            "Spent",
            "Attachments",
            "Odd"
        ])
    );
    // The view's sort is dropped, once.
    assert_eq!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent)
            .into_iter()
            .filter(|d| *d == Some("filters and sorts"))
            .count(),
        1
    );
}

#[test]
fn a_grid_is_a_table_a_calendar_a_table_with_a_warning() {
    let plan = everything_plan(1);
    let attrs = &blocks(&plan, index_of(&plan, "Tasks"))[0]["attrs"];
    assert_eq!(attrs["layout"], "table");
    assert!(attrs.get("groupBy").is_none());
    assert!(attrs.get("hiddenGroups").is_none());
    assert!(!warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("calendar")));

    let plan = everything_plan(3);
    assert_eq!(
        blocks(&plan, index_of(&plan, "Tasks"))[0]["attrs"]["layout"],
        "table"
    );
    assert!(warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("calendar")));

    // A layout this reader does not know, on a view that names a database.
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    let bare = json!({"database_id": "db1", "views": [], "fields": [], "rows": []});
    backup.database("dbv", "Odd", 7, "sp1", &bare);
    let plan = backup.plan();
    assert_eq!(blocks(&plan, 1)[0]["attrs"]["layout"], "table");
    assert!(warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("layout 7")));
}

#[test]
fn a_board_without_a_select_to_group_by_is_a_table_with_a_warning() {
    let db = json!({"database_id": "db1", "views": [], "rows": [],
        "fields": [field("fp", "Name", 0, true, json!({})), field("fc", "Done", 5, false, json!({}))]});
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("dbv", "Board", 2, "sp1", &db);
    let plan = backup.plan();
    assert_eq!(blocks(&plan, 1)[0]["attrs"]["layout"], "table");
    assert!(warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("board")));
}

#[test]
fn another_view_of_a_database_is_a_page_that_links_to_the_first() {
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    let db = everything();
    backup.database("dbv", "Tasks", 1, "sp1", &db);
    backup.database("dbv2", "Tasks again", 2, "sp1", &db);
    let plan = backup.plan();
    // The rows hang from the first view only.
    let first = index_of(&plan, "Tasks");
    let second = index_of(&plan, "Tasks again");
    assert_eq!(
        plan.pages
            .iter()
            .filter(|p| p.parent == Some(first))
            .count(),
        4
    );
    assert_eq!(
        plan.pages
            .iter()
            .filter(|p| p.parent == Some(second))
            .count(),
        0
    );
    let content = blocks(&plan, second);
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "pageLink");
    assert_eq!(content[0]["attrs"]["$page"]["planned"], first);
    assert!(
        warnings_of(&plan, ImportWarningCode::UnsupportedContent).contains(&Some("linked view"))
    );
}

#[test]
fn a_relation_to_rows_that_are_not_planned_is_text() {
    let db = json!({"database_id": "db1", "views": [],
        "fields": [field("fp", "Name", 0, true, json!({})), field("fr", "Related", 10, false, json!({}))],
        "rows": [row(R1, &[("fp", 0, json!("A")), ("fr", 10, json!(format!("{R2},{R3}")))]),
                 row(R2, &[("fp", 0, json!("B"))])]});
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("dbv", "Db", 1, "sp1", &db);
    let plan = backup.plan();
    // R3 is not a row of any database of the archive: the property is text with the titles of the
    // rows that are.
    assert_eq!(property(&plan, "Related").kind, PropertyKind::Text);
    assert_eq!(property(&plan, "Related").config, None);
    assert_eq!(
        value(&plan, index_of(&plan, "A"), "Related"),
        Some(&ImportValue::Text("B".into()))
    );
}

#[test]
fn a_property_met_with_two_kinds_is_text_and_values_follow() {
    let one = json!({"database_id": "db1", "views": [],
        "fields": [field("fp", "Name", 0, true, json!({})),
                   field("fs", "Status", 3, false, select_options(3, &[("o", "Open", "Green")])),
                   field("fn", "Size", 1, false, json!({}))],
        "rows": [row(R1, &[("fp", 0, json!("One")), ("fs", 3, json!("o")), ("fn", 1, json!("3"))])]});
    let two = json!({"database_id": "db2", "views": [],
        "fields": [field("fp", "Name", 0, true, json!({})),
                   field("fs", "status", 0, false, json!({})),
                   field("fn", "Size", 1, false, json!({}))],
        "rows": [row(R2, &[("fp", 0, json!("Two")), ("fs", 0, json!("free text")), ("fn", 1, json!("4"))])]});
    let mut backup = Backup::new();
    backup.space("sp1", "Space");
    backup.database("v1", "First", 1, "sp1", &one);
    backup.database("v2", "Second", 1, "sp1", &two);
    let plan = backup.plan();
    // One property per name over the import; the kinds disagree: text, without options; the
    // number stays a number.
    assert_eq!(
        plan.properties
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case("status"))
            .count(),
        1
    );
    assert_eq!(property(&plan, "Status").kind, PropertyKind::Text);
    assert!(property(&plan, "Status").options.is_empty());
    assert_eq!(property(&plan, "Size").kind, PropertyKind::Number);
    assert_eq!(
        value(&plan, index_of(&plan, "One"), "Status"),
        Some(&ImportValue::Text("Open".into()))
    );
    assert_eq!(
        value(&plan, index_of(&plan, "Two"), "Status"),
        Some(&ImportValue::Text("free text".into()))
    );
    assert_eq!(
        value(&plan, index_of(&plan, "Two"), "Size"),
        Some(&ImportValue::Number(4.0))
    );
}

#[test]
fn the_view_and_rows_resolve_to_a_page_document() {
    let plan = everything_plan(2);
    let mut fake = Fake::default();
    for n in 0..plan.pages.len() {
        fake.pages.push(PageId::from_random_bytes(
            [u8::try_from(n).unwrap() + 1; 16],
        ));
    }
    let mut ids = Vec::new();
    for (n, property) in plan.properties.iter().enumerate() {
        let id = PropertyId::from_random_bytes([u8::try_from(n).unwrap() + 40; 16]);
        ids.push((property.name.clone(), id));
        fake.properties.insert(property.name.to_lowercase(), id);
        for (m, option) in property.options.iter().enumerate() {
            fake.options.insert(
                (property.name.to_lowercase(), option.name.clone()),
                OptionId::from_random_bytes([u8::try_from(m).unwrap() + 90; 16]),
            );
        }
    }
    for path in &plan.needed_files {
        fake.attachments.insert(
            path.clone(),
            kb_types::AttachmentId::from_random_bytes([3; 16]),
        );
    }
    for page in &plan.pages {
        let (json, warnings, _doc) = resolve_and_load(page, &fake);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!json.to_string().contains('$'));
    }
    let (json, _, _) = resolve_and_load(&plan.pages[index_of(&plan, "Tasks")], &fake);
    let attrs = &json["content"][0]["attrs"];
    assert_eq!(attrs["layout"], "board");
    let status = ids.iter().find(|(n, _)| n == "Status").unwrap().1;
    assert_eq!(attrs["groupBy"], status.to_string());
    let hidden = attrs["hiddenGroups"].as_array().unwrap();
    assert_eq!(hidden.len(), 2);
    assert_eq!(hidden[1], "none");
    assert_eq!(attrs["properties"].as_array().unwrap().len(), 14);
    let _ = WS;
}
