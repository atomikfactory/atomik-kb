// The editor of the page in the front tab, for features that live outside the editor (the AI chat
// panel's "Insert into page", ADR-0037). `PageEditor` registers its editor while mounted and
// editable; there is at most one (only the front tab is mounted, ADR-0034).
//
// Owned by the writing-assistant work (packages/ui/src/editor/ai); the chat panel only calls
// `insertMarkdownAtEnd`.

import type { Editor } from "@tiptap/core";

import type { PageId } from "@atomik-kb/core-client";

import { nodesOfMarkdown } from "./apply";

let active: { pageId: PageId; editor: Editor } | null = null;

/** Called by `PageEditor` when its editor is created (and with `null` when it is destroyed). */
export function setActiveEditor(entry: { pageId: PageId; editor: Editor } | null): void {
  active = entry;
}

/** Called when `editor` goes: forgets it, unless another editor was registered since. */
export function clearActiveEditor(editor: Editor): void {
  if (active?.editor === editor) {
    active = null;
  }
}

/** The editable editor of the front page, if any. */
export function getActiveEditor(): { pageId: PageId; editor: Editor } | null {
  const entry = active;
  if (entry === null || entry.editor.isDestroyed || !entry.editor.isEditable) {
    return null;
  }
  return entry;
}

/**
 * Appends Markdown (parsed into blocks) at the end of the front page in one transaction (one undo
 * step). `false` when no editable page is open, or the Markdown holds no block.
 */
export function insertMarkdownAtEnd(markdown: string): boolean {
  const entry = getActiveEditor();
  if (entry === null) {
    return false;
  }
  const { editor } = entry;
  const nodes = nodesOfMarkdown(editor, markdown);
  if (nodes.length === 0) {
    return false;
  }
  const { doc } = editor.state;
  const tr = editor.state.tr;
  const last = doc.lastChild;
  const emptyLast =
    last !== null && last.type.name === "paragraph" && last.content.size === 0 && doc.childCount > 0;
  try {
    if (emptyLast) {
      // The empty line the page ends with (the one a click below the last block makes) takes the blocks' place.
      tr.replaceWith(doc.content.size - last.nodeSize, doc.content.size, nodes);
    } else {
      tr.insert(doc.content.size, nodes);
    }
  } catch {
    return false;
  }
  if (!tr.docChanged) {
    return false;
  }
  editor.view.dispatch(tr.scrollIntoView());
  return true;
}
