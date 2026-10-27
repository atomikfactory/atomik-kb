// The Markdown parser: the inverse of `renderMarkdown` (`./markdown.ts`, whose rules are
// the literal export rules of `docs/development/milestone-03.md`, "Model"). It turns Markdown
// text into block nodes of the portable page shape (`page-doc.ts`, format 1), without `id`
// attributes: whoever inserts the nodes assigns the ids (the editor's `UniqueID`, the core's
// writer). No dependency, line-based, deterministic, never throws: whatever is not
// recognised stays text. Used by the editor's paste handler (`packages/ui`) and meant for
// file import (roadmap M10).
//
// What is recognised (the rule of the export it inverts in brackets):
//
// - blocks are separated by blank lines; ATX headings `#` to `######` (levels above 3 become
//   3, the schema stops there; a lone `#` is an empty heading); paragraphs; `---`, `***`,
//   `___` (also spaced out) dividers;
// - fenced code, ``` or ~~~ with an optional language (the first word after the fence); an
//   unterminated fence runs to the end of its container; `$$` math blocks, `$$`, the LaTeX
//   lines, `$$` (`$$LaTeX$$` on one line works too; an unterminated `$$` is plain text);
// - lists: `-`, `*` or `+` bullets, `N.` or `N)` ordered lists (`start` is the first number),
//   `- [ ]` and `- [x]` task lists; an item holds blocks, a line indented by 2 or more spaces
//   (or a tab) under an item belongs to it (that is how lists nest) and any other line ends
//   the list; blank lines inside a list do not end it, a change of marker does (and a task list is
//   its own list);
// - `> ` blockquotes; one whose first line starts with an icon (an emoji, or another non-ASCII
//   symbol, alone in front of a space: `> 💡 text`) is a `callout` with that `icon` (the export
//   writes `> ` + icon + space in front of the first line);
// - `<details>` / `<details open>`, `<summary>text</summary>`, blocks, `</details>` toggles (a summary that is
//   one `<h1>` to `<h3>` element, `<summary><h2>text</h2></summary>`, is a toggle heading: the summary gets `level`);
// - GFM tables: a header row, a `| --- |` row of as many cells, rows; `\|` is a pipe, `<br>`
//   inside a cell separates its blocks (two in a row, as the export writes between blocks) or,
//   alone, is a line break; the first row is made of `tableHeader` cells; a short row is
//   padded with empty cells and the cells beyond the header's are dropped (GFM);
// - a line that is only `![alt](attachment://<id>)` or `[name](attachment://<id>)` with a valid
//   attachment id is an `image` or `file` block (the name is not kept: the file's own name
//   comes from its manifest); an image line may carry a link title, `![alt](attachment://<id>
//   "caption")`, which is the image's `caption` (2026-10-09, ADR-0028: `\\` and `\"` in it are
//   a backslash and a quote, another backslash stays; an empty title is no caption); any other
//   use of those links stays a link or text;
// - inline: `**bold**`, `*italic*` or `_italic_`, `~~strike~~`, `<u>underline</u>`, `code`
//   (backtick runs of any length), `[text](href)` (a title after the href is dropped) and
//   hard breaks (two spaces or more at the end of a line, or any line break inside a table
//   cell); a single line break is a space. `$latex$` is an inline equation (`mathInline` with
//   that `latex`, 2026-10-07): on one line, a single `$` opening, no whitespace right after it,
//   no `$` inside, no whitespace or backslash right before the closing `$`, and not followed by a
//   digit (so `costs $5 and $10` and `$5-$10` stay text); a `$` right after a backslash and a
//   `$$` run are text (`$$` is the math block's fence), and what is inside is not read as Markdown
//   (`$a*b*c$` is one equation). Colours are not parsed: the export writes none
//   (`textColor` and `highlight`, 2026-10-07, render as plain text), so text never comes back
//   coloured. `[[Title]]` stays the plain text `[[Title]]`
//   (the parser has no page index to make a `pageRef` of it). A date mention written as text
//   (the clipboard flavour: `2026-10-07`, `2026-10-07 14:30`) comes back as that text, but the
//   `<time datetime="2026-10-07T14:30" data-reminder="15" data-relative="today">…</time>` element
//   of the file flavour (ADR-0036) is read as a `dateMention` (`kb-import` does the same): `date`
//   and `time` from `datetime` (read by `parseDate`: `YYYY-MM-DD`, then a time after `T` or a space,
//   seconds and a zone ignored, a Notion style date too), `reminder` from `data-reminder` (a
//   non-negative whole number of minutes, else none) and `relative` from `data-relative` (`today` or
//   `now`, ADR-0056, else none); the element's own text is ignored, and an element without a
//   `datetime` that reads as a date stays text, tags and all. Images with another source and
//   unknown HTML stay text. Emphasis follows CommonMark's delimiter rules, so `snake_case` and
//   `2 * 3 * 4` stay text.
//
// Not recognised: backslash escapes (the export writes none: nothing is unescaped, and a
// delimiter right after a backslash is text, so `\*x\*` stays as typed), setext headings,
// indented code, HTML blocks, reference links, autolinks, footnotes, front matter.
//
// Round trip: `renderMarkdown(parseMarkdown(renderMarkdown(page)))` is the text it started
// from, for any page the editor can write (`markdown-parse.test.ts` checks the golden sample and
// more), with these exceptions, none of which the export keeps either:
// - empty paragraphs between blocks (blank lines collapse), a trailing hard break, and the
//   spaces around a hard break (a line break drops them);
// - text with a mark that starts or ends with a space (bold "Title " is `**Title **`): no
//   Markdown reader takes it for bold, so it comes back as the text it is written as;
// - a `mathInline` whose source has a `$` in it, whitespace at either end, a newline or a backslash
//   as its last character, or that is followed by a digit, does not come back as that equation (the
//   export writes the source as stored and nothing marks it, so what comes back is whatever the rules
//   above find in the text), and an empty one is not written;
// - a `dateMention` of the clipboard flavour comes back as its ISO text (the export writes nothing
//   else: same Markdown, no mention; the file flavour's `<time>` element is the one that is read back,
//   see above, and `renderMarkdown` writes it as the same ISO text);
// - a `pageRef` comes back as the text `[[Title]]` (same Markdown, no reference), a
//   `columnList` as its blocks one after another (the export linearised it), a callout's
//   `color`, the `textColor` and `highlight` marks, an image's `width`, `widthPercent` and
//   `align` and a table cell's `colspan`, `rowspan` and `colwidth` are gone (the export does not
//   write them), and so are the `tableOfContents`, `breadcrumb`, `pageList` and `pageLink`
//   blocks, which the export writes as the text they show, or as nothing; a `gallery` (2026-10-09,
//   ADR-0030) comes back as the separate `image` blocks of its lines (the export writes one image
//   line per item and nothing of the grid: no gallery is rebuilt, and the column count is gone); a
//   `whiteboard` (2026-10-10, ADR-0033) comes back as a block quote with a list (the marker line and the
//   texts of the drawing): no whiteboard is rebuilt;
// - two adjacent lists of the same kind merge into one (the blank line between them does not
//   end a list), and the lines of an item's several paragraphs, which the export writes one
//   under the other, read back as one paragraph.
//
// Text that mixes marks with the characters they are written with (a literal `*` inside bold,
// say) is ambiguous without escapes; the parser reads it as CommonMark would, and what it does
// not read as a mark stays text.

import type { JsonValue } from "./generated/serde_json/JsonValue";
import { parseDate } from "./memory-import-text";
import { canonicalId, parsePortable, reminderMinutesOf } from "./page-doc";
import type { PortableNode } from "./page-doc";

/** A node being built: portable JSON, normalised by `parsePortable` at the end. */
type Obj = { [key: string]: JsonValue };

/** Containers (quotes, lists, toggles, table cells) nest at most this deep; deeper stays text. */
const MAX_DEPTH = 24;
/** Links nest at most this deep (`[a [b](c)](d)`). */
const MAX_LINK_DEPTH = 6;
/** The longest link destination, in characters (a longer one is not a link; the scan for it stays bounded). */
const MAX_DESTINATION = 2000;
/** A paragraph with more emphasis delimiters than this gets none of them (keeps the matching linear enough). */
const MAX_DELIMITERS = 4000;
/** The longest callout icon, in UTF-16 units (a family emoji is 11). */
const MAX_ICON_LENGTH = 24;

/**
 * Parses Markdown into the block nodes of a page: portable nodes, normalised as
 * `normalizePortable` would (sorted attributes and marks, adjacent text with the same marks
 * joined), with no `id` attributes. The structure follows the schema where it is cheap to:
 * list items and task items start with a paragraph, callouts, toggles' content, quotes and table
 * cells hold at least one block. Empty or blank text gives no nodes.
 */
export function parseMarkdown(text: string): PortableNode[] {
  const lines = toLines(text);
  const content = parseBlocks(lines, { depth: 0, hard: false, inItem: false });
  return parsePortable({ type: "doc", content }).content;
}

/**
 * Nodes as plain JSON, the form ProseMirror and `portableToLoro` take: `attrs`, `content`
 * and `marks` only when there is something in them, `text` on text nodes.
 */
export function portableNodesToJson(nodes: readonly PortableNode[]): JsonValue[] {
  return nodes.map(portableNodeToJson);
}

function portableNodeToJson(portable: PortableNode): JsonValue {
  const json: Obj = { type: portable.type };
  if (portable.attrs !== undefined) {
    json.attrs = portable.attrs;
  }
  if (portable.content.length > 0) {
    json.content = portable.content.map(portableNodeToJson);
  }
  if (portable.marks.length > 0) {
    json.marks = portable.marks.map((mark) => {
      const markJson: Obj = { type: mark.type };
      if (mark.attrs !== undefined) {
        markJson.attrs = mark.attrs;
      }
      return markJson;
    });
  }
  if (portable.text !== undefined) {
    json.text = portable.text;
  }
  return json;
}

// ----- lines ----------------------------------------------------------------------------

/** Where the parser is: how deep in containers, and how line breaks and list items behave. */
interface Context {
  depth: number;
  /** Every line break inside a paragraph is a hard break (table cells). */
  hard: boolean;
  /** Inside a list item: an ordered list that does not start at 1 may interrupt a paragraph. */
  inItem: boolean;
}

/** What a block parser found: the nodes, and the index of the first line after them. */
interface Parsed {
  nodes: Obj[];
  next: number;
}

type BlockParser = (lines: readonly string[], at: number, ctx: Context) => Parsed | undefined;

/** The lines of a text: a BOM and `\r` dropped, a tab in the indentation worth four spaces. */
function toLines(text: string): string[] {
  return text
    .replace(/^\uFEFF/u, "")
    .replace(/\r\n?/gu, "\n")
    .split("\n")
    .map((line) => line.replace(/^[ \t]+/u, (indent) => indent.replaceAll("\t", "    ")));
}

function lineAt(lines: readonly string[], index: number): string {
  return lines[index] ?? "";
}

function isBlank(line: string): boolean {
  return /^[ \t]*$/u.test(line);
}

function indentOf(line: string): number {
  return line.length - line.trimStart().length;
}

/** The line without up to `count` leading spaces. */
function stripIndent(line: string, count: number): string {
  let at = 0;
  while (at < count && line.charAt(at) === " ") {
    at += 1;
  }
  return line.slice(at);
}

// ----- node builders --------------------------------------------------------------------

function node(type: string, attrs?: Obj, content?: Obj[]): Obj {
  const result: Obj = { type };
  if (attrs !== undefined && Object.keys(attrs).length > 0) {
    result.attrs = attrs;
  }
  if (content !== undefined && content.length > 0) {
    result.content = content;
  }
  return result;
}

/** A node holding plain text (code and math blocks, toggle summaries); no text node when empty. */
function textual(type: string, attrs: Obj | undefined, text: string): Obj {
  return node(type, attrs, text === "" ? [] : [{ type: "text", text }]);
}

function emptyParagraph(): Obj {
  return node("paragraph");
}

/** The blocks, or one empty paragraph: the schema's `block+` content never stays empty. */
function orEmpty(blocks: Obj[]): Obj[] {
  return blocks.length > 0 ? blocks : [emptyParagraph()];
}

// ----- blocks ---------------------------------------------------------------------------

const BLOCK_PARSERS: readonly BlockParser[] = [
  fenceAt,
  mathAt,
  detailsAt,
  headingAt,
  ruleAt,
  quoteAt,
  attachmentAt,
  tableAt,
  listAt,
];

/** Parses lines as blocks: the first parser that recognises a line takes it, else a paragraph. */
function parseBlocks(lines: readonly string[], ctx: Context): Obj[] {
  const blocks: Obj[] = [];
  let at = 0;
  while (at < lines.length) {
    if (isBlank(lineAt(lines, at))) {
      at += 1;
      continue;
    }
    let parsed: Parsed | undefined;
    for (const parser of BLOCK_PARSERS) {
      parsed = parser(lines, at, ctx);
      if (parsed !== undefined) {
        break;
      }
    }
    parsed ??= paragraphAt(lines, at, ctx);
    blocks.push(...parsed.nodes);
    at = Math.max(parsed.next, at + 1);
  }
  return blocks;
}

/** Whether the line at `at` ends the paragraph above it by starting another block. */
function interrupts(lines: readonly string[], at: number, ctx: Context): boolean {
  const line = lineAt(lines, at);
  if (
    HEADING.test(line) ||
    fenceOf(line) !== undefined ||
    isRule(line) ||
    QUOTE.test(line) ||
    DETAILS_OPEN.test(line) ||
    line.trim() === "$$" ||
    attachmentBlock(line) !== undefined
  ) {
    return true;
  }
  const marker = markerOf(line);
  // An empty item cannot interrupt a paragraph, and neither can an ordered one that does not
  // start at 1 (a sentence ending in "1999." wrapped onto the next line), except inside an item,
  // where the export writes such a list right under the item's text.
  if (marker !== undefined && marker.rest.trim() !== "" && (marker.kind === "bullet" || marker.number === 1 || ctx.inItem)) {
    return true;
  }
  return tableHead(lines, at) !== undefined;
}

function paragraphAt(lines: readonly string[], at: number, ctx: Context): Parsed {
  const collected = [lineAt(lines, at)];
  let next = at + 1;
  while (next < lines.length && !isBlank(lineAt(lines, next)) && !interrupts(lines, next, ctx)) {
    collected.push(lineAt(lines, next));
    next += 1;
  }
  const text = collected
    .map((line) => line.replace(/^[ \t]+/u, ""))
    .join("\n")
    .trimEnd();
  return { nodes: [node("paragraph", undefined, inlineNodes(text, ctx.hard))], next };
}

// ----- code and math ----------------------------------------------------------------------

const FENCE = /^ *(`{3,}|~{3,})(.*)$/u;

interface Fence {
  char: string;
  length: number;
  info: string;
  indent: number;
}

/** The opening fence a line is, if it is one (an info string after backticks has none). */
function fenceOf(line: string): Fence | undefined {
  const match = FENCE.exec(line);
  const fence = match?.[1];
  if (match === null || fence === undefined) {
    return undefined;
  }
  const info = match[2] ?? "";
  if (fence.startsWith("`") && info.includes("`")) {
    return undefined;
  }
  return { char: fence.charAt(0), length: fence.length, info: info.trim(), indent: indentOf(line) };
}

function isFenceClose(line: string, open: Fence): boolean {
  const text = line.trim();
  return text.length >= open.length && text === open.char.repeat(text.length);
}

function fenceAt(lines: readonly string[], at: number): Parsed | undefined {
  const open = fenceOf(lineAt(lines, at));
  if (open === undefined) {
    return undefined;
  }
  const code: string[] = [];
  let next = at + 1;
  let closed = false;
  while (next < lines.length) {
    const line = lineAt(lines, next);
    next += 1;
    if (isFenceClose(line, open)) {
      closed = true;
      break;
    }
    code.push(stripIndent(line, open.indent));
  }
  if (!closed) {
    // Nothing closes the fence: the code runs to the end, minus the blank lines the text ends with.
    while (isBlank(code.at(-1) ?? "x")) {
      code.pop();
    }
  }
  const language = open.info.split(/\s+/u)[0] ?? "";
  return { nodes: [textual("codeBlock", language === "" ? undefined : { language }, code.join("\n"))], next };
}

function mathAt(lines: readonly string[], at: number): Parsed | undefined {
  const text = lineAt(lines, at).trim();
  if (text === "$$") {
    for (let end = at + 1; end < lines.length; end += 1) {
      if (lineAt(lines, end).trim() === "$$") {
        return { nodes: [textual("mathBlock", undefined, lines.slice(at + 1, end).join("\n"))], next: end + 1 };
      }
    }
    return undefined;
  }
  if (text.length > 4 && text.startsWith("$$") && text.endsWith("$$")) {
    const inner = text.slice(2, -2).trim();
    if (inner !== "" && !inner.includes("$$")) {
      return { nodes: [textual("mathBlock", undefined, inner)], next: at + 1 };
    }
  }
  return undefined;
}

// ----- toggles ----------------------------------------------------------------------------

const DETAILS_OPEN = /^[ \t]*<details(?:[ \t]+open(?:=(?:"[^"]*"|'[^']*'|[^\s>]*))?)?[ \t]*>[ \t]*$/iu;
const DETAILS_CLOSE = /^[ \t]*<\/details>[ \t]*$/iu;
const SUMMARY = /^[ \t]*<summary>(.*)<\/summary>[ \t]*$/iu;
/** A summary that is one heading element: a toggle heading (ADR-0040). Deeper levels clamp to 3, as headings do. */
const SUMMARY_HEADING = /^<h([1-6])(?:[ \t][^>]*)?>(.*)<\/h\1>$/iu;

function detailsAt(lines: readonly string[], at: number, ctx: Context): Parsed | undefined {
  if (!DETAILS_OPEN.test(lineAt(lines, at)) || ctx.depth >= MAX_DEPTH) {
    return undefined;
  }
  let depth = 0;
  let end = -1;
  for (let next = at + 1; next < lines.length && end < 0; next += 1) {
    const line = lineAt(lines, next);
    if (DETAILS_OPEN.test(line)) {
      depth += 1;
    } else if (DETAILS_CLOSE.test(line)) {
      if (depth === 0) {
        end = next;
      } else {
        depth -= 1;
      }
    }
  }
  if (end < 0) {
    return undefined; // never closed: plain text
  }
  const body = lines.slice(at + 1, end);
  let summary = "";
  let level: number | undefined;
  const summaryAt = body.findIndex((line) => !isBlank(line));
  const summaryMatch = summaryAt < 0 ? null : SUMMARY.exec(lineAt(body, summaryAt));
  if (summaryMatch !== null) {
    summary = summaryMatch[1] ?? "";
    body.splice(summaryAt, 1);
    const heading = SUMMARY_HEADING.exec(summary.trim());
    if (heading !== null && !/<\/h[1-6]/iu.test(heading[2] ?? "")) {
      level = Math.min(3, Number(heading[1]));
      summary = heading[2] ?? "";
    }
  }
  const open = /\bopen\b/iu.test(lineAt(lines, at));
  const blocks = parseBlocks(body, { ...ctx, depth: ctx.depth + 1 });
  return {
    nodes: [
      node("details", open ? { open: true } : undefined, [
        textual("detailsSummary", level === undefined ? undefined : { level }, summary),
        node("detailsContent", undefined, orEmpty(blocks)),
      ]),
    ],
    next: end + 1,
  };
}

// ----- headings and dividers --------------------------------------------------------------

const HEADING = /^[ \t]*(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/u;
const RULE = /^[ \t]*(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$/u;

function isRule(line: string): boolean {
  return RULE.test(line);
}

function headingAt(lines: readonly string[], at: number, ctx: Context): Parsed | undefined {
  const match = HEADING.exec(lineAt(lines, at));
  const hashes = match?.[1];
  if (match === null || hashes === undefined) {
    return undefined;
  }
  const level = Math.min(3, hashes.length);
  return { nodes: [node("heading", { level }, inlineNodes(match[2] ?? "", ctx.hard))], next: at + 1 };
}

function ruleAt(lines: readonly string[], at: number): Parsed | undefined {
  return isRule(lineAt(lines, at)) ? { nodes: [node("horizontalRule")], next: at + 1 } : undefined;
}

// ----- quotes and callouts ----------------------------------------------------------------

const QUOTE = /^[ \t]*>[ ]?/u;
const ICON_START = /^[\p{Extended_Pictographic}\p{So}\p{Sk}\p{Regional_Indicator}]/u;
const ICON_BODY = /^[\p{Extended_Pictographic}\p{So}\p{Sk}\p{Regional_Indicator}\p{M}\p{Cf}]+$/u;
const KEYCAP = /^[0-9#*]\uFE0F?\u20E3$/u;

/**
 * Whether a word is a callout icon: an emoji (sequences with joiners, variation selectors,
 * skin tones, flags and keycaps included) or another non-ASCII symbol. Never a letter, a digit
 * or ASCII punctuation, so `> - item`, `> **bold**` and `> Note: x` stay quotes.
 */
function isIcon(word: string): boolean {
  return word.length <= MAX_ICON_LENGTH && (KEYCAP.test(word) || (ICON_START.test(word) && ICON_BODY.test(word)));
}

/** The icon a quote's first line starts with (alone, or in front of a space) and the lines without it. */
function calloutOf(inner: readonly string[]): { icon: string; lines: string[] } | undefined {
  const match = /^[ \t]*(\S+)(?:[ \t]+(.*))?$/u.exec(lineAt(inner, 0));
  const icon = match?.[1];
  if (match === null || icon === undefined || !isIcon(icon)) {
    return undefined;
  }
  return { icon, lines: [match[2] ?? "", ...inner.slice(1)] };
}

function quoteAt(lines: readonly string[], at: number, ctx: Context): Parsed | undefined {
  if (!QUOTE.test(lineAt(lines, at)) || ctx.depth >= MAX_DEPTH) {
    return undefined;
  }
  const inner: string[] = [];
  let next = at;
  while (next < lines.length && QUOTE.test(lineAt(lines, next))) {
    inner.push(lineAt(lines, next).replace(QUOTE, ""));
    next += 1;
  }
  const child = { ...ctx, depth: ctx.depth + 1 };
  const callout = calloutOf(inner);
  if (callout !== undefined) {
    return { nodes: [node("callout", { icon: callout.icon }, orEmpty(parseBlocks(callout.lines, child)))], next };
  }
  return { nodes: [node("blockquote", undefined, orEmpty(parseBlocks(inner, child)))], next };
}

// ----- attachments ------------------------------------------------------------------------

const IMAGE_LINE = /^!\[([^\]]*)\]\(attachment:\/\/([^)\s]*)(?:\s+"((?:[^"\\]|\\.)*)")?\)$/u;
const FILE_LINE = /^\[([^\]]*)\]\(attachment:\/\/([^)\s]*)\)$/u;

/** The `image` or `file` block a line is on its own, when the attachment id is valid. */
function attachmentBlock(line: string): Obj | undefined {
  const text = line.trim();
  const image = IMAGE_LINE.exec(text);
  const file = image === null ? FILE_LINE.exec(text) : null;
  const match = image ?? file;
  const attachmentId = canonicalId(match?.[2] ?? "");
  if (match === null || attachmentId === undefined) {
    return undefined;
  }
  if (image !== null) {
    const alt = image[1] ?? "";
    const caption = (image[3] ?? "").replace(/\\([\\"])/gu, "$1");
    return node("image", {
      ...(alt === "" ? {} : { alt }),
      attachmentId,
      ...(caption === "" ? {} : { caption }),
    });
  }
  return node("file", { attachmentId });
}

function attachmentAt(lines: readonly string[], at: number): Parsed | undefined {
  const block = attachmentBlock(lineAt(lines, at));
  return block === undefined ? undefined : { nodes: [block], next: at + 1 };
}

// ----- tables -----------------------------------------------------------------------------

const DELIMITER_CELL = /^:?-+:?$/u;

/** The cells of a row line: outer pipes dropped, split at the pipes not escaped (`\|` stays in the cell). */
function splitRow(line: string): string[] {
  let text = line.trim();
  if (text.startsWith("|")) {
    text = text.slice(1);
  }
  if (text.endsWith("|") && !text.endsWith("\\|")) {
    text = text.slice(0, -1);
  }
  const cells: string[] = [];
  let cell = "";
  for (let at = 0; at < text.length; at += 1) {
    const char = text.charAt(at);
    if (char === "|" && text.charAt(at - 1) !== "\\") {
      cells.push(cell);
      cell = "";
    } else {
      cell += char;
    }
  }
  cells.push(cell);
  return cells.map((text) => text.trim());
}

/** The header cells when the line at `at` and the next make the start of a table. */
function tableHead(lines: readonly string[], at: number): string[] | undefined {
  const header = lineAt(lines, at);
  const delimiter = lineAt(lines, at + 1);
  if (!header.includes("|") || !delimiter.includes("|")) {
    return undefined;
  }
  const cells = splitRow(header);
  const delimiters = splitRow(delimiter);
  const sameWidth = cells.length === delimiters.length;
  return sameWidth && delimiters.every((cell) => DELIMITER_CELL.test(cell)) ? cells : undefined;
}

function tableAt(lines: readonly string[], at: number, ctx: Context): Parsed | undefined {
  const header = tableHead(lines, at);
  if (header === undefined || ctx.depth >= MAX_DEPTH) {
    return undefined;
  }
  const width = header.length;
  const rows = [header];
  let next = at + 2;
  while (next < lines.length) {
    const line = lineAt(lines, next);
    if (isBlank(line) || !line.includes("|") || HEADING.test(line) || fenceOf(line) !== undefined || QUOTE.test(line)) {
      break;
    }
    const cells = splitRow(line).slice(0, width);
    while (cells.length < width) {
      cells.push("");
    }
    rows.push(cells);
    next += 1;
  }
  const child: Context = { depth: ctx.depth + 1, hard: true, inItem: false };
  const content = rows.map((row, index) =>
    node(
      "tableRow",
      undefined,
      row.map((cell) => node(index === 0 ? "tableHeader" : "tableCell", undefined, cellBlocks(cell, child))),
    ),
  );
  return { nodes: [node("table", undefined, content)], next };
}

/** The blocks of a cell: `\|` is a pipe and every `<br>` a line break, which is what separates blocks. */
function cellBlocks(cell: string, ctx: Context): Obj[] {
  const text = cell.replaceAll("\\|", "|").replace(/<br\s*\/?>/giu, "\n");
  return orEmpty(parseBlocks(text.split("\n"), ctx));
}

// ----- lists ------------------------------------------------------------------------------

const BULLET = /^( *)([-*+])([ \t]*)(.*)$/u;
const ORDERED = /^( *)(\d{1,9})([.)])([ \t]*)(.*)$/u;
const TASK = /^\[([ xX])\](?:[ \t]+(.*)|[ \t]*$)/u;

/** A list item marker at the start of a line. */
interface Marker {
  indent: number;
  kind: "bullet" | "ordered";
  /** `-`, `*`, `+`, `.` or `)`: a list continues only with the same one. */
  char: string;
  /** The number of an ordered item. */
  number: number;
  /** The column the item's content starts at. */
  offset: number;
  /** The item's first line, after the marker. */
  rest: string;
}

function markerOf(line: string): Marker | undefined {
  const bullet = BULLET.exec(line);
  if (bullet !== null) {
    const [, spaces = "", char = "", gap = "", rest = ""] = bullet;
    return markerFrom(spaces.length, "bullet", char, 0, 1, gap, rest);
  }
  const ordered = ORDERED.exec(line);
  if (ordered !== null) {
    const [, spaces = "", digits = "", char = "", gap = "", rest = ""] = ordered;
    return markerFrom(spaces.length, "ordered", char, Number(digits), digits.length + 1, gap, rest);
  }
  return undefined;
}

/** A marker, unless text is stuck to it (`-text`, `1.5`): a marker needs a space or the end of the line. */
function markerFrom(
  indent: number,
  kind: Marker["kind"],
  char: string,
  number: number,
  width: number,
  gap: string,
  rest: string,
): Marker | undefined {
  if (gap === "" && rest !== "") {
    return undefined;
  }
  // Up to four spaces after the marker belong to it; more are indentation of the content.
  const spaces = gap.replaceAll("\t", "    ").length;
  const used = rest === "" ? 1 : spaces > 4 ? 1 : spaces;
  return { indent, kind, char, number, offset: indent + width + used, rest: " ".repeat(Math.max(0, spaces - used)) + rest };
}

function sameList(marker: Marker, first: Marker): boolean {
  return marker.kind === first.kind && marker.char === first.char;
}

/** One item of a list being read: its marker and its lines, without the indentation of its content. */
interface Item {
  marker: Marker;
  lines: string[];
}

function listAt(lines: readonly string[], at: number, ctx: Context): Parsed | undefined {
  const first = markerOf(lineAt(lines, at));
  if (first === undefined || isRule(lineAt(lines, at)) || ctx.depth >= MAX_DEPTH) {
    return undefined;
  }
  const base = first.indent;
  const items: Item[] = [];
  let next = at;
  for (;;) {
    // Between items: blank lines may separate them, only another item of this list continues it.
    let start = next;
    while (start < lines.length && isBlank(lineAt(lines, start))) {
      start += 1;
    }
    const line = lineAt(lines, start);
    const marker = start < lines.length ? markerOf(line) : undefined;
    if (marker === undefined || !sameList(marker, first) || marker.indent >= base + 2 || isRule(line)) {
      break;
    }
    const item: Item = { marker, lines: [marker.rest] };
    next = start + 1;
    while (next < lines.length) {
      const current = lineAt(lines, next);
      if (isBlank(current)) {
        // Blank lines belong to the item when its content goes on after them.
        let after = next;
        while (after < lines.length && isBlank(lineAt(lines, after))) {
          after += 1;
        }
        if (after < lines.length && indentOf(lineAt(lines, after)) >= base + 2) {
          for (; next < after; next += 1) {
            item.lines.push("");
          }
          continue;
        }
        break;
      }
      const indent = indentOf(current);
      if (indent >= base + 2) {
        item.lines.push(stripIndent(current, marker.offset));
        next += 1;
        continue;
      }
      // A line that is not indented under the item ends it, even a plain one that CommonMark would
      // fold into the item ("lazy continuation"): in notes pasted as text it is more likely the next
      // paragraph ("- a", "- b", "Thanks!") than the wrapped end of an item.
      break;
    }
    items.push(item);
  }
  return { nodes: listNodes(items, { ...ctx, depth: ctx.depth + 1, inItem: true }), next };
}

/** A task item's checkbox state and the lines after the checkbox, or `undefined` for other items. */
function taskOf(item: Item): { checked: boolean; lines: string[] } | undefined {
  if (item.marker.kind !== "bullet") {
    return undefined;
  }
  const match = TASK.exec(item.lines[0] ?? "");
  const state = match?.[1];
  if (match === null || state === undefined) {
    return undefined;
  }
  return { checked: state !== " ", lines: [match[2] ?? "", ...item.lines.slice(1)] };
}

/** The content of an item: blocks that start with a paragraph (an empty one if need be). */
function itemBlocks(lines: readonly string[], ctx: Context): Obj[] {
  const blocks = parseBlocks(lines, ctx);
  return blocks[0]?.type === "paragraph" ? blocks : [emptyParagraph(), ...blocks];
}

/** Runs of task items and of other items are lists of their own. */
function listNodes(items: readonly Item[], ctx: Context): Obj[] {
  const lists: Obj[] = [];
  let run: Obj[] = [];
  let runItems: Item[] = [];
  let runIsTask = false;
  const close = (): void => {
    const marker = runItems[0]?.marker;
    if (marker !== undefined) {
      if (runIsTask) {
        lists.push(node("taskList", undefined, run));
      } else if (marker.kind === "ordered") {
        lists.push(node("orderedList", marker.number === 1 ? undefined : { start: marker.number }, run));
      } else {
        lists.push(node("bulletList", undefined, run));
      }
    }
    run = [];
    runItems = [];
  };
  for (const item of items) {
    const task = taskOf(item);
    if (runItems.length > 0 && runIsTask !== (task !== undefined)) {
      close();
    }
    runIsTask = task !== undefined;
    runItems.push(item);
    if (task === undefined) {
      run.push(node("listItem", undefined, itemBlocks(item.lines, ctx)));
    } else {
      run.push(node("taskItem", { checked: task.checked }, itemBlocks(task.lines, ctx)));
    }
  }
  close();
  return lists;
}

// ----- inline -----------------------------------------------------------------------------

/** The marks Markdown can express on a run of text. */
interface Marks {
  bold: boolean;
  italic: boolean;
  strike: boolean;
  underline: boolean;
  code: boolean;
  link: string | undefined;
}

/**
 * A piece of a paragraph while its emphasis is being resolved; only `text`, `break`, `math` and
 * `date` remain after.
 */
interface Piece {
  kind: "text" | "break" | "delim" | "tag" | "math" | "date";
  /** The text; the character of a delimiter run; `<u>` or `</u>` for a tag; the LaTeX of an equation. */
  text: string;
  /** The attributes of a `date` (a `<time>` element read as a `dateMention`). */
  attrs?: Obj;
  /** Delimiters left in the run, and at the start. */
  count: number;
  orig: number;
  canOpen: boolean;
  canClose: boolean;
  /** A delimiter between a matched pair, which no longer matches anything. */
  dead: boolean;
  /** A `<u>` tag: opening, not closing. */
  open: boolean;
  /** A tag that was paired and is now marks, not text. */
  gone: boolean;
  marks: Marks;
}

function plainMarks(): Marks {
  return { bold: false, italic: false, strike: false, underline: false, code: false, link: undefined };
}

function piece(kind: Piece["kind"], text: string, marks: Marks = plainMarks()): Piece {
  return { kind, text, count: 0, orig: 0, canOpen: false, canClose: false, dead: false, open: false, gone: false, marks };
}

/** The inline nodes of a paragraph's text (lines joined by `\n`, outer whitespace trimmed). */
function inlineNodes(text: string, hard: boolean): Obj[] {
  return resolve(tokenize(text, hard, 0)).map(pieceNode);
}

function pieceNode(item: Piece): Obj {
  if (item.kind === "break") {
    return { type: "hardBreak" };
  }
  if (item.kind === "math") {
    return { type: "mathInline", attrs: { latex: item.text } };
  }
  if (item.kind === "date") {
    return { type: "dateMention", attrs: item.attrs ?? {} };
  }
  const marks: Obj[] = [];
  for (const name of ["bold", "italic", "strike", "underline", "code"] as const) {
    if (item.marks[name]) {
      marks.push({ type: name });
    }
  }
  if (item.marks.link !== undefined) {
    marks.push({ type: "link", attrs: { href: item.marks.link } });
  }
  return marks.length > 0 ? { type: "text", text: item.text, marks } : { type: "text", text: item.text };
}

const WHITESPACE = /^\s$/u;
const PUNCTUATION = /^[\p{P}\p{S}]$/u;

/** The character before `at` (a whole code point), or a space at the start. */
function charBefore(text: string, at: number): string {
  if (at <= 0) {
    return " ";
  }
  const unit = text.charCodeAt(at - 1);
  if (unit >= 0xdc00 && unit <= 0xdfff && at >= 2) {
    return text.slice(at - 2, at);
  }
  return text.charAt(at - 1);
}

/** The character at `at` (a whole code point), or a space at the end. */
function charAfter(text: string, at: number): string {
  const point = text.codePointAt(at);
  return point === undefined ? " " : String.fromCodePoint(point);
}

function runLength(text: string, at: number, char: string): number {
  let end = at;
  while (text.charAt(end) === char) {
    end += 1;
  }
  return end - at;
}

/** A code span: where it ends (after its closing backticks) and its text. */
interface CodeSpan {
  end: number;
  content: string;
}

/**
 * The code spans of a text by the index of their opening backticks. A run of backticks opens a
 * span when a run of the same length follows; the first one closes it. Found once for the whole
 * text with a lookup per run length, so a text of unmatched backticks stays linear.
 */
function findCodeSpans(text: string): Map<number, CodeSpan> {
  const spans = new Map<number, CodeSpan>();
  const runs: { start: number; length: number }[] = [];
  const byLength = new Map<number, number[]>();
  const pattern = /`+/gu;
  for (let match = pattern.exec(text); match !== null; match = pattern.exec(text)) {
    const length = match[0].length;
    const same = byLength.get(length);
    if (same === undefined) {
      byLength.set(length, [runs.length]);
    } else {
      same.push(runs.length);
    }
    runs.push({ start: match.index, length });
  }
  const pointers = new Map<number, number>();
  let at = 0;
  while (at < runs.length) {
    const run = runs[at];
    const same = run === undefined ? [] : (byLength.get(run.length) ?? []);
    let pointer = pointers.get(run?.length ?? 0) ?? 0;
    while (pointer < same.length && (same[pointer] ?? 0) <= at) {
      pointer += 1;
    }
    pointers.set(run?.length ?? 0, pointer);
    const closer = runs[same[pointer] ?? -1];
    if (run === undefined || closer === undefined) {
      at += 1;
      continue;
    }
    spans.set(run.start, {
      end: closer.start + closer.length,
      content: text.slice(run.start + run.length, closer.start),
    });
    at = (same[pointer] ?? at) + 1;
  }
  return spans;
}

/** The `]` matching each `[` that has one, code spans left out. */
function matchBrackets(text: string, spans: ReadonlyMap<number, CodeSpan>): Map<number, number> {
  const matches = new Map<number, number>();
  const open: number[] = [];
  let at = 0;
  while (at < text.length) {
    const span = spans.get(at);
    if (span !== undefined) {
      at = span.end;
      continue;
    }
    const char = text.charAt(at);
    if (char === "[") {
      open.push(at);
    } else if (char === "]") {
      const start = open.pop();
      if (start !== undefined) {
        matches.set(start, at);
      }
    }
    at += 1;
  }
  return matches;
}

/**
 * The destination of a link starting right after its `(`: `href`, an optional title (dropped)
 * and the closing `)`. `end` is the index after the `)`. A href has no spaces and balanced
 * parentheses, or is written `<...>`.
 */
function destinationAt(text: string, start: number): { href: string; end: number } | undefined {
  const skip = (from: number): number => {
    let at = from;
    while (/[ \t\n]/u.test(text.charAt(at))) {
      at += 1;
    }
    return at;
  };
  let at = skip(start);
  let href: string;
  if (text.charAt(at) === "<") {
    // Looked for in a window only: a text of unclosed `<` stays linear.
    const window = text.slice(at + 1, at + 2 + MAX_DESTINATION);
    const close = window.indexOf(">");
    const inner = close < 0 ? "<" : window.slice(0, close);
    if (close < 0 || inner.includes("<") || inner.includes("\n")) {
      return undefined;
    }
    href = inner;
    at += close + 2;
  } else {
    const from = at;
    let depth = 0;
    while (at < text.length && at - from <= MAX_DESTINATION) {
      const char = text.charAt(at);
      if (char === "(") {
        depth += 1;
      } else if (char === ")") {
        if (depth === 0) {
          break;
        }
        depth -= 1;
      } else if (/\s/u.test(char)) {
        break;
      }
      at += 1;
    }
    if (depth !== 0) {
      return undefined;
    }
    href = text.slice(from, at);
  }
  const hrefEnd = at;
  at = skip(hrefEnd);
  const opener = text.charAt(at);
  if (at > hrefEnd && (opener === '"' || opener === "'" || opener === "(")) {
    // A title in quotes or parentheses after a space is dropped.
    const close = text.slice(at + 1, at + 2 + MAX_DESTINATION).indexOf(opener === "(" ? ")" : opener);
    if (close >= 0) {
      at = skip(at + close + 2);
    }
  }
  return text.charAt(at) === ")" && href !== "" ? { href, end: at + 1 } : undefined;
}

/** The link whose text opens at `open`, when the bracket closes, a destination follows and it is not empty. */
function linkAt(
  text: string,
  open: number,
  brackets: ReadonlyMap<number, number>,
): { close: number; href: string; end: number } | undefined {
  const close = brackets.get(open);
  if (close === undefined || text.charAt(close + 1) !== "(") {
    return undefined;
  }
  const destination = destinationAt(text, close + 2);
  return destination === undefined ? undefined : { close, ...destination };
}

/**
 * The inline equation that opens at `at` (a single `$`, not after a backslash): where it ends (after
 * its closing `$`) and its LaTeX, or `undefined` when what follows is not one. The first `$` after
 * the opening one closes it, on the same line; the character after the opening one and the one before
 * the closing one are no whitespace, that before it is no backslash, and a digit does not follow the
 * closing one. The scan stops at the first `$` or line break, so a text of `$` stays linear.
 */
function mathSpanAt(text: string, at: number): { end: number; latex: string } | undefined {
  if (WHITESPACE.test(charAfter(text, at + 1))) {
    return undefined; // also the end of the text, which `charAfter` reads as a space
  }
  let close = at + 1;
  while (close < text.length && text.charAt(close) !== "$" && text.charAt(close) !== "\n") {
    close += 1;
  }
  if (text.charAt(close) !== "$") {
    return undefined;
  }
  const last = text.charAt(close - 1);
  if (WHITESPACE.test(last) || last === "\\" || /^[0-9]$/u.test(text.charAt(close + 1))) {
    return undefined;
  }
  return { end: close + 1, latex: text.slice(at + 1, close) };
}

/** `<time ...>text</time>`: the element's attributes and a short text with no tag in it (sticky: matched where it starts). */
const TIME_ELEMENT = /<time(\s[^>]{0,400})?>[^<]{0,200}<\/time>/iuy;
/** One attribute of an element: a name and a double-quoted, single-quoted or bare value. */
const HTML_ATTRIBUTE = /([A-Za-z_:][\w:.-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))/gu;

/**
 * A `<time>` element at `at` that reads as a date mention, as `kb-import` reads it: `datetime` is a date (`parseDate`),
 * `data-reminder` a non-negative whole number of minutes, `data-relative` `today` or `now`; the element's text is ignored.
 * `undefined` when there is no such element or its `datetime` is no date (it stays text).
 */
function timeElementAt(text: string, at: number): { end: number; attrs: Obj } | undefined {
  TIME_ELEMENT.lastIndex = at;
  const match = TIME_ELEMENT.exec(text);
  if (match === null) {
    return undefined;
  }
  const values = new Map<string, string>();
  for (const attribute of (match[1] ?? "").matchAll(HTML_ATTRIBUTE)) {
    const name = (attribute[1] ?? "").toLowerCase();
    if (!values.has(name)) {
      values.set(name, attribute[2] ?? attribute[3] ?? attribute[4] ?? "");
    }
  }
  const value = parseDate(values.get("datetime") ?? "");
  if (value === undefined) {
    return undefined;
  }
  const attrs: Obj = { date: value.date };
  if (value.time !== undefined) {
    attrs.time = value.time;
  }
  const minutes = (values.get("data-reminder") ?? "").trim();
  const reminder = /^\+?\d+$/u.test(minutes) ? reminderMinutesOf(Number(minutes)) : null;
  if (reminder !== null) {
    attrs.reminder = reminder;
  }
  const relative = (values.get("data-relative") ?? "").trim();
  if (relative === "today" || relative === "now") {
    attrs.relative = relative;
  }
  return { end: at + match[0].length, attrs };
}

/**
 * Splits a text into pieces: text, code spans, links (their text resolved on its own), inline
 * equations, hard breaks, `<u>` tags and runs of emphasis delimiters with what they can open and
 * close.
 */
function tokenize(text: string, hard: boolean, depth: number): Piece[] {
  const spans = findCodeSpans(text);
  const brackets = matchBrackets(text, spans);
  const pieces: Piece[] = [];
  let buffer = "";
  const flush = (): void => {
    if (buffer !== "") {
      pieces.push(piece("text", buffer));
      buffer = "";
    }
  };
  let at = 0;
  while (at < text.length) {
    const char = text.charAt(at);
    if (char === "`") {
      const span = spans.get(at);
      if (span === undefined) {
        const length = runLength(text, at, "`");
        buffer += "`".repeat(length);
        at += length;
      } else {
        flush();
        pieces.push(piece("text", span.content.replaceAll("\n", " "), { ...plainMarks(), code: true }));
        at = span.end;
      }
      continue;
    }
    if (char === "<") {
      const tag = /^<(\/?)u>/iu.exec(text.slice(at, at + 4));
      if (tag !== null) {
        flush();
        const item = piece("tag", tag[1] === "/" ? "</u>" : "<u>");
        item.open = tag[1] !== "/";
        pieces.push(item);
        at += tag[0].length;
        continue;
      }
      const time = timeElementAt(text, at);
      if (time !== undefined) {
        flush();
        const item = piece("date", "");
        item.attrs = time.attrs;
        pieces.push(item);
        at = time.end;
        continue;
      }
    }
    if (char === "!" && text.charAt(at + 1) === "[") {
      // An image that is not a block (another source, or inside text): its source stays text.
      const image = linkAt(text, at + 1, brackets);
      if (image !== undefined) {
        buffer += text.slice(at, image.end);
        at = image.end;
        continue;
      }
    }
    if (char === "[") {
      const link = depth < MAX_LINK_DEPTH ? linkAt(text, at, brackets) : undefined;
      if (link !== undefined) {
        flush();
        for (const inner of resolve(tokenize(text.slice(at + 1, link.close), hard, depth + 1))) {
          inner.marks.link ??= link.href;
          pieces.push(inner);
        }
        at = link.end;
        continue;
      }
    }
    if (char === "$") {
      const length = runLength(text, at, "$");
      const math = length === 1 && text.charAt(at - 1) !== "\\" ? mathSpanAt(text, at) : undefined;
      if (math === undefined) {
        buffer += "$".repeat(length); // `$$` is the block's fence, not an equation
        at += length;
        continue;
      }
      flush();
      pieces.push(piece("math", math.latex));
      at = math.end;
      continue;
    }
    if (char === "*" || char === "_" || char === "~") {
      const length = runLength(text, at, char);
      // `~` marks strike only in pairs, and a backslash in front of a delimiter keeps it literal
      // (nothing is unescaped: the backslash stays, so escaped Markdown reads as it was typed).
      if ((char === "~" && length !== 2) || text.charAt(at - 1) === "\\") {
        buffer += char.repeat(length);
        at += length;
        continue;
      }
      flush();
      pieces.push(delimiter(text, at, char, length));
      at += length;
      continue;
    }
    if (char === "\n") {
      const lineBreak = hard || buffer.endsWith("  ");
      let end = buffer.length;
      while (end > 0 && (buffer.charAt(end - 1) === " " || buffer.charAt(end - 1) === "\t")) {
        end -= 1;
      }
      buffer = buffer.slice(0, end);
      if (lineBreak) {
        flush();
        pieces.push(piece("break", ""));
      } else {
        buffer += " ";
      }
      at += 1;
      continue;
    }
    buffer += char;
    at += 1;
  }
  flush();
  return pieces;
}

/** A run of `length` emphasis characters at `at`, with what CommonMark's flanking rules let it do. */
function delimiter(text: string, at: number, char: string, length: number): Piece {
  const before = charBefore(text, at);
  const after = charAfter(text, at + length);
  const spaceBefore = WHITESPACE.test(before);
  const spaceAfter = WHITESPACE.test(after);
  const punctBefore = PUNCTUATION.test(before);
  const punctAfter = PUNCTUATION.test(after);
  const left = !spaceAfter && (!punctAfter || spaceBefore || punctBefore);
  const right = !spaceBefore && (!punctBefore || spaceAfter || punctAfter);
  const item = piece("delim", char);
  item.count = length;
  item.orig = length;
  item.canOpen = char === "_" ? left && (!right || punctBefore) : left;
  item.canClose = char === "_" ? right && (!left || punctAfter) : right;
  return item;
}

/** Resolves emphasis and `<u>` tags into marks on the text between them, and leaves text and breaks. */
function resolve(pieces: Piece[]): Piece[] {
  matchEmphasis(pieces);
  matchUnderline(pieces);
  const settled: Piece[] = [];
  for (const item of pieces) {
    if (item.kind === "delim") {
      if (item.count > 0) {
        settled.push(piece("text", item.text.repeat(item.count), item.marks));
      }
    } else if (item.kind === "tag") {
      if (!item.gone) {
        settled.push(piece("text", item.text, item.marks));
      }
    } else {
      settled.push(item);
    }
  }
  return settled;
}

/**
 * CommonMark's emphasis algorithm over the delimiter runs: each closer looks back for the
 * nearest opener of the same character (the rule of three applied); two characters of both make
 * bold, one italic, `~~` strike. The marks go on every piece in between, the unused characters
 * of a run stay text, and the runs in between can no longer match.
 */
function matchEmphasis(pieces: Piece[]): void {
  const stack: Piece[] = [];
  const positions: number[] = [];
  pieces.forEach((item, index) => {
    if (item.kind === "delim") {
      stack.push(item);
      positions.push(index);
    }
  });
  if (stack.length > MAX_DELIMITERS) {
    return;
  }
  stack.forEach((closer, c) => {
    while (closer.canClose && closer.count > 0 && !closer.dead) {
      let found = -1;
      for (let o = c - 1; o >= 0 && found < 0; o -= 1) {
        const opener = stack[o];
        if (opener === undefined || opener.dead || opener.text !== closer.text || !opener.canOpen || opener.count === 0) {
          continue;
        }
        const bothThirds = opener.orig % 3 === 0 && closer.orig % 3 === 0;
        if ((opener.canClose || closer.canOpen) && (opener.orig + closer.orig) % 3 === 0 && !bothThirds) {
          continue;
        }
        found = o;
      }
      const opener = stack[found];
      if (opener === undefined) {
        break;
      }
      const used = closer.text === "~" || (opener.count >= 2 && closer.count >= 2) ? 2 : 1;
      const mark = closer.text === "~" ? "strike" : used === 2 ? "bold" : "italic";
      for (let k = (positions[found] ?? 0) + 1; k < (positions[c] ?? 0); k += 1) {
        const between = pieces[k];
        if (between !== undefined) {
          between.marks[mark] = true;
        }
      }
      for (let k = found + 1; k < c; k += 1) {
        const between = stack[k];
        if (between !== undefined) {
          between.dead = true;
        }
      }
      opener.count -= used;
      closer.count -= used;
    }
  });
}

/** Pairs each `</u>` with the nearest unpaired `<u>` before it; the pieces in between are underlined. */
function matchUnderline(pieces: Piece[]): void {
  const open: number[] = [];
  pieces.forEach((item, index) => {
    if (item.kind !== "tag") {
      return;
    }
    if (item.open) {
      open.push(index);
      return;
    }
    const start = open.pop();
    const opening = start === undefined ? undefined : pieces[start];
    if (start === undefined || opening === undefined) {
      return;
    }
    for (let k = start + 1; k < index; k += 1) {
      const between = pieces[k];
      if (between !== undefined) {
        between.marks.underline = true;
      }
    }
    opening.gone = true;
    item.gone = true;
  });
}

