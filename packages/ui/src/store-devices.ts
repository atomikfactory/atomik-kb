// The devices part of the app store (Milestone 7, ADR-0022): pairing (this device joining
// an account, or adding another device to it), recovery with the kit from the server,
// removing a device, finishing a key rotation made elsewhere, and the one-time notice of
// what changed in the device log. `store.ts` composes it with the rest.
//
// As in the other slices, reads are "latest request wins" and events refresh: the core
// emits `PairingChanged` and `RecoveryChanged` as the shell's network tasks move the
// sessions along, and `DevicesChanged` when the device log brings additions, removals or a
// rotation. Pairing and recovery happen on the onboarding screen too, before a workspace
// exists. Kits and passphrases never enter the store; the six digits and the pairing code
// do (they are shown on screen and expire with the session).

import { toCoreError } from "@atomik-kb/core-client";
import type {
  CoreClient,
  Event as CoreEvent,
  DeviceId,
  PairingInfo,
  PairingRole,
  PairingState,
  RecoveryInfo,
  RecoveryKit,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

/** What the device log brought since the user last looked (`DevicesChanged`), shown once. */
export interface DevicesNotice {
  /** Names of the devices that joined. */
  added: readonly string[];
  /** Names of the devices that were removed; `null` for one this device never knew by name. */
  removed: readonly (string | null)[];
  /** The keys were rotated (a device was removed somewhere). */
  rotated: boolean;
}

export interface DevicesData {
  /** The pairing session as the core reports it; `null` before it was read. */
  pairing: PairingInfo | null;
  /** The recovery through the kit; `null` before it was read. */
  recovery: RecoveryInfo | null;
  /** What changed in the device log, until dismissed. */
  devicesNotice: DevicesNotice | null;
}

export const EMPTY_DEVICES: DevicesData = { pairing: null, recovery: null, devicesNotice: null };

/** Pairing states after which the way in shows again. */
const ENDED_PAIRING: readonly PairingState[] = ["Idle", "Cancelled", "Failed"];

/**
 * The pairing session of this role when one is under way (its steps show instead of the way
 * in; a failed one shows its reason there), else `null`.
 */
export function pairingUnderWay(pairing: PairingInfo | null, role: PairingRole): PairingInfo | null {
  return pairing !== null && pairing.role === role && !ENDED_PAIRING.includes(pairing.state) ? pairing : null;
}

/** The recovery when one is under way, else `null`. */
export function recoveryUnderWay(recovery: RecoveryInfo | null): RecoveryInfo | null {
  return recovery !== null && recovery.state !== "Idle" && recovery.state !== "Failed" ? recovery : null;
}

/**
 * Every action rejects with a `CoreError` when the core refuses; what follows comes back
 * through `PairingChanged`, `RecoveryChanged` and `SecurityChanged` (the direct answers are
 * applied right away).
 */
export interface DevicesActions {
  refreshPairing: () => Promise<void>;
  /**
   * Starts a pairing: the joiner gives the server address (`InvalidInput` for `serverUrl`;
   * `AlreadyExists` when a workspace exists), the sponsor the code the new device shows
   * (`InvalidInput` for `code`, `NotFound` when the server does not know it).
   */
  startPairing: (role: PairingRole, options: { serverUrl?: string; code?: string }) => Promise<PairingInfo>;
  /** The user compared the six digits and they match. */
  confirmPairing: () => Promise<PairingInfo>;
  cancelPairing: () => Promise<PairingInfo>;
  refreshRecovery: () => Promise<void>;
  /** Recovers the account from the server with the kit (`InvalidInput` for `serverUrl` or `kit`). */
  startRecovery: (serverUrl: string, kit: string) => Promise<RecoveryInfo>;
  cancelRecovery: () => Promise<RecoveryInfo>;
  /**
   * Removes another device: the core rotates the keys and returns the new recovery kit,
   * which the caller shows once. The passphrase is needed while the app lock is on.
   */
  revokeDevice: (deviceId: DeviceId, passphrase?: string) => Promise<RecoveryKit>;
  /** Finishes a rotation another device made (the app lock's passphrase stores the new key). */
  applyRotation: (passphrase?: string) => Promise<void>;
  dismissDevicesNotice: () => void;
}

export type DevicesState = DevicesData & DevicesActions;

export interface DevicesSlice {
  initial: DevicesState;
  /**
   * Reads the pairing and recovery status for the boot sequence of an installation without
   * a workspace (a session may be under way). A failure other than `Locked` leaves them
   * unknown; returns nothing for what a newer read replaced meanwhile.
   */
  loadForOnboarding: () => Promise<Partial<DevicesData>>;
  onEvent: (event: CoreEvent) => void;
}

/** Adds what one `DevicesChanged` says to the notice still on screen. */
function mergeNotice(
  previous: DevicesNotice | null,
  added: readonly string[],
  removed: readonly (string | null)[],
  rotated: boolean,
): DevicesNotice | null {
  const merged: DevicesNotice = {
    added: [...(previous?.added ?? []), ...added],
    removed: [...(previous?.removed ?? []), ...removed],
    rotated: (previous?.rotated ?? false) || rotated,
  };
  return merged.added.length === 0 && merged.removed.length === 0 && !merged.rotated ? previous : merged;
}

export function createDevicesSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): DevicesSlice {
  // Bumped by every read and every action that answers a status; only the latest applies.
  let pairingTicket = 0;
  let recoveryTicket = 0;

  async function pairingCall(call: () => Promise<PairingInfo>): Promise<PairingInfo> {
    const mine = (pairingTicket += 1);
    const pairing = await call();
    if (mine === pairingTicket) {
      set({ pairing });
    }
    return pairing;
  }

  async function recoveryCall(call: () => Promise<RecoveryInfo>): Promise<RecoveryInfo> {
    const mine = (recoveryTicket += 1);
    const recovery = await call();
    if (mine === recoveryTicket) {
      set({ recovery });
    }
    return recovery;
  }

  const refreshPairing = async (): Promise<void> => {
    await pairingCall(() => client.pairingStatus());
  };

  const refreshRecovery = async (): Promise<void> => {
    await recoveryCall(() => client.recoveryStatus());
  };

  async function quietly<T>(read: () => Promise<T>, what: string): Promise<T | null> {
    try {
      return await read();
    } catch (error) {
      const failure = toCoreError(error);
      if (failure.code === "Locked") {
        throw failure;
      }
      console.error(`could not read the ${what} status`, failure.code);
      return null;
    }
  }

  async function loadForOnboarding(): Promise<Partial<DevicesData>> {
    const pairingMine = (pairingTicket += 1);
    const recoveryMine = (recoveryTicket += 1);
    const [pairing, recovery] = await Promise.all([
      quietly(() => client.pairingStatus(), "pairing"),
      quietly(() => client.recoveryStatus(), "recovery"),
    ]);
    return {
      ...(pairingMine === pairingTicket ? { pairing } : {}),
      ...(recoveryMine === recoveryTicket ? { recovery } : {}),
    };
  }

  const initial: DevicesState = {
    ...EMPTY_DEVICES,

    refreshPairing,

    startPairing: (role, options) => pairingCall(() => client.pairingStart(role, options)),

    confirmPairing: () => pairingCall(() => client.pairingConfirm()),

    cancelPairing: () => pairingCall(() => client.pairingCancel()),

    refreshRecovery,

    startRecovery: (serverUrl, kit) => recoveryCall(() => client.recoveryStart(serverUrl, kit)),

    cancelRecovery: () => recoveryCall(() => client.recoveryCancel()),

    revokeDevice: (deviceId, passphrase) => client.deviceRevoke(deviceId, passphrase),

    applyRotation: async (passphrase) => {
      const security = await client.rotationApply(passphrase);
      set({ security });
    },

    dismissDevicesNotice: () => {
      set({ devicesNotice: null });
    },
  };

  return {
    initial,
    loadForOnboarding,
    onEvent: (event) => {
      if (get().phase === "locked") {
        // The next boot reads what it needs.
        return;
      }
      switch (event.type) {
        case "PairingChanged":
          refreshQuietly(refreshPairing);
          break;
        case "RecoveryChanged":
          refreshQuietly(refreshRecovery);
          break;
        case "DevicesChanged": {
          const { added, revoked, thisDeviceRevoked, rotated } = event.data;
          // Names of removed devices come from the list as it was before this change.
          const known = new Map(get().devices.map((device) => [device.deviceId, device.name]));
          set((state) => ({
            devicesNotice: thisDeviceRevoked
              ? state.devicesNotice
              : mergeNotice(
                  state.devicesNotice,
                  added.map((device) => device.name),
                  revoked.map((deviceId) => known.get(deviceId) ?? null),
                  rotated,
                ),
          }));
          // The device list and the security status (revoked, rotation pending) follow.
          if (get().workspace !== null) {
            refreshQuietly(get().refreshSecurity);
          }
          break;
        }
        // The other events are not about devices.
      }
    },
  };
}
