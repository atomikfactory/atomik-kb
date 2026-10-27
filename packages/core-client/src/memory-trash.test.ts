// Trash expiry in the in-memory core (ADR-0040 item 3), the mirror of `kb-core`'s `tests_trash`: the
// setting `app.trash.retention_days`, which entries expire and when, that an expiry is a purge, the
// hourly check of `remindersTick`, the unlock, and the arithmetic the UI shares (`./trash`).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import {
  DEFAULT_TRASH_RETENTION_DAYS,
  TRASH_EXPIRY_INTERVAL_MS,
  TRASH_RETENTION_SETTING,
  isTrashExpired,
  trashDaysLeft,
  trashRetentionDays,
  trashRetentionMs,
} from "./trash";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const T0 = Date.UTC(2026, 9, 3, 12, 0, 0);

beforeEach(() => {
  // The simulated sync driver runs on timers; none of these tests advances them.
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

/** A core whose clock the test moves. */
async function setup(options: MemoryTransportOptions = {}) {
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    now: () => clock.now,
    workspaceName: "Home",
    spaces: [{ name: "Personal" }],
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const [space] = await client.spaceList();
  const spaceId = space?.id ?? "";
  const page = async (title: string, parentId?: string): Promise<string> =>
    (await client.pageCreate({ spaceId, title, ...(parentId === undefined ? {} : { parentId }) })).id;
  const trashed = async (): Promise<string[]> => (await client.treeGet(spaceId)).trashed.map((entry) => entry.page.id);
  const exists = async (pageId: string): Promise<boolean> =>
    client.pageGet(pageId).then(
      () => true,
      () => false,
    );
  return { transport, client, clock, events, spaceId, page, trashed, exists };
}

describe("expiring the trash of the in-memory core", () => {
  it("keeps a page for thirty days and takes it a millisecond after", async () => {
    expect(DEFAULT_TRASH_RETENTION_DAYS).toBe(30);
    const { transport, client, clock, page, trashed, exists } = await setup();
    const old = await page("Old");
    await client.pageTrash(old);
    expect(await transport.trashExpire()).toBe(0);

    clock.now = T0 + 30 * DAY;
    expect(await transport.trashExpire()).toBe(0);
    expect(await trashed()).toEqual([old]);

    clock.now = T0 + 30 * DAY + 1;
    expect(await transport.trashExpire()).toBe(1);
    expect(await trashed()).toEqual([]);
    expect(await exists(old)).toBe(false);
    await expect(client.pageGet(old)).rejects.toBeInstanceOf(CoreError);
    expect(await transport.trashExpire()).toBe(0);
  });

  it("never touches live pages", async () => {
    const { transport, clock, page, exists } = await setup();
    const live = await page("Live");
    clock.now = T0 + 400 * DAY;
    expect(await transport.trashExpire()).toBe(0);
    expect(await exists(live)).toBe(true);
  });

  it.each<[string, JsonValue | undefined, number | null]>([
    ["7 days", 7, 7 * DAY],
    ["a day", 1, DAY],
    ["half a day", 0.5, DAY / 2],
    ["a string is no number: the default", "seven", 30 * DAY],
    ["null is no number: the default", null, 30 * DAY],
    ["an object is no number: the default", { days: 1 }, 30 * DAY],
    ["zero keeps everything", 0, null],
    ["a negative number keeps everything", -5, null],
  ])("reads the setting like the core: %s", async (_name, value, limit) => {
    const { transport, client, clock, page, trashed } = await setup();
    const id = await page("Page");
    await client.settingsSet(TRASH_RETENTION_SETTING, value as JsonValue);
    await client.pageTrash(id);
    if (limit === null) {
      clock.now = T0 + 3650 * DAY;
      expect(await transport.trashExpire()).toBe(0);
      expect(await trashed()).toEqual([id]);
      return;
    }
    clock.now = T0 + limit;
    expect(await transport.trashExpire()).toBe(0);
    clock.now = T0 + limit + 1;
    expect(await transport.trashExpire()).toBe(1);
  });

  it("counts entries, not sub-pages, and takes a subtree whole", async () => {
    const { transport, client, clock, page, trashed, exists } = await setup();
    const other = await page("Other");
    const parent = await page("Parent");
    const kid = await page("Kid", parent);
    const grand = await page("Grand", kid);
    const early = await page("Early", parent);
    await client.pageTrash(early);
    clock.now = T0 + 5 * DAY;
    await client.pageTrash(other);
    clock.now = T0 + 20 * DAY;
    await client.pageTrash(parent);
    // The descendants carry the parent's time.
    expect((await client.pageGet(grand)).trashedAt).toBe(T0 + 20 * DAY);

    clock.now = T0 + 31 * DAY;
    expect(await transport.trashExpire()).toBe(1);
    expect(await exists(early)).toBe(false);
    clock.now = T0 + 35 * DAY + 1;
    expect(await transport.trashExpire()).toBe(1);
    expect(await exists(other)).toBe(false);
    expect(await trashed()).toEqual([parent]);
    expect(await exists(kid)).toBe(true);

    clock.now = T0 + 50 * DAY + 1;
    expect(await transport.trashExpire()).toBe(1);
    for (const gone of [parent, kid, grand]) {
      expect(await exists(gone)).toBe(false);
    }
  });

  it("starts the thirty days again when a restored page is trashed again", async () => {
    const { transport, client, clock, page, exists } = await setup();
    const id = await page("Back and forth");
    await client.pageTrash(id);
    clock.now = T0 + 20 * DAY;
    await client.pageRestore(id);
    clock.now = T0 + 25 * DAY;
    await client.pageTrash(id);

    clock.now = T0 + 31 * DAY;
    expect(await transport.trashExpire()).toBe(0);
    clock.now = T0 + 55 * DAY;
    expect(await transport.trashExpire()).toBe(0);
    clock.now = T0 + 55 * DAY + 1;
    expect(await transport.trashExpire()).toBe(1);
    expect(await exists(id)).toBe(false);
  });

  it("sweeps every space", async () => {
    const { transport, client, clock, page, spaceId } = await setup();
    const work = await client.spaceCreate("Work");
    const a = await page("A");
    const b = (await client.pageCreate({ spaceId: work.id, title: "B" })).id;
    await client.pageTrash(a);
    await client.pageTrash(b);
    clock.now = T0 + 31 * DAY;
    expect(await transport.trashExpire()).toBe(2);
    expect((await client.treeGet(spaceId)).trashed).toEqual([]);
    expect((await client.treeGet(work.id)).trashed).toEqual([]);
  });

  it("purges like PagePurge: the content goes, and each event comes once", async () => {
    const { transport, client, clock, events, spaceId, page } = await setup();
    const ids = await Promise.all(["One", "Two", "Three"].map((title) => page(title)));
    for (const id of ids) {
      await client.pageTrash(id);
    }
    // A trashed page still has its content.
    await expect(client.pageContentGet(ids[0] ?? "")).resolves.toBeDefined();
    events.length = 0;

    clock.now = T0 + 31 * DAY;
    expect(await transport.trashExpire()).toBe(3);
    expect(events.filter((event) => event.type === "StructureChanged")).toEqual([
      { type: "StructureChanged", data: { spaceId } },
    ]);
    expect(events.some((event) => event.type === "SpacesChanged")).toBe(false);
    await expect(client.pageContentGet(ids[0] ?? "")).rejects.toMatchObject({ code: "NotFound" });
  });

  it("counts as a local edit for sync, once for the whole run", async () => {
    const { transport, client, clock, page } = await setup();
    const ids = [await page("One"), await page("Two")];
    for (const id of ids) {
      await client.pageTrash(id);
    }
    await client.syncConfigure("https://sync.example.com");
    expect((await client.syncStatus()).pendingEntries).toBe(0);
    clock.now = T0 + 31 * DAY;
    expect(await transport.trashExpire()).toBe(2);
    expect((await client.syncStatus()).pendingEntries).toBe(1);
  });
});

describe("the hourly check of the tick", () => {
  it("starts the hour at the first tick, then expires once an hour and not at every tick", async () => {
    const { transport, client, clock, page, exists } = await setup();
    await client.settingsSet(TRASH_RETENTION_SETTING, 1);
    const a = await page("A");
    await client.pageTrash(a);
    clock.now = T0 + 2 * HOUR;
    const b = await page("B");
    await client.pageTrash(b);

    clock.now = T0 + 3 * HOUR;
    await transport.remindersTick();
    // 25.5 hours: A (trashed 25.5 hours ago) is past a day, B (23.5 hours ago) is not; the last check was long ago.
    clock.now = T0 + 25 * HOUR + 30 * MINUTE;
    await transport.remindersTick();
    expect(await exists(a)).toBe(false);
    expect(await exists(b)).toBe(true);

    // B is overdue at 26 hours, but the hour since the last check is not over.
    clock.now = T0 + 26 * HOUR + 10 * MINUTE;
    await transport.remindersTick();
    expect(await exists(b)).toBe(true);
    clock.now = T0 + 26 * HOUR + 30 * MINUTE;
    await transport.remindersTick();
    expect(await exists(b)).toBe(false);
    expect(TRASH_EXPIRY_INTERVAL_MS).toBe(HOUR);
  });

  it("does nothing while the setting keeps everything", async () => {
    const { transport, client, clock, page, exists } = await setup();
    await client.settingsSet(TRASH_RETENTION_SETTING, 0);
    const id = await page("Kept");
    await client.pageTrash(id);
    await transport.remindersTick();
    clock.now = T0 + 90 * DAY;
    await transport.remindersTick();
    expect(await exists(id)).toBe(true);
  });
});

describe("a locked core", () => {
  const PASSPHRASE = "correct horse";

  it("expires nothing while locked, and the unlock catches up", async () => {
    const { transport, client, clock, events, spaceId, page, exists } = await setup({
      security: { appLockPassphrase: PASSPHRASE },
    });
    const id = await page("Secret");
    await client.pageTrash(id);
    await transport.lock();

    clock.now = T0 + 90 * DAY;
    expect(await transport.trashExpire()).toBe(0);
    await transport.remindersTick();
    events.length = 0;

    await transport.unlock(PASSPHRASE);
    expect(await exists(id)).toBe(false);
    expect((await client.treeGet(spaceId)).trashed).toEqual([]);
    expect(events).toContainEqual({ type: "StructureChanged", data: { spaceId } });
  });
});

describe("the arithmetic of the setting", () => {
  it("reads a stored value the way the core does", () => {
    expect(trashRetentionMs(undefined)).toBe(30 * DAY);
    expect(trashRetentionMs(null)).toBe(30 * DAY);
    expect(trashRetentionMs("7")).toBe(30 * DAY);
    expect(trashRetentionMs(true)).toBe(30 * DAY);
    expect(trashRetentionMs(7)).toBe(7 * DAY);
    expect(trashRetentionMs(0.5)).toBe(DAY / 2);
    expect(trashRetentionMs(0)).toBeNull();
    expect(trashRetentionMs(-1)).toBeNull();
    expect(trashRetentionMs(Number.POSITIVE_INFINITY)).toBeNull();
  });

  it("shows the days the core would use", () => {
    expect([undefined, null, "7", true, [1]].map((value) => trashRetentionDays(value))).toEqual([30, 30, 30, 30, 30]);
    expect([7, 0, 0.5, -3].map((value) => trashRetentionDays(value))).toEqual([7, 0, 0.5, 0]);
  });

  it("is expired strictly after the limit", () => {
    expect(isTrashExpired(100, 50, 150)).toBe(false);
    expect(isTrashExpired(100, 50, 151)).toBe(true);
    expect(isTrashExpired(undefined, 50, 1e12)).toBe(false);
    // A clock that went back expires nothing.
    expect(isTrashExpired(100, 50, 0)).toBe(false);
  });

  it("counts the calendar days left, in the local time zone", () => {
    const local = (month: number, day: number, hour = 15): number => new Date(2026, month - 1, day, hour).getTime();
    const trashedAt = local(10, 3);
    const limit = 30 * DAY;
    expect(trashDaysLeft(trashedAt, limit, trashedAt)).toBe(30);
    expect(trashDaysLeft(trashedAt, limit, local(10, 3, 23))).toBe(30);
    expect(trashDaysLeft(trashedAt, limit, local(10, 4, 1))).toBe(29);
    expect(trashDaysLeft(trashedAt, limit, local(11, 1))).toBe(1);
    expect(trashDaysLeft(trashedAt, limit, local(11, 2))).toBe(0);
    expect(trashDaysLeft(trashedAt, limit, local(11, 9))).toBe(0);
    // A page trashed late in the evening with a one-day limit goes tomorrow evening: tomorrow, then today.
    expect(trashDaysLeft(local(10, 3, 23), DAY, local(10, 3, 23))).toBe(1);
    expect(trashDaysLeft(local(10, 3, 23), DAY, local(10, 4, 0))).toBe(0);
    expect(trashDaysLeft(BigInt(trashedAt), limit, trashedAt)).toBe(30);
    expect(trashDaysLeft(trashedAt, null, trashedAt)).toBeNull();
    expect(trashDaysLeft(Number.NaN, limit, trashedAt)).toBeNull();
  });
});
