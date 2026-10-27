// The Google Drive target's part of the app store (ADR-0059, `docs/architecture/drive-backup.md` section 5): what Settings
// -> Security -> Backup shows about it (the client in use, the connection, the last upload) and the four things it can do:
// connect, disconnect, use one's own Google client, and cancel a sign-in that waits for the browser.
//
// - **Status.** `drive_status` answers without the network, so it is read whenever the section opens
//   (`loadDriveStatus`) and again when the core says the shell-owned setting `app.backup.drive` changed (the shell writes the
//   connection and the outcome of each delivery there), once somebody has read it. An answer that is older than a newer one
//   (a read that left before a connect ended) is dropped.
// - **Errors.** An action never rejects: a failure becomes `driveNotice`, which keeps the failure's `details.reason`
//   (`driveReasonOf`) so that the screen words it. A sign-in that was declined in the consent dialog or cancelled is an
//   information notice, not an error.
// - **Locking.** The account's address is the person's: nothing here survives the lock (`EMPTY_DRIVE`), a sign-in that waits
//   for the browser is cancelled and what an old action still delivers is dropped (`onLocked`).
// - **Secrets.** A client secret goes from the form to the shell in one call; it is never kept here or logged.
//
// Owner: agent D of the Google Drive backup batch.

import { driveReasonOf, toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, DriveApi, DriveClientInput, DriveErrorReason, DriveStatus, Event as CoreEvent } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

/** The device setting that holds the Drive connection (`kb_types::BACKUP_DRIVE_SETTING`): shell-owned, never written from here. */
export const BACKUP_DRIVE_KEY = "app.backup.drive";

/** What the last Drive action said when it did not end in a connection or a change: an error, or a plain "not connected". */
export interface DriveNotice {
  kind: "info" | "error";
  /** The failure's reason when it carried a known one; `null` for anything else (then `message` is what to show). */
  reason: DriveErrorReason | null;
  /** The core's own message: generic by contract (never a token, an id or a name). */
  message: string;
}

export interface DriveData {
  /** The Drive target of this device; `null` until it was read (or when the shell has none). */
  driveStatus: DriveStatus | null;
  /** A sign-in is under way: the consent dialog, then the browser. */
  driveConnecting: boolean;
  /** A disconnect or a change of the Google client runs. */
  driveBusy: boolean;
  /** Why the last action did not do what was asked; `null` when it did or nothing was asked yet. */
  driveNotice: DriveNotice | null;
}

export const EMPTY_DRIVE: DriveData = {
  driveStatus: null,
  driveConnecting: false,
  driveBusy: false,
  driveNotice: null,
};

/** No action rejects: a failure is `driveNotice`. Each does nothing where the transport has no Drive. */
export interface DriveActions {
  /** Reads the status (no network); the connection and the last upload follow `SettingChanged` afterwards. */
  loadDriveStatus: () => Promise<void>;
  /** The consent dialog, the sign-in in the browser, the backup folder; waits for all of it. */
  connectDrive: () => Promise<boolean>;
  /** Ends a sign-in that waits for the browser; `connectDrive` then ends with the notice "cancelled". */
  cancelDriveConnect: () => Promise<void>;
  /** Revokes the access and forgets it; the files stay in Drive. */
  disconnectDrive: () => Promise<boolean>;
  /** Uses one's own Google client (`null`: the built-in one); this disconnects. */
  setDriveClient: (client: DriveClientInput | null) => Promise<boolean>;
  /** Drops the notice (the person read it, or started over). */
  clearDriveNotice: () => void;
}

export type DriveState = DriveData & DriveActions;

export interface DriveSlice {
  initial: DriveState;
  onEvent: (event: CoreEvent) => void;
  /** The workspace was locked: ends a waiting sign-in and drops whatever an action in flight would still deliver. */
  onLocked: () => void;
}

/** The notice a failed action leaves: the reason it carried, and an information notice when the person gave up. */
export function driveNoticeOf(error: unknown): DriveNotice {
  const failure = toCoreError(error);
  const reason = driveReasonOf(failure) ?? null;
  return {
    kind: reason === "cancelled" || reason === "consentDeclined" ? "info" : "error",
    reason,
    message: failure.message,
  };
}

export function createDriveSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): DriveSlice {
  // Bumped when the workspace locks: an action that started before it ends quietly.
  let era = 0;
  // Bumped by every read and every answer that carries a status: only the newest one is shown.
  let newest = 0;

  function drive(): DriveApi | null {
    return client.hasDrive() ? client.drive() : null;
  }

  function show(status: DriveStatus): void {
    newest += 1;
    set({ driveStatus: status });
  }

  /** What marks a sign-in, and what marks any other action, as running. */
  const connecting = (on: boolean): Partial<DriveData> => ({ driveConnecting: on });
  const busy = (on: boolean): Partial<DriveData> => ({ driveBusy: on });

  /**
   * Runs one action that answers a status: marks it running, shows the status or the notice, and always un-marks it.
   * Resolves `true` when the action did what was asked.
   */
  async function act(mark: (on: boolean) => Partial<DriveData>, run: (api: DriveApi) => Promise<DriveStatus>): Promise<boolean> {
    const api = drive();
    const state = get();
    if (api === null || state.driveConnecting || state.driveBusy) {
      return false;
    }
    const started = era;
    set({ ...mark(true), driveNotice: null });
    try {
      const status = await run(api);
      if (era !== started) {
        return false;
      }
      show(status);
      return true;
    } catch (error) {
      if (era === started) {
        const notice = driveNoticeOf(error);
        if (notice.kind === "error") {
          // The code only. Declining the consent or cancelling is the person's choice, not a failure to log.
          console.error("a Google Drive action failed", toCoreError(error).code);
        }
        set({ driveNotice: notice });
      }
      return false;
    } finally {
      if (era === started) {
        set(mark(false));
      }
    }
  }

  async function load(): Promise<void> {
    const api = drive();
    if (api === null) {
      return;
    }
    const started = era;
    newest += 1;
    const mine = newest;
    try {
      const status = await api.status();
      if (era === started && mine === newest) {
        set({ driveStatus: status });
      }
    } catch (error) {
      if (era === started) {
        // The code only: nothing of the reply goes to the log.
        console.error("could not read the Google Drive status", toCoreError(error).code);
        set({ driveNotice: driveNoticeOf(error) });
      }
    }
  }

  const initial: DriveState = {
    ...EMPTY_DRIVE,

    loadDriveStatus: load,

    connectDrive: () => act(connecting, (api) => api.connect()),

    cancelDriveConnect: async () => {
      const api = drive();
      if (api === null || !get().driveConnecting) {
        return;
      }
      try {
        await api.cancel();
      } catch (error) {
        console.error("a Google Drive sign-in could not be cancelled", toCoreError(error).code);
      }
    },

    disconnectDrive: () => act(busy, (api) => api.disconnect()),

    setDriveClient: (input) => act(busy, (api) => api.setClient(input)),

    clearDriveNotice: () => {
      if (get().driveNotice !== null) {
        set({ driveNotice: null });
      }
    },
  };

  return {
    initial,
    onEvent: (event) => {
      // Only once somebody has read the status: before that there is no slice to refresh.
      if (event.type === "SettingChanged" && event.data.key === BACKUP_DRIVE_KEY && get().driveStatus !== null) {
        load().catch(() => undefined);
      }
    },
    onLocked: () => {
      era += 1;
      if (get().driveConnecting) {
        const api = drive();
        api?.cancel().catch(() => undefined);
      }
    },
  };
}
