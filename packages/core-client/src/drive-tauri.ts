// The desktop implementation of `DriveApi` (ADR-0059, `docs/architecture/drive-backup.md` section 3.3): the shell's
// `drive_*` commands (`apps/desktop/src-tauri/src/drive_host.rs`) over `invoke`, with a `Channel` for a download's
// progress. Part of the `@atomik-kb/core-client/tauri` entry, so web builds do not pull in `@tauri-apps/api`.

import { Channel, invoke } from "@tauri-apps/api/core";

import type { BackupFileInfo, DriveApi, DriveClientInput, DriveProgress, DriveStatus } from "./drive";
import { internalError, toCoreError } from "./errors";

/** Runs a command and turns whatever it rejects with into a `CoreError`. */
async function call<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (error) {
    throw toCoreError(error);
  }
}

export class TauriDrive implements DriveApi {
  status(): Promise<DriveStatus> {
    return call(() => invoke<DriveStatus>("drive_status"));
  }

  connect(): Promise<DriveStatus> {
    return call(() => invoke<DriveStatus>("drive_connect"));
  }

  disconnect(): Promise<DriveStatus> {
    return call(() => invoke<DriveStatus>("drive_disconnect"));
  }

  setClient(client: DriveClientInput | null): Promise<DriveStatus> {
    return call(() => invoke<DriveStatus>("drive_client_set", { client }));
  }

  listBackups(): Promise<BackupFileInfo[]> {
    return call(() => invoke<BackupFileInfo[]>("drive_backups_list"));
  }

  download(fileId: string, onProgress: (progress: DriveProgress) => void): Promise<string> {
    return call(async () => {
      const onEvent = new Channel<DriveProgress>();
      onEvent.onmessage = onProgress;
      const path = await invoke<unknown>("drive_backup_download", { fileId, onEvent });
      if (typeof path !== "string") {
        throw internalError("The desktop shell answered drive_backup_download without a path");
      }
      return path;
    });
  }

  cancel(): Promise<void> {
    return call(() => invoke<void>("drive_cancel"));
  }
}
