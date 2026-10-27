// The tabs in the store (ADR-0034): boot and restore, opening pages in the three ways, one tab per
// page, spaces, back and forward, closing and reopening, gone pages and spaces, `ui.tabs`, the lock,
// the recent pages and the scroll positions; over the in-memory core.

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue, PageNode, Request, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import type { AppState } from "./store";
import { RECENT_PAGES_KEY, parseRecentPages } from "./store-layout";
import { MAX_CLOSED_TABS, MAX_TABS, TABS_KEY, currentEntry, selectCanGoBack, selectCanGoForward } from "./store-tabs";
import type { TabEntry } from "./store-tabs";

const PASSPHRASE = "correct horse battery";

const PERSONAL = {
  name: "Personal",
  pages: [{ title: "Projects", children: [{ title: "Roadmap" }, { title: "Ideas" }] }, { title: "Journal" }, { title: "Notes" }],
};
const WORK = { name: "Work", pages: [{ title: "Meetings" }, { title: "Plans" }] };

type SpaceName = "Personal" | "Work";
type PageTitle = "Projects" | "Roadmap" | "Ideas" | "Journal" | "Notes" | "Meetings" | "Plans";

interface Ids {
  space: Readonly<Record<SpaceName, string>>;
  page: Readonly<Record<PageTitle, string>>;
}

async function readIds(client: CoreClient): Promise<Ids> {
  const space: Record<string, string> = {};
  const page: Record<string, string> = {};
  const visit = (nodes: readonly PageNode[]): void => {
    for (const node of nodes) {
      page[node.page.title] = node.page.id;
      visit(node.children);
    }
  };
  for (const info of await client.spaceList()) {
    space[info.name] = info.id;
    visit((await client.treeGet(info.id)).roots);
  }
  return { space: space as Record<SpaceName, string>, page: page as Record<PageTitle, string> };
}

interface SetupOptions {
  /** `ui.tabs` as it was stored before the launch, made from the ids of the seeded workspace. */
  stored?: (ids: Ids) => JsonValue;
}

async function setup(options: SetupOptions = {}) {
  const memory = new MemoryTransport({
    workspaceName: "Research",
    spaces: [PERSONAL, WORK],
    security: { kitCreatedAt: Date.UTC(2026, 9, 3), appLockPassphrase: PASSPHRASE },
  });
  const direct = new CoreClient(memory);
  const ids = await readIds(direct);
  if (options.stored !== undefined) {
    await direct.settingsSet(TABS_KEY, options.stored(ids));
  }
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return memory.dispatch(request);
    },
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    lock: () => memory.lock(),
    unlock: (passphrase) => memory.unlock(passphrase),
  };
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(
    () => {
      expect(store.getState().phase).toBe("ready");
    },
    { timeout: 4000 },
  );
  const titles = new Map<string, string>(Object.entries(ids.page).map(([title, id]) => [id, title]));
  const spaceNames = new Map<string, string>(Object.entries(ids.space).map(([name, id]) => [id, name]));
  const state = (): AppState => store.getState();
  const label = (entry: TabEntry): string =>
    entry.pageId === null ? `Home(${spaceNames.get(entry.spaceId) ?? "?"})` : (titles.get(entry.pageId) ?? entry.pageId);
  return {
    memory,
    direct,
    ids,
    store,
    stop,
    requests,
    state,
    count: (type: Request["type"]): number => requests.filter((request) => request.type === type).length,
    tabWrites: (): Request[] =>
      requests.filter((request) => request.type === "SettingsSet" && request.data.key === TABS_KEY),
    /** The strip as text: the page each tab shows (`Home(Space)` for a Home), `*` after a pinned one, `^` before the active one. */
    strip: (): string[] =>
      state().tabs.map(
        (tab) => `${tab.id === state().activeTabId ? "^" : ""}${label(currentEntry(tab))}${tab.pinned ? "*" : ""}`,
      ),
    /** The active tab's history as text; the current entry in brackets. */
    history: (): string[] => {
      const active = state().tabs.find((tab) => tab.id === state().activeTabId);
      return (active?.entries ?? []).map((entry, at) => (at === active?.index ? `[${label(entry)}]` : label(entry)));
    },
    tabOf: (title: PageTitle) => state().tabs.find((tab) => currentEntry(tab).pageId === ids.page[title]),
    settled: (check: (state: AppState) => boolean) =>
      vi.waitFor(
        () => {
          expect(check(store.getState())).toBe(true);
        },
        { timeout: 4000 },
      ),
    /** Lets what is under way (lookups, tree reads, writes) finish. */
    quiet: () => new Promise<void>((resolve) => setTimeout(resolve, 40)),
    recents: (): string[] =>
      parseRecentPages(state().settings[RECENT_PAGES_KEY]).map((entry) => titles.get(entry.pageId) ?? entry.pageId),
  };
}

type Ctx = Awaited<ReturnType<typeof setup>>;

/** The page ids of the seeded workspace, for `navigateTo`. */
const open = (ctx: Ctx, title: PageTitle, disposition?: "current" | "background" | "foreground", blockId?: string): void => {
  const pageId = ctx.ids.page[title] ?? "";
  const spaceId = ["Meetings", "Plans"].includes(title) ? ctx.ids.space.Work : ctx.ids.space.Personal;
  ctx.state().navigateTo({ pageId, spaceId, disposition, blockId });
};

describe("boot", () => {
  it("starts with one Home tab of the first space and writes nothing for it", async () => {
    const ctx = await setup();
    expect(ctx.strip()).toEqual(["^Home(Personal)"]);
    expect(ctx.state()).toMatchObject({
      currentSpaceId: ctx.ids.space.Personal,
      selectedPageId: null,
      closedTabs: [],
    });
    expect(ctx.state().tabs[0]?.entries).toHaveLength(1);
    expect(ctx.state().settings[TABS_KEY]).toBeNull(); // read, and not set
    await ctx.quiet();
    expect(ctx.tabWrites()).toEqual([]);
    expect(ctx.count("TreeGet")).toBe(1);
  });

  it("restores the stored tabs in place, each with one entry, and loads the tree of the active tab's space", async () => {
    const ctx = await setup({
      stored: ({ space, page }) => ({
        tabs: [
          { spaceId: space.Work, pageId: page.Plans, pinned: true },
          { spaceId: space.Personal, pageId: page.Roadmap, pinned: false },
          { spaceId: space.Work, pageId: page.Meetings, pinned: false },
          { spaceId: space.Personal, pageId: null, pinned: false },
        ],
        active: 2,
      }),
    });
    expect(ctx.strip()).toEqual(["Plans*", "Roadmap", "^Meetings", "Home(Personal)"]);
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Work, selectedPageId: ctx.ids.page.Meetings });
    expect(ctx.state().tabs.every((tab) => tab.entries.length === 1 && tab.index === 0)).toBe(true);
    expect(ctx.state().tree?.spaceId).toBe(ctx.ids.space.Work);
    // Only the active tab's tree is read, once; the page lookups are for the pages that are not in it.
    expect(ctx.count("TreeGet")).toBe(1);
    expect(ctx.state().settings[TABS_KEY]).toMatchObject({ active: 2 });
    expect(selectCanGoBack(ctx.state())).toBe(false);
    expect(selectCanGoForward(ctx.state())).toBe(false);
    await ctx.quiet();
    expect(ctx.tabWrites()).toEqual([]); // what the setting already says is not written again
    ctx.stop();
  });

  it("drops the stored tabs of spaces that no longer exist, and of pages that are gone", async () => {
    const ctx = await setup({
      stored: ({ space, page }) => ({
        tabs: [
          { spaceId: "a-space-that-was-deleted", pageId: "a-page", pinned: false },
          { spaceId: space.Personal, pageId: page.Journal, pinned: false },
          { spaceId: space.Personal, pageId: "9d3a4c3e-5b7c-4f68-9a52-2d1c3a1e0f11", pinned: false },
          { spaceId: space.Work, pageId: page.Plans, pinned: false },
        ],
        active: 1,
      }),
    });
    // The unknown page is looked up and found missing; the deleted space was dropped at once.
    await ctx.settled((s) => s.tabs.length === 2);
    expect(ctx.strip()).toEqual(["^Journal", "Plans"]);
    expect(ctx.state().selectedPageId).toBe(ctx.ids.page.Journal);
    // The pruned set is stored.
    await vi.waitFor(async () => {
      await expect(ctx.direct.settingsGet(TABS_KEY)).resolves.toMatchObject({
        tabs: [{ pageId: ctx.ids.page.Journal }, { pageId: ctx.ids.page.Plans }],
        active: 0,
      });
    });
  });

  it("a stored page that was trashed meanwhile is dropped", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL, WORK] });
    const direct = new CoreClient(memory);
    const ids = await readIds(direct);
    await direct.pageTrash(ids.page.Journal ?? "");
    await direct.settingsSet(TABS_KEY, {
      tabs: [
        { spaceId: ids.space.Personal, pageId: ids.page.Notes, pinned: false },
        { spaceId: ids.space.Personal, pageId: ids.page.Journal, pinned: false },
      ],
      active: 1,
    });
    const store = createAppStore(new CoreClient(memory), { corePollMs: 5 });
    store.getState().start();
    await vi.waitFor(() => {
      expect(store.getState().tabs).toHaveLength(1);
    });
    expect(store.getState().selectedPageId).toBe(ids.page.Notes);
  });

  it("treats a setting that is not a tab set as no stored tabs", async () => {
    const ctx = await setup({ stored: () => "garbage" });
    expect(ctx.strip()).toEqual(["^Home(Personal)"]);
  });
});

describe("opening a page", () => {
  it("a plain open navigates the active tab, and back and forward walk its history", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    open(ctx, "Notes");
    expect(ctx.strip()).toEqual(["^Notes"]);
    expect(ctx.history()).toEqual(["Home(Personal)", "Journal", "[Notes]"]);
    expect(ctx.state()).toMatchObject({ selectedPageId: ctx.ids.page.Notes, route: "home" });

    ctx.state().goBack();
    expect(ctx.history()).toEqual(["Home(Personal)", "[Journal]", "Notes"]);
    expect(ctx.state().selectedPageId).toBe(ctx.ids.page.Journal);
    expect(selectCanGoBack(ctx.state())).toBe(true);
    expect(selectCanGoForward(ctx.state())).toBe(true);

    open(ctx, "Projects"); // drops the forward entry
    expect(ctx.history()).toEqual(["Home(Personal)", "Journal", "[Projects]"]);
    ctx.state().goForward(); // at the end: nothing
    expect(ctx.history()).toEqual(["Home(Personal)", "Journal", "[Projects]"]);
    ctx.state().goBack();
    ctx.state().goBack();
    ctx.state().goBack(); // at the start: nothing
    expect(ctx.state().selectedPageId).toBeNull();
    expect(selectCanGoBack(ctx.state())).toBe(false);
  });

  it("the page the active tab shows again only scrolls: no entry, a scroll target", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    const seq = ctx.state().scrollTarget?.seq ?? 0;
    expect(ctx.state().scrollTarget).toBeNull();
    open(ctx, "Journal", "current", "block-1");
    expect(ctx.history()).toEqual(["Home(Personal)", "[Journal]"]);
    expect(ctx.state().scrollTarget).toMatchObject({ pageId: ctx.ids.page.Journal, blockId: "block-1" });
    expect(ctx.state().scrollTarget?.seq).toBeGreaterThan(seq);
    const first = ctx.state().scrollTarget?.seq ?? 0;
    open(ctx, "Journal", "current", "block-1");
    expect(ctx.state().scrollTarget?.seq).toBeGreaterThan(first); // a second request for the same block is told apart
    open(ctx, "Notes");
    expect(ctx.state().scrollTarget).toBeNull();
  });

  it("selectPage is a plain open in the current space; selectPage(null) is the Home of the space", async () => {
    const ctx = await setup();
    ctx.state().selectPage(ctx.ids.page.Journal ?? "");
    expect(ctx.history()).toEqual(["Home(Personal)", "[Journal]"]);
    ctx.state().selectPage(null);
    expect(ctx.history()).toEqual(["Home(Personal)", "Journal", "[Home(Personal)]"]);
    expect(ctx.state().selectedPageId).toBeNull();
    ctx.state().selectPage(null); // already there
    expect(ctx.history()).toEqual(["Home(Personal)", "Journal", "[Home(Personal)]"]);
  });

  it("a background open adds a tab after the active one, in order, and the active tab stays", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    open(ctx, "Notes", "foreground"); // [Journal, ^Notes]
    open(ctx, "Projects", "background");
    open(ctx, "Roadmap", "background");
    open(ctx, "Ideas", "background");
    expect(ctx.strip()).toEqual(["Journal", "^Notes", "Projects", "Roadmap", "Ideas"]);
    expect(ctx.state().selectedPageId).toBe(ctx.ids.page.Notes);
    // Another tab in front starts a new line.
    ctx.state().activateTab(ctx.tabOf("Journal")?.id ?? "");
    open(ctx, "Plans", "background");
    expect(ctx.strip()).toEqual(["^Journal", "Plans", "Notes", "Projects", "Roadmap", "Ideas"]);
  });

  it("a background open records nothing in the recent pages until the tab is shown", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    expect(ctx.recents()).toEqual(["Journal"]);
    open(ctx, "Notes", "background");
    expect(ctx.recents()).toEqual(["Journal"]);
    ctx.state().activateTab(ctx.tabOf("Notes")?.id ?? "");
    expect(ctx.recents()).toEqual(["Notes", "Journal"]);
  });

  it("a background open with a block scrolls to it the first time the tab shows, and not the second", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "background", "block-7");
    expect(ctx.state().scrollTarget).toBeNull();
    const tab = ctx.tabOf("Journal");
    expect(tab && currentEntry(tab).blockId).toBe("block-7");
    ctx.state().activateTab(tab?.id ?? "");
    expect(ctx.state().scrollTarget).toMatchObject({ pageId: ctx.ids.page.Journal, blockId: "block-7" });
    const shown = ctx.tabOf("Journal");
    expect(shown && currentEntry(shown).blockId).toBeUndefined();
    ctx.state().clearScrollTarget();
    ctx.state().activateTab(ctx.state().tabs[0]?.id ?? "");
    ctx.state().activateTab(tab?.id ?? "");
    expect(ctx.state().scrollTarget).toBeNull();
  });

  it("a foreground open adds a tab and shows it, and records it", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    expect(ctx.strip()).toEqual(["Home(Personal)", "^Journal"]);
    expect(ctx.state().selectedPageId).toBe(ctx.ids.page.Journal);
    expect(ctx.recents()).toEqual(["Journal"]);
  });

  it("a page that is open in a tab is never opened twice: that tab comes to the front", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    open(ctx, "Notes", "foreground");
    ctx.state().navigate("settings");
    open(ctx, "Journal"); // a plain click, the page is in another tab
    expect(ctx.strip()).toEqual(["Home(Personal)", "^Journal", "Notes"]);
    expect(ctx.history()).toEqual(["[Journal]"]);
    expect(ctx.state().route).toBe("home");
    open(ctx, "Notes", "background"); // does nothing at all
    expect(ctx.strip()).toEqual(["Home(Personal)", "^Journal", "Notes"]);
    open(ctx, "Notes", "foreground", "block-3"); // comes to the front and applies the block
    expect(ctx.strip()).toEqual(["Home(Personal)", "Journal", "^Notes"]);
    expect(ctx.state().scrollTarget).toMatchObject({ pageId: ctx.ids.page.Notes, blockId: "block-3" });
  });

  it("back and forward may land on a page that another tab shows", async () => {
    const ctx = await setup();
    open(ctx, "Notes");
    open(ctx, "Projects"); // first tab: Home, Notes, Projects
    open(ctx, "Notes", "foreground"); // Notes is not what the first tab shows now: a second tab on it
    ctx.state().activateTab(ctx.state().tabs[0]?.id ?? "");
    ctx.state().goBack();
    expect(ctx.strip()).toEqual(["^Notes", "Notes"]);
    expect(ctx.history()).toEqual(["Home(Personal)", "[Notes]", "Projects"]);
    open(ctx, "Notes"); // the active tab is one of the two: it stays where it is
    expect(ctx.history()).toEqual(["Home(Personal)", "[Notes]", "Projects"]);
    expect(ctx.strip()).toEqual(["^Notes", "Notes"]);
  });

  it("a pinned active tab keeps its page: a plain open makes a tab in front", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    ctx.state().setTabPinned(ctx.state().activeTabId ?? "", true);
    open(ctx, "Notes");
    expect(ctx.strip()).toEqual(["Journal*", "^Notes"]);
    // A background tab never goes among the pinned tabs.
    ctx.state().activateTab(ctx.tabOf("Journal")?.id ?? "");
    open(ctx, "Projects", "background");
    expect(ctx.strip()).toEqual(["^Journal*", "Projects", "Notes"]);
  });

  it(`at ${MAX_TABS} tabs a new tab is refused and background and foreground opens navigate the active tab`, async () => {
    const ctx = await setup();
    for (let n = 1; n < MAX_TABS; n += 1) {
      ctx.state().newTab();
    }
    expect(ctx.state().tabs).toHaveLength(MAX_TABS);
    ctx.state().newTab();
    expect(ctx.state().tabs).toHaveLength(MAX_TABS);
    open(ctx, "Journal", "background");
    open(ctx, "Notes", "foreground");
    expect(ctx.state().tabs).toHaveLength(MAX_TABS);
    expect(ctx.history()).toEqual(["Home(Personal)", "Journal", "[Notes]"]);
  });

  it("a new tab is the Home of the current space, last, and in front", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    ctx.state().newTab();
    expect(ctx.strip()).toEqual(["Journal", "^Home(Personal)"]);
    expect(ctx.state().selectedPageId).toBeNull();
    ctx.state().activateTabAt(0);
    ctx.state().newTab();
    expect(ctx.strip()).toEqual(["Journal", "Home(Personal)", "^Home(Personal)"]);
  });

  it("createPage opens the new page as a plain open, and a page just made is not taken for a gone one", async () => {
    const ctx = await setup();
    const page = await ctx.state().createPage({ title: "Fresh" });
    expect(ctx.state().selectedPageId).toBe(page.id);
    expect(ctx.history()).toEqual(["Home(Personal)", `[${page.id}]`]);
    await ctx.settled((s) => s.tree?.roots.some((node) => node.page.id === page.id) === true);
    await ctx.quiet();
    expect(ctx.state().selectedPageId).toBe(page.id);
    expect(ctx.recents()[0]).toBe(page.id);
    const hidden = await ctx.state().createPage({ select: false });
    expect(ctx.state().selectedPageId).toBe(page.id);
    expect(ctx.state().tabs).toHaveLength(1);
    expect(hidden.id).not.toBe(page.id);
  });

  it("withdraws a request to edit a title when another page comes to the front", async () => {
    const ctx = await setup();
    ctx.state().focusPageTitle(ctx.ids.page.Notes ?? "");
    open(ctx, "Notes");
    expect(ctx.state().titleFocusPageId).toBe(ctx.ids.page.Notes);
    open(ctx, "Journal");
    expect(ctx.state().titleFocusPageId).toBeNull();
  });
});

describe("spaces", () => {
  it("navigating to a page of another space switches the space and loads its tree", async () => {
    const ctx = await setup();
    open(ctx, "Meetings");
    expect(ctx.state()).toMatchObject({
      currentSpaceId: ctx.ids.space.Work,
      selectedPageId: ctx.ids.page.Meetings,
      tree: null,
      route: "home",
    });
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Work);
    expect(ctx.history()).toEqual(["Home(Personal)", "[Meetings]"]);
  });

  it("activating a tab of another space switches the sidebar to that space", async () => {
    const ctx = await setup();
    open(ctx, "Meetings", "foreground");
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Work);
    ctx.state().activateTab(ctx.state().tabs[0]?.id ?? "");
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Personal, tree: null, selectedPageId: null });
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
  });

  it("keyboard cycling, closing the front tab and back/forward follow the space of the tab that comes to the front", async () => {
    const ctx = await setup();
    open(ctx, "Meetings", "foreground");
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Work);
    ctx.state().activateAdjacentTab(1);
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Personal, selectedPageId: null });
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
    ctx.state().activateAdjacentTab(-1);
    expect(ctx.state().currentSpaceId).toBe(ctx.ids.space.Work);
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Work);
    // Closing the front tab (Work) activates the other one, in Personal; the other tab stays.
    ctx.state().closeTab(ctx.state().activeTabId ?? "");
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Personal, tree: null });
    expect(ctx.state().tabs).toHaveLength(1);
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
    // Back and forward inside one tab across spaces.
    open(ctx, "Plans");
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Work);
    ctx.state().goBack();
    expect(ctx.state().currentSpaceId).toBe(ctx.ids.space.Personal);
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
    ctx.state().goForward();
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Work, selectedPageId: ctx.ids.page.Plans });
    expect(ctx.state().tabs).toHaveLength(1);
  });

  it("the space switcher brings the most recently activated tab of that space to the front", async () => {
    const ctx = await setup();
    open(ctx, "Meetings", "foreground");
    open(ctx, "Plans", "foreground");
    ctx.state().activateTab(ctx.tabOf("Meetings")?.id ?? "");
    ctx.state().selectSpace(ctx.ids.space.Personal ?? ""); // the Home tab
    expect(ctx.strip()).toEqual(["^Home(Personal)", "Meetings", "Plans"]);
    ctx.state().selectSpace(ctx.ids.space.Work ?? ""); // Meetings was activated after Plans
    expect(ctx.strip()).toEqual(["Home(Personal)", "^Meetings", "Plans"]);
    expect(ctx.history()).toEqual(["[Meetings]"]); // no entry was added
  });

  it("with no tab in that space the active tab goes to its Home, and a pinned one leaves a new tab", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    ctx.state().selectSpace(ctx.ids.space.Work ?? "");
    expect(ctx.history()).toEqual(["Home(Personal)", "Journal", "[Home(Work)]"]);
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Work, selectedPageId: null, tree: null });
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Work);

    ctx.state().goBack(); // back to the Journal of Personal
    expect(ctx.state().currentSpaceId).toBe(ctx.ids.space.Personal);
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
    ctx.state().setTabPinned(ctx.state().activeTabId ?? "", true);
    ctx.state().selectSpace(ctx.ids.space.Work ?? "");
    expect(ctx.strip()).toEqual(["Journal*", "^Home(Work)"]);
  });

  it("choosing the current space again only shows the main screen", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    ctx.state().navigate("settings");
    const before = ctx.count("TreeGet");
    ctx.state().selectSpace(ctx.ids.space.Personal ?? "");
    expect(ctx.state().route).toBe("home");
    expect(ctx.history()).toEqual(["Home(Personal)", "[Journal]"]);
    expect(ctx.count("TreeGet")).toBe(before);
  });

  it("a new space is a place to go to: the active tab navigates to its Home", async () => {
    const ctx = await setup();
    const space = await ctx.state().createSpace("Play");
    expect(ctx.state().currentSpaceId).toBe(space.id);
    expect(ctx.state().tabs).toHaveLength(1);
    expect(ctx.history()).toEqual(["Home(Personal)", "[Home(?)]"]);
    await ctx.settled((s) => s.tree?.spaceId === space.id);
    await ctx.quiet();
    expect(ctx.state().currentSpaceId).toBe(space.id); // the space is not taken for a deleted one while the list catches up
    expect(ctx.state().tabs).toHaveLength(1);
  });

  it("a deleted space takes its entries with it: a tab left empty closes, the others show what came before", async () => {
    const ctx = await setup();
    const extra = await ctx.state().createSpace("Extra");
    await ctx.settled((s) => s.spaces.length === 3 && s.tree?.spaceId === extra.id);
    open(ctx, "Journal", "foreground");
    ctx.state().selectSpace(extra.id); // the first tab, which went to the Home of Extra
    ctx.state().newTab(); // and a tab with nothing but the Home of Extra
    await ctx.settled((s) => s.tree?.spaceId === extra.id);
    expect(ctx.state().tabs).toHaveLength(3);
    await ctx.state().deleteSpace(extra.id);
    await ctx.settled((s) => s.spaces.length === 2 && s.tabs.length === 2);
    expect(ctx.strip()).toEqual(["Home(Personal)", "^Journal"]);
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Personal, selectedPageId: ctx.ids.page.Journal });
    expect(ctx.state().closedTabs).toEqual([]);
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
    expect(ctx.state().tabs[0]?.entries).toHaveLength(1);
  });
});

describe("closing and reopening", () => {
  it("closing the active tab brings the tab to its right (else its left) to the front", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    open(ctx, "Notes", "foreground");
    open(ctx, "Projects", "foreground");
    expect(ctx.strip()).toEqual(["Home(Personal)", "Journal", "Notes", "^Projects"]);
    ctx.state().closeTab(ctx.tabOf("Projects")?.id ?? "");
    expect(ctx.strip()).toEqual(["Home(Personal)", "Journal", "^Notes"]);
    ctx.state().activateTabAt(1);
    ctx.state().closeTab(ctx.tabOf("Journal")?.id ?? "");
    expect(ctx.strip()).toEqual(["Home(Personal)", "^Notes"]);
    expect(ctx.state().selectedPageId).toBe(ctx.ids.page.Notes);
  });

  it("closing a tab that is not in front leaves the page and the route alone", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    ctx.state().navigate("settings");
    ctx.state().closeTab(ctx.state().tabs[0]?.id ?? "");
    expect(ctx.strip()).toEqual(["^Journal"]);
    expect(ctx.state().route).toBe("settings");
  });

  it("closing the last tab leaves a Home tab of the current space", async () => {
    const ctx = await setup();
    open(ctx, "Meetings");
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Work);
    ctx.state().closeTab(ctx.state().activeTabId ?? "");
    expect(ctx.strip()).toEqual(["^Home(Work)"]);
    expect(ctx.state()).toMatchObject({ currentSpaceId: ctx.ids.space.Work, selectedPageId: null });
    expect(ctx.state().closedTabs).toHaveLength(1);
  });

  it("reopens the last closed tab with its history and position, and brings it to the front", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    open(ctx, "Notes");
    ctx.state().goBack(); // Home, [Journal], Notes
    open(ctx, "Projects", "foreground");
    const closing = ctx.tabOf("Journal");
    ctx.state().closeTab(closing?.id ?? "");
    expect(ctx.strip()).toEqual(["^Projects"]);
    ctx.state().reopenClosedTab();
    expect(ctx.strip()).toEqual(["^Journal", "Projects"]);
    expect(ctx.history()).toEqual(["Home(Personal)", "[Journal]", "Notes"]);
    expect(ctx.state().closedTabs).toEqual([]);
    ctx.state().reopenClosedTab(); // nothing left
    expect(ctx.strip()).toEqual(["^Journal", "Projects"]);
  });

  it("keeps a page to one tab when it is reopened: the tab that has it comes to the front instead", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    ctx.state().closeTab(ctx.tabOf("Journal")?.id ?? "");
    open(ctx, "Journal", "background");
    ctx.state().reopenClosedTab();
    expect(ctx.strip()).toEqual(["Home(Personal)", "^Journal"]);
    expect(ctx.state().closedTabs).toEqual([]);
  });

  it(`keeps at most ${MAX_CLOSED_TABS} closed tabs, and drops the scroll positions of those that cannot come back`, async () => {
    const ctx = await setup();
    const entries: string[] = [];
    for (let n = 0; n < MAX_CLOSED_TABS + 1; n += 1) {
      ctx.state().newTab();
      const tab = ctx.state().tabs.find((candidate) => candidate.id === ctx.state().activeTabId);
      const entry = tab === undefined ? undefined : currentEntry(tab);
      entries.push(entry?.id ?? "");
      ctx.state().saveScroll(entry?.id ?? "", 100 + n);
    }
    for (const tab of ctx.state().tabs.slice(1)) {
      ctx.state().closeTab(tab.id);
    }
    expect(ctx.state().closedTabs).toHaveLength(MAX_CLOSED_TABS);
    expect(ctx.state().savedScroll(entries[0] ?? "")).toBeUndefined();
    expect(ctx.state().savedScroll(entries[1] ?? "")).toBe(101);
  });

  it("close others and close to the right keep the pinned tabs and can be reopened one by one", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    ctx.state().setTabPinned(ctx.state().activeTabId ?? "", true);
    open(ctx, "Notes", "foreground");
    open(ctx, "Projects", "foreground");
    open(ctx, "Plans", "foreground");
    expect(ctx.strip()).toEqual(["Journal*", "Notes", "Projects", "^Plans", "Home(Personal)"]);
    ctx.state().closeOtherTabs(ctx.tabOf("Notes")?.id ?? "");
    expect(ctx.strip()).toEqual(["Journal*", "^Notes"]);
    expect(ctx.state().closedTabs).toHaveLength(3);
    for (let n = 0; n < 3; n += 1) {
      ctx.state().reopenClosedTab();
    }
    // Each one comes back where it was, the one closed last first.
    expect(ctx.strip()).toEqual(["Journal*", "Notes", "^Projects", "Plans", "Home(Personal)"]);
    ctx.state().closeTabsToRight(ctx.tabOf("Notes")?.id ?? "");
    expect(ctx.strip()).toEqual(["Journal*", "^Notes"]);
    ctx.state().closeTabsToRight(ctx.tabOf("Journal")?.id ?? ""); // the unpinned tab to its right goes
    expect(ctx.strip()).toEqual(["^Journal*"]);
  });

  it("moves tabs inside their group and pins and unpins them", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    open(ctx, "Notes", "foreground");
    open(ctx, "Projects", "foreground");
    ctx.state().moveTab(ctx.tabOf("Projects")?.id ?? "", 0);
    expect(ctx.strip()).toEqual(["^Projects", "Home(Personal)", "Journal", "Notes"]);
    ctx.state().setTabPinned(ctx.tabOf("Journal")?.id ?? "", true);
    expect(ctx.strip()).toEqual(["Journal*", "^Projects", "Home(Personal)", "Notes"]);
    ctx.state().moveTab(ctx.tabOf("Journal")?.id ?? "", 3); // not among the others
    expect(ctx.strip()).toEqual(["Journal*", "^Projects", "Home(Personal)", "Notes"]);
    ctx.state().setTabPinned(ctx.tabOf("Journal")?.id ?? "", false);
    expect(ctx.strip()).toEqual(["Journal", "^Projects", "Home(Personal)", "Notes"]);
    expect(ctx.state().activeTabId).toBe(ctx.tabOf("Projects")?.id);
  });

  it("next and previous tab wrap, by position or by step", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    open(ctx, "Notes", "foreground");
    ctx.state().activateAdjacentTab(1);
    expect(ctx.strip()[0]).toBe("^Home(Personal)");
    ctx.state().activateAdjacentTab(-1);
    expect(ctx.strip()[2]).toBe("^Notes");
    ctx.state().activateTabAt(1);
    expect(ctx.strip()[1]).toBe("^Journal");
    ctx.state().activateTabAt(-1);
    expect(ctx.strip()[2]).toBe("^Notes");
    ctx.state().activateTabAt(7);
    expect(ctx.strip()[2]).toBe("^Notes");
  });
});

describe("gone pages", () => {
  it("trashing the open page returns the tab to the page it was opened from", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    open(ctx, "Notes");
    await ctx.state().trashPage(ctx.ids.page.Notes ?? "");
    await ctx.settled((s) => s.selectedPageId === ctx.ids.page.Journal);
    expect(ctx.history()).toEqual(["Home(Personal)", "[Journal]"]);
    ctx.state().goForward(); // the entry that went is not forward either
    expect(ctx.history()).toEqual(["Home(Personal)", "[Journal]"]);
  });

  it("removes the page from every history and closes the tabs it leaves empty", async () => {
    const ctx = await setup();
    open(ctx, "Projects");
    open(ctx, "Notes"); // first tab: Home, Projects, Notes
    open(ctx, "Projects", "foreground"); // Projects is not what the first tab shows now: a second tab
    expect(ctx.strip()).toEqual(["Notes", "^Projects"]);
    await ctx.state().trashPage(ctx.ids.page.Projects ?? "");
    await ctx.settled((s) => s.tabs.length === 1);
    expect(ctx.strip()).toEqual(["^Notes"]);
    expect(ctx.history()).toEqual(["Home(Personal)", "[Notes]"]);
    expect(ctx.state().closedTabs).toEqual([]);
  });

  it("a page nested under a trashed page goes too", async () => {
    const ctx = await setup();
    open(ctx, "Roadmap", "foreground");
    await ctx.state().trashPage(ctx.ids.page.Projects ?? "");
    await ctx.settled((s) => s.tabs.length === 1);
    expect(ctx.strip()).toEqual(["^Home(Personal)"]);
  });

  it("does not reopen the tab when the trash is undone", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    await ctx.state().trashPage(ctx.ids.page.Journal ?? "");
    await ctx.settled((s) => s.tabs.length === 1);
    await ctx.state().restorePage(ctx.ids.page.Journal ?? "");
    await ctx.settled((s) => s.tree?.roots.some((node) => node.page.title === "Journal") === true);
    await ctx.quiet();
    expect(ctx.strip()).toEqual(["^Home(Personal)"]);
  });

  it("a page of another space that was trashed or purged is found through the lookups", async () => {
    const ctx = await setup();
    open(ctx, "Meetings", "foreground");
    open(ctx, "Plans", "foreground");
    ctx.state().activateTab(ctx.state().tabs[0]?.id ?? "");
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
    // The strip needs the title and the icon of pages that are not in the tree.
    await ctx.settled((s) => s.pageLookup[ctx.ids.page.Meetings ?? ""]?.status === "found" && s.pageLookup[ctx.ids.page.Plans ?? ""]?.status === "found");
    await ctx.memory.dispatch({ type: "PageTrash", data: { pageId: ctx.ids.page.Meetings ?? "" } });
    await ctx.settled((s) => s.tabs.length === 2);
    expect(ctx.strip()).toEqual(["^Home(Personal)", "Plans"]);
    await ctx.memory.dispatch({ type: "PageTrash", data: { pageId: ctx.ids.page.Plans ?? "" } });
    await ctx.memory.dispatch({ type: "PagePurge", data: { pageId: ctx.ids.page.Plans ?? "" } });
    await ctx.settled((s) => s.tabs.length === 1);
    expect(ctx.strip()).toEqual(["^Home(Personal)"]);
  });

  it("looks up only the tab pages that are not in the tree of the current space, once", async () => {
    const ctx = await setup({
      stored: ({ space, page }) => ({
        tabs: [
          { spaceId: space.Personal, pageId: page.Journal, pinned: false },
          { spaceId: space.Work, pageId: page.Meetings, pinned: false },
        ],
        active: 0,
      }),
    });
    await ctx.settled((s) => s.pageLookup[ctx.ids.page.Meetings ?? ""]?.status === "found");
    await ctx.quiet();
    const looked = ctx.requests.filter((request) => request.type === "PageGet").map((request) => request.data.pageId);
    expect(looked).toEqual([ctx.ids.page.Meetings]);
    expect(ctx.state().pageLookup[ctx.ids.page.Journal ?? ""]).toBeUndefined();
    // And it settles: nothing more is asked while nothing changes.
    const before = ctx.requests.length;
    await ctx.quiet();
    expect(ctx.requests.length).toBe(before);
  });

  it("a Home tab appears when the last page of the last tab goes", async () => {
    const ctx = await setup({
      stored: ({ space, page }) => ({ tabs: [{ spaceId: space.Work, pageId: page.Meetings, pinned: false }], active: 0 }),
    });
    expect(ctx.strip()).toEqual(["^Meetings"]);
    await ctx.state().trashPage(ctx.ids.page.Meetings ?? "");
    await ctx.settled((s) => s.selectedPageId === null);
    expect(ctx.strip()).toEqual(["^Home(Work)"]);
    expect(ctx.state().currentSpaceId).toBe(ctx.ids.space.Work);
  });

  it("does not take the user out of the settings screen when a page goes", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    ctx.state().navigate("settings");
    await ctx.state().trashPage(ctx.ids.page.Journal ?? "");
    await ctx.settled((s) => s.selectedPageId === null);
    expect(ctx.state().route).toBe("settings");
  });

  it("a closed tab whose page went meanwhile is not brought back", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    ctx.state().closeTab(ctx.tabOf("Journal")?.id ?? "");
    await ctx.state().trashPage(ctx.ids.page.Journal ?? "");
    await ctx.settled((s) => s.tree?.trashed.length === 1);
    ctx.state().reopenClosedTab();
    expect(ctx.strip()).toEqual(["^Home(Personal)"]);
    expect(ctx.state().closedTabs).toEqual([]);
  });
});

describe("ui.tabs", () => {
  it("is written when the stored part changes, and only then", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    await ctx.settled(() => ctx.tabWrites().length === 1);
    await vi.waitFor(async () => {
      await expect(ctx.direct.settingsGet(TABS_KEY)).resolves.toEqual({
        tabs: [{ spaceId: ctx.ids.space.Personal, pageId: ctx.ids.page.Journal, pinned: false }],
        active: 0,
      });
    });
    expect(ctx.state().settings[TABS_KEY]).toEqual({
      tabs: [{ spaceId: ctx.ids.space.Personal, pageId: ctx.ids.page.Journal, pinned: false }],
      active: 0,
    });

    open(ctx, "Journal", "current", "b"); // the same page again
    ctx.state().saveScroll("some-entry", 40);
    ctx.state().navigate("settings");
    open(ctx, "Notes", "background"); // a new tab: that changes it
    await ctx.settled(() => ctx.tabWrites().length === 2);
    ctx.state().goBack(); // back to Home in the first tab: the current entry changes
    await ctx.settled(() => ctx.tabWrites().length === 3);
    ctx.state().goForward();
    await ctx.settled(() => ctx.tabWrites().length === 4);
    await ctx.quiet();
    expect(ctx.tabWrites()).toHaveLength(4);
    expect(ctx.tabWrites().at(-1)).toMatchObject({
      data: {
        key: TABS_KEY,
        value: {
          tabs: [
            { pageId: ctx.ids.page.Journal, pinned: false },
            { pageId: ctx.ids.page.Notes, pinned: false },
          ],
          active: 0,
        },
      },
    });
  });

  it("stores the pinned tabs first and the position of the active tab", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    open(ctx, "Notes", "foreground");
    ctx.state().setTabPinned(ctx.tabOf("Notes")?.id ?? "", true);
    await ctx.settled(() => ctx.tabWrites().length >= 3);
    await vi.waitFor(async () => {
      await expect(ctx.direct.settingsGet(TABS_KEY)).resolves.toMatchObject({
        tabs: [
          { pageId: ctx.ids.page.Notes, pinned: true },
          { pageId: null, pinned: false },
          { pageId: ctx.ids.page.Journal, pinned: false },
        ],
        active: 0,
      });
    });
  });

  it("is written when only the active tab changes", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    await ctx.settled(() => ctx.tabWrites().length === 1);
    ctx.state().activateTabAt(0);
    await ctx.settled(() => ctx.tabWrites().length === 2);
    expect(ctx.tabWrites().at(-1)).toMatchObject({ data: { key: TABS_KEY, value: { active: 0 } } });
    ctx.state().activateAdjacentTab(1);
    await ctx.settled(() => ctx.tabWrites().length === 3);
    expect(ctx.tabWrites().at(-1)).toMatchObject({ data: { key: TABS_KEY, value: { active: 1 } } });
  });

  it("is not acted on when it changes from elsewhere", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    await ctx.settled(() => ctx.tabWrites().length === 1);
    const before = ctx.state().tabs;
    await ctx.memory.dispatch({
      type: "SettingsSet",
      data: { key: TABS_KEY, value: { tabs: [{ spaceId: ctx.ids.space.Work, pageId: null, pinned: false }], active: 0 } },
    });
    await ctx.quiet();
    expect(ctx.state().tabs).toBe(before);
    expect(ctx.tabWrites()).toHaveLength(1);
  });

  it("keeps the tabs and logs when the setting cannot be written", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const ctx = await setup();
    const real = ctx.memory.dispatch.bind(ctx.memory);
    ctx.memory.dispatch = (request) =>
      request.type === "SettingsSet" && request.data.key === TABS_KEY
        ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
        : real(request);
    open(ctx, "Journal");
    await vi.waitFor(() => {
      expect(logged).toHaveBeenCalledWith("could not remember the open tabs", expect.anything());
    });
    expect(ctx.strip()).toEqual(["^Journal"]);
    logged.mockRestore();
  });
});

describe("lock", () => {
  it("clears the tabs, the closed tabs and the scroll positions, writes nothing, and restores the tabs on unlock", async () => {
    const ctx = await setup();
    open(ctx, "Journal", "foreground");
    open(ctx, "Notes", "foreground");
    open(ctx, "Projects", "foreground");
    ctx.state().closeTab(ctx.tabOf("Notes")?.id ?? "");
    const active = ctx.state().tabs.find((tab) => tab.id === ctx.state().activeTabId);
    ctx.state().saveScroll(active === undefined ? "" : currentEntry(active).id, 321);
    await ctx.settled(() => ctx.tabWrites().length >= 3);
    await ctx.quiet();
    const written = ctx.tabWrites().length;
    const strip = ctx.strip();
    expect(ctx.state().closedTabs).toHaveLength(1);

    await ctx.state().lock();
    expect(ctx.state()).toMatchObject({
      phase: "locked",
      tabs: [],
      activeTabId: null,
      closedTabs: [],
      currentSpaceId: null,
      selectedPageId: null,
    });
    expect(ctx.state().savedScroll(active === undefined ? "" : currentEntry(active).id)).toBeUndefined();
    await ctx.quiet();
    expect(ctx.tabWrites()).toHaveLength(written);

    await ctx.state().unlock(PASSPHRASE);
    await ctx.settled((s) => s.phase === "ready" && s.tabs.length > 0);
    expect(ctx.strip().map((text) => text.replace("^", ""))).toEqual(strip.map((text) => text.replace("^", "")));
    expect(ctx.strip().findIndex((text) => text.startsWith("^"))).toBe(strip.findIndex((text) => text.startsWith("^")));
    expect(ctx.state().closedTabs).toEqual([]);
    expect(ctx.state().tabs.every((tab) => tab.entries.length === 1)).toBe(true);
    expect(ctx.state().selectedPageId).toBe(ctx.ids.page.Projects);
    await ctx.settled((s) => s.tree?.spaceId === ctx.ids.space.Personal);
    await ctx.quiet();
    expect(ctx.tabWrites()).toHaveLength(written); // what was stored is what came back
  });
});

describe("recent pages", () => {
  it("record a page when it shows in front: plain and foreground opens, activation, back and forward", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    open(ctx, "Notes", "foreground");
    expect(ctx.recents()).toEqual(["Notes", "Journal"]);
    ctx.state().activateTab(ctx.tabOf("Journal")?.id ?? "");
    expect(ctx.recents()).toEqual(["Journal", "Notes"]);
    open(ctx, "Projects");
    ctx.state().goBack();
    expect(ctx.recents()).toEqual(["Journal", "Projects", "Notes"]);
    ctx.state().goForward();
    expect(ctx.recents()).toEqual(["Projects", "Journal", "Notes"]);
    // A Home shows no page.
    ctx.state().newTab();
    expect(ctx.recents()).toEqual(["Projects", "Journal", "Notes"]);
  });
});

describe("scroll positions", () => {
  it("are remembered per entry without touching the state", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    const front = ctx.state().tabs[0];
    const entry = front === undefined ? undefined : currentEntry(front);
    const before = ctx.state();
    ctx.state().saveScroll(entry?.id ?? "", 250);
    expect(ctx.state().savedScroll(entry?.id ?? "")).toBe(250);
    expect(ctx.state()).toBe(before);
    expect(ctx.state().savedScroll("nope")).toBeUndefined();
  });

  it("are dropped with the entries that can no longer show: forward entries, and gone pages", async () => {
    const ctx = await setup();
    open(ctx, "Journal");
    open(ctx, "Notes");
    const entries = ctx.state().tabs[0]?.entries ?? [];
    for (const entry of entries) {
      ctx.state().saveScroll(entry.id, 10);
    }
    ctx.state().goBack();
    open(ctx, "Projects"); // drops the Notes entry
    expect(ctx.state().savedScroll(entries[2]?.id ?? "")).toBeUndefined();
    expect(ctx.state().savedScroll(entries[1]?.id ?? "")).toBe(10);
  });
});

describe("no loops", () => {
  it("settles after a burst of opening, closing and trashing", async () => {
    const ctx = await setup();
    const titles: PageTitle[] = ["Journal", "Notes", "Projects", "Roadmap", "Meetings", "Plans"];
    for (const title of titles) {
      open(ctx, title, "foreground");
    }
    await ctx.state().trashPage(ctx.ids.page.Notes ?? "");
    ctx.state().closeOtherTabs(ctx.state().tabs[0]?.id ?? "");
    ctx.state().reopenClosedTab();
    await ctx.quiet();
    const before = ctx.requests.length;
    await ctx.quiet();
    await ctx.quiet();
    expect(ctx.requests.length).toBe(before);
  });
});
