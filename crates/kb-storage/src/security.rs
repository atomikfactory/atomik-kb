//! Device log and keyring rows, backup helpers and the database rekey (schema v4,
//! `docs/development/milestone-04.md`, "Storage"). Blobs are opaque here; `kb-keys`
//! encodes, decodes and verifies them.
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`]
//! through [`SecurityQueries`], both running the same private functions over a
//! connection (a transaction derefs to one), so a method behaves identically in either
//! place. Each method is one SQL statement, hence atomic on a bare [`Db`] too. Rows are
//! read as raw column values first and validated afterwards: a damaged row is a typed
//! `InvalidData` error, never a panic or a silently wrong value, and blob contents never
//! appear in an error.

use kb_keys::DbKey;
use kb_types::{Json, SpaceId};
use rusqlite::{Connection, Row, Transaction, TransactionBehavior, params};

use crate::docs::{docs_all_on, id_from};
use crate::{
    Db, DocRow, Encryption, StorageError, Tx, WorkspaceRow, map_probe_error, meta_get_on,
    meta_set_on, probe_readable, raw_key_pragma, setting_from_text, setting_set_on,
    verify_key_on_disk, workspace_create_on,
};

/// A `device_log` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLogRow {
    /// Sequence number (0 for the Genesis entry).
    pub seq: u64,
    /// Encoded entry.
    pub entry: Vec<u8>,
    /// BLAKE3 of the encoded entry.
    pub hash: [u8; 32],
}

/// A `keyring` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyringRow {
    /// Space.
    pub space_id: SpaceId,
    /// Encoded subject.
    pub subject: Vec<u8>,
    /// Encoded record or tombstone.
    pub record: Vec<u8>,
    /// `true` for a tombstone.
    pub tombstone: bool,
}

/// A `settings` row, for backups.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingRow {
    /// Key.
    pub key: String,
    /// Value.
    pub value: Json,
    /// Last change, Unix ms.
    pub updated_at: i64,
}

/// Queries of the device log and keyring tables, the whole-table reads a backup needs and
/// the few writes a backup restore and the recovery flows make next to them (workspace,
/// settings, `meta`), so that each of those can run inside one [`Db::transaction`]. On a
/// [`Db`] every method is atomic by itself.
pub trait SecurityQueries {
    /// Appends an entry; `row.seq` must be exactly one more than the highest stored
    /// sequence (0 on an empty log), otherwise `InvalidData` and nothing changes. The
    /// check and the insert are one statement, so two writers cannot both append the same
    /// sequence number: one wins, the other gets `InvalidData`.
    fn device_log_append(&self, row: &DeviceLogRow) -> Result<(), StorageError>;
    /// Every entry in sequence order. A stored hash that is not 32 bytes long is
    /// `InvalidData`.
    fn device_log_all(&self) -> Result<Vec<DeviceLogRow>, StorageError>;
    /// Inserts or replaces the row of `(space_id, subject)`. A plain replace: the merge
    /// rules of the keyring (a tombstone wins, a record is immutable) belong to `kb-keys`.
    fn keyring_put(&self, row: &KeyringRow) -> Result<(), StorageError>;
    /// Every row, ordered by space then subject bytes.
    fn keyring_all(&self) -> Result<Vec<KeyringRow>, StorageError>;
    /// Rows of one space, ordered by subject bytes.
    fn keyring_by_space(&self, space_id: SpaceId) -> Result<Vec<KeyringRow>, StorageError>;
    /// Removes every row of a space. Succeeds when the space has none.
    fn keyring_delete_space(&self, space_id: SpaceId) -> Result<(), StorageError>;
    /// Every setting, ordered by key, with its value parsed as JSON like
    /// [`Db::setting_get`].
    fn settings_all(&self) -> Result<Vec<SettingRow>, StorageError>;
    /// Every document row, oldest first (`created_at`, then `doc_id`) like
    /// [`crate::DocQueries::docs_by_kind`].
    fn docs_all(&self) -> Result<Vec<DocRow>, StorageError>;
    /// Every `meta` key and value, ordered by key.
    fn meta_all(&self) -> Result<Vec<(String, String)>, StorageError>;

    /// Inserts the workspace row unless one exists; `Ok(false)` then and nothing changes.
    /// The same as [`Db::workspace_create`], also available inside a transaction.
    fn workspace_insert(&self, row: &WorkspaceRow) -> Result<bool, StorageError>;
    /// Writes a setting (upsert) with the given `updated_at`: the same as
    /// [`Db::setting_set`], also available inside a transaction.
    fn setting_put(&self, key: &str, value: &Json, updated_at: i64) -> Result<(), StorageError>;
    /// Reads a `meta` value: the same as [`Db::meta_get`], also available inside a
    /// transaction.
    fn meta_get(&self, key: &str) -> Result<Option<String>, StorageError>;
    /// Writes a `meta` value (upsert): the same as [`Db::meta_set`], also available
    /// inside a transaction.
    fn meta_set(&self, key: &str, value: &str) -> Result<(), StorageError>;
}

impl Db {
    /// Re-encrypts the database under `key`: `PRAGMA rekey = "x'<64 hex digits>'"` with the
    /// raw key (the statement text is wiped after use, like the one that opens the file),
    /// then a readability probe. A no-op `Ok(())` for [`crate::Encryption::None`].
    /// SQLCipher (native) and sqlite3mc (web) take the same statement.
    ///
    /// # What the caller has to know
    ///
    /// - **One connection.** The caller must not hold any other connection to the file
    ///   while this runs. One that holds the write lock makes the call fail (below); one
    ///   that stays open would keep reading with the old key.
    /// - **WAL mode is fine.** The rewrite goes through the write-ahead log, page by page,
    ///   in one transaction, so it is atomic; the log is checkpointed at the end. It needs
    ///   free disk space for about one more copy of the database.
    /// - **After an `Err`, probe the keys.** An error from the lock step (a busy or
    ///   read-only database) means nothing was touched and this `Db` is as good as before.
    ///   After any other error the state is not known: the file is under the old key if
    ///   the rekey did not take, and under the new one if it did but could not be proved.
    ///   Drop this `Db` (SQLCipher leaves a connection whose rekey failed writing pages
    ///   under the new key, which corrupts the file at the next write), open the file with
    ///   the old key, and if that is [`StorageError::WrongKey`] the new key is the current
    ///   one.
    /// - **Not atomic with the keystore.** The file and the stored key change in two
    ///   steps; what happens between them (a crash) is the caller's to handle.
    ///
    /// # Why it does more than run the pragma
    ///
    /// SQLCipher's `PRAGMA rekey` answers "ok" even when it could not rewrite the file (it
    /// was observed with another connection holding the write lock, and by its source the
    /// same goes for an I/O error such as a full disk), so `Ok` has to be earned:
    ///
    /// 1. The write lock is taken and released first (`BEGIN IMMEDIATE`, `ROLLBACK`): a
    ///    busy or read-only database is an error before the pragma runs.
    /// 2. The pragma runs, then the readability probe.
    /// 3. The write-ahead log is checkpointed (`wal_checkpoint(TRUNCATE)`, best effort):
    ///    until then the main file still holds every page as the old key encrypted it, and
    ///    the old key would open a copy of that file.
    /// 4. For SQLCipher the file is reopened on a second, short-lived connection with the
    ///    new key and its schema table is read; an error there is returned. (sqlite3mc on
    ///    the web has one connection per database, ADR-0012, and skips this; a connection
    ///    without a file has nothing to reopen.)
    ///
    /// Errors never contain key bytes.
    pub fn rekey(&self, key: &DbKey) -> Result<(), StorageError> {
        if !self.encryption.is_encrypted() {
            return Ok(());
        }

        Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?.rollback()?;

        let sql = raw_key_pragma("rekey", key);
        let rekeyed = self
            .conn
            .execute_batch(sql.as_str())
            .map_err(map_probe_error);
        // Wipe the text of the statement before anything else can fail.
        drop(sql);
        rekeyed?;
        probe_readable(&self.conn)?;

        self.checkpoint_after_rekey();
        if matches!(self.encryption, Encryption::SqlCipher { .. }) {
            verify_key_on_disk(&self.conn, key)?;
        }
        tracing::debug!("database re-encrypted under a new key");
        Ok(())
    }

    /// Writes the write-ahead log into the main file. Best effort: the rekey is complete
    /// and durable without it, so a failure here is logged and not returned (an `Err` would
    /// send the caller back to the old key, which no longer opens the file).
    fn checkpoint_after_rekey(&self) {
        match self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            }) {
            Ok(0) => {}
            Ok(_) => tracing::warn!("the write-ahead log could not be checkpointed after rekeying"),
            Err(err) => {
                tracing::warn!(error = %err, "cannot checkpoint the write-ahead log after rekeying");
            }
        }
    }
}

impl SecurityQueries for Db {
    fn device_log_append(&self, row: &DeviceLogRow) -> Result<(), StorageError> {
        device_log_append_on(&self.conn, row)
    }
    fn device_log_all(&self) -> Result<Vec<DeviceLogRow>, StorageError> {
        device_log_all_on(&self.conn)
    }
    fn keyring_put(&self, row: &KeyringRow) -> Result<(), StorageError> {
        keyring_put_on(&self.conn, row)
    }
    fn keyring_all(&self) -> Result<Vec<KeyringRow>, StorageError> {
        keyring_all_on(&self.conn)
    }
    fn keyring_by_space(&self, space_id: SpaceId) -> Result<Vec<KeyringRow>, StorageError> {
        keyring_by_space_on(&self.conn, space_id)
    }
    fn keyring_delete_space(&self, space_id: SpaceId) -> Result<(), StorageError> {
        keyring_delete_space_on(&self.conn, space_id)
    }
    fn settings_all(&self) -> Result<Vec<SettingRow>, StorageError> {
        settings_all_on(&self.conn)
    }
    fn docs_all(&self) -> Result<Vec<DocRow>, StorageError> {
        docs_all_on(&self.conn)
    }
    fn meta_all(&self) -> Result<Vec<(String, String)>, StorageError> {
        meta_all_on(&self.conn)
    }
    fn workspace_insert(&self, row: &WorkspaceRow) -> Result<bool, StorageError> {
        workspace_create_on(&self.conn, row)
    }
    fn setting_put(&self, key: &str, value: &Json, updated_at: i64) -> Result<(), StorageError> {
        setting_set_on(&self.conn, key, value, updated_at)
    }
    fn meta_get(&self, key: &str) -> Result<Option<String>, StorageError> {
        meta_get_on(&self.conn, key)
    }
    fn meta_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        meta_set_on(&self.conn, key, value)
    }
}

impl SecurityQueries for Tx<'_> {
    fn device_log_append(&self, row: &DeviceLogRow) -> Result<(), StorageError> {
        device_log_append_on(&self.tx, row)
    }
    fn device_log_all(&self) -> Result<Vec<DeviceLogRow>, StorageError> {
        device_log_all_on(&self.tx)
    }
    fn keyring_put(&self, row: &KeyringRow) -> Result<(), StorageError> {
        keyring_put_on(&self.tx, row)
    }
    fn keyring_all(&self) -> Result<Vec<KeyringRow>, StorageError> {
        keyring_all_on(&self.tx)
    }
    fn keyring_by_space(&self, space_id: SpaceId) -> Result<Vec<KeyringRow>, StorageError> {
        keyring_by_space_on(&self.tx, space_id)
    }
    fn keyring_delete_space(&self, space_id: SpaceId) -> Result<(), StorageError> {
        keyring_delete_space_on(&self.tx, space_id)
    }
    fn settings_all(&self) -> Result<Vec<SettingRow>, StorageError> {
        settings_all_on(&self.tx)
    }
    fn docs_all(&self) -> Result<Vec<DocRow>, StorageError> {
        docs_all_on(&self.tx)
    }
    fn meta_all(&self) -> Result<Vec<(String, String)>, StorageError> {
        meta_all_on(&self.tx)
    }
    fn workspace_insert(&self, row: &WorkspaceRow) -> Result<bool, StorageError> {
        workspace_create_on(&self.tx, row)
    }
    fn setting_put(&self, key: &str, value: &Json, updated_at: i64) -> Result<(), StorageError> {
        setting_set_on(&self.tx, key, value, updated_at)
    }
    fn meta_get(&self, key: &str) -> Result<Option<String>, StorageError> {
        meta_get_on(&self.tx, key)
    }
    fn meta_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        meta_set_on(&self.tx, key, value)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// Appends a log row if, and only if, its sequence number is the next one: the highest
/// stored one plus 1, or 0 on an empty log (`MAX` of no rows is NULL, and so is NULL + 1).
/// The guard and the insert are one statement, so they are atomic even outside a
/// transaction, and the primary key would refuse a duplicate anyway.
const DEVICE_LOG_APPEND_SQL: &str = "INSERT INTO device_log (seq, entry, hash) \
     SELECT ?1, ?2, ?3 WHERE ?1 = (SELECT COALESCE(MAX(seq) + 1, 0) FROM device_log)";

/// The columns of `keyring` in the order [`KeyringColumns::read`] expects.
const KEYRING_COLUMNS: &str = "space_id, subject, record, tombstone";

fn device_log_append_on(conn: &Connection, row: &DeviceLogRow) -> Result<(), StorageError> {
    let seq = i64::try_from(row.seq).map_err(|_| {
        StorageError::InvalidData("device log sequence number is out of range".to_owned())
    })?;
    let inserted = conn.execute(
        DEVICE_LOG_APPEND_SQL,
        params![seq, row.entry, &row.hash[..]],
    )?;
    if inserted == 0 {
        return Err(StorageError::InvalidData(format!(
            "device log entry {} is not the next in sequence",
            row.seq
        )));
    }
    Ok(())
}

fn device_log_all_on(conn: &Connection) -> Result<Vec<DeviceLogRow>, StorageError> {
    let mut stmt = conn.prepare("SELECT seq, entry, hash FROM device_log ORDER BY seq")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(seq, entry, hash)| {
            Ok(DeviceLogRow {
                seq: u64::try_from(seq).map_err(|_| {
                    StorageError::InvalidData("device log sequence number is negative".to_owned())
                })?,
                entry,
                hash: <[u8; 32]>::try_from(hash.as_slice()).map_err(|_| {
                    StorageError::InvalidData("device log hash is not 32 bytes long".to_owned())
                })?,
            })
        })
        .collect()
}

/// Raw column values of a `keyring` row.
struct KeyringColumns {
    space_id: Vec<u8>,
    subject: Vec<u8>,
    record: Vec<u8>,
    tombstone: i64,
}

impl KeyringColumns {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            space_id: row.get(0)?,
            subject: row.get(1)?,
            record: row.get(2)?,
            tombstone: row.get(3)?,
        })
    }

    fn decode(self) -> Result<KeyringRow, StorageError> {
        Ok(KeyringRow {
            space_id: SpaceId::from_bytes(id_from(&self.space_id, "keyring space id")?),
            subject: self.subject,
            record: self.record,
            tombstone: match self.tombstone {
                0 => false,
                1 => true,
                _ => {
                    return Err(StorageError::InvalidData(
                        "keyring tombstone flag is neither 0 nor 1".to_owned(),
                    ));
                }
            },
        })
    }
}

fn keyring_put_on(conn: &Connection, row: &KeyringRow) -> Result<(), StorageError> {
    conn.execute(
        "INSERT OR REPLACE INTO keyring (space_id, subject, record, tombstone) \
         VALUES (?1, ?2, ?3, ?4)",
        params![
            row.space_id.to_bytes(),
            row.subject,
            row.record,
            i64::from(row.tombstone)
        ],
    )?;
    Ok(())
}

fn keyring_all_on(conn: &Connection) -> Result<Vec<KeyringRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {KEYRING_COLUMNS} FROM keyring ORDER BY space_id, subject"
    ))?;
    let rows = stmt
        .query_map([], KeyringColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(KeyringColumns::decode).collect()
}

fn keyring_by_space_on(
    conn: &Connection,
    space_id: SpaceId,
) -> Result<Vec<KeyringRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {KEYRING_COLUMNS} FROM keyring WHERE space_id = ?1 ORDER BY subject"
    ))?;
    let rows = stmt
        .query_map([space_id.to_bytes()], KeyringColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(KeyringColumns::decode).collect()
}

fn keyring_delete_space_on(conn: &Connection, space_id: SpaceId) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM keyring WHERE space_id = ?1",
        [space_id.to_bytes()],
    )?;
    Ok(())
}

fn settings_all_on(conn: &Connection) -> Result<Vec<SettingRow>, StorageError> {
    let mut stmt = conn.prepare("SELECT key, value, updated_at FROM settings ORDER BY key")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(key, text, updated_at)| {
            Ok(SettingRow {
                value: setting_from_text(&key, &text)?,
                key,
                updated_at,
            })
        })
        .collect()
}

fn meta_all_on(conn: &Connection) -> Result<Vec<(String, String)>, StorageError> {
    let mut stmt = conn.prepare("SELECT key, value FROM meta ORDER BY key")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use kb_crypto::Secret32;
    use kb_keys::{Ark, derive_db_key};
    use kb_types::testing::SeededRng;
    use kb_types::{DocId, InstallId, WorkspaceId};
    use serde_json::json;

    use super::*;
    use crate::{DocKindCode, DocQueries, Encryption};

    /// What a scenario needs of a connection or a transaction: both implement both traits.
    trait Queries: SecurityQueries + DocQueries {}
    impl<T: SecurityQueries + DocQueries> Queries for T {}

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    const fn space(n: u8) -> SpaceId {
        SpaceId::from_bytes([n; 16])
    }

    const fn doc(n: u8) -> DocId {
        DocId::from_bytes([n; 16])
    }

    /// A deterministic blob that cycles through every byte value, NUL included.
    fn blob(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| seed.wrapping_add(u8::try_from(i % 256).unwrap()))
            .collect()
    }

    /// 32 bytes counting up from `start`: every position differs, so a hash that is cut,
    /// shifted or padded does not compare equal to the original.
    fn ramp(start: u8) -> [u8; 32] {
        std::array::from_fn(|i| start.wrapping_add(u8::try_from(i).unwrap()))
    }

    /// The log row with sequence number `seq`: its entry and hash differ from every other's.
    fn log_row(seq: u64) -> DeviceLogRow {
        let n = u8::try_from(seq % 256).unwrap();
        DeviceLogRow {
            seq,
            entry: blob(40 + usize::from(n), n),
            hash: ramp(n.wrapping_mul(16)),
        }
    }

    fn key_row(space_n: u8, subject: &[u8], record: &[u8], tombstone: bool) -> KeyringRow {
        KeyringRow {
            space_id: space(space_n),
            subject: subject.to_vec(),
            record: record.to_vec(),
            tombstone,
        }
    }

    fn workspace(n: u8, name: &str) -> WorkspaceRow {
        WorkspaceRow {
            id: WorkspaceId::from_bytes([n; 16]),
            name: name.to_owned(),
            created_at: 1_000 + i64::from(n),
        }
    }

    fn doc_row(n: u8, kind: DocKindCode, space_n: Option<u8>, created_at: i64) -> DocRow {
        DocRow {
            doc_id: doc(n),
            kind,
            space_id: space_n.map(space),
            created_at,
            format_version: 1,
            min_reader_version: 1,
        }
    }

    fn setting(key: &str, value: Json, updated_at: i64) -> SettingRow {
        SettingRow {
            key: key.to_owned(),
            value,
            updated_at,
        }
    }

    fn pair(key: &str, value: &str) -> (String, String) {
        (key.to_owned(), value.to_owned())
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so
    /// that both `SecurityQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
    }

    // ---- the device log -------------------------------------------------------------

    #[test]
    fn the_device_log_starts_empty_and_appends_in_sequence_order() {
        on_db_and_tx(|q| {
            assert!(q.device_log_all().unwrap().is_empty());
            for seq in 0..4 {
                q.device_log_append(&log_row(seq)).unwrap();
            }
            assert_eq!(
                q.device_log_all().unwrap(),
                (0..4).map(log_row).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn device_log_append_refuses_anything_but_the_next_sequence_number() {
        on_db_and_tx(|q| {
            // An empty log wants 0.
            for wrong in [1, 2, 7, u64::MAX] {
                let err = q.device_log_append(&log_row(wrong)).unwrap_err();
                assert!(
                    matches!(err, StorageError::InvalidData(_)),
                    "{wrong}: {err}"
                );
            }
            assert!(
                q.device_log_all().unwrap().is_empty(),
                "nothing was written"
            );

            q.device_log_append(&log_row(0)).unwrap();
            // The same number again, a gap and a number from the past are refused too.
            for wrong in [0, 2, 3, u64::MAX] {
                let err = q.device_log_append(&log_row(wrong)).unwrap_err();
                assert!(
                    matches!(err, StorageError::InvalidData(_)),
                    "{wrong}: {err}"
                );
            }
            assert_eq!(q.device_log_all().unwrap(), [log_row(0)]);

            q.device_log_append(&log_row(1)).unwrap();
            let err = q.device_log_append(&log_row(0)).unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
            assert_eq!(q.device_log_all().unwrap(), [log_row(0), log_row(1)]);

            // A refusal does not spoil what follows.
            q.device_log_append(&log_row(2)).unwrap();
            assert_eq!(q.device_log_all().unwrap().len(), 3);
        });
    }

    #[test]
    fn a_sequence_number_that_does_not_fit_sqlite_is_invalid_data() {
        on_db_and_tx(|q| {
            let seq = u64::try_from(i64::MAX).unwrap() + 1;
            let err = q.device_log_append(&log_row(seq)).unwrap_err();
            assert!(
                matches!(&err, StorageError::InvalidData(m) if m.contains("out of range")),
                "{err}"
            );
            assert!(q.device_log_all().unwrap().is_empty());
        });
    }

    #[test]
    fn a_refused_append_does_not_echo_the_entry_or_the_hash() {
        on_db_and_tx(|q| {
            let row = DeviceLogRow {
                seq: 5,
                entry: b"SECRET-MARKER".to_vec(),
                hash: [0xAB; 32],
            };
            let text = q.device_log_append(&row).unwrap_err().to_string();
            assert!(!text.contains("SECRET-MARKER"), "{text}");
            assert!(!text.to_lowercase().contains("abab"), "{text}");
        });
    }

    #[test]
    fn device_log_blobs_are_opaque_bytes() {
        on_db_and_tx(|q| {
            let rows = [
                DeviceLogRow {
                    seq: 0,
                    entry: blob(5_000, 3),
                    hash: ramp(0xE0),
                },
                DeviceLogRow {
                    seq: 1,
                    entry: Vec::new(),
                    hash: [0; 32],
                },
                DeviceLogRow {
                    seq: 2,
                    entry: vec![0x00, 0xFF, 0x80, 0x00],
                    hash: [0xFF; 32],
                },
            ];
            for row in &rows {
                q.device_log_append(row).unwrap();
            }
            assert_eq!(q.device_log_all().unwrap(), rows);
        });
    }

    #[test]
    fn device_log_rows_are_listed_by_sequence_number_whatever_the_row_order() {
        // The guard keeps appends in order, so write the rows around it.
        let db = new_db();
        for seq in [5, 3, 4, 0] {
            db.connection()
                .execute(
                    "INSERT INTO device_log (seq, entry, hash) VALUES (?1, x'01', zeroblob(32))",
                    [seq],
                )
                .unwrap();
        }
        let seqs: Vec<u64> = db
            .device_log_all()
            .unwrap()
            .iter()
            .map(|row| row.seq)
            .collect();
        assert_eq!(seqs, [0, 3, 4, 5]);
    }

    #[test]
    fn damaged_device_log_rows_are_invalid_data_and_never_a_panic() {
        for (values, what) in [
            ("0, x'01', zeroblob(31)", "a 31-byte hash"),
            ("0, x'01', zeroblob(33)", "a 33-byte hash"),
            ("0, x'01', x''", "an empty hash"),
            ("-1, x'01', zeroblob(32)", "a negative sequence number"),
        ] {
            let db = new_db();
            db.connection()
                .execute(
                    &format!("INSERT INTO device_log (seq, entry, hash) VALUES ({values})"),
                    [],
                )
                .unwrap();
            let err = db.device_log_all().unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{what}: {err}");
        }
    }

    // ---- the keyring ----------------------------------------------------------------

    #[test]
    fn keyring_put_inserts_and_replaces_the_row_of_a_space_and_subject() {
        on_db_and_tx(|q| {
            assert!(q.keyring_all().unwrap().is_empty());
            let first = key_row(1, b"s0", b"record-1", false);
            q.keyring_put(&first).unwrap();
            assert_eq!(q.keyring_all().unwrap(), [first]);

            // The same space and subject: replaced, not duplicated.
            let second = key_row(1, b"s0", b"record-2-longer", false);
            q.keyring_put(&second).unwrap();
            assert_eq!(q.keyring_all().unwrap(), std::slice::from_ref(&second));

            // Another subject of the space and the same subject of another space are rows
            // of their own.
            let other_subject = key_row(1, b"s1", b"record-3", false);
            let other_space = key_row(2, b"s0", b"record-4", false);
            q.keyring_put(&other_subject).unwrap();
            q.keyring_put(&other_space).unwrap();
            assert_eq!(
                q.keyring_all().unwrap(),
                [second, other_subject, other_space]
            );
        });
    }

    #[test]
    fn keyring_put_swaps_a_record_for_a_tombstone_and_back_as_a_plain_replace() {
        on_db_and_tx(|q| {
            q.keyring_put(&key_row(1, b"doc", b"record", false))
                .unwrap();
            let tombstone = key_row(1, b"doc", b"tombstone", true);
            q.keyring_put(&tombstone).unwrap();
            assert_eq!(q.keyring_all().unwrap(), [tombstone]);

            // The merge rules (a tombstone wins) are kb-keys's: storage replaces the row
            // with whatever it is given.
            let record = key_row(1, b"doc", b"record-again", false);
            q.keyring_put(&record).unwrap();
            assert_eq!(q.keyring_all().unwrap(), [record]);
        });
    }

    #[test]
    fn keyring_all_orders_by_space_then_subject_bytes() {
        on_db_and_tx(|q| {
            // Written out of order. Subjects compare as bytes: a prefix comes first and
            // 0x80 and up come after 0x7f (no text collation).
            let subjects: [&[u8]; 6] =
                [b"\x02", b"\x01\x00", b"\x01", b"\x00\xff", b"\x80", b"\x7f"];
            for space_n in [3, 1, 2] {
                for (i, subject) in subjects.iter().enumerate() {
                    let record = [u8::try_from(i).unwrap()];
                    q.keyring_put(&key_row(space_n, subject, &record, false))
                        .unwrap();
                }
            }

            let all = q.keyring_all().unwrap();
            assert_eq!(all.len(), 18);
            let order: Vec<(SpaceId, Vec<u8>)> = all
                .iter()
                .map(|row| (row.space_id, row.subject.clone()))
                .collect();
            let mut expected = order.clone();
            expected.sort_by(|a, b| (a.0.as_bytes(), &a.1).cmp(&(b.0.as_bytes(), &b.1)));
            assert_eq!(order, expected, "space bytes first, then subject bytes");

            let of_space_1: Vec<Vec<u8>> = all
                .into_iter()
                .filter(|row| row.space_id == space(1))
                .map(|row| row.subject)
                .collect();
            assert_eq!(
                of_space_1,
                [
                    vec![0x00, 0xFF],
                    vec![0x01],
                    vec![0x01, 0x00],
                    vec![0x02],
                    vec![0x7F],
                    vec![0x80],
                ]
            );
        });
    }

    #[test]
    fn keyring_by_space_lists_only_that_space_ordered_by_subject_bytes() {
        on_db_and_tx(|q| {
            for (space_n, subject) in [(2, "b"), (1, "z"), (1, "a"), (2, "a"), (1, "m"), (3, "a")] {
                let row = key_row(space_n, subject.as_bytes(), subject.as_bytes(), false);
                q.keyring_put(&row).unwrap();
            }
            let subjects = |space_n| -> Vec<Vec<u8>> {
                q.keyring_by_space(space(space_n))
                    .unwrap()
                    .into_iter()
                    .map(|row| {
                        assert_eq!(row.space_id, space(space_n), "only that space");
                        row.subject
                    })
                    .collect()
            };
            assert_eq!(subjects(1), [b"a".to_vec(), b"m".to_vec(), b"z".to_vec()]);
            assert_eq!(subjects(2), [b"a".to_vec(), b"b".to_vec()]);
            assert_eq!(subjects(3), [b"a".to_vec()]);
            assert!(subjects(9).is_empty(), "an unknown space has no rows");
        });
    }

    #[test]
    fn keyring_delete_space_removes_only_that_space() {
        on_db_and_tx(|q| {
            for space_n in 1..=3 {
                for subject in [b"a", b"b"] {
                    q.keyring_put(&key_row(space_n, subject, b"r", false))
                        .unwrap();
                }
            }
            q.keyring_put(&key_row(2, b"gone", b"t", true)).unwrap();

            q.keyring_delete_space(space(2)).unwrap();
            assert!(q.keyring_by_space(space(2)).unwrap().is_empty());
            assert_eq!(q.keyring_by_space(space(1)).unwrap().len(), 2);
            assert_eq!(q.keyring_by_space(space(3)).unwrap().len(), 2);
            assert_eq!(q.keyring_all().unwrap().len(), 4);

            // A space without rows, and one that is already empty, are not an error.
            q.keyring_delete_space(space(2)).unwrap();
            q.keyring_delete_space(space(9)).unwrap();
            assert_eq!(q.keyring_all().unwrap().len(), 4);

            // The emptied space takes rows again.
            q.keyring_put(&key_row(2, b"a", b"again", false)).unwrap();
            assert_eq!(q.keyring_by_space(space(2)).unwrap().len(), 1);
        });
    }

    #[test]
    fn keyring_blobs_are_opaque_bytes() {
        on_db_and_tx(|q| {
            let rows = [
                key_row(1, &[0x00, 0xFF, 0x80, 0x00], &blob(5_000, 9), false),
                key_row(1, &[0x01], &[0x00], true),
                key_row(1, &[0x02], &[], false),
            ];
            for row in &rows {
                q.keyring_put(row).unwrap();
            }
            assert_eq!(q.keyring_all().unwrap(), rows);
        });
    }

    #[test]
    fn damaged_keyring_rows_are_invalid_data_and_never_a_panic() {
        // `zeroblob(16)` is a space id.
        for (values, what) in [
            ("x'0102', x'01', x'02', 0", "a space id of 2 bytes"),
            ("zeroblob(16), x'01', x'02', 2", "a tombstone flag of 2"),
            ("zeroblob(16), x'01', x'02', -1", "a tombstone flag of -1"),
        ] {
            let db = new_db();
            db.connection()
                .execute(&format!("INSERT INTO keyring (space_id, subject, record, tombstone) VALUES ({values})"), [])
                .unwrap();
            let err = db.keyring_all().unwrap_err();
            assert!(matches!(err, StorageError::InvalidData(_)), "{what}: {err}");
        }

        // `keyring_by_space` meets a damaged row of its space too.
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO keyring (space_id, subject, record, tombstone) VALUES (zeroblob(16), x'01', x'02', 7)",
                [],
            )
            .unwrap();
        let err = db.keyring_by_space(space(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    // ---- the whole-table reads of a backup ------------------------------------------

    #[test]
    fn settings_all_lists_every_setting_ordered_by_key_with_its_value_and_time() {
        on_db_and_tx(|q| {
            assert!(q.settings_all().unwrap().is_empty());
            let expected = [
                setting("app.locale", json!("en"), 30),
                setting("app.null", Json::Null, 10),
                setting("ui.text", json!("h\u{e9}llo \u{1F600} \"quoted\""), 5),
                setting(
                    "ui.theme",
                    json!({ "mode": "dark", "accent": [1, 2, 3], "ratio": 1.5, "on": true }),
                    20,
                ),
            ];
            // Written in the reverse of the order they are listed in.
            for row in expected.iter().rev() {
                q.setting_put(&row.key, &row.value, row.updated_at).unwrap();
            }
            assert_eq!(q.settings_all().unwrap(), expected);
        });
    }

    #[test]
    fn settings_all_reports_an_unparsable_value_without_echoing_it() {
        let db = new_db();
        db.setting_set("ui.fine", &json!(1), 1).unwrap();
        db.connection()
            .execute(
                "INSERT INTO settings (key, value, updated_at) VALUES ('ui.bad', 'SECRET-MARKER{', 2)",
                [],
            )
            .unwrap();
        let err = db.settings_all().unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        let text = err.to_string();
        assert!(!text.contains("SECRET-MARKER"), "{text}");
        assert!(text.contains("ui.bad"), "the key says which row: {text}");
    }

    #[test]
    fn docs_all_lists_every_document_of_every_kind_oldest_first_then_by_id() {
        on_db_and_tx(|q| {
            assert!(q.docs_all().unwrap().is_empty());
            // Written out of order; documents 1 and 9 share a creation time.
            let rows = [
                doc_row(5, DocKindCode::Page, Some(1), 300),
                doc_row(2, DocKindCode::Structure, Some(1), 100),
                doc_row(9, DocKindCode::Account, None, 200),
                doc_row(1, DocKindCode::Page, Some(2), 200),
                DocRow {
                    format_version: u16::MAX,
                    min_reader_version: 7,
                    ..doc_row(7, DocKindCode::Page, Some(2), 0)
                },
            ];
            for row in &rows {
                q.doc_insert(row).unwrap();
            }
            let all = q.docs_all().unwrap();
            let ids: Vec<DocId> = all.iter().map(|row| row.doc_id).collect();
            assert_eq!(ids, [doc(7), doc(2), doc(1), doc(9), doc(5)]);
            // The rows come back whole.
            for row in &rows {
                assert!(all.contains(row), "{row:?}");
            }
        });
    }

    #[test]
    fn docs_all_reports_a_damaged_row_as_invalid_data() {
        let db = new_db();
        db.connection()
            .execute("INSERT INTO docs VALUES (x'01', 0, NULL, 1, 1, 1)", [])
            .unwrap();
        let err = db.docs_all().unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    #[test]
    fn meta_all_lists_every_key_ordered_by_key() {
        on_db_and_tx(|q| {
            assert!(q.meta_all().unwrap().is_empty());
            // Written out of order; an empty value is a value.
            for (key, value) in [
                ("recovery_id", "ab"),
                ("account_id", "x"),
                ("device_id", "d"),
                ("recovery_blob", ""),
            ] {
                q.meta_set(key, value).unwrap();
            }
            assert_eq!(
                q.meta_all().unwrap(),
                [
                    pair("account_id", "x"),
                    pair("device_id", "d"),
                    pair("recovery_blob", ""),
                    pair("recovery_id", "ab"),
                ]
            );
        });
    }

    #[test]
    fn meta_all_includes_the_device_identity_keys() {
        let db = new_db();
        db.device_init(
            &crate::DeviceRow {
                device_id: kb_types::DeviceId::from_bytes([7; 16]),
                name: "Laptop".to_owned(),
                created_at: 42,
            },
            "0.1.0",
        )
        .unwrap();
        db.meta_set("account_id", "acct").unwrap();
        let keys: Vec<String> = db.meta_all().unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            [
                "account_id",
                "core_version_at_creation",
                "created_at",
                "device_id",
                "device_name"
            ]
        );
    }

    // ---- the writes next to them: workspace, settings, meta -------------------------

    #[test]
    fn workspace_insert_inserts_one_row_and_reports_false_after_that() {
        let first = workspace(1, "Home");
        let second = workspace(2, "Other");

        // On a bare `Db`.
        let db = new_db();
        assert!(
            db.workspace_insert(&first).unwrap(),
            "the first insert wins"
        );
        assert!(
            !db.workspace_insert(&second).unwrap(),
            "a second one is refused"
        );
        assert!(
            !db.workspace_insert(&first).unwrap(),
            "so is the same row again"
        );
        assert!(
            !db.workspace_create(&second).unwrap(),
            "`workspace_create` agrees"
        );
        assert_eq!(db.workspace_get().unwrap(), Some(first.clone()));

        // Inside a transaction.
        let mut db = new_db();
        let outcomes = db
            .transaction(|tx| {
                Ok((
                    tx.workspace_insert(&first)?,
                    tx.workspace_insert(&second)?,
                    tx.workspace_insert(&first)?,
                ))
            })
            .unwrap();
        assert_eq!(outcomes, (true, false, false));
        assert_eq!(db.workspace_get().unwrap(), Some(first.clone()));
        assert!(!db.workspace_create(&second).unwrap());

        // The id is a 16-byte blob, as `workspace_create` writes it.
        let (kind, len): (String, i64) = db
            .connection()
            .query_row("SELECT typeof(id), length(id) FROM workspace", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((kind.as_str(), len), ("blob", 16));
    }

    #[test]
    fn workspace_insert_is_refused_after_workspace_create_and_the_reverse() {
        let created = workspace(1, "Created");
        let db = new_db();
        assert!(db.workspace_create(&created).unwrap());
        assert!(!db.workspace_insert(&workspace(2, "Inserted")).unwrap());
        assert_eq!(db.workspace_get().unwrap(), Some(created));

        let inserted = workspace(3, "Inserted");
        let db = new_db();
        assert!(db.workspace_insert(&inserted).unwrap());
        assert!(!db.workspace_create(&workspace(4, "Created")).unwrap());
        assert_eq!(db.workspace_get().unwrap(), Some(inserted));
    }

    #[test]
    fn setting_put_inserts_and_replaces_and_keeps_the_time_it_is_given() {
        on_db_and_tx(|q| {
            q.setting_put("ui.theme", &json!({ "mode": "dark" }), 10)
                .unwrap();
            q.setting_put("app.other", &json!(2), 15).unwrap();
            // Replaced in place, with the new value and the new time.
            q.setting_put("ui.theme", &json!("light"), 20).unwrap();
            assert_eq!(
                q.settings_all().unwrap(),
                [
                    setting("app.other", json!(2), 15),
                    setting("ui.theme", json!("light"), 20),
                ]
            );

            // The time is the caller's, whatever it is; `null` is a value.
            q.setting_put("app.early", &Json::Null, 0).unwrap();
            q.setting_put("app.before", &json!(false), -5).unwrap();
            let all = q.settings_all().unwrap();
            assert!(all.contains(&setting("app.early", Json::Null, 0)));
            assert!(all.contains(&setting("app.before", json!(false), -5)));
            assert_eq!(all.len(), 4);
        });
    }

    #[test]
    fn setting_put_and_setting_set_write_the_same_rows() {
        let db = new_db();
        let value = json!({ "a": [1, 2, { "b": null }], "c": "d\u{e9}" });
        db.setting_set("app.set", &value, 7).unwrap();
        db.setting_put("app.put", &value, 7).unwrap();
        assert_eq!(db.setting_get("app.set").unwrap(), Some(value.clone()));
        assert_eq!(db.setting_get("app.put").unwrap(), Some(value.clone()));
        let rows = db.settings_all().unwrap();
        assert_eq!(rows[0].value, rows[1].value);
        assert_eq!(rows[0].updated_at, rows[1].updated_at);

        // Each replaces what the other wrote.
        db.setting_put("app.set", &json!("put"), 8).unwrap();
        db.setting_set("app.put", &json!("set"), 9).unwrap();
        assert_eq!(db.setting_get("app.set").unwrap(), Some(json!("put")));
        assert_eq!(db.setting_get("app.put").unwrap(), Some(json!("set")));
    }

    #[test]
    fn meta_get_and_meta_set_read_and_upsert_through_either_impl() {
        on_db_and_tx(|q| {
            assert_eq!(q.meta_get("account_id").unwrap(), None);
            q.meta_set("account_id", "one").unwrap();
            assert_eq!(q.meta_get("account_id").unwrap().as_deref(), Some("one"));
            q.meta_set("account_id", "two").unwrap();
            assert_eq!(q.meta_get("account_id").unwrap().as_deref(), Some("two"));

            // An empty value is a value, not "missing"; other keys are untouched.
            q.meta_set("recovery_id", "").unwrap();
            assert_eq!(q.meta_get("recovery_id").unwrap().as_deref(), Some(""));
            assert_eq!(q.meta_get("account_id").unwrap().as_deref(), Some("two"));
            assert_eq!(q.meta_get("other").unwrap(), None);
        });
    }

    #[test]
    fn the_trait_and_the_inherent_meta_methods_of_db_see_the_same_rows() {
        let db = new_db();
        db.meta_set("written.inherent", "a").unwrap();
        SecurityQueries::meta_set(&db, "written.trait", "b").unwrap();
        assert_eq!(
            SecurityQueries::meta_get(&db, "written.inherent")
                .unwrap()
                .as_deref(),
            Some("a")
        );
        assert_eq!(db.meta_get("written.trait").unwrap().as_deref(), Some("b"));
        assert_eq!(
            db.meta_all().unwrap(),
            [pair("written.inherent", "a"), pair("written.trait", "b")]
        );
    }

    // ---- transactions ---------------------------------------------------------------

    #[test]
    fn security_writes_commit_with_the_transaction_and_reads_see_them_before_that() {
        let mut db = new_db();
        let home = workspace(1, "Home");
        db.transaction(|tx| {
            assert!(tx.workspace_insert(&home)?);
            tx.setting_put("ui.theme", &json!("dark"), 5)?;
            tx.meta_set("account_id", "a")?;
            tx.device_log_append(&log_row(0))?;
            tx.device_log_append(&log_row(1))?;
            tx.keyring_put(&key_row(1, b"s", b"r", false))?;

            // The transaction reads its own writes.
            assert_eq!(tx.device_log_all()?, [log_row(0), log_row(1)]);
            assert_eq!(tx.keyring_by_space(space(1))?.len(), 1);
            assert_eq!(tx.meta_get("account_id")?.as_deref(), Some("a"));
            assert_eq!(tx.settings_all()?.len(), 1);
            Ok(())
        })
        .unwrap();

        assert!(db.connection().is_autocommit(), "the transaction is over");
        assert_eq!(db.workspace_get().unwrap(), Some(home));
        assert_eq!(db.settings_all().unwrap().len(), 1);
        assert_eq!(db.meta_get("account_id").unwrap().as_deref(), Some("a"));
        assert_eq!(db.device_log_all().unwrap(), [log_row(0), log_row(1)]);
        assert_eq!(db.keyring_all().unwrap(), [key_row(1, b"s", b"r", false)]);
    }

    #[test]
    fn security_writes_roll_back_with_a_transaction_that_fails() {
        let mut db = new_db();
        // Some committed state first.
        db.device_log_append(&log_row(0)).unwrap();
        db.keyring_put(&key_row(1, b"keep", b"r", false)).unwrap();
        db.meta_set("keep", "yes").unwrap();
        db.setting_put("keep", &json!(1), 1).unwrap();

        let err = db
            .transaction(|tx| {
                tx.device_log_append(&log_row(1))?;
                tx.device_log_append(&log_row(2))?;
                tx.keyring_put(&key_row(1, b"new", b"r", false))?;
                tx.keyring_put(&key_row(1, b"keep", b"replaced", true))?;
                tx.keyring_delete_space(space(1))?;
                tx.keyring_put(&key_row(2, b"other", b"r", false))?;
                assert!(tx.workspace_insert(&workspace(1, "Home"))?);
                tx.setting_put("ui.theme", &json!("dark"), 9)?;
                tx.setting_put("keep", &json!(2), 9)?;
                tx.meta_set("account_id", "a")?;
                tx.meta_set("keep", "no")?;
                Err::<(), _>(StorageError::InvalidData("boom".to_owned()))
            })
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::InvalidData(m) if m == "boom"),
            "{err}"
        );

        // What was committed before is exactly as it was.
        assert_eq!(db.device_log_all().unwrap(), [log_row(0)]);
        assert_eq!(
            db.keyring_all().unwrap(),
            [key_row(1, b"keep", b"r", false)]
        );
        assert_eq!(db.workspace_get().unwrap(), None);
        assert_eq!(db.settings_all().unwrap(), [setting("keep", json!(1), 1)]);
        assert_eq!(db.meta_all().unwrap(), [pair("keep", "yes")]);
        assert!(db.connection().is_autocommit(), "the transaction is over");

        // The connection is fine afterwards, and the numbers that were rolled back are
        // free again.
        db.transaction(|tx| {
            tx.device_log_append(&log_row(1))?;
            assert!(tx.workspace_insert(&workspace(2, "Again"))?);
            Ok(())
        })
        .unwrap();
        assert_eq!(db.device_log_all().unwrap().len(), 2);
    }

    #[test]
    fn a_refused_append_rolls_back_the_transaction_around_it() {
        let mut db = new_db();
        let err = db
            .transaction(|tx| {
                tx.device_log_append(&log_row(0))?;
                tx.keyring_put(&key_row(1, b"s", b"r", false))?;
                // Not the next number: the `?` ends the transaction with an error.
                tx.device_log_append(&log_row(5))?;
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        assert!(db.device_log_all().unwrap().is_empty());
        assert!(db.keyring_all().unwrap().is_empty());
    }

    #[test]
    fn a_panic_inside_the_transaction_rolls_the_security_writes_back() {
        let mut db = new_db();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _ = db.transaction::<()>(|tx| {
                tx.device_log_append(&log_row(0))?;
                tx.keyring_put(&key_row(1, b"s", b"r", false))?;
                tx.meta_set("account_id", "a")?;
                panic!("the closure panics after writing");
            });
        }));
        assert!(outcome.is_err());
        assert!(
            db.device_log_all().unwrap().is_empty(),
            "rolled back by drop"
        );
        assert!(db.keyring_all().unwrap().is_empty());
        assert_eq!(db.meta_get("account_id").unwrap(), None);
        assert!(db.connection().is_autocommit());
        db.device_log_append(&log_row(0)).unwrap();
    }

    // ---- rekey ----------------------------------------------------------------------

    #[test]
    fn rekey_without_encryption_is_a_no_op() {
        let ark = Ark::from_secret(Secret32::generate(&SeededRng::new(1)));
        let key = derive_db_key(&ark, &InstallId::from_random_bytes([7; 16]));

        let db = new_db();
        assert_eq!(db.encryption(), &Encryption::None);
        db.meta_set("k", "v").unwrap();
        db.device_log_append(&log_row(0)).unwrap();
        db.rekey(&key).unwrap();
        db.rekey(&key).unwrap();
        assert_eq!(db.meta_get("k").unwrap().as_deref(), Some("v"));
        assert_eq!(db.device_log_all().unwrap(), [log_row(0)]);
        assert!(db.connection().is_autocommit());
    }
}
