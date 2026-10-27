//! Generates the checked-in database fixtures, one per released schema version. All
//! are **unencrypted** plain SQLite files that the migration tests copy and open with
//! today's code (docs/development/contributing.md: fixtures are regenerated only by
//! scripts, never by hand).
//!
//! - `fixtures/db/v1.db`: schema v1 (`PRAGMA user_version = 1`) holding one device
//!   identity, one workspace and two settings.
//! - `fixtures/db/v2.db`: schema v2 (`PRAGMA user_version = 2`): the same v1 content
//!   plus what the Milestone 2 app writes: an account document and two structure
//!   documents (spaces "Personal" and "Work"), one of them with a snapshot and an
//!   update log that starts after it (as after a compaction), and `pages_index` rows
//!   with live, trashed and nested pages. Blobs are opaque placeholders (this crate
//!   never interprets blobs); they contain NUL and high bytes on purpose.
//! - `fixtures/db/v3.db`: schema v3 (`PRAGMA user_version = 3`): the v2 content plus
//!   what the Milestone 3 app writes: page documents (their id is the page id), the
//!   text index (`fts_blocks`: one title row per page, block rows for the live and
//!   trashed pages, with accents, astral characters and a reference resolved to a
//!   title; the word index `fts` is filled by the triggers of the migration) and the
//!   link index (`links`, including links from a trashed page and across spaces).
//! - `fixtures/db/v4.db`: schema v4 (`PRAGMA user_version = 4`): the v3 content plus what the
//!   Milestone 4 app writes once a recovery kit exists: the `meta` keys of the kit
//!   (`account_id`, `recovery_id`, `recovery_blob`, `recovery_created_at`), three device log
//!   entries with fake 32-byte hashes, and eight keyring rows (the space key and the
//!   document keys of both spaces as records, and the tombstone of a purged page). The log
//!   entries and keyring records are opaque stand-ins, like the document blobs of v2.
//! - `fixtures/db/v5.db`: schema v5 (`PRAGMA user_version = 5`): the v4 content plus what the
//!   Milestone 5 app writes once pages carry attachments: six `attachments_index` rows (two
//!   spaces, sizes from 0 to 4 MiB + 1, two with the same creation time, two unreferenced) and
//!   the `attachment_refs` rows of the pages that show them (a trashed page, two pages showing
//!   one attachment, a reference to an attachment that was purged). The manifests live inside
//!   the structure documents (opaque blobs here), so none is written.
//! - `fixtures/db/v6.db`: schema v6 (`PRAGMA user_version = 6`): the v5 content plus what the
//!   Milestone 6 app writes once a device has synced: eight `sync_config` pairs, seven
//!   `sync_doc_state` rows (the keyring's all-zero document id and six documents; a seventh
//!   document has none), five `sync_seen` rows, two `sync_heads` rows and three
//!   `sync_pending` rows (two of them for one document); and the `published` markers: half
//!   of the keyring rows and two of the three device log entries are published, the rest
//!   keep the 0 the migration gave them. Signed heads and pending entries are opaque
//!   stand-ins like the other blobs.
//! - `fixtures/db/v7.db`: schema v7 (`PRAGMA user_version = 7`): the v6 content, with the
//!   `sync_pending` table keyed by cursor and document (Milestone 7; the rows are the same).
//! - `fixtures/db/v8.db`: schema v8 (`PRAGMA user_version = 8`): the v7 content plus what the
//!   Milestone 8 app writes once attachments sync: five `chunk_sync` rows, one or two in each
//!   state (two uploads with different attempts, a synced chunk, a download that waits for the
//!   server with a `next_at`, and the delete of a purged attachment's chunk), for three
//!   attachments of the v5 content. Chunk ids are 32-byte stand-ins for the BLAKE3 hash of
//!   the ciphertext (a ramp from a distinct first byte, three of them above 0x7F, inserted
//!   out of order, so that the unsigned blob order is not the insertion order).
//! - `fixtures/db/v9.db`: schema v9 (`PRAGMA user_version = 9`): the v8 content plus what the
//!   reminders write (ADR-0024): five `date_mentions` rows for the pages of the v3 content (two
//!   of the first page, one of the second, one of the trashed page "Old ideas" and one of the
//!   Work space), three with a reminder (15 minutes, a day, two days), one that reminds at the
//!   time of a whole-day mention, and two without one; the fire times are on the local wall
//!   clock (seconds since 1970-01-01T00:00:00, the calendar fields counted as UTC).
//! - `fixtures/db/v10.db`: schema v10 (`PRAGMA user_version = 10`): the v9 content plus what typed
//!   page properties write (ADR-0026): the `page_properties` rows of four pages of the v3 content
//!   (the two live pages of the Personal space, the trashed page "Old ideas" and the Roadmap of
//!   the Work space) for a small schema (Tags, a text, a number, a checkbox, a date, a status, a
//!   select and a URL), and the property rows (`kind = 1`) of the text index for the same
//!   pages. The schema itself lives in the structure documents (opaque blobs here), so none is
//!   written. The rows are inserted out of page order on purpose.
//! - `fixtures/db/v11.db`: schema v11 (`PRAGMA user_version = 11`): the v10 content plus what the
//!   `files` and `checklist` kinds of page properties write (ADR-0026, amendment of 2026-10-07): five
//!   files rows (the report and the photo on Home, the photo on Notes, the empty file on the Roadmap,
//!   the sketch on the trashed page "Old ideas"; they attach attachments of the v5 content, so they
//!   count as references next to the blocks), three checklist rows (Home two of three items done,
//!   the Roadmap one of one, the trashed page none of two), and seven property rows (`kind = 1`) of the
//!   text index (the file names, the item texts). The rows are inserted out of page order on purpose.
//! - `fixtures/db/v12.db`: schema v12 (`PRAGMA user_version = 12`): the v11 content plus what the
//!   `place` kind of page properties writes (ADR-0026, amendment of 2026-10-08): three place rows (the
//!   Home page "Café de Flore, Paris", Notes "Home office", the Roadmap "Lisbon") whose `value_text` is
//!   the name, and the three matching property rows (`kind = 1`) of the text index.
//! - `fixtures/db/v13.db`: schema v13 (`PRAGMA user_version = 13`): the v12 content plus what the
//!   `relation` kind of page properties writes (ADR-0027): the forward property lists Notes and the
//!   trashed page "Old ideas" on Home, the back property lists Home on Notes (one row per related
//!   page, `related_page_id`), and the two matching property rows (`kind = 1`) of the text index
//!   (the titles of the related pages).
//! - `fixtures/db/v14.db`: schema v14 (`PRAGMA user_version = 14`): the v13 content plus what the
//!   image icons of pages write (ADR-0029): three `icon_refs` rows (Home has the photo of the v5
//!   content as its icon, the trashed page "Old ideas" the sketch, the Roadmap the attachment that was
//!   purged, which has no index row), inserted out of page order. They count as references next to
//!   the blocks and the `files` values.
//! - `fixtures/db/v15.db`: schema v15 (`PRAGMA user_version = 15`): the v14 content plus what local AI and
//!   the MCP server write (ADR-0037): the semantic index of two pages (three chunks with four-dimensional
//!   stand-in vectors; the pages that are not indexed wait in `ai_dirty`, which the migration's triggers
//!   filled as the text index rows were inserted), two MCP clients (one with a read grant, one new) and
//!   three audit rows.
//! - `fixtures/db/v16.db`: schema v16 (`PRAGMA user_version = 16`): the v15 content with the key of
//!   `icon_refs` changed to the pair (page, attachment) and two rows for covers (Home has the report as
//!   its cover next to the photo as its icon; Notes has the photo as its cover).
//! - `fixtures/db/v17.db`: schema v17 (`PRAGMA user_version = 17`, shared spaces, M13): the v16 content
//!   **migrated by `017_shared_spaces.sql`** (its sync rows carry the personal feed, the account id) plus
//!   what a device that shares the Work space holds: two `sync_feeds` rows, the sync bookkeeping of the
//!   shared feed (the keyring pseudo-document and the Roadmap have a row in each feed), the device log of
//!   a co-member, three quarantined items and a count of dropped ones, three entry marks, the membership
//!   log of the shared feed (one entry not accepted yet) and the cache of its fold.
//! - `fixtures/db/v18.db`: schema v18 (`PRAGMA user_version = 18`, comments, person mentions and the Person
//!   property, M13 slice 2): the v17 content **migrated by `018_comments_people.sql`** (`page_properties`
//!   rebuilt with the `account_id` column, `date_mentions` with `reminder_for`) plus what a device that
//!   shares the Work space and comments holds: seven comment records (the Roadmap's stream, one record
//!   not pushed yet, and one record of Home), the words of three comments, four notifications (one of
//!   each kind, one read), three person mentions, the Person value of the Roadmap (two accounts) and a
//!   `reminderFor` on the Roadmap's date mention.
//!
//! ```text
//! cargo run -p kb-storage --example make_fixture_db -- v18
//! cargo run -p kb-storage --example make_fixture_db -- v17
//! cargo run -p kb-storage --example make_fixture_db -- v16
//! cargo run -p kb-storage --example make_fixture_db -- v15
//! cargo run -p kb-storage --example make_fixture_db -- v14
//! cargo run -p kb-storage --example make_fixture_db -- v13
//! cargo run -p kb-storage --example make_fixture_db -- v12
//! cargo run -p kb-storage --example make_fixture_db -- v11
//! cargo run -p kb-storage --example make_fixture_db -- v10
//! cargo run -p kb-storage --example make_fixture_db -- v9
//! cargo run -p kb-storage --example make_fixture_db -- v8
//! cargo run -p kb-storage --example make_fixture_db -- v7
//! cargo run -p kb-storage --example make_fixture_db -- v6
//! cargo run -p kb-storage --example make_fixture_db -- v5
//! cargo run -p kb-storage --example make_fixture_db -- v4
//! cargo run -p kb-storage --example make_fixture_db -- v3
//! cargo run -p kb-storage --example make_fixture_db -- v2
//! cargo run -p kb-storage --example make_fixture_db -- v1 [output path]
//! ```
//!
//! Regenerate a fixture only if that schema version itself must change (it should
//! not: released schema versions are frozen; a new version gets its own fixture). The
//! files are built with raw SQL on purpose, not through `Db`, so they stay honest
//! pictures of what that version wrote and do not move with the code that reads them.
//! A keyless connection is plain SQLite even in a SQLCipher build.

// A developer tool, not library code: printing and unwrapping are fine here.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::expect_used
)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kb_storage::rusqlite::{Connection, params};
use kb_types::DocId;

/// The exact SQL that version 1 of the app ran.
const MIGRATION_V1: &str = include_str!("../migrations/001_initial.sql");

/// The exact SQL that version 2 of the app ran on top of it.
const MIGRATION_V2: &str = include_str!("../migrations/002_documents.sql");

/// The exact SQL that version 3 of the app ran on top of that.
const MIGRATION_V3: &str = include_str!("../migrations/003_text_search.sql");

/// The exact SQL that version 4 of the app ran on top of that.
const MIGRATION_V4: &str = include_str!("../migrations/004_keys.sql");

/// The exact SQL that version 5 of the app ran on top of that.
const MIGRATION_V5: &str = include_str!("../migrations/005_attachments.sql");

/// The exact SQL that version 6 of the app ran on top of that.
const MIGRATION_V6: &str = include_str!("../migrations/006_sync.sql");
/// Schema v7: `sync_pending` keyed by cursor and document.
const MIGRATION_V7: &str = include_str!("../migrations/007_sync_pending_key.sql");
/// Schema v8: the `chunk_sync` table of attachment sync.
const MIGRATION_V8: &str = include_str!("../migrations/008_chunk_sync.sql");
/// Schema v9: the `date_mentions` table of the reminders.
const MIGRATION_V9: &str = include_str!("../migrations/009_date_mentions.sql");
/// Schema v10: the `page_properties` table and the `kind` column of `fts_blocks`.
const MIGRATION_V10: &str = include_str!("../migrations/010_page_properties.sql");
/// Schema v11: the `files` and `checklist` kinds (the table is rebuilt with two columns more).
const MIGRATION_V11: &str = include_str!("../migrations/011_files_and_checklist_properties.sql");
/// Schema v12: the `place` kind (the table is rebuilt with a longer list of kinds).
const MIGRATION_V12: &str = include_str!("../migrations/012_place_property.sql");

/// Schema v13: the `relation` kind and the `related_page_id` column (the table is rebuilt).
const MIGRATION_V13: &str = include_str!("../migrations/013_relation_property.sql");

/// Schema v14: the `icon_refs` table of the image icons of pages.
const MIGRATION_V14: &str = include_str!("../migrations/014_icon_refs.sql");

/// Schema v15: the semantic index and the MCP clients and audit.
const MIGRATION_V15: &str = include_str!("../migrations/015_ai_and_mcp.sql");

/// Schema v16: `icon_refs` keyed by the pair (page, attachment), for the covers.
const MIGRATION_V16: &str = include_str!("../migrations/016_cover_refs.sql");

/// Schema v17: shared spaces. Run on top of the v16 content: it moves the rows of the four sync tables
/// to the personal feed, so the fixture is what the migration makes of a v16 file.
const MIGRATION_V17: &str = include_str!("../migrations/017_shared_spaces.sql");

/// Schema v18: comments, person mentions and the Person property. Run on top of the v17 content (the
/// `page_properties` rows and the date mentions are kept, in order, with the new columns empty).
const MIGRATION_V18: &str = include_str!("../migrations/018_comments_people.sql");

/// `0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d`
const DEVICE_ID: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";

/// `7a1d9c3e-5b2f-4e8a-9c41-d3f0a6b7e812` as the 16-byte blob v1 stores.
const WORKSPACE_ID: [u8; 16] = [
    0x7a, 0x1d, 0x9c, 0x3e, 0x5b, 0x2f, 0x4e, 0x8a, 0x9c, 0x41, 0xd3, 0xf0, 0xa6, 0xb7, 0xe8, 0x12,
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (version, name) = match args.first().map(String::as_str) {
        Some("v1") => (1, "v1.db"),
        Some("v2") => (2, "v2.db"),
        Some("v3") => (3, "v3.db"),
        Some("v4") => (4, "v4.db"),
        Some("v5") => (5, "v5.db"),
        Some("v6") => (6, "v6.db"),
        Some("v7") => (7, "v7.db"),
        Some("v8") => (8, "v8.db"),
        Some("v9") => (9, "v9.db"),
        Some("v10") => (10, "v10.db"),
        Some("v11") => (11, "v11.db"),
        Some("v12") => (12, "v12.db"),
        Some("v13") => (13, "v13.db"),
        Some("v14") => (14, "v14.db"),
        Some("v15") => (15, "v15.db"),
        Some("v16") => (16, "v16.db"),
        Some("v17") => (17, "v17.db"),
        Some("v18") => (18, "v18.db"),
        _ => {
            eprintln!(
                "usage: make_fixture_db <v1|v2|v3|v4|v5|v6|v7|v8|v9|v10|v11|v12|v13|v14|v15|v16|v17|v18> [output path]"
            );
            return ExitCode::from(2);
        }
    };
    let path = args.get(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../fixtures/db/{name}")),
        PathBuf::from,
    );
    write_fixture(&path, version);
    println!("wrote {}", path.display());
    ExitCode::SUCCESS
}

fn write_fixture(path: &Path, version: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }

    let conn = Connection::open(path).unwrap();
    // Catches a fixture that breaks its own foreign keys.
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    // The content of the file up to v16 is built in the schema it had; v17 and v18 are that content
    // migrated, with their own rows on top.
    let content_version = version.min(16);
    conn.execute_batch(MIGRATION_V1).unwrap();
    if content_version >= 2 {
        conn.execute_batch(MIGRATION_V2).unwrap();
    }
    if content_version >= 3 {
        conn.execute_batch(MIGRATION_V3).unwrap();
    }
    if content_version >= 4 {
        conn.execute_batch(MIGRATION_V4).unwrap();
    }
    if content_version >= 5 {
        conn.execute_batch(MIGRATION_V5).unwrap();
    }
    if content_version >= 6 {
        conn.execute_batch(MIGRATION_V6).unwrap();
    }
    if content_version >= 7 {
        conn.execute_batch(MIGRATION_V7).unwrap();
    }
    if content_version >= 8 {
        conn.execute_batch(MIGRATION_V8).unwrap();
    }
    if content_version >= 9 {
        conn.execute_batch(MIGRATION_V9).unwrap();
    }
    if content_version >= 10 {
        conn.execute_batch(MIGRATION_V10).unwrap();
    }
    if content_version >= 11 {
        conn.execute_batch(MIGRATION_V11).unwrap();
    }
    if content_version >= 12 {
        conn.execute_batch(MIGRATION_V12).unwrap();
    }
    if content_version >= 13 {
        conn.execute_batch(MIGRATION_V13).unwrap();
    }
    if content_version >= 14 {
        conn.execute_batch(MIGRATION_V14).unwrap();
    }
    if content_version >= 15 {
        conn.execute_batch(MIGRATION_V15).unwrap();
    }
    if content_version >= 16 {
        conn.execute_batch(MIGRATION_V16).unwrap();
    }
    conn.execute_batch(&format!("PRAGMA user_version = {content_version};"))
        .unwrap();

    insert_v1_rows(&conn);
    if content_version >= 2 {
        insert_v2_rows(&conn);
    }
    if content_version >= 3 {
        insert_v3_rows(&conn);
        // FTS5's own consistency check: it tokenizes `fts_blocks` again and compares the
        // words with the index the triggers built.
        conn.execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    }
    if content_version >= 4 {
        insert_v4_rows(&conn);
    }
    if content_version >= 5 {
        insert_v5_rows(&conn);
    }
    if content_version >= 6 {
        insert_v6_rows(&conn);
    }
    if content_version >= 8 {
        insert_v8_rows(&conn);
    }
    if content_version >= 9 {
        insert_v9_rows(&conn);
    }
    if content_version >= 10 {
        insert_v10_rows(&conn);
        conn.execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    }
    if content_version >= 11 {
        insert_v11_rows(&conn);
        conn.execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    }
    if content_version >= 12 {
        insert_v12_rows(&conn);
        conn.execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    }
    if content_version >= 13 {
        insert_v13_rows(&conn);
        conn.execute(
            "INSERT INTO fts (fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    }
    if content_version >= 14 {
        insert_v14_rows(&conn);
    }
    if content_version >= 15 {
        insert_v15_rows(&conn);
    }
    if content_version >= 16 {
        insert_v16_rows(&conn);
    }
    if version >= 17 {
        // The migration moves the rows above to the personal feed; what a device that shares a space
        // holds comes after it.
        conn.execute_batch(MIGRATION_V17).unwrap();
        conn.execute_batch("PRAGMA user_version = 17;").unwrap();
        insert_v17_rows(&conn);
    }
    if version >= 18 {
        // The migration rebuilds `page_properties` and adds the new tables and columns; the comment
        // rows come after it.
        conn.execute_batch(MIGRATION_V18).unwrap();
        conn.execute_batch("PRAGMA user_version = 18;").unwrap();
        insert_v18_rows(&conn);
        conn.execute(
            "INSERT INTO comment_fts (comment_fts, rank) VALUES ('integrity-check', 1)",
            [],
        )
        .unwrap();
    }

    let violations: i64 = conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0, "the fixture breaks a foreign key");
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    conn.close().map_err(|(_, e)| e).unwrap();
}

/// The version 1 content (device identity, workspace, settings), unchanged since v1.
fn insert_v1_rows(conn: &Connection) {
    for (key, value) in [
        ("device_id", DEVICE_ID),
        ("device_name", "Fixture laptop"),
        ("created_at", "1760000000000"),
        ("core_version_at_creation", "0.1.0"),
    ] {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO workspace (id, name, created_at) VALUES (?1, ?2, ?3)",
        params![
            &WORKSPACE_ID[..],
            "Fixture workspace",
            1_760_000_001_000_i64
        ],
    )
    .unwrap();
    for (key, value, updated_at) in [
        (
            "ui.theme",
            r#"{"accent":"violet","mode":"dark"}"#,
            1_760_000_002_000_i64,
        ),
        ("app.locale", r#""en""#, 1_760_000_003_000_i64),
    ] {
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![key, value, updated_at],
        )
        .unwrap();
    }
}

// ---- version 2 --------------------------------------------------------------------
//
// The ids are UUID text so that `tests/files.rs` can name the same rows.

const SPACE_PERSONAL: &str = "5ba0ce00-0000-4000-8000-000000000001";
const SPACE_WORK: &str = "5ba0ce00-0000-4000-8000-000000000002";

const DOC_ACCOUNT: &str = "d0c00000-0000-4000-8000-00000000000a";
const DOC_STRUCTURE_PERSONAL: &str = "d0c00000-0000-4000-8000-000000000051";
const DOC_STRUCTURE_WORK: &str = "d0c00000-0000-4000-8000-000000000052";

const PAGE_HOME: &str = "9a6e0000-0000-4000-8000-000000000001";
const PAGE_NOTES: &str = "9a6e0000-0000-4000-8000-000000000002";
const PAGE_OLD: &str = "9a6e0000-0000-4000-8000-000000000003";
const PAGE_OLD_CHILD: &str = "9a6e0000-0000-4000-8000-000000000004";
const PAGE_ROADMAP: &str = "9a6e0000-0000-4000-8000-000000000005";

/// The 16 bytes of an id written as UUID text (all id types parse alike).
fn id(text: &str) -> [u8; 16] {
    text.parse::<DocId>().unwrap().to_bytes()
}

/// Opaque stand-in for the snapshot of the personal structure document.
const SNAPSHOT_BLOB: &[u8] = b"KBD1\x01\x01\x01\x00snapshot\xff\x00\x80 personal structure";

/// Opaque stand-in for its encoded version.
const SNAPSHOT_VERSION: &[u8] = &[0x01, 0x02, 0x00, 0xff, 0x7f];

/// Opaque stand-in for an update blob: `[0xAA, last id byte, seq, 0x00, 0xFF]`.
fn update_blob(doc: &str, seq: u8) -> Vec<u8> {
    vec![0xAA, id(doc)[15], seq, 0x00, 0xFF]
}

fn insert_v2_rows(conn: &Connection) {
    // Inserted in a different order than they were created, so the fixture shows that
    // listings sort by `created_at` and not by row order.
    for (doc, kind, space, created_at) in [
        (
            DOC_STRUCTURE_WORK,
            0_i64,
            Some(SPACE_WORK),
            1_760_000_030_000_i64,
        ),
        (DOC_ACCOUNT, 2, None, 1_760_000_010_000),
        (
            DOC_STRUCTURE_PERSONAL,
            0,
            Some(SPACE_PERSONAL),
            1_760_000_020_000,
        ),
    ] {
        conn.execute(
            "INSERT INTO docs (doc_id, kind, space_id, created_at, format_version, \
             min_reader_version) VALUES (?1, ?2, ?3, ?4, 1, 1)",
            params![id(doc), kind, space.map(id), created_at],
        )
        .unwrap();
    }

    // The personal structure document was compacted after its second update: the
    // snapshot stands for updates 1 and 2, the log that remains starts at 3.
    conn.execute(
        "INSERT INTO doc_snapshots (doc_id, version, blob, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            id(DOC_STRUCTURE_PERSONAL),
            SNAPSHOT_VERSION,
            SNAPSHOT_BLOB,
            1_760_000_040_000_i64
        ],
    )
    .unwrap();

    // (document, local_seq, origin: 0 local, 1 remote, 2 core, created_at)
    for (doc, seq, origin, created_at) in [
        (DOC_ACCOUNT, 1_u8, 0_i64, 1_760_000_011_000_i64),
        (DOC_ACCOUNT, 2, 0, 1_760_000_012_000),
        (DOC_STRUCTURE_PERSONAL, 3, 0, 1_760_000_050_000),
        (DOC_STRUCTURE_PERSONAL, 4, 1, 1_760_000_051_000),
        (DOC_STRUCTURE_PERSONAL, 5, 2, 1_760_000_052_000),
        (DOC_STRUCTURE_WORK, 1, 0, 1_760_000_031_000),
    ] {
        conn.execute(
            "INSERT INTO doc_updates (doc_id, local_seq, blob, origin, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id(doc), seq, update_blob(doc, seq), origin, created_at],
        )
        .unwrap();
    }

    // Personal: Home > Notes live; "Old ideas" trashed with its child "Archived draft"
    // (trashed_at is set on every page of a trashed subtree). Work: Roadmap.
    // (page, space, parent, title, trashed_at, updated_at)
    for (page, space, parent, title, trashed_at, updated_at) in [
        (
            PAGE_HOME,
            SPACE_PERSONAL,
            None,
            "Home",
            None,
            1_760_000_021_000_i64,
        ),
        (
            PAGE_NOTES,
            SPACE_PERSONAL,
            Some(PAGE_HOME),
            "Notes",
            None,
            1_760_000_022_000,
        ),
        (
            PAGE_OLD,
            SPACE_PERSONAL,
            None,
            "Old ideas",
            Some(1_760_000_060_000_i64),
            1_760_000_060_000,
        ),
        (
            PAGE_OLD_CHILD,
            SPACE_PERSONAL,
            Some(PAGE_OLD),
            "Éléments archivés",
            Some(1_760_000_060_000),
            1_760_000_023_000,
        ),
        (
            PAGE_ROADMAP,
            SPACE_WORK,
            None,
            "Roadmap",
            None,
            1_760_000_031_000,
        ),
    ] {
        conn.execute(
            "INSERT INTO pages_index (page_id, space_id, parent_id, title, trashed_at, \
             updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id(page),
                id(space),
                parent.map(id),
                title,
                trashed_at,
                updated_at
            ],
        )
        .unwrap();
    }
}

// ---- version 3 --------------------------------------------------------------------
//
// Page documents, the text index and the link index, on top of the version 2 content. The
// ids are UUID text again; `tests/files.rs` names the same rows.

const BLOCK_HOME_WELCOME: &str = "b10c0000-0000-4000-8000-000000000001";
const BLOCK_HOME_PLAN: &str = "b10c0000-0000-4000-8000-000000000002";
const BLOCK_NOTES_MEETING: &str = "b10c0000-0000-4000-8000-000000000003";
const BLOCK_NOTES_BACK: &str = "b10c0000-0000-4000-8000-000000000004";
const BLOCK_NOTES_QUOTE: &str = "b10c0000-0000-4000-8000-000000000005";
const BLOCK_OLD_IDEAS: &str = "b10c0000-0000-4000-8000-000000000006";
const BLOCK_ARCHIVED: &str = "b10c0000-0000-4000-8000-000000000007";
const BLOCK_ROADMAP_SHIP: &str = "b10c0000-0000-4000-8000-000000000008";
const BLOCK_ROADMAP_CAFE: &str = "b10c0000-0000-4000-8000-000000000009";

/// Opaque stand-in for the snapshot of the "Notes" page document.
const PAGE_SNAPSHOT_BLOB: &[u8] = b"KBD1\x01\x01\x01\x01snapshot\xff\x00\x80 page notes";

fn insert_v3_rows(conn: &Connection) {
    // Page documents: their id is the page id (ADR-0016), their space the page's space.
    // The trashed child "Archived draft" was never opened, so it has no document.
    // (page, space, created_at)
    for (page, space, created_at) in [
        (PAGE_HOME, SPACE_PERSONAL, 1_760_000_070_000_i64),
        (PAGE_NOTES, SPACE_PERSONAL, 1_760_000_071_000),
        (PAGE_OLD, SPACE_PERSONAL, 1_760_000_072_000),
        (PAGE_ROADMAP, SPACE_WORK, 1_760_000_073_000),
    ] {
        conn.execute(
            "INSERT INTO docs (doc_id, kind, space_id, created_at, format_version, \
             min_reader_version) VALUES (?1, 1, ?2, ?3, 1, 1)",
            params![id(page), id(space), created_at],
        )
        .unwrap();
    }

    // The "Notes" document was compacted after its second update: snapshot, then the
    // log from 3 on. The other documents have a log only.
    conn.execute(
        "INSERT INTO doc_snapshots (doc_id, version, blob, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            id(PAGE_NOTES),
            SNAPSHOT_VERSION,
            PAGE_SNAPSHOT_BLOB,
            1_760_000_080_000_i64
        ],
    )
    .unwrap();
    // (document, local_seq, origin: 0 local, 1 remote, 2 core, created_at)
    for (doc, seq, origin, created_at) in [
        (PAGE_HOME, 1_u8, 2_i64, 1_760_000_074_000_i64),
        (PAGE_HOME, 2, 0, 1_760_000_075_000),
        (PAGE_NOTES, 3, 0, 1_760_000_081_000),
        (PAGE_NOTES, 4, 0, 1_760_000_082_000),
        (PAGE_OLD, 1, 0, 1_760_000_076_000),
        (PAGE_ROADMAP, 1, 2, 1_760_000_077_000),
    ] {
        conn.execute(
            "INSERT INTO doc_updates (doc_id, local_seq, blob, origin, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id(doc), seq, update_blob(doc, seq), origin, created_at],
        )
        .unwrap();
    }

    // The text index as the core writes it: per page one title row (no block id, empty
    // body), then one row per text-bearing block (empty title). Page references are
    // resolved to titles ("Plan: see Notes and Roadmap"). Trashed pages keep their rows.
    // Only `fts_blocks` is written; the triggers of the migration fill the word index.
    // (page, block, title, body)
    for (page, block, title, body) in [
        (PAGE_HOME, None, "Home", ""),
        (
            PAGE_HOME,
            Some(BLOCK_HOME_WELCOME),
            "",
            "Welcome to the fixture workspace. Café notes and ideas live under Notes.",
        ),
        (
            PAGE_HOME,
            Some(BLOCK_HOME_PLAN),
            "",
            "Plan: see Notes and Roadmap",
        ),
        (PAGE_NOTES, None, "Notes", ""),
        (
            PAGE_NOTES,
            Some(BLOCK_NOTES_MEETING),
            "",
            "Meeting notes with \u{1D518}nicode \u{1F389} and café \u{2615}",
        ),
        (PAGE_NOTES, Some(BLOCK_NOTES_BACK), "", "Back to Home"),
        (
            PAGE_NOTES,
            Some(BLOCK_NOTES_QUOTE),
            "",
            "Quoting the first paragraph of Home",
        ),
        (PAGE_OLD, None, "Old ideas", ""),
        (
            PAGE_OLD,
            Some(BLOCK_OLD_IDEAS),
            "",
            "Ideas that did not survive; see Home",
        ),
        (PAGE_OLD_CHILD, None, "Éléments archivés", ""),
        (
            PAGE_OLD_CHILD,
            Some(BLOCK_ARCHIVED),
            "",
            "Brouillon archivé: éléments à relire",
        ),
        (PAGE_ROADMAP, None, "Roadmap", ""),
        (
            PAGE_ROADMAP,
            Some(BLOCK_ROADMAP_SHIP),
            "",
            "Ship search and backlinks in milestone 3",
        ),
        (
            PAGE_ROADMAP,
            Some(BLOCK_ROADMAP_CAFE),
            "",
            "Notes from the café on Friday",
        ),
    ] {
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body) VALUES (?1, ?2, ?3, ?4)",
            params![id(page), block.map(id), title, body],
        )
        .unwrap();
    }

    // The link index: one row per (referencing block, referenced page). The trashed
    // page "Old ideas" links to Home (its row stays, backlinks leave it out) and one link
    // crosses spaces both ways.
    // (from page, from block, to page, to block)
    for (from_page, from_block, to_page, to_block) in [
        (PAGE_HOME, BLOCK_HOME_PLAN, PAGE_NOTES, None),
        (PAGE_HOME, BLOCK_HOME_PLAN, PAGE_ROADMAP, None),
        (PAGE_NOTES, BLOCK_NOTES_BACK, PAGE_HOME, None),
        (
            PAGE_NOTES,
            BLOCK_NOTES_QUOTE,
            PAGE_HOME,
            Some(BLOCK_HOME_WELCOME),
        ),
        (PAGE_OLD, BLOCK_OLD_IDEAS, PAGE_HOME, None),
        (PAGE_ROADMAP, BLOCK_ROADMAP_CAFE, PAGE_NOTES, None),
    ] {
        conn.execute(
            "INSERT INTO links (from_page, from_block, to_page, to_block) \
             VALUES (?1, ?2, ?3, ?4)",
            params![id(from_page), id(from_block), id(to_page), to_block.map(id)],
        )
        .unwrap();
    }
}

// ---- version 4 --------------------------------------------------------------------
//
// The keys, on top of the version 3 content: the `meta` rows of a recovery kit that was
// created and not yet verified or re-issued, the device log and the keyring. This crate
// never interprets a log entry or a keyring record (`kb-keys` encodes, decodes and
// verifies them), so they are opaque stand-ins that only have to survive a round trip byte
// for byte; like the document blobs of v2 they contain NUL and high bytes on purpose. The
// ids are UUID text again; `tests/files.rs` names the same rows and rebuilds the same
// stand-ins with the same formulas.

/// The account id (the first 16 bytes of the Genesis hash, written like every other id).
const ACCOUNT_ID: &str = "ac000000-0000-4000-8000-000000000001";

/// A page that was purged: its document is gone and only a keyring tombstone is left.
const PAGE_PURGED: &str = "9a6e0000-0000-4000-8000-000000000006";

/// `len` bytes counting up from `start` (wrapping at 256): every position differs.
fn ramp(start: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| start.wrapping_add(u8::try_from(i % 256).unwrap()))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Opaque stand-in for the encoded device log entry `seq`: `KLE1`, the sequence number,
/// NUL and high bytes, then a filler whose length grows with the sequence number.
fn log_entry(seq: u8) -> Vec<u8> {
    let mut entry = b"KLE1".to_vec();
    entry.extend([seq, 0x00, 0xFF, 0x80]);
    entry.extend(ramp(0xA0 + seq, 24 * (usize::from(seq) + 1)));
    entry
}

/// A fake 32-byte hash for log entry `seq` (the real one is the BLAKE3 hash of the entry).
fn log_hash(seq: u8) -> Vec<u8> {
    ramp(0x10 * (seq + 1), 32)
}

/// A subject as `kb-keys` encodes it: a tag byte, the document id for a document key, and
/// the epoch (u32, little-endian). 0x00 is a space key, 0x01 a document key.
fn subject_space(epoch: u32) -> Vec<u8> {
    let mut subject = vec![0x00];
    subject.extend(epoch.to_le_bytes());
    subject
}

fn subject_doc(doc: &str, epoch: u32) -> Vec<u8> {
    let mut subject = vec![0x01];
    subject.extend(id(doc));
    subject.extend(epoch.to_le_bytes());
    subject
}

/// Opaque stand-in for an encoded keyring item: `KKR1`, a tag (1 record, 2 tombstone), NUL
/// and high bytes, then 100 bytes counting up from `seed`.
fn keyring_item(tag: u8, seed: u8) -> Vec<u8> {
    let mut item = b"KKR1".to_vec();
    item.extend([tag, 0x00, 0xFF, 0x80]);
    item.extend(ramp(seed, 100));
    item
}

fn insert_v4_rows(conn: &Connection) {
    // The `meta` keys that creating the recovery kit adds: the account id, the recovery id
    // (hex of 32 bytes), the recovery blob (hex of the 72-byte wrapped root key) and the
    // time of the kit. `recovery_verified_at` and `recovery_reissued_at` come later.
    for (key, value) in [
        ("account_id", ACCOUNT_ID.to_owned()),
        ("recovery_id", hex(&ramp(0x40, 32))),
        ("recovery_blob", hex(&ramp(0x80, 72))),
        ("recovery_created_at", "1760000100000".to_owned()),
    ] {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .unwrap();
    }

    // The device log: Genesis, a device added by it, a recovery key rotation.
    for seq in 0..3_u8 {
        conn.execute(
            "INSERT INTO device_log (seq, entry, hash) VALUES (?1, ?2, ?3)",
            params![i64::from(seq), log_entry(seq), log_hash(seq)],
        )
        .unwrap();
    }

    // The keyring, written in neither the order it is listed in nor the order of the ids.
    // Per space: the space key and the document keys of its documents; the purged page of
    // the personal space has a tombstone instead of a record.
    // (space, subject, tag: 1 record, 2 tombstone, seed of the stand-in)
    for (space, subject, tag, seed) in [
        (SPACE_WORK, subject_space(0), 1_u8, 0x31_u8),
        (SPACE_PERSONAL, subject_doc(PAGE_NOTES, 0), 1, 0x12),
        (SPACE_WORK, subject_doc(PAGE_ROADMAP, 0), 1, 0x33),
        (SPACE_PERSONAL, subject_space(0), 1, 0x10),
        (SPACE_PERSONAL, subject_doc(PAGE_PURGED, 0), 2, 0x14),
        (
            SPACE_PERSONAL,
            subject_doc(DOC_STRUCTURE_PERSONAL, 0),
            1,
            0x15,
        ),
        (SPACE_WORK, subject_doc(DOC_STRUCTURE_WORK, 0), 1, 0x32),
        (SPACE_PERSONAL, subject_doc(PAGE_HOME, 0), 1, 0x11),
    ] {
        conn.execute(
            "INSERT INTO keyring (space_id, subject, record, tombstone) VALUES (?1, ?2, ?3, ?4)",
            params![id(space), subject, keyring_item(tag, seed), tag == 2],
        )
        .unwrap();
    }
}

// ---- version 5 --------------------------------------------------------------------
//
// The attachment index and references, on top of the version 4 content. The manifests live
// inside the structure documents (opaque blobs in this fixture), so only the two derived
// tables are written. The ids are UUID text again; `tests/files.rs` names the same rows.

const ATT_PHOTO: &str = "a77a0000-0000-4000-8000-000000000001";
const ATT_REPORT: &str = "a77a0000-0000-4000-8000-000000000002";
const ATT_ARCHIVE: &str = "a77a0000-0000-4000-8000-000000000003";
const ATT_EMPTY: &str = "a77a0000-0000-4000-8000-000000000004";
const ATT_LATE: &str = "a77a0000-0000-4000-8000-000000000005";
const ATT_SKETCH: &str = "a77a0000-0000-4000-8000-000000000006";
/// An attachment that was purged: it has no index row, but a block still references it.
const ATT_PURGED: &str = "a77a0000-0000-4000-8000-000000000007";

const BLOCK_HOME_PHOTO: &str = "b10c0000-0000-4000-8000-0000000000a1";
const BLOCK_NOTES_PHOTO: &str = "b10c0000-0000-4000-8000-0000000000a2";
const BLOCK_NOTES_REPORT: &str = "b10c0000-0000-4000-8000-0000000000a3";
const BLOCK_OLD_SKETCH: &str = "b10c0000-0000-4000-8000-0000000000a4";
const BLOCK_ROADMAP_EMPTY: &str = "b10c0000-0000-4000-8000-0000000000a5";
const BLOCK_ROADMAP_GONE: &str = "b10c0000-0000-4000-8000-0000000000a6";

fn insert_v5_rows(conn: &Connection) {
    // Written in neither the order of the times nor the order of the ids. The photo and the
    // report share a creation time (equal times are listed by id), the archive and the late
    // file lost their last reference (the archive earlier than the late file), and the sizes
    // run from 0 to 4 MiB + 1 and 9 MiB.
    // (attachment, space, name, mime, size, chunk_count, created_at, unreferenced_since)
    for (attachment, space, name, mime, size, chunk_count, created_at, unreferenced_since) in [
        (
            ATT_LATE,
            SPACE_WORK,
            "late.bin",
            "application/octet-stream",
            4_194_305_i64,
            2_i64,
            1_760_000_230_000_i64,
            Some(1_760_000_400_000_i64),
        ),
        (
            ATT_REPORT,
            SPACE_PERSONAL,
            "Q3 report \u{2014} final.pdf",
            "application/pdf",
            9_437_184,
            3,
            1_760_000_200_000,
            None,
        ),
        (
            ATT_EMPTY,
            SPACE_WORK,
            "empty.txt",
            "text/plain",
            0,
            1,
            1_760_000_220_000,
            None,
        ),
        (
            ATT_PHOTO,
            SPACE_PERSONAL,
            "Café photo.png",
            "image/png",
            1_234_567,
            1,
            1_760_000_200_000,
            None,
        ),
        (
            ATT_ARCHIVE,
            SPACE_PERSONAL,
            "archive.zip",
            "application/zip",
            4_194_304,
            1,
            1_760_000_210_000,
            Some(1_760_000_300_000),
        ),
        (
            ATT_SKETCH,
            SPACE_PERSONAL,
            "Old sketch.jpg",
            "image/jpeg",
            51_200,
            1,
            1_760_000_190_000,
            None,
        ),
    ] {
        conn.execute(
            "INSERT INTO attachments_index (attachment_id, space_id, name, mime, size, \
             chunk_count, created_at, unreferenced_since) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id(attachment),
                id(space),
                name,
                mime,
                size,
                chunk_count,
                created_at,
                unreferenced_since
            ],
        )
        .unwrap();
    }

    // The references: the photo is shown by two pages, the sketch by a trashed page (it
    // still counts), and the purged attachment is still shown by a block of the roadmap.
    // (attachment, page, block)
    for (attachment, page, block) in [
        (ATT_PURGED, PAGE_ROADMAP, BLOCK_ROADMAP_GONE),
        (ATT_PHOTO, PAGE_NOTES, BLOCK_NOTES_PHOTO),
        (ATT_EMPTY, PAGE_ROADMAP, BLOCK_ROADMAP_EMPTY),
        (ATT_PHOTO, PAGE_HOME, BLOCK_HOME_PHOTO),
        (ATT_SKETCH, PAGE_OLD, BLOCK_OLD_SKETCH),
        (ATT_REPORT, PAGE_NOTES, BLOCK_NOTES_REPORT),
    ] {
        conn.execute(
            "INSERT INTO attachment_refs (attachment_id, page_id, block_id) VALUES (?1, ?2, ?3)",
            params![id(attachment), id(page), id(block)],
        )
        .unwrap();
    }
}

// ---- version 6 --------------------------------------------------------------------
//
// The sync state, on top of the version 5 content: what a device that has synced with a
// server holds. The signed heads and the pending entries are opaque stand-ins (kb-sync
// encodes and verifies them), like the other blobs, with NUL and high bytes on purpose. The
// ids are UUID text again; tests/files.rs names the same rows and rebuilds the same
// stand-ins with the same formulas.

/// A second device of the account (the first is DEVICE_ID, the one of the `meta` rows).
const PEER_DEVICE: &str = "de71ce00-0000-4000-8000-000000000002";

/// A page whose structure document this device does not have: only a pending entry names it.
const PAGE_UNKNOWN: &str = "9a6e0000-0000-4000-8000-000000000008";

/// The feed document id of the keyring entries (all zero, not a row of docs).
const KEYRING_DOC: [u8; 16] = [0; 16];

/// Opaque stand-in for a signed head: KHD1, the device number, NUL and high bytes, then a
/// filler.
fn signed_head(device: u8, cursor: u8) -> Vec<u8> {
    let mut head = b"KHD1".to_vec();
    head.extend([device, 0x00, 0xFF, 0x80]);
    head.extend(ramp(0xC0 + cursor, 40));
    head
}

/// Opaque stand-in for a pending entry (the envelope as received): KEN1, the cursor, NUL
/// and high bytes, then a filler whose length grows with the cursor.
fn pending_entry(cursor: u8) -> Vec<u8> {
    let mut entry = b"KEN1".to_vec();
    entry.extend([cursor, 0x00, 0xFF, 0x80]);
    entry.extend(ramp(0x20 + cursor, 48 + usize::from(cursor)));
    entry
}

fn insert_v6_rows(conn: &Connection) {
    // The client settings, written in neither the order of the keys nor of the migration
    // comment: the server, the epoch and the chain hash as hex, the cursor, the times of the
    // last push and pull and the fork flag.
    for (key, value) in [
        ("last_pull_at", "1760000510000".to_owned()),
        ("server_url", "https://sync.example.test:8787".to_owned()),
        ("remote_cursor", "42".to_owned()),
        ("fork_detected", "0".to_owned()),
        ("chain_hash", hex(&ramp(0x70, 32))),
        ("enabled", "1".to_owned()),
        ("server_epoch", hex(&ramp(0x60, 16))),
        ("last_push_at", "1760000500000".to_owned()),
    ] {
        conn.execute(
            "INSERT INTO sync_config (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .unwrap();
    }

    // The per-document state. Every number differs from every other in its column, so a
    // swapped column is noticed. The "Old ideas" page document has no row. The roadmap is
    // as a server epoch change leaves a document: nothing pushed, no remote snapshot, the
    // device sequence and the old snapshot coverage kept. The keyring has a row under the
    // all-zero id.
    // (document, pushed_up_to_seq, next_device_seq, snapshot_covers,
    //  entries_since_snapshot, bytes_since_snapshot, remote_snapshot_covers)
    type State = ([u8; 16], i64, i64, Option<i64>, i64, i64, Option<i64>);
    let states: [State; 7] = [
        (id(PAGE_NOTES), 4, 9, Some(41), 8, 777, Some(41)),
        (KEYRING_DOC, 0, 3, None, 0, 0, None),
        (id(DOC_STRUCTURE_WORK), 0, 1, None, 0, 0, None),
        (id(DOC_ACCOUNT), 2, 3, Some(12), 1, 111, Some(12)),
        (id(PAGE_HOME), 2, 7, None, 6, 4_194_304, None),
        (
            id(DOC_STRUCTURE_PERSONAL),
            3,
            4,
            Some(30),
            5,
            2_048,
            Some(28),
        ),
        (id(PAGE_ROADMAP), 0, 5, Some(33), 0, 0, None),
    ];
    for (doc, pushed, next, covers, entries, bytes, remote) in states {
        conn.execute(
            "INSERT INTO sync_doc_state (doc_id, pushed_up_to_seq, next_device_seq, \
             snapshot_covers, entries_since_snapshot, bytes_since_snapshot, \
             remote_snapshot_covers) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![doc.as_slice(), pushed, next, covers, entries, bytes, remote],
        )
        .unwrap();
    }

    // Continuity: the last device sequence applied per (document, device).
    // (document, device, last_seq)
    for (doc, device, last_seq) in [
        (id(PAGE_NOTES), id(PEER_DEVICE), 3_i64),
        (id(DOC_ACCOUNT), id(DEVICE_ID), 2),
        (KEYRING_DOC, id(PEER_DEVICE), 1),
        (id(DOC_STRUCTURE_PERSONAL), id(PEER_DEVICE), 12),
        (id(DOC_ACCOUNT), id(PEER_DEVICE), 7),
    ] {
        conn.execute(
            "INSERT INTO sync_seen (doc_id, device_id, last_seq) VALUES (?1, ?2, ?3)",
            params![doc.as_slice(), device.as_slice(), last_seq],
        )
        .unwrap();
    }

    // The latest head of two devices; this device's own chain hash is the one of the
    // configuration.
    // (device, cursor, chain hash, signed at, stand-in, seen at)
    for (device, cursor, chain_hash, at_ms, signed, seen_at) in [
        (
            id(PEER_DEVICE),
            40_i64,
            ramp(0x90, 32),
            1_760_000_490_000_i64,
            signed_head(2, 40),
            1_760_000_511_000_i64,
        ),
        (
            id(DEVICE_ID),
            42,
            ramp(0x70, 32),
            1_760_000_500_000,
            signed_head(1, 42),
            1_760_000_500_500,
        ),
    ] {
        conn.execute(
            "INSERT INTO sync_heads (device_id, cursor, chain_hash, at_ms, signed, seen_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                device.as_slice(),
                cursor,
                chain_hash,
                at_ms,
                signed,
                seen_at
            ],
        )
        .unwrap();
    }

    // Entries that wait: two for the "Old ideas" page (their key is missing, reason 1) and
    // one for a page whose structure document is missing (reason 2). Inserted out of
    // cursor order.
    // (cursor, document, reason, received at)
    for (cursor, doc, reason, received_at) in [
        (23_u8, PAGE_OLD, 1_i64, 1_760_000_512_000_i64),
        (17, PAGE_OLD, 1, 1_760_000_511_500),
        (19, PAGE_UNKNOWN, 2, 1_760_000_511_800),
    ] {
        conn.execute(
            "INSERT INTO sync_pending (cursor, doc_id, bytes, reason, received_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                i64::from(cursor),
                id(doc).as_slice(),
                pending_entry(cursor),
                reason,
                received_at
            ],
        )
        .unwrap();
    }

    // The published markers: the work space's keys and the personal space key were sent
    // (four of the eight rows), the other four were created or changed since; the Genesis
    // entry of the device log and the entry that added the second device were sent, the
    // last one was not.
    conn.execute(
        "UPDATE keyring SET published = 1 WHERE space_id = ?1",
        [id(SPACE_WORK).as_slice()],
    )
    .unwrap();
    conn.execute(
        "UPDATE keyring SET published = 1 WHERE space_id = ?1 AND subject = ?2",
        params![id(SPACE_PERSONAL).as_slice(), subject_space(0)],
    )
    .unwrap();
    conn.execute("UPDATE device_log SET published = 1 WHERE seq <= 1", [])
        .unwrap();
}

// ---- version 8 --------------------------------------------------------------------
//
// The per-chunk transfer state of attachment sync, on top of the version 7 content: what a
// device that is in the middle of moving attachments holds. The chunk ids are stand-ins for
// the BLAKE3 hash of a ciphertext: 32 bytes counting up from a first byte that tells them
// apart (`ramp(first, 32)`, wrapping at 256, so some hold NUL and 0xFF); `tests/files.rs`
// rebuilds the same ids with the same formula. The attachments are those of the version 5
// content: the report (three chunks), the late file (two) and the purged attachment that
// only a block reference still names. The rows are inserted out of hash order on purpose.

/// First byte of the stand-in chunk ids.
const CHUNK_REPORT_1: u8 = 0xC3;
const CHUNK_REPORT_2: u8 = 0x21;
const CHUNK_REPORT_3: u8 = 0x84;
const CHUNK_LATE_1: u8 = 0x5A;
const CHUNK_PURGED_1: u8 = 0xE7;

fn insert_v8_rows(conn: &Connection) {
    // The report went up in part before the app was killed: its first chunk is synced and
    // the other two are still to upload (the second has already failed three times). The
    // late file is wanted from the server, which did not hold its first chunk at the last
    // two tries, so it waits until `next_at`. The purged attachment's chunk is to be
    // deleted on the server, after one failed try. Every other chunk has no row.
    // (first byte of the chunk id, attachment, state, attempts, next_at)
    // States: 1 upload, 2 download, 3 synced, 4 delete.
    for (first, attachment, state, attempts, next_at) in [
        (CHUNK_PURGED_1, ATT_PURGED, 4_i64, 1_i64, None),
        (CHUNK_REPORT_2, ATT_REPORT, 1, 3, None),
        (CHUNK_LATE_1, ATT_LATE, 2, 2, Some(1_760_000_600_000_i64)),
        (CHUNK_REPORT_3, ATT_REPORT, 1, 0, None),
        (CHUNK_REPORT_1, ATT_REPORT, 3, 0, None),
    ] {
        conn.execute(
            "INSERT INTO chunk_sync (hash, attachment_id, state, attempts, next_at)              VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                ramp(first, 32),
                id(attachment).as_slice(),
                state,
                attempts,
                next_at
            ],
        )
        .unwrap();
    }
}

// ---- version 9 --------------------------------------------------------------------
//
// The date mentions of the pages (reminders, ADR-0024), on top of the version 8 content: what
// a device whose pages hold dates writes. The pages and blocks are those of the version 3
// content. Five mentions: two with a time of day (one with a reminder of 15 minutes, one with
// a day), a whole-day one without a reminder, a whole-day one of the trashed page "Old ideas"
// that reminds at the time (09:00, 0 minutes), and one of the Work space with a reminder of
// two days. The fire times are seconds since 1970-01-01T00:00:00 on the local wall clock
// (the calendar fields counted as UTC), computed by `kb_model` and written down here:
//
// | moment                 | seconds          |
// |------------------------|------------------|
// | 2026-10-07 14:30       | 1_791_383_400    |
// | 2026-10-09 09:00       | 1_791_536_400    |
// | 2027-01-15 08:00       | 1_800_000_000    |

/// The mentions' own ids (the `id` of the `dateMention` nodes).
const MENTION_HOME_MEETING: &str = "d47e0000-0000-4000-8000-000000000001";
const MENTION_HOME_PLAIN: &str = "d47e0000-0000-4000-8000-000000000002";
const MENTION_NOTES_REVIEW: &str = "d47e0000-0000-4000-8000-000000000003";
const MENTION_OLD_DEADLINE: &str = "d47e0000-0000-4000-8000-000000000004";
const MENTION_ROADMAP_SHIP: &str = "d47e0000-0000-4000-8000-000000000005";

fn insert_v9_rows(conn: &Connection) {
    // (page, block, mention, date, time, reminder minutes, fire time), in the order the core
    // writes them: by page, in document order. `doc_id` is the page id (ADR-0016).
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<i64>,
        Option<i64>,
    );
    let rows: [Row; 5] = [
        (
            PAGE_HOME,
            BLOCK_HOME_PLAN,
            MENTION_HOME_MEETING,
            "2026-10-07",
            Some("14:30"),
            Some(15),
            Some(1_791_382_500),
        ),
        (
            PAGE_HOME,
            BLOCK_HOME_WELCOME,
            MENTION_HOME_PLAIN,
            "2026-10-08",
            None,
            None,
            None,
        ),
        (
            PAGE_NOTES,
            BLOCK_NOTES_MEETING,
            MENTION_NOTES_REVIEW,
            "2026-10-09",
            Some("09:00"),
            Some(1440),
            Some(1_791_450_000),
        ),
        (
            PAGE_OLD,
            BLOCK_OLD_IDEAS,
            MENTION_OLD_DEADLINE,
            "2026-10-09",
            None,
            Some(0),
            Some(1_791_536_400),
        ),
        (
            PAGE_ROADMAP,
            BLOCK_ROADMAP_SHIP,
            MENTION_ROADMAP_SHIP,
            "2027-01-15",
            Some("08:00"),
            Some(2880),
            Some(1_799_827_200),
        ),
    ];
    for (page, block, mention, date, time, reminder, fire) in rows {
        conn.execute(
            "INSERT INTO date_mentions \
               (doc_id, page_id, block_id, node_id, date, time, reminder_minutes, fire_at_local) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id(page).as_slice(),
                id(page).as_slice(),
                id(block).as_slice(),
                id(mention).as_slice(),
                date,
                time,
                reminder,
                fire
            ],
        )
        .unwrap();
    }
}

// ---- version 10 -------------------------------------------------------------------
//
// The property values of the pages (typed page properties, ADR-0026), on top of the version 9
// content: what a device whose pages carry properties writes. The pages are those of the version
// 3 content. The schema is small: Tags (the built-in multi-select with its fixed id), a text, a
// number, a checkbox, a date, a status, a select and a URL. Four pages have values: the two live
// pages of the Personal space (the first with a value of most kinds), the trashed page "Old
// ideas" (which keeps its rows) and the Roadmap of the Work space. The rows are inserted out of
// page order on purpose, and within a page in the order of the schema (Tags first), as the core
// writes them. The text index gets a property row (`kind = 1`, the property's id as the block,
// the option names or the text value as the body) for every property of those pages that has
// searchable text.

const PROP_TAGS: &str = "0000000a-0000-4000-8000-000000000001";
const PROP_SUMMARY: &str = "b09e0000-0000-4000-8000-000000000001";
const PROP_COST: &str = "b09e0000-0000-4000-8000-000000000002";
const PROP_DONE: &str = "b09e0000-0000-4000-8000-000000000003";
const PROP_DUE: &str = "b09e0000-0000-4000-8000-000000000004";
const PROP_STAGE: &str = "b09e0000-0000-4000-8000-000000000005";
const PROP_AREA: &str = "b09e0000-0000-4000-8000-000000000006";
const PROP_LINK: &str = "b09e0000-0000-4000-8000-000000000007";

const OPT_URGENT: &str = "0b710000-0000-4000-8000-000000000001";
const OPT_WORK: &str = "0b710000-0000-4000-8000-000000000002";
const OPT_LATER: &str = "0b710000-0000-4000-8000-000000000003";
const OPT_IN_PROGRESS: &str = "0b710000-0000-4000-8000-000000000004";
const OPT_HOME: &str = "0b710000-0000-4000-8000-000000000005";
const OPT_ELSEWHERE: &str = "0b710000-0000-4000-8000-000000000006";

fn insert_v10_rows(conn: &Connection) {
    // (page, space, property, kind, text, number, checkbox, date, time, option)
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<f64>,
        Option<i64>,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    );
    let scalar = |page, space, property, kind, text, number, flag, date, time| -> Row {
        (
            page, space, property, kind, text, number, flag, date, time, None,
        )
    };
    let chosen = |page, space, property, kind, option| -> Row {
        (
            page,
            space,
            property,
            kind,
            None,
            None,
            None,
            None,
            None,
            Some(option),
        )
    };
    let rows: Vec<Row> = vec![
        // The Roadmap of the Work space, first.
        scalar(
            PAGE_ROADMAP,
            SPACE_WORK,
            PROP_SUMMARY,
            "text",
            Some("Ship it"),
            None,
            None,
            None,
            None,
        ),
        scalar(
            PAGE_ROADMAP,
            SPACE_WORK,
            PROP_COST,
            "number",
            None,
            Some(3.0),
            None,
            None,
            None,
        ),
        scalar(
            PAGE_ROADMAP,
            SPACE_WORK,
            PROP_LINK,
            "url",
            Some("https://example.com/roadmap"),
            None,
            None,
            None,
            None,
        ),
        // The trashed page keeps its rows.
        chosen(
            PAGE_OLD,
            SPACE_PERSONAL,
            PROP_TAGS,
            "multi_select",
            OPT_LATER,
        ),
        scalar(
            PAGE_OLD,
            SPACE_PERSONAL,
            PROP_SUMMARY,
            "text",
            Some("old"),
            None,
            None,
            None,
            None,
        ),
        // Notes: Tags, a text, a checkbox that is off, a day without a time, a select.
        chosen(
            PAGE_NOTES,
            SPACE_PERSONAL,
            PROP_TAGS,
            "multi_select",
            OPT_URGENT,
        ),
        scalar(
            PAGE_NOTES,
            SPACE_PERSONAL,
            PROP_SUMMARY,
            "text",
            Some("café notes"),
            None,
            None,
            None,
            None,
        ),
        scalar(
            PAGE_NOTES,
            SPACE_PERSONAL,
            PROP_DONE,
            "checkbox",
            None,
            None,
            Some(0),
            None,
            None,
        ),
        scalar(
            PAGE_NOTES,
            SPACE_PERSONAL,
            PROP_DUE,
            "date",
            None,
            None,
            None,
            Some("2026-10-09"),
            None,
        ),
        chosen(
            PAGE_NOTES,
            SPACE_PERSONAL,
            PROP_AREA,
            "select",
            OPT_ELSEWHERE,
        ),
        // Home: a value of most kinds, two tags (in the order the page lists them).
        chosen(
            PAGE_HOME,
            SPACE_PERSONAL,
            PROP_TAGS,
            "multi_select",
            OPT_WORK,
        ),
        chosen(
            PAGE_HOME,
            SPACE_PERSONAL,
            PROP_TAGS,
            "multi_select",
            OPT_URGENT,
        ),
        scalar(
            PAGE_HOME,
            SPACE_PERSONAL,
            PROP_SUMMARY,
            "text",
            Some("Weekly review ✓ — Café"),
            None,
            None,
            None,
            None,
        ),
        scalar(
            PAGE_HOME,
            SPACE_PERSONAL,
            PROP_COST,
            "number",
            None,
            Some(12.5),
            None,
            None,
            None,
        ),
        scalar(
            PAGE_HOME,
            SPACE_PERSONAL,
            PROP_DONE,
            "checkbox",
            None,
            None,
            Some(1),
            None,
            None,
        ),
        scalar(
            PAGE_HOME,
            SPACE_PERSONAL,
            PROP_DUE,
            "date",
            None,
            None,
            None,
            Some("2026-10-07"),
            Some("14:30"),
        ),
        chosen(
            PAGE_HOME,
            SPACE_PERSONAL,
            PROP_STAGE,
            "status",
            OPT_IN_PROGRESS,
        ),
        chosen(PAGE_HOME, SPACE_PERSONAL, PROP_AREA, "select", OPT_HOME),
    ];
    for (page, space, property, kind, text, number, flag, date, time, option) in rows {
        conn.execute(
            "INSERT INTO page_properties \
               (space_id, page_id, property_id, kind, value_text, value_number, value_bool, \
                value_date, value_time, option_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id(space).as_slice(),
                id(page).as_slice(),
                id(property).as_slice(),
                kind,
                text,
                number,
                flag,
                date,
                time,
                option.map(|option| id(option).to_vec())
            ],
        )
        .unwrap();
    }

    // The property rows of the text index: (page, property, text).
    for (page, property, body) in [
        (PAGE_ROADMAP, PROP_SUMMARY, "Ship it"),
        (PAGE_ROADMAP, PROP_LINK, "https://example.com/roadmap"),
        (PAGE_OLD, PROP_TAGS, "later"),
        (PAGE_OLD, PROP_SUMMARY, "old"),
        (PAGE_NOTES, PROP_TAGS, "urgent"),
        (PAGE_NOTES, PROP_SUMMARY, "café notes"),
        (PAGE_NOTES, PROP_AREA, "Elsewhere"),
        (PAGE_HOME, PROP_TAGS, "work, urgent"),
        (PAGE_HOME, PROP_SUMMARY, "Weekly review ✓ — Café"),
        (PAGE_HOME, PROP_STAGE, "In progress"),
        (PAGE_HOME, PROP_AREA, "Home"),
    ] {
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body, kind) \
             VALUES (?1, ?2, '', ?3, 1)",
            params![id(page).as_slice(), id(property).as_slice(), body],
        )
        .unwrap();
    }
}

// ---- version 11 -------------------------------------------------------------------
//
// The `files` and `checklist` kinds of page properties (ADR-0026, amendment of 2026-10-07), on top
// of the version 10 content: the table was rebuilt by the migration (two columns more), and the
// pages of the v3 content carry a files value and a checklist value. Files rows attach the
// attachments of the v5 content, so they are references next to the blocks that show them. The ids
// are UUID text again; `tests/files.rs` names the same rows.

const PROP_FILES: &str = "b09e0000-0000-4000-8000-000000000008";
const PROP_TODO: &str = "b09e0000-0000-4000-8000-000000000009";

fn insert_v11_rows(conn: &Connection) {
    // The files rows: (page, space, property, attachment), one per attachment, in the order of the
    // list, inserted out of page order. Home lists the report before the photo; the photo is also
    // listed by Notes, the empty file by the Roadmap and the sketch by the trashed page.
    for (page, space, property, attachment) in [
        (PAGE_ROADMAP, SPACE_WORK, PROP_FILES, ATT_EMPTY),
        (PAGE_OLD, SPACE_PERSONAL, PROP_FILES, ATT_SKETCH),
        (PAGE_HOME, SPACE_PERSONAL, PROP_FILES, ATT_REPORT),
        (PAGE_NOTES, SPACE_PERSONAL, PROP_FILES, ATT_PHOTO),
        (PAGE_HOME, SPACE_PERSONAL, PROP_FILES, ATT_PHOTO),
    ] {
        conn.execute(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, attachment_id) \
             VALUES (?1, ?2, ?3, 'files', ?4)",
            params![
                id(space).as_slice(),
                id(page).as_slice(),
                id(property).as_slice(),
                id(attachment).as_slice()
            ],
        )
        .unwrap();
    }

    // The checklist rows: (page, space, items done, items). Home is two of three, the Roadmap's
    // one item is done, the trashed page has two open items.
    for (page, space, done, total) in [
        (PAGE_ROADMAP, SPACE_WORK, 1.0_f64, 1_i64),
        (PAGE_HOME, SPACE_PERSONAL, 2.0, 3),
        (PAGE_OLD, SPACE_PERSONAL, 0.0, 2),
    ] {
        conn.execute(
            "INSERT INTO page_properties \
               (space_id, page_id, property_id, kind, value_number, value_total) \
             VALUES (?1, ?2, ?3, 'checklist', ?4, ?5)",
            params![
                id(space).as_slice(),
                id(page).as_slice(),
                id(PROP_TODO).as_slice(),
                done,
                total
            ],
        )
        .unwrap();
    }

    // The property rows of the text index: the file names of the manifests, the item texts.
    for (page, property, body) in [
        (PAGE_ROADMAP, PROP_FILES, "empty.txt"),
        (PAGE_ROADMAP, PROP_TODO, "Tag the release"),
        (PAGE_OLD, PROP_FILES, "Old sketch.jpg"),
        (PAGE_OLD, PROP_TODO, "Clean up, Archive"),
        (PAGE_NOTES, PROP_FILES, "Café photo.png"),
        (
            PAGE_HOME,
            PROP_FILES,
            "Q3 report \u{2014} final.pdf, Café photo.png",
        ),
        (
            PAGE_HOME,
            PROP_TODO,
            "Pack the charger, Call Ada, Book flights",
        ),
    ] {
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body, kind) \
             VALUES (?1, ?2, '', ?3, 1)",
            params![id(page).as_slice(), id(property).as_slice(), body],
        )
        .unwrap();
    }
}

// ---- version 12 -------------------------------------------------------------------
//
// The `place` kind of page properties (ADR-0026, amendment of 2026-10-08), on top of the version 11
// content: the table was rebuilt by the migration (a longer list of kinds, the same columns), and
// three pages carry a place whose name is the row's `value_text`. `tests/files.rs` names the same
// rows.

const PROP_PLACE: &str = "b09e0000-0000-4000-8000-00000000000a";

fn insert_v12_rows(conn: &Connection) {
    // (page, space, name), inserted out of page order.
    for (page, space, name) in [
        (PAGE_ROADMAP, SPACE_WORK, "Lisbon"),
        (PAGE_HOME, SPACE_PERSONAL, "Caf\u{e9} de Flore, Paris"),
        (PAGE_NOTES, SPACE_PERSONAL, "Home office"),
    ] {
        conn.execute(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, value_text) \
             VALUES (?1, ?2, ?3, 'place', ?4)",
            params![
                id(space).as_slice(),
                id(page).as_slice(),
                id(PROP_PLACE).as_slice(),
                name
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body, kind) \
             VALUES (?1, ?2, '', ?3, 1)",
            params![id(page).as_slice(), id(PROP_PLACE).as_slice(), name],
        )
        .unwrap();
    }
}

// ---- version 13 -------------------------------------------------------------------
//
// The `relation` kind of page properties (ADR-0027), on top of the version 12 content: the table was
// rebuilt by the migration (a longer list of kinds, the new column `related_page_id`). The forward
// property lists two pages on Home (Notes, and the trashed "Old ideas"), the back property lists Home
// on Notes. `tests/files.rs` names the same rows.

const PROP_RELATION: &str = "b09e0000-0000-4000-8000-00000000000b";
const PROP_RELATION_BACK: &str = "b09e0000-0000-4000-8000-00000000000c";

fn insert_v13_rows(conn: &Connection) {
    // (page, property, related pages in order, the text of the property's row in the text index)
    for (page, property, related, text) in [
        (PAGE_NOTES, PROP_RELATION_BACK, vec![PAGE_HOME], "Home"),
        (
            PAGE_HOME,
            PROP_RELATION,
            vec![PAGE_NOTES, PAGE_OLD],
            "Notes, Old ideas",
        ),
    ] {
        for target in related {
            conn.execute(
                "INSERT INTO page_properties (space_id, page_id, property_id, kind, related_page_id)                  VALUES (?1, ?2, ?3, 'relation', ?4)",
                params![
                    id(SPACE_PERSONAL).as_slice(),
                    id(page).as_slice(),
                    id(property).as_slice(),
                    id(target).as_slice()
                ],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO fts_blocks (page_id, block_id, title, body, kind)              VALUES (?1, ?2, '', ?3, 1)",
            params![id(page).as_slice(), id(property).as_slice(), text],
        )
        .unwrap();
    }
}

// ---- version 14 -------------------------------------------------------------------
//
// The image icons of pages (ADR-0029), on top of the version 13 content: the `icon_refs` table is new.
// Home has the photo as its icon, the trashed page "Old ideas" the sketch (a trashed page keeps its
// icon, so it still counts), the Roadmap the attachment that was purged (it has no index row: the icon
// still names it). The rows are inserted out of page order. `tests/files.rs` names the same rows.

fn insert_v14_rows(conn: &Connection) {
    // (page, space, attachment)
    for (page, space, attachment) in [
        (PAGE_ROADMAP, SPACE_WORK, ATT_PURGED),
        (PAGE_HOME, SPACE_PERSONAL, ATT_PHOTO),
        (PAGE_OLD, SPACE_PERSONAL, ATT_SKETCH),
    ] {
        conn.execute(
            "INSERT INTO icon_refs (page_id, space_id, attachment_id) VALUES (?1, ?2, ?3)",
            params![
                id(page).as_slice(),
                id(space).as_slice(),
                id(attachment).as_slice()
            ],
        )
        .unwrap();
    }
}

// ---- version 15 -------------------------------------------------------------------
//
// The semantic index and the MCP clients and audit (ADR-0037), on top of the version 14 content. The
// vectors are four-dimensional stand-ins (unit length, f32 little-endian; a real one has the model's
// dimension). `ai_dirty` is filled by the migration's triggers while the text index rows are inserted:
// every page with text starts dirty, and the two pages that are indexed here leave it again (the core
// removes a page once it is indexed). The token hashes are 32-byte stand-ins for a SHA-256. The audit
// rows have explicit ids. `tests/files.rs` names the same rows.

/// The embedding model of the stand-in index.
const AI_MODEL: &str = "nomic-embed-text";

/// An MCP client with a grant on the personal space (read only); a client id is 32 hex characters.
const MCP_CLIENT_DESKTOP: &str = "c11e0000000040008000000000000001";

/// An MCP client that was created and never granted anything or used.
const MCP_CLIENT_NEW: &str = "c11e0000000040008000000000000002";

/// A vector of unit length as the index stores it: f32 little-endian.
fn vector(values: [f32; 4]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn insert_v15_rows(conn: &Connection) {
    // (page, hash of the chunk hashes, number of chunks, indexed at)
    for (page, source, chunks, indexed_at) in [
        (PAGE_NOTES, 0x51_u8, 1_i64, 1_760_000_601_000_i64),
        (PAGE_HOME, 0x41, 2, 1_760_000_600_000),
    ] {
        conn.execute(
            "INSERT INTO ai_pages (page_id, source_hash, model, chunks, indexed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id(page).as_slice(),
                ramp(source, 32),
                AI_MODEL,
                chunks,
                indexed_at
            ],
        )
        .unwrap();
    }

    // (page, seq, blocks the chunk covers, hash of the embedded text, text, vector)
    type Chunk = (
        &'static str,
        i64,
        Vec<&'static str>,
        u8,
        &'static str,
        [f32; 4],
    );
    let chunks: [Chunk; 3] = [
        (
            PAGE_HOME,
            1,
            vec![BLOCK_HOME_PLAN],
            0x72,
            "The plan for the week",
            [0.0, 1.0, 0.0, 0.0],
        ),
        (
            PAGE_NOTES,
            0,
            vec![BLOCK_NOTES_MEETING, BLOCK_NOTES_BACK, BLOCK_NOTES_QUOTE],
            0x81,
            "Meeting notes, a link back and a quote",
            [0.0, 0.6, 0.8, 0.0],
        ),
        (
            PAGE_HOME,
            0,
            vec![BLOCK_HOME_WELCOME],
            0x71,
            "Welcome home \u{1F3E0} caf\u{e9}",
            [1.0, 0.0, 0.0, 0.0],
        ),
    ];
    for (page, seq, blocks, hash, text, values) in chunks {
        let block_ids: Vec<u8> = blocks.iter().flat_map(|block| id(block)).collect();
        conn.execute(
            "INSERT INTO ai_chunks (page_id, seq, block_ids, hash, text, vector) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id(page).as_slice(),
                seq,
                block_ids,
                ramp(hash, 32),
                text,
                vector(values)
            ],
        )
        .unwrap();
    }
    for page in [PAGE_HOME, PAGE_NOTES] {
        conn.execute(
            "DELETE FROM ai_dirty WHERE page_id = ?1",
            [id(page).as_slice()],
        )
        .unwrap();
    }

    // (id, name, token hash, grants, created at, last used at)
    for (client, name, token, grants, created_at, last_used_at) in [
        (
            MCP_CLIENT_NEW,
            "Scratch client",
            0xB1_u8,
            "[]".to_owned(),
            1_760_000_700_000_i64,
            None,
        ),
        (
            MCP_CLIENT_DESKTOP,
            "Desktop assistant",
            0xA1,
            format!(r#"[{{"spaceId":"{SPACE_PERSONAL}","read":true,"write":false}}]"#),
            1_760_000_650_000,
            Some(1_760_000_680_000_i64),
        ),
    ] {
        conn.execute(
            "INSERT INTO mcp_clients (id, name, token_hash, grants, created_at, last_used_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                client,
                name,
                ramp(token, 32),
                grants,
                created_at,
                last_used_at
            ],
        )
        .unwrap();
    }

    // (id, at, tool, space, page, summary, outcome)
    for (row, at, tool, space, page, summary, outcome) in [
        (
            3_i64,
            1_760_000_682_000_i64,
            "write_page",
            Some(SPACE_WORK),
            None,
            "Roadmap",
            "denied",
        ),
        (
            1,
            1_760_000_670_000,
            "search_pages",
            Some(SPACE_PERSONAL),
            None,
            "budget",
            "ok",
        ),
        (
            2,
            1_760_000_680_000,
            "read_page",
            Some(SPACE_PERSONAL),
            Some(PAGE_HOME),
            "Home",
            "ok",
        ),
    ] {
        conn.execute(
            "INSERT INTO mcp_audit (id, at, client_id, client_name, tool, space_id, page_id, \
             summary, outcome) VALUES (?1, ?2, ?3, 'Desktop assistant', ?4, ?5, ?6, ?7, ?8)",
            params![
                row,
                at,
                MCP_CLIENT_DESKTOP,
                tool,
                space.map(|space| id(space).to_vec()),
                page.map(|page| id(page).to_vec()),
                summary,
                outcome
            ],
        )
        .unwrap();
    }
}

// ---- version 16 -------------------------------------------------------------------
//
// The image covers of pages (ADR-0040), on top of the version 15 content: `icon_refs` has the pair
// (page, attachment) as its key and holds the attachments of covers too. Home has the report as its
// cover next to the photo as its icon (two rows for one page), Notes has the photo as its cover (the
// same attachment as the icon of another page). `tests/files.rs` names the same rows.

fn insert_v16_rows(conn: &Connection) {
    // (page, space, attachment)
    for (page, space, attachment) in [
        (PAGE_NOTES, SPACE_PERSONAL, ATT_PHOTO),
        (PAGE_HOME, SPACE_PERSONAL, ATT_REPORT),
    ] {
        conn.execute(
            "INSERT INTO icon_refs (page_id, space_id, attachment_id) VALUES (?1, ?2, ?3)",
            params![
                id(page).as_slice(),
                id(space).as_slice(),
                id(attachment).as_slice()
            ],
        )
        .unwrap();
    }
}

// ---- version 17 -------------------------------------------------------------------
//
// Shared spaces (M13, slice 1), on top of the version 16 content migrated by `017_shared_spaces.sql`
// (the rows of the four per-feed sync tables carry the personal feed, the account id, now). What a
// device that shares the Work space holds: the `sync_feeds` rows of the personal feed (the state the
// core moves out of `sync_config`: the cursor 42 and the chain hash of the settings) and of the
// shared feed of the Work space; the same sync bookkeeping in the shared feed (the keyring
// pseudo-document has a row in each feed, and the Roadmap has one in each); the device log of a
// co-member; three quarantined items of the shared feed and what the bound dropped; the end version
// vectors of three entries; the membership log of the shared feed (the third entry was appended here
// and is not accepted yet: cursor 0) and the cache of its fold. Every blob is an opaque stand-in with
// NUL and high bytes. The rows are inserted out of key order on purpose. `tests/files.rs` names the
// same rows and rebuilds the same stand-ins.

/// The feed of the shared Work space.
const SHARED_FEED: &str = "5ead0000-0000-4000-8000-000000000001";

/// The account of the co-member who writes in the shared space.
const CO_MEMBER: &str = "ac000000-0000-4000-8000-000000000002";

/// A device of that account.
const CO_MEMBER_DEVICE: &str = "de71ce00-0000-4000-8000-000000000003";

/// A third account whose items were refused.
const OUTSIDER: &str = "ac000000-0000-4000-8000-000000000003";

/// Opaque stand-in for a quarantined item (the envelope as received): `KEN1`, the cursor, NUL and
/// high bytes, a filler.
fn rejected_item(cursor: u8) -> Vec<u8> {
    let mut item = b"KEN1".to_vec();
    item.extend([cursor, 0x00, 0xFF, 0x80]);
    item.extend(ramp(0x30 + cursor, 30 + usize::from(cursor)));
    item
}

/// Opaque stand-in for an encoded membership entry: `KME1`, the sequence number, NUL and high
/// bytes, a filler whose length grows with the sequence number.
fn membership_entry(seq: u8) -> Vec<u8> {
    let mut entry = b"KME1".to_vec();
    entry.extend([seq, 0x00, 0xFF, 0x80]);
    entry.extend(ramp(0xD0 + seq, 40 + 8 * usize::from(seq)));
    entry
}

/// Opaque stand-in for the encoded state of the fold: `KMS1`, NUL and high bytes, a filler.
fn membership_state() -> Vec<u8> {
    let mut state = b"KMS1".to_vec();
    state.extend([0x00, 0xFF, 0x80]);
    state.extend(ramp(0x5A, 120));
    state
}

/// Opaque stand-in for an encoded version vector (a real one is `kb_doc::Version`).
fn end_vv(seed: u8) -> Vec<u8> {
    let mut vv = vec![0x02, 0x00, 0xFF];
    vv.extend(ramp(seed, 12));
    vv
}

fn insert_v17_rows(conn: &Connection) {
    // The feeds: the shared one first, the personal one second (the list puts it first).
    // (feed, kind, space, cursor, chain hash, max covers, chain history, fork, server epoch)
    type Feed = (
        [u8; 16],
        i64,
        Option<[u8; 16]>,
        i64,
        Vec<u8>,
        i64,
        Option<Vec<u8>>,
        i64,
        Vec<u8>,
    );
    let feeds: [Feed; 2] = [
        (
            id(SHARED_FEED),
            2,
            Some(id(SPACE_WORK)),
            17,
            ramp(0x30, 32),
            9,
            None,
            1,
            ramp(0x60, 16),
        ),
        (
            id(ACCOUNT_ID),
            1,
            None,
            42,
            ramp(0x70, 32),
            33,
            Some(ramp(0x44, 50)),
            0,
            ramp(0x60, 16),
        ),
    ];
    for (feed, kind, space, cursor, chain, covers, history, fork, epoch) in feeds {
        conn.execute(
            "INSERT INTO sync_feeds (feed, kind, space_id, cursor, chain_hash, max_covers, \
             chain_history, fork, server_epoch) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                feed.as_slice(),
                kind,
                space.map(|space| space.to_vec()),
                cursor,
                chain,
                covers,
                history,
                fork,
                epoch
            ],
        )
        .unwrap();
    }

    // The bookkeeping of the shared feed (the personal feed's rows are the v6 ones, migrated).
    // (document, pushed_up_to_seq, next_device_seq, snapshot_covers, entries_since_snapshot,
    //  bytes_since_snapshot, remote_snapshot_covers)
    type State = ([u8; 16], i64, i64, Option<i64>, i64, i64, Option<i64>);
    let states: [State; 3] = [
        (id(PAGE_ROADMAP), 1, 2, None, 1, 64, None),
        (KEYRING_DOC, 0, 2, None, 0, 0, None),
        (id(DOC_STRUCTURE_WORK), 3, 4, Some(11), 2, 300, Some(11)),
    ];
    for (doc, pushed, next, covers, entries, bytes, remote) in states {
        conn.execute(
            "INSERT INTO sync_doc_state (feed, doc_id, pushed_up_to_seq, next_device_seq, \
             snapshot_covers, entries_since_snapshot, bytes_since_snapshot, \
             remote_snapshot_covers) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id(SHARED_FEED).as_slice(),
                doc.as_slice(),
                pushed,
                next,
                covers,
                entries,
                bytes,
                remote
            ],
        )
        .unwrap();
    }
    for (doc, device, last_seq) in [
        (id(PAGE_ROADMAP), id(CO_MEMBER_DEVICE), 5_i64),
        (KEYRING_DOC, id(CO_MEMBER_DEVICE), 1),
        (id(PAGE_ROADMAP), id(DEVICE_ID), 2),
    ] {
        conn.execute(
            "INSERT INTO sync_seen (feed, doc_id, device_id, last_seq) VALUES (?1, ?2, ?3, ?4)",
            params![
                id(SHARED_FEED).as_slice(),
                doc.as_slice(),
                device.as_slice(),
                last_seq
            ],
        )
        .unwrap();
    }
    // (device, cursor, chain hash, signed at, stand-in, seen at)
    for (device, cursor, chain_hash, at_ms, signed, seen_at) in [
        (
            id(DEVICE_ID),
            17_i64,
            ramp(0x30, 32),
            1_760_000_800_000_i64,
            signed_head(1, 17),
            1_760_000_800_500_i64,
        ),
        (
            id(CO_MEMBER_DEVICE),
            16,
            ramp(0xA0, 32),
            1_760_000_790_000,
            signed_head(3, 16),
            1_760_000_801_000,
        ),
    ] {
        conn.execute(
            "INSERT INTO sync_heads (feed, device_id, cursor, chain_hash, at_ms, signed, seen_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id(SHARED_FEED).as_slice(),
                device.as_slice(),
                cursor,
                chain_hash,
                at_ms,
                signed,
                seen_at
            ],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO sync_pending (feed, cursor, doc_id, bytes, reason, received_at) \
         VALUES (?1, 15, ?2, ?3, 1, 1760000802000)",
        params![
            id(SHARED_FEED).as_slice(),
            id(PAGE_ROADMAP).as_slice(),
            pending_entry(15)
        ],
    )
    .unwrap();

    // The device log of the co-member (three entries) and of the third account (its genesis).
    // (account, seq, stand-in, fetched at)
    for (account, seq, fetched_at) in [
        (CO_MEMBER, 2_u8, 1_760_000_803_000_i64),
        (OUTSIDER, 0, 1_760_000_803_500),
        (CO_MEMBER, 0, 1_760_000_803_000),
        (CO_MEMBER, 1, 1_760_000_803_000),
    ] {
        conn.execute(
            "INSERT INTO foreign_device_logs (account_id, seq, entry, fetched_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                id(account).as_slice(),
                i64::from(seq),
                log_entry(seq),
                fetched_at
            ],
        )
        .unwrap();
    }

    // The quarantine: an item a role does not allow, a keyring item of a non-member (the all-zero
    // document id) and a snapshot that does not dominate, from two accounts; the bound dropped four
    // more items of the first account.
    // (cursor, document, account, device, reason, at)
    for (cursor, doc, account, device, reason, at) in [
        (
            16_u8,
            id(PAGE_ROADMAP),
            OUTSIDER,
            CO_MEMBER_DEVICE,
            3_i64,
            1_760_000_810_000_i64,
        ),
        (
            12,
            id(PAGE_ROADMAP),
            CO_MEMBER,
            CO_MEMBER_DEVICE,
            2,
            1_760_000_805_000,
        ),
        (
            14,
            KEYRING_DOC,
            CO_MEMBER,
            CO_MEMBER_DEVICE,
            1,
            1_760_000_806_000,
        ),
    ] {
        conn.execute(
            "INSERT INTO sync_rejected (feed, cursor, doc_id, account_id, device_id, reason, \
             bytes, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id(SHARED_FEED).as_slice(),
                i64::from(cursor),
                doc.as_slice(),
                id(account).as_slice(),
                id(device).as_slice(),
                reason,
                rejected_item(cursor),
                at
            ],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO sync_rejected_dropped (feed, account_id, dropped) VALUES (?1, ?2, 4)",
        params![id(SHARED_FEED).as_slice(), id(CO_MEMBER).as_slice()],
    )
    .unwrap();

    // The end version vectors of the last applied entry per (document, writer device).
    // (document, device, seed of the vector, cursor)
    for (doc, device, seed, cursor) in [
        (id(PAGE_ROADMAP), id(DEVICE_ID), 0x21_u8, 17_i64),
        (id(DOC_STRUCTURE_WORK), id(CO_MEMBER_DEVICE), 0x31, 13),
        (id(PAGE_ROADMAP), id(CO_MEMBER_DEVICE), 0x11, 16),
    ] {
        conn.execute(
            "INSERT INTO sync_entry_marks (doc_id, device_id, end_vv, cursor) \
             VALUES (?1, ?2, ?3, ?4)",
            params![doc.as_slice(), device.as_slice(), end_vv(seed), cursor],
        )
        .unwrap();
    }

    // The membership log of the shared feed: Create (found at cursor 3), a Join (cursor 9) and an
    // entry appended here that the server has not accepted (cursor 0), then the cache of the fold
    // up to the second.
    for (seq, cursor) in [(3_u8, 0_i64), (1, 3), (2, 9)] {
        conn.execute(
            "INSERT INTO membership_entries (feed, seq, cursor, bytes) VALUES (?1, ?2, ?3, ?4)",
            params![
                id(SHARED_FEED).as_slice(),
                i64::from(seq),
                cursor,
                membership_entry(seq)
            ],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO membership_state (feed, seq, head_hash, state) VALUES (?1, 2, ?2, ?3)",
        params![
            id(SHARED_FEED).as_slice(),
            ramp(0xE0, 32),
            membership_state()
        ],
    )
    .unwrap();
}

// ---- version 18 -------------------------------------------------------------------
//
// Comments, person mentions and the Person property (M13 slice 2, ADR-0048 and ADR-0049), on top of the
// version 17 content migrated by `018_comments_people.sql`: the device that shares the Work space and
// has comments. The records are opaque stand-ins (this crate never interprets them): the Roadmap's
// stream holds a root comment by the co-member (a Commenter, found at cursor 12), a reply by the owner
// (cursor 13), an edit of the root (cursor 14, `rev` 1), a resolve of the thread (cursor 15, by the
// owner), a delete of the reply by the owner (cursor 16) and a second thread's root this device made
// and has not pushed yet (cursor NULL). Home (a personal page) has one comment of its own. The words
// of three live comments are indexed (the second thread's, the root's latest body, and Home's); the
// reply is deleted, so it has no row. The inbox has one notification of each kind, one of them read.
// `tests/files.rs` names the same rows.

const COMMENT_ROOT: &str = "c0de0000-0000-4000-8000-000000000001";
const COMMENT_REPLY: &str = "c0de0000-0000-4000-8000-000000000002";
const COMMENT_SECOND: &str = "c0de0000-0000-4000-8000-000000000003";
const COMMENT_HOME: &str = "c0de0000-0000-4000-8000-000000000004";
const RECORD_ROOT_POST: &str = "ec0d0000-0000-4000-8000-000000000001";
const RECORD_REPLY_POST: &str = "ec0d0000-0000-4000-8000-000000000002";
const RECORD_ROOT_EDIT: &str = "ec0d0000-0000-4000-8000-000000000003";
const RECORD_ROOT_RESOLVE: &str = "ec0d0000-0000-4000-8000-000000000004";
const RECORD_REPLY_DELETE: &str = "ec0d0000-0000-4000-8000-000000000005";
const RECORD_SECOND_POST: &str = "ec0d0000-0000-4000-8000-000000000006";
const RECORD_HOME_POST: &str = "ec0d0000-0000-4000-8000-000000000007";

/// The Person property of the Roadmap page.
const PROP_OWNER: &str = "b09e0000-0000-4000-8000-00000000000d";

/// Opaque stand-in for an encoded signed comment record: `KCR1`, the number, NUL and high bytes, a
/// filler whose length grows with the number.
fn comment_record(n: u8) -> Vec<u8> {
    let mut record = b"KCR1".to_vec();
    record.extend([n, 0x00, 0xFF, 0x80]);
    record.extend(ramp(0x90 + n, 60 + 4 * usize::from(n)));
    record
}

fn insert_v18_rows(conn: &Connection) {
    // The records, in an order that is not the order of `at` (the query orders them).
    // (record, page, space, thread, comment, op, author, device, rev, at, cursor, author role)
    type Record = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        i64,
        &'static str,
        &'static str,
        i64,
        i64,
        Option<i64>,
        i64,
        u8,
    );
    let records: [Record; 7] = [
        (
            RECORD_ROOT_EDIT,
            PAGE_ROADMAP,
            SPACE_WORK,
            COMMENT_ROOT,
            COMMENT_ROOT,
            2,
            CO_MEMBER,
            CO_MEMBER_DEVICE,
            1,
            1_760_000_830_000,
            Some(14),
            2,
            3,
        ),
        (
            RECORD_ROOT_POST,
            PAGE_ROADMAP,
            SPACE_WORK,
            COMMENT_ROOT,
            COMMENT_ROOT,
            1,
            CO_MEMBER,
            CO_MEMBER_DEVICE,
            0,
            1_760_000_810_000,
            Some(12),
            2,
            1,
        ),
        (
            RECORD_REPLY_DELETE,
            PAGE_ROADMAP,
            SPACE_WORK,
            COMMENT_ROOT,
            COMMENT_REPLY,
            3,
            ACCOUNT_ID,
            DEVICE_ID,
            1,
            1_760_000_860_000,
            Some(16),
            4,
            5,
        ),
        (
            RECORD_REPLY_POST,
            PAGE_ROADMAP,
            SPACE_WORK,
            COMMENT_ROOT,
            COMMENT_REPLY,
            1,
            ACCOUNT_ID,
            DEVICE_ID,
            0,
            1_760_000_820_000,
            Some(13),
            4,
            2,
        ),
        (
            RECORD_ROOT_RESOLVE,
            PAGE_ROADMAP,
            SPACE_WORK,
            COMMENT_ROOT,
            COMMENT_ROOT,
            4,
            ACCOUNT_ID,
            DEVICE_ID,
            1,
            1_760_000_840_000,
            Some(15),
            4,
            4,
        ),
        (
            RECORD_SECOND_POST,
            PAGE_ROADMAP,
            SPACE_WORK,
            COMMENT_SECOND,
            COMMENT_SECOND,
            1,
            ACCOUNT_ID,
            DEVICE_ID,
            0,
            1_760_000_870_000,
            None,
            4,
            6,
        ),
        (
            RECORD_HOME_POST,
            PAGE_HOME,
            SPACE_PERSONAL,
            COMMENT_HOME,
            COMMENT_HOME,
            1,
            ACCOUNT_ID,
            DEVICE_ID,
            0,
            1_760_000_700_000,
            None,
            4,
            7,
        ),
    ];
    for (record, page, space, thread, comment, op, author, device, rev, at, cursor, role, n) in
        records
    {
        conn.execute(
            "INSERT INTO comments (record_id, space_id, page_id, thread_id, comment_id, op, author, \
             device, rev, at, cursor, author_role, record) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id(record).as_slice(),
                id(space).as_slice(),
                id(page).as_slice(),
                id(thread).as_slice(),
                id(comment).as_slice(),
                op,
                id(author).as_slice(),
                id(device).as_slice(),
                rev,
                at,
                cursor,
                role,
                comment_record(n)
            ],
        )
        .unwrap();
    }

    // The words of the comments that count and are not deleted: (space, page, comment, text).
    for (space, page, comment, text) in [
        (
            SPACE_WORK,
            PAGE_ROADMAP,
            COMMENT_SECOND,
            "Can we ship earlier? Caf\u{e9} launch moves too",
        ),
        (
            SPACE_WORK,
            PAGE_ROADMAP,
            COMMENT_ROOT,
            "The ship date looks tight (edited)",
        ),
        (
            SPACE_PERSONAL,
            PAGE_HOME,
            COMMENT_HOME,
            "Remember to review the plan",
        ),
    ] {
        conn.execute(
            "INSERT INTO comment_fts (body, page_id, comment_id, space_id) VALUES (?1, ?2, ?3, ?4)",
            params![
                text,
                id(page).as_slice(),
                id(comment).as_slice(),
                id(space).as_slice()
            ],
        )
        .unwrap();
    }

    // The inbox: (kind, space, page, block, comment, actor, at, read), oldest first.
    type Notification = (
        i64,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        &'static str,
        i64,
        i64,
    );
    let notifications: [Notification; 4] = [
        (
            1,
            SPACE_WORK,
            Some(PAGE_ROADMAP),
            Some(BLOCK_ROADMAP_SHIP),
            Some(COMMENT_REPLY),
            CO_MEMBER,
            1_760_000_820_500,
            1,
        ),
        (
            2,
            SPACE_WORK,
            Some(PAGE_ROADMAP),
            None,
            Some(COMMENT_ROOT),
            CO_MEMBER,
            1_760_000_831_000,
            0,
        ),
        (
            3,
            SPACE_WORK,
            Some(PAGE_ROADMAP),
            Some(BLOCK_ROADMAP_CAFE),
            None,
            CO_MEMBER,
            1_760_000_850_000,
            0,
        ),
        (
            4,
            SPACE_WORK,
            Some(PAGE_ROADMAP),
            None,
            None,
            CO_MEMBER,
            1_760_000_851_000,
            0,
        ),
    ];
    for (kind, space, page, block, comment, actor, at, read) in notifications {
        conn.execute(
            "INSERT INTO notifications (kind, space_id, page_id, block_id, comment_id, actor, at, read) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                kind,
                id(space).as_slice(),
                page.map(|page| id(page).to_vec()),
                block.map(|block| id(block).to_vec()),
                comment.map(|comment| id(comment).to_vec()),
                id(actor).as_slice(),
                at,
                read
            ],
        )
        .unwrap();
    }

    // The derived person mentions: the Roadmap names the co-member twice (two blocks) and the owner
    // once; the document id is the page id (ADR-0016).
    for (page, block, account) in [
        (PAGE_ROADMAP, BLOCK_ROADMAP_SHIP, CO_MEMBER),
        (PAGE_ROADMAP, BLOCK_ROADMAP_CAFE, ACCOUNT_ID),
        (PAGE_ROADMAP, BLOCK_ROADMAP_CAFE, CO_MEMBER),
    ] {
        conn.execute(
            "INSERT INTO person_mentions (doc_id, block_id, account_id) VALUES (?1, ?2, ?3)",
            params![
                id(page).as_slice(),
                id(block).as_slice(),
                id(account).as_slice()
            ],
        )
        .unwrap();
    }

    // The Person value of the Roadmap, in order: the owner, then the co-member.
    for account in [ACCOUNT_ID, CO_MEMBER] {
        conn.execute(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, account_id) \
             VALUES (?1, ?2, ?3, 'person', ?4)",
            params![
                id(SPACE_WORK).as_slice(),
                id(PAGE_ROADMAP).as_slice(),
                id(PROP_OWNER).as_slice(),
                id(account).as_slice()
            ],
        )
        .unwrap();
    }

    // The Roadmap's date mention reminds the co-member only.
    conn.execute(
        "UPDATE date_mentions SET reminder_for = ?1 WHERE node_id = ?2",
        params![
            id(CO_MEMBER).as_slice(),
            id(MENTION_ROADMAP_SHIP).as_slice()
        ],
    )
    .unwrap();
}
