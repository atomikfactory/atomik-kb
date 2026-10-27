// The device-local active view of each database block (ADR-0054): the setting `ui.database.active_views`, how it is
// read and kept (most recent 200), and the store action that remembers a choice in the cache and the core.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import { ACTIVE_VIEWS_KEY, MAX_ACTIVE_VIEW_ENTRIES, parseActiveViews, selectActiveViewId, withActiveView } from "./store-views";

describe("the setting", () => {
  it("reads an object of block id to view id", () => {
    expect(parseActiveViews({ b1: "v1", b2: "v2" })).toEqual({ b1: "v1", b2: "v2" });
  });

  it("reads anything else as no choices, and skips an entry that is no non-empty text", () => {
    for (const odd of [null, undefined, 3, "x", true, [], ["a"]]) {
      expect(parseActiveViews(odd as JsonValue), JSON.stringify(odd)).toEqual({});
    }
    expect(parseActiveViews({ b1: "v1", b2: 4, b3: "", b4: null, b5: ["v"], "": "v" })).toEqual({ b1: "v1" });
  });

  it("never reads a prototype key", () => {
    const stored = JSON.parse('{"__proto__":"v1","b":"v"}') as JsonValue;
    expect(parseActiveViews(stored)).toEqual({ b: "v" });
  });

  it("keeps the last 200 entries", () => {
    const stored: Record<string, string> = {};
    for (let i = 0; i < 250; i += 1) {
      stored[`block-${String(i)}`] = "v";
    }
    const read = parseActiveViews(stored);
    expect(Object.keys(read)).toHaveLength(MAX_ACTIVE_VIEW_ENTRIES);
    expect(Object.keys(read)[0]).toBe("block-50");
    expect(read["block-249"]).toBe("v");
  });
});

describe("choosing a view", () => {
  it("sets the block's view and moves it to the end", () => {
    const next = withActiveView({ a: "1", b: "2" }, "a", "3");
    expect(next).toEqual({ a: "3", b: "2" });
    expect(Object.keys(next)).toEqual(["b", "a"]);
  });

  it("answers the same map when the block already shows that view, and drops the oldest past 200", () => {
    const same = { a: "1" };
    expect(withActiveView(same, "a", "1")).toBe(same);
    let map: Readonly<Record<string, string>> = {};
    for (let i = 0; i < MAX_ACTIVE_VIEW_ENTRIES + 5; i += 1) {
      map = withActiveView(map, `block-${String(i)}`, "v");
    }
    expect(Object.keys(map)).toHaveLength(MAX_ACTIVE_VIEW_ENTRIES);
    expect(map["block-0"]).toBeUndefined();
    expect(map[`block-${String(MAX_ACTIVE_VIEW_ENTRIES + 4)}`]).toBe("v");
  });
});

describe("the store", () => {
  async function setup(stored?: JsonValue) {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [{ name: "Personal", pages: [{ title: "Home" }] }] });
    const client = new CoreClient(memory);
    if (stored !== undefined) {
      await client.settingsSet(ACTIVE_VIEWS_KEY, stored);
    }
    const set = vi.spyOn(client, "settingsSet");
    const store = createAppStore(client, { corePollMs: 5 });
    const stop = store.getState().start();
    await vi.waitFor(() => {
      expect(store.getState().phase).toBe("ready");
    });
    return { client, store, stop, set };
  }

  it("reads the stored choices at boot, before the first paint", async () => {
    const { store, stop } = await setup({ b1: "v2" });
    expect(selectActiveViewId(store.getState(), "b1")).toBe("v2");
    expect(selectActiveViewId(store.getState(), "b9")).toBeUndefined();
    stop();
  });

  it("remembers a choice in the cache at once and writes the setting once, not for the same choice again", async () => {
    const { client, store, stop, set } = await setup();
    store.getState().setActiveView("b1", "v3");
    expect(selectActiveViewId(store.getState(), "b1")).toBe("v3");
    store.getState().setActiveView("b1", "v3");
    expect(set).toHaveBeenCalledTimes(1);
    expect(set).toHaveBeenCalledWith(ACTIVE_VIEWS_KEY, { b1: "v3" });
    await vi.waitFor(async () => {
      expect(await client.settingsGet(ACTIVE_VIEWS_KEY)).toEqual({ b1: "v3" });
    });
    store.getState().setActiveView("b2", "v1");
    expect(selectActiveViewId(store.getState(), "b1")).toBe("v3");
    expect(selectActiveViewId(store.getState(), "b2")).toBe("v1");
    stop();
  });

  it("works without a page being open and never writes the document (nothing but the setting)", async () => {
    const { client, store, stop, set } = await setup();
    const dispatch = vi.spyOn(client, "pageContentGet");
    store.getState().setActiveView("b1", "v1");
    expect(dispatch).not.toHaveBeenCalled();
    expect(set).toHaveBeenCalledTimes(1);
    stop();
  });
});
