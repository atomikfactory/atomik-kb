//! A host-side stand-in for the desktop in the Android smoke (scripts/smoke/android-sync.ps1): a
//! `KbEngine` on its own data directory that takes one command per file from a directory, so a
//! script can make it the account's first device, sponsor the phone and edit pages.
//!
//! `desk <data_dir> <cmd_dir>`: every `<n>.cmd` (one line) is answered by `<n>.out`, in name order.
//! Verbs: `raw <Request json>` (the core's JSON door), `state`, `append <pageId> <text>` (adds a
//! paragraph through a replica, as the editor does), `events` (the events since the last call),
//! `quit`.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kb_doc::{BlobKind, Doc, DocKind};
use kb_mobile::{EngineConfig, EventSink, KbEngine, KeystoreFfiError, PlatformKeystore};
use kb_model::{BlockKind, PageDoc};
use kb_types::BlockId;

/// Locks a mutex, carrying on with the data of a poisoned one (a harness has nothing to protect).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct Memory(Mutex<HashMap<String, Vec<u8>>>);
impl PlatformKeystore for Memory {
    fn get(&self, name: String) -> Result<Option<Vec<u8>>, KeystoreFfiError> {
        Ok(lock(&self.0).get(&name).cloned())
    }
    fn set(&self, name: String, value: Vec<u8>) -> Result<(), KeystoreFfiError> {
        lock(&self.0).insert(name, value);
        Ok(())
    }
    fn delete(&self, name: String) -> Result<(), KeystoreFfiError> {
        lock(&self.0).remove(&name);
        Ok(())
    }
}

#[derive(Default)]
struct Sink(Mutex<Vec<String>>);
impl EventSink for Sink {
    fn on_event(&self, event_json: String) {
        lock(&self.0).push(event_json);
    }
    fn on_state(&self, _state: kb_mobile::EngineState) {}
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(data), Some(cmds)) = (
        args.get(1).cloned(),
        args.get(2).map(std::path::PathBuf::from),
    ) else {
        return Err("usage: desk <data_dir> <cmd_dir>".into());
    };
    std::fs::create_dir_all(&cmds)?;
    let sink = Arc::new(Sink::default());
    let engine = KbEngine::new(
        EngineConfig {
            data_dir: data,
            platform: "desktop-harness".to_owned(),
        },
        Arc::new(Memory::default()),
        sink.clone(),
    );
    println!("desk ready");
    loop {
        let mut names: Vec<_> = std::fs::read_dir(&cmds)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "cmd"))
            .collect();
        names.sort();
        for path in names {
            let line = std::fs::read_to_string(&path)?;
            let line = line.trim_start_matches('\u{feff}').trim().to_owned();
            let (verb, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let answer = match verb {
                "raw" => engine.dispatch(rest.to_owned()),
                "state" => format!("{:?}", engine.state()),
                "events" => std::mem::take(&mut *lock(&sink.0)).join("\n"),
                "append" => {
                    let (page, text) = rest.split_once(' ').unwrap_or((rest, ""));
                    append(&engine, page, text).unwrap_or_else(|e| format!("error: {e}"))
                }
                "quit" => {
                    std::fs::write(path.with_extension("out"), "bye")?;
                    std::fs::rename(&path, path.with_extension("done"))?;
                    return Ok(());
                }
                other => format!("unknown verb {other}"),
            };
            std::fs::write(path.with_extension("out"), answer)?;
            std::fs::rename(&path, path.with_extension("done"))?;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn append(engine: &KbEngine, page: &str, text: &str) -> Result<String, Box<dyn Error>> {
    let snapshot = engine.doc_open(page.to_owned())?;
    let blob = kb_doc::encode_blob(BlobKind::Snapshot, &snapshot);
    let replica = PageDoc::from_doc(Doc::load(DocKind::Page, 0x77, Some(&blob), &[])?)?;
    let before = replica.doc().version();
    let last = replica.block_ids().last().copied();
    replica.insert_block(
        last,
        BlockKind::Paragraph,
        text,
        BlockId::from_random_bytes(rand_bytes()?),
    )?;
    let update = replica
        .doc()
        .export_updates_since(Some(&before))?
        .ok_or("no update")?;
    let (_, payload) = kb_doc::decode_blob(&update)?;
    engine.doc_import(page.to_owned(), payload.to_vec())?;
    Ok("appended".to_owned())
}

fn rand_bytes() -> Result<[u8; 16], Box<dyn Error>> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let mut out = [0u8; 16];
    out.copy_from_slice(&nanos.to_le_bytes());
    Ok(out)
}
