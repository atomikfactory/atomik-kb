//! Page icons (ADR-0029): the grammar through the handlers, the check of an image icon, and the
//! retention of the image an icon uses (`icon_refs`, schema v14): an icon keeps its attachment out of
//! the garbage collector, replacing or clearing it or purging the page starts the retention clock, and
//! undo, redo, the rebuild paths and the open-time check keep the rows in step with the documents.
//! The remote import is in `tests_sync.rs`.

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

fn icon_of(id: AttachmentId) -> String {
    format!("attachment:{id}")
}

fn try_set_icon(
    core: &mut Core,
    page_id: PageId,
    icon: Option<&str>,
) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageSetIcon {
        page_id,
        icon: icon.map(str::to_owned),
    })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn set_icon(core: &mut Core, page_id: PageId, icon: Option<&str>) -> PageInfo {
    try_set_icon(core, page_id, icon).unwrap()
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

/// The refusal of an icon: `InvalidInput` on the field `icon`.
fn refused(core: &mut Core, page_id: PageId, icon: &str) -> ApiError {
    let err = try_set_icon(core, page_id, Some(icon)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{icon}: {err}");
    assert_eq!(field_of(&err), Some("icon"), "{icon}: {err}");
    err
}

// ----- the grammar through the handlers -----------------------------------------------------

#[test]
fn every_form_of_icon_is_set_read_back_and_cleared_on_a_page() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let photo = image(&mut core, space, "photo.png");
    for icon in [
        "🌱",
        "icon:book-open",
        "icon:book-open:blue",
        icon_of(photo).as_str(),
    ] {
        assert_eq!(
            set_icon(&mut core, page_id, Some(icon)).icon.as_deref(),
            Some(icon)
        );
        assert_eq!(page(&mut core, page_id).icon.as_deref(), Some(icon));
    }
    // Trimmed like any icon; the same icon again is not an error.
    let spaced = format!("  {}  ", icon_of(photo));
    assert_eq!(
        set_icon(&mut core, page_id, Some(&spaced)).icon,
        Some(icon_of(photo))
    );
    assert_eq!(set_icon(&mut core, page_id, None).icon, None);
    assert_eq!(set_icon(&mut core, page_id, None).icon, None);
}

#[test]
fn a_malformed_icon_is_refused_on_the_field_icon_for_pages_and_spaces() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    for bad in [
        "icon:",
        "icon:Book",
        "icon:-book",
        "icon:book:teal",
        "attachment:",
        "attachment:5F0C1D2E-3A4B-4C5D-8E6F-0A1B2C3D4E5F",
        "attachment:not-a-uuid",
    ] {
        refused(&mut core, page_id, bad);
        for request in [
            Request::SpaceCreate {
                name: "Work".into(),
                icon: Some(bad.into()),
            },
            Request::SpaceSetIcon {
                space_id: space,
                icon: Some(bad.into()),
            },
        ] {
            let err = core.dispatch(request).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidInput, "{bad}: {err}");
            assert_eq!(field_of(&err), Some("icon"), "{bad}: {err}");
        }
    }
    assert_eq!(page(&mut core, page_id).icon, None, "nothing was written");
    assert_eq!(spaces(&mut core).len(), 1, "no space was created");
}

#[test]
fn a_space_icon_can_be_an_emoji_or_an_icon_of_the_set_but_never_an_image() {
    let (_fx, mut core, space) = setup();
    let photo = image(&mut core, space, "photo.png");

    for icon in ["💼", "icon:briefcase", "icon:briefcase:brown"] {
        match core
            .dispatch(Request::SpaceSetIcon {
                space_id: space,
                icon: Some(icon.into()),
            })
            .unwrap()
        {
            Response::Space(info) => assert_eq!(info.icon.as_deref(), Some(icon)),
            other => panic!("unexpected {other:?}"),
        }
    }
    match core
        .dispatch(Request::SpaceCreate {
            name: "Work".into(),
            icon: Some("icon:folder:green".into()),
        })
        .unwrap()
    {
        Response::Space(info) => assert_eq!(info.icon.as_deref(), Some("icon:folder:green")),
        other => panic!("unexpected {other:?}"),
    }

    // The image is well formed and exists, and the space still refuses it, as a set and as a create.
    for request in [
        Request::SpaceSetIcon {
            space_id: space,
            icon: Some(icon_of(photo)),
        },
        Request::SpaceCreate {
            name: "Third".into(),
            icon: Some(icon_of(photo)),
        },
    ] {
        let err = core.dispatch(request).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        assert_eq!(field_of(&err), Some("icon"), "{err}");
        assert_eq!(err.message, "a space icon cannot be an image");
    }
    assert_eq!(
        spaces(&mut core).len(),
        2,
        "the refused create made no space"
    );
    assert_eq!(
        spaces(&mut core)[0].icon.as_deref(),
        Some("icon:briefcase:brown"),
        "the refused set changed nothing"
    );
    assert_eq!(
        state(&mut core, photo).0,
        0,
        "an icon of a space references nothing"
    );
}

#[test]
fn an_image_icon_must_be_an_image_of_the_pages_space_and_not_purged() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let work = create_space(&mut core, "Work").id;

    // Another space's image.
    let elsewhere = image(&mut core, work, "work.png");
    let err = refused(&mut core, page_id, &icon_of(elsewhere));
    assert!(err.message.contains("another space"), "{err}");

    // A file that is not an image.
    let pdf = upload(&mut core, space, "report.pdf", "application/pdf");
    let err = refused(&mut core, page_id, &icon_of(pdf));
    assert!(err.message.contains("not an image"), "{err}");
    let text = upload(&mut core, space, "notes.txt", "text/plain");
    refused(&mut core, page_id, &icon_of(text));

    // An attachment nobody knows.
    let unknown = AttachmentId::from_random_bytes([0x77; 16]);
    let err = refused(&mut core, page_id, &icon_of(unknown));
    assert!(err.message.contains("unknown or deleted"), "{err}");

    // A purged one.
    let gone = image(&mut core, space, "gone.png");
    call(
        &mut core,
        Request::AttachmentDelete {
            attachment_id: gone,
        },
    );
    let err = refused(&mut core, page_id, &icon_of(gone));
    assert!(err.message.contains("unknown or deleted"), "{err}");

    // Other image types are fine, in any letter case (the type is a token, not a case).
    for (n, mime) in ["image/jpeg", "image/gif", "image/svg+xml", "IMAGE/PNG"]
        .into_iter()
        .enumerate()
    {
        let id = upload(&mut core, space, &format!("pic{n}"), mime);
        assert_eq!(
            set_icon(&mut core, page_id, Some(&icon_of(id))).icon,
            Some(icon_of(id)),
            "{mime}"
        );
    }

    // A refusal changes nothing: the icon and the references are those of the last good write.
    let last = page(&mut core, page_id).icon.unwrap();
    refused(&mut core, page_id, &icon_of(pdf));
    assert_eq!(
        page(&mut core, page_id).icon.as_deref(),
        Some(last.as_str())
    );
    assert_eq!(state(&mut core, pdf).0, 0);
    assert_eq!(state(&mut core, elsewhere).0, 0);
}

// ----- retention ------------------------------------------------------------------------------

#[test]
fn an_icon_keeps_its_image_out_of_the_collector_and_replacing_or_clearing_it_starts_the_clock() {
    let (fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    let b = image(&mut core, space, "b.png");
    assert_eq!(state(&mut core, a), (0, Some(T0)), "uploaded, not used yet");

    core.drain_events();
    set_icon(&mut core, page_id, Some(&icon_of(a)));
    assert_eq!(state(&mut core, a), (1, None), "the icon references it");
    assert_eq!(state(&mut core, b), (0, Some(T0)), "the other is untouched");
    assert!(
        core.drain_events()
            .contains(&Event::AttachmentsChanged { space_id: space }),
        "the bookkeeping announces the attachments that changed"
    );
    drop(core);

    // Well past the retention: the image of an icon stays, the unused one goes.
    let t1 = T0 + 40 * DAY;
    let mut core = fx.open(2, t1);
    assert_eq!(
        attachment(&mut core, b).unwrap_err().code,
        ErrorCode::NotFound,
        "collected at open"
    );
    assert_eq!(state(&mut core, a), (1, None));
    assert_eq!(collect(&mut core), 0);

    // Replacing the icon starts the clock of the old image; the new one is referenced.
    let c = image(&mut core, space, "c.png");
    set_icon(&mut core, page_id, Some(&icon_of(c)));
    assert_eq!(state(&mut core, a), (0, Some(t1)));
    assert_eq!(state(&mut core, c), (1, None));
    drop(core);
    let mut core = fx.open(3, t1 + 29 * DAY);
    assert!(attachment(&mut core, a).is_ok(), "within the retention");
    // Putting it back in time keeps it (the clock stops).
    set_icon(&mut core, page_id, Some(&icon_of(a)));
    assert_eq!(state(&mut core, a), (1, None));
    assert_eq!(state(&mut core, c), (0, Some(t1 + 29 * DAY)));

    // Any other icon, or none, releases it.
    set_icon(&mut core, page_id, Some("icon:book:blue"));
    assert_eq!(state(&mut core, a), (0, Some(t1 + 29 * DAY)));
    set_icon(&mut core, page_id, Some(&icon_of(a)));
    set_icon(&mut core, page_id, None);
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
fn a_trashed_page_keeps_its_icon_image_and_a_purge_releases_it() {
    let (fx, mut core, space) = setup();
    let first = create_page(&mut core, space, None, "First").id;
    let second = create_page(&mut core, space, None, "Second").id;
    let child = create_page(&mut core, space, Some(second), "Child").id;
    let shared = image(&mut core, space, "shared.png");
    let own = image(&mut core, space, "own.png");
    set_icon(&mut core, first, Some(&icon_of(shared)));
    set_icon(&mut core, second, Some(&icon_of(shared)));
    set_icon(&mut core, child, Some(&icon_of(own)));
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
        core.db
            .icon_refs_of_space(space)
            .unwrap()
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
fn undo_and_redo_of_an_icon_change_move_the_reference() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    let b = image(&mut core, space, "b.png");
    set_icon(&mut core, page_id, Some(&icon_of(a)));
    assert_eq!(state(&mut core, a), (1, None));

    // Undo takes the icon away: the image is unreferenced again; redo brings it back.
    undo(&mut core, space, false);
    assert_eq!(page(&mut core, page_id).icon, None);
    assert_eq!(state(&mut core, a), (0, Some(T0)));
    assert!(core.db.icon_refs_of_space(space).unwrap().is_empty());
    undo(&mut core, space, true);
    assert_eq!(page(&mut core, page_id).icon, Some(icon_of(a)));
    assert_eq!(state(&mut core, a), (1, None));

    // A change from one image to another, undone and redone, swaps the references both ways.
    set_icon(&mut core, page_id, Some(&icon_of(b)));
    assert_eq!(state(&mut core, a), (0, Some(T0)));
    assert_eq!(state(&mut core, b), (1, None));
    undo(&mut core, space, false);
    assert_eq!(page(&mut core, page_id).icon, Some(icon_of(a)));
    assert_eq!(state(&mut core, a), (1, None));
    assert_eq!(state(&mut core, b), (0, Some(T0)));
    undo(&mut core, space, true);
    assert_eq!(state(&mut core, a), (0, Some(T0)));
    assert_eq!(state(&mut core, b), (1, None));

    // Undoing the creation of the page takes its row away with it.
    undo(&mut core, space, false);
    undo(&mut core, space, false);
    undo(&mut core, space, false);
    assert!(core.db.icon_refs_of_space(space).unwrap().is_empty());
    assert_eq!(state(&mut core, b), (0, Some(T0)));
}

#[test]
fn deleting_a_purged_image_leaves_the_icon_naming_it_and_nothing_breaks() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    set_icon(&mut core, page_id, Some(&icon_of(a)));

    // The user deletes the image explicitly: the page still names it (a reader shows the default
    // glyph), the row stays, and the next write of the structure is not troubled by it.
    call(&mut core, Request::AttachmentDelete { attachment_id: a });
    assert_eq!(page(&mut core, page_id).icon, Some(icon_of(a)));
    assert_eq!(core.db.icon_refs_of_space(space).unwrap().len(), 1);
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
    // The page can have another icon again.
    set_icon(&mut core, page_id, Some("🌱"));
    assert!(core.db.icon_refs_of_space(space).unwrap().is_empty());
}

// ----- derived rows: rebuild and open-time repair --------------------------------------------

#[test]
fn index_rebuild_restores_the_icon_references_and_drops_a_stale_row() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let other = create_page(&mut core, space, None, "Other").id;
    let a = image(&mut core, space, "a.png");
    let stale = image(&mut core, space, "stale.png");
    set_icon(&mut core, page_id, Some(&icon_of(a)));
    assert_eq!(state(&mut core, a), (1, None));

    // The derived rows are lost: the references go with them, and a rebuild brings them back.
    core.db.icon_refs_clear().unwrap();
    assert_eq!(attachment(&mut core, a).unwrap().references, 0);
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(state(&mut core, a), (1, None));

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
    assert_eq!(
        core.db.icon_refs_of_space(space).unwrap(),
        [(page_id, a)],
        "exactly what the documents say"
    );
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(state(&mut core, a), (1, None));
}

#[test]
fn the_open_time_check_fills_the_table_for_a_database_migrated_to_v14_and_repairs_it() {
    let (fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    let b = image(&mut core, space, "b.png");
    set_icon(&mut core, page_id, Some(&icon_of(a)));
    // A migrated database has an empty table although the document holds an image icon (the
    // image arrived by sync from a newer device, say), and a stale bookkeeping state.
    core.db.icon_refs_clear().unwrap();
    core.db
        .attachment_set_unreferenced_since(a, Some(5))
        .unwrap();
    core.db.icon_refs_set(space, &[(page_id, vec![b])]).unwrap();
    core.db.attachment_set_unreferenced_since(b, None).unwrap();
    drop(core);

    let mut core = fx.open(2, T0 + 10);
    assert_eq!(
        core.db.icon_refs_of_space(space).unwrap(),
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
    assert_eq!(core.db.icon_refs_of_space(space).unwrap(), [(page_id, a)]);
    assert_eq!(
        state(&mut core, b),
        (0, Some(T0 + 10)),
        "the clock is not restarted"
    );
}

#[test]
fn a_backup_restore_rebuilds_the_icon_references_from_the_documents() {
    use crate::test_support::{export_backup, import_backup, kit_create};
    let (_fx, mut core, space) = setup();
    let kit = kit_create(&mut core).words.join(" ");
    let page_id = create_page(&mut core, space, None, "Trip").id;
    let a = image(&mut core, space, "a.png");
    set_icon(&mut core, page_id, Some(&icon_of(a)));
    let backup = export_backup(&mut core);

    let other = Fixture::new();
    let mut restored = other.open(9, T0 + 5);
    import_backup(&mut restored, &backup, Some(&kit));
    assert_eq!(page(&mut restored, page_id).icon, Some(icon_of(a)));
    assert_eq!(
        restored.db.icon_refs_of_space(space).unwrap(),
        [(page_id, a)]
    );
    assert_eq!(state(&mut restored, a), (1, None));
}
