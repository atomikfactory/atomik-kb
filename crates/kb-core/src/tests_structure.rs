//! Milestone 2 tests: spaces and the page tree through `dispatch`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use kb_storage::DocQueries;
use kb_types::{ErrorCode, Event, Request, Response};

use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, outline, page, spaces, tree,
};
use crate::{CompactionPolicy, Core};

#[test]
fn workspace_create_can_create_the_first_space() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    create_workspace(&mut core, "Home", Some("Personal"));
    let events = core.drain_events();
    assert!(matches!(events[0], Event::WorkspaceCreated { .. }));
    assert!(events.contains(&Event::SpacesChanged));

    let list = spaces(&mut core);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Personal");
    assert_eq!(list[0].page_count, 0);
    assert_eq!(list[0].created_at, 100);
    let view = tree(&mut core, list[0].id);
    assert!(view.roots.is_empty());
    assert!(view.trashed.is_empty());
    assert!(!view.undo.can_undo);
}

#[test]
fn spaces_are_ordered_renamed_moved_and_deleted_only_when_empty() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", None);
    let a = create_space(&mut core, "Alpha");
    let b = create_space(&mut core, "Beta");
    let c = create_space(&mut core, "Gamma");
    assert_eq!(
        spaces(&mut core)
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta", "Gamma"]
    );

    match core
        .dispatch(Request::SpaceMove {
            space_id: c.id,
            index: 0,
        })
        .unwrap()
    {
        Response::Spaces(list) => assert_eq!(
            list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Gamma", "Alpha", "Beta"]
        ),
        other => panic!("unexpected {other:?}"),
    }

    match core
        .dispatch(Request::SpaceRename {
            space_id: b.id,
            name: "  Work  ".into(),
        })
        .unwrap()
    {
        Response::Space(s) => assert_eq!(s.name, "Work"),
        other => panic!("unexpected {other:?}"),
    }
    match core
        .dispatch(Request::SpaceSetIcon {
            space_id: b.id,
            icon: Some("💼".into()),
        })
        .unwrap()
    {
        Response::Space(s) => assert_eq!(s.icon.as_deref(), Some("💼")),
        other => panic!("unexpected {other:?}"),
    }

    let p = create_page(&mut core, a.id, None, "Keep");
    // A space with a live page and a trashed one is deleted with both, for good.
    let trashed = create_page(&mut core, a.id, None, "Gone");
    core.dispatch(Request::PageTrash {
        page_id: trashed.id,
    })
    .unwrap();
    assert_eq!(
        core.dispatch(Request::SpaceDelete { space_id: a.id })
            .unwrap(),
        Response::Ok
    );
    for page_id in [p.id, trashed.id] {
        assert_eq!(
            core.dispatch(Request::PageGet { page_id })
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
    }
    assert_eq!(
        spaces(&mut core)
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Gamma", "Work"]
    );
    assert_eq!(
        core.dispatch(Request::TreeGet { space_id: a.id })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );

    let too_long = "n".repeat(101);
    for bad in ["", "   ", too_long.as_str()] {
        let err = core
            .dispatch(Request::SpaceCreate {
                name: bad.into(),
                icon: None,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
    }
    let err = core
        .dispatch(Request::SpaceCreate {
            name: "Ok".into(),
            icon: Some("   ".into()),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn page_lifecycle_create_nest_move_rename_trash_restore_purge() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 10);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;

    let a = create_page(&mut core, space, None, "A");
    let b = create_page(&mut core, space, None, "B");
    let a1 = create_page(&mut core, space, Some(a.id), "A1");
    let a2 = create_page(&mut core, space, Some(a.id), "A2");
    let a1x = create_page(&mut core, space, Some(a1.id), "A1x");
    assert_eq!(
        outline(&tree(&mut core, space)),
        "A\n  A1\n    A1x\n  A2\nB\n"
    );
    assert_eq!(spaces(&mut core)[0].page_count, 5);
    core.drain_events();

    // Insert at an explicit index.
    match core
        .dispatch(Request::PageCreate {
            space_id: space,
            parent_id: None,
            index: Some(0),
            title: "  First  ".into(),
        })
        .unwrap()
    {
        Response::Page(p) => {
            assert_eq!(p.title, "First");
            assert_eq!(p.parent_id, None);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        core.drain_events(),
        vec![
            Event::StructureChanged { space_id: space },
            Event::SpacesChanged
        ],
        "a new page changes the space's page count"
    );

    // Reorder: B to the front; then reparent: A2 under B at index 0.
    core.dispatch(Request::PageMove {
        page_id: b.id,
        parent_id: None,
        index: 0,
    })
    .unwrap();
    core.dispatch(Request::PageMove {
        page_id: a2.id,
        parent_id: Some(b.id),
        index: 0,
    })
    .unwrap();
    assert_eq!(
        outline(&tree(&mut core, space)),
        "B\n  A2\nFirst\nA\n  A1\n    A1x\n"
    );
    assert_eq!(page(&mut core, a2.id).parent_id, Some(b.id));

    // Clamped index and a cyclic move.
    core.dispatch(Request::PageMove {
        page_id: a.id,
        parent_id: None,
        index: 99,
    })
    .unwrap();
    assert_eq!(
        outline(&tree(&mut core, space)),
        "B\n  A2\nFirst\nA\n  A1\n    A1x\n"
    );
    let cyclic = core
        .dispatch(Request::PageMove {
            page_id: a.id,
            parent_id: Some(a1x.id),
            index: 0,
        })
        .unwrap_err();
    assert_eq!(cyclic.code, ErrorCode::InvalidInput);

    // Rename and icon.
    match core
        .dispatch(Request::PageRename {
            page_id: a1.id,
            title: " Renamed ".into(),
        })
        .unwrap()
    {
        Response::Page(p) => assert_eq!(p.title, "Renamed"),
        other => panic!("unexpected {other:?}"),
    }
    match core
        .dispatch(Request::PageSetIcon {
            page_id: a1.id,
            icon: Some("📝".into()),
        })
        .unwrap()
    {
        Response::Page(p) => assert_eq!(p.icon.as_deref(), Some("📝")),
        other => panic!("unexpected {other:?}"),
    }
    let long = "t".repeat(501);
    assert_eq!(
        core.dispatch(Request::PageRename {
            page_id: a1.id,
            title: long,
        })
        .unwrap_err()
        .code,
        ErrorCode::InvalidInput
    );

    // Trash A (with A1, A1x) and inspect.
    core.dispatch(Request::PageTrash { page_id: a.id }).unwrap();
    let view = tree(&mut core, space);
    assert_eq!(outline(&view), "B\n  A2\nFirst\n");
    assert_eq!(view.trashed.len(), 1);
    assert_eq!(view.trashed[0].page.id, a.id);
    assert_eq!(view.trashed[0].descendant_count, 2);
    assert!(view.trashed[0].page.trashed_at.is_some());
    assert!(
        page(&mut core, a1x.id).trashed_at.is_some(),
        "subtree is trashed"
    );
    assert_eq!(spaces(&mut core)[0].page_count, 3);
    let cannot_trash_twice = core
        .dispatch(Request::PageTrash { page_id: a.id })
        .unwrap_err();
    assert_eq!(cannot_trash_twice.code, ErrorCode::InvalidInput);
    let cannot_parent_under_trashed = core
        .dispatch(Request::PageCreate {
            space_id: space,
            parent_id: Some(a.id),
            index: None,
            title: "x".into(),
        })
        .unwrap_err();
    assert_eq!(cannot_parent_under_trashed.code, ErrorCode::InvalidInput);

    // Restore A: back at the root, subtree intact.
    match core
        .dispatch(Request::PageRestore { page_id: a.id })
        .unwrap()
    {
        Response::Page(p) => assert_eq!(p.trashed_at, None),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        outline(&tree(&mut core, space)),
        "B\n  A2\nFirst\nA\n  Renamed\n    A1x\n"
    );
    assert_eq!(page(&mut core, a1x.id).trashed_at, None);

    // Restore to the root when the original parent is gone: trash A1 (under A),
    // then purge A, then restore A1.
    core.dispatch(Request::PageTrash { page_id: a1.id })
        .unwrap();
    core.dispatch(Request::PagePurge { page_id: a.id }).unwrap();
    assert_eq!(
        core.dispatch(Request::PageGet { page_id: a.id })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    match core
        .dispatch(Request::PageRestore { page_id: a1.id })
        .unwrap()
    {
        Response::Page(p) => assert_eq!(p.parent_id, None, "falls back to the root"),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        outline(&tree(&mut core, space)),
        "B\n  A2\nFirst\nRenamed\n  A1x\n"
    );

    // Purge a trashed page with a subtree.
    core.dispatch(Request::PageTrash { page_id: b.id }).unwrap();
    core.dispatch(Request::PagePurge { page_id: b.id }).unwrap();
    assert_eq!(
        core.dispatch(Request::PageGet { page_id: a2.id })
            .unwrap_err()
            .code,
        ErrorCode::NotFound,
        "purged with its parent"
    );
    assert_eq!(outline(&tree(&mut core, space)), "First\nRenamed\n  A1x\n");
}

#[test]
fn move_index_is_the_final_position_among_siblings() {
    // The UI's "Move down" sends index + 1 and "Move up" sends index - 1, both
    // counted among the siblings without the moved page.
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    let a = create_page(&mut core, space, None, "A");
    let b = create_page(&mut core, space, None, "B");
    let c = create_page(&mut core, space, None, "C");
    assert_eq!(outline(&tree(&mut core, space)), "A\nB\nC\n");

    core.dispatch(Request::PageMove {
        page_id: a.id,
        parent_id: None,
        index: 1,
    })
    .unwrap();
    assert_eq!(
        outline(&tree(&mut core, space)),
        "B\nA\nC\n",
        "A moved down one"
    );

    core.dispatch(Request::PageMove {
        page_id: c.id,
        parent_id: None,
        index: 0,
    })
    .unwrap();
    assert_eq!(
        outline(&tree(&mut core, space)),
        "C\nB\nA\n",
        "C moved to the top"
    );

    core.dispatch(Request::PageMove {
        page_id: c.id,
        parent_id: None,
        index: 2,
    })
    .unwrap();
    assert_eq!(
        outline(&tree(&mut core, space)),
        "B\nA\nC\n",
        "C moved to the end"
    );

    // Reparent at index 0 under B, then back to the root at index 1.
    core.dispatch(Request::PageMove {
        page_id: a.id,
        parent_id: Some(b.id),
        index: 0,
    })
    .unwrap();
    assert_eq!(outline(&tree(&mut core, space)), "B\n  A\nC\n");
    core.dispatch(Request::PageMove {
        page_id: a.id,
        parent_id: None,
        index: 1,
    })
    .unwrap();
    assert_eq!(outline(&tree(&mut core, space)), "B\nA\nC\n");

    // Page count events: moves do not change the live count, trash does.
    core.drain_events();
    core.dispatch(Request::PageMove {
        page_id: a.id,
        parent_id: None,
        index: 0,
    })
    .unwrap();
    assert_eq!(
        core.drain_events(),
        vec![Event::StructureChanged { space_id: space }]
    );
    core.dispatch(Request::PageTrash { page_id: a.id }).unwrap();
    assert_eq!(
        core.drain_events(),
        vec![
            Event::StructureChanged { space_id: space },
            Event::SpacesChanged
        ]
    );
}

/// `PageReorderChildren` (ADR-0052): one reorder is one event, one undo step and one persisted
/// update, a no-op writes and emits nothing, a wrong set is refused, no page is stamped as edited.
#[test]
fn reorder_children_is_one_step_and_survives_reopen() {
    let fx = Fixture::new();
    let space = {
        let mut core = fx.open(1, 1_000);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = spaces(&mut core)[0].id;
        let other_space = create_space(&mut core, "Other").id;
        let a = create_page(&mut core, space, None, "A");
        let b = create_page(&mut core, space, None, "B");
        let c = create_page(&mut core, space, None, "C");
        let d = create_page(&mut core, space, Some(c.id), "C1");
        let reorder = |core: &mut Core, parent: Option<kb_types::PageId>, order: Vec<_>| {
            core.dispatch(Request::PageReorderChildren {
                space_id: space,
                parent_id: parent,
                order,
            })
        };
        core.drain_events();

        // The order is applied, as one event and with no new page index row stamped.
        let before = page(&mut core, b.id);
        reorder(&mut core, None, vec![c.id, b.id, a.id]).unwrap();
        assert_eq!(outline(&tree(&mut core, space)), "C\n  C1\nB\nA\n");
        assert_eq!(
            core.drain_events(),
            vec![Event::StructureChanged { space_id: space }]
        );
        assert_eq!(page(&mut core, b.id), before, "no page is edited");

        // Already in that order: nothing written, nothing emitted.
        reorder(&mut core, None, vec![c.id, b.id, a.id]).unwrap();
        assert!(core.drain_events().is_empty());

        // A set that is not the live children is refused with the field `order`.
        for wrong in [
            vec![c.id, b.id],
            vec![c.id, b.id, a.id, d.id],
            vec![c.id, b.id, b.id],
        ] {
            let err = reorder(&mut core, None, wrong).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput);
            assert_eq!(err.details, Some(serde_json::json!({ "field": "order" })));
        }
        // A parent that is not a live page of this space is refused with `parentId`.
        let elsewhere = create_page(&mut core, other_space, None, "Elsewhere");
        let err = reorder(&mut core, Some(elsewhere.id), vec![]).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(
            err.details,
            Some(serde_json::json!({ "field": "parentId" }))
        );
        // A space that does not exist is NotFound.
        let err = core
            .dispatch(Request::PageReorderChildren {
                space_id: kb_types::SpaceId::from_random_bytes([9; 16]),
                parent_id: None,
                order: vec![],
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        core.drain_events();

        // One structural undo step puts the whole order back (the pages are all still there).
        match core
            .dispatch(Request::StructureUndo { space_id: space })
            .unwrap()
        {
            Response::Tree(view) => assert_eq!(outline(&view), "A\nB\nC\n  C1\n"),
            other => panic!("unexpected {other:?}"),
        }
        match core
            .dispatch(Request::StructureRedo { space_id: space })
            .unwrap()
        {
            Response::Tree(view) => assert_eq!(outline(&view), "C\n  C1\nB\nA\n"),
            other => panic!("unexpected {other:?}"),
        }
        space
    };

    // The order is in the persisted update: a reopened core reads it back.
    let mut core = fx.open(2, 2_000);
    assert_eq!(outline(&tree(&mut core, space)), "C\n  C1\nB\nA\n");
}

#[test]
fn everything_survives_reopen() {
    let fx = Fixture::new();
    let (space, a_id) = {
        let mut core = fx.open(1, 0);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = spaces(&mut core)[0].id;
        let a = create_page(&mut core, space, None, "A");
        let _b = create_page(&mut core, space, None, "B");
        let a1 = create_page(&mut core, space, Some(a.id), "A1");
        core.dispatch(Request::PageTrash { page_id: a1.id })
            .unwrap();
        core.dispatch(Request::PageSetIcon {
            page_id: a.id,
            icon: Some("🏠".into()),
        })
        .unwrap();
        create_space(&mut core, "Second");
        (space, a.id)
    };

    let mut core = fx.open(2, 500);
    let list = spaces(&mut core);
    assert_eq!(
        list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Personal", "Second"]
    );
    assert_eq!(list[0].page_count, 2);
    let view = tree(&mut core, space);
    assert_eq!(outline(&view), "A\nB\n");
    assert_eq!(view.trashed.len(), 1);
    assert_eq!(view.trashed[0].page.title, "A1");
    assert_eq!(page(&mut core, a_id).icon.as_deref(), Some("🏠"));
}

#[test]
fn undo_and_redo_structural_changes() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    let a = create_page(&mut core, space, None, "A");
    core.dispatch(Request::PageRename {
        page_id: a.id,
        title: "A2".into(),
    })
    .unwrap();
    assert!(tree(&mut core, space).undo.can_undo);
    core.drain_events();

    let view = match core
        .dispatch(Request::StructureUndo { space_id: space })
        .unwrap()
    {
        Response::Tree(t) => t,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(outline(&view), "A\n", "rename undone");
    assert!(view.undo.can_redo);
    assert_eq!(
        core.drain_events(),
        vec![Event::StructureChanged { space_id: space }]
    );

    let view = match core
        .dispatch(Request::StructureUndo { space_id: space })
        .unwrap()
    {
        Response::Tree(t) => t,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(outline(&view), "", "creation undone");
    assert!(!view.undo.can_undo);
    assert_eq!(
        core.dispatch(Request::PageGet { page_id: a.id })
            .unwrap_err()
            .code,
        ErrorCode::NotFound,
        "index follows the undo"
    );

    let view = match core
        .dispatch(Request::StructureRedo { space_id: space })
        .unwrap()
    {
        Response::Tree(t) => t,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(outline(&view), "A\n");
    assert_eq!(page(&mut core, a.id).title, "A");
    assert!(view.undo.can_redo, "the rename is still on the redo stack");

    // The second redo re-applies the rename; after that there is nothing to redo:
    // no event, same tree.
    let view = match core
        .dispatch(Request::StructureRedo { space_id: space })
        .unwrap()
    {
        Response::Tree(t) => t,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(outline(&view), "A2\n");
    assert!(!view.undo.can_redo);
    core.drain_events();
    let noop = match core
        .dispatch(Request::StructureRedo { space_id: space })
        .unwrap()
    {
        Response::Tree(t) => t,
        other => panic!("unexpected {other:?}"),
    };
    assert!(core.drain_events().is_empty());
    assert_eq!(outline(&noop), "A2\n");

    drop(core);
    let mut core = fx.open(2, 1);
    assert_eq!(
        outline(&tree(&mut core, space)),
        "A2\n",
        "undo/redo results persist"
    );
}

#[test]
fn compaction_replaces_updates_with_a_snapshot_and_reopen_is_consistent() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    core.set_compaction_policy(CompactionPolicy {
        max_updates: 3,
        max_bytes: u64::MAX,
    });
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    for i in 0..10 {
        create_page(&mut core, space, None, &format!("P{i}"));
    }
    let doc_id = core.docs.structures[&space].doc_id;
    let stats = core.db.update_stats(doc_id).unwrap();
    assert!(
        stats.count <= 3,
        "log compacted, {} updates left",
        stats.count
    );
    assert!(core.db.snapshot_get(doc_id).unwrap().is_some());
    drop(core);

    let mut core = fx.open(2, 1);
    let view = tree(&mut core, space);
    assert_eq!(view.roots.len(), 10);
    assert_eq!(
        outline(&view),
        (0..10).map(|i| format!("P{i}\n")).collect::<String>()
    );
}

#[test]
fn page_index_is_repaired_on_open() {
    let fx = Fixture::new();
    let (space, a_id) = {
        let mut core = fx.open(1, 0);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = spaces(&mut core)[0].id;
        let a = create_page(&mut core, space, None, "A");
        create_page(&mut core, space, None, "B");
        // Damage the derived index.
        core.db.pages_index_delete(&[a.id]).unwrap();
        assert!(core.db.pages_index_get(a.id).unwrap().is_none());
        (space, a.id)
    };
    let mut core = fx.open(2, 0);
    let row = core.db.pages_index_get(a_id).unwrap().expect("rebuilt");
    assert_eq!(row.space_id, space);
    assert_eq!(row.title, "A");
    assert_eq!(page(&mut core, a_id).title, "A");
}

#[test]
fn milestone_one_database_gets_an_account_document_on_open() {
    // A workspace without documents (what Milestone 1 produced) must open and be
    // usable: the account document is created lazily.
    let fx = Fixture::new();
    {
        let mut core = fx.open(1, 0);
        create_workspace(&mut core, "Home", None);
        let account_doc_id = core.docs.account.as_ref().unwrap().doc_id;
        core.db.doc_delete(account_doc_id).unwrap();
    }
    let mut core = fx.open(2, 0);
    assert!(spaces(&mut core).is_empty());
    let s = create_space(&mut core, "Later");
    assert_eq!(spaces(&mut core)[0].id, s.id);
}

#[test]
fn a_thousand_pages_across_five_spaces_reopen_quickly() {
    let fx = Fixture::new();
    {
        let mut core = fx.open(1, 0);
        create_workspace(&mut core, "Big", None);
        for s in 0..5 {
            let space = create_space(&mut core, &format!("Space {s}"));
            let mut parents = Vec::new();
            for p in 0..200 {
                let parent = if p % 4 == 0 || parents.is_empty() {
                    None
                } else {
                    Some(parents[p % parents.len()])
                };
                let info = create_page(&mut core, space.id, parent, &format!("Page {s}-{p}"));
                parents.push(info.id);
            }
        }
    }
    let started = Instant::now();
    let mut core = fx.open(2, 0);
    let list = spaces(&mut core);
    let elapsed = started.elapsed();
    assert_eq!(list.len(), 5);
    assert_eq!(list.iter().map(|s| s.page_count).sum::<u32>(), 1_000);
    let budget_ms: u128 = if cfg!(debug_assertions) { 2_000 } else { 200 };
    assert!(
        elapsed.as_millis() < budget_ms,
        "reopen took {elapsed:?}, budget {budget_ms} ms"
    );
}
