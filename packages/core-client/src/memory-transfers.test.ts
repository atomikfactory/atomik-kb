// Attachment sync of `MemoryTransport` (Milestone 8, ADR-0023): `localChunks` and `availability`,
// remote attachments (`MemoryTransportOptions.attachmentSync`), the download policy setting,
// `AttachmentDownload`, reads of incomplete attachments, the fake transfer step
// (`advanceTransfers` and the timer of `transferStepMs`) and the counts of `SyncInfo.transfers`.

import { afterEach, describe, expect, it, vi } from "vitest";

import type { MemoryRemoteAttachmentSeed } from "./attachment-mirror";
import {
  CHUNK_SIZE,
  DOWNLOAD_POLICY_EAGER,
  DOWNLOAD_POLICY_ON_DEMAND,
  DOWNLOAD_POLICY_SETTING,
  downloadPolicyOf,
} from "./attachments";
import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";

const T0 = 1_700_000_000_000;
const PASSPHRASE = "correct horse";
/** Remote: an image of two chunks and a text of one, both shown on "Alpha". */
const IMAGE = "aaaaaaaa-0000-4000-8000-000000000001";
const FILE = "aaaaaaaa-0000-4000-8000-000000000002";
/** Seeded here. */
const LOCAL = "aaaaaaaa-0000-4000-8000-000000000003";
const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";

const SYNC_CHANGED: Event = { type: "SyncChanged" };
const SYNC_REQUESTED: Event = { type: "SyncRequested" };

afterEach(() => {
  vi.useRealTimers();
});

function pattern(length: number, seed: number): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(length);
  for (let i = 0; i < length; i += 1) {
    bytes[i] = (i * 31 + seed) & 0xff;
  }
  return bytes;
}

/** Byte equality without the cost of a deep comparison of megabytes. */
function sameBytes(a: Uint8Array | undefined, b: Uint8Array): boolean {
  return a !== undefined && a.length === b.length && a.every((value, index) => value === b[index]);
}

const IMAGE_BYTES = pattern(CHUNK_SIZE + 5, 1);
const FILE_BYTES = pattern(10, 2);

/** The remote image (two chunks) and file (one chunk); `waiting` applies to the image only. */
function remote(image: Partial<MemoryRemoteAttachmentSeed> = {}): MemoryRemoteAttachmentSeed[] {
  return [
    { attachmentId: IMAGE, name: "photo.png", mime: "image/png", bytes: IMAGE_BYTES, ...image },
    { attachmentId: FILE, name: "notes.txt", mime: "text/plain", bytes: FILE_BYTES },
  ];
}

const ALPHA: JsonValue = {
  type: "doc",
  content: [
    { type: "image", attrs: { id: "bbbbbbbb-0000-4000-8000-000000000001", attachmentId: IMAGE, alt: "A photo" } },
    { type: "file", attrs: { id: "bbbbbbbb-0000-4000-8000-000000000002", attachmentId: FILE } },
  ],
};

/**
 * Deterministic ids, a fixed clock, a client, the events (`take` hands them over and forgets
 * them) and the id of the space "Personal".
 */
async function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
    now: () => T0,
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: [{ title: "Alpha", content: ALPHA }] }],
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const take = (): Event[] => events.splice(0);
  const personal = (await client.spaceList())[0]?.id ?? "";
  const changed: Event = { type: "AttachmentsChanged", data: { spaceId: personal } };
  return { transport, client, take, personal, changed };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

function incomplete(local: number, count: number) {
  return { code: "Storage", message: `the attachment is not complete on this device (${String(local)} of ${String(count)} chunks)` };
}

describe("availability and AttachmentDownload", () => {
  it("attachments made or seeded here are Complete; AttachmentDownload answers them unchanged, without events", async () => {
    const { client, take, personal } = await setup({
      attachments: [{ attachmentId: LOCAL, name: "a.txt", mime: "text/plain", bytes: pattern(5, 3) }],
    });
    const uploaded = await client.attachmentUpload(personal, "b.bin", "application/octet-stream", [pattern(CHUNK_SIZE + 1, 4)]);
    expect(uploaded).toMatchObject({ chunkCount: 2, localChunks: 2, availability: "Complete" });
    expect(await client.attachmentGet(LOCAL)).toMatchObject({ chunkCount: 1, localChunks: 1, availability: "Complete" });
    take();
    await expect(client.attachmentDownload(LOCAL)).resolves.toEqual(await client.attachmentGet(LOCAL));
    await expect(client.attachmentDownload(uploaded.attachmentId)).resolves.toEqual(uploaded);
    expect(take()).toEqual([]);
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 0 });
  });

  it("remote attachments start with no chunk here: Downloading under the default (eager) policy", async () => {
    const { client, personal } = await setup({ attachmentSync: { remoteAttachments: remote() } });
    expect(await client.settingsGet(DOWNLOAD_POLICY_SETTING)).toBeNull();
    expect(await client.attachmentGet(IMAGE)).toEqual({
      attachmentId: IMAGE,
      spaceId: personal,
      name: "photo.png",
      mime: "image/png",
      size: CHUNK_SIZE + 5,
      chunkCount: 2,
      createdAt: T0,
      references: 1,
      localChunks: 0,
      availability: "Downloading",
    });
    expect((await client.attachmentList(personal)).map((info) => [info.attachmentId, info.availability])).toEqual([
      [IMAGE, "Downloading"],
      [FILE, "Downloading"],
    ]);
    // Reported whether sync is configured or not.
    expect(await client.syncStatus()).toMatchObject({ configured: false, transfers: { uploads: 0, downloads: 3, waiting: 0 } });
  });

  it("advanceTransfers moves one chunk per call, oldest attachment first, until each is Complete", async () => {
    const { transport, client, take, changed } = await setup({ attachmentSync: { remoteAttachments: remote() } });
    expect(transport.advanceTransfers()).toBe(1);
    expect(take()).toEqual([changed, SYNC_CHANGED]);
    expect(await client.attachmentGet(IMAGE)).toMatchObject({ localChunks: 1, chunkCount: 2, availability: "Downloading" });
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 2, waiting: 0 });

    expect(transport.advanceTransfers(5)).toBe(2);
    expect(take()).toEqual([changed, SYNC_CHANGED, changed, SYNC_CHANGED]);
    expect(await client.attachmentGet(IMAGE)).toMatchObject({ localChunks: 2, availability: "Complete" });
    expect(await client.attachmentGet(FILE)).toMatchObject({ localChunks: 1, availability: "Complete" });
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 0 });
    expect(transport.advanceTransfers(3)).toBe(0);
    expect(take()).toEqual([]);

    expect(sameBytes(await client.attachmentRead(IMAGE, { start: 0, end: CHUNK_SIZE + 5 }), IMAGE_BYTES)).toBe(true);
    expect(await client.attachmentRead(FILE)).toEqual(FILE_BYTES);
  });

  it("transfer events are not local edits: nothing becomes pending and no cycle is asked for", async () => {
    const { transport, client, take } = await setup({
      sync: { state: "idle" },
      attachmentSync: { remoteAttachments: remote() },
    });
    expect(transport.advanceTransfers(3)).toBe(3);
    expect(take().filter((event) => event.type === "SyncRequested")).toEqual([]);
    expect(await client.syncStatus()).toMatchObject({ state: "Idle", pendingEntries: 0 });
  });

  it("reads of an incomplete attachment fail with Storage (n of m chunks) whatever the range; so do save and open", async () => {
    const { transport, client, take } = await setup({ attachmentSync: { remoteAttachments: remote() } });
    transport.advanceTransfers();
    take();
    for (const call of [
      () => client.attachmentRead(IMAGE),
      () => client.attachmentRead(IMAGE, { start: 0, end: 1 }),
      () => client.attachmentSaveAs(IMAGE, "/downloads/photo.png"),
      () => client.attachmentOpen(IMAGE),
    ]) {
      expect((await failure(call())).toApiError()).toEqual(incomplete(1, 2));
    }
    expect((await failure(client.attachmentRead(FILE))).toApiError()).toEqual(incomplete(0, 1));
    // Under the eager policy they are wanted already: nothing more happens.
    expect(take()).toEqual([]);
    expect(transport.files.size).toBe(0);
  });

  it("AttachmentDownload of an unknown or purged attachment is NotFound", async () => {
    const { client } = await setup({ attachmentSync: { remoteAttachments: remote() } });
    await client.attachmentDelete(FILE);
    for (const attachmentId of [UNKNOWN, FILE]) {
      expect((await failure(client.attachmentDownload(attachmentId))).toApiError()).toEqual({
        code: "NotFound",
        message: "attachment not found",
      });
    }
  });
});

describe("the on-demand policy", () => {
  const onDemand = { attachmentSync: { downloadPolicy: DOWNLOAD_POLICY_ON_DEMAND, remoteAttachments: remote() } } as const;

  it("leaves remote attachments NotDownloaded: nothing is due until something asks", async () => {
    const { transport, client, take } = await setup(onDemand);
    expect(await client.settingsGet(DOWNLOAD_POLICY_SETTING)).toBe("on_demand");
    expect(await client.attachmentGet(IMAGE)).toMatchObject({ localChunks: 0, chunkCount: 2, availability: "NotDownloaded" });
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 0 });
    expect(transport.advanceTransfers(5)).toBe(0);
    expect(take()).toEqual([]);
  });

  it("AttachmentDownload makes one attachment wanted (AttachmentsChanged, SyncRequested), which then downloads", async () => {
    const { transport, client, take, changed } = await setup(onDemand);
    expect(await client.attachmentDownload(FILE)).toMatchObject({ localChunks: 0, availability: "Downloading" });
    expect(take()).toEqual([changed, SYNC_REQUESTED]);
    expect((await client.attachmentGet(IMAGE)).availability).toBe("NotDownloaded");
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 1, waiting: 0 });

    // Asking again pokes the driver only.
    await client.attachmentDownload(FILE);
    expect(take()).toEqual([SYNC_REQUESTED]);

    expect(transport.advanceTransfers(5)).toBe(1);
    expect(take()).toEqual([changed, SYNC_CHANGED]);
    expect(await client.attachmentGet(FILE)).toMatchObject({ localChunks: 1, availability: "Complete" });
    expect(await client.attachmentRead(FILE)).toEqual(FILE_BYTES);
    await client.attachmentDownload(FILE);
    expect(take()).toEqual([]);
  });

  it("a read asks for the attachment as well, and still fails until its chunks are here", async () => {
    const { transport, client, take, changed } = await setup(onDemand);
    expect((await failure(client.attachmentRead(IMAGE))).toApiError()).toEqual(incomplete(0, 2));
    expect(take()).toEqual([changed, SYNC_REQUESTED]);
    expect((await client.attachmentGet(IMAGE)).availability).toBe("Downloading");
    expect((await failure(client.attachmentOpen(IMAGE))).toApiError()).toEqual(incomplete(0, 2));
    expect(take()).toEqual([]);
    expect(transport.advanceTransfers(5)).toBe(2);
    expect(sameBytes(await client.attachmentRead(IMAGE, { start: 0, end: CHUNK_SIZE + 5 }), IMAGE_BYTES)).toBe(true);
    expect((await client.attachmentGet(FILE)).availability).toBe("NotDownloaded");
  });

  it("turning the setting to eager makes every incomplete attachment wanted; back to on demand changes nothing", async () => {
    const { transport, client, take, changed } = await setup(onDemand);
    transport.advanceTransfers();
    await client.settingsSet("ui.theme", "dark");
    expect(take()).toEqual([{ type: "SettingChanged", data: { key: "ui.theme" } }]);

    await client.settingsSet(DOWNLOAD_POLICY_SETTING, DOWNLOAD_POLICY_EAGER);
    expect(take()).toEqual([{ type: "SettingChanged", data: { key: DOWNLOAD_POLICY_SETTING } }, changed, SYNC_REQUESTED]);
    expect((await client.attachmentList((await client.spaceList())[0]?.id ?? "")).map((info) => info.availability)).toEqual([
      "Downloading",
      "Downloading",
    ]);

    await client.settingsSet(DOWNLOAD_POLICY_SETTING, DOWNLOAD_POLICY_ON_DEMAND);
    expect(take()).toEqual([{ type: "SettingChanged", data: { key: DOWNLOAD_POLICY_SETTING } }]);
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 3, waiting: 0 });
    expect(transport.advanceTransfers(5)).toBe(3);
  });

  it("any value but on_demand means eager, as the core reads the setting", async () => {
    for (const value of [DOWNLOAD_POLICY_EAGER, "sometimes", "ON_DEMAND", 1, null, undefined, { on_demand: true }]) {
      expect(downloadPolicyOf(value), JSON.stringify(value)).toBe("eager");
    }
    expect(downloadPolicyOf("on_demand")).toBe("on_demand");
    expect(DOWNLOAD_POLICY_SETTING).toBe("app.attachments.download");

    const { client, take, changed } = await setup(onDemand);
    await client.settingsSet(DOWNLOAD_POLICY_SETTING, "sometimes");
    expect(take()).toEqual([{ type: "SettingChanged", data: { key: DOWNLOAD_POLICY_SETTING } }, changed, SYNC_REQUESTED]);
  });

  it("the downloadPolicy option replaces a value from the settings; without it the settings decide", async () => {
    const replaced = await setup({
      settings: { [DOWNLOAD_POLICY_SETTING]: "eager" },
      attachmentSync: { downloadPolicy: "on_demand", remoteAttachments: remote() },
    });
    expect(await replaced.client.settingsGet(DOWNLOAD_POLICY_SETTING)).toBe("on_demand");
    expect((await replaced.client.attachmentGet(IMAGE)).availability).toBe("NotDownloaded");

    const fromSettings = await setup({
      settings: { [DOWNLOAD_POLICY_SETTING]: "on_demand" },
      attachmentSync: { remoteAttachments: remote() },
    });
    expect((await fromSettings.client.attachmentGet(IMAGE)).availability).toBe("NotDownloaded");
  });
});

describe("attachments waiting for the uploading device", () => {
  it("are Waiting at once under the eager policy and never progress", async () => {
    const { transport, client, take } = await setup({ attachmentSync: { remoteAttachments: remote({ waiting: true }) } });
    expect(await client.attachmentGet(IMAGE)).toMatchObject({ localChunks: 0, chunkCount: 2, availability: "Waiting" });
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 1, waiting: 2 });
    // Only the file moves.
    expect(transport.advanceTransfers(10)).toBe(1);
    expect((await client.attachmentGet(IMAGE)).availability).toBe("Waiting");
    expect((await client.attachmentGet(FILE)).availability).toBe("Complete");
    expect((await failure(client.attachmentRead(IMAGE))).toApiError()).toEqual(incomplete(0, 2));
    take();
    // Asking again keeps it waiting and pokes the driver.
    expect((await client.attachmentDownload(IMAGE)).availability).toBe("Waiting");
    expect(take()).toEqual([SYNC_REQUESTED]);
    expect(transport.advanceTransfers(10)).toBe(0);
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 2 });
  });

  it("under on demand are NotDownloaded, Downloading once asked, then Waiting after one step", async () => {
    const { transport, client, take, changed } = await setup({
      attachmentSync: { downloadPolicy: "on_demand", remoteAttachments: remote({ waiting: true }) },
    });
    expect((await client.attachmentGet(IMAGE)).availability).toBe("NotDownloaded");
    expect((await client.attachmentDownload(IMAGE)).availability).toBe("Downloading");
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 2, waiting: 0 });
    take();
    expect(transport.advanceTransfers()).toBe(1);
    expect(take()).toEqual([changed, SYNC_CHANGED]);
    expect(await client.attachmentGet(IMAGE)).toMatchObject({ localChunks: 0, availability: "Waiting" });
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 2 });
    expect(transport.advanceTransfers()).toBe(0);
  });
});

describe("uploads in SyncInfo.transfers", () => {
  it("count the chunks of attachments finished while sync is configured; a step sends them before any download", async () => {
    const { transport, client, take, personal } = await setup({
      sync: { state: "idle" },
      syncStepMs: 60_000,
      attachmentSync: { remoteAttachments: remote() },
    });
    vi.useFakeTimers();
    const uploaded = await client.attachmentUpload(personal, "c.bin", "application/octet-stream", [pattern(CHUNK_SIZE + 1, 5)]);
    expect(uploaded.availability).toBe("Complete");
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 2, downloads: 3, waiting: 0 });
    take();
    expect(transport.advanceTransfers()).toBe(1);
    expect(take()).toEqual([SYNC_CHANGED]);
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 1, downloads: 3, waiting: 0 });
    expect(transport.advanceTransfers(2)).toBe(2);
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 2, waiting: 0 });
    expect((await client.attachmentGet(IMAGE)).localChunks).toBe(1);
  });

  it("are not counted while sync is off", async () => {
    const { client, personal } = await setup();
    await client.attachmentUpload(personal, "c.bin", "application/octet-stream", [pattern(3, 5)]);
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 0 });
  });

  it("go with the push of a healthy cycle of the fake driver, stay while it is offline, and go when sync is disabled", async () => {
    vi.useFakeTimers();
    const { transport, client, personal } = await setup({ sync: { state: "idle" }, syncStepMs: 100 });
    await client.attachmentUpload(personal, "c.bin", "application/octet-stream", [pattern(3, 5)]);
    expect((await client.syncStatus()).transfers.uploads).toBe(1);
    vi.advanceTimersByTime(200); // waiting, then Pushing: the push sends the chunk
    expect(await client.syncStatus()).toMatchObject({ state: "Pulling", transfers: { uploads: 0 } });
    vi.advanceTimersByTime(1000);

    transport.setSyncMode("offline");
    await client.attachmentUpload(personal, "d.bin", "application/octet-stream", [pattern(3, 6)]);
    vi.advanceTimersByTime(1000);
    expect(await client.syncStatus()).toMatchObject({ state: "Offline", transfers: { uploads: 1 } });
    expect((await client.syncConfigure(null)).transfers).toEqual({ uploads: 0, downloads: 0, waiting: 0 });
  });
});

describe("the transfer timer (transferStepMs)", () => {
  it("runs one step per period while something is due, then stops", async () => {
    vi.useFakeTimers();
    const { client, take, changed } = await setup({ attachmentSync: { remoteAttachments: remote(), transferStepMs: 100 } });
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(99);
    expect((await client.attachmentGet(IMAGE)).localChunks).toBe(0);
    vi.advanceTimersByTime(1);
    expect((await client.attachmentGet(IMAGE)).localChunks).toBe(1);
    expect(take()).toEqual([changed, SYNC_CHANGED]);
    vi.advanceTimersByTime(200);
    expect((await client.attachmentGet(IMAGE)).availability).toBe("Complete");
    expect((await client.attachmentGet(FILE)).availability).toBe("Complete");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("starts when something becomes due, and skips its steps while the core is locked", async () => {
    vi.useFakeTimers();
    const { transport, client } = await setup({
      security: { appLockPassphrase: PASSPHRASE },
      attachmentSync: { downloadPolicy: "on_demand", remoteAttachments: remote(), transferStepMs: 100 },
    });
    expect(vi.getTimerCount()).toBe(0);
    await client.attachmentDownload(IMAGE);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(100);
    await transport.lock();
    vi.advanceTimersByTime(1000);
    expect(transport.advanceTransfers()).toBe(0);
    await transport.unlock(PASSPHRASE);
    expect(await client.attachmentGet(IMAGE)).toMatchObject({ localChunks: 1, availability: "Downloading" });
    vi.advanceTimersByTime(100);
    expect((await client.attachmentGet(IMAGE)).availability).toBe("Complete");
    expect((await client.attachmentGet(FILE)).availability).toBe("NotDownloaded");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not run without transferStepMs, nor for waiting attachments", async () => {
    vi.useFakeTimers();
    await setup({ attachmentSync: { remoteAttachments: remote() } });
    expect(vi.getTimerCount()).toBe(0);
    await setup({ attachmentSync: { remoteAttachments: remote({ waiting: true }).slice(0, 1), transferStepMs: 100 } });
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("remote attachments elsewhere in the mirror", () => {
  it("purges drop them from the counts; the collection leaves the referenced ones alone", async () => {
    const { transport, client } = await setup({ attachmentSync: { remoteAttachments: remote() } });
    transport.advanceTransfers();
    await client.attachmentDelete(FILE);
    expect((await client.syncStatus()).transfers).toEqual({ uploads: 0, downloads: 1, waiting: 0 });
    expect(await client.attachmentGc()).toEqual({ purged: 0, chunksDeleted: 0, bytesFreed: 0 });
    expect(transport.advanceTransfers(5)).toBe(1);
    expect((await client.attachmentGet(IMAGE)).availability).toBe("Complete");
  });

  it("a backup carries only the chunks that are here", async () => {
    const { transport } = await setup({
      security: { kitCreatedAt: T0 },
      attachmentSync: { remoteAttachments: remote() },
    });
    transport.advanceTransfers();
    const { summary } = await transport.backupExport("/backups/b.atomikkb");
    expect(summary).toMatchObject({ chunks: 1 });
  });

  it("seeds are checked like uploads, need a space, and cannot reuse an id", async () => {
    await expect(
      setup({ attachmentSync: { remoteAttachments: [{ name: "a/b", mime: "text/plain", bytes: pattern(1, 1) }] } }),
    ).rejects.toThrow(CoreError);
    await expect(
      setup({ spaces: [], attachmentSync: { remoteAttachments: [{ name: "a", mime: "text/plain", bytes: pattern(1, 1) }] } }),
    ).rejects.toThrow("MemoryTransport: seeded attachments need a space");
    await expect(
      setup({
        attachments: [{ attachmentId: IMAGE, name: "a", mime: "text/plain", bytes: pattern(1, 1) }],
        attachmentSync: { remoteAttachments: remote() },
      }),
    ).rejects.toThrow("attachment id already in use");
  });

  it("advanceTransfers wants a whole number of steps, and does nothing while the core is not ready", async () => {
    const { transport } = await setup({ attachmentSync: { remoteAttachments: remote() } });
    expect(() => transport.advanceTransfers(-1)).toThrow("MemoryTransport: advanceTransfers wants a whole number of steps");
    expect(() => transport.advanceTransfers(1.5)).toThrow(CoreError);
    expect(transport.advanceTransfers(0)).toBe(0);
    transport.setCoreState("opening");
    expect(transport.advanceTransfers()).toBe(0);
    transport.setCoreState("ready");
    expect(transport.advanceTransfers()).toBe(1);
  });
});
