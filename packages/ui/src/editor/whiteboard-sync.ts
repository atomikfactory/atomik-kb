// The two-way sync between the Excalidraw scene of the full-screen editor and the elements of a
// drawing in the page document (ADR-0033, "Sync both ways"). Pure logic over plain element values, so
// it is tested without Excalidraw.
//
// Local to document: `sceneChanged(elements)` keeps the elements whose version differs from the one
// last seen in the document (or last written), and `flush` writes only those, in one commit, after a
// short pause (the scene reports every frame of a drag).
//
// Document to scene: `incoming()` compares the document with the scene by Excalidraw's rule (the higher
// `version` wins, a tie goes to the lower `versionNonce`): the elements that are newer in the document
// go to the scene; the ones that are newer in the scene (a delete that lost the race against an older
// edit) are written back, so every device converges on the newer element.

import type { WhiteboardValue } from "@atomik-kb/core-client";

import type { WhiteboardStore } from "./whiteboard-store";

/** How long the scene must be quiet before its changes are written. */
export const WRITE_DELAY_MS = 400;

/** A string field of an element (`""` when it is not one). */
export function textField(element: WhiteboardValue, field: string): string {
  const value = element[field];
  return typeof value === "string" ? value : "";
}

/** The stamp of an element's state: `version` and `versionNonce`. */
export function stampOf(element: WhiteboardValue): string {
  return `${String(numberOf(element.version))}:${String(numberOf(element.versionNonce))}`;
}

/**
 * Whether `a` is newer than `b` by Excalidraw's reconciliation rule: a higher `version`, or the same
 * version and a lower `versionNonce`. Elements without a numeric version count as version 0.
 */
export function isNewer(a: WhiteboardValue, b: WhiteboardValue): boolean {
  const [va, vb] = [numberOf(a.version), numberOf(b.version)];
  if (va !== vb) {
    return va > vb;
  }
  return numberOf(a.versionNonce) < numberOf(b.versionNonce);
}

function numberOf(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

/** An element of a scene or a document, as far as the sync cares. */
export type SceneElement = WhiteboardValue;

export interface IncomingChange {
  /** Every element the scene should hold, in z-order, after taking the document's newer ones. */
  elements: SceneElement[];
  /** Whether any element of the document is newer than the scene's. */
  changed: boolean;
}

export interface SyncOptions {
  /** Defaults to {@link WRITE_DELAY_MS}. */
  delayMs?: number;
  setTimer?: (callback: () => void, ms: number) => number;
  clearTimer?: (handle: number) => void;
}

/** One editing session of one drawing. */
export class SceneSync {
  readonly #store: WhiteboardStore;
  readonly #blockId: string;
  readonly #delay: number;
  readonly #setTimer: (callback: () => void, ms: number) => number;
  readonly #clearTimer: (handle: number) => void;
  /** The stamp of each element as last seen in the document, or last written. */
  readonly #known = new Map<string, string>();
  #pending = new Map<string, SceneElement>();
  #timer: number | null = null;

  constructor(store: WhiteboardStore, blockId: string, options: SyncOptions = {}) {
    this.#store = store;
    this.#blockId = blockId;
    this.#delay = options.delayMs ?? WRITE_DELAY_MS;
    this.#setTimer = options.setTimer ?? ((callback, ms) => window.setTimeout(callback, ms));
    this.#clearTimer = options.clearTimer ?? ((handle) => {
      window.clearTimeout(handle);
    });
    for (const element of store.read(blockId)) {
      this.#known.set(textField(element, "id"), stampOf(element));
    }
  }

  /** The elements of the document now, in z-order. */
  initial(): SceneElement[] {
    return this.#store.read(this.#blockId);
  }

  /** The scene reported its elements (all of them, deleted ones included). Schedules a write of the changed ones. */
  sceneChanged(elements: readonly SceneElement[]): void {
    let any = false;
    for (const element of elements) {
      const id = textField(element, "id");
      if (id === "") {
        continue;
      }
      const stamp = stampOf(element);
      if (this.#known.get(id) !== stamp) {
        this.#known.set(id, stamp);
        this.#pending.set(id, element);
        any = true;
      }
    }
    if (any && this.#timer === null) {
      this.#timer = this.#setTimer(() => {
        this.#timer = null;
        this.flush();
      }, this.#delay);
    }
  }

  /** Writes the pending elements now (the editor closes, the app locks, a gesture ends). */
  flush(): void {
    if (this.#timer !== null) {
      this.#clearTimer(this.#timer);
      this.#timer = null;
    }
    if (this.#pending.size === 0) {
      return;
    }
    const elements = [...this.#pending.values()];
    this.#pending = new Map();
    this.#store.write(this.#blockId, elements);
  }

  /** Elements waiting to be written. */
  get pending(): number {
    return this.#pending.size;
  }

  /**
   * The document changed (a sync import, another view of the same block). Merges it with the `scene`
   * the editor holds: what is newer in the document replaces the scene's element, what is newer in
   * the scene is queued to be written back. Elements only the scene has stay (they are pending or
   * will be written), elements only the document has are added.
   */
  incoming(scene: readonly SceneElement[]): IncomingChange {
    const stored = this.#store.read(this.#blockId);
    const mine = new Map(scene.map((element) => [textField(element, "id"), element]));
    const merged = new Map<string, SceneElement>(mine);
    let changed = false;
    for (const element of stored) {
      const id = textField(element, "id");
      const local = mine.get(id);
      if (local === undefined || isNewer(element, local)) {
        merged.set(id, element);
        this.#known.set(id, stampOf(element));
        this.#pending.delete(id);
        changed = true;
      } else if (isNewer(local, element)) {
        // The document holds an older state than this scene (a concurrent delete lost to an older
        // edit, say): write the scene's element back so every device converges on it.
        this.#known.set(id, stampOf(local));
        this.#pending.set(id, local);
      } else {
        this.#known.set(id, stampOf(element));
      }
    }
    if (this.#pending.size > 0 && this.#timer === null) {
      this.#timer = this.#setTimer(() => {
        this.#timer = null;
        this.flush();
      }, this.#delay);
    }
    return { elements: orderLike(stored, scene, merged), changed };
  }

  /** Stops the timer (the pending elements are dropped: `flush` first when they matter). */
  dispose(): void {
    if (this.#timer !== null) {
      this.#clearTimer(this.#timer);
      this.#timer = null;
    }
    this.#pending = new Map();
  }
}

/**
 * The merged elements in an order: the scene's order for the elements it had, then the document's new
 * ones in the document's (z) order. The editor sorts by `index` itself when it takes the update.
 */
function orderLike(
  stored: readonly SceneElement[],
  scene: readonly SceneElement[],
  merged: ReadonlyMap<string, SceneElement>,
): SceneElement[] {
  const out: SceneElement[] = [];
  const seen = new Set<string>();
  for (const element of scene) {
    const id = textField(element, "id");
    const next = merged.get(id);
    if (next !== undefined && !seen.has(id)) {
      out.push(next);
      seen.add(id);
    }
  }
  for (const element of stored) {
    const id = textField(element, "id");
    const next = merged.get(id);
    if (next !== undefined && !seen.has(id)) {
      out.push(next);
      seen.add(id);
    }
  }
  // With a fractional index on every element the z-order is theirs (the same order every device computes).
  if (out.every((element) => typeof element.index === "string")) {
    out.sort((a, b) => {
      const [x, y] = [textField(a, "index"), textField(b, "index")];
      return x < y ? -1 : x > y ? 1 : textField(a, "id") < textField(b, "id") ? -1 : 1;
    });
  }
  return out;
}
