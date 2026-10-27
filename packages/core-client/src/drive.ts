// Google Drive as a target of the scheduled backups (ADR-0059, `docs/architecture/drive-backup.md` sections 3 and 7):
// `DriveApi` is what the UI calls; `TauriDrive` (`drive-tauri.ts`, the `@atomik-kb/core-client/tauri` entry) carries it to
// the shell's `drive_*` commands and `MemoryDrive` (`drive-memory.ts`, the dev page's `?drive=fake`) fakes it.
// `Transport.drive` is optional; `CoreClient.hasDrive()` / `drive()` hand it out.

import { CoreError } from "./errors";
import type { BackupFileInfo } from "./generated/BackupFileInfo";
import type { DriveClientInput } from "./generated/DriveClientInput";
import type { DriveProgress } from "./generated/DriveProgress";
import type { DriveStatus } from "./generated/DriveStatus";

export type { BackupFileInfo, DriveClientInput, DriveProgress, DriveStatus };

/** Why a Drive command failed (`details.reason` of its `CoreError`), section 3.4 of the design doc. */
export const DRIVE_ERROR_REASONS = [
  "consentDeclined",
  "cancelled",
  "noClient",
  "browserTimeout",
  "denied",
  "reconnect",
  "quota",
  "notConnected",
  "notFound",
  "network",
  "server",
] as const;

/** One of {@link DRIVE_ERROR_REASONS}. */
export type DriveErrorReason = (typeof DRIVE_ERROR_REASONS)[number];

/** The Google Drive target of this device. Every method rejects with a `CoreError` (see {@link driveReasonOf}). */
export interface DriveApi {
  /** The client in use, the connection and the last upload (no network). */
  status(): Promise<DriveStatus>;
  /**
   * The shell's native consent dialog, then the sign-in in the system browser, then the "atomik-kb backups" folder found or
   * made. Resolves when it is done; rejects with `consentDeclined`, `browserTimeout`, `cancelled`, `denied` or `noClient`.
   */
  connect(): Promise<DriveStatus>;
  /** Revokes the access and forgets it; the files stay in Drive. */
  disconnect(): Promise<DriveStatus>;
  /** Uses one's own Google client (`null`: the built-in one). Disconnects. */
  setClient(client: DriveClientInput | null): Promise<DriveStatus>;
  /** The backups in the Drive folder, newest first, at most 200 (`id` is the Drive file id). */
  listBackups(): Promise<BackupFileInfo[]>;
  /** Downloads a backup into a staging file and resolves with its absolute path, which `backupImport` takes. */
  download(fileId: string, onProgress: (progress: DriveProgress) => void): Promise<string>;
  /** Ends a connection waiting for the browser or a download that runs (it rejects with `cancelled`). */
  cancel(): Promise<void>;
}

/** The reason of a Drive command's failure, when the error carries a known one. */
export function driveReasonOf(error: unknown): DriveErrorReason | undefined {
  if (!(error instanceof CoreError)) {
    return undefined;
  }
  const details: unknown = error.details;
  if (typeof details !== "object" || details === null || !("reason" in details)) {
    return undefined;
  }
  const reason = details.reason;
  return DRIVE_ERROR_REASONS.find((code) => code === reason);
}
