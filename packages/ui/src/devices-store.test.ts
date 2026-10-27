import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { selectKitToReissue } from "./components/DeviceBanners";
import { createAppStore } from "./store";
import type { AppState } from "./store";
import { pairingUnderWay, recoveryUnderWay } from "./store-devices";

const NOW = Date.UTC(2026, 9, 5, 12, 0, 0);
const PASSPHRASE = "correct horse battery";
const SERVER = "https://sync.example.com";
const KIT = `${"abandon ".repeat(23)}art`;
const ACCOUNT: MemoryTransportOptions = {
  workspaceName: "Research",
  security: { kitCreatedAt: NOW },
  sync: { state: "idle", serverUrl: SERVER },
};

/** The memory core (fast fake tasks) behind a recording transport, and a started store over it. */
function setup(options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({ now: () => NOW, pairingStepMs: 5, syncStepMs: 5, ...options });
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

describe("devices: boot", () => {
  it("reads the pairing and recovery status on a fresh installation", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "onboarding");
    expect(ctx.types()).toEqual(expect.arrayContaining(["PairingStatus", "RecoveryStatus"]));
    expect(ctx.store.getState().pairing).toMatchObject({ state: "Idle" });
    expect(ctx.store.getState().recovery).toMatchObject({ state: "Idle" });
  });

  it("does not read them with a workspace", async () => {
    const ctx = setup({ workspaceName: "Research" });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.types()).not.toContain("PairingStatus");
    expect(ctx.types()).not.toContain("RecoveryStatus");
    expect(ctx.store.getState()).toMatchObject({ pairing: null, recovery: null, devicesNotice: null });
  });
});

describe("devices: the session helpers", () => {
  it("tell a session under way from one that ended", () => {
    for (const state of ["Starting", "Waiting", "Confirming", "PeerConfirming", "Transferring", "Done"] as const) {
      expect(pairingUnderWay({ role: "Joiner", state }, "Joiner"), state).not.toBeNull();
      expect(pairingUnderWay({ role: "Joiner", state }, "Sponsor"), state).toBeNull();
    }
    for (const state of ["Idle", "Cancelled", "Failed"] as const) {
      expect(pairingUnderWay({ role: "Joiner", state }, "Joiner"), state).toBeNull();
    }
    expect(pairingUnderWay(null, "Joiner")).toBeNull();
    for (const state of ["Fetching", "Joining", "Done"] as const) {
      expect(recoveryUnderWay({ state }), state).not.toBeNull();
    }
    expect(recoveryUnderWay({ state: "Idle" })).toBeNull();
    expect(recoveryUnderWay({ state: "Failed" })).toBeNull();
    expect(recoveryUnderWay(null)).toBeNull();
  });
});

describe("devices: pairing", () => {
  it("joiner: follows the session through PairingChanged and lands in the installed workspace", async () => {
    // Steps long enough for every state to be seen.
    const ctx = setup({ pairingStepMs: 150 });
    await settled(ctx, (s) => s.phase === "onboarding");
    const started = await ctx.store.getState().startPairing("Joiner", { serverUrl: SERVER });
    expect(started).toMatchObject({ role: "Joiner", state: "Starting", serverUrl: SERVER });
    await settled(ctx, (s) => s.pairing?.state === "Waiting");
    expect(ctx.store.getState().pairing?.code).toMatch(/^[0-9A-Z]{4} [0-9A-Z]{4} [0-9A-Z]{4}$/);
    await settled(ctx, (s) => s.pairing?.state === "Confirming");
    expect(ctx.store.getState().pairing?.sas).toMatch(/^\d{6}$/);

    await ctx.store.getState().confirmPairing();
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().workspace).not.toBeNull();
    expect(ctx.store.getState().devices.length).toBeGreaterThan(1);
    expect(ctx.store.getState().devices.find((device) => device.isThisDevice)?.addedBy).toBeDefined();
    expect(ctx.requests).toContainEqual({ type: "PairingStart", data: { role: "Joiner", serverUrl: SERVER } });
  });

  it("joiner: a cancel ends the session", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "onboarding");
    await ctx.store.getState().startPairing("Joiner", { serverUrl: SERVER });
    await ctx.store.getState().cancelPairing();
    await settled(ctx, (s) => s.pairing?.state === "Cancelled");
    expect(ctx.store.getState().phase).toBe("onboarding");
  });

  it("sponsor: the code, the digits, the new device's name, then it is in the list", async () => {
    const ctx = setup(ACCOUNT);
    await settled(ctx, (s) => s.phase === "ready");
    await ctx.store.getState().startPairing("Sponsor", { code: "abcd-efgh jkmn" });
    expect(ctx.requests).toContainEqual({ type: "PairingStart", data: { role: "Sponsor", code: "ABCDEFGHJKMN" } });
    await settled(ctx, (s) => s.pairing?.state === "Confirming" && s.pairing.peerName !== undefined);
    await ctx.store.getState().confirmPairing();
    await settled(ctx, (s) => s.pairing?.state === "Done");
    await settled(ctx, (s) => s.devices.some((device) => device.name === s.pairing?.peerName));
  });

  it("rejects what the core refuses", async () => {
    const ctx = setup(ACCOUNT);
    await settled(ctx, (s) => s.phase === "ready");
    await expect(ctx.store.getState().startPairing("Sponsor", { code: "short" })).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "code" },
    });
    await expect(ctx.store.getState().startPairing("Joiner", { serverUrl: SERVER })).rejects.toMatchObject({
      code: "AlreadyExists",
    });
  });
});

describe("devices: recovery", () => {
  it("recovers the account with the kit, and the kit then asks for a re-issue", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "onboarding");
    const started = await ctx.store.getState().startRecovery(SERVER, KIT);
    expect(started).toMatchObject({ state: "Fetching", serverUrl: SERVER });
    await settled(ctx, (s) => s.phase === "ready" && s.security !== null);
    await settled(ctx, (s) => s.devices.some((device) => device.isThisDevice));
    expect(ctx.store.getState().devices.find((device) => device.isThisDevice)?.addedViaKit).toBe(true);
    expect(selectKitToReissue(ctx.store.getState())).toBe(true);

    await ctx.store.getState().reissueRecoveryKit();
    await settled(ctx, (s) => s.security?.recoveryKit?.reissuedAt !== undefined);
    expect(selectKitToReissue(ctx.store.getState())).toBe(false);
  });

  it("a kit the server does not know ends Failed with the reason; cancel goes back to idle", async () => {
    const ctx = setup();
    await settled(ctx, (s) => s.phase === "onboarding");
    await ctx.store.getState().startRecovery(SERVER, "wrong kit words");
    await settled(ctx, (s) => s.recovery?.state === "Failed");
    expect(ctx.store.getState().recovery?.error).toBeTruthy();
    expect(ctx.store.getState().phase).toBe("onboarding");

    await ctx.store.getState().startRecovery(SERVER, KIT);
    await ctx.store.getState().cancelRecovery();
    await settled(ctx, (s) => s.recovery?.state === "Idle");
  });
});

describe("devices: removing and rotations", () => {
  it("revokeDevice hands the new kit to the caller, keeps none of it, and the list shows the removal", async () => {
    const ctx = setup({ ...ACCOUNT, devices: { devices: 3 } });
    await settled(ctx, (s) => s.phase === "ready" && s.devices.length === 3);
    const laptop = ctx.store.getState().devices.find((device) => !device.isThisDevice);
    const kit = await ctx.store.getState().revokeDevice(laptop?.deviceId ?? "");
    expect(kit.words).toHaveLength(24);
    await settled(ctx, (s) => s.devices.find((device) => device.deviceId === laptop?.deviceId)?.revokedAt !== undefined);
    expect(ctx.store.getState().security?.rootGeneration).toBe(1);
    expect(JSON.stringify(ctx.store.getState())).not.toContain(kit.words.join(" "));
  });

  it("revokeDevice wants the passphrase while the app lock is on", async () => {
    const ctx = setup({ ...ACCOUNT, security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE }, devices: { devices: 2 } });
    await settled(ctx, (s) => s.phase === "ready" && s.devices.length === 2);
    const other = ctx.store.getState().devices.find((device) => !device.isThisDevice)?.deviceId ?? "";
    await expect(ctx.store.getState().revokeDevice(other)).rejects.toMatchObject({ details: { field: "passphrase" } });
    await expect(ctx.store.getState().revokeDevice(other, "not it at all")).rejects.toMatchObject({
      details: { field: "passphrase" },
    });
    const kit = await ctx.store.getState().revokeDevice(other, PASSPHRASE);
    expect(kit.words).toHaveLength(24);
  });

  it("applyRotation finishes a pending rotation with the passphrase", async () => {
    const ctx = setup({
      workspaceName: "Research",
      security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE },
      devices: { devices: 2, rotationPending: true },
    });
    await settled(ctx, (s) => s.phase === "ready");
    expect(ctx.store.getState().security?.rotationPending).toBe(true);
    await expect(ctx.store.getState().applyRotation("not it at all")).rejects.toMatchObject({
      details: { field: "passphrase" },
    });
    await ctx.store.getState().applyRotation(PASSPHRASE);
    expect(ctx.store.getState().security?.rotationPending).toBe(false);
  });
});

describe("devices: DevicesChanged", () => {
  it("keeps a notice of what the log brought, with the names known before, until dismissed", async () => {
    const ctx = setup({ ...ACCOUNT, devices: { devices: 3 } });
    await settled(ctx, (s) => s.phase === "ready" && s.devices.length === 3);
    const [, first] = ctx.store.getState().devices;
    ctx.memory.receiveDeviceLog({ added: ["Tablet"] });
    await settled(ctx, (s) => s.devicesNotice !== null);
    ctx.memory.receiveDeviceLog({ revoked: [first?.deviceId ?? ""], rotate: true });
    await settled(ctx, (s) => s.devicesNotice?.rotated === true);
    expect(ctx.store.getState().devicesNotice).toEqual({ added: ["Tablet"], removed: [first?.name], rotated: true });
    await settled(ctx, (s) => s.devices.some((device) => device.name === "Tablet"));

    ctx.store.getState().dismissDevicesNotice();
    expect(ctx.store.getState().devicesNotice).toBeNull();
  });

  it("this device removed: no notice, the status says so", async () => {
    const ctx = setup({ ...ACCOUNT, devices: { devices: 2 } });
    await settled(ctx, (s) => s.phase === "ready");
    ctx.memory.receiveDeviceLog({ revokeThisDevice: true });
    await settled(ctx, (s) => s.security?.thisDeviceRevoked === true);
    expect(ctx.store.getState().devicesNotice).toBeNull();
  });

  it("forgets the sessions and the notice while locked", async () => {
    const ctx = setup({ ...ACCOUNT, security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE }, devices: { devices: 2 } });
    await settled(ctx, (s) => s.phase === "ready");
    ctx.memory.receiveDeviceLog({ added: ["Tablet"] });
    await settled(ctx, (s) => s.devicesNotice !== null);
    await ctx.store.getState().lock();
    expect(ctx.store.getState()).toMatchObject({ pairing: null, recovery: null, devicesNotice: null });
  });
});
