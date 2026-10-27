//! The disk half of `export_run` (ADR-0036, `docs/architecture/import-export.md`, 5.1). The core is
//! sans-IO: it plans an export (`ExportBegin`), then hands out the entries of the archive in
//! batches (`ExportNext`) and the shell writes them. This module is the shell's side of that
//! bargain; nothing here looks inside a page.
//!
//! - **Single file**: one page and nothing else is a plain `.md` or `.html` file, written like a
//!   backup is: to `<path>.tmp`, renamed into place once it is on disk.
//! - **Archive**: a `.zip` written to `<path>.tmp` with the `zip` crate and renamed at the end.
//!   Text entries (pages, whiteboards, the sidecar) are deflated; attachments are **stored** (most
//!   are images, video or PDFs that do not compress) and streamed from `attachment_read` one
//!   `CHUNK_SIZE` range at a time, the core lock released between the pieces, so a 2 GiB video
//!   never sits in memory and the editor never waits for longer than one chunk. Zip64 is used
//!   when a size or an offset needs it. Every entry carries the export time. An attachment that
//!   cannot be read is left out with an `AttachmentUnreadable` warning (a partly written entry is
//!   removed again); any other failure removes the temporary file, ends the core's session and is
//!   the caller's error.
//! - The core's session is ended on every path (`ExportEnd`), cancelled dialog included.
//!
//! The path comes from the native save dialog (`choose` here, so the tests drive the logic
//! without one) and the webview never passes one. Logs carry counts and error codes only, never a
//! path, a name or a page's text.

use std::fs;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use kb_types::attachments::CHUNK_SIZE;
use kb_types::{
    ApiError, AttachmentId, ByteRange, ErrorCode, ExportContent, ExportEntry, ExportFormat,
    ExportPlan, ExportWarning, ExportWarningCode, PageId, Request, Response, SpaceId,
};
use serde::{Deserialize, Serialize};
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::archive_path;
use crate::attachment_names::sanitised_name;
use crate::backup::{
    civil_from_days, create_temp_file, path_text, put_in_place, temp_path, write_atomically_as,
    write_error,
};
use crate::state::CoreHost;

/// Entries asked of the core per `ExportNext` call.
pub const BATCH_ENTRIES: u32 = 50;

/// How an export file is called in error messages ("the export file cannot be created").
const EXPORT_FILE: &str = "export file";

/// Between the zip writer's many small writes and the file.
const BUFFER_BYTES: usize = 64 * 1024;

/// Empty, unfinished batches in a row after which the core is taken to be stuck.
const MAX_IDLE_BATCHES: u32 = 3;

/// A size from which a zip entry needs the Zip64 headers.
const ZIP64_ENTRY_BYTES: u64 = u32::MAX as u64;

/// What the UI asks for (`export_run { options }`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRunOptions {
    /// Space.
    pub space_id: SpaceId,
    /// Root page; none for the whole space.
    #[serde(default)]
    pub page_id: Option<PageId>,
    /// Include the root page's descendants (ignored without `page_id`).
    pub include_children: bool,
    /// Markdown or HTML.
    pub format: ExportFormat,
}

/// What an export wrote (`export_run` answers it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRunResult {
    /// The file written, as text (the path chosen in the dialog).
    pub path: String,
    /// Pages written.
    pub pages: u32,
    /// Attachment files written.
    pub attachments: u32,
    /// What was left out: the core's warnings and the attachments the shell could not read.
    pub warnings: Vec<ExportWarning>,
}

fn unexpected() -> ApiError {
    ApiError::internal("the core answered an export request with something unexpected")
}

/// The name the save dialog proposes: the plan's suggested name made safe for a path once more
/// (the core makes it safe; the shell does not rely on it), or `Export.<ext>` when nothing is left.
#[must_use]
pub fn proposed_file_name(plan: &ExportPlan, format: ExportFormat) -> String {
    sanitised_name(&plan.suggested_name).unwrap_or_else(|| {
        let extension = match (plan.single_file, format) {
            (false, _) => "zip",
            (true, ExportFormat::Markdown) => "md",
            (true, ExportFormat::Html) => "html",
        };
        format!("Export.{extension}")
    })
}

/// Runs an export: `ExportBegin`, then `choose` (the save dialog; it gets the plan, which names
/// the file to propose and says whether it is one text file or a `.zip`), then the entries, then
/// `ExportEnd`. A `choose` that fails (a cancelled dialog is `InvalidInput` for `path`) ends the
/// session and is the result.
///
/// # Errors
///
/// Whatever the core answers (`Locked`, `NotFound` for a page that is gone, ...), the dialog's
/// error, and `InvalidInput` for the field `path` or `Storage` when the file cannot be written.
pub fn run(
    host: &dyn CoreHost,
    options: &ExportRunOptions,
    choose: impl FnOnce(&ExportPlan) -> Result<PathBuf, ApiError>,
) -> Result<ExportRunResult, ApiError> {
    run_with(host, options, choose, PIECE_BYTES, unix_seconds_now())
}

/// How much of an attachment one `attachment_read` call asks for: one chunk, which the core
/// decrypts as a unit.
const PIECE_BYTES: u64 = CHUNK_SIZE as u64;

/// [`run`] with the size of an attachment piece and the time stamped on the entries as
/// parameters, so a test needs neither 4 MiB attachments nor a clock.
pub fn run_with(
    host: &dyn CoreHost,
    options: &ExportRunOptions,
    choose: impl FnOnce(&ExportPlan) -> Result<PathBuf, ApiError>,
    piece_bytes: u64,
    unix_seconds: i64,
) -> Result<ExportRunResult, ApiError> {
    let plan = begin(host, options)?;
    let outcome = write_planned(host, &plan, choose, piece_bytes, unix_seconds);
    end(host, plan.export_id);
    match &outcome {
        Ok(result) => tracing::info!(
            pages = result.pages,
            attachments = result.attachments,
            warnings = result.warnings.len(),
            single_file = plan.single_file,
            "export written"
        ),
        // A cancelled save dialog is the person's choice, not a failure.
        Err(error) if is_cancelled(error) => tracing::info!("export cancelled"),
        Err(error) => tracing::warn!(code = ?error.code, "export failed"),
    }
    outcome
}

/// Whether `error` is the answer to a dialog the person cancelled (`InvalidInput` on `path`).
fn is_cancelled(error: &ApiError) -> bool {
    error.code == ErrorCode::InvalidInput
        && error
            .details
            .as_ref()
            .and_then(|details| details.get("field"))
            .and_then(|field| field.as_str())
            == Some("path")
}

fn begin(host: &dyn CoreHost, options: &ExportRunOptions) -> Result<ExportPlan, ApiError> {
    match host.dispatch(Request::ExportBegin {
        space_id: options.space_id,
        page_id: options.page_id,
        include_children: options.include_children,
        format: options.format,
    })? {
        Response::ExportPlanned(plan) => Ok(plan),
        _ => Err(unexpected()),
    }
}

/// Drops the core's session; best effort, the error that made it necessary is what matters.
fn end(host: &dyn CoreHost, export_id: u32) {
    if let Err(error) = host.dispatch(Request::ExportEnd { export_id }) {
        tracing::debug!(code = ?error.code, "an export session could not be ended");
    }
}

fn write_planned(
    host: &dyn CoreHost,
    plan: &ExportPlan,
    choose: impl FnOnce(&ExportPlan) -> Result<PathBuf, ApiError>,
    piece_bytes: u64,
    unix_seconds: i64,
) -> Result<ExportRunResult, ApiError> {
    let path = choose(plan)?;
    let (attachments, warnings) = if plan.single_file {
        (0, write_single_file(host, plan, &path)?)
    } else {
        write_archive(host, plan, &path, piece_bytes, unix_seconds)?
    };
    Ok(ExportRunResult {
        path: path_text(&path),
        pages: plan.pages,
        attachments,
        warnings,
    })
}

// --- single file ---

/// The one text entry of a single-file export, written atomically to `path`. The core promised
/// exactly one entry and no attachment; anything else is its bug and nothing is written.
fn write_single_file(
    host: &dyn CoreHost,
    plan: &ExportPlan,
    path: &Path,
) -> Result<Vec<ExportWarning>, ApiError> {
    let mut page: Option<String> = None;
    let warnings = drive(host, plan.export_id, |entry| match entry.content {
        ExportContent::Text { text } if page.is_none() => {
            page = Some(text);
            Ok(())
        }
        _ => Err(ApiError::internal(
            "the core returned more than one entry for a single-file export",
        )),
    })?;
    let text =
        page.ok_or_else(|| ApiError::internal("the core returned no entry for the export"))?;
    write_atomically_as(EXPORT_FILE, path, |sink| {
        sink(text.as_bytes()).map_err(ApiError::storage)
    })?;
    Ok(warnings)
}

// --- the batches ---

/// Asks the core for batches until it says `done`, handing every entry to `each` in order, and
/// answers the warnings the batches carried. A core that keeps answering empty, unfinished
/// batches is stuck: that is an `Internal` error rather than a loop.
fn drive(
    host: &dyn CoreHost,
    export_id: u32,
    mut each: impl FnMut(ExportEntry) -> Result<(), ApiError>,
) -> Result<Vec<ExportWarning>, ApiError> {
    let mut warnings = Vec::new();
    let mut idle = 0_u32;
    loop {
        let batch = match host.dispatch(Request::ExportNext {
            export_id,
            max_entries: BATCH_ENTRIES,
        })? {
            Response::ExportBatch(batch) => batch,
            _ => return Err(unexpected()),
        };
        warnings.extend(batch.warnings);
        if batch.entries.is_empty() && !batch.done {
            idle += 1;
            if idle >= MAX_IDLE_BATCHES {
                return Err(ApiError::internal(
                    "the core stopped producing export entries",
                ));
            }
        } else {
            idle = 0;
        }
        for entry in batch.entries {
            each(entry)?;
        }
        if batch.done {
            return Ok(warnings);
        }
    }
}

// --- the archive ---

/// Writes the whole archive to `<path>.tmp` and renames it over `path`; on any failure `path` is
/// untouched and the temporary file is gone. Answers how many attachments were written and the
/// warnings (the core's, then the unreadable attachments).
fn write_archive(
    host: &dyn CoreHost,
    plan: &ExportPlan,
    path: &Path,
    piece_bytes: u64,
    unix_seconds: i64,
) -> Result<(u32, Vec<ExportWarning>), ApiError> {
    if path.is_dir() {
        return Err(ApiError::invalid("path", "the path is a directory"));
    }
    let temp = temp_path(path);
    let file = create_temp_file(EXPORT_FILE, &temp)?;
    let outcome = fill_archive(
        host,
        plan,
        BufWriter::with_capacity(BUFFER_BYTES, file),
        piece_bytes,
        unix_seconds,
    )
    .and_then(|(writer, attachments, warnings)| {
        put_in_place(EXPORT_FILE, writer, &temp, path).map(|()| (attachments, warnings))
    });
    if outcome.is_err() {
        // The writer is gone by now (closed before it is removed: Windows will not delete a
        // file that is open). Best effort: the original error is what matters.
        let _ = fs::remove_file(&temp);
    }
    outcome
}

/// Writes every entry the core hands out into a zip over `writer` and gives the writer back
/// (the archive is complete: central directory and all).
fn fill_archive<W: Write + Seek>(
    host: &dyn CoreHost,
    plan: &ExportPlan,
    writer: W,
    piece_bytes: u64,
    unix_seconds: i64,
) -> Result<(W, u32, Vec<ExportWarning>), ApiError> {
    let stamp = zip_time(unix_seconds);
    let text_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(stamp);
    let attachment_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(stamp);

    let mut zip = ZipWriter::new(writer);
    let mut attachments = 0_u32;
    let mut unreadable: Vec<ExportWarning> = Vec::new();
    let mut warnings = drive(host, plan.export_id, |entry| {
        if !archive_path::is_safe_relative_path(&entry.path) {
            return Err(ApiError::internal(
                "the core produced an entry with an unsafe path",
            ));
        }
        match entry.content {
            ExportContent::Text { text } => {
                zip.start_file(entry.path.as_str(), text_options)
                    .map_err(zip_error)?;
                zip.write_all(text.as_bytes()).map_err(write_io_error)?;
            }
            ExportContent::Attachment {
                attachment_id,
                size,
            } => {
                let options = attachment_options.large_file(size >= ZIP64_ENTRY_BYTES);
                match copy_attachment(
                    host,
                    &mut zip,
                    &entry.path,
                    attachment_id,
                    size,
                    options,
                    piece_bytes,
                )? {
                    Copied::Written => attachments += 1,
                    Copied::Unreadable => unreadable.push(ExportWarning {
                        code: ExportWarningCode::AttachmentUnreadable,
                        page_id: None,
                        attachment_id: Some(attachment_id),
                    }),
                }
            }
        }
        Ok(())
    })?;
    warnings.extend(unreadable);
    let writer = zip.finish().map_err(zip_error)?;
    Ok((writer, attachments, warnings))
}

/// What became of an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Copied {
    /// Its entry is in the archive.
    Written,
    /// The core could not read it (or read less than it announced): it is left out.
    Unreadable,
}

/// Streams one attachment into the archive, `piece_bytes` at a time. Each piece is one
/// `attachment_read` of a byte range: the core's lock is held for that call only, and the piece is
/// written after it is released. The first piece is read before the entry is started, so an
/// attachment that cannot be read at all leaves no trace; one that fails later has its partial
/// entry removed again. A zero-byte attachment is an empty entry (an empty range is refused).
/// A locked core or a failing disk is not an unreadable attachment but the end of the export.
fn copy_attachment<W: Write + Seek>(
    host: &dyn CoreHost,
    zip: &mut ZipWriter<W>,
    name: &str,
    attachment_id: AttachmentId,
    size: u64,
    options: SimpleFileOptions,
    piece_bytes: u64,
) -> Result<Copied, ApiError> {
    if size == 0 {
        zip.start_file(name, options).map_err(zip_error)?;
        return Ok(Copied::Written);
    }
    let piece_bytes = piece_bytes.max(1);
    let first = match read_piece(host, attachment_id, 0, size.min(piece_bytes)) {
        Ok(piece) => piece,
        Err(error) => return unreadable(error),
    };
    zip.start_file(name, options).map_err(zip_error)?;
    zip.write_all(&first).map_err(write_io_error)?;
    let mut offset = first.len() as u64;
    drop(first);
    while offset < size {
        let end = size.min(offset.saturating_add(piece_bytes));
        match read_piece(host, attachment_id, offset, end) {
            Ok(piece) => {
                zip.write_all(&piece).map_err(write_io_error)?;
                offset = end;
            }
            Err(error) => {
                // The entry is half written: take it out of the archive again.
                zip.abort_file().map_err(zip_error)?;
                return unreadable(error);
            }
        }
    }
    Ok(Copied::Written)
}

/// An attachment the core could not read: a warning, unless the core is locked (then the whole
/// export is over).
fn unreadable(error: ApiError) -> Result<Copied, ApiError> {
    if error.code == ErrorCode::Locked {
        return Err(error);
    }
    tracing::warn!(code = ?error.code, "an attachment could not be read for the export");
    Ok(Copied::Unreadable)
}

/// The bytes `start..end` of an attachment, as the core delivers them (checked against the
/// length asked for: a core that stops early is an error, not a short entry).
fn read_piece(
    host: &dyn CoreHost,
    attachment_id: AttachmentId,
    start: u64,
    end: u64,
) -> Result<Vec<u8>, ApiError> {
    let wanted = end.saturating_sub(start);
    let capacity = usize::try_from(wanted)
        .map_err(|_| ApiError::internal("the attachment piece is too large for this computer"))?;
    let mut piece = Vec::new();
    piece
        .try_reserve_exact(capacity)
        .map_err(|_| ApiError::internal("there is not enough memory to read the attachment"))?;
    let mut sink = |bytes: &[u8]| -> Result<(), ApiError> {
        if piece.len() + bytes.len() > capacity {
            return Err(ApiError::internal(
                "the attachment is longer than the core announced",
            ));
        }
        piece.extend_from_slice(bytes);
        Ok(())
    };
    host.attachment_read(attachment_id, Some(ByteRange { start, end }), &mut sink)?;
    if piece.len() != capacity {
        return Err(ApiError::internal(
            "the attachment is shorter than the core announced",
        ));
    }
    Ok(piece)
}

/// A zip failure while writing: the disk's reason when it is one, otherwise a bug of ours that
/// says nothing about the content (a zip error can quote an entry's name, which is a page title).
fn zip_error(error: ZipError) -> ApiError {
    match error {
        ZipError::Io(error) => write_error(EXPORT_FILE, &error),
        _ => ApiError::internal("the archive could not be built"),
    }
}

fn write_io_error(error: std::io::Error) -> ApiError {
    write_error(EXPORT_FILE, &error)
}

// --- time ---

/// The current time as Unix seconds (0 for a clock before 1970).
fn unix_seconds_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// `unix_seconds` as the (UTC) timestamp of a zip entry; 1980-01-01, the format's start, when it
/// cannot be represented.
fn zip_time(unix_seconds: i64) -> DateTime {
    let (year, month, day) = civil_from_days(unix_seconds.div_euclid(86_400));
    let seconds = unix_seconds.rem_euclid(86_400);
    let part = |value: i64| u8::try_from(value).ok();
    let built = u16::try_from(year).ok().and_then(|year| {
        DateTime::from_date_and_time(
            year,
            u8::try_from(month).ok()?,
            u8::try_from(day).ok()?,
            part(seconds / 3_600)?,
            part(seconds % 3_600 / 60)?,
            part(seconds % 60)?,
        )
        .ok()
    });
    built.unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_stamped_with_the_export_time() {
        // 2026-10-12 09:30:15 UTC.
        let stamp = zip_time(1_791_797_415);
        assert_eq!(
            (stamp.year(), stamp.month(), stamp.day()),
            (2026, 10, 12),
            "{stamp:?}"
        );
        assert_eq!((stamp.hour(), stamp.minute()), (9, 30));
        // Seconds are stored in two-second steps.
        assert_eq!(stamp.second(), 14);
    }

    #[test]
    fn a_time_the_format_cannot_hold_is_the_start_of_the_format() {
        let start = DateTime::default();
        assert_eq!(zip_time(0), start, "1970 is before 1980");
        assert_eq!(zip_time(-5), start);
        assert_eq!(zip_time(i64::MAX / 2), start);
    }

    #[test]
    fn a_zip_error_never_quotes_the_archive() {
        let invalid = zip_error(ZipError::InvalidArchive(
            "Duplicate filename: Secret".into(),
        ));
        assert_eq!(invalid.code, ErrorCode::Internal);
        assert!(!invalid.message.contains("Secret"), "{}", invalid.message);
        let io = zip_error(ZipError::Io(std::io::Error::other("disk is full")));
        assert_eq!(io.code, ErrorCode::Storage);
        assert!(io.message.contains("disk is full"), "{}", io.message);
    }
}
