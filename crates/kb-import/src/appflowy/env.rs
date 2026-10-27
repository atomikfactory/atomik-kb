//! What the converters share while they read the whole archive: the planned pages their links point
//! to, the files they need, the remote files, and the warnings.

use std::collections::{HashMap, HashSet};

use kb_types::ImportWarningCode;

use crate::RemoteFile;
use crate::ctx::Warnings;
use crate::paths::{self, truncate_chars};

/// The longest link kept as a link, in bytes (the schema's limit for a URL is 2,048 characters).
const MAX_URL_BYTES: usize = 2_048;

/// Shared state of the conversion.
#[derive(Default)]
pub(super) struct Env {
    /// The plan's warnings.
    pub warnings: Warnings,
    /// View id to plan index.
    pub views: HashMap<String, usize>,
    /// Database id to the plan index of the page that holds its rows.
    pub databases: HashMap<String, usize>,
    /// The plan indexes of the pages of databases (the holders and the linked views).
    pub db_pages: HashSet<usize>,
    /// Row id and row document id to the plan index of the row's page.
    pub rows: HashMap<String, usize>,
    /// The title of each planned page (for the label of a link that cannot be resolved).
    pub titles: Vec<String>,
    /// `local_files` names to the binary path as given.
    pub local: HashMap<String, String>,
    /// Needed files, in order of first use.
    pub needed: Vec<String>,
    /// The original name of each needed file (from the first user that names it).
    pub needed_names: Vec<Option<String>>,
    needed_index: HashMap<String, usize>,
    /// Remote files, in order of first use.
    pub remote: Vec<RemoteFile>,
    remote_keys: HashMap<String, usize>,
}

impl Env {
    /// Records a warning about the file at `path`.
    pub(super) fn warn(&mut self, path: &str, code: ImportWarningCode, detail: Option<&str>) {
        self.warnings.push(path, code, detail);
    }

    /// The plan index of the page a view id or a row (document) id stands for.
    pub(super) fn page(&self, id: &str) -> Option<usize> {
        let id = id.trim();
        self.views.get(id).or_else(|| self.rows.get(id)).copied()
    }

    /// The plan index of the page of a database a reference names: its id, or the id of one of its
    /// views.
    pub(super) fn database_page(&self, id: &str) -> Option<usize> {
        let id = id.trim();
        self.databases.get(id).copied().or_else(|| {
            self.views
                .get(id)
                .copied()
                .filter(|index| self.db_pages.contains(index))
        })
    }

    /// The title of a planned page.
    pub(super) fn title(&self, index: usize) -> &str {
        self.titles.get(index).map_or("", String::as_str)
    }

    /// The `local_files` file a reference names (`<view id>/<name>`, `local_files/<name>` or the
    /// bare name), recorded as a needed file with the original `name` its user gives it (the
    /// first one that gives one wins); its path as the shell gave it.
    pub(super) fn local_file(&mut self, reference: &str, name: Option<&str>) -> Option<String> {
        let reference = reference.trim();
        let bare = reference.strip_prefix("local_files/").unwrap_or(reference);
        let candidates = [bare, paths::file_name(bare)];
        let path = candidates
            .iter()
            .find_map(|name| self.local.get(*name))
            .cloned()?;
        let name = name.and_then(clean_file_name);
        match self.needed_index.get(&path) {
            Some(index) => {
                if self.needed_names[*index].is_none() {
                    self.needed_names[*index] = name;
                }
            }
            None => {
                self.needed_index.insert(path.clone(), self.needed.len());
                self.needed.push(path.clone());
                self.needed_names.push(name);
            }
        }
        Some(path)
    }

    /// The key of the remote file of an `https` URL (one key per distinct URL); `None` when the
    /// URL cannot be one.
    pub(super) fn remote_key(&mut self, url: &str) -> Option<String> {
        let url = url.trim();
        if !is_https(url) || url.len() > MAX_URL_BYTES {
            return None;
        }
        let host = host_of(url)?;
        if let Some(index) = self.remote_keys.get(url) {
            return Some(self.remote[*index].key.clone());
        }
        let key = format!("remote/{}", self.remote.len());
        self.remote_keys.insert(url.to_owned(), self.remote.len());
        self.remote.push(RemoteFile {
            key: key.clone(),
            url: url.to_owned(),
            host,
        });
        Some(key)
    }
}

/// The longest file name kept, in characters.
const MAX_FILE_NAME_CHARS: usize = 200;

/// A file name as the shell can use it: the last segment, without control characters, trimmed and
/// cut; `None` when nothing is left.
pub(super) fn clean_file_name(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last.chars().filter(|c| !c.is_control()).collect();
    let cleaned = truncate_chars(cleaned.trim(), MAX_FILE_NAME_CHARS);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// What a reference that leads outside the archive is: a network path (`\\host\share`,
/// `//host/share`) or a path or `file:` URL of the machine that made the backup. Such a
/// reference is never followed.
pub(super) fn outside_path(reference: &str) -> Option<&'static str> {
    let reference = reference.trim();
    if reference.starts_with("\\\\") || reference.starts_with("//") {
        return Some("network path");
    }
    let bytes = reference.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let file_scheme = reference
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"));
    (drive || file_scheme || reference.starts_with(['/', '\\']) || reference.starts_with("~/"))
        .then_some("local path")
}

/// Whether a URL starts with `https://` (any case).
pub(super) fn is_https(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
}

/// Whether a URL starts with `http://` (any case).
pub(super) fn is_http(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
}

/// The host of an `http(s)` URL, lowercase, without credentials and port; `None` when empty.
pub(super) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    let host = if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    let host = host.trim().to_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether a URL can be kept as a link: `http`, `https`, `mailto` or `tel`, short, no white
/// space.
pub(super) fn is_linkable(url: &str) -> bool {
    let url = url.trim();
    if url.len() > MAX_URL_BYTES || url.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    ((lower.starts_with("http://") || lower.starts_with("https://")) && host_of(url).is_some())
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(
            host_of("https://Files.Example.com:8443/a/b?c#d").as_deref(),
            Some("files.example.com")
        );
        assert_eq!(
            host_of("https://user:pw@example.com/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(host_of("https://[::1]:3000/x").as_deref(), Some("[::1]"));
        assert_eq!(host_of("https:///x"), None);
        assert_eq!(host_of("nonsense"), None);
    }

    #[test]
    fn references_outside_the_archive() {
        assert_eq!(outside_path("\\\\host\\share\\a.png"), Some("network path"));
        assert_eq!(outside_path("//host/share/a.png"), Some("network path"));
        assert_eq!(outside_path("file:///C:/a.png"), Some("local path"));
        assert_eq!(outside_path("FILE:a.png"), Some("local path"));
        assert_eq!(outside_path("C:\\Users\\a.png"), Some("local path"));
        assert_eq!(outside_path("c:/a.png"), Some("local path"));
        assert_eq!(outside_path("/home/a/a.png"), Some("local path"));
        assert_eq!(outside_path("view/a.png"), None);
        assert_eq!(outside_path("a.png"), None);
        assert_eq!(outside_path("\u{e9}\u{e9}\u{e9}.png"), None);
    }

    #[test]
    fn file_names_are_the_last_segment_of_either_kind() {
        assert_eq!(
            clean_file_name("\\\\host\\share\\a.png").as_deref(),
            Some("a.png")
        );
        assert_eq!(clean_file_name("x/y/b.pdf").as_deref(), Some("b.pdf"));
        assert_eq!(clean_file_name("dir/").as_deref(), None);
    }

    #[test]
    fn links_and_schemes() {
        assert!(is_linkable("https://example.com/a"));
        assert!(is_linkable("mailto:a@b.c"));
        assert!(!is_linkable("javascript:alert(1)"));
        assert!(!is_linkable("https://exa mple.com"));
        assert!(!is_linkable(&format!(
            "https://example.com/{}",
            "a".repeat(3000)
        )));
        assert!(is_https("HTTPS://x.y"));
        assert!(!is_https("http://x.y"));
        assert!(is_http("http://x.y"));
    }

    #[test]
    fn one_key_per_distinct_url() {
        let mut env = Env::default();
        assert_eq!(
            env.remote_key("https://a.example/1.png").as_deref(),
            Some("remote/0")
        );
        assert_eq!(
            env.remote_key("https://a.example/2.png").as_deref(),
            Some("remote/1")
        );
        assert_eq!(
            env.remote_key("https://a.example/1.png").as_deref(),
            Some("remote/0")
        );
        assert_eq!(env.remote_key("http://a.example/3.png"), None);
        assert_eq!(env.remote.len(), 2);
        assert_eq!(env.remote[0].host, "a.example");
    }

    #[test]
    fn local_files_by_reference() {
        let mut env = Env::default();
        env.local
            .insert("abc.pdf".into(), "Backup/local_files/abc.pdf".into());
        assert_eq!(
            env.local_file("view-id/abc.pdf", Some("Report.pdf"))
                .as_deref(),
            Some("Backup/local_files/abc.pdf")
        );
        assert_eq!(
            env.local_file("local_files/abc.pdf", Some("Other.pdf"))
                .as_deref(),
            Some("Backup/local_files/abc.pdf")
        );
        assert_eq!(env.local_file("zzz.pdf", None), None);
        assert_eq!(env.needed, ["Backup/local_files/abc.pdf"]);
        // The first user that names the file gives its name.
        assert_eq!(env.needed_names, [Some("Report.pdf".to_owned())]);
    }
}
