//! The `place` kind and the computed kinds (created and last edited time and device, ADR-0026
//! amendment of 2026-10-08): validation, storage, attribution to a device, the throttled stamp of
//! a content edit and merging.

use kb_types::properties::MAX_PLACE_NAME_CHARS;
use kb_types::{DeviceId, OptionId, PageId, PlaceValue, PropertyId, PropertyKind, PropertyValue};

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{EDIT_STAMP_INTERVAL_MS, ModelError, StructureDoc};

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xe0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn device(n: u8) -> DeviceId {
    DeviceId::from_random_bytes([0xf0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn create(doc: &mut StructureDoc, n: u8, kind: PropertyKind) {
    doc.create_property(
        prop(n),
        &format!("{kind:?}"),
        kind,
        &mut || OptionId::from_random_bytes([1; 16]),
        10,
    )
    .unwrap();
}

fn place(name: &str, at: Option<(f64, f64)>) -> PropertyValue {
    PropertyValue::Place(PlaceValue {
        name: name.to_owned(),
        lat: at.map(|c| c.0),
        lon: at.map(|c| c.1),
    })
}

fn refused(doc: &mut StructureDoc, page: PageId, n: u8, value: PropertyValue) -> String {
    match doc.set_page_property(page, prop(n), Some(value), 99) {
        Err(ModelError::InvalidProperty { message, .. }) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn value_of(doc: &StructureDoc, page: PageId, n: u8) -> Option<PropertyValue> {
    doc.page_properties(page)
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == prop(n))
        .map(|(_, value)| value)
}

// ----- place ----------------------------------------------------------------------------------

#[test]
fn a_place_is_stored_read_back_indexed_and_cleared() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, PropertyKind::Place);
    for value in [
        place("Café de Flore, Paris", Some((48.854, 2.333))),
        place("Home", None),
        place("South pole", Some((-90.0, -180.0))),
        place("North east", Some((90.0, 180.0))),
    ] {
        doc.set_page_property(page, prop(1), Some(value.clone()), 100)
            .unwrap();
        assert_eq!(value_of(&doc, page, 1), Some(value.clone()));
        assert_eq!(value_of(&replica(&doc, 2), page, 1), Some(value.clone()));
        let entry = &doc.property_index(&[page])[0].1;
        let PropertyValue::Place(shown) = &value else {
            unreachable!()
        };
        assert_eq!(entry.rows.len(), 1);
        assert_eq!(entry.rows[0].kind, PropertyKind::Place);
        assert_eq!(
            entry.rows[0].value_text.as_deref(),
            Some(shown.name.as_str())
        );
        assert_eq!(entry.texts, [(prop(1), shown.name.clone())]);
    }
    assert_healthy(&doc);
    doc.set_page_property(page, prop(1), None, 200).unwrap();
    assert_eq!(value_of(&doc, page, 1), None);
    assert!(doc.property_index(&[page])[0].1.rows.is_empty());
}

#[test]
fn a_place_must_have_a_one_line_name_and_both_or_neither_coordinate_in_range() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    create(&mut doc, 1, PropertyKind::Place);
    let long = "x".repeat(MAX_PLACE_NAME_CHARS + 1);
    assert_eq!(
        refused(&mut doc, page, 1, place("", None)),
        "must not be empty"
    );
    assert_eq!(
        refused(&mut doc, page, 1, place("  ", None)),
        "must not be empty"
    );
    assert_eq!(
        refused(&mut doc, page, 1, place(&long, None)),
        format!("must be at most {MAX_PLACE_NAME_CHARS} characters")
    );
    assert_eq!(
        refused(&mut doc, page, 1, place("a\nb", None)),
        "must not contain control characters"
    );
    let together = "the latitude and the longitude go together";
    let one = |lat, lon| {
        PropertyValue::Place(PlaceValue {
            name: "x".into(),
            lat,
            lon,
        })
    };
    assert_eq!(refused(&mut doc, page, 1, one(Some(1.0), None)), together);
    assert_eq!(refused(&mut doc, page, 1, one(None, Some(1.0))), together);
    for lat in [90.5, -91.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            refused(&mut doc, page, 1, place("x", Some((lat, 0.0)))),
            "the latitude must be between -90 and 90"
        );
    }
    for lon in [180.5, -181.0, f64::NAN, f64::NEG_INFINITY] {
        assert_eq!(
            refused(&mut doc, page, 1, place("x", Some((0.0, lon)))),
            "the longitude must be between -180 and 180"
        );
    }
    assert_eq!(value_of(&doc, page, 1), None, "nothing was written");
    // The longest name is fine.
    let longest = "é".repeat(MAX_PLACE_NAME_CHARS);
    doc.set_page_property(page, prop(1), Some(place(&longest, None)), 5)
        .unwrap();
}

// ----- computed kinds ---------------------------------------------------------------------------

#[test]
fn a_computed_property_refuses_values_and_has_no_rows() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "A");
    for (n, kind) in [
        (1, PropertyKind::CreatedTime),
        (2, PropertyKind::LastEditedTime),
        (3, PropertyKind::CreatedBy),
        (4, PropertyKind::LastEditedBy),
    ] {
        create(&mut doc, n, kind);
        assert!(kind.is_computed());
        for value in [
            Some(PropertyValue::CreatedTime(5)),
            Some(PropertyValue::CreatedBy(device(1))),
            None,
        ] {
            let err = doc.set_page_property(page, prop(n), value, 99).unwrap_err();
            assert!(
                matches!(
                    err,
                    ModelError::InvalidProperty {
                        field: "propertyId",
                        ..
                    }
                ),
                "{err:?}"
            );
        }
    }
    assert!(doc.property_index(&[page])[0].1.rows.is_empty());
    assert!(doc.property_index_all().is_empty());
    assert_healthy(&doc);
}

#[test]
fn times_are_read_from_the_page_and_the_last_edit_follows_every_change() {
    let mut doc = structure(1);
    create(&mut doc, 1, PropertyKind::CreatedTime);
    create(&mut doc, 2, PropertyKind::LastEditedTime);
    let page = add(&mut doc, 7, None, "A"); // created at 7
    assert_eq!(value_of(&doc, page, 1), Some(PropertyValue::CreatedTime(7)));
    assert_eq!(
        value_of(&doc, page, 2),
        Some(PropertyValue::LastEditedTime(7))
    );
    doc.rename_page(page, "B", 500).unwrap();
    assert_eq!(
        doc.page(page).unwrap().properties,
        [
            (prop(1), PropertyValue::CreatedTime(7)),
            (prop(2), PropertyValue::LastEditedTime(500))
        ]
    );
    assert_eq!(
        doc.live_tree()[0].page.properties.len(),
        2,
        "the tree carries them too"
    );
    assert_healthy(&doc);
}

#[test]
fn devices_are_recorded_on_create_and_on_every_edit_and_old_pages_have_none() {
    let mut doc = structure(1);
    create(&mut doc, 3, PropertyKind::CreatedBy);
    create(&mut doc, 4, PropertyKind::LastEditedBy);
    let old = add(&mut doc, 1, None, "Old"); // no author set yet
    assert_eq!(value_of(&doc, old, 3), None);
    assert_eq!(value_of(&doc, old, 4), None);

    doc.set_author(Some(device(1)));
    let page = add(&mut doc, 2, None, "New");
    assert_eq!(
        value_of(&doc, page, 3),
        Some(PropertyValue::CreatedBy(device(1)))
    );
    assert_eq!(
        value_of(&doc, page, 4),
        Some(PropertyValue::LastEditedBy(device(1)))
    );
    // Editing an old page records the editor only.
    doc.rename_page(old, "Old, renamed", 100).unwrap();
    assert_eq!(value_of(&doc, old, 3), None);
    assert_eq!(
        value_of(&doc, old, 4),
        Some(PropertyValue::LastEditedBy(device(1)))
    );

    // Another device edits after a sync: both devices read the same attribution.
    let mut other = replica(&doc, 2);
    other.set_author(Some(device(2)));
    other.rename_page(page, "Edited elsewhere", 200).unwrap();
    sync(&mut doc, &mut other);
    for d in [&doc, &other] {
        assert_eq!(
            value_of(d, page, 3),
            Some(PropertyValue::CreatedBy(device(1)))
        );
        assert_eq!(
            value_of(d, page, 4),
            Some(PropertyValue::LastEditedBy(device(2)))
        );
    }
    assert_eq!(doc.to_portable(), other.to_portable());
    let portable = doc.to_portable();
    assert_eq!(
        portable["pages"][1]["createdBy"],
        serde_json::json!(device(1).to_string())
    );
    assert_healthy(&doc);
}

#[test]
fn page_info_names_the_devices_that_created_and_last_edited_the_page() {
    let mut doc = structure(1);
    let old = add(&mut doc, 1, None, "Old"); // no author set yet
    let info = doc.page(old).unwrap();
    assert_eq!((info.created_by, info.edited_by), (None, None));

    doc.set_author(Some(device(1)));
    let page = add(&mut doc, 2, None, "New");
    let info = doc.page(page).unwrap();
    assert_eq!(info.created_by, Some(device(1)));
    assert_eq!(info.edited_by, Some(device(1)));
    // Editing an old page records the editor only.
    doc.rename_page(old, "Old, renamed", 100).unwrap();
    let info = doc.page(old).unwrap();
    assert_eq!((info.created_by, info.edited_by), (None, Some(device(1))));

    // Another device edits after a sync: both read the same, in the tree as well.
    let mut other = replica(&doc, 2);
    other.set_author(Some(device(2)));
    other.rename_page(page, "Edited elsewhere", 200).unwrap();
    sync(&mut doc, &mut other);
    for d in [&doc, &other] {
        let info = d.page(page).unwrap();
        assert_eq!(info.created_by, Some(device(1)));
        assert_eq!(info.edited_by, Some(device(2)));
        assert_eq!(d.live_tree()[1].page, info);
    }
    assert_healthy(&doc);
}

// ----- the throttled stamp of a content edit --------------------------------------------------

#[test]
fn a_content_edit_stamps_the_page_at_most_once_per_interval_per_device() {
    let mut doc = structure(1);
    doc.set_author(Some(device(1)));
    let page = add(&mut doc, 1, None, "A"); // t = 1
    let t0 = 1_000_000;
    assert_eq!(doc.touch_edited(page, t0).unwrap(), vec![page]);
    assert_eq!(doc.page(page).unwrap().updated_at, t0);
    // Inside the interval: nothing is written.
    let version = doc.doc().version();
    assert!(doc.touch_edited(page, t0 + 1).unwrap().is_empty());
    assert!(
        doc.touch_edited(page, t0 + EDIT_STAMP_INTERVAL_MS - 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(doc.doc().version(), version, "no operation was written");
    assert_eq!(doc.page(page).unwrap().updated_at, t0);
    // At the interval: stamped again.
    let t1 = t0 + EDIT_STAMP_INTERVAL_MS;
    assert_eq!(doc.touch_edited(page, t1).unwrap(), vec![page]);
    assert_eq!(doc.page(page).unwrap().updated_at, t1);
    // The clock never moves a stamp backwards.
    assert!(doc.touch_edited(page, t1 - 500_000).unwrap().is_empty());
    assert_eq!(doc.page(page).unwrap().updated_at, t1);
    assert!(matches!(
        doc.touch_edited(pid(99), t1),
        Err(ModelError::PageNotFound)
    ));
    assert_healthy(&doc);
}

#[test]
fn an_edit_by_another_device_is_stamped_at_once() {
    let mut a = structure(1);
    a.set_author(Some(device(1)));
    let page = add(&mut a, 1, None, "A");
    a.touch_edited(page, 1_000_000).unwrap();
    let mut b = replica(&a, 2);
    b.set_author(Some(device(2)));
    assert_eq!(b.touch_edited(page, 1_000_050).unwrap(), vec![page]);
    assert_eq!(b.page(page).unwrap().updated_at, 1_000_050);
    sync(&mut a, &mut b);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(a.page(page).unwrap().updated_at, 1_000_050);
}

#[test]
fn the_stamp_of_a_content_edit_is_not_an_undo_step_and_survives_undo() {
    let mut doc = structure(1);
    doc.set_author(Some(device(1)));
    let page = add(&mut doc, 1, None, "A");
    doc.rename_page(page, "B", 10_000).unwrap();
    doc.touch_edited(page, 900_000).unwrap();
    assert!(doc.can_undo());
    // The rename is undone; the stamp is not an undo step of its own.
    doc.undo().unwrap().unwrap();
    assert_eq!(doc.page(page).unwrap().title, "A");
    doc.undo().unwrap().unwrap();
    assert!(doc.page(page).is_none(), "the next undo is the creation");
}

#[test]
fn concurrent_stamps_of_one_page_converge() {
    let mut a = structure(1);
    a.set_author(Some(device(1)));
    let page = add(&mut a, 1, None, "A");
    let mut b = replica(&a, 2);
    b.set_author(Some(device(2)));
    a.touch_edited(page, 2_000_000).unwrap();
    b.touch_edited(page, 2_000_100).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(a.page(page).unwrap(), b.page(page).unwrap());
    assert_healthy(&a);
}
