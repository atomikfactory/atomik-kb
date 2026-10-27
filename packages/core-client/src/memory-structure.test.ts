import { describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { PageNode } from "./generated/PageNode";
import type { SpaceInfo } from "./generated/SpaceInfo";
import type { TreeView } from "./generated/TreeView";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";

/** Deterministic ids and clock (every read of the clock advances it by one millisecond). */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  let clock = T0;
  const transport = new MemoryTransport({
    newId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
    now: () => clock++,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** Titles of the live tree, depth-first, indented by depth: structure and order in one value. */
function outline(view: TreeView): string[] {
  const lines: string[] = [];
  const walk = (nodes: PageNode[], depth: number): void => {
    for (const node of nodes) {
      lines.push(`${"  ".repeat(depth)}${node.page.title}`);
      walk(node.children, depth + 1);
    }
  };
  walk(view.roots, 0);
  return lines;
}

/** Looks up the id of a live page by its title. */
function idsOf(view: TreeView): (title: string) => string {
  const found = new Map<string, string>();
  const walk = (nodes: PageNode[]): void => {
    for (const node of nodes) {
      found.set(node.page.title, node.page.id);
      walk(node.children);
    }
  };
  walk(view.roots);
  return (title) => {
    const id = found.get(title);
    if (id === undefined) {
      throw new Error(`no live page titled ${title}`);
    }
    return id;
  };
}

const names = (spaces: SpaceInfo[]): string[] => spaces.map((space) => space.name);

const DEMO: SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "A", children: [{ title: "A1", children: [{ title: "A1x" }] }, { title: "A2" }] },
      { title: "B" },
    ],
  },
];

/** A space with the pages A (A1 (A1x), A2) and B, created through the seed option. */
async function demo(options: MemoryTransportOptions = {}) {
  const ctx = setup({ spaces: DEMO, ...options });
  const [space] = await ctx.client.spaceList();
  if (space === undefined) {
    throw new Error("the demo space is missing");
  }
  const view = await ctx.client.treeGet(space.id);
  return {
    ...ctx,
    spaceId: space.id,
    id: idsOf(view),
    outline: async () => outline(await ctx.client.treeGet(space.id)),
  };
}

describe("spaces", () => {
  it("starts with none and lists them in creation order", async () => {
    const { client } = setup();
    await expect(client.spaceList()).resolves.toEqual([]);
    const alpha = await client.spaceCreate("Alpha");
    const beta = await client.spaceCreate("Beta", "🅱");
    await expect(client.spaceList()).resolves.toEqual([alpha, beta]);
    expect(beta.icon).toBe("🅱");
  });

  it("creates with a trimmed name, no pages and a creation time, and emits SpacesChanged", async () => {
    const { client, events } = setup();
    const created = await client.spaceCreate("  Personal \n");
    expect(created).toStrictEqual({
      id: "00000000-0000-4000-8000-000000000002",
      name: "Personal",
      createdAt: T0 + 1,
      pageCount: 0,
    });
    expect(events).toEqual([{ type: "SpacesChanged" }]);
  });

  it("validates names like the core", async () => {
    const { client, events } = setup();
    const cases = [
      ["", "must not be empty"],
      ["   ", "must not be empty"],
      ["x".repeat(101), "must be at most 100 characters"],
      ["bad\u0007name", "must not contain control characters"],
    ] as const;
    for (const [name, message] of cases) {
      const error = await failure(client.spaceCreate(name));
      expect(error.toApiError()).toEqual({ code: "InvalidInput", message, details: { field: "name" } });
    }
    await expect(client.spaceList()).resolves.toEqual([]);
    expect(events).toEqual([]);
    // 100 characters are fine, also when they are 200 UTF-16 code units.
    await expect(client.spaceCreate("\u{1F600}".repeat(100))).resolves.toBeDefined();
    await expect(client.spaceCreate(`  ${"x".repeat(100)}  `)).resolves.toMatchObject({ name: "x".repeat(100) });
  });

  it("validates icons: trimmed, 1 to 32 bytes, no control characters", async () => {
    const { client } = setup();
    expect((await client.spaceCreate("A", "  📚 ")).icon).toBe("📚");
    // Four bytes each: eight of them are exactly 32 bytes, nine are too many.
    expect((await client.spaceCreate("B", "🙂".repeat(8))).icon).toBe("🙂".repeat(8));
    expect((await failure(client.spaceCreate("C", "   "))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must not be empty; omit it to clear",
      details: { field: "icon" },
    });
    expect(await failure(client.spaceCreate("D", "🙂".repeat(9)))).toMatchObject({
      message: "must be at most 32 bytes",
      details: { field: "icon" },
    });
    expect(await failure(client.spaceCreate("E", "a\u0007"))).toMatchObject({
      message: "must not contain control characters",
      details: { field: "icon" },
    });
    await expect(client.spaceList()).resolves.toHaveLength(2);
  });

  it("renames with a trimmed name and emits SpacesChanged, then StructureChanged", async () => {
    const { client, events } = setup();
    const created = await client.spaceCreate("Old");
    events.length = 0;
    const renamed = await client.spaceRename(created.id, "  New  ");
    expect(renamed).toMatchObject({ id: created.id, name: "New" });
    expect(events).toEqual([
      { type: "SpacesChanged" },
      { type: "StructureChanged", data: { spaceId: created.id } },
    ]);
    await expect(client.spaceList()).resolves.toEqual([renamed]);
  });

  it("checks the new name before it looks for the space", async () => {
    const { client } = setup();
    expect(await failure(client.spaceRename(UNKNOWN, " "))).toMatchObject({
      code: "InvalidInput",
      details: { field: "name" },
    });
    expect((await failure(client.spaceRename(UNKNOWN, "ok"))).toApiError()).toEqual({
      code: "NotFound",
      message: "space not found",
    });
  });

  it("sets and clears the icon and emits SpacesChanged only", async () => {
    const { client, events } = setup();
    const created = await client.spaceCreate("Home");
    events.length = 0;
    const withIcon = await client.spaceSetIcon(created.id, " 🏠 ");
    expect(withIcon.icon).toBe("🏠");
    const cleared = await client.spaceSetIcon(created.id, null);
    expect(cleared).toStrictEqual(created);
    expect(events).toEqual([{ type: "SpacesChanged" }, { type: "SpacesChanged" }]);
    expect(await failure(client.spaceSetIcon(created.id, ""))).toMatchObject({
      code: "InvalidInput",
      details: { field: "icon" },
    });
    expect(await failure(client.spaceSetIcon(UNKNOWN, "🏠"))).toMatchObject({ code: "NotFound" });
    expect(events).toHaveLength(2);
  });

  it("moves a space to an index, clamps it and returns the new order", async () => {
    const { client, events } = setup();
    await client.spaceCreate("Alpha");
    await client.spaceCreate("Beta");
    const gamma = await client.spaceCreate("Gamma");
    const [alpha] = await client.spaceList();
    events.length = 0;

    expect(names(await client.spaceMove(gamma.id, 0))).toEqual(["Gamma", "Alpha", "Beta"]);
    expect(names(await client.spaceMove(gamma.id, 99))).toEqual(["Alpha", "Beta", "Gamma"]);
    expect(names(await client.spaceMove(alpha?.id ?? "", 1))).toEqual(["Beta", "Alpha", "Gamma"]);
    expect(names(await client.spaceList())).toEqual(["Beta", "Alpha", "Gamma"]);
    expect(events).toEqual([{ type: "SpacesChanged" }, { type: "SpacesChanged" }, { type: "SpacesChanged" }]);
  });

  it("rejects moving an unknown space or to a position that is not a whole number", async () => {
    const { client, events } = setup();
    const only = await client.spaceCreate("Only");
    events.length = 0;
    expect(await failure(client.spaceMove(UNKNOWN, 0))).toMatchObject({ code: "NotFound" });
    for (const index of [-1, 1.5, Number.NaN]) {
      expect(await failure(client.spaceMove(only.id, index))).toMatchObject({
        code: "InvalidInput",
        details: { field: "index" },
      });
    }
    expect(events).toEqual([]);
  });

  it("deletes a personal space with its live and trashed pages, for good", async () => {
    const { client, events } = setup();
    const keep = await client.spaceCreate("Keep");
    const gone = await client.spaceCreate("Gone");
    const page = await client.pageCreate({ spaceId: gone.id, title: "Page" });
    const other = await client.pageCreate({ spaceId: gone.id, title: "Other" });
    await client.pageTrash(other.id);

    events.length = 0;
    await expect(client.spaceDelete(gone.id)).resolves.toBeUndefined();
    expect(events).toEqual([{ type: "SpacesChanged" }]);
    await expect(client.spaceList()).resolves.toEqual([keep]);
    expect(await failure(client.treeGet(gone.id))).toMatchObject({ code: "NotFound", message: "space not found" });
    expect(await failure(client.pageGet(page.id))).toMatchObject({ code: "NotFound" });
    expect(await failure(client.pageGet(other.id))).toMatchObject({ code: "NotFound" });
    expect(await failure(client.spaceDelete(gone.id))).toMatchObject({ code: "NotFound" });
  });

  it("counts only live pages in pageCount", async () => {
    const { client, spaceId, id } = await demo();
    await expect(client.spaceList()).resolves.toMatchObject([{ id: spaceId, pageCount: 5 }]);
    await client.pageTrash(id("A1"));
    await expect(client.spaceList()).resolves.toMatchObject([{ pageCount: 3 }]);
    await client.pagePurge(id("A1"));
    await expect(client.spaceList()).resolves.toMatchObject([{ pageCount: 3 }]);
  });
});

describe("workspace with a first space", () => {
  it("creates the first space with the workspace and emits WorkspaceCreated, then SpacesChanged", async () => {
    const { client, events } = setup();
    const workspace = await client.workspaceCreate("Home", "  Personal ");
    expect(workspace.name).toBe("Home");
    expect(events).toEqual([{ type: "WorkspaceCreated", data: { id: workspace.id } }, { type: "SpacesChanged" }]);
    await expect(client.spaceList()).resolves.toMatchObject([{ name: "Personal", pageCount: 0 }]);
    const [first] = await client.spaceList();
    const view = await client.treeGet(first?.id ?? "");
    expect(view).toStrictEqual({
      spaceId: first?.id,
      roots: [],
      trashed: [],
      undo: { canUndo: false, canRedo: false },
    });
  });

  it("creates no space when the workspace is created without a name for it", async () => {
    const { client, events } = setup();
    await client.workspaceCreate("Home");
    await expect(client.spaceList()).resolves.toEqual([]);
    expect(events).toHaveLength(1);
  });

  it("validates the first space name as firstSpaceName, after the workspace name, creating nothing", async () => {
    const { client, events } = setup();
    for (const bad of ["", "   ", "x".repeat(101), "bad\u0007"]) {
      expect(await failure(client.workspaceCreate("Home", bad))).toMatchObject({
        code: "InvalidInput",
        details: { field: "firstSpaceName" },
      });
    }
    expect(await failure(client.workspaceCreate(" ", "Personal"))).toMatchObject({
      details: { field: "name" },
    });
    await expect(client.workspaceGet()).resolves.toBeNull();
    await expect(client.spaceList()).resolves.toEqual([]);
    expect(events).toEqual([]);
  });

  it("refuses a second workspace without adding its first space", async () => {
    const { client } = setup();
    await client.workspaceCreate("First", "One");
    expect(await failure(client.workspaceCreate("Second", "Two"))).toMatchObject({ code: "AlreadyExists" });
    expect(names(await client.spaceList())).toEqual(["One"]);
  });
});

describe("pages: create and read", () => {
  it("creates root pages in order, trimmed, with ids and timestamps, and emits StructureChanged", async () => {
    const { client, events } = setup();
    const space = await client.spaceCreate("Personal");
    events.length = 0;
    const alpha = await client.pageCreate({ spaceId: space.id, title: "  Alpha " });
    const beta = await client.pageCreate({ spaceId: space.id, title: "Beta" });
    expect(alpha).toStrictEqual({
      id: "00000000-0000-4000-8000-000000000003",
      spaceId: space.id,
      title: "Alpha",
      createdAt: T0 + 2,
      updatedAt: T0 + 2,
      // The device that made the page (ADR-0040: PageInfo carries it).
      createdBy: (await client.appStatus()).deviceId,
      editedBy: (await client.appStatus()).deviceId,
    });
    expect(beta.id).toBe("00000000-0000-4000-8000-000000000004");
    const view = await client.treeGet(space.id);
    expect(view.roots.map((node) => node.page.title)).toEqual(["Alpha", "Beta"]);
    expect(events).toEqual([
      { type: "StructureChanged", data: { spaceId: space.id } },
      { type: "StructureChanged", data: { spaceId: space.id } },
    ]);
  });

  it("inserts at an index, clamps it and nests under a parent", async () => {
    const { client, spaceId, id, outline: tree } = await demo();
    await client.pageCreate({ spaceId, index: 0, title: "First" });
    await client.pageCreate({ spaceId, parentId: id("A"), index: 1, title: "A1b" });
    await client.pageCreate({ spaceId, parentId: id("B"), index: 99, title: "B1" });
    await client.pageCreate({ spaceId, parentId: id("B"), index: 0, title: "B0" });
    expect(await tree()).toEqual(["First", "A", "  A1", "    A1x", "  A1b", "  A2", "B", "  B0", "  B1"]);
    const nested = await client.pageGet(id("A1x"));
    expect(nested.parentId).toBe(id("A1"));
  });

  it("validates titles: empty is fine, at most 500 characters, no control characters", async () => {
    const { client } = setup();
    const space = await client.spaceCreate("Personal");
    await expect(client.pageCreate({ spaceId: space.id, title: "   " })).resolves.toMatchObject({ title: "" });
    await expect(client.pageCreate({ spaceId: space.id, title: "x".repeat(500) })).resolves.toBeDefined();
    const cases = [
      ["x".repeat(501), "must be at most 500 characters"],
      ["two\nlines", "must not contain control characters"],
      ["tab\there", "must not contain control characters"],
    ] as const;
    for (const [title, message] of cases) {
      const error = await failure(client.pageCreate({ spaceId: space.id, title }));
      expect(error.toApiError()).toEqual({ code: "InvalidInput", message, details: { field: "title" } });
    }
    expect((await client.treeGet(space.id)).roots).toHaveLength(2);
  });

  it("reports an unknown space, and checks the title before the space", async () => {
    const { client, events } = setup();
    expect((await failure(client.pageCreate({ spaceId: UNKNOWN, title: "x" }))).toApiError()).toEqual({
      code: "NotFound",
      message: "space not found",
    });
    expect(await failure(client.pageCreate({ spaceId: UNKNOWN, title: "x".repeat(501) }))).toMatchObject({
      code: "InvalidInput",
      details: { field: "title" },
    });
    expect(events).toEqual([]);
  });

  it("refuses a parent that does not exist in the space or is in the trash, creating nothing", async () => {
    const { client, spaceId, id, events, outline: tree } = await demo();
    const other = await client.spaceCreate("Other");
    const foreign = await client.pageCreate({ spaceId: other.id, title: "Foreign" });
    await client.pageTrash(id("B"));
    events.length = 0;
    const refusal = {
      code: "InvalidInput",
      message: "the parent page does not exist or is in the trash",
      details: { field: "parentId" },
    };
    for (const parentId of [UNKNOWN, foreign.id, id("B")]) {
      const error = await failure(client.pageCreate({ spaceId, parentId, title: "x" }));
      expect(error.toApiError(), parentId).toEqual(refusal);
    }
    expect(await tree()).toEqual(["A", "  A1", "    A1x", "  A2"]);
    expect(events).toEqual([]);
  });

  it("reads a page from any space and reports NotFound for unknown ids", async () => {
    const { client } = setup();
    const one = await client.spaceCreate("One");
    const two = await client.spaceCreate("Two");
    const inOne = await client.pageCreate({ spaceId: one.id, title: "In one" });
    const inTwo = await client.pageCreate({ spaceId: two.id, title: "In two" });
    const child = await client.pageCreate({ spaceId: two.id, parentId: inTwo.id, title: "Child" });
    await expect(client.pageGet(inOne.id)).resolves.toEqual(inOne);
    await expect(client.pageGet(child.id)).resolves.toMatchObject({ spaceId: two.id, parentId: inTwo.id });
    expect((await failure(client.pageGet(UNKNOWN))).toApiError()).toEqual({
      code: "NotFound",
      message: "page not found",
    });
  });

  it("builds the tree view from nested nodes and leaves unset fields out", async () => {
    const { client } = setup();
    const space = await client.spaceCreate("Personal");
    const parent = await client.pageCreate({ spaceId: space.id, title: "Parent" });
    const child = await client.pageCreate({ spaceId: space.id, parentId: parent.id, title: "Child" });
    const iconified = await client.pageSetIcon(child.id, "📝");
    expect(iconified).toMatchObject({ parentId: parent.id, icon: "📝" });
    await expect(client.treeGet(space.id)).resolves.toStrictEqual({
      spaceId: space.id,
      roots: [{ page: parent, children: [{ page: iconified, children: [] }] }],
      trashed: [],
      undo: { canUndo: true, canRedo: false },
    });
  });

  it("hands out copies, not its own state", async () => {
    const { client, spaceId, outline: tree } = await demo();
    const view = await client.treeGet(spaceId);
    const [first] = view.roots;
    if (first === undefined) {
      throw new Error("expected pages");
    }
    first.page.title = "tampered";
    first.children.length = 0;
    view.roots.length = 0;
    expect(await tree()).toEqual(["A", "  A1", "    A1x", "  A2", "B"]);
    const spaces = await client.spaceList();
    spaces[0] = { ...spaces[0], name: "tampered" } as SpaceInfo;
    expect(names(await client.spaceList())).toEqual(["Personal"]);
  });
});

describe("pages: rename and icon", () => {
  it("renames with a trimmed title, bumps updatedAt and emits StructureChanged", async () => {
    const { client, spaceId, id, events } = await demo();
    const before = await client.pageGet(id("B"));
    events.length = 0;
    const renamed = await client.pageRename(id("B"), "  Renamed \t");
    expect(renamed).toMatchObject({ id: id("B"), title: "Renamed", createdAt: before.createdAt });
    expect(renamed.updatedAt).toBeGreaterThan(before.updatedAt);
    await expect(client.pageGet(id("B"))).resolves.toEqual(renamed);
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId } }]);
  });

  it("allows an empty title and refuses invalid ones without touching the page", async () => {
    const { client, id, events } = await demo();
    await expect(client.pageRename(id("B"), "  ")).resolves.toMatchObject({ title: "" });
    events.length = 0;
    for (const title of ["x".repeat(501), "a\nb"]) {
      expect(await failure(client.pageRename(id("B"), title))).toMatchObject({
        code: "InvalidInput",
        details: { field: "title" },
      });
    }
    await expect(client.pageGet(id("B"))).resolves.toMatchObject({ title: "" });
    expect(events).toEqual([]);
  });

  it("renames trashed pages too, and reports unknown pages", async () => {
    const { client, id } = await demo();
    await client.pageTrash(id("A"));
    await expect(client.pageRename(id("A1"), "Inside the trash")).resolves.toMatchObject({
      title: "Inside the trash",
    });
    expect(await failure(client.pageRename(UNKNOWN, "x"))).toMatchObject({ code: "NotFound" });
    // The title is checked before the page is looked up.
    expect(await failure(client.pageRename(UNKNOWN, "x".repeat(501)))).toMatchObject({ code: "InvalidInput" });
  });

  it("sets and clears icons, with the same rules as for spaces", async () => {
    const { client, id } = await demo();
    const withIcon = await client.pageSetIcon(id("B"), "  📌 ");
    expect(withIcon.icon).toBe("📌");
    const cleared = await client.pageSetIcon(id("B"), null);
    expect("icon" in cleared).toBe(false);
    expect(await failure(client.pageSetIcon(id("B"), "  "))).toMatchObject({
      message: "must not be empty; omit it to clear",
      details: { field: "icon" },
    });
    expect(await failure(client.pageSetIcon(id("B"), "🙂".repeat(9)))).toMatchObject({
      message: "must be at most 32 bytes",
    });
    expect(await failure(client.pageSetIcon(UNKNOWN, "📌"))).toMatchObject({ code: "NotFound" });
  });
});

describe("pages: move", () => {
  it("reorders within a parent: index is the position among the siblings after the move", async () => {
    const { client, id, outline: tree } = await demo();
    await client.pageMove(id("B"), null, 0);
    expect(await tree()).toEqual(["B", "A", "  A1", "    A1x", "  A2"]);
    await client.pageMove(id("B"), null, 1);
    expect(await tree()).toEqual(["A", "  A1", "    A1x", "  A2", "B"]);
    await client.pageMove(id("A"), null, 99);
    expect(await tree()).toEqual(["B", "A", "  A1", "    A1x", "  A2"]);
    await client.pageMove(id("A2"), id("A"), 0);
    expect(await tree()).toEqual(["B", "A", "  A2", "  A1", "    A1x"]);
    await client.pageMove(id("A1"), id("A"), 1);
    expect(await tree()).toEqual(["B", "A", "  A2", "  A1", "    A1x"]);
    await client.pageMove(id("A2"), id("A"), 1);
    expect(await tree()).toEqual(["B", "A", "  A1", "    A1x", "  A2"]);
  });

  it("reparents a page with its subtree at an index among the new siblings", async () => {
    const { client, spaceId, id, events, outline: tree } = await demo();
    events.length = 0;
    await client.pageMove(id("A2"), id("B"), 0);
    await client.pageMove(id("A1"), null, 1);
    expect(await tree()).toEqual(["A", "A1", "  A1x", "B", "  A2"]);
    await expect(client.pageGet(id("A1"))).resolves.not.toHaveProperty("parentId");
    await expect(client.pageGet(id("A2"))).resolves.toMatchObject({ parentId: id("B") });
    await expect(client.pageGet(id("A1x"))).resolves.toMatchObject({ parentId: id("A1") });
    expect(events).toEqual([
      { type: "StructureChanged", data: { spaceId } },
      { type: "StructureChanged", data: { spaceId } },
    ]);
  });

  it("moves a page into the root when the new parent is left out, appending past the end", async () => {
    const { client, id, outline: tree } = await demo();
    await client.pageMove(id("A1x"), null, 42);
    expect(await tree()).toEqual(["A", "  A1", "  A2", "B", "A1x"]);
  });

  it("refuses to move a page under itself or under one of its descendants", async () => {
    const { client, id, events, outline: tree } = await demo();
    events.length = 0;
    for (const parentId of [id("A"), id("A1"), id("A1x")]) {
      const error = await failure(client.pageMove(id("A"), parentId, 0));
      expect(error.toApiError(), parentId).toEqual({
        code: "InvalidInput",
        message: "a page cannot be moved under its own descendant",
        details: { field: "parentId" },
      });
    }
    expect(await tree()).toEqual(["A", "  A1", "    A1x", "  A2", "B"]);
    expect(events).toEqual([]);
    // Moving a descendant up, or a page next to its own subtree, is fine.
    await expect(client.pageMove(id("A1x"), id("A"), 0)).resolves.toBeUndefined();
  });

  it("refuses to move a trashed page or to move a page under a trashed parent", async () => {
    const { client, id } = await demo();
    await client.pageTrash(id("B"));
    expect((await failure(client.pageMove(id("B"), null, 0))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the page is in the trash",
      details: { field: "pageId" },
    });
    expect((await failure(client.pageMove(id("A2"), id("B"), 0))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the parent page does not exist or is in the trash",
      details: { field: "parentId" },
    });
    // A page inside a trashed subtree counts as trashed too.
    await client.pageTrash(id("A"));
    expect(await failure(client.pageMove(id("A1x"), null, 0))).toMatchObject({
      details: { field: "pageId" },
    });
  });

  it("refuses a parent in another space and a parent that does not exist", async () => {
    const { client, id } = await demo();
    const other = await client.spaceCreate("Other");
    const foreign = await client.pageCreate({ spaceId: other.id, title: "Foreign" });
    expect((await failure(client.pageMove(id("B"), foreign.id, 0))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the parent is in another space: use PageMoveToSpace",
      details: { field: "parentId" },
    });
    expect((await failure(client.pageMove(id("B"), UNKNOWN, 0))).toApiError()).toEqual({
      code: "NotFound",
      message: "page not found",
    });
  });

  it("reports unknown pages and positions that are not whole numbers", async () => {
    const { client, id, events } = await demo();
    events.length = 0;
    expect(await failure(client.pageMove(UNKNOWN, null, 0))).toMatchObject({ code: "NotFound" });
    for (const index of [-1, 0.5, Number.POSITIVE_INFINITY]) {
      expect(await failure(client.pageMove(id("B"), null, index))).toMatchObject({
        code: "InvalidInput",
        details: { field: "index" },
      });
    }
    expect(await failure(client.pageCreate({ spaceId: UNKNOWN, index: -2, title: "x" }))).toMatchObject({
      details: { field: "index" },
    });
    expect(events).toEqual([]);
  });
});

describe("pages: reorder the children (ADR-0052)", () => {
  it("puts the top-level pages and the children of a page in the given order, with their subtrees", async () => {
    const { client, spaceId, id, events, outline: tree } = await demo();
    events.length = 0;
    await client.pageReorderChildren(spaceId, null, [id("B"), id("A")]);
    expect(await tree()).toEqual(["B", "A", "  A1", "    A1x", "  A2"]);
    await client.pageReorderChildren(spaceId, id("A"), [id("A2"), id("A1")]);
    expect(await tree()).toEqual(["B", "A", "  A2", "  A1", "    A1x"]);
    await expect(client.pageGet(id("A1x"))).resolves.toMatchObject({ parentId: id("A1") });
    expect(events).toEqual([
      { type: "StructureChanged", data: { spaceId } },
      { type: "StructureChanged", data: { spaceId } },
    ]);
  });

  it("is one undo entry and one redo, and stamps no page as edited", async () => {
    const { client, spaceId, id } = await demo();
    const titles = ["A", "A1", "A1x", "A2", "B"];
    const stamps = async () => Promise.all(titles.map(async (title) => (await client.pageGet(id(title))).updatedAt));
    const before = await stamps();
    await client.pageReorderChildren(spaceId, null, [id("B"), id("A")]);
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: true, canRedo: false });
    expect(await stamps()).toEqual(before);
    expect(outline(await client.structureUndo(spaceId))).toEqual(["A", "  A1", "    A1x", "  A2", "B"]);
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: false, canRedo: true });
    expect(outline(await client.structureRedo(spaceId))).toEqual(["B", "A", "  A1", "    A1x", "  A2"]);
  });

  it("writes nothing when the children already stand in that order: no event, no undo entry", async () => {
    const { client, spaceId, id, events } = await demo();
    events.length = 0;
    await client.pageReorderChildren(spaceId, null, [id("A"), id("B")]);
    await client.pageReorderChildren(spaceId, id("A"), [id("A1"), id("A2")]);
    await client.pageReorderChildren(spaceId, id("B"), []);
    expect(events).toEqual([]);
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: false, canRedo: false });
  });

  it("refuses a set that is not the live children and changes nothing", async () => {
    const { client, spaceId, id, events, outline: tree } = await demo();
    await client.pageTrash(id("A2"));
    events.length = 0;
    const wrong: [string, string[]][] = [
      ["a page missing", []],
      ["a page of another parent", [id("A1"), id("B")]],
      ["a repeated page", [id("A1"), id("A1")]],
      ["the trashed page", [id("A1"), id("A2")]],
      ["an unknown page", [UNKNOWN]],
    ];
    for (const [what, order] of wrong) {
      expect((await failure(client.pageReorderChildren(spaceId, id("A"), order))).toApiError(), what).toEqual({
        code: "InvalidInput",
        message: "the order is not the current children of the parent",
        details: { field: "order" },
      });
    }
    expect(events).toEqual([]);
    expect(await tree()).toEqual(["A", "  A1", "    A1x", "B"]);
    // The trashed sibling is not a live child: the live ones alone are the right set.
    await expect(client.pageReorderChildren(spaceId, id("A"), [id("A1")])).resolves.toBeUndefined();
  });

  it("refuses an unknown space with NotFound and a parent that is not a live page of it with the parentId field", async () => {
    const { client, spaceId, id } = await demo();
    expect(await failure(client.pageReorderChildren(UNKNOWN, null, []))).toMatchObject({ code: "NotFound" });
    const other = await client.spaceCreate("Other");
    const foreign = await client.pageCreate({ spaceId: other.id, title: "Foreign" });
    for (const parentId of [UNKNOWN, foreign.id]) {
      expect(await failure(client.pageReorderChildren(spaceId, parentId, []))).toMatchObject({
        code: "InvalidInput",
        details: { field: "parentId" },
      });
    }
    await client.pageTrash(id("B"));
    expect(await failure(client.pageReorderChildren(spaceId, id("B"), []))).toMatchObject({
      details: { field: "parentId" },
    });
  });
});

describe("pages: trash", () => {
  it("moves the page with its subtree to the trash and marks every page of it as trashed", async () => {
    const { client, spaceId, id, events, outline: tree } = await demo();
    events.length = 0;
    await expect(client.pageTrash(id("A"))).resolves.toBeUndefined();
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId } }]);
    expect(await tree()).toEqual(["B"]);

    const { trashed } = await client.treeGet(spaceId);
    expect(trashed).toHaveLength(1);
    const entry = trashed[0];
    expect(entry).toMatchObject({ descendantCount: 3, page: { id: id("A"), title: "A" } });
    expect(entry).not.toHaveProperty("originalParentId");
    expect(entry?.page).not.toHaveProperty("parentId");
    const trashedAt = entry?.page.trashedAt;
    expect(trashedAt).toBeTypeOf("number");
    // Every page of the subtree reports the same trashedAt and keeps its parent.
    for (const title of ["A1", "A1x", "A2"]) {
      expect((await client.pageGet(id(title))).trashedAt, title).toBe(trashedAt);
    }
    await expect(client.pageGet(id("A1x"))).resolves.toMatchObject({ parentId: id("A1") });
  });

  it("lists the most recently trashed first, also within one millisecond, with the original parent", async () => {
    const { client, spaceId, id } = await demo({ now: () => T0 });
    await client.pageTrash(id("A2"));
    await client.pageTrash(id("B"));
    await client.pageTrash(id("A1"));
    const { trashed } = await client.treeGet(spaceId);
    expect(trashed.map((entry) => entry.page.title)).toEqual(["A1", "B", "A2"]);
    expect(trashed.map((entry) => entry.originalParentId)).toEqual([id("A"), undefined, id("A")]);
    expect(trashed.map((entry) => entry.descendantCount)).toEqual([1, 0, 0]);
  });

  it("reports the original parent only while that page still exists", async () => {
    const { client, spaceId, id } = await demo();
    await client.pageTrash(id("A2"));
    await expect(client.treeGet(spaceId)).resolves.toMatchObject({ trashed: [{ originalParentId: id("A") }] });
    await client.pagePurge(id("A"));
    const { trashed } = await client.treeGet(spaceId);
    expect(trashed).toHaveLength(1);
    expect(trashed[0]).not.toHaveProperty("originalParentId");
  });

  it("refuses to trash a page that is in the trash or inside a trashed page", async () => {
    const { client, spaceId, id, events } = await demo();
    await client.pageTrash(id("A"));
    events.length = 0;
    for (const title of ["A", "A1x"]) {
      expect((await failure(client.pageTrash(id(title)))).toApiError(), title).toEqual({
        code: "InvalidInput",
        message: "the page is in the trash",
        details: { field: "pageId" },
      });
    }
    expect(await failure(client.pageTrash(UNKNOWN))).toMatchObject({
      code: "NotFound",
      message: "page not found",
    });
    expect(events).toEqual([]);
    expect((await client.treeGet(spaceId)).trashed).toHaveLength(1);
  });
});

describe("pages: restore", () => {
  it("returns the page and its subtree to the previous parent, after its siblings", async () => {
    const { client, spaceId, id, events, outline: tree } = await demo();
    await client.pageTrash(id("A1"));
    expect(await tree()).toEqual(["A", "  A2", "B"]);
    await client.pageCreate({ spaceId, parentId: id("A"), title: "A3" });
    events.length = 0;
    const restored = await client.pageRestore(id("A1"));
    expect(restored).toMatchObject({ id: id("A1"), parentId: id("A") });
    expect(restored).not.toHaveProperty("trashedAt");
    expect(await tree()).toEqual(["A", "  A2", "  A3", "  A1", "    A1x", "B"]);
    await expect(client.pageGet(id("A1x"))).resolves.not.toHaveProperty("trashedAt");
    await expect(client.treeGet(spaceId)).resolves.toMatchObject({ trashed: [] });
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId } }]);
  });

  it("returns a root page to the end of the root", async () => {
    const { client, id, outline: tree } = await demo();
    await client.pageTrash(id("A"));
    await client.pageRestore(id("A"));
    expect(await tree()).toEqual(["B", "A", "  A1", "    A1x", "  A2"]);
  });

  it("falls back to the end of the root while the previous parent is in the trash", async () => {
    const { client, spaceId, id, outline: tree } = await demo();
    await client.pageTrash(id("A1"));
    await client.pageTrash(id("A"));
    const restored = await client.pageRestore(id("A1"));
    expect(restored).not.toHaveProperty("parentId");
    expect(await tree()).toEqual(["B", "A1", "  A1x"]);
    await expect(client.treeGet(spaceId)).resolves.toMatchObject({ trashed: [{ page: { id: id("A") } }] });
    await client.pageRestore(id("A"));
    expect(await tree()).toEqual(["B", "A1", "  A1x", "A", "  A2"]);
  });

  it("falls back to the end of the root when the previous parent was purged", async () => {
    const { client, id, outline: tree } = await demo();
    await client.pageTrash(id("A2"));
    await client.pagePurge(id("A"));
    await expect(client.pageRestore(id("A2"))).resolves.not.toHaveProperty("parentId");
    expect(await tree()).toEqual(["B", "A2"]);
  });

  it("goes back under the previous parent once that page is live again", async () => {
    const { client, id, outline: tree } = await demo();
    await client.pageTrash(id("A2"));
    await client.pageTrash(id("A"));
    await client.pageRestore(id("A"));
    await client.pageRestore(id("A2"));
    expect(await tree()).toEqual(["B", "A", "  A1", "    A1x", "  A2"]);
  });

  it("refuses pages that are live and pages inside a trashed subtree", async () => {
    const { client, id, events } = await demo();
    expect((await failure(client.pageRestore(id("B")))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the page is not in the trash",
      details: { field: "pageId" },
    });
    await client.pageTrash(id("A"));
    events.length = 0;
    expect((await failure(client.pageRestore(id("A1")))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the page is in the trash",
      details: { field: "pageId" },
    });
    expect(await failure(client.pageRestore(UNKNOWN))).toMatchObject({ code: "NotFound" });
    expect(events).toEqual([]);
  });
});

describe("pages: purge", () => {
  it("removes a trashed page and its subtree for good", async () => {
    const { client, spaceId, id, events, outline: tree } = await demo();
    await client.pageTrash(id("A"));
    events.length = 0;
    await expect(client.pagePurge(id("A"))).resolves.toBeUndefined();
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId } }]);
    for (const title of ["A", "A1", "A1x", "A2"]) {
      expect((await failure(client.pageGet(id(title)))).toApiError(), title).toEqual({
        code: "NotFound",
        message: "page not found",
      });
    }
    expect(await tree()).toEqual(["B"]);
    await expect(client.treeGet(spaceId)).resolves.toMatchObject({ trashed: [] });
  });

  it("also removes a live page with everything below it", async () => {
    const { client, id, outline: tree } = await demo();
    await client.pagePurge(id("A1"));
    expect(await tree()).toEqual(["A", "  A2", "B"]);
    await expect(client.pageGet(id("A1x"))).rejects.toMatchObject({ code: "NotFound" });
    await expect(client.spaceList()).resolves.toMatchObject([{ pageCount: 3 }]);
  });

  it("leaves a purged page unreachable for every other request", async () => {
    const { client, id } = await demo();
    await client.pagePurge(id("B"));
    const attempts: Promise<unknown>[] = [
      client.pageRename(id("B"), "x"),
      client.pageSetIcon(id("B"), "📌"),
      client.pageMove(id("B"), null, 0),
      client.pageTrash(id("B")),
      client.pageRestore(id("B")),
      client.pagePurge(id("B")),
    ];
    for (const attempt of attempts) {
      expect(await failure(attempt)).toMatchObject({ code: "NotFound", message: "page not found" });
    }
  });
});

describe("structural undo and redo", () => {
  it("has nothing to undo or redo after seeding, and undo then changes nothing", async () => {
    const { client, spaceId, events } = await demo();
    const before = await client.treeGet(spaceId);
    expect(before.undo).toEqual({ canUndo: false, canRedo: false });
    await expect(client.structureUndo(spaceId)).resolves.toEqual(before);
    await expect(client.structureRedo(spaceId)).resolves.toEqual(before);
    expect(events).toEqual([]);
  });

  it("undoes and redoes a creation, keeping the page id", async () => {
    const { client, spaceId } = await demo();
    const created = await client.pageCreate({ spaceId, title: "New" });
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: true, canRedo: false });

    const undone = await client.structureUndo(spaceId);
    expect(undone.undo).toEqual({ canUndo: false, canRedo: true });
    expect(outline(undone)).toEqual(["A", "  A1", "    A1x", "  A2", "B"]);
    expect(await failure(client.pageGet(created.id))).toMatchObject({ code: "NotFound" });

    const redone = await client.structureRedo(spaceId);
    expect(redone.undo).toEqual({ canUndo: true, canRedo: false });
    expect(outline(redone)).toEqual(["A", "  A1", "    A1x", "  A2", "B", "New"]);
    await expect(client.pageGet(created.id)).resolves.toEqual(created);
  });

  it("undoes and redoes every kind of structural change", async () => {
    const { client, spaceId, id } = await demo();
    const steps: [string, () => Promise<unknown>][] = [
      ["rename", () => client.pageRename(id("B"), "B renamed")],
      ["icon", () => client.pageSetIcon(id("B"), "📌")],
      ["move", () => client.pageMove(id("A2"), id("B"), 0)],
      ["trash", () => client.pageTrash(id("A1"))],
      ["restore", () => client.pageRestore(id("A1"))],
      ["purge", () => client.pagePurge(id("A1"))],
    ];
    const contents = async () => {
      const { roots, trashed } = await client.treeGet(spaceId);
      return { roots, trashed };
    };
    const before: Awaited<ReturnType<typeof contents>>[] = [];
    const after: Awaited<ReturnType<typeof contents>>[] = [];
    for (const [, run] of steps) {
      before.push(await contents());
      await run();
      after.push(await contents());
    }

    for (const [index, [name]] of [...steps.entries()].reverse()) {
      const undone = await client.structureUndo(spaceId);
      expect({ roots: undone.roots, trashed: undone.trashed }, `undo ${name}`).toEqual(before[index]);
    }
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: false, canRedo: true });
    for (const [index, [name]] of steps.entries()) {
      const redone = await client.structureRedo(spaceId);
      expect({ roots: redone.roots, trashed: redone.trashed }, `redo ${name}`).toEqual(after[index]);
    }
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: true, canRedo: false });
  });

  it("brings a purged subtree back when the purge is undone", async () => {
    const { client, spaceId, id } = await demo();
    await client.pagePurge(id("A"));
    await expect(client.pageGet(id("A1x"))).rejects.toMatchObject({ code: "NotFound" });
    await client.structureUndo(spaceId);
    await expect(client.pageGet(id("A1x"))).resolves.toMatchObject({ parentId: id("A1") });
    expect(outline(await client.treeGet(spaceId))).toEqual(["A", "  A1", "    A1x", "  A2", "B"]);
  });

  it("clears the redo history when a new change is made", async () => {
    const { client, spaceId } = await demo();
    await client.pageCreate({ spaceId, title: "X" });
    await client.structureUndo(spaceId);
    expect((await client.treeGet(spaceId)).undo.canRedo).toBe(true);
    await client.pageCreate({ spaceId, title: "Y" });
    const view = await client.treeGet(spaceId);
    expect(view.undo).toEqual({ canUndo: true, canRedo: false });
    const redone = await client.structureRedo(spaceId);
    expect(outline(redone)).toEqual(["A", "  A1", "    A1x", "  A2", "B", "Y"]);
  });

  it("emits StructureChanged only when there was something to undo or redo", async () => {
    const { client, spaceId, events } = await demo();
    await client.pageCreate({ spaceId, title: "x" });
    events.length = 0;
    const changed = { type: "StructureChanged", data: { spaceId } };

    await client.structureUndo(spaceId);
    expect(events).toEqual([changed]);
    await client.structureUndo(spaceId);
    expect(events).toEqual([changed]);
    await client.structureRedo(spaceId);
    expect(events).toEqual([changed, changed]);
    await client.structureRedo(spaceId);
    expect(events).toEqual([changed, changed]);
  });

  it("keeps the last 100 steps", async () => {
    const { client } = setup();
    const space = await client.spaceCreate("Long");
    for (let i = 0; i <= 100; i += 1) {
      await client.pageCreate({ spaceId: space.id, title: `P${String(i)}` });
    }
    let undone = 0;
    while ((await client.treeGet(space.id)).undo.canUndo) {
      await client.structureUndo(space.id);
      undone += 1;
    }
    expect(undone).toBe(100);
    expect(outline(await client.treeGet(space.id))).toEqual(["P0"]);
  });

  it("keeps a separate history per space", async () => {
    const { client } = setup();
    const one = await client.spaceCreate("One");
    const two = await client.spaceCreate("Two");
    await client.pageCreate({ spaceId: one.id, title: "In one" });
    await client.pageCreate({ spaceId: two.id, title: "In two" });
    await client.structureUndo(one.id);
    expect(outline(await client.treeGet(one.id))).toEqual([]);
    expect(outline(await client.treeGet(two.id))).toEqual(["In two"]);
    expect((await client.treeGet(two.id)).undo).toEqual({ canUndo: true, canRedo: false });
  });

  it("does not record failed changes, reads, or changes to the space itself", async () => {
    const { client, spaceId, id } = await demo();
    await failure(client.pageMove(id("A"), id("A1"), 0));
    await failure(client.pageRename(id("A"), "x".repeat(501)));
    await failure(client.pageTrash(UNKNOWN));
    await client.pageGet(id("A"));
    await client.treeGet(spaceId);
    await client.spaceRename(spaceId, "Renamed");
    await client.spaceSetIcon(spaceId, "🏠");
    expect((await client.treeGet(spaceId)).undo).toEqual({ canUndo: false, canRedo: false });
  });

  it("reports an unknown space", async () => {
    const { client } = setup();
    for (const call of [client.structureUndo(UNKNOWN), client.structureRedo(UNKNOWN)]) {
      expect((await failure(call)).toApiError()).toEqual({ code: "NotFound", message: "space not found" });
    }
  });
});

describe("structure events", () => {
  it("reach listeners before the dispatch promise resolves", async () => {
    const { transport, client, events } = setup();
    const space = await client.spaceCreate("S");
    events.length = 0;
    const pending = transport.dispatch({ type: "PageCreate", data: { spaceId: space.id, title: "x" } });
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId: space.id } }]);
    await pending;
  });

  it("are not emitted by reads or failed requests", async () => {
    const { client, spaceId, id, events } = await demo();
    await client.spaceList();
    await client.treeGet(spaceId);
    await client.pageGet(id("A"));
    await failure(client.pageCreate({ spaceId, title: "x".repeat(501) }));
    await failure(client.pageRestore(id("A")));
    expect(events).toEqual([]);
  });

  it("do not break a request when a listener throws", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { transport, client } = setup();
    transport.subscribe(() => {
      throw new Error("listener bug");
    });
    await expect(client.spaceCreate("Still works")).resolves.toMatchObject({ name: "Still works" });
    expect(logged).toHaveBeenCalledOnce();
    logged.mockRestore();
  });
});

describe("seeded spaces", () => {
  it("creates spaces and nested pages up front, without events or undo history", async () => {
    const { client, events } = setup({
      spaces: [
        { name: "Personal", icon: "🏠", pages: [{ title: "Notes", icon: "📝", children: [{ title: "Today" }] }] },
        { name: "Work" },
      ],
    });
    const spaces = await client.spaceList();
    expect(names(spaces)).toEqual(["Personal", "Work"]);
    expect(spaces.map((space) => space.icon)).toEqual(["🏠", undefined]);
    expect(spaces.map((space) => space.pageCount)).toEqual([2, 0]);
    const [personal] = spaces;
    const view = await client.treeGet(personal?.id ?? "");
    expect(outline(view)).toEqual(["Notes", "  Today"]);
    expect(view.roots[0]?.page.icon).toBe("📝");
    expect(view.undo).toEqual({ canUndo: false, canRedo: false });
    expect(events).toEqual([]);
  });

  it("validates seeds like requests", () => {
    expect(() => new MemoryTransport({ spaces: [{ name: "  " }] })).toThrow(CoreError);
    expect(() => new MemoryTransport({ spaces: [{ name: "S", pages: [{ title: "x".repeat(501) }] }] })).toThrow(CoreError);
    expect(() => new MemoryTransport({ spaces: [{ name: "S", icon: "  " }] })).toThrow(CoreError);
  });

  it("works with or without a workspace", async () => {
    const { client } = setup({ workspaceName: "Home", spaces: DEMO });
    await expect(client.workspaceGet()).resolves.toMatchObject({ name: "Home" });
    await expect(client.spaceList()).resolves.toHaveLength(1);
    const bare = setup({ spaces: DEMO });
    await expect(bare.client.workspaceGet()).resolves.toBeNull();
    await expect(bare.client.spaceList()).resolves.toHaveLength(1);
  });
});

describe("structure requests and the core lifecycle", () => {
  it("are rejected while the core is opening and served once it is ready", async () => {
    const { transport, client } = setup({ coreState: "opening", spaces: DEMO });
    expect(await failure(client.spaceList())).toMatchObject({ code: "Internal", message: "core is starting" });
    transport.setCoreState("ready");
    await expect(client.spaceList()).resolves.toHaveLength(1);
  });

  it("can be made to fail by failNext", async () => {
    const { transport, client, spaceId } = await demo();
    transport.failNext({ code: "Storage", message: "disk full" });
    expect(await failure(client.treeGet(spaceId))).toMatchObject({ code: "Storage", message: "disk full" });
    await expect(client.treeGet(spaceId)).resolves.toMatchObject({ spaceId });
  });
});
