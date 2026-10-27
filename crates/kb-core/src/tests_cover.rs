//! Page covers (ADR-0040): the request through the handler (the grammar, the position, the check
//! of an image), and the retention of the image a cover uses (`icon_refs`, schema v16): a cover keeps
//! its attachment out of the garbage collector, replacing or clearing it or purging the page starts
//! the retention clock, the same image as icon and cover is one reference, and undo, redo, the
//! rebuild paths and the open-time check keep the rows in step with the documents. Moves, duplicates
//! and the remote import have their cover cases in `tests_space_move.rs`, `tests_duplicate.rs` and
//! `tests_sync.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::{AttachmentQueries, IconRefQueries};
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, ErrorCode, Event, PageId, PageInfo, Request, Response,
    SpaceId,
};

use crate::Core;
use crate::test_support::{Fixture, create_page, create_space, create_workspace, page, spaces};

const DAY: i64 = 86_400_000;
const T0: i64 = 1_000;

fn setup() -> (Fixture, Core, SpaceId) {
    let fixture = Fixture::new();
    let mut core = fixture.open(1, T0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    (fixture, core, space)
}

/// Uploads a small file of the given MIME type into the space and returns its id.
fn upload(core: &mut Core, space_id: SpaceId, name: &str, mime: &str) -> AttachmentId {
    let id = match core
        .dispatch(Request::AttachmentBegin {
            space_id,
            name: name.into(),
            mime: mime.into(),
        })
        .unwrap()
    {
        Response::AttachmentStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    };
    core.attachment_write(id, name.as_bytes()).unwrap();
    match core
        .dispatch(Request::AttachmentFinish { attachment_id: id })
        .unwrap()
    {
        Response::Attachment(info) => info.attachment_id,
        other => panic!("unexpected {other:?}"),
    }
}

fn image(core: &mut Core, space_id: SpaceId, name: &str) -> AttachmentId {
    upload(core, space_id, name, "image/png")
}

fn attachment(core: &mut Core, attachment_id: AttachmentId) -> Result<AttachmentInfo, ApiError> {
    match core.dispatch(Request::AttachmentGet { attachment_id })? {
        Response::Attachment(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

/// `(references, unreferenced_since)` of an attachment that exists.
fn state(core: &mut Core, attachment_id: AttachmentId) -> (u32, Option<i64>) {
    let info = attachment(core, attachment_id).unwrap();
    (info.references, info.unreferenced_since)
}

fn collect(core: &mut Core) -> u32 {
    match core.dispatch(Request::AttachmentGc).unwrap() {
        Response::AttachmentGc(summary) => summary.purged,
        other => panic!("unexpected {other:?}"),
    }
}

fn cover_of(id: AttachmentId) -> String {
    format!("attachment:{id}")
}

fn try_set_cover(
    core: &mut Core,
    page_id: PageId,
    cover: Option<&str>,
    cover_y: Option<u8>,
) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageSetCover {
        page_id,
        cover: cover.map(str::to_owned),
        cover_y,
    })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn set_cover(core: &mut Core, page_id: PageId, cover: Option<&str>) -> PageInfo {
    try_set_cover(core, page_id, cover, None).unwrap()
}

fn set_icon(core: &mut Core, page_id: PageId, icon: Option<&str>) {
    core.dispatch(Request::PageSetIcon {
        page_id,
        icon: icon.map(str::to_owned),
    })
    .unwrap();
}

fn call(core: &mut Core, request: Request) {
    core.dispatch(request).unwrap();
}

fn undo(core: &mut Core, space_id: SpaceId, redo: bool) {
    let request = if redo {
        Request::StructureRedo { space_id }
    } else {
        Request::StructureUndo { space_id }
    };
    call(core, request);
}

fn field_of(err: &ApiError) -> Option<&str> {
    err.details.as_ref()?.get("field")?.as_str()
}

/// The refusal of a cover: `InvalidInput` on the field `cover`.
fn refused(core: &mut Core, page_id: PageId, cover: &str) -> ApiError {
    let err = try_set_cover(core, page_id, Some(cover), None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{cover}: {err}");
    assert_eq!(field_of(&err), Some("cover"), "{cover}: {err}");
    err
}

fn covers_of(core: &mut Core, space: SpaceId) -> Vec<(PageId, AttachmentId)> {
    core.db.icon_refs_of_space(space).unwrap()
}

// ----- the request ----------------------------------------------------------------------------

#[test]
fn every_form_of_cover_is_set_read_back_repositioned_and_cleared_on_a_page() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    assert_eq!(page(&mut core, page_id).cover, None);
    let photo = image(&mut core, space, "photo.png");
    for cover in [
        cover_of(photo).as_str(),
        "color:blue",
        "color:gray",
        "gradient:sunset",
    ] {
        let info = set_cover(&mut core, page_id, Some(cover));
        assert_eq!(info.cover.as_deref(), Some(cover));
        assert_eq!(info.cover_y, None);
        assert_eq!(page(&mut core, page_id), info);
    }

    // The position goes with the cover; 50 is the default and reads as none.
    let info = try_set_cover(&mut core, page_id, Some("gradient:sunset"), Some(30)).unwrap();
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some("gradient:sunset"), Some(30))
    );
    for y in [0u8, 100] {
        let info = try_set_cover(&mut core, page_id, Some("color:red"), Some(y)).unwrap();
        assert_eq!(info.cover_y, Some(y));
    }
    let info = try_set_cover(&mut core, page_id, Some("color:red"), Some(50)).unwrap();
    assert_eq!(info.cover_y, None);

    // A position alone changes only the position of the cover the page has.
    let info = try_set_cover(&mut core, page_id, None, Some(20)).unwrap();
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some("color:red"), Some(20))
    );
    let info = try_set_cover(&mut core, page_id, None, Some(50)).unwrap();
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some("color:red"), None)
    );
    // A new cover without a position starts again at the default.
    try_set_cover(&mut core, page_id, None, Some(70)).unwrap();
    let info = set_cover(&mut core, page_id, Some("color:green"));
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some("color:green"), None)
    );

    // Trimmed like an icon; the same cover again is not an error.
    let spaced = format!("  {}  ", cover_of(photo));
    assert_eq!(
        set_cover(&mut core, page_id, Some(&spaced)).cover,
        Some(cover_of(photo))
    );
    set_cover(&mut core, page_id, Some(&cover_of(photo)));

    // Neither clears both, and clearing again is fine.
    try_set_cover(&mut core, page_id, Some("color:red"), Some(10)).unwrap();
    let info = set_cover(&mut core, page_id, None);
    assert_eq!((info.cover, info.cover_y), (None, None));
    assert_eq!(page(&mut core, page_id).cover, None);
    set_cover(&mut core, page_id, None);

    // The cover is not the icon.
    set_icon(&mut core, page_id, Some("🌱"));
    assert_eq!(page(&mut core, page_id).cover, None);
    set_cover(&mut core, page_id, Some("color:blue"));
    set_icon(&mut core, page_id, None);
    assert_eq!(
        page(&mut core, page_id).cover.as_deref(),
        Some("color:blue")
    );
}

#[test]
fn a_cover_can_be_set_on_a_trashed_page_and_stamps_the_edit() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let before = page(&mut core, page_id).updated_at;
    set_cover(&mut core, page_id, Some("color:blue"));
    assert!(page(&mut core, page_id).updated_at >= before);
    call(&mut core, Request::PageTrash { page_id });
    let info = try_set_cover(&mut core, page_id, Some("color:red"), Some(10)).unwrap();
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some("color:red"), Some(10))
    );
    call(&mut core, Request::PageRestore { page_id });
    assert_eq!(page(&mut core, page_id).cover.as_deref(), Some("color:red"));
}

#[test]
fn a_malformed_cover_or_position_is_refused_and_writes_nothing() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    set_cover(&mut core, page_id, Some("color:blue"));
    for bad in [
        "",
        "   ",
        "🌱",
        "icon:book",
        "blue",
        "https://example.com/a.png",
        "color:teal",
        "color:",
        "gradient:",
        "gradient:Sunset",
        "gradient:sun set",
        "attachment:",
        "attachment:5F0C1D2E-3A4B-4C5D-8E6F-0A1B2C3D4E5F",
        "attachment:not-a-uuid",
    ] {
        refused(&mut core, page_id, bad);
    }
    refused(&mut core, page_id, &format!("gradient:{}", "x".repeat(33)));

    // A position out of range is refused on its own field, with a cover or alone.
    for (cover, y) in [(Some("color:blue"), 101u8), (None, 255)] {
        let err = try_set_cover(&mut core, page_id, cover, Some(y)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{y}: {err}");
        assert_eq!(field_of(&err), Some("coverY"), "{y}: {err}");
    }
    // A position alone needs a cover to move.
    let bare = create_page(&mut core, space, None, "Bare").id;
    let err = try_set_cover(&mut core, bare, None, Some(30)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
    assert_eq!(field_of(&err), Some("coverY"), "{err}");
    assert_eq!(page(&mut core, bare).cover_y, None);

    // An unknown page is not found.
    let err = try_set_cover(
        &mut core,
        PageId::from_random_bytes([0x66; 16]),
        Some("color:blue"),
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "{err}");

    let info = page(&mut core, page_id);
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some("color:blue"), None),
        "nothing was written"
    );
}

#[test]
fn an_image_cover_must_be_an_image_of_the_pages_space_and_not_purged() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let work = create_space(&mut core, "Work").id;

    // Another space's image.
    let elsewhere = image(&mut core, work, "work.png");
    let err = refused(&mut core, page_id, &cover_of(elsewhere));
    assert!(err.message.contains("another space"), "{err}");

    // A file that is not an image.
    let pdf = upload(&mut core, space, "report.pdf", "application/pdf");
    let err = refused(&mut core, page_id, &cover_of(pdf));
    assert!(err.message.contains("not an image"), "{err}");
    let text = upload(&mut core, space, "notes.txt", "text/plain");
    refused(&mut core, page_id, &cover_of(text));

    // An attachment nobody knows.
    let unknown = AttachmentId::from_random_bytes([0x77; 16]);
    let err = refused(&mut core, page_id, &cover_of(unknown));
    assert!(err.message.contains("unknown or deleted"), "{err}");

    // A purged one.
    let gone = image(&mut core, space, "gone.png");
    call(
        &mut core,
        Request::AttachmentDelete {
            attachment_id: gone,
        },
    );
    let err = refused(&mut core, page_id, &cover_of(gone));
    assert!(err.message.contains("unknown or deleted"), "{err}");

    // Other image types are fine, in any letter case.
    for (n, mime) in ["image/jpeg", "image/gif", "image/svg+xml", "IMAGE/PNG"]
        .into_iter()
        .enumerate()
    {
        let id = upload(&mut core, space, &format!("pic{n}"), mime);
        assert_eq!(
            set_cover(&mut core, page_id, Some(&cover_of(id))).cover,
            Some(cover_of(id)),
            "{mime}"
        );
    }

    // A refusal changes nothing: the cover and the references are those of the last good write.
    let last = page(&mut core, page_id).cover.unwrap();
    refused(&mut core, page_id, &cover_of(pdf));
    assert_eq!(
        page(&mut core, page_id).cover.as_deref(),
        Some(last.as_str())
    );
    assert_eq!(state(&mut core, pdf).0, 0);
    assert_eq!(state(&mut core, elsewhere).0, 0);

    // The reposition of a cover whose image was deleted still works (it names no new image).
    call(
        &mut core,
        Request::AttachmentDelete {
            attachment_id: attachment_id_of(&last),
        },
    );
    let info = try_set_cover(&mut core, page_id, None, Some(10)).unwrap();
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some(last.as_str()), Some(10))
    );
}

fn attachment_id_of(cover: &str) -> AttachmentId {
    crate::validate::cover_attachment(cover).unwrap()
}

// ----- retention ------------------------------------------------------------------------------

#[test]
fn a_cover_keeps_its_image_out_of_the_collector_and_replacing_or_clearing_it_starts_the_clock() {
    let (fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    let b = image(&mut core, space, "b.png");
    assert_eq!(state(&mut core, a), (0, Some(T0)), "uploaded, not used yet");

    core.drain_events();
    set_cover(&mut core, page_id, Some(&cover_of(a)));
    assert_eq!(state(&mut core, a), (1, None), "the cover references it");
    assert_eq!(state(&mut core, b), (0, Some(T0)), "the other is untouched");
    assert!(
        core.drain_events()
            .contains(&Event::AttachmentsChanged { space_id: space }),
        "the bookkeeping announces the attachments that changed"
    );
    // The position changes nothing about the references.
    try_set_cover(&mut core, page_id, None, Some(25)).unwrap();
    assert_eq!(state(&mut core, a), (1, None));
    drop(core);

    // Well past the retention: the image of a cover stays, the unused one goes.
    let t1 = T0 + 40 * DAY;
    let mut core = fx.open(2, t1);
    assert_eq!(
        attachment(&mut core, b).unwrap_err().code,
        ErrorCode::NotFound,
        "collected at open"
    );
    assert_eq!(state(&mut core, a), (1, None));
    assert_eq!(collect(&mut core), 0);

    // Replacing the cover starts the clock of the old image; the new one is referenced.
    let c = image(&mut core, space, "c.png");
    set_cover(&mut core, page_id, Some(&cover_of(c)));
    assert_eq!(state(&mut core, a), (0, Some(t1)));
    assert_eq!(state(&mut core, c), (1, None));
    drop(core);
    let mut core = fx.open(3, t1 + 29 * DAY);
    assert!(attachment(&mut core, a).is_ok(), "within the retention");
    // Putting it back in time keeps it (the clock stops).
    set_cover(&mut core, page_id, Some(&cover_of(a)));
    assert_eq!(state(&mut core, a), (1, None));
    assert_eq!(state(&mut core, c), (0, Some(t1 + 29 * DAY)));

    // A colour or a gradient, or none, releases it.
    set_cover(&mut core, page_id, Some("gradient:sunset"));
    assert_eq!(state(&mut core, a), (0, Some(t1 + 29 * DAY)));
    set_cover(&mut core, page_id, Some(&cover_of(a)));
    set_cover(&mut core, page_id, None);
    let t2 = t1 + 29 * DAY;
    assert_eq!(state(&mut core, a), (0, Some(t2)));
    drop(core);
    let mut core = fx.open(4, t2 + 31 * DAY);
    for id in [a, c] {
        assert_eq!(
            attachment(&mut core, id).unwrap_err().code,
            ErrorCode::NotFound,
            "{id}"
        );
    }
}

#[test]
fn the_icon_and_the_cover_of_a_page_hold_their_images_independently() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let logo = image(&mut core, space, "logo.png");
    let banner = image(&mut core, space, "banner.png");
    set_icon(&mut core, page_id, Some(&cover_of(logo)));
    set_cover(&mut core, page_id, Some(&cover_of(banner)));
    assert_eq!(state(&mut core, logo), (1, None));
    assert_eq!(state(&mut core, banner), (1, None));
    let mut rows = vec![(page_id, logo), (page_id, banner)];
    rows.sort();
    assert_eq!(covers_of(&mut core, space), rows, "two rows for the page");

    // Changing the icon releases only the logo; clearing the cover releases only the banner.
    set_icon(&mut core, page_id, Some("🌱"));
    assert_eq!(state(&mut core, logo), (0, Some(T0)));
    assert_eq!(state(&mut core, banner), (1, None));
    set_icon(&mut core, page_id, Some(&cover_of(logo)));
    set_cover(&mut core, page_id, None);
    assert_eq!(state(&mut core, banner), (0, Some(T0)));
    assert_eq!(state(&mut core, logo), (1, None));
    assert_eq!(covers_of(&mut core, space), [(page_id, logo)]);
}

#[test]
fn the_same_image_as_icon_and_cover_is_one_reference_and_stays_while_either_holds_it() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let other = create_page(&mut core, space, None, "Other").id;
    let shared = image(&mut core, space, "shared.png");
    set_icon(&mut core, page_id, Some(&cover_of(shared)));
    set_cover(&mut core, page_id, Some(&cover_of(shared)));
    assert_eq!(
        state(&mut core, shared),
        (1, None),
        "one reference per page"
    );
    assert_eq!(covers_of(&mut core, space), [(page_id, shared)]);

    // Dropping one use leaves the other holding it.
    set_icon(&mut core, page_id, None);
    assert_eq!(state(&mut core, shared), (1, None));
    set_icon(&mut core, page_id, Some(&cover_of(shared)));
    set_cover(&mut core, page_id, Some("color:blue"));
    assert_eq!(state(&mut core, shared), (1, None));
    set_icon(&mut core, page_id, None);
    assert_eq!(state(&mut core, shared), (0, Some(T0)));

    // Two pages with the same cover are two references.
    set_cover(&mut core, page_id, Some(&cover_of(shared)));
    set_cover(&mut core, other, Some(&cover_of(shared)));
    assert_eq!(state(&mut core, shared), (2, None));
}

#[test]
fn a_trashed_page_keeps_its_cover_image_and_a_purge_releases_it() {
    let (fx, mut core, space) = setup();
    let first = create_page(&mut core, space, None, "First").id;
    let second = create_page(&mut core, space, None, "Second").id;
    let child = create_page(&mut core, space, Some(second), "Child").id;
    let shared = image(&mut core, space, "shared.png");
    let own = image(&mut core, space, "own.png");
    set_cover(&mut core, first, Some(&cover_of(shared)));
    set_cover(&mut core, second, Some(&cover_of(shared)));
    set_cover(&mut core, child, Some(&cover_of(own)));
    assert_eq!(
        state(&mut core, shared),
        (2, None),
        "one reference per page"
    );
    assert_eq!(state(&mut core, own), (1, None));

    // A trashed page (and the pages of a trashed subtree) still reference their images.
    call(&mut core, Request::PageTrash { page_id: second });
    assert_eq!(state(&mut core, shared), (2, None));
    assert_eq!(state(&mut core, own), (1, None));
    call(&mut core, Request::PageRestore { page_id: second });
    assert_eq!(state(&mut core, shared), (2, None));
    call(&mut core, Request::PageTrash { page_id: second });

    // Purging it (and its child) releases what only they held; the shared image keeps the other page.
    call(&mut core, Request::PagePurge { page_id: second });
    assert_eq!(state(&mut core, shared), (1, None));
    assert_eq!(state(&mut core, own), (0, Some(T0)));
    assert_eq!(
        covers_of(&mut core, space)
            .into_iter()
            .map(|(page, _)| page)
            .collect::<Vec<_>>(),
        [first]
    );
    drop(core);
    let mut core = fx.open(2, T0 + 31 * DAY);
    assert_eq!(
        attachment(&mut core, own).unwrap_err().code,
        ErrorCode::NotFound,
        "collected at open"
    );
    assert_eq!(state(&mut core, shared), (1, None));

    // Purging the last page that uses it releases the shared image too.
    call(&mut core, Request::PageTrash { page_id: first });
    call(&mut core, Request::PagePurge { page_id: first });
    assert_eq!(state(&mut core, shared), (0, Some(T0 + 31 * DAY)));
}

#[test]
fn undo_and_redo_of_a_cover_change_move_the_reference_and_the_position() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    let b = image(&mut core, space, "b.png");
    set_cover(&mut core, page_id, Some(&cover_of(a)));
    assert_eq!(state(&mut core, a), (1, None));

    // Undo takes the cover away: the image is unreferenced again; redo brings it back.
    undo(&mut core, space, false);
    assert_eq!(page(&mut core, page_id).cover, None);
    assert_eq!(state(&mut core, a), (0, Some(T0)));
    assert!(covers_of(&mut core, space).is_empty());
    undo(&mut core, space, true);
    assert_eq!(page(&mut core, page_id).cover, Some(cover_of(a)));
    assert_eq!(state(&mut core, a), (1, None));

    // A change from one image to another, undone and redone, swaps the references both ways.
    set_cover(&mut core, page_id, Some(&cover_of(b)));
    assert_eq!(state(&mut core, a), (0, Some(T0)));
    assert_eq!(state(&mut core, b), (1, None));
    undo(&mut core, space, false);
    assert_eq!(page(&mut core, page_id).cover, Some(cover_of(a)));
    assert_eq!(state(&mut core, a), (1, None));
    assert_eq!(state(&mut core, b), (0, Some(T0)));
    undo(&mut core, space, true);
    assert_eq!(state(&mut core, a), (0, Some(T0)));
    assert_eq!(state(&mut core, b), (1, None));

    // The position is part of the step: undo restores the one before.
    try_set_cover(&mut core, page_id, None, Some(80)).unwrap();
    assert_eq!(page(&mut core, page_id).cover_y, Some(80));
    undo(&mut core, space, false);
    let info = page(&mut core, page_id);
    assert_eq!((info.cover, info.cover_y), (Some(cover_of(b)), None));

    // Undoing the creation of the page takes its row away with it.
    for _ in 0..3 {
        undo(&mut core, space, false);
    }
    assert!(covers_of(&mut core, space).is_empty());
    assert_eq!(state(&mut core, b), (0, Some(T0)));
}

#[test]
fn deleting_a_purged_image_leaves_the_cover_naming_it_and_nothing_breaks() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    set_cover(&mut core, page_id, Some(&cover_of(a)));

    // The user deletes the image explicitly: the page still names it (a reader shows the default
    // banner), the row stays, and the next write of the structure is not troubled by it.
    call(&mut core, Request::AttachmentDelete { attachment_id: a });
    assert_eq!(page(&mut core, page_id).cover, Some(cover_of(a)));
    assert_eq!(covers_of(&mut core, space).len(), 1);
    call(
        &mut core,
        Request::PageRename {
            page_id,
            title: "Trip 2".into(),
        },
    );
    assert_eq!(
        attachment(&mut core, a).unwrap_err().code,
        ErrorCode::NotFound
    );
    // The page can have another cover again.
    set_cover(&mut core, page_id, Some("color:green"));
    assert!(covers_of(&mut core, space).is_empty());
}

// ----- derived rows: rebuild and open-time repair --------------------------------------------

#[test]
fn index_rebuild_restores_the_cover_references_and_drops_a_stale_row() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let other = create_page(&mut core, space, None, "Other").id;
    let a = image(&mut core, space, "a.png");
    let logo = image(&mut core, space, "logo.png");
    let stale = image(&mut core, space, "stale.png");
    set_cover(&mut core, page_id, Some(&cover_of(a)));
    set_icon(&mut core, page_id, Some(&cover_of(logo)));
    assert_eq!(state(&mut core, a), (1, None));

    // The derived rows are lost: the references go with them, and a rebuild brings them back.
    core.db.icon_refs_clear().unwrap();
    assert_eq!(attachment(&mut core, a).unwrap().references, 0);
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(state(&mut core, a), (1, None));
    assert_eq!(state(&mut core, logo), (1, None));

    // A row for an image the documents do not use (a stale one) is gone after a rebuild of the
    // space, and that image starts its clock.
    core.db
        .icon_refs_set(space, &[(other, vec![stale])])
        .unwrap();
    core.db
        .attachment_set_unreferenced_since(stale, None)
        .unwrap();
    assert_eq!(attachment(&mut core, stale).unwrap().references, 1);
    call(
        &mut core,
        Request::IndexRebuild {
            space_id: Some(space),
        },
    );
    assert_eq!(state(&mut core, stale), (0, Some(T0)));
    assert_eq!(state(&mut core, a), (1, None));
    let mut wanted = vec![(page_id, a), (page_id, logo)];
    wanted.sort();
    assert_eq!(
        covers_of(&mut core, space),
        wanted,
        "exactly what the documents say"
    );
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(state(&mut core, a), (1, None));
}

#[test]
fn the_open_time_check_fills_the_table_for_a_cover_the_rows_lack_and_repairs_it() {
    let (fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    let b = image(&mut core, space, "b.png");
    set_cover(&mut core, page_id, Some(&cover_of(a)));
    // A database migrated to v16 (or one that missed a write) has no row although the document
    // holds an image cover, and a stale bookkeeping state.
    core.db.icon_refs_clear().unwrap();
    core.db
        .attachment_set_unreferenced_since(a, Some(5))
        .unwrap();
    core.db.icon_refs_set(space, &[(page_id, vec![b])]).unwrap();
    core.db.attachment_set_unreferenced_since(b, None).unwrap();
    drop(core);

    let mut core = fx.open(2, T0 + 10);
    assert_eq!(
        covers_of(&mut core, space),
        [(page_id, a)],
        "rewritten from the structure document"
    );
    assert_eq!(state(&mut core, a), (1, None), "its clock is stopped");
    assert_eq!(
        state(&mut core, b),
        (0, Some(T0 + 10)),
        "the stale row's image is released"
    );
    drop(core);

    // A second open finds everything in step and changes nothing.
    let mut core = fx.open(3, T0 + 20);
    assert_eq!(covers_of(&mut core, space), [(page_id, a)]);
    assert_eq!(
        state(&mut core, b),
        (0, Some(T0 + 10)),
        "the clock is not restarted"
    );
}

#[test]
fn a_backup_restore_rebuilds_the_cover_references_from_the_documents() {
    use crate::test_support::{export_backup, import_backup, kit_create};
    let (_fx, mut core, space) = setup();
    let kit = kit_create(&mut core).words.join(" ");
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    try_set_cover(&mut core, page_id, Some(&cover_of(a)), Some(35)).unwrap();
    let backup = export_backup(&mut core);

    let other = Fixture::new();
    let mut restored = other.open(9, T0 + 5);
    import_backup(&mut restored, &backup, Some(&kit));
    let info = page(&mut restored, page_id);
    assert_eq!((info.cover, info.cover_y), (Some(cover_of(a)), Some(35)));
    assert_eq!(covers_of(&mut restored, space), [(page_id, a)]);
    assert_eq!(state(&mut restored, a), (1, None));
}
