import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import type { AppState } from "./store";
import { isSyncBusy } from "./store-sync";

const NOW = Date.UTC(2026, 9, 3, 12, 0, 0);
const PASSPHRASE = "correct horse battery";
const SERVER = "https://sync.example.com";

/** The memory core behind a recording transport, and a started store over it. */
function setup(options: MemoryTransportOptions = {}, overrides: Partial<Transport> = {}) {
  const memory = new MemoryTransport({ now: () => NOW, syncStepMs: 5, ...options });
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
    backupExport: (path) => memory.backupExport(path),
    backupImport: (path, kit) => memory.backupImport(path, kit),
    ...overrides,
  };
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  const types = (): Request["type"][] => requests.map((request) => request.type);
  return { memory, store, stop, requests, types };
}

type Ctx = ReturnType<typeof setup>;

const settled = (ctx: Ctx, check: (state: AppState) => boolean) =>
  vi.waitFor(
    () => {
      expect(check(ctx.store.getState())).toBe(true);
    },
    { timeout: 4000 },
  );

describe("isSyncBusy", () => {
  it("is true while data moves", () => {
    expect(isSyncBusy("Pushing")).toBe(true);
    expect(isSyncBusy("Pulling")).toBe(true);
    for (const state of ["Disabled", "Idle", "Offline", "Error", "Forked"] as const) {
      expect(isSyncBusy(state), state).toBe(false);
    }
  });
});

describe("sync: boot", () => {
  it("reads the sync status before showing the app when there is a workspace", async () => {
    const ctx = setup({ workspaceName: "Research", sync: { state: "offline", lastError: "connection refused" } });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().sync).toMatchObject({
      configured: true,
      serverUrl: SERVER,
      state: "Offline",
      lastError: "connection refused",
      forkDetected: false,
    });
    expect(ctx.types()).toContain("SyncStatus");
  });

  it("reads it as disabled for a workspace that never synced", async () => {
    const ctx = setup({ workspaceName: "Research" });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().sync).toMatchObject({ configured: false, state: "Disabled" });
  });

  it("does not read it without a workspace, and reads it once the onboarding created one", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "onboarding");
    expect(ctx.types()).not.toContain("SyncStatus");
    expect(ctx.store.getState().sync).toBeNull();
    await ctx.store.getState().createWorkspace("Research");
    await settled(ctx, (s) => s.sync !== null);
    expect(ctx.store.getState().sync?.state).toBe("Disabled");
  });

  it("opens the app without the sync status when the core cannot report it", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research" });
    const ctx = setup({}, {
      dispatch: (request) =>
        request.type === "SyncStatus"
          ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
          : memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
    });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState()).toMatchObject({ sync: null, workspace: { name: "Research" } });
    expect(logged).toHaveBeenCalledWith("could not read the sync status", "Storage");
    logged.mockRestore();
  });
});

describe("sync: events", () => {
  it("SyncChanged reads the status again: a connection made elsewhere and its cycle show", async () => {
    const ctx = setup({ workspaceName: "Research" });
    await settled(ctx, (s) => s.phase === "ready");
    ctx.requests.length = 0;
    // Another window connects; the store follows, reading nothing but the status.
    await ctx.memory.dispatch({ type: "SyncConfigure", data: { serverUrl: SERVER } });
    await settled(ctx, (s) => s.sync?.state === "Idle" && s.sync.lastPullAt === NOW);
    expect(new Set(ctx.types())).toEqual(new Set(["SyncStatus"]));
    expect(ctx.store.getState().sync).toMatchObject({ configured: true, serverUrl: SERVER, lastPushAt: NOW });
  });

  it("ignores SyncRequested, which is for the sync driver", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research" });
    const requests: Request[] = [];
    let deliver: Parameters<Transport["subscribe"]>[0] = () => undefined;
    const ctx = setup({}, {
      dispatch: (request) => {
        requests.push(request);
        return memory.dispatch(request);
      },
      subscribe: (onEvent) => {
        deliver = onEvent;
        return memory.subscribe(onEvent);
      },
      coreState: () => memory.coreState(),
      docs: memory.docs,
    });
    await settled(ctx, (s) => s.phase === "ready");
    requests.length = 0;
    deliver({ type: "SyncRequested" });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(requests).toEqual([]);
    deliver({ type: "SyncChanged" });
    await vi.waitFor(() => {
      expect(requests).toEqual([{ type: "SyncStatus" }]);
    });
  });

  it("does not read the status for a SyncChanged while locked", async () => {
    const ctx = setup({
      workspaceName: "Research",
      security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE, startLocked: true },
    });
    await settled(ctx, (s) => s.phase === "locked");
    await ctx.memory.unlock(PASSPHRASE);
    // The store still shows the lock screen (unlocked elsewhere, nothing booted here yet).
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "SyncConfigure", data: { serverUrl: SERVER } });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(ctx.types()).not.toContain("SyncStatus");
    expect(ctx.store.getState().sync).toBeNull();
  });

  it("follows the states of a cycle: Pushing, Pulling, then Idle", async () => {
    const ctx = setup({ workspaceName: "Research", syncStepMs: 40 });
    await settled(ctx, (s) => s.phase === "ready");
    const states: string[] = [];
    const unsubscribe = ctx.store.subscribe((state) => {
      const current = state.sync?.state;
      if (current !== undefined && states[states.length - 1] !== current) {
        states.push(current);
      }
    });
    await ctx.store.getState().configureSync(SERVER);
    await settled(ctx, (s) => s.sync?.lastPullAt !== undefined && s.sync.state === "Idle");
    unsubscribe();
    expect(states).toEqual(["Idle", "Pushing", "Pulling", "Idle"]);
  });
});

describe("sync: actions", () => {
  it("configureSync sends the address (and the token when given) and keeps the answer", async () => {
    const ctx = setup({ workspaceName: "Research" });
    await settled(ctx, (s) => s.phase === "ready");
    const info = await ctx.store.getState().configureSync(` ${SERVER} `);
    expect(info).toMatchObject({ configured: true, serverUrl: SERVER });
    expect(ctx.store.getState().sync?.configured).toBe(true);
    expect(ctx.requests).toContainEqual({ type: "SyncConfigure", data: { serverUrl: SERVER } });

    await ctx.store.getState().configureSync(SERVER, "invite-123");
    expect(ctx.requests).toContainEqual({ type: "SyncConfigure", data: { serverUrl: SERVER, inviteToken: "invite-123" } });
    // The token is never kept.
    expect(JSON.stringify(ctx.store.getState())).not.toContain("invite-123");
  });

  it("configureSync rejects what the core refuses, and the status stays", async () => {
    const ctx = setup({ workspaceName: "Research" });
    await settled(ctx, (s) => s.phase === "ready");
    await expect(ctx.store.getState().configureSync("ftp://example.com")).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "serverUrl" },
    });
    // A token that is not 1 to 128 printable ASCII characters.
    await expect(ctx.store.getState().configureSync(SERVER, "with\ttab")).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "inviteToken" },
    });
    expect(ctx.store.getState().sync).toMatchObject({ configured: false, state: "Disabled" });
  });

  it("follows the driver when the server does not know the account, until a token creates it", async () => {
    const ctx = setup({ workspaceName: "Research", sync: { accountKnown: false } });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().sync).toMatchObject({ configured: false, needsInviteToken: false });

    await ctx.store.getState().configureSync(SERVER);
    await settled(ctx, (s) => s.sync?.needsInviteToken === true);
    expect(ctx.store.getState().sync).toMatchObject({
      configured: true,
      state: "Error",
      lastError: "the server does not know this account; an invite token is required",
    });

    await ctx.store.getState().configureSync(SERVER, "bad");
    await settled(ctx, (s) => s.sync?.lastError === "the server refused the invite token");
    expect(ctx.store.getState().sync).toMatchObject({ state: "Error", needsInviteToken: true });

    const info = await ctx.store.getState().configureSync(SERVER, "invite-123");
    expect(info.needsInviteToken).toBe(false);
    await settled(ctx, (s) => s.sync?.state === "Idle" && s.sync.lastPullAt !== undefined);
    expect(ctx.store.getState().sync).toMatchObject({ needsInviteToken: false });
    expect(ctx.store.getState().sync?.lastError).toBeUndefined();
  });

  it("disconnectSync turns sync off and forgets the server", async () => {
    const ctx = setup({ workspaceName: "Research", sync: { state: "idle" } });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().sync?.configured).toBe(true);
    await ctx.store.getState().disconnectSync();
    expect(ctx.requests).toContainEqual({ type: "SyncConfigure", data: {} });
    await settled(ctx, (s) => s.sync?.configured === false);
    expect(ctx.store.getState().sync).toMatchObject({ state: "Disabled", forkDetected: false });
    expect(ctx.store.getState().sync?.serverUrl).toBeUndefined();
  });

  it("syncNow asks for a cycle", async () => {
    const ctx = setup({ workspaceName: "Research", sync: { state: "offline" } });
    await settled(ctx, (s) => s.phase === "ready");
    ctx.memory.setSyncMode("idle");
    await ctx.store.getState().syncNow();
    expect(ctx.types()).toContain("SyncNow");
    await settled(ctx, (s) => s.sync?.state === "Idle" && s.sync.lastError === undefined);
  });

  it("a slow read never overwrites a newer answer", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", syncStepMs: 10_000 });
    let slow: (() => void) | null = null;
    const ctx = setup({}, {
      dispatch: async (request) => {
        if (request.type === "SyncStatus" && slow === null && (await memory.coreState()) === "ready") {
          const answer = await memory.dispatch(request);
          return new Promise((resolve) => {
            slow = () => {
              resolve(answer);
            };
          });
        }
        return memory.dispatch(request);
      },
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
    });
    // Boot's read is held back (answering "Disabled"); the configuration answers meanwhile.
    await vi.waitFor(() => {
      expect(slow).not.toBeNull();
    });
    await ctx.store.getState().configureSync(SERVER);
    expect(ctx.store.getState().sync?.configured).toBe(true);
    (slow as (() => void) | null)?.();
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().sync?.configured).toBe(true);
  });
});

describe("sync: lock", () => {
  it("forgets the status while locked and reads it again after unlocking", async () => {
    const ctx = setup({
      workspaceName: "Research",
      security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE },
      sync: { state: "forked" },
    });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().sync?.forkDetected).toBe(true);
    await ctx.store.getState().lock();
    expect(ctx.store.getState()).toMatchObject({ phase: "locked", sync: null });
    await ctx.store.getState().unlock(PASSPHRASE);
    expect(ctx.store.getState().sync).toMatchObject({ configured: true, forkDetected: true });
  });
});
