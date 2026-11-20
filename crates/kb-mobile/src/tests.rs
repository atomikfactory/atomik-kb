//! The engine over a real core on a temporary directory, an in-memory keystore and a recording sink.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Condvar;
use std::time::Instant;

use kb_doc::{BlobKind, Doc, DocKind};
use kb_model::{BlockKind, PageDoc};
use kb_types::BlockId;
use serde_json::{Value, json};

use super::*;

const WAIT: Duration = Duration::from_secs(120);
const PASSPHRASE: &str = "correct horse battery";

#[derive(Default)]
struct MemoryKeystore {
    entries: Mutex<HashMap<String, Vec<u8>>>,
}

impl PlatformKeystore for MemoryKeystore {
    fn get(&self, name: String) -> Result<Option<Vec<u8>>, KeystoreFfiError> {
        Ok(self.entries.lock().unwrap().get(&name).cloned())
    }

    fn set(&self, name: String, value: Vec<u8>) -> Result<(), KeystoreFfiError> {
        self.entries.lock().unwrap().insert(name, value);
        Ok(())
    }

    fn delete(&self, name: String) -> Result<(), KeystoreFfiError> {
        self.entries.lock().unwrap().remove(&name);
        Ok(())
    }
}

#[derive(Default)]
struct Recorder {
    seen: Mutex<Seen>,
    changed: Condvar,
}

#[derive(Default)]
struct Seen {
    events: Vec<String>,
    states: Vec<EngineState>,
}

impl EventSink for Recorder {
    fn on_event(&self, event_json: String) {
        self.seen.lock().unwrap().events.push(event_json);
        self.changed.notify_all();
    }

    fn on_state(&self, state: EngineState) {
        self.seen.lock().unwrap().states.push(state);
        self.changed.notify_all();
    }
}

impl Recorder {
    /// Waits until the last state satisfies `wanted`.
    fn wait_state(&self, wanted: impl Fn(&EngineState) -> bool) {
        let deadline = Instant::now() + WAIT;
        let mut seen = self.seen.lock().unwrap();
        while !seen.states.last().is_some_and(&wanted) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out; states: {:?}", seen.states);
            seen = self.changed.wait_timeout(seen, left).unwrap().0;
        }
    }

    /// Waits until `count` events of `kind` were delivered.
    fn wait_events(&self, kind: &str, count: usize) {
        let deadline = Instant::now() + WAIT;
        let mut seen = self.seen.lock().unwrap();
        while types(&seen.events)
            .iter()
            .filter(|seen| *seen == kind)
            .count()
            < count
        {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out; events: {:?}",
                types(&seen.events)
            );
            seen = self.changed.wait_timeout(seen, left).unwrap().0;
        }
    }

    fn event_types(&self) -> Vec<String> {
        types(&self.seen.lock().unwrap().events)
    }

    fn states(&self) -> Vec<EngineState> {
        self.seen.lock().unwrap().states.clone()
    }
}

fn types(events: &[String]) -> Vec<String> {
    events
        .iter()
        .map(|json| {
            let value: Value = serde_json::from_str(json).unwrap();
            value["type"].as_str().unwrap().to_owned()
        })
        .collect()
}

struct Fixture {
    engine: Arc<KbEngine>,
    sink: Arc<Recorder>,
    keystore: Arc<MemoryKeystore>,
    dir: tempfile::TempDir,
}

fn config(dir: &tempfile::TempDir) -> EngineConfig {
    EngineConfig {
        data_dir: dir.path().join("data").display().to_string(),
        platform: "android".to_owned(),
    }
}

fn start() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let keystore = Arc::new(MemoryKeystore::default());
    let sink = Arc::new(Recorder::default());
    let engine = KbEngine::new(config(&dir), keystore.clone(), sink.clone());
    sink.wait_state(|state| *state != EngineState::Opening);
    Fixture {
        engine,
        sink,
        keystore,
        dir,
    }
}

impl Fixture {
    fn call(&self, request: &Value) -> Value {
        let answer = self.engine.dispatch(request.to_string());
        serde_json::from_str(&answer).unwrap()
    }

    /// The response of a request that must succeed.
    fn ok(&self, request: &Value) -> Value {
        let answer = self.call(request);
        assert!(answer.get("err").is_none(), "{answer}");
        answer["ok"].clone()
    }

    /// A workspace with one space and one page; returns their ids.
    fn setup_page(&self) -> (String, String) {
        self.ok(
            &json!({"type": "WorkspaceCreate", "data": {"name": "Me", "firstSpaceName": "Notes"}}),
        );
        let spaces = self.ok(&json!({"type": "SpaceList"}));
        let space = spaces["data"][0]["id"].as_str().unwrap().to_owned();
        let page =
            self.ok(&json!({"type": "PageCreate", "data": {"spaceId": space, "title": "Hello"}}));
        (space, page["data"]["id"].as_str().unwrap().to_owned())
    }
}

#[test]
fn a_fresh_data_dir_opens_ready_and_reports_it() {
    let f = start();
    assert_eq!(f.engine.state(), EngineState::Ready);
    f.sink.wait_state(|state| *state == EngineState::Ready);
    assert_eq!(
        f.sink.states(),
        vec![EngineState::Opening, EngineState::Ready]
    );
    // The key went to the app's keystore.
    assert!(!f.keystore.entries.lock().unwrap().is_empty());
    assert!(f.dir.path().join("data").join("install.json").exists());
}

#[test]
fn a_workspace_a_space_and_a_page_through_dispatch_json() {
    let f = start();
    let (space, page) = f.setup_page();
    assert!(!space.is_empty() && !page.is_empty());
    let spaces = f.ok(&json!({"type": "SpaceList"}));
    assert_eq!(spaces["type"], "Spaces");
    assert_eq!(spaces["data"][0]["name"], "Notes");
}

#[test]
fn the_page_document_round_trips_through_open_import_and_export() {
    let f = start();
    let (_, page) = f.setup_page();

    let snapshot = f.engine.doc_open(page.clone()).unwrap();
    assert!(!snapshot.is_empty());

    // The editor's replica adds a block and sends the update.
    let blob = kb_doc::encode_blob(BlobKind::Snapshot, &snapshot);
    let replica =
        PageDoc::from_doc(Doc::load(DocKind::Page, 0x42, Some(&blob), &[]).unwrap()).unwrap();
    let before = replica.doc().version();
    let blocks = replica.block_ids();
    replica
        .insert_block(
            blocks.last().copied(),
            BlockKind::Paragraph,
            "from the phone",
            BlockId::from_random_bytes([7; 16]),
        )
        .unwrap();
    let update = replica
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    f.engine.doc_import(page.clone(), payload.to_vec()).unwrap();

    // What the core holds beyond the replica's old version is the new block; beyond its current one, nothing.
    let delta = f.engine.doc_export(page.clone(), before.0.clone()).unwrap();
    assert!(!delta.is_empty());
    let nothing = f
        .engine
        .doc_export(page.clone(), replica.doc().version().0)
        .unwrap();
    assert!(nothing.is_empty());

    // The content persisted: a closed and reopened document holds the new block.
    f.engine.doc_close(page.clone());
    let reopened = f.engine.doc_open(page).unwrap();
    let blob = kb_doc::encode_blob(BlobKind::Snapshot, &reopened);
    let again =
        PageDoc::from_doc(Doc::load(DocKind::Page, 0x43, Some(&blob), &[]).unwrap()).unwrap();
    assert_eq!(again.block_ids().len(), blocks.len() + 1);

    // The import told the sink about the page.
    f.sink.wait_events("PageContentChanged", 1);
}

#[test]
fn events_reach_the_sink_in_emission_order() {
    let f = start();
    f.setup_page();
    f.sink.wait_events("StructureChanged", 1);
    let kinds = f.sink.event_types();
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("WorkspaceCreated"),
        "{kinds:?}"
    );
    let created = kinds
        .iter()
        .position(|kind| kind == "WorkspaceCreated")
        .unwrap();
    let structure = kinds
        .iter()
        .position(|kind| kind == "StructureChanged")
        .unwrap();
    assert!(created < structure, "{kinds:?}");
}

#[test]
fn the_app_lock_locks_and_unlocks_with_the_passphrase() {
    let f = start();
    let (_, page) = f.setup_page();

    // Not enabled yet: locking does nothing.
    f.engine.lock();
    assert_eq!(f.engine.state(), EngineState::Ready);

    f.ok(&json!({"type": "AppLockEnable", "data": {"passphrase": PASSPHRASE}}));
    f.engine.lock();
    assert_eq!(f.engine.state(), EngineState::Locked);
    f.sink.wait_state(|state| *state == EngineState::Locked);

    let answer = f.call(&json!({"type": "SpaceList"}));
    assert_eq!(answer["err"]["code"], "Locked", "{answer}");
    assert!(matches!(
        f.engine.doc_open(page.clone()),
        Err(EngineError::Locked)
    ));

    match f.engine.unlock("not the passphrase".to_owned()) {
        Err(EngineError::Api { json }) => {
            let error: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(error["code"], "InvalidInput");
            assert_eq!(error["details"]["field"], "passphrase");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(f.engine.state(), EngineState::Locked);

    f.engine.unlock(PASSPHRASE.to_owned()).unwrap();
    assert_eq!(f.engine.state(), EngineState::Ready);
    f.sink.wait_state(|state| *state == EngineState::Ready);
    assert!(f.engine.doc_open(page).is_ok());
    let spaces = f.ok(&json!({"type": "SpaceList"}));
    assert_eq!(spaces["data"][0]["name"], "Notes");
}

#[test]
fn an_engine_restarted_on_a_locked_workspace_opens_locked() {
    let f = start();
    f.setup_page();
    f.ok(&json!({"type": "AppLockEnable", "data": {"passphrase": PASSPHRASE}}));
    let Fixture {
        engine,
        keystore,
        dir,
        ..
    } = f;
    drop(engine);

    let sink = Arc::new(Recorder::default());
    let engine = KbEngine::new(config(&dir), keystore, sink.clone());
    sink.wait_state(|state| *state != EngineState::Opening);
    assert_eq!(engine.state(), EngineState::Locked);
    engine.unlock(PASSPHRASE.to_owned()).unwrap();
    assert_eq!(engine.state(), EngineState::Ready);
}

#[test]
fn malformed_requests_are_errors_not_panics() {
    let f = start();
    for bad in [
        "",
        "not json",
        "{}",
        r#"{"type":"NoSuchRequest"}"#,
        r#"{"type":"AppLockEnable","data":{"passphrase":7}}"#,
        r#"{"type":"SpaceList"} trailing"#,
    ] {
        let answer: Value = serde_json::from_str(&f.engine.dispatch(bad.to_owned())).unwrap();
        assert_eq!(answer["err"]["code"], "InvalidInput", "{bad}: {answer}");
        assert!(answer.get("ok").is_none());
    }
    // The engine still works.
    f.ok(&json!({"type": "WorkspaceGet"}));
}

#[test]
fn bad_ids_and_empty_pieces_are_api_errors() {
    let f = start();
    assert!(matches!(
        f.engine.doc_open("nope".to_owned()),
        Err(EngineError::Api { .. })
    ));
    let (_, page) = f.setup_page();
    assert!(matches!(
        f.engine.doc_import(page, Vec::new()),
        Err(EngineError::Api { .. })
    ));
    let unknown = "12345678-9abc-4def-8123-456789abcdef";
    assert!(matches!(
        f.engine.attachment_read(unknown.to_owned(), Some(1), None),
        Err(EngineError::Api { .. })
    ));
    assert!(matches!(
        f.engine.attachment_write(unknown.to_owned(), Vec::new()),
        Err(EngineError::Api { .. })
    ));
}

#[test]
fn an_attachment_uploads_and_reads_back_whole_and_by_range() {
    let f = start();
    let (space, _) = f.setup_page();
    let started = f.ok(&json!({
        "type": "AttachmentBegin",
        "data": {"spaceId": space, "name": "hello.txt", "mime": "text/plain"}
    }));
    let id = started["data"].as_str().unwrap().to_owned();
    f.engine
        .attachment_write(id.clone(), b"hello ".to_vec())
        .unwrap();
    f.engine
        .attachment_write(id.clone(), b"phone".to_vec())
        .unwrap();
    f.ok(&json!({"type": "AttachmentFinish", "data": {"attachmentId": id}}));

    assert_eq!(
        f.engine.attachment_read(id.clone(), None, None).unwrap(),
        b"hello phone"
    );
    assert_eq!(
        f.engine.attachment_read(id, Some(6), Some(11)).unwrap(),
        b"phone"
    );
}

#[test]
fn sync_once_without_a_server_is_idle_at_once() {
    let f = start();
    f.setup_page();
    assert!(f.engine.sync_once(2_000));
}

#[test]
fn a_keystore_that_fails_makes_the_engine_failed() {
    struct Broken;
    impl PlatformKeystore for Broken {
        fn get(&self, _: String) -> Result<Option<Vec<u8>>, KeystoreFfiError> {
            Err(KeystoreFfiError::Unavailable("the key is gone".to_owned()))
        }
        fn set(&self, _: String, _: Vec<u8>) -> Result<(), KeystoreFfiError> {
            Err(KeystoreFfiError::Failed("no".to_owned()))
        }
        fn delete(&self, _: String) -> Result<(), KeystoreFfiError> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(Recorder::default());
    let engine = KbEngine::new(config(&dir), Arc::new(Broken), sink.clone());
    sink.wait_state(|state| *state != EngineState::Opening);
    assert!(matches!(engine.state(), EngineState::Failed { .. }));
    let answer: Value =
        serde_json::from_str(&engine.dispatch(r#"{"type":"SpaceList"}"#.to_owned())).unwrap();
    assert!(answer.get("err").is_some());
}
