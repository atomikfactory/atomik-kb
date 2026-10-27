//! The structure changes of many pages in one step (ADR-0058): `without_descendants`,
//! `move_pages_to_end`, `trash_pages`, `duplicate_subtrees` and `set_pages_property`: the order,
//! the refusals that write nothing, one commit, one undo step, replicas that converge.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kb_types::{OptionId, PageId, PropertyId, PropertyKind, PropertyValue};

use super::{add, assert_healthy, outline, pid, replica, structure, sync, trash_titles};
use crate::{ModelError, StructureDoc};

/// Pages A, B, C, D (ids 1 to 4) at the root, P (5) with X (6) and X1 (7) below it.
fn world() -> StructureDoc {
    let mut doc = structure(1);
    for (n, t) in [(1u8, "A"), (2, "B"), (3, "C"), (4, "D"), (5, "P")] {
        add(&mut doc, n, None, t);
    }
    add(&mut doc, 6, Some(5), "X");
    add(&mut doc, 7, Some(6), "X1");
    doc
}

fn ids(list: &[u8]) -> Vec<PageId> {
    list.iter().map(|n| pid(*n)).collect()
}

/// Runs `op` and counts the local commits it makes (each commit that holds a change is one local
/// update).
fn commits<T>(doc: &mut StructureDoc, op: impl FnOnce(&mut StructureDoc) -> T) -> (T, usize) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let subscription = doc.doc().inner().subscribe_local_update(Box::new(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        true
    }));
    let out = op(doc);
    drop(subscription);
    (out, count.load(Ordering::SeqCst))
}

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xb0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn number_property(doc: &mut StructureDoc) -> PropertyId {
    let mut spare = (0..3u8).map(|n| OptionId::from_random_bytes([n; 16]));
    let id = prop(1);
    doc.create_property(
        id,
        "Amount",
        PropertyKind::Number,
        &mut || spare.next().unwrap(),
        10,
    )
    .unwrap();
    id
}

#[test]
fn without_descendants_keeps_the_tops_in_the_order_given() {
    let doc = world();
    assert_eq!(
        doc.without_descendants(&ids(&[7, 3, 6, 5, 1])),
        ids(&[3, 5, 1]),
        "X and X1 go with P"
    );
    assert_eq!(doc.without_descendants(&ids(&[7, 6])), ids(&[6]));
    assert_eq!(doc.without_descendants(&ids(&[2, 1])), ids(&[2, 1]));
    assert!(doc.without_descendants(&[]).is_empty());
    assert!(doc.is_live_page(pid(7)));
    assert!(!doc.is_live_page(pid(99)));
}

#[test]
fn move_pages_to_end_appends_in_the_order_given_in_one_undo_step() {
    let mut doc = world();
    let (affected, commit_count) = commits(&mut doc, |doc| {
        doc.move_pages_to_end(&ids(&[3, 1]), Some(pid(5)), 50)
            .unwrap()
    });
    assert_eq!(outline(&doc), "B\nD\nP\n  X\n    X1\n  C\n  A\n");
    assert_eq!(affected, ids(&[3, 1]));
    assert_eq!(commit_count, 1, "one commit");
    assert_eq!(doc.page(pid(3)).unwrap().parent_id, Some(pid(5)));
    assert_eq!(doc.page(pid(3)).unwrap().updated_at, 50);
    assert_healthy(&doc);

    // One undo puts both back.
    doc.undo().unwrap().unwrap();
    assert_eq!(outline(&doc), "A\nB\nC\nD\nP\n  X\n    X1\n");
    assert_healthy(&doc);
    doc.redo().unwrap().unwrap();
    assert_eq!(outline(&doc), "B\nD\nP\n  X\n    X1\n  C\n  A\n");

    // To the top level; a subtree travels along.
    let affected = doc.move_pages_to_end(&ids(&[6]), None, 60).unwrap();
    assert_eq!(outline(&doc), "B\nD\nP\n  C\n  A\nX\n  X1\n");
    assert_eq!(affected, ids(&[6, 7]), "the page and its subtree");
}

#[test]
fn pages_already_last_under_the_parent_in_that_order_write_nothing() {
    let mut doc = world();
    let (moved, commit_count) = commits(&mut doc, |doc| {
        doc.move_pages_to_end(&ids(&[3, 4]), None, 50).unwrap()
    });
    assert_eq!(moved, ids(&[3, 4]));
    assert_eq!(commit_count, 1);
    assert_eq!(outline(&doc), "A\nB\nP\n  X\n    X1\nC\nD\n");
    // C and D are now last: the same call again is a no-op.
    let (affected, commit_count) = commits(&mut doc, |doc| {
        doc.move_pages_to_end(&ids(&[3, 4]), None, 60).unwrap()
    });
    assert!(affected.is_empty());
    assert_eq!(commit_count, 0);
    assert_eq!(
        doc.page(pid(3)).unwrap().updated_at,
        50,
        "not stamped again"
    );
}

#[test]
fn a_move_that_cannot_be_done_writes_nothing() {
    let mut doc = world();
    let mut other = structure(2);
    add(&mut other, 40, None, "Elsewhere");
    let version = doc.doc().version();
    let cases: Vec<(&str, Vec<PageId>, Option<PageId>, ModelError)> = vec![
        (
            "an unknown page",
            ids(&[99]),
            None,
            ModelError::PageNotFound,
        ),
        ("a page twice", ids(&[1, 1]), None, ModelError::InvalidOrder),
        (
            "a page and its descendant",
            ids(&[5, 7]),
            None,
            ModelError::InvalidOrder,
        ),
        (
            "under itself",
            ids(&[5]),
            Some(pid(5)),
            ModelError::CyclicMove,
        ),
        (
            "under its own descendant",
            ids(&[5, 1]),
            Some(pid(7)),
            ModelError::CyclicMove,
        ),
        (
            "under an unknown parent",
            ids(&[1]),
            Some(pid(99)),
            ModelError::ParentUnavailable,
        ),
    ];
    for (why, pages, parent, expected) in cases {
        let err = doc.move_pages_to_end(&pages, parent, 50).unwrap_err();
        assert_eq!(
            std::mem::discriminant(&err),
            std::mem::discriminant(&expected),
            "{why}: {err}"
        );
    }
    doc.trash_page(pid(2), 40).unwrap();
    let version_after_trash = doc.doc().version();
    assert!(version_after_trash != version);
    assert!(matches!(
        doc.move_pages_to_end(&ids(&[2]), None, 50),
        Err(ModelError::PageTrashed)
    ));
    assert!(matches!(
        doc.move_pages_to_end(&ids(&[1]), Some(pid(2)), 50),
        Err(ModelError::ParentUnavailable)
    ));
    assert_eq!(
        doc.doc().version(),
        version_after_trash,
        "nothing was written"
    );
}

#[test]
fn trash_pages_trashes_each_with_its_subtree_in_one_undo_step() {
    let mut doc = world();
    let (affected, commit_count) =
        commits(&mut doc, |doc| doc.trash_pages(&ids(&[5, 2]), 70).unwrap());
    assert_eq!(
        affected,
        ids(&[5, 6, 7, 2]),
        "every page and its subtree, pre-order"
    );
    assert_eq!(commit_count, 1, "one commit");
    assert_eq!(outline(&doc), "A\nC\nD\n");
    assert_eq!(trash_titles(&doc).len(), 2);
    let p = doc.trashed_pages();
    let p_entry = p.iter().find(|t| t.page.id == pid(5)).unwrap();
    assert_eq!(p_entry.descendant_count, 2);
    assert_eq!(doc.page(pid(7)).unwrap().trashed_at, Some(70));
    assert_healthy(&doc);

    doc.undo().unwrap().unwrap();
    assert_eq!(outline(&doc), "A\nB\nC\nD\nP\n  X\n    X1\n");
    assert!(trash_titles(&doc).is_empty());
    assert_healthy(&doc);

    // A restore puts a page back under the parent it came from.
    doc.redo().unwrap().unwrap();
    doc.restore_page(pid(5), 80).unwrap();
    assert_eq!(outline(&doc), "A\nC\nD\nP\n  X\n    X1\n");
}

#[test]
fn trash_pages_refuses_what_it_cannot_trash_before_writing() {
    let mut doc = world();
    let version = doc.doc().version();
    assert!(matches!(
        doc.trash_pages(&ids(&[1, 99]), 70),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        doc.trash_pages(&ids(&[1, 1]), 70),
        Err(ModelError::InvalidOrder)
    ));
    assert!(matches!(
        doc.trash_pages(&ids(&[1, 6, 5]), 70),
        Err(ModelError::InvalidOrder)
    ));
    assert_eq!(doc.doc().version(), version);
    doc.trash_page(pid(1), 60).unwrap();
    let version = doc.doc().version();
    assert!(matches!(
        doc.trash_pages(&ids(&[2, 1]), 70),
        Err(ModelError::PageTrashed)
    ));
    assert_eq!(doc.doc().version(), version, "B was not trashed either");
    assert!(doc.trash_pages(&[], 70).unwrap().is_empty());
}

#[test]
fn replicas_converge_after_a_bulk_move_and_a_bulk_trash() {
    let mut doc = world();
    let mut other = replica(&doc, 2);
    doc.move_pages_to_end(&ids(&[3, 1]), Some(pid(5)), 50)
        .unwrap();
    other.trash_pages(&ids(&[4]), 51).unwrap();
    sync(&mut doc, &mut other);
    assert_eq!(outline(&doc), outline(&other));
    assert_eq!(outline(&doc), "B\nP\n  X\n    X1\n  C\n  A\n");
    assert_healthy(&doc);
    assert_healthy(&other);
}

#[test]
fn duplicate_subtrees_copies_each_page_right_after_it_in_one_undo_step() {
    let mut doc = world();
    let ids_of_copies: HashMap<PageId, PageId> = [(1u8, 11u8), (5, 15), (6, 16), (7, 17)]
        .iter()
        .map(|(source, copy)| (pid(*source), pid(*copy)))
        .collect();
    let (duplicated, commit_count) = commits(&mut doc, |doc| {
        doc.duplicate_subtrees(
            &[
                (pid(5), "P (copy)".to_owned()),
                (pid(1), "A (copy)".to_owned()),
            ],
            &ids_of_copies,
            90,
        )
        .unwrap()
    });
    assert_eq!(
        outline(&doc),
        "A\nA (copy)\nB\nC\nD\nP\n  X\n    X1\nP (copy)\n  X\n    X1\n"
    );
    assert_eq!(
        duplicated.pages,
        vec![
            (pid(5), pid(15)),
            (pid(6), pid(16)),
            (pid(7), pid(17)),
            (pid(1), pid(11)),
        ],
        "the pages of each top in pre-order, the tops in the order given"
    );
    assert_eq!(commit_count, 1, "one commit for the structure");
    assert_eq!(doc.page(pid(15)).unwrap().created_at, 90);
    assert_healthy(&doc);

    doc.undo().unwrap().unwrap();
    assert_eq!(outline(&doc), "A\nB\nC\nD\nP\n  X\n    X1\n");
    assert_healthy(&doc);
    doc.redo().unwrap().unwrap();
    assert_eq!(doc.page(pid(17)).unwrap().title, "X1");
}

#[test]
fn duplicate_subtrees_refuses_overlaps_and_taken_ids_before_writing() {
    let mut doc = world();
    let copies: HashMap<PageId, PageId> = [(1u8, 11u8), (5, 15), (6, 16), (7, 17)]
        .iter()
        .map(|(source, copy)| (pid(*source), pid(*copy)))
        .collect();
    let version = doc.doc().version();
    // A page and a page of its subtree: the copy ids repeat.
    assert!(matches!(
        doc.duplicate_subtrees(&[(pid(5), "a".into()), (pid(6), "b".into())], &copies, 90),
        Err(ModelError::DuplicatePage)
    ));
    // A copy id that exists.
    let mut taken = copies.clone();
    taken.insert(pid(1), pid(2));
    assert!(matches!(
        doc.duplicate_subtrees(&[(pid(1), "a".into())], &taken, 90),
        Err(ModelError::DuplicatePage)
    ));
    // A page of the subtree without a copy id.
    let mut partial = copies.clone();
    partial.remove(&pid(7));
    assert!(matches!(
        doc.duplicate_subtrees(&[(pid(5), "a".into())], &partial, 90),
        Err(ModelError::Shape(_))
    ));
    assert!(matches!(
        doc.duplicate_subtrees(&[(pid(99), "a".into())], &copies, 90),
        Err(ModelError::PageNotFound)
    ));
    doc.trash_page(pid(1), 80).unwrap();
    let after_trash = doc.doc().version();
    assert!(after_trash != version);
    assert!(matches!(
        doc.duplicate_subtrees(&[(pid(5), "a".into()), (pid(1), "b".into())], &copies, 90),
        Err(ModelError::PageTrashed)
    ));
    assert_eq!(doc.doc().version(), after_trash, "nothing was written");
    assert!(
        doc.duplicate_subtrees(&[], &copies, 90)
            .unwrap()
            .pages
            .is_empty()
    );
}

#[test]
fn set_pages_property_writes_all_the_pages_in_one_commit() {
    let mut doc = world();
    let amount = number_property(&mut doc);
    let (written, commit_count) = commits(&mut doc, |doc| {
        doc.set_pages_property(
            amount,
            &[
                (pid(1), Some(PropertyValue::Number(1.0))),
                (pid(2), Some(PropertyValue::Number(2.0))),
                (pid(3), None),
            ],
            100,
        )
        .unwrap()
    });
    assert_eq!(written, ids(&[1, 2]), "page C had nothing to clear");
    assert_eq!(commit_count, 1, "one commit for all the pages");
    let value = |doc: &StructureDoc, n: u8| {
        doc.page_properties(pid(n))
            .unwrap()
            .into_iter()
            .find(|(id, _)| *id == amount)
            .map(|(_, value)| value)
    };
    assert_eq!(value(&doc, 1), Some(PropertyValue::Number(1.0)));
    assert_eq!(value(&doc, 2), Some(PropertyValue::Number(2.0)));
    assert_eq!(value(&doc, 3), None);
    assert_eq!(doc.page(pid(1)).unwrap().updated_at, 100);

    // What a page holds already writes nothing.
    let again = doc
        .set_pages_property(amount, &[(pid(1), Some(PropertyValue::Number(1.0)))], 200)
        .unwrap();
    assert!(again.is_empty());
    assert_eq!(doc.page(pid(1)).unwrap().updated_at, 100);

    // Values are not an undo step.
    assert!(doc.can_undo());
    doc.undo().unwrap().unwrap();
    assert_eq!(value(&doc, 1), Some(PropertyValue::Number(1.0)));
}

#[test]
fn set_pages_property_checks_everything_before_writing() {
    let mut doc = world();
    let amount = number_property(&mut doc);
    let version = doc.doc().version();
    // A value of another kind for the last page: the first is not written either.
    assert!(matches!(
        doc.set_pages_property(
            amount,
            &[
                (pid(1), Some(PropertyValue::Number(1.0))),
                (pid(2), Some(PropertyValue::Text("x".into()))),
            ],
            100
        ),
        Err(ModelError::InvalidProperty { field: "value", .. })
    ));
    // A number that is not finite.
    assert!(matches!(
        doc.set_pages_property(
            amount,
            &[(pid(1), Some(PropertyValue::Number(f64::NAN)))],
            100
        ),
        Err(ModelError::InvalidProperty { .. })
    ));
    // An unknown page.
    assert!(matches!(
        doc.set_pages_property(
            amount,
            &[
                (pid(1), Some(PropertyValue::Number(1.0))),
                (pid(99), Some(PropertyValue::Number(2.0))),
            ],
            100
        ),
        Err(ModelError::PageNotFound)
    ));
    // An unknown property.
    assert!(matches!(
        doc.set_pages_property(prop(9), &[(pid(1), None)], 100),
        Err(ModelError::PropertyNotFound)
    ));
    assert_eq!(doc.doc().version(), version, "nothing was written");
    assert!(doc.set_pages_property(amount, &[], 100).unwrap().is_empty());
}
