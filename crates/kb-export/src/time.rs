//! Calendar text of Unix milliseconds, without a time zone database: the proleptic Gregorian
//! calendar from the day count (Howard Hinnant's `civil_from_days`).

/// A moment broken into calendar fields.
struct Civil {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
}

/// `ms` (Unix milliseconds) shifted by `offset_seconds`, as calendar fields.
fn civil(ms: i64, offset_seconds: i32) -> Civil {
    let seconds = ms
        .div_euclid(1000)
        .saturating_add(i64::from(offset_seconds));
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date: shift the epoch to 0000-03-01 so that the leap day
    // is the last day of the (March-based) year.
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Civil {
        year,
        month,
        day,
        hour: of_day / 3600,
        minute: of_day % 3600 / 60,
        second: of_day % 60,
    }
}

/// `YYYY-MM-DDTHH:mm:ssZ` (UTC, whole seconds): front matter and the sidecar.
pub(crate) fn rfc3339_utc(ms: i64) -> String {
    let c = civil(ms, 0);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

/// `YYYY-MM-DD HH:mm` on the clock `offset_seconds` from UTC: instants in CSV cells.
pub(crate) fn local_minute(ms: i64, offset_seconds: i32) -> String {
    let c = civil(ms, offset_seconds);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        c.year, c.month, c.day, c.hour, c.minute
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_and_known_moments() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_760_000_000_000), "2025-10-09T08:53:20Z");
        // A leap day, the end of a year, a century that is not a leap year, one that is.
        assert_eq!(rfc3339_utc(1_709_164_800_000), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_767_225_599_000), "2025-12-31T23:59:59Z");
        assert_eq!(rfc3339_utc(4_107_542_400_000), "2100-03-01T00:00:00Z");
        assert_eq!(rfc3339_utc(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn before_the_epoch_and_milliseconds_round_down() {
        assert_eq!(rfc3339_utc(-1), "1969-12-31T23:59:59Z");
        assert_eq!(rfc3339_utc(-86_400_000), "1969-12-31T00:00:00Z");
        assert_eq!(rfc3339_utc(999), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_001), "1970-01-01T00:00:01Z");
    }

    #[test]
    fn local_time_shifts_the_clock_and_the_day() {
        let ms = 1_760_000_000_000; // 2025-10-09T08:53:20Z
        assert_eq!(local_minute(ms, 0), "2025-10-09 08:53");
        assert_eq!(local_minute(ms, 2 * 3600), "2025-10-09 10:53");
        assert_eq!(local_minute(ms, 16 * 3600), "2025-10-10 00:53");
        assert_eq!(local_minute(ms, -9 * 3600), "2025-10-08 23:53");
        assert_eq!(local_minute(ms, 5 * 3600 + 30 * 60), "2025-10-09 14:23");
    }

    #[test]
    fn extreme_values_do_not_panic() {
        let _ = rfc3339_utc(i64::MAX);
        let _ = rfc3339_utc(i64::MIN);
        let _ = local_minute(i64::MAX, i32::MAX);
        let _ = local_minute(i64::MIN, i32::MIN);
    }
}
