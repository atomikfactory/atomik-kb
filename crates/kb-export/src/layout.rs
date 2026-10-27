//! The layout of an export: where each page, attachment and whiteboard goes in the archive
//! (`docs/architecture/import-export.md`, "Layout and names").
//!
//! A page is `<name>.md` (or `.html`) in its parent's folder; a page with children, attachments or
//! whiteboards of its own also has the folder `<name>/` next to its file. Names are the titles made
//! safe for Windows, macOS and Linux, de-duplicated per folder without regard to ASCII case.

use std::collections::{HashMap, HashSet};

use kb_types::{AttachmentId, BlockId, ExportFormat, PageId};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use crate::sidecar::SIDECAR_NAME;

/// Longest stem of a name, in characters.
const MAX_STEM_CHARS: usize = 100;

/// Longest extension (without the dot) that is told from a dot in a name.
const MAX_EXTENSION_CHARS: usize = 10;

/// The stem of a name that cleans to nothing.
const UNTITLED: &str = "Untitled";

/// The stem of a whiteboard's file.
const WHITEBOARD_STEM: &str = "Whiteboard";

/// The extension of a whiteboard's file.
const WHITEBOARD_EXTENSION: &str = ".excalidraw";

/// Bytes of a path segment that are written as `%XX` in a link: ASCII controls, space and
/// `"#%'()<>?[\]^`{|}`. Everything else, non-ASCII included, is written as it is.
const HREF_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// A page of the export, in tree order (a parent before its children).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPage {
    /// The page.
    pub page_id: PageId,
    /// Its parent when the parent is part of the export; `None` for a top-level page of the
    /// export.
    pub parent: Option<PageId>,
    /// Title as stored (may be empty).
    pub title: String,
    /// The attachments the page uses (blocks, gallery items, `files` property values, an
    /// uploaded icon), in document order, without duplicates.
    pub attachments: Vec<LayoutAttachment>,
    /// The page's whiteboard blocks, in document order.
    pub whiteboards: Vec<BlockId>,
}

/// An attachment a page uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutAttachment {
    /// Attachment.
    pub attachment_id: AttachmentId,
    /// Its file name as stored.
    pub name: String,
    /// Its bytes are on this device (only those get an entry in the archive).
    pub available: bool,
}

/// Where everything goes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    /// One page and nothing else: a plain `.md` or `.html` file instead of a `.zip`.
    pub single_file: bool,
    /// The file name to offer: `<name>.md`, `<name>.html` or `<name>.zip`.
    pub suggested_name: String,
    /// Path of each page's file in the archive (or the suggested name for a single file).
    pub page_files: HashMap<PageId, String>,
    /// Path of each available attachment's file in the archive (empty for a single file).
    pub attachment_files: HashMap<AttachmentId, String>,
    /// Path of each whiteboard's `.excalidraw` file in the archive.
    pub whiteboard_files: HashMap<(PageId, BlockId), String>,
    /// Path of `atomik-export.json` (Markdown archives only).
    pub sidecar: Option<String>,
}

/// Lays out an export named `export_name` (the root page's title, or the space's name).
///
/// - **Single file**: exactly one page, no available attachment and no whiteboard. The file is
///   `<title>.md` or `<title>.html`, which is also the suggested name and the page's path; links
///   to its unavailable attachments stay `attachment://<id>`.
/// - Otherwise a `.zip` named `<export name>.zip`. A top-level page is at the archive's top level;
///   a page that has children in the export, attachments it is the first to use (tree order) or
///   whiteboards also has the folder `<name>/` holding them. An attachment shared by several
///   pages is written once, in the folder of the first page that uses it, and only if its bytes
///   are on this device. Names are de-duplicated per folder, ignoring ASCII case, pages first
///   (tree order), then attachments, then whiteboards: `Notes.md` and `notes.md` become
///   `Notes.md` and `notes (2).md`. A Markdown archive also has [`SIDECAR_NAME`] at its top
///   level.
#[must_use]
pub fn layout(export_name: &str, pages: &[LayoutPage], format: ExportFormat) -> Layout {
    let extension = match format {
        ExportFormat::Markdown => ".md",
        ExportFormat::Html => ".html",
    };

    if let [page] = pages
        && page.whiteboards.is_empty()
        && !page
            .attachments
            .iter()
            .any(|attachment| attachment.available)
    {
        let name = format!("{}{extension}", safe_stem(&page.title));
        return Layout {
            single_file: true,
            suggested_name: name.clone(),
            page_files: HashMap::from([(page.page_id, name)]),
            ..Layout::default()
        };
    }

    let mut out = Layout {
        single_file: false,
        suggested_name: format!("{}.zip", safe_stem(export_name)),
        sidecar: (format == ExportFormat::Markdown).then(|| SIDECAR_NAME.to_owned()),
        ..Layout::default()
    };

    // The tree: the children of each page, the top-level pages.
    let position: HashMap<PageId, usize> = pages
        .iter()
        .enumerate()
        .map(|(at, page)| (page.page_id, at))
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    let mut top_level: Vec<usize> = Vec::new();
    for (at, page) in pages.iter().enumerate() {
        // A parent that comes later in the list (or is not in it) cannot hold a folder yet:
        // the page goes to the top level rather than nowhere.
        match page
            .parent
            .and_then(|parent| position.get(&parent))
            .filter(|parent| **parent < at)
        {
            Some(parent) => children[*parent].push(at),
            None => top_level.push(at),
        }
    }

    // Each available attachment goes with the first page (tree order) that uses it.
    let mut placed: HashSet<AttachmentId> = HashSet::new();
    let owned: Vec<Vec<&LayoutAttachment>> = pages
        .iter()
        .map(|page| {
            page.attachments
                .iter()
                .filter(|attachment| {
                    attachment.available && placed.insert(attachment.attachment_id)
                })
                .collect()
        })
        .collect();

    let has_folder: Vec<bool> = pages
        .iter()
        .enumerate()
        .map(|(at, page)| {
            !children[at].is_empty() || !owned[at].is_empty() || !page.whiteboards.is_empty()
        })
        .collect();

    // The folder (with its trailing `/`) of each page that has one.
    let mut folders: Vec<String> = vec![String::new(); pages.len()];

    // The archive's top level, then the folder of each page, in tree order (a page's own name is
    // known by the time its folder is filled).
    let mut root_taken: HashSet<String> = HashSet::new();
    if let Some(sidecar) = &out.sidecar {
        root_taken.insert(sidecar.to_ascii_lowercase());
    }
    let mut fill = |owner: Option<usize>, taken: &mut HashSet<String>| {
        let dir = owner.map_or("", |at| folders[at].as_str()).to_owned();
        let member_pages: &[usize] =
            owner.map_or(top_level.as_slice(), |at| children[at].as_slice());
        let mut named: Vec<(usize, String)> = Vec::new();
        for &child in member_pages {
            let stem = safe_stem(&pages[child].title);
            let name = unique(taken, &stem, extension, has_folder[child]);
            named.push((child, name));
        }
        for (child, name) in named {
            out.page_files
                .insert(pages[child].page_id, format!("{dir}{name}{extension}"));
            if has_folder[child] {
                folders[child] = format!("{dir}{name}/");
            }
        }
        if let Some(at) = owner {
            for attachment in &owned[at] {
                let (stem, ext) = safe_parts(&attachment.name, true);
                let name = unique(taken, &stem, &ext, false);
                out.attachment_files
                    .insert(attachment.attachment_id, format!("{dir}{name}{ext}"));
            }
            for block in &pages[at].whiteboards {
                let name = unique(taken, WHITEBOARD_STEM, WHITEBOARD_EXTENSION, false);
                out.whiteboard_files.insert(
                    (pages[at].page_id, *block),
                    format!("{dir}{name}{WHITEBOARD_EXTENSION}"),
                );
            }
        }
    };
    fill(None, &mut root_taken);
    for (at, has_own_folder) in has_folder.iter().enumerate() {
        if *has_own_folder {
            fill(Some(at), &mut HashSet::new());
        }
    }
    out
}

/// A free name for `stem` + `extension` in the folder whose names (ASCII-lowercased files and
/// folders) are `taken`: `stem`, then `stem (2)`, `stem (3)`, ... When `folder` is set the name
/// must also be free as a folder name. The name is claimed.
fn unique(taken: &mut HashSet<String>, stem: &str, extension: &str, folder: bool) -> String {
    let mut number = 1_usize;
    loop {
        let name = if number == 1 {
            stem.to_owned()
        } else {
            format!("{stem} ({number})")
        };
        let file_key = format!("{name}{extension}").to_ascii_lowercase();
        let folder_key = name.to_ascii_lowercase();
        if !taken.contains(&file_key) && !(folder && taken.contains(&folder_key)) {
            taken.insert(file_key);
            if folder {
                taken.insert(folder_key);
            }
            return name;
        }
        number += 1;
    }
}

/// The relative link from the file at `from_file` to the file at `to_file` (both archive paths),
/// with `../` segments as needed and every segment percent-encoded for a Markdown or HTML link.
///
/// Encoded: ASCII controls, space and `"#%'()<>?[\]^`{|}`; non-ASCII stays as UTF-8.
#[must_use]
pub fn relative_href(from_file: &str, to_file: &str) -> String {
    let mut from_dir: Vec<&str> = from_file.split('/').collect();
    from_dir.pop();
    let to: Vec<&str> = to_file.split('/').collect();
    // Only directories are compared: the last segment of `to` is the file.
    let to_dirs = to.len().saturating_sub(1);
    let common = from_dir
        .iter()
        .zip(to.iter().take(to_dirs))
        .take_while(|(from, to)| from == to)
        .count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from_dir.len().saturating_sub(common)];
    parts.extend(
        to.iter()
            .skip(common)
            .map(|segment| encode_segment(segment)),
    );
    parts.join("/")
}

/// One path segment as it is written in a link.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    let mut buffer = [0_u8; 4];
    for c in segment.chars() {
        if c.is_ascii() {
            out.extend(utf8_percent_encode(
                c.encode_utf8(&mut buffer),
                HREF_SEGMENT,
            ));
        } else {
            out.push(c);
        }
    }
    out
}

/// A title or attachment name made safe as a file or folder name on Windows, macOS and Linux
/// (`Untitled` when nothing is left), without de-duplication.
///
/// Every character in `<>:"/\|?*` and every control character becomes a space; runs of white space
/// become one space; spaces at both ends and dots at both ends are removed (a leading dot would
/// make a hidden file, which the importer skips); a name equal (ASCII case-insensitive, with or
/// without an extension) to `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9` or `LPT1`-`LPT9` gets a `_`
/// appended to its stem; the stem is cut to 100 characters, before the extension (a dot followed
/// by 1 to 10 letters or digits at the end of the name). Emoji and every other character the
/// three file systems accept are kept.
#[must_use]
pub fn safe_file_name(name: &str) -> String {
    let (stem, extension) = safe_parts(name, true);
    format!("{stem}{extension}")
}

/// [`safe_file_name`] for a name that has no extension (a page's title): the whole name is the
/// stem, whatever dots it holds.
fn safe_stem(name: &str) -> String {
    safe_parts(name, false).0
}

/// The safe stem and the extension (with its dot, or empty) of `name`.
fn safe_parts(name: &str, with_extension: bool) -> (String, String) {
    let spaced: String = name
        .chars()
        .map(|c| if is_unsafe(c) { ' ' } else { c })
        .collect();
    let collapsed = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_end_matches([' ', '.']);
    let (stem, extension) = if with_extension {
        split_extension(trimmed)
    } else {
        (trimmed, "")
    };
    let stem: String = stem
        .trim_matches([' ', '.'])
        .chars()
        .take(MAX_STEM_CHARS)
        .collect();
    let stem = stem.trim_matches([' ', '.']);
    if stem.is_empty() {
        return (UNTITLED.to_owned(), extension.to_owned());
    }
    (guard_reserved(stem), extension.to_owned())
}

/// Whether `c` cannot be in a file name on one of the three systems.
fn is_unsafe(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()
}

/// `name` as stem and extension: the extension is what follows the last dot when that dot is not
/// the first character and 1 to 10 letters or digits follow it.
fn split_extension(name: &str) -> (&str, &str) {
    if let Some(dot) = name.rfind('.')
        && dot > 0
    {
        let extension = &name[dot + 1..];
        if (1..=MAX_EXTENSION_CHARS).contains(&extension.len())
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return (&name[..dot], &name[dot..]);
        }
    }
    (name, "")
}

/// A stem whose part before the first dot is a name Windows reserves for a device gets a `_`
/// after that part (`con` is `con_`, `NUL.txt` is `NUL_.txt`): Windows reads `CON.anything` as
/// the console.
fn guard_reserved(stem: &str) -> String {
    let head_len = stem.find('.').unwrap_or(stem.len());
    let (head, tail) = stem.split_at(head_len);
    if is_reserved(head.trim_end_matches(' ')) {
        format!("{head}_{tail}")
    } else {
        stem.to_owned()
    }
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9` and `LPT1`-`LPT9`, in any ASCII case.
fn is_reserved(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let mut chars = upper.chars();
    let (Some(first), Some(second), Some(third), Some(digit), None) = (
        chars.next(),
        chars.next(),
        chars.next(),
        chars.next(),
        chars.next(),
    ) else {
        return false;
    };
    matches!([first, second, third], ['C', 'O', 'M'] | ['L', 'P', 'T'])
        && ('1'..='9').contains(&digit)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use kb_types::{AttachmentId, BlockId, ExportFormat, PageId};

    use super::*;

    fn pid(n: u8) -> PageId {
        PageId::from_random_bytes([n; 16])
    }

    fn aid(n: u8) -> AttachmentId {
        AttachmentId::from_random_bytes([n; 16])
    }

    fn bid(n: u8) -> BlockId {
        BlockId::from_random_bytes([n; 16])
    }

    fn page(n: u8, parent: Option<u8>, title: &str) -> LayoutPage {
        LayoutPage {
            page_id: pid(n),
            parent: parent.map(pid),
            title: title.to_owned(),
            attachments: Vec::new(),
            whiteboards: Vec::new(),
        }
    }

    fn with_attachments(mut page: LayoutPage, files: &[(u8, &str, bool)]) -> LayoutPage {
        page.attachments = files
            .iter()
            .map(|(n, name, available)| LayoutAttachment {
                attachment_id: aid(*n),
                name: (*name).to_owned(),
                available: *available,
            })
            .collect();
        page
    }

    fn with_whiteboards(mut page: LayoutPage, blocks: &[u8]) -> LayoutPage {
        page.whiteboards = blocks.iter().map(|n| bid(*n)).collect();
        page
    }

    fn files(layout: &Layout, pages: &[LayoutPage]) -> Vec<String> {
        pages
            .iter()
            .map(|page| layout.page_files[&page.page_id].clone())
            .collect()
    }

    // ----- safe names -----

    #[test]
    fn unsafe_characters_become_one_space() {
        assert_eq!(safe_file_name("Report: Q3?"), "Report Q3");
        assert_eq!(safe_file_name("a/b\\c|d*e<f>g\"h"), "a b c d e f g h");
        assert_eq!(
            safe_file_name("tab\there\nnew\u{0}nul\u{7f}del"),
            "tab here new nul del"
        );
        assert_eq!(
            safe_file_name("  many   spaces \u{a0}\u{3000} here  "),
            "many spaces here"
        );
        assert_eq!(safe_file_name("Notes"), "Notes");
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
    }

    #[test]
    fn dots_and_spaces_at_the_ends_are_removed() {
        assert_eq!(safe_file_name("Notes."), "Notes");
        assert_eq!(safe_file_name("Notes . . "), "Notes");
        assert_eq!(safe_file_name(" . Notes"), "Notes");
        // A leading dot would hide the file.
        assert_eq!(safe_file_name(".hidden"), "hidden");
        assert_eq!(safe_file_name("..."), "Untitled");
        assert_eq!(safe_file_name(".."), "Untitled");
        assert_eq!(safe_file_name("."), "Untitled");
        // The last dot starts an extension, so the dot before it is a trailing one.
        assert_eq!(safe_file_name("a..b"), "a.b");
        assert_eq!(safe_stem("a..b"), "a..b");
    }

    #[test]
    fn emoji_and_other_scripts_are_kept() {
        assert_eq!(
            safe_file_name("🚀 Launch 日本語 café"),
            "🚀 Launch 日本語 café"
        );
        assert_eq!(safe_file_name("👩‍💻.png"), "👩‍💻.png");
    }

    #[test]
    fn reserved_device_names_get_an_underscore() {
        for name in [
            "CON", "con", "Prn", "AUX", "nul", "COM1", "com9", "LPT1", "lpt9",
        ] {
            assert_eq!(safe_file_name(name), format!("{name}_"), "{name}");
        }
        assert_eq!(safe_file_name("con.txt"), "con_.txt");
        assert_eq!(safe_file_name("NUL.tar.gz"), "NUL_.tar.gz");
        // Not reserved.
        for name in [
            "COM0", "COM10", "LPT", "CONSOLE", "Console", "Aux1", "xCON", "CON x",
        ] {
            assert_eq!(safe_file_name(name), name, "{name}");
        }
        // A title with dots is one stem: the part before the first dot decides.
        assert_eq!(safe_stem("con.v2 notes"), "con_.v2 notes");
        assert_eq!(safe_stem("v1.2 notes"), "v1.2 notes");
    }

    #[test]
    fn the_stem_is_cut_before_the_extension() {
        let long = "x".repeat(150);
        assert_eq!(safe_file_name(&long), "x".repeat(100));
        assert_eq!(
            safe_file_name(&format!("{long}.pdf")),
            format!("{}.pdf", "x".repeat(100))
        );
        // On a character boundary, not a byte one.
        let wide = "日".repeat(150);
        assert_eq!(safe_file_name(&wide), "日".repeat(100));
        let emoji = "🚀".repeat(150);
        assert_eq!(safe_file_name(&emoji).chars().count(), 100);
        // The cut does not leave a space or a dot at the end.
        let spaced = format!("{} tail", "y".repeat(99));
        assert_eq!(safe_file_name(&spaced), "y".repeat(99));
        // A title is never split at a dot, so it is cut whole.
        assert_eq!(
            safe_stem(&format!("{long}.not an extension")),
            "x".repeat(100)
        );
    }

    #[test]
    fn what_looks_like_an_extension_is() {
        assert_eq!(
            safe_parts("archive.tar.gz", true),
            ("archive.tar".to_owned(), ".gz".to_owned())
        );
        assert_eq!(
            safe_parts("Q3. final", true),
            ("Q3. final".to_owned(), String::new())
        );
        assert_eq!(
            safe_parts("noext", true),
            ("noext".to_owned(), String::new())
        );
        assert_eq!(safe_parts("a.verylongextension", true).1, "");
        assert_eq!(safe_parts(".pdf", true), ("pdf".to_owned(), String::new()));
        assert_eq!(safe_parts(" .pdf", true), ("pdf".to_owned(), String::new()));
        assert_eq!(
            safe_parts("a.pdf", false),
            ("a.pdf".to_owned(), String::new())
        );
    }

    #[test]
    fn an_empty_name_is_untitled() {
        for name in ["", " ", "///", "\t\n", "???", ":*?"] {
            assert_eq!(safe_file_name(name), "Untitled", "{name:?}");
        }
        // What is left of a name that is only an extension is its letters.
        assert_eq!(safe_file_name("?.pdf"), "pdf");
        assert_eq!(safe_file_name("? .pdf"), "pdf");
        assert_eq!(safe_file_name("a ?.pdf"), "a.pdf");
    }

    #[test]
    fn a_safe_name_is_stable() {
        for name in ["Report: Q3?", "con.txt", ".hidden", "a b.pdf", "  x  "] {
            let once = safe_file_name(name);
            assert_eq!(safe_file_name(&once), once, "{name}");
        }
    }

    // ----- relative links -----

    #[test]
    fn links_climb_and_descend() {
        assert_eq!(
            relative_href("Notes.md", "Notes/report.pdf"),
            "Notes/report.pdf"
        );
        assert_eq!(
            relative_href("Notes/Sub.md", "Notes/report.pdf"),
            "report.pdf"
        );
        assert_eq!(
            relative_href("Notes/Sub/Deep.md", "Notes/report.pdf"),
            "../report.pdf"
        );
        assert_eq!(
            relative_href("Notes/Sub/Deep.md", "Other.md"),
            "../../Other.md"
        );
        assert_eq!(relative_href("A/B/x.md", "A/C/y.md"), "../C/y.md");
        assert_eq!(relative_href("Notes.md", "Notes.md"), "Notes.md");
        assert_eq!(relative_href("A/x.md", "A/x.md"), "x.md");
        assert_eq!(relative_href("x.md", "y.md"), "y.md");
        // A folder named like the start of another path is not a common prefix.
        assert_eq!(
            relative_href("Notes/x.md", "Notes 2/y.md"),
            "../Notes%202/y.md"
        );
        // The file of the page is not mistaken for a folder.
        assert_eq!(
            relative_href("Notes.md", "Notes.md/inner"),
            "Notes.md/inner"
        );
    }

    #[test]
    fn each_segment_is_percent_encoded() {
        assert_eq!(
            relative_href("a.md", "My Notes/it's (1) [x] #1 50%.md"),
            "My%20Notes/it%27s%20%281%29%20%5Bx%5D%20%231%2050%25.md"
        );
        assert_eq!(
            relative_href("a.md", "q?.md/<a>\"b\"/^`{|}\\.png"),
            "q%3F.md/%3Ca%3E%22b%22/%5E%60%7B%7C%7D%5C.png"
        );
        // Not encoded: non-ASCII, and the characters a URL path keeps.
        assert_eq!(
            relative_href("a.md", "café 日本語/🚀.png"),
            "café%20日本語/🚀.png"
        );
        assert_eq!(
            relative_href("a.md", "a&b,c;d=e+f@g$h!i~j-k_l.md"),
            "a&b,c;d=e+f@g$h!i~j-k_l.md"
        );
        assert_eq!(relative_href("a.md", "tab\there.md"), "tab%09here.md");
    }

    // ----- layout -----

    #[test]
    fn one_page_with_nothing_else_is_a_single_file() {
        let pages = [page(1, None, "Weekly review")];
        let md = layout("Weekly review", &pages, ExportFormat::Markdown);
        assert!(md.single_file);
        assert_eq!(md.suggested_name, "Weekly review.md");
        assert_eq!(md.page_files[&pid(1)], "Weekly review.md");
        assert!(md.attachment_files.is_empty() && md.whiteboard_files.is_empty());
        assert_eq!(md.sidecar, None);
        let html = layout("Weekly review", &pages, ExportFormat::Html);
        assert!(html.single_file);
        assert_eq!(html.suggested_name, "Weekly review.html");
        assert_eq!(html.page_files[&pid(1)], "Weekly review.html");
    }

    #[test]
    fn a_single_page_named_by_its_own_title_not_the_export() {
        // A space with one page: the file is the page's.
        let pages = [page(1, None, "Only: page?")];
        let out = layout("My space", &pages, ExportFormat::Markdown);
        assert!(out.single_file);
        assert_eq!(out.suggested_name, "Only page.md");
        let untitled = [page(1, None, "")];
        assert_eq!(
            layout("x", &untitled, ExportFormat::Markdown).suggested_name,
            "Untitled.md"
        );
    }

    #[test]
    fn unavailable_attachments_keep_a_single_file() {
        let pages = [with_attachments(
            page(1, None, "Page"),
            &[(1, "gone.png", false)],
        )];
        let out = layout("Page", &pages, ExportFormat::Markdown);
        assert!(out.single_file);
        assert!(out.attachment_files.is_empty());
    }

    #[test]
    fn an_available_attachment_or_a_whiteboard_makes_an_archive() {
        let with_file = [with_attachments(
            page(1, None, "Page"),
            &[(1, "photo.png", true)],
        )];
        let out = layout("Page", &with_file, ExportFormat::Markdown);
        assert!(!out.single_file);
        assert_eq!(out.suggested_name, "Page.zip");
        assert_eq!(out.page_files[&pid(1)], "Page.md");
        assert_eq!(out.attachment_files[&aid(1)], "Page/photo.png");
        assert_eq!(out.sidecar.as_deref(), Some("atomik-export.json"));

        let with_board = [with_whiteboards(page(1, None, "Page"), &[1])];
        let out = layout("Page", &with_board, ExportFormat::Markdown);
        assert!(!out.single_file);
        assert_eq!(
            out.whiteboard_files[&(pid(1), bid(1))],
            "Page/Whiteboard.excalidraw"
        );
    }

    #[test]
    fn two_pages_are_an_archive_named_after_the_export() {
        let pages = [page(1, None, "One"), page(2, None, "Two")];
        let out = layout("Q3: plan/2026", &pages, ExportFormat::Markdown);
        assert!(!out.single_file);
        assert_eq!(out.suggested_name, "Q3 plan 2026.zip");
        assert_eq!(files(&out, &pages), ["One.md", "Two.md"]);
        assert_eq!(out.sidecar.as_deref(), Some("atomik-export.json"));
        // No folders for pages with nothing in them.
        assert!(out.page_files.values().all(|path| !path.contains('/')));
    }

    #[test]
    fn the_sidecar_is_for_markdown_archives_only() {
        let pages = [page(1, None, "One"), page(2, None, "Two")];
        assert_eq!(layout("x", &pages, ExportFormat::Html).sidecar, None);
        assert_eq!(
            layout("x", &pages, ExportFormat::Html).page_files[&pid(1)],
            "One.html"
        );
        assert!(
            layout("x", &pages, ExportFormat::Markdown)
                .sidecar
                .is_some()
        );
        // An empty export is an archive too.
        let none = layout("Empty", &[], ExportFormat::Markdown);
        assert!(!none.single_file && none.suggested_name == "Empty.zip" && none.sidecar.is_some());
    }

    #[test]
    fn children_go_in_their_parent_s_folder() {
        let pages = [
            page(1, None, "Projects"),
            page(2, Some(1), "Alpha"),
            page(3, Some(2), "Tasks"),
            page(4, Some(1), "Beta"),
            page(5, None, "Inbox"),
        ];
        let out = layout("Projects", &pages, ExportFormat::Markdown);
        assert_eq!(
            files(&out, &pages),
            [
                "Projects.md",
                "Projects/Alpha.md",
                "Projects/Alpha/Tasks.md",
                "Projects/Beta.md",
                "Inbox.md"
            ]
        );
    }

    #[test]
    fn a_page_without_a_parent_in_the_export_is_top_level() {
        // Its parent is not part of the export (exporting a subtree), or comes later.
        let pages = [page(2, Some(1), "Child"), page(3, Some(2), "Grandchild")];
        let out = layout("Child", &pages, ExportFormat::Markdown);
        assert_eq!(files(&out, &pages), ["Child.md", "Child/Grandchild.md"]);
        let backwards = [page(2, Some(3), "A"), page(3, None, "B")];
        let out = layout("x", &backwards, ExportFormat::Markdown);
        assert_eq!(files(&out, &backwards), ["A.md", "B.md"]);
    }

    #[test]
    fn duplicates_are_numbered_per_folder_ignoring_ascii_case() {
        let pages = [
            page(1, None, "Notes"),
            page(2, None, "notes"),
            page(3, None, "NOTES"),
            page(4, Some(1), "Sub"),
            page(5, Some(2), "Sub"),
            page(6, Some(1), "sub"),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(
            files(&out, &pages),
            [
                "Notes.md",
                "notes (2).md",
                "NOTES (3).md",
                "Notes/Sub.md",
                "notes (2)/Sub.md",
                "Notes/sub (2).md"
            ]
        );
    }

    #[test]
    fn a_numbered_page_numbers_its_folder_too() {
        let pages = [
            with_attachments(page(1, None, "Notes"), &[(1, "a.png", true)]),
            with_attachments(page(2, None, "Notes"), &[(2, "b.png", true)]),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(files(&out, &pages), ["Notes.md", "Notes (2).md"]);
        assert_eq!(out.attachment_files[&aid(1)], "Notes/a.png");
        assert_eq!(out.attachment_files[&aid(2)], "Notes (2)/b.png");
    }

    #[test]
    fn a_page_title_that_is_numbered_already_is_not_reused() {
        let pages = [
            page(1, None, "Notes (2)"),
            page(2, None, "Notes"),
            page(3, None, "Notes"),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(
            files(&out, &pages),
            ["Notes (2).md", "Notes.md", "Notes (3).md"]
        );
    }

    #[test]
    fn pages_come_before_attachments_and_whiteboards_in_a_folder() {
        // The attachment is named like the child page: the page keeps the name.
        let parent = with_whiteboards(
            with_attachments(
                page(1, None, "Parent"),
                &[(1, "Child.md", true), (2, "Whiteboard.excalidraw", true)],
            ),
            &[1, 2],
        );
        let pages = [parent, page(2, Some(1), "Child")];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(out.page_files[&pid(2)], "Parent/Child.md");
        assert_eq!(out.attachment_files[&aid(1)], "Parent/Child (2).md");
        assert_eq!(
            out.attachment_files[&aid(2)],
            "Parent/Whiteboard.excalidraw"
        );
        // Then the whiteboards, in document order, after the attachment that took the name.
        assert_eq!(
            out.whiteboard_files[&(pid(1), bid(1))],
            "Parent/Whiteboard (2).excalidraw"
        );
        assert_eq!(
            out.whiteboard_files[&(pid(1), bid(2))],
            "Parent/Whiteboard (3).excalidraw"
        );
    }

    #[test]
    fn attachments_keep_their_names_and_are_numbered_before_the_extension() {
        let pages = [with_attachments(
            page(1, None, "Page"),
            &[
                (1, "report.pdf", true),
                (2, "Report.PDF", true),
                (3, "REPORT.pdf", true),
                (4, "a/b:c.png", true),
                (5, "noext", true),
                (6, "NOEXT", true),
            ],
        )];
        let out = layout("x", &pages, ExportFormat::Markdown);
        let path = |n: u8| out.attachment_files[&aid(n)].clone();
        assert_eq!(path(1), "Page/report.pdf");
        assert_eq!(path(2), "Page/Report (2).PDF");
        assert_eq!(path(3), "Page/REPORT (3).pdf");
        assert_eq!(path(4), "Page/a b c.png");
        assert_eq!(path(5), "Page/noext");
        assert_eq!(path(6), "Page/NOEXT (2)");
    }

    #[test]
    fn an_attachment_may_not_take_the_name_of_a_folder() {
        // Child "Notes" has a folder of its own (its attachment): a file `notes` is refused.
        let pages = [
            with_attachments(page(1, None, "Parent"), &[(1, "notes", true)]),
            with_attachments(page(2, Some(1), "Notes"), &[(2, "x.png", true)]),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(out.page_files[&pid(2)], "Parent/Notes.md");
        assert_eq!(out.attachment_files[&aid(2)], "Parent/Notes/x.png");
        assert_eq!(out.attachment_files[&aid(1)], "Parent/notes (2)");
    }

    #[test]
    fn the_sidecar_name_is_reserved_at_the_top_level() {
        let pages = [
            with_attachments(page(1, None, "atomik-export.json"), &[(1, "a.png", true)]),
            page(2, None, "Other"),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        // The page file is `atomik-export.json.md`, its folder would be `atomik-export.json/`,
        // which the sidecar file `atomik-export.json` already takes.
        assert_eq!(out.page_files[&pid(1)], "atomik-export.json (2).md");
        assert_eq!(
            out.attachment_files[&aid(1)],
            "atomik-export.json (2)/a.png"
        );
        assert_eq!(out.sidecar.as_deref(), Some("atomik-export.json"));
        // An HTML archive has no sidecar, so the name is free.
        let html = layout("x", &pages, ExportFormat::Html);
        assert_eq!(html.page_files[&pid(1)], "atomik-export.json.html");
    }

    #[test]
    fn a_shared_attachment_is_written_once_in_the_first_pages_folder() {
        let shared = [(1, "logo.png", true)];
        let pages = [
            page(1, None, "Index"),
            with_attachments(page(2, Some(1), "First"), &shared),
            with_attachments(
                page(3, Some(1), "Second"),
                &[(1, "logo.png", true), (2, "own.png", true)],
            ),
            with_attachments(page(4, None, "Third"), &shared),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(out.attachment_files.len(), 2);
        assert_eq!(out.attachment_files[&aid(1)], "Index/First/logo.png");
        assert_eq!(out.attachment_files[&aid(2)], "Index/Second/own.png");
        // The pages that only link to it get no folder for it.
        assert_eq!(out.page_files[&pid(4)], "Third.md");
        assert_eq!(
            relative_href(&out.page_files[&pid(4)], &out.attachment_files[&aid(1)]),
            "Index/First/logo.png"
        );
        // `Index/Second.md` is next to the folder `Index/First/`, not inside it.
        assert_eq!(
            relative_href(&out.page_files[&pid(3)], &out.attachment_files[&aid(1)]),
            "First/logo.png"
        );
    }

    #[test]
    fn unavailable_attachments_have_no_file_in_an_archive() {
        let pages = [
            with_attachments(
                page(1, None, "A"),
                &[(1, "here.png", true), (2, "missing.png", false)],
            ),
            page(2, None, "B"),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert!(out.attachment_files.contains_key(&aid(1)));
        assert!(!out.attachment_files.contains_key(&aid(2)));
        // A page that only uses unavailable ones has no folder.
        let only = [
            with_attachments(page(1, None, "A"), &[(2, "missing.png", false)]),
            page(2, None, "B"),
        ];
        let out = layout("x", &only, ExportFormat::Markdown);
        assert!(out.attachment_files.is_empty());
        assert_eq!(files(&out, &only), ["A.md", "B.md"]);
    }

    #[test]
    fn an_available_attachment_an_unavailable_one_of_the_same_id_cannot_be_both() {
        // The first page sees it unavailable, the second available: the second places it.
        let pages = [
            with_attachments(page(1, None, "A"), &[(1, "f.png", false)]),
            with_attachments(page(2, None, "B"), &[(1, "f.png", true)]),
        ];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(out.attachment_files[&aid(1)], "B/f.png");
    }

    #[test]
    fn whiteboards_are_numbered_in_document_order() {
        let pages = [with_whiteboards(page(1, None, "Page"), &[7, 3, 5])];
        let out = layout("x", &pages, ExportFormat::Markdown);
        assert_eq!(
            out.whiteboard_files[&(pid(1), bid(7))],
            "Page/Whiteboard.excalidraw"
        );
        assert_eq!(
            out.whiteboard_files[&(pid(1), bid(3))],
            "Page/Whiteboard (2).excalidraw"
        );
        assert_eq!(
            out.whiteboard_files[&(pid(1), bid(5))],
            "Page/Whiteboard (3).excalidraw"
        );
    }

    #[test]
    fn hostile_titles_stay_inside_the_archive() {
        let pages = [
            page(1, None, "../../etc/passwd"),
            page(2, None, "C:\\Windows\\system32"),
            page(3, None, "/absolute"),
            page(4, None, ".."),
            page(5, Some(4), "ok"),
        ];
        let out = layout("../x", &pages, ExportFormat::Markdown);
        assert_eq!(out.suggested_name, "x.zip");
        for path in out.page_files.values() {
            assert!(!path.starts_with('/') && !path.contains(".."), "{path}");
            assert!(path.split('/').all(|segment| !segment.is_empty()), "{path}");
        }
        assert_eq!(out.page_files[&pid(1)], "etc passwd.md");
        assert_eq!(out.page_files[&pid(4)], "Untitled.md");
        assert_eq!(out.page_files[&pid(5)], "Untitled/ok.md");
    }

    #[test]
    fn every_path_segment_is_a_safe_name() {
        let pages = [
            with_whiteboards(
                with_attachments(
                    page(1, None, "CON"),
                    &[(1, "nul.txt", true), (2, "a?b.png", true)],
                ),
                &[1],
            ),
            page(2, Some(1), "LPT1"),
            page(3, Some(1), &"long ".repeat(60)),
        ];
        let out = layout("PRN", &pages, ExportFormat::Markdown);
        let mut all: Vec<&String> = out.page_files.values().collect();
        all.extend(out.attachment_files.values());
        all.extend(out.whiteboard_files.values());
        for path in all {
            for segment in path.split('/') {
                assert_eq!(safe_file_name(segment), segment, "{path}");
            }
        }
        assert_eq!(out.suggested_name, "PRN_.zip");
        assert_eq!(out.page_files[&pid(1)], "CON_.md");
        assert_eq!(out.attachment_files[&aid(1)], "CON_/nul_.txt");
    }
}
