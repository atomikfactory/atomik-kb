//! The disk half of the attachment commands (ADR-0020). The core is sans-IO: it takes
//! plaintext one piece at a time (`attachment_write`) and hands plaintext to a sink
//! (`attachment_read`). This module is the shell's side of that bargain: it moves bytes
//! between those calls and files, and nothing here looks inside them.
//!
//! - **Import** (`attachment_import_path`): `AttachmentBegin`, the file read in `CHUNK_SIZE`
//!   pieces into `attachment_write`, `AttachmentFinish`. The core lock is taken once per
//!   piece, never held while the file is read, so the editor stays responsive during a long
//!   import. Any failure after `AttachmentBegin` ends in `AttachmentAbort` and the original
//!   error is what the caller gets. The name comes from the file name
//!   ([`names::display_name`]) and the MIME type from its extension.
//! - **Save as** (`attachment_save_as`): the core's read streams into `<path>.tmp`, which
//!   replaces `path` only when everything is on disk (the backup's atomic write); the disk's
//!   reason wins over the core's vaguer error. The core is called once, under the lock, and
//!   verifies the plaintext hash as it goes.
//! - **Open** (`attachment_open`): a private plaintext copy under
//!   `<app cache dir>/open/<attachment id>/<sanitised name>`, then the system opener. The
//!   directory is owner-only (`0700`/`0600`) on Unix; on Windows the cache directory lies in
//!   the user's profile and inherits its access rules (nothing in the standard library sets
//!   an ACL, and the shell has no `unsafe`). [`clear_open_dir`] removes the whole directory
//!   when the app starts, and a file that cannot be opened is removed at once. Files that
//!   the system would *run* (`.exe`, `.bat`, `.lnk`, ...) are refused
//!   ([`names::runs_code`]).
//! - **Raw read** (`attachment_read`): the plaintext as one `Vec<u8>` sized from the info,
//!   at most `MAX_PREVIEW_BYTES` per answer (a longer attachment needs a byte range, and a
//!   range is capped the same way); every byte the core delivers is checked against the
//!   announced length.
//!
//! A stored name is never used as a path without [`names::path_safe_name`]. Logs carry sizes,
//! counts and error codes, never a name, a MIME type, a path or any content. Failures to
//! create, open or move a file the user chose are `InvalidInput` for the field `path`;
//! failures while reading or writing are `Storage`, and the messages carry the operating
//! system's reason, never a path.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{Mutex, PoisonError};

use kb_types::attachments::{CHUNK_SIZE, MAX_ATTACHMENT_BYTES, MAX_PREVIEW_BYTES};
use kb_types::{ApiError, AttachmentId, AttachmentInfo, ByteRange, Request, Response, SpaceId};
use serde::Serialize;

use crate::attachment_ipc::limit_exceeded;
use crate::attachment_names as names;
use crate::backup::{path_error, path_text, write_atomically_as, write_error};
use crate::state::{BackupSink, CoreHost};

/// The directory of private copies, inside the app cache directory.
pub const OPEN_DIR: &str = "open";

/// How a saved attachment is called in error messages ("the file cannot be created").
const SAVED_FILE: &str = "file";

/// How the private copy is called in error messages.
const COPY_FILE: &str = "temporary copy";

/// Between the core's pieces and a file.
const BUFFER_BYTES: usize = 64 * 1024;

/// Serialises the making of private copies, so that two "Open" clicks on the same attachment
/// cannot remove each other's file while it is being written.
static OPEN_LOCK: Mutex<()> = Mutex::new(());

/// What `attachment_save_as` returns: the attachment as the core describes it and the file
/// that was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSaved {
    /// What the core says it saved.
    pub info: AttachmentInfo,
    /// The file written, as text (the path chosen in the dialog, or the one the caller gave).
    pub path: String,
}

/// Whether the `path` argument of `attachment_import_path` and `attachment_save_as` is
/// honoured: in debug builds only (the dev app, the smoke scripts and the tests drive the
/// commands with paths). A release build always asks the user through the native dialog: an
/// explicit path comes from the page, and a compromised page could use it to read any file
/// the user can read into an attachment, or to write attacker-chosen content over any file
/// the user can write.
pub const EXPLICIT_PATHS_ALLOWED: bool = cfg!(debug_assertions);

/// What a release build says to a `path` it was given.
const CHOOSE_IN_DIALOG: &str = "choose the file in the dialog";

/// An explicit path from the webview, in the builds that accept one
/// ([`EXPLICIT_PATHS_ALLOWED`]; any other build refuses it with `InvalidInput` for the field
/// `path`, "choose the file in the dialog", and touches nothing). It must be absolute (a
/// relative one would depend on the working directory of the process, which means nothing to
/// a user) and, on Windows, on a drive: not a network share (`\\server\share`, which would
/// send the user's credentials to whoever the page names) and not a device
/// (`\\.\pipe\...`).
///
/// This is a trust boundary: the paths the native dialogs return are the user's choice, but
/// an explicit one comes from the page. `allowed` is [`EXPLICIT_PATHS_ALLOWED`] for the
/// commands, and a parameter so that both builds can be tested in one.
fn explicit_path_when(allowed: bool, text: &str) -> Result<PathBuf, ApiError> {
    if !allowed {
        return Err(ApiError::invalid("path", CHOOSE_IN_DIALOG));
    }
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(ApiError::invalid("path", "the path must be absolute"));
    }
    if let Some(Component::Prefix(prefix)) = path.components().next()
        && !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    {
        return Err(ApiError::invalid(
            "path",
            "the path must be on a local drive",
        ));
    }
    Ok(path)
}

// --- the raw read ---

/// The plaintext of an attachment, or of the half-open byte `range`, as one buffer: what the
/// raw `attachment_read` answers.
///
/// Without a range the attachment's size is asked first (`AttachmentGet`) and anything above
/// `MAX_PREVIEW_BYTES` is refused with `InvalidInput` for the field `size` and
/// `details.limit`, before a byte is decrypted. The buffer is reserved once, from the size
/// (or the range's length), and the core's pieces are checked against it: more bytes than
/// announced stop the read, fewer is an `Internal` error. `range` must be what
/// `attachment_ipc::byte_range` accepts (non-empty, at most `MAX_PREVIEW_BYTES` long); the
/// core refuses one that reaches past the end of the file.
pub fn read_bytes(
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
    range: Option<ByteRange>,
) -> Result<Vec<u8>, ApiError> {
    let announced = match range {
        Some(range) => range.end.saturating_sub(range.start),
        None => {
            let info = get(host, attachment_id)?;
            if info.size > MAX_PREVIEW_BYTES {
                return Err(limit_exceeded(
                    "size",
                    "the attachment is too large to read in one piece: ask for a byte range",
                    MAX_PREVIEW_BYTES,
                ));
            }
            info.size
        }
    };
    let capacity = usize::try_from(announced)
        .map_err(|_| ApiError::internal("the attachment is too large for this computer"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| ApiError::internal("there is not enough memory to read the attachment"))?;
    let mut sink = |piece: &[u8]| -> Result<(), ApiError> {
        if bytes.len() + piece.len() > capacity {
            return Err(ApiError::internal(
                "the attachment is longer than the core announced",
            ));
        }
        bytes.extend_from_slice(piece);
        Ok(())
    };
    host.attachment_read(attachment_id, range, &mut sink)?;
    if bytes.len() != capacity {
        return Err(ApiError::internal(
            "the attachment is shorter than the core announced",
        ));
    }
    Ok(bytes)
}

// --- import ---

/// Imports a file into `space_id`: from the explicit `path` (debug builds only, see
/// [`explicit_path_when`]) or, when there is none, from the file `choose` asks the user for (the
/// native open dialog; its cancellation is its error). Answers the attachment as the core
/// describes it.
pub fn import(
    host: &dyn CoreHost,
    space_id: SpaceId,
    path: Option<&str>,
    choose: impl FnOnce() -> Result<PathBuf, ApiError>,
) -> Result<AttachmentInfo, ApiError> {
    import_with(EXPLICIT_PATHS_ALLOWED, host, space_id, path, choose)
}

/// [`import`] with the explicit-path policy as a parameter.
fn import_with(
    explicit_paths_allowed: bool,
    host: &dyn CoreHost,
    space_id: SpaceId,
    path: Option<&str>,
    choose: impl FnOnce() -> Result<PathBuf, ApiError>,
) -> Result<AttachmentInfo, ApiError> {
    let path = match path {
        Some(text) => explicit_path_when(explicit_paths_allowed, text)?,
        None => choose()?,
    };
    import_from_file(host, space_id, &path)
}

/// Streams the file at `path` through `AttachmentBegin`, `attachment_write` and
/// `AttachmentFinish`; aborts the upload when anything fails.
pub fn import_from_file(
    host: &dyn CoreHost,
    space_id: SpaceId,
    path: &Path,
) -> Result<AttachmentInfo, ApiError> {
    import_with_limit(host, space_id, path, MAX_ATTACHMENT_BYTES)
}

/// [`import_from_file`] with the size limit as a parameter, so the tests need no 2 GiB file.
fn import_with_limit(
    host: &dyn CoreHost,
    space_id: SpaceId,
    path: &Path,
    max_bytes: u64,
) -> Result<AttachmentInfo, ApiError> {
    let outcome = upload_file(host, space_id, path, max_bytes);
    match &outcome {
        Ok(info) => {
            tracing::info!(
                bytes = info.size,
                chunks = info.chunk_count,
                "attachment imported"
            );
        }
        Err(error) => tracing::warn!(code = ?error.code, "attachment import failed"),
    }
    outcome
}

fn upload_file(
    host: &dyn CoreHost,
    space_id: SpaceId,
    path: &Path,
    max_bytes: u64,
) -> Result<AttachmentInfo, ApiError> {
    const CANNOT_OPEN: &str = "the file cannot be opened";
    let mut file = File::open(path).map_err(|error| path_error(CANNOT_OPEN, &error))?;
    let metadata = file
        .metadata()
        .map_err(|error| path_error(CANNOT_OPEN, &error))?;
    if !metadata.is_file() {
        return Err(ApiError::invalid("path", "the path is not a file"));
    }
    if metadata.len() > max_bytes {
        return Err(limit_exceeded(
            "size",
            "the file is larger than an attachment may be",
            max_bytes,
        ));
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = names::display_name(&file_name);
    let mime = names::mime_for_name(&name).to_owned();

    let attachment_id = begin(host, space_id, name, mime)?;
    let uploaded = stream_pieces(host, attachment_id, &mut file, max_bytes)
        .and_then(|()| finish(host, attachment_id));
    if uploaded.is_err() {
        abort(host, attachment_id);
    }
    uploaded
}

/// Reads `reader` in `CHUNK_SIZE` pieces into `attachment_write`, never more than
/// `max_bytes` in all (a file can grow while it is read).
pub(crate) fn stream_pieces(
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
    reader: &mut impl Read,
    max_bytes: u64,
) -> Result<(), ApiError> {
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    let mut total = 0_u64;
    let mut pieces = 0_u32;
    loop {
        let filled = fill(reader, &mut buffer)?;
        if filled == 0 {
            break;
        }
        total += filled as u64;
        if total > max_bytes {
            return Err(limit_exceeded(
                "size",
                "the file grew larger than an attachment may be while it was read",
                max_bytes,
            ));
        }
        host.attachment_write(attachment_id, &buffer[..filled])?;
        pieces = pieces.saturating_add(1);
        if filled < buffer.len() {
            break;
        }
    }
    tracing::debug!(bytes = total, pieces, "upload streamed");
    Ok(())
}

/// Reads until `buffer` is full or `reader` is at its end, retrying interrupted reads;
/// returns how many bytes it holds.
fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, ApiError> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(ApiError::storage(format!(
                    "the file could not be read: {error}"
                )));
            }
        }
    }
    Ok(filled)
}

// --- save as ---

/// Saves an attachment to a file: to the explicit `path` (debug builds only, see
/// [`explicit_path_when`]) or, when there is none, to the file `choose` asks the user for, given
/// the sanitised name to propose (the native save dialog; its cancellation is its error). The
/// attachment is looked up first, so an unknown one is `NotFound` before any dialog opens or
/// file is made.
pub fn save_as(
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
    path: Option<&str>,
    choose: impl FnOnce(&str) -> Result<PathBuf, ApiError>,
) -> Result<AttachmentSaved, ApiError> {
    save_as_with(EXPLICIT_PATHS_ALLOWED, host, attachment_id, path, choose)
}

/// [`save_as`] with the explicit-path policy as a parameter.
fn save_as_with(
    explicit_paths_allowed: bool,
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
    path: Option<&str>,
    choose: impl FnOnce(&str) -> Result<PathBuf, ApiError>,
) -> Result<AttachmentSaved, ApiError> {
    // Before the core is asked anything: a refused path touches nothing.
    let explicit = path
        .map(|text| explicit_path_when(explicit_paths_allowed, text))
        .transpose()?;
    let info = get(host, attachment_id)?;
    let path = match explicit {
        Some(path) => path,
        None => choose(&names::path_safe_name(&info.name, attachment_id))?,
    };
    save_to_file(host, attachment_id, &path)
}

/// Streams the whole attachment into `<path>.tmp` and renames it over `path` once it is
/// on disk; on any failure `path` is untouched and the temporary file is gone.
pub fn save_to_file(
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
    path: &Path,
) -> Result<AttachmentSaved, ApiError> {
    let outcome = write_atomically_as(SAVED_FILE, path, |sink| {
        read_into(host, attachment_id, sink)
    });
    match outcome {
        Ok(info) => {
            tracing::info!(bytes = info.size, "attachment saved");
            Ok(AttachmentSaved {
                info,
                path: path_text(path),
            })
        }
        Err(error) => {
            tracing::warn!(code = ?error.code, "attachment save failed");
            Err(error)
        }
    }
}

/// Streams the whole attachment into `put`, counting what arrives: it must be exactly the
/// size the core announces (the core has checked the hash, this catches a core that stops
/// early without saying so).
fn read_into(
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
    put: BackupSink<'_>,
) -> Result<AttachmentInfo, ApiError> {
    let mut written = 0_u64;
    let mut forward = |piece: &[u8]| -> Result<(), ApiError> {
        put(piece).map_err(ApiError::storage)?;
        written += piece.len() as u64;
        Ok(())
    };
    let info = host.attachment_read(attachment_id, None, &mut forward)?;
    if written != info.size {
        return Err(ApiError::internal(
            "the attachment came back with a different size than the core announced",
        ));
    }
    Ok(info)
}

// --- open ---

/// Makes a private plaintext copy of an attachment under
/// `<cache_dir>/open/<attachment id>/<sanitised name>` and hands its path to `open` (the
/// system opener). Answers the path. A copy that could not be written, or that `open`
/// refused, is removed again at once; the rest is removed at the next start
/// ([`clear_open_dir`]).
///
/// An attachment whose file name says the system would run it is refused with `InvalidInput`
/// for the field `attachment`, before anything is written.
pub fn open_copy(
    host: &dyn CoreHost,
    cache_dir: &Path,
    attachment_id: AttachmentId,
    open: impl FnOnce(&Path) -> Result<(), ApiError>,
) -> Result<PathBuf, ApiError> {
    let outcome = make_and_open(host, cache_dir, attachment_id, open);
    match &outcome {
        Ok(_) => tracing::info!("attachment opened from a private copy"),
        Err(error) => tracing::warn!(code = ?error.code, "attachment could not be opened"),
    }
    outcome
}

fn make_and_open(
    host: &dyn CoreHost,
    cache_dir: &Path,
    attachment_id: AttachmentId,
    open: impl FnOnce(&Path) -> Result<(), ApiError>,
) -> Result<PathBuf, ApiError> {
    let info = get(host, attachment_id)?;
    let name = names::path_safe_name(&info.name, attachment_id);
    if names::runs_code(&name) {
        return Err(ApiError::invalid(
            "attachment",
            "files of this type are not opened from here: save the file and open it yourself",
        ));
    }
    let path = {
        let _one_at_a_time = OPEN_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        write_private_copy(host, cache_dir, attachment_id, &name)?
    };
    if let Err(error) = open(&path) {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

/// Writes the copy: the directory and file are created for the owner only where the
/// platform allows it, an earlier copy is removed first (a copy that is in use cannot be
/// replaced, and that is an error rather than a stale file), and a failed or partial copy is
/// removed.
fn write_private_copy(
    host: &dyn CoreHost,
    cache_dir: &Path,
    attachment_id: AttachmentId,
    name: &str,
) -> Result<PathBuf, ApiError> {
    let dir = cache_dir.join(OPEN_DIR).join(attachment_id.to_string());
    create_private_dirs(&dir).map_err(|error| {
        ApiError::storage(format!("the {COPY_FILE} cannot be created: {error}"))
    })?;
    let path = dir.join(name);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ApiError::storage(format!(
                "an earlier {COPY_FILE} is still in use: {error}"
            )));
        }
    }
    let file = create_private_file(&path).map_err(|error| {
        ApiError::storage(format!("the {COPY_FILE} cannot be created: {error}"))
    })?;

    let mut writer = BufWriter::with_capacity(BUFFER_BYTES, file);
    let mut failure: Option<io::Error> = None;
    let streamed = read_into(host, attachment_id, &mut |piece: &[u8]| {
        writer.write_all(piece).map_err(|error| {
            let message = error.to_string();
            failure = Some(error);
            message
        })
    });
    let outcome = match (failure, streamed) {
        (Some(error), _) => Err(write_error(COPY_FILE, &error)),
        (None, Err(error)) => Err(error),
        (None, Ok(_)) => writer
            .flush()
            .map_err(|error| write_error(COPY_FILE, &error)),
    };
    // Closed before it is removed: Windows will not delete a file that is open.
    drop(writer);
    if outcome.is_err() {
        let _ = fs::remove_file(&path);
    }
    outcome.map(|()| path)
}

/// Creates `dir` and its missing parents, owner-only (`0700`) on Unix.
fn create_private_dirs(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Creates a new file, owner-only (`0600`) on Unix; an existing one is an error, so what is
/// written to is always a file made here.
fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Removes every private copy: the whole `open` directory of the app cache directory. Run
/// when the app starts; a directory that is not there is fine, and a failure (a copy still
/// open in another program, say) is logged and does not stop the start.
pub fn clear_open_dir(cache_dir: &Path) {
    match fs::remove_dir_all(cache_dir.join(OPEN_DIR)) {
        Ok(()) => tracing::debug!("temporary attachment copies removed"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, "temporary attachment copies could not all be removed");
        }
    }
}

// --- typed requests to the core ---

fn unexpected() -> ApiError {
    ApiError::internal("the core answered an attachment request with something unexpected")
}

fn get(host: &dyn CoreHost, attachment_id: AttachmentId) -> Result<AttachmentInfo, ApiError> {
    match host.dispatch(Request::AttachmentGet { attachment_id })? {
        Response::Attachment(info) => Ok(info),
        _ => Err(unexpected()),
    }
}

pub(crate) fn begin(
    host: &dyn CoreHost,
    space_id: SpaceId,
    name: String,
    mime: String,
) -> Result<AttachmentId, ApiError> {
    match host.dispatch(Request::AttachmentBegin {
        space_id,
        name,
        mime,
    })? {
        Response::AttachmentStarted(attachment_id) => Ok(attachment_id),
        _ => Err(unexpected()),
    }
}

pub(crate) fn finish(
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
) -> Result<AttachmentInfo, ApiError> {
    match host.dispatch(Request::AttachmentFinish { attachment_id })? {
        Response::Attachment(info) => Ok(info),
        _ => Err(unexpected()),
    }
}

/// Drops an upload; best effort, the error that made it necessary is what matters.
pub(crate) fn abort(host: &dyn CoreHost, attachment_id: AttachmentId) {
    if let Err(error) = host.dispatch(Request::AttachmentAbort { attachment_id }) {
        tracing::warn!(code = ?error.code, "an upload could not be aborted");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::cell::Cell;

    use kb_types::ErrorCode;
    use tempfile::TempDir;

    use super::*;
    use crate::state::AppState;
    use crate::test_support::{AttachCore, SharedSpy, captured_logs, ready_attachment_state, seed};

    fn dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn space() -> SpaceId {
        SpaceId::from_bytes([9; 16])
    }

    fn field(error: &ApiError) -> Option<&str> {
        error.details.as_ref()?.get("field")?.as_str()
    }

    fn limit(error: &ApiError) -> Option<u64> {
        error.details.as_ref()?.get("limit")?.as_u64()
    }

    /// The calls the fake core received, in order ("begin", "write:5", "finish", ...).
    fn calls(spy: &SharedSpy) -> Vec<String> {
        spy.lock().unwrap().calls.clone()
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    fn write_file(dir: &TempDir, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path
    }

    fn no_dialog() -> Result<PathBuf, ApiError> {
        unreachable!("no dialog may open here")
    }

    fn no_dialog_with_name(_: &str) -> Result<PathBuf, ApiError> {
        unreachable!("no dialog may open here")
    }

    // --- explicit paths ---

    #[test]
    fn an_explicit_path_must_be_absolute() {
        let absolute = std::env::temp_dir().join("x.bin");
        assert_eq!(
            explicit_path_when(true, absolute.to_str().unwrap()).unwrap(),
            absolute
        );
        for relative in ["", "x.bin", "./x.bin", "..\\x.bin", "dir/x.bin", "~/x.bin"] {
            let error = explicit_path_when(true, relative).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{relative:?}");
            assert_eq!(field(&error), Some("path"), "{relative:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_only_drive_paths_are_explicit_paths() {
        for allowed in ["C:\\Users\\me\\x.bin", "\\\\?\\C:\\Users\\me\\x.bin"] {
            assert!(explicit_path_when(true, allowed).is_ok(), "{allowed}");
        }
        for refused in [
            "\\\\server\\share\\x.bin",
            "\\\\?\\UNC\\server\\share\\x.bin",
            "\\\\.\\pipe\\x",
            "\\\\.\\COM1",
            "\\\\?\\GLOBALROOT\\Device\\x",
            "C:x.bin",
            "\\x.bin",
        ] {
            let error = explicit_path_when(true, refused).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{refused}");
            assert_eq!(field(&error), Some("path"), "{refused}");
        }
    }

    #[test]
    fn explicit_paths_are_honoured_in_debug_builds_only() {
        // The policy of the build this test runs in.
        assert_eq!(EXPLICIT_PATHS_ALLOWED, cfg!(debug_assertions));
        let absolute = std::env::temp_dir().join("x.bin");
        let outcome = explicit_path_when(EXPLICIT_PATHS_ALLOWED, absolute.to_str().unwrap());
        if cfg!(debug_assertions) {
            assert_eq!(outcome.unwrap(), absolute);
        } else {
            assert_eq!(
                outcome.unwrap_err(),
                ApiError::invalid("path", "choose the file in the dialog")
            );
        }
    }

    #[test]
    fn a_release_build_refuses_any_given_path_whatever_it_is() {
        let absolute = std::env::temp_dir().join("x.bin");
        for text in [absolute.to_str().unwrap(), "", "relative.bin", "C:\\x"] {
            let error = explicit_path_when(false, text).unwrap_err();
            assert_eq!(
                error,
                ApiError::invalid("path", "choose the file in the dialog"),
                "{text:?}"
            );
        }
    }

    #[test]
    fn with_explicit_paths_refused_an_import_touches_nothing_and_never_opens_the_dialog() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "photo.png", b"plaintext");

        let error = import_with(false, &state, space(), path.to_str(), no_dialog).unwrap_err();

        assert_eq!(
            error,
            ApiError::invalid("path", "choose the file in the dialog")
        );
        assert!(calls(&spy).is_empty(), "the core was asked");
        assert_eq!(names_in(dir.path()), ["photo.png"]);
        assert_eq!(fs::read(&path).unwrap(), b"plaintext");

        // Without a path the dialog still decides, in every build.
        let chosen = path.clone();
        let info = import_with(false, &state, space(), None, || Ok(chosen)).unwrap();
        assert_eq!(info.name, "photo.png");
    }

    #[test]
    fn with_explicit_paths_refused_a_save_touches_nothing_and_never_opens_the_dialog() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"x");
        let dir = dir();
        let existing = write_file(&dir, "existing.txt", b"precious");
        let fresh = dir.path().join("fresh.txt");

        for path in [&existing, &fresh] {
            let error =
                save_as_with(false, &state, id, path.to_str(), no_dialog_with_name).unwrap_err();
            assert_eq!(
                error,
                ApiError::invalid("path", "choose the file in the dialog")
            );
        }

        assert!(calls(&spy).is_empty(), "the core was asked");
        assert_eq!(names_in(dir.path()), ["existing.txt"]);
        assert_eq!(fs::read(&existing).unwrap(), b"precious");

        // Without a path the dialog still decides, in every build.
        let saved = save_as_with(false, &state, id, None, |_| Ok(fresh.clone())).unwrap();
        assert_eq!(saved.path, fresh.to_str().unwrap());
        assert_eq!(fs::read(&fresh).unwrap(), b"x");
    }

    // --- the raw read ---

    #[test]
    fn a_whole_attachment_is_read_into_one_buffer_sized_from_its_info() {
        let (state, spy) = ready_attachment_state();
        let content = pattern(1000);
        let id = seed(&spy, "photo.png", "image/png", &content);

        let bytes = read_bytes(&state, id, None).unwrap();

        assert_eq!(bytes, content);
        // The size is asked first, then the plaintext is read.
        assert_eq!(calls(&spy), ["get", "read:full"]);
    }

    #[test]
    fn an_empty_attachment_reads_as_an_empty_buffer() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "empty.txt", "text/plain", b"");
        assert_eq!(read_bytes(&state, id, None).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn a_range_is_read_without_asking_for_the_size_first() {
        let (state, spy) = ready_attachment_state();
        let content = pattern(100);
        let id = seed(&spy, "a.bin", "application/octet-stream", &content);

        let bytes = read_bytes(&state, id, Some(ByteRange { start: 10, end: 25 })).unwrap();

        assert_eq!(bytes, &content[10..25]);
        assert_eq!(calls(&spy), ["read:10-25"]);
    }

    #[test]
    fn a_range_to_the_very_end_is_fine() {
        let (state, spy) = ready_attachment_state();
        let content = pattern(100);
        let id = seed(&spy, "a.bin", "application/octet-stream", &content);
        let bytes = read_bytes(
            &state,
            id,
            Some(ByteRange {
                start: 99,
                end: 100,
            }),
        )
        .unwrap();
        assert_eq!(bytes, &content[99..]);
    }

    #[test]
    fn an_attachment_above_the_preview_limit_is_refused_before_it_is_read() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "big.bin", "application/octet-stream", b"stand-in");
        spy.lock()
            .unwrap()
            .stored
            .get_mut(&id)
            .unwrap()
            .claimed_size = Some(MAX_PREVIEW_BYTES + 1);

        let error = read_bytes(&state, id, None).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("size"));
        assert_eq!(limit(&error), Some(MAX_PREVIEW_BYTES));
        assert_eq!(calls(&spy), ["get"], "nothing may be decrypted");
        // The same attachment is readable in ranges.
        let bytes = read_bytes(&state, id, Some(ByteRange { start: 0, end: 4 })).unwrap();
        assert_eq!(bytes, b"stan");
    }

    #[test]
    fn an_attachment_of_exactly_the_preview_limit_is_not_refused() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "big.bin", "application/octet-stream", b"stand-in");
        spy.lock()
            .unwrap()
            .stored
            .get_mut(&id)
            .unwrap()
            .claimed_size = Some(MAX_PREVIEW_BYTES);

        // The stand-in is shorter than announced, which is the other check's business.
        let error = read_bytes(&state, id, None).unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("shorter"), "{error}");
    }

    #[test]
    fn a_core_that_delivers_more_than_it_announced_is_stopped() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.bin", "application/octet-stream", &pattern(10));
        spy.lock()
            .unwrap()
            .stored
            .get_mut(&id)
            .unwrap()
            .claimed_size = Some(5);

        let error = read_bytes(&state, id, None).unwrap_err();

        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("longer"), "{error}");
    }

    #[test]
    fn a_core_that_delivers_less_than_it_announced_is_an_error_not_a_short_answer() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.bin", "application/octet-stream", &pattern(10));
        spy.lock()
            .unwrap()
            .stored
            .get_mut(&id)
            .unwrap()
            .claimed_size = Some(20);

        let error = read_bytes(&state, id, None).unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("shorter"), "{error}");
    }

    #[test]
    fn errors_of_the_core_pass_through_a_read_untouched() {
        let (state, spy) = ready_attachment_state();
        let unknown = AttachmentId::from_bytes([1; 16]);
        assert_eq!(
            read_bytes(&state, unknown, None).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            read_bytes(&state, unknown, Some(ByteRange { start: 0, end: 1 }))
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );

        let id = seed(&spy, "a.bin", "application/octet-stream", &pattern(10));
        let error = read_bytes(&state, id, Some(ByteRange { start: 5, end: 11 })).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("range"));

        spy.lock().unwrap().fail_read = Some(ApiError::crypto("a chunk does not open"));
        let error = read_bytes(&state, id, None).unwrap_err();
        assert_eq!(error, ApiError::crypto("a chunk does not open"));
    }

    #[test]
    fn a_locked_or_starting_core_answers_a_read_with_its_error() {
        let starting: AppState<AttachCore> = AppState::new();
        let id = AttachmentId::from_bytes([1; 16]);
        for range in [None, Some(ByteRange { start: 0, end: 1 })] {
            assert_eq!(
                read_bytes(&starting, id, range).unwrap_err(),
                ApiError::internal("core is starting")
            );
        }

        let (state, _spy) = ready_attachment_state();
        state.lock().unwrap();
        for range in [None, Some(ByteRange { start: 0, end: 1 })] {
            assert_eq!(
                read_bytes(&state, id, range).unwrap_err(),
                ApiError::locked()
            );
        }
    }

    // --- import ---

    #[test]
    fn a_file_is_imported_in_chunk_sized_pieces() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let content = pattern(2 * CHUNK_SIZE + 5);
        let path = write_file(&dir, "Holiday Photo.JPG", &content);

        let info = import(&state, space(), path.to_str(), no_dialog).unwrap();

        assert_eq!(
            calls(&spy),
            [
                "begin".to_owned(),
                format!("write:{CHUNK_SIZE}"),
                format!("write:{CHUNK_SIZE}"),
                "write:5".to_owned(),
                "finish".to_owned(),
            ]
        );
        let guard = spy.lock().unwrap();
        let stored = guard.stored.get(&info.attachment_id).unwrap();
        assert!(stored.bytes == content, "the stored bytes differ");
        assert_eq!(stored.name, "Holiday Photo.JPG");
        assert_eq!(stored.mime, "image/jpeg");
        assert_eq!(stored.space_id, space());
        assert!(guard.uploads.is_empty());
        drop(guard);
        assert_eq!(info.size, content.len() as u64);
        assert_eq!(info.space_id, space());
        assert_eq!(info.name, "Holiday Photo.JPG");
    }

    #[test]
    fn a_file_of_exactly_one_chunk_is_one_piece() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "exact.bin", &pattern(CHUNK_SIZE));
        import_from_file(&state, space(), &path).unwrap();
        assert_eq!(
            calls(&spy),
            ["begin", &format!("write:{CHUNK_SIZE}"), "finish"]
        );
    }

    #[test]
    fn a_file_of_two_whole_chunks_ends_without_an_empty_piece() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "two.bin", &pattern(2 * CHUNK_SIZE));
        import_from_file(&state, space(), &path).unwrap();
        assert_eq!(
            calls(&spy),
            [
                "begin",
                &format!("write:{CHUNK_SIZE}"),
                &format!("write:{CHUNK_SIZE}"),
                "finish"
            ]
        );
    }

    #[test]
    fn an_empty_file_is_begun_and_finished_without_a_write() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "empty.txt", b"");
        let info = import_from_file(&state, space(), &path).unwrap();
        assert_eq!(calls(&spy), ["begin", "finish"]);
        assert_eq!(info.size, 0);
    }

    #[test]
    fn the_mime_type_comes_from_the_extension() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        for (name, mime) in [
            ("a.png", "image/png"),
            ("b.pdf", "application/pdf"),
            ("c.unknownext", "application/octet-stream"),
            ("noextension", "application/octet-stream"),
        ] {
            let path = write_file(&dir, name, b"x");
            let info = import_from_file(&state, space(), &path).unwrap();
            let guard = spy.lock().unwrap();
            assert_eq!(guard.stored[&info.attachment_id].mime, mime, "{name}");
        }
    }

    #[test]
    fn a_name_the_core_would_refuse_is_made_acceptable_before_it_is_sent() {
        // On Unix a file name may hold a backslash; the core refuses those in names.
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let name = if cfg!(windows) { "a b.txt" } else { "a\\b.txt" };
        let path = write_file(&dir, name, b"x");
        let info = import_from_file(&state, space(), &path).unwrap();
        let sent = spy.lock().unwrap().stored[&info.attachment_id].name.clone();
        assert!(!sent.contains(['/', '\\', '\0']), "{sent:?}");
        assert!(sent.ends_with(".txt"));
    }

    #[test]
    fn a_failed_write_aborts_the_upload_and_returns_the_error() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "big.bin", &pattern(2 * CHUNK_SIZE + 1));
        spy.lock().unwrap().fail_write_on = Some(2);

        let error = import_from_file(&state, space(), &path).unwrap_err();

        assert_eq!(error.code, ErrorCode::Storage);
        assert_eq!(
            calls(&spy),
            [
                "begin".to_owned(),
                format!("write:{CHUNK_SIZE}"),
                format!("write:{CHUNK_SIZE}"),
                "abort".to_owned(),
            ]
        );
        let guard = spy.lock().unwrap();
        assert!(guard.uploads.is_empty(), "the upload is still there");
        assert!(guard.stored.is_empty());
    }

    #[test]
    fn a_failed_finish_aborts_the_upload_and_returns_the_error() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "a.bin", b"abc");
        spy.lock().unwrap().fail_finish = Some(ApiError::storage("the manifest was not written"));

        let error = import_from_file(&state, space(), &path).unwrap_err();

        assert_eq!(error, ApiError::storage("the manifest was not written"));
        assert_eq!(calls(&spy), ["begin", "write:3", "finish", "abort"]);
        assert!(spy.lock().unwrap().uploads.is_empty());
    }

    #[test]
    fn a_refused_begin_has_nothing_to_abort() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "a.bin", b"abc");
        spy.lock().unwrap().fail_begin = Some(ApiError::invalid("name", "bad name"));

        let error = import_from_file(&state, space(), &path).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("name"));
        assert_eq!(calls(&spy), ["begin"]);
    }

    #[test]
    fn a_file_that_is_not_there_never_starts_an_upload() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        for (path, why) in [
            (dir.path().join("missing.bin"), "missing"),
            (dir.path().to_path_buf(), "a directory"),
        ] {
            let error = import_from_file(&state, space(), &path).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{why}");
            assert_eq!(field(&error), Some("path"), "{why}");
        }
        assert!(calls(&spy).is_empty());
    }

    #[test]
    fn a_relative_explicit_path_is_refused_before_anything_happens() {
        let (state, spy) = ready_attachment_state();
        let error = import(&state, space(), Some("photo.png"), no_dialog).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(calls(&spy).is_empty());
    }

    #[test]
    fn without_a_path_the_dialog_chooses_the_file() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let chosen = write_file(&dir, "chosen.txt", b"hello");
        let asked = Cell::new(0);

        let info = import(&state, space(), None, || {
            asked.set(asked.get() + 1);
            Ok(chosen.clone())
        })
        .unwrap();

        assert_eq!(asked.get(), 1);
        assert_eq!(info.name, "chosen.txt");
        assert_eq!(calls(&spy), ["begin", "write:5", "finish"]);
    }

    #[test]
    fn a_cancelled_dialog_is_invalid_input_for_the_path_and_starts_nothing() {
        let (state, spy) = ready_attachment_state();
        let error = import(&state, space(), None, || {
            Err(ApiError::invalid("path", "no file was chosen"))
        })
        .unwrap_err();
        assert_eq!(error, ApiError::invalid("path", "no file was chosen"));
        assert!(calls(&spy).is_empty());
    }

    #[test]
    fn a_file_larger_than_an_attachment_may_be_is_refused_before_it_is_read() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let path = write_file(&dir, "a.bin", &pattern(11));

        let error = import_with_limit(&state, space(), &path, 10).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("size"));
        assert_eq!(limit(&error), Some(10));
        assert!(calls(&spy).is_empty());
        // At the limit it is accepted.
        assert_eq!(
            import_with_limit(&state, space(), &path, 11).unwrap().size,
            11
        );
    }

    #[test]
    fn a_file_that_grows_past_the_limit_while_it_is_read_aborts_the_upload() {
        let (state, spy) = ready_attachment_state();
        let begun = begin(
            &state,
            space(),
            "grow.bin".into(),
            "application/octet-stream".into(),
        )
        .unwrap();
        let mut reader = io::Cursor::new(pattern(CHUNK_SIZE + 10));

        let error = stream_pieces(&state, begun, &mut reader, (CHUNK_SIZE + 5) as u64).unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("size"));
        // The first whole piece went through; the one that crossed the limit did not.
        assert_eq!(
            calls(&spy),
            ["begin".to_owned(), format!("write:{CHUNK_SIZE}")]
        );
    }

    /// Hands out one byte at a time and interrupts every other read.
    struct Stuttering {
        data: Vec<u8>,
        position: usize,
        interrupt: bool,
    }

    impl Read for Stuttering {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.position == self.data.len() || buffer.is_empty() {
                return Ok(0);
            }
            buffer[0] = self.data[self.position];
            self.position += 1;
            Ok(1)
        }
    }

    #[test]
    fn short_and_interrupted_reads_still_fill_a_whole_piece() {
        let mut reader = Stuttering {
            data: pattern(10),
            position: 0,
            interrupt: false,
        };
        let mut buffer = [0_u8; 4];
        assert_eq!(fill(&mut reader, &mut buffer).unwrap(), 4);
        assert_eq!(buffer, [0, 1, 2, 3]);
        let mut rest = [0_u8; 16];
        assert_eq!(fill(&mut reader, &mut rest).unwrap(), 6);
        assert_eq!(&rest[..6], &[4, 5, 6, 7, 8, 9]);
        assert_eq!(fill(&mut reader, &mut rest).unwrap(), 0);
    }

    /// Fails like a disk that went away.
    struct Failing;

    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("the device is gone"))
        }
    }

    #[test]
    fn a_read_error_is_a_storage_error_with_the_reason() {
        let error = fill(&mut Failing, &mut [0_u8; 4]).unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
        assert!(error.message.contains("the device is gone"), "{error}");
        assert!(error.message.contains("could not be read"), "{error}");
    }

    #[test]
    fn a_locked_or_starting_core_refuses_an_import_after_the_file_is_checked_only() {
        let dir = dir();
        let path = write_file(&dir, "a.bin", b"abc");

        let starting: AppState<AttachCore> = AppState::new();
        let error = import_from_file(&starting, space(), &path).unwrap_err();
        assert_eq!(error, ApiError::internal("core is starting"));

        let (state, spy) = ready_attachment_state();
        state.lock().unwrap();
        let error = import_from_file(&state, space(), &path).unwrap_err();
        assert_eq!(error, ApiError::locked());
        assert!(calls(&spy).is_empty());
    }

    // --- save as ---

    #[test]
    fn an_attachment_is_saved_to_the_explicit_path() {
        let (state, spy) = ready_attachment_state();
        let content = pattern(1000);
        let id = seed(&spy, "photo.png", "image/png", &content);
        let dir = dir();
        let path = dir.path().join("saved.png");

        let saved = save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap();

        assert_eq!(fs::read(&path).unwrap(), content);
        assert_eq!(saved.path, path.to_str().unwrap());
        assert_eq!(saved.info.attachment_id, id);
        assert_eq!(saved.info.size, 1000);
        assert_eq!(saved.info.name, "photo.png");
        // The temporary file is gone: the file is the only thing left in the directory.
        assert_eq!(names_in(dir.path()), ["saved.png"]);
        assert_eq!(calls(&spy), ["get", "read:full"]);
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn an_empty_attachment_saves_as_an_empty_file() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "empty.txt", "text/plain", b"");
        let dir = dir();
        let path = dir.path().join("empty.txt");
        save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap();
        assert_eq!(fs::read(&path).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn saving_replaces_a_file_that_is_already_there() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"new");
        let dir = dir();
        let path = write_file(&dir, "a.txt", b"the old content, longer than the new one");

        save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(names_in(dir.path()), ["a.txt"]);
    }

    #[test]
    fn a_failed_read_leaves_the_old_file_and_no_temporary_one() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", &pattern(100));
        spy.lock().unwrap().fail_read = Some(ApiError::crypto("a chunk does not open"));
        let dir = dir();
        let path = write_file(&dir, "a.txt", b"precious");

        let error = save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap_err();

        assert_eq!(error, ApiError::crypto("a chunk does not open"));
        assert_eq!(fs::read(&path).unwrap(), b"precious");
        assert_eq!(names_in(dir.path()), ["a.txt"]);
    }

    #[test]
    fn a_failed_read_leaves_no_new_file_either() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", &pattern(100));
        spy.lock().unwrap().fail_read = Some(ApiError::storage("a chunk is missing"));
        let dir = dir();
        let path = dir.path().join("new.txt");

        let error = save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap_err();

        assert_eq!(error.code, ErrorCode::Storage);
        assert!(names_in(dir.path()).is_empty());
    }

    #[test]
    fn a_size_the_core_did_not_announce_is_not_saved() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", &pattern(10));
        spy.lock()
            .unwrap()
            .stored
            .get_mut(&id)
            .unwrap()
            .claimed_size = Some(11);
        let dir = dir();
        let path = dir.path().join("a.txt");

        let error = save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap_err();

        assert_eq!(error.code, ErrorCode::Internal);
        assert!(names_in(dir.path()).is_empty());
    }

    #[test]
    fn without_a_path_the_dialog_proposes_the_sanitised_name() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "../My: report?.pdf", "application/pdf", b"pdf");
        let dir = dir();
        let chosen = dir.path().join("chosen.pdf");
        let proposed = std::cell::RefCell::new(String::new());

        let saved = save_as(&state, id, None, |name| {
            *proposed.borrow_mut() = name.to_owned();
            Ok(chosen.clone())
        })
        .unwrap();

        assert_eq!(*proposed.borrow(), "..My_ report_.pdf");
        assert_eq!(saved.path, chosen.to_str().unwrap());
        assert_eq!(fs::read(&chosen).unwrap(), b"pdf");
    }

    #[test]
    fn a_name_that_leaves_nothing_is_proposed_as_the_attachment_id() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "..", "text/plain", b"x");
        let proposed = std::cell::RefCell::new(String::new());
        let dir = dir();
        let chosen = dir.path().join("x");
        save_as(&state, id, None, |name| {
            *proposed.borrow_mut() = name.to_owned();
            Ok(chosen.clone())
        })
        .unwrap();
        assert_eq!(*proposed.borrow(), id.to_string());
    }

    #[test]
    fn a_cancelled_save_dialog_reads_nothing_and_writes_nothing() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"x");
        let error = save_as(&state, id, None, |_| {
            Err(ApiError::invalid("path", "no file was chosen"))
        })
        .unwrap_err();
        assert_eq!(error, ApiError::invalid("path", "no file was chosen"));
        assert_eq!(calls(&spy), ["get"]);
    }

    #[test]
    fn an_unknown_attachment_is_not_found_before_any_dialog_opens() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let unknown = AttachmentId::from_bytes([5; 16]);
        let error = save_as(&state, unknown, None, no_dialog_with_name).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        let path = dir.path().join("x");
        let error = save_as(&state, unknown, path.to_str(), no_dialog_with_name).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(names_in(dir.path()).is_empty());
        assert_eq!(calls(&spy), ["get", "get"]);
    }

    #[test]
    fn a_bad_explicit_save_path_never_reaches_the_core() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"x");
        let before = calls(&spy);
        let error = save_as(&state, id, Some("relative.txt"), no_dialog_with_name).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert_eq!(calls(&spy), before);
    }

    #[test]
    fn a_directory_is_not_a_save_target_and_the_core_reads_nothing() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"x");
        let dir = dir();
        let error = save_as(&state, id, dir.path().to_str(), no_dialog_with_name).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(!calls(&spy).iter().any(|call| call.starts_with("read")));
    }

    #[test]
    fn a_path_in_a_missing_directory_is_invalid_input_for_the_path() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"x");
        let dir = dir();
        let path = dir.path().join("no-such-dir").join("a.txt");
        let error = save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(error.message.contains("cannot be created"), "{error}");
        // It says "file", not "backup file".
        assert!(!error.message.contains("backup"), "{error}");
    }

    #[test]
    fn a_locked_core_refuses_a_save_before_a_dialog_or_a_file() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"x");
        state.lock().unwrap();
        let dir = dir();
        let path = dir.path().join("a.txt");
        assert_eq!(
            save_as(&state, id, path.to_str(), no_dialog_with_name).unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(
            save_as(&state, id, None, no_dialog_with_name).unwrap_err(),
            ApiError::locked()
        );
        assert!(names_in(dir.path()).is_empty());
    }

    // --- open ---

    fn opened_with() -> (Cell<u32>, std::cell::RefCell<Option<PathBuf>>) {
        (Cell::new(0), std::cell::RefCell::new(None))
    }

    #[test]
    fn a_private_copy_is_written_under_the_open_directory_and_handed_to_the_opener() {
        let (state, spy) = ready_attachment_state();
        let content = pattern(5000);
        let id = seed(&spy, "Notes.txt", "text/plain", &content);
        let cache = dir();
        let (count, seen) = opened_with();

        let path = open_copy(&state, cache.path(), id, |path| {
            count.set(count.get() + 1);
            *seen.borrow_mut() = Some(path.to_path_buf());
            // The file is complete by the time the system is told about it.
            assert_eq!(fs::read(path).unwrap().len(), 5000);
            Ok(())
        })
        .unwrap();

        assert_eq!(count.get(), 1);
        assert_eq!(seen.borrow().as_deref(), Some(path.as_path()));
        assert_eq!(
            path,
            cache
                .path()
                .join("open")
                .join(id.to_string())
                .join("Notes.txt")
        );
        assert_eq!(fs::read(&path).unwrap(), content);
    }

    #[test]
    fn the_copy_never_leaves_its_directory_whatever_the_stored_name_says() {
        let (state, spy) = ready_attachment_state();
        let cache = dir();
        for name in [
            "../../escape.txt",
            "..\\..\\escape.txt",
            "C:\\Windows\\escape.txt",
            "C:escape.txt",
            "/etc/escape",
            "\\\\server\\share\\escape.txt",
            "stream.txt:hidden",
            "CON",
            "nul.txt",
            "..",
            "",
            "trailing. . .",
        ] {
            let id = seed(&spy, name, "text/plain", b"data");
            let path = open_copy(&state, cache.path(), id, |_| Ok(())).unwrap();
            let expected_dir = cache.path().join("open").join(id.to_string());
            assert_eq!(path.parent(), Some(expected_dir.as_path()), "{name:?}");
            assert!(path.is_file(), "{name:?}");
            assert_eq!(fs::read(&path).unwrap(), b"data", "{name:?}");
            // Nothing was written anywhere else.
            assert_eq!(names_in(&expected_dir).len(), 1, "{name:?}");
        }
        assert!(!cache.path().join("escape.txt").exists());
        assert_eq!(names_in(cache.path()), ["open"]);
    }

    #[test]
    fn a_file_the_system_would_run_is_not_opened_and_nothing_is_written() {
        let (state, spy) = ready_attachment_state();
        let cache = dir();
        for name in [
            "setup.exe",
            "RUN.BAT",
            "link.lnk",
            "x.ps1",
            "deploy.sh",
            "a.js",
        ] {
            let id = seed(&spy, name, "application/octet-stream", b"MZ");
            let (count, _) = opened_with();
            let error = open_copy(&state, cache.path(), id, |_| {
                count.set(count.get() + 1);
                Ok(())
            })
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{name}");
            assert_eq!(field(&error), Some("attachment"), "{name}");
            assert_eq!(count.get(), 0, "{name}");
        }
        assert!(names_in(cache.path()).is_empty(), "something was written");
        assert!(!calls(&spy).iter().any(|call| call.starts_with("read")));
    }

    #[test]
    fn opening_again_replaces_the_copy() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"content");
        let cache = dir();
        let first = open_copy(&state, cache.path(), id, |_| Ok(())).unwrap();
        fs::write(&first, b"edited by another program").unwrap();

        let second = open_copy(&state, cache.path(), id, |_| Ok(())).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(&second).unwrap(), b"content");
    }

    #[test]
    fn a_copy_that_fails_midway_is_removed_and_the_system_is_not_asked() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", &pattern(100));
        spy.lock().unwrap().fail_read = Some(ApiError::crypto("a chunk does not open"));
        let cache = dir();
        let (count, _) = opened_with();

        let error = open_copy(&state, cache.path(), id, |_| {
            count.set(count.get() + 1);
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error, ApiError::crypto("a chunk does not open"));
        assert_eq!(count.get(), 0);
        let id_dir = cache.path().join("open").join(id.to_string());
        assert!(names_in(&id_dir).is_empty(), "a partial copy is left");
    }

    #[test]
    fn a_copy_the_system_refuses_to_open_is_removed() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"abc");
        let cache = dir();

        let error = open_copy(&state, cache.path(), id, |_| {
            Err(ApiError::internal("the file could not be opened"))
        })
        .unwrap_err();

        assert_eq!(error, ApiError::internal("the file could not be opened"));
        let id_dir = cache.path().join("open").join(id.to_string());
        assert!(names_in(&id_dir).is_empty());
    }

    #[test]
    fn an_unknown_attachment_or_a_locked_core_writes_nothing() {
        let (state, spy) = ready_attachment_state();
        let cache = dir();
        let unknown = AttachmentId::from_bytes([7; 16]);
        let error = open_copy(&state, cache.path(), unknown, |_| Ok(())).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);

        let id = seed(&spy, "a.txt", "text/plain", b"abc");
        state.lock().unwrap();
        let error = open_copy(&state, cache.path(), id, |_| Ok(())).unwrap_err();
        assert_eq!(error, ApiError::locked());
        assert!(names_in(cache.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn on_unix_the_copy_and_its_directories_are_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;

        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "a.txt", "text/plain", b"abc");
        let cache = dir();
        let path = open_copy(&state, cache.path(), id, |_| Ok(())).unwrap();

        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&cache.path().join("open")), 0o700);
    }

    #[test]
    fn the_open_directory_is_removed_at_start_and_nothing_else() {
        let cache = dir();
        let copy = cache.path().join("open").join("some-id");
        fs::create_dir_all(&copy).unwrap();
        fs::write(copy.join("file.txt"), b"plaintext").unwrap();
        fs::write(cache.path().join("open").join("loose.txt"), b"x").unwrap();
        let blobs = cache.path().join("blobs");
        fs::create_dir_all(&blobs).unwrap();
        fs::write(blobs.join("keep.chunk"), b"ciphertext").unwrap();

        clear_open_dir(cache.path());

        assert_eq!(names_in(cache.path()), ["blobs"]);
        assert_eq!(fs::read(blobs.join("keep.chunk")).unwrap(), b"ciphertext");
    }

    #[test]
    fn clearing_a_cache_without_an_open_directory_is_fine() {
        let cache = dir();
        clear_open_dir(cache.path());
        clear_open_dir(&cache.path().join("does-not-exist"));
        assert!(names_in(cache.path()).is_empty());
    }

    // --- what is logged ---

    #[test]
    fn logs_carry_sizes_and_codes_and_never_names_types_paths_or_content() {
        let (state, spy) = ready_attachment_state();
        let dir = dir();
        let cache = self::dir();
        let secret_name = "SECRET-name-4471.pdf";
        let imported = write_file(&dir, secret_name, b"CONTENT-sentinel-9913");

        let logs = captured_logs(|| {
            let info = import_from_file(&state, space(), &imported).unwrap();
            // A failed import, a save, a failed save, an open and a refused open.
            spy.lock().unwrap().fail_begin = Some(ApiError::invalid("name", "SECRET-name-4471"));
            let _ = import_from_file(&state, space(), &imported).unwrap_err();
            spy.lock().unwrap().fail_begin = None;
            let saved = dir.path().join("SECRET-saved-1.pdf");
            save_as(
                &state,
                info.attachment_id,
                saved.to_str(),
                no_dialog_with_name,
            )
            .unwrap();
            let blocked = dir.path().join("no-dir").join("SECRET-saved-2.pdf");
            let _ = save_as(
                &state,
                info.attachment_id,
                blocked.to_str(),
                no_dialog_with_name,
            )
            .unwrap_err();
            open_copy(&state, cache.path(), info.attachment_id, |_| Ok(())).unwrap();
            let exe = seed(&spy, "SECRET-run-5.exe", "application/x-msdownload", b"MZ");
            let _ = open_copy(&state, cache.path(), exe, |_| Ok(())).unwrap_err();
            clear_open_dir(cache.path());
        });

        assert!(logs.contains("attachment imported"), "{logs}");
        assert!(logs.contains("attachment import failed"), "{logs}");
        assert!(logs.contains("attachment saved"), "{logs}");
        assert!(logs.contains("attachment save failed"), "{logs}");
        for secret in [
            "SECRET",
            "CONTENT",
            "sentinel",
            "application/pdf",
            "application/x-msdownload",
            ".pdf",
            ".exe",
            "no-dir",
            dir.path().to_str().unwrap(),
            cache.path().to_str().unwrap(),
        ] {
            assert!(!logs.contains(secret), "{secret} reached the log: {logs}");
        }
    }
}
