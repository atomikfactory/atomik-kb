//! Milestone 4 on a real SQLCipher file: re-issuing the recovery kit rekeys the
//! database (the old key no longer opens it) and the app lock survives a restart. The
//! keystore is the in-memory double (the machine's credential store has its own
//! opt-in test in `src/keystore.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use kb_core::{Core, CoreDeps, Opened};
use kb_crypto::{Argon2Params, Secret32};
use kb_keys::{Ark, Keystore, KeystoreError, MemoryKeystore, ark_entry_name, derive_db_key};
use kb_storage::{DbOpener, MemoryBlobStore, StorageError};
use kb_storage_native::{DB_FILE, NativeDbOpener, OsRng, SystemClock, load_or_create_install};
use kb_types::{ErrorCode, RecoveryKit, Request, Response, SpaceInfo};
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

/// Small Argon2id parameters so the lock tests stay fast; the shells use the default.
const FAST_LOCK: Argon2Params = Argon2Params {
    memory_kib: 8 * 1024,
    iterations: 1,
    parallelism: 1,
};

struct Setup {
    dir: tempfile::TempDir,
    keystore: Arc<MemoryKeystore>,
}

impl Setup {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            keystore: Arc::new(MemoryKeystore::new()),
        }
    }

    fn data_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("atomik-kb")
    }

    fn install_id(&self) -> kb_types::InstallId {
        load_or_create_install(&self.data_dir(), &OsRng)
            .unwrap()
            .install_id
    }

    fn deps(&self) -> CoreDeps {
        let data_dir = self.data_dir();
        let install = load_or_create_install(&data_dir, &OsRng).unwrap();
        CoreDeps {
            install_id: install.install_id,
            platform: "windows".to_owned(),
            data_dir: data_dir.display().to_string(),
            keystore: Box::new(SharedKeystore(Arc::clone(&self.keystore))),
            opener: Box::new(NativeDbOpener {
                path: data_dir.join(DB_FILE),
            }),
            clock: Box::new(SystemClock),
            rng: Box::new(OsRng),
            lock_params: FAST_LOCK,
            blob_store: Box::new(MemoryBlobStore::new()),
        }
    }

    fn opener(&self) -> NativeDbOpener {
        NativeDbOpener {
            path: self.data_dir().join(DB_FILE),
        }
    }

    /// The database key derived from the ARK currently in the keystore.
    fn current_db_key(&self) -> kb_keys::DbKey {
        let install_id = self.install_id();
        let bytes = self
            .keystore
            .get(&ark_entry_name(&install_id))
            .unwrap()
            .expect("plain root key in the keystore");
        let ark = Ark::from_secret(Secret32::from_bytes(bytes.as_slice().try_into().unwrap()));
        derive_db_key(&ark, &install_id)
    }
}

fn spaces(core: &mut Core) -> Vec<SpaceInfo> {
    match core.dispatch(Request::SpaceList).unwrap() {
        Response::Spaces(list) => list,
        other => unreachable!("unexpected response {other:?}"),
    }
}

fn workspace_with_a_space(core: &mut Core) {
    core.dispatch(Request::WorkspaceCreate {
        name: "Home".into(),
        first_space_name: Some("Personal".into()),
    })
    .unwrap();
    let space = spaces(core)[0].id;
    core.dispatch(Request::PageCreate {
        space_id: space,
        parent_id: None,
        index: None,
        title: "Encrypted page".into(),
    })
    .unwrap();
}

fn kit(core: &mut Core, request: Request) -> RecoveryKit {
    match core.dispatch(request).unwrap() {
        Response::RecoveryKit(kit) => kit,
        other => unreachable!("unexpected response {other:?}"),
    }
}

#[test]
fn reissuing_the_kit_rekeys_the_sqlcipher_database() {
    let setup = Setup::new();
    let mut core = Core::open(setup.deps()).unwrap();
    workspace_with_a_space(&mut core);
    let first = kit(&mut core, Request::RecoveryKitCreate);
    let old_key = setup.current_db_key();

    let second = kit(&mut core, Request::RecoveryKitReissue { passphrase: None });
    assert_ne!(first.words, second.words);
    drop(core);

    // The old key no longer opens the file; the keystore's new key does.
    let err = setup.opener().open(&old_key).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    assert_ne!(setup.current_db_key(), old_key);
    let mut core = Core::open(setup.deps()).unwrap();
    assert_eq!(spaces(&mut core).len(), 1);
    assert!(
        core.dispatch(Request::RecoveryKitVerify {
            kit: second.code.clone()
        })
        .is_ok()
    );
    assert_eq!(
        core.dispatch(Request::RecoveryKitVerify {
            kit: first.words.join(" ")
        })
        .unwrap_err()
        .code,
        ErrorCode::InvalidInput,
        "the old kit is gone"
    );
}

#[test]
fn the_app_lock_survives_a_restart() {
    let setup = Setup::new();
    let mut core = Core::open(setup.deps()).unwrap();
    workspace_with_a_space(&mut core);
    core.dispatch(Request::AppLockEnable {
        passphrase: "correct horse battery".into(),
    })
    .unwrap();
    drop(core);

    assert_eq!(
        Core::open(setup.deps()).err().unwrap().code,
        ErrorCode::Locked
    );
    let Opened::Locked(locked) = Core::open_or_locked(setup.deps()).unwrap() else {
        panic!("expected a locked core");
    };
    let (locked, err) = locked.unlock("not the passphrase").err().unwrap();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let mut core = locked
        .unlock("correct horse battery")
        .unwrap_or_else(|(_, e)| panic!("{e}"));
    assert_eq!(spaces(&mut core).len(), 1);

    // Lock again and the file is only reachable through the passphrase.
    let locked = core.lock().unwrap_or_else(|(_, e)| panic!("{e}"));
    drop(locked);
    assert!(matches!(
        Core::open_or_locked(setup.deps()).unwrap(),
        Opened::Locked(_)
    ));
    assert!(
        setup
            .keystore
            .get(&ark_entry_name(&setup.install_id()))
            .unwrap()
            .is_none(),
        "no plain root key while the lock is on"
    );
}
