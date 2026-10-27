//! File-backed behaviour of kb-storage: migrating real files, the checked-in v1 to v10
//! fixtures, documents, the text index, the device log, the keyring, the attachment index,
//! the sync state and the chunk transfer state in real files, SQLCipher encryption at rest
//! and rekeying a real SQLCipher file.
//! Native only: the web build has no files.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::time::Duration;

use kb_crypto::Secret32;
use kb_keys::{Ark, DbKey, derive_db_key};
use kb_storage::rusqlite::types::Value;
use kb_storage::rusqlite::{Connection, ErrorCode};
use kb_storage::{
    AiIndexQueries, AttachmentQueries, AttachmentRow, ChunkSyncQueries, ChunkSyncRow,
    CommentQueries, CommentRow, DateMentionQueries, DateMentionRow, Db, DeviceLogRow, DeviceRow,
    DocKindCode, DocQueries, DocRow, DueReminderRow, Encryption, EntryMarkRow, FeedQueries,
    ForeignLogRow, FtsBlock, IconRefQueries, JournalKind, KeyringRow, LinkRow, LinkSource,
    MEMBERSHIP_PENDING_CURSOR, McpQueries, MembershipEntryRow, NewJournalRow, NotificationQueries,
    NotificationRow, OpenedDb, PageFirstImageQueries, PageHistoryQueries, PageIndexRow,
    PagePropertyQueries, PagePropertyRow, PersonMentionQueries, PersonMentionRow, PropertyMatch,
    RejectReason, RejectedRow, SCHEMA_VERSION, SearchParams, SecurityQueries, SettingRow,
    StorageError, SyncDocState, SyncFeedRow, SyncHeadRow, SyncPendingRow, SyncQueries,
    TextIndexQueries, UNRESOLVED_PERSONAL_FEED, UpdateOrigin, UpdateStats, WorkspaceRow, sqlcipher,
};
use kb_types::sync::{FeedKind, KEYRING_DOC_ID};
use kb_types::testing::SeededRng;
use kb_types::{
    AccountId, AttachmentId, BlockId, ChunkId, CommentId, CommentRecordId, DeviceId, DocId, FeedId,
    InstallId, MemberRole, NotificationKind, OptionId, PageId, PropertyId, PropertyKind, SpaceId,
    ThreadId, WorkspaceId,
};
use serde_json::json;

/// The checked-in v1 database (plain SQLite), see `examples/make_fixture_db.rs`.
const FIXTURE_V1: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v1.db");

/// The checked-in v2 database (plain SQLite), see `examples/make_fixture_db.rs`.
const FIXTURE_V2: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v2.db");

/// The checked-in v3 database (plain SQLite), see `examples/make_fixture_db.rs`.
const FIXTURE_V3: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v3.db");

/// The checked-in v4 database (plain SQLite), see `examples/make_fixture_db.rs`.
const FIXTURE_V4: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v4.db");

/// The checked-in v5 database (plain SQLite), see `examples/make_fixture_db.rs`.
const FIXTURE_V5: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v5.db");

/// The checked-in v6 database (plain SQLite), see `examples/make_fixture_db.rs`.
const FIXTURE_V6: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v6.db");
/// A schema v7 file (`make_fixture_db -- v7`): the v6 content under the pending table keyed
/// by cursor and document.
const FIXTURE_V7: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v7.db");
/// A schema v8 file (`make_fixture_db -- v8`): the v7 content plus five `chunk_sync` rows.
const FIXTURE_V8: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v8.db");

/// File name used by the path tests.
const DB_FILE_NAME: &str = "workspace.db";

/// Text that must never appear in the bytes of an encrypted database file.
const MARKER: &str = "plaintext-marker-3f9a7c1e";

/// A database key derived the way the app derives it (a root key, then `derive_db_key`);
/// different seeds, different keys. The root key comes straight from the seeded secret, so
/// these tests do not depend on how kb-keys creates and stores it.
fn db_key(seed: u64) -> DbKey {
    let install_id = InstallId::from_random_bytes([7; 16]);
    let ark = Ark::from_secret(Secret32::generate(&SeededRng::new(seed)));
    derive_db_key(&ark, &install_id)
}

fn key_hex(key: &DbKey) -> String {
    key.secret()
        .expose()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn open_encrypted(path: &Path, key: &DbKey) -> Db {
    Db::new(sqlcipher::open_file(path, key).unwrap()).unwrap()
}

fn open_plain(path: &Path) -> Db {
    let conn = Connection::open(path).unwrap();
    Db::new(OpenedDb {
        conn,
        encryption: Encryption::None,
    })
    .unwrap()
}

/// Writes a recognisable workspace, setting and device identity.
fn fill(db: &Db) {
    let rng = SeededRng::new(11);
    db.workspace_create(&WorkspaceRow {
        id: WorkspaceId::generate(&rng),
        name: MARKER.to_owned(),
        created_at: 1_000,
    })
    .unwrap();
    db.setting_set("ui.theme", &json!({ "note": MARKER }), 2_000)
        .unwrap();
    db.device_init(
        &DeviceRow {
            device_id: DeviceId::generate(&rng),
            name: "Test device".to_owned(),
            created_at: 3_000,
        },
        "0.1.0",
    )
    .unwrap();
}

fn assert_filled(db: &Db) {
    let workspace = db.workspace_get().unwrap().unwrap();
    assert_eq!(workspace.name, MARKER);
    assert_eq!(workspace.created_at, 1_000);
    assert_eq!(
        db.setting_get("ui.theme").unwrap(),
        Some(json!({ "note": MARKER }))
    );
    let device = db.device_get().unwrap().unwrap();
    assert_eq!(device.name, "Test device");
    assert_eq!(device.created_at, 3_000);
}

/// Value of `PRAGMA <name>` as text, whatever type SQLite or SQLCipher answers with.
fn pragma(conn: &Connection, name: &str) -> String {
    let value: Value = conn
        .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
        .unwrap();
    match value {
        Value::Text(text) => text,
        Value::Integer(int) => int.to_string(),
        other => format!("{other:?}"),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Every file in `dir` (database, `-wal`, `-shm`), so the scan covers the WAL too.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .collect()
}

// --- migrations ---------------------------------------------------------------------

#[test]
fn fresh_plain_file_migrates_to_the_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.db");
    let db = open_plain(&path);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string()
    );
    drop(db);

    // Reopening an up-to-date file keeps the version.
    assert_eq!(open_plain(&path).schema_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn migrating_the_v1_fixture_keeps_the_data() {
    let original = fs::read(FIXTURE_V1).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );

    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("v1.db");
    fs::write(&copy, &original).unwrap();

    let fixture_version = pragma(&Connection::open(&copy).unwrap(), "user_version");
    assert_eq!(fixture_version, "1", "the fixture is a schema v1 database");

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);

    let device = db.device_get().unwrap().unwrap();
    assert_eq!(
        device.device_id,
        "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d".parse().unwrap()
    );
    assert_eq!(device.name, "Fixture laptop");
    assert_eq!(device.created_at, 1_760_000_000_000);
    assert_eq!(
        db.meta_get("core_version_at_creation").unwrap().as_deref(),
        Some("0.1.0")
    );

    let workspace = db.workspace_get().unwrap().unwrap();
    assert_eq!(
        workspace.id,
        "7a1d9c3e-5b2f-4e8a-9c41-d3f0a6b7e812".parse().unwrap()
    );
    assert_eq!(workspace.name, "Fixture workspace");
    assert_eq!(workspace.created_at, 1_760_000_001_000);

    assert_eq!(
        db.setting_get("ui.theme").unwrap(),
        Some(json!({ "mode": "dark", "accent": "violet" }))
    );
    assert_eq!(db.setting_get("app.locale").unwrap(), Some(json!("en")));
    assert_eq!(db.setting_get("ui.missing").unwrap(), None);
    drop(db);

    if SCHEMA_VERSION == 1 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn a_file_that_is_not_a_database_is_refused_before_migrating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("junk.db");
    let junk = vec![0x5A; 8192];
    fs::write(&path, &junk).unwrap();

    let err = Db::new(OpenedDb {
        conn: Connection::open(&path).unwrap(),
        encryption: Encryption::None,
    })
    .unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    assert_eq!(fs::read(&path).unwrap(), junk, "the file is left alone");
}

// --- schema v2: the document tables -------------------------------------------------

/// Every row of the three v1 tables as text, in a stable order.
fn v1_rows(conn: &Connection) -> Vec<String> {
    [
        "SELECT key || '=' || value FROM meta ORDER BY key",
        "SELECT hex(id) || '|' || name || '|' || created_at FROM workspace ORDER BY id",
        "SELECT key || '=' || value || '@' || updated_at FROM settings ORDER BY key",
    ]
    .into_iter()
    .flat_map(|sql| {
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    })
    .collect()
}

fn count_rows(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

/// A scratch copy of a fixture; keep the returned directory alive while using the copy.
fn copy_of(fixture: &str, name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join(name);
    fs::copy(fixture, &copy).unwrap();
    (dir, copy)
}

#[test]
fn migrating_the_v1_fixture_keeps_the_data_and_adds_the_empty_document_and_text_tables() {
    let (_dir, copy) = copy_of(FIXTURE_V1, "v1.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "1", "a schema v1 file");
        v1_rows(&conn)
    };
    assert_eq!(before.len(), 7, "4 meta rows, the workspace and 2 settings");

    let mut db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    for table in [
        "docs",
        "doc_snapshots",
        "doc_updates",
        "pages_index",
        "links",
        "fts_blocks",
    ] {
        assert_eq!(
            count_rows(db.connection(), table),
            0,
            "{table} starts empty"
        );
    }

    // The migrated file takes documents like a fresh one.
    let doc_id = DocId::from_bytes([1; 16]);
    let seq = db
        .transaction(|tx| {
            tx.doc_insert(&DocRow {
                doc_id,
                kind: DocKindCode::Account,
                space_id: None,
                created_at: 5,
                format_version: 1,
                min_reader_version: 1,
            })?;
            tx.update_append(doc_id, b"update", UpdateOrigin::Local, 6)
        })
        .unwrap();
    assert_eq!(seq, 1);
    drop(db);

    let conn = Connection::open(&copy).unwrap();
    assert_eq!(
        pragma(&conn, "user_version"),
        SCHEMA_VERSION.to_string(),
        "written to the file"
    );
    assert_eq!(
        v1_rows(&conn),
        before,
        "meta, workspace and settings are untouched"
    );
    assert_eq!(count_rows(&conn, "docs"), 1);
    assert_eq!(count_rows(&conn, "doc_updates"), 1);
}

#[test]
fn opening_the_v2_fixture_migrates_it_to_the_schema_version() {
    let original = fs::read(FIXTURE_V2).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V2, "v2.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "2",
        "the fixture is a schema v2 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        SCHEMA_VERSION.to_string(),
        "written to the file"
    );
    if SCHEMA_VERSION == 2 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

// The ids of `fixtures/db/v2.db`, as written by `examples/make_fixture_db.rs`.
const V2_PERSONAL: &str = "5ba0ce00-0000-4000-8000-000000000001";
const V2_WORK: &str = "5ba0ce00-0000-4000-8000-000000000002";
const V2_DOC_ACCOUNT: &str = "d0c00000-0000-4000-8000-00000000000a";
const V2_DOC_PERSONAL: &str = "d0c00000-0000-4000-8000-000000000051";
const V2_DOC_WORK: &str = "d0c00000-0000-4000-8000-000000000052";
const V2_PAGE_HOME: &str = "9a6e0000-0000-4000-8000-000000000001";
const V2_PAGE_NOTES: &str = "9a6e0000-0000-4000-8000-000000000002";
const V2_PAGE_OLD: &str = "9a6e0000-0000-4000-8000-000000000003";
const V2_PAGE_OLD_CHILD: &str = "9a6e0000-0000-4000-8000-000000000004";
const V2_PAGE_ROADMAP: &str = "9a6e0000-0000-4000-8000-000000000005";

/// An update blob as the generator writes it: `[0xAA, last byte of the doc id, seq, 0, 0xFF]`.
fn v2_update(doc_last_byte: u8, seq: u8) -> Vec<u8> {
    vec![0xAA, doc_last_byte, seq, 0x00, 0xFF]
}

#[test]
fn the_v2_fixture_reads_back_through_the_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V2, "v2.db");
    let db = open_plain(&copy);
    let (personal, work) = (V2_PERSONAL.parse().unwrap(), V2_WORK.parse().unwrap());
    let account_doc: DocId = V2_DOC_ACCOUNT.parse().unwrap();
    let personal_doc: DocId = V2_DOC_PERSONAL.parse().unwrap();
    let work_doc: DocId = V2_DOC_WORK.parse().unwrap();

    let doc_row = |doc_id, kind, space_id, created_at| DocRow {
        doc_id,
        kind,
        space_id,
        created_at,
        format_version: 1,
        min_reader_version: 1,
    };
    let personal_row = doc_row(
        personal_doc,
        DocKindCode::Structure,
        Some(personal),
        1_760_000_020_000,
    );
    let work_row = doc_row(
        work_doc,
        DocKindCode::Structure,
        Some(work),
        1_760_000_030_000,
    );
    let account_row = doc_row(account_doc, DocKindCode::Account, None, 1_760_000_010_000);

    // Listed oldest first, though the rows were written in another order.
    assert_eq!(
        db.docs_by_kind(DocKindCode::Structure).unwrap(),
        [personal_row.clone(), work_row.clone()]
    );
    assert_eq!(
        db.docs_by_kind(DocKindCode::Account).unwrap(),
        std::slice::from_ref(&account_row)
    );
    assert!(db.docs_by_kind(DocKindCode::Page).unwrap().is_empty());
    assert_eq!(
        db.doc_for_space(personal, DocKindCode::Structure).unwrap(),
        Some(personal_row)
    );
    assert_eq!(
        db.doc_for_space(work, DocKindCode::Structure).unwrap(),
        Some(work_row)
    );
    assert_eq!(db.doc_get(account_doc).unwrap(), Some(account_row));

    // The snapshot (blob first, version second), and the log that starts after it.
    assert_eq!(
        db.snapshot_get(personal_doc).unwrap(),
        Some((
            b"KBD1\x01\x01\x01\x00snapshot\xff\x00\x80 personal structure".to_vec(),
            vec![0x01, 0x02, 0x00, 0xff, 0x7f]
        ))
    );
    assert_eq!(db.snapshot_get(work_doc).unwrap(), None);
    assert_eq!(db.snapshot_get(account_doc).unwrap(), None);
    assert_eq!(
        db.updates_after_snapshot(personal_doc).unwrap(),
        [v2_update(0x51, 3), v2_update(0x51, 4), v2_update(0x51, 5)]
    );
    assert_eq!(
        db.updates_after_snapshot(account_doc).unwrap(),
        [v2_update(0x0a, 1), v2_update(0x0a, 2)]
    );
    assert_eq!(
        db.updates_after_snapshot(work_doc).unwrap(),
        [v2_update(0x52, 1)]
    );
    let stats = |count, bytes, last_seq| UpdateStats {
        count,
        bytes,
        last_seq: Some(last_seq),
    };
    assert_eq!(db.update_stats(personal_doc).unwrap(), stats(3, 15, 5));
    assert_eq!(db.update_stats(account_doc).unwrap(), stats(2, 10, 2));
    assert_eq!(db.update_stats(work_doc).unwrap(), stats(1, 5, 1));

    // The numbering carries on after the highest row that is left.
    let next = db
        .update_append(personal_doc, b"next", UpdateOrigin::Local, 1)
        .unwrap();
    assert_eq!(next, 6);

    // The page index: Home > Notes are live, "Old ideas" and its child are trashed.
    let page_row = |id: &str,
                    space_id: SpaceId,
                    parent: Option<&str>,
                    title: &str,
                    trashed_at: Option<i64>,
                    updated_at: i64| PageIndexRow {
        page_id: id.parse().unwrap(),
        space_id,
        parent_id: parent.map(|p| p.parse().unwrap()),
        title: title.to_owned(),
        trashed_at,
        updated_at,
        in_template: false,
    };
    let home = page_row(
        V2_PAGE_HOME,
        personal,
        None,
        "Home",
        None,
        1_760_000_021_000,
    );
    let notes = page_row(
        V2_PAGE_NOTES,
        personal,
        Some(V2_PAGE_HOME),
        "Notes",
        None,
        1_760_000_022_000,
    );
    let old = page_row(
        V2_PAGE_OLD,
        personal,
        None,
        "Old ideas",
        Some(1_760_000_060_000),
        1_760_000_060_000,
    );
    let archived = page_row(
        V2_PAGE_OLD_CHILD,
        personal,
        Some(V2_PAGE_OLD),
        "Éléments archivés",
        Some(1_760_000_060_000),
        1_760_000_023_000,
    );
    let roadmap = page_row(
        V2_PAGE_ROADMAP,
        work,
        None,
        "Roadmap",
        None,
        1_760_000_031_000,
    );
    assert_eq!(
        db.pages_index_by_space(personal).unwrap(),
        [home.clone(), notes, old, archived]
    );
    assert_eq!(
        db.pages_index_by_space(work).unwrap(),
        std::slice::from_ref(&roadmap)
    );
    assert_eq!(
        db.pages_index_get(V2_PAGE_HOME.parse().unwrap()).unwrap(),
        Some(home)
    );
    assert_eq!(
        db.pages_index_get(V2_PAGE_ROADMAP.parse().unwrap())
            .unwrap(),
        Some(roadmap)
    );
    assert_eq!(db.pages_index_live_count(personal).unwrap(), 2);
    assert_eq!(db.pages_index_live_count(work).unwrap(), 1);

    // The cascade works on a file made by the migration SQL: a deleted document takes
    // its snapshot and updates with it and nobody else's.
    db.doc_delete(personal_doc).unwrap();
    assert_eq!(db.doc_get(personal_doc).unwrap(), None);
    assert_eq!(db.snapshot_get(personal_doc).unwrap(), None);
    assert!(db.updates_after_snapshot(personal_doc).unwrap().is_empty());
    assert_eq!(db.update_stats(account_doc).unwrap(), stats(2, 10, 2));
    assert_eq!(db.update_stats(work_doc).unwrap(), stats(1, 5, 1));
}

// --- schema v3: the text and link indexes -------------------------------------------

/// The names of the indexes of a table (the automatic ones included).
fn index_names(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare("SELECT name FROM pragma_index_list(?1) ORDER BY name")
        .unwrap()
        .query_map([table], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The tables schema v17 rebuilt with a `feed` column (first), see [`dump`].
const V17_FEED_TABLES: [&str; 4] = ["sync_doc_state", "sync_heads", "sync_pending", "sync_seen"];

/// The columns schema v18 added to tables that older fixtures have (`page_properties.account_id`
/// for the Person kind, `date_mentions.reminder_for`), left out by [`dump`] like the `feed` column of
/// v17, so a file of an older version and the same file migrated dump alike.
const V18_NEW_COLUMNS: [(&str, &str); 2] = [
    ("page_properties", "account_id"),
    ("date_mentions", "reminder_for"),
];

/// The column schema v20 added to a table that older fixtures have (`pages_index.in_template`, ADR-0056), left
/// out by [`dump`] like the columns of v17 and v18.
const V20_NEW_COLUMNS: [(&str, &str); 1] = [("pages_index", "in_template")];

/// Every row of a table as a line of text (columns joined by `|`, blobs in hex), sorted,
/// so that two dumps are equal exactly when the rows are. The `feed` column that schema v17 gave
/// the sync tables ([`V17_FEED_TABLES`]) is left out, so a file of an older version and the same
/// file migrated dump alike; [`dump_raw`] has every column.
fn dump(conn: &Connection, table: &str) -> Vec<String> {
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap()
        .query_map([table], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let skip_feed = V17_FEED_TABLES.contains(&table);
    let wanted: Vec<&str> = columns
        .iter()
        .map(String::as_str)
        .filter(|name| !(skip_feed && *name == "feed"))
        .filter(|name| !V18_NEW_COLUMNS.contains(&(table, *name)))
        .filter(|name| !V20_NEW_COLUMNS.contains(&(table, *name)))
        .collect();
    dump_columns(conn, table, &wanted.join(", "))
}

/// [`dump`] with every column, the `feed` of the sync tables included.
fn dump_raw(conn: &Connection, table: &str) -> Vec<String> {
    dump_columns(conn, table, "*")
}

fn dump_columns(conn: &Connection, table: &str, columns_sql: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {columns_sql} FROM {table}"))
        .unwrap();
    let columns = stmt.column_count();
    let mut rows: Vec<String> = stmt
        .query_map([], |row| {
            let mut cells = Vec::new();
            for column in 0..columns {
                cells.push(match row.get::<_, Value>(column)? {
                    Value::Null => "NULL".to_owned(),
                    Value::Integer(n) => n.to_string(),
                    Value::Real(x) => x.to_string(),
                    Value::Text(text) => text,
                    Value::Blob(bytes) => bytes.iter().map(|b| format!("{b:02x}")).collect(),
                });
            }
            Ok(cells.join("|"))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.sort();
    rows
}

/// What `PRAGMA compile_options` lists for this SQLite build.
fn compile_options(conn: &Connection) -> Vec<String> {
    conn.prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn page_id(text: &str) -> PageId {
    text.parse().unwrap()
}

fn block_id(text: &str) -> BlockId {
    text.parse().unwrap()
}

fn space_id(text: &str) -> SpaceId {
    text.parse().unwrap()
}

/// `(page, block)` of every hit of a search over all spaces, sorted.
fn found(db: &Db, query: &str) -> Vec<(PageId, Option<BlockId>)> {
    found_in(db, query, None, false)
}

fn found_in(
    db: &Db,
    query: &str,
    space_id: Option<SpaceId>,
    titles_only: bool,
) -> Vec<(PageId, Option<BlockId>)> {
    let hits = db
        .fts_search(&SearchParams {
            query,
            space_id,
            titles_only,
            limit: 50,
            any_word: false,
            edited_after: None,
        })
        .unwrap();
    let mut ids: Vec<_> = hits.iter().map(|h| (h.page_id, h.block_id)).collect();
    ids.sort();
    ids
}

fn sorted(mut ids: Vec<(PageId, Option<BlockId>)>) -> Vec<(PageId, Option<BlockId>)> {
    ids.sort();
    ids
}

#[test]
fn fts5_is_compiled_into_the_sqlite_the_app_uses() {
    let dir = tempfile::tempdir().unwrap();
    let plain = open_plain(&dir.path().join("plain.db"));
    let encrypted = open_encrypted(&dir.path().join("encrypted.db"), &db_key(1));
    for (name, db) in [("plain", &plain), ("SQLCipher", &encrypted)] {
        let options = compile_options(db.connection());
        assert!(
            options.iter().any(|option| option == "ENABLE_FTS5"),
            "{name}: {options:?}"
        );
    }
}

#[test]
fn migrating_the_v2_fixture_to_v3_keeps_its_data_and_adds_the_text_tables() {
    const KEPT: [&str; 7] = [
        "meta",
        "workspace",
        "settings",
        "docs",
        "doc_snapshots",
        "doc_updates",
        "pages_index",
    ];
    let (_dir, copy) = copy_of(FIXTURE_V2, "v2.db");
    let before: Vec<Vec<String>> = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "2", "a schema v2 file");
        // The v2 file has the index that v3 drops and not the one that v3 adds.
        let pages_index = index_names(&conn, "pages_index");
        assert!(pages_index.contains(&"pages_index_space".to_owned()));
        assert!(!index_names(&conn, "docs").contains(&"docs_space_kind".to_owned()));
        KEPT.iter().map(|table| dump(&conn, table)).collect()
    };
    let rows_before: Vec<usize> = before.iter().map(Vec::len).collect();
    assert_eq!(
        rows_before,
        [4, 1, 2, 3, 1, 6, 5],
        "4 meta rows, the workspace, 2 settings, 3 documents, 1 snapshot, 6 updates, 5 pages"
    );

    let mut db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    for (table, rows) in KEPT.iter().zip(&before) {
        assert_eq!(&dump(db.connection(), table), rows, "{table} is unchanged");
    }

    // The index changes of v3.
    let pages_index = index_names(db.connection(), "pages_index");
    assert!(
        !pages_index.contains(&"pages_index_space".to_owned()),
        "{pages_index:?}"
    );
    assert!(pages_index.contains(&"pages_index_space_trashed".to_owned()));
    let docs = index_names(db.connection(), "docs");
    for name in ["docs_kind", "docs_space", "docs_space_kind"] {
        assert!(docs.contains(&name.to_owned()), "{name} in {docs:?}");
    }
    assert!(index_names(db.connection(), "links").contains(&"links_to".to_owned()));
    assert!(
        index_names(db.connection(), "fts_blocks").contains(&"fts_blocks_page".to_owned()),
        "the text rows of a page are found by index"
    );

    // The new tables start empty. That is what the core's open-time check sees: pages
    // without a title row, so it rebuilds the text index of the space.
    assert_eq!(count_rows(db.connection(), "fts_blocks"), 0);
    assert_eq!(count_rows(db.connection(), "links"), 0);
    let personal = space_id(V2_PERSONAL);
    assert_eq!(db.pages_index_by_space(personal).unwrap().len(), 4);
    assert_eq!(db.fts_page_count(personal).unwrap(), 0);

    // The migrated file takes index rows, and they are in the file after a reopen.
    let (home, notes) = (page_id(V2_PAGE_HOME), page_id(V2_PAGE_NOTES));
    let block = block_id(V3_BLOCK_HOME_WELCOME);
    db.transaction(|tx| {
        tx.fts_replace_page(
            home,
            "Home",
            &[FtsBlock {
                block_id: block,
                text: "hello migrated world".to_owned(),
            }],
        )?;
        tx.links_replace_from(
            home,
            &[LinkRow {
                from_block: block,
                to_page: notes,
                to_block: None,
            }],
        )
    })
    .unwrap();
    drop(db);

    let db = open_plain(&copy);
    assert_eq!(found(&db, "migrated"), [(home, Some(block))]);
    assert_eq!(db.fts_page_count(personal).unwrap(), 1);
    assert_eq!(
        db.links_to(notes).unwrap(),
        [LinkSource {
            from_page: home,
            from_block: block,
            to_block: None
        }]
    );
}

#[test]
fn the_v3_fixture_opens_at_version_3_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V3).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V3, "v3.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "3",
        "the fixture is a schema v3 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    if SCHEMA_VERSION == 3 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

// The ids of `fixtures/db/v3.db`: the spaces and pages of v2 (above) and the blocks that
// `examples/make_fixture_db.rs` writes.
const V3_BLOCK_HOME_WELCOME: &str = "b10c0000-0000-4000-8000-000000000001";
const V3_BLOCK_HOME_PLAN: &str = "b10c0000-0000-4000-8000-000000000002";
const V3_BLOCK_NOTES_MEETING: &str = "b10c0000-0000-4000-8000-000000000003";
const V3_BLOCK_NOTES_BACK: &str = "b10c0000-0000-4000-8000-000000000004";
const V3_BLOCK_NOTES_QUOTE: &str = "b10c0000-0000-4000-8000-000000000005";
const V3_BLOCK_OLD_IDEAS: &str = "b10c0000-0000-4000-8000-000000000006";
const V3_BLOCK_ARCHIVED: &str = "b10c0000-0000-4000-8000-000000000007";
const V3_BLOCK_ROADMAP_CAFE: &str = "b10c0000-0000-4000-8000-000000000009";

#[test]
fn the_v3_fixture_reads_back_through_the_documents_and_text_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V3, "v3.db");
    let db = open_plain(&copy);
    // The word index the triggers built agrees with the text rows (FTS5 tokenizes them
    // again and compares).
    db.connection()
        .execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    let (personal, work) = (space_id(V2_PERSONAL), space_id(V2_WORK));
    let (home, notes, old, old_child, roadmap) = (
        page_id(V2_PAGE_HOME),
        page_id(V2_PAGE_NOTES),
        page_id(V2_PAGE_OLD),
        page_id(V2_PAGE_OLD_CHILD),
        page_id(V2_PAGE_ROADMAP),
    );
    let (welcome, plan, meeting) = (
        block_id(V3_BLOCK_HOME_WELCOME),
        block_id(V3_BLOCK_HOME_PLAN),
        block_id(V3_BLOCK_NOTES_MEETING),
    );
    let road_cafe = block_id(V3_BLOCK_ROADMAP_CAFE);

    // The page documents: the id of a page's document is the page id; the v2 documents are
    // all still there.
    let page_docs: Vec<(DocId, Option<SpaceId>)> = db
        .docs_by_kind(DocKindCode::Page)
        .unwrap()
        .iter()
        .map(|row| (row.doc_id, row.space_id))
        .collect();
    assert_eq!(
        page_docs,
        [
            (DocId::for_page(home), Some(personal)),
            (DocId::for_page(notes), Some(personal)),
            (DocId::for_page(old), Some(personal)),
            (DocId::for_page(roadmap), Some(work)),
        ]
    );
    assert_eq!(db.docs_by_kind(DocKindCode::Structure).unwrap().len(), 2);
    assert_eq!(db.docs_by_kind(DocKindCode::Account).unwrap().len(), 1);
    assert_eq!(
        db.doc_get(DocId::for_page(old_child)).unwrap(),
        None,
        "a page that was never opened has no document"
    );
    let notes_doc = DocId::for_page(notes);
    assert_eq!(
        db.snapshot_get(notes_doc).unwrap(),
        Some((
            b"KBD1\x01\x01\x01\x01snapshot\xff\x00\x80 page notes".to_vec(),
            vec![0x01, 0x02, 0x00, 0xff, 0x7f]
        ))
    );
    assert_eq!(
        db.updates_after_snapshot(notes_doc).unwrap(),
        [v2_update(0x02, 3), v2_update(0x02, 4)]
    );
    assert_eq!(
        db.updates_after_snapshot(DocId::for_page(home)).unwrap(),
        [v2_update(0x01, 1), v2_update(0x01, 2)]
    );

    // One title row per page, trashed pages included.
    assert_eq!(db.fts_page_count(personal).unwrap(), 4);
    assert_eq!(db.fts_page_count(work).unwrap(), 1);
    assert_eq!(
        db.fts_page_count(space_id("5ba0ce00-0000-4000-8000-0000000000ff"))
            .unwrap(),
        0
    );
    assert_eq!(
        db.fts_block_text(home, welcome).unwrap().as_deref(),
        Some("Welcome to the fixture workspace. Café notes and ideas live under Notes.")
    );
    assert_eq!(
        db.fts_block_text(old_child, block_id(V3_BLOCK_ARCHIVED))
            .unwrap()
            .as_deref(),
        Some("Brouillon archivé: éléments à relire"),
        "trashed pages keep their rows"
    );
    assert_eq!(db.fts_block_text(home, meeting).unwrap(), None);

    // Search: titles and blocks, trashed pages left out.
    assert_eq!(
        found(&db, "notes"),
        sorted(vec![
            (home, Some(welcome)),
            (home, Some(plan)),
            (notes, None),
            (notes, Some(meeting)),
            (roadmap, Some(road_cafe)),
        ])
    );
    assert_eq!(found_in(&db, "notes", None, true), [(notes, None)]);
    assert_eq!(
        found_in(&db, "notes", Some(personal), false),
        sorted(vec![
            (home, Some(welcome)),
            (home, Some(plan)),
            (notes, None),
            (notes, Some(meeting)),
        ])
    );
    assert_eq!(
        found_in(&db, "notes", Some(work), false),
        [(roadmap, Some(road_cafe))]
    );
    assert_eq!(
        found(&db, "ideas"),
        [(home, Some(welcome))],
        "\"Old ideas\" and its block are trashed"
    );
    assert!(
        found(&db, "elements").is_empty(),
        "trashed, though it matches"
    );

    // Accents are folded: "cafe" finds "Café" and "café"; a prefix finds the rest.
    assert_eq!(
        found(&db, "cafe"),
        sorted(vec![
            (home, Some(welcome)),
            (notes, Some(meeting)),
            (roadmap, Some(road_cafe)),
        ])
    );
    assert_eq!(found(&db, "caf\u{e9}"), found(&db, "cafe"));
    assert_eq!(
        found(&db, "road"),
        sorted(vec![(home, Some(plan)), (roadmap, None)]),
        "a title and a block that mentions the page"
    );

    // Highlights are UTF-16 offsets: the astral letter counts for two units.
    let astral = db
        .fts_search(&SearchParams {
            query: "\u{1D518}nic",
            space_id: None,
            titles_only: false,
            limit: 10,
            any_word: false,
            edited_after: None,
        })
        .unwrap();
    assert_eq!(astral.len(), 1);
    assert_eq!(astral[0].block_id, Some(meeting));
    assert_eq!(
        astral[0].snippet,
        "Meeting notes with \u{1D518}nicode \u{1F389} and café \u{2615}"
    );
    assert_eq!(astral[0].highlights, [(19, 27)]);
}

#[test]
fn the_v3_fixture_reads_back_through_the_link_queries_and_follows_the_page_index() {
    let (_dir, copy) = copy_of(FIXTURE_V3, "v3.db");
    let db = open_plain(&copy);
    let (home, notes, old, roadmap) = (
        page_id(V2_PAGE_HOME),
        page_id(V2_PAGE_NOTES),
        page_id(V2_PAGE_OLD),
        page_id(V2_PAGE_ROADMAP),
    );
    let personal = space_id(V2_PERSONAL);
    let (welcome, plan, back, quote, road_cafe) = (
        block_id(V3_BLOCK_HOME_WELCOME),
        block_id(V3_BLOCK_HOME_PLAN),
        block_id(V3_BLOCK_NOTES_BACK),
        block_id(V3_BLOCK_NOTES_QUOTE),
        block_id(V3_BLOCK_ROADMAP_CAFE),
    );
    let row = |from_block, to_page, to_block| LinkRow {
        from_block,
        to_page,
        to_block,
    };
    let source = |from_page, from_block, to_block| LinkSource {
        from_page,
        from_block,
        to_block,
    };

    assert_eq!(
        db.links_from(home).unwrap(),
        [row(plan, notes, None), row(plan, roadmap, None)],
        "one row per (block, page), ordered by block then page"
    );
    assert_eq!(
        db.links_from(notes).unwrap(),
        [row(back, home, None), row(quote, home, Some(welcome))]
    );
    assert_eq!(
        db.links_from(old).unwrap(),
        [row(block_id(V3_BLOCK_OLD_IDEAS), home, None)]
    );
    assert_eq!(
        db.links_from(roadmap).unwrap(),
        [row(road_cafe, notes, None)]
    );

    // Backlinks leave out the trashed source "Old ideas"; the cross-space ones stay.
    assert_eq!(
        db.links_to(home).unwrap(),
        [
            source(notes, back, None),
            source(notes, quote, Some(welcome))
        ]
    );
    assert_eq!(
        db.links_to(notes).unwrap(),
        [source(home, plan, None), source(roadmap, road_cafe, None)]
    );
    assert_eq!(db.links_to(roadmap).unwrap(), [source(home, plan, None)]);
    assert!(db.links_to(old).unwrap().is_empty());

    // The filter reads `pages_index` when asked: restoring the page brings its link back
    // (and its rows were never gone from the text index).
    let mut restored = db.pages_index_get(old).unwrap().unwrap();
    assert!(restored.trashed_at.is_some());
    restored.trashed_at = None;
    db.pages_index_upsert(&[restored]).unwrap();
    assert_eq!(db.links_to(home).unwrap().len(), 3);
    assert_eq!(
        found(&db, "ideas").len(),
        3,
        "Home, \"Old ideas\", its block"
    );

    // Purging a page removes it from both indexes.
    db.links_delete_page(home).unwrap();
    db.fts_delete_page(home).unwrap();
    assert!(db.links_from(home).unwrap().is_empty());
    assert!(db.links_to(home).unwrap().is_empty());
    assert_eq!(
        db.links_to(notes).unwrap(),
        [source(roadmap, road_cafe, None)]
    );
    assert_eq!(db.fts_page_count(personal).unwrap(), 3);
    assert_eq!(
        found(&db, "notes").len(),
        3,
        "its title, a block and Roadmap's"
    );
}

#[test]
fn the_text_and_link_indexes_survive_a_reopen_and_leave_no_plaintext_in_an_encrypted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let space = SpaceId::from_bytes([2; 16]);
    let (from, to) = (PageId::from_bytes([3; 16]), PageId::from_bytes([4; 16]));
    let block = BlockId::from_bytes([5; 16]);
    let title = format!("{MARKER}-title");
    let body = format!("{MARKER}-body words");

    let mut db = open_encrypted(&path, &key);
    db.transaction(|tx| {
        tx.pages_index_upsert(&[index_row(3, space, "page")])?;
        tx.fts_replace_page(
            from,
            &title,
            &[FtsBlock {
                block_id: block,
                text: body.clone(),
            }],
        )?;
        tx.links_replace_from(
            from,
            &[LinkRow {
                from_block: block,
                to_page: to,
                to_block: None,
            }],
        )
    })
    .unwrap();

    // The full-text index stores the words of a text apart, so look for them too.
    let leaks = |dir: &Path| {
        files_in(dir).iter().any(|file| {
            let bytes = fs::read(file).unwrap();
            [MARKER.as_bytes(), b"3f9a7c1e", b"plaintext"]
                .iter()
                .any(|needle| contains(&bytes, needle))
        })
    };
    assert!(!leaks(dir.path()), "plaintext in an open database's files");
    drop(db);
    assert!(!leaks(dir.path()), "plaintext in a closed database's files");

    let db = open_encrypted(&path, &key);
    assert_eq!(
        found(&db, "3f9a7c1e"),
        sorted(vec![(from, None), (from, Some(block))]),
        "title and block"
    );
    assert_eq!(
        db.fts_block_text(from, block).unwrap().as_deref(),
        Some(body.as_str())
    );
    assert_eq!(
        db.links_to(to).unwrap(),
        [LinkSource {
            from_page: from,
            from_block: block,
            to_block: None
        }]
    );
}

#[test]
fn index_writes_of_one_connection_are_visible_to_another_when_the_transaction_ends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let mut writer = open_encrypted(&path, &key);
    let reader = open_encrypted(&path, &key);
    let space = SpaceId::from_bytes([2; 16]);
    let page = PageId::from_bytes([3; 16]);
    let block = BlockId::from_bytes([5; 16]);
    writer
        .pages_index_upsert(&[index_row(3, space, "page")])
        .unwrap();

    // Nothing leaks out of a transaction that is still open, or that rolled back.
    let outcome = writer.transaction(|tx| {
        tx.fts_replace_page(
            page,
            "Draft",
            &[FtsBlock {
                block_id: block,
                text: "visible only inside".to_owned(),
            }],
        )?;
        assert_eq!(found_in_tx(tx, "inside"), 1);
        assert!(found(&reader, "inside").is_empty());
        Err::<(), _>(StorageError::InvalidData("rolled back".to_owned()))
    });
    assert!(outcome.is_err());
    assert!(found(&reader, "inside").is_empty());
    assert!(found(&writer, "inside").is_empty());

    // A committed one is visible at once, to a reader that was open all along.
    writer
        .transaction(|tx| {
            tx.fts_replace_page(
                page,
                "Final",
                &[FtsBlock {
                    block_id: block,
                    text: "visible after the commit".to_owned(),
                }],
            )
        })
        .unwrap();
    assert_eq!(found(&reader, "commit"), [(page, Some(block))]);
    assert_eq!(found(&reader, "final"), [(page, None)]);
}

/// Number of hits of a search run inside a transaction.
fn found_in_tx(tx: &impl TextIndexQueries, query: &str) -> usize {
    tx.fts_search(&SearchParams {
        query,
        space_id: None,
        titles_only: false,
        limit: 50,
        any_word: false,
        edited_after: None,
    })
    .unwrap()
    .len()
}

// --- documents in real files --------------------------------------------------------

fn structure_doc(doc_id: DocId, space_id: SpaceId) -> DocRow {
    DocRow {
        doc_id,
        kind: DocKindCode::Structure,
        space_id: Some(space_id),
        created_at: 1,
        format_version: 1,
        min_reader_version: 1,
    }
}

fn index_row(page: u8, space_id: SpaceId, title: &str) -> PageIndexRow {
    PageIndexRow {
        page_id: PageId::from_bytes([page; 16]),
        space_id,
        parent_id: None,
        title: title.to_owned(),
        trashed_at: None,
        updated_at: 4,
        in_template: false,
    }
}

#[test]
fn a_write_outside_a_transaction_is_committed_when_it_returns() {
    // `update_append` is one `INSERT ... RETURNING`: it has to be durable and visible
    // to other connections as soon as it returns, not when the statement is finalised.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let writer = open_encrypted(&path, &key);
    let reader = open_encrypted(&path, &key);
    let doc_id = DocId::from_bytes([1; 16]);
    let space_id = SpaceId::from_bytes([2; 16]);

    writer.doc_insert(&structure_doc(doc_id, space_id)).unwrap();
    let first = writer
        .update_append(doc_id, b"one", UpdateOrigin::Local, 1)
        .unwrap();
    assert_eq!(first, 1);
    assert_eq!(
        reader.updates_after_snapshot(doc_id).unwrap(),
        [b"one".to_vec()]
    );

    // The numbering is shared between the connections.
    let second = reader
        .update_append(doc_id, b"two", UpdateOrigin::Remote, 2)
        .unwrap();
    assert_eq!(second, 2);
    assert_eq!(writer.update_stats(doc_id).unwrap().count, 2);

    // A batch is committed in one go.
    writer
        .pages_index_upsert(&[index_row(1, space_id, "a"), index_row(2, space_id, "b")])
        .unwrap();
    assert_eq!(reader.pages_index_live_count(space_id).unwrap(), 2);
}

#[test]
fn a_transaction_holds_the_write_lock_from_its_first_statement() {
    // `BEGIN IMMEDIATE`: even a transaction that has only read so far owns the write
    // lock, so it cannot fail half way because another writer got in first.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let mut owner = open_encrypted(&path, &key);
    let other = open_encrypted(&path, &key);
    other
        .connection()
        .busy_timeout(std::time::Duration::ZERO)
        .unwrap();
    let doc_id = DocId::from_bytes([1; 16]);
    let doc = structure_doc(doc_id, SpaceId::from_bytes([2; 16]));

    owner
        .transaction(|tx| {
            assert_eq!(tx.doc_get(doc_id)?, None, "a read, nothing else yet");
            let err = other.doc_insert(&doc).unwrap_err();
            assert!(
                matches!(
                    &err,
                    StorageError::Sqlite(kb_storage::rusqlite::Error::SqliteFailure(failure, _))
                        if failure.code == ErrorCode::DatabaseBusy
                ),
                "{err}"
            );
            Ok(())
        })
        .unwrap();

    // Once the transaction is over the other connection can write.
    other.doc_insert(&doc).unwrap();
    assert_eq!(owner.doc_get(doc_id).unwrap(), Some(doc));
}

#[test]
fn transactions_on_two_connections_number_every_update_exactly_once() {
    const PER_THREAD: i64 = 40;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let doc_id = DocId::from_bytes([1; 16]);
    open_encrypted(&path, &key)
        .doc_insert(&structure_doc(doc_id, SpaceId::from_bytes([2; 16])))
        .unwrap();

    // Two writers appending as the core does (append, then read the stats, one
    // transaction each). Contended writers wait for each other and never share a number.
    let (key_ref, path_ref) = (&key, &path);
    let mut seqs: Vec<i64> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(move || {
                    let mut db = open_encrypted(path_ref, key_ref);
                    (0..PER_THREAD)
                        .map(|_| {
                            db.transaction(|tx| {
                                let seq =
                                    tx.update_append(doc_id, b"update", UpdateOrigin::Local, 1)?;
                                assert_eq!(tx.update_stats(doc_id)?.last_seq, Some(seq));
                                Ok(seq)
                            })
                            .unwrap()
                        })
                        .collect::<Vec<i64>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    seqs.sort_unstable();
    assert_eq!(seqs, (1..=2 * PER_THREAD).collect::<Vec<i64>>());

    let stats = open_encrypted(&path, &key).update_stats(doc_id).unwrap();
    assert_eq!((stats.count, stats.last_seq), (80, Some(80)));
}

#[test]
fn documents_survive_a_reopen_and_leave_no_plaintext_in_an_encrypted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let doc_id = DocId::from_bytes([1; 16]);
    let space_id = SpaceId::from_bytes([2; 16]);
    let blob = format!("{MARKER}-blob").into_bytes();

    let mut db = open_encrypted(&path, &key);
    db.transaction(|tx| {
        tx.doc_insert(&structure_doc(doc_id, space_id))?;
        tx.snapshot_put(doc_id, MARKER.as_bytes(), &blob, 2)?;
        tx.update_append(doc_id, &blob, UpdateOrigin::Local, 3)?;
        tx.pages_index_upsert(&[index_row(3, space_id, MARKER)])
    })
    .unwrap();

    // Scan while the connection is open (data may still sit in the WAL), then after.
    let leaks = |dir: &Path| {
        files_in(dir)
            .iter()
            .any(|file| contains(&fs::read(file).unwrap(), MARKER.as_bytes()))
    };
    assert!(!leaks(dir.path()), "plaintext in an open database's files");
    drop(db);
    assert!(!leaks(dir.path()), "plaintext in a closed database's files");

    let db = open_encrypted(&path, &key);
    assert_eq!(
        db.snapshot_get(doc_id).unwrap(),
        Some((blob.clone(), MARKER.as_bytes().to_vec()))
    );
    assert_eq!(db.updates_after_snapshot(doc_id).unwrap(), [blob]);
    let rows = db.pages_index_by_space(space_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, MARKER);
}

// --- SQLCipher ----------------------------------------------------------------------

#[test]
fn encrypted_file_keeps_data_across_reopen_with_the_right_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let key = db_key(1);

    let db = open_encrypted(&path, &key);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    fill(&db);
    assert_filled(&db);
    drop(db);

    let db = open_encrypted(&path, &key);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_filled(&db);
}

#[test]
fn wrong_key_is_rejected_and_the_file_survives_the_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let key_a = db_key(1);
    let key_b = db_key(2);
    assert_ne!(key_hex(&key_a), key_hex(&key_b));

    let db = open_encrypted(&path, &key_a);
    fill(&db);
    drop(db);
    let before = fs::read(&path).unwrap();

    let err = sqlcipher::open_file(&path, &key_b).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    let message = err.to_string();
    assert!(!message.contains(&key_hex(&key_a)), "{message}");
    assert!(!message.contains(&key_hex(&key_b)), "{message}");
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "a failed open writes nothing"
    );

    assert_filled(&open_encrypted(&path, &key_a));
}

#[test]
fn encrypted_file_cannot_be_read_by_plain_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let db = open_encrypted(&path, &db_key(1));
    fill(&db);
    drop(db);

    let plain = Connection::open(&path).unwrap();
    let err = plain
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_err();
    assert!(
        matches!(
            &err,
            kb_storage::rusqlite::Error::SqliteFailure(failure, message)
                if failure.code == ErrorCode::NotADatabase
                    && message.as_deref() == Some("file is not a database")
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("file is not a database"), "{err}");

    // And Db::new refuses it as a wrong key, even when the opener did not check.
    let err = Db::new(OpenedDb {
        conn: plain,
        encryption: Encryption::None,
    })
    .unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
}

#[test]
fn encrypted_file_has_no_plaintext_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let key = db_key(1);

    let db = open_encrypted(&path, &key);
    fill(&db);
    // Scan while the connection is open (data may still sit in the WAL), then after.
    for file in files_in(dir.path()) {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !contains(&bytes, MARKER.as_bytes()),
            "{file:?} leaks plaintext"
        );
        assert!(
            !contains(&bytes, b"SQLite format 3"),
            "{file:?} has a plain header"
        );
    }
    drop(db);

    let main = fs::read(&path).unwrap();
    assert!(!main.is_empty());
    assert_ne!(
        &main[..16],
        b"SQLite format 3\0",
        "the header is encrypted too"
    );
    for file in files_in(dir.path()) {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !contains(&bytes, MARKER.as_bytes()),
            "{file:?} leaks plaintext"
        );
        assert!(
            !contains(&bytes, key_hex(&key).as_bytes()),
            "{file:?} contains the key"
        );
    }
}

#[test]
fn plaintext_file_is_rejected_by_the_keyed_open_and_left_untouched() {
    let original = fs::read(FIXTURE_V1).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("v1.db");
    fs::write(&copy, &original).unwrap();

    let err = sqlcipher::open_file(&copy, &db_key(1)).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    assert_eq!(fs::read(&copy).unwrap(), original);
}

#[test]
fn cipher_version_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let opened = sqlcipher::open_file(&dir.path().join("workspace.db"), &db_key(1)).unwrap();
    let version = match &opened.encryption {
        Encryption::SqlCipher { version } => version.clone(),
        _ => String::new(),
    };
    assert!(
        !version.is_empty(),
        "expected SQLCipher, got {:?}",
        opened.encryption
    );
    assert!(
        version.chars().next().unwrap().is_ascii_digit(),
        "{version}"
    );
    assert!(version.contains('.'), "{version}");
    assert_eq!(version, pragma(&opened.conn, "cipher_version"));

    let db = Db::new(opened).unwrap();
    assert!(db.encryption().is_encrypted());
    assert_eq!(db.encryption().describe(), format!("SQLCipher {version}"));
}

#[test]
fn connection_is_configured_as_documented() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_encrypted(&dir.path().join("workspace.db"), &db_key(1));
    let conn = db.connection();
    assert_eq!(pragma(conn, "journal_mode"), "wal");
    assert_eq!(pragma(conn, "foreign_keys"), "1");
    // SQLCipher's default; deliberately left off (see the note on `sqlcipher::open_file`).
    assert_eq!(pragma(conn, "cipher_memory_security"), "0");
    // Not the default (WARN): see the note on `sqlcipher::open_file`.
    assert_eq!(pragma(conn, "cipher_log_level"), "ERROR");
}

#[test]
fn many_encrypted_connections_in_one_process_do_not_crash() {
    // Regression test: with memory security on and SQLCipher's default WARN logging,
    // the process died with a stack overflow once VirtualLock hit the working-set
    // quota on Windows. Memory security is now off, but this keeps guarding the open
    // sequence against process-wide SQLCipher state problems: several connections
    // with real writes in one process must stay stable.
    let dir = tempfile::tempdir().unwrap();
    let key = db_key(1);
    for round in 0..12 {
        let db = open_encrypted(&dir.path().join(format!("db{round}.db")), &key);
        for i in 0..150 {
            db.setting_set(&format!("app.k{i}"), &json!("x".repeat(1_000)), i)
                .unwrap();
        }
        assert_eq!(
            db.setting_get("app.k149")
                .unwrap()
                .unwrap()
                .as_str()
                .unwrap()
                .len(),
            1_000
        );
    }
}

#[test]
fn open_file_creates_missing_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("workspace.db");
    let db = open_encrypted(&path, &db_key(1));
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    assert!(path.is_file());
}

#[test]
fn paths_with_spaces_and_non_ascii_characters_work() {
    // Windows user names and OS-provided data directories routinely look like this.
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("Données de Zoë - 日本語")
        .join("atomik kb")
        .join(DB_FILE_NAME);
    let key = db_key(1);

    let db = open_encrypted(&path, &key);
    fill(&db);
    drop(db);
    assert!(path.is_file());
    assert_filled(&open_encrypted(&path, &key));
}

#[test]
fn two_connections_to_one_file_are_serialised() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let key = db_key(1);

    let first = open_encrypted(&path, &key);
    let second = open_encrypted(&path, &key);
    first.setting_set("ui.a", &json!(1), 1).unwrap();
    assert_eq!(second.setting_get("ui.a").unwrap(), Some(json!(1)));
    second.setting_set("ui.b", &json!(2), 2).unwrap();
    assert_eq!(first.setting_get("ui.b").unwrap(), Some(json!(2)));

    // Contended writers wait for each other instead of failing.
    const WRITES: i64 = 40;
    let (key_ref, path_ref) = (&key, &path);
    std::thread::scope(|scope| {
        for name in ["w1", "w2"] {
            scope.spawn(move || {
                let db = open_encrypted(path_ref, key_ref);
                for i in 0..WRITES {
                    db.setting_set(&format!("app.{name}-{i}"), &json!(i), i)
                        .unwrap();
                }
            });
        }
    });
    let count: i64 = first
        .connection()
        .query_row(
            "SELECT count(*) FROM settings WHERE key LIKE 'app.w%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2 * WRITES);
}

// --- schema v4: the device log, the keyring and the recovery keys -----------------------

/// Every table of a database, the shadow tables of the word index included, with all its
/// rows as lines of text (see `dump`), by table name. Not `icon_refs` (schema v14): a file of an
/// older version gains it empty when it is migrated, which none of the tests of the older
/// migrations is about; the tests of v14 read it with `dump` and `dump_all_with_icon_refs`.
fn dump_all(conn: &Connection) -> Vec<(String, Vec<String>)> {
    dump_all_with_icon_refs(conn)
        .into_iter()
        .filter(|(name, _)| name != "icon_refs")
        .collect()
}

/// The tables of schema v15 (the semantic index and the MCP clients and audit). A file of an
/// older version gains them when it is migrated (`ai_dirty` with a row per page that has text),
/// which none of the tests of the older migrations is about; the tests of v15 read them with
/// `dump_all_with_ai`.
const V15_TABLES: [&str; 5] = [
    "ai_chunks",
    "ai_dirty",
    "ai_pages",
    "mcp_audit",
    "mcp_clients",
];

/// [`dump_all`] with the `icon_refs` table of schema v14 (when there is one), without the tables
/// of schema v15.
fn dump_all_with_icon_refs(conn: &Connection) -> Vec<(String, Vec<String>)> {
    dump_all_with_ai(conn)
        .into_iter()
        .filter(|(name, _)| !V15_TABLES.contains(&name.as_str()))
        .collect()
}

/// The tables of schema v17 (the feeds, the co-members' device logs, the quarantine, the snapshot
/// marks and the membership log of shared spaces). A file of an older version gains them empty when
/// it is migrated, which none of the tests of the older migrations is about; the tests of v17 read
/// them with `dump_every_table`.
const V17_TABLES: [&str; 7] = [
    "foreign_device_logs",
    "membership_entries",
    "membership_state",
    "sync_entry_marks",
    "sync_feeds",
    "sync_rejected",
    "sync_rejected_dropped",
];

/// The tables of schema v18 (comments with their full-text table and its shadow tables, the
/// notifications and the person mentions). A file of an older version gains them empty when it is
/// migrated, which none of the tests of the older migrations is about; the tests of v18 read them
/// with `dump_every_table`.
const V18_TABLES: [&str; 9] = [
    "comment_fts",
    "comment_fts_config",
    "comment_fts_content",
    "comment_fts_data",
    "comment_fts_docsize",
    "comment_fts_idx",
    "comments",
    "notifications",
    "person_mentions",
];

/// The table of schema v19 (the first picture of each page, ADR-0054). A file of an older version gains it
/// empty when it is migrated, which none of the tests of the older migrations is about; the tests of v19 read
/// it with `dump_every_table`.
const V19_TABLES: [&str; 1] = ["page_first_images"];

/// The table of schema v21 (the page history journal, ADR-0057). Left out of [`dump_every_table`] and
/// [`dump_raw_every_table`]: a file of an older version gains it empty when it is migrated, which none of the
/// tests of the older migrations is about; the test of v21 reads it by name.
const V21_TABLES: [&str; 1] = ["page_history_journal"];

/// Every table, the tables of schema v15 included (when there are any), without the tables of
/// schema v17 (see [`V17_TABLES`]), v18 (see [`V18_TABLES`]) and v19 (see [`V19_TABLES`]).
fn dump_all_with_ai(conn: &Connection) -> Vec<(String, Vec<String>)> {
    dump_every_table(conn)
        .into_iter()
        .filter(|(name, _)| !V17_TABLES.contains(&name.as_str()))
        .filter(|(name, _)| !V18_TABLES.contains(&name.as_str()))
        .filter(|(name, _)| !V19_TABLES.contains(&name.as_str()))
        .collect()
}

/// Every table, whatever its schema version.
#[allow(dead_code)]
fn dump_every_table(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let names: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names
        .into_iter()
        .filter(|name| !V21_TABLES.contains(&name.as_str()))
        .map(|name| {
            let rows = dump(conn, &name);
            (name, rows)
        })
        .collect()
}

/// The rows of one table in the result of `dump_all`.
fn rows_of<'a>(dump: &'a [(String, Vec<String>)], table: &str) -> &'a [String] {
    &dump
        .iter()
        .find(|(name, _)| name == table)
        .unwrap_or_else(|| unreachable!("no table {table} in the dump"))
        .1
}

/// The tables of a dump without `chunk_sync` (schema v8), for comparing a file migrated to
/// v8 or later with the dump of the older file it was.
fn without_chunk_sync(dump: Vec<(String, Vec<String>)>) -> Vec<(String, Vec<String>)> {
    dump.into_iter()
        .filter(|(name, _)| name != "chunk_sync")
        .collect()
}

/// The tables of a dump without `date_mentions` (schema v9), for comparing a file migrated to
/// v9 or later with the dump of the older file it was.
fn without_date_mentions(dump: Vec<(String, Vec<String>)>) -> Vec<(String, Vec<String>)> {
    dump.into_iter()
        .filter(|(name, _)| name != "date_mentions")
        .collect()
}

/// A dump as a file older than schema v10 had it: without `page_properties`, and with the rows of
/// `fts_blocks` cut back to the columns they had before the `kind` column (every migrated row has
/// the 0 of its default, written last).
fn without_properties(dump: Vec<(String, Vec<String>)>) -> Vec<(String, Vec<String>)> {
    dump.into_iter()
        .filter(|(name, _)| name != "page_properties")
        .map(|(name, rows)| {
            if name != "fts_blocks" {
                return (name, rows);
            }
            let rows = rows
                .into_iter()
                .map(|row| match row.strip_suffix("|0") {
                    Some(old) => old.to_owned(),
                    None => row,
                })
                .collect();
            (name, rows)
        })
        .collect()
}

/// The five tables of schema v6.
const SYNC_TABLES: [&str; 5] = [
    "sync_config",
    "sync_doc_state",
    "sync_heads",
    "sync_pending",
    "sync_seen",
];

/// The rows of `table` as `dump` shows them after a file of v4 or v5 is migrated to v6: the
/// device log and the keyring gained a `published` column, which is 0 in every existing
/// row, so each of their lines ends in `|0`; every other table is as it was.
fn after_v6(table: &str, rows: &[String]) -> Vec<String> {
    if matches!(table, "device_log" | "keyring") {
        let mut rows: Vec<String> = rows.iter().map(|row| format!("{row}|0")).collect();
        rows.sort();
        rows
    } else {
        rows.to_vec()
    }
}

// The ids and stand-ins of `fixtures/db/v4.db`, as written by `examples/make_fixture_db.rs`
// (the helpers below repeat its formulas).
const V4_ACCOUNT_ID: &str = "ac000000-0000-4000-8000-000000000001";
const V4_PAGE_PURGED: &str = "9a6e0000-0000-4000-8000-000000000006";

/// `len` bytes counting up from `start`, wrapping at 256.
fn ramp(start: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| start.wrapping_add(u8::try_from(i % 256).unwrap()))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}

/// The device log entry `seq` of the v4 fixture: its stand-in entry and fake hash.
fn v4_log_row(seq: u8) -> DeviceLogRow {
    let mut entry = b"KLE1".to_vec();
    entry.extend([seq, 0x00, 0xFF, 0x80]);
    entry.extend(ramp(0xA0 + seq, 24 * (usize::from(seq) + 1)));
    DeviceLogRow {
        seq: u64::from(seq),
        entry,
        hash: ramp(0x10 * (seq + 1), 32).try_into().unwrap(),
    }
}

/// The subject of a space key: a tag byte and the epoch (u32, little-endian).
fn v4_subject_space(epoch: u32) -> Vec<u8> {
    let mut subject = vec![0x00];
    subject.extend(epoch.to_le_bytes());
    subject
}

/// The subject of a document key: a tag byte, the document id and the epoch.
fn v4_subject_doc(doc: &str, epoch: u32) -> Vec<u8> {
    let mut subject = vec![0x01];
    subject.extend(doc.parse::<DocId>().unwrap().to_bytes());
    subject.extend(epoch.to_le_bytes());
    subject
}

/// A keyring row of the v4 fixture; `tag` 1 is a record and 2 a tombstone.
fn v4_key_row(space_id: SpaceId, subject: Vec<u8>, tag: u8, seed: u8) -> KeyringRow {
    let mut record = b"KKR1".to_vec();
    record.extend([tag, 0x00, 0xFF, 0x80]);
    record.extend(ramp(seed, 100));
    KeyringRow {
        space_id,
        subject,
        record,
        tombstone: tag == 2,
    }
}

#[test]
fn migrating_the_v3_fixture_keeps_every_row_of_every_table_and_adds_the_empty_key_attachment_and_sync_tables()
 {
    let (_dir, copy) = copy_of(FIXTURE_V3, "v3.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "3", "a schema v3 file");
        dump_all(&conn)
    };
    // The dump is not empty-handed: the tables of the fixture hold what it was built with.
    for (table, rows) in [
        ("meta", 4),
        ("workspace", 1),
        ("settings", 2),
        ("docs", 7),
        ("doc_snapshots", 2),
        ("doc_updates", 12),
        ("pages_index", 5),
        ("links", 6),
        ("fts_blocks", 14),
    ] {
        assert_eq!(
            rows_of(&before, table).len(),
            rows,
            "{table} in the v3 file"
        );
    }
    let old_tables: Vec<&str> = before.iter().map(|(name, _)| name.as_str()).collect();
    assert!(old_tables.contains(&"fts") && old_tables.contains(&"fts_data"));
    assert!(!old_tables.contains(&"device_log") && !old_tables.contains(&"keyring"));

    let mut db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());

    // Every table of v3 has exactly the rows it had (`fts_blocks` with the 0 of its new `kind`
    // column); the two tables of v4, the two of v5, the five of v6, the one of v8, the one of v9
    // and the one of v10 (the file migrates to the current version) are new, empty.
    let after_rows = without_properties(after.clone());
    for (table, rows) in &before {
        assert_eq!(
            rows_of(&after_rows, table),
            rows.as_slice(),
            "{table} is unchanged"
        );
    }
    let new_tables: Vec<&str> = after
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !old_tables.contains(name))
        .collect();
    assert_eq!(
        new_tables,
        [
            "attachment_refs",
            "attachments_index",
            "chunk_sync",
            "date_mentions",
            "device_log",
            "keyring",
            "page_properties",
            "sync_config",
            "sync_doc_state",
            "sync_heads",
            "sync_pending",
            "sync_seen"
        ]
    );
    for table in SYNC_TABLES {
        assert_eq!(count_rows(db.connection(), table), 0, "{table}");
    }
    assert_eq!(count_rows(db.connection(), "chunk_sync"), 0);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 0);
    assert_eq!(count_rows(db.connection(), "page_properties"), 0);
    assert_eq!(count_rows(db.connection(), "attachments_index"), 0);
    assert_eq!(count_rows(db.connection(), "attachment_refs"), 0);
    assert!(db.device_log_all().unwrap().is_empty());
    assert!(db.keyring_all().unwrap().is_empty());
    assert_eq!(count_rows(db.connection(), "device_log"), 0);
    assert_eq!(count_rows(db.connection(), "keyring"), 0);
    assert_eq!(
        db.meta_all().unwrap().len(),
        4,
        "no recovery keys are invented"
    );
    assert_eq!(db.meta_get("account_id").unwrap(), None);

    // The indexes of v3 still work on the migrated file.
    db.connection()
        .execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    assert_eq!(found(&db, "notes").len(), 5);

    // The migrated file takes log, keyring and recovery rows, and still has them after a
    // reopen.
    let key_row = v4_key_row(space_id(V2_PERSONAL), v4_subject_space(0), 1, 0x10);
    db.transaction(|tx| {
        tx.device_log_append(&v4_log_row(0))?;
        tx.device_log_append(&v4_log_row(1))?;
        tx.keyring_put(&key_row)?;
        tx.meta_set("account_id", V4_ACCOUNT_ID)
    })
    .unwrap();
    drop(db);

    let db = open_plain(&copy);
    assert_eq!(db.device_log_all().unwrap(), [v4_log_row(0), v4_log_row(1)]);
    assert_eq!(db.keyring_all().unwrap(), [key_row]);
    assert_eq!(
        db.meta_get("account_id").unwrap().as_deref(),
        Some(V4_ACCOUNT_ID)
    );
    let reopened = without_properties(dump_all(db.connection()));
    for (table, rows) in before.iter().filter(|(table, _)| table != "meta") {
        assert_eq!(rows_of(&reopened, table), rows.as_slice(), "{table}");
    }
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string(),
        "written to the file"
    );
}

#[test]
fn the_v4_fixture_opens_at_version_4_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V4).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V4, "v4.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "4",
        "the fixture is a schema v4 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    if SCHEMA_VERSION == 4 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v4_fixture_reads_back_through_the_security_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V4, "v4.db");
    let db = open_plain(&copy);
    let (personal, work) = (space_id(V2_PERSONAL), space_id(V2_WORK));

    // The device log: three entries in sequence order, with their fake hashes.
    assert_eq!(
        db.device_log_all().unwrap(),
        [v4_log_row(0), v4_log_row(1), v4_log_row(2)]
    );

    // The keyring, though the rows were written in another order: by space, then by
    // subject bytes (a space key first, then the document keys by document id, the
    // purged page's tombstone among them).
    let personal_rows = [
        v4_key_row(personal, v4_subject_space(0), 1, 0x10),
        v4_key_row(personal, v4_subject_doc(V2_PAGE_HOME, 0), 1, 0x11),
        v4_key_row(personal, v4_subject_doc(V2_PAGE_NOTES, 0), 1, 0x12),
        v4_key_row(personal, v4_subject_doc(V4_PAGE_PURGED, 0), 2, 0x14),
        v4_key_row(personal, v4_subject_doc(V2_DOC_PERSONAL, 0), 1, 0x15),
    ];
    let work_rows = [
        v4_key_row(work, v4_subject_space(0), 1, 0x31),
        v4_key_row(work, v4_subject_doc(V2_PAGE_ROADMAP, 0), 1, 0x33),
        v4_key_row(work, v4_subject_doc(V2_DOC_WORK, 0), 1, 0x32),
    ];
    assert_eq!(db.keyring_by_space(personal).unwrap(), personal_rows);
    assert_eq!(db.keyring_by_space(work).unwrap(), work_rows);
    let all_rows: Vec<KeyringRow> = personal_rows.iter().chain(&work_rows).cloned().collect();
    assert_eq!(db.keyring_all().unwrap(), all_rows);
    assert!(
        db.keyring_by_space(space_id("5ba0ce00-0000-4000-8000-0000000000ff"))
            .unwrap()
            .is_empty()
    );
    let tombstones: Vec<&KeyringRow> = all_rows.iter().filter(|row| row.tombstone).collect();
    assert_eq!(tombstones.len(), 1, "one record became a tombstone");
    assert_eq!(tombstones[0].subject, v4_subject_doc(V4_PAGE_PURGED, 0));

    // Every `meta` key, the four of the recovery kit among them.
    assert_eq!(
        db.meta_all().unwrap(),
        [
            pair("account_id", V4_ACCOUNT_ID),
            pair("core_version_at_creation", "0.1.0"),
            pair("created_at", "1760000000000"),
            pair("device_id", "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"),
            pair("device_name", "Fixture laptop"),
            pair("recovery_blob", &hex(&ramp(0x80, 72))),
            pair("recovery_created_at", "1760000100000"),
            pair("recovery_id", &hex(&ramp(0x40, 32))),
        ]
    );
    assert_eq!(
        db.meta_get("recovery_id").unwrap(),
        Some(hex(&ramp(0x40, 32)))
    );

    // The settings, ordered by key, with their time.
    assert_eq!(
        db.settings_all().unwrap(),
        [
            SettingRow {
                key: "app.locale".to_owned(),
                value: json!("en"),
                updated_at: 1_760_000_003_000,
            },
            SettingRow {
                key: "ui.theme".to_owned(),
                value: json!({ "mode": "dark", "accent": "violet" }),
                updated_at: 1_760_000_002_000,
            },
        ]
    );

    // Every document of every kind, oldest first.
    let doc_row = |doc_id, kind, space_id, created_at| DocRow {
        doc_id,
        kind,
        space_id,
        created_at,
        format_version: 1,
        min_reader_version: 1,
    };
    let page_doc = |page: &str| DocId::for_page(page_id(page));
    assert_eq!(
        db.docs_all().unwrap(),
        [
            doc_row(
                V2_DOC_ACCOUNT.parse().unwrap(),
                DocKindCode::Account,
                None,
                1_760_000_010_000
            ),
            doc_row(
                V2_DOC_PERSONAL.parse().unwrap(),
                DocKindCode::Structure,
                Some(personal),
                1_760_000_020_000
            ),
            doc_row(
                V2_DOC_WORK.parse().unwrap(),
                DocKindCode::Structure,
                Some(work),
                1_760_000_030_000
            ),
            doc_row(
                page_doc(V2_PAGE_HOME),
                DocKindCode::Page,
                Some(personal),
                1_760_000_070_000
            ),
            doc_row(
                page_doc(V2_PAGE_NOTES),
                DocKindCode::Page,
                Some(personal),
                1_760_000_071_000
            ),
            doc_row(
                page_doc(V2_PAGE_OLD),
                DocKindCode::Page,
                Some(personal),
                1_760_000_072_000
            ),
            doc_row(
                page_doc(V2_PAGE_ROADMAP),
                DocKindCode::Page,
                Some(work),
                1_760_000_073_000
            ),
        ]
    );

    // The workspace of the file is its one workspace.
    let refused = WorkspaceRow {
        id: WorkspaceId::from_bytes([9; 16]),
        name: "Another".to_owned(),
        created_at: 1,
    };
    assert!(!db.workspace_insert(&refused).unwrap());
    assert_eq!(
        db.workspace_get().unwrap().map(|row| row.name).as_deref(),
        Some("Fixture workspace")
    );

    // The numbering of the log carries on after the highest entry of a real file.
    let err = db.device_log_append(&v4_log_row(2)).unwrap_err();
    assert!(matches!(err, StorageError::InvalidData(_)), "{err}");
    db.device_log_append(&v4_log_row(3)).unwrap();
    assert_eq!(db.device_log_all().unwrap().len(), 4);

    // A deleted space takes its keyring rows with it and nobody else's.
    db.keyring_delete_space(work).unwrap();
    assert_eq!(db.keyring_all().unwrap(), personal_rows);
}

// The ids of `fixtures/db/v5.db`, as written by `examples/make_fixture_db.rs`.
const V5_ATT_PHOTO: &str = "a77a0000-0000-4000-8000-000000000001";
const V5_ATT_REPORT: &str = "a77a0000-0000-4000-8000-000000000002";
const V5_ATT_ARCHIVE: &str = "a77a0000-0000-4000-8000-000000000003";
const V5_ATT_EMPTY: &str = "a77a0000-0000-4000-8000-000000000004";
const V5_ATT_LATE: &str = "a77a0000-0000-4000-8000-000000000005";
const V5_ATT_SKETCH: &str = "a77a0000-0000-4000-8000-000000000006";
/// No index row: the attachment was purged and a block still references it.
const V5_ATT_PURGED: &str = "a77a0000-0000-4000-8000-000000000007";
const V5_BLOCK_HOME_PHOTO: &str = "b10c0000-0000-4000-8000-0000000000a1";
const V5_BLOCK_NOTES_PHOTO: &str = "b10c0000-0000-4000-8000-0000000000a2";
const V5_BLOCK_NOTES_REPORT: &str = "b10c0000-0000-4000-8000-0000000000a3";
const V5_BLOCK_OLD_SKETCH: &str = "b10c0000-0000-4000-8000-0000000000a4";
const V5_BLOCK_ROADMAP_EMPTY: &str = "b10c0000-0000-4000-8000-0000000000a5";
const V5_BLOCK_ROADMAP_GONE: &str = "b10c0000-0000-4000-8000-0000000000a6";

fn attachment_id(text: &str) -> AttachmentId {
    text.parse().unwrap()
}

/// The six rows of the v5 fixture's `attachments_index`, in the order of their ids.
fn v5_rows() -> [AttachmentRow; 6] {
    let row = |attachment: &str,
               space: &str,
               (name, mime): (&str, &str),
               (size, chunk_count): (u64, u32),
               (created_at, unreferenced_since): (i64, Option<i64>)| AttachmentRow {
        attachment_id: attachment_id(attachment),
        space_id: space_id(space),
        name: name.to_owned(),
        mime: mime.to_owned(),
        size,
        chunk_count,
        created_at,
        unreferenced_since,
    };
    [
        row(
            V5_ATT_PHOTO,
            V2_PERSONAL,
            ("Café photo.png", "image/png"),
            (1_234_567, 1),
            (1_760_000_200_000, None),
        ),
        row(
            V5_ATT_REPORT,
            V2_PERSONAL,
            ("Q3 report \u{2014} final.pdf", "application/pdf"),
            (9_437_184, 3),
            (1_760_000_200_000, None),
        ),
        row(
            V5_ATT_ARCHIVE,
            V2_PERSONAL,
            ("archive.zip", "application/zip"),
            (4_194_304, 1),
            (1_760_000_210_000, Some(1_760_000_300_000)),
        ),
        row(
            V5_ATT_EMPTY,
            V2_WORK,
            ("empty.txt", "text/plain"),
            (0, 1),
            (1_760_000_220_000, None),
        ),
        row(
            V5_ATT_LATE,
            V2_WORK,
            ("late.bin", "application/octet-stream"),
            (4_194_305, 2),
            (1_760_000_230_000, Some(1_760_000_400_000)),
        ),
        row(
            V5_ATT_SKETCH,
            V2_PERSONAL,
            ("Old sketch.jpg", "image/jpeg"),
            (51_200, 1),
            (1_760_000_190_000, None),
        ),
    ]
}

/// Every row of `attachment_refs` as `(attachment, page, block)`, in primary key order.
fn raw_attachment_refs(conn: &Connection) -> Vec<(AttachmentId, PageId, BlockId)> {
    let rows = conn
        .prepare(
            "SELECT attachment_id, page_id, block_id FROM attachment_refs \
             ORDER BY attachment_id, page_id, block_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, [u8; 16]>(0)?,
                row.get::<_, [u8; 16]>(1)?,
                row.get::<_, [u8; 16]>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows.into_iter()
        .map(|(a, p, b)| {
            (
                AttachmentId::from_bytes(a),
                PageId::from_bytes(p),
                BlockId::from_bytes(b),
            )
        })
        .collect()
}

#[test]
fn migrating_the_v4_fixture_keeps_every_row_of_every_table_and_adds_the_empty_attachment_tables() {
    let (_dir, copy) = copy_of(FIXTURE_V4, "v4.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "4", "a schema v4 file");
        dump_all(&conn)
    };
    // The dump is not empty-handed: the tables of the fixture hold what it was built with.
    for (table, rows) in [
        ("meta", 8),
        ("workspace", 1),
        ("settings", 2),
        ("docs", 7),
        ("doc_snapshots", 2),
        ("doc_updates", 12),
        ("pages_index", 5),
        ("links", 6),
        ("fts_blocks", 14),
        ("device_log", 3),
        ("keyring", 8),
    ] {
        assert_eq!(
            rows_of(&before, table).len(),
            rows,
            "{table} in the v4 file"
        );
    }
    let old_tables: Vec<&str> = before.iter().map(|(name, _)| name.as_str()).collect();
    assert!(old_tables.contains(&"device_log") && old_tables.contains(&"keyring"));
    assert!(!old_tables.contains(&"attachments_index") && !old_tables.contains(&"attachment_refs"));

    let mut db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());

    // Every table of v4 has exactly the rows it had (the log and the keyring with the 0 of
    // their new `published` column, `fts_blocks` with the 0 of its `kind`); the two tables of
    // v5, the five of v6, the one of v8, the one of v9 and the one of v10 are new, empty.
    let after_rows = without_properties(after.clone());
    for (table, rows) in &before {
        assert_eq!(
            rows_of(&after_rows, table),
            after_v6(table, rows).as_slice(),
            "{table} is unchanged"
        );
    }
    let new_tables: Vec<&str> = after
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !old_tables.contains(name))
        .collect();
    assert_eq!(
        new_tables,
        [
            "attachment_refs",
            "attachments_index",
            "chunk_sync",
            "date_mentions",
            "page_properties",
            "sync_config",
            "sync_doc_state",
            "sync_heads",
            "sync_pending",
            "sync_seen"
        ]
    );
    for table in SYNC_TABLES {
        assert_eq!(count_rows(db.connection(), table), 0, "{table}");
    }
    assert_eq!(count_rows(db.connection(), "chunk_sync"), 0);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 0);
    assert_eq!(count_rows(db.connection(), "page_properties"), 0);
    assert_eq!(count_rows(db.connection(), "attachments_index"), 0);
    assert_eq!(count_rows(db.connection(), "attachment_refs"), 0);
    assert!(db.attachments_all().unwrap().is_empty());
    assert!(
        db.attachment_ids_referenced_by(page_id(V2_PAGE_HOME))
            .unwrap()
            .is_empty()
    );

    // The indexes of v3 and the keys of v4 still work on the migrated file.
    db.connection()
        .execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    assert_eq!(found(&db, "notes").len(), 5);
    assert_eq!(db.device_log_all().unwrap().len(), 3);
    assert_eq!(db.keyring_all().unwrap().len(), 8);

    // The migrated file takes attachment rows and references, and still has them after a
    // reopen.
    let rows = v5_rows();
    let photo = (block_id(V5_BLOCK_HOME_PHOTO), attachment_id(V5_ATT_PHOTO));
    db.transaction(|tx| {
        tx.attachment_put(&rows[0])?;
        tx.attachment_put(&rows[1])?;
        tx.attachment_refs_replace(page_id(V2_PAGE_HOME), &[photo])
    })
    .unwrap();
    drop(db);

    let db = open_plain(&copy);
    assert_eq!(
        db.attachments_by_space(space_id(V2_PERSONAL)).unwrap(),
        [rows[0].clone(), rows[1].clone()]
    );
    assert_eq!(
        raw_attachment_refs(db.connection()),
        [(photo.1, page_id(V2_PAGE_HOME), photo.0)]
    );
    let reopened = without_properties(dump_all(db.connection()));
    for (table, rows) in &before {
        assert_eq!(
            rows_of(&reopened, table),
            after_v6(table, rows).as_slice(),
            "{table}"
        );
    }
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string(),
        "written to the file"
    );
}

#[test]
fn the_v5_fixture_opens_at_version_5_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V5).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V5, "v5.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "5",
        "the fixture is a schema v5 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    if SCHEMA_VERSION == 5 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v5_fixture_reads_back_through_the_attachment_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V5, "v5.db");
    let mut db = open_plain(&copy);
    let [photo, report, archive, empty, late, sketch] = v5_rows();
    let (personal, work) = (space_id(V2_PERSONAL), space_id(V2_WORK));
    let (home, notes, old, roadmap) = (
        page_id(V2_PAGE_HOME),
        page_id(V2_PAGE_NOTES),
        page_id(V2_PAGE_OLD),
        page_id(V2_PAGE_ROADMAP),
    );

    // Every row, oldest first and by id among equal times (the photo and the report were
    // created at the same instant), though the rows were written in another order.
    assert_eq!(
        db.attachments_all().unwrap(),
        [
            sketch.clone(),
            photo.clone(),
            report.clone(),
            archive.clone(),
            empty.clone(),
            late.clone()
        ]
    );
    // A space lists its attachments newest first.
    assert_eq!(
        db.attachments_by_space(personal).unwrap(),
        [
            archive.clone(),
            photo.clone(),
            report.clone(),
            sketch.clone()
        ]
    );
    assert_eq!(
        db.attachments_by_space(work).unwrap(),
        [late.clone(), empty.clone()]
    );
    assert!(
        db.attachments_by_space(space_id("5ba0ce00-0000-4000-8000-0000000000ff"))
            .unwrap()
            .is_empty()
    );
    for row in [&photo, &report, &archive, &empty, &late, &sketch] {
        assert_eq!(
            db.attachment_get(row.attachment_id).unwrap().as_ref(),
            Some(row)
        );
    }
    assert_eq!(
        db.attachment_get(attachment_id(V5_ATT_PURGED)).unwrap(),
        None,
        "the purged attachment has no row"
    );

    // The sizes and chunk counts agree with the manifests' rule: ceil(size / 4 MiB), at
    // least one chunk (an empty file is one chunk of padding).
    for row in db.attachments_all().unwrap() {
        let chunks = row.size.div_ceil(4 * 1024 * 1024).max(1);
        assert_eq!(u64::from(row.chunk_count), chunks, "{}", row.name);
    }

    // Garbage collection reads the rows that lost their last reference, strictly before the
    // cutoff and the longest first.
    let unreferenced = |cutoff| db.attachments_unreferenced_before(cutoff).unwrap();
    assert!(unreferenced(1_760_000_300_000).is_empty());
    assert_eq!(
        unreferenced(1_760_000_300_001),
        std::slice::from_ref(&archive)
    );
    assert_eq!(
        unreferenced(1_760_000_400_000),
        std::slice::from_ref(&archive)
    );
    assert_eq!(
        unreferenced(1_760_000_400_001),
        [archive.clone(), late.clone()]
    );
    assert_eq!(unreferenced(i64::MAX), [archive.clone(), late.clone()]);

    // The references: the photo is shown by two pages, a trashed page still shows its
    // sketch, and the purged attachment is still referenced.
    let count = |db: &Db, id: &str| db.attachment_reference_count(attachment_id(id)).unwrap();
    assert_eq!(count(&db, V5_ATT_PHOTO), 2);
    assert_eq!(count(&db, V5_ATT_REPORT), 1);
    assert_eq!(count(&db, V5_ATT_SKETCH), 1);
    assert_eq!(count(&db, V5_ATT_EMPTY), 1);
    assert_eq!(count(&db, V5_ATT_PURGED), 1);
    assert_eq!(count(&db, V5_ATT_ARCHIVE), 0);
    assert_eq!(count(&db, V5_ATT_LATE), 0);
    // The fixture's own bookkeeping holds: a row is unreferenced exactly when it has no
    // reference.
    for row in db.attachments_all().unwrap() {
        let referenced = db.attachment_reference_count(row.attachment_id).unwrap() > 0;
        assert_eq!(row.unreferenced_since.is_none(), referenced, "{}", row.name);
    }
    let by_page = |page: PageId| db.attachment_ids_referenced_by(page).unwrap();
    assert_eq!(by_page(home), [attachment_id(V5_ATT_PHOTO)]);
    assert_eq!(
        by_page(notes),
        [attachment_id(V5_ATT_PHOTO), attachment_id(V5_ATT_REPORT)]
    );
    assert_eq!(by_page(old), [attachment_id(V5_ATT_SKETCH)]);
    assert_eq!(
        by_page(roadmap),
        [attachment_id(V5_ATT_EMPTY), attachment_id(V5_ATT_PURGED)]
    );
    assert!(by_page(page_id(V2_PAGE_OLD_CHILD)).is_empty());
    assert_eq!(count_rows(db.connection(), "attachments_index"), 6);
    assert_eq!(
        raw_attachment_refs(db.connection()),
        [
            (
                attachment_id(V5_ATT_PHOTO),
                home,
                block_id(V5_BLOCK_HOME_PHOTO)
            ),
            (
                attachment_id(V5_ATT_PHOTO),
                notes,
                block_id(V5_BLOCK_NOTES_PHOTO)
            ),
            (
                attachment_id(V5_ATT_REPORT),
                notes,
                block_id(V5_BLOCK_NOTES_REPORT)
            ),
            (
                attachment_id(V5_ATT_EMPTY),
                roadmap,
                block_id(V5_BLOCK_ROADMAP_EMPTY)
            ),
            (
                attachment_id(V5_ATT_SKETCH),
                old,
                block_id(V5_BLOCK_OLD_SKETCH)
            ),
            (
                attachment_id(V5_ATT_PURGED),
                roadmap,
                block_id(V5_BLOCK_ROADMAP_GONE)
            ),
        ]
    );

    // The rest of the file is what v4 left: the log, the keyring and the text index.
    assert_eq!(db.device_log_all().unwrap().len(), 3);
    assert_eq!(db.keyring_all().unwrap().len(), 8);
    assert_eq!(found(&db, "notes").len(), 5);

    // Page edits and purges go through the same file: a page's references are replaced as
    // a whole, a purge drops the rows of the pages, a deleted attachment takes its
    // references with it.
    db.transaction(|tx| {
        tx.attachment_refs_replace(
            notes,
            &[(
                block_id(V5_BLOCK_NOTES_REPORT),
                attachment_id(V5_ATT_REPORT),
            )],
        )?;
        tx.attachment_refs_delete_pages(&[old])
    })
    .unwrap();
    assert_eq!(
        count(&db, V5_ATT_PHOTO),
        1,
        "the notes page dropped the photo"
    );
    assert_eq!(
        count(&db, V5_ATT_SKETCH),
        0,
        "the purged page took its reference"
    );
    assert_eq!(count(&db, V5_ATT_REPORT), 1);

    db.attachment_delete(attachment_id(V5_ATT_PHOTO)).unwrap();
    assert_eq!(db.attachments_all().unwrap().len(), 5);
    assert_eq!(count(&db, V5_ATT_PHOTO), 0);
    assert_eq!(
        raw_attachment_refs(db.connection()),
        [
            (
                attachment_id(V5_ATT_REPORT),
                notes,
                block_id(V5_BLOCK_NOTES_REPORT)
            ),
            (
                attachment_id(V5_ATT_EMPTY),
                roadmap,
                block_id(V5_BLOCK_ROADMAP_EMPTY)
            ),
            (
                attachment_id(V5_ATT_PURGED),
                roadmap,
                block_id(V5_BLOCK_ROADMAP_GONE)
            ),
        ]
    );
}

#[test]
fn attachment_rows_survive_a_reopen_and_leave_no_plaintext_in_an_encrypted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let row = AttachmentRow {
        attachment_id: AttachmentId::from_bytes([6; 16]),
        space_id: SpaceId::from_bytes([2; 16]),
        name: format!("{MARKER}.png"),
        mime: format!("image/{MARKER}"),
        size: 4_194_305,
        chunk_count: 2,
        created_at: 10,
        unreferenced_since: Some(20),
    };
    let (page, block) = (PageId::from_bytes([3; 16]), BlockId::from_bytes([5; 16]));

    let mut db = open_encrypted(&path, &key);
    db.transaction(|tx| {
        tx.attachment_put(&row)?;
        tx.attachment_refs_replace(page, &[(block, row.attachment_id)])
    })
    .unwrap();

    let leaks = |dir: &Path| {
        files_in(dir)
            .iter()
            .any(|file| contains(&fs::read(file).unwrap(), MARKER.as_bytes()))
    };
    assert!(!leaks(dir.path()), "plaintext in an open database's files");
    drop(db);
    assert!(!leaks(dir.path()), "plaintext in a closed database's files");

    let db = open_encrypted(&path, &key);
    assert_eq!(db.attachments_all().unwrap(), std::slice::from_ref(&row));
    assert_eq!(
        raw_attachment_refs(db.connection()),
        [(row.attachment_id, page, block)]
    );
}

#[test]
fn the_log_and_the_keyring_survive_a_reopen_and_leave_no_plaintext_in_an_encrypted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let log_row = |seq: u8| DeviceLogRow {
        seq: u64::from(seq),
        entry: format!("{MARKER}-entry-{seq}").into_bytes(),
        hash: [seq + 1; 32],
    };
    let key_row = KeyringRow {
        space_id: SpaceId::from_bytes([2; 16]),
        subject: format!("{MARKER}-subject").into_bytes(),
        record: format!("{MARKER}-record").into_bytes(),
        tombstone: false,
    };
    let blob = format!("{MARKER}-blob");

    let mut db = open_encrypted(&path, &key);
    db.transaction(|tx| {
        tx.device_log_append(&log_row(0))?;
        tx.device_log_append(&log_row(1))?;
        tx.keyring_put(&key_row)?;
        tx.meta_set("recovery_blob", &blob)
    })
    .unwrap();

    let leaks = |dir: &Path| {
        files_in(dir)
            .iter()
            .any(|file| contains(&fs::read(file).unwrap(), MARKER.as_bytes()))
    };
    assert!(!leaks(dir.path()), "plaintext in an open database's files");
    drop(db);
    assert!(!leaks(dir.path()), "plaintext in a closed database's files");

    let db = open_encrypted(&path, &key);
    assert_eq!(db.device_log_all().unwrap(), [log_row(0), log_row(1)]);
    assert_eq!(db.keyring_all().unwrap(), [key_row]);
    assert_eq!(
        db.meta_get("recovery_blob").unwrap().as_deref(),
        Some(blob.as_str())
    );
}

#[test]
fn two_connections_racing_to_append_the_same_log_entry_have_exactly_one_winner_each_time() {
    // The guard and the insert are one statement: of two writers that hold the same
    // sequence number, one wins and the other is told so, never both and never a gap.
    const ROUNDS: u64 = 25;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    drop(open_encrypted(&path, &key));

    let barrier = Barrier::new(2);
    let outcomes: Vec<Vec<Result<(), StorageError>>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..2u8)
            .map(|worker| {
                let (barrier, path, key) = (&barrier, &path, &key);
                scope.spawn(move || {
                    let db = open_encrypted(path, key);
                    (0..ROUNDS)
                        .map(|seq| {
                            // Both writers start the round together, with the same number.
                            barrier.wait();
                            db.device_log_append(&DeviceLogRow {
                                seq,
                                entry: vec![worker; 3],
                                hash: [worker; 32],
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });

    assert_eq!(outcomes[0].len(), usize::try_from(ROUNDS).unwrap());
    assert_eq!(outcomes[1].len(), outcomes[0].len());
    for (round, (first, second)) in outcomes[0].iter().zip(&outcomes[1]).enumerate() {
        let results = [first, second];
        assert_eq!(
            results.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "round {round}: {results:?}"
        );
        let loser = results.iter().find(|outcome| outcome.is_err()).unwrap();
        assert!(
            matches!(loser, Err(StorageError::InvalidData(_))),
            "round {round}: {loser:?}"
        );
    }
    let seqs: Vec<u64> = open_encrypted(&path, &key)
        .device_log_all()
        .unwrap()
        .iter()
        .map(|row| row.seq)
        .collect();
    assert_eq!(
        seqs,
        (0..ROUNDS).collect::<Vec<u64>>(),
        "no gap, no duplicate"
    );
}

// --- schema v6: the sync state -----------------------------------------------------------

// The ids and stand-ins of `fixtures/db/v6.db`, as written by `examples/make_fixture_db.rs`
// (the helpers below repeat its formulas).
/// This device: the one of the `meta` rows of every fixture.
const V6_OWN_DEVICE: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
/// A second device of the account.
const V6_PEER_DEVICE: &str = "de71ce00-0000-4000-8000-000000000002";
/// A page whose structure document this device does not have; only a pending entry names it.
const V6_PAGE_UNKNOWN: &str = "9a6e0000-0000-4000-8000-000000000008";
const V6_SERVER_URL: &str = "https://sync.example.test:8787";

fn device_id(text: &str) -> DeviceId {
    text.parse().unwrap()
}

fn doc_id(text: &str) -> DocId {
    text.parse().unwrap()
}

/// The stand-in for a signed head: `KHD1`, the device number, NUL and high bytes, a filler.
fn v6_signed_head(device: u8, cursor: u8) -> Vec<u8> {
    let mut head = b"KHD1".to_vec();
    head.extend([device, 0x00, 0xFF, 0x80]);
    head.extend(ramp(0xC0 + cursor, 40));
    head
}

/// The stand-in for a pending entry: `KEN1`, the cursor, NUL and high bytes, a filler.
fn v6_pending_entry(cursor: u8) -> Vec<u8> {
    let mut entry = b"KEN1".to_vec();
    entry.extend([cursor, 0x00, 0xFF, 0x80]);
    entry.extend(ramp(0x20 + cursor, 48 + usize::from(cursor)));
    entry
}

/// The seven `sync_doc_state` rows of the fixture, in the order of their document ids.
fn v6_doc_states() -> [SyncDocState; 7] {
    let state = |doc: DocId,
                 (pushed, next): (i64, u64),
                 snapshot_covers: Option<u64>,
                 (entries, bytes): (u64, u64),
                 remote_snapshot_covers: Option<u64>| SyncDocState {
        doc_id: doc,
        pushed_up_to_seq: pushed,
        next_device_seq: next,
        snapshot_covers,
        entries_since_snapshot: entries,
        bytes_since_snapshot: bytes,
        remote_snapshot_covers,
    };
    [
        // The keyring: entries pushed (device sequence 3 is next), never a snapshot.
        state(KEYRING_DOC_ID, (0, 3), None, (0, 0), None),
        // The home page crossed the 4 MiB threshold without a remote snapshot.
        state(doc_id(V2_PAGE_HOME), (2, 7), None, (6, 4_194_304), None),
        state(doc_id(V2_PAGE_NOTES), (4, 9), Some(41), (8, 777), Some(41)),
        // The roadmap as a new server epoch leaves a document.
        state(doc_id(V2_PAGE_ROADMAP), (0, 5), Some(33), (0, 0), None),
        state(doc_id(V2_DOC_ACCOUNT), (2, 3), Some(12), (1, 111), Some(12)),
        state(
            doc_id(V2_DOC_PERSONAL),
            (3, 4),
            Some(30),
            (5, 2_048),
            Some(28),
        ),
        // Never pushed.
        state(doc_id(V2_DOC_WORK), (0, 1), None, (0, 0), None),
    ]
}

/// The five `sync_seen` rows of the fixture as `(document, device, last_seq)`.
fn v6_seen() -> [(DocId, DeviceId, u64); 5] {
    let (own, peer) = (device_id(V6_OWN_DEVICE), device_id(V6_PEER_DEVICE));
    [
        (doc_id(V2_PAGE_NOTES), peer, 3),
        (doc_id(V2_DOC_ACCOUNT), own, 2),
        (KEYRING_DOC_ID, peer, 1),
        (doc_id(V2_DOC_PERSONAL), peer, 12),
        (doc_id(V2_DOC_ACCOUNT), peer, 7),
    ]
}

/// The two `sync_heads` rows of the fixture, in the order of the device ids.
fn v6_heads() -> [SyncHeadRow; 2] {
    [
        SyncHeadRow {
            device_id: device_id(V6_OWN_DEVICE),
            cursor: 42,
            chain_hash: ramp(0x70, 32).try_into().unwrap(),
            at_ms: 1_760_000_500_000,
            signed: v6_signed_head(1, 42),
            seen_at: 1_760_000_500_500,
        },
        SyncHeadRow {
            device_id: device_id(V6_PEER_DEVICE),
            cursor: 40,
            chain_hash: ramp(0x90, 32).try_into().unwrap(),
            at_ms: 1_760_000_490_000,
            signed: v6_signed_head(2, 40),
            seen_at: 1_760_000_511_000,
        },
    ]
}

/// The three `sync_pending` rows of the fixture, in cursor order.
fn v6_pending() -> [SyncPendingRow; 3] {
    let row = |cursor: u8, doc: &str, reason: u8, received_at: i64| SyncPendingRow {
        cursor: u64::from(cursor),
        doc_id: doc_id(doc),
        bytes: v6_pending_entry(cursor),
        reason,
        received_at,
    };
    [
        row(17, V2_PAGE_OLD, 1, 1_760_000_511_500),
        row(19, V6_PAGE_UNKNOWN, 2, 1_760_000_511_800),
        row(23, V2_PAGE_OLD, 1, 1_760_000_512_000),
    ]
}

/// The eight `sync_config` pairs of the fixture, in key order.
fn v6_config() -> Vec<(String, String)> {
    vec![
        pair("chain_hash", &hex(&ramp(0x70, 32))),
        pair("enabled", "1"),
        pair("fork_detected", "0"),
        pair("last_pull_at", "1760000510000"),
        pair("last_push_at", "1760000500000"),
        pair("remote_cursor", "42"),
        pair("server_epoch", &hex(&ramp(0x60, 16))),
        pair("server_url", V6_SERVER_URL),
    ]
}

/// How many rows of `table` satisfy `condition`.
fn count_where(conn: &Connection, table: &str, condition: &str) -> i64 {
    conn.query_row(
        &format!("SELECT count(*) FROM {table} WHERE {condition}"),
        [],
        |row| row.get(0),
    )
    .unwrap()
}

/// How many columns called `published` `table` has (0 or 1).
fn published_columns(conn: &Connection, table: &str) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM pragma_table_info(?1) WHERE name = 'published'",
        [table],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn migrating_the_v5_fixture_keeps_every_row_of_every_table_and_adds_the_empty_sync_tables() {
    let (_dir, copy) = copy_of(FIXTURE_V5, "v5.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "5", "a schema v5 file");
        assert_eq!(published_columns(&conn, "keyring"), 0);
        assert_eq!(published_columns(&conn, "device_log"), 0);
        dump_all(&conn)
    };
    // The dump is not empty-handed: the tables of the fixture hold what it was built with.
    for (table, rows) in [
        ("meta", 8),
        ("workspace", 1),
        ("settings", 2),
        ("docs", 7),
        ("doc_snapshots", 2),
        ("doc_updates", 12),
        ("pages_index", 5),
        ("links", 6),
        ("fts_blocks", 14),
        ("device_log", 3),
        ("keyring", 8),
        ("attachments_index", 6),
        ("attachment_refs", 6),
    ] {
        assert_eq!(
            rows_of(&before, table).len(),
            rows,
            "{table} in the v5 file"
        );
    }
    let old_tables: Vec<&str> = before.iter().map(|(name, _)| name.as_str()).collect();
    for table in SYNC_TABLES {
        assert!(!old_tables.contains(&table), "{table} is new in v6");
    }

    let mut db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());

    // Every table of v5 has exactly the rows it had (the log and the keyring with the 0 of
    // their new column, `fts_blocks` with the 0 of its `kind`); the five tables of v6, the one of
    // v8, the one of v9 and the one of v10 are new, empty.
    let after_rows = without_properties(after.clone());
    for (table, rows) in &before {
        assert_eq!(
            rows_of(&after_rows, table),
            after_v6(table, rows).as_slice(),
            "{table} is unchanged"
        );
    }
    let new_tables: Vec<&str> = after
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !old_tables.contains(name))
        .collect();
    assert_eq!(
        new_tables,
        [
            "chunk_sync",
            "date_mentions",
            "page_properties",
            "sync_config",
            "sync_doc_state",
            "sync_heads",
            "sync_pending",
            "sync_seen"
        ]
    );
    for table in SYNC_TABLES {
        assert_eq!(count_rows(db.connection(), table), 0, "{table}");
    }
    assert_eq!(count_rows(db.connection(), "chunk_sync"), 0);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 0);
    assert_eq!(count_rows(db.connection(), "page_properties"), 0);
    assert!(db.sync_config_all().unwrap().is_empty());
    assert!(db.sync_doc_state_all().unwrap().is_empty());
    assert!(db.sync_heads_all().unwrap().is_empty());
    assert!(db.sync_pending_all().unwrap().is_empty());
    assert_eq!(db.sync_pending_count().unwrap(), 0);
    assert_eq!(
        db.sync_seen_get(doc_id(V2_PAGE_HOME), device_id(V6_OWN_DEVICE))
            .unwrap(),
        None
    );

    // `published` is 0 in every row that existed: the first sync publishes everything.
    let conn = db.connection();
    assert_eq!(published_columns(conn, "keyring"), 1);
    assert_eq!(published_columns(conn, "device_log"), 1);
    assert_eq!(count_where(conn, "keyring", "published = 0"), 8);
    assert_eq!(count_where(conn, "device_log", "published = 0"), 3);
    assert_eq!(count_where(conn, "keyring", "published <> 0"), 0);
    assert_eq!(count_where(conn, "device_log", "published <> 0"), 0);
    assert_eq!(
        db.keyring_unpublished().unwrap(),
        db.keyring_all().unwrap(),
        "every keyring row is unpublished"
    );
    assert_eq!(
        db.device_log_unpublished().unwrap(),
        db.device_log_all().unwrap(),
        "every log entry is unpublished"
    );
    assert_eq!(db.keyring_all().unwrap().len(), 8);
    assert_eq!(db.device_log_all().unwrap().len(), 3);

    // The indexes and the tables of v3 to v5 still work on the migrated file.
    db.connection()
        .execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    assert_eq!(found(&db, "notes").len(), 5);
    assert_eq!(db.attachments_all().unwrap().len(), 6);

    // The migrated file takes sync rows and published markers, and still has them after a
    // reopen.
    let (log_before, keys_before) = (db.device_log_all().unwrap(), db.keyring_all().unwrap());
    let (states, heads, pending) = (v6_doc_states(), v6_heads(), v6_pending());
    db.transaction(|tx| {
        tx.sync_config_set("server_url", V6_SERVER_URL)?;
        for state in &states {
            tx.sync_doc_state_put(state)?;
        }
        for (doc, device, last_seq) in v6_seen() {
            tx.sync_seen_put(doc, device, last_seq)?;
        }
        for head in &heads {
            tx.sync_heads_put(head)?;
        }
        for row in &pending {
            tx.sync_pending_put(row)?;
        }
        tx.keyring_mark_published(&tx.keyring_all()?)?;
        tx.device_log_mark_published(1)
    })
    .unwrap();
    drop(db);

    let db = open_plain(&copy);
    assert_eq!(
        db.sync_config_all().unwrap(),
        [pair("server_url", V6_SERVER_URL)]
    );
    assert_eq!(db.sync_doc_state_all().unwrap(), states);
    assert_eq!(db.sync_heads_all().unwrap(), heads);
    assert_eq!(db.sync_pending_all().unwrap(), pending);
    for (doc, device, last_seq) in v6_seen() {
        assert_eq!(db.sync_seen_get(doc, device).unwrap(), Some(last_seq));
    }
    assert!(db.keyring_unpublished().unwrap().is_empty());
    assert_eq!(db.device_log_unpublished().unwrap(), [v4_log_row(2)]);
    // The rows themselves are what they were; only the marker moved.
    assert_eq!(db.device_log_all().unwrap(), log_before);
    assert_eq!(db.keyring_all().unwrap(), keys_before);
    let reopened = without_properties(dump_all(db.connection()));
    for (table, rows) in before
        .iter()
        .filter(|(table, _)| !matches!(table.as_str(), "device_log" | "keyring"))
    {
        assert_eq!(rows_of(&reopened, table), rows.as_slice(), "{table}");
    }
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string(),
        "written to the file"
    );
}

#[test]
fn the_v6_fixture_opens_at_version_6_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V6).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V6, "v6.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "6",
        "the fixture is a schema v6 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    if SCHEMA_VERSION == 6 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v6_fixture_reads_back_through_the_sync_state_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V6, "v6.db");
    let mut db = open_plain(&copy);

    // The settings, in key order, whatever the order they were written in.
    assert_eq!(db.sync_config_all().unwrap(), v6_config());
    for (key, value) in v6_config() {
        assert_eq!(db.sync_config_get(&key).unwrap(), Some(value), "{key}");
    }
    assert_eq!(db.sync_config_get("no_such_key").unwrap(), None);

    // The per-document state, in the order of the ids. The documents with a row exist in
    // `docs` (the keyring's all-zero id does not and has a row all the same), the "Old
    // ideas" page document has no row, and the trashed child has no document at all.
    let states = v6_doc_states();
    assert_eq!(db.sync_doc_state_all().unwrap(), states);
    for state in &states {
        assert_eq!(
            db.sync_doc_state_get(state.doc_id).unwrap().as_ref(),
            Some(state)
        );
        assert_eq!(
            db.doc_get(state.doc_id).unwrap().is_some(),
            state.doc_id != KEYRING_DOC_ID,
            "{}",
            state.doc_id
        );
    }
    for absent in [V2_PAGE_OLD, V2_PAGE_OLD_CHILD] {
        assert_eq!(db.sync_doc_state_get(doc_id(absent)).unwrap(), None);
    }

    // Continuity per (document, device).
    for (doc, device, last_seq) in v6_seen() {
        assert_eq!(db.sync_seen_get(doc, device).unwrap(), Some(last_seq));
    }
    assert_eq!(
        db.sync_seen_get(doc_id(V2_PAGE_HOME), device_id(V6_PEER_DEVICE))
            .unwrap(),
        None
    );
    assert_eq!(
        db.sync_seen_get(doc_id(V2_PAGE_NOTES), device_id(V6_OWN_DEVICE))
            .unwrap(),
        None
    );
    assert_eq!(count_rows(db.connection(), "sync_seen"), 5);

    // The heads, in the order of the device ids; this device's own chain hash is the one
    // of the settings.
    let heads = v6_heads();
    assert_eq!(db.sync_heads_all().unwrap(), heads);
    assert_eq!(
        hex(&heads[0].chain_hash),
        db.sync_config_get("chain_hash").unwrap().unwrap()
    );
    assert_eq!(
        heads[0].cursor.to_string(),
        db.sync_config_get("remote_cursor").unwrap().unwrap()
    );

    // The pending entries, in cursor order though written out of it.
    let pending = v6_pending();
    assert_eq!(db.sync_pending_all().unwrap(), pending);
    assert_eq!(db.sync_pending_count().unwrap(), 3);

    // Taking the entries of a document returns them in cursor order and deletes them, and
    // only them.
    assert_eq!(
        db.sync_pending_take_for_doc(doc_id(V2_PAGE_OLD)).unwrap(),
        [pending[0].clone(), pending[2].clone()]
    );
    assert_eq!(db.sync_pending_all().unwrap(), [pending[1].clone()]);
    assert!(
        db.sync_pending_take_for_doc(doc_id(V2_PAGE_OLD))
            .unwrap()
            .is_empty(),
        "taken once"
    );
    db.sync_pending_delete(19, doc_id(V6_PAGE_UNKNOWN)).unwrap();
    assert_eq!(db.sync_pending_count().unwrap(), 0);

    // A new server epoch: everything about the old server goes in one transaction, the
    // device's own sequence and its old snapshot coverage stay.
    db.transaction(|tx| {
        tx.sync_doc_state_reset_all()?;
        tx.sync_seen_clear()?;
        tx.sync_heads_clear()?;
        tx.sync_pending_clear()?;
        tx.keyring_mark_all_unpublished()?;
        tx.device_log_mark_all_unpublished()?;
        tx.sync_config_delete("chain_hash")?;
        tx.sync_config_delete("remote_cursor")?;
        tx.sync_config_set("server_epoch", &hex(&ramp(0x61, 16)))
    })
    .unwrap();
    let reset: Vec<SyncDocState> = states
        .iter()
        .map(|state| SyncDocState {
            pushed_up_to_seq: 0,
            entries_since_snapshot: 0,
            bytes_since_snapshot: 0,
            remote_snapshot_covers: None,
            ..state.clone()
        })
        .collect();
    assert_eq!(db.sync_doc_state_all().unwrap(), reset);
    assert_eq!(count_rows(db.connection(), "sync_seen"), 0);
    assert!(db.sync_heads_all().unwrap().is_empty());
    assert_eq!(count_rows(db.connection(), "sync_pending"), 0);
    assert_eq!(db.keyring_unpublished().unwrap().len(), 8);
    assert_eq!(db.device_log_unpublished().unwrap().len(), 3);
    assert_eq!(
        db.sync_config_all().unwrap().len(),
        8 - 2,
        "two keys deleted, one rewritten"
    );
    assert_eq!(
        db.sync_config_get("server_epoch").unwrap(),
        Some(hex(&ramp(0x61, 16)))
    );
    drop(db);

    // All of it is in the file.
    let db = open_plain(&copy);
    assert_eq!(db.sync_doc_state_all().unwrap(), reset);
    assert_eq!(db.keyring_unpublished().unwrap().len(), 8);
}

#[test]
fn the_v6_fixture_reads_back_through_the_published_markers_and_the_outbox() {
    let (_dir, copy) = copy_of(FIXTURE_V6, "v6.db");
    let db = open_plain(&copy);
    let (personal, work) = (space_id(V2_PERSONAL), space_id(V2_WORK));

    // The keyring: the work space's three rows and the personal space key were sent, the
    // four others were not (the rows come back in the order of space and subject).
    let all = db.keyring_all().unwrap();
    assert_eq!(all.len(), 8);
    let unpublished = vec![
        v4_key_row(personal, v4_subject_doc(V2_PAGE_HOME, 0), 1, 0x11),
        v4_key_row(personal, v4_subject_doc(V2_PAGE_NOTES, 0), 1, 0x12),
        v4_key_row(personal, v4_subject_doc(V4_PAGE_PURGED, 0), 2, 0x14),
        v4_key_row(personal, v4_subject_doc(V2_DOC_PERSONAL, 0), 1, 0x15),
    ];
    assert_eq!(db.keyring_unpublished().unwrap(), unpublished);
    assert!(unpublished.iter().all(|row| all.contains(row)));
    assert_eq!(count_where(db.connection(), "keyring", "published = 1"), 4);
    assert!(
        db.keyring_by_space(work)
            .unwrap()
            .iter()
            .all(|row| !unpublished.contains(row)),
        "the work space was sent"
    );

    // A row replaced after it was read stays unpublished when the older copy is marked: a
    // push in flight cannot swallow it.
    let sent = db.keyring_by_space(work).unwrap().remove(0);
    let tombstone = KeyringRow {
        record: v4_key_row(work, sent.subject.clone(), 2, 0x55).record,
        tombstone: true,
        ..sent.clone()
    };
    db.keyring_put(&tombstone).unwrap();
    let mut now_unpublished = db.keyring_unpublished().unwrap();
    assert_eq!(
        now_unpublished.len(),
        5,
        "a replaced row is unpublished again"
    );
    assert!(now_unpublished.contains(&tombstone));
    db.keyring_mark_published(&[sent]).unwrap();
    assert_eq!(db.keyring_unpublished().unwrap(), now_unpublished);
    db.keyring_mark_published(&now_unpublished).unwrap();
    assert!(db.keyring_unpublished().unwrap().is_empty());
    now_unpublished.clear();
    assert_eq!(count_where(db.connection(), "keyring", "published = 1"), 8);
    db.keyring_mark_all_unpublished().unwrap();
    assert_eq!(db.keyring_unpublished().unwrap().len(), 8);
    assert_eq!(db.keyring_all().unwrap().len(), 8);

    // The device log: the Genesis entry and the first device addition were sent.
    assert_eq!(db.device_log_unpublished().unwrap(), [v4_log_row(2)]);
    assert_eq!(
        count_where(db.connection(), "device_log", "published = 1"),
        2
    );
    db.device_log_append(&v4_log_row(3)).unwrap();
    assert_eq!(
        db.device_log_unpublished().unwrap(),
        [v4_log_row(2), v4_log_row(3)],
        "an appended entry starts unpublished"
    );
    db.device_log_mark_published(2).unwrap();
    assert_eq!(db.device_log_unpublished().unwrap(), [v4_log_row(3)]);
    db.device_log_mark_published(3).unwrap();
    assert!(db.device_log_unpublished().unwrap().is_empty());
    db.device_log_mark_all_unpublished().unwrap();
    assert_eq!(db.device_log_unpublished().unwrap().len(), 4);
    assert_eq!(db.device_log_all().unwrap().len(), 4);

    // The outbox: the update log of a document after a sequence number, every origin, in
    // sequence order. The blobs are the generator's `[0xAA, last id byte, seq, 0, 0xFF]`.
    let personal_doc = doc_id(V2_DOC_PERSONAL);
    let update =
        |last_byte: u8, seq: u8, origin| (i64::from(seq), v2_update(last_byte, seq), origin);
    assert_eq!(
        db.updates_from(personal_doc, 0).unwrap(),
        [
            update(0x51, 3, UpdateOrigin::Local),
            update(0x51, 4, UpdateOrigin::Remote),
            update(0x51, 5, UpdateOrigin::Core),
        ],
        "the snapshot covers updates 1 and 2, the log starts at 3"
    );
    assert_eq!(
        db.updates_from(personal_doc, 3).unwrap(),
        [
            update(0x51, 4, UpdateOrigin::Remote),
            update(0x51, 5, UpdateOrigin::Core)
        ],
        "the sequence number is exclusive"
    );
    assert!(db.updates_from(personal_doc, 5).unwrap().is_empty());
    assert_eq!(
        db.updates_from(doc_id(V2_DOC_ACCOUNT), -1).unwrap(),
        [
            update(0x0a, 1, UpdateOrigin::Local),
            update(0x0a, 2, UpdateOrigin::Local)
        ]
    );
    assert_eq!(
        db.updates_from(doc_id(V2_PAGE_HOME), 0).unwrap(),
        [
            update(0x01, 1, UpdateOrigin::Core),
            update(0x01, 2, UpdateOrigin::Local)
        ]
    );
    assert_eq!(
        db.updates_from(doc_id(V2_PAGE_NOTES), 3).unwrap(),
        [update(0x02, 4, UpdateOrigin::Local)]
    );
    assert!(
        db.updates_from(doc_id(V2_PAGE_OLD_CHILD), 0)
            .unwrap()
            .is_empty(),
        "a document without updates"
    );
    assert!(db.updates_from(KEYRING_DOC_ID, 0).unwrap().is_empty());

    // A compaction takes the rows a snapshot covers, and the outbox with them.
    assert_eq!(db.updates_delete_up_to(personal_doc, 3).unwrap(), 1);
    assert_eq!(
        db.updates_from(personal_doc, 0).unwrap(),
        [
            update(0x51, 4, UpdateOrigin::Remote),
            update(0x51, 5, UpdateOrigin::Core)
        ]
    );

    // Appending after the compaction continues the numbering the outbox reads.
    let next = db
        .update_append(personal_doc, b"new", UpdateOrigin::Local, 9)
        .unwrap();
    assert_eq!(next, 6);
    assert_eq!(
        db.updates_from(personal_doc, 5).unwrap(),
        [(6, b"new".to_vec(), UpdateOrigin::Local)]
    );
}

#[test]
fn sync_rows_survive_a_reopen_and_leave_no_plaintext_in_an_encrypted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);
    let url = format!("https://{MARKER}.example.test");
    let head = SyncHeadRow {
        device_id: DeviceId::from_bytes([8; 16]),
        cursor: 10,
        chain_hash: [11; 32],
        at_ms: 12,
        signed: format!("{MARKER}-head").into_bytes(),
        seen_at: 13,
    };
    let entry = SyncPendingRow {
        cursor: 14,
        doc_id: DocId::from_bytes([9; 16]),
        bytes: format!("{MARKER}-entry").into_bytes(),
        reason: 2,
        received_at: 15,
    };
    let key_row = KeyringRow {
        space_id: SpaceId::from_bytes([2; 16]),
        subject: format!("{MARKER}-subject").into_bytes(),
        record: format!("{MARKER}-record").into_bytes(),
        tombstone: false,
    };

    let mut db = open_encrypted(&path, &key);
    db.transaction(|tx| {
        tx.sync_config_set("server_url", &url)?;
        tx.sync_heads_put(&head)?;
        tx.sync_pending_put(&entry)?;
        tx.keyring_put(&key_row)?;
        tx.keyring_mark_published(std::slice::from_ref(&key_row))
    })
    .unwrap();

    let leaks = |dir: &Path| {
        files_in(dir)
            .iter()
            .any(|file| contains(&fs::read(file).unwrap(), MARKER.as_bytes()))
    };
    assert!(!leaks(dir.path()), "plaintext in an open database's files");
    drop(db);
    assert!(!leaks(dir.path()), "plaintext in a closed database's files");

    let db = open_encrypted(&path, &key);
    assert_eq!(
        db.sync_config_get("server_url").unwrap().as_deref(),
        Some(url.as_str())
    );
    assert_eq!(db.sync_heads_all().unwrap(), [head]);
    assert_eq!(db.sync_pending_all().unwrap(), [entry]);
    assert_eq!(db.keyring_all().unwrap(), [key_row]);
    assert!(db.keyring_unpublished().unwrap().is_empty());
}

// --- rekey ------------------------------------------------------------------------------

/// The attachment `fill_everything` writes (its name and MIME type carry the marker).
fn fill_attachment(space_id: SpaceId) -> AttachmentRow {
    AttachmentRow {
        attachment_id: AttachmentId::from_bytes([6; 16]),
        space_id,
        name: format!("{MARKER}.png"),
        mime: format!("image/{MARKER}"),
        size: 4_194_305,
        chunk_count: 2,
        created_at: 4,
        unreferenced_since: Some(5),
    }
}

/// Writes a recognisable row into every table (the workspace, a setting and the device
/// identity by `fill`), with the marker in the content that must never be readable on
/// disk.
fn fill_everything(db: &mut Db) {
    fill(db);
    let space = SpaceId::from_bytes([2; 16]);
    let doc_id = DocId::from_bytes([1; 16]);
    let (page, linked) = (PageId::from_bytes([3; 16]), PageId::from_bytes([4; 16]));
    let block = BlockId::from_bytes([5; 16]);
    db.transaction(|tx| {
        tx.doc_insert(&structure_doc(doc_id, space))?;
        tx.snapshot_put(doc_id, MARKER.as_bytes(), &[0x00, 0xFF, 0x80], 2)?;
        tx.update_append(doc_id, MARKER.as_bytes(), UpdateOrigin::Local, 3)?;
        tx.pages_index_upsert(&[index_row(3, space, MARKER)])?;
        tx.fts_replace_page(
            page,
            MARKER,
            &[FtsBlock {
                block_id: block,
                text: format!("{MARKER} words"),
            }],
        )?;
        tx.links_replace_from(
            page,
            &[LinkRow {
                from_block: block,
                to_page: linked,
                to_block: None,
            }],
        )?;
        tx.device_log_append(&v4_log_row(0))?;
        tx.device_log_append(&v4_log_row(1))?;
        let sent = v4_key_row(space, v4_subject_space(0), 1, 0x10);
        tx.keyring_put(&sent)?;
        tx.keyring_put(&v4_key_row(
            space,
            v4_subject_doc(V4_PAGE_PURGED, 0),
            2,
            0x11,
        ))?;
        tx.meta_set("account_id", V4_ACCOUNT_ID)?;
        tx.attachment_put(&fill_attachment(space))?;
        tx.attachment_refs_replace(page, &[(block, fill_attachment(space).attachment_id)])?;
        // The sync state: the marker is in the server URL and in the opaque blobs.
        let peer = DeviceId::from_bytes([8; 16]);
        tx.sync_config_set("server_url", &format!("https://{MARKER}.example.test"))?;
        tx.sync_doc_state_put(&fill_sync_doc_state(doc_id))?;
        tx.sync_seen_put(doc_id, peer, 9)?;
        tx.sync_heads_put(&fill_sync_head(peer))?;
        tx.sync_pending_put(&fill_sync_pending())?;
        tx.chunk_sync_put(&fill_chunk_sync())?;
        tx.date_mentions_replace(page, &[fill_date_mention(block)])?;
        tx.page_properties_replace(space, page, &[fill_page_property()])?;
        tx.fts_replace_properties(page, &[fill_property_text()])?;
        tx.keyring_mark_published(std::slice::from_ref(&sent))?;
        tx.device_log_mark_published(0)
    })
    .unwrap();
}

/// The `sync_doc_state` row `fill_everything` writes: every number differs from the others.
fn fill_sync_doc_state(doc_id: DocId) -> SyncDocState {
    SyncDocState {
        doc_id,
        pushed_up_to_seq: 3,
        next_device_seq: 4,
        snapshot_covers: Some(5),
        entries_since_snapshot: 6,
        bytes_since_snapshot: 7,
        remote_snapshot_covers: Some(8),
    }
}

/// The `sync_heads` row `fill_everything` writes (the signed head carries the marker).
fn fill_sync_head(device_id: DeviceId) -> SyncHeadRow {
    SyncHeadRow {
        device_id,
        cursor: 10,
        chain_hash: [11; 32],
        at_ms: 12,
        signed: format!("{MARKER}-head").into_bytes(),
        seen_at: 13,
    }
}

/// The `sync_pending` row `fill_everything` writes (the entry carries the marker).
fn fill_sync_pending() -> SyncPendingRow {
    SyncPendingRow {
        cursor: 14,
        doc_id: DocId::from_bytes([9; 16]),
        bytes: format!("{MARKER}-entry").into_bytes(),
        reason: 1,
        received_at: 15,
    }
}

/// The `chunk_sync` row `fill_everything` writes.
fn fill_chunk_sync() -> ChunkSyncRow {
    ChunkSyncRow {
        hash: ChunkId::from_bytes([16; 32]),
        attachment_id: fill_attachment(SpaceId::from_bytes([2; 16])).attachment_id,
        state: 2,
        attempts: 17,
        next_at: Some(18),
    }
}

/// The `date_mentions` row `fill_everything` writes (the table stores any text as the date,
/// so the marker goes there: no part of the table may be readable on disk).
fn fill_date_mention(block: BlockId) -> DateMentionRow {
    DateMentionRow {
        block_id: block,
        node_id: BlockId::from_bytes([19; 16]),
        date: MARKER.to_owned(),
        time: Some("04:05".to_owned()),
        reminder_minutes: Some(20),
        fire_at_local: Some(21),
        reminder_for: None,
    }
}

/// The `page_properties` row `fill_everything` writes (the marker is its text value).
fn fill_page_property() -> PagePropertyRow {
    PagePropertyRow {
        property_id: PropertyId::from_bytes([22; 16]),
        kind: PropertyKind::Text,
        value_text: Some(MARKER.to_owned()),
        value_number: None,
        value_bool: None,
        value_date: None,
        value_time: None,
        option_id: None,
        attachment_id: None,
        value_total: None,
        related_page_id: None,
        account_id: None,
    }
}

/// The property row of the text index `fill_everything` writes: the property's id is the
/// block, the marker is in the text.
fn fill_property_text() -> FtsBlock {
    FtsBlock {
        block_id: BlockId::from_bytes([22; 16]),
        text: format!("{MARKER} property"),
    }
}

/// Number of pages that fail SQLCipher's own integrity check (none in a sound file).
fn cipher_failures(conn: &Connection) -> usize {
    conn.prepare("PRAGMA cipher_integrity_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count()
}

/// Size of the write-ahead log of the database at `path` (0 when it has none).
fn wal_len(path: &Path) -> u64 {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    fs::metadata(PathBuf::from(name)).map_or(0, |meta| meta.len())
}

/// `true` if any file of the database in `dir` holds one of `needles`.
fn any_file_holds(dir: &Path, needles: &[&[u8]]) -> bool {
    files_in(dir).iter().any(|file| {
        let bytes = fs::read(file).unwrap();
        needles.iter().any(|needle| contains(&bytes, needle))
    })
}

fn is_busy(err: &StorageError) -> bool {
    matches!(
        err,
        StorageError::Sqlite(kb_storage::rusqlite::Error::SqliteFailure(failure, _))
            if failure.code == ErrorCode::DatabaseBusy
    )
}

#[test]
fn rekey_re_encrypts_a_real_sqlcipher_file_and_keeps_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let (key_a, key_b) = (db_key(1), db_key(2));
    assert_ne!(key_hex(&key_a), key_hex(&key_b));

    let mut db = open_encrypted(&path, &key_a);
    fill_everything(&mut db);
    let before = dump_all(db.connection());
    assert!(rows_of(&before, "device_log").len() == 2 && rows_of(&before, "keyring").len() == 2);
    assert!(
        rows_of(&before, "attachments_index").len() == 1
            && rows_of(&before, "attachment_refs").len() == 1
    );
    for table in SYNC_TABLES {
        assert_eq!(rows_of(&before, table).len(), 1, "{table}");
    }

    db.rekey(&key_b).unwrap();

    // The same connection carries on with the same rows, and keeps writing.
    assert_eq!(dump_all(db.connection()), before);
    db.transaction(|tx| {
        tx.setting_put("ui.after", &json!("written after the rekey"), 9)?;
        tx.device_log_append(&v4_log_row(2))
    })
    .unwrap();
    assert_eq!(cipher_failures(db.connection()), 0);
    drop(db);

    // The new key opens the file and reads everything, the index included.
    let db = open_encrypted(&path, &key_b);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_filled(&db);
    let after = dump_all(db.connection());
    for (table, rows) in &before {
        match table.as_str() {
            "settings" | "device_log" => assert_eq!(
                rows_of(&after, table).len(),
                rows.len() + 1,
                "{table} has the row written after the rekey"
            ),
            _ => assert_eq!(rows_of(&after, table), rows.as_slice(), "{table}"),
        }
    }
    assert_eq!(
        db.setting_get("ui.after").unwrap(),
        Some(json!("written after the rekey"))
    );
    assert_eq!(db.device_log_all().unwrap().len(), 3);
    assert_eq!(
        db.attachments_all().unwrap(),
        [fill_attachment(SpaceId::from_bytes([2; 16]))]
    );
    // The sync state and the published markers came through the rekey.
    assert_eq!(
        db.sync_config_get("server_url").unwrap().as_deref(),
        Some(format!("https://{MARKER}.example.test").as_str())
    );
    assert_eq!(
        db.sync_doc_state_all().unwrap(),
        [fill_sync_doc_state(DocId::from_bytes([1; 16]))]
    );
    assert_eq!(
        db.sync_seen_get(DocId::from_bytes([1; 16]), DeviceId::from_bytes([8; 16]))
            .unwrap(),
        Some(9)
    );
    assert_eq!(
        db.sync_heads_all().unwrap(),
        [fill_sync_head(DeviceId::from_bytes([8; 16]))]
    );
    assert_eq!(db.sync_pending_all().unwrap(), [fill_sync_pending()]);
    let chunk = fill_chunk_sync();
    assert_eq!(db.chunk_sync_get(chunk.hash).unwrap(), Some(chunk.clone()));
    assert_eq!(db.chunk_sync_count_waiting().unwrap(), 1);
    assert_eq!(
        db.date_mentions_of(PageId::from_bytes([3; 16])).unwrap(),
        [fill_date_mention(BlockId::from_bytes([5; 16]))]
    );
    assert_eq!(
        db.page_properties_of(PageId::from_bytes([3; 16])).unwrap(),
        [fill_page_property()]
    );
    assert_eq!(db.keyring_unpublished().unwrap().len(), 1);
    // Entry 0 was marked, entry 1 was not and entry 2 was appended after the rekey.
    assert_eq!(
        db.device_log_unpublished().unwrap(),
        [v4_log_row(1), v4_log_row(2)]
    );
    db.connection()
        .execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    assert_eq!(
        found(&db, "3f9a7c1e"),
        sorted(vec![
            (PageId::from_bytes([3; 16]), None),
            (
                PageId::from_bytes([3; 16]),
                Some(BlockId::from_bytes([5; 16]))
            ),
            (
                PageId::from_bytes([3; 16]),
                Some(BlockId::from_bytes([22; 16]))
            )
        ]),
        "the property row of the text index is found by the marker, like a block"
    );
    assert_eq!(cipher_failures(db.connection()), 0);
    drop(db);

    // The old key is refused, with no trace of either key in the error.
    let err = sqlcipher::open_file(&path, &key_a).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    let message = err.to_string();
    assert!(!message.contains(&key_hex(&key_a)), "{message}");
    assert!(!message.contains(&key_hex(&key_b)), "{message}");
}

#[test]
fn rekey_leaves_nothing_under_the_old_key_in_the_main_file_before_the_connection_closes() {
    // In WAL mode a rekey is written to the log. Unless it is checkpointed, the main file
    // still holds every page as the old key encrypted it, and the old key opens a copy.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let (key_a, key_b) = (db_key(1), db_key(2));
    let mut db = open_encrypted(&path, &key_a);
    fill_everything(&mut db);

    // All of it goes into the main file under the old key, and a little more stays in the
    // log.
    db.connection()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    db.setting_set("ui.late", &json!("only in the log"), 5)
        .unwrap();
    assert!(wal_len(&path) > 0, "the late row is in the log");

    db.rekey(&key_b).unwrap();
    assert_eq!(wal_len(&path), 0, "the log is in the main file");

    // A copy of the main file alone (no -wal, no -shm) opens with the new key only.
    let copies = tempfile::tempdir().unwrap();
    let copy = copies.path().join("main-only.db");
    fs::copy(&path, &copy).unwrap();
    let err = sqlcipher::open_file(&copy, &key_a).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    let copied = open_encrypted(&copy, &key_b);
    assert_filled(&copied);
    assert_eq!(
        copied.setting_get("ui.late").unwrap(),
        Some(json!("only in the log"))
    );
}

#[test]
fn rekeying_again_and_again_on_one_connection_keeps_only_the_last_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let (key_a, key_b, key_c) = (db_key(1), db_key(2), db_key(3));
    let mut db = open_encrypted(&path, &key_a);
    fill_everything(&mut db);
    let before = dump_all(db.connection());

    db.rekey(&key_b).unwrap();
    db.setting_set("ui.between", &json!(1), 1).unwrap();
    db.rekey(&key_c).unwrap();
    db.rekey(&key_c).unwrap();
    drop(db);

    for old in [&key_a, &key_b] {
        let err = sqlcipher::open_file(&path, old).unwrap_err();
        assert!(matches!(err, StorageError::WrongKey), "{err}");
    }
    let db = open_encrypted(&path, &key_c);
    assert_filled(&db);
    assert_eq!(db.setting_get("ui.between").unwrap(), Some(json!(1)));
    let after = dump_all(db.connection());
    for (table, rows) in before.iter().filter(|(table, _)| table != "settings") {
        assert_eq!(rows_of(&after, table), rows.as_slice(), "{table}");
    }
    assert_eq!(cipher_failures(db.connection()), 0);
}

#[test]
fn rekey_while_another_connection_holds_the_write_lock_fails_and_changes_nothing() {
    // SQLCipher's own `PRAGMA rekey` reports success in this case and then writes pages
    // under the new key at the next write. `Db::rekey` must say no, before the pragma.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let (key_a, key_b) = (db_key(1), db_key(2));
    let mut db = open_encrypted(&path, &key_a);
    fill_everything(&mut db);
    let other = open_encrypted(&path, &key_a);
    db.connection().busy_timeout(Duration::ZERO).unwrap();
    other.connection().execute_batch("BEGIN IMMEDIATE").unwrap();

    let err = db.rekey(&key_b).unwrap_err();
    assert!(is_busy(&err), "{err}");
    let message = err.to_string();
    assert!(!message.contains(&key_hex(&key_a)), "{message}");
    assert!(!message.contains(&key_hex(&key_b)), "{message}");
    other.connection().execute_batch("ROLLBACK").unwrap();

    // The pragma never ran: this connection is as good as before, under the old key.
    db.setting_set("ui.after", &json!("still the old key"), 9)
        .unwrap();
    assert_eq!(
        other.setting_get("ui.after").unwrap(),
        Some(json!("still the old key"))
    );
    assert_eq!(cipher_failures(other.connection()), 0);
    drop(other);

    // With the lock free (and no other connection left), a second try goes through.
    db.rekey(&key_b).unwrap();
    drop(db);
    let err = sqlcipher::open_file(&path, &key_a).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    let db = open_encrypted(&path, &key_b);
    assert_filled(&db);
    assert_eq!(
        db.setting_get("ui.after").unwrap(),
        Some(json!("still the old key"))
    );
}

#[test]
fn a_rekey_that_fails_on_a_read_only_connection_leaves_the_connection_and_the_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let (key_a, key_b) = (db_key(1), db_key(2));
    let mut db = open_encrypted(&path, &key_a);
    fill_everything(&mut db);

    db.connection()
        .execute_batch("PRAGMA query_only = ON")
        .unwrap();
    let err = db.rekey(&key_b).unwrap_err();
    assert!(
        matches!(
            &err,
            StorageError::Sqlite(kb_storage::rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == ErrorCode::ReadOnly
        ),
        "{err}"
    );

    // Nothing was touched: back to normal, the connection writes under the old key, and
    // then a rekey works.
    db.connection()
        .execute_batch("PRAGMA query_only = OFF")
        .unwrap();
    db.setting_set("ui.after", &json!(1), 1).unwrap();
    db.rekey(&key_b).unwrap();
    drop(db);
    let db = open_encrypted(&path, &key_b);
    assert_filled(&db);
    assert_eq!(db.setting_get("ui.after").unwrap(), Some(json!(1)));
}

#[test]
fn rekeying_a_database_larger_than_the_page_cache_rewrites_every_page() {
    // About 6 MB, three times the default page cache: the rewrite cannot be held in
    // memory and spills into the log while it runs.
    const ROWS: i64 = 3_000;
    const VALUE_LEN: usize = 2_000;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let (key_a, key_b) = (db_key(1), db_key(2));
    let mut db = open_encrypted(&path, &key_a);
    db.transaction(|tx| {
        for i in 0..ROWS {
            tx.setting_put(&format!("app.k{i:05}"), &json!("x".repeat(VALUE_LEN)), i)?;
        }
        Ok(())
    })
    .unwrap();
    let pages: i64 = pragma(db.connection(), "page_count").parse().unwrap();
    assert!(pages > 1_000, "{pages} pages");

    db.rekey(&key_b).unwrap();
    drop(db);

    let err = sqlcipher::open_file(&path, &key_a).unwrap_err();
    assert!(matches!(err, StorageError::WrongKey), "{err}");
    let db = open_encrypted(&path, &key_b);
    let (count, total): (i64, i64) = db
        .connection()
        .query_row(
            "SELECT count(*), sum(length(value)) FROM settings",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, ROWS);
    // Each value is a JSON string: the text and its two quotes.
    assert_eq!(total, ROWS * (i64::try_from(VALUE_LEN).unwrap() + 2));
    assert_eq!(cipher_failures(db.connection()), 0);
}

#[test]
fn a_rekeyed_file_leaves_no_plaintext_and_no_key_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let (key_a, key_b) = (db_key(1), db_key(2));
    let (hex_a, hex_b) = (key_hex(&key_a), key_hex(&key_b));
    let needles: [&[u8]; 4] = [
        MARKER.as_bytes(),
        hex_a.as_bytes(),
        hex_b.as_bytes(),
        b"SQLite format 3",
    ];

    let mut db = open_encrypted(&path, &key_a);
    fill_everything(&mut db);
    db.rekey(&key_b).unwrap();
    db.setting_set("ui.after", &json!(MARKER), 9).unwrap();
    assert!(
        !any_file_holds(dir.path(), &needles),
        "an open database's files"
    );
    drop(db);
    assert!(
        !any_file_holds(dir.path(), &needles),
        "a closed database's files"
    );
}

#[test]
fn rekey_without_encryption_changes_no_byte_of_a_plain_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.db");
    let mut db = open_plain(&path);
    fill_everything(&mut db);
    drop(db);
    let before = fs::read(&path).unwrap();

    let db = open_plain(&path);
    assert_eq!(db.encryption(), &Encryption::None);
    db.rekey(&db_key(1)).unwrap();
    drop(db);
    assert_eq!(fs::read(&path).unwrap(), before, "not a byte written");
    assert_filled(&open_plain(&path));
}

#[test]
fn rekeying_a_keyed_in_memory_connection_has_no_file_to_reopen_and_still_works() {
    let (key_a, key_b) = (db_key(1), db_key(2));
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key_hex(&key_a)))
        .unwrap();
    let db = Db::new(OpenedDb {
        conn,
        encryption: Encryption::SqlCipher {
            version: "test".to_owned(),
        },
    })
    .unwrap();
    fill(&db);

    db.rekey(&key_b).unwrap();
    assert_filled(&db);
    db.device_log_append(&v4_log_row(0)).unwrap();
    assert_eq!(db.device_log_all().unwrap(), [v4_log_row(0)]);
}

// --- v7: the pending table keyed by cursor and document --------------------------------

#[test]
fn the_v7_fixture_opens_at_version_7_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V7).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V7, "v7.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "7",
        "the fixture is a schema v7 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    if SCHEMA_VERSION == 7 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn migrating_the_v6_fixture_keeps_every_row_and_rekeys_the_pending_table() {
    let (_dir, copy) = copy_of(FIXTURE_V6, "v6.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "6", "a schema v6 file");
        dump_all(&conn)
    };
    assert_eq!(rows_of(&before, "sync_pending").len(), 3);

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());
    // The file migrates on to the current version, whose three new tables (`chunk_sync`, v8,
    // `date_mentions`, v9, and `page_properties`, v10) are empty; every other table is as it was.
    assert!(rows_of(&after, "chunk_sync").is_empty());
    assert!(rows_of(&after, "date_mentions").is_empty());
    assert!(rows_of(&after, "page_properties").is_empty());
    assert_eq!(
        without_properties(without_date_mentions(without_chunk_sync(after))),
        before,
        "every row of every table is carried over"
    );
    assert_eq!(db.sync_pending_all().unwrap(), v6_pending());

    // Two items parked under one cursor for different documents coexist (several
    // snapshots cover the same cursor); a put replaces the one with both keys only.
    let shared = SyncPendingRow {
        cursor: 19,
        doc_id: doc_id(V2_PAGE_OLD),
        bytes: vec![0x53, 0x4e, 0x00],
        reason: 3,
        received_at: 1_760_000_513_000,
    };
    db.sync_pending_put(&shared).unwrap();
    assert_eq!(db.sync_pending_count().unwrap(), 4);
    let replaced = SyncPendingRow {
        bytes: vec![0x53, 0x4e, 0x01],
        ..shared.clone()
    };
    db.sync_pending_put(&replaced).unwrap();
    assert_eq!(db.sync_pending_count().unwrap(), 4);
    let at_19: Vec<SyncPendingRow> = db
        .sync_pending_all()
        .unwrap()
        .into_iter()
        .filter(|row| row.cursor == 19)
        .collect();
    assert_eq!(at_19.len(), 2, "one cursor, two documents");
    assert!(
        at_19.contains(&replaced),
        "the put replaced the row with both keys"
    );
    assert!(
        at_19.contains(&v6_pending()[1]),
        "the other document's row stayed"
    );

    // A delete names both keys.
    db.sync_pending_delete(19, doc_id(V2_PAGE_OLD)).unwrap();
    assert_eq!(db.sync_pending_all().unwrap(), v6_pending());
    db.sync_pending_delete(19, doc_id(V2_PAGE_OLD)).unwrap();
    assert_eq!(
        db.sync_pending_count().unwrap(),
        3,
        "deleting a missing row is fine"
    );
}

// --- v8: the per-chunk transfer state of attachment sync --------------------------------

// The stand-ins of `fixtures/db/v8.db`, as written by `examples/make_fixture_db.rs`: a chunk
// id is `ramp(first byte, 32)` and the attachments are those of the v5 content.
const V8_CHUNK_REPORT_1: u8 = 0xC3;
const V8_CHUNK_REPORT_2: u8 = 0x21;
const V8_CHUNK_REPORT_3: u8 = 0x84;
const V8_CHUNK_LATE_1: u8 = 0x5A;
const V8_CHUNK_PURGED_1: u8 = 0xE7;

/// The time the late file's first chunk waits for.
const V8_LATE_NEXT_AT: i64 = 1_760_000_600_000;

fn v8_chunk(first: u8) -> ChunkId {
    ChunkId::from_bytes(<[u8; 32]>::try_from(ramp(first, 32)).unwrap())
}

fn v8_row(
    first: u8,
    attachment: &str,
    state: u8,
    attempts: u32,
    next_at: Option<i64>,
) -> ChunkSyncRow {
    ChunkSyncRow {
        hash: v8_chunk(first),
        attachment_id: attachment_id(attachment),
        state,
        attempts,
        next_at,
    }
}

/// The report's first chunk: both sides have it.
fn v8_report_synced() -> ChunkSyncRow {
    v8_row(V8_CHUNK_REPORT_1, V5_ATT_REPORT, 3, 0, None)
}

/// The report's second chunk: to upload, after three failed tries.
fn v8_report_failed_upload() -> ChunkSyncRow {
    v8_row(V8_CHUNK_REPORT_2, V5_ATT_REPORT, 1, 3, None)
}

/// The report's third chunk: to upload, not tried yet.
fn v8_report_fresh_upload() -> ChunkSyncRow {
    v8_row(V8_CHUNK_REPORT_3, V5_ATT_REPORT, 1, 0, None)
}

/// The late file's first chunk: wanted, the server did not hold it twice, wait until
/// `V8_LATE_NEXT_AT`.
fn v8_late_download() -> ChunkSyncRow {
    v8_row(V8_CHUNK_LATE_1, V5_ATT_LATE, 2, 2, Some(V8_LATE_NEXT_AT))
}

/// The purged attachment's chunk: to delete on the server, after one failed try.
fn v8_purged_delete() -> ChunkSyncRow {
    v8_row(V8_CHUNK_PURGED_1, V5_ATT_PURGED, 4, 1, None)
}

/// The five rows of the v8 fixture in hash order (the order `chunk_sync_by_attachment`
/// lists a single attachment's rows in).
fn v8_rows() -> [ChunkSyncRow; 5] {
    [
        v8_report_failed_upload(), // 0x21
        v8_late_download(),        // 0x5A
        v8_report_fresh_upload(),  // 0x84
        v8_report_synced(),        // 0xC3
        v8_purged_delete(),        // 0xE7
    ]
}

/// The first byte of each hash, for readable assertions.
fn firsts(rows: &[ChunkSyncRow]) -> Vec<u8> {
    rows.iter().map(|row| row.hash.as_bytes()[0]).collect()
}

/// Runs `scenario` over a fresh copy of the v8 fixture opened on a bare `Db`, and again over
/// another copy inside a transaction, so both implementations of `ChunkSyncQueries` read and
/// write the same file-backed rows alike.
fn on_v8_db_and_tx(scenario: impl Fn(&dyn ChunkSyncQueries)) {
    let (_dir, copy) = copy_of(FIXTURE_V8, "v8.db");
    scenario(&open_plain(&copy));
    let (_dir, copy) = copy_of(FIXTURE_V8, "v8.db");
    let mut db = open_plain(&copy);
    db.transaction(|tx| {
        scenario(tx);
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_v8_fixture_opens_at_version_8_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V8).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V8, "v8.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "8",
        "the fixture is a schema v8 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "chunk_sync"), 5);
    drop(db);
    if SCHEMA_VERSION == 8 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn migrating_the_v7_fixture_keeps_every_row_of_every_table_and_adds_the_empty_chunk_sync_table() {
    let (_dir, copy) = copy_of(FIXTURE_V7, "v7.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "7", "a schema v7 file");
        dump_all(&conn)
    };
    // The dump is not empty-handed: the tables of the fixture hold what it was built with.
    for (table, rows) in [
        ("meta", 8),
        ("workspace", 1),
        ("settings", 2),
        ("docs", 7),
        ("doc_snapshots", 2),
        ("doc_updates", 12),
        ("pages_index", 5),
        ("links", 6),
        ("fts_blocks", 14),
        ("device_log", 3),
        ("keyring", 8),
        ("attachments_index", 6),
        ("attachment_refs", 6),
        ("sync_config", 8),
        ("sync_doc_state", 7),
        ("sync_seen", 5),
        ("sync_heads", 2),
        ("sync_pending", 3),
    ] {
        assert_eq!(
            rows_of(&before, table).len(),
            rows,
            "{table} in the v7 file"
        );
    }
    let old_tables: Vec<&str> = before.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        !old_tables.contains(&"chunk_sync"),
        "chunk_sync is new in v8"
    );

    let mut db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());

    // Every table of v7 has exactly the rows it had (`fts_blocks` with the 0 of its new `kind`
    // column); the one table of v8, the one of v9 and the one of v10 (the file migrates to the
    // current version) are new, empty.
    let after_rows = without_properties(after.clone());
    for (table, rows) in &before {
        assert_eq!(
            rows_of(&after_rows, table),
            rows.as_slice(),
            "{table} is unchanged"
        );
    }
    let new_tables: Vec<&str> = after
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !old_tables.contains(name))
        .collect();
    assert_eq!(
        new_tables,
        ["chunk_sync", "date_mentions", "page_properties"]
    );
    assert_eq!(count_rows(db.connection(), "chunk_sync"), 0);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 0);
    assert_eq!(count_rows(db.connection(), "page_properties"), 0);
    assert_eq!(
        without_properties(without_date_mentions(without_chunk_sync(after))),
        before,
        "nothing but the new tables differs"
    );

    // The table and its two indexes are there, and every query answers for an empty table.
    let indexes = index_names(db.connection(), "chunk_sync");
    assert!(
        indexes.contains(&"chunk_sync_work".to_owned()),
        "{indexes:?}"
    );
    assert!(
        indexes.contains(&"chunk_sync_attachment".to_owned()),
        "{indexes:?}"
    );
    let attachment = attachment_id(V5_ATT_REPORT);
    assert_eq!(
        db.chunk_sync_get(v8_chunk(V8_CHUNK_REPORT_1)).unwrap(),
        None
    );
    assert!(db.chunk_sync_by_attachment(attachment).unwrap().is_empty());
    for state in 1..=4 {
        assert_eq!(db.chunk_sync_count(state).unwrap(), 0);
        assert!(db.chunk_sync_due(state, i64::MAX, 100).unwrap().is_empty());
    }
    assert_eq!(db.chunk_sync_count_waiting().unwrap(), 0);
    assert_eq!(
        db.chunk_sync_count_attachment(attachment, &[1, 2, 3, 4])
            .unwrap(),
        0
    );
    assert_eq!(db.chunk_sync_reset(3, 1).unwrap(), 0);

    // The tables and the indexes of v3 to v7 still work on the migrated file.
    db.connection()
        .execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    assert_eq!(found(&db, "notes").len(), 5);
    assert_eq!(db.attachments_all().unwrap().len(), 6);
    assert_eq!(db.sync_pending_all().unwrap(), v6_pending());
    assert_eq!(db.sync_doc_state_all().unwrap().len(), 7);
    assert_eq!(db.device_log_all().unwrap().len(), 3);
    assert_eq!(db.keyring_all().unwrap().len(), 8);

    // The migrated file takes chunk rows, and still has them after a reopen.
    db.transaction(|tx| {
        for row in v8_rows() {
            tx.chunk_sync_put(&row)?;
        }
        Ok(())
    })
    .unwrap();
    drop(db);

    let db = open_plain(&copy);
    assert_eq!(
        db.chunk_sync_by_attachment(attachment).unwrap(),
        [
            v8_report_failed_upload(),
            v8_report_fresh_upload(),
            v8_report_synced()
        ]
    );
    assert_eq!(db.chunk_sync_count(1).unwrap(), 2);
    let reopened = without_properties(dump_all(db.connection()));
    for (table, rows) in &before {
        assert_eq!(rows_of(&reopened, table), rows.as_slice(), "{table}");
    }
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string(),
        "written to the file"
    );
}

#[test]
fn the_v8_fixture_reads_back_through_get_by_attachment_and_the_counts() {
    on_v8_db_and_tx(|q| {
        // Every row as written, by hash.
        for row in v8_rows() {
            assert_eq!(q.chunk_sync_get(row.hash).unwrap(), Some(row.clone()));
        }
        assert_eq!(
            q.chunk_sync_get(v8_chunk(0x42)).unwrap(),
            None,
            "a chunk the table does not know"
        );
        let mut other = v8_chunk(V8_CHUNK_REPORT_1).to_bytes();
        other[31] ^= 0x01;
        assert_eq!(
            q.chunk_sync_get(ChunkId::from_bytes(other)).unwrap(),
            None,
            "a hash is matched in full"
        );

        // One attachment's rows come in hash order, not in insertion order.
        let report = q
            .chunk_sync_by_attachment(attachment_id(V5_ATT_REPORT))
            .unwrap();
        assert_eq!(
            report,
            [
                v8_report_failed_upload(),
                v8_report_fresh_upload(),
                v8_report_synced()
            ]
        );
        assert_eq!(firsts(&report), [0x21, 0x84, 0xC3]);
        assert_eq!(
            q.chunk_sync_by_attachment(attachment_id(V5_ATT_LATE))
                .unwrap(),
            [v8_late_download()]
        );
        assert_eq!(
            q.chunk_sync_by_attachment(attachment_id(V5_ATT_PURGED))
                .unwrap(),
            [v8_purged_delete()]
        );
        assert!(
            q.chunk_sync_by_attachment(attachment_id(V5_ATT_PHOTO))
                .unwrap()
                .is_empty(),
            "an attachment without rows"
        );

        // The counts: by state, the waiting downloads and by attachment and state list.
        assert_eq!(q.chunk_sync_count(1).unwrap(), 2);
        assert_eq!(q.chunk_sync_count(2).unwrap(), 1);
        assert_eq!(q.chunk_sync_count(3).unwrap(), 1);
        assert_eq!(q.chunk_sync_count(4).unwrap(), 1);
        assert_eq!(q.chunk_sync_count(0).unwrap(), 0);
        assert_eq!(
            q.chunk_sync_count_waiting().unwrap(),
            1,
            "the late file's chunk: a download that was tried"
        );
        let count = |attachment: &str, states: &[u8]| {
            q.chunk_sync_count_attachment(attachment_id(attachment), states)
                .unwrap()
        };
        assert_eq!(count(V5_ATT_REPORT, &[1]), 2);
        assert_eq!(count(V5_ATT_REPORT, &[3]), 1);
        assert_eq!(count(V5_ATT_REPORT, &[1, 3]), 3);
        assert_eq!(count(V5_ATT_REPORT, &[1, 2, 3, 4]), 3);
        assert_eq!(count(V5_ATT_REPORT, &[2, 4]), 0);
        assert_eq!(count(V5_ATT_REPORT, &[]), 0);
        assert_eq!(count(V5_ATT_REPORT, &[3, 3, 1, 1]), 3);
        assert_eq!(count(V5_ATT_LATE, &[2]), 1);
        assert_eq!(count(V5_ATT_LATE, &[1, 3, 4]), 0);
        assert_eq!(count(V5_ATT_PURGED, &[4]), 1);
        assert_eq!(count(V5_ATT_PURGED, &[1, 2, 3]), 0);
        assert_eq!(count(V5_ATT_PHOTO, &[1, 2, 3, 4]), 0);
    });
}

#[test]
fn the_v8_fixture_is_due_by_state_time_attempts_and_hash() {
    on_v8_db_and_tx(|q| {
        // Uploads: both have no next_at; the one with fewer attempts comes first although
        // its hash is the larger (0x84 against 0x21).
        let uploads = q.chunk_sync_due(1, 0, 10).unwrap();
        assert_eq!(
            uploads,
            [v8_report_fresh_upload(), v8_report_failed_upload()]
        );
        assert_eq!(firsts(&uploads), [0x84, 0x21]);
        assert_eq!(
            q.chunk_sync_due(1, i64::MIN, 10).unwrap(),
            uploads,
            "a NULL next_at is due whatever the time"
        );
        assert_eq!(firsts(&q.chunk_sync_due(1, 0, 1).unwrap()), [0x84]);
        assert!(q.chunk_sync_due(1, 0, 0).unwrap().is_empty());

        // The late file's download is due from its next_at on, inclusive.
        for now in [0, V8_LATE_NEXT_AT - 1] {
            assert!(q.chunk_sync_due(2, now, 10).unwrap().is_empty(), "at {now}");
        }
        for now in [V8_LATE_NEXT_AT, V8_LATE_NEXT_AT + 1, i64::MAX] {
            assert_eq!(
                q.chunk_sync_due(2, now, 10).unwrap(),
                [v8_late_download()],
                "at {now}"
            );
        }

        // The other states.
        assert_eq!(q.chunk_sync_due(3, 0, 10).unwrap(), [v8_report_synced()]);
        assert_eq!(q.chunk_sync_due(4, 0, 10).unwrap(), [v8_purged_delete()]);
        assert!(q.chunk_sync_due(5, i64::MAX, 10).unwrap().is_empty());

        // A back-off moves a row out of the due set and back in: set a time ahead on the
        // fresh upload, then one in the past.
        q.chunk_sync_set(v8_chunk(V8_CHUNK_REPORT_3), 1, 1, Some(5_000))
            .unwrap();
        assert_eq!(firsts(&q.chunk_sync_due(1, 4_999, 10).unwrap()), [0x21]);
        assert_eq!(
            firsts(&q.chunk_sync_due(1, 5_000, 10).unwrap()),
            [0x84, 0x21],
            "one attempt against three: 0x84 is still first"
        );
        q.chunk_sync_set(v8_chunk(V8_CHUNK_REPORT_3), 1, 3, Some(1))
            .unwrap();
        assert_eq!(
            firsts(&q.chunk_sync_due(1, 10, 10).unwrap()),
            [0x21, 0x84],
            "equal attempts: hash order"
        );
        assert_eq!(
            q.chunk_sync_due(1, 10, 10).unwrap()[1],
            v8_row(V8_CHUNK_REPORT_3, V5_ATT_REPORT, 1, 3, Some(1))
        );
    });
}

#[test]
fn the_v8_fixture_takes_put_set_delete_reset_and_clear() {
    on_v8_db_and_tx(|q| {
        let report = attachment_id(V5_ATT_REPORT);
        let late = attachment_id(V5_ATT_LATE);

        // A put with a known hash replaces the whole row; with a new hash it adds one.
        let moved = v8_row(V8_CHUNK_LATE_1, V5_ATT_REPORT, 1, 0, None);
        q.chunk_sync_put(&moved).unwrap();
        assert_eq!(q.chunk_sync_get(moved.hash).unwrap(), Some(moved.clone()));
        assert!(q.chunk_sync_by_attachment(late).unwrap().is_empty());
        assert_eq!(q.chunk_sync_by_attachment(report).unwrap().len(), 4);
        assert_eq!(q.chunk_sync_count(2).unwrap(), 0);
        assert_eq!(q.chunk_sync_count_waiting().unwrap(), 0);
        let added = v8_row(0x99, V5_ATT_LATE, 2, 0, None);
        q.chunk_sync_put(&added).unwrap();
        assert_eq!(
            q.chunk_sync_by_attachment(late).unwrap(),
            std::slice::from_ref(&added)
        );
        assert_eq!(q.chunk_sync_count(2).unwrap(), 1);
        assert_eq!(q.chunk_sync_count_waiting().unwrap(), 0, "never tried");

        // A set on a present row changes the three columns and keeps the attachment; on a
        // missing row nothing happens.
        q.chunk_sync_set(added.hash, 2, 4, Some(123)).unwrap();
        assert_eq!(
            q.chunk_sync_get(added.hash).unwrap(),
            Some(v8_row(0x99, V5_ATT_LATE, 2, 4, Some(123)))
        );
        assert_eq!(q.chunk_sync_count_waiting().unwrap(), 1);
        q.chunk_sync_set(v8_chunk(0x42), 2, 4, Some(123)).unwrap();
        assert_eq!(q.chunk_sync_get(v8_chunk(0x42)).unwrap(), None);
        assert_eq!(q.chunk_sync_count(2).unwrap(), 1);

        // A delete takes one row, a missing one is fine.
        q.chunk_sync_delete(v8_chunk(V8_CHUNK_PURGED_1)).unwrap();
        q.chunk_sync_delete(v8_chunk(V8_CHUNK_PURGED_1)).unwrap();
        assert_eq!(q.chunk_sync_count(4).unwrap(), 0);
        assert!(
            q.chunk_sync_by_attachment(attachment_id(V5_ATT_PURGED))
                .unwrap()
                .is_empty()
        );

        // A reset moves one state to another and clears the back-off: the synced chunk of
        // the report goes up again, as on a new server epoch, and a row of another state
        // keeps what it had.
        q.chunk_sync_set(v8_chunk(V8_CHUNK_REPORT_1), 3, 2, Some(77))
            .unwrap();
        assert_eq!(q.chunk_sync_reset(3, 1).unwrap(), 1);
        assert_eq!(
            q.chunk_sync_get(v8_chunk(V8_CHUNK_REPORT_1)).unwrap(),
            Some(v8_row(V8_CHUNK_REPORT_1, V5_ATT_REPORT, 1, 0, None))
        );
        assert_eq!(
            q.chunk_sync_get(v8_chunk(V8_CHUNK_REPORT_2)).unwrap(),
            Some(v8_report_failed_upload())
        );
        assert_eq!(q.chunk_sync_reset(3, 1).unwrap(), 0, "nothing left to move");
        assert_eq!(q.chunk_sync_count(3).unwrap(), 0);
        assert_eq!(q.chunk_sync_count(1).unwrap(), 4);
        // The downloads are reset in place: still downloads, no back-off any more.
        q.chunk_sync_set(added.hash, 2, 4, Some(123)).unwrap();
        assert_eq!(q.chunk_sync_reset(2, 2).unwrap(), 1);
        assert_eq!(q.chunk_sync_count_waiting().unwrap(), 0);
        assert_eq!(
            q.chunk_sync_due(2, 0, 10).unwrap(),
            std::slice::from_ref(&added)
        );

        // Deleting an attachment's rows leaves the others.
        q.chunk_sync_delete_attachment(report).unwrap();
        q.chunk_sync_delete_attachment(report).unwrap();
        assert!(q.chunk_sync_by_attachment(report).unwrap().is_empty());
        assert_eq!(
            q.chunk_sync_by_attachment(late).unwrap(),
            std::slice::from_ref(&added)
        );
        assert_eq!(q.chunk_sync_count(1).unwrap(), 0);
        assert_eq!(q.chunk_sync_count(2).unwrap(), 1);

        // Clear empties the table, which takes rows again.
        q.chunk_sync_clear().unwrap();
        q.chunk_sync_clear().unwrap();
        for state in 1..=4 {
            assert_eq!(q.chunk_sync_count(state).unwrap(), 0);
        }
        assert!(q.chunk_sync_by_attachment(late).unwrap().is_empty());
        q.chunk_sync_put(&v8_report_synced()).unwrap();
        assert_eq!(q.chunk_sync_count(3).unwrap(), 1);
    });
}

#[test]
fn the_epoch_reset_of_the_sync_tables_does_not_touch_the_chunk_rows_of_the_v8_fixture() {
    let (_dir, copy) = copy_of(FIXTURE_V8, "v8.db");
    let mut db = open_plain(&copy);
    // What the core runs for a new server epoch, apart from `chunk_sync_reset`, which it
    // calls itself.
    db.transaction(|tx| {
        tx.sync_doc_state_reset_all()?;
        tx.keyring_mark_all_unpublished()?;
        tx.device_log_mark_all_unpublished()?;
        tx.sync_seen_clear()?;
        tx.sync_heads_clear()?;
        tx.sync_pending_clear()
    })
    .unwrap();
    let rows: Vec<ChunkSyncRow> = [
        V8_CHUNK_REPORT_2,
        V8_CHUNK_LATE_1,
        V8_CHUNK_REPORT_3,
        V8_CHUNK_REPORT_1,
        V8_CHUNK_PURGED_1,
    ]
    .iter()
    .map(|first| db.chunk_sync_get(v8_chunk(*first)).unwrap().unwrap())
    .collect();
    assert_eq!(rows, v8_rows());
    assert_eq!(db.sync_pending_count().unwrap(), 0, "the reset ran");
}

#[test]
fn chunk_rows_survive_a_reopen_of_an_encrypted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE_NAME);
    let key = db_key(1);

    let mut db = open_encrypted(&path, &key);
    db.transaction(|tx| {
        for row in v8_rows() {
            tx.chunk_sync_put(&row)?;
        }
        Ok(())
    })
    .unwrap();
    drop(db);

    // Without the key the file does not even open as a database.
    assert!(
        Connection::open(&path)
            .unwrap()
            .query_row("SELECT count(*) FROM chunk_sync", [], |row| row
                .get::<_, i64>(0))
            .is_err()
    );

    let db = open_encrypted(&path, &key);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        db.chunk_sync_by_attachment(attachment_id(V5_ATT_REPORT))
            .unwrap(),
        [
            v8_report_failed_upload(),
            v8_report_fresh_upload(),
            v8_report_synced()
        ]
    );
    assert_eq!(
        db.chunk_sync_due(2, V8_LATE_NEXT_AT, 10).unwrap(),
        [v8_late_download()]
    );
    assert_eq!(db.chunk_sync_count_waiting().unwrap(), 1);
}

// --- v9: the date mentions of the pages (reminders, ADR-0024) --------------------------------

/// A schema v9 file (`make_fixture_db -- v9`): the v8 content plus five `date_mentions` rows.
const FIXTURE_V9: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v9.db");

// The ids of `fixtures/db/v9.db`, as written by `examples/make_fixture_db.rs`: the pages and
// blocks of the v3 content and five mentions.
const V9_BLOCK_ROADMAP_SHIP: &str = "b10c0000-0000-4000-8000-000000000008";
const V9_MENTION_HOME_MEETING: &str = "d47e0000-0000-4000-8000-000000000001";
const V9_MENTION_HOME_PLAIN: &str = "d47e0000-0000-4000-8000-000000000002";
const V9_MENTION_NOTES_REVIEW: &str = "d47e0000-0000-4000-8000-000000000003";
const V9_MENTION_OLD_DEADLINE: &str = "d47e0000-0000-4000-8000-000000000004";
const V9_MENTION_ROADMAP_SHIP: &str = "d47e0000-0000-4000-8000-000000000005";

/// Fire times on the local wall clock (seconds since 1970-01-01T00:00:00, calendar fields
/// counted as UTC): 2026-10-07 14:15, 2026-10-08 09:00, 2026-10-09 09:00 and 2027-01-13 08:00.
const V9_FIRE_HOME_MEETING: i64 = 1_791_382_500;
const V9_FIRE_NOTES_REVIEW: i64 = 1_791_450_000;
const V9_FIRE_OLD_DEADLINE: i64 = 1_791_536_400;
const V9_FIRE_ROADMAP_SHIP: i64 = 1_799_827_200;

fn v9_home_meeting() -> DateMentionRow {
    DateMentionRow {
        block_id: block_id(V3_BLOCK_HOME_PLAN),
        node_id: block_id(V9_MENTION_HOME_MEETING),
        date: "2026-10-07".to_owned(),
        time: Some("14:30".to_owned()),
        reminder_minutes: Some(15),
        fire_at_local: Some(V9_FIRE_HOME_MEETING),
        reminder_for: None,
    }
}

fn v9_home_plain() -> DateMentionRow {
    DateMentionRow {
        block_id: block_id(V3_BLOCK_HOME_WELCOME),
        node_id: block_id(V9_MENTION_HOME_PLAIN),
        date: "2026-10-08".to_owned(),
        time: None,
        reminder_minutes: None,
        fire_at_local: None,
        reminder_for: None,
    }
}

fn v9_notes_review() -> DateMentionRow {
    DateMentionRow {
        block_id: block_id(V3_BLOCK_NOTES_MEETING),
        node_id: block_id(V9_MENTION_NOTES_REVIEW),
        date: "2026-10-09".to_owned(),
        time: Some("09:00".to_owned()),
        reminder_minutes: Some(1440),
        fire_at_local: Some(V9_FIRE_NOTES_REVIEW),
        reminder_for: None,
    }
}

fn v9_old_deadline() -> DateMentionRow {
    DateMentionRow {
        block_id: block_id(V3_BLOCK_OLD_IDEAS),
        node_id: block_id(V9_MENTION_OLD_DEADLINE),
        date: "2026-10-09".to_owned(),
        time: None,
        reminder_minutes: Some(0),
        fire_at_local: Some(V9_FIRE_OLD_DEADLINE),
        reminder_for: None,
    }
}

fn v9_roadmap_ship() -> DateMentionRow {
    DateMentionRow {
        block_id: block_id(V9_BLOCK_ROADMAP_SHIP),
        node_id: block_id(V9_MENTION_ROADMAP_SHIP),
        date: "2027-01-15".to_owned(),
        time: Some("08:00".to_owned()),
        reminder_minutes: Some(2880),
        fire_at_local: Some(V9_FIRE_ROADMAP_SHIP),
        reminder_for: None,
    }
}

/// What the due query answers for a reminder row of the fixture.
fn v9_due(page: &str, space: &str, row: &DateMentionRow) -> DueReminderRow {
    DueReminderRow {
        page_id: page_id(page),
        space_id: space_id(space),
        block_id: row.block_id,
        node_id: row.node_id,
        date: row.date.clone(),
        time: row.time.clone(),
        reminder_minutes: row.reminder_minutes.unwrap(),
        fire_at_local: row.fire_at_local.unwrap(),
        reminder_for: None,
    }
}

/// Runs `scenario` over a fresh copy of the v9 fixture opened on a bare `Db`, and again over
/// another copy inside a transaction, so both implementations of `DateMentionQueries` read and
/// write the same file-backed rows alike.
fn on_v9_db_and_tx(scenario: impl Fn(&dyn DateMentionQueries)) {
    let (_dir, copy) = copy_of(FIXTURE_V9, "v9.db");
    scenario(&open_plain(&copy));
    let (_dir, copy) = copy_of(FIXTURE_V9, "v9.db");
    let mut db = open_plain(&copy);
    db.transaction(|tx| {
        scenario(tx);
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_v9_fixture_opens_at_version_9_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V9).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V9, "v9.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "9",
        "the fixture is a schema v9 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 5);
    drop(db);
    if SCHEMA_VERSION == 9 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn migrating_the_v8_fixture_keeps_every_row_of_every_table_and_adds_the_empty_date_mentions_table()
{
    let (_dir, copy) = copy_of(FIXTURE_V8, "v8.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "8", "a schema v8 file");
        dump_all(&conn)
    };
    assert_eq!(rows_of(&before, "chunk_sync").len(), 5);
    assert_eq!(rows_of(&before, "links").len(), 6);
    let old_tables: Vec<&str> = before.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        !old_tables.contains(&"date_mentions"),
        "date_mentions is new in v9"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());
    let new_tables: Vec<&str> = after
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !old_tables.contains(name))
        .collect();
    assert_eq!(new_tables, ["date_mentions", "page_properties"]);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 0);
    assert_eq!(count_rows(db.connection(), "page_properties"), 0);
    assert_eq!(
        without_properties(without_date_mentions(after)),
        before,
        "every row of every table is carried over"
    );

    // The table is there with its partial index, and every query answers for an empty table.
    let indexes = index_names(db.connection(), "date_mentions");
    assert!(
        indexes.contains(&"date_mentions_fire".to_owned()),
        "{indexes:?}"
    );
    assert!(
        db.date_mentions_of(page_id(V2_PAGE_HOME))
            .unwrap()
            .is_empty()
    );
    assert!(
        db.date_mentions_due(i64::MIN, i64::MAX, 100)
            .unwrap()
            .is_empty()
    );

    // And it takes rows on the migrated file, next to the v3 index the pages already have.
    db.date_mentions_replace(page_id(V2_PAGE_HOME), &[v9_home_meeting(), v9_home_plain()])
        .unwrap();
    assert_eq!(
        db.date_mentions_due(0, i64::MAX, 100).unwrap(),
        [v9_due(V2_PAGE_HOME, V2_PERSONAL, &v9_home_meeting())]
    );
}

#[test]
fn the_v9_fixture_lists_the_mentions_of_each_page() {
    on_v9_db_and_tx(|q| {
        assert_eq!(
            q.date_mentions_of(page_id(V2_PAGE_HOME)).unwrap(),
            [v9_home_meeting(), v9_home_plain()],
            "in the order they were written"
        );
        assert_eq!(
            q.date_mentions_of(page_id(V2_PAGE_NOTES)).unwrap(),
            [v9_notes_review()]
        );
        assert_eq!(
            q.date_mentions_of(page_id(V2_PAGE_OLD)).unwrap(),
            [v9_old_deadline()],
            "a trashed page keeps its rows"
        );
        assert_eq!(
            q.date_mentions_of(page_id(V2_PAGE_ROADMAP)).unwrap(),
            [v9_roadmap_ship()]
        );
        assert!(
            q.date_mentions_of(page_id(V2_PAGE_OLD_CHILD))
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn the_v9_fixture_answers_the_due_query_for_live_pages_only() {
    on_v9_db_and_tx(|q| {
        let home = v9_due(V2_PAGE_HOME, V2_PERSONAL, &v9_home_meeting());
        let notes = v9_due(V2_PAGE_NOTES, V2_PERSONAL, &v9_notes_review());
        let roadmap = v9_due(V2_PAGE_ROADMAP, V2_WORK, &v9_roadmap_ship());

        // Everything: the reminder of the trashed page is left out, and so is the mention
        // without a reminder.
        assert_eq!(
            q.date_mentions_due(i64::MIN, i64::MAX, 100).unwrap(),
            [home.clone(), notes.clone(), roadmap.clone()]
        );
        // A window open below, closed above.
        assert_eq!(
            q.date_mentions_due(V9_FIRE_HOME_MEETING, V9_FIRE_NOTES_REVIEW, 100)
                .unwrap(),
            [notes]
        );
        assert_eq!(
            q.date_mentions_due(V9_FIRE_HOME_MEETING - 1, V9_FIRE_HOME_MEETING, 100)
                .unwrap(),
            [home]
        );
        assert!(
            q.date_mentions_due(V9_FIRE_NOTES_REVIEW, V9_FIRE_ROADMAP_SHIP - 1, 100)
                .unwrap()
                .is_empty(),
            "only the trashed page has a reminder in this window"
        );
        assert_eq!(
            q.date_mentions_due(V9_FIRE_NOTES_REVIEW, V9_FIRE_ROADMAP_SHIP, 1)
                .unwrap(),
            [roadmap]
        );
    });
}

#[test]
fn restoring_a_page_of_the_v9_fixture_brings_its_reminder_back() {
    let (_dir, copy) = copy_of(FIXTURE_V9, "v9.db");
    let db = open_plain(&copy);
    let window = (V9_FIRE_NOTES_REVIEW, V9_FIRE_OLD_DEADLINE);
    assert!(
        db.date_mentions_due(window.0, window.1, 100)
            .unwrap()
            .is_empty()
    );
    let old = db.pages_index_get(page_id(V2_PAGE_OLD)).unwrap().unwrap();
    db.pages_index_upsert(&[PageIndexRow {
        trashed_at: None,
        ..old
    }])
    .unwrap();
    assert_eq!(
        db.date_mentions_due(window.0, window.1, 100).unwrap(),
        [v9_due(V2_PAGE_OLD, V2_PERSONAL, &v9_old_deadline())]
    );
}

#[test]
fn the_v9_fixture_stores_the_document_next_to_the_page_and_purges_by_page() {
    let (_dir, copy) = copy_of(FIXTURE_V9, "v9.db");
    let db = open_plain(&copy);
    assert_eq!(
        count_rows(db.connection(), "date_mentions WHERE doc_id = page_id"),
        5,
        "a page document id is its page id (ADR-0016)"
    );
    db.date_mentions_delete_page(page_id(V2_PAGE_HOME)).unwrap();
    assert_eq!(count_rows(db.connection(), "date_mentions"), 3);
    assert!(
        db.date_mentions_of(page_id(V2_PAGE_HOME))
            .unwrap()
            .is_empty()
    );
    db.date_mentions_clear().unwrap();
    assert_eq!(count_rows(db.connection(), "date_mentions"), 0);
}

// --- v10: the property values of the pages (typed page properties, ADR-0026) ------------------

/// A schema v10 file (`make_fixture_db -- v10`): the v9 content plus the `page_properties` rows of
/// four pages and the property rows (`kind = 1`) of the text index.
const FIXTURE_V10: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v10.db");

// The ids of `fixtures/db/v10.db`, as written by `examples/make_fixture_db.rs`.
const V10_PROP_TAGS: &str = "0000000a-0000-4000-8000-000000000001";
const V10_PROP_SUMMARY: &str = "b09e0000-0000-4000-8000-000000000001";
const V10_PROP_COST: &str = "b09e0000-0000-4000-8000-000000000002";
const V10_PROP_DONE: &str = "b09e0000-0000-4000-8000-000000000003";
const V10_PROP_DUE: &str = "b09e0000-0000-4000-8000-000000000004";
const V10_PROP_STAGE: &str = "b09e0000-0000-4000-8000-000000000005";
const V10_PROP_AREA: &str = "b09e0000-0000-4000-8000-000000000006";
const V10_PROP_LINK: &str = "b09e0000-0000-4000-8000-000000000007";
const V10_OPT_URGENT: &str = "0b710000-0000-4000-8000-000000000001";
const V10_OPT_WORK: &str = "0b710000-0000-4000-8000-000000000002";
const V10_OPT_LATER: &str = "0b710000-0000-4000-8000-000000000003";
const V10_OPT_IN_PROGRESS: &str = "0b710000-0000-4000-8000-000000000004";
const V10_OPT_HOME: &str = "0b710000-0000-4000-8000-000000000005";
const V10_OPT_ELSEWHERE: &str = "0b710000-0000-4000-8000-000000000006";

fn property_id(text: &str) -> PropertyId {
    PropertyId::from_bytes(page_id(text).to_bytes())
}

fn option_id(text: &str) -> OptionId {
    OptionId::from_bytes(page_id(text).to_bytes())
}

fn v10_row(property: &str, kind: PropertyKind) -> PagePropertyRow {
    PagePropertyRow {
        property_id: property_id(property),
        kind,
        value_text: None,
        value_number: None,
        value_bool: None,
        value_date: None,
        value_time: None,
        option_id: None,
        attachment_id: None,
        value_total: None,
        related_page_id: None,
        account_id: None,
    }
}

fn v10_text(property: &str, kind: PropertyKind, text: &str) -> PagePropertyRow {
    PagePropertyRow {
        value_text: Some(text.to_owned()),
        ..v10_row(property, kind)
    }
}

fn v10_number(property: &str, number: f64) -> PagePropertyRow {
    PagePropertyRow {
        value_number: Some(number),
        ..v10_row(property, PropertyKind::Number)
    }
}

fn v10_checkbox(property: &str, on: bool) -> PagePropertyRow {
    PagePropertyRow {
        value_bool: Some(on),
        ..v10_row(property, PropertyKind::Checkbox)
    }
}

fn v10_date(property: &str, day: &str, time: Option<&str>) -> PagePropertyRow {
    PagePropertyRow {
        value_date: Some(day.to_owned()),
        value_time: time.map(str::to_owned),
        ..v10_row(property, PropertyKind::Date)
    }
}

fn v10_option(property: &str, kind: PropertyKind, option: &str) -> PagePropertyRow {
    PagePropertyRow {
        option_id: Some(option_id(option)),
        ..v10_row(property, kind)
    }
}

/// The rows of the Home page of the fixture, in the order they were written.
fn v10_home() -> Vec<PagePropertyRow> {
    vec![
        v10_option(V10_PROP_TAGS, PropertyKind::MultiSelect, V10_OPT_WORK),
        v10_option(V10_PROP_TAGS, PropertyKind::MultiSelect, V10_OPT_URGENT),
        v10_text(
            V10_PROP_SUMMARY,
            PropertyKind::Text,
            "Weekly review ✓ — Café",
        ),
        v10_number(V10_PROP_COST, 12.5),
        v10_checkbox(V10_PROP_DONE, true),
        v10_date(V10_PROP_DUE, "2026-10-07", Some("14:30")),
        v10_option(V10_PROP_STAGE, PropertyKind::Status, V10_OPT_IN_PROGRESS),
        v10_option(V10_PROP_AREA, PropertyKind::Select, V10_OPT_HOME),
    ]
}

fn v10_notes() -> Vec<PagePropertyRow> {
    vec![
        v10_option(V10_PROP_TAGS, PropertyKind::MultiSelect, V10_OPT_URGENT),
        v10_text(V10_PROP_SUMMARY, PropertyKind::Text, "café notes"),
        v10_checkbox(V10_PROP_DONE, false),
        v10_date(V10_PROP_DUE, "2026-10-09", None),
        v10_option(V10_PROP_AREA, PropertyKind::Select, V10_OPT_ELSEWHERE),
    ]
}

fn v10_old() -> Vec<PagePropertyRow> {
    vec![
        v10_option(V10_PROP_TAGS, PropertyKind::MultiSelect, V10_OPT_LATER),
        v10_text(V10_PROP_SUMMARY, PropertyKind::Text, "old"),
    ]
}

fn v10_roadmap() -> Vec<PagePropertyRow> {
    vec![
        v10_text(V10_PROP_SUMMARY, PropertyKind::Text, "Ship it"),
        v10_number(V10_PROP_COST, 3.0),
        v10_text(
            V10_PROP_LINK,
            PropertyKind::Url,
            "https://example.com/roadmap",
        ),
    ]
}

/// Runs `scenario` over a fresh copy of the v10 fixture opened on a bare `Db`, and again over
/// another copy inside a transaction, so both implementations of `PagePropertyQueries` and
/// `TextIndexQueries` read and write the same file-backed rows alike.
fn on_v10_db_and_tx(scenario: impl Fn(&dyn V10Queries)) {
    let (_dir, copy) = copy_of(FIXTURE_V10, "v10.db");
    scenario(&open_plain(&copy));
    let (_dir, copy) = copy_of(FIXTURE_V10, "v10.db");
    let mut db = open_plain(&copy);
    db.transaction(|tx| {
        scenario(tx);
        Ok(())
    })
    .unwrap();
}

/// What the v10 scenarios need of a connection or a transaction.
trait V10Queries: PagePropertyQueries + TextIndexQueries + DocQueries {}
impl<T: PagePropertyQueries + TextIndexQueries + DocQueries> V10Queries for T {}

/// The pages of a query, as the fixture's page ids, in order.
fn v10_pages(
    q: &dyn V10Queries,
    space: &str,
    property: &str,
    matching: &PropertyMatch<'_>,
) -> Vec<PageId> {
    q.page_properties_pages(space_id(space), property_id(property), matching)
        .unwrap()
}

#[test]
fn the_v10_fixture_opens_at_version_10_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V10).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V10, "v10.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "10",
        "the fixture is a schema v10 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "page_properties"), 18);
    assert_eq!(count_rows(db.connection(), "fts_blocks WHERE kind = 1"), 11);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 5);
    drop(db);
    if SCHEMA_VERSION == 10 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn migrating_the_v9_fixture_keeps_every_row_and_adds_the_empty_property_table_and_the_kind_column()
{
    let (_dir, copy) = copy_of(FIXTURE_V9, "v9.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "9", "a schema v9 file");
        dump_all(&conn)
    };
    assert_eq!(rows_of(&before, "date_mentions").len(), 5);
    let old_tables: Vec<&str> = before.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        !old_tables.contains(&"page_properties"),
        "page_properties is new in v10"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());
    let new_tables: Vec<&str> = after
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !old_tables.contains(name))
        .collect();
    assert_eq!(new_tables, ["page_properties"]);
    assert_eq!(count_rows(db.connection(), "page_properties"), 0);
    assert_eq!(
        without_properties(after),
        before,
        "every row of every table is carried over"
    );
    assert_eq!(
        count_rows(db.connection(), "fts_blocks WHERE kind = 0"),
        count_rows(db.connection(), "fts_blocks"),
        "the rows the text index had are text rows"
    );

    // The table and its indexes are there, and every query answers for an empty table.
    let indexes = index_names(db.connection(), "page_properties");
    for index in [
        "page_properties_by_option",
        "page_properties_by_text",
        "page_properties_option_row",
        "page_properties_page",
        "page_properties_scalar",
    ] {
        assert!(
            indexes.contains(&index.to_owned()),
            "{index} in {indexes:?}"
        );
    }
    assert!(
        db.page_properties_of(page_id(V2_PAGE_HOME))
            .unwrap()
            .is_empty()
    );
    assert!(
        db.page_properties_pages(
            space_id(V2_PERSONAL),
            property_id(V10_PROP_TAGS),
            &PropertyMatch::HasValue
        )
        .unwrap()
        .is_empty()
    );

    // It takes rows on the migrated file, next to the text index the pages already have, and a
    // replacement of a page's text keeps the property rows.
    db.page_properties_replace(space_id(V2_PERSONAL), page_id(V2_PAGE_HOME), &v10_home())
        .unwrap();
    assert_eq!(
        db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(),
        v10_home()
    );
    db.fts_replace_properties(
        page_id(V2_PAGE_HOME),
        &[FtsBlock {
            block_id: block_id(V10_PROP_TAGS),
            text: "urgent".to_owned(),
        }],
    )
    .unwrap();
    db.fts_replace_page(page_id(V2_PAGE_HOME), "Home", &[])
        .unwrap();
    assert_eq!(
        found(&db, "urgent"),
        [(page_id(V2_PAGE_HOME), Some(block_id(V10_PROP_TAGS)))]
    );
}

#[test]
fn the_v10_fixture_lists_the_rows_of_each_page_in_the_order_they_were_written() {
    on_v10_db_and_tx(|q| {
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(),
            v10_home()
        );
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_NOTES)).unwrap(),
            v10_notes()
        );
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_OLD)).unwrap(),
            v10_old(),
            "a trashed page keeps its rows"
        );
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_ROADMAP)).unwrap(),
            v10_roadmap()
        );
        assert!(
            q.page_properties_of(page_id(V2_PAGE_OLD_CHILD))
                .unwrap()
                .is_empty()
        );
        assert_eq!(q.page_properties_count(space_id(V2_PERSONAL)).unwrap(), 15);
        assert_eq!(q.page_properties_count(space_id(V2_WORK)).unwrap(), 3);
    });
}

#[test]
fn the_v10_fixture_answers_pages_by_property_for_live_pages_of_one_space() {
    on_v10_db_and_tx(|q| {
        let (home, notes, roadmap) = (
            page_id(V2_PAGE_HOME),
            page_id(V2_PAGE_NOTES),
            page_id(V2_PAGE_ROADMAP),
        );
        let personal =
            |property, matching: PropertyMatch<'_>| v10_pages(q, V2_PERSONAL, property, &matching);
        // Tags: the trashed page ("later") is never listed; titles are "Home" and "Notes".
        assert_eq!(
            personal(V10_PROP_TAGS, PropertyMatch::HasValue),
            [home, notes]
        );
        assert_eq!(
            personal(
                V10_PROP_TAGS,
                PropertyMatch::Option(option_id(V10_OPT_URGENT))
            ),
            [home, notes]
        );
        assert_eq!(
            personal(
                V10_PROP_TAGS,
                PropertyMatch::Option(option_id(V10_OPT_WORK))
            ),
            [home]
        );
        assert!(
            personal(
                V10_PROP_TAGS,
                PropertyMatch::Option(option_id(V10_OPT_LATER))
            )
            .is_empty(),
            "only the trashed page has it"
        );
        assert_eq!(
            personal(
                V10_PROP_TAGS,
                PropertyMatch::OptionSet(&[option_id(V10_OPT_URGENT), option_id(V10_OPT_WORK)])
            ),
            [home]
        );
        assert_eq!(
            personal(
                V10_PROP_TAGS,
                PropertyMatch::OptionSet(&[option_id(V10_OPT_URGENT)])
            ),
            [notes]
        );
        // The other kinds.
        assert_eq!(personal(V10_PROP_COST, PropertyMatch::Number(12.5)), [home]);
        assert_eq!(personal(V10_PROP_DONE, PropertyMatch::Bool(true)), [home]);
        assert_eq!(
            personal(V10_PROP_DONE, PropertyMatch::NotChecked),
            [notes],
            "the live page with its checkbox off; the unset ones are in the list too"
        );
        assert_eq!(
            personal(
                V10_PROP_DUE,
                PropertyMatch::DateBetween {
                    from: Some("2026-10-08"),
                    to: None
                }
            ),
            [notes]
        );
        assert_eq!(
            personal(
                V10_PROP_DUE,
                PropertyMatch::Date {
                    date: "2026-10-07",
                    time: Some("14:30")
                }
            ),
            [home]
        );
        assert_eq!(
            personal(
                V10_PROP_STAGE,
                PropertyMatch::Option(option_id(V10_OPT_IN_PROGRESS))
            ),
            [home]
        );
        assert_eq!(
            personal(V10_PROP_SUMMARY, PropertyMatch::TextContains("CAFÉ")),
            [home, notes],
            "ignoring case, accents kept; ordered by title (Home, Notes)"
        );
        assert_eq!(
            personal(V10_PROP_SUMMARY, PropertyMatch::Text("café notes")),
            [notes]
        );
        assert_eq!(
            personal(
                V10_PROP_AREA,
                PropertyMatch::Option(option_id(V10_OPT_ELSEWHERE))
            ),
            [notes]
        );
        // The Work space has its own page and its own rows.
        assert_eq!(
            v10_pages(q, V2_WORK, V10_PROP_LINK, &PropertyMatch::HasValue),
            [roadmap]
        );
        assert!(personal(V10_PROP_LINK, PropertyMatch::HasValue).is_empty());
        assert_eq!(
            v10_pages(
                q,
                V2_WORK,
                V10_PROP_SUMMARY,
                &PropertyMatch::Text("Ship it")
            ),
            [roadmap]
        );
    });
}

#[test]
fn restoring_the_trashed_page_of_the_v10_fixture_lists_it_again() {
    let (_dir, copy) = copy_of(FIXTURE_V10, "v10.db");
    let db = open_plain(&copy);
    let tags = || {
        db.page_properties_pages(
            space_id(V2_PERSONAL),
            property_id(V10_PROP_TAGS),
            &PropertyMatch::Option(option_id(V10_OPT_LATER)),
        )
        .unwrap()
    };
    assert!(tags().is_empty());
    let old = db.pages_index_get(page_id(V2_PAGE_OLD)).unwrap().unwrap();
    db.pages_index_upsert(&[PageIndexRow {
        trashed_at: None,
        ..old
    }])
    .unwrap();
    assert_eq!(tags(), [page_id(V2_PAGE_OLD)]);
}

#[test]
fn the_v10_fixture_finds_property_text_through_the_search_and_keeps_the_page_count() {
    let (_dir, copy) = copy_of(FIXTURE_V10, "v10.db");
    let db = open_plain(&copy);
    let hits = |query: &str| {
        db.fts_search(&SearchParams {
            query,
            space_id: None,
            titles_only: false,
            limit: 50,
            any_word: false,
            edited_after: None,
        })
        .unwrap()
    };
    // "urgent": the Tags row of Home ("work, urgent") and of Notes, and nothing else.
    let urgent = hits("urgent");
    let mut found_pages: Vec<(PageId, bool)> = urgent
        .iter()
        .map(|hit| (hit.page_id, hit.property))
        .collect();
    found_pages.sort();
    assert_eq!(
        found_pages,
        [
            (page_id(V2_PAGE_HOME), true),
            (page_id(V2_PAGE_NOTES), true)
        ]
    );
    assert!(
        urgent
            .iter()
            .all(|hit| hit.block_id == Some(block_id(V10_PROP_TAGS)))
    );
    // A text value and a URL are found by their words; the trashed page's "later" is not.
    assert_eq!(hits("roadmap").iter().filter(|hit| hit.property).count(), 1);
    assert!(hits("later").is_empty(), "the trashed page is never hit");
    assert_eq!(hits("elsewhere").len(), 1);
    // A titles-only search never sees a property row.
    let titles = db
        .fts_search(&SearchParams {
            query: "urgent",
            space_id: None,
            titles_only: true,
            limit: 50,
            any_word: false,
            edited_after: None,
        })
        .unwrap();
    assert!(titles.is_empty());

    // The property rows are no pages: the page count is the v9 file's.
    let (_older, v9_copy) = copy_of(FIXTURE_V9, "v9.db");
    let v9 = open_plain(&v9_copy);
    for space in [V2_PERSONAL, V2_WORK] {
        assert_eq!(
            db.fts_page_count(space_id(space)).unwrap(),
            v9.fts_page_count(space_id(space)).unwrap(),
            "{space}"
        );
    }
    db.connection()
        .execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
}

#[test]
fn the_v10_fixture_loses_both_kinds_of_rows_with_a_purged_page() {
    on_v10_db_and_tx(|q| {
        let home = page_id(V2_PAGE_HOME);
        q.page_properties_delete_pages(&[home]).unwrap();
        q.fts_delete_page(home).unwrap();
        assert!(q.page_properties_of(home).unwrap().is_empty());
        assert_eq!(q.page_properties_count(space_id(V2_PERSONAL)).unwrap(), 7);
        let hits = q
            .fts_search(&SearchParams {
                query: "urgent",
                space_id: None,
                titles_only: false,
                limit: 50,
                any_word: false,
                edited_after: None,
            })
            .unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.page_id).collect::<Vec<_>>(),
            [page_id(V2_PAGE_NOTES)]
        );
        // A rebuild of the space clears and refills its rows.
        q.page_properties_clear_space(space_id(V2_PERSONAL))
            .unwrap();
        assert_eq!(q.page_properties_count(space_id(V2_PERSONAL)).unwrap(), 0);
        assert_eq!(q.page_properties_count(space_id(V2_WORK)).unwrap(), 3);
        q.page_properties_replace(space_id(V2_PERSONAL), home, &v10_home())
            .unwrap();
        assert_eq!(q.page_properties_of(home).unwrap(), v10_home());
        q.page_properties_clear().unwrap();
        assert_eq!(q.page_properties_count(space_id(V2_WORK)).unwrap(), 0);
    });
}

#[test]
fn a_failing_transaction_rolls_the_property_rows_back() {
    let (_dir, copy) = copy_of(FIXTURE_V10, "v10.db");
    let mut db = open_plain(&copy);
    let home = page_id(V2_PAGE_HOME);
    let result: Result<(), StorageError> = db.transaction(|tx| {
        tx.page_properties_replace(space_id(V2_PERSONAL), home, &[])?;
        tx.fts_replace_properties(home, &[])?;
        Err(StorageError::InvalidData("stop".to_owned()))
    });
    assert!(result.is_err());
    assert_eq!(db.page_properties_of(home).unwrap(), v10_home());
    assert_eq!(count_rows(db.connection(), "fts_blocks WHERE kind = 1"), 11);
}

// --- v11: the files and checklist kinds of page properties (ADR-0026, 2026-10-07) -------------

#[test]
fn migrating_the_v10_fixture_rebuilds_the_property_table_and_keeps_every_row() {
    let (_dir, copy) = copy_of(FIXTURE_V10, "v10.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "10", "a schema v10 file");
        dump_all(&conn)
    };
    assert_eq!(rows_of(&before, "page_properties").len(), 18);
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());

    // Every other table is as it was; the property rows have the three new columns, empty.
    let others = |dump: &[(String, Vec<String>)]| {
        dump.iter()
            .filter(|(name, _)| name != "page_properties")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(others(&after), others(&before));
    let widened: Vec<String> = rows_of(&before, "page_properties")
        .iter()
        .map(|row| format!("{row}|NULL|NULL|NULL"))
        .collect();
    assert_eq!(rows_of(&after, "page_properties"), widened.as_slice());

    // A page's rows read back in the order they were written, and the queries answer as before.
    assert_eq!(
        db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(),
        v10_home()
    );
    assert_eq!(
        db.page_properties_of(page_id(V2_PAGE_ROADMAP)).unwrap(),
        v10_roadmap()
    );
    assert_eq!(
        db.page_properties_pages(
            space_id(V2_PERSONAL),
            property_id(V10_PROP_TAGS),
            &PropertyMatch::Option(option_id(V10_OPT_URGENT)),
        )
        .unwrap(),
        [page_id(V2_PAGE_HOME), page_id(V2_PAGE_NOTES)]
    );

    // The indexes are there again, with the two of the new columns.
    let indexes = index_names(db.connection(), "page_properties");
    for index in [
        "page_properties_attachment_row",
        "page_properties_by_attachment",
        "page_properties_by_option",
        "page_properties_by_text",
        "page_properties_option_row",
        "page_properties_page",
        "page_properties_scalar",
    ] {
        assert!(
            indexes.contains(&index.to_owned()),
            "{index} in {indexes:?}"
        );
    }

    // The table takes the new kinds, next to the rows it had.
    let file = AttachmentId::from_bytes([0xf1; 16]);
    let files = PagePropertyRow {
        attachment_id: Some(file),
        ..v10_row(V10_PROP_LINK, PropertyKind::Files)
    };
    let checklist = PagePropertyRow {
        value_number: Some(2.0),
        value_total: Some(5),
        ..v10_row(
            "b09e0000-0000-4000-8000-000000000008",
            PropertyKind::Checklist,
        )
    };
    let mut rows = v10_home();
    rows.extend([files.clone(), checklist.clone()]);
    // The page already shows attachments in its blocks (the v5 content).
    let mut referenced = db
        .attachment_ids_referenced_by(page_id(V2_PAGE_HOME))
        .unwrap();
    assert!(!referenced.is_empty());
    referenced.push(file);
    referenced.sort_unstable();
    db.page_properties_replace(space_id(V2_PERSONAL), page_id(V2_PAGE_HOME), &rows)
        .unwrap();
    assert_eq!(db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(), rows);
    assert_eq!(db.attachment_reference_count(file).unwrap(), 1);
    assert_eq!(
        db.attachment_ids_referenced_by(page_id(V2_PAGE_HOME))
            .unwrap(),
        referenced,
        "the blocks and the property value together"
    );
    assert_eq!(
        db.page_properties_pages_with_attachment(file).unwrap(),
        [(space_id(V2_PERSONAL), page_id(V2_PAGE_HOME))]
    );
    // A kind this version does not know is still refused.
    assert!(
        db.connection()
            .execute_batch(
                "INSERT INTO page_properties (space_id, page_id, property_id, kind) \
                 VALUES (zeroblob(16), zeroblob(16), zeroblob(16), 'rollup')"
            )
            .is_err()
    );
}

/// A schema v11 file (`make_fixture_db -- v11`): the v10 content plus five `files` rows, three
/// `checklist` rows and seven property rows (`kind = 1`) of the text index.
const FIXTURE_V11: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v11.db");

// The ids of `fixtures/db/v11.db` that v10 did not have, as written by `examples/make_fixture_db.rs`.
const V11_PROP_FILES: &str = "b09e0000-0000-4000-8000-000000000008";
const V11_PROP_TODO: &str = "b09e0000-0000-4000-8000-000000000009";

fn v11_file(property: &str, attachment: &str) -> PagePropertyRow {
    PagePropertyRow {
        attachment_id: Some(attachment_id(attachment)),
        ..v10_row(property, PropertyKind::Files)
    }
}

fn v11_checklist(done: u32, total: u32) -> PagePropertyRow {
    PagePropertyRow {
        value_number: Some(f64::from(done)),
        value_total: Some(total),
        ..v10_row(V11_PROP_TODO, PropertyKind::Checklist)
    }
}

/// Home: the rows of the v10 content, then the report and the photo (in the order of the list),
/// then the checklist, two of three items done.
fn v11_home() -> Vec<PagePropertyRow> {
    let mut rows = v10_home();
    rows.extend([
        v11_file(V11_PROP_FILES, V5_ATT_REPORT),
        v11_file(V11_PROP_FILES, V5_ATT_PHOTO),
        v11_checklist(2, 3),
    ]);
    rows
}

fn v11_notes() -> Vec<PagePropertyRow> {
    let mut rows = v10_notes();
    rows.push(v11_file(V11_PROP_FILES, V5_ATT_PHOTO));
    rows
}

fn v11_old() -> Vec<PagePropertyRow> {
    let mut rows = v10_old();
    rows.extend([v11_file(V11_PROP_FILES, V5_ATT_SKETCH), v11_checklist(0, 2)]);
    rows
}

fn v11_roadmap() -> Vec<PagePropertyRow> {
    let mut rows = v10_roadmap();
    rows.extend([v11_file(V11_PROP_FILES, V5_ATT_EMPTY), v11_checklist(1, 1)]);
    rows
}

/// What the v11 scenarios need of a connection or a transaction.
trait V11Queries: PagePropertyQueries + TextIndexQueries + AttachmentQueries {}
impl<T: PagePropertyQueries + TextIndexQueries + AttachmentQueries> V11Queries for T {}

/// Runs `scenario` over a fresh copy of the v11 fixture opened on a bare `Db`, and again over
/// another copy inside a transaction.
fn on_v11_db_and_tx(scenario: impl Fn(&dyn V11Queries)) {
    let (_dir, copy) = copy_of(FIXTURE_V11, "v11.db");
    scenario(&open_plain(&copy));
    let (_dir, copy) = copy_of(FIXTURE_V11, "v11.db");
    let mut db = open_plain(&copy);
    db.transaction(|tx| {
        scenario(tx);
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_v11_fixture_opens_at_version_11_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V11).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V11, "v11.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "11",
        "the fixture is a schema v11 database"
    );

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "page_properties"), 26);
    assert_eq!(
        count_rows(db.connection(), "page_properties WHERE kind = 'files'"),
        5
    );
    assert_eq!(
        count_rows(db.connection(), "page_properties WHERE kind = 'checklist'"),
        3
    );
    assert_eq!(count_rows(db.connection(), "fts_blocks WHERE kind = 1"), 18);
    assert_eq!(count_rows(db.connection(), "date_mentions"), 5);
    drop(db);
    if SCHEMA_VERSION == 11 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v11_fixture_is_the_migrated_v10_fixture_plus_the_rows_of_the_new_kinds() {
    let (_older, v10_copy) = copy_of(FIXTURE_V10, "v10.db");
    let migrated = dump_all(open_plain(&v10_copy).connection());
    let (_dir, copy) = copy_of(FIXTURE_V11, "v11.db");
    let current = dump_all(open_plain(&copy).connection());

    // Every table but the two the new kinds write to (and the word index that follows
    // `fts_blocks` through its triggers) is the same, row for row.
    let others = |dump: &[(String, Vec<String>)]| {
        dump.iter()
            .filter(|(name, _)| name != "page_properties" && !name.starts_with("fts"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(others(&current), others(&migrated));
    // Those two hold everything the migrated file held, and the new rows besides.
    for (table, added) in [("page_properties", 8), ("fts_blocks", 7)] {
        let before = rows_of(&migrated, table);
        let after = rows_of(&current, table);
        assert_eq!(after.len(), before.len() + added, "{table}");
        for row in before {
            assert!(after.contains(row), "{table}: {row}");
        }
    }
}

#[test]
fn the_v11_fixture_lists_the_files_and_checklist_rows_in_the_order_they_were_written() {
    on_v11_db_and_tx(|q| {
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(),
            v11_home()
        );
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_NOTES)).unwrap(),
            v11_notes()
        );
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_OLD)).unwrap(),
            v11_old(),
            "a trashed page keeps its rows"
        );
        assert_eq!(
            q.page_properties_of(page_id(V2_PAGE_ROADMAP)).unwrap(),
            v11_roadmap()
        );
        assert_eq!(q.page_properties_count(space_id(V2_PERSONAL)).unwrap(), 21);
        assert_eq!(q.page_properties_count(space_id(V2_WORK)).unwrap(), 5);
    });
}

#[test]
fn the_v11_fixture_answers_the_queries_of_the_new_kinds() {
    on_v11_db_and_tx(|q| {
        let pages = |space: &str, property: &str, matching: &PropertyMatch<'_>| {
            q.page_properties_pages(space_id(space), property_id(property), matching)
                .unwrap()
        };
        // The live pages, by title: Home and Notes (the trashed page is left out).
        let has = PropertyMatch::HasValue;
        assert_eq!(
            pages(V2_PERSONAL, V11_PROP_FILES, &has),
            [page_id(V2_PAGE_HOME), page_id(V2_PAGE_NOTES)]
        );
        assert_eq!(
            pages(V2_PERSONAL, V11_PROP_TODO, &has),
            [page_id(V2_PAGE_HOME)]
        );
        assert_eq!(
            pages(V2_WORK, V11_PROP_TODO, &has),
            [page_id(V2_PAGE_ROADMAP)]
        );
        // Two of three items done is open; the Roadmap's one item is done; the trashed page
        // (none of two) is never listed.
        assert_eq!(
            pages(
                V2_PERSONAL,
                V11_PROP_TODO,
                &PropertyMatch::ChecklistComplete(false)
            ),
            [page_id(V2_PAGE_HOME)]
        );
        assert!(
            pages(
                V2_PERSONAL,
                V11_PROP_TODO,
                &PropertyMatch::ChecklistComplete(true)
            )
            .is_empty()
        );
        assert_eq!(
            pages(
                V2_WORK,
                V11_PROP_TODO,
                &PropertyMatch::ChecklistComplete(true)
            ),
            [page_id(V2_PAGE_ROADMAP)]
        );
        assert!(
            pages(
                V2_WORK,
                V11_PROP_TODO,
                &PropertyMatch::ChecklistComplete(false)
            )
            .is_empty()
        );

        // The attachments the rows attach.
        let photo = attachment_id(V5_ATT_PHOTO);
        let report = attachment_id(V5_ATT_REPORT);
        assert_eq!(
            q.page_properties_attachments_of(&[page_id(V2_PAGE_HOME)])
                .unwrap(),
            [photo, report]
        );
        assert_eq!(
            q.page_properties_attachments_in_space(space_id(V2_PERSONAL))
                .unwrap(),
            [photo, report, attachment_id(V5_ATT_SKETCH)]
        );
        assert_eq!(
            q.page_properties_attachments_in_space(space_id(V2_WORK))
                .unwrap(),
            [attachment_id(V5_ATT_EMPTY)]
        );
        assert_eq!(
            q.page_properties_pages_with_attachment(photo).unwrap(),
            [
                (space_id(V2_PERSONAL), page_id(V2_PAGE_HOME)),
                (space_id(V2_PERSONAL), page_id(V2_PAGE_NOTES))
            ]
        );

        // References: the blocks of the v5 content plus the files rows. The photo is shown by two
        // pages and listed by two; the archive and the late file have none; the purged
        // attachment (no index row) is still shown by a block.
        let references = |attachment: &str| {
            q.attachment_reference_count(attachment_id(attachment))
                .unwrap()
        };
        assert_eq!(references(V5_ATT_PHOTO), 4);
        assert_eq!(references(V5_ATT_REPORT), 2);
        assert_eq!(references(V5_ATT_EMPTY), 2);
        assert_eq!(references(V5_ATT_SKETCH), 2, "a trashed page still counts");
        assert_eq!(references(V5_ATT_ARCHIVE), 0);
        assert_eq!(references(V5_ATT_LATE), 0);
        assert_eq!(references(V5_ATT_PURGED), 1);
        assert_eq!(
            q.attachment_ids_referenced_by(page_id(V2_PAGE_HOME))
                .unwrap(),
            [photo, report],
            "the photo is a block and a value of Home, the report a value only"
        );
        assert_eq!(
            q.attachment_ids_referenced_by(page_id(V2_PAGE_ROADMAP))
                .unwrap(),
            [attachment_id(V5_ATT_EMPTY), attachment_id(V5_ATT_PURGED)]
        );
    });
}

#[test]
fn the_v11_fixture_finds_a_file_name_and_an_item_text_as_property_hits() {
    on_v11_db_and_tx(|q| {
        // The property hits of a search, sorted: (page, the property, true).
        let property_hits = |query: &str| {
            let mut hits: Vec<(PageId, Option<BlockId>)> = q
                .fts_search(&SearchParams {
                    query,
                    space_id: None,
                    titles_only: false,
                    limit: 50,
                    any_word: false,
                    edited_after: None,
                })
                .unwrap()
                .into_iter()
                .filter(|hit| hit.property)
                .map(|hit| (hit.page_id, hit.block_id))
                .collect();
            hits.sort();
            hits
        };
        let files = Some(block_id(V11_PROP_FILES));
        let todo = Some(block_id(V11_PROP_TODO));
        assert_eq!(
            property_hits("photo"),
            [
                (page_id(V2_PAGE_HOME), files),
                (page_id(V2_PAGE_NOTES), files)
            ]
        );
        assert_eq!(property_hits("ada"), [(page_id(V2_PAGE_HOME), todo)]);
        assert_eq!(property_hits("release"), [(page_id(V2_PAGE_ROADMAP), todo)]);
        assert_eq!(property_hits("empty"), [(page_id(V2_PAGE_ROADMAP), files)]);
        assert!(
            property_hits("sketch").is_empty(),
            "the trashed page is never hit"
        );
    });
}

// --- v12: the place kind of page properties (ADR-0026, 2026-10-08) -------------------------------

#[test]
fn migrating_the_v11_fixture_rebuilds_the_property_table_and_keeps_every_row() {
    let (_dir, copy) = copy_of(FIXTURE_V11, "v11.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "11", "a schema v11 file");
        dump_all(&conn)
    };
    assert_eq!(rows_of(&before, "page_properties").len(), 26);
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());

    // Every other table is as it was, row for row and in the same order; the property rows have
    // the new related-page column, empty.
    let others = |dump: &[(String, Vec<String>)]| {
        dump.iter()
            .filter(|(name, _)| name != "page_properties")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(others(&after), others(&before));
    let widened: Vec<String> = rows_of(&before, "page_properties")
        .iter()
        .map(|row| format!("{row}|NULL"))
        .collect();
    assert_eq!(rows_of(&after, "page_properties"), widened.as_slice());
    assert_eq!(
        db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(),
        v11_home()
    );
    assert_eq!(
        db.page_properties_of(page_id(V2_PAGE_ROADMAP)).unwrap(),
        v11_roadmap()
    );

    // The indexes are there again.
    let indexes = index_names(db.connection(), "page_properties");
    for index in [
        "page_properties_attachment_row",
        "page_properties_by_attachment",
        "page_properties_by_option",
        "page_properties_by_text",
        "page_properties_option_row",
        "page_properties_page",
        "page_properties_scalar",
    ] {
        assert!(
            indexes.contains(&index.to_owned()),
            "{index} in {indexes:?}"
        );
    }

    // The table takes the new kind, next to the rows it had; the computed kinds have no rows.
    let place = PagePropertyRow {
        value_text: Some("Lisbon".to_owned()),
        ..v10_row(V12_PROP_PLACE, PropertyKind::Place)
    };
    let mut rows = v11_home();
    rows.push(place);
    db.page_properties_replace(space_id(V2_PERSONAL), page_id(V2_PAGE_HOME), &rows)
        .unwrap();
    assert_eq!(db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(), rows);
    for kind in [
        "created_time",
        "last_edited_time",
        "created_by",
        "last_edited_by",
    ] {
        assert!(
            db.connection()
                .execute(
                    "INSERT INTO page_properties (space_id, page_id, property_id, kind) \
                     VALUES (zeroblob(16), zeroblob(16), zeroblob(16), ?1)",
                    [kind],
                )
                .is_err(),
            "{kind} has no rows"
        );
    }
}

/// A schema v12 file (`make_fixture_db -- v12`): the v11 content plus three `place` rows and their
/// property rows (`kind = 1`) of the text index.
const FIXTURE_V12: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v12.db");

// The id of `fixtures/db/v12.db` that v11 did not have, as written by `examples/make_fixture_db.rs`.
const V12_PROP_PLACE: &str = "b09e0000-0000-4000-8000-00000000000a";

fn v12_place(name: &str) -> PagePropertyRow {
    PagePropertyRow {
        value_text: Some(name.to_owned()),
        ..v10_row(V12_PROP_PLACE, PropertyKind::Place)
    }
}

#[test]
fn the_v12_fixture_opens_at_version_12_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V12).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V12, "v12.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "12",
        "the fixture is a schema v12 database"
    );
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "page_properties"), 29);
    assert_eq!(
        count_rows(db.connection(), "page_properties WHERE kind = 'place'"),
        3
    );
    assert_eq!(count_rows(db.connection(), "fts_blocks WHERE kind = 1"), 21);
    drop(db);
    if SCHEMA_VERSION == 12 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v12_fixture_is_the_v11_fixture_plus_the_rows_of_the_place_kind() {
    let (_older, v11_copy) = copy_of(FIXTURE_V11, "v11.db");
    let migrated = dump_all(open_plain(&v11_copy).connection());
    let (_dir, copy) = copy_of(FIXTURE_V12, "v12.db");
    let current = dump_all(open_plain(&copy).connection());
    let others = |dump: &[(String, Vec<String>)]| {
        dump.iter()
            .filter(|(name, _)| name != "page_properties" && !name.starts_with("fts"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(others(&current), others(&migrated));
    for (table, added) in [("page_properties", 3), ("fts_blocks", 3)] {
        let before = rows_of(&migrated, table);
        let after = rows_of(&current, table);
        assert_eq!(after.len(), before.len() + added, "{table}");
        for row in before {
            assert!(after.contains(row), "{table}: {row}");
        }
    }
}

#[test]
fn the_v12_fixture_answers_the_queries_of_the_place_kind() {
    let (_dir, copy) = copy_of(FIXTURE_V12, "v12.db");
    let db = open_plain(&copy);
    let personal = space_id(V2_PERSONAL);
    let property = property_id(V12_PROP_PLACE);
    let mut home = v11_home();
    home.push(v12_place("Caf\u{e9} de Flore, Paris"));
    assert_eq!(db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(), home);
    let mut notes = v11_notes();
    notes.push(v12_place("Home office"));
    assert_eq!(
        db.page_properties_of(page_id(V2_PAGE_NOTES)).unwrap(),
        notes
    );

    // HasValue and the text matches read the name; pages come back by title.
    assert_eq!(
        db.page_properties_pages(personal, property, &PropertyMatch::HasValue)
            .unwrap(),
        [page_id(V2_PAGE_HOME), page_id(V2_PAGE_NOTES)]
    );
    assert_eq!(
        db.page_properties_pages(personal, property, &PropertyMatch::TextContains("paris"))
            .unwrap(),
        [page_id(V2_PAGE_HOME)]
    );
    assert_eq!(
        db.page_properties_pages(personal, property, &PropertyMatch::TextContains("OFFICE"))
            .unwrap(),
        [page_id(V2_PAGE_NOTES)]
    );
    // The roadmap is in the other space.
    assert_eq!(
        db.page_properties_pages(
            space_id(V2_WORK),
            property,
            &PropertyMatch::TextContains("lisbon")
        )
        .unwrap(),
        [page_id(V2_PAGE_ROADMAP)]
    );
}

// --- v13: the relation kind of page properties (ADR-0027) ------------------------------------------

/// A schema v13 file (`make_fixture_db -- v13`): the v12 content plus the rows of two `relation`
/// properties (Notes and the trashed "Old ideas" on Home, Home on Notes) and their property rows
/// (`kind = 1`) of the text index.
const FIXTURE_V13: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v13.db");

// The ids of `fixtures/db/v13.db` that v12 did not have, as written by `examples/make_fixture_db.rs`.
const V13_PROP_RELATION: &str = "b09e0000-0000-4000-8000-00000000000b";
const V13_PROP_RELATION_BACK: &str = "b09e0000-0000-4000-8000-00000000000c";

fn v13_related(property: &str, page: &str) -> PagePropertyRow {
    PagePropertyRow {
        related_page_id: Some(page_id(page)),
        account_id: None,
        ..v10_row(property, PropertyKind::Relation)
    }
}

#[test]
fn migrating_the_v12_fixture_rebuilds_the_property_table_and_keeps_every_row() {
    let (_dir, copy) = copy_of(FIXTURE_V12, "v12.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "12", "a schema v12 file");
        dump_all(&conn)
    };
    assert_eq!(rows_of(&before, "page_properties").len(), 29);
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all(db.connection());

    // Every other table is as it was; the property rows have the new column, empty.
    let others = |dump: &[(String, Vec<String>)]| {
        dump.iter()
            .filter(|(name, _)| name != "page_properties")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(others(&after), others(&before));
    let widened: Vec<String> = rows_of(&before, "page_properties")
        .iter()
        .map(|row| format!("{row}|NULL"))
        .collect();
    assert_eq!(rows_of(&after, "page_properties"), widened.as_slice());

    // The indexes are there again, with the one the relation rows use.
    let indexes = index_names(db.connection(), "page_properties");
    for index in [
        "page_properties_attachment_row",
        "page_properties_by_attachment",
        "page_properties_by_option",
        "page_properties_by_related",
        "page_properties_by_text",
        "page_properties_option_row",
        "page_properties_page",
        "page_properties_related_row",
        "page_properties_scalar",
    ] {
        assert!(
            indexes.contains(&index.to_owned()),
            "{index} in {indexes:?}"
        );
    }

    // The table takes the new kind next to the rows it had, one row per related page; the rollup
    // and the computed kinds have no rows.
    let mut rows = v11_home();
    rows.push(PagePropertyRow {
        value_text: Some("Caf\u{e9} de Flore, Paris".to_owned()),
        ..v10_row(V12_PROP_PLACE, PropertyKind::Place)
    });
    rows.push(v13_related(V13_PROP_RELATION, V2_PAGE_NOTES));
    rows.push(v13_related(V13_PROP_RELATION, V2_PAGE_OLD));
    db.page_properties_replace(space_id(V2_PERSONAL), page_id(V2_PAGE_HOME), &rows)
        .unwrap();
    assert_eq!(db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(), rows);
    for kind in [
        "rollup",
        "created_time",
        "last_edited_time",
        "created_by",
        "last_edited_by",
    ] {
        assert!(
            db.connection()
                .execute(
                    "INSERT INTO page_properties (space_id, page_id, property_id, kind) \
                     VALUES (zeroblob(16), zeroblob(16), zeroblob(16), ?1)",
                    [kind],
                )
                .is_err(),
            "{kind} has no rows"
        );
    }
}

#[test]
fn the_v13_fixture_opens_at_version_13_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V13).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V13, "v13.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "13",
        "the fixture is a schema v13 database"
    );
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "page_properties"), 32);
    assert_eq!(
        count_rows(db.connection(), "page_properties WHERE kind = 'relation'"),
        3
    );
    assert_eq!(count_rows(db.connection(), "fts_blocks WHERE kind = 1"), 23);
    drop(db);
    if SCHEMA_VERSION == 13 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v13_fixture_is_the_migrated_v12_fixture_plus_the_rows_of_the_relation_kind() {
    let (_older, v12_copy) = copy_of(FIXTURE_V12, "v12.db");
    let migrated = dump_all(open_plain(&v12_copy).connection());
    let (_dir, copy) = copy_of(FIXTURE_V13, "v13.db");
    let current = dump_all(open_plain(&copy).connection());
    let others = |dump: &[(String, Vec<String>)]| {
        dump.iter()
            .filter(|(name, _)| name != "page_properties" && !name.starts_with("fts"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(others(&current), others(&migrated));
    for (table, added) in [("page_properties", 3), ("fts_blocks", 2)] {
        let before = rows_of(&migrated, table);
        let after = rows_of(&current, table);
        assert_eq!(after.len(), before.len() + added, "{table}");
        for row in before {
            assert!(after.contains(row), "{table}: {row}");
        }
    }
}

#[test]
fn the_v13_fixture_answers_the_queries_of_the_relation_kind() {
    let (_dir, copy) = copy_of(FIXTURE_V13, "v13.db");
    let db = open_plain(&copy);
    let personal = space_id(V2_PERSONAL);
    let forward = property_id(V13_PROP_RELATION);
    let back = property_id(V13_PROP_RELATION_BACK);

    // The rows of a page, in the order of the value.
    let home = db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap();
    let related: Vec<&PagePropertyRow> = home
        .iter()
        .filter(|row| row.kind == PropertyKind::Relation)
        .collect();
    assert_eq!(
        related,
        [
            &v13_related(V13_PROP_RELATION, V2_PAGE_NOTES),
            &v13_related(V13_PROP_RELATION, V2_PAGE_OLD)
        ]
    );
    let notes = db.page_properties_of(page_id(V2_PAGE_NOTES)).unwrap();
    assert!(notes.contains(&v13_related(V13_PROP_RELATION_BACK, V2_PAGE_HOME)));

    let relates = |property, page: Option<&str>, paired: Option<PropertyId>| {
        db.page_properties_pages(
            personal,
            property,
            &PropertyMatch::Related {
                page: page.map(page_id),
                paired,
            },
        )
        .unwrap()
    };
    // Who lists Notes? Home, directly. Who lists the trashed page? Home too (the page that is
    // listed may be trashed; only the pages in the answer are live).
    assert_eq!(
        relates(forward, Some(V2_PAGE_NOTES), None),
        [page_id(V2_PAGE_HOME)]
    );
    assert_eq!(
        relates(forward, Some(V2_PAGE_OLD), None),
        [page_id(V2_PAGE_HOME)]
    );
    // Notes stores Home on the back side: it is found through the back property directly.
    assert_eq!(
        relates(forward, Some(V2_PAGE_HOME), None),
        Vec::<PageId>::new()
    );
    assert_eq!(
        relates(back, Some(V2_PAGE_HOME), None),
        [page_id(V2_PAGE_NOTES)]
    );
    // Read through the pair, Home's forward value is seen from the other side even when Notes
    // stored nothing (the symmetric view of ADR-0027).
    db.page_properties_replace(space_id(V2_PERSONAL), page_id(V2_PAGE_NOTES), &[])
        .unwrap();
    assert_eq!(
        relates(back, Some(V2_PAGE_HOME), None),
        Vec::<PageId>::new()
    );
    assert_eq!(
        relates(back, Some(V2_PAGE_HOME), Some(forward)),
        [page_id(V2_PAGE_NOTES)]
    );
    // Has a value: the pages with a related page that exists in the space.
    assert_eq!(relates(forward, None, None), [page_id(V2_PAGE_HOME)]);
    assert_eq!(relates(back, None, Some(forward)), [page_id(V2_PAGE_NOTES)]);
    assert_eq!(relates(back, None, None), Vec::<PageId>::new());
    // The property row of the text index holds the titles, and a search finds the page.
    let hits = db
        .fts_search(&SearchParams {
            query: "Old ideas",
            space_id: None,
            titles_only: false,
            limit: 10,
            any_word: false,
            edited_after: None,
        })
        .unwrap();
    assert!(
        hits.iter().any(|hit| hit.page_id == page_id(V2_PAGE_HOME)),
        "{hits:?}"
    );
}

#[test]
fn a_page_lists_a_related_page_once_and_the_table_enforces_it() {
    let (_dir, copy) = copy_of(FIXTURE_V13, "v13.db");
    let db = open_plain(&copy);
    let rows = vec![
        v13_related(V13_PROP_RELATION, V2_PAGE_NOTES),
        v13_related(V13_PROP_RELATION, V2_PAGE_NOTES),
        v13_related(V13_PROP_RELATION, V2_PAGE_OLD),
    ];
    db.page_properties_replace(space_id(V2_PERSONAL), page_id(V2_PAGE_HOME), &rows)
        .unwrap();
    assert_eq!(
        db.page_properties_of(page_id(V2_PAGE_HOME)).unwrap(),
        [rows[0].clone(), rows[2].clone()]
    );
}

// --- v14: the image icons of pages (ADR-0029) -----------------------------------------------------

/// A schema v14 file (`make_fixture_db -- v14`): the v13 content plus three `icon_refs` rows (Home has
/// the photo as its icon, the trashed "Old ideas" the sketch, the Roadmap the purged attachment).
const FIXTURE_V14: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v14.db");

/// The `icon_refs` rows of the v14 fixture as (page, space, attachment), as `dump` shows them.
fn v14_icon_lines() -> Vec<String> {
    let hex = |text: &str| text.replace('-', "");
    let mut lines: Vec<String> = [
        (V2_PAGE_ROADMAP, V2_WORK, V5_ATT_PURGED),
        (V2_PAGE_HOME, V2_PERSONAL, V5_ATT_PHOTO),
        (V2_PAGE_OLD, V2_PERSONAL, V5_ATT_SKETCH),
    ]
    .iter()
    .map(|(page, space, attachment)| format!("{}|{}|{}", hex(page), hex(space), hex(attachment)))
    .collect();
    lines.sort();
    lines
}

#[test]
fn migrating_the_v13_fixture_adds_the_empty_icon_table_and_keeps_every_row() {
    let (_dir, copy) = copy_of(FIXTURE_V13, "v13.db");
    let before = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "13", "a schema v13 file");
        assert!(
            conn.prepare("SELECT * FROM icon_refs").is_err(),
            "v13 has no icon table"
        );
        dump_all_with_icon_refs(&conn)
    };
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all_with_icon_refs(db.connection());

    // Every table is as it was, and the migration fills the new one with nothing: a database from
    // before v14 has no image icon (the core compares the table with the documents at open).
    assert_eq!(
        after
            .iter()
            .filter(|(name, _)| name != "icon_refs")
            .cloned()
            .collect::<Vec<_>>(),
        before
    );
    assert!(rows_of(&after, "icon_refs").is_empty());
    let columns: Vec<(String, String)> = db
        .connection()
        .prepare("SELECT name, type FROM pragma_table_info('icon_refs') ORDER BY cid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            ("page_id".to_owned(), "BLOB".to_owned()),
            ("space_id".to_owned(), "BLOB".to_owned()),
            ("attachment_id".to_owned(), "BLOB".to_owned())
        ]
    );
    let indexes = index_names(db.connection(), "icon_refs");
    for index in ["icon_refs_by_attachment", "icon_refs_by_space"] {
        assert!(
            indexes.contains(&index.to_owned()),
            "{index} in {indexes:?}"
        );
    }

    // The table takes a row; a write of the page replaces its rows.
    let home = page_id(V2_PAGE_HOME);
    let personal = space_id(V2_PERSONAL);
    db.icon_refs_set(personal, &[(home, vec![attachment_id(V5_ATT_PHOTO)])])
        .unwrap();
    db.icon_refs_set(personal, &[(home, vec![attachment_id(V5_ATT_SKETCH)])])
        .unwrap();
    assert_eq!(
        db.icon_refs_of_space(personal).unwrap(),
        [(home, attachment_id(V5_ATT_SKETCH))]
    );
}

#[test]
fn the_v14_fixture_opens_at_version_14_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V14).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V14, "v14.db");
    assert_eq!(
        pragma(&Connection::open(&copy).unwrap(), "user_version"),
        "14",
        "the fixture is a schema v14 database"
    );
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "icon_refs"), 3);
    assert_eq!(count_rows(db.connection(), "page_properties"), 32);
    drop(db);
    if SCHEMA_VERSION == 14 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v14_fixture_is_the_migrated_v13_fixture_plus_the_icon_rows() {
    let (_older, v13_copy) = copy_of(FIXTURE_V13, "v13.db");
    let migrated = dump_all_with_icon_refs(open_plain(&v13_copy).connection());
    let (_dir, copy) = copy_of(FIXTURE_V14, "v14.db");
    let current = dump_all_with_icon_refs(open_plain(&copy).connection());
    assert_eq!(migrated.len(), current.len());
    for ((name, before), (current_name, after)) in migrated.iter().zip(&current) {
        assert_eq!(name, current_name);
        if name == "icon_refs" {
            assert!(before.is_empty());
            assert_eq!(after, &v14_icon_lines());
        } else {
            assert_eq!(after, before, "{name}");
        }
    }
}

#[test]
fn the_v14_fixture_counts_the_icons_as_references_and_lists_them_by_space_and_page() {
    let (_dir, copy) = copy_of(FIXTURE_V14, "v14.db");
    let db = open_plain(&copy);
    let personal = space_id(V2_PERSONAL);
    let work = space_id(V2_WORK);
    let (home, old, roadmap) = (
        page_id(V2_PAGE_HOME),
        page_id(V2_PAGE_OLD),
        page_id(V2_PAGE_ROADMAP),
    );
    let (photo, sketch, purged) = (
        attachment_id(V5_ATT_PHOTO),
        attachment_id(V5_ATT_SKETCH),
        attachment_id(V5_ATT_PURGED),
    );

    // The rows by space, in page id order.
    assert_eq!(
        db.icon_refs_of_space(personal).unwrap(),
        [(home, photo), (old, sketch)]
    );
    assert_eq!(db.icon_refs_of_space(work).unwrap(), [(roadmap, purged)]);

    // Each icon is one reference more than the blocks and the file values give: the purged
    // attachment has no index row and is still referenced by the roadmap's block and icon.
    let counts =
        |db: &Db| [photo, sketch, purged].map(|id| db.attachment_reference_count(id).unwrap());
    let with_icons = counts(&db);
    assert_eq!(db.attachment_reference_count(purged).unwrap(), 2);
    assert_eq!(
        db.attachment_ids_referenced_by(roadmap).unwrap(),
        [attachment_id(V5_ATT_EMPTY), purged],
        "the icon is listed with the blocks, once"
    );
    assert!(
        db.attachment_ids_referenced_by(home)
            .unwrap()
            .contains(&photo)
    );
    assert!(
        db.attachment_ids_referenced_by(old)
            .unwrap()
            .contains(&sketch)
    );

    // A trashed page keeps its icon; removing the row drops one reference and only one.
    db.icon_refs_delete_pages(&[old]).unwrap();
    assert_eq!(
        counts(&db),
        [with_icons[0], with_icons[1] - 1, with_icons[2]]
    );
    db.icon_refs_replace_space(personal, &[]).unwrap();
    assert_eq!(
        counts(&db),
        [with_icons[0] - 1, with_icons[1] - 1, with_icons[2]]
    );
    assert_eq!(db.icon_refs_of_space(work).unwrap(), [(roadmap, purged)]);
    db.icon_refs_clear().unwrap();
    assert_eq!(db.attachment_reference_count(purged).unwrap(), 1);
    assert!(
        db.icon_refs_attachments_of(&[home, old, roadmap])
            .unwrap()
            .is_empty()
    );
}

// --- v15: the semantic index and the MCP clients and audit (ADR-0037) ----------------------------

#[test]
fn migrating_the_v14_fixture_adds_the_empty_ai_tables_and_marks_every_page_with_text_dirty() {
    let (_dir, copy) = copy_of(FIXTURE_V14, "v14.db");
    let (before, with_text) = {
        let conn = Connection::open(&copy).unwrap();
        assert_eq!(pragma(&conn, "user_version"), "14", "a schema v14 file");
        assert!(
            conn.prepare("SELECT * FROM ai_dirty").is_err(),
            "v14 has no semantic index"
        );
        let mut with_text: Vec<String> = conn
            .prepare("SELECT DISTINCT page_id FROM fts_blocks")
            .unwrap()
            .query_map([], |row| {
                Ok(row
                    .get::<_, Vec<u8>>(0)?
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>())
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        with_text.sort();
        (dump_all_with_icon_refs(&conn), with_text)
    };
    assert!(!with_text.is_empty(), "the fixture has indexed pages");
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let after = dump_all_with_ai(db.connection());

    // Every table is as it was; the migration adds five tables, four of them empty, and the pages
    // that have rows in the text index start waiting to be indexed.
    assert_eq!(
        after
            .iter()
            .filter(|(name, _)| !V15_TABLES.contains(&name.as_str()))
            .cloned()
            .collect::<Vec<_>>(),
        before
    );
    for table in ["ai_pages", "ai_chunks", "mcp_clients", "mcp_audit"] {
        assert!(rows_of(&after, table).is_empty(), "{table}");
    }
    assert_eq!(rows_of(&after, "ai_dirty"), with_text.as_slice());
    assert_eq!(
        db.ai_dirty_count().unwrap(),
        u32::try_from(with_text.len()).unwrap()
    );

    // The new queries read the migrated file: the live pages, the body of a page with text, and
    // the status numbers (nothing is stored, every live page with text waits).
    let live = db.ai_live_pages(None).unwrap();
    assert!(!live.is_empty());
    assert!(
        live.iter()
            .any(|(page, _)| !db.ai_page_body(*page).unwrap().is_empty()),
        "a live page has body text"
    );
    let counts = db.ai_index_counts(Some("model")).unwrap();
    assert_eq!(counts.pages_total, u32::try_from(live.len()).unwrap());
    assert_eq!(
        (counts.pages_indexed, counts.chunks, counts.model),
        (0, 0, None)
    );
    assert!(counts.dirty > 0 && counts.dirty <= counts.pages_total);

    // The triggers keep the table in step with the text index from here on.
    db.ai_clear().unwrap();
    let page = live[0].0;
    db.fts_set_title(page, "A new title").unwrap();
    assert_eq!(db.ai_dirty_after(None, 10).unwrap(), [page]);
}

// --- v15, v16 and v17: AI and MCP, the covers, and shared spaces (M13) ------------------------------

/// A schema v15 file (`make_fixture_db -- v15`): the v14 content plus the semantic index of two pages
/// (three chunks), two MCP clients and three audit rows.
const FIXTURE_V15: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v15.db");

/// A schema v16 file (`make_fixture_db -- v16`): the v15 content plus two `icon_refs` rows for covers.
const FIXTURE_V16: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v16.db");

/// A schema v17 file (`make_fixture_db -- v17`): the v16 content as the migration leaves it, plus what a
/// device that shares the Work space holds.
const FIXTURE_V17: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v17.db");

// The ids and stand-ins of `fixtures/db/v15.db` to `v17.db`, as written by `examples/make_fixture_db.rs`
// (the helpers below repeat its formulas).
const V15_MCP_CLIENT_DESKTOP: &str = "c11e0000000040008000000000000001";
const V15_MCP_CLIENT_NEW: &str = "c11e0000000040008000000000000002";
const V17_SHARED_FEED: &str = "5ead0000-0000-4000-8000-000000000001";
const V17_CO_MEMBER: &str = "ac000000-0000-4000-8000-000000000002";
const V17_CO_MEMBER_DEVICE: &str = "de71ce00-0000-4000-8000-000000000003";
const V17_OUTSIDER: &str = "ac000000-0000-4000-8000-000000000003";

/// The id as `dump` shows it: 32 lower-case hex characters.
fn hex_id(text: &str) -> String {
    text.replace('-', "")
}

fn account_id(text: &str) -> AccountId {
    text.parse().unwrap()
}

fn feed_id(text: &str) -> FeedId {
    text.parse().unwrap()
}

/// The personal feed of the fixtures: their account id.
fn account_feed() -> FeedId {
    FeedId::from_account(account_id(V4_ACCOUNT_ID))
}

/// A fixture file (copied to a scratch directory) and its connection before anything migrated it.
fn raw_conn(copy: &Path, user_version: &str) -> Connection {
    let conn = Connection::open(copy).unwrap();
    assert_eq!(
        pragma(&conn, "user_version"),
        user_version,
        "a schema v{user_version} file"
    );
    conn
}

// ---- v15 ----

#[test]
fn the_v15_fixture_opens_at_version_15_and_holds_the_ai_and_mcp_rows() {
    let original = fs::read(FIXTURE_V15).unwrap();
    assert_eq!(&original[..16], b"SQLite format 3\0");
    let (_dir, copy) = copy_of(FIXTURE_V15, "v15.db");
    let conn = raw_conn(&copy, "15");
    for (table, rows) in [
        ("ai_pages", 2),
        ("ai_chunks", 3),
        ("mcp_clients", 2),
        ("mcp_audit", 3),
    ] {
        assert_eq!(count_rows(&conn, table), rows, "{table}");
    }
    // Every page with text starts dirty, and the two indexed pages left the table again.
    let with_text = count_rows(&conn, "(SELECT DISTINCT page_id FROM fts_blocks)");
    assert_eq!(count_rows(&conn, "ai_dirty"), with_text - 2);
    drop(conn);

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let clients = db.mcp_clients_list().unwrap();
    assert_eq!(
        clients.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [V15_MCP_CLIENT_DESKTOP, V15_MCP_CLIENT_NEW],
        "listed by creation time, oldest first"
    );
    assert_eq!(db.mcp_audit_count().unwrap(), 3);
    drop(db);
    if SCHEMA_VERSION == 15 {
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn migrating_the_v15_fixture_keeps_every_row_of_every_table() {
    let (_dir, copy) = copy_of(FIXTURE_V15, "v15.db");
    let before = dump_all_with_ai(&raw_conn(&copy, "15"));
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    // `icon_refs` was rebuilt with a new key at v16 and the sync tables at v17: the rows are the same.
    assert_eq!(dump_all_with_ai(db.connection()), before);
    assert_eq!(count_rows(db.connection(), "ai_chunks"), 3);
}

// ---- v16 ----

#[test]
fn the_v16_fixture_holds_the_cover_rows_next_to_the_icons() {
    let original = fs::read(FIXTURE_V16).unwrap();
    assert_eq!(&original[..16], b"SQLite format 3\0");
    let (_dir, copy) = copy_of(FIXTURE_V16, "v16.db");
    let conn = raw_conn(&copy, "16");
    assert_eq!(count_rows(&conn, "icon_refs"), 5, "three icons, two covers");
    assert_eq!(count_rows(&conn, "ai_pages"), 2);
    drop(conn);

    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let (home, notes, old) = (
        page_id(V2_PAGE_HOME),
        page_id(V2_PAGE_NOTES),
        page_id(V2_PAGE_OLD),
    );
    let (photo, report, sketch) = (
        attachment_id(V5_ATT_PHOTO),
        attachment_id(V5_ATT_REPORT),
        attachment_id(V5_ATT_SKETCH),
    );
    // One page with an image icon and an image cover has two rows.
    assert_eq!(
        db.icon_refs_of_space(space_id(V2_PERSONAL)).unwrap(),
        [(home, photo), (home, report), (notes, photo), (old, sketch)]
    );
    drop(db);
    if SCHEMA_VERSION == 16 {
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn migrating_the_v16_fixture_gives_the_sync_rows_the_personal_feed_and_keeps_every_other_row() {
    let (_dir, copy) = copy_of(FIXTURE_V16, "v16.db");
    let (before, raw_before) = {
        let conn = raw_conn(&copy, "16");
        assert!(
            conn.prepare("SELECT feed FROM sync_heads").is_err(),
            "the sync tables of v16 have no feed"
        );
        let raw: Vec<(&str, Vec<String>)> = V17_FEED_TABLES
            .iter()
            .map(|table| (*table, dump_raw(&conn, table)))
            .collect();
        (dump_every_table(&conn), raw)
    };
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string()
    );

    // Every table of v16 has exactly the rows it had (the four sync tables seen without their new
    // column), and the tables of v17 are new and empty.
    assert_eq!(dump_all_with_ai(db.connection()), before);
    for table in V17_TABLES {
        assert_eq!(count_rows(db.connection(), table), 0, "{table}");
    }

    // With the column: the same rows, each in the personal feed, which is the account id.
    let feed = hex_id(V4_ACCOUNT_ID);
    for (table, lines) in &raw_before {
        let mut expected: Vec<String> = lines.iter().map(|line| format!("{feed}|{line}")).collect();
        expected.sort();
        assert_eq!(dump_raw(db.connection(), table), expected, "{table}");
        assert!(!expected.is_empty(), "{table} had rows");
    }
    assert_eq!(
        [
            count_rows(db.connection(), "sync_doc_state"),
            count_rows(db.connection(), "sync_seen"),
            count_rows(db.connection(), "sync_heads"),
            count_rows(db.connection(), "sync_pending"),
        ],
        [7, 5, 2, 3]
    );

    // The queries of the personal feed answer what the v6 fixture was built with, with and without
    // the feed named; another feed has nothing.
    assert_eq!(db.sync_personal_feed().unwrap(), account_feed());
    assert_eq!(db.sync_config_all().unwrap(), v6_config(), "not per feed");
    assert_eq!(db.sync_doc_state_all().unwrap(), v6_doc_states());
    assert_eq!(
        db.sync_doc_state_all_in(account_feed()).unwrap(),
        v6_doc_states()
    );
    assert_eq!(db.sync_heads_all().unwrap(), v6_heads());
    assert_eq!(db.sync_heads_all_in(account_feed()).unwrap(), v6_heads());
    assert_eq!(db.sync_pending_all().unwrap(), v6_pending());
    assert_eq!(
        db.sync_pending_all_in(account_feed()).unwrap(),
        v6_pending()
    );
    for (doc, device, last_seq) in v6_seen() {
        assert_eq!(db.sync_seen_get(doc, device).unwrap(), Some(last_seq));
        assert_eq!(
            db.sync_seen_get_in(account_feed(), doc, device).unwrap(),
            Some(last_seq)
        );
    }
    let other = feed_id(V17_SHARED_FEED);
    assert!(db.sync_doc_state_all_in(other).unwrap().is_empty());
    assert!(db.sync_heads_all_in(other).unwrap().is_empty());
    assert_eq!(db.sync_pending_count_in(other).unwrap(), 0);
    assert!(
        db.sync_feed_list().unwrap().is_empty(),
        "the core adds the personal feed's row once it opens"
    );
}

#[test]
fn migrating_a_v16_file_without_an_account_id_parks_the_sync_rows_under_the_zero_feed() {
    let (_dir, copy) = copy_of(FIXTURE_V16, "v16.db");
    raw_conn(&copy, "16")
        .execute("DELETE FROM meta WHERE key = 'account_id'", [])
        .unwrap();
    let db = open_plain(&copy);
    for table in V17_FEED_TABLES {
        for line in dump_raw(db.connection(), table) {
            assert!(line.starts_with(&"0".repeat(32)), "{table}: {line}");
        }
    }
    // Without an account the personal feed is the zero feed and finds them...
    assert_eq!(db.sync_personal_feed().unwrap(), UNRESOLVED_PERSONAL_FEED);
    assert_eq!(db.sync_doc_state_all().unwrap(), v6_doc_states());
    // ...and once the account exists it does not: those rows belong to no feed of it.
    db.meta_set("account_id", V4_ACCOUNT_ID).unwrap();
    assert!(db.sync_doc_state_all().unwrap().is_empty());
    assert!(db.sync_heads_all().unwrap().is_empty());
    assert_eq!(db.sync_pending_count().unwrap(), 0);
}

#[test]
fn migrating_the_v16_fixture_twice_changes_nothing_and_a_reopened_file_keeps_its_feeds() {
    let (_dir, copy) = copy_of(FIXTURE_V16, "v16.db");
    let first = {
        let db = open_plain(&copy);
        db.sync_feed_put(&v17_personal_feed()).unwrap();
        dump_raw_every_table(db.connection())
    };
    let db = open_plain(&copy);
    assert_eq!(dump_raw_every_table(db.connection()), first);
    assert_eq!(db.sync_feed_list().unwrap(), [v17_personal_feed()]);
}

// ---- v17 ----

/// Every table with every column, the feed of the sync tables included.
fn dump_raw_every_table(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let names: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names
        .into_iter()
        .filter(|name| !V21_TABLES.contains(&name.as_str()))
        .map(|name| {
            let rows = dump_raw(conn, &name);
            (name, rows)
        })
        .collect()
}

/// The `sync_feeds` row of the personal feed of the v17 fixture.
fn v17_personal_feed() -> SyncFeedRow {
    SyncFeedRow {
        feed: account_feed(),
        kind: FeedKind::Personal,
        space_id: None,
        cursor: 42,
        chain_hash: Some(ramp(0x70, 32).try_into().unwrap()),
        max_covers: 33,
        chain_history: Some(ramp(0x44, 50)),
        fork: false,
        server_epoch: Some(ramp(0x60, 16).try_into().unwrap()),
    }
}

/// The `sync_feeds` row of the shared feed of the Work space.
fn v17_shared_feed() -> SyncFeedRow {
    SyncFeedRow {
        feed: feed_id(V17_SHARED_FEED),
        kind: FeedKind::Shared,
        space_id: Some(space_id(V2_WORK)),
        cursor: 17,
        chain_hash: Some(ramp(0x30, 32).try_into().unwrap()),
        max_covers: 9,
        chain_history: None,
        fork: true,
        server_epoch: Some(ramp(0x60, 16).try_into().unwrap()),
    }
}

/// The stand-in for a quarantined item: `KEN1`, the cursor, NUL and high bytes, a filler.
fn v17_rejected_item(cursor: u8) -> Vec<u8> {
    let mut item = b"KEN1".to_vec();
    item.extend([cursor, 0x00, 0xFF, 0x80]);
    item.extend(ramp(0x30 + cursor, 30 + usize::from(cursor)));
    item
}

/// The stand-in for a membership entry: `KME1`, the sequence number, NUL and high bytes, a filler.
fn v17_membership_entry(seq: u8) -> Vec<u8> {
    let mut entry = b"KME1".to_vec();
    entry.extend([seq, 0x00, 0xFF, 0x80]);
    entry.extend(ramp(0xD0 + seq, 40 + 8 * usize::from(seq)));
    entry
}

/// The stand-in for an encoded version vector.
fn v17_end_vv(seed: u8) -> Vec<u8> {
    let mut vv = vec![0x02, 0x00, 0xFF];
    vv.extend(ramp(seed, 12));
    vv
}

fn v17_state(
    doc: &str,
    (pushed, next): (i64, u64),
    snapshot_covers: Option<u64>,
    (entries, bytes): (u64, u64),
    remote_snapshot_covers: Option<u64>,
) -> SyncDocState {
    SyncDocState {
        doc_id: doc_id(doc),
        pushed_up_to_seq: pushed,
        next_device_seq: next,
        snapshot_covers,
        entries_since_snapshot: entries,
        bytes_since_snapshot: bytes,
        remote_snapshot_covers,
    }
}

#[test]
fn the_v17_fixture_opens_at_version_17_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V17).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V17, "v17.db");
    let conn = raw_conn(&copy, "17");
    for (table, rows) in [
        ("sync_feeds", 2),
        ("foreign_device_logs", 4),
        ("sync_rejected", 3),
        ("sync_rejected_dropped", 1),
        ("sync_entry_marks", 3),
        ("membership_entries", 3),
        ("membership_state", 1),
        ("sync_doc_state", 7 + 3),
        ("sync_seen", 5 + 3),
        ("sync_heads", 2 + 2),
        ("sync_pending", 3 + 1),
    ] {
        assert_eq!(count_rows(&conn, table), rows, "{table}");
    }
    drop(conn);
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    if SCHEMA_VERSION == 17 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v17_fixture_is_the_migrated_v16_fixture_plus_the_shared_space_rows() {
    let (_older, v16_copy) = copy_of(FIXTURE_V16, "v16.db");
    let migrated = dump_raw_every_table(open_plain(&v16_copy).connection());
    let (_dir, copy) = copy_of(FIXTURE_V17, "v17.db");
    let current = dump_raw_every_table(open_plain(&copy).connection());
    assert_eq!(
        migrated.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        current.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "the same tables"
    );
    let shared = hex_id(V17_SHARED_FEED);
    for ((name, before), (_, after)) in migrated.iter().zip(&current) {
        if V17_FEED_TABLES.contains(&name.as_str()) {
            // The rows of the personal feed are the migrated ones; the shared feed adds its own.
            let (own, added): (Vec<&String>, Vec<&String>) = after
                .iter()
                .partition(|line| !line.starts_with(&format!("{shared}|")));
            assert_eq!(own, before.iter().collect::<Vec<_>>(), "{name}");
            assert!(!added.is_empty(), "{name} has rows in the shared feed");
        } else if V17_TABLES.contains(&name.as_str()) {
            assert!(before.is_empty(), "{name} is new, empty, after a migration");
            assert!(!after.is_empty(), "{name} has rows in the fixture");
        } else {
            assert_eq!(after, before, "{name}");
        }
    }
}

#[test]
fn the_v17_fixture_reads_back_through_the_feed_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V17, "v17.db");
    let db = open_plain(&copy);
    let shared = feed_id(V17_SHARED_FEED);
    let (own, co_device) = (device_id(V6_OWN_DEVICE), device_id(V17_CO_MEMBER_DEVICE));
    let roadmap = doc_id(V2_PAGE_ROADMAP);
    let work_structure = doc_id(V2_DOC_WORK);

    // The feeds: the personal one first, though it was written second.
    assert_eq!(
        db.sync_feed_list().unwrap(),
        [v17_personal_feed(), v17_shared_feed()]
    );
    assert_eq!(db.sync_feed_get(shared).unwrap(), Some(v17_shared_feed()));
    assert_eq!(
        db.sync_feed_get(feed_id("5ead0000-0000-4000-8000-0000000000ff"))
            .unwrap(),
        None
    );
    // The personal row repeats the settings the core moves out of `sync_config`.
    let personal = v17_personal_feed();
    assert_eq!(
        hex(&personal.chain_hash.unwrap()),
        db.sync_config_get("chain_hash").unwrap().unwrap()
    );
    assert_eq!(
        personal.cursor.to_string(),
        db.sync_config_get("remote_cursor").unwrap().unwrap()
    );

    // The bookkeeping of the shared feed: the keyring and the roadmap have a row in each feed.
    assert_eq!(
        db.sync_doc_state_all_in(shared).unwrap(),
        [
            v17_state(
                "00000000-0000-0000-0000-000000000000",
                (0, 2),
                None,
                (0, 0),
                None
            ),
            v17_state(V2_PAGE_ROADMAP, (1, 2), None, (1, 64), None),
            v17_state(V2_DOC_WORK, (3, 4), Some(11), (2, 300), Some(11)),
        ]
    );
    assert_eq!(
        db.sync_doc_state_get_in(shared, roadmap)
            .unwrap()
            .unwrap()
            .pushed_up_to_seq,
        1
    );
    assert_eq!(
        db.sync_doc_state_get_in(account_feed(), roadmap)
            .unwrap()
            .unwrap()
            .pushed_up_to_seq,
        0,
        "the roadmap's row of the personal feed is the migrated one"
    );
    assert_eq!(db.sync_doc_state_all().unwrap(), v6_doc_states());
    assert_eq!(
        db.sync_seen_get_in(shared, roadmap, co_device).unwrap(),
        Some(5)
    );
    assert_eq!(db.sync_seen_get_in(shared, roadmap, own).unwrap(), Some(2));
    assert_eq!(
        db.sync_seen_get_in(shared, KEYRING_DOC_ID, co_device)
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        db.sync_seen_get_in(shared, work_structure, co_device)
            .unwrap(),
        None
    );
    assert_eq!(
        db.sync_heads_all_in(shared).unwrap(),
        [
            SyncHeadRow {
                device_id: own,
                cursor: 17,
                chain_hash: ramp(0x30, 32).try_into().unwrap(),
                at_ms: 1_760_000_800_000,
                signed: v6_signed_head(1, 17),
                seen_at: 1_760_000_800_500,
            },
            SyncHeadRow {
                device_id: co_device,
                cursor: 16,
                chain_hash: ramp(0xA0, 32).try_into().unwrap(),
                at_ms: 1_760_000_790_000,
                signed: v6_signed_head(3, 16),
                seen_at: 1_760_000_801_000,
            },
        ]
    );
    assert_eq!(
        db.sync_heads_all().unwrap(),
        v6_heads(),
        "the personal feed's heads"
    );
    assert_eq!(
        db.sync_pending_all_in(shared).unwrap(),
        [SyncPendingRow {
            cursor: 15,
            doc_id: roadmap,
            bytes: v6_pending_entry(15),
            reason: 1,
            received_at: 1_760_000_802_000,
        }]
    );
    assert_eq!(db.sync_pending_all().unwrap(), v6_pending());

    // The co-members' device logs: by account, in sequence order, as fetched.
    assert_eq!(
        db.foreign_log_accounts().unwrap(),
        [account_id(V17_CO_MEMBER), account_id(V17_OUTSIDER)]
    );
    let rows = db
        .foreign_log_after(account_id(V17_CO_MEMBER), None)
        .unwrap();
    assert_eq!(
        rows,
        (0..3u8)
            .map(|seq| ForeignLogRow {
                account_id: account_id(V17_CO_MEMBER),
                seq: u64::from(seq),
                entry: v4_log_row(seq).entry,
                fetched_at: 1_760_000_803_000,
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        db.foreign_log_after(account_id(V17_CO_MEMBER), Some(1))
            .unwrap(),
        rows[2..]
    );
    assert_eq!(
        db.foreign_log_after(account_id(V17_OUTSIDER), None)
            .unwrap()
            .len(),
        1
    );

    // The quarantine of the shared feed, newest first, with the count of what was dropped.
    let item = |cursor: u8, doc: DocId, account: &str, reason: RejectReason, at: i64| RejectedRow {
        feed: shared,
        cursor: u64::from(cursor),
        doc_id: doc,
        account_id: account_id(account),
        device_id: co_device,
        reason,
        bytes: v17_rejected_item(cursor),
        at,
    };
    assert_eq!(
        db.rejected_list(shared, 10).unwrap(),
        [
            item(
                16,
                roadmap,
                V17_OUTSIDER,
                RejectReason::SnapshotNotDominant,
                1_760_000_810_000
            ),
            item(
                14,
                KEYRING_DOC_ID,
                V17_CO_MEMBER,
                RejectReason::NotAMember,
                1_760_000_806_000
            ),
            item(
                12,
                roadmap,
                V17_CO_MEMBER,
                RejectReason::RoleForbids,
                1_760_000_805_000
            ),
        ]
    );
    assert_eq!(db.rejected_list(shared, 1).unwrap().len(), 1);
    assert_eq!(
        db.rejected_count_by_account(shared).unwrap(),
        [
            (account_id(V17_CO_MEMBER), 2 + 4),
            (account_id(V17_OUTSIDER), 1)
        ],
        "stored and dropped, by account id"
    );
    assert!(db.rejected_list(account_feed(), 10).unwrap().is_empty());

    // The end version vectors of the last applied entries.
    let mark = |doc: DocId, device: DeviceId, seed: u8, cursor: u64| EntryMarkRow {
        doc_id: doc,
        device_id: device,
        end_vv: v17_end_vv(seed),
        cursor,
    };
    assert_eq!(
        db.entry_marks_for_doc(roadmap).unwrap(),
        [
            mark(roadmap, own, 0x21, 17),
            mark(roadmap, co_device, 0x11, 16)
        ]
    );
    assert_eq!(
        db.entry_mark_get(work_structure, co_device).unwrap(),
        Some(mark(work_structure, co_device, 0x31, 13))
    );
    assert_eq!(db.entry_mark_get(work_structure, own).unwrap(), None);

    // The membership log: two entries found in the feed, one that was not accepted yet.
    let entry = |seq: u8, cursor: u64| MembershipEntryRow {
        feed: shared,
        seq: u64::from(seq),
        cursor,
        bytes: v17_membership_entry(seq),
    };
    assert_eq!(
        db.membership_entries_list(shared, None).unwrap(),
        [
            entry(1, 3),
            entry(2, 9),
            entry(3, MEMBERSHIP_PENDING_CURSOR)
        ]
    );
    assert_eq!(
        db.membership_entries_list(shared, Some(1)).unwrap(),
        [entry(2, 9), entry(3, MEMBERSHIP_PENDING_CURSOR)]
    );
    let state = db.membership_state_get(shared).unwrap().unwrap();
    assert_eq!(state.seq, 2);
    assert_eq!(state.head_hash.to_vec(), ramp(0xE0, 32));
    assert!(state.state.starts_with(b"KMS1"));
    assert_eq!(db.membership_state_get(account_feed()).unwrap(), None);
}

#[test]
fn forgetting_the_shared_feed_of_the_v17_fixture_leaves_the_personal_feed_as_it_was() {
    let (_older, v16_copy) = copy_of(FIXTURE_V16, "v16.db");
    let migrated = dump_raw_every_table(open_plain(&v16_copy).connection());
    let (_dir, copy) = copy_of(FIXTURE_V17, "v17.db");
    let db = open_plain(&copy);
    let shared = feed_id(V17_SHARED_FEED);

    db.sync_feed_delete(shared).unwrap();
    assert_eq!(db.sync_feed_list().unwrap(), [v17_personal_feed()]);
    assert!(db.sync_doc_state_all_in(shared).unwrap().is_empty());
    assert!(db.sync_heads_all_in(shared).unwrap().is_empty());
    assert_eq!(db.sync_pending_count_in(shared).unwrap(), 0);
    assert!(db.rejected_list(shared, 10).unwrap().is_empty());
    assert!(db.rejected_count_by_account(shared).unwrap().is_empty());
    assert!(db.membership_entries_list(shared, None).unwrap().is_empty());
    assert_eq!(db.membership_state_get(shared).unwrap(), None);
    // The marks of the documents the shared feed touched went with it.
    for table in ["sync_entry_marks", "sync_rejected_dropped"] {
        assert_eq!(count_rows(db.connection(), table), 0, "{table}");
    }
    // The device logs of the co-members belong to accounts, not to the feed.
    assert_eq!(db.foreign_log_accounts().unwrap().len(), 2);

    // Everything else is the migrated v16 content (and the personal feed's row).
    let after = dump_raw_every_table(db.connection());
    for ((name, before), (_, rows)) in migrated.iter().zip(&after) {
        if V17_TABLES.contains(&name.as_str()) {
            continue;
        }
        assert_eq!(rows, before, "{name}");
    }
    assert_eq!(db.sync_doc_state_all().unwrap(), v6_doc_states());
}

#[test]
fn a_new_server_epoch_resets_every_feed_of_the_v17_fixture() {
    let (_dir, copy) = copy_of(FIXTURE_V17, "v17.db");
    let mut db = open_plain(&copy);
    let shared = feed_id(V17_SHARED_FEED);
    db.transaction(|tx| {
        tx.sync_doc_state_reset_all()?;
        tx.sync_seen_clear()?;
        tx.sync_heads_clear()?;
        tx.sync_pending_clear()
    })
    .unwrap();
    for feed in [account_feed(), shared] {
        let rows = db.sync_doc_state_all_in(feed).unwrap();
        assert!(!rows.is_empty(), "no row is removed");
        for row in rows {
            assert_eq!(row.pushed_up_to_seq, 0);
            assert_eq!(row.remote_snapshot_covers, None);
            assert_eq!(row.entries_since_snapshot, 0);
        }
        assert!(db.sync_heads_all_in(feed).unwrap().is_empty());
        assert_eq!(db.sync_pending_count_in(feed).unwrap(), 0);
    }
    assert_eq!(count_rows(db.connection(), "sync_seen"), 0);
    // The device sequence of the shared feed's keyring is the device's own: it stays.
    assert_eq!(
        db.sync_doc_state_get_in(shared, KEYRING_DOC_ID)
            .unwrap()
            .unwrap()
            .next_device_seq,
        2
    );
    // What is not bookkeeping of the server stays: the feeds' rows, the membership log, the quarantine.
    assert_eq!(db.sync_feed_list().unwrap().len(), 2);
    assert_eq!(db.membership_entries_list(shared, None).unwrap().len(), 3);
    assert_eq!(db.rejected_list(shared, 10).unwrap().len(), 3);

    // Only the shared feed (a feed that was lost): the personal feed is untouched.
    let (_dir2, copy2) = copy_of(FIXTURE_V17, "v17.db");
    let db = open_plain(&copy2);
    db.sync_doc_state_reset_in(shared).unwrap();
    db.sync_heads_clear_in(shared).unwrap();
    assert_eq!(db.sync_doc_state_all().unwrap(), v6_doc_states());
    assert_eq!(db.sync_heads_all().unwrap(), v6_heads());
    assert!(db.sync_heads_all_in(shared).unwrap().is_empty());
}

#[test]
fn the_v17_fixture_survives_a_reopen_in_an_encrypted_file() {
    // The feed tables take part in the encrypted file like every other table: nothing readable at rest.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v17-encrypted.db");
    let key = db_key(5);
    {
        let db = open_encrypted(&path, &key);
        db.sync_feed_put(&v17_personal_feed()).unwrap();
        db.sync_feed_put(&v17_shared_feed()).unwrap();
        db.membership_entries_put(&[MembershipEntryRow {
            feed: feed_id(V17_SHARED_FEED),
            seq: 1,
            cursor: 3,
            bytes: b"membership-marker-bytes".to_vec(),
        }])
        .unwrap();
        db.rejected_put(&RejectedRow {
            feed: feed_id(V17_SHARED_FEED),
            cursor: 1,
            doc_id: KEYRING_DOC_ID,
            account_id: account_id(V17_CO_MEMBER),
            device_id: device_id(V17_CO_MEMBER_DEVICE),
            reason: RejectReason::NotAMember,
            bytes: b"rejected-marker-bytes".to_vec(),
            at: 1,
        })
        .unwrap();
    }
    for file in files_in(dir.path()) {
        let raw = fs::read(&file).unwrap();
        assert!(!contains(&raw, b"membership-marker-bytes"), "{file:?}");
        assert!(!contains(&raw, b"rejected-marker-bytes"), "{file:?}");
    }
    let db = open_encrypted(&path, &key);
    assert_eq!(db.sync_feed_list().unwrap().len(), 2);
    assert_eq!(
        db.membership_entries_list(feed_id(V17_SHARED_FEED), None)
            .unwrap()[0]
            .bytes,
        b"membership-marker-bytes"
    );
    assert_eq!(
        db.rejected_list(feed_id(V17_SHARED_FEED), 1).unwrap()[0].bytes,
        b"rejected-marker-bytes"
    );
}

// ---- v18 ----

/// A schema v18 file (`make_fixture_db -- v18`): the v17 content as the migration leaves it, plus what a
/// device that shares the Work space and comments holds (comment records, their words, an inbox, person
/// mentions, a Person value and a `reminderFor`).
const FIXTURE_V18: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/db/v18.db");

// The ids of `fixtures/db/v18.db` that v17 did not have, as written by `examples/make_fixture_db.rs`.
const V18_COMMENT_ROOT: &str = "c0de0000-0000-4000-8000-000000000001";
const V18_COMMENT_REPLY: &str = "c0de0000-0000-4000-8000-000000000002";
const V18_COMMENT_SECOND: &str = "c0de0000-0000-4000-8000-000000000003";
const V18_COMMENT_HOME: &str = "c0de0000-0000-4000-8000-000000000004";
const V18_RECORDS: [&str; 7] = [
    "ec0d0000-0000-4000-8000-000000000001",
    "ec0d0000-0000-4000-8000-000000000002",
    "ec0d0000-0000-4000-8000-000000000003",
    "ec0d0000-0000-4000-8000-000000000004",
    "ec0d0000-0000-4000-8000-000000000005",
    "ec0d0000-0000-4000-8000-000000000006",
    "ec0d0000-0000-4000-8000-000000000007",
];
const V18_PROP_OWNER: &str = "b09e0000-0000-4000-8000-00000000000d";

fn v18_comment_id(text: &str) -> CommentId {
    text.parse().unwrap()
}

fn v18_record_id(text: &str) -> CommentRecordId {
    text.parse().unwrap()
}

/// The stand-in for an encoded record: `KCR1`, the number, NUL and high bytes, a filler.
fn v18_record_bytes(n: u8) -> Vec<u8> {
    let mut record = b"KCR1".to_vec();
    record.extend([n, 0x00, 0xFF, 0x80]);
    record.extend(ramp(0x90 + n, 60 + 4 * usize::from(n)));
    record
}

/// The Roadmap's records in the order `(at, record id)` gives them.
fn v18_roadmap_records() -> Vec<CommentRow> {
    let work = space_id(V2_WORK);
    let roadmap = page_id(V2_PAGE_ROADMAP);
    let co_member = account_id(V17_CO_MEMBER);
    let owner = account_id(V4_ACCOUNT_ID);
    let co_device = device_id(V17_CO_MEMBER_DEVICE);
    let own_device = device_id(V6_OWN_DEVICE);
    let row = |record: usize,
               comment: &str,
               op: u8,
               (author, device): (AccountId, DeviceId),
               rev: u32,
               at: i64,
               cursor: Option<u64>,
               role: MemberRole| CommentRow {
        record_id: v18_record_id(V18_RECORDS[record]),
        space_id: work,
        page_id: roadmap,
        // The reply and its delete are in the root's thread; the other comments start their own.
        thread_id: ThreadId::of(v18_comment_id(if comment == V18_COMMENT_REPLY {
            V18_COMMENT_ROOT
        } else {
            comment
        })),
        comment_id: v18_comment_id(comment),
        op,
        author,
        device,
        rev,
        at,
        cursor,
        author_role: role,
        record: v18_record_bytes(u8::try_from(record + 1).unwrap()),
    };
    vec![
        row(
            0,
            V18_COMMENT_ROOT,
            1,
            (co_member, co_device),
            0,
            1_760_000_810_000,
            Some(12),
            MemberRole::Commenter,
        ),
        row(
            1,
            V18_COMMENT_REPLY,
            1,
            (owner, own_device),
            0,
            1_760_000_820_000,
            Some(13),
            MemberRole::Owner,
        ),
        row(
            2,
            V18_COMMENT_ROOT,
            2,
            (co_member, co_device),
            1,
            1_760_000_830_000,
            Some(14),
            MemberRole::Commenter,
        ),
        row(
            3,
            V18_COMMENT_ROOT,
            4,
            (owner, own_device),
            1,
            1_760_000_840_000,
            Some(15),
            MemberRole::Owner,
        ),
        row(
            4,
            V18_COMMENT_REPLY,
            3,
            (owner, own_device),
            1,
            1_760_000_860_000,
            Some(16),
            MemberRole::Owner,
        ),
        row(
            5,
            V18_COMMENT_SECOND,
            1,
            (owner, own_device),
            0,
            1_760_000_870_000,
            None,
            MemberRole::Owner,
        ),
    ]
}

#[test]
fn the_v18_fixture_opens_at_version_18_and_is_left_alone_while_that_is_current() {
    let original = fs::read(FIXTURE_V18).unwrap();
    assert_eq!(
        &original[..16],
        b"SQLite format 3\0",
        "the fixture is an unencrypted SQLite file"
    );
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let conn = raw_conn(&copy, "18");
    for (table, rows) in [
        ("comments", 7),
        ("comment_fts_content", 3),
        ("notifications", 4),
        ("person_mentions", 3),
        ("date_mentions", 5),
        ("sync_feeds", 2),
    ] {
        assert_eq!(count_rows(&conn, table), rows, "{table}");
    }
    assert_eq!(
        count_rows(&conn, "page_properties WHERE kind = 'person'"),
        2,
        "the Person value of the Roadmap"
    );
    drop(conn);
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    drop(db);
    if SCHEMA_VERSION == 18 {
        // Migrating the current version is a no-op: not a single byte is written.
        assert_eq!(fs::read(&copy).unwrap(), original);
    }
}

#[test]
fn the_v18_fixture_is_the_migrated_v17_fixture_plus_the_comment_rows() {
    let (_older, v17_copy) = copy_of(FIXTURE_V17, "v17.db");
    let migrated = dump_raw_every_table(open_plain(&v17_copy).connection());
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let current = dump_raw_every_table(open_plain(&copy).connection());
    assert_eq!(
        migrated.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        current.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "the same tables"
    );
    let work = hex_id(V2_WORK);
    let roadmap = hex_id(V2_PAGE_ROADMAP);
    for ((name, before), (_, after)) in migrated.iter().zip(&current) {
        match name.as_str() {
            // The new tables: empty after a migration (the shadow tables of the full-text table
            // are internal and hold their own bookkeeping), filled in the fixture.
            "comments" | "notifications" | "person_mentions" | "comment_fts_content" => {
                assert!(before.is_empty(), "{name} is new, empty, after a migration");
                assert!(!after.is_empty(), "{name} has rows in the fixture");
            }
            name if V18_TABLES.contains(&name) => {}
            // The Person value is the only addition to the property rows.
            "page_properties" => {
                let person = |account: &str| {
                    format!(
                        "{work}|{roadmap}|{}|person|NULL|NULL|NULL|NULL|NULL|NULL|NULL|NULL|NULL|{}",
                        hex_id(V18_PROP_OWNER),
                        hex_id(account)
                    )
                };
                let mut expected = before.clone();
                expected.push(person(V4_ACCOUNT_ID));
                expected.push(person(V17_CO_MEMBER));
                expected.sort();
                assert_eq!(after, &expected, "{name}");
            }
            // The Roadmap's mention reminds the co-member: its last column is the only difference.
            "date_mentions" => {
                let (changed, same): (Vec<&String>, Vec<&String>) =
                    before.iter().partition(|line| line.contains(&roadmap));
                assert_eq!(changed.len(), 1, "one mention on the Roadmap");
                let mut expected: Vec<String> = same.into_iter().cloned().collect();
                let stem = changed[0].strip_suffix("NULL").unwrap();
                expected.push(format!("{stem}{}", hex_id(V17_CO_MEMBER)));
                expected.sort();
                assert_eq!(after, &expected, "{name}");
            }
            _ => assert_eq!(after, before, "{name}"),
        }
    }
}

#[test]
fn the_v18_fixture_reads_back_through_the_comment_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let db = open_plain(&copy);
    let roadmap = page_id(V2_PAGE_ROADMAP);

    // The Roadmap's stream in `(at, record id)` order, and Home's one record.
    let rows = db.comment_records_of_page(roadmap).unwrap();
    assert_eq!(rows, v18_roadmap_records());
    let home = db.comment_records_of_page(page_id(V2_PAGE_HOME)).unwrap();
    assert_eq!(home.len(), 1);
    assert_eq!(home[0].comment_id, v18_comment_id(V18_COMMENT_HOME));
    assert_eq!(home[0].space_id, space_id(V2_PERSONAL));
    assert_eq!(home[0].cursor, None);
    assert!(
        db.comment_records_of_page(page_id(V2_PAGE_NOTES))
            .unwrap()
            .is_empty()
    );

    // A record this device has not pushed gets its cursor; the same record again is not new.
    let unpushed = &rows[5];
    assert!(
        db.comment_record_set_cursor(unpushed.record_id, 18)
            .unwrap()
    );
    assert_eq!(
        db.comment_records_of_page(roadmap).unwrap()[5].cursor,
        Some(18)
    );
    assert!(!db.comment_record_put(unpushed).unwrap());

    // The words: found by prefix with accents folded, labelled by comment.
    let search = |query: &str| -> Vec<CommentId> {
        let mut ids: Vec<CommentId> = db
            .comment_fts_search(&SearchParams {
                query,
                space_id: None,
                titles_only: false,
                limit: 20,
                any_word: false,
                edited_after: None,
            })
            .unwrap()
            .into_iter()
            .map(|hit| hit.comment_id)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(search("cafe"), [v18_comment_id(V18_COMMENT_SECOND)]);
    assert_eq!(
        search("ship"),
        [
            v18_comment_id(V18_COMMENT_ROOT),
            v18_comment_id(V18_COMMENT_SECOND)
        ]
    );
    assert_eq!(search("review"), [v18_comment_id(V18_COMMENT_HOME)]);
    assert!(
        search("deleted").is_empty(),
        "the deleted reply has no words"
    );

    // The move of the Roadmap to the Personal space carries its records and nothing else.
    assert_eq!(
        db.comment_records_set_space(roadmap, space_id(V2_PERSONAL))
            .unwrap(),
        6
    );
    assert!(
        db.comment_records_of_page(roadmap)
            .unwrap()
            .iter()
            .all(|row| row.space_id == space_id(V2_PERSONAL))
    );
    // A purge of the page takes its records.
    assert_eq!(db.comment_records_delete_page(roadmap).unwrap(), 6);
    assert_eq!(count_rows(db.connection(), "comments"), 1);
}

#[test]
fn the_v18_fixture_reads_back_through_the_inbox_and_the_mention_queries() {
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let db = open_plain(&copy);
    let co_member = account_id(V17_CO_MEMBER);

    let inbox = db.notification_list(10, None, false).unwrap();
    assert_eq!(
        inbox.iter().map(|row| row.kind).collect::<Vec<_>>(),
        [
            NotificationKind::PersonAssigned,
            NotificationKind::PageMention,
            NotificationKind::CommentMention,
            NotificationKind::ThreadReply
        ],
        "newest first"
    );
    assert!(inbox.windows(2).all(|pair| pair[0].id > pair[1].id));
    assert!(inbox.iter().all(|row| row.actor == co_member));
    assert!(inbox.iter().all(|row| row.space_id == space_id(V2_WORK)));
    let reply: &NotificationRow = &inbox[3];
    assert!(reply.read);
    assert_eq!(reply.comment_id, Some(v18_comment_id(V18_COMMENT_REPLY)));
    assert_eq!(reply.block_id, Some(block_id(V9_BLOCK_ROADMAP_SHIP)));
    assert_eq!(db.notification_unread_count().unwrap(), 3);
    assert_eq!(db.notification_list(10, None, true).unwrap().len(), 3);
    assert_eq!(db.notification_mark_read(None).unwrap(), 3);
    assert_eq!(db.notification_delete_space(space_id(V2_WORK)).unwrap(), 4);
    assert!(db.notification_list(10, None, false).unwrap().is_empty());

    let roadmap_doc = DocId::for_page(page_id(V2_PAGE_ROADMAP));
    let row = |block: &str, account: AccountId| PersonMentionRow {
        block_id: block_id(block),
        account_id: account,
    };
    assert_eq!(
        db.person_mentions_of(roadmap_doc).unwrap(),
        [
            row(V9_BLOCK_ROADMAP_SHIP, co_member),
            row(V3_BLOCK_ROADMAP_CAFE, account_id(V4_ACCOUNT_ID)),
            row(V3_BLOCK_ROADMAP_CAFE, co_member),
        ]
    );
    db.person_mentions_replace(roadmap_doc, &[]).unwrap();
    assert!(db.person_mentions_of(roadmap_doc).unwrap().is_empty());
}

#[test]
fn the_v18_fixture_reads_back_the_person_value_and_the_reminder_account() {
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let db = open_plain(&copy);
    let roadmap = page_id(V2_PAGE_ROADMAP);
    let (owner, co_member) = (account_id(V4_ACCOUNT_ID), account_id(V17_CO_MEMBER));

    // The Person value is the last two rows of the Roadmap, in the order written.
    let rows = db.page_properties_of(roadmap).unwrap();
    let person: Vec<&PagePropertyRow> = rows
        .iter()
        .filter(|row| row.kind == PropertyKind::Person)
        .collect();
    assert_eq!(
        person
            .iter()
            .map(|row| (row.property_id, row.account_id))
            .collect::<Vec<_>>(),
        [
            (PropertyId::from_bytes(id_of(V18_PROP_OWNER)), Some(owner)),
            (
                PropertyId::from_bytes(id_of(V18_PROP_OWNER)),
                Some(co_member)
            )
        ]
    );
    assert!(
        rows.iter()
            .filter(|row| row.kind != PropertyKind::Person)
            .all(|row| row.account_id.is_none()),
        "the other kinds have no account"
    );
    let property = PropertyId::from_bytes(id_of(V18_PROP_OWNER));
    let work = space_id(V2_WORK);
    for matching in [
        PropertyMatch::Person(co_member),
        PropertyMatch::Person(owner),
        PropertyMatch::HasValue,
    ] {
        assert_eq!(
            db.page_properties_pages(work, property, &matching).unwrap(),
            [roadmap]
        );
    }
    assert!(
        db.page_properties_pages(
            work,
            property,
            &PropertyMatch::Person(account_id(V17_OUTSIDER))
        )
        .unwrap()
        .is_empty()
    );

    // The date mention on the Roadmap reminds the co-member; the others have no `reminderFor`.
    let mentions = db.date_mentions_of(roadmap).unwrap();
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].reminder_for, Some(co_member));
    assert_eq!(
        mentions[0],
        DateMentionRow {
            reminder_for: Some(co_member),
            ..v9_roadmap_ship()
        }
    );
    let due = db.date_mentions_due(i64::MIN, i64::MAX, 100).unwrap();
    assert_eq!(due.len(), 3);
    assert_eq!(
        due.iter()
            .filter_map(|row| row.reminder_for)
            .collect::<Vec<_>>(),
        [co_member]
    );
    assert_eq!(
        due.last().unwrap(),
        &DueReminderRow {
            reminder_for: Some(co_member),
            ..v9_due(V2_PAGE_ROADMAP, V2_WORK, &v9_roadmap_ship())
        }
    );
}

/// The 16 bytes of an id written as text.
fn id_of(text: &str) -> [u8; 16] {
    text.parse::<PropertyId>().unwrap().to_bytes()
}

#[test]
fn migrating_the_v17_fixture_keeps_every_row_and_adds_the_empty_tables_and_columns() {
    let (_dir, copy) = copy_of(FIXTURE_V17, "v17.db");
    let before = dump_every_table(&raw_conn(&copy, "17"));
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string()
    );
    // Every table of v17 has the rows it had (the two rebuilt or widened tables are compared without
    // their new column, as `dump` leaves it out), and the new tables are empty.
    let skipping = |conn: &Connection| -> Vec<(String, Vec<String>)> {
        dump_every_table(conn)
            .into_iter()
            .filter(|(name, _)| !V18_TABLES.contains(&name.as_str()))
            .filter(|(name, _)| !V19_TABLES.contains(&name.as_str()))
            .collect()
    };
    let after = skipping(db.connection());
    let before: Vec<(String, Vec<String>)> = before
        .into_iter()
        .filter(|(name, _)| !V18_TABLES.contains(&name.as_str()))
        .collect();
    assert_eq!(after, before);
    for table in [
        "comments",
        "notifications",
        "person_mentions",
        "comment_fts_content",
    ] {
        assert_eq!(count_rows(db.connection(), table), 0, "{table}");
    }
    assert_eq!(
        count_rows(
            db.connection(),
            "page_properties WHERE account_id IS NOT NULL"
        ),
        0
    );
    assert_eq!(
        count_rows(
            db.connection(),
            "date_mentions WHERE reminder_for IS NOT NULL"
        ),
        0
    );
    // The property rows keep their order (the rebuild copies by rowid).
    let conn = raw_conn(&copy_of(FIXTURE_V17, "v17.db").1, "17");
    let order = |conn: &Connection| -> Vec<String> {
        conn.prepare("SELECT hex(page_id) || hex(property_id) FROM page_properties ORDER BY rowid")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(order(db.connection()), order(&conn));
    // Migrating twice changes nothing.
    drop(db);
    let db = open_plain(&copy);
    assert_eq!(skipping(db.connection()), before);
}

#[test]
fn the_v18_tables_take_part_in_an_encrypted_file_and_leave_nothing_readable_at_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v18-encrypted.db");
    let key = db_key(6);
    let space = space_id(V2_WORK);
    let page = page_id(V2_PAGE_ROADMAP);
    {
        let db = open_encrypted(&path, &key);
        db.comment_record_put(&CommentRow {
            record: b"comment-record-marker-bytes".to_vec(),
            ..v18_roadmap_records()[0].clone()
        })
        .unwrap();
        db.comment_fts_replace(
            space,
            page,
            &[kb_storage::CommentTextRow {
                comment_id: v18_comment_id(V18_COMMENT_ROOT),
                text: "comment-words-marker-text".to_owned(),
            }],
        )
        .unwrap();
        db.person_mentions_replace(
            DocId::for_page(page),
            &[PersonMentionRow {
                block_id: block_id(V9_BLOCK_ROADMAP_SHIP),
                account_id: account_id(V17_CO_MEMBER),
            }],
        )
        .unwrap();
    }
    for file in files_in(dir.path()) {
        let raw = fs::read(&file).unwrap();
        assert!(!contains(&raw, b"comment-record-marker-bytes"), "{file:?}");
        assert!(!contains(&raw, b"comment-words-marker-text"), "{file:?}");
    }
    let db = open_encrypted(&path, &key);
    assert_eq!(db.comment_records_of_page(page).unwrap().len(), 1);
    assert_eq!(
        db.person_mentions_of(DocId::for_page(page)).unwrap().len(),
        1
    );
}

// ---- v19 ----

#[test]
fn migrating_the_v18_fixture_keeps_every_row_and_adds_the_empty_first_images_table() {
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let before = dump_raw_every_table(&raw_conn(&copy, "18"));
    assert!(
        !before.iter().any(|(name, _)| name == "page_first_images"),
        "a v18 file has no such table"
    );
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        pragma(db.connection(), "user_version"),
        SCHEMA_VERSION.to_string()
    );
    // Every table of v18 has exactly the rows it had (the page index with the column v20 gave it left
    // out); the new table is there, empty and with its three columns.
    let after = dump_raw_every_table(db.connection());
    let (new, old): (Vec<_>, Vec<_>) = after
        .into_iter()
        .partition(|(name, _)| V19_TABLES.contains(&name.as_str()));
    let old: Vec<(String, Vec<String>)> = old
        .into_iter()
        .map(|(name, rows)| {
            if name != "pages_index" {
                return (name, rows);
            }
            let mut rows: Vec<String> = rows
                .into_iter()
                .map(|row| row.strip_suffix("|0").unwrap().to_owned())
                .collect();
            rows.sort();
            (name, rows)
        })
        .collect();
    assert_eq!(old, before);
    assert_eq!(new, [("page_first_images".to_owned(), Vec::new())]);
    let columns: Vec<(String, String, bool, bool)> = db
        .connection()
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info('page_first_images') ORDER BY cid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            ("page_id".to_owned(), "BLOB".to_owned(), false, true),
            ("space_id".to_owned(), "BLOB".to_owned(), true, false),
            ("attachment_id".to_owned(), "BLOB".to_owned(), false, false),
        ]
    );
    // It takes rows: a scanned page with a picture and one without.
    let space = SpaceId::from_bytes([0xaa; 16]);
    let (with, without) = (PageId::from_bytes([1; 16]), PageId::from_bytes([2; 16]));
    let picture = AttachmentId::from_bytes([9; 16]);
    db.first_images_set(space, &[(with, Some(picture)), (without, None)])
        .unwrap();
    assert_eq!(
        db.first_images_get(&[without, with]).unwrap(),
        [(with, Some(picture)), (without, None)]
    );
    drop(db);
    // Migrating the current version again writes nothing.
    let settled = fs::read(&copy).unwrap();
    drop(open_plain(&copy));
    assert_eq!(fs::read(&copy).unwrap(), settled);
}

// ---- v20 ----

#[test]
fn the_v18_fixture_gains_in_template_zero_and_a_template_leaves_every_list_but_the_counts() {
    // Migrating the newest fixture: no page of the page index is in a template, and the column
    // that says so (ADR-0056) keeps the pages of a template out of every list that is not the tree.
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let db = open_plain(&copy);
    let (personal, work) = (space_id(V2_PERSONAL), space_id(V2_WORK));
    let (home, notes, roadmap) = (
        page_id(V2_PAGE_HOME),
        page_id(V2_PAGE_NOTES),
        page_id(V2_PAGE_ROADMAP),
    );
    let rows: Vec<PageIndexRow> = [personal, work]
        .into_iter()
        .flat_map(|space| db.pages_index_by_space(space).unwrap())
        .collect();
    assert!(!rows.is_empty() && rows.iter().all(|row| !row.in_template));
    assert_eq!(db.pages_index_template_count(personal).unwrap(), 0);
    assert_eq!(db.pages_index_template_count(work).unwrap(), 0);
    let live = (
        db.pages_index_live_count(personal).unwrap(),
        db.pages_index_live_count(work).unwrap(),
    );

    let words = |db: &Db, query: &str| -> Vec<PageId> {
        let mut pages: Vec<PageId> = db
            .fts_search(&SearchParams {
                query,
                space_id: None,
                titles_only: false,
                limit: 50,
                any_word: false,
                edited_after: None,
            })
            .unwrap()
            .into_iter()
            .map(|hit| hit.page_id)
            .collect();
        pages.sort();
        pages.dedup();
        pages
    };
    let titles = |db: &Db, query: &str| -> Vec<PageId> {
        let mut pages: Vec<PageId> = db
            .fts_search(&SearchParams {
                query,
                space_id: None,
                titles_only: true,
                limit: 50,
                any_word: false,
                edited_after: None,
            })
            .unwrap()
            .into_iter()
            .map(|hit| hit.page_id)
            .collect();
        pages.sort();
        pages
    };
    let comment_pages = |db: &Db, query: &str| -> Vec<PageId> {
        let mut pages: Vec<PageId> = db
            .comment_fts_search(&SearchParams {
                query,
                space_id: None,
                titles_only: false,
                limit: 50,
                any_word: false,
                edited_after: None,
            })
            .unwrap()
            .into_iter()
            .map(|hit| hit.page_id)
            .collect();
        pages.sort();
        pages.dedup();
        pages
    };
    let due_pages = |db: &Db| -> Vec<PageId> {
        let mut pages: Vec<PageId> = db
            .date_mentions_due(i64::MIN, i64::MAX, 100)
            .unwrap()
            .into_iter()
            .map(|row| row.page_id)
            .collect();
        pages.sort();
        pages.dedup();
        pages
    };
    let backlinks_of = |db: &Db, page: PageId| -> Vec<PageId> {
        db.links_to(page)
            .unwrap()
            .into_iter()
            .map(|source| source.from_page)
            .collect()
    };
    let set_template = |db: &Db, page: PageId, on: bool| {
        let row = db.pages_index_get(page).unwrap().unwrap();
        db.pages_index_upsert(&[PageIndexRow {
            in_template: on,
            ..row
        }])
        .unwrap();
    };
    let set_of = |pages: &[PageId]| -> Vec<PageId> {
        let mut pages = pages.to_vec();
        pages.sort();
        pages
    };

    // What the lists hold before: Home is in each of them.
    assert_eq!(words(&db, "cafe"), set_of(&[home, notes, roadmap]));
    assert!(titles(&db, "road").contains(&roadmap));
    assert!(comment_pages(&db, "review").contains(&home));
    assert_eq!(due_pages(&db), set_of(&[home, notes, roadmap]));
    assert!(backlinks_of(&db, roadmap).contains(&home));
    let live_pages = db.ai_live_pages(None).unwrap();
    assert!(live_pages.iter().any(|(page, _)| *page == home));
    let counts_before = db.ai_index_counts(None).unwrap();

    // Home becomes a template: it is in no list, and it is still a live page.
    set_template(&db, home, true);
    assert_eq!(words(&db, "cafe"), set_of(&[notes, roadmap]));
    assert!(
        comment_pages(&db, "review").is_empty(),
        "the comment of the template page is not found"
    );
    assert!(!due_pages(&db).contains(&home));
    assert!(
        !backlinks_of(&db, roadmap).contains(&home),
        "a link from a template is not a backlink"
    );
    assert!(
        !db.ai_live_pages(None)
            .unwrap()
            .iter()
            .any(|(page, _)| *page == home)
    );
    assert!(
        !db.ai_live_pages(Some(personal))
            .unwrap()
            .iter()
            .any(|(page, _)| *page == home)
    );
    let counts = db.ai_index_counts(None).unwrap();
    assert_eq!(counts.pages_total, counts_before.pages_total - 1);
    assert_eq!(
        (
            db.pages_index_live_count(personal).unwrap(),
            db.pages_index_live_count(work).unwrap()
        ),
        live,
        "a template is a live page: the counts do not move"
    );
    assert_eq!(db.pages_index_template_count(personal).unwrap(), 1);
    assert_eq!(db.pages_index_template_count(work).unwrap(), 0);
    assert!(db.pages_index_get(home).unwrap().unwrap().in_template);
    // The other pages are where they were.
    assert_eq!(due_pages(&db), set_of(&[notes, roadmap]));

    // The semantic index keeps no row of a template: the core finds the ones left over (the fixture
    // may hold rows of its own; the pages it names are the ones that are in a template).
    db.ai_page_replace(
        &kb_storage::AiPageRow {
            page_id: home,
            source_hash: [7; 32],
            model: "m".to_owned(),
            chunks: 0,
            indexed_at: 1,
        },
        &[],
    )
    .unwrap();
    assert_eq!(db.ai_template_pages().unwrap(), [home]);
    assert_eq!(
        db.ai_index_counts(Some("m")).unwrap().pages_indexed,
        0,
        "a template page is not an indexed page"
    );
    set_template(&db, home, false);
    assert!(db.ai_template_pages().unwrap().is_empty());

    // And back: every list has Home again, and the upsert is the only thing that moved it.
    assert_eq!(words(&db, "cafe"), set_of(&[home, notes, roadmap]));
    assert!(comment_pages(&db, "review").contains(&home));
    assert!(backlinks_of(&db, roadmap).contains(&home));
    assert_eq!(db.pages_index_template_count(personal).unwrap(), 0);
    assert_eq!(
        db.ai_index_counts(None).unwrap().pages_total,
        counts_before.pages_total
    );

    // A template inside a space, in a transaction: the same rows, written and read back.
    let mut db = db;
    db.transaction(|tx| {
        let row = tx.pages_index_get(roadmap)?.unwrap();
        tx.pages_index_upsert(&[PageIndexRow {
            in_template: true,
            ..row
        }])
    })
    .unwrap();
    assert_eq!(db.pages_index_template_count(work).unwrap(), 1);
    assert!(!words(&db, "cafe").contains(&roadmap));
    assert!(comment_pages(&db, "ship").is_empty());
    assert!(!due_pages(&db).contains(&roadmap));
}

// ---- v21 ----

#[test]
fn the_v18_fixture_gains_an_empty_page_history_journal_that_follows_its_documents() {
    // Migrating the newest fixture: the journal of ADR-0057 arrives empty, with its columns, and no other
    // table changes (the dump helpers leave it out, like the tables of the earlier versions).
    let (_dir, copy) = copy_of(FIXTURE_V18, "v18.db");
    let before = dump_raw_every_table(&raw_conn(&copy, "18"));
    assert!(
        !before
            .iter()
            .any(|(name, _)| name == "page_history_journal"),
        "a v18 file has no such table"
    );
    let db = open_plain(&copy);
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(count_rows(db.connection(), "page_history_journal"), 0);
    let columns: Vec<(String, String, bool, bool)> = db
        .connection()
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info('page_history_journal') ORDER BY cid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let col = |name: &str, ty: &str, notnull: bool, pk: bool| {
        (name.to_owned(), ty.to_owned(), notnull, pk)
    };
    assert_eq!(
        columns,
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

    // It takes rows of a document that exists, and loses them with the document.
    let doc = DocId::for_page(page_id(V2_PAGE_HOME));
    let journal_row = |version: &str| NewJournalRow {
        doc_id: doc,
        kind: JournalKind::Edit,
        started_at: 1,
        ended_at: 2,
        base_version: String::new(),
        version: version.to_owned(),
        device_id: None,
        account_id: None,
        this_device: true,
        title: Some("Home".to_owned()),
    };
    assert_eq!(db.journal_append(&journal_row("1@0")).unwrap(), 1);
    assert_eq!(db.journal_append(&journal_row("1@1")).unwrap(), 2);
    assert_eq!(db.journal_list(doc, None, None).unwrap().len(), 2);
    assert!(
        db.journal_append(&NewJournalRow {
            doc_id: DocId::from_bytes([0xee; 16]),
            ..journal_row("1@0")
        })
        .is_err(),
        "a row needs its document"
    );
    db.doc_delete(doc).unwrap();
    assert!(db.journal_list(doc, None, None).unwrap().is_empty());
    drop(db);
    // Migrating the current version again writes nothing.
    let settled = fs::read(&copy).unwrap();
    drop(open_plain(&copy));
    assert_eq!(fs::read(&copy).unwrap(), settled);
}
