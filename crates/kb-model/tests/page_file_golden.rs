//! Golden file of the Markdown file flavour (`fixtures/doc/page-v1.file.md`, ADR-0036): the
//! canonical sample page (`page-v1.portable.json`) rendered by `kb_model::render_markdown_file`
//! with links that answer fixed hrefs.
//!
//! The clipboard flavour has its own golden (`page-v1.md`, `tests/page_golden.rs`) and does not
//! change. **If this test fails, the file flavour changed**: that is a change of the format the
//! importer reads (`docs/architecture/import-export.md`, 2.3). When it is on purpose, regenerate
//! the file and review the diff like a protocol change:
//!
//! ```text
//! $env:KB_REGENERATE_FIXTURES = "1"; cargo test -p kb-model --test page_file_golden
//! ```
//!
//! The file is written only when `KB_REGENERATE_FIXTURES` is set (to anything); a test run
//! without it only compares.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

#[path = "support/page_golden.rs"]
mod page_golden;

use kb_model::{ExportLinks, render_markdown_file};
use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{AttachmentId, BlockId, OptionId, PageId, PropertyId};
use page_golden::{
    FILE_ATTACHMENT, FILE_NAME, GALLERY_SECOND, IMAGE_ATTACHMENT, LINKED_PAGE, LINKED_TITLE,
    PAGE_JSON, WHITEBOARD_BLOCK, json,
};

/// Where the golden lives, relative to this crate.
const GOLDEN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/doc/page-v1.file.md"
);

/// The property the sample's list view shows second and sorts by.
const PRIORITY_PROPERTY: &str = "5b2a7c14-9e30-4d68-8a15-3f6e0c9d2b74";
/// The Tags option of the sample's list view.
const WORK_OPTION: &str = "3c9d1e52-6a47-4b8e-9f20-5d1c7a8b4e61";

/// An export of `Welcome.md` with its folder `Welcome/`: the answers are fixed, relative to the
/// file, and percent-encoded as `kb_export::relative_href` writes them.
struct Links;

impl ExportLinks for Links {
    fn page_href(&self, page: PageId) -> Option<String> {
        (page.to_string() == LINKED_PAGE).then(|| "Linked%20page.md".to_owned())
    }

    fn page_title(&self, page: PageId) -> Option<String> {
        (page.to_string() == LINKED_PAGE).then(|| LINKED_TITLE.to_owned())
    }

    fn attachment_href(&self, attachment: AttachmentId) -> Option<String> {
        // The third picture of the gallery is not in the archive: its link stays `attachment://`.
        match attachment.to_string().as_str() {
            IMAGE_ATTACHMENT => Some("Welcome/diagram.png".to_owned()),
            FILE_ATTACHMENT => Some("Welcome/Quarterly%20report.pdf".to_owned()),
            GALLERY_SECOND => Some("Welcome/photo%20(2).jpg".to_owned()),
            _ => None,
        }
    }

    fn attachment_name(&self, attachment: AttachmentId) -> Option<String> {
        (attachment.to_string() == FILE_ATTACHMENT).then(|| FILE_NAME.to_owned())
    }

    fn whiteboard_href(&self, block: BlockId) -> Option<String> {
        (block.to_string() == WHITEBOARD_BLOCK).then(|| "Welcome/Whiteboard.excalidraw".to_owned())
    }

    fn property_name(&self, property: PropertyId) -> Option<String> {
        if property == TAGS_PROPERTY_ID {
            Some("Tags".to_owned())
        } else {
            (property.to_string() == PRIORITY_PROPERTY).then(|| "Priority".to_owned())
        }
    }

    fn option_name(&self, property: PropertyId, option: OptionId) -> Option<String> {
        (property == TAGS_PROPERTY_ID && option.to_string() == WORK_OPTION)
            .then(|| "work".to_owned())
    }
}

/// Line ends as the repository keeps them.
fn unix(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[test]
fn page_v1_file_markdown_is_the_golden() {
    let sample = json(PAGE_JSON);
    let rendered = render_markdown_file(&sample, &Links);
    // The same text from the document the sample builds (the portable form is an identity).
    assert_eq!(
        rendered,
        render_markdown_file(&page_golden::page().to_portable(), &Links)
    );

    if std::env::var_os("KB_REGENERATE_FIXTURES").is_some() {
        std::fs::write(GOLDEN_PATH, &rendered).unwrap();
        println!("wrote {GOLDEN_PATH} ({} bytes)", rendered.len());
        return;
    }
    let golden = std::fs::read_to_string(GOLDEN_PATH).unwrap_or_else(|_| {
        panic!("{GOLDEN_PATH} is missing: set KB_REGENERATE_FIXTURES=1 to write it")
    });
    assert_eq!(rendered, unix(&golden));
}

#[test]
fn the_file_flavour_leaves_the_clipboard_golden_alone() {
    let sample = json(PAGE_JSON);
    // The two flavours differ (links, annotations, escapes), and the clipboard one is the
    // golden of `tests/page_golden.rs`: here only that it is not the file flavour by accident.
    let file = render_markdown_file(&sample, &Links);
    let clipboard = page_golden::markdown(&page_golden::page());
    assert_ne!(file, clipboard);
    assert!(!clipboard.contains("<!-- atomik:"));
    assert!(file.contains("<!-- atomik:"));
}
