//! The plan under construction: an arena of pages (files, folders, CSV databases and their rows)
//! before it is flattened into [`crate::Plan`].

use serde_json::Value;

use kb_types::TemplatePlaceholder;

use crate::{ImportValue, PlannedCover};

/// A page a planned value points to, not yet resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingTarget {
    /// A planned page, by arena index.
    Planned(usize),
    /// A page named in the source (`[[Title]]`): a planned page with that title or name, else an
    /// existing page of the space by title.
    Wiki(String),
    /// An existing page of the space, by exact title.
    Title(String),
}

/// A property value as read, before the planned pages exist.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawValue {
    /// A value that needs no resolution.
    Plain(ImportValue),
    /// Related pages.
    Pages(Vec<PendingTarget>),
}

/// What a page of the arena is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// A Markdown, text or HTML file.
    File,
    /// A CSV file: a database page.
    Csv,
    /// A folder that holds pages but has no page file of its own.
    Folder,
    /// A row of a CSV database that has no page file of its own.
    Row,
    /// The page named after the source, holding several top-level pages.
    Container,
}

/// One page of the plan.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    /// What it is.
    pub kind: NodeKind,
    /// Its parent.
    pub parent: Option<usize>,
    /// Its children, in the order they were found (sorted at the end).
    pub children: Vec<usize>,
    /// The file's path as the shell gave it (empty for folders, rows and the container).
    pub orig: String,
    /// The file's normalised path.
    pub path: String,
    /// The folder of the file, normalised.
    pub dir: String,
    /// The name for people: the stem without its Notion id.
    pub display: String,
    /// The Notion id in the file's name.
    pub notion_id: Option<String>,
    /// The title.
    pub title: String,
    /// The icon.
    pub icon: Option<String>,
    /// The cover (front matter, ADR-0040); a file cover is also in `needed`.
    pub cover: Option<PlannedCover>,
    /// The cover's vertical position in percent.
    pub cover_y: Option<u8>,
    /// The page is a template (front matter `template: true`, ADR-0056).
    pub template: bool,
    /// The placeholders the front matter lists, by property name (ADR-0056).
    pub template_placeholders: Vec<(String, TemplatePlaceholder)>,
    /// The blocks of the content.
    pub blocks: Vec<Value>,
    /// The pages the content links to, in order of first link.
    pub links: Vec<usize>,
    /// The files the content uses, as the shell gave their paths, in order of first use.
    pub needed: Vec<String>,
    /// The values of properties the page itself holds (front matter), by name.
    pub props: Vec<(String, RawValue)>,
    /// The values of the page's row of a CSV database, by column name.
    pub row_props: Vec<(String, RawValue)>,
    /// For a CSV row: its position in the CSV, to keep the CSV's order.
    pub row_index: Option<usize>,
    /// The property lines at the top of the page were removed (its row has the values).
    pub stripped: bool,
}

impl Node {
    /// A node with nothing in it.
    pub(crate) fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            parent: None,
            children: Vec::new(),
            orig: String::new(),
            path: String::new(),
            dir: String::new(),
            display: String::new(),
            notion_id: None,
            title: String::new(),
            icon: None,
            cover: None,
            cover_y: None,
            template: false,
            template_placeholders: Vec::new(),
            blocks: Vec::new(),
            links: Vec::new(),
            needed: Vec::new(),
            props: Vec::new(),
            row_props: Vec::new(),
            row_index: None,
            stripped: false,
        }
    }
}
