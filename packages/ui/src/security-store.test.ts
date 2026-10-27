import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { ACCENT_KEY } from "./accent";
import { registerWhiteboardCopy, whiteboardCopyState } from "./editor/whiteboard-copy";
import { THEME_KEY, createAppStore } from "./store";
import type { AppState } from "./store";
import { AUTO_LOCK_KEY, parseAutoLockMinutes } from "./store-security";

const NOW = Date.UTC(2026, 9, 3, 12, 0, 0);
const PASSPHRASE = "correct horse battery";
const LOCK_ON = { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE };

const PERSONAL = { name: "Personal", pages: [{ title: "Projects" }, { title: "Journal" }] };

/** The memory core behind a recording transport, and a started store over it. */
function setup(options: MemoryTransportOptions = {}, overrides: Partial<Transport> = {}) {
  const memory = new MemoryTransport({ now: () => NOW, ...options });
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

describe("parseAutoLockMinutes", () => {
  it("accepts the offered delays and treats everything else as off", () => {
    for (const minutes of [0, 1, 5, 15, 60]) {
      expect(parseAutoLockMinutes(minutes)).toBe(minutes);
    }
    for (const value of [null, undefined, 2, -1, 1.5, "5", true, [5], { minutes: 5 }]) {
      expect(parseAutoLockMinutes(value), JSON.stringify(value)).toBe(0);
    }
  });

  it("uses a setting key the core accepts (lowercase)", () => {
    expect(AUTO_LOCK_KEY).toMatch(/^app\.[a-z0-9_.-]+$/);
  });
});

describe("boot", () => {
  it("reads the security status, the devices and the auto-lock delay before showing the app", async () => {
    const ctx = setup({ workspaceName: "Research", security: LOCK_ON, settings: { [AUTO_LOCK_KEY]: 5 } });
    await settled(ctx, (s) => s.phase === "ready");
    const state = ctx.store.getState();
    expect(state.security).toMatchObject({ recoveryKit: { createdAt: NOW }, appLock: { enabled: true } });
    expect(state.devices).toHaveLength(1);
    expect(state.devices[0]).toMatchObject({ isThisDevice: true, addedViaKit: false });
    expect(state.settings[AUTO_LOCK_KEY]).toBe(5);
    expect(ctx.types()).toEqual(expect.arrayContaining(["SecurityStatus", "DeviceList"]));
  });

  it("reads none of it without a workspace", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "onboarding");
    expect(ctx.types()).not.toContain("SecurityStatus");
    expect(ctx.types()).not.toContain("DeviceList");
    expect(ctx.store.getState().security).toBeNull();
    expect(ctx.store.getState().settings).not.toHaveProperty(AUTO_LOCK_KEY);
  });

  it("opens the app without the security status when the core cannot report it", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [PERSONAL] });
    const ctx = setup({}, {
      dispatch: (request) =>
        request.type === "SecurityStatus"
          ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
          : memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
    });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState()).toMatchObject({ security: null, devices: [] });
    expect(ctx.store.getState().spaces).toHaveLength(1);
    expect(logged).toHaveBeenCalledWith("could not read the security status", "Storage");
    logged.mockRestore();
  });

  it("shows the lock screen for a locked core and unlocks into the workspace", async () => {
    const ctx = setup({ workspaceName: "Research", spaces: [PERSONAL], security: { ...LOCK_ON, startLocked: true } });
    await settled(ctx, (s) => s.phase === "locked");
    expect(ctx.requests).toEqual([]);
    expect(ctx.store.getState()).toMatchObject({ workspace: null, spaces: [], security: null });

    await expect(ctx.store.getState().unlock("not the passphrase")).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "passphrase" },
    });
    expect(ctx.store.getState().phase).toBe("locked");

    await ctx.store.getState().unlock(PASSPHRASE);
    expect(ctx.store.getState()).toMatchObject({ phase: "ready", workspace: { name: "Research" } });
    expect(ctx.store.getState().spaces).toHaveLength(1);
    expect(ctx.store.getState().security?.appLock.enabled).toBe(true);
  });

  it("shows the lock screen when the core turns out locked during boot", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: LOCK_ON });
    await memory.lock();
    // A shell that still said "ready" (locked by another window in between).
    const ctx = setup({}, {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => Promise.resolve("ready"),
      docs: memory.docs,
    });
    await settled(ctx, (s) => s.phase === "locked");
    expect(ctx.store.getState().fatalError).toBeNull();
  });

  it("starts again like at launch when another window creates the workspace during onboarding", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "onboarding");
    await ctx.memory.dispatch({ type: "WorkspaceCreate", data: { name: "Elsewhere" } });
    await settled(ctx, (s) => s.phase === "ready" && s.security !== null);
    expect(ctx.store.getState().workspace).toMatchObject({ name: "Elsewhere" });
    expect(ctx.store.getState().settings).toHaveProperty(AUTO_LOCK_KEY, null);
  });

  it("knows what the shell can do", async () => {
    const full = setup({ workspaceName: "Research" });
    expect(full.store.getState().capabilities).toEqual({ appLock: true, backups: true });
    const memory = new MemoryTransport();
    const bare = setup({}, {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      lock: undefined,
      unlock: undefined,
      backupExport: undefined,
      backupImport: undefined,
    });
    expect(bare.store.getState().capabilities).toEqual({ appLock: false, backups: false });
    await settled(full, (s) => s.phase === "ready");
  });
});

describe("lock", () => {
  it("forgets everything the workspace put in memory, keeps the theme and the accent, and comes back on unlock", async () => {
    const ctx = setup({
      workspaceName: "Research",
      spaces: [PERSONAL],
      settings: { [THEME_KEY]: "dark", [ACCENT_KEY]: "green", [AUTO_LOCK_KEY]: 5 },
      security: LOCK_ON,
    });
    await settled(ctx, (s) => s.phase === "ready" && s.tree !== null);
    const state = ctx.store.getState();
    const opened = state.tree?.roots[0]?.page.id ?? null;
    state.selectPage(opened);
    state.navigate("settings");
    state.lookupPage(state.tree?.roots[1]?.page.id ?? "");
    await ctx.store.getState().runSearch("Journal");
    expect(ctx.store.getState().search.hits).not.toEqual([]);

    await ctx.store.getState().lock();
    expect(ctx.store.getState()).toMatchObject({
      phase: "locked",
      workspace: null,
      device: null,
      status: null,
      spaces: [],
      spacesLoaded: false,
      currentSpaceId: null,
      tree: null,
      selectedPageId: null,
      tabs: [],
      activeTabId: null,
      closedTabs: [],
      collapsed: {},
      pageLookup: {},
      backlinks: null,
      search: { open: false, query: "", hits: [] },
      security: null,
      devices: [],
      settings: { [THEME_KEY]: "dark", [ACCENT_KEY]: "green" },
      route: "settings",
    });
    await expect(ctx.memory.coreState()).resolves.toBe("locked");

    await ctx.store.getState().unlock(PASSPHRASE);
    // The tabs come back from ui.tabs (ADR-0034 section 5), so the page that was open is open again.
    expect(ctx.store.getState()).toMatchObject({ phase: "ready", route: "settings", selectedPageId: opened });
    expect(ctx.store.getState().tabs).toHaveLength(1);
    expect(ctx.store.getState().spaces).toHaveLength(1);
    expect(ctx.store.getState().settings).toMatchObject({ [THEME_KEY]: "dark", [ACCENT_KEY]: "green", [AUTO_LOCK_KEY]: 5 });
  });

  it("drops the drawings held for a paste (ADR-0033)", async () => {
    const ctx = setup({ workspaceName: "Research", spaces: [PERSONAL], security: LOCK_ON });
    await settled(ctx, (s) => s.phase === "ready" && s.tree !== null);
    registerWhiteboardCopy("copy", "source");
    expect(whiteboardCopyState().pending).toBe(1);
    await ctx.store.getState().lock();
    expect(whiteboardCopyState()).toEqual({ stashed: 0, pending: 0 });
  });

  it("changes nothing when the core refuses (the lock is off)", async () => {
    const ctx = setup({ workspaceName: "Research", spaces: [PERSONAL], security: { kitCreatedAt: NOW } });
    await settled(ctx, (s) => s.phase === "ready" && s.tree !== null);
    await expect(ctx.store.getState().lock()).rejects.toMatchObject({ code: "InvalidInput" });
    expect(ctx.store.getState()).toMatchObject({ phase: "ready", workspace: { name: "Research" } });
    expect(ctx.store.getState().spaces).toHaveLength(1);
  });

  it("closes the open page documents without asking the locked core", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [PERSONAL], security: LOCK_ON });
    const closed: string[] = [];
    const ctx = setup({}, {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: {
        ...memory.docs,
        close: (pageId) => {
          closed.push(pageId);
          return memory.docs.close(pageId);
        },
      },
      lock: () => memory.lock(),
      unlock: (passphrase) => memory.unlock(passphrase),
    });
    await settled(ctx, (s) => s.phase === "ready" && s.tree !== null);
    const pageId = ctx.store.getState().tree?.roots[0]?.page.id ?? "";
    const { pageDocs } = ctx.store.getState();
    const session = pageDocs.acquire(pageId);
    await vi.waitFor(() => {
      expect(session.getSnapshot().status).toBe("ready");
    });
    await ctx.store.getState().lock();
    expect(pageDocs.peek(pageId)).toBeNull();
    pageDocs.release(session); // the editor letting go afterwards changes nothing
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(closed).toEqual([]);
    await expect(memory.coreState()).resolves.toBe("locked");
  });
});

describe("events", () => {
  it("SecurityChanged reads the status and the devices again", async () => {
    const ctx = setup({ workspaceName: "Research" });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().security?.recoveryKit).toBeUndefined();
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "RecoveryKitCreate" });
    await settled(ctx, (s) => s.security?.recoveryKit !== undefined && s.devices.length === 1);
    expect(ctx.types().sort()).toEqual(["DeviceList", "SecurityStatus"]);
  });

  it("DeviceRenamed renames this device in the list without reading it again", async () => {
    const ctx = setup({ workspaceName: "Research", deviceName: "Desk PC", security: { kitCreatedAt: NOW } });
    await settled(ctx, (s) => s.phase === "ready");
    ctx.requests.length = 0;
    await ctx.memory.dispatch({ type: "DeviceRename", data: { name: "Laptop" } });
    await settled(ctx, (s) => s.device?.name === "Laptop");
    expect(ctx.store.getState().devices.map((device) => device.name)).toEqual(["Laptop"]);
    expect(ctx.types()).toEqual(["DeviceInfo"]);
  });

  it("a SettingChanged for the auto-lock delay is followed", async () => {
    const ctx = setup({ workspaceName: "Research", security: LOCK_ON });
    await settled(ctx, (s) => s.phase === "ready");
    await ctx.memory.dispatch({ type: "SettingsSet", data: { key: AUTO_LOCK_KEY, value: 15 } });
    await settled(ctx, (s) => s.settings[AUTO_LOCK_KEY] === 15);
  });
});

describe("actions", () => {
  it("createRecoveryKit hands the kit to the caller and keeps none of it", async () => {
    const ctx = setup({ workspaceName: "Research" });
    await settled(ctx, (s) => s.phase === "ready");
    const kit = await ctx.store.getState().createRecoveryKit();
    expect(kit.words).toHaveLength(24);
    await settled(ctx, (s) => s.security?.recoveryKit !== undefined);
    const kept = JSON.stringify(ctx.store.getState());
    expect(kept).not.toContain(kit.words.join(" "));
    expect(kept).not.toContain(kit.code);
    await expect(ctx.store.getState().verifyRecoveryKit(kit.words.join(" "))).resolves.toBeUndefined();
    await settled(ctx, (s) => s.security?.recoveryKit?.verifiedAt === NOW);
  });

  it("app lock actions go to the core and come back through SecurityChanged", async () => {
    const ctx = setup({ workspaceName: "Research", security: { kitCreatedAt: NOW } });
    await settled(ctx, (s) => s.phase === "ready");
    await ctx.store.getState().enableAppLock(PASSPHRASE);
    await settled(ctx, (s) => s.security?.appLock.enabled === true);
    await ctx.store.getState().changeAppLockPassphrase(PASSPHRASE, "another passphrase");
    await expect(ctx.store.getState().disableAppLock(PASSPHRASE)).rejects.toMatchObject({ code: "InvalidInput" });
    await ctx.store.getState().disableAppLock("another passphrase");
    await settled(ctx, (s) => s.security?.appLock.enabled === false);
  });

  it("reissueRecoveryKit returns a new kit and the status says when", async () => {
    const ctx = setup({ workspaceName: "Research", security: LOCK_ON });
    await settled(ctx, (s) => s.phase === "ready");
    await expect(ctx.store.getState().reissueRecoveryKit()).rejects.toMatchObject({ details: { field: "passphrase" } });
    const kit = await ctx.store.getState().reissueRecoveryKit(PASSPHRASE);
    expect(kit.words).toHaveLength(24);
    await settled(ctx, (s) => s.security?.recoveryKit?.reissuedAt === NOW);
  });

  it("exportBackup and importBackup restore a workspace into a fresh installation, which starts again", async () => {
    const source = setup({ workspaceName: "Research", spaces: [PERSONAL], security: { kitCreatedAt: NOW } });
    await settled(source, (s) => s.phase === "ready");
    const kit = await source.store.getState().reissueRecoveryKit();
    const { path, summary } = await source.store.getState().exportBackup();
    expect(summary.documents).toBeGreaterThan(0);

    const target = setup({ files: source.memory.files });
    await settled(target, (s) => s.phase === "onboarding");
    await expect(target.store.getState().importBackup()).rejects.toMatchObject({ details: { field: "kit" } });
    const restored = await target.store.getState().importBackup(kit.words.join(" "), path);
    expect(restored.path).toBe(path);
    expect(target.store.getState()).toMatchObject({ phase: "ready", workspace: { name: "Research" } });
    expect(target.store.getState().spaces).toHaveLength(1);
    expect(target.store.getState().devices.some((device) => device.isThisDevice && device.addedViaKit)).toBe(true);
  });
});
