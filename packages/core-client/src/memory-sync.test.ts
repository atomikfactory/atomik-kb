// The sync side of `MemoryTransport` (Milestone 6): the status, `SyncConfigure`, `SyncNow`,
// the fake driver's cycles on timers, the simulated servers of `MemoryTransportOptions.sync`,
// and the app lock.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { SyncInfo } from "./generated/SyncInfo";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { MEMORY_SERVER_EPOCH } from "./sync-mirror";

const START = 1_700_000_000_000;
const STEP = 150;
const URL_A = "https://sync.example.com";
const URL_B = "http://localhost:8787";
const PASSPHRASE = "correct horse";

const CHANGED: Event = { type: "SyncChanged" };
const REQUESTED: Event = { type: "SyncRequested" };
const SPACES: Event = { type: "SpacesChanged" };
const UNKNOWN_ACCOUNT = "the server does not know this account; an invite token is required";

const DISABLED: SyncInfo = {
  configured: false,
  state: "Disabled",
  pendingEntries: 0,
  parkedItems: 0,
  transfers: { uploads: 0, downloads: 0, waiting: 0 },
  remoteCursor: 0,
  forkDetected: false,
  devicesSeen: 0,
  needsInviteToken: false,
};

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(START);
});

afterEach(() => {
  vi.useRealTimers();
});

/** Deterministic ids; the clock is the fake timers' `Date.now`. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
    now: () => Date.now(),
    workspaceName: "Home",
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  /** The events so far, forgotten afterwards. */
  const take = (): Event[] => events.splice(0);
  return { transport, client, events, take };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** One local edit: a new space (one `SpacesChanged`). */
async function edit(client: CoreClient, name = "Notes"): Promise<void> {
  await client.spaceCreate(name);
}

describe("SyncStatus and SyncConfigure", () => {
  it("starts Disabled with nothing pending, and local edits neither count nor ask for a cycle", async () => {
    const { client, take } = setup();
    await expect(client.syncStatus()).resolves.toEqual(DISABLED);
    await edit(client);
    expect(take()).toEqual([SPACES]);
    await expect(client.syncStatus()).resolves.toEqual(DISABLED);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("connecting answers Idle and emits SyncChanged then SyncRequested", async () => {
    const { client, take } = setup();
    const info = await client.syncConfigure(URL_A);
    expect(info).toEqual({ ...DISABLED, configured: true, serverUrl: URL_A, state: "Idle" });
    expect(take()).toEqual([CHANGED, REQUESTED]);
    await expect(client.syncStatus()).resolves.toEqual(info);
  });

  it("a cycle goes Pushing, Pulling, Idle, one step each, the cursor growing by the edits pushed", async () => {
    const { client, take } = setup();
    await client.syncConfigure(URL_A);
    await edit(client, "dark");
    await edit(client, "light");
    expect(take()).toEqual([CHANGED, REQUESTED, SPACES, REQUESTED, SPACES, REQUESTED]);
    expect((await client.syncStatus()).pendingEntries).toBe(2);

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([CHANGED]);
    await expect(client.syncStatus()).resolves.toEqual({
      ...DISABLED,
      configured: true,
      serverUrl: URL_A,
      state: "Pushing",
      pendingEntries: 2,
      parkedItems: 0,
      transfers: { uploads: 0, downloads: 0, waiting: 0 },
      serverEpoch: MEMORY_SERVER_EPOCH,
    });

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([CHANGED]);
    await expect(client.syncStatus()).resolves.toMatchObject({
      state: "Pulling",
      pendingEntries: 0,
      parkedItems: 0,
      transfers: { uploads: 0, downloads: 0, waiting: 0 },
      remoteCursor: 0,
      lastPushAt: START + 2 * STEP,
    });

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([CHANGED]);
    await expect(client.syncStatus()).resolves.toEqual({
      configured: true,
      serverUrl: URL_A,
      state: "Idle",
      pendingEntries: 0,
      parkedItems: 0,
      transfers: { uploads: 0, downloads: 0, waiting: 0 },
      remoteCursor: 2,
      serverEpoch: MEMORY_SERVER_EPOCH,
      lastPushAt: START + 2 * STEP,
      lastPullAt: START + 3 * STEP,
      forkDetected: false,
      devicesSeen: 0,
      needsInviteToken: false,
    });
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("counts page, structure, space and attachment changes, never settings or remote ones", async () => {
    const { client, transport, take } = setup();
    const space = await client.spaceCreate("Work");
    const page = await client.pageCreate({ spaceId: space.id, title: "Plan" });
    await client.syncConfigure(URL_A);
    take();

    await client.spaceRename(space.id, "Job"); // SpacesChanged + StructureChanged
    await client.pageRename(page.id, "Plan B"); // StructureChanged
    transport.changePage(page.id, () => undefined, "Editor"); // PageContentChanged (Editor)
    await edit(client); // SpacesChanged
    const attachmentId = await client.attachmentBegin(space.id, "a.txt", "text/plain");
    await client.attachmentAbort(attachmentId); // AttachmentsChanged
    expect((await client.syncStatus()).pendingEntries).toBe(6);
    expect(take().filter((event) => event.type === "SyncRequested")).toHaveLength(6);

    // Settings are local to the device; remote changes are not this device's to upload.
    await client.settingsSet("ui.theme", "dark");
    transport.changePage(page.id, () => undefined);
    expect(take()).toEqual([
      { type: "SettingChanged", data: { key: "ui.theme" } },
      { type: "PageContentChanged", data: { pageId: page.id, source: "Remote" } },
    ]);
    expect((await client.syncStatus()).pendingEntries).toBe(6);
  });

  it("edits during a cycle stay pending and run one more cycle after it", async () => {
    const { client, take } = setup();
    await client.syncConfigure(URL_A);
    await edit(client);
    vi.advanceTimersByTime(STEP); // Pushing 1
    await edit(client, "light");
    expect(await client.syncStatus()).toMatchObject({ state: "Pushing", pendingEntries: 2 });
    vi.advanceTimersByTime(STEP); // Pulling: the first one is pushed, the second still pending
    expect(await client.syncStatus()).toMatchObject({ state: "Pulling", pendingEntries: 1 });
    vi.advanceTimersByTime(STEP); // Idle
    expect(await client.syncStatus()).toMatchObject({ state: "Idle", pendingEntries: 1, remoteCursor: 1 });
    take();
    vi.advanceTimersByTime(3 * STEP); // the second cycle
    expect(take()).toEqual([CHANGED, CHANGED, CHANGED]);
    expect(await client.syncStatus()).toMatchObject({ state: "Idle", pendingEntries: 0, remoteCursor: 2 });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("requests while the driver waits for its first step merge into one cycle", async () => {
    const { client, take } = setup();
    await client.syncConfigure(URL_A);
    await client.syncNow();
    await client.syncNow();
    expect(take()).toEqual([CHANGED, REQUESTED, REQUESTED, REQUESTED]);
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([CHANGED, CHANGED, CHANGED]);
  });

  it("SyncNow emits SyncRequested; while disabled no cycle follows", async () => {
    const { client, take } = setup();
    await expect(client.syncNow()).resolves.toBeUndefined();
    expect(take()).toEqual([REQUESTED]);
    expect(vi.getTimerCount()).toBe(0);
    await expect(client.syncStatus()).resolves.toEqual(DISABLED);
  });

  it("disabling forgets the server, the cursor and the epoch, stops a cycle and emits SyncChanged", async () => {
    const { client, take } = setup();
    await client.syncConfigure(URL_A);
    await edit(client);
    vi.advanceTimersByTime(3 * STEP);
    await edit(client);
    vi.advanceTimersByTime(STEP); // Pushing
    take();
    await expect(client.syncConfigure(null)).resolves.toEqual(DISABLED);
    expect(take()).toEqual([CHANGED]);
    expect(vi.getTimerCount()).toBe(0);
    await expect(client.syncStatus()).resolves.toEqual(DISABLED);
  });

  it("a different server starts from cursor 0; the same one keeps what was learnt", async () => {
    const { client } = setup();
    await client.syncConfigure(URL_A);
    await edit(client);
    vi.advanceTimersByTime(3 * STEP);
    const same = await client.syncConfigure(URL_A);
    expect(same).toMatchObject({ state: "Idle", remoteCursor: 1, serverEpoch: MEMORY_SERVER_EPOCH });
    vi.advanceTimersByTime(3 * STEP);
    const other = await client.syncConfigure(URL_B);
    expect(other).toEqual({ ...DISABLED, configured: true, serverUrl: URL_B, state: "Idle" });
  });

  it("checks the address and the token like the client, changing nothing on a refusal", async () => {
    const { client, transport, take } = setup();
    const raw = (serverUrl: string, inviteToken?: string) =>
      transport.dispatch({
        type: "SyncConfigure",
        data: inviteToken === undefined ? { serverUrl } : { serverUrl, inviteToken },
      });
    expect((await failure(raw("ftp://sync.example.com"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the server address must be http(s)://host[:port]",
      details: { field: "serverUrl" },
    });
    expect((await failure(raw(URL_A, "x".repeat(129)))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the invite token must be 1 to 128 printable ASCII characters",
      details: { field: "inviteToken" },
    });
    expect(take()).toEqual([]);
    await expect(client.syncStatus()).resolves.toEqual(DISABLED);
  });
});

describe("simulated servers (MemoryTransportOptions.sync)", () => {
  it("state idle or a serverUrl starts connected, as a device that synced before, without a cycle", async () => {
    const seeded = { configured: true, state: "Idle", serverEpoch: MEMORY_SERVER_EPOCH, lastPushAt: START, lastPullAt: START };
    const idle = setup({ sync: { state: "idle" } });
    await expect(idle.client.syncStatus()).resolves.toEqual({ ...DISABLED, ...seeded, serverUrl: "https://sync.example.com" });
    const url = setup({ sync: { serverUrl: URL_B } });
    await expect(url.client.syncStatus()).resolves.toEqual({ ...DISABLED, ...seeded, serverUrl: URL_B });
    expect(vi.getTimerCount()).toBe(0);
    expect(idle.events).toEqual([]);
  });

  it("offline stays Offline with \"cannot reach the server\" on every cycle, edits pending", async () => {
    const { client, take } = setup({ sync: { state: "offline" } });
    await expect(client.syncStatus()).resolves.toMatchObject({
      configured: true,
      state: "Offline",
      lastError: "cannot reach the server",
    });
    await edit(client);
    take();
    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([CHANGED]);
    await expect(client.syncStatus()).resolves.toMatchObject({
      state: "Offline",
      lastError: "cannot reach the server",
      pendingEntries: 1,
      parkedItems: 0,
      transfers: { uploads: 0, downloads: 0, waiting: 0 },
      remoteCursor: 0,
    });
    // Reconnecting answers Idle, then the next cycle meets the same unreachable server.
    await expect(client.syncConfigure(URL_A)).resolves.not.toHaveProperty("lastError");
    vi.advanceTimersByTime(STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Offline", pendingEntries: 1 });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("forked starts Forked with forkDetected; a cycle only pulls, uploads stay paused", async () => {
    const { client, take } = setup({ sync: { state: "forked" } });
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Forked", forkDetected: true });
    await edit(client);
    take();
    vi.advanceTimersByTime(STEP);
    expect(await client.syncStatus()).toMatchObject({ state: "Pulling", pendingEntries: 1 });
    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([CHANGED, CHANGED]);
    await expect(client.syncStatus()).resolves.toMatchObject({
      state: "Forked",
      forkDetected: true,
      pendingEntries: 1,
      parkedItems: 0,
      transfers: { uploads: 0, downloads: 0, waiting: 0 },
      lastPullAt: START + 2 * STEP,
      lastPushAt: START,
    });
    // Disconnecting clears the fork.
    await expect(client.syncConfigure(null)).resolves.toEqual(DISABLED);
  });

  it("error shows Error with the given message, or a default one", async () => {
    const given = setup({ sync: { state: "error", lastError: "quota exceeded" } });
    await expect(given.client.syncStatus()).resolves.toMatchObject({ state: "Error", lastError: "quota exceeded" });
    await given.client.syncNow();
    vi.advanceTimersByTime(STEP);
    await expect(given.client.syncStatus()).resolves.toMatchObject({ state: "Error", lastError: "quota exceeded" });
    const plain = setup({ sync: { state: "error" } });
    await expect(plain.client.syncStatus()).resolves.toMatchObject({
      state: "Error",
      lastError: "the server refused the request",
    });
  });

  it("the simulated server stays the same across a disconnect and a reconnect", async () => {
    const { client } = setup({ sync: { state: "offline" }, syncStepMs: 10 });
    await client.syncConfigure(null);
    await expect(client.syncStatus()).resolves.toEqual(DISABLED);
    await client.syncConfigure(URL_B);
    vi.advanceTimersByTime(10);
    await expect(client.syncStatus()).resolves.toMatchObject({ serverUrl: URL_B, state: "Offline" });
  });

  it("setSyncMode switches the server for the next cycle", async () => {
    const { client, transport } = setup();
    await client.syncConfigure(URL_A);
    vi.advanceTimersByTime(3 * STEP);
    transport.setSyncMode("forked");
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Idle", forkDetected: false });
    await client.syncNow();
    vi.advanceTimersByTime(2 * STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Forked", forkDetected: true });
    transport.setSyncMode("offline", "connection refused");
    await client.syncNow();
    vi.advanceTimersByTime(STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Offline", lastError: "connection refused" });
    // A healthy server again: the fork stands (the driver only pulls) until a disconnect.
    transport.setSyncMode("idle");
    await client.syncNow();
    vi.advanceTimersByTime(3 * STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Forked", forkDetected: true });
    await client.syncConfigure(null);
    await client.syncConfigure(URL_A);
    vi.advanceTimersByTime(3 * STEP);
    const back = await client.syncStatus();
    expect(back).toMatchObject({ state: "Idle", forkDetected: false });
    expect(back).not.toHaveProperty("lastError");
  });

  it("reconfiguring the same server keeps a detected fork; another server clears it", async () => {
    const { client } = setup({ sync: { state: "forked", serverUrl: URL_A } });
    await expect(client.syncConfigure(URL_A)).resolves.toMatchObject({ state: "Forked", forkDetected: true });
    vi.advanceTimersByTime(2 * STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Forked", forkDetected: true });
    await expect(client.syncConfigure(URL_B)).resolves.toMatchObject({ state: "Idle", forkDetected: false });
  });

  it("syncStepMs sets the length of a step", async () => {
    const { client } = setup({ syncStepMs: 1000 });
    await client.syncConfigure(URL_A);
    vi.advanceTimersByTime(999);
    expect((await client.syncStatus()).state).toBe("Idle");
    vi.advanceTimersByTime(1);
    expect((await client.syncStatus()).state).toBe("Pushing");
  });

  it("an unknown account ends the cycle in Error with needsInviteToken, without an immediate refusal", async () => {
    const { client, take } = setup({ sync: { accountKnown: false } });
    await expect(client.syncConfigure(URL_A)).resolves.toEqual({
      ...DISABLED,
      configured: true,
      serverUrl: URL_A,
      state: "Idle",
    });
    await edit(client);
    take();
    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([CHANGED]);
    await expect(client.syncStatus()).resolves.toEqual({
      ...DISABLED,
      configured: true,
      serverUrl: URL_A,
      state: "Error",
      pendingEntries: 1,
      parkedItems: 0,
      transfers: { uploads: 0, downloads: 0, waiting: 0 },
      serverEpoch: MEMORY_SERVER_EPOCH,
      lastError: UNKNOWN_ACCOUNT,
      needsInviteToken: true,
    });
    expect(vi.getTimerCount()).toBe(0);
    // Without a token the flag stays, and the next cycle fails the same way.
    await expect(client.syncConfigure(URL_A)).resolves.toMatchObject({ state: "Idle", needsInviteToken: true });
    vi.advanceTimersByTime(STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Error", lastError: UNKNOWN_ACCOUNT });
  });

  it("a refused token (\"bad\") fails the cycle again; another token creates the account", async () => {
    const { client, take } = setup({ sync: { accountKnown: false } });
    await client.syncConfigure(URL_A);
    vi.advanceTimersByTime(STEP);
    // A token clears the flag at once; the server's answer comes with the cycle.
    await expect(client.syncConfigure(URL_A, "bad")).resolves.toMatchObject({ state: "Idle", needsInviteToken: false });
    vi.advanceTimersByTime(STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({
      state: "Error",
      lastError: "the server refused the invite token",
      needsInviteToken: true,
    });
    await client.syncConfigure(URL_A, "invite-1");
    take();
    vi.advanceTimersByTime(3 * STEP);
    expect(take()).toEqual([CHANGED, CHANGED, CHANGED]);
    const synced = await client.syncStatus();
    expect(synced).toMatchObject({ state: "Idle", needsInviteToken: false, lastPullAt: START + 5 * STEP });
    expect(synced).not.toHaveProperty("lastError");
    // The account exists now: a reconnect needs no token.
    await client.syncConfigure(null);
    await client.syncConfigure(URL_A);
    vi.advanceTimersByTime(3 * STEP);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Idle", needsInviteToken: false });
  });

  it("the token waits for a cycle that reaches the server", async () => {
    const { client, transport } = setup({ sync: { state: "offline", accountKnown: false }, syncStepMs: 10 });
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Offline", needsInviteToken: false });
    await client.syncConfigure(URL_A, "invite-1");
    vi.advanceTimersByTime(10);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Offline" });
    transport.setSyncMode("idle");
    await client.syncNow();
    vi.advanceTimersByTime(30);
    await expect(client.syncStatus()).resolves.toMatchObject({ state: "Idle", needsInviteToken: false });
  });

  it("starting connected to a server that does not know the account shows the failed cycle", async () => {
    const { client } = setup({ sync: { state: "idle", accountKnown: false } });
    await expect(client.syncStatus()).resolves.toEqual({
      ...DISABLED,
      configured: true,
      serverUrl: "https://sync.example.com",
      state: "Error",
      serverEpoch: MEMORY_SERVER_EPOCH,
      lastError: UNKNOWN_ACCOUNT,
      needsInviteToken: true,
    });
  });

  it("refuses a seeded server address that SyncConfigure would refuse", () => {
    expect(() => new MemoryTransport({ sync: { serverUrl: "sync.example.com" } })).toThrow(
      "the server address must be http(s)://host[:port]",
    );
  });
});

describe("sync and the app lock", () => {
  it("while locked the sync requests reject with Locked", async () => {
    const { client, transport } = setup({ security: { appLockPassphrase: PASSPHRASE } });
    await transport.lock();
    for (const call of [() => client.syncStatus(), () => client.syncNow(), () => client.syncConfigure(URL_A)]) {
      expect((await failure(call())).toApiError()).toEqual({ code: "Locked", message: "the workspace is locked" });
    }
  });

  it("a lock stops the driver silently, what it pushed pending again; an unlock runs a cycle", async () => {
    const { client, transport, take } = setup({ security: { appLockPassphrase: PASSPHRASE } });
    await client.syncConfigure(URL_A);
    await edit(client);
    vi.advanceTimersByTime(STEP); // Pushing
    take();
    await transport.lock();
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
    expect(vi.getTimerCount()).toBe(0);

    await transport.unlock(PASSPHRASE);
    expect(await client.syncStatus()).toMatchObject({ state: "Idle", pendingEntries: 1, remoteCursor: 0 });
    vi.advanceTimersByTime(3 * STEP);
    expect(take()).toEqual([CHANGED, CHANGED, CHANGED]);
    expect(await client.syncStatus()).toMatchObject({ state: "Idle", pendingEntries: 0, remoteCursor: 1 });
  });

  it("a core that stops being ready mid-cycle stops the driver too", async () => {
    const { client, transport, take } = setup();
    await client.syncConfigure(URL_A);
    vi.advanceTimersByTime(STEP); // Pushing
    take();
    transport.setCoreState("opening");
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
    transport.setCoreState("ready");
    expect((await client.syncStatus()).state).toBe("Idle");
  });
});
