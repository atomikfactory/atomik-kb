//! Import and export (ADR-0036, `docs/architecture/import-export.md`): the data of the export
//! and import sessions, of the CSV export of a view and of scheduled backups. Pure data; the
//! converters live in `kb-export` and `kb-import`, the sessions in `kb-core`.

use serde::{Deserialize, Serialize};

use crate::ids::{AttachmentId, PageId, PropertyId, SpaceId};

// ----- export -------------------------------------------------------------------------------

/// File format of a page or space export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    /// Markdown files with YAML front matter (the "file flavour" of the Markdown renderer).
    Markdown,
    /// Standalone HTML documents with an embedded stylesheet.
    Html,
}

/// What `ExportBegin` planned. The shell picks the file name with it before it asks for entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ExportPlan {
    /// Session id for `ExportNext` and `ExportEnd`.
    pub export_id: u32,
    /// Pages in the export.
    pub pages: u32,
    /// Attachment files in the export (only those whose bytes are on this device).
    pub attachments: u32,
    /// The export is one text file and nothing else: it is written as a plain `.md` or `.html`
    /// file. Otherwise every entry goes into one `.zip` file.
    pub single_file: bool,
    /// File name to offer in the save dialog, without a directory: `Title.md`, `Title.html` or
    /// `Title.zip`, made safe for every file system.
    pub suggested_name: String,
}

/// One piece of an export, in the order the archive is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ExportEntry {
    /// Path inside the archive: relative, `/`-separated, no `.` or `..` segment, no leading
    /// `/`, every segment safe for Windows, macOS and Linux. For a single-file export it is the
    /// suggested name.
    pub path: String,
    /// What to write there.
    pub content: ExportContent,
}

/// The content of an export entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all_fields = "camelCase")]
pub enum ExportContent {
    /// A text file (UTF-8, no byte order mark, LF line ends): a page, a whiteboard
    /// (`.excalidraw`) or the export's `atomik-export.json`.
    Text {
        /// The file's text.
        text: String,
    },
    /// An attachment whose plaintext the shell streams from the core (`attachment_read`).
    Attachment {
        /// Attachment.
        attachment_id: AttachmentId,
        /// Plaintext size in bytes.
        size: u64,
    },
}

/// A batch of entries answered by `ExportNext`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ExportBatch {
    /// Entries, in archive order.
    pub entries: Vec<ExportEntry>,
    /// No entries follow.
    pub done: bool,
    /// Problems met while producing this batch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<ExportWarning>>"))]
    pub warnings: Vec<ExportWarning>,
}

/// Something an export left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ExportWarning {
    /// What happened.
    pub code: ExportWarningCode,
    /// Page concerned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub page_id: Option<PageId>,
    /// Attachment concerned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub attachment_id: Option<AttachmentId>,
}

/// Kinds of [`ExportWarning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ExportWarningCode {
    /// The attachment's bytes are not on this device (on-demand download, ADR-0023): its link
    /// is written but the file is not in the archive.
    AttachmentNotDownloaded,
    /// The attachment is unknown or deleted: its link is written, no file.
    AttachmentMissing,
    /// The attachment could not be read while writing the archive (reported by the shell).
    AttachmentUnreadable,
}

/// A column of the CSV export of a view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all_fields = "camelCase")]
pub enum CsvColumn {
    /// The page's title (header `Name`).
    Title,
    /// A property of the space (header: its name).
    Property {
        /// Property.
        property_id: PropertyId,
    },
}

// ----- import -------------------------------------------------------------------------------

/// What `ImportPlan` found in the files given so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ImportPlanInfo {
    /// Pages the import will create.
    pub pages: u32,
    /// Paths (as given to `ImportPlan` in `binary_paths`) of the files the pages use as images
    /// or attachments: the shell uploads each as an attachment of the space and reports it with
    /// `ImportAddAttachment`. Paths not listed are not imported.
    pub needed_files: Vec<String>,
    /// Problems found while planning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<ImportWarning>>"))]
    pub warnings: Vec<ImportWarning>,
    /// The spaces an `ImportBeginSpaces` session created for the plan, in order (ADR-0039); empty
    /// for an `ImportBegin` session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<ImportSpaceInfo>>"))]
    pub spaces: Vec<ImportSpaceInfo>,
    /// For an `ImportBeginSpaces` session, the space each of `needed_files` is uploaded into
    /// (same order and length); empty otherwise (every file goes to the session's space).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<SpaceId>>"))]
    pub file_spaces: Vec<SpaceId>,
    /// Display names of `needed_files` (same order and length) when the source names its files
    /// apart from their paths (an AppFlowy backup stores them by content hash, ADR-0039); empty
    /// otherwise: the shell names each attachment after the path's last segment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<String>>"))]
    pub file_names: Vec<String>,
    /// Images the source keeps on a server (an AppFlowy workspace's uploads): the shell may
    /// download them after the person agreed, upload each as an attachment of its space and
    /// report it with `ImportAddAttachment` under its key. Not reported: the image stays a link.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<ImportRemoteFile>>"))]
    pub remote_files: Vec<ImportRemoteFile>,
}

/// A space created by an `ImportBeginSpaces` session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ImportSpaceInfo {
    /// The new space.
    pub space_id: SpaceId,
    /// Its name.
    pub name: String,
    /// Pages the import will create in it.
    pub pages: u32,
}

/// A file of the source that lives on a server (ADR-0039).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ImportRemoteFile {
    /// Key to report it under (`ImportAddAttachment::path`), `remote/<n>`.
    pub key: String,
    /// Its `https` URL.
    pub url: String,
    /// The URL's host, for the consent question.
    pub host: String,
    /// The space to upload it into.
    pub space_id: SpaceId,
}

/// How far `ImportCommit` got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    /// Pages written so far.
    pub created: u32,
    /// Pages to write in all.
    pub total: u32,
    /// The import is complete; `report` is set and the session is gone.
    pub done: bool,
    /// The outcome, once `done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub report: Option<ImportReport>,
}

/// The outcome of an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// The pages created directly under the destination, in order (one in most imports: the
    /// imported page or the container named after the source).
    pub root_page_ids: Vec<PageId>,
    /// Pages created.
    pub pages: u32,
    /// Attachments used by the created pages.
    pub attachments: u32,
    /// Properties added to the space's schema.
    pub properties_created: u32,
    /// Everything the import left out or changed, at most 500 (the rest are counted in
    /// `more_warnings`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<ImportWarning>>"))]
    pub warnings: Vec<ImportWarning>,
    /// Warnings beyond the 500 listed.
    #[serde(default)]
    pub more_warnings: u32,
    /// The spaces an `ImportBeginSpaces` session created, in order (ADR-0039); empty otherwise.
    /// `root_page_ids` then lists the root pages of every one of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<SpaceId>>"))]
    pub space_ids: Vec<SpaceId>,
}

/// Something an import left out or changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ImportWarning {
    /// Source path of the file concerned (as the shell gave it).
    pub path: String,
    /// What happened.
    pub code: ImportWarningCode,
    /// A short detail for the person (a link target, a property name, a column), never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub detail: Option<String>,
}

/// Kinds of [`ImportWarning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ImportWarningCode {
    /// A file of a type the importer does not read; left out.
    UnsupportedFile,
    /// A text file that is not valid UTF-8; read with replacement characters.
    NotUtf8,
    /// A file above the size limit; left out.
    TooLarge,
    /// A relative link to a page that is not in the source; kept as a plain link or text.
    UnresolvedLink,
    /// An image or file the page names that the source lacks; kept as text.
    MissingFile,
    /// An `http`/`https` image: not downloaded (nothing is fetched); kept as a link.
    RemoteImage,
    /// A `data:` image: not imported.
    EmbeddedImage,
    /// The space already has 100 properties: a column or front matter key left out.
    PropertyLimit,
    /// A value that does not fit its property's kind; left out.
    InvalidValue,
    /// The import stopped at its page limit; the remaining files were left out.
    Truncated,
    /// Content the page schema cannot hold (a script, a frame, a form, an unknown HTML
    /// element's markup); left out, its text kept where there is some.
    UnsupportedContent,
    /// The uploading of a needed file failed (reported by the shell).
    AttachmentFailed,
}

// ----- scheduled backups ----------------------------------------------------------------------

/// Device setting holding the [`BackupSchedule`] (written by the UI).
pub const BACKUP_SCHEDULE_SETTING: &str = "app.backup.schedule";

/// Device setting holding the folder of scheduled backups, a string path. Written only by the
/// desktop shell after its folder dialog; the shell refuses a `SettingsSet` of this key coming
/// from the webview.
pub const BACKUP_FOLDER_SETTING: &str = "app.backup.folder";

/// Device setting holding the last [`BackupRunStatus`]. Written only by the shell.
pub const BACKUP_LAST_SETTING: &str = "app.backup.last";

/// Prefix of the file names of scheduled backups. Only files named
/// `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb` are ever deleted by the pruning.
pub const BACKUP_AUTO_PREFIX: &str = "atomik-kb-auto-";

/// How often a scheduled backup runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum BackupInterval {
    /// Once a day.
    #[default]
    Daily,
    /// Once a week.
    Weekly,
}

/// The scheduled backup settings (setting [`BACKUP_SCHEDULE_SETTING`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BackupSchedule {
    /// Scheduled backups run.
    pub enabled: bool,
    /// How often.
    #[serde(default)]
    pub interval: BackupInterval,
    /// How many scheduled backups to keep in the folder, 1 to 100 (default 7); older ones are
    /// deleted after a successful backup.
    #[serde(default = "default_keep")]
    pub keep: u32,
}

const fn default_keep() -> u32 {
    7
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: BackupInterval::Daily,
            keep: default_keep(),
        }
    }
}

// ----- Google Drive backups (ADR-0059) -------------------------------------------------------

/// Device setting holding the [`DriveConnection`]. Written only by the desktop shell; device-bound
/// (never in a backup, ignored on restore).
pub const BACKUP_DRIVE_SETTING: &str = "app.backup.drive";

/// The Google Drive target of this device (setting [`BACKUP_DRIVE_SETTING`]). The refresh token is
/// in the OS keystore, not here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DriveConnection {
    /// The Google account's address, from Drive's `about` (shown in Settings, never sent elsewhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub account: Option<String>,
    /// The Drive id of the "atomik-kb backups" folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub folder_id: Option<String>,
    /// When the connection was made, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub connected_at: Option<i64>,
    /// The client id of one's own Google client, when one is used instead of the built-in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub own_client_id: Option<String>,
    /// Google refused the stored token: the connection must be made again.
    #[serde(default)]
    pub needs_reconnect: bool,
    /// The last delivery to Drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last: Option<BackupRunStatus>,
}

/// Which Google client the app signs in with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DriveClientSource {
    /// The client built into this installer.
    BuiltIn,
    /// The person's own client (Settings, Advanced).
    Own,
    /// No client: this build has none and no own client is set.
    None,
}

/// What Settings shows about the Google Drive target (`drive_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DriveStatus {
    /// The client in use.
    pub client: DriveClientSource,
    /// A refresh token is stored.
    pub connected: bool,
    /// The Google account's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub account: Option<String>,
    /// The folder's address in Drive's web interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub folder_url: Option<String>,
    /// The own client's id, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub own_client_id: Option<String>,
    /// Google refused the stored token.
    pub needs_reconnect: bool,
    /// The last delivery to Drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last: Option<BackupRunStatus>,
}

/// One's own Google client (`drive_client_set`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DriveClientInput {
    /// The client id.
    pub client_id: String,
    /// The client secret (kept in the OS keystore).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub client_secret: Option<String>,
}

/// Progress of a download from Drive (`drive_backup_download`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DriveProgress {
    /// Bytes received so far.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub received: u64,
    /// The file's size, when Drive said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub total: Option<u64>,
}

/// A backup file a restore can use: in a folder (`id` is its absolute path) or in Drive (`id` is the
/// Drive file id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BackupFileInfo {
    /// The absolute path, or the Drive file id.
    pub id: String,
    /// The file name.
    pub name: String,
    /// Bytes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    /// Last modified, Unix ms.
    pub modified_at: i64,
}

/// The backups of a folder (`backup_folder_list`), newest first, at most 200.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BackupFolderListing {
    /// The folder listed (absolute path).
    pub folder: String,
    /// Its `.atomikkb` files.
    pub backups: Vec<BackupFileInfo>,
}

/// The outcome of the last scheduled backup (setting [`BACKUP_LAST_SETTING`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BackupRunStatus {
    /// When the last attempt started, Unix ms (one reading of the clock names its file too).
    pub at: i64,
    /// The last attempt succeeded.
    pub ok: bool,
    /// When the last successful backup started, Unix ms (the next one is due a period later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_success_at: Option<i64>,
    /// File name (no directory) of the last successful backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub file: Option<String>,
    /// Error code of a failed attempt (`ErrorCode` as text) and its message, never user content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
}
