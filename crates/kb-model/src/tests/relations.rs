//! Relations (one- and two-way) and rollups (ADR-0027): configuration, values, the symmetric view
//! of a two-way relation, convergence under merging, and the arithmetic over the related pages.

use kb_types::properties::MAX_RELATED_PAGES;
use kb_types::{
    DateValue, OptionId, PageId, PropertyConfig, PropertyDef, PropertyId, PropertyKind,
    PropertyValue, RollupFunction, RollupResult, RollupTarget, RollupValue,
};

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{ModelError, StructureDoc};

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xb7, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn spare() -> impl FnMut() -> OptionId {
    || OptionId::from_random_bytes([1; 16])
}

fn create(doc: &mut StructureDoc, n: u8, name: &str, kind: PropertyKind) -> PropertyDef {
    doc.create_property(prop(n), name, kind, &mut spare(), 10)
        .unwrap()
}

/// A two-way relation: `prop(a)` forward, `prop(b)` back.
fn pair(doc: &mut StructureDoc, a: u8, b: u8) -> PropertyDef {
    doc.create_relation_pair(prop(a), prop(b), "Related", "Related (back)", 10)
        .unwrap()
}

fn rollup(
    doc: &mut StructureDoc,
    n: u8,
    relation: u8,
    target: RollupTarget,
    function: RollupFunction,
) -> Result<PropertyDef, ModelError> {
    doc.create_property_with(
        prop(n),
        "Rollup",
        PropertyKind::Rollup,
        Some(&PropertyConfig::Rollup {
            relation_property_id: prop(relation),
            target,
            function,
        }),
        &mut spare(),
        10,
    )
}

fn relation(pages: &[u8]) -> PropertyValue {
    PropertyValue::Relation(pages.iter().map(|n| pid(*n)).collect())
}

fn set(doc: &mut StructureDoc, page: u8, property: u8, value: PropertyValue) -> Vec<PageId> {
    doc.set_page_property(pid(page), prop(property), Some(value), 100)
        .unwrap()
}

fn clear(doc: &mut StructureDoc, page: u8, property: u8) {
    doc.set_page_property(pid(page), prop(property), None, 100)
        .unwrap();
}

fn value(doc: &StructureDoc, page: u8, property: u8) -> Option<PropertyValue> {
    doc.page_properties(pid(page))
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == prop(property))
        .map(|(_, value)| value)
}

fn related(doc: &StructureDoc, page: u8, property: u8) -> Vec<u8> {
    match value(doc, page, property) {
        Some(PropertyValue::Relation(pages)) => {
            pages.iter().map(|page| page.to_bytes()[0]).collect()
        }
        None => Vec::new(),
        other => panic!("not a relation: {other:?}"),
    }
}

fn refusal(result: Result<impl std::fmt::Debug, ModelError>) -> (&'static str, String) {
    match result {
        Err(ModelError::InvalidProperty { field, message }) => (field, message),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The findings about rollups (values of a deleted property are reported too, by design).
fn rollup_problems(doc: &StructureDoc) -> Vec<String> {
    doc.check_invariants()
        .into_iter()
        .filter(|problem| problem.starts_with("rollup"))
        .collect()
}

fn number(n: f64) -> PropertyValue {
    PropertyValue::Number(n)
}

/// Pages 1..=n under the root, titled "Page <n>".
fn pages(doc: &mut StructureDoc, count: u8) {
    for n in 1..=count {
        add(doc, n, None, &format!("Page {n}"));
    }
}

/// The invariants check without the findings about values of a deleted property, which stay in the
/// documents on purpose (ADR-0026).
fn assert_healthy_but_for_deleted(doc: &StructureDoc) {
    let problems: Vec<String> = doc
        .check_invariants()
        .into_iter()
        .filter(|problem| !problem.contains("unknown property"))
        .collect();
    assert!(problems.is_empty(), "invariant violations: {problems:#?}");
}

/// Every page's view is symmetric: Y is in the forward value of X exactly when X is in the back
/// value of Y.
fn assert_symmetric(doc: &StructureDoc, pages: u8, forward: u8, back: u8) {
    for x in 1..=pages {
        for y in 1..=pages {
            assert_eq!(
                related(doc, x, forward).contains(&y),
                related(doc, y, back).contains(&x),
                "pages {x} and {y}"
            );
        }
    }
}

// ----- the relation kind -------------------------------------------------------------------------

#[test]
fn a_one_way_relation_stores_an_ordered_list_of_pages_and_clears_with_an_empty_one() {
    let mut doc = structure(1);
    pages(&mut doc, 4);
    let def = create(&mut doc, 1, "Links", PropertyKind::Relation);
    assert_eq!(
        def.config,
        Some(PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None
        })
    );
    assert_eq!(set(&mut doc, 1, 1, relation(&[3, 2])), [pid(1)]);
    assert_eq!(related(&doc, 1, 1), [3, 2]);
    assert_eq!(related(&replica(&doc, 2), 1, 1), [3, 2]);
    // Nothing else changes: a one-way relation has no other side.
    assert_eq!(related(&doc, 3, 1), Vec::<u8>::new());
    // The same value again writes nothing.
    assert!(set(&mut doc, 1, 1, relation(&[3, 2])).is_empty());
    assert_eq!(set(&mut doc, 1, 1, relation(&[2, 3, 4])), [pid(1)]);
    assert_eq!(related(&doc, 1, 1), [2, 3, 4]);
    // An empty list is no value.
    assert_eq!(
        set(&mut doc, 1, 1, PropertyValue::Relation(Vec::new())),
        [pid(1)]
    );
    assert_eq!(value(&doc, 1, 1), None);
    assert_healthy(&doc);
}

#[test]
fn a_relation_value_is_checked_but_a_page_that_does_not_exist_is_kept_unread() {
    let mut doc = structure(1);
    pages(&mut doc, 2);
    create(&mut doc, 1, "Links", PropertyKind::Relation);
    let (field, message) = refusal(doc.set_page_property(
        pid(1),
        prop(1),
        Some(PropertyValue::Relation(vec![pid(2), pid(2)])),
        100,
    ));
    assert_eq!(
        (field, message.as_str()),
        ("value", "page ids must be distinct")
    );
    let many = PropertyValue::Relation(
        (0..=u8::try_from(MAX_RELATED_PAGES).unwrap())
            .map(|n| PageId::from_random_bytes([0xee, n, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n]))
            .collect(),
    );
    let (_, message) = refusal(doc.set_page_property(pid(1), prop(1), Some(many), 100));
    assert_eq!(message, "must list at most 100 pages");
    let (field, _) =
        refusal(doc.set_page_property(pid(1), prop(1), Some(PropertyValue::Text("x".into())), 100));
    assert_eq!(field, "value");

    // A page the space does not have is kept in the document but reads as nothing.
    set(&mut doc, 1, 1, relation(&[2, 99]));
    assert_eq!(related(&doc, 1, 1), [2]);
    set(&mut doc, 1, 1, relation(&[99]));
    assert_eq!(value(&doc, 1, 1), None);
    assert_healthy(&doc);
}

#[test]
fn a_trashed_page_stays_in_the_relation_and_a_purged_one_leaves_it() {
    let mut doc = structure(1);
    pages(&mut doc, 3);
    create(&mut doc, 1, "Links", PropertyKind::Relation);
    set(&mut doc, 1, 1, relation(&[2, 3]));
    doc.trash_page(pid(2), 200).unwrap();
    assert_eq!(related(&doc, 1, 1), [2, 3], "listed while in the trash");
    doc.restore_page(pid(2), 210).unwrap();
    doc.trash_page(pid(2), 220).unwrap();
    doc.purge_page(pid(2), 230).unwrap();
    assert_eq!(related(&doc, 1, 1), [3], "purged pages read as nothing");
    assert_healthy(&doc);
}

#[test]
fn the_rows_of_a_relation_are_its_pages_and_its_search_text_their_titles() {
    let mut doc = structure(1);
    pages(&mut doc, 3);
    create(&mut doc, 1, "Links", PropertyKind::Relation);
    set(&mut doc, 1, 1, relation(&[3, 2, 99]));
    let (_, entry) = doc.property_index(&[pid(1)]).remove(0);
    let rows: Vec<(PropertyKind, Option<PageId>)> = entry
        .rows
        .iter()
        .map(|row| (row.kind, row.related_page_id))
        .collect();
    assert_eq!(
        rows,
        [
            (PropertyKind::Relation, Some(pid(3))),
            (PropertyKind::Relation, Some(pid(2)))
        ],
        "one row per related page that exists, in order"
    );
    assert_eq!(entry.texts, [(prop(1), "Page 3, Page 2".to_owned())]);
    assert!(doc.property_index(&[pid(2)])[0].1.rows.is_empty());
}

// ----- two-way relations ---------------------------------------------------------------------------

#[test]
fn creating_a_two_way_relation_makes_two_properties_that_name_each_other() {
    let mut doc = structure(1);
    let forward = pair(&mut doc, 1, 2);
    assert_eq!(forward.id, prop(1));
    assert_eq!(forward.name, "Related");
    assert_eq!(
        forward.config,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(prop(2))
        })
    );
    let schema = doc.property_schema();
    let back = schema.iter().find(|def| def.id == prop(2)).unwrap();
    assert_eq!(back.name, "Related (back)");
    assert_eq!(back.kind, PropertyKind::Relation);
    assert_eq!(
        back.config,
        Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(prop(1))
        })
    );
    assert_eq!(schema.len(), 3, "Tags and the two");
    assert_healthy(&doc);
}

#[test]
fn a_two_way_relation_needs_room_for_both_properties() {
    let mut doc = structure(1);
    for n in 1..=98_u8 {
        doc.create_property(
            PropertyId::from_random_bytes([0xa1, n, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n]),
            "P",
            PropertyKind::Text,
            &mut spare(),
            1,
        )
        .unwrap();
    }
    let (field, message) = refusal(doc.create_relation_pair(prop(1), prop(2), "A", "B", 10));
    assert_eq!(field, "spaceId");
    assert!(message.contains("at most 100 properties"), "{message}");
    assert_eq!(doc.property_schema().len(), 99, "nothing was written");
}

#[test]
fn writing_one_side_writes_the_other_in_the_same_commit() {
    let mut doc = structure(1);
    pages(&mut doc, 4);
    pair(&mut doc, 1, 2);
    let changed = set(&mut doc, 1, 1, relation(&[2, 3]));
    assert_eq!(changed, [pid(1), pid(2), pid(3)]);
    assert_eq!(related(&doc, 2, 2), [1]);
    assert_eq!(related(&doc, 3, 2), [1]);
    // Writing the back side adds to the forward side of the other page.
    set(&mut doc, 4, 2, relation(&[1]));
    assert_eq!(related(&doc, 1, 1), [2, 3, 4]);
    // Removing is symmetric too.
    let changed = set(&mut doc, 1, 1, relation(&[3, 4]));
    assert_eq!(changed, [pid(1), pid(2)]);
    assert_eq!(related(&doc, 2, 2), Vec::<u8>::new());
    assert_eq!(related(&doc, 3, 2), [1]);
    assert_eq!(
        related(&doc, 4, 2),
        [1],
        "page 4 still lists 1 on its back side"
    );
    assert_eq!(related(&doc, 1, 1), [3, 4]);
    clear(&mut doc, 1, 1);
    assert_eq!(related(&doc, 3, 2), Vec::<u8>::new());
    assert_eq!(related(&doc, 4, 2), Vec::<u8>::new());
    assert_symmetric(&doc, 4, 1, 2);
    assert_healthy(&doc);
}

#[test]
fn a_page_may_relate_to_itself_in_a_two_way_relation() {
    let mut doc = structure(1);
    pages(&mut doc, 2);
    pair(&mut doc, 1, 2);
    set(&mut doc, 1, 1, relation(&[1, 2]));
    assert_eq!(related(&doc, 1, 1), [1, 2]);
    assert_eq!(related(&doc, 1, 2), [1]);
    set(&mut doc, 1, 1, relation(&[2]));
    assert_eq!(related(&doc, 1, 2), Vec::<u8>::new());
    assert_symmetric(&doc, 2, 1, 2);
}

#[test]
fn the_back_side_is_complete_even_when_only_one_side_was_stored() {
    // Two replicas add the same kind of link from opposite sides while apart; the stored lists
    // are last-writer-wins, so one write may be lost, but both views stay symmetric and equal.
    let mut a = structure(1);
    pages(&mut a, 3);
    pair(&mut a, 1, 2);
    let mut b = replica(&a, 2);
    set(&mut a, 1, 1, relation(&[2]));
    set(&mut b, 3, 2, relation(&[1]));
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        let mut forward = related(doc, 1, 1);
        forward.sort_unstable();
        assert_eq!(forward, [2, 3]);
        assert_eq!(related(doc, 2, 2), [1]);
        assert_eq!(related(doc, 3, 2), [1]);
        assert_symmetric(doc, 3, 1, 2);
        assert_healthy(doc);
    }
}

#[test]
fn an_add_wins_over_a_concurrent_remove_of_the_same_link() {
    let mut a = structure(1);
    pages(&mut a, 3);
    pair(&mut a, 1, 2);
    set(&mut a, 1, 1, relation(&[2]));
    let mut b = replica(&a, 2);
    // Device a removes the link 1 - 2; device b adds 1 - 3 from page 1's side.
    clear(&mut a, 1, 1);
    set(&mut b, 1, 1, relation(&[2, 3]));
    sync(&mut a, &mut b);
    // Whatever the last-writer-wins pick for page 1's list, both replicas agree and are symmetric.
    assert_eq!(related(&a, 1, 1), related(&b, 1, 1));
    assert_eq!(related(&a, 2, 2), related(&b, 2, 2));
    assert_eq!(related(&a, 3, 2), related(&b, 3, 2));
    assert_symmetric(&a, 3, 1, 2);
    assert_symmetric(&b, 3, 1, 2);
    assert!(related(&a, 1, 1).contains(&3), "the added link survives");
}

#[test]
fn a_remove_from_one_side_and_a_remove_from_the_other_converge_on_no_link() {
    let mut a = structure(1);
    pages(&mut a, 2);
    pair(&mut a, 1, 2);
    set(&mut a, 1, 1, relation(&[2]));
    let mut b = replica(&a, 2);
    clear(&mut a, 1, 1);
    clear(&mut b, 2, 2);
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert_eq!(related(doc, 1, 1), Vec::<u8>::new());
        assert_eq!(related(doc, 2, 2), Vec::<u8>::new());
    }
}

#[test]
fn a_page_trashed_or_purged_on_one_device_leaves_the_other_view_consistent() {
    let mut a = structure(1);
    pages(&mut a, 3);
    pair(&mut a, 1, 2);
    set(&mut a, 1, 1, relation(&[2, 3]));
    let mut b = replica(&a, 2);
    a.trash_page(pid(2), 200).unwrap();
    set(&mut b, 1, 1, relation(&[2, 3]));
    clear(&mut b, 3, 2);
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert!(
            related(doc, 1, 1).contains(&2),
            "a trashed page stays listed"
        );
        assert_symmetric(doc, 3, 1, 2);
    }
    a.purge_page(pid(2), 300).unwrap();
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert!(
            !related(doc, 1, 1).contains(&2),
            "a purged page reads as nothing"
        );
        assert_healthy(doc);
    }
}

#[test]
fn deleting_one_property_of_a_pair_turns_the_other_into_a_one_way_relation_that_keeps_its_links() {
    let mut doc = structure(1);
    pages(&mut doc, 3);
    pair(&mut doc, 1, 2);
    set(&mut doc, 1, 1, relation(&[2, 3]));
    doc.delete_property(prop(2)).unwrap();
    let forward = doc.property(prop(1)).unwrap();
    assert_eq!(
        forward.config,
        Some(PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None
        })
    );
    assert_eq!(related(&doc, 1, 1), [2, 3], "the links stay");
    // A write no longer touches the other pages.
    assert_eq!(set(&mut doc, 1, 1, relation(&[3])), [pid(1)]);
    assert_healthy_but_for_deleted(&doc);

    // The same from the other side: deleting the forward property keeps the back one's links.
    let mut doc = structure(1);
    pages(&mut doc, 3);
    pair(&mut doc, 1, 2);
    set(&mut doc, 1, 1, relation(&[2, 3]));
    doc.delete_property(prop(1)).unwrap();
    let back = doc.property(prop(2)).unwrap();
    assert_eq!(
        back.config,
        Some(PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None
        })
    );
    assert_eq!(
        related(&doc, 2, 2),
        [1],
        "the stored back side still lists the page"
    );
    assert_healthy_but_for_deleted(&doc);
}

#[test]
fn a_pair_whose_partner_was_deleted_by_another_device_reads_as_one_way() {
    let mut a = structure(1);
    pages(&mut a, 2);
    pair(&mut a, 1, 2);
    let mut b = replica(&a, 2);
    a.delete_property(prop(2)).unwrap();
    // Device b writes the forward side while the partner is still there.
    set(&mut b, 1, 1, relation(&[2]));
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert_eq!(
            doc.property(prop(1)).unwrap().config,
            Some(PropertyConfig::Relation {
                two_way: false,
                paired_property_id: None
            })
        );
        assert_eq!(related(doc, 1, 1), [2]);
        assert_healthy_but_for_deleted(doc);
    }
}

#[test]
fn a_relation_configuration_can_only_be_given_at_creation() {
    let mut doc = structure(1);
    create(&mut doc, 1, "Links", PropertyKind::Relation);
    let (field, message) = refusal(doc.set_property_config(
        prop(1),
        &PropertyConfig::Relation {
            two_way: true,
            paired_property_id: None,
        },
    ));
    assert_eq!(field, "config");
    assert_eq!(message, "the configuration of a relation cannot be changed");
    let (field, message) = refusal(doc.create_property_with(
        prop(2),
        "T",
        PropertyKind::Text,
        Some(&PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None,
        }),
        &mut spare(),
        10,
    ));
    assert_eq!(field, "config");
    assert_eq!(
        message,
        "only a relation, a rollup or a formula takes a configuration"
    );
    // A paired id cannot be supplied: the core makes the paired property.
    let (field, message) = refusal(doc.create_property_with(
        prop(3),
        "R",
        PropertyKind::Relation,
        Some(&PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(prop(1)),
        }),
        &mut spare(),
        10,
    ));
    assert_eq!(field, "config");
    assert_eq!(
        message,
        "a new two-way relation makes its own paired property"
    );
}

// ----- rollups -----------------------------------------------------------------------------------

/// Page 1 relates to pages 2, 3, 4 through `prop(1)`; they carry a number (`prop(2)`), a date
/// (`prop(3)`), a checkbox (`prop(4)`).
fn board() -> StructureDoc {
    let mut doc = structure(1);
    pages(&mut doc, 5);
    create(&mut doc, 1, "Links", PropertyKind::Relation);
    create(&mut doc, 2, "Cost", PropertyKind::Number);
    create(&mut doc, 3, "Due", PropertyKind::Date);
    create(&mut doc, 4, "Done", PropertyKind::Checkbox);
    set(&mut doc, 1, 1, relation(&[2, 3, 4]));
    set(&mut doc, 2, 2, number(10.0));
    set(&mut doc, 3, 2, number(5.0));
    set(
        &mut doc,
        2,
        3,
        PropertyValue::Date(DateValue {
            date: "2026-10-09".into(),
            time: None,
        }),
    );
    set(
        &mut doc,
        3,
        3,
        PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("09:00".into()),
        }),
    );
    set(&mut doc, 2, 4, PropertyValue::Checkbox(true));
    set(&mut doc, 3, 4, PropertyValue::Checkbox(false));
    doc
}

fn rolled(doc: &StructureDoc, page: u8, property: u8) -> Option<RollupResult> {
    match value(doc, page, property) {
        Some(PropertyValue::Rollup(RollupValue { function, result })) => {
            let configured = match doc.property(prop(property)).and_then(|def| def.config) {
                Some(PropertyConfig::Rollup { function, .. }) => function,
                other => panic!("not a rollup: {other:?}"),
            };
            assert_eq!(function, configured, "the value names its function");
            Some(result)
        }
        None => None,
        other => panic!("not a rollup: {other:?}"),
    }
}

#[test]
fn a_rollup_summarises_a_property_of_the_related_pages() {
    let mut doc = board();
    let target = |n: u8| RollupTarget::Property(prop(n));
    rollup(&mut doc, 10, 1, target(2), RollupFunction::Sum).unwrap();
    rollup(&mut doc, 11, 1, target(2), RollupFunction::Average).unwrap();
    rollup(&mut doc, 12, 1, target(3), RollupFunction::Earliest).unwrap();
    rollup(&mut doc, 13, 1, target(4), RollupFunction::PercentChecked).unwrap();
    rollup(
        &mut doc,
        14,
        1,
        RollupTarget::Title,
        RollupFunction::CountAll,
    )
    .unwrap();
    rollup(
        &mut doc,
        15,
        1,
        RollupTarget::Title,
        RollupFunction::ShowOriginal,
    )
    .unwrap();
    assert_eq!(rolled(&doc, 1, 10), Some(RollupResult::Number(15.0)));
    assert_eq!(rolled(&doc, 1, 11), Some(RollupResult::Number(7.5)));
    assert_eq!(
        rolled(&doc, 1, 12),
        Some(RollupResult::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("09:00".into())
        }))
    );
    assert_eq!(rolled(&doc, 1, 13), Some(RollupResult::Number(100.0 / 3.0)));
    assert_eq!(rolled(&doc, 1, 14), Some(RollupResult::Number(3.0)));
    assert_eq!(
        rolled(&doc, 1, 15),
        Some(RollupResult::Values(vec![
            PropertyValue::Text("Page 2".into()),
            PropertyValue::Text("Page 3".into()),
            PropertyValue::Text("Page 4".into())
        ]))
    );
    // A page without relations: counts are zero, an average has no result.
    assert_eq!(rolled(&doc, 5, 14), Some(RollupResult::Number(0.0)));
    assert_eq!(rolled(&doc, 5, 11), None);
    // The rollup is in PageInfo.properties like the others, and nothing is stored.
    let info = doc.page(pid(1)).unwrap();
    assert!(
        info.properties
            .iter()
            .any(|(id, v)| *id == prop(10) && v.kind() == PropertyKind::Rollup)
    );
    assert!(
        doc.property_index(&[pid(1)])[0]
            .1
            .rows
            .iter()
            .all(|row| row.kind == PropertyKind::Relation)
    );
    assert_healthy(&doc);
}

#[test]
fn a_rollup_follows_changes_of_the_related_pages_and_ignores_the_trashed() {
    let mut doc = board();
    rollup(
        &mut doc,
        10,
        1,
        RollupTarget::Property(prop(2)),
        RollupFunction::Sum,
    )
    .unwrap();
    set(&mut doc, 4, 2, number(100.0));
    assert_eq!(rolled(&doc, 1, 10), Some(RollupResult::Number(115.0)));
    doc.trash_page(pid(4), 300).unwrap();
    assert_eq!(
        rolled(&doc, 1, 10),
        Some(RollupResult::Number(15.0)),
        "trashed pages do not count"
    );
    doc.restore_page(pid(4), 310).unwrap();
    assert_eq!(rolled(&doc, 1, 10), Some(RollupResult::Number(115.0)));
    set(&mut doc, 1, 1, relation(&[2]));
    assert_eq!(rolled(&doc, 1, 10), Some(RollupResult::Number(10.0)));
}

#[test]
fn a_rollup_through_a_two_way_relation_sees_the_back_links() {
    let mut doc = structure(1);
    pages(&mut doc, 3);
    pair(&mut doc, 1, 2);
    create(&mut doc, 3, "Cost", PropertyKind::Number);
    rollup(
        &mut doc,
        4,
        2,
        RollupTarget::Property(prop(3)),
        RollupFunction::Sum,
    )
    .unwrap();
    set(&mut doc, 2, 3, number(4.0));
    set(&mut doc, 3, 3, number(6.0));
    // Pages 2 and 3 both list page 1 (on their forward side), so page 1 sees them on its back side.
    set(&mut doc, 2, 1, relation(&[1]));
    set(&mut doc, 3, 1, relation(&[1]));
    assert_eq!(related(&doc, 1, 2), [2, 3]);
    assert_eq!(rolled(&doc, 1, 4), Some(RollupResult::Number(10.0)));
}

#[test]
fn a_rollup_configuration_is_checked_when_it_is_written() {
    let mut doc = board();
    let target = |n: u8| RollupTarget::Property(prop(n));
    let check = |doc: &mut StructureDoc, relation: u8, target: RollupTarget, function| {
        refusal(rollup(doc, 20, relation, target, function))
    };
    // The relation must be a relation property of the space.
    assert_eq!(
        check(&mut doc, 2, target(2), RollupFunction::Sum),
        ("config", "the rollup needs a relation property".to_owned())
    );
    assert_eq!(
        check(&mut doc, 99, target(2), RollupFunction::Sum),
        ("config", "the rollup needs a relation property".to_owned())
    );
    // The target must exist and be a kind a rollup can summarise.
    assert_eq!(
        check(&mut doc, 1, target(99), RollupFunction::CountAll),
        ("config", "the rollup target does not exist".to_owned())
    );
    create(&mut doc, 30, "Files", PropertyKind::Files);
    assert_eq!(
        check(&mut doc, 1, target(30), RollupFunction::CountAll),
        (
            "config",
            "this kind of property cannot be rolled up".to_owned()
        )
    );
    // The function must fit the target.
    assert_eq!(
        check(&mut doc, 1, target(3), RollupFunction::Sum),
        (
            "config",
            "this function does not apply to the target".to_owned()
        )
    );
    assert_eq!(
        check(&mut doc, 1, RollupTarget::Title, RollupFunction::Average),
        (
            "config",
            "this function does not apply to the target".to_owned()
        )
    );
    assert_eq!(
        check(&mut doc, 1, target(2), RollupFunction::Earliest),
        (
            "config",
            "this function does not apply to the target".to_owned()
        )
    );
    assert_eq!(
        check(&mut doc, 1, target(2), RollupFunction::PercentChecked),
        (
            "config",
            "this function does not apply to the target".to_owned()
        )
    );
    // A rollup without a configuration, and a value cannot be set.
    let (field, message) =
        refusal(doc.create_property(prop(21), "R", PropertyKind::Rollup, &mut spare(), 10));
    assert_eq!(
        (field, message.as_str()),
        ("config", "a rollup needs a configuration")
    );
    rollup(&mut doc, 22, 1, target(2), RollupFunction::Sum).unwrap();
    let (field, _) = refusal(doc.set_page_property(pid(1), prop(22), None, 100));
    assert_eq!(field, "propertyId");
    assert_eq!(
        doc.property_schema()
            .iter()
            .filter(|d| d.kind == PropertyKind::Rollup)
            .count(),
        1
    );
}

#[test]
fn a_rollup_configuration_can_be_changed() {
    let mut doc = board();
    rollup(
        &mut doc,
        10,
        1,
        RollupTarget::Property(prop(2)),
        RollupFunction::Sum,
    )
    .unwrap();
    let def = doc
        .set_property_config(
            prop(10),
            &PropertyConfig::Rollup {
                relation_property_id: prop(1),
                target: RollupTarget::Property(prop(2)),
                function: RollupFunction::Max,
            },
        )
        .unwrap();
    assert_eq!(
        def.config,
        Some(PropertyConfig::Rollup {
            relation_property_id: prop(1),
            target: RollupTarget::Property(prop(2)),
            function: RollupFunction::Max,
        })
    );
    assert_eq!(rolled(&doc, 1, 10), Some(RollupResult::Number(10.0)));
    let (field, _) = refusal(doc.set_property_config(
        prop(10),
        &PropertyConfig::Rollup {
            relation_property_id: prop(1),
            target: RollupTarget::Property(prop(3)),
            function: RollupFunction::Max,
        },
    ));
    assert_eq!(field, "config");
    // Another property's configuration: a text property takes none.
    let (field, _) = refusal(doc.set_property_config(
        prop(2),
        &PropertyConfig::Rollup {
            relation_property_id: prop(1),
            target: RollupTarget::Title,
            function: RollupFunction::CountAll,
        },
    ));
    assert_eq!(field, "config");
}

#[test]
fn a_rollup_whose_configuration_broke_reads_as_nothing_and_is_reported() {
    let mut doc = board();
    rollup(
        &mut doc,
        10,
        1,
        RollupTarget::Property(prop(2)),
        RollupFunction::Sum,
    )
    .unwrap();
    assert_healthy(&doc);
    // The target is deleted (here, or by another device).
    doc.delete_property(prop(2)).unwrap();
    assert_eq!(rolled(&doc, 1, 10), None);
    assert_eq!(
        rollup_problems(&doc),
        [format!(
            "rollup {} has a configuration that is no longer valid",
            prop(10)
        )]
    );
    // The relation is deleted.
    let mut doc = board();
    rollup(
        &mut doc,
        10,
        1,
        RollupTarget::Title,
        RollupFunction::CountAll,
    )
    .unwrap();
    doc.delete_property(prop(1)).unwrap();
    assert_eq!(rolled(&doc, 1, 10), None);
    assert_eq!(rollup_problems(&doc).len(), 1);
}

#[test]
fn rollups_and_two_way_relations_converge_after_concurrent_edits() {
    let mut a = structure(1);
    pages(&mut a, 4);
    pair(&mut a, 1, 2);
    create(&mut a, 3, "Cost", PropertyKind::Number);
    rollup(
        &mut a,
        4,
        1,
        RollupTarget::Property(prop(3)),
        RollupFunction::Sum,
    )
    .unwrap();
    let mut b = replica(&a, 2);
    set(&mut a, 1, 1, relation(&[2]));
    set(&mut a, 2, 3, number(1.0));
    set(&mut b, 3, 2, relation(&[1]));
    set(&mut b, 3, 3, number(2.0));
    set(&mut b, 4, 3, number(4.0));
    set(&mut b, 1, 1, relation(&[4]));
    sync(&mut a, &mut b);
    assert_eq!(related(&a, 1, 1), related(&b, 1, 1));
    assert_eq!(rolled(&a, 1, 4), rolled(&b, 1, 4));
    assert_symmetric(&a, 4, 1, 2);
    assert_symmetric(&b, 4, 1, 2);
    assert_healthy(&a);
    assert_healthy(&b);
    // The portable JSON of both agrees too.
    assert_eq!(a.to_portable(), b.to_portable());
}
