//! The text of a date mention (`dateMention { id, date, time? }`, ADR-0016, amendment of
//! 2026-10-07).
//!
//! `date` is a floating local date, `YYYY-MM-DD`: a calendar day with no time zone, so it
//! reads the same on every device and never shifts when someone travels. `time` is `HH:mm`
//! (24 hours), also floating, and optional. Both are checked strictly here: a mention whose
//! `date` is not a real day of the proleptic Gregorian calendar is malformed
//! (`check_invariants` reports it) and reads as nothing; a malformed `time` is left out. The
//! TypeScript mirror (`core-client`, `isIsoDate`, `isIsoTime`, `dateMentionText`) applies the
//! same rules.

/// Whether `text` is exactly `YYYY-MM-DD` and a real day: a year from `0001` to `9999`, a
/// month from `01` to `12`, a day that exists in that month (leap years of the Gregorian
/// calendar: divisible by 4, except centuries that are not divisible by 400).
pub(crate) fn valid_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = bytes.get(range)?;
        if part.iter().all(u8::is_ascii_digit) {
            part.iter().try_fold(0_u32, |acc, digit| {
                acc.checked_mul(10)?.checked_add(u32::from(digit - b'0'))
            })
        } else {
            None
        }
    };
    let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

/// Whether `text` is exactly `HH:mm`: an hour from `00` to `23`, a minute from `00` to `59`.
pub(crate) fn valid_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return false;
    }
    let digits = |pair: &[u8]| -> Option<u8> {
        match pair {
            [tens, ones] if tens.is_ascii_digit() && ones.is_ascii_digit() => {
                Some((tens - b'0') * 10 + (ones - b'0'))
            }
            _ => None,
        }
    };
    matches!(
        (bytes.get(0..2).and_then(digits), bytes.get(3..5).and_then(digits)),
        (Some(hour), Some(minute)) if hour <= 23 && minute <= 59
    )
}

/// The ISO text a mention reads as: `2026-10-07`, or `2026-10-07 14:30` with a time. `None`
/// when `date` is missing or not a valid date (the mention then reads as nothing); a missing
/// or malformed `time` is left out.
pub(super) fn iso_text(date: Option<&str>, time: Option<&str>) -> Option<String> {
    let date = date.filter(|date| valid_date(date))?;
    match time.filter(|time| valid_time(time)) {
        Some(time) => Some(format!("{date} {time}")),
        None => Some(date.to_owned()),
    }
}

/// The time of day, in minutes since midnight, of a mention of a whole day (no `time`):
/// 09:00. A reminder counts back from it.
const WHOLE_DAY_MOMENT_MINUTES: i64 = 9 * 60;

/// Days from 1970-01-01 to the given day of the proleptic Gregorian calendar (negative
/// before it). `month` is 1 to 12 and `day` exists in that month (the callers check). The
/// well-known civil-from-days algorithm: the year starts in March, so the leap day is the
/// last day of the year and every other month has a fixed offset.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_from_march = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// When a reminder fires, on the device's wall clock: the mention's moment (`date` at `time`,
/// or at 09:00 when there is no valid `time`) minus `reminder_minutes`, in seconds since
/// 1970-01-01T00:00:00 as that clock reads it (the calendar fields counted as if they were
/// UTC, so no time zone data is needed: a mention is a floating local date, ADR-0016, and a
/// reminder is due when the local clock reaches its moment, ADR-0024). `None` unless `date`
/// is a valid date, or when the arithmetic overflows.
pub(super) fn fire_at_local(date: &str, time: Option<&str>, reminder_minutes: i64) -> Option<i64> {
    if !valid_date(date) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> { date.get(range)?.parse().ok() };
    let days = days_from_civil(number(0..4)?, number(5..7)?, number(8..10)?);
    let minutes = match time.filter(|time| valid_time(time)) {
        Some(time) => {
            let part =
                |range: std::ops::Range<usize>| -> Option<i64> { time.get(range)?.parse().ok() };
            part(0..2)? * 60 + part(3..5)?
        }
        None => WHOLE_DAY_MOMENT_MINUTES,
    };
    let moment = days.checked_mul(86_400)?.checked_add(minutes * 60)?;
    moment.checked_sub(reminder_minutes.checked_mul(60)?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn days_count_from_the_epoch_across_leap_years_and_centuries() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 10, 7), 20_733);
        // 1900 is not a leap year, 2000 is, 2100 is not.
        assert_eq!(
            days_from_civil(1900, 3, 1) - days_from_civil(1900, 2, 28),
            1
        );
        assert_eq!(
            days_from_civil(2100, 3, 1) - days_from_civil(2100, 2, 28),
            1
        );
        assert_eq!(
            days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 28),
            2
        );
        assert_eq!(days_from_civil(1, 1, 1), -719_162);
        assert_eq!(days_from_civil(9999, 12, 31), 2_932_896);
    }

    /// 2026-10-07 14:30 on the wall clock, as seconds since 1970-01-01T00:00:00.
    const OCT_7_1430: i64 = 1_791_383_400;

    #[test]
    fn the_fire_time_is_the_moment_minus_the_reminder() {
        assert_eq!(
            fire_at_local("2026-10-07", Some("14:30"), 0),
            Some(OCT_7_1430)
        );
        assert_eq!(
            fire_at_local("2026-10-07", Some("14:30"), 15),
            Some(OCT_7_1430 - 15 * 60)
        );
        assert_eq!(
            fire_at_local("2026-10-07", Some("14:30"), 1_440),
            Some(OCT_7_1430 - 86_400)
        );
        assert_eq!(
            fire_at_local("2026-10-07", Some("14:30"), 2_880),
            Some(OCT_7_1430 - 2 * 86_400)
        );
    }

    #[test]
    fn a_day_without_a_time_is_at_nine_in_the_morning() {
        let nine = OCT_7_1430 - (14 * 60 + 30 - 9 * 60) * 60;
        assert_eq!(fire_at_local("2026-10-07", None, 0), Some(nine));
        assert_eq!(fire_at_local("2026-10-07", None, 60), Some(nine - 3_600));
        // A malformed time is as good as none, as everywhere else.
        assert_eq!(fire_at_local("2026-10-07", Some("25:00"), 0), Some(nine));
        assert_eq!(fire_at_local("2026-10-07", Some(""), 0), Some(nine));
    }

    #[test]
    fn midnight_and_the_edges_of_the_calendar_work() {
        assert_eq!(fire_at_local("1970-01-01", Some("00:00"), 0), Some(0));
        assert_eq!(fire_at_local("1970-01-01", Some("00:00"), 1), Some(-60));
        assert_eq!(
            fire_at_local("2026-01-01", Some("00:00"), 1),
            fire_at_local("2025-12-31", Some("23:59"), 0)
        );
        assert_eq!(
            fire_at_local("2024-02-29", Some("23:59"), 0),
            fire_at_local("2024-03-01", Some("00:00"), 1)
        );
    }

    #[test]
    fn an_invalid_date_or_an_overflow_has_no_fire_time() {
        assert_eq!(fire_at_local("2026-02-30", None, 0), None);
        assert_eq!(fire_at_local("", None, 0), None);
        assert_eq!(fire_at_local("2026-10-07", None, i64::MAX), None);
        assert!(fire_at_local("9999-12-31", Some("23:59"), 0).is_some());
        assert!(fire_at_local("0001-01-01", Some("00:00"), 0).is_some());
    }
}
