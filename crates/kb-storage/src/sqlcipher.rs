//! Native SQLCipher file opening (desktop, Android via cargo-ndk).

use std::path::Path;

use kb_keys::DbKey;
use rusqlite::{Connection, OptionalExtension};

use crate::{
    Encryption, OpenedDb, StorageError, map_probe_error, probe_readable, raw_key_pragma,
    set_checkpoint_on_close,
};

/// Opens or creates `path` as a SQLCipher database keyed with the raw 32-byte key
/// (`PRAGMA key = "x'<hex>'"`), enables WAL mode and foreign keys, verifies
/// readability, and reports the cipher version.
///
/// Missing parent directories are created. The statements run in this order so that
/// nothing is ever written to the file before we know it is encrypted and readable:
///
/// 0. checkpoint-on-close is switched off (`SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE`) and only
///    switched back on once step 4 has passed. Without it, a connection that fails the
///    key probe still checkpoints the write-ahead log into the file and deletes the log
///    when it closes (a raw copy, so the key does not matter), and when the log was
///    left behind by a killed process its recovery decides how much of it survives:
///    the Milestone 4 real-app pass lost a workspace that way (a lock-screen unlock
///    with a database key the file did not accept truncated it to its first pages).
///    A wrong key must leave the files exactly as they were;
/// 1. `PRAGMA cipher_log_level = ERROR` (see below);
/// 2. `PRAGMA key`;
/// 3. `PRAGMA cipher_version`: a build without SQLCipher ignores `PRAGMA key` and
///    would silently produce a plaintext database, so an empty answer is fatal
///    ([`StorageError::Open`]);
/// 4. `SELECT count(*) FROM sqlite_master`: reads and decrypts the first page; a wrong
///    key (or a plaintext or foreign file) fails with `SQLITE_NOTADB` ("file is not a
///    database"), reported as [`StorageError::WrongKey`];
/// 5. `PRAGMA journal_mode = WAL` and `PRAGMA foreign_keys = ON`.
///
/// # Memory security is left off (SQLCipher's default)
///
/// `PRAGMA cipher_memory_security = ON` makes SQLCipher lock (`VirtualLock`/`mlock`)
/// and wipe every SQLite allocation. Measured during Milestone 1 on Windows: the lock
/// soon exceeds the process working-set quota (`ERROR_WORKING_SET_QUOTA`, 1453),
/// SQLCipher 4.14 logs a `WARN` from inside its allocator hook and the process dies
/// with a stack overflow; with the warning silenced it still costs about 2.6x on a
/// write-heavy workload. The feature only guards against plaintext pages reaching
/// swap, which full-disk encryption already covers, so it stays off
/// (`docs/security/encryption-design.md`). `cipher_log_level = ERROR` is kept: it is
/// process-wide, harmless, and keeps SQLCipher's own stderr output to real failures.
///
/// The key is never logged and never appears in an error. The SQL text that carries
/// it is wiped after use; SQLite's own copy of a statement is out of our reach, which
/// is inherent to keying through a `PRAGMA` (the alternative, `sqlite3_key`, needs
/// `unsafe`).
pub fn open_file(path: &Path, key: &DbKey) -> Result<OpenedDb, StorageError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| StorageError::Open(format!("cannot create the data directory: {e}")))?;
    }
    let conn = Connection::open(path)
        .map_err(|e| StorageError::Open(format!("cannot open the database file: {e}")))?;

    // Until the key is proven right, closing this connection must not checkpoint the
    // write-ahead log (see the module documentation: a failed probe would otherwise
    // rewrite and delete the log a killed process left behind).
    set_checkpoint_on_close(&conn, false)?;
    conn.execute_batch("PRAGMA cipher_log_level = ERROR;")?;
    apply_key(&conn, key)?;

    let version: Option<String> = conn
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .optional()?;
    let Some(version) = version.filter(|v| !v.is_empty()) else {
        return Err(StorageError::Open("SQLCipher not available".to_owned()));
    };

    probe_readable(&conn)?;
    set_checkpoint_on_close(&conn, true)?;

    let journal_mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .map_err(map_probe_error)?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        // Some file systems (network shares) cannot do WAL; SQLite keeps working in
        // rollback-journal mode, just with less concurrency.
        tracing::warn!(%journal_mode, "database is not in WAL mode");
    }
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    tracing::debug!(%version, "opened encrypted database");
    Ok(OpenedDb {
        conn,
        encryption: Encryption::SqlCipher { version },
    })
}

/// Runs `PRAGMA key = "x'<64 hex digits>'"` (SQLCipher's raw-key form, no passphrase
/// KDF) with the key as lowercase hex; the statement text is wiped after use
/// ([`raw_key_pragma`]).
fn apply_key(conn: &Connection, key: &DbKey) -> Result<(), StorageError> {
    let sql = raw_key_pragma("key", key);
    conn.execute_batch(sql.as_str()).map_err(map_probe_error)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use kb_crypto::Secret32;
    use kb_keys::{Ark, derive_db_key};
    use kb_types::InstallId;
    use kb_types::testing::SeededRng;

    use super::*;
    use crate::verify_key_on_disk;

    fn db_key(seed: u64) -> DbKey {
        let ark = Ark::from_secret(Secret32::generate(&SeededRng::new(seed)));
        derive_db_key(&ark, &InstallId::from_random_bytes([7; 16]))
    }

    fn key_hex(key: &DbKey) -> String {
        key.secret()
            .expose()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn verify_key_on_disk_accepts_the_key_of_the_file_and_refuses_another() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (db_key(1), db_key(2));
        let opened = open_file(&dir.path().join("verify.db"), &a).unwrap();

        verify_key_on_disk(&opened.conn, &a).unwrap();
        let err = verify_key_on_disk(&opened.conn, &b).unwrap_err();
        assert!(
            matches!(&err, StorageError::Open(m) if m.contains("does not open with the new key")),
            "{err}"
        );
        let text = err.to_string();
        assert!(!text.contains(&key_hex(&a)), "{text}");
        assert!(!text.contains(&key_hex(&b)), "{text}");
    }

    #[test]
    fn verify_key_on_disk_has_nothing_to_check_for_a_connection_without_a_file() {
        let conn = Connection::open_in_memory().unwrap();
        verify_key_on_disk(&conn, &db_key(1)).unwrap();
    }

    #[test]
    fn a_rekey_that_failed_inside_sqlcipher_is_invisible_to_the_pragma_but_not_to_the_reopen() {
        // SQLCipher's `PRAGMA rekey` answers "ok" even when it could not rewrite the file,
        // here because another connection holds the write lock. That is why `Db::rekey`
        // takes the lock itself first and then proves the result with this reopen.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quirk.db");
        let (a, b) = (db_key(1), db_key(2));
        let first = open_file(&path, &a).unwrap();
        let second = open_file(&path, &a).unwrap();
        second.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        first.conn.busy_timeout(Duration::ZERO).unwrap();

        let sql = raw_key_pragma("rekey", &b);
        assert!(
            first.conn.execute_batch(sql.as_str()).is_ok(),
            "the pragma reports success although nothing was rekeyed"
        );
        second.conn.execute_batch("ROLLBACK").unwrap();

        // The file does not open with the new key, and still opens with the old one.
        let err = verify_key_on_disk(&first.conn, &b).unwrap_err();
        assert!(matches!(err, StorageError::Open(_)), "{err}");
        verify_key_on_disk(&first.conn, &a).unwrap();
    }
}
