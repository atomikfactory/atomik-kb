import { describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { AppStatus } from "./generated/AppStatus";
import type { DeviceInfo } from "./generated/DeviceInfo";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { Workspace } from "./generated/Workspace";
import type { CoreStateInfo, DocTransport, Transport } from "./transport";

const status: AppStatus = {
  workspaceInitialized: true,
  schemaVersion: 1,
  dbEncrypted: true,
  dbCipher: "SQLCipher 4.14.0",
  deviceId: "11111111-1111-4111-8111-111111111111",
  dataDir: "C:\\Users\\me\\AppData\\Roaming\\com.atomik-kb.desktop",
  coreVersion: "0.1.0",
  apiVersion: 1,
  platform: "windows",
};

// Records as the shell sends them: JSON text, so timestamps are plain numbers (ts-rs
// types Rust `i64` as `bigint`, which JSON cannot carry; see `viaWire` in memory.ts).
const workspace = JSON.parse(
  '{"id":"22222222-2222-4222-8222-222222222222","name":"Home","createdAt":1700000000000}',
) as Workspace;

const device = JSON.parse(
  `{"deviceId":"${status.deviceId}","name":"Windows device","platform":"windows","createdAt":1700000000000}`,
) as DeviceInfo;

/** Page documents are not used by these tests; touching them is a test bug. */
const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response | Error, extras: Partial<Transport> = {}) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return response instanceof Error ? Promise.reject(response) : Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
    ...extras,
  };
  return { transport, requests, client: new CoreClient(transport) };
}

describe("CoreClient request and response mapping", () => {
  it("appStatus sends AppStatus and returns the status", async () => {
    const { client, requests } = stubTransport({ type: "AppStatus", data: status });
    await expect(client.appStatus()).resolves.toEqual(status);
    expect(requests).toEqual([{ type: "AppStatus" }]);
  });

  it("workspaceCreate sends the name and returns the workspace", async () => {
    const { client, requests } = stubTransport({ type: "Workspace", data: workspace });
    await expect(client.workspaceCreate("Home")).resolves.toEqual(workspace);
    expect(requests).toEqual([{ type: "WorkspaceCreate", data: { name: "Home" } }]);
  });

  it("workspaceCreate fails when the core answers with no workspace", async () => {
    const { client } = stubTransport({ type: "Workspace", data: null });
    await expect(client.workspaceCreate("Home")).rejects.toMatchObject({ code: "Internal" });
  });

  it("workspaceGet returns the workspace or null", async () => {
    const some = stubTransport({ type: "Workspace", data: workspace });
    await expect(some.client.workspaceGet()).resolves.toEqual(workspace);
    expect(some.requests).toEqual([{ type: "WorkspaceGet" }]);
    const none = stubTransport({ type: "Workspace", data: null });
    await expect(none.client.workspaceGet()).resolves.toBeNull();
  });

  it("settingsGet returns the stored JSON or null", async () => {
    const set = stubTransport({ type: "Setting", data: { nested: [1, "two", null] } });
    await expect(set.client.settingsGet("ui.theme")).resolves.toEqual({ nested: [1, "two", null] });
    expect(set.requests).toEqual([{ type: "SettingsGet", data: { key: "ui.theme" } }]);
    const unset = stubTransport({ type: "Setting", data: null });
    await expect(unset.client.settingsGet("ui.theme")).resolves.toBeNull();
  });

  it("settingsSet sends key and value and resolves with nothing", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    await expect(client.settingsSet("ui.theme", "dark")).resolves.toBeUndefined();
    expect(requests).toEqual([{ type: "SettingsSet", data: { key: "ui.theme", value: "dark" } }]);
  });

  it("deviceInfo and deviceRename return the device", async () => {
    const info = stubTransport({ type: "DeviceInfo", data: device });
    await expect(info.client.deviceInfo()).resolves.toEqual(device);
    expect(info.requests).toEqual([{ type: "DeviceInfo" }]);
    const renamed = stubTransport({ type: "DeviceInfo", data: { ...device, name: "Laptop" } });
    await expect(renamed.client.deviceRename("Laptop")).resolves.toMatchObject({ name: "Laptop" });
    expect(renamed.requests).toEqual([{ type: "DeviceRename", data: { name: "Laptop" } }]);
  });
});

describe("CoreClient response narrowing", () => {
  it("throws an ApiError-shaped Internal error when the response variant is unexpected", async () => {
    const wrong: Response = { type: "Ok" };
    const calls: [string, (client: CoreClient) => Promise<unknown>][] = [
      ["AppStatus", (c) => c.appStatus()],
      ["WorkspaceCreate", (c) => c.workspaceCreate("x")],
      ["WorkspaceGet", (c) => c.workspaceGet()],
      ["SettingsGet", (c) => c.settingsGet("ui.theme")],
      ["DeviceInfo", (c) => c.deviceInfo()],
      ["DeviceRename", (c) => c.deviceRename("x")],
    ];
    for (const [name, call] of calls) {
      const { client } = stubTransport(wrong);
      const error: unknown = await call(client).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error, name).toBeInstanceOf(CoreError);
      expect(error, name).toMatchObject({ code: "Internal" });
      expect((error as CoreError).message, name).toContain(`Unexpected response to ${name}`);
    }
  });

  it("settingsSet rejects any response other than Ok", async () => {
    const { client } = stubTransport({ type: "Setting", data: "dark" });
    await expect(client.settingsSet("ui.theme", "dark")).rejects.toMatchObject({
      code: "Internal",
      message: expect.stringContaining("expected Ok, got Setting") as unknown,
    });
  });

  it("treats a malformed response as an Internal error instead of crashing", async () => {
    const { client } = stubTransport(null as unknown as Response);
    await expect(client.appStatus()).rejects.toMatchObject({
      code: "Internal",
      message: expect.stringContaining("malformed") as unknown,
    });
  });

  it("passes transport failures through untouched", async () => {
    const failure = new CoreError({ code: "NotFound", message: "nothing here" });
    const { client } = stubTransport(failure);
    await expect(client.workspaceGet()).rejects.toBe(failure);
  });
});

describe("CoreClient lifecycle and events", () => {
  it("coreState counts a transport without lifecycle support as ready", async () => {
    const { client } = stubTransport({ type: "Ok" });
    await expect(client.coreState()).resolves.toBe("ready");
  });

  it("coreState forwards what the transport reports", async () => {
    const failed: CoreStateInfo = { failed: { code: "KeystoreUnavailable", message: "no keystore" } };
    const { client } = stubTransport({ type: "Ok" }, { coreState: () => Promise.resolve(failed) });
    await expect(client.coreState()).resolves.toEqual(failed);
  });

  it("subscribe registers on the transport and returns its unsubscribe function", () => {
    const unsubscribe = vi.fn();
    const subscribe = vi.fn<Transport["subscribe"]>(() => unsubscribe);
    const { client } = stubTransport({ type: "Ok" }, { subscribe });
    const listener = (): void => undefined;
    const stop = client.subscribe(listener);
    expect(subscribe).toHaveBeenCalledWith(listener);
    stop();
    expect(unsubscribe).toHaveBeenCalledOnce();
  });
});
