// The peek's state and its device-local width (ADR-0054, section 8 of `docs/architecture/database-views.md`): the width
// setting and how it is read and kept, opening, swapping and closing, and the plain rule that closes the peek when the
// tab, the page or the space in front changes, and the rest of the closing rules: the screen is left, the app locks, the page
// is trashed, purged or moved out, a page nobody has seen yet stays. Over the in-memory core.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue, PageNode } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import {
  PEEK_DEFAULT_WIDTH,
  PEEK_MAX_WIDTH,
  PEEK_MIN_WIDTH,
  PEEK_WIDTH_KEY,
  clampPeekWidth,
  parsePeekWidth,
  selectPeekWidth,
} from "./store-peek";

describe("the width setting", () => {
  it("reads an integer from 320 to 960 as it is and anything else as 480", () => {
    expect(parsePeekWidth(PEEK_MIN_WIDTH)).toBe(320);
    expect(parsePeekWidth(640)).toBe(640);
    expect(parsePeekWidth(PEEK_MAX_WIDTH)).toBe(960);
    for (const bad of [319, 961, 500.5, "500", null, undefined, true, [500], { width: 500 }, Number.NaN]) {
      expect(parsePeekWidth(bad as never), JSON.stringify(bad)).toBe(PEEK_DEFAULT_WIDTH);
    }
  });

  it("clamps a chosen width to 320 to 960 and rounds it", () => {
    expect(clampPeekWidth(100)).toBe(PEEK_MIN_WIDTH);
    expect(clampPeekWidth(5000)).toBe(PEEK_MAX_WIDTH);
    expect(clampPeekWidth(500.4)).toBe(500);
    expect(clampPeekWidth(500.6)).toBe(501);
    expect(clampPeekWidth(Number.NaN)).toBe(PEEK_DEFAULT_WIDTH);
  });
});

async function setup(stored?: JsonValue) {
  const memory = new MemoryTransport({
    workspaceName: "Research",
    spaces: [
      { name: "Personal", pages: [{ title: "Hub", children: [{ title: "Alpha" }, { title: "Beta" }] }, { title: "Other" }] },
      { name: "Work", pages: [{ title: "Meetings" }] },
    ],
  });
  const client = new CoreClient(memory);
  if (stored !== undefined) {
    await client.settingsSet(PEEK_WIDTH_KEY, stored);
  }
  const writes = vi.spyOn(client, "settingsSet");
  const store = createAppStore(client, { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  const space = store.getState().currentSpaceId ?? "";
  const ids = new Map<string, string>();
  const visit = (nodes: readonly PageNode[]): void => {
    for (const node of nodes) {
      ids.set(node.page.title, node.page.id);
      visit(node.children);
    }
  };
  visit((await client.treeGet(space)).roots);
  const otherSpace = (await client.spaceList()).find((candidate) => candidate.id !== space)?.id ?? "";
  visit((await client.treeGet(otherSpace)).roots);
  return { client, store, stop, writes, space, otherSpace, id: (title: string) => ids.get(title) ?? "" };
}

describe("the store", () => {
  it("reads the stored width at boot and falls back to the default", async () => {
    const stored = await setup(700);
    expect(selectPeekWidth(stored.store.getState())).toBe(700);
    stored.stop();
    const fresh = await setup();
    expect(selectPeekWidth(fresh.store.getState())).toBe(PEEK_DEFAULT_WIDTH);
    fresh.stop();
  });

  it("clamps a width, remembers it in the cache at once and writes the setting once per change", async () => {
    const { store, stop, writes } = await setup();
    store.getState().setPeekWidth(5000);
    expect(selectPeekWidth(store.getState())).toBe(PEEK_MAX_WIDTH);
    store.getState().setPeekWidth(5000);
    expect(writes).toHaveBeenCalledTimes(1);
    expect(writes).toHaveBeenCalledWith(PEEK_WIDTH_KEY, PEEK_MAX_WIDTH);
    store.getState().setPeekWidth(PEEK_DEFAULT_WIDTH);
    expect(writes).toHaveBeenCalledTimes(2);
    stop();
  });

  it("opens, swaps and closes", async () => {
    const { store, stop, space, id } = await setup();
    expect(store.getState().peek).toBeNull();
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    expect(store.getState().peek).toEqual({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().openPeek({ pageId: id("Beta"), spaceId: space, mode: "center" });
    expect(store.getState().peek).toEqual({ pageId: id("Beta"), spaceId: space, mode: "center" });
    store.getState().closePeek();
    expect(store.getState().peek).toBeNull();
    store.getState().closePeek();
    expect(store.getState().peek).toBeNull();
    stop();
  });

  it("leaves the tab, the page in front and the history as they are", async () => {
    const { store, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    const before = { tabs: store.getState().tabs, active: store.getState().activeTabId, selected: store.getState().selectedPageId };
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().openPeek({ pageId: id("Beta"), spaceId: space, mode: "side" });
    expect(store.getState().tabs).toBe(before.tabs);
    expect(store.getState().activeTabId).toBe(before.active);
    expect(store.getState().selectedPageId).toBe(before.selected);
    stop();
  });

  it("closes when the page in front changes, when another tab comes to the front, and keeps the width", async () => {
    const { store, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().navigateTo({ pageId: id("Other"), spaceId: space });
    expect(store.getState().peek).toBeNull();

    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().newTab();
    expect(store.getState().peek).toBeNull();

    store.getState().setPeekWidth(600);
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().closePeek();
    expect(selectPeekWidth(store.getState())).toBe(600);
    stop();
  });
});

describe("what opens it", () => {
  it("never opens the page in front, which an editor shows already", async () => {
    const { store, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Hub"), spaceId: space, mode: "side" });
    expect(store.getState().peek).toBeNull();
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().openPeek({ pageId: id("Hub"), spaceId: space, mode: "center" });
    expect(store.getState().peek?.pageId).toBe(id("Alpha"));
    stop();
  });

  it("shows pages of the space in front only", async () => {
    const { store, stop, space, otherSpace, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Meetings"), spaceId: otherSpace, mode: "side" });
    expect(store.getState().peek).toBeNull();
    stop();
  });

  it("keeps the same object when the same page is opened again", async () => {
    const { store, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    const first = store.getState().peek;
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    expect(store.getState().peek).toBe(first);
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "center" });
    expect(store.getState().peek?.mode).toBe("center");
    stop();
  });
});

describe("what closes it", () => {
  it("closes when the main screen is left, and when the app is no longer open", async () => {
    const { store, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().navigate("settings");
    expect(store.getState().peek).toBeNull();

    store.getState().navigate("home");
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "center" });
    expect(store.getState().peek).not.toBeNull();
    store.setState({ phase: "locked" });
    expect(store.getState().peek).toBeNull();
    stop();
  });

  it("closes when the space changes", async () => {
    const { store, stop, space, otherSpace, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.getState().selectSpace(otherSpace);
    expect(store.getState().peek).toBeNull();
    stop();
  });

  it("closes when its page is trashed, and when it is purged", async () => {
    const { store, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    await store.getState().trashPage(id("Alpha"));
    await vi.waitFor(() => {
      expect(store.getState().peek).toBeNull();
    });

    store.getState().openPeek({ pageId: id("Beta"), spaceId: space, mode: "center" });
    await store.getState().trashPage(id("Beta"));
    await vi.waitFor(() => {
      expect(store.getState().peek).toBeNull();
    });
    await store.getState().purgePage(id("Beta"));
    expect(store.getState().peek).toBeNull();
    stop();
  });

  it("closes when the lookup of its page says it is in the trash or gone", async () => {
    const { store, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    store.setState({ pageLookup: { [id("Alpha")]: { status: "missing" } } });
    expect(store.getState().peek).toBeNull();
    stop();
  });

  it("stays open while the tree is read again with its page in it, and for a page the tree has not seen yet", async () => {
    const { store, client, stop, space, id } = await setup();
    store.getState().navigateTo({ pageId: id("Hub"), spaceId: space });
    store.getState().openPeek({ pageId: id("Alpha"), spaceId: space, mode: "side" });
    await store.getState().renamePage(id("Beta"), "Beta 2");
    await vi.waitFor(() => {
      expect(store.getState().tree?.roots[0]?.children.some((node) => node.page.title === "Beta 2")).toBe(true);
    });
    expect(store.getState().peek?.pageId).toBe(id("Alpha"));

    // A page made a moment ago: the row opens the peek before the tree has caught up with it.
    const made = await client.pageCreate({ spaceId: space, parentId: id("Hub"), index: 0, title: "Made" });
    store.getState().openPeek({ pageId: made.id, spaceId: space, mode: "side" });
    await vi.waitFor(() => {
      expect(store.getState().tree?.roots[0]?.children.some((node) => node.page.id === made.id)).toBe(true);
    });
    expect(store.getState().peek?.pageId).toBe(made.id);
    stop();
  });
});

