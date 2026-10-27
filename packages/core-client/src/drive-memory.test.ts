// The fake Google Drive (ADR-0059, `./drive-memory`): the consent, the sign-in that can be cancelled, the failures with their
// reasons, the list and the download into the fake disk of a `MemoryTransport` (restorable with `backupImport`), the
// connection kept in `app.backup.drive` (device-bound, shell-owned), the delivery of a scheduled backup.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { DRIVE_ERROR_REASONS, driveReasonOf } from "./drive";
import type { DriveProgress } from "./drive";
import {
  FAKE_DRIVE_ACCOUNT,
  FAKE_DRIVE_FOLDER_ID,
  FAKE_DRIVE_FOLDER_URL,
  FAKE_RESTORE_STAGING,
  MemoryDrive,
  driveFailure,
} from "./drive-memory";
import type { MemoryDriveOptions } from "./drive-memory";
import { CoreError } from "./errors";
import type { BackupRunStatus } from "./generated/BackupRunStatus";
import type { DriveConnection } from "./generated/DriveConnection";
import type { Event } from "./generated/Event";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";

const START = 1_700_000_000_000;
const DAY = 86_400_000;
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index + 9);
const SETTING = "app.backup.drive";

function setup(drive: MemoryDriveOptions | "fake" = "fake", options: MemoryTransportOptions = {}) {
  let clock = START;
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
    now: () => clock++,
    drive,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const fake = transport.drive;
  if (fake === undefined) {
    throw new Error("the transport has no drive");
  }
  return { transport, client, events, drive: fake };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

const connection = async (client: CoreClient): Promise<DriveConnection> => (await client.settingsGet(SETTING)) as unknown as DriveConnection;

describe("the option", () => {
  it("gives the transport a drive, and the client says so; without it there is none", () => {
    expect(new CoreClient(new MemoryTransport({ drive: "fake" })).hasDrive()).toBe(true);
    expect(new CoreClient(new MemoryTransport({ drive: {} })).hasDrive()).toBe(true);
    const without = new CoreClient(new MemoryTransport());
    expect(without.hasDrive()).toBe(false);
    expect(() => without.drive()).toThrow(CoreError);
  });

  it("works alone, without a transport", async () => {
    const drive = new MemoryDrive();
    expect(await drive.status()).toEqual({ client: "builtIn", connected: false, needsReconnect: false });
    const status = await drive.connect();
    expect(status.connected).toBe(true);
    expect(status.account).toBe(FAKE_DRIVE_ACCOUNT);
  });
});

describe("status and connect", () => {
  it("starts disconnected with the built-in client", async () => {
    const { client, events } = setup();
    expect(await client.drive().status()).toEqual({ client: "builtIn", connected: false, needsReconnect: false });
    expect(await client.settingsGet(SETTING)).toBeNull();
    expect(events).toEqual([]);
  });

  it("connects after the consent: the account, the folder's address, the setting written and announced", async () => {
    const { client, events, drive } = setup();
    const status = await client.drive().connect();
    expect(status).toEqual({
      client: "builtIn",
      connected: true,
      needsReconnect: false,
      account: FAKE_DRIVE_ACCOUNT,
      folderUrl: FAKE_DRIVE_FOLDER_URL,
    });
    expect(await client.drive().status()).toEqual(status);
    expect(drive.connected).toBe(true);
    expect(await connection(client)).toEqual({
      account: FAKE_DRIVE_ACCOUNT,
      folderId: FAKE_DRIVE_FOLDER_ID,
      connectedAt: expect.any(Number) as unknown,
      needsReconnect: false,
    });
    expect(events).toEqual([{ type: "SettingChanged", data: { key: SETTING } }]);
  });

  it("uses the account it was given", async () => {
    const { client } = setup({ account: "someone@example.org" });
    expect((await client.drive().connect()).account).toBe("someone@example.org");
  });

  it("can start connected, without an event", async () => {
    const { client, events, drive } = setup({ connected: true });
    expect(drive.connected).toBe(true);
    expect((await client.drive().status()).account).toBe(FAKE_DRIVE_ACCOUNT);
    expect(await connection(client)).toMatchObject({ account: FAKE_DRIVE_ACCOUNT, folderId: FAKE_DRIVE_FOLDER_ID });
    expect(events).toEqual([]);
  });

  it("is refused when the consent is declined: consentDeclined, nothing stored", async () => {
    const { client, events, drive } = setup({ consent: false });
    const error = await failure(client.drive().connect());
    expect(driveReasonOf(error)).toBe("consentDeclined");
    expect(error.code).toBe("InvalidInput");
    expect((await client.drive().status()).connected).toBe(false);
    expect(await client.settingsGet(SETTING)).toBeNull();
    expect(events).toEqual([]);
    drive.setConsent(true);
    expect((await client.drive().connect()).connected).toBe(true);
    drive.setConsent(false);
    expect(driveReasonOf(await failure(client.drive().connect()))).toBe("consentDeclined");
  });

  it("with no client: the status says none and connecting is noClient, before the consent", async () => {
    const { client } = setup({ noClient: true, consent: false });
    expect(await client.drive().status()).toMatchObject({ client: "none", connected: false });
    expect(driveReasonOf(await failure(client.drive().connect()))).toBe("noClient");
    // One's own client is the way out; it takes the place of the missing one.
    const own = await client.drive().setClient({ clientId: "  my-client.apps.example.com ", clientSecret: "s" });
    expect(own).toMatchObject({ client: "own", ownClientId: "my-client.apps.example.com", connected: false });
    expect(driveReasonOf(await failure(client.drive().connect()))).toBe("consentDeclined");
    expect((await client.drive().setClient(null)).client).toBe("none");
  });

  it("changing the client disconnects, and an empty client id is refused", async () => {
    const { client } = setup();
    await client.drive().connect();
    const changed = await client.drive().setClient({ clientId: "mine" });
    expect(changed.connected).toBe(false);
    expect(changed.ownClientId).toBe("mine");
    expect(await connection(client)).toEqual({ ownClientId: "mine", needsReconnect: false });
    const error = await failure(client.drive().setClient({ clientId: "   " }));
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "clientId" });
    expect((await client.drive().status()).ownClientId).toBe("mine");
    await client.drive().connect();
    const back = await client.drive().setClient(null);
    expect(back).toEqual({ client: "builtIn", connected: false, needsReconnect: false });
  });

  it("fails with any reason of the contract, and stays disconnected", async () => {
    for (const reason of DRIVE_ERROR_REASONS) {
      const { client } = setup({ fail: { connect: reason } });
      const error = await failure(client.drive().connect());
      expect(driveReasonOf(error), reason).toBe(reason);
      expect(error.message.length, reason).toBeGreaterThan(0);
      expect((await client.drive().status()).connected, reason).toBe(false);
      expect(await client.settingsGet(SETTING), reason).toBeNull();
    }
  });

  it("can be made to fail and to work again later", async () => {
    const { client, drive } = setup();
    drive.setFailure("connect", "browserTimeout");
    expect(driveReasonOf(await failure(client.drive().connect()))).toBe("browserTimeout");
    drive.setFailure("connect", undefined);
    expect((await client.drive().connect()).connected).toBe(true);
  });

  it("waits for the browser, and cancel ends it with cancelled", async () => {
    const { client, drive } = setup({ connectMs: Number.POSITIVE_INFINITY });
    const waiting = failure(client.drive().connect());
    await client.drive().cancel();
    const error = await waiting;
    expect(driveReasonOf(error)).toBe("cancelled");
    expect(drive.connected).toBe(false);
    // Nothing waits any more: cancelling again does nothing.
    await client.drive().cancel();
  });

  it("takes the time it was told to", async () => {
    const { client } = setup({ connectMs: 25 });
    const before = performance.now();
    await client.drive().connect();
    expect(performance.now() - before).toBeGreaterThanOrEqual(20);
  });

  it("is not a setting the webview may write", async () => {
    const { client } = setup();
    const error = await failure(client.settingsSet(SETTING, { account: "x" }));
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "key" });
  });
});

describe("disconnect", () => {
  it("forgets the connection, keeps the own client and the last upload, and the files stay in Drive", async () => {
    const last: BackupRunStatus = { at: START, ok: true, lastSuccessAt: START, file: "atomik-kb-auto-2026-10-12-0900.atomikkb" };
    const { client, drive } = setup(
      { backups: [{ name: "a.atomikkb" }] },
      {
        settings: {
          [SETTING]: { account: "x@y.z", folderId: "f", connectedAt: START, ownClientId: "mine", needsReconnect: true, last },
        },
      },
    );
    expect(await client.drive().status()).toMatchObject({ connected: true, needsReconnect: true, ownClientId: "mine", last });
    const status = await client.drive().disconnect();
    expect(status).toEqual({ client: "own", connected: false, needsReconnect: false, ownClientId: "mine", last });
    expect(await connection(client)).toEqual({ ownClientId: "mine", needsReconnect: false, last });
    expect(drive.backups).toHaveLength(1);
    expect(driveReasonOf(await failure(client.drive().listBackups()))).toBe("notConnected");
  });

  it("is fine when nothing is connected", async () => {
    const { client } = setup();
    expect(await client.drive().disconnect()).toEqual({ client: "builtIn", connected: false, needsReconnect: false });
  });
});

describe("a connection that needs reconnecting", () => {
  const stale = { [SETTING]: { account: "x@y.z", folderId: "f", connectedAt: START, needsReconnect: true } };

  it("still reads as connected, refuses to list and to download with reconnect, and the next connect clears it", async () => {
    const { client } = setup({ backups: [{ name: "a.atomikkb" }] }, { settings: stale });
    expect(await client.drive().status()).toMatchObject({ connected: true, needsReconnect: true });
    expect(driveReasonOf(await failure(client.drive().listBackups()))).toBe("reconnect");
    expect(driveReasonOf(await failure(client.drive().download("fake-file-1", () => undefined)))).toBe("reconnect");
    const status = await client.drive().connect();
    expect(status.needsReconnect).toBe(false);
    expect((await client.drive().listBackups()).map((file) => file.name)).toEqual(["a.atomikkb"]);
  });
});

describe("listBackups", () => {
  it("needs a connection", async () => {
    const { client } = setup({ backups: [{ name: "a.atomikkb" }] });
    const error = await failure(client.drive().listBackups());
    expect(driveReasonOf(error)).toBe("notConnected");
  });

  it("lists the backups newest first: the first given is the newest unless the times say otherwise", async () => {
    const { client } = setup({
      connected: true,
      backups: [
        { name: "first.atomikkb", bytes: new Uint8Array(10) },
        { name: "second.atomikkb", size: 2048 },
        { name: "oldest.atomikkb", modifiedAt: START - 99 * DAY },
        { name: "newest.atomikkb", modifiedAt: START + DAY },
      ],
    });
    const list = await client.drive().listBackups();
    expect(list.map((file) => file.name)).toEqual(["newest.atomikkb", "first.atomikkb", "second.atomikkb", "oldest.atomikkb"]);
    expect(list.find((file) => file.name === "first.atomikkb")).toMatchObject({ size: 10 });
    expect(list.find((file) => file.name === "second.atomikkb")).toMatchObject({ size: 2048 });
    expect(new Set(list.map((file) => file.id)).size).toBe(4);
    for (const file of list) {
      expect(file.modifiedAt).toEqual(expect.any(Number));
    }
  });

  it("answers at most 200", async () => {
    const backups = Array.from({ length: 205 }, (_, index) => ({ name: `b${String(index)}.atomikkb` }));
    const { client, drive } = setup({ connected: true, backups });
    expect(await client.drive().listBackups()).toHaveLength(200);
    expect(drive.backups).toHaveLength(205);
  });

  it("fails with the reason it was given", async () => {
    const { client } = setup({ connected: true, fail: { list: "network" } });
    expect(driveReasonOf(await failure(client.drive().listBackups()))).toBe("network");
  });
});

describe("download", () => {
  it("writes the file on the fake disk, with progress that ends complete, and answers its absolute path", async () => {
    const bytes = Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8]);
    const { client, transport } = setup({ connected: true, steps: 4, stepMs: 0, backups: [{ name: "a.atomikkb", bytes }] });
    const [file] = await client.drive().listBackups();
    const progress: DriveProgress[] = [];
    const path = await client.drive().download(file?.id ?? "", (step) => progress.push(step));
    expect(path.startsWith(`${FAKE_RESTORE_STAGING}/`)).toBe(true);
    expect(path.endsWith(".atomikkb")).toBe(true);
    expect(transport.files.get(path)).toEqual(bytes);
    expect(progress).toEqual([
      { received: 2, total: 8 },
      { received: 4, total: 8 },
      { received: 6, total: 8 },
      { received: 8, total: 8 },
    ]);
  });

  it("deletes the previous staging file first, and a bytes function is called when the file is downloaded", async () => {
    let made = 0;
    const { client, transport, drive } = setup({
      connected: true,
      stepMs: 0,
      backups: [
        {
          name: "made.atomikkb",
          bytes: () => {
            made += 1;
            return Promise.resolve(Uint8Array.from([9, 9]));
          },
        },
        { name: "plain.atomikkb" },
      ],
    });
    const [made1, plain] = await client.drive().listBackups();
    expect(made).toBe(0);
    const first = await client.drive().download(made1?.id ?? "", () => undefined);
    expect(made).toBe(1);
    expect(drive.stagedPath).toBe(first);
    expect(transport.files.get(first)).toEqual(Uint8Array.from([9, 9]));
    const second = await client.drive().download(plain?.id ?? "", () => undefined);
    expect(second).not.toBe(first);
    expect(transport.files.has(first)).toBe(false);
    expect(transport.files.has(second)).toBe(true);
    expect(drive.stagedPath).toBe(second);
  });

  it("refuses an unknown file with notFound, and a missing connection with notConnected", async () => {
    const { client, drive } = setup({ connected: true, backups: [{ name: "a.atomikkb" }] });
    expect(driveReasonOf(await failure(client.drive().download("nope", () => undefined)))).toBe("notFound");
    await client.drive().disconnect();
    expect(driveReasonOf(await failure(client.drive().download("fake-file-1", () => undefined)))).toBe("notConnected");
    expect(drive.stagedPath).toBeUndefined();
  });

  it("fails with the reason it was given, and writes nothing", async () => {
    const { client, transport } = setup({ connected: true, fail: { download: "quota" }, backups: [{ name: "a.atomikkb" }] });
    const [file] = await client.drive().listBackups();
    const error = await failure(client.drive().download(file?.id ?? "", () => undefined));
    expect(driveReasonOf(error)).toBe("quota");
    expect(transport.files.size).toBe(0);
  });

  it("is cancelled with cancel: cancelled, and nothing is written", async () => {
    const { client, transport } = setup({ connected: true, stepMs: Number.POSITIVE_INFINITY, backups: [{ name: "a.atomikkb" }] });
    const [file] = await client.drive().listBackups();
    const progress: DriveProgress[] = [];
    const running = failure(client.drive().download(file?.id ?? "", (step) => progress.push(step)));
    await client.drive().cancel();
    expect(driveReasonOf(await running)).toBe("cancelled");
    expect(progress).toEqual([]);
    expect(transport.files.size).toBe(0);
  });

  it("makes a backup that restores into a fresh installation with the kit", async () => {
    // The installation that made the backup.
    const origin = setup("fake", {
      workspaceName: "Research",
      spaces: [{ name: "Notes", pages: [{ title: "Plan" }] }],
      security: { kitCreatedAt: START, kitVerifiedAt: START, kitKey: KEY },
    });
    const { path } = await origin.client.backupExport("/backups/origin.atomikkb");
    const bytes = origin.transport.files.get(path) ?? new Uint8Array();

    // A fresh one, whose Drive holds it: the wizard's way.
    const fresh = setup({ connected: true, stepMs: 0, backups: [{ name: "atomik-kb-auto-2026-10-12-0900.atomikkb", bytes }] }, {});
    const [file] = await fresh.client.drive().listBackups();
    expect(file?.size).toBe(bytes.length);
    const downloaded = await fresh.client.drive().download(file?.id ?? "", () => undefined);
    const restored = await fresh.client.backupImport(downloaded, await kitWords());
    expect(restored.path).toBe(downloaded);
    expect((await fresh.client.spaceList()).map((space) => space.name)).toEqual(["Notes"]);
    // The connection is this device's: the restore leaves it.
    expect((await fresh.client.drive().status()).connected).toBe(true);
  });

  it("makes a file that is not a backup when none was given: restoring it is a Crypto error", async () => {
    const { client } = setup({ connected: true, stepMs: 0, backups: [{ name: "x.atomikkb" }] });
    const [file] = await client.drive().listBackups();
    const path = await client.drive().download(file?.id ?? "", () => undefined);
    const error = await failure(client.backupImport(path, await kitWords()));
    expect(error.code).toBe("Crypto");
  });
});

async function kitWords(): Promise<string> {
  return (await kitWordsFromEntropy(KEY)).join(" ");
}

describe("deliver, for the scheduled run", () => {
  const scheduled = (minute: number): string => `atomik-kb-auto-2026-10-12-09${String(minute).padStart(2, "0")}.atomikkb`;

  it("is not an error when Drive is not connected: nothing is sent", async () => {
    const { drive, client } = setup();
    expect(await drive.deliver(scheduled(0), new Uint8Array(3), 7)).toBe(false);
    expect(drive.backups).toEqual([]);
    expect(await client.settingsGet(SETTING)).toBeNull();
  });

  it("uploads, keeps the newest keep scheduled backups and leaves other files alone, and records the outcome", async () => {
    const { drive, client, events } = setup({
      connected: true,
      backups: [{ name: "mine.atomikkb" }, { name: scheduled(1) }, { name: scheduled(2) }],
    });
    events.length = 0;
    expect(await drive.deliver(scheduled(5), Uint8Array.from([1, 2, 3]), 2)).toBe(true);
    // The newest two by name: the new one and the scheduled(2); the others of its form went; "mine" stays.
    expect(
      drive.backups
        .map((file) => file.name)
        .sort(),
    ).toEqual(["mine.atomikkb", scheduled(2), scheduled(5)].sort());
    const uploaded = drive.backups.find((file) => file.name === scheduled(5));
    expect(uploaded?.size).toBe(3);
    const last = (await client.drive().status()).last;
    expect(last).toMatchObject({ ok: true, file: scheduled(5) });
    expect(last?.lastSuccessAt).toBe(last?.at);
    expect(events).toEqual([{ type: "SettingChanged", data: { key: SETTING } }]);
    // The upload can be downloaded again.
    const path = await client.drive().download(uploaded?.id ?? "", () => undefined);
    expect(path.endsWith(`${scheduled(5)}`)).toBe(true);
  });

  it("keeps at least one, whatever keep says", async () => {
    const { drive } = setup({ connected: true });
    await drive.deliver(scheduled(1), new Uint8Array(1), 0);
    expect(drive.backups.map((file) => file.name)).toEqual([scheduled(1)]);
  });

  it("fails with the reason it was given, keeping the last success in what it records", async () => {
    const { drive, client } = setup({ connected: true });
    await drive.deliver(scheduled(1), new Uint8Array(1), 7);
    const success = (await client.drive().status()).last;
    drive.setFailure("deliver", "quota");
    const error = await failure(drive.deliver(scheduled(2), new Uint8Array(1), 7));
    expect(driveReasonOf(error)).toBe("quota");
    const last = (await client.drive().status()).last;
    expect(last).toMatchObject({ ok: false, error: `${error.code}: ${error.message}`, lastSuccessAt: success?.at, file: scheduled(1) });
    expect(drive.backups.map((file) => file.name)).toEqual([scheduled(1)]);
  });

  it("a refused token (reconnect) sets needsReconnect, shown by the status", async () => {
    const { drive, client } = setup({ connected: true, fail: { deliver: "reconnect" } });
    const error = await failure(drive.deliver(scheduled(1), new Uint8Array(1), 7));
    expect(driveReasonOf(error)).toBe("reconnect");
    expect(await client.drive().status()).toMatchObject({ connected: true, needsReconnect: true, last: { ok: false } });
    // Until the next connect.
    drive.setFailure("deliver", undefined);
    expect(driveReasonOf(await failure(drive.deliver(scheduled(2), new Uint8Array(1), 7)))).toBe("reconnect");
    await client.drive().connect();
    expect(await drive.deliver(scheduled(2), new Uint8Array(1), 7)).toBe(true);
  });
});

describe("driveFailure", () => {
  it("carries the reason in the details, whatever the code", () => {
    for (const reason of DRIVE_ERROR_REASONS) {
      const error = driveFailure(reason);
      expect(driveReasonOf(error)).toBe(reason);
      expect(error.details).toMatchObject({ reason });
    }
    expect(driveFailure("notFound").code).toBe("NotFound");
  });
});
