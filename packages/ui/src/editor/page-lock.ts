// The lock of a page (ADR-0040) as the page editor honours it: a guard against accidental edits, not a
// permission. A locked page opens read-only, and the editor flips when the lock does (this device's menu, or
// another device's change arriving as a new `PageInfo`) without being rebuilt: the selection, the scroll and
// the open document stay.
//
// The read-only state is the binding's (`readOnlyReason`, atomik-kb patch (e)): while it is set the view is not
// editable (`editor.isEditable` is false, so the format menu, the block handle, the find bar's replace, the AI
// writer and drops all stand down) and nothing is written to the Loro document. The lock sets the reason
// `requested` (the host asked for it) and clears only that one: a page that is read-only for another reason (a
// block this version cannot show) stays so.

import type { Editor } from "@tiptap/core";

import { ROOT_DOC_KEY, findUnknownNames, loroSyncPluginKey } from "./loro-binding";
import type { LoroDocType, LoroSyncReadOnly } from "./loro-binding";

/**
 * Locks or unlocks the editor's page. Does nothing when the editor is already in that state, or is read-only
 * for another reason. The change is one empty transaction, so every part of the UI that follows the editor
 * (`useEditorState`) draws again.
 */
export function setPageLock(editor: Editor, locked: boolean): void {
  if (editor.isDestroyed) {
    return;
  }
  const state = loroSyncPluginKey.getState(editor.state);
  if (state === undefined) {
    return;
  }
  const current = state.readOnlyReason ?? null;
  let next: LoroSyncReadOnly | null;
  if (locked) {
    if (current !== null) {
      return;
    }
    next = { reason: "requested" };
  } else {
    if (current?.reason !== "requested") {
      return;
    }
    // A remote change may have brought a block this version cannot show while the page was locked (the binding
    // looks only while it is writing): the page is then read-only for that reason instead.
    const unknown = unknownContentOf(editor, state.doc);
    next = unknown === null ? null : { reason: "unknown-content", ...unknown };
  }
  editor.view.dispatch(editor.state.tr.setMeta(loroSyncPluginKey, { type: "update-state", state: { readOnlyReason: next } }));
}

/** The node and mark names the document holds that the editor's schema does not know; `null` when none. */
function unknownContentOf(editor: Editor, doc: LoroDocType | undefined): { nodes: string[]; marks: string[] } | null {
  if (doc === undefined) {
    return null;
  }
  const root = doc.getMap(ROOT_DOC_KEY);
  if (root.size === 0) {
    return null;
  }
  const unknown = findUnknownNames(editor.schema, root);
  return unknown.nodes.length > 0 || unknown.marks.length > 0 ? unknown : null;
}
