//! The words the shell shows in its own native surfaces (ADR-0040): the tray menu, the native menu bar
//! (`menu_bar`), the native consent dialogs and the file types of the file dialogs, so the strings can be translated
//! (item 13) without the shell knowing a language.
//!
//! The shell starts with English defaults ([`Labels::default`]); the UI sends its own strings with the command
//! `shell_labels_set` as soon as it starts, and again whenever its language changes. The message is a small
//! typed map, [`ShellLabels`]: the label ids `open`, `newPage`, `lock` and `quit` (the tray's), the ids that
//! start with `menu` (the menu bar's), `dialog*` (the consent dialogs) and `fileFilter*` (the file types), each
//! optional, so a message sets only the labels it names and a newer UI that names one this shell does not know is
//! refused, not guessed at (`deny_unknown_fields`).
//!
//! **Mnemonics.** The labels of the menu bar may carry an `&` before the letter that opens the item when Alt and
//! that letter are pressed (`&File`, `Cu&t`), the convention of the platform's own menus; `&&` is a literal `&`.
//! The tray's labels have none. Each menu's mnemonics must differ; that is the translator's care.
//!
//! **Dialog texts.** The consent dialogs (an AI endpoint that is another computer, an AI tool's access to spaces,
//! the download of the images of an import) are the shell's, because the page must not be able to fake them. Their
//! texts have `{placeholders}` that the shell fills (`fill`, in one pass, so a value is never read as a template), and
//! the placeholders a consent depends on are required: a text that drops `{host}`, `{client}` or `{spaces}` is
//! refused, because it would ask the person a question that hides what they agree to. A text is up to
//! [`MAX_TEXT_CHARS`] characters.
//!
//! **Validation.** A label is 1 to [`MAX_LABEL_CHARS`] characters after trimming, with no control character
//! (no line break, no tab, no NUL). One bad label refuses the whole message (`InvalidInput` for the field that
//! names it) and changes nothing. A label is user-facing text that a translator wrote: its value is never
//! logged and never put in an error.

use std::sync::{Mutex, PoisonError};

use kb_types::ApiError;
use serde::Deserialize;

/// The longest label, in characters. A menu item is one short line.
pub const MAX_LABEL_CHARS: usize = 64;

/// What the UI sends: the labels it wants changed, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShellLabels {
    /// The tray menu's "Open".
    pub open: Option<String>,
    /// The tray menu's "New page".
    pub new_page: Option<String>,
    /// The tray menu's "Lock".
    pub lock: Option<String>,
    /// The tray menu's "Quit".
    pub quit: Option<String>,
    /// The menu bar's "File" menu.
    pub menu_file: Option<String>,
    /// The menu bar's "Edit" menu.
    pub menu_edit: Option<String>,
    /// The menu bar's "View" menu.
    pub menu_view: Option<String>,
    /// The menu bar's "Window" menu.
    pub menu_window: Option<String>,
    /// The menu bar's "Help" menu.
    pub menu_help: Option<String>,
    /// File: new page.
    pub menu_new_page: Option<String>,
    /// File: import.
    pub menu_import: Option<String>,
    /// File: export.
    pub menu_export: Option<String>,
    /// File: print.
    pub menu_print: Option<String>,
    /// File: lock.
    pub menu_lock: Option<String>,
    /// File: quit.
    pub menu_quit: Option<String>,
    /// Edit: undo.
    pub menu_undo: Option<String>,
    /// Edit: redo.
    pub menu_redo: Option<String>,
    /// Edit: cut.
    pub menu_cut: Option<String>,
    /// Edit: copy.
    pub menu_copy: Option<String>,
    /// Edit: paste.
    pub menu_paste: Option<String>,
    /// Edit: select all.
    pub menu_select_all: Option<String>,
    /// Edit: find in the open page.
    pub menu_find: Option<String>,
    /// View: show or hide the sidebar.
    pub menu_toggle_sidebar: Option<String>,
    /// View: back in the active tab's history.
    pub menu_back: Option<String>,
    /// View: forward in the active tab's history.
    pub menu_forward: Option<String>,
    /// Window: minimize.
    pub menu_minimize: Option<String>,
    /// Window: close the window (to the tray while the setting says so).
    pub menu_close_window: Option<String>,
    /// Help: about.
    pub menu_about: Option<String>,
    /// The button that says yes in a consent dialog.
    pub dialog_allow: Option<String>,
    /// The button that says no in a consent dialog.
    pub dialog_cancel: Option<String>,
    /// The button that says yes in the image download dialog.
    pub dialog_yes: Option<String>,
    /// The button that says no in the image download dialog.
    pub dialog_no: Option<String>,
    /// What a name that is blank is called in a dialog (an AI tool is asked about a space with no name) and in the notification of a reminder on an untitled page.
    pub dialog_untitled: Option<String>,
    /// The title of the dialog that asks to send text to another computer.
    pub dialog_ai_title: Option<String>,
    /// That dialog's text: `{host}`.
    pub dialog_ai_message: Option<String>,
    /// The title of the dialog that asks to let an AI tool into spaces.
    pub dialog_mcp_title: Option<String>,
    /// That dialog's text when the tool can only read: `{client}`, `{spaces}`.
    pub dialog_mcp_read: Option<String>,
    /// That dialog's text when the tool can only change: `{client}`, `{spaces}`.
    pub dialog_mcp_write: Option<String>,
    /// That dialog's text when the tool can read some spaces and change others: `{client}`, `{read}`, `{write}`.
    pub dialog_mcp_both: Option<String>,
    /// How a list of names ends: `{names}` (comma-separated) and `{last}`.
    pub dialog_list_and: Option<String>,
    /// The last item of a list that was cut: `{count}`.
    pub dialog_list_more: Option<String>,
    /// The title of the dialog that asks to download the images of an import.
    pub dialog_images_title: Option<String>,
    /// That dialog's text for one image: `{count}`, `{stored}`.
    pub dialog_images_one: Option<String>,
    /// That dialog's text for several images: `{count}`, `{stored}`.
    pub dialog_images_other: Option<String>,
    /// Where the images are, with one other host: `{host}`, `{count}`.
    pub dialog_hosts_one: Option<String>,
    /// Where the images are, with several other hosts: `{host}`, `{count}`.
    pub dialog_hosts_other: Option<String>,
    /// The body of the OS notification of a reply in a thread this account wrote in (M13 slice 2): `{actor}` (required),
    /// `{page}`.
    pub dialog_notify_thread_reply: Option<String>,
    /// The body of the OS notification of a mention in a comment: `{actor}` (required), `{page}`.
    pub dialog_notify_comment_mention: Option<String>,
    /// The body of the OS notification of a mention in a page: `{actor}` (required), `{page}`.
    pub dialog_notify_page_mention: Option<String>,
    /// The body of the OS notification of being added to a Person property: `{actor}` (required), `{page}`.
    pub dialog_notify_person_assigned: Option<String>,
    /// The title of the dialog that asks to download a speech model (dictation, ADR-0053).
    pub dialog_dictation_title: Option<String>,
    /// That dialog's text: `{model}`, `{size}` (required), `{host}` (required).
    pub dialog_dictation_message: Option<String>,
    /// The button that says yes in that dialog.
    pub dialog_download: Option<String>,
    /// The title of the dialog that asks to send encrypted backups to Google Drive (ADR-0059).
    pub dialog_drive_title: Option<String>,
    /// That dialog's text: what leaves this computer and what Google can see; no placeholders.
    pub dialog_drive_message: Option<String>,
    /// The button that says yes in that dialog.
    pub dialog_drive_accept: Option<String>,
    /// The button that says no in that dialog.
    pub dialog_drive_cancel: Option<String>,
    /// What the page the browser lands on after signing in to Google says: "You can close this tab"; no placeholders.
    pub dialog_drive_done: Option<String>,
    /// The file type of a backup in the native file dialogs.
    pub file_filter_backup: Option<String>,
    /// The file type of a zip archive in the native file dialogs.
    pub file_filter_zip: Option<String>,
    /// The file type of a plain text file in the native file dialogs.
    pub file_filter_text: Option<String>,
    /// The file type of the files an import accepts in the native file dialogs.
    pub file_filter_import: Option<String>,
}

/// Every label the shell shows, resolved: English until the UI says otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Labels {
    /// The tray menu's "Open".
    pub open: String,
    /// The tray menu's "New page".
    pub new_page: String,
    /// The tray menu's "Lock".
    pub lock: String,
    /// The tray menu's "Quit".
    pub quit: String,
    /// The menu bar's "File" menu.
    pub menu_file: String,
    /// The menu bar's "Edit" menu.
    pub menu_edit: String,
    /// The menu bar's "View" menu.
    pub menu_view: String,
    /// The menu bar's "Window" menu.
    pub menu_window: String,
    /// The menu bar's "Help" menu.
    pub menu_help: String,
    /// File: new page.
    pub menu_new_page: String,
    /// File: import.
    pub menu_import: String,
    /// File: export.
    pub menu_export: String,
    /// File: print.
    pub menu_print: String,
    /// File: lock.
    pub menu_lock: String,
    /// File: quit.
    pub menu_quit: String,
    /// Edit: undo.
    pub menu_undo: String,
    /// Edit: redo.
    pub menu_redo: String,
    /// Edit: cut.
    pub menu_cut: String,
    /// Edit: copy.
    pub menu_copy: String,
    /// Edit: paste.
    pub menu_paste: String,
    /// Edit: select all.
    pub menu_select_all: String,
    /// Edit: find in the open page.
    pub menu_find: String,
    /// View: show or hide the sidebar.
    pub menu_toggle_sidebar: String,
    /// View: back in the active tab's history.
    pub menu_back: String,
    /// View: forward in the active tab's history.
    pub menu_forward: String,
    /// Window: minimize.
    pub menu_minimize: String,
    /// Window: close the window.
    pub menu_close_window: String,
    /// Help: about.
    pub menu_about: String,
    /// The button that says yes in a consent dialog.
    pub dialog_allow: String,
    /// The button that says no in a consent dialog.
    pub dialog_cancel: String,
    /// The button that says yes in the image download dialog.
    pub dialog_yes: String,
    /// The button that says no in the image download dialog.
    pub dialog_no: String,
    /// What a name that is blank is called in a dialog (an AI tool is asked about a space with no name) and in the notification of a reminder on an untitled page.
    pub dialog_untitled: String,
    /// The title of the dialog that asks to send text to another computer.
    pub dialog_ai_title: String,
    /// That dialog's text: `{host}`.
    pub dialog_ai_message: String,
    /// The title of the dialog that asks to let an AI tool into spaces.
    pub dialog_mcp_title: String,
    /// That dialog's text when the tool can only read: `{client}`, `{spaces}`.
    pub dialog_mcp_read: String,
    /// That dialog's text when the tool can only change: `{client}`, `{spaces}`.
    pub dialog_mcp_write: String,
    /// That dialog's text when the tool can read some spaces and change others: `{client}`, `{read}`, `{write}`.
    pub dialog_mcp_both: String,
    /// How a list of names ends: `{names}` (comma-separated) and `{last}`.
    pub dialog_list_and: String,
    /// The last item of a list that was cut: `{count}`.
    pub dialog_list_more: String,
    /// The title of the dialog that asks to download the images of an import.
    pub dialog_images_title: String,
    /// That dialog's text for one image: `{count}`, `{stored}`.
    pub dialog_images_one: String,
    /// That dialog's text for several images: `{count}`, `{stored}`.
    pub dialog_images_other: String,
    /// Where the images are, with one other host: `{host}`, `{count}`.
    pub dialog_hosts_one: String,
    /// Where the images are, with several other hosts: `{host}`, `{count}`.
    pub dialog_hosts_other: String,
    /// The body of the OS notification of a reply in a thread this account wrote in: `{actor}`, `{page}`.
    pub dialog_notify_thread_reply: String,
    /// The body of the OS notification of a mention in a comment: `{actor}`, `{page}`.
    pub dialog_notify_comment_mention: String,
    /// The body of the OS notification of a mention in a page: `{actor}`, `{page}`.
    pub dialog_notify_page_mention: String,
    /// The body of the OS notification of being added to a Person property: `{actor}`, `{page}`.
    pub dialog_notify_person_assigned: String,
    /// The title of the dialog that asks to download a speech model (dictation, ADR-0053).
    pub dialog_dictation_title: String,
    /// That dialog's text: `{model}`, `{size}`, `{host}`.
    pub dialog_dictation_message: String,
    /// The button that says yes in that dialog.
    pub dialog_download: String,
    /// The title of the dialog that asks to send encrypted backups to Google Drive (ADR-0059).
    pub dialog_drive_title: String,
    /// That dialog's text: what leaves this computer and what Google can see; no placeholders.
    pub dialog_drive_message: String,
    /// The button that says yes in that dialog.
    pub dialog_drive_accept: String,
    /// The button that says no in that dialog.
    pub dialog_drive_cancel: String,
    /// What the page the browser lands on after signing in to Google says: "You can close this tab"; no placeholders.
    pub dialog_drive_done: String,
    /// The file type of a backup in the native file dialogs.
    pub file_filter_backup: String,
    /// The file type of a zip archive in the native file dialogs.
    pub file_filter_zip: String,
    /// The file type of a plain text file in the native file dialogs.
    pub file_filter_text: String,
    /// The file type of the files an import accepts in the native file dialogs.
    pub file_filter_import: String,
}

/// The English defaults, which are also `tray.open`, `tray.newPage`, `tray.lock` and `tray.quit` of the UI's
/// `en.json` (a test of the desktop app keeps the two the same).
pub const DEFAULT_OPEN: &str = "Open atomik-kb";
/// See [`DEFAULT_OPEN`].
pub const DEFAULT_NEW_PAGE: &str = "New page";
/// See [`DEFAULT_OPEN`].
pub const DEFAULT_LOCK: &str = "Lock";
/// See [`DEFAULT_OPEN`].
pub const DEFAULT_QUIT: &str = "Quit";

/// The English defaults of the menu bar, which are the `menu.*` strings of the UI's `en.json` (`menuNewPage` is
/// `menu.newPage`; a test of the desktop app keeps the two the same).
pub const DEFAULT_MENU_FILE: &str = "&File";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_EDIT: &str = "&Edit";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_VIEW: &str = "&View";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_WINDOW: &str = "&Window";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_HELP: &str = "&Help";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_NEW_PAGE: &str = "&New page";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_IMPORT: &str = "&Import\u{2026}";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_EXPORT: &str = "&Export\u{2026}";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_PRINT: &str = "&Print\u{2026}";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_LOCK: &str = "&Lock";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_QUIT: &str = "&Quit";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_UNDO: &str = "&Undo";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_REDO: &str = "&Redo";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_CUT: &str = "Cu&t";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_COPY: &str = "&Copy";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_PASTE: &str = "&Paste";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_SELECT_ALL: &str = "Select &all";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_FIND: &str = "&Find in page";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_TOGGLE_SIDEBAR: &str = "Toggle &sidebar";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_BACK: &str = "&Back";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_FORWARD: &str = "&Forward";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_MINIMIZE: &str = "&Minimize";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_CLOSE_WINDOW: &str = "&Close window";
/// See [`DEFAULT_MENU_FILE`].
pub const DEFAULT_MENU_ABOUT: &str = "&About atomik-kb";

/// The English defaults of the dialogs and file types (`shellDialog.*` and `fileFilter.*` of the UI's `en.json`; a test of
/// the desktop app keeps the two the same). The texts with `{placeholders}` are filled by [`fill`].
pub const DEFAULT_DIALOG_ALLOW: &str = "Allow";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_CANCEL: &str = "Cancel";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_YES: &str = "Yes";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_NO: &str = "No";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_UNTITLED: &str = "Untitled";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_AI_TITLE: &str = "Send text to another computer?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_AI_MESSAGE: &str = "AI features will send the text of your pages, your questions and your recordings to {host}. Anyone who controls that computer can read them.";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_MCP_TITLE: &str = "Allow access to your pages?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_MCP_READ: &str = "Allow {client} to read {spaces}?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_MCP_WRITE: &str = "Allow {client} to change {spaces}?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_MCP_BOTH: &str = "Allow {client} to read {read} and change {write}?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_LIST_AND: &str = "{names} and {last}";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_LIST_MORE: &str = "{count} more";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_IMAGES_TITLE: &str = "Download images?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_IMAGES_ONE: &str = "This import has {count} image stored on {stored}. Download it now? atomik-kb will connect to those servers once; nothing else is sent.";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_IMAGES_OTHER: &str = "This import has {count} images stored on {stored}. Download them now? atomik-kb will connect to those servers once; nothing else is sent.";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_HOSTS_ONE: &str = "{host} (and {count} other host)";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_HOSTS_OTHER: &str = "{host} (and {count} other hosts)";
/// See [`DEFAULT_DIALOG_ALLOW`]. The body of the OS notifications of comments and mentions (the title is the page's): a phrase
/// with the actor's name, never the words of a comment.
pub const DEFAULT_DIALOG_NOTIFY_THREAD_REPLY: &str = "{actor} replied to your thread";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_NOTIFY_COMMENT_MENTION: &str = "{actor} mentioned you in a comment";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_NOTIFY_PAGE_MENTION: &str = "{actor} mentioned you";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_NOTIFY_PERSON_ASSIGNED: &str = "{actor} assigned you";
/// See [`DEFAULT_DIALOG_ALLOW`]. The consent to download a speech model (dictation, ADR-0053): it names the model, its size and the
/// host that will see the computer's address.
pub const DEFAULT_DIALOG_DICTATION_TITLE: &str = "Download a speech model?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_DICTATION_MESSAGE: &str = "Dictation needs to download {model} ({size}) from {host}. {host} will see this computer's IP address and which model you download. The files are checked against a fixed fingerprint, and your recordings are never stored or sent anywhere.";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_DOWNLOAD: &str = "Download";
/// See [`DEFAULT_DIALOG_ALLOW`]. The consent to send encrypted backups to Google Drive (ADR-0059): what leaves this computer and
/// what Google can see.
pub const DEFAULT_DIALOG_DRIVE_TITLE: &str = "Connect Google Drive?";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_DRIVE_MESSAGE: &str = "atomik-kb will upload your encrypted backups to a folder in your Google Drive. Google will see your Google account, the names, sizes and times of the backup files and a small unencrypted header in each, but cannot read what you wrote. You can disconnect at any time; the files stay in Drive.";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_DRIVE_ACCEPT: &str = "Connect";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_DIALOG_DRIVE_CANCEL: &str = "Cancel";
/// See [`DEFAULT_DIALOG_ALLOW`]. The page the browser shows once Google has sent it back to the app.
pub const DEFAULT_DIALOG_DRIVE_DONE: &str = "You can close this tab and go back to atomik-kb.";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_FILE_FILTER_BACKUP: &str = "atomik-kb backup";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_FILE_FILTER_ZIP: &str = "Zip archive";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_FILE_FILTER_TEXT: &str = "Text";
/// See [`DEFAULT_DIALOG_ALLOW`].
pub const DEFAULT_FILE_FILTER_IMPORT: &str = "Markdown, text, HTML and CSV";

impl Default for Labels {
    fn default() -> Self {
        let text = |text: &str| text.to_owned();
        Self {
            open: text(DEFAULT_OPEN),
            new_page: text(DEFAULT_NEW_PAGE),
            lock: text(DEFAULT_LOCK),
            quit: text(DEFAULT_QUIT),
            menu_file: text(DEFAULT_MENU_FILE),
            menu_edit: text(DEFAULT_MENU_EDIT),
            menu_view: text(DEFAULT_MENU_VIEW),
            menu_window: text(DEFAULT_MENU_WINDOW),
            menu_help: text(DEFAULT_MENU_HELP),
            menu_new_page: text(DEFAULT_MENU_NEW_PAGE),
            menu_import: text(DEFAULT_MENU_IMPORT),
            menu_export: text(DEFAULT_MENU_EXPORT),
            menu_print: text(DEFAULT_MENU_PRINT),
            menu_lock: text(DEFAULT_MENU_LOCK),
            menu_quit: text(DEFAULT_MENU_QUIT),
            menu_undo: text(DEFAULT_MENU_UNDO),
            menu_redo: text(DEFAULT_MENU_REDO),
            menu_cut: text(DEFAULT_MENU_CUT),
            menu_copy: text(DEFAULT_MENU_COPY),
            menu_paste: text(DEFAULT_MENU_PASTE),
            menu_select_all: text(DEFAULT_MENU_SELECT_ALL),
            menu_find: text(DEFAULT_MENU_FIND),
            menu_toggle_sidebar: text(DEFAULT_MENU_TOGGLE_SIDEBAR),
            menu_back: text(DEFAULT_MENU_BACK),
            menu_forward: text(DEFAULT_MENU_FORWARD),
            menu_minimize: text(DEFAULT_MENU_MINIMIZE),
            menu_close_window: text(DEFAULT_MENU_CLOSE_WINDOW),
            menu_about: text(DEFAULT_MENU_ABOUT),
            dialog_allow: text(DEFAULT_DIALOG_ALLOW),
            dialog_cancel: text(DEFAULT_DIALOG_CANCEL),
            dialog_yes: text(DEFAULT_DIALOG_YES),
            dialog_no: text(DEFAULT_DIALOG_NO),
            dialog_untitled: text(DEFAULT_DIALOG_UNTITLED),
            dialog_ai_title: text(DEFAULT_DIALOG_AI_TITLE),
            dialog_ai_message: text(DEFAULT_DIALOG_AI_MESSAGE),
            dialog_mcp_title: text(DEFAULT_DIALOG_MCP_TITLE),
            dialog_mcp_read: text(DEFAULT_DIALOG_MCP_READ),
            dialog_mcp_write: text(DEFAULT_DIALOG_MCP_WRITE),
            dialog_mcp_both: text(DEFAULT_DIALOG_MCP_BOTH),
            dialog_list_and: text(DEFAULT_DIALOG_LIST_AND),
            dialog_list_more: text(DEFAULT_DIALOG_LIST_MORE),
            dialog_images_title: text(DEFAULT_DIALOG_IMAGES_TITLE),
            dialog_images_one: text(DEFAULT_DIALOG_IMAGES_ONE),
            dialog_images_other: text(DEFAULT_DIALOG_IMAGES_OTHER),
            dialog_hosts_one: text(DEFAULT_DIALOG_HOSTS_ONE),
            dialog_hosts_other: text(DEFAULT_DIALOG_HOSTS_OTHER),
            dialog_notify_thread_reply: text(DEFAULT_DIALOG_NOTIFY_THREAD_REPLY),
            dialog_notify_comment_mention: text(DEFAULT_DIALOG_NOTIFY_COMMENT_MENTION),
            dialog_notify_page_mention: text(DEFAULT_DIALOG_NOTIFY_PAGE_MENTION),
            dialog_notify_person_assigned: text(DEFAULT_DIALOG_NOTIFY_PERSON_ASSIGNED),
            dialog_dictation_title: text(DEFAULT_DIALOG_DICTATION_TITLE),
            dialog_dictation_message: text(DEFAULT_DIALOG_DICTATION_MESSAGE),
            dialog_download: text(DEFAULT_DIALOG_DOWNLOAD),
            dialog_drive_title: text(DEFAULT_DIALOG_DRIVE_TITLE),
            dialog_drive_message: text(DEFAULT_DIALOG_DRIVE_MESSAGE),
            dialog_drive_accept: text(DEFAULT_DIALOG_DRIVE_ACCEPT),
            dialog_drive_cancel: text(DEFAULT_DIALOG_DRIVE_CANCEL),
            dialog_drive_done: text(DEFAULT_DIALOG_DRIVE_DONE),
            file_filter_backup: text(DEFAULT_FILE_FILTER_BACKUP),
            file_filter_zip: text(DEFAULT_FILE_FILTER_ZIP),
            file_filter_text: text(DEFAULT_FILE_FILTER_TEXT),
            file_filter_import: text(DEFAULT_FILE_FILTER_IMPORT),
        }
    }
}

/// The longest text of a dialog, in characters: a question of a sentence or two.
pub const MAX_TEXT_CHARS: usize = 400;

/// What a dialog text must look like: which `{placeholders}` it may use and which it must use. The ones a
/// consent depends on (the computer's address, the tool, the spaces) are required, so a translation that drops
/// one is refused instead of asking the person a question that hides what they agree to.
struct TemplateSpec {
    field: &'static str,
    allowed: &'static [&'static str],
    required: &'static [&'static str],
}

/// The rules of `dialogAiMessage`.
const DIALOG_AI_MESSAGE: TemplateSpec = TemplateSpec {
    field: "dialogAiMessage",
    allowed: &["host"],
    required: &["host"],
};

/// The rules of `dialogMcpRead`.
const DIALOG_MCP_READ: TemplateSpec = TemplateSpec {
    field: "dialogMcpRead",
    allowed: &["client", "spaces"],
    required: &["client", "spaces"],
};

/// The rules of `dialogMcpWrite`.
const DIALOG_MCP_WRITE: TemplateSpec = TemplateSpec {
    field: "dialogMcpWrite",
    allowed: &["client", "spaces"],
    required: &["client", "spaces"],
};

/// The rules of `dialogMcpBoth`.
const DIALOG_MCP_BOTH: TemplateSpec = TemplateSpec {
    field: "dialogMcpBoth",
    allowed: &["client", "read", "write"],
    required: &["client", "read", "write"],
};

/// The rules of `dialogListAnd`.
const DIALOG_LIST_AND: TemplateSpec = TemplateSpec {
    field: "dialogListAnd",
    allowed: &["names", "last"],
    required: &["names", "last"],
};

/// The rules of `dialogListMore`.
const DIALOG_LIST_MORE: TemplateSpec = TemplateSpec {
    field: "dialogListMore",
    allowed: &["count"],
    required: &["count"],
};

/// The rules of `dialogImagesOne`.
const DIALOG_IMAGES_ONE: TemplateSpec = TemplateSpec {
    field: "dialogImagesOne",
    allowed: &["count", "stored"],
    required: &["stored"],
};

/// The rules of `dialogImagesOther`.
const DIALOG_IMAGES_OTHER: TemplateSpec = TemplateSpec {
    field: "dialogImagesOther",
    allowed: &["count", "stored"],
    required: &["count", "stored"],
};

/// The rules of `dialogHostsOne`.
const DIALOG_HOSTS_ONE: TemplateSpec = TemplateSpec {
    field: "dialogHostsOne",
    allowed: &["host", "count"],
    required: &["host"],
};

/// The rules of `dialogHostsOther`.
const DIALOG_HOSTS_OTHER: TemplateSpec = TemplateSpec {
    field: "dialogHostsOther",
    allowed: &["host", "count"],
    required: &["host", "count"],
};

/// The rules of `dialogNotifyThreadReply`: it names the person who replied (`{actor}`) and may name the page (`{page}`).
const DIALOG_NOTIFY_THREAD_REPLY: TemplateSpec = TemplateSpec {
    field: "dialogNotifyThreadReply",
    allowed: &["actor", "page"],
    required: &["actor"],
};

/// The rules of `dialogNotifyCommentMention`.
const DIALOG_NOTIFY_COMMENT_MENTION: TemplateSpec = TemplateSpec {
    field: "dialogNotifyCommentMention",
    allowed: &["actor", "page"],
    required: &["actor"],
};

/// The rules of `dialogNotifyPageMention`.
const DIALOG_NOTIFY_PAGE_MENTION: TemplateSpec = TemplateSpec {
    field: "dialogNotifyPageMention",
    allowed: &["actor", "page"],
    required: &["actor"],
};

/// The rules of `dialogNotifyPersonAssigned`.
const DIALOG_NOTIFY_PERSON_ASSIGNED: TemplateSpec = TemplateSpec {
    field: "dialogNotifyPersonAssigned",
    allowed: &["actor", "page"],
    required: &["actor"],
};

/// The rules of `dialogDictationMessage`: it names the model (`{model}`), and must say how big the download is
/// (`{size}`) and which host sees the computer's address (`{host}`).
const DIALOG_DICTATION_MESSAGE: TemplateSpec = TemplateSpec {
    field: "dialogDictationMessage",
    allowed: &["model", "size", "host"],
    required: &["size", "host"],
};

/// The rules of `dialogDriveMessage`: a text of its own words, with no placeholder (nothing in it is filled).
const DIALOG_DRIVE_MESSAGE: TemplateSpec = TemplateSpec {
    field: "dialogDriveMessage",
    allowed: &[],
    required: &[],
};

/// The rules of `dialogDriveDone`: the same, for the page the browser lands on.
const DIALOG_DRIVE_DONE: TemplateSpec = TemplateSpec {
    field: "dialogDriveDone",
    allowed: &[],
    required: &[],
};

/// A piece of a template: text as written, or the name of a `{placeholder}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Name(&'a str),
}

/// Splits `text` into literal text and `{name}` placeholders (a name is one or more ASCII letters). A brace that
/// does not close such a name is literal text.
fn pieces(text: &str) -> Vec<Piece<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut literal = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes.get(at) == Some(&b'{') {
            let mut end = at + 1;
            while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
                end += 1;
            }
            if end > at + 1 && bytes.get(end) == Some(&b'}') {
                if literal < at {
                    out.push(Piece::Text(&text[literal..at]));
                }
                out.push(Piece::Name(&text[at + 1..end]));
                at = end + 1;
                literal = at;
                continue;
            }
        }
        at += 1;
    }
    if literal < text.len() {
        out.push(Piece::Text(&text[literal..]));
    }
    out
}

/// `template` with each `{name}` that `values` names replaced by its value, in one pass: a value is never read
/// again, so a name or a host that happens to contain `{host}` stays as it is. A placeholder `values` does not name
/// is left as written.
#[must_use]
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    for piece in pieces(template) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Name(name) => match values.iter().find(|(key, _)| *key == name) {
                Some((_, value)) => out.push_str(value),
                None => {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                }
            },
        }
    }
    out
}

/// `text` as a dialog template: trimmed, 1 to [`MAX_TEXT_CHARS`] characters, no control character, using only the
/// placeholders `spec` allows and all the ones it requires.
fn validated_template(spec: &TemplateSpec, text: &str) -> Result<String, ApiError> {
    let template = text.trim();
    let length = template.chars().count();
    let names: Vec<&str> = pieces(template)
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Name(name) => Some(name),
            Piece::Text(_) => None,
        })
        .collect();
    let sound = length > 0
        && length <= MAX_TEXT_CHARS
        && !template.chars().any(char::is_control)
        && names.iter().all(|name| spec.allowed.contains(name))
        && spec.required.iter().all(|name| names.contains(name));
    if !sound {
        // Never the text itself.
        return Err(ApiError::invalid(
            spec.field,
            "a dialog text is 1 to 400 characters, without control characters, and keeps the placeholders it needs",
        ));
    }
    Ok(template.to_owned())
}

/// `text` as a label: trimmed, 1 to [`MAX_LABEL_CHARS`] characters, no control character.
fn validated(field: &str, text: &str) -> Result<String, ApiError> {
    let label = text.trim();
    let length = label.chars().count();
    if length == 0 || length > MAX_LABEL_CHARS || label.chars().any(char::is_control) {
        // Never the text itself.
        return Err(ApiError::invalid(
            field,
            "a label is 1 to 64 characters, without line breaks or other control characters",
        ));
    }
    Ok(label.to_owned())
}

impl Labels {
    /// These labels with the ones `update` names replaced.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for the field (`open`, `menuFile`, ...) of the first label that is not valid; nothing is
    /// returned then, so nothing changes.
    pub fn updated(&self, update: &ShellLabels) -> Result<Self, ApiError> {
        let mut next = self.clone();
        for (field, slot, new) in [
            ("open", &mut next.open, &update.open),
            ("newPage", &mut next.new_page, &update.new_page),
            ("lock", &mut next.lock, &update.lock),
            ("quit", &mut next.quit, &update.quit),
            ("menuFile", &mut next.menu_file, &update.menu_file),
            ("menuEdit", &mut next.menu_edit, &update.menu_edit),
            ("menuView", &mut next.menu_view, &update.menu_view),
            ("menuWindow", &mut next.menu_window, &update.menu_window),
            ("menuHelp", &mut next.menu_help, &update.menu_help),
            (
                "menuNewPage",
                &mut next.menu_new_page,
                &update.menu_new_page,
            ),
            ("menuImport", &mut next.menu_import, &update.menu_import),
            ("menuExport", &mut next.menu_export, &update.menu_export),
            ("menuPrint", &mut next.menu_print, &update.menu_print),
            ("menuLock", &mut next.menu_lock, &update.menu_lock),
            ("menuQuit", &mut next.menu_quit, &update.menu_quit),
            ("menuUndo", &mut next.menu_undo, &update.menu_undo),
            ("menuRedo", &mut next.menu_redo, &update.menu_redo),
            ("menuCut", &mut next.menu_cut, &update.menu_cut),
            ("menuCopy", &mut next.menu_copy, &update.menu_copy),
            ("menuPaste", &mut next.menu_paste, &update.menu_paste),
            (
                "menuSelectAll",
                &mut next.menu_select_all,
                &update.menu_select_all,
            ),
            ("menuFind", &mut next.menu_find, &update.menu_find),
            (
                "menuToggleSidebar",
                &mut next.menu_toggle_sidebar,
                &update.menu_toggle_sidebar,
            ),
            ("menuBack", &mut next.menu_back, &update.menu_back),
            ("menuForward", &mut next.menu_forward, &update.menu_forward),
            (
                "menuMinimize",
                &mut next.menu_minimize,
                &update.menu_minimize,
            ),
            (
                "menuCloseWindow",
                &mut next.menu_close_window,
                &update.menu_close_window,
            ),
            ("menuAbout", &mut next.menu_about, &update.menu_about),
            ("dialogAllow", &mut next.dialog_allow, &update.dialog_allow),
            (
                "dialogCancel",
                &mut next.dialog_cancel,
                &update.dialog_cancel,
            ),
            ("dialogYes", &mut next.dialog_yes, &update.dialog_yes),
            ("dialogNo", &mut next.dialog_no, &update.dialog_no),
            (
                "dialogUntitled",
                &mut next.dialog_untitled,
                &update.dialog_untitled,
            ),
            (
                "dialogAiTitle",
                &mut next.dialog_ai_title,
                &update.dialog_ai_title,
            ),
            (
                "dialogMcpTitle",
                &mut next.dialog_mcp_title,
                &update.dialog_mcp_title,
            ),
            (
                "dialogImagesTitle",
                &mut next.dialog_images_title,
                &update.dialog_images_title,
            ),
            (
                "dialogDictationTitle",
                &mut next.dialog_dictation_title,
                &update.dialog_dictation_title,
            ),
            (
                "dialogDownload",
                &mut next.dialog_download,
                &update.dialog_download,
            ),
            (
                "dialogDriveTitle",
                &mut next.dialog_drive_title,
                &update.dialog_drive_title,
            ),
            (
                "dialogDriveAccept",
                &mut next.dialog_drive_accept,
                &update.dialog_drive_accept,
            ),
            (
                "dialogDriveCancel",
                &mut next.dialog_drive_cancel,
                &update.dialog_drive_cancel,
            ),
            (
                "fileFilterBackup",
                &mut next.file_filter_backup,
                &update.file_filter_backup,
            ),
            (
                "fileFilterZip",
                &mut next.file_filter_zip,
                &update.file_filter_zip,
            ),
            (
                "fileFilterText",
                &mut next.file_filter_text,
                &update.file_filter_text,
            ),
            (
                "fileFilterImport",
                &mut next.file_filter_import,
                &update.file_filter_import,
            ),
        ] {
            if let Some(text) = new {
                *slot = validated(field, text)?;
            }
        }
        for (slot, new, spec) in [
            (
                &mut next.dialog_ai_message,
                &update.dialog_ai_message,
                &DIALOG_AI_MESSAGE,
            ),
            (
                &mut next.dialog_mcp_read,
                &update.dialog_mcp_read,
                &DIALOG_MCP_READ,
            ),
            (
                &mut next.dialog_mcp_write,
                &update.dialog_mcp_write,
                &DIALOG_MCP_WRITE,
            ),
            (
                &mut next.dialog_mcp_both,
                &update.dialog_mcp_both,
                &DIALOG_MCP_BOTH,
            ),
            (
                &mut next.dialog_list_and,
                &update.dialog_list_and,
                &DIALOG_LIST_AND,
            ),
            (
                &mut next.dialog_list_more,
                &update.dialog_list_more,
                &DIALOG_LIST_MORE,
            ),
            (
                &mut next.dialog_images_one,
                &update.dialog_images_one,
                &DIALOG_IMAGES_ONE,
            ),
            (
                &mut next.dialog_images_other,
                &update.dialog_images_other,
                &DIALOG_IMAGES_OTHER,
            ),
            (
                &mut next.dialog_hosts_one,
                &update.dialog_hosts_one,
                &DIALOG_HOSTS_ONE,
            ),
            (
                &mut next.dialog_hosts_other,
                &update.dialog_hosts_other,
                &DIALOG_HOSTS_OTHER,
            ),
            (
                &mut next.dialog_notify_thread_reply,
                &update.dialog_notify_thread_reply,
                &DIALOG_NOTIFY_THREAD_REPLY,
            ),
            (
                &mut next.dialog_notify_comment_mention,
                &update.dialog_notify_comment_mention,
                &DIALOG_NOTIFY_COMMENT_MENTION,
            ),
            (
                &mut next.dialog_notify_page_mention,
                &update.dialog_notify_page_mention,
                &DIALOG_NOTIFY_PAGE_MENTION,
            ),
            (
                &mut next.dialog_notify_person_assigned,
                &update.dialog_notify_person_assigned,
                &DIALOG_NOTIFY_PERSON_ASSIGNED,
            ),
            (
                &mut next.dialog_dictation_message,
                &update.dialog_dictation_message,
                &DIALOG_DICTATION_MESSAGE,
            ),
            (
                &mut next.dialog_drive_message,
                &update.dialog_drive_message,
                &DIALOG_DRIVE_MESSAGE,
            ),
            (
                &mut next.dialog_drive_done,
                &update.dialog_drive_done,
                &DIALOG_DRIVE_DONE,
            ),
        ] {
            if let Some(text) = new {
                *slot = validated_template(spec, text)?;
            }
        }
        Ok(next)
    }
}

/// The labels the shell shows now. Managed by Tauri inside an `Arc`.
#[derive(Debug, Default)]
pub struct LabelStore {
    labels: Mutex<Labels>,
}

impl LabelStore {
    /// The English defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The labels now.
    #[must_use]
    pub fn get(&self) -> Labels {
        self.labels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Applies `update` and answers the labels that result, or refuses it whole.
    ///
    /// # Errors
    ///
    /// See [`Labels::updated`].
    pub fn update(&self, update: &ShellLabels) -> Result<Labels, ApiError> {
        let mut labels = self.labels.lock().unwrap_or_else(PoisonError::into_inner);
        let next = labels.updated(update)?;
        labels.clone_from(&next);
        Ok(next)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use kb_types::ErrorCode;
    use serde_json::json;

    use super::*;

    fn named(open: Option<&str>, new_page: Option<&str>) -> ShellLabels {
        ShellLabels {
            open: open.map(str::to_owned),
            new_page: new_page.map(str::to_owned),
            ..ShellLabels::default()
        }
    }

    fn field_of(error: &ApiError) -> String {
        assert_eq!(error.code, ErrorCode::InvalidInput);
        error.details.as_ref().unwrap()["field"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Every menu label id with its English default, in the order of the struct.
    fn menu_ids() -> Vec<(&'static str, &'static str)> {
        vec![
            ("menuFile", DEFAULT_MENU_FILE),
            ("menuEdit", DEFAULT_MENU_EDIT),
            ("menuView", DEFAULT_MENU_VIEW),
            ("menuWindow", DEFAULT_MENU_WINDOW),
            ("menuHelp", DEFAULT_MENU_HELP),
            ("menuNewPage", DEFAULT_MENU_NEW_PAGE),
            ("menuImport", DEFAULT_MENU_IMPORT),
            ("menuExport", DEFAULT_MENU_EXPORT),
            ("menuPrint", DEFAULT_MENU_PRINT),
            ("menuLock", DEFAULT_MENU_LOCK),
            ("menuQuit", DEFAULT_MENU_QUIT),
            ("menuUndo", DEFAULT_MENU_UNDO),
            ("menuRedo", DEFAULT_MENU_REDO),
            ("menuCut", DEFAULT_MENU_CUT),
            ("menuCopy", DEFAULT_MENU_COPY),
            ("menuPaste", DEFAULT_MENU_PASTE),
            ("menuSelectAll", DEFAULT_MENU_SELECT_ALL),
            ("menuFind", DEFAULT_MENU_FIND),
            ("menuToggleSidebar", DEFAULT_MENU_TOGGLE_SIDEBAR),
            ("menuBack", DEFAULT_MENU_BACK),
            ("menuForward", DEFAULT_MENU_FORWARD),
            ("menuMinimize", DEFAULT_MENU_MINIMIZE),
            ("menuCloseWindow", DEFAULT_MENU_CLOSE_WINDOW),
            ("menuAbout", DEFAULT_MENU_ABOUT),
        ]
    }

    #[test]
    fn the_defaults_are_english() {
        let labels = Labels::default();
        assert_eq!(labels.open, "Open atomik-kb");
        assert_eq!(labels.new_page, "New page");
        assert_eq!(labels.lock, "Lock");
        assert_eq!(labels.quit, "Quit");
        assert_eq!(labels.menu_file, "&File");
        assert_eq!(labels.menu_import, "&Import\u{2026}");
        assert_eq!(labels.menu_cut, "Cu&t");
        assert_eq!(labels.menu_about, "&About atomik-kb");
    }

    #[test]
    fn the_message_is_a_map_of_label_ids_to_strings() {
        let all: ShellLabels = serde_json::from_value(json!({
            "open": "Ouvrir", "newPage": "Nouvelle page", "lock": "Verrouiller", "quit": "Quitter"
        }))
        .unwrap();
        assert_eq!(all.open.as_deref(), Some("Ouvrir"));
        assert_eq!(all.new_page.as_deref(), Some("Nouvelle page"));
        assert_eq!(all.lock.as_deref(), Some("Verrouiller"));
        assert_eq!(all.quit.as_deref(), Some("Quitter"));

        let some: ShellLabels = serde_json::from_value(json!({ "quit": "Salir" })).unwrap();
        assert_eq!(some.quit.as_deref(), Some("Salir"));
        assert_eq!(some.open, None);

        let none: ShellLabels = serde_json::from_value(json!({})).unwrap();
        assert_eq!(none, ShellLabels::default());
    }

    #[test]
    fn the_menu_bar_has_its_own_label_ids() {
        let message = menu_ids()
            .into_iter()
            .map(|(id, _)| (id.to_owned(), json!(format!("<{id}>"))))
            .collect::<serde_json::Map<_, _>>();
        let all: ShellLabels = serde_json::from_value(message.into()).unwrap();
        let next = Labels::default().updated(&all).unwrap();
        assert_eq!(next.menu_file, "<menuFile>");
        assert_eq!(next.menu_toggle_sidebar, "<menuToggleSidebar>");
        assert_eq!(next.menu_close_window, "<menuCloseWindow>");
        assert_eq!(next.menu_about, "<menuAbout>");
        // The tray's four are untouched.
        assert_eq!(next.open, DEFAULT_OPEN);
        assert_eq!(next.quit, DEFAULT_QUIT);
    }

    #[test]
    fn an_id_this_shell_does_not_know_is_refused() {
        for message in [
            json!({ "nope": "x" }),
            json!({ "open": "Open", "new_page": "New page" }),
            json!({ "Open": "Open" }),
            // The menu ids are camelCase too.
            json!({ "menu_file": "File" }),
            json!({ "menuFiles": "File" }),
            json!({ "menu": "File" }),
        ] {
            assert!(
                serde_json::from_value::<ShellLabels>(message.clone()).is_err(),
                "{message}"
            );
        }
        // And a label is text.
        for message in [
            json!({ "open": 5 }),
            json!({ "open": null, "quit": ["Quit"] }),
            json!({ "menuFile": 5 }),
            json!({ "menuEdit": { "text": "Edit" } }),
        ] {
            assert!(
                serde_json::from_value::<ShellLabels>(message.clone()).is_err(),
                "{message}"
            );
        }
    }

    #[test]
    fn a_message_changes_only_the_labels_it_names() {
        let next = Labels::default()
            .updated(&named(Some("Ouvrir"), None))
            .unwrap();
        assert_eq!(next.open, "Ouvrir");
        assert_eq!(next.new_page, DEFAULT_NEW_PAGE);
        assert_eq!(next.lock, DEFAULT_LOCK);
        assert_eq!(next.quit, DEFAULT_QUIT);
        // And a second message keeps what the first set.
        let later = next.updated(&named(None, Some("Nouvelle page"))).unwrap();
        assert_eq!(later.open, "Ouvrir");
        assert_eq!(later.new_page, "Nouvelle page");

        // The same for a menu label: the others keep their words.
        let menu = ShellLabels {
            menu_edit: Some("&Edition".to_owned()),
            ..ShellLabels::default()
        };
        let next = later.updated(&menu).unwrap();
        assert_eq!(next.menu_edit, "&Edition");
        assert_eq!(next.menu_file, DEFAULT_MENU_FILE);
        assert_eq!(next.open, "Ouvrir");
    }

    #[test]
    fn labels_are_trimmed() {
        let next = Labels::default()
            .updated(&named(Some("  Ouvrir \t"), None))
            .unwrap();
        assert_eq!(next.open, "Ouvrir");
    }

    #[test]
    fn a_label_is_one_to_sixty_four_characters() {
        for empty in ["", "   ", "\u{a0}\u{3000}"] {
            // A non-breaking space is whitespace for `trim`: a label of nothing else is empty.
            let error = Labels::default()
                .updated(&named(Some(empty), None))
                .unwrap_err();
            assert_eq!(field_of(&error), "open", "{empty:?}");
        }
        let longest = "\u{e9}".repeat(MAX_LABEL_CHARS);
        assert!(longest.len() > MAX_LABEL_CHARS, "counted in characters");
        assert_eq!(
            Labels::default()
                .updated(&named(Some(&longest), None))
                .unwrap()
                .open,
            longest
        );
        let too_long = "a".repeat(MAX_LABEL_CHARS + 1);
        let error = Labels::default()
            .updated(&named(None, Some(&too_long)))
            .unwrap_err();
        assert_eq!(field_of(&error), "newPage");
    }

    #[test]
    fn every_menu_label_is_validated_and_the_error_names_its_field() {
        // A bad value is refused for exactly its own field.
        let too_long = "x".repeat(MAX_LABEL_CHARS + 1);
        for (id, _) in menu_ids() {
            for bad in [
                "",
                "   ",
                "Ab\ncd",
                "Ab\tcd",
                "Ab\u{0}cd",
                too_long.as_str(),
            ] {
                let message: ShellLabels = serde_json::from_value(json!({ id: bad })).unwrap();
                let error = Labels::default().updated(&message).unwrap_err();
                assert_eq!(field_of(&error), id, "{id}: {bad:?}");
            }
            // And a good one, with a mnemonic, a literal ampersand and non-Latin text, is kept as written.
            for good in ["&Datei", "Q&&A", "\u{30d5}\u{30a1}\u{30a4}\u{30eb}(&F)"] {
                let message: ShellLabels = serde_json::from_value(json!({ id: good })).unwrap();
                assert!(Labels::default().updated(&message).is_ok(), "{id}: {good}");
            }
        }
    }

    #[test]
    fn a_menu_label_may_not_hide_a_tab_that_would_pass_for_a_shortcut_hint() {
        // The shell appends the hint after a tab itself; a label that brings its own is refused.
        let message: ShellLabels =
            serde_json::from_value(json!({ "menuPrint": "Print\tCtrl+Q" })).unwrap();
        let error = Labels::default().updated(&message).unwrap_err();
        assert_eq!(field_of(&error), "menuPrint");
    }

    #[test]
    fn a_control_character_is_refused() {
        for bad in [
            "Op\nen",
            "Op\ren",
            "Op\ten",
            "Op\0en",
            "\u{1b}[31mOpen",
            "Open\u{7f}",
        ] {
            let error = Labels::default()
                .updated(&named(Some(bad), None))
                .unwrap_err();
            assert_eq!(field_of(&error), "open", "{bad:?}");
        }
    }

    #[test]
    fn one_bad_label_changes_nothing() {
        let before = Labels::default();
        let update = ShellLabels {
            open: Some("Ouvrir".to_owned()),
            lock: Some("a".repeat(100)),
            ..ShellLabels::default()
        };
        let error = before.updated(&update).unwrap_err();
        assert_eq!(field_of(&error), "lock");

        let store = LabelStore::new();
        assert_eq!(store.update(&update).unwrap_err(), error);
        assert_eq!(store.get(), Labels::default());

        // A menu label that is refused takes the tray's good ones down with it, and the other way round.
        let mixed = ShellLabels {
            menu_file: Some("&Datei".to_owned()),
            menu_help: Some("\n".to_owned()),
            ..ShellLabels::default()
        };
        assert_eq!(
            field_of(&LabelStore::new().update(&mixed).unwrap_err()),
            "menuHelp"
        );
        assert_eq!(store.get(), Labels::default());
    }

    #[test]
    fn an_error_names_the_field_and_never_the_value() {
        let secret = format!("SECRET-{}", "x".repeat(100));
        let error = Labels::default()
            .updated(&named(Some(&secret), None))
            .unwrap_err();
        let text = serde_json::to_string(&error).unwrap();
        assert!(!text.contains("SECRET"), "{text}");

        let menu = ShellLabels {
            menu_about: Some(secret),
            ..ShellLabels::default()
        };
        let error = Labels::default().updated(&menu).unwrap_err();
        let text = serde_json::to_string(&error).unwrap();
        assert!(!text.contains("SECRET"), "{text}");
    }

    #[test]
    fn the_store_keeps_what_was_set() {
        let store = LabelStore::new();
        assert_eq!(store.get(), Labels::default());
        let next = store.update(&named(Some("Ouvrir"), None)).unwrap();
        assert_eq!(next.open, "Ouvrir");
        assert_eq!(store.get(), next);
        store.update(&named(None, Some("Nouvelle page"))).unwrap();
        assert_eq!(store.get().open, "Ouvrir");
        assert_eq!(store.get().new_page, "Nouvelle page");
    }

    #[test]
    fn the_english_menu_defaults_fit_a_label() {
        // They are valid labels, so a UI that sends them back changes nothing.
        let mut all = ShellLabels::default();
        for (id, default) in menu_ids() {
            assert!(default.chars().count() <= MAX_LABEL_CHARS, "{id}");
            assert!(!default.chars().any(char::is_control), "{id}");
            assert_eq!(default, default.trim(), "{id}");
        }
        all.menu_file = Some(DEFAULT_MENU_FILE.to_owned());
        assert_eq!(Labels::default().updated(&all).unwrap(), Labels::default());
    }

    // --- the dialogs and the file types (ADR-0040 item 13) ---

    /// Every dialog text with its id, its English default, the placeholders it may use and the ones it must use.
    #[allow(clippy::type_complexity)]
    fn dialog_templates() -> Vec<(
        &'static str,
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    )> {
        vec![
            (
                "dialogAiMessage",
                DEFAULT_DIALOG_AI_MESSAGE,
                &["host"],
                &["host"],
            ),
            (
                "dialogMcpRead",
                DEFAULT_DIALOG_MCP_READ,
                &["client", "spaces"],
                &["client", "spaces"],
            ),
            (
                "dialogMcpWrite",
                DEFAULT_DIALOG_MCP_WRITE,
                &["client", "spaces"],
                &["client", "spaces"],
            ),
            (
                "dialogMcpBoth",
                DEFAULT_DIALOG_MCP_BOTH,
                &["client", "read", "write"],
                &["client", "read", "write"],
            ),
            (
                "dialogListAnd",
                DEFAULT_DIALOG_LIST_AND,
                &["names", "last"],
                &["names", "last"],
            ),
            (
                "dialogListMore",
                DEFAULT_DIALOG_LIST_MORE,
                &["count"],
                &["count"],
            ),
            (
                "dialogImagesOne",
                DEFAULT_DIALOG_IMAGES_ONE,
                &["count", "stored"],
                &["stored"],
            ),
            (
                "dialogImagesOther",
                DEFAULT_DIALOG_IMAGES_OTHER,
                &["count", "stored"],
                &["count", "stored"],
            ),
            (
                "dialogHostsOne",
                DEFAULT_DIALOG_HOSTS_ONE,
                &["host", "count"],
                &["host"],
            ),
            (
                "dialogHostsOther",
                DEFAULT_DIALOG_HOSTS_OTHER,
                &["host", "count"],
                &["host", "count"],
            ),
            (
                "dialogNotifyThreadReply",
                DEFAULT_DIALOG_NOTIFY_THREAD_REPLY,
                &["actor", "page"],
                &["actor"],
            ),
            (
                "dialogNotifyCommentMention",
                DEFAULT_DIALOG_NOTIFY_COMMENT_MENTION,
                &["actor", "page"],
                &["actor"],
            ),
            (
                "dialogNotifyPageMention",
                DEFAULT_DIALOG_NOTIFY_PAGE_MENTION,
                &["actor", "page"],
                &["actor"],
            ),
            (
                "dialogNotifyPersonAssigned",
                DEFAULT_DIALOG_NOTIFY_PERSON_ASSIGNED,
                &["actor", "page"],
                &["actor"],
            ),
            (
                "dialogDictationMessage",
                DEFAULT_DIALOG_DICTATION_MESSAGE,
                &["model", "size", "host"],
                &["size", "host"],
            ),
            ("dialogDriveMessage", DEFAULT_DIALOG_DRIVE_MESSAGE, &[], &[]),
            ("dialogDriveDone", DEFAULT_DIALOG_DRIVE_DONE, &[], &[]),
        ]
    }

    /// The ids of the plain dialog labels and the file types, with their English defaults.
    fn plain_dialog_ids() -> Vec<(&'static str, &'static str)> {
        vec![
            ("dialogAllow", DEFAULT_DIALOG_ALLOW),
            ("dialogCancel", DEFAULT_DIALOG_CANCEL),
            ("dialogYes", DEFAULT_DIALOG_YES),
            ("dialogNo", DEFAULT_DIALOG_NO),
            ("dialogUntitled", DEFAULT_DIALOG_UNTITLED),
            ("dialogAiTitle", DEFAULT_DIALOG_AI_TITLE),
            ("dialogMcpTitle", DEFAULT_DIALOG_MCP_TITLE),
            ("dialogImagesTitle", DEFAULT_DIALOG_IMAGES_TITLE),
            ("dialogDictationTitle", DEFAULT_DIALOG_DICTATION_TITLE),
            ("dialogDownload", DEFAULT_DIALOG_DOWNLOAD),
            ("dialogDriveTitle", DEFAULT_DIALOG_DRIVE_TITLE),
            ("dialogDriveAccept", DEFAULT_DIALOG_DRIVE_ACCEPT),
            ("dialogDriveCancel", DEFAULT_DIALOG_DRIVE_CANCEL),
            ("fileFilterBackup", DEFAULT_FILE_FILTER_BACKUP),
            ("fileFilterZip", DEFAULT_FILE_FILTER_ZIP),
            ("fileFilterText", DEFAULT_FILE_FILTER_TEXT),
            ("fileFilterImport", DEFAULT_FILE_FILTER_IMPORT),
        ]
    }

    fn one_label(id: &str, text: &str) -> ShellLabels {
        serde_json::from_value(json!({ id: text })).unwrap()
    }

    #[test]
    fn the_dialogs_start_in_english_and_the_defaults_are_valid_to_send_back() {
        let labels = Labels::default();
        assert_eq!(labels.dialog_allow, "Allow");
        assert_eq!(labels.dialog_cancel, "Cancel");
        assert_eq!(labels.dialog_yes, "Yes");
        assert_eq!(labels.dialog_no, "No");
        assert_eq!(labels.dialog_ai_title, "Send text to another computer?");
        assert_eq!(labels.file_filter_zip, "Zip archive");
        // A UI that sends the English back changes nothing.
        for (id, default) in plain_dialog_ids() {
            let next = Labels::default().updated(&one_label(id, default)).unwrap();
            assert_eq!(next, Labels::default(), "{id}");
        }
        for (id, default, allowed, required) in dialog_templates() {
            assert!(default.chars().count() <= MAX_TEXT_CHARS, "{id}");
            let next = Labels::default().updated(&one_label(id, default)).unwrap();
            assert_eq!(next, Labels::default(), "{id}");
            // The defaults use what they may and what they must.
            let names: Vec<&str> = pieces(default)
                .into_iter()
                .filter_map(|piece| match piece {
                    Piece::Name(name) => Some(name),
                    Piece::Text(_) => None,
                })
                .collect();
            assert!(required.iter().all(|name| names.contains(name)), "{id}");
            assert!(names.iter().all(|name| allowed.contains(name)), "{id}");
        }
    }

    #[test]
    fn a_dialog_label_is_set_by_its_own_id_and_changes_only_that() {
        for (id, _) in plain_dialog_ids()
            .into_iter()
            .chain(dialog_templates().into_iter().map(|(id, d, ..)| (id, d)))
        {
            // Keep each template's placeholders so the text is valid for its own id.
            let text = dialog_templates()
                .into_iter()
                .find(|(template, ..)| *template == id)
                .map_or_else(
                    || format!("<{id}>"),
                    |(_, default, ..)| format!("{default} !"),
                );
            let next = Labels::default().updated(&one_label(id, &text)).unwrap();
            assert_ne!(next, Labels::default(), "{id}");
            // The tray's four and the menu bar are untouched.
            assert_eq!(next.open, DEFAULT_OPEN, "{id}");
            assert_eq!(next.menu_file, DEFAULT_MENU_FILE, "{id}");
        }
    }

    #[test]
    fn a_plain_dialog_label_is_validated_like_any_label() {
        let too_long = "x".repeat(MAX_LABEL_CHARS + 1);
        for (id, _) in plain_dialog_ids() {
            for bad in ["", "   ", "Ab\ncd", "Ab\u{0}cd", too_long.as_str()] {
                let error = Labels::default().updated(&one_label(id, bad)).unwrap_err();
                assert_eq!(field_of(&error), id, "{id}: {bad:?}");
            }
        }
    }

    #[test]
    fn a_dialog_text_must_keep_the_placeholders_a_consent_depends_on() {
        // A translation that drops the computer's address, the tool or the spaces would ask the person a question
        // that hides what they agree to: it is refused, naming the field and not the text.
        for (id, _, _, required) in dialog_templates() {
            for missing in required {
                // Every required placeholder but `missing`, joined by words.
                let text = required
                    .iter()
                    .filter(|name| *name != missing)
                    .map(|name| format!("{{{name}}}"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                let text = format!("Is it fine {text}?");
                let error = Labels::default()
                    .updated(&one_label(id, &text))
                    .unwrap_err();
                assert_eq!(field_of(&error), id, "{id} without {missing}");
            }
        }
        let error = Labels::default()
            .updated(&one_label(
                "dialogAiMessage",
                "Send everything to the cloud?",
            ))
            .unwrap_err();
        assert_eq!(field_of(&error), "dialogAiMessage");
        assert!(!serde_json::to_string(&error).unwrap().contains("cloud"));
    }

    #[test]
    fn a_dialog_text_may_not_use_a_placeholder_the_shell_does_not_fill() {
        for (id, default, ..) in dialog_templates() {
            let text = format!("{default} {{surprise}}");
            let error = Labels::default()
                .updated(&one_label(id, &text))
                .unwrap_err();
            assert_eq!(field_of(&error), id);
        }
        // The two forms of the images text fill different things: `{host}` is not one of its.
        let error = Labels::default()
            .updated(&one_label("dialogImagesOne", "{stored} on {host}"))
            .unwrap_err();
        assert_eq!(field_of(&error), "dialogImagesOne");
    }

    #[test]
    fn a_dialog_text_is_up_to_four_hundred_characters_without_control_characters() {
        let longest = format!(
            "{{host}}{}",
            "\u{e9}".repeat(MAX_TEXT_CHARS - "{host}".len())
        );
        assert_eq!(longest.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(
            Labels::default()
                .updated(&one_label("dialogAiMessage", &longest))
                .unwrap()
                .dialog_ai_message,
            longest
        );
        let too_long = format!("{longest}x");
        for bad in [
            too_long.as_str(),
            "",
            "   ",
            "{host}\nx",
            "{host}\tx",
            "{host}\u{0}x",
        ] {
            let error = Labels::default()
                .updated(&one_label("dialogAiMessage", bad))
                .unwrap_err();
            assert_eq!(field_of(&error), "dialogAiMessage", "{bad:?}");
        }
        // Trimmed, like the other labels.
        let next = Labels::default()
            .updated(&one_label("dialogAiMessage", "  Send to {host}? \n"))
            .unwrap();
        assert_eq!(next.dialog_ai_message, "Send to {host}?");
    }

    #[test]
    fn one_bad_dialog_text_changes_nothing() {
        let store = LabelStore::new();
        let update = ShellLabels {
            dialog_allow: Some("Autoriser".to_owned()),
            dialog_ai_message: Some("No address here".to_owned()),
            ..ShellLabels::default()
        };
        let error = store.update(&update).unwrap_err();
        assert_eq!(field_of(&error), "dialogAiMessage");
        assert_eq!(store.get(), Labels::default());
    }

    #[test]
    fn braces_that_are_not_a_placeholder_are_text() {
        // `{ host }`, `{}`, `{host` and `{host-1}` are not placeholders: they are kept as written.
        let text = "Send to {host} (a {} b { host } {host c {host-1})";
        let next = Labels::default()
            .updated(&one_label("dialogAiMessage", text))
            .unwrap();
        assert_eq!(next.dialog_ai_message, text);
        assert_eq!(
            fill(text, &[("host", "pc.local")]),
            "Send to pc.local (a {} b { host } {host c {host-1})"
        );
    }

    #[test]
    fn fill_replaces_each_placeholder_in_one_pass() {
        assert_eq!(
            fill("{a} and {b}, {a} again", &[("a", "1"), ("b", "2")]),
            "1 and 2, 1 again"
        );
        // A value is never read again: a name that holds a placeholder keeps it.
        assert_eq!(
            fill(
                "Allow {client} to read {spaces}?",
                &[("client", "{spaces}"), ("spaces", "{client}")]
            ),
            "Allow {spaces} to read {client}?"
        );
        // A placeholder with no value stays visible; text with no placeholder is as it was.
        assert_eq!(fill("{a} {c}", &[("a", "1")]), "1 {c}");
        assert_eq!(fill("plain", &[("a", "1")]), "plain");
        assert_eq!(fill("", &[("a", "1")]), "");
        // Multi-byte text around the braces is safe.
        assert_eq!(
            fill("\u{e0} {host} \u{2026}", &[("host", "h\u{f4}te")]),
            "\u{e0} h\u{f4}te \u{2026}"
        );
    }
}
