// Commands on whole top-level blocks (the block handle's menu): turn into another kind,
// duplicate, delete, move up and down. They keep block ids where a block stays the same
// block (a paragraph turned into a heading, a bulleted list turned into a numbered one,
// a moved block) and leave new wrappers and copies without one, so `UniqueID` gives them
// fresh ids.

import { dateMentionText } from "@atomik-kb/core-client";
import { Fragment } from "@tiptap/pm/model";
import type { Mark, Node as PmNode, NodeType, Schema } from "@tiptap/pm/model";
import { Selection, TextSelection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";

import { FILE_NODE, IMAGE_NODE, isAttachmentNode } from "./attachment-nodes";
import { isDerivedBlock } from "./derived-blocks";
import { WHITEBOARD_NODE } from "./whiteboard-nodes";
import { registerWhiteboardCopy } from "./whiteboard-copy";
import { BLOCK_ID_ATTRIBUTE } from "./schema";
import { toggleHeadingLevel } from "./toggle-heading";
import type { ToggleHeadingLevel } from "./toggle-heading";

export type BlockKind =
  | "paragraph"
  | "heading1"
  | "heading2"
  | "heading3"
  | "bulletList"
  | "orderedList"
  | "taskList"
  | "blockquote"
  | "callout"
  | "codeBlock"
  | "mathBlock"
  | "toggle"
  | "toggleHeading1"
  | "toggleHeading2"
  | "toggleHeading3"
  | "image"
  | "file";

/** The kinds a block of text (or a container of blocks) can be turned into. */
export const TEXT_KINDS: readonly BlockKind[] = [
  "paragraph",
  "heading1",
  "heading2",
  "heading3",
  "bulletList",
  "orderedList",
  "taskList",
  "blockquote",
  "callout",
  "codeBlock",
  "mathBlock",
];

/**
 * The toggle kinds (ADR-0040): a plain toggle and the toggle headings of levels 1 to 3. A paragraph or a
 * heading becomes one with its text as the summary and an empty body, and a toggle only sets or clears
 * the `level` of its summary. They are offered for those blocks and for toggles; other blocks (lists,
 * quotes, code...) have no single line of text to make a summary of.
 */
export const TOGGLE_KINDS: readonly BlockKind[] = ["toggle", "toggleHeading1", "toggleHeading2", "toggleHeading3"];

/**
 * The kinds an attachment block can be turned into: the other attachment block (an image
 * shown as a file card, or a file shown as an image), or a quote or callout around it. Text
 * kinds would drop the attachment, and text cannot become an attachment.
 */
export const ATTACHMENT_KINDS: readonly BlockKind[] = ["image", "file", "blockquote", "callout"];

/**
 * The kinds a table can be turned into: a quote or a callout around it. Text kinds would
 * flatten its cells into paragraphs, which is not turning a table into something else but
 * losing it.
 */
export const TABLE_KINDS: readonly BlockKind[] = ["blockquote", "callout"];

/**
 * The kinds a column layout can be turned into, as for a table: a quote or a callout around
 * it. Text kinds would flatten its columns into paragraphs (and lose the layout).
 */
export const COLUMN_KINDS: readonly BlockKind[] = ["blockquote", "callout"];

/**
 * The kinds a table of contents, a breadcrumb or a list view can be turned into: a quote or a callout
 * around it, as for a table. Text kinds would replace a block that holds nothing of its own (its
 * content is derived from the page and the tree) with an empty text block.
 */
export const DERIVED_KINDS: readonly BlockKind[] = ["blockquote", "callout"];

/**
 * The kinds a page link card can be turned into: a quote or a callout around it. Text kinds
 * would replace a leaf that holds nothing but a reference with an empty text block, which loses
 * the link.
 */
export const PAGE_LINK_KINDS: readonly BlockKind[] = ["blockquote", "callout"];

/** True for a page link card. */
export function isPageLink(node: PmNode): boolean {
  return node.type.name === "pageLink";
}

export const BLOCK_KINDS: readonly BlockKind[] = [...TEXT_KINDS, "image", "file"];

/** True for a table. */
export function isTable(node: PmNode): boolean {
  return node.type.name === "table";
}

/** True for a column layout (the node that holds the columns). */
export function isColumnList(node: PmNode): boolean {
  return node.type.name === "columnList";
}

/**
 * Blocks a conversion never takes apart: attachments, derived blocks and page link cards
 * (leaves), tables (cells in rows) and column layouts (blocks in columns).
 */
function isWhole(node: PmNode): boolean {
  return isAttachmentNode(node) || isDerivedBlock(node) || isPageLink(node) || isTable(node) || isColumnList(node);
}

/** Text blocks and toggles, which can become toggles: the kinds of a text block plus the toggle kinds. */
const TEXT_AND_TOGGLE_KINDS: readonly BlockKind[] = [...TEXT_KINDS, ...TOGGLE_KINDS];

/** The kinds the block menu offers for `node`. */
export function turnIntoKinds(node: PmNode): readonly BlockKind[] {
  if (isTable(node)) {
    return TABLE_KINDS;
  }
  if (isColumnList(node)) {
    return COLUMN_KINDS;
  }
  if (isDerivedBlock(node)) {
    return DERIVED_KINDS;
  }
  if (isPageLink(node)) {
    return PAGE_LINK_KINDS;
  }
  if (isAttachmentNode(node)) {
    return ATTACHMENT_KINDS;
  }
  return isToggleSource(node) ? TEXT_AND_TOGGLE_KINDS : TEXT_KINDS;
}

const LIST_ITEM: Record<string, string> = { bulletList: "listItem", orderedList: "listItem", taskList: "taskItem" };

/** The top-level blocks of a document with their start positions. */
export function topLevelBlocks(doc: PmNode): { node: PmNode; pos: number }[] {
  const blocks: { node: PmNode; pos: number }[] = [];
  doc.forEach((node, offset) => {
    blocks.push({ node, pos: offset });
  });
  return blocks;
}

/** The kind a block has now, if it is one of the kinds blocks can be turned into. */
export function kindOf(node: PmNode): BlockKind | null {
  switch (node.type.name) {
    case "heading":
      return node.attrs.level === 1 ? "heading1" : node.attrs.level === 2 ? "heading2" : "heading3";
    case "paragraph":
    case "bulletList":
    case "orderedList":
    case "taskList":
    case "blockquote":
    case "callout":
    case "codeBlock":
    case "mathBlock":
    case "image":
    case "file":
      return node.type.name;
    case "details": {
      const level = toggleHeadingLevel(node.firstChild?.attrs.level);
      return level === null ? "toggle" : (`toggleHeading${String(level)}` as BlockKind);
    }
    default:
      return null;
  }
}

function type(schema: Schema, name: string): NodeType {
  const nodeType = schema.nodes[name];
  if (nodeType === undefined) {
    throw new Error(`the schema has no ${name} node`);
  }
  return nodeType;
}

/** `node` and everything in it without block ids (copies get new ones). */
export function withoutIds(node: PmNode): PmNode {
  if (node.isText) {
    return node;
  }
  const children: PmNode[] = [];
  node.forEach((child) => {
    children.push(withoutIds(child));
  });
  let attrs =
    BLOCK_ID_ATTRIBUTE in (node.type.spec.attrs ?? {}) ? { ...node.attrs, [BLOCK_ID_ATTRIBUTE]: null } : node.attrs;
  const source: unknown = node.attrs[BLOCK_ID_ATTRIBUTE];
  if (node.type.name === WHITEBOARD_NODE && typeof source === "string" && source !== "") {
    // The drawing is not in the node (ADR-0033): the copy gets a new id now and the elements of the source
    // follow it into the page (whiteboard-copy.ts).
    const copy = crypto.randomUUID();
    registerWhiteboardCopy(copy, source);
    attrs = { ...attrs, [BLOCK_ID_ATTRIBUTE]: copy };
  }
  return node.type.create(attrs, Fragment.from(children), node.marks);
}

/**
 * The textblocks, attachment and derived blocks, tables and column layouts inside `node` (itself
 * if it is one), in order: what a conversion keeps. All but the textblocks pass through
 * unchanged, so turning a callout that holds an image, a table, columns or a table of contents
 * into text keeps it.
 */
function leavesOf(node: PmNode): PmNode[] {
  if (node.isTextblock || isWhole(node)) {
    return [node];
  }
  const found: PmNode[] = [];
  node.descendants((child) => {
    if (child.isTextblock || isWhole(child)) {
      found.push(child);
      return false;
    }
    return true;
  });
  return found;
}

/** The text of a textblock as code: line breaks as newlines, references and other atoms dropped. */
function plainText(node: PmNode): string {
  return node.textBetween(0, node.content.size, "\n", (leaf) => (leaf.type.name === "hardBreak" ? "\n" : ""));
}

/** Inline content a paragraph or heading can take from another textblock. */
function inlineContent(node: PmNode, target: NodeType): Fragment {
  if (node.type.spec.code === true) {
    const text = node.textContent;
    return text === "" ? Fragment.empty : Fragment.from(target.schema.text(text));
  }
  return node.content;
}

function idOf(node: PmNode): unknown {
  return node.attrs[BLOCK_ID_ATTRIBUTE] ?? null;
}

/** Blocks that can become a toggle or a toggle heading: a single line of text, or a toggle. */
function isToggleSource(node: PmNode): boolean {
  return node.type.name === "paragraph" || node.type.name === "heading" || node.type.name === "details";
}

/** The level of the toggle kinds: `null` for the plain toggle. */
function toggleLevelOf(kind: BlockKind): ToggleHeadingLevel | null {
  switch (kind) {
    case "toggleHeading1":
      return 1;
    case "toggleHeading2":
      return 2;
    case "toggleHeading3":
      return 3;
    default:
      return null;
  }
}

/**
 * The inline content of a paragraph or heading as the content of a summary, which holds text only:
 * text keeps its marks, a line break is a space, a date mention is its ISO text and an inline
 * equation its source. A page reference has no text of its own and is left out (as when a block is
 * turned into code).
 */
function summaryContent(block: PmNode, schema: Schema): Fragment {
  const parts: PmNode[] = [];
  const add = (value: string, marks: readonly Mark[]): void => {
    if (value !== "") {
      parts.push(schema.text(value, marks));
    }
  };
  block.forEach((child) => {
    switch (child.type.name) {
      case "text":
        parts.push(child);
        return;
      case "hardBreak":
        add(" ", []);
        return;
      case "dateMention":
        add(dateMentionText(child.attrs.date, child.attrs.time) ?? "", child.marks);
        return;
      case "mathInline":
        add(typeof child.attrs.latex === "string" ? child.attrs.latex : "", child.marks);
        return;
      default:
    }
  });
  return Fragment.fromArray(parts);
}

/**
 * `block` as a toggle (`level` `null`) or a toggle heading: a toggle only gets or loses the level of its
 * summary (same ids, same body); a paragraph or a heading becomes a closed toggle whose summary is its
 * text and whose body is one empty paragraph. The block keeps its id, the summary and the body get new
 * ones. `null` for any other block.
 */
function toToggle(block: PmNode, level: ToggleHeadingLevel | null, schema: Schema): PmNode[] | null {
  if (block.type.name === "details") {
    const summary = block.firstChild;
    if (summary?.type.name !== "detailsSummary") {
      return null;
    }
    const next = summary.type.create({ ...summary.attrs, level }, summary.content, summary.marks);
    return [block.copy(block.content.replaceChild(0, next))];
  }
  if (block.type.name !== "paragraph" && block.type.name !== "heading") {
    return null;
  }
  const summary = type(schema, "detailsSummary").create({ [BLOCK_ID_ATTRIBUTE]: null, level }, summaryContent(block, schema));
  const body = type(schema, "detailsContent").create({ [BLOCK_ID_ATTRIBUTE]: null }, type(schema, "paragraph").create());
  return [type(schema, "details").create({ [BLOCK_ID_ATTRIBUTE]: idOf(block), open: false }, [summary, body])];
}

/**
 * A toggle (plain or a toggle heading) as a paragraph or a heading: the summary's text is the new
 * block, which keeps the toggle's id (a link to the toggle goes on working), and the blocks of the
 * body follow it as they are, ids included. A body that is one empty paragraph (what a new toggle
 * has) is not carried over, so that a heading made a toggle heading and back is the same heading.
 */
function liftToggle(block: PmNode, kind: BlockKind, schema: Schema): PmNode[] | null {
  let lead: Fragment = Fragment.empty;
  let seen = false;
  const body: PmNode[] = [];
  for (let index = 0; index < block.childCount; index += 1) {
    const child = block.child(index);
    if (!seen && child.type.name === "detailsSummary") {
      lead = child.content;
      seen = true;
    } else if (child.type.name === "detailsContent") {
      for (let at = 0; at < child.childCount; at += 1) {
        body.push(child.child(at));
      }
    } else {
      body.push(child);
    }
  }
  const first = body[0];
  if (body.length === 1 && first?.type.name === "paragraph" && first.content.size === 0) {
    body.length = 0;
  }
  const heading = kind === "paragraph" ? null : Number(kind.slice(-1));
  const attrs: Record<string, unknown> = { [BLOCK_ID_ATTRIBUTE]: idOf(block) };
  if (heading !== null) {
    attrs.level = heading;
  }
  return [type(schema, heading === null ? "paragraph" : "heading").create(attrs, lead), ...body];
}

/** The nodes `block` becomes as `kind`, or `null` when it already is one. */
function convert(block: PmNode, kind: BlockKind, schema: Schema): PmNode[] | null {
  if (kindOf(block) === kind) {
    return null;
  }
  const paragraph = type(schema, "paragraph");
  if (isAttachmentNode(block) && !ATTACHMENT_KINDS.includes(kind)) {
    return null; // text kinds would drop the attachment
  }
  if (isTable(block) && !TABLE_KINDS.includes(kind)) {
    return null; // text kinds would flatten the table into paragraphs
  }
  if (isColumnList(block) && !COLUMN_KINDS.includes(kind)) {
    return null; // text kinds would flatten the columns into paragraphs
  }
  if (isDerivedBlock(block) && !DERIVED_KINDS.includes(kind)) {
    return null; // text kinds would replace the block with an empty text block
  }
  if (isPageLink(block) && !PAGE_LINK_KINDS.includes(kind)) {
    return null; // text kinds would replace the card with an empty text block and lose the link
  }
  if (block.type.name === "details" && (kind === "paragraph" || kind === "heading1" || kind === "heading2" || kind === "heading3")) {
    return liftToggle(block, kind, schema);
  }
  switch (kind) {
    case "toggle":
    case "toggleHeading1":
    case "toggleHeading2":
    case "toggleHeading3":
      return toToggle(block, toggleLevelOf(kind), schema);
    case "image":
    case "file": {
      if (!isAttachmentNode(block)) {
        return null; // text has no attachment to show
      }
      // The same block (id) showing the same attachment another way.
      const attrs: Record<string, unknown> = {
        [BLOCK_ID_ATTRIBUTE]: idOf(block),
        attachmentId: block.attrs.attachmentId ?? null,
      };
      return [type(schema, kind === "image" ? IMAGE_NODE : FILE_NODE).create(attrs)];
    }
    case "paragraph":
    case "heading1":
    case "heading2":
    case "heading3":
    case "codeBlock":
    case "mathBlock": {
      const leaves = leavesOf(block);
      const target = type(
        schema,
        kind === "paragraph" ? "paragraph" : kind === "codeBlock" || kind === "mathBlock" ? kind : "heading",
      );
      const attrsFor = (leaf: PmNode | null): Record<string, unknown> => {
        const attrs: Record<string, unknown> = { [BLOCK_ID_ATTRIBUTE]: leaf === null ? null : idOf(leaf) };
        if (target.name === "heading") {
          attrs.level = Number(kind.slice(-1));
        }
        if (target.name === "codeBlock" && leaf?.type.name === "codeBlock") {
          attrs.language = leaf.attrs.language ?? null;
        }
        return attrs;
      };
      if (leaves.length === 0) {
        return [target.create(attrsFor(null))];
      }
      return leaves.map((leaf) => {
        if (isWhole(leaf)) {
          return leaf;
        }
        if (target.name === "codeBlock" || target.name === "mathBlock") {
          const text = plainText(leaf);
          return target.create(attrsFor(leaf), text === "" ? null : schema.text(text));
        }
        return target.create(attrsFor(leaf), inlineContent(leaf, target));
      });
    }
    case "bulletList":
    case "orderedList":
    case "taskList": {
      const list = type(schema, kind);
      const item = type(schema, LIST_ITEM[kind] ?? "listItem");
      if (LIST_ITEM[block.type.name] !== undefined) {
        // A list of another kind: same items, same ids, other markers.
        const items: PmNode[] = [];
        block.forEach((child) => {
          items.push(item.create({ [BLOCK_ID_ATTRIBUTE]: idOf(child) }, child.content));
        });
        return [list.create({ [BLOCK_ID_ATTRIBUTE]: idOf(block) }, items)];
      }
      const leaves = leavesOf(block);
      const items: PmNode[] = [];
      for (const leaf of leaves.length === 0 ? [null] : leaves) {
        if (leaf !== null && isWhole(leaf)) {
          // An item starts with a paragraph: the attachment or table joins the item before it.
          const previous = items.pop();
          items.push(
            previous === undefined
              ? item.create(null, [paragraph.create(), leaf])
              : item.create(previous.attrs, previous.content.addToEnd(leaf)),
          );
          continue;
        }
        items.push(
          item.create(
            null,
            paragraph.create(
              { [BLOCK_ID_ATTRIBUTE]: leaf === null ? null : idOf(leaf) },
              leaf === null ? null : inlineContent(leaf, paragraph),
            ),
          ),
        );
      }
      return [list.create(null, items)];
    }
    case "blockquote":
    case "callout": {
      const wrapper = type(schema, kind);
      if (block.type.name === "blockquote" || block.type.name === "callout") {
        // The other container: same blocks inside, same id.
        return [wrapper.create({ [BLOCK_ID_ATTRIBUTE]: idOf(block) }, block.content)];
      }
      return [wrapper.create(null, block)];
    }
  }
}

/** Turns the top-level block at `pos` into `kind`; `null` when it is already that kind. */
export function turnInto(state: EditorState, pos: number, kind: BlockKind): Transaction | null {
  const block = state.doc.nodeAt(pos);
  if (block === null) {
    return null;
  }
  const nodes = convert(block, kind, state.schema);
  if (nodes === null) {
    return null;
  }
  const tr = state.tr.replaceWith(pos, pos + block.nodeSize, nodes);
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(pos + 1, tr.doc.content.size))));
  return tr.scrollIntoView();
}

/** Inserts a copy of the block at `pos` (with new ids) right after it. */
export function duplicateBlock(state: EditorState, pos: number): Transaction | null {
  const block = state.doc.nodeAt(pos);
  if (block === null) {
    return null;
  }
  const after = pos + block.nodeSize;
  const tr = state.tr.insert(after, withoutIds(block));
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(after + 1, tr.doc.content.size))));
  return tr.scrollIntoView();
}

/** Deletes the block at `pos`; the last block of a page becomes an empty paragraph. */
export function deleteBlock(state: EditorState, pos: number): Transaction | null {
  const block = state.doc.nodeAt(pos);
  if (block === null) {
    return null;
  }
  if (state.doc.childCount === 1) {
    const tr = state.tr.replaceWith(pos, pos + block.nodeSize, type(state.schema, "paragraph").create());
    return tr.setSelection(TextSelection.create(tr.doc, 1));
  }
  const tr = state.tr.delete(pos, pos + block.nodeSize);
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(pos, tr.doc.content.size))));
  return tr;
}

/** Moves the block at `pos` one place up (`-1`) or down (`1`); `null` at the edge. */
export function moveBlock(state: EditorState, pos: number, direction: -1 | 1): Transaction | null {
  const blocks = topLevelBlocks(state.doc);
  const index = blocks.findIndex((block) => block.pos === pos);
  const here = blocks[index];
  const other = blocks[index + direction];
  if (here === undefined || other === undefined) {
    return null;
  }
  const tr = state.tr.delete(here.pos, here.pos + here.node.nodeSize);
  // Moving up: the block goes where the previous one starts. Moving down: after the next
  // one, which now starts where the moved block started.
  const target = direction === -1 ? other.pos : here.pos + other.node.nodeSize;
  tr.insert(target, here.node);
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(target + 1, tr.doc.content.size))));
  return tr.scrollIntoView();
}

/** True when the block at `pos` can move in `direction`. */
export function canMoveBlock(state: EditorState, pos: number, direction: -1 | 1): boolean {
  const blocks = topLevelBlocks(state.doc);
  const index = blocks.findIndex((block) => block.pos === pos);
  return index !== -1 && blocks[index + direction] !== undefined;
}

/**
 * The nodes the top-level block `block` becomes as `kind`; `null` when it stays as it is (it
 * already is that kind, or the conversion would lose what it holds). The bulk "turn into" of a
 * block range (`block-range-commands.ts`) applies it to each block of the range.
 */
export function convertBlock(block: PmNode, kind: BlockKind, schema: Schema): PmNode[] | null {
  return convert(block, kind, schema);
}
