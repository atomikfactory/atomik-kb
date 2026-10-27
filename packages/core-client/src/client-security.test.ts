import { describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { DeviceEntry } from "./generated/DeviceEntry";
import type { RecoveryKit } from "./generated/RecoveryKit";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SecurityInfo } from "./generated/SecurityInfo";
import type { BackupResult, DocTransport, Transport } from "./transport";

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

const security: SecurityInfo = {
  accountId: "44444444-4444-4444-8444-444444444444",
  recoveryKit: { createdAt: 1_700_000_000_000, verifiedAt: 1_700_000_100_000, stale: false },
  appLock: { enabled: true, memoryKib: 65_536, iterations: 3, parallelism: 4 },
  thisDevice: "11111111-1111-4111-8111-111111111111",
  devices: 1,
  keyring: { spaces: 2, documents: 5, attachments: 0, tombstones: 1 },
  rootGeneration: 0,
  rotationPending: false,
  thisDeviceRevoked: false,
  recoveryPublished: false,
};

const kit: RecoveryKit = {
  words: Array.from({ length: 24 }, () => "abandon"),
  code: "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AA",
  accountId: "44444444-4444-4444-8444-444444444444",
  createdAt: 1_700_000_000_000,
};

const device: DeviceEntry = {
  deviceId: "11111111-1111-4111-8111-111111111111",
  name: "Laptop",
  addedAt: 1_700_000_000_000,
  addedViaKit: false,
  isThisDevice: true,
  signingKey: "ab".repeat(32),
};

const backup: BackupResult = {
  summary: {
    bytes: 2048,
    documents: 3,
    settings: 1,
    createdAt: 1_700_000_000_000,
    accountId: "44444444-4444-4444-8444-444444444444",
    chunks: 2,
    chunkBytes: 5_000_000,
  },
  path: "C:/backups/atomik-kb-2023-11-14.atomikkb",
};

describe("CoreClient security requests", () => {
  it("securityStatus sends SecurityStatus and returns the status", async () => {
    const { client, requests } = stubTransport({ type: "Security", data: security });
    await expect(client.securityStatus()).resolves.toEqual(security);
    expect(requests).toEqual([{ type: "SecurityStatus" }]);
  });

  it("recoveryKitCreate sends RecoveryKitCreate and returns the kit", async () => {
    const { client, requests } = stubTransport({ type: "RecoveryKit", data: kit });
    await expect(client.recoveryKitCreate()).resolves.toEqual(kit);
    expect(requests).toEqual([{ type: "RecoveryKitCreate" }]);
  });

  it("recoveryKitVerify sends the kit as typed and resolves with nothing", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    await expect(client.recoveryKitVerify(" abandon ability ")).resolves.toBeUndefined();
    expect(requests).toEqual([{ type: "RecoveryKitVerify", data: { kit: " abandon ability " } }]);
  });

  it("recoveryKitReissue sends the passphrase only when given", async () => {
    const { client, requests } = stubTransport({ type: "RecoveryKit", data: kit });
    await expect(client.recoveryKitReissue()).resolves.toEqual(kit);
    await expect(client.recoveryKitReissue("correct horse")).resolves.toEqual(kit);
    expect(requests).toEqual([
      { type: "RecoveryKitReissue", data: {} },
      { type: "RecoveryKitReissue", data: { passphrase: "correct horse" } },
    ]);
  });

  it("appLockEnable, appLockDisable and appLockChange send their passphrases", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    await expect(client.appLockEnable("first one")).resolves.toBeUndefined();
    await expect(client.appLockChange("first one", "second one")).resolves.toBeUndefined();
    await expect(client.appLockDisable("second one")).resolves.toBeUndefined();
    expect(requests).toEqual([
      { type: "AppLockEnable", data: { passphrase: "first one" } },
      { type: "AppLockChange", data: { current: "first one", new: "second one" } },
      { type: "AppLockDisable", data: { passphrase: "second one" } },
    ]);
  });

  it("deviceList sends DeviceList and returns the devices", async () => {
    const { client, requests } = stubTransport({ type: "Devices", data: [device] });
    await expect(client.deviceList()).resolves.toEqual([device]);
    expect(requests).toEqual([{ type: "DeviceList" }]);
  });

  it("rejects an unexpected response variant with Internal", async () => {
    const calls: [string, (client: CoreClient) => Promise<unknown>][] = [
      ["SecurityStatus", (c) => c.securityStatus()],
      ["RecoveryKitCreate", (c) => c.recoveryKitCreate()],
      ["RecoveryKitVerify", (c) => c.recoveryKitVerify("x")],
      ["RecoveryKitReissue", (c) => c.recoveryKitReissue()],
      ["AppLockEnable", (c) => c.appLockEnable("12345678")],
      ["AppLockDisable", (c) => c.appLockDisable("12345678")],
      ["AppLockChange", (c) => c.appLockChange("12345678", "87654321")],
      ["DeviceList", (c) => c.deviceList()],
    ];
    for (const [name, call] of calls) {
      const { client } = stubTransport({ type: "Spaces", data: [] });
      const error: unknown = await call(client).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error, name).toBeInstanceOf(CoreError);
      expect((error as CoreError).code, name).toBe("Internal");
      expect((error as CoreError).message, name).toContain(`Unexpected response to ${name}`);
    }
  });

  it("passes the core's refusals through untouched", async () => {
    const wrong = new CoreError({ code: "InvalidInput", message: "wrong passphrase", details: { field: "passphrase" } });
    const { client } = stubTransport(wrong);
    await expect(client.appLockDisable("nope nope")).rejects.toBe(wrong);
    const locked = new CoreError({ code: "Locked", message: "the workspace is locked" });
    await expect(stubTransport(locked).client.securityStatus()).rejects.toBe(locked);
  });
});

describe("CoreClient app lock and backups", () => {
  it("reports the capabilities of the transport", () => {
    const none = stubTransport({ type: "Ok" }).client;
    expect(none.hasAppLock()).toBe(false);
    expect(none.hasBackups()).toBe(false);

    const lockOnly = stubTransport(
      { type: "Ok" },
      { lock: () => Promise.resolve(), unlock: () => Promise.resolve() },
    ).client;
    expect(lockOnly.hasAppLock()).toBe(true);
    expect(lockOnly.hasBackups()).toBe(false);

    const halfLock = stubTransport({ type: "Ok" }, { lock: () => Promise.resolve() }).client;
    expect(halfLock.hasAppLock()).toBe(false);

    const backups = stubTransport(
      { type: "Ok" },
      { backupExport: () => Promise.resolve(backup), backupImport: () => Promise.resolve(backup) },
    ).client;
    expect(backups.hasBackups()).toBe(true);
    expect(backups.hasAppLock()).toBe(false);
  });

  it("lock and unlock call the transport, on the transport", async () => {
    const calls: string[] = [];
    const extras: Partial<Transport> = {
      lock(this: Transport) {
        calls.push(this === transport ? "lock" : "lock (detached)");
        return Promise.resolve();
      },
      unlock(this: Transport, passphrase: string) {
        calls.push(this === transport ? `unlock ${passphrase}` : "unlock (detached)");
        return Promise.resolve();
      },
    };
    const { client, transport } = stubTransport({ type: "Ok" }, extras);
    await expect(client.lock()).resolves.toBeUndefined();
    await expect(client.unlock("correct horse")).resolves.toBeUndefined();
    expect(calls).toEqual(["lock", "unlock correct horse"]);
  });

  it("backupExport and backupImport forward path and kit and return the result", async () => {
    const backupExport = vi.fn<NonNullable<Transport["backupExport"]>>(() => Promise.resolve(backup));
    const backupImport = vi.fn<NonNullable<Transport["backupImport"]>>(() => Promise.resolve(backup));
    const { client } = stubTransport({ type: "Ok" }, { backupExport, backupImport });
    await expect(client.backupExport()).resolves.toEqual(backup);
    await expect(client.backupExport("D:/a.atomikkb")).resolves.toEqual(backup);
    await expect(client.backupImport()).resolves.toEqual(backup);
    await expect(client.backupImport("D:/a.atomikkb", "abandon ability")).resolves.toEqual(backup);
    expect(backupExport.mock.calls).toEqual([[undefined], ["D:/a.atomikkb"]]);
    expect(backupImport.mock.calls).toEqual([
      [undefined, undefined],
      ["D:/a.atomikkb", "abandon ability"],
    ]);
  });

  it("passes the transport's failures through", async () => {
    const cancelled = new CoreError({ code: "InvalidInput", message: "cancelled", details: { field: "path" } });
    const { client } = stubTransport(
      { type: "Ok" },
      {
        lock: () => Promise.reject(cancelled),
        unlock: () => Promise.reject(cancelled),
        backupExport: () => Promise.reject(cancelled),
        backupImport: () => Promise.reject(cancelled),
      },
    );
    await expect(client.lock()).rejects.toBe(cancelled);
    await expect(client.unlock("x")).rejects.toBe(cancelled);
    await expect(client.backupExport()).rejects.toBe(cancelled);
    await expect(client.backupImport()).rejects.toBe(cancelled);
  });

  it("rejects with Internal when the transport cannot lock or back up", async () => {
    const { client } = stubTransport({ type: "Ok" });
    const calls: [string, () => Promise<unknown>][] = [
      ["lock", () => client.lock()],
      ["unlock", () => client.unlock("correct horse")],
      ["backupExport", () => client.backupExport()],
      ["backupImport", () => client.backupImport(undefined, "kit")],
    ];
    for (const [name, call] of calls) {
      const error: unknown = await call().then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error, name).toBeInstanceOf(CoreError);
      expect((error as CoreError).toApiError(), name).toEqual({
        code: "Internal",
        message: `${name} is not supported by this transport`,
      });
    }
  });
});
