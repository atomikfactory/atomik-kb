//! Favourites through `dispatch` (ADR-0029, section 2): the tree lists them in order, adding and
//! removing are idempotent, moves, the trash, purges, undo, a restart and compaction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::DocQueries;
use kb_types::{ApiError, ErrorCode, Event, PageId, Request, Response, SpaceId};

use crate::CompactionPolicy;
use crate::Core;
use crate::test_support::{Fixture, create_page, create_workspace, spaces, tree};

fn set(core: &mut Core, page_id: PageId, favorite: bool) -> Result<Response, ApiError> {
    core.dispatch(Request::PageFavoriteSet { page_id, favorite })
}

fn move_to(core: &mut Core, page_id: PageId, before: Option<PageId>) -> Result<Response, ApiError> {
    core.dispatch(Request::PageFavoriteMove { page_id, before })
}

fn favorites(core: &mut Core, space_id: SpaceId) -> Vec<PageId> {
    tree(core, space_id).favorites
}

fn field_of(err: &ApiError) -> Option<&str> {
    err.details.as_ref()?.get("field")?.as_str()
}

/// A workspace with one space and three live root pages `A`, `B`, `C`.
fn setup(core: &mut Core) -> (SpaceId, [PageId; 3]) {
    create_workspace(core, "Home", Some("Personal"));
    let space = spaces(core)[0].id;
    let a = create_page(core, space, None, "A").id;
    let b = create_page(core, space, None, "B").id;
    let c = create_page(core, space, None, "C").id;
    core.drain_events();
    (space, [a, b, c])
}

#[test]
fn the_tree_lists_the_favourites_in_order_and_each_change_announces_the_structure() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, c]) = setup(&mut core);
    assert!(favorites(&mut core, space).is_empty());

    assert_eq!(set(&mut core, c, true).unwrap(), Response::Ok);
    assert_eq!(
        core.drain_events(),
        vec![Event::StructureChanged { space_id: space }]
    );
    set(&mut core, a, true).unwrap();
    set(&mut core, b, true).unwrap();
    assert_eq!(favorites(&mut core, space), [c, a, b], "in the order added");

    assert_eq!(move_to(&mut core, b, Some(c)).unwrap(), Response::Ok);
    assert_eq!(
        core.drain_events()
            .into_iter()
            .filter(|event| *event == Event::StructureChanged { space_id: space })
            .count(),
        3,
        "two more adds and the move (the first add's event was drained above)"
    );
    assert_eq!(favorites(&mut core, space), [b, c, a]);
    move_to(&mut core, b, None).unwrap();
    assert_eq!(favorites(&mut core, space), [c, a, b]);

    // The page list itself is untouched: the tree's order does not depend on favourites.
    let view = tree(&mut core, space);
    assert_eq!(
        view.roots.iter().map(|n| n.page.id).collect::<Vec<_>>(),
        [a, b, c]
    );
}

#[test]
fn adding_and_removing_are_idempotent() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, _c]) = setup(&mut core);
    set(&mut core, a, true).unwrap();
    set(&mut core, b, true).unwrap();
    assert_eq!(set(&mut core, a, true).unwrap(), Response::Ok);
    assert_eq!(favorites(&mut core, space), [a, b], "keeps its place");
    assert_eq!(set(&mut core, a, false).unwrap(), Response::Ok);
    assert_eq!(set(&mut core, a, false).unwrap(), Response::Ok);
    assert_eq!(favorites(&mut core, space), [b]);
    set(&mut core, b, false).unwrap();
    assert!(favorites(&mut core, space).is_empty());
    let view = serde_json::to_value(tree(&mut core, space)).unwrap();
    assert!(view.get("favorites").is_none(), "an empty list is omitted");
}

#[test]
fn an_unknown_page_is_not_found_and_a_trashed_one_cannot_be_added() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, _c]) = setup(&mut core);
    let unknown = PageId::from_random_bytes([9; 16]);
    for favorite in [true, false] {
        let err = set(&mut core, unknown, favorite).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "set {favorite}");
    }
    assert_eq!(
        move_to(&mut core, unknown, None).unwrap_err().code,
        ErrorCode::NotFound
    );

    set(&mut core, a, true).unwrap();
    core.dispatch(Request::PageTrash { page_id: a }).unwrap();
    core.dispatch(Request::PageTrash { page_id: b }).unwrap();
    for page in [a, b] {
        let err = set(&mut core, page, true).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        assert_eq!(field_of(&err), Some("pageId"));
    }
    assert!(favorites(&mut core, space).is_empty());
    // Removing works for a trashed page, and takes the hidden entry away for good.
    assert_eq!(set(&mut core, a, false).unwrap(), Response::Ok);
    core.dispatch(Request::PageRestore { page_id: a }).unwrap();
    assert!(
        favorites(&mut core, space).is_empty(),
        "removed while in the trash: not back after the restore"
    );
}

#[test]
fn a_move_goes_in_front_of_a_favourite_or_last_and_refuses_the_others() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, c]) = setup(&mut core);
    set(&mut core, a, true).unwrap();
    set(&mut core, b, true).unwrap();

    let err = move_to(&mut core, c, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "not a favourite");
    assert_eq!(field_of(&err), Some("pageId"));
    let err = move_to(&mut core, a, Some(c)).unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidInput,
        "`before` is not a favourite"
    );
    assert_eq!(field_of(&err), Some("before"));
    let err = move_to(&mut core, a, Some(PageId::from_random_bytes([9; 16]))).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(field_of(&err), Some("before"));
    assert_eq!(favorites(&mut core, space), [a, b], "nothing changed");

    // Moving in place, or in front of itself, is accepted and changes nothing.
    assert_eq!(move_to(&mut core, a, Some(b)).unwrap(), Response::Ok);
    assert_eq!(move_to(&mut core, a, Some(a)).unwrap(), Response::Ok);
    assert_eq!(move_to(&mut core, b, None).unwrap(), Response::Ok);
    assert_eq!(favorites(&mut core, space), [a, b]);
    move_to(&mut core, b, Some(a)).unwrap();
    assert_eq!(favorites(&mut core, space), [b, a]);
}

#[test]
fn a_favourite_of_another_space_is_not_a_place_to_go() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, _c]) = setup(&mut core);
    let other_space = match core
        .dispatch(Request::SpaceCreate {
            name: "Other".into(),
            icon: None,
        })
        .unwrap()
    {
        Response::Space(s) => s.id,
        other => panic!("unexpected {other:?}"),
    };
    let elsewhere = create_page(&mut core, other_space, None, "Elsewhere").id;
    set(&mut core, a, true).unwrap();
    set(&mut core, b, true).unwrap();
    set(&mut core, elsewhere, true).unwrap();
    assert_eq!(favorites(&mut core, space), [a, b]);
    assert_eq!(favorites(&mut core, other_space), [elsewhere]);
    let err = move_to(&mut core, a, Some(elsewhere)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(field_of(&err), Some("before"));
}

#[test]
fn trashing_hides_restoring_shows_and_purging_removes() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, c]) = setup(&mut core);
    let child = create_page(&mut core, space, Some(b), "B child").id;
    for page in [a, child, c] {
        set(&mut core, page, true).unwrap();
    }
    assert_eq!(favorites(&mut core, space), [a, child, c]);

    // A child hides with its trashed parent and returns with it.
    core.dispatch(Request::PageTrash { page_id: b }).unwrap();
    assert_eq!(favorites(&mut core, space), [a, c]);
    core.dispatch(Request::PageRestore { page_id: b }).unwrap();
    assert_eq!(favorites(&mut core, space), [a, child, c], "same place");

    // Purging takes the entries of the page and of its subtree.
    core.dispatch(Request::PageTrash { page_id: b }).unwrap();
    core.dispatch(Request::PagePurge { page_id: b }).unwrap();
    assert_eq!(favorites(&mut core, space), [a, c]);
    // The entry is gone from the document, not only hidden: a purged id is not a favourite any
    // more, whatever happens to the tree.
    let err = set(&mut core, child, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    set(&mut core, a, false).unwrap();
    set(&mut core, c, false).unwrap();
    assert!(favorites(&mut core, space).is_empty());
}

#[test]
fn favourites_are_not_undo_steps() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, c]) = setup(&mut core);
    set(&mut core, a, true).unwrap();
    set(&mut core, c, true).unwrap();
    move_to(&mut core, c, Some(a)).unwrap();
    set(&mut core, b, true).unwrap();
    set(&mut core, b, false).unwrap();
    assert_eq!(favorites(&mut core, space), [c, a]);

    // Undo walks back through the page creations, never through a favourite.
    match core
        .dispatch(Request::StructureUndo { space_id: space })
        .unwrap()
    {
        Response::Tree(view) => {
            assert_eq!(view.roots.len(), 2, "page C's creation was undone");
            assert_eq!(
                view.favorites,
                [a],
                "the entry of the vanished page is ignored"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    match core
        .dispatch(Request::StructureRedo { space_id: space })
        .unwrap()
    {
        Response::Tree(view) => {
            assert_eq!(view.roots.len(), 3);
            assert_eq!(view.favorites, [c, a], "redo changed no favourite");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn favourites_survive_a_restart_and_compaction() {
    let fx = Fixture::new();
    let (space, order) = {
        let mut core = fx.open(1, 100);
        core.set_compaction_policy(CompactionPolicy {
            max_updates: 2,
            max_bytes: 1 << 30,
        });
        let (space, [a, b, c]) = setup(&mut core);
        for page in [b, c, a] {
            set(&mut core, page, true).unwrap();
        }
        move_to(&mut core, a, Some(b)).unwrap();
        assert_eq!(favorites(&mut core, space), [a, b, c]);
        (space, [a, b, c])
    };
    let mut core = fx.open(2, 500);
    assert_eq!(favorites(&mut core, space), order);
    // And the next write goes on from the reloaded document.
    set(&mut core, order[0], false).unwrap();
    assert_eq!(favorites(&mut core, space), [order[1], order[2]]);
    drop(core);
    let mut core = fx.open(3, 900);
    assert_eq!(favorites(&mut core, space), [order[1], order[2]]);
}

#[test]
fn a_favourite_write_is_persisted_as_a_local_structure_update() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 100);
    let (space, [a, b, _c]) = setup(&mut core);
    let doc_id = core.docs.structures[&space].doc_id;
    let before = core.db.update_stats(doc_id).unwrap().count;
    set(&mut core, a, true).unwrap();
    let after = core.db.update_stats(doc_id).unwrap().count;
    assert_eq!(after, before + 1, "one update appended");
    // An idempotent no-op appends nothing.
    set(&mut core, a, true).unwrap();
    set(&mut core, b, false).unwrap();
    assert_eq!(core.db.update_stats(doc_id).unwrap().count, after);
}
