//! Markdown rendering of a page (`docs/development/milestone-03.md`, "Model").
//!
//! Rendered from the page's portable JSON, like the TypeScript mirror in
//! `core-client` (`markdown.ts`) renders it, so both walk the same normalised tree;
//! `fixtures/doc/page-v1.md` is their shared golden. The rules, literal and
//! deliberately simple (no escaping in v1):
//!
//! - Top-level blocks are joined by one blank line; trailing newlines are trimmed and
//!   exactly one is added (an empty page is `"\n"`). Consecutive inline nodes (text,
//!   `hardBreak`, `pageRef`, `dateMention`, `mathInline`, `personMention`) met among blocks render together as
//!   one block.
//! - Inline: consecutive text nodes with the same Markdown marks (link, bold, italic,
//!   strike, underline, code) are one run, and each run is wrapped in its marks,
//!   innermost first, so that the order from the outside is link `[...](href)` (empty
//!   `href` when missing), `**`, `*`, `~~`, `<u>...</u>`, backticks. Every other mark
//!   renders nothing and does not split a run: that is unknown marks, and the two colour
//!   marks `textColor` and `highlight` (added 2026-10-07, attribute `color`; Markdown has
//!   no colour, so coloured text exports as plain text, and a bold sentence with one
//!   red word is still one bold run). `hardBreak` is two spaces and a newline;
//!   `pageRef` is `[[Title]]`, or `[[<pageId as stored>]]` when `title_of` does not know
//!   the page (`blockId` ignored); `dateMention` is its ISO text and `mathInline` is
//!   `$latex$` (see below); other nodes render their inline content.
//! - `paragraph`: its inline content. `heading`: `#` x level (1 when the level is not
//!   an integer in 1..=6), a space, the text. `horizontalRule`: `---`. `codeBlock`:
//!   three backticks + language (empty when unset), newline, the text without marks,
//!   newline, three backticks.
//! - Lists: items joined by single newlines. An item (`listItem`/`taskItem`, or any
//!   other node, which stands for itself) is the marker followed by its first child's
//!   inline content when that child is a `paragraph`; its other child blocks follow on
//!   the next lines, indented by the marker width (the paragraph's own lines are not
//!   indented); empty lines stay empty. Markers: `- ` (`bulletList`), `N. ` counting
//!   from `start` (default 1, `orderedList`), `- [x] ` / `- [ ] ` from the item's
//!   `checked` (`taskList`, indent 2).
//! - `blockquote`: its blocks joined by blank lines, every line prefixed with `> `
//!   (empty lines become `>`). `callout`: the same after putting the icon (default
//!   `💡` when unset or empty) and a space before the first line. The callout's `color`
//!   (added 2026-10-06, a background name such as `blue`) is ignored: Markdown has no
//!   background, so a coloured callout exports exactly like an uncoloured one.
//! - `details`: `<details open>` or `<details>`, newline, `<summary>` + the first
//!   `detailsSummary`'s text without marks + `</summary>`, blank line, the blocks of
//!   `detailsContent` (and of any other child) joined by blank lines, blank line,
//!   `</details>`. A toggle heading (additive attribute `level` of the summary, 1 to 3,
//!   2026-10-14, ADR-0040; any other value reads as absent) puts the summary's text in a
//!   heading element, `<summary><h2>Title</h2></summary>` (an empty summary is
//!   `<summary><h2></h2></summary>`): the importers read the level back from it.
//! - `image` (Milestone 5): `![<alt>](attachment://<attachmentId>)`, both as stored
//!   (empty when unset); `width`, `widthPercent` and `align` are not rendered. With a non-empty
//!   `caption` (2026-10-09, ADR-0028) the line gains the CommonMark link title,
//!   `![<alt>](attachment://<attachmentId> "<caption>")`, in which `\` is written `\`, `"` is
//!   written `\"` and a line break (CRLF, LF or CR) is written as one space; without a
//!   caption (absent, empty or not a string) the line is as before. `file`:
//!   `[<name>](attachment://<attachmentId>)` where `<name>` is what `name_of` gives for
//!   the attachment, or the stored `attachmentId` when it gives none (or the id is
//!   malformed). Their children, if any, are ignored (they are leaves). `name_of` is
//!   asked once per distinct valid attachment id of the `file` nodes, in document
//!   order, before rendering.
//! - `gallery` (additive node, 2026-10-09, ADR-0030): one image line per item of `items`, in
//!   order, `![](attachment://<attachmentId>)` with the item's non-empty `caption` as the link
//!   title exactly as for an `image` (`![](attachment://<attachmentId> "<caption>")`), the lines
//!   joined by blank lines (so each reads back as an image block of its own). An item that is
//!   not a map or has no non-empty string `attachmentId` is skipped; the id is written as
//!   stored. A gallery without a list `items`, or without a usable item, is the empty string
//!   (the rule of the derived blocks above). `columns` is layout and is not written.
//! - `table` (additive node, 2026-10-05): a GFM table, one line per child row. A row
//!   (`tableRow`, in fact any child of the table) is `| cell | cell |`: `| `, its
//!   children (any name: `tableCell`, `tableHeader`) separated by ` | `, ` |`. A cell's
//!   text is its children rendered as blocks and joined by blank lines, then with every
//!   `|` escaped as `\|` and every newline replaced by `<br>` (a cell cannot span lines;
//!   no other escaping). After the first row comes the separator row `| --- | --- |`
//!   with as many `---` as that row has cells. Rows and separator are joined by single
//!   newlines; a table without rows is the empty string. `colspan`, `rowspan`,
//!   `colwidth` and `align` are ignored, and whether a cell is a header cell does not
//!   show (GFM's first row is the header).
//! - `mathBlock` (additive node, 2026-10-06): `$$`, newline, the text without marks,
//!   newline, `$$` (an empty block is `$$`, an empty line, `$$`; a multi-line source keeps
//!   its newlines). No escaping: the LaTeX is written as stored.
//! - `pageLink` (additive node, 2026-10-06): the card that links to a page renders like
//!   a `pageRef` standing alone on its own line, `[[Title]]`, or `[[<pageId as stored>]]`
//!   when `title_of` does not know the page; `blockId` is ignored. It is a block, so it
//!   is joined to its neighbours by blank lines like any block (a `pageRef` among text
//!   stays part of its paragraph). Importing `[[...]]` lines from Markdown is not part of
//!   this.
//! - `dateMention` (additive inline node, 2026-10-07): its ISO text, `2026-10-07`, or
//!   `2026-10-07 14:30` when it has a time: the `date` (`YYYY-MM-DD`, a floating local date),
//!   then a space and the `time` (`HH:mm`). Plain text, no brackets, no escaping: it never
//!   needs any, and importing Markdown does not read a date back (it stays text). A mention
//!   whose `date` is not a valid date renders as the empty string, and a malformed `time` is
//!   left out, so nothing but a date or a time ever reaches the export. It is inline: it
//!   stays inside its paragraph, joins the text around it, and in a toggle's summary or any
//!   other plain-text context it is the same text. Its `reminder` attribute (minutes before
//!   the moment, ADR-0024) is not rendered: a reminder is a device's business, not part of the
//!   text, so the export is the same with or without one.
//! - `mathInline` (additive inline node, 2026-10-07): `$`, the `latex` source exactly as stored,
//!   `$`. No escaping and no trimming: the LaTeX is written as stored, so a source that holds a
//!   `$` (or starts or ends with a space) does not read back as one equation (the parser in
//!   `core-client` takes `$...$` on one line with no `$` inside and no space right after the
//!   opening or before the closing `$`). An equation whose `latex` is empty or not a string
//!   renders as the empty string (`$$` is the math block's fence, so it is never written). It is
//!   inline: it stays inside its paragraph and joins the text around it.
//! - `personMention` (additive inline node, M13 slice 2, ADR-0049): `@`, then its `label`, the name
//!   when the mention was inserted. The account id is never written. A mention whose `label` is
//!   empty (or only white space) or not a string renders as the empty string, and a control character
//!   in the label (a line break included) is written as a space, so a self-asserted name cannot end the
//!   paragraph. The label is text: the file flavour escapes it like any text run, the plain flavour
//!   writes it as it is, no escaping, as it does for the other inline nodes. It is inline: it stays
//!   inside its paragraph and joins the text around it, in a toggle's summary or any other
//!   plain-text context it is the same text. Importing Markdown does not read a mention back (it
//!   stays text).
//! - `columnList` (additive node, 2026-10-06): its columns one after another, linearised
//!   (Markdown has no side by side, and nothing marks where a column starts): each
//!   `column` is its blocks joined by blank lines, and the columns are joined by blank
//!   lines too. That is the rule for any other node below, written down here because the
//!   layout depends on it; there is no special case in the code. An empty column (no
//!   blocks, or one empty paragraph) renders as the empty string and still takes its
//!   place in the join, as an empty top-level paragraph does: the blank lines pile up
//!   where it was, and at the very end of the page they are trimmed. The number of
//!   columns and the nesting are not looked at.
//! - `tableOfContents` and `breadcrumb` (additive nodes, 2026-10-06): the empty string.
//!   Both are leaf blocks whose content the editor derives (the page's headings, the page's
//!   place in the tree), so the document stores nothing to export. That is the rule for any
//!   other node below (a leaf has no children to render), written down here because the
//!   blank lines depend on it; there is no special case in the code. The empty block takes
//!   its place in the join, as an empty top-level paragraph and an empty column do, and
//!   each one adds two newlines: between two blocks it leaves three blank lines instead of
//!   one (two derived blocks in a row leave five), as the first block of the page it
//!   leaves two empty lines before the first line (leading newlines are not trimmed), and
//!   as the last blocks of the page it vanishes with the trailing newlines.
//! - `pageList` (additive node, 2026-10-07): the empty string too, by the same rule and with
//!   the same blank lines. The list view shows a set of pages (the children of the page that
//!   holds it, or the pages that carry a tag) that the app derives at run time from the tree
//!   and the property values; the document stores only the configuration of the view
//!   (`source`, `layout`, `properties`, `sort`). A line per page, `- [[Title]]`, is therefore
//!   not possible in the core (it would be a snapshot of what the view happened to show, stale
//!   in the file as soon as a page moves or is renamed), so the export writes nothing for it.
//! - `whiteboard` (additive node, 2026-10-10, ADR-0033): a block quote that starts with the marker
//!   paragraph `> **Whiteboard**`, then a blank quote line and one list item per live text element of the drawing, in
//!   z-order (`> - <text>`, whitespace collapsed to single spaces, empty texts skipped). The
//!   elements come from the top-level `whiteboards` value of the portable JSON; a whiteboard
//!   without text is the marker line alone. No image of the drawing is written. Read back, it
//!   is an ordinary block quote.
//! - `templateButton` (additive node, ADR-0056): the empty string, with the same blank lines as the derived blocks
//!   above. The button holds template material (blocks that a click copies into the page): its content is not part
//!   of the page's text, so the clipboard flavour writes none of it. The file flavour keeps it (below).
//! - `dateMention` and `personMention` render as before whatever their `relative` attribute (ADR-0056): the stored
//!   `date`, `time` or `label` are the text; "filled in when created" is not text.
//! - Any other node: its children rendered as blocks and joined by blank lines.
//!
//! # File flavour (ADR-0036, `docs/architecture/import-export.md`, "Markdown file flavour")
//!
//! [`render_file`] writes a page to a `.md` file that other tools read and that atomik-kb
//! reads back. It is every rule above with **these differences, and only these**; the
//! clipboard flavour (`render`) does not change by one byte. What it needs from the exporter
//! (links, titles, names) it asks through [`ExportLinks`]; the front matter is not part of it
//! (`kb-export` adds it).
//!
//! 1. **Escaping.** In a text run (not in a `code` mark, a code block, a math node, a link
//!    destination or an annotation) every `\`, `` ` ``, `*`, `_`, `[`, `]`, `<`, `>`, `$`, `~`, `|`
//!    and `&` is preceded by `\`; at the start of a line of a block (after a list marker, a quote
//!    prefix or a hard break; not in a heading, not in a table cell, not when a mark's
//!    characters come first) a `#`, `+`, `-` or `=` is too, and so is the `.` or `)` of a leading
//!    run of digits (`1\. not a list`); the `#`s at the end of a heading's text are, so that they
//!    are not read as a closing sequence; and four columns or more of white space at the start of
//!    a line (an indented code block) begin with a character reference for their first character
//!    (`&#32;`, `&#9;`). The alt text of an image uses the caption escaping
//!    (`\` and `"`, line breaks as spaces) and also escapes `[` and `]`; a file's name and a
//!    page link's title are escaped as text; the texts of a whiteboard are escaped as text. A
//!    code span uses as many backticks as it needs (one more than the longest run inside, and a
//!    space inside them when the code starts or ends with a backtick); a fenced code block a
//!    fence longer than any run of backticks in it and an info string without white space or
//!    backticks. A link destination that holds white space, a control character, `<`, `>` or
//!    unbalanced parentheses is written in angle brackets (`[x](<a b>)`). In a table cell every
//!    `|` that is not escaped yet is (code, math and destinations included).
//! 2. **Links.** A `pageRef` whose page has `page_href` is `[Title](href)` (the title escaped as
//!    text; the stored id when the title is unknown), otherwise `[[Title]]` as before. A
//!    `pageLink` is the same, alone as a block. An image is `![alt](href "caption")`, a file
//!    `[name](href)` with the name from `attachment_name` (the stored id when there is none);
//!    without an `attachment_href` the destination stays `attachment://<id>`.
//! 3. **Dates.** A `dateMention` is `<time datetime="2026-10-07">2026-10-07</time>`, with a time
//!    `<time datetime="2026-10-07T14:30">2026-10-07 14:30</time>`, with a reminder (a
//!    non-negative integer, minutes as stored) also ` data-reminder="15"` after `datetime`, and with a
//!    `relative` of `today` or `now` (ADR-0056) also ` data-relative="today"` after that. An
//!    invalid mention stays the empty string. In the summary of a toggle it is the same element.
//! 4. **Annotations.** An annotation is one line, `<!-- atomik:<json> -->`
//!    ([`ANNOTATION_OPEN`], [`ANNOTATION_CLOSE`]), compact JSON with the keys of every object
//!    sorted (so `attrs` before `node`) and every `--` written with its second hyphen as the JSON
//!    escape `-\u002d`, so that the annotation cannot close its own comment. It is a block of its
//!    own, joined to the next block by a blank line (inside a quote it carries the `> ` prefix, in
//!    a list item it is indented like the item's other blocks), and is not written inside table
//!    cells. Attributes are written when set (a number or a non-empty string) and `attrs` is left
//!    out when it would be empty (a `pageList` always has it: a reader takes one without it for a
//!    damaged annotation). Forms:
//!    - before an `image`: `{"attrs":{"align":"right","width":640,"widthPercent":60}}`
//!      (`align`, `width` (legacy pixels) and `widthPercent`), before a `file`:
//!      `{"attrs":{"view":"preview"}}`; no annotation when none is set;
//!    - before every callout (the callout stays the quote that starts with its icon):
//!      `{"attrs":{"color":"blue","icon":"💡"},"node":"callout"}` (`icon` always, the default
//!      when unset; `color` when set);
//!    - before a gallery's image lines: `{"attrs":{"columns":3},"items":3,"node":"gallery"}`
//!      (`items` the number of image lines written; a gallery without a usable item is
//!      nothing, as in the clipboard flavour);
//!    - before a whiteboard's quote (which stays):
//!      `{"attrs":{"height":360},"file":"<whiteboard_href>","node":"whiteboard"}` (`file` only
//!      when the exporter has one);
//!    - around columns: `{"node":"columnList"}`, then `{"node":"column"}` before each column's
//!      blocks, then `{"end":"columnList"}` after the last column; the columns stay linearised
//!      between them (an empty column is its annotation alone);
//!    - around a template button's content (ADR-0056): `{"attrs":{"label":"Add task","position":"above"},"node":"templateButton"}`
//!      (`label` when it is a non-empty string, `position` when it is `below` or `above`; `attrs` left out when
//!      neither is set), then the content's blocks (each as it is written in this flavour, annotations included),
//!      then `{"end":"templateButton"}`. The clipboard flavour writes nothing for the button; inside a table cell,
//!      where annotations are not written, neither does the file flavour;
//!    - **instead of** the empty string of the clipboard flavour: `{"node":"tableOfContents"}`,
//!      `{"node":"breadcrumb"}` and `{"attrs":{...},"node":"pageList"}`. In a `pageList`'s
//!      attributes every id of the space's schema is a name: `properties` property names;
//!      `sort.by` a property name (`title`, `updated`, `created` stay); `source` is
//!      `{"kind":"tag","option":"<option name of the Tags property>"}` (a `children` source, the
//!      `{"kind":"page","pageId"}` and `{"kind":"space"}` sources of ADR-0058 and kinds this version
//!      does not know stay as stored); each filter's `propertyId` becomes
//!      `property` and the option ids of a `hasOption` or `lacksOption` value option names (of that
//!      property; a `relatesTo` or `notRelatesTo` value is a page id and a relative date value stays
//!      as stored); a view's `sorts` (ADR-0058) is the list of its keys written like `sort`, a key
//!      without a name dropped and the attribute with them when none is left;
//!      `groupBy` a property name; `hiddenGroups` option names of the `groupBy` property
//!      (`none` stays); `columnWidths` keys property names (`title` stays); `calculations`
//!      (2026-10-14) keys property names (`title` stays) with their function names as stored,
//!      and the attribute is left out when nothing is left of it. `views` (2026-10-16, ADR-0054)
//!      is the list of the block's views as [`views::read_views`] reads them (at most 20), each
//!      written like the block's own attributes with names for ids, plus its `id`, `name`,
//!      `openIn` and `cardSize` as stored, `dateBy` (a property name; `created` and `updated`
//!      stay) and `endBy` (a property name), and its `filter` tree `{"op":"and"|"or","items":[...]}`
//!      whose filters are written like the flat `filters` (a nested group left with no filter
//!      after its filters were dropped is dropped; one that was empty stays); a view left with
//!      nothing to say is still there with its `id`, `name`, `layout` and `properties`, and keys a
//!      newer version added stay as stored. An id without a
//!      name is dropped (a filter, a `sort` or a `source` that cannot be named is dropped
//!      whole). `id` and null attributes are never written; other attributes stay as stored.
//! 5. Text colour and highlight stay plain text, as in the clipboard flavour.
//!
//! Two more places are HTML, not Markdown, and so are escaped as HTML: the summary of a
//! toggle writes `&`, `<` and `>` as `&amp;`, `&lt;` and `&gt;` (its text is read back from the
//! HTML block, where backslash escapes are not processed). Everything inside an annotation's
//! JSON is JSON.

use std::collections::HashMap;

use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{AttachmentId, BlockId, OptionId, PageId, PropertyId};
use serde_json::{Map, Value, json};

use super::views;

/// Icon of a callout without one (the schema default).
const DEFAULT_CALLOUT_ICON: &str = "💡";

/// Scheme of attachment links in exported Markdown.
const ATTACHMENT_SCHEME: &str = "attachment://";

/// Renders portable page JSON.
pub(super) fn render(
    page: &Value,
    title_of: &dyn Fn(PageId) -> Option<String>,
    name_of: &mut dyn FnMut(AttachmentId) -> Option<String>,
) -> String {
    let names = file_names(page, name_of);
    let renderer = Renderer {
        whiteboards: page.get("whiteboards").unwrap_or(&Value::Null),
        title_of,
        names: &names,
        links: None,
        annotate: false,
        in_cell: false,
    };
    finish(&renderer, page)
}

/// The text of a page: its blocks, trailing newlines trimmed and exactly one added.
fn finish(renderer: &Renderer<'_>, page: &Value) -> String {
    let mut out = if node_type(page) == "doc" {
        renderer.blocks(content(page))
    } else {
        renderer.block(page)
    };
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

/// Names of the attachments of the `file` nodes under `page`: `name_of` asked once per
/// distinct valid id, in document order; ids it does not name are left out.
fn file_names(
    page: &Value,
    name_of: &mut dyn FnMut(AttachmentId) -> Option<String>,
) -> HashMap<AttachmentId, String> {
    let mut asked: HashMap<AttachmentId, Option<String>> = HashMap::new();
    let mut stack = vec![page];
    while let Some(node) = stack.pop() {
        if node_type(node) == "file"
            && let Some(id) = attachment_id(node).and_then(|id| id.parse::<AttachmentId>().ok())
            && !asked.contains_key(&id)
        {
            asked.insert(id, name_of(id));
        }
        stack.extend(content(node).iter().rev());
    }
    asked
        .into_iter()
        .filter_map(|(id, name)| Some((id, name?)))
        .collect()
}

/// The `attachmentId` of an `image` or `file` node, as stored (`None` when unset or
/// empty).
fn attachment_id(node: &Value) -> Option<&str> {
    string_attr(node, "attachmentId")
}

fn node_type(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn content(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn attr<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    node.get("attrs")?.get(key)
}

/// A non-empty string attribute.
fn string_attr<'a>(node: &'a Value, key: &str) -> Option<&'a str> {
    attr(node, key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// `text` as the body of a double-quoted CommonMark link title: `\` and `"` are escaped with
/// a backslash and every line break (CRLF, LF or CR) becomes one space.
fn link_title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(' ');
            }
            '\n' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

/// A `gallery` as one image line per usable item, joined by blank lines.
fn gallery(node: &Value) -> String {
    let items = attr(node, "items")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    items
        .iter()
        .filter_map(|item| {
            let id = item
                .get("attachmentId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            let caption = item
                .get("caption")
                .and_then(Value::as_str)
                .filter(|caption| !caption.is_empty());
            Some(match caption {
                Some(caption) => {
                    format!("![]({ATTACHMENT_SCHEME}{id} \"{}\")", link_title(caption))
                }
                None => format!("![]({ATTACHMENT_SCHEME}{id})"),
            })
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn integer_attr(node: &Value, key: &str) -> Option<i64> {
    let value = attr(node, key)?;
    value
        .as_i64()
        .or_else(|| value.as_f64().and_then(super::read::integral))
}

/// What a `dateMention` renders as: its ISO text, the empty string when its `date` is not a
/// valid date (see `super::date`).
fn date_text(node: &Value) -> String {
    super::date::iso_text(string_attr(node, "date"), string_attr(node, "time")).unwrap_or_default()
}

/// What a `mathInline` renders as: `$latex$`, the empty string when its `latex` is empty or not
/// a string.
fn math_inline_text(node: &Value) -> String {
    string_attr(node, "latex")
        .map(|latex| format!("${latex}$"))
        .unwrap_or_default()
}

/// What a `personMention` renders as: `@label` (ADR-0049), the empty string when its `label` is
/// empty or not a string. The account id is never written. A name is self-asserted, so a line break
/// or any other control character in the label is written as a space: the mention stays on its line
/// and cannot open a block of the exported file.
fn person_mention_text(node: &Value) -> String {
    let Some(label) = string_attr(node, "label") else {
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

fn is_inline(node: &Value) -> bool {
    matches!(
        node_type(node),
        "text" | "hardBreak" | "pageRef" | "dateMention" | "mathInline" | "personMention"
    )
}

/// Prefixes every line with `> ` (empty lines become `>`).
fn quote(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Indents every non-empty line by `pad`.
fn indent_lines(text: &str, pad: &str) -> String {
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One line of a table: `| ` + the cells separated by ` | ` + ` |`.
fn table_line(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

/// The text of a table cell as one line: `|` escaped as `\|`, newlines as `<br>`.
fn table_cell_text(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br>")
}

/// The marks Markdown can express on a run of text. Any other mark (an unknown one,
/// `textColor`, `highlight`) is not part of it, so it neither shows nor splits a run.
#[derive(Debug, Default, PartialEq, Eq)]
struct Style<'a> {
    /// The target of the link (empty when the mark has none); `None` without a link.
    link: Option<&'a str>,
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    code: bool,
}

impl<'a> Style<'a> {
    fn of(marks: &'a [Value]) -> Self {
        let find = |name: &str| marks.iter().find(|mark| node_type(mark) == name);
        Self {
            link: find("link").map(|link| {
                attr(link, "href")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            }),
            bold: find("bold").is_some(),
            italic: find("italic").is_some(),
            strike: find("strike").is_some(),
            underline: find("underline").is_some(),
            code: find("code").is_some(),
        }
    }

    /// Wraps `text` in the marks, innermost first.
    fn wrap(&self, text: &str) -> String {
        let mut out = text.to_owned();
        if self.code {
            out = format!("`{out}`");
        }
        self.wrap_outer(out, false)
    }

    /// File flavour: [`Self::wrap`] with the text escaped (not for code, which gets a fence
    /// longer than any run of backticks it holds) and a link destination that survives
    /// white space. `line_start`: the text starts a line of the block.
    fn wrap_file(&self, text: &str, line_start: bool) -> String {
        let inner = if self.code {
            code_span(text)
        } else {
            // Wrapped text does not start the line: the mark's characters do.
            escape_text(text, line_start && !self.wraps())
        };
        self.wrap_outer(inner, true)
    }

    /// Whether any mark writes characters around the text.
    fn wraps(&self) -> bool {
        self.link.is_some() || self.bold || self.italic || self.strike || self.underline
    }

    /// The marks around `out` (the code span, if any, is already in it), innermost first.
    fn wrap_outer(&self, mut out: String, file: bool) -> String {
        if self.underline {
            out = format!("<u>{out}</u>");
        }
        if self.strike {
            out = format!("~~{out}~~");
        }
        if self.italic {
            out = format!("*{out}*");
        }
        if self.bold {
            out = format!("**{out}**");
        }
        if let Some(href) = self.link {
            if file {
                out = format!("[{out}]({})", link_destination(href));
            } else {
                out = format!("[{out}]({href})");
            }
        }
        out
    }
}

struct Renderer<'f> {
    /// The top-level `whiteboards` value (`Null` when the page has none).
    whiteboards: &'f Value,
    title_of: &'f dyn Fn(PageId) -> Option<String>,
    /// Names of the attachments of `file` nodes (see [`file_names`]).
    names: &'f HashMap<AttachmentId, String>,
    /// What the file flavour asks the exporter; `None` is the clipboard flavour.
    links: Option<&'f dyn ExportLinks>,
    /// File flavour: annotations are written (not inside a table cell).
    annotate: bool,
    /// File flavour: the text is part of a table cell (one line, no block syntax, pipes escaped).
    in_cell: bool,
}

impl Renderer<'_> {
    fn blocks(&self, nodes: &[Value]) -> String {
        self.chunks(nodes).join("\n\n")
    }

    /// One rendered chunk per block; consecutive inline nodes form one chunk.
    fn chunks(&self, nodes: &[Value]) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut inline: Vec<&Value> = Vec::new();
        for node in nodes {
            if is_inline(node) {
                inline.push(node);
            } else {
                if !inline.is_empty() {
                    chunks.push(self.inline(&std::mem::take(&mut inline), true));
                }
                chunks.push(self.block(node));
            }
        }
        if !inline.is_empty() {
            chunks.push(self.inline(&inline, true));
        }
        chunks
    }

    fn block(&self, node: &Value) -> String {
        if let Some(links) = self.links
            && let Some(text) = self.file_block(links, node)
        {
            return text;
        }
        match node_type(node) {
            "paragraph" => self.inline_of(node, true),
            "heading" => {
                let level = integer_attr(node, "level")
                    .filter(|level| (1..=6).contains(level))
                    .and_then(|level| usize::try_from(level).ok())
                    .unwrap_or(1);
                // The text follows the `#`s: it never starts a line of its own.
                let text = self.inline_of_at(node, true, false);
                let text = if self.links.is_some() {
                    escape_closing_hashes(&text)
                } else {
                    text
                };
                format!("{} {text}", "#".repeat(level))
            }
            "bulletList" => self.list(node, |_, _| "- ".to_owned(), str::len),
            "orderedList" => {
                let start = integer_attr(node, "start").unwrap_or(1);
                self.list(
                    node,
                    |index, _| {
                        let offset = i64::try_from(index).unwrap_or(i64::MAX);
                        format!("{}. ", start.saturating_add(offset))
                    },
                    str::len,
                )
            }
            "taskList" => self.list(
                node,
                |_, item| {
                    if attr(item, "checked") == Some(&Value::Bool(true)) {
                        "- [x] ".to_owned()
                    } else {
                        "- [ ] ".to_owned()
                    }
                },
                |_| 2,
            ),
            "blockquote" => quote(&self.blocks(content(node))),
            "callout" => {
                let icon = string_attr(node, "icon").unwrap_or(DEFAULT_CALLOUT_ICON);
                quote(&format!("{icon} {}", self.blocks(content(node))))
            }
            "codeBlock" => format!(
                "```{}\n{}\n```",
                string_attr(node, "language").unwrap_or_default(),
                self.inline_of(node, false)
            ),
            "mathBlock" => format!("$$\n{}\n$$", self.inline_of(node, false)),
            "horizontalRule" => "---".to_owned(),
            "details" => self.details(node),
            "image" => {
                let alt = attr(node, "alt")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let id = attachment_id(node).unwrap_or_default();
                match string_attr(node, "caption") {
                    Some(caption) => {
                        format!(
                            "![{alt}]({ATTACHMENT_SCHEME}{id} \"{}\")",
                            link_title(caption)
                        )
                    }
                    None => format!("![{alt}]({ATTACHMENT_SCHEME}{id})"),
                }
            }
            "file" => {
                let id = attachment_id(node).unwrap_or_default();
                let name = id
                    .parse::<AttachmentId>()
                    .ok()
                    .and_then(|parsed| self.names.get(&parsed))
                    .map_or(id, String::as_str);
                format!("[{name}]({ATTACHMENT_SCHEME}{id})")
            }
            "gallery" => gallery(node),
            "whiteboard" => self.whiteboard(node),
            "table" => self.table(node),
            "pageLink" => self.page_ref(node),
            // Template material (ADR-0056): the page's text has none of it.
            "templateButton" => String::new(),
            _ if is_inline(node) => self.inline(&[node], true),
            _ => self.blocks(content(node)),
        }
    }

    /// A `whiteboard` as a quote of its text elements.
    fn whiteboard(&self, node: &Value) -> String {
        let texts = string_attr(node, "id")
            .and_then(|id| self.whiteboards.get(id))
            .and_then(Value::as_object)
            .map(super::whiteboard::texts_of_portable)
            .unwrap_or_default();
        let mut blocks = vec!["**Whiteboard**".to_owned()];
        if !texts.is_empty() {
            let items: Vec<String> = texts
                .into_iter()
                .map(|text| {
                    if self.links.is_some() {
                        format!("- {}", escape_text(&text, true))
                    } else {
                        format!("- {text}")
                    }
                })
                .collect();
            blocks.push(items.join("\n"));
        }
        quote(&blocks.join("\n\n"))
    }

    /// A GFM table: `| a | b |` per row, the `| --- | --- |` separator after the first.
    fn table(&self, table: &Value) -> String {
        // A cell is one line of inline syntax: no annotations, no block escapes, pipes escaped.
        let cell = Renderer {
            annotate: false,
            in_cell: self.links.is_some(),
            ..*self
        };
        let mut lines = Vec::new();
        for (index, row) in content(table).iter().enumerate() {
            let cells: Vec<String> = content(row)
                .iter()
                .map(|node| cell.cell_text(&cell.blocks(content(node))))
                .collect();
            lines.push(table_line(&cells));
            if index == 0 {
                lines.push(table_line(&vec!["---".to_owned(); cells.len()]));
            }
        }
        lines.join("\n")
    }

    /// The rendered blocks of a table cell as one line.
    fn cell_text(&self, text: &str) -> String {
        if self.in_cell {
            escape_pipes(&text.replace('\n', "<br>"))
        } else {
            table_cell_text(text)
        }
    }

    /// Items joined by single newlines; `marker(index, item)` and `indent(marker)`.
    fn list(
        &self,
        list: &Value,
        marker: impl Fn(usize, &Value) -> String,
        indent: impl Fn(&str) -> usize,
    ) -> String {
        content(list)
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let prefix = marker(index, item);
                // A list item wraps its blocks; anything else in a list stands for itself.
                let body = if matches!(node_type(item), "listItem" | "taskItem") {
                    content(item)
                } else {
                    std::slice::from_ref(item)
                };
                let (lead, rest) = match body.split_first() {
                    Some((first, rest)) if node_type(first) == "paragraph" => {
                        (self.inline_of(first, true), rest)
                    }
                    _ => (String::new(), body),
                };
                let pad = " ".repeat(indent(&prefix));
                let mut lines = vec![format!("{prefix}{lead}")];
                lines.extend(
                    self.chunks(rest)
                        .iter()
                        .map(|chunk| indent_lines(chunk, &pad)),
                );
                lines.join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn details(&self, node: &Value) -> String {
        let children = content(node);
        let summary = children
            .iter()
            .position(|child| node_type(child) == "detailsSummary");
        let mut body = Vec::new();
        for (index, child) in children.iter().enumerate() {
            if Some(index) == summary {
                continue;
            }
            if node_type(child) == "detailsContent" {
                body.extend(self.chunks(content(child)));
            } else {
                body.extend(self.chunks(std::slice::from_ref(child)));
            }
        }
        let summary = summary
            .and_then(|index| children.get(index))
            .map(|summary| {
                let text = if self.links.is_some() {
                    self.summary_html(summary)
                } else {
                    self.inline_of(summary, false)
                };
                // A toggle heading (2026-10-14, ADR-0040): the text sits in `<h1>` to `<h3>`.
                match integer_attr(summary, "level").filter(|level| (1..=3).contains(level)) {
                    Some(level) => format!("<h{level}>{text}</h{level}>"),
                    None => text,
                }
            })
            .unwrap_or_default();
        let open = if attr(node, "open") == Some(&Value::Bool(true)) {
            " open"
        } else {
            ""
        };
        format!(
            "<details{open}>\n<summary>{summary}</summary>\n\n{}\n\n</details>",
            body.join("\n\n")
        )
    }

    fn inline_of(&self, node: &Value, with_marks: bool) -> String {
        self.inline_of_at(node, with_marks, true)
    }

    /// [`Self::inline_of`]; `line_start`: the first character is the start of a line of the
    /// block (the file flavour escapes what would read as block syntax there).
    fn inline_of_at(&self, node: &Value, with_marks: bool, line_start: bool) -> String {
        let nodes: Vec<&Value> = content(node).iter().collect();
        self.inline_from(&nodes, with_marks, line_start)
    }

    fn inline(&self, nodes: &[&Value], with_marks: bool) -> String {
        self.inline_from(nodes, with_marks, true)
    }

    fn inline_from(&self, nodes: &[&Value], with_marks: bool, line_start: bool) -> String {
        let file = self.links.is_some();
        let mut out = String::new();
        // The run of text nodes with one Markdown style that is being collected.
        let mut run = String::new();
        let mut style = Style::default();
        for node in nodes {
            if node_type(node) == "text" {
                let text = node.get("text").and_then(Value::as_str).unwrap_or_default();
                if with_marks {
                    let marks = node
                        .get("marks")
                        .and_then(Value::as_array)
                        .map_or(&[][..], Vec::as_slice);
                    let next = Style::of(marks);
                    if next != style {
                        self.flush_run(&mut out, &mut run, &style, line_start);
                        style = next;
                    }
                    run.push_str(text);
                } else {
                    out.push_str(text);
                }
                continue;
            }
            self.flush_run(&mut out, &mut run, &style, line_start);
            match node_type(node) {
                "hardBreak" => out.push_str("  \n"),
                "pageRef" => out.push_str(&self.page_ref(node)),
                "dateMention" if file => out.push_str(&date_element(node)),
                "dateMention" => out.push_str(&date_text(node)),
                "mathInline" => out.push_str(&math_inline_text(node)),
                // The file flavour writes text that is escaped; the label is text.
                "personMention" if file => {
                    out.push_str(&escape_text(&person_mention_text(node), false));
                }
                "personMention" => out.push_str(&person_mention_text(node)),
                _ => out.push_str(&self.inline_of_at(
                    node,
                    with_marks,
                    starts_line(&out, line_start),
                )),
            }
        }
        self.flush_run(&mut out, &mut run, &style, line_start);
        out
    }

    /// Writes the pending run of text, wrapped in its style, and clears it. `line_start`: the
    /// text being built (`out`) started a line of the block.
    fn flush_run(&self, out: &mut String, run: &mut String, style: &Style<'_>, line_start: bool) {
        if run.is_empty() {
            return;
        }
        if self.links.is_some() {
            // A table cell has no block syntax, so nothing starts a line there.
            let starts = !self.in_cell && starts_line(out, line_start);
            out.push_str(&style.wrap_file(run, starts));
        } else {
            out.push_str(&style.wrap(run));
        }
        run.clear();
    }

    /// `[[Title]]`, or `[[<pageId as stored>]]` when the title is unknown: a `pageRef`
    /// in a line of text, or a `pageLink` block on a line of its own. The file flavour writes
    /// `[Title](href)` for a page of the export.
    fn page_ref(&self, node: &Value) -> String {
        let stored = string_attr(node, "pageId").unwrap_or_default();
        let page = stored.parse::<PageId>().ok();
        let title = page.and_then(|id| (self.title_of)(id));
        if let Some(links) = self.links
            && let Some(href) = page.and_then(|id| links.page_href(id))
        {
            let text = escape_text(title.as_deref().unwrap_or(stored), false);
            return format!("[{text}]({})", link_destination(&href));
        }
        format!("[[{}]]", title.as_deref().unwrap_or(stored))
    }

    /// The summary of a toggle in the file flavour: it sits in an HTML block, so its text is
    /// HTML (`&`, `<` and `>` as entities), not Markdown with backslash escapes.
    fn summary_html(&self, node: &Value) -> String {
        let mut out = String::new();
        for child in content(node) {
            match node_type(child) {
                "text" => out.push_str(&escape_html(
                    child
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )),
                "hardBreak" => out.push_str("  \n"),
                "pageRef" => out.push_str(&escape_html(&self.page_ref(child))),
                "dateMention" => out.push_str(&date_element(child)),
                "mathInline" => out.push_str(&escape_html(&math_inline_text(child))),
                "personMention" => out.push_str(&escape_html(&person_mention_text(child))),
                _ => out.push_str(&self.summary_html(child)),
            }
        }
        out
    }
}

// ----- File flavour (ADR-0036, `docs/architecture/import-export.md`, "Markdown file flavour") -----

/// Opening of an annotation: an HTML comment on its own line that carries what Markdown cannot
/// hold, as compact JSON (`<!-- atomik:{"node":"pageList","attrs":{...}} -->`). Written by the
/// file flavour, read back by `kb-import`.
pub const ANNOTATION_OPEN: &str = "<!-- atomik:";

/// Closing of an annotation.
pub const ANNOTATION_CLOSE: &str = " -->";

/// What the file flavour of the renderer asks the exporter. Every link it answers is relative to
/// the file being written, `/`-separated and percent-encoded where Markdown needs it.
pub trait ExportLinks {
    /// Link to another page's exported file, when that page is part of the export (`None`:
    /// the page is outside the export and renders as `[[Title]]`).
    fn page_href(&self, page: PageId) -> Option<String>;
    /// Title of any page of the workspace (`None`: unknown page).
    fn page_title(&self, page: PageId) -> Option<String>;
    /// Link to an attachment's exported file (`None`: not in the archive; the link stays
    /// `attachment://<id>`).
    fn attachment_href(&self, attachment: AttachmentId) -> Option<String>;
    /// Display name of an attachment (file blocks).
    fn attachment_name(&self, attachment: AttachmentId) -> Option<String>;
    /// Link to the `.excalidraw` file of one of this page's whiteboard blocks.
    fn whiteboard_href(&self, block: BlockId) -> Option<String>;
    /// Name of a property of the page's space (ids in `pageList` annotations become names).
    fn property_name(&self, property: PropertyId) -> Option<String>;
    /// Name of an option of a property of the page's space.
    fn option_name(&self, property: PropertyId, option: OptionId) -> Option<String>;
}

/// Renders portable page JSON in the file flavour: the clipboard rules above with escaped text,
/// relative links, `<time>` date mentions and annotations (no front matter: `kb-export` adds it).
/// The rules are the "File flavour" section of this file's header.
#[must_use]
pub fn render_file(page: &Value, links: &dyn ExportLinks) -> String {
    let title_of = |id: PageId| links.page_title(id);
    let names = file_names(page, &mut |id| links.attachment_name(id));
    let renderer = Renderer {
        whiteboards: page.get("whiteboards").unwrap_or(&Value::Null),
        title_of: &title_of,
        names: &names,
        links: Some(links),
        annotate: true,
        in_cell: false,
    };
    finish(&renderer, page)
}

/// Blocks and annotations of the file flavour.
impl Renderer<'_> {
    /// The blocks whose file flavour differs from the clipboard rules (the rest are shared and
    /// reach the file rules through the inline renderer); `None` for those.
    fn file_block(&self, links: &dyn ExportLinks, node: &Value) -> Option<String> {
        Some(match node_type(node) {
            "image" => self.image_file(links, node),
            "file" => self.attachment_file(links, node),
            "gallery" => self.gallery_file(links, node),
            "callout" => self.callout_file(node),
            "codeBlock" => self.code_block_file(node),
            "whiteboard" => self.whiteboard_file(links, node),
            "columnList" => {
                let columns = self.blocks(content(node));
                if self.annotate {
                    [
                        annotation(&json!({ "node": "columnList" })),
                        columns,
                        annotation(&json!({ "end": "columnList" })),
                    ]
                    .join("\n\n")
                } else {
                    columns
                }
            }
            "column" => {
                let blocks = self.blocks(content(node));
                match (self.annotate, blocks.is_empty()) {
                    (false, _) => blocks,
                    (true, true) => annotation(&json!({ "node": "column" })),
                    (true, false) => {
                        format!("{}\n\n{blocks}", annotation(&json!({ "node": "column" })))
                    }
                }
            }
            "templateButton" => self.template_button_file(node),
            "tableOfContents" => self.derived(json!({ "node": "tableOfContents" })),
            "breadcrumb" => self.derived(json!({ "node": "breadcrumb" })),
            // `attrs` is always there, even when empty: a reader takes a `pageList` without it
            // for a damaged annotation.
            "pageList" => self.derived(
                json!({ "node": "pageList", "attrs": Value::Object(page_list_attrs(links, node)) }),
            ),
            _ => return None,
        })
    }

    /// `body` preceded, as a block of its own, by the annotation `note` (when annotating and
    /// there is one).
    fn noted(&self, note: Option<Value>, body: String) -> String {
        match note {
            Some(note) if self.annotate => format!("{}\n\n{body}", annotation(&note)),
            _ => body,
        }
    }

    /// A template button (ADR-0056): its opening annotation (label and position), the blocks it holds, its
    /// closing annotation. The empty string where annotations are not written (a table cell): the content is
    /// not part of the page's text.
    fn template_button_file(&self, node: &Value) -> String {
        if !self.annotate {
            return String::new();
        }
        let mut attrs = Map::new();
        if let Some(label) = string_attr(node, "label") {
            attrs.insert("label".to_owned(), json!(label));
        }
        if let Some(position) =
            string_attr(node, "position").filter(|position| matches!(*position, "below" | "above"))
        {
            attrs.insert("position".to_owned(), json!(position));
        }
        let mut parts = vec![annotation(&note("templateButton", attrs, Map::new()))];
        parts.extend(self.chunks(content(node)));
        parts.push(annotation(&json!({ "end": "templateButton" })));
        parts.join("\n\n")
    }

    /// A derived block: its annotation, or the empty string where annotations are not written.
    fn derived(&self, note: Value) -> String {
        if self.annotate {
            annotation(&note)
        } else {
            String::new()
        }
    }

    /// `![alt](href "caption")` and the annotation of its layout.
    fn image_file(&self, links: &dyn ExportLinks, node: &Value) -> String {
        let alt = attr(node, "alt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let alt = link_title(alt).replace('[', "\\[").replace(']', "\\]");
        let dest = attachment_destination(links, attachment_id(node).unwrap_or_default());
        let line = match string_attr(node, "caption") {
            Some(caption) => format!("![{alt}]({dest} \"{}\")", link_title(caption)),
            None => format!("![{alt}]({dest})"),
        };
        let attrs = picked_attrs(node, &["width", "widthPercent", "align"]);
        let note = (!attrs.is_empty()).then(|| json!({ "attrs": attrs }));
        self.noted(note, line)
    }

    /// `[name](href)` and the annotation of its view.
    fn attachment_file(&self, links: &dyn ExportLinks, node: &Value) -> String {
        let id = attachment_id(node).unwrap_or_default();
        let name = id
            .parse::<AttachmentId>()
            .ok()
            .and_then(|parsed| self.names.get(&parsed))
            .map_or(id, String::as_str);
        let line = format!(
            "[{}]({})",
            escape_text(name, false),
            attachment_destination(links, id)
        );
        let attrs = picked_attrs(node, &["view"]);
        let note = (!attrs.is_empty()).then(|| json!({ "attrs": attrs }));
        self.noted(note, line)
    }

    /// One image line per usable item, behind the gallery's annotation.
    fn gallery_file(&self, links: &dyn ExportLinks, node: &Value) -> String {
        let items = attr(node, "items")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let lines: Vec<String> = items
            .iter()
            .filter_map(|item| {
                let id = item
                    .get("attachmentId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())?;
                let dest = attachment_destination(links, id);
                let caption = item
                    .get("caption")
                    .and_then(Value::as_str)
                    .filter(|caption| !caption.is_empty());
                Some(match caption {
                    Some(caption) => format!("![]({dest} \"{}\")", link_title(caption)),
                    None => format!("![]({dest})"),
                })
            })
            .collect();
        if lines.is_empty() {
            return String::new();
        }
        let mut extra = Map::new();
        extra.insert("items".to_owned(), json!(lines.len()));
        let note = note("gallery", picked_attrs(node, &["columns"]), extra);
        self.noted(Some(note), lines.join("\n\n"))
    }

    /// The callout's quote behind its annotation (icon, colour).
    fn callout_file(&self, node: &Value) -> String {
        let icon = string_attr(node, "icon").unwrap_or(DEFAULT_CALLOUT_ICON);
        let body = quote(&format!("{icon} {}", self.blocks(content(node))));
        let mut attrs = Map::new();
        attrs.insert("icon".to_owned(), json!(icon));
        if let Some(color) = string_attr(node, "color") {
            attrs.insert("color".to_owned(), json!(color));
        }
        self.noted(Some(note("callout", attrs, Map::new())), body)
    }

    /// A fenced code block whose fence is longer than any run of backticks in the code, with an
    /// info string that cannot end the fence line.
    fn code_block_file(&self, node: &Value) -> String {
        let code = self.inline_of(node, false);
        let fence = "`".repeat(longest_run(&code, '`').max(2) + 1);
        let language: String = string_attr(node, "language")
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_control() && *c != '`')
            .collect();
        format!("{fence}{language}\n{code}\n{fence}")
    }

    /// The whiteboard's summary quote behind its annotation (height, `.excalidraw` file).
    fn whiteboard_file(&self, links: &dyn ExportLinks, node: &Value) -> String {
        let mut extra = Map::new();
        let file = string_attr(node, "id")
            .and_then(|id| id.parse::<BlockId>().ok())
            .and_then(|block| links.whiteboard_href(block));
        if let Some(file) = file {
            extra.insert("file".to_owned(), json!(file));
        }
        let note = note("whiteboard", picked_attrs(node, &["height"]), extra);
        self.noted(Some(note), self.whiteboard(node))
    }
}

/// An annotation value: `{"node": name, "attrs": attrs?, ...extra}` (`attrs` only when not
/// empty).
fn note(name: &str, attrs: Map<String, Value>, extra: Map<String, Value>) -> Value {
    let mut object = extra;
    object.insert("node".to_owned(), json!(name));
    if !attrs.is_empty() {
        object.insert("attrs".to_owned(), Value::Object(attrs));
    }
    Value::Object(object)
}

/// The attributes of `node` named in `keys` that are set: a number or a non-empty string.
fn picked_attrs(node: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|key| {
            let value = attr(node, key)?;
            let set = value.is_number() || value.as_str().is_some_and(|text| !text.is_empty());
            set.then(|| ((*key).to_owned(), value.clone()))
        })
        .collect()
}

/// The annotation comment of `value`: [`ANNOTATION_OPEN`], compact JSON with sorted keys in
/// which every `--` is written `-\u002d` (a comment cannot hold two hyphens), [`ANNOTATION_CLOSE`].
fn annotation(value: &Value) -> String {
    let json = canonical(value).to_string().replace("--", "-\\u002d");
    format!("{ANNOTATION_OPEN}{json}{ANNOTATION_CLOSE}")
}

/// `value` with the keys of every object sorted (arrays keep their order). Sorted explicitly:
/// whether `serde_json::Map` keeps insertion order depends on a feature any crate of the build
/// may switch on.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                if let Some(entry) = map.get(key) {
                    out.insert(key.clone(), canonical(entry));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

// ----- the attributes of a `pageList`, with names instead of ids -----

/// Looks a property id (a JSON string) up: the id and the name of the property.
type PropertyLookup<'a> = &'a dyn Fn(&Value) -> Option<(PropertyId, String)>;

/// The attributes of a `pageList` as the annotation carries them: every id of the space's
/// schema replaced by a name, ids without a name dropped, `id` and null attributes left out.
/// `views` (ADR-0054) is carried too: every view as the block's own attributes are, plus its `id`,
/// `name`, `filter` tree, `openIn`, `dateBy`, `endBy`, `cardSize` and, for a dashboard, `widgets` (ADR-0055).
fn page_list_attrs(links: &dyn ExportLinks, node: &Value) -> Map<String, Value> {
    match node.get("attrs").and_then(Value::as_object) {
        Some(attrs) => named_attrs(links, attrs, Level::Block),
        None => Map::new(),
    }
}

/// Whose attributes [`named_attrs`] names: the block's, or one of its views'.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Block,
    View,
}

/// The attributes of a `pageList` block or of one of its views (a view is "the attributes of the block, one
/// per view") with names in the place of ids. A block leaves its `id` out, names its tag `source` and carries
/// its views; a view keeps its `id` (it names the tab) and has no source of its own.
fn named_attrs(
    links: &dyn ExportLinks,
    attrs: &Map<String, Value>,
    level: Level,
) -> Map<String, Value> {
    let mut out = Map::new();
    let property = |value: &Value| -> Option<(PropertyId, String)> {
        let id = value.as_str()?.parse::<PropertyId>().ok()?;
        Some((id, links.property_name(id)?))
    };
    let group_by = attrs.get("groupBy").and_then(property);
    for (key, value) in attrs {
        if value.is_null() {
            continue;
        }
        let kept = match key.as_str() {
            "id" => (level == Level::View).then(|| value.clone()),
            "source" if level == Level::Block => source_names(links, value),
            "views" if level == Level::Block => views_names(links, attrs),
            "filter" => filter_group_names(links, value, &property),
            "dateBy" => match value.as_str() {
                // The page's own times are no property.
                Some("created" | "updated") => Some(value.clone()),
                _ => property(value).map(|(_, name)| Value::String(name)),
            },
            "endBy" => property(value).map(|(_, name)| Value::String(name)),
            "properties" => value.as_array().map(|ids| {
                Value::Array(
                    ids.iter()
                        .filter_map(|id| property(id).map(|(_, name)| Value::String(name)))
                        .collect(),
                )
            }),
            "sort" => sort_names(value, &property),
            "sorts" => sorts_names(value, &property),
            "filters" => value.as_array().map(|filters| {
                Value::Array(
                    filters
                        .iter()
                        .filter_map(|filter| filter_names(links, filter, &property))
                        .collect(),
                )
            }),
            "groupBy" => group_by
                .as_ref()
                .map(|(_, name)| Value::String(name.clone())),
            "hiddenGroups" => hidden_names(links, group_by.as_ref().map(|(id, _)| *id), value),
            "columnWidths" => width_names(value, &property),
            "calculations" => calculation_names(value, &property),
            "widgets" if level == Level::View => widget_names(links, value, &property),
            _ => Some(value.clone()),
        };
        if let Some(kept) = kept {
            out.insert(key.clone(), kept);
        }
    }
    out
}

/// The views of a block as the annotation carries them (see [`named_attrs`]): the entries
/// [`views::read_views`] reads (at most 20, the first of a repeated id), each with names in the place of
/// ids. A view with nothing left to say is still there, with its `id`, `name` and `layout`. `None` when the
/// block holds no usable view (it reads as one made from its legacy attributes, which are carried).
fn views_names(links: &dyn ExportLinks, attrs: &Map<String, Value>) -> Option<Value> {
    let views = views::read_views(attrs)?;
    let named: Vec<Value> = views
        .iter()
        .filter_map(|view| match serde_json::to_value(view) {
            Ok(Value::Object(stored)) => {
                Some(Value::Object(named_attrs(links, &stored, Level::View)))
            }
            _ => None,
        })
        .collect();
    (!named.is_empty()).then_some(Value::Array(named))
}

/// A filter group with names in the place of ids: `{ "op", "items": [filters and (in the top-level group)
/// groups] }`. A filter or a property or option without a name is dropped; a group that is left with no item
/// after it had some is dropped with them (`None`), one that was empty stays.
fn filter_group_names(
    links: &dyn ExportLinks,
    group: &Value,
    property: PropertyLookup<'_>,
) -> Option<Value> {
    let object = group.as_object()?;
    let op = object.get("op")?;
    let items = object.get("items")?.as_array()?;
    let kept: Vec<Value> = items
        .iter()
        .filter_map(|item| {
            if item.get("items").is_some() {
                filter_group_names(links, item, property)
            } else {
                filter_names(links, item, property)
            }
        })
        .collect();
    if kept.is_empty() && !items.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("op".to_owned(), op.clone());
    out.insert("items".to_owned(), Value::Array(kept));
    Some(Value::Object(out))
}

/// `{"kind":"tag","option":"<name>"}`; other kinds as they are; a tag that has no name: none.
fn source_names(links: &dyn ExportLinks, source: &Value) -> Option<Value> {
    let object = source.as_object()?;
    if object.get("kind").and_then(Value::as_str) != Some("tag") {
        return Some(source.clone());
    }
    let option = object.get("optionId")?.as_str()?.parse::<OptionId>().ok()?;
    let name = links.option_name(TAGS_PROPERTY_ID, option)?;
    Some(json!({ "kind": "tag", "option": name }))
}

/// `{"by": "<name>", "dir": ...}`: `title`, `updated` and `created` stay.
fn sort_names(sort: &Value, property: PropertyLookup<'_>) -> Option<Value> {
    let object = sort.as_object()?;
    let by = object.get("by")?;
    let by_name = match by.as_str()? {
        built_in @ ("title" | "updated" | "created") => built_in.to_owned(),
        _ => property(by)?.1,
    };
    let mut out = Map::new();
    out.insert("by".to_owned(), Value::String(by_name));
    if let Some(dir) = object.get("dir").and_then(Value::as_str) {
        out.insert("dir".to_owned(), json!(dir));
    }
    Some(Value::Object(out))
}

/// The keys of a view's `sorts` (ADR-0058) as [`sort_names`] writes each; a key without a name is dropped
/// and so is the whole attribute when none is left. `None` for what is no list.
fn sorts_names(sorts: &Value, property: PropertyLookup<'_>) -> Option<Value> {
    let keys: Vec<Value> = sorts
        .as_array()?
        .iter()
        .filter_map(|key| sort_names(key, property))
        .collect();
    (!keys.is_empty()).then_some(Value::Array(keys))
}

/// A filter with `property` (a name) for `propertyId` and option names in the value of a
/// `hasOption` or `lacksOption`; none when the property or an option has no name.
fn filter_names(
    links: &dyn ExportLinks,
    filter: &Value,
    property: PropertyLookup<'_>,
) -> Option<Value> {
    let object = filter.as_object()?;
    let (id, name) = property(object.get("propertyId")?)?;
    let has_option = matches!(
        object.get("op").and_then(Value::as_str),
        Some("hasOption" | "lacksOption")
    );
    let mut out = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "propertyId" => {
                out.insert("property".to_owned(), Value::String(name.clone()));
            }
            "value" if has_option => {
                out.insert(key.clone(), option_names(links, id, value)?);
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Some(Value::Object(out))
}

/// The names of the option ids in `value` (one id or a list of them); none for an id that has
/// no name or a value that holds none.
fn option_names(links: &dyn ExportLinks, property: PropertyId, value: &Value) -> Option<Value> {
    let name = |id: &Value| {
        let option = id.as_str()?.parse::<OptionId>().ok()?;
        links.option_name(property, option)
    };
    match value {
        Value::String(_) => name(value).map(Value::String),
        Value::Array(ids) => Some(Value::Array(
            ids.iter().filter_map(name).map(Value::String).collect(),
        )),
        _ => None,
    }
}

/// The hidden columns of a board by option name (`none` stays); the options of the grouping
/// property (`group_by`) are the only ones that can be named.
fn hidden_names(
    links: &dyn ExportLinks,
    group_by: Option<PropertyId>,
    hidden: &Value,
) -> Option<Value> {
    let names = hidden.as_array()?.iter().filter_map(|entry| {
        let text = entry.as_str()?;
        if text == "none" {
            return Some(text.to_owned());
        }
        links.option_name(group_by?, text.parse::<OptionId>().ok()?)
    });
    Some(Value::Array(names.map(Value::String).collect()))
}

/// `{ "<property name>" | "title": px }`; widths of properties without a name are dropped.
fn width_names(widths: &Value, property: PropertyLookup<'_>) -> Option<Value> {
    let mut out = Map::new();
    for (key, width) in widths.as_object()? {
        if key == "title" {
            out.insert(key.clone(), width.clone());
        } else if let Some((_, name)) = property(&Value::String(key.clone())) {
            out.insert(name, width.clone());
        }
    }
    Some(Value::Object(out))
}

/// `{ "<property name>" | "title": "<function>" }`; the entries of properties without a name and
/// the ones whose function is no text are dropped, and so is the whole map when none is left.
fn calculation_names(calculations: &Value, property: PropertyLookup<'_>) -> Option<Value> {
    let mut out = Map::new();
    for (key, function) in calculations.as_object()? {
        if !function.is_string() {
            continue;
        }
        if key == "title" {
            out.insert(key.clone(), function.clone());
        } else if let Some((_, name)) = property(&Value::String(key.clone())) {
            out.insert(name, function.clone());
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// The widgets of a dashboard view (ADR-0055) with names in the place of property ids, in order: each
/// widget as stored, except its own `filter` tree (as a view's), the column of a number (`calc.by`;
/// `title` stays, a property without a name makes the number count all pages again), the grouping of a
/// chart (`chart.groupBy`; `created` and `updated` stay, a property without a name is `null`) and the
/// measure of a chart (`chart.measure.propertyId`; without a name the chart counts). Entries that are no
/// object stay as stored. `None` when `widgets` is no list.
fn widget_names(
    links: &dyn ExportLinks,
    widgets: &Value,
    property: PropertyLookup<'_>,
) -> Option<Value> {
    let list = widgets.as_array()?;
    Some(Value::Array(
        list.iter()
            .map(|widget| named_widget(links, widget, property))
            .collect(),
    ))
}

fn named_widget(links: &dyn ExportLinks, widget: &Value, property: PropertyLookup<'_>) -> Value {
    let Value::Object(object) = widget else {
        return widget.clone();
    };
    let mut out = object.clone();
    if let Some(filter) = object.get("filter") {
        match filter_group_names(links, filter, property) {
            Some(named) => {
                out.insert("filter".to_owned(), named);
            }
            None => {
                out.remove("filter");
            }
        }
    }
    if let Some(Value::Object(calc)) = object.get("calc")
        && let Some(by) = calc.get("by")
        && by.as_str().is_some_and(|by| by != "title")
    {
        let mut named = calc.clone();
        match property(by) {
            Some((_, name)) => {
                named.insert("by".to_owned(), Value::String(name));
            }
            None => {
                named.insert("by".to_owned(), Value::String("title".to_owned()));
                named.insert("fn".to_owned(), Value::String("count_all".to_owned()));
            }
        }
        out.insert("calc".to_owned(), Value::Object(named));
    }
    if let Some(Value::Object(chart)) = object.get("chart") {
        let mut named = chart.clone();
        if let Some(group) = chart.get("groupBy").and_then(Value::as_str)
            && !matches!(group, "created" | "updated")
        {
            named.insert(
                "groupBy".to_owned(),
                property(&Value::String(group.to_owned()))
                    .map_or(Value::Null, |(_, name)| Value::String(name)),
            );
        }
        if let Some(Value::Object(measure)) = chart.get("measure")
            && let Some(by) = measure.get("propertyId")
        {
            match property(by) {
                Some((_, name)) => {
                    let mut kept = measure.clone();
                    kept.insert("propertyId".to_owned(), Value::String(name));
                    named.insert("measure".to_owned(), Value::Object(kept));
                }
                None => {
                    named.insert("measure".to_owned(), json!({ "fn": "count" }));
                }
            }
        }
        out.insert("chart".to_owned(), Value::Object(named));
    }
    Value::Object(out)
}

// ----- text of the file flavour -----

/// Whether the next character of the text built so far (`out`) starts a line of the block:
/// the text is empty and starts one (`start`), or ends with a line break.
fn starts_line(out: &str, start: bool) -> bool {
    (out.is_empty() && start) || out.ends_with('\n')
}

/// A text run as Markdown text: every `\`, `` ` ``, `*`, `_`, `[`, `]`, `<`, `>`, `$`, `~`, `|`
/// and `&` gets a backslash; at the start of a line (`line_start`, or after a line break in the
/// text) a `#`, `+`, `-` or `=` does too, and so does the `.` or `)` of a leading run of
/// digits (`1\. not a list`).
fn escape_text(text: &str, line_start: bool) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut start = line_start;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '$' | '~' | '|' | '&' => {
                out.push('\\');
                out.push(c);
                start = false;
            }
            '\n' => {
                out.push(c);
                start = true;
            }
            ' ' | '\t' if start => {
                // Four columns of indentation would make the line a code block: the first
                // character of the white space is written as a character reference.
                let width = indent_width(c)
                    + chars
                        .clone()
                        .take_while(|next| matches!(next, ' ' | '\t'))
                        .map(indent_width)
                        .sum::<usize>();
                if width >= 4 {
                    out.push_str(if c == '\t' { "&#9;" } else { "&#32;" });
                    start = false;
                } else {
                    out.push(c);
                }
            }
            '#' | '+' | '-' | '=' if start => {
                out.push('\\');
                out.push(c);
                start = false;
            }
            '0'..='9' if start => {
                out.push(c);
                while let Some(digit) = chars.next_if(char::is_ascii_digit) {
                    out.push(digit);
                }
                if let Some(mark) = chars.next_if(|mark| matches!(mark, '.' | ')')) {
                    out.push('\\');
                    out.push(mark);
                }
                start = false;
            }
            _ => {
                out.push(c);
                start = false;
            }
        }
    }
    out
}

/// The columns a space or a tab takes in the indentation of a line.
fn indent_width(c: char) -> usize {
    if c == '\t' { 4 } else { 1 }
}

/// The text of a heading with the `#`s at its end escaped: `## Issue #` would read as a heading
/// `Issue` with a closing sequence.
fn escape_closing_hashes(text: &str) -> String {
    let body = text.trim_end_matches('#');
    let hashes = text.len() - body.len();
    if hashes == 0 {
        return text.to_owned();
    }
    // The first `#` is escaped already when an odd number of backslashes comes before it.
    let escaped = body.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1;
    let (kept, rest) = if escaped {
        (1, hashes - 1)
    } else {
        (0, hashes)
    };
    format!("{body}{}{}", "#".repeat(kept), "\\#".repeat(rest))
}

/// A code span: backticks one more than the longest run inside, and a space inside them when
/// the code starts or ends with a backtick or with a space on both ends (CommonMark strips one).
fn code_span(text: &str) -> String {
    let fence = "`".repeat(longest_run(text, '`') + 1);
    let spaced = text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' ') && text.chars().any(|c| c != ' '));
    if spaced {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

/// The length of the longest run of `target` in `text`.
fn longest_run(text: &str, target: char) -> usize {
    let (mut longest, mut current) = (0, 0);
    for c in text.chars() {
        if c == target {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// A link destination as it is written after `](`: as it is, or in angle brackets when it holds
/// white space, a control character, a `<`, a `>` or unbalanced parentheses.
fn link_destination(href: &str) -> String {
    let mut depth = 0_usize;
    let mut balanced = true;
    for c in href.chars() {
        match c {
            '(' => depth += 1,
            ')' => match depth.checked_sub(1) {
                Some(next) => depth = next,
                None => balanced = false,
            },
            _ => {}
        }
    }
    let plain = balanced
        && depth == 0
        && !href
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '<' || c == '>');
    if plain {
        return href.to_owned();
    }
    let mut out = String::from("<");
    for c in href.chars() {
        match c {
            '<' | '>' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            other => out.push(other),
        }
    }
    out.push('>');
    out
}

/// The destination of a link to an attachment: its file in the archive, or `attachment://<id>`.
fn attachment_destination(links: &dyn ExportLinks, stored: &str) -> String {
    let href = stored
        .parse::<AttachmentId>()
        .ok()
        .and_then(|id| links.attachment_href(id))
        .unwrap_or_else(|| format!("{ATTACHMENT_SCHEME}{stored}"));
    link_destination(&href)
}

/// Escapes every `|` of a table cell that is not already escaped (preceded by an odd number of
/// backslashes).
fn escape_pipes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut backslashes = 0_usize;
    for c in text.chars() {
        if c == '|' && backslashes.is_multiple_of(2) {
            out.push('\\');
        }
        out.push(c);
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
    }
    out
}

/// `&`, `<` and `>` as entities.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A date mention as a `<time>` element: `datetime` is the day, or `YYYY-MM-DDTHH:mm` with a time,
/// `data-reminder` the reminder in minutes as stored, `data-relative` the `relative` value (`today` or
/// `now`, ADR-0056); the text is the ISO text. A mention that is not valid is the empty string.
fn date_element(node: &Value) -> String {
    let text = date_text(node);
    if text.is_empty() {
        return text;
    }
    let datetime = text.replacen(' ', "T", 1);
    let reminder = integer_attr(node, "reminder")
        .filter(|minutes| *minutes >= 0)
        .map(|minutes| format!(" data-reminder=\"{minutes}\""))
        .unwrap_or_default();
    // A value filled in when a page is made from a template (ADR-0056): the two names only.
    let relative = string_attr(node, "relative")
        .filter(|relative| matches!(*relative, "today" | "now"))
        .map(|relative| format!(" data-relative=\"{relative}\""))
        .unwrap_or_default();
    format!("<time datetime=\"{datetime}\"{reminder}{relative}>{text}</time>")
}
