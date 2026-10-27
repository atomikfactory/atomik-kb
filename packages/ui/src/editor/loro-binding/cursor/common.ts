// Vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9). MIT License: Copyright (c) 2019 Kevin Jahns, Copyright (c) 2024
// LORO; see LICENSE-loro-prosemirror. Local changes are marked "atomik-kb patch" and
// listed in VENDORED.md.
//
// atomik-kb patch (c), partly reverted by patch (c2) (ADR-0050): the selection <->
// cursor conversions are kept as they were; the presence part (`createCursorPlugin`,
// `createDecorations`, the presence store type) is back at the end of this file,
// without cursor/awareness.ts (Loro's deprecated `Awareness`; the store is
// cursor/ephemeral.ts).

import {
  type ContainerID,
  type Cursor,
  LoroList,
  LoroText,
} from "loro-crdt";
// atomik-kb patch (a): ProseMirror from @tiptap/pm, the instance Tiptap uses.
import type { Node } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import type { EditorState, Selection } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { DecorationAttrs, EditorView } from "@tiptap/pm/view";

import {
  CHILDREN_KEY,
  type LoroDocType,
  type LoroNode,
  type LoroNodeMapping,
  WEAK_NODE_TO_LORO_CONTAINER_MAPPING,
} from "../lib";
import { loroSyncPluginKey } from "../sync-plugin-key";
import type { LoroSyncPluginState } from "../sync-plugin-key";

export function convertPmSelectionToCursors(
  pmRootNode: Node,
  selection: Selection,
  loroState: LoroSyncPluginState,
) {
  const anchor = absolutePositionToCursor(
    pmRootNode,
    selection.anchor,
    loroState.doc as LoroDocType,
    loroState.mapping,
  );
  const focus =
    selection.head == selection.anchor
      ? anchor
      : absolutePositionToCursor(
          pmRootNode,
          selection.head,
          loroState.doc as LoroDocType,
          loroState.mapping,
        );
  return { anchor, focus };
}

function getByValue(map: Map<ContainerID, Node | Node[]>, searchValue: Node) {
  for (let [key, value] of map.entries()) {
    if (value === searchValue) return key;
  }
  // atomik-kb patch (t): explicit for noImplicitReturns.
  return undefined;
}

function absolutePositionToCursor(
  pmRootNode: Node,
  anchor: number,
  doc: LoroDocType,
  mapping: LoroNodeMapping,
): Cursor | undefined {
  const pos = pmRootNode.resolve(anchor);
  const nodeParent = pos.node(pos.depth);
  const offset = pos.parentOffset;

  const loroId =
    WEAK_NODE_TO_LORO_CONTAINER_MAPPING.get(nodeParent) ??
    getByValue(mapping, nodeParent);
  if (!loroId) {
    // atomik-kb patch (w): no `console.error("Cannot find the loroNode")` here.
    // The undo plugin asks for every transaction, including those appended
    // between a change and its write to Loro (UniqueID's ids after Enter or a
    // paste), when the new nodes are not in the mapping yet: expected, not an error.
    return;
  }

  // atomik-kb patch (t): typed cast instead of `any`.
  const loroMap = doc.getMap(loroId) as LoroNode;
  const children = loroMap.get(CHILDREN_KEY);
  if (children.length == 0) {
    // This is a new line, so we can use the list cursor instead
    return children.getCursor(0);
  }

  let index = offset;
  let childIndex = 0;
  while (index >= 0 && childIndex < children.length) {
    const child = children.get(childIndex);
    childIndex += 1;
    if (child instanceof LoroText) {
      return child.getCursor(index);
    } else {
      if (index == 0) {
        // This happens when user selects an image or a horizontal rule
        if (childIndex < children.length) {
          // Select next text node
          index += 1;
        }
      }

      index -= 1;
    }
  }

  // Selection is not on text
  return undefined;
}

export function cursorToAbsolutePosition(
  cursor: Cursor,
  doc: LoroDocType,
  mapping: LoroNodeMapping,
): [number, Cursor | undefined] {
  const containerId = cursor.containerId();
  let index = -1;
  let targetChildId: ContainerID;
  let loroNode: LoroNode | undefined;
  let update: Cursor | undefined;
  if (containerId.endsWith("List")) {
    const loroList = doc.getContainerById(containerId) as LoroList | undefined;
    if (!loroList) {
      return [1, undefined];
    }

    const parentNode = loroList.parent();
    if (!parentNode) {
      return [1, undefined];
    }

    targetChildId = parentNode.id;
    loroNode = parentNode.parent()?.parent() as LoroNode | undefined;
    index = 0;
  } else {
    const loroText = doc.getText(containerId);
    const pos = doc.getCursorPos(cursor);
    if (!pos) {
      return [1, undefined];
    }
    update = pos.update;
    index += pos.offset;
    targetChildId = loroText.id;
    loroNode = loroText.parent()?.parent() as LoroNode | undefined;
  }
  while (loroNode != null) {
    const children = loroNode.get(CHILDREN_KEY);
    if (children instanceof LoroList) {
      const childIds = children.toArray() as LoroNode[];
      for (const iter of childIds) {
        if (iter.id === targetChildId) {
          break;
        }

        const mapped = mapping.get(iter.id);
        if (Array.isArray(mapped)) {
          mapped.forEach((child) => {
            index += child.nodeSize;
          });
        } else {
          if (mapped != null) {
            index += mapped.nodeSize;
          } else {
            console.error(childIds, children.toJSON());
          }
        }
      }

      targetChildId = loroNode.id;
      loroNode = loroNode.parent()?.parent() as LoroNode | undefined;
      index += 1;
    } else {
      throw new Error("Unreachable code");
    }
  }

  return [index, update];
}

export function cursorEq(a?: Cursor | null, b?: Cursor | null) {
  if (!a && !b) {
    return true;
  }
  if (!a || !b) {
    return false;
  }

  const aPos = a.pos();
  const bPos = b.pos();
  return (
    aPos?.peer === bPos?.peer &&
    aPos?.counter === bPos?.counter &&
    a.containerId() === b.containerId()
  );
}

// ----- atomik-kb patch (c2), ADR-0050: remote carets and selections (upstream `createCursorPlugin`) ----------------

/**
 * Who a cursor is shown as. Upstream reads it from the sender's state; here the host fills it from membership (the name is
 * the member's profile name, the colour their place in join order), so a frame cannot pick a name or a colour of its own.
 */
export type PresenceUser = { name: string; color: string };

/** One device's cursor: Loro cursors of the anchor and the head of its selection. */
export type CursorState = { anchor?: Cursor; focus?: Cursor; user?: PresenceUser };

/** What the cursor plugin needs of a store of cursors (upstream's `PresenceStore`; `CursorEphemeralStore` is the one). */
export interface PresenceStore {
  /** The key of this device's own state in `getAll`: never drawn. */
  readonly localKey: string;
  /** Every device's state by key. */
  getAll(): Record<string, CursorState>;
  getLocal(): CursorState | undefined;
  /** Sets this device's cursor; no cursor at all (neither anchor nor focus) clears it. */
  setLocal(state: CursorState): void;
  /** Listens to changes; the plugin redraws for the ones other devices make (`import`, `timeout`). */
  subscribeBy(listener: (by: "local" | "import" | "timeout") => void): () => void;
}

export interface CursorPluginOptions {
  /** The caret of a remote device (upstream `createCursor`); `defaultCursorBuilder`. */
  createCursor?: (user: PresenceUser) => HTMLElement;
  /** The attributes of the range a remote device has selected (upstream `createSelection`); `defaultSelectionBuilder`. */
  createSelection?: (user: PresenceUser) => DecorationAttrs;
}

interface CursorPluginState {
  decorations: DecorationSet;
}

/** The caret of a remote device: a thin line in the member's colour with their name above it. */
export function defaultCursorBuilder(user: PresenceUser): HTMLElement {
  const caret = document.createElement("span");
  caret.className = "loro-cursor";
  caret.style.borderColor = user.color;
  caret.setAttribute("contenteditable", "false");
  caret.setAttribute("aria-hidden", "true");
  const label = document.createElement("span");
  label.className = "loro-cursor-label";
  label.style.backgroundColor = user.color;
  label.textContent = user.name;
  caret.append(label);
  return caret;
}

/** The range a remote device has selected: the member's colour, see through. */
export function defaultSelectionBuilder(user: PresenceUser): DecorationAttrs {
  return { class: "loro-cursor-selection", style: `background-color: ${user.color}40` };
}

/** The key of the cursor plugin's state, and of the transactions that ask it to redraw (`{ refresh: true }`). */
export const loroCursorPluginKey = new PluginKey<CursorPluginState>("loro-cursor");

const FALLBACK_USER: PresenceUser = { name: "?", color: "#6b7280" };

/**
 * Whether this replica can place a cursor: its container exists and is not deleted, and (for a text) the position is known. A
 * cursor of another device may be ahead of this replica (the text is not here yet) or behind it (the text is gone); resolving
 * one of those walks containers the view never mapped (an error log, and a caret at the start of the page).
 */
function isResolvable(doc: LoroDocType, cursor: Cursor): boolean {
  const container = doc.getContainerById(cursor.containerId());
  if (container == null || ("isDeleted" in container && container.isDeleted())) {
    return false;
  }
  if (cursor.containerId().endsWith("List")) {
    return true;
  }
  try {
    return doc.getCursorPos(cursor) != null;
  } catch {
    return false;
  }
}

/**
 * The carets and selections of the other devices as decorations. A device whose cursors cannot be resolved (its text is not in
 * this replica yet, or is gone) is not drawn; the others are.
 */
export function createDecorations(
  state: EditorState,
  states: Record<string, CursorState>,
  localKey: string,
  options: CursorPluginOptions = {},
): DecorationSet {
  const loro = loroSyncPluginKey.getState(state);
  if (loro == null || loro.doc == null) {
    return DecorationSet.empty;
  }
  const createCursor = options.createCursor ?? defaultCursorBuilder;
  const createSelection = options.createSelection ?? defaultSelectionBuilder;
  const size = state.doc.content.size;
  const clamp = (position: number) => Math.max(0, Math.min(position, size));
  const decorations: Decoration[] = [];
  for (const [key, cursors] of Object.entries(states)) {
    if (key === localKey || cursors.anchor == null) {
      continue;
    }
    try {
      const doc = loro.doc as LoroDocType;
      if (
        !isResolvable(doc, cursors.anchor) ||
        (cursors.focus != null && !isResolvable(doc, cursors.focus))
      ) {
        continue;
      }
      const anchor = clamp(
        cursorToAbsolutePosition(cursors.anchor, doc, loro.mapping)[0],
      );
      const head =
        cursors.focus == null
          ? anchor
          : clamp(cursorToAbsolutePosition(cursors.focus, doc, loro.mapping)[0]);
      const user = cursors.user ?? FALLBACK_USER;
      if (anchor !== head) {
        decorations.push(
          Decoration.inline(
            Math.min(anchor, head),
            Math.max(anchor, head),
            createSelection(user),
            { key: `${key}:selection` },
          ),
        );
      }
      decorations.push(
        Decoration.widget(head, () => createCursor(user), {
          // Redrawn when the name or the colour change, kept as it is otherwise.
          key: `${key}:${user.name}:${user.color}`,
          side: 10,
          ignoreSelection: true,
        }),
      );
    } catch {
      // A cursor of text this replica does not hold (yet): the device is not drawn until it does.
    }
  }
  return DecorationSet.create(state.doc, decorations);
}

/**
 * The cursor plugin (upstream `createCursorPlugin`): draws the other devices' carets and selections from `presence`, and writes
 * this view's selection to it as Loro cursors while the editor has focus (and clears it when it has not). It must come after
 * `LoroSyncPlugin`, whose state it reads.
 */
export function createCursorPlugin(
  presence: PresenceStore,
  options: CursorPluginOptions = {},
) {
  return new Plugin<CursorPluginState>({
    key: loroCursorPluginKey,
    state: {
      init: () => ({ decorations: DecorationSet.empty }),
      apply: (tr, previous, _oldState, newState) => {
        const loro = loroSyncPluginKey.getState(newState);
        if (loro == null || loro.doc == null) {
          return previous;
        }
        const refresh =
          (
            tr.getMeta(loroCursorPluginKey) as
              | { refresh?: boolean }
              | undefined
          )?.refresh === true;
        const sync = tr.getMeta(loroSyncPluginKey) as
          | { type?: string }
          | undefined;
        // A remote change, the first load and a change of the other devices'
        // cursors place the carets again; a local edit only moves the ones
        // that are there.
        if (
          refresh ||
          loro.changedBy !== "local" ||
          sync?.type === "update-state"
        ) {
          return {
            decorations: createDecorations(
              newState,
              presence.getAll(),
              presence.localKey,
              options,
            ),
          };
        }
        return tr.docChanged
          ? { decorations: previous.decorations.map(tr.mapping, tr.doc) }
          : previous;
      },
    },
    props: {
      decorations: (state) =>
        loroCursorPluginKey.getState(state)?.decorations ??
        DecorationSet.empty,
    },
    view: (view: EditorView) => {
      let anchor: Cursor | undefined;
      let focus: Cursor | undefined;
      const clear = () => {
        if (
          anchor !== undefined ||
          focus !== undefined ||
          presence.getLocal() !== undefined
        ) {
          anchor = undefined;
          focus = undefined;
          presence.setLocal({});
        }
      };
      // This view's selection as Loro cursors, while the editor has focus and
      // the selection is in text.
      const updateLocal = () => {
        if (view.isDestroyed) {
          return;
        }
        const loro = loroSyncPluginKey.getState(view.state);
        if (!view.hasFocus() || loro == null || loro.doc == null) {
          clear();
          return;
        }
        const cursors = convertPmSelectionToCursors(
          view.state.doc,
          view.state.selection,
          loro,
        );
        if (cursors.anchor === undefined) {
          clear();
          return;
        }
        if (cursorEq(cursors.anchor, anchor) && cursorEq(cursors.focus, focus)) {
          return;
        }
        anchor = cursors.anchor;
        focus = cursors.focus;
        presence.setLocal({
          anchor,
          ...(focus === undefined ? {} : { focus }),
        });
      };
      const stop = presence.subscribeBy((by) => {
        if (by !== "local" && !view.isDestroyed) {
          view.dispatch(
            view.state.tr
              .setMeta(loroCursorPluginKey, { refresh: true })
              .setMeta("addToHistory", false),
          );
        }
      });
      const onFocusOut = () => {
        clear();
      };
      view.dom.addEventListener("focusin", updateLocal);
      view.dom.addEventListener("focusout", onFocusOut);
      return {
        update: updateLocal,
        destroy: () => {
          stop();
          view.dom.removeEventListener("focusin", updateLocal);
          view.dom.removeEventListener("focusout", onFocusOut);
          clear();
        },
      };
    },
  });
}
