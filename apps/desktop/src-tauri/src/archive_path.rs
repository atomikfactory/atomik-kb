//! The rules for a path inside an archive or an import source (ADR-0036,
//! `docs/architecture/import-export.md`, 2.2 and 3.9), as pure functions so a table can test them.
//!
//! A path is **safe** when it is relative, `/`-separated and cannot leave the directory it is
//! resolved against: no leading `/`, no backslash, no drive letter, no NUL or other control
//! character, no empty, `.` or `..` segment, and at most [`MAX_PATH_BYTES`] bytes. The export writer
//! refuses to write an entry whose path is not safe (the core produces safe ones; this is the
//! second line), and the import readers skip every entry that is not (the "zip slip" of a crafted
//! archive) before it reaches the core, which refuses such paths as well.

/// The longest path the core accepts for a source file.
pub const MAX_PATH_BYTES: usize = 1024;

/// Folders the import never reads: macOS resource forks and package folders.
const SKIPPED_SEGMENTS: [&str; 2] = ["__MACOSX", "node_modules"];

/// Why a path cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathProblem {
    /// Absolute, escaping, or holding a character no portable path may hold.
    Unsafe,
    /// Longer than [`MAX_PATH_BYTES`].
    TooLong,
}

/// Whether `path` is a safe relative path (see the module documentation).
#[must_use]
pub fn is_safe_relative_path(path: &str) -> bool {
    path_problem(path).is_none()
}

/// What is wrong with `path`, or `None` when it is a safe relative path.
#[must_use]
pub fn path_problem(path: &str) -> Option<PathProblem> {
    if path.is_empty() || path.starts_with('/') {
        return Some(PathProblem::Unsafe);
    }
    if path.chars().any(|c| c.is_control() || c == '\\') {
        return Some(PathProblem::Unsafe);
    }
    // A drive letter (`C:/x`, `C:x`): the first segment, which would leave the root on Windows.
    let mut chars = path.chars();
    if let (Some(first), Some(':')) = (chars.next(), chars.next())
        && first.is_ascii_alphabetic()
    {
        return Some(PathProblem::Unsafe);
    }
    if path
        .split('/')
        .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Some(PathProblem::Unsafe);
    }
    if path.len() > MAX_PATH_BYTES {
        return Some(PathProblem::TooLong);
    }
    None
}

/// Whether the import leaves `path` out without a word: a hidden entry (a segment starting with
/// `.`), `__MACOSX` or `node_modules` anywhere in it.
#[must_use]
pub fn is_skipped(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment.starts_with('.') || SKIPPED_SEGMENTS.contains(&segment))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_relative_paths_are_safe() {
        for path in [
            "a.md",
            "Folder/Page.md",
            "Folder with spaces/Sub folder/Page (2).md",
            "日本語/ページ.md",
            "a/b/c/d/e.png",
            "a/b:c.md",
            "not-a-drive:/x",
        ] {
            assert_eq!(path_problem(path), None, "{path}");
        }
    }

    #[test]
    fn paths_that_escape_or_cannot_be_portable_are_unsafe() {
        for path in [
            "",
            "/etc/passwd",
            "../evil.md",
            "a/../../evil.md",
            "a/./b.md",
            "a//b.md",
            "a/b/",
            "./a.md",
            "..",
            "a\\b.md",
            "C:\\Windows\\x",
            "C:/Windows/x",
            "c:x",
            "a/b\0.md",
            "a/\u{7}b.md",
            "a/\nb.md",
        ] {
            assert_eq!(path_problem(path), Some(PathProblem::Unsafe), "{path:?}");
        }
    }

    #[test]
    fn a_path_over_the_core_limit_is_too_long() {
        let longest = format!("{}.md", "a".repeat(MAX_PATH_BYTES - 3));
        assert_eq!(longest.len(), MAX_PATH_BYTES);
        assert_eq!(path_problem(&longest), None);
        let too_long = format!("{longest}x");
        assert_eq!(path_problem(&too_long), Some(PathProblem::TooLong));
        assert!(!is_safe_relative_path(&too_long));
    }

    #[test]
    fn hidden_and_package_entries_are_skipped() {
        for path in [
            ".git/config",
            "notes/.DS_Store",
            ".hidden.md",
            "__MACOSX/._a.md",
            "a/__MACOSX/b",
            "node_modules/x/index.js",
            "a/node_modules/b.md",
        ] {
            assert!(is_skipped(path), "{path}");
        }
        for path in [
            "a.md",
            "notes/page.md",
            "a/b.c.md",
            "node_modules.md",
            "my__MACOSX/a",
        ] {
            assert!(!is_skipped(path), "{path}");
        }
    }
}
