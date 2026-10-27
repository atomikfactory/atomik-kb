//! The disk half of the backup commands (ADR-0017). The core is sans-IO: it hands an
//! encrypted backup to a sink piece by piece and pulls the bytes of one from a source. This
//! module is the shell's side of that bargain, and nothing here looks inside the bytes.
//!
//! - **Export** writes to `<path>.tmp` and renames it over `<path>` once the core has
//!   finished and the data is on disk (`sync_all`), so an interrupted or failed export never
//!   leaves a half-written backup under the real name and never destroys a good older one.
//!   Whatever goes wrong, the temporary file is removed.
//! - **Import** reads the file in whatever sizes the core asks for (`Read::read` semantics,
//!   `0` at the end).
//! - The core's error is passed through untouched, except when the disk itself failed: that
//!   I/O error is the root cause, so it is what the caller is told, and a write that failed
//!   is never reported as a success even when the core did not notice.
//!
//! Failures to create, open or move a file are `InvalidInput` for the field `path` (the
//! user can pick another place); failures while reading or writing are `Storage`. Messages
//! carry the operating system's reason ("Access is denied", "not enough space"), never a
//! path or any content.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kb_types::{ApiError, BackupSummary, ErrorCode, Json};
use serde::Serialize;

use crate::state::{BackupSink, BackupSource, CoreHost};

/// Extension of backup files; the file dialogs filter on it.
pub const EXTENSION: &str = "atomikkb";

/// Between the core's many small writes (magic, version, header fields) and the file, and
/// the other way round for reads. Large chunks bypass it.
const BUFFER_BYTES: usize = 64 * 1024;

/// What `backup_export` and `backup_import` return: the core's summary and the file that was
/// written or read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    /// What the core says it exported or restored.
    pub summary: BackupSummary,
    /// The file used, as text (a path chosen in the dialog, or the one the caller gave).
    pub path: String,
}

/// File extension of backup files.
pub const BACKUP_EXTENSION: &str = "atomikkb";

/// Under the data directory: where a scheduled backup that has no folder is written before it is
/// uploaded to Drive (ADR-0059); the file is deleted when the run is over.
pub const BACKUP_STAGING_DIR: &str = "backup-staging";

/// Under the data directory: where a backup downloaded from Drive waits for the restore
/// (`drive_host`).
pub const RESTORE_STAGING_DIR: &str = "restore-staging";

/// An explicit path from the webview. It must be absolute (a relative one would depend
/// on the working directory of the process, which means nothing to a user) and end in
/// `.atomikkb`, so that a compromised page cannot make the shell overwrite or read an
/// arbitrary file through the backup commands.
pub fn explicit_path(text: &str) -> Result<PathBuf, ApiError> {
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(ApiError::invalid("path", "the path must be absolute"));
    }
    let has_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(BACKUP_EXTENSION));
    if !has_extension {
        return Err(ApiError::invalid(
            "path",
            format!("the path must end in .{BACKUP_EXTENSION}"),
        ));
    }
    Ok(path)
}

/// The name the save dialog proposes: `atomik-kb-<yyyy-mm-dd>.atomikkb`, for the UTC date
/// of `unix_seconds`.
pub fn default_file_name(unix_seconds: i64) -> String {
    let (year, month, day) = civil_from_days(unix_seconds.div_euclid(86_400));
    format!("atomik-kb-{year:04}-{month:02}-{day:02}.{EXTENSION}")
}

/// [`default_file_name`] for today. The user can edit the name in the dialog, so the UTC
/// date (the shell has no time-zone database) is good enough.
pub fn default_file_name_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0);
    default_file_name(seconds)
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    // `month` is 1..=12 and `day` 1..=31 by construction.
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// `<path>.tmp`: where a backup is written before it replaces `path`.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Exports the workspace to `path` through the host: the core is called once, under the
/// single-writer lock, and its pieces end up in the file or not at all.
pub fn export_to_file(host: &dyn CoreHost, path: &Path) -> Result<BackupResult, ApiError> {
    let outcome = write_atomically(path, |sink| host.backup_export(sink));
    match outcome {
        Ok(summary) => {
            tracing::info!(
                path = %path.display(),
                bytes = summary.bytes,
                documents = summary.documents,
                settings = summary.settings,
                "backup exported"
            );
            Ok(BackupResult {
                summary,
                path: path_text(path),
            })
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), code = ?error.code, "backup export failed");
            Err(error)
        }
    }
}

/// Restores the workspace from the file at `path` through the host: the core is called
/// once, under the single-writer lock, with a source that reads the file. `kit` is the
/// recovery kit text, passed on untouched.
pub fn import_from_file(
    host: &dyn CoreHost,
    path: &Path,
    kit: Option<&str>,
) -> Result<BackupResult, ApiError> {
    let outcome = read_file(path, |source| host.backup_import(source, kit));
    match outcome {
        Ok(summary) => {
            tracing::info!(
                path = %path.display(),
                bytes = summary.bytes,
                documents = summary.documents,
                settings = summary.settings,
                "backup imported"
            );
            Ok(BackupResult {
                summary,
                path: path_text(path),
            })
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), code = ?error.code, "backup import failed");
            Err(with_chosen_path(error, path))
        }
    }
}

/// When the core needs the recovery kit to restore a file (`InvalidInput` for the field
/// `kit`), the error also carries the file that was read as `details.path`, so the caller
/// can ask for the kit and retry without opening the file dialog a second time. Every
/// other error passes through untouched.
fn with_chosen_path(mut error: ApiError, path: &Path) -> ApiError {
    if error.code != ErrorCode::InvalidInput {
        return error;
    }
    if let Some(Json::Object(details)) = error.details.as_mut()
        && details.get("field").and_then(Json::as_str) == Some("kit")
    {
        details.insert("path".to_owned(), Json::String(path_text(path)));
    }
    error
}

/// How the backup file is called in error messages.
pub const BACKUP_FILE: &str = "backup file";

/// Runs `produce` with a sink that writes to `<path>.tmp`. When `produce` succeeds the data
/// is flushed and synced and the temporary file is renamed over `path` (replacing a file
/// that is already there); when it fails, or the disk does, the temporary file is removed
/// and `path` is untouched.
pub fn write_atomically<T>(
    path: &Path,
    produce: impl FnOnce(BackupSink<'_>) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    write_atomically_as(BACKUP_FILE, path, produce)
}

/// [`write_atomically`] for a file that is not a backup (a saved attachment): `noun` names
/// it in the error messages ("the <noun> cannot be created", "the <noun> could not be
/// written").
pub fn write_atomically_as<T>(
    noun: &str,
    path: &Path,
    produce: impl FnOnce(BackupSink<'_>) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    if path.is_dir() {
        return Err(ApiError::invalid("path", "the path is a directory"));
    }
    let temp = temp_path(path);
    let mut writer = BufWriter::with_capacity(BUFFER_BYTES, create_temp_file(noun, &temp)?);
    let outcome = match stream_into(noun, &mut writer, produce) {
        Ok(value) => put_in_place(noun, writer, &temp, path).map(|()| value),
        Err(error) => {
            // Closed before it is removed: Windows will not delete a file that is open.
            drop(writer);
            Err(error)
        }
    };
    if outcome.is_err() {
        // Best effort: the original error is what matters, and a leftover is replaced by
        // the next export anyway.
        let _ = fs::remove_file(&temp);
    }
    outcome
}

/// Creates the temporary file, replacing a leftover of an earlier export that died. The
/// leftover is removed first and the file then created with `create_new`, so what is
/// written to is always a file made here, never something (a symbolic link, say) that
/// somebody placed at that name. On Unix the file is private to its owner (`0600`): the
/// backup is encrypted, but its header is not, and nobody else has a use for it.
pub(crate) fn create_temp_file(noun: &str, temp: &Path) -> Result<File, ApiError> {
    let cannot_create = format!("the {noun} cannot be created");
    match fs::remove_file(temp) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error(&cannot_create, &error)),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(temp)
        .map_err(|error| path_error(&cannot_create, &error))
}

/// Flushes, syncs and closes the temporary file, then renames it over the target.
pub(crate) fn put_in_place(
    noun: &str,
    writer: BufWriter<File>,
    temp: &Path,
    path: &Path,
) -> Result<(), ApiError> {
    let file = writer
        .into_inner()
        .map_err(|error| write_error(noun, error.error()))?;
    file.sync_all().map_err(|error| write_error(noun, &error))?;
    drop(file);
    fs::rename(temp, path)
        .map_err(|error| path_error(&format!("the {noun} cannot be put in place"), &error))
}

/// Runs `produce` with a sink that appends to `writer`. If a write fails, that I/O error is
/// the result, whatever `produce` made of it (even a success: the data is incomplete).
fn stream_into<W: Write, T>(
    noun: &str,
    writer: &mut W,
    produce: impl FnOnce(BackupSink<'_>) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let mut failure: Option<io::Error> = None;
    let result = {
        let mut sink = |piece: &[u8]| -> Result<(), String> {
            writer.write_all(piece).map_err(|error| {
                let message = error.to_string();
                failure = Some(error);
                message
            })
        };
        produce(&mut sink)
    };
    match failure {
        Some(error) => Err(write_error(noun, &error)),
        None => result,
    }
}

/// Opens `path` and runs `consume` with a source that reads it. If a read fails and
/// `consume` fails with it, that I/O error is the result.
pub fn read_file<T>(
    path: &Path,
    consume: impl FnOnce(BackupSource<'_>) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    const CANNOT_OPEN: &str = "the backup file cannot be opened";
    let file = File::open(path).map_err(|error| path_error(CANNOT_OPEN, &error))?;
    let metadata = file
        .metadata()
        .map_err(|error| path_error(CANNOT_OPEN, &error))?;
    if !metadata.is_file() {
        return Err(ApiError::invalid("path", "the path is not a file"));
    }
    stream_from(&mut BufReader::with_capacity(BUFFER_BYTES, file), consume)
}

/// Runs `consume` with a source that reads from `reader`: it fills the buffer it is given
/// like [`Read::read`] does, retrying when a read is interrupted, and returns `0` at the end.
fn stream_from<R: Read, T>(
    reader: &mut R,
    consume: impl FnOnce(BackupSource<'_>) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let mut failure: Option<io::Error> = None;
    let result = {
        let mut source = |buffer: &mut [u8]| -> Result<usize, String> {
            loop {
                match reader.read(buffer) {
                    Ok(count) => return Ok(count),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        let message = error.to_string();
                        failure = Some(error);
                        return Err(message);
                    }
                }
            }
        };
        consume(&mut source)
    };
    match (failure, result) {
        (Some(error), Err(_)) => Err(read_error(&error)),
        (_, result) => result,
    }
}

/// A file the user chose cannot be created, opened or moved: the path is what to change.
pub fn path_error(what: &str, error: &io::Error) -> ApiError {
    ApiError::invalid("path", format!("{what}: {error}"))
}

/// A write to the file failed: the disk's reason is the cause (`noun` names the file).
pub fn write_error(noun: &str, error: &io::Error) -> ApiError {
    ApiError::storage(format!("the {noun} could not be written: {error}"))
}

fn read_error(error: &io::Error) -> ApiError {
    ApiError::storage(format!("the backup file could not be read: {error}"))
}

/// A path as text for the answer of a command (lossy on the rare non-Unicode path).
pub fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Empties a staging folder (`BACKUP_STAGING_DIR`, `RESTORE_STAGING_DIR`): files and links go by
/// themselves, folders with what they hold; the folder itself stays. Answers `(removed, failed)`.
/// A folder that is not there has nothing to remove. Best effort: what cannot be removed is counted,
/// not an error (the next start tries again), and nothing is logged here, least of all a name.
pub fn empty_staging(dir: &Path) -> (usize, usize) {
    let Ok(read) = fs::read_dir(dir) else {
        return (0, 0);
    };
    let (mut removed, mut failed) = (0, 0);
    for entry in read.filter_map(Result::ok) {
        // `file_type` does not follow a link: a link to a folder is removed as a link.
        let gone = match entry.file_type() {
            Ok(kind) if kind.is_dir() => fs::remove_dir_all(entry.path()),
            _ => fs::remove_file(entry.path()),
        };
        match gone {
            Ok(()) => removed += 1,
            Err(_) => failed += 1,
        }
    }
    (removed, failed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::cell::Cell;

    use kb_types::{AccountId, ErrorCode};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// The `field` an error names in its details.
    fn field(error: &ApiError) -> Option<&str> {
        error.details.as_ref()?.get("field")?.as_str()
    }

    /// A producer that writes `pieces` to the sink and reports how many bytes it wrote.
    fn write_pieces<'a>(
        pieces: &'a [&'a [u8]],
    ) -> impl FnOnce(BackupSink<'_>) -> Result<usize, ApiError> + 'a {
        move |sink| {
            let mut total = 0;
            for piece in pieces {
                sink(piece).map_err(ApiError::storage)?;
                total += piece.len();
            }
            Ok(total)
        }
    }

    /// A writer that accepts `limit` bytes and then fails like a full disk.
    struct FullDisk {
        limit: usize,
        written: usize,
    }

    impl Write for FullDisk {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.written + buf.len() > self.limit {
                return Err(io::Error::other("there is not enough space on the disk"));
            }
            self.written += buf.len();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // --- naming ---

    #[test]
    fn the_default_name_carries_the_utc_date_and_the_extension() {
        for (seconds, expected) in [
            (0, "atomik-kb-1970-01-01.atomikkb"),
            (1_000_000_000, "atomik-kb-2001-09-09.atomikkb"),
            (1_700_000_000, "atomik-kb-2023-11-14.atomikkb"),
            (1_790_985_600, "atomik-kb-2026-10-03.atomikkb"),
            (4_102_444_800, "atomik-kb-2100-01-01.atomikkb"),
        ] {
            assert_eq!(default_file_name(seconds), expected, "{seconds}");
        }
    }

    #[test]
    fn the_default_name_handles_leap_days_and_the_edges_of_a_day() {
        for (seconds, expected) in [
            (951_782_400, "atomik-kb-2000-02-29.atomikkb"),
            (951_868_799, "atomik-kb-2000-02-29.atomikkb"),
            (951_868_800, "atomik-kb-2000-03-01.atomikkb"),
            (1_709_164_800, "atomik-kb-2024-02-29.atomikkb"),
            (-1, "atomik-kb-1969-12-31.atomikkb"),
            (-86_400, "atomik-kb-1969-12-31.atomikkb"),
            (-86_401, "atomik-kb-1969-12-30.atomikkb"),
        ] {
            assert_eq!(default_file_name(seconds), expected, "{seconds}");
        }
    }

    #[test]
    fn the_default_name_for_today_looks_like_a_backup_name() {
        let name = default_file_name_now();
        assert!(name.starts_with("atomik-kb-"), "{name}");
        assert!(name.ends_with(".atomikkb"), "{name}");
        // atomik-kb-yyyy-mm-dd.atomikkb
        assert_eq!(name.len(), "atomik-kb-2026-10-03.atomikkb".len(), "{name}");
    }

    #[test]
    fn emptying_a_staging_folder_removes_what_it_holds_and_keeps_the_folder() {
        let dir = dir();
        let staging = dir.path().join(BACKUP_STAGING_DIR);
        fs::create_dir(&staging).unwrap();
        fs::write(
            staging.join("atomik-kb-auto-2026-10-19-0930.atomikkb"),
            b"x",
        )
        .unwrap();
        fs::write(staging.join("leftover.atomikkb.tmp"), b"y").unwrap();
        fs::create_dir(staging.join("nested")).unwrap();
        fs::write(staging.join("nested").join("deep.bin"), b"z").unwrap();
        // A neighbour is not touched.
        fs::write(dir.path().join("keep.txt"), b"keep").unwrap();

        assert_eq!(empty_staging(&staging), (3, 0));

        assert!(staging.is_dir());
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 0);
        assert_eq!(fs::read(dir.path().join("keep.txt")).unwrap(), b"keep");
        // Nothing left to do, and a folder that is not there is nothing either.
        assert_eq!(empty_staging(&staging), (0, 0));
        assert_eq!(empty_staging(&dir.path().join(RESTORE_STAGING_DIR)), (0, 0));
    }

    #[test]
    fn the_temporary_name_is_the_target_name_plus_tmp() {
        assert_eq!(
            temp_path(Path::new("/a/b/backup.atomikkb")),
            PathBuf::from("/a/b/backup.atomikkb.tmp")
        );
        assert_eq!(temp_path(Path::new("x")), PathBuf::from("x.tmp"));
    }

    #[test]
    fn explicit_paths_must_be_absolute_backup_files() {
        let absolute = std::env::temp_dir().join("backup.atomikkb");
        assert_eq!(explicit_path(absolute.to_str().unwrap()).unwrap(), absolute);
        let upper = std::env::temp_dir().join("BACKUP.ATOMIKKB");
        assert_eq!(explicit_path(upper.to_str().unwrap()).unwrap(), upper);
        for relative in [
            "",
            "backup.atomikkb",
            "./backup.atomikkb",
            "..\\backup",
            "a/b",
        ] {
            let error = explicit_path(relative).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{relative:?}");
            assert_eq!(field(&error), Some("path"), "{relative:?}");
        }
        // Absolute, but not a backup file: the shell must not touch it.
        for other in ["notes.md", "workspace.db", "backup", "backup.atomikkb.exe"] {
            let path = std::env::temp_dir().join(other);
            let error = explicit_path(path.to_str().unwrap()).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{other}");
            assert_eq!(field(&error), Some("path"), "{other}");
        }
    }

    #[test]
    fn the_result_serialises_with_camel_case_names() {
        let result = BackupResult {
            summary: BackupSummary {
                bytes: 12,
                documents: 3,
                settings: 4,
                created_at: 1_700_000_000_000,
                account_id: AccountId::from_bytes([1; 16]),
                chunks: 0,
                chunk_bytes: 0,
            },
            path: "C:\\backups\\x.atomikkb".into(),
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "summary": {
                    "bytes": 12,
                    "documents": 3,
                    "settings": 4,
                    "createdAt": 1_700_000_000_000_i64,
                    "accountId": "01010101-0101-0101-0101-010101010101",
                    "chunks": 0,
                    "chunkBytes": 0,
                },
                "path": "C:\\backups\\x.atomikkb",
            })
        );
    }

    // --- writing: temp file, sync, rename ---

    #[test]
    fn an_export_ends_up_in_the_file_and_leaves_no_temporary_file() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");

        let total = write_atomically(&path, write_pieces(&[b"KBB1", &[1, 2, 3], &[255]])).unwrap();

        assert_eq!(total, 8);
        assert_eq!(fs::read(&path).unwrap(), b"KBB1\x01\x02\x03\xff");
        assert!(!temp_path(&path).exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn the_data_goes_to_the_temporary_file_until_the_export_is_done() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::write(&path, b"the previous backup").unwrap();
        let temp = temp_path(&path);

        write_atomically(&path, |sink| {
            sink(b"new data").map_err(ApiError::storage)?;
            // Still working: the old file is untouched and the new one is being made.
            assert_eq!(fs::read(&path).unwrap(), b"the previous backup");
            assert!(temp.exists());
            Ok(())
        })
        .unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new data");
        assert!(!temp.exists());
    }

    #[test]
    fn a_failed_export_removes_the_temporary_file_and_keeps_the_previous_backup() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::write(&path, b"the previous backup").unwrap();

        let error = write_atomically(&path, |sink| {
            sink(b"half of a ba").map_err(ApiError::storage)?;
            Err::<(), _>(ApiError::crypto("something went wrong"))
        })
        .unwrap_err();

        assert_eq!(error, ApiError::crypto("something went wrong"));
        assert_eq!(fs::read(&path).unwrap(), b"the previous backup");
        assert!(!temp_path(&path).exists());
    }

    #[test]
    fn a_failed_first_export_creates_nothing() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");

        let result = write_atomically(&path, |sink| {
            sink(b"data").map_err(ApiError::storage)?;
            Err::<(), _>(ApiError::internal("boom"))
        });

        assert!(result.is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_leftover_temporary_file_from_a_crash_is_replaced() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::write(
            temp_path(&path),
            b"junk from an export that died, longer than the data",
        )
        .unwrap();

        write_atomically(&path, write_pieces(&[b"fresh"])).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"fresh");
        assert!(!temp_path(&path).exists());
    }

    #[test]
    fn a_missing_directory_is_an_invalid_path_and_the_core_is_never_asked() {
        let dir = dir();
        let path = dir.path().join("no-such-directory").join("backup.atomikkb");
        let asked = Cell::new(false);

        let error = write_atomically(&path, |_| {
            asked.set(true);
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(error.message.contains("cannot be created"), "{error}");
        assert!(
            !asked.get(),
            "the core must not run when the file cannot be created"
        );
    }

    #[test]
    fn a_directory_as_the_target_is_refused_before_anything_is_created() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::create_dir(&path).unwrap();
        let asked = Cell::new(false);

        let error = write_atomically(&path, |_| {
            asked.set(true);
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(!asked.get());
        assert!(!temp_path(&path).exists());
        assert!(path.is_dir(), "the directory must still be there");
    }

    #[test]
    fn a_directory_at_the_temporary_name_is_an_invalid_path() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::create_dir(temp_path(&path)).unwrap();
        let asked = Cell::new(false);

        let error = write_atomically(&path, |_| {
            asked.set(true);
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(!asked.get());
        assert!(
            temp_path(&path).is_dir(),
            "somebody else's directory is left alone"
        );
    }

    #[test]
    fn large_exports_are_written_completely_and_in_order() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        let first = vec![1_u8; 3 * BUFFER_BYTES + 5];
        let last = vec![2_u8; 17];

        write_atomically(&path, write_pieces(&[&first, &[9], &last])).unwrap();

        let mut expected = first.clone();
        expected.push(9);
        expected.extend_from_slice(&last);
        assert_eq!(fs::read(&path).unwrap(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn the_backup_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        write_atomically(&path, write_pieces(&[b"KBB1"])).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    #[test]
    fn an_export_of_nothing_is_an_empty_file() {
        let dir = dir();
        let path = dir.path().join("empty.atomikkb");
        write_atomically(&path, write_pieces(&[])).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"");
    }

    #[test]
    fn the_return_value_of_the_producer_is_passed_through() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        let value = write_atomically(&path, |_| Ok("the summary")).unwrap();
        assert_eq!(value, "the summary");
    }

    #[test]
    fn a_failing_disk_is_reported_as_the_cause_whatever_the_core_makes_of_it() {
        // The core maps a failed sink to its own, vaguer error: the disk's reason wins.
        let mut disk = FullDisk {
            limit: 4,
            written: 0,
        };
        let error = stream_into(BACKUP_FILE, &mut disk, |sink| {
            sink(b"abcd").map_err(ApiError::internal)?;
            sink(b"efgh").map_err(|_| ApiError::internal("the backup failed"))?;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::Storage);
        assert!(
            error
                .message
                .contains("there is not enough space on the disk"),
            "{error}"
        );
        assert!(error.message.contains("could not be written"), "{error}");
    }

    #[test]
    fn a_write_that_failed_is_never_reported_as_a_success() {
        // A core that ignores the sink's error and says "done" must not get its way: the
        // file would be missing data.
        let mut disk = FullDisk {
            limit: 0,
            written: 0,
        };
        let error = stream_into(BACKUP_FILE, &mut disk, |sink| {
            let _ = sink(b"data");
            Ok("done")
        })
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
    }

    #[test]
    fn the_core_s_own_error_passes_through_when_the_disk_was_fine() {
        let mut disk = FullDisk {
            limit: 100,
            written: 0,
        };
        let error = stream_into(BACKUP_FILE, &mut disk, |sink| {
            sink(b"abc").map_err(ApiError::internal)?;
            Err::<(), _>(ApiError::invalid(
                "workspace",
                "there is nothing to back up",
            ))
        })
        .unwrap_err();
        assert_eq!(
            error,
            ApiError::invalid("workspace", "there is nothing to back up")
        );
    }

    // --- reading in chunks ---

    /// A file whose bytes are recognisable at any offset.
    fn patterned(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from((i * 7 + i / 251) % 256).unwrap())
            .collect()
    }

    /// Reads the whole source with a buffer of `size` bytes, returning what it read and how
    /// many reads it took.
    fn drain(source: BackupSource<'_>, size: usize) -> Result<(Vec<u8>, usize), ApiError> {
        let mut buffer = vec![0_u8; size];
        let mut all = Vec::new();
        let mut reads = 0;
        loop {
            let count = source(&mut buffer).map_err(ApiError::storage)?;
            reads += 1;
            if count == 0 {
                return Ok((all, reads));
            }
            assert!(count <= size, "a read returned more than the buffer holds");
            all.extend_from_slice(&buffer[..count]);
        }
    }

    #[test]
    fn a_file_is_read_to_the_end_whatever_the_buffer_size() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        let content = patterned(3 * BUFFER_BYTES + 123);
        fs::write(&path, &content).unwrap();

        for size in [1, 7, 4096, BUFFER_BYTES, BUFFER_BYTES + 1, 1_000_000] {
            let (read, _) = read_file(&path, |source| drain(source, size)).unwrap();
            assert_eq!(read.len(), content.len(), "buffer of {size}");
            assert!(read == content, "buffer of {size}: the bytes differ");
        }
    }

    #[test]
    fn small_reads_come_back_a_buffer_at_a_time() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::write(&path, [1_u8, 2, 3, 4, 5, 6, 7]).unwrap();

        let (read, reads) = read_file(&path, |source| drain(source, 3)).unwrap();

        assert_eq!(read, [1, 2, 3, 4, 5, 6, 7]);
        // 3 + 3 + 1 bytes, then the end.
        assert_eq!(reads, 4);
    }

    #[test]
    fn the_end_of_the_file_stays_the_end() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::write(&path, b"ab").unwrap();

        read_file(&path, |source| {
            let mut buffer = [0_u8; 8];
            assert_eq!(source(&mut buffer).unwrap(), 2);
            assert_eq!(source(&mut buffer).unwrap(), 0);
            assert_eq!(source(&mut buffer).unwrap(), 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn an_empty_file_reads_as_the_end_at_once() {
        let dir = dir();
        let path = dir.path().join("empty.atomikkb");
        fs::write(&path, b"").unwrap();
        let (read, reads) = read_file(&path, |source| drain(source, 16)).unwrap();
        assert!(read.is_empty());
        assert_eq!(reads, 1);
    }

    #[test]
    fn a_missing_file_is_an_invalid_path_and_the_core_is_never_asked() {
        let dir = dir();
        let asked = Cell::new(false);
        let error = read_file(&dir.path().join("nothing.atomikkb"), |_| {
            asked.set(true);
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(error.message.contains("cannot be opened"), "{error}");
        assert!(!asked.get());
    }

    #[test]
    fn a_directory_is_not_a_backup_file() {
        let dir = dir();
        let asked = Cell::new(false);
        let error = read_file(dir.path(), |_| {
            asked.set(true);
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
        assert!(!asked.get());
    }

    #[test]
    fn the_core_s_error_passes_through_a_read() {
        let dir = dir();
        let path = dir.path().join("backup.atomikkb");
        fs::write(&path, b"not a backup").unwrap();
        let error = read_file(&path, |_| {
            Err::<(), _>(ApiError::crypto("the file is not a backup"))
        })
        .unwrap_err();
        assert_eq!(error, ApiError::crypto("the file is not a backup"));
    }

    #[test]
    fn a_kit_required_error_carries_the_file_that_was_read() {
        let path = Path::new("/backups/home.atomikkb");
        let error = with_chosen_path(
            ApiError::invalid("kit", "the recovery kit is required to restore this backup"),
            path,
        );
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("kit"));
        assert_eq!(
            error
                .details
                .as_ref()
                .and_then(|d| d.get("path"))
                .and_then(Json::as_str),
            Some(path_text(path).as_str())
        );
        assert_eq!(
            error.message,
            "the recovery kit is required to restore this backup"
        );
    }

    #[test]
    fn other_import_errors_pass_through_without_a_path() {
        let path = Path::new("/backups/home.atomikkb");
        let wrong_kit = ApiError::invalid("kit", "this kit does not belong to this backup");
        // The same field, so the path is attached: the caller can retry with another kit.
        assert!(
            with_chosen_path(wrong_kit, path)
                .details
                .unwrap()
                .get("path")
                .is_some()
        );

        let bad_path = ApiError::invalid("path", "no such file");
        assert_eq!(with_chosen_path(bad_path.clone(), path), bad_path);
        let damaged = ApiError::crypto("the backup is damaged or the key is wrong");
        assert_eq!(with_chosen_path(damaged.clone(), path), damaged);
        let bare = ApiError::new(ErrorCode::InvalidInput, "no details at all");
        assert_eq!(with_chosen_path(bare.clone(), path), bare);
    }

    /// A reader that is interrupted `interruptions` times, serves `data`, then fails.
    struct FlakyReader {
        interruptions: usize,
        data: &'static [u8],
        fail_at_end: bool,
    }

    impl Read for FlakyReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.interruptions > 0 {
                self.interruptions -= 1;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.data.is_empty() {
                if self.fail_at_end {
                    return Err(io::Error::other("the drive was unplugged"));
                }
                return Ok(0);
            }
            let count = buf.len().min(self.data.len());
            buf[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            Ok(count)
        }
    }

    #[test]
    fn an_interrupted_read_is_retried() {
        let mut reader = FlakyReader {
            interruptions: 3,
            data: b"payload",
            fail_at_end: false,
        };
        let (read, _) = stream_from(&mut reader, |source| drain(source, 64)).unwrap();
        assert_eq!(read, b"payload");
    }

    #[test]
    fn a_failing_drive_is_reported_as_the_cause_whatever_the_core_makes_of_it() {
        let mut reader = FlakyReader {
            interruptions: 0,
            data: b"payload",
            fail_at_end: true,
        };
        let error = stream_from(&mut reader, |source| {
            drain(source, 64).map_err(|_| ApiError::crypto("the backup is truncated"))
        })
        .unwrap_err();

        assert_eq!(error.code, ErrorCode::Storage);
        assert!(error.message.contains("the drive was unplugged"), "{error}");
        assert!(error.message.contains("could not be read"), "{error}");
    }
}
