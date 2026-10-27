// Paste Markdown as blocks (feature-gap analysis, "Paste Markdown as blocks"; M3 follow-up 6).
//
// Plain text that is Markdown (a heading, a list, a code fence, a quote, a table, `$$`, a
// divider, or text with `**bold**`, backticks, `[links](x)` or an equation `$x$` in it) pastes as the
// blocks it stands for instead of as literal text. `parseMarkdown` of core-client reads it (the inverse
// of the Markdown export, which `renderText` of the image and file blocks and the "Copy as
// Markdown" action write), this file turns what it found into nodes of the live schema and puts
// them in the page in one transaction (so the Loro binding writes one update, and one undo step
// takes the paste back).
//
// The handler leaves a paste alone, for the editor's default handling, when
// - the clipboard carries real HTML (a page, a document, a spreadsheet: those paste as the rich
//   text they are); HTML that only wraps the text (`<pre>`, `<p>`, `<span>`, `<div>`, as
//   terminals and code editors make) does not count;
// - the text has nothing the parser would turn into structure or marks (it pastes as text, as
//   it always did);
// - the selection is in a code block or a math block (code is pasted as plain text) or in
//   inline code, the editor is read-only, the text is huge, or Shift is held (paste as plain
//   text is a way to get the Markdown itself).
//
// What the parser finds never reaches the schema unchecked: only the node and mark names it
// writes are built, a node whose content does not fit its type is repaired the way the schema
// asks (an item starts with a paragraph) or dropped, and a link keeps its mark only for schemes
// the app opens.

import { Extension } from "@tiptap/core";
import { Fragment, Mark, Slice } from "@tiptap/pm/model";
import type { Attrs, Node as PmNode, NodeType, ResolvedPos, Schema } from "@tiptap/pm/model";
import { Plugin, PluginKey, Selection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { parseMarkdown, portableNodesToJson } from "@atomik-kb/core-client";
import type { JsonValue, PortableNode } from "@atomik-kb/core-client";

import { isIsoDate, isIsoTime } from "./dates";
import { dateRelativeOf } from "./template-button";

/** Longer text is pasted as it comes: one transaction of that size would stall the editor. */
const MAX_PASTE_CHARS = 500_000;

/**
 * The inline atoms `parseMarkdown` writes: an equation, `$latex$` in the text, and a date mention, the `<time datetime>`
 * element of a file the app exported.
 */
const PARSED_INLINE_ATOMS: ReadonlySet<string> = new Set(["mathInline", "dateMention"]);

/** The block nodes `parseMarkdown` writes (any other name is dropped, whatever the schema holds). */
const PARSED_BLOCKS: ReadonlySet<string> = new Set([
  "paragraph",
  "heading",
  "bulletList",
  "orderedList",
  "listItem",
  "taskList",
  "taskItem",
  "blockquote",
  "callout",
  "codeBlock",
  "mathBlock",
  "horizontalRule",
  "details",
  "detailsSummary",
  "detailsContent",
  "table",
  "tableRow",
  "tableCell",
  "tableHeader",
  "image",
  "file",
]);

/** The marks `parseMarkdown` writes. */
const PARSED_MARKS: ReadonlySet<string> = new Set(["bold", "italic", "strike", "underline", "code", "link"]);

/** Blocks that count as Markdown by themselves, text or not. */
const STRUCTURAL_BLOCKS: ReadonlySet<string> = new Set([
  "horizontalRule",
  "codeBlock",
  "mathBlock",
  "table",
  "image",
  "file",
  "details",
]);

/** The elements HTML that only wraps text is made of. */
const WRAPPER_TAGS: ReadonlySet<string> = new Set(["PRE", "P", "SPAN", "DIV", "BR", "CODE", "META"]);

/** The schemes of a link kept as a link: the ones the click handler opens, plus the app's own. */
const LINK_SCHEMES: ReadonlySet<string> = new Set(["http", "https", "mailto", "tel", "attachment"]);

// ----- should the paste be taken ----------------------------------------------------------

/**
 * HTML that is the text and nothing else: only wrapper elements, and the same characters. A
 * coloured span (`data-text-color`, our own HTML) is formatting, not a wrapper: text copied
 * from the editor keeps its colour, even when it happens to look like Markdown.
 */
function isTextWrapper(html: string, text: string): boolean {
  if (html.trim() === "") {
    return true;
  }
  const body = new DOMParser().parseFromString(html, "text/html").body;
  for (const element of body.querySelectorAll("*")) {
    if (!WRAPPER_TAGS.has(element.tagName) || element.hasAttribute("data-text-color")) {
      return false;
    }
  }
  const squash = (value: string): string => value.replace(/\s+/gu, "");
  return squash(body.textContent ?? "") === squash(text);
}

/** Whether Shift is held (View > paste as plain text): ProseMirror keeps the key in its input state. */
function wantsPlainText(view: EditorView): boolean {
  const input = (view as unknown as { input?: { shiftKey?: unknown; lastKeyCode?: unknown } }).input;
  return input?.shiftKey === true && input.lastKeyCode !== 45; // Shift+Insert is a plain paste of its own
}

function hasText(node: PortableNode): boolean {
  return node.type === "text" ? (node.text ?? "") !== "" : node.content.some(hasText);
}

/**
 * Whether what the parser found is more than the text itself: a block that is not a paragraph
 * (and has something in it: a lone `-` or `#` is no list or heading), or a paragraph with a
 * marked run or an equation. Plain lines of text are not, so they paste as before.
 */
function isStructured(blocks: readonly PortableNode[]): boolean {
  return blocks.some((block) => {
    if (block.type === "paragraph") {
      return block.content.some(
        (inline) => (inline.type === "text" && inline.marks.length > 0) || PARSED_INLINE_ATOMS.has(inline.type),
      );
    }
    return STRUCTURAL_BLOCKS.has(block.type) || hasText(block);
  });
}

// ----- portable JSON to nodes of the live schema ------------------------------------------

function isRecord(value: JsonValue | undefined): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** A link target the app handles: a scheme in the list, or none at all (relative, `#anchor`). */
function isSafeHref(href: JsonValue | undefined): href is string {
  if (typeof href !== "string" || href.trim() === "") {
    return false;
  }
  const scheme = /^([a-z][a-z0-9+.-]*):/iu.exec(href.trim())?.[1];
  return scheme === undefined || LINK_SCHEMES.has(scheme.toLowerCase());
}

function buildText(schema: Schema, json: { [key: string]: JsonValue }): PmNode | null {
  const text = json.text;
  if (typeof text !== "string" || text === "") {
    return null;
  }
  let marks: readonly Mark[] = Mark.none;
  for (const raw of Array.isArray(json.marks) ? json.marks : []) {
    if (!isRecord(raw) || typeof raw.type !== "string" || !PARSED_MARKS.has(raw.type)) {
      continue;
    }
    const type = schema.marks[raw.type];
    const attrs = isRecord(raw.attrs) ? raw.attrs : {};
    if (type === undefined || (raw.type === "link" && !isSafeHref(attrs.href))) {
      continue;
    }
    // `addToSet` honours what marks exclude (code takes no other mark in the schema).
    marks = type.create(attrs).addToSet(marks);
  }
  return schema.text(text, marks);
}

/**
 * A node of `type` holding `children`, repaired where the schema asks for it: a node the type
 * does not take there is dropped, and what its content must start with or end with is made
 * (an empty paragraph first in an item that starts with something else). `null` when nothing
 * valid can be made.
 */
function fit(type: NodeType, attrs: Attrs, children: readonly PmNode[]): PmNode | null {
  let match = type.contentMatch;
  const kept: PmNode[] = [];
  for (const child of children) {
    let next = match.matchType(child.type);
    if (next === null) {
      const filler = match.fillBefore(Fragment.from(child));
      const filled = filler === null ? null : match.matchFragment(filler);
      next = filled === null ? null : filled.matchType(child.type);
      if (filler !== null && next !== null) {
        kept.push(...filler.content);
      }
    }
    if (next !== null) {
      kept.push(child);
      match = next;
    }
  }
  const end = match.fillBefore(Fragment.empty, true);
  if (end === null) {
    return null;
  }
  kept.push(...end.content);
  const node = type.create(attrs, kept);
  try {
    node.check();
  } catch {
    return null;
  }
  return node;
}

function buildNode(schema: Schema, json: JsonValue): PmNode | null {
  if (!isRecord(json)) {
    return null;
  }
  const name = json.type;
  if (name === "text") {
    return buildText(schema, json);
  }
  if (name === "hardBreak") {
    return schema.nodes.hardBreak?.create() ?? null;
  }
  if (name === "dateMention") {
    const attrs = isRecord(json.attrs) ? json.attrs : {};
    const { date, time, reminder } = attrs;
    if (typeof date !== "string" || !isIsoDate(date)) {
      return null;
    }
    return (
      schema.nodes.dateMention?.create({
        date,
        time: typeof time === "string" && isIsoTime(time) ? time : null,
        reminder: typeof reminder === "number" ? reminder : null,
        relative: dateRelativeOf(attrs.relative),
      }) ?? null
    );
  }
  if (typeof name === "string" && PARSED_INLINE_ATOMS.has(name)) {
    const latex = isRecord(json.attrs) ? json.attrs.latex : undefined;
    return typeof latex === "string" && latex !== "" ? (schema.nodes[name]?.create({ latex }) ?? null) : null;
  }
  if (typeof name !== "string" || !PARSED_BLOCKS.has(name)) {
    return null;
  }
  const type = schema.nodes[name];
  if (type === undefined) {
    return null;
  }
  const children: PmNode[] = [];
  for (const child of Array.isArray(json.content) ? json.content : []) {
    const built = buildNode(schema, child);
    if (built !== null) {
      children.push(built);
    }
  }
  return fit(type, isRecord(json.attrs) ? json.attrs : {}, children);
}

/** The nodes of the live schema for portable block nodes; no ids (`UniqueID` assigns them). */
export function nodesFromPortable(schema: Schema, blocks: readonly JsonValue[]): PmNode[] {
  const nodes: PmNode[] = [];
  for (const block of blocks) {
    const node = buildNode(schema, block);
    if (node !== null) {
      nodes.push(node);
    }
  }
  return nodes;
}

// ----- putting them in the page -----------------------------------------------------------

/** The nodes that hold one list entry. */
const LIST_ITEMS: ReadonlySet<string> = new Set(["listItem", "taskItem"]);

/** What to replace, and with what. */
interface Target {
  from: number;
  to: number;
  slice: Slice;
}

/**
 * Where blocks go for a cursor at `$at` (a collapsed selection, in a textblock or between
 * blocks):
 * - the empty paragraph the cursor is in is replaced; in a list item that holds nothing else,
 *   the whole item is (ProseMirror then fits the blocks in: a list of one item is replaced by
 *   them, a list is split around them);
 * - a lone list of the kind of the list the cursor is in adds its items to that list, after the
 *   current item (or in place of it, when it is empty), instead of nesting under it;
 * - otherwise they go after the block the cursor is in, or after the nearest ancestor that takes
 *   blocks there (a toggle's summary does not: they go after the toggle).
 * With `replaceEmpty` false (the AI panel's "Insert below") an empty paragraph is not replaced: the
 * blocks go after it like after any other block.
 */
function targetFor($at: ResolvedPos, nodes: readonly PmNode[], replaceEmpty = true): Target {
  const blocks = new Slice(Fragment.from(nodes), 0, 0);
  const parent = $at.parent;
  if (!parent.isTextblock) {
    return { from: $at.pos, to: $at.pos, slice: blocks };
  }
  const depth = $at.depth;
  const empty = replaceEmpty && parent.type.name === "paragraph" && parent.content.size === 0;
  const item = depth >= 2 ? $at.node(depth - 1) : undefined;
  if (item !== undefined && LIST_ITEMS.has(item.type.name) && $at.index(depth - 1) === 0 && parent.type.name === "paragraph") {
    const pasted = nodes.length === 1 ? nodes[0] : undefined;
    const whole = empty && item.childCount === 1;
    const before = $at.before(depth - 1);
    const after = $at.after(depth - 1);
    if (pasted !== undefined && pasted.type === $at.node(depth - 2).type) {
      return { from: whole ? before : after, to: after, slice: new Slice(pasted.content, 0, 0) };
    }
    if (whole) {
      return { from: before, to: after, slice: blocks };
    }
  }
  if (empty) {
    return { from: $at.before(), to: $at.after(), slice: blocks };
  }
  const fragment = Fragment.from(nodes);
  for (let d = depth; d > 0; d -= 1) {
    const index = $at.indexAfter(d - 1);
    if ($at.node(d - 1).canReplace(index, index, fragment)) {
      return { from: $at.after(d), to: $at.after(d), slice: blocks };
    }
  }
  return { from: $at.after(), to: $at.after(), slice: blocks };
}

/**
 * Puts `nodes` where `targetFor` says for the cursor `$at` of `tr`, with the cursor at the end of
 * them; `null` when the document would not change.
 */
function placeBlocks(
  tr: Transaction,
  $at: ResolvedPos,
  nodes: readonly PmNode[],
  replaceEmpty: boolean,
): Transaction | null {
  const { from, to, slice } = targetFor($at, nodes, replaceEmpty);
  const steps = tr.steps.length;
  try {
    tr.replaceRange(from, to, slice);
  } catch {
    return null;
  }
  if (tr.steps.length === steps) {
    return null;
  }
  const end = tr.mapping.slice(steps).map(to, 1);
  tr.setSelection(Selection.near(tr.doc.resolve(end), -1));
  return tr;
}

/**
 * The transaction that pastes `nodes` in place of `selection` (the state's own by default), or `null`
 * when the document would not change. A lone paragraph goes in as text, where the cursor is; blocks go
 * where `targetFor` says, with the cursor at the end of them. The writing assistant (editor/ai) passes
 * the range it was asked about, so that one undo step takes the replacement back.
 */
export function pasteTransaction(
  state: EditorState,
  nodes: readonly PmNode[],
  selection: Selection = state.selection,
): Transaction | null {
  const tr = state.tr;
  if (selection !== state.selection) {
    tr.setSelection(selection);
  }
  if (!selection.empty) {
    tr.deleteSelection();
  }
  const only = nodes.length === 1 ? nodes[0] : undefined;
  const $at = tr.doc.resolve(tr.selection.from);
  if (only?.type.name === "paragraph" && $at.parent.inlineContent) {
    tr.replaceSelection(new Slice(only.content, 0, 0));
    return tr.docChanged ? tr : null;
  }
  return placeBlocks(tr, $at, nodes, true);
}

/**
 * The transaction that adds `nodes`, as blocks whatever they are, after the block `pos` is in (or after
 * the nearest ancestor that takes them): the writing assistant's "Insert below". An empty paragraph
 * the cursor is in is replaced only with `replaceEmpty` (an `/ai` typed on a line of its own).
 */
export function insertBelowTransaction(
  state: EditorState,
  nodes: readonly PmNode[],
  pos: number,
  replaceEmpty: boolean,
): Transaction | null {
  const tr = state.tr;
  const $at = tr.doc.resolve(Math.max(0, Math.min(pos, tr.doc.content.size)));
  return placeBlocks(tr, $at, nodes, replaceEmpty);
}

/** The `handlePaste` of the extension: `true` when the paste was taken. */
function pasteMarkdown(view: EditorView, event: ClipboardEvent): boolean {
  const data = event.clipboardData;
  if (!view.editable || data === null || wantsPlainText(view)) {
    return false;
  }
  const text = data.getData("text/plain");
  if (text === "" || text.length > MAX_PASTE_CHARS || !isTextWrapper(data.getData("text/html"), text)) {
    return false;
  }
  const { selection } = view.state;
  const inCode = selection.$from.parent.type.spec.code === true || selection.$to.parent.type.spec.code === true;
  if (inCode || selection.$from.marks().some((mark) => mark.type.name === "code")) {
    return false;
  }
  const blocks = parseMarkdown(text);
  if (!isStructured(blocks)) {
    return false;
  }
  const nodes = nodesFromPortable(view.state.schema, portableNodesToJson(blocks));
  const tr = nodes.length === 0 ? null : pasteTransaction(view.state, nodes);
  if (tr === null) {
    return false;
  }
  event.preventDefault();
  view.dispatch(tr.scrollIntoView());
  return true;
}

/**
 * Pasted Markdown becomes blocks. Added to the editor next to the other paste handler
 * (`Attachments`, which takes files and leaves text alone), so that a paste of files and a
 * paste of text never compete.
 */
export const MarkdownPaste = Extension.create({
  name: "markdownPaste",

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey("markdownPaste"),
        props: {
          handlePaste: (view, event) => pasteMarkdown(view, event),
        },
      }),
    ];
  },
});
