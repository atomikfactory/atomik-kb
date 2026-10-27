//! The import session driven from the shell (ADR-0036, `docs/architecture/import-export.md`, 3.1
//! and 5.2). The core is sans-IO: it plans pages from the text files it is given, says which other
//! files the pages need, and writes the pages in batches. This module reads the source
//! ([`ImportSource`]), feeds the core and uploads the files it asks for; nothing here looks inside a
//! page.
//!
//! The order is the core's protocol: `ImportBegin`, `ImportAddFile` for every text file (decoded
//! by the source, read one at a time), `ImportPlan` with the paths of the other files,
//! `Attachment*` and `ImportAddAttachment` for each file the plan needs, then `ImportCommit` in
//! batches of [`COMMIT_PAGES`] until it answers `done` with the report. Events reach the UI after
//! every dispatch (the host publishes them), which is what moves the progress bar. Any failure
//! after `ImportBegin` ends in `ImportAbort` and the original error is what the caller gets; pages
//! already written stay.
//!
//! The shell has a say of its own, as warnings appended to the core's report: a text file above
//! 16 MiB (`TooLarge`), one that was not UTF-8 (`NotUtf8`), one that could not be read, a file
//! the source could not list (`UnsupportedFile`, `Truncated`), and a needed file that could not
//! be uploaded (`AttachmentFailed`, never reported to the core, so the pages keep their text).
//! More than 10,000 text files, or 256 MiB of text, are left out with `Truncated` instead of
//! failing the whole import. A file above 2 GiB is `AttachmentFailed` without a byte read.
//!
//! **New spaces** (ADR-0039, `docs/architecture/appflowy-import.md` section 6). With the destination
//! `newSpaces` the session starts with `ImportBeginSpaces` (the source's name is the zip's or
//! folder's) and the destination space is none: every needed file is uploaded into the space the
//! plan gives it (`fileSpaces`). **Remote files**: when the plan lists images the source keeps on
//! a server (an AppFlowy workspace's uploads), the shell asks once in a native Yes/No dialog,
//! owned by the main window, naming the host and the count. On Yes each is downloaded
//! ([`RemoteFiles::fetch`], `url_import::download`: https only, no cookies, sniffed image types,
//! 32 MiB), at most [`MAX_PARALLEL_DOWNLOADS`] at a time, uploaded as an attachment of its space
//! and reported under its key; a failure is `AttachmentFailed` (detail: the host) and the import
//! goes on. On No nothing is fetched and the images stay links. Neither a link nor a host is ever
//! logged.
//!
//! The source comes from a native dialog (the caller's business, so the tests drive this without
//! one). Logs carry counts only, never a path, a name or a page's text.

use std::io::{Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use kb_types::attachments::MAX_ATTACHMENT_BYTES;
use kb_types::{
    ApiError, AttachmentId, ErrorCode, ImportPlanInfo, ImportProgress, ImportRemoteFile,
    ImportReport, ImportWarning, ImportWarningCode, PageId, Request, Response, SpaceId,
};
use serde::Deserialize;

use crate::attachment_ipc::limit_exceeded;
use crate::attachment_names as names;
use crate::attachments;
use crate::dialogs::{Confirm, Question};
use crate::import_source::{ImportSource, SourceFile, TextRead};
use crate::shell_labels::{Labels, fill};
use crate::state::CoreHost;
use crate::url_import::{self, FetchPolicy};

/// Pages written per `ImportCommit` call.
pub const COMMIT_PAGES: u32 = 50;

/// The most remote files downloaded at the same time.
pub const MAX_PARALLEL_DOWNLOADS: usize = 4;

/// The most text files an import adds (the core refuses more).
pub const MAX_TEXT_FILES: usize = 10_000;

/// The most text an import adds, in bytes (the core refuses more).
pub const MAX_TEXT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// The most warnings a report lists; the rest are counted.
pub const MAX_REPORT_WARNINGS: usize = 500;

/// `ImportCommit` answers that bring no new page, in a row, after which the core is taken to be
/// stuck.
const MAX_STALLED_COMMITS: u32 = 3;

/// What the dialog lets the person pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportSourceKind {
    /// Some Markdown, text, HTML or CSV files.
    Files,
    /// A folder and everything in it.
    Folder,
    /// A `.zip` (a Notion or atomik-kb export).
    Zip,
}

/// Where the pages go (`import_run` option `destination`, ADR-0039).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ImportDestination {
    /// Into `spaceId` under `parentId` (`ImportBegin`).
    #[default]
    Page,
    /// Into one new space per space of the source (`ImportBeginSpaces`).
    NewSpaces,
}

/// What the UI asks for (`import_run { options }`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRunOptions {
    /// Destination space (required for the destination `page`, ignored for `newSpaces`).
    #[serde(default)]
    pub space_id: Option<SpaceId>,
    /// Destination page; none for the space root.
    #[serde(default)]
    pub parent_id: Option<PageId>,
    /// What to pick.
    pub source: ImportSourceKind,
    /// Where the pages go; `page` when absent.
    #[serde(default)]
    pub destination: ImportDestination,
}

impl ImportRunOptions {
    /// An import into a page (or the root) of a space.
    #[must_use]
    pub fn page(space_id: SpaceId, parent_id: Option<PageId>, source: ImportSourceKind) -> Self {
        Self {
            space_id: Some(space_id),
            parent_id,
            source,
            destination: ImportDestination::Page,
        }
    }

    /// An import into new spaces.
    #[must_use]
    pub fn new_spaces(source: ImportSourceKind) -> Self {
        Self {
            space_id: None,
            parent_id: None,
            source,
            destination: ImportDestination::NewSpaces,
        }
    }

    /// Whether the options name what the destination needs.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for the field `spaceId` when the destination is a page and no space is named.
    pub fn check(&self) -> Result<(), ApiError> {
        match (self.destination, self.space_id) {
            (ImportDestination::Page, None) => Err(ApiError::invalid(
                "spaceId",
                "an import into a page needs the space",
            )),
            _ => Ok(()),
        }
    }
}

fn unexpected() -> ApiError {
    ApiError::internal("the core answered an import request with something unexpected")
}

// --- the files a source keeps on a server ---

/// A picture downloaded from a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteImage {
    /// Its name (from the link's path).
    pub name: String,
    /// Its MIME type (from its bytes).
    pub mime: String,
    /// The picture.
    pub bytes: Vec<u8>,
}

/// What an import needs to bring in the files its source keeps on a server (ADR-0039): the
/// question put to the person, and the download. The commands use [`DownloadAfterConsent`]; the
/// tests answer and fetch for themselves.
pub trait RemoteFiles: Sync {
    /// Whether the person agrees to download `files` now. Asked once, only when the plan lists some.
    fn consent(&self, files: &[ImportRemoteFile]) -> bool;

    /// Downloads the picture at `link`. Called from up to [`MAX_PARALLEL_DOWNLOADS`] threads at
    /// once, and never for a file the person did not agree to. A failure is a warning for that
    /// file only.
    ///
    /// # Errors
    ///
    /// Whatever kept the picture from arriving.
    fn fetch(&self, link: &str) -> Result<RemoteImage, ApiError>;
}

/// The real thing: a native Yes/No question owned by the main window, then `url_import::download`
/// (https only, no cookies, redirects to https only, 32 MiB, sniffed image types).
pub struct DownloadAfterConsent {
    confirm: Arc<dyn Confirm>,
    policy: FetchPolicy,
}

impl DownloadAfterConsent {
    /// Asks through `confirm`, downloads under `policy`.
    pub fn new(confirm: Arc<dyn Confirm>, policy: FetchPolicy) -> Self {
        Self { confirm, policy }
    }
}

impl RemoteFiles for DownloadAfterConsent {
    fn consent(&self, files: &[ImportRemoteFile]) -> bool {
        let labels = self.confirm.labels();
        let (title, message) = consent_text(&labels, files);
        self.confirm.ask(&Question {
            title,
            message,
            accept: labels.dialog_yes.clone(),
            cancel: labels.dialog_no.clone(),
        })
    }

    fn fetch(&self, link: &str) -> Result<RemoteImage, ApiError> {
        // Called from a plain thread (never from the runtime's own): blocking on the download is fine.
        let downloaded = tauri::async_runtime::block_on(url_import::download(link, &self.policy))?;
        Ok(RemoteImage {
            name: downloaded.name,
            mime: downloaded.kind.mime().to_owned(),
            bytes: downloaded.bytes,
        })
    }
}

/// The longest host shown in the question and in a warning, in characters.
const HOST_CHARS: usize = 100;

/// A host as shown to the person: no control characters, at most [`HOST_CHARS`] characters.
fn shown_host(host: &str) -> String {
    host.chars()
        .filter(|c| !c.is_control())
        .take(HOST_CHARS)
        .collect()
}

/// Why a remote file failed, in a few words for the report and the log (never the address):
/// from the `reason` that `url_import::download` puts in the details, a size limit, or nothing
/// when the error says no more than its code.
fn failure_reason(error: &ApiError) -> Option<&'static str> {
    let details = error.details.as_ref()?;
    if details.get("limit").is_some() {
        return Some("too large");
    }
    match details.get("reason").and_then(|reason| reason.as_str())? {
        "network" => Some("could not connect"),
        "timeout" => Some("timed out"),
        "redirect" => Some("redirect refused"),
        "notImage" => Some("not an image"),
        "status" => Some(
            match details.get("status").and_then(serde_json::Value::as_u64) {
                Some(401 | 403) => "access refused by the server",
                Some(404 | 410) => "not found on the server",
                _ => "the server answered with an error",
            },
        ),
        _ => None,
    }
}

/// The title and the text of the question that asks the person to let the shell download `files`:
/// the number of images, the host that has most of them and how many other hosts there are, in the words of
/// `labels` (the UI's language; the text always says where the images are, `shell_labels` refuses a translation
/// that does not).
#[must_use]
pub fn consent_text(labels: &Labels, files: &[ImportRemoteFile]) -> (String, String) {
    let mut hosts: Vec<(&str, usize)> = Vec::new();
    for file in files {
        match hosts.iter_mut().find(|(host, _)| *host == file.host) {
            Some((_, count)) => *count += 1,
            None => hosts.push((file.host.as_str(), 1)),
        }
    }
    // The host with most files, the first one met when they are level.
    let main = hosts
        .iter()
        .fold(None::<(&str, usize)>, |best, next| match best {
            Some(best) if best.1 >= next.1 => Some(best),
            _ => Some(*next),
        });
    let main = shown_host(main.map_or("", |(host, _)| host));
    let others = hosts.len().saturating_sub(1);
    let count = files.len().to_string();
    let others_count = others.to_string();
    let stored = match others {
        0 => main,
        1 => fill(
            &labels.dialog_hosts_one,
            &[("host", &main), ("count", &others_count)],
        ),
        _ => fill(
            &labels.dialog_hosts_other,
            &[("host", &main), ("count", &others_count)],
        ),
    };
    // One image and several are two texts, so that a language can say "it" and "them" its own way.
    let template = if files.len() == 1 {
        &labels.dialog_images_one
    } else {
        &labels.dialog_images_other
    };
    (
        labels.dialog_images_title.clone(),
        fill(template, &[("count", &count), ("stored", &stored)]),
    )
}

/// Runs an import of `source` into the destination of `options` and answers the core's report
/// with the shell's own warnings appended. `remote` is asked about, and fetches, the files the
/// source keeps on a server.
///
/// # Errors
///
/// What the core answers (`Locked`, `NotFound` for a destination that is gone, ...). A file that
/// cannot be read, uploaded or downloaded is a warning, not an error.
pub fn run(
    host: &dyn CoreHost,
    options: &ImportRunOptions,
    source: ImportSource,
    remote: &dyn RemoteFiles,
) -> Result<ImportReport, ApiError> {
    options.check()?;
    let mut source = source;
    let import_id = begin(host, options, source.name())?;
    let outcome = run_session(host, import_id, options.space_id, &mut source, remote);
    if outcome.is_err() {
        abort(host, import_id);
    }
    // Removes the temporary files of nested zips.
    drop(source);
    match &outcome {
        Ok(report) => tracing::info!(
            pages = report.pages,
            attachments = report.attachments,
            properties = report.properties_created,
            spaces = report.space_ids.len(),
            warnings = report.warnings.len(),
            "import finished"
        ),
        Err(error) => tracing::warn!(code = ?error.code, "import failed"),
    }
    outcome
}

fn begin(
    host: &dyn CoreHost,
    options: &ImportRunOptions,
    source_name: Option<&str>,
) -> Result<u32, ApiError> {
    let source_name = source_name.map(str::to_owned);
    let request = match (options.destination, options.space_id) {
        (ImportDestination::Page, Some(space_id)) => Request::ImportBegin {
            space_id,
            parent_id: options.parent_id,
            source_name,
        },
        (ImportDestination::NewSpaces, _) => Request::ImportBeginSpaces { source_name },
        (ImportDestination::Page, None) => {
            return Err(ApiError::invalid(
                "spaceId",
                "an import into a page needs the space",
            ));
        }
    };
    match host.dispatch(request)? {
        Response::ImportStarted(import_id) => Ok(import_id),
        _ => Err(unexpected()),
    }
}

/// Drops the core's session; best effort, the error that made it necessary is what matters.
fn abort(host: &dyn CoreHost, import_id: u32) {
    if let Err(error) = host.dispatch(Request::ImportAbort { import_id }) {
        tracing::debug!(code = ?error.code, "an import session could not be aborted");
    }
}

fn run_session(
    host: &dyn CoreHost,
    import_id: u32,
    session_space: Option<SpaceId>,
    source: &mut ImportSource,
    remote: &dyn RemoteFiles,
) -> Result<ImportReport, ApiError> {
    let mut warnings = source.take_warnings();
    add_text_files(host, import_id, source, &mut warnings)?;
    let plan = plan(host, import_id, source.binary_paths())?;

    // Each file goes into the space the plan gives it (new spaces), else into the session's.
    let (mut uploaded, mut failed) = (0_u32, 0_u32);
    for (at, path) in plan.needed_files.iter().enumerate() {
        let space_id = plan.file_spaces.get(at).copied().or(session_space);
        let display = plan
            .file_names
            .get(at)
            .map(String::as_str)
            .filter(|name| !name.trim().is_empty());
        let outcome = match space_id {
            Some(space_id) => upload_needed(host, import_id, space_id, source, path, display)?,
            None => Upload::Failed,
        };
        match outcome {
            Upload::Reported => uploaded += 1,
            Upload::Failed => {
                failed += 1;
                warnings.push(ImportWarning {
                    path: path.clone(),
                    code: ImportWarningCode::AttachmentFailed,
                    detail: None,
                });
            }
        }
    }
    tracing::debug!(
        needed = plan.needed_files.len(),
        uploaded,
        failed,
        "needed files uploaded"
    );

    fetch_remote_files(host, import_id, &plan, remote, &mut warnings)?;

    let report = commit_all(host, import_id)?;
    Ok(merge_warnings(report, warnings))
}

// --- the remote files ---

/// Asks the person once, then downloads the plan's remote files (at most
/// [`MAX_PARALLEL_DOWNLOADS`] at a time), uploads each into its space and reports it under its
/// key. A download or an upload that fails is `AttachmentFailed` (path: the key, detail: the host
/// and, when known, why: `cloud.example.com: could not connect`)
/// and the import goes on; a locked core or a refused report ends it. Nothing is fetched when the
/// plan lists nothing or the person says no.
fn fetch_remote_files(
    host: &dyn CoreHost,
    import_id: u32,
    plan: &ImportPlanInfo,
    remote: &dyn RemoteFiles,
    warnings: &mut Vec<ImportWarning>,
) -> Result<(), ApiError> {
    if plan.remote_files.is_empty() {
        return Ok(());
    }
    if !remote.consent(&plan.remote_files) {
        tracing::debug!(
            files = plan.remote_files.len(),
            "remote files left as links: not agreed to"
        );
        return Ok(());
    }
    let (mut uploaded, mut failed) = (0_u32, 0_u32);
    download_all(remote, &plan.remote_files, |file, fetched| {
        let attachment_id = match fetched.and_then(|image| upload_image(host, file.space_id, image))
        {
            Ok(attachment_id) => attachment_id,
            Err(error) if error.code == ErrorCode::Locked => return Err(error),
            Err(error) => {
                let why = failure_reason(&error);
                tracing::warn!(code = ?error.code, reason = why.unwrap_or("none"), "a remote file of the import could not be fetched");
                failed += 1;
                let host = shown_host(&file.host);
                warnings.push(ImportWarning {
                    path: file.key.clone(),
                    code: ImportWarningCode::AttachmentFailed,
                    detail: Some(match why {
                        Some(why) => format!("{host}: {why}"),
                        None => host,
                    }),
                });
                return Ok(());
            }
        };
        match host.dispatch(Request::ImportAddAttachment {
            import_id,
            path: file.key.clone(),
            attachment_id,
        })? {
            Response::Ok => {
                uploaded += 1;
                Ok(())
            }
            _ => Err(unexpected()),
        }
    })?;
    tracing::debug!(
        remote = plan.remote_files.len(),
        uploaded,
        failed,
        "remote files fetched"
    );
    Ok(())
}

/// Fetches every file with at most [`MAX_PARALLEL_DOWNLOADS`] downloads at a time and hands each
/// result, as it arrives, to `handle` on the calling thread (which does the uploads, so the core
/// is only ever used from one thread). An error from `handle` stops the downloads that have not
/// started and is returned once the running ones have ended.
fn download_all<F>(
    remote: &dyn RemoteFiles,
    files: &[ImportRemoteFile],
    mut handle: F,
) -> Result<(), ApiError>
where
    F: FnMut(&ImportRemoteFile, Result<RemoteImage, ApiError>) -> Result<(), ApiError>,
{
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::sync_channel::<(usize, Result<RemoteImage, ApiError>)>(0);
    let mut outcome = Ok(());
    std::thread::scope(|scope| {
        for _ in 0..MAX_PARALLEL_DOWNLOADS.min(files.len()) {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || {
                loop {
                    let at = next.fetch_add(1, Ordering::SeqCst);
                    let Some(file) = files.get(at) else {
                        break;
                    };
                    let fetched = remote.fetch(&file.url);
                    if sender.send((at, fetched)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (at, fetched) in &receiver {
            let Some(file) = files.get(at) else {
                continue;
            };
            if let Err(error) = handle(file, fetched) {
                outcome = Err(error);
                // Nothing new starts; the downloads running now end and are dropped.
                next.store(files.len(), Ordering::SeqCst);
                break;
            }
        }
        // A worker blocked in `send` wakes with an error once the receiver is gone.
        drop(receiver);
    });
    outcome
}

/// A downloaded picture as an attachment of `space_id`: `AttachmentBegin`, the bytes in pieces,
/// `AttachmentFinish`; aborted when anything fails.
fn upload_image(
    host: &dyn CoreHost,
    space_id: SpaceId,
    image: RemoteImage,
) -> Result<AttachmentId, ApiError> {
    let name = names::display_name(&image.name);
    let attachment_id = attachments::begin(host, space_id, name, image.mime)?;
    let uploaded = attachments::stream_pieces(
        host,
        attachment_id,
        &mut Cursor::new(image.bytes),
        MAX_ATTACHMENT_BYTES,
    )
    .and_then(|()| attachments::finish(host, attachment_id));
    match uploaded {
        Ok(info) => Ok(info.attachment_id),
        Err(error) => {
            attachments::abort(host, attachment_id);
            Err(error)
        }
    }
}

// --- the text files ---

/// Adds every text file to the session, one at a time (read, decode, send), within the limits.
fn add_text_files(
    host: &dyn CoreHost,
    import_id: u32,
    source: &mut ImportSource,
    warnings: &mut Vec<ImportWarning>,
) -> Result<(), ApiError> {
    let (mut count, mut total) = (0_usize, 0_u64);
    let mut truncated = false;
    for file in source.text_files() {
        if count >= MAX_TEXT_FILES || total.saturating_add(file.size()) > MAX_TEXT_TOTAL_BYTES {
            truncated = true;
            continue;
        }
        let (text, lossy) = match source.read_text(&file) {
            TextRead::Text { text, lossy } => (text, lossy),
            TextRead::TooLarge => {
                warnings.push(warning_of(&file, ImportWarningCode::TooLarge, None));
                continue;
            }
            TextRead::Unreadable => {
                warnings.push(warning_of(
                    &file,
                    ImportWarningCode::UnsupportedFile,
                    Some("unreadable"),
                ));
                continue;
            }
        };
        if total.saturating_add(text.len() as u64) > MAX_TEXT_TOTAL_BYTES {
            truncated = true;
            continue;
        }
        total += text.len() as u64;
        count += 1;
        if lossy {
            warnings.push(warning_of(&file, ImportWarningCode::NotUtf8, None));
        }
        match host.dispatch(Request::ImportAddFile {
            import_id,
            path: file.path().to_owned(),
            text,
        })? {
            Response::Ok => {}
            _ => return Err(unexpected()),
        }
    }
    if truncated {
        warnings.push(ImportWarning {
            path: String::new(),
            code: ImportWarningCode::Truncated,
            detail: Some("files".to_owned()),
        });
    }
    tracing::debug!(files = count, bytes = total, "text files added");
    Ok(())
}

fn warning_of(file: &SourceFile, code: ImportWarningCode, detail: Option<&str>) -> ImportWarning {
    ImportWarning {
        path: file.path().to_owned(),
        code,
        detail: detail.map(str::to_owned),
    }
}

// --- the plan and the files it needs ---

fn plan(
    host: &dyn CoreHost,
    import_id: u32,
    binary_paths: Vec<String>,
) -> Result<ImportPlanInfo, ApiError> {
    match host.dispatch(Request::ImportPlan {
        import_id,
        binary_paths,
    })? {
        Response::ImportPlanned(plan) => Ok(plan),
        _ => Err(unexpected()),
    }
}

/// What became of a needed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Upload {
    /// Uploaded and reported to the session.
    Reported,
    /// Could not be uploaded (too large, unreadable, refused): a warning, not reported.
    Failed,
}

/// Uploads a file the plan needs as an attachment of the space and reports it to the session. A
/// file that cannot be uploaded is [`Upload::Failed`]; a locked core is the end of the import, and
/// so is the session refusing the report.
fn upload_needed(
    host: &dyn CoreHost,
    import_id: u32,
    space_id: SpaceId,
    source: &mut ImportSource,
    path: &str,
    display: Option<&str>,
) -> Result<Upload, ApiError> {
    let attachment_id = match upload(host, space_id, source, path, display, MAX_ATTACHMENT_BYTES) {
        Ok(attachment_id) => attachment_id,
        Err(error) if error.code == ErrorCode::Locked => return Err(error),
        Err(error) => {
            tracing::warn!(code = ?error.code, "a file of the import could not be uploaded");
            return Ok(Upload::Failed);
        }
    };
    match host.dispatch(Request::ImportAddAttachment {
        import_id,
        path: path.to_owned(),
        attachment_id,
    })? {
        Response::Ok => Ok(Upload::Reported),
        _ => Err(unexpected()),
    }
}

/// The name an uploaded file gets and its type: the name the plan gives it (`file_names`, for a
/// source that stores files under another name, an AppFlowy backup by content hash), else the
/// last segment of the path, made safe as every attachment name is; the type comes from the
/// extension of that name, else of the path's.
fn attachment_name(path: &str, display: Option<&str>) -> (String, String) {
    let path_name = path.rsplit('/').next().unwrap_or(path);
    let name = names::display_name(display.unwrap_or(path_name));
    let mut mime = names::mime_for_name(&name);
    if mime == names::UNKNOWN_MIME {
        mime = names::mime_for_name(path_name);
    }
    (name, mime.to_owned())
}

/// How many first bytes of a file whose name gives no type are read to recognise a picture.
const SNIFF_BYTES: usize = 64;

/// A file whose name gives no known type (an AppFlowy backup stores pictures by content hash,
/// sometimes without an extension): when its first bytes are a picture's, the picture's type, and
/// its extension added to the name. Otherwise the name and type stay as they are.
fn sniffed_name(name: String, mime: String, head: &[u8]) -> (String, String) {
    if mime != names::UNKNOWN_MIME {
        return (name, mime);
    }
    match url_import::sniff_image(head, None) {
        Some(kind) => (
            format!("{name}.{}", kind.extension()),
            kind.mime().to_owned(),
        ),
        None => (name, mime),
    }
}

/// `AttachmentBegin`, the file in `CHUNK_SIZE` pieces, `AttachmentFinish`; the upload is aborted
/// when anything fails. The name is `display` or the last segment of the path (see
/// [`attachment_name`]); a name without a known type gets the one its first bytes show
/// ([`sniffed_name`]). A file above `max_bytes` (an attachment's limit, 2 GiB) is refused
/// without a byte read or a request made, and one that grows past it while it is read aborts the
/// upload.
fn upload(
    host: &dyn CoreHost,
    space_id: SpaceId,
    source: &mut ImportSource,
    path: &str,
    display: Option<&str>,
    max_bytes: u64,
) -> Result<kb_types::AttachmentId, ApiError> {
    source.with_binary(path, |reader, size| {
        if size > max_bytes {
            return Err(limit_exceeded(
                "size",
                "the file is larger than an attachment may be",
                max_bytes,
            ));
        }
        let (name, mime) = attachment_name(path, display);
        // The first bytes are read once, for the type, and sent ahead of the rest.
        let mut head = Vec::with_capacity(SNIFF_BYTES);
        (&mut *reader)
            .take(SNIFF_BYTES as u64)
            .read_to_end(&mut head)
            .map_err(|_| ApiError::storage("the file could not be read"))?;
        let (name, mime) = sniffed_name(name, mime, &head);
        let attachment_id = attachments::begin(host, space_id, name, mime)?;
        let mut whole = Cursor::new(head).chain(&mut *reader);
        let uploaded = attachments::stream_pieces(host, attachment_id, &mut whole, max_bytes)
            .and_then(|()| attachments::finish(host, attachment_id));
        match uploaded {
            Ok(info) => Ok(info.attachment_id),
            Err(error) => {
                attachments::abort(host, attachment_id);
                Err(error)
            }
        }
    })
}

// --- the commit ---

/// `ImportCommit` until the core says `done`, and its report.
fn commit_all(host: &dyn CoreHost, import_id: u32) -> Result<ImportReport, ApiError> {
    let mut last_created: Option<u32> = None;
    let mut stalled = 0_u32;
    loop {
        let progress: ImportProgress = match host.dispatch(Request::ImportCommit {
            import_id,
            max_pages: COMMIT_PAGES,
        })? {
            Response::ImportProgress(progress) => progress,
            _ => return Err(unexpected()),
        };
        if progress.done {
            return progress.report.ok_or_else(|| {
                ApiError::internal("the core finished the import without a report")
            });
        }
        if last_created == Some(progress.created) {
            stalled += 1;
            if stalled >= MAX_STALLED_COMMITS {
                return Err(ApiError::internal("the import stopped making progress"));
            }
        } else {
            stalled = 0;
        }
        last_created = Some(progress.created);
    }
}

/// The core's report with the shell's warnings after its own, at most
/// [`MAX_REPORT_WARNINGS`] listed and the rest counted in `more_warnings`.
fn merge_warnings(mut report: ImportReport, shell: Vec<ImportWarning>) -> ImportReport {
    for warning in shell {
        if report.warnings.len() < MAX_REPORT_WARNINGS {
            report.warnings.push(warning);
        } else {
            report.more_warnings = report.more_warnings.saturating_add(1);
        }
    }
    report
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_file_without_a_known_type_gets_the_type_its_first_bytes_show() {
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
        ];
        let unknown = names::UNKNOWN_MIME.to_owned();
        assert_eq!(
            sniffed_name("abc".into(), unknown.clone(), &jpeg),
            ("abc.jpg".to_owned(), "image/jpeg".to_owned())
        );
        // A known type is kept; bytes that are no picture leave the name alone.
        assert_eq!(
            sniffed_name("a.pdf".into(), "application/pdf".into(), &jpeg),
            ("a.pdf".to_owned(), "application/pdf".to_owned())
        );
        assert_eq!(
            sniffed_name("notes".into(), unknown.clone(), b"plain text"),
            ("notes".to_owned(), unknown)
        );
    }

    #[test]
    fn a_failed_download_says_why_without_the_address() {
        let network = ApiError::invalid("url", "the server could not be reached")
            .with_details(serde_json::json!({ "field": "url", "reason": "network" }));
        assert_eq!(failure_reason(&network), Some("could not connect"));
        let forbidden = ApiError::invalid("url", "error")
            .with_details(serde_json::json!({ "field": "url", "reason": "status", "status": 403 }));
        assert_eq!(
            failure_reason(&forbidden),
            Some("access refused by the server")
        );
        let plain = ApiError::invalid("url", "the server could not be reached");
        assert_eq!(failure_reason(&plain), None);
        let big = limit_exceeded("size", "too big", 42);
        assert_eq!(failure_reason(&big), Some("too large"));
    }

    fn report(warnings: usize, more: u32) -> ImportReport {
        ImportReport {
            root_page_ids: Vec::new(),
            pages: 1,
            attachments: 0,
            properties_created: 0,
            warnings: (0..warnings)
                .map(|index| ImportWarning {
                    path: format!("core-{index}"),
                    code: ImportWarningCode::UnresolvedLink,
                    detail: None,
                })
                .collect(),
            more_warnings: more,
            space_ids: Vec::new(),
        }
    }

    fn shell(count: usize) -> Vec<ImportWarning> {
        (0..count)
            .map(|index| ImportWarning {
                path: format!("shell-{index}"),
                code: ImportWarningCode::AttachmentFailed,
                detail: None,
            })
            .collect()
    }

    /// A source with one binary file, `name`, holding `bytes`.
    fn source_of(name: &str, bytes: &[u8]) -> (tempfile::TempDir, ImportSource) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(name), bytes).unwrap();
        let source = ImportSource::from_folder(dir.path()).unwrap();
        (dir, source)
    }

    fn space() -> SpaceId {
        SpaceId::from_bytes([9; 16])
    }

    #[test]
    fn a_file_above_the_attachment_limit_is_refused_before_anything_is_asked_of_the_core() {
        let (state, spy) = crate::test_support::ready_attachment_state();
        let (_dir, mut source) = source_of("big.bin", b"123456");

        let error = upload(&state, space(), &mut source, "big.bin", None, 5).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        let details = error.details.unwrap();
        assert_eq!(
            (details["field"].as_str(), details["limit"].as_u64()),
            (Some("size"), Some(5))
        );
        assert!(spy.lock().unwrap().calls.is_empty());
    }

    #[test]
    fn a_file_at_the_limit_goes_up_named_and_typed() {
        let (state, spy) = crate::test_support::ready_attachment_state();
        let (_dir, mut source) = source_of("pic.png", b"12345");

        let id = upload(&state, space(), &mut source, "pic.png", None, 5).unwrap();

        let spy = spy.lock().unwrap();
        assert_eq!(spy.calls, ["begin", "write:5", "finish"]);
        let stored = &spy.stored[&id];
        assert_eq!(
            (stored.name.as_str(), stored.mime.as_str()),
            ("pic.png", "image/png")
        );
        assert_eq!(stored.bytes, b"12345");
    }

    #[test]
    fn a_path_the_source_does_not_list_is_not_found_and_starts_nothing() {
        let (state, spy) = crate::test_support::ready_attachment_state();
        let (_dir, mut source) = source_of("a.bin", b"x");
        let error = upload(&state, space(), &mut source, "b.bin", None, 5).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(spy.lock().unwrap().calls.is_empty());
    }

    #[test]
    fn the_shell_warnings_come_after_the_cores() {
        let merged = merge_warnings(report(2, 0), shell(2));
        let paths: Vec<&str> = merged.warnings.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(paths, ["core-0", "core-1", "shell-0", "shell-1"]);
        assert_eq!(merged.more_warnings, 0);
    }

    #[test]
    fn warnings_past_500_are_counted_not_listed() {
        let merged = merge_warnings(report(MAX_REPORT_WARNINGS - 1, 7), shell(3));
        assert_eq!(merged.warnings.len(), MAX_REPORT_WARNINGS);
        assert_eq!(merged.warnings.last().unwrap().path, "shell-0");
        assert_eq!(merged.more_warnings, 9);

        let full = merge_warnings(report(MAX_REPORT_WARNINGS, 0), shell(2));
        assert_eq!(full.warnings.len(), MAX_REPORT_WARNINGS);
        assert_eq!(full.more_warnings, 2);
    }

    // --- names of needed files ---

    #[test]
    fn a_needed_file_is_named_by_the_plan_or_by_its_path() {
        // No display name: the last segment of the path, typed by its extension.
        assert_eq!(
            attachment_name("Backup/local_files/hash.png", None),
            ("hash.png".to_owned(), "image/png".to_owned())
        );
        // The plan's name wins (a backup stores files by content hash), made safe as any
        // attachment name is; the type follows its extension.
        assert_eq!(
            attachment_name("local_files/hash.bin", Some("Quarterly report.pdf")),
            (
                "Quarterly report.pdf".to_owned(),
                "application/pdf".to_owned()
            )
        );
        assert_eq!(
            attachment_name("local_files/hash.png", Some("a/b\\c\u{7}.PNG")),
            ("abc.PNG".to_owned(), "image/png".to_owned())
        );
        // A name without an extension takes the type of the path's.
        assert_eq!(
            attachment_name("local_files/hash.png", Some("Holiday")),
            ("Holiday".to_owned(), "image/png".to_owned())
        );
        // Neither knows it.
        assert_eq!(
            attachment_name("local_files/hash.zzz", Some("Thing")).1,
            names::UNKNOWN_MIME
        );
    }

    #[test]
    fn a_display_name_names_the_attachment() {
        let (state, spy) = crate::test_support::ready_attachment_state();
        let (_dir, mut source) = source_of("hash.bin", b"12345");

        let id = upload(
            &state,
            space(),
            &mut source,
            "hash.bin",
            Some("Plan.pdf"),
            5,
        )
        .unwrap();

        let spy = spy.lock().unwrap();
        let stored = &spy.stored[&id];
        assert_eq!(
            (stored.name.as_str(), stored.mime.as_str()),
            ("Plan.pdf", "application/pdf")
        );
    }

    // --- the question about remote files ---

    fn remote(host: &str) -> ImportRemoteFile {
        ImportRemoteFile {
            key: "remote/0".to_owned(),
            url: format!("https://{host}/images/a.png"),
            host: host.to_owned(),
            space_id: space(),
        }
    }

    fn files_on(hosts: &[(&str, usize)]) -> Vec<ImportRemoteFile> {
        hosts
            .iter()
            .flat_map(|(host, count)| (0..*count).map(|_| remote(host)))
            .collect()
    }

    #[test]
    fn the_question_names_how_many_images_and_where_they_are() {
        let (title, message) =
            consent_text(&Labels::default(), &files_on(&[("cloud.example.com", 28)]));
        assert_eq!(title, "Download images?");
        assert_eq!(
            message,
            "This import has 28 images stored on cloud.example.com. Download them now? atomik-kb \
             will connect to those servers once; nothing else is sent."
        );

        // The host with most of them, and how many others there are.
        let (_, message) = consent_text(
            &Labels::default(),
            &files_on(&[
                ("small.example.net", 3),
                ("big.example.com", 20),
                ("other.example.org", 5),
            ]),
        );
        assert_eq!(
            message,
            "This import has 28 images stored on big.example.com (and 2 other hosts). Download \
             them now? atomik-kb will connect to those servers once; nothing else is sent."
        );
        let (_, message) = consent_text(
            &Labels::default(),
            &files_on(&[("a.example.com", 2), ("b.example.com", 2)]),
        );
        assert!(
            message.contains("stored on a.example.com (and 1 other host). Download them"),
            "the first host wins a tie: {message}"
        );

        let (_, message) = consent_text(&Labels::default(), &files_on(&[("one.example.com", 1)]));
        assert_eq!(
            message,
            "This import has 1 image stored on one.example.com. Download it now? atomik-kb will \
             connect to those servers once; nothing else is sent."
        );
    }

    #[test]
    fn a_host_is_shown_without_control_characters_and_cut() {
        let long = format!("{}.example.com", "a".repeat(300));
        let (_, message) = consent_text(
            &Labels::default(),
            &files_on(&[("evil\u{7}\nhost.example.com", 1)]),
        );
        assert!(
            message.contains("stored on evilhost.example.com."),
            "{message}"
        );
        let (_, message) = consent_text(&Labels::default(), &files_on(&[(long.as_str(), 1)]));
        assert!(message.contains(&"a".repeat(HOST_CHARS)));
        assert!(!message.contains(&"a".repeat(HOST_CHARS + 1)));
    }

    /// A question answered by the test.
    struct Answer {
        says: bool,
        asked: std::sync::Mutex<Vec<Question>>,
        words: Labels,
    }

    impl Confirm for Answer {
        fn labels(&self) -> Labels {
            self.words.clone()
        }

        fn ask(&self, question: &Question) -> bool {
            self.asked.lock().unwrap().push(question.clone());
            self.says
        }
    }

    #[test]
    fn the_real_remote_asks_a_yes_no_question_with_that_text() {
        for says in [true, false] {
            let answer = Arc::new(Answer {
                says,
                asked: std::sync::Mutex::default(),
                words: Labels::default(),
            });
            let remote = DownloadAfterConsent::new(answer.clone(), FetchPolicy::production());
            let files = files_on(&[("cloud.example.com", 3)]);

            assert_eq!(remote.consent(&files), says);

            let asked = answer.asked.lock().unwrap();
            assert_eq!(asked.len(), 1);
            let (title, message) = consent_text(&Labels::default(), &files);
            assert_eq!(
                asked[0],
                Question {
                    title,
                    message,
                    accept: "Yes".to_owned(),
                    cancel: "No".to_owned(),
                }
            );
        }
    }

    #[test]
    fn the_question_is_written_in_the_words_the_ui_sent() {
        let french = Labels {
            dialog_images_title: "T\u{e9}l\u{e9}charger les images ?".to_owned(),
            dialog_images_one: "Cette importation contient {count} image h\u{e9}berg\u{e9}e sur {stored}. La t\u{e9}l\u{e9}charger maintenant ?".to_owned(),
            dialog_images_other: "Cette importation contient {count} images h\u{e9}berg\u{e9}es sur {stored}. Les t\u{e9}l\u{e9}charger maintenant ?".to_owned(),
            dialog_hosts_one: "{host} (et {count} autre h\u{f4}te)".to_owned(),
            dialog_hosts_other: "{host} (et {count} autres h\u{f4}tes)".to_owned(),
            dialog_yes: "Oui".to_owned(),
            dialog_no: "Non".to_owned(),
            ..Labels::default()
        };
        let (title, message) = consent_text(&french, &files_on(&[("cloud.example.com", 1)]));
        assert_eq!(title, "T\u{e9}l\u{e9}charger les images ?");
        assert_eq!(
            message,
            "Cette importation contient 1 image h\u{e9}berg\u{e9}e sur cloud.example.com. La t\u{e9}l\u{e9}charger maintenant ?"
        );
        let (_, message) = consent_text(
            &french,
            &files_on(&[
                ("big.example.com", 20),
                ("a.example.com", 5),
                ("b.example.com", 1),
            ]),
        );
        assert_eq!(
            message,
            "Cette importation contient 26 images h\u{e9}berg\u{e9}es sur big.example.com (et 2 autres h\u{f4}tes). Les t\u{e9}l\u{e9}charger maintenant ?"
        );
        let (_, message) = consent_text(
            &french,
            &files_on(&[("big.example.com", 20), ("a.example.com", 5)]),
        );
        assert!(
            message.contains("sur big.example.com (et 1 autre h\u{f4}te)."),
            "{message}"
        );

        // And the buttons of the real question are the UI's too.
        let answer = Arc::new(Answer {
            says: true,
            asked: std::sync::Mutex::default(),
            words: french,
        });
        let remote = DownloadAfterConsent::new(answer.clone(), FetchPolicy::production());
        assert!(remote.consent(&files_on(&[("cloud.example.com", 3)])));
        let asked = answer.asked.lock().unwrap();
        assert_eq!(
            (asked[0].accept.as_str(), asked[0].cancel.as_str()),
            ("Oui", "Non")
        );
    }

    // --- the downloads ---

    /// Serves one picture and one page on a port of this machine; answers its base address.
    fn stub_server() -> String {
        use axum::Router;
        use axum::http::header;
        use axum::routing::get;

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(b"\0\0\0\rIHDR rest of the picture");
        let app = Router::new()
            .route(
                "/pics/Sunset.png",
                get(move || {
                    let body = png.clone();
                    async move { ([(header::CONTENT_TYPE, "image/png")], body) }
                }),
            )
            .route(
                "/pics/page",
                get(|| async { ([(header::CONTENT_TYPE, "text/html")], vec![b'x'; 4096]) }),
            );
        tauri::async_runtime::block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            format!("http://{address}")
        })
    }

    #[test]
    fn a_link_is_fetched_with_the_rules_of_image_from_a_link() {
        let base = stub_server();
        let answer = Arc::new(Answer {
            says: true,
            asked: std::sync::Mutex::default(),
            words: Labels::default(),
        });
        let policy = FetchPolicy::local_test(
            kb_types::attachments::MAX_PREVIEW_BYTES,
            Duration::from_secs(20),
        );
        let remote = DownloadAfterConsent::new(answer.clone(), policy);

        let image = remote.fetch(&format!("{base}/pics/Sunset.png")).unwrap();
        assert_eq!(
            (image.name.as_str(), image.mime.as_str()),
            ("Sunset.png", "image/png")
        );
        assert!(image.bytes.starts_with(b"\x89PNG"));

        // A page is not a picture; nothing is kept.
        let error = remote.fetch(&format!("{base}/pics/page")).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.details.unwrap()["reason"], "notImage");
        assert!(
            answer.asked.lock().unwrap().is_empty(),
            "fetching asks nothing"
        );

        // The shell's own policy takes https only: a plain http link is refused before any request.
        let strict = DownloadAfterConsent::new(answer, FetchPolicy::production());
        let error = strict
            .fetch(&format!("{base}/pics/Sunset.png"))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }

    /// A remote that takes a moment and counts how many fetches overlap.
    struct Slow {
        in_flight: AtomicUsize,
        most: AtomicUsize,
        fetched: AtomicUsize,
    }

    impl Slow {
        fn new() -> Self {
            Self {
                in_flight: AtomicUsize::new(0),
                most: AtomicUsize::new(0),
                fetched: AtomicUsize::new(0),
            }
        }
    }

    impl RemoteFiles for Slow {
        fn consent(&self, _: &[ImportRemoteFile]) -> bool {
            true
        }

        fn fetch(&self, _: &str) -> Result<RemoteImage, ApiError> {
            self.fetched.fetch_add(1, Ordering::SeqCst);
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(15));
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(RemoteImage {
                name: "a.png".to_owned(),
                mime: "image/png".to_owned(),
                bytes: vec![1],
            })
        }
    }

    fn many(count: usize) -> Vec<ImportRemoteFile> {
        (0..count)
            .map(|n| ImportRemoteFile {
                key: format!("remote/{n}"),
                url: format!("https://cloud.example.com/{n}.png"),
                host: "cloud.example.com".to_owned(),
                space_id: space(),
            })
            .collect()
    }

    #[test]
    fn downloads_run_four_at_a_time_and_every_file_is_handled_once() {
        let slow = Slow::new();
        let files = many(20);
        let mut handled: Vec<String> = Vec::new();

        download_all(&slow, &files, |file, fetched| {
            assert!(fetched.is_ok());
            handled.push(file.key.clone());
            Ok(())
        })
        .unwrap();

        handled.sort();
        let mut wanted: Vec<String> = files.iter().map(|file| file.key.clone()).collect();
        wanted.sort();
        assert_eq!(handled, wanted);
        let most = slow.most.load(Ordering::SeqCst);
        assert!(
            (2..=MAX_PARALLEL_DOWNLOADS).contains(&most),
            "{most} downloads at once"
        );
        assert_eq!(slow.fetched.load(Ordering::SeqCst), 20);
        // Nothing is left running.
        assert_eq!(slow.in_flight.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fewer_files_than_workers_and_no_files_are_fine() {
        let slow = Slow::new();
        download_all(&slow, &[], |_, _| panic!("nothing to handle")).unwrap();
        let mut count = 0;
        download_all(&slow, &many(2), |_, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn an_error_from_the_handler_stops_the_downloads_that_have_not_started() {
        let slow = Slow::new();
        let files = many(60);
        let mut seen = 0;

        let error = download_all(&slow, &files, |_, _| {
            seen += 1;
            Err(ApiError::locked())
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::Locked);
        assert_eq!(seen, 1, "the handler is not called again");
        let started = slow.fetched.load(Ordering::SeqCst);
        assert!(started < 60, "{started} downloads were started");
        assert_eq!(
            slow.in_flight.load(Ordering::SeqCst),
            0,
            "the running ones ended"
        );
    }
}
