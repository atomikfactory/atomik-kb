//! A host-side stand-in for the desktop in the Android smoke (scripts/smoke/android-sync.ps1): a
//! `KbEngine` on its own data directory that takes one command per file from a directory, so a
//! script can make it the account's first device, sponsor the phone and edit pages.
//!
//! `desk <data_dir> <cmd_dir>`: every `<n>.cmd` (one line) is answered by `<n>.out`, in name order.
//! Verbs: `raw <Request json>` (the core's JSON door), `state`, `append <pageId> <text>` (adds a
//! paragraph through a replica, as the editor does), `events` (the events since the last call),
//! `quit`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kb_doc::{BlobKind, Doc, DocKind};
use kb_mobile::{EngineConfig, EventSink, KbEngine, KeystoreFfiError, PlatformKeystore};
use kb_model::{BlockKind, PageDoc};
use kb_types::BlockId;

#[derive(Default)]
struct Memory(Mutex<HashMap<String, Vec<u8>>>);
impl PlatformKeystore for Memory {
    fn get(&self, name: String) -> Result<Option<Vec<u8>>, KeystoreFfiError> {
        Ok(self.0.lock().unwrap().get(&name).cloned())
    }
    fn set(&self, name: String, value: Vec<u8>) -> Result<(), KeystoreFfiError> {
        self.0.lock().unwrap().insert(name, value);
        Ok(())
    }
    fn delete(&self, name: String) -> Result<(), KeystoreFfiError> {
        self.0.lock().unwrap().remove(&name);
        Ok(())
    }
}

#[derive(Default)]
struct Sink(Mutex<Vec<String>>);
impl EventSink for Sink {
    fn on_event(&self, event_json: String) {
        self.0.lock().unwrap().push(event_json);
    }
    fn on_state(&self, _state: kb_mobile::EngineState) {}
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (data, cmds) = (args[1].clone(), std::path::PathBuf::from(&args[2]));
    std::fs::create_dir_all(&cmds).unwrap();
    let sink = Arc::new(Sink::default());
    let engine = KbEngine::new(
        EngineConfig { data_dir: data, platform: "desktop-harness".to_owned() },
        Arc::new(Memory::default()),
        sink.clone(),
    );
    println!("desk ready");
    loop {
        let mut names: Vec<_> = std::fs::read_dir(&cmds)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "cmd"))
            .collect();
        names.sort();
        for path in names {
            let line = std::fs::read_to_string(&path).unwrap();
            let line = line.trim_start_matches('\u{feff}').trim().to_owned();
            let (verb, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let answer = match verb {
                "raw" => engine.dispatch(rest.to_owned()),
                "state" => format!("{:?}", engine.state()),
                "events" => std::mem::take(&mut *sink.0.lock().unwrap()).join("\n"),
                "append" => {
                    let (page, text) = rest.split_once(' ').unwrap();
                    append(&engine, page, text)
                }
                "quit" => {
                    std::fs::write(path.with_extension("out"), "bye").unwrap();
                    std::fs::rename(&path, path.with_extension("done")).unwrap();
                    return;
                }
                other => format!("unknown verb {other}"),
            };
            std::fs::write(path.with_extension("out"), answer).unwrap();
            std::fs::rename(&path, path.with_extension("done")).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn append(engine: &KbEngine, page: &str, text: &str) -> String {
    let snapshot = engine.doc_open(page.to_owned()).unwrap();
    let blob = kb_doc::encode_blob(BlobKind::Snapshot, &snapshot);
    let replica = PageDoc::from_doc(Doc::load(DocKind::Page, 0x77, Some(&blob), &[]).unwrap()).unwrap();
    let before = replica.doc().version();
    let last = replica.block_ids().last().copied();
    replica
        .insert_block(last, BlockKind::Paragraph, text, BlockId::from_random_bytes(rand_bytes()))
        .unwrap();
    let update = replica.doc().export_updates_since(Some(&before)).unwrap().unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    engine.doc_import(page.to_owned(), payload.to_vec()).unwrap();
    "appended".to_owned()
}

fn rand_bytes() -> [u8; 16] {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let mut out = [0u8; 16];
    out.copy_from_slice(&nanos.to_le_bytes());
    out
}
