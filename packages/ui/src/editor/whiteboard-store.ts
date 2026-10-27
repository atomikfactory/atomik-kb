// The page editor's access to the elements of whiteboards (ADR-0033): the root map `whiteboards` of
// the page's Loro document (core-client, `whiteboard.ts`), read, written and watched per block.
//
// - `read(blockId)`: the elements of one drawing, tombstones included, in z-order.
// - `write(blockId, elements)`: stores the given elements in one commit with the origin
//   `whiteboard`, which the page's undo manager excludes (page undo never rewinds a drawing).
// - `subscribe(blockId, listener)`: called after the elements of that block changed, whoever
//   changed them (a sync import, or a write from another view of the same block).
// - `copy(from, to, source?)`: gives the block `to` a copy of the elements of `from` (or of
//   `source`, elements taken from another page's editor): what duplicating a block does.

import { orphanWhiteboardKeys, readWhiteboard, splitWhiteboardKey, WHITEBOARDS, writeWhiteboardElements } from "@atomik-kb/core-client";
import type { WhiteboardValue } from "@atomik-kb/core-client";
import type { LoroDoc } from "loro-crdt";

export interface WhiteboardStore {
  read: (blockId: string) => WhiteboardValue[];
  write: (blockId: string, elements: readonly WhiteboardValue[]) => void;
  subscribe: (blockId: string, listener: () => void) => () => void;
  copy: (from: string, to: string, source?: readonly WhiteboardValue[]) => void;
}

/** A plain JSON copy of an element: what Loro stores (no `undefined`, no shared references). */
export function plainElement(element: unknown): WhiteboardValue {
  return JSON.parse(JSON.stringify(element)) as WhiteboardValue;
}

/** The store over the editor's replica of a page. */
export function createWhiteboardStore(doc: LoroDoc): WhiteboardStore {
  const listeners = new Map<string, Set<() => void>>();

  // One subscription for the store, created with the first listener and dropped with the last.
  let unsubscribe: (() => void) | null = null;
  const attach = (): void => {
    if (unsubscribe !== null) {
      return;
    }
    unsubscribe = doc.subscribe((batch) => {
      const touched = new Set<string>();
      for (const event of batch.events) {
        if (event.path[0] !== WHITEBOARDS || event.diff.type !== "map") {
          continue;
        }
        for (const key of Object.keys(event.diff.updated)) {
          const split = splitWhiteboardKey(key);
          if (split !== undefined) {
            touched.add(split.blockId);
          }
        }
      }
      for (const blockId of touched) {
        for (const listener of [...(listeners.get(blockId) ?? [])]) {
          listener();
        }
      }
    });
  };
  const detach = (): void => {
    if (listeners.size === 0 && unsubscribe !== null) {
      unsubscribe();
      unsubscribe = null;
    }
  };

  return {
    read: (blockId) => readWhiteboard(doc, blockId).map((element) => element.value),
    write: (blockId, elements) => {
      writeWhiteboardElements(doc, blockId, elements.map(plainElement));
    },
    subscribe: (blockId, listener) => {
      const set = listeners.get(blockId) ?? new Set<() => void>();
      set.add(listener);
      listeners.set(blockId, set);
      attach();
      return () => {
        set.delete(listener);
        if (set.size === 0) {
          listeners.delete(blockId);
        }
        detach();
      };
    },
    copy: (from, to, source) => {
      const elements = source ?? readWhiteboard(doc, from).map((element) => element.value);
      if (elements.length > 0 && from !== to) {
        writeWhiteboardElements(
          doc,
          to,
          elements.map((element) => plainElement(element)),
        );
      }
    },
  };
}

/** The keys of the elements whose block has no node in `liveBlocks` (what a later pruning would remove). */
export function orphanKeys(doc: LoroDoc, liveBlocks: ReadonlySet<string>): string[] {
  return orphanWhiteboardKeys(doc, liveBlocks);
}
