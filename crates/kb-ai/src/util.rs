//! Character-safe text helpers shared by the modules. Every limit of `docs/architecture/ai.md`
//! counts characters (`chars()`), never bytes, and nothing here can cut inside a UTF-8 sequence.

/// Number of characters of `text`.
pub(crate) fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// Whether `text` has more than `limit` characters (stops reading after `limit + 1`).
pub(crate) fn longer_than(text: &str, limit: usize) -> bool {
    text.chars().nth(limit).is_some()
}

/// Byte index of the `n`-th character (0-based), or the length of `text` when it has fewer.
pub(crate) fn byte_index_of_char(text: &str, n: usize) -> usize {
    text.char_indices()
        .nth(n)
        .map_or(text.len(), |(index, _)| index)
}

/// The first `max` characters of `text`.
pub(crate) fn truncate_chars(text: &str, max: usize) -> &str {
    text.get(..byte_index_of_char(text, max)).unwrap_or(text)
}

/// `text` without the prefix `prefix`, compared ignoring ASCII case.
pub(crate) fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        text.get(prefix.len()..)
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_cuts_characters_not_bytes() {
        let text = "aé😀b";
        assert_eq!(char_len(text), 4);
        assert_eq!(truncate_chars(text, 0), "");
        assert_eq!(truncate_chars(text, 2), "aé");
        assert_eq!(truncate_chars(text, 3), "aé😀");
        assert_eq!(truncate_chars(text, 99), text);
        assert_eq!(byte_index_of_char(text, 2), 3);
        assert_eq!(byte_index_of_char(text, 4), text.len());
        assert!(longer_than(text, 3));
        assert!(!longer_than(text, 4));
    }

    #[test]
    fn prefix_is_compared_ignoring_ascii_case() {
        assert_eq!(
            strip_prefix_ignore_case("Transcript: x", "transcript:"),
            Some(" x")
        );
        assert_eq!(strip_prefix_ignore_case("Trans", "transcript:"), None);
        // A multi-byte character straddling the prefix length must not panic.
        assert_eq!(strip_prefix_ignore_case("日本語日本語", "transcript"), None);
    }
}
