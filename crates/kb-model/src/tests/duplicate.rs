//! Duplicating a subtree (ADR-0040, "Duplicate a page with its sub-pages"): the copies, their
//! place, titles, icons and values, the relations, one undo step and the refusals.

use std::collections::HashMap;

use kb_types::{
    DeviceId, OptionColor, OptionId, PageFont, PageId, PropertyDef, PropertyId, PropertyKind,
    PropertyValue,
};

use super::{add, assert_healthy, outline, pid, replica, structure, sync};
use crate::{Duplicated, ModelError, StructureDoc};

fn device() -> DeviceId {
    DeviceId::from_random_bytes([0xd1, 7, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 7])
}

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xd2, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn opt(n: u8) -> OptionId {
    OptionId::from_random_bytes([0xd3, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

/// The id of the copy of `pid(n)`: `pid(n + 100)`.
fn copy(n: u8) -> PageId {
    pid(n + 100)
}

/// The number a page id was made from.
fn number_of(id: PageId) -> u8 {
    id.to_bytes()[0]
}

/// Inbox(1); Project(2, icon, values) with Plan(3), Notes(4) that holds Deep(5), and Gone(6) in the
/// trash; Archive(7). Favourites: Project, Plan. The schema: Status (select, "Doing"), Cost
/// (number), and a two-way relation `Blocks` (`prop(3)`) with its back side `prop(4)`.
fn world() -> StructureDoc {
    let mut doc = structure(1);
    doc.set_author(Some(device()));
    let mut next = 0;
    doc.create_property(
        prop(1),
        "Status",
        PropertyKind::Select,
        &mut || {
            next += 1;
            opt(100 + next)
        },
        1_000,
    )
    .unwrap();
    doc.add_option(prop(1), opt(1), "Doing", OptionColor::Blue, None, 1_001)
        .unwrap();
    doc.create_property(
        prop(2),
        "Cost",
        PropertyKind::Number,
        &mut || opt(200),
        1_002,
    )
    .unwrap();
    doc.create_relation_pair(prop(3), prop(4), "Blocks", "Blocked by", 1_003)
        .unwrap();
    add(&mut doc, 1, None, "Inbox");
    add(&mut doc, 2, None, "Project");
    add(&mut doc, 3, Some(2), "Plan");
    add(&mut doc, 4, Some(2), "Notes");
    add(&mut doc, 5, Some(4), "Deep");
    add(&mut doc, 6, Some(2), "Gone");
    add(&mut doc, 7, None, "Archive");
    doc.set_page_icon(pid(2), Some("⭐"), 20).unwrap();
    doc.set_page_cover(pid(2), Some("gradient:sunset"), Some(30), 21)
        .unwrap();
    doc.set_page_cover(pid(3), Some("color:blue"), None, 22)
        .unwrap();
    doc.set_page_appearance(pid(2), Some(PageFont::Serif), Some(true), Some(true), 23)
        .unwrap();
    doc.set_page_appearance(pid(3), Some(PageFont::Mono), None, None, 24)
        .unwrap();
    doc.set_page_property(pid(2), prop(1), Some(PropertyValue::Select(opt(1))), 30)
        .unwrap();
    doc.set_page_property(pid(2), prop(2), Some(PropertyValue::Number(3.5)), 31)
        .unwrap();
    doc.set_page_property(pid(5), prop(2), Some(PropertyValue::Number(7.0)), 32)
        .unwrap();
    doc.set_favorite(pid(2), true).unwrap();
    doc.set_favorite(pid(3), true).unwrap();
    doc.trash_page(pid(6), 40).unwrap();
    doc
}

/// The new ids of the pages of the subtree of `top`.
fn ids_of(doc: &StructureDoc, top: PageId) -> HashMap<PageId, PageId> {
    doc.subtree(top)
        .unwrap()
        .into_iter()
        .map(|id| (id, pid(number_of(id) + 100)))
        .collect()
}

fn duplicate(doc: &mut StructureDoc, top: u8, title: &str, now: i64) -> Duplicated {
    let ids = ids_of(doc, pid(top));
    doc.duplicate_subtree(pid(top), &ids, title, now).unwrap()
}

fn value(doc: &StructureDoc, page: PageId, property: u8) -> Option<PropertyValue> {
    doc.page_properties(page)
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == prop(property))
        .map(|(_, value)| value)
}

/// The pages (as numbers) a relation value of `page` lists.
fn related(doc: &StructureDoc, page: PageId, property: u8) -> Vec<u8> {
    match value(doc, page, property) {
        Some(PropertyValue::Relation(pages)) => pages.into_iter().map(number_of).collect(),
        None => Vec::new(),
        other => panic!("not a relation: {other:?}"),
    }
}

fn schema(doc: &StructureDoc) -> Vec<PropertyDef> {
    doc.property_schema()
}

// ----- the copies --------------------------------------------------------------------------

#[test]
fn the_live_subtree_is_copied_right_after_the_page_with_new_ids() {
    let mut doc = world();
    let favorites = doc.favorites();
    let original = doc.page(pid(2)).unwrap();

    let done = duplicate(&mut doc, 2, "Project (copy)", 500);

    assert_eq!(
        done.pages,
        vec![
            (pid(2), copy(2)),
            (pid(3), copy(3)),
            (pid(4), copy(4)),
            (pid(5), copy(5))
        ],
        "pre-order, the top page first; the trashed page is not copied"
    );
    assert_eq!(
        outline(&doc),
        "Inbox\nProject\n  Plan\n  Notes\n    Deep\nProject (copy)\n  Plan\n  Notes\n    Deep\nArchive\n"
    );
    assert_eq!(
        doc.trashed_pages().len(),
        1,
        "the trashed page stays where it was"
    );
    assert_eq!(
        doc.page(pid(2)).unwrap(),
        original,
        "the original is untouched"
    );
    assert_eq!(doc.favorites(), favorites, "favourites are not copied");
    assert_healthy(&doc);

    // The top copy: the given title, the icon, new times, this device; no parent.
    let top = doc.page(copy(2)).unwrap();
    assert_eq!(top.title, "Project (copy)");
    assert_eq!(top.icon.as_deref(), Some("⭐"));
    // The cover and its position are copied with the page, and sub-pages have their own.
    assert_eq!(top.cover.as_deref(), Some("gradient:sunset"));
    assert_eq!(top.cover_y, Some(30));
    let plan = doc.page(copy(3)).unwrap();
    assert_eq!(
        (plan.cover.as_deref(), plan.cover_y),
        (Some("color:blue"), None)
    );
    assert_eq!(doc.page(copy(4)).unwrap().cover, None);
    // The font, the small text and the lock are copied too (a copy of a locked page is locked).
    assert_eq!(
        (top.font, top.small_text, top.locked),
        (Some(PageFont::Serif), true, true)
    );
    assert_eq!(
        (plan.font, plan.small_text, plan.locked),
        (Some(PageFont::Mono), false, false)
    );
    let notes = doc.page(copy(4)).unwrap();
    assert_eq!(
        (notes.font, notes.small_text, notes.locked),
        (None, false, false)
    );
    assert_eq!(top.parent_id, None);
    assert_eq!((top.created_at, top.updated_at), (500, 500));
    assert_eq!(top.created_by, Some(device()));
    assert_eq!(top.edited_by, Some(device()));
    // The sub-pages keep their titles and hang under the copies of their parents.
    let deep = doc.page(copy(5)).unwrap();
    assert_eq!(deep.title, "Deep");
    assert_eq!(deep.parent_id, Some(copy(4)));
    assert_eq!(doc.page(copy(3)).unwrap().parent_id, Some(copy(2)));
    assert_eq!(doc.live_page_count(), 10, "six live pages and four copies");
}

#[test]
fn a_copy_goes_after_its_page_among_its_siblings_at_any_depth() {
    let mut doc = world();
    // At the root, between two pages: Inbox, Project, Archive -> Inbox, Inbox (copy), Project, ...
    duplicate(&mut doc, 1, "Inbox (copy)", 500);
    assert_eq!(
        outline(&doc)
            .lines()
            .filter(|line| !line.starts_with(' '))
            .collect::<Vec<_>>(),
        ["Inbox", "Inbox (copy)", "Project", "Archive"]
    );
    // The last page of the root.
    duplicate(&mut doc, 7, "Archive (copy)", 501);
    assert_eq!(
        outline(&doc)
            .lines()
            .filter(|line| !line.starts_with(' '))
            .collect::<Vec<_>>(),
        [
            "Inbox",
            "Inbox (copy)",
            "Project",
            "Archive",
            "Archive (copy)"
        ]
    );
    // Inside a parent, before its last live child (the trashed child is not a sibling).
    duplicate(&mut doc, 3, "Plan (copy)", 502);
    let titles: Vec<String> = doc
        .live_tree()
        .into_iter()
        .find(|node| node.page.id == pid(2))
        .unwrap()
        .children
        .into_iter()
        .map(|node| node.page.title)
        .collect();
    assert_eq!(titles, ["Plan", "Plan (copy)", "Notes"]);
    assert_eq!(doc.page(copy(3)).unwrap().parent_id, Some(pid(2)));
    assert_healthy(&doc);
}

#[test]
fn values_are_copied_and_computed_ones_are_the_copys_own() {
    let mut doc = world();
    duplicate(&mut doc, 2, "Project (copy)", 500);
    assert_eq!(
        value(&doc, copy(2), 1),
        Some(PropertyValue::Select(opt(1))),
        "a select"
    );
    assert_eq!(value(&doc, copy(2), 2), Some(PropertyValue::Number(3.5)));
    assert_eq!(value(&doc, copy(5), 2), Some(PropertyValue::Number(7.0)));
    assert_eq!(
        value(&doc, copy(3), 2),
        None,
        "a page without a value has none"
    );
    // The original keeps its own.
    assert_eq!(value(&doc, pid(2), 2), Some(PropertyValue::Number(3.5)));
    assert_eq!(
        schema(&doc).len(),
        5,
        "no property is created (Tags, two, the pair)"
    );
    assert_healthy(&doc);
}

#[test]
fn two_way_relations_keep_both_sides_and_relate_the_copies_among_themselves() {
    let mut doc = world();
    let relation = |pages: &[u8]| PropertyValue::Relation(pages.iter().map(|n| pid(*n)).collect());
    // Project blocks Archive (outside the subtree); Plan blocks Notes (inside).
    doc.set_page_property(pid(2), prop(3), Some(relation(&[7])), 50)
        .unwrap();
    doc.set_page_property(pid(3), prop(3), Some(relation(&[4])), 51)
        .unwrap();
    assert_eq!(related(&doc, pid(7), 4), [2], "the paired side, before");

    let done = duplicate(&mut doc, 2, "Project (copy)", 500);

    // Outside: the copy lists the same page, and that page lists both sides of the pair.
    assert_eq!(related(&doc, copy(2), 3), [7]);
    assert_eq!(
        related(&doc, pid(7), 4),
        [2, 102],
        "Archive is blocked by both"
    );
    // Inside: the copy of Plan blocks the copy of Notes, not the original.
    assert_eq!(related(&doc, copy(3), 3), [104]);
    assert_eq!(
        related(&doc, copy(4), 4),
        [103],
        "the paired side of the copy"
    );
    assert_eq!(
        related(&doc, pid(4), 4),
        [3],
        "the original Notes is blocked by the original Plan only"
    );
    assert_eq!(related(&doc, pid(3), 3), [4]);
    // The pages whose values were written: the copies that have values and the outside page.
    assert!(done.touched.contains(&copy(2)) && done.touched.contains(&pid(7)));
    assert!(
        !done.touched.contains(&pid(4)),
        "an original is not touched"
    );
    assert_healthy(&doc);
}

#[test]
fn a_relation_that_the_original_shows_from_the_other_side_is_copied_too() {
    let mut doc = world();
    // Archive stores `Blocks` listing Notes: Notes shows it as `Blocked by` (the paired side).
    doc.set_page_property(
        pid(7),
        prop(3),
        Some(PropertyValue::Relation(vec![pid(4)])),
        50,
    )
    .unwrap();
    duplicate(&mut doc, 2, "Project (copy)", 500);
    assert_eq!(related(&doc, pid(4), 4), [7]);
    assert_eq!(
        related(&doc, copy(4), 4),
        [7],
        "the copy of Notes is blocked by Archive as well"
    );
    assert_eq!(
        related(&doc, pid(7), 3),
        [4, 104],
        "and Archive blocks both"
    );
    assert_healthy(&doc);
}

// ----- undo, replicas ---------------------------------------------------------------------

#[test]
fn a_duplication_is_one_undo_step_and_redo_brings_the_copies_and_their_values_back() {
    let mut doc = world();
    let relation = PropertyValue::Relation(vec![pid(7)]);
    doc.set_page_property(pid(2), prop(3), Some(relation), 50)
        .unwrap();
    let before = outline(&doc);
    duplicate(&mut doc, 2, "Project (copy)", 500);
    let after = outline(&doc);
    assert_ne!(before, after);
    let values = doc.page_properties(copy(2)).unwrap();
    assert_eq!(values.len(), 3);

    // One undo removes the four copies together.
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(outline(&doc), before);
    assert!(doc.page(copy(2)).is_none() && doc.page(copy(5)).is_none());
    assert_eq!(
        related(&doc, pid(7), 4),
        [2],
        "a page that is gone is not listed"
    );
    assert_healthy(&doc);

    // One redo brings them all back, with their values.
    assert!(doc.redo().unwrap().is_some());
    assert_eq!(outline(&doc), after);
    assert_eq!(doc.page_properties(copy(2)).unwrap(), values);
    assert_eq!(doc.page(copy(5)).unwrap().parent_id, Some(copy(4)));
    assert_eq!(related(&doc, pid(7), 4), [2, 102]);
    assert_healthy(&doc);

    // The copies are one step: undoing again removes them again, and the steps before are intact.
    assert!(doc.undo().unwrap().is_some());
    assert_eq!(outline(&doc), before);
    assert!(doc.can_undo(), "the earlier steps are still there");
    assert!(doc.can_redo());
}

#[test]
fn a_copy_is_an_ordinary_page_for_a_replica() {
    let mut a = world();
    let mut b = replica(&a, 2);
    duplicate(&mut a, 2, "Project (copy)", 500);
    sync(&mut a, &mut b);
    assert_eq!(outline(&a), outline(&b));
    assert_eq!(value(&b, copy(5), 2), Some(PropertyValue::Number(7.0)));
    assert_healthy(&b);
    // The replica can duplicate the copy in turn.
    let mut ids = HashMap::new();
    for id in b.subtree(copy(2)).unwrap() {
        ids.insert(id, pid(number_of(id) + 50));
    }
    b.duplicate_subtree(copy(2), &ids, "Project (copy) (copy)", 600)
        .unwrap();
    sync(&mut b, &mut a);
    assert_eq!(outline(&a), outline(&b));
    assert_healthy(&a);
}

// ----- refusals ---------------------------------------------------------------------------

#[test]
fn refusals_write_nothing() {
    let mut doc = world();
    let before = outline(&doc);
    let version = doc.doc().version();
    let ids = ids_of(&doc, pid(2));

    // Unknown, trashed (the trashed page itself), and a page below a trashed one are refused.
    assert!(matches!(
        doc.duplicate_subtree(pid(99), &ids, "x", 500),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        doc.duplicate_subtree(pid(6), &ids_of(&doc, pid(6)), "x", 500),
        Err(ModelError::PageTrashed)
    ));
    // A new id that is taken (by a live page, a trashed one), repeated or missing.
    let mut taken = ids.clone();
    taken.insert(pid(3), pid(7));
    assert!(matches!(
        doc.duplicate_subtree(pid(2), &taken, "x", 500),
        Err(ModelError::DuplicatePage)
    ));
    let mut trashed = ids.clone();
    trashed.insert(pid(3), pid(6));
    assert!(matches!(
        doc.duplicate_subtree(pid(2), &trashed, "x", 500),
        Err(ModelError::DuplicatePage)
    ));
    let mut twice = ids.clone();
    twice.insert(pid(3), copy(2));
    assert!(matches!(
        doc.duplicate_subtree(pid(2), &twice, "x", 500),
        Err(ModelError::DuplicatePage)
    ));
    let mut missing = ids;
    missing.remove(&pid(5));
    assert!(matches!(
        doc.duplicate_subtree(pid(2), &missing, "x", 500),
        Err(ModelError::Shape(_))
    ));

    assert_eq!(outline(&doc), before);
    assert_eq!(doc.doc().version(), version, "nothing was written");
    assert_healthy(&doc);
}

#[test]
fn a_purged_id_cannot_be_reused_for_a_copy() {
    let mut doc = world();
    doc.purge_page(pid(6), 60).unwrap();
    let mut ids = ids_of(&doc, pid(2));
    ids.insert(pid(3), pid(6));
    assert!(matches!(
        doc.duplicate_subtree(pid(2), &ids, "x", 500),
        Err(ModelError::DuplicatePage)
    ));
}
