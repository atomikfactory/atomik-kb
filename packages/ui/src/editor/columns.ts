// Columns layout (ADR-0016, amendment of 2026-10-06): blocks side by side. A `columnList`
// (group `block`, content `column{2,5}`) holds two to five `column`s, and a column holds
// blocks (`block+`). Columns have equal widths (no resizing) and a column list never sits
// inside a column. Both nodes carry a block `id` like every block, assigned by `UniqueID`
// (the commands create nodes without one).
//
// The commands work on the top-level block the selection is in (`setColumns`) or on the
// column list around it (`addColumn`, `removeColumn`, `unwrapColumns`), keep the ids of the
// blocks they move (a moved block is the same block) and leave the caret where it was.
//
// No nesting is a rule of the editor, not of the schema (a column list inside a column
// is valid content, so a merge or an older editor could still produce one, and every reader
// shows it as it is): the commands refuse, the `/` entries are unavailable
// (`columnsUnavailable`) and a plugin rejects a local transaction that adds one. Transactions
// the Loro binding dispatches (initial load, remote changes) are never filtered: they show
// what the document holds.

import { findParentNodeClosestToPos, mergeAttributes, Node } from "@tiptap/core";
import type { Editor } from "@tiptap/core";
import { Fragment, Slice } from "@tiptap/pm/model";
import type { Node as PmNode } from "@tiptap/pm/model";
import { NodeSelection, Plugin, PluginKey, Selection, TextSelection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";
import { ReplaceAroundStep } from "@tiptap/pm/transform";
import { Decoration, DecorationSet } from "@tiptap/pm/view";

import type { MessageKey } from "../i18n";

export const COLUMN_LIST_NODE = "columnList";
export const COLUMN_NODE = "column";

/** A column list holds at least this many columns (a single column is no layout)... */
export const MIN_COLUMNS = 2;
/** ... and at most this many. */
export const MAX_COLUMNS = 5;

/** The class the column holding the selection gets (the editor draws its outline). */
export const ACTIVE_COLUMN_CLASS = "is-active";

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    columns: {
      /**
       * Wraps the top-level block the selection is in into a column list of `count` columns
       * (2 to 5): the block in the first column, an empty paragraph in each other one.
       */
      setColumns: (count: number) => ReturnType;
      /** Appends an empty column to the column list around the selection (at most 5). */
      addColumn: () => ReturnType;
      /**
       * Removes the column the selection is in: its blocks join the previous column (the next
       * one for the first column). With two columns left the list is unwrapped instead.
       */
      removeColumn: () => ReturnType;
      /** Lifts every block out of the column list around the selection, in column order. */
      unwrapColumns: () => ReturnType;
    };
  }
}

/** A node and where it starts (just before it). */
interface Located {
  node: PmNode;
  pos: number;
}

function isNamed(node: PmNode, name: string): boolean {
  return node.type.name === name;
}

/**
 * The top-level block the selection is in (the selected block itself for a node selection
 * of one); `null` for a selection that is in none (a gap cursor).
 */
export function topLevelBlock(state: EditorState): Located | null {
  const { selection } = state;
  const { $from } = selection;
  if ($from.depth === 0) {
    return selection instanceof NodeSelection ? { node: selection.node, pos: selection.from } : null;
  }
  return { node: $from.node(1), pos: $from.before(1) };
}

/** True when `node` is a column list or holds one somewhere inside. */
function holdsColumns(node: PmNode): boolean {
  if (isNamed(node, COLUMN_LIST_NODE)) {
    return true;
  }
  let found = false;
  node.descendants((child) => {
    if (found || child.isTextblock) {
      return false;
    }
    found = isNamed(child, COLUMN_LIST_NODE);
    return !found;
  });
  return found;
}

/** The innermost column list around the selection; a selected column list counts. */
export function enclosingColumnList(state: EditorState): Located | null {
  const { selection } = state;
  if (selection instanceof NodeSelection && isNamed(selection.node, COLUMN_LIST_NODE)) {
    return { node: selection.node, pos: selection.from };
  }
  const found = findParentNodeClosestToPos(selection.$from, (node) => isNamed(node, COLUMN_LIST_NODE));
  return found === undefined ? null : { node: found.node, pos: found.pos };
}

/** The column the selection is in, with the list it belongs to and its place in it. */
export function enclosingColumn(state: EditorState): { list: Located; index: number } | null {
  const found = findParentNodeClosestToPos(state.selection.$from, (node) => isNamed(node, COLUMN_NODE));
  if (found === undefined) {
    return null;
  }
  const $column = state.doc.resolve(found.pos);
  if (!isNamed($column.parent, COLUMN_LIST_NODE)) {
    return null; // a column outside a list: a document no editor writes
  }
  return { list: { node: $column.parent, pos: $column.before() }, index: $column.index() };
}

/**
 * Why `/columns` cannot run where the selection is, as the message to show in the menu in
 * place of the hint, or `null` when it can: inside a column (no column list in a column), or
 * inside a table cell (the command wraps the whole top-level block, which would be the
 * table).
 */
export function columnsUnavailable(editor: Editor): MessageKey | null {
  const { $from } = editor.state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    const name = $from.node(depth).type.name;
    if (name === COLUMN_NODE) {
      return "slash.columns.inColumn";
    }
    if (name === "tableCell" || name === "tableHeader") {
      return "slash.columns.inCell";
    }
  }
  const block = topLevelBlock(editor.state);
  // A block that holds a column list (a callout around one) cannot go into a column either.
  return block !== null && holdsColumns(block.node) ? "slash.columns.inColumn" : null;
}

/** How many column lists sit inside a column (at any depth) in `node`. */
function nestedColumnLists(node: PmNode, inColumn = false): number {
  if (node.isTextblock || node.isLeaf) {
    return 0;
  }
  const within = inColumn || isNamed(node, COLUMN_NODE);
  let count = 0;
  node.forEach((child) => {
    if (within && isNamed(child, COLUMN_LIST_NODE)) {
      count += 1;
    }
    count += nestedColumnLists(child, within);
  });
  return count;
}

/**
 * True when `tr` leaves more column lists inside columns than the document had. A document
 * that already holds one (from a merge or another editor) can still be edited: only a change
 * that adds one is refused.
 */
function addsNestedColumns(tr: Transaction, before: EditorState): boolean {
  if (!tr.docChanged) {
    return false;
  }
  const now = nestedColumnLists(tr.doc);
  return now > 0 && now > nestedColumnLists(before.doc);
}

/** A range of positions that moved as a whole, now starting at `target`. */
interface Moved {
  from: number;
  to: number;
  target: number;
}

/**
 * Puts the selection back after blocks moved: a selection inside one of the `moved` ranges
 * keeps its place in it (a text selection, or a selected node); anything else (a selection
 * that spans several ranges, a cell selection) becomes a caret at the start of the range it
 * begins in, or at `fallback`.
 */
function keepSelection(state: EditorState, tr: Transaction, moved: readonly Moved[], fallback: number): void {
  const { selection } = state;
  const hit = moved.find((range) => selection.from >= range.from && selection.from <= range.to);
  const doc = tr.doc;
  const caretAt = (pos: number): void => {
    tr.setSelection(Selection.near(doc.resolve(Math.max(0, Math.min(pos, doc.content.size))), 1));
  };
  try {
    if (hit !== undefined && selection.to <= hit.to) {
      const delta = hit.target - hit.from;
      if (selection instanceof NodeSelection) {
        tr.setSelection(NodeSelection.create(doc, selection.from + delta));
        return;
      }
      if (selection instanceof TextSelection) {
        tr.setSelection(TextSelection.create(doc, selection.anchor + delta, selection.head + delta));
        return;
      }
    }
    caretAt(hit === undefined ? fallback : selection.from + hit.target - hit.from);
  } catch {
    caretAt(fallback);
  }
}

function setColumns(state: EditorState, tr: Transaction, count: number, apply: boolean): boolean {
  if (!Number.isInteger(count) || count < MIN_COLUMNS || count > MAX_COLUMNS) {
    return false;
  }
  const block = topLevelBlock(state);
  if (block === null || holdsColumns(block.node)) {
    return false; // no column list in a column, so no block holding one goes into a column
  }
  const column = state.schema.nodes[COLUMN_NODE];
  const columnList = state.schema.nodes[COLUMN_LIST_NODE];
  const paragraph = state.schema.nodes.paragraph;
  if (column === undefined || columnList === undefined || paragraph === undefined) {
    return false;
  }
  if (!apply) {
    return true;
  }
  // The block stays where it is and the list and its first column are put around it (what
  // `tr.wrap` does), with the other columns after it. Replacing the block with a copy inside a
  // list would work too, but then UniqueID, which treats an empty block followed by another
  // new one as the start of a split, would hand the block's id to the next column; wrapping
  // leaves the block out of the changed range, so it keeps its id (and the selection maps by
  // itself). The first column is empty in the slice: the block is its content.
  const others = Array.from({ length: count - 1 }, () => column.create(null, paragraph.create()));
  const wrapper = columnList.create(null, [column.create(null, Fragment.empty), ...others]);
  const end = block.pos + block.node.nodeSize;
  tr.step(new ReplaceAroundStep(block.pos, end, block.pos, end, new Slice(Fragment.from(wrapper), 0, 0), 2, true));
  return true;
}

function addColumn(state: EditorState, tr: Transaction, apply: boolean): boolean {
  const list = enclosingColumnList(state);
  const column = state.schema.nodes[COLUMN_NODE];
  const paragraph = state.schema.nodes.paragraph;
  if (list === null || list.node.childCount >= MAX_COLUMNS || column === undefined || paragraph === undefined) {
    return false;
  }
  if (apply) {
    tr.insert(list.pos + list.node.nodeSize - 1, column.create(null, paragraph.create()));
  }
  return true;
}

function unwrapColumns(state: EditorState, tr: Transaction, apply: boolean): boolean {
  const list = enclosingColumnList(state);
  if (list === null) {
    return false;
  }
  const blocks: PmNode[] = [];
  list.node.forEach((column) => {
    column.forEach((block) => {
      blocks.push(block);
    });
  });
  const lifted = Fragment.fromArray(blocks);
  const $list = state.doc.resolve(list.pos);
  if (blocks.length === 0 || !$list.parent.canReplace($list.index(), $list.index() + 1, lifted)) {
    return false;
  }
  if (!apply) {
    return true;
  }
  // Where each column's blocks start now: the list's own start plus the blocks before them.
  const moved: Moved[] = [];
  let before = 0; // size of the blocks of the columns before
  let offset = 0; // size of the columns before, their tags included
  list.node.forEach((column) => {
    const from = list.pos + 1 + offset + 1;
    moved.push({ from, to: from + column.content.size, target: list.pos + before });
    before += column.content.size;
    offset += column.nodeSize;
  });
  tr.replaceWith(list.pos, list.pos + list.node.nodeSize, lifted);
  keepSelection(state, tr, moved, list.pos);
  return true;
}

function removeColumn(state: EditorState, tr: Transaction, apply: boolean): boolean {
  const current = enclosingColumn(state);
  if (current === null) {
    return false;
  }
  const { list, index } = current;
  if (list.node.childCount <= MIN_COLUMNS) {
    return unwrapColumns(state, tr, apply); // one column is no layout: the blocks stay, in order
  }
  if (!apply) {
    return true;
  }
  // The blocks join the end of the previous column; the first column's join the start of the
  // next one, so that the reading order stays what it was.
  const target = index > 0 ? index - 1 : index + 1;
  const removed = list.node.child(index);
  const receiver = list.node.child(target);
  const merged = target < index ? receiver.content.append(removed.content) : removed.content.append(receiver.content);
  const columns: PmNode[] = [];
  let beforeRemoved = 0; // size of the columns before the removed one
  let beforeMerged = 0; // size of the columns before the first of the two that merge
  list.node.forEach((column, _at, i) => {
    if (i < index) {
      beforeRemoved += column.nodeSize;
    }
    if (i < Math.min(index, target)) {
      beforeMerged += column.nodeSize;
    }
    if (i !== index) {
      columns.push(i === target ? column.type.create(column.attrs, merged, column.marks) : column);
    }
  });
  const from = list.pos + 2 + beforeRemoved;
  const first = list.pos + 2 + beforeMerged; // where the merged column's blocks start
  tr.replaceWith(
    list.pos,
    list.pos + list.node.nodeSize,
    list.node.type.create(list.node.attrs, columns, list.node.marks),
  );
  keepSelection(
    state,
    tr,
    [{ from, to: from + removed.content.size, target: target < index ? first + receiver.content.size : first }],
    first,
  );
  return true;
}

/**
 * The commands as plain functions of a state and a transaction, for code that has no Tiptap
 * editor around it (the binding fuzz test drives them on bare ProseMirror views). Each returns
 * whether it applied; when it did, it has added its steps to `tr`.
 */
export const columnOperations = {
  setColumns: (state: EditorState, tr: Transaction, count: number): boolean => setColumns(state, tr, count, true),
  addColumn: (state: EditorState, tr: Transaction): boolean => addColumn(state, tr, true),
  removeColumn: (state: EditorState, tr: Transaction): boolean => removeColumn(state, tr, true),
  unwrapColumns: (state: EditorState, tr: Transaction): boolean => unwrapColumns(state, tr, true),
};

export interface ColumnListOptions {
  /**
   * True for the transactions the Loro binding dispatches itself (initial load, remote
   * changes): they are never filtered. `schema.ts` passes `isBindingTransaction`.
   */
  isBindingTransaction: (tr: Transaction) => boolean;
}

/**
 * The column list: two to five columns side by side. In HTML `<div data-type="columnList">`
 * holding the columns; `isolating`, so that editing keys do not reach across its edge.
 */
export const ColumnList = Node.create<ColumnListOptions>({
  name: COLUMN_LIST_NODE,
  group: "block",
  content: "column{2,5}",
  isolating: true,

  addOptions() {
    return { isBindingTransaction: () => false };
  },

  parseHTML() {
    return [{ tag: `div[data-type="${COLUMN_LIST_NODE}"]` }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-type": COLUMN_LIST_NODE }, HTMLAttributes), 0];
  },

  addCommands() {
    return {
      setColumns:
        (count) =>
        ({ state, tr, dispatch }) =>
          setColumns(state, tr, count, dispatch !== undefined),
      addColumn:
        () =>
        ({ state, tr, dispatch }) =>
          addColumn(state, tr, dispatch !== undefined),
      removeColumn:
        () =>
        ({ state, tr, dispatch }) =>
          removeColumn(state, tr, dispatch !== undefined),
      unwrapColumns:
        () =>
        ({ state, tr, dispatch }) =>
          unwrapColumns(state, tr, dispatch !== undefined),
    };
  },

  addProseMirrorPlugins() {
    const { isBindingTransaction } = this.options;
    return [
      new Plugin({
        key: new PluginKey("columnNesting"),
        filterTransaction: (tr, state) => isBindingTransaction(tr) || !addsNestedColumns(tr, state),
      }),
    ];
  },
});

/**
 * One column of a column list: blocks (`block+`). In HTML `<div data-type="column">`. The
 * column the selection is in carries the class {@link ACTIVE_COLUMN_CLASS} (a decoration,
 * nothing stored), which the editor's style outlines while the editor has focus.
 */
export const Column = Node.create({
  name: COLUMN_NODE,
  content: "block+",
  isolating: true,

  parseHTML() {
    return [{ tag: `div[data-type="${COLUMN_NODE}"]` }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-type": COLUMN_NODE }, HTMLAttributes), 0];
  },

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey("activeColumn"),
        props: {
          decorations: (state) => {
            const column = findParentNodeClosestToPos(state.selection.$from, (node) => isNamed(node, COLUMN_NODE));
            return column === undefined
              ? null
              : DecorationSet.create(state.doc, [
                  Decoration.node(column.pos, column.pos + column.node.nodeSize, { class: ACTIVE_COLUMN_CLASS }),
                ]);
          },
        },
      }),
    ];
  },
});
