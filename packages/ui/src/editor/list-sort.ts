// "Sort list A-Z" (ADR-0052): puts the items of one list in alphabetical order, once.
//
// - **Scope.** The innermost bullet, numbered or to-do list around the caret; nothing else on the page. An item moves
//   with everything in it (its nested lists, marks, ids, `checked`), and a nested list keeps its own order.
// - **Key.** The text of the item's first child, its paragraph, compared the way the sidebar sorts titles
//   (`sort-order.ts`: the language of the app, numbers by value, accents and case ignored, an empty item last). The
//   sort is stable: items with the same key keep the order they had.
// - **One transaction.** The list's content is replaced by the same item nodes in the new order, so the Loro binding
//   writes one update and one undo step reverses the sort (the house rule of `block-range-commands.ts`), and the ids of the
//   items travel with them. The caret goes back into the item it was in, at the same place in its text.
// - **Nothing to do** is no transaction at all: a list that already is in order, or has fewer than two items.

import { Fragment } from "@tiptap/pm/model";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Selection, TextSelection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";

import { compareTitles } from "../sort-order";

/** The nodes that hold list items. */
const LIST_NODES: ReadonlySet<string> = new Set(["bulletList", "orderedList", "taskList"]);

/** Whether a node is a bullet, numbered or to-do list. */
export function isListNode(node: PmNode | null | undefined): node is PmNode {
  return node !== null && node !== undefined && LIST_NODES.has(node.type.name);
}

/** The position just before the innermost list that contains `pos`, or `null` when `pos` is in no list. */
export function innermostListPos(doc: PmNode, pos: number): number | null {
  const $pos = doc.resolve(Math.max(0, Math.min(pos, doc.content.size)));
  for (let depth = $pos.depth; depth > 0; depth -= 1) {
    if (isListNode($pos.node(depth))) {
      return $pos.before(depth);
    }
  }
  return null;
}

/** What an item is sorted by: the text of its first child (its paragraph). */
export function itemKey(item: PmNode): string {
  return item.firstChild?.textContent ?? "";
}

/** The indexes of the list's items in the order they should have, or `null` when they already have it. */
export function sortedItemOrder(list: PmNode): number[] | null {
  const keys: string[] = [];
  list.forEach((item) => {
    keys.push(itemKey(item));
  });
  const order = keys.map((_, index) => index);
  // Explicit tie-break by position: the order does not depend on the engine's sort being stable.
  order.sort((a, b) => compareTitles(keys[a] ?? "", keys[b] ?? "") || a - b);
  return order.every((index, at) => index === at) ? null : order;
}

/**
 * Sorts the items of the list that starts at `listPos` in `tr.doc`, in `tr`. The selection follows the item it was in.
 * Returns whether anything changed (no step is added when not).
 */
export function sortListIn(tr: Transaction, listPos: number): boolean {
  const list = tr.doc.nodeAt(listPos);
  if (!isListNode(list)) {
    return false;
  }
  const order = sortedItemOrder(list);
  if (order === null) {
    return false;
  }
  // Where each item starts now, and where it will start.
  const items: PmNode[] = [];
  list.forEach((item) => {
    items.push(item);
  });
  const oldStart: number[] = [];
  let at = listPos + 1;
  for (const item of items) {
    oldStart.push(at);
    at += item.nodeSize;
  }
  const newStart: number[] = [];
  at = listPos + 1;
  for (const index of order) {
    newStart[index] = at;
    at += items[index]?.nodeSize ?? 0;
  }
  const remap = (pos: number): number | null => {
    for (const [index, item] of items.entries()) {
      const start = oldStart[index] ?? 0;
      if (pos >= start && pos <= start + item.nodeSize) {
        return (newStart[index] ?? start) + (pos - start);
      }
    }
    return null;
  };

  const { selection } = tr;
  const anchor = remap(selection.anchor);
  const head = remap(selection.head);
  const sorted = order.flatMap((index) => {
    const item = items[index];
    return item === undefined ? [] : [item];
  });
  tr.replaceWith(listPos + 1, listPos + list.nodeSize - 1, Fragment.fromArray(sorted));
  if (head !== null) {
    tr.setSelection(
      selection instanceof TextSelection
        ? TextSelection.create(tr.doc, anchor ?? head, head)
        : Selection.near(tr.doc.resolve(head)),
    );
  }
  return true;
}

/** The transaction that sorts the innermost list around the caret (or the list at `listPos`), or `null` when there is nothing to do. */
export function sortListTransaction(state: EditorState, listPos?: number): Transaction | null {
  const pos = listPos ?? innermostListPos(state.doc, state.selection.from);
  if (pos === null) {
    return null;
  }
  const tr = state.tr;
  return sortListIn(tr, pos) ? tr.scrollIntoView() : null;
}
