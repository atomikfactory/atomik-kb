//! Import of Markdown, plain text, HTML, CSV, Notion exports, atomik-kb's own exports and
//! AppFlowy workspace backups (ADR-0036, `docs/architecture/import-export.md`, "Import";
//! ADR-0039, `docs/architecture/appflowy-import.md`).
//!
//! Two pure steps. [`plan`] turns the source's text files and the list of its other files into
//! an [`Plan`]: a tree of pages with portable content whose links and attachments are
//! placeholders, the properties they need and the files they use. [`resolve`] turns one planned
//! page's content into portable JSON the core can write, once the core knows the new page ids,
//! the uploaded attachments and the property ids. No I/O, no state; compiles for
//! `wasm32-unknown-unknown`. The core (`kb-core`, `import` module) runs the session.
//!
//! How the source is read:
//!
//! - `plan` builds the tree from the file names (a page's children are in the folder with its
//!   stem, a folder without a page is a page, Notion's ids are stripped), reads front matter
//!   (`yaml`, `frontmatter`), Markdown (`markdown`: pulldown-cmark, with Notion's `<aside>`,
//!   `<details>` and the annotations of atomik-kb's own Markdown), HTML (`html`: scraper), plain
//!   text (`text`) and CSV (`table`: a database page and a child page per row), and aggregates
//!   the properties (`props`).
//! - People (ADR-0049) are text: an export writes a person mention as `@label` and a Person
//!   property as names, never as account ids, so a mention comes back as the text `@label` and a
//!   sidecar's `person` property as a text property (with one `UnsupportedContent` warning).
//! - An AppFlowy backup (`relation_map.json` and `metadata.json` at the root) goes to its own
//!   reader (`appflowy`): the view tree, its spaces (`Plan::space_roots`), documents, databases
//!   with their rows, `local_files` as needed files and `https` images as remote files
//!   (`Plan::remote_files`). Only the JSON copies of the backup are read.
//! - Links and images follow one set of rules (`ctx`): pages of the source are planned pages,
//!   binary files of the source are needed files, nothing is ever fetched.
//! - The content of a planned page is portable JSON without block ids. Where it points at
//!   something the core must supply (a page, an attachment, a property, an option) it carries a
//!   placeholder attribute whose name starts with `$` (see `placeholder`); the format is private to
//!   this crate and [`resolve`] replaces every one.
//! - Hostile input is data: nesting is flattened to a bounded depth, HTML is parsed in pieces and
//!   stops building a tree past a fixed depth, front matter and tables have size limits, and
//!   nothing here panics or logs.

#![forbid(unsafe_code)]

mod annotations;
mod appflowy;
mod cover;
mod ctx;
mod emoji;
mod excalidraw;
#[cfg(test)]
mod export_round_trip_tests;
mod frontmatter;
#[cfg(test)]
mod fuzz_tests;
mod html;
mod htmltag;
mod inline;
mod json;
mod markdown;
mod model;
mod notion;
mod paths;
mod placeholder;
mod plan;
#[cfg(test)]
mod plan_tests;
mod props;
mod resolve;
#[cfg(test)]
mod resolve_tests;
mod sidecar;
#[cfg(test)]
mod stress_tests;
mod table;
#[cfg(test)]
mod testing;
mod text;
pub mod values;
mod yaml;

pub use cover::PlannedCover;
use kb_types::{
    AttachmentId, ImportWarning, OptionColor, OptionId, PageId, PropertyId, PropertyKind, Rng,
    RollupFunction, StatusGroup, TemplatePlaceholder,
};
use serde_json::Value;

/// A text file of the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Path relative to the source root, `/`-separated.
    pub path: String,
    /// Its text.
    pub text: String,
}

/// Options of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOptions {
    /// Name of the source: several top-level pages go into one container page of this title.
    /// `None`: never a container (files picked one by one).
    pub source_name: Option<String>,
    /// Most pages to plan; files beyond are left out with a `Truncated` warning.
    pub max_pages: usize,
}

/// What an import will create.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    /// Pages in creation order: a parent before its children, siblings in their final order.
    pub pages: Vec<PlannedPage>,
    /// Properties the pages' values need, by name, aggregated over every page (one kind each).
    pub properties: Vec<PlannedProperty>,
    /// Paths among the binary paths that the pages use (images, files, `files` values).
    pub needed_files: Vec<String>,
    /// The original name of each needed file, parallel to [`Plan::needed_files`] (same order and
    /// length) when the source gives names: `Some(name)` from the first block or cell that uses
    /// the file and names it (an AppFlowy `file` block or media cell, whose files are stored under
    /// the hash of their content), `None` otherwise. Empty (not parallel) for every other source:
    /// the file's own name is its last path segment.
    pub needed_file_names: Vec<Option<String>>,
    /// Problems found while planning.
    pub warnings: Vec<ImportWarning>,
    /// Indexes of the planned pages that stand for the source's spaces (an AppFlowy backup's
    /// spaces, ADR-0039), in order; empty for other sources. Into new spaces, each becomes a
    /// space and its children that space's root pages; into a page, they are container pages.
    pub space_roots: Vec<usize>,
    /// Images the source keeps on a server (ADR-0039): the core lists them in
    /// `ImportPlanInfo::remote_files`; their blocks carry the key as their file placeholder.
    pub remote_files: Vec<RemoteFile>,
}

/// A file of the source that lives on a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// Key the shell reports the downloaded attachment under (`remote/<n>`, n from 0).
    pub key: String,
    /// Its `https` URL.
    pub url: String,
    /// The URL's host.
    pub host: String,
}

/// A page to create.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedPage {
    /// Index of the parent in [`Plan::pages`]; `None` for a page directly under the destination.
    pub parent: Option<usize>,
    /// Title.
    pub title: String,
    /// Icon (an emoji or `icon:<name>[:<color>]`).
    pub icon: Option<String>,
    /// Cover (ADR-0040): a named one (`color:`, `gradient:`) or a file of the source that is
    /// uploaded and used when it is an image. A file cover's path or key is also listed in
    /// [`Plan::needed_files`] or [`Plan::remote_files`].
    pub cover: Option<PlannedCover>,
    /// The cover's vertical position in percent (0 to 100); `None` is the default 50.
    pub cover_y: Option<u8>,
    /// The page is a template (ADR-0056): the front matter of an atomik-kb export says `template: true`.
    pub template: bool,
    /// The placeholders of the page by property name, as the front matter lists them under
    /// `templatePlaceholders` (ADR-0056): `today`, `now` or `me`. The core matches the names against the
    /// space's properties and keeps the ones whose property takes the placeholder.
    pub template_placeholders: Vec<(String, TemplatePlaceholder)>,
    /// The source file it comes from (empty for a folder page or a CSV row without a file).
    pub source_path: String,
    /// Portable page JSON (`{"type": "doc", ...}`) with placeholders, private to this crate,
    /// for links, attachments and schema ids; [`resolve`] replaces them.
    pub content: Value,
    /// Property values by property name (a name of [`Plan::properties`]).
    pub properties: Vec<(String, ImportValue)>,
}

impl PlannedPage {
    /// The file the page's cover uses, when it is one: a path of [`Plan::needed_files`] or a key of
    /// [`Plan::remote_files`].
    #[must_use]
    pub fn cover_file(&self) -> Option<&str> {
        match &self.cover {
            Some(PlannedCover::File(file)) => Some(file),
            _ => None,
        }
    }

    /// The files the page's content uses (images, files, gallery items): needed paths of
    /// [`Plan::needed_files`] and keys of [`Plan::remote_files`], in document order. Property
    /// values list theirs in [`ImportValue::Files`].
    #[must_use]
    pub fn file_refs(&self) -> Vec<&str> {
        let mut out = Vec::new();
        placeholder::file_refs(&self.content, &mut out, 0);
        out
    }

    /// The names of the planned properties the saved views (`pageList` blocks) of the page's
    /// content use: columns, group, sort, filters and column widths.
    #[must_use]
    pub fn view_property_names(&self) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        placeholder::view_property_names(&self.content, &mut out, 0);
        out
    }
}

/// A property the import needs.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedProperty {
    /// Name (matched case-insensitively against the space's schema; an existing property of
    /// that name is reused whatever its kind).
    pub name: String,
    /// Kind of a property to create.
    pub kind: PropertyKind,
    /// Options of a select, multi-select or status property, in order.
    pub options: Vec<PlannedOption>,
    /// Relation, rollup or formula configuration (from an atomik-kb sidecar).
    pub config: Option<PlannedConfig>,
}

/// An option of a planned property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOption {
    /// Name.
    pub name: String,
    /// Colour (from a sidecar; the core picks the next palette colour otherwise).
    pub color: Option<OptionColor>,
    /// Group of a status option (from a sidecar; to-do otherwise).
    pub group: Option<StatusGroup>,
}

/// Configuration of a planned relation, rollup or formula, with names instead of ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedConfig {
    /// A relation.
    Relation {
        /// Two-way.
        two_way: bool,
        /// Name of the paired property (two-way only).
        pair_name: Option<String>,
    },
    /// A rollup.
    Rollup {
        /// Name of the relation property.
        relation: String,
        /// What to summarise.
        target: PlannedRollupTarget,
        /// How.
        function: RollupFunction,
    },
    /// A formula: its expression in display form, properties named as a person reads them
    /// (`prop("Price")`); the core turns the names into the new space's property ids.
    Formula {
        /// The expression as the sidecar wrote it.
        expression: String,
    },
}

/// Target of a planned rollup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedRollupTarget {
    /// The related pages' titles.
    Title,
    /// A property of the related pages, by name.
    Property(String),
}

/// A property value as read from the source, before it meets the property's kind.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportValue {
    /// Text.
    Text(String),
    /// A number.
    Number(f64),
    /// A boolean.
    Bool(bool),
    /// A floating local date with an optional time.
    Date {
        /// `YYYY-MM-DD`.
        date: String,
        /// `HH:mm`.
        time: Option<String>,
    },
    /// Several texts (option names, tags).
    List(Vec<String>),
    /// Related pages.
    Pages(Vec<PageTarget>),
    /// Source paths of attachments (paths of [`Plan::needed_files`]).
    Files(Vec<String>),
    /// Checklist items (done, text).
    Checklist(Vec<(bool, String)>),
    /// A place.
    Place {
        /// Name.
        name: String,
        /// Latitude.
        lat: Option<f64>,
        /// Longitude.
        lon: Option<f64>,
    },
}

/// A page a value or a link points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTarget {
    /// A planned page, by index.
    Planned(usize),
    /// An existing page of the destination space, by exact title (resolved by the core).
    Title(String),
}

/// Plans an import. Never fails: what it cannot read becomes a warning.
///
/// `files` are the text files of the source (path relative to the source root, `/`-separated),
/// `binary_paths` its other files. The pages come in creation order (a parent before its
/// children, siblings in their final order) and refer to each other by index; at most
/// `options.max_pages` are planned. A CSV file is a page whose content is one table view of its
/// children (a `pageList` block whose columns are the properties it needs) with a child page per
/// row; the core writes it like any other page and resolves the view's column names to property
/// ids in [`resolve`], after the properties exist.
#[must_use]
pub fn plan(files: Vec<SourceFile>, binary_paths: &[String], options: &ImportOptions) -> Plan {
    // An AppFlowy backup is recognised after the usual stripping of one wrapping folder; any
    // other source goes through the file-based planner.
    match appflowy::plan(files, binary_paths, options) {
        Ok(plan) => plan,
        Err(files) => plan::run(files, binary_paths, options),
    }
}

/// What [`resolve`] asks the core.
pub trait Resolver {
    /// The id given to a planned page.
    fn page(&self, index: usize) -> Option<PageId>;
    /// A live page of the destination space with exactly this title (unique), for wiki links
    /// and `[[Title]]` relation values that match no planned page.
    fn page_by_title(&self, title: &str) -> Option<PageId>;
    /// Whether a page id read from the source (an `atomik-kb://page/...` link) is a live page
    /// of the destination space.
    fn existing_page(&self, page: PageId) -> bool;
    /// The attachment uploaded for a needed file.
    fn attachment(&self, path: &str) -> Option<AttachmentId>;
    /// Whether an attachment id read from the source (an `attachment://...` link) is an
    /// attachment of the destination space.
    fn existing_attachment(&self, attachment: AttachmentId) -> bool;
    /// The id of a property by name (after the core created or matched it).
    fn property(&self, name: &str) -> Option<PropertyId>;
    /// The id of an option of a property, both by name.
    fn option(&self, property: &str, option: &str) -> Option<OptionId>;
}

/// The writable portable JSON of a planned page: placeholders replaced by ids, links whose
/// target is missing turned into text, and a fresh block id on every block that needs one.
///
/// The result is accepted by `kb_model::PageDoc::from_portable` and passes `check_invariants`:
/// every id-bearing node has a fresh `BlockId`, page and attachment references are valid ids, and
/// the elements of whiteboards sit in the top-level `whiteboards` object under the new block id.
/// Warnings (with the page's source path): `UnresolvedLink` for a page reference that does not
/// resolve (turned into its text), `MissingFile` for an image, file or gallery item whose
/// attachment is missing (turned into its text or dropped).
#[must_use]
pub fn resolve(
    page: &PlannedPage,
    resolver: &dyn Resolver,
    rng: &dyn Rng,
) -> (Value, Vec<ImportWarning>) {
    resolve::run(page, resolver, rng)
}
