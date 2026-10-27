//! Page history (ADR-0057, `docs/architecture/page-history.md`) through `Request`: the journal and its
//! sessions, the version list (dated, then undated) with its paging, the content at a version, restore
//! (content, title, events, sync, roles, trash), what purge, backup and the index rebuild do to the
//! journal, and that the history survives compaction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_doc::{BlobKind, Doc, DocKind, Frontiers};
use kb_model::{BlockKind, PageDoc};
use kb_storage::{DocKindCode, DocQueries, PageHistoryQueries, TextIndexQueries};
use kb_types::testing::FixedClock;
use kb_types::{
    ApiError, BlockId, ChangeSource, ErrorCode, Event, MemberRole, PageHistory, PageId, PageInfo,
    PageVersion, PageVersionContent, PageVersionKind, Request, Response, SpaceId,
};
use serde_json::Value;

use crate::CompactionPolicy;
use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, kit_create, page, page_content, spaces,
    write_text,
};
use crate::tests_sharing::{Dev, add_member, server, share};

const MIN: i64 = 60_000;
/// A plausible moment (2001-09-09), far from zero.
const T0: i64 = 1_000_000_000_000;

// ----- helpers ----------------------------------------------------------------------------------

fn setup() -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(1, T0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    core.drain_events();
    (fx, core, space)
}

/// Moves the core's clock (tests live inside the crate, so the field is theirs to swap).
fn at(core: &mut Core, now_ms: i64) {
    core.clock = Box::new(FixedClock::new(now_ms));
}

fn list(core: &mut Core, page_id: PageId) -> PageHistory {
    list_with(core, page_id, None, None).unwrap()
}

fn list_with(
    core: &mut Core,
    page_id: PageId,
    before: Option<&str>,
    limit: Option<u32>,
) -> Result<PageHistory, ApiError> {
    match core.dispatch(Request::PageHistoryList {
        page_id,
        before: before.map(str::to_owned),
        limit,
    })? {
        Response::PageHistory(history) => Ok(history),
        other => panic!("unexpected {other:?}"),
    }
}

fn get(core: &mut Core, page_id: PageId, version: &str) -> Result<PageVersionContent, ApiError> {
    match core.dispatch(Request::PageHistoryGet {
        page_id,
        version: version.to_owned(),
    })? {
        Response::PageVersionContent(content) => Ok(content),
        other => panic!("unexpected {other:?}"),
    }
}

fn restore(core: &mut Core, page_id: PageId, version: &str) -> Result<PageInfo, ApiError> {
    match core.dispatch(Request::PageHistoryRestore {
        page_id,
        version: version.to_owned(),
    })? {
        Response::Page(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn ids(history: &PageHistory) -> Vec<String> {
    history.versions.iter().map(|v| v.id.clone()).collect()
}

fn dated(history: &PageHistory) -> Vec<&PageVersion> {
    history
        .versions
        .iter()
        .filter(|v| v.ended_at.is_some())
        .collect()
}

fn undated(history: &PageHistory) -> Vec<&PageVersion> {
    history
        .versions
        .iter()
        .filter(|v| v.ended_at.is_none())
        .collect()
}

fn current(history: &PageHistory) -> &PageVersion {
    let all: Vec<&PageVersion> = history.versions.iter().filter(|v| v.current).collect();
    assert_eq!(all.len(), 1, "exactly one current version: {history:?}");
    all[0]
}

/// The text of every node of portable content, in document order.
fn texts(content: &Value) -> Vec<String> {
    fn walk(value: &Value, out: &mut Vec<String>) {
        if let Value::Object(map) = value {
            if let Some(Value::String(text)) = map.get("text") {
                out.push(text.clone());
            }
            if let Some(Value::Array(children)) = map.get("content") {
                for child in children {
                    walk(child, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(content, &mut out);
    out
}

fn page_texts(core: &mut Core, page_id: PageId) -> Vec<String> {
    texts(&page_content(core, page_id))
}

fn journal(core: &Core, page_id: PageId) -> Vec<kb_storage::JournalRow> {
    core.db
        .journal_list(kb_types::DocId::for_page(page_id), None, None)
        .unwrap()
}

fn field_of(err: &ApiError) -> Option<&str> {
    assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");
    err.details.as_ref()?.get("field")?.as_str()
}

fn rename(core: &mut Core, page_id: PageId, title: &str) {
    core.dispatch(Request::PageRename {
        page_id,
        title: title.into(),
    })
    .unwrap();
}

/// What an editor sends when a replica that holds the page's snapshot appends a paragraph: the update's
/// raw bytes, the replica not told to the core (so several of them can be concurrent).
fn editor_update(snapshot: &[u8], text: &str, seed: u64) -> Vec<u8> {
    let blob = kb_doc::encode_blob(BlobKind::Snapshot, snapshot);
    let doc =
        PageDoc::from_doc(Doc::load(DocKind::Page, 0x7100_0000 + seed, Some(&blob), &[]).unwrap())
            .unwrap();
    let before = doc.doc().version();
    let last = doc.block_ids().last().copied();
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&seed.to_le_bytes());
    id[8..].copy_from_slice(&seed.rotate_left(23).to_le_bytes());
    doc.insert_block(
        last,
        BlockKind::Paragraph,
        text,
        BlockId::from_random_bytes(id),
    )
    .unwrap();
    let update = doc
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    kb_doc::decode_blob(&update).unwrap().1.to_vec()
}

/// An editor appends a paragraph that holds a reference to `target`.
fn write_link(core: &mut Core, page_id: PageId, target: PageId, seed: u64) {
    let snapshot = core.page_doc_open(page_id).unwrap();
    let blob = kb_doc::encode_blob(BlobKind::Snapshot, &snapshot);
    let doc =
        PageDoc::from_doc(Doc::load(DocKind::Page, 0x7200_0000 + seed, Some(&blob), &[]).unwrap())
            .unwrap();
    let before = doc.doc().version();
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&seed.to_le_bytes());
    id[8..].copy_from_slice(&seed.rotate_left(29).to_le_bytes());
    let block = BlockId::from_random_bytes(id);
    let last = doc.block_ids().last().copied();
    doc.insert_block(last, BlockKind::Paragraph, "see ", block)
        .unwrap();
    id[0] ^= 0xff;
    doc.insert_page_ref(block, BlockId::from_random_bytes(id), target, None)
        .unwrap();
    let update = doc
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    core.page_doc_import(page_id, payload).unwrap();
    core.page_doc_close(page_id).unwrap();
}

/// A page with editing sessions an hour apart, each appending one paragraph: the titles and the texts
/// of the versions are known. Answers the page and its versions, oldest first.
fn page_with_sessions(
    core: &mut Core,
    space: SpaceId,
    texts: &[&str],
) -> (PageId, Vec<PageVersion>) {
    at(core, T0);
    let page_id = create_page(core, space, None, "Plan").id;
    for (n, text) in texts.iter().enumerate() {
        at(core, T0 + (i64::try_from(n).unwrap() + 1) * 60 * MIN);
        write_text(core, page_id, text, 10 + n as u64);
    }
    let mut versions = list(core, page_id).versions;
    versions.reverse();
    (page_id, versions)
}

// ----- ids ------------------------------------------------------------------------------------------

#[test]
fn version_ids_are_the_documents_frontiers_and_ask_for_the_same_version_back() {
    let (_fx, mut core, space) = setup();
    let (page_id, versions) = page_with_sessions(&mut core, space, &["one", "two"]);
    assert_eq!(versions.len(), 3, "the creation and two sessions");
    for version in &versions {
        // The text of the spec: peer@counter pairs, peers in numeric order, joined with commas.
        let parsed = Frontiers::parse(&version.id).unwrap();
        assert_eq!(parsed.encode(), version.id);
        let content = get(&mut core, page_id, &version.id).unwrap();
        assert_eq!(content.version, version.id);
        assert_eq!(content.page_id, page_id);
    }
    // The current version is the document's frontiers now.
    let doc_now = {
        let open = core.open_page(page_id).unwrap();
        open.model.doc().frontiers().encode()
    };
    assert_eq!(current(&list(&mut core, page_id)).id, doc_now);
    // A non-canonical spelling of a version asks for the same one.
    let newest = versions.last().unwrap();
    let padded = format!("{},{}", newest.id, versions[0].id);
    let same = get(&mut core, page_id, &padded).unwrap();
    assert_eq!(
        same.version, newest.id,
        "the older head is implied by the newer"
    );
}

#[test]
fn ids_that_do_not_parse_or_name_unknown_changes_are_invalid_input() {
    let (_fx, mut core, space) = setup();
    let (page_id, versions) = page_with_sessions(&mut core, space, &["one"]);
    core.drain_events();
    for bad in [
        "",
        "nonsense",
        "1@",
        "@1",
        "1@x",
        "99999999999@99999",
        "+1@2",
    ] {
        let err = get(&mut core, page_id, bad).unwrap_err();
        assert_eq!(field_of(&err), Some("version"), "get {bad:?}");
        let err = restore(&mut core, page_id, bad).unwrap_err();
        assert_eq!(field_of(&err), Some("version"), "restore {bad:?}");
        if !bad.is_empty() {
            let err = list_with(&mut core, page_id, Some(bad), None).unwrap_err();
            assert_eq!(field_of(&err), Some("before"), "before {bad:?}");
        }
    }
    // An id that parses but is not a version of this page's document is not "before" either.
    let unknown = "4242@7";
    assert_eq!(
        field_of(&list_with(&mut core, page_id, Some(unknown), None).unwrap_err()),
        Some("before")
    );
    // Nothing changed for all of this.
    assert_eq!(ids(&list(&mut core, page_id)).len(), versions.len());
    assert!(core.drain_events().is_empty());
}

#[test]
fn an_unknown_page_is_not_found_and_a_page_without_a_document_has_no_history() {
    let (_fx, mut core, space) = setup();
    let ghost = PageId::from_bytes([9; 16]);
    for err in [
        list_with(&mut core, ghost, None, None).unwrap_err(),
        get(&mut core, ghost, "1@1").unwrap_err(),
        restore(&mut core, ghost, "1@1").unwrap_err(),
    ] {
        assert_eq!(err.code, ErrorCode::NotFound);
    }
    // A page whose document is not here yet (its content has not arrived): nothing to list, and
    // nothing is created by asking.
    let p = create_page(&mut core, space, None, "Empty");
    core.docs.pages.remove(&p.id);
    core.db.doc_delete(kb_types::DocId::for_page(p.id)).unwrap();
    let history = list(&mut core, p.id);
    assert!(history.versions.is_empty());
    assert!(!history.more);
    assert_eq!(
        field_of(&get(&mut core, p.id, "1@1").unwrap_err()),
        Some("version")
    );
    assert_eq!(
        field_of(&restore(&mut core, p.id, "1@1").unwrap_err()),
        Some("version")
    );
    assert_eq!(
        field_of(&list_with(&mut core, p.id, Some("1@1"), None).unwrap_err()),
        Some("before")
    );
    assert!(
        core.db
            .doc_get(kb_types::DocId::for_page(p.id))
            .unwrap()
            .is_none(),
        "asking for history created no document"
    );
}

#[test]
fn opening_an_existing_page_writes_nothing_and_does_no_journal_work() {
    let (fx, mut core, space) = setup();
    let (page_id, _) = page_with_sessions(&mut core, space, &["one", "two"]);
    drop(core);

    // A new process: the page is opened, read, listed and read at a version, and no row of the
    // database is inserted, updated or deleted by any of it (the journal's included).
    let mut core = fx.open(2, T0 + 5 * 60 * MIN);
    let rows = journal(&core, page_id);
    let changes = core.db.connection().total_changes();
    core.page_doc_open(page_id).unwrap();
    core.page_doc_close(page_id).unwrap();
    assert_eq!(page_texts(&mut core, page_id), ["one", "two"]);
    let history = list(&mut core, page_id);
    get(&mut core, page_id, &history.versions[1].id).unwrap();
    core.page_doc_open(page_id).unwrap();
    assert_eq!(
        core.db.connection().total_changes(),
        changes,
        "nothing was written"
    );
    assert_eq!(journal(&core, page_id), rows);
}

#[test]
fn a_journal_that_cannot_be_written_never_fails_or_undoes_the_write_it_records() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    write_text(&mut core, page_id, "before", 1);
    let rows = journal(&core, page_id);
    // The journal refuses every write from now on.
    core.db
        .connection()
        .execute_batch(
            "CREATE TRIGGER journal_refuses_insert BEFORE INSERT ON page_history_journal \
             BEGIN SELECT RAISE(ABORT, 'refused'); END; \
             CREATE TRIGGER journal_refuses_update BEFORE UPDATE ON page_history_journal \
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();

    // A session that goes on (an update of the row), a new session (an insert), a restore, and a new
    // page (the insert rides in the transaction that creates the document): all succeed in full.
    at(&mut core, T0 + MIN);
    write_text(&mut core, page_id, "continues", 2);
    at(&mut core, T0 + 60 * MIN);
    write_text(&mut core, page_id, "starts", 3);
    assert_eq!(
        page_texts(&mut core, page_id),
        ["before", "continues", "starts"]
    );
    let first = list(&mut core, page_id).versions.last().unwrap().id.clone();
    restore(&mut core, page_id, &first).unwrap();
    assert!(page_texts(&mut core, page_id).len() < 3);
    let other = create_page(&mut core, space, None, "Other").id;
    write_text(&mut core, other, "kept", 4);
    assert_eq!(page_texts(&mut core, other), ["kept"]);
    assert_eq!(journal(&core, page_id), rows, "the journal is as it was");
    assert!(journal(&core, other).is_empty());
    // And the document still has its history: undated, the newest first.
    let history = list(&mut core, other);
    assert!(!history.versions.is_empty());
    assert!(history.versions[0].current);
}

// ----- the journal: sessions ---------------------------------------------------------------------

#[test]
fn a_new_page_starts_its_history_with_a_row_of_this_device() {
    let (_fx, mut core, space) = setup();
    let device = core.device.device_id;
    at(&mut core, T0 + 5);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 1);
    assert!(!history.more);
    let only = &history.versions[0];
    assert!(only.current);
    assert_eq!(only.kind, PageVersionKind::Edit);
    assert_eq!(
        (only.started_at, only.ended_at),
        (Some(T0 + 5), Some(T0 + 5))
    );
    assert_eq!(only.title.as_deref(), Some("Plan"));
    let author = only.author.as_ref().unwrap();
    assert!(author.this_device);
    assert_eq!(author.device_id, Some(device));
    assert_eq!(
        author.account_id, None,
        "no account before the recovery kit"
    );
    // The journal's row, with the empty base of a document that began with it.
    let rows = journal(&core, page_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].base_version, "");
    assert_eq!(rows[0].version, only.id);
    assert!(
        undated(&history).is_empty(),
        "a journal that starts at creation has none"
    );
    // With a kit the account is known too.
    kit_create(&mut core);
    let second = create_page(&mut core, space, None, "Other").id;
    let author = list(&mut core, second).versions[0].author.clone().unwrap();
    assert!(author.account_id.is_some());
    assert!(author.this_device);
}

#[test]
fn writes_of_one_author_within_ten_minutes_are_one_session_and_a_gap_starts_another() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let created = list(&mut core, page_id).versions[0].clone();

    // A minute later: the creation's session goes on (same author, same kind, inside the gap).
    at(&mut core, T0 + MIN);
    write_text(&mut core, page_id, "first", 1);
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 1);
    let session = &history.versions[0];
    assert_ne!(session.id, created.id, "the version moved with the content");
    assert_eq!(
        (session.started_at, session.ended_at),
        (Some(T0), Some(T0 + MIN))
    );
    assert!(session.current);

    // Exactly ten minutes after the last change is still inside the gap...
    at(&mut core, T0 + 11 * MIN);
    write_text(&mut core, page_id, "second", 2);
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 1);
    assert_eq!(history.versions[0].ended_at, Some(T0 + 11 * MIN));
    assert_eq!(history.versions[0].started_at, Some(T0));

    // ...one millisecond more is a new session.
    at(&mut core, T0 + 21 * MIN + 1);
    write_text(&mut core, page_id, "third", 3);
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 2);
    let (newest, oldest) = (&history.versions[0], &history.versions[1]);
    assert!(newest.current && !oldest.current);
    assert_eq!(
        (newest.started_at, newest.ended_at),
        (Some(T0 + 21 * MIN + 1), Some(T0 + 21 * MIN + 1))
    );
    assert_eq!(
        (oldest.started_at, oldest.ended_at),
        (Some(T0), Some(T0 + 11 * MIN))
    );
    // The older session is what the page read when it ended.
    assert_eq!(
        texts(&get(&mut core, page_id, &oldest.id).unwrap().content),
        ["first", "second"]
    );
    // The journal keeps the base of each session: the version before its first write.
    let rows = journal(&core, page_id);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].base_version, rows[1].version);
    assert_eq!(rows[1].base_version, "");
}

#[test]
fn an_import_that_brings_nothing_new_writes_no_row_and_moves_nothing() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    write_text(&mut core, page_id, "one", 1);
    let before = list(&mut core, page_id);
    // The editor sends everything the page has again.
    let everything = core.page_doc_export(page_id, None).unwrap();
    at(&mut core, T0 + 5 * MIN);
    core.page_doc_import(page_id, &everything).unwrap();
    core.page_doc_close(page_id).unwrap();
    assert_eq!(list(&mut core, page_id), before);
    assert_eq!(journal(&core, page_id).len(), 1);
}

#[test]
fn the_title_of_a_row_is_the_pages_title_when_its_session_last_wrote() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "First").id;
    at(&mut core, T0 + 20 * MIN);
    rename(&mut core, page_id, "Second");
    at(&mut core, T0 + 30 * MIN);
    write_text(&mut core, page_id, "x", 1);
    // A session that goes on takes the title of its latest write.
    at(&mut core, T0 + 31 * MIN);
    rename(&mut core, page_id, "Third");
    at(&mut core, T0 + 32 * MIN);
    write_text(&mut core, page_id, "y", 2);
    let history = list(&mut core, page_id);
    let titles: Vec<Option<&str>> = history
        .versions
        .iter()
        .map(|v| v.title.as_deref())
        .collect();
    assert_eq!(titles, [Some("Third"), Some("First")]);
}

#[test]
fn a_page_made_from_a_copy_has_its_own_history_from_its_creation() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let source = create_page(&mut core, space, None, "Source").id;
    write_text(&mut core, source, "content", 1);
    at(&mut core, T0 + MIN);
    let copy = match core
        .dispatch(Request::PageDuplicate { page_id: source })
        .unwrap()
    {
        Response::Page(info) => info.id,
        other => panic!("unexpected {other:?}"),
    };
    let history = list(&mut core, copy);
    assert_eq!(history.versions.len(), 1, "{history:?}");
    let only = &history.versions[0];
    assert!(only.current && only.author.as_ref().unwrap().this_device);
    assert_eq!(only.started_at, Some(T0 + MIN));
    assert_eq!(only.title.as_deref(), Some("Source (copy)"));
    assert_eq!(
        texts(&get(&mut core, copy, &only.id).unwrap().content),
        texts(&page_content(&mut core, copy))
    );
}

#[test]
fn an_imported_page_has_its_history_from_the_import_with_its_title() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let imported = crate::test_support_interop::run_import(
        &mut core,
        space,
        None,
        None,
        &[("Notes.md".to_owned(), "# Notes\n\nhello world".to_owned())],
        &[],
        100,
    );
    let page_id = imported.report.root_page_ids[0];
    let title = page(&mut core, page_id).title;
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 1, "{history:?}");
    let only = &history.versions[0];
    assert!(only.current && only.author.as_ref().unwrap().this_device);
    assert_eq!(only.started_at, Some(T0));
    assert_eq!(only.title.as_deref(), Some(title.as_str()));
    assert!(undated(&history).is_empty());
    // An edit after the session gap starts another row.
    at(&mut core, T0 + 30 * MIN);
    write_text(&mut core, page_id, "edited", 1);
    assert_eq!(list(&mut core, page_id).versions.len(), 2);
}

// ----- remote authors ----------------------------------------------------------------------------------

struct Team {
    alice: Dev,
    bob: Dev,
    page: PageId,
}

/// Alice shares a space with a page; Bob joins with `role`; both are settled.
fn team(role: MemberRole) -> Team {
    let server = server();
    let (mut alice, _) = Dev::first("alice", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    alice.configure();
    bob.configure();
    let space = create_space(&mut alice.core, "Project").id;
    let page = create_page(&mut alice.core, space, None, "Plan").id;
    alice.write(page, "alice wrote this");
    let feed = share(&mut alice, space, "Project", "Alice");
    add_member(&mut alice, &mut bob, feed, space, role, "Bob");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    Team { alice, bob, page }
}

#[test]
fn applied_entries_carry_their_writers_account_and_device() {
    let Team {
        mut alice,
        mut bob,
        page,
        ..
    } = team(MemberRole::Editor);
    let (alice_account, bob_account) = (alice.account(), bob.account());
    let (alice_device, bob_device) = (alice.core.device.device_id, bob.core.device.device_id);
    alice.core.drain_events();
    bob.core.drain_events();

    // Bob writes twice at one moment: one session, seen by Alice as the entries arrive.
    bob.write(page, "bob one");
    bob.write(page, "bob two");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    alice.cycle().unwrap();
    assert!(alice.texts(page).contains(&"bob two".to_owned()));
    let history = list(&mut alice.core, page);
    let newest = &history.versions[0];
    assert!(newest.current);
    let author = newest.author.as_ref().unwrap();
    assert_eq!(author.account_id, Some(bob_account));
    assert_eq!(author.device_id, Some(bob_device));
    assert!(!author.this_device);
    assert_eq!(newest.title.as_deref(), Some("Plan"));
    let alices: Vec<&PageVersion> = history
        .versions
        .iter()
        .filter(|v| v.author.as_ref().is_some_and(|a| a.this_device))
        .collect();
    assert!(!alices.is_empty(), "her own sessions are hers: {history:?}");
    assert!(alices.iter().all(
        |v| v.author.as_ref().unwrap().account_id == Some(alice_account)
            && v.author.as_ref().unwrap().device_id == Some(alice_device)
    ));
    let rows_of_bob = history
        .versions
        .iter()
        .filter(|v| {
            v.author
                .as_ref()
                .is_some_and(|a| a.account_id == Some(bob_account))
        })
        .count();
    assert_eq!(
        rows_of_bob, 1,
        "his two writes at one moment are one session here"
    );

    // The journal is local: Bob's own device holds his session as his.
    let bobs = list(&mut bob.core, page);
    let bobs_current = bobs.versions.iter().find(|v| v.current).unwrap();
    assert!(bobs_current.author.as_ref().unwrap().this_device);
    assert_eq!(
        bobs_current.author.as_ref().unwrap().account_id,
        Some(bob_account)
    );

    // Ten minutes and a bit later, another write of Bob's is another row on Alice's device.
    alice.clock.advance(11 * MIN);
    bob.write(page, "bob three");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    alice.cycle().unwrap();
    let after = list(&mut alice.core, page);
    let rows_of_bob = after
        .versions
        .iter()
        .filter(|v| {
            v.author
                .as_ref()
                .is_some_and(|a| a.account_id == Some(bob_account))
        })
        .count();
    assert_eq!(rows_of_bob, 2);
    // Another author never continues somebody's session: her own write starts a row too.
    alice.write(page, "alice again");
    let last = list(&mut alice.core, page);
    assert_eq!(last.versions.len(), after.versions.len() + 1);
    assert!(last.versions[0].author.as_ref().unwrap().this_device);
}

#[test]
fn a_viewer_may_list_and_read_the_history_but_not_restore() {
    let Team {
        mut alice,
        mut bob,
        page,
        ..
    } = team(MemberRole::Viewer);
    alice.clock.advance(30 * MIN);
    alice.write(page, "second line");
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    let history = list(&mut bob.core, page);
    assert!(!history.versions.is_empty());
    let oldest = history.versions.last().unwrap().clone();
    let content = get(&mut bob.core, page, &oldest.id).unwrap();
    assert_eq!(content.page_id, page);
    let err = restore(&mut bob.core, page, &oldest.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    assert_eq!(bob.texts(page), alice.texts(page), "nothing changed");
    // The Owner's device may.
    let alices = list(&mut alice.core, page);
    let first = alices.versions.last().unwrap().id.clone();
    restore(&mut alice.core, page, &first).unwrap();
}

// ----- the list: undated history, paging -------------------------------------------------------------

#[test]
fn history_before_the_journal_is_listed_undated_after_the_dated_versions() {
    let (_fx, mut core, space) = setup();
    let (page_id, versions) = page_with_sessions(&mut core, space, &["one", "two", "three"]);
    assert_eq!(versions.len(), 4);
    let newest = versions.last().unwrap().clone();

    // A database from before the journal: the history is there, the rows are not.
    core.db
        .journal_delete_docs(&[kb_types::DocId::for_page(page_id)])
        .unwrap();
    let wiped = list(&mut core, page_id);
    assert!(dated(&wiped).is_empty());
    assert_eq!(
        wiped.versions.len(),
        4,
        "one run per replica: creation and three editors"
    );
    for version in &wiped.versions {
        assert_eq!(version.kind, PageVersionKind::Edit);
        assert_eq!((version.started_at, version.ended_at), (None, None));
        assert!(version.author.is_none() && version.title.is_none());
    }
    assert!(
        wiped.versions[0].current,
        "the newest run is the current version"
    );
    assert_eq!(wiped.versions[0].id, newest.id);
    // Each one is a version like any other: its content reads as the sessions' did.
    let by_texts: Vec<Vec<String>> = wiped
        .versions
        .iter()
        .map(|v| texts(&get(&mut core, page_id, &v.id).unwrap().content))
        .collect();
    assert_eq!(by_texts[0], ["one", "two", "three"].to_vec());
    assert_eq!(by_texts[1], ["one", "two"].to_vec());
    assert_eq!(by_texts[2], ["one"].to_vec());
    assert!(by_texts[3].is_empty());
    // None has a title, whatever the page is called.
    assert_eq!(
        get(&mut core, page_id, &wiped.versions[1].id)
            .unwrap()
            .title,
        None
    );

    // The next session is dated and the old history stays undated behind it, newest first.
    at(&mut core, T0 + 10 * 60 * MIN);
    write_text(&mut core, page_id, "four", 99);
    let history = list(&mut core, page_id);
    assert_eq!(dated(&history).len(), 1);
    assert_eq!(undated(&history).len(), 4);
    assert!(history.versions[0].ended_at.is_some() && history.versions[0].current);
    assert_eq!(
        history.versions[1..]
            .iter()
            .map(|v| v.id.clone())
            .collect::<Vec<_>>(),
        ids(&wiped),
        "the earlier history is listed as it was, after the new session"
    );
    // The new session did not take the older runs with it (its base is the version before it).
    let row = &journal(&core, page_id)[0];
    assert_eq!(row.base_version, newest.id);
}

#[test]
fn a_current_state_no_version_lists_is_listed_first() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    // Two editors write at the same time from the same snapshot: the page has two heads.
    let snapshot = core.page_doc_open(page_id).unwrap();
    let a = editor_update(&snapshot, "from a", 1);
    let b = editor_update(&snapshot, "from b", 2);
    core.page_doc_import(page_id, &a).unwrap();
    core.page_doc_import(page_id, &b).unwrap();
    core.page_doc_close(page_id).unwrap();
    let with_journal = list(&mut core, page_id);
    assert_eq!(with_journal.versions.len(), 1, "one session");
    assert!(with_journal.versions[0].current);
    assert!(
        with_journal.versions[0].id.contains(','),
        "two heads: {with_journal:?}"
    );

    core.db
        .journal_delete_docs(&[kb_types::DocId::for_page(page_id)])
        .unwrap();
    let wiped = list(&mut core, page_id);
    assert_eq!(
        wiped.versions.len(),
        4,
        "the merged state and the three runs: {wiped:?}"
    );
    assert_eq!(wiped.versions[0].id, with_journal.versions[0].id);
    assert!(wiped.versions[0].current);
    assert_eq!(current(&wiped).id, wiped.versions[0].id);
    assert!(wiped.versions[1..].iter().all(|v| !v.current));
    // The state is restorable like the others (it is the page now: nothing happens).
    core.drain_events();
    restore(&mut core, page_id, &wiped.versions[0].id).unwrap();
    assert!(core.drain_events().is_empty());
}

#[test]
fn the_list_pages_by_limit_and_by_the_id_of_the_last_version() {
    let (_fx, mut core, space) = setup();
    let (page_id, versions) = page_with_sessions(&mut core, space, &["a", "b", "c", "d", "e"]);
    assert_eq!(versions.len(), 6);
    let all = ids(&list(&mut core, page_id));
    assert_eq!(all.len(), 6);

    let first = list_with(&mut core, page_id, None, Some(2)).unwrap();
    assert_eq!(ids(&first), all[..2]);
    assert!(first.more);
    let second = list_with(&mut core, page_id, Some(&all[1]), Some(2)).unwrap();
    assert_eq!(ids(&second), all[2..4]);
    assert!(second.more);
    let third = list_with(&mut core, page_id, Some(&all[3]), Some(2)).unwrap();
    assert_eq!(ids(&third), all[4..]);
    assert!(!third.more, "the list ends exactly with the page");
    let none = list_with(&mut core, page_id, Some(&all[5]), Some(2)).unwrap();
    assert!(none.versions.is_empty() && !none.more);

    // Defaults and limits: everything fits in the default; 0 is one; a huge limit is the maximum.
    let default = list(&mut core, page_id);
    assert_eq!(ids(&default), all);
    assert!(!default.more);
    assert_eq!(
        list_with(&mut core, page_id, None, Some(0))
            .unwrap()
            .versions
            .len(),
        1
    );
    assert_eq!(
        list_with(&mut core, page_id, None, Some(u32::MAX))
            .unwrap()
            .versions
            .len(),
        6
    );

    // `before` pages through the undated versions too (a page whose journal is gone).
    core.db
        .journal_delete_docs(&[kb_types::DocId::for_page(page_id)])
        .unwrap();
    let wiped = list(&mut core, page_id);
    assert_eq!(wiped.versions.len(), 6);
    let part = list_with(&mut core, page_id, Some(&wiped.versions[2].id), Some(2)).unwrap();
    assert_eq!(ids(&part), ids(&wiped)[3..5]);
    assert!(part.more);
}

#[test]
fn a_before_that_is_a_version_of_the_page_but_no_longer_listed_is_invalid() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let old = list(&mut core, page_id).versions[0].id.clone();
    at(&mut core, T0 + MIN);
    write_text(&mut core, page_id, "x", 1);
    let now = list(&mut core, page_id);
    assert_eq!(
        now.versions.len(),
        1,
        "the session went on: its row moved to a newer version"
    );
    assert!(!ids(&now).contains(&old));
    // The old id is still a version of the page's document: it can be read...
    assert!(get(&mut core, page_id, &old).is_ok());
    // ...but it is not where a listing can resume.
    let err = list_with(&mut core, page_id, Some(&old), None).unwrap_err();
    assert_eq!(field_of(&err), Some("before"));
}

// ----- content at a version ------------------------------------------------------------------------------

#[test]
fn the_content_at_a_version_is_what_the_page_read_then_with_its_title() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "First").id;
    at(&mut core, T0 + MIN);
    write_text(&mut core, page_id, "alpha", 1);
    let v1 = list(&mut core, page_id).versions[0].clone();
    at(&mut core, T0 + 20 * MIN);
    rename(&mut core, page_id, "Second");
    at(&mut core, T0 + 21 * MIN);
    write_text(&mut core, page_id, "beta", 2);
    let v2 = list(&mut core, page_id).versions[0].clone();
    assert_ne!(v1.id, v2.id);

    let old = get(&mut core, page_id, &v1.id).unwrap();
    assert_eq!(texts(&old.content), ["alpha"]);
    assert_eq!(old.title.as_deref(), Some("First"));
    assert_eq!(old.version, v1.id);
    let new = get(&mut core, page_id, &v2.id).unwrap();
    assert_eq!(texts(&new.content), ["alpha", "beta"]);
    assert_eq!(new.title.as_deref(), Some("Second"));
    // The current version is the page as `PageContentGet` answers it, in the same shape.
    assert_eq!(new.content, page_content(&mut core, page_id));
    assert_eq!(old.content.get("type").and_then(Value::as_str), Some("doc"));
    // Reading changes nothing: the page, its history and the events are as they were.
    let before = list(&mut core, page_id);
    core.drain_events();
    get(&mut core, page_id, &v1.id).unwrap();
    assert_eq!(list(&mut core, page_id), before);
    assert_eq!(page_texts(&mut core, page_id), ["alpha", "beta"]);
    assert!(core.drain_events().is_empty());
}

// ----- restore ---------------------------------------------------------------------------------------------

#[test]
fn restoring_makes_the_content_and_the_title_what_they_were_as_a_new_change() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "First").id;
    at(&mut core, T0 + MIN);
    write_text(&mut core, page_id, "alpha", 1);
    let v1 = list(&mut core, page_id).versions[0].clone();
    at(&mut core, T0 + 30 * MIN);
    rename(&mut core, page_id, "Second");
    write_text(&mut core, page_id, "beta", 2);
    let v2 = list(&mut core, page_id).versions[0].clone();
    core.drain_events();
    let updates_before = core
        .db
        .updates_after_snapshot(kb_types::DocId::for_page(page_id))
        .unwrap()
        .len();

    at(&mut core, T0 + 90 * MIN);
    let info = restore(&mut core, page_id, &v1.id).unwrap();
    assert_eq!(info.title, "First");
    assert_eq!(page(&mut core, page_id).title, "First");
    assert_eq!(page_texts(&mut core, page_id), ["alpha"]);
    // The same content as the version, exactly.
    assert_eq!(
        page_content(&mut core, page_id),
        get(&mut core, page_id, &v1.id).unwrap().content
    );

    // A new change after the old ones: stored (so it syncs), nothing rewritten.
    let updates_after = core
        .db
        .updates_after_snapshot(kb_types::DocId::for_page(page_id))
        .unwrap()
        .len();
    assert_eq!(updates_after, updates_before + 1);
    // The events: the content from the core, the title in the structure.
    let events = core.drain_events();
    assert!(events.contains(&Event::PageContentChanged {
        page_id,
        source: ChangeSource::Core
    }));
    assert!(events.contains(&Event::StructureChanged { space_id: space }));

    // The history: a restore row on top, dated now, this device, the title after the restore;
    // the versions before it are all still listed.
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 3);
    let top = &history.versions[0];
    assert_eq!(top.kind, PageVersionKind::Restore);
    assert!(top.current);
    assert_eq!(
        (top.started_at, top.ended_at),
        (Some(T0 + 90 * MIN), Some(T0 + 90 * MIN))
    );
    assert!(top.author.as_ref().unwrap().this_device);
    assert_eq!(top.title.as_deref(), Some("First"));
    assert_eq!(history.versions[1].id, v2.id);
    assert_eq!(history.versions[2].kind, PageVersionKind::Edit);
    assert_ne!(
        top.id, v1.id,
        "a restore is a new version, not the old one again"
    );
    // The page's last edit moved with it.
    assert_eq!(page(&mut core, page_id).updated_at, T0 + 90 * MIN);

    // Undo: restoring the version that was current before brings content and title back.
    at(&mut core, T0 + 91 * MIN);
    let back = restore(&mut core, page_id, &v2.id).unwrap();
    assert_eq!(back.title, "Second");
    assert_eq!(page_texts(&mut core, page_id), ["alpha", "beta"]);
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 4);
    assert_eq!(history.versions[0].kind, PageVersionKind::Restore);
    assert_eq!(history.versions[0].title.as_deref(), Some("Second"));

    // The restore row's own version restores like any other, with the title it recorded.
    let first_restore = history.versions[1].id.clone();
    at(&mut core, T0 + 92 * MIN);
    let again = restore(&mut core, page_id, &first_restore).unwrap();
    assert_eq!(again.title, "First");
    assert_eq!(page_texts(&mut core, page_id), ["alpha"]);
}

#[test]
fn restoring_the_version_the_page_has_changes_nothing() {
    let (_fx, mut core, space) = setup();
    let (page_id, _) = page_with_sessions(&mut core, space, &["one"]);
    let history = list(&mut core, page_id);
    let now = current(&history).id.clone();
    core.drain_events();
    let updates = core
        .db
        .updates_after_snapshot(kb_types::DocId::for_page(page_id))
        .unwrap()
        .len();
    at(&mut core, T0 + 100 * 60 * MIN);
    let info = restore(&mut core, page_id, &now).unwrap();
    assert_eq!(info.id, page_id);
    assert_eq!(list(&mut core, page_id), history, "no row, no new version");
    assert!(core.drain_events().is_empty());
    assert_eq!(
        core.db
            .updates_after_snapshot(kb_types::DocId::for_page(page_id))
            .unwrap()
            .len(),
        updates
    );
}

#[test]
fn an_edit_after_a_restore_starts_its_own_session_and_the_restore_never_continues_one() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    at(&mut core, T0 + MIN);
    write_text(&mut core, page_id, "one", 1);
    let created = list(&mut core, page_id).versions[0].clone();
    // Two minutes later, inside the session gap, a restore still starts a row of its own...
    at(&mut core, T0 + 3 * MIN);
    write_text(&mut core, page_id, "two", 2);
    at(&mut core, T0 + 4 * MIN);
    restore(&mut core, page_id, &created.id).unwrap();
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 2);
    assert_eq!(history.versions[0].kind, PageVersionKind::Restore);
    // ...and so does the edit that follows it.
    at(&mut core, T0 + 5 * MIN);
    write_text(&mut core, page_id, "three", 3);
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), 3);
    assert_eq!(history.versions[0].kind, PageVersionKind::Edit);
    assert_eq!(history.versions[1].kind, PageVersionKind::Restore);
    assert_eq!(page_texts(&mut core, page_id), ["one", "three"]);
}

#[test]
fn restoring_an_undated_version_restores_the_content_and_leaves_the_title() {
    let (_fx, mut core, space) = setup();
    let (page_id, _) = page_with_sessions(&mut core, space, &["one", "two"]);
    core.db
        .journal_delete_docs(&[kb_types::DocId::for_page(page_id)])
        .unwrap();
    rename(&mut core, page_id, "Renamed");
    core.drain_events();
    let history = list(&mut core, page_id);
    let only_one = history.versions[1].id.clone();
    assert_eq!(
        texts(&get(&mut core, page_id, &only_one).unwrap().content),
        ["one"]
    );
    restore(&mut core, page_id, &only_one).unwrap();
    assert_eq!(page_texts(&mut core, page_id), ["one"]);
    assert_eq!(
        page(&mut core, page_id).title,
        "Renamed",
        "an undated version knows no title"
    );
    // The restore is the first row of a journal that began late; the undated ones stay behind it.
    let after = list(&mut core, page_id);
    assert_eq!(dated(&after).len(), 1);
    assert_eq!(dated(&after)[0].kind, PageVersionKind::Restore);
    assert_eq!(undated(&after).len(), 3);
}

#[test]
fn restoring_announces_the_links_it_changes() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    let target = create_page(&mut core, space, None, "Target").id;
    write_text(&mut core, page_id, "no link", 1);
    let without = list(&mut core, page_id).versions[0].id.clone();
    at(&mut core, T0 + 30 * MIN);
    write_link(&mut core, page_id, target, 2);
    assert_eq!(
        core.db.links_from(page_id).unwrap().len(),
        1,
        "the link is indexed"
    );
    core.drain_events();

    at(&mut core, T0 + 60 * MIN);
    restore(&mut core, page_id, &without).unwrap();
    assert!(
        core.db.links_from(page_id).unwrap().is_empty(),
        "the links follow the content"
    );
    let events = core.drain_events();
    assert!(events.contains(&Event::BacklinksChanged { page_id: target }));
    assert!(events.contains(&Event::PageContentChanged {
        page_id,
        source: ChangeSource::Core
    }));
    // The text index follows too.
    assert_eq!(
        core.search_pages(
            "link",
            None,
            kb_types::SearchScope::default(),
            None,
            None,
            false
        )
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn a_trashed_page_keeps_its_history_readable_and_refuses_a_restore() {
    let (_fx, mut core, space) = setup();
    let (page_id, versions) = page_with_sessions(&mut core, space, &["one"]);
    core.dispatch(Request::PageTrash { page_id }).unwrap();
    core.drain_events();
    let history = list(&mut core, page_id);
    assert_eq!(history.versions.len(), versions.len());
    assert!(get(&mut core, page_id, &history.versions[1].id).is_ok());
    let err = restore(&mut core, page_id, &history.versions[1].id).unwrap_err();
    assert_eq!(field_of(&err), Some("pageId"));
    assert!(core.drain_events().is_empty());
    // Out of the trash it can be restored again.
    core.dispatch(Request::PageRestore { page_id }).unwrap();
    assert!(restore(&mut core, page_id, &history.versions[1].id).is_ok());
}

#[test]
fn a_restore_reaches_the_other_devices_like_an_edit() {
    let server = server();
    let (mut a, kit) = Dev::first("a", 1, &server);
    a.configure();
    let space = spaces(&mut a.core)[0].id;
    let page_id = create_page(&mut a.core, space, None, "Notes").id;
    a.write(page_id, "keep");
    a.cycle().unwrap();
    let mut b = Dev::from_backup("b", 2, &server, &mut a, &kit);
    b.configure();
    b.cycle().unwrap();
    let keep = list(&mut a.core, page_id).versions[0].id.clone();
    a.clock.advance(30 * MIN);
    a.write(page_id, "drop");
    a.cycle().unwrap();
    b.cycle().unwrap();
    assert_eq!(b.texts(page_id), ["keep", "drop"]);

    a.clock.advance(MIN);
    restore(&mut a.core, page_id, &keep).unwrap();
    assert_eq!(a.texts(page_id), ["keep"]);
    a.cycle().unwrap();
    b.cycle().unwrap();
    assert_eq!(b.texts(page_id), ["keep"], "the restore synced");
    // On B it is an entry of A's device, like any: the content is what it was, the history grew.
    let history = list(&mut b.core, page_id);
    assert!(history.versions[0].current);
    let author = history.versions[0].author.as_ref().unwrap();
    assert_eq!(author.device_id, Some(a.core.device.device_id));
    assert!(!author.this_device);
    assert_eq!(
        texts(&get(&mut b.core, page_id, &keep).unwrap().content),
        ["keep"],
        "the version it came from is there on the other device too (same id)"
    );
}

// ----- purge, backup, rebuild, compaction ----------------------------------------------------------------

#[test]
fn purging_a_page_deletes_its_journal_rows_and_only_its_rows() {
    let (_fx, mut core, space) = setup();
    at(&mut core, T0);
    let gone = create_page(&mut core, space, None, "Gone").id;
    let kept = create_page(&mut core, space, None, "Kept").id;
    let child = create_page(&mut core, space, Some(gone), "Child").id;
    write_text(&mut core, gone, "x", 1);
    write_text(&mut core, kept, "y", 2);
    assert!(!journal(&core, gone).is_empty());
    assert!(!journal(&core, child).is_empty());
    core.dispatch(Request::PageTrash { page_id: gone }).unwrap();
    // In the trash the history is still there.
    assert!(!journal(&core, gone).is_empty());
    core.dispatch(Request::PagePurge { page_id: gone }).unwrap();
    assert!(journal(&core, gone).is_empty());
    assert!(
        journal(&core, child).is_empty(),
        "the sub-page goes with it"
    );
    assert!(!journal(&core, kept).is_empty());
    assert_eq!(list(&mut core, kept).versions.len(), 1);
}

#[test]
fn the_collection_of_purged_documents_takes_the_journal_with_it() {
    let (fx, core, space) = setup();
    // A document of a page nobody knows (what is left when another device purged a page and this
    // one applied it) with a journal: collected at the next open, with its rows.
    let orphan = kb_types::PageId::from_bytes([0x55; 16]);
    let doc_id = kb_types::DocId::for_page(orphan);
    core.db
        .doc_insert(&kb_storage::DocRow {
            doc_id,
            kind: DocKindCode::Page,
            space_id: Some(space),
            created_at: T0,
            format_version: kb_doc::FORMAT_VERSION,
            min_reader_version: kb_doc::MIN_READER_VERSION,
        })
        .unwrap();
    core.db
        .journal_append(&kb_storage::NewJournalRow {
            doc_id,
            kind: kb_storage::JournalKind::Edit,
            started_at: T0,
            ended_at: T0,
            base_version: String::new(),
            version: "1@0".into(),
            device_id: None,
            account_id: None,
            this_device: true,
            title: None,
        })
        .unwrap();
    assert_eq!(journal(&core, orphan).len(), 1);
    drop(core);
    let core = fx.open(2, T0 + 1);
    assert!(core.db.doc_get(doc_id).unwrap().is_none());
    assert!(journal(&core, orphan).is_empty());
}

#[test]
fn the_index_rebuild_leaves_the_journal_alone() {
    let (_fx, mut core, space) = setup();
    let (page_id, _) = page_with_sessions(&mut core, space, &["one", "two"]);
    let rows = journal(&core, page_id);
    assert_eq!(rows.len(), 3);
    core.dispatch(Request::IndexRebuild { space_id: None })
        .unwrap();
    core.dispatch(Request::IndexRebuild {
        space_id: Some(space),
    })
    .unwrap();
    assert_eq!(journal(&core, page_id), rows);
}

#[test]
fn a_backup_does_not_carry_the_journal_and_the_restored_device_lists_the_history_undated() {
    let server = server();
    let (mut a, kit) = Dev::first("a", 1, &server);
    let space = spaces(&mut a.core)[0].id;
    let page_id = create_page(&mut a.core, space, None, "Notes").id;
    a.write(page_id, "one");
    a.clock.advance(30 * MIN);
    a.write(page_id, "two");
    let on_a = list(&mut a.core, page_id);
    assert_eq!(dated(&on_a).len(), 2);

    let mut b = Dev::from_backup("b", 2, &server, &mut a, &kit);
    assert!(journal(&b.core, page_id).is_empty());
    let on_b = list(&mut b.core, page_id);
    assert!(dated(&on_b).is_empty());
    assert!(!on_b.versions.is_empty());
    assert!(on_b.versions[0].current);
    // The ids are the same on both devices: a version is the document's, not the device's.
    let newest = on_a.versions[0].id.clone();
    assert!(ids(&on_b).contains(&newest));
    assert_eq!(
        get(&mut b.core, page_id, &newest).unwrap().content,
        get(&mut a.core, page_id, &newest).unwrap().content
    );
}

#[test]
fn the_history_survives_the_compaction_of_the_update_log() {
    let (fx, mut core, space) = setup();
    core.set_compaction_policy(CompactionPolicy {
        max_updates: 2,
        max_bytes: u64::MAX,
    });
    at(&mut core, T0);
    let page_id = create_page(&mut core, space, None, "Plan").id;
    for n in 0..6u64 {
        at(&mut core, T0 + (i64::try_from(n).unwrap() + 1) * 20 * MIN);
        write_text(&mut core, page_id, &format!("line {n}"), 20 + n);
    }
    let doc_id = kb_types::DocId::for_page(page_id);
    assert!(
        core.db.snapshot_get(doc_id).unwrap().is_some(),
        "the log was compacted into a snapshot"
    );
    let before = list(&mut core, page_id);
    assert_eq!(before.versions.len(), 7);
    let second = before.versions[5].clone();
    let at_second = texts(&get(&mut core, page_id, &second.id).unwrap().content);
    assert_eq!(at_second, ["line 0"]);
    drop(core);

    // A new process loads snapshot and updates: every version is still there and reads as it did.
    let mut core = fx.open(2, T0 + 24 * 60 * MIN);
    assert_eq!(list(&mut core, page_id), before);
    assert_eq!(
        texts(&get(&mut core, page_id, &second.id).unwrap().content),
        at_second
    );
    // Even with no journal at all, the compacted document still has all of its runs.
    core.db.journal_delete_docs(&[doc_id]).unwrap();
    let wiped = list(&mut core, page_id);
    assert_eq!(
        wiped.versions.len(),
        7,
        "creation and six editors: {wiped:?}"
    );
    assert_eq!(wiped.versions[0].id, before.versions[0].id);
    // And restoring across the compaction works.
    restore(&mut core, page_id, &wiped.versions[5].id).unwrap();
    assert_eq!(page_texts(&mut core, page_id), ["line 0"]);
}
