//! Milestone 3 tests: page documents, text and link indexes, search, backlinks,
//! export, through `dispatch` and the binary API.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

use kb_doc::{Doc, DocKind};
use kb_model::{BlockKind, PageDoc};
use kb_storage::{DocQueries, PageFirstImageQueries, TextIndexQueries, UpdateOrigin};
use kb_types::{
    AttachmentId, Backlink, BlockId, ChangeSource, DocId, ErrorCode, Event, PageId, Request,
    Response, SearchHit, SearchScope,
};

use crate::CompactionPolicy;
use crate::test_support::{Fixture, create_page, create_space, create_workspace, page, tree};

/// An editor replica: a second `PageDoc` fed from the core's snapshot, whose updates
/// are sent back with `page_doc_import`, exactly as the TypeScript editor does.
struct Editor {
    page_id: PageId,
    doc: PageDoc,
    last_sent: kb_doc::Version,
}

impl Editor {
    fn open(core: &mut crate::Core, page_id: PageId, peer: u64) -> Self {
        let snapshot = core.page_doc_open(page_id).unwrap();
        let blob = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &snapshot);
        let doc = Doc::load(DocKind::Page, peer, Some(&blob), &[]).unwrap();
        let doc = PageDoc::from_doc(doc).unwrap();
        let last_sent = doc.doc().version();
        Self {
            page_id,
            doc,
            last_sent,
        }
    }

    /// Sends everything since the last flush, like the UI's batched flush.
    fn flush(&mut self, core: &mut crate::Core) {
        let blob = self
            .doc
            .doc()
            .export_updates_since(Some(&self.last_sent))
            .unwrap()
            .expect("something to send");
        let (_, payload) = kb_doc::decode_blob(&blob).unwrap();
        core.page_doc_import(self.page_id, payload).unwrap();
        self.last_sent = self.doc.doc().version();
    }

    /// Pulls what the core has that this replica lacks.
    fn pull(&mut self, core: &mut crate::Core) -> usize {
        let since = self.doc.doc().version();
        let bytes = core.page_doc_export(self.page_id, Some(&since.0)).unwrap();
        if bytes.is_empty() {
            return 0;
        }
        let blob = kb_doc::encode_blob(kb_doc::BlobKind::Update, &bytes);
        self.doc.doc().import(&blob).unwrap();
        self.last_sent = self.doc.doc().version();
        bytes.len()
    }

    fn append_paragraph(&self, text: &str, id: BlockId) {
        let last = self.doc.block_ids().last().copied();
        self.doc
            .insert_block(last, BlockKind::Paragraph, text, id)
            .unwrap();
    }

    fn text(&self) -> String {
        self.doc
            .blocks()
            .iter()
            .flat_map(|b| b.inlines.iter())
            .filter_map(|i| match i {
                kb_model::Inline::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("|")
    }
}

fn block(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

fn search(core: &mut crate::Core, query: &str, scope: Option<SearchScope>) -> Vec<SearchHit> {
    match core
        .dispatch(Request::Search {
            query: query.into(),
            space_id: None,
            scope,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

fn backlinks(core: &mut crate::Core, page_id: PageId) -> Vec<Backlink> {
    match core.dispatch(Request::BacklinksGet { page_id }).unwrap() {
        Response::Backlinks(links) => links,
        other => panic!("unexpected {other:?}"),
    }
}

fn content(core: &mut crate::Core, page_id: PageId) -> serde_json::Value {
    match core.dispatch(Request::PageContentGet { page_id }).unwrap() {
        Response::PageContent(c) => {
            assert_eq!(c.page_id, page_id);
            c.content
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn markdown(core: &mut crate::Core, page_id: PageId) -> String {
    match core
        .dispatch(Request::PageExportMarkdown { page_id })
        .unwrap()
    {
        Response::Markdown(m) => m,
        other => panic!("unexpected {other:?}"),
    }
}

/// Appends a paragraph `paragraph_text` + `pageRef -> target` to the editor replica,
/// the shape the binding writes for "See [[Target]]".
fn insert_page_ref(editor: &Editor, paragraph_text: &str, target: PageId, id: BlockId) {
    let last = editor.doc.block_ids().last().copied();
    editor
        .doc
        .insert_block(last, BlockKind::Paragraph, paragraph_text, id)
        .unwrap();
    let ref_id = BlockId::from_random_bytes({
        let mut bytes = id.to_bytes();
        bytes[0] ^= 0xff;
        bytes
    });
    editor
        .doc
        .insert_page_ref(id, ref_id, target, None)
        .unwrap();
}

#[test]
fn open_creates_an_empty_document_and_reopen_keeps_it() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let p = create_page(&mut core, space, None, "Plan");
    core.drain_events();

    let snapshot = core.page_doc_open(p.id).unwrap();
    assert!(!snapshot.is_empty());
    let doc_id = DocId::for_page(p.id);
    let row = core.db.doc_get(doc_id).unwrap().expect("document row");
    assert_eq!(row.space_id, Some(space));
    assert_eq!(row.kind, kb_storage::DocKindCode::Page);

    let editor = Editor::open(&mut core, p.id, 0xA1);
    let blocks = editor.doc.blocks();
    assert_eq!(blocks.len(), 1, "one empty paragraph");
    assert_eq!(blocks[0].kind, "paragraph");
    assert!(editor.doc.is_empty());

    // Opening again returns the same content; closing is idempotent.
    let again = Editor::open(&mut core, p.id, 0xA2);
    assert_eq!(again.doc.to_portable(), editor.doc.to_portable());
    core.page_doc_close(p.id).unwrap();
    core.page_doc_close(p.id).unwrap();
    drop(core);

    let mut core = fx.open(2, 2_000);
    let reopened = Editor::open(&mut core, p.id, 0xA3);
    assert_eq!(reopened.doc.to_portable(), editor.doc.to_portable());
    assert!(core.drain_events().is_empty(), "opening emits nothing");
}

#[test]
fn unknown_pages_are_not_found_and_bad_updates_are_rejected() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let missing = PageId::from_random_bytes([9; 16]);
    assert_eq!(
        core.page_doc_open(missing).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        core.page_doc_import(missing, &[1, 2, 3]).unwrap_err().code,
        ErrorCode::NotFound
    );
    let space = tree_space(&mut core);
    let p = create_page(&mut core, space, None, "P");
    assert_eq!(
        core.page_doc_import(p.id, &[]).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    let garbage = core
        .page_doc_import(p.id, b"not a loro update")
        .unwrap_err();
    assert_eq!(garbage.code, ErrorCode::InvalidInput);
    assert_eq!(
        core.page_doc_export(p.id, Some(b"bad version"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    let huge = vec![0u8; crate::MAX_DOC_UPDATE_BYTES + 1];
    assert_eq!(
        core.page_doc_import(p.id, &huge).unwrap_err().code,
        ErrorCode::InvalidInput
    );
}

#[test]
fn editor_updates_are_persisted_indexed_and_announced() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let p = create_page(&mut core, space, None, "Meeting notes");
    core.drain_events();

    let mut editor = Editor::open(&mut core, p.id, 0xB1);
    editor.append_paragraph("Discussed the roadmap for Q4", block(1));
    editor.append_paragraph("Action: write the spec", block(2));
    editor.flush(&mut core);
    assert_eq!(
        core.drain_events(),
        vec![Event::PageContentChanged {
            page_id: p.id,
            source: ChangeSource::Editor
        }]
    );

    // Persisted as an update row and indexed per block.
    let stats = core.db.update_stats(DocId::for_page(p.id)).unwrap();
    assert_eq!(stats.count, 2, "creation + the editor's update");
    assert_eq!(
        core.db.fts_block_text(p.id, block(2)).unwrap().as_deref(),
        Some("Action: write the spec")
    );

    let hits = search(&mut core, "roadmap", None);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].page_id, p.id);
    assert_eq!(hits[0].block_id, Some(block(1)));
    assert_eq!(hits[0].title, "Meeting notes");
    assert!(!hits[0].highlights.is_empty());
    let (start, end) = (
        hits[0].highlights[0].start as usize,
        hits[0].highlights[0].end as usize,
    );
    let snippet: Vec<u16> = hits[0].snippet.encode_utf16().collect();
    assert_eq!(String::from_utf16(&snippet[start..end]).unwrap(), "roadmap");

    // Title hits come first and carry no block id.
    let hits = search(&mut core, "meeting", None);
    assert_eq!(hits[0].block_id, None);
    assert!(search(&mut core, "roadmap", Some(SearchScope::Titles)).is_empty());

    // Prefix match, trimmed query, validation.
    assert_eq!(search(&mut core, "  road ", None).len(), 1);
    let err = core
        .dispatch(Request::Search {
            query: "   ".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);

    // Reopen: the content is there, the editor replica resyncs from its version.
    drop(core);
    let mut core = fx.open(2, 10);
    let reopened = Editor::open(&mut core, p.id, 0xB2);
    assert_eq!(
        reopened.text(),
        "Discussed the roadmap for Q4|Action: write the spec"
    );
    assert_eq!(search(&mut core, "spec", None).len(), 1);
}

#[test]
fn export_gives_the_editor_what_it_lacks() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let p = create_page(&mut core, space, None, "P");

    let mut a = Editor::open(&mut core, p.id, 0xC1);
    let mut b = Editor::open(&mut core, p.id, 0xC2);
    a.append_paragraph("from a", block(1));
    a.flush(&mut core);
    assert!(b.pull(&mut core) > 0, "b lacked a's paragraph");
    assert_eq!(b.pull(&mut core), 0, "nothing new afterwards");
    assert_eq!(b.text(), a.text());

    b.append_paragraph("from b", block(2));
    b.flush(&mut core);
    a.pull(&mut core);
    assert_eq!(a.to_portable_text(), b.to_portable_text());

    // Everything when no version is given.
    let all = core.page_doc_export(p.id, None).unwrap();
    assert!(!all.is_empty());
    let empty_version = core.page_doc_export(p.id, Some(&[])).unwrap();
    assert_eq!(empty_version, all, "an empty version means everything");
}

impl Editor {
    fn to_portable_text(&self) -> String {
        self.doc.to_portable().to_string()
    }
}

#[test]
fn page_references_make_backlinks_that_follow_rename_trash_restore_and_purge() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let target = create_page(&mut core, space, None, "Target");
    let source = create_page(&mut core, space, None, "Source");
    core.drain_events();

    let mut editor = Editor::open(&mut core, source.id, 0xD1);
    insert_page_ref(&editor, "See ", target.id, block(1));
    editor.flush(&mut core);
    let events = core.drain_events();
    assert!(
        events.contains(&Event::BacklinksChanged { page_id: target.id }),
        "{events:?}"
    );

    let links = backlinks(&mut core, target.id);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].page_id, source.id);
    assert_eq!(links[0].title, "Source");
    assert_eq!(links[0].block_id, block(1));
    assert_eq!(
        links[0].text, "See Target",
        "the reference resolves to the title"
    );

    // The referenced title is searchable from the source page's text.
    assert!(
        search(&mut core, "target", None)
            .iter()
            .any(|h| h.page_id == source.id)
    );

    // Renaming the target updates the source's indexed text.
    core.dispatch(Request::PageRename {
        page_id: target.id,
        title: "Goal".into(),
    })
    .unwrap();
    assert_eq!(backlinks(&mut core, target.id)[0].text, "See Goal");
    assert!(
        search(&mut core, "goal", None)
            .iter()
            .any(|h| h.page_id == source.id)
    );

    // Trashing the source hides it from backlinks and search; restoring brings it back.
    core.drain_events();
    core.dispatch(Request::PageTrash { page_id: source.id })
        .unwrap();
    assert!(
        core.drain_events()
            .contains(&Event::BacklinksChanged { page_id: target.id })
    );
    assert!(backlinks(&mut core, target.id).is_empty());
    assert!(
        search(&mut core, "goal", None)
            .iter()
            .all(|h| h.page_id != source.id)
    );
    core.dispatch(Request::PageRestore { page_id: source.id })
        .unwrap();
    assert_eq!(backlinks(&mut core, target.id).len(), 1);

    // Purging the source removes its document, text and links for good.
    core.drain_events();
    core.dispatch(Request::PageTrash { page_id: source.id })
        .unwrap();
    core.dispatch(Request::PagePurge { page_id: source.id })
        .unwrap();
    assert!(
        core.drain_events()
            .contains(&Event::BacklinksChanged { page_id: target.id })
    );
    assert!(backlinks(&mut core, target.id).is_empty());
    assert!(core.db.links_from(source.id).unwrap().is_empty());
    assert!(
        core.db
            .fts_block_text(source.id, block(1))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        core.page_doc_open(source.id).unwrap_err().code,
        ErrorCode::NotFound
    );
    // The document row goes at the next open (undo could still bring the page back).
    assert!(
        core.db
            .doc_get(DocId::for_page(source.id))
            .unwrap()
            .is_some()
    );
    drop(core);
    let core = fx.open(2, 5);
    assert!(
        core.db
            .doc_get(DocId::for_page(source.id))
            .unwrap()
            .is_none()
    );
}

#[test]
fn content_get_and_markdown_export() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let target = create_page(&mut core, space, None, "Linked page");
    let p = create_page(&mut core, space, None, "Doc");

    let mut editor = Editor::open(&mut core, p.id, 0xE1);
    editor.append_paragraph("Hello", block(1));
    editor
        .doc
        .insert_block(Some(block(1)), BlockKind::Heading(2), "Section", block(2))
        .unwrap();
    insert_page_ref(&editor, "See ", target.id, block(3));
    editor.flush(&mut core);

    let json = content(&mut core, p.id);
    assert_eq!(json["type"], "doc");
    let kinds: Vec<&str> = json["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["paragraph", "paragraph", "heading", "paragraph"]);

    let md = markdown(&mut core, p.id);
    assert!(md.contains("Hello"), "{md}");
    assert!(md.contains("## Section"), "{md}");
    assert!(md.contains("[[Linked page]]"), "{md}");

    // Content of a page nobody opened yet: the empty document is created on demand.
    let fresh = create_page(&mut core, space, None, "Fresh");
    let json = content(&mut core, fresh.id);
    assert_eq!(json["content"].as_array().unwrap().len(), 1);
    assert!(
        core.db
            .doc_get(DocId::for_page(fresh.id))
            .unwrap()
            .is_some()
    );
}

#[test]
fn index_rebuild_matches_incremental_indexing() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let other = create_space(&mut core, "Work");
    let target = create_page(&mut core, space, None, "Target");
    let a = create_page(&mut core, space, None, "Alpha");
    let b = create_page(&mut core, other.id, None, "Beta");
    for (page, text, n) in [(a.id, "alpha text", 1u8), (b.id, "beta text", 2u8)] {
        let mut editor = Editor::open(&mut core, page, 0xF0 + u64::from(n));
        editor.append_paragraph(text, block(n));
        insert_page_ref(&editor, "ref ", target.id, block(0x10 + n));
        editor.flush(&mut core);
        core.page_doc_close(page).unwrap();
    }
    let before_hits = search(&mut core, "text", None);
    let before_links = backlinks(&mut core, target.id);
    assert_eq!(before_hits.len(), 2);
    assert_eq!(before_links.len(), 2);

    // Damage the derived data, then rebuild one space and then everything.
    core.db.fts_clear().unwrap();
    core.db.links_clear().unwrap();
    assert!(search(&mut core, "text", None).is_empty());
    core.dispatch(Request::IndexRebuild {
        space_id: Some(other.id),
    })
    .unwrap();
    assert_eq!(
        search(&mut core, "text", None).len(),
        1,
        "only Work was rebuilt"
    );
    core.dispatch(Request::IndexRebuild { space_id: None })
        .unwrap();
    assert_eq!(search(&mut core, "text", None), before_hits);
    assert_eq!(backlinks(&mut core, target.id), before_links);

    // Space filter.
    let scoped = match core
        .dispatch(Request::Search {
            query: "text".into(),
            space_id: Some(other.id),
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].space_id, other.id);

    // The open-time check repairs a space whose rows are missing.
    core.db.fts_clear().unwrap();
    drop(core);
    let mut core = fx.open(2, 1);
    assert_eq!(search(&mut core, "text", None).len(), 2);
}

#[test]
fn undo_of_a_purge_brings_the_content_back_into_the_index() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let p = create_page(&mut core, space, None, "Keep");
    let mut editor = Editor::open(&mut core, p.id, 0xAB);
    editor.append_paragraph("precious words", block(1));
    editor.flush(&mut core);
    core.page_doc_close(p.id).unwrap();

    core.dispatch(Request::PageTrash { page_id: p.id }).unwrap();
    core.dispatch(Request::PagePurge { page_id: p.id }).unwrap();
    assert!(search(&mut core, "precious", None).is_empty());

    // Undo the purge (and the trash): the page and its text are back.
    core.dispatch(Request::StructureUndo { space_id: space })
        .unwrap();
    core.dispatch(Request::StructureUndo { space_id: space })
        .unwrap();
    assert_eq!(page(&mut core, p.id).trashed_at, None);
    assert_eq!(search(&mut core, "precious", None).len(), 1);
    let reopened = Editor::open(&mut core, p.id, 0xAC);
    assert_eq!(reopened.text(), "precious words");
}

#[test]
fn page_documents_are_compacted() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    core.set_compaction_policy(CompactionPolicy {
        max_updates: 3,
        max_bytes: u64::MAX,
    });
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let p = create_page(&mut core, space, None, "P");
    let mut editor = Editor::open(&mut core, p.id, 0xCC);
    for n in 1..=6u8 {
        editor.append_paragraph(&format!("line {n}"), block(n));
        editor.flush(&mut core);
    }
    let doc_id = DocId::for_page(p.id);
    assert!(
        core.db.snapshot_get(doc_id).unwrap().is_some(),
        "a snapshot was written"
    );
    assert!(core.db.update_stats(doc_id).unwrap().count <= 3);
    drop(core);
    let mut core = fx.open(2, 1);
    let reopened = Editor::open(&mut core, p.id, 0xCD);
    assert_eq!(reopened.doc.blocks().len(), 7);
}

#[test]
fn a_one_megabyte_page_opens_within_budget() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let p = create_page(&mut core, space, None, "Big");
    let mut editor = Editor::open(&mut core, p.id, 0xDD);
    // 2,000 paragraphs of 500 characters, each one different, so the engine's
    // encoding cannot collapse them into a few repeated chunks.
    let words = [
        "lorem", "ipsum", "dolor", "amet", "elit", "sed", "do", "magna",
    ];
    let mut total_chars = 0usize;
    for n in 0..2_000u16 {
        let mut bytes = [0u8; 16];
        bytes[..2].copy_from_slice(&n.to_le_bytes());
        bytes[15] = 1;
        let mut line = format!("{n:04} ");
        let mut k = usize::from(n);
        while line.len() < 500 {
            k = k.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            line.push_str(words[(k >> 16) % words.len()]);
            line.push(' ');
        }
        line.truncate(500);
        total_chars += line.len();
        editor.append_paragraph(&line, BlockId::from_random_bytes(bytes));
    }
    assert!(total_chars >= 1_000_000);
    editor.flush(&mut core);
    core.page_doc_close(p.id).unwrap();
    drop(core);

    let mut core = fx.open(2, 1);
    let started = Instant::now();
    let snapshot = core.page_doc_open(p.id).unwrap();
    let elapsed = started.elapsed();
    let budget_ms: u128 = if cfg!(debug_assertions) { 2_000 } else { 100 };
    assert!(
        elapsed.as_millis() < budget_ms,
        "opening took {elapsed:?}, budget {budget_ms} ms"
    );
    assert!(snapshot.len() > 100_000, "{} bytes", snapshot.len());
    let reopened = Editor::open(&mut core, p.id, 0xDE);
    assert_eq!(reopened.doc.blocks().len(), 2_001);
    let chars: usize = reopened
        .doc
        .blocks()
        .iter()
        .flat_map(|b| b.inlines.iter())
        .map(|i| match i {
            kb_model::Inline::Text(t) => t.chars().count(),
            _ => 0,
        })
        .sum();
    assert!(chars >= 1_000_000, "{chars} characters");
}

fn tree_space(core: &mut crate::Core) -> kb_types::SpaceId {
    let list = crate::test_support::spaces(core);
    let space = list[0].id;
    let _ = tree(core, space);
    space
}

#[test]
fn a_page_link_card_is_a_backlink_and_is_indexed_like_a_chip() {
    // ADR-0016, amendment of 2026-10-06: a `pageLink` block counts as a link of its own
    // block, and reads as the title of the page it links to.
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let target = create_page(&mut core, space, None, "Target");
    let source = create_page(&mut core, space, None, "Source");
    core.page_doc_open(source.id).unwrap();
    core.drain_events();

    // The editor writes the card; its replica reaches the core as one update.
    let card = serde_json::json!({
        "type": "doc",
        "content": [{
            "type": "pageLink",
            "attrs": {
                "id": block(7).to_string(),
                "pageId": target.id.to_string(),
                "blockId": block(9).to_string()
            }
        }]
    });
    let editor = PageDoc::from_portable(Doc::new(DocKind::Page, 0xF1), &card).unwrap();
    let blob = editor.doc().export_updates_since(None).unwrap().unwrap();
    let (_, payload) = kb_doc::decode_blob(&blob).unwrap();
    core.page_doc_import(source.id, payload).unwrap();
    assert!(
        core.drain_events()
            .contains(&Event::BacklinksChanged { page_id: target.id })
    );

    let links = backlinks(&mut core, target.id);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].page_id, source.id);
    assert_eq!(links[0].block_id, block(7), "the card's own block");
    assert_eq!(
        links[0].text, "Target",
        "a card reads as the title of its page"
    );
    let rows = core.db.links_from(source.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].from_block, block(7));
    assert_eq!(rows[0].to_page, target.id);
    assert_eq!(rows[0].to_block, Some(block(9)));

    // Its title is searchable from the source page, and follows a rename.
    assert!(
        search(&mut core, "target", None)
            .iter()
            .any(|h| h.page_id == source.id)
    );
    core.dispatch(Request::PageRename {
        page_id: target.id,
        title: "Goal".into(),
    })
    .unwrap();
    assert_eq!(backlinks(&mut core, target.id)[0].text, "Goal");
    assert!(
        search(&mut core, "goal", None)
            .iter()
            .any(|h| h.page_id == source.id)
    );
    // The empty paragraph the core created and the card merged in some order: the card is a
    // line of its own either way.
    let md = markdown(&mut core, source.id);
    assert!(md.lines().any(|line| line == "[[Goal]]"), "{md:?}");

    // Trashing the source hides the link, like any other.
    core.dispatch(Request::PageTrash { page_id: source.id })
        .unwrap();
    assert!(backlinks(&mut core, target.id).is_empty());
}

#[test]
fn an_inline_equation_is_indexed_by_its_latex_and_exported_as_dollars() {
    // ADR-0016, amendment of 2026-10-07: a `mathInline` reads as its LaTeX source in the text
    // index and in the text of a backlink (`PageDoc::blocks` folds it into the run around it),
    // and the Markdown export writes `$latex$`; an equation with no source reads as nothing.
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let target = create_page(&mut core, space, None, "Target");
    let source = create_page(&mut core, space, None, "Source");
    core.page_doc_open(source.id).unwrap();
    core.drain_events();

    let page_content = serde_json::json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "attrs": { "id": block(7).to_string() },
            "content": [
                { "type": "text", "text": "See " },
                { "type": "pageRef", "attrs": { "id": block(8).to_string(), "pageId": target.id.to_string() } },
                { "type": "text", "text": " for " },
                { "type": "mathInline", "attrs": { "id": block(9).to_string(), "latex": "E=mc^2" } },
                { "type": "text", "text": " and " },
                { "type": "mathInline", "attrs": { "id": block(10).to_string(), "latex": "" } },
                { "type": "mathInline", "attrs": { "id": block(11).to_string(), "latex": "x2" } }
            ]
        }]
    });
    let editor = PageDoc::from_portable(Doc::new(DocKind::Page, 0xF3), &page_content).unwrap();
    let blob = editor.doc().export_updates_since(None).unwrap().unwrap();
    let (_, payload) = kb_doc::decode_blob(&blob).unwrap();
    core.page_doc_import(source.id, payload).unwrap();

    // The indexed text holds the LaTeX as written, in one run with the text around it.
    assert_eq!(
        core.db
            .fts_block_text(source.id, block(7))
            .unwrap()
            .as_deref(),
        Some("See Target for E=mc^2 and x2")
    );
    // The formula, or a part of it, finds the page.
    for query in ["mc", "x2"] {
        assert!(
            search(&mut core, query, None)
                .iter()
                .any(|h| h.page_id == source.id),
            "{query}"
        );
    }
    // The backlink shows the sentence with the formula in it; the formula is not a link.
    let links = backlinks(&mut core, target.id);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].block_id, block(7));
    assert_eq!(links[0].text, "See Target for E=mc^2 and x2");
    assert_eq!(core.db.links_from(source.id).unwrap().len(), 1);
    // Markdown is `$latex$`, and nothing for the empty equation.
    let md = markdown(&mut core, source.id);
    assert!(
        md.lines()
            .any(|line| line == "See [[Target]] for $E=mc^2$ and $x2$"),
        "{md:?}"
    );
}

#[test]
fn a_date_mention_is_indexed_and_exported_as_its_iso_text() {
    // ADR-0016, amendment of 2026-10-07: a `dateMention` reads as its ISO text, in the text
    // index, in the text of a backlink and in the Markdown export, like the title of a page
    // reference does.
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let target = create_page(&mut core, space, None, "Target");
    let source = create_page(&mut core, space, None, "Source");
    core.page_doc_open(source.id).unwrap();
    core.drain_events();

    let page_content = serde_json::json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "attrs": { "id": block(7).to_string() },
            "content": [
                { "type": "text", "text": "See " },
                { "type": "pageRef", "attrs": { "id": block(8).to_string(), "pageId": target.id.to_string() } },
                { "type": "text", "text": " on " },
                { "type": "dateMention", "attrs": { "id": block(9).to_string(), "date": "2026-10-07", "time": "14:30" } },
                { "type": "text", "text": " and " },
                { "type": "dateMention", "attrs": { "id": block(10).to_string(), "date": "2026-11-02" } }
            ]
        }]
    });
    let editor = PageDoc::from_portable(Doc::new(DocKind::Page, 0xF2), &page_content).unwrap();
    let blob = editor.doc().export_updates_since(None).unwrap().unwrap();
    let (_, payload) = kb_doc::decode_blob(&blob).unwrap();
    core.page_doc_import(source.id, payload).unwrap();

    // The indexed text holds the dates as typed (one run with the text around them).
    assert_eq!(
        core.db
            .fts_block_text(source.id, block(7))
            .unwrap()
            .as_deref(),
        Some("See Target on 2026-10-07 14:30 and 2026-11-02")
    );
    // A date, or a part of it, finds the page.
    for query in [
        "2026-10-07",
        "2026-10-07 14:30",
        "14:30",
        "2026-11-02",
        "2026",
    ] {
        assert!(
            search(&mut core, query, None)
                .iter()
                .any(|h| h.page_id == source.id),
            "{query}"
        );
    }
    assert!(
        search(&mut core, "2026-12-25", None)
            .iter()
            .all(|h| h.page_id != source.id)
    );
    // The backlink shows the sentence with the dates in it; the dates are not links.
    let links = backlinks(&mut core, target.id);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].block_id, block(7));
    assert_eq!(
        links[0].text,
        "See Target on 2026-10-07 14:30 and 2026-11-02"
    );
    assert_eq!(core.db.links_from(source.id).unwrap().len(), 1);
    // Markdown is the same text.
    let md = markdown(&mut core, source.id);
    assert!(
        md.lines()
            .any(|line| line == "See [[Target]] on 2026-10-07 14:30 and 2026-11-02"),
        "{md:?}"
    );
}

#[test]
fn an_image_caption_is_indexed_with_its_alt_text_and_exported_as_the_link_title() {
    // ADR-0016, amendment of 2026-10-09 (ADR-0028): an `image` reads as its caption then its alt
    // text, so the text index finds a word that only the caption holds, and the Markdown export
    // writes the caption as the link title; the width and the alignment are neither.
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let source = create_page(&mut core, space, None, "Photos");
    core.page_doc_open(source.id).unwrap();
    core.drain_events();

    let attachment = kb_types::AttachmentId::from_random_bytes([0xa7; 16]).to_string();
    let page_content = serde_json::json!({
        "type": "doc",
        "content": [
            { "type": "image", "attrs": {
                "id": block(7).to_string(), "attachmentId": attachment, "alt": "a cat",
                "caption": "Sunrise over Lisbon", "widthPercent": 40, "align": "right"
            } },
            { "type": "image", "attrs": {
                "id": block(8).to_string(), "attachmentId": attachment, "alt": "a dog",
                "caption": "Quoted \"Porto\" at C:\\harbour", "width": 320
            } },
            { "type": "image", "attrs": {
                "id": block(9).to_string(), "attachmentId": attachment, "alt": "a bird"
            } }
        ]
    });
    let editor = PageDoc::from_portable(Doc::new(DocKind::Page, 0xF4), &page_content).unwrap();
    assert_eq!(editor.check_invariants(), Vec::<String>::new());
    let blob = editor.doc().export_updates_since(None).unwrap().unwrap();
    let (_, payload) = kb_doc::decode_blob(&blob).unwrap();
    core.page_doc_import(source.id, payload).unwrap();

    // The indexed text is the caption, a space, the alt text.
    assert_eq!(
        core.db
            .fts_block_text(source.id, block(7))
            .unwrap()
            .as_deref(),
        Some("Sunrise over Lisbon a cat")
    );
    assert_eq!(
        core.db
            .fts_block_text(source.id, block(9))
            .unwrap()
            .as_deref(),
        Some("a bird")
    );
    // A word that appears only in a caption finds the page, and so do the alt texts.
    for query in ["Lisbon", "Sunrise over", "Porto", "cat", "bird"] {
        assert!(
            search(&mut core, query, None)
                .iter()
                .any(|h| h.page_id == source.id),
            "{query}"
        );
    }
    // The layout is not text.
    for query in ["right", "40"] {
        assert!(
            search(&mut core, query, None)
                .iter()
                .all(|h| h.page_id != source.id),
            "{query}"
        );
    }
    // The export has the caption as the link title, escaped, and no layout.
    let md = markdown(&mut core, source.id);
    assert_eq!(
        md,
        format!(
            "![a cat](attachment://{attachment} \"Sunrise over Lisbon\")\n\n\
             ![a dog](attachment://{attachment} \"Quoted \\\"Porto\\\" at C:\\\\harbour\")\n\n\
             ![a bird](attachment://{attachment})\n"
        )
    );
}

// ----- the first picture of each page (ADR-0054, schema v19) ----------------------------------

fn attachment(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([n; 16])
}

/// Appends portable blocks to a page's document and persists them, like an import or the MCP server does.
fn append(core: &mut crate::Core, page_id: PageId, blocks: serde_json::Value) {
    core.open_page(page_id)
        .unwrap()
        .model
        .append_portable(&serde_json::json!({ "type": "doc", "content": blocks }))
        .unwrap();
    core.persist_page(page_id, None, UpdateOrigin::Core, ChangeSource::Core)
        .unwrap();
    core.page_doc_close(page_id).unwrap();
}

fn image(id: u8, attachment_n: u8) -> serde_json::Value {
    serde_json::json!({ "type": "image", "attrs": {
        "id": block(id).to_string(),
        "attachmentId": attachment(attachment_n).to_string(),
        "alt": ""
    }})
}

fn file(id: u8, attachment_n: u8) -> serde_json::Value {
    serde_json::json!({ "type": "file", "attrs": {
        "id": block(id).to_string(),
        "attachmentId": attachment(attachment_n).to_string()
    }})
}

fn gallery(id: u8, attachments: &[u8]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = attachments
        .iter()
        .map(|n| serde_json::json!({ "attachmentId": attachment(*n).to_string() }))
        .collect();
    serde_json::json!({ "type": "gallery", "attrs": { "id": block(id).to_string(), "items": items } })
}

fn ask_first_images(
    core: &mut crate::Core,
    space_id: kb_types::SpaceId,
    pages: &[PageId],
) -> Result<Vec<(PageId, AttachmentId)>, kb_types::ApiError> {
    core.dispatch(Request::PageFirstImages {
        space_id,
        page_ids: pages.to_vec(),
    })
    .map(|response| match response {
        Response::PageFirstImages(list) => list
            .into_iter()
            .map(|found| (found.page_id, found.attachment_id))
            .collect(),
        other => panic!("unexpected {other:?}"),
    })
}

/// The row of a page in the derived table: `None` when it has none, else its attachment.
fn first_image_row(core: &crate::Core, page_id: PageId) -> Option<Option<AttachmentId>> {
    core.db
        .first_images_get(&[page_id])
        .unwrap()
        .into_iter()
        .next()
        .map(|(_, attachment)| attachment)
}

#[test]
fn the_first_picture_is_indexed_with_every_write_and_answered_in_the_order_asked() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let work = create_space(&mut core, "Work");
    let plain = create_page(&mut core, space, None, "Plain");
    let photo = create_page(&mut core, space, None, "Photo");
    let album = create_page(&mut core, space, None, "Album");
    let deep = create_page(&mut core, space, None, "Deep");
    let never = create_page(&mut core, space, None, "Never opened");
    let elsewhere = create_page(&mut core, work.id, None, "Elsewhere");
    append(&mut core, plain.id, serde_json::json!([para_block(30)]));
    append(
        &mut core,
        photo.id,
        serde_json::json!([image(1, 1), image(2, 2)]),
    );
    // A gallery comes first and its first item counts; the image after it does not.
    append(
        &mut core,
        album.id,
        serde_json::json!([gallery(3, &[6, 7]), image(4, 8)]),
    );
    // A file is no picture; the image in the callout comes before the later one.
    append(
        &mut core,
        deep.id,
        serde_json::json!([
            file(5, 3),
            { "type": "callout", "attrs": { "id": block(6).to_string() }, "content": [image(7, 4)] },
            image(8, 5)
        ]),
    );
    append(&mut core, elsewhere.id, serde_json::json!([image(9, 14)]));

    // Every write rewrote the page's row, in the same transaction as its other derived rows.
    assert_eq!(first_image_row(&core, photo.id), Some(Some(attachment(1))));
    assert_eq!(first_image_row(&core, album.id), Some(Some(attachment(6))));
    assert_eq!(first_image_row(&core, deep.id), Some(Some(attachment(4))));
    assert_eq!(
        first_image_row(&core, elsewhere.id),
        Some(Some(attachment(14)))
    );
    assert_eq!(first_image_row(&core, plain.id), Some(None));
    assert_eq!(
        first_image_row(&core, never.id),
        None,
        "never written, not scanned"
    );

    // The order asked, each page once; no picture, another space and an unknown page are left out.
    let unknown = PageId::from_random_bytes([0xEE; 16]);
    let asked = [
        album.id,
        plain.id,
        never.id,
        elsewhere.id,
        photo.id,
        unknown,
        album.id,
        deep.id,
    ];
    assert_eq!(
        ask_first_images(&mut core, space, &asked).unwrap(),
        [
            (album.id, attachment(6)),
            (photo.id, attachment(1)),
            (deep.id, attachment(4))
        ]
    );
    assert_eq!(
        ask_first_images(&mut core, work.id, &asked).unwrap(),
        [(elsewhere.id, attachment(14))]
    );
    // The page nobody wrote was scanned by the request, and now has a row that says it has no picture.
    assert_eq!(first_image_row(&core, never.id), Some(None));
    assert!(ask_first_images(&mut core, space, &[]).unwrap().is_empty());

    // An edit changes the answer: a picture added in front, then every picture removed.
    core.open_page(photo.id)
        .unwrap()
        .model
        .insert_block(
            None,
            BlockKind::Image {
                attachment_id: attachment(9),
                width: None,
            },
            "",
            block(20),
        )
        .unwrap();
    core.persist_page(photo.id, None, UpdateOrigin::Local, ChangeSource::Editor)
        .unwrap();
    assert_eq!(
        ask_first_images(&mut core, space, &[photo.id]).unwrap(),
        [(photo.id, attachment(9))]
    );
    for id in [20, 1, 2] {
        core.open_page(photo.id)
            .unwrap()
            .model
            .delete_block(block(id))
            .unwrap();
    }
    core.persist_page(photo.id, None, UpdateOrigin::Local, ChangeSource::Editor)
        .unwrap();
    assert!(
        ask_first_images(&mut core, space, &[photo.id])
            .unwrap()
            .is_empty()
    );
    assert_eq!(first_image_row(&core, photo.id), Some(None));
}

#[test]
fn pages_without_a_row_are_scanned_by_the_request_and_the_index_is_rebuilt_from_the_documents() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let other = create_space(&mut core, "Work");
    let a = create_page(&mut core, space, None, "A");
    let b = create_page(&mut core, space, None, "B");
    let c = create_page(&mut core, other.id, None, "C");
    append(&mut core, a.id, serde_json::json!([image(1, 1)]));
    append(&mut core, b.id, serde_json::json!([para_block(2)]));
    append(&mut core, c.id, serde_json::json!([gallery(3, &[3])]));

    // A database from before schema v19: no rows at all. The request scans what it is asked about,
    // from the documents, once, and keeps the answer.
    core.db.first_images_clear().unwrap();
    assert_eq!(first_image_row(&core, a.id), None);
    assert_eq!(
        ask_first_images(&mut core, space, &[a.id, b.id]).unwrap(),
        [(a.id, attachment(1))]
    );
    assert_eq!(first_image_row(&core, a.id), Some(Some(attachment(1))));
    assert_eq!(first_image_row(&core, b.id), Some(None));
    assert_eq!(
        first_image_row(&core, c.id),
        None,
        "not asked about, not scanned"
    );

    // The scan reads the open document as well as a stored one.
    core.db.first_images_clear().unwrap();
    core.open_page(a.id).unwrap();
    assert_eq!(
        ask_first_images(&mut core, space, &[a.id]).unwrap(),
        [(a.id, attachment(1))]
    );
    assert_eq!(first_image_row(&core, a.id), Some(Some(attachment(1))));
    core.page_doc_close(a.id).unwrap();

    // `IndexRebuild` of one space rewrites the rows of its pages, of everything rewrites all.
    core.db.first_images_clear().unwrap();
    core.dispatch(Request::IndexRebuild {
        space_id: Some(other.id),
    })
    .unwrap();
    assert_eq!(first_image_row(&core, c.id), Some(Some(attachment(3))));
    assert_eq!(
        first_image_row(&core, a.id),
        None,
        "the other space was not rebuilt"
    );
    core.dispatch(Request::IndexRebuild { space_id: None })
        .unwrap();
    assert_eq!(first_image_row(&core, a.id), Some(Some(attachment(1))));
    assert_eq!(first_image_row(&core, b.id), Some(None));
    assert_eq!(first_image_row(&core, c.id), Some(Some(attachment(3))));

    // A stale row (a document changed behind the index's back) is what a rebuild repairs.
    core.db
        .first_images_set(space, &[(a.id, Some(attachment(77)))])
        .unwrap();
    core.dispatch(Request::IndexRebuild {
        space_id: Some(space),
    })
    .unwrap();
    assert_eq!(first_image_row(&core, a.id), Some(Some(attachment(1))));
}

fn para_block(id: u8) -> serde_json::Value {
    serde_json::json!({ "type": "paragraph", "attrs": { "id": block(id).to_string() } })
}

#[test]
fn the_request_checks_the_space_and_the_number_of_pages_and_answers_trashed_pages() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = tree_space(&mut core);
    let photo = create_page(&mut core, space, None, "Photo");
    let gone = create_page(&mut core, space, None, "Gone");
    let child = create_page(&mut core, space, Some(photo.id), "Child");
    append(&mut core, photo.id, serde_json::json!([image(1, 1)]));
    append(&mut core, gone.id, serde_json::json!([image(2, 2)]));
    append(&mut core, child.id, serde_json::json!([image(3, 3)]));

    // An unknown space is NotFound, more than 500 pages InvalidInput (field `pageIds`), 500 are fine.
    let unknown_space = kb_types::SpaceId::from_random_bytes([0xDD; 16]);
    assert_eq!(
        ask_first_images(&mut core, unknown_space, &[photo.id])
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let many: Vec<PageId> = (0..501_u16)
        .map(|n| {
            let [high, low] = n.to_be_bytes();
            PageId::from_random_bytes([high, low, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7])
        })
        .collect();
    let refused = ask_first_images(&mut core, space, &many).unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidInput);
    assert_eq!(
        refused.details,
        Some(serde_json::json!({ "field": "pageIds" }))
    );
    assert!(
        ask_first_images(&mut core, space, &many[..500])
            .unwrap()
            .is_empty()
    );

    // A trashed page, and the sub-pages that go with it, are still answered.
    core.dispatch(Request::PageTrash { page_id: photo.id })
        .unwrap();
    assert_eq!(
        ask_first_images(&mut core, space, &[child.id, photo.id, gone.id]).unwrap(),
        [
            (child.id, attachment(3)),
            (photo.id, attachment(1)),
            (gone.id, attachment(2))
        ]
    );

    // A purged page is no page any more, and its row goes with it.
    core.dispatch(Request::PageTrash { page_id: gone.id })
        .unwrap();
    core.dispatch(Request::PagePurge { page_id: gone.id })
        .unwrap();
    assert_eq!(first_image_row(&core, gone.id), None);
    assert_eq!(
        ask_first_images(&mut core, space, &[gone.id, gone.id, photo.id]).unwrap(),
        [(photo.id, attachment(1))]
    );
}
