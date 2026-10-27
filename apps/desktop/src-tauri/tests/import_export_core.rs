//! Import and export end to end (ADR-0036): the shell's writers and readers against the **real
//! core** (`kb_core::Core` over a plain SQLite file and an in-memory chunk store, deterministic
//! clock and RNG), with no dialog: a folder imported, the space exported as a `.zip`, the `.zip`
//! imported into another space, single-file exports, an export cancelled at the dialog, and a
//! scheduled backup. What each side does alone is covered by the scripted-core tests
//! (`export_run.rs`, `import_run.rs`, `scheduled_backup.rs`) and in `kb-core` itself; these tests
//! are the ones that need the core's `ExportBegin`/`ImportBegin` sessions to be real.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use atomik_kb_desktop_lib::backup_scheduler::{Targets, run_now};
use atomik_kb_desktop_lib::exporter::{self, ExportRunOptions};
use atomik_kb_desktop_lib::import_source::ImportSource;
use atomik_kb_desktop_lib::importer::{self, ImportRunOptions, ImportSourceKind};
use atomik_kb_desktop_lib::{AppState, CoreHost};
use kb_core::{Core, CoreDeps};
use kb_keys::{DbKey, MemoryKeystore};
use kb_storage::{DbOpener, Encryption, MemoryBlobStore, OpenedDb, StorageError};
use kb_types::interop::{BACKUP_FOLDER_SETTING, BACKUP_LAST_SETTING};
use kb_types::testing::{FixedClock, SeededRng};
use kb_types::{
    ApiError, AttachmentInfo, BackupRunStatus, Clock, ErrorCode, ExportFormat, ImportReport,
    ImportWarningCode, InstallId, Json, PageId, PageNode, Request, Response, SpaceId,
};
use zip::ZipArchive;

use common::{NoRemote, names_in, read_zip};

const NOW: i64 = 1_791_797_400_000;

/// A plain SQLite file: the tests need no SQLCipher key.
struct PlainFileOpener(PathBuf);

impl DbOpener for PlainFileOpener {
    fn open(&self, _key: &DbKey) -> Result<OpenedDb, StorageError> {
        let conn = kb_storage::rusqlite::Connection::open(&self.0)?;
        Ok(OpenedDb {
            conn,
            encryption: Encryption::None,
        })
    }
}

/// A shared clock the test can move.
struct TestClock(Arc<AtomicI64>);

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A workspace with two spaces over the real core.
struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState<Core>>,
    a: SpaceId,
    b: SpaceId,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let deps = CoreDeps {
            install_id: InstallId::from_random_bytes([7; 16]),
            platform: "windows".into(),
            data_dir: dir.path().display().to_string(),
            keystore: Box::new(MemoryKeystore::new()),
            opener: Box::new(PlainFileOpener(dir.path().join("workspace.db"))),
            clock: Box::new(FixedClock::new(NOW)),
            rng: Box::new(SeededRng::new(1)),
            lock_params: kb_crypto::Argon2Params {
                memory_kib: 8 * 1024,
                iterations: 1,
                parallelism: 1,
            },
            blob_store: Box::new(MemoryBlobStore::new()),
        };
        let core = Core::open(deps).unwrap();
        let state = Arc::new(AppState::<Core>::new());
        state.set_ready(core);
        let fixture = Self {
            _dir: dir,
            state,
            a: SpaceId::from_bytes([0; 16]),
            b: SpaceId::from_bytes([0; 16]),
        };
        fixture.send(Request::WorkspaceCreate {
            name: "Home".into(),
            first_space_name: Some("Space A".into()),
        });
        let b = match fixture.send(Request::SpaceCreate {
            name: "Space B".into(),
            icon: None,
        }) {
            Response::Space(space) => space.id,
            other => panic!("unexpected {other:?}"),
        };
        let a = match fixture.send(Request::SpaceList) {
            Response::Spaces(spaces) => spaces[0].id,
            other => panic!("unexpected {other:?}"),
        };
        Self { a, b, ..fixture }
    }

    fn host(&self) -> &dyn CoreHost {
        &*self.state
    }

    fn send(&self, request: Request) -> Response {
        self.state.dispatch(request).unwrap()
    }

    fn try_send(&self, request: Request) -> Result<Response, ApiError> {
        self.state.dispatch(request)
    }

    fn tree(&self, space_id: SpaceId) -> Vec<PageNode> {
        match self.send(Request::TreeGet { space_id }) {
            Response::Tree(tree) => tree.roots,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// The titles of the tree, depth first, indented by depth.
    fn titles(&self, space_id: SpaceId) -> Vec<String> {
        fn walk(nodes: &[PageNode], depth: usize, out: &mut Vec<String>) {
            for node in nodes {
                out.push(format!("{}{}", "  ".repeat(depth), node.page.title));
                walk(&node.children, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.tree(space_id), 0, &mut out);
        out
    }

    fn attachments(&self, space_id: SpaceId) -> Vec<AttachmentInfo> {
        match self.send(Request::AttachmentList { space_id }) {
            Response::Attachments(list) => list,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn attachment_bytes(&self, info: &AttachmentInfo) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.state
            .attachment_read(info.attachment_id, None, &mut |piece: &[u8]| {
                bytes.extend_from_slice(piece);
                Ok(())
            })
            .unwrap();
        bytes
    }

    fn import(
        &self,
        space_id: SpaceId,
        parent_id: Option<PageId>,
        source: ImportSource,
        kind: ImportSourceKind,
    ) -> ImportReport {
        let options = ImportRunOptions::page(space_id, parent_id, kind);
        importer::run(self.host(), &options, source, &NoRemote).unwrap()
    }

    fn export(
        &self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        format: ExportFormat,
        target: &Path,
    ) -> exporter::ExportRunResult {
        let options = ExportRunOptions {
            space_id,
            page_id,
            include_children: true,
            format,
        };
        exporter::run(self.host(), &options, |_| Ok(target.to_path_buf())).unwrap()
    }
}

fn put(dir: &Path, relative: &str, bytes: &[u8]) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// 3 KiB of bytes that are not text and not a real image.
fn logo_bytes() -> Vec<u8> {
    (0..3_072_u32)
        .map(|n| (n.wrapping_mul(31) % 251) as u8)
        .collect()
}

/// An Obsidian-style vault: front matter, tags, a wiki link, a relative link, an image with a
/// title, a GitHub alert and a task list.
fn write_vault(dir: &Path) {
    put(
        dir,
        "Home.md",
        b"---\ntitle: Home page\ntags: [start, demo]\n---\n# Home page\n\n\
Welcome to [[Projects Alpha]]. The logo: ![logo](assets/logo.png \"The logo\")\n\n\
> [!NOTE]\n> Remember the milk.\n\n- [x] done\n- [ ] todo\n",
    );
    put(
        dir,
        "Projects Alpha.md",
        b"# Projects Alpha\n\nBack to [Home](Home.md).\n",
    );
    put(dir, "assets/logo.png", &logo_bytes());
    put(dir, ".obsidian/workspace.json", b"{}");
}

fn titles_without_indent(titles: &[String]) -> Vec<String> {
    let mut plain: Vec<String> = titles.iter().map(|t| t.trim().to_owned()).collect();
    plain.sort();
    plain
}

/// The clipboard Markdown of every page of a space by title, with attachment ids masked (they
/// differ between spaces).
fn markdown_by_title(fixture: &Fixture, space_id: SpaceId) -> BTreeMap<String, String> {
    fn collect(nodes: &[PageNode], out: &mut Vec<(String, PageId)>) {
        for node in nodes {
            out.push((node.page.title.clone(), node.page.id));
            collect(&node.children, out);
        }
    }
    let mut pages = Vec::new();
    collect(&fixture.tree(space_id), &mut pages);
    pages
        .into_iter()
        .map(|(title, page_id)| {
            let markdown = match fixture.send(Request::PageExportMarkdown { page_id }) {
                Response::Markdown(text) => text,
                other => panic!("unexpected {other:?}"),
            };
            (title, mask_attachment_ids(&markdown))
        })
        .collect()
}

fn mask_attachment_ids(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("attachment://") {
        let (before, after) = rest.split_at(at + "attachment://".len());
        out.push_str(before);
        out.push('X');
        let end = after
            .find(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

// --- the round trip ---

#[test]
fn a_vault_survives_import_export_and_import_of_the_archive() {
    let fixture = Fixture::new();
    let work = tempfile::tempdir().unwrap();
    let vault = work.path().join("Vault");
    write_vault(&vault);

    // 1. Import the folder into space A.
    let source = ImportSource::from_folder(&vault).unwrap();
    assert_eq!(source.name(), Some("Vault"));
    let report = fixture.import(fixture.a, None, source, ImportSourceKind::Folder);
    assert!(report.pages >= 2, "{report:?}");
    assert_eq!(report.attachments, 1, "{report:?}");
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.code == ImportWarningCode::AttachmentFailed),
        "{:?}",
        report.warnings
    );
    let titles = fixture.titles(fixture.a);
    let plain = titles_without_indent(&titles);
    assert!(plain.contains(&"Home page".to_owned()), "{titles:?}");
    assert!(plain.contains(&"Projects Alpha".to_owned()), "{titles:?}");
    let attachments = fixture.attachments(fixture.a);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "logo.png");
    assert_eq!(fixture.attachment_bytes(&attachments[0]), logo_bytes());

    // 2. Export the whole space as Markdown.
    let archive = work.path().join("Space A.zip");
    let exported = fixture.export(fixture.a, None, ExportFormat::Markdown, &archive);
    assert_eq!(exported.attachments, 1);
    assert!(exported.warnings.is_empty(), "{:?}", exported.warnings);
    assert_eq!(names_in(work.path()), ["Space A.zip", "Vault"]);
    let entries = read_zip(&archive);
    let names: Vec<&str> = entries.keys().map(String::as_str).collect();
    assert!(names.contains(&"atomik-export.json"), "{names:?}");
    let png: Vec<&String> = entries.keys().filter(|n| n.ends_with("logo.png")).collect();
    assert_eq!(png.len(), 1, "{names:?}");
    assert_eq!(entries[png[0]].bytes, logo_bytes());
    let home = entries
        .iter()
        .find(|(name, _)| name.ends_with("Home page.md"))
        .map(|(_, entry)| String::from_utf8(entry.bytes.clone()).unwrap())
        .unwrap_or_else(|| panic!("no Home page.md in {names:?}"));
    assert!(home.starts_with("---\n"), "{home}");
    assert!(home.contains("title: \"Home page\""), "{home}");
    assert!(home.contains("logo.png"), "{home}");
    assert!(!home.contains("attachment://"), "{home}");
    // Every path is one the shell would also accept when reading it back.
    for name in entries.keys() {
        assert!(
            !name.contains('\\') && !name.starts_with('/') && !name.contains(".."),
            "{name}"
        );
    }

    // 3. Import the archive into space B.
    let cache = work.path().join("cache");
    let source = ImportSource::from_zip(&archive, &cache).unwrap();
    assert_eq!(source.name(), Some("Space A"));
    let report = fixture.import(fixture.b, None, source, ImportSourceKind::Zip);
    assert_eq!(report.attachments, 1, "{report:?}");
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.code == ImportWarningCode::AttachmentFailed),
        "{:?}",
        report.warnings
    );

    // The pages came back, with the same text and the picture's bytes.
    let a = markdown_by_title(&fixture, fixture.a);
    let b = markdown_by_title(&fixture, fixture.b);
    for title in ["Home page", "Projects Alpha"] {
        assert!(b.contains_key(title), "{:?}", b.keys().collect::<Vec<_>>());
        assert_eq!(a[title], b[title], "{title}");
    }
    let attachments = fixture.attachments(fixture.b);
    assert_eq!(attachments.len(), 1);
    assert_eq!(fixture.attachment_bytes(&attachments[0]), logo_bytes());
    // Nothing temporary is left in the cache folder.
    assert!(names_in(&cache.join("import")).is_empty());
}

#[test]
fn a_page_without_attachments_exports_as_one_file_in_either_format() {
    let fixture = Fixture::new();
    let work = tempfile::tempdir().unwrap();
    let notes = work.path().join("notes");
    put(
        &notes,
        "Weekly review.md",
        b"# Weekly review\n\nA *plain* page.\n",
    );
    let source = ImportSource::from_folder(&notes).unwrap();
    fixture.import(fixture.a, None, source, ImportSourceKind::Folder);
    let tree = fixture.tree(fixture.a);
    let page = find(&tree, "Weekly review").expect("the page was imported");

    for (format, suggested, start) in [
        (ExportFormat::Markdown, "Weekly review.md", "---\n"),
        (ExportFormat::Html, "Weekly review.html", "<!doctype html>"),
    ] {
        let target = work.path().join(suggested);
        let mut offered = None;
        let options = ExportRunOptions {
            space_id: fixture.a,
            page_id: Some(page),
            include_children: false,
            format,
        };
        let result = exporter::run(fixture.host(), &options, |plan| {
            offered = Some((plan.single_file, plan.suggested_name.clone()));
            Ok(target.clone())
        })
        .unwrap();
        assert_eq!(offered, Some((true, suggested.to_owned())));
        assert_eq!((result.pages, result.attachments), (1, 0));
        let text = fs::read_to_string(&target).unwrap();
        assert!(text.starts_with(start), "{text}");
        assert!(text.contains("Weekly review"), "{text}");
    }
}

fn find(nodes: &[PageNode], title: &str) -> Option<PageId> {
    for node in nodes {
        if node.page.title == title {
            return Some(node.page.id);
        }
        if let Some(found) = find(&node.children, title) {
            return Some(found);
        }
    }
    None
}

#[test]
fn an_import_goes_under_the_page_it_was_asked_to() {
    let fixture = Fixture::new();
    let work = tempfile::tempdir().unwrap();
    let parent = match fixture.send(Request::PageCreate {
        space_id: fixture.a,
        parent_id: None,
        title: "Inbox".into(),
        index: None,
    }) {
        Response::Page(page) => page.id,
        other => panic!("unexpected {other:?}"),
    };
    put(work.path(), "Imported.md", b"# Imported\n\nText\n");
    let files = [work.path().join("Imported.md")];
    let source = ImportSource::from_files(&files).unwrap();
    let report = fixture.import(fixture.a, Some(parent), source, ImportSourceKind::Files);
    assert_eq!(report.pages, 1);
    let tree = fixture.tree(fixture.a);
    let inbox = tree.iter().find(|node| node.page.id == parent).unwrap();
    let children: Vec<&str> = inbox
        .children
        .iter()
        .map(|c| c.page.title.as_str())
        .collect();
    assert_eq!(children, ["Imported"]);
}

#[test]
fn a_cancelled_export_dialog_drops_the_core_session() {
    let fixture = Fixture::new();
    let work = tempfile::tempdir().unwrap();
    put(work.path(), "a.md", b"# A\n");
    let source = ImportSource::from_folder(work.path()).unwrap();
    fixture.import(fixture.a, None, source, ImportSourceKind::Folder);

    let options = ExportRunOptions {
        space_id: fixture.a,
        page_id: None,
        include_children: true,
        format: ExportFormat::Markdown,
    };
    let mut export_id = None;
    let error = exporter::run(fixture.host(), &options, |plan| {
        export_id = Some(plan.export_id);
        Err(ApiError::invalid("path", "no file was chosen"))
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);

    // The session is gone: asking for its entries is `NotFound`.
    let next = fixture.try_send(Request::ExportNext {
        export_id: export_id.unwrap(),
        max_entries: 10,
    });
    assert_eq!(next.unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn a_zip_with_a_traversal_entry_imports_without_it() {
    let fixture = Fixture::new();
    let work = tempfile::tempdir().unwrap();
    let zip_path = work.path().join("Evil.zip");
    common::write_zip(
        &zip_path,
        &[
            ("Good/Page.md", b"# Page\n\nfine\n"),
            ("../escaped.md", b"# Escaped\n"),
            ("/absolute.md", b"# Absolute\n"),
        ],
    );
    let source = ImportSource::from_zip(&zip_path, work.path()).unwrap();
    let report = fixture.import(fixture.a, None, source, ImportSourceKind::Zip);
    let titles = titles_without_indent(&fixture.titles(fixture.a));
    assert!(titles.contains(&"Page".to_owned()), "{titles:?}");
    assert!(!titles.contains(&"Escaped".to_owned()), "{titles:?}");
    assert!(!titles.contains(&"Absolute".to_owned()), "{titles:?}");
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|w| w.code == ImportWarningCode::UnsupportedFile)
            .count(),
        2
    );
    // Nothing was written outside the temporary work folder: only the zip is there.
    assert_eq!(names_in(work.path()), ["Evil.zip"]);
}

// --- scheduled backups with the real core ---

#[test]
fn a_scheduled_backup_needs_the_recovery_kit_and_says_so_when_it_is_missing() {
    let fixture = Fixture::new();
    let folder = tempfile::tempdir().unwrap();
    // Written the way the shell writes it, not through the webview's guarded `dispatch` command.
    atomik_kb_desktop_lib::backup_scheduler::choose_folder(fixture.host(), || {
        Ok(folder.path().to_path_buf())
    })
    .unwrap();
    let clock = TestClock(Arc::new(AtomicI64::new(NOW)));

    let error = run_now(fixture.host(), &clock, &Targets::without_drive()).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "kit");
    assert!(names_in(folder.path()).is_empty());
    let status = last_status(&fixture);
    assert!(!status.ok);
    assert!(status.error.unwrap().contains("recovery kit"));
}

#[test]
fn scheduled_backups_are_written_pruned_and_recorded_by_the_real_core() {
    let fixture = Fixture::new();
    let folder = tempfile::tempdir().unwrap();
    fixture.send(Request::RecoveryKitCreate);
    atomik_kb_desktop_lib::backup_scheduler::choose_folder(fixture.host(), || {
        Ok(folder.path().to_path_buf())
    })
    .unwrap();
    fixture.send(Request::SettingsSet {
        key: "app.backup.schedule".into(),
        value: serde_json::json!({ "enabled": true, "interval": "daily", "keep": 2 }),
    });
    let now = Arc::new(AtomicI64::new(NOW));
    let clock = TestClock(Arc::clone(&now));

    let mut written = Vec::new();
    for _ in 0..3 {
        let result = run_now(fixture.host(), &clock, &Targets::without_drive()).unwrap();
        written.push(
            Path::new(&result.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        assert!(result.summary.bytes > 0);
        now.fetch_add(24 * 3_600_000, Ordering::SeqCst);
    }

    // Only the newest two scheduled backups are left, and they are backups: the core's magic
    // number starts the file.
    assert_eq!(names_in(folder.path()), written[1..]);
    for name in &written[1..] {
        let bytes = fs::read(folder.path().join(name)).unwrap();
        assert!(bytes.starts_with(kb_core::BACKUP_MAGIC), "{name}");
    }
    let status = last_status(&fixture);
    assert!(status.ok);
    assert_eq!(status.file.as_deref(), written.last().map(String::as_str));
    // The webview may read the folder setting; the value is the folder the shell stored.
    match fixture.send(Request::SettingsGet {
        key: BACKUP_FOLDER_SETTING.into(),
    }) {
        Response::Setting(Some(Json::String(text))) => {
            assert_eq!(text, folder.path().to_str().unwrap());
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn last_status(fixture: &Fixture) -> BackupRunStatus {
    match fixture.send(Request::SettingsGet {
        key: BACKUP_LAST_SETTING.into(),
    }) {
        Response::Setting(Some(value)) => serde_json::from_value(value).unwrap(),
        other => panic!("unexpected {other:?}"),
    }
}

/// The zip is a real one: another reader (the plain `zip` crate API) opens it and every entry
/// checks its checksum.
#[test]
fn the_exported_archive_is_a_valid_zip_for_any_reader() {
    let fixture = Fixture::new();
    let work = tempfile::tempdir().unwrap();
    let vault = work.path().join("Vault");
    write_vault(&vault);
    let source = ImportSource::from_folder(&vault).unwrap();
    fixture.import(fixture.a, None, source, ImportSourceKind::Folder);
    let archive_path = work.path().join("out.zip");
    fixture.export(fixture.a, None, ExportFormat::Markdown, &archive_path);

    let mut archive = ZipArchive::new(fs::File::open(&archive_path).unwrap()).unwrap();
    assert!(!archive.is_empty());
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut sink = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut sink).unwrap();
        assert_eq!(sink.len() as u64, entry.size());
    }
}

// ----- an AppFlowy backup into new spaces (ADR-0039) -------------------------------------------------

/// A document of top-level blocks in AppFlowy's JSON: `(type, data, delta)`.
fn af_doc(blocks: &[(&str, serde_json::Value, Option<serde_json::Value>)]) -> String {
    use serde_json::{Map, Value, json};
    let mut map = Map::new();
    let mut children = Map::new();
    let mut text = Map::new();
    let mut ids: Vec<Value> = Vec::new();
    map.insert(
        "root".into(),
        json!({"id": "root", "ty": "page", "parent": "", "children": "ch-root",
               "external_id": null, "external_type": null, "data": {}}),
    );
    for (at, (ty, data, delta)) in blocks.iter().enumerate() {
        let id = format!("b{}", at + 1);
        let (external_id, external_type) = match delta {
            Some(delta) => {
                text.insert(format!("tx-{id}"), Value::String(delta.to_string()));
                (json!(format!("tx-{id}")), json!("text"))
            }
            None => (Value::Null, Value::Null),
        };
        map.insert(
            id.clone(),
            json!({"id": id, "ty": ty, "parent": "root", "children": format!("ch-{id}"),
                   "external_id": external_id, "external_type": external_type, "data": data}),
        );
        children.insert(format!("ch-{id}"), json!([]));
        ids.push(json!(id));
    }
    children.insert("ch-root".into(), Value::Array(ids));
    json!({"page_id": "root", "blocks": map,
           "meta": {"children_map": children, "text_map": text}})
    .to_string()
}

fn af_view(
    id: &str,
    name: &str,
    parent: &str,
    children: &[&str],
    space: bool,
    at: i64,
) -> serde_json::Value {
    let extra = space
        .then(|| serde_json::json!({"is_space": true, "space_created_at": at * 1000}).to_string());
    serde_json::json!({"view_id": id, "name": name, "layout": 0, "parent_id": parent,
        "children": children, "collab_object_id": id, "created_at": at, "updated_at": at,
        "extra": extra, "icon": null})
}

/// A backup zip as AppFlowy writes it, in one wrapping folder: two spaces and three pages, a
/// local file, a remote image and the binary copies the shell must skip.
fn write_appflowy_zip(path: &Path) {
    use serde_json::json;
    let views = json!({
        "sp-work": af_view("sp-work", "Work", "ws", &["p-plan"], true, 1_700_000_100),
        "p-plan": af_view("p-plan", "Plan", "sp-work", &["p-sub"], false, 1_700_000_110),
        "p-sub": af_view("p-sub", "Sub", "p-plan", &[], false, 1_700_000_120),
        "sp-home": af_view("sp-home", "Home", "ws", &["p-recipes"], true, 1_700_000_200),
        "p-recipes": af_view("p-recipes", "Recipes", "sp-home", &[], false, 1_700_000_210),
    });
    let plan = af_doc(&[
        (
            "paragraph",
            json!({}),
            Some(json!([{"insert": "go to "},
                {"insert": "$", "attributes": {"mention": {"type": "page", "page_id": "p-recipes"}}}])),
        ),
        (
            "image",
            json!({"url": "https://cloud.example.com/img/one.png"}),
            None,
        ),
        (
            "file",
            json!({"url": "p-plan/hash1.pdf", "url_type": 2, "name": "Report.pdf"}),
            None,
        ),
    ]);
    let sub = af_doc(&[(
        "paragraph",
        json!({}),
        Some(json!([{"insert": "sub text"}])),
    )]);
    let recipes = af_doc(&[
        (
            "paragraph",
            json!({}),
            Some(json!([{"insert": "back to "},
                {"insert": "$", "attributes": {"mention": {"type": "page", "page_id": "p-plan"}}}])),
        ),
        (
            "image",
            json!({"url": "https://cloud.example.com/img/two.png"}),
            None,
        ),
    ]);
    let metadata = json!({"export_info": {"workspace_id": "ws", "version": "1.0"}}).to_string();
    let relation = json!({"workspace_id": "ws", "views": views, "collab_objects": {},
                          "dependencies": [], "workspace_database_meta": []})
    .to_string();
    let entries: Vec<(&str, &[u8])> = vec![
        ("Backup/metadata.json", metadata.as_bytes()),
        ("Backup/relation_map.json", relation.as_bytes()),
        ("Backup/collab_jsons/documents/p-plan.json", plan.as_bytes()),
        ("Backup/collab_jsons/documents/p-sub.json", sub.as_bytes()),
        (
            "Backup/collab_jsons/documents/p-recipes.json",
            recipes.as_bytes(),
        ),
        ("Backup/collab_objects/p-plan.bin", b"yjs"),
        ("Backup/local_files/hash1.pdf", b"%PDF-1.7 report"),
    ];
    common::write_zip(path, &entries);
}

#[test]
fn an_appflowy_backup_zip_becomes_two_new_spaces_with_its_files_and_downloaded_images() {
    let fixture = Fixture::new();
    let work_dir = tempfile::tempdir().unwrap();
    let zip_path = work_dir.path().join("My AppFlowy.zip");
    write_appflowy_zip(&zip_path);
    let source = ImportSource::from_zip(&zip_path, &work_dir.path().join("cache")).unwrap();
    // The person agrees; the first image comes, the second is not there.
    let remote = common::ScriptedRemote::new(true);
    remote.serve("https://cloud.example.com/img/one.png", b"\x89PNG-one");

    let report = importer::run(
        fixture.host(),
        &ImportRunOptions::new_spaces(ImportSourceKind::Zip),
        source,
        &remote,
    )
    .unwrap();

    // Two spaces after the two the fixture has, named after AppFlowy's.
    let spaces = match fixture.send(Request::SpaceList) {
        Response::Spaces(spaces) => spaces,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        spaces.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Space A", "Space B", "Work", "Home"]
    );
    let (work, home) = (spaces[2].id, spaces[3].id);
    assert_eq!(report.space_ids, [work, home]);
    assert_eq!(report.pages, 3);
    assert_eq!(
        titles_without_indent(&fixture.titles(work)),
        ["Plan", "Sub"],
        "the space root is not a page: its children are the space's root pages"
    );
    assert_eq!(titles_without_indent(&fixture.titles(home)), ["Recipes"]);

    // The file was uploaded under its own name into the space of the page that uses it; so was
    // the downloaded image. The page of the other space has neither file (they stay in the
    // space of their first user), and the image that could not be fetched is reported.
    let attachments = |space_id| match fixture.send(Request::AttachmentList { space_id }) {
        Response::Attachments(list) => list,
        other => panic!("unexpected {other:?}"),
    };
    let mut in_work: Vec<String> = attachments(work).into_iter().map(|a| a.name).collect();
    in_work.sort();
    assert_eq!(in_work, ["Report.pdf", "one.png"]);
    assert!(attachments(home).is_empty());
    let failed: Vec<(&str, Option<&str>)> = report
        .warnings
        .iter()
        .filter(|warning| warning.code == ImportWarningCode::AttachmentFailed)
        .map(|warning| (warning.path.as_str(), warning.detail.as_deref()))
        .collect();
    assert_eq!(failed, [("remote/1", Some("cloud.example.com"))]);
    assert_eq!(
        remote.asked.lock().unwrap()[0]
            .iter()
            .map(|file| file.host.as_str())
            .collect::<Vec<_>>(),
        ["cloud.example.com", "cloud.example.com"]
    );
}

#[test]
fn saying_no_leaves_the_images_of_a_backup_as_links() {
    let fixture = Fixture::new();
    let work_dir = tempfile::tempdir().unwrap();
    let zip_path = work_dir.path().join("Backup.zip");
    write_appflowy_zip(&zip_path);
    let source = ImportSource::from_zip(&zip_path, &work_dir.path().join("cache")).unwrap();
    let remote = common::ScriptedRemote::new(false);

    let report = importer::run(
        fixture.host(),
        &ImportRunOptions::new_spaces(ImportSourceKind::Zip),
        source,
        &remote,
    )
    .unwrap();

    assert!(
        remote.fetched.lock().unwrap().is_empty(),
        "nothing was fetched"
    );
    let remote_images = report
        .warnings
        .iter()
        .filter(|warning| warning.code == ImportWarningCode::RemoteImage)
        .count();
    assert_eq!(remote_images, 2, "{:?}", report.warnings);
    assert_eq!(report.space_ids.len(), 2);
    // Only the local file was uploaded.
    let work = report.space_ids[0];
    let names: Vec<String> = match fixture.send(Request::AttachmentList { space_id: work }) {
        Response::Attachments(list) => list.into_iter().map(|a| a.name).collect(),
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(names, ["Report.pdf"]);
}
