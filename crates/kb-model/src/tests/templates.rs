//! Templates (ADR-0056): the node meta `template` and `template_values`, what a page reports, the
//! writers and their refusals, the interplay with property values, hiding (`in_template`), undo, the
//! portable form, moves between spaces and the merge of two devices.

use kb_doc::{Doc, DocKind};
use kb_types::{DateValue, OptionId, PropertyId, PropertyKind, PropertyValue, TemplatePlaceholder};
use serde_json::json;

use super::{add, assert_healthy, pid, replica, sid, structure, sync};
use crate::{ModelError, MovedPage, SpaceMeta, StructureDoc, placeholder_fits};

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xb0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn opt(n: u8) -> OptionId {
    OptionId::from_random_bytes([0xc0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn create(doc: &mut StructureDoc, n: u8, name: &str, kind: PropertyKind) {
    let mut next = 100;
    doc.create_property(
        prop(n),
        name,
        kind,
        &mut || {
            next += 1;
            opt(next - 1)
        },
        1_000 + i64::from(n),
    )
    .unwrap();
}

fn day(date: &str) -> PropertyValue {
    PropertyValue::Date(DateValue {
        date: date.into(),
        time: None,
    })
}

/// A space with a Due date (property 1), a Owner person (2) and a Notes text (3), and the pages
/// `1` (a top-level page) with children `2` and `3`, and `4` (a top-level page).
fn space() -> StructureDoc {
    let mut doc = structure(1);
    create(&mut doc, 1, "Due", PropertyKind::Date);
    create(&mut doc, 2, "Owner", PropertyKind::Person);
    create(&mut doc, 3, "Notes", PropertyKind::Text);
    add(&mut doc, 1, None, "Template");
    add(&mut doc, 2, Some(1), "Child");
    add(&mut doc, 3, Some(1), "Other child");
    add(&mut doc, 4, None, "Plain");
    doc
}

/// `(template, placeholders)` of page `n`.
fn state(doc: &StructureDoc, n: u8) -> (bool, Vec<(PropertyId, TemplatePlaceholder)>) {
    let page = doc.page(pid(n)).unwrap();
    (page.template, page.template_placeholders)
}

/// The keys the node meta of page `n` holds, sorted.
fn meta_keys(doc: &StructureDoc, n: u8) -> Vec<String> {
    let node = doc.index.node(pid(n)).unwrap();
    let mut keys: Vec<String> = doc
        .meta_of(node)
        .unwrap()
        .keys()
        .map(|key| key.to_string())
        .collect();
    keys.sort();
    keys
}

fn due_of(doc: &StructureDoc, n: u8) -> Option<PropertyValue> {
    doc.page(pid(n))
        .unwrap()
        .properties
        .into_iter()
        .find(|(id, _)| *id == prop(1))
        .map(|(_, value)| value)
}

#[test]
fn a_page_is_made_a_template_and_a_page_again() {
    let mut doc = space();
    assert_eq!(state(&doc, 1), (false, Vec::new()));
    assert!(!meta_keys(&doc, 1).contains(&"template".to_owned()));

    // The affected pages are the page and its whole live subtree, in pre-order.
    assert_eq!(
        doc.set_page_template(pid(1), true, 100).unwrap(),
        vec![pid(1), pid(2), pid(3)]
    );
    assert!(state(&doc, 1).0);
    assert_eq!(
        doc.page(pid(1)).unwrap().updated_at,
        100,
        "the page is stamped"
    );
    // The sub-pages are inside the template without carrying the flag themselves.
    assert!(!state(&doc, 2).0);
    assert!(doc.in_template(pid(1)) && doc.in_template(pid(2)) && doc.in_template(pid(3)));
    assert!(!doc.in_template(pid(4)));
    // The flag is the boolean `true`, the key absent again when it is off.
    let node = doc.index.node(pid(1)).unwrap();
    assert_eq!(
        crate::meta::value(&doc.meta_of(node).unwrap(), "template"),
        Some(loro::LoroValue::Bool(true))
    );
    // The tree reports it too.
    assert!(doc.live_tree()[0].page.template);

    // Setting what the page already is writes nothing.
    let version = doc.doc().version();
    assert!(doc.set_page_template(pid(1), true, 110).unwrap().is_empty());
    assert_eq!(doc.doc().version(), version);

    assert_eq!(
        doc.set_page_template(pid(1), false, 120).unwrap(),
        vec![pid(1), pid(2), pid(3)]
    );
    assert_eq!(state(&doc, 1), (false, Vec::new()));
    assert!(!meta_keys(&doc, 1).contains(&"template".to_owned()));
    assert!(!doc.in_template(pid(2)));
    assert!(
        doc.set_page_template(pid(1), false, 130)
            .unwrap()
            .is_empty()
    );
    assert_healthy(&doc);
}

#[test]
fn the_flag_is_refused_for_a_page_in_the_trash_and_for_an_unknown_page() {
    let mut doc = space();
    doc.trash_page(pid(4), 10).unwrap();
    assert!(matches!(
        doc.set_page_template(pid(4), true, 11),
        Err(ModelError::PageTrashed)
    ));
    // A page inside a trashed subtree is in the trash too.
    doc.trash_page(pid(1), 12).unwrap();
    assert!(matches!(
        doc.set_page_template(pid(2), true, 13),
        Err(ModelError::PageTrashed)
    ));
    assert!(matches!(
        doc.set_page_template(pid(99), true, 14),
        Err(ModelError::PageNotFound)
    ));
    assert!(!doc.in_template(pid(99)));
    assert_healthy(&doc);
}

#[test]
fn a_page_is_in_a_template_by_its_ancestors_and_moves_change_it() {
    let mut doc = space();
    doc.set_page_template(pid(1), true, 100).unwrap();
    assert_eq!(
        doc.pages_in_templates(),
        [pid(1), pid(2), pid(3)].into_iter().collect()
    );

    // Out: a move out of the template leaves it (the affected ids are the moved subtree).
    assert_eq!(doc.move_page(pid(3), None, 0, 110).unwrap(), vec![pid(3)]);
    assert!(!doc.in_template(pid(3)));
    // In: a page moved under the template is inside it, with its own sub-pages.
    add(&mut doc, 5, Some(4), "Under plain");
    assert_eq!(
        doc.move_page(pid(4), Some(pid(2)), 0, 120).unwrap(),
        vec![pid(4), pid(5)]
    );
    assert!(doc.in_template(pid(4)) && doc.in_template(pid(5)));
    assert_eq!(
        doc.pages_in_templates(),
        [pid(1), pid(2), pid(4), pid(5)].into_iter().collect()
    );

    // A template nested in a template, and a trashed template: the trash keeps the subtree.
    doc.set_page_template(pid(5), true, 130).unwrap();
    doc.trash_page(pid(1), 140).unwrap();
    assert!(doc.pages_in_templates().contains(&pid(2)));
    assert!(
        doc.in_template(pid(4)),
        "its ancestors go with it into the trash"
    );
    doc.restore_page(pid(1), 150).unwrap();
    // A trashed page that was inside a template is looked at where the tree holds it.
    doc.trash_page(pid(4), 160).unwrap();
    assert!(!doc.pages_in_templates().contains(&pid(4)));
    assert!(
        doc.pages_in_templates().contains(&pid(5)),
        "5 carries the flag itself"
    );
    doc.restore_page(pid(4), 170).unwrap();
    assert!(doc.in_template(pid(4)));
    assert_healthy(&doc);
}

#[test]
fn placeholders_are_set_reported_in_schema_order_and_cleared() {
    let mut doc = space();
    doc.set_page_template(pid(1), true, 100).unwrap();

    // Owner is the second property, Due the first: the page reports them in schema order whatever
    // the order they were set in.
    assert_eq!(
        doc.set_template_placeholder(pid(1), prop(2), Some(TemplatePlaceholder::Me), 110)
            .unwrap(),
        vec![pid(1)]
    );
    doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Now), 120)
        .unwrap();
    assert_eq!(
        state(&doc, 1).1,
        vec![
            (prop(1), TemplatePlaceholder::Now),
            (prop(2), TemplatePlaceholder::Me)
        ]
    );
    assert_eq!(doc.page(pid(1)).unwrap().updated_at, 120);
    assert!(meta_keys(&doc, 1).contains(&"template_values".to_owned()));

    // A page inside the template holds placeholders too.
    doc.set_template_placeholder(pid(2), prop(1), Some(TemplatePlaceholder::Today), 130)
        .unwrap();
    assert_eq!(
        state(&doc, 2),
        (false, vec![(prop(1), TemplatePlaceholder::Today)])
    );

    // Changing one replaces it; the same again writes nothing.
    doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Today), 140)
        .unwrap();
    assert_eq!(state(&doc, 1).1[0], (prop(1), TemplatePlaceholder::Today));
    let version = doc.doc().version();
    assert!(
        doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Today), 150)
            .unwrap()
            .is_empty()
    );
    assert_eq!(doc.doc().version(), version);

    // Clearing removes the entry, and the key with the last one; clearing what is not there is
    // nothing.
    assert_eq!(
        doc.set_template_placeholder(pid(1), prop(1), None, 160)
            .unwrap(),
        vec![pid(1)]
    );
    assert!(
        doc.set_template_placeholder(pid(1), prop(1), None, 161)
            .unwrap()
            .is_empty()
    );
    doc.set_template_placeholder(pid(1), prop(2), None, 170)
        .unwrap();
    assert_eq!(state(&doc, 1), (true, Vec::new()));
    assert!(!meta_keys(&doc, 1).contains(&"template_values".to_owned()));
    assert_healthy(&doc);
}

#[test]
fn a_placeholder_is_refused_outside_a_template_and_for_a_kind_that_does_not_take_it() {
    let mut doc = space();
    // Not a template and not inside one.
    assert!(matches!(
        doc.set_template_placeholder(pid(4), prop(1), Some(TemplatePlaceholder::Today), 10),
        Err(ModelError::InvalidProperty {
            field: "pageId",
            ..
        })
    ));
    doc.set_page_template(pid(1), true, 11).unwrap();
    // Kinds: `today` and `now` need a date, `me` a person; text takes none.
    for (property, placeholder) in [
        (prop(2), TemplatePlaceholder::Today),
        (prop(2), TemplatePlaceholder::Now),
        (prop(1), TemplatePlaceholder::Me),
        (prop(3), TemplatePlaceholder::Today),
        (prop(3), TemplatePlaceholder::Me),
    ] {
        assert!(
            matches!(
                doc.set_template_placeholder(pid(1), property, Some(placeholder), 12),
                Err(ModelError::InvalidProperty {
                    field: "placeholder",
                    ..
                })
            ),
            "{placeholder:?} on {property}"
        );
    }
    assert!(placeholder_fits(
        TemplatePlaceholder::Now,
        PropertyKind::Date
    ));
    assert!(placeholder_fits(
        TemplatePlaceholder::Me,
        PropertyKind::Person
    ));
    assert!(!placeholder_fits(
        TemplatePlaceholder::Me,
        PropertyKind::Text
    ));
    // Unknown page, unknown property.
    assert!(matches!(
        doc.set_template_placeholder(pid(99), prop(1), Some(TemplatePlaceholder::Today), 13),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        doc.set_template_placeholder(pid(1), prop(99), Some(TemplatePlaceholder::Today), 14),
        Err(ModelError::PropertyNotFound)
    ));
    assert!(matches!(
        doc.set_template_placeholder(pid(1), prop(99), None, 15),
        Err(ModelError::PropertyNotFound)
    ));
    // A page in the trash.
    doc.trash_page(pid(1), 16).unwrap();
    assert!(matches!(
        doc.set_template_placeholder(pid(2), prop(1), Some(TemplatePlaceholder::Today), 17),
        Err(ModelError::PageTrashed)
    ));
    // Nothing was written by any refusal.
    doc.restore_page(pid(1), 18).unwrap();
    assert_eq!(state(&doc, 1), (true, Vec::new()));
    assert!(!meta_keys(&doc, 1).contains(&"template_values".to_owned()));
    assert_healthy(&doc);
}

#[test]
fn a_placeholder_replaces_the_stored_value_and_a_value_write_removes_the_placeholder() {
    let mut doc = space();
    doc.set_page_template(pid(1), true, 100).unwrap();
    doc.set_page_property(pid(1), prop(1), Some(day("2026-10-18")), 110)
        .unwrap();
    assert_eq!(due_of(&doc, 1), Some(day("2026-10-18")));

    // Setting a placeholder removes the value in the same commit.
    let version = doc.doc().version();
    doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Today), 120)
        .unwrap();
    assert_eq!(due_of(&doc, 1), None);
    assert_eq!(
        state(&doc, 1).1,
        vec![(prop(1), TemplatePlaceholder::Today)]
    );
    // One commit: the value's removal and the placeholder are in the same update.
    let after = doc.doc().version();
    assert_ne!(after, version);

    // Clearing the placeholder leaves the (absent) value as it is.
    doc.set_template_placeholder(pid(1), prop(1), None, 130)
        .unwrap();
    assert_eq!(due_of(&doc, 1), None);

    // A value write removes the placeholder of the property, and only that one.
    doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Now), 140)
        .unwrap();
    doc.set_template_placeholder(pid(1), prop(2), Some(TemplatePlaceholder::Me), 141)
        .unwrap();
    doc.set_page_property(pid(1), prop(1), Some(day("2026-11-02")), 150)
        .unwrap();
    assert_eq!(due_of(&doc, 1), Some(day("2026-11-02")));
    assert_eq!(state(&doc, 1).1, vec![(prop(2), TemplatePlaceholder::Me)]);
    // A value of another property leaves the placeholders alone.
    doc.set_page_property(pid(1), prop(3), Some(PropertyValue::Text("x".into())), 151)
        .unwrap();
    assert_eq!(state(&doc, 1).1, vec![(prop(2), TemplatePlaceholder::Me)]);

    // Clearing a value that is already empty still removes the placeholder: the page holds a
    // placeholder for the property, so a write of the value is the person taking it back.
    assert!(
        doc.set_page_property(pid(1), prop(2), None, 160)
            .unwrap()
            .contains(&pid(1))
    );
    assert_eq!(state(&doc, 1).1, Vec::new());
    assert!(!meta_keys(&doc, 1).contains(&"template_values".to_owned()));
    // And a write that changes nothing and has no placeholder to remove is still nothing.
    assert!(
        doc.set_page_property(pid(1), prop(2), None, 161)
            .unwrap()
            .is_empty()
    );
    assert_healthy(&doc);
}

#[test]
fn placeholders_follow_the_template_the_schema_and_the_kind() {
    let mut doc = space();
    doc.set_page_template(pid(1), true, 100).unwrap();
    doc.set_template_placeholder(pid(2), prop(1), Some(TemplatePlaceholder::Today), 110)
        .unwrap();
    assert_eq!(state(&doc, 2).1.len(), 1);

    // A page that leaves the template keeps its map and reports none; it is back with the template.
    doc.move_page(pid(2), None, 0, 120).unwrap();
    assert_eq!(state(&doc, 2).1, Vec::new());
    assert!(meta_keys(&doc, 2).contains(&"template_values".to_owned()));
    doc.move_page(pid(2), Some(pid(1)), 0, 130).unwrap();
    assert_eq!(state(&doc, 2).1.len(), 1);
    // The same when the flag goes off and on.
    doc.set_page_template(pid(1), false, 140).unwrap();
    assert_eq!(state(&doc, 2).1, Vec::new());
    doc.set_page_template(pid(1), true, 150).unwrap();
    assert_eq!(state(&doc, 2).1.len(), 1);

    // A deleted property is not reported, and goes with the next write of the map.
    doc.set_template_placeholder(pid(2), prop(2), Some(TemplatePlaceholder::Me), 160)
        .unwrap();
    doc.delete_property(prop(2)).unwrap();
    assert_eq!(
        state(&doc, 2).1,
        vec![(prop(1), TemplatePlaceholder::Today)]
    );
    doc.set_template_placeholder(pid(2), prop(1), Some(TemplatePlaceholder::Now), 170)
        .unwrap();
    let node = doc.index.node(pid(2)).unwrap();
    assert_eq!(
        crate::templates::read_values(&doc.meta_of(node).unwrap()),
        vec![(prop(1), TemplatePlaceholder::Now)],
        "the entry of the deleted property went with the write"
    );
    assert_healthy(&doc);
}

#[test]
fn a_foreign_or_damaged_map_reads_as_far_as_it_can() {
    let mut doc = space();
    doc.set_page_template(pid(1), true, 100).unwrap();
    let node = doc.index.node(pid(1)).unwrap();
    let meta = doc.meta_of(node).unwrap();
    // An entry that is not a property id, one whose placeholder this version does not know and one
    // that is not text are left out; the rest is read.
    let map: std::collections::HashMap<String, loro::LoroValue> = [
        ("not-an-id".to_owned(), loro::LoroValue::from("today")),
        (prop(1).to_string(), loro::LoroValue::from("now")),
        (prop(2).to_string(), loro::LoroValue::from("everyone")),
        (prop(3).to_string(), loro::LoroValue::from(7_i64)),
    ]
    .into_iter()
    .collect();
    meta.insert("template_values", loro::LoroValue::from(map))
        .unwrap();
    assert_eq!(state(&doc, 1).1, vec![(prop(1), TemplatePlaceholder::Now)]);
    // A flag that is not the boolean `true` reads as off.
    meta.insert("template", "yes").unwrap();
    assert!(!state(&doc, 1).0);
    assert!(!doc.in_template(pid(1)));
}

#[test]
fn undo_and_redo_take_the_flag_and_a_placeholder_in_one_step_each() {
    let mut doc = space();
    let plain = doc.to_portable();
    doc.set_page_template(pid(1), true, 5_000).unwrap();
    let flagged = doc.to_portable();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(doc.to_portable(), plain);
    assert!(!doc.in_template(pid(2)));
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(doc.to_portable(), flagged);
    assert!(doc.in_template(pid(2)));
    assert_healthy(&doc);

    // A placeholder and the removal of the value it replaces are one step.
    doc.set_page_property(pid(1), prop(1), Some(day("2026-10-18")), 6_000)
        .unwrap();
    doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Today), 7_000)
        .unwrap();
    assert_eq!(due_of(&doc, 1), None);
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(state(&doc, 1).1, Vec::new());
    assert_eq!(
        due_of(&doc, 1),
        Some(day("2026-10-18")),
        "the undo of the placeholder brings the value back"
    );
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(due_of(&doc, 1), None);
    assert_eq!(
        state(&doc, 1).1,
        vec![(prop(1), TemplatePlaceholder::Today)]
    );

    // The flag and a placeholder of the same page pressed in quick succession are one step; a later
    // change is another.
    let mut doc = space();
    doc.set_page_template(pid(1), true, 10_000).unwrap();
    doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Now), 10_100)
        .unwrap();
    doc.set_template_placeholder(pid(1), prop(2), Some(TemplatePlaceholder::Me), 20_000)
        .unwrap();
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(
        state(&doc, 1),
        (true, vec![(prop(1), TemplatePlaceholder::Now)])
    );
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(state(&doc, 1), (false, Vec::new()));
    assert_healthy(&doc);
}

#[test]
fn the_portable_form_lists_the_flag_and_the_map_only_on_pages_that_have_them() {
    let mut doc = space();
    let before = doc.to_portable().to_string();
    assert!(
        !before.contains("template"),
        "additive: no key without a value"
    );

    doc.set_page_template(pid(1), true, 10).unwrap();
    doc.set_template_placeholder(pid(2), prop(1), Some(TemplatePlaceholder::Today), 11)
        .unwrap();
    doc.set_template_placeholder(pid(2), prop(2), Some(TemplatePlaceholder::Me), 12)
        .unwrap();
    let portable = doc.to_portable();
    let top = &portable["pages"][0];
    assert_eq!(top["template"], json!(true));
    assert!(top.get("templateValues").is_none());
    let child = &top["children"][0];
    assert!(child.get("template").is_none());
    assert_eq!(
        child["templateValues"],
        json!({ prop(1).to_string(): "today", prop(2).to_string(): "me" })
    );
    // The keys stay in sorted order (the text is the same whichever way serde_json keeps them).
    let keys: Vec<&str> = child
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);

    // A trashed page lists them too, and so does the map of a page that is not in a template.
    doc.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Now), 13)
        .unwrap();
    doc.set_page_template(pid(1), false, 14).unwrap();
    doc.trash_page(pid(1), 15).unwrap();
    let trash = &doc.to_portable()["trash"][0];
    assert_eq!(
        trash["templateValues"],
        json!({ prop(1).to_string(): "now" })
    );
    assert!(trash.get("template").is_none());
    assert_healthy(&doc);
}

/// A new structure document of space `sid(space)` with peer `peer`.
fn structure_in(peer: u64, space: u8) -> StructureDoc {
    StructureDoc::new(
        Doc::new(DocKind::Structure, peer),
        SpaceMeta {
            id: sid(space),
            name: format!("Space {space}"),
            icon: None,
            created_at: 5,
        },
    )
}

fn moved(doc: &StructureDoc, page: u8) -> Vec<MovedPage> {
    doc.moved_pages(pid(page)).unwrap()
}

#[test]
fn the_flag_and_the_map_travel_with_a_move_between_spaces() {
    let mut a = space();
    a.set_page_template(pid(1), true, 100).unwrap();
    a.set_template_placeholder(pid(1), prop(1), Some(TemplatePlaceholder::Today), 110)
        .unwrap();
    a.set_template_placeholder(pid(2), prop(2), Some(TemplatePlaceholder::Me), 120)
        .unwrap();
    // The pages as the move reads them: the flag and the stored map, by the source's property ids.
    let pages = moved(&a, 1);
    assert_eq!(pages.len(), 3);
    assert!(pages[0].template && !pages[1].template);
    assert_eq!(
        pages[0].template_values,
        vec![(prop(1), TemplatePlaceholder::Today)]
    );
    assert_eq!(
        pages[1].template_values,
        vec![(prop(2), TemplatePlaceholder::Me)]
    );
    assert!(pages[2].template_values.is_empty());

    // The target holds the same property ids here (the core maps them for another schema).
    let mut b = structure_in(2, 2);
    for (n, name, kind) in [
        (1, "Due", PropertyKind::Date),
        (2, "Owner", PropertyKind::Person),
    ] {
        create(&mut b, n, name, kind);
    }
    let out = a.move_out(pid(1), sid(2), 200).unwrap();
    assert_eq!(out, pages);
    // The residue keeps nothing of it.
    assert!(a.page(pid(1)).is_none());
    assert!(!a.to_portable().to_string().contains("template"));
    b.move_in(&out, None, None, sid(1), 210).unwrap();
    assert_eq!(
        state(&b, 1),
        (true, vec![(prop(1), TemplatePlaceholder::Today)])
    );
    assert_eq!(
        state(&b, 2),
        (false, vec![(prop(2), TemplatePlaceholder::Me)])
    );
    assert!(b.in_template(pid(3)));
    assert_healthy(&a);
    assert_healthy(&b);

    // A page that carries neither gets no key.
    let mut c = structure_in(3, 3);
    let plain = moved(&space(), 4);
    c.move_in(&plain, None, None, sid(1), 5).unwrap();
    let keys = meta_keys(&c, 4);
    assert!(!keys.contains(&"template".to_owned()));
    assert!(!keys.contains(&"template_values".to_owned()));
    assert_healthy(&c);
}

#[test]
fn two_devices_converge_on_the_flag_and_the_map() {
    let mut a = space();
    let mut b = replica(&a, 2);
    a.set_page_template(pid(1), true, 100).unwrap();
    b.set_page_template(pid(4), true, 101).unwrap();
    b.set_template_placeholder(pid(4), prop(1), Some(TemplatePlaceholder::Today), 102)
        .unwrap();
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert!(doc.in_template(pid(1)) && doc.in_template(pid(2)) && doc.in_template(pid(4)));
        assert_eq!(
            state(doc, 4),
            (true, vec![(prop(1), TemplatePlaceholder::Today)])
        );
    }
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(a.pages_in_templates(), b.pages_in_templates());

    // The map is one value: two devices that set different placeholders of one page at the same
    // time keep one of the maps whole (last writer wins), never a broken one.
    a.set_template_placeholder(pid(4), prop(2), Some(TemplatePlaceholder::Me), 200)
        .unwrap();
    b.set_template_placeholder(pid(4), prop(1), Some(TemplatePlaceholder::Now), 201)
        .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(state(&a, 4), state(&b, 4));
    assert!(!state(&a, 4).1.is_empty());
    assert_healthy(&a);
    assert_healthy(&b);

    // A device that removes the flag while another adds a sub-page: the sub-page is not in a
    // template once the flag is gone, on both.
    a.set_page_template(pid(1), false, 300).unwrap();
    add(&mut b, 8, Some(1), "Added under it");
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert!(!doc.in_template(pid(8)));
    }
    assert_healthy(&a);
    assert_healthy(&b);
}
