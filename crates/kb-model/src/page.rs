//! The page document (shape v1, ADR-0016, `docs/development/milestone-03.md`, "Page
//! document" and "Model").
//!
//! A [`PageDoc`] wraps a [`kb_doc::Doc`] of kind [`DocKind::Page`] holding exactly what
//! the vendored loro-prosemirror binding writes: the root map `doc` with `nodeName`,
//! `attributes` and `children`, element nodes as the same triple, text runs as
//! `LoroText` with marks. This type never interprets node names beyond the few it
//! renders to Markdown or creates through the semantic operations; everything else
//! passes through untouched so that newer clients can add block kinds.
//!
//! Reading conventions (shared by every reader, see `page/read.rs`):
//!
//! - A node "has an id" when `attributes.id` is a string that parses as a
//!   [`BlockId`]. Duplicated ids (possible after concurrent block moves) are reported
//!   by [`PageDoc::check_invariants`]; readers list every occurrence.
//! - Adjacent `LoroText`s (possible after concurrent edits) read as one text: block
//!   text concatenates them, portable JSON merges runs with equal marks.
//! - A `null` mark value (left by an unmark) means "no mark"; null attributes do not
//!   exist.
//! - Children lists may only hold element maps and texts; anything else, and nodes
//!   nested deeper than 128 levels, is skipped by readers and reported by
//!   `check_invariants`.
//!
//! **The root triple is mergeable, element triples are regular** (shape v1). The
//! root's `attributes` and `children` are mergeable child containers
//! (`ensure_mergeable_map` / `ensure_mergeable_list`: their container ids derive from
//! the root and the key), so devices that create the same page document concurrently
//! (`PageDoc::new`, [`PageDoc::from_portable`], or a [`PageDoc::insert_block`] into a
//! document without a root) write into the same list, and after a merge every
//! replica's first blocks are there (two empty paragraphs after two `new`s, texts
//! typed into them included). Element nodes keep regular child containers created
//! with `insert_container`, exactly as the binding writes them. Readers go by key and
//! accept both kinds everywhere: a root written with regular containers (the binding's
//! `getOrCreateContainer`, older TypeScript mirrors) reads and edits the same. Only a
//! mixed concurrent creation (one regular root, one mergeable root) still resolves the
//! root fields last-writer-wins. See `concurrent_creations_merge_both_first_paragraphs`
//! in `tests/page_convergence.rs`.

pub(crate) mod date;
mod index;
mod markdown;
mod portable;
mod read;
mod remap;
pub mod views;
mod whiteboard;
mod write;

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use kb_doc::{Doc, DocKind, MarkExpand, PAGE_MARKS};
use kb_types::{AccountId, AttachmentId, BlockId, PageId};
use loro::{Container, LoroList, LoroMap, LoroText, LoroValue, TextDelta, ValueOrContainer};
use serde_json::Value;

use crate::ModelError;
use crate::meta::engine;

use self::index::{Index, Top};
use self::read::{Child, Element, Mode};
use self::write::{NewChild, NewNode};

pub use self::markdown::{
    ANNOTATION_CLOSE, ANNOTATION_OPEN, ExportLinks, render_file as render_markdown_file,
};
pub use self::remap::{
    CopyIds, LocalMoment, TemplateFill, TemplateMe, fill_template_content, portable_block_ids,
    remap_portable,
};
pub use self::whiteboard::WhiteboardElement;

/// Container and key names of shape v1 (the binding's constants). Frozen by the
/// fixtures under `fixtures/doc/`.
pub mod shape {
    /// Root map of a page document.
    pub const ROOT: &str = "doc";
    /// Node name key of an element node (`"doc"`, `"paragraph"`, ).
    pub const NODE_NAME: &str = "nodeName";
    /// Attributes map of an element node. The root triple is mergeable, element
    /// triples are regular: on the root this is a mergeable child container
    /// (`ensure_mergeable_map`), so that concurrent creations of a page document merge;
    /// on element nodes a regular one (`insert_container`), as the binding writes it.
    /// Readers accept both kinds.
    pub const ATTRIBUTES: &str = "attributes";
    /// Children list of an element node: mergeable on the root
    /// (`ensure_mergeable_list`), regular on element nodes (see [`ATTRIBUTES`]).
    pub const CHILDREN: &str = "children";
    /// Attribute holding the block id (UUID string).
    pub const ID: &str = "id";
    /// Node name of the root.
    pub const DOC: &str = "doc";
    /// Node name of the inline page reference.
    pub const PAGE_REF: &str = "pageRef";
    /// `pageRef` attribute: referenced page (UUID string).
    pub const PAGE_ID: &str = "pageId";
    /// `pageRef` attribute: referenced block (UUID string), optional.
    pub const BLOCK_ID: &str = "blockId";
    /// Node name of the page link block: a leaf block (a card) that links to a page like
    /// an inline `pageRef` does, with the same `pageId` and `blockId` attributes (ADR-0016,
    /// amendment of 2026-10-06).
    pub const PAGE_LINK: &str = "pageLink";
    /// Node name of the inline date mention (`dateMention { id, date, time? }`, an inline atom
    /// with no content and no marks; ADR-0016, amendment of 2026-10-07). The date is a
    /// *floating local date*: a calendar day with no time zone, so it reads the same on every
    /// device; it may carry a `reminder` (minutes before its moment; ADR-0016, amendment of
    /// 2026-10-07 for reminders, ADR-0024), which is not text and is not rendered anywhere
    /// but the editor's chip and the derived `date_mentions` table. It reads as its ISO text (`2026-10-07`, or
    /// `2026-10-07 14:30` with a time) wherever the core reads text: [`super::PageDoc::blocks`]
    /// folds it into the surrounding [`super::Inline::Text`], so the text index and the
    /// backlinks' text hold it, and Markdown renders the same text. Additive to shape v1;
    /// readers that do not know it pass it through.
    pub const DATE_MENTION: &str = "dateMention";
    /// `dateMention` attribute: the day, `YYYY-MM-DD`, a floating local date (a string).
    pub const DATE: &str = "date";
    /// `dateMention` attribute: the time of day, `HH:mm`, floating, optional (a string).
    pub const TIME: &str = "time";
    /// `dateMention` attribute: the reminder, an integer number of minutes before the
    /// mention's moment (`0` = at the time), optional: absent means no reminder. Any
    /// non-negative integer is valid storage (ADR-0024).
    pub const REMINDER: &str = "reminder";
    /// `dateMention` attribute (M13 slice 2, ADR-0047): the account whose devices fire the reminder in
    /// a shared space, an account id as the other ids are written (a UUID string), optional. Absent
    /// means the Owners' devices fire it in a shared space and this device in a personal one. Not
    /// text. Additive to shape v1; readers that do not know it ignore it.
    pub const REMINDER_FOR: &str = "reminderFor";
    /// `dateMention` and `personMention` attribute (ADR-0056, templates): a value filled in when a page is made from a
    /// template or a template button is clicked, a string, optional. A date mention takes `"today"` or `"now"`
    /// (its `date`, and for `now` its `time`, become the local day and time of that moment), a person mention
    /// `"me"` (its `accountId` and `label` become the person's who makes the page). The mention's other attributes
    /// stay valid values (those of the moment it was written or last resolved). Not text, not stored when unset;
    /// additive to shape v1, older builds drop it when they edit the node.
    pub const RELATIVE: &str = "relative";
    /// Node name of the template button block (`templateButton { id, label, position }`, content `block+`;
    /// ADR-0056): a button that holds blocks and inserts a fresh copy of them, below or above itself, when it is
    /// clicked. Its content is template material: the readers that feed search, backlinks and reminders
    /// ([`super::PageDoc::blocks`], [`super::PageDoc::links`], [`super::PageDoc::date_mentions`]) skip it. Additive to
    /// shape v1; readers that do not know it pass it through (an older client opens a page that holds one
    /// read-only, ADR-0016 open question 17).
    pub const TEMPLATE_BUTTON: &str = "templateButton";
    /// `templateButton` attribute: the label of the button, a string of at most 80 characters; empty or absent shows
    /// the default label.
    pub const BUTTON_LABEL: &str = "label";
    /// `templateButton` attribute: where a click inserts the copy, `"below"` or `"above"`; absent (or anything else) reads as
    /// `"below"`.
    pub const POSITION: &str = "position";
    /// Node name of the inline person mention (`personMention { accountId, label }`, an inline atom
    /// with no content, no marks and **no block id**; M13 slice 2, ADR-0049): a reference to a member
    /// by account id, so it survives renames. It reads as its [`LABEL`] wherever the core reads text
    /// (the text index holds the label; Markdown and HTML export `@label`). Additive to shape v1,
    /// `FORMAT_VERSION` stays 1; readers that do not know it pass it through (an older client opens a
    /// page that holds one read-only, ADR-0016 open question 17).
    pub const PERSON_MENTION: &str = "personMention";
    /// `personMention` attribute: the mentioned account (a UUID string). A mention without a valid id
    /// shows its label; `check_invariants` reports it.
    pub const ACCOUNT_ID: &str = "accountId";
    /// `personMention` attribute: the member's display name when the mention was inserted, 1 to 100
    /// characters: a fallback for the chip when the account is no longer a member ("Former member
    /// (label)"), and the text the index and the exports use.
    pub const LABEL: &str = "label";
    /// Node name of the inline equation (`mathInline { id, latex }`, an inline atom with no content
    /// and no marks; ADR-0016, amendment of 2026-10-07): a LaTeX formula inside a line of text,
    /// typeset by the editor's view and stored as its source only. It reads as that source
    /// wherever the core reads text: [`super::PageDoc::blocks`] folds it into the surrounding
    /// [`super::Inline::Text`], so the text index and the backlinks' text hold `E=mc^2`, and
    /// Markdown renders it as `$latex$`. Additive to shape v1; readers that do not know it pass it
    /// through.
    pub const MATH_INLINE: &str = "mathInline";
    /// `mathInline` attribute: the LaTeX source (a string, empty while the equation is being
    /// written).
    pub const LATEX: &str = "latex";
    /// Node name of a hard line break.
    pub const HARD_BREAK: &str = "hardBreak";
    /// Node name of the image block (leaf, Milestone 5, ADR-0020).
    pub const IMAGE: &str = "image";
    /// Node name of the file block (leaf, Milestone 5, ADR-0020).
    pub const FILE: &str = "file";
    /// `image`/`file` attribute: the attachment (UUID string).
    pub const ATTACHMENT_ID: &str = "attachmentId";
    /// `image` attribute: alternative text (a string, possibly empty).
    pub const ALT: &str = "alt";
    /// `image` attribute: display width in CSS pixels (integer), absent when unset.
    pub const WIDTH: &str = "width";
    /// `image` attribute (ADR-0016, amendment of 2026-10-09; ADR-0028): display width as a
    /// share of the block's container, an integer from 10 to 100 (`50.0` is 50), absent when
    /// unset. It wins over [`WIDTH`] when both are present; it is layout only, not text.
    pub const WIDTH_PERCENT: &str = "widthPercent";
    /// `image` attribute (ADR-0028): where the picture sits in its block, one of `"left"`,
    /// `"center"` or `"right"`, absent when unset (= centre). Layout only, not text.
    pub const ALIGN: &str = "align";
    /// `image` attribute (ADR-0028): the caption shown under the picture, a string of one
    /// line of plain text, absent when unset (an empty one counts as none). Unlike the alt
    /// text it is what everyone reads, and it is text: it is indexed and exported.
    pub const CAPTION: &str = "caption";
    /// `file` attribute (ADR-0016, amendment of 2026-10-09; ADR-0031): how the file is shown,
    /// `"preview"` (an inline viewer, for a PDF) or absent (= the file card). Layout only,
    /// not text; stored only when set.
    pub const VIEW: &str = "view";
    /// The photo gallery block (ADR-0016, amendment of 2026-10-09; ADR-0030).
    pub const GALLERY: &str = "gallery";
    /// `gallery` attribute: the pictures in order, a list of plain maps
    /// `{ attachmentId, caption? }` set whole (like `pageList`'s `properties` and `sort`).
    pub const ITEMS: &str = "items";
    /// `gallery` attribute: the number of columns of the grid, an integer from 2 to 4,
    /// absent = 3.
    pub const COLUMNS: &str = "columns";
    /// The whiteboard block (ADR-0016, amendment of 2026-10-10; ADR-0033): a leaf block whose
    /// elements live in the root map [`WHITEBOARDS`], not in the node.
    pub const WHITEBOARD: &str = "whiteboard";
    /// `whiteboard` attribute: the height of the inline view in CSS pixels, an integer from
    /// 160 to 1200, absent = 360. Layout only.
    pub const HEIGHT: &str = "height";
    /// Root map of the page document that holds the whiteboard elements, one plain map value
    /// per element under `<block id>/<element id>`. Not part of the ProseMirror tree.
    pub const WHITEBOARDS: &str = "whiteboards";
    /// Node name of a toggle's summary line (the first child of a `details`; content `text*`).
    pub const DETAILS_SUMMARY: &str = "detailsSummary";
    /// `detailsSummary` attribute (ADR-0016, amendment of 2026-10-14; ADR-0040): the level of a
    /// toggle heading, an integer from 1 to 3 (`2.0` is 2), absent for a plain toggle. Layout
    /// only, not text: the summary reads as the same text with or without it. Not stored when
    /// unset (the editor deletes the key). Additive to shape v1; an older editor shows a plain
    /// toggle.
    pub const LEVEL: &str = "level";
}

/// The block schema v1 as the core knows it (`fixtures/doc/block-schema-v1.json` is
/// the editor's view; a test compares them).
pub mod schema {
    use super::{MarkExpand, PAGE_MARKS};

    /// Marks with their expansion, in fixed order.
    pub const MARKS: &[(&str, MarkExpand)] = PAGE_MARKS;

    /// Every node name of shape v1, in fixture order.
    pub const NODE_NAMES: &[&str] = &[
        "doc",
        "paragraph",
        "heading",
        "bulletList",
        "orderedList",
        "listItem",
        "taskList",
        "taskItem",
        "blockquote",
        "codeBlock",
        "horizontalRule",
        "hardBreak",
        "details",
        "detailsSummary",
        "detailsContent",
        "callout",
        "pageRef",
        "text",
        "breadcrumb",
        "column",
        "columnList",
        "dateMention",
        "file",
        "gallery",
        "image",
        "mathBlock",
        "mathInline",
        "pageLink",
        "pageList",
        "personMention",
        "table",
        "tableCell",
        "tableHeader",
        "tableOfContents",
        "tableRow",
        "templateButton",
        "whiteboard",
    ];

    /// Node names that carry an `id` attribute.
    pub const ID_BEARING: &[&str] = &[
        "paragraph",
        "heading",
        "bulletList",
        "orderedList",
        "listItem",
        "taskList",
        "taskItem",
        "blockquote",
        "codeBlock",
        "mathBlock",
        "horizontalRule",
        "details",
        "detailsSummary",
        "detailsContent",
        "callout",
        "pageRef",
        "pageLink",
        "image",
        "file",
        "gallery",
        "whiteboard",
        "table",
        "tableRow",
        "tableCell",
        "tableHeader",
        "columnList",
        "column",
        "tableOfContents",
        "breadcrumb",
        "pageList",
        "dateMention",
        "mathInline",
        "templateButton",
    ];

    /// Leaf blocks that reference an attachment through `attachmentId` (Milestone 5,
    /// ADR-0020): `image { id, attachmentId, alt, width?, widthPercent?, align?, caption? }`
    /// (the last three added 2026-10-09, ADR-0028) and `file { id, attachmentId, view? }` (`view`
    /// added 2026-10-09, ADR-0031). Additive
    /// to shape v1: they are allowed wherever blocks are, and readers that do not know them
    /// pass them through.
    pub const ATTACHMENT_NODES: &[&str] = &["image", "file"];

    /// The photo gallery (ADR-0016, amendment of 2026-10-09; ADR-0030): `gallery { id, items,
    /// columns? }`, a leaf block that shows several pictures in a grid. `items` is a list of
    /// `{ attachmentId, caption? }` set whole, `columns` an integer from 2 to 4 (absent = 3).
    /// Every attachment it lists counts as referenced (the derived reference index) and its
    /// captions are its text. Additive to shape v1; readers that do not know it pass it through.
    pub const GALLERY_NODE: &str = "gallery";

    /// The simple table (ADR-0016, amendment of 2026-10-05): `table` (content
    /// `tableRow+`), `tableRow` (`(tableCell | tableHeader)*`) and the cells `tableCell`
    /// and `tableHeader` (`block+`; the editor keeps `colspan`, `rowspan`, `colwidth` and
    /// `align` at their defaults). Additive to shape v1, every one carries an `id`. A
    /// table holds its text in the paragraphs inside its cells, so text indexing and
    /// backlinks need nothing beyond the generic reading of nodes; readers that do not
    /// know the nodes pass them through.
    pub const TABLE_NODES: &[&str] = &["table", "tableRow", "tableCell", "tableHeader"];

    /// The columns layout (ADR-0016, amendment of 2026-10-06): `columnList` (content
    /// `column{2,5}`, group block) and `column` (content `block+`). Additive to shape v1,
    /// both carry an `id` and nothing else. Like the table they hold no text of their own:
    /// the text of a column lives in the blocks inside it, so text indexing and backlinks
    /// need nothing beyond the generic reading of nodes. A column list never sits inside a
    /// column in the editor, but the shape allows it and every reader takes the document as
    /// it is. Readers that do not know the nodes pass them through.
    pub const COLUMN_NODES: &[&str] = &["columnList", "column"];

    /// The derived blocks (ADR-0016, amendment of 2026-10-06): `tableOfContents` (a list of
    /// the page's headings) and `breadcrumb` (the page's place in the page tree), and, since the
    /// amendment of 2026-10-07, `pageList` (a saved view over a set of pages: the list view).
    /// All are leaf blocks (no content, no marks) whose view the editor draws from the page and
    /// the tree: the document stores the block and nothing of what it shows (the first two
    /// carry the attribute `id` only; `pageList` also holds the configuration of its view,
    /// `source`, `layout`, `properties` and `sort`, which the core passes through like any other
    /// attribute). They hold no text and no reference, so text indexing and backlinks need
    /// nothing beyond the generic reading of nodes, and Markdown renders them as the empty
    /// string. Additive to shape v1; readers that do not know them pass them through.
    pub const DERIVED_NODES: &[&str] = &["tableOfContents", "breadcrumb", "pageList"];

    /// The math equation block (ADR-0016, amendment of 2026-10-06): `mathBlock` (content
    /// `text*`, no marks, attribute `id` only). It holds the LaTeX source as plain text,
    /// like a `codeBlock` holds code, so the text index and `set_block_text` treat it as a
    /// text block. Additive to shape v1; readers that do not know it pass it through.
    pub const MATH_BLOCK: &str = "mathBlock";

    /// Node names whose content is text ([`super::PageDoc::set_block_text`] targets).
    pub const TEXT_NODES: &[&str] = &[
        "paragraph",
        "heading",
        "codeBlock",
        "mathBlock",
        "detailsSummary",
    ];

    /// Node names whose content is inline (text, `pageRef`, `dateMention`, `mathInline`,
    /// `hardBreak`), the targets of [`super::PageDoc::insert_page_ref`].
    pub const INLINE_CONTENT_NODES: &[&str] = &["paragraph", "heading"];
}

/// One piece of a block's inline content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// A text run (marks dropped). A date mention (2026-10-07) is part of the run around it,
    /// as its ISO text (`2026-10-07`, `2026-10-07 14:30`), and so is an inline equation, as its
    /// LaTeX source.
    Text(String),
    /// An inline page reference (a `pageLink` block reads as one, see
    /// [`PageDoc::blocks`]).
    PageRef {
        /// Referenced page.
        page_id: PageId,
        /// Referenced block, if any.
        block_id: Option<BlockId>,
    },
    /// A hard line break.
    Break,
}

/// A node with an `id`, with its direct inline content (empty for container blocks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockText {
    /// Block id.
    pub id: BlockId,
    /// Node name (`"paragraph"`, `"heading"`, ...).
    pub kind: String,
    /// Direct text runs, references and breaks, in order.
    pub inlines: Vec<Inline>,
}

/// A page reference attributed to the nearest enclosing block with an id (a `pageLink`
/// block is attributed to itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLink {
    /// Referencing block.
    pub from_block: BlockId,
    /// Referenced page.
    pub to_page: PageId,
    /// Referenced block, if any.
    pub to_block: Option<BlockId>,
}

/// A date mention attributed to the nearest enclosing block with an id: what the derived
/// `date_mentions` table holds (ADR-0024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateMention {
    /// The block that holds the mention (the paragraph, usually).
    pub block_id: BlockId,
    /// The mention itself (the `id` of the `dateMention` node).
    pub node_id: BlockId,
    /// The day, `YYYY-MM-DD` (always a valid date here).
    pub date: String,
    /// The time of day, `HH:mm`; `None` when the mention has none or its time is malformed.
    pub time: Option<String>,
    /// Minutes before the moment that a reminder fires; `None` for no reminder (also for
    /// a damaged one, which `check_invariants` reports).
    pub reminder_minutes: Option<i64>,
    /// When the reminder fires, in seconds since 1970-01-01T00:00:00 on the local wall clock
    /// (the moment, `date` at `time` or at 09:00, minus the reminder; see
    /// [`PageDoc::date_mentions`]); `None` exactly when there is no reminder.
    pub fire_at_local: Option<i64>,
    /// The account whose devices fire the reminder (the `reminderFor` attribute, ADR-0047); `None`
    /// when absent or not a valid id. Meaningful with a reminder only.
    pub reminder_for: Option<AccountId>,
}

/// A person mention attributed to the nearest enclosing block with an id: what the derived
/// `person_mentions` table holds (M13 slice 2, ADR-0049).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonMention {
    /// The block that holds the mention (the paragraph, usually).
    pub block_id: BlockId,
    /// The mentioned account (always a valid id here).
    pub account_id: AccountId,
    /// The label stored at insertion (the fallback name).
    pub label: String,
}

/// Block kinds the semantic operations can create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// `paragraph`.
    Paragraph,
    /// `heading` with a level 1..=3.
    Heading(u8),
    /// `codeBlock` with an optional language.
    CodeBlock {
        /// Language attribute.
        language: Option<String>,
    },
    /// `blockquote` holding one paragraph.
    Quote,
    /// `horizontalRule`.
    Divider,
    /// `image` (leaf): `attachmentId`, `alt` (the inserted text, possibly empty) and
    /// `width` when set (`widthPercent`, `align` and `caption`, ADR-0028, are written by
    /// the editor only).
    Image {
        /// The attachment shown.
        attachment_id: AttachmentId,
        /// Display width in CSS pixels; `None` = natural size (attribute not stored).
        width: Option<u32>,
    },
    /// `file` (leaf): `attachmentId`; holds no text (the name comes from the manifest).
    File {
        /// The attachment offered.
        attachment_id: AttachmentId,
    },
}

/// A page's content document.
///
/// Readers ([`Self::blocks`], [`Self::to_portable`], ...) always read the engine
/// document, so they see imported changes without any refresh. The semantic
/// operations commit before returning.
pub struct PageDoc {
    doc: Doc,
    /// Block-id index, valid while the document is unchanged (`page/index.rs`). A
    /// mutex rather than a `RefCell` keeps the type `Sync` like the other documents.
    index: Mutex<Option<Index>>,
}

impl std::fmt::Debug for PageDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageDoc").finish_non_exhaustive()
    }
}

impl PageDoc {
    /// A new page document: the root plus one empty paragraph whose id is
    /// `first_block`. Created by the core on first open, never by the editor.
    ///
    /// The document must be empty (no root yet). The root's `attributes` and
    /// `children` are mergeable (see [`shape::CHILDREN`]), so two devices creating the
    /// document of one page concurrently keep both first paragraphs.
    pub fn new(doc: Doc, first_block: BlockId) -> Result<Self, ModelError> {
        let page = Self::wrap(doc)?;
        let root = page.root();
        if !root.is_empty() {
            return Err(ModelError::Shape(
                "a new page document must start empty".into(),
            ));
        }
        let node = NewNode {
            name: shape::DOC.to_owned(),
            attrs: Vec::new(),
            content: vec![NewChild::Node(paragraph(first_block, ""))],
        };
        write::init_root(&root, &node)?;
        page.commit();
        Ok(page)
    }

    /// Wraps a loaded document, checking its kind and that the root is a `doc` node
    /// (an empty document is accepted and treated as having no blocks).
    pub fn from_doc(doc: Doc) -> Result<Self, ModelError> {
        Self::wrap(doc)
    }

    /// The engine document (for persistence and export).
    #[must_use]
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// No text, no references, no date mentions, no person mentions, no inline equations, no breaks,
    /// no image, file, page link or template button blocks anywhere.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        fn has_content(element: &Element) -> bool {
            element.is(shape::PAGE_REF)
                || element.is(shape::PAGE_LINK)
                || element.is(shape::DATE_MENTION)
                || element.is(shape::MATH_INLINE)
                || element.is(shape::PERSON_MENTION)
                || element.is(shape::TEMPLATE_BUTTON)
                || element.is(shape::HARD_BREAK)
                || schema::ATTACHMENT_NODES.contains(&element.name.as_str())
                || element.is(shape::GALLERY)
                || element.is(shape::WHITEBOARD)
                || element.children.iter().any(|child| match child {
                    Child::Text(segments) => !segments.is_empty(),
                    Child::Element(child) => has_content(child),
                    Child::Skipped => false,
                })
        }
        self.read(Mode::Plain)
            .root
            .as_ref()
            .is_none_or(|root| !has_content(root))
    }

    /// Every node with an `id`, in document order, with its direct inline content.
    ///
    /// Inline content: `pageRef` children with a valid `pageId` are
    /// [`Inline::PageRef`] (a malformed `blockId` reads as none), `hardBreak` children
    /// are [`Inline::Break`], and the text between them (several `LoroText`s in a row
    /// included) is one [`Inline::Text`] without marks; empty text is dropped. Other
    /// child nodes are blocks of their own and do not split the text (as in the
    /// TypeScript mirror).
    ///
    /// The leaf blocks `image` and `file` have no inline content of their own: an
    /// `image` reads as its `caption` then its `alt` text (2026-10-09, ADR-0028), one
    /// [`Inline::Text`] with one space between them when both are non-empty, either alone
    /// when the other is empty or not a string, nothing when both are (`widthPercent` and
    /// `align` are layout, not text), a `file` as nothing (the core indexes the manifest's
    /// name). A `pageLink` block reads as the one [`Inline::PageRef`] it stands for (none when its
    /// `pageId` is not valid), so that the text index treats a card exactly like a chip:
    /// the referenced page's title is its text. A `dateMention` (2026-10-07) reads as its
    /// ISO text, `2026-10-07` or `2026-10-07 14:30`, inside the [`Inline::Text`] around it
    /// (it does not split the run), so the text index and the backlinks' text hold the date
    /// as typed; one without a valid `date` reads as nothing, a malformed `time` is left out. A
    /// `mathInline` (2026-10-07) reads as its `latex` source inside the [`Inline::Text`] around it
    /// as well (it does not split the run either), so a search for `E=mc^2` finds the block; one
    /// without a string `latex` reads as nothing. A `personMention` (M13 slice 2, ADR-0049) reads as
    /// its `label` inside the run around it too (the account id is never text); it has no block id, so
    /// it is no block of its own, and one without a string label reads as nothing.
    ///
    /// The content of a `templateButton` (ADR-0056) is template material: its blocks are listed (they have ids, and
    /// `block_ids` lists them) with no inline content at all, whatever they are (an image's caption, a card's page,
    /// a drawing's text included), so that the text index, the backlinks' text and the AI index hold none of it.
    #[must_use]
    pub fn blocks(&self) -> Vec<BlockText> {
        fn visit(page: &PageDoc, element: &Element, in_button: bool, out: &mut Vec<BlockText>) {
            if let Some(id) = element.block_id() {
                let inlines = if in_button {
                    // Template material (ADR-0056): the block is listed, its text is not.
                    Vec::new()
                } else if element.is(shape::WHITEBOARD) {
                    // The live text elements of the drawing in z-order, one space apart (ADR-0033).
                    let text = whiteboard::texts(&page.whiteboard_elements(id)).join(" ");
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![Inline::Text(text)]
                    }
                } else if element.is(shape::IMAGE) {
                    // The caption, then the alt text, one space apart when both are there.
                    let caption = element.image_caption().unwrap_or_default();
                    let alt = element.str_attr(shape::ALT).unwrap_or_default();
                    let text = match (caption.is_empty(), alt.is_empty()) {
                        (true, true) => String::new(),
                        (false, true) => caption.to_owned(),
                        (true, false) => alt.to_owned(),
                        (false, false) => format!("{caption} {alt}"),
                    };
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![Inline::Text(text)]
                    }
                } else if element.is(shape::GALLERY) {
                    // The captions of the pictures (entries with a valid attachment), one space apart (ADR-0030).
                    let text = element
                        .gallery_entries()
                        .unwrap_or_default()
                        .iter()
                        .filter(|entry| entry.attachment.is_some())
                        .filter_map(|entry| entry.caption.filter(|caption| !caption.is_empty()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![Inline::Text(text)]
                    }
                } else if element.is(shape::FILE) {
                    Vec::new()
                } else if element.is(shape::PAGE_LINK) {
                    element
                        .ref_page()
                        .map(|page_id| {
                            vec![Inline::PageRef {
                                page_id,
                                block_id: element.ref_block(),
                            }]
                        })
                        .unwrap_or_default()
                } else {
                    inlines(element)
                };
                out.push(BlockText {
                    id,
                    kind: element.name.clone(),
                    inlines,
                });
            }
            let in_button = in_button || element.is(shape::TEMPLATE_BUTTON);
            for child in &element.children {
                if let Child::Element(child) = child {
                    visit(page, child, in_button, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.read(Mode::Plain).root {
            visit(self, root, false, &mut out);
        }
        out
    }

    /// Every page reference, attributed to the nearest enclosing block with an id.
    ///
    /// The reference's own id is skipped: a link belongs to the block that contains
    /// it (the paragraph), which is what backlinks show. A `pageLink` block (the card,
    /// 2026-10-06) is a link of its own and counts exactly like a `pageRef`, except that
    /// it is the block: it belongs to itself (its own `id`, or the enclosing block's when
    /// it has none). References without a valid `pageId` or without a block to belong to
    /// are left out. Every reference is listed, so the same target can appear twice for
    /// one block. The content of a `templateButton` is template material (ADR-0056): its
    /// references are not listed, so a link in it is no backlink.
    #[must_use]
    pub fn links(&self) -> Vec<PageLink> {
        fn visit(element: &Element, enclosing: Option<BlockId>, out: &mut Vec<PageLink>) {
            if element.is(shape::TEMPLATE_BUTTON) {
                return;
            }
            let from_block = match element.name.as_str() {
                shape::PAGE_REF => enclosing,
                shape::PAGE_LINK => element.block_id().or(enclosing),
                _ => None,
            };
            if let (Some(from_block), Some(to_page)) = (from_block, element.ref_page()) {
                out.push(PageLink {
                    from_block,
                    to_page,
                    to_block: element.ref_block(),
                });
            }
            let enclosing = element.block_id().or(enclosing);
            for child in &element.children {
                if let Child::Element(child) = child {
                    visit(child, enclosing, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.read(Mode::Structure).root {
            visit(root, None, &mut out);
        }
        out
    }

    /// Every date mention with a valid `date`, in document order, attributed to the nearest
    /// enclosing block with an id (the mention's own id is `node_id`): the rows of the
    /// derived `date_mentions` table (ADR-0024). A mention without a valid `date`, without
    /// an `id` of its own or outside any block with an id is left out
    /// (`check_invariants` reports the first two).
    ///
    /// `time` is the mention's time when it is a valid `HH:mm`. `reminder_minutes` is its
    /// `reminder` when that is a non-negative integer (a damaged one reads as none). The fire
    /// time, `fire_at_local`, is the mention's moment, `date` at `time` (09:00 when there is
    /// none), minus the reminder, in seconds since 1970-01-01T00:00:00 as the local wall clock
    /// reads it: calendar arithmetic on the floating local date, so no time zone data is
    /// needed and the device only has to say what its clock reads now (a mention is a floating
    /// local date, ADR-0016: the reminder is due when the local clock reaches that moment).
    ///
    /// The content of a `templateButton` is template material (ADR-0056): its mentions are not listed, so
    /// they never remind (a mention that a click copies into the page does).
    #[must_use]
    pub fn date_mentions(&self) -> Vec<DateMention> {
        fn visit(element: &Element, enclosing: Option<BlockId>, out: &mut Vec<DateMention>) {
            if element.is(shape::TEMPLATE_BUTTON) {
                return;
            }
            if element.is(shape::DATE_MENTION)
                && let (Some(block_id), Some(node_id), Some(date)) = (
                    enclosing,
                    element.block_id(),
                    element
                        .str_attr(shape::DATE)
                        .filter(|date| date::valid_date(date)),
                )
            {
                let time = element
                    .str_attr(shape::TIME)
                    .filter(|time| date::valid_time(time));
                let reminder_minutes = element.reminder_minutes();
                out.push(DateMention {
                    block_id,
                    node_id,
                    date: date.to_owned(),
                    time: time.map(str::to_owned),
                    reminder_minutes,
                    fire_at_local: reminder_minutes
                        .and_then(|minutes| date::fire_at_local(date, time, minutes)),
                    reminder_for: element.reminder_for(),
                });
            }
            let enclosing = element.block_id().or(enclosing);
            for child in &element.children {
                if let Child::Element(child) = child {
                    visit(child, enclosing, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.read(Mode::Structure).root {
            visit(root, None, &mut out);
        }
        out
    }

    /// Every person mention with a valid `accountId`, in document order, attributed to the nearest
    /// enclosing block with an id: the rows of the derived `person_mentions` table (M13 slice 2,
    /// ADR-0049). A mention without a valid `accountId` or outside any block with an id is left out
    /// (`check_invariants` reports the first). `label` is the stored label, or the empty string when it
    /// is missing or not a string. The same account twice in one block is listed twice (the table keeps
    /// one row per `(doc, block, account)`). The content of a `templateButton` is template material
    /// (ADR-0056): its mentions are not listed, so they notify nobody.
    #[must_use]
    pub fn person_mentions(&self) -> Vec<PersonMention> {
        fn visit(element: &Element, enclosing: Option<BlockId>, out: &mut Vec<PersonMention>) {
            if element.is(shape::TEMPLATE_BUTTON) {
                return;
            }
            if element.is(shape::PERSON_MENTION)
                && let (Some(block_id), Some(account_id)) = (enclosing, element.account_id())
            {
                out.push(PersonMention {
                    block_id,
                    account_id,
                    label: element
                        .str_attr(shape::LABEL)
                        .unwrap_or_default()
                        .to_owned(),
                });
            }
            let enclosing = element.block_id().or(enclosing);
            for child in &element.children {
                if let Child::Element(child) = child {
                    visit(child, enclosing, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.read(Mode::Structure).root {
            visit(root, None, &mut out);
        }
        out
    }

    /// Every attachment reference: the `image` and `file` blocks (at any depth) with
    /// their attachment, in document order.
    ///
    /// Duplicates are kept (one attachment shown twice, or a block id duplicated by
    /// concurrent moves). Blocks without a valid block id or without a valid
    /// `attachmentId` are left out (`check_invariants` reports them).
    #[must_use]
    pub fn attachment_refs(&self) -> Vec<(BlockId, AttachmentId)> {
        fn visit(element: &Element, out: &mut Vec<(BlockId, AttachmentId)>) {
            if schema::ATTACHMENT_NODES.contains(&element.name.as_str())
                && let (Some(block), Some(attachment)) =
                    (element.block_id(), element.attachment_id())
            {
                out.push((block, attachment));
            }
            if element.is(shape::GALLERY)
                && let Some(block) = element.block_id()
            {
                for entry in element.gallery_entries().unwrap_or_default() {
                    if let Some(attachment) = entry.attachment {
                        out.push((block, attachment));
                    }
                }
            }
            for child in &element.children {
                if let Child::Element(child) = child {
                    visit(child, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.read(Mode::Structure).root {
            visit(root, &mut out);
        }
        out
    }

    /// The attachment of the page's first picture (ADR-0054, the gallery card's preview): the first
    /// `image` block, or the first usable entry of the first `gallery` block, whichever comes first
    /// in document order, at any depth (columns, toggles and callouts included). A block without a
    /// valid block id or without a valid attachment id is skipped, exactly as
    /// [`Self::attachment_refs`] leaves it out (the attachment is retained by that reference);
    /// `file` blocks (a preview of a file is not a picture), whiteboards, the content of a
    /// `templateButton` (template material, ADR-0056) and the page's own icon and cover do not count.
    #[must_use]
    pub fn first_image(&self) -> Option<AttachmentId> {
        fn visit(element: &Element) -> Option<AttachmentId> {
            if element.is(shape::TEMPLATE_BUTTON) {
                return None;
            }
            if element.is(shape::IMAGE)
                && element.block_id().is_some()
                && let Some(attachment) = element.attachment_id()
            {
                return Some(attachment);
            }
            if element.is(shape::GALLERY)
                && element.block_id().is_some()
                && let Some(attachment) = element
                    .gallery_entries()
                    .unwrap_or_default()
                    .into_iter()
                    .find_map(|entry| entry.attachment)
            {
                return Some(attachment);
            }
            element.children.iter().find_map(|child| match child {
                Child::Element(child) => visit(child),
                _ => None,
            })
        }
        let tree = self.read(Mode::Structure);
        tree.root.as_ref().and_then(visit)
    }

    /// Ids of every node with an `id`, in document order (the ids of
    /// [`Self::blocks`]).
    #[must_use]
    pub fn block_ids(&self) -> Vec<BlockId> {
        self.with_index(|index| index.ids())
    }

    /// Ids of the blocks inside a `templateButton` (ADR-0056), at any depth, the buttons' own ids not included: template
    /// material, whose text is not the page's (`blocks` lists these blocks with no inline content). The core uses it for
    /// the one text that does not come from the inlines, the name of a file block's attachment.
    #[must_use]
    pub fn template_blocks(&self) -> HashSet<BlockId> {
        fn visit(element: &Element, in_button: bool, out: &mut HashSet<BlockId>) {
            if in_button && let Some(id) = element.block_id() {
                out.insert(id);
            }
            let in_button = in_button || element.is(shape::TEMPLATE_BUTTON);
            for child in &element.children {
                if let Child::Element(child) = child {
                    visit(child, in_button, out);
                }
            }
        }
        let mut out = HashSet::new();
        if let Some(root) = &self.read(Mode::Structure).root {
            visit(root, false, &mut out);
        }
        out
    }

    /// Node names present in the document that are not in `known`, deduplicated, in
    /// order of first appearance.
    #[must_use]
    pub fn unknown_node_names(&self, known: &[&str]) -> Vec<String> {
        fn visit(
            element: &Element,
            known: &[&str],
            seen: &mut HashSet<String>,
            out: &mut Vec<String>,
        ) {
            if !known.contains(&element.name.as_str()) && seen.insert(element.name.clone()) {
                out.push(element.name.clone());
            }
            for child in &element.children {
                if let Child::Element(child) = child {
                    visit(child, known, seen, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.read(Mode::Structure).root {
            visit(root, known, &mut HashSet::new(), &mut out);
        }
        out
    }

    /// ProseMirror JSON in the portable format of the milestone spec: `attrs` holds
    /// non-null values only (omitted when empty), `content` omitted when empty, marks
    /// `{type, attrs?}`, keys in the order `type, attrs, content, marks, text`.
    ///
    /// Text nodes are split per distinct mark set (adjacent runs with equal marks are
    /// joined, across `LoroText`s too); marks are listed in canonical order (the v1
    /// marks in table order, then unknown marks by name). A document without a root
    /// yet is `{"type": "doc"}`. Details in `page/portable.rs`.
    #[must_use]
    pub fn to_portable(&self) -> serde_json::Value {
        let mut doc = portable::document(self.read(Mode::Rich).root.as_ref());
        let live = portable::node_ids(&doc, shape::WHITEBOARD);
        let whiteboards = self.whiteboards_portable(&live);
        if !whiteboards.is_empty()
            && let Value::Object(obj) = &mut doc
        {
            obj.insert(shape::WHITEBOARDS.into(), Value::Object(whiteboards));
        }
        doc
    }

    /// Builds the Loro shape of `json` (portable or raw ProseMirror JSON) into an
    /// empty page document. Fails with [`ModelError::Shape`] on anything that is not
    /// a node tree (`type` strings, `content` arrays, `text` strings, `attrs`/`marks`
    /// objects).
    ///
    /// The top-level node must be a `doc`. Null attributes, empty `attrs`/`marks`/
    /// `content` and empty text nodes are accepted (raw ProseMirror JSON); marks on
    /// element nodes are dropped (the binding does not store them). Nothing is written
    /// unless the whole input is valid.
    pub fn from_portable(doc: Doc, json: &serde_json::Value) -> Result<Self, ModelError> {
        let page = Self::wrap(doc)?;
        let root = page.root();
        if !root.is_empty() {
            return Err(ModelError::Shape(
                "portable JSON can only be imported into an empty document".into(),
            ));
        }
        let node = portable::parse_doc(json)?;
        let whiteboards = match json.get(shape::WHITEBOARDS) {
            None => None,
            Some(Value::Object(whiteboards)) => Some(whiteboards),
            Some(_) => {
                return Err(ModelError::Shape(
                    "portable JSON: whiteboards is not an object".into(),
                ));
            }
        };
        write::init_root(&root, &node)?;
        if let Some(whiteboards) = whiteboards {
            page.import_whiteboards(whiteboards)?;
        }
        page.commit();
        Ok(page)
    }

    /// Markdown rendering (mapping in the milestone spec). `title_of` resolves page
    /// references to titles; unresolved references render as `[[<uuid>]]`. File blocks
    /// render with their attachment id as the link text; use
    /// [`Self::to_markdown_with`] to show their names.
    ///
    /// The detailed rules (shared with the TypeScript mirror) are in
    /// `page/markdown.rs`.
    #[must_use]
    pub fn to_markdown(&self, title_of: &dyn Fn(PageId) -> Option<String>) -> String {
        self.to_markdown_with(title_of, &mut |_| None)
    }

    /// [`Self::to_markdown`] with a resolver for attachment names: `file` blocks render
    /// as `[<name>](attachment://<id>)`, with the id as the name when `name_of` answers
    /// `None` (`image` blocks render as `![<alt>](attachment://<id>)`, with the caption as
    /// the link title when they have one, either way).
    /// `name_of` is called once per distinct attachment of the page's file blocks, in
    /// document order, before rendering.
    #[must_use]
    pub fn to_markdown_with(
        &self,
        title_of: &dyn Fn(PageId) -> Option<String>,
        name_of: &mut dyn FnMut(AttachmentId) -> Option<String>,
    ) -> String {
        markdown::render(&self.to_portable(), title_of, name_of)
    }

    /// Shape problems: duplicate or malformed ids, a root that is not `doc`, element
    /// nodes without the `nodeName`/`attributes`/`children` triple, non-string node
    /// names. Empty when the document is sound.
    ///
    /// Also reported: children-list entries that are neither nodes nor texts, nodes
    /// nested deeper than the readers follow, schema nodes that should carry an id
    /// but have none, `pageRef`s and `pageLink`s without a valid `pageId` or with a malformed
    /// `blockId`, `image`/`file` blocks without a valid `attachmentId`, `dateMention`s without
    /// a valid `date` or with a malformed `time`, `reminder` (one that is not a non-negative
    /// integer) or `reminderFor` (present and not a valid account id), `personMention`s without a
    /// valid `accountId` (M13 slice 2, ADR-0049), `image`s with a malformed `widthPercent` (present and not an integer from 10
    /// to 100), `align` (not `left`, `center` or `right`) or `caption` (not a string), and
    /// `mathInline`s whose `latex` is not a string, and `detailsSummary`s with a malformed
    /// `level` (present and not an integer from 1 to 3), and (ADR-0056) a `dateMention` whose `relative` is
    /// not `today` or `now`, a `personMention` whose is not `me`, and a `templateButton` whose `position`
    /// is not `below` or `above`. Messages
    /// hold node positions (`node /0/2`), ids and node names, never text.
    #[must_use]
    pub fn check_invariants(&self) -> Vec<String> {
        fn visit(
            element: &Element,
            path: &mut Vec<usize>,
            ids: &mut HashMap<BlockId, usize>,
            problems: &mut Vec<String>,
        ) {
            let at = read::at(path);
            match element.attr(shape::ID) {
                Some(LoroValue::String(raw)) => match raw.as_str().parse::<BlockId>() {
                    Ok(id) => *ids.entry(id).or_default() += 1,
                    Err(_) => problems.push(format!("{at}: malformed id")),
                },
                Some(_) => problems.push(format!("{at}: id is not a string")),
                // The root is the page itself, whatever its (reported) name.
                None if !path.is_empty() && schema::ID_BEARING.contains(&element.name.as_str()) => {
                    problems.push(format!("{at}: {} without an id", element.name));
                }
                None => {}
            }
            if element.is(shape::PAGE_REF) || element.is(shape::PAGE_LINK) {
                if element.ref_page().is_none() {
                    problems.push(format!("{at}: {} without a valid pageId", element.name));
                }
                if element.attr(shape::BLOCK_ID).is_some() && element.ref_block().is_none() {
                    problems.push(format!("{at}: {} with a malformed blockId", element.name));
                }
            }
            if element.is(shape::DATE_MENTION) {
                if element.date_text().is_none() {
                    problems.push(format!("{at}: {} without a valid date", element.name));
                }
                if element.attr(shape::TIME).is_some()
                    && !element.str_attr(shape::TIME).is_some_and(date::valid_time)
                {
                    problems.push(format!("{at}: {} with a malformed time", element.name));
                }
                if element.attr(shape::REMINDER).is_some() && element.reminder_minutes().is_none() {
                    problems.push(format!("{at}: {} with a malformed reminder", element.name));
                }
                if element.attr(shape::REMINDER_FOR).is_some() && element.reminder_for().is_none() {
                    problems.push(format!(
                        "{at}: {} with a malformed reminderFor",
                        element.name
                    ));
                }
            }
            if element.is(shape::DATE_MENTION)
                && element.attr(shape::RELATIVE).is_some()
                && element.date_relative().is_none()
            {
                problems.push(format!("{at}: {} with a malformed relative", element.name));
            }
            if element.is(shape::PERSON_MENTION)
                && element.attr(shape::RELATIVE).is_some()
                && element.person_relative().is_none()
            {
                problems.push(format!("{at}: {} with a malformed relative", element.name));
            }
            if element.is(shape::TEMPLATE_BUTTON)
                && element.attr(shape::POSITION).is_some()
                && element.button_position().is_none()
            {
                problems.push(format!("{at}: {} with a malformed position", element.name));
            }
            if element.is(shape::MATH_INLINE) && element.latex().is_none() {
                problems.push(format!("{at}: {} without a string latex", element.name));
            }
            if element.is(shape::PERSON_MENTION) && element.account_id().is_none() {
                problems.push(format!("{at}: {} without a valid accountId", element.name));
            }
            if schema::ATTACHMENT_NODES.contains(&element.name.as_str())
                && element.attachment_id().is_none()
            {
                problems.push(format!(
                    "{at}: {} without a valid attachmentId",
                    element.name
                ));
            }
            if element.is(shape::IMAGE) {
                if element.attr(shape::WIDTH_PERCENT).is_some()
                    && element.image_width_percent().is_none()
                {
                    problems.push(format!("{at}: image with a malformed widthPercent"));
                }
                if element.attr(shape::ALIGN).is_some() && element.image_align().is_none() {
                    problems.push(format!("{at}: image with a malformed align"));
                }
                if element.attr(shape::CAPTION).is_some() && element.image_caption().is_none() {
                    problems.push(format!("{at}: image with a malformed caption"));
                }
            }
            if element.is(shape::FILE)
                && element.attr(shape::VIEW).is_some()
                && element.file_view().is_none()
            {
                problems.push(format!("{at}: file with a malformed view"));
            }
            if element.is(shape::GALLERY) {
                if element.attr(shape::COLUMNS).is_some() && element.gallery_columns().is_none() {
                    problems.push(format!("{at}: gallery with a malformed columns"));
                }
                if element.attr(shape::ITEMS).is_some() {
                    match element.gallery_entries() {
                        None => problems.push(format!("{at}: gallery with a malformed items")),
                        Some(entries) => {
                            for (index, entry) in entries.iter().enumerate() {
                                if !entry.well_formed {
                                    problems
                                        .push(format!("{at}: gallery item {index} is malformed"));
                                }
                            }
                        }
                    }
                }
            }
            if element.is(shape::WHITEBOARD)
                && element.attr(shape::HEIGHT).is_some()
                && element.whiteboard_height().is_none()
            {
                problems.push(format!("{at}: whiteboard with a malformed height"));
            }
            if element.is(shape::DETAILS_SUMMARY)
                && element.attr(shape::LEVEL).is_some()
                && element.summary_level().is_none()
            {
                problems.push(format!("{at}: detailsSummary with a malformed level"));
            }
            for (index, child) in element.children.iter().enumerate() {
                if let Child::Element(child) = child {
                    path.push(index);
                    visit(child, path, ids, problems);
                    path.pop();
                }
            }
        }

        let tree = self.read(Mode::Structure);
        let mut problems = tree.problems;
        if let Some(root) = &tree.root {
            let mut ids: HashMap<BlockId, usize> = HashMap::new();
            visit(root, &mut Vec::new(), &mut ids, &mut problems);
            let mut duplicates: Vec<(BlockId, usize)> =
                ids.into_iter().filter(|(_, count)| *count > 1).collect();
            duplicates.sort_unstable();
            for (id, count) in duplicates {
                problems.push(format!("block id {id} appears {count} times"));
            }
        }
        problems.extend(self.whiteboard_problems());
        problems
    }

    /// Inserts a top-level block after `after` (or first when `None`) with `text` as
    /// its single text run and `id` as its block id. `Quote` wraps a paragraph that
    /// gets a derived id.
    ///
    /// `after` may name any block: the new block goes right after the top-level block
    /// that contains it (so `block_ids().last()` always appends). The paragraph inside
    /// a `Quote` gets [`Self::quote_paragraph_id`]`(id)`, so the operation is
    /// reproducible. Nodes carry the attributes the editor gives them (`level` for
    /// headings, `language` for code blocks when set). An `Image` takes `text` as its
    /// `alt` attribute (always stored, possibly empty) and has no children; a `File`
    /// takes no text. A document without a root yet gets one.
    ///
    /// Errors: [`ModelError::BlockNotFound`] (`after`), [`ModelError::DuplicateBlock`]
    /// (`id` or the derived id already present), [`ModelError::InvalidBlock`] (a
    /// heading level outside 1..=3, text for a divider or a file block).
    pub fn insert_block(
        &self,
        after: Option<BlockId>,
        kind: BlockKind,
        text: &str,
        id: BlockId,
    ) -> Result<(), ModelError> {
        let node = block_node(&kind, text, id)?;
        let mut new_ids = vec![id];
        if kind == BlockKind::Quote {
            new_ids.push(Self::quote_paragraph_id(id));
        }
        self.with_index(|index| {
            if new_ids.iter().any(|new| index.contains(*new)) {
                return Err(duplicate_block());
            }
            let pos = match after {
                None => 0,
                Some(after) => match index.find(after) {
                    Some((Some(top), _)) => top + 1,
                    Some((None, _)) | None => return Err(block_not_found()),
                },
            };
            let children = write::root_children(&self.root())?;
            let map = write::insert_node(&children, pos, &node)?;
            self.commit();
            index.tops.insert(pos, Top::read(&map));
            index.frontiers = self.frontiers();
            Ok(())
        })
    }

    /// Replaces the direct text runs of a text-bearing block with one unmarked run.
    ///
    /// The block (`paragraph`, `heading`, `codeBlock`, `mathBlock` or `detailsSummary`,
    /// at any depth) keeps its node and attributes; its whole inline content becomes `text`:
    /// page references and breaks inside it are removed, the first existing
    /// `LoroText` is kept and edited in place (common prefix and suffix untouched, so
    /// concurrent typing elsewhere in the text merges) and its marks are removed.
    ///
    /// Errors: [`ModelError::BlockNotFound`], [`ModelError::InvalidBlock`] (a block
    /// that does not hold text).
    pub fn set_block_text(&self, id: BlockId, text: &str) -> Result<(), ModelError> {
        self.with_index(|index| {
            let (top, entry) = index.find(id).ok_or_else(block_not_found)?;
            if !schema::TEXT_NODES.contains(&entry.name.as_str()) {
                return Err(ModelError::InvalidBlock("this block holds no text"));
            }
            let children = write::child_list(&entry.map, shape::CHILDREN)?;
            let mut items = Vec::with_capacity(children.len());
            children.for_each(|item| items.push(item));
            let keep = items.iter().enumerate().find_map(|(pos, item)| match item {
                ValueOrContainer::Container(Container::Text(text)) => Some((pos, text.clone())),
                _ => None,
            });
            delete_all_but(&children, items.len(), keep.as_ref().map(|(pos, _)| *pos))?;
            match keep {
                Some((_, existing)) => replace_text(&existing, text)?,
                None if !text.is_empty() => {
                    write::insert_text(
                        &children,
                        0,
                        &[write::NewRun {
                            text: text.to_owned(),
                            marks: Vec::new(),
                        }],
                    )?;
                }
                None => {}
            }
            self.commit();
            self.reindex_top(index, top);
            Ok(())
        })
    }

    /// Every `pageList` block anywhere in the tree (columns, toggles and callouts included), in
    /// document order, with its non-null attributes as plain JSON: `id`, `source`, `layout`,
    /// `properties`, `sort`, `filters`, `groupBy`, `hiddenGroups`, `columnWidths`, `calculations`,
    /// `views` (ADR-0054; its typed form is [`views::PageListView`]) and whatever a newer version added, exactly as stored (an integral number reads as an integer). A
    /// `pageList` without a valid block id cannot be addressed and is left out; a duplicated id is
    /// listed once per occurrence, as every reader does.
    ///
    /// The core uses it to remap the schema ids of a view when its page moves to another space
    /// (ADR-0038); it reads the document and writes nothing.
    #[must_use]
    pub fn page_lists(&self) -> Vec<(BlockId, serde_json::Map<String, Value>)> {
        let tree = self.read(Mode::Structure);
        let mut out = Vec::new();
        let Some(root) = tree.root.as_ref() else {
            return out;
        };
        // Pre-order, children in order (iterative: the depth limit of the reader bounds it anyway).
        let mut stack: Vec<&Element> = vec![root];
        while let Some(element) = stack.pop() {
            if element.is(PAGE_LIST)
                && let Some(id) = element.block_id()
            {
                let attrs: serde_json::Map<String, Value> = element
                    .attrs
                    .iter()
                    .filter(|(_, value)| !matches!(value, LoroValue::Null))
                    .map(|(key, value)| (key.clone(), portable::json_value(value, 0)))
                    .collect();
                out.push((id, attrs));
            }
            for child in element.children.iter().rev() {
                if let Child::Element(child) = child {
                    stack.push(child);
                }
            }
        }
        out
    }

    /// Changes attributes of one `pageList` block, in one commit: `Some(value)` sets the
    /// attribute whole (a list or an object is one plain value, like `properties` and `sort` are
    /// written by the editor), `None` (or `Some(null)`) removes it. Answers whether anything
    /// changed: a value equal to the stored one, or the removal of an absent attribute, writes
    /// nothing and answers `false`.
    ///
    /// Errors: [`ModelError::BlockNotFound`] for an unknown block and for a block that is not a
    /// `pageList`; [`ModelError::InvalidBlock`] when a change names the `id` or an empty key;
    /// [`ModelError::Shape`] for a value nested too deeply. Everything is checked before anything
    /// is written. The core uses it to remap the schema ids of a view when its page moves to
    /// another space (ADR-0038).
    pub fn set_page_list_attrs(
        &self,
        block: BlockId,
        changes: &[(&str, Option<Value>)],
    ) -> Result<bool, ModelError> {
        self.with_index(|index| {
            let (top, entry) = index.find(block).ok_or_else(block_not_found)?;
            if entry.name != PAGE_LIST {
                return Err(block_not_found());
            }
            let attributes = match entry.map.get(shape::ATTRIBUTES) {
                Some(ValueOrContainer::Container(Container::Map(map))) => Some(map),
                _ => None,
            };
            // What each change writes (`None` for a removal), checked against what is stored.
            let mut writes: Vec<(&str, Option<LoroValue>)> = Vec::new();
            for (key, change) in changes {
                if key.is_empty() || *key == shape::ID {
                    return Err(ModelError::InvalidBlock(
                        "the id and unnamed attributes of a block cannot change",
                    ));
                }
                let wanted = match change {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(portable::loro_value(value, 0)?),
                };
                let stored = attributes
                    .as_ref()
                    .and_then(|map| map.get(key))
                    .map(|stored| stored.get_deep_value())
                    .filter(|stored| !matches!(stored, LoroValue::Null));
                let same = match (&wanted, &stored) {
                    (None, None) => true,
                    // Compared as the readers see them: `100` and `100.0` are one value.
                    (Some(wanted), Some(stored)) => {
                        portable::json_value(wanted, 0) == portable::json_value(stored, 0)
                    }
                    _ => false,
                };
                if !same {
                    writes.push((key, wanted));
                }
            }
            if writes.is_empty() {
                return Ok(false);
            }
            let attributes = write::child_map(&entry.map, shape::ATTRIBUTES)?;
            for (key, wanted) in writes {
                match wanted {
                    Some(value) => attributes.insert(key, value).map_err(engine)?,
                    None => attributes.delete(key).map_err(engine)?,
                }
            }
            self.commit();
            self.reindex_top(index, top);
            Ok(true)
        })
    }

    /// Points every reference to the attachment `from` at the attachment `to`: the `attachmentId` of
    /// the `image` and `file` blocks (at any depth) and of the entries of `gallery` blocks, each
    /// block in its own commit. Answers how many blocks changed (0 when nothing refers to `from`).
    /// The core uses it when a page that moves to a shared space (or out of one) takes a copy of an
    /// attachment other pages keep using (ADR-0047): the copy is a new attachment, the moved
    /// page's blocks name it.
    ///
    /// Errors: [`ModelError::BlockNotFound`] when a block vanished while this ran (nothing else).
    pub fn replace_attachment(
        &self,
        from: AttachmentId,
        to: AttachmentId,
    ) -> Result<usize, ModelError> {
        let mut blocks: Vec<BlockId> = Vec::new();
        for (block, attachment) in self.attachment_refs() {
            if attachment == from && !blocks.contains(&block) {
                blocks.push(block);
            }
        }
        let mut changed = 0;
        for block in blocks {
            changed += usize::from(self.replace_attachment_in(block, from, to)?);
        }
        Ok(changed)
    }

    /// One block of [`Self::replace_attachment`]: `true` when it named `from` and now names `to`.
    fn replace_attachment_in(
        &self,
        block: BlockId,
        from: AttachmentId,
        to: AttachmentId,
    ) -> Result<bool, ModelError> {
        let (from, to) = (from.to_string(), to.to_string());
        self.with_index(|index| {
            let (top, entry) = index.find(block).ok_or_else(block_not_found)?;
            let Some(ValueOrContainer::Container(Container::Map(attributes))) =
                entry.map.get(shape::ATTRIBUTES)
            else {
                return Ok(false);
            };
            if entry.name == shape::GALLERY {
                let Some(stored) = attributes.get(shape::ITEMS) else {
                    return Ok(false);
                };
                let Value::Array(mut items) = portable::json_value(&stored.get_deep_value(), 0)
                else {
                    return Ok(false);
                };
                let mut touched = false;
                for item in &mut items {
                    if let Value::Object(map) = item
                        && map.get(shape::ATTACHMENT_ID).and_then(Value::as_str) == Some(&from)
                    {
                        map.insert(shape::ATTACHMENT_ID.to_owned(), Value::String(to.clone()));
                        touched = true;
                    }
                }
                if !touched {
                    return Ok(false);
                }
                let list = portable::loro_value(&Value::Array(items), 0)?;
                attributes.insert(shape::ITEMS, list).map_err(engine)?;
            } else {
                let named = match attributes.get(shape::ATTACHMENT_ID) {
                    Some(ValueOrContainer::Value(LoroValue::String(id))) => {
                        id.as_str() == from.as_str()
                    }
                    _ => false,
                };
                if !named {
                    return Ok(false);
                }
                attributes
                    .insert(shape::ATTACHMENT_ID, to.as_str())
                    .map_err(engine)?;
            }
            self.commit();
            self.reindex_top(index, top);
            Ok(true)
        })
    }

    /// Appends an inline `pageRef` (`attributes: {id, pageId, blockId?}`, empty
    /// `children`) at the end of the content of the block `block`, which must be a
    /// `paragraph` or a `heading` (at any depth).
    ///
    /// Errors: [`ModelError::BlockNotFound`] (`block`), [`ModelError::DuplicateBlock`]
    /// (`id`), [`ModelError::InvalidBlock`] (a block whose content is not inline).
    pub fn insert_page_ref(
        &self,
        block: BlockId,
        id: BlockId,
        page_id: PageId,
        to_block: Option<BlockId>,
    ) -> Result<(), ModelError> {
        self.with_index(|index| {
            let (top, entry) = index.find(block).ok_or_else(block_not_found)?;
            if !schema::INLINE_CONTENT_NODES.contains(&entry.name.as_str()) {
                return Err(ModelError::InvalidBlock(
                    "page references go into paragraphs and headings",
                ));
            }
            if index.contains(id) {
                return Err(duplicate_block());
            }
            let mut attrs = vec![
                (shape::ID.to_owned(), LoroValue::from(id.to_string())),
                (
                    shape::PAGE_ID.to_owned(),
                    LoroValue::from(page_id.to_string()),
                ),
            ];
            if let Some(to_block) = to_block {
                attrs.push((
                    shape::BLOCK_ID.to_owned(),
                    LoroValue::from(to_block.to_string()),
                ));
            }
            let node = NewNode {
                name: shape::PAGE_REF.to_owned(),
                attrs,
                content: Vec::new(),
            };
            let children = write::child_list(&entry.map, shape::CHILDREN)?;
            write::insert_node(&children, children.len(), &node)?;
            self.commit();
            self.reindex_top(index, top);
            Ok(())
        })
    }

    /// Appends the top-level blocks of portable page JSON (a `doc` node, as
    /// [`Self::from_portable`] reads it) after the page's last top-level block, in one commit, and
    /// answers how many top-level blocks were appended. Existing content is never touched.
    ///
    /// The ids of the new blocks come from the JSON (the import and the MCP server give every
    /// block a fresh one); an id that the page already has, or that repeats inside the JSON, is
    /// [`ModelError::DuplicateBlock`]. Whiteboard elements are not appended: a non-empty
    /// `whiteboards` object is a [`ModelError::Shape`] (appended content comes from Markdown, which
    /// has no whiteboards). Nothing is written unless the whole input is valid.
    pub fn append_portable(&self, json: &Value) -> Result<usize, ModelError> {
        let node = portable_node(json)?;
        let mut ids: Vec<BlockId> = Vec::new();
        let mut appended = 0;
        for child in &node.content {
            if let NewChild::Node(node) = child {
                appended += 1;
                collect_block_ids(node, &mut ids);
            }
        }
        self.with_index(|index| {
            let mut seen: HashSet<BlockId> = HashSet::with_capacity(ids.len());
            if ids
                .iter()
                .any(|id| index.contains(*id) || !seen.insert(*id))
            {
                return Err(duplicate_block());
            }
            let children = write::root_children(&self.root())?;
            for child in &node.content {
                let pos = children.len();
                match child {
                    NewChild::Node(child) => {
                        write::insert_node(&children, pos, child)?;
                    }
                    NewChild::Text(runs) => {
                        write::insert_text(&children, pos, runs)?;
                    }
                }
            }
            self.commit();
            // The index is stale now (its frontiers no longer match): the next reader rebuilds it.
            Ok(appended)
        })
    }

    /// Deletes a top-level block (and everything inside it).
    ///
    /// Errors: [`ModelError::BlockNotFound`]; [`ModelError::InvalidBlock`] when the id
    /// belongs to a nested node or the block is the page's last one (a page keeps at
    /// least one block; concurrent deletions on two devices can still empty it).
    pub fn delete_block(&self, id: BlockId) -> Result<(), ModelError> {
        self.with_index(|index| {
            let Some(pos) = index.top_position(id) else {
                return Err(if index.contains(id) {
                    ModelError::InvalidBlock("only top-level blocks can be deleted")
                } else {
                    block_not_found()
                });
            };
            if index.top_elements() <= 1 {
                return Err(ModelError::InvalidBlock(
                    "the last block of a page cannot be deleted",
                ));
            }
            let children = write::root_children(&self.root())?;
            children.delete(pos, 1).map_err(engine)?;
            self.commit();
            index.tops.remove(pos);
            index.frontiers = self.frontiers();
            Ok(())
        })
    }

    /// The id [`Self::insert_block`] gives the paragraph inside a [`BlockKind::Quote`]
    /// block `quote`: `quote` with its last byte inverted (version and variant bits
    /// unchanged, so it is still a UUIDv4 when `quote` is one).
    #[must_use]
    pub fn quote_paragraph_id(quote: BlockId) -> BlockId {
        let mut bytes = quote.to_bytes();
        bytes[15] ^= 0xff;
        BlockId::from_bytes(bytes)
    }

    // ----- internals --------------------------------------------------------------------

    fn wrap(doc: Doc) -> Result<Self, ModelError> {
        if doc.kind() != DocKind::Page {
            return Err(ModelError::Shape(format!(
                "expected a page document, got {:?}",
                doc.kind()
            )));
        }
        let root = doc.inner().get_map(shape::ROOT);
        match root.get(shape::NODE_NAME) {
            None => {}
            Some(ValueOrContainer::Value(LoroValue::String(name)))
                if name.as_str() == shape::DOC => {}
            Some(_) => {
                return Err(ModelError::Shape("the root node is not a doc".into()));
            }
        }
        Ok(Self {
            doc,
            index: Mutex::new(None),
        })
    }

    fn root(&self) -> LoroMap {
        self.doc.inner().get_map(shape::ROOT)
    }

    fn read(&self, mode: Mode) -> read::Tree {
        read::read(&self.root(), mode)
    }

    fn commit(&self) {
        self.doc.inner().commit();
    }

    fn frontiers(&self) -> loro::Frontiers {
        self.doc.inner().state_frontiers()
    }

    /// Runs `f` with an index of the current document state (rebuilt when the
    /// document changed since it was made). `f` updates the index after its own
    /// committed writes, including `frontiers`; if it fails half-way, the frontiers
    /// no longer match and the next call rebuilds.
    fn with_index<T>(&self, f: impl FnOnce(&mut Index) -> T) -> T {
        // Uncommitted engine operations change the state but not the frontiers.
        self.commit();
        let frontiers = self.frontiers();
        // Poisoning cannot leave a wrong index behind: a stale one fails the check.
        let mut slot = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let index = match slot.take() {
            Some(index) if index.frontiers == frontiers => index,
            _ => Index::build(&self.root(), frontiers),
        };
        f(slot.insert(index))
    }

    /// After a committed edit inside the top-level entry `top` (`None`: the root
    /// itself), re-reads that entry, or rebuilds everything when the edit was
    /// outside the top-level blocks.
    fn reindex_top(&self, index: &mut Index, top: Option<usize>) {
        let frontiers = self.frontiers();
        let refreshed = top.and_then(|pos| {
            let map = index.tops.get(pos)?.map.clone()?;
            Some((pos, Top::read(&map)))
        });
        match refreshed {
            Some((pos, entry)) => {
                if let Some(slot) = index.tops.get_mut(pos) {
                    *slot = entry;
                }
                index.frontiers = frontiers;
            }
            None => *index = Index::build(&self.root(), frontiers),
        }
    }
}

// ----- helpers ------------------------------------------------------------------------------

/// Node name of the list/table view block.
const PAGE_LIST: &str = "pageList";

fn block_not_found() -> ModelError {
    ModelError::BlockNotFound
}

/// The `doc` node of portable page JSON for [`PageDoc::append_portable`]: valid, without
/// whiteboard elements.
fn portable_node(json: &Value) -> Result<NewNode, ModelError> {
    let node = portable::parse_doc(json)?;
    let has_whiteboards = match json.get(shape::WHITEBOARDS) {
        None | Some(Value::Null) => false,
        Some(Value::Object(whiteboards)) => !whiteboards.is_empty(),
        Some(_) => true,
    };
    if has_whiteboards {
        return Err(ModelError::Shape(
            "portable JSON: appended content cannot hold whiteboards".into(),
        ));
    }
    Ok(node)
}

/// The block ids (`id` attributes that parse as one) of a node and everything inside it.
fn collect_block_ids(node: &NewNode, ids: &mut Vec<BlockId>) {
    for (key, value) in &node.attrs {
        if key == shape::ID
            && let LoroValue::String(raw) = value
            && let Ok(id) = raw.as_str().parse::<BlockId>()
        {
            ids.push(id);
        }
    }
    for child in &node.content {
        if let NewChild::Node(child) = child {
            collect_block_ids(child, ids);
        }
    }
}

fn duplicate_block() -> ModelError {
    ModelError::DuplicateBlock
}

/// A paragraph node with plain text.
fn paragraph(id: BlockId, text: &str) -> NewNode {
    NewNode {
        name: "paragraph".to_owned(),
        attrs: vec![id_attr(id)],
        content: write::plain_text(text),
    }
}

fn id_attr(id: BlockId) -> (String, LoroValue) {
    (shape::ID.to_owned(), LoroValue::from(id.to_string()))
}

/// The node a semantic insert writes.
fn block_node(kind: &BlockKind, text: &str, id: BlockId) -> Result<NewNode, ModelError> {
    Ok(match kind {
        BlockKind::Paragraph => paragraph(id, text),
        BlockKind::Heading(level) => {
            if !(1..=3).contains(level) {
                return Err(ModelError::InvalidBlock("heading levels are 1 to 3"));
            }
            NewNode {
                name: "heading".to_owned(),
                attrs: vec![
                    id_attr(id),
                    ("level".to_owned(), LoroValue::I64(i64::from(*level))),
                ],
                content: write::plain_text(text),
            }
        }
        BlockKind::CodeBlock { language } => {
            let mut attrs = vec![id_attr(id)];
            if let Some(language) = language {
                attrs.push(("language".to_owned(), LoroValue::from(language.as_str())));
            }
            NewNode {
                name: "codeBlock".to_owned(),
                attrs,
                content: write::plain_text(text),
            }
        }
        BlockKind::Quote => NewNode {
            name: "blockquote".to_owned(),
            attrs: vec![id_attr(id)],
            content: vec![NewChild::Node(paragraph(
                PageDoc::quote_paragraph_id(id),
                text,
            ))],
        },
        BlockKind::Divider => {
            if !text.is_empty() {
                return Err(ModelError::InvalidBlock("a divider holds no text"));
            }
            NewNode {
                name: "horizontalRule".to_owned(),
                attrs: vec![id_attr(id)],
                content: Vec::new(),
            }
        }
        BlockKind::Image {
            attachment_id,
            width,
        } => {
            let mut attrs = vec![
                id_attr(id),
                attachment_attr(*attachment_id),
                (shape::ALT.to_owned(), LoroValue::from(text)),
            ];
            if let Some(width) = width {
                attrs.push((shape::WIDTH.to_owned(), LoroValue::I64(i64::from(*width))));
            }
            NewNode {
                name: shape::IMAGE.to_owned(),
                attrs,
                content: Vec::new(),
            }
        }
        BlockKind::File { attachment_id } => {
            if !text.is_empty() {
                return Err(ModelError::InvalidBlock("a file block holds no text"));
            }
            NewNode {
                name: shape::FILE.to_owned(),
                attrs: vec![id_attr(id), attachment_attr(*attachment_id)],
                content: Vec::new(),
            }
        }
    })
}

fn attachment_attr(attachment_id: AttachmentId) -> (String, LoroValue) {
    (
        shape::ATTACHMENT_ID.to_owned(),
        LoroValue::from(attachment_id.to_string()),
    )
}

/// The inline content of an element (see [`PageDoc::blocks`]).
fn inlines(element: &Element) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut Vec<Inline>| {
        if !text.is_empty() {
            out.push(Inline::Text(std::mem::take(text)));
        }
    };
    for child in &element.children {
        match child {
            Child::Text(segments) => {
                for segment in segments {
                    text.push_str(&segment.text);
                }
            }
            Child::Element(child) if child.is(shape::PAGE_REF) => {
                if let Some(page_id) = child.ref_page() {
                    flush(&mut text, &mut out);
                    out.push(Inline::PageRef {
                        page_id,
                        block_id: child.ref_block(),
                    });
                }
            }
            Child::Element(child) if child.is(shape::DATE_MENTION) => {
                // Reads as text, in the run around it: `Meeting 2026-10-07 at the office`.
                if let Some(iso) = child.date_text() {
                    text.push_str(&iso);
                }
            }
            Child::Element(child) if child.is(shape::MATH_INLINE) => {
                // Reads as its LaTeX source, in the run around it: `Energy E=mc^2 is`.
                if let Some(latex) = child.latex() {
                    text.push_str(latex);
                }
            }
            Child::Element(child) if child.is(shape::PERSON_MENTION) => {
                // Reads as its label, in the run around it: `Ask Ada Lovelace about it` (the text
                // index holds the label, ADR-0049). One without a string label reads as nothing.
                if let Some(label) = child.str_attr(shape::LABEL) {
                    text.push_str(label);
                }
            }
            Child::Element(child) if child.is(shape::HARD_BREAK) => {
                flush(&mut text, &mut out);
                out.push(Inline::Break);
            }
            Child::Element(_) | Child::Skipped => {}
        }
    }
    flush(&mut text, &mut out);
    out
}

/// Deletes every entry of `list` (of length `len`) except the one at `keep`.
fn delete_all_but(list: &LoroList, len: usize, keep: Option<usize>) -> Result<(), ModelError> {
    match keep {
        Some(keep) => {
            if keep + 1 < len {
                list.delete(keep + 1, len - keep - 1).map_err(engine)?;
            }
            if keep > 0 {
                list.delete(0, keep).map_err(engine)?;
            }
        }
        None => {
            if len > 0 {
                list.delete(0, len).map_err(engine)?;
            }
        }
    }
    Ok(())
}

/// Turns `text` into `new` with minimal edits (common prefix and suffix kept, like
/// the binding's diff) and removes every mark.
fn replace_text(text: &LoroText, new: &str) -> Result<(), ModelError> {
    let old: Vec<char> = text.to_string().chars().collect();
    let new: Vec<char> = new.chars().collect();
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let removed = old.len() - prefix - suffix;
    if removed > 0 {
        text.delete(prefix, removed).map_err(engine)?;
    }
    let inserted: String = new[prefix..new.len() - suffix].iter().collect();
    if !inserted.is_empty() {
        text.insert(prefix, &inserted).map_err(engine)?;
    }
    let len = text.len_unicode();
    if len > 0 {
        let mut marks: Vec<String> = text
            .to_delta()
            .into_iter()
            .filter_map(|delta| match delta {
                TextDelta::Insert {
                    attributes: Some(attributes),
                    ..
                } => Some(attributes),
                _ => None,
            })
            .flatten()
            .filter(|(_, value)| !matches!(value, LoroValue::Null))
            .map(|(name, _)| name)
            .collect();
        marks.sort_unstable();
        marks.dedup();
        for mark in marks {
            text.unmark(0..len, &mark).map_err(engine)?;
        }
    }
    Ok(())
}
