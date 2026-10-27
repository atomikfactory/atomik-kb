// The Markdown mirror of `kb_model::PageDoc::to_markdown` (`docs/development/milestone-03.md`,
// "Model"; `crates/kb-model/src/page/markdown.rs`). Both runtimes render the same portable
// JSON with the same literal rules, and `fixtures/doc/page-v1.md` is their shared golden.
// No escaping in v1.

import type { AttachmentId } from "./generated/AttachmentId";
import type { PageId } from "./generated/PageId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { canonicalId, dateMentionText, mathInlineSource, parsePortable, personMentionLabel } from "./page-doc";
import type { PortableMark, PortableNode } from "./page-doc";
import { whiteboardTextsOfPortable } from "./whiteboard";

/** Resolves a referenced page to its title; `undefined` renders the stored id instead. */
type TitleOf = (pageId: PageId) => string | undefined;

/** Optional inputs of {@link renderMarkdown}. */
export interface MarkdownOptions {
  /**
   * The name of an attachment shown by a `file` block (from its manifest); `undefined`, or
   * no resolver at all, renders the stored attachment id instead.
   */
  resolveAttachmentName?: (attachmentId: AttachmentId) => string | undefined;
}

/** What the renderer resolves references with. */
interface Context {
  titleOf: TitleOf;
  nameOf: (attachmentId: AttachmentId) => string | undefined;
  /** The top-level `whiteboards` value of the portable JSON (ADR-0033), when the page has one. */
  whiteboards: unknown;
}

/** Scheme of attachment links in exported Markdown (`attachment://<id>`). */
const ATTACHMENT_SCHEME = "attachment://";

/** Icon of a callout without one (the schema default). */
const DEFAULT_CALLOUT_ICON = "💡";

/** What a sequence of children renders as among blocks. */
type Unit = { kind: "inline"; nodes: PortableNode[] } | { kind: "block"; node: PortableNode };

/**
 * The marks Markdown can express on a run of text; other marks render nothing and do not
 * split a run. That is the unknown marks and the colour marks `textColor` and `highlight`
 * (added 2026-10-07, attribute `color`): Markdown has no colour, so coloured text exports as
 * plain text, and a bold sentence with one red word is still one bold run.
 */
interface Style {
  link: string | undefined;
  bold: boolean;
  italic: boolean;
  strike: boolean;
  underline: boolean;
  code: boolean;
}

/**
 * Renders a page (portable JSON or raw ProseMirror JSON, normalised first) as Markdown:
 *
 * - the root's children are blocks joined by one blank line; the output ends with
 *   exactly one newline (an empty page is `"\n"`);
 * - inline content: consecutive text with the same Markdown-relevant marks (bold, italic,
 *   strike, underline, code, and link with its `href`) is joined, then wrapped from the
 *   outermost: link `[...](href)`, `**`, `*`, `~~`, `<u>...</u>`, backticks; unknown
 *   marks, `textColor` and `highlight` (added 2026-10-07: Markdown has no colour) and a link
 *   without `href` render nothing, and do not split a run; `hardBreak` is two spaces and a
 *   newline; `pageRef` is `[[Title]]` through `titleOf`, or `[[<pageId as stored>]]`
 *   when the title is unknown (`blockId` ignored); `dateMention` is its ISO text and `mathInline`
 *   is `$latex$` (both below);
 * - `paragraph` is its inline content; `heading` is `#` x level (1 to 6, default 1), a
 *   space, the text; `horizontalRule` is `---`; `codeBlock` is three backticks and the
 *   language (empty when absent), the plain text, three backticks;
 * - lists: one item per marker, no blank lines inside a list; the marker is `- `
 *   (`bulletList`), `N. ` counting from `start` (default 1, `orderedList`) or
 *   `- [x] ` / `- [ ] ` (`taskList`); an item's blocks are joined by newlines, the first
 *   line follows the marker and the others are indented by the marker width (two for
 *   task items), empty lines staying empty;
 * - `blockquote` prefixes every line of its blocks (joined by blank lines) with `> `
 *   (`>` for empty lines); `callout` does the same with `> ` + icon (default 💡) + space
 *   in front of its first line, and ignores its `color` (added 2026-10-06; Markdown has no
 *   background, so a coloured callout exports like an uncoloured one);
 * - `details` is `<details open>` or `<details>`, `<summary>` + plain summary text +
 *   `</summary>`, a blank line, the blocks of `detailsContent`, a blank line, `</details>`. A toggle
 *   heading (the summary's attribute `level`, an integer from 1 to 3, added 2026-10-14, ADR-0040; any
 *   other value reads as absent) puts the summary's text in a heading element,
 *   `<summary><h2>Title</h2></summary>` (empty: `<summary><h2></h2></summary>`);
 * - `image` is `![alt](attachment://<attachmentId>)`, both as stored (empty when absent),
 *   `width`, `widthPercent` and `align` not rendered; with a non-empty `caption` (2026-10-09,
 *   ADR-0028) the line gains the CommonMark link title, `![alt](attachment://<id> "caption")`,
 *   in which `\` is written `\\`, `"` is written `\"` and a line break (CRLF, LF or CR) is
 *   written as one space; `file` is `[name](attachment://<attachmentId>)`, the name coming
 *   from `resolveAttachmentName` (asked once per distinct valid id of the `file` nodes, in
 *   document order, before rendering) or else the attachment id as stored;
 * - `table` (additive node, 2026-10-05) is a GFM table with one line per child row: a row
 *   (`tableRow`, in fact any child of the table) is `| ` + its children (any name:
 *   `tableCell`, `tableHeader`) separated by ` | ` + ` |`; a cell's text is its children
 *   rendered as blocks and joined by blank lines, then every `|` is escaped as `\|` and
 *   every newline becomes `<br>` (no other escaping); after the first row comes the
 *   separator row `| --- | --- |` with as many `---` as that row has cells; lines are joined
 *   by single newlines and a table without rows is the empty string; `colspan`, `rowspan`,
 *   `colwidth` and `align` are ignored, and header cells look like other cells (GFM's
 *   first row is the header);
 * - `mathBlock` (additive node, 2026-10-06) is `$$`, a newline, the plain text, a newline,
 *   `$$` (an empty block is `$$`, an empty line, `$$`; a multi-line source keeps its
 *   newlines; no escaping: the LaTeX is written as stored);
 * - `pageLink` (additive node, 2026-10-06), the card that links to a page, renders like a
 *   `pageRef` standing alone: `[[Title]]` through `titleOf`, or `[[<pageId as stored>]]`
 *   when the title is unknown (`blockId` ignored). It is a block, so it is joined to its
 *   neighbours by blank lines like any block (a `pageRef` among text stays part of its
 *   paragraph);
 * - `dateMention` (additive inline node, 2026-10-07) is its ISO text, `2026-10-07`, or
 *   `2026-10-07 14:30` when it has a time: the `date` (`YYYY-MM-DD`, a floating local date),
 *   then a space and the `time` (`HH:mm`). Plain text, no brackets, no escaping: it never needs
 *   any, and importing Markdown does not read a date back (it stays text). A mention whose
 *   `date` is not a valid date renders as the empty string and a malformed `time` is left out,
 *   so nothing but a date or a time ever reaches the export. It is inline: it stays inside its
 *   paragraph, joins the text around it, and in a toggle's summary or any other plain-text
 *   context it is the same text. Its `reminder` attribute (minutes before the moment, ADR-0024)
 *   is not rendered: a reminder is a device's business, not part of the text, so the export is
 *   the same with or without one;
 * - `mathInline` (additive inline node, 2026-10-07) is `$`, the `latex` source exactly as stored,
 *   `$`. No escaping and no trimming: the LaTeX is written as stored, so a source that holds a `$`
 *   (or starts or ends with a space) does not read back as one equation (the parser takes `$...$`
 *   on one line with no `$` inside and no space right after the opening or before the closing
 *   `$`). An equation whose `latex` is empty or not a string is the empty string (`$$` is the math
 *   block's fence, so it is never written). It is inline: it stays inside its paragraph and joins
 *   the text around it;
 * - `columnList` (additive node, 2026-10-06) is its columns one after another, linearised
 *   (Markdown has no side by side, and nothing marks where a column starts): each `column`
 *   is its blocks joined by blank lines, and the columns are joined by blank lines too. That
 *   is the rule for unknown nodes below, written down because the layout depends on it;
 *   there is no special case in the code. An empty column (no blocks, or one empty
 *   paragraph) renders as the empty string and still takes its place in the join, as an
 *   empty top-level paragraph does: the blank lines pile up where it was, and at the very
 *   end of the page they are trimmed. The number of columns and the nesting are not looked at;
 * - `tableOfContents` and `breadcrumb` (additive nodes, 2026-10-06) are the empty string. Both
 *   are leaf blocks whose content the editor derives (the page's headings, the page's place in
 *   the tree), so the document stores nothing to export. That is the rule for unknown nodes
 *   below (a leaf has no children to render), written down because the blank lines depend on
 *   it; there is no special case in the code. The empty block takes its place in the join, as
 *   an empty top-level paragraph and an empty column do, and each one adds two newlines:
 *   between two blocks it leaves three blank lines instead of one (two derived blocks in a
 *   row leave five), as the first block of the page it leaves two empty lines before the
 *   first line (leading newlines are not trimmed), and as the last blocks of the page it
 *   vanishes with the trailing newlines;
 * - `pageList` (additive node, 2026-10-07) is the empty string too, by the same rule and with the
 *   same blank lines. The list view shows a set of pages (the children of the page that holds it,
 *   or the pages that carry a tag) that the app derives at run time from the tree and the property
 *   values; the document stores only the configuration of the view (`source`, `layout`,
 *   `properties`, `sort`). A line per page, `- [[Title]]`, is therefore not possible in the core (it
 *   would be a snapshot of what the view happened to show, stale in the file as soon as a page moves
 *   or is renamed), so the export writes nothing for it;
 * - `templateButton` (additive node, ADR-0056) is the empty string, with the same blank lines as the derived blocks
 *   above: the button holds template material (blocks that a click copies into the page), which is not part of the
 *   page's text. A `dateMention` or `personMention` with a `relative` attribute renders as without it;
 * - `gallery` (additive node, 2026-10-09, ADR-0030) is one image line per item of `items`, in order,
 *   `![](attachment://<attachmentId>)`, with the item's non-empty `caption` as the link title
 *   exactly as for an `image` (`![](attachment://<attachmentId> "caption")`), the lines joined by
 *   blank lines (so each reads back as an image block of its own). An item that is not a map or
 *   has no non-empty string `attachmentId` is skipped, the id is written as stored; a gallery
 *   without a list `items` or without a usable item is the empty string (the rule of the derived
 *   blocks above). `columns` is layout and is not written;
 * - `whiteboard` (additive node, 2026-10-10, ADR-0033) is a block quote: the marker paragraph `> **Whiteboard**`,
 *   a blank quote line and one `> - <text>` item per live text element of the drawing in z-order (whitespace collapsed,
 *   blank texts skipped), the elements read from the top-level `whiteboards` value of the portable JSON;
 *   a whiteboard without text is the marker line alone. No image is written;
 * - plain text (`codeBlock`, `mathBlock`, `detailsSummary`) drops marks; a `hardBreak` is a newline;
 * - unknown nodes render their children as blocks joined by blank lines, or their inline
 *   content inside a line; consecutive inline nodes met among blocks form one paragraph.
 *
 * Throws an `InvalidInput` `CoreError` when `json` is not a node tree.
 */
export function renderMarkdown(json: JsonValue, titleOf: TitleOf, options: MarkdownOptions = {}): string {
  const root = parsePortable(json);
  const names = fileNames(root, options.resolveAttachmentName);
  const whiteboards = typeof json === "object" && json !== null && !Array.isArray(json) ? json.whiteboards : undefined;
  const ctx: Context = { titleOf, nameOf: (attachmentId) => names.get(attachmentId), whiteboards };
  return `${blocks(root.content, ctx).join("\n\n").replace(/\n+$/u, "")}\n`;
}

/**
 * The names of the attachments of the `file` nodes, asked once per distinct valid id in
 * document order, before rendering (as `kb-model` asks them); ids without a name are left out.
 */
function fileNames(
  root: PortableNode,
  resolve: ((attachmentId: AttachmentId) => string | undefined) | undefined,
): Map<AttachmentId, string> {
  const asked = new Set<AttachmentId>();
  const names = new Map<AttachmentId, string>();
  const visit = (node: PortableNode): void => {
    const stored = node.type === "file" ? stringAttr(node, "attachmentId") : undefined;
    const attachmentId = stored === undefined ? undefined : canonicalId(stored);
    if (attachmentId !== undefined && !asked.has(attachmentId)) {
      asked.add(attachmentId);
      const name = resolve?.(attachmentId);
      if (name !== undefined) {
        names.set(attachmentId, name);
      }
    }
    node.content.forEach(visit);
  };
  visit(root);
  return names;
}

function isInline(node: PortableNode): boolean {
  return (
    node.type === "text" ||
    node.type === "pageRef" ||
    node.type === "dateMention" ||
    node.type === "mathInline" ||
    node.type === "personMention" ||
    node.type === "hardBreak"
  );
}

/** Groups children into blocks: consecutive inline nodes become one unit. */
function units(children: readonly PortableNode[]): Unit[] {
  const grouped: Unit[] = [];
  let inline: PortableNode[] = [];
  for (const child of children) {
    if (isInline(child)) {
      inline.push(child);
      continue;
    }
    if (inline.length > 0) {
      grouped.push({ kind: "inline", nodes: inline });
      inline = [];
    }
    grouped.push({ kind: "block", node: child });
  }
  if (inline.length > 0) {
    grouped.push({ kind: "inline", nodes: inline });
  }
  return grouped;
}

/** Every child block rendered, in order. */
function blocks(children: readonly PortableNode[], ctx: Context): string[] {
  return units(children).map((unit) => renderUnit(unit, ctx));
}

function renderUnit(unit: Unit, ctx: Context): string {
  return unit.kind === "inline" ? renderInline(unit.nodes, ctx) : renderBlock(unit.node, ctx);
}

function renderBlock(node: PortableNode, ctx: Context): string {
  switch (node.type) {
    case "paragraph":
      return renderInline(node.content, ctx);
    case "heading": {
      const level = Math.min(6, Math.max(1, integerAttr(node, "level") ?? 1));
      return `${"#".repeat(level)} ${renderInline(node.content, ctx)}`;
    }
    case "codeBlock":
      return `\`\`\`${stringAttr(node, "language") ?? ""}\n${plainText(node.content, ctx)}\n\`\`\``;
    case "mathBlock":
      return `$$\n${plainText(node.content, ctx)}\n$$`;
    case "horizontalRule":
      return "---";
    case "bulletList":
      return renderList(node, () => ["- ", 2], ctx);
    case "orderedList": {
      const start = integerAttr(node, "start") ?? 1;
      return renderList(
        node,
        (n) => {
          const marker = `${String(start + n)}. `;
          return [marker, marker.length];
        },
        ctx,
      );
    }
    case "taskList":
      return renderList(
        node,
        (_n, item) => [item?.type === "taskItem" && item.attrs?.checked === true ? "- [x] " : "- [ ] ", 2],
        ctx,
      );
    case "blockquote":
      return quote(blocks(node.content, ctx).join("\n\n"), undefined);
    case "callout":
      return quote(blocks(node.content, ctx).join("\n\n"), stringAttr(node, "icon") ?? DEFAULT_CALLOUT_ICON);
    case "details":
      return renderDetails(node, ctx);
    case "detailsSummary":
      return plainText(node.content, ctx);
    case "image": {
      const target = `${ATTACHMENT_SCHEME}${stringAttr(node, "attachmentId") ?? ""}`;
      const caption = stringAttr(node, "caption");
      const title = caption === undefined || caption === "" ? "" : ` "${linkTitle(caption)}"`;
      return `![${stringAttr(node, "alt") ?? ""}](${target}${title})`;
    }
    case "file": {
      const stored = stringAttr(node, "attachmentId") ?? "";
      const attachmentId = canonicalId(stored);
      const name = (attachmentId === undefined ? undefined : ctx.nameOf(attachmentId)) ?? stored;
      return `[${name}](${ATTACHMENT_SCHEME}${stored})`;
    }
    case "gallery":
      return renderGallery(node);
    case "whiteboard":
      return renderWhiteboard(node, ctx);
    case "table":
      return renderTable(node, ctx);
    case "pageLink":
      return pageRef(node, ctx);
    case "templateButton":
      return ""; // template material (ADR-0056)
    default:
      return blocks(node.content, ctx).join("\n\n");
  }
}

/** A `gallery`: one image line per usable item, joined by blank lines (ADR-0030). */
function renderGallery(node: PortableNode): string {
  const items: unknown = node.attrs?.items;
  if (!Array.isArray(items)) {
    return "";
  }
  const lines: string[] = [];
  for (const item of items as unknown[]) {
    if (typeof item !== "object" || item === null || Array.isArray(item)) {
      continue;
    }
    const { attachmentId, caption } = item as Record<string, unknown>;
    if (typeof attachmentId !== "string" || attachmentId === "") {
      continue;
    }
    const title = typeof caption === "string" && caption !== "" ? ` "${linkTitle(caption)}"` : "";
    lines.push(`![](${ATTACHMENT_SCHEME}${attachmentId}${title})`);
  }
  return lines.join("\n\n");
}

/** A `whiteboard`: a quote of the marker line and one list item per live text element (ADR-0033). */
function renderWhiteboard(node: PortableNode, ctx: Context): string {
  const id = stringAttr(node, "id");
  const boards = ctx.whiteboards;
  const elements =
    id !== undefined && typeof boards === "object" && boards !== null && !Array.isArray(boards)
      ? (boards as Record<string, unknown>)[id]
      : undefined;
  const items = whiteboardTextsOfPortable(elements).map((text) => `- ${text}`);
  // The marker paragraph, then the list as a block of its own: what the parser reads back renders the same.
  return quote(["**Whiteboard**", ...(items.length === 0 ? [] : [items.join("\n")])].join("\n\n"), undefined);
}

/** A GFM table: `| a | b |` per row, the `| --- | --- |` separator after the first. */
function renderTable(table: PortableNode, ctx: Context): string {
  const lines: string[] = [];
  table.content.forEach((row, index) => {
    const cells = row.content.map((cell) => blocks(cell.content, ctx).join("\n\n").replaceAll("|", "\\|").replaceAll("\n", "<br>"));
    lines.push(tableLine(cells));
    if (index === 0) {
      lines.push(tableLine(cells.map(() => "---")));
    }
  });
  return lines.join("\n");
}

/** One line of a table: `| ` + the cells separated by ` | ` + ` |`. */
function tableLine(cells: readonly string[]): string {
  return `| ${cells.join(" | ")} |`;
}

/**
 * One line group per entry of the list: `marker(n, item)` gives the marker of the `n`-th
 * entry and the indent of its following lines (`item` is undefined for stray inline content).
 */
function renderList(
  list: PortableNode,
  marker: (n: number, item: PortableNode | undefined) => [string, number],
  ctx: Context,
): string {
  const lines: string[] = [];
  units(list.content).forEach((unit, n) => {
    let item: PortableNode | undefined;
    let content: string;
    if (unit.kind === "block" && (unit.node.type === "listItem" || unit.node.type === "taskItem")) {
      item = unit.node;
      content = blocks(unit.node.content, ctx).join("\n");
    } else if (unit.kind === "block") {
      item = unit.node;
      content = renderBlock(unit.node, ctx);
    } else {
      content = renderInline(unit.nodes, ctx);
    }
    const [prefix, indent] = marker(n, item);
    const [first = "", ...rest] = content.split("\n");
    lines.push(`${prefix}${first}`);
    const pad = " ".repeat(indent);
    for (const line of rest) {
      lines.push(line === "" ? "" : `${pad}${line}`);
    }
  });
  return lines.join("\n");
}

function renderDetails(node: PortableNode, ctx: Context): string {
  let summary: string | undefined;
  const body: string[] = [];
  for (const unit of units(node.content)) {
    if (unit.kind === "block" && unit.node.type === "detailsSummary" && summary === undefined) {
      summary = plainText(unit.node.content, ctx);
      const level = integerAttr(unit.node, "level");
      if (level !== undefined && level >= 1 && level <= 3) {
        summary = `<h${String(level)}>${summary}</h${String(level)}>`;
      }
    } else if (unit.kind === "block" && unit.node.type === "detailsContent") {
      body.push(...blocks(unit.node.content, ctx));
    } else {
      body.push(renderUnit(unit, ctx));
    }
  }
  const open = node.attrs?.open === true ? "<details open>" : "<details>";
  return `${open}\n<summary>${summary ?? ""}</summary>\n\n${body.join("\n\n")}\n\n</details>`;
}

/** Inline content with marks. */
function renderInline(nodes: readonly PortableNode[], ctx: Context): string {
  let out = "";
  let run = "";
  let style = styleOf([]);
  const flush = (): void => {
    if (run !== "") {
      out += wrap(run, style);
      run = "";
    }
  };
  for (const node of nodes) {
    if (node.type === "text") {
      const next = styleOf(node.marks);
      if (!sameStyle(next, style)) {
        flush();
        style = next;
      }
      run += node.text ?? "";
      continue;
    }
    flush();
    if (node.type === "pageRef") {
      out += pageRef(node, ctx);
    } else if (node.type === "dateMention") {
      out += dateText(node);
    } else if (node.type === "mathInline") {
      out += mathText(node);
    } else if (node.type === "personMention") {
      out += personText(node);
    } else if (node.type === "hardBreak") {
      out += "  \n";
    } else {
      out += renderInline(node.content, ctx);
    }
  }
  flush();
  return out;
}

/** Text without marks (code blocks, toggle summaries). */
function plainText(nodes: readonly PortableNode[], ctx: Context): string {
  let out = "";
  for (const node of nodes) {
    if (node.type === "text") {
      out += node.text ?? "";
    } else if (node.type === "pageRef") {
      out += pageRef(node, ctx);
    } else if (node.type === "dateMention") {
      out += dateText(node);
    } else if (node.type === "mathInline") {
      out += mathText(node);
    } else if (node.type === "personMention") {
      out += personText(node);
    } else if (node.type === "hardBreak") {
      out += "\n";
    } else {
      out += plainText(node.content, ctx);
    }
  }
  return out;
}

/** What a `dateMention` renders as: its ISO text, or nothing when its `date` is not a valid date. */
function dateText(node: PortableNode): string {
  return dateMentionText(node.attrs?.date, node.attrs?.time) ?? "";
}

/**
 * What a `mathInline` renders as, given its `latex` attribute: `$latex$` as stored, or the empty
 * string when it is empty or not a string. The editor's plain-text copy of the node is the same text.
 */
export function mathInlineMarkdown(latex: unknown): string {
  const source = mathInlineSource(latex);
  return source === undefined || source === "" ? "" : `$${source}$`;
}

/**
 * What a `personMention` (ADR-0049) renders as: `@label`, the empty string when its `label` is empty or not a string. The
 * account id is never written. The editor's plain-text copy of the node is the same text.
 */
export function personMentionMarkdown(label: unknown): string {
  const text = personMentionLabel(label);
  return text === "" ? "" : `@${text}`;
}

/** What a `personMention` node renders as (see `personMentionMarkdown`). */
function personText(node: PortableNode): string {
  return personMentionMarkdown(node.attrs?.label);
}

/** What a `mathInline` node renders as (see `mathInlineMarkdown`). */
function mathText(node: PortableNode): string {
  return mathInlineMarkdown(node.attrs?.latex);
}

function pageRef(node: PortableNode, ctx: Context): string {
  const stored = stringAttr(node, "pageId") ?? "";
  const pageId = canonicalId(stored);
  return `[[${(pageId === undefined ? undefined : ctx.titleOf(pageId)) ?? stored}]]`;
}

function styleOf(marks: readonly PortableMark[]): Style {
  const has = (type: string): boolean => marks.some((mark) => mark.type === type);
  const href = marks.find((mark) => mark.type === "link")?.attrs?.href;
  return {
    link: typeof href === "string" ? href : undefined,
    bold: has("bold"),
    italic: has("italic"),
    strike: has("strike"),
    underline: has("underline"),
    code: has("code"),
  };
}

function sameStyle(a: Style, b: Style): boolean {
  return (
    a.link === b.link &&
    a.bold === b.bold &&
    a.italic === b.italic &&
    a.strike === b.strike &&
    a.underline === b.underline &&
    a.code === b.code
  );
}

/** Applies the marks innermost first, so that a link ends up outermost and code innermost. */
function wrap(text: string, style: Style): string {
  let out = text;
  if (style.code) {
    out = `\`${out}\``;
  }
  if (style.underline) {
    out = `<u>${out}</u>`;
  }
  if (style.strike) {
    out = `~~${out}~~`;
  }
  if (style.italic) {
    out = `*${out}*`;
  }
  if (style.bold) {
    out = `**${out}**`;
  }
  if (style.link !== undefined) {
    out = `[${out}](${style.link})`;
  }
  return out;
}

/** Prefixes every line with `> ` (`>` when empty); with an icon, the first line gets `> ` + icon + space. */
function quote(content: string, icon: string | undefined): string {
  return content
    .split("\n")
    .map((line, n) => {
      if (icon !== undefined && n === 0) {
        return `> ${icon} ${line}`;
      }
      return line === "" ? ">" : `> ${line}`;
    })
    .join("\n");
}

/** An integer attribute (JavaScript numbers that are whole count, as in `kb-model`). */
function integerAttr(node: PortableNode, key: string): number | undefined {
  const value = node.attrs?.[key];
  return typeof value === "number" && Number.isInteger(value) ? value : undefined;
}

/**
 * `text` as the body of a double-quoted CommonMark link title: `\` and `"` are escaped with a
 * backslash and every line break (CRLF, LF or CR) becomes one space. Mirrors `link_title` in
 * `kb-model`'s `page/markdown.rs`.
 */
function linkTitle(text: string): string {
  return text.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\r\n|\r|\n/g, " ");
}

function stringAttr(node: PortableNode, key: string): string | undefined {
  const value = node.attrs?.[key];
  return typeof value === "string" ? value : undefined;
}
