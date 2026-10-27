// The sync between the editor's scene and the page document (ADR-0033): which elements are written, when,
// and how what comes from the document is merged into the scene.

import type { WhiteboardValue } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { isNewer, SceneSync, stampOf, WRITE_DELAY_MS } from "./whiteboard-sync";
import type { WhiteboardStore } from "./whiteboard-store";

const BLOCK = "0b0b0b0b-0b0b-4b0b-8b0b-0b0b0b0b0b0b";

function el(id: string, version: number, extra: WhiteboardValue = {}): WhiteboardValue {
  return { id, type: "rectangle", index: `a${id}`, version, versionNonce: 100, isDeleted: false, ...extra };
}

/** A store over a map, recording the writes. */
function fakeStore(initial: WhiteboardValue[] = []) {
  const elements = new Map(initial.map((e) => [e.id as string, e]));
  const writes: WhiteboardValue[][] = [];
  const store: WhiteboardStore = {
    read: () => [...elements.values()].sort((a, b) => (a.index as string).localeCompare(b.index as string)),
    write: (_block, written) => {
      writes.push([...written]);
      for (const e of written) {
        elements.set(e.id as string, e);
      }
    },
    subscribe: () => () => undefined,
    copy: () => undefined,
  };
  return { store, writes, elements };
}

/** A timer the test advances by hand. */
function manualTimers() {
  let next = 1;
  const pending = new Map<number, () => void>();
  return {
    setTimer: (callback: () => void) => {
      pending.set(next, callback);
      return next++;
    },
    clearTimer: (handle: number) => {
      pending.delete(handle);
    },
    fire: () => {
      const callbacks = [...pending.values()];
      pending.clear();
      callbacks.forEach((callback) => {
        callback();
      });
    },
    get count() {
      return pending.size;
    },
  };
}

describe("the reconciliation rule", () => {
  it("prefers the higher version, then the lower nonce (Excalidraw's)", () => {
    expect(isNewer(el("a", 3), el("a", 2))).toBe(true);
    expect(isNewer(el("a", 2), el("a", 3))).toBe(false);
    expect(isNewer(el("a", 2, { versionNonce: 5 }), el("a", 2, { versionNonce: 9 }))).toBe(true);
    expect(isNewer(el("a", 2), el("a", 2))).toBe(false);
    expect(isNewer({ id: "a" }, { id: "a" })).toBe(false);
  });

  it("stamps an element by version and nonce", () => {
    expect(stampOf(el("a", 4, { versionNonce: 8 }))).toBe("4:8");
  });
});

describe("local changes", () => {
  it("write only the elements whose version changed, in one commit, after a pause", () => {
    const { store, writes } = fakeStore([el("a", 1), el("b", 1)]);
    const timers = manualTimers();
    const sync = new SceneSync(store, BLOCK, timers);
    // The scene reports every element on every change; one is new, one changed, one is as it was.
    sync.sceneChanged([el("a", 1), el("b", 2), el("c", 1)]);
    sync.sceneChanged([el("a", 1), el("b", 2), el("c", 1)]);
    expect(writes).toEqual([]);
    expect(timers.count).toBe(1);
    timers.fire();
    expect(writes).toHaveLength(1);
    expect(writes[0]?.map((e) => e.id)).toEqual(["b", "c"]);
    // Nothing is written twice.
    sync.sceneChanged([el("a", 1), el("b", 2), el("c", 1)]);
    expect(timers.count).toBe(0);
    sync.flush();
    expect(writes).toHaveLength(1);
  });

  it("write a tombstone like any change", () => {
    const { store, writes } = fakeStore([el("a", 1)]);
    const sync = new SceneSync(store, BLOCK, manualTimers());
    sync.sceneChanged([el("a", 2, { isDeleted: true })]);
    sync.flush();
    expect(writes[0]).toEqual([el("a", 2, { isDeleted: true })]);
  });

  it("are written at once by flush, and the timer is gone", () => {
    const { store, writes } = fakeStore();
    const timers = manualTimers();
    const sync = new SceneSync(store, BLOCK, timers);
    sync.sceneChanged([el("a", 1)]);
    expect(sync.pending).toBe(1);
    sync.flush();
    expect(writes).toHaveLength(1);
    expect(timers.count).toBe(0);
    expect(sync.pending).toBe(0);
  });

  it("keep the latest state of an element changed again before the write", () => {
    const { store, writes } = fakeStore();
    const sync = new SceneSync(store, BLOCK, manualTimers());
    sync.sceneChanged([el("a", 1)]);
    sync.sceneChanged([el("a", 2, { x: 10 })]);
    sync.flush();
    expect(writes).toHaveLength(1);
    expect(writes[0]).toEqual([el("a", 2, { x: 10 })]);
  });

  it("use the real timers with the default delay", () => {
    vi.useFakeTimers();
    try {
      const { store, writes } = fakeStore();
      const sync = new SceneSync(store, BLOCK);
      sync.sceneChanged([el("a", 1)]);
      vi.advanceTimersByTime(WRITE_DELAY_MS - 1);
      expect(writes).toHaveLength(0);
      vi.advanceTimersByTime(1);
      expect(writes).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("are dropped by dispose (flush first when they matter)", () => {
    const { store, writes } = fakeStore();
    const sync = new SceneSync(store, BLOCK, manualTimers());
    sync.sceneChanged([el("a", 1)]);
    sync.dispose();
    sync.flush();
    expect(writes).toEqual([]);
  });

  it("ignore an element without an id", () => {
    const { store, writes } = fakeStore();
    const sync = new SceneSync(store, BLOCK, manualTimers());
    sync.sceneChanged([{ type: "rectangle", version: 1 }]);
    sync.flush();
    expect(writes).toEqual([]);
  });
});

describe("what comes from the document", () => {
  it("replaces the scene's older elements, adds the new ones and is no change to write back", () => {
    const { store, writes, elements } = fakeStore([el("a", 1), el("b", 1)]);
    const sync = new SceneSync(store, BLOCK, manualTimers());
    const scene = [el("a", 1), el("b", 1)];
    elements.set("b", el("b", 3, { x: 7 }));
    elements.set("c", el("c", 1));
    const { elements: merged, changed } = sync.incoming(scene);
    expect(changed).toBe(true);
    expect(merged.map((e) => [e.id, e.version])).toEqual([
      ["a", 1],
      ["b", 3],
      ["c", 1],
    ]);
    // The scene reports what it now holds: nothing to write.
    sync.sceneChanged(merged);
    sync.flush();
    expect(writes).toEqual([]);
  });

  it("is no change when the document and the scene agree (our own writes coming back)", () => {
    const { store } = fakeStore([el("a", 2)]);
    const sync = new SceneSync(store, BLOCK, manualTimers());
    expect(sync.incoming([el("a", 2)]).changed).toBe(false);
  });

  it("deletes an element the scene still shows when the document holds a newer tombstone", () => {
    const { store } = fakeStore([el("a", 4, { isDeleted: true })]);
    const sync = new SceneSync(store, BLOCK, manualTimers());
    const { elements, changed } = sync.incoming([el("a", 3)]);
    expect(changed).toBe(true);
    expect(elements[0]?.isDeleted).toBe(true);
  });

  it("writes back a newer element of the scene that the document lost to an older write (a delete that lost)", () => {
    const { store, writes, elements } = fakeStore([el("a", 1)]);
    const timers = manualTimers();
    const sync = new SceneSync(store, BLOCK, timers);
    // This device deleted the element (version 5); the document, after a merge, holds another device's older edit.
    const tombstone = el("a", 5, { isDeleted: true });
    sync.sceneChanged([tombstone]);
    sync.flush();
    expect(writes).toHaveLength(1);
    elements.set("a", el("a", 4)); // the merge kept the other write
    const { elements: merged, changed } = sync.incoming([tombstone]);
    expect(changed).toBe(false);
    expect(merged[0]?.isDeleted).toBe(true);
    expect(sync.pending).toBe(1);
    timers.fire();
    expect(writes).toHaveLength(2);
    expect(writes[1]).toEqual([tombstone]);
  });

  it("keeps the scene's own elements that the document does not hold yet", () => {
    const { store } = fakeStore([el("a", 1)]);
    const sync = new SceneSync(store, BLOCK, manualTimers());
    const mine = el("z", 1);
    const { elements } = sync.incoming([el("a", 1), mine]);
    expect(elements.map((e) => e.id)).toEqual(["a", "z"]);
  });

  it("orders the merged elements by their fractional index when they all have one", () => {
    const { store, elements } = fakeStore([el("a", 1, { index: "a1" })]);
    const sync = new SceneSync(store, BLOCK, manualTimers());
    elements.set("b", el("b", 1, { index: "a0" }));
    const { elements: merged } = sync.incoming([el("a", 1, { index: "a1" })]);
    expect(merged.map((e) => e.id)).toEqual(["b", "a"]);
  });

  it("starts from the document", () => {
    const { store } = fakeStore([el("a", 1), el("b", 1)]);
    expect(new SceneSync(store, BLOCK, manualTimers()).initial().map((e) => e.id)).toEqual(["a", "b"]);
  });
});
