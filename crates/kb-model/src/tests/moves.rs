//! Moves between spaces (ADR-0038, `docs/architecture/space-move.md` section 3): the residue
//! nodes, `move_out`, `move_in`, eviction, orphans, undo, merging and an older reader.

use std::collections::HashSet;

use kb_doc::{Doc, DocKind};
use kb_types::{
    DeviceId, OptionColor, OptionId, PageFont, PageId, PropertyId, PropertyKind, PropertyValue,
};
use loro::{LoroTree, TreeID, TreeParentId};
use serde_json::json;

use super::{add, assert_healthy, outline, pid, replica, sid, structure, sync, title};
use crate::{MOVE_ORIGIN, ModelError, MovedPage, SpaceMeta, StructureDoc};

fn device() -> DeviceId {
    DeviceId::from_random_bytes([0xf1, 7, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 7])
}

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xb1, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn opt(n: u8) -> OptionId {
    OptionId::from_random_bytes([0xc1, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
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

/// The schema both spaces share in these tests: a select (`prop(1)`, option `opt(1)` "Doing")
/// and a number (`prop(2)`).
fn with_schema(doc: &mut StructureDoc) {
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
}

/// Space 1 (`a`, with `author` set): Inbox(1); Project(2, icon, values) with Plan(3) and Notes(4)
/// that holds Deep(5); Archive(6). Favourites: Plan, then Project.
fn space_a() -> StructureDoc {
    let mut a = structure_in(1, 1);
    a.set_author(Some(device()));
    with_schema(&mut a);
    add(&mut a, 1, None, "Inbox");
    add(&mut a, 2, None, "Project");
    add(&mut a, 3, Some(2), "Plan");
    add(&mut a, 4, Some(2), "Notes");
    add(&mut a, 5, Some(4), "Deep");
    add(&mut a, 6, None, "Archive");
    a.set_page_icon(pid(2), Some("⭐"), 20).unwrap();
    a.set_page_cover(pid(2), Some("gradient:sunset"), Some(30), 21)
        .unwrap();
    a.set_page_appearance(pid(2), Some(PageFont::Serif), Some(true), Some(true), 22)
        .unwrap();
    a.set_page_appearance(pid(3), Some(PageFont::Mono), None, None, 23)
        .unwrap();
    a.set_page_property(pid(2), prop(1), Some(PropertyValue::Select(opt(1))), 30)
        .unwrap();
    a.set_page_property(pid(2), prop(2), Some(PropertyValue::Number(3.5)), 31)
        .unwrap();
    a.set_page_property(pid(5), prop(2), Some(PropertyValue::Number(7.0)), 32)
        .unwrap();
    a.touch_edited(pid(2), 100_000).unwrap();
    a.set_favorite(pid(3), true).unwrap();
    a.set_favorite(pid(2), true).unwrap();
    a
}

/// Space 2 (`b`): Target(10) with X(11) and Y(12); Other(13).
fn space_b() -> StructureDoc {
    let mut b = structure_in(2, 2);
    with_schema(&mut b);
    add(&mut b, 10, None, "Target");
    add(&mut b, 11, Some(10), "X");
    add(&mut b, 12, Some(10), "Y");
    add(&mut b, 13, None, "Other");
    b
}

fn ids(ns: &[u8]) -> Vec<PageId> {
    ns.iter().map(|n| pid(*n)).collect()
}

fn favorite_ns(doc: &StructureDoc) -> Vec<u8> {
    doc.favorites()
        .into_iter()
        .map(|id| u8::from_str_radix(&id.to_string()[..2], 16).unwrap())
        .collect()
}

fn moved_ns(doc: &StructureDoc) -> Vec<u8> {
    doc.moved_out()
        .into_iter()
        .map(|(id, _, _)| u8::from_str_radix(&id.to_string()[..2], 16).unwrap())
        .collect()
}

/// The meta keys of a node, sorted.
fn meta_keys(doc: &StructureDoc, node: TreeID) -> Vec<String> {
    let mut keys: Vec<String> = doc
        .meta_of(node)
        .unwrap()
        .keys()
        .map(|k| k.to_string())
        .collect();
    keys.sort();
    keys
}

fn tree_of(doc: &StructureDoc) -> LoroTree {
    doc.doc().inner().get_tree("pages")
}

// ----- the reading half ---------------------------------------------------------------------

#[test]
fn moved_pages_reads_the_live_subtree_in_pre_order_without_writing() {
    let a = space_a();
    let portable = a.to_portable();
    let version = a.doc().version();

    let pages = a.moved_pages(pid(2)).unwrap();
    assert_eq!(
        pages.iter().map(|p| p.page_id).collect::<Vec<_>>(),
        ids(&[2, 3, 4, 5])
    );
    assert_eq!(
        pages.iter().map(|p| p.parent).collect::<Vec<_>>(),
        [None, Some(0), Some(0), Some(2)]
    );
    assert_eq!(
        pages.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
        ["Project", "Plan", "Notes", "Deep"]
    );
    assert_eq!(
        pages.iter().map(|p| p.favorite).collect::<Vec<_>>(),
        [true, true, false, false]
    );
    let project = &pages[0];
    assert_eq!(project.icon.as_deref(), Some("⭐"));
    assert_eq!(project.cover.as_deref(), Some("gradient:sunset"));
    assert_eq!(project.cover_y, Some(30));
    assert_eq!((pages[1].cover.as_deref(), pages[1].cover_y), (None, None));
    // The look of each page, as the page reports it (ADR-0040).
    assert_eq!(
        (project.font, project.small_text, project.locked),
        (Some(PageFont::Serif), true, true)
    );
    assert_eq!(
        (pages[1].font, pages[1].small_text, pages[1].locked),
        (Some(PageFont::Mono), false, false)
    );
    assert_eq!(
        (pages[2].font, pages[2].small_text, pages[2].locked),
        (None, false, false)
    );
    assert_eq!(project.created_at, 2);
    assert_eq!(project.created_by, Some(device()));
    assert_eq!(project.updated_at, 100_000);
    assert_eq!(project.edited_by, Some(device()));
    // The values are exactly what the node stores, by the source property id: a select is the
    // option id string, a number a JSON number.
    assert_eq!(
        project.props,
        vec![(prop(1), json!(opt(1).to_string())), (prop(2), json!(3.5)),]
    );
    assert_eq!(pages[3].props, vec![(prop(2), json!(7.0))]);
    assert!(pages[1].props.is_empty());

    // Nothing was written.
    assert_eq!(a.to_portable(), portable);
    assert_eq!(a.doc().version(), version);
}

#[test]
fn moved_pages_refuses_what_move_out_refuses() {
    let mut a = space_a();
    a.trash_page(pid(6), 40).unwrap();
    assert!(matches!(
        a.moved_pages(pid(99)),
        Err(ModelError::PageNotFound)
    ));
    assert!(matches!(
        a.moved_pages(pid(6)),
        Err(ModelError::PageTrashed)
    ));
    a.move_out(pid(1), sid(2), 50).unwrap();
    assert!(
        matches!(a.moved_pages(pid(1)), Err(ModelError::PageNotFound)),
        "residue is not a page"
    );
}

// ----- out and in ---------------------------------------------------------------------------

#[test]
fn a_move_from_a_to_b_keeps_metadata_values_favourites_and_order() {
    let mut a = space_a();
    let mut b = space_b();

    let moved = a.move_out(pid(2), sid(2), 500).unwrap();
    assert_eq!(moved, space_a_pages_before_the_move());
    let created = b
        .move_in(&moved, Some(pid(10)), Some(1), sid(1), 600)
        .unwrap();
    assert_eq!(created, ids(&[2, 3, 4, 5]));

    // The source: the pages are gone from every reader, the others are untouched.
    assert_eq!(outline(&a), "Inbox\nArchive\n");
    assert_eq!(a.live_page_count(), 2);
    assert_eq!(a.all_page_ids(), ids(&[1, 6]));
    assert_eq!(moved_ns(&a), [2, 3, 4, 5]);
    for (_, to, at) in a.moved_out() {
        assert_eq!((to, at), (sid(2), 500));
    }
    assert!(
        a.favorites().is_empty(),
        "the favourites left with the pages"
    );
    assert!(a.tombstones().is_empty(), "a move is not a purge");
    assert_healthy(&a);

    // The target: the first page at index 1 under Target, the others under their parents.
    assert_eq!(
        outline(&b),
        "Target\n  X\n  Project\n    Plan\n    Notes\n      Deep\n  Y\nOther\n"
    );
    let project = b.page(pid(2)).unwrap();
    assert_eq!(project.space_id, sid(2));
    assert_eq!(project.parent_id, Some(pid(10)));
    assert_eq!(project.title, "Project");
    assert_eq!(project.icon.as_deref(), Some("⭐"));
    assert_eq!(project.cover.as_deref(), Some("gradient:sunset"));
    assert_eq!(project.cover_y, Some(30));
    assert_eq!(
        (project.font, project.small_text, project.locked),
        (Some(PageFont::Serif), true, true),
        "the look moves with the page"
    );
    let plan = b.page(pid(3)).unwrap();
    assert_eq!(
        (plan.font, plan.small_text, plan.locked),
        (Some(PageFont::Mono), false, false)
    );
    let notes = b.page(pid(4)).unwrap();
    assert_eq!(
        (notes.font, notes.small_text, notes.locked),
        (None, false, false)
    );
    assert_eq!((project.created_at, project.updated_at), (2, 100_000));
    assert_eq!(
        project.properties,
        vec![
            (prop(1), PropertyValue::Select(opt(1))),
            (prop(2), PropertyValue::Number(3.5)),
        ]
    );
    assert_eq!(b.page(pid(5)).unwrap().parent_id, Some(pid(4)));
    // The favourites came along, appended in the order of the list.
    assert_eq!(favorite_ns(&b), [2, 3]);
    // Arrival: when it came; a page that never moved reports its creation.
    assert_eq!(b.arrival(pid(2)), Some(600));
    assert_eq!(b.arrival(pid(10)), Some(10));
    assert_eq!(b.arrival(pid(99)), None);
    let portable = b.to_portable();
    let top = &portable["pages"][0]["children"][1];
    assert_eq!(top["arrivedAt"], 600);
    assert_eq!(top["arrivedFrom"], sid(1).to_string());
    assert_eq!(top["createdBy"], device().to_string());
    assert_eq!(top["editedBy"], device().to_string());
    // What the target reads back is what the source read out.
    assert_eq!(b.moved_pages(pid(2)).unwrap(), moved);
    assert_healthy(&b);
}

/// What `space_a()` holds of the Project subtree, as the move must read it.
fn space_a_pages_before_the_move() -> Vec<MovedPage> {
    space_a().moved_pages(pid(2)).unwrap()
}

#[test]
fn the_residue_holds_the_id_and_the_stamps_and_nothing_else() {
    let mut a = space_a();
    a.move_out(pid(2), sid(2), 500).unwrap();

    let tree = tree_of(&a);
    let roots = tree.roots();
    let moved_roots: Vec<TreeID> = roots
        .iter()
        .copied()
        .filter(|r| meta_keys(&a, *r) == ["kind"])
        .filter(|r| crate::meta::is_moved_marker(&a.meta_of(*r).unwrap()))
        .collect();
    assert_eq!(moved_roots.len(), 1, "one moved root, made on first use");
    let root = moved_roots[0];

    // The top node sits under the moved root; the shape of the subtree is kept below it.
    let top = a.index.residue[&pid(2)];
    assert_eq!(tree.parent(top), Some(TreeParentId::Node(root)));
    assert_eq!(tree.children(TreeParentId::Node(top)).unwrap().len(), 2);
    for n in [2, 3, 4, 5] {
        let node = a.index.residue[&pid(n)];
        assert_eq!(
            meta_keys(&a, node),
            ["id", "movedAt", "movedTo"],
            "residue of page {n} keeps the id and the stamps only"
        );
        let meta = a.meta_of(node).unwrap();
        assert_eq!(crate::meta::int(&meta, "movedAt"), Some(500));
        assert_eq!(
            crate::meta::string(&meta, "movedTo"),
            Some(sid(2).to_string())
        );
    }
    // The trash root and the live tree are as they were.
    assert_eq!(a.index.trash_roots.len(), 1);
    assert_eq!(a.index.moved_roots, [root]);
    assert_healthy(&a);

    // A second move reuses the root.
    a.move_out(pid(1), sid(2), 510).unwrap();
    assert_eq!(a.index.moved_roots, [root]);
    assert_eq!(moved_ns(&a), [1, 2, 3, 4, 5]);
}

#[test]
fn the_residue_shows_in_the_portable_json_only_when_there_is_some() {
    let mut a = space_a();
    assert!(a.to_portable().get("moved").is_none());
    a.move_out(pid(6), sid(2), 70).unwrap();
    let portable = a.to_portable();
    assert_eq!(
        portable["moved"],
        json!({ pid(6).to_string(): {"movedAt": 70, "movedTo": sid(2).to_string()} })
    );
    assert!(!portable["pages"].to_string().contains(&pid(6).to_string()));
    assert!(!portable["trash"].to_string().contains(&pid(6).to_string()));
}

#[test]
fn a_page_can_go_there_and_back_and_there_again() {
    let mut a = space_a();
    let mut b = space_b();
    let original = a.moved_pages(pid(2)).unwrap();

    // A -> B.
    let out = a.move_out(pid(2), sid(2), 500).unwrap();
    b.move_in(&out, None, None, sid(1), 600).unwrap();
    assert_eq!(favorite_ns(&b), [2, 3]);

    // B -> A: the residue of A is replaced by the live pages, the page keeps its metadata.
    let back = b.move_out(pid(2), sid(1), 700).unwrap();
    assert_eq!(back, out);
    assert_eq!(b.favorites(), vec![]);
    a.move_in(&back, Some(pid(1)), None, sid(2), 800).unwrap();
    assert!(moved_ns(&a).is_empty(), "the residue of A is gone");
    assert_eq!(
        outline(&a),
        "Inbox\n  Project\n    Plan\n    Notes\n      Deep\nArchive\n"
    );
    assert_eq!(a.moved_pages(pid(2)).unwrap(), {
        // The same pages, now arrived at 800 (favourites followed).
        let mut again = original.clone();
        again[0].parent = None;
        again
    });
    assert_eq!(a.arrival(pid(2)), Some(800));
    assert_eq!(
        a.page(pid(2)).unwrap().created_at,
        2,
        "creation survives the trips"
    );
    assert_eq!(favorite_ns(&a), [2, 3]);
    assert_eq!(moved_ns(&b), [2, 3, 4, 5], "B holds the residue now");
    assert_healthy(&a);
    assert_healthy(&b);

    // A -> B once more (B's residue is removed again), the other pages never noticed.
    let third = a.move_out(pid(2), sid(2), 900).unwrap();
    b.move_in(&third, Some(pid(13)), None, sid(1), 1_000)
        .unwrap();
    assert_eq!(
        outline(&b),
        "Target\n  X\n  Y\nOther\n  Project\n    Plan\n    Notes\n      Deep\n"
    );
    assert!(moved_ns(&b).is_empty());
    assert_eq!(moved_ns(&a), [2, 3, 4, 5]);
    assert_eq!(b.arrival(pid(2)), Some(1_000));
    assert!(a.tombstones().is_empty() && b.tombstones().is_empty());
    assert_healthy(&a);
    assert_healthy(&b);
}

#[test]
fn move_in_positions_follow_the_live_order_of_create_page() {
    let one = |n: u8, title: &str| MovedPage {
        page_id: pid(n),
        parent: None,
        title: title.into(),
        icon: None,
        cover: None,
        cover_y: None,
        font: None,
        small_text: false,
        locked: false,
        template: false,
        template_values: Vec::new(),
        created_at: 1,
        created_by: None,
        updated_at: 1,
        edited_by: None,
        props: Vec::new(),
        favorite: false,
    };
    let mut b = space_b();
    // Trash and residue roots sit between the live ones.
    b.trash_page(pid(13), 20).unwrap();
    b.move_out(pid(12), sid(1), 21).unwrap();
    add(&mut b, 14, None, "Last");
    let from = sid(1);
    b.move_in(&[one(20, "first")], Some(pid(10)), Some(0), from, 30)
        .unwrap();
    b.move_in(&[one(21, "clamped")], Some(pid(10)), Some(99), from, 30)
        .unwrap();
    b.move_in(&[one(22, "second")], Some(pid(10)), Some(1), from, 30)
        .unwrap();
    b.move_in(&[one(23, "at the top")], None, Some(0), from, 30)
        .unwrap();
    b.move_in(&[one(24, "last")], None, None, from, 30).unwrap();
    assert_eq!(
        outline(&b),
        "at the top\nTarget\n  first\n  second\n  X\n  clamped\nLast\nlast\n"
    );
    assert_healthy(&b);
}

#[test]
fn a_move_in_writes_everything_a_page_carries_and_nothing_it_lacks() {
    let mut b = space_b();
    let page = MovedPage {
        page_id: pid(30),
        parent: None,
        title: String::new(),
        icon: None,
        cover: None,
        cover_y: None,
        font: None,
        small_text: false,
        locked: false,
        template: false,
        template_values: Vec::new(),
        created_at: 11,
        created_by: None,
        updated_at: 12,
        edited_by: None,
        props: vec![
            (prop(1), json!(opt(1).to_string())),
            (prop(2), json!(4)),
            (prop(3), json!(2.5)),
            (prop(4), json!([opt(1).to_string(), "x"])),
            (prop(5), json!({"date": "2026-10-13", "time": "09:30"})),
            (prop(6), json!(true)),
            (prop(7), json!(null)),
        ],
        favorite: false,
    };
    b.move_in(&[page], None, None, sid(1), 99).unwrap();
    let read = b.moved_pages(pid(30)).unwrap();
    assert_eq!(read[0].icon, None);
    assert_eq!((read[0].created_by, read[0].edited_by), (None, None));
    assert_eq!((read[0].created_at, read[0].updated_at), (11, 12));
    // Integers stay integers, floats floats, lists and maps what they were, nulls are dropped.
    assert_eq!(
        read[0].props,
        vec![
            (prop(1), json!(opt(1).to_string())),
            (prop(2), json!(4)),
            (prop(3), json!(2.5)),
            (prop(4), json!([opt(1).to_string(), "x"])),
            (prop(5), json!({"date": "2026-10-13", "time": "09:30"})),
            (prop(6), json!(true)),
        ]
    );
    assert!(read[0].props[1].1.is_i64() && read[0].props[2].1.is_f64());
    // Values of properties the schema does not have are kept and reported, as before.
    let problems = b.check_invariants();
    assert_eq!(problems.len(), 4, "{problems:?}");
    assert!(problems.iter().all(|p| p.contains("unknown property")));
}

#[test]
fn a_move_in_writes_the_look_a_page_carries_and_a_page_without_one_gets_no_keys() {
    let mut b = space_b();
    let one = |n: u8, font: Option<PageFont>, small_text: bool, locked: bool| MovedPage {
        page_id: pid(n),
        parent: None,
        title: format!("P{n}"),
        icon: None,
        cover: None,
        cover_y: None,
        font,
        small_text,
        locked,
        template: false,
        template_values: Vec::new(),
        created_at: 1,
        created_by: None,
        updated_at: 1,
        edited_by: None,
        props: Vec::new(),
        favorite: false,
    };
    b.move_in(
        &[one(30, Some(PageFont::Mono), true, true)],
        None,
        None,
        sid(1),
        99,
    )
    .unwrap();
    b.move_in(&[one(31, None, false, false)], None, None, sid(1), 99)
        .unwrap();
    let styled = b.page(pid(30)).unwrap();
    assert_eq!(
        (styled.font, styled.small_text, styled.locked),
        (Some(PageFont::Mono), true, true)
    );
    let plain = b.moved_pages(pid(31)).unwrap();
    assert_eq!(
        (plain[0].font, plain[0].small_text, plain[0].locked),
        (None, false, false)
    );
    let node = b.index.node(pid(31)).unwrap();
    for key in ["font", "smallText", "locked"] {
        assert!(!meta_keys(&b, node).contains(&key.to_owned()), "{key}");
    }
    // A default font in the list is no value either.
    b.move_in(
        &[one(32, Some(PageFont::Default), false, false)],
        None,
        None,
        sid(1),
        99,
    )
    .unwrap();
    assert_eq!(b.page(pid(32)).unwrap().font, None);
    assert_healthy(&b);
}

// ----- refusals -----------------------------------------------------------------------------

#[test]
fn refusals_write_nothing() {
    let mut a = space_a();
    a.trash_page(pid(6), 40).unwrap();
    a.add_trashed_child_for_the_test();
    let mut b = space_b();
    b.trash_page(pid(13), 41).unwrap();
    b.purge_page(pid(12), 42).unwrap();

    let a_state = (a.to_portable(), a.doc().version(), a.can_undo());
    let out_refusals: [(&str, PageId, bool); 3] = [
        ("an unknown page", pid(99), false),
        ("a trashed page", pid(6), true),
        ("a page inside a trashed subtree", pid(7), true),
    ];
    for (what, id, trashed) in out_refusals {
        let err = a.move_out(id, sid(2), 100).unwrap_err();
        if trashed {
            assert!(matches!(err, ModelError::PageTrashed), "{what}: {err:?}");
        } else {
            assert!(matches!(err, ModelError::PageNotFound), "{what}: {err:?}");
        }
    }
    assert!(matches!(
        a.move_out(pid(2), sid(1), 100),
        Err(ModelError::Shape(_))
    ));
    assert_eq!((a.to_portable(), a.doc().version(), a.can_undo()), a_state);

    let page = |n: u8| MovedPage {
        page_id: pid(n),
        parent: None,
        title: format!("P{n}"),
        icon: None,
        cover: None,
        cover_y: None,
        font: None,
        small_text: false,
        locked: false,
        template: false,
        template_values: Vec::new(),
        created_at: 1,
        created_by: None,
        updated_at: 1,
        edited_by: None,
        props: Vec::new(),
        favorite: true,
    };
    let child = |n: u8, parent: usize| MovedPage {
        parent: Some(parent),
        ..page(n)
    };
    let b_state = (b.to_portable(), b.doc().version(), b.can_undo());
    let in_refusals: Vec<(&str, Vec<MovedPage>, Option<PageId>, bool)> = vec![
        (
            "an id that is live here",
            vec![page(30), page(10)],
            None,
            true,
        ),
        ("an id in the trash", vec![page(30), page(13)], None, true),
        (
            "an id that was purged",
            vec![page(30), page(12)],
            None,
            true,
        ),
        ("an id twice", vec![page(30), child(30, 0)], None, true),
        (
            "a parent in the trash",
            vec![page(30)],
            Some(pid(13)),
            false,
        ),
        (
            "a parent that was purged",
            vec![page(30)],
            Some(pid(12)),
            false,
        ),
        (
            "a parent that does not exist",
            vec![page(30)],
            Some(pid(99)),
            false,
        ),
        (
            "a first page with a parent",
            vec![child(30, 0)],
            None,
            false,
        ),
        (
            "a later page without a parent",
            vec![page(30), page(31)],
            None,
            false,
        ),
        (
            "a parent that comes later",
            vec![page(30), child(31, 2), child(32, 0)],
            None,
            false,
        ),
        (
            "a parent that is the page itself",
            vec![page(30), child(31, 1)],
            None,
            false,
        ),
    ];
    for (what, pages, parent, duplicate) in in_refusals {
        let err = b.move_in(&pages, parent, None, sid(1), 100).unwrap_err();
        if duplicate {
            assert!(matches!(err, ModelError::DuplicatePage), "{what}: {err:?}");
        } else {
            assert!(
                matches!(err, ModelError::ParentUnavailable),
                "{what}: {err:?}"
            );
        }
        assert_eq!(
            (b.to_portable(), b.doc().version(), b.can_undo()),
            b_state,
            "{what}"
        );
    }
    assert!(matches!(
        b.move_in(&[page(30)], None, None, sid(2), 100),
        Err(ModelError::Shape(_))
    ));
    assert!(b.move_in(&[], None, None, sid(1), 100).unwrap().is_empty());
    assert_eq!(b.doc().version(), b_state.1);
    assert_healthy(&a);
    assert_healthy(&b);
}

impl StructureDoc {
    /// A trashed page inside the trashed Archive (6): page 7 under it, trashed with it.
    fn add_trashed_child_for_the_test(&mut self) {
        let node = self.index.node(pid(6)).unwrap();
        let tree = self.tree.clone();
        let child = tree.create(TreeParentId::Node(node)).unwrap();
        let meta = tree.get_meta(child).unwrap();
        crate::meta::set_string(&meta, "id", &pid(7).to_string()).unwrap();
        crate::meta::set_string(&meta, "title", "Trashed child").unwrap();
        crate::meta::set_int(&meta, "created_at", 1).unwrap();
        crate::meta::set_int(&meta, "updated_at", 1).unwrap();
        self.doc().inner().commit();
        self.refresh().unwrap();
    }
}

// ----- undo ---------------------------------------------------------------------------------

#[test]
fn a_move_is_not_an_undo_step_and_undo_never_touches_one() {
    let mut a = structure_in(1, 1);
    // The person's own steps: Keep, Moved, a rename of Keep.
    add(&mut a, 1, None, "Keep");
    add(&mut a, 2, None, "Moved");
    add(&mut a, 3, Some(2), "Child");
    a.rename_page(pid(1), "Keep 2", 1_000).unwrap();
    assert!(a.can_undo());

    a.move_out(pid(2), sid(2), 2_000).unwrap();
    assert_eq!(outline(&a), "Keep 2\n");
    let residue_before = a.moved_out();

    // Ctrl+Z undoes the rename, not the move.
    a.undo().unwrap().expect("the rename");
    assert_eq!(title(&a, pid(1)), "Keep");
    assert_eq!(a.moved_out(), residue_before, "the move stays");
    assert_eq!(outline(&a), "Keep\n");
    assert_healthy(&a);

    // And the creation steps of the moved pages do not take the residue with them.
    while a.can_undo() {
        a.undo().unwrap();
        assert_healthy(&a);
        assert_eq!(
            a.moved_out(),
            residue_before,
            "undo went through the creations"
        );
    }
    assert!(a.page(pid(2)).is_none() && a.page(pid(1)).is_none());
    while a.can_redo() {
        a.redo().unwrap();
        assert_healthy(&a);
    }
    assert_eq!(a.moved_out(), residue_before);
    assert!(a.page(pid(2)).is_none(), "the moved page is not live in A");
}

#[test]
fn move_in_adds_no_undo_step_and_the_pages_survive_undo() {
    let mut a = space_a();
    let mut b = structure_in(2, 2);
    with_schema(&mut b);
    add(&mut b, 1, None, "Mine");
    b.rename_page(pid(1), "Mine 2", 1_000).unwrap();
    let moved = a.move_out(pid(2), sid(2), 500).unwrap();
    b.move_in(&moved, None, None, sid(1), 600).unwrap();

    b.undo().unwrap().expect("the rename");
    assert_eq!(title(&b, pid(1)), "Mine");
    assert_eq!(b.live_page_count(), 5, "the arrived pages are still there");
    b.undo().unwrap().expect("the creation");
    assert!(!b.can_undo(), "the move was not on the stack");
    assert_eq!(outline(&b), "Project\n  Plan\n  Notes\n    Deep\n");
    while b.can_redo() {
        b.redo().unwrap();
    }
    assert_eq!(b.live_page_count(), 5);
    assert_healthy(&b);
}

#[test]
fn the_origin_is_part_of_the_public_surface() {
    assert_eq!(MOVE_ORIGIN, "kb-model/move");
}

// ----- invisibility -------------------------------------------------------------------------

#[test]
fn residue_is_invisible_to_every_reader_and_every_write() {
    let mut a = space_a();
    // A relation from a page that stays to a page that moves.
    let mut next = 0;
    let relation = prop(9);
    a.create_property(
        relation,
        "Related",
        PropertyKind::Relation,
        &mut || {
            next += 1;
            opt(210 + next)
        },
        1_003,
    )
    .unwrap();
    a.set_page_property(
        pid(1),
        relation,
        Some(PropertyValue::Relation(vec![pid(3), pid(6)])),
        50,
    )
    .unwrap();
    a.trash_page(pid(6), 51).unwrap();
    a.restore_page(pid(6), 52).unwrap();

    a.move_out(pid(2), sid(2), 500).unwrap();
    let gone = ids(&[2, 3, 4, 5]);

    assert_eq!(outline(&a), "Inbox\nArchive\n");
    assert_eq!(a.live_page_count(), 2);
    assert_eq!(a.trashed_page_count(), 0);
    assert!(a.trashed_pages().is_empty());
    assert_eq!(a.all_page_ids(), ids(&[1, 6]));
    assert!(a.tombstones().is_empty());
    assert!(a.favorites().is_empty());
    assert!(a.page_icons().is_empty());
    assert!(
        a.property_index_all()
            .iter()
            .all(|(page, _)| !gone.contains(page))
    );
    assert!(a.property_index(&gone).is_empty());
    for id in &gone {
        assert!(a.page(*id).is_none(), "{id}");
        assert!(a.subtree(*id).is_err());
        assert!(!a.is_favorite(*id));
        assert!(a.page_icon(*id).is_none());
        assert!(matches!(
            a.page_properties(*id),
            Err(ModelError::PageNotFound)
        ));
        assert_eq!(a.arrival(*id), None);
    }
    // The relation shows what still exists.
    assert_eq!(
        a.page_properties(pid(1)).unwrap(),
        vec![(relation, PropertyValue::Relation(vec![pid(6)]))]
    );
    // Nothing can change a residue: every write answers "no such page".
    let writes: Vec<Result<Vec<PageId>, ModelError>> = vec![
        a.rename_page(pid(2), "x", 600),
        a.set_page_icon(pid(2), Some("x"), 600),
        a.move_page(pid(2), None, 0, 600),
        a.trash_page(pid(2), 600),
        a.restore_page(pid(2), 600),
        a.purge_page(pid(2), 600),
        a.touch_edited(pid(3), 700_000),
        a.set_page_property(pid(3), prop(2), Some(PropertyValue::Number(1.0)), 600),
        a.create_page(pid(7), Some(pid(2)), None, "under residue", 600),
    ];
    for write in writes {
        assert!(
            matches!(
                write,
                Err(ModelError::PageNotFound | ModelError::ParentUnavailable)
            ),
            "{write:?}"
        );
    }
    assert!(matches!(
        a.set_favorite(pid(2), true),
        Err(ModelError::PageNotFound)
    ));
    assert!(a.tombstones().is_empty());
    assert_healthy(&a);
    // The residue survives a reload.
    let loaded = replica(&a, 9);
    assert_eq!(moved_ns(&loaded), [2, 3, 4, 5]);
    assert_eq!(loaded.to_portable(), a.to_portable());
    assert_healthy(&loaded);
}

#[test]
fn a_document_with_residue_read_the_old_way_shows_no_page_of_it() {
    // The index of the build before ADR-0038: roots are the trash (kind "trash") or live roots;
    // a node without a valid page id is skipped with its subtree.
    fn old_visible(doc: &StructureDoc) -> HashSet<PageId> {
        let tree = tree_of(doc);
        let mut out = HashSet::new();
        let mut roots = Vec::new();
        let mut trash = Vec::new();
        for root in tree.roots() {
            let meta = tree.get_meta(root).unwrap();
            if crate::meta::is_trash_marker(&meta) {
                trash.push(root);
            } else {
                roots.push(root);
            }
        }
        let mut stack: Vec<TreeID> = roots;
        for root in trash {
            stack.extend(tree.children(TreeParentId::Node(root)).unwrap_or_default());
        }
        while let Some(node) = stack.pop() {
            let meta = tree.get_meta(node).unwrap();
            let Some(id) = crate::meta::page_id(&meta, "id") else {
                continue;
            };
            out.insert(id);
            stack.extend(tree.children(TreeParentId::Node(node)).unwrap_or_default());
        }
        out
    }

    let mut a = space_a();
    // Even with an orphan hanging below the residue.
    let mut other = replica(&a, 7);
    a.move_out(pid(2), sid(2), 500).unwrap();
    other
        .create_page(pid(8), Some(pid(4)), None, "Orphan", 450)
        .unwrap();
    sync(&mut a, &mut other);

    let visible = old_visible(&a);
    assert_eq!(
        visible,
        HashSet::from_iter(ids(&[1, 6])),
        "only the pages that stayed"
    );
    // The new reader agrees on the pages that stayed.
    assert_eq!(a.all_page_ids(), ids(&[1, 6]));
}

// ----- evict --------------------------------------------------------------------------------

#[test]
fn evict_turns_the_losing_copies_into_residue_and_rehomes_the_children_it_leaves() {
    let mut b = space_b();
    b.set_favorite(pid(10), true).unwrap();
    b.set_favorite(pid(11), true).unwrap();
    // Evicting Target (10) and the trashed Other (13); X (11) is evicted too, Y (12) is not.
    b.trash_page(pid(13), 20).unwrap();
    b.set_page_property(pid(10), prop(2), Some(PropertyValue::Number(1.0)), 21)
        .unwrap();
    let before = b.doc().version();
    let evicted = b
        .evict(
            &[
                (pid(10), sid(3)),
                (pid(13), sid(3)),
                (pid(11), sid(3)),
                (pid(10), sid(4)), // twice: counted once
                (pid(99), sid(3)), // unknown: skipped
                (pid(12), sid(2)), // winner is this space: skipped
            ],
            700,
        )
        .unwrap();
    assert_eq!(evicted, ids(&[10, 13, 11]));
    assert!(b.doc().version() != before);

    // Y was a child of Target and was not listed: it is live again, at the end of the root.
    assert_eq!(outline(&b), "Y\n");
    assert_eq!(b.page(pid(12)).unwrap().parent_id, None);
    assert!(b.trashed_pages().is_empty());
    assert!(b.favorites().is_empty());
    assert!(b.orphans().is_empty());
    assert_eq!(moved_ns(&b), [10, 11, 13]);
    for (_, to, at) in b.moved_out() {
        assert_eq!((to, at), (sid(3), 700));
    }
    let node = b.index.residue[&pid(10)];
    assert_eq!(meta_keys(&b, node), ["id", "movedAt", "movedTo"]);
    assert!(b.tombstones().is_empty());
    assert_healthy(&b);

    // Evicting what is not there writes nothing.
    let version = b.doc().version();
    assert!(
        b.evict(&[(pid(10), sid(3)), (pid(99), sid(3))], 800)
            .unwrap()
            .is_empty()
    );
    assert!(b.evict(&[], 800).unwrap().is_empty());
    assert_eq!(b.doc().version(), version);
}

#[test]
fn evict_is_not_an_undo_step() {
    let mut b = space_b();
    let can_undo = b.can_undo();
    b.evict(&[(pid(13), sid(3))], 700).unwrap();
    assert_eq!(b.can_undo(), can_undo);
    assert_eq!(outline(&b), "Target\n  X\n  Y\n");
    while b.can_undo() {
        b.undo().unwrap();
    }
    assert_eq!(moved_ns(&b), [13]);
}

// ----- orphans ------------------------------------------------------------------------------

#[test]
fn a_page_created_under_a_page_that_moved_is_an_orphan_until_it_is_rehomed() {
    let mut a = space_a();
    let mut other = replica(&a, 7);
    // The other device, offline, makes a page (and a page below it) under Notes.
    other
        .create_page(pid(8), Some(pid(4)), None, "Orphan", 450)
        .unwrap();
    other
        .create_page(pid(9), Some(pid(8)), None, "Orphan child", 451)
        .unwrap();
    a.move_out(pid(2), sid(2), 500).unwrap();
    sync(&mut a, &mut other);

    for doc in [&mut a, &mut other] {
        assert_eq!(outline(doc), "Inbox\nArchive\n", "hidden until rehomed");
        assert!(doc.page(pid(8)).is_none());
        assert_eq!(doc.orphans(), ids(&[8, 9]));
        assert_eq!(doc.all_page_ids(), ids(&[1, 6]));
        assert_eq!(doc.live_page_count(), 2);
        assert_healthy(doc);
    }
    let rehomed = a.rehome_orphans(600).unwrap();
    assert_eq!(rehomed, ids(&[8, 9]), "the orphan and what is below it");
    assert_eq!(outline(&a), "Inbox\nArchive\nOrphan\n  Orphan child\n");
    assert_eq!(a.page(pid(8)).unwrap().parent_id, None);
    assert_eq!(a.page(pid(9)).unwrap().parent_id, Some(pid(8)));
    assert!(a.orphans().is_empty());
    assert_eq!(moved_ns(&a), [2, 3, 4, 5]);
    assert_healthy(&a);

    // Nothing left: nothing written.
    let version = a.doc().version();
    assert!(a.rehome_orphans(700).unwrap().is_empty());
    assert_eq!(a.doc().version(), version);

    // The other replica does the same; after a merge both are the same document.
    other.rehome_orphans(601).unwrap();
    sync(&mut a, &mut other);
    sync(&mut a, &mut other);
    assert_eq!(outline(&a), outline(&other));
    assert_eq!(a.to_portable(), other.to_portable());
    assert_healthy(&a);
    assert_healthy(&other);
}

#[test]
fn rehoming_leaves_the_tombstoned_the_nameless_and_the_duplicates_hidden() {
    let mut a = space_a();
    let mut other = replica(&a, 7);
    other
        .create_page(pid(8), Some(pid(4)), None, "Orphan", 450)
        .unwrap();
    other
        .create_page(pid(9), Some(pid(4)), None, "Doomed", 451)
        .unwrap();
    other.purge_page(pid(9), 452).unwrap();
    a.move_out(pid(2), sid(2), 500).unwrap();
    sync(&mut a, &mut other);
    // A node without a page id below the residue.
    let tree = tree_of(&a);
    let node = a.index.residue[&pid(4)];
    tree.create(TreeParentId::Node(node)).unwrap();
    a.doc().inner().commit();
    a.refresh().unwrap();

    assert_eq!(a.rehome_orphans(600).unwrap(), ids(&[8]));
    assert_eq!(outline(&a), "Inbox\nArchive\nOrphan\n");
    assert!(a.tombstones().iter().any(|(id, _)| *id == pid(9)));
    assert!(a.page(pid(9)).is_none());
    // The anomaly of the nameless node is reported, nothing else.
    let problems = a.check_invariants();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("no valid page id"));
}

#[test]
fn moving_a_page_back_re_roots_what_another_device_put_below_its_residue() {
    let mut a = space_a();
    let mut b = space_b();
    let mut other = replica(&a, 7);
    other
        .create_page(pid(8), Some(pid(4)), None, "Orphan", 450)
        .unwrap();
    let out = a.move_out(pid(2), sid(2), 500).unwrap();
    b.move_in(&out, None, None, sid(1), 600).unwrap();
    sync(&mut a, &mut other);
    assert_eq!(a.orphans(), ids(&[8]));

    // The page comes back: its residue goes, the orphan is live at the end of the root.
    let back = b.move_out(pid(2), sid(1), 700).unwrap();
    a.move_in(&back, None, Some(0), sid(2), 800).unwrap();
    assert_eq!(
        outline(&a),
        "Project\n  Plan\n  Notes\n    Deep\nInbox\nArchive\nOrphan\n"
    );
    assert!(moved_ns(&a).is_empty());
    assert!(a.orphans().is_empty());
    assert!(tree_of(&a).nodes().len() >= 8);
    assert_healthy(&a);
    // Another device that merges it sees the same.
    sync(&mut a, &mut other);
    other.rehome_orphans(801).unwrap();
    sync(&mut a, &mut other);
    assert_eq!(outline(&a), outline(&other));
    assert_healthy(&other);
}

#[test]
fn residue_that_hangs_below_the_residue_of_a_returning_page_stays_residue() {
    // p moved A -> B, then its child q moved on B -> C, then p came back to A without q.
    let mut a = space_a();
    let mut b = space_b();
    let out = a.move_out(pid(2), sid(2), 500).unwrap();
    b.move_in(&out, None, None, sid(1), 600).unwrap();
    let q = b.move_out(pid(4), sid(3), 650).unwrap();
    assert_eq!(q.len(), 2);
    let back = b.move_out(pid(2), sid(1), 700).unwrap();
    assert_eq!(back.len(), 2);

    a.move_in(&back, None, None, sid(2), 800).unwrap();
    // Notes (4) and Deep (5) are still residue in A (they went to B, then on), under the moved root.
    assert_eq!(moved_ns(&a), [4, 5]);
    assert_eq!(outline(&a), "Inbox\nArchive\nProject\n  Plan\n");
    assert_healthy(&a);
}

// ----- merging ------------------------------------------------------------------------------

#[test]
fn a_move_and_a_concurrent_child_creation_merge_to_one_orphan_that_comes_back() {
    let mut a = structure(1);
    add(&mut a, 1, None, "Keep");
    add(&mut a, 2, None, "Moved");
    add(&mut a, 3, Some(2), "Moved child");
    let mut b = replica(&a, 2);

    a.move_out(pid(2), sid(2), 100).unwrap();
    add(&mut b, 4, Some(2), "Late child");
    sync(&mut a, &mut b);

    for doc in [&mut a, &mut b] {
        assert_eq!(outline(doc), "Keep\n");
        assert_eq!(doc.orphans(), ids(&[4]));
        assert_eq!(moved_ns(doc), [2, 3]);
        assert_healthy(doc);
    }
    assert_eq!(a.to_portable(), b.to_portable());

    // Both devices re-root it; what they write converges.
    assert_eq!(a.rehome_orphans(200).unwrap(), ids(&[4]));
    assert_eq!(b.rehome_orphans(201).unwrap(), ids(&[4]));
    sync(&mut a, &mut b);
    assert_eq!(outline(&a), "Keep\nLate child\n");
    assert_eq!(a.to_portable(), b.to_portable());
    assert_eq!(a.live_page_count(), 2);
    assert_healthy(&a);
    assert_healthy(&b);
}

#[test]
fn a_move_and_a_concurrent_rename_merge_to_residue_and_the_rename_is_lost() {
    let mut a = structure(1);
    add(&mut a, 1, None, "Keep");
    add(&mut a, 2, None, "Moved");
    let mut b = replica(&a, 2);

    let out = a.move_out(pid(2), sid(2), 100).unwrap();
    b.rename_page(pid(2), "Renamed offline", 90).unwrap();
    sync(&mut a, &mut b);

    // The page is residue on both devices whichever write won inside its meta map.
    for doc in [&a, &b] {
        assert_eq!(outline(doc), "Keep\n");
        assert!(doc.page(pid(2)).is_none());
        assert_eq!(moved_ns(doc), [2]);
        assert_eq!(doc.moved_out()[0].1, sid(2));
        assert_healthy(doc);
    }
    assert_eq!(a.to_portable(), b.to_portable());
    // What arrived in the other space is the title read before the rename.
    assert_eq!(out[0].title, "Moved");
    let mut target = structure_in(3, 2);
    target.move_in(&out, None, None, sid(1), 110).unwrap();
    assert_eq!(title(&target, pid(2)), "Moved");
}

#[test]
fn two_devices_moving_the_same_page_converge_on_one_stamp() {
    let mut a = structure(1);
    add(&mut a, 1, None, "Keep");
    add(&mut a, 2, None, "Moved");
    let mut b = replica(&a, 2);

    a.move_out(pid(2), sid(2), 100).unwrap();
    b.move_out(pid(2), sid(3), 110).unwrap();
    sync(&mut a, &mut b);

    assert_eq!(a.moved_out(), b.moved_out());
    assert_eq!(a.moved_out().len(), 1);
    assert_eq!(outline(&a), "Keep\n");
    assert_eq!(a.to_portable(), b.to_portable());
    // Two moved roots (both created one) are both moved roots.
    assert_eq!(a.index.moved_roots.len(), 2);
    assert_eq!(b.index.moved_roots.len(), 2);
    assert_healthy(&a);
    assert_healthy(&b);
    // New residue goes to one of them.
    a.move_out(pid(1), sid(2), 120).unwrap();
    assert_eq!(moved_ns(&a), [1, 2]);
    assert_healthy(&a);
}

#[test]
fn a_page_stamped_as_moved_is_residue_wherever_a_merge_leaves_it() {
    let mut a = structure(1);
    add(&mut a, 1, None, "Home");
    add(&mut a, 2, None, "Moved");
    add(&mut a, 3, None, "Elsewhere");
    let mut b = replica(&a, 2);

    // A moves page 2 out; B (offline, with a higher clock) puts it under Home.
    a.move_out(pid(2), sid(2), 100).unwrap();
    for i in 0..10 {
        b.rename_page(pid(3), &format!("Elsewhere {i}"), 10 + i)
            .unwrap();
    }
    b.move_page(pid(2), Some(pid(1)), 0, 200).unwrap();
    add(&mut b, 4, Some(2), "Child of the moved page");
    sync(&mut a, &mut b);

    let tree = tree_of(&a);
    let node = a.index.residue[&pid(2)];
    assert_eq!(
        tree.parent(node),
        Some(TreeParentId::Node(a.index.node(pid(1)).unwrap())),
        "precondition: the node sits under a live page"
    );
    for doc in [&mut a, &mut b] {
        assert!(doc.page(pid(2)).is_none(), "a stamped node is never a page");
        assert_eq!(moved_ns(doc), [2]);
        assert_eq!(outline(doc), "Home\nElsewhere 9\n");
        assert_eq!(doc.orphans(), ids(&[4]));
        assert_healthy(doc);
    }
    assert_eq!(a.rehome_orphans(300).unwrap(), ids(&[4]));
    assert_eq!(outline(&a), "Home\nElsewhere 9\nChild of the moved page\n");
    assert_healthy(&a);
}

#[test]
fn residue_and_a_returning_page_can_coexist_after_a_merge_and_stay_healthy() {
    // Device 1 moves the page back (residue deleted, fresh node); device 2 meanwhile re-stamps the
    // residue with a newer move. Loro lets a move beat a delete, so both nodes exist.
    let mut a = space_a();
    let mut b = space_b();
    let out = a.move_out(pid(6), sid(2), 500).unwrap();
    b.move_in(&out, None, None, sid(1), 600).unwrap();
    let mut other = replica(&a, 7);
    for i in 0..10 {
        other
            .rename_page(pid(1), &format!("Inbox {i}"), 610 + i)
            .unwrap();
    }
    other.evict(&[(pid(1), sid(3))], 700).unwrap();
    let back = b.move_out(pid(6), sid(1), 650).unwrap();
    a.move_in(&back, None, None, sid(2), 660).unwrap();
    sync(&mut a, &mut other);
    sync(&mut a, &mut other);

    assert_eq!(a.to_portable(), other.to_portable());
    assert!(a.page(pid(6)).is_some(), "the page is live again");
    assert_healthy(&a);
    assert_healthy(&other);
}

// ----- the checks ---------------------------------------------------------------------------

#[test]
fn check_invariants_reports_a_residue_that_does_not_say_where_it_went() {
    let mut a = space_a();
    a.move_out(pid(6), sid(2), 70).unwrap();
    assert_healthy(&a);
    let node = a.index.residue[&pid(6)];
    let meta = a.meta_of(node).unwrap();
    meta.insert("movedTo", "not a space id").unwrap();
    a.doc().inner().commit();
    a.refresh().unwrap();
    let problems = a.check_invariants();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("movedTo"), "{problems:?}");
    assert!(
        a.moved_out().is_empty(),
        "the entry is unreadable, not listed"
    );
    assert_eq!(moved_ns(&a), Vec::<u8>::new());
}

#[test]
fn check_invariants_reports_an_orphan_that_duplicates_a_live_page() {
    let mut a = space_a();
    let mut other = replica(&a, 7);
    // The other device makes page 8 under Notes; A makes a page with the same id elsewhere.
    other
        .create_page(pid(8), Some(pid(4)), None, "Orphan", 450)
        .unwrap();
    a.move_out(pid(2), sid(2), 500).unwrap();
    add(&mut a, 8, None, "Same id");
    sync(&mut a, &mut other);
    let problems = a.check_invariants();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("hidden below residue"), "{problems:?}");
    // Re-rooting it would duplicate the page, so it stays hidden.
    assert!(a.rehome_orphans(600).unwrap().is_empty());
    assert_eq!(title(&a, pid(8)), "Same id");
}

#[test]
fn a_moved_marker_below_the_roots_is_an_anomaly() {
    let mut a = space_a();
    let tree = tree_of(&a);
    let node = a.index.node(pid(6)).unwrap();
    let marker = tree.create(TreeParentId::Node(node)).unwrap();
    crate::meta::set_string(&tree.get_meta(marker).unwrap(), "kind", "moved").unwrap();
    a.doc().inner().commit();
    a.refresh().unwrap();
    let problems = a.check_invariants();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("moved marker on non-root node")),
        "{problems:?}"
    );
    assert_eq!(
        outline(&a),
        "Inbox\nProject\n  Plan\n  Notes\n    Deep\nArchive\n"
    );
}

// ----- plumbing ----------------------------------------------------------------------------

#[test]
fn a_move_in_a_loaded_replica_reads_the_same() {
    let mut a = space_a();
    let mut b = space_b();
    let out = a.move_out(pid(2), sid(2), 500).unwrap();
    b.move_in(&out, Some(pid(10)), Some(0), sid(1), 600)
        .unwrap();
    for doc in [&a, &b] {
        let loaded = replica(doc, 9);
        assert_eq!(loaded.to_portable(), doc.to_portable());
        assert_eq!(loaded.moved_out(), doc.moved_out());
        assert_eq!(loaded.favorites(), doc.favorites());
        assert_healthy(&loaded);
    }
}

#[test]
fn undoing_the_creation_of_a_page_that_received_moved_pages_keeps_them() {
    let mut a = space_a();
    let mut b = structure_in(2, 2);
    with_schema(&mut b);
    add(&mut b, 1, None, "Container");
    let out = a.move_out(pid(2), sid(2), 500).unwrap();
    b.move_in(&out, Some(pid(1)), None, sid(1), 600).unwrap();
    assert_eq!(
        outline(&b),
        "Container\n  Project\n    Plan\n    Notes\n      Deep\n"
    );

    // The person undoes the creation of the container: it goes, the pages that arrived in it
    // stay, at the end of the root, with their subtree and their arrival.
    b.undo().unwrap().expect("the creation of the container");
    assert_eq!(outline(&b), "Project\n  Plan\n  Notes\n    Deep\n");
    assert_eq!(b.arrival(pid(2)), Some(600));
    assert_eq!(favorite_ns(&b), [2, 3]);
    assert_healthy(&b);
    // Redo brings the container back and leaves the pages where they are.
    b.redo().unwrap().expect("the creation again");
    assert_eq!(b.live_page_count(), 5);
    assert!(b.page(pid(2)).is_some() && b.page(pid(5)).is_some());
    assert_healthy(&b);
}

#[test]
fn undoing_an_earlier_edit_of_a_moved_page_leaves_its_residue_clean() {
    let mut a = structure_in(1, 1);
    add(&mut a, 1, None, "P");
    a.rename_page(pid(1), "P renamed", 1_000).unwrap();
    a.move_out(pid(1), sid(2), 2_000).unwrap();

    a.undo().unwrap().expect("the rename");
    let node = a.index.residue[&pid(1)];
    assert_eq!(meta_keys(&a, node), ["id", "movedAt", "movedTo"]);
    assert_eq!(moved_ns(&a), [1]);
    assert_healthy(&a);

    // Redo of the rename does not bring the title back either; the creation steps neither.
    a.redo().unwrap().expect("the rename again");
    let node = a.index.residue[&pid(1)];
    assert_eq!(meta_keys(&a, node), ["id", "movedAt", "movedTo"]);
    assert_eq!(outline(&a), "");
    assert_healthy(&a);
}

// ----- duplicates (two devices moved the same subtree in) ----------------------------------------

/// Two replicas of a space-1 document ("Keep" and the shared schema) that have not met yet.
fn two_replicas() -> (StructureDoc, StructureDoc) {
    let mut base = structure_in(1, 1);
    with_schema(&mut base);
    add(&mut base, 1, None, "Keep");
    (replica(&base, 2), replica(&base, 3))
}

fn ids_sorted(mut ids: Vec<PageId>) -> Vec<PageId> {
    ids.sort();
    ids
}

#[test]
fn two_devices_moving_the_same_subtree_in_merge_to_one_copy_of_each_page() {
    let (mut r1, mut r2) = two_replicas();
    let list = space_a_pages_before_the_move();
    r1.move_in(&list, None, None, sid(2), 100).unwrap();
    r2.move_in(&list, Some(pid(1)), None, sid(2), 110).unwrap();
    sync(&mut r1, &mut r2);

    // Before the merge: every page is shown once (the index keeps the first node of an id), the
    // other nodes are reported as duplicates.
    for doc in [&r1, &r2] {
        assert_eq!(doc.live_page_count(), 5);
        assert!(
            doc.check_invariants()
                .iter()
                .any(|p| p.contains("duplicate page id")),
            "the duplicates are reported until they are merged"
        );
    }
    assert_eq!(r1.to_portable(), r2.to_portable());
    let shown = outline(&r1);

    // Each device merges on its own, with its own clock; the result is the same document.
    let merged1 = r1.merge_duplicates(200).unwrap();
    let merged2 = r2.merge_duplicates(210).unwrap();
    assert_eq!(ids_sorted(merged1), ids(&[2, 3, 4, 5]));
    assert_eq!(ids_sorted(merged2), ids(&[2, 3, 4, 5]));
    sync(&mut r1, &mut r2);
    sync(&mut r1, &mut r2);
    for doc in [&r1, &r2] {
        assert_eq!(
            outline(doc),
            shown,
            "the page the index showed stays where it was"
        );
        assert_eq!(
            doc.live_page_count(),
            5,
            "every page once, with its children"
        );
        assert_eq!(doc.all_page_ids().len(), 5);
        assert_eq!(doc.page(pid(5)).unwrap().title, "Deep");
        assert_eq!(doc.subtree(pid(2)).unwrap(), ids(&[2, 3, 4, 5]));
        assert!(doc.orphans().is_empty());
        assert!(doc.tombstones().is_empty());
        assert_healthy(doc);
        // The merged-away copies are residue stamped with the document's own space, and no move.
        assert_eq!(doc.index.residue_nodes.len(), 4);
        assert!(doc.moved_out().is_empty());
    }
    assert_eq!(r1.to_portable(), r2.to_portable());

    // Nothing left to do: no commit, on either device.
    let version = r1.doc().version();
    assert!(r1.merge_duplicates(300).unwrap().is_empty());
    assert_eq!(r1.doc().version(), version);

    // A real move out shows next to the merged-away copies, and only it counts as a move.
    r1.move_out(pid(5), sid(3), 400).unwrap();
    assert_eq!(r1.moved_out(), vec![(pid(5), sid(3), 400)]);
    assert_healthy(&r1);
}

#[test]
fn a_duplicate_whose_copy_has_an_extra_child_keeps_that_child_live() {
    // Whichever copy the index shows, the child only one copy had stays live: under the page when
    // its copy is the one kept, at the end of the root when its copy was merged away.
    let mut rehomed = 0;
    for extra_on_first in [true, false] {
        let (mut r1, mut r2) = two_replicas();
        let list = space_a_pages_before_the_move();
        r1.move_in(&list, None, None, sid(2), 100).unwrap();
        r2.move_in(&list, None, None, sid(2), 110).unwrap();
        let extra_doc = if extra_on_first { &mut r1 } else { &mut r2 };
        extra_doc
            .create_page(pid(8), Some(pid(4)), None, "Extra", 120)
            .unwrap();
        extra_doc
            .create_page(pid(9), Some(pid(8)), None, "Extra child", 121)
            .unwrap();
        sync(&mut r1, &mut r2);

        let merged = r1.merge_duplicates(200).unwrap();
        assert_eq!(ids_sorted(merged), ids(&[2, 3, 4, 5]));
        r2.merge_duplicates(210).unwrap();
        sync(&mut r1, &mut r2);
        sync(&mut r1, &mut r2);
        for doc in [&r1, &r2] {
            assert_eq!(
                doc.live_page_count(),
                7,
                "the four pages and the extra two, once each"
            );
            assert_eq!(doc.page(pid(8)).unwrap().title, "Extra");
            assert_eq!(doc.page(pid(9)).unwrap().parent_id, Some(pid(8)));
            assert!(doc.orphans().is_empty());
            assert_healthy(doc);
        }
        assert_eq!(r1.to_portable(), r2.to_portable());
        match r1.page(pid(8)).unwrap().parent_id {
            Some(parent) => assert_eq!(parent, pid(4), "under the copy that was kept"),
            None => rehomed += 1,
        }
    }
    assert_eq!(
        rehomed, 1,
        "exactly one of the two runs had the extra child on the lost copy"
    );
}

#[test]
fn a_trashed_copy_is_merged_away_like_any_other() {
    let (mut r1, mut r2) = two_replicas();
    let list = space_a_pages_before_the_move();
    r1.move_in(&list, None, None, sid(2), 100).unwrap();
    r2.move_in(&list, None, None, sid(2), 110).unwrap();
    r1.trash_page(pid(2), 150).unwrap();
    sync(&mut r1, &mut r2);
    // The live copy is shown, the trashed one is the duplicate.
    assert_eq!(r1.live_page_count(), 5);
    assert_eq!(r1.trashed_page_count(), 0);

    assert_eq!(
        ids_sorted(r1.merge_duplicates(200).unwrap()),
        ids(&[2, 3, 4, 5])
    );
    r2.merge_duplicates(210).unwrap();
    sync(&mut r1, &mut r2);
    for doc in [&r1, &r2] {
        assert_eq!(doc.live_page_count(), 5);
        assert!(doc.trashed_pages().is_empty());
        assert!(doc.moved_out().is_empty());
        assert_healthy(doc);
    }
    assert_eq!(r1.to_portable(), r2.to_portable());
}

#[test]
fn merging_duplicates_writes_nothing_when_there_are_none() {
    let mut a = space_a();
    a.move_out(pid(6), sid(2), 70).unwrap();
    let version = a.doc().version();
    let can_undo = a.can_undo();
    assert!(a.merge_duplicates(100).unwrap().is_empty());
    assert_eq!(a.doc().version(), version);
    assert_eq!(a.can_undo(), can_undo);
    // Residue and orphans that are no duplicates do not count.
    assert_eq!(moved_ns(&a), [6]);
}
