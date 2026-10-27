//! Tests of the semantic index's worker with a fake core and a stub Ollama. The worker runs on the
//! runtime tauri owns (as in the app); the tests wait for what it does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_types::ai::{AiIndexChunk, AiIndexWork, AiSettings};
use kb_types::testing::FixedClock;
use kb_types::{PageId, SpaceId};

use super::*;
use crate::ai_client::{AiClient, Timeouts};
use crate::ai_test_support::{
    AiFake, ChatBehaviour, SharedScript, Stub, ai_on, put_setting, ready_host, requests_of, script,
    start_stub,
};
use crate::state::{AppState, all_of};

/// Waits (on this thread) until `check` holds or `within` passes.
fn wait_until(within: Duration, mut check: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < within {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    check()
}

fn timing() -> IndexerTiming {
    IndexerTiming {
        debounce: Duration::from_millis(250),
        tick: Duration::from_secs(30),
        backoff_first: Duration::from_millis(150),
        backoff_max: Duration::from_millis(400),
        pause: Duration::from_millis(5),
    }
}

fn quick() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(1),
        plain: Duration::from_secs(5),
        meta: Duration::from_secs(5),
        first_byte: Duration::from_secs(5),
        between_chunks: Duration::from_secs(5),
        show_cache: Duration::from_secs(60),
    }
}

/// Everything a test needs: the state, the script, the stub and the shell. The worker is not started
/// until `start`.
struct Rig {
    state: Arc<AppState<AiFake>>,
    host: Arc<dyn CoreHost>,
    script: SharedScript,
    shell: Arc<AiShell>,
    stub: Stub,
}

fn rig() -> Rig {
    let stub = tauri::async_runtime::block_on(start_stub(ChatBehaviour::Json(String::new())));
    let (state, host, script) = ready_host();
    ai_on(
        &script,
        &stub.url(),
        Some("chat-model"),
        Some("nomic-embed-text"),
    );
    let shell = Arc::new(AiShell::with_client(AiClient::with_timeouts(quick())));
    Rig {
        state,
        host,
        script,
        shell,
        stub,
    }
}

impl Rig {
    fn start(&self, timing: IndexerTiming) {
        self.state.set_background(all_of(vec![indexer_starter(
            Arc::downgrade(&self.host),
            Arc::clone(&self.shell),
            Arc::new(FixedClock::new(1234)),
            timing,
        )]));
    }

    fn calls(&self, kind: &str) -> usize {
        requests_of(&self.script, kind).len()
    }

    /// What the core is told by the app's webview-less side: an event the worker listens to.
    fn emit(&self, event: Event) {
        script(&self.script).events.push(event);
        // Any call drains and publishes what the core emitted.
        let _ = self.host.dispatch(Request::SettingsGet {
            key: "ui.theme".into(),
        });
    }
}

fn page(id: u8, chunks: &[(&str, bool)]) -> AiIndexPage {
    AiIndexPage {
        page_id: PageId::from_bytes([id; 16]),
        source_hash: format!("{id:02x}").repeat(32),
        chunks: chunks
            .iter()
            .enumerate()
            .map(|(seq, (text, needs_vector))| AiIndexChunk {
                seq: u32::try_from(seq).unwrap(),
                hash: format!("{seq:02x}").repeat(32),
                text: (*text).to_owned(),
                needs_vector: *needs_vector,
            })
            .collect(),
    }
}

fn work(pages: Vec<AiIndexPage>, remaining: u32) -> AiIndexWork {
    AiIndexWork {
        model: Some("nomic-embed-text".into()),
        pages,
        remaining,
    }
}

fn stores(rig: &Rig) -> Vec<(PageId, String, String, Vec<kb_types::ai::AiChunkVector>)> {
    requests_of(&rig.script, "AiIndexStore")
        .into_iter()
        .map(|request| match request {
            Request::AiIndexStore {
                page_id,
                source_hash,
                model,
                vectors,
            } => (page_id, source_hash, model, vectors),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn the_chunks_that_need_a_vector_are_embedded_with_the_document_prefix_and_the_page_is_stored() {
    let rig = rig();
    script(&rig.script).index_work.push_back(work(
        vec![page(
            1,
            &[
                ("Title\nfirst", true),
                ("Title\nsecond", false),
                ("Title\nthird", true),
            ],
        )],
        0,
    ));
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));

    let sent = rig.stub.state.received("/api/embed");
    assert_eq!(sent.len(), 1, "one call for the two chunks");
    assert_eq!(sent[0].body["model"], "nomic-embed-text");
    assert_eq!(
        sent[0].body["input"],
        serde_json::json!([
            "search_document: Title\nfirst",
            "search_document: Title\nthird"
        ])
    );
    assert_eq!(sent[0].body["truncate"], true);

    let stored = stores(&rig);
    assert_eq!(stored[0].0, PageId::from_bytes([1; 16]));
    assert_eq!(stored[0].1, "01".repeat(32));
    assert_eq!(stored[0].2, "nomic-embed-text");
    // Only the chunks that needed a vector carry one, by their position in the page.
    let seqs: Vec<u32> = stored[0].3.iter().map(|vector| vector.seq).collect();
    assert_eq!(seqs, [0, 2]);
    assert!(stored[0].3.iter().all(|vector| vector.vector.len() == 16));

    // The work is done: idle, with the time of the batch.
    assert!(wait_until(Duration::from_secs(2), || {
        let state = rig.shell.indexer_state();
        state.phase == AiIndexerPhase::Idle && state.last_run_at == Some(1234)
    }));
    assert!(rig.shell.indexer_state().error.is_none());
    // The first request was the maximum of 16 chunks.
    assert!(matches!(
        requests_of(&rig.script, "AiIndexNext")[0],
        Request::AiIndexNext { max_chunks: 16 }
    ));
}

#[test]
fn a_page_with_many_chunks_is_embedded_16_at_a_time() {
    let rig = rig();
    let texts: Vec<String> = (0..40).map(|n| format!("chunk {n}")).collect();
    let chunks: Vec<(&str, bool)> = texts.iter().map(|text| (text.as_str(), true)).collect();
    script(&rig.script)
        .index_work
        .push_back(work(vec![page(2, &chunks)], 0));
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));
    let sizes: Vec<usize> = rig
        .stub
        .state
        .received("/api/embed")
        .iter()
        .map(|call| call.body["input"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, [16, 16, 8]);
    assert_eq!(stores(&rig)[0].3.len(), 40);
}

#[test]
fn a_page_whose_vectors_are_all_known_is_stored_with_none() {
    let rig = rig();
    script(&rig.script)
        .index_work
        .push_back(work(vec![page(3, &[("a", false), ("b", false)])], 0));
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));
    assert!(rig.stub.state.received("/api/embed").is_empty());
    assert!(stores(&rig)[0].3.is_empty());
}

#[test]
fn the_worker_goes_on_while_pages_remain() {
    let rig = rig();
    {
        let mut core = script(&rig.script);
        core.index_work
            .push_back(work(vec![page(1, &[("one", true)])], 2));
        core.index_work
            .push_back(work(vec![page(2, &[("two", true)])], 1));
        core.index_work
            .push_back(work(vec![page(3, &[("three", true)])], 0));
    }
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 3));
    let pages: Vec<PageId> = stores(&rig).iter().map(|stored| stored.0).collect();
    assert_eq!(
        pages,
        [
            PageId::from_bytes([1; 16]),
            PageId::from_bytes([2; 16]),
            PageId::from_bytes([3; 16])
        ]
    );
    // After the third batch (nothing remaining) the worker asks once more and the core has no pages:
    // that answer, not the count, is what ends the run.
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        == 4));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.calls("AiIndexNext"), 4);
}

#[test]
fn a_page_without_chunks_is_stored_with_no_vectors_and_a_batch_that_comes_back_ends_the_run() {
    let rig = rig();
    // The same page is offered again and again (it did not become clean): one store, then the run ends
    // instead of spinning.
    let again = work(vec![page(8, &[])], 0);
    script(&rig.script).answers.push(Box::new(move |request| {
        matches!(request, Request::AiIndexNext { .. })
            .then(|| Ok(Response::AiIndexWork(again.clone())))
    }));
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(rig.calls("AiIndexStore"), 1);
    assert_eq!(rig.calls("AiIndexNext"), 2);
    assert!(rig.stub.state.received("/api/embed").is_empty());
    let stored = stores(&rig);
    assert_eq!(stored[0].0, PageId::from_bytes([8; 16]));
    assert!(
        stored[0].3.is_empty(),
        "stored even with no chunk and no vector"
    );
    assert_eq!(rig.shell.indexer_state().phase, AiIndexerPhase::Idle);
}

#[test]
fn while_ai_or_the_index_is_off_the_worker_is_idle_and_never_calls_ollama() {
    let rig = rig();
    // The core answers an empty work with no model: AI off, index off, or no embedding model.
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        >= 1));
    std::thread::sleep(Duration::from_millis(100));
    assert!(rig.stub.state.requests.lock().unwrap().is_empty());
    let state = rig.shell.indexer_state();
    assert_eq!((state.phase, state.error), (AiIndexerPhase::Idle, None));
    // Not a failure: no backoff, a settings change (AI turned on) is looked at after the debounce.
    script(&rig.script)
        .index_work
        .push_back(work(vec![page(7, &[("seven", true)])], 0));
    let value = serde_json::to_value(AiSettings {
        enabled: true,
        embedding_model: Some("nomic-embed-text".into()),
        ..AiSettings::default()
    })
    .unwrap();
    rig.host
        .dispatch(Request::SettingsSet {
            key: AI_SETTINGS_SETTING.into(),
            value,
        })
        .unwrap();
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));
}

#[test]
fn a_failure_is_an_error_state_without_page_text_or_host_and_the_worker_tries_again_with_a_growing_wait()
 {
    let rig = rig();
    // The same work is offered until it is stored.
    let stored = Arc::new(AtomicBool::new(false));
    {
        let stored = Arc::clone(&stored);
        let work = work(vec![page(4, &[("SECRET PAGE TEXT", true)])], 0);
        script(&rig.script)
            .answers
            .push(Box::new(move |request| match request {
                Request::AiIndexNext { .. } => Some(Ok(Response::AiIndexWork(
                    if stored.load(Ordering::SeqCst) {
                        AiIndexWork::default()
                    } else {
                        work.clone()
                    },
                ))),
                Request::AiIndexStore { .. } => {
                    stored.store(true, Ordering::SeqCst);
                    Some(Ok(Response::Ok))
                }
                _ => None,
            }));
    }
    *rig.stub.state.embed_status.lock().unwrap() = 500;
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || {
        rig.shell.indexer_state().phase == AiIndexerPhase::Error
    }));
    let state = rig.shell.indexer_state();
    let message = state.error.unwrap();
    assert!(
        !message.contains("SECRET") && !message.contains("127.0.0.1"),
        "{message}"
    );
    assert!(state.last_run_at.is_none());

    // It tries again by itself: after 150 ms, then 300 ms, then at most 400 ms.
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        >= 3));
    // The third attempt's embedding request may still be on its way when its AiIndexNext is
    // counted: wait for it rather than count at once (flaky under a loaded test run).
    assert!(
        wait_until(Duration::from_secs(5), || rig
            .stub
            .state
            .received("/api/embed")
            .len()
            >= 3),
        "{}",
        rig.stub.state.received("/api/embed").len()
    );
    // Ollama comes back: the worker recovers, stores the page and is idle with no error.
    *rig.stub.state.embed_status.lock().unwrap() = 200;
    assert!(wait_until(Duration::from_secs(5), || stored.load(Ordering::SeqCst)));
    assert!(wait_until(Duration::from_secs(5), || {
        let state = rig.shell.indexer_state();
        state.phase == AiIndexerPhase::Idle
            && state.error.is_none()
            && state.last_run_at == Some(1234)
    }));
}

#[test]
fn a_content_event_does_not_shorten_the_wait_after_a_failure_but_a_settings_change_ends_it() {
    let rig = rig();
    let work = work(vec![page(5, &[("text", true)])], 0);
    {
        let work = work.clone();
        script(&rig.script)
            .answers
            .push(Box::new(move |request| match request {
                Request::AiIndexNext { .. } => Some(Ok(Response::AiIndexWork(work.clone()))),
                _ => None,
            }));
    }
    *rig.stub.state.embed_status.lock().unwrap() = 500;
    // A long wait after the first failure.
    rig.start(IndexerTiming {
        backoff_first: Duration::from_secs(30),
        backoff_max: Duration::from_secs(30),
        ..timing()
    });
    assert!(wait_until(Duration::from_secs(5), || {
        rig.shell.indexer_state().phase == AiIndexerPhase::Error
    }));
    let before = rig.calls("AiIndexNext");

    // The person types: after the debounce the worker looks, and sees it must wait.
    rig.emit(Event::PageContentChanged {
        page_id: PageId::from_bytes([5; 16]),
        source: kb_types::ChangeSource::Editor,
    });
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        rig.calls("AiIndexNext"),
        before,
        "content events do not end a backoff"
    );

    // A change of the AI settings does.
    rig.emit(Event::SettingChanged {
        key: AI_SETTINGS_SETTING.into(),
    });
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        > before));
    // So does a change of the endpoint.
    let before = rig.calls("AiIndexNext");
    rig.emit(Event::SettingChanged {
        key: AI_ENDPOINT_SETTING.into(),
    });
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        > before));
}

#[test]
fn events_are_debounced_and_other_settings_are_ignored() {
    let rig = rig();
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        == 1));

    // Events 100 ms apart, each inside the 250 ms debounce: one look, after the last.
    for _ in 0..3 {
        rig.emit(Event::StructureChanged {
            space_id: SpaceId::from_bytes([1; 16]),
        });
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(rig.calls("AiIndexNext"), 1, "still typing");
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        == 2));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(rig.calls("AiIndexNext"), 2, "once, not once per event");

    // Settings that are not the AI's, and events that are not about pages, wake nothing.
    for event in [
        Event::SettingChanged {
            key: "ui.theme".into(),
        },
        Event::McpChanged,
        Event::SyncChanged,
    ] {
        rig.emit(event);
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(rig.calls("AiIndexNext"), 2);
}

#[test]
fn a_cleared_index_wakes_an_idle_worker_after_the_debounce_not_at_the_next_tick() {
    let rig = rig();
    // Idle: the first look finds nothing. The tick is long, so only the event can bring the next look.
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        == 1));
    // Settings → Rebuild: the core clears the index (AiIndexChanged) and everything is stale again.
    script(&rig.script)
        .index_work
        .push_back(work(vec![page(9, &[("nine", true)])], 0));
    let cleared = Instant::now();
    rig.emit(Event::AiIndexChanged);
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));
    // After the 250 ms debounce, long before the 30 s tick.
    assert!(
        cleared.elapsed() >= Duration::from_millis(200),
        "{:?}",
        cleared.elapsed()
    );
    assert!(
        cleared.elapsed() < Duration::from_secs(5),
        "{:?}",
        cleared.elapsed()
    );
    assert_eq!(stores(&rig)[0].0, PageId::from_bytes([9; 16]));
}

#[test]
fn the_index_events_of_a_run_do_not_wake_the_worker_again() {
    let rig = rig();
    script(&rig.script)
        .index_work
        .push_back(work(vec![page(1, &[("one", true)])], 0));
    // The embedding takes a while: the events arrive while the worker is in its run, as those of its
    // own AiIndexNext and AiIndexStore do (the core emits them before the call returns).
    *rig.stub.state.embed_delay.lock().unwrap() = Duration::from_millis(400);
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .stub
        .state
        .received("/api/embed")
        .len()
        == 1));
    for _ in 0..3 {
        rig.emit(Event::AiIndexChanged);
    }
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));
    // The run ends with one more AiIndexNext (the core has no pages); then nothing: no run is
    // scheduled by the events, even well past the 250 ms debounce.
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        == 2));
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(rig.calls("AiIndexNext"), 2);
    assert_eq!(rig.calls("AiIndexStore"), 1);
    assert_eq!(rig.shell.indexer_state().phase, AiIndexerPhase::Idle);
}

#[test]
fn the_listener_hears_the_index_only_while_the_worker_is_idle() {
    let wake = Wake::default();
    // Idle: a change of the index is a reason to look (no flag, so a run in progress is not ended).
    assert!(!wake.running.load(Ordering::SeqCst));
    wake.hear(&Event::AiIndexChanged);
    assert!(!wake.content.load(Ordering::SeqCst) && !wake.settings.load(Ordering::SeqCst));
    // The permit is stored: the next wait ends at once.
    let woken = tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_millis(100), wake.notify.notified())
            .await
            .is_ok()
    });
    assert!(woken);
    // Running: it is the worker's own, and nothing is stored.
    wake.running.store(true, Ordering::SeqCst);
    wake.hear(&Event::AiIndexChanged);
    let woken = tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_millis(100), wake.notify.notified())
            .await
            .is_ok()
    });
    assert!(!woken);
}

#[test]
fn the_worker_looks_every_tick_without_events() {
    let rig = rig();
    rig.start(IndexerTiming {
        tick: Duration::from_millis(120),
        ..timing()
    });
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        >= 3));
}

#[test]
fn an_edit_during_a_run_ends_the_run_at_the_next_batch_boundary() {
    let rig = rig();
    {
        let mut core = script(&rig.script);
        core.index_work
            .push_back(work(vec![page(1, &[("one", true)])], 5));
        core.index_work
            .push_back(work(vec![page(2, &[("two", true)])], 0));
    }
    // The first embedding takes a while, so the person edits while it is in flight.
    *rig.stub.state.embed_delay.lock().unwrap() = Duration::from_millis(300);
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .stub
        .state
        .received("/api/embed")
        .len()
        == 1));
    rig.emit(Event::PageContentChanged {
        page_id: PageId::from_bytes([1; 16]),
        source: kb_types::ChangeSource::Editor,
    });
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 1));
    let stored_at = Instant::now();
    // Pages remain (5), but the content changed: the next look comes after the 250 ms debounce, not
    // after the 5 ms pause between two batches.
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        == 2));
    assert!(
        stored_at.elapsed() >= Duration::from_millis(200),
        "{:?}",
        stored_at.elapsed()
    );
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexStore")
        == 2));
}

#[test]
fn the_listener_reacts_to_the_events_that_matter_and_to_nothing_else() {
    let wake = Wake::default();
    for event in [
        Event::AiIndexChanged,
        Event::McpChanged,
        Event::SyncChanged,
        Event::SettingChanged {
            key: "ui.theme".into(),
        },
    ] {
        wake.hear(&event);
    }
    assert!(!wake.content.load(Ordering::SeqCst) && !wake.settings.load(Ordering::SeqCst));
    wake.hear(&Event::PageContentChanged {
        page_id: PageId::from_bytes([1; 16]),
        source: kb_types::ChangeSource::Editor,
    });
    assert!(wake.content.load(Ordering::SeqCst) && !wake.settings.load(Ordering::SeqCst));
    wake.content.store(false, Ordering::SeqCst);
    wake.hear(&Event::StructureChanged {
        space_id: SpaceId::from_bytes([1; 16]),
    });
    assert!(wake.content.load(Ordering::SeqCst));
    for key in [AI_SETTINGS_SETTING, AI_ENDPOINT_SETTING] {
        let wake = Wake::default();
        wake.hear(&Event::SettingChanged { key: key.into() });
        assert!(wake.settings.load(Ordering::SeqCst), "{key}");
        assert!(!wake.content.load(Ordering::SeqCst), "{key}");
    }
}

#[test]
fn an_endpoint_without_consent_is_an_error_and_ollama_is_not_called() {
    let rig = rig();
    put_setting(
        &rig.script,
        kb_types::ai::AI_ENDPOINT_SETTING,
        serde_json::json!({ "url": "http://ollama-nas.invalid:11434", "remoteConsent": false }),
    );
    script(&rig.script)
        .index_work
        .push_back(work(vec![page(6, &[("six", true)])], 0));
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || {
        rig.shell.indexer_state().phase == AiIndexerPhase::Error
    }));
    assert_eq!(
        rig.shell.indexer_state().error.as_deref(),
        Some("consent required")
    );
    assert_eq!(rig.calls("AiIndexStore"), 0);
}

#[test]
fn a_core_that_fails_is_an_error_state_too() {
    let rig = rig();
    script(&rig.script).answers.push(Box::new(|request| {
        matches!(request, Request::AiIndexNext { .. })
            .then(|| Err(ApiError::internal("not implemented")))
    }));
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || {
        rig.shell.indexer_state().phase == AiIndexerPhase::Error
    }));
    assert_eq!(
        rig.shell.indexer_state().error.as_deref(),
        Some("not implemented")
    );
}

#[test]
fn locking_stops_the_worker_cancels_the_streams_and_forgets_the_state_and_unlocking_starts_it_again()
 {
    let rig = rig();
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        >= 1));
    assert_eq!(rig.state.listener_count(), 1);
    // A stream is running and the indexer reported something.
    let guard = rig.shell.track("busy-stream").unwrap();
    rig.shell.update_indexer(|state| {
        state.phase = AiIndexerPhase::Indexing;
        state.last_run_at = Some(9);
    });
    assert!(!guard.token.is_cancelled());

    rig.host.lock().unwrap();
    assert!(
        guard.token.is_cancelled(),
        "a stream of an open workspace is not kept"
    );
    assert_eq!(rig.state.listener_count(), 0);
    assert_eq!(
        rig.shell.indexer_state(),
        kb_types::ai::AiIndexerState::default()
    );
    // Nothing runs while locked: the worker made its last call.
    std::thread::sleep(Duration::from_millis(150));
    let calls = rig.calls("AiIndexNext");
    // An event while locked reaches nobody.
    script(&rig.script).events.push(Event::AiIndexChanged);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(rig.calls("AiIndexNext"), calls);

    // Unlocking starts a worker again, and it looks at once.
    rig.host.unlock("passphrase").unwrap();
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        > calls));
    assert_eq!(rig.state.listener_count(), 1);
    drop(guard);
}

#[test]
fn the_worker_holds_the_state_only_weakly() {
    let rig = rig();
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || rig
        .calls("AiIndexNext")
        >= 1));
    let weak = Arc::downgrade(&rig.state);
    let Rig {
        state,
        host,
        script: _script,
        shell: _shell,
        stub: _stub,
    } = rig;
    drop(host);
    drop(state);
    // No cycle: the state, the starter it holds and the worker it started all hold it weakly.
    assert!(wait_until(Duration::from_secs(2), || weak
        .upgrade()
        .is_none()));
}

#[test]
fn the_back_off_doubles_from_60_seconds_to_10_minutes() {
    let timing = IndexerTiming::production();
    let secs: Vec<u64> = (1..=8)
        .map(|failures| timing.backoff(failures).as_secs())
        .collect();
    assert_eq!(secs, [60, 120, 240, 480, 600, 600, 600, 600]);
    assert_eq!(timing.backoff(0).as_secs(), 60);
    assert_eq!(timing.backoff(u32::MAX).as_secs(), 600);
    assert_eq!(timing.debounce, Duration::from_secs(5));
    assert_eq!(timing.tick, Duration::from_secs(60));
    assert_eq!(timing.pause, Duration::from_millis(200));
}

#[test]
fn a_core_that_answers_locked_is_reported_by_its_own_message() {
    // A locked core answers `Locked` to the worker's request; that is what the state reports.
    let rig = rig();
    script(&rig.script).answers.push(Box::new(|request| {
        matches!(request, Request::AiIndexNext { .. }).then(|| Err(ApiError::locked()))
    }));
    rig.start(timing());
    assert!(wait_until(Duration::from_secs(5), || {
        rig.shell.indexer_state().phase == AiIndexerPhase::Error
    }));
    assert_eq!(
        rig.shell.indexer_state().error.as_deref(),
        Some("the workspace is locked")
    );
}
