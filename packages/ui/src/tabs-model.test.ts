// The rules of the tabs (ADR-0034) as pure functions: opening, history, closing and reopening,
// pinning and order, gone entries, and the stored tab set.

import type { JsonValue } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import type { StoredTabs, Tab, TabEntry } from "./store-tabs";
import {
  EMPTY_MODEL,
  MAX_CLOSED_TABS,
  MAX_TABS,
  MAX_TAB_HISTORY,
  activateAdjacentTab,
  activateTab,
  activateTabAt,
  appendHomeTab,
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  currentEntry,
  mirrorOf,
  movePagesToSpace,
  moveTab,
  openTarget,
  parseStoredTabs,
  pruneTab,
  removeEntries,
  reopenClosedTab,
  restoreTabs,
  setTabPinned,
  stepHistory,
  storedTabsToJson,
  tabInSpace,
  takePendingBlock,
  toStoredTabs,
} from "./tabs-model";
import type { TabsModel } from "./tabs-model";

const SPACE = "space-a";
const OTHER_SPACE = "space-b";

/** Ids that read well in a failure: `n1`, `n2`, ... */
function counter(): () => string {
  let n = 0;
  return () => {
    n += 1;
    return `n${n}`;
  };
}

function entry(id: string, pageId: string | null, spaceId = SPACE, blockId?: string): TabEntry {
  return blockId === undefined ? { id, spaceId, pageId } : { id, spaceId, pageId, blockId };
}

/** A tab with its entries given as page ids (`null` is a Home); the current one is the last unless `index` says. */
function tab(id: string, pages: readonly (string | null)[], options: { index?: number; pinned?: boolean; space?: string } = {}): Tab {
  const entries = pages.map((page, at) => entry(`${id}.${at}`, page, options.space ?? SPACE));
  return { id, pinned: options.pinned ?? false, entries, index: options.index ?? entries.length - 1 };
}

/** A tab on one page: `t1` shows `p1`, and so on. */
function page(n: number, options: { pinned?: boolean; space?: string } = {}): Tab {
  return tab(`t${n}`, [`p${n}`], options);
}

function model(tabs: readonly Tab[], activeTabId: string | null = tabs[0]?.id ?? null, extra: Partial<TabsModel> = {}): TabsModel {
  return { ...EMPTY_MODEL, tabs, activeTabId, activation: activeTabId === null ? [] : [activeTabId], ...extra };
}

const ids = (m: TabsModel): string[] => m.tabs.map((t) => t.id);
const pages = (m: TabsModel): (string | null)[] => m.tabs.map((t) => currentEntry(t).pageId);
const target = (pageId: string | null, spaceId = SPACE, blockId?: string) =>
  blockId === undefined ? { spaceId, pageId } : { spaceId, pageId, blockId };

describe("opening a page", () => {
  it("a current open navigates the active tab: forward entries are dropped, a new entry is pushed", () => {
    const m = model([tab("t1", ["a", "b", "c"], { index: 1 }), page(2)]);
    const { model: next, showed } = openTarget(m, target("d"), "current", counter());
    expect(showed).toBe(true);
    const [first] = next.tabs;
    expect(first?.entries.map((e) => e.pageId)).toEqual(["a", "b", "d"]);
    expect(first?.index).toBe(2);
    expect(next.activeTabId).toBe("t1");
    expect(next.tabs).toHaveLength(2);
  });

  it("drops the oldest entry beyond the history limit", () => {
    const pagesOf = Array.from({ length: MAX_TAB_HISTORY }, (_, at) => `p${at}`);
    const m = model([tab("t1", pagesOf)]);
    const next = openTarget(m, target("new"), "current", counter()).model.tabs[0];
    expect(next?.entries).toHaveLength(MAX_TAB_HISTORY);
    expect(next?.entries[0]?.pageId).toBe("p1");
    expect(next?.entries.at(-1)?.pageId).toBe("new");
    expect(next?.index).toBe(MAX_TAB_HISTORY - 1);
  });

  it("the page the active tab shows again only shows: no entry is added", () => {
    const m = model([tab("t1", ["a", "b"])]);
    const { model: next, showed } = openTarget(m, target("b"), "current", counter());
    expect(showed).toBe(true);
    expect(next.tabs[0]).toBe(m.tabs[0]);
  });

  it("the Home of the space the active tab shows is not pushed twice", () => {
    const m = model([tab("t1", ["a", null])]);
    const { model: next, showed } = openTarget(m, target(null), "current", counter());
    expect(showed).toBe(true);
    expect(next.tabs[0]).toBe(m.tabs[0]);
  });

  it("a page open in another tab brings that tab to the front instead of opening it twice", () => {
    const m = model([page(1), page(2), page(3)]);
    for (const disposition of ["current", "foreground"] as const) {
      const { model: next, showed } = openTarget(m, target("p3"), disposition, counter());
      expect(showed).toBe(true);
      expect(next.activeTabId).toBe("t3");
      expect(next.tabs).toBe(m.tabs);
    }
  });

  it("a background open of a page that is open does nothing", () => {
    const m = model([page(1), page(2)]);
    const { model: next, showed } = openTarget(m, target("p2"), "background", counter());
    expect(showed).toBe(false);
    expect(next).toBe(m);
  });

  it("with the page in several tabs, the active one wins, else the most recently activated", () => {
    // Back and forward can make two tabs show the same page.
    const tabs = [tab("t1", ["x"]), tab("t2", ["x"]), tab("t3", ["x"]), page(4)];
    const viaActive = openTarget(model(tabs, "t2", { activation: ["t2", "t3", "t1"] }), target("x"), "current", counter());
    expect(viaActive.model.activeTabId).toBe("t2");
    const viaRecent = openTarget(model(tabs, "t4", { activation: ["t4", "t3", "t1", "t2"] }), target("x"), "current", counter());
    expect(viaRecent.model.activeTabId).toBe("t3");
    // A tab that was never activated ranks after the ones that were, then strip order.
    const never = openTarget(model(tabs, "t4", { activation: ["t4"] }), target("x"), "current", counter());
    expect(never.model.activeTabId).toBe("t1");
  });

  it("a background open adds a tab right after the active one and leaves the active tab", () => {
    const m = model([page(1), page(2), page(3)], "t2");
    const { model: next, showed } = openTarget(m, target("new"), "background", counter());
    expect(showed).toBe(false);
    expect(pages(next)).toEqual(["p1", "p2", "new", "p3"]);
    expect(next.activeTabId).toBe("t2");
  });

  it("successive background opens from the same tab line up in order", () => {
    const next = counter();
    let m = model([page(1), page(2), page(3)], "t1");
    for (const name of ["a", "b", "c"]) {
      m = openTarget(m, target(name), "background", next).model;
    }
    expect(pages(m)).toEqual(["p1", "a", "b", "c", "p2", "p3"]);
  });

  it("another tab in front starts a new line of background tabs", () => {
    const next = counter();
    let m = model([page(1), page(2)], "t1");
    m = openTarget(m, target("a"), "background", next).model;
    m = activateTab(m, "t2");
    m = openTarget(m, target("b"), "background", next).model;
    m = activateTab(m, "t1");
    m = openTarget(m, target("c"), "background", next).model;
    expect(pages(m)).toEqual(["p1", "c", "a", "p2", "b"]);
  });

  it("a background tab keeps the block for the first time its entry shows", () => {
    const m = model([page(1)]);
    const { model: next } = openTarget(m, target("p2", SPACE, "block-9"), "background", counter());
    const background = next.tabs[1];
    expect(background && currentEntry(background).blockId).toBe("block-9");
    // A current or foreground open scrolls at once: the entry carries nothing.
    const foreground = openTarget(m, target("p3", SPACE, "block-9"), "foreground", counter()).model.tabs[1];
    expect(foreground && currentEntry(foreground).blockId).toBeUndefined();
  });

  it("a foreground open adds a tab after the active one and brings it to the front", () => {
    const m = model([page(1), page(2)], "t1");
    const { model: next, showed } = openTarget(m, target("new"), "foreground", counter());
    expect(showed).toBe(true);
    expect(pages(next)).toEqual(["p1", "new", "p2"]);
    expect(currentEntry(next.tabs[1] as Tab).pageId).toBe("new");
    expect(next.activeTabId).toBe(next.tabs[1]?.id);
    expect(next.activation[0]).toBe(next.activeTabId);
  });

  it("a foreground open goes behind the tabs opened from the active tab in the background", () => {
    const next = counter();
    let m = model([page(1), page(2)], "t1");
    m = openTarget(m, target("a"), "background", next).model;
    m = openTarget(m, target("b"), "foreground", next).model;
    expect(pages(m)).toEqual(["p1", "a", "b", "p2"]);
  });

  it("opens in another space when the target says so, and the Home of a space when there is no page", () => {
    const m = model([page(1)]);
    const there = openTarget(m, target("q", OTHER_SPACE), "current", counter()).model;
    expect(mirrorOf(there)).toEqual({ currentSpaceId: OTHER_SPACE, selectedPageId: "q" });
    const home = openTarget(m, target(null, OTHER_SPACE), "foreground", counter()).model;
    expect(mirrorOf(home)).toEqual({ currentSpaceId: OTHER_SPACE, selectedPageId: null });
  });

  it("a pinned active tab keeps its page: a current open becomes a foreground one", () => {
    const m = model([page(1, { pinned: true }), page(2, { pinned: true }), page(3)], "t1");
    const { model: next } = openTarget(m, target("new"), "current", counter());
    expect(m.tabs[0]).toBe(next.tabs[0]);
    expect(pages(next)).toEqual(["p1", "p2", "new", "p3"]);
    expect(next.activeTabId).toBe(next.tabs[2]?.id);
  });

  it("background tabs never go among the pinned tabs", () => {
    const next = counter();
    let m = model([page(1, { pinned: true }), page(2, { pinned: true }), page(3)], "t1");
    m = openTarget(m, target("a"), "background", next).model;
    m = openTarget(m, target("b"), "background", next).model;
    expect(pages(m)).toEqual(["p1", "p2", "a", "b", "p3"]);
    expect(m.tabs.map((t) => t.pinned)).toEqual([true, true, false, false, false]);
  });

  it(`at ${MAX_TABS} tabs a background or foreground open navigates the active tab instead`, () => {
    const full = model(Array.from({ length: MAX_TABS }, (_, at) => page(at + 1)), "t1");
    for (const disposition of ["background", "foreground"] as const) {
      const { model: next, showed } = openTarget(full, target("new"), disposition, counter());
      expect(showed).toBe(true);
      expect(next.tabs).toHaveLength(MAX_TABS);
      expect(next.tabs[0]?.entries.map((e) => e.pageId)).toEqual(["p1", "new"]);
    }
  });

  it("makes the first tab when there is none", () => {
    const { model: next, showed } = openTarget(EMPTY_MODEL, target("p1"), "background", counter());
    expect(showed).toBe(true);
    expect(pages(next)).toEqual(["p1"]);
    expect(next.activeTabId).toBe(next.tabs[0]?.id);
  });
});

describe("pending blocks", () => {
  it("takes the block off the front entry once", () => {
    const m = model([tab("t1", ["a", "b"])]);
    const withBlock: TabsModel = {
      ...m,
      tabs: [{ ...m.tabs[0], entries: [entry("e0", "a"), entry("e1", "b", SPACE, "blk")] } as Tab],
    };
    const first = takePendingBlock(withBlock);
    expect(first.blockId).toBe("blk");
    expect(currentEntry(first.model.tabs[0] as Tab).blockId).toBeUndefined();
    expect(takePendingBlock(first.model).blockId).toBeUndefined();
    expect(takePendingBlock(first.model).model).toBe(first.model);
  });
});

describe("history", () => {
  it("steps back and forward inside the active tab and stops at the ends", () => {
    const m = model([tab("t1", ["a", "b", "c"], { index: 1 })]);
    const back = stepHistory(m, -1);
    expect(back.tabs[0]?.index).toBe(0);
    expect(stepHistory(back, -1)).toBe(back);
    const forward = stepHistory(stepHistory(back, 1), 1);
    expect(forward.tabs[0]?.index).toBe(2);
    expect(stepHistory(forward, 1)).toBe(forward);
  });

  it("may land on a page that another tab shows", () => {
    const m = model([tab("t1", ["a", "b"]), tab("t2", ["a"])], "t1");
    const back = stepHistory(m, -1);
    expect(pages(back)).toEqual(["a", "a"]);
  });

  it("does nothing without a tab", () => {
    expect(stepHistory(EMPTY_MODEL, -1)).toBe(EMPTY_MODEL);
  });
});

describe("activating tabs", () => {
  it("brings a tab to the front and remembers the order", () => {
    const m = model([page(1), page(2), page(3)], "t1");
    const next = activateTab(activateTab(m, "t3"), "t2");
    expect(next.activeTabId).toBe("t2");
    expect(next.activation.slice(0, 3)).toEqual(["t2", "t3", "t1"]);
    expect(activateTab(m, "nope")).toBe(m);
  });

  it("by position: -1 is the last tab, out of range does nothing", () => {
    const m = model([page(1), page(2), page(3)]);
    expect(activateTabAt(m, -1).activeTabId).toBe("t3");
    expect(activateTabAt(m, 1).activeTabId).toBe("t2");
    expect(activateTabAt(m, 3)).toBe(m);
    expect(activateTabAt(m, -2)).toBe(m);
  });

  it("next and previous wrap, and do nothing with fewer than two tabs", () => {
    const m = model([page(1), page(2), page(3)], "t3");
    expect(activateAdjacentTab(m, 1).activeTabId).toBe("t1");
    expect(activateAdjacentTab(model([page(1), page(2)], "t1"), -1).activeTabId).toBe("t2");
    const single = model([page(1)]);
    expect(activateAdjacentTab(single, 1)).toBe(single);
  });

  it("finds the most recently activated tab of a space", () => {
    const tabs = [page(1), page(2, { space: OTHER_SPACE }), page(3, { space: OTHER_SPACE }), page(4)];
    const m = model(tabs, "t1", { activation: ["t1", "t3", "t2"] });
    expect(tabInSpace(m, OTHER_SPACE)?.id).toBe("t3");
    expect(tabInSpace(m, "space-c")).toBeNull();
  });

  it("a new Home tab goes last and comes to the front, but not beyond the limit", () => {
    const m = model([page(1), page(2)], "t1");
    const next = appendHomeTab(m, OTHER_SPACE, counter());
    expect(pages(next)).toEqual(["p1", "p2", null]);
    expect(next.activeTabId).toBe(next.tabs[2]?.id);
    expect(mirrorOf(next)).toEqual({ currentSpaceId: OTHER_SPACE, selectedPageId: null });
    const full = model(Array.from({ length: MAX_TABS }, (_, at) => page(at + 1)));
    expect(appendHomeTab(full, SPACE, counter())).toBe(full);
  });
});

describe("closing tabs", () => {
  it("the tab to the right takes the front, else the one to the left", () => {
    const m = model([page(1), page(2), page(3)], "t2");
    expect(closeTab(m, "t2", SPACE, counter()).activeTabId).toBe("t3");
    const last = model([page(1), page(2), page(3)], "t3");
    expect(closeTab(last, "t3", SPACE, counter()).activeTabId).toBe("t2");
  });

  it("closing a tab that is not in front leaves the front alone", () => {
    const m = model([page(1), page(2), page(3)], "t1");
    const next = closeTab(m, "t3", SPACE, counter());
    expect(next.activeTabId).toBe("t1");
    expect(ids(next)).toEqual(["t1", "t2"]);
    expect(closeTab(m, "nope", SPACE, counter())).toBe(m);
  });

  it("closing the last tab leaves a Home tab of the given space", () => {
    const m = model([page(1)]);
    const next = closeTab(m, "t1", OTHER_SPACE, counter());
    expect(next.tabs).toHaveLength(1);
    expect(mirrorOf(next)).toEqual({ currentSpaceId: OTHER_SPACE, selectedPageId: null });
    expect(next.closedTabs.map((t) => t.id)).toEqual(["t1"]);
    // With no space there is nothing to show.
    expect(closeTab(m, "t1", null, counter()).tabs).toEqual([]);
  });

  it("keeps the closed tabs, newest first, at most the limit", () => {
    let m = model(Array.from({ length: MAX_CLOSED_TABS + 2 }, (_, at) => page(at + 1)), "t1");
    for (let at = 1; at <= MAX_CLOSED_TABS + 1; at += 1) {
      m = closeTab(m, `t${at}`, SPACE, counter());
    }
    expect(m.closedTabs).toHaveLength(MAX_CLOSED_TABS);
    expect(m.closedTabs[0]?.id).toBe(`t${MAX_CLOSED_TABS + 1}`);
    expect(Object.keys(m.closedAt)).toHaveLength(MAX_CLOSED_TABS);
  });

  it("close others and close to the right keep the pinned tabs, and the kept tab takes the front", () => {
    const m = model([page(1, { pinned: true }), page(2), page(3), page(4), page(5)], "t5");
    const others = closeOtherTabs(m, "t3");
    expect(ids(others)).toEqual(["t1", "t3"]);
    expect(others.activeTabId).toBe("t3");
    expect(others.closedTabs.map((t) => t.id)).toEqual(["t5", "t4", "t2"]);

    const right = closeTabsToRight(model([page(1, { pinned: true }), page(2), page(3), page(4)], "t2"), "t2");
    expect(ids(right)).toEqual(["t1", "t2"]);
    expect(right.activeTabId).toBe("t2");

    // The front stays when it was not among the closed.
    const keepFront = closeTabsToRight(model([page(1), page(2), page(3)], "t1"), "t1");
    expect(keepFront.activeTabId).toBe("t1");
    expect(closeTabsToRight(model([page(1)]), "t1")).toEqual(model([page(1)]));
  });
});

describe("reopening a closed tab", () => {
  const nothingGone = (): boolean => false;

  it("brings it back with its history and position, at its place, and in front", () => {
    const m = model([page(1), tab("t2", ["a", "b", "c"], { index: 1 }), page(3)], "t2");
    const closed = closeTab(m, "t2", SPACE, counter());
    expect(ids(closed)).toEqual(["t1", "t3"]);
    const back = reopenClosedTab(closed, nothingGone);
    expect(ids(back)).toEqual(["t1", "t2", "t3"]);
    expect(back.activeTabId).toBe("t2");
    expect(back.tabs[1]?.entries.map((e) => e.pageId)).toEqual(["a", "b", "c"]);
    expect(back.tabs[1]?.index).toBe(1);
    expect(back.closedTabs).toEqual([]);
    expect(back.closedAt).toEqual({});
  });

  it("clamps the place when the strip got shorter, and keeps a pinned tab among the pinned", () => {
    const m = model([page(1), page(2), page(3), page(4)], "t1");
    const closed = closeTab(m, "t4", SPACE, counter());
    const fewer = closeTab(closeTab(closed, "t3", SPACE, counter()), "t2", SPACE, counter());
    expect(ids(reopenClosedTab(fewer, nothingGone))).toEqual(["t1", "t2"]);

    const pinned = model([page(1, { pinned: true }), page(2), page(3)], "t2");
    const closedPinned = closeTab(pinned, "t1", SPACE, counter());
    const reopened = reopenClosedTab(closedPinned, nothingGone);
    expect(ids(reopened)).toEqual(["t1", "t2", "t3"]);
    expect(reopened.tabs[0]?.pinned).toBe(true);
  });

  it("puts tabs closed together back one by one, newest first, in their old order", () => {
    const m = model([page(1), page(2), page(3), page(4)], "t1");
    let next = closeOtherTabs(m, "t1");
    expect(ids(next)).toEqual(["t1"]);
    for (let n = 0; n < 3; n += 1) {
      next = reopenClosedTab(next, nothingGone);
    }
    expect(ids(next)).toEqual(["t1", "t2", "t3", "t4"]);
  });

  it("brings the tab that shows the same page to the front instead, and drops the closed one", () => {
    const m = model([page(1), page(2)], "t1");
    const closed = closeTab(m, "t2", SPACE, counter());
    const opened = openTarget(closed, target("p2"), "background", counter()).model;
    const back = reopenClosedTab(opened, nothingGone);
    expect(back.tabs).toHaveLength(opened.tabs.length);
    expect(currentEntry(back.activeTabId === null ? (back.tabs[0] as Tab) : (back.tabs.find((t) => t.id === back.activeTabId) as Tab)).pageId).toBe("p2");
    expect(back.closedTabs).toEqual([]);
  });

  it("leaves out the entries that are gone, and skips a tab with nothing left", () => {
    const m = model([page(1), tab("t2", ["a", "b", "c"]), tab("t3", ["x"])], "t1");
    const closed = closeTab(closeTab(m, "t2", SPACE, counter()), "t3", SPACE, counter());
    const back = reopenClosedTab(closed, (e) => e.pageId === "x" || e.pageId === "c");
    expect(ids(back)).toEqual(["t1", "t2"]);
    expect(back.tabs[1]?.entries.map((e) => e.pageId)).toEqual(["a", "b"]);
    expect(back.closedTabs).toEqual([]);
  });

  it("does nothing without a closed tab, or when the strip is full", () => {
    const m = model([page(1)]);
    expect(reopenClosedTab(m, nothingGone)).toBe(m);
    const full = model(Array.from({ length: MAX_TABS }, (_, at) => page(at + 1)), "t1");
    const withClosed: TabsModel = { ...full, closedTabs: [page(99)], closedAt: { t99: 0 } };
    expect(reopenClosedTab(withClosed, nothingGone)).toBe(withClosed);
  });
});

describe("order and pinning", () => {
  it("moves a tab to a strip position and keeps it inside its group", () => {
    const m = model([page(1, { pinned: true }), page(2, { pinned: true }), page(3), page(4), page(5)]);
    expect(ids(moveTab(m, "t3", 4))).toEqual(["t1", "t2", "t4", "t5", "t3"]);
    expect(ids(moveTab(m, "t5", 3))).toEqual(["t1", "t2", "t3", "t5", "t4"]);
    expect(ids(moveTab(m, "t5", 0))).toEqual(["t1", "t2", "t5", "t3", "t4"]); // not among the pinned
    expect(ids(moveTab(m, "t1", 4))).toEqual(["t2", "t1", "t3", "t4", "t5"]); // not among the others
    expect(ids(moveTab(m, "t4", 99))).toEqual(["t1", "t2", "t3", "t5", "t4"]);
    expect(moveTab(m, "t3", 2)).toBe(m);
    expect(moveTab(m, "nope", 1)).toBe(m);
    expect(moveTab(m, "t3", Number.NaN)).toBe(m);
  });

  it("pins to the end of the pinned group and unpins to the start of the others", () => {
    const m = model([page(1, { pinned: true }), page(2), page(3), page(4)]);
    const pinned = setTabPinned(m, "t4", true);
    expect(ids(pinned)).toEqual(["t1", "t4", "t2", "t3"]);
    expect(pinned.tabs.map((t) => t.pinned)).toEqual([true, true, false, false]);
    const unpinned = setTabPinned(pinned, "t1", false);
    expect(ids(unpinned)).toEqual(["t4", "t1", "t2", "t3"]);
    expect(unpinned.tabs.map((t) => t.pinned)).toEqual([true, false, false, false]);
    expect(setTabPinned(m, "t2", false)).toBe(m);
  });
});

describe("removing gone entries", () => {
  const gone = (...pageIds: string[]) => (e: TabEntry) => e.pageId !== null && pageIds.includes(e.pageId);

  it("leaves a tab alone when nothing in it is gone", () => {
    const t = tab("t1", ["a", "b"]);
    expect(pruneTab(t, gone("z"))).toBe(t);
  });

  it("shows the entry before the current one when that went", () => {
    const t = tab("t1", ["a", "b", "c"], { index: 1 });
    const next = pruneTab(t, gone("b"));
    expect(next?.entries.map((e) => e.pageId)).toEqual(["a", "c"]);
    expect(next?.index).toBe(0);
  });

  it("else the entry after it", () => {
    const next = pruneTab(tab("t1", ["a", "b", "c"], { index: 0 }), gone("a"));
    expect(next?.entries.map((e) => e.pageId)).toEqual(["b", "c"]);
    expect(next?.index).toBe(0);
  });

  it("keeps the current entry when another one went", () => {
    const next = pruneTab(tab("t1", ["a", "b", "c"], { index: 2 }), gone("a"));
    expect(next?.entries.map((e) => e.pageId)).toEqual(["b", "c"]);
    expect(next?.index).toBe(1);
    expect(next && currentEntry(next).pageId).toBe("c");
  });

  it("merges neighbours that show the same place, keeping the current one", () => {
    const t = tab("t1", ["a", "b", "a", "c"], { index: 2 });
    const next = pruneTab(t, gone("b"));
    expect(next?.entries.map((e) => e.pageId)).toEqual(["a", "c"]);
    expect(next?.index).toBe(0);
    expect(next?.entries[0]?.id).toBe("t1.2"); // the entry that was current keeps its identity (and its scroll position)

    const homes = pruneTab(tab("t2", [null, "x", null, "y"], { index: 3 }), gone("x"));
    expect(homes?.entries.map((e) => e.pageId)).toEqual([null, "y"]);
    expect(homes?.index).toBe(1);
  });

  it("is null when every entry went", () => {
    expect(pruneTab(tab("t1", ["a", "b"]), gone("a", "b"))).toBeNull();
  });

  it("removes a page from every tab, closes the tabs left empty and moves the front to the neighbour", () => {
    const m = model([tab("t1", ["a", "x"]), tab("t2", ["x"]), page(3)], "t2");
    const next = removeEntries(m, gone("x"), SPACE, counter());
    expect(ids(next)).toEqual(["t1", "t3"]);
    expect(next.tabs[0]?.entries.map((e) => e.pageId)).toEqual(["a"]);
    expect(next.activeTabId).toBe("t3"); // the tab to the right of the one that closed
    expect(next.activation[0]).toBe("t3");
    expect(next.closedTabs).toEqual([]); // nothing to reopen

    const lastOne = removeEntries(model([tab("t1", ["a"]), tab("t2", ["x"])], "t2"), gone("x"), SPACE, counter());
    expect(lastOne.activeTabId).toBe("t1");
  });

  it("leaves the front tab on the page before the one that went (trashing the open page)", () => {
    const m = model([tab("t1", ["a", "b"]), page(2)], "t1");
    const next = removeEntries(m, gone("b"), SPACE, counter());
    expect(next.activeTabId).toBe("t1");
    expect(mirrorOf(next)).toEqual({ currentSpaceId: SPACE, selectedPageId: "a" });
  });

  it("makes a Home tab when no tab is left, or when there was none", () => {
    const m = model([tab("t1", ["x"])]);
    const next = removeEntries(m, gone("x"), OTHER_SPACE, counter());
    expect(next.tabs).toHaveLength(1);
    expect(mirrorOf(next)).toEqual({ currentSpaceId: OTHER_SPACE, selectedPageId: null });
    expect(removeEntries(m, gone("x"), null, counter()).tabs).toEqual([]);

    const none = removeEntries(EMPTY_MODEL, gone("x"), SPACE, counter());
    expect(none.tabs).toHaveLength(1);
    expect(removeEntries(EMPTY_MODEL, gone("x"), null, counter())).toBe(EMPTY_MODEL);
  });

  it("returns the very model when nothing went", () => {
    const m = model([page(1), page(2)]);
    expect(removeEntries(m, gone("z"), SPACE, counter())).toBe(m);
  });

  it("removes the entries of a deleted space, the Home entries included", () => {
    const m = model([tab("t1", [null, "a"], { space: OTHER_SPACE }), tab("t2", ["b", null]), tab("t3", [null], { space: OTHER_SPACE })], "t1");
    const next = removeEntries(m, (e) => e.spaceId === OTHER_SPACE, SPACE, counter());
    expect(ids(next)).toEqual(["t2"]);
    expect(next.activeTabId).toBe("t2");
  });

  it("does not keep a line of background tabs whose last tab went", () => {
    const next = counter();
    let m = model([page(1), page(2)], "t1");
    m = openTarget(m, target("a"), "background", next).model;
    const last = m.tabs[1] as Tab;
    const removed = removeEntries(m, gone("a"), SPACE, next);
    expect(removed.chain).toBeNull();
    expect(ids(removed)).not.toContain(last.id);
  });
});

describe("pages moved to another space (ADR-0038)", () => {
  const moved = (...pageIds: string[]): ReadonlySet<string> => new Set(pageIds);

  it("gives the entries of the moved pages the new space, in every tab and in the closed ones", () => {
    const closed = tab("t9", ["p1", "x"], { index: 0 });
    const m = model([page(1), tab("t2", ["a", "p2", "b"], { index: 1 }), page(3)], "t1", { closedTabs: [closed] });
    const next = movePagesToSpace(m, moved("p1", "p2"), OTHER_SPACE);
    expect(next.tabs.map((t) => t.entries.map((e) => [e.pageId, e.spaceId]))).toEqual([
      [["p1", OTHER_SPACE]],
      [["a", SPACE], ["p2", OTHER_SPACE], ["b", SPACE]],
      [["p3", SPACE]],
    ]);
    expect(next.closedTabs[0]?.entries.map((e) => [e.pageId, e.spaceId])).toEqual([["p1", OTHER_SPACE], ["x", SPACE]]);
    // The mirrors of the front tab follow its page.
    expect(mirrorOf(next)).toEqual({ currentSpaceId: OTHER_SPACE, selectedPageId: "p1" });
    // What did not move is the very same tab.
    expect(next.tabs[2]).toBe(m.tabs[2]);
  });

  it("keeps the ids of the entries, so that the scroll positions stay", () => {
    const m = model([tab("t1", ["p1", "b"], { index: 0 })]);
    const next = movePagesToSpace(m, moved("p1"), OTHER_SPACE);
    expect(next.tabs[0]?.entries.map((e) => e.id)).toEqual(["t1.0", "t1.1"]);
    expect(next.tabs[0]?.index).toBe(0);
  });

  it("is the model itself when no entry shows a moved page, or the page is in that space already", () => {
    const m = model([page(1), page(2, { space: OTHER_SPACE })]);
    expect(movePagesToSpace(m, moved("p9"), OTHER_SPACE)).toBe(m);
    expect(movePagesToSpace(m, moved("p2"), OTHER_SPACE)).toBe(m);
    expect(movePagesToSpace(m, moved(), OTHER_SPACE)).toBe(m);
  });

  it("leaves the Home of a space alone", () => {
    const m = model([tab("t1", [null, "p1"], { index: 1 })]);
    const next = movePagesToSpace(m, moved("p1"), OTHER_SPACE);
    expect(next.tabs[0]?.entries.map((e) => [e.pageId, e.spaceId])).toEqual([[null, SPACE], ["p1", OTHER_SPACE]]);
  });

  it("merges neighbours that then show the same place, the current entry surviving", () => {
    const entries = [entry("e0", "p1", OTHER_SPACE), entry("e1", "p1", SPACE), entry("e2", "b", SPACE)];
    const m = model([{ id: "t1", pinned: false, entries, index: 1 }]);
    const next = movePagesToSpace(m, moved("p1"), OTHER_SPACE);
    const merged = next.tabs[0];
    expect(merged?.entries.map((e) => [e.id, e.pageId, e.spaceId])).toEqual([
      ["e1", "p1", OTHER_SPACE],
      ["e2", "b", SPACE],
    ]);
    expect(merged?.index).toBe(0);
  });
});

describe("restoring tabs", () => {
  const stored: StoredTabs = {
    tabs: [
      { spaceId: SPACE, pageId: "p1", pinned: true },
      { spaceId: "gone-space", pageId: "p2", pinned: false },
      { spaceId: SPACE, pageId: null, pinned: false },
      { spaceId: OTHER_SPACE, pageId: "p3", pinned: false },
    ],
    active: 3,
  };

  it("makes a tab with one entry from each stored one, in place, dropping the tabs of missing spaces", () => {
    const m = restoreTabs(stored, [SPACE, OTHER_SPACE], counter());
    expect(pages(m)).toEqual(["p1", null, "p3"]);
    expect(m.tabs.map((t) => t.pinned)).toEqual([true, false, false]);
    expect(m.tabs.every((t) => t.entries.length === 1 && t.index === 0)).toBe(true);
    expect(m.activeTabId).toBe(m.tabs[2]?.id);
    expect(m.activation).toEqual([m.activeTabId]);
    expect(m.closedTabs).toEqual([]);
  });

  it("falls back to the first tab when the active tab's space is gone", () => {
    const m = restoreTabs({ ...stored, active: 1 }, [SPACE, OTHER_SPACE], counter());
    expect(m.activeTabId).toBe(m.tabs[0]?.id);
  });

  it("starts with the Home of the first space when nothing is stored, or nothing is left", () => {
    for (const none of [null, { tabs: [{ spaceId: "gone", pageId: "x", pinned: false }], active: 0 }]) {
      const m = restoreTabs(none, [OTHER_SPACE, SPACE], counter());
      expect(m.tabs).toHaveLength(1);
      expect(mirrorOf(m)).toEqual({ currentSpaceId: OTHER_SPACE, selectedPageId: null });
    }
    expect(restoreTabs(stored, [], counter())).toBe(EMPTY_MODEL);
  });
});

describe("the stored tab set", () => {
  it("reads the tab set it writes", () => {
    const m = model([page(1, { pinned: true }), tab("t2", [null], { space: OTHER_SPACE }), page(3)], "t3");
    const out = toStoredTabs(m);
    expect(out).toEqual({
      tabs: [
        { spaceId: SPACE, pageId: "p1", pinned: true },
        { spaceId: OTHER_SPACE, pageId: null, pinned: false },
        { spaceId: SPACE, pageId: "p3", pinned: false },
      ],
      active: 2,
    });
    expect(parseStoredTabs(out === null ? null : storedTabsToJson(out))).toEqual(out);
  });

  it("stores the current entry of a tab, not its history", () => {
    const m = model([tab("t1", ["a", "b", "c"], { index: 1 })]);
    expect(toStoredTabs(m)?.tabs).toEqual([{ spaceId: SPACE, pageId: "b", pinned: false }]);
  });

  it("has nothing to store without a tab", () => {
    expect(toStoredTabs(EMPTY_MODEL)).toBeNull();
  });

  it("anything that is not the expected shape means no stored tabs", () => {
    const values: (JsonValue | undefined)[] = [
      null,
      undefined,
      "x",
      3,
      true,
      [],
      {},
      { tabs: "x" },
      { tabs: {}, active: 0 },
      { tabs: [], active: 0 },
      { tabs: [1, "a", null], active: 0 },
    ];
    for (const value of values) {
      expect(parseStoredTabs(value), JSON.stringify(value)).toBeNull();
    }
  });

  it("skips a malformed tab", () => {
    const parsed = parseStoredTabs({
      tabs: [
        { spaceId: "s", pageId: "a", pinned: false },
        { spaceId: 1, pageId: "b", pinned: false },
        { spaceId: "s", pageId: 2, pinned: false },
        { spaceId: "s", pageId: "", pinned: false },
        { spaceId: "", pageId: "c", pinned: false },
        { spaceId: "s", pageId: "d", pinned: "yes" },
        { spaceId: "s", pinned: false },
        "nope",
        null,
        [],
        { spaceId: "s", pageId: null, pinned: true },
      ],
      active: 0,
    });
    expect(parsed).toEqual({
      tabs: [
        { spaceId: "s", pageId: null, pinned: true },
        { spaceId: "s", pageId: "a", pinned: false },
      ],
      active: 1,
    });
  });

  it("a page listed twice keeps its first place; a Home may be listed twice", () => {
    const parsed = parseStoredTabs({
      tabs: [
        { spaceId: "s", pageId: "a", pinned: false },
        { spaceId: "s", pageId: null, pinned: false },
        { spaceId: "s", pageId: "a", pinned: true },
        { spaceId: "s", pageId: null, pinned: false },
      ],
      active: 2,
    });
    expect(parsed?.tabs).toEqual([
      { spaceId: "s", pageId: "a", pinned: false },
      { spaceId: "s", pageId: null, pinned: false },
      { spaceId: "s", pageId: null, pinned: false },
    ]);
    expect(parsed?.active).toBe(0); // the active tab was the duplicate: its page is in the first tab
  });

  it("moves the pinned tabs first and keeps the active tab with its tab", () => {
    const parsed = parseStoredTabs({
      tabs: [
        { spaceId: "s", pageId: "a", pinned: false },
        { spaceId: "s", pageId: "b", pinned: true },
        { spaceId: "s", pageId: "c", pinned: false },
        { spaceId: "s", pageId: "d", pinned: true },
      ],
      active: 2,
    });
    expect(parsed?.tabs.map((t) => t.pageId)).toEqual(["b", "d", "a", "c"]);
    expect(parsed?.active).toBe(3);
  });

  it(`keeps at most ${MAX_TABS} tabs, the pinned ones first`, () => {
    const tabs = Array.from({ length: MAX_TABS + 5 }, (_, at) => ({ spaceId: "s", pageId: `p${at}`, pinned: at === MAX_TABS + 4 }));
    const parsed = parseStoredTabs({ tabs, active: 0 });
    expect(parsed?.tabs).toHaveLength(MAX_TABS);
    expect(parsed?.tabs[0]?.pageId).toBe(`p${MAX_TABS + 4}`);
    expect(parsed?.tabs.at(-1)?.pageId).toBe(`p${MAX_TABS - 2}`);
    expect(parsed?.active).toBe(1);
  });

  it("an out-of-range or unreadable active position means the first tab", () => {
    const tabs = [
      { spaceId: "s", pageId: "a", pinned: false },
      { spaceId: "s", pageId: "b", pinned: false },
    ];
    const unreadable: JsonValue[] = [2, 99, -1, 0.5, "1", null, Number.NaN];
    expect(parseStoredTabs({ tabs })?.active).toBe(0);
    for (const active of unreadable) {
      expect(parseStoredTabs({ tabs, active })?.active, JSON.stringify(active)).toBe(0);
    }
    expect(parseStoredTabs({ tabs, active: 1 })?.active).toBe(1);
  });
});
