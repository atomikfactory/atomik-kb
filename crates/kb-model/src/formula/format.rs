//! Numbers as text and text as numbers (`formulas.md` sections 3.3 and 3.4.2).

/// The shortest text that reads back as the same number, as JavaScript's `String(n)`: an exponent
/// form below 1e-6 and from 1e21 (`1e+21`, `1.5e-7`), `-0` is `0`.
pub(super) fn number_to_string(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    if !value.is_finite() {
        return String::new();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // Rust's `{:e}` gives the shortest digits that round-trip: `d.ddde<exponent>`.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let count = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    // The value is 0.<digits> * 10^point.
    let point = exponent + 1;
    let body = if count <= point && point <= 21 {
        let zeros = usize::try_from(point - count).unwrap_or(0);
        format!("{digits}{}", "0".repeat(zeros))
    } else if 0 < point && point <= 21 {
        let at = usize::try_from(point).unwrap_or(0);
        let (whole, fraction) = digits.split_at(at.min(digits.len()));
        format!("{whole}.{fraction}")
    } else if -6 < point && point <= 0 {
        let zeros = usize::try_from(-point).unwrap_or(0);
        format!("0.{}{digits}", "0".repeat(zeros))
    } else {
        let power = point - 1;
        let sign_of_power = if power < 0 { '-' } else { '+' };
        let (first, rest) = digits.split_at(1.min(digits.len()));
        let mantissa = if rest.is_empty() {
            first.to_owned()
        } else {
            format!("{first}.{rest}")
        };
        format!("{mantissa}e{sign_of_power}{}", power.abs())
    };
    format!("{sign}{body}")
}

/// `toNumber` of a text: trimmed, a decimal number with an optional sign, fraction and exponent;
/// anything else (`""`, `0x10`, `1_000`, `Infinity`, `NaN`) is nothing.
pub(super) fn parse_number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    if matches!(bytes.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = |from: usize| {
        bytes.get(from..).map_or(0, |rest| {
            rest.iter().take_while(|b| b.is_ascii_digit()).count()
        })
    };
    let whole = digits(i);
    i += whole;
    let mut fraction = 0;
    if bytes.get(i) == Some(&b'.') {
        fraction = digits(i + 1);
        i += 1 + fraction;
    }
    if whole == 0 && fraction == 0 {
        return None;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exponent = digits(i);
        if exponent == 0 {
            return None;
        }
        i += exponent;
    }
    if i != bytes.len() {
        return None;
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_like_javascript() {
        let cases: [(f64, &str); 31] = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (1.5, "1.5"),
            (0.5, "0.5"),
            (123_456_789_012.0, "123456789012"),
            (0.1 + 0.2, "0.30000000000000004"),
            (100.0, "100"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (-1.5e21, "-1.5e+21"),
            (123_456_789_012_345_680_000.0, "123456789012345680000"),
            (1.234_567_8e25, "1.2345678e+25"),
            (0.000_001, "0.000001"),
            (0.000_000_1, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (1.234e-9, "1.234e-9"),
            (0.000_012_34, "0.00001234"),
            (-0.000_000_5, "-5e-7"),
            (12.5, "12.5"),
            (1e300, "1e+300"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (2.0_f64.powi(53), "9007199254740992"),
            (1.0 / 3.0, "0.3333333333333333"),
            (-123.456, "-123.456"),
            (4.35, "4.35"),
            (1e-6, "0.000001"),
            (999_999_999_999_999_900_000.0, "999999999999999900000"),
        ];
        for (value, expected) in cases {
            assert_eq!(number_to_string(value), expected, "{value:e}");
        }
    }

    #[test]
    fn texts_are_numbers_only_in_decimal() {
        for (text, expected) in [
            ("42", Some(42.0)),
            (" 42.5 ", Some(42.5)),
            ("-3", Some(-3.0)),
            ("+3", Some(3.0)),
            (".5", Some(0.5)),
            ("5.", Some(5.0)),
            ("1e3", Some(1000.0)),
            ("1E-2", Some(0.01)),
            ("-1.5e+2", Some(-150.0)),
            ("\t7\n", Some(7.0)),
            ("", None),
            (" ", None),
            ("abc", None),
            ("0x10", None),
            ("1_000", None),
            ("Infinity", None),
            ("NaN", None),
            ("inf", None),
            ("1e", None),
            ("1e+", None),
            (".", None),
            ("+", None),
            ("1 2", None),
            ("--1", None),
            ("1e999", None),
        ] {
            assert_eq!(parse_number(text), expected, "{text:?}");
        }
    }
}
