//! A page as a standalone HTML document (`docs/architecture/import-export.md`, "HTML").
//!
//! `<!doctype html>`, `lang="en"`, charset, viewport and generator metas, `<title>`, one embedded
//! `<style>` (readable typography, no maximum width, light colours with a `prefers-color-scheme:
//! dark` variant, print rules) and `<article class="page">` with a `<header>` (icon, the
//! `<h1 class="page-title">`, a `<table class="properties">`) followed by the blocks. No script and
//! no external resource: images, files and whiteboards are the archive's own relative files.
//!
//! **Nothing in a page can inject markup**: every text and attribute value is escaped (`& < > " '`);
//! the `href` of a `link` mark is kept only for `http`, `https`, `mailto` and `tel` (otherwise the
//! text stands alone); the links the layout produced are kept only when they are relative (no
//! scheme, not protocol-relative) or `attachment://<id>`; class names, ids and styles are made of
//! what the exporter chooses (a code block's language is cut down to letters, digits and `_+#.-`).
//!
//! Mapping: `paragraph` `<p>`; `heading` `<h1>` to `<h3>` with `id="h-<blockId>"`; `bulletList`
//! `<ul>`, `orderedList` `<ol start>`, `taskList` `<ul class="task-list">` with `<li class="task"
//! data-checked="true|false"><input type="checkbox" disabled [checked]>`; `blockquote`; `callout`
//! `<aside class="callout" data-color><span class="callout-icon">…</span><div>…</div></aside>`;
//! `codeBlock` `<pre><code class="language-x">` (Mermaid stays its source); `mathBlock` `<div
//! class="math math-display">\[…\]</div>` and `mathInline` `<span class="math math-inline">\(…\)</span>`
//! (the LaTeX source, escaped, not rendered); `horizontalRule` `<hr>`; `table` (`tableHeader` as
//! `<th>`); `details`; `columnList` `<div class="columns" style="--columns:N">` of `<div
//! class="column">`; `image` `<figure class="image align-…" style="width:N%"><img src alt>
//! <figcaption>`; `file` `<a class="file" href>` (a `<video controls>` or `<audio controls>` for a
//! preview of a video or audio file); `gallery` `<div class="gallery" style="--columns:N">` of
//! figures; `whiteboard` `<figure class="whiteboard"><a href="….excalidraw">Whiteboard</a><ul>` of
//! its texts; `pageRef` `<a class="page-ref" href>` (a `<span class="page-ref">` when the page is
//! not in the export) and `pageLink` the same in `<p class="page-link">`; `dateMention` `<time
//! datetime>` (`data-reminder` as in the Markdown flavour); `personMention` `<span
//! class="person-mention">@label</span>` (ADR-0049: the label stored with the mention, escaped, a
//! control character in it a space, and no account id: it means nothing outside the workspace;
//! nothing when the label is empty or blank);
//! `tableOfContents` `<nav class="toc">`
//! listing the page's headings as links to their ids; `breadcrumb`, `pageList` and `templateButton` (ADR-0056: the
//! button holds template material that only a click makes part of the page) nothing; marks
//! `strong`, `em`, `u`, `s`, `code`, `a`, `mark data-color`, `span class="text-color" data-color`;
//! `hardBreak` `<br>`; any other node its children.

use kb_types::{AttachmentId, PageId};
use serde_json::Value;

use crate::ExportLinks;
use crate::excalidraw;
use crate::value::{ExportValue, PageMeta};

/// The scheme of the link to an attachment that is not in the archive.
const ATTACHMENT_SCHEME: &str = "attachment://";

/// Icon of a callout without one (the schema default).
const DEFAULT_CALLOUT_ICON: &str = "💡";

/// Extensions of the files a `view: "preview"` file block plays.
const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "m4v", "mov", "webm", "ogv"];
const AUDIO_EXTENSIONS: [&str; 8] = ["mp3", "m4a", "wav", "ogg", "oga", "flac", "aac", "opus"];

/// The stylesheet: no `@import`, no `url()`, nothing is loaded.
const STYLE: &str = "\
:root { color-scheme: light dark; --fg: #1f2328; --bg: #ffffff; --muted: #656d76; --line: #d0d7de; --soft: #f6f8fa; --link: #0969da; --mark: #fff3a3; }
@media (prefers-color-scheme: dark) { :root { --fg: #e6edf3; --bg: #0d1117; --muted: #9198a1; --line: #30363d; --soft: #161b22; --link: #4493f8; --mark: #5a4a00; } }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.6 system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, sans-serif; }
.page { max-width: none; padding: 2.5rem clamp(1rem, 6vw, 5rem) 4rem; }
.page-cover { display: block; width: auto; height: clamp(160px, 30vh, 280px); margin: -2.5rem calc(-1 * clamp(1rem, 6vw, 5rem)) 1.5rem; object-fit: cover; background: var(--soft); }
.page-cover[data-color=\"gray\"] { background: #d9d8d4; }
.page-cover[data-color=\"brown\"] { background: #d8c3ba; }
.page-cover[data-color=\"orange\"] { background: #f5c996; }
.page-cover[data-color=\"yellow\"] { background: #f1dc8f; }
.page-cover[data-color=\"green\"] { background: #b4d9b9; }
.page-cover[data-color=\"blue\"] { background: #a6cfe8; }
.page-cover[data-color=\"purple\"] { background: #cdbbe6; }
.page-cover[data-color=\"pink\"] { background: #eab4cd; }
.page-cover[data-color=\"red\"] { background: #f0aeb4; }
.page-cover[data-gradient=\"sunset\"] { background: linear-gradient(135deg, #f6d365, #fda085); }
.page-cover[data-gradient=\"dawn\"] { background: linear-gradient(135deg, #a18cd1, #fbc2eb); }
.page-cover[data-gradient=\"ocean\"] { background: linear-gradient(135deg, #2193b0, #6dd5ed); }
.page-cover[data-gradient=\"forest\"] { background: linear-gradient(135deg, #134e5e, #71b280); }
.page-cover[data-gradient=\"lavender\"] { background: linear-gradient(135deg, #cfd9df, #e2d1f9); }
.page-cover[data-gradient=\"peach\"] { background: linear-gradient(135deg, #ffecd2, #fcb69f); }
.page-cover[data-gradient=\"mint\"] { background: linear-gradient(135deg, #84fab0, #8fd3f4); }
.page-cover[data-gradient=\"slate\"] { background: linear-gradient(135deg, #485563, #29323c); }
.page-cover[data-gradient=\"candy\"] { background: linear-gradient(135deg, #ff9a9e, #fad0c4); }
.page-cover[data-gradient=\"aurora\"] { background: linear-gradient(135deg, #43e97b, #38f9d7); }
.page-icon { font-size: 3rem; line-height: 1; display: block; margin-bottom: .5rem; }
img.page-icon { width: 3rem; height: 3rem; object-fit: cover; border-radius: .25rem; }
.page-title { font-size: 2.25rem; line-height: 1.2; margin: 0 0 1rem; }
a { color: var(--link); }
p { margin: .6em 0; }
h1, h2, h3 { line-height: 1.25; margin: 1.6em 0 .5em; }
ul, ol { padding-left: 1.6em; }
li > p { margin: .2em 0; }
.task-list { list-style: none; padding-left: .2em; }
.task input { margin-right: .5em; }
blockquote { margin: .8em 0; padding: .1em 1em; border-left: 3px solid var(--line); color: var(--muted); }
hr { border: 0; border-top: 1px solid var(--line); margin: 1.6em 0; }
code { font: .9em ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; background: var(--soft); padding: .1em .3em; border-radius: .25rem; }
pre { background: var(--soft); padding: .8em 1em; border-radius: .4rem; overflow: auto; }
pre code { background: none; padding: 0; }
mark { background: var(--mark); color: inherit; }
table { border-collapse: collapse; margin: 1em 0; }
th, td { border: 1px solid var(--line); padding: .35em .7em; text-align: left; vertical-align: top; }
th { background: var(--soft); }
table.properties { margin: 0 0 1.5rem; }
table.properties th { width: 1%; white-space: nowrap; font-weight: 600; color: var(--muted); background: none; border-width: 0 0 1px; }
table.properties td { border-width: 0 0 1px; }
details { margin: .8em 0; }
summary { cursor: pointer; font-weight: 600; }
summary > :is(h1, h2, h3) { display: inline; margin: 0; }
.callout { display: flex; gap: .7em; margin: .8em 0; padding: .8em 1em; border-radius: .4rem; background: var(--soft); }
.callout-icon { font-size: 1.3em; line-height: 1.4; }
.callout > div { flex: 1; min-width: 0; }
.callout > div > :first-child { margin-top: 0; }
.callout > div > :last-child { margin-bottom: 0; }
.columns { display: grid; grid-template-columns: repeat(var(--columns, 2), minmax(0, 1fr)); gap: 1.5rem; }
.gallery { display: grid; grid-template-columns: repeat(var(--columns, 3), minmax(0, 1fr)); gap: .75rem; }
figure { margin: 1em 0; }
figure.image img, .gallery img { max-width: 100%; height: auto; display: block; border-radius: .25rem; }
figure.image.align-center img { margin: 0 auto; }
figure.image.align-right img { margin-left: auto; }
figcaption { color: var(--muted); font-size: .9em; margin-top: .4em; }
video, audio { max-width: 100%; }
.math-display { overflow-x: auto; margin: .8em 0; }
.toc { margin: 1em 0; padding: .5em 1em; background: var(--soft); border-radius: .4rem; }
.toc ul { list-style: none; margin: 0; padding: 0; }
.toc .toc-level-2 { padding-left: 1.2em; }
.toc .toc-level-3 { padding-left: 2.4em; }
.page-ref { text-decoration: none; border-bottom: 1px solid var(--line); }
.person-mention { background: var(--soft); border-radius: .25rem; padding: 0 .25em; }
@media print {
  :root { --fg: #000; --bg: #fff; --muted: #444; --line: #bbb; --soft: #f2f2f2; --link: #000; }
  .page { padding: 0; }
  a { text-decoration: none; }
  pre, blockquote, figure, table, tr, .callout { break-inside: avoid; }
  h1, h2, h3 { break-after: avoid; }
}
";

/// The whole HTML document of a page: an embedded stylesheet (light and dark), a header with the
/// icon, the title and the properties table, then the blocks. Nothing is loaded from elsewhere
/// except the archive's own files.
#[must_use]
pub fn page_html(meta: &PageMeta, content: &Value, links: &dyn ExportLinks) -> String {
    let headings = headings_of(content);
    let html = Html {
        links,
        whiteboards: content.get("whiteboards").unwrap_or(&Value::Null),
        headings: &headings,
    };
    let body = if node_type(content) == "doc" {
        html.blocks(children(content))
    } else {
        html.block(content)
    };
    let title = if meta.title.is_empty() {
        "Untitled"
    } else {
        meta.title.as_str()
    };
    let title = escape(title);
    let mut out = String::new();
    out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str("<meta name=\"generator\" content=\"atomik-kb\">\n");
    out.push_str(&format!(
        "<title>{title}</title>\n<style>\n{STYLE}</style>\n</head>\n<body>\n"
    ));
    out.push_str("<article class=\"page\">\n");
    out.push_str(&cover_html(meta.cover.as_deref(), meta.cover_y));
    out.push_str("<header>\n");
    out.push_str(&icon_html(meta.icon.as_deref()));
    out.push_str(&format!("<h1 class=\"page-title\">{title}</h1>\n"));
    out.push_str(&properties_html(meta));
    out.push_str("</header>\n");
    out.push_str(&body);
    out.push_str("</article>\n</body>\n</html>\n");
    out
}

// ----- the header ----------------------------------------------------------------------------

/// The page cover (ADR-0040), a banner above the header: a picture (a relative link to its file) as
/// an image placed by its vertical position, a palette colour or a bundled gradient as a block the
/// stylesheet colours (`data-color`, `data-gradient`; a gradient this stylesheet does not know shows
/// as the neutral banner). Nothing when there is no cover or it is a link to nothing.
fn cover_html(cover: Option<&str>, cover_y: Option<u8>) -> String {
    let Some(cover) = cover.filter(|cover| !cover.is_empty()) else {
        return String::new();
    };
    if let Some(name) = cover.strip_prefix("color:") {
        return format!(
            "<div class=\"page-cover\" data-color=\"{}\"></div>\n",
            escape(name)
        );
    }
    if let Some(name) = cover.strip_prefix("gradient:") {
        return format!(
            "<div class=\"page-cover\" data-gradient=\"{}\"></div>\n",
            escape(name)
        );
    }
    if !cover.contains(':') && (cover.contains('/') || cover.contains('.')) && is_local(cover) {
        let y = cover_y.filter(|y| *y <= 100).unwrap_or(50);
        return format!(
            "<img class=\"page-cover\" src=\"{}\" alt=\"\" style=\"object-position:50% {y}%\">\n",
            escape(cover)
        );
    }
    String::new()
}

/// The page icon: an emoji as a span, an uploaded icon (a relative link) as an image, nothing for
/// a named icon of the app's own set.
fn icon_html(icon: Option<&str>) -> String {
    let Some(icon) = icon.filter(|icon| !icon.is_empty() && !icon.starts_with("icon:")) else {
        return String::new();
    };
    let is_link = !icon.contains(':') && (icon.contains('/') || icon.contains('.'));
    if is_link {
        if is_local(icon) {
            format!(
                "<img class=\"page-icon\" src=\"{}\" alt=\"\">\n",
                escape(icon)
            )
        } else {
            String::new()
        }
    } else {
        // Notion's class too, so that an importer that knows its pages finds the icon.
        format!(
            "<span class=\"page-icon page-header-icon\">{}</span>\n",
            escape(icon)
        )
    }
}

/// The table of the page's properties: the name and the value as text; related pages and files as
/// links where they have one.
fn properties_html(meta: &PageMeta) -> String {
    if meta.properties.is_empty() {
        return String::new();
    }
    let mut out = String::from("<table class=\"properties\">\n");
    for property in &meta.properties {
        let value = match &property.value {
            ExportValue::Pages(pages) => pages
                .iter()
                .map(
                    |page| match page.href.as_deref().filter(|href| is_local(href)) {
                        Some(href) => format!(
                            "<a class=\"page-ref\" href=\"{}\">{}</a>",
                            escape(href),
                            escape(&page.title)
                        ),
                        None => format!("<span class=\"page-ref\">{}</span>", escape(&page.title)),
                    },
                )
                .collect::<Vec<_>>()
                .join(", "),
            ExportValue::Files(files) => files
                .iter()
                .map(
                    |file| match file.href.as_deref().filter(|href| is_local(href)) {
                        Some(href) => format!(
                            "<a class=\"file\" href=\"{}\">{}</a>",
                            escape(href),
                            escape(&file.name)
                        ),
                        None => escape(&file.name),
                    },
                )
                .collect::<Vec<_>>()
                .join(", "),
            other => escape(&other.text(0)),
        };
        out.push_str(&format!(
            "<tr><th scope=\"row\">{}</th><td>{value}</td></tr>\n",
            escape(&property.name)
        ));
    }
    out.push_str("</table>\n");
    out
}

// ----- the blocks ----------------------------------------------------------------------------

/// A heading of the page, for the table of contents.
struct Heading {
    /// The fragment id (`h-<blockId>`).
    id: String,
    /// 1 to 3.
    level: u8,
    /// Its text.
    text: String,
}

/// The headings that have an id, in document order, at any depth. A toggle heading (a
/// `detailsSummary` with a `level`, ADR-0040) is one, with the id of its summary.
fn headings_of(content: &Value) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut stack = vec![content];
    while let Some(node) = stack.pop() {
        let level = match node_type(node) {
            "heading" => Some(heading_level(node)),
            "detailsSummary" => summary_level(node),
            _ => None,
        };
        if let Some(level) = level
            && let Some(id) = heading_id(node)
        {
            out.push(Heading {
                id,
                level,
                text: plain_text(node),
            });
        }
        stack.extend(children(node).iter().rev());
    }
    out
}

/// `h-<blockId>` with the characters that are fit for a fragment, or `None` without a usable id.
fn heading_id(node: &Value) -> Option<String> {
    let id: String = str_attr(node, "id")?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    (!id.is_empty()).then(|| format!("h-{id}"))
}

/// The level of a heading: 1 to 3 (1 when it is not an integer, 3 when it is deeper).
fn heading_level(node: &Value) -> u8 {
    let level = attr(node, "level")
        .and_then(Value::as_i64)
        .filter(|level| *level >= 1)
        .unwrap_or(1);
    u8::try_from(level.min(3)).unwrap_or(1)
}

/// The level of a toggle heading, the `level` of its summary: an integer from 1 to 3 (`2.0` is 2);
/// `None` for a plain toggle and for any other value.
fn summary_level(node: &Value) -> Option<u8> {
    let level = attr(node, "level")?;
    let level = level.as_i64().or_else(|| {
        level
            .as_f64()
            .filter(|f| f.fract() == 0.0)
            .map(|f| f as i64)
    })?;
    u8::try_from(level)
        .ok()
        .filter(|level| (1..=3).contains(level))
}

/// The text under `node`, without marks.
fn plain_text(node: &Value) -> String {
    match node_type(node) {
        "text" => node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        "mathInline" => str_attr(node, "latex").unwrap_or_default().to_owned(),
        "personMention" => person_mention_text(node),
        _ => children(node).iter().map(plain_text).collect(),
    }
}

struct Html<'a> {
    links: &'a dyn ExportLinks,
    /// The top-level `whiteboards` value (`Null` when the page has none).
    whiteboards: &'a Value,
    headings: &'a [Heading],
}

impl Html<'_> {
    /// The blocks, one after the other; consecutive inline nodes form a paragraph.
    fn blocks(&self, nodes: &[Value]) -> String {
        let mut out = String::new();
        let mut inline: Vec<&Value> = Vec::new();
        for node in nodes {
            if is_inline(node) {
                inline.push(node);
                continue;
            }
            if !inline.is_empty() {
                out.push_str(&format!(
                    "<p>{}</p>\n",
                    self.inline(&std::mem::take(&mut inline))
                ));
            }
            out.push_str(&self.block(node));
        }
        if !inline.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", self.inline(&inline)));
        }
        out
    }

    fn block(&self, node: &Value) -> String {
        match node_type(node) {
            "paragraph" => format!("<p>{}</p>\n", self.inline_of(node)),
            "heading" => {
                let level = heading_level(node);
                let id = heading_id(node)
                    .map(|id| format!(" id=\"{}\"", escape(&id)))
                    .unwrap_or_default();
                format!("<h{level}{id}>{}</h{level}>\n", self.inline_of(node))
            }
            "bulletList" => format!("<ul>\n{}</ul>\n", self.items(node, false)),
            "orderedList" => {
                let start = attr(node, "start")
                    .and_then(Value::as_i64)
                    .filter(|start| *start != 1)
                    .map(|start| format!(" start=\"{start}\""))
                    .unwrap_or_default();
                format!("<ol{start}>\n{}</ol>\n", self.items(node, false))
            }
            "taskList" => format!(
                "<ul class=\"task-list\">\n{}</ul>\n",
                self.items(node, true)
            ),
            "blockquote" => format!(
                "<blockquote>\n{}</blockquote>\n",
                self.blocks(children(node))
            ),
            "callout" => self.callout(node),
            "codeBlock" => {
                let language: String = str_attr(node, "language")
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '#' | '.' | '-')
                    })
                    .collect();
                let class = if language.is_empty() {
                    String::new()
                } else {
                    format!(" class=\"language-{language}\"")
                };
                format!(
                    "<pre><code{class}>{}</code></pre>\n",
                    escape(&plain_text(node))
                )
            }
            "mathBlock" => format!(
                "<div class=\"math math-display\">\\[{}\\]</div>\n",
                escape(&plain_text(node))
            ),
            "horizontalRule" => "<hr>\n".to_owned(),
            "table" => self.table(node),
            "details" => self.details(node),
            "columnList" => {
                let columns = children(node)
                    .iter()
                    .filter(|child| node_type(child) == "column")
                    .count()
                    .max(1);
                format!(
                    "<div class=\"columns\" style=\"--columns:{columns}\">\n{}</div>\n",
                    self.blocks(children(node))
                )
            }
            "column" => format!(
                "<div class=\"column\">\n{}</div>\n",
                self.blocks(children(node))
            ),
            "image" => self.image(node),
            "file" => self.file(node),
            "gallery" => self.gallery(node),
            "whiteboard" => self.whiteboard(node),
            "pageLink" => format!("<p class=\"page-link\">{}</p>\n", self.page_ref(node)),
            "tableOfContents" => self.table_of_contents(),
            // The rows of these are derived from the tree at run time; a file has no tree.
            "breadcrumb" | "pageList" => String::new(),
            // Template material (ADR-0056): the page's text has none of it, and a file has no button to click.
            "templateButton" => String::new(),
            _ if is_inline(node) => format!("<p>{}</p>\n", self.inline(&[node])),
            _ => self.blocks(children(node)),
        }
    }

    /// The `<li>`s of a list.
    fn items(&self, list: &Value, task: bool) -> String {
        let mut out = String::new();
        for item in children(list) {
            // A list item wraps its blocks; anything else in a list stands for itself.
            let body: &[Value] = if matches!(node_type(item), "listItem" | "taskItem") {
                children(item)
            } else {
                std::slice::from_ref(item)
            };
            let (lead, rest) = match body.split_first() {
                Some((first, rest)) if node_type(first) == "paragraph" => {
                    (self.inline_of(first), rest)
                }
                _ => (String::new(), body),
            };
            let opening = if task {
                let checked = attr(item, "checked") == Some(&Value::Bool(true));
                format!(
                    "<li class=\"task\" data-checked=\"{checked}\"><input type=\"checkbox\" disabled{}> ",
                    if checked { " checked" } else { "" }
                )
            } else {
                "<li>".to_owned()
            };
            out.push_str(&opening);
            out.push_str(&lead);
            let rest = self.blocks(rest);
            if !rest.is_empty() {
                out.push('\n');
                out.push_str(&rest);
            }
            out.push_str("</li>\n");
        }
        out
    }

    fn callout(&self, node: &Value) -> String {
        let icon = str_attr(node, "icon").unwrap_or(DEFAULT_CALLOUT_ICON);
        let color = str_attr(node, "color")
            .map(|color| format!(" data-color=\"{}\"", escape(color)))
            .unwrap_or_default();
        format!(
            "<aside class=\"callout\"{color}><span class=\"callout-icon\">{}</span><div>\n{}</div></aside>\n",
            escape(icon),
            self.blocks(children(node))
        )
    }

    /// A table: `tableHeader` cells are `<th>`; a cell that is one paragraph is its inline content.
    fn table(&self, table: &Value) -> String {
        let mut out = String::from("<table>\n");
        for row in children(table) {
            out.push_str("<tr>");
            for cell in children(row) {
                let tag = if node_type(cell) == "tableHeader" {
                    "th"
                } else {
                    "td"
                };
                let span = |name: &str| {
                    attr(cell, name)
                        .and_then(Value::as_i64)
                        .filter(|span| *span > 1)
                        .map(|span| format!(" {name}=\"{span}\""))
                        .unwrap_or_default()
                };
                let body = match children(cell) {
                    [only] if node_type(only) == "paragraph" => self.inline_of(only),
                    blocks => self.blocks(blocks),
                };
                out.push_str(&format!(
                    "<{tag}{}{}>{body}</{tag}>",
                    span("colspan"),
                    span("rowspan")
                ));
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</table>\n");
        out
    }

    fn details(&self, node: &Value) -> String {
        let open = if attr(node, "open") == Some(&Value::Bool(true)) {
            " open"
        } else {
            ""
        };
        let mut summary = String::new();
        let mut body = String::new();
        for child in children(node) {
            match node_type(child) {
                "detailsSummary" if summary.is_empty() => summary = self.summary_of(child),
                "detailsContent" => body.push_str(&self.blocks(children(child))),
                _ => body.push_str(&self.block(child)),
            }
        }
        format!("<details{open}>\n<summary>{summary}</summary>\n{body}</details>\n")
    }

    /// The inside of a toggle's `<summary>`: its text, in a heading element for a toggle heading
    /// (with the fragment id the table of contents links to, when the summary has one).
    fn summary_of(&self, node: &Value) -> String {
        let text = self.inline_of(node);
        let Some(level) = summary_level(node) else {
            return text;
        };
        let id = heading_id(node)
            .map(|id| format!(" id=\"{}\"", escape(&id)))
            .unwrap_or_default();
        format!("<h{level}{id}>{text}</h{level}>")
    }

    /// The href of an attachment's file in the archive, or `attachment://<id>`; escaped.
    fn attachment_src(&self, stored: &str) -> String {
        let href = stored
            .parse::<AttachmentId>()
            .ok()
            .and_then(|id| self.links.attachment_href(id))
            .filter(|href| is_local(href))
            .unwrap_or_else(|| format!("{ATTACHMENT_SCHEME}{stored}"));
        escape(&href)
    }

    fn image(&self, node: &Value) -> String {
        let src = self.attachment_src(str_attr(node, "attachmentId").unwrap_or_default());
        let alt = escape(
            attr(node, "alt")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let align = match str_attr(node, "align") {
            Some(side @ ("left" | "center" | "right")) => format!(" align-{side}"),
            _ => String::new(),
        };
        let percent = attr(node, "widthPercent")
            .and_then(Value::as_i64)
            .filter(|percent| (10..=100).contains(percent));
        let pixels = attr(node, "width")
            .and_then(Value::as_i64)
            .filter(|pixels| *pixels > 0);
        let style = match (percent, pixels) {
            (Some(percent), _) => format!(" style=\"width:{percent}%\""),
            (None, Some(pixels)) => format!(" style=\"width:{pixels}px;max-width:100%\""),
            (None, None) => String::new(),
        };
        let caption = str_attr(node, "caption")
            .map(|caption| format!("<figcaption>{}</figcaption>", escape(caption)))
            .unwrap_or_default();
        format!(
            "<figure class=\"image{align}\"{style}><img src=\"{src}\" alt=\"{alt}\">{caption}</figure>\n"
        )
    }

    fn file(&self, node: &Value) -> String {
        let stored = str_attr(node, "attachmentId").unwrap_or_default();
        let name = stored
            .parse::<AttachmentId>()
            .ok()
            .and_then(|id| self.links.attachment_name(id))
            .unwrap_or_else(|| stored.to_owned());
        let src = self.attachment_src(stored);
        let extension = name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_default();
        if str_attr(node, "view") == Some("preview") {
            if VIDEO_EXTENSIONS.contains(&extension.as_str()) {
                return format!("<p><video controls src=\"{src}\"></video></p>\n");
            }
            if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
                return format!("<p><audio controls src=\"{src}\"></audio></p>\n");
            }
        }
        format!(
            "<p><a class=\"file\" href=\"{src}\">{}</a></p>\n",
            escape(&name)
        )
    }

    fn gallery(&self, node: &Value) -> String {
        let items = attr(node, "items")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let figures: String = items
            .iter()
            .filter_map(|item| {
                let id = item
                    .get("attachmentId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())?;
                let caption = item
                    .get("caption")
                    .and_then(Value::as_str)
                    .filter(|caption| !caption.is_empty())
                    .map(|caption| format!("<figcaption>{}</figcaption>", escape(caption)))
                    .unwrap_or_default();
                Some(format!(
                    "<figure class=\"image\"><img src=\"{}\" alt=\"\">{caption}</figure>\n",
                    self.attachment_src(id)
                ))
            })
            .collect();
        if figures.is_empty() {
            return String::new();
        }
        let columns = attr(node, "columns")
            .and_then(Value::as_i64)
            .filter(|columns| (2..=4).contains(columns))
            .unwrap_or(3);
        format!("<div class=\"gallery\" style=\"--columns:{columns}\">\n{figures}</div>\n")
    }

    fn whiteboard(&self, node: &Value) -> String {
        let id = str_attr(node, "id").unwrap_or_default();
        let file = id
            .parse::<kb_types::BlockId>()
            .ok()
            .and_then(|block| self.links.whiteboard_href(block))
            .filter(|href| is_local(href));
        let label = match file {
            Some(href) => format!("<a href=\"{}\">Whiteboard</a>", escape(&href)),
            None => "<span>Whiteboard</span>".to_owned(),
        };
        let texts = excalidraw::texts(self.whiteboards, id);
        let list = if texts.is_empty() {
            String::new()
        } else {
            let items: String = texts
                .iter()
                .map(|text| format!("<li>{}</li>", escape(text)))
                .collect();
            format!("<ul>{items}</ul>")
        };
        format!("<figure class=\"whiteboard\">{label}{list}</figure>\n")
    }

    fn table_of_contents(&self) -> String {
        if self.headings.is_empty() {
            return "<nav class=\"toc\"></nav>\n".to_owned();
        }
        let items: String = self
            .headings
            .iter()
            .map(|heading| {
                format!(
                    "<li class=\"toc-level-{}\"><a href=\"#{}\">{}</a></li>\n",
                    heading.level,
                    escape(&heading.id),
                    escape(&heading.text)
                )
            })
            .collect();
        format!("<nav class=\"toc\">\n<ul>\n{items}</ul>\n</nav>\n")
    }

    // ----- inline -----

    fn inline_of(&self, node: &Value) -> String {
        let nodes: Vec<&Value> = children(node).iter().collect();
        self.inline(&nodes)
    }

    /// Inline nodes: consecutive text nodes with the same marks are one element.
    fn inline(&self, nodes: &[&Value]) -> String {
        let mut out = String::new();
        let mut run = String::new();
        let mut marks = Marks::default();
        for node in nodes {
            if node_type(node) == "text" {
                let text = node.get("text").and_then(Value::as_str).unwrap_or_default();
                let next = Marks::of(
                    node.get("marks")
                        .and_then(Value::as_array)
                        .map_or(&[][..], Vec::as_slice),
                );
                if next != marks {
                    flush(&mut out, &mut run, &marks);
                    marks = next;
                }
                run.push_str(&escape(text));
                continue;
            }
            flush(&mut out, &mut run, &marks);
            marks = Marks::default();
            match node_type(node) {
                "hardBreak" => out.push_str("<br>"),
                "pageRef" => out.push_str(&self.page_ref(node)),
                "dateMention" => out.push_str(&date_html(node)),
                "personMention" => out.push_str(&person_mention_html(node)),
                "mathInline" => out.push_str(&format!(
                    "<span class=\"math math-inline\">\\({}\\)</span>",
                    escape(str_attr(node, "latex").unwrap_or_default())
                )),
                _ => out.push_str(&self.inline_of(node)),
            }
        }
        flush(&mut out, &mut run, &marks);
        out
    }

    /// A reference to a page: a link when the page is part of the export.
    fn page_ref(&self, node: &Value) -> String {
        let stored = str_attr(node, "pageId").unwrap_or_default();
        let page = stored.parse::<PageId>().ok();
        let title = page
            .and_then(|id| self.links.page_title(id))
            .unwrap_or_else(|| stored.to_owned());
        let href = page
            .and_then(|id| self.links.page_href(id))
            .filter(|href| is_local(href));
        match href {
            Some(href) => format!(
                "<a class=\"page-ref\" href=\"{}\">{}</a>",
                escape(&href),
                escape(&title)
            ),
            None => format!("<span class=\"page-ref\">{}</span>", escape(&title)),
        }
    }
}

/// The marks HTML shows on a run of text. Anything else is ignored and does not split a run.
#[derive(Debug, Default, PartialEq, Eq)]
struct Marks {
    /// The target of the link, when the mark has one that may be kept.
    link: Option<String>,
    /// `textColor`'s colour.
    color: Option<String>,
    /// `highlight`: present, and its colour when it has one.
    highlight: Option<Option<String>>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
}

impl Marks {
    fn of(marks: &[Value]) -> Self {
        let find = |name: &str| marks.iter().find(|mark| node_type(mark) == name);
        let color = |mark: &Value| str_attr(mark, "color").map(str::to_owned);
        Self {
            link: find("link")
                .and_then(|link| str_attr(link, "href"))
                .filter(|href| is_safe_link(href))
                .map(str::to_owned),
            color: find("textColor").and_then(color),
            highlight: find("highlight").map(color),
            bold: find("bold").is_some(),
            italic: find("italic").is_some(),
            underline: find("underline").is_some(),
            strike: find("strike").is_some(),
            code: find("code").is_some(),
        }
    }

    /// `text` (already escaped) inside the elements of the marks.
    fn wrap(&self, text: String) -> String {
        let mut out = text;
        let mut wrap = |tag: &str, enabled: bool| {
            if enabled {
                out = format!("<{tag}>{out}</{tag}>");
            }
        };
        wrap("code", self.code);
        wrap("s", self.strike);
        wrap("u", self.underline);
        wrap("em", self.italic);
        wrap("strong", self.bold);
        if let Some(highlight) = &self.highlight {
            out = match highlight {
                Some(color) => format!("<mark data-color=\"{}\">{out}</mark>", escape(color)),
                None => format!("<mark>{out}</mark>"),
            };
        }
        if let Some(color) = &self.color {
            out = format!(
                "<span class=\"text-color\" data-color=\"{}\">{out}</span>",
                escape(color)
            );
        }
        if let Some(href) = &self.link {
            out = format!("<a href=\"{}\">{out}</a>", escape(href));
        }
        out
    }
}

/// Writes the pending run (escaped text) wrapped in its marks, and clears it.
fn flush(out: &mut String, run: &mut String, marks: &Marks) {
    if !run.is_empty() {
        out.push_str(&marks.wrap(std::mem::take(run)));
    }
}

/// What a `personMention` reads as in plain text: `@label` (ADR-0049), as `kb-model` writes it in
/// Markdown: a line break or any other control character of the self-asserted label is a space,
/// and the empty string when the label is empty, blank or not a string. The account id is not
/// exported: it means nothing outside the workspace.
fn person_mention_text(node: &Value) -> String {
    let Some(label) = str_attr(node, "label") else {
        return String::new();
    };
    let label: String = label
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if label.trim().is_empty() {
        return String::new();
    }
    format!("@{label}")
}

/// A person mention as HTML (ADR-0049, M13 slice 2): `<span class="person-mention">@label</span>`,
/// the label escaped, no account id; nothing when the label is empty.
fn person_mention_html(node: &Value) -> String {
    let text = person_mention_text(node);
    if text.is_empty() {
        return String::new();
    }
    format!("<span class=\"person-mention\">{}</span>", escape(&text))
}

/// A date mention as a `<time>` element (nothing when it is not a valid mention).
fn date_html(node: &Value) -> String {
    let Some(date) = str_attr(node, "date").filter(|date| kb_model::valid_date(date)) else {
        return String::new();
    };
    let (datetime, text) = match str_attr(node, "time").filter(|time| valid_time(time)) {
        Some(time) => (format!("{date}T{time}"), format!("{date} {time}")),
        None => (date.to_owned(), date.to_owned()),
    };
    let reminder = attr(node, "reminder")
        .and_then(Value::as_i64)
        .filter(|minutes| *minutes >= 0)
        .map(|minutes| format!(" data-reminder=\"{minutes}\""))
        .unwrap_or_default();
    format!(
        "<time datetime=\"{}\"{reminder}>{}</time>",
        escape(&datetime),
        escape(&text)
    )
}

/// Whether `text` is exactly `HH:mm` (an hour from 00 to 23, a minute from 00 to 59).
fn valid_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    let [h1, h2, b':', m1, m2] = bytes else {
        return false;
    };
    let hour = h1.is_ascii_digit() && h2.is_ascii_digit() && (h1 - b'0') * 10 + (h2 - b'0') <= 23;
    let minute = m1.is_ascii_digit() && m2.is_ascii_digit() && (m1 - b'0') * 10 + (m2 - b'0') <= 59;
    hour && minute
}

// ----- helpers ---------------------------------------------------------------------------------

/// Escapes text for an element or a quoted attribute: `& < > " '`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Whether a `link` mark's target may become an `href`: `http`, `https`, `mailto` or `tel`, in any
/// case, with nothing (white space and control characters included) before the scheme.
fn is_safe_link(href: &str) -> bool {
    ["http:", "https:", "mailto:", "tel:"].iter().any(|scheme| {
        href.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    })
}

/// Whether a link the layout produced stays: `attachment://<id>`, or a relative path (no scheme
/// before the first `/`, `?` or `#`, not protocol-relative, no control character).
fn is_local(href: &str) -> bool {
    if href.starts_with(ATTACHMENT_SCHEME) {
        return !href.chars().any(char::is_control);
    }
    if href.starts_with("//") || href.starts_with('\\') || href.chars().any(char::is_control) {
        return false;
    }
    !matches!(href.find([':', '/', '?', '#']), Some(at) if href.as_bytes().get(at) == Some(&b':'))
}

fn is_inline(node: &Value) -> bool {
    matches!(
        node_type(node),
        "text" | "hardBreak" | "pageRef" | "dateMention" | "mathInline" | "personMention"
    )
}

fn node_type(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn attr<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    node.get("attrs")?.get(key)
}

/// A non-empty string attribute.
fn str_attr<'a>(node: &'a Value, key: &str) -> Option<&'a str> {
    attr(node, key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "html_tests.rs"]
mod tests;
