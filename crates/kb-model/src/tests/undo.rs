use std::collections::HashSet;

use super::{add, assert_healthy, outline, pid, replica, send, structure, title, trash_titles};
use crate::{StructureDoc, UNDO_MERGE_WINDOW_MS};

/// One scripted edit of a document.
type Edit = Box<dyn Fn(&mut StructureDoc)>;

#[test]
fn undo_and_redo_walk_back_and_forth_through_every_operation() {
    let mut doc = structure(1);
    let mut states = vec![doc.to_portable()];
    let a = pid(1);
    let b = pid(2);
    let c = pid(3);
    let steps: Vec<Edit> = vec![
        Box::new(move |d| {
            d.create_page(a, None, None, "A", 10).unwrap();
        }),
        Box::new(move |d| {
            d.create_page(b, None, None, "B", 20).unwrap();
        }),
        Box::new(move |d| {
            d.create_page(c, Some(b), None, "C", 30).unwrap();
        }),
        Box::new(move |d| {
            d.rename_page(a, "A2", 1_000).unwrap();
        }),
        Box::new(move |d| {
            d.set_page_icon(a, Some("⭐"), 2_000).unwrap();
        }),
        Box::new(move |d| {
            d.move_page(b, None, 0, 3_000).unwrap();
        }),
        Box::new(move |d| {
            d.move_page(a, Some(c), 0, 4_000).unwrap();
        }),
        Box::new(move |d| {
            d.trash_page(b, 5_000).unwrap();
        }),
        Box::new(move |d| {
            d.restore_page(b, 6_000).unwrap();
        }),
        Box::new(move |d| {
            d.purge_page(b, 7_000).unwrap();
        }),
    ];
    for step in &steps {
        step(&mut doc);
        assert_healthy(&doc);
        states.push(doc.to_portable());
    }
    assert_eq!(outline(&doc), "");
    assert_eq!(doc.tombstones().len(), 3);

    for k in (0..steps.len()).rev() {
        let ids = doc.undo().unwrap().expect("something to undo");
        assert_eq!(doc.to_portable(), states[k], "after undoing step {}", k + 1);
        assert_healthy(&doc);
        let listed: HashSet<_> = ids.into_iter().collect();
        for id in doc.all_page_ids() {
            assert!(listed.contains(&id), "undo reports every page");
        }
    }
    assert!(!doc.can_undo());
    assert!(doc.undo().unwrap().is_none());
    assert!(doc.page(a).is_none(), "the index follows the undo");

    for (k, state) in states.iter().enumerate().skip(1) {
        assert!(doc.redo().unwrap().is_some());
        assert_eq!(&doc.to_portable(), state, "after redoing step {k}");
        assert_healthy(&doc);
    }
    assert!(!doc.can_redo());
    assert!(doc.redo().unwrap().is_none());
}

#[test]
fn undo_of_a_purge_brings_back_pages_and_removes_tombstones() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, None, "B");
    doc.purge_page(pid(1), 100).unwrap();
    assert_eq!(doc.tombstones().len(), 2);
    let ids = doc.undo().unwrap().unwrap();
    assert!(ids.contains(&pid(1)) && ids.contains(&pid(2)) && ids.contains(&pid(3)));
    assert!(doc.tombstones().is_empty());
    assert_eq!(outline(&doc), "A\n  A1\nB\n");
    assert_eq!(doc.page(pid(2)).unwrap().parent_id, Some(pid(1)));
    // The restored pages are fully usable.
    doc.rename_page(pid(2), "A1 again", 200).unwrap();
    doc.trash_page(pid(1), 300).unwrap();
    assert_eq!(trash_titles(&doc), ["A"]);
    assert_healthy(&doc);
}

#[test]
fn nothing_to_undo_is_none_and_setup_is_never_undone() {
    let mut doc = structure(1);
    assert!(!doc.can_undo() && !doc.can_redo());
    assert!(doc.undo().unwrap().is_none());
    assert!(doc.redo().unwrap().is_none());

    add(&mut doc, 1, None, "A");
    assert!(doc.can_undo());
    assert!(doc.undo().unwrap().is_some());
    assert!(!doc.can_undo() && doc.can_redo());
    assert!(
        doc.undo().unwrap().is_none(),
        "the space and trash root stay"
    );
    assert_eq!(doc.space().name, "Personal");
    assert_healthy(&doc);

    // An undone creation frees nothing for reuse but does not block a new page.
    doc.create_page(pid(1), None, None, "A again", 50).unwrap();
    assert!(!doc.can_redo(), "a new edit clears the redo stack");
    assert_eq!(title(&doc, pid(1)), "A again");
    assert_healthy(&doc);

    // A reloaded document starts with empty stacks.
    let reloaded = replica(&doc, 2);
    assert!(!reloaded.can_undo() && !reloaded.can_redo());
}

#[test]
fn rapid_edits_of_one_kind_on_one_page_merge() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, None, "B");
    let w = UNDO_MERGE_WINDOW_MS;

    doc.rename_page(pid(1), "a1", 1_000).unwrap();
    doc.rename_page(pid(1), "a2", 1_100).unwrap();
    doc.rename_page(pid(1), "a3", 1_000 + w - 1).unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "A", "one step for the burst");
    doc.redo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "a3");

    // The window counts from the first edit of the step.
    doc.rename_page(pid(1), "b1", 10_000).unwrap();
    doc.rename_page(pid(1), "b2", 10_000 + w).unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "b1");
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "a3");

    // Rapid moves of one page merge too.
    doc.move_page(pid(1), None, 1, 20_000).unwrap();
    doc.move_page(pid(1), None, 0, 20_010).unwrap();
    doc.move_page(pid(1), None, 1, 20_020).unwrap();
    assert_eq!(outline(&doc), "B\na3\n");
    doc.undo().unwrap().unwrap();
    assert_eq!(outline(&doc), "a3\nB\n");
}

#[test]
fn edits_of_other_kinds_or_pages_stay_separate() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, None, "B");

    // Same instant, different kinds: create then rename (kb-core relies on this).
    doc.create_page(pid(3), None, None, "C", 500).unwrap();
    doc.rename_page(pid(3), "C2", 500).unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(3)), "C");
    doc.undo().unwrap().unwrap();
    assert!(doc.page(pid(3)).is_none());

    doc.rename_page(pid(1), "A2", 1_000).unwrap();
    doc.set_page_icon(pid(1), Some("⭐"), 1_000).unwrap();
    doc.rename_page(pid(2), "B2", 1_000).unwrap();
    doc.rename_page(pid(1), "A3", 1_000).unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "A2");
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(2)), "B");
    doc.undo().unwrap().unwrap();
    assert_eq!(doc.page(pid(1)).unwrap().icon, None);
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "A");

    // A clock going backwards never merges.
    doc.rename_page(pid(1), "x", 5_000).unwrap();
    doc.rename_page(pid(1), "y", 4_999).unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "x");
}

#[test]
fn undo_and_refresh_close_the_merge_window() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    doc.rename_page(pid(1), "a1", 1_000).unwrap();
    doc.undo().unwrap().unwrap();
    doc.rename_page(pid(1), "b1", 1_050).unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "A");
    assert!(doc.can_undo(), "the creation is still a separate step");

    let mut other = replica(&doc, 2);
    doc.rename_page(pid(1), "c1", 2_000).unwrap();
    add(&mut other, 9, None, "Remote");
    send(&other, &mut doc);
    doc.rename_page(pid(1), "c2", 2_050).unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(title(&doc, pid(1)), "c1");
}

#[test]
fn remote_changes_are_never_undone() {
    let mut local = structure(1);
    add(&mut local, 1, None, "X");
    let mut remote = replica(&local, 2);
    add(&mut remote, 2, None, "Z");
    remote.rename_page(pid(1), "X by remote", 3_000).unwrap();
    send(&remote, &mut local);
    assert_eq!(title(&local, pid(1)), "X by remote");
    local.rename_page(pid(1), "X2", 5_000).unwrap();

    local.undo().unwrap().unwrap();
    assert_eq!(
        title(&local, pid(1)),
        "X by remote",
        "back to the remote value"
    );
    assert_eq!(title(&local, pid(2)), "Z");
    local.undo().unwrap().unwrap();
    assert!(local.page(pid(1)).is_none(), "own creation undone");
    assert_eq!(title(&local, pid(2)), "Z", "remote page untouched");
    assert!(
        local.undo().unwrap().is_none(),
        "nothing of the remote peer to undo"
    );
    assert_healthy(&local);
    local.redo().unwrap().unwrap();
    assert!(local.page(pid(1)).is_some());
}
