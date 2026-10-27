// Favourite pages in `MemoryTransport` (ADR-0029): `PageFavoriteSet`, `PageFavoriteMove` and
// `TreeView.favorites` behave as `kb-core` and `kb-model`'s `favorites.rs` do: order, idempotence,
// the trash (hidden, back with a restore), purge (dropped), undo (never touched), the seed flag
// and backups.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { PageNode } from "./generated/PageNode";
import type { TreeView } from "./generated/TreeView";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);

/** Deterministic ids (`prefix` tells installations apart) and clock. */
function setup(options: MemoryTransportOptions = {}, prefix = "00000000") {
  let counter = 0;
  let clock = T0;
  const transport = new MemoryTransport({
    newId: () => `${prefix}-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
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

function idsByTitle(view: TreeView): (title: string) => string {
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

const DEMO: SeedSpace[] = [
  {
    name: "Personal",
    pages: [{ title: "A", children: [{ title: "A1" }] }, { title: "B" }, { title: "C" }],
  },
];

/** A space with the pages A (A1), B and C; `titles` maps a title to its page id. */
async function demo(spaces: SeedSpace[] = DEMO) {
  const ctx = setup({ spaces });
  const [space] = await ctx.client.spaceList();
  if (space === undefined) {
    throw new Error("the demo space is missing");
  }
  const id = idsByTitle(await ctx.client.treeGet(space.id));
  /** Titles of the live favourites, in order. */
  const favorites = async (): Promise<string[]> => {
    const view = await ctx.client.treeGet(space.id);
    const titles = new Map<string, string>();
    for (const title of ["A", "A1", "B", "C"]) {
      titles.set(id(title), title);
    }
    return (view.favorites ?? []).map((pageId) => titles.get(pageId) ?? pageId);
  };
  return { ...ctx, spaceId: space.id, id, favorites };
}

describe("favourites: add, remove, order", () => {
  it("starts without favourites and leaves the list out of the tree", async () => {
    const { client, spaceId } = await demo();
    const view = await client.treeGet(spaceId);
    expect(view.favorites).toBeUndefined();
    expect("favorites" in view).toBe(false);
  });

  it("lists the favourites in the order they were added and emits StructureChanged for each change", async () => {
    const { client, events, id, favorites, spaceId } = await demo();
    events.length = 0;
    await client.pageFavoriteSet(id("C"), true);
    await client.pageFavoriteSet(id("A"), true);
    await client.pageFavoriteSet(id("B"), true);
    await expect(favorites()).resolves.toEqual(["C", "A", "B"]);
    expect(events).toEqual(Array.from({ length: 3 }, () => ({ type: "StructureChanged", data: { spaceId } })));
    // The page order of the tree does not depend on favourites.
    const view = await client.treeGet(spaceId);
    expect(view.roots.map((node) => node.page.title)).toEqual(["A", "B", "C"]);
  });

  it("is idempotent: adding keeps the place, removing what is not a favourite is fine", async () => {
    const { client, events, id, favorites } = await demo();
    await client.pageFavoriteSet(id("A"), true);
    await client.pageFavoriteSet(id("B"), true);
    events.length = 0;
    await client.pageFavoriteSet(id("A"), true);
    await expect(favorites()).resolves.toEqual(["A", "B"]);
    await client.pageFavoriteSet(id("C"), false);
    await client.pageFavoriteSet(id("A"), false);
    await client.pageFavoriteSet(id("A"), false);
    await expect(favorites()).resolves.toEqual(["B"]);
    expect(events).toHaveLength(4);
    await client.pageFavoriteSet(id("B"), false);
    await expect(favorites()).resolves.toEqual([]);
    // Adding again puts the page last.
    await client.pageFavoriteSet(id("B"), true);
    await client.pageFavoriteSet(id("A"), true);
    await expect(favorites()).resolves.toEqual(["B", "A"]);
  });

  it("reports unknown pages as NotFound and refuses to add a page in the trash", async () => {
    const { client, id, favorites, events } = await demo();
    for (const favorite of [true, false]) {
      expect(await failure(client.pageFavoriteSet(UNKNOWN, favorite))).toMatchObject({ code: "NotFound" });
    }
    expect(await failure(client.pageFavoriteMove(UNKNOWN))).toMatchObject({ code: "NotFound" });
    await client.pageTrash(id("A"));
    await client.pageTrash(id("B"));
    events.length = 0;
    // A trashed page, and a page inside a trashed subtree.
    for (const title of ["A", "A1", "B"]) {
      const error = await failure(client.pageFavoriteSet(id(title), true));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    }
    expect(events).toEqual([]);
    await expect(favorites()).resolves.toEqual([]);
    // Removing works for a trashed page.
    await client.pageFavoriteSet(id("A"), false);
  });
});

describe("favourites: move", () => {
  async function three() {
    const ctx = await demo();
    for (const title of ["A", "B", "C"]) {
      await ctx.client.pageFavoriteSet(ctx.id(title), true);
    }
    ctx.events.length = 0;
    return ctx;
  }

  it("puts a favourite in front of another one, or last", async () => {
    const { client, id, favorites, events, spaceId } = await three();
    await client.pageFavoriteMove(id("C"), id("A"));
    await expect(favorites()).resolves.toEqual(["C", "A", "B"]);
    await client.pageFavoriteMove(id("C"), id("B"));
    await expect(favorites()).resolves.toEqual(["A", "C", "B"]);
    await client.pageFavoriteMove(id("A"));
    await expect(favorites()).resolves.toEqual(["C", "B", "A"]);
    await client.pageFavoriteMove(id("B"), null);
    await expect(favorites()).resolves.toEqual(["C", "A", "B"]);
    expect(events).toEqual(Array.from({ length: 4 }, () => ({ type: "StructureChanged", data: { spaceId } })));
  });

  it("accepts a move that changes nothing", async () => {
    const { client, id, favorites } = await three();
    await client.pageFavoriteMove(id("A"), id("B"));
    await client.pageFavoriteMove(id("A"), id("A"));
    await client.pageFavoriteMove(id("C"));
    await expect(favorites()).resolves.toEqual(["A", "B", "C"]);
  });

  it("refuses pages that are not favourites, naming the field", async () => {
    const { client, id, favorites } = await demo();
    await client.pageFavoriteSet(id("A"), true);
    await client.pageFavoriteSet(id("B"), true);
    expect(await failure(client.pageFavoriteMove(id("C")))).toMatchObject({
      code: "InvalidInput",
      details: { field: "pageId" },
    });
    expect(await failure(client.pageFavoriteMove(id("A"), id("C")))).toMatchObject({
      code: "InvalidInput",
      details: { field: "before" },
    });
    expect(await failure(client.pageFavoriteMove(id("A"), UNKNOWN))).toMatchObject({
      code: "InvalidInput",
      details: { field: "before" },
    });
    await expect(favorites()).resolves.toEqual(["A", "B"]);
  });

  it("does not take a favourite of another space as a place to go", async () => {
    const { client, id, spaceId, favorites } = await demo();
    const other = await client.spaceCreate("Other");
    const elsewhere = await client.pageCreate({ spaceId: other.id, title: "Elsewhere" });
    await client.pageFavoriteSet(id("A"), true);
    await client.pageFavoriteSet(elsewhere.id, true);
    expect((await client.treeGet(other.id)).favorites).toEqual([elsewhere.id]);
    expect((await client.treeGet(spaceId)).favorites).toEqual([id("A")]);
    expect(await failure(client.pageFavoriteMove(id("A"), elsewhere.id))).toMatchObject({
      details: { field: "before" },
    });
    await expect(favorites()).resolves.toEqual(["A"]);
  });

  it("skips hidden favourites as places to go", async () => {
    const { client, id, favorites } = await three();
    await client.pageTrash(id("B"));
    await expect(favorites()).resolves.toEqual(["A", "C"]);
    expect(await failure(client.pageFavoriteMove(id("C"), id("B")))).toMatchObject({
      details: { field: "before" },
    });
    await client.pageFavoriteMove(id("C"), id("A"));
    await expect(favorites()).resolves.toEqual(["C", "A"]);
    await client.pageRestore(id("B"));
    await expect(favorites()).resolves.toHaveLength(3);
  });
});

describe("favourites: trash, purge, undo", () => {
  it("hides the favourites of a trashed subtree and brings them back with the restore", async () => {
    const { client, id, favorites } = await demo();
    for (const title of ["A", "A1", "C"]) {
      await client.pageFavoriteSet(id(title), true);
    }
    await client.pageTrash(id("A"));
    await expect(favorites()).resolves.toEqual(["C"]);
    await client.pageRestore(id("A"));
    await expect(favorites()).resolves.toEqual(["A", "A1", "C"]);
  });

  it("drops the favourites of a purged page and of its subtree", async () => {
    const { client, id, favorites } = await demo();
    for (const title of ["A", "A1", "C"]) {
      await client.pageFavoriteSet(id(title), true);
    }
    await client.pageTrash(id("A"));
    await client.pageTrash(id("C"));
    await client.pagePurge(id("A"));
    await client.pageRestore(id("C"));
    await expect(favorites()).resolves.toEqual(["C"]);
    expect(await failure(client.pageFavoriteSet(id("A1"), true))).toMatchObject({ code: "NotFound" });
  });

  it("is not on the undo history, and undo and redo never touch it", async () => {
    const { client, id, favorites, spaceId } = await demo();
    // Seeding leaves no history; favourite writes add none.
    await client.pageFavoriteSet(id("A"), true);
    await client.pageFavoriteSet(id("C"), true);
    await client.pageFavoriteMove(id("C"), id("A"));
    await expect(client.treeGet(spaceId)).resolves.toMatchObject({ undo: { canUndo: false, canRedo: false } });
    expect(await client.structureUndo(spaceId)).toMatchObject({ favorites: [id("C"), id("A")] });

    // Between two structural edits: undo walks back through the edits only.
    const created = await client.pageCreate({ spaceId, title: "New" });
    await client.pageFavoriteSet(created.id, true);
    await expect(favorites()).resolves.toEqual(["C", "A", created.id]);
    const undone = await client.structureUndo(spaceId);
    expect(undone.roots.map((node) => node.page.title)).toEqual(["A", "B", "C"]);
    expect(undone.favorites).toEqual([id("C"), id("A")]);
    const redone = await client.structureRedo(spaceId);
    expect(redone.favorites).toEqual([id("C"), id("A"), created.id]);
  });

  it("does not bring a purged page's favourite back when the purge is undone", async () => {
    const { client, id, favorites, spaceId } = await demo();
    await client.pageFavoriteSet(id("B"), true);
    await client.pageFavoriteSet(id("C"), true);
    await client.pagePurge(id("B"));
    await expect(favorites()).resolves.toEqual(["C"]);
    const undone = await client.structureUndo(spaceId);
    expect(undone.roots.map((node) => node.page.title)).toEqual(["A", "B", "C"]);
    expect(undone.favorites).toEqual([id("C")]);
  });
});

describe("favourites: seed and backups", () => {
  it("marks seeded pages as favourites in creation order, without events or history", async () => {
    const seed: SeedSpace[] = [
      {
        name: "Personal",
        pages: [
          { title: "A", favorite: true, children: [{ title: "A1", favorite: true }] },
          { title: "B" },
          { title: "C", favorite: true },
        ],
      },
    ];
    const { client, events, favorites, spaceId } = await demo(seed);
    await expect(favorites()).resolves.toEqual(["A", "A1", "C"]);
    expect(events).toEqual([]);
    await expect(client.treeGet(spaceId)).resolves.toMatchObject({ undo: { canUndo: false } });
  });

  it("carries the favourites, hidden ones included, through a backup", async () => {
    const seed: SeedSpace[] = [
      { name: "Personal", pages: [{ title: "A", favorite: true }, { title: "B", favorite: true }, { title: "C" }] },
    ];
    const origin = setup({
      workspaceName: "Research",
      spaces: seed,
      security: { kitCreatedAt: T0 - 10_000, kitVerifiedAt: T0 - 9_000, kitKey: KEY },
    });
    const [space] = await origin.client.spaceList();
    if (space === undefined) {
      throw new Error("the space is missing");
    }
    const id = idsByTitle(await origin.client.treeGet(space.id));
    await origin.client.pageFavoriteSet(id("C"), true);
    await origin.client.pageFavoriteMove(id("C"), id("A"));
    await origin.client.pageTrash(id("B"));
    const before = await origin.client.treeGet(space.id);
    expect(before.favorites).toEqual([id("C"), id("A")]);

    await origin.client.backupExport("C:/backups/favourites.atomikkb");
    const restored = setup({ files: origin.transport.files }, "11111111");
    await restored.client.backupImport(undefined, (await kitWordsFromEntropy(KEY)).join(" "));
    const after = await restored.client.treeGet(space.id);
    expect(after.favorites).toEqual([id("C"), id("A")]);
    // The hidden favourite came along: it is back with the restore.
    await restored.client.pageRestore(id("B"));
    expect((await restored.client.treeGet(space.id)).favorites).toEqual([id("C"), id("A"), id("B")]);
  });

  it("restores a backup without favourites as a space without favourites", async () => {
    const origin = setup({
      workspaceName: "Research",
      spaces: DEMO,
      security: { kitCreatedAt: T0 - 10_000, kitVerifiedAt: T0 - 9_000, kitKey: KEY },
    });
    const [space] = await origin.client.spaceList();
    if (space === undefined) {
      throw new Error("the space is missing");
    }
    await origin.client.backupExport("C:/backups/plain.atomikkb");
    const restored = setup({ files: origin.transport.files }, "11111111");
    await restored.client.backupImport(undefined, (await kitWordsFromEntropy(KEY)).join(" "));
    expect((await restored.client.treeGet(space.id)).favorites).toBeUndefined();
  });
});
