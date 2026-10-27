// Vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9). MIT License: Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024
// LORO; see LICENSE-loro-prosemirror. Local changes are marked "atomik-kb patch" and
// listed in VENDORED.md.

import type { LoroDoc, UndoManager } from "loro-crdt";
// atomik-kb patch (a): ProseMirror from @tiptap/pm, the instance Tiptap uses.
import { PluginKey } from "@tiptap/pm/state";

export const loroUndoPluginKey = new PluginKey<LoroUndoPluginState>(
  "loro-undo",
);

export interface LoroUndoPluginProps {
  doc: LoroDoc;
  undoManager?: UndoManager;
}

export interface LoroUndoPluginState {
  undoManager: UndoManager;
  canUndo: boolean;
  canRedo: boolean;
  isUndoing: { current: boolean };
}
