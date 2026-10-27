//! Shared fixtures for the core's tests: a plain (unencrypted) temp-file database so
//! tests do not need SQLCipher, an in-memory keystore shared across reopenings, and
//! small helpers around `dispatch`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use kb_doc::{Doc, DocKind};
use kb_keys::{DbKey, Keystore, KeystoreError, MemoryKeystore};
use kb_model::{BlockKind, PageDoc};
use kb_storage::{DbOpener, Encryption, OpenedDb, StorageError};
use kb_types::testing::{FixedClock, SeededRng};
use kb_types::{
    AppStatus, BackupSummary, BlockId, InstallId, PageId, PageInfo, RecoveryKit, Request, Response,
    SpaceId, SpaceInfo, TreeView, Workspace,
};

use crate::{Core, CoreDeps};

/// Opens a plain SQLite file so core tests do not need SQLCipher.
pub struct PlainFileOpener(pub PathBuf);

impl DbOpener for PlainFileOpener {
    fn open(&self, _key: &DbKey) -> Result<OpenedDb, StorageError> {
        let conn = kb_storage::rusqlite::Connection::open(&self.0)?;
        Ok(OpenedDb {
            conn,
            encryption: Encryption::None,
        })
    }
}

/// Lets several `Core`s share one in-memory keystore (survives "restarts").
pub struct SharedKeystore(pub Arc<MemoryKeystore>);

impl Keystore for SharedKeystore {
    fn get(&self, name: &str) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, KeystoreError> {
        self.0.get(name)
    }
    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        self.0.set(name, secret)
    }
    fn delete(&self, name: &str) -> Result<(), KeystoreError> {
        self.0.delete(name)
    }
}

/// Lets several `Core`s share one in-memory blob store (survives "restarts").
pub struct SharedBlobStore(pub Arc<kb_storage::MemoryBlobStore>);

impl kb_storage::BlobStore for SharedBlobStore {
    fn put(&self, id: &kb_types::ChunkId, bytes: &[u8]) -> Result<(), StorageError> {
        self.0.put(id, bytes)
    }
    fn get(&self, id: &kb_types::ChunkId) -> Result<Option<Vec<u8>>, StorageError> {
        self.0.get(id)
    }
    fn has(&self, id: &kb_types::ChunkId) -> Result<bool, StorageError> {
        self.0.has(id)
    }
    fn delete(&self, id: &kb_types::ChunkId) -> Result<(), StorageError> {
        self.0.delete(id)
    }
    fn list(&self) -> Result<Vec<kb_types::ChunkId>, StorageError> {
        self.0.list()
    }
}

/// A temp data directory plus the keystore, blob store and install id a reopen needs.
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub keystore: Arc<MemoryKeystore>,
    pub blob_store: Arc<kb_storage::MemoryBlobStore>,
    pub install_id: InstallId,
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            keystore: Arc::new(MemoryKeystore::new()),
            blob_store: Arc::new(kb_storage::MemoryBlobStore::new()),
            install_id: InstallId::from_random_bytes([7; 16]),
        }
    }

    pub fn open(&self, seed: u64, now_ms: i64) -> Core {
        Core::open(self.deps(seed, now_ms)).unwrap()
    }

    pub fn deps(&self, seed: u64, now_ms: i64) -> CoreDeps {
        CoreDeps {
            install_id: self.install_id,
            platform: "windows".into(),
            data_dir: self.dir.path().display().to_string(),
            keystore: Box::new(SharedKeystore(Arc::clone(&self.keystore))),
            opener: Box::new(PlainFileOpener(self.dir.path().join("workspace.db"))),
            clock: Box::new(FixedClock::new(now_ms)),
            rng: Box::new(SeededRng::new(seed)),
            // Small so that lock tests stay fast; production shells use the default.
            lock_params: kb_crypto::Argon2Params {
                memory_kib: 8 * 1024,
                iterations: 1,
                parallelism: 1,
            },
            blob_store: Box::new(SharedBlobStore(Arc::clone(&self.blob_store))),
        }
    }
}

pub fn status(core: &mut Core) -> AppStatus {
    match core.dispatch(Request::AppStatus).unwrap() {
        Response::AppStatus(s) => s,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn create_workspace(core: &mut Core, name: &str, first_space: Option<&str>) -> Workspace {
    match core
        .dispatch(Request::WorkspaceCreate {
            name: name.into(),
            first_space_name: first_space.map(str::to_owned),
        })
        .unwrap()
    {
        Response::Workspace(Some(ws)) => ws,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn spaces(core: &mut Core) -> Vec<SpaceInfo> {
    match core.dispatch(Request::SpaceList).unwrap() {
        Response::Spaces(s) => s,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn create_space(core: &mut Core, name: &str) -> SpaceInfo {
    match core
        .dispatch(Request::SpaceCreate {
            name: name.into(),
            icon: None,
        })
        .unwrap()
    {
        Response::Space(s) => s,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn tree(core: &mut Core, space_id: SpaceId) -> TreeView {
    match core.dispatch(Request::TreeGet { space_id }).unwrap() {
        Response::Tree(t) => t,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn create_page(
    core: &mut Core,
    space_id: SpaceId,
    parent: Option<PageId>,
    title: &str,
) -> PageInfo {
    match core
        .dispatch(Request::PageCreate {
            space_id,
            parent_id: parent,
            index: None,
            title: title.into(),
        })
        .unwrap()
    {
        Response::Page(p) => p,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn page(core: &mut Core, page_id: PageId) -> PageInfo {
    match core.dispatch(Request::PageGet { page_id }).unwrap() {
        Response::Page(p) => p,
        other => panic!("unexpected {other:?}"),
    }
}

/// Titles of the live tree, depth-first, with indentation by depth: a compact way
/// to assert structure and order in one string.
pub fn outline(view: &TreeView) -> String {
    fn walk(nodes: &[kb_types::PageNode], depth: usize, out: &mut String) {
        for node in nodes {
            out.push_str(&"  ".repeat(depth));
            out.push_str(&node.page.title);
            out.push('\n');
            walk(&node.children, depth + 1, out);
        }
    }
    let mut out = String::new();
    walk(&view.roots, 0, &mut out);
    out
}

/// Appends a paragraph to a page the way the editor does (a transient replica imports
/// the current snapshot, edits, and sends the update back). `seed` makes the block id
/// and the replica's peer id unique per call.
pub fn write_text(core: &mut Core, page_id: PageId, text: &str, seed: u64) {
    let snapshot = core.page_doc_open(page_id).unwrap();
    let blob = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &snapshot);
    let doc =
        PageDoc::from_doc(Doc::load(DocKind::Page, 0x7000_0000 + seed, Some(&blob), &[]).unwrap())
            .unwrap();
    let before = doc.doc().version();
    let last = doc.block_ids().last().copied();
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&seed.to_le_bytes());
    id[8..].copy_from_slice(&seed.rotate_left(17).to_le_bytes());
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
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    core.page_doc_import(page_id, payload).unwrap();
    core.page_doc_close(page_id).unwrap();
}

/// The content of a page as the UI reads it.
pub fn page_content(core: &mut Core, page_id: PageId) -> serde_json::Value {
    match core.dispatch(Request::PageContentGet { page_id }).unwrap() {
        Response::PageContent(c) => c.content,
        other => panic!("unexpected {other:?}"),
    }
}

pub fn kit_create(core: &mut Core) -> RecoveryKit {
    match core.dispatch(Request::RecoveryKitCreate).unwrap() {
        Response::RecoveryKit(kit) => kit,
        other => panic!("unexpected {other:?}"),
    }
}

/// Exports a backup into memory.
pub fn export_backup(core: &mut Core) -> Vec<u8> {
    let mut bytes = Vec::new();
    core.backup_export(&mut |chunk: &[u8]| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })
    .unwrap();
    bytes
}

/// Imports a backup from memory.
pub fn import_backup(core: &mut Core, bytes: &[u8], kit: Option<&str>) -> BackupSummary {
    let mut pos = 0usize;
    core.backup_import(
        &mut |buf: &mut [u8]| {
            let n = buf.len().min(bytes.len() - pos);
            buf[..n].copy_from_slice(&bytes[pos..pos + n]);
            pos += n;
            Ok(n)
        },
        kit,
    )
    .unwrap()
}
