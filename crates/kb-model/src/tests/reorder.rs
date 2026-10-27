//! Reordering the children of a parent in one step (ADR-0052).

use kb_types::PageId;

use super::{add, assert_healthy, outline, pid, replica, send, structure, sync, title};
use crate::reorder::longest_increasing_run;
use crate::{ModelError, StructureDoc};

/// Pages A, B, C, D, E (ids 1 to 5) at the root, in that order.
fn five() -> StructureDoc {
    let mut doc = structure(1);
    for (n, t) in [(1u8, "A"), (2, "B"), (3, "C"), (4, "D"), (5, "E")] {
        add(&mut doc, n, None, t);
    }
    doc
}

fn ids(list: &[u8]) -> Vec<PageId> {
    list.iter().map(|n| pid(*n)).collect()
}

#[test]
fn the_children_take_the_given_order_and_the_moved_pages_are_reported() {
    let mut doc = five();
    let moved = doc.reorder_children(None, &ids(&[5, 4, 3, 2, 1])).unwrap();
    assert_eq!(outline(&doc), "E\nD\nC\nB\nA\n");
    assert_eq!(moved.len(), 4, "the longest run that already agrees stays");
    assert_healthy(&doc);

    // Under a page, with subtrees that travel along.
    add(&mut doc, 6, Some(1), "A1");
    add(&mut doc, 7, Some(1), "A2");
    add(&mut doc, 8, Some(1), "A3");
    add(&mut doc, 9, Some(7), "A2a");
    doc.reorder_children(Some(pid(1)), &ids(&[8, 7, 6]))
        .unwrap();
    assert_eq!(outline(&doc), "E\nD\nC\nB\nA\n  A3\n  A2\n    A2a\n  A1\n");
    assert_eq!(doc.page(pid(9)).unwrap().parent_id, Some(pid(7)));
    assert_healthy(&doc);
}

#[test]
fn a_page_that_must_jump_is_the_only_one_that_moves() {
    let mut doc = five();
    // Rotate the first to the end: one move, not four.
    let moved = doc.reorder_children(None, &ids(&[2, 3, 4, 5, 1])).unwrap();
    assert_eq!(moved, vec![pid(1)]);
    assert_eq!(outline(&doc), "B\nC\nD\nE\nA\n");
    // And the last to the front.
    let moved = doc.reorder_children(None, &ids(&[1, 2, 3, 4, 5])).unwrap();
    assert_eq!(moved, vec![pid(1)]);
    assert_eq!(outline(&doc), "A\nB\nC\nD\nE\n");
    // An interleaved case keeps the longest agreeing run in place.
    let moved = doc.reorder_children(None, &ids(&[2, 1, 4, 3, 5])).unwrap();
    assert_eq!(moved.len(), 2);
    assert_eq!(outline(&doc), "B\nA\nD\nC\nE\n");
    assert_healthy(&doc);
}

#[test]
fn already_in_that_order_writes_nothing() {
    let mut doc = five();
    let version = doc.doc().version();
    let undo_before = doc.can_undo();
    assert!(
        doc.reorder_children(None, &ids(&[1, 2, 3, 4, 5]))
            .unwrap()
            .is_empty()
    );
    assert_eq!(doc.doc().version(), version, "no commit");
    assert_eq!(doc.can_undo(), undo_before);
    assert!(!doc.can_redo());

    // Zero and one child are in order too.
    add(&mut doc, 6, Some(1), "A1");
    let version = doc.doc().version();
    assert!(
        doc.reorder_children(Some(pid(1)), &ids(&[6]))
            .unwrap()
            .is_empty()
    );
    assert!(doc.reorder_children(Some(pid(2)), &[]).unwrap().is_empty());
    assert_eq!(doc.doc().version(), version);
}

#[test]
fn a_reorder_is_one_undo_step_and_one_redo() {
    let mut doc = five();
    let before = doc.to_portable();
    doc.reorder_children(None, &ids(&[5, 4, 3, 2, 1])).unwrap();
    let after = doc.to_portable();
    assert_ne!(before, after);

    doc.undo().unwrap().expect("something to undo");
    assert_eq!(
        doc.to_portable(),
        before,
        "one undo puts every page back, and no page is lost"
    );
    assert_eq!(outline(&doc), "A\nB\nC\nD\nE\n");
    assert_healthy(&doc);

    doc.redo().unwrap().expect("something to redo");
    assert_eq!(doc.to_portable(), after);
    assert_eq!(outline(&doc), "E\nD\nC\nB\nA\n");
    assert!(!doc.can_redo());
    assert_healthy(&doc);
}

#[test]
fn a_reorder_does_not_merge_into_the_move_before_it() {
    let mut doc = five();
    // A move of page 1 opens an undo group; the reorder right after it is its own step.
    doc.move_page(pid(1), None, 2, 100).unwrap();
    assert_eq!(outline(&doc), "B\nC\nA\nD\nE\n");
    doc.reorder_children(None, &ids(&[1, 2, 3, 4, 5])).unwrap();
    assert_eq!(outline(&doc), "A\nB\nC\nD\nE\n");

    doc.undo().unwrap();
    assert_eq!(
        outline(&doc),
        "B\nC\nA\nD\nE\n",
        "only the reorder is undone"
    );
    doc.undo().unwrap();
    assert_eq!(outline(&doc), "A\nB\nC\nD\nE\n", "then the move");
    assert_healthy(&doc);
}

#[test]
fn no_page_is_stamped_as_edited() {
    let mut doc = five();
    // Edit one page long after creation so the stamps differ per page.
    doc.rename_page(pid(3), "C2", 9_000).unwrap();
    let stamps = |doc: &StructureDoc| -> Vec<(i64, Option<kb_types::DeviceId>)> {
        (1..=5)
            .map(|n| {
                let page = doc.page(pid(n)).unwrap();
                (page.updated_at, page.edited_by)
            })
            .collect()
    };
    let before = stamps(&doc);
    doc.reorder_children(None, &ids(&[5, 3, 1, 4, 2])).unwrap();
    assert_eq!(
        stamps(&doc),
        before,
        "updated_at and edited_by are left alone"
    );
    assert_eq!(doc.page(pid(3)).unwrap().updated_at, 9_000);
    assert_eq!(title(&doc, pid(3)), "C2");
}

#[test]
fn a_set_that_is_not_the_live_children_is_refused_and_nothing_is_written() {
    let mut doc = five();
    add(&mut doc, 6, Some(1), "A1");
    add(&mut doc, 7, Some(1), "A2");
    doc.trash_page(pid(5), 50).unwrap();
    let version = doc.doc().version();
    let outline_before = outline(&doc);

    let wrong: [(&str, Option<PageId>, Vec<PageId>); 7] = [
        ("a page missing", None, ids(&[4, 3, 2])),
        ("a page too many", None, ids(&[4, 3, 2, 1, 6])),
        ("a repeated page", None, ids(&[4, 3, 2, 2])),
        ("the trashed page", None, ids(&[4, 3, 2, 1, 5])),
        ("an unknown page", None, ids(&[4, 3, 2, 99])),
        ("children of another parent", Some(pid(1)), ids(&[7, 6, 2])),
        ("nothing at all", None, Vec::new()),
    ];
    for (what, parent, order) in wrong {
        assert!(
            matches!(
                doc.reorder_children(parent, &order),
                Err(ModelError::InvalidOrder)
            ),
            "{what}"
        );
    }
    assert_eq!(doc.doc().version(), version);
    assert_eq!(outline(&doc), outline_before);

    // The live children are enough: the trashed sibling is not one of them.
    doc.reorder_children(None, &ids(&[4, 3, 2, 1])).unwrap();
    assert_eq!(outline(&doc), "D\nC\nB\nA\n  A1\n  A2\n");
    assert_healthy(&doc);
}

#[test]
fn the_parent_must_be_a_live_page() {
    let mut doc = five();
    add(&mut doc, 6, Some(1), "A1");
    add(&mut doc, 7, Some(1), "A2");
    doc.trash_page(pid(1), 50).unwrap();
    assert!(matches!(
        doc.reorder_children(Some(pid(1)), &ids(&[7, 6])),
        Err(ModelError::ParentUnavailable)
    ));
    assert!(matches!(
        doc.reorder_children(Some(pid(99)), &[]),
        Err(ModelError::ParentUnavailable)
    ));
}

#[test]
fn a_reorder_keeps_the_trash_and_its_pages_out_of_the_way() {
    let mut doc = five();
    doc.trash_page(pid(2), 50).unwrap();
    doc.trash_page(pid(4), 51).unwrap();
    doc.reorder_children(None, &ids(&[5, 3, 1])).unwrap();
    assert_eq!(outline(&doc), "E\nC\nA\n");
    assert_eq!(doc.trashed_page_count(), 2);
    // Restoring brings the pages back after the live ones, as always.
    doc.restore_page(pid(2), 60).unwrap();
    assert_eq!(outline(&doc), "E\nC\nA\nB\n");
    assert_healthy(&doc);
}

#[test]
fn a_reorder_travels_to_another_replica_and_merges_with_a_concurrent_add() {
    let mut a = five();
    let mut b = replica(&a, 2);
    a.reorder_children(None, &ids(&[3, 1, 5, 2, 4])).unwrap();
    send(&a, &mut b);
    assert_eq!(outline(&b), "C\nA\nE\nB\nD\n");
    assert_healthy(&b);

    // A page added elsewhere meanwhile is not lost, and both replicas agree.
    b.create_page(pid(6), None, None, "F", 70).unwrap();
    a.reorder_children(None, &ids(&[5, 4, 3, 2, 1])).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(outline(&a), outline(&b));
    assert!(outline(&a).contains('F'));
    assert_eq!(a.live_page_count(), 6);
    assert_healthy(&a);
    assert_healthy(&b);
}

#[test]
fn longest_increasing_run_finds_a_longest_run() {
    assert!(longest_increasing_run(&[]).is_empty());
    assert_eq!(longest_increasing_run(&[0, 1, 2, 3]), vec![0, 1, 2, 3]);
    assert_eq!(longest_increasing_run(&[3, 2, 1, 0]).len(), 1);
    assert_eq!(longest_increasing_run(&[1, 2, 3, 4, 0]), vec![0, 1, 2, 3]);
    let run = longest_increasing_run(&[2, 5, 3, 7, 4, 9, 6]);
    assert_eq!(run.len(), 4);
    let values = [2usize, 5, 3, 7, 4, 9, 6];
    assert!(
        run.windows(2)
            .all(|w| w[0] < w[1] && values[w[0]] < values[w[1]])
    );
}

#[test]
fn a_reorder_of_many_children_ends_in_the_wanted_order() {
    let mut doc = structure(1);
    // 40 pages, shuffled by a fixed permutation (multiplication by 7 modulo 41).
    for n in 1..=40u8 {
        add(&mut doc, n, None, &format!("P{n:02}"));
    }
    let shuffled: Vec<u8> = (1..=40u32)
        .map(|n| u8::try_from((n * 7) % 41).unwrap())
        .collect();
    doc.reorder_children(None, &ids(&shuffled)).unwrap();
    let wanted: Vec<String> = shuffled.iter().map(|n| format!("P{n:02}")).collect();
    let live: Vec<String> = doc.live_tree().into_iter().map(|n| n.page.title).collect();
    assert_eq!(live, wanted);
    assert_healthy(&doc);
    // And a reorder back to creation order is one undo from the first.
    let sorted: Vec<u8> = (1..=40).collect();
    doc.reorder_children(None, &ids(&sorted)).unwrap();
    doc.undo().unwrap();
    let live: Vec<String> = doc.live_tree().into_iter().map(|n| n.page.title).collect();
    assert_eq!(live, wanted);
}
