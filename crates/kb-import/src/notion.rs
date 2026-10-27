//! Notion's "Markdown & CSV" export: the property lines at the top of a database row's page.

use std::borrow::Cow;
use std::collections::HashSet;

/// Removes the paragraph of `Key: Value` lines that follows the title of a database row's page,
/// when every line of it names a column of the database (`columns`, lowercase). The values are in
/// the CSV; the page keeps its content. Anything else is returned as it is.
pub(crate) fn strip_property_lines<'a>(body: &'a str, columns: &HashSet<String>) -> Cow<'a, str> {
    if columns.is_empty() {
        return Cow::Borrowed(body);
    }
    let lines: Vec<&str> = body.split('\n').collect();
    let blank = |line: &str| line.trim().is_empty();
    let mut at = 0;
    while lines.get(at).is_some_and(|line| blank(line)) {
        at += 1;
    }
    // The title heading.
    if lines
        .get(at)
        .is_some_and(|line| line.starts_with("# ") || line.trim_end() == "#")
    {
        at += 1;
        while lines.get(at).is_some_and(|line| blank(line)) {
            at += 1;
        }
    }
    let start = at;
    while lines.get(at).is_some_and(|line| !blank(line)) {
        at += 1;
    }
    if start == at {
        return Cow::Borrowed(body);
    }
    let all_properties = lines[start..at].iter().all(|line| {
        line.split_once(':')
            .is_some_and(|(key, _)| columns.contains(&key.trim().to_lowercase()))
    });
    if !all_properties {
        return Cow::Borrowed(body);
    }
    let mut kept: Vec<&str> = lines[..start].to_vec();
    kept.extend_from_slice(&lines[at..]);
    Cow::Owned(kept.join("\n"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn columns(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_lowercase()).collect()
    }

    #[test]
    fn the_property_paragraph_is_removed() {
        let body = "# Task\n\nStatus: In progress\nDue: October 7, 2026\nRelated: [A](A.md)\n\nBody text\n";
        let got = strip_property_lines(body, &columns(&["Status", "Due", "Related"]));
        assert_eq!(got, "# Task\n\n\nBody text\n");
    }

    #[test]
    fn other_paragraphs_stay() {
        let columns = columns(&["Status"]);
        // A line that is not a column.
        let body = "# T\n\nStatus: x\nNote: y\n\nBody";
        assert_eq!(strip_property_lines(body, &columns), body);
        // Not a `Key: Value` line.
        let body = "# T\n\nJust a paragraph.\n";
        assert_eq!(strip_property_lines(body, &columns), body);
        // No columns known.
        assert_eq!(
            strip_property_lines("# T\n\nStatus: x\n", &HashSet::new()),
            "# T\n\nStatus: x\n"
        );
        // Nothing after the title.
        assert_eq!(strip_property_lines("# T\n", &columns), "# T\n");
        assert_eq!(strip_property_lines("", &columns), "");
    }

    #[test]
    fn without_a_title_the_first_paragraph_counts() {
        let got = strip_property_lines("Status: x\n\nBody", &columns(&["status"]));
        assert_eq!(got, "\nBody");
        let got = strip_property_lines(
            "Status:\r\nDue: y\r\n\r\nBody",
            &columns(&["status", "due"]),
        );
        assert_eq!(got, "\r\nBody");
    }
}
