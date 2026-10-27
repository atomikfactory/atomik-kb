//! Helpers shared by the import/export integration tests: zip files read and written with the
//! `zip` crate, listeners on the state's events, and a **scripted core** (`Fake`) that answers the
//! ten requests of ADR-0036 from a script the test fills in, so the shell's logic (the archive
//! writer, the source readers, the upload loop, the scheduled backups) is exercised without the
//! real core. What the real core does with the same requests is covered in `kb-core` and, end to
//! end, in `import_export_core.rs`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use atomik_kb_desktop_lib::importer::{RemoteFiles, RemoteImage};
use atomik_kb_desktop_lib::{
    AppState, AttachmentSink, BackupSink, BackupSource, CoreApi, CoreHost, LockedApi,
};
use kb_types::{
    AccountId, ApiError, AttachmentId, AttachmentInfo, BackupSummary, ByteRange, Event,
    ExportBatch, ExportPlan, ImportPlanInfo, ImportProgress, ImportRemoteFile, ImportReport,
    ImportSpaceInfo, ImportWarning, Json, PageId, Request, Response, SpaceId,
};

// --- zip files ---

/// One entry of a zip as a test reads it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub bytes: Vec<u8>,
    pub method: zip::CompressionMethod,
    pub modified: Option<zip::DateTime>,
}

/// Every file entry of the zip at `path` (directories left out), read to the end so that every
/// checksum is verified, by name.
pub fn read_zip(path: &Path) -> BTreeMap<String, Entry> {
    let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        if file.is_dir() {
            continue;
        }
        let name = file.name().unwrap().into_owned();
        let method = file.compression();
        let modified = file.last_modified();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        entries.insert(
            name,
            Entry {
                bytes,
                method,
                modified,
            },
        );
    }
    entries
}

/// The names of the file entries of the zip at `path`, in archive order.
pub fn zip_names(path: &Path) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .unwrap()
                .name()
                .unwrap()
                .into_owned()
        })
        .collect()
}

/// Writes a zip at `path` with the given entries (names exactly as given, deflated).
pub fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

/// The same, to memory (for a nested zip).
pub fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// The names in `dir`, sorted (empty when it is not there).
pub fn names_in(dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// --- events ---

/// Every event the state publishes from now on, as the shell's Rust listeners see it.
pub fn record_events<C: CoreApi>(state: &AppState<C>) -> Arc<Mutex<Vec<Event>>> {
    let seen: Arc<Mutex<Vec<Event>>> = Arc::default();
    let sink = Arc::clone(&seen);
    // Never removed: the state lives as long as the test.
    let _ = state.add_listener(Arc::new(move |event: &Event| {
        sink.lock().unwrap().push(event.clone());
    }));
    seen
}

// --- the scripted core ---

/// An upload (or a finished attachment) of the scripted core.
#[derive(Debug, Clone, Default)]
pub struct Blob {
    pub space_id: Option<SpaceId>,
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// What the scripted core is told to do and what it saw. Shared with the test.
#[derive(Default)]
pub struct World {
    /// The kind of every request, in order, with `write:<n>` for attachment pieces.
    pub log: Vec<String>,
    pub pending: Vec<Event>,

    // export
    pub plan: Option<ExportPlan>,
    pub batches: VecDeque<ExportBatch>,
    /// The 1-based `ExportNext` call that fails.
    pub fail_next_on: Option<(usize, ApiError)>,
    pub next_calls: usize,
    pub ended: Vec<u32>,

    // attachments
    pub stored: BTreeMap<AttachmentId, Blob>,
    pub uploads: BTreeMap<AttachmentId, Blob>,
    pub next_attachment: u8,
    /// The ranges `attachment_read` was asked for, per call.
    pub reads: Vec<(AttachmentId, Option<(u64, u64)>)>,
    /// For this attachment the n-th (1-based) read fails.
    pub fail_read: BTreeMap<AttachmentId, (usize, ApiError)>,
    /// An upload named so fails on its first write.
    pub fail_upload_named: Option<String>,

    // import
    pub began: Vec<(SpaceId, Option<PageId>, Option<String>)>,
    /// The `sourceName` of each `ImportBeginSpaces`.
    pub began_spaces: Vec<Option<String>>,
    /// What the plan of a new-spaces session says: its spaces, the space of each needed file
    /// (parallel to `needed`) and the remote files.
    pub plan_spaces: Vec<ImportSpaceInfo>,
    pub plan_file_spaces: Vec<SpaceId>,
    /// The display names of the needed files (parallel to `needed`, or empty).
    pub plan_file_names: Vec<String>,
    pub plan_remote_files: Vec<ImportRemoteFile>,
    /// `spaceIds` of the report.
    pub report_spaces: Vec<SpaceId>,
    pub files: Vec<(String, String)>,
    pub binary_paths: Vec<String>,
    /// What `ImportPlan` says is needed.
    pub needed: Vec<String>,
    pub plan_failure: Option<ApiError>,
    pub reported: Vec<(String, AttachmentId)>,
    pub commits: Vec<u32>,
    pub aborted: Vec<u32>,
    /// Pages the commit loop creates (default: one per file).
    pub total_pages: Option<u32>,
    /// Warnings the core's report carries.
    pub core_warnings: Vec<ImportWarning>,
    pub created: u32,
    pub fail_commit_on: Option<(usize, ApiError)>,
    /// Commits answer "not done" without ever creating a page.
    pub stall_commits: bool,

    // settings and backups
    pub settings: BTreeMap<String, Json>,
    pub backup_pieces: Vec<Vec<u8>>,
    pub backup_failure: Option<ApiError>,
    pub backups: usize,
    pub locked_error: bool,
}

pub type Shared = Arc<Mutex<World>>;

/// The scripted core.
pub struct Fake {
    pub world: Shared,
}

pub struct FakeLocked(pub Fake);

impl LockedApi for FakeLocked {
    type Core = Fake;

    fn unlock(self, _passphrase: &str) -> Result<Fake, (Self, ApiError)> {
        Ok(self.0)
    }
}

pub fn info_of(attachment_id: AttachmentId, blob: &Blob) -> AttachmentInfo {
    AttachmentInfo {
        attachment_id,
        space_id: blob
            .space_id
            .unwrap_or_else(|| SpaceId::from_bytes([1; 16])),
        name: blob.name.clone(),
        mime: blob.mime.clone(),
        size: blob.bytes.len() as u64,
        chunk_count: 1,
        created_at: 1_700_000_000_000,
        references: 0,
        unreferenced_since: None,
        local_chunks: 1,
        availability: kb_types::AttachmentAvailability::Complete,
    }
}

pub fn summary(bytes: u64) -> BackupSummary {
    BackupSummary {
        bytes,
        documents: 2,
        settings: 3,
        created_at: 1_700_000_000_000,
        account_id: AccountId::from_bytes([7; 16]),
        chunks: 0,
        chunk_bytes: 0,
    }
}

impl Fake {
    pub fn new() -> (Self, Shared) {
        let world: Shared = Arc::default();
        (
            Self {
                world: Arc::clone(&world),
            },
            world,
        )
    }

    /// A ready state over a new scripted core.
    pub fn ready() -> (Arc<AppState<Fake>>, Shared) {
        let (fake, world) = Self::new();
        let state = Arc::new(AppState::<Fake>::new());
        state.set_ready(fake);
        (state, world)
    }

    fn dispatch_one(&mut self, request: Request) -> Result<Response, ApiError> {
        let mut world = self.world.lock().unwrap();
        world.log.push(request.kind().to_owned());
        match request {
            Request::ExportBegin { .. } => world
                .plan
                .clone()
                .map(Response::ExportPlanned)
                .ok_or_else(|| ApiError::not_found("no such page")),
            Request::ExportNext { export_id, .. } => {
                world.next_calls += 1;
                if let Some((call, error)) = world.fail_next_on.clone()
                    && call == world.next_calls
                {
                    return Err(error);
                }
                let batch = world.batches.pop_front().unwrap_or(ExportBatch {
                    entries: Vec::new(),
                    done: true,
                    warnings: Vec::new(),
                });
                let done = u32::try_from(world.next_calls).unwrap();
                world.pending.push(Event::ExportProgress {
                    export_id,
                    done,
                    total: 99,
                });
                Ok(Response::ExportBatch(batch))
            }
            Request::ExportEnd { export_id } => {
                world.ended.push(export_id);
                Ok(Response::Ok)
            }
            Request::ImportBegin {
                space_id,
                parent_id,
                source_name,
            } => {
                world.began.push((space_id, parent_id, source_name));
                Ok(Response::ImportStarted(7))
            }
            Request::ImportBeginSpaces { source_name } => {
                world.began_spaces.push(source_name);
                Ok(Response::ImportStarted(7))
            }
            Request::ImportAddFile { path, text, .. } => {
                world.files.push((path, text));
                Ok(Response::Ok)
            }
            Request::ImportPlan { binary_paths, .. } => {
                world.binary_paths = binary_paths;
                if let Some(error) = world.plan_failure.clone() {
                    return Err(error);
                }
                let pages = u32::try_from(world.files.len()).unwrap();
                let needed_files = world.needed.clone();
                Ok(Response::ImportPlanned(ImportPlanInfo {
                    pages,
                    needed_files,
                    warnings: Vec::new(),
                    spaces: world.plan_spaces.clone(),
                    file_spaces: world.plan_file_spaces.clone(),
                    file_names: world.plan_file_names.clone(),
                    remote_files: world.plan_remote_files.clone(),
                }))
            }
            Request::ImportAddAttachment {
                path,
                attachment_id,
                ..
            } => {
                world.reported.push((path, attachment_id));
                Ok(Response::Ok)
            }
            Request::ImportCommit {
                import_id,
                max_pages,
            } => {
                world.commits.push(max_pages);
                if let Some((call, error)) = world.fail_commit_on.clone()
                    && call == world.commits.len()
                {
                    return Err(error);
                }
                let total = world
                    .total_pages
                    .unwrap_or_else(|| u32::try_from(world.files.len()).unwrap());
                if !world.stall_commits {
                    world.created = (world.created + max_pages).min(total);
                }
                let created = world.created;
                world.pending.push(Event::ImportProgress {
                    import_id,
                    done: created,
                    total,
                });
                let done = created >= total && !world.stall_commits;
                let report = done.then(|| ImportReport {
                    root_page_ids: vec![PageId::from_bytes([5; 16])],
                    pages: total,
                    attachments: u32::try_from(world.reported.len()).unwrap(),
                    properties_created: 0,
                    warnings: world.core_warnings.clone(),
                    more_warnings: 0,
                    space_ids: world.report_spaces.clone(),
                });
                Ok(Response::ImportProgress(ImportProgress {
                    created,
                    total,
                    done,
                    report,
                }))
            }
            Request::ImportAbort { import_id } => {
                world.aborted.push(import_id);
                Ok(Response::Ok)
            }
            Request::AttachmentBegin {
                space_id,
                name,
                mime,
            } => {
                world.next_attachment += 1;
                let id = AttachmentId::from_bytes([world.next_attachment; 16]);
                world.uploads.insert(
                    id,
                    Blob {
                        space_id: Some(space_id),
                        name,
                        mime,
                        bytes: Vec::new(),
                    },
                );
                Ok(Response::AttachmentStarted(id))
            }
            Request::AttachmentFinish { attachment_id } => {
                let blob = world
                    .uploads
                    .remove(&attachment_id)
                    .ok_or_else(|| ApiError::not_found("no such upload"))?;
                let info = info_of(attachment_id, &blob);
                world.stored.insert(attachment_id, blob);
                Ok(Response::Attachment(info))
            }
            Request::AttachmentAbort { attachment_id } => {
                world.uploads.remove(&attachment_id);
                Ok(Response::Ok)
            }
            Request::SettingsGet { key } => {
                Ok(Response::Setting(world.settings.get(&key).cloned()))
            }
            Request::SettingsSet { key, value } => {
                world.settings.insert(key.clone(), value);
                world.pending.push(Event::SettingChanged { key });
                Ok(Response::Ok)
            }
            _ => Ok(Response::Ok),
        }
    }
}

impl CoreApi for Fake {
    type Locked = FakeLocked;

    fn dispatch(&mut self, request: Request) -> Result<Response, ApiError> {
        self.dispatch_one(request)
    }

    fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.world.lock().unwrap().pending)
    }

    fn page_doc_open(&mut self, _: PageId) -> Result<Vec<u8>, ApiError> {
        Ok(vec![1])
    }

    fn page_doc_import(&mut self, _: PageId, _: &[u8]) -> Result<(), ApiError> {
        Ok(())
    }

    fn page_doc_export(&mut self, _: PageId, _: Option<&[u8]>) -> Result<Vec<u8>, ApiError> {
        Ok(vec![2])
    }

    fn page_doc_close(&mut self, _: PageId) -> Result<(), ApiError> {
        Ok(())
    }

    fn lock(self) -> Result<FakeLocked, (Self, ApiError)> {
        Ok(FakeLocked(self))
    }

    fn backup_export(&mut self, sink: BackupSink<'_>) -> Result<BackupSummary, ApiError> {
        let (pieces, failure) = {
            let mut world = self.world.lock().unwrap();
            world.backups += 1;
            (world.backup_pieces.clone(), world.backup_failure.clone())
        };
        let mut bytes = 0;
        for piece in &pieces {
            sink(piece).map_err(ApiError::storage)?;
            bytes += piece.len() as u64;
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(summary(bytes)),
        }
    }

    fn backup_import(
        &mut self,
        _: BackupSource<'_>,
        _: Option<&str>,
    ) -> Result<BackupSummary, ApiError> {
        Ok(summary(0))
    }

    fn attachment_write(
        &mut self,
        attachment_id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let mut world = self.world.lock().unwrap();
        world.log.push(format!("write:{}", bytes.len()));
        if world.locked_error {
            return Err(ApiError::locked());
        }
        let failing = world.fail_upload_named.clone();
        let blob = world
            .uploads
            .get_mut(&attachment_id)
            .ok_or_else(|| ApiError::not_found("no such upload"))?;
        if failing.as_deref() == Some(blob.name.as_str()) {
            return Err(ApiError::storage("the chunk could not be stored"));
        }
        blob.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn attachment_read(
        &mut self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        let blob = {
            let mut world = self.world.lock().unwrap();
            world
                .reads
                .push((attachment_id, range.map(|r| (r.start, r.end))));
            let calls = world
                .reads
                .iter()
                .filter(|(id, _)| *id == attachment_id)
                .count();
            if let Some((nth, error)) = world.fail_read.get(&attachment_id)
                && *nth == calls
            {
                return Err(error.clone());
            }
            world
                .stored
                .get(&attachment_id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("no such attachment"))?
        };
        let wanted: &[u8] = match range {
            None => &blob.bytes,
            Some(range) => {
                let start = usize::try_from(range.start).unwrap();
                let end = usize::try_from(range.end).unwrap();
                if end > blob.bytes.len() || start >= end {
                    return Err(ApiError::invalid("range", "the range is outside the file"));
                }
                &blob.bytes[start..end]
            }
        };
        // Small pieces: the shell must cope with any size.
        for piece in wanted.chunks(3) {
            sink(piece)?;
        }
        Ok(info_of(attachment_id, &blob))
    }
}

/// Puts an attachment into the scripted core's store.
pub fn store_attachment(world: &Shared, byte: u8, name: &str, bytes: &[u8]) -> AttachmentId {
    let id = AttachmentId::from_bytes([byte; 16]);
    world.lock().unwrap().stored.insert(
        id,
        Blob {
            space_id: None,
            name: name.to_owned(),
            mime: "application/octet-stream".to_owned(),
            bytes: bytes.to_vec(),
        },
    );
    id
}

/// The host a function under test takes.
pub fn host<C: CoreApi>(state: &Arc<AppState<C>>) -> &dyn CoreHost {
    &**state
}

// --- the files a source keeps on a server ---

/// For an import whose plan lists no remote file: asking about one, or fetching one, is a bug.
pub struct NoRemote;

impl RemoteFiles for NoRemote {
    fn consent(&self, _: &[ImportRemoteFile]) -> bool {
        panic!("the plan listed no remote file, so nobody should be asked");
    }

    fn fetch(&self, _: &str) -> Result<RemoteImage, ApiError> {
        panic!("the plan listed no remote file, so nothing should be fetched");
    }
}

/// A remote that answers the question as told, serves what the test gives it (by link) and notes
/// what was asked, what was fetched and how many fetches overlapped.
pub struct ScriptedRemote {
    pub agree: bool,
    /// Every question: the files it was about.
    pub asked: Mutex<Vec<Vec<ImportRemoteFile>>>,
    /// The answer for each link; a link that is not here is a network failure.
    pub images: Mutex<BTreeMap<String, Result<RemoteImage, ApiError>>>,
    /// Every link fetched, in the order the fetches began.
    pub fetched: Mutex<Vec<String>>,
    /// How long each fetch takes, so that fetches overlap.
    pub delay: Duration,
    in_flight: AtomicUsize,
    pub max_in_flight: AtomicUsize,
}

impl ScriptedRemote {
    pub fn new(agree: bool) -> Self {
        Self {
            agree,
            asked: Mutex::default(),
            images: Mutex::default(),
            fetched: Mutex::default(),
            delay: Duration::from_millis(25),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
        }
    }

    pub fn serve(&self, link: &str, bytes: &[u8]) {
        let name = link.rsplit('/').next().unwrap_or(link).to_owned();
        self.images.lock().unwrap().insert(
            link.to_owned(),
            Ok(RemoteImage {
                name,
                mime: "image/png".to_owned(),
                bytes: bytes.to_vec(),
            }),
        );
    }
}

impl RemoteFiles for ScriptedRemote {
    fn consent(&self, files: &[ImportRemoteFile]) -> bool {
        self.asked.lock().unwrap().push(files.to_vec());
        self.agree
    }

    fn fetch(&self, link: &str) -> Result<RemoteImage, ApiError> {
        self.fetched.lock().unwrap().push(link.to_owned());
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.images
            .lock()
            .unwrap()
            .get(link)
            .cloned()
            .unwrap_or_else(|| Err(ApiError::invalid("url", "the server could not be reached")))
    }
}
