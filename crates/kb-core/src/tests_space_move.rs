//! Moving pages between spaces (ADR-0038, `docs/architecture/space-move.md`, section 7): the
//! request end to end on one device, the keys, and the two-device scenarios through the memory
//! remote (the fleet simulator of `tests_sync.rs`, reduced to what these scenarios need).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::collections::{BTreeMap, BTreeSet};

use kb_keys::KeySubject;
use kb_model::BlockKind;
use kb_storage::{
    AttachmentQueries, DocQueries, IconRefQueries, SyncQueries, TextIndexQueries, UpdateOrigin,
};
use kb_sync::v2::MemoryRemote;
use kb_types::attachments::CHUNK_SIZE;
use kb_types::testing::SeededRng;
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, BlockId, ChangeSource, DocId, ErrorCode, Event, FeedId,
    OptionColor, PageFont, PageId, PageInfo, PropertyDef, PropertyId, PropertyKind, PropertyOption,
    PropertyValue, Request, Response, SpaceId, SpaceMoveSummary,
};
use serde_json::{Value, json};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, kit_create, outline, page, page_content,
    spaces, tree, write_text,
};
use crate::tests_sharing::Dev;

/// The documents a core would snapshot at its next cycle (its personal feed's candidates).
fn candidates(core: &mut Core) -> Vec<DocId> {
    core.sync_feeds().unwrap();
    let personal = core.personal_feed().unwrap().expect("an account");
    core.sync_feed_snapshot_candidates(personal).unwrap()
}

/// A snapshot of one document of the personal feed, and the cursor it covers.
fn make_snapshot(core: &mut Core, doc: DocId) -> (Vec<u8>, u64) {
    core.sync_feeds().unwrap();
    let personal = core.personal_feed().unwrap().expect("an account");
    core.sync_feed_make_snapshot(personal, doc).unwrap()
}

// ----- one device ------------------------------------------------------------------------------

/// A core with a workspace and two spaces, "Personal" and "Work".
fn world() -> (Fixture, Core, SpaceId, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    create_workspace(&mut core, "Home", Some("Personal"));
    let personal = spaces(&mut core)[0].id;
    let work = create_space(&mut core, "Work").id;
    core.drain_events();
    (fx, core, personal, work)
}

fn move_to(
    core: &mut Core,
    page_id: PageId,
    space_id: SpaceId,
    parent_id: Option<PageId>,
) -> Result<SpaceMoveSummary, ApiError> {
    match core.dispatch(Request::PageMoveToSpace {
        page_id,
        space_id,
        parent_id,
        index: None,
    })? {
        Response::PagesMovedToSpace(summary) => Ok(summary),
        other => panic!("unexpected {other:?}"),
    }
}

fn plan(
    core: &mut Core,
    page_id: PageId,
    space_id: SpaceId,
    parent_id: Option<PageId>,
) -> Result<SpaceMoveSummary, ApiError> {
    match core.dispatch(Request::PageMoveToSpacePlan {
        page_id,
        space_id,
        parent_id,
    })? {
        Response::SpaceMovePlanned(summary) => Ok(summary),
        other => panic!("unexpected {other:?}"),
    }
}

fn call(core: &mut Core, request: Request) -> Response {
    core.dispatch(request).unwrap()
}

fn search(core: &mut Core, query: &str, space_id: Option<SpaceId>) -> Vec<(PageId, SpaceId)> {
    match call(
        core,
        Request::Search {
            query: query.into(),
            space_id,
            scope: None,
            limit: Some(50),
            edited_after: None,
        },
    ) {
        Response::Search(hits) => hits.into_iter().map(|h| (h.page_id, h.space_id)).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

fn favorites(core: &mut Core, space_id: SpaceId) -> Vec<PageId> {
    tree(core, space_id).favorites
}

fn schema(core: &mut Core, space_id: SpaceId) -> Vec<PropertyDef> {
    match call(core, Request::PropertySchemaGet { space_id }) {
        Response::PropertySchema(schema) => schema.properties,
        other => panic!("unexpected {other:?}"),
    }
}

fn prop_create(core: &mut Core, space_id: SpaceId, name: &str, kind: PropertyKind) -> PropertyDef {
    match call(
        core,
        Request::PropertyCreate {
            space_id,
            name: name.into(),
            kind,
            config: None,
        },
    ) {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    }
}

fn option_add(
    core: &mut Core,
    space_id: SpaceId,
    property_id: PropertyId,
    name: &str,
) -> PropertyOption {
    match call(
        core,
        Request::PropertyOptionAdd {
            space_id,
            property_id,
            name: name.into(),
            color: OptionColor::Green,
            group: None,
        },
    ) {
        Response::PropertyOption(option) => option,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_value(core: &mut Core, page_id: PageId, property_id: PropertyId, value: PropertyValue) {
    call(
        core,
        Request::PagePropertySet {
            page_id,
            property_id,
            value: Some(value),
        },
    );
}

fn values(core: &mut Core, page_id: PageId) -> Vec<(PropertyId, PropertyValue)> {
    match call(core, Request::PagePropertiesGet { page_id }) {
        Response::PageProperties(values) => values.values,
        other => panic!("unexpected {other:?}"),
    }
}

fn value_of(core: &mut Core, page_id: PageId, property_id: PropertyId) -> Option<PropertyValue> {
    values(core, page_id)
        .into_iter()
        .find(|(id, _)| *id == property_id)
        .map(|(_, value)| value)
}

/// The structure documents, the page index and the keyring, as one comparable value.
fn fingerprint(core: &mut Core) -> Value {
    let mut spaces_json = BTreeMap::new();
    for (space, open) in &core.docs.structures {
        let rows: Vec<String> = kb_storage::DocQueries::pages_index_by_space(&core.db, *space)
            .unwrap()
            .into_iter()
            .map(|row| format!("{:?}", row))
            .collect();
        spaces_json.insert(
            space.to_string(),
            json!({ "doc": open.model.to_portable(), "rows": rows }),
        );
    }
    let counts = core.security.keyring.counts();
    json!({
        "spaces": spaces_json,
        "keyring": [counts.spaces, counts.documents, counts.attachments, counts.tombstones],
    })
}

/// Appends portable blocks to a page's document, like an import or the MCP server does.
fn append_blocks(core: &mut Core, page_id: PageId, blocks: Value) {
    let appended = core
        .open_page(page_id)
        .unwrap()
        .model
        .append_portable(&json!({ "type": "doc", "content": blocks }))
        .unwrap();
    assert!(appended > 0);
    core.persist_page(page_id, None, UpdateOrigin::Core, ChangeSource::Core)
        .unwrap();
    core.page_doc_close(page_id).unwrap();
}

/// Appends a paragraph with a reference to `target`, the shape the editor writes for "See [[T]]".
fn append_page_ref(core: &mut Core, page_id: PageId, target: PageId, id: u8) {
    {
        let open = core.open_page(page_id).unwrap();
        let block = BlockId::from_random_bytes([id; 16]);
        let last = open.model.block_ids().last().copied();
        open.model
            .insert_block(last, BlockKind::Paragraph, "See ", block)
            .unwrap();
        open.model
            .insert_page_ref(
                block,
                BlockId::from_random_bytes([id.wrapping_add(100); 16]),
                target,
                None,
            )
            .unwrap();
    }
    core.persist_page(page_id, None, UpdateOrigin::Local, ChangeSource::Editor)
        .unwrap();
    core.page_doc_close(page_id).unwrap();
}

fn upload(core: &mut Core, space_id: SpaceId, name: &str, bytes: &[u8]) -> AttachmentInfo {
    let id = match call(
        core,
        Request::AttachmentBegin {
            space_id,
            name: name.into(),
            mime: "image/png".into(),
        },
    ) {
        Response::AttachmentStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    };
    for piece in bytes.chunks(CHUNK_SIZE / 3 + 1) {
        core.attachment_write(id, piece).unwrap();
    }
    match call(core, Request::AttachmentFinish { attachment_id: id }) {
        Response::Attachment(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn attachments_of(core: &mut Core, space_id: SpaceId) -> Vec<AttachmentId> {
    match call(core, Request::AttachmentList { space_id }) {
        Response::Attachments(list) => list.into_iter().map(|a| a.attachment_id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

fn read_attachment(core: &mut Core, id: AttachmentId) -> Result<Vec<u8>, ApiError> {
    let mut out = Vec::new();
    core.attachment_read(id, None, &mut |piece| {
        out.extend_from_slice(piece);
        Ok(())
    })
    .map(|_| out)
}

fn image_block(id: u8, attachment: AttachmentId) -> Value {
    json!({ "type": "image", "attrs": {
        "id": BlockId::from_random_bytes([id; 16]).to_string(),
        "attachmentId": attachment.to_string(),
        "alt": ""
    }})
}

fn events_of(core: &mut Core) -> Vec<Event> {
    core.drain_events()
}

fn doc_spaces(core: &Core, page_id: PageId) -> Vec<SpaceId> {
    core.security.keyring.doc_spaces(DocId::for_page(page_id))
}

#[test]
fn a_move_carries_pages_content_favourites_icons_and_rows() {
    let (_fx, mut core, personal, work) = world();
    let keep = create_page(&mut core, personal, None, "Stays");
    let top = create_page(&mut core, personal, None, "Roadmap");
    let child = create_page(&mut core, personal, Some(top.id), "Quarter one");
    let grandchild = create_page(&mut core, personal, Some(child.id), "Week two");
    let other_child = create_page(&mut core, personal, Some(top.id), "Quarter two");
    write_text(&mut core, top.id, "quarterly roadmap zebra", 1);
    write_text(&mut core, child.id, "milestones giraffe", 2);
    call(
        &mut core,
        Request::PageFavoriteSet {
            page_id: top.id,
            favorite: true,
        },
    );
    call(
        &mut core,
        Request::PageFavoriteSet {
            page_id: keep.id,
            favorite: true,
        },
    );
    call(
        &mut core,
        Request::PageSetIcon {
            page_id: top.id,
            icon: Some("\u{1F680}".into()),
        },
    );
    call(
        &mut core,
        Request::PageSetCover {
            page_id: top.id,
            cover: Some("gradient:sunset".into()),
            cover_y: Some(30),
        },
    );
    call(
        &mut core,
        Request::PageSetAppearance {
            page_id: top.id,
            font: Some(PageFont::Serif),
            small_text: Some(true),
            locked: Some(true),
        },
    );
    call(
        &mut core,
        Request::PageSetAppearance {
            page_id: child.id,
            font: Some(PageFont::Mono),
            small_text: None,
            locked: None,
        },
    );
    let content_before = page_content(&mut core, top.id);
    let info_before = page(&mut core, top.id);
    create_page(&mut core, work, None, "Already here");
    core.drain_events();

    let summary = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(
        summary.page_ids,
        vec![top.id, child.id, grandchild.id, other_child.id],
        "pre-order, the moved page first"
    );
    assert_eq!(
        (
            summary.attachments_moved,
            summary.attachments_left,
            summary.properties_created,
            summary.values_dropped,
            summary.relation_links_removed
        ),
        (0, 0, 0, 0, 0)
    );

    // Source: only the page that stayed. Target: the subtree after what was there, same shape.
    assert_eq!(outline(&tree(&mut core, personal)), "Stays\n");
    assert_eq!(
        outline(&tree(&mut core, work)),
        "Already here\nRoadmap\n  Quarter one\n    Week two\n  Quarter two\n"
    );
    assert_eq!(favorites(&mut core, personal), vec![keep.id]);
    assert_eq!(favorites(&mut core, work), vec![top.id]);
    let info_after = page(&mut core, top.id);
    assert_eq!(info_after.space_id, work);
    assert_eq!(
        PageInfo {
            space_id: work,
            ..info_before
        },
        info_after,
        "id, title, icon, cover, parent, times and values are unchanged"
    );
    assert_eq!(
        (info_after.cover.as_deref(), info_after.cover_y),
        (Some("gradient:sunset"), Some(30)),
        "the cover and its position moved with the page"
    );
    assert_eq!(
        (info_after.font, info_after.small_text, info_after.locked),
        (Some(PageFont::Serif), true, true),
        "the font, the small text and the lock moved with the page"
    );
    let child_after = page(&mut core, child.id);
    assert_eq!(
        (child_after.font, child_after.small_text, child_after.locked),
        (Some(PageFont::Mono), false, false)
    );
    assert_eq!(
        page(&mut core, grandchild.id).font,
        None,
        "a sub-page without a look arrives without one"
    );
    assert_eq!(page_content(&mut core, top.id), content_before);
    assert_eq!(page(&mut core, grandchild.id).parent_id, Some(child.id));
    assert_eq!(page(&mut core, child.id).space_id, work);

    // The rows follow the pages: the index, the documents, the text search.
    assert_eq!(
        kb_storage::DocQueries::pages_index_live_count(&core.db, personal).unwrap(),
        1
    );
    assert_eq!(
        kb_storage::DocQueries::pages_index_live_count(&core.db, work).unwrap(),
        5
    );
    assert_eq!(
        core.db
            .doc_get(DocId::for_page(top.id))
            .unwrap()
            .unwrap()
            .space_id,
        Some(work)
    );
    assert_eq!(search(&mut core, "zebra", Some(personal)), vec![]);
    assert_eq!(
        search(&mut core, "zebra", Some(work)),
        vec![(top.id, work)],
        "the text is found in the target space only"
    );
    assert_eq!(search(&mut core, "giraffe", None), vec![(child.id, work)]);
    assert_eq!(search(&mut core, "Roadmap", Some(personal)), vec![]);
    assert!(core.db.fts_page_count(work).unwrap() >= 5);

    // The residue is invisible: nothing in the source tree, trash or counts mentions it.
    let source = tree(&mut core, personal);
    assert!(source.trashed.is_empty());
    assert_eq!(
        core.docs.structures[&personal].model.moved_out().len(),
        4,
        "four residue nodes"
    );
    assert!(
        core.docs.structures[&personal]
            .model
            .check_invariants()
            .is_empty()
    );
    assert!(
        core.docs.structures[&work]
            .model
            .check_invariants()
            .is_empty()
    );
    assert_eq!(
        spaces(&mut core)
            .iter()
            .map(|s| (s.id, s.page_count))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([(personal, 1), (work, 5)])
    );

    // The events: the move itself, both trees, the space counts.
    let events = events_of(&mut core);
    assert!(events.contains(&Event::PagesMovedSpace {
        from_space_id: personal,
        to_space_id: work,
        page_ids: summary.page_ids.clone(),
    }));
    assert!(events.contains(&Event::StructureChanged { space_id: personal }));
    assert!(events.contains(&Event::StructureChanged { space_id: work }));
    assert!(events.contains(&Event::SpacesChanged));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::PropertySchemaChanged { .. }))
    );

    // A move is not an undo step: undo in either space leaves the page where it is.
    for space in [personal, work] {
        call(&mut core, Request::StructureUndo { space_id: space });
    }
    assert_eq!(page(&mut core, top.id).space_id, work);
    assert_eq!(outline(&tree(&mut core, personal)), "Stays\n");
}

#[test]
fn the_plan_counts_and_writes_nothing() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");
    let owner = prop_create(&mut core, personal, "Owner", PropertyKind::Text);
    set_value(
        &mut core,
        child.id,
        owner.id,
        PropertyValue::Text("Ben".into()),
    );
    let pic = upload(&mut core, personal, "a.png", b"png bytes");
    append_blocks(
        &mut core,
        child.id,
        json!([image_block(1, pic.attachment_id)]),
    );
    core.drain_events();
    let before = fingerprint(&mut core);
    let rows_before = core.db.attachment_get(pic.attachment_id).unwrap();
    let docs_before = core.db.doc_get(DocId::for_page(top.id)).unwrap();

    let summary = plan(&mut core, top.id, work, None).unwrap();
    assert_eq!(summary.page_ids, vec![top.id, child.id]);
    assert_eq!(summary.properties_created, 1, "Owner is missing in Work");
    assert_eq!(summary.attachments_moved, 1);
    assert_eq!(summary.attachments_left, 0);

    assert_eq!(
        fingerprint(&mut core),
        before,
        "structures, rows and keys are untouched"
    );
    assert_eq!(
        core.db.attachment_get(pic.attachment_id).unwrap(),
        rows_before
    );
    assert_eq!(
        core.db.doc_get(DocId::for_page(top.id)).unwrap(),
        docs_before
    );
    assert!(core.drain_events().is_empty(), "the plan announces nothing");
    assert!(schema(&mut core, work).iter().all(|p| p.name != "Owner"));

    // The move then does what the plan said.
    let moved = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(moved, summary);
}

#[test]
fn a_move_is_refused_with_the_field_that_is_wrong() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let there = create_page(&mut core, work, None, "There");
    let trashed_there = create_page(&mut core, work, None, "Trashed there");
    call(
        &mut core,
        Request::PageTrash {
            page_id: trashed_there.id,
        },
    );
    let gone = create_page(&mut core, personal, None, "Gone");
    call(&mut core, Request::PageTrash { page_id: gone.id });
    let third = create_space(&mut core, "Third").id;
    let elsewhere = create_page(&mut core, third, None, "Elsewhere");
    let field = |err: ApiError| -> String {
        assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
        err.details.unwrap()["field"].as_str().unwrap().to_owned()
    };

    // The same space: use PageMove.
    assert_eq!(
        field(move_to(&mut core, top.id, personal, None).unwrap_err()),
        "spaceId"
    );
    assert_eq!(
        field(plan(&mut core, top.id, personal, None).unwrap_err()),
        "spaceId"
    );
    // A page in the trash.
    assert_eq!(
        field(move_to(&mut core, gone.id, work, None).unwrap_err()),
        "pageId"
    );
    // A parent that is not a live page of the target.
    assert_eq!(
        field(move_to(&mut core, top.id, work, Some(trashed_there.id)).unwrap_err()),
        "parentId"
    );
    assert_eq!(
        field(move_to(&mut core, top.id, work, Some(elsewhere.id)).unwrap_err()),
        "parentId"
    );
    assert_eq!(
        field(plan(&mut core, top.id, work, Some(top.id)).unwrap_err()),
        "parentId"
    );
    // Unknown page and unknown space.
    let missing = PageId::from_bytes([9; 16]);
    assert_eq!(
        move_to(&mut core, missing, work, None).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        move_to(&mut core, top.id, SpaceId::from_bytes([9; 16]), None)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    // Nothing changed.
    assert_eq!(outline(&tree(&mut core, personal)), "Top\n");
    assert_eq!(outline(&tree(&mut core, work)), "There\n");

    // Under a parent of the target, at an index.
    let summary = match call(
        &mut core,
        Request::PageMoveToSpace {
            page_id: top.id,
            space_id: work,
            parent_id: Some(there.id),
            index: Some(0),
        },
    ) {
        Response::PagesMovedToSpace(summary) => summary,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(summary.page_ids, vec![top.id]);
    assert_eq!(page(&mut core, top.id).parent_id, Some(there.id));
}

#[test]
fn a_subtree_of_more_than_ten_thousand_pages_is_refused() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let batch: Vec<kb_model::NewPage> = (0..10_000u32)
        .map(|n| {
            let mut bytes = [0u8; 16];
            bytes[..4].copy_from_slice(&n.to_le_bytes());
            bytes[15] = 0x77;
            kb_model::NewPage {
                page_id: PageId::from_random_bytes(bytes),
                parent: Some(top.id),
                title: format!("p{n}"),
                icon: None,
                cover: None,
                cover_y: None,
            }
        })
        .collect();
    core.structure_mut(personal)
        .unwrap()
        .model
        .create_pages(&batch, 5)
        .unwrap();
    core.persist_structure(personal, None, UpdateOrigin::Local)
        .unwrap();
    // 10,001 pages with the top page.
    let err = plan(&mut core, top.id, work, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(err.details.unwrap()["field"], "pageId");
    // One page fewer fits.
    let first = batch[0].page_id;
    core.structure_mut(personal)
        .unwrap()
        .model
        .trash_page(first, 6)
        .unwrap();
    core.persist_structure(personal, None, UpdateOrigin::Local)
        .unwrap();
    let summary = plan(&mut core, top.id, work, None).unwrap();
    assert_eq!(summary.page_ids.len(), 10_000);
}

#[test]
fn backlinks_stay_and_follow_the_moved_page() {
    let (_fx, mut core, personal, work) = world();
    let target = create_page(&mut core, personal, None, "Target");
    let source = create_page(&mut core, personal, None, "Source");
    append_page_ref(&mut core, source.id, target.id, 1);
    let backlinks =
        |core: &mut Core, id: PageId| match call(core, Request::BacklinksGet { page_id: id }) {
            Response::Backlinks(links) => links
                .into_iter()
                .map(|l| (l.page_id, l.space_id))
                .collect::<Vec<_>>(),
            other => panic!("unexpected {other:?}"),
        };
    assert_eq!(backlinks(&mut core, target.id), vec![(source.id, personal)]);

    move_to(&mut core, target.id, work, None).unwrap();
    assert_eq!(
        backlinks(&mut core, target.id),
        vec![(source.id, personal)],
        "the link is by page id, so it still finds its target across spaces"
    );
    // Moving the page that holds the link: the backlink names its new space.
    move_to(&mut core, source.id, work, None).unwrap();
    assert_eq!(backlinks(&mut core, target.id), vec![(source.id, work)]);
}

#[test]
fn values_follow_by_name_and_missing_properties_are_created() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");

    // Personal: Owner (text, also in Work), Stage (select, also in Work with other options),
    // Due (date, missing in Work), Effort (number; Work has a text property of that name), Tags.
    let owner = prop_create(&mut core, personal, "Owner", PropertyKind::Text);
    let stage = prop_create(&mut core, personal, "Stage", PropertyKind::Select);
    let draft = option_add(&mut core, personal, stage.id, "Draft");
    let final_ = option_add(&mut core, personal, stage.id, "Final");
    let due = prop_create(&mut core, personal, "Due", PropertyKind::Date);
    let effort = prop_create(&mut core, personal, "Effort", PropertyKind::Number);
    let tags = kb_types::properties::TAGS_PROPERTY_ID;
    let urgent = option_add(&mut core, personal, tags, "urgent");
    let _unused = option_add(&mut core, personal, tags, "unused");
    set_value(
        &mut core,
        top.id,
        owner.id,
        PropertyValue::Text("Ben".into()),
    );
    set_value(
        &mut core,
        top.id,
        stage.id,
        PropertyValue::Select(final_.id),
    );
    set_value(
        &mut core,
        top.id,
        due.id,
        PropertyValue::Date(kb_types::DateValue {
            date: "2026-10-13".into(),
            time: Some("09:30".into()),
        }),
    );
    set_value(&mut core, top.id, effort.id, PropertyValue::Number(12.5));
    set_value(
        &mut core,
        child.id,
        tags,
        PropertyValue::MultiSelect(vec![urgent.id]),
    );
    set_value(
        &mut core,
        child.id,
        stage.id,
        PropertyValue::Select(draft.id),
    );

    let work_owner = prop_create(&mut core, work, "owner", PropertyKind::Text);
    let work_stage = prop_create(&mut core, work, "STAGE", PropertyKind::Select);
    let work_final = option_add(&mut core, work, work_stage.id, "final");
    let work_effort = prop_create(&mut core, work, "Effort", PropertyKind::Text);
    let work_schema_before = schema(&mut core, work);
    core.drain_events();

    let summary = plan(&mut core, top.id, work, None).unwrap();
    assert_eq!(summary.properties_created, 1, "only Due is missing");
    assert_eq!(summary.values_dropped, 0);
    let summary = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!((summary.properties_created, summary.values_dropped), (1, 0));
    let events = events_of(&mut core);
    assert!(events.contains(&Event::PropertySchemaChanged { space_id: work }));

    let after = schema(&mut core, work);
    assert_eq!(after.len(), work_schema_before.len() + 1);
    let created_due = after
        .iter()
        .find(|p| p.name == "Due" && p.kind == PropertyKind::Date)
        .expect("Due was created with its kind");
    assert!(
        after
            .iter()
            .filter(|p| p.name == "Owner" || p.name == "owner")
            .count()
            == 1
    );

    let top_values = values(&mut core, top.id);
    let get = |id: PropertyId| {
        top_values
            .iter()
            .find(|(p, _)| *p == id)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(get(work_owner.id), Some(PropertyValue::Text("Ben".into())));
    assert_eq!(
        get(work_stage.id),
        Some(PropertyValue::Select(work_final.id)),
        "the option follows by name (case folded)"
    );
    assert_eq!(
        get(created_due.id),
        Some(PropertyValue::Date(kb_types::DateValue {
            date: "2026-10-13".into(),
            time: Some("09:30".into()),
        }))
    );
    assert_eq!(
        get(work_effort.id),
        Some(PropertyValue::Text("12.5".into())),
        "a number follows into a text property by its text"
    );

    // The child's value of a source option the target lacks adds the option; Tags follows by name.
    let stage_options: Vec<&str> = after
        .iter()
        .find(|p| p.id == work_stage.id)
        .unwrap()
        .options
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert_eq!(
        stage_options,
        ["final", "Draft"],
        "Draft was added for the child's value"
    );
    let tags_after = after.iter().find(|p| p.id == tags).unwrap();
    let tag_names: Vec<&str> = tags_after.options.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        tag_names,
        ["urgent"],
        "an option nothing refers to is not copied"
    );
    assert!(matches!(
        value_of(&mut core, child.id, tags),
        Some(PropertyValue::MultiSelect(ids)) if ids == vec![tags_after.options[0].id]
    ));

    // The derived rows are the target's: filtering by the property finds the page in Work.
    let found = match call(
        &mut core,
        Request::PagesByProperty {
            space_id: work,
            property_id: work_owner.id,
            filter: kb_types::PropertyFilter::HasValue,
        },
    ) {
        Response::PageIds(ids) => ids,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(found, vec![top.id]);
    let in_source = match call(
        &mut core,
        Request::PagesByProperty {
            space_id: personal,
            property_id: owner.id,
            filter: kb_types::PropertyFilter::HasValue,
        },
    ) {
        Response::PageIds(ids) => ids,
        other => panic!("unexpected {other:?}"),
    };
    assert!(in_source.is_empty(), "the source space no longer lists it");
}

#[test]
fn a_full_target_schema_drops_the_values_that_cannot_follow() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let due = prop_create(&mut core, personal, "Due", PropertyKind::Date);
    let score = prop_create(&mut core, personal, "Score", PropertyKind::Number);
    set_value(
        &mut core,
        top.id,
        due.id,
        PropertyValue::Date(kb_types::DateValue {
            date: "2026-10-13".into(),
            time: None,
        }),
    );
    set_value(&mut core, top.id, score.id, PropertyValue::Number(3.0));
    // Fill Work up to the limit (Tags counts).
    while schema(&mut core, work).len() < kb_types::properties::MAX_PROPERTIES {
        let n = schema(&mut core, work).len();
        prop_create(&mut core, work, &format!("Filler {n}"), PropertyKind::Text);
    }
    let summary = plan(&mut core, top.id, work, None).unwrap();
    assert_eq!(summary.properties_created, 0);
    assert_eq!(summary.values_dropped, 2);
    let summary = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!((summary.properties_created, summary.values_dropped), (0, 2));
    assert!(values(&mut core, top.id).is_empty());
    assert_eq!(
        schema(&mut core, work).len(),
        kb_types::properties::MAX_PROPERTIES
    );
}

#[test]
fn relations_keep_the_pages_that_move_and_both_sides_lose_the_links_that_cross() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");
    let stays = create_page(&mut core, personal, None, "Stays");
    let forward = match call(
        &mut core,
        Request::PropertyCreate {
            space_id: personal,
            name: "Blocks".into(),
            kind: PropertyKind::Relation,
            config: Some(kb_types::PropertyConfig::Relation {
                two_way: true,
                paired_property_id: None,
            }),
        },
    ) {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    };
    let back = match forward.config {
        Some(kb_types::PropertyConfig::Relation {
            paired_property_id: Some(back),
            ..
        }) => back,
        ref other => panic!("{other:?}"),
    };
    // top blocks child and stays; stays also blocks top (the other direction, a second link).
    set_value(
        &mut core,
        top.id,
        forward.id,
        PropertyValue::Relation(vec![child.id, stays.id]),
    );
    set_value(
        &mut core,
        stays.id,
        forward.id,
        PropertyValue::Relation(vec![top.id]),
    );
    assert_eq!(
        value_of(&mut core, child.id, back),
        Some(PropertyValue::Relation(vec![top.id])),
        "the paired side shows the link"
    );

    let summary = plan(&mut core, top.id, work, None).unwrap();
    // Stored entries that cross: top lists stays (1), stays lists top (1), stays' back side lists
    // top (the pair writes both sides, 1), top's back side lists stays (1).
    assert!(summary.relation_links_removed >= 2, "{summary:?}");
    assert_eq!(
        summary.properties_created, 2,
        "a two-way pair is created as a pair"
    );
    let summary = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(summary.properties_created, 2);

    let work_schema = schema(&mut core, work);
    let new_forward = work_schema
        .iter()
        .find(|p| p.name == "Blocks")
        .expect("the forward half exists in Work");
    let new_back = match new_forward.config {
        Some(kb_types::PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(back),
        }) => back,
        ref other => panic!("not a pair: {other:?}"),
    };
    // top still blocks child (both moved); the pair shows it on the child's back side.
    assert_eq!(
        value_of(&mut core, top.id, new_forward.id),
        Some(PropertyValue::Relation(vec![child.id]))
    );
    assert_eq!(
        value_of(&mut core, child.id, new_back),
        Some(PropertyValue::Relation(vec![top.id]))
    );
    // The page that stayed has lost every entry that listed a moved page, stored and shown.
    assert_eq!(value_of(&mut core, stays.id, forward.id), None);
    assert_eq!(value_of(&mut core, stays.id, back), None);
    let stored = core.docs.structures[&personal]
        .model
        .property_index_all()
        .into_iter()
        .filter(|(page, _)| *page == stays.id)
        .flat_map(|(_, entry)| entry.rows)
        .filter(|row| row.related_page_id.is_some())
        .count();
    assert_eq!(stored, 0, "the stored value was rewritten, not just hidden");
}

#[test]
fn the_views_of_moved_pages_follow_the_property_mapping() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let due = prop_create(&mut core, personal, "Due", PropertyKind::Date);
    let stage = prop_create(&mut core, personal, "Stage", PropertyKind::Select);
    let draft = option_add(&mut core, personal, stage.id, "Draft");
    let work_stage = prop_create(&mut core, work, "Stage", PropertyKind::Select);
    let work_draft = option_add(&mut core, work, work_stage.id, "Draft");
    append_blocks(
        &mut core,
        top.id,
        json!([{ "type": "pageList", "attrs": {
            "id": BlockId::from_random_bytes([5; 16]).to_string(),
            "source": { "kind": "children" },
            "layout": "board",
            "properties": [due.id.to_string(), stage.id.to_string()],
            "sort": { "by": due.id.to_string(), "dir": "desc" },
            "filters": [{ "propertyId": stage.id.to_string(), "op": "hasOption", "value": draft.id.to_string() }],
            "groupBy": stage.id.to_string(),
            "columnWidths": { "title": 300, due.id.to_string(): 140 },
            "calculations": { "title": "count_all", due.id.to_string(): "date_range", "a-key-that-is-no-column": "sum" },
        }}]),
    );
    core.drain_events();
    move_to(&mut core, top.id, work, None).unwrap();
    let events = events_of(&mut core);
    assert!(
        events.contains(&Event::PageContentChanged {
            page_id: top.id,
            source: ChangeSource::Core,
        }),
        "{events:?}"
    );

    let work_schema = schema(&mut core, work);
    let new_due = work_schema.iter().find(|p| p.name == "Due").unwrap().id;
    let lists = core.open_page(top.id).unwrap().model.page_lists();
    assert_eq!(lists.len(), 1);
    let attrs = &lists[0].1;
    assert_eq!(
        attrs["properties"],
        json!([new_due.to_string(), work_stage.id.to_string()])
    );
    assert_eq!(
        attrs["sort"],
        json!({ "by": new_due.to_string(), "dir": "desc" })
    );
    assert_eq!(
        attrs["filters"],
        json!([{ "propertyId": work_stage.id.to_string(), "op": "hasOption", "value": work_draft.id.to_string() }])
    );
    assert_eq!(attrs["groupBy"], json!(work_stage.id.to_string()));
    assert_eq!(
        attrs["columnWidths"],
        json!({ "title": 300, new_due.to_string(): 140 })
    );
    assert_eq!(
        attrs["calculations"],
        json!({ "title": "count_all", new_due.to_string(): "date_range" })
    );
    assert_eq!(
        attrs["layout"],
        json!("board"),
        "the rest of the view is as it was"
    );
}

#[test]
fn every_view_of_a_moved_block_follows_the_property_mapping_and_the_mirror_stays_equal() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let due = prop_create(&mut core, personal, "Due", PropertyKind::Date);
    let end = prop_create(&mut core, personal, "End", PropertyKind::Date);
    let stage = prop_create(&mut core, personal, "Stage", PropertyKind::Select);
    let draft = option_add(&mut core, personal, stage.id, "Draft");
    let work_stage = prop_create(&mut core, work, "Stage", PropertyKind::Select);
    let work_draft = option_add(&mut core, work, work_stage.id, "Draft");
    let (due_id, end_id, stage_id, draft_id) = (
        due.id.to_string(),
        end.id.to_string(),
        stage.id.to_string(),
        draft.id.to_string(),
    );
    let has_draft = json!({ "propertyId": stage_id, "op": "hasOption", "value": draft_id });
    append_blocks(
        &mut core,
        top.id,
        json!([{ "type": "pageList", "attrs": {
            "id": BlockId::from_random_bytes([5; 16]).to_string(),
            "source": { "kind": "children" },
            "layout": "table",
            "properties": [due_id, stage_id],
            "sort": { "by": due_id, "dir": "asc" },
            "filters": [has_draft],
            "columnWidths": { "title": 320, due_id.clone(): 120 },
            "views": [
                { "id": "k3f9a2bc", "name": "", "layout": "table", "properties": [due_id, stage_id],
                  "sort": { "by": due_id, "dir": "asc" },
                  "filter": { "op": "and", "items": [
                      has_draft,
                      { "op": "or", "items": [
                          { "propertyId": due_id, "op": "isSet" },
                          { "propertyId": end_id, "op": "isSet" } ] } ] },
                  "columnWidths": { "title": 320, due_id.clone(): 120 } },
                { "id": "b81d00e4", "name": "By stage", "layout": "board", "properties": [],
                  "sort": { "by": "title", "dir": "asc" }, "groupBy": stage_id,
                  "hiddenGroups": [draft_id, "none"], "openIn": "center" },
                { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "properties": [stage_id],
                  "sort": { "by": "updated", "dir": "asc" }, "dateBy": due_id, "endBy": end_id },
                { "id": "e2a9b6f0", "name": "", "layout": "gallery", "properties": [],
                  "sort": { "by": "created", "dir": "desc" }, "dateBy": "created", "cardSize": "l" },
            ]
        }}]),
    );
    move_to(&mut core, top.id, work, None).unwrap();

    let work_schema = schema(&mut core, work);
    let new_due = work_schema
        .iter()
        .find(|p| p.name == "Due")
        .unwrap()
        .id
        .to_string();
    let new_end = work_schema
        .iter()
        .find(|p| p.name == "End")
        .unwrap()
        .id
        .to_string();
    let new_stage = work_stage.id.to_string();
    let new_draft = work_draft.id.to_string();
    assert_eq!(
        work_schema
            .iter()
            .filter(|p| p.kind == PropertyKind::Date)
            .count(),
        2,
        "the properties only a view names are created in the target"
    );
    let lists = core.open_page(top.id).unwrap().model.page_lists();
    assert_eq!(lists.len(), 1);
    let attrs = &lists[0].1;
    let new_draft_filter =
        json!({ "propertyId": new_stage, "op": "hasOption", "value": new_draft });

    // The block's own attributes (the mirror of the first view).
    assert_eq!(attrs["properties"], json!([new_due, new_stage]));
    assert_eq!(attrs["sort"], json!({ "by": new_due, "dir": "asc" }));
    assert_eq!(attrs["filters"], json!([new_draft_filter]));
    assert_eq!(
        attrs["columnWidths"],
        json!({ "title": 320, new_due.clone(): 120 })
    );

    // The views, through the same code.
    let views = attrs["views"].as_array().unwrap();
    assert_eq!(views.len(), 4);
    assert_eq!(views[0]["properties"], attrs["properties"]);
    assert_eq!(views[0]["sort"], attrs["sort"]);
    assert_eq!(views[0]["columnWidths"], attrs["columnWidths"]);
    assert_eq!(
        views[0]["filter"],
        json!({ "op": "and", "items": [
            new_draft_filter,
            { "op": "or", "items": [
                { "propertyId": new_due, "op": "isSet" },
                { "propertyId": new_end, "op": "isSet" } ] } ] })
    );
    assert_eq!(views[1]["groupBy"], json!(new_stage));
    assert_eq!(views[1]["hiddenGroups"], json!([new_draft, "none"]));
    assert_eq!(views[1]["openIn"], json!("center"));
    assert_eq!(views[2]["dateBy"], json!(new_due));
    assert_eq!(views[2]["endBy"], json!(new_end));
    assert_eq!(views[2]["properties"], json!([new_stage]));
    assert_eq!(views[3]["dateBy"], json!("created"));
    assert_eq!(views[3]["cardSize"], json!("l"));
    // Ids, names and layouts are untouched.
    let heads: Vec<(&str, &str, &str)> = views
        .iter()
        .map(|v| {
            (
                v["id"].as_str().unwrap(),
                v["name"].as_str().unwrap(),
                v["layout"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        heads,
        [
            ("k3f9a2bc", "", "table"),
            ("b81d00e4", "By stage", "board"),
            ("7c0a55d1", "Schedule", "calendar"),
            ("e2a9b6f0", "", "gallery")
        ]
    );
}

#[test]
fn the_sorts_the_new_operators_and_the_linked_sources_of_a_moved_block_follow_the_mapping() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let other = create_page(&mut core, personal, None, "Other");
    let due = prop_create(&mut core, personal, "Due", PropertyKind::Date);
    let stage = prop_create(&mut core, personal, "Stage", PropertyKind::Select);
    let draft = option_add(&mut core, personal, stage.id, "Draft");
    let work_stage = prop_create(&mut core, work, "Stage", PropertyKind::Select);
    let work_draft = option_add(&mut core, work, work_stage.id, "Draft");
    let (due_id, stage_id, draft_id, other_id) = (
        due.id.to_string(),
        stage.id.to_string(),
        draft.id.to_string(),
        other.id.to_string(),
    );
    let filters = json!({ "op": "and", "items": [
        { "propertyId": stage_id, "op": "lacksOption", "value": draft_id },
        { "propertyId": stage_id, "op": "lacksOption", "value": [draft_id] },
        { "propertyId": due_id, "op": "within", "value": { "relative": "pastDays", "n": 7 } },
        { "propertyId": due_id, "op": "onOrBefore", "value": { "relative": "today" } },
        { "propertyId": stage_id, "op": "notRelatesTo", "value": other_id },
    ] });
    let block = |n: u8, source: Value| {
        json!({ "type": "pageList", "attrs": {
            "id": BlockId::from_random_bytes([n; 16]).to_string(),
            "source": source,
            "layout": "table",
            "properties": [],
            "sort": { "by": due_id, "dir": "desc" },
            "views": [{
                "id": "k3f9a2bc", "name": "", "layout": "table", "properties": [],
                "sort": { "by": due_id, "dir": "desc" },
                "sorts": [
                    { "by": due_id, "dir": "desc" },
                    { "by": "created", "dir": "asc" },
                    { "by": stage_id, "dir": "asc" },
                    { "by": "title", "dir": "desc" }
                ],
                "filter": filters
            }]
        }})
    };
    append_blocks(
        &mut core,
        top.id,
        json!([
            block(5, json!({ "kind": "page", "pageId": other_id })),
            block(6, json!({ "kind": "space" }))
        ]),
    );
    move_to(&mut core, top.id, work, None).unwrap();

    let work_schema = schema(&mut core, work);
    let new_due = work_schema
        .iter()
        .find(|p| p.name == "Due")
        .unwrap()
        .id
        .to_string();
    let new_stage = work_stage.id.to_string();
    let new_draft = work_draft.id.to_string();
    let lists = core.open_page(top.id).unwrap().model.page_lists();
    assert_eq!(lists.len(), 2);
    // The linked sources are kept as they are (page ids are kept by a move).
    assert_eq!(
        lists[0].1["source"],
        json!({ "kind": "page", "pageId": other_id })
    );
    assert_eq!(lists[1].1["source"], json!({ "kind": "space" }));
    for (_, attrs) in &lists {
        // The block's mirror of the first view follows the first key.
        assert_eq!(attrs["sort"], json!({ "by": new_due, "dir": "desc" }));
        let view = &attrs["views"][0];
        assert_eq!(view["sort"], attrs["sort"]);
        assert_eq!(
            view["sorts"],
            json!([
                { "by": new_due, "dir": "desc" },
                { "by": "created", "dir": "asc" },
                { "by": new_stage, "dir": "asc" },
                { "by": "title", "dir": "desc" }
            ])
        );
        assert_eq!(
            view["filter"],
            json!({ "op": "and", "items": [
                { "propertyId": new_stage, "op": "lacksOption", "value": new_draft },
                { "propertyId": new_stage, "op": "lacksOption", "value": [new_draft] },
                { "propertyId": new_due, "op": "within", "value": { "relative": "pastDays", "n": 7 } },
                { "propertyId": new_due, "op": "onOrBefore", "value": { "relative": "today" } },
                { "propertyId": new_stage, "op": "notRelatesTo", "value": other_id },
            ] })
        );
    }
}

#[test]
fn a_moved_page_with_a_picture_is_answered_by_the_target_space_and_no_longer_by_the_source() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let picture = upload(&mut core, personal, "picture.png", &[1, 2, 3, 4]);
    append_blocks(
        &mut core,
        top.id,
        json!([image_block(1, picture.attachment_id)]),
    );
    let first_images = |core: &mut Core, space_id: SpaceId| -> Vec<(PageId, AttachmentId)> {
        match call(
            core,
            Request::PageFirstImages {
                space_id,
                page_ids: vec![top.id],
            },
        ) {
            Response::PageFirstImages(list) => list
                .into_iter()
                .map(|found| (found.page_id, found.attachment_id))
                .collect(),
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(
        first_images(&mut core, personal),
        [(top.id, picture.attachment_id)]
    );
    assert!(first_images(&mut core, work).is_empty());

    move_to(&mut core, top.id, work, None).unwrap();
    assert!(first_images(&mut core, personal).is_empty());
    assert_eq!(
        first_images(&mut core, work),
        [(top.id, picture.attachment_id)]
    );
}

#[test]
fn attachments_only_the_moved_pages_use_move_and_the_others_stay() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let stays = create_page(&mut core, personal, None, "Stays");
    let bytes_a = vec![7u8; CHUNK_SIZE + 123];
    let bytes_b = b"shared picture".to_vec();
    let only = upload(&mut core, personal, "only.png", &bytes_a);
    let shared = upload(&mut core, personal, "shared.png", &bytes_b);
    append_blocks(
        &mut core,
        top.id,
        json!([
            image_block(1, only.attachment_id),
            image_block(2, shared.attachment_id)
        ]),
    );
    append_blocks(
        &mut core,
        stays.id,
        json!([image_block(3, shared.attachment_id)]),
    );
    let chunks_before = core.blob_store.list().unwrap().len();
    core.drain_events();

    let summary = plan(&mut core, top.id, work, None).unwrap();
    assert_eq!(
        (summary.attachments_moved, summary.attachments_left),
        (1, 1)
    );
    let summary = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(
        (summary.attachments_moved, summary.attachments_left),
        (1, 1)
    );

    assert_eq!(attachments_of(&mut core, work), vec![only.attachment_id]);
    assert_eq!(
        attachments_of(&mut core, personal),
        vec![shared.attachment_id]
    );
    assert_eq!(
        core.db
            .attachment_get(only.attachment_id)
            .unwrap()
            .unwrap()
            .space_id,
        work
    );
    assert!(
        core.docs.structures[&work]
            .model
            .attachment(only.attachment_id)
            .is_some()
    );
    assert!(
        core.docs.structures[&personal]
            .model
            .attachment(only.attachment_id)
            .is_none()
    );
    assert_eq!(
        core.blob_store.list().unwrap().len(),
        chunks_before,
        "no chunk was written or removed"
    );
    // Both are still readable from the moved page: the moved one under its new record, the shared
    // one across spaces.
    assert_eq!(
        read_attachment(&mut core, only.attachment_id).unwrap(),
        bytes_a
    );
    assert_eq!(
        read_attachment(&mut core, shared.attachment_id).unwrap(),
        bytes_b
    );
    let subject = KeySubject::Attachment {
        attachment_id: only.attachment_id,
    };
    assert!(core.security.keyring.record(work, &subject).is_some());
    assert!(
        core.security.keyring.record(personal, &subject).is_some(),
        "the source's record stays"
    );
    let events = events_of(&mut core);
    assert!(events.contains(&Event::AttachmentsChanged { space_id: personal }));
    assert!(events.contains(&Event::AttachmentsChanged { space_id: work }));

    // The garbage collector does not take it for unclaimed: the manifest is in the target.
    let now = core.clock.now_ms();
    core.attachment_gc(now).unwrap();
    assert_eq!(
        read_attachment(&mut core, only.attachment_id).unwrap(),
        bytes_a
    );

    // Purging it erases every record.
    call(
        &mut core,
        Request::AttachmentDelete {
            attachment_id: only.attachment_id,
        },
    );
    for space in [personal, work] {
        assert!(
            core.security.keyring.is_tombstoned(space, &subject),
            "{space}"
        );
    }
    assert_eq!(
        read_attachment(&mut core, only.attachment_id)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[test]
fn an_attachment_icon_and_a_files_value_count_as_references() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let shared = create_page(&mut core, personal, None, "Other");
    let icon = upload(&mut core, personal, "icon.png", b"icon");
    let attached = upload(&mut core, personal, "doc.png", b"doc");
    let both = upload(&mut core, personal, "both.png", b"both");
    call(
        &mut core,
        Request::PageSetIcon {
            page_id: top.id,
            icon: Some(format!("attachment:{}", icon.attachment_id)),
        },
    );
    let files = prop_create(&mut core, personal, "Files", PropertyKind::Files);
    set_value(
        &mut core,
        top.id,
        files.id,
        PropertyValue::Files(vec![attached.attachment_id, both.attachment_id]),
    );
    set_value(
        &mut core,
        shared.id,
        files.id,
        PropertyValue::Files(vec![both.attachment_id]),
    );
    let summary = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(
        (summary.attachments_moved, summary.attachments_left),
        (2, 1)
    );
    assert_eq!(
        attachments_of(&mut core, work)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([icon.attachment_id, attached.attachment_id])
    );
    assert_eq!(
        attachments_of(&mut core, personal),
        vec![both.attachment_id]
    );
    // The icon stays on the page and the files value keeps naming its attachments.
    assert_eq!(
        page(&mut core, top.id).icon,
        Some(format!("attachment:{}", icon.attachment_id))
    );
    let files_in_work = schema(&mut core, work)
        .into_iter()
        .find(|p| p.name == "Files")
        .unwrap();
    assert_eq!(
        value_of(&mut core, top.id, files_in_work.id),
        Some(PropertyValue::Files(vec![
            attached.attachment_id,
            both.attachment_id
        ]))
    );
    // The references are the page's wherever it is: nothing became collectable.
    for id in [
        icon.attachment_id,
        attached.attachment_id,
        both.attachment_id,
    ] {
        let info = match call(&mut core, Request::AttachmentGet { attachment_id: id }) {
            Response::Attachment(info) => info,
            other => panic!("unexpected {other:?}"),
        };
        assert!(info.references >= 1, "{id}: {info:?}");
        assert_eq!(info.unreferenced_since, None);
    }
}

#[test]
fn an_attachment_cover_counts_as_a_reference_moves_with_its_page_and_stays_when_another_page_uses_it()
 {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let sub = create_page(&mut core, personal, Some(top.id), "Sub");
    let shared = create_page(&mut core, personal, None, "Other");
    let banner = upload(&mut core, personal, "banner.png", b"banner");
    let common = upload(&mut core, personal, "common.png", b"common");
    let cover =
        |attachment: &AttachmentInfo| Some(format!("attachment:{}", attachment.attachment_id));
    call(
        &mut core,
        Request::PageSetCover {
            page_id: top.id,
            cover: cover(&banner),
            cover_y: Some(20),
        },
    );
    for page_id in [sub.id, shared.id] {
        call(
            &mut core,
            Request::PageSetCover {
                page_id,
                cover: cover(&common),
                cover_y: None,
            },
        );
    }

    // The banner only the moved page uses moves; the one another page also uses stays.
    let summary = move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(
        (summary.attachments_moved, summary.attachments_left),
        (1, 1)
    );
    assert_eq!(attachments_of(&mut core, work), vec![banner.attachment_id]);
    assert_eq!(
        attachments_of(&mut core, personal),
        vec![common.attachment_id]
    );
    assert_eq!(
        read_attachment(&mut core, banner.attachment_id).unwrap(),
        b"banner"
    );

    // The covers stay on the pages, with the position.
    let moved = page(&mut core, top.id);
    assert_eq!(moved.cover, cover(&banner));
    assert_eq!(moved.cover_y, Some(20));
    assert_eq!(page(&mut core, sub.id).cover, cover(&common));
    assert_eq!(page(&mut core, shared.id).cover, cover(&common));

    // The references follow the pages: the rows are in the target for the moved pages and in the
    // source for the page that stayed, and nothing became collectable.
    let mut in_work = vec![
        (top.id, banner.attachment_id),
        (sub.id, common.attachment_id),
    ];
    in_work.sort();
    assert_eq!(core.db.icon_refs_of_space(work).unwrap(), in_work);
    assert_eq!(
        core.db.icon_refs_of_space(personal).unwrap(),
        vec![(shared.id, common.attachment_id)]
    );
    for id in [banner.attachment_id, common.attachment_id] {
        let info = match call(&mut core, Request::AttachmentGet { attachment_id: id }) {
            Response::Attachment(info) => info,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(info.unreferenced_since, None, "{id}: {info:?}");
    }
    assert_eq!(
        match call(
            &mut core,
            Request::AttachmentGet {
                attachment_id: common.attachment_id
            }
        ) {
            Response::Attachment(info) => info.references,
            other => panic!("unexpected {other:?}"),
        },
        2,
        "the moved sub-page and the page that stayed"
    );
}

#[test]
fn a_new_key_record_is_written_under_the_target_and_the_source_record_stays() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");
    let no_doc = create_page(&mut core, personal, Some(child.id), "Never opened");
    // A page whose document was never created has no key yet.
    core.db.doc_delete(DocId::for_page(no_doc.id)).unwrap();
    core.keyring_tombstone_docs(personal, &[DocId::for_page(no_doc.id)])
        .unwrap();
    let before: Vec<_> = core
        .security
        .keyring
        .doc_records(DocId::for_page(top.id))
        .into_iter()
        .cloned()
        .collect();
    assert_eq!(before.len(), 1);
    let counts_before = core.security.keyring.counts();

    move_to(&mut core, top.id, work, None).unwrap();

    for page in [top.id, child.id] {
        assert_eq!(
            doc_spaces(&core, page),
            vec![personal.min(work), personal.max(work)]
        );
        let records = core.security.keyring.doc_records(DocId::for_page(page));
        assert_eq!(records.len(), 2);
        assert!(
            records.iter().any(|r| r.space_id == personal),
            "the source record stays"
        );
        assert!(
            records.iter().any(|r| r.space_id == work),
            "a new record under the target"
        );
        assert_ne!(
            records[0].wrapped_key.to_bytes(),
            records[1].wrapped_key.to_bytes(),
            "a fresh key, not the old one wrapped again"
        );
        assert_eq!(
            core.db
                .doc_get(DocId::for_page(page))
                .unwrap()
                .unwrap()
                .space_id,
            Some(work)
        );
    }
    let source_record = core
        .security
        .keyring
        .doc_records(DocId::for_page(top.id))
        .into_iter()
        .find(|r| r.space_id == personal)
        .cloned()
        .unwrap();
    assert_eq!(source_record, before[0], "the source record is untouched");
    assert_eq!(
        core.security.keyring.counts().documents,
        counts_before.documents + 2,
        "one new record per moved document that exists"
    );
    // The unopened page's document is made on first use, under the space it is in.
    assert!(
        core.db
            .doc_get(DocId::for_page(no_doc.id))
            .unwrap()
            .is_none()
    );
    // The rows are in the database too (a reopen sees the same keyring).
    let stored = kb_storage::SecurityQueries::keyring_by_space(&core.db, work).unwrap();
    assert!(stored.iter().any(|row| {
        row.subject
            == KeySubject::Doc {
                doc_id: DocId::for_page(top.id),
                epoch: 0,
            }
            .encode()
    }));
}

#[test]
fn a_device_never_creates_the_target_record_of_a_page_another_device_moved() {
    let (_fx, mut core, personal, work) = world();
    kit_create(&mut core);
    let top = create_page(&mut core, personal, None, "Top");
    let doc = DocId::for_page(top.id);
    move_to(&mut core, top.id, work, None).unwrap();

    // What a peer that has not got the moving device's record yet sees: the source record only.
    let full = core.security.keyring.clone();
    let mut peer_view = kb_keys::Keyring::new();
    for item in full.items() {
        let keep = match &item {
            kb_keys::KeyringItem::Record(r) => {
                !(r.space_id == work && matches!(r.subject, KeySubject::Doc { .. }))
            }
            kb_keys::KeyringItem::Tombstone(_) => true,
        };
        if keep {
            peer_view.merge(item);
        }
    }
    core.security.keyring = peer_view;
    assert_eq!(core.security.keyring.doc_spaces(doc), vec![personal]);

    // Nothing creates it: not the maintenance at open, not the page code.
    core.ensure_doc_key(work, doc).unwrap();
    core.keyring_maintain().unwrap();
    assert!(core.doc_key_item(work, doc).unwrap().is_none());
    assert_eq!(
        core.security.keyring.doc_spaces(doc),
        vec![personal],
        "still only the source's record"
    );
    // A snapshot of the page is sealed under the source record until the target's arrives.
    let (bytes, _) = make_snapshot(&mut core, doc);
    let header = kb_sync::v2::peek_snapshot(&bytes).unwrap();
    assert_eq!(header.doc_id, doc);
    assert_eq!(core.security.keyring.doc_spaces(doc), vec![personal]);

    // The moving device's record arrives: from then on it seals under that one.
    for item in full.items() {
        core.security.keyring.merge(item);
    }
    assert_eq!(core.security.keyring.doc_spaces(doc).len(), 2);
    let (bytes, _) = make_snapshot(&mut core, doc);
    assert_eq!(kb_sync::v2::peek_snapshot(&bytes).unwrap().doc_id, doc);
}

#[test]
fn a_page_can_move_back_and_forth() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");
    write_text(&mut core, child.id, "hello zebra", 1);
    let pic = upload(&mut core, personal, "a.png", b"bytes");
    append_blocks(
        &mut core,
        top.id,
        json!([image_block(1, pic.attachment_id)]),
    );
    let content = page_content(&mut core, child.id);

    move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(outline(&tree(&mut core, work)), "Top\n  Child\n");
    move_to(&mut core, top.id, personal, None).unwrap();
    core.drain_events();

    assert_eq!(outline(&tree(&mut core, personal)), "Top\n  Child\n");
    assert_eq!(outline(&tree(&mut core, work)), "");
    assert_eq!(page(&mut core, child.id).space_id, personal);
    assert_eq!(page_content(&mut core, child.id), content);
    assert_eq!(
        search(&mut core, "zebra", Some(personal)),
        vec![(child.id, personal)]
    );
    assert_eq!(attachments_of(&mut core, personal), vec![pic.attachment_id]);
    assert!(attachments_of(&mut core, work).is_empty());
    assert_eq!(
        read_attachment(&mut core, pic.attachment_id).unwrap(),
        b"bytes"
    );
    // The target of the first move holds residue now, the source none of the pages (the residue
    // of the first move was removed when the pages came back).
    assert!(core.docs.structures[&personal].model.moved_out().is_empty());
    assert_eq!(core.docs.structures[&work].model.moved_out().len(), 2);
    for page in [top.id, child.id] {
        assert_eq!(
            core.security
                .keyring
                .doc_records(DocId::for_page(page))
                .len(),
            2,
            "the first space's record was reused, not duplicated"
        );
        assert_eq!(
            core.db
                .doc_get(DocId::for_page(page))
                .unwrap()
                .unwrap()
                .space_id,
            Some(personal)
        );
    }
    // And forth again.
    move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(outline(&tree(&mut core, work)), "Top\n  Child\n");
    assert_eq!(outline(&tree(&mut core, personal)), "");
    assert_eq!(attachments_of(&mut core, work), vec![pic.attachment_id]);
    assert!(
        core.docs.structures[&personal]
            .model
            .check_invariants()
            .is_empty()
    );
    assert!(
        core.docs.structures[&work]
            .model
            .check_invariants()
            .is_empty()
    );
}

#[test]
fn a_purge_after_a_move_tombstones_the_document_in_every_space() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");
    let doc = DocId::for_page(child.id);
    write_text(&mut core, child.id, "secret zebra", 1);
    move_to(&mut core, top.id, work, None).unwrap();

    call(&mut core, Request::PageTrash { page_id: top.id });
    call(&mut core, Request::PagePurge { page_id: top.id });
    for space in [personal, work] {
        assert!(
            core.security.keyring.is_tombstoned(
                space,
                &KeySubject::Doc {
                    doc_id: doc,
                    epoch: 0
                }
            ),
            "tombstone in {space}"
        );
    }
    assert!(core.security.keyring.doc_spaces(doc).is_empty());
    assert_eq!(search(&mut core, "zebra", None), vec![]);
    // The residue of the purged pages in the source is not a purge's business: still there, and
    // nothing resurrects a key for them.
    core.ensure_doc_key(personal, doc).unwrap();
    core.ensure_doc_key(work, doc).unwrap();
    assert!(core.security.keyring.doc_spaces(doc).is_empty());
}

#[test]
fn collecting_page_documents_keeps_the_pages_that_are_in_transit() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let gone = create_page(&mut core, personal, None, "Purged");
    let pic_doc = DocId::for_page(top.id);
    write_text(&mut core, top.id, "kept", 1);
    move_to(&mut core, top.id, work, None).unwrap();
    call(&mut core, Request::PageTrash { page_id: gone.id });
    call(&mut core, Request::PagePurge { page_id: gone.id });
    // The page is in transit when the target has not arrived: no index row, residue in the source.
    core.db.pages_index_delete(&[top.id]).unwrap();
    assert!(core.db.pages_index_get(top.id).unwrap().is_none());
    assert!(core.db.doc_get(DocId::for_page(gone.id)).unwrap().is_some());

    Core::gc_page_documents(&mut core.db, &core.docs).unwrap();
    assert!(
        core.db.doc_get(pic_doc).unwrap().is_some(),
        "a page that is residue in a structure keeps its document"
    );
    assert!(
        core.db.doc_get(DocId::for_page(gone.id)).unwrap().is_none(),
        "a purged page's document goes"
    );
}

#[test]
fn deleting_a_space_whose_pages_moved_out_is_allowed() {
    let (_fx, mut core, personal, work) = world();
    kit_create(&mut core);
    let top = create_page(&mut core, personal, None, "Top");
    write_text(&mut core, top.id, "still here zebra", 1);
    let doc = DocId::for_page(top.id);
    move_to(&mut core, top.id, work, None).unwrap();
    assert_eq!(outline(&tree(&mut core, personal)), "");

    call(&mut core, Request::SpaceDelete { space_id: personal });
    assert!(spaces(&mut core).iter().all(|s| s.id != personal));
    assert_eq!(
        doc_spaces(&core, top.id),
        vec![work],
        "its records went with the space"
    );
    assert_eq!(page(&mut core, top.id).space_id, work);
    assert_eq!(search(&mut core, "zebra", None), vec![(top.id, work)]);
    // The moved document can still be snapshotted under its new key.
    let (bytes, _) = make_snapshot(&mut core, doc);
    assert_eq!(kb_sync::v2::peek_snapshot(&bytes).unwrap().doc_id, doc);
}

#[test]
fn a_move_survives_a_reopen_and_stays_consistent() {
    let (fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");
    write_text(&mut core, child.id, "zebra", 1);
    let owner = prop_create(&mut core, personal, "Owner", PropertyKind::Text);
    set_value(
        &mut core,
        child.id,
        owner.id,
        PropertyValue::Text("Ben".into()),
    );
    let pic = upload(&mut core, personal, "a.png", b"bytes");
    append_blocks(
        &mut core,
        top.id,
        json!([image_block(1, pic.attachment_id)]),
    );
    move_to(&mut core, top.id, work, None).unwrap();
    let before = fingerprint(&mut core);
    drop(core);

    let mut core = fx.open(2, 9_000);
    assert_eq!(
        fingerprint(&mut core),
        before,
        "nothing was repaired or rewritten at open"
    );
    assert_eq!(outline(&tree(&mut core, work)), "Top\n  Child\n");
    assert_eq!(outline(&tree(&mut core, personal)), "");
    assert_eq!(
        search(&mut core, "zebra", Some(work)),
        vec![(child.id, work)]
    );
    assert_eq!(
        read_attachment(&mut core, pic.attachment_id).unwrap(),
        b"bytes"
    );
    assert!(
        core.db
            .doc_get(DocId::for_page(child.id))
            .unwrap()
            .is_some()
    );
    let owner_in_work = schema(&mut core, work)
        .into_iter()
        .find(|p| p.name == "Owner")
        .unwrap();
    assert_eq!(
        value_of(&mut core, child.id, owner_in_work.id),
        Some(PropertyValue::Text("Ben".into()))
    );
}

#[test]
fn a_move_interrupted_after_the_target_was_written_is_settled_at_open() {
    // The target structure is persisted first: a crash before the source is leaves the pages live
    // in both spaces. The next open evicts the older copy (the source's), like a concurrent move.
    // The state of that crash is the pages live in both structures: built by hand, then settled.
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let child = create_page(&mut core, personal, Some(top.id), "Child");
    write_text(&mut core, child.id, "zebra", 1);
    let pages = core.docs.structures[&personal]
        .model
        .moved_pages(top.id)
        .unwrap();
    // Target only (as if the source write never happened).
    core.structure_mut(work)
        .unwrap()
        .model
        .move_in(&pages, None, None, personal, 50_000)
        .unwrap();
    let ids = [top.id, child.id];
    core.persist_structure_with(
        work,
        Some(&ids),
        UpdateOrigin::Local,
        crate::properties::PropsScope::Pages(&ids),
    )
    .unwrap();
    assert!(core.docs.structures[&personal].model.page(top.id).is_some());
    assert!(core.docs.structures[&work].model.page(top.id).is_some());

    core.reconcile_moves_at_open().unwrap();
    assert!(
        core.docs.structures[&personal].model.page(top.id).is_none(),
        "the source's copy became residue"
    );
    assert_eq!(outline(&tree(&mut core, work)), "Top\n  Child\n");
    assert_eq!(outline(&tree(&mut core, personal)), "");
    assert_eq!(page(&mut core, child.id).space_id, work);
    let events = core.drain_events();
    assert!(
        events.contains(&Event::PagesMovedSpace {
            from_space_id: personal,
            to_space_id: work,
            page_ids: vec![top.id, child.id],
        }),
        "{events:?}"
    );
    // Settled means settled: a second pass finds nothing to do.
    let before = fingerprint(&mut core);
    core.reconcile_moves_at_open().unwrap();
    assert_eq!(fingerprint(&mut core), before);
}

// ----- two devices -----------------------------------------------------------------------------

/// One simulated device: a [`Dev`] of the v2 simulator (its own installation, keystore, clock and
/// connection) with the helpers of these scenarios.
struct Device {
    dev: Dev,
    /// The recovery kit words of the account (every device is restored with them).
    kit: String,
}

impl std::ops::Deref for Device {
    type Target = Dev;
    fn deref(&self) -> &Dev {
        &self.dev
    }
}

impl std::ops::DerefMut for Device {
    fn deref_mut(&mut self) -> &mut Dev {
        &mut self.dev
    }
}

impl Device {
    fn first(name: &'static str, seed: u64, server: &MemoryRemote) -> (Self, String) {
        let (dev, kit) = Dev::first(name, seed, server);
        (
            Self {
                dev,
                kit: kit.clone(),
            },
            kit,
        )
    }

    fn from_backup(
        name: &'static str,
        seed: u64,
        server: &MemoryRemote,
        source: &mut Device,
        kit: &str,
    ) -> Self {
        Self {
            dev: Dev::from_backup(name, seed, server, &mut source.dev, kit),
            kit: kit.to_owned(),
        }
    }

    /// One driver cycle, in the driver's order.
    fn cycle(&mut self) -> Result<(), String> {
        self.dev.cycle().map_err(|e| format!("{e:?}"))
    }

    /// The account's personal feed.
    fn personal_feed(&self) -> FeedId {
        FeedId::from_account(self.dev.account())
    }

    /// One page of the personal feed from the core's cursor (at most `limit` items), handed to the
    /// core by hand.
    fn pull_once(&mut self, limit: u32) -> kb_sync::v2::SyncApplied {
        let feed = self.personal_feed();
        let cursor = self.core.sync_info().unwrap().remote_cursor;
        let page = self.remote.changes_sync(feed, cursor, limit).unwrap();
        self.core.sync_feed_apply(feed, &page).unwrap()
    }

    /// Publishes a snapshot of one document by hand.
    fn publish_snapshot(&mut self, doc: DocId) {
        let feed = self.personal_feed();
        let (bytes, covers) = self.core.sync_feed_make_snapshot(feed, doc).unwrap();
        self.remote.put_snapshot_sync(feed, &bytes).unwrap();
        self.core
            .sync_feed_snapshot_published(feed, doc, covers)
            .unwrap();
    }

    fn next_edit(&mut self) -> u64 {
        self.edits += 1;
        self.edits
    }

    fn space_ids(&mut self) -> Vec<SpaceId> {
        spaces(&mut self.core).into_iter().map(|s| s.id).collect()
    }

    fn new_page(&mut self, space_id: SpaceId, parent: Option<PageId>, title: &str) -> PageId {
        let page = create_page(&mut self.core, space_id, parent, title);
        self.core.drain_events();
        page.id
    }

    fn write(&mut self, page_id: PageId, text: &str) {
        let n = self.next_edit();
        write_text(&mut self.core, page_id, text, n);
        self.core.drain_events();
    }

    fn rename(&mut self, page_id: PageId, title: &str) {
        self.core
            .dispatch(Request::PageRename {
                page_id,
                title: title.into(),
            })
            .unwrap();
        self.core.drain_events();
    }

    fn move_to(&mut self, page_id: PageId, space_id: SpaceId) -> SpaceMoveSummary {
        let summary = move_to(&mut self.core, page_id, space_id, None).unwrap();
        self.core.drain_events();
        summary
    }

    fn text(&mut self, page_id: PageId) -> String {
        page_content(&mut self.core, page_id).to_string()
    }

    fn where_is(&mut self, page_id: PageId) -> SpaceId {
        page(&mut self.core, page_id).space_id
    }

    fn outline_of(&mut self, space_id: SpaceId) -> String {
        outline(&tree(&mut self.core, space_id))
    }

    /// The residue of a space: page -> space it went to.
    fn residue(&self, space_id: SpaceId) -> BTreeMap<PageId, SpaceId> {
        self.core.docs.structures[&space_id]
            .model
            .moved_out()
            .into_iter()
            .map(|(page, to, _)| (page, to))
            .collect()
    }
}

/// Everything the convergence check compares.
#[derive(Debug, PartialEq)]
struct DeviceState {
    spaces: BTreeMap<SpaceId, SpaceState>,
}

#[derive(Debug, PartialEq)]
struct SpaceState {
    outline: String,
    structure: Value,
    residue: BTreeMap<PageId, SpaceId>,
    pages: BTreeMap<PageId, Value>,
    attachments: Vec<AttachmentId>,
}

impl Device {
    fn state(&mut self) -> DeviceState {
        let mut out = BTreeMap::new();
        for space_id in self.space_ids() {
            let model = &self.core.docs.structures[&space_id].model;
            let structure = model.to_portable();
            let ids = model.all_page_ids();
            let outline = outline(&tree(&mut self.core, space_id));
            let residue = self.residue(space_id);
            let mut pages = BTreeMap::new();
            for page_id in ids {
                let has_doc = self
                    .core
                    .db
                    .doc_get(DocId::for_page(page_id))
                    .unwrap()
                    .is_some();
                if has_doc {
                    pages.insert(page_id, page_content(&mut self.core, page_id));
                }
            }
            let attachments = attachments_of(&mut self.core, space_id);
            out.insert(
                space_id,
                SpaceState {
                    outline,
                    structure,
                    residue,
                    pages,
                    attachments,
                },
            );
        }
        DeviceState { spaces: out }
    }
}

/// Runs full cycles until nothing moves any more, then checks every device agrees.
fn settle(devices: &mut [&mut Device]) {
    for _ in 0..4 {
        for device in devices.iter_mut() {
            device
                .cycle()
                .unwrap_or_else(|e| panic!("{} cycle: {e}", device.name));
        }
    }
    assert_converged(devices);
}

fn assert_converged(devices: &mut [&mut Device]) {
    let states: Vec<(&str, DeviceState)> =
        devices.iter_mut().map(|d| (d.name, d.state())).collect();
    for pair in states.windows(2) {
        let (a_name, a) = &pair[0];
        let (b_name, b) = &pair[1];
        assert_eq!(
            a.spaces.keys().collect::<Vec<_>>(),
            b.spaces.keys().collect::<Vec<_>>(),
            "{a_name} and {b_name} list different spaces"
        );
        for (space, sa) in &a.spaces {
            let sb = &b.spaces[space];
            assert_eq!(sa.outline, sb.outline, "{a_name}/{b_name} tree of {space}");
            assert_eq!(
                sa.residue, sb.residue,
                "{a_name}/{b_name} residue of {space}"
            );
            assert_eq!(
                sa.structure, sb.structure,
                "{a_name}/{b_name} structure of {space}"
            );
            assert_eq!(
                sa.pages.keys().collect::<Vec<_>>(),
                sb.pages.keys().collect::<Vec<_>>(),
                "{a_name}/{b_name} pages with documents in {space}"
            );
            for (page, ca) in &sa.pages {
                assert_eq!(
                    Some(ca),
                    sb.pages.get(page),
                    "{a_name}/{b_name} content of {page}"
                );
            }
            assert_eq!(
                sa.attachments, sb.attachments,
                "{a_name}/{b_name} attachments of {space}"
            );
        }
    }
}

/// Two devices of one account with a second space, everything synced.
struct Pair {
    a: Device,
    b: Device,
    personal: SpaceId,
    work: SpaceId,
}

fn pair(seed: u64) -> Pair {
    let server = MemoryRemote::new(&SeededRng::new(seed));
    let (mut a, kit) = Device::first("a", 11, &server);
    let personal = a.space_ids()[0];
    let work = create_space(&mut a.core, "Work").id;
    a.core.drain_events();
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 22, &server, &mut a, &kit);
    b.configure();
    b.cycle().unwrap();
    a.cycle().unwrap();
    Pair {
        a,
        b,
        personal,
        work,
    }
}

/// The order the structure documents reach the feed in (oldest document first): a move from the
/// first space to the second sends the source before the target, the other way round the target
/// before the source.
fn feed_order(core: &Core, spaces: [SpaceId; 2]) -> [SpaceId; 2] {
    let order: Vec<SpaceId> = kb_storage::SecurityQueries::docs_all(&core.db)
        .unwrap()
        .into_iter()
        .filter_map(|row| row.space_id.filter(|s| spaces.contains(s)))
        .filter(|s| core.docs.structures.contains_key(s))
        .fold(Vec::new(), |mut seen, space| {
            if !seen.contains(&space) {
                seen.push(space);
            }
            seen
        });
    [order[0], order[1]]
}

fn a_move_reaches_the_other_device(source_first: bool) {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(if source_first { 1 } else { 2 });
    let [first, second] = feed_order(&a.core, [personal, work]);
    let (source, target) = if source_first {
        (first, second)
    } else {
        (second, first)
    };
    let top = a.new_page(source, None, "Roadmap");
    let child = a.new_page(source, Some(top), "Quarter");
    let stays = a.new_page(source, None, "Stays");
    a.write(top, "plan zebra");
    a.write(child, "milestones giraffe");
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.where_is(top), source);

    a.move_to(top, target);
    b.core.drain_events();
    b.log.clear();
    a.cycle().unwrap();
    b.cycle().unwrap();
    // The simulator's cycle drains the core's events into its log at its end.
    let events = b.log.clone();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::PagesMovedSpace { from_space_id, to_space_id, page_ids }
                if *from_space_id == source && *to_space_id == target && page_ids.first() == Some(&top)
        )),
        "b is told about the move: {events:?}"
    );
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        assert_eq!(device.where_is(top), target, "{}", device.name);
        assert_eq!(device.where_is(child), target);
        assert_eq!(device.where_is(stays), source);
        assert_eq!(device.outline_of(target), "Roadmap\n  Quarter\n");
        assert_eq!(device.outline_of(source), "Stays\n");
        assert_eq!(
            device
                .residue(source)
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([top, child])
        );
        assert!(device.text(top).contains("plan zebra"));
        assert!(device.text(child).contains("milestones giraffe"));
        assert_eq!(search(&mut device.core, "zebra", Some(source)), vec![]);
        assert_eq!(
            search(&mut device.core, "zebra", Some(target)),
            vec![(top, target)]
        );
        assert_eq!(
            device
                .core
                .db
                .doc_get(DocId::for_page(top))
                .unwrap()
                .unwrap()
                .space_id,
            Some(target),
            "{}: the document row follows",
            device.name
        );
        assert_eq!(doc_spaces(&device.core, top).len(), 2, "{}", device.name);
        assert!(
            device.core.docs.structures[&source]
                .model
                .check_invariants()
                .is_empty()
        );
        assert!(
            device.core.docs.structures[&target]
                .model
                .check_invariants()
                .is_empty()
        );
    }
    // Nobody but the moving device wrote the target's record.
    let a_records: Vec<_> = a
        .core
        .security
        .keyring
        .doc_records(DocId::for_page(top))
        .into_iter()
        .cloned()
        .collect();
    let b_records: Vec<_> = b
        .core
        .security
        .keyring
        .doc_records(DocId::for_page(top))
        .into_iter()
        .cloned()
        .collect();
    assert_eq!(a_records, b_records);
    let a_device = a.core.device.device_id;
    for record in &a_records {
        assert_eq!(record.created_by, a_device, "{record:?}");
    }

    // And edits keep flowing in both directions after the move.
    b.write(top, "from b after zebra");
    a.write(child, "from a after giraffe");
    settle(&mut [&mut a, &mut b]);
    assert!(a.text(top).contains("from b after"));
    assert!(b.text(child).contains("from a after"));
}

#[test]
fn a_move_applies_on_the_other_device_when_the_source_structure_arrives_first() {
    a_move_reaches_the_other_device(true);
}

#[test]
fn a_move_applies_on_the_other_device_when_the_target_structure_arrives_first() {
    a_move_reaches_the_other_device(false);
}

#[test]
fn content_edits_made_offline_to_a_page_that_moved_are_kept() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(3);
    let top = a.new_page(personal, None, "Roadmap");
    let child = a.new_page(personal, Some(top), "Quarter");
    a.write(top, "base zebra");
    settle(&mut [&mut a, &mut b]);

    // b goes offline and edits the content of both pages (sealed under the source space's key);
    // a moves them meanwhile.
    b.write(top, "b offline top");
    b.write(child, "b offline child");
    a.write(child, "a online child");
    a.move_to(top, work);
    a.cycle().unwrap();
    // b's cycle pushes its entries first (the key it holds is the source's), then pulls the move.
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        assert_eq!(device.where_is(top), work, "{}", device.name);
        assert_eq!(device.outline_of(work), "Roadmap\n  Quarter\n");
        let top_text = device.text(top);
        assert!(
            top_text.contains("base zebra") && top_text.contains("b offline top"),
            "{top_text}"
        );
        let child_text = device.text(child);
        assert!(
            child_text.contains("b offline child") && child_text.contains("a online child"),
            "{child_text}"
        );
        assert_eq!(
            search(&mut device.core, "offline", Some(work))
                .into_iter()
                .map(|(page, _)| page)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([top, child])
        );
    }
}

#[test]
fn a_child_created_offline_under_a_page_that_moved_is_re_rooted_in_its_space() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(4);
    let top = a.new_page(personal, None, "Roadmap");
    let child = a.new_page(personal, Some(top), "Quarter");
    settle(&mut [&mut a, &mut b]);

    // b creates a page under the page that a moves at the same time.
    let born = b.new_page(personal, Some(top), "Born offline");
    b.write(born, "orphan zebra");
    a.move_to(top, work);
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        assert_eq!(
            device.outline_of(work),
            "Roadmap\n  Quarter\n",
            "{}",
            device.name
        );
        assert_eq!(
            device.outline_of(personal),
            "Born offline\n",
            "{}: the new page is live again at the root of its own space",
            device.name
        );
        assert_eq!(device.where_is(born), personal);
        assert_eq!(device.where_is(child), work);
        assert!(device.text(born).contains("orphan zebra"));
        assert_eq!(
            search(&mut device.core, "zebra", Some(personal)),
            vec![(born, personal)],
            "{}",
            device.name
        );
        assert!(
            device.core.docs.structures[&personal]
                .model
                .orphans()
                .is_empty()
        );
        assert!(
            device.core.docs.structures[&personal]
                .model
                .check_invariants()
                .is_empty()
        );
        // Its document was never collected or erased.
        assert!(
            device
                .core
                .db
                .doc_get(DocId::for_page(born))
                .unwrap()
                .is_some()
        );
        assert!(!device.core.security.keyring.is_tombstoned(
            personal,
            &KeySubject::Doc {
                doc_id: DocId::for_page(born),
                epoch: 0
            }
        ));
    }
}

#[test]
fn two_concurrent_moves_to_different_spaces_leave_one_winner_with_one_result() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(5);
    let archive = create_space(&mut a.core, "Archive").id;
    a.core.drain_events();
    settle(&mut [&mut a, &mut b]);
    let top = a.new_page(personal, None, "Roadmap");
    let child = a.new_page(personal, Some(top), "Quarter");
    a.write(top, "base zebra");
    settle(&mut [&mut a, &mut b]);

    // Both move the page at once, to different spaces. b's clock is later, so its arrival wins.
    a.move_to(top, work);
    b.move_to(top, archive);
    b.write(child, "b edit giraffe");
    a.write(child, "a edit giraffe");
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        assert_eq!(
            device.where_is(top),
            archive,
            "{}: the later arrival wins",
            device.name
        );
        assert_eq!(device.where_is(child), archive);
        assert_eq!(device.outline_of(archive), "Roadmap\n  Quarter\n");
        assert_eq!(device.outline_of(work), "", "the losing copy is gone");
        assert_eq!(device.outline_of(personal), "");
        assert_eq!(
            device.residue(work).get(&top),
            Some(&archive),
            "{}: the loser points at the winner",
            device.name
        );
        let content = device.text(child);
        assert!(
            content.contains("b edit giraffe") && content.contains("a edit giraffe"),
            "both edits merge: {content}"
        );
        assert_eq!(
            search(&mut device.core, "giraffe", None)
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(child, archive)]),
            "{}: hits only in the winning space",
            device.name
        );
        assert_eq!(
            kb_storage::DocQueries::pages_index_live_count(&device.core.db, archive).unwrap(),
            2
        );
        assert_eq!(
            kb_storage::DocQueries::pages_index_live_count(&device.core.db, work).unwrap(),
            0
        );
        assert!(
            device.core.docs.structures[&work]
                .model
                .check_invariants()
                .is_empty()
        );
        assert!(
            device.core.docs.structures[&archive]
                .model
                .check_invariants()
                .is_empty()
        );
    }
}

#[test]
fn a_page_moved_back_on_the_other_device_converges() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(6);
    let top = a.new_page(personal, None, "Roadmap");
    let child = a.new_page(personal, Some(top), "Quarter");
    a.write(child, "zebra");
    settle(&mut [&mut a, &mut b]);
    a.move_to(top, work);
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.where_is(top), work);

    b.move_to(top, personal);
    b.write(child, "back home");
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(
            device.outline_of(personal),
            "Roadmap\n  Quarter\n",
            "{}",
            device.name
        );
        assert_eq!(device.outline_of(work), "");
        assert_eq!(device.where_is(child), personal);
        assert!(device.text(child).contains("back home"));
        assert!(device.residue(personal).is_empty());
        assert_eq!(
            device
                .residue(work)
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([top, child])
        );
    }
    // And it can go again.
    a.move_to(top, work);
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.outline_of(work), "Roadmap\n  Quarter\n");
}

#[test]
fn a_purge_after_a_move_tombstones_every_space_on_every_device() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(7);
    let top = a.new_page(personal, None, "Roadmap");
    a.write(top, "secret zebra");
    settle(&mut [&mut a, &mut b]);
    a.move_to(top, work);
    settle(&mut [&mut a, &mut b]);

    b.core
        .dispatch(Request::PageTrash { page_id: top })
        .unwrap();
    b.core
        .dispatch(Request::PagePurge { page_id: top })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    let subject = KeySubject::Doc {
        doc_id: DocId::for_page(top),
        epoch: 0,
    };
    for device in [&mut a, &mut b] {
        for space in [personal, work] {
            assert!(
                device.core.security.keyring.is_tombstoned(space, &subject),
                "{}: tombstone in {space}",
                device.name
            );
        }
        assert!(
            device
                .core
                .security
                .keyring
                .doc_spaces(DocId::for_page(top))
                .is_empty()
        );
        assert_eq!(search(&mut device.core, "zebra", None), vec![]);
        assert_eq!(device.outline_of(work), "");
    }
}

#[test]
fn attachments_that_moved_are_readable_on_the_other_device() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(8);
    let top = a.new_page(personal, None, "Roadmap");
    let bytes = vec![3u8; CHUNK_SIZE + 500];
    let pic = upload(&mut a.core, personal, "a.png", &bytes);
    append_blocks(&mut a.core, top, json!([image_block(1, pic.attachment_id)]));
    settle(&mut [&mut a, &mut b]);
    assert_eq!(
        read_attachment(&mut b.core, pic.attachment_id).unwrap(),
        bytes
    );

    a.move_to(top, work);
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(
            attachments_of(&mut device.core, work),
            vec![pic.attachment_id],
            "{}",
            device.name
        );
        assert!(attachments_of(&mut device.core, personal).is_empty());
        assert_eq!(
            read_attachment(&mut device.core, pic.attachment_id).unwrap(),
            bytes,
            "{}",
            device.name
        );
        assert_eq!(
            device
                .core
                .db
                .attachment_get(pic.attachment_id)
                .unwrap()
                .unwrap()
                .space_id,
            work
        );
    }
}

#[test]
fn a_device_that_joins_after_a_move_bootstraps_from_snapshots() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(9);
    let top = a.new_page(personal, None, "Roadmap");
    let child = a.new_page(personal, Some(top), "Quarter");
    a.write(child, "zebra");
    settle(&mut [&mut a, &mut b]);
    a.move_to(top, work);
    a.write(child, "after the move");
    settle(&mut [&mut a, &mut b]);

    // A third device restored from a backup of `a` (taken after the move) and the account kit:
    // the moved documents come to it as snapshots under their new keys.
    let server = a.remote.clone();
    let kit = a.kit.clone();
    let mut c = Device::from_backup("c", 33, &server, &mut a, &kit);
    c.configure();
    settle(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(c.outline_of(work), "Roadmap\n  Quarter\n");
    assert!(c.text(child).contains("after the move"));
    assert_eq!(c.where_is(top), work);
}

#[test]
fn the_moved_documents_are_snapshotted_under_the_new_key() {
    let Pair {
        mut a,
        b: _b,
        personal,
        work,
    } = pair(10);
    let top = a.new_page(personal, None, "Roadmap");
    a.write(top, "zebra");
    a.cycle().unwrap();
    let doc = DocId::for_page(top);
    // After a cycle the document was snapshotted once (never snapshotted before).
    assert!(candidates(&mut a.core).is_empty());
    a.move_to(top, work);
    assert!(
        candidates(&mut a.core).contains(&doc),
        "the move marks the document for a snapshot"
    );
    let state = a.core.db.sync_doc_state_get(doc).unwrap().unwrap();
    assert!(
        state.remote_snapshot_covers.is_some(),
        "the thresholds, not the cursor, were touched"
    );
    a.cycle().unwrap();
    assert!(
        !candidates(&mut a.core).contains(&doc),
        "published, so no longer a candidate"
    );
}

#[test]
fn what_a_peer_cannot_open_yet_waits_for_the_key_instead_of_failing() {
    // A moved page's entries sealed under the target key reach a device whose keyring lacks that
    // record: the item parks (a missing key), it is not an error, and it applies once the record
    // is there.
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(11);
    let top = a.new_page(personal, None, "Roadmap");
    a.write(top, "base zebra");
    settle(&mut [&mut a, &mut b]);
    a.move_to(top, work);
    a.write(top, "after the move giraffe");
    a.cycle().unwrap();

    // b's keyring entry feed is hidden by dropping the target's record from b's view of the
    // keyring until its entries have been seen: emulate by removing the record from the keyring
    // right before the page entries are applied.
    let doc = DocId::for_page(top);
    let target_records: Vec<_> = a
        .core
        .security
        .keyring
        .doc_records(doc)
        .into_iter()
        .filter(|r| r.space_id == work)
        .cloned()
        .collect();
    assert_eq!(target_records.len(), 1);
    b.cycle().unwrap();
    assert!(b.text(top).contains("after the move giraffe"));
    // Take the record away again and replay: a copy of the entries from a fresh cursor is parked.
    let mut view = kb_keys::Keyring::new();
    for item in b.core.security.keyring.items() {
        let drop_it = matches!(&item, kb_keys::KeyringItem::Record(r) if r.space_id == work && matches!(r.subject, KeySubject::Doc { .. }));
        if !drop_it {
            view.merge(item);
        }
    }
    b.core.security.keyring = view;
    assert_eq!(b.core.security.keyring.doc_spaces(doc), vec![personal]);
    // Another entry from a, sealed under the target record, now cannot be opened by b.
    a.write(top, "second giraffe");
    a.cycle().unwrap();
    let report = b.pull_once(200);
    assert!(report.pending >= 1, "{report:?}");
    assert_eq!(report.rejected, 0);
    assert!(!b.text(top).contains("second giraffe"));
    // The record comes back and a snapshot of the document reaches b: the snapshot carries what
    // the parked entry held, so b is complete and nothing stays parked.
    for record in target_records {
        b.core
            .put_keyring_item(kb_keys::KeyringItem::Record(record))
            .unwrap();
    }
    b.core.drain_events();
    a.publish_snapshot(doc);
    settle(&mut [&mut a, &mut b]);
    assert!(b.text(top).contains("second giraffe"));
    assert_eq!(b.core.sync_info().unwrap().parked_items, 0);
}

#[test]
fn two_devices_that_make_the_same_move_offline_leave_one_copy() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(12);
    let top = a.new_page(personal, None, "Roadmap");
    let child = a.new_page(personal, Some(top), "Quarter");
    a.write(child, "zebra");
    settle(&mut [&mut a, &mut b]);

    // Both devices move the same page into the same space while offline from each other.
    a.move_to(top, work);
    b.move_to(top, work);
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        assert_eq!(
            device.outline_of(work),
            "Roadmap\n  Quarter\n",
            "{}: one copy",
            device.name
        );
        assert_eq!(device.outline_of(personal), "");
        assert_eq!(device.where_is(child), work);
        assert_eq!(
            kb_storage::DocQueries::pages_index_live_count(&device.core.db, work).unwrap(),
            2
        );
        assert_eq!(search(&mut device.core, "zebra", None), vec![(child, work)]);
        assert_eq!(
            device
                .residue(personal)
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([top, child])
        );
        assert!(
            device.core.docs.structures[&work]
                .model
                .check_invariants()
                .is_empty()
        );
        assert!(
            device.core.docs.structures[&personal]
                .model
                .check_invariants()
                .is_empty()
        );
    }
    // The page still takes edits from both sides.
    a.write(child, "from a giraffe");
    b.write(child, "from b giraffe");
    settle(&mut [&mut a, &mut b]);
    assert!(a.text(child).contains("from b giraffe"));
    assert!(b.text(child).contains("from a giraffe"));
}

#[test]
fn a_rename_made_offline_to_a_page_that_moves_is_lost_and_everything_else_converges() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(13);
    let top = a.new_page(personal, None, "Roadmap");
    let stays = a.new_page(personal, None, "Stays");
    settle(&mut [&mut a, &mut b]);

    // b renames the page that a moves, and a page that stays; only the second rename survives
    // (a title lands on the residue of a moved page, ADR-0038).
    b.rename(top, "Renamed by b");
    b.rename(stays, "Stays, renamed");
    a.move_to(top, work);
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        assert_eq!(device.outline_of(work), "Roadmap\n", "{}", device.name);
        assert_eq!(device.outline_of(personal), "Stays, renamed\n");
        assert_eq!(device.where_is(top), work);
    }
}

#[test]
fn a_move_can_be_placed_among_the_children_of_the_new_parent() {
    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Moved");
    let parent = create_page(&mut core, work, None, "Parent");
    create_page(&mut core, work, Some(parent.id), "First");
    create_page(&mut core, work, Some(parent.id), "Second");
    match call(
        &mut core,
        Request::PageMoveToSpace {
            page_id: top.id,
            space_id: work,
            parent_id: Some(parent.id),
            index: Some(1),
        },
    ) {
        Response::PagesMovedToSpace(_) => {}
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        outline(&tree(&mut core, work)),
        "Parent\n  First\n  Moved\n  Second\n"
    );
    // An index past the end is clamped.
    let last = create_page(&mut core, personal, None, "Last");
    match call(
        &mut core,
        Request::PageMoveToSpace {
            page_id: last.id,
            space_id: work,
            parent_id: Some(parent.id),
            index: Some(99),
        },
    ) {
        Response::PagesMovedToSpace(_) => {}
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        outline(&tree(&mut core, work)),
        "Parent\n  First\n  Moved\n  Second\n  Last\n"
    );
}

#[test]
fn a_page_in_transit_is_hidden_keeps_its_document_and_key_and_arrives_when_the_target_does() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(14);
    let [source, target] = feed_order(&a.core, [personal, work]);
    let top = a.new_page(source, None, "Roadmap");
    let child = a.new_page(source, Some(top), "Quarter");
    a.write(top, "plan zebra");
    a.write(child, "milestones giraffe");
    settle(&mut [&mut a, &mut b]);
    let doc = DocId::for_page(top);

    a.move_to(top, target);
    a.cycle().unwrap();
    b.core.drain_events();

    // b takes the feed one entry at a time and looks at every state in between.
    let mut in_transit_seen = false;
    for _ in 0..200 {
        let feed = b.personal_feed();
        let cursor = b.core.sync_info().unwrap().remote_cursor;
        let page = b.remote.changes_sync(feed, cursor, 1).unwrap();
        if page.changes.is_empty() {
            break;
        }
        let more = page.more;
        b.core.sync_feed_apply(feed, &page).unwrap();
        let in_source = b.core.docs.structures[&source].model.page(top).is_some();
        let in_target = b.core.docs.structures[&target].model.page(top).is_some();
        if !in_source && !in_target {
            in_transit_seen = true;
            // Residue says where it went; the page is nowhere else, so nothing shows it.
            assert_eq!(b.residue(source).get(&top), Some(&target));
            assert!(b.core.db.pages_index_get(top).unwrap().is_none());
            assert_eq!(
                b.core
                    .dispatch(Request::PageGet { page_id: top })
                    .unwrap_err()
                    .code,
                ErrorCode::NotFound
            );
            // Its document and its key records are untouched: not a purge.
            assert!(b.core.db.doc_get(doc).unwrap().is_some());
            assert!(!b.core.security.keyring.doc_spaces(doc).is_empty());
            for space in [personal, work] {
                assert!(!b.core.security.keyring.is_tombstoned(
                    space,
                    &KeySubject::Doc {
                        doc_id: doc,
                        epoch: 0
                    }
                ));
            }
            // A collection at this moment keeps it.
            Core::gc_page_documents(&mut b.dev.core.db, &b.dev.core.docs).unwrap();
            assert!(b.core.db.doc_get(doc).unwrap().is_some());
        }
        if !more {
            break;
        }
    }
    assert!(
        in_transit_seen,
        "the source structure arrived before the target's"
    );
    settle(&mut [&mut a, &mut b]);
    assert_eq!(b.where_is(top), target);
    assert!(b.text(top).contains("plan zebra"));
    assert_eq!(
        search(&mut b.core, "zebra", Some(target)),
        vec![(top, target)]
    );
}

#[test]
fn a_document_key_that_loses_a_race_is_replaced_by_a_snapshot_and_no_edit_is_lost() {
    let Pair {
        mut a,
        mut b,
        personal,
        work,
    } = pair(15);
    let top = a.new_page(personal, None, "Roadmap");
    a.write(top, "base zebra");
    settle(&mut [&mut a, &mut b]);
    let doc = DocId::for_page(top);
    let subject = KeySubject::Doc {
        doc_id: doc,
        epoch: 0,
    };

    // Both devices move the page into the same space while apart, so each makes its own key record
    // for it there; b (the later clock) also edits the page, sealing the edit under its own record.
    a.move_to(top, work);
    b.move_to(top, work);
    b.write(top, "b offline giraffe");
    let b_record = b
        .core
        .security
        .keyring
        .record(work, &subject)
        .cloned()
        .unwrap();
    assert_eq!(b_record.created_by, b.core.device.device_id);

    a.cycle().unwrap();
    // b pushes its edit (sealed under its own record) and then pulls a's record, which wins
    // (the earlier of the two): b's record is gone and its document is marked for a snapshot.
    b.cycle().unwrap();
    let a_record = a
        .core
        .security
        .keyring
        .record(work, &subject)
        .cloned()
        .unwrap();
    assert_eq!(a_record.created_by, a.core.device.device_id);
    assert_eq!(
        b.core.security.keyring.record(work, &subject),
        Some(&a_record),
        "the earlier record wins on b too"
    );
    assert!(
        candidates(&mut b.core).contains(&doc),
        "the document is marked for a snapshot under the winning record"
    );
    // The stored row is the winner as well: a reopen would load it.
    let stored = kb_storage::SecurityQueries::keyring_by_space(&b.core.db, work).unwrap();
    let row = stored
        .iter()
        .find(|row| row.subject == subject.encode())
        .unwrap();
    assert_eq!(
        row.record,
        kb_keys::KeyringItem::Record(a_record.clone()).encode()
    );

    // a cannot open b's edit (sealed under a record it does not have): it parks it.
    a.cycle().unwrap();
    assert!(
        a.core.sync_info().unwrap().parked_items >= 1,
        "the edit waits for a key a will never get"
    );
    assert!(!a.text(top).contains("b offline giraffe"));
    // b publishes the snapshot, a applies it and drops what the snapshot covers.
    b.cycle().unwrap();
    a.cycle().unwrap();
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        let text = device.text(top);
        assert!(
            text.contains("base zebra") && text.contains("b offline giraffe"),
            "{}: {text}",
            device.name
        );
        assert_eq!(
            device.core.sync_info().unwrap().parked_items,
            0,
            "{}: nothing left parked",
            device.name
        );
        assert_eq!(device.where_is(top), work);
        assert_eq!(
            device.core.security.keyring.record(work, &subject),
            Some(&a_record)
        );
        assert_eq!(
            search(&mut device.core, "giraffe", Some(work)),
            vec![(top, work)]
        );
    }
    // The page keeps taking edits from both sides under the one record.
    a.write(top, "a after zebra");
    b.write(top, "b after giraffe");
    settle(&mut [&mut a, &mut b]);
    assert!(a.text(top).contains("b after giraffe"));
    assert!(b.text(top).contains("a after zebra"));
}

#[test]
fn a_record_of_another_device_that_loses_does_not_replace_the_stored_row() {
    let (_fx, mut core, personal, work) = world();
    kit_create(&mut core);
    let top = create_page(&mut core, personal, None, "Top");
    move_to(&mut core, top.id, work, None).unwrap();
    let doc = DocId::for_page(top.id);
    let subject = KeySubject::Doc {
        doc_id: doc,
        epoch: 0,
    };
    let own = core
        .security
        .keyring
        .record(work, &subject)
        .cloned()
        .unwrap();
    // The move marked the document for a snapshot; start from a clean mark.
    let mut state = core.db.sync_doc_state_get(doc).unwrap().unwrap();
    state.entries_since_snapshot = 0;
    core.db.sync_doc_state_put(&state).unwrap();
    let marked = |core: &Core| {
        core.db
            .sync_doc_state_get(doc)
            .unwrap()
            .unwrap()
            .entries_since_snapshot
            >= kb_types::sync::SNAPSHOT_AFTER_ENTRIES
    };
    // A later record for the same subject made by "another device" (any signature will do for the
    // merge rule): the earlier one, this device's, stays in memory and in the database.
    let mut later = own.clone();
    later.created_at += 10;
    later.created_by = kb_types::DeviceId::from_bytes([9; 16]);
    assert!(
        !core
            .merge_keyring_item(kb_keys::KeyringItem::Record(later))
            .unwrap()
    );
    assert_eq!(core.security.keyring.record(work, &subject), Some(&own));
    assert!(!marked(&core), "a record that loses marks nothing");
    // An earlier one replaces it, and the document is marked for a snapshot.
    let mut earlier = own.clone();
    earlier.created_at -= 10;
    earlier.created_by = kb_types::DeviceId::from_bytes([8; 16]);
    assert!(
        core.merge_keyring_item(kb_keys::KeyringItem::Record(earlier.clone()))
            .unwrap()
    );
    assert_eq!(core.security.keyring.record(work, &subject), Some(&earlier));
    assert!(
        marked(&core),
        "the replaced record marks the document for a snapshot"
    );
    assert!(candidates(&mut core).contains(&doc));
}

// ----- formulas and dashboards (ADR-0055) ---------------------------------------------------------

#[test]
fn the_formulas_a_view_names_and_the_widgets_of_a_dashboard_follow_a_moved_page() {
    use kb_types::{FormulaBase, FormulaResult, FormulaType, PropertyConfig};

    let (_fx, mut core, personal, work) = world();
    let top = create_page(&mut core, personal, None, "Top");
    let card = create_page(&mut core, personal, Some(top.id), "Card");
    let priority = prop_create(&mut core, personal, "Priority", PropertyKind::Number);
    let unused = prop_create(&mut core, personal, "Unused", PropertyKind::Number);
    let formula = |core: &mut Core, name: &str, expression: String| match call(
        core,
        Request::PropertyCreate {
            space_id: personal,
            name: name.into(),
            kind: PropertyKind::Formula,
            config: Some(PropertyConfig::Formula {
                expression,
                result_type: None,
            }),
        },
    ) {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    };
    let score = formula(
        &mut core,
        "Score",
        format!("prop(\"{}\") * 10", priority.id),
    );
    let total = formula(
        &mut core,
        "Total",
        format!("prop(\"{}\") + length(prop(\"title\"))", score.id),
    );
    set_value(&mut core, card.id, priority.id, PropertyValue::Number(3.0));
    let (score_id, total_id, priority_id, unused_id) = (
        score.id.to_string(),
        total.id.to_string(),
        priority.id.to_string(),
        unused.id.to_string(),
    );
    append_blocks(
        &mut core,
        top.id,
        json!([{ "type": "pageList", "attrs": {
            "id": BlockId::from_random_bytes([5; 16]).to_string(),
            "source": { "kind": "children" },
            "layout": "table",
            "properties": [score_id],
            "views": [
                { "id": "t", "name": "", "layout": "table", "properties": [score_id] },
                { "id": "d", "name": "Overview", "layout": "dashboard", "properties": [],
                  "widgets": [
                    { "id": "w0000001", "kind": "number", "title": "", "w": 1, "h": "s",
                      "calc": { "by": total_id, "fn": "sum" } },
                    { "id": "w0000002", "kind": "chart", "title": "", "w": 2, "h": "m",
                      "chart": { "type": "bar", "groupBy": priority_id,
                                 "measure": { "fn": "sum", "propertyId": score_id } },
                      "filter": { "op": "and", "items": [{ "propertyId": priority_id, "op": "isSet" }] } },
                    { "id": "w0000003", "kind": "number", "title": "", "w": 1, "h": "s",
                      "calc": { "by": unused_id, "fn": "sum" } }
                  ] }
            ]
        }}]),
    );
    core.drain_events();

    // Priority has a value, Score and Total are named by a view and read what is not made yet, Unused is
    // named by a widget only (like any property a view names, it is made without a value): four.
    let summary = plan(&mut core, top.id, work, None).unwrap();
    assert_eq!(summary.properties_created, 4);
    move_to(&mut core, top.id, work, None).unwrap();

    let work_schema = schema(&mut core, work);
    let names: Vec<&str> = work_schema.iter().map(|def| def.name.as_str()).collect();
    // The others in the order of the source's schema, then each formula after the ones it reads.
    assert_eq!(names, ["Tags", "Priority", "Unused", "Score", "Total"]);
    let target = |name: &str| work_schema.iter().find(|def| def.name == name).unwrap();
    let number = Some(FormulaType {
        base: FormulaBase::Number,
        list: false,
    });
    assert_eq!(
        target("Score").config,
        Some(PropertyConfig::Formula {
            expression: format!("prop(\"{}\") * 10", target("Priority").id),
            result_type: number,
        })
    );
    assert_eq!(
        target("Total").config,
        Some(PropertyConfig::Formula {
            expression: format!("prop(\"{}\") + length(prop(\"title\"))", target("Score").id),
            result_type: number,
        })
    );
    // The moved page computes them in its new space.
    assert_eq!(
        value_of(&mut core, card.id, target("Score").id),
        Some(PropertyValue::Formula(FormulaResult::Number(30.0)))
    );
    assert_eq!(
        value_of(&mut core, card.id, target("Total").id),
        Some(PropertyValue::Formula(FormulaResult::Number(34.0)))
    );

    // The view and the widgets name the target's properties.
    let lists = core.open_page(top.id).unwrap().model.page_lists();
    let attrs = &lists[0].1;
    let (new_score, new_total, new_priority) = (
        target("Score").id.to_string(),
        target("Total").id.to_string(),
        target("Priority").id.to_string(),
    );
    assert_eq!(attrs["properties"], json!([new_score]));
    let views = attrs["views"].as_array().unwrap();
    assert_eq!(views[0]["properties"], json!([new_score]));
    let widgets = views[1]["widgets"].as_array().unwrap();
    assert_eq!(widgets[0]["calc"], json!({ "by": new_total, "fn": "sum" }));
    assert_eq!(
        widgets[1]["chart"],
        json!({ "type": "bar", "groupBy": new_priority,
                "measure": { "fn": "sum", "propertyId": new_score } })
    );
    assert_eq!(
        widgets[1]["filter"],
        json!({ "op": "and", "items": [{ "propertyId": new_priority, "op": "isSet" }] })
    );
    assert_eq!(
        widgets[2]["calc"],
        json!({ "by": target("Unused").id.to_string(), "fn": "sum" })
    );
    assert_eq!(
        (widgets[0]["id"].as_str(), views[1]["layout"].as_str()),
        (Some("w0000001"), Some("dashboard"))
    );
}
