//! A small RFC 3339 parser (Drive's `modifiedTime`), to Unix milliseconds. No clock, no `chrono`.

/// Days from 1970-01-01 to a civil date (proleptic Gregorian; the well-known days-from-civil
/// algorithm: the year starts in March so the leap day is the last day of a year).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The days of a month of the proleptic Gregorian calendar.
pub(crate) fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)` as Unix milliseconds; the fraction is cut
/// to milliseconds. `None` for anything else or an impossible date or time.
pub(crate) fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let number = |from: usize, len: usize| -> Option<i64> {
        let part = bytes.get(from..from.checked_add(len)?)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(
            part.iter()
                .fold(0i64, |acc, d| acc * 10 + i64::from(d - b'0')),
        )
    };
    let at = |index: usize, accepted: &[u8]| bytes.get(index).is_some_and(|b| accepted.contains(b));

    if !(at(4, b"-") && at(7, b"-") && at(10, b"Tt") && at(13, b":") && at(16, b":")) {
        return None;
    }
    let year = number(0, 4)?;
    let month = number(5, 2)?;
    let day = number(8, 2)?;
    let hour = number(11, 2)?;
    let minute = number(14, 2)?;
    let second = number(17, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut position = 19;
    let mut fraction_ms = 0i64;
    if at(position, b".") {
        position += 1;
        let start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        let digits = bytes.get(start..position)?;
        if digits.is_empty() {
            return None;
        }
        for index in 0..3 {
            let digit = digits.get(index).map_or(0, |d| i64::from(d - b'0'));
            fraction_ms = fraction_ms * 10 + digit;
        }
    }

    let offset_s = if at(position, b"Zz") {
        position += 1;
        0
    } else if at(position, b"+-") {
        let negative = at(position, b"-");
        let offset_hour = number(position + 1, 2)?;
        let offset_minute = number(position + 4, 2)?;
        if !at(position + 3, b":") || offset_hour > 23 || offset_minute > 59 {
            return None;
        }
        position += 6;
        let seconds = offset_hour * 3_600 + offset_minute * 60;
        if negative { -seconds } else { seconds }
    } else {
        return None;
    };
    if position != bytes.len() {
        return None;
    }

    let local_s = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    Some((local_s - offset_s) * 1_000 + fraction_ms)
}
