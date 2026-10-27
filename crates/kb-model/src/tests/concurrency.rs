use kb_doc::{Doc, DocKind};
use loro::TreeParentId;

use super::{
    add, assert_healthy, outline, pid, replica, space_meta, structure, sync, title, trash_titles,
};
use crate::{ModelError, StructureDoc};

fn assert_converged(a: &StructureDoc, b: &StructureDoc) {
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(a);
    assert_healthy(b);
}

#[test]
fn a_purge_wins_over_a_concurrent_move_that_revives_the_node() {
    let mut a = structure(1);
    add(&mut a, 1, None, "X");
    add(&mut a, 2, None, "Y");
    add(&mut a, 3, Some(1), "X1");
    let mut b = replica(&a, 2);
    let x_node = a.index.node(pid(1)).unwrap();

    // B gets a higher lamport clock, so its move is applied after A's delete.
    for i in 0..10 {
        b.rename_page(pid(2), &format!("Y{i}"), 100 + i).unwrap();
    }
    a.purge_page(pid(1), 500).unwrap();
    b.move_page(pid(1), Some(pid(2)), 0, 600).unwrap();
    sync(&mut a, &mut b);

    let tree = a.doc().inner().get_tree("pages");
    assert!(
        !tree.is_node_deleted(&x_node).unwrap(),
        "precondition: Loro revived the purged node"
    );
    assert_eq!(
        tree.parent(x_node),
        Some(TreeParentId::Node(a.index.node(pid(2)).unwrap()))
    );
    for doc in [&mut a, &mut b] {
        assert!(doc.page(pid(1)).is_none(), "the tombstone wins");
        assert!(doc.page(pid(3)).is_none(), "with the purged subtree");
        assert_eq!(outline(doc), "Y9\n");
        assert_eq!(doc.tombstones().len(), 2);
        assert!(matches!(
            doc.move_page(pid(1), None, 0, 700),
            Err(ModelError::PageNotFound)
        ));
    }
    assert_converged(&a, &b);
}

#[test]
fn crossing_moves_converge_without_a_cycle() {
    let mut a = structure(1);
    add(&mut a, 1, None, "X");
    add(&mut a, 2, None, "Y");
    let mut b = replica(&a, 2);
    a.move_page(pid(1), Some(pid(2)), 0, 10).unwrap();
    b.move_page(pid(2), Some(pid(1)), 0, 10).unwrap();
    sync(&mut a, &mut b);
    let shape = outline(&a);
    assert!(shape == "X\n  Y\n" || shape == "Y\n  X\n", "{shape}");
    assert_eq!(a.live_page_count(), 2);
    assert_converged(&a, &b);
}

#[test]
fn a_child_created_under_a_concurrently_trashed_page_is_trashed_with_it() {
    let mut a = structure(1);
    add(&mut a, 1, None, "X");
    let mut b = replica(&a, 2);
    a.trash_page(pid(1), 10).unwrap();
    add(&mut b, 2, Some(1), "X1");
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert_eq!(outline(doc), "");
        assert_eq!(trash_titles(doc), ["X"]);
        assert_eq!(doc.trashed_pages()[0].descendant_count, 1);
        let child = doc.page(pid(2)).unwrap();
        assert_eq!(
            (child.parent_id, child.trashed_at),
            (Some(pid(1)), Some(10))
        );
        assert_eq!(doc.live_page_count(), 0);
    }
    assert_converged(&a, &b);
}

#[test]
fn a_child_created_under_a_concurrently_purged_page_disappears() {
    let mut a = structure(1);
    add(&mut a, 1, None, "X");
    let mut b = replica(&a, 2);
    a.purge_page(pid(1), 10).unwrap();
    add(&mut b, 2, Some(1), "X1");
    sync(&mut a, &mut b);
    for doc in [&a, &b] {
        assert!(doc.page(pid(2)).is_none());
        assert_eq!(doc.all_page_ids(), vec![]);
        // Its id was never purged, but it is gone with its deleted parent.
        assert_eq!(doc.tombstones().len(), 1);
    }
    assert_converged(&a, &b);
}

#[test]
fn a_restore_under_a_concurrently_trashed_parent_stays_in_the_trash() {
    let mut a = structure(1);
    add(&mut a, 1, None, "P");
    add(&mut a, 2, Some(1), "X");
    a.trash_page(pid(2), 10).unwrap();
    let mut b = replica(&a, 2);
    a.restore_page(pid(2), 20).unwrap();
    b.trash_page(pid(1), 21).unwrap();
    sync(&mut a, &mut b);
    for doc in [&mut a, &mut b] {
        assert_eq!(outline(doc), "");
        assert_eq!(trash_titles(doc), ["P"]);
        let x = doc.page(pid(2)).unwrap();
        assert_eq!((x.parent_id, x.trashed_at), (Some(pid(1)), Some(21)));
        assert!(matches!(
            doc.restore_page(pid(2), 30),
            Err(ModelError::PageNotTrashed)
        ));
    }
    assert_converged(&a, &b);
}

#[test]
fn concurrent_renames_converge_to_one_title() {
    let mut a = structure(1);
    add(&mut a, 1, None, "X");
    let mut b = replica(&a, 2);
    a.rename_page(pid(1), "from A", 10).unwrap();
    b.rename_page(pid(1), "from B", 10).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(title(&a, pid(1)), title(&b, pid(1)));
    assert_converged(&a, &b);
}

#[test]
fn two_trash_roots_after_a_merge_both_count_as_trash() {
    // Two devices initialised the same space independently.
    let mut a = StructureDoc::new(Doc::new(DocKind::Structure, 1), space_meta());
    let mut b = StructureDoc::new(Doc::new(DocKind::Structure, 2), space_meta());
    add(&mut a, 1, None, "A");
    add(&mut b, 2, None, "B");
    a.trash_page(pid(1), 10).unwrap();
    b.trash_page(pid(2), 20).unwrap();
    sync(&mut a, &mut b);

    for doc in [&a, &b] {
        assert_eq!(doc.index.trash_roots.len(), 2);
        assert_eq!(trash_titles(doc), ["B", "A"]);
        assert_eq!(doc.trashed_page_count(), 2);
        assert_eq!(outline(doc), "");
    }
    assert_converged(&a, &b);

    // New trash goes to one of them; restores work from either.
    add(&mut a, 3, None, "C");
    a.trash_page(pid(3), 30).unwrap();
    a.restore_page(pid(2), 40).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(trash_titles(&b), ["C", "A"]);
    assert_eq!(outline(&b), "B\n");
    assert_converged(&a, &b);
}
