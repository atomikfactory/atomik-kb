//! The fixed construction behind the page golden fixtures in `fixtures/doc/`.
//!
//! Shared by `examples/make_golden.rs` (writes `page-v1.snapshot.bin`, `page-v1.md`
//! and `page-v1-unknown.snapshot.bin`) and `tests/page_golden.rs` (checks them). The
//! portable JSON files are the inputs: `page-v1.portable.json` is the canonical
//! sample (every node and mark once), `page-v1-unknown.portable.json` adds a node and
//! a mark outside schema v1. Peer ids are fixed, so the snapshots are reproducible.

#![allow(dead_code)]

use kb_doc::{Doc, DocKind};
use kb_model::PageDoc;
use kb_types::{AttachmentId, PageId};

/// Peer of the replica that writes `page-v1.snapshot.bin`.
pub const PAGE_PEER: u64 = 0x0000_9a6e_0000_0001;
/// Peer of the replica that writes `page-v1-unknown.snapshot.bin`.
pub const UNKNOWN_PEER: u64 = 0x0000_9a6e_0000_0002;

/// The canonical sample page.
pub const PAGE_JSON: &str = include_str!("../../../../fixtures/doc/page-v1.portable.json");
/// A page with a node and a mark a newer client could write.
pub const UNKNOWN_JSON: &str =
    include_str!("../../../../fixtures/doc/page-v1-unknown.portable.json");

/// The page both references of the sample point to.
pub const LINKED_PAGE: &str = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
/// Its title in `page-v1.md`.
pub const LINKED_TITLE: &str = "Linked page";

/// The `image` block of the sample and its attachment.
pub const IMAGE_BLOCK: &str = "a9b8c7d6-e5f4-4a3b-9c2d-1e0f2a3b4c5d";
pub const IMAGE_ATTACHMENT: &str = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
/// The `file` block of the sample and its attachment.
pub const FILE_BLOCK: &str = "b0c9d8e7-f6a5-4b4c-8d3e-2f1a3b4c5d6e";
pub const FILE_ATTACHMENT: &str = "6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6";
/// The `gallery` block of the sample (after the file block) and the attachments of its second and
/// third pictures; its first picture is the image's attachment.
pub const GALLERY_BLOCK: &str = "e2f3a4b5-8071-4b92-8ca3-1d2e3f4a5b6c";
pub const GALLERY_SECOND: &str = "7b2e3f4a-5c6d-4e7f-8091-a2b3c4d5e6f7";
pub const GALLERY_THIRD: &str = "8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708";
/// The `whiteboard` block of the sample (after the gallery): a rectangle with its text, and a deleted ellipse.
pub const WHITEBOARD_BLOCK: &str = "f3a4b5c6-9182-4ca3-9db4-2e3f4a5b6c7d";
/// The `table` block of the sample (a header row of three `tableHeader` cells and two
/// body rows of `tableCell`s: one cell with two paragraphs, one with a `|`, one bold).
pub const TABLE_BLOCK: &str = "7b1e0c01-5d2a-4e01-9a01-3c4d5e6f7a01";
/// The `mathBlock` of the sample (one line of LaTeX, `E = mc^2`).
pub const MATH_BLOCK: &str = "6c2f1a08-4d3e-4b17-8a6f-5e7d9c0b1a24";
/// The `mathInline` of the sample (inline in the blockquote's paragraph: `E=mc^2`).
pub const MATH_INLINE_BLOCK: &str = "e51c0a01-8f4d-4a31-9d41-5b6c7d8e9f01";
/// The blockquote's paragraph of the sample, which holds the inline equation.
pub const QUOTE_PARAGRAPH: &str = "6fe319e4-d270-4e61-9eb3-7192a3b4c5d6";
/// The `columnList` block of the sample (two columns: one paragraph, then two paragraphs).
pub const COLUMN_LIST_BLOCK: &str = "c01a5d01-6e2b-4f01-8b01-9d3c4e5f6a01";
/// The `pageLink` block of the sample: a card to the linked page and its block.
pub const PAGE_LINK_BLOCK: &str = "d1b2c3a4-7f60-4a81-9b92-0c1d2e3f4a5b";
/// The `tableOfContents` block of the sample (the second to last block, a leaf with an `id`).
pub const TABLE_OF_CONTENTS_BLOCK: &str = "d01e7a01-7f3c-4a01-9c01-0e4d5f6a7b01";
/// The `breadcrumb` block of the sample (the last block, a leaf with an `id`).
pub const BREADCRUMB_BLOCK: &str = "d01e7a02-7f3c-4a02-9c02-0e4d5f6a7b02";
/// The `pageList` block of the sample (the last block, a leaf with an `id` and the configuration
/// of its view: a tag source, two properties shown, sorted by the second one, descending).
pub const PAGE_LIST_BLOCK: &str = "d01e7a03-7f3c-4a03-9c03-0e4d5f6a7b03";
/// The `templateButton` block of the sample (just before the three derived blocks: above, labelled "Add task") and its one paragraph, which
/// holds a person mention of `me` and a date mention of `today` with a reminder (ADR-0056).
pub const TEMPLATE_BUTTON_BLOCK: &str = "d01e7a04-7f3c-4a04-9c04-0e4d5f6a7b04";
pub const TEMPLATE_BUTTON_PARAGRAPH: &str = "d01e7a05-7f3c-4a05-9c05-0e4d5f6a7b05";
/// The file attachment's name in `page-v1.md`.
pub const FILE_NAME: &str = "Quarterly report.pdf";

/// Title resolution used for `page-v1.md`.
pub fn title_of(id: PageId) -> Option<String> {
    (id.to_string() == LINKED_PAGE).then(|| LINKED_TITLE.to_owned())
}

/// Attachment name resolution used for `page-v1.md`.
pub fn name_of(id: AttachmentId) -> Option<String> {
    (id.to_string() == FILE_ATTACHMENT).then(|| FILE_NAME.to_owned())
}

/// Parses a fixture.
pub fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("fixture JSON parses")
}

/// Builds a page from portable JSON with a fixed peer.
pub fn build(peer: u64, text: &str) -> PageDoc {
    PageDoc::from_portable(Doc::new(DocKind::Page, peer), &json(text)).expect("fixture builds")
}

/// The sample page.
pub fn page() -> PageDoc {
    build(PAGE_PEER, PAGE_JSON)
}

/// The page with unknown content.
pub fn unknown_page() -> PageDoc {
    build(UNKNOWN_PEER, UNKNOWN_JSON)
}

/// The Markdown golden of the sample (`page-v1.md`).
pub fn markdown(page: &PageDoc) -> String {
    page.to_markdown_with(&title_of, &mut name_of)
}
