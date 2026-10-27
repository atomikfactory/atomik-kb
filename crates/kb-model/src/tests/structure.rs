use std::time::{Duration, Instant};

use kb_doc::{Doc, DocKind};
use kb_types::PageInfo;
use loro::TreeParentId;
use serde_json::json;

use super::{
    add, assert_healthy, outline, pid, replica, sid, space_meta, structure, title, trash_titles,
};
use crate::{ModelError, SpaceMeta, StructureDoc};

#[test]
fn a_new_document_holds_the_space_and_an_empty_tree() {
    let doc = structure(1);
    assert_eq!(doc.space(), space_meta());
    assert!(doc.live_tree().is_empty());
    assert!(doc.trashed_pages().is_empty());
    assert_eq!(doc.live_page_count(), 0);
    assert_eq!(doc.trashed_page_count(), 0);
    assert!(doc.all_page_ids().is_empty());
    assert!(doc.tombstones().is_empty());
    assert!(!doc.can_undo() && !doc.can_redo());
    assert!(
        doc.doc().export_updates_since(None).unwrap().is_some(),
        "setup is persisted"
    );
    assert_healthy(&doc);
    assert_eq!(
        doc.to_portable(),
        json!({
            "format": 1,
            "pages": [],
            "space": {"createdAt": 5, "id": sid(1).to_string(), "name": "Personal"},
            "tombstones": {},
            "trash": []
        })
    );
}

#[test]
fn from_doc_rebuilds_everything_from_a_snapshot() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    let b = add(&mut doc, 3, None, "B");
    doc.trash_page(b, 50).unwrap();
    let loaded = replica(&doc, 2);
    assert_eq!(loaded.to_portable(), doc.to_portable());
    assert_eq!(outline(&loaded), "A\n  A1\n");
    assert_eq!(loaded.page(a), doc.page(a));
    assert_eq!(loaded.page(b).unwrap().trashed_at, Some(50));
    assert_eq!(loaded.space(), doc.space());
    assert_healthy(&loaded);
}

#[test]
fn from_doc_refuses_foreign_shapes() {
    assert!(matches!(
        StructureDoc::from_doc(Doc::new(DocKind::Account, 1)),
        Err(ModelError::Shape(_))
    ));
    assert!(matches!(
        StructureDoc::from_doc(Doc::new(DocKind::Structure, 1)),
        Err(ModelError::Shape(msg)) if msg.contains("space id")
    ));
    let doc = Doc::new(DocKind::Structure, 1);
    doc.inner()
        .get_map("space")
        .insert("id", "not-a-uuid")
        .unwrap();
    assert!(matches!(
        StructureDoc::from_doc(doc),
        Err(ModelError::Shape(_))
    ));
}

#[test]
fn space_name_and_icon_are_editable_and_not_undoable() {
    let doc = structure(1);
    doc.set_name("Work").unwrap();
    doc.set_icon(Some("💼")).unwrap();
    assert_eq!(doc.space().name, "Work");
    assert_eq!(doc.space().icon.as_deref(), Some("💼"));
    let version = doc.doc().version();
    doc.set_icon(Some("💼")).unwrap();
    doc.set_name("Work").unwrap();
    assert_eq!(
        doc.doc().version(),
        version,
        "unchanged values write nothing"
    );
    doc.set_icon(None).unwrap();
    assert_eq!(doc.space().icon, None);
    doc.set_icon(None).unwrap();
    assert!(!doc.can_undo(), "space metadata is outside structural undo");
    assert_eq!(doc.to_portable()["space"]["name"], "Work");
    assert!(doc.to_portable()["space"].get("icon").is_none());
}

#[test]
fn create_appends_or_inserts_at_the_clamped_live_index() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, None, "B");
    doc.create_page(pid(3), None, Some(0), "First", 30).unwrap();
    doc.create_page(pid(4), None, Some(2), "Middle", 40)
        .unwrap();
    doc.create_page(pid(5), None, Some(99), "Last", 50).unwrap();
    assert_eq!(outline(&doc), "First\nA\nMiddle\nB\nLast\n");

    add(&mut doc, 6, Some(1), "A-x");
    doc.create_page(pid(7), Some(pid(1)), Some(0), "A-first", 70)
        .unwrap();
    doc.create_page(pid(8), Some(pid(1)), Some(1), "A-mid", 80)
        .unwrap();
    doc.create_page(pid(9), Some(pid(7)), Some(5), "deep", 90)
        .unwrap();
    assert_eq!(
        outline(&doc),
        "First\nA\n  A-first\n    deep\n  A-mid\n  A-x\nMiddle\nB\nLast\n"
    );

    let info = doc.page(pid(9)).unwrap();
    assert_eq!(
        info,
        PageInfo {
            id: pid(9),
            space_id: sid(1),
            parent_id: Some(pid(7)),
            title: "deep".into(),
            icon: None,
            cover: None,
            cover_y: None,
            font: None,
            small_text: false,
            locked: false,
            created_at: 90,
            updated_at: 90,
            trashed_at: None,
            created_by: None,
            edited_by: None,
            properties: Vec::new(),
            template: false,
            template_placeholders: Vec::new(),
        }
    );
    assert_eq!(doc.page(pid(3)).unwrap().parent_id, None);
    assert_eq!(doc.live_page_count(), 9);
    assert_eq!(
        doc.create_page(pid(10), None, None, "", 1).unwrap(),
        vec![pid(10)]
    );
    assert_eq!(title(&doc, pid(10)), "", "empty titles are allowed");
    assert_healthy(&doc);
}

#[test]
fn create_refuses_duplicates_and_unavailable_parents() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, None, "Trashed");
    add(&mut doc, 4, None, "Purged");
    doc.trash_page(pid(3), 10).unwrap();
    doc.purge_page(pid(4), 11).unwrap();

    for existing in [1u8, 2, 3, 4] {
        assert!(
            matches!(
                doc.create_page(pid(existing), None, None, "x", 20),
                Err(ModelError::DuplicatePage)
            ),
            "page {existing}"
        );
    }
    doc.trash_page(pid(1), 12).unwrap();
    for parent in [pid(1), pid(2), pid(3), pid(4), pid(99)] {
        assert!(matches!(
            doc.create_page(pid(50), Some(parent), None, "x", 20),
            Err(ModelError::ParentUnavailable)
        ));
    }
    assert!(doc.page(pid(50)).is_none(), "nothing written on error");
    assert_healthy(&doc);
}

#[test]
fn rename_and_icon_work_on_live_and_trashed_pages() {
    let mut doc = structure(1);
    let a = add(&mut doc, 1, None, "A");
    assert_eq!(doc.rename_page(a, "Alpha", 100).unwrap(), vec![a]);
    let info = doc.page(a).unwrap();
    assert_eq!(
        (info.title.as_str(), info.created_at, info.updated_at),
        ("Alpha", 1, 100)
    );

    let version = doc.doc().version();
    assert!(doc.rename_page(a, "Alpha", 200).unwrap().is_empty());
    assert_eq!(doc.doc().version(), version, "same title writes nothing");
    assert_eq!(doc.page(a).unwrap().updated_at, 100);

    assert_eq!(doc.set_page_icon(a, Some("📝"), 110).unwrap(), vec![a]);
    assert_eq!(doc.page(a).unwrap().icon.as_deref(), Some("📝"));
    assert!(doc.set_page_icon(a, Some("📝"), 120).unwrap().is_empty());
    assert_eq!(doc.set_page_icon(a, None, 130).unwrap(), vec![a]);
    assert_eq!(doc.page(a).unwrap().icon, None);
    assert_eq!(doc.page(a).unwrap().updated_at, 130);
    assert!(doc.set_page_icon(a, None, 140).unwrap().is_empty());

    doc.trash_page(a, 150).unwrap();
    assert_eq!(doc.rename_page(a, "In the trash", 160).unwrap(), vec![a]);
    assert_eq!(doc.set_page_icon(a, Some("🗑"), 170).unwrap(), vec![a]);
    let trashed = &doc.trashed_pages()[0];
    assert_eq!(trashed.page.title, "In the trash");
    assert_eq!(trashed.page.icon.as_deref(), Some("🗑"));

    assert!(matches!(
        doc.rename_page(pid(9), "x", 1),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        doc.set_page_icon(pid(9), None, 1),
        Err(ModelError::PageNotFound)
    ));
}

#[test]
fn page_icons_lists_every_page_with_an_icon_live_or_trashed_and_none_after_a_purge() {
    let mut doc = structure(1);
    let (a, b, c, d) = (
        add(&mut doc, 4, None, "A"),
        add(&mut doc, 2, None, "B"),
        add(&mut doc, 3, Some(2), "C"),
        add(&mut doc, 1, None, "D"),
    );
    assert!(doc.page_icons().is_empty());
    assert_eq!(doc.page_icon(a), None);
    assert_eq!(doc.page_icon(pid(9)), None, "an unknown page has none");

    doc.set_page_icon(
        a,
        Some("attachment:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        10,
    )
    .unwrap();
    doc.set_page_icon(c, Some("🌱"), 11).unwrap();
    doc.set_page_icon(d, Some("icon:book-open:blue"), 12)
        .unwrap();
    assert_eq!(doc.page_icon(c).as_deref(), Some("🌱"));
    // In page id order, whatever the order of the pages in the tree.
    assert_eq!(
        doc.page_icons(),
        vec![
            (d, "icon:book-open:blue".to_owned()),
            (c, "🌱".to_owned()),
            (
                a,
                "attachment:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned()
            ),
        ]
    );

    // A trashed page (and the pages of a trashed subtree) keep their icons.
    doc.trash_page(b, 20).unwrap();
    assert_eq!(doc.page_icons().len(), 3);
    assert_eq!(doc.page_icon(c).as_deref(), Some("🌱"));
    doc.trash_page(a, 21).unwrap();
    assert_eq!(doc.page_icons().len(), 3);

    // Clearing an icon removes the entry; a purge removes the page's.
    doc.set_page_icon(d, None, 30).unwrap();
    assert_eq!(doc.page_icon(d), None);
    doc.purge_page(a, 31).unwrap();
    assert_eq!(doc.page_icons(), vec![(c, "🌱".to_owned())]);
    assert_healthy(&doc);
}

#[test]
fn move_within_a_parent_uses_the_final_position() {
    let mut doc = structure(1);
    for (n, t) in [(1u8, "A"), (2, "B"), (3, "C"), (4, "D")] {
        add(&mut doc, n, None, t);
    }
    assert_eq!(doc.move_page(pid(1), None, 2, 10).unwrap(), vec![pid(1)]);
    assert_eq!(outline(&doc), "B\nC\nA\nD\n");
    doc.move_page(pid(4), None, 0, 11).unwrap();
    assert_eq!(outline(&doc), "D\nB\nC\nA\n");
    doc.move_page(pid(2), None, 99, 12).unwrap();
    assert_eq!(
        outline(&doc),
        "D\nC\nA\nB\n",
        "clamped to the last position"
    );
    doc.move_page(pid(2), None, 2, 13).unwrap();
    assert_eq!(outline(&doc), "D\nC\nB\nA\n");
    assert_eq!(doc.page(pid(2)).unwrap().updated_at, 13);

    // Moving to where it already is changes nothing.
    let version = doc.doc().version();
    assert!(doc.move_page(pid(2), None, 2, 14).unwrap().is_empty());
    assert!(doc.move_page(pid(1), None, 3, 14).unwrap().is_empty());
    assert!(doc.move_page(pid(1), None, 50, 14).unwrap().is_empty());
    assert_eq!(doc.doc().version(), version);
    assert_eq!(doc.page(pid(2)).unwrap().updated_at, 13);
    assert_healthy(&doc);
}

#[test]
fn move_across_parents_returns_the_subtree() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, Some(2), "A1a");
    add(&mut doc, 4, None, "B");
    add(&mut doc, 5, Some(4), "B1");
    add(&mut doc, 6, Some(4), "B2");

    assert_eq!(
        doc.move_page(pid(2), Some(pid(4)), 1, 20).unwrap(),
        vec![pid(2), pid(3)]
    );
    assert_eq!(outline(&doc), "A\nB\n  B1\n  A1\n    A1a\n  B2\n");
    assert_eq!(doc.page(pid(2)).unwrap().parent_id, Some(pid(4)));
    assert_eq!(doc.page(pid(2)).unwrap().updated_at, 20);

    doc.move_page(pid(5), Some(pid(1)), 7, 21).unwrap();
    assert_eq!(outline(&doc), "A\n  B1\nB\n  A1\n    A1a\n  B2\n");
    doc.move_page(pid(3), None, 1, 22).unwrap();
    assert_eq!(outline(&doc), "A\n  B1\nA1a\nB\n  A1\n  B2\n");
    assert_eq!(doc.page(pid(3)).unwrap().parent_id, None);
    doc.move_page(pid(4), Some(pid(3)), 0, 23).unwrap();
    assert_eq!(outline(&doc), "A\n  B1\nA1a\n  B\n    A1\n    B2\n");
    assert_healthy(&doc);
}

#[test]
fn move_refuses_cycles_trash_and_unknown_pages() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, Some(2), "A1a");
    add(&mut doc, 4, None, "B");
    add(&mut doc, 5, Some(4), "B1");
    doc.trash_page(pid(4), 10).unwrap();
    let before = doc.to_portable();

    for (page, parent) in [(1u8, 1u8), (1, 2), (1, 3), (2, 3)] {
        assert!(
            matches!(
                doc.move_page(pid(page), Some(pid(parent)), 0, 20),
                Err(ModelError::CyclicMove)
            ),
            "{page} under {parent}"
        );
    }
    assert!(matches!(
        doc.move_page(pid(9), None, 0, 20),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        doc.move_page(pid(4), None, 0, 20),
        Err(ModelError::PageTrashed)
    ));
    assert!(matches!(
        doc.move_page(pid(5), None, 0, 20),
        Err(ModelError::PageTrashed)
    ));
    for parent in [pid(4), pid(5), pid(9)] {
        assert!(matches!(
            doc.move_page(pid(1), Some(parent), 0, 20),
            Err(ModelError::ParentUnavailable)
        ));
    }
    assert_eq!(doc.to_portable(), before, "refused moves write nothing");
    assert_healthy(&doc);
}

#[test]
fn trash_moves_the_subtree_and_records_where_it_came_from() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, Some(2), "A1a");
    add(&mut doc, 4, None, "B");

    assert_eq!(doc.trash_page(pid(2), 100).unwrap(), vec![pid(2), pid(3)]);
    assert_eq!(outline(&doc), "A\nB\n");
    assert_eq!(doc.live_page_count(), 2);
    assert_eq!(doc.trashed_page_count(), 1);
    let trashed = doc.trashed_pages();
    assert_eq!(trashed.len(), 1);
    assert_eq!(trashed[0].page.id, pid(2));
    assert_eq!(trashed[0].page.trashed_at, Some(100));
    assert_eq!(trashed[0].page.parent_id, None);
    assert_eq!(trashed[0].page.updated_at, 100);
    assert_eq!(trashed[0].original_parent_id, Some(pid(1)));
    assert_eq!(trashed[0].descendant_count, 1);

    let child = doc.page(pid(3)).unwrap();
    assert_eq!(
        child.parent_id,
        Some(pid(2)),
        "descendants keep their parent"
    );
    assert_eq!(
        child.trashed_at,
        Some(100),
        "and carry the top page's trash time"
    );

    assert!(matches!(
        doc.trash_page(pid(2), 101),
        Err(ModelError::PageTrashed)
    ));
    assert!(matches!(
        doc.trash_page(pid(3), 101),
        Err(ModelError::PageTrashed)
    ));
    assert!(matches!(
        doc.trash_page(pid(9), 101),
        Err(ModelError::PageNotFound)
    ));

    // A root page has no `trashedFrom`.
    doc.trash_page(pid(4), 200).unwrap();
    let portable = doc.to_portable();
    let trash = portable["trash"].as_array().unwrap();
    assert_eq!(trash[0]["id"], pid(4).to_string());
    assert_eq!(trash[0]["trashedAt"], 200);
    assert!(trash[0].get("trashedFrom").is_none());
    assert_eq!(trash[1]["trashedFrom"], pid(1).to_string());
    assert_eq!(trash[1]["children"][0]["id"], pid(3).to_string());
    assert!(trash[1]["children"][0].get("trashedAt").is_none());
    // Live pages first, then the trash in its sibling order (oldest first).
    assert_eq!(doc.all_page_ids(), vec![pid(1), pid(2), pid(3), pid(4)]);
    assert_healthy(&doc);
}

#[test]
fn the_trash_lists_the_most_recently_trashed_first() {
    let mut doc = structure(1);
    for n in 1..=4u8 {
        add(&mut doc, n, None, &format!("P{n}"));
    }
    doc.trash_page(pid(1), 10).unwrap();
    doc.trash_page(pid(2), 30).unwrap();
    doc.trash_page(pid(3), 20).unwrap();
    doc.trash_page(pid(4), 30).unwrap();
    assert_eq!(
        trash_titles(&doc),
        ["P2", "P4", "P3", "P1"],
        "ties by page id"
    );
    assert_eq!(doc.trashed_page_count(), 4);
    assert_eq!(doc.live_page_count(), 0);
}

#[test]
fn restore_returns_to_the_live_parent_or_to_the_root() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, Some(2), "A1a");
    add(&mut doc, 4, Some(1), "A2");
    add(&mut doc, 5, None, "B");

    // Back under its live parent, appended after the siblings.
    doc.trash_page(pid(2), 10).unwrap();
    assert!(matches!(
        doc.restore_page(pid(3), 11),
        Err(ModelError::PageNotTrashed)
    ));
    assert!(matches!(
        doc.restore_page(pid(1), 11),
        Err(ModelError::PageNotTrashed)
    ));
    assert!(matches!(
        doc.restore_page(pid(9), 11),
        Err(ModelError::PageNotFound)
    ));
    assert_eq!(doc.restore_page(pid(2), 12).unwrap(), vec![pid(2), pid(3)]);
    assert_eq!(outline(&doc), "A\n  A2\n  A1\n    A1a\nB\n");
    let info = doc.page(pid(2)).unwrap();
    assert_eq!(
        (info.parent_id, info.trashed_at, info.updated_at),
        (Some(pid(1)), None, 12)
    );
    assert_eq!(doc.page(pid(3)).unwrap().trashed_at, None);
    let portable = doc.to_portable();
    assert!(portable["trash"].as_array().unwrap().is_empty());

    // Parent trashed meanwhile: to the root.
    doc.trash_page(pid(4), 20).unwrap();
    doc.trash_page(pid(1), 21).unwrap();
    doc.restore_page(pid(4), 22).unwrap();
    assert_eq!(outline(&doc), "B\nA2\n");
    assert_eq!(doc.page(pid(4)).unwrap().parent_id, None);

    // Parent purged meanwhile: to the root as well.
    doc.trash_page(pid(5), 30).unwrap();
    doc.restore_page(pid(1), 31).unwrap();
    doc.trash_page(pid(2), 32).unwrap();
    doc.purge_page(pid(1), 33).unwrap();
    doc.restore_page(pid(2), 34).unwrap();
    assert_eq!(outline(&doc), "A2\nA1\n  A1a\n");
    assert_eq!(trash_titles(&doc), ["B"]);
    assert_healthy(&doc);
}

#[test]
fn purge_deletes_the_subtree_and_writes_tombstones() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, Some(2), "A1a");
    add(&mut doc, 4, Some(1), "A2");
    add(&mut doc, 5, None, "B");

    assert_eq!(
        doc.purge_page(pid(1), 500).unwrap(),
        vec![pid(1), pid(2), pid(3), pid(4)]
    );
    for n in 1..=4 {
        assert!(doc.page(pid(n)).is_none());
        assert!(matches!(doc.subtree(pid(n)), Err(ModelError::PageNotFound)));
    }
    assert_eq!(doc.tombstones(), {
        let mut t: Vec<_> = (1..=4).map(|n| (pid(n), 500)).collect();
        t.sort();
        t
    });
    assert_eq!(outline(&doc), "B\n");

    // Purging from the trash.
    doc.trash_page(pid(5), 600).unwrap();
    assert_eq!(doc.purge_page(pid(5), 601).unwrap(), vec![pid(5)]);
    assert!(doc.trashed_pages().is_empty());
    assert!(matches!(
        doc.purge_page(pid(5), 602),
        Err(ModelError::PageNotFound)
    ));
    assert_eq!(doc.all_page_ids(), vec![]);

    let portable = doc.to_portable();
    let keys: Vec<&String> = portable["tombstones"].as_object().unwrap().keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "tombstone keys are sorted");
    assert_eq!(portable["tombstones"][pid(5).to_string()], 601);
    assert_healthy(&doc);
}

#[test]
fn subtree_is_pre_order() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    add(&mut doc, 2, Some(1), "A1");
    add(&mut doc, 3, Some(2), "A1a");
    add(&mut doc, 4, Some(1), "A2");
    add(&mut doc, 5, Some(4), "A2a");
    assert_eq!(
        doc.subtree(pid(1)).unwrap(),
        vec![pid(1), pid(2), pid(3), pid(4), pid(5)]
    );
    assert_eq!(doc.subtree(pid(4)).unwrap(), vec![pid(4), pid(5)]);
    assert_eq!(doc.subtree(pid(5)).unwrap(), vec![pid(5)]);
    assert!(matches!(doc.subtree(pid(9)), Err(ModelError::PageNotFound)));
}

#[test]
fn portable_json_is_exact_and_engine_free() {
    let mut doc = StructureDoc::new(
        Doc::new(DocKind::Structure, 7),
        SpaceMeta {
            id: sid(2),
            name: "Work".into(),
            icon: Some("💼".into()),
            created_at: 1_000,
        },
    );
    doc.create_page(pid(1), None, None, "Notes", 1_001).unwrap();
    doc.create_page(pid(2), Some(pid(1)), None, "Draft", 1_002)
        .unwrap();
    doc.set_page_icon(pid(1), Some("🗒"), 1_003).unwrap();
    doc.create_page(pid(3), None, None, "Old", 1_004).unwrap();
    doc.create_page(pid(4), Some(pid(3)), None, "Older", 1_005)
        .unwrap();
    doc.create_page(pid(5), None, None, "Gone", 1_006).unwrap();
    doc.trash_page(pid(3), 1_007).unwrap();
    doc.purge_page(pid(5), 1_008).unwrap();

    let expected = json!({
        "format": 1,
        "pages": [{
            "children": [{
                "children": [], "createdAt": 1_002, "id": pid(2).to_string(),
                "title": "Draft", "updatedAt": 1_002
            }],
            "createdAt": 1_001, "icon": "🗒", "id": pid(1).to_string(),
            "title": "Notes", "updatedAt": 1_003
        }],
        "space": {"createdAt": 1_000, "icon": "💼", "id": sid(2).to_string(), "name": "Work"},
        "tombstones": {pid(5).to_string(): 1_008},
        "trash": [{
            "children": [{
                "children": [], "createdAt": 1_005, "id": pid(4).to_string(),
                "title": "Older", "updatedAt": 1_005
            }],
            "createdAt": 1_004, "id": pid(3).to_string(), "title": "Old",
            "trashedAt": 1_007, "updatedAt": 1_007
        }]
    });
    assert_eq!(doc.to_portable(), expected);
    // Byte-identical serialisation on a replica.
    assert_eq!(
        serde_json::to_string(&replica(&doc, 8).to_portable()).unwrap(),
        serde_json::to_string(&doc.to_portable()).unwrap()
    );
}

#[test]
fn check_invariants_reports_damaged_documents() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "A");
    assert_healthy(&doc);
    let tree = doc.doc().inner().get_tree("pages");

    // A change behind the model's back leaves the index stale until `refresh`.
    let raw = tree.create(TreeParentId::Root).unwrap();
    let raw_meta = tree.get_meta(raw).unwrap();
    raw_meta.insert("id", pid(2).to_string()).unwrap();
    raw_meta.insert("title", "B").unwrap();
    raw_meta.insert("created_at", 1).unwrap();
    raw_meta.insert("updated_at", 1).unwrap();
    doc.doc().inner().commit();
    let stale = doc.check_invariants();
    assert!(stale.iter().any(|p| p.contains("out of date")), "{stale:?}");
    assert!(doc.page(pid(2)).is_none());
    doc.refresh().unwrap();
    assert_healthy(&doc);
    assert_eq!(outline(&doc), "A\nB\n");

    // A node without a page id, a nested trash marker and a duplicate id.
    let anonymous = tree.create(TreeParentId::Root).unwrap();
    tree.get_meta(anonymous)
        .unwrap()
        .insert("title", "?")
        .unwrap();
    let a_node = doc.index.node(pid(1)).unwrap();
    let marker = tree.create(a_node).unwrap();
    tree.get_meta(marker)
        .unwrap()
        .insert("kind", "trash")
        .unwrap();
    let twin = tree.create(TreeParentId::Root).unwrap();
    tree.get_meta(twin)
        .unwrap()
        .insert("id", pid(1).to_string())
        .unwrap();
    doc.doc().inner().commit();
    doc.refresh().unwrap();
    let problems = doc.check_invariants();
    assert!(
        problems.iter().any(|p| p.contains("no valid page id")),
        "{problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("trash marker on non-root")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("duplicate page id")),
        "{problems:?}"
    );
    // Damaged nodes are hidden, the real pages stay usable.
    assert_eq!(outline(&doc), "A\nB\n");
    assert_eq!(doc.live_page_count(), 2);
    doc.rename_page(pid(1), "A2", 10).unwrap();
    assert_eq!(title(&doc, pid(1)), "A2");

    // No titles in diagnostics.
    assert!(problems.iter().all(|p| !p.contains('?')));
}

#[test]
fn with_duplicate_ids_the_first_copy_in_tree_order_stays_visible_after_edits() {
    // Duplicates only arise from undo racing with remote edits (Loro re-creates
    // nodes under new ids); visibility then depends on tree order, which local
    // moves change, so the index must follow.
    let mut doc = structure(1);
    add(&mut doc, 1, None, "X");
    add(&mut doc, 2, None, "Y");
    let tree = doc.doc().inner().get_tree("pages");
    let copy = tree.create(TreeParentId::Root).unwrap();
    let meta = tree.get_meta(copy).unwrap();
    meta.insert("id", pid(1).to_string()).unwrap();
    meta.insert("title", "X copy").unwrap();
    meta.insert("created_at", 1).unwrap();
    meta.insert("updated_at", 1).unwrap();
    doc.doc().inner().commit();
    doc.refresh().unwrap();
    assert_eq!(
        outline(&doc),
        "X
Y
"
    );

    // Moving the visible copy after the hidden one swaps which one is visible.
    doc.move_page(pid(1), None, 5, 10).unwrap();
    assert_eq!(
        outline(&doc),
        "Y
X copy
"
    );
    let problems = doc.check_invariants();
    assert!(
        problems.iter().all(|p| p.starts_with("duplicate page id")),
        "{problems:?}"
    );
    assert_eq!(title(&doc, pid(1)), "X copy");
}

#[test]
fn a_document_without_trash_root_gets_one_when_needed() {
    let doc = Doc::new(DocKind::Structure, 1);
    doc.inner()
        .get_map("space")
        .insert("id", sid(3).to_string())
        .unwrap();
    doc.inner().commit();
    let mut doc = StructureDoc::from_doc(doc).unwrap();
    assert!(
        doc.check_invariants()
            .iter()
            .any(|p| p.contains("no trash root"))
    );
    add(&mut doc, 1, None, "A");
    doc.trash_page(pid(1), 5).unwrap();
    assert_eq!(trash_titles(&doc), ["A"]);
    assert_healthy(&doc);
    assert_eq!(doc.space().name, "", "missing fields read as defaults");
}

#[test]
fn documents_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<StructureDoc>();
    assert_send::<crate::AccountDoc>();
}

#[test]
fn a_thousand_pages_round_trip_quickly() {
    let started = Instant::now();
    let mut doc = structure(1);
    let mut ids = Vec::with_capacity(1_000);
    for i in 0..1_000u32 {
        let id = kb_types::PageId::from_random_bytes({
            let mut b = [0u8; 16];
            b[..4].copy_from_slice(&i.to_le_bytes());
            b[15] = 0xaa;
            b
        });
        let parent = if i % 4 == 0 || ids.is_empty() {
            None
        } else {
            Some(ids[(i as usize * 7) % ids.len()])
        };
        doc.create_page(id, parent, None, &format!("Page {i}"), i64::from(i))
            .unwrap();
        ids.push(id);
    }
    let snapshot = doc.doc().export_snapshot().unwrap();
    let loaded =
        StructureDoc::from_doc(Doc::load(DocKind::Structure, 2, Some(&snapshot), &[]).unwrap())
            .unwrap();
    assert_eq!(loaded.live_page_count(), 1_000);
    assert_eq!(loaded.all_page_ids().len(), 1_000);
    assert_eq!(loaded.live_tree().len(), doc.live_tree().len());
    let elapsed = started.elapsed();
    // Well under a second in release; the assertion only guards against blow-ups.
    assert!(
        elapsed < Duration::from_secs(20),
        "1,000 pages took {elapsed:?}"
    );
}
