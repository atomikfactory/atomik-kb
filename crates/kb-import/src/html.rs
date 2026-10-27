//! HTML into blocks (ADR-0036, section 3.5): whole documents (Notion's and atomik-kb's HTML
//! exports, any other page) and the HTML blocks found inside Markdown.
//!
//! Nothing is run or fetched: scripts, frames, forms and the like are dropped with a warning, and
//! every link and image follows the rules of Markdown links (`PageCtx::link_action`).

use html5ever::driver::{self, ParseOpts};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{QualName, local_name, ns};
use kb_types::{ImportWarningCode, OptionColor};
use scraper::{ElementRef, Html, HtmlTreeSink, Node};
use serde_json::{Map, Value};

use crate::ctx::{LinkAction, PageCtx};
use crate::emoji;
use crate::inline::{InlineRun, Role};
use crate::json::{self, MarkSet, attrs, element, kind, plain, plain_text, set_attr};
use crate::placeholder;
use crate::values;

/// The deepest DOM nesting walked with structure; deeper content is read as text.
const MAX_RECURSION: usize = 120;
/// How much JSON nesting open containers may use (the model's limit is 128).
pub(crate) const MAX_JSON_DEPTH: usize = 60;
/// Most rows and columns kept of one table.
pub(crate) const MAX_TABLE_ROWS: usize = 10_000;
pub(crate) const MAX_TABLE_COLS: usize = 100;

/// The deepest nesting of elements the parser may build. HTML parsers take time quadratic in the
/// nesting depth (every start tag looks down the stack of open elements), so a hostile file of
/// nested elements would run for hours; past this depth the rest of the input is read as text.
const MAX_PARSE_DEPTH: usize = 256;
/// How much input is fed to the parser at a time.
const PARSE_CHUNK: usize = 8 * 1024;

/// Parses HTML in pieces, stopping when the tree gets too deep. Answers the tree and the part of
/// the input that was not parsed.
fn parse_bounded(source: &str, fragment: bool) -> (Html, &str) {
    let sink = HtmlTreeSink::new(if fragment {
        Html::new_fragment()
    } else {
        Html::new_document()
    });
    let mut parser = if fragment {
        driver::parse_fragment(
            sink,
            ParseOpts::default(),
            QualName::new(None, ns!(html), local_name!("body")),
            Vec::new(),
            false,
        )
    } else {
        driver::parse_document(sink, ParseOpts::default())
    };
    let mut at = 0;
    while at < source.len() {
        let mut end = (at + PARSE_CHUNK).min(source.len());
        while !source.is_char_boundary(end) {
            end += 1;
        }
        parser.process(StrTendril::from_slice(&source[at..end]));
        at = end;
        let too_deep = parser
            .tokenizer
            .sink
            .sink
            .0
            .borrow()
            .tree
            .nodes()
            .next_back()
            .is_some_and(|last| last.ancestors().count() > MAX_PARSE_DEPTH);
        if too_deep {
            break;
        }
    }
    (parser.finish(), &source[at..])
}

/// The text of HTML that was not parsed: tags dropped, character references decoded, white space
/// collapsed, blocks of text apart by their blank lines.
fn text_of_unparsed(rest: &str) -> Vec<Value> {
    let mut text = String::with_capacity(rest.len() / 2);
    let mut in_tag = false;
    for c in rest.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push('\n');
            }
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            _ => text.push(c),
        }
    }
    let decoded = crate::htmltag::decode_entities(&text);
    decoded
        .split("\n\n")
        .map(|part| collapse_ws(part).trim().to_owned())
        .filter(|part| !part.is_empty())
        .map(|part| json::text_paragraph(&part))
        .collect()
}

/// What an HTML document gives a page.
#[derive(Debug, Default)]
pub(crate) struct HtmlPage {
    /// The blocks of the content.
    pub blocks: Vec<Value>,
    /// The title.
    pub title: Option<String>,
    /// The icon.
    pub icon: Option<String>,
}

/// Reads a whole HTML document: the content is the `<article>`, else the `<body>`.
pub(crate) fn convert_document(ctx: &mut PageCtx<'_>, source: &str) -> HtmlPage {
    let (html, rest) = parse_bounded(source, false);
    let Some(root) = html.tree.root().children().find_map(ElementRef::wrap) else {
        return HtmlPage::default();
    };
    let find = |name: &str| {
        root.descendent_elements()
            .find(|e| e.value().name() == name)
    };
    let content = find("article").or_else(|| find("body")).unwrap_or(root);

    let mut walker = Walker {
        ctx,
        skip: Vec::new(),
        used: 0,
    };
    let mut page = HtmlPage::default();

    // The title: `h1.page-title`, else `<title>`, else a first `<h1>` (below).
    if let Some(h1) = content
        .descendent_elements()
        .find(|e| e.value().name() == "h1" && has_class(*e, "page-title"))
    {
        page.title = non_empty(&collapse_ws(&text_of(h1)));
        walker.skip.push(h1);
    }
    if page.title.is_none() {
        page.title = find("title").and_then(|t| non_empty(&collapse_ws(&text_of(t))));
    }
    // The icon of a Notion page.
    if let Some(icon) = content
        .descendent_elements()
        .find(|e| has_class(*e, "page-header-icon"))
    {
        let text = collapse_ws(&text_of(icon));
        if emoji::is_icon_word(&text) {
            page.icon = Some(text);
        }
        walker.skip.push(icon);
    }

    page.blocks = walker.block_children(content, 0);
    if !rest.is_empty() {
        walker.drop_warning("html nesting");
        page.blocks.extend(text_of_unparsed(rest));
    }
    if page.title.is_none()
        && let Some(first) = page.blocks.first()
        && kind(first) == "heading"
        && json::attr(first, "level").and_then(Value::as_i64) == Some(1)
    {
        let text = plain_text(first);
        page.title = non_empty(text.trim());
        page.blocks.remove(0);
    }
    page
}

/// Reads a piece of HTML (a block of a Markdown file).
pub(crate) fn convert_fragment(ctx: &mut PageCtx<'_>, source: &str) -> Vec<Value> {
    let (html, rest) = parse_bounded(source, true);
    let Some(root) = html.tree.root().children().find_map(ElementRef::wrap) else {
        return Vec::new();
    };
    let mut walker = Walker {
        ctx,
        skip: Vec::new(),
        used: 0,
    };
    let mut blocks = walker.block_children(root, 0);
    if !rest.is_empty() {
        walker.drop_warning("html nesting");
        blocks.extend(text_of_unparsed(rest));
    }
    blocks
}

struct Walker<'c, 'a, 'd> {
    ctx: &'c mut PageCtx<'a>,
    /// Elements that were read for the page's title or icon.
    skip: Vec<ElementRef<'d>>,
    /// JSON nesting used by the containers being built.
    used: usize,
}

fn has_class(element: ElementRef<'_>, class: &str) -> bool {
    element.value().classes().any(|c| c == class)
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The first descendant at most `depth` levels down for which `matches` holds (breadth first
/// at each level, so deep trees cost little).
fn find_within<'d>(
    el: ElementRef<'d>,
    depth: usize,
    matches: &dyn Fn(ElementRef<'d>) -> bool,
) -> Option<ElementRef<'d>> {
    if depth == 0 {
        return None;
    }
    if let Some(found) = el.child_elements().find(|child| matches(*child)) {
        return Some(found);
    }
    el.child_elements()
        .find_map(|child| find_within(child, depth - 1, matches))
}

/// The text of an element and everything in it.
fn text_of(element: ElementRef<'_>) -> String {
    element.text().collect()
}

/// White space runs of HTML text as single spaces.
fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

fn ensure_blocks(mut blocks: Vec<Value>) -> Vec<Value> {
    if blocks.is_empty() {
        blocks.push(json::paragraph(Vec::new()));
    }
    blocks
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

impl<'d> Walker<'_, '_, 'd> {
    /// The blocks of the content of `parent`.
    fn block_children(&mut self, parent: ElementRef<'d>, depth: usize) -> Vec<Value> {
        let mut run = InlineRun::default();
        let mut out = Vec::new();
        self.walk_children(
            parent,
            &MarkSet::default(),
            Role::Normal,
            &mut run,
            &mut out,
            depth,
        );
        out.extend(run.into_blocks());
        out
    }

    fn walk_children(
        &mut self,
        parent: ElementRef<'d>,
        marks: &MarkSet,
        role: Role,
        run: &mut InlineRun,
        out: &mut Vec<Value>,
        depth: usize,
    ) {
        for child in parent.children() {
            match child.value() {
                Node::Text(text) => {
                    let mut collapsed = collapse_ws(text);
                    // A space after a space (or at the start of a line) is dropped, across
                    // elements too.
                    if collapsed.starts_with(' ') && run.ends_with_space() {
                        collapsed.remove(0);
                    }
                    run.push_text(&collapsed, marks, role);
                }
                Node::Element(_) => {
                    if let Some(el) = ElementRef::wrap(child) {
                        self.element(el, marks, role, run, out, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    /// Ends the paragraph being read: its blocks go to `out`.
    fn flush(run: &mut InlineRun, out: &mut Vec<Value>) {
        out.extend(std::mem::take(run).into_blocks());
    }

    fn drop_warning(&mut self, name: &str) {
        self.ctx
            .warn(ImportWarningCode::UnsupportedContent, Some(name));
    }

    /// Whether a container of this JSON weight fits.
    fn fits(&self, weight: usize) -> bool {
        self.used + weight <= MAX_JSON_DEPTH
    }

    fn element(
        &mut self,
        el: ElementRef<'d>,
        marks: &MarkSet,
        role: Role,
        run: &mut InlineRun,
        out: &mut Vec<Value>,
        depth: usize,
    ) {
        if self.skip.contains(&el) {
            return;
        }
        if depth > MAX_RECURSION {
            run.push_text(&collapse_ws(&text_of(el)), marks, role);
            return;
        }
        let name = el.value().name();
        if name == "aside" || has_class(el, "callout") {
            Self::flush(run, out);
            self.callout(el, out, depth);
            return;
        }
        match name {
            // Never read.
            "script" | "style" | "template" | "canvas" | "svg" | "object" | "embed" | "iframe"
            | "select" | "input" => self.drop_warning(name),
            "head" | "title" | "meta" | "link" | "base" | "wbr" | "colgroup" | "col" => {}
            // Dropped, their text kept.
            "form" | "button" | "textarea" | "noscript" => {
                self.drop_warning(name);
                self.walk_children(el, marks, role, run, out, depth);
            }
            "br" => run.push_hard_break(),
            "strong" | "b" => {
                let marks = MarkSet {
                    bold: true,
                    ..marks.clone()
                };
                self.walk_children(el, &marks, role, run, out, depth);
            }
            "em" | "i" | "cite" | "dfn" => {
                let marks = MarkSet {
                    italic: true,
                    ..marks.clone()
                };
                self.walk_children(el, &marks, role, run, out, depth);
            }
            "u" | "ins" => {
                let marks = MarkSet {
                    underline: true,
                    ..marks.clone()
                };
                self.walk_children(el, &marks, role, run, out, depth);
            }
            "s" | "del" | "strike" => {
                let marks = MarkSet {
                    strike: true,
                    ..marks.clone()
                };
                self.walk_children(el, &marks, role, run, out, depth);
            }
            "mark" => {
                let color = el
                    .attr("data-color")
                    .filter(|c| OptionColor::parse(c).is_some())
                    .unwrap_or("yellow");
                let marks = MarkSet {
                    highlight: Some(color.to_owned()),
                    ..marks.clone()
                };
                self.walk_children(el, &marks, role, run, out, depth);
            }
            "code" | "kbd" | "samp" => {
                let marks = MarkSet {
                    code: true,
                    ..MarkSet::default()
                };
                run.push_text(&collapse_ws(&text_of(el)), &marks, role);
            }
            "time" => match date_mention(el) {
                Some(node) => run.push_node(node),
                None => self.walk_children(el, marks, role, run, out, depth),
            },
            "math" => self.math(el, marks, role, run, out, depth),
            "a" => self.anchor(el, marks, role, run, out, depth),
            "img" => self.image(el, run),
            "p" => {
                Self::flush(run, out);
                let mut inner = InlineRun::default();
                self.walk_children(
                    el,
                    &MarkSet::default(),
                    Role::Normal,
                    &mut inner,
                    out,
                    depth,
                );
                out.extend(inner.into_blocks());
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                Self::flush(run, out);
                let level = name.as_bytes().get(1).map_or(1, |b| b - b'0');
                let mut inner = InlineRun::default();
                let mut nested = Vec::new();
                self.walk_children(
                    el,
                    &MarkSet::default(),
                    Role::Normal,
                    &mut inner,
                    &mut nested,
                    depth,
                );
                let (content, blocks) = inner.into_inline();
                // A heading that is only a picture (or only blocks) is just those.
                if !(content.is_empty() && !(blocks.is_empty() && nested.is_empty())) {
                    out.push(json::heading(level.clamp(1, 3), content));
                }
                out.extend(nested);
                out.extend(blocks);
            }
            "ul" | "ol" => {
                Self::flush(run, out);
                self.list(el, out, depth);
            }
            "blockquote" => {
                Self::flush(run, out);
                self.container(out, el, 1, depth, |blocks| {
                    plain("blockquote", ensure_blocks(blocks))
                });
            }
            "pre" => {
                Self::flush(run, out);
                out.push(code_block(el));
            }
            "hr" => {
                Self::flush(run, out);
                out.push(plain("horizontalRule", Vec::new()));
            }
            "table" => {
                Self::flush(run, out);
                self.table(el, out, depth);
            }
            "details" => {
                Self::flush(run, out);
                self.details(el, out, depth);
            }
            "figure" => {
                Self::flush(run, out);
                self.figure(el, out, depth);
            }
            "video" | "audio" => {
                Self::flush(run, out);
                self.media(el, out);
            }
            "nav" if has_class(el, "toc") => {
                Self::flush(run, out);
                out.push(plain("tableOfContents", Vec::new()));
            }
            _ => self.generic(el, marks, role, run, out, depth),
        }
    }

    /// Elements with no rule of their own: callouts, columns and equations by class, containers
    /// and inline elements by position.
    fn generic(
        &mut self,
        el: ElementRef<'d>,
        marks: &MarkSet,
        role: Role,
        run: &mut InlineRun,
        out: &mut Vec<Value>,
        depth: usize,
    ) {
        let name = el.value().name();
        if has_class(el, "column-list") || has_class(el, "columns") {
            Self::flush(run, out);
            self.columns(el, out, depth);
            return;
        }
        if has_class(el, "math-display") {
            Self::flush(run, out);
            let latex = strip_delimiters(&text_of(el), "\\[", "\\]");
            out.push(math_block(&latex));
            return;
        }
        if has_class(el, "math-inline") {
            let latex = strip_delimiters(&text_of(el), "\\(", "\\)");
            run.push_node(math_inline(&latex));
            return;
        }
        if (has_class(el, "katex") || has_class(el, "katex-display"))
            && let Some(latex) = tex_annotation(el)
        {
            if has_class(el, "katex-display") {
                run.push_block(math_block(&latex));
            } else {
                run.push_node(math_inline(&latex));
            }
            return;
        }
        let is_block = matches!(
            name,
            "div"
                | "section"
                | "main"
                | "header"
                | "footer"
                | "article"
                | "body"
                | "html"
                | "figcaption"
                | "address"
                | "fieldset"
                | "dl"
                | "dt"
                | "dd"
                | "center"
                | "summary"
                | "caption"
                | "hgroup"
        );
        if is_block {
            Self::flush(run, out);
            let mut inner = InlineRun::default();
            self.walk_children(
                el,
                &MarkSet::default(),
                Role::Normal,
                &mut inner,
                out,
                depth,
            );
            out.extend(inner.into_blocks());
        } else {
            // An inline element: its text keeps the marks around it.
            self.walk_children(el, marks, role, run, out, depth);
        }
    }

    /// Builds a container block from the content of `el`, or flattens it into `out` when the
    /// JSON nesting is used up.
    fn container(
        &mut self,
        out: &mut Vec<Value>,
        el: ElementRef<'d>,
        weight: usize,
        depth: usize,
        build: impl FnOnce(Vec<Value>) -> Value,
    ) {
        if self.fits(weight) {
            self.used += weight;
            let blocks = self.block_children(el, depth);
            self.used -= weight;
            out.push(build(blocks));
        } else {
            let blocks = self.block_children(el, depth);
            out.extend(blocks);
        }
    }

    fn math(
        &mut self,
        el: ElementRef<'d>,
        marks: &MarkSet,
        role: Role,
        run: &mut InlineRun,
        out: &mut Vec<Value>,
        depth: usize,
    ) {
        match tex_annotation(el) {
            Some(latex) if el.attr("display") == Some("block") => {
                run.push_block(math_block(&latex));
            }
            Some(latex) => run.push_node(math_inline(&latex)),
            None => self.walk_children(el, marks, role, run, out, depth),
        }
    }

    fn anchor(
        &mut self,
        el: ElementRef<'d>,
        marks: &MarkSet,
        role: Role,
        run: &mut InlineRun,
        out: &mut Vec<Value>,
        depth: usize,
    ) {
        let Some(href) = el.attr("href").filter(|h| !h.trim().is_empty()) else {
            self.walk_children(el, marks, role, run, out, depth);
            return;
        };
        let has_image = el.descendent_elements().any(|d| d.value().name() == "img");
        match self.ctx.link_action(href) {
            LinkAction::Mark(url) => {
                let marks = MarkSet {
                    link: Some(url),
                    ..marks.clone()
                };
                self.walk_children(el, &marks, role, run, out, depth);
            }
            LinkAction::Plain => self.walk_children(el, marks, role, run, out, depth),
            LinkAction::Inline(_) | LinkAction::After(_) if has_image => {
                self.walk_children(el, marks, role, run, out, depth);
            }
            LinkAction::Inline(mut node) => {
                let label = collapse_ws(&text_of(el));
                let label = label.trim();
                if !label.is_empty() {
                    set_attr(
                        &mut node,
                        placeholder::LABEL,
                        Value::String(label.to_owned()),
                    );
                }
                run.push_node(node);
            }
            LinkAction::After(mut block) => {
                let label = collapse_ws(&text_of(el));
                PageCtx::name_after_block(&mut block, &label);
                self.walk_children(el, marks, Role::FileLinkText, run, out, depth);
                run.push_after(block);
            }
        }
    }

    fn image(&mut self, el: ElementRef<'d>, run: &mut InlineRun) {
        let src = el.attr("src").unwrap_or("");
        let alt = el.attr("alt").unwrap_or("");
        let title = el.attr("title").unwrap_or("");
        push_image(self.ctx, run, src, alt, title);
    }

    /// A list (nothing when it has no items; its items as plain blocks when the JSON nesting is
    /// used up).
    fn list(&mut self, el: ElementRef<'d>, out: &mut Vec<Value>, depth: usize) {
        let ordered = el.value().name() == "ol";
        let items: Vec<ElementRef<'d>> = el
            .child_elements()
            .filter(|child| child.value().name() == "li")
            .collect();
        if items.is_empty() {
            return;
        }
        if !self.fits(2) {
            for item in items {
                out.extend(self.block_children(item, depth));
            }
            return;
        }
        self.used += 2;
        let tasks: Vec<Option<bool>> = items.iter().map(|item| task_state(*item)).collect();
        let all_tasks = tasks.iter().all(Option::is_some);
        let mut nodes = Vec::new();
        for (item, task) in items.iter().zip(&tasks) {
            let blocks = item_blocks(self.block_children(*item, depth));
            if all_tasks {
                nodes.push(element(
                    "taskItem",
                    attrs([("checked", Value::Bool(task.unwrap_or(false)))]),
                    blocks,
                ));
            } else {
                nodes.push(plain("listItem", blocks));
            }
        }
        self.used -= 2;
        out.push(if all_tasks {
            plain("taskList", nodes)
        } else if ordered {
            let start = el
                .attr("start")
                .and_then(|s| s.trim().parse::<i64>().ok())
                .filter(|start| *start != 1);
            let mut map = Map::new();
            if let Some(start) = start {
                map.insert("start".into(), Value::from(start));
            }
            element("orderedList", map, nodes)
        } else {
            plain("bulletList", nodes)
        });
    }

    fn table(&mut self, el: ElementRef<'d>, out: &mut Vec<Value>, depth: usize) {
        let mut row_elements: Vec<ElementRef<'d>> = Vec::new();
        let mut caption = None;
        for child in el.child_elements() {
            match child.value().name() {
                "tr" => row_elements.push(child),
                "thead" | "tbody" | "tfoot" => {
                    row_elements
                        .extend(child.child_elements().filter(|c| c.value().name() == "tr"));
                }
                "caption" => caption = non_empty(&collapse_ws(&text_of(child))),
                _ => {}
            }
        }
        if let Some(caption) = caption {
            out.push(json::text_paragraph(&caption));
        }
        if row_elements.is_empty() {
            return;
        }
        if row_elements.len() > MAX_TABLE_ROWS {
            row_elements.truncate(MAX_TABLE_ROWS);
            self.drop_warning("table rows");
        }
        let flatten = !self.fits(3);
        if !flatten {
            self.used += 3;
        }
        let mut rows: Vec<Vec<Value>> = Vec::new();
        for row in row_elements {
            let mut cells = Vec::new();
            for cell in row
                .child_elements()
                .filter(|c| matches!(c.value().name(), "td" | "th"))
                .take(MAX_TABLE_COLS)
            {
                let blocks = self.block_children(cell, depth);
                if flatten {
                    out.extend(blocks);
                } else {
                    let header = cell.value().name() == "th";
                    cells.push(element(
                        if header { "tableHeader" } else { "tableCell" },
                        attrs([("colspan", Value::from(1)), ("rowspan", Value::from(1))]),
                        ensure_blocks(blocks),
                    ));
                }
            }
            if !flatten {
                rows.push(cells);
            }
        }
        if flatten {
            return;
        }
        self.used -= 3;
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        if width == 0 {
            return;
        }
        let rows: Vec<Value> = rows
            .into_iter()
            .filter(|row| !row.is_empty() || width > 0)
            .map(|mut cells| {
                while cells.len() < width {
                    cells.push(element(
                        "tableCell",
                        attrs([("colspan", Value::from(1)), ("rowspan", Value::from(1))]),
                        vec![json::paragraph(Vec::new())],
                    ));
                }
                plain("tableRow", cells)
            })
            .collect();
        out.push(plain("table", rows));
    }

    fn details(&mut self, el: ElementRef<'d>, out: &mut Vec<Value>, depth: usize) {
        let summary = el
            .child_elements()
            .find(|child| child.value().name() == "summary");
        let summary_text = summary
            .map(|s| collapse_ws(&text_of(s)).trim().to_owned())
            .unwrap_or_default();
        // A heading element inside the summary makes it a toggle heading (ADR-0040).
        let summary_level = summary.and_then(|s| {
            s.child_elements()
                .find_map(|child| match child.value().name().as_bytes() {
                    [b'h', digit @ b'1'..=b'6'] => Some((digit - b'0').clamp(1, 3)),
                    _ => None,
                })
        });
        let open = el.attr("open").is_some();
        if !self.fits(2) {
            let mut blocks = Vec::new();
            if !summary_text.is_empty() {
                blocks.push(json::text_paragraph(&summary_text));
            }
            blocks.extend(self.block_children(el, depth));
            out.extend(blocks);
            return;
        }
        self.used += 2;
        // The content is everything but the summary.
        if let Some(summary) = summary {
            self.skip.push(summary);
        }
        let blocks = self.block_children(el, depth);
        if summary.is_some() {
            self.skip.pop();
        }
        self.used -= 2;
        let summary_node = json::summary(&summary_text, summary_level);
        let mut map = Map::new();
        if open {
            map.insert("open".into(), Value::Bool(true));
        }
        out.push(element(
            "details",
            map,
            vec![summary_node, plain("detailsContent", ensure_blocks(blocks))],
        ));
    }

    /// A `<figure>`: a picture with its caption, or a whiteboard.
    fn figure(&mut self, el: ElementRef<'d>, out: &mut Vec<Value>, depth: usize) {
        let caption = el
            .child_elements()
            .find(|child| child.value().name() == "figcaption");
        let caption_text = caption
            .map(|c| collapse_ws(&text_of(c)).trim().to_owned())
            .unwrap_or_default();
        if has_class(el, "whiteboard") {
            let link =
                find_within(el, 4, &|d| d.value().name() == "a").and_then(|a| a.attr("href"));
            if let Some(href) = link
                && let crate::ctx::Dest::Whiteboard(path) = self.ctx.classify(href)
                && let Some(block) = self.ctx.whiteboard_block(&path)
            {
                out.push(block);
                return;
            }
        }
        let picture = find_within(el, 4, &|d| d.value().name() == "img");
        if let Some(picture) = picture {
            let mut run = InlineRun::default();
            let src = picture.attr("src").unwrap_or("");
            let alt = picture.attr("alt").unwrap_or("");
            let title = if caption_text.is_empty() {
                picture.attr("title").unwrap_or("")
            } else {
                caption_text.as_str()
            };
            push_image(self.ctx, &mut run, src, alt, title);
            let mut blocks = run.into_blocks();
            if let Some(image) = blocks.iter_mut().find(|b| kind(b) == "image") {
                layout_of_figure(el, image);
            }
            out.extend(blocks);
            return;
        }
        // Not a picture: a container.
        let mut run = InlineRun::default();
        self.walk_children(el, &MarkSet::default(), Role::Normal, &mut run, out, depth);
        out.extend(run.into_blocks());
    }

    /// `<video>` and `<audio>` with a source in the archive: a file block shown as a player.
    fn media(&mut self, el: ElementRef<'d>, out: &mut Vec<Value>) {
        let src = el.attr("src").map(str::to_owned).or_else(|| {
            find_within(el, 2, &|d| d.value().name() == "source")
                .and_then(|d| d.attr("src"))
                .map(str::to_owned)
        });
        let Some(src) = src else {
            return;
        };
        match self.ctx.classify(&src) {
            crate::ctx::Dest::Binary { orig, .. } => {
                self.ctx.need(&orig);
                let mut block = placeholder::file_path(&orig, "");
                set_attr(&mut block, "view", Value::String("preview".into()));
                out.push(block);
            }
            _ => self
                .ctx
                .warn(ImportWarningCode::MissingFile, Some(src.as_str())),
        }
    }

    fn callout(&mut self, el: ElementRef<'d>, out: &mut Vec<Value>, depth: usize) {
        // The icon: `.callout-icon`, or Notion's `.icon`, when it is an emoji.
        let found = find_within(el, 3, &|d| {
            has_class(d, "callout-icon") || has_class(d, "icon")
        });
        let mut icon = None;
        let mut icon_element = None;
        if let Some(icon_el) = found {
            let text = collapse_ws(&text_of(icon_el));
            let text = text.trim();
            if emoji::is_icon_word(text) {
                icon = Some(text.to_owned());
                icon_element = Some(icon_el);
                self.skip.push(icon_el);
            }
        }
        let color = el
            .attr("data-color")
            .filter(|c| OptionColor::parse(c).is_some())
            .map(str::to_owned);
        let weight = 1;
        let fits = self.fits(weight);
        if fits {
            self.used += weight;
        }
        let mut blocks = self.block_children(el, depth);
        if icon_element.is_some() {
            self.skip.pop();
        }
        if fits {
            self.used -= weight;
        }
        if icon.is_none() {
            icon = crate::markdown::strip_leading_icon(&mut blocks);
        }
        if !fits {
            out.extend(blocks);
            return;
        }
        let mut map = Map::new();
        map.insert(
            "icon".into(),
            Value::String(icon.unwrap_or_else(|| "💡".to_owned())),
        );
        if let Some(color) = color {
            map.insert("color".into(), Value::String(color));
        }
        out.push(element("callout", map, ensure_blocks(blocks)));
    }

    fn columns(&mut self, el: ElementRef<'d>, out: &mut Vec<Value>, depth: usize) {
        let column_elements: Vec<ElementRef<'d>> = el
            .child_elements()
            .filter(|child| has_class(*child, "column"))
            .collect();
        if column_elements.len() < 2 || !self.fits(2) {
            for column in column_elements {
                out.extend(self.block_children(column, depth));
            }
            return;
        }
        self.used += 2;
        let mut columns: Vec<Vec<Value>> = Vec::new();
        for column in column_elements {
            let blocks = ensure_blocks(self.block_children(column, depth));
            if columns.len() < 5 {
                columns.push(blocks);
            } else if let Some(last) = columns.last_mut() {
                last.extend(blocks);
            }
        }
        self.used -= 2;
        out.push(plain(
            "columnList",
            columns.into_iter().map(|c| plain("column", c)).collect(),
        ));
    }
}

/// Adds an image to a paragraph being read: a block, or its text.
pub(crate) fn push_image(
    ctx: &mut PageCtx<'_>,
    run: &mut InlineRun,
    src: &str,
    alt: &str,
    title: &str,
) {
    match ctx.image_action(src, alt, title) {
        crate::ctx::ImageAction::Block(block) => run.push_block(block),
        crate::ctx::ImageAction::Text { text, href } => {
            if text.is_empty() {
                return;
            }
            let marks = MarkSet {
                link: href,
                ..MarkSet::default()
            };
            run.push_text(&text, &marks, Role::Normal);
        }
    }
}

/// `width: 50%` and `align-right` of an exported figure.
fn layout_of_figure(figure: ElementRef<'_>, image: &mut Value) {
    if let Some(style) = figure.attr("style") {
        for declaration in style.split(';') {
            if let Some((property, value)) = declaration.split_once(':')
                && property.trim().eq_ignore_ascii_case("width")
                && let Some(percent) = value.trim().strip_suffix('%')
                && let Ok(percent) = percent.trim().parse::<f64>()
                && percent.fract() == 0.0
                && (10.0..=100.0).contains(&percent)
            {
                // The checks above make the conversion exact.
                set_attr(image, "widthPercent", Value::from(percent as i64));
            }
        }
    }
    for align in ["left", "center", "right"] {
        if has_class(figure, &format!("align-{align}")) {
            set_attr(image, "align", Value::String(align.to_owned()));
        }
    }
}

/// `<time datetime="2026-10-07T14:30" data-reminder="15">` as a date mention.
fn date_mention(el: ElementRef<'_>) -> Option<Value> {
    let (date, time) = values::parse_date(el.attr("datetime")?)?;
    let mut map = attrs([("date", Value::String(date))]);
    if let Some(time) = time {
        map.insert("time".into(), Value::String(time));
    }
    if let Some(reminder) = el
        .attr("data-reminder")
        .and_then(|r| r.trim().parse::<u32>().ok())
    {
        map.insert("reminder".into(), Value::from(reminder));
    }
    // Filled in when a page is made from a template (ADR-0056).
    if let Some(relative) = el
        .attr("data-relative")
        .map(str::trim)
        .filter(|relative| matches!(*relative, "today" | "now"))
    {
        map.insert("relative".into(), Value::String(relative.to_owned()));
    }
    Some(element("dateMention", map, Vec::new()))
}

/// `pre > code` (or `pre`) as a code block; a `math` language is an equation.
fn code_block(el: ElementRef<'_>) -> Value {
    let code = el
        .child_elements()
        .find(|child| child.value().name() == "code");
    let source = code.unwrap_or(el);
    let language = [Some(source), Some(el)]
        .into_iter()
        .flatten()
        .find_map(|e| {
            e.value()
                .classes()
                .find_map(|class| class.strip_prefix("language-"))
                .map(str::to_owned)
        });
    let mut text = preformatted_text(source);
    if text.ends_with('\n') {
        text.pop();
    }
    if language.as_deref() == Some("math") {
        return math_block(&text);
    }
    let mut map = Map::new();
    if let Some(language) = language.filter(|l| !l.is_empty()) {
        map.insert("language".into(), Value::String(language));
    }
    element(
        "codeBlock",
        map,
        if text.is_empty() {
            Vec::new()
        } else {
            vec![json::text_node(&text, &MarkSet::default())]
        },
    )
}

/// The text of a preformatted element: a `<br>` is a line break.
fn preformatted_text(el: ElementRef<'_>) -> String {
    let mut out = String::new();
    for node in el.descendants() {
        match node.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(element) if element.name() == "br" => out.push('\n'),
            _ => {}
        }
    }
    out
}

/// The LaTeX of an equation element: its `annotation encoding="application/x-tex"`.
fn tex_annotation(el: ElementRef<'_>) -> Option<String> {
    el.descendent_elements()
        .find(|d| {
            d.value().name() == "annotation"
                && d.attr("encoding")
                    .is_some_and(|e| e.eq_ignore_ascii_case("application/x-tex"))
        })
        .map(|annotation| text_of(annotation).trim().to_owned())
}

/// `text` without the delimiters around it (`\(x\)` is `x`).
fn strip_delimiters(text: &str, open: &str, close: &str) -> String {
    let text = text.trim();
    let text = text.strip_prefix(open).unwrap_or(text);
    let text = text.strip_suffix(close).unwrap_or(text);
    text.trim().to_owned()
}

pub(crate) fn math_block(latex: &str) -> Value {
    plain(
        "mathBlock",
        if latex.is_empty() {
            Vec::new()
        } else {
            vec![json::text_node(latex, &MarkSet::default())]
        },
    )
}

pub(crate) fn math_inline(latex: &str) -> Value {
    element(
        "mathInline",
        attrs([("latex", Value::String(latex.to_owned()))]),
        Vec::new(),
    )
}

/// Whether a list item is a task, and whether it is done: Notion's `.checkbox-on` and
/// `.checkbox-off`, a `<input type=checkbox>`, atomik-kb's `data-checked`.
fn task_state(item: ElementRef<'_>) -> Option<bool> {
    if let Some(checked) = item.attr("data-checked") {
        return Some(checked == "true");
    }
    for child in item.child_elements() {
        if has_class(child, "checkbox-on") {
            return Some(true);
        }
        if has_class(child, "checkbox-off") {
            return Some(false);
        }
        if is_checkbox(child) {
            return Some(child.attr("checked").is_some());
        }
        // A checkbox in the first paragraph of the item.
        if child.value().name() == "p"
            && let Some(input) = child.child_elements().find(|c| is_checkbox(*c))
        {
            return Some(input.attr("checked").is_some());
        }
    }
    None
}

fn is_checkbox(el: ElementRef<'_>) -> bool {
    el.value().name() == "input"
        && el
            .attr("type")
            .is_some_and(|t| t.eq_ignore_ascii_case("checkbox"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ctx::{SourceIndex, Warnings};

    fn index() -> SourceIndex {
        let mut index = SourceIndex::default();
        index.pages.insert("Other.md".into(), 5);
        index.pages_lower.insert("other.md".into(), vec![5]);
        index
            .binary
            .insert("pic.png".into(), "Export/pic.png".into());
        index
            .binary
            .insert("doc.pdf".into(), "Export/doc.pdf".into());
        index
            .binary
            .insert("clip.mp4".into(), "Export/clip.mp4".into());
        index.whiteboards.insert(
            "Board.excalidraw".into(),
            r#"{"elements":[{"id":"a","type":"rectangle"}]}"#.into(),
        );
        index
    }

    fn blocks(html: &str) -> (Value, Vec<kb_types::ImportWarning>) {
        let index = index();
        let mut warnings = Warnings::default();
        let mut ctx = PageCtx::new(&index, &mut warnings, "Export/Page.html", "", 0);
        let blocks = convert_fragment(&mut ctx, html);
        (Value::Array(blocks), warnings.into_vec())
    }

    fn document(html: &str) -> (HtmlPage, Vec<kb_types::ImportWarning>) {
        let index = index();
        let mut warnings = Warnings::default();
        let mut ctx = PageCtx::new(&index, &mut warnings, "Export/Page.html", "", 0);
        let page = convert_document(&mut ctx, html);
        (page, warnings.into_vec())
    }

    #[test]
    fn paragraphs_headings_and_marks() {
        let (got, _) = blocks(
            "<h1>Title</h1><h5>Deep</h5><p>a <strong>bold</strong> <em>it</em> <u>u</u> <s>s</s> <code>c</code> <mark>m</mark><br>next</p>",
        );
        assert_eq!(
            got[0],
            json!({"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "Title"}]})
        );
        assert_eq!(got[1]["attrs"]["level"], 3);
        let content = got[2]["content"].as_array().unwrap();
        let summary: Vec<(&str, Vec<&str>)> = content
            .iter()
            .map(|n| {
                (
                    n.get("text").and_then(Value::as_str).unwrap_or("<node>"),
                    n.get("marks")
                        .and_then(Value::as_array)
                        .map(|m| m.iter().map(|x| x["type"].as_str().unwrap()).collect())
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("a ", vec![]),
                ("bold", vec!["bold"]),
                (" ", vec![]),
                ("it", vec!["italic"]),
                (" ", vec![]),
                ("u", vec!["underline"]),
                (" ", vec![]),
                ("s", vec!["strike"]),
                (" ", vec![]),
                ("c", vec!["code"]),
                (" ", vec![]),
                ("m", vec!["highlight"]),
                ("<node>", vec![]),
                ("next", vec![]),
            ]
        );
        assert_eq!(content[11]["marks"][0]["attrs"]["color"], "yellow");
        assert_eq!(content[12]["type"], "hardBreak");
    }

    #[test]
    fn white_space_collapses_outside_pre() {
        let (got, _) = blocks(
            "<p>  a \n\t b   <b> c </b> </p><pre><code class=\"language-rust\">fn  main() {\n  x\n}\n</code></pre>",
        );
        assert_eq!(got[0]["content"][0]["text"], "a b ");
        assert_eq!(got[0]["content"][1]["text"], "c");
        assert_eq!(
            got[1],
            json!({"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "fn  main() {\n  x\n}"}]})
        );
    }

    #[test]
    fn lists_tasks_and_start() {
        let (got, _) = blocks(
            "<ul><li>one<ul><li>nested</li></ul></li><li><p>two</p></li></ul><ol start=\"3\"><li>a</li></ol>\
             <ul class=\"to-do-list\"><li><div class=\"checkbox checkbox-on\"></div> <span>done</span></li><li><div class=\"checkbox checkbox-off\"></div>todo</li></ul>\
             <ul><li><input type=\"checkbox\" checked disabled> yes</li><li><input type=\"checkbox\"> no</li></ul>\
             <ul><li><input type=\"checkbox\" checked> mixed</li><li>plain</li></ul>",
        );
        assert_eq!(got[0]["type"], "bulletList");
        assert_eq!(
            got[0]["content"][0]["content"][0]["content"][0]["text"],
            "one"
        );
        assert_eq!(got[0]["content"][0]["content"][1]["type"], "bulletList");
        assert_eq!(got[1]["type"], "orderedList");
        assert_eq!(got[1]["attrs"]["start"], 3);
        assert_eq!(got[2]["type"], "taskList");
        assert_eq!(got[2]["content"][0]["attrs"]["checked"], true);
        assert_eq!(got[2]["content"][1]["attrs"]["checked"], false);
        assert_eq!(got[3]["type"], "taskList");
        assert_eq!(got[3]["content"][0]["attrs"]["checked"], true);
        assert_eq!(got[4]["type"], "bulletList");
    }

    #[test]
    fn tables_with_header_cells() {
        let (got, _) = blocks(
            "<table><thead><tr><th>A</th><th>B</th></tr></thead><tbody><tr><td>1</td></tr><tr><td>2</td><td>3</td><td>4</td></tr></tbody></table>",
        );
        let table = &got[0];
        assert_eq!(table["type"], "table");
        let rows = table["content"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["content"][0]["type"], "tableHeader");
        assert_eq!(rows[1]["content"][0]["type"], "tableCell");
        // Every row is as wide as the widest.
        assert!(
            rows.iter()
                .all(|r| r["content"].as_array().map(Vec::len) == Some(3))
        );
        assert_eq!(
            rows[0]["content"][0]["attrs"],
            json!({"colspan": 1, "rowspan": 1})
        );
    }

    #[test]
    fn details_callouts_columns_toc() {
        let (got, _) = blocks(
            "<details open><summary>More</summary><p>hidden</p></details>\
             <aside>💡 <b>Tip</b> text</aside>\
             <aside class=\"callout\" data-color=\"blue\"><span class=\"callout-icon\">📌</span><div><p>Pinned</p></div></aside>\
             <div class=\"columns\" style=\"--columns:2\"><div class=\"column\"><p>L</p></div><div class=\"column\"><p>R</p></div></div>\
             <nav class=\"toc\"><a href=\"#h\">H</a></nav>",
        );
        assert_eq!(got[0]["type"], "details");
        assert_eq!(got[0]["attrs"]["open"], true);
        assert_eq!(got[0]["content"][0]["content"][0]["text"], "More");
        assert_eq!(
            got[0]["content"][1]["content"][0]["content"][0]["text"],
            "hidden"
        );
        assert_eq!(got[1]["type"], "callout");
        assert_eq!(got[1]["attrs"]["icon"], "💡");
        assert_eq!(got[1]["content"][0]["content"][0]["text"], "Tip");
        assert_eq!(got[2]["attrs"], json!({"icon": "📌", "color": "blue"}));
        assert_eq!(got[2]["content"][0]["content"][0]["text"], "Pinned");
        assert_eq!(got[3]["type"], "columnList");
        assert_eq!(got[3]["content"].as_array().map(Vec::len), Some(2));
        assert_eq!(got[4], json!({"type": "tableOfContents"}));
    }

    #[test]
    fn a_heading_inside_the_summary_makes_a_toggle_heading() {
        // atomik-kb's own HTML export (ADR-0040) and the toggle headings of other tools.
        let (got, _) = blocks(
            "<details open><summary><h2 id=\"h-1\">Part &amp; one</h2></summary><p>hidden</p></details>\
             <details><summary><h5>Deep</h5></summary><p>x</p></details>\
             <details><summary><h1></h1></summary><p>y</p></details>\
             <details><summary>Plain <b>bold</b></summary><p>z</p></details>",
        );
        assert_eq!(
            got[0]["content"][0],
            json!({"type": "detailsSummary", "attrs": {"level": 2},
                   "content": [{"type": "text", "text": "Part & one"}]})
        );
        assert_eq!(got[0]["attrs"]["open"], true);
        assert_eq!(
            got[0]["content"][1]["content"][0]["content"][0]["text"],
            "hidden"
        );
        // h4 to h6 clamp to 3, an empty heading keeps its level, plain text has none.
        assert_eq!(got[1]["content"][0]["attrs"]["level"], 3);
        assert_eq!(
            got[2]["content"][0],
            json!({"type": "detailsSummary", "attrs": {"level": 1}})
        );
        assert!(got[3]["content"][0]["attrs"].is_null());
        assert_eq!(got[3]["content"][0]["content"][0]["text"], "Plain bold");
    }

    #[test]
    fn notion_callout_figures_use_their_icon() {
        let (got, _) = blocks(
            "<figure class=\"callout\"><div><span class=\"icon\">🔥</span></div><div><p>Hot</p></div></figure>",
        );
        assert_eq!(got[0]["type"], "callout");
        assert_eq!(got[0]["attrs"]["icon"], "🔥");
        assert_eq!(got[0]["content"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn links_images_files_and_pages() {
        let (got, warnings) = blocks(
            "<p>see <a href=\"Other.md\">other</a> and <a href=\"https://example.com\">web</a></p>\
             <p><a href=\"Other.md\">Other</a></p>\
             <p><a href=\"doc.pdf\">the doc</a></p>\
             <figure><img src=\"pic.png\" alt=\"A pic\"><figcaption>Caption</figcaption></figure>\
             <p>text <img src=\"https://x.y/i.png\" alt=\"remote\"> more</p>\
             <p><a href=\"gone.md\">gone</a></p>",
        );
        let para = &got[0]["content"];
        assert_eq!(para[1]["type"], "pageRef");
        assert_eq!(para[1]["attrs"]["$page"]["planned"], 5);
        assert_eq!(para[3]["marks"][0]["attrs"]["href"], "https://example.com");
        assert_eq!(got[1]["type"], "pageLink");
        assert_eq!(got[2]["type"], "file");
        assert_eq!(got[2]["attrs"]["$file"], "Export/doc.pdf");
        assert_eq!(got[3]["type"], "image");
        assert_eq!(got[3]["attrs"]["alt"], "A pic");
        assert_eq!(got[3]["attrs"]["caption"], "Caption");
        assert_eq!(
            got[4]["content"][1]["marks"][0]["attrs"]["href"],
            "https://x.y/i.png"
        );
        assert_eq!(got[5]["content"][0]["text"], "gone");
        let codes: Vec<_> = warnings.iter().map(|w| w.code).collect();
        assert_eq!(
            codes,
            [
                ImportWarningCode::RemoteImage,
                ImportWarningCode::UnresolvedLink
            ]
        );
    }

    #[test]
    fn exported_figures_keep_their_layout() {
        let (got, _) = blocks(
            "<figure class=\"image align-right\" style=\"width:60%\"><img src=\"pic.png\" alt=\"x\"><figcaption>c</figcaption></figure>",
        );
        assert_eq!(got[0]["attrs"]["widthPercent"], 60);
        assert_eq!(got[0]["attrs"]["align"], "right");
    }

    #[test]
    fn media_and_whiteboards() {
        let (got, warnings) = blocks(
            "<video controls src=\"clip.mp4\"></video><audio><source src=\"gone.mp3\"></audio>\
             <figure class=\"whiteboard\"><a href=\"Board.excalidraw\">Whiteboard</a><ul><li>text</li></ul></figure>",
        );
        assert_eq!(got[0]["type"], "file");
        assert_eq!(got[0]["attrs"]["view"], "preview");
        assert_eq!(got[1]["type"], "whiteboard");
        assert!(got[1]["attrs"]["$elements"]["a"].is_object());
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, ImportWarningCode::MissingFile);
    }

    #[test]
    fn dates_and_equations() {
        let (got, _) = blocks(
            "<p>due <time datetime=\"2026-10-07T14:30\" data-reminder=\"15\">2026-10-07 14:30</time> and \
             <span class=\"math math-inline\">\\(E=mc^2\\)</span></p>\
             <div class=\"math math-display\">\\[a^2\\]</div>\
             <p><math><semantics><mrow><mi>x</mi></mrow><annotation encoding=\"application/x-tex\">x^2</annotation></semantics></math></p>",
        );
        let content = &got[0]["content"];
        assert_eq!(
            content[1],
            json!({"type": "dateMention", "attrs": {"date": "2026-10-07", "time": "14:30", "reminder": 15}})
        );
        assert_eq!(
            content[3],
            json!({"type": "mathInline", "attrs": {"latex": "E=mc^2"}})
        );
        assert_eq!(
            got[1],
            json!({"type": "mathBlock", "content": [{"type": "text", "text": "a^2"}]})
        );
        assert_eq!(
            got[2]["content"][0],
            json!({"type": "mathInline", "attrs": {"latex": "x^2"}})
        );
    }

    #[test]
    fn unsupported_elements_are_dropped_with_a_warning() {
        let (got, warnings) = blocks(
            "<p>keep</p><script>alert(1)</script><style>p{}</style><iframe src=\"x\"></iframe>\
             <form><p>form text</p><input type=\"text\"><button>Go</button></form><svg><text>no</text></svg>",
        );
        let text = serde_json::to_string(&got).unwrap();
        assert!(text.contains("keep") && text.contains("form text") && text.contains("Go"));
        assert!(!text.contains("alert") && !text.contains("p{}") && !text.contains("no\""));
        let mut details: Vec<_> = warnings.iter().map(|w| w.detail.clone().unwrap()).collect();
        details.sort();
        assert_eq!(
            details,
            [
                "button", "form", "iframe", "input", "script", "style", "svg"
            ]
        );
        assert!(
            warnings
                .iter()
                .all(|w| w.code == ImportWarningCode::UnsupportedContent)
        );
    }

    #[test]
    fn documents_give_title_icon_and_content() {
        let (page, _) = document(
            "<html><head><title>Head title</title></head><body><nav>menu</nav>\
             <article><header><div class=\"page-header-icon\"><span class=\"icon\">📝</span></div><h1 class=\"page-title\">Real title</h1></header>\
             <div class=\"page-body\"><p>Body</p></div></article></body></html>",
        );
        assert_eq!(page.title.as_deref(), Some("Real title"));
        assert_eq!(page.icon.as_deref(), Some("📝"));
        assert_eq!(page.blocks.len(), 1);
        assert_eq!(page.blocks[0]["content"][0]["text"], "Body");

        let (page, _) = document(
            "<html><head><title>Only title</title></head><body><h1>Only title</h1><p>x</p></body></html>",
        );
        assert_eq!(page.title.as_deref(), Some("Only title"));
        assert_eq!(page.blocks.len(), 2);

        let (page, _) = document("<body><h1>Heading</h1><p>x</p></body>");
        assert_eq!(page.title.as_deref(), Some("Heading"));
        assert_eq!(page.blocks.len(), 1);

        let (page, _) = document("<body><p>x</p><h1>Late</h1></body>");
        assert_eq!(page.title, None);
        assert_eq!(page.blocks.len(), 2);

        let (page, _) = document("");
        assert!(page.blocks.is_empty());
    }

    #[test]
    fn hostile_nesting_is_flattened_not_followed() {
        let deep = format!("{}text{}", "<div>".repeat(5_000), "</div>".repeat(5_000));
        let (got, _) = blocks(&deep);
        assert!(serde_json::to_string(&got).unwrap().contains("text"));
        let quotes = format!(
            "{}deep{}",
            "<blockquote>".repeat(300),
            "</blockquote>".repeat(300)
        );
        let (got, _) = blocks(&quotes);
        assert!(json_depth(&got) <= 62);
        assert!(serde_json::to_string(&got).unwrap().contains("deep"));
        let lists = format!("{}x{}", "<ul><li>".repeat(300), "</li></ul>".repeat(300));
        let (got, _) = blocks(&lists);
        assert!(json_depth(&got) <= 62);
    }

    /// The nesting of nodes (the depth the page document limits), not of JSON containers.
    fn json_depth(value: &Value) -> usize {
        match value {
            Value::Array(items) => items.iter().map(json_depth).max().unwrap_or(0),
            Value::Object(map) => 1 + map.get("content").map_or(0, json_depth),
            _ => 0,
        }
    }

    #[test]
    fn input_cut_into_chunks_parses_like_whole_input() {
        // Tags, entities and multi-byte characters straddle the boundaries of the 8 KiB pieces.
        for pad in 8_170..8_200 {
            let html = format!("<p>{}&amp;é<b>y</b>&lt;<i>z</i></p>", "x".repeat(pad));
            let (got, _) = blocks(&html);
            let content = got[0]["content"].as_array().unwrap();
            let text: String = content.iter().filter_map(|n| n["text"].as_str()).collect();
            assert_eq!(text, format!("{}&éy<z", "x".repeat(pad)), "pad {pad}");
            assert_eq!(
                content.iter().filter(|n| n.get("marks").is_some()).count(),
                2,
                "pad {pad}"
            );
        }
        let many = "<p>paragraph</p>".repeat(5_000);
        let (got, _) = blocks(&many);
        assert_eq!(got.as_array().map(Vec::len), Some(5_000));
    }

    #[test]
    fn huge_tables_are_cut() {
        let rows = "<tr><td>x</td></tr>".repeat(MAX_TABLE_ROWS + 50);
        let (got, warnings) = blocks(&format!("<table>{rows}</table>"));
        assert_eq!(
            got[0]["content"].as_array().map(Vec::len),
            Some(MAX_TABLE_ROWS)
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.detail.as_deref() == Some("table rows"))
        );
    }
}
