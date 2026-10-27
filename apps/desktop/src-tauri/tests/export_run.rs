//! `export_run` without a dialog: the archive writer and the single-file writer of ADR-0036 driven
//! against a scripted core (`common::Fake`) and a temporary folder. What the real core hands out
//! is covered in `kb-core` and in `import_export_core.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::PathBuf;

use atomik_kb_desktop_lib::exporter::{self, ExportRunOptions};
use kb_types::{
    ApiError, AttachmentId, ErrorCode, Event, ExportBatch, ExportContent, ExportEntry,
    ExportFormat, ExportPlan, ExportWarning, ExportWarningCode, SpaceId,
};

use common::{Fake, Shared, host, names_in, read_zip, record_events, store_attachment, zip_names};

/// 2026-10-12 09:30:15 UTC.
const STAMP: i64 = 1_791_797_415;

fn options() -> ExportRunOptions {
    ExportRunOptions {
        space_id: SpaceId::from_bytes([1; 16]),
        page_id: None,
        include_children: true,
        format: ExportFormat::Markdown,
    }
}

fn plan(single_file: bool, name: &str) -> ExportPlan {
    ExportPlan {
        export_id: 1,
        pages: 2,
        attachments: 1,
        single_file,
        suggested_name: name.to_owned(),
    }
}

fn text(path: &str, text: &str) -> ExportEntry {
    ExportEntry {
        path: path.to_owned(),
        content: ExportContent::Text {
            text: text.to_owned(),
        },
    }
}

fn attachment(path: &str, id: AttachmentId, size: u64) -> ExportEntry {
    ExportEntry {
        path: path.to_owned(),
        content: ExportContent::Attachment {
            attachment_id: id,
            size,
        },
    }
}

fn batch(entries: Vec<ExportEntry>, done: bool) -> ExportBatch {
    ExportBatch {
        entries,
        done,
        warnings: Vec::new(),
    }
}

/// Scripts the core: this plan, then these batches.
fn script(world: &Shared, plan: ExportPlan, batches: Vec<ExportBatch>) {
    let mut world = world.lock().unwrap();
    world.plan = Some(plan);
    world.batches = batches.into();
}

fn unreadable(id: AttachmentId) -> ExportWarning {
    ExportWarning {
        code: ExportWarningCode::AttachmentUnreadable,
        page_id: None,
        attachment_id: Some(id),
    }
}

fn run(
    state: &std::sync::Arc<atomik_kb_desktop_lib::AppState<Fake>>,
    target: &std::path::Path,
    piece_bytes: u64,
) -> Result<exporter::ExportRunResult, ApiError> {
    exporter::run_with(
        host(state),
        &options(),
        |_| Ok(target.to_path_buf()),
        piece_bytes,
        STAMP,
    )
}

#[test]
fn a_multi_file_export_is_a_zip_that_reads_back() {
    let (state, world) = Fake::ready();
    let events = record_events(&state);
    let id = store_attachment(&world, 9, "diagram.png", b"0123456789");
    script(
        &world,
        plan(false, "Space.zip"),
        vec![
            batch(
                vec![
                    text("Notes.md", "# Notes\n\nHello\n"),
                    text("Notes/Whiteboard.excalidraw", "{\"type\":\"excalidraw\"}"),
                    text("日本語 ページ.md", "ünï\n"),
                ],
                false,
            ),
            ExportBatch {
                entries: vec![attachment("Notes/diagram.png", id, 10)],
                done: false,
                warnings: vec![ExportWarning {
                    code: ExportWarningCode::AttachmentNotDownloaded,
                    page_id: None,
                    attachment_id: Some(AttachmentId::from_bytes([4; 16])),
                }],
            },
            batch(vec![text("atomik-export.json", "{\"format\":1}")], true),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");

    let result = run(&state, &target, 4).unwrap();

    assert_eq!(result.path, target.to_string_lossy());
    assert_eq!((result.pages, result.attachments), (2, 1));
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(
        result.warnings[0].code,
        ExportWarningCode::AttachmentNotDownloaded
    );
    // Only the archive is left: the temporary file was renamed into place.
    assert_eq!(names_in(dir.path()), ["Space.zip"]);

    let entries = read_zip(&target);
    assert_eq!(
        entries.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "Notes.md",
            "Notes/Whiteboard.excalidraw",
            "Notes/diagram.png",
            "atomik-export.json",
            "日本語 ページ.md",
        ]
    );
    assert_eq!(entries["Notes.md"].bytes, b"# Notes\n\nHello\n");
    assert_eq!(entries["日本語 ページ.md"].bytes, "ünï\n".as_bytes());
    assert_eq!(entries["Notes/diagram.png"].bytes, b"0123456789");
    // Text is deflated, the attachment stored; every entry carries the export time.
    assert_eq!(entries["Notes.md"].method, zip::CompressionMethod::Deflated);
    assert_eq!(
        entries["Notes/diagram.png"].method,
        zip::CompressionMethod::Stored
    );
    for (name, entry) in &entries {
        let modified = entry.modified.unwrap();
        assert_eq!(
            (modified.year(), modified.month(), modified.day()),
            (2026, 10, 12),
            "{name}"
        );
        assert_eq!((modified.hour(), modified.minute()), (9, 30), "{name}");
    }
    // The attachment came from the core one range at a time, in order, never whole.
    let reads = world.lock().unwrap().reads.clone();
    assert_eq!(
        reads,
        vec![(id, Some((0, 4))), (id, Some((4, 8))), (id, Some((8, 10)))]
    );
    // The session was ended; every batch's events reached the listener.
    assert_eq!(world.lock().unwrap().ended, [1]);
    let progress = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| matches!(event, Event::ExportProgress { .. }))
        .count();
    assert_eq!(progress, 3);
}

#[test]
fn text_entries_are_utf8_with_a_flag_other_tools_read() {
    let (state, world) = Fake::ready();
    script(
        &world,
        plan(false, "x.zip"),
        vec![batch(vec![text("Žluťoučký kůň/Příliš.md", "x")], true)],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("x.zip");
    run(&state, &target, 4).unwrap();
    assert_eq!(zip_names(&target), ["Žluťoučký kůň/Příliš.md"]);
}

#[test]
fn a_single_page_is_one_plain_file() {
    let (state, world) = Fake::ready();
    script(
        &world,
        plan(true, "Weekly review.md"),
        vec![ExportBatch {
            entries: vec![text(
                "Weekly review.md",
                "---\ntitle: \"Weekly review\"\n---\n\nHi\n",
            )],
            done: true,
            warnings: vec![ExportWarning {
                code: ExportWarningCode::AttachmentMissing,
                page_id: None,
                attachment_id: Some(AttachmentId::from_bytes([3; 16])),
            }],
        }],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Weekly review.md");
    fs::write(&target, "an older export").unwrap();

    let result = run(&state, &target, 4).unwrap();

    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "---\ntitle: \"Weekly review\"\n---\n\nHi\n"
    );
    assert_eq!(names_in(dir.path()), ["Weekly review.md"]);
    assert_eq!(result.attachments, 0);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(world.lock().unwrap().ended, [1]);
}

#[test]
fn a_single_file_plan_with_more_than_one_entry_writes_nothing() {
    let (state, world) = Fake::ready();
    script(
        &world,
        plan(true, "Page.md"),
        vec![batch(
            vec![text("Page.md", "a"), text("Other.md", "b")],
            true,
        )],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Page.md");
    let error = run(&state, &target, 4).unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(names_in(dir.path()).is_empty());
    assert_eq!(world.lock().unwrap().ended, [1]);
}

#[test]
fn a_cancelled_dialog_ends_the_session_and_asks_for_nothing_else() {
    let (state, world) = Fake::ready();
    script(&world, plan(false, "Space.zip"), vec![batch(vec![], true)]);
    let error = exporter::run_with(
        host(&state),
        &options(),
        |_| Err(ApiError::invalid("path", "no file was chosen")),
        4,
        STAMP,
    )
    .unwrap_err();
    assert_eq!(error, ApiError::invalid("path", "no file was chosen"));
    let world = world.lock().unwrap();
    assert_eq!(world.log, ["ExportBegin", "ExportEnd"]);
    assert_eq!(world.ended, [1]);
}

#[test]
fn the_dialog_is_given_the_plan() {
    let (state, world) = Fake::ready();
    script(&world, plan(false, "Space.zip"), vec![batch(vec![], true)]);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("empty.zip");
    let mut seen = None;
    exporter::run_with(
        host(&state),
        &options(),
        |plan| {
            seen = Some(plan.clone());
            Ok(target.clone())
        },
        4,
        STAMP,
    )
    .unwrap();
    let seen = seen.unwrap();
    assert_eq!(seen.suggested_name, "Space.zip");
    assert!(!seen.single_file);
    // An export of nothing is still a valid, empty archive.
    assert!(read_zip(&target).is_empty());
}

#[test]
fn a_failure_in_the_middle_removes_the_temporary_file_and_keeps_the_old_archive() {
    let (state, world) = Fake::ready();
    script(
        &world,
        plan(false, "Space.zip"),
        vec![
            batch(vec![text("a.md", "a")], false),
            batch(vec![text("b.md", "b")], true),
        ],
    );
    world.lock().unwrap().fail_next_on = Some((2, ApiError::not_found("the page is gone")));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");
    fs::write(&target, "the earlier export").unwrap();

    let error = run(&state, &target, 4).unwrap_err();

    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(fs::read_to_string(&target).unwrap(), "the earlier export");
    assert_eq!(
        names_in(dir.path()),
        ["Space.zip"],
        "no temporary file is left"
    );
    assert_eq!(world.lock().unwrap().ended, [1]);
}

#[test]
fn a_locked_core_stops_the_export_and_leaves_nothing() {
    let (state, world) = Fake::ready();
    let id = store_attachment(&world, 9, "a.bin", b"0123456789");
    script(
        &world,
        plan(false, "Space.zip"),
        vec![batch(vec![attachment("a.bin", id, 10)], true)],
    );
    world
        .lock()
        .unwrap()
        .fail_read
        .insert(id, (2, ApiError::locked()));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");
    let error = run(&state, &target, 4).unwrap_err();
    assert_eq!(error.code, ErrorCode::Locked);
    assert!(names_in(dir.path()).is_empty());
}

#[test]
fn an_attachment_that_cannot_be_read_at_all_is_left_out_with_a_warning() {
    let (state, world) = Fake::ready();
    let good = store_attachment(&world, 8, "good.bin", b"abc");
    let bad = store_attachment(&world, 9, "bad.bin", b"0123456789");
    script(
        &world,
        plan(false, "Space.zip"),
        vec![batch(
            vec![
                text("p.md", "p"),
                attachment("p/bad.bin", bad, 10),
                attachment("p/good.bin", good, 3),
            ],
            true,
        )],
    );
    world
        .lock()
        .unwrap()
        .fail_read
        .insert(bad, (1, ApiError::storage("a chunk is missing")));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");

    let result = run(&state, &target, 4).unwrap();

    assert_eq!(result.attachments, 1);
    assert_eq!(result.warnings, vec![unreadable(bad)]);
    assert_eq!(zip_names(&target), ["p.md", "p/good.bin"]);
    assert_eq!(read_zip(&target)["p/good.bin"].bytes, b"abc");
}

#[test]
fn an_attachment_that_fails_half_way_is_taken_out_of_the_archive_again() {
    let (state, world) = Fake::ready();
    let good = store_attachment(&world, 8, "good.bin", b"abcdef");
    let bad = store_attachment(&world, 9, "bad.bin", b"0123456789");
    script(
        &world,
        plan(false, "Space.zip"),
        vec![batch(
            vec![
                text("before.md", "before"),
                attachment("bad.bin", bad, 10),
                attachment("good.bin", good, 6),
                text("after.md", "after"),
            ],
            true,
        )],
    );
    // The first piece (4 bytes) arrives, the second fails.
    world
        .lock()
        .unwrap()
        .fail_read
        .insert(bad, (2, ApiError::crypto("a chunk is damaged")));
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");

    let result = run(&state, &target, 4).unwrap();

    assert_eq!(result.attachments, 1);
    assert_eq!(result.warnings, vec![unreadable(bad)]);
    // `read_zip` reads every entry to the end, so a damaged archive would panic here.
    let entries = read_zip(&target);
    assert_eq!(
        entries.keys().map(String::as_str).collect::<Vec<_>>(),
        ["after.md", "before.md", "good.bin"]
    );
    assert_eq!(entries["good.bin"].bytes, b"abcdef");
    assert_eq!(entries["before.md"].bytes, b"before");
    assert_eq!(entries["after.md"].bytes, b"after");
}

#[test]
fn an_attachment_shorter_than_announced_is_unreadable_not_a_short_entry() {
    let (state, world) = Fake::ready();
    // The core says 10 bytes; the store holds 6.
    let id = store_attachment(&world, 9, "short.bin", b"012345");
    script(
        &world,
        plan(false, "Space.zip"),
        vec![batch(vec![attachment("short.bin", id, 10)], true)],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");
    let result = run(&state, &target, 4).unwrap();
    assert_eq!(result.attachments, 0);
    assert_eq!(result.warnings, vec![unreadable(id)]);
    assert!(zip_names(&target).is_empty());
}

#[test]
fn an_empty_attachment_is_an_empty_entry_and_is_never_read() {
    let (state, world) = Fake::ready();
    let id = store_attachment(&world, 9, "empty.txt", b"");
    script(
        &world,
        plan(false, "Space.zip"),
        vec![batch(vec![attachment("empty.txt", id, 0)], true)],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");
    let result = run(&state, &target, 4).unwrap();
    assert_eq!(result.attachments, 1);
    assert_eq!(read_zip(&target)["empty.txt"].bytes, b"");
    assert!(world.lock().unwrap().reads.is_empty());
}

#[test]
fn an_attachment_of_exactly_one_piece_is_one_read() {
    let (state, world) = Fake::ready();
    let id = store_attachment(&world, 9, "four.bin", b"abcd");
    script(
        &world,
        plan(false, "Space.zip"),
        vec![batch(vec![attachment("four.bin", id, 4)], true)],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");
    run(&state, &target, 4).unwrap();
    assert_eq!(world.lock().unwrap().reads, vec![(id, Some((0, 4)))]);
    assert_eq!(read_zip(&target)["four.bin"].bytes, b"abcd");
}

#[test]
fn an_entry_path_that_could_leave_the_archive_is_refused() {
    for path in [
        "../evil.md",
        "/abs.md",
        "a/../../b.md",
        "C:\\x.md",
        "a\\b.md",
        "",
        "a//b.md",
    ] {
        let (state, world) = Fake::ready();
        script(
            &world,
            plan(false, "Space.zip"),
            vec![batch(vec![text("fine.md", "x"), text(path, "y")], true)],
        );
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Space.zip");
        let error = run(&state, &target, 4).unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal, "{path:?}");
        assert!(names_in(dir.path()).is_empty(), "{path:?}");
        assert_eq!(world.lock().unwrap().ended, [1], "{path:?}");
    }
}

#[test]
fn a_core_that_never_finishes_is_an_error_not_a_loop() {
    let (state, world) = Fake::ready();
    script(
        &world,
        plan(false, "Space.zip"),
        (0..10).map(|_| batch(vec![], false)).collect(),
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Space.zip");
    let error = run(&state, &target, 4).unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(names_in(dir.path()).is_empty());
    // Three empty answers and no more.
    assert_eq!(world.lock().unwrap().next_calls, 3);
}

#[test]
fn a_directory_is_not_a_place_to_write_an_export() {
    for single_file in [false, true] {
        let (state, world) = Fake::ready();
        script(
            &world,
            plan(single_file, "x"),
            vec![batch(vec![text("a.md", "a")], true)],
        );
        let dir = tempfile::tempdir().unwrap();
        let error = run(&state, dir.path(), 4).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput, "single {single_file}");
        assert_eq!(error.details.unwrap()["field"], "path");
        assert_eq!(world.lock().unwrap().ended, [1]);
    }
}

#[test]
fn a_path_that_cannot_be_created_is_invalid_input_for_the_path() {
    let (state, world) = Fake::ready();
    script(&world, plan(false, "x.zip"), vec![batch(vec![], true)]);
    let dir = tempfile::tempdir().unwrap();
    let target: PathBuf = dir.path().join("no-such-folder").join("x.zip");
    let error = run(&state, &target, 4).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "path");
}

#[test]
fn an_archive_past_the_old_zip_limits_is_written_with_zip64_headers() {
    // More entries than a classic zip can count (65,535).
    const ENTRIES: usize = 66_000;
    let (state, world) = Fake::ready();
    let batches: Vec<ExportBatch> = (0..ENTRIES)
        .collect::<Vec<_>>()
        .chunks(500)
        .map(|chunk| {
            batch(
                chunk
                    .iter()
                    .map(|index| text(&format!("p{index}.md"), "x"))
                    .collect(),
                false,
            )
        })
        .chain(std::iter::once(batch(vec![], true)))
        .collect();
    script(&world, plan(false, "Big.zip"), batches);
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Big.zip");

    run(&state, &target, 4).unwrap();

    let archive = zip::ZipArchive::new(fs::File::open(&target).unwrap()).unwrap();
    assert_eq!(archive.len(), ENTRIES);
}

#[test]
fn nothing_about_the_export_reaches_the_log() {
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

    // A subscriber that is interested in everything, so that no thread caches a call site as
    // disabled before the capturing one below is installed.
    let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    let (state, world) = Fake::ready();
    let id = store_attachment(&world, 9, "Secret plans.pdf", b"0123456789");
    script(
        &world,
        plan(false, "Secret space.zip"),
        vec![batch(
            vec![
                text("Secret page.md", "secret text"),
                attachment("Secret page/Secret plans.pdf", id, 10),
            ],
            true,
        )],
    );
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Secret space.zip");
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        run(&state, &target, 4).unwrap();
    });
    let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("export written"), "{logs}");
    for secret in ["Secret", "secret", "plans", "space", ".zip", "tmp"] {
        assert!(!logs.contains(secret), "{secret} leaked: {logs}");
    }
}
