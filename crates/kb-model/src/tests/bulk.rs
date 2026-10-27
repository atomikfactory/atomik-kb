//! `StructureDoc::create_pages`, the bulk creation of the import (ADR-0036): positions, metadata,
//! refusals that write nothing, and the commit origin that keeps an import off the undo stack.

use kb_types::{DeviceId, PageId};

use super::{add, assert_healthy, outline, pid, structure, title};
use crate::{IMPORT_ORIGIN, ModelError, NewPage, StructureDoc};

fn device() -> DeviceId {
    DeviceId::from_random_bytes([0xf0, 9, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 9])
}

fn new_page(n: u8, parent: Option<u8>, title: &str) -> NewPage {
    NewPage {
        page_id: pid(n),
        parent: parent.map(pid),
        title: title.into(),
        icon: None,
        cover: None,
        cover_y: None,
    }
}

/// A document with A (with A1) and B at the root, B in the trash, and a purged page.
fn base() -> StructureDoc {
    let mut doc = structure(1);
    doc.set_author(Some(device()));
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, None, "B");
    add(&mut doc, 3, Some(1), "A1");
    add(&mut doc, 4, None, "Gone");
    doc.trash_page(pid(2), 50).unwrap();
    doc.purge_page(pid(4), 60).unwrap();
    doc
}

fn batch() -> Vec<NewPage> {
    vec![
        NewPage {
            icon: Some("⭐".into()),
            ..new_page(10, None, "N1")
        },
        new_page(11, Some(10), "N2"),
        new_page(12, Some(1), "N3"),
        new_page(13, None, "N4"),
        new_page(14, Some(10), "N5"),
        new_page(15, Some(11), "N6"),
    ]
}

#[test]
fn pages_are_appended_in_order_exactly_as_create_page_would_have() {
    let mut bulk = base();
    let created = bulk.create_pages(&batch(), 100).unwrap();
    assert_eq!(created, (10..=15).map(pid).collect::<Vec<_>>());
    assert_healthy(&bulk);

    // The same pages made one by one.
    let mut single = base();
    for page in batch() {
        single
            .create_page(page.page_id, page.parent, None, &page.title, 100)
            .unwrap();
        if let Some(icon) = page.icon.as_deref() {
            single.set_page_icon(page.page_id, Some(icon), 100).unwrap();
        }
    }
    let expected = "A\n  A1\n  N3\nN1\n  N2\n    N6\n  N5\nN4\n";
    assert_eq!(outline(&bulk), expected);
    assert_eq!(outline(&single), expected);

    // The metadata is what `create_page` sets: id, title, creation and edit times, the icon.
    for id in (10..=15).map(pid) {
        assert_eq!(bulk.page(id), single.page(id), "{id}");
    }
    let n1 = bulk.page(pid(10)).unwrap();
    assert_eq!(
        (
            n1.title.as_str(),
            n1.icon.as_deref(),
            n1.created_at,
            n1.updated_at
        ),
        ("N1", Some("⭐"), 100, 100)
    );
    assert_eq!(n1.parent_id, None);
    assert_eq!(bulk.page(pid(15)).unwrap().parent_id, Some(pid(11)));
    // The creating device too (`to_portable` lists it with the page).
    assert_eq!(bulk.to_portable(), single.to_portable());
    // The index follows: every page is found, the subtree walks the batch.
    assert_eq!(
        bulk.subtree(pid(10)).unwrap(),
        [10, 11, 15, 14].map(pid).to_vec()
    );
    assert_eq!(bulk.live_page_count(), 8);
}

#[test]
fn a_big_flat_batch_keeps_its_order() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "First");
    let ids: Vec<PageId> = (0..1_000u32)
        .map(|n| {
            let b = n.to_le_bytes();
            PageId::from_random_bytes([b[0], b[1], 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7])
        })
        .collect();
    let pages: Vec<NewPage> = ids
        .iter()
        .enumerate()
        .map(|(n, id)| NewPage {
            page_id: *id,
            parent: None,
            title: format!("Row {n}"),
            icon: None,
            cover: None,
            cover_y: None,
        })
        .collect();
    doc.create_pages(&pages, 10).unwrap();
    assert_healthy(&doc);
    let titles: Vec<String> = doc.live_tree().into_iter().map(|n| n.page.title).collect();
    assert_eq!(titles.len(), 1_001);
    assert_eq!(titles[0], "First");
    assert_eq!(titles[1], "Row 0");
    assert_eq!(titles[1_000], "Row 999");
    // A second batch goes after the first.
    let more = NewPage {
        page_id: pid(2),
        parent: None,
        title: "After".into(),
        icon: None,
        cover: None,
        cover_y: None,
    };
    doc.create_pages(&[more], 11).unwrap();
    assert_eq!(doc.live_tree().last().unwrap().page.title, "After");
}

#[test]
fn a_refusal_writes_nothing() {
    let mut doc = base();
    let portable = doc.to_portable();
    let version = doc.doc().version();
    let can_undo = doc.can_undo();

    let refusals: Vec<(&str, Vec<NewPage>, bool)> = vec![
        (
            "an id twice in the batch",
            vec![new_page(10, None, "X"), new_page(10, None, "Y")],
            true,
        ),
        (
            "an id that exists",
            vec![new_page(10, None, "X"), new_page(1, None, "A again")],
            true,
        ),
        (
            "an id in the trash",
            vec![new_page(10, None, "X"), new_page(2, None, "B again")],
            true,
        ),
        (
            "an id that was purged",
            vec![new_page(10, None, "X"), new_page(4, None, "Gone again")],
            true,
        ),
        (
            "a parent that comes later",
            vec![new_page(10, Some(11), "X"), new_page(11, None, "Y")],
            false,
        ),
        (
            "a page that is its own parent",
            vec![new_page(10, Some(10), "X")],
            false,
        ),
        (
            "a parent that does not exist",
            vec![new_page(10, None, "X"), new_page(11, Some(99), "Y")],
            false,
        ),
        (
            "a parent in the trash",
            vec![new_page(10, None, "X"), new_page(11, Some(2), "Y")],
            false,
        ),
        (
            "a parent that was purged",
            vec![new_page(10, None, "X"), new_page(11, Some(4), "Y")],
            false,
        ),
    ];
    for (what, pages, duplicate) in refusals {
        let err = doc.create_pages(&pages, 100).unwrap_err();
        if duplicate {
            assert!(matches!(err, ModelError::DuplicatePage), "{what}: {err:?}");
        } else {
            assert!(
                matches!(err, ModelError::ParentUnavailable),
                "{what}: {err:?}"
            );
        }
        assert_eq!(doc.to_portable(), portable, "{what}: nothing was written");
        assert_eq!(doc.doc().version(), version, "{what}: no operation either");
        assert_eq!(doc.can_undo(), can_undo, "{what}");
        assert!(doc.page(pid(10)).is_none(), "{what}");
        assert_healthy(&doc);
    }

    // An empty batch is nothing.
    assert!(doc.create_pages(&[], 100).unwrap().is_empty());
    assert_eq!(doc.doc().version(), version);
}

#[test]
fn an_import_is_not_an_undo_step_and_pushes_none_off_the_stack() {
    fn imported_id(n: u32) -> PageId {
        let b = n.to_le_bytes();
        PageId::from_random_bytes([b[0], b[1], 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9])
    }
    let mut doc = structure(1);
    // Three steps of the person's own, then three hundred imported pages (three times the depth
    // of the undo stack): the odd ones under the page before them in the batch.
    add(&mut doc, 1, None, "Keep");
    doc.rename_page(pid(1), "Renamed", 1_000).unwrap();
    add(&mut doc, 2, None, "Second");
    let pages: Vec<NewPage> = (0..300u32)
        .map(|n| NewPage {
            page_id: imported_id(n),
            parent: (n % 2 == 1).then(|| imported_id(n - 1)),
            title: format!("Imported {n}"),
            icon: None,
            cover: None,
            cover_y: None,
        })
        .collect();
    doc.create_pages(&pages, 2_000).unwrap();
    let imported = |doc: &StructureDoc| {
        doc.all_page_ids()
            .into_iter()
            .filter(|id| {
                doc.page(*id)
                    .is_some_and(|p| p.title.starts_with("Imported"))
            })
            .count()
    };
    assert_eq!(imported(&doc), 300);
    assert!(doc.can_undo());

    // Undo walks back through the person's three steps, and the imported pages stay in place.
    doc.undo().unwrap().expect("the second page");
    assert!(
        doc.page(pid(2)).is_none(),
        "the creation of Second is undone"
    );
    assert_eq!(title(&doc, pid(1)), "Renamed");
    assert_eq!(imported(&doc), 300);
    doc.undo().unwrap().expect("the rename");
    assert_eq!(title(&doc, pid(1)), "Keep");
    doc.undo().unwrap().expect("the first page");
    assert!(doc.page(pid(1)).is_none());
    assert!(!doc.can_undo(), "nothing else was recorded");
    assert_eq!(imported(&doc), 300, "the imported pages are still there");
    assert_eq!(
        doc.page(imported_id(1)).unwrap().parent_id,
        Some(imported_id(0))
    );
    assert_healthy(&doc);

    // And redo brings the person's steps back, not the import's.
    doc.redo().unwrap().expect("the first page");
    doc.redo().unwrap().expect("the rename");
    doc.redo().unwrap().expect("the second page");
    assert!(!doc.can_redo());
    assert_eq!(title(&doc, pid(1)), "Renamed");
    assert_eq!(imported(&doc), 300);
    assert_healthy(&doc);
}

#[test]
fn the_origin_is_part_of_the_public_surface() {
    assert_eq!(IMPORT_ORIGIN, "kb-model/import");
}
