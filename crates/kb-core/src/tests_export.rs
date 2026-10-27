//! Export sessions and the CSV export of a view (ADR-0036, `docs/architecture/import-export.md`,
//! section 2): the session protocol, what is gathered, the batches, the attachments and their
//! warnings, whiteboards, the values and the CSV rules.
//!
//! The tests that read the text of an exported file need `kb-export` (layout, front matter,
//! file flavour); the session protocol, the plan counts and the lock tests do not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::DocQueries;
use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{
    AttachmentId, BlockId, ChecklistItem, ChecklistItemId, CsvColumn, DateValue, DocId, ErrorCode,
    Event, ExportContent, ExportFormat, ExportWarningCode, OptionColor, PageId, PlaceValue,
    PropertyConfig, PropertyKind, PropertyValue, Request, Response, RollupFunction, RollupTarget,
    SpaceId,
};
use serde_json::{Value, json};

use crate::Core;
use crate::test_support::{Fixture, create_page, create_space, create_workspace, spaces};
use crate::test_support_interop::{
    export_all, export_begin, export_next, make_not_downloaded, option_add, prop_create,
    prop_create_with, set_value, setup, setup_with_offset, upload,
};

fn uuid(n: u8) -> String {
    BlockId::from_random_bytes([n; 16]).to_string()
}

fn text_para(n: u8, text: &str) -> Value {
    json!({
        "type": "paragraph",
        "attrs": { "id": uuid(n) },
        "content": [{ "type": "text", "text": text }]
    })
}

fn doc(blocks: Vec<Value>) -> Value {
    json!({ "type": "doc", "content": blocks })
}

fn image_block(n: u8, attachment: AttachmentId) -> Value {
    json!({
        "type": "image",
        "attrs": { "id": uuid(n), "attachmentId": attachment.to_string(), "alt": "picture" }
    })
}

fn page_ref_para(n: u8, target: PageId) -> Value {
    json!({
        "type": "paragraph",
        "attrs": { "id": uuid(n) },
        "content": [
            { "type": "text", "text": "See " },
            { "type": "pageRef", "attrs": { "pageId": target.to_string() } }
        ]
    })
}

/// Removes a page's stored document, as a page that was created on another device and has not
/// been opened yet has none.
fn drop_document(core: &mut Core, page_id: PageId) {
    core.docs.pages.remove(&page_id);
    core.db
        .transaction(|tx| tx.doc_delete(DocId::for_page(page_id)))
        .unwrap();
}

fn has_document(core: &Core, page_id: PageId) -> bool {
    core.db.doc_get(DocId::for_page(page_id)).unwrap().is_some()
}

fn begin_request(space_id: SpaceId, page_id: Option<PageId>, children: bool) -> Request {
    Request::ExportBegin {
        space_id,
        page_id,
        include_children: children,
        format: ExportFormat::Markdown,
    }
}

// ----- the protocol (independent of the converters) -----------------------------------------------

#[test]
fn export_sessions_are_numbered_replaced_and_ended() {
    let (_fx, mut core, space) = setup();
    create_page(&mut core, space, None, "One");

    let first = export_begin(&mut core, space, None, false, ExportFormat::Markdown);
    assert_eq!(first.export_id, 1, "ids count up from 1");
    let second = export_begin(&mut core, space, None, false, ExportFormat::Html);
    assert_eq!(second.export_id, 2);

    // A new session replaced the first.
    let err = core
        .dispatch(Request::ExportNext {
            export_id: first.export_id,
            max_entries: 10,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // The limits of a batch.
    for bad in [0, 501, u32::MAX] {
        let err = core
            .dispatch(Request::ExportNext {
                export_id: second.export_id,
                max_entries: bad,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad}");
        assert_eq!(err.details, Some(json!({ "field": "maxEntries" })), "{bad}");
    }
    // A wrong limit does not end the session.
    let err = core
        .dispatch(Request::ExportNext {
            export_id: 77,
            max_entries: 10,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "an unknown session");

    // Ending is for any id; an ended session is gone.
    assert_eq!(
        core.dispatch(Request::ExportEnd { export_id: 99 }).unwrap(),
        Response::Ok
    );
    assert_eq!(
        core.dispatch(Request::ExportEnd { export_id: 2 }).unwrap(),
        Response::Ok
    );
    let err = core
        .dispatch(Request::ExportNext {
            export_id: 2,
            max_entries: 10,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // Ids are not reused while the core runs.
    let third = export_begin(&mut core, space, None, false, ExportFormat::Markdown);
    assert_eq!(third.export_id, 3);
}

#[test]
fn locking_the_app_drops_export_and_import_sessions() {
    const PASSPHRASE: &str = "correct horse battery";
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    create_page(&mut core, space, None, "Locked away");
    core.dispatch(Request::AppLockEnable {
        passphrase: PASSPHRASE.into(),
    })
    .unwrap();

    let export = export_begin(&mut core, space, None, false, ExportFormat::Markdown);
    let import = match core
        .dispatch(Request::ImportBegin {
            space_id: space,
            parent_id: None,
            source_name: None,
        })
        .unwrap()
    {
        Response::ImportStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!((export.export_id, import), (1, 1));

    let locked = core.lock().map_err(|(_, e)| e).unwrap();
    let mut core = locked.unlock(PASSPHRASE).map_err(|(_, e)| e).unwrap();

    let err = core
        .dispatch(Request::ExportNext {
            export_id: 1,
            max_entries: 10,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "the export session is gone");
    let err = core
        .dispatch(Request::ImportAddFile {
            import_id: 1,
            path: "a.md".into(),
            text: "x".into(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "the import session is gone");
}

#[test]
fn an_export_plan_counts_the_live_pages_it_covers() {
    let (_fx, mut core, space) = setup();
    let projects = create_page(&mut core, space, None, "Projects");
    let alpha = create_page(&mut core, space, Some(projects.id), "Alpha");
    let beta = create_page(&mut core, space, Some(projects.id), "Beta");
    let deep = create_page(&mut core, space, Some(alpha.id), "Deep");
    let notes = create_page(&mut core, space, None, "Notes");
    let gone = create_page(&mut core, space, Some(projects.id), "Gone");
    core.dispatch(Request::PageTrash { page_id: gone.id })
        .unwrap();

    let count = |core: &mut Core, page: Option<PageId>, children: bool| match core
        .dispatch(begin_request(space, page, children))
        .unwrap()
    {
        Response::ExportPlanned(plan) => plan.pages,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        count(&mut core, None, false),
        5,
        "the whole space, live pages only"
    );
    assert_eq!(
        count(&mut core, None, true),
        5,
        "children are ignored without a page"
    );
    assert_eq!(count(&mut core, Some(projects.id), false), 1);
    assert_eq!(count(&mut core, Some(projects.id), true), 4);
    assert_eq!(count(&mut core, Some(alpha.id), true), 2);
    assert_eq!(count(&mut core, Some(beta.id), true), 1);
    assert_eq!(count(&mut core, Some(notes.id), true), 1);
    assert_eq!(count(&mut core, Some(deep.id), true), 1);

    // Trashed, unknown or foreign pages and unknown spaces.
    for page_id in [gone.id, PageId::from_random_bytes([9; 16])] {
        let err = core
            .dispatch(begin_request(space, Some(page_id), false))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }
    let other = create_space(&mut core, "Other");
    let foreign = create_page(&mut core, other.id, None, "Foreign");
    let err = core
        .dispatch(begin_request(space, Some(foreign.id), true))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "a page of another space");
    let err = core
        .dispatch(begin_request(
            SpaceId::from_random_bytes([3; 16]),
            None,
            false,
        ))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn a_failed_begin_leaves_the_open_session_alone() {
    let (_fx, mut core, space) = setup();
    create_page(&mut core, space, None, "One");
    let plan = export_begin(&mut core, space, None, false, ExportFormat::Markdown);
    let err = core
        .dispatch(begin_request(
            space,
            Some(PageId::from_random_bytes([5; 16])),
            false,
        ))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // The first session is still there (its next call fails only for the converters' stub).
    let result = core.dispatch(Request::ExportNext {
        export_id: plan.export_id,
        max_entries: 10,
    });
    assert!(
        !matches!(&result, Err(e) if e.code == ErrorCode::NotFound),
        "{result:?}"
    );
}

// ----- what an export holds -------------------------------------------------------------------

#[test]
fn a_tree_exports_in_tree_order_with_the_sidecar_last() {
    let (_fx, mut core, space) = setup();
    let projects = create_page(&mut core, space, None, "Projects");
    let alpha = create_page(&mut core, space, Some(projects.id), "Alpha");
    create_page(&mut core, space, Some(projects.id), "Beta");
    create_page(&mut core, space, None, "Notes");
    core.replace_page_document_for_test(alpha.id, &doc(vec![text_para(1, "Alpha body")]))
        .unwrap();

    let exported = export_all(&mut core, space, None, false, ExportFormat::Markdown, 500);
    assert_eq!(exported.plan.pages, 4);
    assert!(!exported.plan.single_file);
    assert_eq!(exported.plan.suggested_name, "Personal.zip");
    assert_eq!(
        exported.paths(),
        [
            "Projects.md",
            "Projects/Alpha.md",
            "Projects/Beta.md",
            "Notes.md",
            "atomik-export.json"
        ]
    );
    assert!(
        exported
            .text("Projects/Alpha.md")
            .contains("title: \"Alpha\"")
    );
    assert!(exported.text("Projects/Alpha.md").contains("Alpha body"));
    assert!(
        exported
            .text("atomik-export.json")
            .contains("Projects/Alpha.md")
    );
    assert!(exported.warnings.is_empty());
}

#[test]
fn one_page_without_attachments_is_a_single_file() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Weekly review");
    core.replace_page_document_for_test(page.id, &doc(vec![text_para(1, "Hello")]))
        .unwrap();

    let md = export_all(
        &mut core,
        space,
        Some(page.id),
        false,
        ExportFormat::Markdown,
        500,
    );
    assert!(md.plan.single_file);
    assert_eq!(md.plan.suggested_name, "Weekly review.md");
    assert_eq!(md.paths(), ["Weekly review.md"]);
    assert!(md.text("Weekly review.md").starts_with("---\n"));

    let html = export_all(
        &mut core,
        space,
        Some(page.id),
        false,
        ExportFormat::Html,
        500,
    );
    assert!(html.plan.single_file);
    assert_eq!(html.plan.suggested_name, "Weekly review.html");
    assert!(
        html.text("Weekly review.html")
            .starts_with("<!doctype html>")
    );

    // A page with children is an archive, and an HTML archive has no sidecar.
    create_page(&mut core, space, Some(page.id), "Child");
    let archive = export_all(
        &mut core,
        space,
        Some(page.id),
        true,
        ExportFormat::Html,
        500,
    );
    assert!(!archive.plan.single_file);
    assert_eq!(archive.plan.suggested_name, "Weekly review.zip");
    assert_eq!(
        archive.paths(),
        ["Weekly review.html", "Weekly review/Child.html"]
    );
}

#[test]
fn batches_never_exceed_the_limit_and_progress_is_announced() {
    let (_fx, mut core, space) = setup();
    for title in ["A", "B", "C"] {
        create_page(&mut core, space, None, title);
    }
    core.drain_events();

    let whole = export_all(&mut core, space, None, false, ExportFormat::Markdown, 500);
    assert_eq!(whole.entries.len(), 4, "three pages and the sidecar");
    assert_eq!(whole.calls, 1);

    core.drain_events();
    let exported = export_all(&mut core, space, None, false, ExportFormat::Markdown, 1);
    assert_eq!(
        exported.paths(),
        whole.paths(),
        "the same entries in the same order"
    );
    assert_eq!(exported.calls, 4, "one entry per call");
    let progress: Vec<(u32, u32)> = core
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ExportProgress { done, total, .. } => Some((done, total)),
            _ => None,
        })
        .collect();
    // Pages rendered after each call: A, B, C, then the sidecar (no more pages).
    assert_eq!(progress, [(1, 3), (2, 3), (3, 3), (3, 3)]);

    // Asking again after the end is not an error: nothing more, still done.
    let plan = export_begin(&mut core, space, None, false, ExportFormat::Markdown);
    let finished = export_next(&mut core, plan.export_id, 500);
    assert!(finished.done);
    let again = export_next(&mut core, plan.export_id, 500);
    assert!(again.done && again.entries.is_empty() && again.warnings.is_empty());
}

#[test]
fn the_export_never_creates_a_page_document() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Empty on arrival");
    drop_document(&mut core, page.id);
    assert!(!has_document(&core, page.id));

    let exported = export_all(
        &mut core,
        space,
        Some(page.id),
        false,
        ExportFormat::Markdown,
        500,
    );
    assert_eq!(exported.paths(), ["Empty on arrival.md"]);
    assert!(
        exported
            .text("Empty on arrival.md")
            .contains("Empty on arrival")
    );
    assert!(!has_document(&core, page.id), "reading is not creating");
}

#[test]
fn page_links_inside_the_export_are_files_and_outside_it_are_titles() {
    let (_fx, mut core, space) = setup();
    let alpha = create_page(&mut core, space, None, "Alpha");
    let beta = create_page(&mut core, space, None, "Beta");
    core.replace_page_document_for_test(alpha.id, &doc(vec![page_ref_para(1, beta.id)]))
        .unwrap();

    let both = export_all(&mut core, space, None, false, ExportFormat::Markdown, 500);
    assert!(
        both.text("Alpha.md").contains("[Beta](Beta.md)"),
        "{}",
        both.text("Alpha.md")
    );

    let alone = export_all(
        &mut core,
        space,
        Some(alpha.id),
        false,
        ExportFormat::Markdown,
        500,
    );
    assert!(
        alone.text("Alpha.md").contains("[[Beta]]"),
        "{}",
        alone.text("Alpha.md")
    );
}

#[test]
fn attachments_are_written_once_next_to_the_first_page_that_uses_them() {
    let (_fx, mut core, space) = setup();
    let other = create_space(&mut core, "Other").id;
    let alpha = create_page(&mut core, space, None, "Alpha");
    let beta = create_page(&mut core, space, None, "Beta");

    let photo = upload(&mut core, space, "photo.png", "image/png", b"PNG-BYTES");
    let remote = upload(
        &mut core,
        space,
        "remote.pdf",
        "application/pdf",
        b"%PDF remote",
    );
    let deleted = upload(
        &mut core,
        space,
        "deleted.bin",
        "application/octet-stream",
        b"gone",
    );
    let foreign = upload(&mut core, other, "foreign.png", "image/png", b"FOREIGN");
    make_not_downloaded(&mut core, remote.attachment_id);
    core.replace_page_document_for_test(
        alpha.id,
        &doc(vec![
            image_block(1, photo.attachment_id),
            image_block(2, remote.attachment_id),
            image_block(3, deleted.attachment_id),
            image_block(4, foreign.attachment_id),
        ]),
    )
    .unwrap();
    core.replace_page_document_for_test(
        beta.id,
        &doc(vec![
            image_block(5, photo.attachment_id),
            image_block(6, remote.attachment_id),
        ]),
    )
    .unwrap();
    core.dispatch(Request::AttachmentDelete {
        attachment_id: deleted.attachment_id,
    })
    .unwrap();

    let exported = export_all(&mut core, space, None, false, ExportFormat::Markdown, 500);
    assert_eq!(
        exported.plan.attachments, 1,
        "only the file the device holds"
    );
    assert!(!exported.plan.single_file);
    assert_eq!(
        exported.paths(),
        [
            "Alpha.md",
            "Alpha/photo.png",
            "Beta.md",
            "atomik-export.json"
        ]
    );
    let attachment_entries: Vec<_> = exported
        .entries
        .iter()
        .filter(|e| matches!(e.content, ExportContent::Attachment { .. }))
        .collect();
    assert_eq!(attachment_entries.len(), 1, "written once");
    assert_eq!(
        attachment_entries[0].content,
        ExportContent::Attachment {
            attachment_id: photo.attachment_id,
            size: 9
        }
    );
    assert!(
        exported.text("Beta.md").contains("Alpha/photo.png"),
        "the second page links to the first page's file: {}",
        exported.text("Beta.md")
    );

    // One warning per attachment, with the first page that uses it.
    let mut warnings: Vec<_> = exported
        .warnings
        .iter()
        .map(|w| (w.code, w.page_id, w.attachment_id))
        .collect();
    warnings.sort_by_key(|(_, _, attachment)| attachment.map(|a| a.to_string()));
    let mut expected = vec![
        (
            ExportWarningCode::AttachmentNotDownloaded,
            Some(alpha.id),
            Some(remote.attachment_id),
        ),
        (
            ExportWarningCode::AttachmentMissing,
            Some(alpha.id),
            Some(deleted.attachment_id),
        ),
        (
            ExportWarningCode::AttachmentMissing,
            Some(alpha.id),
            Some(foreign.attachment_id),
        ),
    ];
    expected.sort_by_key(|(_, _, attachment)| attachment.map(|a| a.to_string()));
    assert_eq!(warnings, expected);
}

#[test]
fn a_page_whose_only_attachment_is_missing_is_still_a_single_file() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Lonely");
    let remote = upload(&mut core, space, "remote.png", "image/png", b"x");
    make_not_downloaded(&mut core, remote.attachment_id);
    core.replace_page_document_for_test(page.id, &doc(vec![image_block(1, remote.attachment_id)]))
        .unwrap();

    let exported = export_all(
        &mut core,
        space,
        Some(page.id),
        false,
        ExportFormat::Markdown,
        500,
    );
    assert!(
        exported.plan.single_file,
        "no attachment goes into the archive"
    );
    assert_eq!(exported.plan.attachments, 0);
    assert_eq!(exported.paths(), ["Lonely.md"]);
    assert!(
        exported
            .text("Lonely.md")
            .contains(&format!("attachment://{}", remote.attachment_id)),
        "the link stays as it was: {}",
        exported.text("Lonely.md")
    );
    assert_eq!(exported.warnings.len(), 1);
    assert_eq!(
        exported.warnings[0].code,
        ExportWarningCode::AttachmentNotDownloaded
    );
}

#[test]
fn attachments_named_by_properties_and_icons_go_into_the_archive() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Dossier");
    let files = prop_create(&mut core, space, "Docs", PropertyKind::Files);
    let report = upload(&mut core, space, "report.pdf", "application/pdf", b"%PDF-1");
    let logo = upload(&mut core, space, "logo.png", "image/png", b"LOGO");
    set_value(
        &mut core,
        page.id,
        files.id,
        PropertyValue::Files(vec![report.attachment_id]),
    );
    core.dispatch(Request::PageSetIcon {
        page_id: page.id,
        icon: Some(format!("attachment:{}", logo.attachment_id)),
    })
    .unwrap();

    let exported = export_all(
        &mut core,
        space,
        Some(page.id),
        false,
        ExportFormat::Markdown,
        500,
    );
    assert!(!exported.plan.single_file);
    assert_eq!(exported.plan.attachments, 2);
    let text = exported.text("Dossier.md");
    assert!(text.contains("Dossier/report.pdf"), "{text}");
    assert!(
        text.contains("Dossier/logo.png"),
        "the icon is a file: {text}"
    );
}

#[test]
fn whiteboards_become_excalidraw_files() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Board");
    let block = uuid(7);
    let content = json!({
        "type": "doc",
        "content": [
            text_para(1, "Above"),
            { "type": "whiteboard", "attrs": { "id": block, "height": 360 } }
        ],
        "whiteboards": {
            block.clone(): {
                "frame1": {
                    "id": "frame1", "type": "rectangle", "index": "a0", "isDeleted": false,
                    "x": 120.5, "y": 64, "width": 200, "height": 80.5, "version": 3
                },
                "gone1": {
                    "id": "gone1", "type": "ellipse", "index": "a2", "isDeleted": true,
                    "x": 10, "y": 10, "version": 5
                }
            }
        }
    });
    core.replace_page_document_for_test(page.id, &content)
        .unwrap();

    let exported = export_all(
        &mut core,
        space,
        Some(page.id),
        false,
        ExportFormat::Markdown,
        500,
    );
    assert!(!exported.plan.single_file);
    assert_eq!(
        exported.paths(),
        [
            "Board.md",
            "Board/Whiteboard.excalidraw",
            "atomik-export.json"
        ]
    );
    let scene = exported.text("Board/Whiteboard.excalidraw");
    assert!(scene.contains("\"excalidraw\""), "{scene}");
    assert!(scene.contains("frame1"));
    assert!(!scene.contains("gone1"), "deleted elements are left out");
    assert!(
        exported
            .text("Board.md")
            .contains("Board/Whiteboard.excalidraw")
    );
}

#[test]
fn property_values_are_written_by_name_with_links_to_related_pages() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Weekly review");
    let other = create_page(&mut core, space, None, "Other page");
    let status = prop_create(&mut core, space, "Status", PropertyKind::Status);
    let done = status.options.iter().find(|o| o.name == "Done").unwrap().id;
    let estimate = prop_create(&mut core, space, "Estimate", PropertyKind::Number);
    let checkbox = prop_create(&mut core, space, "Shipped", PropertyKind::Checkbox);
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date);
    let related = prop_create_with(
        &mut core,
        space,
        "Related",
        PropertyKind::Relation,
        Some(PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None,
        }),
    );
    let tags = option_add(
        &mut core,
        space,
        TAGS_PROPERTY_ID,
        "work",
        OptionColor::Blue,
    );
    let place = prop_create(&mut core, space, "Place", PropertyKind::Place);
    set_value(&mut core, page.id, status.id, PropertyValue::Status(done));
    set_value(&mut core, page.id, estimate.id, PropertyValue::Number(3.5));
    set_value(
        &mut core,
        page.id,
        checkbox.id,
        PropertyValue::Checkbox(true),
    );
    set_value(
        &mut core,
        page.id,
        due.id,
        PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
        }),
    );
    set_value(
        &mut core,
        page.id,
        related.id,
        PropertyValue::Relation(vec![other.id]),
    );
    set_value(
        &mut core,
        page.id,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![tags.id]),
    );
    set_value(
        &mut core,
        page.id,
        place.id,
        PropertyValue::Place(PlaceValue {
            name: "Paris".into(),
            lat: Some(48.8566),
            lon: Some(2.3522),
        }),
    );

    // In an export with the other page, the relation links to its file.
    let both = export_all(&mut core, space, None, false, ExportFormat::Markdown, 500);
    let text = both.text("Weekly review.md");
    assert!(text.contains("\"Status\": \"Done\""), "{text}");
    assert!(text.contains("\"Estimate\": 3.5"), "{text}");
    assert!(text.contains("\"Shipped\": true"), "{text}");
    assert!(text.contains("\"Due\": \"2026-10-07 14:30\""), "{text}");
    assert!(text.contains("\"Related\":"), "{text}");
    assert!(
        text.contains("[[Other page]]") || text.contains("Other page"),
        "{text}"
    );
    assert!(text.contains("tags:"), "{text}");
    assert!(text.contains("- \"work\""), "{text}");
    assert!(text.contains("name: \"Paris\""), "{text}");
    // The sidecar carries the schema by names.
    let sidecar = both.text("atomik-export.json");
    assert!(
        sidecar.contains("\"Status\"") && sidecar.contains("\"Related\""),
        "{sidecar}"
    );
    assert!(sidecar.contains("\"Done\""), "{sidecar}");
}

// ----- CSV ----------------------------------------------------------------------------------------

#[test]
fn csv_rows_follow_the_view_with_values_as_text() {
    // 2025-10-09 08:53:20 UTC on a device two hours east of Greenwich.
    let (_fx, mut core, space) = setup_with_offset(1_760_000_000_000, 7200);
    let first = create_page(&mut core, space, None, "Alpha, the first");
    let second = create_page(&mut core, space, None, "=SUM(A1)");
    let trashed = create_page(&mut core, space, None, "Trashed");
    core.dispatch(Request::PageTrash {
        page_id: trashed.id,
    })
    .unwrap();

    let estimate = prop_create(&mut core, space, "Estimate", PropertyKind::Number);
    let shipped = prop_create(&mut core, space, "Shipped", PropertyKind::Checkbox);
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date);
    let stage = prop_create(&mut core, space, "Stage", PropertyKind::Select);
    let labels = prop_create(&mut core, space, "Labels", PropertyKind::MultiSelect);
    let tasks = prop_create(&mut core, space, "Tasks", PropertyKind::Checklist);
    let place = prop_create(&mut core, space, "Place", PropertyKind::Place);
    let related = prop_create_with(
        &mut core,
        space,
        "Related",
        PropertyKind::Relation,
        Some(PropertyConfig::Relation {
            two_way: false,
            paired_property_id: None,
        }),
    );
    let progress = prop_create_with(
        &mut core,
        space,
        "Progress",
        PropertyKind::Rollup,
        Some(PropertyConfig::Rollup {
            relation_property_id: related.id,
            target: RollupTarget::Property(tasks.id),
            function: RollupFunction::ChecklistProgress,
        }),
    );
    let created = prop_create(&mut core, space, "Created", PropertyKind::CreatedTime);
    let author = prop_create(&mut core, space, "Author", PropertyKind::CreatedBy);
    let docs = prop_create(&mut core, space, "Docs", PropertyKind::Files);

    let in_progress = option_add(&mut core, space, stage.id, "In progress", OptionColor::Blue);
    let red = option_add(&mut core, space, labels.id, "red, hot", OptionColor::Red);
    let blue = option_add(&mut core, space, labels.id, "blue", OptionColor::Blue);
    let report = upload(&mut core, space, "report.pdf", "application/pdf", b"%PDF");

    set_value(&mut core, first.id, estimate.id, PropertyValue::Number(3.5));
    set_value(
        &mut core,
        first.id,
        shipped.id,
        PropertyValue::Checkbox(true),
    );
    set_value(
        &mut core,
        first.id,
        due.id,
        PropertyValue::Date(DateValue {
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
        }),
    );
    set_value(
        &mut core,
        first.id,
        stage.id,
        PropertyValue::Select(in_progress.id),
    );
    set_value(
        &mut core,
        first.id,
        labels.id,
        PropertyValue::MultiSelect(vec![red.id, blue.id]),
    );
    set_value(
        &mut core,
        first.id,
        place.id,
        PropertyValue::Place(PlaceValue {
            name: "Paris".into(),
            lat: Some(48.8566),
            lon: Some(2.3522),
        }),
    );
    set_value(
        &mut core,
        first.id,
        related.id,
        PropertyValue::Relation(vec![second.id]),
    );
    set_value(
        &mut core,
        first.id,
        docs.id,
        PropertyValue::Files(vec![report.attachment_id]),
    );
    let item = |done, text: &str, n: u8| ChecklistItem {
        id: ChecklistItemId::from_random_bytes([n; 16]),
        text: text.into(),
        done,
    };
    set_value(
        &mut core,
        second.id,
        tasks.id,
        PropertyValue::Checklist(vec![item(true, "buy milk", 1), item(false, "call Ann", 2)]),
    );

    let unknown = PageId::from_random_bytes([8; 16]);
    let foreign_space = create_space(&mut core, "Other");
    let foreign = create_page(&mut core, foreign_space.id, None, "Foreign");
    let csv = match core
        .dispatch(Request::ExportCsv {
            space_id: space,
            page_ids: vec![second.id, trashed.id, unknown, first.id, foreign.id],
            columns: vec![
                CsvColumn::Title,
                CsvColumn::Property {
                    property_id: estimate.id,
                },
                CsvColumn::Property {
                    property_id: shipped.id,
                },
                CsvColumn::Property {
                    property_id: due.id,
                },
                CsvColumn::Property {
                    property_id: stage.id,
                },
                CsvColumn::Property {
                    property_id: labels.id,
                },
                CsvColumn::Property {
                    property_id: place.id,
                },
                CsvColumn::Property {
                    property_id: related.id,
                },
                CsvColumn::Property {
                    property_id: progress.id,
                },
                CsvColumn::Property {
                    property_id: created.id,
                },
                CsvColumn::Property {
                    property_id: author.id,
                },
                CsvColumn::Property {
                    property_id: docs.id,
                },
                CsvColumn::Property {
                    property_id: tasks.id,
                },
            ],
        })
        .unwrap()
    {
        Response::Csv(csv) => csv,
        other => panic!("unexpected {other:?}"),
    };

    let header =
        "Name,Estimate,Shipped,Due,Stage,Labels,Place,Related,Progress,Created,Author,Docs,Tasks";
    assert!(csv.starts_with(&format!("\u{feff}{header}\r\n")), "{csv:?}");
    assert!(csv.ends_with("\r\n"), "CRLF line ends");
    assert!(!csv.replace("\r\n", "").contains('\n'), "no bare line feed");
    // Quoted when needed: a comma in the title and in a list of options.
    assert!(csv.contains("\r\n\"Alpha, the first\","), "{csv:?}");
    assert!(csv.contains(",\"red, hot, blue\","), "{csv:?}");

    let rows = parse_csv(&csv);
    assert_eq!(rows.len(), 3, "a header and two rows: {csv:?}");
    // Rows in the order given; the trashed, unknown and foreign pages are skipped; the title that
    // looks like a formula is guarded (number cells never are).
    let second_row = &rows[1];
    assert_eq!(second_row.len(), 13);
    assert_eq!(second_row[..8], ["'=SUM(A1)", "", "", "", "", "", "", ""]);
    assert!(
        ["", "0/0"].contains(&second_row[8].as_str()),
        "{second_row:?}"
    );
    assert_eq!(
        second_row[9..],
        [
            "2025-10-09 10:53",
            "Windows device",
            "",
            "[x] buy milk; [ ] call Ann"
        ]
    );
    assert_eq!(
        rows[2],
        [
            "Alpha, the first",
            "3.5",
            "Yes",
            "2026-10-07 14:30",
            "In progress",
            "red, hot, blue",
            "Paris (48.8566, 2.3522)",
            "'=SUM(A1)",
            "1/2",
            "2025-10-09 10:53",
            "Windows device",
            "report.pdf",
            ""
        ]
    );
}

/// RFC 4180 rows of a CSV text (the byte order mark removed).
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[test]
fn csv_without_rows_is_the_header_and_bad_requests_are_refused() {
    let (_fx, mut core, space) = setup();
    let estimate = prop_create(&mut core, space, "Estimate", PropertyKind::Number);
    let csv = |core: &mut Core, page_ids: Vec<PageId>, columns: Vec<CsvColumn>| {
        core.dispatch(Request::ExportCsv {
            space_id: space,
            page_ids,
            columns,
        })
    };
    match csv(
        &mut core,
        Vec::new(),
        vec![
            CsvColumn::Title,
            CsvColumn::Property {
                property_id: estimate.id,
            },
        ],
    )
    .unwrap()
    {
        Response::Csv(text) => assert_eq!(text, "\u{feff}Name,Estimate\r\n"),
        other => panic!("unexpected {other:?}"),
    }

    // A property the schema lacks.
    let err = csv(
        &mut core,
        Vec::new(),
        vec![CsvColumn::Property {
            property_id: kb_types::PropertyId::from_random_bytes([4; 16]),
        }],
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(err.details, Some(json!({ "field": "columns" })));

    // The limits: 50,000 rows and 101 columns.
    let rows = vec![PageId::from_random_bytes([1; 16]); 50_001];
    let err = csv(&mut core, rows, vec![CsvColumn::Title]).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = csv(&mut core, Vec::new(), vec![CsvColumn::Title; 102]).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(csv(&mut core, Vec::new(), vec![CsvColumn::Title; 101]).is_ok());

    // An unknown space.
    let err = core
        .dispatch(Request::ExportCsv {
            space_id: SpaceId::from_random_bytes([6; 16]),
            page_ids: Vec::new(),
            columns: Vec::new(),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn csv_of_a_large_view_reads_the_same_rows() {
    // More rows than the direct-lookup threshold (a page may be listed again and again): the rows
    // come from one pass over the tree and read as they do from direct lookups.
    let (_fx, mut core, space) = setup();
    let ids: Vec<PageId> = ["Row A", "Row B", "Row C"]
        .iter()
        .map(|title| create_page(&mut core, space, None, title).id)
        .collect();
    let trashed = create_page(&mut core, space, None, "Row T");
    core.dispatch(Request::PageTrash {
        page_id: trashed.id,
    })
    .unwrap();
    let csv_of = |core: &mut Core, page_ids: Vec<PageId>| match core
        .dispatch(Request::ExportCsv {
            space_id: space,
            page_ids,
            columns: vec![CsvColumn::Title],
        })
        .unwrap()
    {
        Response::Csv(csv) => csv,
        other => panic!("unexpected {other:?}"),
    };
    let few = vec![ids[2], trashed.id, ids[0], ids[0], ids[1]];
    assert_eq!(
        csv_of(&mut core, few.clone()),
        "\u{feff}Name\r\nRow C\r\nRow A\r\nRow A\r\nRow B\r\n"
    );
    let many: Vec<PageId> = few.iter().copied().cycle().take(5 * 101).collect();
    assert!(many.len() > 500);
    let csv = csv_of(&mut core, many);
    let lines: Vec<&str> = csv.trim_end_matches("\r\n").split("\r\n").collect();
    assert_eq!(
        lines.len(),
        1 + 4 * 101,
        "the trashed page is skipped each time"
    );
    assert_eq!(lines[1..5], ["Row C", "Row A", "Row A", "Row B"]);
    assert_eq!(lines[lines.len() - 1], "Row B");
}
