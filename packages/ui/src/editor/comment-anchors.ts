// Where a comment points in a page (ADR-0048): a block id plus Loro cursors, made from the editor's selection and resolved back
// to a range of the document, with the binding's cursor conversions (`loro-binding/cursor/common.ts`). Comments live outside
// the page document, so an anchor is a hint: the cursors follow later edits of the text, and one that can no longer be resolved
// (the text is gone, the cursors do not belong to this document) is `null`, which the panel shows as a page-level thread with
// its quote.

import { COMMENT_QUOTE_MAX_CHARS, fromBase64, toBase64 } from "@atomik-kb/core-client";
import type { CommentAnchor } from "@atomik-kb/core-client";
import { TextSelection } from "@tiptap/pm/state";
import type { EditorState } from "@tiptap/pm/state";
import { Cursor } from "loro-crdt";

import { loroSyncPluginKey } from "./loro-binding";
import { convertPmSelectionToCursors, cursorToAbsolutePosition } from "./loro-binding/cursor/common";

const BLOCK_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu;

/** A range of the document, in editor positions. */
export interface DocRange {
  from: number;
  to: number;
}

/** The words of a range as a quote: whitespace collapsed, at most 200 characters. */
export function quoteOf(text: string): string {
  return Array.from(text.replace(/\s+/gu, " ").trim()).slice(0, COMMENT_QUOTE_MAX_CHARS).join("");
}

/**
 * The anchor of the selection: its block (the nearest block with an id above its start), the Loro cursors of both ends when the
 * binding is linked and both ends sit in text, and the quoted words. `null` unless a range of text is selected.
 */
export function anchorOfSelection(state: EditorState): CommentAnchor | null {
  const { selection, doc } = state;
  if (!(selection instanceof TextSelection) || selection.empty) {
    return null;
  }
  const { from, to } = selection;
  const quote = quoteOf(doc.textBetween(from, to, " ", " "));
  const anchor: CommentAnchor = { quote };
  const $from = doc.resolve(from);
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    const id: unknown = $from.node(depth).attrs.id;
    if (typeof id === "string" && BLOCK_ID.test(id)) {
      anchor.blockId = id;
      break;
    }
  }
  const sync = loroSyncPluginKey.getState(state);
  if (sync?.docSubscription != null) {
    try {
      const { anchor: start, focus: end } = convertPmSelectionToCursors(doc, TextSelection.create(doc, from, to), sync);
      if (start !== undefined && end !== undefined) {
        anchor.start = toBase64(start.encode());
        anchor.end = toBase64(end.encode());
      }
    } catch {
      // No cursors: the anchor stays a block-level one.
    }
  }
  return anchor;
}

/** The first node with this block id, as its position and size; `null` when the document has none. */
function findBlock(state: EditorState, blockId: string): { pos: number; size: number; inline: boolean } | null {
  let found: { pos: number; size: number; inline: boolean } | null = null;
  state.doc.descendants((node, pos) => {
    if (found !== null) {
      return false;
    }
    if (node.attrs.id === blockId) {
      found = { pos, size: node.nodeSize, inline: node.inlineContent };
      return false;
    }
    return true;
  });
  return found;
}

/**
 * The range an anchor points at now: between its two cursors when it has them, else the text of its block. `null` when it cannot be
 * resolved (the range collapsed because its text was deleted, the cursors are not of this document, the block is gone), and
 * `"pending"` while the binding has not linked the editor to the document yet (try again after the next update).
 */
export function anchorRange(state: EditorState, anchor: CommentAnchor): DocRange | null | "pending" {
  const sync = loroSyncPluginKey.getState(state);
  if (anchor.start !== undefined && anchor.end !== undefined) {
    if (sync?.docSubscription == null) {
      return "pending";
    }
    try {
      const [first] = cursorToAbsolutePosition(Cursor.decode(fromBase64(anchor.start)), sync.doc, sync.mapping);
      const [second] = cursorToAbsolutePosition(Cursor.decode(fromBase64(anchor.end)), sync.doc, sync.mapping);
      const from = Math.min(first, second);
      const to = Math.max(first, second);
      return from < to && from >= 0 && to <= state.doc.content.size ? { from, to } : null;
    } catch {
      return null;
    }
  }
  if (anchor.blockId === undefined) {
    return null;
  }
  const block = findBlock(state, anchor.blockId);
  if (block === null || !block.inline || block.size <= 2) {
    return null;
  }
  return { from: block.pos + 1, to: block.pos + block.size - 1 };
}
