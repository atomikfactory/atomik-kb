// What happens to the drawing of a whiteboard block that is duplicated, copied and pasted, or dragged
// (ADR-0033, "Lifecycle"). The elements are not in the node (they are plain values of the page
// document's root map `whiteboards`, keyed by block id), so a copy of the node alone would be an empty
// drawing. A copy gets a new block id and a copy of the elements of its source in the same local
// commit that follows the paste:
//
// - `duplicateBlock` (blocks.ts) gives the copy a fresh id through `withoutIds`, which calls
//   `registerWhiteboardCopy(new id, source id)`;
// - a paste or a drop goes through `transformPasted`, which does the same for every whiteboard of the
//   slice;
// - once the node is in the document (the plugin view's `update`), the elements of the source are
//   copied to the new id: from this page's document when the source is still there (it is, unless it
//   was cut and the page changed), else from a small in-memory stash filled when the selection was
//   copied or cut (that is what carries a drawing from one page to another). The stash holds at most
//   a few drawings, is never persisted, and is emptied when the app locks.

import { Extension } from "@tiptap/core";
import { Fragment, Slice } from "@tiptap/pm/model";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import type { WhiteboardValue } from "@atomik-kb/core-client";

import type { WhiteboardStore } from "./whiteboard-store";
import { WHITEBOARD_NODE } from "./whiteboard-nodes";

/** The most drawings the stash keeps (the oldest goes first). */
const MAX_STASHED = 8;
/** The most copies waiting for their node (a slice that never lands leaves one behind). */
const MAX_PENDING = 64;

/** New block id -> the block id whose elements it gets. */
const pending = new Map<string, string>();
/** Source block id -> its elements as they were when the selection was copied. */
const stash = new Map<string, WhiteboardValue[]>();

/** The block `to` is a copy of the block `from`: it gets the elements of `from` once it is in a page. */
export function registerWhiteboardCopy(to: string, from: string): void {
  pending.set(to, from);
  while (pending.size > MAX_PENDING) {
    const oldest = pending.keys().next();
    if (oldest.done === true) {
      break;
    }
    pending.delete(oldest.value);
  }
}

/** Empties the stash (the app locked): no drawing stays in memory after the editors are gone. */
export function clearWhiteboardStash(): void {
  stash.clear();
  pending.clear();
}

/** Drawings in the stash and copies waiting for their node (tests). */
export function whiteboardCopyState(): { stashed: number; pending: number } {
  return { stashed: stash.size, pending: pending.size };
}

function remap(fragment: Fragment): Fragment {
  const nodes: PmNode[] = [];
  fragment.forEach((node) => {
    if (node.isText) {
      nodes.push(node);
      return;
    }
    let attrs = node.attrs;
    const from: unknown = attrs.id;
    if (node.type.name === WHITEBOARD_NODE && typeof from === "string" && from !== "") {
      const to = crypto.randomUUID();
      registerWhiteboardCopy(to, from);
      attrs = { ...attrs, id: to };
    }
    nodes.push(node.type.create(attrs, remap(node.content), node.marks));
  });
  return Fragment.fromArray(nodes);
}

/** Gives every whiteboard of `slice` a new block id and queues the copy of its elements. */
export function withNewWhiteboardIds(slice: Slice): Slice {
  return new Slice(remap(slice.content), slice.openStart, slice.openEnd);
}

/** Remembers the drawings of the selection, to carry them to another page. */
function stashSelection(view: EditorView, store: WhiteboardStore): void {
  view.state.selection.content().content.descendants((node) => {
    const id: unknown = node.attrs.id;
    if (node.type.name === WHITEBOARD_NODE && typeof id === "string" && id !== "") {
      const elements = store.read(id);
      if (elements.length > 0) {
        stash.delete(id);
        stash.set(id, elements);
        while (stash.size > MAX_STASHED) {
          const oldest = stash.keys().next();
          if (oldest.done === true) {
            break;
          }
          stash.delete(oldest.value);
        }
      }
    }
    return true;
  });
}

/** Copies the elements for the copies whose node is in the document now. */
function settle(view: EditorView, store: WhiteboardStore): void {
  view.state.doc.descendants((node) => {
    const id: unknown = node.attrs.id;
    if (node.type.name === WHITEBOARD_NODE && typeof id === "string") {
      const from = pending.get(id);
      if (from !== undefined) {
        pending.delete(id);
        const here = store.read(from);
        store.copy(from, id, here.length > 0 ? here : stash.get(from));
      }
      return false;
    }
    return true;
  });
}

export interface WhiteboardCopyOptions {
  /** The page's elements; `null` adds nothing (schema-only editors). */
  store: WhiteboardStore | null;
}

/** Copies the drawing with its block (see the module comment). */
export const WhiteboardCopy = Extension.create<WhiteboardCopyOptions>({
  name: "whiteboardCopy",

  addOptions() {
    return { store: null };
  },

  addProseMirrorPlugins() {
    const { store } = this.options;
    if (store === null) {
      return [];
    }
    return [
      new Plugin({
        key: new PluginKey("whiteboardCopy"),
        props: {
          transformPasted: (slice) => withNewWhiteboardIds(slice),
          handleDOMEvents: {
            copy: (view) => {
              stashSelection(view, store);
              return false;
            },
            cut: (view) => {
              stashSelection(view, store);
              return false;
            },
          },
        },
        view: () => ({
          update: (view) => {
            if (pending.size > 0) {
              settle(view, store);
            }
          },
        }),
      }),
    ];
  },
});
