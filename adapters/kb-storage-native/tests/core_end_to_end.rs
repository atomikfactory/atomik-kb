//! The adapter's pieces wired into `kb_core::Core`: `install.json`, the SQLCipher
//! opener, the clock and the OS RNG against a real file in a temp directory. The
//! keystore is the in-memory double, so the test never touches the machine's
//! credential store (that has its own opt-in test in `src/keystore.rs`).

#![allow(clippy::unwrap_used)]

use std::fs;
use std::sync::Arc;

use kb_core::{Core, CoreDeps};
use kb_keys::{Keystore, KeystoreError, MemoryKeystore, ark_entry_name};
use kb_storage::MemoryBlobStore;
use kb_storage_native::{
    DB_FILE, INSTALL_FILE, NativeDbOpener, OsRng, SystemClock, load_or_create_install,
};
use kb_types::{AppStatus, ErrorCode, Request, Response};
use zeroize::Zeroizing;

/// Lets several `Core`s share one in-memory keystore.
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

struct Setup {
    dir: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn data_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("atomik-kb")
    }

    /// The deps a native shell builds, with the keystore swapped for `keystore`.
    fn deps(&self, keystore: &Arc<MemoryKeystore>) -> CoreDeps {
        let data_dir = self.data_dir();
        let install = load_or_create_install(&data_dir, &OsRng).unwrap();
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
            lock_params: kb_crypto::Argon2Params::DEFAULT,
            blob_store: Box::new(MemoryBlobStore::new()),
        }
    }
}

fn status(core: &mut Core) -> AppStatus {
    match core.dispatch(Request::AppStatus).unwrap() {
        Response::AppStatus(status) => status,
        other => unreachable!("unexpected response {other:?}"),
    }
}

#[test]
fn core_runs_on_an_encrypted_database_and_reopens_it() {
    let setup = Setup::new();
    let keystore = Arc::new(MemoryKeystore::new());

    // First launch: ARK created, database created encrypted, device identity minted.
    let mut core = Core::open(setup.deps(&keystore)).unwrap();
    let first = status(&mut core);
    assert!(first.db_encrypted);
    assert!(
        first.db_cipher.starts_with("SQLCipher "),
        "{}",
        first.db_cipher
    );
    assert_eq!(first.schema_version, kb_storage::SCHEMA_VERSION);
    assert!(!first.workspace_initialized);
    assert_eq!(first.platform, "windows");

    core.dispatch(Request::WorkspaceCreate {
        name: "Notes".into(),
        first_space_name: None,
    })
    .unwrap();
    core.dispatch(Request::SettingsSet {
        key: "ui.theme".into(),
        value: serde_json::json!("dark"),
    })
    .unwrap();
    drop(core);

    // The ARK lives in the keystore, under the install id, as 32 bytes.
    let install = load_or_create_install(&setup.data_dir(), &OsRng).unwrap();
    let ark = keystore
        .get(&ark_entry_name(&install.install_id))
        .unwrap()
        .unwrap();
    assert_eq!(ark.len(), 32);

    // The database file is not readable without it.
    let db_bytes = fs::read(setup.data_dir().join(DB_FILE)).unwrap();
    assert_ne!(&db_bytes[..16], b"SQLite format 3\0");

    // Restart: no prompt, same device, data and settings are there.
    let mut core = Core::open(setup.deps(&keystore)).unwrap();
    let second = status(&mut core);
    assert!(second.db_encrypted);
    assert!(second.workspace_initialized);
    assert_eq!(second.device_id, first.device_id);
    assert_eq!(
        core.dispatch(Request::SettingsGet {
            key: "ui.theme".into()
        })
        .unwrap(),
        Response::Setting(Some(serde_json::json!("dark")))
    );
    match core.dispatch(Request::WorkspaceGet).unwrap() {
        Response::Workspace(Some(workspace)) => assert_eq!(workspace.name, "Notes"),
        other => unreachable!("unexpected response {other:?}"),
    }
}

#[test]
fn opening_with_a_different_keystore_fails_with_the_crypto_error() {
    let setup = Setup::new();
    let keystore = Arc::new(MemoryKeystore::new());
    drop(Core::open(setup.deps(&keystore)).unwrap());

    // A keystore that does not hold this installation's ARK mints a new one, whose
    // database key cannot open the existing file: reported as a crypto error, and the
    // database is left alone.
    let before = fs::read(setup.data_dir().join(DB_FILE)).unwrap();
    let other_keystore = Arc::new(MemoryKeystore::new());
    let err = Core::open(setup.deps(&other_keystore)).err().unwrap();
    assert_eq!(err.code, ErrorCode::Crypto);
    assert_eq!(fs::read(setup.data_dir().join(DB_FILE)).unwrap(), before);

    // The right keystore still works.
    assert!(Core::open(setup.deps(&keystore)).is_ok());
}

#[test]
fn install_id_is_stable_across_launches() {
    let setup = Setup::new();
    let keystore = Arc::new(MemoryKeystore::new());
    drop(Core::open(setup.deps(&keystore)).unwrap());
    let file = setup.data_dir().join(INSTALL_FILE);
    let first = fs::read_to_string(&file).unwrap();
    drop(Core::open(setup.deps(&keystore)).unwrap());
    assert_eq!(fs::read_to_string(&file).unwrap(), first);
}
