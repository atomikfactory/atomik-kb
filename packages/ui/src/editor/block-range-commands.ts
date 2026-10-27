// The bulk actions of the block handle's menu on a range of top-level blocks (`block-selection.ts`):
// delete, duplicate, move up and down, turn into. Each builds ONE transaction, so the Loro binding
// writes one update and one undo step reverses the whole action. A range of one block is the
// single-block command of `blocks.ts` (same result, same caret afterwards); a longer range keeps
// the blocks it acted on selected (the copies after a duplicate), so that the next action can
// follow, and puts a caret where deleted blocks were.

import { Fragment } from "@tiptap/pm/model";
import { Selection, TextSelection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";

import { BlockRangeSelection, blockCount, blocksOf, blockStart } from "./block-selection";
import type { BlockRange } from "./block-selection";
import { convertBlock, deleteBlock, duplicateBlock, moveBlock, turnInto, turnIntoKinds, withoutIds } from "./blocks";
import type { BlockKind } from "./blocks";

/** Deletes the blocks; deleting every block of the page leaves one empty paragraph. */
export function deleteBlocks(state: EditorState, range: BlockRange): Transaction | null {
  if (blockCount(range) === 1) {
    return deleteBlock(state, range.from);
  }
  const { doc } = state;
  if (range.first === 0 && range.last === doc.childCount - 1) {
    const paragraph = state.schema.nodes.paragraph;
    if (paragraph === undefined) {
      return null;
    }
    const tr = state.tr.replaceWith(range.from, range.to, paragraph.create());
    return tr.setSelection(TextSelection.create(tr.doc, 1));
  }
  const tr = state.tr.delete(range.from, range.to);
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(range.from, tr.doc.content.size))));
  return tr;
}

/** Inserts a copy of the blocks (with new ids) right after the range and selects the copies. */
export function duplicateBlocks(state: EditorState, range: BlockRange): Transaction | null {
  if (blockCount(range) === 1) {
    return duplicateBlock(state, range.from);
  }
  const copies = blocksOf(state.doc, range).map(withoutIds);
  const tr = state.tr.insert(range.to, Fragment.fromArray(copies));
  tr.setSelection(BlockRangeSelection.create(tr.doc, range.last + 1, range.last + copies.length));
  return tr.scrollIntoView();
}

/** True when the blocks can move in `direction` (`-1` up, `1` down). */
export function canMoveBlocks(state: EditorState, range: BlockRange, direction: -1 | 1): boolean {
  return direction < 0 ? range.first > 0 : range.last < state.doc.childCount - 1;
}

/** Moves the blocks, together, one place up or down; they stay selected. `null` at the edge of the page. */
export function moveBlocks(state: EditorState, range: BlockRange, direction: -1 | 1): Transaction | null {
  if (!canMoveBlocks(state, range, direction)) {
    return null;
  }
  if (blockCount(range) === 1) {
    return moveBlock(state, range.from, direction);
  }
  const { doc } = state;
  const blocks = Fragment.fromArray(blocksOf(doc, range));
  const tr = state.tr.delete(range.from, range.to);
  // Moving up: the blocks go where the previous one starts. Moving down: after the next one,
  // which now starts where the range started.
  const target = direction < 0 ? blockStart(doc, range.first - 1) : range.from + doc.child(range.last + 1).nodeSize;
  tr.insert(target, blocks);
  tr.setSelection(BlockRangeSelection.create(tr.doc, range.anchorIndex + direction, range.headIndex + direction));
  return tr.scrollIntoView();
}

/** The kinds the menu offers for a range: those one of its blocks can be turned into (in the order of the first that lists them). */
export function turnIntoKindsOf(state: EditorState, range: BlockRange): BlockKind[] {
  const kinds: BlockKind[] = [];
  for (const block of blocksOf(state.doc, range)) {
    for (const kind of turnIntoKinds(block)) {
      if (!kinds.includes(kind)) {
        kinds.push(kind);
      }
    }
  }
  return kinds;
}

/** True when `kind` changes at least one block of the range. */
export function canTurnBlocksInto(state: EditorState, range: BlockRange, kind: BlockKind): boolean {
  return blocksOf(state.doc, range).some((block) => convertBlock(block, kind, state.schema) !== null);
}

/**
 * Turns each block of the range into `kind`. A block that already is that kind, or that the
 * conversion would damage (text kinds for an attachment, a table or a column layout), stays as
 * it is, untouched. `null` when no block changes.
 */
export function turnBlocksInto(state: EditorState, range: BlockRange, kind: BlockKind): Transaction | null {
  if (blockCount(range) === 1) {
    return turnInto(state, range.from, kind);
  }
  let pos = range.from;
  const items = blocksOf(state.doc, range).map((block) => {
    const item = { pos, size: block.nodeSize, nodes: convertBlock(block, kind, state.schema) };
    pos += block.nodeSize;
    return item;
  });
  if (items.every((item) => item.nodes === null)) {
    return null;
  }
  const tr = state.tr;
  // Last to first: the positions of the blocks before one stay valid when it is replaced.
  for (const { pos: start, size, nodes } of [...items].reverse()) {
    if (nodes !== null) {
      tr.replaceWith(start, start + size, nodes);
    }
  }
  const count = items.reduce((sum, item) => sum + (item.nodes?.length ?? 1), 0);
  const last = range.first + count - 1;
  const forward = range.anchorIndex <= range.headIndex;
  tr.setSelection(BlockRangeSelection.create(tr.doc, forward ? range.first : last, forward ? last : range.first));
  return tr.scrollIntoView();
}
