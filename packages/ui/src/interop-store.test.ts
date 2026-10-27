// Import and export in the app store (ADR-0036, store-interop.ts): what the shell can do, the runs and the
// progress their events report (only while the run is on), the toast of an export and of a CSV, toasts
// that go by themselves or stay, nothing kept while locked, print mode's flag, and the readers of the
// scheduled backup settings.

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { createAppStore } from "./store";
import {
  MAX_TOASTS,
  TOAST_MS,
  exportToastText,
  fileNameOf,
  parseBackupFolder,
  parseBackupLast,
  parseBackupSchedule,
} from "./store-interop";
import { backupResult, exportResult, importReport, interopTransport } from "./test/interop";
import type { InteropOptions } from "./test/interop";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const CANCELLED = new CoreError({ code: "InvalidInput", message: "no file was chosen", details: { field: "path" } });

async function setup(options: InteropOptions = {}, memoryOptions: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: [{ title: "Notes" }] }],
    ...memoryOptions,
  });
  const wrapped = interopTransport(memory, options);
  const store = createAppStore(new CoreClient(wrapped.transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  return { memory, store, stop, ...wrapped };
}

const OPTIONS = { spaceId: "s", includeChildren: false, format: "markdown" } as const;

describe("capabilities", () => {
  it("follow the optional methods of the transport", async () => {
    const none = await setup();
    expect(none.store.getState().interopCapabilities).toEqual({
      export: false,
      import: false,
      saveTextFile: false,
      scheduledBackups: false,
      drive: false,
    });
    none.stop();

    const all = await setup({
      methods: {
        exportRun: () => Promise.resolve(exportResult()),
        importRun: () => Promise.resolve(importReport()),
        saveTextFile: () => Promise.resolve("a.csv"),
        backupFolderChoose: () => Promise.resolve("C:\\Backups"),
        backupRunNow: () => Promise.resolve(backupResult()),
      },
    });
    expect(all.store.getState().interopCapabilities).toEqual({
      export: true,
      import: true,
      saveTextFile: true,
      scheduledBackups: true,
      drive: false,
    });
    all.stop();
  });

  it("need both calls for scheduled backups", async () => {
    const half = await setup({ methods: { backupRunNow: () => Promise.resolve(backupResult()) } });
    expect(half.store.getState().interopCapabilities.scheduledBackups).toBe(false);
    half.stop();
  });
});

describe("runExport", () => {
  it("shows the progress of its own run only, and says what it wrote in a toast", async () => {
    let finish: (value: ReturnType<typeof exportResult>) => void = () => undefined;
    const ctx = await setup({
      methods: {
        exportRun: () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      },
    });
    const progress = (done: number) => ({ type: "ExportProgress", data: { exportId: 1, done, total: 10 } }) as const;

    // An event with no run on (another window's, or a late one) shows nothing.
    ctx.emit(progress(1));
    expect(ctx.store.getState().exportProgress).toBeNull();

    const run = ctx.store.getState().runExport(OPTIONS);
    expect(ctx.store.getState().exportProgress).toBeNull(); // before the first event
    ctx.emit(progress(4));
    expect(ctx.store.getState().exportProgress).toEqual({ done: 4, total: 10 });

    finish(exportResult({ path: "C:\\Users\\Ada\\Notes.zip", pages: 10 }));
    await run;
    expect(ctx.store.getState().exportProgress).toBeNull();
    expect(ctx.store.getState().toasts.map((toast) => toast.text)).toEqual(["Exported 10 pages to Notes.zip."]);
    ctx.emit(progress(9));
    expect(ctx.store.getState().exportProgress).toBeNull();
    ctx.stop();
  });

  it("rejects with what the shell says, and shows no toast, for a cancelled dialog too", async () => {
    const ctx = await setup({ methods: { exportRun: () => Promise.reject(CANCELLED) } });
    await expect(ctx.store.getState().runExport(OPTIONS)).rejects.toBe(CANCELLED);
    expect(ctx.store.getState().toasts).toEqual([]);
    expect(ctx.store.getState().exportProgress).toBeNull();
    ctx.stop();
  });
});

describe("the text of the export toast", () => {
  it("says one page, many pages, and how many items were left out", () => {
    expect(exportToastText(exportResult({ path: "/home/ada/Notes.md", pages: 1 }))).toBe("Exported 1 page to Notes.md.");
    expect(exportToastText(exportResult({ path: "C:\\x\\Space.zip", pages: 12 }))).toBe("Exported 12 pages to Space.zip.");
    const left = { code: "attachmentMissing", attachmentId: "a" } as const;
    expect(exportToastText(exportResult({ warnings: [left] }))).toBe("Exported 3 pages to Notes.zip. 1 item was left out.");
    expect(exportToastText(exportResult({ warnings: [left, left, left] }))).toBe(
      "Exported 3 pages to Notes.zip. 3 items were left out.",
    );
  });

  it("names the file by its last segment in either spelling", () => {
    expect(fileNameOf("C:\\a\\b\\c.zip")).toBe("c.zip");
    expect(fileNameOf("/a/b/c.html")).toBe("c.html");
    expect(fileNameOf("c.md")).toBe("c.md");
  });
});

describe("runImport", () => {
  it("shows the pages written while its run is on, and resolves with the report", async () => {
    let finish: (value: ReturnType<typeof importReport>) => void = () => undefined;
    const ctx = await setup({
      methods: {
        importRun: () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      },
    });
    const progress = (done: number) => ({ type: "ImportProgress", data: { importId: 1, done, total: 8 } }) as const;
    ctx.emit(progress(1));
    expect(ctx.store.getState().importProgress).toBeNull();

    const run = ctx.store.getState().runImport({ spaceId: "s", source: "zip" });
    ctx.emit(progress(3));
    expect(ctx.store.getState().importProgress).toEqual({ done: 3, total: 8 });
    const report = importReport({ pages: 8 });
    finish(report);
    await expect(run).resolves.toBe(report);
    expect(ctx.store.getState().importProgress).toBeNull();
    ctx.stop();
  });
});

describe("exportViewCsv", () => {
  it("formats the rows, saves them under the name and says so", async () => {
    const saved: [string, string][] = [];
    const ctx = await setup({
      methods: {
        saveTextFile: (name, text) => {
          saved.push([name, text]);
          return Promise.resolve(`C:\\out\\${name}`);
        },
      },
      intercept: (request) => (request.type === "ExportCsv" ? { type: "Csv", data: "Name\r\nA\r\n" } : undefined),
    });
    const path = await ctx.store.getState().exportViewCsv("s", ["p1", "p2"], [{ type: "Title" }], "Tasks.csv");
    expect(path).toBe("C:\\out\\Tasks.csv");
    expect(saved).toEqual([["Tasks.csv", "Name\r\nA\r\n"]]);
    const asked = ctx.requests.find((request): request is Extract<Request, { type: "ExportCsv" }> => request.type === "ExportCsv");
    expect(asked?.data).toEqual({ spaceId: "s", pageIds: ["p1", "p2"], columns: [{ type: "Title" }] });
    expect(ctx.store.getState().toasts.map((toast) => toast.text)).toEqual(["Saved Tasks.csv."]);
    ctx.stop();
  });

  it("is silent when the save dialog is cancelled, and passes on any other failure", async () => {
    const cancelled = await setup({
      methods: { saveTextFile: () => Promise.reject(CANCELLED) },
      intercept: (request) => (request.type === "ExportCsv" ? { type: "Csv", data: "x" } : undefined),
    });
    await expect(cancelled.store.getState().exportViewCsv("s", [], [], "a.csv")).resolves.toBeNull();
    expect(cancelled.store.getState().toasts).toEqual([]);
    cancelled.stop();

    const broken = await setup({
      methods: { saveTextFile: () => Promise.reject(new CoreError({ code: "Storage", message: "disk full" })) },
      intercept: (request) => (request.type === "ExportCsv" ? { type: "Csv", data: "x" } : undefined),
    });
    await expect(broken.store.getState().exportViewCsv("s", [], [], "a.csv")).rejects.toMatchObject({ message: "disk full" });
    expect(broken.store.getState().toasts).toEqual([]);
    broken.stop();
  });
});

describe("toasts", () => {
  it("go by themselves after 15 seconds, each on its own clock, and an error stays", async () => {
    vi.useFakeTimers();
    const memory = new MemoryTransport({ workspaceName: "Home", spaces: [{ name: "Personal" }] });
    const transport: Transport = interopTransport(memory).transport;
    const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
    const { showToast, dismissToast } = store.getState();

    showToast("first");
    await vi.advanceTimersByTimeAsync(TOAST_MS / 2);
    showToast("second");
    showToast("broken", "error");
    expect(store.getState().toasts.map((toast) => toast.text)).toEqual(["first", "second", "broken"]);

    await vi.advanceTimersByTimeAsync(TOAST_MS / 2);
    expect(store.getState().toasts.map((toast) => toast.text)).toEqual(["second", "broken"]);
    await vi.advanceTimersByTimeAsync(TOAST_MS);
    expect(store.getState().toasts.map((toast) => toast.text)).toEqual(["broken"]);

    const [error] = store.getState().toasts;
    dismissToast(error?.id ?? -1);
    expect(store.getState().toasts).toEqual([]);
  });

  it("keep the newest few", () => {
    const memory = new MemoryTransport({ workspaceName: "Home", spaces: [] });
    const store = createAppStore(new CoreClient(interopTransport(memory).transport), { corePollMs: 5 });
    for (let n = 1; n <= MAX_TOASTS + 2; n += 1) {
      store.getState().showToast(`toast ${String(n)}`, "error");
    }
    expect(store.getState().toasts.map((toast) => toast.text)).toEqual(["toast 3", "toast 4", "toast 5", "toast 6"]);
  });

  it("are gone, and so is a print mode, once the app is locked", async () => {
    const ctx = await setup(
      {},
      { security: { kitCreatedAt: 1, kitKey: new Uint8Array(32).fill(3), appLockPassphrase: "correct horse battery" } },
    );
    ctx.store.getState().showToast("Exported 1 page to Secret plans.md.");
    ctx.store.getState().startPrint();
    expect(ctx.store.getState().printing).toBe(true);
    await ctx.store.getState().lock();
    expect(ctx.store.getState().phase).toBe("locked");
    // Toasts hold page titles (as file names): the lock screen forgets them with the rest of the workspace.
    expect(ctx.store.getState().toasts).toEqual([]);
    expect(ctx.store.getState().printing).toBe(false);
    ctx.stop();
  });
});

describe("print mode", () => {
  it("is a flag that starts and ends", async () => {
    const ctx = await setup();
    expect(ctx.store.getState().printing).toBe(false);
    ctx.store.getState().startPrint();
    ctx.store.getState().startPrint();
    expect(ctx.store.getState().printing).toBe(true);
    ctx.store.getState().endPrint();
    ctx.store.getState().endPrint();
    expect(ctx.store.getState().printing).toBe(false);
    ctx.stop();
  });
});

describe("the readers of the scheduled backup settings", () => {
  it("read the schedule leniently", () => {
    expect(parseBackupSchedule(null)).toEqual({ enabled: false, interval: "daily", keep: 7 });
    expect(parseBackupSchedule("yes")).toEqual({ enabled: false, interval: "daily", keep: 7 });
    expect(parseBackupSchedule({ enabled: true, interval: "weekly", keep: 30 })).toEqual({
      enabled: true,
      interval: "weekly",
      keep: 30,
    });
    expect(parseBackupSchedule({ enabled: "true", interval: "hourly", keep: 0 })).toEqual({
      enabled: false,
      interval: "daily",
      keep: 7,
    });
    expect(parseBackupSchedule({ enabled: true, keep: 101 }).keep).toBe(7);
    expect(parseBackupSchedule({ enabled: true, keep: 2.5 }).keep).toBe(7);
  });

  it("read the folder and the last result", () => {
    expect(parseBackupFolder("C:\\Backups")).toBe("C:\\Backups");
    expect(parseBackupFolder("")).toBeNull();
    expect(parseBackupFolder(3)).toBeNull();
    expect(parseBackupFolder(undefined)).toBeNull();

    expect(parseBackupLast({ at: 5, ok: true, lastSuccessAt: 5, file: "a.atomikkb" })).toEqual({
      at: 5,
      ok: true,
      lastSuccessAt: 5,
      file: "a.atomikkb",
    });
    expect(parseBackupLast({ at: 9, ok: false, error: "Storage: disk full" })).toEqual({
      at: 9,
      ok: false,
      error: "Storage: disk full",
    });
    expect(parseBackupLast({ ok: true })).toBeNull();
    expect(parseBackupLast([])).toBeNull();
    expect(parseBackupLast(null)).toBeNull();
  });
});

describe("the strings", () => {
  it("name every kind of import warning", () => {
    for (const code of [
      "unsupportedFile",
      "notUtf8",
      "tooLarge",
      "unresolvedLink",
      "missingFile",
      "remoteImage",
      "embeddedImage",
      "propertyLimit",
      "invalidValue",
      "truncated",
      "unsupportedContent",
      "attachmentFailed",
    ] as const) {
      expect(t(`import.warning.${code}`)).not.toBe("");
    }
  });
});
