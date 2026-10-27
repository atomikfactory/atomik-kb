//! AppFlowy's text: a Quill-style delta (`[{insert, attributes?}]`) into inline nodes of the page
//! shape (ADR-0016): text with marks, page references, date mentions, inline equations, links.

use kb_types::ImportWarningCode;
use serde_json::{Map, Value};

use super::archive::{array_of, flag, int, object_of, text_of};
use super::colors;
use super::env::{Env, is_linkable};
use crate::json::{self, MarkSet, attrs, element};
use crate::placeholder::{self, Target};
use crate::values;

/// The text of a block's delta as an array of operations: the delta is an array, or a string that
/// holds one.
pub(super) fn ops(delta: &Value) -> Option<Vec<Value>> {
    array_of(delta)
}

/// The string inserts of a delta put together (code blocks, equations, plain-text contexts).
pub(super) fn plain_text(ops: &[Value]) -> String {
    ops.iter()
        .filter_map(|op| op.get("insert").and_then(Value::as_str))
        .collect()
}

/// The inline nodes of a delta. A newline-only insert at the end is dropped; a newline inside a
/// text is a hard break.
pub(super) fn nodes(env: &mut Env, path: &str, ops: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        let Some(op) = op.as_object() else {
            continue;
        };
        let Some(text) = op.get("insert").and_then(Value::as_str) else {
            // An embed object: nothing to show.
            continue;
        };
        if index + 1 == ops.len() && text.chars().all(|c| c == '\n' || c == '\r') {
            continue;
        }
        let attributes = op.get("attributes").and_then(Value::as_object);
        run(env, path, text, attributes, &mut out);
    }
    out
}

/// What a run of text with attributes becomes.
fn run(
    env: &mut Env,
    path: &str,
    text: &str,
    attributes: Option<&Map<String, Value>>,
    out: &mut Vec<Value>,
) {
    let empty = Map::new();
    let a = attributes.unwrap_or(&empty);
    let mut marks = MarkSet {
        bold: flag(a.get("bold")),
        italic: flag(a.get("italic")),
        underline: flag(a.get("underline")),
        strike: flag(a.get("strikethrough")),
        code: flag(a.get("code")),
        ..MarkSet::default()
    };
    if let Some(color) = a
        .get("font_color")
        .and_then(Value::as_str)
        .and_then(|c| colors::palette(c, false))
    {
        marks.text_color = Some(color.to_owned());
    }
    if let Some(color) = a
        .get("bg_color")
        .and_then(Value::as_str)
        .and_then(|c| colors::palette(c, true))
    {
        marks.highlight = Some(color.to_owned());
    }

    // A citation of an AI summary (`^` with `{number, blockIds}`): its number in brackets.
    if let Some(reference) = a.get("reference") {
        if let Some(number) = reference
            .as_object()
            .and_then(|r| text_of(r.get("number")))
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty() && n.chars().count() <= 10)
        {
            push_text(out, &format!("[{number}]"), &marks);
        }
        return;
    }
    // An inline equation: the insert is a placeholder, the LaTeX is the attribute.
    if let Some(formula) = a.get("formula") {
        let latex = match formula {
            Value::String(latex) => latex.clone(),
            _ => text.to_owned(),
        };
        out.push(math_inline(&latex));
        return;
    }
    if let Some(mention) = a.get("mention").and_then(object_of) {
        mention_run(env, path, text, &mention, &marks, out);
        return;
    }
    if let Some(href) = a.get("href").and_then(Value::as_str) {
        let href = href.trim();
        let is_page = flag(a.get("is_page_link"));
        let page = if is_page || !is_linkable(href) {
            env.page(href)
        } else {
            None
        };
        if let Some(index) = page {
            out.push(page_ref(env, index));
            return;
        }
        if is_page {
            unresolved_page(env, path, href, text, out);
            return;
        }
        if is_linkable(href) {
            marks.link = Some(href.to_owned());
        } else if let Some(url) = bare_domain(href) {
            marks.link = Some(url);
        } else if !href.is_empty() {
            let scheme = href
                .split_once(':')
                .map_or("link", |(scheme, _)| scheme)
                .chars()
                .take(20)
                .collect::<String>();
            env.warn(path, ImportWarningCode::UnresolvedLink, Some(&scheme));
        }
    }
    push_text(out, text, &marks);
}

/// A web address written without its scheme (`example.com/page`) as an `https` URL.
fn bare_domain(href: &str) -> Option<String> {
    let first = href.chars().next()?;
    if !first.is_alphanumeric() || href.contains(':') || !href.contains('.') {
        return None;
    }
    let url = format!("https://{href}");
    is_linkable(&url).then_some(url)
}

/// A mention: a page, a date, an external link, a person.
fn mention_run(
    env: &mut Env,
    path: &str,
    text: &str,
    mention: &Map<String, Value>,
    marks: &MarkSet,
    out: &mut Vec<Value>,
) {
    match text_of(mention.get("type")).as_deref() {
        Some("page") => {
            let id = text_of(mention.get("page_id")).unwrap_or_default();
            match env.page(&id) {
                Some(index) => out.push(page_ref(env, index)),
                None => unresolved_page(env, path, &id, text, out),
            }
        }
        Some("date") => {
            let include_time = flag(mention.get("include_time"));
            match mention.get("date").and_then(|d| date_of(d, include_time)) {
                Some((date, time)) => {
                    let mut map = attrs([("date", Value::String(date))]);
                    if let Some(time) = time {
                        map.insert("time".into(), Value::String(time));
                    }
                    out.push(element("dateMention", map, Vec::new()));
                }
                None => push_text(out, text_or(text, "Date"), marks),
            }
        }
        Some("externalLink") => {
            let url = text_of(mention.get("url")).unwrap_or_default();
            let shown = text_or(text, &url);
            let mut marks = marks.clone();
            if is_linkable(&url) {
                marks.link = Some(url.trim().to_owned());
            }
            push_text(out, shown, &marks);
        }
        Some("person") => {
            let name = text_of(mention.get("person_name")).unwrap_or_default();
            let name = name.trim().trim_start_matches('@');
            let shown = if name.is_empty() {
                "@person".to_owned()
            } else {
                format!("@{name}")
            };
            push_text(out, &shown, marks);
        }
        _ => push_text(out, text_or(text, "Mention"), marks),
    }
}

/// The text of a mention's insert; AppFlowy writes a placeholder character there.
fn text_or<'a>(text: &'a str, fallback: &'a str) -> &'a str {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "$" || trimmed == "@" {
        fallback
    } else {
        text
    }
}

/// A reference to a planned page; its title is the label shown if the target is missing later.
fn page_ref(env: &Env, index: usize) -> Value {
    placeholder::page_ref(&Target::Planned(index), env.title(index))
}

/// A page that is not in the plan: its text (when the mention has any), and a warning.
fn unresolved_page(env: &mut Env, path: &str, id: &str, text: &str, out: &mut Vec<Value>) {
    env.warn(path, ImportWarningCode::UnresolvedLink, Some(id));
    push_text(out, text_or(text, "Unknown page"), &MarkSet::default());
}

/// Text with marks; a newline is a hard break.
pub(super) fn push_text(out: &mut Vec<Value>, text: &str, marks: &MarkSet) {
    for (index, part) in text.split('\n').enumerate() {
        if index > 0 {
            out.push(json::hard_break());
        }
        let part = part.trim_end_matches('\r');
        if !part.is_empty() {
            out.push(json::text_node(part, marks));
        }
    }
}

/// An inline equation.
pub(super) fn math_inline(latex: &str) -> Value {
    element(
        "mathInline",
        attrs([("latex", Value::String(latex.to_owned()))]),
        Vec::new(),
    )
}

/// The day (and time) of a date mention: an ISO string, or a timestamp (milliseconds, or seconds
/// below 10^11).
fn date_of(value: &Value, include_time: bool) -> Option<(String, Option<String>)> {
    let from_stamp = |stamp: i64| {
        let seconds = if stamp.abs() >= 100_000_000_000 {
            stamp / 1000
        } else {
            stamp
        };
        civil(seconds, include_time)
    };
    let (date, time) = match value {
        Value::String(text) => {
            let text = text.trim();
            if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
                from_stamp(text.parse().ok()?)?
            } else {
                values::parse_date(text)?
            }
        }
        Value::Number(_) => from_stamp(int(Some(value))?)?,
        _ => return None,
    };
    Some((date, time.filter(|_| include_time)))
}

/// The UTC day and time of a timestamp in seconds. A day without a time that falls in the second
/// half of a UTC day is the next day: AppFlowy stores the local midnight, which east of Greenwich
/// is the evening before.
pub(super) fn civil(seconds: i64, with_time: bool) -> Option<(String, Option<String>)> {
    const DAY: i64 = 86_400;
    let mut seconds = seconds;
    if !with_time && seconds.rem_euclid(DAY) >= DAY / 2 {
        seconds = seconds.checked_add(DAY)?;
    }
    let days = seconds.div_euclid(DAY);
    let in_day = seconds.rem_euclid(DAY);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    if !(1..=9999).contains(&year) {
        return None;
    }
    let date = format!("{year:04}-{month:02}-{day:02}");
    let time = with_time.then(|| format!("{:02}:{:02}", in_day / 3600, in_day % 3600 / 60));
    Some((date, time))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_to_days() {
        assert_eq!(
            civil(0, true),
            Some(("1970-01-01".to_owned(), Some("00:00".to_owned())))
        );
        // 2026-10-07 14:30:00 UTC.
        assert_eq!(
            civil(1_791_383_400, true),
            Some(("2026-10-07".to_owned(), Some("14:30".to_owned())))
        );
        // A local midnight east of Greenwich (22:00 UTC the evening before) is the next day.
        assert_eq!(
            civil(1_791_324_000, false),
            Some(("2026-10-07".to_owned(), None))
        );
        // A local midnight west of Greenwich (05:00 UTC) is that day.
        assert_eq!(
            civil(1_791_349_200, false),
            Some(("2026-10-07".to_owned(), None))
        );
        assert_eq!(civil(951_782_400, true).unwrap().0, "2000-02-29");
        assert_eq!(civil(i64::MAX, true), None);
    }

    #[test]
    fn dates_of_mentions() {
        assert_eq!(
            date_of(&Value::from("2026-10-07T14:30:00.000Z"), true),
            Some(("2026-10-07".to_owned(), Some("14:30".to_owned())))
        );
        assert_eq!(
            date_of(&Value::from("2026-10-07T14:30:00.000Z"), false),
            Some(("2026-10-07".to_owned(), None))
        );
        // Milliseconds, as a number and as a string.
        assert_eq!(
            date_of(&Value::from(1_791_383_400_000_i64), true),
            Some(("2026-10-07".to_owned(), Some("14:30".to_owned())))
        );
        assert_eq!(
            date_of(&Value::from("1791383400000"), true),
            Some(("2026-10-07".to_owned(), Some("14:30".to_owned())))
        );
        assert_eq!(date_of(&Value::from("soon"), true), None);
        assert_eq!(date_of(&Value::Null, true), None);
    }
}
