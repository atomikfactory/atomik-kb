//! The semantic index and retrieval (ADR-0037, `docs/architecture/ai.md` section 5): the work for
//! the shell's indexer, storing vectors, the status and the clearing, and the hybrid retrieval
//! over the vector cache. The vectors here are made by hand: `embed` maps a few keywords of a
//! chunk to the axes of a 16-dimensional space, so the cosines the tests rely on are exact.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_ai::chunk::chunk_hash;
use kb_ai::vector::decode;
use kb_storage::{AiIndexQueries, FtsBlock, TextIndexQueries};
use kb_types::ai::{
    AI_SETTINGS_SETTING, AiChunkVector, AiIndexInfo, AiIndexWork, AiScope, AiSource,
};
use kb_types::{ApiError, BlockId, ErrorCode, Event, PageId, Request, Response, SpaceId};
use serde_json::json;

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, spaces, write_text,
};

const MODEL: &str = "test-embed";
const DIMS: usize = 16;
const T0: i64 = 5_000;

fn setup() -> (Fixture, Core, SpaceId) {
    let fixture = Fixture::new();
    let mut core = fixture.open(1, T0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    (fixture, core, space)
}

fn set_ai(core: &mut Core, value: serde_json::Value) {
    core.dispatch(Request::SettingsSet {
        key: AI_SETTINGS_SETTING.into(),
        value,
    })
    .unwrap();
}

fn ai_on(core: &mut Core, model: &str) {
    set_ai(
        core,
        json!({ "enabled": true, "embeddingModel": model, "semanticIndex": true }),
    );
}

/// A page with a title and one paragraph per text.
fn page_with(core: &mut Core, space: SpaceId, title: &str, paragraphs: &[&str]) -> PageId {
    let page = create_page(core, space, None, title).id;
    for text in paragraphs {
        add_text(core, page, text);
    }
    page
}

/// The seed of `write_text` must differ per call: it makes the block id.
fn add_text(core: &mut Core, page: PageId, text: &str) {
    thread_local! {
        static SEED: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
    }
    let seed = SEED.with(|seed| {
        seed.set(seed.get() + 1);
        seed.get()
    });
    write_text(core, page, text, seed);
}

fn next(core: &mut Core, max_chunks: u32) -> Result<AiIndexWork, ApiError> {
    match core.dispatch(Request::AiIndexNext { max_chunks })? {
        Response::AiIndexWork(work) => Ok(work),
        other => panic!("unexpected {other:?}"),
    }
}

fn status(core: &mut Core) -> AiIndexInfo {
    match core.dispatch(Request::AiIndexStatus).unwrap() {
        Response::AiIndex(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

fn clear(core: &mut Core) {
    assert_eq!(core.dispatch(Request::AiIndexClear).unwrap(), Response::Ok);
}

fn store(
    core: &mut Core,
    page_id: PageId,
    source_hash: &str,
    model: &str,
    vectors: Vec<AiChunkVector>,
) -> Result<(), ApiError> {
    match core.dispatch(Request::AiIndexStore {
        page_id,
        source_hash: source_hash.to_owned(),
        model: model.to_owned(),
        vectors,
    })? {
        Response::Ok => Ok(()),
        other => panic!("unexpected {other:?}"),
    }
}

/// The unit vector of an axis.
fn axis(i: usize) -> Vec<f32> {
    let mut v = vec![0.0; DIMS];
    v[i] = 1.0;
    v
}

/// Keywords of a chunk's text as axes: alpha 0, beta 1, gamma 2, delta 3, anything else 15.
fn embed(text: &str) -> Vec<f32> {
    let text = text.to_lowercase();
    let mut v = vec![0.0; DIMS];
    let mut any = false;
    for (i, word) in ["alpha", "beta", "gamma", "delta"].iter().enumerate() {
        if text.contains(word) {
            v[i] = 1.0;
            any = true;
        }
    }
    if !any {
        v[15] = 1.0;
    }
    v
}

/// Stores the work: a vector from `embed` for every chunk that needs one. Returns the pages stored.
fn store_work(core: &mut Core, work: &AiIndexWork) -> Vec<PageId> {
    let model = work.model.clone().unwrap();
    for page in &work.pages {
        let vectors = page
            .chunks
            .iter()
            .filter(|chunk| chunk.needs_vector)
            .map(|chunk| AiChunkVector {
                seq: chunk.seq,
                vector: embed(&chunk.text),
            })
            .collect();
        store(core, page.page_id, &page.source_hash, &model, vectors).unwrap();
    }
    work.pages.iter().map(|page| page.page_id).collect()
}

/// Walks `AiIndexNext` and stores every page until nothing waits. Returns how many pages were stored.
fn index_all(core: &mut Core) -> usize {
    let mut stored = 0;
    for _ in 0..100 {
        let work = next(core, 64).unwrap();
        if work.pages.is_empty() && work.remaining == 0 {
            return stored;
        }
        stored += store_work(core, &work).len();
    }
    panic!("the index does not settle");
}

fn retrieve(
    core: &mut Core,
    query: &str,
    vector: Option<Vec<f32>>,
    scope: AiScope,
    limit: Option<u32>,
) -> Result<Vec<AiSource>, ApiError> {
    match core.dispatch(Request::AiRetrieve {
        query: query.to_owned(),
        vector,
        scope,
        limit,
    })? {
        Response::AiPassages(passages) => Ok(passages),
        other => panic!("unexpected {other:?}"),
    }
}

fn pages_of(passages: &[AiSource]) -> Vec<PageId> {
    passages.iter().map(|p| p.page_id).collect()
}

fn texts(passages: &[AiSource]) -> Vec<&str> {
    passages.iter().map(|p| p.text.as_str()).collect()
}

fn changed(core: &mut Core) -> usize {
    core.drain_events()
        .iter()
        .filter(|event| **event == Event::AiIndexChanged)
        .count()
}

fn invalid_field(err: &ApiError) -> Option<&str> {
    assert_eq!(err.code, ErrorCode::InvalidInput, "{err:?}");
    err.details.as_ref()?.get("field")?.as_str()
}

// ---- AiIndexNext: when there is nothing to do --------------------------------------------

#[test]
fn work_is_empty_while_ai_is_off_or_the_index_is_off_or_no_model_is_chosen() {
    let (_fixture, mut core, space) = setup();
    page_with(&mut core, space, "Alpha", &["alpha text"]);
    for value in [
        None,
        Some(json!({})),
        Some(json!({ "enabled": false, "embeddingModel": MODEL })),
        Some(json!({ "enabled": true })),
        Some(json!({ "enabled": true, "embeddingModel": "  " })),
        Some(json!({ "enabled": true, "embeddingModel": MODEL, "semanticIndex": false })),
        Some(json!({ "enabled": "yes please", "embeddingModel": MODEL })),
        Some(json!(["not", "settings"])),
    ] {
        if let Some(value) = value {
            set_ai(&mut core, value.clone());
            let work = next(&mut core, 16).unwrap();
            assert_eq!(work, AiIndexWork::default(), "{value}");
        } else {
            assert_eq!(next(&mut core, 16).unwrap(), AiIndexWork::default());
        }
    }
    assert!(
        core.db.ai_dirty_count().unwrap() > 0,
        "nothing was consumed"
    );
}

#[test]
fn max_chunks_outside_one_to_sixty_four_is_invalid_input() {
    let (_fixture, mut core, _space) = setup();
    for bad in [0, 65, u32::MAX] {
        let err = next(&mut core, bad).unwrap_err();
        assert_eq!(invalid_field(&err), Some("maxChunks"), "{bad}");
    }
    next(&mut core, 1).unwrap();
    next(&mut core, 64).unwrap();
}

// ---- AiIndexNext: the work ----------------------------------------------------------------

#[test]
fn work_lists_the_dirty_pages_in_id_order_with_their_chunks() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(
        &mut core,
        space,
        "Alpha plan",
        &["alpha first", "alpha second"],
    );
    let b = page_with(&mut core, space, "Beta notes", &["beta only"]);
    let c = create_page(&mut core, space, None, "Gamma").id;
    core.drain_events();

    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.model.as_deref(), Some(MODEL));
    assert_eq!(work.remaining, 0);
    let mut ids = vec![a, b, c];
    ids.sort();
    assert_eq!(
        work.pages.iter().map(|p| p.page_id).collect::<Vec<_>>(),
        ids,
        "in page id order"
    );
    let page = |id: PageId| work.pages.iter().find(|p| p.page_id == id).unwrap();

    // The two paragraphs of A fit one chunk: the title, then the blocks joined by a new line.
    let alpha = page(a);
    assert_eq!(alpha.chunks.len(), 1);
    assert_eq!(alpha.chunks[0].seq, 0);
    assert_eq!(
        alpha.chunks[0].text,
        "Alpha plan\nalpha first\nalpha second"
    );
    assert_eq!(
        alpha.chunks[0].hash,
        kb_ai::hex(&chunk_hash(MODEL, &alpha.chunks[0].text))
    );
    assert!(alpha.chunks[0].needs_vector);
    assert_eq!(alpha.source_hash.len(), 64);
    // A page without text is its title alone.
    assert_eq!(page(c).chunks.len(), 1);
    assert_eq!(page(c).chunks[0].text, "Gamma");
    // Another page, another source hash.
    assert_ne!(page(a).source_hash, page(b).source_hash);

    // The work changes nothing: asking again gives the same work.
    assert_eq!(next(&mut core, 64).unwrap(), work);
    assert_eq!(status(&mut core).dirty, 3);
}

#[test]
fn work_stops_before_max_chunks_vectors_are_needed_but_always_has_a_page() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    for n in 0..5 {
        page_with(&mut core, space, &format!("Page {n}"), &[]);
    }
    let work = next(&mut core, 2).unwrap();
    assert_eq!(work.pages.len(), 2);
    assert_eq!(work.remaining, 3);
    let work = next(&mut core, 1).unwrap();
    assert_eq!(work.pages.len(), 1);
    assert_eq!(work.remaining, 4);

    // A page with more chunks than the budget still comes alone.
    let long = "word ".repeat(180); // 900 characters: one chunk each
    let big = page_with(
        &mut core,
        space,
        "Big",
        &[long.as_str(), long.as_str(), long.as_str()],
    );
    for _ in 0..20 {
        let work = next(&mut core, 1).unwrap();
        if work.pages[0].page_id == big {
            assert_eq!(work.pages.len(), 1);
            assert_eq!(work.pages[0].chunks.len(), 3);
            return;
        }
        store_work(&mut core, &work);
    }
    panic!("the big page never came");
}

#[test]
fn a_page_with_no_chunk_at_all_goes_through_as_an_empty_page() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let empty = create_page(&mut core, space, None, "").id;
    let work = next(&mut core, 16).unwrap();
    assert_eq!(work.pages.len(), 1);
    assert_eq!(work.pages[0].page_id, empty);
    assert!(
        work.pages[0].chunks.is_empty(),
        "no title, no text, no chunk"
    );
    store_work(&mut core, &work);
    let after = next(&mut core, 16).unwrap();
    assert!(after.pages.is_empty());
    assert_eq!(after.remaining, 0);
    let info = status(&mut core);
    assert_eq!((info.pages_indexed, info.chunks, info.dirty), (1, 0, 0));
}

// ---- AiIndexStore ---------------------------------------------------------------------------

#[test]
fn storing_the_vectors_indexes_the_page_and_emits_a_change() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    let b = page_with(&mut core, space, "Beta", &["beta text"]);
    core.drain_events();

    let work = next(&mut core, 64).unwrap();
    assert_eq!(changed(&mut core), 0, "the walk changed nothing");
    let page = work.pages.iter().find(|p| p.page_id == a).unwrap();
    // A vector that is not unit length is normalised.
    let mut vector = embed("alpha");
    vector[1] = 3.0;
    store(
        &mut core,
        a,
        &page.source_hash,
        MODEL,
        vec![AiChunkVector { seq: 0, vector }],
    )
    .unwrap();
    assert_eq!(changed(&mut core), 1);

    let rows = core.db.ai_chunks_of(a).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text, "Alpha\nalpha text");
    let stored = decode(&rows[0].vector).unwrap();
    let norm = stored.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5, "unit length, got {norm}");
    let row = core.db.ai_page_get(a).unwrap().unwrap();
    assert_eq!(
        (row.model.as_str(), row.chunks, row.indexed_at),
        (MODEL, 1, T0)
    );

    let info = status(&mut core);
    assert_eq!(
        info,
        AiIndexInfo {
            model: Some(MODEL.to_owned()),
            pages_indexed: 1,
            pages_total: 2,
            chunks: 1,
            dirty: 1,
            dimensions: Some(16),
        }
    );
    // The other page is the only one left.
    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.pages.len(), 1);
    assert_eq!(work.pages[0].page_id, b);
    assert_eq!(work.remaining, 0);
    store_work(&mut core, &work);
    let info = status(&mut core);
    assert_eq!((info.pages_indexed, info.dirty), (2, 0));
    let work = next(&mut core, 64).unwrap();
    assert!(work.pages.is_empty());
    assert_eq!(work.remaining, 0);
    assert_eq!(work.model.as_deref(), Some(MODEL));
}

#[test]
fn a_store_that_is_stale_changes_nothing_and_answers_ok() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    let work = next(&mut core, 64).unwrap();
    let hash = work.pages[0].source_hash.clone();
    let vectors = |seq| {
        vec![AiChunkVector {
            seq,
            vector: axis(0),
        }]
    };
    core.drain_events();

    // The text changed since the work was handed out.
    add_text(&mut core, a, "more alpha");
    core.drain_events();
    store(&mut core, a, &hash, MODEL, vectors(0)).unwrap();
    assert_eq!(changed(&mut core), 0);
    assert!(core.db.ai_page_get(a).unwrap().is_none());
    assert_eq!(status(&mut core).dirty, 1, "the page stays dirty");

    // The model of the request is not the current one; the hash is garbage; the page is unknown.
    let work = next(&mut core, 64).unwrap();
    let hash = work.pages[0].source_hash.clone();
    store(&mut core, a, &hash, "another-model", vectors(0)).unwrap();
    store(&mut core, a, "stale", MODEL, vectors(0)).unwrap();
    store(&mut core, a, "", MODEL, vec![]).unwrap();
    store(
        &mut core,
        PageId::from_bytes([9; 16]),
        &hash,
        MODEL,
        vectors(0),
    )
    .unwrap();
    assert!(core.db.ai_page_get(a).unwrap().is_none());
    assert_eq!(changed(&mut core), 0);

    // AI turned off, the index turned off, the page trashed: all stale.
    set_ai(
        &mut core,
        json!({ "enabled": false, "embeddingModel": MODEL }),
    );
    store(&mut core, a, &hash, MODEL, vectors(0)).unwrap();
    set_ai(
        &mut core,
        json!({ "enabled": true, "embeddingModel": MODEL, "semanticIndex": false }),
    );
    store(&mut core, a, &hash, MODEL, vectors(0)).unwrap();
    ai_on(&mut core, MODEL);
    core.dispatch(Request::PageTrash { page_id: a }).unwrap();
    store(&mut core, a, &hash, MODEL, vectors(0)).unwrap();
    assert!(core.db.ai_page_get(a).unwrap().is_none());
    core.dispatch(Request::PageRestore { page_id: a }).unwrap();
    core.drain_events();

    // And the right one still works.
    store(&mut core, a, &hash, MODEL, vectors(0)).unwrap();
    assert!(core.db.ai_page_get(a).unwrap().is_some());
    assert_eq!(changed(&mut core), 1);
}

#[test]
fn a_malformed_store_is_invalid_input_for_vectors() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let long = "word ".repeat(180);
    let a = page_with(&mut core, space, "Alpha", &[long.as_str(), long.as_str()]);
    let work = next(&mut core, 64).unwrap();
    let page = work.pages.iter().find(|p| p.page_id == a).unwrap();
    assert_eq!(page.chunks.len(), 2);
    let hash = page.source_hash.clone();
    let vec_of = |seq: u32, len: usize| AiChunkVector {
        seq,
        vector: vec![0.5; len],
    };

    let nan = AiChunkVector {
        seq: 1,
        vector: {
            let mut v = vec![0.5; 16];
            v[3] = f32::NAN;
            v
        },
    };
    let infinite = AiChunkVector {
        seq: 1,
        vector: {
            let mut v = vec![0.5; 16];
            v[3] = f32::INFINITY;
            v
        },
    };
    for (what, vectors) in [
        ("too short", vec![vec_of(0, 15), vec_of(1, 15)]),
        ("too long", vec![vec_of(0, 8_193), vec_of(1, 8_193)]),
        ("empty", vec![vec_of(0, 0), vec_of(1, 0)]),
        ("different lengths", vec![vec_of(0, 16), vec_of(1, 17)]),
        ("duplicate seq", vec![vec_of(0, 16), vec_of(0, 16)]),
        (
            "unknown seq",
            vec![vec_of(0, 16), vec_of(1, 16), vec_of(2, 16)],
        ),
        ("not a number", vec![vec_of(0, 16), nan]),
        ("infinite", vec![vec_of(0, 16), infinite]),
        ("a chunk without a vector", vec![vec_of(0, 16)]),
        ("no vector at all", vec![]),
    ] {
        let err = store(&mut core, a, &hash, MODEL, vectors).unwrap_err();
        assert_eq!(invalid_field(&err), Some("vectors"), "{what}");
    }
    // Rejected even when the request would have been stale.
    let err = store(&mut core, a, "stale", MODEL, vec![vec_of(0, 15)]).unwrap_err();
    assert_eq!(invalid_field(&err), Some("vectors"));
    assert!(
        core.db.ai_page_get(a).unwrap().is_none(),
        "nothing was written"
    );
    assert_eq!(changed(&mut core), 0);

    // The boundaries are fine.
    store(
        &mut core,
        a,
        &hash,
        MODEL,
        vec![vec_of(0, 16), vec_of(1, 16)],
    )
    .unwrap();
    assert_eq!(core.db.ai_chunks_of(a).unwrap().len(), 2);
    let work = next(&mut core, 64).unwrap();
    assert!(work.pages.is_empty());
    add_text(&mut core, a, "tail");
    let work = next(&mut core, 64).unwrap();
    let hash = work.pages[0].source_hash.clone();
    let wide = |seq| AiChunkVector {
        seq,
        vector: vec![0.5; 8_192],
    };
    // Reused vectors have 16 values: a request of another length conflicts with them.
    let err = store(&mut core, a, &hash, MODEL, vec![wide(2)]).unwrap_err();
    assert_eq!(invalid_field(&err), Some("vectors"));
}

#[test]
fn a_vector_is_reused_by_the_hash_of_its_text_across_pages_and_edits() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let long = |c: char| c.to_string().repeat(700);
    // Two pages with the same title and the same text: the same chunk hash.
    let a = page_with(&mut core, space, "Same", &["alpha shared body"]);
    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.pages.len(), 1);
    store_work(&mut core, &work);

    let b = page_with(&mut core, space, "Same", &["alpha shared body"]);
    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.pages.len(), 1);
    assert_eq!(work.pages[0].page_id, b);
    assert!(
        !work.pages[0].chunks[0].needs_vector,
        "another page holds the vector of this text"
    );
    // The answer needs no vectors; the chunk takes the stored one.
    store(&mut core, b, &work.pages[0].source_hash, MODEL, vec![]).unwrap();
    let stored_a = core.db.ai_chunks_of(a).unwrap();
    let stored_b = core.db.ai_chunks_of(b).unwrap();
    assert_eq!(stored_a[0].vector, stored_b[0].vector);
    assert_eq!(status(&mut core).chunks, 2);

    // An edit of one chunk of a page re-embeds that chunk only.
    let c = page_with(
        &mut core,
        space,
        "Two",
        &[long('a').as_str(), long('b').as_str()],
    );
    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.pages.len(), 1);
    assert_eq!(work.pages[0].chunks.len(), 2);
    assert!(work.pages[0].chunks.iter().all(|chunk| chunk.needs_vector));
    store_work(&mut core, &work);
    let first_vector = core.db.ai_chunks_of(c).unwrap()[0].vector.clone();
    add_text(&mut core, c, &long('c'));
    let work = next(&mut core, 64).unwrap();
    let chunks = &work.pages[0].chunks;
    assert_eq!(chunks.len(), 3);
    assert_eq!(
        chunks.iter().map(|c| c.needs_vector).collect::<Vec<_>>(),
        [false, false, true],
        "the two unchanged chunks keep their vectors"
    );
    let vectors = vec![AiChunkVector {
        seq: 2,
        vector: axis(7),
    }];
    store(&mut core, c, &work.pages[0].source_hash, MODEL, vectors).unwrap();
    let rows = core.db.ai_chunks_of(c).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].vector, first_vector);

    // A chunk with no vector anywhere, and none in the request, is refused.
    let d = page_with(&mut core, space, "Fresh", &["something unseen"]);
    let work = next(&mut core, 64).unwrap();
    let page = work.pages.iter().find(|p| p.page_id == d).unwrap();
    let err = store(&mut core, d, &page.source_hash, MODEL, vec![]).unwrap_err();
    assert_eq!(invalid_field(&err), Some("vectors"));
}

#[test]
fn a_block_written_again_with_the_same_text_makes_the_page_stale() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    index_all(&mut core);
    let old_ids = core.db.ai_chunks_of(a).unwrap()[0].block_ids.clone();
    assert_eq!(old_ids.len(), 1);

    // The block is deleted and written again with the same text: same chunk, another block id.
    let rewrite = |core: &mut Core, id: u8| {
        core.db
            .fts_replace_page(
                a,
                "Alpha",
                &[FtsBlock {
                    block_id: BlockId::from_random_bytes([id; 16]),
                    text: "alpha text".to_owned(),
                }],
            )
            .unwrap();
    };
    rewrite(&mut core, 77);
    let work = next(&mut core, 64).unwrap();
    assert_eq!(
        work.pages.len(),
        1,
        "the page is not up to date: its block ids changed"
    );
    let chunk = &work.pages[0].chunks[0];
    assert!(
        !chunk.needs_vector,
        "the vector is reused by the hash of the text"
    );
    // A store for a work that has gone stale in the same way changes nothing.
    rewrite(&mut core, 78);
    store(&mut core, a, &work.pages[0].source_hash, MODEL, vec![]).unwrap();
    assert_eq!(core.db.ai_chunks_of(a).unwrap()[0].block_ids, old_ids);
    // The current work stores, and the chunk now names the new block.
    let work = next(&mut core, 64).unwrap();
    store(&mut core, a, &work.pages[0].source_hash, MODEL, vec![]).unwrap();
    assert_eq!(
        core.db.ai_chunks_of(a).unwrap()[0].block_ids,
        [BlockId::from_random_bytes([78; 16])]
    );
    assert!(next(&mut core, 64).unwrap().pages.is_empty());
}

#[test]
fn a_change_of_model_clears_the_index_and_starts_over() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    let b = page_with(&mut core, space, "Beta", &["beta text"]);
    assert_eq!(index_all(&mut core), 2);
    assert_eq!(status(&mut core).chunks, 2);
    core.drain_events();

    ai_on(&mut core, "other-embed");
    // Until the next walk the index still holds the old vectors but counts none as current.
    let info = status(&mut core);
    assert_eq!((info.pages_indexed, info.chunks), (0, 2));
    assert_eq!(info.model.as_deref(), Some(MODEL));

    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.model.as_deref(), Some("other-embed"));
    assert_eq!(work.pages.len(), 2);
    assert!(
        work.pages
            .iter()
            .flat_map(|p| &p.chunks)
            .all(|c| c.needs_vector),
        "no vector of the old model is reused"
    );
    assert_eq!(changed(&mut core), 1, "the clearing is announced");
    let info = status(&mut core);
    assert_eq!(
        (info.chunks, info.pages_indexed, info.dirty, info.model),
        (0, 0, 2, None)
    );
    assert!(core.db.ai_page_get(a).unwrap().is_none());

    store_work(&mut core, &work);
    let info = status(&mut core);
    assert_eq!(info.model.as_deref(), Some("other-embed"));
    assert_eq!((info.pages_indexed, info.chunks), (2, 2));
    assert!(core.db.ai_page_get(b).unwrap().is_some());
    // The same model again changes nothing.
    core.drain_events();
    assert!(next(&mut core, 64).unwrap().pages.is_empty());
    assert_eq!(changed(&mut core), 0);
}

// ---- trashed and missing pages ---------------------------------------------------------------

#[test]
fn trashed_and_purged_pages_are_dropped_from_the_walk() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    let b = page_with(&mut core, space, "Beta", &["beta text"]);
    let c = page_with(&mut core, space, "Gamma", &["gamma text"]);
    assert_eq!(index_all(&mut core), 3);
    assert_eq!(status(&mut core).chunks, 3);

    // Trashing does not touch the text index: the page keeps its chunks, and counts as neither
    // indexed nor total.
    core.dispatch(Request::PageTrash { page_id: a }).unwrap();
    core.dispatch(Request::PageTrash { page_id: b }).unwrap();
    let info = status(&mut core);
    assert_eq!(
        (info.pages_total, info.pages_indexed, info.chunks),
        (1, 1, 3)
    );

    // The walk drops the chunks of a trashed page that is waiting, and of a purged one.
    core.db.ai_dirty_mark_all().unwrap();
    core.dispatch(Request::PagePurge { page_id: b }).unwrap();
    core.drain_events();
    let work = next(&mut core, 64).unwrap();
    assert!(
        work.pages.is_empty(),
        "the live page did not change: nothing to embed"
    );
    assert_eq!(work.remaining, 0);
    assert_eq!(changed(&mut core), 1);
    assert!(
        core.db.ai_page_get(a).unwrap().is_none(),
        "trashed: dropped"
    );
    assert!(core.db.ai_page_get(b).unwrap().is_none(), "purged: dropped");
    assert!(core.db.ai_chunks_of(a).unwrap().is_empty());
    assert_eq!(status(&mut core).chunks, 1);
    assert_eq!(
        core.db.ai_dirty_count().unwrap(),
        0,
        "nothing of the trashed, purged or unchanged pages waits any more"
    );
    assert!(core.db.ai_page_get(c).unwrap().is_some());
}

#[test]
fn a_page_restored_after_the_walk_dropped_it_is_indexed_again() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    // Waiting, then trashed: the walk drops it.
    core.dispatch(Request::PageTrash { page_id: a }).unwrap();
    let work = next(&mut core, 64).unwrap();
    assert!(work.pages.is_empty());
    assert_eq!(work.remaining, 0);
    assert_eq!(core.db.ai_dirty_count().unwrap(), 0);

    // Restoring changes no text row, so nothing marks it: the walk finds it by itself.
    core.dispatch(Request::PageRestore { page_id: a }).unwrap();
    assert_eq!(core.db.ai_dirty_count().unwrap(), 0);
    core.drain_events();
    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.pages.len(), 1);
    assert_eq!(work.pages[0].page_id, a);
    assert_eq!(changed(&mut core), 1);
    store_work(&mut core, &work);
    assert_eq!(status(&mut core).pages_indexed, 1);
    assert!(next(&mut core, 64).unwrap().pages.is_empty());
}

#[test]
fn a_page_whose_text_did_not_change_leaves_the_walk_without_work() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    page_with(&mut core, space, "Alpha", &["alpha text"]);
    page_with(&mut core, space, "Beta", &["beta text"]);
    index_all(&mut core);

    // A rebuild of the text index rewrites every row and marks every page: nothing is embedded.
    core.dispatch(Request::IndexRebuild { space_id: None })
        .unwrap();
    assert_eq!(core.db.ai_dirty_count().unwrap(), 2);
    core.drain_events();
    let work = next(&mut core, 64).unwrap();
    assert!(work.pages.is_empty());
    assert_eq!(work.remaining, 0);
    assert_eq!(
        changed(&mut core),
        1,
        "the dirty pages left: the status moved"
    );
    let info = status(&mut core);
    assert_eq!((info.pages_indexed, info.dirty, info.chunks), (2, 0, 2));
}

// ---- status and clearing -------------------------------------------------------------------------

#[test]
fn status_counts_live_pages_chunks_waiting_pages_and_dimensions() {
    let (_fixture, mut core, space) = setup();
    assert_eq!(status(&mut core), AiIndexInfo::default());
    let other = create_space(&mut core, "Work").id;
    page_with(&mut core, space, "Alpha", &["alpha text"]);
    page_with(&mut core, other, "Beta", &["beta text"]);
    let trashed = page_with(&mut core, other, "Gamma", &[]);
    core.dispatch(Request::PageTrash { page_id: trashed })
        .unwrap();
    // AI is off: the pages wait and nothing is indexed.
    let info = status(&mut core);
    assert_eq!(info.pages_total, 2, "live pages only");
    assert_eq!((info.pages_indexed, info.chunks, info.model), (0, 0, None));
    assert_eq!(info.dirty, 2);
    assert_eq!(info.dimensions, None);

    ai_on(&mut core, MODEL);
    index_all(&mut core);
    let info = status(&mut core);
    assert_eq!(
        info,
        AiIndexInfo {
            model: Some(MODEL.to_owned()),
            pages_indexed: 2,
            pages_total: 2,
            chunks: 2,
            dirty: 0,
            dimensions: Some(16),
        }
    );
    // A change of text makes the page wait again.
    let alpha = create_page(&mut core, space, None, "Delta").id;
    add_text(&mut core, alpha, "delta text");
    let info = status(&mut core);
    assert_eq!(
        (info.pages_indexed, info.pages_total, info.dirty),
        (2, 3, 1)
    );
}

#[test]
fn clearing_deletes_the_vectors_marks_every_page_and_drops_the_cache() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    page_with(&mut core, space, "Beta", &["beta text"]);
    index_all(&mut core);
    retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, None).unwrap();
    assert!(core.ai_cache_loaded());
    core.drain_events();

    clear(&mut core);
    assert_eq!(changed(&mut core), 1);
    assert!(!core.ai_cache_loaded());
    let info = status(&mut core);
    assert_eq!(
        (
            info.chunks,
            info.pages_indexed,
            info.dirty,
            info.model,
            info.dimensions
        ),
        (0, 0, 2, None, None)
    );
    assert!(core.db.ai_page_get(a).unwrap().is_none());
    // The pages are walked again, with no vector to reuse.
    let work = next(&mut core, 64).unwrap();
    assert_eq!(work.pages.len(), 2);
    assert!(
        work.pages
            .iter()
            .flat_map(|p| &p.chunks)
            .all(|c| c.needs_vector)
    );
    // Clearing an empty index is fine.
    clear(&mut core);
    clear(&mut core);
    assert_eq!(status(&mut core).dirty, 2);
}

// ---- AiRetrieve: validation and keyword only ------------------------------------------------------

#[test]
fn retrieval_validates_its_arguments() {
    let (_fixture, mut core, space) = setup();
    let page = page_with(&mut core, space, "Alpha", &["alpha text"]);
    let ok = |core: &mut Core, query: &str, limit: Option<u32>| {
        retrieve(core, query, None, AiScope::All, limit)
    };
    for (query, limit, field) in [
        ("", None, "query"),
        (&"q".repeat(2_001), None, "query"),
        ("alpha", Some(0), "limit"),
        ("alpha", Some(21), "limit"),
    ] {
        let err = ok(&mut core, query, limit).unwrap_err();
        assert_eq!(invalid_field(&err), Some(field));
    }
    ok(&mut core, "a", Some(1)).unwrap();
    ok(&mut core, &"q".repeat(2_000), Some(20)).unwrap();
    // A non-finite or oversized vector.
    for vector in [vec![f32::NAN; 16], vec![0.1; 8_193]] {
        let err = retrieve(&mut core, "alpha", Some(vector), AiScope::All, None).unwrap_err();
        assert_eq!(invalid_field(&err), Some("vector"));
    }
    // Unknown space and page.
    let err = retrieve(
        &mut core,
        "alpha",
        None,
        AiScope::Space {
            space_id: SpaceId::from_bytes([9; 16]),
        },
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = retrieve(
        &mut core,
        "alpha",
        None,
        AiScope::Page {
            page_id: PageId::from_bytes([9; 16]),
        },
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // The empty vector is no vector; a page that exists but is trashed has no passages.
    assert!(
        !retrieve(&mut core, "alpha", Some(vec![]), AiScope::All, None)
            .unwrap()
            .is_empty()
    );
    core.dispatch(Request::PageTrash { page_id: page }).unwrap();
    assert!(
        retrieve(
            &mut core,
            "alpha",
            None,
            AiScope::Page { page_id: page },
            None
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn without_an_index_the_keyword_search_gives_the_text_of_the_blocks() {
    let (_fixture, mut core, space) = setup();
    let a = page_with(
        &mut core,
        space,
        "Launch plan",
        &["the launch date is in spring", "budget is small"],
    );
    let b = page_with(
        &mut core,
        space,
        "Notes",
        &["we should launch the product soon"],
    );
    let passages = retrieve(
        &mut core,
        "when is the launch happening",
        None,
        AiScope::All,
        None,
    )
    .unwrap();
    // Any-word mode: "launch" alone is enough (the sentence as a whole matches nothing).
    let mut pages = pages_of(&passages);
    pages.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(pages, expected);
    assert_eq!(
        passages.iter().map(|p| p.n).collect::<Vec<_>>(),
        [1, 2],
        "numbered from 1"
    );
    for passage in &passages {
        assert_eq!(passage.space_id, space);
        assert!(passage.block_id.is_some(), "a block hit names its block");
        assert!(passage.score > 0.0);
    }
    let by_page = |id: PageId| passages.iter().find(|p| p.page_id == id).unwrap();
    assert_eq!(by_page(a).text, "the launch date is in spring");
    assert_eq!(by_page(a).title, "Launch plan");
    assert_eq!(by_page(b).text, "we should launch the product soon");
    // A question of short words only searches nothing.
    assert!(
        retrieve(&mut core, "is it to be", None, AiScope::All, None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_passage_of_an_unindexed_page_is_truncated_to_1500_characters() {
    let (_fixture, mut core, space) = setup();
    let long = format!("zebra {}", "x".repeat(4_000));
    page_with(&mut core, space, "Long", &[long.as_str()]);
    let passages = retrieve(&mut core, "zebra", None, AiScope::All, None).unwrap();
    assert_eq!(passages.len(), 1);
    assert_eq!(passages[0].text.chars().count(), 1_500);
    assert!(passages[0].text.starts_with("zebra x"));
}

#[test]
fn a_title_hit_on_a_page_without_chunks_is_the_title_alone() {
    let (_fixture, mut core, space) = setup();
    let a = page_with(&mut core, space, "Quarterly roadmap", &[]);
    let passages = retrieve(&mut core, "roadmap", None, AiScope::All, None).unwrap();
    assert_eq!(pages_of(&passages), [a]);
    assert_eq!(passages[0].block_id, None);
    assert_eq!(passages[0].text, "Quarterly roadmap");
}

// ---- AiRetrieve: semantic, fusion and the chunk mapping -------------------------------------------

/// Three pages about alpha, beta and gamma, indexed.
fn indexed_trio(core: &mut Core, space: SpaceId) -> (PageId, PageId, PageId) {
    ai_on(core, MODEL);
    let a = page_with(core, space, "Alpha", &["alpha facts are written here"]);
    let b = page_with(core, space, "Beta", &["beta facts are written here"]);
    let c = page_with(core, space, "Gamma", &["gamma facts are written here"]);
    assert_eq!(index_all(core), 3);
    (a, b, c)
}

#[test]
fn the_semantic_list_ranks_by_cosine_and_drops_what_is_below_the_threshold() {
    let (_fixture, mut core, space) = setup();
    let (a, b, _c) = indexed_trio(&mut core, space);
    // A query with nothing in common with the words: only the vector finds pages.
    let query = "zzzz";
    let mut mixed = vec![0.0; DIMS];
    mixed[0] = 1.0;
    mixed[1] = 0.5; // cosine with alpha 0.894, with beta 0.447, with gamma 0
    let passages = retrieve(&mut core, query, Some(mixed), AiScope::All, None).unwrap();
    assert_eq!(pages_of(&passages), [a, b], "gamma scores 0, below 0.25");
    assert_eq!(
        passages[0].text, "alpha facts are written here",
        "the title line is dropped"
    );
    assert_eq!(passages[0].title, "Alpha");
    assert!(passages[0].score > passages[1].score);

    // Just below and above the threshold: cosine 0.2 and 0.3 with alpha.
    let against = |cos: f32| {
        let mut v = vec![0.0; DIMS];
        v[0] = cos;
        v[5] = (1.0 - cos * cos).sqrt();
        v
    };
    assert!(
        retrieve(&mut core, query, Some(against(0.2)), AiScope::All, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        pages_of(&retrieve(&mut core, query, Some(against(0.3)), AiScope::All, None).unwrap()),
        [a]
    );
    // The query vector is normalised: its length does not matter.
    let long: Vec<f32> = against(0.9).iter().map(|x| x * 40.0).collect();
    assert_eq!(
        pages_of(&retrieve(&mut core, query, Some(long), AiScope::All, None).unwrap()),
        [a]
    );
}

#[test]
fn a_vector_of_another_dimension_is_ignored_and_the_keywords_still_work() {
    let (_fixture, mut core, space) = setup();
    let (a, _b, _c) = indexed_trio(&mut core, space);
    let wide = vec![1.0; 32];
    let passages = retrieve(&mut core, "alpha", Some(wide), AiScope::All, None).unwrap();
    assert_eq!(
        pages_of(&passages),
        [a],
        "the semantic list is empty; the words find alpha"
    );
}

#[test]
fn the_two_lists_are_fused_and_a_passage_found_by_both_comes_first() {
    let (_fixture, mut core, space) = setup();
    let (a, b, _c) = indexed_trio(&mut core, space);
    // Keywords: only beta's page contains "beta". Semantic: alpha 1.0, beta 0.5 / 1.118.
    let mut v = vec![0.0; DIMS];
    v[0] = 1.0;
    v[1] = 0.6;
    let passages = retrieve(&mut core, "beta question", Some(v), AiScope::All, None).unwrap();
    assert_eq!(passages[0].page_id, b, "beta is in both lists");
    assert_eq!(passages[1].page_id, a, "alpha only in the semantic one");
    assert!(passages[0].score > passages[1].score);
    // The fused score of rank 0 in both lists is 2/61, in one list 1/61.
    assert!((passages[0].score - (1.0 / 61.0 + 1.0 / 62.0)).abs() < 1e-6);
    assert!((passages[1].score - 1.0 / 61.0).abs() < 1e-6);
    assert_eq!(passages.iter().map(|p| p.n).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn a_keyword_hit_maps_to_the_chunk_that_contains_its_block() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let first = format!("alpha {}", "one ".repeat(220)); // about 880 characters
    let second = format!("beta {}", "two ".repeat(220));
    let page = page_with(&mut core, space, "Long", &[first.as_str(), second.as_str()]);
    index_all(&mut core);
    assert_eq!(core.db.ai_chunks_of(page).unwrap().len(), 2, "two chunks");
    let blocks = core.db.ai_chunks_of(page).unwrap();

    // "beta" is in the second block: the passage is the second chunk, starting at that block.
    let passages = retrieve(&mut core, "beta", None, AiScope::All, None).unwrap();
    assert_eq!(passages.len(), 1);
    assert_eq!(passages[0].block_id, blocks[1].block_ids.first().copied());
    assert_eq!(
        passages[0].text,
        second.as_str(),
        "the chunk's text without its title line"
    );

    // A title hit is the first chunk.
    let passages = retrieve(&mut core, "long", None, AiScope::All, None).unwrap();
    assert_eq!(passages.len(), 1);
    assert_eq!(passages[0].block_id, blocks[0].block_ids.first().copied());
    assert!(passages[0].text.starts_with("alpha one"));

    // A block that no stored chunk covers (the page changed since) is returned alone.
    add_text(&mut core, page, "fresh gamma words");
    let passages = retrieve(&mut core, "fresh", None, AiScope::All, None).unwrap();
    assert_eq!(texts(&passages), ["fresh gamma words"]);
    assert!(passages[0].block_id.is_some());
}

#[test]
fn a_chunk_found_by_keyword_and_by_vector_is_one_passage() {
    let (_fixture, mut core, space) = setup();
    let (a, _b, _c) = indexed_trio(&mut core, space);
    let passages = retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, None).unwrap();
    assert_eq!(pages_of(&passages), [a], "one passage, not two");
}

#[test]
fn at_most_three_passages_per_page_and_the_limit_applies() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let long = |c: char| format!("alpha {}", c.to_string().repeat(1_100));
    let paragraphs: Vec<String> = ['a', 'b', 'c', 'd', 'e'].iter().map(|c| long(*c)).collect();
    let refs: Vec<&str> = paragraphs.iter().map(String::as_str).collect();
    let many = page_with(&mut core, space, "Many", &refs);
    let one = page_with(&mut core, space, "One", &["alpha single"]);
    index_all(&mut core);
    assert_eq!(core.db.ai_chunks_of(many).unwrap().len(), 5);

    let passages = retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, None).unwrap();
    let per_page = |id: PageId| passages.iter().filter(|p| p.page_id == id).count();
    assert_eq!(per_page(many), 3);
    assert_eq!(per_page(one), 1);
    assert_eq!(passages.len(), 4);
    assert_eq!(
        passages.iter().map(|p| p.n).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    // The limit counts passages, after the cap per page.
    let two = retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, Some(2)).unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].page_id, passages[0].page_id);
    let default = retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, None).unwrap();
    assert_eq!(
        default, passages,
        "the default limit of 8 is not reached here"
    );

    // The default is 8: ten pages give eight.
    for n in 0..10 {
        page_with(&mut core, space, &format!("Alpha {n}"), &["alpha again"]);
    }
    index_all(&mut core);
    assert_eq!(
        retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, None)
            .unwrap()
            .len(),
        8
    );
    assert_eq!(
        retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, Some(20))
            .unwrap()
            .len(),
        14
    );
}

// ---- AiRetrieve: scopes ----------------------------------------------------------------------------------

#[test]
fn the_scopes_cover_one_space_all_spaces_or_one_page() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let other = create_space(&mut core, "Work").id;
    let a = page_with(&mut core, space, "Alpha home", &["alpha in the home space"]);
    let b = page_with(&mut core, other, "Alpha work", &["alpha in the work space"]);
    let c = page_with(&mut core, other, "Alpha more", &["alpha again in work"]);
    index_all(&mut core);

    let sorted = |passages: &[AiSource]| {
        let mut pages = pages_of(passages);
        pages.sort();
        pages
    };
    let sorted_ids = |ids: &[PageId]| {
        let mut ids = ids.to_vec();
        ids.sort();
        ids
    };
    for vector in [None, Some(axis(0))] {
        let all = retrieve(&mut core, "alpha", vector.clone(), AiScope::All, None).unwrap();
        assert_eq!(sorted(&all), sorted_ids(&[a, b, c]));
        let home = retrieve(
            &mut core,
            "alpha",
            vector.clone(),
            AiScope::Space { space_id: space },
            None,
        )
        .unwrap();
        assert_eq!(pages_of(&home), [a]);
        assert_eq!(home[0].space_id, space);
        let work = retrieve(
            &mut core,
            "alpha",
            vector.clone(),
            AiScope::Space { space_id: other },
            None,
        )
        .unwrap();
        assert_eq!(sorted(&work), sorted_ids(&[b, c]));
        let one = retrieve(
            &mut core,
            "alpha",
            vector.clone(),
            AiScope::Page { page_id: c },
            None,
        )
        .unwrap();
        assert_eq!(pages_of(&one), [c], "that page only");
    }
}

#[test]
fn the_page_scope_finds_the_page_even_when_other_pages_match_better() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    // 120 rows elsewhere that match the word "launch" better than the page does.
    for n in 0..120 {
        page_with(
            &mut core,
            space,
            &format!("Launch launch {n}"),
            &["launch launch launch"],
        );
    }
    let target = page_with(
        &mut core,
        space,
        "Target",
        &["one mention of launch among many words here"],
    );
    let passages = retrieve(
        &mut core,
        "launch",
        None,
        AiScope::Page { page_id: target },
        None,
    )
    .unwrap();
    assert_eq!(pages_of(&passages), [target]);
}

#[test]
fn trashed_pages_are_never_returned_even_when_their_chunks_are_cached() {
    let (_fixture, mut core, space) = setup();
    let (a, b, c) = indexed_trio(&mut core, space);
    let all = retrieve(
        &mut core,
        "facts",
        Some(embed("alpha beta gamma")),
        AiScope::All,
        None,
    )
    .unwrap();
    assert_eq!(all.len(), 3);
    assert!(core.ai_cache_loaded());

    // Trashed after the cache was loaded: filtered at query time, no cache change needed.
    core.dispatch(Request::PageTrash { page_id: b }).unwrap();
    let all = retrieve(
        &mut core,
        "facts",
        Some(embed("alpha beta gamma")),
        AiScope::All,
        None,
    )
    .unwrap();
    let mut pages = pages_of(&all);
    pages.sort();
    let mut expected = vec![a, c];
    expected.sort();
    assert_eq!(pages, expected);
    assert!(
        retrieve(
            &mut core,
            "facts",
            Some(axis(1)),
            AiScope::Page { page_id: b },
            None
        )
        .unwrap()
        .is_empty(),
        "a trashed page given as the scope has nothing"
    );
    // And after it is restored, it comes back.
    core.dispatch(Request::PageRestore { page_id: b }).unwrap();
    assert_eq!(
        retrieve(
            &mut core,
            "facts",
            Some(embed("alpha beta gamma")),
            AiScope::All,
            None
        )
        .unwrap()
        .len(),
        3
    );
}

// ---- the vector cache ----------------------------------------------------------------------------------------

#[test]
fn the_cache_is_loaded_by_the_first_retrieval_and_kept_in_step_by_store_clear_and_the_walk() {
    let (_fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    index_all(&mut core);
    assert!(
        !core.ai_cache_loaded(),
        "nothing loads it before a retrieval"
    );
    assert_eq!(status(&mut core).chunks, 1);
    assert!(!core.ai_cache_loaded(), "the status does not either");

    retrieve(&mut core, "alpha", Some(axis(0)), AiScope::All, None).unwrap();
    assert!(core.ai_cache_loaded());
    assert_eq!(core.ai_cache_shape(), [(a, vec![0])]);

    // A page stored after the load is in the cache and found at once.
    let b = page_with(&mut core, space, "Beta", &["beta text"]);
    index_all(&mut core);
    let mut shape = core.ai_cache_shape();
    shape.sort();
    let mut expected = vec![(a, vec![0]), (b, vec![0])];
    expected.sort();
    assert_eq!(shape, expected);
    let found = retrieve(&mut core, "zzz", Some(axis(1)), AiScope::All, None).unwrap();
    assert_eq!(pages_of(&found), [b]);

    // A page that changes: the new chunks replace the old ones in the cache.
    let long = "alpha ".repeat(190);
    add_text(&mut core, a, &long);
    add_text(&mut core, a, &long.replace("alpha", "gamma"));
    index_all(&mut core);
    let (_, seqs) = core
        .ai_cache_shape()
        .into_iter()
        .find(|(page, _)| *page == a)
        .unwrap();
    assert_eq!(
        seqs,
        [0, 1, 2].as_slice()[..seqs.len()],
        "contiguous chunks from 0"
    );
    assert_eq!(seqs.len(), core.db.ai_chunks_of(a).unwrap().len());
    let found = retrieve(&mut core, "zzz", Some(axis(2)), AiScope::All, None).unwrap();
    assert_eq!(
        pages_of(&found),
        [a],
        "the new vector is the one that is searched"
    );

    // A purged page leaves the cache with the walk.
    core.dispatch(Request::PageTrash { page_id: b }).unwrap();
    core.dispatch(Request::PagePurge { page_id: b }).unwrap();
    next(&mut core, 64).unwrap();
    assert!(core.ai_cache_shape().iter().all(|(page, _)| *page != b));

    // A model change drops the whole cache; the next retrieval loads what is stored (nothing).
    ai_on(&mut core, "other-embed");
    next(&mut core, 64).unwrap();
    assert!(!core.ai_cache_loaded());
    assert!(
        retrieve(&mut core, "zzz", Some(axis(0)), AiScope::All, None)
            .unwrap()
            .is_empty()
    );
    assert!(core.ai_cache_loaded());
    assert!(core.ai_cache_shape().is_empty());
}

#[test]
fn a_new_core_loads_the_stored_vectors_and_finds_pages_by_meaning() {
    let (fixture, mut core, space) = setup();
    ai_on(&mut core, MODEL);
    let a = page_with(&mut core, space, "Alpha", &["alpha text"]);
    page_with(&mut core, space, "Beta", &["beta text"]);
    index_all(&mut core);
    drop(core);

    // The index is stored: a core opened later finds the page by its vector alone.
    let mut core = fixture.open(2, T0 + 1);
    assert!(!core.ai_cache_loaded());
    let found = retrieve(&mut core, "zzz", Some(axis(0)), AiScope::All, None).unwrap();
    assert_eq!(pages_of(&found), [a]);
    assert_eq!(status(&mut core).pages_indexed, 2);
}
