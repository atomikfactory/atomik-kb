//! What the converters share while they read one page: the index of the source (pages, binary
//! files, whiteboards), the warnings, the files and pages the page uses, and the rules that turn a
//! link or an image destination into what the page gets (ADR-0036, section 3.4).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use kb_types::{AttachmentId, BlockId, ImportWarning, ImportWarningCode, PageId};
use serde_json::Value;

use crate::excalidraw;
use crate::json::set_attr;
use crate::paths::{self, truncate_chars};
use crate::placeholder::{self, Target};

/// Extensions of the files that become pages.
pub(crate) const PAGE_EXTENSIONS: &[&str] = &["md", "markdown", "txt", "html", "htm", "csv"];

/// Extensions of files shown as pictures.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg"];

/// The most warnings kept for one file.
const MAX_WARNINGS_PER_PATH: usize = 100;

/// The longest link kept as a link, in bytes (the schema's limit for a URL is 2,048 characters).
const MAX_URL_BYTES: usize = 2_048;

/// The longest detail of a warning, in characters.
const MAX_DETAIL_CHARS: usize = 200;

/// Warnings of a plan, without repeats.
#[derive(Debug, Default)]
pub(crate) struct Warnings {
    list: Vec<ImportWarning>,
    seen: HashSet<(String, String, Option<String>)>,
    per_path: HashMap<String, usize>,
}

impl Warnings {
    /// Records a warning unless the same one (file, code, detail) is already there or the file has
    /// too many.
    pub(crate) fn push(&mut self, path: &str, code: ImportWarningCode, detail: Option<&str>) {
        let detail = detail.map(|d| truncate_chars(d, MAX_DETAIL_CHARS));
        let key = (path.to_owned(), format!("{code:?}"), detail.clone());
        if self.seen.contains(&key) {
            return;
        }
        let count = self.per_path.entry(path.to_owned()).or_default();
        if *count >= MAX_WARNINGS_PER_PATH {
            return;
        }
        *count += 1;
        self.seen.insert(key);
        self.list.push(ImportWarning {
            path: path.to_owned(),
            code,
            detail,
        });
    }

    /// The warnings, in the order they were recorded.
    pub(crate) fn into_vec(self) -> Vec<ImportWarning> {
        self.list
    }
}

/// The files and pages of the source as the converters look them up. Paths are the normalised
/// ones (the stripped top-level folder removed).
#[derive(Debug, Default)]
pub(crate) struct SourceIndex {
    /// Page files by path: the arena index of their page.
    pub pages: HashMap<String, usize>,
    /// The same by lowercase path.
    pub pages_lower: HashMap<String, Vec<usize>>,
    /// Pages by Notion id.
    pub notion: HashMap<String, usize>,
    /// Binary files by path: the path as the shell gave it.
    pub binary: HashMap<String, String>,
    /// Binary files by lowercase path.
    pub binary_lower: HashMap<String, Vec<String>>,
    /// Binary files by lowercase file name.
    pub binary_names: HashMap<String, Vec<String>>,
    /// The text of the `.excalidraw` files by path.
    pub whiteboards: HashMap<String, String>,
}

impl SourceIndex {
    /// The page for a normalised path: exact, then ASCII-case-insensitive when unique, then with
    /// a page extension added when the path has none.
    pub(crate) fn find_page(&self, path: &str) -> Option<usize> {
        if let Some(id) = self.pages.get(path) {
            return Some(*id);
        }
        if let Some([only]) = self
            .pages_lower
            .get(&path.to_lowercase())
            .map(Vec::as_slice)
        {
            return Some(*only);
        }
        if paths::ext_of(path).is_empty() {
            for ext in ["md", "markdown", "html", "htm", "txt"] {
                let candidate = format!("{path}.{ext}");
                if let Some(id) = self.pages.get(&candidate) {
                    return Some(*id);
                }
            }
        }
        None
    }

    /// The path of a binary file: exact, else ASCII-case-insensitive when unique.
    pub(crate) fn find_binary(&self, path: &str) -> Option<&str> {
        if let Some((key, _)) = self.binary.get_key_value(path) {
            return Some(key.as_str());
        }
        match self
            .binary_lower
            .get(&path.to_lowercase())
            .map(Vec::as_slice)
        {
            Some([only]) => Some(only.as_str()),
            _ => None,
        }
    }

    /// The path of a `.excalidraw` file.
    pub(crate) fn find_whiteboard(&self, path: &str) -> Option<&str> {
        self.whiteboards
            .get_key_value(path)
            .map(|(key, _)| key.as_str())
    }

    /// The page whose Notion id ends the name of `norm`, when `norm` looks like a page.
    pub(crate) fn notion_fallback(&self, norm: &str) -> Option<usize> {
        let (stem, ext) = paths::split_ext(paths::file_name(norm));
        if !(ext.is_empty() || PAGE_EXTENSIONS.contains(&ext.as_str())) {
            return None;
        }
        let id = paths::notion_id_in(stem.strip_suffix("_all").unwrap_or(stem))?;
        self.notion.get(&id).copied()
    }

    /// The planned page a `notion.so` URL names, by the id at the end of its path.
    pub(crate) fn notion_page(&self, url: &str) -> Option<usize> {
        let after_scheme = url.split_once("://")?.1;
        let (host, path) = after_scheme.split_once('/')?;
        let host = host.to_ascii_lowercase();
        if !(host.ends_with("notion.so") || host.ends_with("notion.site")) {
            return None;
        }
        let path = path.split(['?', '#']).next().unwrap_or_default();
        let segment = path.rsplit('/').find(|s| !s.is_empty())?;
        let id = paths::notion_id_in(&paths::decode(segment))?;
        self.notion.get(&id).copied()
    }

    /// The planned page a link destination names: a path relative to `dir` (by path, then by the
    /// Notion id in its name) or a `notion.so` URL.
    pub(crate) fn page_for_dest(&self, dir: &str, dest: &str) -> Option<usize> {
        let dest = dest.trim();
        if let Some((scheme, _)) = split_scheme(dest) {
            return matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
                .then(|| self.notion_page(dest))
                .flatten();
        }
        let norm = paths::resolve_relative(dir, dest)?;
        self.find_page(&norm)
            .or_else(|| self.notion_fallback(&norm))
    }

    /// A binary file by bare name (an Obsidian embed `![[photo.png]]`), when exactly one has it.
    pub(crate) fn binary_by_name(&self, name: &str) -> Option<&str> {
        match self
            .binary_names
            .get(&name.to_lowercase())
            .map(Vec::as_slice)
        {
            Some([only]) => Some(only.as_str()),
            _ => None,
        }
    }
}

/// Where a link or image destination leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Dest {
    /// An `http`, `https`, `mailto` or `tel` URL.
    External(String),
    /// A page of the source (arena index).
    Page(usize),
    /// A binary file of the source: its normalised path and the path as the shell gave it.
    Binary {
        /// Normalised path.
        norm: String,
        /// Original path (the one of `needed_files`).
        orig: String,
    },
    /// A `.excalidraw` file of the source: its path.
    Whiteboard(String),
    /// An `attachment://` id (canonical).
    Attachment(String),
    /// An `atomik-kb://page/...` link.
    Atomik {
        /// Page id (canonical).
        page: String,
        /// Block id (canonical).
        block: Option<String>,
    },
    /// A `data:` URI.
    Data,
    /// A `#fragment` of the same page.
    Anchor,
    /// A relative destination that names nothing in the source.
    Missing(String),
    /// Anything else (another scheme, an empty destination).
    Other(String),
}

/// What a link does to the text it wraps.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LinkAction {
    /// The text carries a `link` mark with this URL.
    Mark(String),
    /// The text stays as it is.
    Plain,
    /// The text is swallowed; this inline node (a page reference) takes its place, and its
    /// `$label` is the text.
    Inline(Value),
    /// The text stays; this block (a file, image or whiteboard) follows the paragraph, and
    /// becomes the paragraph when the link is alone in it.
    After(Value),
}

/// What an image does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ImageAction {
    /// An image or whiteboard block.
    Block(Value),
    /// Text in place of the image, linked when `href` is set.
    Text {
        /// The text.
        text: String,
        /// Its link.
        href: Option<String>,
    },
}

/// The state shared by the converters of one page.
#[derive(Debug)]
pub(crate) struct PageCtx<'a> {
    /// The source.
    pub index: &'a SourceIndex,
    /// The page's path as the shell gave it, for warnings.
    pub orig: &'a str,
    /// The folder of the page, normalised.
    pub dir: &'a str,
    /// The page's arena index.
    pub node: usize,
    /// The plan's warnings.
    pub warnings: &'a mut Warnings,
    /// Files the page uses, as the shell gave their paths, in first-use order.
    pub needed: Vec<String>,
    /// Pages the page links to, in order of first link.
    pub links: Vec<usize>,
    needed_seen: HashSet<String>,
    links_seen: HashSet<usize>,
}

impl<'a> PageCtx<'a> {
    /// A context for the page `node`.
    pub(crate) fn new(
        index: &'a SourceIndex,
        warnings: &'a mut Warnings,
        orig: &'a str,
        dir: &'a str,
        node: usize,
    ) -> Self {
        Self {
            index,
            orig,
            dir,
            node,
            warnings,
            needed: Vec::new(),
            links: Vec::new(),
            needed_seen: HashSet::new(),
            links_seen: HashSet::new(),
        }
    }

    /// Records a warning about this page's file.
    pub(crate) fn warn(&mut self, code: ImportWarningCode, detail: Option<&str>) {
        self.warnings.push(self.orig, code, detail);
    }

    /// Records that the page uses a file.
    pub(crate) fn need(&mut self, orig: &str) {
        if self.needed_seen.insert(orig.to_owned()) {
            self.needed.push(orig.to_owned());
        }
    }

    /// Records that the page links to a page.
    pub(crate) fn note_link(&mut self, page: usize) {
        if self.links_seen.insert(page) {
            self.links.push(page);
        }
    }

    /// Where a link or image destination leads.
    pub(crate) fn classify(&self, raw: &str) -> Dest {
        let raw = raw.trim();
        if raw.is_empty() {
            return Dest::Other(String::new());
        }
        if raw.starts_with('#') {
            return Dest::Anchor;
        }
        if let Some((scheme, rest)) = split_scheme(raw) {
            return match scheme.to_ascii_lowercase().as_str() {
                "http" | "https" => match self.index.notion_page(raw) {
                    Some(id) => Dest::Page(id),
                    None if raw.len() > MAX_URL_BYTES => Dest::Other("link too long".to_owned()),
                    None => Dest::External(raw.to_owned()),
                },
                "mailto" | "tel" if raw.len() > MAX_URL_BYTES => {
                    Dest::Other("link too long".to_owned())
                }
                "mailto" | "tel" => Dest::External(raw.to_owned()),
                "attachment" => attachment_dest(rest),
                "atomik-kb" => atomik_dest(rest),
                "data" => Dest::Data,
                other => Dest::Other(other.to_owned()),
            };
        }
        let Some(norm) = paths::resolve_relative(self.dir, raw) else {
            return Dest::Missing(raw.to_owned());
        };
        let index = self.index;
        if let Some(id) = index.find_page(&norm) {
            return Dest::Page(id);
        }
        if let Some(found) = index.find_binary(&norm) {
            let orig = index.binary.get(found).cloned().unwrap_or_default();
            return Dest::Binary {
                norm: found.to_owned(),
                orig,
            };
        }
        if let Some(found) = index.find_whiteboard(&norm) {
            return Dest::Whiteboard(found.to_owned());
        }
        // Notion links by path; the id in the name is the sturdier part.
        if let Some(page) = index.notion_fallback(&norm) {
            return Dest::Page(page);
        }
        Dest::Missing(norm)
    }

    /// What a link wrapping text does.
    pub(crate) fn link_action(&mut self, raw: &str) -> LinkAction {
        match self.classify(raw) {
            Dest::External(url) => LinkAction::Mark(url),
            Dest::Page(id) => {
                self.note_link(id);
                LinkAction::Inline(placeholder::page_ref(&Target::Planned(id), ""))
            }
            Dest::Binary { orig, .. } => {
                self.need(&orig);
                if is_image_path(&orig) {
                    LinkAction::After(placeholder::image_file(&orig, "", ""))
                } else {
                    LinkAction::After(placeholder::file_path(&orig, ""))
                }
            }
            Dest::Whiteboard(path) => match self.whiteboard_block(&path) {
                Some(block) => LinkAction::After(block),
                None => LinkAction::Plain,
            },
            Dest::Attachment(id) => LinkAction::After(placeholder::file_attachment(&id, "")),
            Dest::Atomik { page, block } => {
                LinkAction::Inline(placeholder::page_ref(&Target::Existing { page, block }, ""))
            }
            Dest::Anchor | Dest::Data => LinkAction::Plain,
            Dest::Missing(dest) => {
                self.warn_missing(&dest);
                LinkAction::Plain
            }
            Dest::Other(scheme) => {
                self.warn(ImportWarningCode::UnresolvedLink, Some(&scheme));
                LinkAction::Plain
            }
        }
    }

    /// What an image does. `alt` is the alternative text, `title` the link title (the caption).
    pub(crate) fn image_action(&mut self, raw: &str, alt: &str, title: &str) -> ImageAction {
        let alt = alt.trim();
        let title = title.trim();
        match self.classify(raw) {
            Dest::Binary { orig, .. } => {
                self.need(&orig);
                ImageAction::Block(placeholder::image_file(&orig, alt, title))
            }
            Dest::Whiteboard(path) => match self.whiteboard_block(&path) {
                Some(block) => ImageAction::Block(block),
                None => ImageAction::Text {
                    text: alt.to_owned(),
                    href: None,
                },
            },
            Dest::Attachment(id) => {
                ImageAction::Block(placeholder::image_attachment(&id, alt, title))
            }
            Dest::External(url) if url.starts_with("http") => {
                self.warn(ImportWarningCode::RemoteImage, Some(&url));
                ImageAction::Text {
                    text: if alt.is_empty() {
                        url.clone()
                    } else {
                        alt.to_owned()
                    },
                    href: Some(url),
                }
            }
            Dest::Data => {
                self.warn(ImportWarningCode::EmbeddedImage, None);
                ImageAction::Text {
                    text: alt.to_owned(),
                    href: None,
                }
            }
            Dest::Missing(dest) => {
                self.warnings
                    .push(self.orig, ImportWarningCode::MissingFile, Some(&dest));
                let name = paths::file_name(&dest);
                ImageAction::Text {
                    text: if alt.is_empty() {
                        name.to_owned()
                    } else {
                        alt.to_owned()
                    },
                    href: None,
                }
            }
            Dest::Other(scheme) => {
                self.warn(ImportWarningCode::UnresolvedLink, Some(&scheme));
                ImageAction::Text {
                    text: alt.to_owned(),
                    href: None,
                }
            }
            Dest::External(_) | Dest::Page(_) | Dest::Atomik { .. } | Dest::Anchor => {
                ImageAction::Text {
                    text: alt.to_owned(),
                    href: None,
                }
            }
        }
    }

    /// The warning for a relative link or file that names nothing in the source: `MissingFile`
    /// for something that looks like a file, `UnresolvedLink` for something that looks like a
    /// page.
    pub(crate) fn warn_missing(&mut self, dest: &str) {
        let ext = paths::ext_of(dest);
        let code = if ext.is_empty() || PAGE_EXTENSIONS.contains(&ext.as_str()) {
            ImportWarningCode::UnresolvedLink
        } else {
            ImportWarningCode::MissingFile
        };
        self.warnings.push(self.orig, code, Some(dest));
    }

    /// A whiteboard block built from the `.excalidraw` file at `path` (`None` and a warning when
    /// the file cannot be read).
    pub(crate) fn whiteboard_block(&mut self, path: &str) -> Option<Value> {
        let text = self.index.whiteboards.get(path)?;
        match excalidraw::elements(text) {
            Some(parsed) => {
                if parsed.images {
                    self.warn(
                        ImportWarningCode::UnsupportedContent,
                        Some("whiteboard images"),
                    );
                }
                Some(placeholder::whiteboard(parsed.elements, None))
            }
            None => {
                self.warn(ImportWarningCode::UnsupportedContent, Some("whiteboard"));
                None
            }
        }
    }

    /// Gives the block that follows a paragraph the text of the link that made it: the alternative
    /// text of an image, the label of a file.
    pub(crate) fn name_after_block(block: &mut Value, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match crate::json::kind(block) {
            "image" => set_attr(block, "alt", Value::String(text.to_owned())),
            "file" => set_attr(block, placeholder::LABEL, Value::String(text.to_owned())),
            _ => {}
        }
    }
}

/// Whether a path names a picture, by its extension.
pub(crate) fn is_image_path(path: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&paths::ext_of(path).as_str())
}

/// `scheme` and the rest of a destination like `https://host/path`, `mailto:a@b`; `None` for a
/// relative path (a one-letter scheme is a drive letter, not a scheme).
fn split_scheme(raw: &str) -> Option<(&str, &str)> {
    let colon = raw.find(':')?;
    let scheme = &raw[..colon];
    let first = scheme.chars().next()?;
    if scheme.len() < 2
        || !first.is_ascii_alphabetic()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    Some((scheme, &raw[colon + 1..]))
}

/// `//<uuid>` of an `attachment:` link.
fn attachment_dest(rest: &str) -> Dest {
    let id = rest
        .trim_start_matches('/')
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    match AttachmentId::from_str(id) {
        Ok(id) => Dest::Attachment(id.to_string()),
        Err(_) => Dest::Missing(format!("attachment:{rest}")),
    }
}

/// `//page/<uuid>[#<block>]` of an `atomik-kb:` link.
fn atomik_dest(rest: &str) -> Dest {
    let rest = rest.trim_start_matches('/');
    let (path, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let path = path.split('?').next().unwrap_or_default();
    let Some(id) = path.strip_prefix("page/") else {
        return Dest::Other("atomik-kb".to_owned());
    };
    let Ok(page) = PageId::from_str(id.trim_end_matches('/')) else {
        return Dest::Other("atomik-kb".to_owned());
    };
    Dest::Atomik {
        page: page.to_string(),
        block: BlockId::from_str(fragment).ok().map(|id| id.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const NOTION_ID: &str = "0123456789abcdef0123456789abcdef";

    fn index() -> SourceIndex {
        let mut index = SourceIndex::default();
        index.pages.insert("Notes/A.md".into(), 1);
        index.pages_lower.insert("notes/a.md".into(), vec![1]);
        index.pages.insert("B.md".into(), 2);
        index.pages_lower.insert("b.md".into(), vec![2]);
        index.pages.insert(format!("C {NOTION_ID}.md"), 3);
        index
            .pages_lower
            .insert(format!("c {NOTION_ID}.md"), vec![3]);
        index.notion.insert(NOTION_ID.into(), 3);
        index
            .binary
            .insert("Notes/pic.png".into(), "Export/Notes/pic.png".into());
        index
            .binary_lower
            .insert("notes/pic.png".into(), vec!["Notes/pic.png".into()]);
        index
            .binary_names
            .insert("pic.png".into(), vec!["Notes/pic.png".into()]);
        index
            .whiteboards
            .insert("Notes/w.excalidraw".into(), "{}".into());
        index
    }

    fn classify(raw: &str) -> Dest {
        let index = index();
        let mut warnings = Warnings::default();
        let ctx = PageCtx::new(&index, &mut warnings, "Export/Notes/A.md", "Notes", 1);
        ctx.classify(raw)
    }

    #[test]
    fn destinations_are_classified() {
        assert_eq!(
            classify("https://example.com/x"),
            Dest::External("https://example.com/x".into())
        );
        assert_eq!(
            classify("mailto:a@b.c"),
            Dest::External("mailto:a@b.c".into())
        );
        assert_eq!(classify("../B.md"), Dest::Page(2));
        assert_eq!(classify("../b.MD"), Dest::Page(2));
        assert_eq!(classify("../B"), Dest::Page(2));
        assert_eq!(classify("A.md#section"), Dest::Page(1));
        assert_eq!(
            classify("pic.png"),
            Dest::Binary {
                norm: "Notes/pic.png".into(),
                orig: "Export/Notes/pic.png".into()
            }
        );
        assert_eq!(
            classify("w.excalidraw"),
            Dest::Whiteboard("Notes/w.excalidraw".into())
        );
        assert_eq!(classify("#heading"), Dest::Anchor);
        assert_eq!(classify("data:image/png;base64,AAAA"), Dest::Data);
        assert_eq!(
            classify("missing.md"),
            Dest::Missing("Notes/missing.md".into())
        );
        assert_eq!(
            classify("../../../x.md"),
            Dest::Missing("../../../x.md".into())
        );
        assert_eq!(
            classify("javascript:alert(1)"),
            Dest::Other("javascript".into())
        );
        assert_eq!(classify(""), Dest::Other(String::new()));
        // Links by Notion id: the path is wrong, the id is right.
        assert_eq!(
            classify(&format!("Elsewhere/C%20{NOTION_ID}.md")),
            Dest::Page(3)
        );
        assert_eq!(
            classify(&format!(
                "https://www.notion.so/Some-Title-{NOTION_ID}?pvs=4"
            )),
            Dest::Page(3)
        );
        assert_eq!(
            classify(&format!("https://www.notion.so/workspace/{NOTION_ID}")),
            Dest::Page(3)
        );
        assert_eq!(
            classify("https://www.notion.so/Other-fedcba9876543210fedcba9876543210"),
            Dest::External("https://www.notion.so/Other-fedcba9876543210fedcba9876543210".into())
        );
    }

    #[test]
    fn very_long_links_are_not_kept() {
        let long = format!("https://example.com/{}", "a".repeat(3_000));
        assert_eq!(classify(&long), Dest::Other("link too long".into()));
        let ok = format!("https://example.com/{}", "a".repeat(1_000));
        assert_eq!(classify(&ok), Dest::External(ok.clone()));
    }

    #[test]
    fn attachment_and_atomik_links_need_valid_ids() {
        let id = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
        assert_eq!(
            classify(&format!("attachment://{id}")),
            Dest::Attachment(id.into())
        );
        assert!(matches!(
            classify("attachment://not-an-id"),
            Dest::Missing(_)
        ));
        let page = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
        let block = "8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7";
        assert_eq!(
            classify(&format!("atomik-kb://page/{page}#{block}")),
            Dest::Atomik {
                page: page.into(),
                block: Some(block.into())
            }
        );
        assert_eq!(
            classify(&format!("atomik-kb://page/{page}")),
            Dest::Atomik {
                page: page.into(),
                block: None
            }
        );
        assert_eq!(
            classify("atomik-kb://page/nope"),
            Dest::Other("atomik-kb".into())
        );
    }

    #[test]
    fn link_actions_record_files_pages_and_warnings() {
        let index = index();
        let mut warnings = Warnings::default();
        let mut ctx = PageCtx::new(&index, &mut warnings, "Export/Notes/A.md", "Notes", 1);
        assert_eq!(
            ctx.link_action("https://a.b"),
            LinkAction::Mark("https://a.b".into())
        );
        assert!(matches!(ctx.link_action("../B.md"), LinkAction::Inline(_)));
        assert!(matches!(ctx.link_action("pic.png"), LinkAction::After(_)));
        assert_eq!(ctx.link_action("gone.md"), LinkAction::Plain);
        assert_eq!(ctx.link_action("gone.pdf"), LinkAction::Plain);
        assert_eq!(ctx.needed, ["Export/Notes/pic.png"]);
        assert_eq!(ctx.links, [2]);
        let list = warnings.into_vec();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].code, ImportWarningCode::UnresolvedLink);
        assert_eq!(list[0].path, "Export/Notes/A.md");
        assert_eq!(list[1].code, ImportWarningCode::MissingFile);
    }

    #[test]
    fn image_actions() {
        let index = index();
        let mut warnings = Warnings::default();
        let mut ctx = PageCtx::new(&index, &mut warnings, "A.md", "Notes", 1);
        assert!(matches!(
            ctx.image_action("pic.png", "alt", "cap"),
            ImageAction::Block(_)
        ));
        assert_eq!(
            ctx.image_action("https://x.y/i.png", "", ""),
            ImageAction::Text {
                text: "https://x.y/i.png".into(),
                href: Some("https://x.y/i.png".into())
            }
        );
        assert_eq!(
            ctx.image_action("data:image/png;base64,AA", "a picture", ""),
            ImageAction::Text {
                text: "a picture".into(),
                href: None
            }
        );
        assert_eq!(
            ctx.image_action("nope.png", "", ""),
            ImageAction::Text {
                text: "nope.png".into(),
                href: None
            }
        );
        let codes: Vec<_> = warnings.into_vec().into_iter().map(|w| w.code).collect();
        assert_eq!(
            codes,
            [
                ImportWarningCode::RemoteImage,
                ImportWarningCode::EmbeddedImage,
                ImportWarningCode::MissingFile
            ]
        );
    }

    #[test]
    fn warnings_are_deduplicated_and_capped() {
        let mut warnings = Warnings::default();
        for _ in 0..3 {
            warnings.push("a.md", ImportWarningCode::UnresolvedLink, Some("x"));
        }
        for n in 0..300 {
            warnings.push(
                "b.md",
                ImportWarningCode::UnresolvedLink,
                Some(&n.to_string()),
            );
        }
        warnings.push(
            "a.md",
            ImportWarningCode::UnresolvedLink,
            Some(&"y".repeat(1000)),
        );
        let list = warnings.into_vec();
        assert_eq!(list.iter().filter(|w| w.path == "a.md").count(), 2);
        assert_eq!(list.iter().filter(|w| w.path == "b.md").count(), 100);
        assert!(
            list.iter()
                .all(|w| w.detail.as_ref().is_some_and(|d| d.chars().count() <= 200))
        );
    }
}
