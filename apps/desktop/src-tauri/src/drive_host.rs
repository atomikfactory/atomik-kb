//! Google Drive as a target of the scheduled backups (ADR-0059, `docs/architecture/drive-backup.md`
//! section 3): the Google client (built in at compile time, or the person's own), the sign-in in the
//! system browser with a loopback redirect, the refresh token in the OS keystore, the HTTP transport
//! of `kb-drive`'s requests, the `drive_*` commands' bodies and the delivery of a backup file.
//!
//! The protocol itself (PKCE, the URLs, the requests as values, the parsers, the resumable upload's steps) is
//! the sans-IO crate `kb-drive`; this module sends its requests (`net`), runs the loopback listener of the
//! sign-in (`loopback`), keeps the secrets and the state, and decides what to do next.
//!
//! # As built
//!
//! - **The client.** Built in: `KB_GOOGLE_CLIENT_ID` and `KB_GOOGLE_CLIENT_SECRET`, read when the shell is compiled
//!   (`option_env!`; empty is absent). Own: the id in `app.backup.drive` (`ownClientId`) and the secret in the
//!   keystore entry `gdrive-client:<install id>` ([`client_set`]); an own client wins. With neither,
//!   [`connect`] answers `InvalidInput` with the reason `noClient`, before any dialog.
//! - **The setting.** `app.backup.drive` (`DriveConnection`) is written only here, through the shell's own handle on
//!   the core (the webview's `SettingsSet` of it is refused by `guard`), one writer at a time. The refresh token is
//!   never in it: it is in the keystore entry `gdrive:<install id>`; the access token is in memory with its expiry
//!   and is refreshed 60 s before it ends. The install id is the one of `install.json` in the data directory
//!   ([`DriveShell::set_data_dir`]).
//! - **Connecting** ([`connect`]): the core must be ready (the restore wizard connects during onboarding, before
//!   a workspace exists: like `backup_import`, only `Locked` and "starting" are refused); the native consent
//!   dialog (`dialogDrive*` labels; declined is `consentDeclined`); `127.0.0.1:0`; a random `state` and PKCE
//!   verifier (32 bytes each, from the OS); the authorisation URL in the system browser (as `open_external`);
//!   the redirect (see `loopback`; 5 minutes, then `browserTimeout`; [`cancel`] ends it with `cancelled`); the code
//!   exchange; the refresh token into the keystore; Drive's `about` for the account's address; the backups folder
//!   found (the first match) or made; the setting. A failure after the token was stored forgets it again.
//! - **Disconnecting** ([`disconnect`]) revokes the refresh token (errors ignored), deletes the keystore entry and
//!   clears the setting but `ownClientId`; the files stay in Drive. [`client_set`] disconnects too (a token is
//!   bound to its client).
//! - **Delivering** ([`deliver`], called by the scheduler): the folder is looked up every time (a trashed folder
//!   would swallow the backups), the file goes up as a resumable upload in `kb_drive::CHUNK_SIZE` pieces, and the
//!   newest `keep` scheduled backups stay (`kb_core::backup_schedule::to_prune`, only names of the scheduled form,
//!   never the file just uploaded; a failed deletion is counted and logged, not an error). A chunk that fails with
//!   the network, a 5xx or a rate limit asks the session where it stands and resumes, up to 5 times in a row
//!   (counted again after every chunk that got through) with 1, 2, 4, 8 and 16 s waits; then the delivery fails.
//!   The outcome is in `app.backup.drive.last`. A connection that is not made is `Ok(NotConnected)`.
//! - **Listing and downloading** ([`backups_list`], [`download`]): the `.atomikkb` files of the folder, newest
//!   first, at most 200; a download goes to `<app data>/restore-staging/<random>.atomikkb` (the folder is emptied
//!   first), piece by piece with a 60 s read timeout, progress at most every 250 ms, and the answer is the
//!   absolute path, which `backup_import` takes. A failed or cancelled download leaves no file.
//! - **Errors.** `InvalidInput` for the field `drive` with `details.reason` one of `consentDeclined`, `cancelled`,
//!   `noClient`, `browserTimeout`, `denied`, `reconnect`, `quota`, `notConnected`, `notFound`, `network` and
//!   `server` ([`Reason`]); the messages are generic. A `reconnect` also sets `needsReconnect` in the setting (the next
//!   connect clears it). Keystore and disk failures are their own `ApiError`s.
//! - **Threads.** The network work runs on a blocking thread that drives the flow with `block_on` (the core's lock
//!   can be held for a whole backup, and a dialog parks its thread), never on an async worker.
//! - **Logs.** The step and the kind of failure only: never a token, a code, a state, a client secret, a file id, a
//!   file name, a folder id, an address or a path.
//! - **Tests.** `tests.rs`: the flows against a fake Google on `127.0.0.1` (`tests/fake_google.rs`), the loopback
//!   redirect driven by the test, the keystore replaced by `kb_keys::MemoryKeystore`; the transport's base URLs are
//!   rewritten for the tests only (`net::Config::local`), production is HTTPS only.

mod loopback;
mod net;
#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use kb_core::backup_schedule;
use kb_drive::{
    CHUNK_SIZE, DriveError, DriveFile, HttpRequest, HttpResponse, MAX_LISTED, OAuthClient,
    UploadStep,
};
use kb_keys::{Keystore, KeystoreError};
use kb_storage_native::{
    KEYSTORE_SERVICE, NativeKeystore, OsRng, SystemClock, load_or_create_install,
};
use kb_types::interop::{
    BACKUP_DRIVE_SETTING, BackupFileInfo, DriveClientInput, DriveClientSource, DriveConnection,
    DriveProgress, DriveStatus,
};
use kb_types::{ApiError, BackupRunStatus, Clock, Request, Response, RngExt};
use reqwest::Response as HttpStream;
use serde_json::json;
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::backup::{BACKUP_EXTENSION, RESTORE_STAGING_DIR, empty_staging, path_text};
use crate::dialogs::{Confirm, Question};
use crate::state::CoreHost;
use crate::{DRIVE_STOPPED, blocking};
use net::{Config, Net};

/// The built-in Google client id, read when the shell is compiled (ADR-0059 decision 6).
pub(crate) const BUILT_IN_CLIENT_ID: Option<&str> = option_env!("KB_GOOGLE_CLIENT_ID");
/// The built-in client's secret (an installed app's, not confidential), read when the shell is compiled.
pub(crate) const BUILT_IN_CLIENT_SECRET: Option<&str> = option_env!("KB_GOOGLE_CLIENT_SECRET");

/// The field every Drive failure names.
const DRIVE_FIELD: &str = "drive";

/// How many times in a row a chunk may fail before the delivery does (the waits are `Config::backoff`).
const RETRIES: usize = 5;

/// An access token is refreshed this long before it ends.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// How often at most a download reports its progress.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// How long a revocation may take when the person disconnects: it is best effort.
const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most pages of a folder's listing that are read (200 files each).
const MAX_PAGES: usize = 20;

/// The longest client id or secret `drive_client_set` accepts.
const MAX_CLIENT_CHARS: usize = 256;

// --- errors ---

/// Why a Drive command failed, as `details.reason` says it (section 3.4 of the design doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    /// The person declined the native notice.
    ConsentDeclined,
    /// `drive_cancel` (or a newer sign-in or download) ended it.
    Cancelled,
    /// This build has no Google client and none is set.
    NoClient,
    /// The browser did not bring the sign-in back in time.
    BrowserTimeout,
    /// The person refused access in the browser.
    Denied,
    /// Google refused the stored token or the client: connect again.
    Reconnect,
    /// The Drive is full.
    Quota,
    /// Drive is not connected.
    NotConnected,
    /// The file or folder is gone.
    NotFound,
    /// Google could not be reached.
    Network,
    /// Google failed, asked to slow down or answered something unexpected.
    Server,
}

impl Reason {
    /// The word `details.reason` carries.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ConsentDeclined => "consentDeclined",
            Self::Cancelled => "cancelled",
            Self::NoClient => "noClient",
            Self::BrowserTimeout => "browserTimeout",
            Self::Denied => "denied",
            Self::Reconnect => "reconnect",
            Self::Quota => "quota",
            Self::NotConnected => "notConnected",
            Self::NotFound => "notFound",
            Self::Network => "network",
            Self::Server => "server",
        }
    }

    /// A generic message: nothing in it comes from Google, the person or the files.
    const fn message(self) -> &'static str {
        match self {
            Self::ConsentDeclined => "Google Drive was not connected: the notice was declined",
            Self::Cancelled => "the Google Drive request was cancelled",
            Self::NoClient => "this build has no Google client: set your own in the settings",
            Self::BrowserTimeout => "the sign-in in the browser took too long",
            Self::Denied => "access to Google Drive was refused",
            Self::Reconnect => "Google Drive must be connected again",
            Self::Quota => "Google Drive is full",
            Self::NotConnected => "Google Drive is not connected",
            Self::NotFound => "the backup is not in Google Drive any more",
            Self::Network => "Google Drive could not be reached",
            Self::Server => "Google Drive did not answer as expected",
        }
    }

    /// The error: `InvalidInput` for the field `drive`, with the reason.
    pub(crate) fn error(self) -> ApiError {
        ApiError::invalid(DRIVE_FIELD, self.message())
            .with_details(json!({ "field": DRIVE_FIELD, "reason": self.as_str() }))
    }
}

/// The `details.reason` of an error of this module.
pub(crate) fn reason_of(error: &ApiError) -> Option<&str> {
    error.details.as_ref()?.get("reason")?.as_str()
}

/// What went wrong inside a flow: the network, an answer of Google, or an error of the shell's own.
#[derive(Debug)]
enum Fail {
    /// The request did not get an answer.
    Network,
    /// Google's answer means this.
    Drive(DriveError),
    /// Something else (the keystore, a file, the core), already an `ApiError`.
    Api(ApiError),
}

impl From<DriveError> for Fail {
    fn from(error: DriveError) -> Self {
        Self::Drive(error)
    }
}

impl From<ApiError> for Fail {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<Fail> for ApiError {
    fn from(fail: Fail) -> Self {
        match fail {
            Fail::Api(error) => error,
            other => other
                .reason()
                .map_or_else(|| ApiError::internal(DRIVE_STOPPED), Reason::error),
        }
    }
}

impl Fail {
    /// Whether trying again may help: the network, a 5xx or a rate limit.
    fn transient(&self) -> bool {
        matches!(
            self,
            Self::Network | Self::Drive(DriveError::Server | DriveError::RateLimited)
        )
    }

    /// The reason a failure is reported with.
    fn reason(&self) -> Option<Reason> {
        match self {
            Self::Network => Some(Reason::Network),
            Self::Api(error) => match reason_of(error)? {
                "cancelled" => Some(Reason::Cancelled),
                _ => None,
            },
            Self::Drive(error) => Some(match error {
                DriveError::Denied | DriveError::State => Reason::Denied,
                DriveError::Reconnect => Reason::Reconnect,
                DriveError::Quota => Reason::Quota,
                DriveError::NotFound => Reason::NotFound,
                DriveError::RateLimited | DriveError::Server | DriveError::Protocol(_) => {
                    Reason::Server
                }
            }),
        }
    }

    /// Whether this is a refusal that means "connect again".
    fn is_reconnect(&self) -> bool {
        matches!(self, Self::Drive(DriveError::Reconnect))
    }

    /// Logs a failed step: the step and the kind of failure, nothing else.
    fn log(&self, step: &'static str) {
        let reason = self.reason().map_or("other", Reason::as_str);
        tracing::warn!(step, reason, "a Google Drive step failed");
    }
}

fn cancelled() -> Fail {
    Fail::Api(Reason::Cancelled.error())
}

// --- the shell's state ---

/// A sign-in or a download that runs: what to tell it to stop.
struct Slot {
    token: u64,
    cancel: watch::Sender<bool>,
}

/// The two things that can run and be cancelled.
#[derive(Default)]
struct Slots {
    connect: Option<Slot>,
    download: Option<Slot>,
}

/// Which slot.
#[derive(Clone, Copy)]
enum Operation {
    Connect,
    Download,
}

/// An operation's place in the shell; dropping it frees the place (a newer operation keeps its own).
struct Running<'a> {
    shell: &'a DriveShell,
    operation: Operation,
    token: u64,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let mut slots = self.shell.lock_slots();
        let slot = match self.operation {
            Operation::Connect => &mut slots.connect,
            Operation::Download => &mut slots.download,
        };
        if slot.as_ref().is_some_and(|slot| slot.token == self.token) {
            *slot = None;
        }
    }
}

/// An access token and when it stops being good.
struct Access {
    token: Zeroizing<String>,
    expires: Instant,
}

/// What the tests replace; the app never does.
struct Seams {
    /// The OS keystore, opened when it is first needed.
    keystore: Option<Arc<dyn Keystore>>,
    /// The built-in client (id, secret): what the build had.
    builtin: Option<(String, Option<String>)>,
    /// How the transport behaves.
    config: Config,
}

/// What the Drive host holds while the app runs: the access token in memory, a sign-in or a download in
/// progress (to cancel it), the keystore and where the data directory is. Managed by Tauri like `AiShell`.
pub struct DriveShell {
    data_dir: OnceLock<PathBuf>,
    install: OnceLock<String>,
    seams: Mutex<Seams>,
    /// One writer of `app.backup.drive` at a time (a read, a change, a write).
    settings: Mutex<()>,
    access: Mutex<Option<Access>>,
    slots: Mutex<Slots>,
    tokens: AtomicU64,
}

impl std::fmt::Debug for DriveShell {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("DriveShell").finish_non_exhaustive()
    }
}

impl Default for DriveShell {
    fn default() -> Self {
        Self::new()
    }
}

/// `text` when it is not blank.
fn non_blank(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The built-in client the build had: an id that is not blank, with its secret when that is not blank either.
fn builtin_of(id: Option<&str>, secret: Option<&str>) -> Option<(String, Option<String>)> {
    non_blank(id).map(|id| (id, non_blank(secret)))
}

impl DriveShell {
    /// Nothing connected yet; the keystore and the setting say whether a connection exists. `run` tells it the
    /// data directory at start-up.
    #[must_use]
    pub fn new() -> Self {
        let builtin = builtin_of(BUILT_IN_CLIENT_ID, BUILT_IN_CLIENT_SECRET);
        Self {
            data_dir: OnceLock::new(),
            install: OnceLock::new(),
            seams: Mutex::new(Seams {
                keystore: None,
                builtin,
                config: Config::production(),
            }),
            settings: Mutex::new(()),
            access: Mutex::new(None),
            slots: Mutex::new(Slots::default()),
            tokens: AtomicU64::new(1),
        }
    }

    /// Tells the shell the machine-local data directory: the install id (for the keystore entries) and the staging
    /// folder of a download are found there. Once.
    pub fn set_data_dir(&self, dir: &Path) {
        let _ = self.data_dir.set(dir.to_path_buf());
    }

    /// A shell for the tests: its own directory, keystore, install id, built-in client and fake Google.
    #[cfg(test)]
    pub(in crate::drive_host) fn for_tests(
        data_dir: &Path,
        keystore: Arc<dyn Keystore>,
        builtin: Option<(&str, Option<&str>)>,
        config: Config,
    ) -> Self {
        let shell = Self::new();
        shell.set_data_dir(data_dir);
        let _ = shell.install.set("test-install".to_owned());
        {
            let mut seams = shell.lock_seams();
            seams.keystore = Some(keystore);
            seams.builtin = builtin.map(|(id, secret)| (id.to_owned(), secret.map(str::to_owned)));
            seams.config = config;
        }
        shell
    }

    fn lock_seams(&self) -> std::sync::MutexGuard<'_, Seams> {
        self.seams.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_slots(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_access(&self) -> std::sync::MutexGuard<'_, Option<Access>> {
        self.access.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn config(&self) -> Config {
        self.lock_seams().config.clone()
    }

    /// Takes the place of a sign-in or a download; one that already runs is told to stop (it ends with
    /// `cancelled`). The receiver turns true when this one is cancelled.
    fn begin(&self, operation: Operation) -> (Running<'_>, watch::Receiver<bool>) {
        let (sender, receiver) = watch::channel(false);
        let token = self.tokens.fetch_add(1, Ordering::SeqCst);
        let mut slots = self.lock_slots();
        let slot = match operation {
            Operation::Connect => &mut slots.connect,
            Operation::Download => &mut slots.download,
        };
        if let Some(old) = slot.replace(Slot {
            token,
            cancel: sender,
        }) {
            old.cancel.send_replace(true);
        }
        drop(slots);
        (
            Running {
                shell: self,
                operation,
                token,
            },
            receiver,
        )
    }

    /// Ends a sign-in that waits for the browser and a download that runs (`drive_cancel`); with none, nothing.
    pub(crate) fn cancel(&self) {
        let slots = self.lock_slots();
        for slot in [&slots.connect, &slots.download].into_iter().flatten() {
            slot.cancel.send_replace(true);
        }
    }

    fn data_dir(&self) -> Result<&Path, ApiError> {
        self.data_dir
            .get()
            .map(PathBuf::as_path)
            .ok_or_else(|| ApiError::internal("the data directory is not set up"))
    }

    /// The install id: the name the keystore entries carry.
    fn install_id(&self) -> Result<String, ApiError> {
        if let Some(id) = self.install.get() {
            return Ok(id.clone());
        }
        let info = load_or_create_install(self.data_dir()?, &OsRng).map_err(|_| {
            tracing::warn!("the installation file could not be read");
            ApiError::internal("the installation file cannot be read")
        })?;
        let id = info.install_id.to_string();
        let _ = self.install.set(id.clone());
        Ok(id)
    }

    fn keystore(&self) -> Result<Arc<dyn Keystore>, ApiError> {
        let mut seams = self.lock_seams();
        if let Some(keystore) = &seams.keystore {
            return Ok(Arc::clone(keystore));
        }
        let native =
            NativeKeystore::new(KEYSTORE_SERVICE).map_err(|error| keystore_failure(&error))?;
        let keystore: Arc<dyn Keystore> = Arc::new(native);
        seams.keystore = Some(Arc::clone(&keystore));
        Ok(keystore)
    }

    /// `gdrive:<install id>`: the refresh token.
    fn token_entry(&self) -> Result<String, ApiError> {
        Ok(format!("gdrive:{}", self.install_id()?))
    }

    /// `gdrive-client:<install id>`: the secret of an own client.
    fn secret_entry(&self) -> Result<String, ApiError> {
        Ok(format!("gdrive-client:{}", self.install_id()?))
    }

    fn load_secret(&self, entry: &str) -> Result<Option<Zeroizing<String>>, ApiError> {
        let stored = self
            .keystore()?
            .get(entry)
            .map_err(|error| keystore_failure(&error))?;
        Ok(stored.and_then(|bytes| String::from_utf8(bytes.to_vec()).ok().map(Zeroizing::new)))
    }

    fn store_secret(&self, entry: &str, secret: &str) -> Result<(), ApiError> {
        self.keystore()?
            .set(entry, secret.as_bytes())
            .map_err(|error| keystore_failure(&error))
    }

    fn remove_secret(&self, entry: &str) -> Result<(), ApiError> {
        self.keystore()?
            .delete(entry)
            .map_err(|error| keystore_failure(&error))
    }

    fn load_refresh_token(&self) -> Result<Option<Zeroizing<String>>, ApiError> {
        self.load_secret(&self.token_entry()?)
    }

    /// The access token, when it is good for more than the refresh margin.
    fn cached_access(&self) -> Option<String> {
        let access = self.lock_access();
        access
            .as_ref()
            .filter(|access| access.expires > Instant::now() + REFRESH_MARGIN)
            .map(|access| access.token.to_string())
    }

    fn cache_access(&self, token: &str, expires_in_s: u64) {
        let now = Instant::now();
        let expires = now
            .checked_add(Duration::from_secs(expires_in_s))
            .unwrap_or(now);
        *self.lock_access() = Some(Access {
            token: Zeroizing::new(token.to_owned()),
            expires,
        });
    }

    fn drop_access(&self) {
        *self.lock_access() = None;
    }

    /// The built-in client, when the build had one.
    fn builtin_client(&self) -> Option<OAuthClient> {
        self.lock_seams()
            .builtin
            .as_ref()
            .map(|(id, secret)| OAuthClient {
                client_id: id.clone(),
                client_secret: secret.clone(),
            })
    }

    /// Which client the app signs in with, given the setting.
    fn client_source(&self, connection: &DriveConnection) -> DriveClientSource {
        if non_blank(connection.own_client_id.as_deref()).is_some() {
            DriveClientSource::Own
        } else if self.builtin_client().is_some() {
            DriveClientSource::BuiltIn
        } else {
            DriveClientSource::None
        }
    }

    /// The client to sign in with (an own one wins, its secret read from the keystore); `None` without any.
    fn client(&self, connection: &DriveConnection) -> Result<Option<OAuthClient>, ApiError> {
        if let Some(id) = non_blank(connection.own_client_id.as_deref()) {
            let secret = self
                .load_secret(&self.secret_entry()?)?
                .map(|secret| secret.to_string());
            return Ok(Some(OAuthClient {
                client_id: id,
                client_secret: secret,
            }));
        }
        Ok(self.builtin_client())
    }

    /// Reads `app.backup.drive`, lets `change` change it and writes it, with no other writer in between.
    fn update_connection(
        &self,
        host: &dyn CoreHost,
        change: impl FnOnce(&mut DriveConnection),
    ) -> Result<DriveConnection, ApiError> {
        let _one_writer = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
        let mut connection = read_connection(host)?;
        change(&mut connection);
        write_connection(host, &connection)?;
        Ok(connection)
    }
}

/// A keystore failure as an error that says nothing the OS said.
fn keystore_failure(_: &KeystoreError) -> ApiError {
    tracing::warn!("the system keystore could not be used");
    ApiError::keystore_unavailable("the system keystore could not be used")
}

// --- the setting ---

fn unexpected() -> ApiError {
    ApiError::internal("the core answered a setting request with something unexpected")
}

/// `app.backup.drive`; the default (not connected) when it is not set or not readable.
fn read_connection(host: &dyn CoreHost) -> Result<DriveConnection, ApiError> {
    match host.dispatch(Request::SettingsGet {
        key: BACKUP_DRIVE_SETTING.to_owned(),
    })? {
        Response::Setting(Some(value)) => Ok(serde_json::from_value(value).unwrap_or_default()),
        Response::Setting(None) => Ok(DriveConnection::default()),
        _ => Err(unexpected()),
    }
}

fn write_connection(host: &dyn CoreHost, connection: &DriveConnection) -> Result<(), ApiError> {
    let value = serde_json::to_value(connection)
        .map_err(|_| ApiError::internal("the Google Drive connection could not be stored"))?;
    match host.dispatch(Request::SettingsSet {
        key: BACKUP_DRIVE_SETTING.to_owned(),
        value,
    })? {
        Response::Ok => Ok(()),
        _ => Err(unexpected()),
    }
}

/// The connection of a connected, usable Drive: `notConnected` and `reconnect` otherwise.
fn usable_connection(host: &dyn CoreHost) -> Result<DriveConnection, ApiError> {
    let connection = read_connection(host)?;
    if connection.connected_at.is_none() {
        Err(Reason::NotConnected.error())
    } else if connection.needs_reconnect {
        Err(Reason::Reconnect.error())
    } else {
        Ok(connection)
    }
}

/// What Settings shows, from the setting and the client the build has (no network, no keystore).
fn status_of(shell: &DriveShell, connection: &DriveConnection) -> DriveStatus {
    DriveStatus {
        client: shell.client_source(connection),
        connected: connection.connected_at.is_some(),
        account: connection.account.clone(),
        folder_url: connection.folder_id.as_deref().map(kb_drive::folder_url),
        own_client_id: non_blank(connection.own_client_id.as_deref()),
        needs_reconnect: connection.needs_reconnect,
        last: connection.last.clone(),
    }
}

/// The Drive target's state for Settings (no network). While the core is locked or starting it cannot be read:
/// the answer is then that of a Drive that is not connected, with the client the build has (the page asks again
/// when the core is ready, and never shows it before).
pub(crate) fn status(host: &dyn CoreHost, drive: &DriveShell) -> Result<DriveStatus, ApiError> {
    let connection = if host.ensure_ready().is_ok() {
        read_connection(host)?
    } else {
        DriveConnection::default()
    };
    Ok(status_of(drive, &connection))
}

// --- randomness ---

/// Lower-case hexadecimal text of `bytes`.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

// --- the flows' shared context ---

/// What a flow uses: the core, the shell's state, the transport and the client.
struct Ctx<'a> {
    host: &'a dyn CoreHost,
    shell: &'a DriveShell,
    net: &'a Net,
    client: OAuthClient,
}

impl Ctx<'_> {
    /// Remembers that Google refused the connection (best effort).
    fn mark_reconnect(&self) {
        self.shell.drop_access();
        let stored = self
            .shell
            .update_connection(self.host, |connection| connection.needs_reconnect = true);
        if let Err(error) = stored {
            tracing::warn!(code = ?error.code, "the need to reconnect could not be stored");
        }
    }

    /// An access token (and whether it was just made): the one in memory while it lasts, else a refresh. A grant
    /// Google no longer knows is `Reconnect`, and the connection is marked.
    async fn access(&self, force: bool) -> Result<(String, bool), Fail> {
        if !force && let Some(token) = self.shell.cached_access() {
            return Ok((token, false));
        }
        let Some(refresh) = self.shell.load_refresh_token()? else {
            // The connection says it is made but the keystore has no token (cleared by the person, say).
            self.mark_reconnect();
            return Err(Fail::Drive(DriveError::Reconnect));
        };
        let response = self
            .net
            .send(kb_drive::refresh_request(&self.client, &refresh))
            .await?;
        match kb_drive::parse_token(&response) {
            Ok(tokens) => {
                self.shell
                    .cache_access(&tokens.access_token, tokens.expires_in_s);
                if let Some(rotated) = tokens.refresh_token
                    && let Err(error) = self
                        .shell
                        .token_entry()
                        .and_then(|entry| self.shell.store_secret(&entry, &rotated))
                {
                    tracing::warn!(code = ?error.code, "a new refresh token could not be stored");
                }
                Ok((tokens.access_token, true))
            }
            Err(DriveError::Reconnect) => {
                self.mark_reconnect();
                Err(Fail::Drive(DriveError::Reconnect))
            }
            Err(other) => Err(Fail::Drive(other)),
        }
    }

    /// Runs `run` with an access token. A 401 with a token from memory is tried once more after a refresh; a 401
    /// with a fresh token is `Reconnect`, and the connection is marked.
    async fn authorized<T, F, Fut>(&self, mut run: F) -> Result<T, Fail>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Result<T, Fail>>,
    {
        let (token, fresh) = self.access(false).await?;
        let first = run(token).await;
        if !first.as_ref().is_err_and(Fail::is_reconnect) {
            return first;
        }
        if fresh {
            self.mark_reconnect();
            return first;
        }
        self.shell.drop_access();
        let (token, _) = self.access(true).await?;
        let second = run(token).await;
        if second.as_ref().is_err_and(Fail::is_reconnect) {
            self.mark_reconnect();
        }
        second
    }

    /// An authorised request and its parsed answer.
    async fn call<T>(
        &self,
        build: impl Fn(&str) -> HttpRequest,
        parse: impl Fn(&HttpResponse) -> Result<T, DriveError>,
    ) -> Result<T, Fail> {
        let (build, parse) = (&build, &parse);
        self.authorized(|token| {
            let request = build(&token);
            async move {
                let response = self.net.send(request).await?;
                parse(&response).map_err(Fail::from)
            }
        })
        .await
    }

    /// The first folder this app made that is called "atomik-kb backups" and is not in the trash.
    async fn find_folder(&self) -> Result<Option<DriveFile>, Fail> {
        let (files, _) = self
            .call(kb_drive::find_folder_request, kb_drive::parse_file_list)
            .await?;
        Ok(files.into_iter().next())
    }

    /// The backups folder, made when there is none.
    async fn ensure_folder(&self) -> Result<DriveFile, Fail> {
        if let Some(folder) = self.find_folder().await? {
            return Ok(folder);
        }
        self.call(kb_drive::create_folder_request, kb_drive::parse_file)
            .await
    }

    /// The backups of a folder, newest first (as Drive orders them), over at most [`MAX_PAGES`] pages.
    async fn list_all(&self, folder_id: &str) -> Result<Vec<DriveFile>, Fail> {
        let mut files = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let (found, next) = self
                .call(
                    |token| kb_drive::list_backups_request(token, folder_id, page.as_deref()),
                    kb_drive::parse_file_list,
                )
                .await?;
            files.extend(
                found
                    .into_iter()
                    .filter(|file| kb_drive::is_backup_name(&file.name)),
            );
            match next {
                Some(token) => page = Some(token),
                None => break,
            }
        }
        Ok(files)
    }

    /// Remembers the folder's id when it is not the one the setting has (best effort).
    fn remember_folder(&self, known: Option<&str>, folder_id: &str) {
        if known == Some(folder_id) {
            return;
        }
        let stored = self.shell.update_connection(self.host, |connection| {
            if connection.connected_at.is_some() {
                connection.folder_id = Some(folder_id.to_owned());
            }
        });
        if let Err(error) = stored {
            tracing::warn!(code = ?error.code, "the folder could not be stored");
        }
    }
}

/// Builds the transport and the context for a flow.
fn context<'a>(
    host: &'a dyn CoreHost,
    shell: &'a DriveShell,
    net: &'a Net,
    connection: &DriveConnection,
) -> Result<Ctx<'a>, ApiError> {
    let client = shell
        .client(connection)?
        .ok_or_else(|| Reason::NoClient.error())?;
    Ok(Ctx {
        host,
        shell,
        net,
        client,
    })
}

/// Resolves when `cancel` turns true; never when its sender is gone.
async fn cancelled_signal(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Runs a flow on a blocking thread, driven there with `block_on`, so that neither the core's lock nor a dialog nor
/// the network ever holds an async worker.
async fn on_blocking_thread<T, F, Fut>(flow: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    blocking(
        move || tauri::async_runtime::block_on(flow()),
        DRIVE_STOPPED,
    )
    .await
}

// --- connecting ---

/// Opens the system browser on a URL.
pub(crate) trait Browser: Send + Sync {
    /// Opens `url`.
    ///
    /// # Errors
    ///
    /// `Internal` when the system cannot open it.
    fn open(&self, url: &str) -> Result<(), ApiError>;
}

/// The system browser, through `tauri-plugin-opener` from Rust (as `open_external`; the webview has no permission).
pub(crate) struct SystemBrowser<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> SystemBrowser<R> {
    /// The browser the app's opener starts.
    pub(crate) fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> Browser for SystemBrowser<R> {
    fn open(&self, url: &str) -> Result<(), ApiError> {
        self.app.opener().open_url(url, None::<&str>).map_err(|_| {
            // The OS error can carry the address, which holds the sign-in's state: not logged.
            tracing::warn!("the system could not open the browser");
            ApiError::internal("the browser could not be opened")
        })
    }
}

/// `drive_connect`: the native notice, the sign-in in the browser, the account and the folder.
///
/// # Errors
///
/// `Locked`, or "core is starting", before anything is shown; `InvalidInput` for the field `drive` with the
/// reason `noClient`, `consentDeclined`, `cancelled`, `browserTimeout`, `denied`, `network`, `server`.
pub(crate) async fn connect(
    host: Arc<dyn CoreHost>,
    drive: Arc<DriveShell>,
    confirm: Arc<dyn Confirm>,
    browser: Arc<dyn Browser>,
) -> Result<DriveStatus, ApiError> {
    on_blocking_thread(
        move || async move { connect_flow(&*host, &drive, &*confirm, &*browser).await },
    )
    .await
}

async fn connect_flow(
    host: &dyn CoreHost,
    shell: &DriveShell,
    confirm: &dyn Confirm,
    browser: &dyn Browser,
) -> Result<DriveStatus, ApiError> {
    host.ensure_ready()?;
    let connection = read_connection(host)?;
    // No client, no point in asking the person anything.
    let client = shell
        .client(&connection)?
        .ok_or_else(|| Reason::NoClient.error())?;
    // Before the notice, so that a cancel that comes while it is open still counts.
    let (_running, mut cancel) = shell.begin(Operation::Connect);
    let labels = confirm.labels();
    let question = Question {
        title: labels.dialog_drive_title.clone(),
        message: labels.dialog_drive_message.clone(),
        accept: labels.dialog_drive_accept.clone(),
        cancel: labels.dialog_drive_cancel.clone(),
    };
    if !confirm.ask(&question) {
        return Err(Reason::ConsentDeclined.error());
    }
    if *cancel.borrow() {
        return Err(Reason::Cancelled.error());
    }
    let net = Net::new(shell.config())?;
    let listener = loopback::Loopback::bind().await.map_err(|_| {
        tracing::warn!("the loopback address could not be bound");
        ApiError::internal("the sign-in could not be started")
    })?;
    let redirect_uri = listener.redirect_uri();
    let state = hex(&OsRng.random_array::<32>());
    let pkce = kb_drive::pkce(OsRng.random_array::<32>());
    browser.open(&kb_drive::authorize_url(
        &client,
        &redirect_uri,
        &state,
        &pkce.challenge,
    ))?;
    tracing::info!("waiting for the browser to bring the Google sign-in back");
    let code = listener
        .wait_for_code(
            &state,
            &labels.dialog_drive_done,
            &mut cancel,
            shell.config().browser_timeout,
        )
        .await
        .map_err(Reason::error)?;
    let ctx = Ctx {
        host,
        shell,
        net: &net,
        client,
    };
    let tokens = exchange(&ctx, &code, &pkce.verifier, &redirect_uri)
        .await
        .map_err(|fail| {
            fail.log("code exchange");
            ApiError::from(fail)
        })?;
    let Some(refresh) = tokens.refresh_token else {
        tracing::warn!("Google gave no refresh token");
        return Err(Reason::Server.error());
    };
    shell.store_secret(&shell.token_entry()?, &refresh)?;
    shell.cache_access(&tokens.access_token, tokens.expires_in_s);
    let finished = finish_connect(&ctx, &connection).await;
    match finished {
        Ok(connected) => {
            tracing::info!("Google Drive connected");
            Ok(status_of(shell, &connected))
        }
        Err(error) => {
            // The token must not outlive a connection that was not made.
            forget(shell, Some(&net)).await;
            Err(error)
        }
    }
}

/// The code exchange.
async fn exchange(
    ctx: &Ctx<'_>,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<kb_drive::Tokens, Fail> {
    let response = ctx
        .net
        .send(kb_drive::token_request(
            &ctx.client,
            code,
            verifier,
            redirect_uri,
        ))
        .await?;
    Ok(kb_drive::parse_token(&response)?)
}

/// The account's address and the folder, then the setting.
async fn finish_connect(
    ctx: &Ctx<'_>,
    before: &DriveConnection,
) -> Result<DriveConnection, ApiError> {
    let setup = async {
        let account = ctx
            .call(kb_drive::about_request, kb_drive::parse_about)
            .await?;
        let folder = ctx.ensure_folder().await?;
        Ok::<_, Fail>((account, folder))
    };
    let (account, folder) = setup.await.map_err(|fail| {
        fail.log("account and folder");
        ApiError::from(fail)
    })?;
    ctx.shell.update_connection(ctx.host, |connection| {
        *connection = DriveConnection {
            account,
            folder_id: Some(folder.id),
            connected_at: Some(SystemClock.now_ms()),
            own_client_id: before.own_client_id.clone(),
            needs_reconnect: false,
            last: None,
        };
    })
}

// --- disconnecting and choosing the client ---

/// Revokes the stored refresh token (best effort: with no answer in time it stays unrevoked), deletes the keystore
/// entry and forgets the access token.
async fn forget(shell: &DriveShell, net: Option<&Net>) {
    if let (Some(net), Ok(Some(token))) = (net, shell.load_refresh_token()) {
        let revoke = net.send(kb_drive::revoke_request(&token));
        match tokio::time::timeout(REVOKE_TIMEOUT, revoke).await {
            Ok(Ok(_)) => tracing::info!("the Google token was revoked"),
            _ => tracing::info!("the Google token could not be revoked"),
        }
    }
    if let Err(error) = shell
        .token_entry()
        .and_then(|entry| shell.remove_secret(&entry))
    {
        tracing::warn!(code = ?error.code, "the refresh token could not be deleted");
    }
    shell.drop_access();
}

/// Disconnects: [`forget`], then the setting without the connection (`ownClientId` stays).
async fn disconnect_steps(
    host: &dyn CoreHost,
    shell: &DriveShell,
) -> Result<DriveConnection, ApiError> {
    let net = Net::new(shell.config()).ok();
    forget(shell, net.as_ref()).await;
    shell.update_connection(host, |connection| {
        let own_client_id = connection.own_client_id.take();
        *connection = DriveConnection {
            own_client_id,
            ..DriveConnection::default()
        };
    })
}

/// `drive_disconnect`: revokes the token at Google (errors ignored), deletes the keystore entry and clears the
/// connection. The files stay in Drive.
///
/// # Errors
///
/// `Locked`, or "core is starting"; whatever the core answers.
pub(crate) async fn disconnect(
    host: Arc<dyn CoreHost>,
    drive: Arc<DriveShell>,
) -> Result<DriveStatus, ApiError> {
    on_blocking_thread(move || async move {
        host.ensure_ready()?;
        let connection = disconnect_steps(&*host, &drive).await?;
        tracing::info!("Google Drive disconnected");
        Ok(status_of(&drive, &connection))
    })
    .await
}

/// A client id or secret as the keystore and the setting keep it: trimmed, 1 to 256 characters, no control
/// characters; a secret that is blank is none.
fn checked_client_text(field: &str, text: &str) -> Result<Option<String>, ApiError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().count() > MAX_CLIENT_CHARS || text.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            field,
            "this is 1 to 256 characters without control characters",
        ));
    }
    Ok(Some(text.to_owned()))
}

/// `drive_client_set`: one's own Google client (`None`: the built-in one). A token is bound to its client, so
/// this disconnects first.
///
/// # Errors
///
/// `Locked`, or "core is starting"; `InvalidInput` for `clientId` or `clientSecret` when a text is not one;
/// `KeystoreUnavailable` when the secret cannot be stored.
pub(crate) async fn client_set(
    host: Arc<dyn CoreHost>,
    drive: Arc<DriveShell>,
    client: Option<DriveClientInput>,
) -> Result<DriveStatus, ApiError> {
    on_blocking_thread(move || async move {
        host.ensure_ready()?;
        let (own_id, own_secret) = match &client {
            Some(input) => {
                let id = checked_client_text("clientId", &input.client_id)?
                    .ok_or_else(|| ApiError::invalid("clientId", "this cannot be empty"))?;
                let secret = match &input.client_secret {
                    Some(secret) => checked_client_text("clientSecret", secret)?,
                    None => None,
                };
                (Some(id), secret)
            }
            None => (None, None),
        };
        disconnect_steps(&*host, &drive).await?;
        // The old secret never pairs with another id.
        match own_secret {
            Some(secret) => drive.store_secret(&drive.secret_entry()?, &secret)?,
            None => {
                if let Err(error) = drive
                    .secret_entry()
                    .and_then(|entry| drive.remove_secret(&entry))
                {
                    tracing::warn!(code = ?error.code, "the client secret could not be deleted");
                }
            }
        }
        let connection = drive.update_connection(&*host, |connection| {
            connection.own_client_id = own_id;
        })?;
        tracing::info!("the Google client was set");
        Ok(status_of(&drive, &connection))
    })
    .await
}

/// `drive_cancel`: ends a sign-in that waits for the browser and a download that runs.
pub(crate) fn cancel(drive: &DriveShell) {
    drive.cancel();
}

// --- listing ---

/// `drive_backups_list`: the `.atomikkb` files of the folder, newest first, at most 200. The folder is looked up
/// again when its id is unknown or gone; if there is none, the list is empty.
///
/// # Errors
///
/// `Locked`, or "core is starting"; `notConnected`, `reconnect`, `network`, `server`.
pub(crate) async fn backups_list(
    host: Arc<dyn CoreHost>,
    drive: Arc<DriveShell>,
) -> Result<Vec<BackupFileInfo>, ApiError> {
    on_blocking_thread(move || async move { list_flow(&*host, &drive).await }).await
}

async fn list_flow(
    host: &dyn CoreHost,
    shell: &DriveShell,
) -> Result<Vec<BackupFileInfo>, ApiError> {
    host.ensure_ready()?;
    let connection = usable_connection(host)?;
    let net = Net::new(shell.config())?;
    let ctx = context(host, shell, &net, &connection)?;
    let listed = list_folder(&ctx, connection.folder_id.as_deref())
        .await
        .map_err(|fail| {
            fail.log("list");
            ApiError::from(fail)
        })?;
    let mut backups: Vec<BackupFileInfo> = listed
        .into_iter()
        .map(|file| BackupFileInfo {
            id: file.id,
            name: file.name,
            size: file.size.unwrap_or(0),
            modified_at: file.modified_time_ms.unwrap_or(0),
        })
        .collect();
    backups.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.name.cmp(&a.name))
    });
    backups.truncate(MAX_LISTED);
    Ok(backups)
}

/// The backups of the folder `known` names, or of the folder found again when that one is unknown or gone.
async fn list_folder(ctx: &Ctx<'_>, known: Option<&str>) -> Result<Vec<DriveFile>, Fail> {
    if let Some(id) = known {
        match ctx.list_all(id).await {
            Err(Fail::Drive(DriveError::NotFound)) => {}
            other => return other,
        }
    }
    match ctx.find_folder().await? {
        Some(folder) => {
            ctx.remember_folder(known, &folder.id);
            ctx.list_all(&folder.id).await
        }
        None => Ok(Vec::new()),
    }
}

// --- downloading ---

/// What a Drive file id looks like: 1 to 128 letters, digits, `-` and `_`.
fn is_file_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// What reports a download's progress: `false` means the page is gone.
pub(crate) type Emit = Arc<dyn Fn(DriveProgress) -> bool + Send + Sync>;

/// `drive_backup_download`: a backup into `<app data>/restore-staging/<random>.atomikkb` (the folder is emptied
/// first), progress at most every 250 ms through `emit`. Answers the absolute path.
///
/// # Errors
///
/// `Locked`, or "core is starting"; `InvalidInput` for `fileId` when it is not a Drive id; `notConnected`,
/// `reconnect`, `notFound`, `cancelled`, `network`, `server`; `Storage` when the file cannot be written.
pub(crate) async fn download(
    host: Arc<dyn CoreHost>,
    drive: Arc<DriveShell>,
    file_id: String,
    emit: Emit,
) -> Result<String, ApiError> {
    on_blocking_thread(move || async move { download_flow(&*host, &drive, &file_id, &*emit).await })
        .await
}

async fn download_flow(
    host: &dyn CoreHost,
    shell: &DriveShell,
    file_id: &str,
    emit: &(dyn Fn(DriveProgress) -> bool + Send + Sync),
) -> Result<String, ApiError> {
    host.ensure_ready()?;
    if !is_file_id(file_id) {
        return Err(ApiError::invalid(
            "fileId",
            "this is not a Google Drive file id",
        ));
    }
    let connection = usable_connection(host)?;
    let (_running, mut cancel) = shell.begin(Operation::Download);
    let path = staging_file(shell)?;
    let net = Net::new(shell.config())?;
    let ctx = context(host, shell, &net, &connection)?;
    let streamed = stream_download(&ctx, file_id, &path, &mut cancel, emit).await;
    match streamed {
        Ok(()) => {
            tracing::info!("a backup was downloaded from Google Drive");
            Ok(path_text(&path))
        }
        Err(fail) => {
            let _ = fs::remove_file(&path);
            fail.log("download");
            Err(fail.into())
        }
    }
}

/// A new staging file's path, the staging folder emptied first (a previous download is not kept).
fn staging_file(shell: &DriveShell) -> Result<PathBuf, ApiError> {
    let dir = shell.data_dir()?.join(RESTORE_STAGING_DIR);
    fs::create_dir_all(&dir)
        .map_err(|_| ApiError::storage("the folder for the download cannot be made"))?;
    let _ = empty_staging(&dir);
    Ok(dir.join(format!(
        "{}.{BACKUP_EXTENSION}",
        hex(&OsRng.random_array::<16>())
    )))
}

/// Downloads `file_id` into `path`, piece by piece.
async fn stream_download(
    ctx: &Ctx<'_>,
    file_id: &str,
    path: &Path,
    cancel: &mut watch::Receiver<bool>,
    emit: &(dyn Fn(DriveProgress) -> bool + Send + Sync),
) -> Result<(), Fail> {
    let started = ctx.authorized(|token| {
        let request = kb_drive::download_request(&token, file_id);
        async move { ctx.net.download(request).await }
    });
    let mut response: HttpStream = tokio::select! {
        response = started => response?,
        () = cancelled_signal(cancel) => return Err(cancelled()),
    };
    let total = response.content_length();
    let mut file = File::create(path)
        .map_err(|_| Fail::Api(ApiError::storage("the backup file cannot be written")))?;
    if !emit(DriveProgress { received: 0, total }) {
        return Err(cancelled());
    }
    let mut received: u64 = 0;
    let mut last = Instant::now();
    loop {
        let piece = tokio::select! {
            piece = tokio::time::timeout(ctx.net.read_timeout(), response.chunk()) => piece,
            () = cancelled_signal(cancel) => return Err(cancelled()),
        };
        let piece = match piece {
            Ok(Ok(Some(piece))) => piece,
            Ok(Ok(None)) => break,
            Ok(Err(_)) | Err(_) => {
                tracing::warn!(kind = "body", "a request to Google failed");
                return Err(Fail::Network);
            }
        };
        file.write_all(&piece)
            .map_err(|_| Fail::Api(ApiError::storage("the backup file cannot be written")))?;
        received = received.saturating_add(u64::try_from(piece.len()).unwrap_or(u64::MAX));
        if last.elapsed() >= PROGRESS_INTERVAL {
            last = Instant::now();
            if !emit(DriveProgress { received, total }) {
                return Err(cancelled());
            }
        }
    }
    file.sync_all()
        .map_err(|_| Fail::Api(ApiError::storage("the backup file cannot be written")))?;
    if total.is_some_and(|total| total != received) {
        return Err(Fail::Network);
    }
    Ok(())
}

// --- delivering ---

/// What a delivery did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DriveDelivery {
    /// Drive is not connected: nothing was sent (not an error).
    NotConnected,
    /// The file is in Drive; this many old scheduled backups were deleted there.
    Uploaded {
        /// Old scheduled backups deleted in the folder.
        pruned: usize,
    },
}

/// Uploads the backup at `file` to the Drive folder under `name`, then keeps the newest `keep` of the
/// scheduled backups there, and records the outcome in `app.backup.drive` (`last`). Called by the
/// scheduler after each export; blocking (run it off the async runtime's core threads).
///
/// # Errors
///
/// `reconnect` (also when the connection already needs it: no request is made), `noClient`, `quota`,
/// `notFound`, `network`, `server`; `Storage` when the file cannot be read. Failing to prune is not one.
pub(crate) fn deliver(
    host: &dyn CoreHost,
    drive: &DriveShell,
    file: &Path,
    name: &str,
    keep: u32,
) -> Result<DriveDelivery, ApiError> {
    let connection = read_connection(host)?;
    if connection.connected_at.is_none() {
        return Ok(DriveDelivery::NotConnected);
    }
    tauri::async_runtime::block_on(deliver_flow(host, drive, connection, file, name, keep))
}

async fn deliver_flow(
    host: &dyn CoreHost,
    shell: &DriveShell,
    connection: DriveConnection,
    file: &Path,
    name: &str,
    keep: u32,
) -> Result<DriveDelivery, ApiError> {
    let at = SystemClock.now_ms();
    let outcome = deliver_steps(host, shell, &connection, file, name, keep).await;
    record_last(host, shell, at, name, &outcome);
    match &outcome {
        Ok(pruned) => tracing::info!(pruned, "a backup was delivered to Google Drive"),
        Err(error) => {
            tracing::warn!(
                reason = reason_of(error).unwrap_or("other"),
                "delivery to Google Drive failed"
            );
        }
    }
    outcome.map(|pruned| DriveDelivery::Uploaded { pruned })
}

async fn deliver_steps(
    host: &dyn CoreHost,
    shell: &DriveShell,
    connection: &DriveConnection,
    file: &Path,
    name: &str,
    keep: u32,
) -> Result<usize, ApiError> {
    if connection.needs_reconnect {
        return Err(Reason::Reconnect.error());
    }
    let net = Net::new(shell.config())?;
    let ctx = context(host, shell, &net, connection)?;
    let step = |label: &'static str| {
        move |fail: Fail| {
            fail.log(label);
            ApiError::from(fail)
        }
    };
    let folder = ctx.ensure_folder().await.map_err(step("folder"))?;
    ctx.remember_folder(connection.folder_id.as_deref(), &folder.id);
    let uploaded = upload_file(&ctx, &folder.id, file, name)
        .await
        .map_err(step("upload"))?;
    Ok(prune(&ctx, &folder.id, &uploaded.id, keep).await)
}

/// Records the outcome in `app.backup.drive.last`; a failure keeps the previous success (when, and which file).
/// A refusal that means "connect again" also marks the connection. Best effort.
fn record_last(
    host: &dyn CoreHost,
    shell: &DriveShell,
    at: i64,
    name: &str,
    outcome: &Result<usize, ApiError>,
) {
    let reconnect = outcome
        .as_ref()
        .err()
        .is_some_and(|error| reason_of(error) == Some(Reason::Reconnect.as_str()));
    let stored = shell.update_connection(host, |connection| {
        if connection.connected_at.is_none() {
            // Disconnected while it ran: nothing to record on.
            return;
        }
        let previous = connection.last.take();
        connection.last = Some(match outcome {
            Ok(_) => BackupRunStatus {
                at,
                ok: true,
                last_success_at: Some(at),
                file: Some(name.to_owned()),
                error: None,
            },
            Err(error) => BackupRunStatus {
                at,
                ok: false,
                last_success_at: previous.as_ref().and_then(|status| status.last_success_at),
                file: previous.and_then(|status| status.file),
                error: Some(error.to_string()),
            },
        });
        if reconnect {
            connection.needs_reconnect = true;
        }
    });
    if let Err(error) = stored {
        tracing::warn!(code = ?error.code, "the outcome of a Drive delivery could not be stored");
    }
}

/// Reads up to one chunk of `file` at `offset` (none at or past `total`).
fn read_chunk(file: &mut File, offset: u64, total: u64) -> Result<Vec<u8>, Fail> {
    let changed = || {
        Fail::Api(ApiError::storage(
            "the backup file changed while it was uploaded",
        ))
    };
    let want = total
        .saturating_sub(offset)
        .min(u64::try_from(CHUNK_SIZE).unwrap_or(u64::MAX));
    let mut bytes = Vec::with_capacity(usize::try_from(want).unwrap_or(0));
    file.seek(SeekFrom::Start(offset)).map_err(|_| changed())?;
    Read::take(&mut *file, want)
        .read_to_end(&mut bytes)
        .map_err(|_| changed())?;
    if u64::try_from(bytes.len()).ok() != Some(want) {
        return Err(changed());
    }
    Ok(bytes)
}

/// A chunk's (or status query's) answer as the step it means.
async fn send_step(net: &Net, request: HttpRequest) -> Result<UploadStep, Fail> {
    let response = net.send(request).await?;
    Ok(kb_drive::parse_upload_step(&response)?)
}

/// Starts the resumable upload; a transient failure is tried again after the same waits as a chunk's.
async fn start_session(
    ctx: &Ctx<'_>,
    folder_id: &str,
    name: &str,
    total: u64,
) -> Result<String, Fail> {
    let backoff = ctx.shell.config().backoff;
    let mut failures = 0;
    loop {
        let started = ctx
            .call(
                |token| kb_drive::upload_start_request(token, folder_id, name, total),
                kb_drive::parse_upload_session,
            )
            .await;
        match started {
            Err(fail) if fail.transient() && failures < RETRIES => {
                tokio::time::sleep(backoff.get(failures).copied().unwrap_or_default()).await;
                failures += 1;
            }
            other => return other,
        }
    }
}

/// Uploads `path` as `name` into the folder: a resumable session, then the file in `CHUNK_SIZE` pieces. A piece
/// that fails with the network, a 5xx or a rate limit asks the session where it stands and goes on from there,
/// up to [`RETRIES`] times in a row.
async fn upload_file(
    ctx: &Ctx<'_>,
    folder_id: &str,
    path: &Path,
    name: &str,
) -> Result<DriveFile, Fail> {
    let unreadable = || Fail::Api(ApiError::storage("the backup file cannot be read"));
    let mut file = File::open(path).map_err(|_| unreadable())?;
    let total = file.metadata().map_err(|_| unreadable())?.len();
    let session = start_session(ctx, folder_id, name, total).await?;
    let backoff = ctx.shell.config().backoff;
    let mut offset = 0_u64;
    let mut failures = 0_usize;
    loop {
        let bytes = read_chunk(&mut file, offset, total)?;
        let mut result = send_step(
            ctx.net,
            kb_drive::upload_chunk_request(&session, offset, bytes, total),
        )
        .await;
        while let Err(fail) = &result {
            if !fail.transient() {
                break;
            }
            failures += 1;
            if failures > RETRIES {
                break;
            }
            tokio::time::sleep(backoff.get(failures - 1).copied().unwrap_or_default()).await;
            result = send_step(ctx.net, kb_drive::upload_status_request(&session, total)).await;
        }
        match result? {
            UploadStep::Done(uploaded) => return Ok(uploaded),
            UploadStep::Continue { next_offset } => {
                if next_offset > total {
                    return Err(Fail::Drive(DriveError::Protocol(
                        "upload: the stored size is larger than the file".to_owned(),
                    )));
                }
                if next_offset > offset {
                    failures = 0;
                } else {
                    // Drive stored nothing of what was sent: not progress, and not for ever.
                    failures += 1;
                    if failures > RETRIES {
                        return Err(Fail::Drive(DriveError::Server));
                    }
                    tokio::time::sleep(backoff.get(failures - 1).copied().unwrap_or_default())
                        .await;
                }
                offset = next_offset;
            }
        }
    }
}

/// Deletes the scheduled backups of the folder beyond the newest `keep` (the core says which): only names of the
/// scheduled form, never the file just uploaded. A failure is counted and logged, not an error: the backup is in
/// Drive. Answers how many were deleted.
async fn prune(ctx: &Ctx<'_>, folder_id: &str, keep_id: &str, keep: u32) -> usize {
    let files = match ctx.list_all(folder_id).await {
        Ok(files) => files,
        Err(fail) => {
            fail.log("prune list");
            return 0;
        }
    };
    let names: Vec<String> = files
        .iter()
        .filter(|file| kb_drive::is_scheduled_name(&file.name))
        .map(|file| file.name.clone())
        .collect();
    let doomed = backup_schedule::to_prune(&names, keep);
    let (mut deleted, mut failed) = (0_usize, 0_usize);
    for file in files
        .iter()
        .filter(|file| file.id != keep_id && doomed.contains(&file.name))
        .filter(|file| kb_drive::is_scheduled_name(&file.name))
    {
        let result = ctx
            .call(
                |token| kb_drive::delete_request(token, &file.id),
                kb_drive::parse_empty,
            )
            .await;
        match result {
            Ok(()) => deleted += 1,
            // Already gone is as good as deleted.
            Err(Fail::Drive(DriveError::NotFound)) => deleted += 1,
            Err(_) => failed += 1,
        }
    }
    if deleted > 0 || failed > 0 {
        tracing::info!(
            deleted,
            failed,
            "old scheduled backups pruned in Google Drive"
        );
    }
    deleted
}
