//! Regular expressions (`formulas.md` section 3.4.1): the subset JavaScript and the `regex` crate
//! share, with JavaScript's meaning. The pattern is rewritten before it reaches the crate: `\d`,
//! `\w`, `\b` (and their negations) are ASCII, `.` does not match the four line terminators,
//! `[]` and `[^]` are the empty and the full class, and the crate's own extensions (flags, set
//! operations in classes) are refused or escaped.

use regex::{Regex, RegexBuilder};

/// The most memory a compiled pattern may take.
const SIZE_LIMIT: usize = 1 << 20;

/// Compiles a pattern; `None` when it is not in the shared subset or does not compile.
pub(super) fn compile(pattern: &str) -> Option<Regex> {
    let translated = translate(pattern)?;
    RegexBuilder::new(&translated)
        .size_limit(SIZE_LIMIT)
        .build()
        .ok()
}

/// The pattern in the crate's syntax with JavaScript's meaning.
fn translate(pattern: &str) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut in_class = false;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '\\' => {
                let next = *chars.get(i + 1)?;
                escape(next, in_class, &mut out)?;
                i += 2;
                continue;
            }
            '[' if in_class => out.push_str("\\["),
            '[' => {
                in_class = true;
                let negated = chars.get(i + 1) == Some(&'^');
                let body = i + 1 + usize::from(negated);
                if chars.get(body) == Some(&']') {
                    // JavaScript's `[]` matches nothing and `[^]` anything.
                    out.push_str(if negated { "[\\s\\S]" } else { "[^\\s\\S]" });
                    in_class = false;
                    i = body + 1;
                    continue;
                }
                out.push('[');
                if negated {
                    out.push('^');
                }
                i = body;
                continue;
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '.' if !in_class => out.push_str("[^\\n\\r\\x{2028}\\x{2029}]"),
            '&' | '~' if in_class => {
                out.push('\\');
                out.push(c);
            }
            '-' if in_class && chars.get(i + 1) == Some(&'-') => out.push_str("\\-"),
            '(' if !in_class => {
                if chars.get(i + 1) == Some(&'?') {
                    // Only `(?:` is in the subset: no flags, look-around or named groups.
                    if chars.get(i + 2) != Some(&':') {
                        return None;
                    }
                    out.push_str("(?:");
                    i += 3;
                    continue;
                }
                out.push('(');
            }
            other => out.push(other),
        }
        i += 1;
    }
    if in_class {
        return None;
    }
    Some(out)
}

fn escape(next: char, in_class: bool, out: &mut String) -> Option<()> {
    match next {
        'd' => out.push_str(if in_class { "0-9" } else { "[0-9]" }),
        'D' => out.push_str("[^0-9]"),
        'w' => out.push_str(if in_class {
            "0-9A-Za-z_"
        } else {
            "[0-9A-Za-z_]"
        }),
        'W' => out.push_str("[^0-9A-Za-z_]"),
        'b' if in_class => out.push_str("\\x08"),
        'b' => out.push_str("(?-u:\\b)"),
        'B' if in_class => return None,
        'B' => out.push_str("(?-u:\\B)"),
        's' | 'S' | 'n' | 'r' | 't' | 'f' | 'v' => {
            out.push('\\');
            out.push(next);
        }
        // Escapes of punctuation (including `/` and `-`); a letter or digit that is not one of the
        // above is not in the subset.
        other if other.is_ascii_punctuation() => {
            out.push('\\');
            out.push(other);
        }
        _ => return None,
    }
    Some(())
}

/// Every match of `re` in `text`, left to right, the way JavaScript's global matching finds them: after
/// a match the next search starts where it ended (an empty match is allowed right after another match),
/// and after an empty match it starts one character on. At most `limit` matches.
pub(super) fn all_captures<'t>(
    re: &Regex,
    text: &'t str,
    limit: usize,
) -> Vec<regex::Captures<'t>> {
    let mut found = Vec::new();
    let mut at = 0;
    while at <= text.len() && found.len() < limit {
        let Some(captures) = re.captures_at(text, at) else {
            break;
        };
        let Some(whole) = captures.get(0) else {
            break;
        };
        at = if whole.is_empty() {
            // One character past the empty match.
            text.get(whole.end()..)
                .and_then(|rest| rest.chars().next())
                .map_or(text.len() + 1, |c| whole.end() + c.len_utf8())
        } else {
            whole.end()
        };
        found.push(captures);
    }
    found
}

/// `replacement` with `$0`..`$9` (the groups; an unmatched or missing one is empty) and `$$`
/// expanded; any other `$` is literal.
pub(super) fn expand(replacement: &str, captures: &regex::Captures<'_>, out: &mut String) {
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some(digit) if digit.is_ascii_digit() => {
                chars.next();
                let index = usize::try_from(u32::from(digit) - u32::from('0')).unwrap_or(0);
                if let Some(group) = captures.get(index) {
                    out.push_str(group.as_str());
                }
            }
            _ => out.push('$'),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn matches(pattern: &str, text: &str) -> Vec<String> {
        let re = compile(pattern).unwrap();
        all_captures(&re, text, usize::MAX)
            .iter()
            .map(|captures| {
                captures
                    .get(0)
                    .map_or(String::new(), |m| m.as_str().to_owned())
            })
            .collect()
    }

    #[test]
    fn digits_words_and_boundaries_are_ascii() {
        assert_eq!(matches("\\d+", "a1b22c٣"), ["1", "22"]);
        assert_eq!(matches("\\w+", "héllo wörld"), ["h", "llo", "w", "rld"]);
        assert_eq!(matches("\\bfoo\\b", "foo foobar éfoo"), ["foo", "foo"]);
        assert_eq!(matches("\\D+", "a1é"), ["a", "é"]);
        assert_eq!(matches("\\W", "aé b"), ["é", " "]);
        assert_eq!(matches("[\\d.]+", "1.5x2"), ["1.5", "2"]);
        assert_eq!(matches("[\\w-]+", "a-b c"), ["a-b", "c"]);
        assert_eq!(matches("[^\\d]+", "a1b"), ["a", "b"]);
        assert_eq!(matches("\\B.", "ab"), ["b"]);
    }

    #[test]
    fn the_dot_stops_at_line_terminators() {
        assert_eq!(
            matches(".+", "ab\rcd\nef\u{2028}gh"),
            ["ab", "cd", "ef", "gh"]
        );
        assert_eq!(matches("a.c", "a\u{e9}c"), ["a\u{e9}c"]);
    }

    #[test]
    fn classes_follow_javascript() {
        assert!(matches("[]", "abc").is_empty());
        assert_eq!(matches("[^]", "ab"), ["a", "b"]);
        assert_eq!(matches("[a&&b]+", "a&b"), ["a&b"]);
        assert_eq!(matches("[[]", "a[b"), ["["]);
        assert_eq!(matches("[a-c]+", "abcd"), ["abc"]);
        assert_eq!(matches("[\\]]", "a]"), ["]"]);
    }

    #[test]
    fn the_unsupported_is_refused() {
        for pattern in [
            "(?i)a", "(?=a)b", "(?<n>a)", "(?P<n>a)", "\\1", "\\k", "(", "[a", "a)", "\\q", "a{",
            "*",
        ] {
            assert!(compile(pattern).is_none(), "{pattern}");
        }
        assert!(compile("(?:a|b)+").is_some());
        assert!(compile("\\.\\/\\-\\+").is_some());
        assert!(compile("(a+)+$").is_some());
    }

    #[test]
    fn matching_is_leftmost_first_with_javascript_empty_matches() {
        assert_eq!(matches("a|ab", "ab"), ["a"]);
        assert_eq!(matches("a*", "baac"), ["", "aa", "", ""]);
        assert_eq!(matches("^", "abc"), [""]);
        assert_eq!(matches("a*?", "aa"), ["", "", ""]);
        assert_eq!(matches("x*", "é"), ["", ""]);
    }

    #[test]
    fn replacements_expand_groups() {
        let expand_with = |pattern: &str, text: &str, replacement: &str| {
            let re = compile(pattern).unwrap();
            let captures = re.captures(text).unwrap();
            let mut out = String::new();
            expand(replacement, &captures, &mut out);
            out
        };
        assert_eq!(expand_with("(a)(b)?", "a", "[$0|$1|$2|$3]"), "[a|a||]");
        assert_eq!(expand_with("(a)", "a", "$$1 $a $ $"), "$1 $a $ $");
        assert_eq!(expand_with("(a)", "a", "$1$1"), "aa");
    }

    #[test]
    fn a_huge_pattern_is_refused() {
        assert!(compile("(((a{100}){100}){100}){100}").is_none());
    }
}
