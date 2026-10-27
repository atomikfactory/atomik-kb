//! `import_run` without a dialog: the source readers (files, a folder, a `.zip`), the decoding of
//! text, the upload of the files the plan needs and the commit loop of ADR-0036, driven against a
//! scripted core (`common::Fake`) and temporary folders. What the real core makes of the pages is
//! covered in `kb-core` and in `import_export_core.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atomik_kb_desktop_lib::import_source::ImportSource;
use atomik_kb_desktop_lib::importer::{self, ImportRunOptions, ImportSourceKind};
use kb_types::{
    ApiError, ErrorCode, Event, ImportReport, ImportWarning, ImportWarningCode, PageId, SpaceId,
};

use common::{Fake, NoRemote, host, names_in, record_events, write_zip, zip_bytes};

const SPACE: [u8; 16] = [1; 16];

fn options(source: ImportSourceKind) -> ImportRunOptions {
    ImportRunOptions::page(SpaceId::from_bytes(SPACE), None, source)
}

/// Writes `bytes` at `relative` under `dir`, making the folders.
fn put(dir: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

fn paths(files: &[(String, String)]) -> Vec<&str> {
    files.iter().map(|(path, _)| path.as_str()).collect()
}

fn warning(report: &ImportReport, code: ImportWarningCode) -> Vec<&ImportWarning> {
    report
        .warnings
        .iter()
        .filter(|warning| warning.code == code)
        .collect()
}

fn run_folder(
    state: &std::sync::Arc<atomik_kb_desktop_lib::AppState<Fake>>,
    folder: &Path,
) -> Result<ImportReport, ApiError> {
    let source = ImportSource::from_folder(folder)?;
    importer::run(
        host(state),
        &options(ImportSourceKind::Folder),
        source,
        &NoRemote,
    )
}

// --- a folder ---

#[test]
fn a_folder_is_imported_and_only_the_files_the_pages_need_are_uploaded() {
    let (state, world) = Fake::ready();
    let events = record_events(&state);
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("Vault");
    put(&vault, "Home.md", b"# Home\n![logo](assets/logo.png)\n");
    put(&vault, "Projects/Alpha.md", b"alpha");
    put(&vault, "Projects/data.csv", b"Name\nA\n");
    put(&vault, "assets/logo.png", b"\x89PNG-bytes");
    put(&vault, "assets/big.bin", b"not needed");
    // Skipped without a word.
    put(&vault, ".git/config", b"x");
    put(&vault, ".obsidian/workspace.md", b"x");
    put(&vault, "Notes/.hidden.md", b"x");
    put(&vault, "__MACOSX/._Home.md", b"x");
    put(&vault, "node_modules/pkg/readme.md", b"x");
    world.lock().unwrap().needed = vec!["assets/logo.png".to_owned()];

    let report = run_folder(&state, &vault).unwrap();

    let world = world.lock().unwrap();
    assert_eq!(
        world.began,
        [(SpaceId::from_bytes(SPACE), None, Some("Vault".to_owned()))]
    );
    assert_eq!(
        paths(&world.files),
        ["Home.md", "Projects/Alpha.md", "Projects/data.csv"]
    );
    assert_eq!(world.files[0].1, "# Home\n![logo](assets/logo.png)\n");
    // Every other file is a binary path, hidden entries are not.
    assert_eq!(world.binary_paths, ["assets/big.bin", "assets/logo.png"]);
    // The needed file went up as an attachment, named after the file and typed by its extension,
    // and was reported to the session; the file that is not needed was never opened.
    let logo = world.stored.values().next().unwrap();
    assert_eq!(world.stored.len(), 1);
    assert_eq!(
        (logo.name.as_str(), logo.mime.as_str()),
        ("logo.png", "image/png")
    );
    assert_eq!(logo.space_id, Some(SpaceId::from_bytes(SPACE)));
    assert_eq!(logo.bytes, b"\x89PNG-bytes");
    assert_eq!(
        world
            .reported
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["assets/logo.png"]
    );
    assert_eq!(
        world.log,
        [
            "ImportBegin",
            "ImportAddFile",
            "ImportAddFile",
            "ImportAddFile",
            "ImportPlan",
            "AttachmentBegin",
            "write:10",
            "AttachmentFinish",
            "ImportAddAttachment",
            "ImportCommit",
        ]
    );
    assert_eq!(world.commits, [50]);
    assert!(world.aborted.is_empty());
    assert_eq!(report.pages, 3);
    assert_eq!(report.attachments, 1);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.root_page_ids, [PageId::from_bytes([5; 16])]);
    // The progress event reached the listener.
    assert!(events.lock().unwrap().iter().any(|event| matches!(
        event,
        Event::ImportProgress {
            done: 3,
            total: 3,
            ..
        }
    )));
}

#[test]
fn pages_are_committed_fifty_at_a_time_with_an_event_after_every_call() {
    let (state, world) = Fake::ready();
    let events = record_events(&state);
    let root = tempfile::tempdir().unwrap();
    for index in 0..120 {
        put(root.path(), &format!("page-{index:03}.md"), b"x");
    }

    let report = run_folder(&state, root.path()).unwrap();

    assert_eq!(report.pages, 120);
    assert_eq!(world.lock().unwrap().commits, [50, 50, 50]);
    let progress: Vec<(u32, u32)> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            Event::ImportProgress { done, total, .. } => Some((*done, *total)),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [(50, 120), (100, 120), (120, 120)]);
}

#[test]
fn a_folder_that_is_not_there_is_invalid_input_for_the_path() {
    let root = tempfile::tempdir().unwrap();
    let error = ImportSource::from_folder(&root.path().join("missing"))
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "path");
}

#[test]
fn text_files_past_the_limit_are_left_out_with_one_warning() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    for index in 0..10_010 {
        put(root.path(), &format!("d{}/p{index}.md", index % 50), b"x");
    }
    let report = run_folder(&state, root.path()).unwrap();
    assert_eq!(world.lock().unwrap().files.len(), 10_000);
    let truncated = warning(&report, ImportWarningCode::Truncated);
    assert_eq!(truncated.len(), 1);
}

// --- decoding ---

#[test]
fn text_is_decoded_with_its_byte_order_mark_and_odd_files_are_reported() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    let mut bom8 = vec![0xEF, 0xBB, 0xBF];
    bom8.extend_from_slice("héllo\r\nwörld".as_bytes());
    put(root.path(), "bom8.md", &bom8);
    let mut le = vec![0xFF, 0xFE];
    let mut be = vec![0xFE, 0xFF];
    for unit in "Grüße 😀".encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
        be.extend_from_slice(&unit.to_be_bytes());
    }
    put(root.path(), "le.txt", &le);
    put(root.path(), "be.md", &be);
    put(root.path(), "latin1.md", b"caf\xE9");
    put(root.path(), "plain.md", "日本語".as_bytes());
    // Exactly the limit is fine, one byte more is not.
    put(root.path(), "limit.md", &vec![b'a'; 16 * 1024 * 1024]);
    put(root.path(), "over.md", &vec![b'a'; 16 * 1024 * 1024 + 1]);

    let report = run_folder(&state, root.path()).unwrap();

    let world = world.lock().unwrap();
    let text = |name: &str| {
        world
            .files
            .iter()
            .find(|(path, _)| path == name)
            .map(|(_, text)| text.as_str())
    };
    assert_eq!(text("bom8.md"), Some("héllo\r\nwörld"));
    assert_eq!(text("le.txt"), Some("Grüße 😀"));
    assert_eq!(text("be.md"), Some("Grüße 😀"));
    assert_eq!(text("latin1.md"), Some("caf\u{FFFD}"));
    assert_eq!(text("plain.md"), Some("日本語"));
    assert_eq!(text("limit.md").map(str::len), Some(16 * 1024 * 1024));
    assert_eq!(text("over.md"), None);
    let not_utf8 = warning(&report, ImportWarningCode::NotUtf8);
    assert_eq!(not_utf8.len(), 1);
    assert_eq!(not_utf8[0].path, "latin1.md");
    let too_large = warning(&report, ImportWarningCode::TooLarge);
    assert_eq!(too_large.len(), 1);
    assert_eq!(too_large[0].path, "over.md");
}

#[test]
fn the_shells_warnings_come_after_the_cores() {
    let (state, world) = Fake::ready();
    world.lock().unwrap().core_warnings = vec![ImportWarning {
        path: "Home.md".to_owned(),
        code: ImportWarningCode::UnresolvedLink,
        detail: Some("Other.md".to_owned()),
    }];
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "Home.md", b"x");
    put(root.path(), "latin1.md", b"caf\xE9");
    let report = run_folder(&state, root.path()).unwrap();
    let codes: Vec<ImportWarningCode> = report.warnings.iter().map(|w| w.code).collect();
    assert_eq!(
        codes,
        [
            ImportWarningCode::UnresolvedLink,
            ImportWarningCode::NotUtf8
        ]
    );
}

// --- picked files ---

#[test]
fn picked_files_import_from_their_folder_and_their_neighbours_are_binary_paths() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    let a = put(root.path(), "a.md", b"a");
    let b = put(root.path(), "b.txt", b"b");
    put(root.path(), "other.md", b"not picked");
    put(root.path(), "pic.png", b"png");
    put(root.path(), "sub/ok.png", b"png");
    put(root.path(), "sub/x/y/z.png", b"png");
    // Five levels down is too deep.
    put(root.path(), "sub/x/y/z/deep.png", b"png");
    put(root.path(), ".hidden/h.png", b"png");
    world.lock().unwrap().needed = vec!["pic.png".to_owned(), "sub/ok.png".to_owned()];

    let source = ImportSource::from_files(&[b, a]).unwrap();
    assert_eq!(source.name(), None);
    let report = importer::run(
        host(&state),
        &options(ImportSourceKind::Files),
        source,
        &NoRemote,
    )
    .unwrap();

    let world = world.lock().unwrap();
    assert_eq!(world.began[0].2, None);
    assert_eq!(paths(&world.files), ["a.md", "b.txt"]);
    assert_eq!(
        world.binary_paths,
        ["other.md", "pic.png", "sub/ok.png", "sub/x/y/z.png"]
    );
    assert_eq!(world.stored.len(), 2);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn a_picked_file_that_is_not_a_text_type_is_left_out_with_a_warning() {
    let root = tempfile::tempdir().unwrap();
    let doc = put(root.path(), "report.docx", b"x");
    let md = put(root.path(), "a.md", b"a");
    let mut source = ImportSource::from_files(&[doc, md]).unwrap();
    assert_eq!(source.text_files().len(), 1);
    let warnings = source.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, ImportWarningCode::UnsupportedFile);
    assert_eq!(warnings[0].path, "report.docx");
}

#[test]
fn no_picked_files_is_a_cancelled_choice() {
    let error = ImportSource::from_files(&[]).err().unwrap();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "path");
}

// --- a zip ---

#[test]
fn a_zip_is_read_in_place_unsafe_entries_are_refused_and_nested_zips_merged() {
    let (state, world) = Fake::ready();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let nested = zip_bytes(&[
        ("Part/Other.md", b"other page"),
        ("Part/pic.png", b"nested png"),
        ("../nested-evil.md", b"evil"),
        ("Deep.zip", b"PK-but-not-opened-again"),
    ]);
    let zip_path = dir.path().join("Export-abc.zip");
    write_zip(
        &zip_path,
        &[
            ("Export/Page.md", b"# Page\n"),
            ("Export/img.png", b"outer png"),
            ("Export/", b""),
            // The slip: none of these may reach the core.
            ("../evil.md", b"evil"),
            ("/etc/passwd.md", b"evil"),
            ("Export/../../x.md", b"evil"),
            ("C:\\win.md", b"evil"),
            ("a\\b.md", b"evil"),
            ("Export//double.md", b"evil"),
            // Quietly skipped.
            ("__MACOSX/._Page.md", b"x"),
            (".DS_Store", b"x"),
            ("Export/.git/HEAD", b"x"),
            // Only a zip at the root is opened.
            ("docs/archive.zip", b"PK-attachment"),
            ("Part-1.zip", &nested),
            ("Broken.zip", b"this is not a zip"),
        ],
    );
    world.lock().unwrap().needed = vec!["Export/img.png".to_owned(), "Part/pic.png".to_owned()];

    let source = ImportSource::from_zip(&zip_path, &cache).unwrap();
    assert_eq!(source.name(), Some("Export-abc"));
    // The nested zip was unpacked into a temporary file under the cache folder.
    assert_eq!(names_in(&cache.join("import")).len(), 1);
    let report = importer::run(
        host(&state),
        &options(ImportSourceKind::Zip),
        source,
        &NoRemote,
    )
    .unwrap();

    let world = world.lock().unwrap();
    assert_eq!(world.began[0].2.as_deref(), Some("Export-abc"));
    assert_eq!(paths(&world.files), ["Export/Page.md", "Part/Other.md"]);
    assert_eq!(
        world.binary_paths,
        [
            "Broken.zip",
            "Deep.zip",
            "Export/img.png",
            "Part/pic.png",
            "docs/archive.zip"
        ]
    );
    // Files read out of the archive, and out of the nested one, were uploaded.
    let mut uploaded: Vec<(&str, &[u8])> = world
        .stored
        .values()
        .map(|blob| (blob.name.as_str(), blob.bytes.as_slice()))
        .collect();
    uploaded.sort();
    assert_eq!(
        uploaded,
        [
            ("img.png", &b"outer png"[..]),
            ("pic.png", &b"nested png"[..])
        ]
    );
    // Seven entries were refused, each with a warning naming it, none of them with its content.
    let unsupported = warning(&report, ImportWarningCode::UnsupportedFile);
    assert_eq!(unsupported.len(), 7, "{unsupported:?}");
    // The temporary file of the nested zip is gone, with the source.
    assert!(names_in(&cache.join("import")).is_empty());
}

#[test]
fn a_zip_that_is_not_a_zip_is_invalid_input_for_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let not_zip = dir.path().join("x.zip");
    fs::write(&not_zip, b"definitely not a zip file").unwrap();
    let error = ImportSource::from_zip(&not_zip, dir.path()).err().unwrap();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "path");

    let missing = ImportSource::from_zip(&dir.path().join("none.zip"), dir.path())
        .err()
        .unwrap();
    assert_eq!(missing.code, ErrorCode::InvalidInput);
}

#[test]
fn the_temporary_files_of_a_nested_zip_are_removed_when_the_import_fails() {
    let (state, world) = Fake::ready();
    world.lock().unwrap().plan_failure = Some(ApiError::invalid("importId", "unknown session"));
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let zip_path = dir.path().join("Export.zip");
    let nested = zip_bytes(&[("a.md", b"a")]);
    write_zip(&zip_path, &[("Part-1.zip", &nested)]);

    let source = ImportSource::from_zip(&zip_path, &cache).unwrap();
    assert_eq!(names_in(&cache.join("import")).len(), 1);
    let error = importer::run(
        host(&state),
        &options(ImportSourceKind::Zip),
        source,
        &NoRemote,
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert!(names_in(&cache.join("import")).is_empty());
}

#[test]
fn leftover_temporary_files_are_cleared_at_start() {
    let cache = tempfile::tempdir().unwrap();
    put(&cache.path().join("import"), "part-1.zip", b"x");
    atomik_kb_desktop_lib::import_source::clear_import_dir(cache.path());
    assert!(!cache.path().join("import").exists());
    // Nothing to clear is fine.
    atomik_kb_desktop_lib::import_source::clear_import_dir(cache.path());
}

// --- failures ---

#[test]
fn a_failure_aborts_the_session_and_is_the_callers_error() {
    let (state, world) = Fake::ready();
    world.lock().unwrap().plan_failure = Some(ApiError::storage("the database failed"));
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a.md", b"a");

    let error = run_folder(&state, root.path()).unwrap_err();

    assert_eq!(error.code, ErrorCode::Storage);
    assert_eq!(world.lock().unwrap().aborted, [7]);
}

#[test]
fn a_failing_commit_aborts_the_session() {
    let (state, world) = Fake::ready();
    world.lock().unwrap().fail_commit_on = Some((2, ApiError::storage("the database failed")));
    let root = tempfile::tempdir().unwrap();
    for index in 0..120 {
        put(root.path(), &format!("p{index}.md"), b"x");
    }
    let error = run_folder(&state, root.path()).unwrap_err();
    assert_eq!(error.code, ErrorCode::Storage);
    let world = world.lock().unwrap();
    assert_eq!(world.commits.len(), 2);
    assert_eq!(world.aborted, [7]);
}

#[test]
fn a_core_that_stops_making_progress_is_an_error_not_a_loop() {
    let (state, world) = Fake::ready();
    world.lock().unwrap().stall_commits = true;
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a.md", b"a");

    let error = run_folder(&state, root.path()).unwrap_err();

    assert_eq!(error.code, ErrorCode::Internal);
    let world = world.lock().unwrap();
    // Three answers that bring no page in a row, after the first.
    assert_eq!(world.commits.len(), 4);
    assert_eq!(world.aborted, [7]);
}

#[test]
fn a_needed_file_that_cannot_be_uploaded_is_a_warning_and_is_not_reported() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a.md", b"a");
    put(root.path(), "ok.png", b"fine");
    put(root.path(), "fail.png", b"the core refuses this one");
    {
        let mut world = world.lock().unwrap();
        world.needed = vec![
            "fail.png".to_owned(),
            "gone.png".to_owned(),
            "ok.png".to_owned(),
        ];
        world.fail_upload_named = Some("fail.png".to_owned());
    }

    let report = run_folder(&state, root.path()).unwrap();

    let world = world.lock().unwrap();
    // The refused upload was aborted; the file the source does not have never started one.
    assert!(world.uploads.is_empty());
    assert_eq!(world.stored.len(), 1);
    assert_eq!(
        world
            .reported
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["ok.png"]
    );
    let failed = warning(&report, ImportWarningCode::AttachmentFailed);
    let mut failed: Vec<&str> = failed.iter().map(|warning| warning.path.as_str()).collect();
    failed.sort_unstable();
    assert_eq!(failed, ["fail.png", "gone.png"]);
    assert_eq!(report.pages, 1);
}

#[test]
fn a_locked_core_ends_the_import_even_during_an_upload() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a.md", b"a");
    put(root.path(), "pic.png", b"png");
    {
        let mut world = world.lock().unwrap();
        world.needed = vec!["pic.png".to_owned()];
        // The app locks while the file goes up.
        world.locked_error = true;
    }

    let error = run_folder(&state, root.path()).unwrap_err();

    // Locked is not a failed file (a warning) but the end of everything.
    assert_eq!(error.code, ErrorCode::Locked);
    let world = world.lock().unwrap();
    assert!(world.commits.is_empty());
    assert_eq!(world.aborted, [7]);
    assert!(world.uploads.is_empty(), "the upload was aborted");
}

#[test]
fn the_destination_page_is_passed_to_the_core() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a.md", b"a");
    let parent = PageId::from_bytes([8; 16]);
    let source = ImportSource::from_folder(root.path()).unwrap();
    let mut with_parent = options(ImportSourceKind::Folder);
    with_parent.parent_id = Some(parent);
    importer::run(host(&state), &with_parent, source, &NoRemote).unwrap();
    assert_eq!(world.lock().unwrap().began[0].1, Some(parent));
}

#[test]
fn nothing_about_the_source_reaches_the_log() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("Secret vault");
    put(
        &vault,
        "Secret page.md",
        b"secret text [x](Secret%20plans.pdf)",
    );
    put(&vault, "Secret plans.pdf", b"secret bytes");
    put(&vault, "latin1 secret.md", b"caf\xE9");
    world.lock().unwrap().needed = vec!["Secret plans.pdf".to_owned()];
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        run_folder(&state, &vault).unwrap();
    });
    let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("import finished"), "{logs}");
    for secret in ["Secret", "secret", "plans", "vault", ".pdf", "latin1"] {
        assert!(!logs.contains(secret), "{secret} leaked: {logs}");
    }
}
