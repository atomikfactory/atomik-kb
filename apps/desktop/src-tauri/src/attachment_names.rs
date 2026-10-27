//! Names and types of attachments, as the shell derives and sanitises them (ADR-0020).
//!
//! Three jobs, all pure functions so a table can test them:
//!
//! - [`display_name`]: the name an imported file gets in the manifest, from its file name;
//! - [`path_safe_name`]: a stored name turned into something that is safe to use as the
//!   last component of a path (the suggestion of "Save as", the file "Open" writes). **A
//!   stored name is never used as a path without it**: the name lives in the encrypted
//!   manifest and may have come from anywhere (a pasted page, a restored backup, later a
//!   sync peer);
//! - [`mime_for_name`] and [`runs_code`]: the MIME type guessed from an extension, and
//!   whether the system would *run* a file with this name when it is "opened".

use kb_types::AttachmentId;
use kb_types::attachments::MAX_NAME_BYTES;

/// The MIME type of a file the table below does not know.
pub const UNKNOWN_MIME: &str = "application/octet-stream";

/// What an imported file is called when its file name leaves nothing usable.
const FALLBACK_NAME: &str = "file";

/// When a name has to be shortened, an extension up to this many bytes (dot included) is
/// kept and the part before it is cut instead.
const KEPT_EXTENSION_BYTES: usize = 16;

/// Extension (lowercase, no dot) to MIME type. Deliberately small: the type is a hint the
/// UI uses to choose an icon and to decide whether to preview, and anything missing is
/// `application/octet-stream`.
const MIME_TYPES: &[(&str, &str)] = &[
    // text and documents
    ("txt", "text/plain"),
    ("md", "text/markdown"),
    ("markdown", "text/markdown"),
    ("csv", "text/csv"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("css", "text/css"),
    ("js", "text/javascript"),
    ("json", "application/json"),
    ("xml", "application/xml"),
    ("pdf", "application/pdf"),
    ("rtf", "application/rtf"),
    ("epub", "application/epub+zip"),
    ("doc", "application/msword"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    ("xls", "application/vnd.ms-excel"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    ("ppt", "application/vnd.ms-powerpoint"),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    // images
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("ico", "image/vnd.microsoft.icon"),
    ("svg", "image/svg+xml"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("heic", "image/heic"),
    // audio and video
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("oga", "audio/ogg"),
    ("flac", "audio/flac"),
    ("m4a", "audio/mp4"),
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("webm", "video/webm"),
    ("ogv", "video/ogg"),
    ("mov", "video/quicktime"),
    ("avi", "video/x-msvideo"),
    ("mkv", "video/x-matroska"),
    // archives
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
    ("tar", "application/x-tar"),
    ("7z", "application/x-7z-compressed"),
    ("rar", "application/vnd.rar"),
    // fonts
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
];

/// Extensions (lowercase, no dot) of files that a desktop "open" runs as a program or a
/// script, or that install, mount or load something: executables, installers, shell and
/// script languages, shortcuts and settings files of Windows, launchers of macOS and Linux.
/// `attachment_open` refuses them; "Save as" writes them like any other file.
const RUNS_CODE: &[&str] = &[
    // Windows programs, installers and scripts
    "exe",
    "com",
    "scr",
    "pif",
    "bat",
    "cmd",
    "msi",
    "msp",
    "mst",
    "ps1",
    "ps1xml",
    "ps2",
    "ps2xml",
    "psc1",
    "psc2",
    "psd1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "wsc",
    "hta",
    "msc",
    "cpl",
    "inf",
    "reg",
    "dll",
    "sys",
    "jar",
    "py",
    "pyw",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "appinstaller",
    "gadget",
    "application",
    "appref-ms",
    // Windows shortcuts and search connectors (they start whatever they point at)
    "lnk",
    "url",
    "scf",
    "library-ms",
    "search-ms",
    // macOS and Linux launchers
    "sh",
    "bash",
    "zsh",
    "command",
    "terminal",
    "workflow",
    "app",
    "desktop",
    "run",
    "appimage",
];

/// What a Windows file name may not contain besides control characters and the path
/// separators; a saved file may travel to Windows, so no platform allows them.
const WINDOWS_ILLEGAL: [char; 7] = ['<', '>', ':', '"', '|', '?', '*'];

/// The name an imported file gets in the manifest, from the name of the file on disk.
///
/// Control characters (NUL included) and the path separators `/` and `\` are removed (the
/// core refuses names with them), a name longer than `MAX_NAME_BYTES` is shortened without
/// cutting a character (keeping a short extension), and a name that leaves nothing, or only
/// `.` or `..`, becomes `file`. Nothing else is changed: this is the name the user will see.
pub fn display_name(file_name: &str) -> String {
    let kept: String = file_name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\'))
        .collect();
    let name = cap_bytes(&kept, MAX_NAME_BYTES);
    if name.is_empty() || name == "." || name == ".." {
        FALLBACK_NAME.to_owned()
    } else {
        name
    }
}

/// A stored attachment name as the last component of a path.
///
/// - control characters and the path separators `/` and `\` are removed, and so are the
///   invisible direction and zero-width characters that make a name read differently from
///   what it is;
/// - the characters Windows does not allow (`< > : " | ? *`) become `_` (a `:` would make
///   `C:name` leave the directory and `name:stream` write to an alternate data stream);
/// - trailing dots and spaces are dropped (Windows drops them silently, and `..` is not a
///   name);
/// - a Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9`,
///   `CONIN$`, `CONOUT$`, with any extension) gets a `_` in front;
/// - the result is at most `MAX_NAME_BYTES` bytes, never cut inside a character, and keeps
///   a short extension when it can;
/// - a name that leaves nothing becomes the attachment id.
///
/// The rules are the same on every platform: a file saved here may be carried to Windows.
pub fn path_safe_name(stored: &str, id: AttachmentId) -> String {
    sanitised_name(stored).unwrap_or_else(|| id.to_string())
}

/// The rules of [`path_safe_name`] without its fallback: `None` when nothing is left of the
/// name. For a name that is not stored yet, such as the one an image downloaded from a link
/// is given (`url_import`), where the caller has its own fallback.
pub fn sanitised_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\') && !is_invisible_format(*c))
        .map(|c| if WINDOWS_ILLEGAL.contains(&c) { '_' } else { c })
        .collect();
    let mut name = trim_end(&cap_bytes(trim_end(&cleaned), MAX_NAME_BYTES)).to_owned();
    if name.is_empty() {
        return None;
    }
    if is_device_name(&name) {
        name.insert(0, '_');
        name = trim_end(&cap_bytes(&name, MAX_NAME_BYTES)).to_owned();
    }
    Some(name)
}

/// The MIME type for a file name, from its extension; `application/octet-stream` when the
/// name has none or the table does not know it. Always valid for the core: printable ASCII,
/// one `/`, well under 127 bytes.
pub fn mime_for_name(name: &str) -> &'static str {
    let Some(extension) = extension(name) else {
        return UNKNOWN_MIME;
    };
    MIME_TYPES
        .iter()
        .find(|(known, _)| *known == extension)
        .map_or(UNKNOWN_MIME, |(_, mime)| mime)
}

/// Whether the system would run a file with this name when it is opened (see `RUNS_CODE`).
/// `name` is the name the file will have on disk, i.e. after [`path_safe_name`].
pub fn runs_code(name: &str) -> bool {
    extension(name).is_some_and(|extension| RUNS_CODE.contains(&extension.as_str()))
}

/// The lowercase extension of a name, without the dot; `None` when there is none.
fn extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    (!extension.is_empty()).then(|| extension.to_ascii_lowercase())
}

/// Drops trailing dots and spaces.
fn trim_end(name: &str) -> &str {
    name.trim_end_matches(['.', ' '])
}

/// Whether Windows would read the name as a device (`CON`, `COM1.txt`, `lpt9 .log`, ...):
/// the part before the first dot, without trailing spaces, compared without regard to case.
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let numbered = upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"));
    numbered.is_some_and(|digit| {
        let mut chars = digit.chars();
        matches!(
            (chars.next(), chars.next()),
            (Some('0'..='9' | '¹' | '²' | '³'), None)
        )
    })
}

/// Direction overrides, isolates and zero-width characters: invisible, and they can make
/// `invoice\u{202e}fdp.exe` read as `invoiceexe.pdf`.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
    )
}

/// `name` cut to at most `max` bytes without splitting a character. A short extension is
/// kept (`very long name....pdf` stays a PDF) by cutting the part before it.
fn cap_bytes(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    if let Some(dot) = name.rfind('.') {
        let extension = &name[dot..];
        if dot > 0 && extension.len() <= KEPT_EXTENSION_BYTES {
            let stem = cut(&name[..dot], max - extension.len());
            return format!("{stem}{extension}");
        }
    }
    cut(name, max).to_owned()
}

/// The longest prefix of `text` that fits `max` bytes and ends on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn id() -> AttachmentId {
        "12345678-9abc-4def-8123-456789abcdef".parse().unwrap()
    }

    fn safe(name: &str) -> String {
        path_safe_name(name, id())
    }

    // --- sanitising for a path ---

    #[test]
    fn ordinary_names_pass_through_unchanged() {
        for name in [
            "photo.png",
            "Report 2026 (final).pdf",
            "archive.tar.gz",
            "no extension",
            ".bashrc",
            "été-naïve.txt",
            "日本語のファイル.docx",
            "emoji 🎉.png",
            "a",
            "COM10.txt",
            "CONSOLE.txt",
            "console",
            "LPT.txt",
            "aux-file",
        ] {
            assert_eq!(safe(name), name, "{name:?}");
        }
    }

    #[test]
    fn path_separators_and_control_characters_are_removed() {
        for (stored, expected) in [
            ("a/b.txt", "ab.txt"),
            ("a\\b.txt", "ab.txt"),
            ("../../etc/passwd", "....etcpasswd"),
            ("..\\..\\Windows\\win.ini", "....Windowswin.ini"),
            ("C:\\Windows\\win.ini", "C_Windowswin.ini"),
            ("tab\there.txt", "tabhere.txt"),
            ("line\nbreak.txt", "linebreak.txt"),
            ("nul\0byte.txt", "nulbyte.txt"),
            ("bell\u{7}.txt", "bell.txt"),
            ("c1\u{85}control.txt", "c1control.txt"),
            ("/absolute/path", "absolutepath"),
            ("\\\\server\\share\\file.txt", "serversharefile.txt"),
        ] {
            assert_eq!(safe(stored), expected, "{stored:?}");
        }
    }

    #[test]
    fn a_name_can_never_name_a_directory_or_leave_one() {
        for stored in [
            "", ".", "..", "...", " ", "  ", ". .", "/", "\\", "//", "\\\\", "../", "..\\", "\0",
            "\n",
        ] {
            let name = safe(stored);
            assert_eq!(name, id().to_string(), "{stored:?}");
        }
    }

    #[test]
    fn the_result_never_contains_a_separator_or_a_stream_or_drive_marker() {
        for stored in [
            "C:evil.txt",
            "name:stream",
            "name.txt:Zone.Identifier",
            "file.txt::$DATA",
            "a<b>c|d?e*f\"g.txt",
            "\\\\?\\C:\\x",
            "//host/share",
            "~/.ssh/id_rsa",
        ] {
            let name = safe(stored);
            for forbidden in ['/', '\\', ':', '<', '>', '|', '?', '*', '"', '\0'] {
                assert!(!name.contains(forbidden), "{stored:?} became {name:?}");
            }
            assert!(!name.is_empty());
            assert!(!name.ends_with(['.', ' ']), "{name:?}");
        }
    }

    #[test]
    fn characters_windows_forbids_become_underscores() {
        assert_eq!(safe("what? yes: no|maybe*.txt"), "what_ yes_ no_maybe_.txt");
        assert_eq!(safe("a<b>c\"d.txt"), "a_b_c_d.txt");
        assert_eq!(safe("C:evil.txt"), "C_evil.txt");
        assert_eq!(safe("name.txt:Zone.Identifier"), "name.txt_Zone.Identifier");
    }

    #[test]
    fn trailing_dots_and_spaces_are_dropped() {
        for (stored, expected) in [
            ("report.", "report"),
            ("report..", "report"),
            ("report. ", "report"),
            ("report ", "report"),
            ("report.txt.", "report.txt"),
            ("report.txt ...", "report.txt"),
            ("a . b . ", "a . b"),
        ] {
            assert_eq!(safe(stored), expected, "{stored:?}");
        }
        // Leading dots and spaces are legal names and stay.
        assert_eq!(safe(".hidden"), ".hidden");
        assert_eq!(safe(" lead.txt"), " lead.txt");
    }

    #[test]
    fn windows_device_names_get_an_underscore_whatever_the_case_or_extension() {
        for stored in [
            "CON",
            "con",
            "Con.txt",
            "PRN",
            "prn.log",
            "AUX",
            "aux.tar.gz",
            "NUL",
            "nul.txt",
            "COM1",
            "com9.txt",
            "LPT1",
            "lpt9.doc",
            "COM0",
            "CONIN$",
            "CONOUT$.txt",
            "COM\u{b9}",
            "LPT\u{b2}.txt",
            "COM\u{b3}",
            "CON .txt",
            "nul  .txt",
        ] {
            let name = safe(stored);
            assert_eq!(name, format!("_{stored}"), "{stored:?}");
            assert!(!is_device_name(&name), "{name:?}");
        }
        // Not devices: another word, or more than one digit.
        for stored in [
            "CONSOLE",
            "COM10",
            "LPT12.txt",
            "xCON",
            "CON-1",
            "COM",
            "LPT",
        ] {
            assert_eq!(safe(stored), stored, "{stored:?}");
        }
    }

    #[test]
    fn a_device_name_that_only_appears_after_cleaning_is_caught() {
        assert_eq!(safe("C/O/N"), "_CON");
        assert_eq!(safe("N\0U\0L.txt"), "_NUL.txt");
        assert_eq!(safe("CON..."), "_CON");
        assert_eq!(safe("con . "), "_con");
    }

    #[test]
    fn invisible_direction_and_zero_width_characters_are_removed() {
        assert_eq!(safe("invoice\u{202e}fdp.exe"), "invoicefdp.exe");
        assert_eq!(safe("a\u{200b}b\u{feff}c.txt"), "abc.txt");
        assert_eq!(safe("\u{2066}x\u{2069}.txt"), "x.txt");
        assert_eq!(safe("soft\u{ad}hyphen.txt"), "softhyphen.txt");
    }

    #[test]
    fn long_names_are_cut_to_255_bytes_on_a_character_boundary() {
        let long = "a".repeat(400);
        let name = safe(&long);
        assert_eq!(name.len(), MAX_NAME_BYTES);

        // 3-byte characters: 85 fit, 86 would be 258 bytes.
        let cjk = "日".repeat(100);
        let name = safe(&cjk);
        assert_eq!(name, "日".repeat(85));
        assert!(name.len() <= MAX_NAME_BYTES);

        // 4-byte characters.
        let emoji = "🎉".repeat(100);
        let name = safe(&emoji);
        assert_eq!(name.len(), 252);
        assert!(name.is_char_boundary(name.len()));
    }

    #[test]
    fn a_name_of_exactly_255_bytes_is_kept_whole() {
        let exact = format!("{}.txt", "x".repeat(251));
        assert_eq!(exact.len(), MAX_NAME_BYTES);
        assert_eq!(safe(&exact), exact);
    }

    #[test]
    fn a_short_extension_survives_the_cut() {
        let stored = format!("{}.pdf", "very long name ".repeat(30));
        let name = safe(&stored);
        assert_eq!(name.len(), MAX_NAME_BYTES);
        assert!(name.ends_with(".pdf"), "{name:?}");
        assert!(name.starts_with("very long name"), "{name:?}");

        // A long "extension" is not one: the whole name is cut.
        let stored = format!("name.{}", "e".repeat(400));
        let name = safe(&stored);
        assert_eq!(name.len(), MAX_NAME_BYTES);
        assert!(name.starts_with("name.e"), "{name:?}");
    }

    #[test]
    fn a_cut_that_leaves_trailing_dots_or_spaces_is_trimmed_again() {
        // 250 letters, five dots, then a tail too long to be an extension: the cut at 255
        // bytes falls inside the dots.
        let stored = format!("{}.....{}", "a".repeat(250), "b".repeat(24));
        assert_eq!(safe(&stored), "a".repeat(250));

        // The same with spaces between the dots.
        let stored = format!("{}. . . . .{}", "a".repeat(250), "b".repeat(24));
        let name = safe(&stored);
        assert!(name.len() <= MAX_NAME_BYTES, "{}", name.len());
        assert!(!name.ends_with(['.', ' ']), "{name:?}");
        assert!(name.starts_with("aaa"));
    }

    #[test]
    fn a_device_name_at_the_length_limit_stays_within_it() {
        let stored = format!("con.{}", "x".repeat(251));
        let name = safe(&stored);
        assert!(name.starts_with("_con."), "{name:?}");
        assert!(name.len() <= MAX_NAME_BYTES, "{}", name.len());
    }

    #[test]
    fn sanitising_twice_changes_nothing() {
        for stored in [
            "a/b.txt",
            "CON.txt",
            "report. ",
            "x:y",
            "日本語.pdf",
            "",
            "..",
            "con . ",
            "\u{202e}x",
        ] {
            let once = safe(stored);
            assert_eq!(safe(&once), once, "{stored:?}");
        }
    }

    #[test]
    fn the_fallback_is_the_attachment_id_text() {
        assert_eq!(safe(".."), "12345678-9abc-4def-8123-456789abcdef");
    }

    // --- the name an import stores ---

    #[test]
    fn an_imported_name_keeps_what_the_user_sees() {
        for name in [
            "photo.png",
            "CON.txt",
            "report.",
            "what? yes: no.txt",
            " lead.txt",
            ".bashrc",
            "日本語.pdf",
        ] {
            assert_eq!(display_name(name), name, "{name:?}");
        }
    }

    #[test]
    fn an_imported_name_loses_only_what_the_core_refuses() {
        assert_eq!(display_name("a/b.txt"), "ab.txt");
        assert_eq!(display_name("a\\b.txt"), "ab.txt");
        assert_eq!(display_name("nul\0byte.txt"), "nulbyte.txt");
        assert_eq!(display_name("tab\there"), "tabhere");
        assert_eq!(display_name(""), "file");
        assert_eq!(display_name("."), "file");
        assert_eq!(display_name(".."), "file");
        assert_eq!(display_name("/"), "file");
        assert_eq!(display_name("\0"), "file");
    }

    #[test]
    fn an_imported_name_is_cut_to_255_bytes_keeping_the_extension() {
        let stored = format!("{}.jpeg", "é".repeat(200));
        let name = display_name(&stored);
        assert!(name.len() <= MAX_NAME_BYTES, "{}", name.len());
        assert!(name.ends_with(".jpeg"), "{name:?}");
        assert!(name.starts_with('é'));

        let plain = display_name(&"b".repeat(1000));
        assert_eq!(plain.len(), MAX_NAME_BYTES);
    }

    #[test]
    fn an_imported_name_always_passes_the_cores_name_rules() {
        let long_ascii = "z".repeat(999);
        let long_cjk = "日".repeat(999);
        for stored in [
            "a/b",
            "..",
            ".",
            "",
            "x\\y",
            "\0",
            long_ascii.as_str(),
            long_cjk.as_str(),
        ] {
            let name = display_name(stored);
            assert!(
                !name.is_empty() && name.len() <= MAX_NAME_BYTES,
                "{stored:?}"
            );
            assert!(
                !name.contains(['/', '\\', '\0']) && name != "." && name != "..",
                "{name:?}"
            );
        }
    }

    // --- MIME types ---

    #[test]
    fn common_extensions_map_to_their_mime_types() {
        for (name, mime) in [
            ("photo.png", "image/png"),
            ("photo.PNG", "image/png"),
            ("photo.jpg", "image/jpeg"),
            ("photo.JPEG", "image/jpeg"),
            ("anim.gif", "image/gif"),
            ("pic.webp", "image/webp"),
            ("logo.svg", "image/svg+xml"),
            ("doc.pdf", "application/pdf"),
            ("notes.txt", "text/plain"),
            ("README.md", "text/markdown"),
            ("data.json", "application/json"),
            ("table.csv", "text/csv"),
            ("song.mp3", "audio/mpeg"),
            ("clip.mp4", "video/mp4"),
            ("pack.zip", "application/zip"),
            ("archive.tar.gz", "application/gzip"),
            (
                "letter.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            ("with.many.dots.png", "image/png"),
        ] {
            assert_eq!(mime_for_name(name), mime, "{name}");
        }
    }

    #[test]
    fn everything_else_is_an_octet_stream() {
        for name in [
            "",
            "noextension",
            "trailing.",
            ".gitignore",
            "weird.xyz123",
            "file.PNGX",
            "Makefile",
            "image.png.bak",
        ] {
            assert_eq!(mime_for_name(name), UNKNOWN_MIME, "{name:?}");
        }
    }

    #[test]
    fn every_mime_type_in_the_table_passes_the_cores_rules() {
        let mut seen = std::collections::HashSet::new();
        for (extension, mime) in MIME_TYPES {
            assert!(seen.insert(*extension), "{extension} is listed twice");
            assert_eq!(*extension, extension.to_ascii_lowercase(), "{extension}");
            assert!(!extension.contains('.'), "{extension}");
            assert!(
                !mime.is_empty() && mime.len() <= kb_types::attachments::MAX_MIME_BYTES,
                "{mime}"
            );
            assert!(mime.bytes().all(|b| (0x21..=0x7e).contains(&b)), "{mime}");
            assert_eq!(mime.matches('/').count(), 1, "{mime}");
            assert_eq!(*mime, mime.to_ascii_lowercase(), "{mime}");
        }
        assert!(UNKNOWN_MIME.contains('/'));
    }

    // --- files the system would run ---

    #[test]
    fn programs_scripts_and_shortcuts_are_recognised_whatever_the_case() {
        for name in [
            "setup.exe",
            "SETUP.EXE",
            "run.bat",
            "x.cmd",
            "install.msi",
            "script.ps1",
            "macro.vbs",
            "evil.js",
            "page.hta",
            "link.lnk",
            "web.url",
            "tool.jar",
            "lib.dll",
            "deploy.sh",
            "Thing.command",
            "Thing.app",
            "launcher.desktop",
            "app.AppImage",
            "double.pdf.exe",
            "settings.reg",
        ] {
            assert!(runs_code(name), "{name}");
        }
    }

    #[test]
    fn documents_images_and_archives_are_not_in_the_list() {
        for name in [
            "photo.png",
            "doc.pdf",
            "letter.docx",
            "pack.zip",
            "notes.txt",
            "page.html",
            "data.json",
            "song.mp3",
            "noextension",
            "exe",
            "trailing.",
            "",
            "exe.txt",
        ] {
            assert!(!runs_code(name), "{name}");
        }
    }

    #[test]
    fn every_extension_in_the_list_is_lowercase_without_a_dot() {
        let mut seen = std::collections::HashSet::new();
        for extension in RUNS_CODE {
            assert!(seen.insert(*extension), "{extension} is listed twice");
            assert_eq!(*extension, extension.to_ascii_lowercase());
            assert!(!extension.starts_with('.'));
        }
    }
}
