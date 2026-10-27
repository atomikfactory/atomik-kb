//! Native platform adapter (desktop; Android later via cargo-ndk).
//!
//! Provides the platform pieces of [`kb_core::CoreDeps`]: the install id file, the OS
//! keystore (the keyring ecosystem, see [`NativeKeystore`]), the SQLCipher file opener,
//! the attachment chunk store ([`FsBlobStore`]), the system clock and the OS CSPRNG. No
//! business logic lives here (ADR-0015).

#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kb_core::CoreDeps;
use kb_keys::{DbKey, KeystoreError};
use kb_storage::{DbOpener, OpenedDb, StorageError};
use kb_types::{Clock, InstallId, Rng};

pub use zeroize;

mod fs_blob;
mod keystore;

pub use fs_blob::FsBlobStore;
pub use keystore::NativeKeystore;

/// File name of the install file inside the data directory.
pub const INSTALL_FILE: &str = "install.json";

/// File name of the database inside the data directory.
pub const DB_FILE: &str = "workspace.db";

/// Directory of the attachment chunks inside the data directory (`<data_dir>/blobs`).
pub const BLOB_DIR: &str = "blobs";

/// Service name under which the keystore entries are stored.
pub const KEYSTORE_SERVICE: &str = "atomik-kb";

/// Current `install.json` format version.
const INSTALL_VERSION: u32 = 1;

/// Adapter errors.
#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    /// The data directory or install file could not be read or written.
    #[error("data directory error: {0}")]
    DataDir(String),
    /// Keystore problem while building the dependencies.
    #[error(transparent)]
    Keystore(#[from] KeystoreError),
}

/// Contents of `install.json`, the only plaintext file in the data directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstallInfo {
    /// File format version (1).
    pub version: u32,
    /// Random id naming the keystore entry and salting the DB key. Not secret.
    pub install_id: InstallId,
}

/// Reads `install.json` in `data_dir` or creates it with a fresh id.
///
/// The directory is created if missing. A file that cannot be read, parsed or that
/// carries another `version` is a [`NativeError::DataDir`] and is **left untouched**:
/// minting a new id over it would orphan the keystore entry and with it the database.
/// A new file is written next to its final name and renamed into place, so a crash
/// never leaves a partial `install.json`.
pub fn load_or_create_install(data_dir: &Path, rng: &dyn Rng) -> Result<InstallInfo, NativeError> {
    fs::create_dir_all(data_dir)
        .map_err(|e| NativeError::DataDir(format!("cannot create the data directory: {e}")))?;
    let path = data_dir.join(INSTALL_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => parse_install(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_install(&path, rng),
        Err(e) => Err(NativeError::DataDir(format!(
            "cannot read {INSTALL_FILE}: {e}"
        ))),
    }
}

fn parse_install(text: &str) -> Result<InstallInfo, NativeError> {
    let corrupt =
        |e: serde_json::Error| NativeError::DataDir(format!("{INSTALL_FILE} is corrupt: {e}"));

    let value: serde_json::Value = serde_json::from_str(text).map_err(corrupt)?;
    // Check the version before the rest: a future format may change other fields too.
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(version) if version == u64::from(INSTALL_VERSION) => {}
        Some(version) => {
            return Err(NativeError::DataDir(format!(
                "{INSTALL_FILE} has unsupported version {version} (this build reads version {INSTALL_VERSION})"
            )));
        }
        None => {
            return Err(NativeError::DataDir(format!(
                "{INSTALL_FILE} is corrupt: it has no version"
            )));
        }
    }
    serde_json::from_value(value).map_err(corrupt)
}

fn create_install(path: &Path, rng: &dyn Rng) -> Result<InstallInfo, NativeError> {
    let info = InstallInfo {
        version: INSTALL_VERSION,
        install_id: InstallId::generate(rng),
    };
    let mut json = serde_json::to_string_pretty(&info)
        .map_err(|e| NativeError::DataDir(format!("cannot encode {INSTALL_FILE}: {e}")))?;
    json.push('\n');

    let temp = path.with_extension("json.tmp");
    let write = || -> io::Result<()> {
        let mut file = File::create(&temp)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&temp);
        NativeError::DataDir(format!("cannot write {INSTALL_FILE}: {e}"))
    })?;

    tracing::debug!("created {INSTALL_FILE}");
    Ok(info)
}

/// Opens `<data_dir>/workspace.db` with SQLCipher.
#[derive(Debug, Clone)]
pub struct NativeDbOpener {
    /// Full path of the database file.
    pub path: PathBuf,
}

impl DbOpener for NativeDbOpener {
    fn open(&self, key: &DbKey) -> Result<OpenedDb, StorageError> {
        kb_storage::sqlcipher::open_file(&self.path, key)
    }
}

/// `std::time::SystemTime` clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
            // A clock set before 1970: report it as negative rather than fail.
            Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
        }
    }

    /// The UTC offset of the operating system's time zone right now (daylight saving
    /// included), for the reminders (ADR-0024).
    fn local_offset_seconds(&self) -> i32 {
        chrono::Local::now().offset().local_minus_utc()
    }
}

/// OS CSPRNG via `getrandom`.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsRng;

impl Rng for OsRng {
    // The operating system's random generator failing is unrecoverable: carrying on
    // would mean generating keys and ids from something that is not random. The `Rng`
    // trait is infallible by design (the core is sans-IO and cannot handle it), so this
    // is a fatal invariant rather than an error to return.
    #[allow(clippy::expect_used)]
    fn fill_bytes(&self, buf: &mut [u8]) {
        getrandom::fill(buf).expect("the operating system's random number generator failed");
    }
}

/// Builds the full [`CoreDeps`] for a native shell from a data directory.
///
/// Creates the directory, loads or creates `install.json`, wires the keystore, opener,
/// the attachment chunk store (`<data_dir>/blobs`, created when missing), clock and RNG.
/// The shell then calls `kb_core::Core::open(deps)`.
///
/// Opening the keystore here does not read or write any secret; the ARK is created or
/// loaded by `Core::open`. On a machine without a usable keystore (Linux without a
/// Secret Service) this fails with [`NativeError::Keystore`].
pub fn native_deps(data_dir: &Path, platform: &str) -> Result<CoreDeps, NativeError> {
    let rng = OsRng;
    let install = load_or_create_install(data_dir, &rng)?;
    let keystore = NativeKeystore::new(KEYSTORE_SERVICE)?;
    let blob_store = FsBlobStore::new(data_dir.join(BLOB_DIR))
        .map_err(|e| NativeError::DataDir(format!("cannot open the attachment store: {e}")))?;
    Ok(CoreDeps {
        install_id: install.install_id,
        platform: platform.to_owned(),
        data_dir: data_dir.display().to_string(),
        keystore: Box::new(keystore),
        opener: Box::new(NativeDbOpener {
            path: data_dir.join(DB_FILE),
        }),
        clock: Box::new(SystemClock),
        rng: Box::new(rng),
        lock_params: kb_crypto::Argon2Params::DEFAULT,
        blob_store: Box::new(blob_store),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use kb_keys::{MemoryKeystore, derive_db_key, load_or_create_ark};
    use kb_storage::Encryption;
    use kb_types::RngExt;
    use kb_types::testing::SeededRng;
    use serde_json::json;

    /// 2025-01-01T00:00:00Z in Unix milliseconds.
    const JAN_2025_MS: i64 = 1_735_689_600_000;

    fn read_install(dir: &Path) -> String {
        fs::read_to_string(dir.join(INSTALL_FILE)).unwrap()
    }

    // --- install.json ---------------------------------------------------------------

    #[test]
    fn install_is_created_with_a_fresh_id_in_a_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let info = load_or_create_install(dir.path(), &SeededRng::new(7)).unwrap();
        assert_eq!(info.version, 1);
        assert_eq!(info.install_id, InstallId::generate(&SeededRng::new(7)));

        // The file format is frozen as v1: exactly these two fields.
        let written: serde_json::Value = serde_json::from_str(&read_install(dir.path())).unwrap();
        assert_eq!(
            written,
            json!({ "version": 1, "install_id": info.install_id.to_string() })
        );
        assert!(
            !dir.path().join("install.json.tmp").exists(),
            "the temporary file is renamed away"
        );
    }

    #[test]
    fn install_is_loaded_not_regenerated() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_install(dir.path(), &SeededRng::new(1)).unwrap();
        let text = read_install(dir.path());

        for seed in [1, 2, 3] {
            let again = load_or_create_install(dir.path(), &SeededRng::new(seed)).unwrap();
            assert_eq!(again, first, "seed {seed}");
        }
        assert_eq!(read_install(dir.path()), text, "the file is not rewritten");
    }

    #[test]
    fn install_creates_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("data");
        let info = load_or_create_install(&nested, &SeededRng::new(1)).unwrap();
        assert!(nested.join(INSTALL_FILE).is_file());
        assert_eq!(load_or_create_install(&nested, &OsRng).unwrap(), info);
    }

    #[test]
    fn install_with_a_real_rng_gets_distinct_ids() {
        let a = load_or_create_install(tempfile::tempdir().unwrap().path(), &OsRng).unwrap();
        let b = load_or_create_install(tempfile::tempdir().unwrap().path(), &OsRng).unwrap();
        assert_ne!(a.install_id, b.install_id);
    }

    #[test]
    fn corrupt_install_files_are_refused_and_left_untouched() {
        let valid_id = "12345678-9abc-4def-8123-456789abcdef";
        let corrupt = [
            String::new(),
            "{".to_owned(),
            "not json".to_owned(),
            "[]".to_owned(),
            "null".to_owned(),
            r#"{"version": 1}"#.to_owned(),
            r#"{"version": 1, "install_id": "nope"}"#.to_owned(),
            r#"{"version": 1, "install_id": 5}"#.to_owned(),
            format!(r#"{{"install_id": "{valid_id}"}}"#),
            format!(r#"{{"version": "1", "install_id": "{valid_id}"}}"#),
            format!(r#"{{"version": -1, "install_id": "{valid_id}"}}"#),
        ];
        for content in corrupt {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(INSTALL_FILE), &content).unwrap();

            let err = load_or_create_install(dir.path(), &SeededRng::new(1)).unwrap_err();
            assert!(matches!(err, NativeError::DataDir(_)), "{content:?}: {err}");
            assert_eq!(
                read_install(dir.path()),
                content,
                "a corrupt file must not be replaced by a new id: {content:?}"
            );
        }
    }

    #[test]
    fn non_utf8_install_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(INSTALL_FILE), [0xff, 0xfe, 0x00]).unwrap();
        let err = load_or_create_install(dir.path(), &SeededRng::new(1)).unwrap_err();
        assert!(matches!(err, NativeError::DataDir(_)), "{err}");
    }

    #[test]
    fn other_install_versions_are_refused() {
        let id = "12345678-9abc-4def-8123-456789abcdef";
        for version in [0, 2, 99] {
            let dir = tempfile::tempdir().unwrap();
            // A future format may also change the other fields; the version decides.
            let content = format!(r#"{{"version": {version}, "install_id": "{id}", "extra": 1}}"#);
            fs::write(dir.path().join(INSTALL_FILE), &content).unwrap();

            let err = load_or_create_install(dir.path(), &SeededRng::new(1)).unwrap_err();
            assert!(
                matches!(&err, NativeError::DataDir(m) if m.contains(&format!("version {version}"))),
                "{err}"
            );
            assert_eq!(read_install(dir.path()), content);
        }

        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(INSTALL_FILE),
            r#"{"version": 2, "install_id": {"scheme": "different"}}"#,
        )
        .unwrap();
        let err = load_or_create_install(dir.path(), &SeededRng::new(1)).unwrap_err();
        assert!(
            matches!(&err, NativeError::DataDir(m) if m.contains("unsupported version 2")),
            "a newer layout is reported as a version problem, not as corruption: {err}"
        );
    }

    #[test]
    fn install_dir_that_is_a_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("occupied");
        fs::write(&file, "x").unwrap();
        let err = load_or_create_install(&file, &SeededRng::new(1)).unwrap_err();
        assert!(matches!(err, NativeError::DataDir(_)), "{err}");
    }

    // --- clock and rng --------------------------------------------------------------

    #[test]
    fn system_clock_is_plausible() {
        let now = SystemClock.now_ms();
        assert!(now > JAN_2025_MS, "{now}");
        assert!(now < 4_102_444_800_000, "before the year 2100: {now}");
        let later = SystemClock.now_ms();
        assert!(later >= now - 1_000, "no big jumps between two reads");
    }

    #[test]
    fn system_clock_reports_the_offset_of_the_local_zone() {
        // The reminders' "now" is the UTC clock plus this (ADR-0024). Zones run from UTC-12 to
        // UTC+14, in steps of fifteen minutes today; the offset must agree with itself and
        // with the one `chrono` reads for the same instant.
        let offset = SystemClock.local_offset_seconds();
        assert!((-12 * 3600..=14 * 3600).contains(&offset), "{offset}");
        assert_eq!(offset % 900, 0, "{offset}");
        let again = SystemClock.local_offset_seconds();
        assert!(
            (again - offset).abs() <= 3600,
            "at most a daylight saving change apart"
        );
    }

    #[test]
    fn os_rng_fills_nonzero_and_differs_between_calls() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        OsRng.fill_bytes(&mut a);
        OsRng.fill_bytes(&mut b);
        assert_ne!(a, [0u8; 32], "32 random bytes are not all zero");
        assert_ne!(b, [0u8; 32]);
        assert_ne!(a, b, "two draws differ");

        // Odd sizes, empty buffers and large buffers all work.
        OsRng.fill_bytes(&mut []);
        let mut odd = [0u8; 7];
        OsRng.fill_bytes(&mut odd);
        let mut large = vec![0u8; 1 << 20];
        OsRng.fill_bytes(&mut large);
        // 1 MiB of random bytes holds about 4096 zero bytes; twice that is 64 sigma away.
        assert!(
            large.iter().filter(|&&byte| byte == 0).count() < 8_192,
            "1 MiB of output is not mostly zero"
        );
        assert_ne!(OsRng.random_array::<16>(), OsRng.random_array::<16>());
        assert_ne!(InstallId::generate(&OsRng), InstallId::generate(&OsRng));
    }

    // --- opener and dependency wiring -----------------------------------------------

    fn db_key(seed: u64) -> DbKey {
        let install_id = InstallId::from_random_bytes([3; 16]);
        let ark =
            load_or_create_ark(&MemoryKeystore::new(), &SeededRng::new(seed), &install_id).unwrap();
        derive_db_key(&ark, &install_id)
    }

    #[test]
    fn native_opener_creates_an_encrypted_database_and_rejects_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let opener = NativeDbOpener {
            path: dir.path().join("nested").join(DB_FILE),
        };

        let opened = opener.open(&db_key(1)).unwrap();
        assert!(
            matches!(opened.encryption, Encryption::SqlCipher { .. }),
            "{:?}",
            opened.encryption
        );
        drop(opened);
        assert!(opener.path.is_file());

        assert!(opener.open(&db_key(1)).is_ok(), "same key reopens");
        let err = opener.open(&db_key(2)).unwrap_err();
        assert!(matches!(err, StorageError::WrongKey), "{err}");
    }

    #[test]
    fn native_deps_wires_the_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("atomik-kb");
        let result = native_deps(&data_dir, "testos");
        // Only Linux can lack a keystore (no running Secret Service); there the error
        // must be the typed one, not a crash. Windows and macOS must have one.
        const KEYSTORE_MAY_BE_ABSENT: bool = cfg!(all(unix, not(target_os = "macos")));
        if KEYSTORE_MAY_BE_ABSENT
            && matches!(
                &result,
                Err(NativeError::Keystore(KeystoreError::Unavailable(_)))
            )
        {
            assert!(data_dir.join(INSTALL_FILE).is_file());
            return;
        }

        let deps = result.unwrap();
        let install = load_or_create_install(&data_dir, &SeededRng::new(1)).unwrap();
        assert_eq!(deps.install_id, install.install_id);
        assert_eq!(deps.platform, "testos");
        assert_eq!(deps.data_dir, data_dir.display().to_string());
        assert!(deps.clock.now_ms() > JAN_2025_MS);
        assert_ne!(deps.rng.random_array::<16>(), deps.rng.random_array::<16>());
        // Wiring only: no secret was read or written, no database was created.
        assert!(!data_dir.join(DB_FILE).exists());

        // The attachment chunks go to files under `<data_dir>/blobs`.
        assert!(data_dir.join(BLOB_DIR).is_dir());
        let chunk = kb_types::ChunkId::from_bytes(kb_crypto::hash(b"wired"));
        deps.blob_store.put(&chunk, b"wired").unwrap();
        let hex = chunk.to_hex();
        assert_eq!(
            fs::read(
                data_dir
                    .join("blobs")
                    .join(&hex[..2])
                    .join(format!("{hex}.chunk"))
            )
            .unwrap(),
            b"wired"
        );
        assert_eq!(deps.blob_store.get(&chunk).unwrap().unwrap(), b"wired");
        assert_eq!(deps.blob_store.list().unwrap(), [chunk]);
    }

    #[test]
    fn native_deps_reports_a_blob_directory_that_cannot_be_created() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the chunk directory belongs.
        fs::write(dir.path().join(BLOB_DIR), "in the way").unwrap();
        let err = native_deps(dir.path(), "testos").err().unwrap();
        // (Linux without a Secret Service fails earlier, with the keystore error.)
        if matches!(err, NativeError::Keystore(_)) {
            return;
        }
        let NativeError::DataDir(message) = &err else {
            unreachable!("{err}")
        };
        assert!(message.contains("attachment store"), "{message}");
        assert!(
            !message.contains(dir.path().to_str().unwrap()),
            "no path: {message}"
        );
    }
}
