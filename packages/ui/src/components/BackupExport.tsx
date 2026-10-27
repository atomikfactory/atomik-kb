// "Export a backup" outside Settings -> Security (the sync fork banner, the screen of a
// removed device): the existing export, with its outcome shown next to the button.

import { toCoreError } from "@atomik-kb/core-client";
import { useState } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { useAppStore } from "../store-context";

export type BackupExportState =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "done"; path: string }
  | { kind: "cancelled" }
  | { kind: "error"; message: string };

/** The export and where it stands: the shell asks where, the core writes the file. */
export function useBackupExport(): { state: BackupExportState; run: () => Promise<void> } {
  const exportBackup = useAppStore((store) => store.exportBackup);
  const [state, setState] = useState<BackupExportState>({ kind: "idle" });

  async function run(): Promise<void> {
    setState({ kind: "busy" });
    try {
      const { path } = await exportBackup();
      setState({ kind: "done", path });
    } catch (failure) {
      const reason = toCoreError(failure);
      if (isFieldError(reason, "path")) {
        setState({ kind: "cancelled" });
      } else if (reason.code === "InvalidInput") {
        // The core refuses to export a workspace without a kit: nothing could restore it.
        setState({ kind: "error", message: t("security.backup.needsKit") });
      } else {
        setState({ kind: "error", message: t("security.backup.error", { message: reason.message }) });
      }
    }
  }

  return { state, run };
}

/** The outcome under the button: the file written, a cancelled dialog, or why it failed. */
export function BackupExportResult({ state, className }: { state: BackupExportState; className?: string }) {
  const classes = (extra?: string): string => [className, extra].filter((name) => name !== undefined).join(" ");
  switch (state.kind) {
    case "done":
      return (
        <p className={classes()} role="status">
          {t("backupExport.saved", { path: state.path })}
        </p>
      );
    case "cancelled":
      return (
        <p className={classes()} role="status">
          {t("security.backup.cancelled")}
        </p>
      );
    case "error":
      return (
        <p className={classes("error-text")} role="alert">
          {state.message}
        </p>
      );
    case "idle":
    case "busy":
      return null;
  }
}
