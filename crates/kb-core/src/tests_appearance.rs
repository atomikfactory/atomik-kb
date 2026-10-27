//! Page appearance (ADR-0040): `PageSetAppearance` through the handler (the three fields, absent
//! ones, the neutral values that clear), what `PageInfo` and the tree report, the event, the undo
//! step, persistence, and that the lock is a guard of the UI: nothing in the core refuses a write to
//! a locked page. Moves, duplicates and the remote import have their appearance cases in
//! `tests_space_move.rs`, `tests_duplicate.rs`, `tests_sync.rs` and `tests_mcp.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::{
    ApiError, ErrorCode, Event, PageFont, PageId, PageInfo, Request, Response, SpaceId,
};

use crate::Core;
use crate::test_support::{Fixture, create_page, create_workspace, page, spaces, tree};

const T0: i64 = 1_000;

fn setup() -> (Fixture, Core, SpaceId) {
    let fixture = Fixture::new();
    let mut core = fixture.open(1, T0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    (fixture, core, space)
}

fn try_set(
    core: &mut Core,
    page_id: PageId,
    font: Option<PageFont>,
    small_text: Option<bool>,
    locked: Option<bool>,
) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageSetAppearance {
        page_id,
        font,
        small_text,
        locked,
    })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn set(
    core: &mut Core,
    page_id: PageId,
    font: Option<PageFont>,
    small_text: Option<bool>,
    locked: Option<bool>,
) -> PageInfo {
    try_set(core, page_id, font, small_text, locked).unwrap()
}

fn look(info: &PageInfo) -> (Option<PageFont>, bool, bool) {
    (info.font, info.small_text, info.locked)
}

fn call(core: &mut Core, request: Request) {
    core.dispatch(request).unwrap();
}

fn undo(core: &mut Core, space_id: SpaceId, redo: bool) {
    call(
        core,
        if redo {
            Request::StructureRedo { space_id }
        } else {
            Request::StructureUndo { space_id }
        },
    );
}

#[test]
fn each_part_is_set_read_back_in_the_page_and_the_tree_and_cleared_by_its_neutral_value() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Notes").id;
    assert_eq!(look(&page(&mut core, page_id)), (None, false, false));

    for font in [PageFont::Serif, PageFont::Mono] {
        let info = set(&mut core, page_id, Some(font), None, None);
        assert_eq!(look(&info), (Some(font), false, false));
        assert_eq!(page(&mut core, page_id), info);
    }
    let info = set(&mut core, page_id, None, Some(true), None);
    assert_eq!(look(&info), (Some(PageFont::Mono), true, false));
    let info = set(&mut core, page_id, None, None, Some(true));
    assert_eq!(look(&info), (Some(PageFont::Mono), true, true));
    assert_eq!(page(&mut core, page_id), info);

    // The tree carries the same, so the sidebar and the page view see a lock at once.
    let view = tree(&mut core, space);
    assert_eq!(
        look(&view.roots[0].page),
        (Some(PageFont::Mono), true, true)
    );

    // Absent fields leave the page as it is, even when nothing at all is asked.
    assert_eq!(set(&mut core, page_id, None, None, None), info);

    // The neutral values clear.
    let info = set(&mut core, page_id, Some(PageFont::Default), None, None);
    assert_eq!(look(&info), (None, true, true));
    let info = set(&mut core, page_id, None, Some(false), Some(false));
    assert_eq!(look(&info), (None, false, false));
    assert_eq!(page(&mut core, page_id), info);

    // All three at once; the answer is the page.
    let info = set(
        &mut core,
        page_id,
        Some(PageFont::Serif),
        Some(true),
        Some(true),
    );
    assert_eq!(look(&info), (Some(PageFont::Serif), true, true));
    assert_eq!(info.title, "Notes");
    // The appearance is not the icon or the cover.
    assert_eq!((info.icon, info.cover), (None, None));
}

#[test]
fn a_change_stamps_the_page_and_announces_a_structure_change_and_the_same_value_does_not_stamp() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Notes").id;
    let before = page(&mut core, page_id).updated_at;
    core.drain_events();

    set(&mut core, page_id, None, None, Some(true));
    assert!(
        core.drain_events()
            .contains(&Event::StructureChanged { space_id: space }),
        "the lock reaches the open page through the structure change"
    );
    let stamped = page(&mut core, page_id).updated_at;
    assert!(stamped >= before);

    // The same value again changes nothing about the page.
    let again = set(&mut core, page_id, None, None, Some(true));
    assert_eq!(again.updated_at, stamped);
    assert!(again.locked);
}

#[test]
fn an_unknown_page_is_not_found_and_a_trashed_page_keeps_its_appearance() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Notes").id;
    let err = try_set(
        &mut core,
        PageId::from_bytes([9; 16]),
        Some(PageFont::Serif),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    call(&mut core, Request::PageTrash { page_id });
    let info = set(
        &mut core,
        page_id,
        Some(PageFont::Mono),
        Some(true),
        Some(true),
    );
    assert_eq!(look(&info), (Some(PageFont::Mono), true, true));
    call(&mut core, Request::PageRestore { page_id });
    assert_eq!(
        look(&page(&mut core, page_id)),
        (Some(PageFont::Mono), true, true)
    );
}

#[test]
fn undo_and_redo_take_and_bring_back_the_whole_change_and_it_survives_a_restart() {
    let (fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Notes").id;
    set(
        &mut core,
        page_id,
        Some(PageFont::Serif),
        Some(true),
        Some(true),
    );
    // One step: a single undo clears all three.
    undo(&mut core, space, false);
    assert_eq!(look(&page(&mut core, page_id)), (None, false, false));
    undo(&mut core, space, true);
    assert_eq!(
        look(&page(&mut core, page_id)),
        (Some(PageFont::Serif), true, true)
    );
    drop(core);

    // The documents hold it: another run of the core reads the same.
    let mut core = fx.open(2, T0 + 5_000);
    assert_eq!(
        look(&page(&mut core, page_id)),
        (Some(PageFont::Serif), true, true)
    );
    let view = tree(&mut core, space);
    assert_eq!(
        look(&view.roots[0].page),
        (Some(PageFont::Serif), true, true)
    );
}

#[test]
fn a_lock_is_a_guard_of_the_ui_and_the_core_refuses_nothing_on_a_locked_page() {
    let (_fx, mut core, space) = setup();
    let other = create_page(&mut core, space, None, "Other").id;
    let page_id = create_page(&mut core, space, None, "Locked").id;
    let child = create_page(&mut core, space, Some(page_id), "Child").id;
    set(&mut core, page_id, None, None, Some(true));
    set(&mut core, child, None, None, Some(true));

    call(
        &mut core,
        Request::PageRename {
            page_id,
            title: "Renamed".into(),
        },
    );
    call(
        &mut core,
        Request::PageSetIcon {
            page_id,
            icon: Some("🔒".into()),
        },
    );
    call(
        &mut core,
        Request::PageSetCover {
            page_id,
            cover: Some("color:blue".into()),
            cover_y: None,
        },
    );
    call(
        &mut core,
        Request::PageMove {
            page_id,
            parent_id: Some(other),
            index: 0,
        },
    );
    let info = page(&mut core, page_id);
    assert_eq!(
        (info.title.as_str(), info.icon.as_deref(), info.locked),
        ("Renamed", Some("🔒"), true)
    );
    assert_eq!(info.cover.as_deref(), Some("color:blue"));
    assert_eq!(info.parent_id, Some(other));

    // The duplicate of a locked page is locked too, sub-pages included.
    let copy = match core.dispatch(Request::PageDuplicate { page_id }).unwrap() {
        Response::Page(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert!(copy.locked);

    // Trashing, restoring and purging go through.
    call(&mut core, Request::PageTrash { page_id });
    assert!(page(&mut core, page_id).locked);
    call(&mut core, Request::PageRestore { page_id });
    call(&mut core, Request::PageTrash { page_id });
    call(&mut core, Request::PagePurge { page_id });
    let err = core.dispatch(Request::PageGet { page_id }).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}
