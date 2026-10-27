//! Paths and names of the source: safety, folders, relative links, display names and the Notion
//! ids in them.

use std::cmp::Ordering;

use percent_encoding::percent_decode_str;

/// The longest path accepted, in bytes (the same limit as the core's).
pub(crate) const MAX_PATH_BYTES: usize = 1024;

/// Whether a path of the source is acceptable: relative, `/`-separated, no empty, `.` or `..`
/// segment, no backslash, no drive letter, no control character, at most 1,024 bytes.
pub(crate) fn is_safe_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.contains('\\') {
        return false;
    }
    if path.chars().any(char::is_control) {
        return false;
    }
    let mut segments = path.split('/').peekable();
    if let Some(first) = segments.peek() {
        let bytes = first.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return false;
        }
    }
    path.split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// Whether a path goes through a hidden folder or file (a segment starting with `.`), a macOS
/// `__MACOSX` folder or a `node_modules` folder: the shell skips those, and so does `plan`.
pub(crate) fn is_hidden(path: &str) -> bool {
    path.split('/').any(|segment| {
        segment.starts_with('.') || segment == "__MACOSX" || segment == "node_modules"
    })
}

/// The folder part of a path (`""` at the top level).
pub(crate) fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The last segment of a path.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// `dir` and `name` joined (`dir` may be empty).
pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// A file name split into its stem and its lowercase extension (without the dot). A name with a
/// leading dot only, or without a dot, has no extension.
pub(crate) fn split_ext(name: &str) -> (&str, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, ext.to_lowercase()),
        _ => (name, String::new()),
    }
}

/// The extension of a path's last segment, lowercase.
pub(crate) fn ext_of(path: &str) -> String {
    split_ext(file_name(path)).1
}

/// Percent-decodes a link destination (invalid UTF-8 becomes replacement characters).
pub(crate) fn decode(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().into_owned()
}

/// Resolves a link destination against the folder of the file holding it: the destination is cut
/// at `#` and `?`, percent-decoded, backslashes become `/`, and `.` and `..` segments are
/// applied. A destination starting with `/` is relative to the source root. `None` when it
/// climbs above the root or names nothing.
pub(crate) fn resolve_relative(base_dir: &str, dest: &str) -> Option<String> {
    let cut = dest.find(['#', '?']).unwrap_or(dest.len());
    let dest = decode(&dest[..cut]).replace('\\', "/");
    if dest.trim().is_empty() {
        return None;
    }
    let mut segments: Vec<&str> = if dest.starts_with('/') {
        Vec::new()
    } else {
        base_dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for segment in dest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(segments.join("/"))
}

/// Resolves a path written relative to a folder, as it is (no percent-decoding, no cutting at `#`
/// or `?`): `.` and `..` segments are applied. `None` when it climbs above the root or is empty.
pub(crate) fn resolve_plain(base_dir: &str, path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let mut segments: Vec<&str> = if path.starts_with('/') {
        Vec::new()
    } else {
        base_dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(segments.join("/"))
}

/// The Notion id ending a stem (` ` and 32 lowercase hex digits): the stem without it, and the id.
pub(crate) fn split_notion_id(stem: &str) -> Option<(&str, &str)> {
    let (base, id) = stem.rsplit_once(' ')?;
    is_notion_id(id).then_some((base, id))
}

/// 32 lowercase hexadecimal digits.
pub(crate) fn is_notion_id(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The Notion id at the end of a file name or URL segment: after the last `-` or ` `, or the whole
/// segment, in lower case.
pub(crate) fn notion_id_in(segment: &str) -> Option<String> {
    let segment = segment.trim().to_ascii_lowercase();
    if is_notion_id(&segment) {
        return Some(segment);
    }
    let tail = segment.rsplit(['-', ' ']).next()?;
    is_notion_id(tail).then(|| tail.to_owned())
}

/// A name for people: the stem with its Notion id removed and trimmed; `Untitled` when nothing is
/// left. `strip_all` removes a trailing `_all` first (a Notion database's complete CSV).
pub(crate) fn clean_name(stem: &str, strip_all: bool) -> String {
    let stem = if strip_all {
        stem.strip_suffix("_all").unwrap_or(stem)
    } else {
        stem
    };
    let base = split_notion_id(stem).map_or(stem, |(base, _)| base);
    let name = base.trim();
    if name.is_empty() {
        "Untitled".to_owned()
    } else {
        name.to_owned()
    }
}

/// Natural order of names: case-insensitive, runs of digits compared as numbers.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().flat_map(char::to_lowercase).peekable();
    let mut right = b.chars().flat_map(char::to_lowercase).peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut run_x = String::new();
                while let Some(c) = left.next_if(char::is_ascii_digit) {
                    run_x.push(c);
                }
                let mut run_y = String::new();
                while let Some(c) = right.next_if(char::is_ascii_digit) {
                    run_y.push(c);
                }
                let (tx, ty) = (run_x.trim_start_matches('0'), run_y.trim_start_matches('0'));
                let order = tx
                    .len()
                    .cmp(&ty.len())
                    .then_with(|| tx.cmp(ty))
                    .then_with(|| run_x.len().cmp(&run_y.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                left.next();
                right.next();
            }
        }
    }
}

/// `text` cut to at most `max` characters.
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn unsafe_paths_are_refused() {
        for path in [
            "",
            "/a.md",
            "../a.md",
            "a/../b.md",
            "a//b.md",
            "./a.md",
            "a/./b.md",
            "a\\b.md",
            "C:/a.md",
            "c:a.md",
            "a/\u{0}b.md",
            "a/\nb.md",
        ] {
            assert!(!is_safe_path(path), "{path:?}");
        }
        assert!(!is_safe_path(&"a/".repeat(600)));
        for path in [
            "a.md",
            "a/b.md",
            "My Notes/Page 1.md",
            "日本/ページ.md",
            "a/b:c.md",
        ] {
            assert!(is_safe_path(path), "{path:?}");
        }
    }

    #[test]
    fn hidden_paths() {
        for hidden in [
            ".hidden.md",
            "a/.obsidian/x.md",
            "__MACOSX/a.md",
            "a/node_modules/b.md",
            "a/.b",
        ] {
            assert!(is_hidden(hidden), "{hidden}");
        }
        for shown in [
            "a.md",
            "a/b.md",
            "a/b.c.md",
            "my.notes/a.md",
            "node_modules.md",
        ] {
            assert!(!is_hidden(shown), "{shown}");
        }
    }

    #[test]
    fn folders_names_and_extensions() {
        assert_eq!(dir_of("a/b/c.md"), "a/b");
        assert_eq!(dir_of("c.md"), "");
        assert_eq!(file_name("a/b/c.md"), "c.md");
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("a", "x"), "a/x");
        assert_eq!(split_ext("Notes.MD"), ("Notes", "md".to_owned()));
        assert_eq!(
            split_ext("archive.tar.gz"),
            ("archive.tar", "gz".to_owned())
        );
        assert_eq!(split_ext(".hidden"), (".hidden", String::new()));
        assert_eq!(split_ext("noext"), ("noext", String::new()));
        assert_eq!(ext_of("a/b.PNG"), "png");
    }

    #[test]
    fn relative_links_resolve_and_cannot_escape() {
        assert_eq!(resolve_relative("a/b", "c.md").as_deref(), Some("a/b/c.md"));
        assert_eq!(
            resolve_relative("a/b", "../c%20d.md#top").as_deref(),
            Some("a/c d.md")
        );
        assert_eq!(
            resolve_relative("a", "./x/../y.md").as_deref(),
            Some("a/y.md")
        );
        assert_eq!(
            resolve_relative("a", "/root.md").as_deref(),
            Some("root.md")
        );
        assert_eq!(resolve_relative("a", "..\\b.md").as_deref(), Some("b.md"));
        assert_eq!(resolve_relative("", "../x.md"), None);
        assert_eq!(resolve_relative("a", "../../x.md"), None);
        assert_eq!(resolve_relative("a", "#only-anchor"), None);
        assert_eq!(resolve_relative("a", ""), None);
        assert_eq!(resolve_relative("", "a%2Fb.md").as_deref(), Some("a/b.md"));
    }

    #[test]
    fn notion_ids() {
        let stem = format!("My Page {ID}");
        assert_eq!(split_notion_id(&stem), Some(("My Page", ID)));
        assert_eq!(clean_name(&stem, false), "My Page");
        assert_eq!(clean_name(&format!("Tasks {ID}_all"), true), "Tasks");
        assert_eq!(clean_name(&format!("Tasks {ID}"), true), "Tasks");
        assert_eq!(clean_name(&format!("Untitled {ID}"), false), "Untitled");
        assert_eq!(clean_name(&format!(" {ID}"), false), "Untitled");
        assert_eq!(clean_name("", false), "Untitled");
        assert_eq!(clean_name("Plain name", false), "Plain name");
        // Uppercase, short and unspaced ids are not Notion's.
        assert_eq!(split_notion_id(&format!("A {}", ID.to_uppercase())), None);
        assert_eq!(split_notion_id("A 0123456789abcdef"), None);
        assert_eq!(split_notion_id(&format!("A{ID}")), None);
        assert_eq!(notion_id_in(&format!("Title-{ID}")).as_deref(), Some(ID));
        assert_eq!(notion_id_in(ID).as_deref(), Some(ID));
        assert_eq!(
            notion_id_in(&format!("Some Title {ID}")).as_deref(),
            Some(ID)
        );
        assert_eq!(notion_id_in("Title-123"), None);
    }

    #[test]
    fn natural_order() {
        let mut names = vec![
            "Chapter 10",
            "chapter 2",
            "Chapter 1",
            "Appendix",
            "chapter 02a",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            [
                "Appendix",
                "Chapter 1",
                "chapter 2",
                "chapter 02a",
                "Chapter 10"
            ]
        );
        assert_eq!(natural_cmp("a", "A"), Ordering::Equal);
        assert_eq!(natural_cmp("a1", "a01"), Ordering::Less);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
    }

    #[test]
    fn truncation_is_on_character_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("abc", 10), "abc");
        assert_eq!(truncate_chars("日本語", 2), "日本");
    }
}
