//! Favourites of a space (ADR-0029, section 2): order, idempotence, moves, concurrent merges,
//! the trash, purges, undo, and the portable output.

use serde_json::json;

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{ModelError, StructureDoc};

fn favorite_ids(doc: &StructureDoc) -> Vec<u8> {
    // The page ids of these tests are `pid(n)` = sixteen bytes of `n`; recover `n`.
    doc.favorites()
        .into_iter()
        .map(|id| {
            let text = id.to_string();
            u8::from_str_radix(&text[..2], 16).unwrap()
        })
        .collect()
}

/// A document with live root pages 1 to `n` (titles `P1`, `P2`, ...).
fn with_pages(n: u8) -> StructureDoc {
    let mut doc = structure(1);
    for i in 1..=n {
        add(&mut doc, i, None, &format!("P{i}"));
    }
    doc
}

fn invalid_field(err: &ModelError) -> Option<&'static str> {
    match err {
        ModelError::InvalidFavorite { field, .. } => Some(field),
        _ => None,
    }
}

#[test]
fn a_new_space_has_no_favourites_and_exports_none() {
    let doc = with_pages(2);
    assert!(doc.favorites().is_empty());
    assert!(!doc.is_favorite(pid(1)));
    assert!(doc.to_portable().get("favorites").is_none());
    assert_healthy(&doc);
}

#[test]
fn favourites_are_listed_in_the_order_they_were_added() {
    let mut doc = with_pages(4);
    for n in [3, 1, 4] {
        assert!(doc.set_favorite(pid(n), true).unwrap());
    }
    assert_eq!(favorite_ids(&doc), [3, 1, 4]);
    assert!(doc.is_favorite(pid(1)));
    assert!(!doc.is_favorite(pid(2)));
    assert_healthy(&doc);
    // The order survives a reload from a snapshot.
    assert_eq!(favorite_ids(&replica(&doc, 2)), [3, 1, 4]);
}

#[test]
fn adding_is_idempotent_and_keeps_the_place() {
    let mut doc = with_pages(3);
    doc.set_favorite(pid(1), true).unwrap();
    doc.set_favorite(pid(2), true).unwrap();
    let before = doc.to_portable();
    assert!(
        !doc.set_favorite(pid(1), true).unwrap(),
        "already a favourite"
    );
    assert_eq!(favorite_ids(&doc), [1, 2]);
    assert_eq!(doc.to_portable(), before, "nothing was written");
}

#[test]
fn removing_deletes_the_entry_and_is_idempotent() {
    let mut doc = with_pages(3);
    doc.set_favorite(pid(1), true).unwrap();
    doc.set_favorite(pid(2), true).unwrap();
    assert!(doc.set_favorite(pid(1), false).unwrap());
    assert_eq!(favorite_ids(&doc), [2]);
    assert!(
        !doc.set_favorite(pid(1), false).unwrap(),
        "not a favourite any more"
    );
    assert!(
        !doc.set_favorite(pid(3), false).unwrap(),
        "never was a favourite"
    );
    assert!(doc.set_favorite(pid(2), false).unwrap());
    assert!(doc.favorites().is_empty());
    assert!(
        doc.to_portable().get("favorites").is_none(),
        "an empty map is not exported"
    );
    // Adding again puts it last, whatever its old place was.
    doc.set_favorite(pid(2), true).unwrap();
    doc.set_favorite(pid(1), true).unwrap();
    assert_eq!(favorite_ids(&doc), [2, 1]);
    assert_healthy(&doc);
}

#[test]
fn adding_needs_a_live_page_and_removing_needs_nothing() {
    let mut doc = with_pages(2);
    assert!(matches!(
        doc.set_favorite(pid(9), true),
        Err(ModelError::PageNotFound)
    ));
    doc.set_favorite(pid(1), true).unwrap();
    doc.trash_page(pid(1), 100).unwrap();
    assert!(matches!(
        doc.set_favorite(pid(1), true),
        Err(ModelError::PageTrashed)
    ));
    // Also for a page that was never a favourite.
    doc.trash_page(pid(2), 101).unwrap();
    assert!(matches!(
        doc.set_favorite(pid(2), true),
        Err(ModelError::PageTrashed)
    ));
    assert!(doc.favorites().is_empty());
    // Removing works for a trashed page, and for an id that names nothing.
    assert!(doc.set_favorite(pid(1), false).unwrap());
    assert!(!doc.set_favorite(pid(9), false).unwrap());
    assert_healthy(&doc);
}

#[test]
fn a_move_puts_the_favourite_in_front_of_another_or_last() {
    let mut doc = with_pages(5);
    for n in 1..=5 {
        doc.set_favorite(pid(n), true).unwrap();
    }
    assert_eq!(favorite_ids(&doc), [1, 2, 3, 4, 5]);

    assert!(doc.move_favorite(pid(4), Some(pid(2))).unwrap());
    assert_eq!(favorite_ids(&doc), [1, 4, 2, 3, 5]);
    assert!(doc.move_favorite(pid(5), Some(pid(1))).unwrap());
    assert_eq!(favorite_ids(&doc), [5, 1, 4, 2, 3]);
    assert!(doc.move_favorite(pid(5), None).unwrap());
    assert_eq!(favorite_ids(&doc), [1, 4, 2, 3, 5]);
    assert!(doc.move_favorite(pid(2), Some(pid(4))).unwrap());
    assert_eq!(favorite_ids(&doc), [1, 2, 4, 3, 5]);
    assert_healthy(&doc);
    assert_eq!(
        favorite_ids(&replica(&doc, 2)),
        [1, 2, 4, 3, 5],
        "the new order is in the document"
    );
}

#[test]
fn a_move_to_the_place_it_holds_writes_nothing() {
    let mut doc = with_pages(3);
    for n in 1..=3 {
        doc.set_favorite(pid(n), true).unwrap();
    }
    let before = doc.to_portable();
    assert!(
        !doc.move_favorite(pid(2), Some(pid(3))).unwrap(),
        "in place"
    );
    assert!(!doc.move_favorite(pid(3), None).unwrap(), "already last");
    assert!(!doc.move_favorite(pid(1), Some(pid(2))).unwrap());
    assert!(
        !doc.move_favorite(pid(2), Some(pid(2))).unwrap(),
        "in front of itself"
    );
    assert_eq!(doc.to_portable(), before);
}

#[test]
fn a_move_refuses_pages_that_are_not_favourites() {
    let mut doc = with_pages(3);
    doc.set_favorite(pid(1), true).unwrap();
    doc.set_favorite(pid(2), true).unwrap();
    let err = doc.move_favorite(pid(3), None).unwrap_err();
    assert_eq!(invalid_field(&err), Some("pageId"), "{err}");
    let err = doc.move_favorite(pid(9), None).unwrap_err();
    assert_eq!(invalid_field(&err), Some("pageId"), "{err}");
    let err = doc.move_favorite(pid(1), Some(pid(3))).unwrap_err();
    assert_eq!(invalid_field(&err), Some("before"), "{err}");
    let err = doc.move_favorite(pid(1), Some(pid(9))).unwrap_err();
    assert_eq!(invalid_field(&err), Some("before"), "{err}");
    assert_eq!(favorite_ids(&doc), [1, 2], "nothing changed");
}

#[test]
fn a_move_skips_hidden_favourites_as_neighbours() {
    let mut doc = with_pages(4);
    for n in 1..=4 {
        doc.set_favorite(pid(n), true).unwrap();
    }
    doc.trash_page(pid(2), 100).unwrap();
    assert_eq!(favorite_ids(&doc), [1, 3, 4]);
    // A trashed favourite is not a place to go in front of.
    let err = doc.move_favorite(pid(4), Some(pid(2))).unwrap_err();
    assert_eq!(invalid_field(&err), Some("before"));
    assert!(doc.move_favorite(pid(4), Some(pid(3))).unwrap());
    assert_eq!(favorite_ids(&doc), [1, 4, 3]);
    doc.restore_page(pid(2), 200).unwrap();
    assert_eq!(favorite_ids(&doc).len(), 4, "the favourite comes back");
    assert_healthy(&doc);
}

#[test]
fn many_favourites_keep_their_order_through_repeated_moves() {
    let mut doc = with_pages(40);
    for n in 1..=40 {
        doc.set_favorite(pid(n), true).unwrap();
    }
    let mut expected: Vec<u8> = (1..=40).collect();
    assert_eq!(favorite_ids(&doc), expected);
    // Always move the last one to the front: the keys between "nothing" and the first one.
    for _ in 0..60 {
        let last = *expected.last().unwrap();
        let first = expected[0];
        doc.move_favorite(pid(last), Some(pid(first))).unwrap();
        expected.pop();
        expected.insert(0, last);
        assert_eq!(favorite_ids(&doc), expected);
    }
    // And squeeze one between the first two, again and again.
    for _ in 0..40 {
        let last = *expected.last().unwrap();
        let second = expected[1];
        doc.move_favorite(pid(last), Some(pid(second))).unwrap();
        expected.pop();
        expected.insert(1, last);
        assert_eq!(favorite_ids(&doc), expected);
    }
    assert_healthy(&doc);
}

#[test]
fn tied_positions_are_ordered_by_page_id_and_a_move_renumbers_them() {
    let base = with_pages(3);
    let mut a = replica(&base, 2);
    let mut b = replica(&base, 3);
    // Both devices add their first favourite at the same moment: the same position.
    a.set_favorite(pid(1), true).unwrap();
    b.set_favorite(pid(2), true).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(favorite_ids(&a), [1, 2], "ties are ordered by page id");
    assert_eq!(favorite_ids(&b), [1, 2]);
    let portable = a.to_portable();
    let positions = &portable["favorites"];
    assert_eq!(
        positions[pid(1).to_string()],
        positions[pid(2).to_string()],
        "tied"
    );

    a.set_favorite(pid(3), true).unwrap();
    assert_eq!(favorite_ids(&a), [1, 2, 3]);
    // Going between the two tied favourites has no position that fits: all are renumbered.
    assert!(a.move_favorite(pid(3), Some(pid(2))).unwrap());
    assert_eq!(favorite_ids(&a), [1, 3, 2]);
    let portable = a.to_portable();
    let listed: Vec<String> = [1, 3, 2]
        .iter()
        .map(|n| {
            portable["favorites"][pid(*n).to_string()]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(
        listed.windows(2).all(|pair| pair[0] < pair[1]),
        "distinct, increasing positions: {listed:?}"
    );
    sync(&mut a, &mut b);
    assert_eq!(favorite_ids(&b), [1, 3, 2]);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
}

#[test]
fn concurrent_favourites_of_different_pages_merge() {
    let base = with_pages(4);
    let mut a = replica(&base, 2);
    let mut b = replica(&base, 3);
    a.set_favorite(pid(1), true).unwrap();
    a.set_favorite(pid(2), true).unwrap();
    b.set_favorite(pid(3), true).unwrap();
    b.set_favorite(pid(4), true).unwrap();
    sync(&mut a, &mut b);
    let mut merged = favorite_ids(&a);
    assert_eq!(
        favorite_ids(&b),
        merged,
        "both replicas read the same order"
    );
    merged.sort_unstable();
    assert_eq!(merged, [1, 2, 3, 4]);
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
}

#[test]
fn concurrent_moves_of_one_favourite_converge() {
    let mut base = with_pages(4);
    for n in 1..=4 {
        base.set_favorite(pid(n), true).unwrap();
    }
    let mut a = replica(&base, 2);
    let mut b = replica(&base, 3);
    a.move_favorite(pid(4), Some(pid(1))).unwrap();
    b.move_favorite(pid(4), Some(pid(3))).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(favorite_ids(&a), favorite_ids(&b), "last writer wins");
    assert!(matches!(
        favorite_ids(&a).as_slice(),
        [4, 1, 2, 3] | [1, 2, 4, 3]
    ));
    assert_eq!(a.to_portable(), b.to_portable());
}

#[test]
fn a_concurrent_remove_and_add_merge_per_page() {
    let mut base = with_pages(3);
    base.set_favorite(pid(1), true).unwrap();
    base.set_favorite(pid(2), true).unwrap();
    let mut a = replica(&base, 2);
    let mut b = replica(&base, 3);
    a.set_favorite(pid(1), false).unwrap();
    b.set_favorite(pid(3), true).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(favorite_ids(&a), [2, 3]);
    assert_eq!(favorite_ids(&b), [2, 3]);
}

#[test]
fn a_trashed_page_is_hidden_and_comes_back_with_a_restore() {
    let mut doc = with_pages(3);
    for n in [1, 2, 3] {
        doc.set_favorite(pid(n), true).unwrap();
    }
    doc.trash_page(pid(2), 100).unwrap();
    assert_eq!(favorite_ids(&doc), [1, 3], "hidden while in the trash");
    assert!(!doc.is_favorite(pid(2)));
    assert!(
        doc.to_portable()["favorites"]
            .get(pid(2).to_string())
            .is_some(),
        "the entry is kept"
    );
    doc.restore_page(pid(2), 200).unwrap();
    assert_eq!(favorite_ids(&doc), [1, 2, 3], "back in its place");
    assert_healthy(&doc);
}

#[test]
fn a_favourite_inside_a_trashed_subtree_is_hidden_too() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Parent");
    add(&mut doc, 2, Some(1), "Child");
    add(&mut doc, 3, None, "Other");
    doc.set_favorite(pid(2), true).unwrap();
    doc.set_favorite(pid(3), true).unwrap();
    doc.trash_page(pid(1), 100).unwrap();
    assert_eq!(favorite_ids(&doc), [3]);
    doc.restore_page(pid(1), 200).unwrap();
    assert_eq!(favorite_ids(&doc), [2, 3]);
}

#[test]
fn purging_a_page_removes_its_favourite_and_its_descendants() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Parent");
    add(&mut doc, 2, Some(1), "Child");
    add(&mut doc, 3, None, "Other");
    for n in [1, 2, 3] {
        doc.set_favorite(pid(n), true).unwrap();
    }
    doc.trash_page(pid(1), 100).unwrap();
    doc.purge_page(pid(1), 200).unwrap();
    assert_eq!(favorite_ids(&doc), [3]);
    let portable = doc.to_portable();
    let entries = portable["favorites"].as_object().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "the entries of the purged subtree are gone"
    );
    assert!(entries.contains_key(&pid(3).to_string()));
    assert_healthy(&doc);

    doc.trash_page(pid(3), 300).unwrap();
    doc.purge_page(pid(3), 301).unwrap();
    assert!(doc.favorites().is_empty());
    assert!(doc.to_portable().get("favorites").is_none());
}

#[test]
fn a_purge_reaches_the_other_replicas_with_its_favourite() {
    let mut a = with_pages(2);
    a.set_favorite(pid(1), true).unwrap();
    a.set_favorite(pid(2), true).unwrap();
    let mut b = replica(&a, 2);
    a.trash_page(pid(1), 100).unwrap();
    a.purge_page(pid(1), 200).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(favorite_ids(&b), [2]);
    assert!(b.page(pid(1)).is_none());
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&b);
}

#[test]
fn undo_and_redo_of_a_purge_never_touch_the_favourites() {
    let mut doc = with_pages(2);
    doc.set_favorite(pid(1), true).unwrap();
    doc.set_favorite(pid(2), true).unwrap();
    doc.purge_page(pid(1), 1_000).unwrap();
    assert_eq!(favorite_ids(&doc), [2]);
    // Undoing the purge brings the page back, not its favourite.
    doc.undo().unwrap().unwrap();
    assert!(doc.page(pid(1)).is_some());
    assert_eq!(favorite_ids(&doc), [2]);
    // The page can be favourited again (last); redoing the purge makes the entry name no page,
    // so it is ignored on read.
    doc.set_favorite(pid(1), true).unwrap();
    assert_eq!(favorite_ids(&doc), [2, 1]);
    doc.redo().unwrap().unwrap();
    assert!(doc.page(pid(1)).is_none());
    assert_eq!(favorite_ids(&doc), [2]);
    assert_healthy(&doc);
}

#[test]
fn an_entry_that_names_no_page_is_ignored_on_read() {
    let mut doc = with_pages(2);
    doc.set_favorite(pid(1), true).unwrap();
    // A favourite of a page this replica does not know (it may arrive later, or never).
    doc.favorites.insert(&pid(9).to_string(), "k").unwrap();
    doc.doc().inner().commit();
    assert_eq!(favorite_ids(&doc), [1]);
    assert!(!doc.is_favorite(pid(9)));
    assert!(
        doc.check_invariants().is_empty(),
        "an unknown page is not a problem"
    );
    // The page appears (a merge brought it, say): the favourite shows.
    add(&mut doc, 9, None, "Late");
    assert_eq!(favorite_ids(&doc), [1, 9]);
    // Moving and adding work around entries of unknown pages.
    doc.favorites.insert(&pid(8).to_string(), "zz").unwrap();
    doc.doc().inner().commit();
    doc.set_favorite(pid(2), true).unwrap();
    assert_eq!(favorite_ids(&doc), [1, 9, 2]);
}

#[test]
fn damaged_entries_are_reported_and_a_damaged_last_position_is_renumbered() {
    let mut doc = with_pages(3);
    doc.set_favorite(pid(1), true).unwrap();
    doc.favorites
        .insert(&pid(2).to_string(), "not base 62!")
        .unwrap();
    doc.favorites.insert("not a page id", "V").unwrap();
    doc.favorites.insert(&pid(3).to_string(), 7_i64).unwrap();
    doc.doc().inner().commit();
    let problems = doc.check_invariants();
    assert_eq!(
        problems.len(),
        3,
        "a malformed key, an unreadable position, an integer value: {problems:?}"
    );
    // Entries with a string position still list (ordered as strings); the integer one does not.
    assert!(doc.is_favorite(pid(2)));
    assert!(!doc.is_favorite(pid(3)));
    // Adding a page after a position that is not a base-62 string renumbers the visible ones.
    doc.set_favorite(pid(3), true).unwrap();
    assert!(doc.is_favorite(pid(3)));
    let order = favorite_ids(&doc);
    assert_eq!(order.len(), 3);
    assert_eq!(*order.last().unwrap(), 3, "the new favourite is last");
}

#[test]
fn favourites_are_not_on_the_undo_stack() {
    let mut doc = with_pages(2);
    // Starting from a clean stack: only favourite writes, so there is nothing to undo.
    let mut fresh = structure(1);
    // (A page creation is an undo step; make it, then drop the step by reloading.)
    add(&mut fresh, 1, None, "A");
    let mut loaded = replica(&fresh, 5);
    assert!(!loaded.can_undo());
    loaded.set_favorite(pid(1), true).unwrap();
    loaded.move_favorite(pid(1), None).unwrap();
    loaded.set_favorite(pid(1), false).unwrap();
    loaded.set_favorite(pid(1), true).unwrap();
    assert!(!loaded.can_undo(), "favourite writes are not undo steps");
    assert!(loaded.undo().unwrap().is_none());
    assert_eq!(favorite_ids(&loaded), [1]);

    // Undo and redo of structural edits leave favourites alone.
    doc.set_favorite(pid(1), true).unwrap();
    add(&mut doc, 3, None, "P3");
    doc.set_favorite(pid(3), true).unwrap();
    doc.set_favorite(pid(2), true).unwrap();
    assert_eq!(favorite_ids(&doc), [1, 3, 2]);
    doc.undo().unwrap().unwrap();
    assert!(doc.page(pid(3)).is_none(), "the page creation was undone");
    assert_eq!(
        favorite_ids(&doc),
        [1, 2],
        "the entry of the vanished page is ignored, the others stay"
    );
    doc.redo().unwrap().unwrap();
    assert!(doc.page(pid(3)).is_some());
    assert_eq!(favorite_ids(&doc), [1, 3, 2], "redo changed no favourite");
    assert_healthy(&doc);
}

#[test]
fn a_favourite_write_between_two_edits_does_not_break_the_undo_stack() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    doc.set_favorite(pid(1), true).unwrap();
    add(&mut doc, 2, None, "B");
    doc.set_favorite(pid(2), true).unwrap();
    doc.move_favorite(pid(2), Some(pid(1))).unwrap();
    assert_eq!(favorite_ids(&doc), [2, 1]);
    // Two creations: two undo steps, whatever was written in between.
    doc.undo().unwrap().unwrap();
    assert!(doc.page(pid(2)).is_none());
    doc.undo().unwrap().unwrap();
    assert!(doc.page(pid(1)).is_none());
    assert!(!doc.can_undo());
}

#[test]
fn the_portable_output_lists_the_map_only_when_it_has_entries() {
    let mut doc = with_pages(2);
    doc.set_favorite(pid(2), true).unwrap();
    doc.set_favorite(pid(1), true).unwrap();
    let portable = doc.to_portable();
    let favorites = portable["favorites"].as_object().unwrap();
    assert_eq!(favorites.len(), 2);
    let first = favorites[&pid(2).to_string()].as_str().unwrap();
    let second = favorites[&pid(1).to_string()].as_str().unwrap();
    assert!(
        first < second,
        "positions follow the order: {first} < {second}"
    );
    assert!(
        portable.as_object().unwrap().keys().eq([
            "favorites",
            "format",
            "pages",
            "space",
            "tombstones",
            "trash"
        ]),
        "sorted keys, `favorites` before `format`"
    );
    // The two replicas export the same.
    assert_eq!(replica(&doc, 2).to_portable(), portable);
    // The other keys are those of a space without favourites.
    doc.set_favorite(pid(1), false).unwrap();
    doc.set_favorite(pid(2), false).unwrap();
    assert!(doc.to_portable().get("favorites").is_none());
    assert_eq!(
        doc.to_portable()["format"],
        json!(1),
        "the format number did not change"
    );
}
