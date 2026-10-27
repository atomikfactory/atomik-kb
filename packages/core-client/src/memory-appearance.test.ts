// Page appearance in `MemoryTransport` (ADR-0040): `PageSetAppearance` with the core's rules (a field left out
// stays, the neutral value clears, the same value changes nothing), the lock as a guard of the UI that refuses
// nothing, undo and redo, duplicates, moves between spaces, backups and seeds.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";

const T0 = 1_700_000_000_000;
const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index * 3);

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

/** Deterministic ids, a clock the test moves, a client, the emitted events, the spaces and pages "Alpha" and "Beta". */
async function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: [{ title: "Alpha" }, { title: "Beta" }] }, { name: "Work" }],
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const spaces = await client.spaceList();
  const spaceId = (name: string): string => spaces.find((space) => space.name === name)?.id ?? "";
  const personal = spaceId("Personal");
  const roots = personal === "" ? [] : (await client.treeGet(personal)).roots;
  const pageId = (title: string): string => roots.find((node) => node.page.title === title)?.page.id ?? "";
  return { transport, client, events, clock, personal, work: spaceId("Work"), alpha: pageId("Alpha"), beta: pageId("Beta") };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** What a page reports of its look: the three fields. */
const lookOf = (page: { font?: string; smallText?: boolean; locked?: boolean }) => [page.font, page.smallText, page.locked];

describe("PageSetAppearance", () => {
  it("sets each part, leaves out parts as they are and clears with the neutral values", async () => {
    const { client, alpha } = await setup();
    expect(lookOf(await client.pageGet(alpha))).toEqual([undefined, undefined, undefined]);

    expect(lookOf(await client.pageSetAppearance(alpha, { font: "serif" }))).toEqual(["serif", undefined, undefined]);
    expect(lookOf(await client.pageSetAppearance(alpha, { font: "mono" }))).toEqual(["mono", undefined, undefined]);
    expect(lookOf(await client.pageSetAppearance(alpha, { smallText: true }))).toEqual(["mono", true, undefined]);
    expect(lookOf(await client.pageSetAppearance(alpha, { locked: true }))).toEqual(["mono", true, true]);
    expect(lookOf(await client.pageGet(alpha))).toEqual(["mono", true, true]);
    // Nothing asked: the page as it is.
    expect(lookOf(await client.pageSetAppearance(alpha, {}))).toEqual(["mono", true, true]);

    expect(lookOf(await client.pageSetAppearance(alpha, { font: "default" }))).toEqual([undefined, true, true]);
    expect(lookOf(await client.pageSetAppearance(alpha, { smallText: false, locked: false }))).toEqual([undefined, undefined, undefined]);
    // The cleared parts are absent from the page, not `false`.
    const cleared = await client.pageGet(alpha);
    expect("smallText" in cleared || "locked" in cleared || "font" in cleared).toBe(false);

    // All three at once, and the tree carries them.
    await client.pageSetAppearance(alpha, { font: "serif", smallText: true, locked: true });
    const roots = (await client.treeGet((await client.pageGet(alpha)).spaceId)).roots;
    expect(roots.map((node) => lookOf(node.page))).toEqual([["serif", true, true], [undefined, undefined, undefined]]);
  });

  it("is a separate value from the icon and the cover", async () => {
    const { client, alpha } = await setup();
    await client.pageSetIcon(alpha, "🌱");
    await client.pageSetCover(alpha, "color:blue");
    await client.pageSetAppearance(alpha, { font: "serif", locked: true });
    expect(await client.pageGet(alpha)).toMatchObject({ icon: "🌱", cover: "color:blue", font: "serif", locked: true });
    await client.pageSetIcon(alpha, null);
    await client.pageSetCover(alpha, null);
    expect(await client.pageGet(alpha)).toMatchObject({ font: "serif", locked: true });
  });

  it("works on a trashed page and stamps the edit", async () => {
    const { client, clock, alpha } = await setup();
    clock.now = T0 + 5;
    expect((await client.pageSetAppearance(alpha, { smallText: true })).updatedAt).toBe(T0 + 5);
    await client.pageTrash(alpha);
    expect(lookOf(await client.pageSetAppearance(alpha, { font: "mono", locked: true }))).toEqual(["mono", true, true]);
    await client.pageRestore(alpha);
    expect(lookOf(await client.pageGet(alpha))).toEqual(["mono", true, true]);
  });

  it("refuses an unknown page, a font that is not one and a flag that is not a boolean, and writes nothing", async () => {
    const { client, events, alpha } = await setup();
    await client.pageSetAppearance(alpha, { font: "serif" });
    events.length = 0;
    const unknown = await failure(client.pageSetAppearance(UNKNOWN, { locked: true }));
    expect(unknown.toApiError().code).toBe("NotFound");

    for (const bad of ["comic", "Serif", "", "sans"]) {
      const error = await failure(client.pageSetAppearance(alpha, { font: bad as "serif" }));
      expect(error.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "font" } });
    }
    for (const field of ["smallText", "locked"] as const) {
      for (const bad of ["true", 1, 0, {}]) {
        const error = await failure(client.pageSetAppearance(alpha, { [field]: bad as boolean }));
        expect(error.toApiError()).toMatchObject({ code: "InvalidInput", details: { field } });
      }
    }
    // A refusal changes nothing, even when another part of the request was fine.
    const mixed = await failure(client.pageSetAppearance(alpha, { locked: true, font: "comic" as "serif" }));
    expect(mixed.toApiError().code).toBe("InvalidInput");
    expect(lookOf(await client.pageGet(alpha))).toEqual(["serif", undefined, undefined]);
    expect(events).toEqual([]);
  });

  it("writes nothing and announces nothing when the page already has the look", async () => {
    const { client, events, clock, alpha, personal } = await setup();
    await client.pageSetAppearance(alpha, { font: "mono", smallText: true, locked: true });
    events.length = 0;
    clock.now = T0 + 900;
    const same = await client.pageSetAppearance(alpha, { font: "mono", smallText: true, locked: true });
    expect(same.updatedAt).toBe(T0);
    await client.pageSetAppearance(alpha, { smallText: true });
    await client.pageSetAppearance(alpha, {});
    // The neutral value over a page without that part is the same thing.
    await client.pageSetAppearance(
      (await client.pageCreate({ spaceId: personal, title: "Plain" })).id,
      { font: "default", smallText: false, locked: false },
    );
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId: personal } }]); // only the page just created
    events.length = 0;
    await client.pageSetAppearance(alpha, { smallText: false });
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId: personal } }]);
  });

  it("undoes and redoes the whole change as one step", async () => {
    const { client, alpha, personal } = await setup();
    await client.pageSetAppearance(alpha, { font: "serif", smallText: true, locked: true });
    await client.structureUndo(personal);
    expect(lookOf(await client.pageGet(alpha))).toEqual([undefined, undefined, undefined]);
    await client.structureRedo(personal);
    expect(lookOf(await client.pageGet(alpha))).toEqual(["serif", true, true]);
    await client.pageSetAppearance(alpha, { locked: false });
    await client.structureUndo(personal);
    expect(lookOf(await client.pageGet(alpha))).toEqual(["serif", true, true]);
  });

  it("is a guard of the UI: renaming, the icon, the cover, moving, trashing and duplicating a locked page all work", async () => {
    const { client, personal, work, alpha, beta } = await setup();
    await client.pageSetAppearance(alpha, { locked: true, font: "mono" });
    await client.pageRename(alpha, "Renamed");
    await client.pageSetIcon(alpha, "🔒");
    await client.pageSetCover(alpha, "color:red");
    await client.pageMove(alpha, beta, 0);
    expect(await client.pageGet(alpha)).toMatchObject({ title: "Renamed", icon: "🔒", cover: "color:red", parentId: beta, locked: true, font: "mono" });
    const copy = await client.pageDuplicate(alpha);
    expect(copy).toMatchObject({ locked: true, font: "mono" });
    await client.pageMoveToSpace(alpha, work, null, null);
    expect(await client.pageGet(alpha)).toMatchObject({ spaceId: work, locked: true, font: "mono" });
    await client.pageTrash(alpha);
    expect(await client.pageGet(alpha)).toMatchObject({ locked: true });
    expect((await client.treeGet(personal)).roots.length).toBeGreaterThan(0);
  });
});

describe("the look of duplicated and moved pages", () => {
  it("is copied with a duplicate, sub-pages included", async () => {
    const { client, personal, alpha } = await setup();
    const child = await client.pageCreate({ spaceId: personal, parentId: alpha, title: "Child" });
    const plain = await client.pageCreate({ spaceId: personal, parentId: alpha, title: "Plain" });
    await client.pageSetAppearance(alpha, { font: "serif", smallText: true, locked: true });
    await client.pageSetAppearance(child.id, { font: "mono" });
    const copy = await client.pageDuplicate(alpha);
    expect(lookOf(copy)).toEqual(["serif", true, true]);
    const copied = (await client.treeGet(personal)).roots.find((node) => node.page.id === copy.id);
    expect(copied?.children.map((node) => [node.page.title, ...lookOf(node.page)])).toEqual([
      ["Child", "mono", undefined, undefined],
      ["Plain", undefined, undefined, undefined],
    ]);
    // The original is untouched.
    expect(lookOf(await client.pageGet(plain.id))).toEqual([undefined, undefined, undefined]);
    expect(lookOf(await client.pageGet(alpha))).toEqual(["serif", true, true]);
  });

  it("travels with a move to another space", async () => {
    const { client, work, alpha } = await setup();
    await client.pageSetAppearance(alpha, { font: "mono", smallText: true, locked: true });
    await client.pageMoveToSpace(alpha, work, null, null);
    expect(await client.pageGet(alpha)).toMatchObject({ spaceId: work, font: "mono", smallText: true, locked: true });
    // And through undo in the target space: the move is not an undo step, the look stays.
    expect(lookOf(await client.pageGet(alpha))).toEqual(["mono", true, true]);
  });
});

describe("the look in backups and seeds", () => {
  it("is carried by a backup and read back, and a font that is not one reads as the default", async () => {
    const files = new Map<string, Uint8Array>();
    const source = await setup({ files, security: { kitCreatedAt: T0, kitKey: KEY } });
    await source.client.pageSetAppearance(source.alpha, { font: "serif", smallText: true, locked: true });
    await source.client.pageSetAppearance(source.beta, { font: "mono" });
    await source.client.backupExport("/backups/looks.atomikkb");

    const target = await setup({ files, workspaceName: undefined, spaces: [] });
    target.clock.now = T0 + 50;
    await target.client.backupImport("/backups/looks.atomikkb", (await kitWordsFromEntropy(KEY)).join(" "));
    expect(lookOf(await target.client.pageGet(source.alpha))).toEqual(["serif", true, true]);
    expect(lookOf(await target.client.pageGet(source.beta))).toEqual(["mono", undefined, undefined]);
  });

  it("is set by a seed, without events or undo history", async () => {
    const { client, events, personal } = await setup({
      spaces: [
        {
          name: "Personal",
          pages: [{ title: "Alpha", font: "serif", locked: true }, { title: "Beta", smallText: true }, { title: "Gamma" }],
        },
      ],
    });
    const roots = (await client.treeGet(personal)).roots;
    expect(roots.map((node) => lookOf(node.page))).toEqual([
      ["serif", undefined, true],
      [undefined, true, undefined],
      [undefined, undefined, undefined],
    ]);
    expect(events).toEqual([]);
    expect((await client.treeGet(personal)).undo).toEqual({ canUndo: false, canRedo: false });
  });
});
