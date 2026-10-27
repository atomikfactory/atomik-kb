//! Templates (ADR-0056) through `Request`: `PageSetTemplate` and `PageSetTemplatePlaceholder` (the
//! shapes, the refusals, the events, persistence, undo), the page index column that keeps templates
//! out of search, backlinks, reminders, the semantic index and MCP's lists while the counts stay as
//! they are, the rows of a subtree that moves into or out of a template, the rebuild of the index,
//! and the flag and the placeholders through export, import and a move to another space. The roles
//! are in `tests_sharing/rules.rs`; the model's own cases are in `kb-model`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::{AiIndexQueries, CommentQueries, DateMentionQueries, DocQueries};
use kb_types::DateValue;
use kb_types::ai::{
    AI_SETTINGS_SETTING, AiChunkVector, AiIndexInfo, AiIndexWork, AiScope, AiSource, McpGrant,
};
use kb_types::{
    ApiError, Backlink, BlockId, ChangeSource, ErrorCode, Event, ExportFormat, PageId, PageInfo,
    PropertyId, PropertyKind, PropertyValue, Request, Response, SearchHit, SpaceId,
    TemplatePlaceholder,
};
use serde_json::{Value, json};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, page, spaces, tree, write_text,
};
use crate::test_support_interop::{
    another_core, as_source, export_all, prop_create, property_named, run_import, schema,
    set_value, values_of,
};

const T0: i64 = 5_000;
/// 2026-10-07 14:30 on the wall clock, in seconds since 1970-01-01T00:00:00.
const OCT_7_1430: i64 = 1_791_383_400;

fn setup() -> (Fixture, Core, SpaceId) {
    let fixture = Fixture::new();
    let mut core = fixture.open(1, T0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    core.drain_events();
    (fixture, core, space)
}

fn set_template(core: &mut Core, page_id: PageId, template: bool) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageSetTemplate { page_id, template })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn set_placeholder(
    core: &mut Core,
    page_id: PageId,
    property_id: PropertyId,
    placeholder: Option<TemplatePlaceholder>,
) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageSetTemplatePlaceholder {
        page_id,
        property_id,
        placeholder,
    })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
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

fn field_of(err: &ApiError) -> Option<&str> {
    assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");
    err.details.as_ref()?.get("field")?.as_str()
}

/// Whether the page index says the page is in a template.
fn indexed_in_template(core: &Core, page_id: PageId) -> bool {
    core.db
        .pages_index_get(page_id)
        .unwrap()
        .expect("an indexed page")
        .in_template
}

fn templates_indexed(core: &Core, space_id: SpaceId) -> u32 {
    core.db.pages_index_template_count(space_id).unwrap()
}

fn page_count(core: &mut Core, space_id: SpaceId) -> u32 {
    spaces(core)
        .into_iter()
        .find(|space| space.id == space_id)
        .unwrap()
        .page_count
}

fn search(core: &mut Core, query: &str) -> Vec<SearchHit> {
    match core
        .dispatch(Request::Search {
            query: query.into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

fn found(core: &mut Core, query: &str) -> Vec<PageId> {
    let mut pages: Vec<PageId> = search(core, query)
        .into_iter()
        .map(|hit| hit.page_id)
        .collect();
    pages.sort();
    pages.dedup();
    pages
}

fn backlinks(core: &mut Core, page_id: PageId) -> Vec<Backlink> {
    match core.dispatch(Request::BacklinksGet { page_id }).unwrap() {
        Response::Backlinks(links) => links,
        other => panic!("unexpected {other:?}"),
    }
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
    core.persist_page(
        page_id,
        None,
        kb_storage::UpdateOrigin::Core,
        ChangeSource::Core,
    )
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
            .insert_block(last, kb_model::BlockKind::Paragraph, "See ", block)
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
    core.persist_page(
        page_id,
        None,
        kb_storage::UpdateOrigin::Local,
        ChangeSource::Editor,
    )
    .unwrap();
    core.page_doc_close(page_id).unwrap();
}

/// A paragraph with a date mention that reminds 30 minutes before 2026-10-07 14:30.
fn reminder_block(id: u8, text: &str) -> Value {
    json!({
        "type": "paragraph",
        "attrs": { "id": BlockId::from_random_bytes([id; 16]).to_string() },
        "content": [
            { "type": "text", "text": text },
            { "type": "dateMention", "attrs": {
                "id": BlockId::from_random_bytes([id.wrapping_add(50); 16]).to_string(),
                "date": "2026-10-07",
                "time": "14:30",
                "reminder": 30
            } }
        ]
    })
}

fn reminders(core: &mut Core) -> Vec<PageId> {
    match core
        .dispatch(Request::RemindersDue {
            until: OCT_7_1430 + 3_600,
        })
        .unwrap()
    {
        Response::Reminders(list) => list.into_iter().map(|due| due.page_id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

fn day(date: &str) -> PropertyValue {
    PropertyValue::Date(DateValue {
        date: date.into(),
        time: None,
    })
}

fn flatten(nodes: &[kb_types::PageNode], out: &mut Vec<PageInfo>) {
    for node in nodes {
        out.push(node.page.clone());
        flatten(&node.children, out);
    }
}

/// The pages of a space's tree by title.
fn by_title(core: &mut Core, space_id: SpaceId, title: &str) -> PageInfo {
    let view = tree(core, space_id);
    let mut all = Vec::new();
    flatten(&view.roots, &mut all);
    all.into_iter()
        .find(|info| info.title == title)
        .unwrap_or_else(|| panic!("no page titled {title}"))
}

fn schema_ids(core: &mut Core, space_id: SpaceId) -> Vec<PropertyId> {
    schema(core, space_id)
        .into_iter()
        .map(|def| def.id)
        .collect()
}

// ----- the requests ---------------------------------------------------------------------------

#[test]
fn a_page_is_made_a_template_and_a_page_again_with_its_events_the_tree_and_a_restart() {
    let (fixture, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Meeting").id;
    let child = create_page(&mut core, space, Some(top), "Agenda").id;
    let plain = create_page(&mut core, space, None, "Plain").id;
    core.drain_events();

    let info = set_template(&mut core, top, true).unwrap();
    assert!(info.template);
    assert_eq!(info.updated_at, T0, "the page is stamped");
    let events = core.drain_events();
    assert!(
        events.contains(&Event::StructureChanged { space_id: space }),
        "{events:?}"
    );
    // The page, its tree row and the sub-page: only the top carries the flag; the rows of the
    // whole subtree say they are in a template; the page next to it is not.
    assert!(page(&mut core, top).template);
    assert!(!page(&mut core, child).template);
    assert!(by_title(&mut core, space, "Meeting").template);
    assert!(!by_title(&mut core, space, "Agenda").template);
    assert!(indexed_in_template(&core, top) && indexed_in_template(&core, child));
    assert!(!indexed_in_template(&core, plain));
    assert_eq!(templates_indexed(&core, space), 2);

    // What the page already is changes nothing (the event is sent like every structure request's).
    let version = core.structure(space).unwrap().model.doc().version();
    set_template(&mut core, top, true).unwrap();
    assert_eq!(
        core.structure(space).unwrap().model.doc().version(),
        version,
        "nothing is written"
    );

    // It survives a restart, with the rows.
    drop(core);
    let mut core = fixture.open(1, T0 + 10);
    assert!(page(&mut core, top).template);
    assert!(indexed_in_template(&core, child));

    // And turning it off brings the subtree back.
    let info = set_template(&mut core, top, false).unwrap();
    assert!(!info.template);
    assert!(!indexed_in_template(&core, top) && !indexed_in_template(&core, child));
    assert_eq!(templates_indexed(&core, space), 0);
}

#[test]
fn the_flag_is_refused_for_a_page_in_the_trash_and_an_unknown_page() {
    let (_fx, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Meeting").id;
    let child = create_page(&mut core, space, Some(top), "Agenda").id;
    call(&mut core, Request::PageTrash { page_id: top });
    for page_id in [top, child] {
        let err = set_template(&mut core, page_id, true).unwrap_err();
        assert_eq!(field_of(&err), Some("pageId"));
    }
    let unknown = PageId::from_random_bytes([9; 16]);
    assert_eq!(
        set_template(&mut core, unknown, true).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        set_placeholder(
            &mut core,
            unknown,
            PropertyId::from_random_bytes([1; 16]),
            None
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(templates_indexed(&core, space), 0);
}

#[test]
fn the_flag_is_one_undo_step_that_rewrites_the_rows_of_the_subtree_each_way() {
    let (_fx, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Meeting").id;
    let child = create_page(&mut core, space, Some(top), "Agenda").id;
    set_template(&mut core, top, true).unwrap();
    assert!(indexed_in_template(&core, child));

    undo(&mut core, space, false);
    assert!(!page(&mut core, top).template);
    assert!(!indexed_in_template(&core, top) && !indexed_in_template(&core, child));
    undo(&mut core, space, true);
    assert!(page(&mut core, top).template);
    assert!(indexed_in_template(&core, top) && indexed_in_template(&core, child));
    assert_eq!(templates_indexed(&core, space), 2);
}

#[test]
fn placeholders_are_set_listed_in_schema_order_refused_and_replace_the_value() {
    let (fixture, mut core, space) = setup();
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date);
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person);
    let notes = prop_create(&mut core, space, "Notes", PropertyKind::Text);
    let top = create_page(&mut core, space, None, "Meeting").id;
    let child = create_page(&mut core, space, Some(top), "Agenda").id;
    let plain = create_page(&mut core, space, None, "Plain").id;
    core.drain_events();

    // Not a template and not inside one.
    let err =
        set_placeholder(&mut core, plain, due.id, Some(TemplatePlaceholder::Today)).unwrap_err();
    assert_eq!(field_of(&err), Some("pageId"));
    set_template(&mut core, top, true).unwrap();

    // The kinds: `today` and `now` a date, `me` a person.
    for (property, placeholder) in [
        (owner.id, TemplatePlaceholder::Today),
        (due.id, TemplatePlaceholder::Me),
        (notes.id, TemplatePlaceholder::Now),
    ] {
        let err = set_placeholder(&mut core, top, property, Some(placeholder)).unwrap_err();
        assert_eq!(field_of(&err), Some("placeholder"), "{placeholder:?}");
    }
    assert_eq!(
        set_placeholder(
            &mut core,
            top,
            PropertyId::from_random_bytes([4; 16]),
            Some(TemplatePlaceholder::Today)
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );

    // The value it replaces goes in the same commit; the page and a page inside the template both
    // report in schema order (Owner was set first).
    set_value(&mut core, top, due.id, day("2026-10-18"));
    assert_eq!(values_of(&mut core, top).len(), 1);
    core.drain_events();
    set_placeholder(&mut core, top, owner.id, Some(TemplatePlaceholder::Me)).unwrap();
    let info = set_placeholder(&mut core, top, due.id, Some(TemplatePlaceholder::Now)).unwrap();
    assert_eq!(
        info.template_placeholders,
        vec![
            (due.id, TemplatePlaceholder::Now),
            (owner.id, TemplatePlaceholder::Me)
        ]
    );
    assert!(
        values_of(&mut core, top).is_empty(),
        "the placeholder replaced the value"
    );
    assert!(info.properties.is_empty());
    let events = core.drain_events();
    assert!(
        events.contains(&Event::StructureChanged { space_id: space }),
        "{events:?}"
    );
    let inside =
        set_placeholder(&mut core, child, due.id, Some(TemplatePlaceholder::Today)).unwrap();
    assert!(!inside.template);
    assert_eq!(
        inside.template_placeholders,
        vec![(due.id, TemplatePlaceholder::Today)]
    );
    // The tree reports them too.
    assert_eq!(
        by_title(&mut core, space, "Meeting")
            .template_placeholders
            .len(),
        2
    );

    // `PagePropertySet` on a property that has a placeholder takes the placeholder away.
    set_value(&mut core, top, due.id, day("2026-11-02"));
    let info = page(&mut core, top);
    assert_eq!(
        info.template_placeholders,
        vec![(owner.id, TemplatePlaceholder::Me)]
    );
    assert_eq!(info.properties, vec![(due.id, day("2026-11-02"))]);

    // It survives a restart; clearing removes the entry.
    drop(core);
    let mut core = fixture.open(1, T0 + 10);
    assert_eq!(
        page(&mut core, top).template_placeholders,
        vec![(owner.id, TemplatePlaceholder::Me)]
    );
    let info = set_placeholder(&mut core, top, owner.id, None).unwrap();
    assert!(info.template_placeholders.is_empty());
    // A page that leaves the template reports none (its map stays).
    call(
        &mut core,
        Request::PageMove {
            page_id: child,
            parent_id: None,
            index: 0,
        },
    );
    assert!(page(&mut core, child).template_placeholders.is_empty());
    call(
        &mut core,
        Request::PageMove {
            page_id: child,
            parent_id: Some(top),
            index: 0,
        },
    );
    assert_eq!(page(&mut core, child).template_placeholders.len(), 1);
}

// ----- hiding ---------------------------------------------------------------------------------

#[test]
fn a_template_and_what_is_inside_it_leave_search_backlinks_and_reminders_and_the_counts_stay() {
    let (_fx, mut core, space) = setup();
    let target = create_page(&mut core, space, None, "Target").id;
    let top = create_page(&mut core, space, None, "Zebra meeting").id;
    let child = create_page(&mut core, space, Some(top), "Giraffe agenda").id;
    let plain = create_page(&mut core, space, None, "Zebra plain").id;
    write_text(&mut core, top, "zebra words here", 1);
    write_text(&mut core, child, "giraffe words here", 2);
    write_text(&mut core, plain, "zebra words too", 3);
    append_page_ref(&mut core, top, target, 40);
    append_page_ref(&mut core, child, target, 41);
    append_blocks(
        &mut core,
        top,
        json!([reminder_block(60, "Template reminder")]),
    );
    append_blocks(
        &mut core,
        plain,
        json!([reminder_block(70, "Plain reminder")]),
    );
    let counts = page_count(&mut core, space);
    assert_eq!(counts, 4);
    assert_eq!(found(&mut core, "zebra"), {
        let mut pages = vec![top, plain];
        pages.sort();
        pages
    });
    assert_eq!(found(&mut core, "giraffe"), vec![child]);
    assert_eq!(backlinks(&mut core, target).len(), 2);
    assert_eq!(reminders(&mut core).len(), 2);
    core.drain_events();

    set_template(&mut core, top, true).unwrap();

    // Title and body hits, the fuzzy pass and the spread pass leave the template and its sub-page out.
    assert_eq!(found(&mut core, "zebra"), vec![plain]);
    assert!(found(&mut core, "giraffe").is_empty());
    assert!(
        found(&mut core, "zebra words")
            .iter()
            .all(|id| *id == plain)
    );
    assert!(
        found(&mut core, "meetng").is_empty(),
        "a fuzzy title hit too"
    );
    assert!(found(&mut core, "agenda").is_empty());
    // A link from a template is not a backlink; the page it links to hears of it.
    assert!(backlinks(&mut core, target).is_empty());
    let events = core.drain_events();
    assert!(
        events.contains(&Event::BacklinksChanged { page_id: target }),
        "{events:?}"
    );
    // A reminder inside a template never fires or lists.
    assert_eq!(reminders(&mut core), vec![plain]);
    // The counts and the tree are as they were.
    assert_eq!(page_count(&mut core, space), counts);
    assert_eq!(by_title(&mut core, space, "Giraffe agenda").id, child);
    assert!(core.structure(space).unwrap().model.live_page_count() == counts);

    // And back.
    set_template(&mut core, top, false).unwrap();
    assert_eq!(found(&mut core, "giraffe"), vec![child]);
    assert_eq!(backlinks(&mut core, target).len(), 2);
    assert_eq!(reminders(&mut core).len(), 2);
    let events = core.drain_events();
    assert!(
        events.contains(&Event::BacklinksChanged { page_id: target }),
        "{events:?}"
    );
}

#[test]
fn comments_on_a_template_are_not_found() {
    let (_fx, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Meeting").id;
    let plain = create_page(&mut core, space, None, "Plain").id;
    for page_id in [top, plain] {
        // The comment index holds words by page; the join with the page index is what is tested.
        core.db
            .comment_fts_replace(
                space,
                page_id,
                &[kb_storage::CommentTextRow {
                    comment_id: kb_types::CommentId::from_random_bytes([page_id.to_bytes()[0]; 16]),
                    text: "comment marker words".to_owned(),
                }],
            )
            .unwrap();
    }
    let comment_hits = |core: &mut Core| -> Vec<PageId> {
        match core
            .dispatch(Request::Search {
                query: "marker".into(),
                space_id: None,
                scope: None,
                limit: None,
                edited_after: None,
            })
            .unwrap()
        {
            Response::Search(hits) => hits
                .into_iter()
                .filter(|hit| hit.comment_id.is_some())
                .map(|hit| hit.page_id)
                .collect(),
            other => panic!("unexpected {other:?}"),
        }
    };
    let mut both = vec![top, plain];
    both.sort();
    let mut before = comment_hits(&mut core);
    before.sort();
    assert_eq!(before, both);
    set_template(&mut core, top, true).unwrap();
    assert_eq!(comment_hits(&mut core), vec![plain]);
}

#[test]
fn pages_moved_into_or_out_of_a_template_get_their_rows_rewritten() {
    let (_fx, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Meeting").id;
    let loose = create_page(&mut core, space, None, "Loose").id;
    let loose_child = create_page(&mut core, space, Some(loose), "Loose child").id;
    let inside = create_page(&mut core, space, Some(top), "Inside").id;
    set_template(&mut core, top, true).unwrap();
    assert!(indexed_in_template(&core, inside));
    core.drain_events();

    // In: the page and its sub-page.
    call(
        &mut core,
        Request::PageMove {
            page_id: loose,
            parent_id: Some(top),
            index: 0,
        },
    );
    assert!(indexed_in_template(&core, loose) && indexed_in_template(&core, loose_child));
    assert_eq!(templates_indexed(&core, space), 4);
    // Out again.
    call(
        &mut core,
        Request::PageMove {
            page_id: loose,
            parent_id: None,
            index: 0,
        },
    );
    assert!(!indexed_in_template(&core, loose) && !indexed_in_template(&core, loose_child));
    assert!(indexed_in_template(&core, inside));
    // A page created under a template is inside it at once.
    let fresh = create_page(&mut core, space, Some(top), "Fresh").id;
    assert!(indexed_in_template(&core, fresh));
    // Trashing and restoring a page of a template keeps it inside.
    call(&mut core, Request::PageTrash { page_id: inside });
    call(&mut core, Request::PageRestore { page_id: inside });
    assert!(indexed_in_template(&core, inside));
}

#[test]
fn the_index_rebuild_and_the_check_at_open_restore_the_rows_that_say_template() {
    let (fixture, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Meeting").id;
    let child = create_page(&mut core, space, Some(top), "Agenda").id;
    let plain = create_page(&mut core, space, None, "Plain").id;
    set_template(&mut core, top, true).unwrap();
    let wipe = |core: &Core| {
        core.db
            .connection()
            .execute("UPDATE pages_index SET in_template = 0", [])
            .unwrap();
    };

    // `IndexRebuild` writes the column again from the structure document.
    wipe(&core);
    assert!(!indexed_in_template(&core, child));
    call(
        &mut core,
        Request::IndexRebuild {
            space_id: Some(space),
        },
    );
    assert!(indexed_in_template(&core, top) && indexed_in_template(&core, child));
    assert!(!indexed_in_template(&core, plain));
    wipe(&core);
    call(&mut core, Request::IndexRebuild { space_id: None });
    assert_eq!(templates_indexed(&core, space), 2);

    // A database whose rows lack the column's value (a flag written by another device while this one
    // was older) is rebuilt at open: the live counts are the same, the template counts differ.
    wipe(&core);
    drop(core);
    let mut core = fixture.open(1, T0 + 10);
    assert!(indexed_in_template(&core, top) && indexed_in_template(&core, child));
    assert!(!indexed_in_template(&core, plain));
    assert_eq!(page_count(&mut core, space), 3);
}

// ----- the semantic index ---------------------------------------------------------------------

fn ai_on(core: &mut Core) {
    core.dispatch(Request::SettingsSet {
        key: AI_SETTINGS_SETTING.into(),
        value: json!({ "enabled": true, "embeddingModel": "test-embed", "semanticIndex": true }),
    })
    .unwrap();
}

fn ai_next(core: &mut Core) -> AiIndexWork {
    match core
        .dispatch(Request::AiIndexNext { max_chunks: 64 })
        .unwrap()
    {
        Response::AiIndexWork(work) => work,
        other => panic!("unexpected {other:?}"),
    }
}

fn ai_status(core: &mut Core) -> AiIndexInfo {
    match core.dispatch(Request::AiIndexStatus).unwrap() {
        Response::AiIndex(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn ai_store(core: &mut Core, work: &AiIndexWork) -> Vec<PageId> {
    let model = work.model.clone().unwrap();
    for page in &work.pages {
        let vectors = page
            .chunks
            .iter()
            .filter(|chunk| chunk.needs_vector)
            .map(|chunk| AiChunkVector {
                seq: chunk.seq,
                vector: {
                    let mut v = vec![0.0; 16];
                    v[chunk.text.len() % 16] = 1.0;
                    v
                },
            })
            .collect();
        core.dispatch(Request::AiIndexStore {
            page_id: page.page_id,
            source_hash: page.source_hash.clone(),
            model: model.clone(),
            vectors,
        })
        .unwrap();
    }
    work.pages.iter().map(|page| page.page_id).collect()
}

fn ai_index_all(core: &mut Core) -> Vec<PageId> {
    let mut stored = Vec::new();
    for _ in 0..100 {
        let work = ai_next(core);
        if work.pages.is_empty() && work.remaining == 0 {
            stored.sort();
            return stored;
        }
        stored.extend(ai_store(core, &work));
    }
    panic!("the index does not settle");
}

fn ai_retrieve(core: &mut Core, query: &str, scope: AiScope) -> Vec<AiSource> {
    match core
        .dispatch(Request::AiRetrieve {
            query: query.to_owned(),
            vector: None,
            scope,
            limit: None,
        })
        .unwrap()
    {
        Response::AiPassages(passages) => passages,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn templates_are_not_indexed_for_the_ai_and_a_page_that_becomes_one_loses_its_chunks() {
    let (_fx, mut core, space) = setup();
    ai_on(&mut core);
    let top = create_page(&mut core, space, None, "Meeting").id;
    let child = create_page(&mut core, space, Some(top), "Agenda").id;
    let plain = create_page(&mut core, space, None, "Plain").id;
    write_text(&mut core, top, "alpha meeting words", 1);
    write_text(&mut core, child, "alpha agenda words", 2);
    write_text(&mut core, plain, "alpha plain words", 3);
    let mut all = vec![top, child, plain];
    all.sort();
    assert_eq!(ai_index_all(&mut core), all);
    assert_eq!(ai_status(&mut core).pages_indexed, 3);
    assert_eq!(ai_retrieve(&mut core, "alpha words", AiScope::All).len(), 3);
    core.drain_events();

    // The page becomes a template: its chunks and its sub-page's go at once, it is no page the index
    // is waiting for, and retrieval leaves it out in every scope.
    set_template(&mut core, top, true).unwrap();
    assert!(core.db.ai_page_get(top).unwrap().is_none());
    assert!(core.db.ai_page_get(child).unwrap().is_none());
    assert!(core.db.ai_page_get(plain).unwrap().is_some());
    assert!(
        core.drain_events().contains(&Event::AiIndexChanged),
        "the index changed"
    );
    let info = ai_status(&mut core);
    assert_eq!(
        (info.pages_total, info.pages_indexed, info.chunks),
        (1, 1, 1)
    );
    let passages = ai_retrieve(&mut core, "alpha words", AiScope::All);
    assert!(passages.iter().all(|passage| passage.page_id == plain));
    assert!(ai_retrieve(&mut core, "alpha words", AiScope::Page { page_id: top }).is_empty());
    assert!(
        ai_retrieve(&mut core, "alpha words", AiScope::Space { space_id: space })
            .iter()
            .all(|passage| passage.page_id == plain)
    );
    // The walk has no work for it, whatever marks it: an edit of the template is not indexed.
    write_text(&mut core, top, "alpha template edit", 4);
    assert!(ai_next(&mut core).pages.is_empty());
    assert!(core.db.ai_page_get(top).unwrap().is_none());
    assert_eq!(core.db.ai_dirty_count().unwrap(), 0);

    // A page that moves into a template loses its chunks in the next walk of the index.
    call(
        &mut core,
        Request::PageMove {
            page_id: plain,
            parent_id: Some(top),
            index: 0,
        },
    );
    assert!(core.db.ai_page_get(plain).unwrap().is_none());
    assert_eq!(ai_status(&mut core).chunks, 0);

    // Turned off, the pages are indexed again by the walk.
    set_template(&mut core, top, false).unwrap();
    let again = ai_index_all(&mut core);
    assert_eq!(again, all);
    assert_eq!(ai_status(&mut core).pages_indexed, 3);
}

// ----- MCP ------------------------------------------------------------------------------------

fn mcp_call(core: &mut Core, token: &str, name: &str, arguments: &Value) -> (String, bool) {
    let message = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    let reply = match core
        .dispatch(Request::McpCall {
            token: token.into(),
            message: message.to_string(),
        })
        .unwrap()
    {
        Response::McpReply(reply) => reply,
        other => panic!("unexpected {other:?}"),
    };
    let body: Value = serde_json::from_str(&reply.body.unwrap()).unwrap();
    (
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned(),
        body["result"]["isError"].as_bool().unwrap(),
    )
}

#[test]
fn mcp_lists_and_searches_leave_templates_out_and_read_page_by_id_still_works() {
    let (_fx, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Zebra template").id;
    let child = create_page(&mut core, space, Some(top), "Zebra agenda").id;
    let plain = create_page(&mut core, space, None, "Zebra plain").id;
    let parent = create_page(&mut core, space, None, "Zebra parent").id;
    create_page(&mut core, space, Some(parent), "Zebra kid");
    write_text(&mut core, top, "zebra template words", 1);
    set_template(&mut core, top, true).unwrap();
    // A sub-page of an ordinary page that is itself a template is hidden and not counted.
    let nested = create_page(&mut core, space, Some(parent), "Zebra nested template").id;
    set_template(&mut core, nested, true).unwrap();

    let created = match core
        .dispatch(Request::McpClientCreate {
            name: "agent".into(),
        })
        .unwrap()
    {
        Response::McpClientCreated(created) => created,
        other => panic!("unexpected {other:?}"),
    };
    core.dispatch(Request::McpClientGrantsSet {
        client_id: created.client.id.clone(),
        grants: vec![McpGrant {
            space_id: space,
            read: true,
            write: false,
        }],
    })
    .unwrap();
    let token = created.token;
    let space_arg = space.to_string();

    let (listing, is_error) = mcp_call(
        &mut core,
        &token,
        "list_pages",
        &json!({ "spaceId": space_arg }),
    );
    assert!(!is_error, "{listing}");
    assert!(
        listing.contains("Zebra plain") && listing.contains("Zebra parent"),
        "{listing}"
    );
    assert!(
        !listing.contains("Zebra template") && !listing.contains("Zebra agenda"),
        "{listing}"
    );
    assert!(
        listing.contains("(pageId: ") && listing.contains("1 subpage"),
        "the sub-page count leaves the nested template out: {listing}"
    );
    assert!(!listing.contains("2 subpages"), "{listing}");
    // The sub-pages of the parent: the template is not listed.
    let (kids, _) = mcp_call(
        &mut core,
        &token,
        "list_pages",
        &json!({ "spaceId": space_arg, "parentId": parent.to_string() }),
    );
    assert!(
        kids.contains("Zebra kid") && !kids.contains("nested"),
        "{kids}"
    );
    // The sub-pages of a template are not found.
    for page_id in [top, child, nested] {
        let (answer, is_error) = mcp_call(
            &mut core,
            &token,
            "list_pages",
            &json!({ "spaceId": space_arg, "parentId": page_id.to_string() }),
        );
        assert!(is_error, "{answer}");
        assert!(answer.contains("not found"), "{answer}");
    }
    // Search leaves them out too.
    let (hits, is_error) = mcp_call(&mut core, &token, "search", &json!({ "query": "zebra" }));
    assert!(!is_error, "{hits}");
    assert!(hits.contains("Zebra plain"), "{hits}");
    assert!(
        !hits.contains("Zebra template") && !hits.contains("Zebra agenda"),
        "{hits}"
    );
    // `read_page` by id still works.
    let (page_text, is_error) = mcp_call(
        &mut core,
        &token,
        "read_page",
        &json!({ "pageId": top.to_string() }),
    );
    assert!(!is_error, "{page_text}");
    assert!(page_text.contains("zebra template words"), "{page_text}");
    let _ = plain;
}

// ----- export, import and moves ---------------------------------------------------------------

#[test]
fn the_flag_and_the_placeholders_survive_an_export_and_an_import_by_property_name() {
    let (_fx, mut core, space) = setup();
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date);
    let owner = prop_create(&mut core, space, "Owner", PropertyKind::Person);
    prop_create(&mut core, space, "Notes", PropertyKind::Text);
    let top = create_page(&mut core, space, None, "Daily").id;
    let child = create_page(&mut core, space, Some(top), "Sub").id;
    let plain = create_page(&mut core, space, None, "Ordinary").id;
    write_text(&mut core, top, "marker words daily", 1);
    write_text(&mut core, plain, "marker words ordinary", 2);
    set_template(&mut core, top, true).unwrap();
    set_placeholder(&mut core, top, due.id, Some(TemplatePlaceholder::Today)).unwrap();
    set_placeholder(&mut core, child, owner.id, Some(TemplatePlaceholder::Me)).unwrap();

    let exported = export_all(&mut core, space, None, true, ExportFormat::Markdown, 100);
    let file_of = |title: &str| -> String {
        exported
            .paths()
            .into_iter()
            .find(|path| path.ends_with(&format!("{title}.md")))
            .unwrap_or_else(|| panic!("no file for {title} in {:?}", exported.paths()))
            .to_owned()
    };
    let daily = exported.text(&file_of("Daily"));
    assert!(
        daily.contains("\ntemplate: true\ntemplatePlaceholders:\n  \"Due\": \"today\"\n"),
        "{daily}"
    );
    let sub = exported.text(&file_of("Sub"));
    assert!(
        sub.contains("\ntemplatePlaceholders:\n  \"Owner\": \"me\"\n"),
        "{sub}"
    );
    assert!(!sub.contains("template: true"), "{sub}");
    let ordinary = exported.text(&file_of("Ordinary"));
    assert!(!ordinary.contains("template"), "{ordinary}");

    // Into another workspace: the flag is restored after the content, the placeholders follow the
    // property names the import matched or made.
    let (_fx2, mut other, other_space) = another_core(2);
    let (texts, binaries) = as_source(&mut core, &exported);
    run_import(
        &mut other,
        other_space,
        None,
        Some("Copy"),
        &texts,
        &binaries,
        100,
    );
    let daily = by_title(&mut other, other_space, "Daily");
    let sub = by_title(&mut other, other_space, "Sub");
    let ordinary = by_title(&mut other, other_space, "Ordinary");
    assert!(daily.template && !sub.template && !ordinary.template);
    let due_there = property_named(&mut other, other_space, "Due");
    let owner_there = property_named(&mut other, other_space, "Owner");
    assert_eq!(due_there.kind, PropertyKind::Date);
    assert_eq!(
        daily.template_placeholders,
        vec![(due_there.id, TemplatePlaceholder::Today)]
    );
    // An import never makes a Person property (the people of an export are names, ADR-0049): the
    // property that comes back is a text one, which takes no `me`, so that placeholder is dropped.
    assert_eq!(owner_there.kind, PropertyKind::Text);
    assert!(sub.template_placeholders.is_empty());
    // The index rows say so, and the imported templates are left out of the lists.
    assert!(indexed_in_template(&other, daily.id) && indexed_in_template(&other, sub.id));
    assert!(!indexed_in_template(&other, ordinary.id));
    assert_eq!(found(&mut other, "marker"), vec![ordinary.id]);
    // A template is a live page: the container and the three pages are counted.
    assert_eq!(page_count(&mut other, other_space), 4);

    // Into a space that has the Person property, under a template (the placeholder needs its page
    // in one): the file of the sub-page gets its placeholder back, matched to the property by name.
    let host = create_page(&mut core, space, None, "Host").id;
    set_template(&mut core, host, true).unwrap();
    let exported = export_all(
        &mut core,
        space,
        Some(child),
        false,
        ExportFormat::Markdown,
        100,
    );
    let (texts, binaries) = as_source(&mut core, &exported);
    run_import(&mut core, space, Some(host), None, &texts, &binaries, 100);
    let view = tree(&mut core, space);
    let mut all = Vec::new();
    flatten(&view.roots, &mut all);
    let copy: Vec<&PageInfo> = all
        .iter()
        .filter(|info| info.title == "Sub" && info.id != child)
        .collect();
    assert_eq!(copy.len(), 1);
    assert!(!copy[0].template);
    assert_eq!(
        copy[0].template_placeholders,
        vec![(owner.id, TemplatePlaceholder::Me)]
    );
    assert!(indexed_in_template(&core, copy[0].id));
    let _ = plain;
}

#[test]
fn a_template_imported_under_a_template_is_inside_it_and_a_value_stays_a_value() {
    let (_fx, mut core, space) = setup();
    let due = prop_create(&mut core, space, "Due", PropertyKind::Date);
    let host = create_page(&mut core, space, None, "Host").id;
    set_template(&mut core, host, true).unwrap();
    let top = create_page(&mut core, space, None, "Source").id;
    set_value(&mut core, top, due.id, day("2026-10-18"));
    let exported = export_all(
        &mut core,
        space,
        Some(top),
        false,
        ExportFormat::Markdown,
        100,
    );
    let (texts, binaries) = as_source(&mut core, &exported);
    run_import(&mut core, space, Some(host), None, &texts, &binaries, 100);
    // The page imported under the template host is inside it (its row says so at creation) and is
    // no template itself.
    let imported = core.structure(space).unwrap().model.page(host).unwrap();
    assert!(imported.template);
    let view = tree(&mut core, space);
    let mut all = Vec::new();
    flatten(&view.roots, &mut all);
    let copies: Vec<&PageInfo> = all
        .iter()
        .filter(|info| info.title == "Source" && info.id != top)
        .collect();
    assert_eq!(copies.len(), 1);
    assert!(!copies[0].template);
    assert!(indexed_in_template(&core, copies[0].id));
    assert_eq!(copies[0].properties, vec![(due.id, day("2026-10-18"))]);
}

#[test]
fn a_move_to_another_space_carries_the_flag_and_the_placeholders_by_the_property_mapping() {
    let (_fx, mut core, personal) = setup();
    let work = create_space(&mut core, "Work").id;
    let due = prop_create(&mut core, personal, "Due", PropertyKind::Date);
    let owner = prop_create(&mut core, personal, "Owner", PropertyKind::Person);
    // The target has an Owner of its own and no Due.
    let owner_there = prop_create(&mut core, work, "owner", PropertyKind::Person);
    let top = create_page(&mut core, personal, None, "Daily").id;
    let child = create_page(&mut core, personal, Some(top), "Sub").id;
    write_text(&mut core, top, "marker words daily", 1);
    set_template(&mut core, top, true).unwrap();
    set_placeholder(&mut core, top, due.id, Some(TemplatePlaceholder::Today)).unwrap();
    set_placeholder(&mut core, child, owner.id, Some(TemplatePlaceholder::Me)).unwrap();
    core.drain_events();

    let summary = match core
        .dispatch(Request::PageMoveToSpace {
            page_id: top,
            space_id: work,
            parent_id: None,
            index: None,
        })
        .unwrap()
    {
        Response::PagesMovedToSpace(summary) => summary,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        summary.properties_created, 1,
        "Due is made for its placeholder"
    );
    let due_there = property_named(&mut core, work, "Due");
    assert_eq!(due_there.kind, PropertyKind::Date);

    let moved = page(&mut core, top);
    assert!(moved.template);
    assert_eq!(moved.space_id, work);
    assert_eq!(
        moved.template_placeholders,
        vec![(due_there.id, TemplatePlaceholder::Today)]
    );
    let sub = page(&mut core, child);
    assert!(!sub.template);
    assert_eq!(
        sub.template_placeholders,
        vec![(owner_there.id, TemplatePlaceholder::Me)]
    );
    // The rows in the new space say the pages are in a template, the content stays out of search,
    // and the page counts follow the pages.
    assert!(indexed_in_template(&core, top) && indexed_in_template(&core, child));
    assert!(found(&mut core, "marker").is_empty());
    assert_eq!(page_count(&mut core, work), 2);
    assert_eq!(page_count(&mut core, personal), 0);
    assert_eq!(templates_indexed(&core, work), 2);
    assert_eq!(templates_indexed(&core, personal), 0);
    assert!(schema_ids(&mut core, work).contains(&due_there.id));
}

#[test]
fn a_page_moved_into_a_template_of_another_space_is_inside_it() {
    let (_fx, mut core, personal) = setup();
    let work = create_space(&mut core, "Work").id;
    let host = create_page(&mut core, work, None, "Host").id;
    set_template(&mut core, host, true).unwrap();
    let page_id = create_page(&mut core, personal, None, "Notes").id;
    let kid = create_page(&mut core, personal, Some(page_id), "Notes kid").id;
    write_text(&mut core, page_id, "needle words", 1);
    assert_eq!(found(&mut core, "needle"), vec![page_id]);
    call(
        &mut core,
        Request::PageMoveToSpace {
            page_id,
            space_id: work,
            parent_id: Some(host),
            index: None,
        },
    );
    assert!(indexed_in_template(&core, page_id) && indexed_in_template(&core, kid));
    assert!(found(&mut core, "needle").is_empty());
    assert!(!page(&mut core, page_id).template);
}

#[test]
fn due_rows_of_a_template_stay_in_the_table_and_come_back_with_the_flag() {
    // The reminder rows are derived data of the page document: turning the flag off needs no rewrite.
    let (_fx, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Meeting").id;
    append_blocks(&mut core, top, json!([reminder_block(60, "Standup")]));
    assert_eq!(core.db.date_mentions_of(top).unwrap().len(), 1);
    set_template(&mut core, top, true).unwrap();
    assert_eq!(core.db.date_mentions_of(top).unwrap().len(), 1);
    assert!(reminders(&mut core).is_empty());
    set_template(&mut core, top, false).unwrap();
    assert_eq!(reminders(&mut core), vec![top]);
}
