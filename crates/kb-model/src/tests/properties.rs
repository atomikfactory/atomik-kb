//! Typed page properties (ADR-0026): the schema, the values, the derived rows and merging.

use kb_types::properties::{
    DEFAULT_STATUS_OPTIONS, MAX_NAME_CHARS, MAX_OPTIONS, MAX_PROPERTIES, TAGS_NAME,
    TAGS_PROPERTY_ID,
};
use kb_types::{
    AttachmentId, ChecklistItem, ChecklistItemId, ChunkId, DateValue, DeviceId, OptionColor,
    OptionId, PageId, PropertyDef, PropertyId, PropertyKind, PropertyValue, StatusGroup,
};
use serde_json::json;

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{AttachmentManifest, MANIFEST_VERSION, ModelError, PROPERTIES_ORIGIN, StructureDoc};

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xb0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn opt(n: u8) -> OptionId {
    OptionId::from_random_bytes([0xc0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

/// Option ids `opt(100)`, `opt(101)`, ... for a status property's defaults.
fn defaults() -> impl FnMut() -> OptionId {
    let mut next = 100;
    move || {
        next += 1;
        opt(next - 1)
    }
}

fn create(doc: &mut StructureDoc, n: u8, name: &str, kind: PropertyKind) -> PropertyDef {
    doc.create_property(prop(n), name, kind, &mut defaults(), 1_000 + i64::from(n))
        .unwrap()
}

fn add_option(doc: &mut StructureDoc, property: u8, option: u8, name: &str) {
    doc.add_option(
        prop(property),
        opt(option),
        name,
        OptionColor::Blue,
        None,
        2_000,
    )
    .unwrap();
}

fn set(doc: &mut StructureDoc, page: PageId, property: PropertyId, value: PropertyValue) {
    doc.set_page_property(page, property, Some(value), 3_000)
        .unwrap();
}

fn text(value: &str) -> PropertyValue {
    PropertyValue::Text(value.into())
}

fn file(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([0xa0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn files(ns: &[u8]) -> PropertyValue {
    PropertyValue::Files(ns.iter().map(|n| file(*n)).collect())
}

fn item(n: u8, text: &str, done: bool) -> ChecklistItem {
    ChecklistItem {
        id: ChecklistItemId::from_random_bytes([
            0xd0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n,
        ]),
        text: text.to_owned(),
        done,
    }
}

fn checklist(items: Vec<ChecklistItem>) -> PropertyValue {
    PropertyValue::Checklist(items)
}

/// Puts the manifest of attachment `n` (one empty chunk) with this name into the document.
fn put_manifest(doc: &StructureDoc, n: u8, name: &str) {
    doc.attachment_put(&AttachmentManifest {
        v: MANIFEST_VERSION,
        attachment_id: file(n),
        name: name.to_owned(),
        mime: "application/octet-stream".into(),
        size: 0,
        hash: [n; 32],
        chunk_size: 4 * 1024 * 1024,
        chunks: vec![ChunkId::from_bytes([n; 32])],
        created_at: 10,
        created_by: DeviceId::from_bytes([1; 16]),
    })
    .unwrap();
}

fn names(doc: &StructureDoc) -> Vec<String> {
    doc.property_schema().into_iter().map(|p| p.name).collect()
}

fn invalid_field(err: &ModelError) -> Option<&'static str> {
    match err {
        ModelError::InvalidProperty { field, .. } => Some(field),
        _ => None,
    }
}

// ----- schema ---------------------------------------------------------------------------------

#[test]
fn a_space_lists_tags_first_before_anything_is_written() {
    let doc = structure(1);
    let schema = doc.property_schema();
    assert_eq!(schema.len(), 1);
    let tags = &schema[0];
    assert_eq!(tags.id, TAGS_PROPERTY_ID);
    assert_eq!(tags.name, TAGS_NAME);
    assert_eq!(tags.kind, PropertyKind::MultiSelect);
    assert!(tags.options.is_empty());
    assert_eq!(doc.property(TAGS_PROPERTY_ID).as_ref(), Some(tags));
    assert!(
        doc.to_portable().get("properties").is_none(),
        "not written yet"
    );
    assert_healthy(&doc);
    assert!(!doc.can_undo());
}

#[test]
fn properties_are_created_in_order_with_their_kinds() {
    let mut doc = structure(1);
    let text_property = create(&mut doc, 1, "Notes", PropertyKind::Text);
    create(&mut doc, 2, "Cost", PropertyKind::Number);
    create(&mut doc, 3, "Kind", PropertyKind::Select);
    for kind in PropertyKind::ALL {
        if kind == PropertyKind::Rollup {
            // A rollup summarises the titles through the relation made just before it.
            doc.create_property_with(
                prop(10 + kind as u8),
                "Rollup",
                kind,
                Some(&kb_types::PropertyConfig::Rollup {
                    relation_property_id: prop(10 + PropertyKind::Relation as u8),
                    target: kb_types::RollupTarget::Title,
                    function: kb_types::RollupFunction::CountAll,
                }),
                &mut defaults(),
                1_000,
            )
            .unwrap();
            continue;
        }
        if kind == PropertyKind::Formula {
            // A formula reads the text property made first.
            doc.create_property_with(
                prop(10 + kind as u8),
                "Formula",
                kind,
                Some(&kb_types::PropertyConfig::Formula {
                    expression: format!("prop(\"{}\")", prop(1)),
                    result_type: None,
                }),
                &mut defaults(),
                1_000,
            )
            .unwrap();
            continue;
        }
        create(&mut doc, 10 + kind as u8, &format!("{kind:?}"), kind);
    }
    assert_eq!(text_property.name, "Notes");
    assert_eq!(text_property.kind, PropertyKind::Text);
    assert_eq!(text_property.created_at, 1_001);
    assert!(text_property.options.is_empty());
    let schema = doc.property_schema();
    assert_eq!(schema[0].id, TAGS_PROPERTY_ID, "Tags first");
    assert_eq!(
        names(&doc)[..4],
        ["Tags", "Notes", "Cost", "Kind"],
        "then by position, in creation order"
    );
    assert_eq!(schema.len(), 1 + 3 + PropertyKind::ALL.len());
    assert_healthy(&doc);

    // Reloading from a snapshot reads the same schema.
    assert_eq!(replica(&doc, 2).property_schema(), schema);
    assert!(
        !doc.can_undo(),
        "schema writes are not on the structural undo stack"
    );
}

#[test]
fn a_status_property_starts_with_three_grouped_options() {
    let mut doc = structure(1);
    let status = create(&mut doc, 1, "Stage", PropertyKind::Status);
    let listed: Vec<(String, Option<StatusGroup>, OptionColor)> = status
        .options
        .iter()
        .map(|o| (o.name.clone(), o.group, o.color))
        .collect();
    let expected: Vec<(String, Option<StatusGroup>, OptionColor)> = DEFAULT_STATUS_OPTIONS
        .iter()
        .map(|(name, group, color)| ((*name).to_owned(), Some(*group), *color))
        .collect();
    assert_eq!(listed, expected);
    assert_eq!(
        listed.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(),
        ["Not started", "In progress", "Done"]
    );
    assert_eq!(
        status.options.iter().map(|o| o.id).collect::<Vec<_>>(),
        [opt(100), opt(101), opt(102)]
    );
    let select = create(&mut doc, 2, "Pick", PropertyKind::Select);
    assert!(select.options.is_empty(), "no defaults for a select");
    assert_healthy(&doc);
}

#[test]
fn names_ids_and_the_size_of_the_schema_are_checked() {
    let mut doc = structure(1);
    let mut new_option = defaults();
    let mut try_create = |doc: &mut StructureDoc, id: PropertyId, name: &str| {
        doc.create_property(id, name, PropertyKind::Text, &mut new_option, 1)
    };
    for bad in ["", "   ", "tab\tname", "line\nbreak"] {
        let err = try_create(&mut doc, prop(1), bad).unwrap_err();
        assert_eq!(invalid_field(&err), Some("name"), "{bad:?}");
    }
    let too_long = "x".repeat(MAX_NAME_CHARS + 1);
    assert_eq!(
        invalid_field(&try_create(&mut doc, prop(1), &too_long).unwrap_err()),
        Some("name")
    );
    try_create(&mut doc, prop(1), &"x".repeat(MAX_NAME_CHARS)).unwrap();
    let err = try_create(&mut doc, prop(1), "again").unwrap_err();
    assert_eq!(invalid_field(&err), Some("propertyId"), "the id is used");
    let err = try_create(&mut doc, TAGS_PROPERTY_ID, "Tags 2").unwrap_err();
    assert_eq!(invalid_field(&err), Some("propertyId"));

    // Tags counts: 99 more fill the schema.
    for n in 2..u8::try_from(MAX_PROPERTIES).unwrap() {
        try_create(&mut doc, prop(n), "p").unwrap();
    }
    assert_eq!(doc.property_schema().len(), MAX_PROPERTIES);
    let err = try_create(&mut doc, prop(200), "one too many").unwrap_err();
    assert_eq!(invalid_field(&err), Some("spaceId"));
    assert_healthy(&doc);
}

#[test]
fn a_property_is_renamed_and_removed_and_unknown_ones_are_not_found() {
    let mut doc = structure(1);
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    let renamed = doc.rename_property(prop(1), "Remarks", 10).unwrap();
    assert_eq!(renamed.name, "Remarks");
    let version = doc.doc().version();
    doc.rename_property(prop(1), "Remarks", 11).unwrap();
    assert_eq!(doc.doc().version(), version, "the same name writes nothing");
    let err = doc.rename_property(prop(1), "  ", 12).unwrap_err();
    assert_eq!(invalid_field(&err), Some("name"));
    assert!(matches!(
        doc.rename_property(prop(9), "x", 12),
        Err(ModelError::PropertyNotFound)
    ));

    doc.delete_property(prop(1)).unwrap();
    assert!(doc.property(prop(1)).is_none());
    assert_eq!(names(&doc), ["Tags"]);
    assert!(matches!(
        doc.delete_property(prop(1)),
        Err(ModelError::PropertyNotFound)
    ));
    assert_healthy(&doc);
}

#[test]
fn tags_is_written_on_first_use_renamed_but_never_deleted_or_moved() {
    let mut doc = structure(1);
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    let err = doc.delete_property(TAGS_PROPERTY_ID).unwrap_err();
    assert_eq!(invalid_field(&err), Some("propertyId"));
    let err = doc.move_property(TAGS_PROPERTY_ID, None).unwrap_err();
    assert_eq!(invalid_field(&err), Some("propertyId"));
    assert!(
        doc.to_portable()["properties"]
            .get(TAGS_PROPERTY_ID.to_string())
            .is_none()
    );

    let option = doc
        .add_option(
            TAGS_PROPERTY_ID,
            opt(1),
            "urgent",
            OptionColor::Red,
            None,
            50,
        )
        .unwrap();
    assert_eq!(option.name, "urgent");
    let portable = doc.to_portable();
    let tags = &portable["properties"][TAGS_PROPERTY_ID.to_string()];
    assert_eq!(tags["name"], "Tags");
    assert_eq!(tags["kind"], "multi_select");
    assert_eq!(tags["createdAt"], 50);
    assert_eq!(tags["options"][opt(1).to_string()]["color"], "red");
    assert_eq!(doc.property_schema()[0].options.len(), 1);
    assert_healthy(&doc);

    let renamed = doc.rename_property(TAGS_PROPERTY_ID, "Labels", 60).unwrap();
    assert_eq!(renamed.name, "Labels");
    assert_eq!(doc.property_schema()[0].id, TAGS_PROPERTY_ID, "still first");
    let err = doc.delete_property(TAGS_PROPERTY_ID).unwrap_err();
    assert_eq!(invalid_field(&err), Some("propertyId"));
}

#[test]
fn properties_move_before_another_or_to_the_end() {
    let mut doc = structure(1);
    for (n, name) in [(1, "A"), (2, "B"), (3, "C"), (4, "D")] {
        create(&mut doc, n, name, PropertyKind::Text);
    }
    assert_eq!(names(&doc), ["Tags", "A", "B", "C", "D"]);
    doc.move_property(prop(4), Some(prop(2))).unwrap();
    assert_eq!(names(&doc), ["Tags", "A", "D", "B", "C"]);
    doc.move_property(prop(1), None).unwrap();
    assert_eq!(names(&doc), ["Tags", "D", "B", "C", "A"]);
    doc.move_property(prop(3), Some(TAGS_PROPERTY_ID)).unwrap();
    assert_eq!(
        names(&doc),
        ["Tags", "C", "D", "B", "A"],
        "before Tags = first of the others"
    );
    doc.move_property(prop(3), Some(prop(4))).unwrap();
    assert_eq!(names(&doc), ["Tags", "C", "D", "B", "A"], "already there");

    let version = doc.doc().version();
    doc.move_property(prop(1), None).unwrap();
    doc.move_property(prop(2), Some(prop(2))).unwrap();
    doc.move_property(prop(4), Some(prop(2))).unwrap();
    assert_eq!(
        doc.doc().version(),
        version,
        "moves that change nothing write nothing"
    );

    assert!(matches!(
        doc.move_property(prop(9), None),
        Err(ModelError::PropertyNotFound)
    ));
    assert!(matches!(
        doc.move_property(prop(1), Some(prop(9))),
        Err(ModelError::PropertyNotFound)
    ));
    assert_healthy(&doc);
    assert_eq!(names(&replica(&doc, 2)), names(&doc));
}

#[test]
fn moves_between_properties_with_equal_positions_renumber_instead_of_failing() {
    let mut a = structure(1);
    create(&mut a, 1, "A", PropertyKind::Text);
    let mut b = replica(&a, 2);
    // Both devices append at the same moment: the same position for two properties.
    create(&mut a, 2, "From A", PropertyKind::Text);
    create(&mut b, 3, "From B", PropertyKind::Text);
    sync(&mut a, &mut b);
    assert_eq!(names(&a), names(&b), "ties break by id on every replica");
    assert_eq!(a.property_schema().len(), 4);
    let order: Vec<PropertyId> = a.property_schema().iter().map(|p| p.id).collect();
    let (first, second) = (order[2], order[3]);

    // Squeezing a property between the two tied ones works.
    a.move_property(prop(1), Some(second)).unwrap();
    let after: Vec<PropertyId> = a.property_schema().iter().map(|p| p.id).collect();
    assert_eq!(after, [TAGS_PROPERTY_ID, first, prop(1), second]);
    assert_healthy(&a);
}

// ----- options --------------------------------------------------------------------------------

#[test]
fn options_are_added_updated_and_ordered() {
    let mut doc = structure(1);
    create(&mut doc, 1, "Kind", PropertyKind::Select);
    create(&mut doc, 2, "Stage", PropertyKind::Status);
    for (n, name) in [(1, "One"), (2, "Two"), (3, "Three")] {
        add_option(&mut doc, 1, n, name);
    }
    let kind = doc.property(prop(1)).unwrap();
    assert_eq!(
        kind.options
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["One", "Two", "Three"]
    );
    assert!(kind.options.iter().all(|o| o.group.is_none()));

    let updated = doc
        .update_option(prop(1), opt(2), Some("Deux"), Some(OptionColor::Pink), None)
        .unwrap();
    assert_eq!(
        (updated.name.as_str(), updated.color, updated.group),
        ("Deux", OptionColor::Pink, None)
    );
    let version = doc.doc().version();
    doc.update_option(prop(1), opt(2), Some("Deux"), Some(OptionColor::Pink), None)
        .unwrap();
    doc.update_option(prop(1), opt(2), None, None, None)
        .unwrap();
    assert_eq!(doc.doc().version(), version, "no change, no write");
    assert_eq!(
        doc.property(prop(1)).unwrap().options[1].name,
        "Deux",
        "an update keeps the option's place"
    );

    // Status options: the group defaults to todo and can be changed.
    let added = doc
        .add_option(prop(2), opt(9), "Blocked", OptionColor::Red, None, 10)
        .unwrap();
    assert_eq!(added.group, Some(StatusGroup::Todo));
    let moved = doc
        .update_option(prop(2), opt(9), None, None, Some(StatusGroup::InProgress))
        .unwrap();
    assert_eq!(moved.group, Some(StatusGroup::InProgress));
    let with_group = doc
        .add_option(
            prop(2),
            opt(10),
            "Shipped",
            OptionColor::Green,
            Some(StatusGroup::Done),
            11,
        )
        .unwrap();
    assert_eq!(with_group.group, Some(StatusGroup::Done));
    assert_eq!(doc.property(prop(2)).unwrap().options.len(), 5);
    assert_healthy(&doc);
    assert_eq!(replica(&doc, 2).property_schema(), doc.property_schema());
}

#[test]
fn option_operations_refuse_what_does_not_fit() {
    let mut doc = structure(1);
    create(&mut doc, 1, "Kind", PropertyKind::Select);
    create(&mut doc, 2, "Notes", PropertyKind::Text);
    add_option(&mut doc, 1, 1, "One");

    let err = doc
        .add_option(prop(2), opt(5), "x", OptionColor::Gray, None, 1)
        .unwrap_err();
    assert_eq!(
        invalid_field(&err),
        Some("propertyId"),
        "text has no options"
    );
    let err = doc
        .add_option(
            prop(1),
            opt(5),
            "x",
            OptionColor::Gray,
            Some(StatusGroup::Done),
            1,
        )
        .unwrap_err();
    assert_eq!(invalid_field(&err), Some("group"));
    let err = doc
        .update_option(prop(1), opt(1), None, None, Some(StatusGroup::Done))
        .unwrap_err();
    assert_eq!(invalid_field(&err), Some("group"));
    let err = doc
        .add_option(prop(1), opt(1), "dup id", OptionColor::Gray, None, 1)
        .unwrap_err();
    assert_eq!(invalid_field(&err), Some("optionId"));
    for bad in ["", "  ", "a\u{0}b"] {
        let err = doc
            .add_option(prop(1), opt(6), bad, OptionColor::Gray, None, 1)
            .unwrap_err();
        assert_eq!(invalid_field(&err), Some("name"), "{bad:?}");
    }
    assert!(matches!(
        doc.add_option(prop(9), opt(6), "x", OptionColor::Gray, None, 1),
        Err(ModelError::PropertyNotFound)
    ));
    assert!(matches!(
        doc.update_option(prop(1), opt(9), Some("x"), None, None),
        Err(ModelError::OptionNotFound)
    ));
    assert!(matches!(
        doc.delete_option(prop(1), opt(9), 1),
        Err(ModelError::OptionNotFound)
    ));
    assert!(matches!(
        doc.delete_option(prop(9), opt(1), 1),
        Err(ModelError::PropertyNotFound)
    ));

    // The list is bounded.
    for n in 2..=u8::try_from(MAX_OPTIONS.min(250)).unwrap() {
        add_option(&mut doc, 1, n, "o");
    }
    // `MAX_OPTIONS` is 500: fill the rest with distinct ids from another byte.
    let mut next: u16 = 1;
    while doc.property(prop(1)).unwrap().options.len() < MAX_OPTIONS {
        let mut bytes = [0xd0_u8; 16];
        bytes[1..3].copy_from_slice(&next.to_be_bytes());
        next += 1;
        doc.add_option(
            prop(1),
            OptionId::from_random_bytes(bytes),
            "o",
            OptionColor::Gray,
            None,
            1,
        )
        .unwrap();
    }
    let err = doc
        .add_option(
            prop(1),
            opt(251),
            "one too many",
            OptionColor::Gray,
            None,
            1,
        )
        .unwrap_err();
    assert_eq!(invalid_field(&err), Some("propertyId"));
}

#[test]
fn deleting_an_option_takes_it_out_of_every_value_in_one_commit() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    let b = add(&mut doc, 2, None, "B");
    let c = add(&mut doc, 3, None, "C");
    let trashed = add(&mut doc, 4, None, "Trashed");
    let untouched = add(&mut doc, 5, None, "Untouched");
    create(&mut doc, 1, "Kind", PropertyKind::Select);
    create(&mut doc, 2, "Stage", PropertyKind::Status);
    for (n, name) in [(1, "red"), (2, "green"), (3, "blue")] {
        add_option(&mut doc, 1, n, name);
    }
    doc.add_option(TAGS_PROPERTY_ID, opt(21), "x", OptionColor::Gray, None, 5)
        .unwrap();
    doc.add_option(TAGS_PROPERTY_ID, opt(22), "y", OptionColor::Gray, None, 5)
        .unwrap();
    set(&mut doc, a, prop(1), PropertyValue::Select(opt(1)));
    set(&mut doc, b, prop(1), PropertyValue::Select(opt(2)));
    set(&mut doc, trashed, prop(1), PropertyValue::Select(opt(1)));
    set(
        &mut doc,
        a,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![opt(21), opt(22)]),
    );
    set(
        &mut doc,
        c,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![opt(21)]),
    );
    set(
        &mut doc,
        untouched,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![opt(22)]),
    );
    set(&mut doc, a, prop(2), PropertyValue::Status(opt(100)));
    doc.trash_page(trashed, 100).unwrap();

    let mut other = replica(&doc, 2);
    let before = doc.doc().version();

    // A select option: the pages that had it lose the value (a trashed page too).
    let mut changed = doc.delete_option(prop(1), opt(1), 777).unwrap();
    changed.sort_unstable();
    let mut expected = vec![a, trashed];
    expected.sort_unstable();
    assert_eq!(changed, expected);
    assert_eq!(
        doc.page_properties(a)
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [TAGS_PROPERTY_ID, prop(2)]
    );
    assert!(doc.page_properties(trashed).unwrap().is_empty());
    assert_eq!(
        doc.page_properties(b).unwrap(),
        [(prop(1), PropertyValue::Select(opt(2)))],
        "other options stay"
    );
    assert_eq!(doc.page(a).unwrap().updated_at, 777);
    assert_eq!(
        doc.page(b).unwrap().updated_at,
        3_000,
        "untouched pages keep their time"
    );
    assert_eq!(doc.property(prop(1)).unwrap().options.len(), 2);

    // A multi-select option: removed from the list, and a list left empty is cleared.
    let mut changed = doc.delete_option(TAGS_PROPERTY_ID, opt(21), 778).unwrap();
    changed.sort_unstable();
    let mut expected = vec![a, c];
    expected.sort_unstable();
    assert_eq!(changed, expected);
    assert_eq!(
        doc.page_properties(a).unwrap()[0],
        (TAGS_PROPERTY_ID, PropertyValue::MultiSelect(vec![opt(22)]))
    );
    assert!(
        doc.page_properties(c).unwrap().is_empty(),
        "the last option clears the value"
    );
    assert_eq!(
        doc.page_properties(untouched).unwrap(),
        [(TAGS_PROPERTY_ID, PropertyValue::MultiSelect(vec![opt(22)]))]
    );
    assert_healthy(&doc);

    // One export carries both deletions, cascade included.
    let update = doc
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    other.doc().import(&update).unwrap();
    other.refresh().unwrap();
    assert_eq!(other.to_portable(), doc.to_portable());
    assert_healthy(&other);

    // A deletion that touches no page changes the schema only.
    assert!(doc.delete_option(prop(1), opt(3), 800).unwrap().is_empty());
}

// ----- values ---------------------------------------------------------------------------------

#[test]
fn every_kind_of_value_is_set_read_back_and_cleared() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    let kinds = [
        (1, PropertyKind::Text),
        (2, PropertyKind::Number),
        (3, PropertyKind::Select),
        (4, PropertyKind::MultiSelect),
        (5, PropertyKind::Date),
        (6, PropertyKind::Checkbox),
        (7, PropertyKind::Url),
        (8, PropertyKind::Status),
        (9, PropertyKind::Email),
        (10, PropertyKind::Phone),
        (11, PropertyKind::Files),
        (12, PropertyKind::Checklist),
    ];
    for (n, kind) in kinds {
        create(&mut doc, n, &format!("{kind:?}"), kind);
    }
    add_option(&mut doc, 3, 1, "red");
    add_option(&mut doc, 4, 2, "x");
    add_option(&mut doc, 4, 3, "y");
    let values = [
        (1, text("Hello\nworld")),
        (2, PropertyValue::Number(-12.25)),
        (3, PropertyValue::Select(opt(1))),
        (4, PropertyValue::MultiSelect(vec![opt(3), opt(2)])),
        (
            5,
            PropertyValue::Date(DateValue {
                date: "2026-10-07".into(),
                time: Some("14:30".into()),
            }),
        ),
        (6, PropertyValue::Checkbox(false)),
        (7, PropertyValue::Url("https://example.com/a".into())),
        (8, PropertyValue::Status(opt(101))),
        (9, PropertyValue::Email("ada@example.com".into())),
        (10, PropertyValue::Phone("+1 555 0100".into())),
        (11, files(&[3, 1, 2])),
        (
            12,
            checklist(vec![item(1, "Pack", true), item(2, "Call Ada", false)]),
        ),
    ];
    for (n, value) in &values {
        assert_eq!(
            doc.set_page_property(page, prop(*n), Some(value.clone()), 500)
                .unwrap(),
            vec![page]
        );
    }
    let read = doc.page_properties(page).unwrap();
    let expected: Vec<(PropertyId, PropertyValue)> =
        values.iter().map(|(n, v)| (prop(*n), v.clone())).collect();
    assert_eq!(read, expected, "in schema order");
    assert_eq!(doc.page(page).unwrap().properties, expected);
    assert_eq!(doc.page(page).unwrap().updated_at, 500);
    assert_eq!(doc.live_tree()[0].page.properties, expected);
    assert_healthy(&doc);
    assert_eq!(replica(&doc, 2).page_properties(page).unwrap(), expected);

    for (n, _) in &values {
        assert_eq!(
            doc.set_page_property(page, prop(*n), None, 600).unwrap(),
            vec![page]
        );
    }
    assert!(doc.page_properties(page).unwrap().is_empty());
    assert!(doc.page(page).unwrap().properties.is_empty());
    assert_healthy(&doc);
}

#[test]
fn a_write_of_the_same_value_changes_nothing_and_empties_clear() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    create(&mut doc, 2, "Link", PropertyKind::Url);
    set(&mut doc, page, prop(1), text("x"));
    let version = doc.doc().version();
    assert!(
        doc.set_page_property(page, prop(1), Some(text("x")), 900)
            .unwrap()
            .is_empty()
    );
    assert!(
        doc.set_page_property(page, prop(2), None, 900)
            .unwrap()
            .is_empty()
    );
    assert_eq!(doc.doc().version(), version);
    assert_eq!(
        doc.page(page).unwrap().updated_at,
        3_000,
        "no write, no time"
    );

    // An empty text or URL is no value.
    assert_eq!(
        doc.set_page_property(page, prop(1), Some(text("")), 901)
            .unwrap(),
        vec![page]
    );
    assert!(doc.page_properties(page).unwrap().is_empty());
    assert!(
        doc.set_page_property(page, prop(2), Some(PropertyValue::Url(String::new())), 902)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_value_that_does_not_fit_its_property_is_refused_and_writes_nothing() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    create(&mut doc, 2, "Kind", PropertyKind::Select);
    create(&mut doc, 3, "Stage", PropertyKind::Status);
    create(&mut doc, 4, "When", PropertyKind::Date);
    create(&mut doc, 5, "Attached", PropertyKind::Files);
    create(&mut doc, 6, "To do", PropertyKind::Checklist);
    add_option(&mut doc, 2, 1, "red");
    add_option(&mut doc, 2, 2, "green");
    let version = doc.doc().version();
    let refuse = |doc: &mut StructureDoc, property: PropertyId, value: PropertyValue| {
        let err = doc
            .set_page_property(page, property, Some(value), 9)
            .unwrap_err();
        assert_eq!(invalid_field(&err), Some("value"), "{err}");
    };
    refuse(&mut doc, prop(1), PropertyValue::Number(1.0));
    refuse(&mut doc, prop(1), text(&"x".repeat(10_001)));
    refuse(&mut doc, prop(1), text("bell\u{7}"));
    refuse(&mut doc, prop(2), PropertyValue::Select(opt(9)));
    refuse(&mut doc, prop(2), PropertyValue::Status(opt(1)));
    refuse(&mut doc, prop(3), PropertyValue::Status(opt(1)));
    refuse(
        &mut doc,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![opt(1)]),
    );
    refuse(
        &mut doc,
        prop(4),
        PropertyValue::Date(DateValue {
            date: "2026-02-30".into(),
            time: None,
        }),
    );
    refuse(&mut doc, prop(4), PropertyValue::Number(f64::NAN));
    // Files and checklists: the kind must be the property's, and the value follows its rules.
    refuse(&mut doc, prop(1), files(&[1]));
    refuse(&mut doc, prop(1), checklist(vec![item(1, "a", false)]));
    refuse(&mut doc, prop(5), text("x"));
    refuse(&mut doc, prop(5), checklist(vec![item(1, "a", false)]));
    refuse(&mut doc, prop(6), files(&[1]));
    refuse(&mut doc, prop(5), files(&[1, 2, 1]));
    refuse(&mut doc, prop(5), files(&(1..=51).collect::<Vec<u8>>()));
    refuse(
        &mut doc,
        prop(6),
        checklist(vec![item(1, "a", false), item(1, "b", true)]),
    );
    refuse(
        &mut doc,
        prop(6),
        checklist(vec![item(1, &"x".repeat(501), false)]),
    );
    refuse(
        &mut doc,
        prop(6),
        checklist(vec![item(1, "two\nlines", false)]),
    );
    assert_eq!(doc.doc().version(), version, "nothing was written");

    assert!(matches!(
        doc.set_page_property(pid(99), prop(1), Some(text("x")), 9),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        doc.set_page_property(page, prop(99), Some(text("x")), 9),
        Err(ModelError::PropertyNotFound)
    ));
    assert!(matches!(
        doc.page_properties(pid(99)),
        Err(ModelError::PageNotFound)
    ));
    assert_healthy(&doc);
}

#[test]
fn multi_select_values_keep_their_order_and_must_name_known_distinct_options() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    for n in 1..=3 {
        doc.add_option(TAGS_PROPERTY_ID, opt(n), "t", OptionColor::Gray, None, 1)
            .unwrap();
    }
    let value = PropertyValue::MultiSelect(vec![opt(3), opt(1)]);
    set(&mut doc, page, TAGS_PROPERTY_ID, value.clone());
    assert_eq!(
        doc.page_properties(page).unwrap(),
        [(TAGS_PROPERTY_ID, value)]
    );
    for bad in [vec![opt(1), opt(1)], vec![opt(1), opt(9)]] {
        let err = doc
            .set_page_property(
                page,
                TAGS_PROPERTY_ID,
                Some(PropertyValue::MultiSelect(bad)),
                1,
            )
            .unwrap_err();
        assert_eq!(invalid_field(&err), Some("value"));
    }
    doc.set_page_property(
        page,
        TAGS_PROPERTY_ID,
        Some(PropertyValue::MultiSelect(Vec::new())),
        1,
    )
    .unwrap();
    assert!(
        doc.page_properties(page).unwrap().is_empty(),
        "an empty list clears"
    );
}

#[test]
fn trashed_pages_keep_and_take_values_and_a_restore_brings_them_back() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    set(&mut doc, page, prop(1), text("kept"));
    doc.trash_page(page, 50).unwrap();
    assert_eq!(
        doc.page(page).unwrap().properties,
        [(prop(1), text("kept"))]
    );
    assert_eq!(
        doc.trashed_pages()[0].page.properties,
        [(prop(1), text("kept"))]
    );
    set(&mut doc, page, prop(1), text("edited while trashed"));
    doc.restore_page(page, 60).unwrap();
    assert_eq!(
        doc.live_tree()[0].page.properties,
        [(prop(1), text("edited while trashed"))]
    );
    assert_healthy(&doc);
}

#[test]
fn values_of_a_deleted_property_stay_unread_and_go_with_the_next_write() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    let other = add(&mut doc, 2, None, "B");
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    create(&mut doc, 2, "Cost", PropertyKind::Number);
    set(&mut doc, page, prop(1), text("old"));
    set(&mut doc, page, prop(2), PropertyValue::Number(5.0));
    set(&mut doc, other, prop(1), text("also old"));
    doc.delete_property(prop(1)).unwrap();

    assert_eq!(
        doc.page_properties(page).unwrap(),
        [(prop(2), PropertyValue::Number(5.0))],
        "unread"
    );
    assert!(doc.page(other).unwrap().properties.is_empty());
    let index = doc.property_index(&[page, other]);
    assert!(index[0].1.rows.iter().all(|row| row.property_id == prop(2)));
    assert!(index[1].1.rows.is_empty(), "and unindexed");
    let portable = doc.to_portable();
    assert!(
        portable["pages"][0]["props"]
            .get(prop(1).to_string())
            .is_some()
            || portable["pages"][1]["props"]
                .get(prop(1).to_string())
                .is_some(),
        "still in the document"
    );
    let problems = doc.check_invariants();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().all(|p| p.contains("unknown property")));

    // The next write of the page's values removes its dangling value; the other page keeps its.
    set(&mut doc, page, prop(2), PropertyValue::Number(6.0));
    assert_eq!(doc.check_invariants().len(), 1);
    set(&mut doc, other, prop(2), PropertyValue::Number(1.0));
    assert_healthy(&doc);
}

#[test]
fn a_value_naming_an_unknown_option_is_kept_hidden_and_reported() {
    let mut a = structure(1);
    let page = add(&mut a, 1, None, "A");
    create(&mut a, 1, "Kind", PropertyKind::Select);
    add_option(&mut a, 1, 1, "red");
    add_option(&mut a, 1, 2, "green");
    let mut b = replica(&a, 2);
    // One device deletes the option while the other sets a value that names it.
    a.delete_option(prop(1), opt(2), 10).unwrap();
    set(&mut b, page, prop(1), PropertyValue::Select(opt(2)));
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert!(doc.page_properties(page).unwrap().is_empty(), "hidden");
        assert!(
            doc.property_index(&[page])[0].1.rows.is_empty(),
            "unindexed"
        );
        let problems = doc.check_invariants();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("unknown option"));
    }
    assert_eq!(a.to_portable(), b.to_portable());
}

#[test]
fn values_and_schema_survive_undo_of_structural_edits() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    doc.rename_page(page, "Renamed", 10).unwrap();
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    set(&mut doc, page, prop(1), text("v"));
    doc.rename_page(page, "Renamed again", 20_000).unwrap();
    assert!(doc.can_undo());
    doc.undo().unwrap().unwrap();
    assert_eq!(doc.page(page).unwrap().title, "Renamed");
    assert_eq!(doc.page(page).unwrap().properties, [(prop(1), text("v"))]);
    assert!(doc.property(prop(1)).is_some());
    doc.undo().unwrap().unwrap();
    doc.undo().unwrap().unwrap();
    assert!(doc.page(page).is_none(), "the creation was undone");
    assert!(doc.property(prop(1)).is_some(), "the schema was not");
    assert!(!doc.can_undo());
    doc.redo().unwrap().unwrap();
    assert_healthy(&doc);
}

#[test]
fn property_writes_use_the_excluded_origin_and_reads_never_write() {
    // A read-only pass over an existing document leaves its version alone.
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    set(&mut doc, page, prop(1), text("v"));
    let version = doc.doc().version();
    let _ = doc.property_schema();
    let _ = doc.live_tree();
    let _ = doc.page_properties(page);
    let _ = doc.property_index_all();
    let _ = doc.to_portable();
    let _ = doc.check_invariants();
    assert_eq!(doc.doc().version(), version);
    assert!(PROPERTIES_ORIGIN.starts_with("kb-model/"));
}

// ----- derived rows ---------------------------------------------------------------------------

#[test]
fn derived_rows_hold_one_row_per_scalar_and_one_per_selected_option() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    let empty = add(&mut doc, 2, None, "Empty");
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    create(&mut doc, 2, "Cost", PropertyKind::Number);
    create(&mut doc, 3, "Done", PropertyKind::Checkbox);
    create(&mut doc, 4, "When", PropertyKind::Date);
    create(&mut doc, 5, "Stage", PropertyKind::Status);
    create(&mut doc, 6, "Email", PropertyKind::Email);
    doc.add_option(
        TAGS_PROPERTY_ID,
        opt(1),
        "urgent",
        OptionColor::Red,
        None,
        1,
    )
    .unwrap();
    doc.add_option(TAGS_PROPERTY_ID, opt(2), "work", OptionColor::Blue, None, 1)
        .unwrap();
    set(&mut doc, page, prop(1), text("Quarterly plan"));
    set(&mut doc, page, prop(2), PropertyValue::Number(42.5));
    set(&mut doc, page, prop(3), PropertyValue::Checkbox(false));
    set(
        &mut doc,
        page,
        prop(4),
        PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
        }),
    );
    set(&mut doc, page, prop(5), PropertyValue::Status(opt(101)));
    set(
        &mut doc,
        page,
        prop(6),
        PropertyValue::Email("a@b.c".into()),
    );
    set(
        &mut doc,
        page,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![opt(2), opt(1)]),
    );

    let index = doc.property_index(&[page, empty, pid(99)]);
    assert_eq!(index.len(), 2, "unknown pages have no entry");
    assert_eq!(index[1], (empty, crate::PageIndexEntry::default()));
    let (found, entry) = &index[0];
    assert_eq!(*found, page);
    // Schema order: Tags first (two rows, in the order of the list), then the others.
    let simple: Vec<(PropertyId, PropertyKind, Option<OptionId>)> = entry
        .rows
        .iter()
        .map(|row| (row.property_id, row.kind, row.option_id))
        .collect();
    assert_eq!(
        simple,
        [
            (TAGS_PROPERTY_ID, PropertyKind::MultiSelect, Some(opt(2))),
            (TAGS_PROPERTY_ID, PropertyKind::MultiSelect, Some(opt(1))),
            (prop(1), PropertyKind::Text, None),
            (prop(2), PropertyKind::Number, None),
            (prop(3), PropertyKind::Checkbox, None),
            (prop(4), PropertyKind::Date, None),
            (prop(5), PropertyKind::Status, Some(opt(101))),
            (prop(6), PropertyKind::Email, None),
        ]
    );
    let by = |n: u8| {
        entry
            .rows
            .iter()
            .find(|r| r.property_id == prop(n))
            .unwrap()
    };
    assert_eq!(by(1).value_text.as_deref(), Some("Quarterly plan"));
    assert_eq!(by(2).value_number, Some(42.5));
    assert_eq!(by(3).value_bool, Some(false));
    assert_eq!(by(4).value_date.as_deref(), Some("2026-10-07"));
    assert_eq!(by(4).value_time.as_deref(), Some("14:30"));
    assert_eq!(by(6).value_text.as_deref(), Some("a@b.c"));
    assert!(by(5).value_text.is_none() && by(5).value_number.is_none());

    // The text index gets option names and text-like values only.
    assert_eq!(
        entry.texts,
        [
            (TAGS_PROPERTY_ID, "work, urgent".to_owned()),
            (prop(1), "Quarterly plan".to_owned()),
            (prop(5), "In progress".to_owned()),
            (prop(6), "a@b.c".to_owned()),
        ]
    );

    // `property_index_all` lists the pages that have rows, by page id.
    let all = doc.property_index_all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, page);
    assert_eq!(all[0].1, *entry);
}

#[test]
fn a_files_value_has_one_row_per_attachment_and_its_text_is_the_manifest_names() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Attached", PropertyKind::Files);
    put_manifest(&doc, 1, "plan.pdf");
    put_manifest(&doc, 2, "atomik.png");
    // Existence is not checked when a value is written: attachment 3 has no manifest (yet).
    set(&mut doc, page, prop(1), files(&[2, 3, 1]));

    let (_, entry) = &doc.property_index(&[page])[0];
    let rows: Vec<(PropertyKind, Option<AttachmentId>)> = entry
        .rows
        .iter()
        .map(|row| (row.kind, row.attachment_id))
        .collect();
    assert_eq!(
        rows,
        [
            (PropertyKind::Files, Some(file(2))),
            (PropertyKind::Files, Some(file(3))),
            (PropertyKind::Files, Some(file(1))),
        ],
        "in the order of the list"
    );
    assert!(entry.rows.iter().all(|row| {
        row.value_text.is_none()
            && row.value_number.is_none()
            && row.option_id.is_none()
            && row.value_total.is_none()
    }));
    assert_eq!(
        entry.texts,
        [(prop(1), "atomik.png, plan.pdf".to_owned())],
        "the file names of the manifests that exist, in order"
    );

    // The manifest of attachment 3 arrives later: the text follows, the rows do not change.
    put_manifest(&doc, 3, "notes.txt");
    let (_, later) = &doc.property_index(&[page])[0];
    assert_eq!(later.rows, entry.rows);
    assert_eq!(later.texts[0].1, "atomik.png, notes.txt, plan.pdf");

    // Reordering writes the new order, removing the last file clears the value.
    set(&mut doc, page, prop(1), files(&[1, 2]));
    assert_eq!(
        doc.page_properties(page).unwrap(),
        [(prop(1), files(&[1, 2]))]
    );
    assert_eq!(
        doc.set_page_property(page, prop(1), Some(files(&[])), 7)
            .unwrap(),
        vec![page],
        "an empty list clears"
    );
    assert!(doc.page_properties(page).unwrap().is_empty());
    assert!(doc.property_index(&[page])[0].1.rows.is_empty());
    assert_eq!(replica(&doc, 2).page_properties(page).unwrap(), []);
    assert_healthy(&doc);
}

#[test]
fn a_checklist_has_one_row_with_the_done_count_and_the_total() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "To do", PropertyKind::Checklist);
    set(
        &mut doc,
        page,
        prop(1),
        checklist(vec![
            item(1, "Pack", true),
            item(2, "", false),
            item(3, "Call Ada", true),
            item(4, "Book", false),
        ]),
    );
    let (_, entry) = &doc.property_index(&[page])[0];
    assert_eq!(entry.rows.len(), 1);
    let row = &entry.rows[0];
    assert_eq!(row.kind, PropertyKind::Checklist);
    assert_eq!(row.value_number, Some(2.0), "items done");
    assert_eq!(row.value_total, Some(4), "items");
    assert!(row.value_text.is_none() && row.option_id.is_none() && row.attachment_id.is_none());
    assert_eq!(
        entry.texts,
        [(prop(1), "Pack, Call Ada, Book".to_owned())],
        "the texts of the items that have one"
    );

    // All done, and a list of blank items (no text to index).
    set(&mut doc, page, prop(1), checklist(vec![item(1, "a", true)]));
    let (_, done) = &doc.property_index(&[page])[0];
    assert_eq!(
        (done.rows[0].value_number, done.rows[0].value_total),
        (Some(1.0), Some(1))
    );
    set(
        &mut doc,
        page,
        prop(1),
        checklist(vec![item(1, " ", false)]),
    );
    let (_, blank) = &doc.property_index(&[page])[0];
    assert!(blank.texts.is_empty());
    assert_eq!(blank.rows.len(), 1);

    // The order, the ids and the flags are what was written.
    let value = checklist(vec![item(9, "z", false), item(2, "a", true)]);
    set(&mut doc, page, prop(1), value.clone());
    assert_eq!(
        doc.page_properties(page).unwrap(),
        [(prop(1), value.clone())]
    );
    assert_eq!(
        replica(&doc, 2).page_properties(page).unwrap(),
        [(prop(1), value)]
    );
    assert_healthy(&doc);
}

#[test]
fn a_property_of_the_new_kinds_is_removed_from_the_index_when_it_is_deleted() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Attached", PropertyKind::Files);
    create(&mut doc, 2, "To do", PropertyKind::Checklist);
    set(&mut doc, page, prop(1), files(&[1]));
    set(
        &mut doc,
        page,
        prop(2),
        checklist(vec![item(1, "a", false)]),
    );
    assert_eq!(doc.property_index(&[page])[0].1.rows.len(), 2);
    doc.delete_property(prop(1)).unwrap();
    doc.delete_property(prop(2)).unwrap();
    assert!(doc.page_properties(page).unwrap().is_empty());
    assert!(doc.property_index(&[page])[0].1.rows.is_empty());
}

// ----- merging --------------------------------------------------------------------------------

#[test]
fn concurrent_first_values_of_different_properties_of_one_page_both_survive() {
    let mut a = structure(1);
    let page = add(&mut a, 1, None, "Shared");
    create(&mut a, 1, "Notes", PropertyKind::Text);
    create(&mut a, 2, "Cost", PropertyKind::Number);
    let mut b = replica(&a, 2);
    assert!(
        a.to_portable()["pages"][0].get("props").is_none(),
        "no values yet on either"
    );

    // Neither replica has written a value of this page: both create its `props` map.
    set(&mut a, page, prop(1), text("from a"));
    set(&mut b, page, prop(2), PropertyValue::Number(7.0));
    sync(&mut a, &mut b);
    let expected = [
        (prop(1), text("from a")),
        (prop(2), PropertyValue::Number(7.0)),
    ];
    assert_eq!(a.page_properties(page).unwrap(), expected);
    assert_eq!(b.page_properties(page).unwrap(), expected);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
}

#[test]
fn concurrent_edits_of_the_same_value_converge_last_writer_wins() {
    let mut a = structure(1);
    let page = add(&mut a, 1, None, "Shared");
    create(&mut a, 1, "Notes", PropertyKind::Text);
    set(&mut a, page, prop(1), text("start"));
    let mut b = replica(&a, 2);
    set(&mut a, page, prop(1), text("a"));
    set(&mut b, page, prop(1), text("b"));
    sync(&mut a, &mut b);
    assert_eq!(
        a.page_properties(page).unwrap(),
        b.page_properties(page).unwrap()
    );
    let winner = a.page_properties(page).unwrap();
    assert!(winner == [(prop(1), text("a"))] || winner == [(prop(1), text("b"))]);
}

#[test]
fn concurrent_edits_of_files_and_checklists_follow_the_per_property_rules() {
    let mut a = structure(1);
    let page = add(&mut a, 1, None, "Shared");
    create(&mut a, 1, "Attached", PropertyKind::Files);
    create(&mut a, 2, "To do", PropertyKind::Checklist);
    set(&mut a, page, prop(1), files(&[1]));
    let mut b = replica(&a, 2);
    // Different properties of one page merge; two edits of one value are last-writer-wins
    // (a list is written as a whole).
    set(&mut a, page, prop(1), files(&[1, 2]));
    set(&mut b, page, prop(1), files(&[1, 3]));
    set(&mut b, page, prop(2), checklist(vec![item(1, "x", false)]));
    sync(&mut a, &mut b);
    assert_eq!(
        a.page_properties(page).unwrap(),
        b.page_properties(page).unwrap()
    );
    let merged = a.page_properties(page).unwrap();
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1], (prop(2), checklist(vec![item(1, "x", false)])));
    assert!(merged[0].1 == files(&[1, 2]) || merged[0].1 == files(&[1, 3]));
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
}

#[test]
fn a_clear_and_a_set_of_different_properties_merge() {
    let mut a = structure(1);
    let page = add(&mut a, 1, None, "Shared");
    create(&mut a, 1, "Notes", PropertyKind::Text);
    create(&mut a, 2, "Cost", PropertyKind::Number);
    set(&mut a, page, prop(1), text("x"));
    let mut b = replica(&a, 2);
    a.set_page_property(page, prop(1), None, 10).unwrap();
    set(&mut b, page, prop(2), PropertyValue::Number(1.0));
    sync(&mut a, &mut b);
    let expected = [(prop(2), PropertyValue::Number(1.0))];
    assert_eq!(a.page_properties(page).unwrap(), expected);
    assert_eq!(b.page_properties(page).unwrap(), expected);
}

#[test]
fn concurrent_options_added_to_one_property_keep_both() {
    let mut a = structure(1);
    create(&mut a, 1, "Kind", PropertyKind::Select);
    add_option(&mut a, 1, 1, "shared");
    let mut b = replica(&a, 2);
    add_option(&mut a, 1, 2, "from a");
    add_option(&mut b, 1, 3, "from b");
    sync(&mut a, &mut b);
    let listed = |doc: &StructureDoc| -> Vec<String> {
        doc.property(prop(1))
            .unwrap()
            .options
            .into_iter()
            .map(|o| o.name)
            .collect()
    };
    assert_eq!(listed(&a).len(), 3);
    assert_eq!(listed(&a), listed(&b));
    assert!(listed(&a).contains(&"from a".to_owned()) && listed(&a).contains(&"from b".to_owned()));
    assert_healthy(&a);
}

#[test]
fn the_first_tags_written_on_two_replicas_merge_their_options() {
    // Neither replica has Tags in its document: both write it, with different tags.
    let mut a = structure(1);
    let mut b = replica(&a, 2);
    assert!(a.to_portable().get("properties").is_none());
    a.add_option(
        TAGS_PROPERTY_ID,
        opt(1),
        "from a",
        OptionColor::Red,
        None,
        10,
    )
    .unwrap();
    b.add_option(
        TAGS_PROPERTY_ID,
        opt(2),
        "from b",
        OptionColor::Blue,
        None,
        11,
    )
    .unwrap();
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        let tags = doc.property(TAGS_PROPERTY_ID).unwrap();
        let mut found: Vec<String> = tags.options.iter().map(|o| o.name.clone()).collect();
        found.sort();
        assert_eq!(found, ["from a", "from b"], "both tags survive");
        assert_eq!(tags.name, "Tags");
        assert_eq!(doc.property_schema().len(), 1);
    }
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
}

#[test]
fn a_rename_and_a_move_and_an_option_edit_of_one_property_merge_field_by_field() {
    let mut a = structure(1);
    create(&mut a, 1, "A", PropertyKind::Select);
    create(&mut a, 2, "B", PropertyKind::Text);
    add_option(&mut a, 1, 1, "red");
    let mut b = replica(&a, 2);
    a.rename_property(prop(1), "Colour", 10).unwrap();
    a.update_option(prop(1), opt(1), Some("crimson"), None, None)
        .unwrap();
    b.move_property(prop(1), None).unwrap();
    b.update_option(prop(1), opt(1), None, Some(OptionColor::Pink), None)
        .unwrap();
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert_eq!(names(doc), ["Tags", "B", "Colour"], "renamed and moved");
        let option = doc.property(prop(1)).unwrap().options[0].clone();
        assert_eq!(
            (option.name.as_str(), option.color),
            ("crimson", OptionColor::Pink),
            "name from one, colour from the other"
        );
    }
    assert_eq!(a.to_portable(), b.to_portable());
}

#[test]
fn a_concurrent_value_write_and_option_cascade_leave_both_replicas_equal() {
    let mut a = structure(1);
    let page = add(&mut a, 1, None, "P");
    create(&mut a, 1, "Kind", PropertyKind::Select);
    add_option(&mut a, 1, 1, "red");
    set(&mut a, page, prop(1), PropertyValue::Select(opt(1)));
    let mut b = replica(&a, 2);
    a.delete_option(prop(1), opt(1), 10).unwrap();
    b.rename_page(page, "Renamed", 11).unwrap();
    sync(&mut a, &mut b);
    assert!(a.page_properties(page).unwrap().is_empty());
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(a.page(page).unwrap().title, "Renamed");
    assert_healthy(&a);
}

// ----- the portable form ----------------------------------------------------------------------

#[test]
fn the_portable_form_lists_the_schema_and_the_stored_values() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Stage", PropertyKind::Status);
    create(&mut doc, 2, "When", PropertyKind::Date);
    set(&mut doc, page, prop(1), PropertyValue::Status(opt(101)));
    set(
        &mut doc,
        page,
        prop(2),
        PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: None,
        }),
    );
    create(&mut doc, 3, "Attached", PropertyKind::Files);
    create(&mut doc, 4, "To do", PropertyKind::Checklist);
    set(&mut doc, page, prop(3), files(&[2, 1]));
    set(
        &mut doc,
        page,
        prop(4),
        checklist(vec![item(1, "Pack", true)]),
    );
    let portable = doc.to_portable();
    let stage = &portable["properties"][prop(1).to_string()];
    assert_eq!(stage["name"], "Stage");
    assert_eq!(stage["kind"], "status");
    assert_eq!(stage["createdAt"], 1_001);
    assert_eq!(
        stage["options"][opt(101).to_string()],
        json!({"color": "blue", "group": "in_progress", "name": "In progress", "position": stage["options"][opt(101).to_string()]["position"]})
    );
    let props = &portable["pages"][0]["props"];
    assert_eq!(props[prop(1).to_string()], json!(opt(101).to_string()));
    assert_eq!(props[prop(2).to_string()], json!({"date": "2026-10-07"}));
    assert_eq!(
        props[prop(3).to_string()],
        json!([file(2).to_string(), file(1).to_string()])
    );
    assert_eq!(
        props[prop(4).to_string()],
        json!([{"done": true, "id": item(1, "", false).id.to_string(), "text": "Pack"}])
    );
    assert_eq!(portable["properties"][prop(3).to_string()]["kind"], "files");
    assert_eq!(
        portable["properties"][prop(4).to_string()]["kind"],
        "checklist"
    );
    // A replica loaded from a snapshot exports the same text.
    assert_eq!(replica(&doc, 2).to_portable(), portable);
}

#[test]
fn an_undone_purge_or_trash_keeps_the_page_values() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, "Notes", PropertyKind::Text);
    doc.add_option(TAGS_PROPERTY_ID, opt(1), "t", OptionColor::Gray, None, 1)
        .unwrap();
    set(&mut doc, page, prop(1), text("kept"));
    set(
        &mut doc,
        page,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![opt(1)]),
    );
    let expected = doc.page_properties(page).unwrap();
    assert_eq!(expected.len(), 2);

    doc.trash_page(page, 100).unwrap();
    doc.purge_page(page, 101).unwrap();
    assert!(doc.page(page).is_none());
    doc.undo().unwrap().unwrap();
    assert_eq!(
        doc.page_properties(page).unwrap(),
        expected,
        "undo of the purge"
    );
    doc.undo().unwrap().unwrap();
    assert_eq!(
        doc.page_properties(page).unwrap(),
        expected,
        "undo of the trash"
    );
    doc.redo().unwrap().unwrap();
    doc.redo().unwrap().unwrap();
    assert!(doc.page(page).is_none());
    doc.undo().unwrap().unwrap();
    doc.undo().unwrap().unwrap();
    // The values are still writable, and the replica agrees.
    set(&mut doc, page, prop(1), text("again"));
    assert_eq!(
        replica(&doc, 2).page_properties(page).unwrap(),
        doc.page_properties(page).unwrap()
    );
    assert_healthy(&doc);
}
