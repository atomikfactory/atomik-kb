// The restore wizard's folder listing and the scheduled run with Google Drive as a second target, on the in-memory
// transport (ADR-0059): `backupFolderList` over the fake disk, `backupRunNow` with a folder, with Drive alone and with both,
// and the device-bound settings that no backup carries.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { sha256, toHex } from "./digest";
import { driveReasonOf } from "./drive";
import type { MemoryDriveOptions } from "./drive-memory";
import { CoreError } from "./errors";
import type { BackupRunStatus } from "./generated/BackupRunStatus";
import type { DriveConnection } from "./generated/DriveConnection";
import type { Event } from "./generated/Event";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { isAutoBackupName } from "./memory-backup-schedule";

const MINUTE = 60_000;
const DAY = 86_400_000;
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index + 5);

/** A local time: the names of scheduled backups are in local time, whatever the zone of the machine. */
const local = (year: number, month: number, day: number, hour: number, minute: number): number =>
  new Date(year, month - 1, day, hour, minute).getTime();

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

/** An installation with the fake Drive, connected unless told otherwise. */
function withDrive(drive: MemoryDriveOptions = {}, options: MemoryTransportOptions = {}) {
  const installation = setup({ drive: { connected: true, ...drive }, ...options });
  const fake = installation.transport.drive;
  if (fake === undefined) {
    throw new Error("the transport has no drive");
  }
  return { ...installation, drive: fake };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

const lastOf = async (client: CoreClient): Promise<BackupRunStatus> => (await client.settingsGet("app.backup.last")) as unknown as BackupRunStatus;
const driveOf = async (client: CoreClient): Promise<DriveConnection> => (await client.settingsGet("app.backup.drive")) as unknown as DriveConnection;
const words = async (): Promise<string> => (await kitWordsFromEntropy(KEY)).join(" ");

const text = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);

/** Re-encodes a backup with `edit` applied to its state, with a valid checksum. */
async function reseal(bytes: Uint8Array, edit: (state: { settings: { key: string; value: unknown }[] }) => void): Promise<Uint8Array> {
  const [first = "", headerLine = "", body = ""] = text(bytes).split("\n");
  const state = JSON.parse(text(Uint8Array.from(atob(body), (char) => char.charCodeAt(0)))) as { settings: { key: string; value: unknown }[] };
  edit(state);
  const encoded = btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(state))));
  const signed = `${first}\n${headerLine}\n${encoded}\n`;
  return new TextEncoder().encode(`${signed}${toHex(await sha256(signed))}\n`);
}

describe("backupFolderList", () => {
  it("lists the backups directly in the folder, newest first, and each id restores", async () => {
    const origin = setup();
    await origin.client.backupExport("/backups/old.atomikkb");
    origin.clock.now += DAY;
    await origin.client.backupExport("/backups/new.atomikkb");
    origin.clock.now += DAY;
    await origin.client.backupExport("/backups/ARCHIVE.ATOMIKKB");
    // Not backups of the folder: another extension, a sub-folder, another folder, a name that only starts alike.
    for (const other of ["/backups/notes.txt", "/backups/sub/deep.atomikkb", "/backups2/x.atomikkb", "/backups/x.atomikkb.bak"]) {
      origin.transport.files.set(other, new Uint8Array([1]));
    }

    const listing = await origin.client.backupFolderList("/backups");

    expect(listing.folder).toBe("/backups");
    expect(listing.backups.map((file) => file.name)).toEqual(["ARCHIVE.ATOMIKKB", "new.atomikkb", "old.atomikkb"]);
    expect(listing.backups.map((file) => file.id)).toEqual(["/backups/ARCHIVE.ATOMIKKB", "/backups/new.atomikkb", "/backups/old.atomikkb"]);
    const [newest, , oldest] = listing.backups;
    expect(newest?.size).toBe(origin.transport.files.get("/backups/ARCHIVE.ATOMIKKB")?.length);
    expect(newest?.modifiedAt).toBe(local(2026, 10, 14, 9, 0));
    expect(oldest?.modifiedAt).toBe(local(2026, 10, 12, 9, 0));

    // The id is what `backupImport` takes: restored into a fresh installation sharing the disk.
    const fresh = new MemoryTransport({ files: origin.transport.files });
    const restored = await fresh.backupImport(newest?.id, await words());
    expect(restored.path).toBe("/backups/ARCHIVE.ATOMIKKB");
  });

  it("without a path asks the folder dialog, which picks /backups/scheduled unless told otherwise", async () => {
    const { client, transport } = setup();
    await client.backupExport("/backups/scheduled/atomik-kb-auto-2026-10-12-0900.atomikkb");
    await client.backupExport("/backups/elsewhere.atomikkb");
    const listing = await client.backupFolderList();
    expect(listing.folder).toBe("/backups/scheduled");
    expect(listing.backups.map((file) => file.name)).toEqual(["atomik-kb-auto-2026-10-12-0900.atomikkb"]);

    const chosen = setup({ dialogFolder: "/mnt/usb", files: transport.files });
    await chosen.client.backupExport("/mnt/usb/mine.atomikkb");
    expect((await chosen.client.backupFolderList()).backups.map((file) => file.name)).toEqual(["mine.atomikkb"]);
    expect(await chosen.client.backupFolderChoose()).toBe("/mnt/usb");
  });

  it("is a cancelled dialog like the others: InvalidInput for path, saying no folder was chosen", async () => {
    const { client, transport } = setup({ cancelDialogs: true });
    const error = await failure(client.backupFolderList());
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "path" });
    expect(error.message).toMatch(/^no (file|folder) was chosen/iu);
    // An explicit path needs no dialog.
    expect((await client.backupFolderList("/backups")).backups).toEqual([]);
    transport.setCancelDialogs(false);
    expect((await client.backupFolderList()).folder).toBe("/backups/scheduled");
  });

  it("wants an absolute path, and says so in words that are not a cancellation", async () => {
    const { client } = setup();
    for (const relative of ["backups", "./backups", "C:backups", ""]) {
      const error = await failure(client.backupFolderList(relative));
      expect(error.code, relative).toBe("InvalidInput");
      expect(error.details, relative).toEqual({ field: "path" });
      expect(error.message, relative).not.toMatch(/^no (file|folder) was chosen/iu);
    }
  });

  it("reads Windows paths and trailing separators, and a folder with no backups lists none", async () => {
    const { client, transport } = setup();
    await client.backupExport("C:\\backups\\a.atomikkb");
    await client.backupExport("C:/backups/b.atomikkb");
    const named = async (folder: string) => {
      const listing = await client.backupFolderList(folder);
      return [listing.folder, listing.backups.map((file) => file.name)];
    };
    expect(await named("C:\\backups")).toEqual(["C:\\backups", ["a.atomikkb"]]);
    expect(await named("C:\\backups\\")).toEqual(["C:\\backups", ["a.atomikkb"]]);
    expect(await named("C:/backups/")).toEqual(["C:/backups", ["b.atomikkb"]]);
    expect(await named("C:/backups")).toEqual(["C:/backups", ["b.atomikkb"]]);
    expect(await client.backupFolderList("/nothing/here")).toEqual({ folder: "/nothing/here", backups: [] });
    expect(transport.files.size).toBe(2);
  });

  it("lists a file that is not a backup, as the oldest; the restore says what is wrong", async () => {
    const { client, transport } = setup();
    await client.backupExport("/backups/real.atomikkb");
    transport.files.set("/backups/junk.atomikkb", new Uint8Array([1, 2, 3]));
    const listing = await client.backupFolderList("/backups");
    expect(listing.backups.map((file) => file.name)).toEqual(["real.atomikkb", "junk.atomikkb"]);
    expect(listing.backups[1]).toMatchObject({ size: 3, modifiedAt: 0 });
    const fresh = new MemoryTransport({ files: transport.files });
    expect((await failure(fresh.backupImport("/backups/junk.atomikkb", await words()))).code).toBe("Crypto");
  });

  it("lists at most 200", async () => {
    const { client, transport } = setup();
    for (let index = 0; index < 205; index += 1) {
      transport.files.set(`/many/b${String(index).padStart(3, "0")}.atomikkb`, new Uint8Array([1]));
    }
    expect((await client.backupFolderList("/many")).backups).toHaveLength(200);
  });

  it("needs the core to be ready, but not a workspace: the wizard lists before there is one", async () => {
    const { client, transport } = setup({ workspaceName: undefined, spaces: undefined, security: undefined });
    expect((await client.backupFolderList("/backups")).backups).toEqual([]);
    transport.setCoreState("locked");
    expect((await failure(client.backupFolderList("/backups"))).code).toBe("Locked");
  });
});

describe("the scheduled run with Drive", () => {
  it("with a folder and Drive: one export, in the folder and in Drive, the same bytes", async () => {
    const { client, transport, drive, clock } = withDrive();
    await client.backupFolderChoose();
    const result = await client.backupRunNow();
    const file = "atomik-kb-auto-2026-10-12-0900.atomikkb";

    expect(result.path).toBe(`/backups/scheduled/${file}`);
    expect(transport.files.has(result.path)).toBe(true);
    const [uploaded] = drive.backups;
    expect(uploaded?.name).toBe(file);
    const folderBytes = transport.files.get(result.path);
    expect(uploaded?.size).toBe(folderBytes?.length);
    const staged = await client.drive().download(uploaded?.id ?? "", () => undefined);
    expect(transport.files.get(staged)).toEqual(folderBytes);
    // Nothing is left of a staging file, and both outcomes are recorded.
    expect([...transport.files.keys()].filter((path) => path.startsWith("/backup-staging"))).toEqual([]);
    expect(await lastOf(client)).toEqual({ at: clock.now, ok: true, lastSuccessAt: clock.now, file });
    expect((await driveOf(client)).last).toEqual({ at: clock.now, ok: true, lastSuccessAt: clock.now, file });
  });

  it("with Drive alone (no folder): the export is staged, delivered and the staging file deleted", async () => {
    const { client, transport, drive, events, clock } = withDrive();
    events.length = 0;
    const result = await client.backupRunNow();
    const file = "atomik-kb-auto-2026-10-12-0900.atomikkb";

    // The staging file's path is the answer; the file itself is gone.
    expect(result.path).toBe(`/backup-staging/${file}`);
    expect(transport.files.has(result.path)).toBe(false);
    expect(transport.files.size).toBe(0);
    expect(drive.backups.map((entry) => entry.name)).toEqual([file]);
    expect(result.summary.bytes).toBe(drive.backups[0]?.size);
    expect(await lastOf(client)).toEqual({ at: clock.now, ok: true, lastSuccessAt: clock.now, file });
    const changed = events.filter((event) => event.type === "SettingChanged").map((event) => (event.type === "SettingChanged" ? event.data.key : ""));
    expect(changed.sort()).toEqual(["app.backup.drive", "app.backup.last"]);
  });

  it("Drive alone is a backup another installation restores with the kit, from the file downloaded back", async () => {
    const { client, transport, drive } = withDrive({ stepMs: 0 });
    await client.backupRunNow();
    const [uploaded] = drive.backups;
    const staged = await client.drive().download(uploaded?.id ?? "", () => undefined);
    const fresh = new MemoryTransport({ files: transport.files });
    const restored = await fresh.backupImport(staged, await words());
    expect(restored.path).toBe(staged);
    expect((await new CoreClient(fresh).spaceList()).map((space) => space.name)).toEqual(["Home"]);
  });

  it("a Drive that is not connected is not a target: no folder either is a failed attempt, as before", async () => {
    const { client, clock } = setup({ drive: "fake" });
    const error = await failure(client.backupRunNow());
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "folder" });
    expect(await lastOf(client)).toEqual({ at: clock.now, ok: false, error: `InvalidInput: ${error.message}` });
    expect(await client.settingsGet("app.backup.drive")).toBeNull();
  });

  it("with only a folder, a transport that has Drive but is not connected writes the folder alone", async () => {
    const { client, transport } = setup({ drive: "fake" });
    await client.backupFolderChoose();
    await client.backupRunNow();
    expect([...transport.files.keys()]).toEqual(["/backups/scheduled/atomik-kb-auto-2026-10-12-0900.atomikkb"]);
    expect(transport.drive?.backups).toEqual([]);
    expect(await client.settingsGet("app.backup.drive")).toBeNull();
  });

  it("each target keeps the newest keep scheduled backups of its own", async () => {
    const { client, transport, drive, clock } = withDrive();
    await client.backupFolderChoose();
    await client.settingsSet("app.backup.schedule", { enabled: true, interval: "daily", keep: 2 });
    transport.files.set("/backups/scheduled/notes.txt", new Uint8Array([1]));
    for (let run = 0; run < 4; run += 1) {
      await client.backupRunNow();
      clock.now += 90 * MINUTE;
    }
    const names = (path: string): string[] => [...transport.files.keys()].filter((key) => key.startsWith(path)).map((key) => key.slice(path.length));
    expect(names("/backups/scheduled/").filter(isAutoBackupName).sort()).toEqual([
      "atomik-kb-auto-2026-10-12-1200.atomikkb",
      "atomik-kb-auto-2026-10-12-1330.atomikkb",
    ]);
    expect(transport.files.has("/backups/scheduled/notes.txt")).toBe(true);
    expect(drive.backups.map((file) => file.name).sort()).toEqual(["atomik-kb-auto-2026-10-12-1200.atomikkb", "atomik-kb-auto-2026-10-12-1330.atomikkb"]);
  });

  it("a failed delivery fails the run: the folder keeps its copy, the error is Drive's, the last success's date is kept", async () => {
    const { client, transport, drive, clock } = withDrive();
    await client.backupFolderChoose();
    await client.backupRunNow();
    const first = clock.now;
    clock.now += DAY;
    drive.setFailure("deliver", "quota");

    const error = await failure(client.backupRunNow());

    expect(driveReasonOf(error)).toBe("quota");
    const second = "atomik-kb-auto-2026-10-13-0900.atomikkb";
    expect(transport.files.has(`/backups/scheduled/${second}`)).toBe(true);
    expect(await lastOf(client)).toEqual({
      at: clock.now,
      ok: false,
      lastSuccessAt: first,
      // The folder's file of this run, which the retry delivers to Drive again.
      file: second,
      error: `${error.code}: ${error.message}`,
    });
    expect((await driveOf(client)).last).toMatchObject({ ok: false, error: `${error.code}: ${error.message}`, lastSuccessAt: first });
    expect(drive.backups.map((file) => file.name)).toEqual(["atomik-kb-auto-2026-10-12-0900.atomikkb"]);
  });

  it("a failed delivery with Drive alone leaves no staging file, and a refused token asks to reconnect", async () => {
    const { client, transport, clock } = withDrive({ fail: { deliver: "reconnect" } });
    const error = await failure(client.backupRunNow());
    expect(driveReasonOf(error)).toBe("reconnect");
    expect(transport.files.size).toBe(0);
    expect(await lastOf(client)).toEqual({ at: clock.now, ok: false, error: `${error.code}: ${error.message}` });
    expect(await client.drive().status()).toMatchObject({ connected: true, needsReconnect: true });
  });

  it("a failed export is no delivery: no kit is InvalidInput for kit, Drive is not asked, no staging file is left", async () => {
    const { client, transport, drive } = withDrive({}, { security: {} });
    const error = await failure(client.backupRunNow());
    expect(error.details).toEqual({ field: "kit" });
    expect(drive.backups).toEqual([]);
    expect(transport.files.size).toBe(0);
    expect(await client.settingsGet("app.backup.drive")).toMatchObject({ needsReconnect: false });
    expect((await driveOf(client)).last).toBeUndefined();
  });

  it("needs the core to be ready", async () => {
    const { client, transport } = withDrive();
    transport.setCoreState("locked");
    expect((await failure(client.backupRunNow())).code).toBe("Locked");
  });
});

describe("the retry after a Drive failure", () => {
  const dir = "/backups/scheduled/";
  const old10 = "atomik-kb-auto-2026-10-10-0900.atomikkb";
  const old11 = "atomik-kb-auto-2026-10-11-0900.atomikkb";
  const first = "atomik-kb-auto-2026-10-12-0900.atomikkb";

  /** A folder with two older scheduled backups, room for three, and a Drive that fails to take what is delivered. */
  async function failing(failWith: "quota" | undefined = "quota") {
    const installation = withDrive();
    const { client, transport, drive } = installation;
    await client.backupFolderChoose();
    await client.settingsSet("app.backup.schedule", { enabled: true, interval: "daily", keep: 3 });
    transport.files.set(`${dir}${old10}`, new Uint8Array([10]));
    transport.files.set(`${dir}${old11}`, new Uint8Array([11]));
    drive.setFailure("deliver", failWith);
    return installation;
  }

  const folderNames = (transport: MemoryTransport): string[] =>
    [...transport.files.keys()]
      .filter((path) => path.startsWith(dir))
      .map((path) => path.slice(dir.length))
      .sort();

  it("never costs the folder its older backups: eight failing retries export nothing and prune nothing", async () => {
    const { client, transport, drive, clock } = await failing();

    expect(driveReasonOf(await failure(client.backupRunNow()))).toBe("quota");
    expect(folderNames(transport)).toEqual([old10, old11, first]);
    const written = transport.files.get(`${dir}${first}`);
    expect(written).toBeDefined();

    for (let retry = 0; retry < 8; retry += 1) {
      clock.now += 61 * MINUTE;
      const error = await failure(transport.runScheduledBackup());
      expect(driveReasonOf(error)).toBe("quota");
      expect(folderNames(transport)).toEqual([old10, old11, first]);
      // The very file of the first run: not exported again.
      expect(transport.files.get(`${dir}${first}`)).toBe(written);
      expect(await lastOf(client)).toEqual({
        at: clock.now,
        ok: false,
        file: first,
        error: `${error.code}: ${error.message}`,
      });
    }
    expect(drive.backups).toEqual([]);
    expect([...transport.files.keys()].filter((path) => path.startsWith("/backup-staging"))).toEqual([]);
  });

  it("delivers the same file under the same name once Drive is back, dating the run by that backup", async () => {
    const { client, transport, drive, clock } = await failing();
    const started = clock.now;
    await failure(client.backupRunNow());
    const bytes = transport.files.get(`${dir}${first}`);
    clock.now += 3 * 60 * MINUTE;
    drive.setFailure("deliver", undefined);

    const result = await transport.runScheduledBackup();

    // The same name and the same bytes, from the folder's file; the folder is as it was.
    expect(result.path).toBe(`${dir}${first}`);
    expect(drive.backups.map((file) => file.name)).toEqual([first]);
    const staged = await client.drive().download(drive.backups[0]?.id ?? "", () => undefined);
    expect(transport.files.get(staged)).toEqual(bytes);
    expect(folderNames(transport)).toEqual([old10, old11, first]);
    expect(result.summary.bytes).toBe(bytes?.length);
    // `ok` again: the last run, dated by the backup it delivered, naming the folder's file.
    expect(await lastOf(client)).toEqual({ at: clock.now, ok: true, lastSuccessAt: started, file: first });
    expect((await driveOf(client)).last).toMatchObject({ ok: true, file: first });
  });

  it("a new period makes a normal export for both targets, pruning the folder as usual", async () => {
    const { client, transport, drive, clock } = await failing();
    await failure(client.backupRunNow());
    drive.setFailure("deliver", undefined);
    clock.now += DAY - 14 * MINUTE;

    const result = await transport.runScheduledBackup();

    const second = "atomik-kb-auto-2026-10-13-0846.atomikkb";
    expect(result.path).toBe(`${dir}${second}`);
    expect(folderNames(transport)).toEqual([old11, first, second]);
    expect(drive.backups.map((file) => file.name)).toEqual([second]);
    expect((await lastOf(client)).file).toBe(second);
  });

  it("a file that is gone, or that Drive already has, is not delivered again: a normal export", async () => {
    for (const driveHasIt of [false, true]) {
      const { client, transport, drive, clock } = await failing();
      await failure(client.backupRunNow());
      drive.setFailure("deliver", undefined);
      if (driveHasIt) {
        await drive.deliver(first, new Uint8Array([1]), 7);
      } else {
        transport.files.delete(`${dir}${first}`);
      }
      clock.now += 61 * MINUTE;

      const result = await transport.runScheduledBackup();

      const second = "atomik-kb-auto-2026-10-12-1001.atomikkb";
      expect(result.path, `drive has it: ${String(driveHasIt)}`).toBe(`${dir}${second}`);
      expect(drive.backups.map((file) => file.name).sort()).toEqual(driveHasIt ? [first, second] : [second]);
    }
  });

  it("with Drive alone there is nothing to protect: a retry exports again into staging and names no file", async () => {
    const { client, transport, drive, clock } = withDrive({ fail: { deliver: "quota" } });
    await failure(client.backupRunNow());
    expect((await lastOf(client)).file).toBeUndefined();
    drive.setFailure("deliver", undefined);
    clock.now += 61 * MINUTE;

    const result = await transport.runScheduledBackup();

    // A new export, under the name of its own time, and its staging file is gone.
    expect(result.path).toBe("/backup-staging/atomik-kb-auto-2026-10-12-1001.atomikkb");
    expect(drive.backups.map((file) => file.name)).toEqual(["atomik-kb-auto-2026-10-12-1001.atomikkb"]);
    expect(transport.files.size).toBe(0);
  });

  it("a person's press of Back up now after a Drive failure in the same period exports a new file", async () => {
    const { client, transport, drive, clock } = await failing();
    await failure(client.backupRunNow());
    drive.setFailure("deliver", undefined);
    clock.now += 5 * MINUTE;

    const result = await client.backupRunNow();

    // Fresh: not the folder's file again, whatever the timer would retry.
    const second = "atomik-kb-auto-2026-10-12-0905.atomikkb";
    expect(result.path).toBe(`${dir}${second}`);
    expect(folderNames(transport)).toEqual([old11, first, second]);
    expect(drive.backups.map((file) => file.name)).toEqual([second]);
    expect(await lastOf(client)).toEqual({ at: clock.now, ok: true, lastSuccessAt: clock.now, file: second });
  });
});

describe("device-bound settings and backups", () => {
  it("a backup leaves out the folder, the last outcome and the Drive connection", async () => {
    const { client, transport } = withDrive();
    await client.settingsSet("ui.theme", "dark");
    await client.backupFolderChoose();
    await client.backupRunNow();
    const { path, summary } = await client.backupExport("/backups/manual.atomikkb");
    // `ui.theme` only: the folder, the last result and the connection (with its last upload) stay on this device.
    expect(summary.settings).toBe(1);

    const fresh = new MemoryTransport({ files: transport.files });
    await fresh.backupImport(path, await words());
    const restored = new CoreClient(fresh);
    expect(await restored.settingsGet("ui.theme")).toBe("dark");
    for (const key of ["app.backup.folder", "app.backup.last", "app.backup.drive"]) {
      expect(await restored.settingsGet(key), key).toBeNull();
    }
  });

  it("a crafted backup does not bring another device's folder, last outcome or connection", async () => {
    const { client, transport } = setup();
    await client.settingsSet("ui.theme", "dark");
    const { path } = await client.backupExport("/backups/manual.atomikkb");
    const bytes = transport.files.get(path) ?? new Uint8Array();
    const crafted = await reseal(bytes, (state) => {
      state.settings.push(
        { key: "app.backup.folder", value: "/somewhere/else" },
        { key: "app.backup.last", value: { at: 1, ok: true } },
        { key: "app.backup.drive", value: { account: "x@y.z", folderId: "f", connectedAt: 1, needsReconnect: false } },
        { key: "app.ai.endpoint", value: "http://example.invalid" },
      );
    });
    transport.files.set("/backups/crafted.atomikkb", crafted);

    const fresh = new MemoryTransport({ files: transport.files, drive: "fake" });
    await fresh.backupImport("/backups/crafted.atomikkb", await words());
    const restored = new CoreClient(fresh);
    expect(await restored.settingsGet("ui.theme")).toBe("dark");
    for (const key of ["app.backup.folder", "app.backup.last", "app.backup.drive", "app.ai.endpoint"]) {
      expect(await restored.settingsGet(key), key).toBeNull();
    }
    expect(await restored.drive().status()).toMatchObject({ connected: false });
  });
});
