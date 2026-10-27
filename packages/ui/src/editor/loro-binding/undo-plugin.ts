// Vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9). MIT License: Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024
// LORO; see LICENSE-loro-prosemirror. Local changes are marked "atomik-kb patch" and
// listed in VENDORED.md.

import type { Cursor } from "loro-crdt";
import { UndoManager } from "loro-crdt";
// atomik-kb patch (a): ProseMirror from @tiptap/pm, the instance Tiptap uses.
import {
  type Command,
  type EditorState,
  Plugin,
  type StateField,
} from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { convertPmSelectionToCursors } from "./cursor/common";
import { syncCursorsToPmSelection } from "./sync-plugin";
import { loroSyncPluginKey } from "./sync-plugin-key";
import { configLoroTextStyle } from "./text-style";
import {
  loroUndoPluginKey,
  type LoroUndoPluginProps,
  type LoroUndoPluginState,
} from "./undo-plugin-key";

type Cursors = { anchor: Cursor | null; focus: Cursor | null };
export const LoroUndoPlugin = (props: LoroUndoPluginProps): Plugin => {
  const undoManager = props.undoManager || new UndoManager(props.doc, {});
  let lastSelection: Cursors = {
    anchor: null,
    focus: null,
  };
  return new Plugin({
    key: loroUndoPluginKey,
    state: {
      init: (_config, editorState): LoroUndoPluginState => {
        configLoroTextStyle(props.doc, editorState.schema);

        undoManager.addExcludeOriginPrefix("sys:init");
        return {
          undoManager,
          canUndo: undoManager.canUndo(),
          canRedo: undoManager.canRedo(),
          isUndoing: { current: false },
        };
      },
      apply: (_tr, state, oldEditorState, _newEditorState) => {
        const undoState = loroUndoPluginKey.getState(oldEditorState);
        const loroState = loroSyncPluginKey.getState(oldEditorState);
        if (!undoState || !loroState) {
          return state;
        }

        const canUndo = undoState.undoManager.canUndo();
        const canRedo = undoState.undoManager.canRedo();
        {
          const { anchor, focus } = convertPmSelectionToCursors(
            oldEditorState.doc,
            oldEditorState.selection,
            loroState,
          );
          lastSelection = {
            anchor: anchor ?? null,
            focus: focus ?? null,
          };
        }
        return {
          ...state,
          canUndo,
          canRedo,
        };
      },
    } as StateField<LoroUndoPluginState>,

    view: (view: EditorView) => {
      undoManager.setOnPush((isUndo, _counterRange) => {
        const loroState = loroSyncPluginKey.getState(view.state);
        if (loroState?.doc == null) {
          return {
            value: null,
            cursors: [],
          };
        }

        const cursors: Cursor[] = [];
        let selection = lastSelection;
        if (!isUndo) {
          const loroState = loroSyncPluginKey.getState(view.state);
          if (loroState) {
            const { anchor, focus } = convertPmSelectionToCursors(
              view.state.doc,
              view.state.selection,
              loroState,
            );
            selection.anchor = anchor || null;
            selection.focus = focus || null;
          }
        }

        if (selection.anchor) {
          cursors.push(selection.anchor);
        }
        if (selection.focus) {
          cursors.push(selection.focus);
        }

        return {
          value: null,
          // The undo manager will internally transform the cursors.
          // Undo/redo operations may recreate deleted content, so we need to remap
          // the cursors to their new positions. Additionally, if containers are deleted
          // and recreated, they also need remapping. Remote changes to the document
          // should be considered in these transformations.
          cursors,
        };
      });
      undoManager.setOnPop((_isUndo, meta, _counterRange) => {
        // After this call, the `onPush` will be called immediately.
        // The immediate `onPush` will contain the inverse operations that undone the effect caused by the current `onPop`
        const loroState = loroSyncPluginKey.getState(view.state);
        if (loroState?.doc == null) {
          return;
        }

        const anchor: Cursor | undefined = meta.cursors[0];
        const focus: Cursor | undefined = meta.cursors[1];
        if (anchor == null) {
          return;
        }
        setTimeout(() => {
          syncCursorsToPmSelection(view, anchor, focus);
        }, 0);
      });
      return {
        destroy: () => {
          undoManager.setOnPop();
          undoManager.setOnPush();
        },
      };
    },
  });
};

export function canUndo(state: EditorState): boolean {
  const undoState = loroUndoPluginKey.getState(state);
  return undoState?.undoManager.canUndo() || false;
}

export function canRedo(state: EditorState): boolean {
  const undoState = loroUndoPluginKey.getState(state);
  return undoState?.undoManager.canRedo() || false;
}

export const undo: Command = (state, dispatch): boolean => {
  const undoState = loroUndoPluginKey.getState(state);
  if (!undoState) {
    return false;
  }

  if (!dispatch) {
    return undoState.undoManager.canUndo();
  }

  // atomik-kb patch (v): loro-crdt delivers the undo's events before `undo()`
  // returns, so the flag is cleared right after it instead of in a `setTimeout`
  // (a local edit dispatched before that timeout was never written to Loro).
  undoState.isUndoing.current = true;
  try {
    return undoState.undoManager.undo();
  } finally {
    undoState.isUndoing.current = false;
  }
};

export const redo: Command = (state, dispatch): boolean => {
  const undoState = loroUndoPluginKey.getState(state);
  if (!undoState) {
    return false;
  }

  if (!dispatch) {
    return undoState.undoManager.canRedo();
  }

  // atomik-kb patch (v), as in `undo`.
  undoState.isUndoing.current = true;
  try {
    return undoState.undoManager.redo();
  } finally {
    undoState.isUndoing.current = false;
  }
};
