//! Milestone 5 tests: attachment uploads and reads, tampering, references, garbage
//! collection, deletion, backups with chunks, index repair.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_doc::{Doc, DocKind};
use kb_keys::KeySubject;
use kb_model::{BlockKind, PageDoc};
use kb_storage::{AttachmentQueries, BlobStore};
use kb_types::attachments::{CHUNK_SIZE, RETENTION_SETTING};
use kb_types::{
    AttachmentId, AttachmentInfo, BlockId, ByteRange, ChunkId, ErrorCode, Event, PageId, Request,
    Response, SpaceId,
};
use serde_json::json;

use crate::Core;
use crate::test_support::{Fixture, create_page, create_space, create_workspace, spaces};

const DAY: i64 = 86_400_000;

fn workspace(core: &mut Core) -> SpaceId {
    create_workspace(core, "Home", Some("Personal"));
    spaces(core)[0].id
}

/// Deterministic bytes that differ from chunk to chunk.
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    let mut state = u32::from(seed).wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

fn begin(core: &mut Core, space: SpaceId, name: &str, mime: &str) -> AttachmentId {
    match core
        .dispatch(Request::AttachmentBegin {
            space_id: space,
            name: name.into(),
            mime: mime.into(),
        })
        .unwrap()
    {
        Response::AttachmentStarted(id) => id,
        other => panic!("unexpected {other:?}"),
    }
}

fn finish(core: &mut Core, id: AttachmentId) -> AttachmentInfo {
    match core
        .dispatch(Request::AttachmentFinish { attachment_id: id })
        .unwrap()
    {
        Response::Attachment(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

/// Uploads `data` in uneven pieces, the way a shell streams a file.
fn upload(core: &mut Core, space: SpaceId, name: &str, data: &[u8]) -> AttachmentInfo {
    let id = begin(core, space, name, "application/octet-stream");
    let mut offset = 0;
    let mut piece = 1usize;
    while offset < data.len() {
        let end = (offset + piece).min(data.len());
        core.attachment_write(id, &data[offset..end]).unwrap();
        offset = end;
        piece = (piece * 3 + 1_000_003) % (CHUNK_SIZE + 7) + 1;
    }
    finish(core, id)
}

fn read(core: &mut Core, id: AttachmentId, range: Option<ByteRange>) -> Vec<u8> {
    let mut out = Vec::new();
    core.attachment_read(id, range, &mut |piece| {
        assert!(piece.len() <= CHUNK_SIZE);
        out.extend_from_slice(piece);
        Ok(())
    })
    .unwrap();
    out
}

fn info(core: &mut Core, id: AttachmentId) -> Result<AttachmentInfo, kb_types::ApiError> {
    match core.dispatch(Request::AttachmentGet { attachment_id: id })? {
        Response::Attachment(info) => Ok(info),
        other => panic!("unexpected {other:?}"),
    }
}

fn list(core: &mut Core, space: SpaceId) -> Vec<AttachmentInfo> {
    match core
        .dispatch(Request::AttachmentList { space_id: space })
        .unwrap()
    {
        Response::Attachments(list) => list,
        other => panic!("unexpected {other:?}"),
    }
}

fn gc(core: &mut Core) -> kb_types::GcSummary {
    match core.dispatch(Request::AttachmentGc).unwrap() {
        Response::AttachmentGc(summary) => summary,
        other => panic!("unexpected {other:?}"),
    }
}

fn chunks_of(core: &Core, space: SpaceId, id: AttachmentId) -> Vec<ChunkId> {
    core.structure(space)
        .unwrap()
        .model
        .attachment(id)
        .unwrap()
        .chunks
}

/// Puts a block into a page the way the editor does; returns the block id.
fn insert_block(core: &mut Core, page_id: PageId, kind: BlockKind, text: &str, n: u8) -> BlockId {
    let snapshot = core.page_doc_open(page_id).unwrap();
    let blob = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &snapshot);
    let doc = PageDoc::from_doc(
        Doc::load(DocKind::Page, 0x6000 + u64::from(n), Some(&blob), &[]).unwrap(),
    )
    .unwrap();
    let before = doc.doc().version();
    let last = doc.block_ids().last().copied();
    let id = BlockId::from_random_bytes([n; 16]);
    doc.insert_block(last, kind, text, id).unwrap();
    let update = doc
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    core.page_doc_import(page_id, payload).unwrap();
    core.page_doc_close(page_id).unwrap();
    id
}

fn delete_block(core: &mut Core, page_id: PageId, block: BlockId, n: u8) {
    let snapshot = core.page_doc_open(page_id).unwrap();
    let blob = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &snapshot);
    let doc = PageDoc::from_doc(
        Doc::load(DocKind::Page, 0x7000 + u64::from(n), Some(&blob), &[]).unwrap(),
    )
    .unwrap();
    let before = doc.doc().version();
    doc.delete_block(block).unwrap();
    let update = doc
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    core.page_doc_import(page_id, payload).unwrap();
    core.page_doc_close(page_id).unwrap();
}

fn image(attachment_id: AttachmentId) -> BlockKind {
    BlockKind::Image {
        attachment_id,
        width: Some(320),
    }
}

#[test]
fn uploads_of_every_size_round_trip() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let sizes = [
        0usize,
        1,
        CHUNK_SIZE - 1,
        CHUNK_SIZE,
        CHUNK_SIZE + 1,
        2 * CHUNK_SIZE + 123,
    ];
    for (n, size) in sizes.iter().enumerate() {
        let data = pattern(*size, n as u8);
        core.drain_events();
        let info = upload(&mut core, space, &format!("file-{n}.bin"), &data);
        assert_eq!(info.size, *size as u64);
        assert_eq!(info.chunk_count, (size / CHUNK_SIZE + 1) as u32);
        assert_eq!(info.references, 0);
        assert!(
            info.unreferenced_since.is_some(),
            "nothing references a fresh upload"
        );
        assert_eq!(
            core.drain_events(),
            vec![Event::AttachmentsChanged { space_id: space }]
        );
        let chunks = chunks_of(&core, space, info.attachment_id);
        assert_eq!(chunks.len(), info.chunk_count as usize);
        for chunk in &chunks {
            assert!(fx.blob_store.has(chunk).unwrap());
            let sealed = fx.blob_store.get(chunk).unwrap().unwrap();
            assert_eq!(ChunkId::from_bytes(kb_crypto::hash(&sealed)), *chunk);
            assert!(sealed.len() <= crate::attachments::MAX_SEALED_CHUNK);
        }
        assert_eq!(read(&mut core, info.attachment_id, None), data);
    }
    // Every upload is listed (the fixed test clock gives them all the same time, so the
    // "newest first" order degenerates to the id order).
    let mut names: Vec<String> = list(&mut core, space).into_iter().map(|i| i.name).collect();
    names.sort();
    let expected: Vec<String> = (0..sizes.len()).map(|n| format!("file-{n}.bin")).collect();
    assert_eq!(names, expected);
}

#[test]
fn ranged_reads_return_the_right_bytes() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let data = pattern(2 * CHUNK_SIZE + 123, 9);
    let id = upload(&mut core, space, "big.bin", &data).attachment_id;
    let cases = [
        (0u64, 10u64),
        (CHUNK_SIZE as u64 - 5, CHUNK_SIZE as u64 + 5),
        (CHUNK_SIZE as u64, 2 * CHUNK_SIZE as u64),
        (2 * CHUNK_SIZE as u64 + 100, data.len() as u64),
        (7, 7),
        (0, data.len() as u64),
    ];
    for (start, end) in cases {
        let got = read(&mut core, id, Some(ByteRange { start, end }));
        assert_eq!(
            got,
            &data[start as usize..end as usize],
            "range {start}..{end}"
        );
    }
    let bad = core
        .attachment_read(
            id,
            Some(ByteRange {
                start: 0,
                end: data.len() as u64 + 1,
            }),
            &mut |_| Ok(()),
        )
        .unwrap_err();
    assert_eq!(bad.code, ErrorCode::InvalidInput);
    let unknown = core
        .attachment_read(AttachmentId::from_random_bytes([3; 16]), None, &mut |_| {
            Ok(())
        })
        .unwrap_err();
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[test]
fn names_and_mime_types_are_validated() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let long = "x".repeat(256);
    for (name, mime) in [
        ("", "image/png"),
        ("   ", "image/png"),
        ("a/b.png", "image/png"),
        ("a\\b.png", "image/png"),
        ("..", "image/png"),
        (long.as_str(), "image/png"),
        ("ok.png", ""),
        ("ok.png", "noslash"),
        ("ok.png", "/png"),
        ("ok.png", "image/"),
        ("ok.png", "image/png/extra"),
        ("ok.png", "image/pn g"),
        ("ok.png", "imagé/png"),
    ] {
        let err = core
            .dispatch(Request::AttachmentBegin {
                space_id: space,
                name: name.into(),
                mime: mime.into(),
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{name:?} {mime:?}");
    }
    let missing_space = core
        .dispatch(Request::AttachmentBegin {
            space_id: SpaceId::from_random_bytes([9; 16]),
            name: "ok.png".into(),
            mime: "image/png".into(),
        })
        .unwrap_err();
    assert_eq!(missing_space.code, ErrorCode::NotFound);
    assert!(list(&mut core, space).is_empty());
    assert!(fx.blob_store.is_empty());
}

#[test]
fn tampering_is_detected_and_changes_nothing() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let data = pattern(CHUNK_SIZE + 500, 4);
    let id = upload(&mut core, space, "two-chunks.bin", &data).attachment_id;
    let original = core.structure(space).unwrap().model.attachment(id).unwrap();

    // Swapped chunk order: the AAD binds the index.
    let mut swapped = original.clone();
    swapped.chunks.swap(0, 1);
    core.structure_mut(space)
        .unwrap()
        .model
        .attachment_put(&swapped)
        .unwrap();
    let err = core.attachment_read(id, None, &mut |_| Ok(())).unwrap_err();
    assert_eq!(err.code, ErrorCode::Crypto);
    core.structure_mut(space)
        .unwrap()
        .model
        .attachment_put(&original)
        .unwrap();

    // A size the chunks cannot back.
    let mut bigger = original.clone();
    bigger.size += 10;
    core.structure_mut(space)
        .unwrap()
        .model
        .attachment_put(&bigger)
        .unwrap();
    let err = core.attachment_read(id, None, &mut |_| Ok(())).unwrap_err();
    assert_eq!(err.code, ErrorCode::Crypto);
    core.structure_mut(space)
        .unwrap()
        .model
        .attachment_put(&original)
        .unwrap();

    // A flipped ciphertext byte: the content address no longer matches.
    let first = original.chunks[0];
    let mut sealed = fx.blob_store.get(&first).unwrap().unwrap();
    sealed[100] ^= 0x01;
    fx.blob_store.delete(&first).unwrap();
    fx.blob_store.put(&first, &sealed).unwrap();
    let err = core.attachment_read(id, None, &mut |_| Ok(())).unwrap_err();
    assert_eq!(err.code, ErrorCode::Crypto);
    sealed[100] ^= 0x01;
    fx.blob_store.delete(&first).unwrap();
    fx.blob_store.put(&first, &sealed).unwrap();

    // A truncated final chunk.
    let last = original.chunks[1];
    let full = fx.blob_store.get(&last).unwrap().unwrap();
    fx.blob_store.delete(&last).unwrap();
    fx.blob_store.put(&last, &full[..full.len() - 1]).unwrap();
    let err = core.attachment_read(id, None, &mut |_| Ok(())).unwrap_err();
    assert_eq!(err.code, ErrorCode::Crypto);
    fx.blob_store.delete(&last).unwrap();

    // A missing chunk.
    let err = core.attachment_read(id, None, &mut |_| Ok(())).unwrap_err();
    assert_eq!(err.code, ErrorCode::Storage);
    fx.blob_store.put(&last, &full).unwrap();

    // Intact again.
    assert_eq!(read(&mut core, id, None), data);
}

#[test]
fn abort_removes_what_was_written() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let id = begin(&mut core, space, "aborted.bin", "application/octet-stream");
    core.attachment_write(id, &pattern(CHUNK_SIZE + 10, 2))
        .unwrap();
    assert_eq!(fx.blob_store.len(), 1, "one full chunk was sealed early");
    core.dispatch(Request::AttachmentAbort { attachment_id: id })
        .unwrap();
    assert!(fx.blob_store.is_empty());
    assert_eq!(
        core.dispatch(Request::AttachmentFinish { attachment_id: id })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        core.attachment_write(id, b"x").unwrap_err().code,
        ErrorCode::NotFound
    );
    assert!(list(&mut core, space).is_empty());
}

#[test]
fn references_follow_the_page_documents() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let page = create_page(&mut core, space, None, "Photos").id;
    let att = upload(&mut core, space, "cat.png", &pattern(10, 1)).attachment_id;
    assert_eq!(info(&mut core, att).unwrap().references, 0);

    core.drain_events();
    let block = insert_block(&mut core, page, image(att), "a cat", 1);
    let after = info(&mut core, att).unwrap();
    assert_eq!(after.references, 1);
    assert_eq!(after.unreferenced_since, None);
    assert!(
        core.drain_events()
            .contains(&Event::AttachmentsChanged { space_id: space })
    );
    // The alt text is searchable.
    match core
        .dispatch(Request::Search {
            query: "cat".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => assert!(hits.iter().any(|h| h.page_id == page)),
        other => panic!("unexpected {other:?}"),
    }

    // A second block on another page, then its removal.
    let other = create_page(&mut core, space, None, "More").id;
    let block2 = insert_block(&mut core, other, image(att), "", 2);
    assert_eq!(info(&mut core, att).unwrap().references, 2);
    delete_block(&mut core, other, block2, 3);
    assert_eq!(info(&mut core, att).unwrap().references, 1);

    // Trash keeps the reference; purge drops it and starts the clock.
    core.dispatch(Request::PageTrash { page_id: page }).unwrap();
    assert_eq!(info(&mut core, att).unwrap().references, 1);
    core.dispatch(Request::PageRestore { page_id: page })
        .unwrap();
    delete_block(&mut core, page, block, 4);
    let gone = info(&mut core, att).unwrap();
    assert_eq!(gone.references, 0);
    assert_eq!(gone.unreferenced_since, Some(1_000));
    let _ = insert_block(&mut core, page, image(att), "", 5);
    assert_eq!(info(&mut core, att).unwrap().unreferenced_since, None);
    core.dispatch(Request::PageTrash { page_id: page }).unwrap();
    core.dispatch(Request::PagePurge { page_id: page }).unwrap();
    let purged = info(&mut core, att).unwrap();
    assert_eq!(purged.references, 0);
    assert_eq!(purged.unreferenced_since, Some(1_000));
}

#[test]
fn garbage_collection_respects_retention_and_the_setting() {
    let fx = Fixture::new();
    let t0 = 1_000;
    let mut core = fx.open(1, t0);
    let space = workspace(&mut core);
    let page = create_page(&mut core, space, None, "Page").id;
    let kept = upload(&mut core, space, "kept.bin", &pattern(100, 1)).attachment_id;
    let loose = upload(&mut core, space, "loose.bin", &pattern(100, 2)).attachment_id;
    insert_block(&mut core, page, image(kept), "", 1);
    // A chunk nobody claims (a crash between write and finish).
    let orphan = ChunkId::from_bytes([0xAB; 32]);
    fx.blob_store.put(&orphan, b"leftover").unwrap();
    // An upload in progress keeps its chunks through a sweep.
    let pending = begin(&mut core, space, "pending.bin", "application/octet-stream");
    core.attachment_write(pending, &pattern(CHUNK_SIZE, 3))
        .unwrap();

    let summary = gc(&mut core);
    assert_eq!(summary.purged, 0);
    assert_eq!(summary.chunks_deleted, 1, "only the orphan goes");
    assert!(!fx.blob_store.has(&orphan).unwrap());
    assert_eq!(chunks_of(&core, space, loose).len(), 1);
    let pending_chunks = fx.blob_store.len();
    core.dispatch(Request::AttachmentAbort {
        attachment_id: pending,
    })
    .unwrap();
    assert_eq!(fx.blob_store.len(), pending_chunks - 1);
    drop(core);

    // 29 days later: within the default retention.
    let mut core = fx.open(2, t0 + 29 * DAY);
    assert_eq!(gc(&mut core).purged, 0);
    assert!(info(&mut core, loose).is_ok());

    // 31 days later: the loose one goes (already at open), the referenced one stays.
    let loose_chunks = chunks_of(&core, space, loose);
    drop(core);
    let mut core = fx.open(3, t0 + 31 * DAY);
    let summary = gc(&mut core);
    assert_eq!(
        summary.purged, 0,
        "the open-time collection already purged it"
    );
    assert_eq!(
        info(&mut core, loose).unwrap_err().code,
        ErrorCode::NotFound
    );
    for chunk in &loose_chunks {
        assert!(!fx.blob_store.has(chunk).unwrap());
    }
    assert!(info(&mut core, kept).is_ok());
    assert!(core.security.keyring.is_tombstoned(
        space,
        &KeySubject::Attachment {
            attachment_id: loose
        }
    ));

    // A one-day retention collects sooner; zero means never.
    let fresh = upload(&mut core, space, "fresh.bin", &pattern(10, 4)).attachment_id;
    core.dispatch(Request::SettingsSet {
        key: RETENTION_SETTING.into(),
        value: json!(1),
    })
    .unwrap();
    drop(core);
    let mut core = fx.open(4, t0 + 33 * DAY);
    assert_eq!(
        info(&mut core, fresh).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(gc(&mut core).purged, 0);
    let forever = upload(&mut core, space, "forever.bin", &pattern(10, 5)).attachment_id;
    core.dispatch(Request::SettingsSet {
        key: RETENTION_SETTING.into(),
        value: json!(0),
    })
    .unwrap();
    drop(core);
    let mut core = fx.open(5, t0 + 1_000 * DAY);
    assert_eq!(gc(&mut core).purged, 0);
    assert!(info(&mut core, forever).is_ok());
}

#[test]
fn delete_purges_even_a_referenced_attachment() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let page = create_page(&mut core, space, None, "Page").id;
    let att = upload(
        &mut core,
        space,
        "quarterly-report.pdf",
        &pattern(CHUNK_SIZE + 1, 6),
    )
    .attachment_id;
    insert_block(
        &mut core,
        page,
        BlockKind::File { attachment_id: att },
        "",
        1,
    );
    // The file's name is searchable through the block that shows it.
    match core
        .dispatch(Request::Search {
            query: "quarterly".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => assert!(hits.iter().any(|h| h.page_id == page), "{hits:?}"),
        other => panic!("unexpected {other:?}"),
    }
    let chunks = chunks_of(&core, space, att);
    core.drain_events();
    core.dispatch(Request::AttachmentDelete { attachment_id: att })
        .unwrap();
    assert_eq!(info(&mut core, att).unwrap_err().code, ErrorCode::NotFound);
    for chunk in &chunks {
        assert!(!fx.blob_store.has(chunk).unwrap());
    }
    assert!(
        core.structure(space)
            .unwrap()
            .model
            .attachment(att)
            .is_none()
    );
    assert!(
        core.security
            .keyring
            .is_tombstoned(space, &KeySubject::Attachment { attachment_id: att })
    );
    assert!(
        core.drain_events()
            .contains(&Event::AttachmentsChanged { space_id: space })
    );
    // The block still exists; a read now says NotFound.
    assert_eq!(
        core.attachment_read(att, None, &mut |_| Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    // The page still indexes fine (a reference to a purged attachment is harmless).
    core.dispatch(Request::IndexRebuild { space_id: None })
        .unwrap();
}

#[test]
fn deleting_a_space_purges_its_attachments() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    workspace(&mut core);
    let other = create_space(&mut core, "Archive").id;
    let att = upload(&mut core, other, "old.zip", &pattern(50, 7)).attachment_id;
    let chunks = chunks_of(&core, other, att);
    core.dispatch(Request::SpaceDelete { space_id: other })
        .unwrap();
    for chunk in &chunks {
        assert!(!fx.blob_store.has(chunk).unwrap());
    }
    assert_eq!(info(&mut core, att).unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn the_index_is_repaired_at_open() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let page = create_page(&mut core, space, None, "Page").id;
    let att = upload(&mut core, space, "pic.png", &pattern(30, 8)).attachment_id;
    insert_block(&mut core, page, image(att), "", 1);
    // Lose the index row and the references behind the core's back.
    core.db.attachment_delete(att).unwrap();
    assert_eq!(info(&mut core, att).unwrap_err().code, ErrorCode::NotFound);
    drop(core);

    let mut core = fx.open(2, 2_000);
    let repaired = info(&mut core, att).unwrap();
    assert_eq!(repaired.name, "pic.png");
    assert_eq!(repaired.references, 1);
    assert_eq!(repaired.unreferenced_since, None);
    assert_eq!(read(&mut core, att, None), pattern(30, 8));
}

#[test]
fn backups_carry_the_chunks() {
    let fx = Fixture::new();
    let mut a = fx.open(1, 1_000);
    let space = workspace(&mut a);
    let page = create_page(&mut a, space, None, "Page").id;
    let data = pattern(CHUNK_SIZE + 77, 10);
    let att = upload(&mut a, space, "photo.jpg", &data).attachment_id;
    insert_block(&mut a, page, image(att), "holiday", 1);
    let kit = match a.dispatch(Request::RecoveryKitCreate).unwrap() {
        Response::RecoveryKit(kit) => kit,
        other => panic!("unexpected {other:?}"),
    };
    let mut file = Vec::new();
    let summary = a
        .backup_export(&mut |bytes| {
            file.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
    assert_eq!(summary.chunks, 2);
    assert!(summary.chunk_bytes > data.len() as u64);
    drop(a);

    let fx_b = Fixture::new();
    let mut b = fx_b.open(7, 5_000);
    let mut pos = 0usize;
    let restored = b
        .backup_import(
            &mut |buf| {
                let n = buf.len().min(file.len() - pos);
                buf[..n].copy_from_slice(&file[pos..pos + n]);
                pos += n;
                Ok(n)
            },
            Some(&kit.words.join(" ")),
        )
        .unwrap();
    assert_eq!(restored.chunks, 2);
    assert_eq!(restored.documents, summary.documents);
    let back = info(&mut b, att).unwrap();
    assert_eq!(back.name, "photo.jpg");
    assert_eq!(back.references, 1);
    assert_eq!(read(&mut b, att, None), data);
    assert_eq!(fx_b.blob_store.len(), 2);
}

#[test]
fn a_damaged_backup_leaves_no_chunks_behind() {
    let fx = Fixture::new();
    let mut a = fx.open(1, 1_000);
    let space = workspace(&mut a);
    upload(&mut a, space, "x.bin", &pattern(10, 11));
    a.dispatch(Request::RecoveryKitCreate).unwrap();
    let mut file = Vec::new();
    a.backup_export(&mut |bytes| {
        file.extend_from_slice(bytes);
        Ok(())
    })
    .unwrap();
    drop(a);
    // Truncate inside the frames.
    file.truncate(file.len() - 40);
    let fx_b = Fixture::new();
    let mut b = fx_b.open(7, 5_000);
    let mut pos = 0usize;
    let err = b
        .backup_import(
            &mut |buf| {
                let n = buf.len().min(file.len() - pos);
                buf[..n].copy_from_slice(&file[pos..pos + n]);
                pos += n;
                Ok(n)
            },
            None,
        )
        .unwrap_err();
    assert!(
        matches!(
            err.code,
            ErrorCode::Crypto | ErrorCode::InvalidInput | ErrorCode::Storage
        ),
        "{err}"
    );
    assert!(fx_b.blob_store.is_empty());
}

/// Puts a block given as portable JSON into a page, the way the editor's binding does.
fn insert_portable_block(core: &mut Core, page_id: PageId, node: serde_json::Value, n: u8) {
    let content = json!({ "type": "doc", "content": [node] });
    let editor =
        PageDoc::from_portable(Doc::new(DocKind::Page, 0x8000 + u64::from(n)), &content).unwrap();
    assert_eq!(editor.check_invariants(), Vec::<String>::new());
    let blob = editor.doc().export_updates_since(None).unwrap().unwrap();
    let (_, payload) = kb_doc::decode_blob(&blob).unwrap();
    core.page_doc_open(page_id).unwrap();
    core.page_doc_import(page_id, payload).unwrap();
    core.page_doc_close(page_id).unwrap();
}

#[test]
fn every_attachment_a_gallery_lists_counts_as_referenced_until_the_block_goes() {
    // ADR-0016, amendment of 2026-10-09 (ADR-0030): a `gallery` lists its pictures in `items`;
    // each one is a reference of the block, so the retention collector never takes one while the
    // gallery is in a page, whatever its position in the list, and takes them after it is gone.
    let fx = Fixture::new();
    let t0 = 1_000;
    let mut core = fx.open(1, t0);
    let space = workspace(&mut core);
    let page = create_page(&mut core, space, None, "Photos").id;
    let first = upload(&mut core, space, "a.png", &pattern(10, 1)).attachment_id;
    let second = upload(&mut core, space, "b.png", &pattern(10, 2)).attachment_id;
    let third = upload(&mut core, space, "c.png", &pattern(10, 3)).attachment_id;
    let loose = upload(&mut core, space, "loose.png", &pattern(10, 4)).attachment_id;
    let gallery = BlockId::from_random_bytes([0x61; 16]);
    insert_portable_block(
        &mut core,
        page,
        json!({ "type": "gallery", "attrs": {
            "id": gallery.to_string(),
            "columns": 2,
            "items": [
                { "attachmentId": first.to_string(), "caption": "One" },
                { "attachmentId": second.to_string() },
                // The same picture twice is one reference.
                { "attachmentId": first.to_string() },
                { "attachmentId": third.to_string(), "caption": "Three" }
            ]
        } }),
        1,
    );
    for id in [first, second, third] {
        let shown = info(&mut core, id).unwrap();
        assert_eq!(shown.references, 1, "one block, once");
        assert_eq!(shown.unreferenced_since, None);
    }
    assert_eq!(info(&mut core, loose).unwrap().references, 0);
    drop(core);

    // 31 days later: the loose picture goes at open, the gallery's three stay.
    let mut core = fx.open(2, t0 + 31 * DAY);
    assert_eq!(gc(&mut core).purged, 0);
    assert_eq!(
        info(&mut core, loose).unwrap_err().code,
        ErrorCode::NotFound
    );
    for id in [first, second, third] {
        assert!(info(&mut core, id).is_ok());
        assert_eq!(info(&mut core, id).unwrap().unreferenced_since, None);
    }

    // The captions are text: the page is found by one.
    match core
        .dispatch(Request::Search {
            query: "Three".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => assert!(hits.iter().any(|h| h.page_id == page)),
        other => panic!("unexpected {other:?}"),
    }

    // Deleting the block starts the clock for all three; a month later they are collected.
    delete_block(&mut core, page, gallery, 2);
    for id in [first, second, third] {
        let gone = info(&mut core, id).unwrap();
        assert_eq!(gone.references, 0);
        assert_eq!(gone.unreferenced_since, Some(t0 + 31 * DAY));
    }
    drop(core);
    let mut core = fx.open(3, t0 + 63 * DAY);
    for id in [first, second, third] {
        assert_eq!(info(&mut core, id).unwrap_err().code, ErrorCode::NotFound);
    }
}

#[test]
fn an_expired_trashed_page_releases_its_attachment_like_a_purge() {
    use kb_types::testing::FixedClock;
    let fx = Fixture::new();
    let t0 = 1_000_000_000_000;
    let mut core = fx.open(1, t0);
    let space = workspace(&mut core);
    let page = create_page(&mut core, space, None, "Photos").id;
    let att = upload(&mut core, space, "cat.png", &pattern(10, 1)).attachment_id;
    insert_block(&mut core, page, image(att), "a cat", 1);
    core.dispatch(Request::PageTrash { page_id: page }).unwrap();
    assert_eq!(
        info(&mut core, att).unwrap().references,
        1,
        "a trashed page keeps its files"
    );

    core.clock = Box::new(FixedClock::new(t0 + 31 * DAY));
    core.drain_events();
    assert_eq!(core.trash_expire().unwrap(), 1);
    let released = info(&mut core, att).unwrap();
    assert_eq!(released.references, 0);
    // The retention of the file starts with the purge, not with the page's trashing.
    assert_eq!(released.unreferenced_since, Some(t0 + 31 * DAY));
    assert!(
        core.drain_events()
            .contains(&Event::AttachmentsChanged { space_id: space })
    );
}

/// The content of a template button is template material (ADR-0056): the name of a file block in it is not
/// searched, as no other text in it is, while the same file outside a button is.
#[test]
fn a_file_inside_a_template_button_is_not_searchable_by_its_name() {
    let fx = Fixture::new();
    let mut core = fx.open(1, 1_000);
    let space = workspace(&mut core);
    let page = create_page(&mut core, space, None, "Page").id;
    let inside = upload(
        &mut core,
        space,
        "inside-the-button.pdf",
        &pattern(CHUNK_SIZE + 1, 6),
    )
    .attachment_id;
    let outside = upload(
        &mut core,
        space,
        "outside-the-button.pdf",
        &pattern(CHUNK_SIZE + 1, 7),
    )
    .attachment_id;
    let id = |n: u8| BlockId::from_random_bytes([n; 16]).to_string();
    let content = json!({ "type": "doc", "content": [
        { "type": "paragraph", "attrs": { "id": id(1) }, "content": [{ "type": "text", "text": "plain words" }] },
        { "type": "templateButton", "attrs": { "id": id(2), "label": "Add" }, "content": [
            { "type": "paragraph", "attrs": { "id": id(3) }, "content": [{ "type": "text", "text": "template words" }] },
            { "type": "file", "attrs": { "id": id(4), "attachmentId": inside.to_string() } },
        ] },
        { "type": "file", "attrs": { "id": id(5), "attachmentId": outside.to_string() } },
    ] });
    core.replace_page_document_for_test(page, &content).unwrap();

    let found = |core: &mut Core, query: &str| -> bool {
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
            Response::Search(hits) => hits.iter().any(|h| h.page_id == page),
            other => panic!("unexpected {other:?}"),
        }
    };
    assert!(found(&mut core, "plain"));
    assert!(
        found(&mut core, "outside"),
        "a file outside a button is found by its name"
    );
    assert!(
        !found(&mut core, "template"),
        "the words in a button are not searched"
    );
    assert!(
        !found(&mut core, "inside"),
        "nor is the name of a file in it"
    );
}
