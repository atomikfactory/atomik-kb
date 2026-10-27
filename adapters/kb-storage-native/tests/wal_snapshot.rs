//! The write-ahead log of a database that a killed process left behind must survive
//! every open that follows, including one with a wrong key (the lock screen: a wrong
//! passphrase never reaches the database, but a stale keystore or a moved file can).
//! Without a guard, SQLite checkpoints and deletes the log when the probing connection
//! closes, and a recovery that stops early then truncates the database to its first
//! pages: that is how the Milestone 4 real-app pass lost a workspace.
//!
//! A process kill is simulated by copying the database files while the core still holds
//! them open (the copy has no live locks, exactly like the files after a kill).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kb_core::{Core, CoreDeps, Opened};
use kb_crypto::{Argon2Params, Secret32};
use kb_keys::{Ark, Keystore, KeystoreError, MemoryKeystore, ark_entry_name, derive_db_key};
use kb_storage::{Db, DbOpener, MemoryBlobStore, StorageError};
use kb_storage_native::{DB_FILE, NativeDbOpener, OsRng, SystemClock, load_or_create_install};
use kb_types::{Request, Response, SpaceId};
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
        blob_store: Box::new(MemoryBlobStore::new()),
    }
}

fn size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Sizes of the database, its write-ahead log and its shared-memory index.
fn files(dir: &Path) -> (Option<u64>, Option<u64>, Option<u64>) {
    (
        size(&dir.join(DB_FILE)),
        size(&dir.join(format!("{DB_FILE}-wal"))),
        size(&dir.join(format!("{DB_FILE}-shm"))),
    )
}

/// Copies the installation (`install.json` and the database files) as they are on disk,
/// which is what a process kill leaves behind.
fn copy_snapshot(from: &Path, to: &Path) -> PathBuf {
    std::fs::create_dir_all(to).unwrap();
    std::fs::copy(from.join("install.json"), to.join("install.json")).unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let name = format!("{DB_FILE}{suffix}");
        let source = from.join(&name);
        if source.exists() {
            std::fs::copy(&source, to.join(&name)).unwrap();
        }
    }
    to.join(DB_FILE)
}

fn first_space(core: &mut Core) -> SpaceId {
    match core.dispatch(Request::SpaceList).unwrap() {
        Response::Spaces(list) => list[0].id,
        other => unreachable!("{other:?}"),
    }
}

fn add_pages(core: &mut Core, titles: &[&str]) {
    let space = first_space(core);
    for title in titles {
        core.dispatch(Request::PageCreate {
            space_id: space,
            parent_id: None,
            index: None,
            title: (*title).into(),
        })
        .unwrap();
    }
}

fn page_titles(core: &mut Core) -> Vec<String> {
    let space = first_space(core);
    match core.dispatch(Request::TreeGet { space_id: space }).unwrap() {
        Response::Tree(tree) => {
            let mut titles: Vec<String> = tree.roots.iter().map(|n| n.page.title.clone()).collect();
            titles.sort();
            titles
        }
        other => unreachable!("{other:?}"),
    }
}

fn new_workspace(data_dir: &Path, keystore: &Arc<MemoryKeystore>, titles: &[&str]) -> Core {
    let mut core = Core::open(deps(data_dir, keystore)).unwrap();
    core.dispatch(Request::WorkspaceCreate {
        name: "Home".into(),
        first_space_name: Some("Personal".into()),
    })
    .unwrap();
    add_pages(&mut core, titles);
    core
}

fn db_key_in(keystore: &Arc<MemoryKeystore>, data_dir: &Path) -> kb_keys::DbKey {
    let install_id = load_or_create_install(data_dir, &OsRng).unwrap().install_id;
    let bytes = keystore
        .get(&ark_entry_name(&install_id))
        .unwrap()
        .expect("plain root key");
    derive_db_key(
        &Ark::from_secret(Secret32::from_bytes(bytes.as_slice().try_into().unwrap())),
        &install_id,
    )
}

fn wrong_key_for(data_dir: &Path) -> kb_keys::DbKey {
    let install_id = load_or_create_install(data_dir, &OsRng).unwrap().install_id;
    derive_db_key(&Ark::from_secret(Secret32::generate(&OsRng)), &install_id)
}

#[test]
fn a_wrong_key_leaves_a_killed_process_s_files_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live");
    let snapshot = dir.path().join("snapshot");
    let keystore = Arc::new(MemoryKeystore::new());

    // A workspace with content: everything lives in the WAL, the main file holds page 1.
    let core = new_workspace(&live, &keystore, &["Alpha", "Beta", "Gamma"]);
    let (_, wal, _) = files(&live);
    assert!(wal.unwrap_or(0) > 0, "the content should be in the WAL");
    let snapshot_db = copy_snapshot(&live, &snapshot);
    let before = files(&snapshot);
    drop(core);

    let opener = NativeDbOpener {
        path: snapshot_db.clone(),
    };
    let err = opener.open(&wrong_key_for(&snapshot)).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    assert_eq!(
        files(&snapshot),
        before,
        "a wrong key must not change the database files"
    );

    // The right key opens the snapshot with all its content.
    drop(Db::new(opener.open(&db_key_in(&keystore, &snapshot)).unwrap()).unwrap());
    let mut core = Core::open(deps(&snapshot, &keystore)).unwrap();
    assert_eq!(page_titles(&mut core), vec!["Alpha", "Beta", "Gamma"]);
}

#[test]
fn locking_closes_the_database_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live");
    let keystore = Arc::new(MemoryKeystore::new());

    let mut core = new_workspace(&live, &keystore, &["Alpha", "Beta"]);
    core.dispatch(Request::AppLockEnable {
        passphrase: "correct horse battery".into(),
    })
    .unwrap();
    let locked = core.lock().unwrap_or_else(|(_, e)| panic!("{e}"));
    // The connection is closed on lock: the WAL is checkpointed into the file and gone.
    let (db, wal, shm) = files(&live);
    assert!(wal.is_none() && shm.is_none(), "wal={wal:?} shm={shm:?}");
    assert!(db.unwrap() > 4096, "db={db:?}");
    drop(locked);

    let Opened::Locked(locked) = Core::open_or_locked(deps(&live, &keystore)).unwrap() else {
        panic!("expected a locked core");
    };
    let mut core = locked
        .unlock("correct horse battery")
        .unwrap_or_else(|(_, e)| panic!("{e}"));
    assert_eq!(page_titles(&mut core), vec!["Alpha", "Beta"]);
}

#[test]
fn two_killed_generations_then_a_wrong_key_lose_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let gen1 = dir.path().join("gen1");
    let gen2 = dir.path().join("gen2");
    let gen3 = dir.path().join("gen3");
    let keystore = Arc::new(MemoryKeystore::new());

    // Generation 1 writes and is killed.
    let core1 = new_workspace(&gen1, &keystore, &["Alpha", "Beta"]);
    copy_snapshot(&gen1, &gen2);
    drop(core1);

    // Generation 2 recovers the log, writes more (a kit, a lock, pages) and is killed.
    let mut core2 = Core::open(deps(&gen2, &keystore)).unwrap();
    assert_eq!(page_titles(&mut core2), vec!["Alpha", "Beta"]);
    core2.dispatch(Request::RecoveryKitCreate).unwrap();
    add_pages(&mut core2, &["Gamma", "Delta"]);
    core2
        .dispatch(Request::AppLockEnable {
            passphrase: "correct horse battery".into(),
        })
        .unwrap();
    let snapshot_db = copy_snapshot(&gen2, &gen3);
    let before = files(&gen3);
    drop(core2);

    // Generation 3 probes with a wrong key (a stale keystore), then unlocks for real.
    let opener = NativeDbOpener {
        path: snapshot_db.clone(),
    };
    let err = opener.open(&wrong_key_for(&gen3)).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    assert_eq!(
        files(&gen3),
        before,
        "a wrong key must not change the database files"
    );
    let Opened::Locked(locked) = Core::open_or_locked(deps(&gen3, &keystore)).unwrap() else {
        panic!("expected a locked core");
    };
    let mut core3 = locked
        .unlock("correct horse battery")
        .unwrap_or_else(|(_, e)| panic!("{e}"));
    assert_eq!(
        page_titles(&mut core3),
        vec!["Alpha", "Beta", "Delta", "Gamma"]
    );
}
