import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { ACCENT_KEY, selectAccent } from "./accent";
import { LANGUAGE_KEY } from "./i18n";
import { THEME_KEY, createAppStore, parseTheme, selectTheme } from "./store";
import type { AppState } from "./store";

function setup(options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport(options);
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return memory.dispatch(request);
    },
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
  };
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  return { memory, store, stop, requests };
}

const settled = (store: ReturnType<typeof createAppStore>, check: (state: AppState) => boolean) =>
  vi.waitFor(
    () => {
      expect(check(store.getState())).toBe(true);
    },
    { timeout: 4000 },
  );

describe("parseTheme and selectTheme", () => {
  it("accepts the three themes and treats everything else as system", () => {
    expect(parseTheme("light")).toBe("light");
    expect(parseTheme("dark")).toBe("dark");
    expect(parseTheme("system")).toBe("system");
    for (const value of [null, undefined, "", "Dark", "solarized", 1, true, ["dark"], { theme: "dark" }]) {
      expect(parseTheme(value), JSON.stringify(value)).toBe("system");
    }
  });
});

describe("boot", () => {
  it("starts in the loading phase", () => {
    const store = createAppStore(new CoreClient(new MemoryTransport()));
    expect(store.getState()).toMatchObject({ phase: "loading", route: "home", status: null, workspace: null });
  });

  it("ends in onboarding on a fresh install and fills status, device and theme", async () => {
    const { store } = setup();
    await settled(store, (s) => s.phase === "onboarding");
    const state = store.getState();
    expect(state.workspace).toBeNull();
    expect(state.status).toMatchObject({ workspaceInitialized: false, schemaVersion: 1 });
    expect(state.device).toMatchObject({ platform: "memory" });
    expect(state.settings).toEqual({ [THEME_KEY]: null, [ACCENT_KEY]: null, [LANGUAGE_KEY]: null });
    expect(selectTheme(state)).toBe("system");
    expect(selectAccent(state)).toBe("blue");
  });

  it("ends in ready with the workspace and the stored theme", async () => {
    const { store } = setup({ workspaceName: "Research", settings: { "ui.theme": "dark" } });
    await settled(store, (s) => s.phase === "ready");
    expect(store.getState().workspace).toMatchObject({ name: "Research" });
    expect(selectTheme(store.getState())).toBe("dark");
  });

  it("reads the stored accent with the theme, before the app is ready, and writes nothing", async () => {
    const { store, requests } = setup({ workspaceName: "Research", settings: { "ui.theme": "dark", "ui.accent": "green" } });
    await settled(store, (s) => s.phase === "ready");
    expect(store.getState().settings).toMatchObject({ [THEME_KEY]: "dark", [ACCENT_KEY]: "green" });
    expect(selectAccent(store.getState())).toBe("green");
    expect(requests.filter((request) => request.type === "SettingsSet")).toEqual([]);
  });

  it("takes an accent it does not know for the default blue", async () => {
    const { store } = setup({ workspaceName: "Research", settings: { "ui.accent": "teal" } });
    await settled(store, (s) => s.phase === "ready");
    expect(selectAccent(store.getState())).toBe("blue");
  });

  it("waits while the core is opening and sends nothing before it is ready", async () => {
    const { memory, store, requests } = setup({ coreState: "opening" });
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(store.getState().phase).toBe("loading");
    expect(requests).toEqual([]);
    memory.setCoreState("ready");
    await settled(store, (s) => s.phase === "onboarding");
  });

  it("goes fatal with the core's own error when opening failed", async () => {
    const error = { code: "KeystoreUnavailable", message: "no usable keystore" } as const;
    const { store } = setup({ coreState: { failed: error } });
    await settled(store, (s) => s.phase === "fatal");
    expect(store.getState().fatalError).toEqual(error);
  });

  it("goes fatal when a request fails, keeping code and message", async () => {
    const { memory, store } = setup();
    memory.failNext({ code: "Crypto", message: "the database key was rejected" });
    await settled(store, (s) => s.phase === "fatal");
    expect(store.getState().fatalError).toEqual({ code: "Crypto", message: "the database key was rejected" });
  });

  it("can be run again and recovers", async () => {
    const { memory, store } = setup({ coreState: { failed: { code: "Internal", message: "boom" } } });
    await settled(store, (s) => s.phase === "fatal");
    memory.setCoreState("ready");
    const retry = store.getState().boot();
    expect(store.getState()).toMatchObject({ phase: "loading", fatalError: null });
    await retry;
    expect(store.getState().phase).toBe("onboarding");
  });

  it("does nothing more after stop, even if the core becomes ready", async () => {
    const { memory, store, stop, requests } = setup({ coreState: "opening" });
    await new Promise((resolve) => setTimeout(resolve, 15));
    stop();
    memory.setCoreState("ready");
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(store.getState().phase).toBe("loading");
    expect(requests).toEqual([]);
  });

  it("can be started again after a stop (StrictMode)", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research" });
    const store = createAppStore(new CoreClient(memory), { corePollMs: 5 });
    const stopFirst = store.getState().start();
    stopFirst();
    const stopSecond = store.getState().start();
    await settled(store, (s) => s.phase === "ready");
    stopSecond();
  });

  it("stops listening for events after stop", async () => {
    const { memory, store, stop } = setup({ workspaceName: "Research" });
    await settled(store, (s) => s.phase === "ready");
    stop();
    await memory.dispatch({ type: "DeviceRename", data: { name: "After stop" } });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(store.getState().device?.name).not.toBe("After stop");
  });
});

describe("events refresh only the slice they name", () => {
  it("DeviceRenamed re-reads the device", async () => {
    const { memory, store, requests } = setup({ workspaceName: "Research" });
    await settled(store, (s) => s.phase === "ready");
    requests.length = 0;
    await memory.dispatch({ type: "DeviceRename", data: { name: "Elsewhere" } });
    await settled(store, (s) => s.device?.name === "Elsewhere");
    // Renamed straight in the core, so the one request seen is the store's own refresh.
    expect(requests.map((r) => r.type)).toEqual(["DeviceInfo"]);
  });

  it("SettingChanged re-reads a setting the UI has read before", async () => {
    const { memory, store } = setup({ workspaceName: "Research" });
    await settled(store, (s) => s.phase === "ready");
    await memory.dispatch({ type: "SettingsSet", data: { key: THEME_KEY, value: "dark" } });
    await settled(store, (s) => selectTheme(s) === "dark");
  });

  it("SettingChanged ignores settings nobody has read", async () => {
    const { memory, store, requests } = setup({ workspaceName: "Research" });
    await settled(store, (s) => s.phase === "ready");
    requests.length = 0;
    // Written straight to the core, so only requests the store makes itself show up.
    await memory.dispatch({ type: "SettingsSet", data: { key: "app.other", value: 1 } });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(requests).toEqual([]);
    expect(store.getState().settings).not.toHaveProperty("app.other");
  });

  it("WorkspaceCreated re-reads the workspace and the status and leaves onboarding", async () => {
    const { memory, store } = setup();
    await settled(store, (s) => s.phase === "onboarding");
    await memory.dispatch({ type: "WorkspaceCreate", data: { name: "Made elsewhere" } });
    await settled(store, (s) => s.phase === "ready" && s.status?.workspaceInitialized === true);
    expect(store.getState().workspace).toMatchObject({ name: "Made elsewhere" });
  });

  it("a failing refresh keeps the old data and does not crash", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const memory = new MemoryTransport({ workspaceName: "Research", deviceName: "Before" });
    let failDeviceReads = false;
    const transport: Transport = {
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      dispatch: (request) =>
        failDeviceReads && request.type === "DeviceInfo"
          ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
          : memory.dispatch(request),
    };
    const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
    store.getState().start();
    await settled(store, (s) => s.phase === "ready");

    failDeviceReads = true;
    await memory.dispatch({ type: "DeviceRename", data: { name: "After" } });
    await vi.waitFor(
      () => {
        expect(logged).toHaveBeenCalled();
      },
      { timeout: 4000 },
    );
    expect(store.getState().device?.name).toBe("Before");
    expect(store.getState().phase).toBe("ready");
    logged.mockRestore();
  });
});

describe("actions", () => {
  it("createWorkspace stores the workspace, moves on to the recovery kit and refreshes the status", async () => {
    const { store } = setup();
    await settled(store, (s) => s.phase === "onboarding");
    store.getState().navigate("diagnostics");
    await store.getState().createWorkspace("  Personal ");
    expect(store.getState()).toMatchObject({
      phase: "onboarding",
      onboardingStep: "kit",
      route: "home",
      workspace: { name: "Personal" },
    });
    await settled(store, (s) => s.status?.workspaceInitialized === true);
    // What boot reads for an existing workspace arrives too: the security status (no kit yet).
    await settled(store, (s) => s.security !== null && s.security.recoveryKit === undefined);
  });

  it("finishOnboarding leaves the onboarding for home", async () => {
    const { store } = setup();
    await settled(store, (s) => s.phase === "onboarding");
    await store.getState().createWorkspace("Personal");
    store.getState().navigate("settings");
    store.getState().finishOnboarding();
    expect(store.getState()).toMatchObject({ phase: "ready", onboardingStep: "workspace", route: "home" });
  });

  it("createWorkspace rejects with the core's error and stays in onboarding", async () => {
    const { store } = setup();
    await settled(store, (s) => s.phase === "onboarding");
    const error: unknown = await store.getState().createWorkspace("   ").then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "name" } });
    expect(store.getState().phase).toBe("onboarding");
  });

  it("setSetting writes through and updates the cache; a failure leaves the cache alone", async () => {
    const { memory, store } = setup({ workspaceName: "Research" });
    await settled(store, (s) => s.phase === "ready");
    await store.getState().setSetting(THEME_KEY, "dark");
    expect(selectTheme(store.getState())).toBe("dark");

    memory.failNext({ code: "Storage", message: "disk I/O error" });
    await expect(store.getState().setSetting(THEME_KEY, "light")).rejects.toMatchObject({ code: "Storage" });
    expect(selectTheme(store.getState())).toBe("dark");
  });

  it("renameDevice updates the device with what the core stored", async () => {
    const { store } = setup({ workspaceName: "Research" });
    await settled(store, (s) => s.phase === "ready");
    await store.getState().renameDevice("  New name ");
    expect(store.getState().device?.name).toBe("New name");
    await expect(store.getState().renameDevice(" ")).rejects.toMatchObject({ code: "InvalidInput" });
    expect(store.getState().device?.name).toBe("New name");
  });

  it("navigate changes the route", () => {
    const store = createAppStore(new CoreClient(new MemoryTransport()));
    store.getState().navigate("settings");
    expect(store.getState().route).toBe("settings");
  });

  it("refreshStatus re-reads the status", async () => {
    const { store, requests } = setup({ workspaceName: "Research" });
    await settled(store, (s) => s.phase === "ready");
    const before = requests.filter((r) => r.type === "AppStatus").length;
    await store.getState().refreshStatus();
    expect(requests.filter((r) => r.type === "AppStatus")).toHaveLength(before + 1);
  });
});
