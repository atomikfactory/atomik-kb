// Backups of `MemoryTransport` (Milestone 4, ADR-0017, in the mirror's own file format):
// export, restore into another installation, the simulated file dialogs, kit checks,
// damaged files and what a restore brings back. Codes, fields and messages are those of
// `kb-core`'s `backup.rs` and of the desktop shell's backup commands.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { sha256, toHex } from "./digest";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { kitCodeFromEntropy, kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { SeedSpace } from "./structure";

const START = 1_700_000_000_000;
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);
const PATH = "C:/backups/research.atomikkb";

/** Deterministic ids (`prefix` tells installations apart) and clock. */
function setup(options: MemoryTransportOptions = {}, prefix = "00000000") {
  let counter = 0;
  let clock = START;
  const transport = new MemoryTransport({
    newId: () => `${prefix}-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
    now: () => clock++,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events };
}

function id(n: number, prefix = "00000000"): string {
  return `${prefix}-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function blockId(n: number): string {
  return `b0000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

function paragraph(n: number, ...content: JsonValue[]): JsonValue {
  return { type: "paragraph", attrs: { id: blockId(n) }, content };
}

// Ids in creation order: the device (1), the workspace (2), "Notes" (3), "Plan" (4),
// "Ideas" (5), "Old" (6), "Work" (7), then the seeded kit's account (8).
const SPACES: readonly SeedSpace[] = [
  {
    name: "Notes",
    icon: "📒",
    pages: [
      {
        title: "Plan",
        icon: "🗺",
        content: {
          type: "doc",
          content: [
            paragraph(1, { type: "text", text: "See " }, { type: "pageRef", attrs: { id: blockId(2), pageId: id(5) } }),
          ],
        },
        children: [
          { title: "Ideas", content: { type: "doc", content: [paragraph(3, { type: "text", text: "zebra crossing" })] } },
        ],
      },
      { title: "Old" },
    ],
  },
  { name: "Work" },
];

/** The installation that writes backups: a workspace, pages, a setting, a kit with a known key. */
async function source(options: MemoryTransportOptions = {}) {
  const installation = setup({
    workspaceName: "Research",
    spaces: SPACES,
    settings: { "ui.theme": "dark" },
    security: { kitCreatedAt: START - 10_000, kitVerifiedAt: START - 9_000, kitKey: KEY },
    ...options,
  });
  await installation.client.pageTrash(id(6));
  return installation;
}

/** A fresh installation ("another machine") that sees the same files. */
function target(files: Map<string, Uint8Array>, options: MemoryTransportOptions = {}) {
  return setup({ files, ...options }, "11111111");
}

const words = async (): Promise<string> => (await kitWordsFromEntropy(KEY)).join(" ");

const text = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);

/** Re-encodes a backup with `edit` applied to its header and state, with a valid checksum. */
async function reseal(
  bytes: Uint8Array,
  edit: (parts: { header: Record<string, unknown>; state: Record<string, unknown> }) => void,
): Promise<Uint8Array> {
  const [first = "", headerLine = "", body = ""] = text(bytes).split("\n");
  const header = JSON.parse(headerLine) as Record<string, unknown>;
  const state = JSON.parse(text(Uint8Array.from(atob(body), (char) => char.charCodeAt(0)))) as Record<string, unknown>;
  edit({ header, state });
  const encoded = btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(state))));
  const signed = `${first}\n${JSON.stringify(header)}\n${encoded}\n`;
  return new TextEncoder().encode(`${signed}${toHex(await sha256(signed))}\n`);
}

const DIALOG_CANCELLED = { code: "InvalidInput", message: "no file was chosen", details: { field: "path" } };
const RELATIVE_PATH = { code: "InvalidInput", message: "the path must be absolute", details: { field: "path" } };

describe("backupExport", () => {
  it("writes the file and reports its size, documents, settings, time and account", async () => {
    const { client, transport, events } = await source();
    events.length = 0;
    const result = await client.backupExport(PATH);
    const bytes = transport.files.get(PATH);
    expect(bytes).toBeInstanceOf(Uint8Array);
    expect(result).toEqual({
      path: PATH,
      summary: {
        bytes: bytes?.length,
        // Two structure documents and the two page documents that exist.
        documents: 4,
        settings: 1,
        createdAt: expect.any(Number) as unknown,
        accountId: id(8),
        // No attachments in this workspace.
        chunks: 0,
        chunkBytes: 0,
      },
    });
    expect(events).toEqual([]);
  });

  it("is the mirror's own format, which never holds the kit", async () => {
    const { client, transport } = await source();
    await client.backupExport(PATH);
    const file = text(transport.files.get(PATH) ?? new Uint8Array());
    const [first, header, body, checksum, end] = file.split("\n");
    expect(first).toBe("ATOMIK-KB-MEMORY-BACKUP 1");
    expect(JSON.parse(header ?? "")).toEqual({
      accountId: id(8),
      createdAt: expect.any(Number) as unknown,
      recoveryId: expect.stringMatching(/^[0-9a-f]{64}$/u) as unknown,
    });
    expect(checksum).toMatch(/^[0-9a-f]{64}$/u);
    expect(end).toBe("");
    const state = text(Uint8Array.from(atob(body ?? ""), (char) => char.charCodeAt(0)));
    for (const kit of [await words(), await kitCodeFromEntropy(KEY), toHex(KEY)]) {
      expect(file).not.toContain(kit);
      expect(state).not.toContain(kit);
    }
  });

  it("without a path, writes the file the save dialog proposes", async () => {
    const { client, transport } = await source();
    const result = await client.backupExport();
    expect(result.path).toBe(`/backups/atomik-kb-${new Date(START).toISOString().slice(0, 10)}.atomikkb`);
    expect(result.path).toBe("/backups/atomik-kb-2023-11-14.atomikkb");
    expect([...transport.files.keys()]).toEqual([result.path]);
  });

  it("is InvalidInput for path when the dialog is cancelled or the path is not absolute", async () => {
    const { client, transport } = await source({ cancelDialogs: true });
    expect((await failure(client.backupExport())).toApiError()).toEqual(DIALOG_CANCELLED);
    for (const relative of ["research.atomikkb", "./research.atomikkb", "backups/a.atomikkb", "C:a.atomikkb", ""]) {
      expect((await failure(client.backupExport(relative))).toApiError(), relative).toEqual(RELATIVE_PATH);
    }
    expect(transport.files.size).toBe(0);
    for (const absolute of [PATH, "C:\\backups\\b.atomikkb", "/home/me/c.atomikkb", "\\\\server\\share\\d.atomikkb"]) {
      await expect(client.backupExport(absolute), absolute).resolves.toMatchObject({ path: absolute });
    }
    transport.setCancelDialogs(false);
    await expect(client.backupExport()).resolves.toMatchObject({
      path: expect.stringMatching(/^\/backups\/atomik-kb-/u) as unknown,
    });
  });

  it("needs the workspace and a recovery kit", async () => {
    const empty = setup();
    expect((await failure(empty.client.backupExport(PATH))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "there is no workspace to back up",
      details: { field: "workspace" },
    });
    const noKit = setup({ workspaceName: "Research" });
    expect((await failure(noKit.client.backupExport(PATH))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "create your recovery kit before exporting a backup",
      details: { field: "kit" },
    });
    expect(noKit.transport.files.size).toBe(0);
  });

  it("replaces a file at the same path", async () => {
    const { client, transport } = await source();
    await client.backupExport(PATH);
    const first = transport.files.get(PATH);
    await client.settingsSet("ui.theme", "light");
    await client.backupExport(PATH);
    expect(transport.files.size).toBe(1);
    expect(transport.files.get(PATH)).not.toEqual(first);
  });
});

describe("backupImport", () => {
  it("restores the whole workspace on another installation with the kit", async () => {
    const origin = await source();
    const exported = await origin.client.backupExport(PATH);
    const restored = target(origin.transport.files);
    const result = await restored.client.backupImport(undefined, await words());
    // The open dialog picked the file just written; the summary is the export's.
    expect(result).toEqual({ path: PATH, summary: exported.summary });
    expect(restored.events).toEqual([
      { type: "WorkspaceCreated", data: { id: id(2) } },
      { type: "SpacesChanged" },
      { type: "SecurityChanged" },
    ]);

    const [a, b] = [origin.client, restored.client];
    await expect(b.workspaceGet()).resolves.toEqual(await a.workspaceGet());
    await expect(b.appStatus()).resolves.toMatchObject({ workspaceInitialized: true, deviceId: id(1, "11111111") });
    await expect(b.spaceList()).resolves.toEqual(await a.spaceList());
    for (const spaceId of [id(3), id(7)]) {
      const [before, after] = [await a.treeGet(spaceId), await b.treeGet(spaceId)];
      expect(after.roots).toEqual(before.roots);
      expect(after.trashed).toEqual(before.trashed);
      // A restore brings no undo history.
      expect(after.undo).toEqual({ canUndo: false, canRedo: false });
    }
    for (const pageId of [id(4), id(5)]) {
      await expect(b.pageContentGet(pageId)).resolves.toEqual(await a.pageContentGet(pageId));
    }
    await expect(b.settingsGet("ui.theme")).resolves.toBe("dark");
    // Derived data is rebuilt: search and backlinks work on the restored pages.
    await expect(b.search("zebra")).resolves.toMatchObject([{ pageId: id(5), blockId: blockId(3) }]);
    await expect(b.backlinksGet(id(5))).resolves.toMatchObject([{ pageId: id(4), blockId: blockId(1) }]);
  });

  it("brings back the kit and the device log, with this device added via the kit and the lock off", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    const restored = target(origin.transport.files, { security: { appLockPassphrase: "correct horse" } });
    await restored.client.backupImport(PATH, await words());
    const status = await restored.client.securityStatus();
    expect(status).toEqual({
      accountId: id(8),
      // Restored with the kit: the kit is stale until re-issued.
      recoveryKit: { createdAt: START - 10_000, verifiedAt: START - 9_000, stale: true },
      appLock: { enabled: false, memoryKib: 65_536, iterations: 3, parallelism: 4 },
      thisDevice: id(1, "11111111"),
      devices: 2,
      keyring: { spaces: 2, documents: 4, attachments: 0, tombstones: 0 },
      rootGeneration: 0,
      rotationPending: false,
      thisDeviceRevoked: false,
      recoveryPublished: false,
    });
    const devices = await restored.client.deviceList();
    expect(devices).toEqual([
      expect.objectContaining({ deviceId: id(1), name: "memory device", addedViaKit: false, isThisDevice: false }),
      expect.objectContaining({ deviceId: id(1, "11111111"), addedViaKit: true, isThisDevice: true }),
    ]);
    // The same kit verifies, and opens the backups the restored installation writes.
    await restored.client.recoveryKitVerify(await words());
    await restored.client.backupExport("/backups/again.atomikkb");
    const third = target(restored.transport.files);
    await expect(
      third.client.backupImport("/backups/again.atomikkb", await kitCodeFromEntropy(KEY)),
    ).resolves.toMatchObject({ summary: { accountId: id(8) } });
  });

  it("keeps the keyring tombstones of purged pages", async () => {
    const origin = await source();
    await origin.client.pagePurge(id(6));
    await origin.client.backupExport(PATH);
    const restored = target(origin.transport.files);
    await restored.client.backupImport(PATH, await words());
    expect((await restored.client.securityStatus()).keyring).toEqual({
      spaces: 2,
      documents: 4,
      attachments: 0,
      tombstones: 1,
    });
  });

  it("restores the same account without the kit", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    const sameAccount = target(origin.transport.files, { security: { accountId: id(8) } });
    await expect(sameAccount.client.backupImport(PATH)).resolves.toMatchObject({ summary: { accountId: id(8) } });
    // Not opened through the kit: nothing is added to the device log.
    await expect(sameAccount.client.deviceList()).resolves.toMatchObject([{ deviceId: id(1), isThisDevice: false }]);
  });

  it("needs the kit of the backup: missing, wrong or malformed kits are InvalidInput for kit", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    const restored = target(origin.transport.files);
    expect((await failure(restored.client.backupImport(PATH))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the recovery kit is required to restore this backup",
      details: { field: "kit" },
    });
    const other = (await kitWordsFromEntropy(new Uint8Array(32).fill(9))).join(" ");
    expect((await failure(restored.client.backupImport(PATH, other))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "this kit does not belong to this backup",
      details: { field: "kit" },
    });
    expect((await failure(restored.client.backupImport(PATH, "abandon ability"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "expected 24 words, got 2",
      details: { field: "kit" },
    });
    const anotherAccount = target(origin.transport.files, { security: { accountId: id(99) } });
    expect((await failure(anotherAccount.client.backupImport(PATH))).message).toBe(
      "the recovery kit is required to restore this backup",
    );
    await expect(restored.client.workspaceGet()).resolves.toBeNull();
    expect(restored.events).toEqual([]);
  });

  it("is AlreadyExists where a workspace exists", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    expect((await failure(origin.client.backupImport(PATH, await words()))).toApiError()).toEqual({
      code: "AlreadyExists",
      message: "this installation already has a workspace; restoring needs a fresh one",
    });
    const other = target(origin.transport.files, { workspaceName: "Mine" });
    expect((await failure(other.client.backupImport(PATH, await words()))).code).toBe("AlreadyExists");
    await expect(other.client.workspaceGet()).resolves.toMatchObject({ name: "Mine" });
  });

  it("opens the most recently written file without a path, unless the dialog is cancelled", async () => {
    const origin = await source();
    await origin.client.backupExport("/backups/first.atomikkb");
    await origin.client.settingsSet("ui.theme", "light");
    await origin.client.backupExport("/backups/second.atomikkb");
    const cancelled = target(origin.transport.files, { cancelDialogs: true });
    expect((await failure(cancelled.client.backupImport(undefined, await words()))).toApiError()).toEqual(
      DIALOG_CANCELLED,
    );
    cancelled.transport.setCancelDialogs(false);
    await expect(cancelled.client.backupImport(undefined, await words())).resolves.toMatchObject({
      path: "/backups/second.atomikkb",
    });
    await expect(cancelled.client.settingsGet("ui.theme")).resolves.toBe("light");
  });

  it("has nothing to open on an empty disk, and refuses relative and missing paths", async () => {
    const empty = target(new Map());
    expect((await failure(empty.client.backupImport())).toApiError()).toEqual(DIALOG_CANCELLED);
    expect((await failure(empty.client.backupImport("research.atomikkb", await words()))).toApiError()).toEqual(
      RELATIVE_PATH,
    );
    expect((await failure(empty.client.backupImport("/backups/nowhere.atomikkb", await words()))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the backup file cannot be opened: no such file",
      details: { field: "path" },
    });
  });

  it("refuses damaged files and changes nothing", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    const good = origin.transport.files.get(PATH) ?? new Uint8Array();
    const restored = target(origin.transport.files);
    const kit = await words();
    const lines = text(good).split("\n");
    const versionAt = (lines[0]?.length ?? 0) - 1;
    const headerStart = versionAt + 2;
    const bodyStart = headerStart + (lines[1]?.length ?? 0) + 1;
    const checksumStart = bodyStart + (lines[2]?.length ?? 0) + 1;
    const positions = new Set([0, 10, headerStart, headerStart + 5, bodyStart, bodyStart + 100, checksumStart - 1]);
    for (let index = 0; index < good.length; index += 37) {
      positions.add(index);
    }
    positions.add(checksumStart);
    positions.add(good.length - 2);
    positions.add(good.length - 1);
    positions.delete(versionAt);
    const damaged: [string, Uint8Array][] = [...positions].map((index) => {
      const copy = good.slice();
      copy[index] = (copy[index] ?? 0) ^ 0x01;
      return [`byte ${String(index)} flipped`, copy];
    });
    damaged.push(
      ["truncated", good.slice(0, good.length - 10)],
      ["without the last line break", good.slice(0, good.length - 1)],
      ["extended", new Uint8Array([...good, 0x0a])],
      ["not text", new Uint8Array([0xff, 0xfe, 0x00, 0x41])],
    );
    for (const [name, bytes] of damaged) {
      restored.transport.files.set("/backups/damaged.atomikkb", bytes);
      const error = await failure(restored.client.backupImport("/backups/damaged.atomikkb", kit));
      expect(error.code, name).toBe("Crypto");
    }
    // A changed format version reads as a newer format, as in the core (`Storage`).
    const version = good.slice();
    version[versionAt] = (version[versionAt] ?? 0) ^ 0x01;
    restored.transport.files.set("/backups/damaged.atomikkb", version);
    expect((await failure(restored.client.backupImport("/backups/damaged.atomikkb", kit))).code).toBe("Storage");

    await expect(restored.client.workspaceGet()).resolves.toBeNull();
    await expect(restored.client.spaceList()).resolves.toEqual([]);
    expect((await restored.client.securityStatus()).recoveryKit).toBeUndefined();
    expect(restored.events).toEqual([]);
    await expect(restored.client.backupImport(PATH, kit)).resolves.toMatchObject({ path: PATH });
  });

  it("speaks the core's language about foreign, empty, damaged and newer files", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    const good = origin.transport.files.get(PATH) ?? new Uint8Array();
    const restored = target(origin.transport.files);
    const kit = await words();
    const cases: [string, Uint8Array, unknown][] = [
      ["a text file", new TextEncoder().encode("hello\n"), { code: "Crypto", message: "not a valid backup file: bad magic" }],
      ["an empty file", new Uint8Array(), { code: "Crypto", message: "not a valid backup file: empty file" }],
      ["a cut file", good.slice(0, good.length - 1), { code: "Crypto", message: "not a valid backup file: layout" }],
      [
        "a changed byte",
        new TextEncoder().encode(text(good).replace("\n", "\nX")),
        { code: "Crypto", message: "the backup is damaged or the key is wrong" },
      ],
      [
        "a newer format",
        new TextEncoder().encode(text(good).replace("BACKUP 1\n", "BACKUP 2\n")),
        { code: "Storage", message: "this backup needs a newer app version (format 2)" },
      ],
    ];
    for (const [name, bytes, expected] of cases) {
      restored.transport.files.set("/backups/file.atomikkb", bytes);
      expect((await failure(restored.client.backupImport("/backups/file.atomikkb", kit))).toApiError(), name).toEqual(
        expected,
      );
    }
  });

  it("checks a well-sealed file completely before restoring anything", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    const good = origin.transport.files.get(PATH) ?? new Uint8Array();
    const restored = target(origin.transport.files);
    const kit = await words();
    const records = { code: "Crypto", message: "not a valid backup file: records" };
    const edits: [string, (parts: { header: Record<string, unknown>; state: Record<string, unknown> }) => void, unknown][] =
      [
        [
          "a root that is not a page of its space",
          ({ state }) => {
            (state.spaces as { roots: string[] }[])[0]?.roots.push(id(77));
          },
          records,
        ],
        [
          "a page twice",
          ({ state }) => {
            const spaces = state.spaces as { pages: unknown[] }[];
            spaces[1]?.pages.push(spaces[0]?.pages[0]);
          },
          records,
        ],
        [
          "an invalid setting key",
          ({ state }) => {
            (state.settings as { key: string; value: JsonValue }[]).push({ key: "Not A Key", value: 1 });
          },
          records,
        ],
        [
          "content that is not a page document",
          ({ state }) => {
            const [document] = state.documents as { content: unknown }[];
            if (document !== undefined) {
              document.content = { type: "paragraph" };
            }
          },
          records,
        ],
        [
          "a missing workspace",
          ({ state }) => {
            delete state.workspace;
          },
          records,
        ],
        [
          "a malformed header",
          ({ header }) => {
            header.createdAt = "yesterday";
          },
          { code: "Crypto", message: "not a valid backup file: header" },
        ],
      ];
    for (const [name, edit, expected] of edits) {
      restored.transport.files.set("/backups/crafted.atomikkb", await reseal(good, edit));
      const error = await failure(restored.client.backupImport("/backups/crafted.atomikkb", kit));
      expect(error.toApiError(), name).toEqual(expected);
    }
    await expect(restored.client.workspaceGet()).resolves.toBeNull();
    await expect(restored.client.spaceList()).resolves.toEqual([]);
    expect(restored.events).toEqual([]);
    // The resealing itself is sound: an unchanged state restores.
    restored.transport.files.set("/backups/resealed.atomikkb", await reseal(good, () => undefined));
    await expect(restored.client.backupImport("/backups/resealed.atomikkb", kit)).resolves.toMatchObject({
      summary: { accountId: id(8) },
    });
  });

  it("checks the kit against the header before reading the state", async () => {
    const origin = await source();
    await origin.client.backupExport(PATH);
    const good = origin.transport.files.get(PATH) ?? new Uint8Array();
    const restored = target(origin.transport.files);
    const foreign = await reseal(good, ({ header, state }) => {
      header.recoveryId = "00".repeat(32);
      delete state.workspace;
    });
    restored.transport.files.set("/backups/foreign.atomikkb", foreign);
    const error = await failure(restored.client.backupImport("/backups/foreign.atomikkb", await words()));
    expect(error.toApiError()).toEqual({
      code: "InvalidInput",
      message: "this kit does not belong to this backup",
      details: { field: "kit" },
    });
  });
});
