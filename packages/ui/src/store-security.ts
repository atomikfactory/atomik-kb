// The security part of the app store (Milestone 4): the recovery kit's status, the app
// lock, the devices of the account and backups. `store.ts` composes it with the rest.
//
// As in the other slices, mutations only call the core, and `SecurityChanged` brings the
// new status and device list; reads are "latest request wins". Two things never enter the
// store: the recovery kit (`createRecoveryKit` and `reissueRecoveryKit` hand it to the
// component that shows it, which forgets it when it unmounts) and passphrases.

import { toCoreError } from "@atomik-kb/core-client";
import type {
  BackupResult,
  CoreClient,
  DeviceEntry,
  Event as CoreEvent,
  JsonValue,
  RecoveryKit,
  SecurityInfo,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

/**
 * Setting with the auto-lock delay in minutes; 0 (or unset) turns auto-lock off. ADR-0018
 * and the milestone spec call it `app.lock.autoMinutes`, but the core's setting keys are
 * lowercase (`[a-z0-9_.-]`, `kb-core` `validate::setting_key`), so it is spelled this way.
 */
export const AUTO_LOCK_KEY = "app.lock.auto_minutes";

/** The delays offered, in minutes. */
export const AUTO_LOCK_CHOICES: readonly number[] = [0, 1, 5, 15, 60];

/** Reads a stored auto-lock delay; anything that is not one of the choices means off. */
export function parseAutoLockMinutes(value: JsonValue | null | undefined): number {
  return typeof value === "number" && AUTO_LOCK_CHOICES.includes(value) ? value : 0;
}

export interface SecurityData {
  /**
   * Recovery kit, app lock, device and keyring status. `null` without a workspace, while
   * locked, and when the core could not report it.
   */
  security: SecurityInfo | null;
  /** The devices of the account as the device log lists them; empty before the kit exists. */
  devices: readonly DeviceEntry[];
}

export const EMPTY_SECURITY: SecurityData = { security: null, devices: [] };

/** What the shell can do beyond requests (see `Transport`). */
export interface Capabilities {
  /** Locking and unlocking the core: without it the app lock cannot be offered. */
  appLock: boolean;
  /** Backup files (native file dialogs): the desktop shell has them, the web shell not yet. */
  backups: boolean;
}

/**
 * Every action rejects with a `CoreError` when the core refuses; the status comes back
 * through `SecurityChanged`, as in the other slices.
 */
export interface SecurityActions {
  capabilities: Capabilities;
  /** Reads the status and the device list again (for example after a failed read). */
  refreshSecurity: () => Promise<void>;
  /** Creates the recovery kit and returns it to show once; `AlreadyExists` when there is one. */
  createRecoveryKit: () => Promise<RecoveryKit>;
  /** Checks a typed kit against the current one; `InvalidInput` when it does not match. */
  verifyRecoveryKit: (kit: string) => Promise<void>;
  /**
   * Replaces the kit (the core also rotates the root key) and returns the new one. The
   * app-lock passphrase is required while the lock is on.
   */
  reissueRecoveryKit: (passphrase?: string) => Promise<RecoveryKit>;
  enableAppLock: (passphrase: string) => Promise<void>;
  disableAppLock: (passphrase: string) => Promise<void>;
  changeAppLockPassphrase: (current: string, next: string) => Promise<void>;
  /**
   * Saves what the editor has not sent yet, locks the core and forgets everything the
   * workspace put in memory (spaces, tree, selection, open documents, search); the lock
   * screen shows next.
   */
  lock: () => Promise<void>;
  /** Unlocks with the passphrase and starts again like at launch; a wrong one is `InvalidInput`. */
  unlock: (passphrase: string) => Promise<void>;
  /** Writes a backup file (the shell asks where); `InvalidInput` for `path` when the user cancels. */
  exportBackup: () => Promise<BackupResult>;
  /**
   * Restores a backup into this installation, which has no workspace yet (the shell asks
   * for the file unless `path` is given), then starts again with the restored workspace.
   * Without `kit` the core answers `InvalidInput` for the field `kit` unless the backup
   * belongs to this installation's account.
   */
  importBackup: (kit?: string, path?: string) => Promise<BackupResult>;
}

export type SecurityState = SecurityData & SecurityActions;

/** What the slice needs from the store around it. */
export interface SecurityHooks {
  /**
   * Runs a core call that creates the workspace, so that the `WorkspaceCreated` it causes
   * is known to come from this UI (and not from another window).
   */
  ownWorkspace: <T>(work: () => Promise<T>) => Promise<T>;
  /** Forgets everything from the workspace and shows the lock screen. */
  locked: () => void;
}

export interface SecuritySlice {
  initial: SecurityState;
  /**
   * Reads the status and the devices for the boot sequence. A failure other than `Locked`
   * leaves them unknown instead of stopping the app: they matter for Settings, not for
   * opening the notes. Returns nothing when a newer read started meanwhile.
   */
  load: () => Promise<Partial<SecurityData>>;
  /** Reads the status and the devices again. */
  refresh: () => Promise<void>;
  onEvent: (event: CoreEvent) => void;
}

export function createSecuritySlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
  hooks: SecurityHooks,
): SecuritySlice {
  // Bumped by every read; only the answer to the latest one is applied.
  let ticket = 0;

  async function read(): Promise<SecurityData> {
    const [security, devices] = await Promise.all([client.securityStatus(), client.deviceList()]);
    return { security, devices };
  }

  async function load(): Promise<Partial<SecurityData>> {
    const mine = (ticket += 1);
    let data: SecurityData;
    try {
      data = await read();
    } catch (error) {
      const failure = toCoreError(error);
      if (failure.code === "Locked") {
        throw failure;
      }
      console.error("could not read the security status", failure.code);
      data = EMPTY_SECURITY;
    }
    return mine === ticket ? data : {};
  }

  async function refresh(): Promise<void> {
    const mine = (ticket += 1);
    const data = await read();
    if (mine === ticket) {
      set(data);
    }
  }

  const initial: SecurityState = {
    ...EMPTY_SECURITY,
    capabilities: { appLock: client.hasAppLock(), backups: client.hasBackups() },

    refreshSecurity: refresh,

    createRecoveryKit: () => client.recoveryKitCreate(),

    verifyRecoveryKit: async (kit) => {
      await client.recoveryKitVerify(kit);
    },

    reissueRecoveryKit: (passphrase) => client.recoveryKitReissue(passphrase),

    enableAppLock: async (passphrase) => {
      await client.appLockEnable(passphrase);
    },

    disableAppLock: async (passphrase) => {
      await client.appLockDisable(passphrase);
    },

    changeAppLockPassphrase: async (current, next) => {
      await client.appLockChange(current, next);
    },

    lock: async () => {
      // Send what the editor has not saved yet, while the core can still take it.
      await get().pageDocs.flushAll();
      await client.lock();
      // The core dropped its documents with the key; forget the editor's copies without asking it.
      get().pageDocs.discardAll();
      hooks.locked();
    },

    unlock: async (passphrase) => {
      await client.unlock(passphrase);
      await get().boot();
    },

    exportBackup: () => client.backupExport(),

    importBackup: async (kit, path) => {
      const result = await hooks.ownWorkspace(() => client.backupImport(path, kit));
      await get().boot();
      return result;
    },
  };

  return {
    initial,
    load,
    refresh,
    onEvent: (event) => {
      switch (event.type) {
        case "SecurityChanged": {
          const { phase, workspace } = get();
          // Without a workspace (a restore in progress) or while locked, the next boot reads it.
          if (workspace !== null && phase !== "locked") {
            refreshQuietly(refresh);
          }
          break;
        }
        case "DeviceRenamed": {
          // The list shows this device under its current name; no need to read it again.
          const { name } = event.data;
          set((state) => ({
            devices: state.devices.map((device) => (device.isThisDevice ? { ...device, name } : device)),
          }));
          break;
        }
        // The other events are not about security.
      }
    },
  };
}
