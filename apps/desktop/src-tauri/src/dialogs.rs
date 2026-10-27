//! The native file dialogs of the backup and attachment commands, opened from Rust with
//! `tauri-plugin-dialog` (the webview has no permission to open any, see
//! `capabilities/default.json`).
//!
//! The dialogs are *blocking* calls: the plugin creates the dialog on the main thread and
//! parks the calling thread until the user answers, so they must never run on the main
//! thread itself. The commands call them from `spawn_blocking` threads, and only after they
//! have checked that the core is ready, so a locked or starting app never shows a dialog.
//! The dialogs are modal to the main window; the backup ones filter on `.atomikkb`, the
//! attachment ones filter on nothing (any file). The import and export ones (ADR-0036) filter on
//! the type of file they ask for: the import's files on Markdown, text, HTML and CSV, its zip on
//! `.zip`, a saved export or text file on the extension of the proposed name; the folder dialogs
//! (an import's folder, the folder of scheduled backups) filter on nothing. Cancelling any of them is
//! `InvalidInput` for the field `path`.

use std::path::PathBuf;
use std::sync::Arc;

use kb_types::ApiError;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath, MessageDialogButtons};

use crate::MAIN_WINDOW;
use crate::backup::{EXTENSION, default_file_name_now};
use crate::shell_labels::{LabelStore, Labels};

/// The words of the shell now (English until the UI sends its own, see `shell_labels`): the file types the file
/// dialogs list, through [`Confirm::labels`] the consent dialogs, and the title of a reminder's notification for an
/// untitled page.
pub(crate) fn labels_of<R: Runtime>(app: &AppHandle<R>) -> Labels {
    app.try_state::<Arc<LabelStore>>()
        .map(|store| store.get())
        .unwrap_or_default()
}

/// Asks where to save a backup: the native save dialog, proposing
/// `atomik-kb-<yyyy-mm-dd>.atomikkb`. Cancelling is `InvalidInput` for the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_export_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    let mut dialog = app
        .dialog()
        .file()
        .set_file_name(default_file_name_now())
        .add_filter(labels_of(app).file_filter_backup, &[EXTENSION]);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    chosen_path(dialog.blocking_save_file())
}

/// Asks which backup to restore: the native open dialog. Cancelling is `InvalidInput` for
/// the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_import_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    let mut dialog = app
        .dialog()
        .file()
        .add_filter(labels_of(app).file_filter_backup, &[EXTENSION]);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    chosen_path(dialog.blocking_pick_file())
}

/// Asks which file to import as an attachment: the native open dialog, any file. Cancelling
/// is `InvalidInput` for the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_attachment_import_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    let mut dialog = app.dialog().file();
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    chosen_path(dialog.blocking_pick_file())
}

/// Asks where to save an attachment: the native save dialog, proposing `suggested` (a name
/// the shell has already sanitised, see `attachment_names::path_safe_name`). Cancelling is
/// `InvalidInput` for the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_attachment_save_path<R: Runtime>(
    app: &AppHandle<R>,
    suggested: &str,
) -> Result<PathBuf, ApiError> {
    let mut dialog = app.dialog().file().set_file_name(suggested);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    chosen_path(dialog.blocking_save_file())
}

// --- import and export (ADR-0036) ---

/// The extensions the import's file dialog offers: Markdown, text, HTML and CSV.
pub const IMPORT_FILE_EXTENSIONS: [&str; 6] = ["md", "markdown", "txt", "html", "htm", "csv"];

/// A file dialog parented to the main window, when there is one.
fn parented<R: Runtime>(app: &AppHandle<R>) -> FileDialogBuilder<R> {
    let dialog = app.dialog().file();
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) => dialog.set_parent(&window),
        None => dialog,
    }
}

/// The filter of a save dialog from the extension of the proposed name: a label (in the language of `labels`
/// where it is a word, `Zip archive` and `Text`) and the extension, for the types the shell proposes (`None` for
/// others: no filter).
#[must_use]
pub fn save_filter(name: &str, labels: &Labels) -> Option<(String, String)> {
    let extension = crate::text_file::extension_of(name)?;
    let label = match extension.as_str() {
        "zip" => labels.file_filter_zip.as_str(),
        "md" => "Markdown",
        "html" => "HTML",
        "csv" => "CSV",
        "txt" => labels.file_filter_text.as_str(),
        "json" => "JSON",
        _ => return None,
    };
    Some((label.to_owned(), extension))
}

/// Asks where to save a file the shell makes (an export, the CSV of a view): the native save
/// dialog proposing `suggested` (a name the shell has already made safe), filtered on its
/// extension when it is a known one (`.md`, `.html`, `.zip`, `.csv`, ...). Cancelling is
/// `InvalidInput` for the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_save_path<R: Runtime>(
    app: &AppHandle<R>,
    suggested: &str,
) -> Result<PathBuf, ApiError> {
    let dialog = parented(app).set_file_name(suggested);
    let dialog = match save_filter(suggested, &labels_of(app)) {
        Some((label, extension)) => dialog.add_filter(label, &[extension.as_str()]),
        None => dialog,
    };
    chosen_path(dialog.blocking_save_file())
}

/// Asks which files to import: the native multi-select open dialog, filtered to Markdown, text,
/// HTML and CSV. Cancelling is `InvalidInput` for the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_import_files<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<PathBuf>, ApiError> {
    let dialog =
        parented(app).add_filter(labels_of(app).file_filter_import, &IMPORT_FILE_EXTENSIONS);
    let Some(choices) = dialog.blocking_pick_files() else {
        return Err(ApiError::invalid("path", "no file was chosen"));
    };
    if choices.is_empty() {
        return Err(ApiError::invalid("path", "no file was chosen"));
    }
    choices
        .into_iter()
        .map(|choice| chosen_path(Some(choice)))
        .collect()
}

/// Asks which folder to import: the native folder dialog. Cancelling is `InvalidInput` for the field
/// `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_import_folder<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    chosen_path(parented(app).blocking_pick_folder())
}

/// Asks which `.zip` to import: the native open dialog, filtered on `.zip`. Cancelling is
/// `InvalidInput` for the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_import_zip<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    chosen_path(
        parented(app)
            .add_filter(labels_of(app).file_filter_zip, &["zip"])
            .blocking_pick_file(),
    )
}

/// Asks which folder scheduled backups go to: the native folder dialog. Cancelling is
/// `InvalidInput` for the field `path`.
///
/// Blocking; call it from a thread that is not the main thread.
pub fn choose_backup_folder<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, ApiError> {
    chosen_path(parented(app).blocking_pick_folder())
}

// --- consent (ADR-0037) ---

/// A yes or no question the shell asks the person in a native dialog the webview cannot fake: the
/// consent to send text to another computer (`ai_endpoint_set`) and the confirmation that gives an AI
/// tool access to spaces (`mcp_client_grants_set`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The dialog's title.
    pub title: String,
    /// What is being asked, in full.
    pub message: String,
    /// The label of the button that says yes.
    pub accept: String,
    /// The label of the button that says no.
    pub cancel: String,
}

/// Something that can put a [`Question`] to the person. The commands take it as a trait object so
/// the tests can answer for the person; [`NativeConfirm`] is the real one.
pub trait Confirm: Send + Sync {
    /// Shows the question and waits for the answer: `true` when the person accepted. Blocking: call it
    /// from a thread that is not the main thread (the commands call it inside `spawn_blocking`).
    fn ask(&self, question: &Question) -> bool;

    /// The words the question is written in: what the UI last sent (`shell_labels_set`), read when the question
    /// is made, so a change of language reaches the next dialog. English for an asker with no shell (the tests).
    fn labels(&self) -> Labels {
        Labels::default()
    }
}

/// The native dialog, parented to the main window when there is one. Cancelling or closing the dialog
/// is a "no".
pub struct NativeConfirm<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> NativeConfirm<R> {
    /// A question that opens its dialog through `app`.
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> Confirm for NativeConfirm<R> {
    fn labels(&self) -> Labels {
        labels_of(&self.app)
    }

    fn ask(&self, question: &Question) -> bool {
        let mut dialog = self
            .app
            .dialog()
            .message(question.message.clone())
            .title(question.title.clone())
            .buttons(MessageDialogButtons::OkCancelCustom(
                question.accept.clone(),
                question.cancel.clone(),
            ));
        if let Some(window) = self.app.get_webview_window(MAIN_WINDOW) {
            dialog = dialog.parent(&window);
        }
        dialog.blocking_show()
    }
}

/// What a dialog answered, as a path: `None` is a cancelled dialog, a `file:` URL is
/// converted, anything the file system cannot open (an Android `content:` URI, say) is
/// refused.
fn chosen_path(choice: Option<FilePath>) -> Result<PathBuf, ApiError> {
    let Some(choice) = choice else {
        return Err(ApiError::invalid("path", "no file was chosen"));
    };
    choice
        .into_path()
        .map_err(|_| ApiError::invalid("path", "the chosen file is not on this computer"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use kb_types::ErrorCode;
    use tauri::Url;

    use super::*;

    fn field(error: &ApiError) -> Option<&str> {
        error.details.as_ref()?.get("field")?.as_str()
    }

    #[test]
    fn a_cancelled_dialog_is_invalid_input_for_the_path() {
        let error = chosen_path(None).unwrap_err();
        assert_eq!(error, ApiError::invalid("path", "no file was chosen"));
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), Some("path"));
    }

    #[test]
    fn a_chosen_path_is_used_as_it_is() {
        let path = std::env::temp_dir().join("chosen.atomikkb");
        assert_eq!(
            chosen_path(Some(FilePath::Path(path.clone()))).unwrap(),
            path
        );
    }

    #[test]
    fn a_file_url_is_converted_to_a_path() {
        let path = std::env::temp_dir().join("from-a-url.atomikkb");
        let url = Url::from_file_path(&path).unwrap();
        assert_eq!(chosen_path(Some(FilePath::Url(url))).unwrap(), path);
    }

    #[test]
    fn a_save_dialog_filters_on_the_extension_of_the_proposed_name() {
        let english = Labels::default();
        let some = |label: &str, extension: &str| Some((label.to_owned(), extension.to_owned()));
        assert_eq!(
            save_filter("Notes.zip", &english),
            some("Zip archive", "zip")
        );
        assert_eq!(save_filter("Notes.MD", &english), some("Markdown", "md"));
        assert_eq!(save_filter("Notes.html", &english), some("HTML", "html"));
        assert_eq!(save_filter("Tasks.csv", &english), some("CSV", "csv"));
        assert_eq!(save_filter("Tasks.txt", &english), some("Text", "txt"));
        assert_eq!(save_filter("Tasks.exe", &english), None);
        assert_eq!(save_filter("Tasks", &english), None);
    }

    #[test]
    fn the_words_of_a_file_type_are_the_uis() {
        let french = Labels {
            file_filter_zip: "Archive zip".to_owned(),
            file_filter_text: "Texte".to_owned(),
            ..Labels::default()
        };
        let some = |label: &str, extension: &str| Some((label.to_owned(), extension.to_owned()));
        assert_eq!(
            save_filter("Notes.zip", &french),
            some("Archive zip", "zip")
        );
        assert_eq!(save_filter("Tasks.txt", &french), some("Texte", "txt"));
        // Names that are not words stay as they are.
        assert_eq!(save_filter("Notes.md", &french), some("Markdown", "md"));
    }

    #[test]
    fn a_url_that_is_not_a_local_file_is_refused() {
        for url in [
            "content://com.android.providers/document/1",
            "https://example.com/x",
        ] {
            let error = chosen_path(Some(FilePath::Url(url.parse().unwrap()))).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{url}");
            assert_eq!(field(&error), Some("path"), "{url}");
        }
    }
}
