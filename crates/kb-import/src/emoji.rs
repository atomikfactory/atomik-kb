//! What counts as an icon: an emoji (with its joiners, selectors and skin tones, a flag, a keycap)
//! or another non-ASCII symbol. The rule of the paste parser of `core-client` (`isIcon`),
//! without Unicode property tables: the symbol blocks that hold emoji and pictographs.

/// The longest icon, in characters.
const MAX_ICON_CHARS: usize = 16;

/// A character that can start or be part of an icon.
fn is_symbol(c: char) -> bool {
    let code = u32::from(c);
    matches!(
        code,
        0x00A9
            | 0x00AE
            | 0x203C
            | 0x2049
            | 0x2122
            | 0x2139
            | 0x2194..=0x21AA
            | 0x2300..=0x23FF
            | 0x24C2
            | 0x25A0..=0x25FF
            | 0x2600..=0x27BF
            | 0x2934
            | 0x2935
            | 0x2B00..=0x2BFF
            | 0x3030
            | 0x303D
            | 0x3297
            | 0x3299
            | 0x1F000..=0x1FAFF
    )
}

/// A character that can follow a symbol inside one icon: a joiner, a variation selector, a
/// combining keycap, a tag character (subdivision flags).
fn is_modifier(c: char) -> bool {
    matches!(
        u32::from(c),
        0x200D | 0xFE0E | 0xFE0F | 0x20E3 | 0xE0020..=0xE007F
    )
}

/// A regional indicator letter (flags are two of them).
fn is_regional(c: char) -> bool {
    matches!(u32::from(c), 0x1F1E6..=0x1F1FF)
}

/// Whether `word` is an icon: an emoji sequence, a flag, a keycap (`1️⃣`) or another symbol.
/// Never a letter, a digit or ASCII punctuation, so `-`, `**bold**` and `Note:` are not icons.
pub(crate) fn is_icon_word(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if word.chars().count() > MAX_ICON_CHARS {
        return false;
    }
    // Keycaps: `1`, `#` or `*`, an optional variation selector, U+20E3.
    if matches!(first, '0'..='9' | '#' | '*') {
        let rest: Vec<char> = chars.collect();
        return matches!(rest.as_slice(), ['\u{20E3}'] | ['\u{FE0F}', '\u{20E3}']);
    }
    if !(is_symbol(first) || is_regional(first)) {
        return false;
    }
    word.chars()
        .all(|c| is_symbol(c) || is_regional(c) || is_modifier(c))
}

/// The first whitespace-delimited word of `text` when it is an icon, and the text after it with
/// its leading white space removed.
pub(crate) fn split_icon(text: &str) -> Option<(&str, &str)> {
    let trimmed = text.trim_start();
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let word = &trimmed[..end];
    is_icon_word(word).then(|| (word, trimmed[end..].trim_start()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn emoji_are_icons() {
        for icon in [
            "💡",
            "ℹ️",
            "⚠️",
            "❗",
            "🛑",
            "👩\u{200d}💻",
            "🇫🇷",
            "1️⃣",
            "👍🏽",
            "✓",
            "★",
            "🏳️‍🌈",
        ] {
            assert!(is_icon_word(icon), "{icon:?}");
        }
    }

    #[test]
    fn words_and_punctuation_are_not() {
        for word in [
            "",
            "-",
            "**bold**",
            "Note:",
            "note",
            "1",
            "12",
            "é",
            "日本",
            ">",
            "→",
            "#",
            "*",
            "abcdefghijklmnopqrstuvwxyz",
        ] {
            assert!(!is_icon_word(word), "{word:?}");
        }
    }

    #[test]
    fn splits_the_leading_icon() {
        assert_eq!(split_icon("💡 A tip"), Some(("💡", "A tip")));
        assert_eq!(split_icon("  💡   A tip"), Some(("💡", "A tip")));
        assert_eq!(split_icon("💡"), Some(("💡", "")));
        assert_eq!(split_icon("💡A tip"), None);
        assert_eq!(split_icon("A tip"), None);
        assert_eq!(split_icon(""), None);
    }
}
