import { describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { Request } from "./generated/Request";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { API_VERSION } from "./version";

/** Deterministic ids and clock so assertions can name exact values. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  let clock = 1_700_000_000_000;
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

describe("AppStatus", () => {
  it("reports the defaults of an in-memory core before onboarding", async () => {
    const { client } = setup();
    await expect(client.appStatus()).resolves.toEqual({
      workspaceInitialized: false,
      schemaVersion: 1,
      dbEncrypted: false,
      dbCipher: "none",
      deviceId: "00000000-0000-4000-8000-000000000001",
      dataDir: "(memory)",
      coreVersion: "0.0.0-memory",
      apiVersion: API_VERSION,
      platform: "memory",
    });
  });

  it("applies overrides for the static fields and keeps the platform consistent", async () => {
    const { client } = setup({
      status: { platform: "windows", dbEncrypted: true, dbCipher: "SQLCipher 4.14.0", apiVersion: 2 },
    });
    await expect(client.appStatus()).resolves.toMatchObject({
      platform: "windows",
      dbEncrypted: true,
      dbCipher: "SQLCipher 4.14.0",
      apiVersion: 2,
    });
    await expect(client.deviceInfo()).resolves.toMatchObject({ platform: "windows", name: "Windows device" });
  });

  it("flips workspaceInitialized once the workspace exists", async () => {
    const { client } = setup();
    await client.workspaceCreate("Home");
    await expect(client.appStatus()).resolves.toMatchObject({ workspaceInitialized: true });
  });

  it("is the same device id as DeviceInfo", async () => {
    const { client } = setup();
    const [status, device] = await Promise.all([client.appStatus(), client.deviceInfo()]);
    expect(status.deviceId).toBe(device.deviceId);
  });
});

describe("workspace", () => {
  it("is null before onboarding", async () => {
    const { client } = setup();
    await expect(client.workspaceGet()).resolves.toBeNull();
  });

  it("is created with a trimmed name, an id and a creation time, and can be read back", async () => {
    const { client } = setup();
    const created = await client.workspaceCreate("  My notes \n");
    expect(created).toEqual({
      id: "00000000-0000-4000-8000-000000000002",
      name: "My notes",
      createdAt: 1_700_000_000_001,
    });
    await expect(client.workspaceGet()).resolves.toEqual(created);
  });

  it("emits WorkspaceCreated with the new id", async () => {
    const { client, events } = setup();
    const created = await client.workspaceCreate("Home");
    expect(events).toEqual([{ type: "WorkspaceCreated", data: { id: created.id } }]);
  });

  it("rejects an empty or blank name with InvalidInput naming the field", async () => {
    const { client, events } = setup();
    for (const name of ["", "   ", "\t\n"]) {
      const error = await failure(client.workspaceCreate(name));
      expect(error.toApiError()).toEqual({
        code: "InvalidInput",
        message: "must not be empty",
        details: { field: "name" },
      });
    }
    await expect(client.workspaceGet()).resolves.toBeNull();
    expect(events).toEqual([]);
  });

  it("accepts exactly 200 characters and rejects 201, counting characters not UTF-16 units", async () => {
    const { client } = setup();
    const tooLong = await failure(client.workspaceCreate("x".repeat(201)));
    expect(tooLong).toMatchObject({
      code: "InvalidInput",
      message: "must be at most 200 characters",
      details: { field: "name" },
    });
    // 200 astral characters are 400 UTF-16 code units but 200 characters.
    const created = await client.workspaceCreate("\u{1F600}".repeat(200));
    expect(Array.from(created.name)).toHaveLength(200);
  });

  it("measures the length after trimming", async () => {
    const { client } = setup();
    await expect(client.workspaceCreate(`  ${"x".repeat(200)}  `)).resolves.toMatchObject({
      name: "x".repeat(200),
    });
  });

  it("rejects control characters", async () => {
    const { client } = setup();
    const error = await failure(client.workspaceCreate("bad\u0007name"));
    expect(error).toMatchObject({
      code: "InvalidInput",
      message: "must not contain control characters",
      details: { field: "name" },
    });
  });

  it("rejects a second workspace with AlreadyExists and emits nothing more", async () => {
    const { client, events } = setup();
    await client.workspaceCreate("First");
    const error = await failure(client.workspaceCreate("Second"));
    expect(error).toMatchObject({ code: "AlreadyExists" });
    await expect(client.workspaceGet()).resolves.toMatchObject({ name: "First" });
    expect(events).toHaveLength(1);
  });

  it("can start with a workspace already created", async () => {
    const { client } = setup({ workspaceName: "Seeded" });
    await expect(client.workspaceGet()).resolves.toMatchObject({ name: "Seeded" });
    await expect(client.appStatus()).resolves.toMatchObject({ workspaceInitialized: true });
  });

  it("hands out copies, not its own state", async () => {
    const { client } = setup({ workspaceName: "Seeded" });
    const first = await client.workspaceGet();
    if (first === null) {
      throw new Error("expected a workspace");
    }
    first.name = "tampered";
    await expect(client.workspaceGet()).resolves.toMatchObject({ name: "Seeded" });
  });
});

describe("settings", () => {
  it("returns null for a setting that was never written", async () => {
    const { client } = setup();
    await expect(client.settingsGet("ui.theme")).resolves.toBeNull();
  });

  it("round-trips any JSON value and emits SettingChanged without the value", async () => {
    const { client, events } = setup();
    const value = { list: [1, 2.5, "three", null, true], nested: { deep: "yes" } };
    await client.settingsSet("app.layout", value);
    await expect(client.settingsGet("app.layout")).resolves.toEqual(value);
    expect(events).toEqual([{ type: "SettingChanged", data: { key: "app.layout" } }]);
  });

  it("overwrites an existing value and emits an event for every write", async () => {
    const { client, events } = setup();
    await client.settingsSet("ui.theme", "dark");
    await client.settingsSet("ui.theme", "light");
    await expect(client.settingsGet("ui.theme")).resolves.toBe("light");
    expect(events).toHaveLength(2);
  });

  it("stores values by value: later mutation by the caller or reader does not leak in", async () => {
    const { client } = setup();
    const written = { items: ["a"] };
    await client.settingsSet("ui.list", written);
    written.items.push("b");
    const read = await client.settingsGet("ui.list");
    expect(read).toEqual({ items: ["a"] });
    (read as { items: string[] }).items.push("c");
    await expect(client.settingsGet("ui.list")).resolves.toEqual({ items: ["a"] });
  });

  it("accepts the documented key grammar", async () => {
    const { client } = setup();
    for (const key of ["ui.theme", "app.last-dir", "ui.editor.font_size", "app.x1"]) {
      await client.settingsSet(key, true);
      await expect(client.settingsGet(key)).resolves.toBe(true);
    }
  });

  it("rejects malformed keys on read and write with details.field = key", async () => {
    const { client, events } = setup();
    const bad = ["", "ui", "ui.", ".ui", "ui..a", "Ui.theme", "sys.x", "ui.a b", "ui.é", `ui.${"x".repeat(126)}`];
    for (const key of bad) {
      for (const call of [() => client.settingsGet(key), () => client.settingsSet(key, 1)]) {
        const error = await failure(call());
        expect(error, key).toMatchObject({ code: "InvalidInput", details: { field: "key" } });
      }
    }
    expect(events).toEqual([]);
  });

  it("explains which rule a key broke", async () => {
    const { client } = setup();
    expect((await failure(client.settingsGet("sys.x"))).message).toBe("must start with one of: ui, app");
    expect((await failure(client.settingsGet("ui"))).message).toBe("must have a name after the namespace");
    expect((await failure(client.settingsGet("ui..a"))).message).toBe("must not contain empty segments");
    expect((await failure(client.settingsGet("Ui.a"))).message).toBe("may only contain a-z, 0-9, '_', '-' and '.'");
    expect((await failure(client.settingsGet(""))).message).toBe("must be 1 to 128 bytes");
  });

  it("limits serialised values to 64 KiB", async () => {
    const { client, events } = setup();
    // The JSON text of a string adds two quotes: 65534 characters serialise to exactly 65536 bytes.
    await expect(client.settingsSet("app.blob", "x".repeat(64 * 1024 - 2))).resolves.toBeUndefined();
    const error = await failure(client.settingsSet("app.blob", "x".repeat(64 * 1024 - 1)));
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
    expect(events).toHaveLength(1);
    await expect(client.settingsGet("app.blob")).resolves.toHaveLength(64 * 1024 - 2);
  });

  it("measures the limit in UTF-8 bytes", async () => {
    const { client } = setup();
    const error = await failure(client.settingsSet("app.blob", "€".repeat(30_000)));
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
  });

  it("can start with settings in place", async () => {
    const { client } = setup({ settings: { "ui.theme": "dark" } });
    await expect(client.settingsGet("ui.theme")).resolves.toBe("dark");
  });
});

describe("device", () => {
  it("is named after the platform by default", async () => {
    for (const [platform, name] of [
      ["windows", "Windows device"],
      ["macos", "macOS device"],
      ["linux", "Linux device"],
      ["memory", "memory device"],
    ] as const) {
      const { client } = setup({ status: { platform } });
      await expect(client.deviceInfo()).resolves.toMatchObject({ name });
    }
  });

  it("reports id, platform and first-launch time", async () => {
    const { client } = setup({ deviceName: "Desk" });
    await expect(client.deviceInfo()).resolves.toEqual({
      deviceId: "00000000-0000-4000-8000-000000000001",
      name: "Desk",
      platform: "memory",
      createdAt: 1_700_000_000_000,
    });
  });

  it("renames with a trimmed name, returns the new record and emits DeviceRenamed", async () => {
    const { client, events } = setup();
    const renamed = await client.deviceRename("  Work laptop ");
    expect(renamed.name).toBe("Work laptop");
    await expect(client.deviceInfo()).resolves.toEqual(renamed);
    expect(events).toEqual([{ type: "DeviceRenamed", data: { name: "Work laptop" } }]);
  });

  it("limits names to 100 characters and rejects empty or control characters", async () => {
    const { client, events } = setup();
    await expect(client.deviceRename("x".repeat(100))).resolves.toBeDefined();
    const tooLong = await failure(client.deviceRename("x".repeat(101)));
    expect(tooLong).toMatchObject({
      code: "InvalidInput",
      message: "must be at most 100 characters",
      details: { field: "name" },
    });
    const empty = await failure(client.deviceRename("  "));
    expect(empty).toMatchObject({ code: "InvalidInput", message: "must not be empty" });
    const control = await failure(client.deviceRename("a\u0000b"));
    expect(control).toMatchObject({ code: "InvalidInput", message: "must not contain control characters" });
    expect(events).toHaveLength(1);
  });
});

describe("events", () => {
  it("reach listeners before the dispatch promise resolves, in emission order", async () => {
    const { transport } = setup();
    const seen: string[] = [];
    transport.subscribe((event) => seen.push(event.type));
    const pending = transport.dispatch({ type: "SettingsSet", data: { key: "ui.theme", value: "dark" } });
    expect(seen).toEqual(["SettingChanged"]);
    await pending;
    await transport.dispatch({ type: "DeviceRename", data: { name: "A" } });
    expect(seen).toEqual(["SettingChanged", "DeviceRenamed"]);
  });

  it("stop after unsubscribe", async () => {
    const { transport, client } = setup();
    const seen: Event[] = [];
    const stop = transport.subscribe((event) => seen.push(event));
    await client.settingsSet("ui.theme", "dark");
    stop();
    await client.settingsSet("ui.theme", "light");
    expect(seen).toHaveLength(1);
  });

  it("are not emitted by reads or failed requests", async () => {
    const { client, events } = setup();
    await client.appStatus();
    await client.workspaceGet();
    await client.settingsGet("ui.theme");
    await client.deviceInfo();
    await failure(client.settingsSet("nope", 1));
    expect(events).toEqual([]);
  });

  it("do not break the request when a listener throws", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { transport, client } = setup();
    transport.subscribe(() => {
      throw new Error("listener bug");
    });
    await expect(client.settingsSet("ui.theme", "dark")).resolves.toBeUndefined();
    expect(logged).toHaveBeenCalledOnce();
    logged.mockRestore();
  });
});

describe("core lifecycle", () => {
  it("is ready by default", async () => {
    const { transport } = setup();
    await expect(transport.coreState()).resolves.toBe("ready");
  });

  it("rejects requests with 'core is starting' while opening, then serves them once ready", async () => {
    const { transport, client } = setup({ coreState: "opening" });
    await expect(transport.coreState()).resolves.toBe("opening");
    await expect(failure(client.appStatus())).resolves.toMatchObject({
      code: "Internal",
      message: "core is starting",
    });
    transport.setCoreState("ready");
    await expect(client.appStatus()).resolves.toMatchObject({ workspaceInitialized: false });
  });

  it("answers every request with the opening error once failed, like the shell", async () => {
    const error = { code: "KeystoreUnavailable", message: "no usable keystore: no secret service" } as const;
    const { transport, client } = setup({ coreState: { failed: error } });
    await expect(transport.coreState()).resolves.toEqual({ failed: error });
    expect((await failure(client.appStatus())).toApiError()).toEqual(error);
    expect((await failure(client.workspaceCreate("Home"))).toApiError()).toEqual(error);
  });

  it("failNext makes the next requests fail, then recovers", async () => {
    const { transport, client } = setup();
    transport.failNext({ code: "Storage", message: "disk full" }, 2);
    await expect(failure(client.appStatus())).resolves.toMatchObject({ code: "Storage", message: "disk full" });
    await expect(failure(client.deviceInfo())).resolves.toMatchObject({ code: "Storage" });
    await expect(client.deviceInfo()).resolves.toBeDefined();
  });
});

describe("defaults", () => {
  it("generates UUID v4 ids and uses the real clock when not injected", async () => {
    const before = Date.now();
    const client = new CoreClient(new MemoryTransport());
    const workspace = await client.workspaceCreate("Home");
    expect(workspace.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    const createdAt = Number(workspace.createdAt);
    expect(createdAt).toBeGreaterThanOrEqual(before);
    expect(createdAt).toBeLessThanOrEqual(Date.now());
  });

  it("gives separate instances separate state", async () => {
    const a = new CoreClient(new MemoryTransport());
    const b = new CoreClient(new MemoryTransport());
    await a.workspaceCreate("Only in A");
    await expect(b.workspaceGet()).resolves.toBeNull();
    expect((await a.appStatus()).deviceId).not.toBe((await b.appStatus()).deviceId);
  });
});

describe("untyped callers", () => {
  it("get an error for a request type the core does not have, never an empty answer", async () => {
    const { transport } = setup();
    const unknown = { type: "Teleport" } as unknown as Request;
    await expect(transport.dispatch(unknown)).rejects.toMatchObject({
      code: "Internal",
      message: "unknown request type: Teleport",
    });
  });
});
