//! Scheduled backups to a folder (ADR-0036, `docs/architecture/import-export.md`, "Scheduled
//! backups"): when one is due, how its file is named and which old ones to delete. Pure
//! functions; the desktop shell runs the timer, writes the file and deletes the old ones.

use kb_types::interop::BACKUP_AUTO_PREFIX;
use kb_types::{BackupInterval, BackupRunStatus, BackupSchedule};

/// Extension of a backup file (`BACKUP_MAGIC` files, see `backup.rs`).
const BACKUP_EXTENSION: &str = ".atomikkb";

const MS_PER_MINUTE: i64 = 60_000;
const MS_PER_HOUR: i64 = 60 * MS_PER_MINUTE;
const MS_PER_DAY: i64 = 24 * MS_PER_HOUR;

/// A backup is due a little before a full period has passed, so that a timer that ticks every ten
/// minutes does not push the next backup a whole tick (and so a day) late.
const SLACK_MS: i64 = 15 * MS_PER_MINUTE;

/// After a failed attempt nothing is tried again for this long.
const RETRY_AFTER_FAILURE_MS: i64 = MS_PER_HOUR;

/// A last success this far in the future means the device clock was set back: it says nothing
/// about the age of the backup, so a new one is due.
const CLOCK_SKEW_MS: i64 = MS_PER_DAY;

/// Whether a scheduled backup is due at `now_ms`: the schedule is enabled, and either no backup
/// succeeded yet or the last success is at least one period (24 h or 7 days, less 15 minutes of
/// slack) old, and no attempt failed during the last hour.
#[must_use]
pub fn is_due(now_ms: i64, schedule: &BackupSchedule, last: Option<&BackupRunStatus>) -> bool {
    if !schedule.enabled {
        return false;
    }
    let Some(last) = last else {
        return true;
    };
    if !last.ok {
        let since = now_ms.saturating_sub(last.at);
        if (0..RETRY_AFTER_FAILURE_MS).contains(&since) {
            return false;
        }
    }
    let success = last.last_success_at.or_else(|| last.ok.then_some(last.at));
    let Some(success) = success else {
        return true;
    };
    let period = match schedule.interval {
        BackupInterval::Daily => MS_PER_DAY,
        BackupInterval::Weekly => 7 * MS_PER_DAY,
    };
    let age = now_ms.saturating_sub(success);
    age >= period - SLACK_MS || age < -CLOCK_SKEW_MS
}

/// The file name of a scheduled backup written at `now_ms` on a device whose UTC offset is
/// `local_offset_seconds`: `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb` in local time.
#[must_use]
pub fn file_name(now_ms: i64, local_offset_seconds: i32) -> String {
    let local_seconds = now_ms.div_euclid(1000) + i64::from(local_offset_seconds);
    let days = local_seconds.div_euclid(86_400);
    let of_day = local_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = of_day / 3_600;
    let minute = of_day % 3_600 / 60;
    format!(
        "{BACKUP_AUTO_PREFIX}{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}{BACKUP_EXTENSION}"
    )
}

/// Among the file names of a folder, the scheduled backups to delete so that the newest `keep`
/// remain, oldest first. Only names of the exact scheduled form count (a name with another
/// prefix, another extension, a different layout or an impossible date or time is never listed).
/// A `keep` of 0 is treated as 1: the pruning never leaves the folder without a scheduled backup.
#[must_use]
pub fn to_prune(file_names: &[String], keep: u32) -> Vec<String> {
    let mut ours: Vec<&String> = file_names
        .iter()
        .filter(|name| is_scheduled_name(name))
        .collect();
    // Zero-padded fields, so the text order is the time order.
    ours.sort_unstable();
    ours.dedup();
    let keep = usize::try_from(keep.max(1)).unwrap_or(usize::MAX);
    let surplus = ours.len().saturating_sub(keep);
    ours.into_iter().take(surplus).cloned().collect()
}

/// `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb` with real calendar numbers.
fn is_scheduled_name(name: &str) -> bool {
    name_fields(name).is_some()
}

/// The `(year, month, day, hour, minute)` a scheduled backup's name says, when it is one.
fn name_fields(name: &str) -> Option<(u32, u32, u32, u32, u32)> {
    let rest = name.strip_prefix(BACKUP_AUTO_PREFIX)?;
    let stamp = rest.strip_suffix(BACKUP_EXTENSION)?;
    let bytes = stamp.as_bytes();
    // YYYY-MM-DD-HHMM
    if bytes.len() != 15 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'-' {
        return None;
    }
    let digits = |from: usize, to: usize| -> Option<u32> {
        let part = bytes.get(from..to)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        part.iter()
            .try_fold(0u32, |acc, d| Some(acc * 10 + u32::from(d - b'0')))
    };
    let (year, month, day, hour, minute) = (
        digits(0, 4)?,
        digits(5, 7)?,
        digits(8, 10)?,
        digits(11, 13)?,
        digits(13, 15)?,
    );
    ((1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60)
        .then_some((year, month, day, hour, minute))
}

/// When (Unix ms, to the minute) the scheduled backup named `name` was made on a device whose UTC offset is
/// `local_offset_seconds` (the inverse of [`file_name`]); `None` for a name that is not a scheduled backup's.
#[must_use]
pub fn name_time_ms(name: &str, local_offset_seconds: i32) -> Option<i64> {
    let (year, month, day, hour, minute) = name_fields(name)?;
    let days = days_from_civil(i64::from(year), i64::from(month), i64::from(day));
    let local_seconds = days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60;
    Some((local_seconds - i64::from(local_offset_seconds)) * 1000)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) {
                29
            } else {
                28
            }
        }
    }
}

/// The civil date of a day count since 1970-01-01 (proleptic Gregorian; the well-known
/// days-to-civil algorithm: the year starts in March so the leap day is the last day of a year).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The day count since 1970-01-01 of a civil date (the inverse of `civil_from_days`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn schedule(interval: BackupInterval) -> BackupSchedule {
        BackupSchedule {
            enabled: true,
            interval,
            keep: 7,
        }
    }

    fn success(at: i64) -> BackupRunStatus {
        BackupRunStatus {
            at,
            ok: true,
            last_success_at: Some(at),
            file: Some("atomik-kb-auto-2026-10-01-0900.atomikkb".into()),
            error: None,
        }
    }

    fn failure(at: i64, last_success_at: Option<i64>) -> BackupRunStatus {
        BackupRunStatus {
            at,
            ok: false,
            last_success_at,
            file: None,
            error: Some("Storage: the folder is missing".into()),
        }
    }

    #[test]
    fn a_name_says_when_the_backup_was_made_the_inverse_of_file_name() {
        for offset in [0, 3600, -5 * 3600, 5 * 3600 + 1800, -(9 * 3600 + 1800)] {
            for ms in [
                0_i64,
                951_782_400_000,   // 2000-02-29 00:00 UTC
                1_700_000_000_000, // 2023-11-14
                1_791_797_400_000, // 2026-10-12 09:30 UTC
                4_102_444_800_000, // 2100-01-01
            ] {
                let name = file_name(ms, offset);
                // To the minute: the name has no seconds.
                assert_eq!(
                    name_time_ms(&name, offset),
                    Some(ms - ms.rem_euclid(60_000)),
                    "{name} at {offset}"
                );
            }
        }
        for name in [
            "",
            "notes.md",
            "atomik-kb-2026-10-12.atomikkb",
            "atomik-kb-auto-2026-02-30-0900.atomikkb",
            "atomik-kb-auto-2026-10-12-2460.atomikkb",
            "atomik-kb-auto-2026-10-12-0900.zip",
        ] {
            assert_eq!(name_time_ms(name, 0), None, "{name:?}");
        }
    }

    #[test]
    fn disabled_is_never_due() {
        let off = BackupSchedule {
            enabled: false,
            ..schedule(BackupInterval::Daily)
        };
        assert!(!is_due(1_000_000_000_000, &off, None));
        assert!(!is_due(
            1_000_000_000_000,
            &off,
            Some(&success(1_000_000_000_000 - 10 * MS_PER_DAY))
        ));
    }

    #[test]
    fn due_without_a_previous_success() {
        let daily = schedule(BackupInterval::Daily);
        assert!(is_due(5, &daily, None));
        // A status that records neither a success nor an hour-old failure.
        let never = BackupRunStatus {
            at: 0,
            ok: false,
            last_success_at: None,
            file: None,
            error: Some("Storage".into()),
        };
        assert!(is_due(2 * MS_PER_HOUR, &daily, Some(&never)));
    }

    #[test]
    fn daily_is_due_a_day_less_the_slack_after_the_last_success() {
        let daily = schedule(BackupInterval::Daily);
        let t0 = 1_800_000_000_000;
        let last = success(t0);
        assert!(!is_due(t0, &daily, Some(&last)));
        assert!(!is_due(t0 + MS_PER_DAY - SLACK_MS - 1, &daily, Some(&last)));
        assert!(is_due(t0 + MS_PER_DAY - SLACK_MS, &daily, Some(&last)));
        assert!(is_due(t0 + 3 * MS_PER_DAY, &daily, Some(&last)));
    }

    #[test]
    fn weekly_is_due_a_week_less_the_slack_after_the_last_success() {
        let weekly = schedule(BackupInterval::Weekly);
        let t0 = 1_800_000_000_000;
        let last = success(t0);
        assert!(!is_due(t0 + 6 * MS_PER_DAY, &weekly, Some(&last)));
        assert!(!is_due(
            t0 + 7 * MS_PER_DAY - SLACK_MS - 1,
            &weekly,
            Some(&last)
        ));
        assert!(is_due(t0 + 7 * MS_PER_DAY - SLACK_MS, &weekly, Some(&last)));
    }

    #[test]
    fn a_failure_holds_the_next_attempt_back_for_an_hour() {
        let daily = schedule(BackupInterval::Daily);
        let t0 = 1_800_000_000_000;
        let old_success = t0 - 2 * MS_PER_DAY;
        let failed = failure(t0, Some(old_success));
        // The last success is two days old, yet a failure a minute ago is not retried at once.
        assert!(!is_due(t0 + MS_PER_MINUTE, &daily, Some(&failed)));
        assert!(!is_due(t0 + MS_PER_HOUR - 1, &daily, Some(&failed)));
        assert!(is_due(t0 + MS_PER_HOUR, &daily, Some(&failed)));
        // A failure after a recent success changes nothing: the success is what counts.
        let recent = failure(t0, Some(t0 - MS_PER_HOUR));
        assert!(!is_due(t0 + 2 * MS_PER_HOUR, &daily, Some(&recent)));
    }

    #[test]
    fn a_last_success_far_in_the_future_means_the_clock_moved_back() {
        let daily = schedule(BackupInterval::Daily);
        let now = 1_800_000_000_000;
        // A few minutes ahead is ordinary clock drift: not due.
        assert!(!is_due(
            now,
            &daily,
            Some(&success(now + 5 * MS_PER_MINUTE))
        ));
        // Days ahead cannot be trusted: due.
        assert!(is_due(now, &daily, Some(&success(now + 30 * MS_PER_DAY))));
    }

    #[test]
    fn file_names_are_in_local_time() {
        // 2026-10-12T20:07:41Z is 1_791_835_661 s.
        let at = 1_791_835_661_000;
        assert_eq!(file_name(at, 0), "atomik-kb-auto-2026-10-12-2007.atomikkb");
        // Mountain Daylight Time, UTC-6, still on the same day.
        assert_eq!(
            file_name(at, -6 * 3600),
            "atomik-kb-auto-2026-10-12-1407.atomikkb"
        );
        // UTC+5:30 crosses midnight into the next day.
        assert_eq!(
            file_name(at, 5 * 3600 + 1800),
            "atomik-kb-auto-2026-10-13-0137.atomikkb"
        );
        // The epoch, and a leap day.
        assert_eq!(file_name(0, 0), "atomik-kb-auto-1970-01-01-0000.atomikkb");
        assert_eq!(
            file_name(951_782_400_000 + 23 * 3_600_000 + 59 * 60_000, 0),
            "atomik-kb-auto-2000-02-29-2359.atomikkb"
        );
        // Before 1970 the day arithmetic floors, it does not truncate.
        assert_eq!(
            file_name(-60_000, 0),
            "atomik-kb-auto-1969-12-31-2359.atomikkb"
        );
    }

    #[test]
    fn file_names_are_recognised_as_scheduled() {
        let name = file_name(1_791_835_661_000, -6 * 3600);
        assert!(is_scheduled_name(&name));
        assert_eq!(to_prune(&[name], 1), Vec::<String>::new());
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn pruning_lists_the_oldest_beyond_the_newest_keep() {
        let folder = names(&[
            "atomik-kb-auto-2026-10-03-0900.atomikkb",
            "atomik-kb-auto-2026-10-01-0900.atomikkb",
            "atomik-kb-auto-2026-10-05-0900.atomikkb",
            "atomik-kb-auto-2026-10-02-0900.atomikkb",
            "atomik-kb-auto-2026-10-04-0900.atomikkb",
        ]);
        assert_eq!(
            to_prune(&folder, 3),
            names(&[
                "atomik-kb-auto-2026-10-01-0900.atomikkb",
                "atomik-kb-auto-2026-10-02-0900.atomikkb",
            ])
        );
        assert!(to_prune(&folder, 5).is_empty());
        assert!(to_prune(&folder, 100).is_empty());
        assert_eq!(to_prune(&folder, 1).len(), 4);
    }

    #[test]
    fn pruning_never_lists_other_files() {
        let folder = names(&[
            "atomik-kb-auto-2026-10-01-0900.atomikkb",
            "atomik-kb-auto-2026-10-02-0900.atomikkb",
            // The manual backups, other tools' files and near misses stay.
            "my-backup.atomikkb",
            "atomik-kb-2026-10-01.atomikkb",
            "atomik-kb-auto-2026-10-01-0900.atomikkb.bak",
            "atomik-kb-auto-2026-10-01-0900.zip",
            "Atomik-kb-auto-2026-10-01-0900.atomikkb",
            "atomik-kb-auto-2026-1-01-0900.atomikkb",
            "atomik-kb-auto-2026-10-01-09000.atomikkb",
            "atomik-kb-auto-2026-10-01-0900 (1).atomikkb",
            "atomik-kb-auto-2026-13-01-0900.atomikkb",
            "atomik-kb-auto-2026-02-30-0900.atomikkb",
            "atomik-kb-auto-2026-10-01-2400.atomikkb",
            "atomik-kb-auto-2026-10-01-0960.atomikkb",
            "atomik-kb-auto-2026-10-01-09aa.atomikkb",
            "atomik-kb-auto-+026-10-01-0900.atomikkb",
            "notes.txt",
            "",
        ]);
        assert_eq!(
            to_prune(&folder, 1),
            names(&["atomik-kb-auto-2026-10-01-0900.atomikkb"])
        );
        assert!(to_prune(&folder, 2).is_empty());
    }

    #[test]
    fn pruning_keeps_at_least_one_and_ignores_duplicates() {
        let folder = names(&[
            "atomik-kb-auto-2026-10-01-0900.atomikkb",
            "atomik-kb-auto-2026-10-01-0900.atomikkb",
        ]);
        assert!(to_prune(&folder, 0).is_empty());
        assert!(to_prune(&[], 7).is_empty());
        let two = names(&[
            "atomik-kb-auto-2026-10-01-0900.atomikkb",
            "atomik-kb-auto-2026-10-02-0900.atomikkb",
        ]);
        assert_eq!(
            to_prune(&two, 0),
            names(&["atomik-kb-auto-2026-10-01-0900.atomikkb"])
        );
    }

    #[test]
    fn leap_days_are_real() {
        assert!(is_scheduled_name("atomik-kb-auto-2024-02-29-0000.atomikkb"));
        assert!(!is_scheduled_name(
            "atomik-kb-auto-2026-02-29-0000.atomikkb"
        ));
        assert!(!is_scheduled_name(
            "atomik-kb-auto-2100-02-29-0000.atomikkb"
        ));
        assert!(is_scheduled_name("atomik-kb-auto-2000-02-29-0000.atomikkb"));
    }
}
