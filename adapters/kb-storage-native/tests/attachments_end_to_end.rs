//! Milestone 5 on the real stores: an attachment streamed through the core lands as
//! chunk files under `blobs/`, survives a reopen, reads back whole and by range, and
//! disappears with its files on delete; an interrupted write is swept at open.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kb_core::{Core, CoreDeps};
use kb_crypto::Argon2Params;
use kb_keys::{Keystore, KeystoreError, MemoryKeystore};
use kb_storage_native::{
    BLOB_DIR, DB_FILE, FsBlobStore, NativeDbOpener, OsRng, SystemClock, load_or_create_install,
};
use kb_types::attachments::CHUNK_SIZE;
use kb_types::{AttachmentId, AttachmentInfo, ByteRange, Request, Response, SpaceId};
use zeroize::Zeroizing;

struct SharedKeystore(Arc<MemoryKeystore>);

impl Keystore for SharedKeystore {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
        self.0.get(name)
    }
    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeystoreError> {
        self.0.set(name, secret)
    }
    fn delete(&self, name: &str) -> Result<(), KeystoreError> {
        self.0.delete(name)
    }
}

const FAST_LOCK: Argon2Params = Argon2Params {
    memory_kib: 8 * 1024,
    iterations: 1,
    parallelism: 1,
};

fn deps(data_dir: &Path, keystore: &Arc<MemoryKeystore>) -> CoreDeps {
    let install = load_or_create_install(data_dir, &OsRng).unwrap();
    CoreDeps {
        install_id: install.install_id,
        platform: "windows".to_owned(),
        data_dir: data_dir.display().to_string(),
        keystore: Box::new(SharedKeystore(Arc::clone(keystore))),
        opener: Box::new(NativeDbOpener {
            path: data_dir.join(DB_FILE),
        }),
        clock: Box::new(SystemClock),
        rng: Box::new(OsRng),
        lock_params: FAST_LOCK,
        blob_store: Box::new(FsBlobStore::new(data_dir.join(BLOB_DIR)).unwrap()),
    }
}

fn chunk_files(data_dir: &Path) -> Vec<PathBuf> {
    let root = data_dir.join(BLOB_DIR);
    let mut out = Vec::new();
    if let Ok(shards) = std::fs::read_dir(&root) {
        for shard in shards.flatten() {
            if let Ok(files) = std::fs::read_dir(shard.path()) {
                out.extend(files.flatten().map(|f| f.path()));
            }
        }
    }
    out.sort();
    out
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    let mut state = u32::from(seed).wrapping_mul(2_654_435_761).wrapping_add(7);
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

fn workspace(core: &mut Core) -> SpaceId {
    core.dispatch(Request::WorkspaceCreate {
        name: "Home".into(),
        first_space_name: Some("Personal".into()),
    })
    .unwrap();
    match core.dispatch(Request::SpaceList).unwrap() {
        Response::Spaces(list) => list[0].id,
        other => unreachable!("{other:?}"),
    }
}

fn upload(core: &mut Core, space: SpaceId, name: &str, data: &[u8]) -> AttachmentInfo {
    let id = match core
        .dispatch(Request::AttachmentBegin {
            space_id: space,
            name: name.into(),
            mime: "application/octet-stream".into(),
        })
        .unwrap()
    {
        Response::AttachmentStarted(id) => id,
        other => unreachable!("{other:?}"),
    };
    // Pieces of one chunk, the way the shell streams a file.
    for piece in data.chunks(CHUNK_SIZE) {
        core.attachment_write(id, piece).unwrap();
    }
    match core
        .dispatch(Request::AttachmentFinish { attachment_id: id })
        .unwrap()
    {
        Response::Attachment(info) => info,
        other => unreachable!("{other:?}"),
    }
}

fn read(core: &mut Core, id: AttachmentId, range: Option<ByteRange>) -> Vec<u8> {
    let mut out = Vec::new();
    core.attachment_read(id, range, &mut |piece| {
        out.extend_from_slice(piece);
        Ok(())
    })
    .unwrap();
    out
}

#[test]
fn an_attachment_lands_as_chunk_files_and_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("atomik-kb");
    let keystore = Arc::new(MemoryKeystore::new());
    let data = pattern(2 * CHUNK_SIZE + 4_321, 1);

    let mut core = Core::open(deps(&data_dir, &keystore)).unwrap();
    let space = workspace(&mut core);
    let info = upload(&mut core, space, "big.bin", &data);
    assert_eq!(info.size, data.len() as u64);
    assert_eq!(info.chunk_count, 3);
    let files = chunk_files(&data_dir);
    assert_eq!(files.len(), 3, "{files:?}");
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(".chunk") && name.len() == 64 + 6, "{name}");
        let shard = file
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(shard, name[..2]);
        let bytes = std::fs::read(file).unwrap();
        assert!(bytes.len() <= kb_core::MAX_SEALED_CHUNK);
        // Ciphertext: the plaintext pattern is nowhere in it.
        assert!(!bytes.windows(64).any(|w| w == &data[1000..1064]));
    }
    drop(core);

    // A leftover of an interrupted write is swept at the next open.
    let stray = data_dir
        .join(BLOB_DIR)
        .join("ab")
        .join(format!("{}.tmp", "ab".repeat(32)));
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, b"half written").unwrap();

    let mut core = Core::open(deps(&data_dir, &keystore)).unwrap();
    assert!(
        !stray.exists(),
        "the stray temporary file was swept at open"
    );
    assert_eq!(chunk_files(&data_dir).len(), 3);
    assert_eq!(read(&mut core, info.attachment_id, None), data);
    let tail = read(
        &mut core,
        info.attachment_id,
        Some(ByteRange {
            start: CHUNK_SIZE as u64 - 10,
            end: CHUNK_SIZE as u64 + 10,
        }),
    );
    assert_eq!(tail, &data[CHUNK_SIZE - 10..CHUNK_SIZE + 10]);

    core.dispatch(Request::AttachmentDelete {
        attachment_id: info.attachment_id,
    })
    .unwrap();
    assert!(chunk_files(&data_dir).is_empty());
    assert_eq!(
        core.dispatch(Request::AttachmentGet {
            attachment_id: info.attachment_id
        })
        .unwrap_err()
        .code,
        kb_types::ErrorCode::NotFound
    );
}
