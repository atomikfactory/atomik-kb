//! The backups of a folder, for the restore wizard's "A folder of backups..." (ADR-0059,
//! `docs/architecture/drive-backup.md` section 3.3): the `.atomikkb` files of one folder, newest first.
//!
//! Pure over `std::fs` (no core, no dialog): the command in `lib.rs` asks the person for the folder (or takes
//! an absolute path the page already got from the previous listing) and calls [`list_folder`]. The listing
//! names each file by its absolute path, which is what `backup_import` takes. Nothing is read from a file
//! and nothing is logged: not a name, not a path.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use kb_types::ApiError;
use kb_types::interop::{BackupFileInfo, BackupFolderListing};

use crate::backup::{BACKUP_EXTENSION, path_text};

/// The most backups a listing returns (the newest).
pub const MAX_LISTED: usize = 200;

/// The field the failures name.
const PATH_FIELD: &str = "path";

/// The `.atomikkb` regular files of `folder` (its sub-folders are not entered), newest first by
/// modification time (a tie goes to the name that sorts last, which for scheduled backups is the later
/// run), at most [`MAX_LISTED`]. A file the system will not describe is left out; one whose time it will
/// not say counts as the oldest.
///
/// # Errors
///
/// `InvalidInput` for the field `path` when `folder` is not absolute, is not a folder or cannot be read.
pub fn list_folder(folder: &Path) -> Result<BackupFolderListing, ApiError> {
    if !folder.is_absolute() {
        return Err(ApiError::invalid(PATH_FIELD, "the path must be absolute"));
    }
    let read = fs::read_dir(folder)
        .map_err(|_| ApiError::invalid(PATH_FIELD, "the folder cannot be read"))?;
    let mut backups: Vec<BackupFileInfo> = read
        .filter_map(Result::ok)
        .filter_map(|entry| describe(&entry.path()))
        .collect();
    backups.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.name.cmp(&a.name))
    });
    backups.truncate(MAX_LISTED);
    Ok(BackupFolderListing {
        folder: path_text(folder),
        backups,
    })
}

/// What a listing says of `path`, when it is a backup: a regular file (a link to one counts) whose name
/// is text and ends in `.atomikkb` in any case, with something before the extension.
fn describe(path: &Path) -> Option<BackupFileInfo> {
    let name = path.file_name()?.to_str()?;
    let extension = path.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case(BACKUP_EXTENSION) || path.file_stem()?.is_empty() {
        return None;
    }
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0);
    Some(BackupFileInfo {
        id: path_text(path),
        name: name.to_owned(),
        size: metadata.len(),
        modified_at,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use kb_types::ErrorCode;

    use super::*;

    /// A file of `size` bytes whose modification time is `seconds` after the epoch.
    fn make(folder: &Path, name: &str, size: usize, seconds: u64) {
        let path = folder.join(name);
        fs::write(&path, vec![7_u8; size]).unwrap();
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    fn names(listing: &BackupFolderListing) -> Vec<&str> {
        listing
            .backups
            .iter()
            .map(|backup| backup.name.as_str())
            .collect()
    }

    #[test]
    fn the_backups_of_a_folder_come_newest_first_with_their_path_size_and_time() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), "old.atomikkb", 10, 1_000);
        make(dir.path(), "new.atomikkb", 30, 3_000);
        make(dir.path(), "middle.atomikkb", 20, 2_000);

        let listing = list_folder(dir.path()).unwrap();

        assert_eq!(listing.folder, path_text(dir.path()));
        assert_eq!(
            names(&listing),
            ["new.atomikkb", "middle.atomikkb", "old.atomikkb"]
        );
        let newest = &listing.backups[0];
        assert_eq!(newest.size, 30);
        assert_eq!(newest.modified_at, 3_000_000);
        assert_eq!(newest.id, path_text(&dir.path().join("new.atomikkb")));
        // The id is the absolute path `backup_import` takes.
        assert!(Path::new(&newest.id).is_absolute());
    }

    #[test]
    fn only_regular_backup_files_of_the_folder_itself_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), "kept.atomikkb", 1, 10);
        make(dir.path(), "UPPER.ATOMIKKB", 1, 20);
        make(dir.path(), "notes.txt", 1, 30);
        make(dir.path(), "backup.atomikkb.tmp", 1, 40);
        make(dir.path(), "atomikkb", 1, 50);
        make(dir.path(), ".atomikkb", 1, 60);
        make(dir.path(), "zip.zip", 1, 70);
        // A folder named like a backup is not one, and what is inside a sub-folder is not listed.
        fs::create_dir(dir.path().join("folder.atomikkb")).unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        make(&sub, "inside.atomikkb", 1, 80);

        let listing = list_folder(dir.path()).unwrap();

        assert_eq!(names(&listing), ["UPPER.ATOMIKKB", "kept.atomikkb"]);
    }

    #[test]
    fn equal_times_are_ordered_by_name_latest_first() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "atomik-kb-auto-2026-10-12-0930.atomikkb",
            "atomik-kb-auto-2026-10-13-0930.atomikkb",
            "atomik-kb-auto-2026-10-11-0930.atomikkb",
        ] {
            make(dir.path(), name, 1, 500);
        }

        let listing = list_folder(dir.path()).unwrap();

        assert_eq!(
            names(&listing),
            [
                "atomik-kb-auto-2026-10-13-0930.atomikkb",
                "atomik-kb-auto-2026-10-12-0930.atomikkb",
                "atomik-kb-auto-2026-10-11-0930.atomikkb",
            ]
        );
    }

    #[test]
    fn at_most_two_hundred_of_the_newest_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..(MAX_LISTED + 5) {
            make(
                dir.path(),
                &format!("b-{index:03}.atomikkb"),
                1,
                1_000 + index as u64,
            );
        }

        let listing = list_folder(dir.path()).unwrap();

        assert_eq!(listing.backups.len(), MAX_LISTED);
        assert_eq!(listing.backups[0].name, "b-204.atomikkb");
        assert_eq!(
            listing.backups[MAX_LISTED - 1].name,
            format!("b-{:03}.atomikkb", 5)
        );
    }

    #[test]
    fn an_empty_folder_has_no_backups() {
        let dir = tempfile::tempdir().unwrap();
        let listing = list_folder(dir.path()).unwrap();
        assert!(listing.backups.is_empty());
        assert_eq!(listing.folder, path_text(dir.path()));
    }

    #[test]
    fn a_folder_that_cannot_be_read_is_invalid_input_for_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain.atomikkb");
        fs::write(&file, b"x").unwrap();
        for bad in [dir.path().join("missing"), file] {
            let error = list_folder(&bad).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{bad:?}");
            assert_eq!(error.details.unwrap()["field"], "path", "{bad:?}");
        }
        let error = list_folder(Path::new("relative/folder")).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.details.unwrap()["field"], "path");
    }

    #[test]
    fn an_error_never_names_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("secret-folder-name");
        let error = list_folder(&missing).unwrap_err();
        assert!(!serde_json::to_string(&error).unwrap().contains("secret"));
    }
}
