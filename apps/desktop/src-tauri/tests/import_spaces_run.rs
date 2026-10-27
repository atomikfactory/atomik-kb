//! `import_run` into new spaces and an AppFlowy backup (ADR-0039, `docs/architecture/appflowy-import.md`
//! sections 6 and 9, "Shell"), without a dialog and against the scripted core: how a backup zip or
//! folder is recognised (its `.json` files are text, `collab_objects/` is skipped), `ImportBeginSpaces`
//! and the upload of each needed file into the space the plan gives it, and the files a source keeps
//! on a server: the question asked once, nothing fetched on No, and on Yes the downloads (four at a
//! time), their uploads into their own spaces and their reports under their keys.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::type_complexity
)]

mod common;

use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

use atomik_kb_desktop_lib::import_source::ImportSource;
use atomik_kb_desktop_lib::importer::{
    self, ImportDestination, ImportRunOptions, ImportSourceKind, MAX_PARALLEL_DOWNLOADS,
};
use kb_types::{
    ApiError, ErrorCode, ImportRemoteFile, ImportReport, ImportSpaceInfo, ImportWarningCode,
    SpaceId,
};
use serde_json::json;

use common::{Fake, NoRemote, ScriptedRemote, host, write_zip};

fn space(n: u8) -> SpaceId {
    SpaceId::from_bytes([n; 16])
}

fn put(dir: &Path, relative: &str, bytes: &[u8]) {
    let path = dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn texts(world: &common::World) -> Vec<&str> {
    let mut paths: Vec<&str> = world.files.iter().map(|(path, _)| path.as_str()).collect();
    paths.sort_unstable();
    paths
}

/// The entries of a small synthetic backup, under `root` (empty or `Folder/`).
fn backup_entries(root: &str) -> Vec<(String, Vec<u8>)> {
    [
        ("metadata.json", &b"{\"export_info\":{}}"[..]),
        ("relation_map.json", b"{\"views\":{}}"),
        ("EXPORT_WARNINGS.json", b"{}"),
        ("collab_jsons/documents/d1.json", b"{\"blocks\":{}}"),
        ("collab_jsons/databases/v1.json", b"{\"rows\":[]}"),
        ("collab_objects/one.bin", b"yjs"),
        ("collab_objects/two.json", b"{\"binary\":\"copy\"}"),
        ("collab_objects/sub/three.md", b"# not a page"),
        ("local_files/hash-one.png", b"png"),
        ("local_files/attached.json", b"{\"an\":\"attachment\"}"),
        ("notes/stray.json", b"{}"),
    ]
    .into_iter()
    .map(|(name, bytes)| (format!("{root}{name}"), bytes.to_vec()))
    .collect()
}

fn write_backup_zip(path: &Path, root: &str) {
    let entries = backup_entries(root);
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    write_zip(path, &borrowed);
}

fn run_new_spaces(
    state: &std::sync::Arc<atomik_kb_desktop_lib::AppState<Fake>>,
    source: ImportSource,
    remote: &dyn importer::RemoteFiles,
) -> Result<ImportReport, ApiError> {
    importer::run(
        host(state),
        &ImportRunOptions::new_spaces(ImportSourceKind::Zip),
        source,
        remote,
    )
}

// --- the options ---

#[test]
fn the_options_say_where_the_pages_go() {
    let page: ImportRunOptions =
        serde_json::from_value(json!({ "spaceId": space(1).to_string(), "source": "zip" }))
            .unwrap();
    assert_eq!(page.destination, ImportDestination::Page);
    assert_eq!(page.space_id, Some(space(1)));
    assert_eq!(page.check(), Ok(()));

    let named: ImportRunOptions = serde_json::from_value(json!({
        "destination": "page",
        "spaceId": space(1).to_string(),
        "parentId": kb_types::PageId::from_bytes([4; 16]).to_string(),
        "source": "folder"
    }))
    .unwrap();
    assert_eq!(named.parent_id, Some(kb_types::PageId::from_bytes([4; 16])));

    // New spaces need no space; one that is given is not looked at.
    let spaces: ImportRunOptions =
        serde_json::from_value(json!({ "destination": "newSpaces", "source": "zip" })).unwrap();
    assert_eq!(spaces, ImportRunOptions::new_spaces(ImportSourceKind::Zip));
    assert_eq!(spaces.check(), Ok(()));
    let with_space: ImportRunOptions = serde_json::from_value(json!({
        "destination": "newSpaces",
        "spaceId": space(1).to_string(),
        "source": "zip"
    }))
    .unwrap();
    assert_eq!(with_space.destination, ImportDestination::NewSpaces);

    // An import into a page needs its space.
    let missing: ImportRunOptions = serde_json::from_value(json!({ "source": "zip" })).unwrap();
    let error = missing.check().unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "spaceId");
    assert!(
        serde_json::from_value::<ImportRunOptions>(
            json!({ "destination": "elsewhere", "source": "zip" })
        )
        .is_err()
    );
}

#[test]
fn an_import_into_a_page_without_a_space_asks_the_core_nothing() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a.md", b"a");
    let source = ImportSource::from_folder(root.path()).unwrap();
    let mut options = ImportRunOptions::new_spaces(ImportSourceKind::Folder);
    options.destination = ImportDestination::Page;

    let error = importer::run(host(&state), &options, source, &NoRemote).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert!(world.lock().unwrap().log.is_empty());
}

// --- an AppFlowy backup ---

#[test]
fn a_backup_zip_has_its_json_read_as_text_and_collab_objects_skipped() {
    for root in ["", "Backup/"] {
        let (state, world) = Fake::ready();
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("My workspace.zip");
        write_backup_zip(&zip_path, root);

        let mut source = ImportSource::from_zip(&zip_path, &dir.path().join("cache")).unwrap();
        assert_eq!(source.name(), Some("My workspace"));
        assert!(
            source.take_warnings().is_empty(),
            "collab_objects is skipped without a word ({root:?})"
        );
        let report = run_new_spaces(&state, source, &NoRemote).unwrap();

        let world = world.lock().unwrap();
        // New spaces: the session begins with the source's name and there is no destination page.
        assert!(world.began.is_empty(), "{root:?}");
        assert_eq!(world.began_spaces, [Some("My workspace".to_owned())]);
        let expected: Vec<String> = [
            "EXPORT_WARNINGS.json",
            "collab_jsons/databases/v1.json",
            "collab_jsons/documents/d1.json",
            "metadata.json",
            "notes/stray.json",
            "relation_map.json",
        ]
        .iter()
        .map(|name| format!("{root}{name}"))
        .collect();
        assert_eq!(texts(&world), expected, "{root:?}");
        // The files attached to the workspace stay binary, even a `.json`; the binary copies are
        // not listed at all.
        assert_eq!(
            world.binary_paths,
            [
                format!("{root}local_files/attached.json"),
                format!("{root}local_files/hash-one.png")
            ],
            "{root:?}"
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }
}

#[test]
fn a_backup_folder_is_recognised_too() {
    for wrapper in ["", "Workspace backup/"] {
        let (state, world) = Fake::ready();
        let root = tempfile::tempdir().unwrap();
        for (name, bytes) in backup_entries(wrapper) {
            put(root.path(), &name, &bytes);
        }
        let source = ImportSource::from_folder(root.path()).unwrap();
        let folder_name = root
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();

        let report = importer::run(
            host(&state),
            &ImportRunOptions::new_spaces(ImportSourceKind::Folder),
            source,
            &NoRemote,
        )
        .unwrap();

        let world = world.lock().unwrap();
        assert_eq!(world.began_spaces, [Some(folder_name)]);
        assert_eq!(texts(&world).len(), 6, "{wrapper:?}");
        assert!(
            texts(&world).contains(&format!("{wrapper}relation_map.json").as_str()),
            "{wrapper:?}"
        );
        assert_eq!(world.binary_paths.len(), 2, "{wrapper:?}");
        assert!(report.warnings.is_empty());
    }
}

#[test]
fn a_source_that_is_not_a_backup_keeps_its_json_files_as_files() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let cases: Vec<(&str, Vec<(&str, &[u8])>)> = vec![
        // One of the two files is not enough.
        (
            "only the map",
            vec![("relation_map.json", b"{}"), ("data/other.json", b"{}")],
        ),
        (
            "only the metadata",
            vec![("metadata.json", b"{}"), ("data/other.json", b"{}")],
        ),
        // Two top-level folders are not one wrapping folder.
        (
            "two folders",
            vec![
                ("A/relation_map.json", b"{}"),
                ("A/metadata.json", b"{}"),
                ("B/other.json", b"{}"),
            ],
        ),
        // Deeper than one wrapping folder.
        (
            "too deep",
            vec![
                ("A/B/relation_map.json", b"{}"),
                ("A/B/metadata.json", b"{}"),
            ],
        ),
        // A file beside the folder.
        (
            "a stray file",
            vec![
                ("A/relation_map.json", b"{}"),
                ("A/metadata.json", b"{}"),
                ("readme.txt", b"hi"),
            ],
        ),
    ];
    for (what, entries) in cases {
        let zip_path = dir.path().join("Other.zip");
        write_zip(&zip_path, &entries);
        let source = ImportSource::from_zip(&zip_path, &cache).unwrap();
        let json_texts: Vec<String> = source
            .text_files()
            .iter()
            .map(|file| file.path().to_owned())
            .filter(|path| path.ends_with(".json"))
            .collect();
        assert!(json_texts.is_empty(), "{what}: {json_texts:?}");
        assert!(
            source
                .binary_paths()
                .iter()
                .any(|path| path.ends_with(".json")),
            "{what}"
        );
    }
}

#[test]
fn collab_objects_stay_when_the_source_is_not_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("Other.zip");
    write_zip(
        &zip_path,
        &[("collab_objects/a.bin", b"x"), ("notes.md", b"# n")],
    );
    let source = ImportSource::from_zip(&zip_path, &dir.path().join("cache")).unwrap();
    assert_eq!(source.binary_paths(), ["collab_objects/a.bin"]);
}

// --- new spaces ---

#[test]
fn each_needed_file_is_uploaded_into_the_space_the_plan_gives_it() {
    let (state, world) = Fake::ready();
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("Backup.zip");
    write_zip(
        &zip_path,
        &[
            ("page.md", b"# Page"),
            ("local_files/one.png", b"first"),
            ("local_files/two.pdf", b"second"),
            ("local_files/three.png", b"third"),
        ],
    );
    {
        let mut world = world.lock().unwrap();
        world.needed = vec![
            "local_files/one.png".to_owned(),
            "local_files/two.pdf".to_owned(),
            "local_files/three.png".to_owned(),
        ];
        world.plan_file_spaces = vec![space(10), space(11), space(10)];
        world.plan_spaces = vec![
            ImportSpaceInfo {
                space_id: space(10),
                name: "Work".into(),
                pages: 2,
            },
            ImportSpaceInfo {
                space_id: space(11),
                name: "Home".into(),
                pages: 1,
            },
        ];
        world.report_spaces = vec![space(10), space(11)];
    }
    let source = ImportSource::from_zip(&zip_path, &dir.path().join("cache")).unwrap();

    let report = run_new_spaces(&state, source, &NoRemote).unwrap();

    let world = world.lock().unwrap();
    let stored: Vec<(&str, Option<SpaceId>)> = world
        .stored
        .values()
        .map(|blob| (blob.name.as_str(), blob.space_id))
        .collect();
    assert_eq!(
        stored,
        [
            ("one.png", Some(space(10))),
            ("two.pdf", Some(space(11))),
            ("three.png", Some(space(10)))
        ]
    );
    assert_eq!(
        world
            .reported
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "local_files/one.png",
            "local_files/two.pdf",
            "local_files/three.png"
        ]
    );
    // The report is the core's: its spaces come through.
    assert_eq!(report.space_ids, [space(10), space(11)]);
    assert!(report.warnings.is_empty());
    assert_eq!(world.log[0], "ImportBeginSpaces");
}

#[test]
fn a_file_the_plan_names_is_uploaded_under_that_name() {
    // An AppFlowy backup stores its files by content hash; the plan carries their own names.
    let (state, world) = Fake::ready();
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("Backup.zip");
    write_zip(
        &zip_path,
        &[
            ("local_files/aGFzaDE.pdf", b"first"),
            ("local_files/aGFzaDI.png", b"second"),
            ("local_files/aGFzaDM.bin", b"third"),
        ],
    );
    {
        let mut world = world.lock().unwrap();
        world.needed = vec![
            "local_files/aGFzaDE.pdf".to_owned(),
            "local_files/aGFzaDI.png".to_owned(),
            "local_files/aGFzaDM.bin".to_owned(),
        ];
        world.plan_file_spaces = vec![space(10); 3];
        // The second has no name of its own; the third's name has path separators in it.
        world.plan_file_names = vec![
            "Quarterly report.pdf".to_owned(),
            String::new(),
            "..\\evil/name.bin".to_owned(),
        ];
    }
    let source = ImportSource::from_zip(&zip_path, &dir.path().join("cache")).unwrap();

    let report = run_new_spaces(&state, source, &NoRemote).unwrap();

    let world = world.lock().unwrap();
    let stored: Vec<(&str, &str)> = world
        .stored
        .values()
        .map(|blob| (blob.name.as_str(), blob.mime.as_str()))
        .collect();
    assert_eq!(
        stored,
        [
            ("Quarterly report.pdf", "application/pdf"),
            ("aGFzaDI.png", "image/png"),
            ("..evilname.bin", "application/octet-stream")
        ]
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn a_file_without_a_space_in_a_new_spaces_import_is_a_warning() {
    // A plan that names no space for a file (it should not happen) loses the file, not the import.
    let (state, world) = Fake::ready();
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("Backup.zip");
    write_zip(&zip_path, &[("page.md", b"# Page"), ("pic.png", b"png")]);
    world.lock().unwrap().needed = vec!["pic.png".to_owned()];
    let source = ImportSource::from_zip(&zip_path, &dir.path().join("cache")).unwrap();

    let report = run_new_spaces(&state, source, &NoRemote).unwrap();

    assert!(world.lock().unwrap().stored.is_empty());
    let failed: Vec<&str> = report
        .warnings
        .iter()
        .filter(|warning| warning.code == ImportWarningCode::AttachmentFailed)
        .map(|warning| warning.path.as_str())
        .collect();
    assert_eq!(failed, ["pic.png"]);
}

#[test]
fn a_plain_zip_goes_into_new_spaces_too() {
    let (state, world) = Fake::ready();
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("Notes.zip");
    write_zip(&zip_path, &[("Notes/A.md", b"# A")]);
    let source = ImportSource::from_zip(&zip_path, &dir.path().join("cache")).unwrap();

    run_new_spaces(&state, source, &NoRemote).unwrap();

    let world = world.lock().unwrap();
    assert_eq!(world.began_spaces, [Some("Notes".to_owned())]);
    assert_eq!(texts(&world), ["Notes/A.md"]);
}

// --- the files a source keeps on a server ---

/// `count` remote files `remote/<n>` on `cloud<n % hosts>.example.com`, in space `10 + n % 2`.
fn remote_files(count: usize, hosts: usize) -> Vec<ImportRemoteFile> {
    (0..count)
        .map(|n| ImportRemoteFile {
            key: format!("remote/{n}"),
            url: format!("https://cloud{}.example.com/images/{n}.png", n % hosts),
            host: format!("cloud{}.example.com", n % hosts),
            space_id: space(u8::try_from(10 + n % 2).unwrap()),
        })
        .collect()
}

fn backup_with_remote(files: Vec<ImportRemoteFile>) -> (ImportSource, tempfile::TempDir, Fake0) {
    let (state, world) = Fake::ready();
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("Backup.zip");
    write_zip(&zip_path, &[("page.md", b"# Page")]);
    world.lock().unwrap().plan_remote_files = files;
    let source = ImportSource::from_zip(&zip_path, &dir.path().join("cache")).unwrap();
    (source, dir, (state, world))
}

type Fake0 = (
    std::sync::Arc<atomik_kb_desktop_lib::AppState<Fake>>,
    common::Shared,
);

#[test]
fn saying_no_downloads_nothing_and_asks_only_once() {
    let files = remote_files(6, 2);
    let (source, _dir, (state, world)) = backup_with_remote(files.clone());
    let remote = ScriptedRemote::new(false);

    let report = run_new_spaces(&state, source, &remote).unwrap();

    assert_eq!(*remote.asked.lock().unwrap(), [files]);
    assert!(
        remote.fetched.lock().unwrap().is_empty(),
        "nothing is fetched"
    );
    let world = world.lock().unwrap();
    assert!(world.stored.is_empty() && world.uploads.is_empty());
    assert!(world.reported.is_empty());
    assert!(
        !world.log.iter().any(|kind| kind.starts_with("Attachment")),
        "{:?}",
        world.log
    );
    // The import went on: the core's commit ran, and no warning is the shell's.
    assert_eq!(world.commits, [50]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn no_question_is_asked_when_the_plan_lists_no_remote_file() {
    let (source, _dir, (state, _world)) = backup_with_remote(Vec::new());
    // `NoRemote` panics when asked.
    run_new_spaces(&state, source, &NoRemote).unwrap();
}

#[test]
fn saying_yes_downloads_four_at_a_time_into_their_spaces_and_reports_them_by_key() {
    let files = remote_files(10, 3);
    let (source, _dir, (state, world)) = backup_with_remote(files.clone());
    let remote = ScriptedRemote::new(true);
    for file in &files {
        remote.serve(&file.url, format!("bytes of {}", file.key).as_bytes());
    }

    let report = run_new_spaces(&state, source, &remote).unwrap();

    assert_eq!(remote.asked.lock().unwrap().len(), 1);
    let mut fetched = remote.fetched.lock().unwrap().clone();
    fetched.sort();
    let mut wanted: Vec<String> = files.iter().map(|file| file.url.clone()).collect();
    wanted.sort();
    assert_eq!(fetched, wanted, "each link was fetched once");
    let most = remote.max_in_flight.load(Ordering::SeqCst);
    assert!(
        (2..=MAX_PARALLEL_DOWNLOADS).contains(&most),
        "{most} downloads at once"
    );

    let world = world.lock().unwrap();
    // Each picture is an attachment of its file's space, reported under its key.
    assert_eq!(world.stored.len(), 10);
    for file in &files {
        let (_, id) = world
            .reported
            .iter()
            .find(|(path, _)| *path == file.key)
            .unwrap_or_else(|| panic!("{} was not reported", file.key));
        let blob = &world.stored[id];
        assert_eq!(blob.space_id, Some(file.space_id), "{}", file.key);
        assert_eq!(blob.bytes, format!("bytes of {}", file.key).as_bytes());
        assert_eq!(blob.mime, "image/png");
        assert_eq!(
            blob.name,
            file.url.rsplit('/').next().unwrap(),
            "named after the link"
        );
    }
    assert_eq!(world.reported.len(), 10);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    // The downloads come after the question and before the commit.
    let first_attachment = world
        .log
        .iter()
        .position(|kind| kind == "AttachmentBegin")
        .unwrap();
    let commit = world
        .log
        .iter()
        .position(|kind| kind == "ImportCommit")
        .unwrap();
    assert!(first_attachment < commit);
}

#[test]
fn a_download_that_fails_is_a_warning_naming_the_host_and_the_import_goes_on() {
    let files = remote_files(5, 2);
    let (source, _dir, (state, world)) = backup_with_remote(files.clone());
    let remote = ScriptedRemote::new(true);
    for file in &files {
        // `remote/1` is not served: the network fails. `remote/3` is not an image.
        if file.key == "remote/3" {
            remote.images.lock().unwrap().insert(
                file.url.clone(),
                Err(ApiError::invalid("url", "the link is not an image")),
            );
        } else if file.key != "remote/1" {
            remote.serve(&file.url, b"png");
        }
    }

    let report = run_new_spaces(&state, source, &remote).unwrap();

    let world = world.lock().unwrap();
    let mut reported: Vec<&str> = world
        .reported
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    reported.sort_unstable();
    assert_eq!(reported, ["remote/0", "remote/2", "remote/4"]);
    let mut failed: Vec<(&str, Option<&str>)> = report
        .warnings
        .iter()
        .filter(|warning| warning.code == ImportWarningCode::AttachmentFailed)
        .map(|warning| (warning.path.as_str(), warning.detail.as_deref()))
        .collect();
    failed.sort_unstable();
    assert_eq!(
        failed,
        [
            ("remote/1", Some("cloud1.example.com")),
            ("remote/3", Some("cloud1.example.com"))
        ],
        "the host, never the link"
    );
    for warning in &report.warnings {
        assert!(
            !warning.path.contains("https")
                && !warning.detail.clone().unwrap_or_default().contains('/'),
            "{warning:?}"
        );
    }
    assert_eq!(world.commits, [50], "the import was committed");
    assert!(world.aborted.is_empty());
}

#[test]
fn a_download_the_core_refuses_to_store_is_a_warning_too() {
    let files = remote_files(2, 1);
    let (source, _dir, (state, world)) = backup_with_remote(files.clone());
    let remote = ScriptedRemote::new(true);
    remote.serve(&files[0].url, b"one");
    // The scripted core refuses the upload of a file named `1.png`.
    remote.serve(&files[1].url, b"two");
    world.lock().unwrap().fail_upload_named = Some("1.png".to_owned());

    let report = run_new_spaces(&state, source, &remote).unwrap();

    let world = world.lock().unwrap();
    assert!(world.uploads.is_empty(), "the refused upload was aborted");
    assert_eq!(
        world
            .reported
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["remote/0"]
    );
    assert_eq!(
        report
            .warnings
            .iter()
            .map(|warning| (warning.code, warning.path.as_str()))
            .collect::<Vec<_>>(),
        [(ImportWarningCode::AttachmentFailed, "remote/1")]
    );
}

#[test]
fn a_locked_core_ends_the_import_during_the_downloads() {
    let files = remote_files(4, 1);
    let (source, _dir, (state, world)) = backup_with_remote(files.clone());
    let remote = ScriptedRemote::new(true);
    for file in &files {
        remote.serve(&file.url, b"png");
    }
    world.lock().unwrap().locked_error = true;

    let error = run_new_spaces(&state, source, &remote).unwrap_err();

    assert_eq!(error.code, ErrorCode::Locked);
    let world = world.lock().unwrap();
    assert!(world.commits.is_empty());
    assert_eq!(world.aborted, [7]);
    assert!(world.uploads.is_empty());
}

#[test]
fn remote_files_are_offered_for_an_import_into_a_page_too() {
    let (state, world) = Fake::ready();
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "a.md", b"a");
    let files = remote_files(3, 1);
    world.lock().unwrap().plan_remote_files = files.clone();
    let remote = ScriptedRemote::new(true);
    for file in &files {
        remote.serve(&file.url, b"png");
    }
    let source = ImportSource::from_folder(root.path()).unwrap();
    let options = ImportRunOptions::page(space(1), None, ImportSourceKind::Folder);

    let report = importer::run(host(&state), &options, source, &remote).unwrap();

    let world = world.lock().unwrap();
    assert_eq!(world.began.len(), 1);
    assert_eq!(world.reported.len(), 3);
    assert!(report.warnings.is_empty());
}
