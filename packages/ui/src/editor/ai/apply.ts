// Putting an AI answer in the page (ADR-0037 decision 3, `docs/architecture/ai.md` section 8.3): the
// Markdown is parsed into portable blocks, built as nodes of the live schema (`nodesFromPortable`,
// the paste path of Markdown) and written in ONE transaction, so the Loro binding writes one update and
// one undo step takes it back. Nothing reaches the page before the person chooses Replace or Insert below.

import { parseMarkdown, portableNodesToJson } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Selection, TextSelection } from "@tiptap/pm/state";

import { insertBelowTransaction, nodesFromPortable, pasteTransaction } from "../markdown-paste";

/** Where an answer goes. */
export type ApplyTarget =
  /** In place of the text between `from` and `to` (inside text blocks). */
  | { mode: "replace"; from: number; to: number }
  /** In place of whole top-level blocks, from the start of the first to the end of the last. */
  | { mode: "replaceBlocks"; from: number; to: number }
  /** After the block `pos` is in; an empty paragraph there is replaced with `replaceEmpty`. */
  | { mode: "below"; pos: number; replaceEmpty: boolean };

/** The blocks of the live schema an answer's Markdown stands for (none for text that holds nothing). */
export function nodesOfMarkdown(editor: Editor, markdown: string): PmNode[] {
  const blocks = parseMarkdown(markdown);
  return nodesFromPortable(editor.schema, portableNodesToJson(blocks));
}

function clamp(editor: Editor, pos: number): number {
  return Math.max(0, Math.min(pos, editor.state.doc.content.size));
}

/**
 * Writes `markdown` where `target` says, in one transaction. `false` when there is nothing to write
 * (the answer holds no blocks) or the page does not take it there.
 */
export function applyMarkdown(editor: Editor, markdown: string, target: ApplyTarget): boolean {
  if (!editor.isEditable || editor.isDestroyed) {
    return false;
  }
  const nodes = nodesOfMarkdown(editor, markdown);
  if (nodes.length === 0) {
    return false;
  }
  const { state } = editor;
  let tr;
  switch (target.mode) {
    case "replace": {
      const from = clamp(editor, Math.min(target.from, target.to));
      const to = clamp(editor, Math.max(target.from, target.to));
      const selection = TextSelection.between(state.doc.resolve(from), state.doc.resolve(to));
      tr = pasteTransaction(state, nodes, selection);
      break;
    }
    case "replaceBlocks": {
      const from = clamp(editor, Math.min(target.from, target.to));
      const to = clamp(editor, Math.max(target.from, target.to));
      tr = state.tr;
      try {
        tr.replaceWith(from, to, nodes);
      } catch {
        return false;
      }
      tr.setSelection(Selection.near(tr.doc.resolve(tr.mapping.map(to, 1)), -1));
      break;
    }
    case "below":
      tr = insertBelowTransaction(state, nodes, clamp(editor, target.pos), target.replaceEmpty);
      break;
  }
  if (tr === null || !tr.docChanged) {
    return false;
  }
  editor.view.dispatch(tr.scrollIntoView());
  editor.view.focus();
  return true;
}
