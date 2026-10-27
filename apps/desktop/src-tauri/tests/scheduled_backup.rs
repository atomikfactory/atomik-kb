//! Scheduled backups and `save_text_file` (ADR-0036) without a dialog: the run, the pruning, the
//! outcome recorded in `app.backup.last`, the timer, the folder setting, and the text file writer,
//! against a scripted core (`common::Fake`) and temporary folders. The pure decisions (is a backup
//! due, how is it named, which are pruned) are `kb_core::backup_schedule`'s and are tested there;
//! here they are used as they are. Google Drive as a second target (ADR-0059) goes through a fake
//! `DriveTarget`: one export for both targets, the staging file and its cleanup, the first failure
//! recorded, the retry.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use atomik_kb_desktop_lib::backup_scheduler::{self, DriveTarget, Targets, run_if_due, run_now};
use atomik_kb_desktop_lib::text_file;
use atomik_kb_desktop_lib::{AppState, CoreHost};
use kb_core::backup_schedule;
use kb_types::interop::{BACKUP_FOLDER_SETTING, BACKUP_LAST_SETTING, BACKUP_SCHEDULE_SETTING};
use kb_types::{
    ApiError, BackupInterval, BackupRunStatus, BackupSchedule, Clock, ErrorCode, Event, Json,
};
use serde_json::json;

use common::{Fake, Shared, host, names_in, record_events};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

/// 2026-10-12 09:30 UTC.
const NOW: i64 = 1_791_797_400_000;

struct TestClock {
    now: AtomicI64,
    offset: i32,
}

impl TestClock {
    fn at(now: i64) -> Self {
        Self {
            now: AtomicI64::new(now),
            offset: 2 * 3600,
        }
    }

    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    fn local_offset_seconds(&self) -> i32 {
        self.offset
    }
}

fn set(world: &Shared, key: &str, value: Json) {
    world.lock().unwrap().settings.insert(key.to_owned(), value);
}

fn schedule(enabled: bool, interval: BackupInterval, keep: u32) -> Json {
    serde_json::to_value(BackupSchedule {
        enabled,
        interval,
        keep,
    })
    .unwrap()
}

fn last(world: &Shared) -> BackupRunStatus {
    let value = world
        .lock()
        .unwrap()
        .settings
        .get(BACKUP_LAST_SETTING)
        .cloned()
        .expect("app.backup.last was written");
    serde_json::from_value(value).unwrap()
}

/// A scripted core whose backup is `KBB1` and a few bytes, with a schedule and a folder set.
fn ready(folder: &Path, keep: u32) -> (Arc<AppState<Fake>>, Shared) {
    let (state, world) = Fake::ready();
    {
        let mut world = world.lock().unwrap();
        world.backup_pieces = vec![b"KBB1".to_vec(), vec![1, 2, 3]];
    }
    set(
        &world,
        BACKUP_SCHEDULE_SETTING,
        schedule(true, BackupInterval::Daily, keep),
    );
    set(
        &world,
        BACKUP_FOLDER_SETTING,
        Json::String(folder.to_str().unwrap().to_owned()),
    );
    (state, world)
}

fn auto_name(ms: i64) -> String {
    backup_schedule::file_name(ms, 2 * 3600)
}

/// The folder alone: the targets of a shell without Drive.
fn no_drive() -> Targets {
    Targets::without_drive()
}

// --- run_now ---

#[test]
fn a_backup_is_written_named_by_the_core_old_ones_pruned_and_the_outcome_recorded() {
    let folder = tempfile::tempdir().unwrap();
    // Four older scheduled backups, a manual one, a note and a folder named like a backup.
    for days_ago in 1..=4 {
        fs::write(folder.path().join(auto_name(NOW - days_ago * DAY)), b"old").unwrap();
    }
    fs::write(
        folder.path().join("atomik-kb-2026-01-01.atomikkb"),
        b"manual",
    )
    .unwrap();
    fs::write(folder.path().join("notes.txt"), b"notes").unwrap();
    fs::create_dir(
        folder
            .path()
            .join("atomik-kb-auto-2020-01-01-0000.atomikkb"),
    )
    .unwrap();
    let (state, world) = ready(folder.path(), 2);
    let events = record_events(&state);
    let clock = TestClock::at(NOW);

    let result = run_now(host(&state), &clock, &no_drive()).unwrap();

    let name = auto_name(NOW);
    assert!(name.starts_with("atomik-kb-auto-2026-10-12-11"), "{name}");
    assert_eq!(result.path, folder.path().join(&name).to_string_lossy());
    assert_eq!(result.summary.bytes, 7);
    assert_eq!(
        fs::read(folder.path().join(&name)).unwrap(),
        [b"KBB1".as_slice(), &[1, 2, 3]].concat()
    );
    // The newest two scheduled backups stay (the new one and yesterday's); the others, the manual
    // backup, the note and the folder are not touched; nothing temporary is left.
    let mut expected = vec![
        auto_name(NOW - DAY),
        name.clone(),
        "atomik-kb-2026-01-01.atomikkb".to_owned(),
        "atomik-kb-auto-2020-01-01-0000.atomikkb".to_owned(),
        "notes.txt".to_owned(),
    ];
    expected.sort();
    assert_eq!(names_in(folder.path()), expected);

    let status = last(&world);
    assert!(status.ok);
    assert_eq!(status.last_success_at, Some(NOW));
    assert_eq!(status.at, NOW);
    assert_eq!(status.file.as_deref(), Some(name.as_str()));
    assert_eq!(status.error, None);
    assert!(events.lock().unwrap().iter().any(|event| matches!(
        event,
        Event::SettingChanged { key } if key == BACKUP_LAST_SETTING
    )));
    assert_eq!(world.lock().unwrap().backups, 1);
}

#[test]
fn a_failed_backup_is_recorded_with_its_error_and_keeps_the_last_success_and_the_old_files() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 1);
    let clock = TestClock::at(NOW);
    run_now(host(&state), &clock, &no_drive()).unwrap();
    let first = auto_name(NOW);

    clock.advance(DAY);
    world.lock().unwrap().backup_failure = Some(ApiError::storage("the disk is full"));
    let error = run_now(host(&state), &clock, &no_drive()).unwrap_err();

    assert_eq!(error.code, ErrorCode::Storage);
    let status = last(&world);
    assert!(!status.ok);
    assert_eq!(status.at, NOW + DAY);
    assert_eq!(status.last_success_at, Some(NOW));
    assert_eq!(status.file.as_deref(), Some(first.as_str()));
    let recorded = status.error.unwrap();
    assert!(recorded.starts_with("Storage: "), "{recorded}");
    // The failed file is gone, and the earlier backup was not pruned for it.
    assert_eq!(names_in(folder.path()), [first]);
}

#[test]
fn without_a_folder_or_with_a_missing_one_the_attempt_fails_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (state, world) = ready(dir.path(), 7);
    let clock = TestClock::at(NOW);

    world.lock().unwrap().settings.remove(BACKUP_FOLDER_SETTING);
    let error = run_now(host(&state), &clock, &no_drive()).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "folder");
    assert!(!last(&world).ok);

    set(
        &world,
        BACKUP_FOLDER_SETTING,
        Json::String(dir.path().join("unplugged").to_str().unwrap().to_owned()),
    );
    clock.advance(1000);
    let error = run_now(host(&state), &clock, &no_drive()).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    let status = last(&world);
    assert!(!status.ok);
    assert_eq!(status.at, NOW + 1000);
    assert!(status.error.unwrap().contains("not there"));
    // The folder was not created behind the person's back.
    assert!(!dir.path().join("unplugged").exists());
    assert_eq!(world.lock().unwrap().backups, 0);
}

#[test]
fn a_locked_core_is_answered_before_anything_is_touched() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 7);
    state.lock().unwrap();
    let error = run_now(host(&state), &TestClock::at(NOW), &no_drive()).unwrap_err();
    assert_eq!(error.code, ErrorCode::Locked);
    assert!(names_in(folder.path()).is_empty());
    assert!(
        !world
            .lock()
            .unwrap()
            .settings
            .contains_key(BACKUP_LAST_SETTING)
    );
}

#[test]
fn the_keep_setting_is_held_to_one_or_more() {
    let folder = tempfile::tempdir().unwrap();
    for days_ago in 1..=3 {
        fs::write(folder.path().join(auto_name(NOW - days_ago * DAY)), b"old").unwrap();
    }
    let (state, _world) = ready(folder.path(), 0);
    run_now(host(&state), &TestClock::at(NOW), &no_drive()).unwrap();
    // Zero would delete everything, the new backup included: at least one is kept.
    assert_eq!(names_in(folder.path()), [auto_name(NOW)]);
}

// --- Google Drive as a second target (ADR-0059) ---

/// What the fake Drive was given: the file as it was on disk at that moment.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Delivered {
    name: String,
    bytes: Vec<u8>,
    keep: u32,
    path: PathBuf,
}

/// A Drive that keeps what it is given and fails when told to.
struct FakeDrive {
    connected: AtomicBool,
    /// Answers `Ok(false)`: a Drive that lost its token between the look and the delivery.
    lost_token: AtomicBool,
    failure: Mutex<Option<ApiError>>,
    deliveries: Mutex<Vec<Delivered>>,
    /// The name of every delivery that was tried, the failed ones too.
    attempts: Mutex<Vec<String>>,
    /// Told when a delivery starts, and waited on before it ends, when a test wants to look in between.
    gate: Mutex<Option<(Sender<()>, Receiver<()>)>>,
}

impl FakeDrive {
    fn new(connected: bool) -> Arc<Self> {
        Arc::new(Self {
            connected: AtomicBool::new(connected),
            lost_token: AtomicBool::new(false),
            failure: Mutex::new(None),
            deliveries: Mutex::new(Vec::new()),
            attempts: Mutex::new(Vec::new()),
            gate: Mutex::new(None),
        })
    }

    fn fail_with(&self, error: Option<ApiError>) {
        *self.failure.lock().unwrap() = error;
    }

    fn delivered(&self) -> Vec<Delivered> {
        self.deliveries.lock().unwrap().clone()
    }

    fn attempts(&self) -> Vec<String> {
        self.attempts.lock().unwrap().clone()
    }
}

impl DriveTarget for FakeDrive {
    fn connected(&self, _: &dyn CoreHost) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn newest_delivered(&self, _: &dyn CoreHost) -> Option<String> {
        self.delivered()
            .into_iter()
            .map(|delivered| delivered.name)
            .max()
    }

    fn deliver(
        &self,
        _: &dyn CoreHost,
        file: &Path,
        name: &str,
        keep: u32,
    ) -> Result<bool, ApiError> {
        if self.lost_token.load(Ordering::SeqCst) {
            return Ok(false);
        }
        self.attempts.lock().unwrap().push(name.to_owned());
        // The file is read now: after the run it is gone.
        let bytes = fs::read(file).map_err(|error| ApiError::storage(error.to_string()))?;
        if let Some((entered, release)) = &*self.gate.lock().unwrap() {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        self.deliveries.lock().unwrap().push(Delivered {
            name: name.to_owned(),
            bytes,
            keep,
            path: file.to_path_buf(),
        });
        Ok(true)
    }
}

/// The targets of a shell with `drive`, whose data directory is `data_dir`.
fn with_drive(drive: &Arc<FakeDrive>, data_dir: &Path) -> Targets {
    let targets = Targets::new(Arc::clone(drive) as Arc<dyn DriveTarget>);
    targets.set_data_dir(data_dir);
    targets
}

/// A drive failure as the shell's Drive host reports one: a reason in the details.
fn quota() -> ApiError {
    ApiError::new(ErrorCode::Internal, "Google Drive has no room left")
        .with_details(json!({ "reason": "quota" }))
}

/// What the scripted core's backup holds.
fn backup_bytes() -> Vec<u8> {
    [b"KBB1".as_slice(), &[1, 2, 3]].concat()
}

/// Drive alone (no folder), the data directory `data`: a scripted core with the schedule on.
fn drive_alone(data: &Path, keep: u32) -> (Arc<AppState<Fake>>, Shared) {
    let (state, world) = ready(data, keep);
    world.lock().unwrap().settings.remove(BACKUP_FOLDER_SETTING);
    (state, world)
}

#[test]
fn drive_alone_gets_one_export_through_a_staging_file_deleted_afterwards() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    let (state, world) = drive_alone(data.path(), 5);
    let clock = TestClock::at(NOW);

    let result = run_now(host(&state), &clock, &with_drive(&drive, data.path())).unwrap();

    let name = auto_name(NOW);
    let staging = data.path().join("backup-staging");
    assert_eq!(result.path, staging.join(&name).to_string_lossy());
    assert_eq!(result.summary.bytes, 7);
    assert_eq!(
        drive.delivered(),
        [Delivered {
            name: name.clone(),
            bytes: backup_bytes(),
            keep: 5,
            path: staging.join(&name),
        }]
    );
    // The staging file is gone (and no temporary file either); the folder stays.
    assert!(staging.is_dir());
    assert!(names_in(&staging).is_empty());
    // The export happened once, and the run is recorded as a success.
    assert_eq!(world.lock().unwrap().backups, 1);
    let status = last(&world);
    assert!(status.ok);
    assert_eq!(status.last_success_at, Some(NOW));
    assert_eq!(status.file.as_deref(), Some(name.as_str()));
    assert_eq!(status.error, None);
}

#[test]
fn a_folder_and_drive_share_one_export_and_each_keeps_its_own_newest() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    for days_ago in 1..=2 {
        fs::write(folder.path().join(auto_name(NOW - days_ago * DAY)), b"old").unwrap();
    }
    let drive = FakeDrive::new(true);
    let (state, world) = ready(folder.path(), 2);

    let result = run_now(
        host(&state),
        &TestClock::at(NOW),
        &with_drive(&drive, data.path()),
    )
    .unwrap();

    let name = auto_name(NOW);
    // The folder's file is the answer, and the folder is pruned as before.
    assert_eq!(result.path, folder.path().join(&name).to_string_lossy());
    assert_eq!(fs::read(folder.path().join(&name)).unwrap(), backup_bytes());
    assert_eq!(
        names_in(folder.path()),
        [auto_name(NOW - DAY), name.clone()]
    );
    // Drive got the very same bytes from the folder's file, with the same `keep`; nothing was staged.
    assert_eq!(
        drive.delivered(),
        [Delivered {
            name,
            bytes: backup_bytes(),
            keep: 2,
            path: folder.path().join(auto_name(NOW)),
        }]
    );
    assert!(!data.path().join("backup-staging").exists());
    assert_eq!(world.lock().unwrap().backups, 1);
    assert!(last(&world).ok);
}

const MINUTE: i64 = 60_000;

#[test]
fn a_failed_delivery_fails_the_run_and_the_folder_keeps_its_copy() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, world) = ready(folder.path(), 7);
    let targets = with_drive(&drive, data.path());

    let error = run_now(host(&state), &TestClock::at(NOW), &targets).unwrap_err();

    assert_eq!(error, quota());
    let status = last(&world);
    assert!(!status.ok);
    assert_eq!(status.at, NOW);
    // Not every target succeeded: there is no success to date, though the folder has its copy,
    // and the status names it (the retry delivers that file to Drive again).
    assert_eq!(status.last_success_at, None);
    assert_eq!(status.file.as_deref(), Some(auto_name(NOW).as_str()));
    assert_eq!(status.error.as_deref(), Some(error.to_string().as_str()));
    assert_eq!(names_in(folder.path()), [auto_name(NOW)]);
    assert!(drive.delivered().is_empty());
}

#[test]
fn drive_failing_for_hours_never_costs_the_folder_its_older_backups() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    // Two older scheduled backups, and room for three.
    let older = [auto_name(NOW - 2 * DAY), auto_name(NOW - DAY)];
    for name in &older {
        fs::write(folder.path().join(name), b"older").unwrap();
    }
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, world) = ready(folder.path(), 3);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);
    let first = auto_name(NOW);
    let mut expected: Vec<String> = older.to_vec();
    expected.push(first.clone());

    assert_eq!(
        run_now(host(&state), &clock, &targets).unwrap_err(),
        quota()
    );
    assert_eq!(names_in(folder.path()), expected);

    // Eight hourly retries, all failing: nothing is exported, nothing is pruned.
    for _ in 0..8 {
        clock.advance(61 * MINUTE);
        let retry = run_if_due(host(&state), &clock, &targets).expect("a retry is due");
        assert_eq!(retry.unwrap_err(), quota());
        assert_eq!(names_in(folder.path()), expected);
        let status = last(&world);
        assert!(!status.ok);
        assert_eq!(status.at, clock.now_ms());
        assert_eq!(status.last_success_at, None);
        assert_eq!(status.file.as_deref(), Some(first.as_str()));
    }
    assert_eq!(
        world.lock().unwrap().backups,
        1,
        "one export, however long Drive is down"
    );
    assert_eq!(drive.attempts(), vec![first.clone(); 9]);
    assert!(drive.delivered().is_empty());
    // The files are the ones from before, byte for byte.
    assert_eq!(
        fs::read(folder.path().join(&first)).unwrap(),
        backup_bytes()
    );
    assert_eq!(fs::read(folder.path().join(&older[0])).unwrap(), b"older");
}

#[test]
fn the_retry_delivers_the_very_file_the_folder_got_and_the_schedule_keeps_its_rhythm() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, world) = ready(folder.path(), 3);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);
    let first = auto_name(NOW);
    run_now(host(&state), &clock, &targets).unwrap_err();

    // The workspace has changed since (a new export would differ), and Drive is back.
    world.lock().unwrap().backup_pieces = vec![b"KBB1".to_vec(), vec![9, 9, 9, 9]];
    drive.fail_with(None);
    clock.advance(61 * MINUTE);
    let result = run_if_due(host(&state), &clock, &targets)
        .expect("a retry is due")
        .unwrap();

    // The same name, the same bytes, from the folder's file: no new export.
    assert_eq!(
        drive.delivered(),
        [Delivered {
            name: first.clone(),
            bytes: backup_bytes(),
            keep: 3,
            path: folder.path().join(&first),
        }]
    );
    assert_eq!(world.lock().unwrap().backups, 1);
    assert_eq!(names_in(folder.path()), [first.as_str()]);
    // The answer is the folder's file.
    assert_eq!(result.path, folder.path().join(&first).to_string_lossy());
    assert_eq!(result.summary.bytes, 7);
    // The run is ok now, and dated by the backup it delivered, so the next one is due a day after it
    // (less the slack), not a day after the retry.
    let status = last(&world);
    assert!(status.ok);
    assert_eq!(status.at, NOW + 61 * MINUTE);
    assert_eq!(status.last_success_at, Some(NOW));
    assert_eq!(status.file.as_deref(), Some(first.as_str()));
    assert_eq!(status.error, None);
    assert!(run_if_due(host(&state), &clock, &targets).is_none());

    // The next period makes a normal export for both targets.
    clock.advance(DAY - 61 * MINUTE - 14 * MINUTE);
    assert!(run_if_due(host(&state), &clock, &targets).unwrap().is_ok());
    assert_eq!(world.lock().unwrap().backups, 2);
    let second = auto_name(clock.now_ms());
    assert_ne!(second, first);
    assert_eq!(
        drive.attempts(),
        [first.clone(), first.clone(), second.clone()]
    );
    let mut both = vec![first, second];
    both.sort();
    assert_eq!(names_in(folder.path()), both);
}

#[test]
fn a_new_period_makes_a_normal_export_even_while_drive_still_fails() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, world) = ready(folder.path(), 7);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);
    let first = auto_name(NOW);
    run_now(host(&state), &clock, &targets).unwrap_err();

    // Just short of the day (less the slack): the period is over.
    clock.advance(DAY - 14 * MINUTE);
    let retry = run_if_due(host(&state), &clock, &targets).expect("a run is due");
    assert_eq!(retry.unwrap_err(), quota());

    let second = auto_name(clock.now_ms());
    assert_eq!(world.lock().unwrap().backups, 2);
    let mut both = vec![first.clone(), second.clone()];
    both.sort();
    assert_eq!(names_in(folder.path()), both);
    assert_eq!(drive.attempts(), [first, second.clone()]);
    // And the failed run now names the new file.
    assert_eq!(last(&world).file.as_deref(), Some(second.as_str()));
}

#[test]
fn a_manual_backup_after_a_drive_failure_in_the_same_period_exports_a_new_file() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, world) = ready(folder.path(), 7);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);
    let first = auto_name(NOW);
    run_now(host(&state), &clock, &targets).unwrap_err();

    // A person pressing the button wants the current state, whatever the timer would retry.
    drive.fail_with(None);
    world.lock().unwrap().backup_pieces = vec![b"KBB1".to_vec(), vec![7, 7]];
    clock.advance(5 * MINUTE);
    let result = run_now(host(&state), &clock, &targets).unwrap();

    let second = auto_name(NOW + 5 * MINUTE);
    assert_ne!(first, second);
    assert_eq!(world.lock().unwrap().backups, 2);
    assert_eq!(result.path, folder.path().join(&second).to_string_lossy());
    let mut both = vec![first, second.clone()];
    both.sort();
    assert_eq!(names_in(folder.path()), both);
    // Delivered to Drive, new bytes under the new name; the run is ok and dated by this backup.
    assert_eq!(
        drive.delivered(),
        [Delivered {
            name: second.clone(),
            bytes: [b"KBB1".as_slice(), &[7, 7]].concat(),
            keep: 7,
            path: folder.path().join(&second),
        }]
    );
    let status = last(&world);
    assert!(status.ok);
    assert_eq!(status.last_success_at, Some(NOW + 5 * MINUTE));
    assert_eq!(status.file.as_deref(), Some(second.as_str()));
}

#[test]
fn with_drive_alone_a_retry_exports_again_into_staging() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, world) = drive_alone(data.path(), 7);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);

    run_now(host(&state), &clock, &targets).unwrap_err();
    clock.advance(61 * MINUTE);
    run_if_due(host(&state), &clock, &targets)
        .expect("a retry is due")
        .unwrap_err();

    // No folder to protect: each retry is a new export, delivered under its own name.
    assert_eq!(world.lock().unwrap().backups, 2);
    assert_eq!(
        drive.attempts(),
        [auto_name(NOW), auto_name(NOW + 61 * MINUTE)]
    );
    assert!(names_in(&data.path().join("backup-staging")).is_empty());
    // The failed run names no file: there is none to deliver again.
    assert_eq!(last(&world).file, None);
}

#[test]
fn a_file_that_is_gone_or_that_drive_already_has_is_not_delivered_again() {
    for drive_has_it in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let drive = FakeDrive::new(true);
        drive.fail_with(Some(quota()));
        let (state, world) = ready(folder.path(), 7);
        let targets = with_drive(&drive, data.path());
        let clock = TestClock::at(NOW);
        let first = auto_name(NOW);
        run_now(host(&state), &clock, &targets).unwrap_err();

        drive.fail_with(None);
        if drive_has_it {
            // Drive's own record says it holds this file (or a newer one).
            drive.deliveries.lock().unwrap().push(Delivered {
                name: first.clone(),
                bytes: backup_bytes(),
                keep: 7,
                path: folder.path().join(&first),
            });
        } else {
            fs::remove_file(folder.path().join(&first)).unwrap();
        }
        clock.advance(61 * MINUTE);
        assert!(run_if_due(host(&state), &clock, &targets).unwrap().is_ok());

        // A normal export for both targets.
        assert_eq!(
            world.lock().unwrap().backups,
            2,
            "drive has it: {drive_has_it}"
        );
        let second = auto_name(clock.now_ms());
        assert!(names_in(folder.path()).contains(&second));
        assert_eq!(drive.attempts().last(), Some(&second));
    }
}

#[test]
fn a_previous_success_is_never_delivered_again_by_a_retry_after_another_kind_of_failure() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    let (state, world) = ready(folder.path(), 7);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);
    run_now(host(&state), &clock, &targets).unwrap();

    // The export fails the next day: the failed run keeps the success's file, which Drive has.
    clock.advance(DAY);
    world.lock().unwrap().backup_failure = Some(ApiError::storage("the disk is full"));
    run_now(host(&state), &clock, &targets).unwrap_err();
    assert_eq!(last(&world).file.as_deref(), Some(auto_name(NOW).as_str()));

    world.lock().unwrap().backup_failure = None;
    clock.advance(61 * MINUTE);
    assert!(run_if_due(host(&state), &clock, &targets).unwrap().is_ok());

    // A new export, not the old file again.
    assert_eq!(world.lock().unwrap().backups, 3);
    assert_eq!(drive.delivered().len(), 2);
}

#[test]
fn a_folder_that_is_not_there_does_not_keep_drive_from_its_copy() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    let (state, world) = ready(&data.path().join("unplugged"), 7);

    let error = run_now(
        host(&state),
        &TestClock::at(NOW),
        &with_drive(&drive, data.path()),
    )
    .unwrap_err();

    // The folder's failure is the run's, and Drive has the backup all the same.
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "folder");
    assert_eq!(drive.delivered().len(), 1);
    assert!(names_in(&data.path().join("backup-staging")).is_empty());
    assert!(!data.path().join("unplugged").exists());
    let status = last(&world);
    assert!(!status.ok);
    assert!(status.error.unwrap().contains("not there"));
}

#[test]
fn when_both_targets_fail_the_folders_failure_comes_first() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, world) = ready(&data.path().join("unplugged"), 7);

    let error = run_now(
        host(&state),
        &TestClock::at(NOW),
        &with_drive(&drive, data.path()),
    )
    .unwrap_err();

    assert_eq!(error.details.unwrap()["field"], "folder");
    assert!(last(&world).error.unwrap().contains("not there"));
    assert!(names_in(&data.path().join("backup-staging")).is_empty());
}

#[test]
fn a_failed_export_is_not_delivered_and_leaves_no_staging_file() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    let (state, world) = drive_alone(data.path(), 7);
    world.lock().unwrap().backup_failure = Some(ApiError::storage("the disk is full"));

    let error = run_now(
        host(&state),
        &TestClock::at(NOW),
        &with_drive(&drive, data.path()),
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::Storage);
    assert!(drive.delivered().is_empty());
    assert!(names_in(&data.path().join("backup-staging")).is_empty());
    assert!(!last(&world).ok);
}

#[test]
fn a_failed_delivery_leaves_no_staging_file_either() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.fail_with(Some(quota()));
    let (state, _world) = drive_alone(data.path(), 7);

    let error = run_now(
        host(&state),
        &TestClock::at(NOW),
        &with_drive(&drive, data.path()),
    )
    .unwrap_err();

    assert_eq!(error, quota());
    assert!(names_in(&data.path().join("backup-staging")).is_empty());
}

#[test]
fn a_drive_that_lost_its_connection_is_a_failed_target_not_a_skipped_one() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    drive.lost_token.store(true, Ordering::SeqCst);
    let (state, world) = ready(folder.path(), 7);

    let error = run_now(
        host(&state),
        &TestClock::at(NOW),
        &with_drive(&drive, data.path()),
    )
    .unwrap_err();

    let details = error.details.unwrap();
    assert_eq!(details["reason"], "notConnected");
    assert_eq!(details["field"], "drive");
    assert!(!last(&world).ok);
    // The folder's copy is there all the same.
    assert_eq!(names_in(folder.path()), [auto_name(NOW)]);
}

#[test]
fn without_a_folder_or_a_connected_drive_nothing_is_exported() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(false);
    let (state, world) = drive_alone(data.path(), 7);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);

    let error = run_now(host(&state), &clock, &targets).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "folder");
    assert!(!last(&world).ok);
    assert_eq!(world.lock().unwrap().backups, 0);
    assert!(drive.delivered().is_empty());
    assert!(!data.path().join("backup-staging").exists());

    // The timer does not even try, and records nothing.
    world.lock().unwrap().settings.remove(BACKUP_LAST_SETTING);
    assert!(run_if_due(host(&state), &clock, &targets).is_none());
    assert!(
        !world
            .lock()
            .unwrap()
            .settings
            .contains_key(BACKUP_LAST_SETTING)
    );
}

#[test]
fn a_schedule_with_drive_alone_runs_when_due() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    let (state, world) = drive_alone(data.path(), 7);
    let targets = with_drive(&drive, data.path());
    let clock = TestClock::at(NOW);

    assert!(run_if_due(host(&state), &clock, &targets).unwrap().is_ok());
    clock.advance(HOUR);
    assert!(run_if_due(host(&state), &clock, &targets).is_none());
    clock.advance(DAY - HOUR);
    assert!(run_if_due(host(&state), &clock, &targets).unwrap().is_ok());

    assert_eq!(world.lock().unwrap().backups, 2);
    assert_eq!(drive.delivered().len(), 2);
}

#[test]
fn drive_alone_needs_to_know_where_to_stage_before_anything_is_exported() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    let (state, world) = drive_alone(data.path(), 7);
    // No data directory told.
    let targets = Targets::new(Arc::clone(&drive) as Arc<dyn DriveTarget>);

    let error = run_now(host(&state), &TestClock::at(NOW), &targets).unwrap_err();

    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(world.lock().unwrap().backups, 0);
    assert!(drive.delivered().is_empty());
}

// --- the staging folders ---

/// Leftovers of an earlier session in both staging folders, and a neighbour that is not one.
fn leave_staging_files(data: &Path) {
    for (dir, file) in [
        ("backup-staging", "atomik-kb-auto-2026-10-12-0930.atomikkb"),
        (
            "backup-staging",
            "atomik-kb-auto-2026-10-12-0930.atomikkb.tmp",
        ),
        ("restore-staging", "download.atomikkb"),
    ] {
        fs::create_dir_all(data.join(dir)).unwrap();
        fs::write(data.join(dir).join(file), b"left behind").unwrap();
    }
    fs::write(data.join("workspace.db"), b"not staging").unwrap();
}

#[test]
fn the_staging_folders_are_emptied_when_the_core_becomes_ready_and_after_an_unlock() {
    let data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let (state, _world) = ready(folder.path(), 7);
    leave_staging_files(data.path());
    let targets = Arc::new(with_drive(&FakeDrive::new(false), data.path()));
    let weak = Arc::downgrade(&state);
    let host_ref: Weak<dyn CoreHost> = weak;

    // The core is ready already: registering the background work starts it, and that empties them.
    state.set_background(backup_scheduler::scheduler_starter(
        host_ref,
        Arc::new(TestClock::at(NOW)),
        targets,
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    ));

    assert!(names_in(&data.path().join("backup-staging")).is_empty());
    assert!(names_in(&data.path().join("restore-staging")).is_empty());
    assert!(data.path().join("backup-staging").is_dir());
    assert_eq!(
        fs::read(data.path().join("workspace.db")).unwrap(),
        b"not staging"
    );

    // Locked and unlocked: the core becomes ready again, and the folders are emptied again.
    state.lock().unwrap();
    leave_staging_files(data.path());
    state.unlock("any").unwrap();
    assert!(names_in(&data.path().join("backup-staging")).is_empty());
    assert!(names_in(&data.path().join("restore-staging")).is_empty());
}

#[test]
fn a_run_that_outlived_a_lock_keeps_its_staging_file_when_the_core_is_ready_again() {
    let data = tempfile::tempdir().unwrap();
    let drive = FakeDrive::new(true);
    let (state, _world) = drive_alone(data.path(), 7);
    let targets = with_drive(&drive, data.path());
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *drive.gate.lock().unwrap() = Some((entered_tx, release_rx));
    let clock = TestClock::at(NOW);

    std::thread::scope(|scope| {
        let run = scope.spawn(|| run_now(host(&state), &clock, &targets));
        // The delivery is under way: the staging file is on disk, and a session starting now (the
        // core ready again) must not take it away; the restore folder it empties all the same.
        entered_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        fs::create_dir_all(data.path().join("restore-staging")).unwrap();
        fs::write(data.path().join("restore-staging").join("x.atomikkb"), b"x").unwrap();
        targets.clear_staging();
        assert_eq!(
            names_in(&data.path().join("backup-staging")),
            [auto_name(NOW)]
        );
        assert!(names_in(&data.path().join("restore-staging")).is_empty());
        release_tx.send(()).unwrap();
        assert!(run.join().unwrap().is_ok());
    });

    // The run deleted its own file.
    assert!(names_in(&data.path().join("backup-staging")).is_empty());
    assert_eq!(drive.delivered().len(), 1);
}

// --- the timer's look ---

#[test]
fn a_backup_is_taken_when_the_core_says_one_is_due_and_not_before() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 7);
    let clock = TestClock::at(NOW);

    // Nothing yet: due at once.
    assert!(
        run_if_due(host(&state), &clock, &no_drive())
            .unwrap()
            .is_ok()
    );
    assert_eq!(world.lock().unwrap().backups, 1);
    // An hour later it is not.
    clock.advance(HOUR);
    assert!(run_if_due(host(&state), &clock, &no_drive()).is_none());
    // A day later (less the slack) it is again.
    clock.advance(DAY - HOUR);
    assert!(
        run_if_due(host(&state), &clock, &no_drive())
            .unwrap()
            .is_ok()
    );
    assert_eq!(world.lock().unwrap().backups, 2);
}

#[test]
fn a_failed_attempt_is_not_retried_for_an_hour() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 7);
    world.lock().unwrap().backup_failure = Some(ApiError::storage("the disk is full"));
    let clock = TestClock::at(NOW);

    assert!(
        run_if_due(host(&state), &clock, &no_drive())
            .unwrap()
            .is_err()
    );
    clock.advance(30 * 60_000);
    assert!(run_if_due(host(&state), &clock, &no_drive()).is_none());
    clock.advance(31 * 60_000);
    world.lock().unwrap().backup_failure = None;
    assert!(
        run_if_due(host(&state), &clock, &no_drive())
            .unwrap()
            .is_ok()
    );
}

#[test]
fn a_disabled_schedule_or_a_missing_folder_means_no_attempt_and_no_record() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 7);
    let clock = TestClock::at(NOW);

    set(
        &world,
        BACKUP_SCHEDULE_SETTING,
        schedule(false, BackupInterval::Daily, 7),
    );
    assert!(run_if_due(host(&state), &clock, &no_drive()).is_none());

    set(
        &world,
        BACKUP_SCHEDULE_SETTING,
        schedule(true, BackupInterval::Weekly, 7),
    );
    world.lock().unwrap().settings.remove(BACKUP_FOLDER_SETTING);
    assert!(run_if_due(host(&state), &clock, &no_drive()).is_none());
    assert!(
        !world
            .lock()
            .unwrap()
            .settings
            .contains_key(BACKUP_LAST_SETTING)
    );
    assert_eq!(world.lock().unwrap().backups, 0);
}

#[test]
fn a_weekly_schedule_waits_a_week() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 7);
    set(
        &world,
        BACKUP_SCHEDULE_SETTING,
        schedule(true, BackupInterval::Weekly, 7),
    );
    let clock = TestClock::at(NOW);
    assert!(
        run_if_due(host(&state), &clock, &no_drive())
            .unwrap()
            .is_ok()
    );
    clock.advance(3 * DAY);
    assert!(run_if_due(host(&state), &clock, &no_drive()).is_none());
    clock.advance(4 * DAY);
    assert!(
        run_if_due(host(&state), &clock, &no_drive())
            .unwrap()
            .is_ok()
    );
}

// --- the timer ---

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_timer_runs_while_the_core_is_ready_and_stops_when_it_locks() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 7);
    let clock = Arc::new(TestClock::at(NOW));
    let weak = Arc::downgrade(&state);
    let host_ref: Weak<dyn CoreHost> = weak;
    state.set_background(backup_scheduler::scheduler_starter(
        host_ref,
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::new(no_drive()),
        Duration::from_millis(20),
        Duration::from_millis(20),
    ));

    // Started with the ready core; the first look finds a backup due and writes it.
    assert!(state.background_running());
    wait_for("the first backup", || world.lock().unwrap().backups == 1);
    // It is not due again (an hour later); the timer keeps looking and leaves it alone.
    clock.advance(HOUR);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(world.lock().unwrap().backups, 1);
    // A day on, the next look writes the next one.
    clock.advance(DAY);
    wait_for("the second backup", || world.lock().unwrap().backups == 2);

    // Locking stops the thread with the rest of the background work; unlocking starts it again.
    state.lock().unwrap();
    assert!(!state.background_running());
    state.unlock("any").unwrap();
    assert!(state.background_running());
    clock.advance(2 * DAY);
    wait_for("a backup after the unlock", || {
        world.lock().unwrap().backups == 3
    });
}

#[test]
fn the_timer_does_nothing_before_its_first_delay() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = ready(folder.path(), 7);
    let weak = Arc::downgrade(&state);
    let host_ref: Weak<dyn CoreHost> = weak;
    state.set_background(backup_scheduler::scheduler_starter(
        host_ref,
        Arc::new(TestClock::at(NOW)),
        Arc::new(no_drive()),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    ));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(world.lock().unwrap().backups, 0);
    // Dropping the state's background work ends the thread without waiting for the delay.
    state.lock().unwrap();
    assert!(!state.background_running());
}

#[test]
fn the_timers_defaults_are_two_minutes_then_every_ten() {
    assert_eq!(backup_scheduler::FIRST_RUN_DELAY, Duration::from_secs(120));
    assert_eq!(backup_scheduler::CHECK_INTERVAL, Duration::from_secs(600));
}

// --- the folder setting ---

#[test]
fn the_chosen_folder_is_stored_in_the_shell_owned_setting() {
    let folder = tempfile::tempdir().unwrap();
    let (state, world) = Fake::ready();
    let path =
        backup_scheduler::choose_folder(host(&state), || Ok(folder.path().to_path_buf())).unwrap();
    assert_eq!(path, folder.path().to_str().unwrap());
    assert_eq!(
        world.lock().unwrap().settings.get(BACKUP_FOLDER_SETTING),
        Some(&json!(folder.path().to_str().unwrap()))
    );
}

#[test]
fn a_cancelled_or_unusable_folder_choice_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a-file.txt");
    fs::write(&file, b"x").unwrap();
    let (state, world) = Fake::ready();

    let cancelled = backup_scheduler::choose_folder(host(&state), || {
        Err(ApiError::invalid("path", "no file was chosen"))
    })
    .unwrap_err();
    assert_eq!(cancelled, ApiError::invalid("path", "no file was chosen"));

    let not_a_folder =
        backup_scheduler::choose_folder(host(&state), || Ok(file.clone())).unwrap_err();
    assert_eq!(not_a_folder.code, ErrorCode::InvalidInput);

    assert!(world.lock().unwrap().settings.is_empty());
}

// --- save_text_file ---

#[test]
fn a_text_is_saved_exactly_and_replaces_what_was_there() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("Tasks.csv");
    fs::write(&target, "old").unwrap();
    let text = "\u{FEFF}Name,Status\r\n\"Buy, milk\",Done\r\n";

    let path = text_file::save("Tasks.csv", text, |name| {
        assert_eq!(name, "Tasks.csv");
        Ok(target.clone())
    })
    .unwrap();

    assert_eq!(path, target.to_string_lossy());
    assert_eq!(fs::read(&target).unwrap(), text.as_bytes());
    assert_eq!(
        names_in(dir.path()),
        ["Tasks.csv"],
        "no temporary file is left"
    );
}

#[test]
fn a_text_that_cannot_be_saved_leaves_the_old_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    // A path inside a folder that is not there.
    let target = dir.path().join("missing").join("Tasks.csv");
    let error = text_file::save("Tasks.csv", "x", |_| Ok(target.clone())).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(error.details.unwrap()["field"], "path");

    // A directory is not a file.
    let error = text_file::save("Tasks.csv", "x", |_| Ok(dir.path().to_path_buf())).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
}

#[test]
fn a_text_of_exactly_the_limit_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("big.txt");
    let text = "a".repeat(text_file::MAX_TEXT_BYTES);
    text_file::save("big.txt", &text, |_| Ok(target.clone())).unwrap();
    assert_eq!(
        fs::metadata(&target).unwrap().len(),
        text_file::MAX_TEXT_BYTES as u64
    );
}
