//! Scheduled backups to a folder and to Google Drive (ADR-0036 and ADR-0059,
//! `docs/architecture/import-export.md` 5.4, `docs/architecture/drive-backup.md` 4): the existing
//! encrypted backup file, written by the shell on a schedule into a folder the person chose and/or
//! delivered to Drive, the newest N kept in each.
//!
//! The core decides (`kb_core::backup_schedule`: whether one is due, how it is named, which old
//! ones to delete); the shell supplies the clock, the thread and the files.
//!
//! - **Settings.** `app.backup.schedule` (`BackupSchedule`, written by the UI) says whether and
//!   how often; `app.backup.folder` (a path) and `app.backup.last` (`BackupRunStatus`) are
//!   written **only here**, through the shell's own handle on the core: the `dispatch` command
//!   refuses a `SettingsSet` of either key from the webview (`guard`), so a compromised page cannot
//!   point the backups, or the deletion of old ones, at another folder.
//! - **`backup_folder_choose`** ([`choose_folder`]): the native folder dialog; the path is stored
//!   in `app.backup.folder` and answered.
//! - **Targets.** A run is possible when at least one target is set: the folder
//!   (`app.backup.folder`) or Drive (connected, [`DriveTarget::connected`]).
//! - **`backup_run_now`** ([`run_now`]): one export per run. With a folder it writes
//!   `<folder>/atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb` with the backup export (atomic:
//!   `<file>.tmp`, then renamed) and deletes the scheduled backups beyond the newest `keep` (only
//!   files named exactly like that, only in that folder, and only after the new one is on disk);
//!   without one (or when it is not there) the export goes to
//!   `<data dir>/backup-staging/<name>` under the same rule. When Drive is connected the file is then
//!   delivered to it ([`DriveTarget::deliver`]); the staging file is deleted afterwards whatever
//!   happened. The outcome is recorded in `app.backup.last`, on failure too: `ok` only when every
//!   target that is on succeeded, `error` the first failure (the error code and message, never a
//!   path). A folder that is not there is a failed attempt like any other (Drive still gets its
//!   copy); no target at all is one too.
//! - **The timer's retry after a Drive failure exports nothing.** When the previous run failed with a file in
//!   the folder (`app.backup.last.file`: the folder worked, Drive did not), that file is still
//!   there, is newer than anything Drive has and its period is not over, the run delivers it to
//!   Drive again and touches nothing else: the hourly retries never fill the folder with copies of
//!   one period and never push its older backups out through the pruning. With no folder the retry
//!   exports again (into staging); a new period always makes a normal export. A person's press of
//!   "Back up now" ([`run_now`]) always exports afresh.
//! - **The timer** ([`scheduler_starter`]): background work like the reminders': it starts when the
//!   core becomes ready (at launch, after an unlock), first looks two minutes later, then every ten
//!   minutes, runs the same when a backup is due and a target is set, and stops when the core
//!   locks. The core holds its lock for the whole export, as for a manual one, and not for the
//!   upload. When the core becomes ready the staging folders are emptied ([`Targets::clear_staging`]).
//!
//! Logs carry counts and error codes only, never a path or a file name.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, TryLockError, Weak};
use std::time::Duration;

use kb_core::backup_schedule;
use kb_types::interop::{
    BACKUP_AUTO_PREFIX, BACKUP_FOLDER_SETTING, BACKUP_LAST_SETTING, BACKUP_SCHEDULE_SETTING,
};
use kb_types::{
    AccountId, ApiError, BackupRunStatus, BackupSchedule, BackupSummary, Clock, Json, Request,
    Response,
};

use crate::backup::{
    BACKUP_EXTENSION, BACKUP_STAGING_DIR, BackupResult, RESTORE_STAGING_DIR, empty_staging,
    path_text, temp_path, write_atomically, write_error,
};
use crate::drive_host::{self, DriveDelivery, DriveShell};
use crate::reminders::Stop;
use crate::state::{BackgroundStarter, BackgroundTask, CoreHost};

/// How long after the core is ready (or unlocked) the timer first looks.
pub const FIRST_RUN_DELAY: Duration = Duration::from_secs(2 * 60);

/// How often the timer looks after that.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// The most scheduled backups kept, whatever the setting says (the UI offers 1 to 100).
const MAX_KEEP: u32 = 100;

// --- the targets ---

/// Google Drive as the second target of a run (ADR-0059): the seam between the scheduler and the
/// shell's Drive host (`drive_host`), so that the scheduler is tested without a network.
pub trait DriveTarget: Send + Sync + 'static {
    /// Whether Drive is connected, which makes it a target of the run. No network.
    fn connected(&self, host: &dyn CoreHost) -> bool;

    /// The name of the newest scheduled backup Drive got, by its own record (its last success), when
    /// it has had one. No network.
    fn newest_delivered(&self, host: &dyn CoreHost) -> Option<String>;

    /// Uploads the backup at `file` under `name` and keeps the newest `keep` scheduled backups in
    /// Drive. `Ok(false)` when Drive turned out not to be connected: nothing was sent. Blocking.
    ///
    /// # Errors
    ///
    /// Whatever the delivery failed with (`details.reason` says why, for the Drive failures).
    fn deliver(
        &self,
        host: &dyn CoreHost,
        file: &Path,
        name: &str,
        keep: u32,
    ) -> Result<bool, ApiError>;
}

impl DriveTarget for DriveShell {
    fn connected(&self, host: &dyn CoreHost) -> bool {
        drive_host::status(host, self).is_ok_and(|status| status.connected)
    }

    fn newest_delivered(&self, host: &dyn CoreHost) -> Option<String> {
        let last = drive_host::status(host, self).ok()?.last?;
        // A failure keeps the previous success's file, so only a record with a success has a name.
        last.last_success_at.and(last.file)
    }

    fn deliver(
        &self,
        host: &dyn CoreHost,
        file: &Path,
        name: &str,
        keep: u32,
    ) -> Result<bool, ApiError> {
        match drive_host::deliver(host, self, file, name, keep)? {
            DriveDelivery::NotConnected => Ok(false),
            DriveDelivery::Uploaded { pruned } => {
                tracing::info!(pruned, "scheduled backup delivered to Google Drive");
                Ok(true)
            }
        }
    }
}

/// No Drive: for a shell built without the target and for tests of the folder alone.
pub struct NoDrive;

impl DriveTarget for NoDrive {
    fn connected(&self, _: &dyn CoreHost) -> bool {
        false
    }

    fn newest_delivered(&self, _: &dyn CoreHost) -> Option<String> {
        None
    }

    fn deliver(&self, _: &dyn CoreHost, _: &Path, _: &str, _: u32) -> Result<bool, ApiError> {
        Ok(false)
    }
}

/// What a run needs besides the core and the clock: Drive, and where the staging files go. Managed
/// by Tauri (inside an `Arc`) like the other shells; the timer holds the same one.
pub struct Targets {
    drive: Arc<dyn DriveTarget>,
    /// One backup at a time: the timer's and a click on "Back up now" never write the same file at
    /// once (the shell has one `Targets`).
    running: Mutex<()>,
    /// The machine-local data directory, known once the shell has chosen it (`run`).
    data_dir: Mutex<Option<PathBuf>>,
}

impl Targets {
    /// With `drive` as the Drive target; the data directory is not known yet.
    #[must_use]
    pub fn new(drive: Arc<dyn DriveTarget>) -> Self {
        Self {
            drive,
            running: Mutex::new(()),
            data_dir: Mutex::new(None),
        }
    }

    /// The folder alone: no Drive.
    #[must_use]
    pub fn without_drive() -> Self {
        Self::new(Arc::new(NoDrive))
    }

    /// Tells where the staging folders live (`<dir>/backup-staging`, `<dir>/restore-staging`).
    pub fn set_data_dir(&self, dir: &Path) {
        *self.data_dir.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir.to_owned());
    }

    fn data_dir(&self) -> Option<PathBuf> {
        self.data_dir
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The folder a run without a folder writes its file in.
    fn staging_dir(&self) -> Option<PathBuf> {
        self.data_dir().map(|dir| dir.join(BACKUP_STAGING_DIR))
    }

    /// Empties `backup-staging/` and `restore-staging/`: what a run or a download that died left.
    /// Called when the core becomes ready (launch, unlock). A run that is still going (one that
    /// outlived a lock) owns its staging file and deletes it itself, so `backup-staging/` is left
    /// alone then. Logs counts only.
    pub fn clear_staging(&self) {
        let Some(dir) = self.data_dir() else {
            return;
        };
        let (mut removed, mut failed) = empty_staging(&dir.join(RESTORE_STAGING_DIR));
        // Held while it is emptied, so that a run cannot start in between.
        let idle = match self.running.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        if idle.is_some() {
            let (more_removed, more_failed) = empty_staging(&dir.join(BACKUP_STAGING_DIR));
            removed += more_removed;
            failed += more_failed;
        }
        drop(idle);
        if removed > 0 || failed > 0 {
            tracing::info!(removed, failed, "staging folders emptied");
        }
    }
}

/// The file of a run without a folder: deleted when the run is over, whatever became of it.
struct Staged(PathBuf);

impl Drop for Staged {
    fn drop(&mut self) {
        // Best effort: what stays is emptied when the core is next ready.
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(temp_path(&self.0));
    }
}

fn unexpected() -> ApiError {
    ApiError::internal("the core answered a setting request with something unexpected")
}

// --- the settings ---

fn read_setting(host: &dyn CoreHost, key: &str) -> Result<Option<Json>, ApiError> {
    match host.dispatch(Request::SettingsGet {
        key: key.to_owned(),
    })? {
        Response::Setting(value) => Ok(value),
        _ => Err(unexpected()),
    }
}

fn write_setting(host: &dyn CoreHost, key: &str, value: Json) -> Result<(), ApiError> {
    match host.dispatch(Request::SettingsSet {
        key: key.to_owned(),
        value,
    })? {
        Response::Ok => Ok(()),
        _ => Err(unexpected()),
    }
}

/// The schedule the UI stored; the default (disabled, daily, keep 7) when there is none or it is
/// not one.
fn read_schedule(host: &dyn CoreHost) -> BackupSchedule {
    let stored = read_setting(host, BACKUP_SCHEDULE_SETTING)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<BackupSchedule>(value).ok())
        .unwrap_or_default();
    BackupSchedule {
        keep: stored.keep.clamp(1, MAX_KEEP),
        ..stored
    }
}

/// The outcome of the last attempt, when there is one.
fn read_last(host: &dyn CoreHost) -> Option<BackupRunStatus> {
    read_setting(host, BACKUP_LAST_SETTING)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// The folder chosen for scheduled backups, when there is one.
fn read_folder(host: &dyn CoreHost) -> Option<String> {
    match read_setting(host, BACKUP_FOLDER_SETTING).ok().flatten()? {
        Json::String(folder) if !folder.is_empty() => Some(folder),
        _ => None,
    }
}

// --- choosing the folder ---

/// Stores the folder `choose` asks the person for (the native folder dialog; its cancellation is
/// its error) in `app.backup.folder` and answers it as text.
///
/// # Errors
///
/// The dialog's error, `InvalidInput` for `path` when the choice is not a folder or its name is
/// not text, and whatever the core answers.
pub fn choose_folder(
    host: &dyn CoreHost,
    choose: impl FnOnce() -> Result<std::path::PathBuf, ApiError>,
) -> Result<String, ApiError> {
    let folder = choose()?;
    if !folder.is_dir() {
        return Err(ApiError::invalid("path", "the chosen path is not a folder"));
    }
    let Some(text) = folder.to_str() else {
        return Err(ApiError::invalid(
            "path",
            "the folder's name is not valid text",
        ));
    };
    write_setting(host, BACKUP_FOLDER_SETTING, Json::String(text.to_owned()))?;
    tracing::info!("backup folder chosen");
    Ok(text.to_owned())
}

// --- running a backup ---

/// Who asks for a run: a person pressing "Back up now", or the timer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// Always a fresh export: the person wants the current state, and one press cannot flood the
    /// folder.
    Person,
    /// May deliver the folder's file to Drive again after a Drive failure instead of exporting
    /// (see [`held_for_retry`]): an hourly retry could flood the folder.
    Timer,
}

/// Writes a scheduled backup now (`backup_run_now`, a person's press of "Back up now"): one
/// export, named for the clock, into the chosen folder (old ones pruned) or into a staging file,
/// delivered to Drive when it is connected; the outcome recorded in `app.backup.last` whether it
/// worked or not. Answers the path of the folder's file, or of the staging file (deleted by then)
/// when there is no folder. Always a fresh export, even right after a Drive failure: only the
/// timer's retries deliver the folder's file again ([`run_if_due`]).
///
/// # Errors
///
/// `Locked` (or the core's "starting") before anything is touched; otherwise the first failure of
/// the attempt, which is also what `app.backup.last` says: `InvalidInput` for the field `folder`
/// when no target is set or the folder is not there, `Storage` when the disk fails, the Drive
/// delivery's error (`details.reason`), whatever the core answers.
pub fn run_now(
    host: &dyn CoreHost,
    clock: &dyn Clock,
    targets: &Targets,
) -> Result<BackupResult, ApiError> {
    run_for(host, clock, targets, Trigger::Person)
}

/// A run for `trigger`: what [`run_now`] and [`run_if_due`] share.
fn run_for(
    host: &dyn CoreHost,
    clock: &dyn Clock,
    targets: &Targets,
    trigger: Trigger,
) -> Result<BackupResult, ApiError> {
    host.ensure_ready()?;
    let _one_at_a_time = targets
        .running
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let previous = read_last(host);
    // One reading of the clock for the whole attempt: the file's name and the time recorded agree,
    // and a clock read again after a long backup cannot move the next due time.
    let now = clock.now_ms();
    let attempt = write_backup(
        host,
        targets,
        trigger,
        previous.as_ref(),
        now,
        clock.local_offset_seconds(),
    );
    record(host, now, previous.as_ref(), &attempt);
    match &attempt.outcome {
        Ok((result, _)) if attempt.reused => tracing::info!(
            bytes = result.summary.bytes,
            "the folder's scheduled backup was delivered again"
        ),
        Ok((result, _)) => tracing::info!(bytes = result.summary.bytes, "scheduled backup written"),
        Err(error) => tracing::warn!(code = ?error.code, "scheduled backup failed"),
    }
    attempt.outcome.map(|(result, _)| result)
}

/// One look of the timer: a backup when the core says one is due and a target is set. `None` when
/// nothing was due (or the core is not ready); otherwise what the attempt came to.
pub fn run_if_due(
    host: &dyn CoreHost,
    clock: &dyn Clock,
    targets: &Targets,
) -> Option<Result<BackupResult, ApiError>> {
    host.ensure_ready().ok()?;
    let schedule = read_schedule(host);
    if !schedule.enabled {
        return None;
    }
    let last = read_last(host);
    if !backup_schedule::is_due(clock.now_ms(), &schedule, last.as_ref()) {
        return None;
    }
    // Nowhere to write: the settings screen asks for a folder or Drive before it offers the
    // schedule.
    if read_folder(host).is_none() && !targets.drive.connected(host) {
        return None;
    }
    Some(run_for(host, clock, targets, Trigger::Timer))
}

/// Drive said it is not connected although it was when the run began: a failed target.
fn drive_not_connected() -> ApiError {
    ApiError::invalid("drive", "Google Drive is not connected")
        .with_details(serde_json::json!({ "field": "drive", "reason": "notConnected" }))
}

/// What an attempt came to, and what it left in the folder.
struct Attempt {
    /// The result and the file's name when every target that is on succeeded, else the first
    /// failure.
    outcome: Result<(BackupResult, String), ApiError>,
    /// The scheduled backup the folder holds of this attempt (written now, or delivered again),
    /// whatever became of the other targets: the next retry delivers it to Drive again.
    folder_file: Option<String>,
    /// When the backup this attempt delivered was made, Unix ms: the attempt's own start, or the
    /// time the name of a file delivered again says.
    made_at: i64,
    /// No export was made: the folder's file was delivered again.
    reused: bool,
}

impl Attempt {
    /// An attempt that failed before it left anything in the folder.
    fn failed(error: ApiError, now_ms: i64) -> Self {
        Self {
            outcome: Err(error),
            folder_file: None,
            made_at: now_ms,
            reused: false,
        }
    }
}

/// The folder's scheduled backup that a retry delivers to Drive again instead of exporting once
/// more: the one the previous, failed run left (`app.backup.last.file`).
struct Held {
    name: String,
    path: PathBuf,
    bytes: u64,
    /// When it was made, Unix ms (to the minute, from its name).
    made_at: i64,
}

/// The backup a retry should deliver again, when there is one: the previous run failed with a file
/// in the folder (the folder worked, so Drive was what failed), the file is still there and is a
/// scheduled backup, it is newer than anything Drive has, and its period is not over (a new
/// period makes a normal export). Exporting again would only fill the folder with copies of the
/// same period and push its older backups out through the pruning.
fn held_for_retry(
    host: &dyn CoreHost,
    targets: &Targets,
    folder: &Path,
    previous: Option<&BackupRunStatus>,
    schedule: &BackupSchedule,
    now_ms: i64,
    local_offset_seconds: i32,
) -> Option<Held> {
    let name = previous.filter(|status| !status.ok)?.file.as_deref()?;
    if !is_scheduled_name(name) {
        return None;
    }
    let path = folder.join(name);
    let metadata = fs::metadata(&path).ok().filter(fs::Metadata::is_file)?;
    let made_at = backup_schedule::name_time_ms(name, local_offset_seconds)?;
    // Would a run made then be due again by now? The schedule's `enabled` does not matter here.
    let then = BackupRunStatus {
        at: made_at,
        ok: true,
        last_success_at: Some(made_at),
        file: None,
        error: None,
    };
    let period = BackupSchedule {
        enabled: true,
        ..*schedule
    };
    if backup_schedule::is_due(now_ms, &period, Some(&then)) {
        return None;
    }
    // The names are zero-padded times, so the text order is the time order.
    if targets
        .drive
        .newest_delivered(host)
        .is_some_and(|newest| newest.as_str() >= name)
    {
        return None;
    }
    Some(Held {
        name: name.to_owned(),
        path,
        bytes: metadata.len(),
        made_at,
    })
}

/// What an attempt that delivers the folder's file again knows of it: the size and the time of its
/// name (there is no export, so no summary from the core).
fn summary_of(held: &Held) -> BackupSummary {
    BackupSummary {
        bytes: held.bytes,
        documents: 0,
        settings: 0,
        created_at: held.made_at,
        account_id: AccountId::from_bytes([0; 16]),
        chunks: 0,
        chunk_bytes: 0,
    }
}

/// Delivers `held` to Drive again; the folder is not touched (no new file, nothing pruned).
fn deliver_again(host: &dyn CoreHost, targets: &Targets, held: Held, keep: u32) -> Attempt {
    let delivered = match targets.drive.deliver(host, &held.path, &held.name, keep) {
        Ok(true) => Ok(()),
        Ok(false) => Err(drive_not_connected()),
        Err(error) => {
            tracing::warn!(code = ?error.code, "delivery to Google Drive failed");
            Err(error)
        }
    };
    let summary = summary_of(&held);
    Attempt {
        folder_file: Some(held.name.clone()),
        made_at: held.made_at,
        reused: true,
        outcome: delivered.map(|()| {
            (
                BackupResult {
                    summary,
                    path: path_text(&held.path),
                },
                held.name,
            )
        }),
    }
}

/// Writes the backup file, prunes the folder and delivers to Drive; or, on the timer's retry of a
/// run whose folder part worked and only Drive failed, delivers that run's file again and exports
/// nothing.
fn write_backup(
    host: &dyn CoreHost,
    targets: &Targets,
    trigger: Trigger,
    previous: Option<&BackupRunStatus>,
    now_ms: i64,
    local_offset_seconds: i32,
) -> Attempt {
    let folder = read_folder(host);
    let drive_on = targets.drive.connected(host);
    if folder.is_none() && !drive_on {
        return Attempt::failed(
            ApiError::invalid(
                "folder",
                "no folder or Google Drive is set for scheduled backups",
            ),
            now_ms,
        );
    }
    let schedule = read_schedule(host);
    // The first failure of the targets: what the run comes to when it is the only one, and the
    // error recorded when several fail.
    let mut failure: Option<ApiError> = None;
    let folder = match folder.as_deref().map(Path::new) {
        Some(folder) if folder.is_dir() => Some(folder),
        Some(_) => {
            failure = Some(ApiError::invalid(
                "folder",
                "the folder for scheduled backups is not there",
            ));
            None
        }
        None => None,
    };
    if !drive_on && let Some(error) = failure {
        // Only the folder, and it is not there: nothing is exported.
        return Attempt::failed(error, now_ms);
    }
    if drive_on
        && trigger == Trigger::Timer
        && let Some(folder) = folder
        && let Some(held) = held_for_retry(
            host,
            targets,
            folder,
            previous,
            &schedule,
            now_ms,
            local_offset_seconds,
        )
    {
        return deliver_again(host, targets, held, schedule.keep);
    }
    let name = backup_schedule::file_name(now_ms, local_offset_seconds);
    if !is_scheduled_name(&name) {
        return Attempt::failed(
            ApiError::internal("the core produced a scheduled backup name that is not usable"),
            now_ms,
        );
    }
    // One export: into the folder when there is one, else into the staging folder, for Drive.
    let (path, _staged) = match folder {
        Some(folder) => (folder.join(&name), None),
        None => {
            let staged = targets
                .staging_dir()
                .ok_or_else(|| ApiError::internal("the folder for staged backups is not known yet"))
                .and_then(|staging| {
                    fs::create_dir_all(&staging)
                        .map_err(|error| write_error("staging folder", &error))?;
                    Ok(staging.join(&name))
                });
            match staged {
                Ok(path) => (path.clone(), Some(Staged(path))),
                Err(error) => return Attempt::failed(failure.unwrap_or(error), now_ms),
            }
        }
    };
    // Not `export_to_file`: that logs the path, and the folder of a scheduled backup stays out of
    // the logs.
    let summary = match write_atomically(&path, |sink| host.backup_export(sink)) {
        Ok(summary) => summary,
        // The export is the first failure unless the folder already failed.
        Err(error) => return Attempt::failed(failure.unwrap_or(error), now_ms),
    };
    if let Some(folder) = folder {
        prune(folder, &name, schedule.keep);
    }
    if drive_on {
        match targets.drive.deliver(host, &path, &name, schedule.keep) {
            Ok(true) => {}
            Ok(false) => {
                failure.get_or_insert_with(drive_not_connected);
            }
            Err(error) => {
                tracing::warn!(code = ?error.code, "delivery to Google Drive failed");
                failure.get_or_insert(error);
            }
        }
    }
    // `_staged` goes out of scope here, and with an early return above: the staging file is deleted
    // whatever became of the delivery.
    Attempt {
        folder_file: folder.map(|_| name.clone()),
        made_at: now_ms,
        reused: false,
        outcome: match failure {
            Some(error) => Err(error),
            None => Ok((
                BackupResult {
                    summary,
                    path: path_text(&path),
                },
                name,
            )),
        },
    }
}

/// Whether `name` has the form of a scheduled backup's file name: the prefix, no path separator,
/// the backup extension. Only such names are ever written or deleted here.
fn is_scheduled_name(name: &str) -> bool {
    name.starts_with(BACKUP_AUTO_PREFIX)
        && name.ends_with(&format!(".{BACKUP_EXTENSION}"))
        && !name.contains(['/', '\\', '\0'])
}

/// Deletes the scheduled backups of `folder` beyond the newest `keep` (the core says which).
/// Never `keep_name`, the file just written; only names of the scheduled form that are regular
/// files of the folder. A failure is counted and logged, not an error: the backup is on disk.
fn prune(folder: &Path, keep_name: &str, keep: u32) {
    let Ok(read) = fs::read_dir(folder) else {
        return;
    };
    let names: Vec<String> = read
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    let (mut deleted, mut failed) = (0_u32, 0_u32);
    for name in backup_schedule::to_prune(&names, keep) {
        if name == keep_name || !is_scheduled_name(&name) || !names.contains(&name) {
            continue;
        }
        match fs::remove_file(folder.join(&name)) {
            Ok(()) => deleted += 1,
            Err(_) => failed += 1,
        }
    }
    if deleted > 0 || failed > 0 {
        tracing::info!(deleted, failed, "old scheduled backups pruned");
    }
}

/// Writes `app.backup.last`: the attempt (`at`, when it started) and its outcome. A success dates
/// the backup it delivered (`made_at`: a file delivered again keeps the time of its own run, so the
/// schedule keeps its rhythm). A failure keeps the previous success's date, and names the file the
/// folder got of this attempt when it got one (else the previous success's): the retry delivers that
/// file to Drive again. Best effort: a status that cannot be stored is logged and the backup's own
/// outcome is still what the caller gets.
fn record(host: &dyn CoreHost, at: i64, previous: Option<&BackupRunStatus>, attempt: &Attempt) {
    let status = match &attempt.outcome {
        Ok((_, name)) => BackupRunStatus {
            at,
            ok: true,
            last_success_at: Some(attempt.made_at),
            file: Some(name.clone()),
            error: None,
        },
        Err(error) => BackupRunStatus {
            at,
            ok: false,
            last_success_at: previous.and_then(|status| status.last_success_at),
            file: attempt
                .folder_file
                .clone()
                .or_else(|| previous.and_then(|status| status.file.clone())),
            error: Some(error.to_string()),
        },
    };
    let stored = serde_json::to_value(&status)
        .map_err(|_| ApiError::internal("the backup status could not be stored"))
        .and_then(|value| write_setting(host, BACKUP_LAST_SETTING, value));
    if let Err(error) = stored {
        tracing::warn!(code = ?error.code, "the outcome of a scheduled backup could not be stored");
    }
}

// --- the timer ---

/// The running timer: dropping it stops the thread (which finishes its sleep or its backup on its
/// own: dropping never waits).
struct Timer {
    stop: Arc<Stop>,
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// The background work `lib.rs` registers next to the sync driver and the reminders: empties the
/// staging folders when the core becomes ready, waits `first_delay`, then looks every `interval`
/// whether a scheduled backup is due ([`run_if_due`]) while the core is ready. It holds the state
/// only weakly: when the state is gone the thread ends.
pub fn scheduler_starter(
    state: Weak<dyn CoreHost>,
    clock: Arc<dyn Clock>,
    targets: Arc<Targets>,
    first_delay: Duration,
    interval: Duration,
) -> BackgroundStarter {
    Box::new(move || {
        state.upgrade()?;
        // What a run or a download of an earlier session left; before this session's first run.
        targets.clear_staging();
        let stop = Arc::new(Stop::default());
        let thread_stop = Arc::clone(&stop);
        let thread_state = state.clone();
        let thread_clock = Arc::clone(&clock);
        let thread_targets = Arc::clone(&targets);
        let spawned = std::thread::Builder::new()
            .name("scheduled-backup".to_owned())
            .spawn(move || {
                run(
                    &thread_state,
                    &*thread_clock,
                    &thread_targets,
                    &thread_stop,
                    first_delay,
                    interval,
                )
            });
        match spawned {
            Ok(_) => {
                let task: BackgroundTask = Box::new(Timer { stop });
                Some(task)
            }
            Err(error) => {
                tracing::error!(%error, "the scheduled backup thread could not be started");
                None
            }
        }
    })
}

/// The thread: sleep, look, sleep; until stopped or the state is gone.
fn run(
    state: &Weak<dyn CoreHost>,
    clock: &dyn Clock,
    targets: &Targets,
    stop: &Stop,
    first_delay: Duration,
    interval: Duration,
) {
    if stop.sleep(first_delay) {
        return;
    }
    loop {
        let Some(host) = state.upgrade() else {
            return;
        };
        // The outcome is in `app.backup.last` and the log; the timer has nothing else to do with it.
        let _ = run_if_due(&*host, clock, targets);
        drop(host);
        if stop.sleep(interval) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_of_the_scheduled_form_are_written_or_deleted() {
        for name in [
            "atomik-kb-auto-2026-10-12-0930.atomikkb",
            "atomik-kb-auto-1999-01-01-0000.atomikkb",
        ] {
            assert!(is_scheduled_name(name), "{name}");
        }
        for name in [
            "",
            "atomik-kb-2026-10-12.atomikkb",
            "atomik-kb-auto-2026-10-12-0930.zip",
            "other-atomik-kb-auto-2026-10-12-0930.atomikkb",
            "atomik-kb-auto-../x.atomikkb",
            "atomik-kb-auto-2026\\x.atomikkb",
            "atomik-kb-auto-2026-10-12-0930.atomikkb\0",
            "notes.md",
        ] {
            assert!(!is_scheduled_name(name), "{name:?}");
        }
    }
}
