// Scheduled backups and the shell's text files (ADR-0036, 5.3 and 5.4): the file names and the
// pruning rule, the folder dialog, `backupRunNow` on the fake disk with its status setting, the
// settings only the shell writes, and `saveTextFile`.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { BackupRunStatus } from "./generated/BackupRunStatus";
import type { Event } from "./generated/Event";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import {
  BACKUP_AUTO_PREFIX,
  DEFAULT_BACKUP_KEEP,
  autoBackupName,
  backupNameTime,
  backupPeriodOver,
  backupsToPrune,
  isAutoBackupName,
  keepOf,
  scheduleOf,
} from "./memory-backup-schedule";

const MINUTE = 60_000;
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index + 5);

/** A local time: the names of scheduled backups are in local time, whatever the zone of the machine. */
const local = (year: number, month: number, day: number, hour: number, minute: number): number =>
  new Date(year, month - 1, day, hour, minute).getTime();

describe("the names of scheduled backups", () => {
  it("are atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb in local time", () => {
    expect(autoBackupName(local(2026, 10, 12, 9, 5))).toBe("atomik-kb-auto-2026-10-12-0905.atomikkb");
    expect(autoBackupName(local(2026, 1, 2, 23, 59))).toBe("atomik-kb-auto-2026-01-02-2359.atomikkb");
    expect(autoBackupName(local(2026, 12, 31, 0, 0))).toBe("atomik-kb-auto-2026-12-31-0000.atomikkb");
    expect(BACKUP_AUTO_PREFIX).toBe("atomik-kb-auto-");
  });

  it("are recognised only in exactly that form", () => {
    expect(isAutoBackupName("atomik-kb-auto-2026-10-12-0905.atomikkb")).toBe(true);
    for (const other of [
      "atomik-kb-2026-10-12.atomikkb",
      "atomik-kb-auto-2026-10-12-0905.atomikkb.bak",
      "my-atomik-kb-auto-2026-10-12-0905.atomikkb",
      "atomik-kb-auto-2026-1-12-0905.atomikkb",
      "atomik-kb-auto-2026-10-12-905.atomikkb",
      "atomik-kb-auto-2026-10-12-0905.ATOMIKKB",
      "notes.txt",
      "",
    ]) {
      expect(isAutoBackupName(other)).toBe(false);
    }
  });

  it("say when the backup was made, to the minute, the inverse of the name", () => {
    for (const ms of [local(2026, 10, 12, 9, 5), local(2026, 1, 2, 23, 59), local(2024, 2, 29, 0, 0), local(2026, 12, 31, 0, 0)]) {
      expect(backupNameTime(autoBackupName(ms))).toBe(ms);
    }
    expect(backupNameTime(autoBackupName(local(2026, 10, 12, 9, 5) + 42_000))).toBe(local(2026, 10, 12, 9, 5));
    for (const other of [
      "",
      "notes.txt",
      "atomik-kb-2026-10-12.atomikkb",
      "atomik-kb-auto-2026-02-30-0900.atomikkb",
      "atomik-kb-auto-2026-10-12-2460.atomikkb",
      "atomik-kb-auto-2026-13-01-0900.atomikkb",
    ]) {
      expect(backupNameTime(other), other).toBeUndefined();
    }
  });

  it("have a period that is over a day (or a week) less 15 minutes later, or when the clock was set back", () => {
    const made = local(2026, 10, 12, 9, 0);
    const MIN = 60_000;
    const DAY = 24 * 60 * MIN;
    expect(backupPeriodOver(made, made + 5 * MIN, "daily")).toBe(false);
    expect(backupPeriodOver(made, made + DAY - 16 * MIN, "daily")).toBe(false);
    expect(backupPeriodOver(made, made + DAY - 15 * MIN, "daily")).toBe(true);
    expect(backupPeriodOver(made, made + 6 * DAY, "weekly")).toBe(false);
    expect(backupPeriodOver(made, made + 7 * DAY - 15 * MIN, "weekly")).toBe(true);
    expect(backupPeriodOver(made, made - 2 * DAY, "daily")).toBe(true);
    expect(backupPeriodOver(made, made - 5 * MIN, "daily")).toBe(false);
  });

  it("are pruned oldest first beyond the newest keep, and nothing else is ever listed", () => {
    const names = [
      "atomik-kb-auto-2026-10-12-0905.atomikkb",
      "notes.txt",
      "atomik-kb-auto-2026-10-10-0905.atomikkb",
      "atomik-kb-2026-10-01.atomikkb",
      "atomik-kb-auto-2026-10-11-0905.atomikkb",
      "atomik-kb-auto-2025-12-31-2359.atomikkb",
    ];
    expect(backupsToPrune(names, 2)).toEqual([
      "atomik-kb-auto-2025-12-31-2359.atomikkb",
      "atomik-kb-auto-2026-10-10-0905.atomikkb",
    ]);
    expect(backupsToPrune(names, 4)).toEqual([]);
    expect(backupsToPrune(names, 100)).toEqual([]);
    expect(backupsToPrune(["notes.txt"], 1)).toEqual([]);
    expect(backupsToPrune([], 1)).toEqual([]);
  });

  it("read the schedule with defaults and a keep of 1 to 100", () => {
    expect(keepOf(undefined)).toBe(DEFAULT_BACKUP_KEEP);
    expect(keepOf({ keep: 3 })).toBe(3);
    expect(keepOf({ keep: 0 })).toBe(1);
    expect(keepOf({ keep: 1000 })).toBe(100);
    expect(keepOf({ keep: "x" })).toBe(7);
    expect(keepOf([1])).toBe(7);
    expect(scheduleOf(undefined)).toEqual({ enabled: false, interval: "daily", keep: 7 });
    expect(scheduleOf({ enabled: true, interval: "weekly", keep: 2 })).toEqual({ enabled: true, interval: "weekly", keep: 2 });
  });
});

function setup(options: MemoryTransportOptions = {}) {
  const clock = { now: local(2026, 10, 12, 9, 0) };
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
    now: () => clock.now,
    workspaceName: "Workspace",
    spaces: [{ name: "Home", pages: [{ title: "A" }] }],
    security: { kitCreatedAt: clock.now, kitVerifiedAt: clock.now, kitKey: KEY },
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events, clock };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

const status = async (client: CoreClient): Promise<BackupRunStatus> => (await client.settingsGet("app.backup.last")) as unknown as BackupRunStatus;

describe("backupFolderChoose", () => {
  it("picks the folder, stores it in the setting only the shell writes, and announces it", async () => {
    const { client, events } = setup();
    expect(await client.backupFolderChoose()).toBe("/backups/scheduled");
    expect(await client.settingsGet("app.backup.folder")).toBe("/backups/scheduled");
    expect(events).toEqual([{ type: "SettingChanged", data: { key: "app.backup.folder" } }]);
  });

  it("is a cancelled dialog like the others: InvalidInput for path, nothing stored", async () => {
    const { client, transport } = setup({ cancelDialogs: true });
    const error = await failure(client.backupFolderChoose());
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "path" });
    expect(await client.settingsGet("app.backup.folder")).toBeNull();
    transport.setCancelDialogs(false);
    expect(await client.backupFolderChoose()).toBe("/backups/scheduled");
  });

  it("needs the core to be ready", async () => {
    const { client, transport } = setup();
    transport.setCoreState("locked");
    expect((await failure(client.backupFolderChoose())).code).toBe("Locked");
  });
});

describe("the settings of scheduled backups", () => {
  it("refuses the folder and the last result from the webview, and takes the schedule", async () => {
    const { client } = setup();
    for (const key of ["app.backup.folder", "app.backup.last"]) {
      const error = await failure(client.settingsSet(key, "x"));
      expect(error.code).toBe("InvalidInput");
      expect(error.details).toEqual({ field: "key" });
    }
    await client.settingsSet("app.backup.schedule", { enabled: true, interval: "daily", keep: 3 });
    expect(await client.settingsGet("app.backup.schedule")).toEqual({ enabled: true, interval: "daily", keep: 3 });
  });

  it("can start with them set", async () => {
    const { client } = setup({ settings: { "app.backup.folder": "/mnt/backups", "app.backup.last": { at: 1, ok: true } } });
    expect(await client.settingsGet("app.backup.folder")).toBe("/mnt/backups");
  });
});

describe("backupRunNow", () => {
  it("writes a backup named after the local time into the folder and records the outcome", async () => {
    const { client, transport, events, clock } = setup();
    await client.backupFolderChoose();
    events.length = 0;
    const result = await client.backupRunNow();
    const file = "atomik-kb-auto-2026-10-12-0900.atomikkb";
    expect(result.path).toBe(`/backups/scheduled/${file}`);
    expect(transport.files.has(result.path)).toBe(true);
    expect(result.summary.bytes).toBe(transport.files.get(result.path)?.length);
    // The structure document of the one space.
    expect(result.summary.documents).toBe(1);
    expect(await status(client)).toEqual({ at: clock.now, ok: true, lastSuccessAt: clock.now, file });
    expect(events.filter((event) => event.type === "SettingChanged")).toEqual([{ type: "SettingChanged", data: { key: "app.backup.last" } }]);
  });

  it("is a backup another installation restores with the kit", async () => {
    const { client, transport } = setup();
    await client.backupFolderChoose();
    const { path, summary } = await client.backupRunNow();
    const other = new MemoryTransport({ files: transport.files });
    const restored = await other.backupImport(path, (await kitWordsFromEntropy(KEY)).join(" "));
    expect(restored.summary).toEqual(summary);
    expect((await new CoreClient(other).spaceList()).map((space) => space.name)).toEqual(["Home"]);
  });

  it("keeps the newest keep scheduled backups of the schedule (7 unless it says otherwise) and touches nothing else", async () => {
    const { client, transport, clock } = setup();
    await client.backupFolderChoose();
    await client.settingsSet("app.backup.schedule", { enabled: true, interval: "daily", keep: 3 });
    const keepSafe = [
      "/backups/scheduled/notes.txt",
      "/backups/scheduled/atomik-kb-2026-10-01.atomikkb",
      "/backups/scheduled/atomik-kb-auto-2026-10-12-0900.atomikkb.bak",
      "/backups/scheduled/sub/atomik-kb-auto-2020-01-01-0000.atomikkb",
      "/backups/other/atomik-kb-auto-2020-01-01-0000.atomikkb",
    ];
    for (const path of keepSafe) {
      transport.files.set(path, new Uint8Array([1]));
    }
    // An old scheduled backup that the first run has to prune.
    transport.files.set("/backups/scheduled/atomik-kb-auto-2026-10-01-0900.atomikkb", new Uint8Array([2]));
    for (let run = 0; run < 5; run += 1) {
      await client.backupRunNow();
      clock.now += 90 * MINUTE;
    }
    const names = [...transport.files.keys()].filter((path) => path.startsWith("/backups/scheduled/") && !path.slice(19).includes("/")).map((path) => path.slice(19)).sort();
    expect(names.filter(isAutoBackupName)).toEqual([
      "atomik-kb-auto-2026-10-12-1200.atomikkb",
      "atomik-kb-auto-2026-10-12-1330.atomikkb",
      "atomik-kb-auto-2026-10-12-1500.atomikkb",
    ]);
    for (const path of keepSafe) {
      expect(transport.files.has(path), path).toBe(true);
    }
    expect(transport.files.has("/backups/scheduled/atomik-kb-auto-2026-10-01-0900.atomikkb")).toBe(false);
  });

  it("keeps 7 by default", async () => {
    const { client, transport, clock } = setup();
    await client.backupFolderChoose();
    for (let run = 0; run < 9; run += 1) {
      await client.backupRunNow();
      clock.now += MINUTE;
    }
    const kept = [...transport.files.keys()].map((path) => path.slice("/backups/scheduled/".length)).filter(isAutoBackupName).sort();
    expect(kept).toEqual(["0902", "0903", "0904", "0905", "0906", "0907", "0908"].map((time) => `atomik-kb-auto-2026-10-12-${time}.atomikkb`));
  });

  it("writes one file for a minute, whatever the number of runs in it", async () => {
    const { client, transport } = setup();
    await client.backupFolderChoose();
    await client.backupRunNow();
    await client.backupRunNow();
    expect([...transport.files.keys()]).toEqual(["/backups/scheduled/atomik-kb-auto-2026-10-12-0900.atomikkb"]);
  });

  it("without a folder is a failed attempt: InvalidInput for folder, recorded with its code and message", async () => {
    const { client, clock } = setup();
    const error = await failure(client.backupRunNow());
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "folder" });
    expect(await status(client)).toEqual({ at: clock.now, ok: false, error: `InvalidInput: ${error.message}` });
  });

  it("without a recovery kit fails like the manual backup, recorded as a failure", async () => {
    const { client, clock } = setup({ security: {} });
    await client.backupFolderChoose();
    const error = await failure(client.backupRunNow());
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "kit" });
    expect(await status(client)).toEqual({ at: clock.now, ok: false, error: `InvalidInput: ${error.message}` });
  });

  it("keeps the last success in the status of a failure", async () => {
    const earlier = local(2026, 10, 11, 9, 0);
    const { client, clock } = setup({
      // A folder that cannot be written to (not absolute), and the result of a run before.
      settings: {
        "app.backup.folder": "relative/folder",
        "app.backup.last": { at: earlier, ok: true, lastSuccessAt: earlier, file: "atomik-kb-auto-2026-10-11-0900.atomikkb" },
      },
    });
    const error = await failure(client.backupRunNow());
    expect(error.details).toEqual({ field: "path" });
    expect(await status(client)).toEqual({
      at: clock.now,
      ok: false,
      lastSuccessAt: earlier,
      file: "atomik-kb-auto-2026-10-11-0900.atomikkb",
      error: `InvalidInput: ${error.message}`,
    });
  });

  it("needs the core to be ready", async () => {
    const { client, transport } = setup();
    transport.setCoreState("locked");
    expect((await failure(client.backupRunNow())).code).toBe("Locked");
  });
});

describe("saveTextFile", () => {
  it("writes the text to the file the save dialog proposes, in /downloads", async () => {
    const { client, transport } = setup();
    const path = await client.saveTextFile("Tasks.csv", "\u{FEFF}Name\r\nWrite\r\n");
    expect(path).toBe("/downloads/Tasks.csv");
    expect(new TextDecoder("utf-8", { ignoreBOM: true }).decode(transport.files.get(path))).toBe("\u{FEFF}Name\r\nWrite\r\n");
    // Written again, it is the file the open dialog picks.
    await client.saveTextFile("Other.csv", "x");
    await client.saveTextFile("Tasks.csv", "y");
    expect([...transport.files.keys()].at(-1)).toBe("/downloads/Tasks.csv");
  });

  it("makes the suggested name safe for a path", async () => {
    const { client } = setup();
    expect(await client.saveTextFile("a/b:c.csv", "x")).toBe("/downloads/ab_c.csv");
    expect(await client.saveTextFile("CON.csv", "x")).toBe("/downloads/_CON.csv");
    expect(await client.saveTextFile("///", "x")).toBe("/downloads/Untitled");
  });

  it("is a cancelled dialog: InvalidInput for path, nothing written", async () => {
    const { client, transport } = setup({ cancelDialogs: true });
    const error = await failure(client.saveTextFile("Tasks.csv", "x"));
    expect(error.details).toEqual({ field: "path" });
    expect(transport.files.size).toBe(0);
  });

  it("refuses text above 64 MiB", async () => {
    const { client, transport } = setup();
    const error = await failure(client.saveTextFile("big.csv", "x".repeat(64 * 1024 * 1024 + 1)));
    expect(error.details).toEqual({ field: "text", limit: 64 * 1024 * 1024 });
    expect(transport.files.size).toBe(0);
    expect(await client.saveTextFile("fits.csv", "x".repeat(64 * 1024 * 1024))).toBe("/downloads/fits.csv");
  });
});
