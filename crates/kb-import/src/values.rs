//! Parsing of values read as text (CSV cells, front matter strings) into the forms properties
//! store; shared with the core, which coerces an [`crate::ImportValue`] into a property's kind.
//!
//! Everything here is strict on purpose: a text that is not clearly a number, a checkbox word or
//! a date stays text, so a column of identifiers, zip codes or free words is never turned into
//! something else.

/// A finite number written as `-12`, `3.5`, `1e3` (a `.` decimal separator; no thousands
/// separators, currency or percent signs).
///
/// Surrounding white space is ignored. An integer with a leading zero (`007`, `01234`) is an
/// identifier, and a leading `+` makes a phone number: neither is a number. `NaN`, `inf` and
/// anything that overflows are refused too.
#[must_use]
pub fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    let mut at = 0;
    // No leading `+`: `+33612345678` is a phone number, not a number.
    if bytes.first() == Some(&b'-') {
        at += 1;
    }
    let int_start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    let int_digits = at - int_start;
    let mut frac_digits = 0;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let frac_start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        frac_digits = at - frac_start;
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    // `007` and `01234` are identifiers; `0`, `0.5` and `-0.25` are numbers.
    if int_digits > 1 && bytes.get(int_start) == Some(&b'0') {
        return None;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let exp_start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == exp_start {
            return None;
        }
    }
    if at != bytes.len() {
        return None;
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// A checkbox word: `yes`/`no`, `true`/`false`, `checked`/`unchecked`, `x`/empty, `✓`, `1`/`0`
/// (case-insensitive).
///
/// An empty text is `false` (an unchecked box in a CSV file is an empty cell); `x`, `✓`, `✔`,
/// `☑` and `✅` are `true`, `☐` is `false`.
#[must_use]
pub fn parse_bool(text: &str) -> Option<bool> {
    let text = text.trim();
    if text.is_empty() {
        return Some(false);
    }
    let lower = text.to_lowercase();
    match lower.as_str() {
        "yes" | "true" | "checked" | "x" | "1" | "✓" | "✔" | "☑" | "✅" => Some(true),
        "no" | "false" | "unchecked" | "0" | "☐" => Some(false),
        _ => None,
    }
}

/// A date and optional time: ISO `2026-10-07`, `2026-10-07 14:30`, `2026-10-07T14:30[:ss][Z]`,
/// `2026/10/07`, and Notion's `October 7, 2026`, `Oct 7, 2026` and `October 7, 2026 2:30 PM`
/// (a range `A → B` reads as its start). Answers (`YYYY-MM-DD`, `HH:mm`).
///
/// The day must exist in the Gregorian calendar and the time must be `00:00` to `23:59`. A time
/// zone is not read: the date is a floating local date (ADR-0016), so `...T14:30Z` is 14:30. A
/// trailing parenthesis (`(GMT+2)`) is ignored; day-first (`7 October 2026`) and a leading
/// weekday (`Wednesday, October 7, 2026`) are read too. Slash dates other than year first
/// (`10/07/2026`) are ambiguous and stay text.
#[must_use]
pub fn parse_date(text: &str) -> Option<(String, Option<String>)> {
    let mut text = text.trim();
    // A range reads as its start.
    if let Some((start, _)) = text.split_once('→') {
        text = start.trim_end();
    }
    text = text.trim_start_matches('@').trim_start();
    // A trailing parenthesis, `(GMT+2)`.
    if text.ends_with(')')
        && let Some(open) = text.rfind('(')
    {
        text = text[..open].trim_end();
    }
    if text.is_empty() || text.len() > 64 {
        return None;
    }
    parse_numeric_date(text).or_else(|| parse_named_date(text))
}

/// `true` for a real day of the Gregorian calendar (year 1 to 9999).
pub(crate) fn valid_day(year: u32, month: u32, day: u32) -> bool {
    if year == 0 || year > 9999 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

/// `YYYY-MM-DD` of a checked day.
fn iso(year: u32, month: u32, day: u32) -> Option<String> {
    valid_day(year, month, day).then(|| format!("{year:04}-{month:02}-{day:02}"))
}

/// The digits at the start of `text` (at most `max`), their value and the rest.
fn digits(text: &str, max: usize) -> Option<(u32, &str)> {
    let end = text
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 || end > max {
        return None;
    }
    let value = text[..end].parse::<u32>().ok()?;
    Some((value, &text[end..]))
}

/// `YYYY-MM-DD`, `YYYY/MM/DD` or `YYYY.MM.DD`, then nothing, or a time after a space or `T`.
fn parse_numeric_date(text: &str) -> Option<(String, Option<String>)> {
    let (year, rest) = digits(text, 4)?;
    if text.len() - rest.len() != 4 {
        return None;
    }
    let sep = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '-' | '/' | '.'))?;
    let (month, rest) = digits(&rest[1..], 2)?;
    let rest = rest.strip_prefix(sep)?;
    let (day, rest) = digits(rest, 2)?;
    let date = iso(year, month, day)?;
    if rest.is_empty() {
        return Some((date, None));
    }
    let rest = rest
        .strip_prefix('T')
        .or_else(|| rest.strip_prefix('t'))
        .or_else(|| rest.strip_prefix(' '))?;
    let time = parse_clock(rest, true)?;
    Some((date, Some(time)))
}

/// A clock time: `H:MM`, `HH:MM`, `HH:MM:SS`, `HH:MM:SS.fff`, optionally followed by a
/// `Z` or `±hh:mm` offset (`iso` only, ignored) or `AM`/`PM` (12-hour clocks). Answers `HH:mm`.
fn parse_clock(text: &str, iso_zone: bool) -> Option<String> {
    let text = text.trim();
    let (hour, rest) = digits(text, 2)?;
    let rest = rest.strip_prefix(':')?;
    let (minute, mut rest) = digits(rest, 2)?;
    if text.len() - rest.len() < 4 {
        return None;
    }
    if let Some(after) = rest.strip_prefix(':') {
        let (second, after) = digits(after, 2)?;
        if second > 59 {
            return None;
        }
        rest = after;
        if let Some(fraction) = rest.strip_prefix('.') {
            let end = fraction
                .bytes()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(fraction.len());
            if end == 0 {
                return None;
            }
            rest = &fraction[end..];
        }
    }
    let rest = rest.trim();
    let mut hour = hour;
    let meridiem = rest.to_ascii_lowercase();
    match meridiem.as_str() {
        "" => {}
        "am" | "pm" | "a.m." | "p.m." => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            let pm = meridiem.starts_with('p');
            hour = match (hour, pm) {
                (12, false) => 0,
                (12, true) => 12,
                (h, true) => h + 12,
                (h, false) => h,
            };
        }
        "z" if iso_zone => {}
        zone if iso_zone && is_offset(zone) => {}
        _ => return None,
    }
    (hour <= 23 && minute <= 59).then(|| format!("{hour:02}:{minute:02}"))
}

/// `+02:00`, `-0500`, `+02`.
fn is_offset(text: &str) -> bool {
    let Some(rest) = text.strip_prefix(['+', '-']) else {
        return false;
    };
    let digits_only: String = rest.chars().filter(|c| *c != ':').collect();
    matches!(digits_only.len(), 2 | 4) && digits_only.bytes().all(|b| b.is_ascii_digit())
}

/// The month number of an English month name or its abbreviation.
fn month_of(word: &str) -> Option<u32> {
    let lower = word.trim_end_matches('.').to_ascii_lowercase();
    const NAMES: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    if lower.len() < 3 {
        return None;
    }
    NAMES.iter().enumerate().find_map(|(index, name)| {
        let short = &name[..3];
        let matches = lower == *name || lower == short || (lower == "sept" && index == 8);
        matches.then(|| u32::try_from(index + 1).ok()).flatten()
    })
}

fn is_weekday(word: &str) -> bool {
    let lower = word.trim_end_matches(['.', ',']).to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "monday"
            | "tuesday"
            | "wednesday"
            | "thursday"
            | "friday"
            | "saturday"
            | "sunday"
            | "mon"
            | "tue"
            | "tues"
            | "wed"
            | "thu"
            | "thur"
            | "thurs"
            | "fri"
            | "sat"
            | "sun"
    )
}

/// A day number with an optional English ordinal suffix (`7`, `7th`).
fn day_number(word: &str) -> Option<u32> {
    let word = word.trim_end_matches(',');
    let (value, rest) = digits(word, 2)?;
    matches!(rest, "" | "st" | "nd" | "rd" | "th").then_some(value)
}

/// `October 7, 2026`, `Oct 7 2026`, `7 October 2026`, each with a time after it, and an
/// optional leading weekday.
fn parse_named_date(text: &str) -> Option<(String, Option<String>)> {
    let mut words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 1 && is_weekday(words[0]) {
        words.remove(0);
    }
    let (year, month, day, used) = match words.as_slice() {
        [first, second, third, ..] => match (
            month_of(first.trim_end_matches(',')),
            month_of(second.trim_end_matches(',')),
        ) {
            (Some(month), _) => (year_number(third)?, month, day_number(second)?, 3),
            (None, Some(month)) => (year_number(third)?, month, day_number(first)?, 3),
            (None, None) => return None,
        },
        _ => return None,
    };
    let date = iso(year, month, day)?;
    let rest = &words[used..];
    if rest.is_empty() {
        return Some((date, None));
    }
    if rest.len() > 2 {
        return None;
    }
    let time = parse_clock(&rest.join(" "), false)?;
    Some((date, Some(time)))
}

/// A four-digit year (a trailing comma allowed).
fn year_number(word: &str) -> Option<u32> {
    let word = word.trim_end_matches(',');
    let (value, rest) = digits(word, 4)?;
    (rest.is_empty() && word.len() == 4).then_some(value)
}

/// A number as text: integral values without `.0`, the rest in Rust's shortest form.
pub(crate) fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        // The check above makes the conversion exact.
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn date(text: &str) -> Option<(String, Option<String>)> {
        parse_date(text)
    }

    fn d(day: &str) -> Option<(String, Option<String>)> {
        Some((day.to_owned(), None))
    }

    fn dt(day: &str, time: &str) -> Option<(String, Option<String>)> {
        Some((day.to_owned(), Some(time.to_owned())))
    }

    #[test]
    fn numbers_are_strict() {
        assert_eq!(parse_number("-12"), Some(-12.0));
        assert_eq!(parse_number(" 3.5 "), Some(3.5));
        assert_eq!(parse_number("1e3"), Some(1000.0));
        assert_eq!(parse_number("1E-2"), Some(0.01));
        assert_eq!(parse_number("0"), Some(0.0));
        assert_eq!(parse_number("0.5"), Some(0.5));
        assert_eq!(parse_number(".5"), Some(0.5));
        assert_eq!(parse_number("5."), Some(5.0));
        for refused in [
            "",
            " ",
            "abc",
            "1,000",
            "$5",
            "50%",
            "NaN",
            "inf",
            "-inf",
            "infinity",
            "0x10",
            "1_000",
            "1e",
            "e5",
            "--1",
            "1.2.3",
            "007",
            "01234",
            "1 2",
            "1e999",
            "١٢",
            "+7",
            "+33612345678",
        ] {
            assert_eq!(parse_number(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn checkbox_words() {
        for yes in [
            "Yes", "TRUE", "checked", "x", "X", "1", "✓", "✔", "☑", " yes ",
        ] {
            assert_eq!(parse_bool(yes), Some(true), "{yes:?}");
        }
        for no in ["No", "false", "UNCHECKED", "0", "", "  ", "☐"] {
            assert_eq!(parse_bool(no), Some(false), "{no:?}");
        }
        for neither in ["maybe", "2", "yes please", "nope", "on"] {
            assert_eq!(parse_bool(neither), None, "{neither:?}");
        }
    }

    #[test]
    fn iso_dates() {
        assert_eq!(date("2026-10-07"), d("2026-10-07"));
        assert_eq!(date(" 2026-10-07 "), d("2026-10-07"));
        assert_eq!(date("2026-10-07 14:30"), dt("2026-10-07", "14:30"));
        assert_eq!(date("2026-10-07T14:30"), dt("2026-10-07", "14:30"));
        assert_eq!(date("2026-10-07T14:30:15"), dt("2026-10-07", "14:30"));
        assert_eq!(date("2026-10-07T14:30:15Z"), dt("2026-10-07", "14:30"));
        assert_eq!(
            date("2026-10-07T14:30:15.123+02:00"),
            dt("2026-10-07", "14:30")
        );
        assert_eq!(date("2026-10-07T09:05:00-0500"), dt("2026-10-07", "09:05"));
        assert_eq!(date("2026/10/07"), d("2026-10-07"));
        assert_eq!(date("2026.10.07"), d("2026-10-07"));
        assert_eq!(date("2026-1-5"), d("2026-01-05"));
        assert_eq!(date("2026-10-07 9:05"), dt("2026-10-07", "09:05"));
        assert_eq!(date("2028-02-29"), d("2028-02-29"));
    }

    #[test]
    fn invalid_dates_are_refused() {
        for refused in [
            "",
            "2026",
            "2026-13-01",
            "2026-00-10",
            "2026-02-29",
            "2026-04-31",
            "0000-01-01",
            "2026-10-07 24:00",
            "2026-10-07 14:60",
            "2026-10-07 14",
            "2026-10-07T",
            "2026-10-07 14:30 extra",
            "10/07/2026",
            "07.10.2026",
            "20261007",
            "12345-10-07",
            "hello",
            "October 32, 2026",
            "Octember 7, 2026",
            "October 7",
            "October 7, 26",
            "-5",
        ] {
            assert_eq!(date(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn notion_dates() {
        assert_eq!(date("October 7, 2026"), d("2026-10-07"));
        assert_eq!(date("Oct 7, 2026"), d("2026-10-07"));
        assert_eq!(date("October 7 2026"), d("2026-10-07"));
        assert_eq!(date("Sept 7, 2026"), d("2026-09-07"));
        assert_eq!(date("May 1, 2026"), d("2026-05-01"));
        assert_eq!(date("7 October 2026"), d("2026-10-07"));
        assert_eq!(date("7 Oct 2026"), d("2026-10-07"));
        assert_eq!(date("October 7th, 2026"), d("2026-10-07"));
        assert_eq!(date("Wednesday, October 7, 2026"), d("2026-10-07"));
        assert_eq!(date("@October 7, 2026"), d("2026-10-07"));
        assert_eq!(date("October 7, 2026 2:30 PM"), dt("2026-10-07", "14:30"));
        assert_eq!(date("October 7, 2026 2:30PM"), dt("2026-10-07", "14:30"));
        assert_eq!(date("October 7, 2026 12:05 AM"), dt("2026-10-07", "00:05"));
        assert_eq!(date("October 7, 2026 12:05 PM"), dt("2026-10-07", "12:05"));
        assert_eq!(date("October 7, 2026 14:30"), dt("2026-10-07", "14:30"));
        assert_eq!(
            date("October 7, 2026 2:30 PM (GMT+2)"),
            dt("2026-10-07", "14:30")
        );
        assert_eq!(date("October 7, 2026 13:00 PM"), None);
        assert_eq!(date("October 7, 2026 0:30 AM"), None);
    }

    #[test]
    fn ranges_read_as_their_start() {
        assert_eq!(date("October 7, 2026 → October 9, 2026"), d("2026-10-07"));
        assert_eq!(
            date("October 7, 2026 2:30 PM → October 7, 2026 4:00 PM"),
            dt("2026-10-07", "14:30")
        );
        assert_eq!(date("2026-10-07 → 2026-10-09"), d("2026-10-07"));
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let long = "9".repeat(10_000);
        for text in [
            long.as_str(),
            "\u{0}",
            "→",
            "(",
            ")",
            "(((((",
            "2026-10-07T",
            "٢٠٢٦-١٠-٠٧",
            "Oct",
            "Oct 7 2026 1:1:1:1",
        ] {
            let _ = parse_date(text);
            let _ = parse_number(text);
            let _ = parse_bool(text);
        }
    }

    #[test]
    fn number_text_is_shortest() {
        assert_eq!(number_text(3.0), "3");
        assert_eq!(number_text(-3.0), "-3");
        assert_eq!(number_text(3.5), "3.5");
        assert_eq!(number_text(0.1), "0.1");
        assert_eq!(number_text(1e21), "1000000000000000000000");
    }
}
