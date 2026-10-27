// The sync part of the app store (Milestone 6, ADR-0021): where sync stands on this device
// (`SyncInfo`) and the actions of Settings -> Sync. `store.ts` composes it with the rest.
//
// As in the other slices, reads are "latest request wins" and the event refreshes: the core
// (through the sync driver) emits `SyncChanged` whenever the status moves (Idle, Pushing,
// Pulling, Offline, Error, Forked), and the UI reads `SyncStatus` again. Chunks move inside a
// cycle (ADR-0023): `AttachmentsChanged` (a chunk arrived or left) also refreshes a configured
// status, so the transfer counts of Settings -> Sync fall as they move. `SyncRequested` is
// for the driver, not for the UI. Invite tokens never enter the store: the form that asks
// for one hands it to `configureSync` and forgets it.

import { toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, Event as CoreEvent, SyncInfo, SyncState } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

export interface SyncData {
  /**
   * Where sync stands on this device. `null` without a workspace, while locked, and when
   * the core could not report it.
   */
  sync: SyncInfo | null;
}

export const EMPTY_SYNC: SyncData = { sync: null };

/** True while the driver is moving data: "Sync now" waits for it to finish. */
export function isSyncBusy(state: SyncState): boolean {
  return state === "Pushing" || state === "Pulling";
}

/**
 * Every action rejects with a `CoreError` when the core refuses; the status that follows
 * comes back through `SyncChanged` (the answer of `configureSync` is applied right away).
 */
export interface SyncActions {
  /** Reads the status again (for example after a failed read). */
  refreshSync: () => Promise<void>;
  /**
   * Connects this device to a sync server. `InvalidInput` for the field `serverUrl` when the
   * address is not http(s), for `inviteToken` when the server does not know this account yet
   * and a token is needed (or the token given was refused).
   */
  configureSync: (serverUrl: string, inviteToken?: string) => Promise<SyncInfo>;
  /** Stops syncing and forgets the server; the notes stay on this device. */
  disconnectSync: () => Promise<SyncInfo>;
  /** Asks the driver to run a cycle now. */
  syncNow: () => Promise<void>;
}

export type SyncSliceState = SyncData & SyncActions;

export interface SyncSlice {
  initial: SyncSliceState;
  /**
   * Reads the status for the boot sequence. A failure other than `Locked` leaves it unknown
   * instead of stopping the app: sync matters for Settings and the indicator, not for
   * opening the notes. Returns nothing when a newer read started meanwhile.
   */
  load: () => Promise<Partial<SyncData>>;
  /** Reads the status again. */
  refresh: () => Promise<void>;
  onEvent: (event: CoreEvent) => void;
}

export function createSyncSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): SyncSlice {
  // Bumped by every read and every configuration; only the answer to the latest one is applied.
  let ticket = 0;

  async function load(): Promise<Partial<SyncData>> {
    const mine = (ticket += 1);
    let sync: SyncInfo | null;
    try {
      sync = await client.syncStatus();
    } catch (error) {
      const failure = toCoreError(error);
      if (failure.code === "Locked") {
        throw failure;
      }
      console.error("could not read the sync status", failure.code);
      sync = null;
    }
    return mine === ticket ? { sync } : {};
  }

  async function refresh(): Promise<void> {
    const mine = (ticket += 1);
    const sync = await client.syncStatus();
    if (mine === ticket) {
      set({ sync });
    }
  }

  async function configure(serverUrl: string | null, inviteToken?: string): Promise<SyncInfo> {
    const mine = (ticket += 1);
    const sync = await client.syncConfigure(serverUrl, inviteToken);
    // A `SyncChanged` that arrived meanwhile started a newer read, which wins.
    if (mine === ticket) {
      set({ sync });
    }
    return sync;
  }

  const initial: SyncSliceState = {
    ...EMPTY_SYNC,

    refreshSync: refresh,

    configureSync: (serverUrl, inviteToken) => configure(serverUrl, inviteToken),

    disconnectSync: () => configure(null),

    syncNow: () => client.syncNow(),
  };

  return {
    initial,
    load,
    refresh,
    onEvent: (event) => {
      switch (event.type) {
        case "SyncChanged": {
          const { phase, workspace } = get();
          // Without a workspace or while locked, the next boot reads it.
          if (workspace !== null && phase !== "locked") {
            refreshQuietly(refresh);
          }
          break;
        }
        case "AttachmentsChanged": {
          const { phase, workspace, sync } = get();
          if (sync?.configured === true && workspace !== null && phase !== "locked") {
            refreshQuietly(refresh);
          }
          break;
        }
        // `SyncRequested` asks the driver for a cycle; the other events are not about sync.
      }
    },
  };
}
