// The re-vendored cursor plugin and its store (`loro-binding/cursor`, VENDORED.md patch c2; ADR-0050): the carets and selections
// of other devices as decorations in the member's colour with their name, this view's own selection written to the store while
// it has focus, the update of a frame (what is kept of it, what is dropped), and the 30-second timeout. The fuzz run with the
// cursor plugin is `binding-fuzz.test.ts`.

import { loroToPortable } from "@atomik-kb/core-client";
import { TextSelection } from "@tiptap/pm/state";
import { Cursor, EphemeralStore, LoroDoc, LoroList, LoroMap, LoroText } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { bindView, loroDocFrom, paragraphs, sync } from "../test/editor";
import type { BoundView } from "../test/editor";
import {
  CursorEphemeralStore,
  decodeCursorUpdate,
  defaultCursorBuilder,
  loroCursorPluginKey,
} from "./loro-binding";
import { createEditorSchema } from "./schema";

const schema = createEditorSchema();
const BLUE = "#1d4ed8";
const GREEN = "#15803d";
const bob = { name: "Bob", color: BLUE };

const handles: BoundView[] = [];
const stores: CursorEphemeralStore[] = [];

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
});

afterEach(() => {
  for (const handle of handles.splice(0)) {
    handle.destroy();
  }
  for (const store of stores.splice(0)) {
    store.destroy();
  }
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function newStore(doc: LoroDoc): CursorEphemeralStore {
  const store = new CursorEphemeralStore(doc.peerIdStr);
  stores.push(store);
  return store;
}

async function open(doc: LoroDoc, options: { attach?: boolean } = {}) {
  const store = newStore(doc);
  const handle = bindView(schema, doc, { cursors: store, ...options });
  handles.push(handle);
  await vi.advanceTimersByTimeAsync(0); // the binding initialises its view in a timeout
  return { view: handle.view, store };
}

/** The first text run of a page document. */
function firstText(doc: LoroDoc): LoroText {
  const visit = (map: LoroMap): LoroText | undefined => {
    for (const child of (map.get("children") as LoroList).toArray()) {
      if (child instanceof LoroText) {
        return child;
      }
      if (child instanceof LoroMap) {
        const found = visit(child as LoroMap);
        if (found !== undefined) {
          return found;
        }
      }
    }
    return undefined;
  };
  const found = visit(doc.getMap("doc"));
  if (found === undefined) {
    throw new Error("the document has no text");
  }
  return found;
}

/** What a device sends: an `EphemeralStore` update holding Loro cursors, under a key of its own. */
function frame(state: Record<string, unknown>, key = "peer"): Uint8Array {
  const store = new EphemeralStore(30_000);
  try {
    store.set(key, state as never);
    return store.encode(key);
  } finally {
    store.destroy();
  }
}

/** The update of a device whose selection is `[from, to]` in the first text of `doc` (a caret when equal). */
function selectionUpdate(doc: LoroDoc, from: number, to: number): Uint8Array {
  const text = firstText(doc);
  return frame({ anchor: text.getCursor(from)?.encode(), focus: text.getCursor(to)?.encode() });
}

/** The decorations the plugin holds, as `[from, to, kind]`. */
function decorations(view: BoundView["view"]): [number, number, string][] {
  const found = loroCursorPluginKey.getState(view.state)?.decorations.find() ?? [];
  return found
    .map((decoration): [number, number, string] => [decoration.from, decoration.to, decoration.from === decoration.to ? "caret" : "selection"])
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
}

describe("remote carets and selections", () => {
  it("draws a selection as a coloured range and a caret with the member's name at its end", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 6, 11));
    // "Hello world" starts at position 1 of the page; "world" is 7 to 12.
    expect(decorations(view)).toEqual([
      [7, 12, "selection"],
      [12, 12, "caret"],
    ]);
    const label = view.dom.querySelector<HTMLElement>(".loro-cursor .loro-cursor-label");
    expect(label?.textContent).toBe("Bob");
    expect(label?.style.backgroundColor).toBe("rgb(29, 78, 216)");
    expect(view.dom.querySelector<HTMLElement>(".loro-cursor")?.style.borderColor).toBe("rgb(29, 78, 216)");
    const selected = view.dom.querySelector<HTMLElement>(".loro-cursor-selection");
    expect(selected?.textContent).toBe("world");
    // The selection is the colour, see through.
    expect(selected?.style.backgroundColor).toBe("rgba(29, 78, 216, 0.25)");
  });

  it("draws the caret at the head: the end where the selection was made backwards", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 11, 6));
    expect(decorations(view)).toEqual([
      [7, 7, "caret"],
      [7, 12, "selection"],
    ]);
  });

  it("a caret without a selection is only the caret", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 3, 3));
    expect(decorations(view)).toEqual([[4, 4, "caret"]]);
    expect(view.dom.querySelector(".loro-cursor-selection")).toBeNull();
  });

  it("draws several people, each in their own colour", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    store.setRemote("device-carol", { name: "Carol", color: GREEN }, selectionUpdate(doc, 6, 11));
    expect(view.dom.querySelectorAll(".loro-cursor")).toHaveLength(2);
    const labels = [...view.dom.querySelectorAll<HTMLElement>(".loro-cursor-label")].map((label) => [label.textContent, label.style.backgroundColor]);
    expect(labels).toEqual([
      ["Bob", "rgb(29, 78, 216)"],
      ["Carol", "rgb(21, 128, 61)"],
    ]);
  });

  it("follows the text: a local edit before the caret moves it, and so does a remote one", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 6, 11));
    view.dispatch(view.state.tr.insertText("Oh! ", 1));
    expect(decorations(view)).toEqual([
      [11, 16, "selection"],
      [16, 16, "caret"],
    ]);
    // Another replica of the page types at its start; the change arrives.
    const other = new LoroDoc();
    other.import(doc.export({ mode: "snapshot" }));
    other.setPeerId(77);
    firstText(other).insert(0, ">> ");
    other.commit();
    sync(other, doc);
    expect(decorations(view)).toEqual([
      [14, 19, "selection"],
      [19, 19, "caret"],
    ]);
  });

  it("goes when the device's state is removed, when it times out and when a frame says it left the page", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    store.removeRemote("device-bob");
    expect(decorations(view)).toEqual([]);

    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    vi.advanceTimersByTime(29_999);
    expect(decorations(view)).toHaveLength(2);
    vi.advanceTimersByTime(2);
    expect(decorations(view)).toEqual([]);
    expect(view.dom.querySelector(".loro-cursor")).toBeNull();

    // An update without a cursor (the device only views the page) removes its cursors too.
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    store.setRemote("device-bob", bob, new Uint8Array());
    expect(decorations(view)).toEqual([]);
  });

  it("another frame starts the 30 seconds again", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    vi.advanceTimersByTime(20_000);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 6));
    vi.advanceTimersByTime(20_000);
    expect(decorations(view)).toEqual([
      [1, 7, "selection"],
      [7, 7, "caret"],
    ]);
    vi.advanceTimersByTime(10_001);
    expect(decorations(view)).toEqual([]);
  });

  it("does not draw this device's own cursor", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    const own = firstText(doc);
    store.setLocal({ anchor: own.getCursor(2), focus: own.getCursor(5) });
    expect(decorations(view)).toEqual([]);
    // Nor does a frame that names this device's own key.
    store.setRemote(store.peer, bob, selectionUpdate(doc, 0, 5));
    expect(decorations(view)).toEqual([]);
  });

  it("does not draw a cursor of text this replica does not hold, and logs nothing", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    // Another replica has a paragraph this one has not seen yet, and a device in it.
    const ahead = new LoroDoc();
    ahead.import(doc.export({ mode: "snapshot" }));
    ahead.setPeerId(88);
    const list = ahead.getMap("doc").get("children") as LoroList;
    const block = list.insertContainer(1, new LoroMap());
    block.set("nodeName", "paragraph");
    block.setContainer("attributes", new LoroMap());
    const children = block.setContainer("children", new LoroList());
    const text = children.insertContainer(0, new LoroText());
    text.insert(0, "Only there");
    ahead.commit();
    store.setRemote("device-ahead", bob, frame({ anchor: text.getCursor(3)?.encode(), focus: text.getCursor(6)?.encode() }));
    expect(decorations(view)).toEqual([]);
    // Once the text arrives the device is drawn.
    sync(ahead, doc);
    store.setRemote("device-ahead", bob, frame({ anchor: text.getCursor(3)?.encode(), focus: text.getCursor(6)?.encode() }));
    expect(decorations(view)).toHaveLength(2);
    expect(errors).not.toHaveBeenCalled();
    expect(loroToPortable(doc)).toMatchObject({ type: "doc" });
  });
});

describe("this view's selection", () => {
  it("is written to the store as Loro cursors while the editor has focus, and cleared when it has not", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc, { attach: true });
    expect(store.encodeLocal()).toEqual(new Uint8Array());
    view.focus();
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 3, 8)));
    const local = store.getLocal();
    expect(local?.anchor).toBeDefined();
    const update = store.encodeLocal();
    expect(update.length).toBeGreaterThan(0);
    // What a receiver reads from the update: the cursors at offsets 2 and 7 of the page's text.
    const wire = decodeCursorUpdate(update);
    const at = (bytes: Uint8Array | undefined) => (bytes === undefined ? undefined : doc.getCursorPos(Cursor.decode(bytes))?.offset);
    expect([at(wire?.anchor), at(wire?.focus)]).toEqual([2, 7]);
    // A caret: the anchor and the head are one.
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 5)));
    const caret = decodeCursorUpdate(store.encodeLocal());
    expect([at(caret?.anchor), at(caret?.focus)]).toEqual([4, 4]);

    view.dom.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    expect(store.getLocal()).toBeUndefined();
    expect(store.encodeLocal()).toEqual(new Uint8Array());
  });

  it("is not written while the editor has no focus", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 3, 8)));
    expect(store.getLocal()).toBeUndefined();
  });

  it("tells the listeners of the store that this device changed, and the plugin does not redraw for it", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc, { attach: true });
    const seen: string[] = [];
    store.subscribeBy((by) => seen.push(by));
    const before = loroCursorPluginKey.getState(view.state);
    view.focus();
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 3)));
    expect(seen).toContain("local");
    expect(loroCursorPluginKey.getState(view.state)).toBe(before);
  });
});

describe("the update of a frame", () => {
  const sampleDoc = loroDocFrom(paragraphs("Hello world"));

  it("keeps the two cursors and drops everything else, a name and a colour included", () => {
    const text = firstText(sampleDoc);
    const decoded = decodeCursorUpdate(
      frame({
        anchor: text.getCursor(1)?.encode(),
        focus: text.getCursor(4)?.encode(),
        user: { name: "Alice", color: "#ff0000" },
        extra: "x".repeat(100),
      }),
    );
    expect(Object.keys(decoded ?? {}).sort()).toEqual(["anchor", "focus"]);
  });

  it("takes the state of whatever key the sender used", () => {
    const text = firstText(sampleDoc);
    for (const key of ["peer", "12345", "a much longer key of the sender's own choosing"]) {
      expect(decodeCursorUpdate(frame({ anchor: text.getCursor(1)?.encode() }, key))?.anchor).toBeDefined();
    }
  });

  it("is not fooled by a sender whose clock is hours off, in either direction", () => {
    const text = firstText(sampleDoc);
    const now = Date.now();
    for (const skew of [-6 * 3600 * 1000, 6 * 3600 * 1000]) {
      vi.setSystemTime(now + skew);
      const update = frame({ anchor: text.getCursor(1)?.encode() });
      vi.setSystemTime(now);
      expect(decodeCursorUpdate(update)?.anchor).toBeDefined();
    }
  });

  it("ignores garbage, an empty update, one over the limit, wrong types and cursors of the wrong size", () => {
    const text = firstText(sampleDoc);
    expect(decodeCursorUpdate(new Uint8Array())).toBeUndefined();
    expect(decodeCursorUpdate(Uint8Array.of(1, 2, 3, 4, 5))).toBeUndefined();
    expect(decodeCursorUpdate(new Uint8Array(4_000).fill(7))).toBeUndefined();
    expect(decodeCursorUpdate(frame({ anchor: "not bytes", focus: 5 }))).toBeUndefined();
    expect(decodeCursorUpdate(frame({ anchor: new Uint8Array(300) }))).toBeUndefined();
    expect(decodeCursorUpdate(frame({ anchor: new Uint8Array() }))).toBeUndefined();
    expect(decodeCursorUpdate(frame({ user: { name: "x", color: "red" } }))).toBeUndefined();
    // Cursor bytes that are not a cursor are kept as bytes but never become a caret.
    expect(decodeCursorUpdate(frame({ anchor: Uint8Array.of(9, 9, 9), focus: text.getCursor(2)?.encode() }))?.anchor).toBeDefined();
  });

  it("cannot overwrite another device's cursor: the key is the device the core verified", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    // Carol's frame uses the key Bob's frame used ("peer") and a state of its own: Bob's stays where it is.
    store.setRemote("device-carol", { name: "Carol", color: GREEN }, selectionUpdate(doc, 6, 11));
    expect(decorations(view)).toEqual([
      [1, 6, "selection"],
      [6, 6, "caret"],
      [7, 12, "selection"],
      [12, 12, "caret"],
    ]);
  });

  it("garbage bytes of cursors draw nothing and break nothing", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const doc = loroDocFrom(paragraphs("Hello world"));
    const { view, store } = await open(doc);
    store.setRemote("device-bad", bob, frame({ anchor: Uint8Array.of(9, 9, 9), focus: Uint8Array.of(1, 2) }));
    expect(decorations(view)).toEqual([]);
    expect(errors).not.toHaveBeenCalled();
  });
});

describe("the store", () => {
  it("is announced to its listeners by who changed it, and stops when destroyed", () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const store = newStore(doc);
    const seen: string[] = [];
    const stop = store.subscribeBy((by) => seen.push(by));
    const text = firstText(doc);
    store.setLocal({ anchor: text.getCursor(1) });
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    vi.advanceTimersByTime(30_000);
    store.setLocal({});
    expect(seen).toEqual(["local", "import", "timeout", "local"]);
    stop();
    store.setLocal({ anchor: text.getCursor(1) });
    expect(seen).toHaveLength(4);
  });

  it("keeps this device's cursor in its frames for as long as it is set, however long ago it moved", () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const store = newStore(doc);
    const text = firstText(doc);
    store.setLocal({ anchor: text.getCursor(1), focus: text.getCursor(3) });
    for (let minute = 0; minute < 3; minute += 1) {
      vi.advanceTimersByTime(60_000);
      expect(decodeCursorUpdate(store.encodeLocal())?.anchor).toBeDefined();
    }
    expect(Object.keys(store.getAll())).toEqual([store.peer]);
  });

  it("never sends a name or a colour of this device in its update", () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const store = newStore(doc);
    store.setLocal({ anchor: firstText(doc).getCursor(1), user: { name: "Maya", color: "#123456" } });
    const read = new EphemeralStore(7 * 24 * 3600 * 1000);
    read.apply(store.encodeLocal());
    expect(Object.keys(Object.values(read.getAllStates())[0] as object)).toEqual(["anchor"]);
    read.destroy();
    expect(store.getLocal()?.user).toEqual({ name: "Maya", color: "#123456" });
  });

  it("builds the caret widget with text only: a name is never markup", () => {
    const caret = defaultCursorBuilder({ name: "<img src=x onerror=alert(1)>", color: BLUE });
    expect(caret.querySelector("img")).toBeNull();
    expect(caret.querySelector(".loro-cursor-label")?.textContent).toBe("<img src=x onerror=alert(1)>");
    expect(caret.getAttribute("aria-hidden")).toBe("true");
  });
});

describe("the editor stays itself", () => {
  it("a remote caret does not make an edit: the document is untouched", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const before = doc.oplogVersion();
    const { view, store } = await open(doc);
    store.setRemote("device-bob", bob, selectionUpdate(doc, 0, 5));
    expect(doc.oplogVersion().compare(before)).toBe(0);
    expect(view.state.doc.textContent).toBe("Hello world");
  });
});
