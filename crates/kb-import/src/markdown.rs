//! Markdown into blocks (ADR-0036, section 3.4): CommonMark and GFM through `pulldown-cmark`,
//! with the callouts, annotations, wiki links and HTML that Notion, Obsidian and atomik-kb's own
//! exports use.
//!
//! The events are consumed into a stack of frames (quotes, list items, details, columns...).
//! Containers that would nest deeper than the JSON budget are flattened into their parent, so no
//! structure ever gets deeper than the page document accepts, whatever the input.

use kb_types::ImportWarningCode;
use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd,
};
use serde_json::{Map, Value};

use crate::annotations::{self, Annotation, MAX_GALLERY_ITEMS};
use crate::ctx::{ImageAction, LinkAction, PageCtx};
use crate::emoji;
use crate::html::{self, MAX_JSON_DEPTH, MAX_TABLE_COLS, MAX_TABLE_ROWS, math_block, math_inline};
use crate::htmltag::{self, Tag as HtmlTag, Token};
use crate::inline::{InlineRun, Role};
use crate::json::{self, MarkSet, attrs, element, kind, plain, plain_text, set_attr};
use crate::paths;
use crate::placeholder::{self, Target};
use crate::values;

/// How deep Markdown inside HTML inside Markdown is parsed.
const MAX_SUBPARSE: usize = 4;
/// The icon of a callout that names none.
const DEFAULT_ICON: &str = "💡";

/// The parser options: everything GFM and Obsidian write.
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_WIKILINKS
}

/// Reads Markdown (front matter already cut off) into blocks.
pub(crate) fn convert(ctx: &mut PageCtx<'_>, source: &str) -> Vec<Value> {
    convert_at(ctx, source, 0)
}

fn convert_at(ctx: &mut PageCtx<'_>, source: &str, depth: usize) -> Vec<Value> {
    let mut conv = Conv::new(ctx, depth);
    for (event, range) in Parser::new_ext(source, options()).into_offset_iter() {
        let event = match event {
            // A closing `$` followed by a digit is a price (`$5-$10`), not the end of an
            // equation: the paste parser of the editor reads it the same way.
            Event::InlineMath(latex)
                if source
                    .get(range.end..)
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())) =>
            {
                Event::Text(format!("${latex}$").into())
            }
            other => other,
        };
        conv.event(event);
    }
    conv.finish()
}

/// Removes the icon a list of blocks starts with (an emoji or symbol in front of a space, in the
/// first paragraph) and answers it.
pub(crate) fn strip_leading_icon(blocks: &mut Vec<Value>) -> Option<String> {
    strip_icon(blocks, None)
}

fn strip_icon(blocks: &mut Vec<Value>, known: Option<&str>) -> Option<String> {
    let only_block = blocks.len() == 1;
    let first = blocks.first_mut()?;
    if kind(first) != "paragraph" {
        return None;
    }
    let content = first.get_mut("content")?.as_array_mut()?;
    let node = content.first_mut()?;
    if kind(node) != "text" {
        return None;
    }
    let text = node.get("text")?.as_str()?.to_owned();
    let (icon, rest) = match known {
        Some(icon) => {
            let trimmed = text.trim_start();
            let rest = trimmed.strip_prefix(icon)?;
            if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                return None;
            }
            (icon.to_owned(), rest.trim_start().to_owned())
        }
        None => {
            let (icon, rest) = emoji::split_icon(&text)?;
            (icon.to_owned(), rest.to_owned())
        }
    };
    if rest.is_empty() {
        content.remove(0);
        if content.is_empty() {
            if only_block {
                if let Value::Object(object) = first {
                    object.remove("content");
                }
            } else {
                blocks.remove(0);
            }
        }
    } else if let Value::Object(object) = node {
        object.insert("text".into(), Value::String(rest));
    }
    Some(icon)
}

// ----- the frames ----------------------------------------------------------------------------

struct ItemData {
    task: Option<bool>,
    blocks: Vec<Value>,
}

struct CalloutNote {
    icon: Option<String>,
    color: Option<String>,
}

enum FrameKind {
    Root,
    Quote {
        alert: Option<BlockQuoteKind>,
        annotated: Option<CalloutNote>,
        skip: bool,
    },
    List {
        start: Option<u64>,
        items: Vec<ItemData>,
    },
    Item {
        task: Option<bool>,
    },
    Footnote {
        label: String,
    },
    Cell {
        header: bool,
    },
    Details {
        open: bool,
        summary: String,
        /// The level of a toggle heading (its summary is one heading element).
        level: Option<u8>,
    },
    Aside,
    Columns {
        columns: Vec<Vec<Value>>,
        started: bool,
    },
    /// A template button (ADR-0056): its blocks are its content.
    TemplateButton {
        label: Option<String>,
        position: Option<String>,
    },
}

struct Frame {
    kind: FrameKind,
    blocks: Vec<Value>,
    weight: usize,
    /// Over the JSON budget: its content goes to the parent as it is.
    transparent: bool,
    /// The frame whose `blocks` take the blocks read inside this one: itself, or, when it is
    /// transparent, the nearest frame below it that is not (so flattening moves nothing).
    sink: usize,
    /// Opened by HTML or an annotation, not by the Markdown parser.
    html: bool,
}

#[derive(Default)]
struct Counters {
    bold: u32,
    italic: u32,
    strike: u32,
    /// Marks opened by HTML tags: they do not outlive their paragraph.
    underline: u32,
    highlight: u32,
    html_bold: u32,
    html_italic: u32,
    html_strike: u32,
}

struct LinkState {
    action: LinkAction,
    text: String,
    /// `Some(has_pothole)` for a wiki link.
    wiki: Option<bool>,
    /// The wiki target as written.
    target: String,
}

struct ImageState {
    dest: String,
    title: String,
    alt: String,
    wiki: bool,
}

struct CodeState {
    language: Option<String>,
    text: String,
}

#[derive(Default)]
struct TableState {
    rows: Vec<Vec<Value>>,
    row: Vec<Value>,
    header: bool,
    flatten: bool,
    cut: bool,
}

struct GalleryState {
    columns: Option<i64>,
    expected: usize,
    items: Vec<Value>,
    depth: usize,
}

#[derive(Default)]
struct Pending {
    callout: Option<CalloutNote>,
    attrs: Option<Map<String, Value>>,
    gallery: Option<GalleryState>,
    skip_quote: bool,
}

struct Conv<'c, 'a> {
    ctx: &'c mut PageCtx<'a>,
    md_depth: usize,
    frames: Vec<Frame>,
    used: usize,
    run: InlineRun,
    counters: Counters,
    heading: Option<u8>,
    link: Option<LinkState>,
    image: Option<ImageState>,
    code: Option<CodeState>,
    html: Option<String>,
    table: Option<TableState>,
    footnotes: Vec<(String, Vec<Value>)>,
    pending: Pending,
    skip_time: bool,
    last_br: bool,
    in_summary: bool,
    /// The link being read was opened by an `<a>` tag.
    html_link: bool,
    summary: String,
}

impl<'c, 'a> Conv<'c, 'a> {
    fn new(ctx: &'c mut PageCtx<'a>, md_depth: usize) -> Self {
        Self {
            ctx,
            md_depth,
            frames: vec![Frame {
                kind: FrameKind::Root,
                blocks: Vec::new(),
                weight: 0,
                transparent: false,
                sink: 0,
                html: false,
            }],
            used: 0,
            run: InlineRun::default(),
            counters: Counters::default(),
            heading: None,
            link: None,
            image: None,
            code: None,
            html: None,
            table: None,
            footnotes: Vec::new(),
            pending: Pending::default(),
            skip_time: false,
            last_br: false,
            in_summary: false,
            html_link: false,
            summary: String::new(),
        }
    }

    // ----- events ---------------------------------------------------------------------------

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self.push_code(&code),
            Event::InlineMath(latex) => self.inline_math(&latex),
            Event::DisplayMath(latex) => self.display_math(&latex),
            Event::Html(html) => match &mut self.html {
                Some(buffer) => buffer.push_str(&html),
                None => self.inline_html(&html),
            },
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::FootnoteReference(label) => self.push_text(&format!("[{label}]")),
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.hard_break(),
            Event::Rule => {
                self.flush_run();
                self.push_block(plain("horizontalRule", Vec::new()));
            }
            Event::TaskListMarker(done) => self.task_marker(done),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.forget_notes(),
            Tag::Heading { level, .. } => {
                self.flush_run();
                self.forget_notes();
                self.heading = Some(level as u8);
            }
            Tag::BlockQuote(alert) => {
                self.flush_run();
                let annotated = self.pending.callout.take();
                let skip = std::mem::take(&mut self.pending.skip_quote);
                self.open(
                    FrameKind::Quote {
                        alert,
                        annotated,
                        skip,
                    },
                    1,
                    false,
                );
            }
            Tag::CodeBlock(code_kind) => {
                self.flush_run();
                self.forget_notes();
                let language = match code_kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .map(|word| word.trim_matches(['{', '}', '.']).to_owned())
                        .filter(|word| !word.is_empty()),
                    CodeBlockKind::Indented => None,
                };
                self.code = Some(CodeState {
                    language,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => {
                self.flush_run();
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.flush_run();
                self.forget_notes();
                self.open(
                    FrameKind::List {
                        start,
                        items: Vec::new(),
                    },
                    1,
                    false,
                );
            }
            Tag::Item => self.open(FrameKind::Item { task: None }, 1, false),
            Tag::FootnoteDefinition(label) => {
                self.flush_run();
                self.open(
                    FrameKind::Footnote {
                        label: label.to_string(),
                    },
                    0,
                    false,
                );
            }
            Tag::Table(_) => {
                self.flush_run();
                self.forget_notes();
                self.table = Some(TableState {
                    flatten: !self.fits(3),
                    ..TableState::default()
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.header = true;
                    table.row.clear();
                }
            }
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.header = false;
                    table.row.clear();
                }
            }
            Tag::TableCell => {
                let (header, flatten) = self
                    .table
                    .as_ref()
                    .map_or((false, true), |t| (t.header, t.flatten));
                self.open(
                    FrameKind::Cell { header },
                    if flatten { 0 } else { 3 },
                    false,
                );
                if flatten && let Some(frame) = self.frames.last_mut() {
                    frame.transparent = true;
                }
            }
            Tag::Emphasis => self.counters.italic += 1,
            Tag::Strong => self.counters.bold += 1,
            Tag::Strikethrough => self.counters.strike += 1,
            Tag::Superscript | Tag::Subscript => {}
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => self.begin_link(link_type, &dest_url),
            Tag::Image {
                link_type,
                dest_url,
                title,
                ..
            } => {
                self.image = Some(ImageState {
                    dest: dest_url.to_string(),
                    title: title.to_string(),
                    alt: String::new(),
                    wiki: matches!(link_type, LinkType::WikiLink { .. }),
                });
            }
            Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_run(),
            TagEnd::Heading(_) => self.end_heading(),
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::FootnoteDefinition => {
                self.flush_run();
                self.close_html_frames();
                self.close_top();
            }
            TagEnd::CodeBlock => self.end_code(),
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    self.html_block(&html);
                }
            }
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    if table.rows.len() < MAX_TABLE_ROWS {
                        table.rows.push(row);
                    } else {
                        table.cut = true;
                    }
                    table.header = false;
                }
            }
            TagEnd::TableCell => {
                self.flush_run();
                self.close_top();
            }
            TagEnd::Emphasis => self.counters.italic = self.counters.italic.saturating_sub(1),
            TagEnd::Strong => self.counters.bold = self.counters.bold.saturating_sub(1),
            TagEnd::Strikethrough => {
                self.counters.strike = self.counters.strike.saturating_sub(1);
            }
            TagEnd::Link => self.end_link(),
            TagEnd::Image => self.end_image(),
            TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
        }
    }

    // ----- frames ---------------------------------------------------------------------------

    fn fits(&self, weight: usize) -> bool {
        self.used + weight <= MAX_JSON_DEPTH
    }

    fn open(&mut self, kind: FrameKind, weight: usize, html: bool) {
        let transparent = !self.fits(weight);
        let weight = if transparent { 0 } else { weight };
        self.used += weight;
        let sink = if transparent {
            self.frames.last().map_or(0, |frame| frame.sink)
        } else {
            self.frames.len()
        };
        self.frames.push(Frame {
            kind,
            blocks: Vec::new(),
            weight,
            transparent,
            sink,
            html,
        });
    }

    /// Closes the frames that HTML or annotations opened and that the Markdown parser's frame
    /// below them is about to close.
    fn close_html_frames(&mut self) {
        while self.frames.len() > 1 && self.frames.last().is_some_and(|f| f.html) {
            self.close_top();
        }
    }

    /// The frame on top receives a block (through the pending annotations).
    fn push_block(&mut self, mut block: Value) {
        let depth = self.frames.len();
        if self
            .pending
            .gallery
            .as_ref()
            .is_some_and(|g| g.depth == depth)
        {
            if kind(&block) == "image"
                && let Some(item) = gallery_item(&block)
                && let Some(gallery) = &mut self.pending.gallery
            {
                gallery.items.push(item);
                if gallery.items.len() >= gallery.expected {
                    self.end_gallery();
                }
                return;
            }
            self.end_gallery();
        }
        if let Some(attrs) = self.pending.attrs.take() {
            apply_media_attrs(&mut block, &attrs);
        }
        self.pending.callout = None;
        self.pending.skip_quote = false;
        self.sink_push(block);
    }

    /// Adds a block to the frame that takes the blocks read in the one on top.
    fn sink_push(&mut self, block: Value) {
        let Some(sink) = self.frames.last().map(|frame| frame.sink) else {
            return;
        };
        if let Some(frame) = self.frames.get_mut(sink) {
            frame.blocks.push(block);
        }
    }

    /// Gives the pending gallery its block.
    fn end_gallery(&mut self) {
        let depth = self.frames.len();
        if !self
            .pending
            .gallery
            .as_ref()
            .is_some_and(|g| g.depth == depth)
        {
            return;
        }
        let Some(gallery) = self.pending.gallery.take() else {
            return;
        };
        if gallery.items.is_empty() {
            return;
        }
        let mut map = attrs([("items", Value::Array(gallery.items))]);
        if let Some(columns) = gallery.columns {
            map.insert("columns".into(), Value::from(columns));
        }
        self.sink_push(element("gallery", map, Vec::new()));
    }

    fn extend_parent(&mut self, blocks: Vec<Value>) {
        for block in blocks {
            self.sink_push(block);
        }
    }

    /// Closes the top frame and gives its block to the one below.
    fn close_top(&mut self) {
        self.end_gallery();
        if self.frames.len() <= 1 {
            return;
        }
        let Some(frame) = self.frames.pop() else {
            return;
        };
        self.used = self.used.saturating_sub(frame.weight);
        let Frame {
            kind,
            blocks,
            transparent,
            ..
        } = frame;
        match kind {
            FrameKind::Root => {}
            FrameKind::Footnote { label } => self.footnotes.push((label, blocks)),
            FrameKind::Cell { header } => {
                if transparent {
                    self.extend_parent(blocks);
                } else if let Some(table) = &mut self.table
                    && table.row.len() < MAX_TABLE_COLS
                {
                    table.row.push(cell_node(header, blocks));
                }
            }
            _ if transparent => self.extend_parent(blocks),
            FrameKind::Quote {
                alert,
                annotated,
                skip,
            } => {
                if skip {
                    return;
                }
                let node = quote_node(alert, annotated, blocks);
                self.push_block(node);
            }
            FrameKind::List { start, items } => {
                if items.is_empty() {
                    self.extend_parent(blocks);
                } else {
                    let node = list_node(start, items);
                    self.push_block(node);
                    self.extend_parent(blocks);
                }
            }
            FrameKind::Item { task } => {
                match self.frames.last_mut() {
                    Some(Frame {
                        kind: FrameKind::List { items, .. },
                        transparent: false,
                        ..
                    }) => items.push(ItemData { task, blocks }),
                    _ => self.extend_parent(blocks),
                };
            }
            FrameKind::Details {
                open,
                summary,
                level,
            } => {
                let node = details_node(open, &summary, level, blocks);
                self.push_block(node);
            }
            FrameKind::Aside => {
                let mut blocks = blocks;
                let icon =
                    strip_leading_icon(&mut blocks).unwrap_or_else(|| DEFAULT_ICON.to_owned());
                let node = callout_node(&icon, None, blocks);
                self.push_block(node);
            }
            FrameKind::TemplateButton { label, position } => {
                let mut map = Map::new();
                if let Some(label) = label {
                    map.insert("label".into(), Value::String(label));
                }
                if let Some(position) = position {
                    map.insert("position".into(), Value::String(position));
                }
                let node = element("templateButton", map, ensure_blocks(blocks));
                self.push_block(node);
            }
            FrameKind::Columns {
                mut columns,
                started,
            } => {
                if started || !blocks.is_empty() {
                    columns.push(blocks);
                }
                if columns.len() < 2 {
                    let flat: Vec<Value> = columns.into_iter().flatten().collect();
                    self.extend_parent(flat);
                } else {
                    let node = columns_node(columns);
                    self.push_block(node);
                }
            }
        }
    }

    fn forget_notes(&mut self) {
        self.pending.callout = None;
        self.pending.skip_quote = false;
    }

    // ----- inline content -------------------------------------------------------------------

    fn marks(&self) -> MarkSet {
        MarkSet {
            bold: self.counters.bold + self.counters.html_bold > 0,
            italic: self.counters.italic + self.counters.html_italic > 0,
            underline: self.counters.underline > 0,
            strike: self.counters.strike + self.counters.html_strike > 0,
            code: false,
            link: match &self.link {
                Some(LinkState {
                    action: LinkAction::Mark(url),
                    ..
                }) => Some(url.clone()),
                _ => None,
            },
            text_color: None,
            highlight: (self.counters.highlight > 0).then(|| "yellow".to_owned()),
        }
    }

    /// Text goes where the context says: an image's alternative text, a code block, the text of a
    /// link that is replaced by a reference, or the paragraph.
    fn push_text(&mut self, text: &str) {
        if self.skip_time || text.is_empty() {
            return;
        }
        self.last_br = false;
        if let Some(image) = &mut self.image {
            image.alt.push_str(text);
            return;
        }
        if let Some(code) = &mut self.code {
            code.text.push_str(text);
            return;
        }
        let mut role = Role::Normal;
        if let Some(link) = &mut self.link {
            link.text.push_str(text);
            match link.action {
                LinkAction::Inline(_) => return,
                LinkAction::After(_) => role = Role::FileLinkText,
                _ => {}
            }
        }
        let marks = self.marks();
        self.run.push_text(text, &marks, role);
    }

    fn push_code(&mut self, text: &str) {
        if self.skip_time || text.is_empty() {
            return;
        }
        self.last_br = false;
        if let Some(image) = &mut self.image {
            image.alt.push_str(text);
            return;
        }
        let mut role = Role::Normal;
        if let Some(link) = &mut self.link {
            link.text.push_str(text);
            match link.action {
                LinkAction::Inline(_) => return,
                LinkAction::After(_) => role = Role::FileLinkText,
                _ => {}
            }
        }
        let marks = MarkSet {
            code: true,
            ..MarkSet::default()
        };
        self.run.push_text(text, &marks, role);
    }

    fn push_node(&mut self, node: Value) {
        if self.skip_time {
            return;
        }
        self.last_br = false;
        if self.image.is_some() {
            return;
        }
        self.run.push_node(node);
    }

    fn hard_break(&mut self) {
        if self.skip_time {
            return;
        }
        if let Some(image) = &mut self.image {
            image.alt.push(' ');
            return;
        }
        self.last_br = false;
        self.run.push_hard_break();
    }

    fn inline_math(&mut self, latex: &str) {
        if let Some(image) = &mut self.image {
            image.alt.push_str(latex);
            return;
        }
        if let Some(link) = &mut self.link {
            link.text.push_str(latex);
            if matches!(link.action, LinkAction::Inline(_)) {
                return;
            }
        }
        if !latex.trim().is_empty() {
            self.push_node(math_inline(latex));
        }
    }

    fn display_math(&mut self, latex: &str) {
        if let Some(image) = &mut self.image {
            image.alt.push_str(latex);
            return;
        }
        self.run.push_block(math_block(latex.trim()));
    }

    /// The paragraph being read ends: its blocks go to the frame on top.
    fn flush_run(&mut self) {
        // An `<a>` that was never closed does not reach into the next paragraph.
        if std::mem::take(&mut self.html_link) {
            self.end_link();
        }
        self.counters.underline = 0;
        self.counters.highlight = 0;
        self.counters.html_bold = 0;
        self.counters.html_italic = 0;
        self.counters.html_strike = 0;
        self.skip_time = false;
        self.last_br = false;
        let run = std::mem::take(&mut self.run);
        for block in run.into_blocks() {
            self.push_block(block);
        }
    }

    fn end_heading(&mut self) {
        let Some(level) = self.heading.take() else {
            self.flush_run();
            return;
        };
        if std::mem::take(&mut self.html_link) {
            self.end_link();
        }
        self.counters.underline = 0;
        self.counters.highlight = 0;
        self.counters.html_bold = 0;
        self.counters.html_italic = 0;
        self.counters.html_strike = 0;
        let (content, blocks) = std::mem::take(&mut self.run).into_inline();
        // A heading that is only a picture is the picture.
        if !(content.is_empty() && !blocks.is_empty()) {
            self.push_block(json::heading(level, content));
        }
        for block in blocks {
            self.push_block(block);
        }
    }

    fn task_marker(&mut self, done: bool) {
        if let Some(Frame {
            kind: FrameKind::Item { task },
            ..
        }) = self.frames.last_mut()
        {
            *task = Some(done);
        }
    }

    // ----- links and images -----------------------------------------------------------------

    fn begin_link(&mut self, link_type: LinkType, dest: &str) {
        if self.image.is_some() || self.link.is_some() {
            return;
        }
        let wiki = match link_type {
            LinkType::WikiLink { has_pothole } => Some(has_pothole),
            _ => None,
        };
        let (action, target) = if wiki.is_some() {
            let target = wiki_target(dest);
            // `[[#Heading]]` points inside the page: its text is all there is to keep.
            let action = if target.is_empty() {
                LinkAction::Plain
            } else {
                self.wiki_action(&target)
            };
            (action, target)
        } else if link_type == LinkType::Email {
            (
                self.ctx.link_action(&format!("mailto:{dest}")),
                String::new(),
            )
        } else {
            (self.ctx.link_action(dest), String::new())
        };
        self.link = Some(LinkState {
            action,
            text: String::new(),
            wiki,
            target,
        });
    }

    fn wiki_action(&mut self, target: &str) -> LinkAction {
        let name = paths::file_name(target);
        let (_, ext) = paths::split_ext(name);
        if !ext.is_empty()
            && !crate::ctx::PAGE_EXTENSIONS.contains(&ext.as_str())
            && let Some(norm) = self.ctx.index.binary_by_name(name)
            && let Some(orig) = self.ctx.index.binary.get(norm).cloned()
        {
            self.ctx.need(&orig);
            return LinkAction::After(if crate::ctx::is_image_path(&orig) {
                placeholder::image_file(&orig, "", "")
            } else {
                placeholder::file_path(&orig, "")
            });
        }
        LinkAction::Inline(placeholder::page_ref(
            &Target::Wiki {
                name: target.to_owned(),
                from: self.ctx.node,
            },
            "",
        ))
    }

    fn end_link(&mut self) {
        let Some(link) = self.link.take() else {
            return;
        };
        let text = link.text.trim().to_owned();
        match link.action {
            LinkAction::Inline(mut node) => {
                let label = match link.wiki {
                    Some(true) => format!("[[{}|{}]]", link.target, text),
                    Some(false) => format!("[[{}]]", link.target),
                    None => text,
                };
                if !label.is_empty() {
                    set_attr(&mut node, placeholder::LABEL, Value::String(label));
                }
                self.push_node(node);
            }
            LinkAction::After(mut block) => {
                crate::ctx::PageCtx::name_after_block(&mut block, &text);
                self.run.push_after(block);
            }
            LinkAction::Mark(_) | LinkAction::Plain => {}
        }
    }

    fn end_image(&mut self) {
        let Some(image) = self.image.take() else {
            return;
        };
        let alt = image.alt.trim().to_owned();
        if image.wiki {
            self.wiki_embed(&image.dest);
            return;
        }
        self.finish_image(&image.dest, &alt, &image.title);
    }

    fn finish_image(&mut self, dest: &str, alt: &str, title: &str) {
        match self.ctx.image_action(dest, alt, title) {
            ImageAction::Block(block) => self.run.push_block(block),
            ImageAction::Text { text, href } => {
                if text.is_empty() {
                    return;
                }
                let marks = MarkSet {
                    link: href,
                    ..self.marks()
                };
                self.last_br = false;
                self.run.push_text(&text, &marks, Role::Normal);
            }
        }
    }

    /// `![[photo.png]]` embeds a file of the source; `![[Note]]` links the page.
    fn wiki_embed(&mut self, dest: &str) {
        let target = wiki_target(dest);
        if target.is_empty() {
            return;
        }
        match self.wiki_action(&target) {
            LinkAction::After(block) => self.run.push_block(block),
            LinkAction::Inline(mut node) => {
                set_attr(
                    &mut node,
                    placeholder::LABEL,
                    Value::String(format!("[[{target}]]")),
                );
                self.run.push_node(node);
            }
            LinkAction::Mark(_) | LinkAction::Plain => {}
        }
    }

    // ----- inline HTML ----------------------------------------------------------------------

    fn inline_html(&mut self, raw: &str) {
        let Some((tag, _)) = htmltag::parse_tag(raw) else {
            return;
        };
        let counter = |count: &mut u32, closing: bool| {
            if closing {
                *count = count.saturating_sub(1);
            } else {
                *count += 1;
            }
        };
        match tag.name.as_str() {
            "u" | "ins" => counter(&mut self.counters.underline, tag.closing),
            "mark" => counter(&mut self.counters.highlight, tag.closing),
            "b" | "strong" => counter(&mut self.counters.html_bold, tag.closing),
            "i" | "em" => counter(&mut self.counters.html_italic, tag.closing),
            "s" | "del" | "strike" => counter(&mut self.counters.html_strike, tag.closing),
            "br" => self.line_break(),
            "time" => self.time_tag(&tag),
            "a" => {
                if tag.closing {
                    // Only a link that an `<a>` tag opened ends with `</a>`.
                    if std::mem::take(&mut self.html_link) {
                        self.end_link();
                    }
                } else if self.link.is_none()
                    && self.image.is_none()
                    && let Some(href) = tag.attr("href").filter(|h| !h.trim().is_empty())
                {
                    self.begin_link(LinkType::Inline, href);
                    self.html_link = self.link.is_some();
                }
            }
            "img" if !tag.closing => {
                if let Some(src) = tag.attr("src") {
                    let alt = tag.attr("alt").unwrap_or_default().to_owned();
                    let title = tag.attr("title").unwrap_or_default().to_owned();
                    if let Some(image) = &mut self.image {
                        image.alt.push_str(&alt);
                    } else {
                        self.finish_image(src, &alt, &title);
                    }
                }
            }
            _ => {}
        }
    }

    /// `<br>`: a hard break; in a table cell two in a row end the paragraph (the export writes
    /// them between the blocks of a cell).
    fn line_break(&mut self) {
        if self.skip_time || self.image.is_some() {
            return;
        }
        let in_cell = matches!(
            self.frames.last(),
            Some(Frame {
                kind: FrameKind::Cell { .. },
                ..
            })
        );
        if in_cell && self.last_br && self.run.pop_hard_break() {
            self.flush_run();
        } else {
            self.run.push_hard_break();
            self.last_br = true;
        }
    }

    fn time_tag(&mut self, tag: &HtmlTag) {
        if tag.closing {
            self.skip_time = false;
            return;
        }
        let Some((date, time)) = tag.attr("datetime").and_then(values::parse_date) else {
            return;
        };
        let mut map = attrs([("date", Value::String(date))]);
        if let Some(time) = time {
            map.insert("time".into(), Value::String(time));
        }
        if let Some(reminder) = tag
            .attr("data-reminder")
            .and_then(|r| r.trim().parse::<u32>().ok())
        {
            map.insert("reminder".into(), Value::from(reminder));
        }
        // Filled in when a page is made from a template (ADR-0056).
        if let Some(relative) = tag
            .attr("data-relative")
            .map(str::trim)
            .filter(|relative| matches!(*relative, "today" | "now"))
        {
            map.insert("relative".into(), Value::String(relative.to_owned()));
        }
        self.push_node(element("dateMention", map, Vec::new()));
        self.skip_time = true;
    }

    // ----- code, tables ---------------------------------------------------------------------

    fn end_code(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        let mut text = code.text;
        if text.ends_with('\n') {
            text.pop();
        }
        let block = if code.language.as_deref() == Some("math") {
            math_block(&text)
        } else {
            let mut map = Map::new();
            if let Some(language) = code.language {
                map.insert("language".into(), Value::String(language));
            }
            let content = if text.is_empty() {
                Vec::new()
            } else {
                vec![json::text_node(&text, &MarkSet::default())]
            };
            element("codeBlock", map, content)
        };
        self.push_block(block);
    }

    fn end_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        if table.cut {
            self.ctx
                .warn(ImportWarningCode::UnsupportedContent, Some("table rows"));
        }
        if table.flatten {
            return;
        }
        let width = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if width == 0 {
            return;
        }
        let rows: Vec<Value> = table
            .rows
            .into_iter()
            .map(|mut cells| {
                while cells.len() < width {
                    cells.push(cell_node(false, Vec::new()));
                }
                plain("tableRow", cells)
            })
            .collect();
        self.push_block(plain("table", rows));
    }

    // ----- HTML blocks and annotations ------------------------------------------------------

    fn html_block(&mut self, html: &str) {
        for token in htmltag::tokens(html) {
            match token {
                Token::Annotation(json) => {
                    if let Some(annotation) = annotations::parse(json) {
                        self.annotate(annotation);
                    }
                }
                Token::Container(tag) => self.container_tag(&tag),
                Token::Raw(text) => self.raw_html(text),
            }
        }
    }

    fn container_tag(&mut self, tag: &HtmlTag) {
        match (tag.name.as_str(), tag.closing) {
            ("details", false) => {
                if !tag.self_closing {
                    self.open(
                        FrameKind::Details {
                            open: tag.attr("open").is_some(),
                            summary: String::new(),
                            level: None,
                        },
                        2,
                        true,
                    );
                }
            }
            ("details", true) => self.close_html_kind(|k| matches!(k, FrameKind::Details { .. })),
            ("summary", false) => {
                self.in_summary = true;
                self.summary.clear();
            }
            ("summary", true) => {
                if std::mem::take(&mut self.in_summary) {
                    let source = std::mem::take(&mut self.summary);
                    // A summary that is one heading element is a toggle heading (ADR-0040).
                    let (heading, inner) = htmltag::summary_heading(&source)
                        .map_or((None, source.as_str()), |(level, inner)| {
                            (Some(level), inner)
                        });
                    let text = self.plain_of(inner);
                    let too_deep = self.frames.last().is_some_and(|frame| {
                        frame.transparent && matches!(frame.kind, FrameKind::Details { .. })
                    });
                    if too_deep {
                        // Too deep for a toggle: the summary is a paragraph of the blocks around.
                        if !text.is_empty() {
                            self.sink_push(json::text_paragraph(&text));
                        }
                    } else if let Some(Frame {
                        kind: FrameKind::Details { summary, level, .. },
                        ..
                    }) = self.frames.last_mut()
                    {
                        *summary = text;
                        *level = heading;
                    }
                }
            }
            ("aside", false) => {
                if !tag.self_closing {
                    self.open(FrameKind::Aside, 1, true);
                }
            }
            ("aside", true) => self.close_html_kind(|k| matches!(k, FrameKind::Aside)),
            _ => {}
        }
    }

    /// Closes the top frame when HTML opened it and `matches` says it is the right kind.
    fn close_html_kind(&mut self, matches: impl Fn(&FrameKind) -> bool) {
        self.flush_run();
        if self
            .frames
            .last()
            .is_some_and(|frame| frame.html && matches(&frame.kind))
        {
            self.close_top();
        }
    }

    /// Text and HTML between container tags: HTML goes through the HTML reader, anything else is
    /// Markdown (Notion's `<aside>` holds Markdown).
    fn raw_html(&mut self, text: &str) {
        if self.in_summary {
            self.summary.push_str(text);
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let blocks = if trimmed.starts_with('<') {
            html::convert_fragment(self.ctx, text)
        } else if self.md_depth < MAX_SUBPARSE {
            convert_at(self.ctx, text, self.md_depth + 1)
        } else {
            vec![json::text_paragraph(trimmed)]
        };
        for block in blocks {
            self.push_block(block);
        }
    }

    /// The plain text of a piece of Markdown (a summary).
    fn plain_of(&mut self, source: &str) -> String {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        if self.md_depth >= MAX_SUBPARSE {
            return trimmed.to_owned();
        }
        let blocks = convert_at(self.ctx, trimmed, self.md_depth + 1);
        let texts: Vec<String> = blocks.iter().map(plain_text).collect();
        texts.join(" ").trim().to_owned()
    }

    fn annotate(&mut self, annotation: Annotation) {
        match annotation {
            Annotation::Attrs(map) => self.pending.attrs = Some(map),
            Annotation::Callout { icon, color } => {
                self.pending.callout = Some(CalloutNote { icon, color });
            }
            Annotation::Gallery { columns, items } => {
                self.end_gallery();
                if items > 0 {
                    self.pending.gallery = Some(GalleryState {
                        columns,
                        expected: items.min(MAX_GALLERY_ITEMS),
                        items: Vec::new(),
                        depth: self.frames.len(),
                    });
                }
            }
            Annotation::Whiteboard { height, file } => {
                let mut block = self.whiteboard_of(file.as_deref());
                if let Some(height) = height {
                    set_attr(&mut block, "height", Value::from(height));
                }
                self.push_block(block);
                self.pending.skip_quote = true;
            }
            Annotation::ColumnList => self.open(
                FrameKind::Columns {
                    columns: Vec::new(),
                    started: false,
                },
                2,
                true,
            ),
            Annotation::Column => self.next_column(),
            Annotation::EndColumnList => {
                self.close_html_kind(|k| matches!(k, FrameKind::Columns { .. }));
            }
            Annotation::TemplateButton { label, position } => {
                self.end_gallery();
                self.open(FrameKind::TemplateButton { label, position }, 1, true);
            }
            Annotation::EndTemplateButton => {
                self.close_html_kind(|k| matches!(k, FrameKind::TemplateButton { .. }));
            }
            Annotation::TableOfContents => self.push_block(plain("tableOfContents", Vec::new())),
            Annotation::Breadcrumb => self.push_block(plain("breadcrumb", Vec::new())),
            Annotation::PageList(mut map) => {
                map.insert(placeholder::NAMES.into(), Value::Bool(true));
                self.push_block(element("pageList", map, Vec::new()));
            }
        }
    }

    /// A whiteboard block for the `.excalidraw` file an annotation names (an empty one when the
    /// file is not in the source).
    fn whiteboard_of(&mut self, file: Option<&str>) -> Value {
        if let Some(file) = file
            && let crate::ctx::Dest::Whiteboard(path) = self.ctx.classify(file)
            && let Some(block) = self.ctx.whiteboard_block(&path)
        {
            return block;
        }
        if let Some(file) = file {
            self.ctx.warn(ImportWarningCode::MissingFile, Some(file));
        }
        placeholder::whiteboard(Map::new(), None)
    }

    fn next_column(&mut self) {
        self.end_gallery();
        if let Some(Frame {
            kind: FrameKind::Columns { columns, started },
            blocks,
            ..
        }) = self.frames.last_mut()
        {
            if *started {
                columns.push(std::mem::take(blocks));
            }
            *started = true;
        }
    }

    fn finish(mut self) -> Vec<Value> {
        self.flush_run();
        while self.frames.len() > 1 {
            self.flush_run();
            self.close_top();
        }
        self.end_gallery();
        let mut blocks = self
            .frames
            .first_mut()
            .map(|root| std::mem::take(&mut root.blocks))
            .unwrap_or_default();
        for (label, mut content) in std::mem::take(&mut self.footnotes) {
            let marker = format!("[{label}] ");
            match content.first_mut() {
                Some(first) if kind(first) == "paragraph" => prepend_text(first, &marker),
                _ => content.insert(0, json::text_paragraph(&marker)),
            }
            blocks.extend(content);
        }
        blocks
    }
}

/// Puts text at the start of a paragraph, joining the first run when it has no marks.
fn prepend_text(paragraph: &mut Value, text: &str) {
    if let Some(items) = json::content_mut(paragraph) {
        if let Some(first) = items.first_mut()
            && kind(first) == "text"
            && first.get("marks").is_none()
            && let Some(Value::String(existing)) = first.get_mut("text")
        {
            existing.insert_str(0, text);
        } else {
            items.insert(0, json::text_node(text, &MarkSet::default()));
        }
    } else {
        set_content(paragraph, vec![json::text_node(text, &MarkSet::default())]);
    }
}

fn set_content(node: &mut Value, content: Vec<Value>) {
    if let Value::Object(object) = node {
        object.insert("content".into(), Value::Array(content));
    }
}

/// The target of a wiki link: `Title`, `Folder/Title`, without a `#heading` or `^block`.
fn wiki_target(dest: &str) -> String {
    dest.split(['#', '^'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

// ----- building nodes --------------------------------------------------------------------------

fn ensure_blocks(mut blocks: Vec<Value>) -> Vec<Value> {
    if blocks.is_empty() {
        blocks.push(json::paragraph(Vec::new()));
    }
    blocks
}

fn cell_node(header: bool, blocks: Vec<Value>) -> Value {
    element(
        if header { "tableHeader" } else { "tableCell" },
        attrs([("colspan", Value::from(1)), ("rowspan", Value::from(1))]),
        ensure_blocks(blocks),
    )
}

fn callout_node(icon: &str, color: Option<&str>, blocks: Vec<Value>) -> Value {
    let mut map = attrs([("icon", Value::String(icon.to_owned()))]);
    if let Some(color) = color {
        map.insert("color".into(), Value::String(color.to_owned()));
    }
    element("callout", map, ensure_blocks(blocks))
}

fn alert_icon(alert: BlockQuoteKind) -> &'static str {
    match alert {
        BlockQuoteKind::Note => "ℹ️",
        BlockQuoteKind::Tip => "💡",
        BlockQuoteKind::Important => "❗",
        BlockQuoteKind::Warning => "⚠️",
        BlockQuoteKind::Caution => "🛑",
    }
}

fn quote_node(
    alert: Option<BlockQuoteKind>,
    annotated: Option<CalloutNote>,
    mut blocks: Vec<Value>,
) -> Value {
    if let Some(alert) = alert {
        return callout_node(alert_icon(alert), None, blocks);
    }
    if let Some(note) = annotated {
        let known = note.icon.as_deref();
        let stripped = strip_icon(&mut blocks, known).or_else(|| strip_leading_icon(&mut blocks));
        let icon = note
            .icon
            .or(stripped)
            .unwrap_or_else(|| DEFAULT_ICON.to_owned());
        return callout_node(&icon, note.color.as_deref(), blocks);
    }
    match strip_leading_icon(&mut blocks) {
        Some(icon) => callout_node(&icon, None, blocks),
        None => plain("blockquote", ensure_blocks(blocks)),
    }
}

fn details_node(open: bool, summary: &str, level: Option<u8>, blocks: Vec<Value>) -> Value {
    let summary_node = json::summary(summary, level);
    let mut map = Map::new();
    if open {
        map.insert("open".into(), Value::Bool(true));
    }
    element(
        "details",
        map,
        vec![summary_node, plain("detailsContent", ensure_blocks(blocks))],
    )
}

fn columns_node(mut columns: Vec<Vec<Value>>) -> Value {
    while columns.len() > 5 {
        if let Some(extra) = columns.pop()
            && let Some(last) = columns.last_mut()
        {
            last.extend(extra);
        }
    }
    plain(
        "columnList",
        columns
            .into_iter()
            .map(|blocks| plain("column", ensure_blocks(blocks)))
            .collect(),
    )
}

/// The first block of a list item is a paragraph (the schema's rule).
fn item_blocks(mut blocks: Vec<Value>) -> Vec<Value> {
    if blocks
        .first()
        .is_none_or(|first| kind(first) != "paragraph")
    {
        blocks.insert(0, json::paragraph(Vec::new()));
    }
    blocks
}

fn list_node(start: Option<u64>, items: Vec<ItemData>) -> Value {
    let all_tasks = items.iter().all(|item| item.task.is_some());
    let nodes: Vec<Value> = items
        .into_iter()
        .map(|item| {
            let mut blocks = item_blocks(item.blocks);
            if all_tasks {
                return element(
                    "taskItem",
                    attrs([("checked", Value::Bool(item.task.unwrap_or(false)))]),
                    blocks,
                );
            }
            if let Some(done) = item.task {
                // In a list that is not all tasks the marker stays text.
                if let Some(first) = blocks.first_mut() {
                    prepend_text(first, if done { "[x] " } else { "[ ] " });
                }
            }
            plain("listItem", blocks)
        })
        .collect();
    if all_tasks {
        return plain("taskList", nodes);
    }
    match start {
        Some(start) => {
            let mut map = Map::new();
            if start != 1 {
                map.insert(
                    "start".into(),
                    Value::from(i64::try_from(start).unwrap_or(i64::MAX)),
                );
            }
            element("orderedList", map, nodes)
        }
        None => plain("bulletList", nodes),
    }
}

/// An image block as a gallery item: its attachment (a needed file or an attachment id) and its
/// caption.
fn gallery_item(image: &Value) -> Option<Value> {
    let mut item = Map::new();
    if let Some(path) = json::str_attr(image, placeholder::FILE) {
        item.insert(placeholder::FILE.into(), Value::String(path.to_owned()));
    } else {
        let id = json::str_attr(image, placeholder::ATTACHMENT)?;
        item.insert(placeholder::ATTACHMENT.into(), Value::String(id.to_owned()));
    }
    if let Some(caption) = json::str_attr(image, "caption").filter(|c| !c.is_empty()) {
        item.insert("caption".into(), Value::String(caption.to_owned()));
    }
    Some(Value::Object(item))
}

/// Merges what an annotation says into the image or file block it precedes.
fn apply_media_attrs(block: &mut Value, attrs: &Map<String, Value>) {
    let keys: &[&str] = match kind(block) {
        "image" => &["widthPercent", "width", "align"],
        "file" => &["view"],
        _ => return,
    };
    for key in keys {
        if let Some(value) = attrs.get(*key) {
            set_attr(block, key, value.clone());
        }
    }
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
