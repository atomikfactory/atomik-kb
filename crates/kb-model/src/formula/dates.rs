//! Dates (`formulas.md` sections 2.4 and 3.5): local "floating" date-times as milliseconds since
//! 1970-01-01T00:00 local time, the civil calendar (proleptic Gregorian, years 0 to 9999), ISO weeks,
//! `dateAdd`, `dateBetween`, `formatDate` and `parseDate`.

use kb_types::DateValue;

pub(super) const MS_MINUTE: i64 = 60_000;
pub(super) const MS_HOUR: i64 = 3_600_000;
pub(super) const MS_DAY: i64 = 86_400_000;
pub(super) const MS_WEEK: i64 = 604_800_000;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Days since 1970-01-01 of a civil date (Hinnant's algorithm).
pub(super) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date (year, month 1..12, day 1..31) of a day count since 1970-01-01.
pub(super) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let y = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    (if month <= 2 { y + 1 } else { y }, month, day)
}

pub(super) fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub(super) fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The earliest date a formula can hold (0001-01-01T00:00).
fn min_ms() -> i64 {
    days_from_civil(1, 1, 1) * MS_DAY
}

/// The latest (9999-12-31T23:59:59.999).
fn max_ms() -> i64 {
    (days_from_civil(9999, 12, 31) + 1) * MS_DAY - 1
}

/// Whether a date can be written as a `DateValue` (year 1 to 9999).
pub(super) fn in_range(ms: i64) -> bool {
    (min_ms()..=max_ms()).contains(&ms)
}

/// A date taken apart.
#[derive(Debug, Clone, Copy)]
pub(super) struct Parts {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    /// 1 (Monday) to 7 (Sunday).
    pub weekday: i64,
    /// Day of the year from 1.
    pub ordinal: i64,
}

pub(super) fn parts(ms: i64) -> Parts {
    let days = ms.div_euclid(MS_DAY);
    let of_day = ms.rem_euclid(MS_DAY);
    let (year, month, day) = civil_from_days(days);
    Parts {
        year,
        month,
        day,
        hour: of_day / MS_HOUR,
        minute: of_day % MS_HOUR / MS_MINUTE,
        second: of_day % MS_MINUTE / 1000,
        weekday: (days + 3).rem_euclid(7) + 1,
        ordinal: days - days_from_civil(year, 1, 1) + 1,
    }
}

/// The date at 00:00 of a civil day, `None` outside 0001..9999.
fn start_of(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=9999).contains(&year) {
        return None;
    }
    Some(days_from_civil(year, month, day) * MS_DAY)
}

fn weeks_in_year(year: i64) -> i64 {
    let p = |y: i64| (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400)).rem_euclid(7);
    if p(year) == 4 || p(year - 1) == 3 {
        53
    } else {
        52
    }
}

/// The ISO 8601 week number, 1..53.
pub(super) fn iso_week(p: &Parts) -> i64 {
    let week = (p.ordinal - p.weekday + 10).div_euclid(7);
    if week < 1 {
        weeks_in_year(p.year - 1)
    } else if week > weeks_in_year(p.year) {
        1
    } else {
        week
    }
}

/// The truncation of a number to a whole one, `None` when it does not fit.
#[allow(clippy::cast_possible_truncation)]
pub(super) fn to_i64(value: f64) -> Option<i64> {
    let whole = value.trunc();
    (whole.abs() < 9.0e18).then_some(whole as i64)
}

/// Months added, the day of the month clamped to the target month's last day.
fn add_months(ms: i64, months: i64) -> Option<i64> {
    let p = parts(ms);
    let total = (p.year * 12 + (p.month - 1)).checked_add(months)?;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) + 1;
    let day = p.day.min(days_in_month(year, month));
    let start = start_of(year, month, day)?;
    let moved = start + ms.rem_euclid(MS_DAY);
    in_range(moved).then_some(moved)
}

/// `dateAdd`: `count` units added (negative to subtract); `None` for an unknown unit or a result
/// outside the dates.
pub(super) fn add(ms: i64, count: i64, unit: &str) -> Option<i64> {
    let delta = match unit {
        "minutes" => count.checked_mul(MS_MINUTE)?,
        "hours" => count.checked_mul(MS_HOUR)?,
        "days" => count.checked_mul(MS_DAY)?,
        "weeks" => count.checked_mul(MS_WEEK)?,
        "months" => return add_months(ms, count),
        "quarters" => return add_months(ms, count.checked_mul(3)?),
        "years" => return add_months(ms, count.checked_mul(12)?),
        _ => return None,
    };
    let moved = ms.checked_add(delta)?;
    in_range(moved).then_some(moved)
}

/// Whether adding in this unit gives the result a time of day.
pub(super) fn unit_sets_time(unit: &str) -> bool {
    unit == "hours" || unit == "minutes"
}

/// Whole months from `b` to `a` (section 3.5): the difference of the calendar months, less one
/// when the rest of the later date (day of the month and time of day) has not reached the rest of
/// the earlier one.
fn month_difference(a: i64, b: i64) -> i64 {
    let (pa, pb) = (parts(a), parts(b));
    let months = (pa.year - pb.year) * 12 + (pa.month - pb.month);
    let rest_a = (pa.day, a.rem_euclid(MS_DAY));
    let rest_b = (pb.day, b.rem_euclid(MS_DAY));
    if months > 0 && rest_a < rest_b {
        months - 1
    } else if months < 0 && rest_a > rest_b {
        months + 1
    } else {
        months
    }
}

/// `dateBetween`: `a - b` in whole units truncated toward zero; `None` for an unknown unit.
pub(super) fn between(a: i64, b: i64, unit: &str) -> Option<i64> {
    let difference = a.checked_sub(b)?;
    Some(match unit {
        "minutes" => difference / MS_MINUTE,
        "hours" => difference / MS_HOUR,
        "days" => difference / MS_DAY,
        "weeks" => difference / MS_WEEK,
        "months" => month_difference(a, b),
        "quarters" => month_difference(a, b) / 3,
        "years" => month_difference(a, b) / 12,
        _ => return None,
    })
}

fn ordinal_suffix(day: i64) -> &'static str {
    if (11..=13).contains(&(day % 100)) {
        return "th";
    }
    match day % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

const TOKENS: [&str; 23] = [
    "YYYY", "MMMM", "dddd", "MMM", "ddd", "YY", "MM", "DD", "Do", "HH", "hh", "mm", "ss", "Q", "M",
    "D", "d", "H", "h", "m", "A", "a", "s",
];

fn name_of(names: &[&'static str], index: i64) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i))
        .copied()
        .unwrap_or("")
}

fn token_text(token: &str, p: &Parts) -> String {
    let weekday = p.weekday % 7;
    let hour12 = if p.hour % 12 == 0 { 12 } else { p.hour % 12 };
    match token {
        "YYYY" => format!("{:04}", p.year),
        "YY" => format!("{:02}", p.year.rem_euclid(100)),
        "Q" => ((p.month - 1) / 3 + 1).to_string(),
        "MMMM" => name_of(&MONTHS, p.month - 1).to_owned(),
        "MMM" => name_of(&MONTHS, p.month - 1).chars().take(3).collect(),
        "MM" => format!("{:02}", p.month),
        "M" => p.month.to_string(),
        "DD" => format!("{:02}", p.day),
        "D" => p.day.to_string(),
        "Do" => format!("{}{}", p.day, ordinal_suffix(p.day)),
        "dddd" => name_of(&WEEKDAYS, weekday).to_owned(),
        "ddd" => name_of(&WEEKDAYS, weekday).chars().take(3).collect(),
        "d" => weekday.to_string(),
        "HH" => format!("{:02}", p.hour),
        "H" => p.hour.to_string(),
        "hh" => format!("{hour12:02}"),
        "h" => hour12.to_string(),
        "mm" => format!("{:02}", p.minute),
        "m" => p.minute.to_string(),
        "A" => if p.hour < 12 { "AM" } else { "PM" }.to_owned(),
        "a" => if p.hour < 12 { "am" } else { "pm" }.to_owned(),
        "ss" => format!("{:02}", p.second),
        "s" => p.second.to_string(),
        _ => String::new(),
    }
}

/// `formatDate`: the tokens of section 3.5 (longest match first), text in `[ ]` copied as is,
/// anything else copied.
pub(super) fn format_date(ms: i64, pattern: &str) -> String {
    let p = parts(ms);
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c == '['
            && let Some(close) = chars
                .get(i + 1..)
                .and_then(|rest| rest.iter().position(|&x| x == ']'))
        {
            out.extend(chars.get(i + 1..i + 1 + close).unwrap_or_default());
            i += close + 2;
            continue;
        }
        let rest = chars.get(i..).unwrap_or_default();
        let token = TOKENS.iter().find(|token| {
            token
                .chars()
                .enumerate()
                .all(|(k, expected)| rest.get(k) == Some(&expected))
        });
        match token {
            Some(token) => {
                out.push_str(&token_text(token, &p));
                i += token.len();
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn digits(bytes: &[u8], from: usize, count: usize) -> Option<i64> {
    let slice = bytes.get(from..from + count)?;
    let mut value = 0_i64;
    for byte in slice {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(value)
}

/// `YYYY-MM-DD` at the start of `bytes`: the day's midnight.
fn parse_day(bytes: &[u8]) -> Option<i64> {
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let (year, month, day) = (
        digits(bytes, 0, 4)?,
        digits(bytes, 5, 2)?,
        digits(bytes, 8, 2)?,
    );
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    start_of(year, month, day)
}

/// `HH:mm` or `HH:mm:ss` in `bytes`: the milliseconds since midnight.
fn parse_time(bytes: &[u8]) -> Option<i64> {
    if bytes.get(2) != Some(&b':') {
        return None;
    }
    let (hour, minute) = (digits(bytes, 0, 2)?, digits(bytes, 3, 2)?);
    let second = match bytes.len() {
        5 => 0,
        8 if bytes.get(5) == Some(&b':') => digits(bytes, 6, 2)?,
        _ => return None,
    };
    (hour < 24 && minute < 60 && second < 60)
        .then_some(hour * MS_HOUR + minute * MS_MINUTE + second * 1000)
}

/// `parseDate`: `YYYY-MM-DD`, `YYYY-MM-DDTHH:mm` or `YYYY-MM-DD HH:mm`, seconds allowed; the date
/// and whether it has a time.
pub(super) fn parse_date(text: &str) -> Option<(i64, bool)> {
    let bytes = text.as_bytes();
    let day = parse_day(bytes.get(..10)?)?;
    if bytes.len() == 10 {
        return Some((day, false));
    }
    if !matches!(bytes.get(10), Some(b'T' | b' ')) {
        return None;
    }
    let time = parse_time(bytes.get(11..)?)?;
    Some((day + time, true))
}

/// A `DateValue` as a date: the milliseconds and whether it has a time; `None` for a damaged one.
pub(super) fn from_value(value: &DateValue) -> Option<(i64, bool)> {
    let day = parse_day(value.date.as_bytes())?;
    if value.date.len() != 10 {
        return None;
    }
    match &value.time {
        None => Some((day, false)),
        Some(time) => Some((day + parse_time(time.as_bytes())?, true)),
    }
}

/// A date as a `DateValue` (seconds and milliseconds are dropped); `None` outside 0001..9999.
pub(super) fn to_value(ms: i64, has_time: bool) -> Option<DateValue> {
    if !in_range(ms) {
        return None;
    }
    let p = parts(ms);
    Some(DateValue {
        date: format!("{:04}-{:02}-{:02}", p.year, p.month, p.day),
        time: has_time.then(|| format!("{:02}:{:02}", p.hour, p.minute)),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn ms(text: &str) -> i64 {
        parse_date(text).unwrap().0
    }

    #[test]
    fn the_calendar_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for days in (-800_000..3_000_000).step_by(997) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
            assert!((1..=12).contains(&m) && (1..=days_in_month(y, m)).contains(&d));
        }
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(parts(ms("2026-10-18")).weekday, 7);
        assert_eq!(parts(ms("2026-10-19")).weekday, 1);
        assert_eq!(parts(ms("1970-01-01")).weekday, 4);
    }

    #[test]
    fn iso_weeks() {
        let week = |text: &str| iso_week(&parts(ms(text)));
        assert_eq!(week("2026-01-01"), 1);
        assert_eq!(week("2020-12-31"), 53);
        assert_eq!(week("2021-01-01"), 53);
        assert_eq!(week("2021-01-04"), 1);
        assert_eq!(week("2018-12-31"), 1);
        assert_eq!(week("2024-12-30"), 1);
        assert_eq!(week("2026-12-31"), 53);
        assert_eq!(week("2027-01-03"), 53);
        assert_eq!(week("2026-10-18"), 42);
    }

    #[test]
    fn dates_are_added_with_clamping() {
        let text = |ms: i64| format_date(ms, "YYYY-MM-DD HH:mm");
        let jan31 = ms("2026-01-31");
        assert_eq!(text(add(jan31, 1, "months").unwrap()), "2026-02-28 00:00");
        assert_eq!(text(add(jan31, 13, "months").unwrap()), "2027-02-28 00:00");
        assert_eq!(
            text(add(ms("2024-01-31"), 1, "months").unwrap()),
            "2024-02-29 00:00"
        );
        assert_eq!(
            text(add(ms("2024-02-29"), 1, "years").unwrap()),
            "2025-02-28 00:00"
        );
        assert_eq!(
            text(add(ms("2026-11-30"), 1, "quarters").unwrap()),
            "2027-02-28 00:00"
        );
        assert_eq!(
            text(add(ms("2026-03-31"), -1, "months").unwrap()),
            "2026-02-28 00:00"
        );
        assert_eq!(
            text(add(ms("2026-01-15"), -2, "months").unwrap()),
            "2025-11-15 00:00"
        );
        assert_eq!(
            text(add(ms("2026-12-31"), 1, "days").unwrap()),
            "2027-01-01 00:00"
        );
        assert_eq!(
            text(add(ms("2026-12-31T23:30"), 45, "minutes").unwrap()),
            "2027-01-01 00:15"
        );
        assert_eq!(
            text(add(ms("2026-01-01"), 2, "weeks").unwrap()),
            "2026-01-15 00:00"
        );
        assert!(add(jan31, 1, "day").is_none());
        assert!(add(ms("9999-12-31"), 1, "days").is_none());
        assert!(add(jan31, i64::MAX, "months").is_none());
        assert!(add(jan31, i64::MAX, "days").is_none());
        // the time of day is kept by months
        assert_eq!(
            text(add(ms("2026-01-31T10:45"), 1, "months").unwrap()),
            "2026-02-28 10:45"
        );
    }

    #[test]
    fn differences_follow_the_month_algorithm() {
        let diff = |a: &str, b: &str, unit: &str| between(ms(a), ms(b), unit).unwrap();
        assert_eq!(diff("2026-01-31", "2026-03-01", "months"), -1);
        assert_eq!(diff("2026-03-01", "2026-01-31", "months"), 1);
        assert_eq!(diff("2026-03-31", "2026-01-31", "months"), 2);
        assert_eq!(diff("2026-03-30", "2026-01-31", "months"), 1);
        assert_eq!(diff("2026-03-30T10:00", "2026-03-30T09:00", "months"), 0);
        assert_eq!(diff("2027-01-01", "2026-01-02", "years"), 0);
        assert_eq!(diff("2027-01-02", "2026-01-02", "years"), 1);
        assert_eq!(diff("2025-01-02", "2026-01-02", "years"), -1);
        assert_eq!(diff("2026-12-31", "2026-01-01", "quarters"), 3);
        assert_eq!(diff("2026-01-10", "2026-01-01", "weeks"), 1);
        assert_eq!(diff("2026-01-01", "2026-01-10", "weeks"), -1);
        assert_eq!(diff("2026-01-01T00:00", "2026-01-10T12:00", "days"), -9);
        assert_eq!(diff("2026-01-01T01:30", "2026-01-01T00:00", "minutes"), 90);
        assert!(between(0, 0, "weekz").is_none());
    }

    #[test]
    fn dates_are_formatted() {
        let t = ms("2026-10-02T15:04:05");
        assert_eq!(
            format_date(t, "dddd, MMMM Do YYYY h:mm A"),
            "Friday, October 2nd 2026 3:04 PM"
        );
        assert_eq!(
            format_date(t, "ddd MMM D YY Q HH:mm:ss a d"),
            "Fri Oct 2 26 4 15:04:05 pm 5"
        );
        assert_eq!(format_date(t, "M/D/YYYY hh:mm:ss"), "10/2/2026 03:04:05");
        assert_eq!(format_date(ms("2026-01-01T00:05"), "h:mm A"), "12:05 AM");
        assert_eq!(format_date(ms("2026-01-01T12:05"), "h A"), "12 PM");
        assert_eq!(
            format_date(t, "[Today is] dddd [at] H"),
            "Today is Friday at 15"
        );
        assert_eq!(format_date(t, "[xyz YYYY"), "[xyz 2026");
        assert_eq!(format_date(t, "T x"), "T x");
        for (day, suffix) in [
            (1, "1st"),
            (2, "2nd"),
            (3, "3rd"),
            (4, "4th"),
            (11, "11th"),
            (12, "12th"),
            (13, "13th"),
            (21, "21st"),
            (22, "22nd"),
            (23, "23rd"),
            (31, "31st"),
        ] {
            assert_eq!(format_date(ms(&format!("2026-01-{day:02}")), "Do"), suffix);
        }
        assert_eq!(format_date(ms("0007-01-01"), "YYYY"), "0007");
    }

    #[test]
    fn dates_are_parsed() {
        assert_eq!(parse_date("2026-10-18"), Some((ms("2026-10-18"), false)));
        let with_time = parse_date("2026-10-18T09:30").unwrap();
        assert!(with_time.1);
        assert_eq!(with_time.0, ms("2026-10-18") + 9 * MS_HOUR + 30 * MS_MINUTE);
        assert_eq!(
            parse_date("2026-10-18 09:30"),
            parse_date("2026-10-18T09:30")
        );
        assert_eq!(
            parse_date("2026-10-18T09:30:15").unwrap().0 - with_time.0,
            15_000
        );
        for bad in [
            "",
            "2026-10-1",
            "2026-13-01",
            "2026-02-30",
            "2026-10-18T24:00",
            "2026-10-18T09:60",
            "2026-10-18T09:30:60",
            "2026-10-18X09:30",
            "2026-10-18T9:30",
            " 2026-10-18",
            "2026-10-18 ",
            "26-10-18",
            "2026/10/18",
            "２０２６-10-18",
        ] {
            assert_eq!(parse_date(bad), None, "{bad:?}");
        }
        assert!(parse_date("2024-02-29").is_some());
        assert!(parse_date("2025-02-29").is_none());
    }

    #[test]
    fn date_values_convert_both_ways() {
        let value = DateValue {
            date: "2026-10-18".into(),
            time: Some("09:30".into()),
        };
        let (t, has_time) = from_value(&value).unwrap();
        assert!(has_time);
        assert_eq!(to_value(t, true), Some(value));
        // seconds are dropped on output
        assert_eq!(
            to_value(t + 59_000, true).unwrap().time.as_deref(),
            Some("09:30")
        );
        assert_eq!(to_value(t, false).unwrap().time, None);
        assert!(
            from_value(&DateValue {
                date: "2026-10-18x".into(),
                time: None
            })
            .is_none()
        );
        assert!(
            from_value(&DateValue {
                date: "2026-10-18".into(),
                time: Some("9:30".into())
            })
            .is_none()
        );
        assert!(to_value(max_ms() + 1, false).is_none());
        assert!(to_value(min_ms() - 1, false).is_none());
    }
}
