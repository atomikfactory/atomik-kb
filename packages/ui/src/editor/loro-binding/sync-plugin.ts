// Vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9). MIT License: Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024
// LORO; see LICENSE-loro-prosemirror. Local changes are marked "atomik-kb patch" and
// listed in VENDORED.md.

import type { ContainerID, Cursor, LoroEventBatch, LoroMap } from "loro-crdt";
// atomik-kb patch (a): ProseMirror from @tiptap/pm, the instance Tiptap uses.
import {
  Fragment,
  type Node,
  type ResolvedPos,
  Slice,
} from "@tiptap/pm/model";
import {
  type EditorState,
  Plugin,
  type Selection,
  type StateField,
  TextSelection,
  type Transaction,
} from "@tiptap/pm/state";
import { StepMap } from "@tiptap/pm/transform";
import type { EditorView } from "@tiptap/pm/view";

import {
  convertPmSelectionToCursors,
  cursorToAbsolutePosition,
} from "./cursor/common";
import {
  clearChangedNodes,
  createNodeFromLoroObj,
  findUnknownNames,
  type LoroDocType,
  type LoroNode,
  type LoroNodeContainerType,
  type LoroNodeMapping,
  NODE_NAME_KEY,
  ROOT_DOC_KEY,
  safeSelection,
  safeSetSelection,
  updateLoroToPmState,
  WEAK_NODE_TO_LORO_CONTAINER_MAPPING,
} from "./lib";
import {
  loroSyncPluginKey,
  type LoroSyncPluginProps,
  type LoroSyncPluginState,
} from "./sync-plugin-key";
import { configLoroTextStyle } from "./text-style";
import { loroUndoPluginKey } from "./undo-plugin-key";

type PluginTransactionType =
  | {
      type: "doc-changed";
    }
  | {
      type: "non-local-updates";
    }
  | {
      type: "update-state";
      state: Partial<LoroSyncPluginState>;
    };

// atomik-kb patch (d), upstream PR #85: the selection as Loro cursors.
type SelectionCursors = { anchor?: Cursor; focus?: Cursor };

// atomik-kb patch (r): the transactions the binding dispatches itself.
function isSyncTransaction(tr: Transaction): boolean {
  const meta = tr.getMeta(loroSyncPluginKey) as
    | PluginTransactionType
    | null
    | undefined;
  return meta?.type === "non-local-updates" || meta?.type === "update-state";
}

// atomik-kb patch (r): the binding's own transactions are not written back, so
// the document node they produce is registered here (top-level cursors use it).
function registerDocNode(view: EditorView, id: ContainerID): void {
  const state = loroSyncPluginKey.getState(view.state);
  if (view.isDestroyed || state == null) {
    return;
  }
  state.mapping.set(id, view.state.doc);
  WEAK_NODE_TO_LORO_CONTAINER_MAPPING.set(view.state.doc, id);
}

// atomik-kb patch (x): false when the cursor's container is gone (another
// replica deleted it, or moved its block, which the binding writes as a delete
// and an insert). Resolving such a cursor walks containers the view never
// mapped (an error log, and a position somewhere else in the document).
function isLiveCursor(doc: LoroDocType, cursor: Cursor): boolean {
  const container = doc.getContainerById(cursor.containerId());
  return (
    container != null && !("isDeleted" in container && container.isDeleted())
  );
}

// atomik-kb patch (x): the selection when Loro cursors cannot carry it (a node
// selection, or text whose container is gone). A text position stays in the
// textblock with the same `id` attribute when the new document has one (a
// block another replica wrapped, lifted or moved), at the same offset; other
// selections are mapped across the changed range, so the content before and
// after the change keeps them in place. Upstream mapped them through the
// whole-document replace, which moves every selection to the end.
function fallbackSelection(oldState: EditorState, doc: Node): Selection {
  const { selection } = oldState;
  if (selection instanceof TextSelection) {
    const anchor = sameTextblockPosition(selection.$anchor, doc);
    const head = sameTextblockPosition(selection.$head, doc);
    if (anchor != null && head != null) {
      return TextSelection.between(doc.resolve(anchor), doc.resolve(head));
    }
  }
  return selection.map(doc, changedRange(oldState.doc, doc));
}

// atomik-kb patch (x): `$pos` in the textblock of `doc` with the same `id`.
function sameTextblockPosition($pos: ResolvedPos, doc: Node): number | null {
  const id: unknown = $pos.parent.attrs.id;
  if (!$pos.parent.isTextblock || typeof id !== "string" || id === "") {
    return null;
  }
  let found: number | null = null;
  doc.descendants((node, pos) => {
    if (found != null) {
      return false;
    }
    if (node.isTextblock && node.attrs.id === id) {
      found = pos + 1 + Math.min($pos.parentOffset, node.content.size);
      return false;
    }
    return true;
  });
  return found;
}

// atomik-kb patch (x): the range where `oldDoc` and `doc` differ, as a step
// map (as prosemirror-view finds a DOM change: repeated content can put the
// end of the difference before its start).
function changedRange(oldDoc: Node, doc: Node): StepMap {
  const start = oldDoc.content.findDiffStart(doc.content);
  if (start == null) {
    return StepMap.empty;
  }
  const end = oldDoc.content.findDiffEnd(doc.content);
  let endA = end?.a ?? oldDoc.content.size;
  let endB = end?.b ?? doc.content.size;
  if (endA < start && oldDoc.content.size < doc.content.size) {
    endB = start + (endB - endA);
    endA = start;
  } else if (endB < start) {
    endA = start + (endA - endB);
    endB = start;
  }
  return new StepMap([start, endA - start, endB - start]);
}

export const LoroSyncPlugin = (props: LoroSyncPluginProps): Plugin => {
  // atomik-kb patch (d), upstream PR #85: the selection as Loro cursors, taken
  // at every view update, while ProseMirror and Loro agree. A remote change
  // resolves them after its import, so the selection stays with its text.
  let selectionCursors: SelectionCursors = {};
  const captureSelection = (view: EditorView): void => {
    selectionCursors = {};
    const state = loroSyncPluginKey.getState(view.state);
    const { selection } = view.state;
    if (
      state?.docSubscription == null ||
      selection.$anchor.depth === 0 ||
      selection.$head.depth === 0
    ) {
      return; // not initialised yet, or not inside a textblock
    }
    const { anchor, focus } = convertPmSelectionToCursors(
      view.state.doc,
      selection,
      state,
    );
    selectionCursors = { anchor, focus };
  };

  return new Plugin({
    key: loroSyncPluginKey,
    props: {
      editable: (state) => {
        const syncState = loroSyncPluginKey.getState(state);
        // atomik-kb patch (e): read-only once the binding refuses to write.
        return (
          syncState?.snapshot == null && syncState?.readOnlyReason == null
        );
      },
    },
    state: {
      init: (_config, editorState): LoroSyncPluginState => {
        configLoroTextStyle(props.doc, editorState.schema);

        return {
          doc: props.doc,
          mapping: props.mapping ?? new Map(),
          changedBy: "local",
          containerId: props.containerId,
          // atomik-kb patch (e).
          readOnlyReason:
            props.readOnly === true ? { reason: "requested" } : null,
        };
      },
      apply: (tr, state, oldEditorState, newEditorState) => {
        const meta = tr.getMeta(
          loroSyncPluginKey,
        ) as PluginTransactionType | null;
        const undoState = loroUndoPluginKey.getState(oldEditorState);

        if (meta?.type === "non-local-updates") {
          state.changedBy = "import";
        } else {
          state.changedBy = "local";
        }
        switch (meta?.type) {
          case "doc-changed":
            // atomik-kb patch (e), upstream issue #77: nothing is written
            // before `init` replaced the editor's initial content with the
            // Loro document (writing that content would overwrite the
            // document), nor once the binding is read-only.
            if (
              !undoState?.isUndoing.current &&
              state.docSubscription != null &&
              state.readOnlyReason == null
            ) {
              updateLoroToPmState(
                state.doc as LoroDocType,
                state.mapping,
                newEditorState,
                props.containerId,
              );
            }
            break;
          case "update-state":
            state = { ...state, ...meta.state };
            // atomik-kb patch (b): no `timestamp: Date.now()` (ADR-0011).
            state.doc.commit({
              origin: "sys:init",
            });
            break;
          default:
            break;
        }
        return state;
      },
    } as StateField<LoroSyncPluginState>,
    appendTransaction: (transactions, _oldEditorState, newEditorState) => {
      // atomik-kb patch (r): the binding's own transactions (the initial load,
      // remote changes, undo and redo) show what the document holds. Writing
      // them back made every replica write its own completion of merged content
      // (see `createNodeFromLoroObj`), so only other transactions are written.
      if (transactions.some((tr) => tr.docChanged && !isSyncTransaction(tr))) {
        return newEditorState.tr.setMeta(loroSyncPluginKey, {
          type: "doc-changed",
        });
      }
      return null;
    },
    view: (view: EditorView) => {
      const timeoutId = setTimeout(
        () => init(view, () => selectionCursors),
        0,
      );
      return {
        update: (_view: EditorView, _prevState: EditorState) => {
          // atomik-kb patch (d), upstream PR #85.
          captureSelection(view);
        },
        destroy: () => {
          clearTimeout(timeoutId);
          // atomik-kb patch (u): stop listening to the document with the view.
          loroSyncPluginKey.getState(view.state)?.docSubscription?.();
        },
      };
    },
  });
};

// This is called when the plugin's state is associated with an editor view
function init(view: EditorView, getCursors: () => SelectionCursors) {
  if (view.isDestroyed) {
    return;
  }

  const state = loroSyncPluginKey.getState(view.state) as LoroSyncPluginState;

  let docSubscription = state.docSubscription;

  docSubscription?.();

  if (state.containerId) {
    docSubscription = state
      .doc!.getContainerById(state.containerId)!
      .subscribe((event) => {
        updateNodeOnLoroEvent(view, event, getCursors());
      });
  } else {
    docSubscription = state.doc.subscribe((event) =>
      updateNodeOnLoroEvent(view, event, getCursors()),
    );
  }

  const innerDoc = state.containerId
    ? (state.doc.getContainerById(
        state.containerId,
      ) as LoroMap<LoroNodeContainerType>)
    : (state.doc as LoroDocType).getMap(ROOT_DOC_KEY);

  const mapping: LoroNodeMapping = new Map();
  if (innerDoc.size === 0 || innerDoc.get(NODE_NAME_KEY) == null) {
    // atomik-kb patch (e), upstream issue #77: upstream wrote the editor's
    // default document into an empty Loro document here, racing with content
    // that arrives later (two roots: one wins, the other is lost). A document
    // must exist before an editor opens it, so an empty one is a programming
    // error: the editor stays read-only and nothing is written.
    console.error(
      "loro-prosemirror: the Loro document has no root node; the editor stays read-only",
    );
    const tr = view.state.tr;
    tr.setMeta(loroSyncPluginKey, {
      type: "update-state",
      state: {
        mapping,
        docSubscription,
        snapshot: null,
        readOnlyReason: { reason: "empty-document" },
      },
    });
    view.dispatch(tr);
  } else {
    const schema = view.state.schema;
    // atomik-kb patch (e): never write into a document whose content the
    // schema cannot represent (the binding drops what it cannot create).
    const unknown = findUnknownNames(schema, innerDoc as LoroNode);
    const readOnlyReason =
      state.readOnlyReason ??
      (unknown.nodes.length > 0 || unknown.marks.length > 0
        ? { reason: "unknown-content" as const, ...unknown }
        : null);
    // Create node from loro object
    const node = createNodeFromLoroObj(
      schema,
      innerDoc as LoroMap<LoroNodeContainerType>,
      mapping,
    );
    const tr = view.state.tr.replace(
      0,
      view.state.doc.content.size,
      new Slice(Fragment.from(node), 0, 0),
    );
    tr.setMeta(loroSyncPluginKey, {
      type: "update-state",
      state: { mapping, docSubscription, snapshot: null, readOnlyReason },
    });
    view.dispatch(tr);
    registerDocNode(view, innerDoc.id);
  }
}

function updateNodeOnLoroEvent(
  view: EditorView,
  event: LoroEventBatch,
  cursors: SelectionCursors,
) {
  if (view.isDestroyed) {
    return;
  }

  const state = loroSyncPluginKey.getState(view.state) as LoroSyncPluginState;
  // atomik-kb patch (y): a batch that touches no container of the editor's tree (a whiteboard's
  // elements live in the root map `whiteboards`, ADR-0033) changes nothing the editor shows: do not
  // rebuild the whole document for it.
  if (
    !state.containerId &&
    event.events.length > 0 &&
    event.events.every((change) => change.path[0] !== ROOT_DOC_KEY)
  ) {
    return;
  }
  state.changedBy = event.by;
  if (event.by === "local" && event.origin !== "undo") {
    return;
  }

  const mapping = state.mapping;
  clearChangedNodes(state.doc as LoroDocType, event, mapping);
  const innerDoc = state.containerId
    ? (state.doc.getContainerById(
        state.containerId,
      ) as LoroMap<LoroNodeContainerType>)
    : (state.doc as LoroDocType).getMap(ROOT_DOC_KEY);
  if (state.readOnlyReason == null) {
    // atomik-kb patch (e): content the schema cannot represent turns the
    // binding read-only before the view changes, so the write that follows
    // this transaction cannot delete it.
    const unknown = findUnknownNames(view.state.schema, innerDoc as LoroNode);
    if (unknown.nodes.length > 0 || unknown.marks.length > 0) {
      state.readOnlyReason = { reason: "unknown-content", ...unknown };
    }
  }
  const node = createNodeFromLoroObj(view.state.schema, innerDoc, mapping);

  const tr = view.state.tr.replace(
    0,
    view.state.doc.content.size,
    new Slice(Fragment.from(node), 0, 0),
  );

  tr.setMeta(loroSyncPluginKey, {
    type: "non-local-updates",
  });

  // atomik-kb patch (d), upstream PR #85: the selection is restored in the
  // same transaction as the remote change (upstream dispatched a second
  // transaction from a `setTimeout`, so a keystroke in between landed at a
  // stale position), from cursors taken before the change (upstream took them
  // from the old view against the already changed document).
  const { anchor, focus } = cursors;
  const doc = state.doc as LoroDocType;
  let selection: Selection | null = null;
  // atomik-kb patch (x): only cursors whose text still exists, else the
  // fallback selection.
  if (
    anchor != null &&
    isLiveCursor(doc, anchor) &&
    (focus == null || isLiveCursor(doc, focus))
  ) {
    try {
      const anchorPos = cursorToAbsolutePosition(anchor, doc, mapping)[0];
      const focusPos =
        focus && cursorToAbsolutePosition(focus, doc, mapping)[0];
      selection = safeSelection(tr.doc, anchorPos, focusPos);
    } catch {
      selection = null;
    }
  }
  tr.setSelection(selection ?? fallbackSelection(view.state, tr.doc));
  view.dispatch(tr);
  registerDocNode(view, innerDoc.id);
}

/**
 * Update ProseMirror selection based on the given Loro cursors.
 */
export function syncCursorsToPmSelection(
  view: EditorView,
  anchor: Cursor,
  focus?: Cursor,
) {
  if (view.isDestroyed) {
    return;
  }

  const state = loroSyncPluginKey.getState(view.state);
  if (!state) {
    return;
  }

  const { doc, mapping } = state;
  // atomik-kb patch (x): a cursor whose text is gone keeps the selection.
  if (
    !isLiveCursor(doc, anchor) ||
    (focus != null && !isLiveCursor(doc, focus))
  ) {
    return;
  }
  const anchorPos = cursorToAbsolutePosition(anchor, doc, mapping)[0];
  const focusPos = focus && cursorToAbsolutePosition(focus, doc, mapping)[0];
  if (anchorPos == null) {
    return;
  }

  // If the cursors are synced faster than the document, then the cursors might
  // be out of bounds. Thus, we need to check if the cursors are out of bounds.
  safeSetSelection(view, anchorPos, focusPos);
}
