//! Formulas (ADR-0055): the configuration, the checks at write time, the values computed on read,
//! the derived result type, the reach over relations (one hop), the time of a read and the
//! convergence of two devices that make a loop. The engine has its own tests and corpus
//! (`formula/`, `tests/formula_corpus.rs`); these are the integration with the structure document.

use kb_types::{
    AttachmentId, ChunkId, DeviceId, FormulaBase, FormulaResult, FormulaType, OptionId,
    PropertyConfig, PropertyDef, PropertyId, PropertyKind, PropertyValue,
};

use super::{add, pid, replica, structure, sync};
use crate::formula::FormulaEnv;
use crate::{AttachmentManifest, MANIFEST_VERSION, ModelError, StructureDoc};

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xf0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn spare() -> impl FnMut() -> OptionId {
    || OptionId::from_random_bytes([1; 16])
}

fn create(doc: &mut StructureDoc, n: u8, name: &str, kind: PropertyKind) -> PropertyDef {
    doc.create_property(prop(n), name, kind, &mut spare(), 10)
        .unwrap()
}

/// The stored expression `template` with `{N}` replaced by the id of `prop(N)` (N from 0 to 99).
fn stored(template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map(|i| open + i).unwrap();
        let n: u8 = rest[open + 1..close].parse().unwrap();
        out.push_str(&prop(n).to_string());
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

fn config(template: &str) -> PropertyConfig {
    PropertyConfig::Formula {
        expression: stored(template),
        result_type: None,
    }
}

fn formula(doc: &mut StructureDoc, n: u8, template: &str) -> Result<PropertyDef, ModelError> {
    doc.create_property_with(
        prop(n),
        "Formula",
        PropertyKind::Formula,
        Some(&config(template)),
        &mut spare(),
        10,
    )
}

fn refusal(result: Result<impl std::fmt::Debug, ModelError>) -> (&'static str, String) {
    match result {
        Err(ModelError::InvalidProperty { field, message }) => (field, message),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn set(doc: &mut StructureDoc, page: u8, property: u8, value: PropertyValue) {
    doc.set_page_property(pid(page), prop(property), Some(value), 100)
        .unwrap();
}

fn value(doc: &StructureDoc, page: u8, property: u8) -> Option<PropertyValue> {
    doc.page_properties(pid(page))
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == prop(property))
        .map(|(_, value)| value)
}

fn result(doc: &StructureDoc, page: u8, property: u8) -> Option<FormulaResult> {
    match value(doc, page, property) {
        Some(PropertyValue::Formula(result)) => Some(result),
        None => None,
        other => panic!("not a formula value: {other:?}"),
    }
}

fn result_type(doc: &StructureDoc, property: u8) -> Option<FormulaType> {
    match doc.property(prop(property)).unwrap().config {
        Some(PropertyConfig::Formula { result_type, .. }) => result_type,
        other => panic!("not a formula: {other:?}"),
    }
}

fn number(n: f64) -> PropertyValue {
    PropertyValue::Number(n)
}

fn text(s: &str) -> FormulaResult {
    FormulaResult::Text(s.to_owned())
}

/// 2026-10-07T14:30:00Z in Unix ms.
const OCT_7_1430: i64 = 1_791_383_400_000;

fn env(now_ms: i64, offset_seconds: i32) -> FormulaEnv {
    FormulaEnv {
        now_ms,
        offset_seconds,
    }
}

/// Only the findings about formulas.
fn formula_problems(doc: &StructureDoc) -> Vec<String> {
    doc.check_invariants()
        .into_iter()
        .filter(|problem| problem.starts_with("formula"))
        .collect()
}

// ----- values ------------------------------------------------------------------------------------

#[test]
fn a_formula_is_computed_when_the_values_are_read_and_never_stored() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "One");
    add(&mut doc, 2, None, "Two");
    create(&mut doc, 1, "Price", PropertyKind::Number);
    formula(&mut doc, 2, "prop(\"{1}\") * 2").unwrap();

    assert_eq!(result(&doc, 1, 2), None, "no price, no value");
    set(&mut doc, 1, 1, number(3.0));
    assert_eq!(result(&doc, 1, 2), Some(FormulaResult::Number(6.0)));
    assert_eq!(result(&doc, 2, 2), None, "another page, no price");
    set(&mut doc, 1, 1, number(4.5));
    assert_eq!(result(&doc, 1, 2), Some(FormulaResult::Number(9.0)));

    // The tree carries it like any value.
    let tree = doc.live_tree();
    let info = &tree[0].page;
    assert_eq!(info.id, pid(1));
    assert!(
        info.properties
            .iter()
            .any(|(id, v)| *id == prop(2)
                && *v == PropertyValue::Formula(FormulaResult::Number(9.0)))
    );
    assert_eq!(doc.page(pid(1)).unwrap().properties, info.properties);

    // No page stores a value for it, and no row is derived.
    assert!(
        doc.to_portable().to_string().contains("\"expression\""),
        "the expression is stored with the property"
    );
    let portable = doc.to_portable();
    let stored_props = portable.to_string();
    assert!(
        !stored_props.contains("\"formula\":{"),
        "no stored formula value: {stored_props}"
    );
    assert!(
        doc.property_index(&[pid(1)])
            .iter()
            .all(|(_, entry)| entry.rows.iter().all(|row| row.property_id != prop(2))),
        "a formula has no derived rows"
    );

    // A formula cannot be set.
    let err = doc
        .set_page_property(pid(1), prop(2), Some(PropertyValue::Formula(text("x"))), 5)
        .unwrap_err();
    assert!(matches!(
        err,
        ModelError::InvalidProperty {
            field: "propertyId",
            ..
        }
    ));
    let err = doc.set_page_property(pid(1), prop(2), None, 5).unwrap_err();
    assert!(matches!(err, ModelError::InvalidProperty { .. }));
    assert!(formula_problems(&doc).is_empty());
}

#[test]
fn a_formula_reads_the_title_other_formulas_and_rollups() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Alpha");
    add(&mut doc, 2, None, "Beta");
    create(&mut doc, 1, "Price", PropertyKind::Number);
    // title + text
    formula(&mut doc, 2, "prop(\"title\") + \"!\"").unwrap();
    // a formula of a formula
    formula(&mut doc, 3, "prop(\"{1}\") + 1").unwrap();
    formula(&mut doc, 4, "prop(\"{3}\") * 10").unwrap();
    set(&mut doc, 1, 1, number(1.0));
    set(&mut doc, 2, 1, number(2.0));

    assert_eq!(result(&doc, 1, 2), Some(text("Alpha!")));
    assert_eq!(result(&doc, 2, 2), Some(text("Beta!")));
    assert_eq!(result(&doc, 1, 4), Some(FormulaResult::Number(20.0)));
    assert_eq!(result(&doc, 2, 4), Some(FormulaResult::Number(30.0)));
    assert_eq!(
        result_type(&doc, 4),
        Some(FormulaType {
            base: FormulaBase::Number,
            list: false
        })
    );

    // A rollup of a relation is readable too.
    create(&mut doc, 5, "Tasks", PropertyKind::Relation);
    create(&mut doc, 6, "Hours", PropertyKind::Number);
    doc.create_property_with(
        prop(7),
        "Total hours",
        PropertyKind::Rollup,
        Some(&PropertyConfig::Rollup {
            relation_property_id: prop(5),
            target: kb_types::RollupTarget::Property(prop(6)),
            function: kb_types::RollupFunction::Sum,
        }),
        &mut spare(),
        10,
    )
    .unwrap();
    formula(&mut doc, 8, "prop(\"{7}\") + 1").unwrap();
    set(&mut doc, 1, 6, number(2.0));
    set(&mut doc, 2, 6, number(5.0));
    set(
        &mut doc,
        1,
        5,
        PropertyValue::Relation(vec![pid(1), pid(2)]),
    );
    assert_eq!(result(&doc, 1, 8), Some(FormulaResult::Number(8.0)));
}

#[test]
fn a_formula_reaches_the_pages_of_a_relation_one_hop_and_only_live_ones() {
    let mut doc = structure(1);
    for (n, title) in [(1, "Project"), (2, "Task A"), (3, "Task B"), (4, "Task C")] {
        add(&mut doc, n, None, title);
    }
    create(&mut doc, 1, "Tasks", PropertyKind::Relation);
    create(&mut doc, 2, "Hours", PropertyKind::Number);
    formula(
        &mut doc,
        3,
        "prop(\"{1}\").map(current.prop(\"{2}\")).sum()",
    )
    .unwrap();
    formula(
        &mut doc,
        4,
        "prop(\"{1}\").map(current.prop(\"title\")).join(\", \")",
    )
    .unwrap();
    set(&mut doc, 2, 2, number(5.0));
    set(&mut doc, 3, 2, number(7.0));
    set(&mut doc, 4, 2, number(11.0));
    set(
        &mut doc,
        1,
        1,
        PropertyValue::Relation(vec![pid(2), pid(3), pid(4)]),
    );
    assert_eq!(result(&doc, 1, 3), Some(FormulaResult::Number(23.0)));
    assert_eq!(result(&doc, 1, 4), Some(text("Task A, Task B, Task C")));
    assert_eq!(
        result_type(&doc, 3),
        Some(FormulaType {
            base: FormulaBase::Number,
            list: false
        })
    );

    // A trashed page is not related any more (as for a rollup); restoring brings it back.
    doc.trash_page(pid(3), 200).unwrap();
    assert_eq!(result(&doc, 1, 3), Some(FormulaResult::Number(16.0)));
    assert_eq!(result(&doc, 1, 4), Some(text("Task A, Task C")));
    doc.restore_page(pid(3), 210).unwrap();
    assert_eq!(result(&doc, 1, 3), Some(FormulaResult::Number(23.0)));

    // A page reached through the relation is read at depth 1: the formula of Task A reads its own
    // relation to Task B, and the `.prop` it calls on it is empty there (nothing to sum: 0), while
    // on Task A itself (depth 0) the same formula adds up the 7 hours of Task B.
    set(&mut doc, 2, 1, PropertyValue::Relation(vec![pid(3)]));
    formula(
        &mut doc,
        5,
        "prop(\"{1}\").map(current.prop(\"{3}\")).sum()",
    )
    .unwrap();
    assert_eq!(result(&doc, 2, 3), Some(FormulaResult::Number(7.0)));
    assert_eq!(
        result(&doc, 1, 5),
        Some(FormulaResult::Number(0.0)),
        "a related page's own formula reads empty through a relation of its own"
    );
}

#[test]
fn a_two_way_relation_is_read_from_both_sides() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Project");
    add(&mut doc, 2, None, "Task");
    doc.create_relation_pair(prop(1), prop(2), "Tasks", "Project", 10)
        .unwrap();
    // The task reads its project through the back side, which only the project stores.
    formula(
        &mut doc,
        3,
        "prop(\"{2}\").map(current.prop(\"title\")).join(\"\")",
    )
    .unwrap();
    set(&mut doc, 1, 1, PropertyValue::Relation(vec![pid(2)]));
    assert_eq!(result(&doc, 2, 3), Some(text("Project")));
}

#[test]
fn people_and_files_read_what_the_document_knows() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    create(&mut doc, 1, "Attachments", PropertyKind::Files);
    formula(&mut doc, 2, "prop(\"{1}\").join(\"|\")").unwrap();
    let file =
        AttachmentId::from_random_bytes([0xa0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    doc.attachment_put(&AttachmentManifest {
        v: MANIFEST_VERSION,
        attachment_id: file,
        name: "plan.pdf".into(),
        mime: "application/pdf".into(),
        size: 0,
        hash: [7; 32],
        chunk_size: 4 * 1024 * 1024,
        chunks: vec![ChunkId::from_bytes([7; 32])],
        created_at: 10,
        created_by: DeviceId::from_bytes([1; 16]),
    })
    .unwrap();
    set(&mut doc, 1, 1, PropertyValue::Files(vec![file]));
    assert_eq!(result(&doc, 1, 2), Some(text("plan.pdf")));
}

// ----- the time of a read -----------------------------------------------------------------------

#[test]
fn now_is_the_time_the_owner_set_and_nothing_is_written_when_it_changes() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    formula(&mut doc, 1, "formatDate(now(), \"YYYY-MM-DD HH:mm\")").unwrap();
    formula(&mut doc, 2, "formatDate(today(), \"YYYY-MM-DD\")").unwrap();

    doc.set_formula_env(env(OCT_7_1430, 0));
    assert_eq!(doc.formula_env(), env(OCT_7_1430, 0));
    assert_eq!(result(&doc, 1, 1), Some(text("2026-10-07 14:30")));
    assert_eq!(result(&doc, 1, 2), Some(text("2026-10-07")));

    // Ten hours east of Greenwich it is already the next day: the offset of the read.
    let version = doc.doc().version();
    doc.set_formula_env(env(OCT_7_1430, 10 * 3600));
    assert_eq!(result(&doc, 1, 1), Some(text("2026-10-08 00:30")));
    assert_eq!(result(&doc, 1, 2), Some(text("2026-10-08")));
    assert_eq!(doc.page(pid(1)).unwrap().properties.len(), 2);
    assert_eq!(doc.live_tree()[0].page.properties.len(), 2);
    assert_eq!(doc.doc().version(), version, "a read writes nothing");

    // Time moves, the value follows at the next read.
    doc.set_formula_env(env(OCT_7_1430 + 86_400_000, 0));
    assert_eq!(result(&doc, 1, 2), Some(text("2026-10-08")));
}

#[test]
fn created_and_edited_times_are_local_dates() {
    let mut doc = structure(1);
    doc.create_page(pid(1), None, None, "Page", OCT_7_1430)
        .unwrap();
    create(&mut doc, 1, "Created", PropertyKind::CreatedTime);
    formula(
        &mut doc,
        2,
        "formatDate(prop(\"{1}\"), \"YYYY-MM-DD HH:mm\")",
    )
    .unwrap();
    doc.set_formula_env(env(OCT_7_1430, 2 * 3600));
    assert_eq!(result(&doc, 1, 2), Some(text("2026-10-07 16:30")));
}

// ----- the checks at write time -------------------------------------------------------------------

#[test]
fn a_formula_is_parsed_and_checked_when_created_or_changed() {
    let mut doc = structure(1);
    create(&mut doc, 1, "Price", PropertyKind::Number);
    create(&mut doc, 2, "Notes", PropertyKind::Text);

    // Creation.
    let (field, message) = refusal(formula(&mut doc, 3, "1 +"));
    assert_eq!(field, "config");
    assert!(message.contains("syntax"), "{message}");
    let (field, message) = refusal(formula(&mut doc, 3, "prop(\"0000\") + 1"));
    assert_eq!(field, "config");
    assert!(message.contains("unknown_property"), "{message}");
    let (field, message) = refusal(formula(&mut doc, 3, "prop(\"{2}\") * 2"));
    assert_eq!(field, "config");
    assert!(
        message.contains("type"),
        "a text is not a number: {message}"
    );
    assert_eq!(
        refusal(doc.create_property(
            prop(3),
            "No config",
            PropertyKind::Formula,
            &mut spare(),
            10,
        ))
        .0,
        "config"
    );
    assert!(doc.property(prop(3)).is_none(), "nothing was created");

    // The other kinds take no formula, and a formula takes no other config.
    let wrong = |doc: &mut StructureDoc, kind, config: &PropertyConfig| {
        refusal(doc.create_property_with(prop(9), "x", kind, Some(config), &mut spare(), 10)).0
    };
    assert_eq!(wrong(&mut doc, PropertyKind::Text, &config("1")), "config");
    assert_eq!(
        wrong(&mut doc, PropertyKind::Relation, &config("1")),
        "config"
    );
    assert_eq!(
        wrong(
            &mut doc,
            PropertyKind::Formula,
            &PropertyConfig::Relation {
                two_way: false,
                paired_property_id: None
            }
        ),
        "config"
    );

    // A good one is created and reads back with its result type, the expression as it was given.
    let made = formula(&mut doc, 3, "prop(\"{1}\") * 2").unwrap();
    assert_eq!(made.kind, PropertyKind::Formula);
    assert_eq!(
        made.config,
        Some(PropertyConfig::Formula {
            expression: stored("prop(\"{1}\") * 2"),
            result_type: Some(FormulaType {
                base: FormulaBase::Number,
                list: false
            }),
        })
    );

    // Changing it: a bad expression changes nothing, a good one is stored.
    let before = doc.property_schema();
    let (field, _) = refusal(doc.set_property_config(prop(3), &config("prop(\"{2}\") * 2")));
    assert_eq!(field, "config");
    assert_eq!(doc.property_schema(), before);
    let changed = doc
        .set_property_config(prop(3), &config("prop(\"{2}\") + \"!\""))
        .unwrap();
    assert_eq!(
        changed.config,
        Some(PropertyConfig::Formula {
            expression: stored("prop(\"{2}\") + \"!\""),
            result_type: Some(FormulaType {
                base: FormulaBase::Text,
                list: false
            }),
        })
    );
    // The same expression writes nothing; a result type sent by a client is ignored.
    let version = doc.doc().version();
    doc.set_property_config(
        prop(3),
        &PropertyConfig::Formula {
            expression: stored("prop(\"{2}\") + \"!\""),
            result_type: Some(FormulaType {
                base: FormulaBase::Boolean,
                list: true,
            }),
        },
    )
    .unwrap();
    assert_eq!(doc.doc().version(), version);
    // Another kind's property takes no configuration.
    let (field, _) = refusal(doc.set_property_config(prop(1), &config("1")));
    assert_eq!(field, "config");
    // A rollup or a relation cannot get a formula.
    assert!(formula_problems(&doc).is_empty());
}

#[test]
fn a_formula_that_depends_on_itself_is_refused() {
    let mut doc = structure(1);
    create(&mut doc, 1, "Price", PropertyKind::Number);
    formula(&mut doc, 2, "prop(\"{1}\") + 1").unwrap();
    formula(&mut doc, 3, "prop(\"{2}\") + 1").unwrap();

    // Directly.
    let (field, message) = refusal(doc.set_property_config(prop(2), &config("prop(\"{2}\")")));
    assert_eq!(field, "config");
    assert!(message.contains("cycle"), "{message}");
    // Through another formula.
    let (field, message) = refusal(doc.set_property_config(prop(2), &config("prop(\"{3}\") + 1")));
    assert_eq!(field, "config");
    assert!(message.contains("cycle"), "{message}");
    // Nothing changed.
    assert_eq!(
        doc.property(prop(2)).unwrap().config,
        Some(PropertyConfig::Formula {
            expression: stored("prop(\"{1}\") + 1"),
            result_type: Some(FormulaType {
                base: FormulaBase::Number,
                list: false
            }),
        })
    );
}

// ----- the derived result type -------------------------------------------------------------------

#[test]
fn the_schema_reports_the_result_type_of_each_formula() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    create(&mut doc, 1, "Price", PropertyKind::Number);
    create(&mut doc, 2, "Due", PropertyKind::Date);
    create(&mut doc, 3, "Done", PropertyKind::Checkbox);
    create(&mut doc, 4, "Related", PropertyKind::Relation);
    for (n, template) in [
        (10, "prop(\"{1}\") * 2"),
        (11, "prop(\"title\") + \"x\""),
        (12, "prop(\"{2}\")"),
        (13, "not prop(\"{3}\")"),
        (14, "prop(\"{4}\")"),
        (15, "prop(\"Tags\")"),
    ] {
        let template = template.replace(
            "\"Tags\"",
            &format!("\"{}\"", kb_types::properties::TAGS_PROPERTY_ID),
        );
        formula(&mut doc, n, &template).unwrap();
    }
    let ty = |base, list| Some(FormulaType { base, list });
    assert_eq!(result_type(&doc, 10), ty(FormulaBase::Number, false));
    assert_eq!(result_type(&doc, 11), ty(FormulaBase::Text, false));
    assert_eq!(result_type(&doc, 12), ty(FormulaBase::Date, false));
    assert_eq!(result_type(&doc, 13), ty(FormulaBase::Boolean, false));
    assert_eq!(result_type(&doc, 14), ty(FormulaBase::Page, true));
    assert_eq!(result_type(&doc, 15), ty(FormulaBase::Text, true));
    // `property_schema` and `property` agree, a replica reads the same.
    let schema = doc.property_schema();
    assert_eq!(replica(&doc, 2).property_schema(), schema);
    assert!(schema.iter().any(|def| def.id == prop(14)
        && def.config
            == Some(PropertyConfig::Formula {
                expression: stored("prop(\"{4}\")"),
                result_type: ty(FormulaBase::Page, true),
            })));
}

#[test]
fn a_formula_that_no_longer_checks_has_no_type_no_value_and_is_reported() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    create(&mut doc, 1, "Price", PropertyKind::Number);
    formula(&mut doc, 2, "prop(\"{1}\") * 2").unwrap();
    formula(&mut doc, 3, "prop(\"{2}\") + 1").unwrap();
    set(&mut doc, 1, 1, number(2.0));
    assert_eq!(result(&doc, 1, 3), Some(FormulaResult::Number(5.0)));
    assert!(formula_problems(&doc).is_empty());

    // The property it reads is deleted: both formulas lose their type and their values.
    doc.delete_property(prop(1)).unwrap();
    assert_eq!(result_type(&doc, 2), None);
    assert_eq!(result_type(&doc, 3), None, "a formula of a broken formula");
    assert_eq!(result(&doc, 1, 2), None);
    assert_eq!(result(&doc, 1, 3), None);
    let problems = formula_problems(&doc);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[0].contains(&prop(2).to_string()) || problems[1].contains(&prop(2).to_string())
    );
    // The expression is still stored: the settings show the error and the person fixes it.
    assert_eq!(
        doc.property(prop(2)).unwrap().config,
        Some(PropertyConfig::Formula {
            expression: stored("prop(\"{1}\") * 2"),
            result_type: None,
        })
    );
    create(&mut doc, 4, "Cost", PropertyKind::Number);
    doc.set_property_config(prop(2), &config("prop(\"{4}\") * 2"))
        .unwrap();
    assert!(result_type(&doc, 3).is_some(), "the dependant checks again");
    assert!(formula_problems(&doc).is_empty());
}

#[test]
fn two_devices_that_make_a_loop_leave_formulas_without_a_value() {
    let mut a = structure(1);
    add(&mut a, 1, None, "Page");
    create(&mut a, 1, "Price", PropertyKind::Number);
    formula(&mut a, 2, "prop(\"{1}\") + 1").unwrap();
    formula(&mut a, 3, "prop(\"{1}\") + 2").unwrap();
    set(&mut a, 1, 1, number(1.0));
    let mut b = replica(&a, 2);

    // Each device points one formula at the other: fine on its own, a loop once merged.
    a.set_property_config(prop(2), &config("prop(\"{3}\") + 1"))
        .unwrap();
    b.set_property_config(prop(3), &config("prop(\"{2}\") + 1"))
        .unwrap();
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert_eq!(result_type(doc, 2), None);
        assert_eq!(result_type(doc, 3), None);
        assert_eq!(result(doc, 1, 2), None);
        assert_eq!(result(doc, 1, 3), None);
        assert_eq!(formula_problems(doc).len(), 2);
        assert_eq!(
            result(doc, 1, 2),
            None,
            "no panic and no endless evaluation"
        );
    }
    assert_eq!(a.property_schema(), b.property_schema());
}

// ----- ids, not names ---------------------------------------------------------------------------

#[test]
fn a_rename_changes_nothing_stored_and_the_value_follows_the_id() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    create(&mut doc, 1, "Price", PropertyKind::Number);
    formula(&mut doc, 2, "prop(\"{1}\") * 2").unwrap();
    set(&mut doc, 1, 1, number(4.0));
    doc.rename_property(prop(1), "Cost", 50).unwrap();
    assert_eq!(result(&doc, 1, 2), Some(FormulaResult::Number(8.0)));
    assert_eq!(
        doc.property(prop(2)).unwrap().config,
        Some(PropertyConfig::Formula {
            expression: stored("prop(\"{1}\") * 2"),
            result_type: Some(FormulaType {
                base: FormulaBase::Number,
                list: false
            }),
        })
    );
}

#[test]
fn a_duplicated_page_gets_the_values_it_stored_and_computes_its_own_formula() {
    use std::collections::HashMap;

    let mut doc = structure(1);
    add(&mut doc, 1, None, "Source");
    create(&mut doc, 1, "Price", PropertyKind::Number);
    formula(&mut doc, 2, "prop(\"{1}\") * 2 + length(prop(\"title\"))").unwrap();
    set(&mut doc, 1, 1, number(3.0));
    assert_eq!(result(&doc, 1, 2), Some(FormulaResult::Number(12.0)));

    let ids = HashMap::from([(pid(1), pid(101))]);
    doc.duplicate_subtree(pid(1), &ids, "Copy", 500).unwrap();
    assert_eq!(
        value(&doc, 101, 1),
        Some(number(3.0)),
        "the stored value is copied"
    );
    assert_eq!(
        result(&doc, 101, 2),
        Some(FormulaResult::Number(10.0)),
        "the copy computes its own: 3 * 2 + the length of \"Copy\""
    );
    assert!(formula_problems(&doc).is_empty());
}

#[test]
fn a_rollup_that_shows_the_originals_carries_the_options_of_its_target() {
    let mut doc = structure(1);
    for (n, title) in [(1, "Project"), (2, "Task A"), (3, "Task B")] {
        add(&mut doc, n, None, title);
    }
    create(&mut doc, 1, "Tasks", PropertyKind::Relation);
    create(&mut doc, 2, "Stage", PropertyKind::Select);
    for (n, name) in [(1, "Todo"), (2, "Done")] {
        doc.add_option(
            prop(2),
            OptionId::from_random_bytes([0xc1, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n]),
            name,
            kb_types::OptionColor::Blue,
            None,
            20,
        )
        .unwrap();
    }
    let option = |n: u8| doc.property(prop(2)).unwrap().options[usize::from(n)].id;
    let (todo, done) = (option(0), option(1));
    doc.create_property_with(
        prop(3),
        "Originals",
        PropertyKind::Rollup,
        Some(&PropertyConfig::Rollup {
            relation_property_id: prop(1),
            target: kb_types::RollupTarget::Property(prop(2)),
            function: kb_types::RollupFunction::ShowOriginal,
        }),
        &mut spare(),
        10,
    )
    .unwrap();
    formula(&mut doc, 4, "prop(\"{3}\").join(\", \")").unwrap();
    set(&mut doc, 2, 2, PropertyValue::Select(todo));
    set(&mut doc, 3, 2, PropertyValue::Select(done));
    set(
        &mut doc,
        1,
        1,
        PropertyValue::Relation(vec![pid(2), pid(3)]),
    );
    assert_eq!(result(&doc, 1, 4), Some(text("Todo, Done")));
}
