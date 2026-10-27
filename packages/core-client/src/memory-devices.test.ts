// The devices side of `MemoryTransport` (Milestone 7): pairing in both roles, revocation with
// the new kit, pending rotations, the device log seeds and `receiveDeviceLog`, the recovery with
// the kit, and the recovery blob published by sync.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { EXPIRED_PAIRING_CODE, MEMORY_JOINER_NAME, MEMORY_PAIRING_CODE, MEMORY_PAIRING_SAS } from "./devices-mirror";
import { CoreError } from "./errors";
import type { DeviceEntry } from "./generated/DeviceEntry";
import type { Event } from "./generated/Event";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";

const START = 1_700_000_000_000;
const STEP = 150;
const URL_A = "https://sync.example.com";
const PASSPHRASE = "correct horse";
/** The key 00 01 … 1f, for kits a test has to type. */
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);

const PAIRING: Event = { type: "PairingChanged" };
const PAIRING_REQUESTED: Event = { type: "PairingRequested" };
const RECOVERY: Event = { type: "RecoveryChanged" };
const RECOVERY_REQUESTED: Event = { type: "RecoveryRequested" };
const SECURITY: Event = { type: "SecurityChanged" };
const SYNC_CHANGED: Event = { type: "SyncChanged" };
const SYNC_REQUESTED: Event = { type: "SyncRequested" };
const LOCKED = { code: "Locked", message: "the workspace is locked" };
const WRONG_PASSPHRASE = { code: "InvalidInput", message: "wrong passphrase", details: { field: "passphrase" } };

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
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  /** The events so far, forgotten afterwards. */
  const take = (): Event[] => events.splice(0);
  return { transport, client, events, take };
}

/** An existing device of an account: workspace, kit (of `KEY`) and sync on. */
function sponsor(options: MemoryTransportOptions = {}) {
  return setup({
    workspaceName: "Home",
    security: { kitCreatedAt: START - 10_000, kitKey: KEY },
    sync: { serverUrl: URL_A },
    ...options,
  });
}

/** A device of an account with `devices` devices in the log (lock off unless `security` says so). */
function account(devices: number, options: MemoryTransportOptions = {}) {
  return setup({
    workspaceName: "Home",
    security: { kitCreatedAt: START - 10_000, kitKey: KEY },
    devices: { devices },
    ...options,
  });
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

function named(devices: readonly DeviceEntry[], name: string): DeviceEntry {
  const device = devices.find((entry) => entry.name === name);
  if (device === undefined) {
    throw new Error(`no device named ${name}`);
  }
  return device;
}

describe("pairing as the joiner", () => {
  it("goes Starting, Waiting with the code, Confirming with the digits, then installs the account", async () => {
    const { client, take } = setup();
    await expect(client.pairingStatus()).resolves.toEqual({ role: "Joiner", state: "Idle" });
    await expect(client.pairingStart("Joiner", { serverUrl: URL_A })).resolves.toEqual({
      role: "Joiner",
      state: "Starting",
      serverUrl: URL_A,
    });
    expect(take()).toEqual([PAIRING, PAIRING_REQUESTED]);

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([PAIRING]);
    await expect(client.pairingStatus()).resolves.toEqual({
      role: "Joiner",
      state: "Waiting",
      serverUrl: URL_A,
      code: MEMORY_PAIRING_CODE,
      expiresAt: START + STEP + 600_000,
    });

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([PAIRING]);
    await expect(client.pairingStatus()).resolves.toMatchObject({ state: "Confirming", sas: MEMORY_PAIRING_SAS });
    // The user compares the digits: nothing moves meanwhile.
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);

    await expect(client.pairingConfirm()).resolves.toMatchObject({ state: "PeerConfirming" });
    expect(take()).toEqual([PAIRING]);
    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([PAIRING]);
    expect((await client.pairingStatus()).state).toBe("Transferring");
    await expect(client.workspaceGet()).resolves.toBeNull();

    vi.advanceTimersByTime(STEP);
    const workspace = await client.workspaceGet();
    expect(workspace).toMatchObject({ name: "Paired workspace", createdAt: START + 14 * STEP });
    expect(take()).toEqual([
      { type: "WorkspaceCreated", data: { id: workspace?.id } },
      SECURITY,
      SYNC_CHANGED,
      SYNC_REQUESTED,
      PAIRING,
    ]);
    await expect(client.pairingStatus()).resolves.toMatchObject({ state: "Done", code: MEMORY_PAIRING_CODE });

    const devices = await client.deviceList();
    expect(devices).toHaveLength(2);
    const [laptop, me] = devices;
    expect(laptop).toMatchObject({ name: "Laptop", isThisDevice: false, addedViaKit: false });
    expect(laptop).not.toHaveProperty("addedBy");
    expect(me).toMatchObject({ isThisDevice: true, addedViaKit: false, addedBy: laptop?.deviceId });
    await expect(client.securityStatus()).resolves.toMatchObject({
      accountId: expect.any(String) as string,
      recoveryKit: { createdAt: START + 14 * STEP, stale: false },
      devices: 2,
      rootGeneration: 0,
      rotationPending: false,
      thisDeviceRevoked: false,
      recoveryPublished: false,
    });
    await expect(client.syncStatus()).resolves.toMatchObject({ configured: true, serverUrl: URL_A, state: "Idle" });
  });

  it("needs a server address and a fresh installation", async () => {
    const fresh = setup();
    expect((await failure(fresh.client.pairingStart("Joiner"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the server address must be http(s)://host[:port]",
      details: { field: "serverUrl" },
    });
    const existing = setup({ workspaceName: "Home" });
    expect((await failure(existing.client.pairingStart("Joiner", { serverUrl: URL_A }))).toApiError()).toEqual({
      code: "AlreadyExists",
      message: "this installation already has a workspace; joining an account needs a fresh one",
    });
    expect(fresh.events).toEqual([]);
    expect(existing.events).toEqual([]);
  });

  it("confirming outside Confirming is InvalidInput for pairing", async () => {
    const { client } = setup();
    const NOTHING = { code: "InvalidInput", message: "there is nothing to confirm", details: { field: "pairing" } };
    expect((await failure(client.pairingConfirm())).toApiError()).toEqual(NOTHING);
    await client.pairingStart("Joiner", { serverUrl: URL_A });
    vi.advanceTimersByTime(STEP); // Waiting
    expect((await failure(client.pairingConfirm())).toApiError()).toEqual(NOTHING);
  });

  it("cancelling ends the session Cancelled and stops it; a new start replaces a session", async () => {
    const { client, take } = setup();
    await client.pairingStart("Joiner", { serverUrl: URL_A });
    vi.advanceTimersByTime(STEP); // Waiting
    take();
    await expect(client.pairingCancel()).resolves.toMatchObject({ state: "Cancelled", code: MEMORY_PAIRING_CODE });
    expect(take()).toEqual([PAIRING]);
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
    await expect(client.pairingCancel()).resolves.toMatchObject({ state: "Cancelled" });
    expect(take()).toEqual([]);

    await client.pairingStart("Joiner", { serverUrl: "http://localhost:8787" });
    vi.advanceTimersByTime(STEP);
    await client.pairingStart("Joiner", { serverUrl: URL_A });
    await expect(client.pairingStatus()).resolves.toEqual({ role: "Joiner", state: "Starting", serverUrl: URL_A });
    vi.advanceTimersByTime(STEP);
    expect((await client.pairingStatus()).state).toBe("Waiting");
  });

  it("fails when a workspace appeared before the bundle arrived", async () => {
    const { client } = setup();
    await client.pairingStart("Joiner", { serverUrl: URL_A });
    vi.advanceTimersByTime(2 * STEP);
    await client.pairingConfirm();
    await client.workspaceCreate("Mine");
    vi.advanceTimersByTime(2 * STEP);
    await expect(client.pairingStatus()).resolves.toMatchObject({
      state: "Failed",
      error: "this installation already has a workspace",
    });
    await expect(client.workspaceGet()).resolves.toMatchObject({ name: "Mine" });
  });

  it("pairingStepMs sets the length of a step", async () => {
    const { client } = setup({ pairingStepMs: 1000 });
    await client.pairingStart("Joiner", { serverUrl: URL_A });
    vi.advanceTimersByTime(999);
    expect((await client.pairingStatus()).state).toBe("Starting");
    vi.advanceTimersByTime(1);
    expect((await client.pairingStatus()).state).toBe("Waiting");
  });
});

describe("pairing as the sponsor", () => {
  it("shows the digits, then the joiner's name, and adds the joiner once confirmed", async () => {
    const { client, take } = sponsor();
    await expect(client.pairingStart("Sponsor", { code: "abcd-efgh-jkmn" })).resolves.toEqual({
      role: "Sponsor",
      state: "Starting",
      serverUrl: URL_A,
      code: "ABCD EFGH JKMN",
    });
    expect(take()).toEqual([PAIRING, PAIRING_REQUESTED]);

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([PAIRING]);
    const confirming = await client.pairingStatus();
    expect(confirming).toEqual({
      role: "Sponsor",
      state: "Confirming",
      serverUrl: URL_A,
      code: "ABCD EFGH JKMN",
      sas: MEMORY_PAIRING_SAS,
      expiresAt: START + STEP + 600_000,
    });
    // The joiner confirms first.
    expect((await failure(client.pairingConfirm())).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the other device has not confirmed yet",
      details: { field: "pairing" },
    });

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([PAIRING]);
    await expect(client.pairingStatus()).resolves.toMatchObject({ state: "Confirming", peerName: MEMORY_JOINER_NAME });
    // Confirming appends the joiner to the device log, before the bundle goes.
    await expect(client.pairingConfirm()).resolves.toMatchObject({ state: "Transferring" });
    const devices = await client.deviceList();
    const me = devices.find((device) => device.isThisDevice);
    const joiner = named(devices, MEMORY_JOINER_NAME);
    expect(joiner).toMatchObject({
      addedAt: START + 2 * STEP,
      addedViaKit: false,
      addedBy: me?.deviceId,
      isThisDevice: false,
    });
    expect(take()).toEqual([
      SECURITY,
      { type: "DevicesChanged", data: { added: [joiner], revoked: [], thisDeviceRevoked: false, rotated: false } },
      PAIRING,
    ]);
    expect((await client.securityStatus()).devices).toBe(2);

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([PAIRING]);
    await expect(client.pairingStatus()).resolves.toMatchObject({ state: "Done", peerName: MEMORY_JOINER_NAME });
    expect(await client.deviceList()).toHaveLength(2);
  });

  it("ends Failed with \"the code expired\" for a code the server no longer knows", async () => {
    const { client, take } = sponsor();
    await client.pairingStart("Sponsor", { code: EXPIRED_PAIRING_CODE });
    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([PAIRING, PAIRING_REQUESTED, PAIRING]);
    await expect(client.pairingStatus()).resolves.toEqual({
      role: "Sponsor",
      state: "Failed",
      serverUrl: URL_A,
      code: EXPIRED_PAIRING_CODE,
      error: "the code expired",
    });
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
  });

  it("needs a well-formed code, the recovery kit and sync on", async () => {
    const BAD_CODE = { code: "InvalidInput", message: "the code is 12 letters and digits", details: { field: "code" } };
    const ready = sponsor();
    expect((await failure(ready.client.pairingStart("Sponsor"))).toApiError()).toEqual(BAD_CODE);
    const raw = (code: string) => ready.transport.dispatch({ type: "PairingStart", data: { role: "Sponsor", code } });
    expect((await failure(raw("ABCD EFGH JKM"))).toApiError()).toEqual(BAD_CODE);
    expect(ready.events).toEqual([]);

    const noKit = setup({ workspaceName: "Home", sync: { serverUrl: URL_A } });
    expect((await failure(noKit.client.pairingStart("Sponsor", { code: MEMORY_PAIRING_CODE }))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "create your recovery kit before adding a device",
      details: { field: "kit" },
    });
    const noSync = setup({ workspaceName: "Home", security: { kitCreatedAt: START } });
    expect((await failure(noSync.client.pairingStart("Sponsor", { code: MEMORY_PAIRING_CODE }))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "turn sync on before adding a device",
      details: { field: "serverUrl" },
    });
  });

  it("a lock cancels the session silently; while locked the pairing requests reject", async () => {
    const { client, transport, take } = sponsor({
      security: { kitCreatedAt: START - 10_000, appLockPassphrase: PASSPHRASE },
    });
    await client.pairingStart("Sponsor", { code: MEMORY_PAIRING_CODE });
    vi.advanceTimersByTime(STEP);
    take();
    await transport.lock();
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
    for (const call of [() => client.pairingStatus(), () => client.pairingConfirm(), () => client.pairingCancel()]) {
      expect((await failure(call())).toApiError()).toEqual(LOCKED);
    }
    expect((await failure(client.pairingStart("Sponsor", { code: MEMORY_PAIRING_CODE }))).toApiError()).toEqual(LOCKED);
    await transport.unlock(PASSPHRASE);
    await expect(client.pairingStatus()).resolves.toMatchObject({ state: "Cancelled", sas: MEMORY_PAIRING_SAS });
  });
});

describe("DeviceRevoke", () => {
  it("revokes another device, rotates the root key and answers a new kit that replaces the old one", async () => {
    const { client, take } = account(3);
    const before = await client.deviceList();
    expect(before.map((device) => device.name)).toEqual(["memory device", "Laptop", "Work desktop"]);
    const me = before[0];
    const laptop = named(before, "Laptop");
    expect(laptop).toMatchObject({ addedBy: me?.deviceId, addedAt: START - 10_000 });
    const accountId = (await client.securityStatus()).accountId;

    const kit = await client.deviceRevoke(laptop.deviceId);
    expect(kit.words).toHaveLength(24);
    expect(kit).toMatchObject({ accountId, createdAt: START });
    expect(take()).toEqual([
      SECURITY,
      { type: "DevicesChanged", data: { added: [], revoked: [laptop.deviceId], thisDeviceRevoked: false, rotated: true } },
    ]);
    expect(named(await client.deviceList(), "Laptop")).toMatchObject({ revokedAt: START, revokedBy: me?.deviceId });
    await expect(client.securityStatus()).resolves.toMatchObject({
      devices: 3,
      rootGeneration: 1,
      recoveryKit: { createdAt: START - 10_000, reissuedAt: START, stale: false },
    });
    // The old kit is void; the new one verifies.
    const oldKit = (await kitWordsFromEntropy(KEY)).join(" ");
    expect((await failure(client.recoveryKitVerify(oldKit))).message).toBe("the kit does not match this workspace");
    await expect(client.recoveryKitVerify(kit.words.join(" "))).resolves.toBeUndefined();
  });

  it("refuses this device, unknown and revoked devices for the field deviceId, changing nothing", async () => {
    const { client, take } = account(2);
    const [me, laptop] = await client.deviceList();
    const refusal = async (deviceId: string) => (await failure(client.deviceRevoke(deviceId))).toApiError();
    expect(await refusal(me?.deviceId ?? "")).toEqual({
      code: "InvalidInput",
      message: "this device cannot remove itself; remove it from another device",
      details: { field: "deviceId" },
    });
    const UNKNOWN = { code: "InvalidInput", message: "no active device of this account has this id", details: { field: "deviceId" } };
    expect(await refusal("99999999-9999-4999-8999-999999999999")).toEqual(UNKNOWN);
    expect(take()).toEqual([]);
    await client.deviceRevoke(laptop?.deviceId ?? "");
    take();
    expect(await refusal(laptop?.deviceId ?? "")).toEqual(UNKNOWN);
    expect(take()).toEqual([]);
    expect((await client.securityStatus()).rootGeneration).toBe(1);
  });

  it("wants the app-lock passphrase while the lock is on", async () => {
    const { client } = account(2, {
      security: { kitCreatedAt: START - 10_000, kitKey: KEY, appLockPassphrase: PASSPHRASE },
    });
    const laptop = named(await client.deviceList(), "Laptop");
    expect((await failure(client.deviceRevoke(laptop.deviceId))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the app-lock passphrase is required to remove a device",
      details: { field: "passphrase" },
    });
    expect((await failure(client.deviceRevoke(laptop.deviceId, "wrong horse"))).toApiError()).toEqual(WRONG_PASSPHRASE);
    expect((await client.securityStatus()).rootGeneration).toBe(0);
    await expect(client.deviceRevoke(laptop.deviceId, PASSPHRASE)).resolves.toMatchObject({ createdAt: START });
  });

  it("a revoked device cannot revoke", async () => {
    const { client } = account(3, { devices: { devices: 3, thisDeviceRevoked: true } });
    const desktop = named(await client.deviceList(), "Work desktop");
    expect((await failure(client.deviceRevoke(desktop.deviceId))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "this device was removed from the account",
      details: { field: "deviceId" },
    });
  });

  it("while locked it rejects with Locked", async () => {
    const { client, transport } = account(2, {
      security: { kitCreatedAt: START - 10_000, appLockPassphrase: PASSPHRASE },
    });
    const laptop = named(await client.deviceList(), "Laptop");
    await transport.lock();
    expect((await failure(client.deviceRevoke(laptop.deviceId, PASSPHRASE))).toApiError()).toEqual(LOCKED);
    expect((await failure(client.rotationApply(PASSPHRASE))).toApiError()).toEqual(LOCKED);
  });

  it("backups carry the generation and who added and revoked each device", async () => {
    const files = new Map<string, Uint8Array>();
    const { client } = account(2, { files });
    const [me, laptop] = await client.deviceList();
    const kit = await client.deviceRevoke(laptop?.deviceId ?? "");
    await client.backupExport("/backups/revoked.atomikkb");
    const restored = setup({ files });
    await restored.client.backupImport("/backups/revoked.atomikkb", kit.words.join(" "));
    await expect(restored.client.securityStatus()).resolves.toMatchObject({
      rootGeneration: 1,
      recoveryKit: { stale: true },
    });
    expect(named(await restored.client.deviceList(), "Laptop")).toMatchObject({
      addedBy: me?.deviceId,
      revokedBy: me?.deviceId,
      revokedAt: START,
    });
  });
});

describe("RotationApply", () => {
  it("finishes a pending rotation with the passphrase while the app lock is on", async () => {
    const { client, take } = setup({
      workspaceName: "Home",
      security: { kitCreatedAt: START, appLockPassphrase: PASSPHRASE, rotationPending: true },
    });
    await expect(client.securityStatus()).resolves.toMatchObject({ rotationPending: true, rootGeneration: 0 });
    expect((await failure(client.rotationApply())).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the app-lock passphrase is required to apply the new keys",
      details: { field: "passphrase" },
    });
    expect((await failure(client.rotationApply("wrong horse"))).toApiError()).toEqual(WRONG_PASSPHRASE);
    expect(take()).toEqual([]);
    await expect(client.rotationApply(PASSPHRASE)).resolves.toMatchObject({ rotationPending: false, rootGeneration: 1 });
    expect(take()).toEqual([SECURITY]);
    // Nothing pending any more: the status, unchanged, without checks or events.
    await expect(client.rotationApply()).resolves.toMatchObject({ rotationPending: false, rootGeneration: 1 });
    expect(take()).toEqual([]);
  });

  it("devices.rotationPending seeds the same; without the lock no passphrase is needed", async () => {
    const { client } = account(2, { devices: { devices: 2, rotationPending: true } });
    await expect(client.securityStatus()).resolves.toMatchObject({ rotationPending: true });
    await expect(client.rotationApply()).resolves.toMatchObject({ rotationPending: false, rootGeneration: 1 });
  });
});

describe("device log seeds and receiveDeviceLog", () => {
  it("devices names the others and needs a kit and a whole number", async () => {
    const { client } = account(6);
    expect((await client.deviceList()).map((device) => device.name)).toEqual([
      "memory device",
      "Laptop",
      "Work desktop",
      "Phone",
      "Tablet",
      "Device 6",
    ]);
    expect(() => new MemoryTransport({ devices: { devices: 2 } })).toThrow(/need security\.kitCreatedAt/u);
    expect(() => new MemoryTransport({ security: { rotationPending: true } })).toThrow(/need security\.kitCreatedAt/u);
    expect(() => account(0)).toThrow(/whole number of at least 1/u);
  });

  it("thisDeviceRevoked: revoked by the first other device, sync forgotten", async () => {
    const { client } = setup({
      workspaceName: "Home",
      security: { kitCreatedAt: START - 10_000 },
      devices: { thisDeviceRevoked: true },
      sync: { state: "idle" },
    });
    const [me, laptop] = await client.deviceList();
    expect(me).toMatchObject({ isThisDevice: true, revokedAt: START, revokedBy: laptop?.deviceId });
    await expect(client.securityStatus()).resolves.toMatchObject({ devices: 2, thisDeviceRevoked: true });
    await expect(client.syncStatus()).resolves.toMatchObject({ configured: false, state: "Disabled" });
  });

  it("applies devices added and revoked elsewhere and a rotation, with SecurityChanged and DevicesChanged", async () => {
    const { client, transport, take } = account(3);
    const [, laptop, desktop] = await client.deviceList();
    const data = transport.receiveDeviceLog({ added: ["Phone"], revoked: [desktop?.deviceId ?? ""], rotate: true });
    const phone = named(await client.deviceList(), "Phone");
    expect(phone).toMatchObject({ addedAt: START, addedBy: laptop?.deviceId, isThisDevice: false });
    expect(data).toEqual({ added: [phone], revoked: [desktop?.deviceId], thisDeviceRevoked: false, rotated: true });
    expect(take()).toEqual([SECURITY, { type: "DevicesChanged", data }]);
    expect(named(await client.deviceList(), "Work desktop")).toMatchObject({ revokedAt: START, revokedBy: laptop?.deviceId });
    await expect(client.securityStatus()).resolves.toMatchObject({ devices: 4, rootGeneration: 1, rotationPending: false });
  });

  it("a rotation waits for RotationApply while the app lock is on", async () => {
    const { client, transport } = account(2, {
      security: { kitCreatedAt: START - 10_000, appLockPassphrase: PASSPHRASE },
    });
    transport.receiveDeviceLog({ rotate: true });
    transport.receiveDeviceLog({ rotate: true });
    await expect(client.securityStatus()).resolves.toMatchObject({ rootGeneration: 0, rotationPending: true });
    await expect(client.rotationApply(PASSPHRASE)).resolves.toMatchObject({ rootGeneration: 2, rotationPending: false });
  });

  it("this device revoked elsewhere: thisDeviceRevoked, no rotation, sync forgotten", async () => {
    const { client, transport, take } = account(2, { sync: { serverUrl: URL_A } });
    const data = transport.receiveDeviceLog({ revokeThisDevice: true, rotate: true });
    expect(data).toEqual({ added: [], revoked: [], thisDeviceRevoked: true, rotated: true });
    expect(take()).toEqual([SECURITY, { type: "DevicesChanged", data }, SYNC_CHANGED]);
    await expect(client.securityStatus()).resolves.toMatchObject({ thisDeviceRevoked: true, rootGeneration: 0 });
    await expect(client.syncStatus()).resolves.toMatchObject({ configured: false });
  });

  it("refuses misuse as Internal: no kit, or a device that cannot be revoked", async () => {
    const { transport } = setup({ workspaceName: "Home" });
    expect(() => transport.receiveDeviceLog({ rotate: true })).toThrow(/only for an account/u);
    const withLog = account(2);
    const [me] = await withLog.client.deviceList();
    expect(() => withLog.transport.receiveDeviceLog({ revoked: [me?.deviceId ?? ""] })).toThrow(/not another active device/u);
    expect(withLog.events).toEqual([]);
  });
});

describe("recovery with the kit", () => {
  it("goes Fetching, Joining, Done and installs the account, this device added via the kit", async () => {
    const { client, take } = setup();
    const words = (await kitWordsFromEntropy(KEY)).join(" ");
    await expect(client.recoveryStatus()).resolves.toEqual({ state: "Idle" });
    await expect(client.recoveryStart(URL_A, words)).resolves.toEqual({ state: "Fetching", serverUrl: URL_A });
    expect(take()).toEqual([RECOVERY, RECOVERY_REQUESTED]);

    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([RECOVERY]);
    await expect(client.recoveryStatus()).resolves.toEqual({ state: "Joining", serverUrl: URL_A });

    vi.advanceTimersByTime(STEP);
    const workspace = await client.workspaceGet();
    expect(workspace).toMatchObject({ name: "Recovered workspace" });
    expect(take()).toEqual([
      { type: "WorkspaceCreated", data: { id: workspace?.id } },
      SECURITY,
      SYNC_CHANGED,
      SYNC_REQUESTED,
      RECOVERY,
    ]);
    await expect(client.recoveryStatus()).resolves.toEqual({ state: "Done", serverUrl: URL_A });
    const [laptop, me] = await client.deviceList();
    expect(laptop).toMatchObject({ name: "Laptop", addedViaKit: false, isThisDevice: false });
    expect(me).toMatchObject({ isThisDevice: true, addedViaKit: true, addedAt: START + 2 * STEP });
    expect(me).not.toHaveProperty("addedBy");
    // The kit that opened the account is still its kit, but stale until re-issued.
    await expect(client.recoveryKitVerify(words)).resolves.toBeUndefined();
    expect((await client.securityStatus()).recoveryKit).toMatchObject({ stale: true });
    await expect(client.syncStatus()).resolves.toMatchObject({ configured: true, serverUrl: URL_A });
    await client.recoveryKitReissue();
    expect((await client.securityStatus()).recoveryKit).toMatchObject({ stale: false, reissuedAt: START + 2 * STEP });
  });

  it("a kit the server does not know ends Failed after the fetch", async () => {
    const { client, take } = setup();
    await client.recoveryStart(URL_A, "some wrong kit");
    vi.advanceTimersByTime(STEP);
    expect(take()).toEqual([RECOVERY, RECOVERY_REQUESTED, RECOVERY]);
    await expect(client.recoveryStatus()).resolves.toEqual({
      state: "Failed",
      serverUrl: URL_A,
      error: "this server does not know that kit",
    });
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
    await expect(client.workspaceGet()).resolves.toBeNull();
  });

  it("checks the address and the kit, and wants a fresh installation", async () => {
    const fresh = setup();
    const raw = (serverUrl: string, kit: string) =>
      fresh.transport.dispatch({ type: "RecoveryStart", data: { serverUrl, kit } });
    expect((await failure(raw("sync.example.com", "abandon"))).toApiError()).toMatchObject({
      code: "InvalidInput",
      details: { field: "serverUrl" },
    });
    expect((await failure(raw(URL_A, "abandon ability able"))).toApiError()).toMatchObject({
      code: "InvalidInput",
      details: { field: "kit" },
    });
    expect(fresh.events).toEqual([]);
    const existing = setup({ workspaceName: "Home" });
    const words = (await kitWordsFromEntropy(KEY)).join(" ");
    expect((await failure(existing.client.recoveryStart(URL_A, words))).code).toBe("AlreadyExists");
  });

  it("cancelling drops the session; nothing moves afterwards", async () => {
    const { client, take } = setup();
    await expect(client.recoveryCancel()).resolves.toEqual({ state: "Idle" });
    expect(take()).toEqual([]);
    await client.recoveryStart(URL_A, (await kitWordsFromEntropy(KEY)).join(" "));
    take();
    await expect(client.recoveryCancel()).resolves.toEqual({ state: "Idle" });
    expect(take()).toEqual([RECOVERY]);
    vi.advanceTimersByTime(10 * STEP);
    expect(take()).toEqual([]);
    await expect(client.workspaceGet()).resolves.toBeNull();
  });
});

describe("the recovery blob on the sync server", () => {
  it("the first healthy push after a kit was issued publishes it; a new kit unpublishes it", async () => {
    const { client, take } = sponsor();
    expect((await client.securityStatus()).recoveryPublished).toBe(false);
    await client.syncNow();
    vi.advanceTimersByTime(2 * STEP); // Pushing, then Pulling
    expect(take()).toEqual([SYNC_REQUESTED, SYNC_CHANGED, SYNC_CHANGED, SECURITY]);
    expect((await client.securityStatus()).recoveryPublished).toBe(true);
    vi.advanceTimersByTime(STEP);
    await client.syncNow();
    vi.advanceTimersByTime(3 * STEP);
    expect(take().filter((event) => event.type === "SecurityChanged")).toEqual([]);
    await client.recoveryKitReissue();
    expect((await client.securityStatus()).recoveryPublished).toBe(false);
  });
});
