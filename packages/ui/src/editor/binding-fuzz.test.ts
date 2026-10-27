// Binding fuzz: two ProseMirror views (no Tiptap, no React) with the block schema v1, each
// bound to its own LoroDoc, apply random transactions (text, marks, blocks and attributes,
// and the table operations of the toolbar: new tables, typing in cells, rows and columns
// added and deleted, the header row toggled, tables deleted, and the column operations of the
// editor: layouts made of a block, columns added and removed, layouts unwrapped, typing in
// columns, new layouts anywhere, nested ones included, and the bulk actions of the block handle
// on a random range of top-level blocks: delete, duplicate, move, turn into, with block ranges
// selected while the other replica's changes arrive) and exchange updates in random order.
// After every round both Loro documents and both ProseMirror documents must be equal, and
// each view must show exactly what its Loro document holds (the binding wrote it faithfully).
//
// 500 rounds by default; `KB_FUZZ_ITERATIONS=10000` for the long run, `KB_FUZZ_SEED=<n>` to
// replay a failure (the seed is printed when a round fails).

import { loroToPortable, normalizePortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import type { Mark, MarkType, Node as PmNode, NodeType, Schema } from "@tiptap/pm/model";
import { Fragment, Slice } from "@tiptap/pm/model";
import { TextSelection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";
import {
  addColumnAfter,
  addColumnBefore,
  addRowAfter,
  addRowBefore,
  deleteColumn,
  deleteRow,
  deleteTable,
  toggleHeaderRow,
} from "@tiptap/pm/tables";
import { canJoin, canSplit, findWrapping, liftTarget } from "@tiptap/pm/transform";
import type { EditorView } from "@tiptap/pm/view";
import { LoroDoc } from "loro-crdt";
import { describe, expect, it, vi } from "vitest";

import { bindView, loroDocFrom, PAGE_FIXTURE, sync, tick, withoutHardBreakIds } from "../test/editor";
import { columnOperations } from "./columns";
import { deleteBlocks, duplicateBlocks, moveBlocks, turnBlocksInto } from "./block-range-commands";
import { BlockRangeSelection, rangeOfBlocks } from "./block-selection";
import type { BlockRange } from "./block-selection";
import type { BlockKind } from "./blocks";
import { createNodeFromLoroObj, CursorEphemeralStore, loroCursorPluginKey, loroSyncPluginKey } from "./loro-binding";
import { convertPmSelectionToCursors, cursorToAbsolutePosition } from "./loro-binding/cursor/common";
import type { LoroNode, LoroNodeMapping } from "./loro-binding";
import { createEditorSchema } from "./schema";

const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {};
const ITERATIONS = Number(env.KB_FUZZ_ITERATIONS ?? "500");
const SEED = env.KB_FUZZ_SEED === undefined ? Math.floor(Math.random() * 2 ** 32) : Number(env.KB_FUZZ_SEED);
/** Rounds per pair of documents; then both start again from the fixture. */
const ROUNDS_PER_EPISODE = 25;
/** Also check after every step, not only after every round (slower; for replaying failures). */
const STRICT = env.KB_FUZZ_STRICT === "1";

const schema = createEditorSchema();

/** Deterministic random numbers (mulberry32). */
class Rng {
  #state: number;

  constructor(seed: number) {
    this.#state = seed >>> 0;
  }

  next(): number {
    this.#state = (this.#state + 0x6d2b79f5) >>> 0;
    let t = this.#state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }

  int(max: number): number {
    return Math.floor(this.next() * max);
  }

  chance(probability: number): boolean {
    return this.next() < probability;
  }

  pick<T>(items: readonly T[]): T | undefined {
    return items.length === 0 ? undefined : items[this.int(items.length)];
  }
}

// BMP characters only: ProseMirror positions and Loro text offsets are both UTF-16, and a
// random range must not cut a surrogate pair (that is not something the editor does).
const ALPHABET = "abcdefghijklmnopqrstuvwxyz  éüßñ日本語";

function randomText(rng: Rng): string {
  const length = 1 + rng.int(6);
  let text = "";
  for (let i = 0; i < length; i += 1) {
    text += ALPHABET[rng.int(ALPHABET.length)] ?? "x";
  }
  return text;
}

function nodeType(name: string): NodeType {
  const type = schema.nodes[name];
  if (type === undefined) {
    throw new Error(`no node type ${name}`);
  }
  return type;
}

function markType(name: string): MarkType {
  const type = schema.marks[name];
  if (type === undefined) {
    throw new Error(`no mark type ${name}`);
  }
  return type;
}

function randomMark(rng: Rng): Mark {
  const name = rng.pick(["bold", "italic", "underline", "strike", "code", "link"]) ?? "bold";
  return name === "link"
    ? markType("link").create({ href: rng.pick(["https://a.example", "https://b.example", "mailto:x@example.com"]) })
    : markType(name).create();
}

function paragraph(rng: Rng): PmNode {
  return nodeType("paragraph").create(null, schema.text(randomText(rng), rng.chance(0.3) ? [randomMark(rng)] : []));
}

/**
 * A table of 1 to 3 rows and columns of cells that each hold a paragraph, with a header row
 * half of the time (the cells' span attributes keep their defaults, as the editor leaves them).
 */
function randomTable(rng: Rng): PmNode {
  const rows = 1 + rng.int(3);
  const columns = 1 + rng.int(3);
  const header = rng.chance(0.5);
  const row = (index: number): PmNode =>
    nodeType("tableRow").create(
      null,
      Array.from({ length: columns }, () => nodeType(header && index === 0 ? "tableHeader" : "tableCell").create(null, paragraph(rng))),
    );
  return nodeType("table").create(null, Array.from({ length: rows }, (_, index) => row(index)));
}

/** A random block node of block schema v1 (without ids: no UniqueID outside Tiptap). */
function randomBlock(rng: Rng): PmNode {
  const text = () => schema.text(randomText(rng));
  switch (rng.int(12)) {
    case 0:
      return nodeType("heading").create({ level: 1 + rng.int(3) }, text());
    case 1:
      return nodeType("codeBlock").create({ language: rng.pick([null, "rust", "ts"]) }, text());
    case 2:
      return nodeType("horizontalRule").create();
    case 3:
      return nodeType("blockquote").create(null, paragraph(rng));
    case 4:
      return nodeType("callout").create({ icon: rng.pick(["💡", "⚠️"]) }, paragraph(rng));
    case 5:
      return nodeType("bulletList").create(null, nodeType("listItem").create(null, paragraph(rng)));
    case 6:
      return nodeType("orderedList").create({ start: 1 + rng.int(3) }, nodeType("listItem").create(null, paragraph(rng)));
    case 7:
      return nodeType("taskList").create(null, nodeType("taskItem").create({ checked: rng.chance(0.5) }, paragraph(rng)));
    case 8:
      return nodeType("details").create({ open: rng.chance(0.5) }, [
        nodeType("detailsSummary").create(null, text()),
        nodeType("detailsContent").create(null, paragraph(rng)),
      ]);
    case 9:
      return randomTable(rng);
    default:
      return paragraph(rng);
  }
}

/**
 * `pos`, moved off the middle of a surrogate pair (the fixture has astral characters). The
 * editor never puts a cursor there, and Loro, which stores UTF-8, cannot hold half a pair.
 */
function whole(doc: PmNode, pos: number): number {
  if (pos <= 0 || pos >= doc.content.size) {
    return pos;
  }
  const $pos = doc.resolve(pos);
  const node = $pos.parent.maybeChild($pos.index());
  if ($pos.textOffset > 0 && node?.text !== undefined) {
    const code = node.text.charCodeAt($pos.textOffset - 1);
    if (code >= 0xd800 && code <= 0xdbff) {
      return pos - 1;
    }
  }
  return pos;
}

/** A random position inside a textblock (never inside a surrogate pair). */
function inside(doc: PmNode, spot: TextblockSpot, rng: Rng): number {
  return whole(doc, spot.start + rng.int(spot.node.content.size + 1));
}

interface TextblockSpot {
  node: PmNode;
  /** Position of the textblock's start (inside it). */
  start: number;
}

function textblocks(doc: PmNode): TextblockSpot[] {
  const spots: TextblockSpot[] = [];
  doc.descendants((node, pos) => {
    if (node.isTextblock) {
      spots.push({ node, start: pos + 1 });
      return false;
    }
    return true;
  });
  return spots;
}

/** Positions between the children of block containers (where a block can go). */
function blockGaps(doc: PmNode): number[] {
  const gaps: number[] = [];
  const visit = (node: PmNode, contentStart: number): void => {
    if (node.isTextblock || node.isLeaf) {
      return;
    }
    let pos = contentStart;
    gaps.push(pos);
    node.forEach((child) => {
      visit(child, pos + 1);
      pos += child.nodeSize;
      gaps.push(pos);
    });
  };
  visit(doc, 0);
  return gaps;
}

/** Start positions of every block node (not text, not inline). */
function blockStarts(doc: PmNode): number[] {
  const starts: number[] = [];
  doc.descendants((node, pos) => {
    if (node.isBlock) {
      starts.push(pos);
    }
    return !node.isTextblock;
  });
  return starts;
}

type Op = (state: EditorState, rng: Rng) => Transaction | null;

const insertText: Op = (state, rng) => {
  const spot = rng.pick(textblocks(state.doc));
  if (spot === undefined) {
    return null;
  }
  const pos = inside(state.doc, spot, rng);
  const $pos = state.doc.resolve(pos);
  const index = $pos.index();
  if (rng.chance(0.15) && $pos.parent.canReplaceWith(index, index, nodeType("pageRef"))) {
    const pageId = rng.pick(["7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6", "11111111-2222-4333-8444-555555555555"]);
    return state.tr.insert(pos, nodeType("pageRef").create({ pageId }));
  }
  if (rng.chance(0.1) && $pos.parent.canReplaceWith(index, index, nodeType("hardBreak"))) {
    return state.tr.insert(pos, nodeType("hardBreak").create());
  }
  return state.tr.insertText(randomText(rng), pos);
};

const deleteText: Op = (state, rng) => {
  const spot = rng.pick(textblocks(state.doc));
  if (spot === undefined || spot.node.content.size === 0) {
    return null;
  }
  const a = inside(state.doc, spot, rng);
  const b = inside(state.doc, spot, rng);
  return a === b ? null : state.tr.delete(Math.min(a, b), Math.max(a, b));
};

const toggleMark: Op = (state, rng) => {
  const spot = rng.pick(textblocks(state.doc));
  if (spot === undefined || spot.node.content.size === 0) {
    return null;
  }
  const mark = randomMark(rng);
  if (!spot.node.type.allowsMarkType(mark.type)) {
    return null;
  }
  const a = inside(state.doc, spot, rng);
  const b = inside(state.doc, spot, rng);
  if (a === b) {
    return null;
  }
  const [from, to] = [Math.min(a, b), Math.max(a, b)];
  return rng.chance(0.6) ? state.tr.addMark(from, to, mark) : state.tr.removeMark(from, to, mark.type);
};

const splitBlock: Op = (state, rng) => {
  const spot = rng.pick(textblocks(state.doc));
  if (spot === undefined) {
    return null;
  }
  const pos = inside(state.doc, spot, rng);
  const depth = rng.chance(0.4) ? 2 : 1;
  return canSplit(state.doc, pos, depth) ? state.tr.split(pos, depth) : null;
};

const joinBlocks: Op = (state, rng) => {
  const candidates = blockGaps(state.doc).filter((pos) => canJoin(state.doc, pos));
  const pos = rng.pick(candidates);
  return pos === undefined ? null : state.tr.join(pos);
};

const insertBlock: Op = (state, rng) => {
  const block = randomBlock(rng);
  const candidates = blockGaps(state.doc).filter((pos) => {
    const $pos = state.doc.resolve(pos);
    return $pos.parent.canReplaceWith($pos.index(), $pos.index(), block.type);
  });
  const pos = rng.pick(candidates);
  return pos === undefined ? null : state.tr.insert(pos, block);
};

const deleteBlock: Op = (state, rng) => {
  const start = rng.pick(blockStarts(state.doc));
  if (start === undefined) {
    return null;
  }
  const node = state.doc.nodeAt(start);
  if (node === null) {
    return null;
  }
  return rng.chance(0.5) ? state.tr.delete(start, start + node.nodeSize) : state.tr.deleteRange(start, start + node.nodeSize);
};

const replaceRange: Op = (state, rng) => {
  const size = state.doc.content.size;
  const a = whole(state.doc, rng.int(size + 1));
  const b = whole(state.doc, rng.int(size + 1));
  const c = whole(state.doc, rng.int(size + 1));
  const d = whole(state.doc, rng.int(size + 1));
  const slice = rng.chance(0.3) ? Slice.empty : state.doc.slice(Math.min(c, d), Math.max(c, d));
  return state.tr.replaceRange(Math.min(a, b), Math.max(a, b), slice);
};

const setAttributes: Op = (state, rng) => {
  const candidates: { pos: number; node: PmNode }[] = [];
  state.doc.descendants((node, pos) => {
    if (["heading", "taskItem", "details", "codeBlock", "orderedList", "callout"].includes(node.type.name)) {
      candidates.push({ pos, node });
    }
  });
  const target = rng.pick(candidates);
  if (target === undefined) {
    return null;
  }
  const { pos, node } = target;
  switch (node.type.name) {
    case "heading":
      return state.tr.setNodeAttribute(pos, "level", 1 + rng.int(3));
    case "taskItem":
      return state.tr.setNodeAttribute(pos, "checked", node.attrs.checked !== true);
    case "details":
      return state.tr.setNodeAttribute(pos, "open", node.attrs.open !== true);
    case "codeBlock":
      return state.tr.setNodeAttribute(pos, "language", rng.pick([null, "rust", "python"]));
    case "orderedList":
      return state.tr.setNodeAttribute(pos, "start", 1 + rng.int(5));
    default:
      return state.tr.setNodeAttribute(pos, "icon", rng.pick(["💡", "📌", "🔥"]));
  }
};

const wrapOrLift: Op = (state, rng) => {
  const spot = rng.pick(textblocks(state.doc));
  if (spot === undefined) {
    return null;
  }
  const $from = state.doc.resolve(spot.start);
  const $to = state.doc.resolve(spot.start + spot.node.content.size);
  const range = $from.blockRange($to);
  if (range === null) {
    return null;
  }
  if (rng.chance(0.5)) {
    const target = liftTarget(range);
    return target === null ? null : state.tr.lift(range, target);
  }
  const wrapper = nodeType(rng.pick(["blockquote", "callout", "bulletList", "taskList"]) ?? "blockquote");
  const wrapping = findWrapping(range, wrapper);
  return wrapping === null ? null : state.tr.wrap(range, wrapping);
};

/** The textblocks inside a table cell. */
function cellTextblocks(doc: PmNode): TextblockSpot[] {
  return textblocks(doc).filter((spot) => {
    const $pos = doc.resolve(spot.start);
    for (let depth = $pos.depth; depth > 0; depth -= 1) {
      const name = $pos.node(depth).type.name;
      if (name === "tableCell" || name === "tableHeader") {
        return true;
      }
    }
    return false;
  });
}

/** A new table wherever blocks can go: the page, a quote, an item, a cell of another table. */
const insertTableBlock: Op = (state, rng) => {
  const table = randomTable(rng);
  const candidates = blockGaps(state.doc).filter((pos) => {
    const $pos = state.doc.resolve(pos);
    return $pos.parent.canReplaceWith($pos.index(), $pos.index(), table.type);
  });
  const pos = rng.pick(candidates);
  return pos === undefined ? null : state.tr.insert(pos, table);
};

/** Typing in a random cell: the cells' text is what edits there are mostly made of. */
const typeInCell: Op = (state, rng) => {
  const spot = rng.pick(cellTextblocks(state.doc));
  return spot === undefined ? null : state.tr.insertText(randomText(rng), inside(state.doc, spot, rng));
};

/**
 * A table command (prosemirror-tables, what the editor's toolbar runs: rows and columns
 * added and deleted, the header row toggled, the table deleted) with the caret in a random
 * cell. The command reads its selection from the state, so it runs on a state that has the
 * caret there; the transaction it builds applies to the view's own state (same document).
 */
function tableCommand(command: (state: EditorState, dispatch: (tr: Transaction) => void) => boolean): Op {
  return (state, rng) => {
    const spot = rng.pick(cellTextblocks(state.doc));
    if (spot === undefined) {
      return null;
    }
    const placed = state.apply(state.tr.setSelection(TextSelection.create(state.doc, inside(state.doc, spot, rng))));
    const built: Transaction[] = [];
    command(placed, (tr) => {
      built.push(tr);
    });
    return built[0] ?? null;
  };
}

/** A layout of two to four columns that each hold a paragraph. */
function randomLayout(rng: Rng): PmNode {
  const columns = 2 + rng.int(3);
  return nodeType("columnList").create(
    null,
    Array.from({ length: columns }, () => nodeType("column").create(null, paragraph(rng))),
  );
}

/** The textblocks inside a column. */
function columnTextblocks(doc: PmNode): TextblockSpot[] {
  return textblocks(doc).filter((spot) => {
    const $pos = doc.resolve(spot.start);
    for (let depth = $pos.depth; depth > 0; depth -= 1) {
      if ($pos.node(depth).type.name === "column") {
        return true;
      }
    }
    return false;
  });
}

/**
 * A new layout wherever blocks can go: the page, a quote, an item, a cell, a column. The
 * schema allows a layout in a column, the editor's commands and filter do not: this is the
 * binding's worst case, which a merge or an older editor could produce.
 */
const insertLayout: Op = (state, rng) => {
  const layout = randomLayout(rng);
  const candidates = blockGaps(state.doc).filter((pos) => {
    const $pos = state.doc.resolve(pos);
    return $pos.parent.canReplaceWith($pos.index(), $pos.index(), layout.type);
  });
  const pos = rng.pick(candidates);
  return pos === undefined ? null : state.tr.insert(pos, layout);
};

/** Typing in a random block of a column. */
const typeInColumn: Op = (state, rng) => {
  const spot = rng.pick(columnTextblocks(state.doc));
  return spot === undefined ? null : state.tr.insertText(randomText(rng), inside(state.doc, spot, rng));
};

/**
 * A column command of the editor (`columns.ts`: wrap the block into columns, add a column,
 * remove one, unwrap the layout) with the caret in a random textblock (in a column when
 * `inColumn`). Like `tableCommand`, it runs on a state that has the caret there and builds
 * the transaction the view then applies.
 */
function columnCommand(run: (state: EditorState, tr: Transaction, rng: Rng) => boolean, inColumn: boolean): Op {
  return (state, rng) => {
    const spot = rng.pick(inColumn ? columnTextblocks(state.doc) : textblocks(state.doc));
    if (spot === undefined) {
      return null;
    }
    const placed = state.apply(state.tr.setSelection(TextSelection.create(state.doc, inside(state.doc, spot, rng))));
    const tr = placed.tr;
    return run(placed, tr, rng) ? tr : null;
  };
}

/** Marks a transaction that only selects: `randomEdit` dispatches those too. */
const SELECT_ONLY = "fuzzSelectOnly";

/** A random run of one to five top-level blocks, grown upward or downward from its first block. */
function randomRange(doc: PmNode, rng: Rng): BlockRange | null {
  if (doc.childCount === 0) {
    return null;
  }
  const anchor = rng.int(doc.childCount);
  return rangeOfBlocks(doc, anchor, anchor + rng.int(5) - 2);
}

/** A bulk action of the block handle's menu (block-range-commands.ts) on a random range of blocks. */
function rangeCommand(run: (state: EditorState, range: BlockRange, rng: Rng) => Transaction | null): Op {
  return (state, rng) => {
    const range = randomRange(state.doc, rng);
    return range === null ? null : run(state, range, rng);
  };
}

/** Selects a random range of blocks (no document change), so that the other replica's changes meet it. */
const selectBlockRange: Op = (state, rng) => {
  const range = randomRange(state.doc, rng);
  return range === null
    ? null
    : state.tr.setSelection(BlockRangeSelection.create(state.doc, range.anchorIndex, range.headIndex)).setMeta(SELECT_ONLY, true);
};

const TURN_INTO: readonly BlockKind[] = ["paragraph", "heading1", "heading3", "bulletList", "taskList", "blockquote", "callout", "codeBlock"];

const OPS: readonly Op[] = [
  insertText,
  insertText,
  insertText,
  deleteText,
  deleteText,
  toggleMark,
  toggleMark,
  splitBlock,
  splitBlock,
  joinBlocks,
  insertBlock,
  insertBlock,
  deleteBlock,
  replaceRange,
  setAttributes,
  wrapOrLift,
  // Tables: new tables, typing in cells and the row, column, header and table commands of
  // the toolbar (the fixture holds a table too, and `insertBlock` may add one).
  insertTableBlock,
  typeInCell,
  typeInCell,
  tableCommand(addRowBefore),
  tableCommand(addRowAfter),
  tableCommand(addColumnBefore),
  tableCommand(addColumnAfter),
  tableCommand(deleteRow),
  tableCommand(deleteColumn),
  tableCommand(toggleHeaderRow),
  tableCommand(deleteTable),
  // Columns: new layouts, typing in columns and the commands of the editor (a layout made of
  // the block the caret is in, a column added or removed, the layout unwrapped).
  insertLayout,
  typeInColumn,
  typeInColumn,
  columnCommand((state, tr, rng) => columnOperations.setColumns(state, tr, 2 + rng.int(4)), false),
  columnCommand((state, tr, rng) => columnOperations.setColumns(state, tr, 2 + rng.int(4)), false),
  columnCommand((state, tr) => columnOperations.addColumn(state, tr), true),
  columnCommand((state, tr) => columnOperations.removeColumn(state, tr), true),
  columnCommand((state, tr) => columnOperations.unwrapColumns(state, tr), true),
  // Block ranges: a range selected, and the menu's bulk actions on a random range (one
  // transaction each, like the single-block commands).
  selectBlockRange,
  selectBlockRange,
  rangeCommand((state, range) => deleteBlocks(state, range)),
  rangeCommand((state, range) => duplicateBlocks(state, range)),
  rangeCommand((state, range) => duplicateBlocks(state, range)),
  rangeCommand((state, range, rng) => moveBlocks(state, range, rng.chance(0.5) ? -1 : 1)),
  rangeCommand((state, range, rng) => moveBlocks(state, range, rng.chance(0.5) ? -1 : 1)),
  rangeCommand((state, range, rng) => turnBlocksInto(state, range, rng.pick(TURN_INTO) ?? "paragraph")),
];

/** What the last steps did, printed when a round fails. */
const trail: string[] = [];

function remember(entry: string): void {
  trail.push(entry);
  if (trail.length > 40) {
    trail.shift();
  }
}

/** Applies one random transaction; an operation ProseMirror refuses is skipped. */
function randomEdit(name: string, view: EditorView, rng: Rng): void {
  const op = rng.pick(OPS);
  let tr: Transaction | null;
  try {
    tr = op === undefined ? null : op(view.state, rng);
  } catch {
    tr = null; // not a valid step here (for example a replace that cannot be fitted)
  }
  if (tr !== null && (tr.docChanged || tr.getMeta(SELECT_ONLY) === true)) {
    remember(
      tr.docChanged
        ? `${name}: ${JSON.stringify(tr.steps.map((step) => step.toJSON() as unknown))}`
        : `${name}: select ${JSON.stringify(tr.selection.toJSON())}`,
    );
    view.dispatch(tr); // outside the try: a failure in the binding fails the test
  }
}

/**
 * Entries of the binding's container -> node cache that no longer describe their container
 * (the cache is reused when a remote change is applied). Strict mode only.
 */
function staleCacheEntries(view: EditorView, doc: LoroDoc): string[] {
  const state = loroSyncPluginKey.getState(view.state);
  if (state === undefined) {
    return [];
  }
  const fresh: LoroNodeMapping = new Map();
  createNodeFromLoroObj(schema, doc.getMap("doc") as LoroNode, fresh);
  const stale: string[] = [];
  for (const [id, cached] of state.mapping) {
    const actual = fresh.get(id);
    if (actual === undefined) {
      continue; // no longer in the document
    }
    // Text runs compare as ProseMirror joins them (Loro may split a run where a mark was),
    // and marks on inline nodes other than text are not stored (see `withoutAtomMarks`).
    const json = (value: PmNode | PmNode[]): unknown =>
      withoutAtomMarks(Array.isArray(value) ? Fragment.fromArray(value).toJSON() : value.toJSON());
    const same = Array.isArray(cached) === Array.isArray(actual) && JSON.stringify(json(cached)) === JSON.stringify(json(actual));
    if (!same) {
      const show = (value: PmNode | PmNode[]) =>
        JSON.stringify(Array.isArray(value) ? value.map((n) => n.toJSON() as unknown) : value.toJSON());
      stale.push(`${id}: cached ${show(cached)} but the document holds ${show(actual)}`);
    }
  }
  return stale;
}

/** One replica's view with the store of its cursor plugin (ADR-0050). */
interface Replica {
  name: string;
  view: EditorView;
  store: CursorEphemeralStore;
}

/**
 * Hands a replica's selection to the other one the way presence does: as the update of a frame the receiver's cursor plugin
 * draws (a device of the receiver's own store, named after the sender).
 */
function relayCursors(from: Replica, to: Replica): void {
  const state = loroSyncPluginKey.getState(from.view.state);
  if (state === undefined) {
    return;
  }
  const { anchor, focus } = convertPmSelectionToCursors(from.view.state.doc, from.view.state.selection, state);
  from.store.setLocal(anchor === undefined ? {} : { anchor, ...(focus === undefined ? {} : { focus }) });
  to.store.setRemote(`device-${from.name}`, { name: from.name, color: "#1d4ed8" }, from.store.encodeLocal());
}

/**
 * What the cursor plugin of a replica draws must lie inside its document; and once the replicas are equal, the caret of the other
 * replica is where that replica's selection has its head.
 */
function checkCursors(replica: Replica, other: Replica, synced: boolean, label: string): void {
  const found = loroCursorPluginKey.getState(replica.view.state)?.decorations.find() ?? [];
  const size = replica.view.state.doc.content.size;
  for (const decoration of found) {
    expect(decoration.from >= 0 && decoration.from <= decoration.to && decoration.to <= size, `${label}: a decoration outside the document`).toBe(true);
  }
  const selection = other.view.state.selection;
  if (synced && selection instanceof TextSelection && other.store.getLocal()?.anchor !== undefined) {
    // The conversion of a selection to a Loro cursor and back is upstream's, and it does not return every selection to where it
    // was (a position between inline atoms, say): such a selection is not a test of what the plugin does with the cursor.
    const otherState = loroSyncPluginKey.getState(other.view.state);
    const cursor = other.store.getLocal()?.focus ?? other.store.getLocal()?.anchor;
    const own =
      otherState === undefined || cursor === undefined
        ? undefined
        : cursorToAbsolutePosition(cursor, otherState.doc, otherState.mapping)[0];
    if (own === selection.head) {
      const carets = found.filter((decoration) => decoration.from === decoration.to).map((decoration) => decoration.from);
      expect(carets, `${label}: the caret of the other replica`).toEqual([selection.head]);
    }
  }
}

function portableOf(view: EditorView): JsonValue {
  return normalizePortable(view.state.doc.toJSON() as JsonValue);
}

/**
 * What the binding shows for `doc` when it renders it from scratch: the document itself,
 * except that content concurrent edits left outside the schema is completed where
 * ProseMirror can (an empty list gets an item; see patch r in VENDORED.md).
 */
function rendered(doc: LoroDoc): JsonValue {
  const node = createNodeFromLoroObj(schema, doc.getMap("doc") as LoroNode, new Map());
  return normalizePortable(node.toJSON() as JsonValue);
}

/**
 * The view's document as ProseMirror JSON (all attributes, `null` ones included), without
 * marks on inline nodes other than text: shape v1 stores marks on text only, so a mark
 * ProseMirror puts on a page reference or a line break exists in the view that applied it
 * until the next reload (a known limitation of the binding).
 */
function viewJson(view: EditorView): unknown {
  return withoutAtomMarks(view.state.doc.toJSON());
}

/** ProseMirror JSON without marks on nodes other than text (shape v1 stores none there). */
function withoutAtomMarks(json: unknown): unknown {
  if (Array.isArray(json)) {
    return json.map(withoutAtomMarks);
  }
  if (typeof json !== "object" || json === null) {
    return json;
  }
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(json)) {
    result[key] = withoutAtomMarks(value);
  }
  if (result.type !== "text") {
    delete result.marks;
  }
  return result;
}

describe(`binding fuzz (${String(ITERATIONS)} rounds, seed ${String(SEED)})`, () => {
  it(
    "keeps two replicas identical under random concurrent edits",
    async () => {
      const rng = new Rng(SEED);
      // The fixture has every node and mark; its hardBreak id is outside the schema.
      const snapshot = loroDocFrom(withoutHardBreakIds(PAGE_FIXTURE)).export({ mode: "snapshot" });
      let round = 0;
      // The binding logs what it cannot handle (a node it cannot create, a cursor it
      // cannot place): a log fails the round.
      const log = console.error.bind(console);
      const logged: unknown[][] = [];
      const consoleError = vi.spyOn(console, "error").mockImplementation((...args: unknown[]) => {
        logged.push(args);
      });
      try {
        while (round < ITERATIONS) {
          // One episode: two replicas of the fixture, as two devices that opened the page.
          const docA = new LoroDoc();
          const docB = new LoroDoc();
          docA.import(snapshot);
          docB.import(snapshot);
          // Fixed peer ids: Loro orders concurrent inserts by peer id, so a seed
          // replays the same run.
          docA.setPeerId(1);
          docB.setPeerId(2);
          // The cursor plugin runs in both views: each replica's selection reaches the other as a presence frame would carry it.
          const storeA = new CursorEphemeralStore(docA.peerIdStr);
          const storeB = new CursorEphemeralStore(docB.peerIdStr);
          const a = bindView(schema, docA, { cursors: storeA });
          const b = bindView(schema, docB, { cursors: storeB });
          const replicaA: Replica = { name: "A", view: a.view, store: storeA };
          const replicaB: Replica = { name: "B", view: b.view, store: storeB };
          await tick();
          try {
            for (let i = 0; i < ROUNDS_PER_EPISODE && round < ITERATIONS; i += 1, round += 1) {
              const steps = 1 + rng.int(7);
              for (let step = 0; step < steps; step += 1) {
                const choice = rng.int(10);
                if (choice < 4) {
                  randomEdit("A", a.view, rng);
                } else if (choice < 8) {
                  randomEdit("B", b.view, rng);
                } else if (choice === 8) {
                  remember("sync A -> B");
                  sync(docA, docB);
                } else {
                  remember("sync B -> A");
                  sync(docB, docA);
                }
                relayCursors(replicaA, replicaB);
                relayCursors(replicaB, replicaA);
                checkCursors(replicaA, replicaB, false, `round ${String(round)}: A after a step`);
                checkCursors(replicaB, replicaA, false, `round ${String(round)}: B after a step`);
                if (STRICT) {
                  // Between steps each view always shows exactly its own document.
                  expect(portableOf(a.view), `round ${String(round)}: view A after a step`).toEqual(rendered(docA));
                  expect(portableOf(b.view), `round ${String(round)}: view B after a step`).toEqual(rendered(docB));
                  expect(staleCacheEntries(a.view, docA), `round ${String(round)}: cache A`).toEqual([]);
                  expect(staleCacheEntries(b.view, docB), `round ${String(round)}: cache B`).toEqual([]);
                }
              }
              const exchange = (from: LoroDoc, to: LoroDoc, view: EditorView, name: string): void => {
                remember(`final sync -> ${name}`);
                sync(from, to);
                if (STRICT) {
                  expect(portableOf(view), `round ${String(round)}: view ${name} after the final sync`).toEqual(
                    rendered(to),
                  );
                }
              };
              if (rng.chance(0.5)) {
                exchange(docA, docB, b.view, "B");
                exchange(docB, docA, a.view, "A");
              } else {
                exchange(docB, docA, a.view, "A");
                exchange(docA, docB, b.view, "B");
              }

              const portableA = loroToPortable(docA);
              expect(loroToPortable(docB), `round ${String(round)}: Loro documents differ`).toEqual(portableA);
              const shown = rendered(docA);
              expect(portableOf(a.view), `round ${String(round)}: view A differs from its document`).toEqual(shown);
              expect(portableOf(b.view), `round ${String(round)}: view B differs from its document`).toEqual(shown);
              expect(viewJson(a.view), `round ${String(round)}: views differ`).toEqual(viewJson(b.view));
              // The replicas are equal: each draws the other's caret where the other's selection has its head.
              relayCursors(replicaA, replicaB);
              relayCursors(replicaB, replicaA);
              checkCursors(replicaA, replicaB, true, `round ${String(round)}: A`);
              checkCursors(replicaB, replicaA, true, `round ${String(round)}: B`);
              expect(logged, `round ${String(round)}: the binding logged errors`).toEqual([]);
            }
          } finally {
            a.destroy();
            b.destroy();
            storeA.destroy();
            storeB.destroy();
          }
        }
      } catch (error) {
        log(
          `binding fuzz failed in round ${String(round)}; replay with KB_FUZZ_SEED=${String(SEED)} KB_FUZZ_ITERATIONS=${String(ITERATIONS)} (KB_FUZZ_STRICT=1 checks after every step)\nlast steps:\n${trail.join("\n")}`,
        );
        throw error;
      } finally {
        consoleError.mockRestore();
      }
    },
    Math.max(60_000, ITERATIONS * 200),
  );
});

// Keeps the schema helper honest: every op works on the schema the editor uses.
describe("binding fuzz helpers", () => {
  it("only builds blocks the schema accepts", () => {
    const rng = new Rng(1);
    for (let i = 0; i < 200; i += 1) {
      expect(() => {
        randomBlock(rng).check();
      }).not.toThrow();
    }
    expect((schema satisfies Schema).nodes.callout).toBeDefined();
  });
});
