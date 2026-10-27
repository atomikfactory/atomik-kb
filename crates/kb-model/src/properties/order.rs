//! Fractional positions of properties and options (ADR-0026).
//!
//! A position is a string of base-62 digits (`0-9A-Za-z`, in that order, which is also byte
//! order) read as the fraction `0.d1d2d3...`; a missing digit is 0. Positions compare as
//! strings. [`key_between`] makes a position that sorts strictly between two others, so an
//! insert or a move rewrites one position and concurrent moves of different items never
//! conflict. Generated keys never end in the digit `0` (so each fraction has one spelling),
//! which leaves room below every key. Two devices that append at the same moment make equal
//! positions; readers break such ties by id.

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// One more than the largest digit: the upper bound of an open end.
const RADIX: u8 = 62;

fn digit_value(byte: u8) -> Option<u8> {
    DIGITS
        .iter()
        .position(|candidate| *candidate == byte)
        .and_then(|at| u8::try_from(at).ok())
}

/// The digit values of a position, or `None` if a character is not a digit.
fn digits_of(position: &str) -> Option<Vec<u8>> {
    position.bytes().map(digit_value).collect()
}

fn text_of(digits: &[u8]) -> String {
    digits
        .iter()
        .filter_map(|digit| DIGITS.get(usize::from(*digit)).copied())
        .map(char::from)
        .collect()
}

/// A position strictly between `before` and `after`; `None` for an open end. `None` when the
/// bounds are not valid positions or `before` is not smaller than `after`.
pub(crate) fn key_between(before: Option<&str>, after: Option<&str>) -> Option<String> {
    let low = match before {
        Some(text) => digits_of(text)?,
        None => Vec::new(),
    };
    let high = match after {
        Some(text) => Some(digits_of(text)?),
        None => None,
    };
    if let Some(high) = &high {
        // Compare as fractions: pad the shorter one with zeros.
        let len = low.len().max(high.len());
        let pad = |digits: &[u8]| -> Vec<u8> {
            let mut padded = digits.to_vec();
            padded.resize(len, 0);
            padded
        };
        if pad(&low) >= pad(high) {
            return None;
        }
    }
    Some(text_of(&midpoint(&low, high.as_deref())))
}

/// The recursive step: a digit string above `low` (padded with zeros) and below `high`
/// (`None` = 1.0). `low` is below `high`.
fn midpoint(low: &[u8], high: Option<&[u8]>) -> Vec<u8> {
    if let Some(high) = high {
        // Share the common prefix, then split what is left.
        let mut shared = 0;
        while shared < high.len() && low.get(shared).copied().unwrap_or(0) == high[shared] {
            shared += 1;
        }
        if shared > 0 {
            let mut out = high[..shared].to_vec();
            out.extend(midpoint(
                low.get(shared..).unwrap_or(&[]),
                Some(&high[shared..]),
            ));
            return out;
        }
    }
    let low_digit = low.first().copied().unwrap_or(0);
    let high_digit = high.and_then(|high| high.first().copied()).unwrap_or(RADIX);
    if high_digit - low_digit > 1 {
        // Room for a digit in between (at least 1, so the key never ends in 0).
        return vec![low_digit + (high_digit - low_digit) / 2];
    }
    // Consecutive digits: either `high` goes on (so its first digit alone is below it), or
    // keep `low`'s digit and go above the rest of `low`.
    if high.is_some_and(|high| high.len() > 1) {
        return vec![high_digit];
    }
    let mut out = vec![low_digit];
    out.extend(midpoint(low.get(1..).unwrap_or(&[]), None));
    out
}

/// `count` increasing positions from nothing (a fresh, evenly spread sequence).
pub(crate) fn sequence(count: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(count);
    for _ in 0..count {
        let next = key_between(out.last().map(String::as_str), None);
        // Cannot be `None` for a sequence built from nothing; the fallback keeps order anyway.
        out.push(next.unwrap_or_else(|| format!("{}V", out.last().map_or("", String::as_str))));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_first_key_is_in_the_middle() {
        assert_eq!(key_between(None, None).unwrap(), "V");
    }

    #[test]
    fn keys_go_between_after_and_before() {
        let after = key_between(Some("V"), None).unwrap();
        assert!(after.as_str() > "V");
        let before = key_between(None, Some("V")).unwrap();
        assert!(before.as_str() < "V");
        let between = key_between(Some("V"), Some("k")).unwrap();
        assert!(between.as_str() > "V" && between.as_str() < "k");
    }

    #[test]
    fn consecutive_digits_grow_one_digit() {
        let key = key_between(Some("V"), Some("W")).unwrap();
        assert!(key.as_str() > "V" && key.as_str() < "W", "{key}");
        assert_eq!(key.len(), 2);
        let key = key_between(Some("z"), None).unwrap();
        assert!(key.as_str() > "z", "{key}");
        let key = key_between(Some("zz"), None).unwrap();
        assert!(key.as_str() > "zz", "{key}");
    }

    #[test]
    fn a_key_is_never_made_from_equal_or_reversed_bounds_or_foreign_characters() {
        assert_eq!(key_between(Some("V"), Some("V")), None);
        assert_eq!(key_between(Some("k"), Some("V")), None);
        assert_eq!(key_between(Some("V"), Some("V0")), None, "V0 is V");
        assert_eq!(key_between(Some("V!"), None), None);
        assert_eq!(key_between(None, Some("é")), None);
    }

    #[test]
    fn nothing_fits_below_the_smallest_digit_but_keys_never_get_there() {
        assert_eq!(key_between(None, Some("0")), None);
        assert_eq!(key_between(None, Some("1")).unwrap(), "0V");
        let below = key_between(None, Some("0V")).unwrap();
        assert!(below.as_str() < "0V", "{below}");
    }

    #[test]
    fn repeated_appends_prepends_and_splits_keep_the_order() {
        let mut keys: Vec<String> = Vec::new();
        for _ in 0..200 {
            let next = key_between(keys.last().map(String::as_str), None).unwrap();
            keys.push(next);
        }
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        let mut front = vec![key_between(None, None).unwrap()];
        for _ in 0..200 {
            let next = key_between(None, front.first().map(String::as_str)).unwrap();
            front.insert(0, next);
        }
        assert!(front.windows(2).all(|pair| pair[0] < pair[1]));
        // Always splitting the same gap makes keys longer, never wrong.
        let (low, mut high) = (String::from("V"), String::from("W"));
        for _ in 0..100 {
            let mid = key_between(Some(&low), Some(&high)).unwrap();
            assert!(mid > low && mid < high);
            high = mid;
        }
        assert!(
            !high.ends_with('0'),
            "keys never end in the smallest digit: {high}"
        );
    }

    #[test]
    fn a_random_workload_keeps_every_key_strictly_between_its_neighbours() {
        // A small deterministic generator (no dependency): xorshift.
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move |bound: usize| -> usize {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % u64::try_from(bound).unwrap()).unwrap()
        };
        let mut keys: Vec<String> = Vec::new();
        for _ in 0..500 {
            let at = next(keys.len() + 1);
            let before = (at > 0).then(|| keys[at - 1].clone());
            let after = keys.get(at).cloned();
            let key = key_between(before.as_deref(), after.as_deref()).unwrap();
            keys.insert(at, key);
            assert!(
                keys.windows(2).all(|pair| pair[0] < pair[1]),
                "order broke at insert {at}"
            );
            assert!(
                keys.iter()
                    .all(|key| !key.ends_with('0') && !key.is_empty())
            );
        }
    }

    #[test]
    fn a_sequence_is_increasing() {
        let keys = sequence(30);
        assert_eq!(keys.len(), 30);
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(sequence(0).is_empty());
    }
}
