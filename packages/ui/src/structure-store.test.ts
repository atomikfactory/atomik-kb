import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import type { AppState } from "./store";
import { collapsedKey, parseCollapsed, selectCurrentSpace } from "./store-structure";

const PERSONAL = {
  name: "Personal",
  pages: [{ title: "Projects", children: [{ title: "Roadmap" }, { title: "Ideas" }] }, { title: "Journal" }],
};
const WORK = { name: "Work", pages: [{ title: "Meetings" }] };

function setup(options: MemoryTransportOptions = {}, before?: (request: Request, memory: MemoryTransport) => Promise<void>) {
  const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL, WORK], ...options });
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: async (request) => {
      requests.push(request);
      await before?.(request, memory);
      return memory.dispatch(request);
    },
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
  };
  const client = new CoreClient(transport);
  const store = createAppStore(client, { corePollMs: 5 });
  const stop = store.getState().start();
  const count = (type: Request["type"]): number => requests.filter((request) => request.type === type).length;
  return { memory, client: new CoreClient(memory), store, stop, requests, count };
}

type Ctx = ReturnType<typeof setup>;

const settled = (ctx: Ctx, check: (state: AppState) => boolean) =>
  vi.waitFor(
    () => {
      expect(check(ctx.store.getState())).toBe(true);
    },
    { timeout: 4000 },
  );

/** Boots the store and waits until the first space and its tree are in. */
async function ready(options: MemoryTransportOptions = {}, before?: Parameters<typeof setup>[1]): Promise<Ctx> {
  const ctx = setup(options, before);
  await settled(ctx, (s) => s.phase === "ready");
  return ctx;
}

const titles = (state: AppState): string[] => state.tree?.roots.map((node) => node.page.title) ?? [];

describe("collapsed pages setting", () => {
  it("is stored per space under ui.sidebar.collapsed.<spaceId>", () => {
    expect(collapsedKey("11111111-1111-4111-8111-111111111111")).toBe(
      "ui.sidebar.collapsed.11111111-1111-4111-8111-111111111111",
    );
  });

  it("reads a list of page ids and treats anything else as nothing collapsed", () => {
    expect([...parseCollapsed(["a", "b"])]).toEqual(["a", "b"]);
    expect([...parseCollapsed(["a", 1, null, { x: 1 }, "b"])]).toEqual(["a", "b"]);
    for (const value of [null, undefined, "a", 3, true, { a: 1 }]) {
      expect(parseCollapsed(value).size, JSON.stringify(value)).toBe(0);
    }
  });
});

describe("boot", () => {
  it("loads the spaces and the tree of the first space before the app is ready", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "ready");
    const state = ctx.store.getState();
    expect(state.spaces.map((space) => space.name)).toEqual(["Personal", "Work"]);
    expect(state.spacesLoaded).toBe(true);
    expect(state.currentSpaceId).toBe(state.spaces[0]?.id);
    expect(selectCurrentSpace(state)).toMatchObject({ name: "Personal" });
    expect(titles(state)).toEqual(["Projects", "Journal"]);
    expect(state.selectedPageId).toBeNull();
    // One read each, nothing refreshed twice.
    expect(ctx.count("SpaceList")).toBe(1);
    expect(ctx.count("TreeGet")).toBe(1);
  });

  it("has no space and no tree for a workspace without spaces, but knows that it looked", async () => {
    const ctx = await ready({ spaces: [] });
    const state = ctx.store.getState();
    expect(state).toMatchObject({ spaces: [], spacesLoaded: true, currentSpaceId: null, tree: null });
    expect(selectCurrentSpace(state)).toBeNull();
    expect(ctx.count("TreeGet")).toBe(0);
  });

  it("does not look for spaces before there is a workspace", async () => {
    const ctx = setup({ workspaceName: undefined, spaces: [] });
    await settled(ctx, (s) => s.phase === "onboarding");
    expect(ctx.count("SpaceList")).toBe(0);
    expect(ctx.store.getState().spacesLoaded).toBe(false);
  });

  it("goes fatal when the spaces cannot be read", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const transport: Transport = {
      dispatch: (request) =>
        request.type === "SpaceList"
          ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
          : memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
    };
    const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
    store.getState().start();
    await vi.waitFor(() => {
      expect(store.getState().phase).toBe("fatal");
    });
    expect(store.getState().fatalError).toEqual({ code: "Storage", message: "disk I/O error" });
  });

  it("brings the first space with the workspace when onboarding asks for one", async () => {
    const ctx = setup({ workspaceName: undefined, spaces: [] });
    await settled(ctx, (s) => s.phase === "onboarding");
    await ctx.store.getState().createWorkspace("Home", "Personal");
    // The space is there while the onboarding still shows the recovery kit step.
    await settled(ctx, (s) => s.spaces.length === 1 && s.tree !== null);
    const state = ctx.store.getState();
    expect(state).toMatchObject({ phase: "onboarding", onboardingStep: "kit", spacesLoaded: true });
    expect(selectCurrentSpace(state)).toMatchObject({ name: "Personal" });
    expect(state.tree?.roots).toEqual([]);
    ctx.store.getState().finishOnboarding();
    expect(ctx.store.getState()).toMatchObject({ phase: "ready", spacesLoaded: true });
  });

  it("reads the collapsed pages of the first space on boot", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const [space] = await new CoreClient(memory).spaceList();
    const view = await new CoreClient(memory).treeGet(space?.id ?? "");
    const projects = view.roots[0]?.page.id ?? "";
    await memory.dispatch({ type: "SettingsSet", data: { key: collapsedKey(space?.id ?? ""), value: [projects] } });
    const store = createAppStore(new CoreClient(memory), { corePollMs: 5 });
    store.getState().start();
    await vi.waitFor(() => {
      expect(store.getState().phase).toBe("ready");
    });
    expect([...(store.getState().collapsed[space?.id ?? ""] ?? [])]).toEqual([projects]);
  });
});

describe("selecting spaces and pages", () => {
  it("switches space: clears the tree and the open page, then loads the other tree", async () => {
    const ctx = await ready();
    const [, work] = ctx.store.getState().spaces;
    ctx.store.getState().selectPage(ctx.store.getState().tree?.roots[0]?.page.id ?? null);
    ctx.store.getState().navigate("settings");
    ctx.store.getState().selectSpace(work?.id ?? "");
    expect(ctx.store.getState()).toMatchObject({ currentSpaceId: work?.id, tree: null, selectedPageId: null, route: "home" });
    await settled(ctx, (s) => s.tree !== null);
    expect(titles(ctx.store.getState())).toEqual(["Meetings"]);
  });

  it("does not reload when the current space is selected again", async () => {
    const ctx = await ready();
    const before = ctx.count("TreeGet");
    ctx.store.getState().selectSpace(ctx.store.getState().currentSpaceId ?? "");
    expect(ctx.count("TreeGet")).toBe(before);
    expect(ctx.store.getState().tree).not.toBeNull();
  });

  it("opens a page and shows the home screen", async () => {
    const ctx = await ready();
    ctx.store.getState().navigate("diagnostics");
    ctx.store.getState().selectPage("some-page");
    expect(ctx.store.getState()).toMatchObject({ selectedPageId: "some-page", route: "home" });
    // The Home of the space is a place to go to (ADR-0034), so it shows the main screen too.
    ctx.store.getState().navigate("settings");
    ctx.store.getState().selectPage(null);
    expect(ctx.store.getState()).toMatchObject({ selectedPageId: null, route: "home" });
  });

  it("drops the answer for a space that is no longer the current one", async () => {
    const ctx = await ready();
    const [, work] = ctx.store.getState().spaces;
    const personal = ctx.store.getState().currentSpaceId ?? "";
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const real = ctx.memory.dispatch.bind(ctx.memory);
    // The tree of Work comes back late.
    ctx.memory.dispatch = async (request) => {
      if (request.type === "TreeGet" && request.data.spaceId === work?.id) {
        await gate;
      }
      return real(request);
    };
    ctx.store.getState().selectSpace(work?.id ?? "");
    ctx.store.getState().selectSpace(personal);
    await settled(ctx, (s) => s.tree?.spaceId === personal);
    release();
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(ctx.store.getState().tree?.spaceId).toBe(personal);
    expect(ctx.store.getState().currentSpaceId).toBe(personal);
  });
});

describe("events refresh only what they name", () => {
  it("StructureChanged for the current space re-reads its tree, once", async () => {
    const ctx = await ready();
    const { currentSpaceId } = ctx.store.getState();
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "PageCreate", data: { spaceId: currentSpaceId ?? "", title: "New" } });
    await settled(ctx, (s) => titles(s).includes("New"));
    // The page was created straight in the core, so the one request seen is the store's own refresh.
    expect(ctx.requests.map((request) => request.type)).toEqual(["TreeGet"]);
  });

  it("StructureChanged for another space does nothing", async () => {
    const ctx = await ready();
    const [, work] = await ctx.client.spaceList();
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "PageCreate", data: { spaceId: work?.id ?? "", title: "Elsewhere" } });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(ctx.requests).toEqual([]);
    expect(titles(ctx.store.getState())).toEqual(["Projects", "Journal"]);
  });

  it("SpacesChanged re-reads the spaces only, while the current space is still there", async () => {
    const ctx = await ready();
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "SpaceCreate", data: { name: "Added" } });
    await settled(ctx, (s) => s.spaces.length === 3);
    expect(ctx.requests.map((request) => request.type)).toEqual(["SpaceList"]);
    expect(ctx.store.getState().tree).not.toBeNull();
  });

  it("a space rename re-reads the spaces and the tree, as the core announces both", async () => {
    const ctx = await ready();
    const personal = ctx.store.getState().currentSpaceId ?? "";
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "SpaceRename", data: { spaceId: personal, name: "Home" } });
    await settled(ctx, (s) => selectCurrentSpace(s)?.name === "Home");
    expect(ctx.requests.map((request) => request.type).sort()).toEqual(["SpaceList", "TreeGet"]);
  });

  it("ignores events that are not about the structure", async () => {
    const ctx = await ready();
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "DeviceRename", data: { name: "Elsewhere" } });
    await ctx.memory.dispatch({ type: "SettingsSet", data: { key: "app.other", value: 1 } });
    await new Promise((resolve) => setTimeout(resolve, 20));
    // Only the device is re-read (the UI knows it); no space or tree request follows.
    expect(ctx.requests.map((request) => request.type)).toEqual(["DeviceInfo"]);
  });

  it("continues in the first space when the current one is deleted", async () => {
    const ctx = await ready();
    const [personal, work] = ctx.store.getState().spaces;
    ctx.store.getState().selectSpace(work?.id ?? "");
    await settled(ctx, (s) => s.tree?.spaceId === work?.id);
    ctx.store.getState().selectPage("a-page");
    const meetings = ctx.store.getState().tree?.roots[0]?.page.id ?? "";
    await ctx.memory.dispatch({ type: "PagePurge", data: { pageId: meetings } });
    await ctx.memory.dispatch({ type: "SpaceDelete", data: { spaceId: work?.id ?? "" } });
    await settled(ctx, (s) => s.currentSpaceId === personal?.id && s.tree?.spaceId === personal.id);
    expect(ctx.store.getState().selectedPageId).toBeNull();
    expect(ctx.store.getState().spaces.map((space) => space.name)).toEqual(["Personal"]);
  });

  it("drops the answer to a spaces request that was overtaken by a newer one", async () => {
    const ctx = await ready();
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const real = ctx.memory.dispatch.bind(ctx.memory);
    let first = true;
    ctx.memory.dispatch = async (request) => {
      if (request.type === "SpaceList" && first) {
        first = false;
        const stale = await real(request); // answered now, delivered late
        await gate;
        return stale;
      }
      return real(request);
    };
    await ctx.memory.dispatch({ type: "SpaceCreate", data: { name: "One" } }); // first (late) refresh
    await ctx.memory.dispatch({ type: "SpaceCreate", data: { name: "Two" } }); // second refresh, fast
    await settled(ctx, (s) => s.spaces.length === 4);
    release();
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(ctx.store.getState().spaces.map((space) => space.name)).toEqual(["Personal", "Work", "One", "Two"]);
  });
});

describe("actions rely on events for the refresh", () => {
  it("createPage creates, opens the page and expands its parent, with one tree read", async () => {
    const ctx = await ready();
    const state = () => ctx.store.getState();
    const projects = state().tree?.roots[0]?.page.id ?? "";
    state().setPageCollapsed(projects, true);
    ctx.requests.length = 0;

    const page = await state().createPage({ parentId: projects, title: "Plan" });
    expect(page).toMatchObject({ title: "Plan", parentId: projects });
    expect(state()).toMatchObject({ selectedPageId: page.id, route: "home" });
    expect(state().collapsed[state().currentSpaceId ?? ""]?.has(projects)).toBe(false);
    await settled(ctx, (s) => s.tree?.roots[0]?.children.some((child) => child.page.id === page.id) === true);
    expect(ctx.count("TreeGet")).toBe(1);
    expect(ctx.count("PageCreate")).toBe(1);
  });

  it("createPage with select: false leaves the open page open, and focusPageTitle asks for a title to edit", async () => {
    const ctx = await ready();
    const state = () => ctx.store.getState();
    const projects = state().tree?.roots[0]?.page.id ?? "";
    state().selectPage(projects);
    expect(state().titleFocusPageId).toBeNull();

    const page = await state().createPage({ parentId: projects, select: false });
    expect(page).toMatchObject({ title: "", parentId: projects });
    expect(state().selectedPageId).toBe(projects);
    await settled(ctx, (s) => s.tree?.roots[0]?.children.some((child) => child.page.id === page.id) === true);

    state().focusPageTitle(page.id);
    state().selectPage(page.id);
    expect(state()).toMatchObject({ selectedPageId: page.id, titleFocusPageId: page.id });
    state().focusPageTitle(null);
    expect(state().titleFocusPageId).toBeNull();
  });

  it("createPage appends to the root when no parent is given, and rejects without a space", async () => {
    const ctx = await ready();
    await ctx.store.getState().createPage();
    await settled(ctx, (s) => titles(s).length === 3);
    expect(titles(ctx.store.getState())).toEqual(["Projects", "Journal", ""]);

    const empty = await ready({ spaces: [] });
    await expect(empty.store.getState().createPage()).rejects.toMatchObject({ code: "Internal" });
  });

  it("createSpace creates a space and switches to it", async () => {
    const ctx = await ready();
    const space = await ctx.store.getState().createSpace("  Play ", "🎲");
    expect(space).toMatchObject({ name: "Play", icon: "🎲" });
    expect(ctx.store.getState().currentSpaceId).toBe(space.id);
    await settled(ctx, (s) => s.spaces.length === 3 && s.tree?.spaceId === space.id);
    expect(ctx.store.getState().tree?.roots).toEqual([]);
  });

  it("rename, icon, move, trash, restore and purge go to the core and come back as events", async () => {
    const ctx = await ready();
    const state = () => ctx.store.getState();
    const journal = state().tree?.roots[1]?.page.id ?? "";
    const projects = state().tree?.roots[0]?.page.id ?? "";

    await state().renamePage(journal, "Diary");
    await state().setPageIcon(journal, "📔");
    await settled(ctx, (s) => s.tree?.roots[1]?.page.title === "Diary" && s.tree.roots[1].page.icon === "📔");
    await state().movePage(journal, null, 0);
    await settled(ctx, (s) => s.tree?.roots[0]?.page.id === journal);

    await state().trashPage(projects);
    await settled(ctx, (s) => s.tree?.trashed.length === 1);
    await state().restorePage(projects);
    await settled(ctx, (s) => s.tree?.trashed.length === 0 && titles(s).includes("Projects"));
    await state().trashPage(projects);
    await settled(ctx, (s) => s.tree?.trashed.length === 1);
    await state().purgePage(projects);
    await settled(ctx, (s) => s.tree?.trashed.length === 0 && !titles(s).includes("Projects"));
  });

  it("undo and redo work on the current space and follow the undo state", async () => {
    const ctx = await ready();
    const state = () => ctx.store.getState();
    expect(state().tree?.undo).toEqual({ canUndo: false, canRedo: false });
    await state().createPage({ title: "Scratch" });
    await settled(ctx, (s) => s.tree?.undo.canUndo === true);
    await state().undo();
    await settled(ctx, (s) => !titles(s).includes("Scratch") && s.tree?.undo.canRedo === true);
    await state().redo();
    await settled(ctx, (s) => titles(s).includes("Scratch"));
  });

  it("rejects with the core's error and leaves the state alone", async () => {
    const ctx = await ready();
    const journal = ctx.store.getState().tree?.roots[1]?.page.id ?? "";
    const before = ctx.store.getState().tree;
    const error: unknown = await ctx.store.getState().renamePage(journal, "x".repeat(501)).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "title" } });
    expect(ctx.store.getState().tree).toBe(before);
    await expect(ctx.store.getState().deleteSpace("no-such-space")).rejects.toMatchObject({
      code: "NotFound",
    });
  });

  it("renameSpace, setSpaceIcon, moveSpace and deleteSpace go to the core", async () => {
    const ctx = await ready();
    const state = () => ctx.store.getState();
    const [personal, work] = state().spaces;
    await state().renameSpace(personal?.id ?? "", "Home");
    await state().setSpaceIcon(personal?.id ?? "", "🏡");
    await state().moveSpace(personal?.id ?? "", 1);
    await settled(ctx, (s) => s.spaces.map((space) => space.name).join() === "Work,Home" && s.spaces[1]?.icon === "🏡");
    const extra = await state().createSpace("Extra");
    await state().deleteSpace(extra.id);
    await settled(ctx, (s) => s.spaces.length === 2);
    expect(work).toBeDefined();
  });
});

describe("sortChildren (ADR-0052)", () => {
  /** A clock that moves one millisecond per read, so that pages are created in distinct, increasing times. */
  function counting(): MemoryTransportOptions {
    let clock = 1_700_000_000_000;
    return { now: () => (clock += 1) };
  }
  const reorders = (ctx: Ctx): Extract<Request, { type: "PageReorderChildren" }>[] =>
    ctx.requests.filter((request): request is Extract<Request, { type: "PageReorderChildren" }> => request.type === "PageReorderChildren");

  it("sorts the top-level pages A to Z and back by date created, one request each", async () => {
    const ctx = await ready(counting());
    const state = () => ctx.store.getState();
    const space = state().currentSpaceId ?? "";
    expect(titles(state())).toEqual(["Projects", "Journal"]);

    await state().sortChildren(space, null, "title");
    await settled(ctx, (s) => titles(s).join() === "Journal,Projects");
    expect(reorders(ctx)).toHaveLength(1);
    expect(reorders(ctx)[0]?.data).toMatchObject({ spaceId: space, order: state().tree?.roots.map((node) => node.page.id) });
    expect(reorders(ctx)[0]?.data).not.toHaveProperty("parentId");

    await state().sortChildren(space, null, "created");
    await settled(ctx, (s) => titles(s).join() === "Projects,Journal");
    expect(reorders(ctx)).toHaveLength(2);
  });

  it("sorts the subpages of a page, and one undo takes the sort back", async () => {
    const ctx = await ready(counting());
    const state = () => ctx.store.getState();
    const space = state().currentSpaceId ?? "";
    const projects = state().tree?.roots[0]?.page.id ?? "";
    const children = () => state().tree?.roots[0]?.children.map((node) => node.page.title).join() ?? "";
    expect(children()).toBe("Roadmap,Ideas");

    await state().sortChildren(space, projects, "title");
    await settled(ctx, (s) => children() === "Ideas,Roadmap" && s.tree?.undo.canUndo === true);
    expect(reorders(ctx)[0]?.data).toMatchObject({ spaceId: space, parentId: projects });
    expect(titles(state())).toEqual(["Projects", "Journal"]); // the top level is left alone

    await state().undo();
    await settled(ctx, () => children() === "Roadmap,Ideas");
  });

  it("sends nothing when there are fewer than two subpages or they are in that order already", async () => {
    const ctx = await ready(counting());
    const state = () => ctx.store.getState();
    const space = state().currentSpaceId ?? "";
    const journal = state().tree?.roots[1]?.page.id ?? "";
    const projects = state().tree?.roots[0]?.page.id ?? "";
    await state().sortChildren(space, journal, "title"); // no subpages
    await state().sortChildren(space, null, "created"); // already in the order they were created
    await state().sortChildren(space, projects, "created"); // likewise
    await state().sortChildren(space, "00000000-0000-4000-8000-0000000000ff", "title"); // not a page of the tree
    expect(reorders(ctx)).toHaveLength(0);
    expect(state().tree?.undo.canUndo).toBe(false);
  });

  it("reads the tree again and tries once more when the subpages changed meanwhile", async () => {
    let injected = false;
    const ctx = await ready(counting(), async (request, memory) => {
      // Another device adds a page right before the first order reaches the core.
      if (request.type === "PageReorderChildren" && !injected) {
        injected = true;
        await memory.dispatch({ type: "PageCreate", data: { spaceId: request.data.spaceId, title: "Aardvark" } });
      }
    });
    const state = () => ctx.store.getState();
    const space = state().currentSpaceId ?? "";
    await state().sortChildren(space, null, "title");
    expect(reorders(ctx)).toHaveLength(2);
    expect(reorders(ctx)[0]?.data.order).toHaveLength(2);
    expect(reorders(ctx)[1]?.data.order).toHaveLength(3);
    await settled(ctx, (s) => titles(s).join() === "Aardvark,Journal,Projects");
  });

  it("gives up after the second miss, with the core's error", async () => {
    let created = 0;
    const ctx = await ready(counting(), async (request, memory) => {
      if (request.type === "PageReorderChildren") {
        created += 1;
        await memory.dispatch({ type: "PageCreate", data: { spaceId: request.data.spaceId, title: `Extra ${String(created)}` } });
      }
    });
    const space = ctx.store.getState().currentSpaceId ?? "";
    const error: unknown = await ctx.store.getState().sortChildren(space, null, "title").then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "order" } });
    expect(reorders(ctx)).toHaveLength(2);
  });

  it("does not retry another refusal", async () => {
    const ctx = await ready(counting(), (request) =>
      request.type === "PageReorderChildren"
        ? Promise.reject(new CoreError({ code: "Storage", message: "database error: disk I/O error" }))
        : Promise.resolve(),
    );
    const space = ctx.store.getState().currentSpaceId ?? "";
    await expect(ctx.store.getState().sortChildren(space, null, "title")).rejects.toMatchObject({ code: "Storage" });
    expect(reorders(ctx)).toHaveLength(1);
  });
});

describe("collapsed pages", () => {
  it("collapsing remembers the page in the core under the key of the space", async () => {
    const ctx = await ready();
    const state = () => ctx.store.getState();
    const space = state().currentSpaceId ?? "";
    const projects = state().tree?.roots[0]?.page.id ?? "";
    expect(state().collapsed[space]?.size).toBe(0);

    state().setPageCollapsed(projects, true);
    expect(state().collapsed[space]?.has(projects)).toBe(true);
    await vi.waitFor(async () => {
      await expect(ctx.client.settingsGet(collapsedKey(space))).resolves.toEqual([projects]);
    });
    state().setPageCollapsed(projects, false);
    await vi.waitFor(async () => {
      await expect(ctx.client.settingsGet(collapsedKey(space))).resolves.toEqual([]);
    });
  });

  it("does nothing when the page is already in the wanted state", async () => {
    const ctx = await ready();
    const projects = ctx.store.getState().tree?.roots[0]?.page.id ?? "";
    ctx.requests.length = 0;
    ctx.store.getState().setPageCollapsed(projects, false);
    expect(ctx.requests).toEqual([]);
    ctx.store.getState().setPageCollapsed(projects, true);
    ctx.store.getState().setPageCollapsed(projects, true);
    expect(ctx.requests.filter((request) => request.type === "SettingsSet")).toHaveLength(1);
  });

  it("does not write anything before the setting was read, which would drop what is stored", async () => {
    const ctx = await ready({ spaces: [] });
    ctx.requests.length = 0;
    ctx.store.getState().setPageCollapsed("a-page", true);
    expect(ctx.requests).toEqual([]);
    expect(ctx.store.getState().collapsed).toEqual({});
  });

  it("keeps the pages of each space apart and reads each setting once", async () => {
    const ctx = await ready();
    const [personal, work] = ctx.store.getState().spaces;
    const projects = ctx.store.getState().tree?.roots[0]?.page.id ?? "";
    ctx.store.getState().setPageCollapsed(projects, true);

    ctx.store.getState().selectSpace(work?.id ?? "");
    await settled(ctx, (s) => s.tree?.spaceId === work?.id);
    expect(ctx.store.getState().collapsed[work?.id ?? ""]?.size).toBe(0);
    ctx.store.getState().selectSpace(personal?.id ?? "");
    await settled(ctx, (s) => s.tree?.spaceId === personal?.id);
    expect(ctx.store.getState().collapsed[personal?.id ?? ""]?.has(projects)).toBe(true);
    const reads = ctx.requests.filter(
      (request) => request.type === "SettingsGet" && request.data.key === collapsedKey(personal?.id ?? ""),
    );
    expect(reads).toHaveLength(1);
  });

  it("keeps the new state and reports it when the setting cannot be written", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const ctx = await ready();
    const projects = ctx.store.getState().tree?.roots[0]?.page.id ?? "";
    ctx.memory.failNext({ code: "Storage", message: "disk I/O error" });
    ctx.store.getState().setPageCollapsed(projects, true);
    await vi.waitFor(() => {
      expect(logged).toHaveBeenCalled();
    });
    expect(ctx.store.getState().collapsed[ctx.store.getState().currentSpaceId ?? ""]?.has(projects)).toBe(true);
    logged.mockRestore();
  });

  it("falls back to nothing collapsed when the setting cannot be read", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const transport: Transport = {
      dispatch: (request) =>
        request.type === "SettingsGet" && request.data.key.startsWith("ui.sidebar.collapsed")
          ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
          : memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
    };
    const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
    store.getState().start();
    await vi.waitFor(() => {
      expect(store.getState().phase).toBe("ready");
    });
    expect(store.getState().collapsed[store.getState().currentSpaceId ?? ""]?.size).toBe(0);
    expect(logged).toHaveBeenCalled();
    logged.mockRestore();
  });
});
