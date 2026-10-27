// The import and export part of the app store (ADR-0036): what the shell can do (the capability
// checks), the running export and import with the progress their events report, the toasts that
// say how an export or a CSV went, the print mode flag, and the scheduled backups' folder and
// "Back up now". `store.ts` composes it with the rest.
//
// The runs are one call each (`exportRun`, `importRun`): the shell asks where, drives the core's
// session and writes the files, and the core emits `ExportProgress` and `ImportProgress` on the
// way. This slice only shows them while its own run is on, so an event that arrives late, or from
// another window's run, never leaves a stale progress behind.
//
// Nothing here is kept while the workspace is locked: toasts hold page titles (as file names).

import { toCoreError } from "@atomik-kb/core-client";
import type {
  BackupResult,
  BackupRunStatus,
  BackupSchedule,
  CoreClient,
  CsvColumn,
  Event as CoreEvent,
  ExportRunOptions,
  ExportRunResult,
  ImportReport,
  ImportRunOptions,
  JsonValue,
  PageId,
  SpaceId,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { isFieldError } from "./errors";
import { t, tp } from "./i18n";
import type { AppState } from "./store";

/** The device settings of scheduled backups (`kb_types::BACKUP_*_SETTING`). */
export const BACKUP_SCHEDULE_KEY = "app.backup.schedule";
/** The folder of scheduled backups: written by the shell after its folder dialog, read-only here. */
export const BACKUP_FOLDER_KEY = "app.backup.folder";
/** The outcome of the last scheduled backup: written by the shell, read-only here. */
export const BACKUP_LAST_KEY = "app.backup.last";

/** How many backups a schedule keeps by default, and the bounds the core accepts. */
export const DEFAULT_BACKUP_KEEP = 7;
export const MIN_BACKUP_KEEP = 1;
export const MAX_BACKUP_KEEP = 100;

/** How long an information toast stays when nobody dismisses it, in milliseconds. */
export const TOAST_MS = 15_000;
/** Most toasts on screen at once; a newer one pushes the oldest out. */
export const MAX_TOASTS = 4;

/** What the shell can do beyond requests (see `Transport`); each entry of the UI is hidden without its one. */
export interface InteropCapabilities {
  /** Writing export files (`exportRun`). */
  export: boolean;
  /** Reading import sources (`importRun`). */
  import: boolean;
  /** Saving a text file such as the CSV of a view (`saveTextFile`). */
  saveTextFile: boolean;
  /** Choosing the folder of scheduled backups and running one now. */
  scheduledBackups: boolean;
  /** Google Drive as a backup target (`CoreClient.hasDrive`, ADR-0059): the Drive target of Settings is hidden without it. */
  drive: boolean;
}

/** How far a run got: pages done of pages in all. */
export interface RunProgress {
  done: number;
  total: number;
}

/** A button of a toast ("Open" on the notice of unsent changes that were copied to new pages). */
export interface ToastAction {
  label: string;
  /** Runs when the button is pressed; the toast goes with it. */
  run: () => void;
}

/** One toast on screen. */
export interface Toast {
  /** Distinct for the life of the store. */
  id: number;
  text: string;
  /** An error stays until dismissed; information goes by itself. */
  kind: "info" | "error";
  /** A button next to the text, if the toast has one. */
  action?: ToastAction;
}

export interface InteropData {
  /** Pages rendered by the running export; `null` while none runs or before its first event. */
  exportProgress: RunProgress | null;
  /** Pages written by the running import; `null` while none runs or while the files are read and uploaded. */
  importProgress: RunProgress | null;
  toasts: readonly Toast[];
  /**
   * Print mode (ADR-0036, section 4): the open page renders fully (every row of a table view, every
   * picture of a gallery, a PDF as its card), the app waits for fonts and images, calls
   * `window.print()` and turns the mode off on `afterprint`. See `print.ts`.
   */
  printing: boolean;
}

export const EMPTY_INTEROP: InteropData = {
  exportProgress: null,
  importProgress: null,
  toasts: [],
  printing: false,
};

/** Every action rejects with a `CoreError` when the core or the shell refuses; a cancelled dialog is `InvalidInput` for `path`. */
export interface InteropActions {
  interopCapabilities: InteropCapabilities;
  /**
   * Exports a page, a page with its sub-pages or a space (the shell asks where to save), then says
   * so in a toast: "Exported N pages to <file name>" with the number of items left out.
   */
  runExport: (options: ExportRunOptions) => Promise<ExportRunResult>;
  /** Imports files, a folder or a `.zip` (the shell asks which) and resolves with the report. */
  runImport: (options: ImportRunOptions) => Promise<ImportReport>;
  /**
   * Formats the rows of a view as CSV (the core) and saves it under `fileName` (the shell asks where).
   * Resolves with the path written, or `null` when the dialog was cancelled (nothing to say then).
   */
  exportViewCsv: (
    spaceId: SpaceId,
    pageIds: PageId[],
    columns: CsvColumn[],
    fileName: string,
  ) => Promise<string | null>;
  /** Shows a toast, with a button when `action` is given. */
  showToast: (text: string, kind?: Toast["kind"], action?: ToastAction) => void;
  dismissToast: (id: number) => void;
  /** Turns print mode on for the open page. */
  startPrint: () => void;
  endPrint: () => void;
  /** Lets the person choose the folder of scheduled backups; `null` when the dialog was cancelled. */
  chooseBackupFolder: () => Promise<string | null>;
  /** Writes a scheduled backup into the chosen folder now. */
  runBackupNow: () => Promise<BackupResult>;
}

export type InteropState = InteropData & InteropActions;

export interface InteropSlice {
  initial: InteropState;
  onEvent: (event: CoreEvent) => void;
}

/** The last segment of a path in either spelling: the file name. */
export function fileNameOf(path: string): string {
  const parts = path.split(/[\\/]/).filter((part) => part !== "");
  return parts[parts.length - 1] ?? path;
}

function isObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Reads the stored schedule leniently: anything unexpected is off, daily, keeping 7. */
export function parseBackupSchedule(value: JsonValue | null | undefined): BackupSchedule {
  if (!isObject(value)) {
    return { enabled: false, interval: "daily", keep: DEFAULT_BACKUP_KEEP };
  }
  const { enabled, interval, keep } = value;
  return {
    enabled: enabled === true,
    interval: interval === "weekly" ? "weekly" : "daily",
    keep:
      typeof keep === "number" && Number.isInteger(keep) && keep >= MIN_BACKUP_KEEP && keep <= MAX_BACKUP_KEEP
        ? keep
        : DEFAULT_BACKUP_KEEP,
  };
}

/** Reads the stored folder of scheduled backups; `null` when none is set. */
export function parseBackupFolder(value: JsonValue | null | undefined): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

/** Reads the stored outcome of the last scheduled backup; `null` when there is none or it is not one. */
export function parseBackupLast(value: JsonValue | null | undefined): BackupRunStatus | null {
  if (!isObject(value) || typeof value.at !== "number" || typeof value.ok !== "boolean") {
    return null;
  }
  const status: BackupRunStatus = { at: value.at, ok: value.ok };
  if (typeof value.lastSuccessAt === "number") {
    status.lastSuccessAt = value.lastSuccessAt;
  }
  if (typeof value.file === "string") {
    status.file = value.file;
  }
  if (typeof value.error === "string") {
    status.error = value.error;
  }
  return status;
}

/** "Exported N pages to <file>" and, when something was left out, how many items. */
export function exportToastText(result: ExportRunResult): string {
  const file = fileNameOf(result.path);
  let text = tp("export.toast", result.pages, { file });
  const left = result.warnings.length;
  if (left > 0) {
    text += ` ${tp("export.toast.warnings", left)}`;
  }
  return text;
}

export function createInteropSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): InteropSlice {
  let nextToast = 0;
  const timers = new Map<number, ReturnType<typeof setTimeout>>();
  // True while this store's own run is on: progress events outside it are not shown.
  let exporting = false;
  let importing = false;

  function forget(id: number): void {
    const timer = timers.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      timers.delete(id);
    }
  }

  function dismiss(id: number): void {
    forget(id);
    set((state) =>
      state.toasts.some((toast) => toast.id === id) ? { toasts: state.toasts.filter((toast) => toast.id !== id) } : state,
    );
  }

  function show(text: string, kind: Toast["kind"] = "info", action?: ToastAction): void {
    nextToast += 1;
    const toast: Toast = { id: nextToast, text, kind, ...(action === undefined ? {} : { action }) };
    const kept = [...get().toasts, toast];
    const dropped = kept.slice(0, Math.max(0, kept.length - MAX_TOASTS));
    for (const old of dropped) {
      forget(old.id);
    }
    set({ toasts: kept.slice(dropped.length) });
    if (kind === "info") {
      timers.set(
        toast.id,
        setTimeout(() => {
          dismiss(toast.id);
        }, TOAST_MS),
      );
    }
  }

  const initial: InteropState = {
    ...EMPTY_INTEROP,
    interopCapabilities: {
      export: client.hasExport(),
      import: client.hasImport(),
      saveTextFile: client.hasSaveTextFile(),
      scheduledBackups: client.hasScheduledBackups(),
      drive: client.hasDrive(),
    },

    runExport: async (options) => {
      exporting = true;
      set({ exportProgress: null });
      try {
        const result = await client.exportRun(options);
        show(exportToastText(result));
        return result;
      } finally {
        exporting = false;
        set({ exportProgress: null });
      }
    },

    runImport: async (options) => {
      importing = true;
      set({ importProgress: null });
      try {
        return await client.importRun(options);
      } finally {
        importing = false;
        set({ importProgress: null });
      }
    },

    exportViewCsv: async (spaceId, pageIds, columns, fileName) => {
      const csv = await client.exportCsv(spaceId, pageIds, columns);
      let path: string;
      try {
        path = await client.saveTextFile(fileName, csv);
      } catch (error) {
        if (isFieldError(toCoreError(error), "path")) {
          return null; // the person closed the dialog
        }
        throw error;
      }
      show(t("csv.saved", { file: fileNameOf(path) }));
      return path;
    },

    showToast: show,
    dismissToast: dismiss,

    startPrint: () => {
      if (!get().printing) {
        set({ printing: true });
      }
    },

    endPrint: () => {
      if (get().printing) {
        set({ printing: false });
      }
    },

    chooseBackupFolder: async () => {
      let folder: string;
      try {
        folder = await client.backupFolderChoose();
      } catch (error) {
        if (isFieldError(toCoreError(error), "path")) {
          return null;
        }
        throw error;
      }
      // The shell stores it and `SettingChanged` follows; reading it again does not wait for that.
      await get().loadSetting(BACKUP_FOLDER_KEY);
      return folder;
    },

    runBackupNow: async () => {
      try {
        return await client.backupRunNow();
      } finally {
        // The shell records the outcome, a failure included.
        await get()
          .loadSetting(BACKUP_LAST_KEY)
          .catch(() => undefined);
      }
    },
  };

  return {
    initial,
    onEvent: (event) => {
      switch (event.type) {
        case "ExportProgress":
          if (exporting) {
            set({ exportProgress: { done: event.data.done, total: event.data.total } });
          }
          break;
        case "ImportProgress":
          if (importing) {
            set({ importProgress: { done: event.data.done, total: event.data.total } });
          }
          break;
        // The other events are not about import and export.
      }
    },
  };
}
