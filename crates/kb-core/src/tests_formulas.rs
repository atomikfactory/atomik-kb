//! Formulas (ADR-0055, `docs/architecture/formulas.md`) through `Request`: the configuration and its
//! refusals, the values computed with the time of the core, the properties a formula is not (set,
//! filtered), the CSV and the sidecar of an export, and a Markdown archive round trip that brings the
//! formulas back with the ids of the other workspace. The move to another space is tested with the
//! other moves (`tests_space_move.rs`), the engine in `kb-model`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use kb_model::formula;
use kb_types::{
    ApiError, CsvColumn, ErrorCode, ExportFormat, FormulaBase, FormulaResult, FormulaType, PageId,
    PropertyConfig, PropertyDef, PropertyFilter, PropertyKind, PropertyValue, Request, Response,
    SpaceId,
};
use serde_json::json;

use crate::Core;
use crate::test_support::{create_page, tree};
use crate::test_support_interop::{
    another_core, as_source, export_all, prop_create, prop_create_with, property_named, run_import,
    schema, set_value, setup_with_offset, value_named, values_of,
};

/// 2026-10-07T14:30:00Z in Unix ms.
const OCT_7_1430: i64 = 1_791_383_400_000;

fn formula_config(expression: impl Into<String>) -> Option<PropertyConfig> {
    Some(PropertyConfig::Formula {
        expression: expression.into(),
        result_type: None,
    })
}

fn make_formula(core: &mut Core, space: SpaceId, name: &str, expression: &str) -> PropertyDef {
    // `{Name}` stands for the id of the property of that name.
    let defs = schema(core, space);
    let mut stored = String::new();
    let mut rest = expression;
    while let Some(open) = rest.find('{') {
        stored.push_str(&rest[..open]);
        let close = open + rest[open..].find('}').unwrap();
        let wanted = &rest[open + 1..close];
        stored.push_str(
            &defs
                .iter()
                .find(|def| def.name == wanted)
                .unwrap_or_else(|| panic!("no property {wanted}"))
                .id
                .to_string(),
        );
        rest = &rest[close + 1..];
    }
    stored.push_str(rest);
    prop_create_with(
        core,
        space,
        name,
        PropertyKind::Formula,
        formula_config(stored),
    )
}

fn result_of(core: &mut Core, space: SpaceId, page: PageId, name: &str) -> Option<FormulaResult> {
    match value_named(core, space, page, name) {
        Some(PropertyValue::Formula(result)) => Some(result),
        None => None,
        other => panic!("not a formula value: {other:?}"),
    }
}

fn text(value: &str) -> FormulaResult {
    FormulaResult::Text(value.to_owned())
}

fn refused(core: &mut Core, request: Request) -> ApiError {
    core.dispatch(request).unwrap_err()
}

fn field_of(error: &ApiError) -> &str {
    error.details.as_ref().unwrap()["field"].as_str().unwrap()
}

#[test]
fn a_formula_is_checked_when_written_and_computed_with_the_time_of_the_core() {
    // Ten hours east of Greenwich: at 14:30 UTC it is 00:30 of the next day here.
    let (_fx, mut core, space) = setup_with_offset(OCT_7_1430, 10 * 3600);
    let price = prop_create(&mut core, space, "Price", PropertyKind::Number);
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    let alpha = create_page(&mut core, space, None, "Alpha").id;
    set_value(&mut core, alpha, price.id, PropertyValue::Number(3.0));

    // Refusals name the code and the position, and create nothing.
    for (expression, code) in [
        ("1 +".to_owned(), "syntax"),
        ("prop(\"nothing\")".to_owned(), "unknown_property"),
        (format!("prop(\"{}\") * 2", notes.id), "type"),
    ] {
        let error = refused(
            &mut core,
            Request::PropertyCreate {
                space_id: space,
                name: "Bad".into(),
                kind: PropertyKind::Formula,
                config: formula_config(expression),
            },
        );
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field_of(&error), "config");
        assert!(error.message.contains(code), "{}", error.message);
    }
    assert_eq!(schema(&mut core, space).len(), 3, "Tags, Price, Notes");
    assert_eq!(
        field_of(&refused(
            &mut core,
            Request::PropertyCreate {
                space_id: space,
                name: "Bad".into(),
                kind: PropertyKind::Formula,
                config: None,
            }
        )),
        "config"
    );

    // A good one reads back with its result type and computes on read.
    let double = make_formula(&mut core, space, "Double", "prop(\"{Price}\") * 2");
    assert_eq!(
        double.config,
        Some(PropertyConfig::Formula {
            expression: format!("prop(\"{}\") * 2", price.id),
            result_type: Some(FormulaType {
                base: FormulaBase::Number,
                list: false
            }),
        })
    );
    assert_eq!(
        result_of(&mut core, space, alpha, "Double"),
        Some(FormulaResult::Number(6.0))
    );
    set_value(&mut core, alpha, price.id, PropertyValue::Number(4.5));
    assert_eq!(
        result_of(&mut core, space, alpha, "Double"),
        Some(FormulaResult::Number(9.0))
    );
    // The tree and a page carry it like any value.
    let in_tree = tree(&mut core, space)
        .roots
        .into_iter()
        .find(|node| node.page.id == alpha)
        .unwrap()
        .page;
    assert!(in_tree.properties.contains(&(
        double.id,
        PropertyValue::Formula(FormulaResult::Number(9.0))
    )));

    // `now()` and `today()` are the clock and the offset of the core.
    make_formula(
        &mut core,
        space,
        "Stamp",
        "formatDate(now(), \"YYYY-MM-DD HH:mm\")",
    );
    make_formula(
        &mut core,
        space,
        "Day",
        "formatDate(today(), \"YYYY-MM-DD\")",
    );
    assert_eq!(
        result_of(&mut core, space, alpha, "Stamp"),
        Some(text("2026-10-08 00:30"))
    );
    assert_eq!(
        result_of(&mut core, space, alpha, "Day"),
        Some(text("2026-10-08"))
    );

    // Changing the configuration checks again; a loop is refused with its code.
    let before = schema(&mut core, space);
    let error = refused(
        &mut core,
        Request::PropertyConfigSet {
            space_id: space,
            property_id: double.id,
            config: PropertyConfig::Formula {
                expression: format!("prop(\"{}\")", double.id),
                result_type: None,
            },
        },
    );
    assert_eq!(field_of(&error), "config");
    assert!(error.message.contains("cycle"), "{}", error.message);
    assert_eq!(schema(&mut core, space), before);

    // Nothing sets a formula, nothing searches by one.
    let error = refused(
        &mut core,
        Request::PagePropertySet {
            page_id: alpha,
            property_id: double.id,
            value: Some(PropertyValue::Formula(text("x"))),
        },
    );
    assert_eq!(field_of(&error), "propertyId");
    let error = refused(
        &mut core,
        Request::PagesByProperty {
            space_id: space,
            property_id: double.id,
            filter: PropertyFilter::HasValue,
        },
    );
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(field_of(&error), "filter");

    // The property it reads is deleted: no type, no value (the expression stays for the settings).
    core.dispatch(Request::PropertyDelete {
        space_id: space,
        property_id: price.id,
    })
    .unwrap();
    assert_eq!(result_of(&mut core, space, alpha, "Double"), None);
    let after = property_named(&mut core, space, "Double");
    assert_eq!(
        after.config,
        Some(PropertyConfig::Formula {
            expression: format!("prop(\"{}\") * 2", price.id),
            result_type: None
        })
    );
}

#[test]
fn the_csv_writes_a_formula_like_the_kind_it_is_and_the_sidecar_its_expression_by_names() {
    let (_fx, mut core, space) = setup_with_offset(OCT_7_1430, 0);
    let price = prop_create(&mut core, space, "Price", PropertyKind::Number);
    let tasks = prop_create_with(
        &mut core,
        space,
        "Tasks",
        PropertyKind::Relation,
        Some(PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None,
        }),
    );
    let alpha = create_page(&mut core, space, None, "Alpha").id;
    let task_a = create_page(&mut core, space, None, "Task A").id;
    let task_b = create_page(&mut core, space, None, "Task B").id;
    set_value(&mut core, alpha, price.id, PropertyValue::Number(3.0));
    set_value(
        &mut core,
        alpha,
        tasks.id,
        PropertyValue::Relation(vec![task_a, task_b]),
    );
    let columns: Vec<PropertyDef> = [
        ("Double", "prop(\"{Price}\") * 2"),
        ("Shout", "prop(\"title\") + \"!\""),
        ("Cheap", "prop(\"{Price}\") < 5"),
        ("Linked", "prop(\"{Tasks}\")"),
        ("Both", "[prop(\"{Price}\"), prop(\"{Price}\") * 2]"),
        ("Today", "today()"),
    ]
    .iter()
    .map(|(name, expression)| make_formula(&mut core, space, name, expression))
    .collect();
    let csv = match core
        .dispatch(Request::ExportCsv {
            space_id: space,
            page_ids: vec![alpha],
            columns: columns
                .iter()
                .map(|def| CsvColumn::Property {
                    property_id: def.id,
                })
                .collect(),
        })
        .unwrap()
    {
        Response::Csv(csv) => csv,
        other => panic!("unexpected {other:?}"),
    };
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert_eq!(
        lines,
        [
            "\u{feff}Double,Shout,Cheap,Linked,Both,Today",
            "6,Alpha!,Yes,\"Task A, Task B\",\"3, 6\",2026-10-07",
            ""
        ]
    );

    // The Markdown archive leaves a formula out of the front matter and writes its expression in
    // display form in the sidecar.
    let exported = export_all(&mut core, space, None, true, ExportFormat::Markdown, 100);
    let page_md = exported.text("Alpha.md");
    assert!(page_md.contains("\"Price\": 3"), "{page_md}");
    assert!(!page_md.contains("Double"), "{page_md}");
    let sidecar: serde_json::Value =
        serde_json::from_str(exported.text("atomik-export.json")).unwrap();
    let entry = |name: &str| {
        sidecar["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|property| property["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(
        entry("Double"),
        json!({ "name": "Double", "kind": "formula", "config": { "expression": "prop(\"Price\") * 2" } })
    );
    assert_eq!(
        entry("Shout")["config"],
        json!({ "expression": "prop(\"title\") + \"!\"" })
    );
    assert!(
        !sidecar.to_string().contains(&price.id.to_string()),
        "no id in the sidecar"
    );
}

/// The formulas of a space by name, with their expressions in display form.
fn displayed(core: &mut Core, space: SpaceId) -> Vec<(String, String)> {
    let defs = schema(core, space);
    let names: HashMap<String, String> = defs
        .iter()
        .map(|def| (def.id.to_string(), def.name.clone()))
        .collect();
    defs.iter()
        .filter_map(|def| match &def.config {
            Some(PropertyConfig::Formula { expression, .. }) => Some((
                def.name.clone(),
                formula::to_display(expression, &|id| names.get(id).cloned()),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn an_archive_brings_the_formulas_back_with_the_ids_of_the_other_workspace() {
    let (_fx, mut core, space) = setup_with_offset(OCT_7_1430, 0);
    let priority = prop_create(&mut core, space, "Priority", PropertyKind::Number);
    // Label reads Score, which is made after Priority; the sidecar lists them in schema order.
    make_formula(&mut core, space, "Score", "prop(\"{Priority}\") * 10");
    make_formula(
        &mut core,
        space,
        "Label",
        "prop(\"title\") + format(prop(\"{Score}\"))",
    );
    let write = create_page(&mut core, space, None, "Write").id;
    set_value(&mut core, write, priority.id, PropertyValue::Number(3.0));
    // A second page makes it an archive (one page is a plain file, without the sidecar).
    create_page(&mut core, space, None, "Read");
    let here = displayed(&mut core, space);
    assert_eq!(
        here,
        [
            ("Score".to_owned(), "prop(\"Priority\") * 10".to_owned()),
            (
                "Label".to_owned(),
                "prop(\"title\") + format(prop(\"Score\"))".to_owned()
            ),
        ]
    );

    let exported = export_all(&mut core, space, None, true, ExportFormat::Markdown, 100);
    let (texts, binaries) = as_source(&mut core, &exported);
    let (_other_fx, mut other, other_space) = another_core(9);
    let imported = run_import(&mut other, other_space, None, None, &texts, &binaries, 50);
    assert_eq!(imported.report.properties_created, 3);

    assert_eq!(displayed(&mut other, other_space), here);
    let score = property_named(&mut other, other_space, "Score");
    let priority_there = property_named(&mut other, other_space, "Priority");
    assert_ne!(
        priority_there.id, priority.id,
        "the other workspace's own id"
    );
    assert_eq!(
        score.config,
        Some(PropertyConfig::Formula {
            expression: format!("prop(\"{}\") * 10", priority_there.id),
            result_type: Some(FormulaType {
                base: FormulaBase::Number,
                list: false
            }),
        })
    );
    let page = tree(&mut other, other_space)
        .roots
        .into_iter()
        .find(|node| node.page.title == "Write")
        .unwrap()
        .page
        .id;
    assert_eq!(
        result_of(&mut other, other_space, page, "Score"),
        Some(FormulaResult::Number(30.0))
    );
    assert_eq!(
        result_of(&mut other, other_space, page, "Label"),
        Some(text("Write30"))
    );
    assert!(!values_of(&mut other, page).is_empty());
}

#[test]
fn a_sidecar_formula_is_made_after_the_formulas_it_reads_and_one_that_does_not_resolve_stays_broken()
 {
    let (_fx, mut core, space) = setup_with_offset(OCT_7_1430, 0);
    let sidecar = json!({
        "format": 1,
        "properties": [
            // Total reads Score and is listed first; Ghost reads a property nobody has; Bad does not parse.
            { "name": "Total", "kind": "formula", "config": { "expression": "prop(\"Score\") + length(prop(\"title\"))" } },
            { "name": "Score", "kind": "formula", "config": { "expression": "prop(\"Priority\") * 10" } },
            { "name": "Priority", "kind": "number" },
            { "name": "Ghost", "kind": "formula", "config": { "expression": "prop(\"Nothing\") * 2" } },
            { "name": "Bad", "kind": "formula", "config": { "expression": "1 +" } },
            // The English label of the editor is not the title in a file: only `title` is.
            { "name": "Legacy", "kind": "formula", "config": { "expression": "prop(\"Name\") + \"!\"" } },
            { "name": "Empty", "kind": "formula", "config": { "expression": "  " } },
            { "name": "NoText", "kind": "formula" }
        ],
        "pages": ["A.md"]
    })
    .to_string();
    let files = vec![
        ("atomik-export.json".to_owned(), sidecar),
        ("A.md".to_owned(), "---\nPriority: 2\n---\nx".to_owned()),
    ];
    let imported = run_import(&mut core, space, None, None, &files, &[], 50);
    // Priority, Score, Total, Ghost, Bad and Legacy; the two with no text are left out.
    assert_eq!(imported.report.properties_created, 6);
    let names: Vec<String> = schema(&mut core, space)
        .into_iter()
        .map(|def| def.name)
        .collect();
    // Each formula after the ones it reads: Total, listed first, waits for Score.
    assert_eq!(
        names,
        [
            "Tags", "Priority", "Score", "Ghost", "Bad", "Legacy", "Total"
        ]
    );

    let score = property_named(&mut core, space, "Score");
    let total = property_named(&mut core, space, "Total");
    let priority = property_named(&mut core, space, "Priority");
    assert_eq!(
        total.config,
        Some(PropertyConfig::Formula {
            expression: format!("prop(\"{}\") + length(prop(\"title\"))", score.id),
            result_type: Some(FormulaType {
                base: FormulaBase::Number,
                list: false
            }),
        })
    );
    assert_eq!(
        score.config,
        Some(PropertyConfig::Formula {
            expression: format!("prop(\"{}\") * 10", priority.id),
            result_type: Some(FormulaType {
                base: FormulaBase::Number,
                list: false
            }),
        })
    );
    // As written, no result type: the settings show the error and the person fixes the text.
    for (name, written) in [
        ("Ghost", "prop(\"Nothing\") * 2"),
        ("Bad", "1 +"),
        ("Legacy", "prop(\"Name\") + \"!\""),
    ] {
        assert_eq!(
            property_named(&mut core, space, name).config,
            Some(PropertyConfig::Formula {
                expression: written.to_owned(),
                result_type: None
            })
        );
    }
    let page = tree(&mut core, space)
        .roots
        .into_iter()
        .find(|node| node.page.title == "A")
        .unwrap()
        .page
        .id;
    assert_eq!(result_of(&mut core, space, page, "Ghost"), None);
    assert_eq!(
        result_of(&mut core, space, page, "Total"),
        Some(FormulaResult::Number(21.0)),
        "2 * 10 and the length of the title \"A\""
    );
}

#[test]
fn name_of_a_person_reads_the_members_names_and_follows_a_rename() {
    let (_fx, mut core, space) = setup_with_offset(OCT_7_1430, 0);
    crate::test_support::kit_create(&mut core);
    let set_name = |core: &mut Core, name: &str| {
        core.dispatch(Request::ProfileSet {
            display_name: name.into(),
        })
        .unwrap();
    };
    set_name(&mut core, "Ada");
    let me = core.me().unwrap();
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person);
    let page = create_page(&mut core, space, None, "Alpha").id;
    set_value(&mut core, page, owner.id, PropertyValue::Person(vec![me]));
    make_formula(&mut core, space, "Who", "name(prop(\"{Owner}\").first())");
    make_formula(
        &mut core,
        space,
        "Everyone",
        "prop(\"{Owner}\").map(name(current)).join(\", \")",
    );
    // The result of a formula that reads a person is one too.
    make_formula(&mut core, space, "Same", "prop(\"{Owner}\").first()");
    assert_eq!(result_of(&mut core, space, page, "Who"), Some(text("Ada")));
    assert_eq!(
        result_of(&mut core, space, page, "Everyone"),
        Some(text("Ada"))
    );
    assert_eq!(
        result_of(&mut core, space, page, "Same"),
        Some(FormulaResult::Person(me))
    );

    // A new name is read at the next read.
    set_name(&mut core, "Grace");
    assert_eq!(
        result_of(&mut core, space, page, "Who"),
        Some(text("Grace"))
    );
}
