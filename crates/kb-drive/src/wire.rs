//! Private wire helpers: percent-encoding, form bodies, query parsing, header lookup, the Drive
//! query language's string quoting.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// RFC 3986 unreserved characters stay as they are: letters, digits, `-`, `.`, `_` and `~`.
const ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encodes one query, form or path component.
pub(crate) fn encode(value: &str) -> String {
    utf8_percent_encode(value, ENCODE_SET).to_string()
}

/// `a=1&b=2` with every key and value percent-encoded.
pub(crate) fn form_pairs(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (key, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&encode(key));
        out.push('=');
        out.push_str(&encode(value));
    }
    out
}

/// Decodes one component of a query (`+` is a space, `%XX` a byte). `None` when the bytes are not
/// UTF-8.
fn decode(component: &str) -> Option<String> {
    let spaced = component.replace('+', " ");
    percent_decode_str(&spaced)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// The decoded pairs of a query string (a leading `?` is tolerated). A pair that cannot be decoded
/// is skipped; the same key may appear more than once.
pub(crate) fn parse_query(query: &str) -> Vec<(String, String)> {
    let query = query.strip_prefix('?').unwrap_or(query);
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((decode(key)?, decode(value)?))
        })
        .collect()
}

/// The first value of `key` in decoded pairs.
pub(crate) fn query_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// The value of a response header, whatever the case of its name, trimmed.
pub(crate) fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

/// A string literal of Drive's query language: single quotes around it, `\` and `'` escaped.
pub(crate) fn q_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\\' || c == '\'' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Equality that does not stop at the first differing byte (for the sign-in `state`).
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
