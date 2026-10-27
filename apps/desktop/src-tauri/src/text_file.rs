//! `save_text_file` (ADR-0036, `docs/architecture/import-export.md`, 5.3): the UI hands the shell
//! a text (the CSV of a view) and a suggested name, the shell asks where to save it and writes it.
//!
//! The text is whatever the UI sends and may hold the person's data, so it is written exactly as
//! given (a byte order mark the core put at its start is kept) and never logged. At most
//! [`MAX_TEXT_BYTES`]. The name is only a proposal: it is made safe for a path first
//! (`attachment_names::sanitised_name`), and the person may change it in the dialog. The file is
//! written to `<path>.tmp` and renamed into place, like a backup. The dialog is the caller's
//! business (`choose` here), so the tests drive this without one; the webview never passes a path.

use std::path::{Path, PathBuf};

use kb_types::ApiError;

use crate::attachment_ipc::limit_exceeded;
use crate::attachment_names as names;
use crate::backup::{path_text, write_atomically_as};

/// The most text a file may hold: 64 MiB.
pub const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;

/// What the dialog proposes when the suggested name leaves nothing usable.
const FALLBACK_NAME: &str = "export.txt";

/// How the file is called in error messages ("the file cannot be created").
const SAVED_FILE: &str = "file";

/// Saves `text` to the file `choose` asks the person for, given the proposed name (the suggested
/// name made safe); answers the path written. A text above [`MAX_TEXT_BYTES`] is refused first, with
/// no dialog: `InvalidInput` for the field `text` with `details.limit`.
///
/// # Errors
///
/// `InvalidInput` for `text` (too large), the dialog's error (a cancelled dialog is `InvalidInput`
/// for `path`), `InvalidInput` for `path` when the file cannot be created or put in place,
/// `Storage` when it cannot be written.
pub fn save(
    suggested_name: &str,
    text: &str,
    choose: impl FnOnce(&str) -> Result<PathBuf, ApiError>,
) -> Result<String, ApiError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(limit_exceeded(
            "text",
            "the text is too large to save as a file",
            MAX_TEXT_BYTES as u64,
        ));
    }
    let name = proposed_name(suggested_name);
    let path = choose(&name)?;
    let outcome = write(&path, text);
    match &outcome {
        Ok(_) => tracing::info!(bytes = text.len(), "text file saved"),
        Err(error) => tracing::warn!(code = ?error.code, "text file could not be saved"),
    }
    outcome
}

/// The name the dialog proposes: the suggestion made safe for a path, or a fallback.
#[must_use]
pub fn proposed_name(suggested_name: &str) -> String {
    names::sanitised_name(suggested_name).unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// The extension of a name, lowercase, without the dot (`Tasks.CSV` is `csv`).
#[must_use]
pub fn extension_of(name: &str) -> Option<String> {
    let extension = Path::new(name).extension()?.to_str()?;
    (!extension.is_empty()).then(|| extension.to_ascii_lowercase())
}

/// Writes `text` to `path` atomically.
fn write(path: &Path, text: &str) -> Result<String, ApiError> {
    write_atomically_as(SAVED_FILE, path, |sink| {
        sink(text.as_bytes()).map_err(ApiError::storage)
    })?;
    Ok(path_text(path))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use kb_types::ErrorCode;

    use super::*;

    #[test]
    fn the_proposed_name_is_made_safe_and_never_empty() {
        assert_eq!(proposed_name("Tasks.csv"), "Tasks.csv");
        assert_eq!(
            proposed_name("../evil/Tasks: all?.csv"),
            "..evilTasks_ all_.csv"
        );
        assert_eq!(proposed_name(""), FALLBACK_NAME);
        assert_eq!(proposed_name("\u{7}/\\"), FALLBACK_NAME);
        assert_eq!(proposed_name("CON.csv"), "_CON.csv");
    }

    #[test]
    fn the_extension_picks_the_dialog_filter() {
        assert_eq!(extension_of("Tasks.csv").as_deref(), Some("csv"));
        assert_eq!(extension_of("Tasks.CSV").as_deref(), Some("csv"));
        assert_eq!(extension_of("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension_of("noextension"), None);
        assert_eq!(extension_of("trailing."), None);
    }

    #[test]
    fn a_text_above_the_limit_is_refused_before_any_dialog() {
        let text = "x".repeat(MAX_TEXT_BYTES + 1);
        let error = save("big.csv", &text, |_| unreachable!("no dialog may open")).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        let details = error.details.unwrap();
        assert_eq!(details["field"], "text");
        assert_eq!(details["limit"], MAX_TEXT_BYTES as u64);
    }

    #[test]
    fn the_dialog_gets_the_proposed_name_and_a_cancel_is_the_answer() {
        let mut proposed = None;
        let error = save("a/b.csv", "x", |name| {
            proposed = Some(name.to_owned());
            Err(ApiError::invalid("path", "no file was chosen"))
        })
        .unwrap_err();
        assert_eq!(proposed.as_deref(), Some("ab.csv"));
        assert_eq!(error, ApiError::invalid("path", "no file was chosen"));
    }
}
