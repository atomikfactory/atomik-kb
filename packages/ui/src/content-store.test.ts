// The content slice of the store (store-content.ts): references, backlinks, search,
// navigation into a page and external links, over the in-memory core.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import type { AppState } from "./store";
import { EMPTY_SEARCH, editedAfterOf, isExternalHref } from "./store-content";
import { paragraphs } from "./test/editor";
import { appendReference } from "./test/page";

afterEach(() => {
  vi.restoreAllMocks();
});

const SPACES = [
  {
    name: "Personal",
    pages: [{ title: "Target", content: paragraphs("Linked") }, { title: "Journal", content: paragraphs("Today") }],
  },
  { name: "Work", pages: [{ title: "Roadmap", content: paragraphs("Milestones") }] },
];

async function setup(options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({ workspaceName: "Research", spaces: SPACES, ...options });
  const requests: Request[] = [];
  let hold: Promise<void> | null = null;
  const transport: Transport = {
    dispatch: async (request) => {
      requests.push(request);
      if (hold !== null && request.type === "Search") {
        const held = hold;
        hold = null; // only the next search waits
        await held;
      }
      return memory.dispatch(request);
    },
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    openExternal: options.openExternal,
  };
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  const client = new CoreClient(memory);
  const ids = new Map<string, string>();
  for (const space of await client.spaceList()) {
    ids.set(space.name, space.id);
    for (const node of (await client.treeGet(space.id)).roots) {
      ids.set(node.page.title, node.page.id);
    }
  }
  const id = (name: string): string => ids.get(name) ?? "";
  return {
    memory,
    store,
    stop,
    requests,
    id,
    state: (): AppState => store.getState(),
    /** Makes the next search wait until the returned function is called. */
    holdNextSearch(): () => void {
      let release: () => void = () => undefined;
      hold = new Promise<void>((resolve) => {
        release = resolve;
      });
      return release;
    },
  };
}

describe("navigation", () => {
  it("opens a page of another space and remembers which block to show", async () => {
    const ctx = await setup();
    ctx.state().navigateTo({ pageId: ctx.id("Roadmap"), spaceId: ctx.id("Work"), blockId: "b-1" });
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.id("Work"), selectedPageId: ctx.id("Roadmap") });
    expect(ctx.state().scrollTarget).toMatchObject({ pageId: ctx.id("Roadmap"), blockId: "b-1" });
    await vi.waitFor(() => {
      expect(ctx.state().tree?.spaceId).toBe(ctx.id("Work"));
    });
    ctx.state().clearScrollTarget();
    expect(ctx.state().scrollTarget).toBeNull();
    // A page of the current space: no space switch, no block.
    ctx.state().navigateTo({ pageId: ctx.id("Roadmap") });
    expect(ctx.state().scrollTarget).toBeNull();
    ctx.stop();
  });
});

describe("referenced pages", () => {
  it("reads a page once and follows its renames", async () => {
    const ctx = await setup();
    ctx.state().lookupPage(ctx.id("Roadmap"));
    ctx.state().lookupPage(ctx.id("Roadmap"));
    expect(ctx.state().pageLookup[ctx.id("Roadmap")]).toEqual({ status: "loading" });
    await vi.waitFor(() => {
      expect(ctx.state().pageLookup[ctx.id("Roadmap")]).toMatchObject({ status: "found", page: { title: "Roadmap" } });
    });
    expect(ctx.requests.filter((request) => request.type === "PageGet")).toHaveLength(1);

    await ctx.memory.dispatch({ type: "PageRename", data: { pageId: ctx.id("Roadmap"), title: "Plan" } });
    await vi.waitFor(() => {
      expect(ctx.state().pageLookup[ctx.id("Roadmap")]).toMatchObject({ page: { title: "Plan" } });
    });
    ctx.stop();
  });

  it("knows purged pages as missing", async () => {
    const ctx = await setup();
    await ctx.memory.dispatch({ type: "PageTrash", data: { pageId: ctx.id("Roadmap") } });
    await ctx.memory.dispatch({ type: "PagePurge", data: { pageId: ctx.id("Roadmap") } });
    ctx.state().lookupPage(ctx.id("Roadmap"));
    await vi.waitFor(() => {
      expect(ctx.state().pageLookup[ctx.id("Roadmap")]).toEqual({ status: "missing" });
    });
    ctx.stop();
  });
});

describe("backlinks", () => {
  it("loads the open page's backlinks and refreshes them when they change", async () => {
    const ctx = await setup();
    ctx.state().loadBacklinks(ctx.id("Target"));
    await vi.waitFor(() => {
      expect(ctx.state().backlinks).toEqual({ pageId: ctx.id("Target"), status: "ready", items: [] });
    });
    ctx.memory.changePage(ctx.id("Journal"), (doc) => {
      appendReference(doc, "See ", ctx.id("Target"), "33333333-3333-4333-8333-333333333333");
    });
    await vi.waitFor(() => {
      expect(ctx.state().backlinks?.items).toEqual([
        expect.objectContaining({
          pageId: ctx.id("Journal"),
          title: "Journal",
          blockId: "33333333-3333-4333-8333-333333333333",
          text: "See Target",
        }),
      ]);
    });
    ctx.state().loadBacklinks(null);
    expect(ctx.state().backlinks).toBeNull();
    ctx.stop();
  });
});

describe("search", () => {
  it("lets the latest query win and clears on an empty one", async () => {
    const ctx = await setup();
    ctx.state().openSearch();
    expect(ctx.state().search.open).toBe(true);

    const release = ctx.holdNextSearch();
    const slow = ctx.state().runSearch("today");
    await ctx.state().runSearch("milestones");
    expect(ctx.state().search).toMatchObject({ query: "milestones", status: "ready" });
    release();
    await slow; // answers last, but asked first: dropped
    expect(ctx.state().search).toMatchObject({ query: "milestones", status: "ready" });
    expect(ctx.state().search.hits.map((hit) => hit.title)).toEqual(["Roadmap"]);

    await ctx.state().runSearch("   ");
    expect(ctx.state().search).toMatchObject({ status: "idle", hits: [] });
    ctx.state().closeSearch();
    expect(ctx.state().search).toEqual(EMPTY_SEARCH);
    ctx.stop();
  });

  it("offers pages for the [[ picker: titles across spaces, or the current space's pages", async () => {
    const ctx = await setup();
    expect((await ctx.state().findPages("road")).map((page) => page.title)).toEqual(["Roadmap"]);
    expect((await ctx.state().findPages("")).map((page) => page.title)).toEqual(["Target", "Journal"]);
    expect(ctx.requests.filter((request) => request.type === "Search").at(-1)).toEqual({
      type: "Search",
      data: { query: "road", scope: "Titles", limit: 8 },
    });
    ctx.stop();
  });
});

describe("search filter chips", () => {
  const DAY = 24 * 60 * 60 * 1000;
  const OLD = Date.UTC(2020, 0, 15, 12);

  /** The data of the last `Search` request the store sent. */
  const lastSearch = (ctx: Awaited<ReturnType<typeof setup>>) =>
    (ctx.requests.filter((request) => request.type === "Search").at(-1) as { data: Record<string, unknown> } | undefined)?.data;

  it("start at every space, any time and titles and bodies, and a plain request says so by saying nothing", async () => {
    const ctx = await setup();
    ctx.state().openSearch();
    expect(ctx.state().search).toMatchObject({ space: "all", edited: "any", titlesOnly: false });
    await ctx.state().runSearch("road");
    expect(lastSearch(ctx)).toEqual({ query: "road", limit: 30 });
    ctx.stop();
  });

  it("limit the request to the open space, to titles, and to pages edited since", async () => {
    const ctx = await setup({ now: () => OLD });
    ctx.state().openSearch();
    await ctx.state().runSearch("milestones");
    expect(ctx.state().search.hits.map((hit) => hit.title)).toEqual(["Roadmap"]);

    // The open space is the first one, Personal: Roadmap is in Work.
    ctx.state().setSearchFilters({ space: "current" });
    expect(ctx.state().search).toMatchObject({ space: "current", status: "loading", hits: [] });
    await ctx.state().runSearch("milestones");
    expect(lastSearch(ctx)).toEqual({ query: "milestones", limit: 30, spaceId: ctx.id("Personal") });
    expect(ctx.state().search.hits).toEqual([]);
    ctx.state().setSearchFilters({ space: "all", titlesOnly: true });
    await ctx.state().runSearch("milestones");
    expect(lastSearch(ctx)).toEqual({ query: "milestones", limit: 30, scope: "Titles" });
    expect(ctx.state().search.hits).toEqual([]);
    ctx.state().setSearchFilters({ titlesOnly: false });

    // The pages were last edited in 2020: "Past 7 days" leaves them out.
    ctx.state().setSearchFilters({ edited: "week" });
    const before = Date.now();
    await ctx.state().runSearch("milestones");
    const sent = lastSearch(ctx);
    expect(sent).toMatchObject({ query: "milestones", limit: 30 });
    expect(sent?.editedAfter).toBeGreaterThanOrEqual(before - 7 * DAY);
    expect(sent?.editedAfter).toBeLessThanOrEqual(Date.now() - 7 * DAY);
    expect(ctx.state().search.hits).toEqual([]);
    ctx.state().setSearchFilters({ edited: "any" });
    await ctx.state().runSearch("milestones");
    expect(ctx.state().search.hits.map((hit) => hit.title)).toEqual(["Roadmap"]);
    ctx.stop();
  });

  it("clear the results when they change, and closing the search brings them back to the defaults", async () => {
    const ctx = await setup();
    ctx.state().openSearch();
    await ctx.state().runSearch("road");
    expect(ctx.state().search.hits.length).toBeGreaterThan(0);
    ctx.state().setSearchFilters({ edited: "today" });
    expect(ctx.state().search).toMatchObject({ edited: "today", status: "loading", hits: [], query: "road" });
    // With nothing typed and no tag there is nothing to wait for.
    await ctx.state().runSearch("");
    ctx.state().setSearchFilters({ titlesOnly: true });
    expect(ctx.state().search).toMatchObject({ titlesOnly: true, status: "idle", hits: [] });
    ctx.state().closeSearch();
    expect(ctx.state().search).toEqual(EMPTY_SEARCH);
    ctx.stop();
  });

  it("work out the Edited chip: local midnight for today, else 7 or 30 days back from now", () => {
    const now = new Date(2026, 9, 14, 15, 30, 12);
    expect(editedAfterOf("any", now)).toBeUndefined();
    expect(editedAfterOf("today", now)).toBe(new Date(2026, 9, 14, 0, 0, 0, 0).getTime());
    expect(editedAfterOf("week", now)).toBe(now.getTime() - 7 * DAY);
    expect(editedAfterOf("month", now)).toBe(now.getTime() - 30 * DAY);
    // Just after midnight, today is only a moment long.
    const late = new Date(2026, 9, 14, 0, 0, 1);
    expect(late.getTime() - (editedAfterOf("today", late) ?? 0)).toBe(1000);
  });
});

describe("external links", () => {
  it("accepts http, https and mailto only", () => {
    expect(isExternalHref("https://example.com/a")).toBe(true);
    expect(isExternalHref("http://example.com")).toBe(true);
    expect(isExternalHref("mailto:me@example.com")).toBe(true);
    for (const href of ["javascript:alert(1)", "file:///etc/passwd", "tauri://localhost", "/relative", ""]) {
      expect(isExternalHref(href), href).toBe(false);
    }
  });

  it("opens through the shell when it can, else in a new tab, and never goes around a refusal", async () => {
    const opened = vi.spyOn(window, "open").mockImplementation(() => null);
    const viaShell = vi.fn(() => Promise.resolve());
    const shell = await setup({ openExternal: viaShell });
    await shell.state().openExternal("https://example.com");
    expect(viaShell).toHaveBeenCalledWith("https://example.com");
    expect(opened).not.toHaveBeenCalled();
    await shell.state().openExternal("javascript:alert(1)");
    expect(viaShell).toHaveBeenCalledTimes(1);
    shell.stop();

    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const refusing = await setup({ openExternal: () => Promise.reject(new Error("refused")) });
    await refusing.state().openExternal("https://example.com");
    expect(opened).not.toHaveBeenCalled();
    refusing.stop();

    const browser = await setup();
    await browser.state().openExternal("mailto:me@example.com");
    expect(opened).toHaveBeenCalledWith("mailto:me@example.com", "_blank", "noopener,noreferrer");
    browser.stop();
  });
});
