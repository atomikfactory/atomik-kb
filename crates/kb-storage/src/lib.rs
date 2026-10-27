//! SQLite storage for atomik-kb (ADR-0012, `docs/data-model/sqlite-schema.md`).
//!
//! All SQL lives here. Platform adapters only decide *how* the database is opened
//! (SQLCipher file natively, sqlite3mc over OPFS on the web) through [`DbOpener`].
//! The core receives a [`Db`] and never touches `rusqlite` directly.
//!
//! Scope so far: schema v1 (`meta`, `workspace`, `settings`), schema v2 (documents and
//! the page index, see the `docs` module), schema v3 (the link index and full-text
//! search, see the `text` module), schema v4 (the device log and keyring, see the
//! `security` module), schema v5 (the attachment index and references, see the
//! `attachments` module; the chunks themselves live behind the `blob` module's
//! `BlobStore`), schema v6 (the sync state and the `published` markers of the keyring and
//! the device log, see the `sync` module), schema v7 (the pending table keyed by cursor and
//! document), schema v8 (the per-chunk transfer state of attachment sync, see the
//! `chunk_sync` module), schema v9 (the date mentions of the pages, the reminders' derived
//! table, see the `date_mentions` module), schema v10 (the property values of the pages and
//! the property rows of the text index, see the `page_properties` module and `text`), schema v11
//! (the `files` and `checklist` kinds of those values, which also count as attachment
//! references), schema v12 (the `place` kind), schema v13 (the `relation` kind and `related_page_id`),
//! schema v14 (`icon_refs`: the image icons of pages, which also count as attachment references, see
//! the `icon_refs` module), schema v16 (the same table for the image covers of pages), schema v17
//! (the feeds, the co-members' device logs, the quarantine, the snapshot marks and the membership
//! log of shared spaces, see the `sharing` module, and the `feed` column of the sync tables, see the
//! `sync` module), schema v19 (the first picture of each page, for the gallery, see the `page_first_images`
//! module), schema v20 (`pages_index.in_template`, which keeps templates out of the lists, see the `docs`
//! module), schema v21 (the page history journal, the sessions of each page document this device
//! witnessed, see the `page_history` module), migrations, encrypted open helpers and the queries the
//! core API needs.

#![forbid(unsafe_code)]

use kb_keys::DbKey;
use kb_types::{DeviceId, Json, MaybeSend, WorkspaceId};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use rusqlite_migration::{M, Migrations};
use zeroize::Zeroizing;

pub use rusqlite;

#[cfg(not(target_arch = "wasm32"))]
pub mod sqlcipher;

mod docs;
pub use docs::*;
mod fuzzy;
pub use fuzzy::*;
mod text;
pub use text::*;
mod security;
pub use security::*;
mod blob;
pub use blob::*;
mod attachments;
pub use attachments::*;
mod sync;
pub use sync::*;
mod chunk_sync;
pub use chunk_sync::*;
mod date_mentions;
pub use date_mentions::*;
mod page_properties;
pub use page_properties::*;
mod icon_refs;
pub use icon_refs::*;
mod ai;
pub use ai::*;
mod mcp;
pub use mcp::*;
mod sharing;
pub use sharing::*;
mod comments;
pub use comments::*;
mod page_first_images;
pub use page_first_images::*;
mod page_history;
pub use page_history::*;

/// Current schema version (`PRAGMA user_version` after migration).
pub const SCHEMA_VERSION: u32 = 21;

/// `meta` keys that make up this device's identity.
const META_DEVICE_ID: &str = "device_id";
const META_DEVICE_NAME: &str = "device_name";
const META_CREATED_AT: &str = "created_at";
const META_CORE_VERSION: &str = "core_version_at_creation";

/// Storage errors. Mapped to `ApiError::Storage` / `ApiError::Crypto` by the core.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Underlying SQLite error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Migration failure.
    #[error("migration error: {0}")]
    Migration(String),
    /// The file is not a database we can read with this key (wrong key or corruption).
    #[error("wrong key or corrupted database")]
    WrongKey,
    /// Stored data violates an invariant (bad length, bad JSON, ...).
    #[error("invalid data: {0}")]
    InvalidData(String),
    /// The platform could not open the database file or storage.
    #[error("cannot open database: {0}")]
    Open(String),
    /// The blob store (attachment chunks) failed; the message says what, never a path or
    /// a chunk's content.
    #[error("blob store error: {0}")]
    Blob(String),
}

/// What encryption the opened connection reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encryption {
    /// SQLCipher (native builds); `version` from `PRAGMA cipher_version`.
    SqlCipher {
        /// e.g. `"4.14.0 community"`.
        version: String,
    },
    /// SQLite3MultipleCiphers (web builds); `cipher` from `PRAGMA cipher`.
    Sqlite3Mc {
        /// e.g. `"chacha20"`.
        cipher: String,
    },
    /// No encryption (in-memory test databases only).
    None,
}

impl Encryption {
    /// `true` for any real cipher.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        !matches!(self, Encryption::None)
    }

    /// Human-readable description for `AppStatus::db_cipher`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Encryption::SqlCipher { version } => format!("SQLCipher {version}"),
            Encryption::Sqlite3Mc { cipher } => format!("SQLite3MultipleCiphers {cipher}"),
            Encryption::None => "none".to_owned(),
        }
    }
}

/// A freshly opened, keyed, not-yet-migrated connection.
#[derive(Debug)]
pub struct OpenedDb {
    /// The connection, already keyed and verified readable.
    pub conn: rusqlite::Connection,
    /// Encryption in effect.
    pub encryption: Encryption,
}

/// Platform hook: opens (creating if needed) the database with the given raw key.
pub trait DbOpener: MaybeSend {
    /// Opens the database. Must return [`StorageError::WrongKey`] when the file exists
    /// but cannot be read with `key`.
    fn open(&self, key: &DbKey) -> Result<OpenedDb, StorageError>;
}

/// The workspace row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    /// Id.
    pub id: WorkspaceId,
    /// Name.
    pub name: String,
    /// Creation time, Unix ms.
    pub created_at: i64,
}

/// This device's identity, stored in `meta` (keys `device_id`, `device_name`,
/// `created_at`, `core_version_at_creation`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// Random id generated on first launch.
    pub device_id: DeviceId,
    /// User-visible name.
    pub name: String,
    /// First-launch time, Unix ms.
    pub created_at: i64,
}

/// A migrated database. All queries are methods here.
#[derive(Debug)]
pub struct Db {
    conn: rusqlite::Connection,
    encryption: Encryption,
}

impl Db {
    /// Runs migrations to [`SCHEMA_VERSION`] and returns the ready database.
    ///
    /// Turns foreign keys on, checks that the database is readable (a wrong key or a
    /// foreign file is [`StorageError::WrongKey`] even if the opener did not check),
    /// applies the pending migrations in order and verifies that `PRAGMA user_version`
    /// equals [`SCHEMA_VERSION`] afterwards. A database written by a newer version of
    /// the app is refused with [`StorageError::Migration`].
    pub fn new(opened: OpenedDb) -> Result<Self, StorageError> {
        let OpenedDb {
            mut conn,
            encryption,
        } = opened;

        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        probe_readable(&conn)?;
        migrations().to_latest(&mut conn).map_err(migration_error)?;

        let db = Self { conn, encryption };
        let version = db.schema_version()?;
        if version != SCHEMA_VERSION {
            return Err(StorageError::Migration(format!(
                "schema version is {version} after migration, expected {SCHEMA_VERSION}"
            )));
        }
        tracing::debug!(
            schema_version = version,
            encryption = %db.encryption.describe(),
            "database ready"
        );
        Ok(db)
    }

    /// Unencrypted in-memory database for fast tests.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn = rusqlite::Connection::open_in_memory()?;
        Self::new(OpenedDb {
            conn,
            encryption: Encryption::None,
        })
    }

    /// Encryption in effect.
    #[must_use]
    pub fn encryption(&self) -> &Encryption {
        &self.encryption
    }

    /// `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<u32, StorageError> {
        let raw: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        u32::try_from(raw)
            .map_err(|_| StorageError::InvalidData("schema version is out of range".to_owned()))
    }

    /// Reads a `meta` value.
    pub fn meta_get(&self, key: &str) -> Result<Option<String>, StorageError> {
        meta_get_on(&self.conn, key)
    }

    /// Writes a `meta` value (upsert).
    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        meta_set_on(&self.conn, key, value)
    }

    /// The single workspace row, if onboarding happened.
    pub fn workspace_get(&self) -> Result<Option<WorkspaceRow>, StorageError> {
        let row = self
            .conn
            .query_row(
                "SELECT id, name, created_at FROM workspace ORDER BY created_at, id LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, name, created_at)) = row else {
            return Ok(None);
        };
        let id = <[u8; 16]>::try_from(id.as_slice()).map_err(|_| {
            StorageError::InvalidData("workspace id is not 16 bytes long".to_owned())
        })?;
        Ok(Some(WorkspaceRow {
            id: WorkspaceId::from_bytes(id),
            name,
            created_at,
        }))
    }

    /// Inserts the workspace row. Returns `Ok(false)` if one already exists.
    pub fn workspace_create(&self, row: &WorkspaceRow) -> Result<bool, StorageError> {
        workspace_create_on(&self.conn, row)
    }

    /// Reads a setting as JSON.
    pub fn setting_get(&self, key: &str) -> Result<Option<Json>, StorageError> {
        let text: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        text.map(|text| setting_from_text(key, &text)).transpose()
    }

    /// Writes a setting (upsert) with `updated_at = now_ms`.
    pub fn setting_set(&self, key: &str, value: &Json, now_ms: i64) -> Result<(), StorageError> {
        setting_set_on(&self.conn, key, value, now_ms)
    }

    /// This device's identity from `meta`, or `None` on a fresh database.
    pub fn device_get(&self) -> Result<Option<DeviceRow>, StorageError> {
        let Some(id_text) = self.meta_get(META_DEVICE_ID)? else {
            return Ok(None);
        };
        let device_id = id_text
            .parse::<DeviceId>()
            .map_err(|_| StorageError::InvalidData("stored device id is malformed".to_owned()))?;
        let name = self.meta_get(META_DEVICE_NAME)?.ok_or_else(|| {
            StorageError::InvalidData("the device name is missing from meta".to_owned())
        })?;
        let created_at = self
            .meta_get(META_CREATED_AT)?
            .ok_or_else(|| {
                StorageError::InvalidData(
                    "the device creation time is missing from meta".to_owned(),
                )
            })?
            .parse::<i64>()
            .map_err(|_| {
                StorageError::InvalidData("the stored device creation time is malformed".to_owned())
            })?;
        Ok(Some(DeviceRow {
            device_id,
            name,
            created_at,
        }))
    }

    /// Records the device identity on first launch, in one transaction. Fails with
    /// `InvalidData` if an identity already exists.
    pub fn device_init(&self, row: &DeviceRow, core_version: &str) -> Result<(), StorageError> {
        // IMMEDIATE: take the write lock before the existence check, so two openers
        // cannot both decide the identity is missing.
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if meta_get_on(&tx, META_DEVICE_ID)?.is_some() {
            return Err(StorageError::InvalidData(
                "the device identity is already initialised".to_owned(),
            ));
        }
        meta_set_on(&tx, META_DEVICE_ID, &row.device_id.to_string())?;
        meta_set_on(&tx, META_DEVICE_NAME, &row.name)?;
        meta_set_on(&tx, META_CREATED_AT, &row.created_at.to_string())?;
        meta_set_on(&tx, META_CORE_VERSION, core_version)?;
        tx.commit()?;
        Ok(())
    }

    /// Renames this device.
    pub fn device_rename(&self, name: &str) -> Result<(), StorageError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if meta_get_on(&tx, META_DEVICE_ID)?.is_none() {
            return Err(StorageError::InvalidData(
                "there is no device identity to rename".to_owned(),
            ));
        }
        meta_set_on(&tx, META_DEVICE_NAME, name)?;
        tx.commit()?;
        Ok(())
    }

    /// Direct connection access for the storage crate's own tests and tooling.
    #[doc(hidden)]
    #[must_use]
    pub fn connection(&self) -> &rusqlite::Connection {
        &self.conn
    }
}

/// The forward-only migration list: entry N takes the database to `user_version = N`.
/// A released migration is never edited; a change is a new numbered file.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("../migrations/001_initial.sql")),
        M::up(include_str!("../migrations/002_documents.sql")),
        M::up(include_str!("../migrations/003_text_search.sql")),
        M::up(include_str!("../migrations/004_keys.sql")),
        M::up(include_str!("../migrations/005_attachments.sql")),
        M::up(include_str!("../migrations/006_sync.sql")),
        M::up(include_str!("../migrations/007_sync_pending_key.sql")),
        M::up(include_str!("../migrations/008_chunk_sync.sql")),
        M::up(include_str!("../migrations/009_date_mentions.sql")),
        M::up(include_str!("../migrations/010_page_properties.sql")),
        M::up(include_str!(
            "../migrations/011_files_and_checklist_properties.sql"
        )),
        M::up(include_str!("../migrations/012_place_property.sql")),
        M::up(include_str!("../migrations/013_relation_property.sql")),
        M::up(include_str!("../migrations/014_icon_refs.sql")),
        M::up(include_str!("../migrations/015_ai_and_mcp.sql")),
        M::up(include_str!("../migrations/016_cover_refs.sql")),
        M::up(include_str!("../migrations/017_shared_spaces.sql")),
        M::up(include_str!("../migrations/018_comments_people.sql")),
        M::up(include_str!("../migrations/019_page_first_images.sql")),
        M::up(include_str!("../migrations/020_templates.sql")),
        M::up(include_str!("../migrations/021_page_history_journal.sql")),
    ])
}

fn migration_error(err: rusqlite_migration::Error) -> StorageError {
    use rusqlite_migration::{Error, MigrationDefinitionError};
    match err {
        Error::MigrationDefinition(MigrationDefinitionError::DatabaseTooFarAhead) => {
            StorageError::Migration(
                "the database was written by a newer version of the app".to_owned(),
            )
        }
        other => StorageError::Migration(other.to_string()),
    }
}

fn meta_get_on(conn: &Connection, key: &str) -> Result<Option<String>, StorageError> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

fn meta_set_on(conn: &Connection, key: &str, value: &str) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Inserts the workspace row unless one exists (`Ok(false)` then). One statement, so the
/// existence check and the insert are atomic.
fn workspace_create_on(conn: &Connection, row: &WorkspaceRow) -> Result<bool, StorageError> {
    let inserted = conn.execute(
        "INSERT INTO workspace (id, name, created_at) \
         SELECT ?1, ?2, ?3 WHERE NOT EXISTS (SELECT 1 FROM workspace)",
        params![&row.id.as_bytes()[..], row.name, row.created_at],
    )?;
    Ok(inserted == 1)
}

/// Writes a setting (upsert) with the given `updated_at`.
fn setting_set_on(
    conn: &Connection,
    key: &str,
    value: &Json,
    updated_at: i64,
) -> Result<(), StorageError> {
    let text = serde_json::to_string(value).map_err(|e| {
        StorageError::InvalidData(format!("setting `{key}` cannot be serialised: {e}"))
    })?;
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, text, updated_at],
    )?;
    Ok(())
}

/// Parses the stored text of a setting. The parse error carries a position, never the
/// stored text.
fn setting_from_text(key: &str, text: &str) -> Result<Json, StorageError> {
    serde_json::from_str::<Json>(text)
        .map_err(|e| StorageError::InvalidData(format!("setting `{key}` is not valid JSON: {e}")))
}

/// Builds `PRAGMA <name> = "x'<64 hex digits>'";` (SQLCipher's raw-key form, no passphrase
/// KDF) with the key as lowercase hex; `name` is `key` or `rekey`.
///
/// The text carries the key, so it is returned in a [`Zeroizing`] string that is wiped on
/// drop, and it is allocated with its exact length so that building it never leaves an
/// unwiped copy behind. SQLite's own copy of a statement is out of our reach, which is
/// inherent to keying through a `PRAGMA` (the alternative, `sqlite3_key`, needs `unsafe`).
pub(crate) fn raw_key_pragma(name: &str, key: &DbKey) -> Zeroizing<String> {
    const PREFIX: &str = "PRAGMA ";
    const MIDDLE: &str = " = \"x'";
    const SUFFIX: &str = "'\";";
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let bytes = key.secret().expose();
    let mut sql = Zeroizing::new(String::with_capacity(
        PREFIX.len() + name.len() + MIDDLE.len() + 2 * bytes.len() + SUFFIX.len(),
    ));
    sql.push_str(PREFIX);
    sql.push_str(name);
    sql.push_str(MIDDLE);
    for byte in bytes {
        sql.push(char::from(HEX[usize::from(byte >> 4)]));
        sql.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    sql.push_str(SUFFIX);
    sql
}

/// Whether closing `conn` may checkpoint the write-ahead log into the database file
/// (`SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE`, inverted). Off while a key is being probed: a
/// connection that could not read the file has no business rewriting it, and SQLite
/// would otherwise checkpoint and delete the log a killed process left behind
/// (`sqlcipher::open_file` has the full story).
pub(crate) fn set_checkpoint_on_close(
    conn: &Connection,
    enabled: bool,
) -> Result<(), StorageError> {
    conn.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        !enabled,
    )
    .map(|_| ())
    .map_err(StorageError::Sqlite)
}

/// Reads the schema table, which decrypts the first page of an encrypted database.
pub(crate) fn probe_readable(conn: &Connection) -> Result<(), StorageError> {
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(map_probe_error)?;
    Ok(())
}

/// Opens the file behind `conn` a second time, on a throw-away connection keyed with `key`,
/// and reads its schema table: proof that the file as it is on disk opens with `key`.
/// [`Db::rekey`] uses it for SQLCipher because `PRAGMA rekey` does not report every
/// failure. A connection without a file (an in-memory or temporary database) has nothing to
/// open again and passes.
///
/// The WAL mode of the file lets this second connection read while `conn` stays open. A
/// file that does not open is [`StorageError::Open`], whatever the reason; the message
/// carries the reason and never the key.
pub(crate) fn verify_key_on_disk(conn: &Connection, key: &DbKey) -> Result<(), StorageError> {
    let Some(path) = conn.path().filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    let reopen_failed = |e: StorageError| {
        StorageError::Open(format!("the file does not open with the new key: {e}"))
    };
    let other = Connection::open(path)
        .map_err(|e| StorageError::Open(format!("cannot reopen the database file: {e}")))?;
    // A probe only reads; `conn` owns the file and its write-ahead log.
    set_checkpoint_on_close(&other, false)?;
    let sql = raw_key_pragma("key", key);
    other
        .execute_batch(sql.as_str())
        .map_err(|e| reopen_failed(map_probe_error(e)))?;
    drop(sql);
    probe_readable(&other).map_err(reopen_failed)
}

/// Maps the error of a statement that has to read an encrypted database for the first
/// time. `SQLITE_NOTADB` ("file is not a database") is what a wrong key, a plaintext
/// file and a foreign file all look like.
pub(crate) fn map_probe_error(err: rusqlite::Error) -> StorageError {
    match &err {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::NotADatabase =>
        {
            StorageError::WrongKey
        }
        _ => StorageError::Sqlite(err),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use kb_types::testing::SeededRng;
    use serde_json::json;

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn device(seed: u64, name: &str, created_at: i64) -> DeviceRow {
        DeviceRow {
            device_id: DeviceId::generate(&SeededRng::new(seed)),
            name: name.to_owned(),
            created_at,
        }
    }

    fn workspace(seed: u64, name: &str, created_at: i64) -> WorkspaceRow {
        WorkspaceRow {
            id: WorkspaceId::generate(&SeededRng::new(seed)),
            name: name.to_owned(),
            created_at,
        }
    }

    /// `(name, type, notnull, pk)` for every column of `table`, in order.
    fn columns(conn: &Connection, table: &str) -> Vec<(String, String, bool, bool)> {
        let mut stmt = conn
            .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid")
            .unwrap();
        stmt.query_map([table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, i64>(3)? != 0,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    fn col(name: &str, ty: &str, notnull: bool, pk: bool) -> (String, String, bool, bool) {
        (name.to_owned(), ty.to_owned(), notnull, pk)
    }

    #[test]
    fn fresh_database_is_migrated_to_the_schema_version() {
        let db = new_db();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.encryption(), &Encryption::None);
        assert!(!db.encryption().is_encrypted());
        assert_eq!(db.encryption().describe(), "none");
    }

    #[test]
    fn migration_list_is_valid_and_ends_at_the_schema_version() {
        migrations().validate().unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version, SCHEMA_VERSION,
            "SCHEMA_VERSION tracks the migrations"
        );
        // Migrating again is a no-op.
        migrations().to_latest(&mut conn).unwrap();
    }

    /// Names of the tables, sorted.
    fn table_names(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Every index made by an explicit `CREATE INDEX` as `(name, columns)`, by name.
    fn explicit_indexes(conn: &Connection) -> Vec<(String, Vec<String>)> {
        let names: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'index' AND sql IS NOT NULL ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        names
            .into_iter()
            .map(|name| {
                let indexed = conn
                    .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                    .unwrap()
                    .query_map([&name], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                (name, indexed)
            })
            .collect()
    }

    /// `(parent table, child column, parent column, on delete)` of the foreign keys of `table`.
    fn foreign_keys(conn: &Connection, table: &str) -> Vec<(String, String, String, String)> {
        conn.prepare(
            "SELECT \"table\", \"from\", \"to\", on_delete FROM pragma_foreign_key_list(?1) \
             ORDER BY id",
        )
        .unwrap()
        .query_map([table], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    #[test]
    fn schema_v1_matches_the_documented_tables() {
        // The frozen baseline: a fresh database migrated to exactly version 1.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 1).unwrap();
        assert_eq!(table_names(&conn), ["meta", "settings", "workspace"]);

        assert_eq!(
            columns(&conn, "meta"),
            [
                col("key", "TEXT", false, true),
                col("value", "TEXT", true, false)
            ]
        );
        assert_eq!(
            columns(&conn, "workspace"),
            [
                col("id", "BLOB", false, true),
                col("name", "TEXT", true, false),
                col("created_at", "INTEGER", true, false),
            ]
        );
        assert_eq!(
            columns(&conn, "settings"),
            [
                col("key", "TEXT", false, true),
                col("value", "TEXT", true, false),
                col("updated_at", "INTEGER", true, false),
            ]
        );
    }

    #[test]
    fn schema_v2_matches_the_documented_tables() {
        // The frozen v2 schema: a fresh database migrated to exactly version 2.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 2).unwrap();
        let conn = &conn;
        assert_eq!(
            table_names(conn),
            [
                "doc_snapshots",
                "doc_updates",
                "docs",
                "meta",
                "pages_index",
                "settings",
                "workspace"
            ]
        );

        // v1 is untouched.
        assert_eq!(columns(conn, "meta").len(), 2);
        assert_eq!(columns(conn, "workspace").len(), 3);
        assert_eq!(columns(conn, "settings").len(), 3);

        assert_eq!(
            columns(conn, "docs"),
            [
                col("doc_id", "BLOB", false, true),
                col("kind", "INTEGER", true, false),
                col("space_id", "BLOB", false, false),
                col("created_at", "INTEGER", true, false),
                col("format_version", "INTEGER", true, false),
                col("min_reader_version", "INTEGER", true, false),
            ]
        );
        assert_eq!(
            columns(conn, "doc_snapshots"),
            [
                col("doc_id", "BLOB", false, true),
                col("version", "BLOB", true, false),
                col("blob", "BLOB", true, false),
                col("created_at", "INTEGER", true, false),
            ]
        );
        assert_eq!(
            columns(conn, "doc_updates"),
            [
                col("doc_id", "BLOB", true, true),
                col("local_seq", "INTEGER", true, true),
                col("blob", "BLOB", true, false),
                col("origin", "INTEGER", true, false),
                col("created_at", "INTEGER", true, false),
            ]
        );
        assert_eq!(
            columns(conn, "pages_index"),
            [
                col("page_id", "BLOB", false, true),
                col("space_id", "BLOB", true, false),
                col("parent_id", "BLOB", false, false),
                col("title", "TEXT", true, false),
                col("trashed_at", "INTEGER", false, false),
                col("updated_at", "INTEGER", true, false),
            ]
        );

        // A document owns its snapshot and its updates.
        for table in ["doc_snapshots", "doc_updates"] {
            assert_eq!(
                foreign_keys(conn, table),
                [(
                    "docs".to_owned(),
                    "doc_id".to_owned(),
                    "doc_id".to_owned(),
                    "CASCADE".to_owned()
                )],
                "{table}"
            );
        }
        assert!(foreign_keys(conn, "docs").is_empty());
        assert!(foreign_keys(conn, "pages_index").is_empty());

        let indexed = |name: &str, cols: &[&str]| {
            (
                name.to_owned(),
                cols.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            explicit_indexes(conn),
            [
                indexed("docs_kind", &["kind"]),
                indexed("docs_space", &["space_id"]),
                indexed("pages_index_space", &["space_id"]),
                indexed("pages_index_space_trashed", &["space_id", "trashed_at"]),
            ]
        );
    }

    #[test]
    fn schema_v3_matches_the_documented_tables() {
        // The frozen v3 schema: a fresh database migrated to exactly version 3.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 3).unwrap();
        let conn = &conn;

        // `fts` is a virtual table that brings its own shadow tables; it is an external
        // content table over `fts_blocks`, so there is no `fts_content`.
        assert_eq!(
            table_names(conn),
            [
                "doc_snapshots",
                "doc_updates",
                "docs",
                "fts",
                "fts_blocks",
                "fts_config",
                "fts_data",
                "fts_docsize",
                "fts_idx",
                "links",
                "meta",
                "pages_index",
                "settings",
                "workspace"
            ]
        );

        // The v1 and v2 tables are untouched.
        assert_eq!(columns(conn, "meta").len(), 2);
        assert_eq!(columns(conn, "workspace").len(), 3);
        assert_eq!(columns(conn, "settings").len(), 3);
        assert_eq!(columns(conn, "docs").len(), 6);
        assert_eq!(columns(conn, "doc_snapshots").len(), 4);
        assert_eq!(columns(conn, "doc_updates").len(), 5);
        assert_eq!(columns(conn, "pages_index").len(), 6);

        assert_eq!(
            columns(conn, "links"),
            [
                col("from_page", "BLOB", true, true),
                col("from_block", "BLOB", true, true),
                col("to_page", "BLOB", true, true),
                col("to_block", "BLOB", false, false),
            ]
        );
        assert!(foreign_keys(conn, "links").is_empty());

        // The text of the index: ids as blobs, a NULL block id for the title row.
        assert_eq!(
            columns(conn, "fts_blocks"),
            [
                col("id", "INTEGER", false, true),
                col("page_id", "BLOB", true, false),
                col("block_id", "BLOB", false, false),
                col("title", "TEXT", true, false),
                col("body", "TEXT", true, false),
            ]
        );
        assert!(foreign_keys(conn, "fts_blocks").is_empty());

        // The word index over its `title` and `body`, in the order `snippet()` and `bm25()`
        // count the columns (0 and 1).
        assert_eq!(
            columns(conn, "fts"),
            [
                col("title", "", false, false),
                col("body", "", false, false),
            ]
        );
        let sql_of = |name: &str| -> String {
            let sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap();
            // Whitespace is not part of the contract.
            sql.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let fts_sql = sql_of("fts");
        assert!(fts_sql.contains("USING fts5"), "{fts_sql}");
        assert!(fts_sql.contains("content = 'fts_blocks'"), "{fts_sql}");
        assert!(fts_sql.contains("content_rowid = 'id'"), "{fts_sql}");
        assert!(
            fts_sql.contains("unicode61 remove_diacritics 2"),
            "{fts_sql}"
        );

        // Three triggers keep the word index in step with the text.
        let triggers: Vec<(String, String)> = conn
            .prepare(
                "SELECT name, tbl_name FROM sqlite_master WHERE type = 'trigger' ORDER BY name",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let trigger = |name: &str| (name.to_owned(), "fts_blocks".to_owned());
        assert_eq!(
            triggers,
            [
                trigger("fts_blocks_ad"),
                trigger("fts_blocks_ai"),
                trigger("fts_blocks_au")
            ]
        );
        assert!(sql_of("fts_blocks_ai").contains("AFTER INSERT ON fts_blocks"));
        assert!(sql_of("fts_blocks_ad").contains("AFTER DELETE ON fts_blocks"));
        assert!(sql_of("fts_blocks_ad").contains("'delete'"));
        assert!(sql_of("fts_blocks_au").contains("AFTER UPDATE ON fts_blocks"));

        let indexed = |name: &str, cols: &[&str]| {
            (
                name.to_owned(),
                cols.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            explicit_indexes(conn),
            [
                indexed("docs_kind", &["kind"]),
                indexed("docs_space", &["space_id"]),
                indexed("docs_space_kind", &["space_id", "kind"]),
                indexed("fts_blocks_page", &["page_id", "block_id"]),
                indexed("links_to", &["to_page"]),
                indexed("pages_index_space_trashed", &["space_id", "trashed_at"]),
            ],
            "pages_index_space is gone; docs_space_kind, fts_blocks_page and links_to are new"
        );
    }

    #[test]
    fn fts_blocks_keeps_blob_ids_and_nulls_and_the_triggers_index_its_text() {
        // ADR-0012 and the schema doc: ids are 16-byte blobs everywhere, and a title row
        // has no block id. The word index is filled by the triggers alone.
        let db = new_db();
        let conn = db.connection();
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body) \
             VALUES (?1, ?2, '', 'some text')",
            params![[7u8; 16], [9u8; 16]],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body) \
             VALUES (?1, NULL, 'a title', '')",
            [[7u8; 16]],
        )
        .unwrap();

        type Row = (String, Vec<u8>, String, Option<Vec<u8>>);
        let rows: Vec<Row> = conn
            .prepare(
                "SELECT typeof(page_id), page_id, typeof(block_id), block_id \
                   FROM fts_blocks ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                (
                    "blob".to_owned(),
                    vec![7; 16],
                    "blob".to_owned(),
                    Some(vec![9; 16])
                ),
                ("blob".to_owned(), vec![7; 16], "null".to_owned(), None),
            ]
        );

        // The words are findable by rowid back to the content row.
        let hits: Vec<(Vec<u8>, Option<Vec<u8>>)> = conn
            .prepare(
                "SELECT b.page_id, b.block_id FROM fts \
                   JOIN fts_blocks AS b ON b.id = fts.rowid \
                  WHERE fts MATCH 'text' ORDER BY b.id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(hits, [(vec![7; 16], Some(vec![9; 16]))]);
        let title_hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM fts WHERE fts MATCH 'title'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title_hits, 1);
    }

    #[test]
    fn foreign_keys_are_enabled() {
        let db = new_db();
        let on: i64 = db
            .connection()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(on, 1);
    }

    #[test]
    fn database_from_a_newer_version_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        let err = Db::new(OpenedDb {
            conn,
            encryption: Encryption::None,
        })
        .unwrap_err();
        assert!(
            matches!(&err, StorageError::Migration(m) if m.contains("newer")),
            "{err}"
        );
    }

    #[test]
    fn db_can_be_moved_to_another_thread() {
        // The shell runs the core on a blocking pool, so `Db` has to stay `Send`.
        let db = new_db();
        let version = std::thread::spawn(move || db.schema_version().unwrap())
            .join()
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn bundled_sqlite_is_new_enough_for_wal() {
        // ADR-0012: SQLite 3.51.3 or newer everywhere (older versions have a WAL-reset
        // bug). 3.51.3 is version number 3_051_003.
        assert!(
            rusqlite::version_number() >= 3_051_003,
            "bundled SQLite is {}",
            rusqlite::version()
        );
    }

    #[test]
    fn encryption_describes_itself() {
        let sqlcipher = Encryption::SqlCipher {
            version: "4.14.0 community".to_owned(),
        };
        assert!(sqlcipher.is_encrypted());
        assert_eq!(sqlcipher.describe(), "SQLCipher 4.14.0 community");
        let mc = Encryption::Sqlite3Mc {
            cipher: "chacha20".to_owned(),
        };
        assert!(mc.is_encrypted());
        assert_eq!(mc.describe(), "SQLite3MultipleCiphers chacha20");
    }

    #[test]
    fn meta_roundtrip_and_upsert() {
        let db = new_db();
        assert_eq!(db.meta_get("k").unwrap(), None);
        db.meta_set("k", "one").unwrap();
        assert_eq!(db.meta_get("k").unwrap().as_deref(), Some("one"));
        db.meta_set("k", "two").unwrap();
        assert_eq!(db.meta_get("k").unwrap().as_deref(), Some("two"));
        db.meta_set("other", "x").unwrap();
        assert_eq!(db.meta_get("k").unwrap().as_deref(), Some("two"));
        assert_eq!(db.meta_get("other").unwrap().as_deref(), Some("x"));
    }

    #[test]
    fn workspace_roundtrip_and_second_create_is_refused() {
        let db = new_db();
        assert_eq!(db.workspace_get().unwrap(), None);

        let first = workspace(1, "Home", 1_000);
        assert!(db.workspace_create(&first).unwrap(), "first create inserts");
        assert_eq!(db.workspace_get().unwrap(), Some(first.clone()));

        let second = workspace(2, "Other", 2_000);
        assert!(
            !db.workspace_create(&second).unwrap(),
            "second create is refused"
        );
        assert!(
            !db.workspace_create(&first).unwrap(),
            "the same row again is refused too"
        );
        assert_eq!(
            db.workspace_get().unwrap(),
            Some(first),
            "the original row is untouched"
        );
        let count: i64 = db
            .connection()
            .query_row("SELECT count(*) FROM workspace", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn workspace_id_is_stored_as_a_16_byte_blob() {
        let db = new_db();
        let row = workspace(3, "Home", 5);
        db.workspace_create(&row).unwrap();
        let (kind, len, bytes): (String, i64, Vec<u8>) = db
            .connection()
            .query_row(
                "SELECT typeof(id), length(id), id FROM workspace",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(kind, "blob");
        assert_eq!(len, 16);
        assert_eq!(bytes, row.id.as_bytes().to_vec());
    }

    #[test]
    fn workspace_with_a_malformed_id_is_invalid_data() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO workspace (id, name, created_at) VALUES (x'0102', 'Bad', 1)",
                [],
            )
            .unwrap();
        let err = db.workspace_get().unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    }

    #[test]
    fn settings_roundtrip_overwrite_and_missing() {
        let db = new_db();
        assert_eq!(db.setting_get("ui.theme").unwrap(), None);

        let theme = json!({ "mode": "dark", "accent": [1, 2, 3], "ratio": 1.5, "on": true });
        db.setting_set("ui.theme", &theme, 10).unwrap();
        assert_eq!(db.setting_get("ui.theme").unwrap(), Some(theme));

        db.setting_set("ui.theme", &json!("light"), 20).unwrap();
        assert_eq!(db.setting_get("ui.theme").unwrap(), Some(json!("light")));

        // Every JSON shape survives, including `null` (which is not "missing").
        for (key, value) in [
            ("app.null", Json::Null),
            ("app.bool", json!(false)),
            ("app.int", json!(-7)),
            ("app.array", json!([])),
            ("app.object", json!({})),
            ("app.text", json!("héllo \u{1F600} \"quoted\"")),
        ] {
            db.setting_set(key, &value, 30).unwrap();
            assert_eq!(db.setting_get(key).unwrap(), Some(value), "{key}");
        }
        assert_eq!(db.setting_get("ui.theme").unwrap(), Some(json!("light")));
    }

    #[test]
    fn setting_updated_at_follows_the_last_write() {
        let db = new_db();
        db.setting_set("ui.theme", &json!(1), 10).unwrap();
        db.setting_set("app.other", &json!(2), 15).unwrap();
        db.setting_set("ui.theme", &json!(3), 20).unwrap();
        let updated: i64 = db
            .connection()
            .query_row(
                "SELECT updated_at FROM settings WHERE key = 'ui.theme'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated, 20);
        let other: i64 = db
            .connection()
            .query_row(
                "SELECT updated_at FROM settings WHERE key = 'app.other'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(other, 15);
    }

    #[test]
    fn unparsable_stored_setting_is_invalid_data_and_does_not_echo_the_value() {
        let db = new_db();
        db.connection()
            .execute(
                "INSERT INTO settings (key, value, updated_at) VALUES ('ui.bad', 'SECRET-MARKER{', 1)",
                [],
            )
            .unwrap();
        let err = db.setting_get("ui.bad").unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        assert!(!err.to_string().contains("SECRET-MARKER"), "{err}");
    }

    #[test]
    fn device_is_absent_on_a_fresh_database() {
        assert_eq!(new_db().device_get().unwrap(), None);
    }

    #[test]
    fn device_init_get_and_rename_roundtrip() {
        let db = new_db();
        let row = device(1, "Windows device", 1_234);
        db.device_init(&row, "0.1.0").unwrap();
        assert_eq!(db.device_get().unwrap(), Some(row.clone()));

        // The documented `meta` keys hold the identity.
        assert_eq!(
            db.meta_get("device_id").unwrap(),
            Some(row.device_id.to_string())
        );
        assert_eq!(
            db.meta_get("device_name").unwrap().as_deref(),
            Some("Windows device")
        );
        assert_eq!(db.meta_get("created_at").unwrap().as_deref(), Some("1234"));
        assert_eq!(
            db.meta_get("core_version_at_creation").unwrap().as_deref(),
            Some("0.1.0")
        );

        db.device_rename("Ben's laptop").unwrap();
        let renamed = db.device_get().unwrap().unwrap();
        assert_eq!(renamed.name, "Ben's laptop");
        assert_eq!(renamed.device_id, row.device_id);
        assert_eq!(renamed.created_at, 1_234);
    }

    #[test]
    fn device_init_twice_fails_and_changes_nothing() {
        let db = new_db();
        let first = device(1, "First", 1);
        db.device_init(&first, "0.1.0").unwrap();

        let err = db
            .device_init(&device(2, "Second", 2), "9.9.9")
            .unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        assert_eq!(db.device_get().unwrap(), Some(first));
        assert_eq!(
            db.meta_get("core_version_at_creation").unwrap().as_deref(),
            Some("0.1.0")
        );
    }

    #[test]
    fn device_init_is_all_or_nothing() {
        let db = new_db();
        // Make the last of the four inserts fail.
        db.connection()
            .execute_batch(
                "CREATE TRIGGER fail_last BEFORE INSERT ON meta \
                 WHEN NEW.key = 'core_version_at_creation' \
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let err = db.device_init(&device(1, "Name", 1), "0.1.0").unwrap_err();
        assert!(matches!(err, StorageError::Sqlite(_)), "{err}");
        assert_eq!(db.meta_get("device_id").unwrap(), None, "rolled back");
        assert_eq!(db.device_get().unwrap(), None);

        db.connection()
            .execute_batch("DROP TRIGGER fail_last;")
            .unwrap();
        db.device_init(&device(1, "Name", 1), "0.1.0").unwrap();
        assert!(db.device_get().unwrap().is_some());
    }

    #[test]
    fn device_rename_needs_an_identity() {
        let db = new_db();
        let err = db.device_rename("Nobody").unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
        assert_eq!(
            db.meta_get("device_name").unwrap(),
            None,
            "nothing was written"
        );
    }

    #[test]
    fn malformed_device_meta_is_invalid_data() {
        let db = new_db();
        db.device_init(&device(1, "Name", 1), "0.1.0").unwrap();

        db.meta_set("created_at", "yesterday").unwrap();
        assert!(matches!(
            db.device_get().unwrap_err(),
            StorageError::InvalidData(_)
        ));
        db.meta_set("created_at", "1").unwrap();
        assert!(db.device_get().is_ok());

        db.meta_set("device_id", "not-a-uuid").unwrap();
        assert!(matches!(
            db.device_get().unwrap_err(),
            StorageError::InvalidData(_)
        ));

        let db = new_db();
        db.meta_set("device_id", &DeviceId::from_bytes([7; 16]).to_string())
            .unwrap();
        assert!(
            matches!(db.device_get().unwrap_err(), StorageError::InvalidData(_)),
            "an id without name and creation time is incomplete"
        );
    }

    #[test]
    fn schema_v4_matches_the_documented_tables() {
        // The frozen v4 schema: a fresh database migrated to exactly version 4.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 4).unwrap();
        let conn = &conn;

        // The tables of v3 (the word index brings its shadow tables) and the two new ones.
        assert_eq!(
            table_names(conn),
            [
                "device_log",
                "doc_snapshots",
                "doc_updates",
                "docs",
                "fts",
                "fts_blocks",
                "fts_config",
                "fts_data",
                "fts_docsize",
                "fts_idx",
                "keyring",
                "links",
                "meta",
                "pages_index",
                "settings",
                "workspace"
            ]
        );

        // The tables of v1 to v3 are untouched.
        for (table, width) in [
            ("meta", 2),
            ("workspace", 3),
            ("settings", 3),
            ("docs", 6),
            ("doc_snapshots", 4),
            ("doc_updates", 5),
            ("pages_index", 6),
            ("links", 4),
            ("fts_blocks", 5),
        ] {
            assert_eq!(columns(conn, table).len(), width, "{table}");
        }

        // The device log is keyed by its sequence number (the rowid itself).
        assert_eq!(
            columns(conn, "device_log"),
            [
                col("seq", "INTEGER", false, true),
                col("entry", "BLOB", true, false),
                col("hash", "BLOB", true, false),
            ]
        );
        // The keyring has one row per (space, subject).
        assert_eq!(
            columns(conn, "keyring"),
            [
                col("space_id", "BLOB", true, true),
                col("subject", "BLOB", true, true),
                col("record", "BLOB", true, false),
                col("tombstone", "INTEGER", true, false),
            ]
        );
        let default: String = conn
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('keyring') WHERE name = 'tombstone'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(default, "0", "a row is a record unless it says otherwise");
        assert!(foreign_keys(conn, "device_log").is_empty());
        assert!(foreign_keys(conn, "keyring").is_empty());

        // No explicit index is new. The keyring's primary key brings its own, over the
        // columns in that order; the device log needs none (its key is the rowid).
        assert_eq!(
            explicit_indexes(conn),
            [
                ("docs_kind".to_owned(), vec!["kind".to_owned()]),
                ("docs_space".to_owned(), vec!["space_id".to_owned()]),
                (
                    "docs_space_kind".to_owned(),
                    vec!["space_id".to_owned(), "kind".to_owned()]
                ),
                (
                    "fts_blocks_page".to_owned(),
                    vec!["page_id".to_owned(), "block_id".to_owned()]
                ),
                ("links_to".to_owned(), vec!["to_page".to_owned()]),
                (
                    "pages_index_space_trashed".to_owned(),
                    vec!["space_id".to_owned(), "trashed_at".to_owned()]
                ),
            ]
        );
        let (index, origin): (String, String) = conn
            .query_row(
                "SELECT name, origin FROM pragma_index_list('keyring')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(origin, "pk");
        let indexed: Vec<String> = conn
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
            .unwrap()
            .query_map([&index], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(indexed, ["space_id", "subject"]);
        let log_indexes: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_index_list('device_log')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(log_indexes, 0);
    }

    #[test]
    fn migrating_from_v3_adds_only_the_two_empty_key_tables() {
        // A v3 database with rows in every table that exists at v3 (a bare minimum of
        // them), migrated to v4 in place: the rows stay, the new tables start empty.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 3).unwrap();
        conn.execute_batch(
            "INSERT INTO meta VALUES ('device_id', 'x');
             INSERT INTO workspace VALUES (zeroblob(16), 'w', 1);
             INSERT INTO settings VALUES ('ui.a', '1', 2);
             INSERT INTO docs VALUES (zeroblob(16), 0, NULL, 3, 1, 1);
             INSERT INTO doc_snapshots VALUES (zeroblob(16), x'01', x'02', 4);
             INSERT INTO doc_updates VALUES (zeroblob(16), 1, x'03', 0, 5);
             INSERT INTO pages_index VALUES (zeroblob(16), zeroblob(16), NULL, 't', NULL, 6);
             INSERT INTO links VALUES (zeroblob(16), zeroblob(16), zeroblob(16), NULL);
             INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES (zeroblob(16), NULL, 'a', '');",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();

        for (table, rows) in [
            ("meta", 1),
            ("workspace", 1),
            ("settings", 1),
            ("docs", 1),
            ("doc_snapshots", 1),
            ("doc_updates", 1),
            ("pages_index", 1),
            ("links", 1),
            ("fts_blocks", 1),
            ("device_log", 0),
            ("keyring", 0),
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, rows, "{table}");
        }
    }

    #[test]
    fn schema_v5_matches_the_documented_tables() {
        // The frozen v5 schema: a fresh database migrated to exactly version 5.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 5).unwrap();
        let conn = &conn;

        // The tables of v4 and the two new ones.
        assert_eq!(
            table_names(conn),
            [
                "attachment_refs",
                "attachments_index",
                "device_log",
                "doc_snapshots",
                "doc_updates",
                "docs",
                "fts",
                "fts_blocks",
                "fts_config",
                "fts_data",
                "fts_docsize",
                "fts_idx",
                "keyring",
                "links",
                "meta",
                "pages_index",
                "settings",
                "workspace"
            ]
        );

        // The tables of v1 to v4 are untouched.
        for (table, width) in [
            ("meta", 2),
            ("workspace", 3),
            ("settings", 3),
            ("docs", 6),
            ("doc_snapshots", 4),
            ("doc_updates", 5),
            ("pages_index", 6),
            ("links", 4),
            ("fts_blocks", 5),
            ("device_log", 3),
            ("keyring", 4),
        ] {
            assert_eq!(columns(conn, table).len(), width, "{table}");
        }

        // The index mirrors a manifest (SQLite does not mark a BLOB primary key NOT NULL,
        // hence `false` for it).
        assert_eq!(
            columns(conn, "attachments_index"),
            [
                col("attachment_id", "BLOB", false, true),
                col("space_id", "BLOB", true, false),
                col("name", "TEXT", true, false),
                col("mime", "TEXT", true, false),
                col("size", "INTEGER", true, false),
                col("chunk_count", "INTEGER", true, false),
                col("created_at", "INTEGER", true, false),
                col("unreferenced_since", "INTEGER", false, false),
            ]
        );
        // One reference per (attachment, page, block): a composite primary key.
        assert_eq!(
            columns(conn, "attachment_refs"),
            [
                col("attachment_id", "BLOB", true, true),
                col("page_id", "BLOB", true, true),
                col("block_id", "BLOB", true, true),
            ]
        );
        // Both are derived data without foreign keys: a reference outlives a purged
        // attachment (the block then renders "deleted") and the core rebuilds each table
        // on its own.
        assert!(foreign_keys(conn, "attachments_index").is_empty());
        assert!(foreign_keys(conn, "attachment_refs").is_empty());

        // The two explicit indexes are new; the six of v4 are still there.
        let indexes = explicit_indexes(conn);
        assert_eq!(
            indexes
                .iter()
                .filter(|(name, _)| name.starts_with("attachment"))
                .cloned()
                .collect::<Vec<_>>(),
            [
                (
                    "attachment_refs_page".to_owned(),
                    vec!["page_id".to_owned()]
                ),
                (
                    "attachments_index_space".to_owned(),
                    vec!["space_id".to_owned(), "created_at".to_owned()]
                ),
            ]
        );
        assert_eq!(indexes.len(), 6 + 2, "the six of v4 and the two of v5");
        // The listing index is descending on the time: a space is read newest first.
        let descending: Vec<i64> = conn
            .prepare(
                "SELECT \"desc\" FROM pragma_index_xinfo('attachments_index_space') \
                 WHERE key = 1 ORDER BY seqno",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            descending,
            [0, 1],
            "space_id ascending, created_at descending"
        );

        // The primary keys bring their own indexes, over the columns in order.
        for (table, indexed) in [
            ("attachments_index", vec!["attachment_id"]),
            (
                "attachment_refs",
                vec!["attachment_id", "page_id", "block_id"],
            ),
        ] {
            let index: String = conn
                .query_row(
                    "SELECT name FROM pragma_index_list(?1) WHERE origin = 'pk'",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            let columns: Vec<String> = conn
                .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                .unwrap()
                .query_map([&index], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(columns, indexed, "{table}");
        }
    }

    #[test]
    fn migrating_from_v4_adds_only_the_two_empty_attachment_tables() {
        // A v4 database with rows in every table that exists at v4 (a bare minimum of
        // them), migrated to v5 in place: the rows stay, the new tables start empty.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 4).unwrap();
        conn.execute_batch(
            "INSERT INTO meta VALUES ('device_id', 'x');
             INSERT INTO workspace VALUES (zeroblob(16), 'w', 1);
             INSERT INTO settings VALUES ('ui.a', '1', 2);
             INSERT INTO docs VALUES (zeroblob(16), 0, NULL, 3, 1, 1);
             INSERT INTO doc_snapshots VALUES (zeroblob(16), x'01', x'02', 4);
             INSERT INTO doc_updates VALUES (zeroblob(16), 1, x'03', 0, 5);
             INSERT INTO pages_index VALUES (zeroblob(16), zeroblob(16), NULL, 't', NULL, 6);
             INSERT INTO links VALUES (zeroblob(16), zeroblob(16), zeroblob(16), NULL);
             INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES (zeroblob(16), NULL, 'a', '');
             INSERT INTO device_log VALUES (0, x'04', zeroblob(32));
             INSERT INTO keyring VALUES (zeroblob(16), x'05', x'06', 0);",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        for (table, rows) in [
            ("meta", 1),
            ("workspace", 1),
            ("settings", 1),
            ("docs", 1),
            ("doc_snapshots", 1),
            ("doc_updates", 1),
            ("pages_index", 1),
            ("links", 1),
            ("fts_blocks", 1),
            ("device_log", 1),
            ("keyring", 1),
            ("attachments_index", 0),
            ("attachment_refs", 0),
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, rows, "{table}");
        }
    }

    #[test]
    fn schema_v15_adds_the_semantic_index_and_the_mcp_tables() {
        // A v14 database with two indexed pages (and a property row) migrated to v15: the new
        // tables are there, the pages that have text rows start dirty, nothing else changes.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 14).unwrap();
        let tables_before = table_names(&conn);
        assert!(!tables_before.contains(&"ai_pages".to_owned()));
        conn.execute_batch(
            "INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES (x'01010101010101010101010101010101', NULL, 'one', '');
             INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES (x'01010101010101010101010101010101', zeroblob(16), '', 'text');
             INSERT INTO fts_blocks (page_id, block_id, title, body, kind) VALUES (x'02020202020202020202020202020202', zeroblob(16), '', 'tag', 1);",
        )
        .unwrap();
        migrations().to_version(&mut conn, 15).unwrap();
        let conn = &conn;

        let new_tables = [
            "ai_chunks",
            "ai_dirty",
            "ai_pages",
            "mcp_audit",
            "mcp_clients",
        ];
        let mut expected = tables_before.clone();
        expected.extend(new_tables.map(str::to_owned));
        expected.sort();
        assert_eq!(table_names(conn), expected, "exactly the five new tables");

        assert_eq!(
            columns(conn, "ai_pages"),
            [
                col("page_id", "BLOB", true, true),
                col("source_hash", "BLOB", true, false),
                col("model", "TEXT", true, false),
                col("chunks", "INTEGER", true, false),
                col("indexed_at", "INTEGER", true, false),
            ]
        );
        assert_eq!(
            columns(conn, "ai_chunks"),
            [
                col("page_id", "BLOB", true, true),
                col("seq", "INTEGER", true, true),
                col("block_ids", "BLOB", true, false),
                col("hash", "BLOB", true, false),
                col("text", "TEXT", true, false),
                col("vector", "BLOB", true, false),
            ]
        );
        assert_eq!(
            columns(conn, "ai_dirty"),
            [col("page_id", "BLOB", true, true)]
        );
        assert_eq!(
            columns(conn, "mcp_clients"),
            [
                col("id", "TEXT", false, true),
                col("name", "TEXT", true, false),
                col("token_hash", "BLOB", true, false),
                col("grants", "TEXT", true, false),
                col("created_at", "INTEGER", true, false),
                col("last_used_at", "INTEGER", false, false),
            ]
        );
        assert_eq!(
            columns(conn, "mcp_audit"),
            [
                col("id", "INTEGER", false, true),
                col("at", "INTEGER", true, false),
                col("client_id", "TEXT", true, false),
                col("client_name", "TEXT", true, false),
                col("tool", "TEXT", true, false),
                col("space_id", "BLOB", false, false),
                col("page_id", "BLOB", false, false),
                col("summary", "TEXT", true, false),
                col("outcome", "TEXT", true, false),
            ]
        );

        // The explicit indexes of the new tables (`token_hash` is UNIQUE: an automatic index).
        let indexes = explicit_indexes(conn);
        let has = |name: &str, cols: &[&str]| {
            let found = indexes.iter().find(|(n, _)| n == name);
            assert_eq!(
                found.map(|(_, c)| c.iter().map(String::as_str).collect::<Vec<_>>()),
                Some(cols.to_vec()),
                "{name} in {indexes:?}"
            );
        };
        has("ai_chunks_hash", &["hash"]);
        has("mcp_audit_at", &["at"]);
        has("mcp_audit_client", &["client_id", "id"]);

        // Three triggers keep `ai_dirty` in step with `fts_blocks`, next to the three of the word index.
        let triggers: Vec<(String, String)> = conn
            .prepare(
                "SELECT name, tbl_name FROM sqlite_master WHERE type = 'trigger' ORDER BY name",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let trigger = |name: &str| (name.to_owned(), "fts_blocks".to_owned());
        assert_eq!(
            triggers,
            [
                trigger("ai_dirty_on_fts_delete"),
                trigger("ai_dirty_on_fts_insert"),
                trigger("ai_dirty_on_fts_update"),
                trigger("fts_blocks_ad"),
                trigger("fts_blocks_ai"),
                trigger("fts_blocks_au"),
            ]
        );
        let sql_of = |name: &str| -> String {
            conn.query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(sql_of("ai_dirty_on_fts_insert").contains("AFTER INSERT ON fts_blocks"));
        assert!(sql_of("ai_dirty_on_fts_update").contains("AFTER UPDATE ON fts_blocks"));
        assert!(sql_of("ai_dirty_on_fts_delete").contains("AFTER DELETE ON fts_blocks"));

        // The pages that had text rows start dirty (the property-only page too: it has a row),
        // and nothing else was written.
        let dirty = |conn: &Connection| -> Vec<Vec<u8>> {
            conn.prepare("SELECT page_id FROM ai_dirty ORDER BY page_id")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(dirty(conn), [vec![1u8; 16], vec![2u8; 16]]);
        for table in ["ai_pages", "ai_chunks", "mcp_clients", "mcp_audit"] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }

        // The triggers: an insert, an update and a delete of a text row each mark its page.
        let clear = || conn.execute("DELETE FROM ai_dirty", []).unwrap();
        clear();
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES (x'03030303030303030303030303030303', NULL, 't', '')",
            [],
        )
        .unwrap();
        assert_eq!(dirty(conn), [vec![3u8; 16]], "insert");
        clear();
        conn.execute(
            "UPDATE fts_blocks SET title = 'u' WHERE page_id = x'03030303030303030303030303030303'",
            [],
        )
        .unwrap();
        assert_eq!(dirty(conn), [vec![3u8; 16]], "update");
        clear();
        conn.execute(
            "DELETE FROM fts_blocks WHERE page_id = x'03030303030303030303030303030303'",
            [],
        )
        .unwrap();
        assert_eq!(dirty(conn), [vec![3u8; 16]], "delete");
    }

    #[test]
    fn schema_v16_keys_the_icon_table_by_page_and_attachment_and_keeps_every_row() {
        // A v15 database with three icon rows migrated to v16: the rows are all there, a page can
        // now hold two attachments (its icon and its cover), and the indexes are as before.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 15).unwrap();
        assert_eq!(
            columns(&conn, "icon_refs"),
            [
                col("page_id", "BLOB", false, true),
                col("space_id", "BLOB", true, false),
                col("attachment_id", "BLOB", true, false),
            ],
            "v15 keys the table by the page alone"
        );
        let tables_before = table_names(&conn);
        conn.execute_batch(
            "INSERT INTO icon_refs VALUES (x'01010101010101010101010101010101', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', x'11111111111111111111111111111111');
             INSERT INTO icon_refs VALUES (x'02020202020202020202020202020202', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', x'22222222222222222222222222222222');
             INSERT INTO icon_refs VALUES (x'03030303030303030303030303030303', x'0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b', x'11111111111111111111111111111111');",
        )
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO icon_refs VALUES (x'01010101010101010101010101010101', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', x'33333333333333333333333333333333')",
                [],
            )
            .is_err(),
            "a page holds one row in v15"
        );
        migrations().to_version(&mut conn, 16).unwrap();
        let conn = &conn;
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 16);

        assert_eq!(table_names(conn), tables_before, "no table came or went");
        assert_eq!(
            columns(conn, "icon_refs"),
            [
                col("page_id", "BLOB", true, true),
                col("space_id", "BLOB", true, false),
                col("attachment_id", "BLOB", true, true),
            ]
        );
        let rows = || -> Vec<(u8, u8, u8)> {
            conn.prepare(
                "SELECT page_id, space_id, attachment_id FROM icon_refs ORDER BY page_id, attachment_id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?[0],
                    r.get::<_, Vec<u8>>(1)?[0],
                    r.get::<_, Vec<u8>>(2)?[0],
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
        };
        assert_eq!(rows(), [(1, 0x0a, 0x11), (2, 0x0a, 0x22), (3, 0x0b, 0x11)]);

        // The indexes the reference count and the space rewrite read are there again.
        let indexes = explicit_indexes(conn);
        for (name, wanted) in [
            ("icon_refs_by_attachment", "attachment_id"),
            ("icon_refs_by_space", "space_id"),
        ] {
            let found = indexes.iter().find(|(n, _)| n == name);
            assert_eq!(
                found.map(|(_, c)| c.iter().map(String::as_str).collect::<Vec<_>>()),
                Some(vec![wanted]),
                "{name} in {indexes:?}"
            );
        }
        let plan = |sql: &str| -> String {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            stmt.query_map([[1u8; 16]], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join(" | ")
        };
        assert!(
            plan("SELECT attachment_id FROM icon_refs WHERE page_id = ?1")
                .contains("sqlite_autoindex_icon_refs_1 (page_id=?)")
        );

        // A page now holds two attachments; the same pair twice is refused.
        conn.execute(
            "INSERT INTO icon_refs VALUES (x'01010101010101010101010101010101', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', x'33333333333333333333333333333333')",
            [],
        )
        .unwrap();
        assert_eq!(rows().iter().filter(|(page, _, _)| *page == 1).count(), 2);
        assert!(
            conn.execute(
                "INSERT INTO icon_refs VALUES (x'01010101010101010101010101010101', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', x'33333333333333333333333333333333')",
                [],
            )
            .is_err()
        );

        // Migrating again is a no-op.
        let mut again = Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut again).unwrap();
        migrations().to_latest(&mut again).unwrap();
    }

    #[test]
    fn schema_v20_adds_the_in_template_column_to_the_page_index_and_keeps_every_row() {
        // A v19 database with three indexed pages migrated to v20: the rows are all there, each one
        // is not in a template, and a new row may say it is.
        let mut conn = Connection::open_in_memory().unwrap();
        migrations().to_version(&mut conn, 19).unwrap();
        assert_eq!(columns(&conn, "pages_index").len(), 6);
        let tables_before = table_names(&conn);
        conn.execute_batch(
            "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (x'01010101010101010101010101010101', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', NULL, 'one', NULL, 10);
             INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (x'02020202020202020202020202020202', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', x'01010101010101010101010101010101', 'two', NULL, 11);
             INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (x'03030303030303030303030303030303', x'0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b', NULL, 'three', 99, 12);",
        )
        .unwrap();
        migrations().to_version(&mut conn, 20).unwrap();
        let conn = &conn;
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 20);
        assert_eq!(table_names(conn), tables_before, "no table came or went");
        assert_eq!(
            columns(conn, "pages_index"),
            [
                col("page_id", "BLOB", false, true),
                col("space_id", "BLOB", true, false),
                col("parent_id", "BLOB", false, false),
                col("title", "TEXT", true, false),
                col("trashed_at", "INTEGER", false, false),
                col("updated_at", "INTEGER", true, false),
                col("in_template", "INTEGER", true, false),
            ]
        );
        let rows = || -> Vec<(u8, String, i64)> {
            conn.prepare("SELECT page_id, title, in_template FROM pages_index ORDER BY page_id")
                .unwrap()
                .query_map([], |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?[0],
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            rows(),
            [
                (1, "one".to_owned(), 0),
                (2, "two".to_owned(), 0),
                (3, "three".to_owned(), 0)
            ],
            "every existing page is outside a template"
        );
        // The other columns are as they were, and the indexes are untouched.
        let (parent, trashed, updated): (Vec<u8>, Option<i64>, i64) = conn
            .query_row(
                "SELECT parent_id, trashed_at, updated_at FROM pages_index WHERE title = 'two'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((parent, trashed, updated), (vec![1u8; 16], None, 11));
        assert!(
            explicit_indexes(conn)
                .iter()
                .any(|(name, _)| name == "pages_index_space_trashed")
        );
        // The column takes 0 and 1 and is never null; a row inserted without it is not in a template.
        conn.execute(
            "UPDATE pages_index SET in_template = 1 WHERE title = 'one'",
            [],
        )
        .unwrap();
        assert!(
            conn.execute("UPDATE pages_index SET in_template = NULL", [])
                .is_err()
        );
        conn.execute(
            "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, updated_at) VALUES (x'04040404040404040404040404040404', x'0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a', NULL, 'four', NULL, 13)",
            [],
        )
        .unwrap();
        assert_eq!(
            rows().iter().map(|(_, _, flag)| *flag).collect::<Vec<_>>(),
            [1, 0, 0, 0]
        );
    }

    #[test]
    fn schema_v21_adds_the_page_history_journal_and_touches_nothing_else() {
        // A v20 database with a document migrated to v21: the journal table is there, empty, with its
        // columns, and no other table came or went; the rows of the journal follow their document.
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        migrations().to_version(&mut conn, 20).unwrap();
        let tables_before = table_names(&conn);
        assert!(!tables_before.contains(&"page_history_journal".to_owned()));
        conn.execute_batch(
            "INSERT INTO docs (doc_id, kind, space_id, created_at, format_version, min_reader_version) VALUES (x'01010101010101010101010101010101', 1, NULL, 5, 1, 1);",
        )
        .unwrap();
        migrations().to_version(&mut conn, 21).unwrap();
        let conn = &conn;
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 21);
        let mut expected = tables_before;
        expected.push("page_history_journal".to_owned());
        expected.sort();
        assert_eq!(table_names(conn), expected);
        assert_eq!(
            columns(conn, "page_history_journal"),
            [
                col("doc_id", "BLOB", true, true),
                col("seq", "INTEGER", true, true),
                col("kind", "TEXT", true, false),
                col("started_at", "INTEGER", true, false),
                col("ended_at", "INTEGER", true, false),
                col("base_version", "TEXT", true, false),
                col("version", "TEXT", true, false),
                col("device_id", "BLOB", false, false),
                col("account_id", "BLOB", false, false),
                col("this_device", "INTEGER", true, false),
                col("title", "TEXT", false, false),
            ]
        );
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("page_history_journal"), 0);
        assert_eq!(count("docs"), 1, "the document row is as it was");
        conn.execute(
            "INSERT INTO page_history_journal VALUES (x'01010101010101010101010101010101', 1, 'edit', 1, 2, '', '1@0', NULL, NULL, 1, NULL)",
            [],
        )
        .unwrap();
        // The primary key is the document and the number.
        assert!(
            conn.execute(
                "INSERT INTO page_history_journal VALUES (x'01010101010101010101010101010101', 1, 'edit', 1, 2, '', '1@0', NULL, NULL, 1, NULL)",
                [],
            )
            .is_err()
        );
        conn.execute("DELETE FROM docs", []).unwrap();
        assert_eq!(
            count("page_history_journal"),
            0,
            "the rows follow the document"
        );
        // Migrating again is a no-op.
        let mut again = Connection::open_in_memory().unwrap();
        migrations().to_latest(&mut again).unwrap();
        migrations().to_latest(&mut again).unwrap();
    }

    #[test]
    fn raw_key_pragma_builds_the_statement_that_keys_and_rekeys() {
        let ark = kb_keys::Ark::from_secret(kb_crypto::Secret32::generate(&SeededRng::new(3)));
        let key = kb_keys::derive_db_key(&ark, &kb_types::InstallId::from_random_bytes([7; 16]));
        let hex: String = key
            .secret()
            .expose()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex.len(), 64);
        for name in ["key", "rekey"] {
            let sql = raw_key_pragma(name, &key);
            assert_eq!(sql.as_str(), format!("PRAGMA {name} = \"x'{hex}'\";"));
        }
        // Different keys, different statements.
        let other = kb_keys::derive_db_key(
            &kb_keys::Ark::from_secret(kb_crypto::Secret32::generate(&SeededRng::new(4))),
            &kb_types::InstallId::from_random_bytes([7; 16]),
        );
        assert_ne!(
            raw_key_pragma("key", &key).as_str(),
            raw_key_pragma("key", &other).as_str()
        );
    }
}
