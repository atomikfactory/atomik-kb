// The editor's store over the page document's `whiteboards` map (ADR-0033).

import { portableToLoro, WHITEBOARDS, WHITEBOARD_ORIGIN } from "@atomik-kb/core-client";
import type { WhiteboardValue } from "@atomik-kb/core-client";
import { LoroDoc, UndoManager } from "loro-crdt";
import { describe, expect, it, vi } from "vitest";

import { createWhiteboardStore, plainElement } from "./whiteboard-store";

const BOARD = "0b0b0b0b-0b0b-4b0b-8b0b-0b0b0b0b0b0b";
const OTHER = "0c0c0c0c-0c0c-4c0c-8c0c-0c0c0c0c0c0c";

function page(): LoroDoc {
  const doc = new LoroDoc();
  portableToLoro(doc, {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: "0a0a0a0a-0a0a-4a0a-8a0a-0a0a0a0a0a0a" } },
      { type: "whiteboard", attrs: { id: BOARD, height: 360 } },
    ],
  });
  return doc;
}

const el = (id: string, version = 1, extra: WhiteboardValue = {}): WhiteboardValue => ({
  id,
  type: "rectangle",
  index: `a${id}`,
  version,
  versionNonce: 5,
  isDeleted: false,
  ...extra,
});

describe("the whiteboard store", () => {
  it("writes elements in one commit and reads them back in z-order", () => {
    const doc = page();
    const store = createWhiteboardStore(doc);
    store.write(BOARD, [el("b"), el("a")]);
    expect(store.read(BOARD).map((e) => e.id)).toEqual(["a", "b"]);
    expect(store.read(OTHER)).toEqual([]);
  });

  it("stores plain JSON: undefined fields are dropped, nothing is shared with the scene", () => {
    const doc = page();
    const store = createWhiteboardStore(doc);
    const scene = { ...el("a"), customData: undefined, points: [[0, 0]] };
    store.write(BOARD, [scene as unknown as WhiteboardValue]);
    const stored = store.read(BOARD)[0];
    expect(stored).toEqual({ ...el("a"), points: [[0, 0]] });
    expect(plainElement(scene)).not.toBe(scene);
  });

  it("tells the listeners of a block when its elements change, whoever changed them", async () => {
    const doc = page();
    const store = createWhiteboardStore(doc);
    const here = vi.fn();
    const elsewhere = vi.fn();
    const stopHere = store.subscribe(BOARD, here);
    store.subscribe(OTHER, elsewhere);
    store.write(BOARD, [el("a")]);
    await vi.waitFor(() => {
      expect(here).toHaveBeenCalledTimes(1);
    });
    // A change that arrives from another replica.
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    createWhiteboardStore(remote).write(BOARD, [el("a", 2)]);
    doc.import(remote.export({ mode: "update", from: doc.version() }));
    await vi.waitFor(() => {
      expect(here).toHaveBeenCalledTimes(2);
    });
    expect(elsewhere).not.toHaveBeenCalled();
    // A change elsewhere in the page is no news to it.
    doc.getMap("doc").set("note", "x");
    doc.commit();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(here).toHaveBeenCalledTimes(2);
    stopHere();
    store.write(BOARD, [el("a", 3)]);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(here).toHaveBeenCalledTimes(2);
  });

  it("copies the elements of a block to another, tombstones included, or those it is given", () => {
    const doc = page();
    const store = createWhiteboardStore(doc);
    store.write(BOARD, [el("a"), el("b", 3, { isDeleted: true })]);
    store.copy(BOARD, OTHER);
    expect(store.read(OTHER).map((e) => [e.id, e.isDeleted])).toEqual([
      ["a", false],
      ["b", true],
    ]);
    store.copy("0d0d0d0d-0d0d-4d0d-8d0d-0d0d0d0d0d0d", OTHER, [el("z")]);
    expect(store.read(OTHER).map((e) => e.id)).toEqual(["a", "b", "z"]);
    // Nothing to copy, nothing written.
    const before = doc.version();
    store.copy(OTHER, "0e0e0e0e-0e0e-4e0e-8e0e-0e0e0e0e0e0e", []);
    expect(doc.version().compare(before)).toBe(0);
  });

  it("commits with the origin the page's undo manager skips", () => {
    const doc = page();
    const undo = new UndoManager(doc, { mergeInterval: 0, excludeOriginPrefixes: [WHITEBOARD_ORIGIN] });
    const store = createWhiteboardStore(doc);
    doc.getMap("doc").set("typed", "x");
    doc.commit();
    store.write(BOARD, [el("a")]);
    expect(undo.canUndo()).toBe(true);
    undo.undo();
    expect(doc.getMap("doc").get("typed")).toBeUndefined();
    // The drawing is where it was; there is nothing left to undo.
    expect(store.read(BOARD)).toHaveLength(1);
    expect(Object.keys(doc.getMap(WHITEBOARDS).toJSON() as object)).toHaveLength(1);
    expect(undo.canUndo()).toBe(false);
  });
});
