// Attachments of `MemoryTransport` (Milestone 5, ADR-0020): uploads, chunking, reads,
// references derived from the page documents, garbage collection, the shell's file commands,
// backups. Codes, fields, messages and events are those of `kb-core`'s `attachments.rs`.

import { LoroDoc, LoroList, LoroMap } from "loro-crdt";
import { describe, expect, it } from "vitest";

import { CHUNK_SIZE, MAX_ATTACHMENT_BYTES, MAX_PREVIEW_BYTES, RETENTION_SETTING } from "./attachments";
import { MemoryAttachments } from "./attachment-mirror";
import { CoreClient } from "./client";
import type { MemoryContent } from "./content";
import { demoImagePng } from "./demo-image";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { configurePageDoc } from "./page-doc";
import type { MemoryStructure } from "./structure";

const DAY = 24 * 60 * 60 * 1000;
const T0 = 1_700_000_000_000;
const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";
/** The recovery kit key of the installations that write backups. */
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index * 3);

/** The ids `setup` hands out, in order. */
function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

/**
 * Deterministic ids, a clock the test moves (`clock.now`), a client, the emitted events and the
 * ids of the spaces "Personal" and "Work" and of the pages "Alpha" and "Beta" ("" when absent).
 */
async function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: [{ title: "Alpha" }, { title: "Beta" }] }, { name: "Work" }],
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const spaces = await client.spaceList();
  const spaceId = (name: string): string => spaces.find((space) => space.name === name)?.id ?? "";
  const personal = spaceId("Personal");
  const roots = personal === "" ? [] : (await client.treeGet(personal)).roots;
  const pageId = (title: string): string => roots.find((node) => node.page.title === title)?.page.id ?? "";
  return { transport, client, events, clock, personal, work: spaceId("Work"), alpha: pageId("Alpha"), beta: pageId("Beta") };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

function pattern(length: number, seed = 1): Uint8Array<ArrayBuffer> {
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

const changed = (spaceId: string): Event => ({ type: "AttachmentsChanged", data: { spaceId } });

/** Writes `bytes` to an upload in pieces of at most one chunk, as the shell accepts them. */
async function writeAll(client: CoreClient, attachmentId: string, bytes: Uint8Array): Promise<void> {
  for (let start = 0; start < bytes.length; start += CHUNK_SIZE) {
    await client.attachmentWrite(attachmentId, bytes.subarray(start, start + CHUNK_SIZE));
  }
}

// `instanceof` alone would narrow to `LoroList<any>`; these keep the default `unknown` values.
function isList(value: unknown): value is LoroList {
  return value instanceof LoroList;
}

function isMap(value: unknown): value is LoroMap {
  return value instanceof LoroMap;
}

/** Appends a leaf block (`image` or `file`, or any node) to a page document. */
function appendLeaf(doc: LoroDoc, nodeName: string, attributes: Record<string, string | number>): void {
  const blocks = doc.getMap("doc").get("children");
  if (!isList(blocks)) {
    throw new Error("no blocks");
  }
  const block = blocks.insertContainer(blocks.length, new LoroMap());
  block.set("nodeName", nodeName);
  const attrs = block.setContainer("attributes", new LoroMap());
  for (const [key, value] of Object.entries(attributes)) {
    attrs.set(key, value);
  }
  block.setContainer("children", new LoroList());
}

/** Removes every top-level block of a page document that shows `attachmentId`. */
function removeLeaves(doc: LoroDoc, attachmentId: string): void {
  const blocks = doc.getMap("doc").get("children");
  if (!isList(blocks)) {
    throw new Error("no blocks");
  }
  for (let index = blocks.length - 1; index >= 0; index -= 1) {
    const block: unknown = blocks.get(index);
    const attrs: unknown = isMap(block) ? block.get("attributes") : undefined;
    if (isMap(attrs) && attrs.get("attachmentId") === attachmentId) {
      blocks.delete(index, 1);
    }
  }
}

function imagePage(attachmentId: string, alt = "A diagram"): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: blockId(1) }, content: [{ type: "text", text: "Look:" }] },
      { type: "image", attrs: { id: blockId(2), attachmentId, alt, width: 320 } },
    ],
  };
}

describe("uploads (begin, write, finish)", () => {
  it("cut chunks like the core: full chunks, then a final one that may be empty", async () => {
    const { transport, client, personal } = await setup();
    const sizes: [number, number][] = [
      [0, 1],
      [1, 1],
      [CHUNK_SIZE - 1, 1],
      [CHUNK_SIZE, 2],
      [CHUNK_SIZE + 1, 2],
      [9 * 1024 * 1024, 3],
    ];
    for (const [size, chunkCount] of sizes) {
      const before = transport.blobs.size;
      const content = pattern(size, size % 7);
      const info = await client.attachmentUpload(personal, `f${String(size)}.bin`, "application/octet-stream", new Blob([content]));
      expect(info, String(size)).toMatchObject({ size, chunkCount, references: 0, spaceId: personal });
      expect(transport.blobs.size - before).toBe(chunkCount);
      expect(sameBytes(await client.attachmentRead(info.attachmentId), content), String(size)).toBe(true);
    }
  });

  it("store a chunk as soon as it is complete, whatever the pieces", async () => {
    const { transport, client, personal } = await setup();
    const attachmentId = await client.attachmentBegin(personal, "pieces.bin", "application/octet-stream");
    const content = pattern(CHUNK_SIZE + 100);
    await client.attachmentWrite(attachmentId, content.subarray(0, 10));
    expect(transport.blobs.size).toBe(0);
    await client.attachmentWrite(attachmentId, content.subarray(10, CHUNK_SIZE));
    expect(transport.blobs.size).toBe(1);
    await client.attachmentWrite(attachmentId, content.subarray(CHUNK_SIZE));
    expect(transport.blobs.size).toBe(1);
    const info = await client.attachmentFinish(attachmentId);
    expect(info).toMatchObject({ size: CHUNK_SIZE + 100, chunkCount: 2 });
    expect(transport.blobs.size).toBe(2);
    expect(sameBytes(await client.attachmentRead(attachmentId), content)).toBe(true);
  });

  it("copy the bytes they are given", async () => {
    const { client, personal } = await setup();
    const content = pattern(10);
    const attachmentId = await client.attachmentBegin(personal, "a.bin", "application/octet-stream");
    await client.attachmentWrite(attachmentId, content);
    content.fill(0);
    await client.attachmentFinish(attachmentId);
    expect(await client.attachmentRead(attachmentId)).toEqual(pattern(10));
  });

  it("are created at begin and unreferenced since finish", async () => {
    const { client, clock, personal } = await setup();
    const attachmentId = await client.attachmentBegin(personal, "a.txt", "text/plain");
    clock.now = T0 + 500;
    const info = await client.attachmentFinish(attachmentId);
    expect(info).toEqual({
      attachmentId,
      spaceId: personal,
      name: "a.txt",
      mime: "text/plain",
      size: 0,
      chunkCount: 1,
      createdAt: T0,
      references: 0,
      unreferencedSince: T0 + 500,
      localChunks: 1,
      availability: "Complete",
    });
  });

  it("emit AttachmentsChanged on finish and abort, not on begin", async () => {
    const { client, events, personal } = await setup();
    events.length = 0;
    const first = await client.attachmentBegin(personal, "a.txt", "text/plain");
    expect(events).toEqual([]);
    await client.attachmentFinish(first);
    expect(events).toEqual([changed(personal)]);
    const second = await client.attachmentBegin(personal, "b.txt", "text/plain");
    await client.attachmentAbort(second);
    expect(events).toEqual([changed(personal), changed(personal)]);
  });

  it("abort drops the written chunks and the upload", async () => {
    const { transport, client, personal } = await setup();
    const attachmentId = await client.attachmentBegin(personal, "a.bin", "application/octet-stream");
    await writeAll(client, attachmentId, pattern(2 * CHUNK_SIZE + 1));
    expect(transport.blobs.size).toBe(2);
    await client.attachmentAbort(attachmentId);
    expect(transport.blobs.size).toBe(0);
    for (const call of [
      () => client.attachmentFinish(attachmentId),
      () => client.attachmentAbort(attachmentId),
      () => client.attachmentWrite(attachmentId, pattern(1)),
    ]) {
      expect((await failure(call())).toApiError()).toEqual({
        code: "NotFound",
        message: "no upload in progress for this attachment",
      });
    }
    expect((await failure(client.attachmentGet(attachmentId))).toApiError()).toEqual({
      code: "NotFound",
      message: "attachment not found",
    });
    expect(await client.attachmentList(personal)).toEqual([]);
  });

  it("are not listed, read or deleted before they finish, and cannot be written after", async () => {
    const { client, personal } = await setup();
    const attachmentId = await client.attachmentBegin(personal, "a.txt", "text/plain");
    await client.attachmentWrite(attachmentId, pattern(5));
    expect(await client.attachmentList(personal)).toEqual([]);
    for (const call of [
      () => client.attachmentGet(attachmentId),
      () => client.attachmentRead(attachmentId),
      () => client.attachmentDelete(attachmentId),
    ]) {
      expect((await failure(call())).message).toBe("attachment not found");
    }
    await client.attachmentFinish(attachmentId);
    expect((await failure(client.attachmentWrite(attachmentId, pattern(1)))).message).toBe(
      "no upload in progress for this attachment",
    );
  });

  it("begin checks the name, the MIME type, the workspace and the space, in that order", async () => {
    const { client, personal } = await setup();
    expect((await failure(client.attachmentBegin(UNKNOWN, " ", "nope"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the attachment needs a name",
      details: { field: "name" },
    });
    expect((await failure(client.attachmentBegin(UNKNOWN, "a.txt", "nope"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the MIME type must look like type/subtype",
      details: { field: "mime" },
    });
    expect((await failure(client.attachmentBegin(UNKNOWN, "a.txt", "text/plain"))).toApiError()).toEqual({
      code: "NotFound",
      message: "space not found",
    });
    const fresh = await setup({ workspaceName: undefined });
    expect((await failure(fresh.client.attachmentBegin(personal, "a.txt", "text/plain"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "create the workspace first",
      details: { field: "workspace" },
    });
  });

  it("begin refuses an id that exists already with AlreadyExists", async () => {
    const transport = new MemoryTransport({
      newId: () => "00000000-0000-4000-8000-000000000001",
      workspaceName: "Home",
    });
    const client = new CoreClient(transport);
    const space = await client.spaceCreate("Personal");
    // Every id is the same: the second upload in progress collides with the first.
    const attachmentId = await client.attachmentBegin(space.id, "a.txt", "text/plain");
    expect((await failure(client.attachmentBegin(space.id, "b.txt", "text/plain"))).toApiError()).toEqual({
      code: "AlreadyExists",
      message: "attachment id already in use",
    });
    await client.attachmentFinish(attachmentId);
    expect((await failure(client.attachmentBegin(space.id, "b.txt", "text/plain"))).code).toBe("AlreadyExists");
  });

  it("the core refuses a write past 2 GiB and keeps nothing of the refused piece", () => {
    // Below the shell (which caps a piece at one chunk): a 2 GiB array cannot be allocated in a
    // test, a view that claims the length is enough.
    const attachments = new MemoryAttachments({
      now: () => T0,
      newId: () => id(1),
      emit: () => undefined,
      structure: { spacePageIds: () => [], spaceIds: () => [id(2)] } as unknown as MemoryStructure,
      content: { attachmentReferenceCounts: () => new Map<string, number>() } as unknown as MemoryContent,
      hasWorkspace: () => true,
      setting: () => undefined,
    });
    const attachmentId = attachments.begin(id(2), "a.bin", "application/octet-stream");
    attachments.write(attachmentId, pattern(10));
    const huge = new Proxy(new Uint8Array(0), {
      get: (target, key: string | symbol): unknown => (key === "length" ? MAX_ATTACHMENT_BYTES - 9 : Reflect.get(target, key)),
    });
    let refused: unknown;
    try {
      attachments.write(attachmentId, huge);
    } catch (error) {
      refused = error;
    }
    expect(refused).toBeInstanceOf(CoreError);
    expect((refused as CoreError).toApiError()).toEqual({
      code: "InvalidInput",
      message: "an attachment is at most 2147483648 bytes",
      details: { field: "size", limit: MAX_ATTACHMENT_BYTES },
    });
    attachments.write(attachmentId, pattern(5));
    expect(attachments.finish(attachmentId)).toMatchObject({ size: 15, chunkCount: 1 });
  });

  it("the raw commands check what the shell checks: the id header and the piece", async () => {
    const { transport, client, personal } = await setup();
    for (const call of [() => transport.attachments.write("photo", pattern(1)), () => transport.attachments.read("12")]) {
      expect((await failure(call())).toApiError()).toEqual({
        code: "InvalidInput",
        message: "the attachment id must be UUID text",
        details: { field: "x-kb-attachment" },
      });
    }
    const attachmentId = await client.attachmentBegin(personal, "a.bin", "application/octet-stream");
    expect((await failure(client.attachmentWrite(attachmentId, new Uint8Array()))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "a piece must not be empty",
      details: { field: "body" },
    });
    expect((await failure(client.attachmentWrite(attachmentId, new Uint8Array(CHUNK_SIZE + 1)))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "a piece must not be larger than one chunk",
      details: { field: "body", limit: CHUNK_SIZE },
    });
    expect(await client.attachmentFinish(attachmentId)).toMatchObject({ size: 0 });
  });
});

describe("listing, reading and deleting", () => {
  it("lists a space's attachments newest first, then by id, leaving out other spaces", async () => {
    const { client, clock, personal, work } = await setup();
    const a = await client.attachmentUpload(personal, "a.txt", "text/plain", new Blob(["a"]));
    clock.now = T0 + 1000;
    const b = await client.attachmentUpload(personal, "b.png", "image/png", new Blob(["bb"]));
    // Same millisecond: by id.
    await client.attachmentUpload(personal, "c.pdf", "application/pdf", new Blob(["ccc"]));
    await client.attachmentUpload(work, "w.txt", "text/plain", new Blob(["w"]));
    expect((await client.attachmentList(personal)).map((info) => info.name)).toEqual(["b.png", "c.pdf", "a.txt"]);
    expect(await client.attachmentGet(b.attachmentId)).toEqual({
      attachmentId: b.attachmentId,
      spaceId: personal,
      name: "b.png",
      mime: "image/png",
      size: 2,
      chunkCount: 1,
      createdAt: T0 + 1000,
      references: 0,
      unreferencedSince: T0 + 1000,
      localChunks: 1,
      availability: "Complete",
    });
    expect(a.createdAt).toBe(T0);
    expect((await failure(client.attachmentList(UNKNOWN))).toApiError()).toEqual({
      code: "NotFound",
      message: "space not found",
    });
  });

  it("reads byte ranges across chunks, and refuses empty, inverted and outside ranges", async () => {
    const { client, personal } = await setup();
    const content = pattern(2 * CHUNK_SIZE + 10);
    const { attachmentId } = await client.attachmentUpload(personal, "a.bin", "application/octet-stream", new Blob([content]));
    for (const [start, end] of [
      [0, 1],
      [CHUNK_SIZE - 3, CHUNK_SIZE + 3],
      [CHUNK_SIZE, 2 * CHUNK_SIZE],
      [5, 2 * CHUNK_SIZE + 10],
      [2 * CHUNK_SIZE + 9, 2 * CHUNK_SIZE + 10],
    ] as const) {
      const read = await client.attachmentRead(attachmentId, { start, end });
      expect(sameBytes(read, content.subarray(start, end)), `${String(start)}..${String(end)}`).toBe(true);
    }
    // The shell refuses empty and inverted ranges; the core one past the end.
    for (const range of [
      { start: 3, end: 2 },
      { start: 7, end: 7 },
    ]) {
      expect((await failure(client.attachmentRead(attachmentId, range))).toApiError()).toEqual({
        code: "InvalidInput",
        message: "the range must end after it starts",
        details: { field: "range" },
      });
    }
    expect((await failure(client.attachmentRead(attachmentId, { start: 0, end: 2 * CHUNK_SIZE + 11 }))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the byte range is outside the attachment",
      details: { field: "range" },
    });
    expect((await failure(client.attachmentRead(attachmentId, { start: -1, end: 2 }))).details).toEqual({
      field: "x-kb-range-start",
    });
  });

  it("answers at most MAX_PREVIEW_BYTES without a range, like the shell", async () => {
    const { client, transport, personal } = await setup();
    const { attachmentId } = await client.attachmentUpload(
      personal,
      "big.bin",
      "application/octet-stream",
      new Blob([new Uint8Array(MAX_PREVIEW_BYTES + 1)]),
    );
    expect((await failure(client.attachmentRead(attachmentId))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the attachment is too large to read in one piece: ask for a byte range",
      details: { field: "size", limit: MAX_PREVIEW_BYTES },
    });
    expect((await failure(client.attachmentRead(attachmentId, { start: 0, end: MAX_PREVIEW_BYTES + 1 }))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the range is longer than one read may answer; ask for it in pieces",
      details: { field: "range", limit: MAX_PREVIEW_BYTES },
    });
    expect(await client.attachmentRead(attachmentId, { start: MAX_PREVIEW_BYTES, end: MAX_PREVIEW_BYTES + 1 })).toEqual(
      new Uint8Array(1),
    );
    // Save as streams everything.
    const saved = await client.attachmentSaveAs(attachmentId, "/out/big.bin");
    expect(transport.files.get(saved.path)).toHaveLength(MAX_PREVIEW_BYTES + 1);
  });

  it("reports a missing chunk as Storage (8 hex characters of its id) and a damaged one as Crypto", async () => {
    const { client, transport, personal } = await setup();
    const { attachmentId } = await client.attachmentUpload(
      personal,
      "a.bin",
      "application/octet-stream",
      new Blob([pattern(CHUNK_SIZE + 1)]),
    );
    const [first = "", last = ""] = [...transport.blobs.keys()];
    transport.blobs.delete(last);
    expect(await client.attachmentRead(attachmentId, { start: 0, end: 10 })).toHaveLength(10);
    expect((await failure(client.attachmentRead(attachmentId))).toApiError()).toEqual({
      code: "Storage",
      message: `an attachment chunk is missing from the store (${last.slice(0, 8)})`,
    });
    transport.blobs.set(first, pattern(100));
    expect((await failure(client.attachmentRead(attachmentId, { start: 0, end: 10 }))).toApiError()).toEqual({
      code: "Crypto",
      message: "the attachment is damaged or its key is wrong",
    });
  });

  it("delete purges now, whatever references it; the block keeps its node", async () => {
    const { transport, client, events, personal, alpha } = await setup();
    const { attachmentId } = await client.attachmentUpload(personal, "p.png", "image/png", new Blob([pattern(5)]));
    transport.changePage(alpha, (doc) => {
      appendLeaf(doc, "image", { id: blockId(9), attachmentId, alt: "Kept" });
    });
    expect((await client.attachmentGet(attachmentId)).references).toBe(1);
    events.length = 0;
    await client.attachmentDelete(attachmentId);
    expect(events).toEqual([changed(personal)]);
    expect(transport.blobs.size).toBe(0);
    for (const call of [
      () => client.attachmentGet(attachmentId),
      () => client.attachmentRead(attachmentId),
      () => client.attachmentDelete(attachmentId),
    ]) {
      expect((await failure(call())).toApiError()).toEqual({ code: "NotFound", message: "attachment not found" });
    }
    const content = await client.pageContentGet(alpha);
    expect(JSON.stringify(content.content)).toContain(attachmentId);
  });
});

describe("references", () => {
  it("follow image and file blocks as documents change; AttachmentsChanged when unreferencedSince changes", async () => {
    const { transport, client, events, clock, personal, alpha, beta } = await setup();
    const { attachmentId, unreferencedSince } = await client.attachmentUpload(personal, "p.png", "image/png", new Blob(["x"]));
    expect(unreferencedSince).toBe(T0);
    events.length = 0;
    clock.now = T0 + 10;
    transport.changePage(alpha, (doc) => {
      appendLeaf(doc, "image", { id: blockId(1), attachmentId, alt: "One" });
      appendLeaf(doc, "file", { id: blockId(2), attachmentId });
    });
    // The core's bookkeeping runs before the content event.
    expect(events).toEqual([changed(personal), { type: "PageContentChanged", data: { pageId: alpha, source: "Remote" } }]);
    let info = await client.attachmentGet(attachmentId);
    expect(info.references).toBe(2);
    expect("unreferencedSince" in info).toBe(false);

    // More references, or an unrelated edit, change no mark: no event.
    events.length = 0;
    transport.changePage(beta, (doc) => {
      appendLeaf(doc, "file", { id: blockId(3), attachmentId });
    });
    transport.changePage(beta, (doc) => {
      appendLeaf(doc, "paragraph", { id: blockId(4) });
    });
    expect(events.filter((event) => event.type === "AttachmentsChanged")).toEqual([]);
    expect((await client.attachmentGet(attachmentId)).references).toBe(3);

    clock.now = T0 + 20;
    transport.changePage(alpha, (doc) => {
      removeLeaves(doc, attachmentId);
    });
    expect((await client.attachmentGet(attachmentId)).references).toBe(1);
    events.length = 0;
    transport.changePage(beta, (doc) => {
      removeLeaves(doc, attachmentId);
    });
    expect(events[0]).toEqual(changed(personal));
    info = await client.attachmentGet(attachmentId);
    expect(info.references).toBe(0);
    expect(info.unreferencedSince).toBe(T0 + 20);
  });

  it("come from the editor's updates too", async () => {
    const { transport, client, events, personal, alpha } = await setup();
    const { attachmentId } = await client.attachmentUpload(personal, "p.png", "image/png", new Blob(["x"]));
    const doc = new LoroDoc();
    configurePageDoc(doc);
    doc.import(await transport.docs.open(alpha));
    const acked = doc.oplogVersion();
    appendLeaf(doc, "image", { id: blockId(1), attachmentId, alt: "" });
    doc.commit();
    events.length = 0;
    await transport.docs.import(alpha, doc.export({ mode: "update", from: acked }));
    expect(events).toEqual([changed(personal), { type: "PageContentChanged", data: { pageId: alpha, source: "Editor" } }]);
    expect((await client.attachmentGet(attachmentId)).references).toBe(1);
  });

  it("ignore blocks without a valid block id or attachment id, and other node kinds", async () => {
    const { transport, client, personal, alpha } = await setup();
    const { attachmentId } = await client.attachmentUpload(personal, "p.png", "image/png", new Blob(["x"]));
    transport.changePage(alpha, (doc) => {
      appendLeaf(doc, "image", { attachmentId });
      appendLeaf(doc, "file", { id: "not-an-id", attachmentId });
      appendLeaf(doc, "image", { id: blockId(1), attachmentId: "photo.png" });
      appendLeaf(doc, "paragraph", { id: blockId(2), attachmentId });
    });
    expect((await client.attachmentGet(attachmentId)).references).toBe(0);
  });

  it("survive the trash, go with a purge and come back with its undo", async () => {
    const { transport, client, events, clock, personal, alpha } = await setup();
    const { attachmentId } = await client.attachmentUpload(personal, "p.png", "image/png", new Blob(["x"]));
    transport.changePage(alpha, (doc) => {
      appendLeaf(doc, "image", { id: blockId(1), attachmentId });
    });
    await client.pageTrash(alpha);
    expect((await client.attachmentGet(attachmentId)).references).toBe(1);
    clock.now = T0 + 5;
    events.length = 0;
    await client.pagePurge(alpha);
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId: personal } }, changed(personal)]);
    expect(await client.attachmentGet(attachmentId)).toMatchObject({ references: 0, unreferencedSince: T0 + 5 });
    await client.structureUndo(personal);
    expect((await client.attachmentGet(attachmentId)).references).toBe(1);
    expect((await client.attachmentGet(attachmentId)).unreferencedSince).toBeUndefined();
    await client.structureRedo(personal);
    expect((await client.attachmentGet(attachmentId)).references).toBe(0);
  });

  it("of seeded content count from the start", async () => {
    const attachmentId = id(50);
    const { client, personal } = await setup({
      spaces: [{ name: "Personal", pages: [{ title: "Alpha", content: imagePage(attachmentId) }] }],
      attachments: [{ attachmentId, name: "diagram.png", mime: "image/png", bytes: demoImagePng() }],
    });
    expect(await client.attachmentList(personal)).toEqual([
      expect.objectContaining({ attachmentId, name: "diagram.png", references: 1, spaceId: personal }),
    ]);
    expect((await client.attachmentGet(attachmentId)).unreferencedSince).toBeUndefined();
  });
});

describe("page icons that are images (ADR-0029)", () => {
  const iconOf = (attachmentId: string): string => `attachment:${attachmentId}`;
  /** Refused as `InvalidInput` for the field `icon`; the message. */
  async function iconRefusal(promise: Promise<unknown>): Promise<string> {
    const error = await failure(promise);
    expect(error.toApiError().code).toBe("InvalidInput");
    expect(error.toApiError().details).toEqual({ field: "icon" });
    return error.message;
  }

  it("keep their image referenced through set, replace, clear, trash, purge, undo and redo", async () => {
    const { client, events, clock, personal, alpha, beta } = await setup();
    const a = (await client.attachmentUpload(personal, "a.png", "image/png", new Blob(["a"]))).attachmentId;
    const b = (await client.attachmentUpload(personal, "b.png", "image/png", new Blob(["b"]))).attachmentId;
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 });
    events.length = 0;

    clock.now = T0 + 10;
    expect((await client.pageSetIcon(alpha, iconOf(a))).icon).toBe(iconOf(a));
    expect(events).toContainEqual(changed(personal));
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    expect(await client.attachmentGet(b)).toMatchObject({ references: 0, unreferencedSince: T0 });

    // One reference per page that has it as its icon.
    await client.pageSetIcon(beta, iconOf(a));
    expect((await client.attachmentGet(a)).references).toBe(2);

    // Replacing the icon moves the reference; the other page still holds the first image.
    clock.now = T0 + 20;
    await client.pageSetIcon(alpha, iconOf(b));
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    expect(await client.attachmentGet(b)).toMatchObject({ references: 1 });

    // Clearing it, or any other icon, starts the clock.
    clock.now = T0 + 30;
    events.length = 0;
    await client.pageSetIcon(beta, null);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 30 });
    expect(events).toContainEqual(changed(personal));
    clock.now = T0 + 40;
    await client.pageSetIcon(alpha, "icon:book-open:blue");
    expect(await client.attachmentGet(b)).toMatchObject({ references: 0, unreferencedSince: T0 + 40 });

    // A trashed page keeps its icon image; a purge releases it; undo and redo move it back and forth.
    clock.now = T0 + 45;
    await client.pageSetIcon(alpha, iconOf(a));
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    await client.pageTrash(alpha);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    await client.pageRestore(alpha);
    await client.pageTrash(alpha);
    clock.now = T0 + 50;
    await client.pagePurge(alpha);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 50 });
    await client.structureUndo(personal);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    await client.structureRedo(personal);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 50 });

    // The retention clock is the one of any attachment: both go once 30 days have passed.
    clock.now = T0 + 50 + 30 * DAY + 1;
    expect(await client.attachmentGc()).toMatchObject({ purged: 2 });
  });

  it("undo and redo of an icon change swap the references", async () => {
    const { client, clock, personal, alpha } = await setup();
    const a = (await client.attachmentUpload(personal, "a.png", "image/png", new Blob(["a"]))).attachmentId;
    await client.pageSetIcon(alpha, iconOf(a));
    clock.now = T0 + 5;
    await client.structureUndo(personal);
    expect((await client.pageGet(alpha)).icon).toBeUndefined();
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 5 });
    await client.structureRedo(personal);
    expect((await client.pageGet(alpha)).icon).toBe(iconOf(a));
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
  });

  it("an index rebuild keeps them, and a restored backup derives them again", async () => {
    const files = new Map<string, Uint8Array>();
    const source = await setup({ files, security: { kitCreatedAt: T0, kitKey: KEY } });
    const a = (await source.client.attachmentUpload(source.personal, "a.png", "image/png", new Blob(["a"]))).attachmentId;
    await source.client.pageSetIcon(source.alpha, iconOf(a));
    await source.client.indexRebuild();
    expect(await source.client.attachmentGet(a)).toMatchObject({ references: 1 });
    await source.client.backupExport("/backups/icons.atomikkb");

    const target = await setup({ files, workspaceName: undefined, spaces: [] });
    target.clock.now = T0 + 50;
    await target.client.backupImport("/backups/icons.atomikkb", (await kitWordsFromEntropy(KEY)).join(" "));
    expect((await target.client.pageGet(source.alpha)).icon).toBe(iconOf(a));
    expect(await target.client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await target.client.attachmentGet(a)).unreferencedSince).toBeUndefined();
  });

  it("of seeded pages count from the start", async () => {
    const attachmentId = id(50);
    const { client, personal } = await setup({
      spaces: [{ name: "Personal", pages: [{ title: "Alpha", icon: iconOf(attachmentId) }] }],
      attachments: [{ attachmentId, name: "icon.png", mime: "image/png", bytes: demoImagePng() }],
    });
    expect(await client.attachmentGet(attachmentId)).toMatchObject({ references: 1, spaceId: personal });
    expect((await client.attachmentGet(attachmentId)).unreferencedSince).toBeUndefined();
  });

  it("must be an image of the page's space, not purged; a refusal changes nothing", async () => {
    const { client, personal, work, alpha } = await setup();
    const elsewhere = (await client.attachmentUpload(work, "w.png", "image/png", new Blob(["w"]))).attachmentId;
    const pdf = (await client.attachmentUpload(personal, "r.pdf", "application/pdf", new Blob(["r"]))).attachmentId;
    const gone = (await client.attachmentUpload(personal, "g.png", "image/png", new Blob(["g"]))).attachmentId;
    await client.attachmentDelete(gone);

    expect(await iconRefusal(client.pageSetIcon(alpha, iconOf(elsewhere)))).toBe("the image belongs to another space");
    expect(await iconRefusal(client.pageSetIcon(alpha, iconOf(pdf)))).toBe(
      "the attachment is not an image (its type must start with image/)",
    );
    const unknown = "the image is not an attachment of this space (unknown or deleted)";
    expect(await iconRefusal(client.pageSetIcon(alpha, iconOf(UNKNOWN)))).toBe(unknown);
    expect(await iconRefusal(client.pageSetIcon(alpha, iconOf(gone)))).toBe(unknown);
    expect(await iconRefusal(client.pageSetIcon(alpha, "attachment:nope"))).toBe(
      "the attachment id must be a lowercase hyphenated UUID",
    );
    expect(await iconRefusal(client.pageSetIcon(alpha, "icon:Book"))).toBe(
      "the icon name may only contain a-z, 0-9 and '-'",
    );
    expect((await client.pageGet(alpha)).icon).toBeUndefined();
    expect((await client.attachmentGet(elsewhere)).references).toBe(0);
    expect((await client.attachmentGet(pdf)).references).toBe(0);

    // Any image type, in any letter case, is fine.
    for (const mime of ["image/jpeg", "image/gif", "image/svg+xml", "IMAGE/PNG"]) {
      const { attachmentId } = await client.attachmentUpload(personal, "i", mime, new Blob(["i"]));
      expect((await client.pageSetIcon(alpha, iconOf(attachmentId))).icon).toBe(iconOf(attachmentId));
    }
  });

  it("are not for spaces: a space takes an emoji or an icon of the set only", async () => {
    const { client, personal } = await setup();
    const a = (await client.attachmentUpload(personal, "a.png", "image/png", new Blob(["a"]))).attachmentId;
    expect((await client.spaceSetIcon(personal, "icon:briefcase:brown")).icon).toBe("icon:briefcase:brown");
    expect((await client.spaceCreate("Third", "icon:folder:green")).icon).toBe("icon:folder:green");
    expect(await iconRefusal(client.spaceSetIcon(personal, iconOf(a)))).toBe("a space icon cannot be an image");
    expect(await iconRefusal(client.spaceCreate("Fourth", iconOf(a)))).toBe("a space icon cannot be an image");
    expect(await iconRefusal(client.spaceCreate("Fifth", "attachment:nope"))).toBe(
      "the attachment id must be a lowercase hyphenated UUID",
    );
    expect((await client.spaceList()).map((space) => space.name)).toEqual(["Personal", "Work", "Third"]);
    expect((await client.attachmentGet(a)).references).toBe(0);
  });
});

describe("garbage collection", () => {
  async function unreferenced(options: MemoryTransportOptions = {}) {
    const ctx = await setup(options);
    const old = await ctx.client.attachmentUpload(ctx.personal, "old.bin", "application/octet-stream", new Blob([pattern(CHUNK_SIZE + 3)]));
    ctx.clock.now = T0 + 10 * DAY;
    const recent = await ctx.client.attachmentUpload(ctx.work, "recent.txt", "text/plain", new Blob(["r"]));
    const kept = await ctx.client.attachmentUpload(ctx.personal, "kept.png", "image/png", new Blob(["k"]));
    ctx.transport.changePage(ctx.alpha, (doc) => {
      appendLeaf(doc, "image", { id: blockId(1), attachmentId: kept.attachmentId });
    });
    ctx.events.length = 0;
    return { ...ctx, old, recent, kept };
  }

  it("purges what has been unreferenced for longer than the default 30 days", async () => {
    const { transport, client, events, clock, personal, old, recent, kept } = await unreferenced();
    clock.now = T0 + 30 * DAY;
    expect(await client.attachmentGc()).toEqual({ purged: 0, chunksDeleted: 0, bytesFreed: 0 });
    expect(events).toEqual([]);
    clock.now = T0 + 30 * DAY + 1;
    expect(await client.attachmentGc()).toEqual({ purged: 1, chunksDeleted: 2, bytesFreed: CHUNK_SIZE + 3 });
    expect(events).toEqual([changed(personal)]);
    expect((await failure(client.attachmentGet(old.attachmentId))).code).toBe("NotFound");
    await expect(client.attachmentGet(recent.attachmentId)).resolves.toMatchObject({ references: 0 });
    await expect(client.attachmentGet(kept.attachmentId)).resolves.toMatchObject({ references: 1 });
    expect(transport.blobs.size).toBe(2);
  });

  it("reads the retention setting as days, fractions allowed; <= 0 keeps everything; anything else means the default", async () => {
    const { client, clock, events, personal, work, old, recent } = await unreferenced({
      settings: { [RETENTION_SETTING]: 7 },
    });
    clock.now = T0 + 17 * DAY + 1;
    expect(await client.attachmentGc()).toMatchObject({ purged: 2 });
    // One event per purge, the longest unreferenced first.
    expect(events).toEqual([changed(personal), changed(work)]);
    expect((await failure(client.attachmentGet(old.attachmentId))).code).toBe("NotFound");
    expect((await failure(client.attachmentGet(recent.attachmentId))).code).toBe("NotFound");

    const half = await unreferenced({ settings: { [RETENTION_SETTING]: 0.5 } });
    half.clock.now = T0 + 10 * DAY + DAY / 2;
    expect(await half.client.attachmentGc()).toMatchObject({ purged: 1 });
    half.clock.now += 1;
    expect(await half.client.attachmentGc()).toMatchObject({ purged: 1 });

    for (const never of [0, -3]) {
      const ctx = await unreferenced({ settings: { [RETENTION_SETTING]: never } });
      ctx.clock.now = T0 + 10_000 * DAY;
      expect(await ctx.client.attachmentGc()).toEqual({ purged: 0, chunksDeleted: 0, bytesFreed: 0 });
    }

    const text = await unreferenced({ settings: { [RETENTION_SETTING]: "7" } });
    text.clock.now = T0 + 17 * DAY + 1;
    expect(await text.client.attachmentGc()).toMatchObject({ purged: 0 });
    await text.client.settingsSet(RETENTION_SETTING, 1);
    expect(await text.client.attachmentGc()).toMatchObject({ purged: 2 });
  });

  it("does not collect an attachment whose page is in the trash", async () => {
    const { transport, client, clock, personal, alpha } = await setup();
    const { attachmentId } = await client.attachmentUpload(personal, "p.png", "image/png", new Blob(["x"]));
    transport.changePage(alpha, (doc) => {
      appendLeaf(doc, "image", { id: blockId(1), attachmentId });
    });
    await client.pageTrash(alpha);
    clock.now = T0 + 100 * DAY;
    expect(await client.attachmentGc()).toMatchObject({ purged: 0 });
    await client.pageRestore(alpha);
    expect((await client.attachmentGet(attachmentId)).references).toBe(1);
  });

  it("sweeps chunks no manifest claims, leaving uploads in progress alone", async () => {
    const { transport, client, personal } = await setup();
    const uploading = await client.attachmentBegin(personal, "u.bin", "application/octet-stream");
    await writeAll(client, uploading, pattern(CHUNK_SIZE + 1));
    transport.blobs.set("ff".repeat(32), pattern(40));
    expect(await client.attachmentGc()).toEqual({ purged: 0, chunksDeleted: 1, bytesFreed: 40 });
    expect(transport.blobs.size).toBe(1);
    await expect(client.attachmentFinish(uploading)).resolves.toMatchObject({ chunkCount: 2 });
  });

  it("runs at open: unlocking opens the workspace again", async () => {
    const { transport, client, clock, events, personal } = await setup({ security: { appLockPassphrase: "correct horse" } });
    const { attachmentId } = await client.attachmentUpload(personal, "a.txt", "text/plain", new Blob(["a"]));
    transport.blobs.set("ee".repeat(32), pattern(4));
    await client.lock();
    clock.now = T0 + 31 * DAY;
    events.length = 0;
    await client.unlock("correct horse");
    expect(events).toEqual([changed(personal)]);
    expect((await failure(client.attachmentGet(attachmentId))).code).toBe("NotFound");
    expect(transport.blobs.size).toBe(0);
  });
});

describe("the locked core and failures", () => {
  it("refuses every attachment call with Locked while locked", async () => {
    const { transport, client, personal } = await setup({ security: { appLockPassphrase: "correct horse" } });
    const { attachmentId } = await client.attachmentUpload(personal, "a.txt", "text/plain", new Blob(["a"]));
    await client.lock();
    const calls = [
      () => client.attachmentBegin(personal, "b.txt", "text/plain"),
      () => client.attachmentList(personal),
      () => client.attachmentGet(attachmentId),
      () => client.attachmentGc(),
      () => transport.attachments.write(attachmentId, pattern(1)),
      () => transport.attachments.read(attachmentId),
      () => transport.attachments.importPath(personal, "/in/a.txt"),
      () => transport.attachments.saveAs(attachmentId, "/out/a.txt"),
      () => transport.attachments.open(attachmentId),
    ];
    for (const call of calls) {
      expect((await failure(call())).toApiError()).toEqual({ code: "Locked", message: "the workspace is locked" });
    }
    await client.unlock("correct horse");
    await expect(client.attachmentRead(attachmentId)).resolves.toEqual(new TextEncoder().encode("a"));
  });

  it("failNextAttachments makes the next transport calls fail, and an upload aborts", async () => {
    const { transport, client, personal } = await setup();
    transport.failNextAttachments({ code: "Storage", message: "disk full" });
    expect(
      (await failure(client.attachmentUpload(personal, "a.txt", "text/plain", new Blob(["abc"])))).toApiError(),
    ).toEqual({ code: "Storage", message: "disk full" });
    expect(await client.attachmentList(personal)).toEqual([]);
    expect(transport.blobs.size).toBe(0);
  });
});

describe("files (importPath, saveAs, open)", () => {
  it("import a file from files, named after it, with a MIME type from its extension", async () => {
    const { transport, client, events, personal } = await setup();
    const content = pattern(CHUNK_SIZE + 2);
    transport.files.set("C:\\Users\\me\\Pictures\\Holiday.JPG", content);
    events.length = 0;
    const info = await client.attachmentImportPath(personal, "C:\\Users\\me\\Pictures\\Holiday.JPG");
    expect(info).toMatchObject({ name: "Holiday.JPG", mime: "image/jpeg", size: CHUNK_SIZE + 2, chunkCount: 2 });
    expect(events).toEqual([changed(personal)]);
    expect(sameBytes(await client.attachmentRead(info.attachmentId), content)).toBe(true);

    transport.files.set("/home/me/notes", pattern(3));
    expect(await client.attachmentImportPath(personal, "/home/me/notes")).toMatchObject({
      name: "notes",
      mime: "application/octet-stream",
    });
  });

  it("import through the open dialog picks the most recent file; cancelling or a missing file is InvalidInput for path", async () => {
    const { transport, client, personal } = await setup();
    expect((await failure(client.attachmentImportPath(personal))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "no file was chosen",
      details: { field: "path" },
    });
    transport.files.set("/a/first.txt", pattern(1));
    transport.files.set("/a/second.pdf", pattern(2));
    expect(await client.attachmentImportPath(personal)).toMatchObject({ name: "second.pdf", mime: "application/pdf" });
    transport.setCancelDialogs(true);
    expect((await failure(client.attachmentImportPath(personal))).details).toEqual({ field: "path" });
    expect((await failure(client.attachmentImportPath(personal, "/a/missing.txt"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the file cannot be opened: no such file",
      details: { field: "path" },
    });
    expect((await failure(client.attachmentImportPath(personal, "relative.txt"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the path must be absolute",
      details: { field: "path" },
    });
  });

  it("import names the attachment like the shell: unusable characters out, long names shortened", async () => {
    const { transport, client, personal } = await setup();
    const long = `${"n".repeat(300)}.pdf`;
    transport.files.set(`/a/${long}`, pattern(5));
    const info = await client.attachmentImportPath(personal, `/a/${long}`);
    expect(info.name).toBe(`${"n".repeat(251)}.pdf`);
    expect(info.mime).toBe("application/pdf");
    transport.files.set("/a/tab\there.txt", pattern(1));
    expect((await client.attachmentImportPath(personal, "/a/tab\there.txt")).name).toBe("tabhere.txt");
  });

  it("import checks the size, the name and the space; a failure leaves nothing behind", async () => {
    const { transport, client, events, personal } = await setup();
    transport.files.set("/a/x.txt", pattern(5));
    expect((await failure(client.attachmentImportPath(UNKNOWN, "/a/x.txt"))).code).toBe("NotFound");
    transport.files.set("/a/   ", pattern(5));
    expect((await failure(client.attachmentImportPath(personal, "/a/   "))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the attachment needs a name",
      details: { field: "name" },
    });
    // A file above 2 GiB, without allocating it.
    const huge = new Proxy(new Uint8Array(0), {
      get: (target, key: string | symbol): unknown => (key === "length" ? MAX_ATTACHMENT_BYTES + 1 : Reflect.get(target, key)),
    });
    transport.files.set("/a/huge.bin", huge);
    expect((await failure(client.attachmentImportPath(personal, "/a/huge.bin"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the file is larger than an attachment may be",
      details: { field: "size", limit: MAX_ATTACHMENT_BYTES },
    });
    expect(events).toEqual([]);
    expect(transport.blobs.size).toBe(0);
  });

  it("save writes the bytes to files, through the save dialog without a path", async () => {
    const { transport, client, personal } = await setup();
    const content = pattern(12);
    const { attachmentId } = await client.attachmentUpload(personal, "CON.txt", "text/plain", new Blob([content]));
    const saved = await client.attachmentSaveAs(attachmentId);
    expect(saved).toEqual({ info: await client.attachmentGet(attachmentId), path: "/downloads/_CON.txt" });
    expect(transport.files.get("/downloads/_CON.txt")).toEqual(content);
    await expect(client.attachmentSaveAs(attachmentId, "D:\\backup\\copy.txt")).resolves.toMatchObject({
      path: "D:\\backup\\copy.txt",
    });
    expect(transport.files.get("D:\\backup\\copy.txt")).toEqual(content);
    transport.setCancelDialogs(true);
    expect((await failure(client.attachmentSaveAs(attachmentId))).details).toEqual({ field: "path" });
    expect((await failure(client.attachmentSaveAs(UNKNOWN, "/out/x"))).code).toBe("NotFound");
  });

  it("open answers the path of a private copy, and refuses files the system would run", async () => {
    const { client, personal } = await setup();
    const { attachmentId } = await client.attachmentUpload(personal, "report?.pdf", "application/pdf", new Blob(["%PDF"]));
    await expect(client.attachmentOpen(attachmentId)).resolves.toBe(`/cache/open/${attachmentId}/report_.pdf`);
    expect((await failure(client.attachmentOpen(UNKNOWN))).code).toBe("NotFound");
    const program = await client.attachmentUpload(personal, "setup.EXE", "application/octet-stream", new Blob(["MZ"]));
    expect((await failure(client.attachmentOpen(program.attachmentId))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "files of this type are not opened from here: save the file and open it yourself",
      details: { field: "attachment" },
    });
  });
});

describe("search, Markdown, keyring, spaces and backups", () => {
  it("search finds an image's alt text (not a file's name); Markdown links both", async () => {
    const { transport, client, personal, alpha } = await setup();
    const image = await client.attachmentUpload(personal, "diagram.png", "image/png", new Blob(["i"]));
    const file = await client.attachmentUpload(personal, "Quarterly report.pdf", "application/pdf", new Blob(["f"]));
    transport.changePage(alpha, (doc) => {
      appendLeaf(doc, "image", { id: blockId(1), attachmentId: image.attachmentId, alt: "Sync flow overview" });
      appendLeaf(doc, "file", { id: blockId(2), attachmentId: file.attachmentId });
    });
    expect((await client.search("overview")).map((hit) => hit.blockId)).toEqual([blockId(1)]);
    // The core indexes a file block's text, which is none.
    expect(await client.search("quarterly")).toEqual([]);
    // The page starts with the empty paragraph every new page document has.
    expect(await client.pageExportMarkdown(alpha)).toBe(
      `\n\n![Sync flow overview](attachment://${image.attachmentId})\n\n[Quarterly report.pdf](attachment://${file.attachmentId})\n`,
    );
    await client.attachmentDelete(file.attachmentId);
    expect(await client.pageExportMarkdown(alpha)).toContain(`[${file.attachmentId}](attachment://${file.attachmentId})`);
  });

  it("the keyring counts one key per attachment and a tombstone per purge", async () => {
    const { client, personal } = await setup();
    const a = await client.attachmentUpload(personal, "a.txt", "text/plain", new Blob(["a"]));
    await client.attachmentUpload(personal, "b.txt", "text/plain", new Blob(["b"]));
    expect((await client.securityStatus()).keyring).toMatchObject({ attachments: 2, tombstones: 0 });
    await client.attachmentDelete(a.attachmentId);
    expect((await client.securityStatus()).keyring).toMatchObject({ attachments: 1, tombstones: 1 });
  });

  it("deleting a space purges its attachments first; an upload into it can no longer finish", async () => {
    const { transport, client, events, work } = await setup();
    await client.attachmentUpload(work, "w.txt", "text/plain", new Blob(["w"]));
    const uploading = await client.attachmentBegin(work, "u.bin", "application/octet-stream");
    await writeAll(client, uploading, pattern(CHUNK_SIZE + 1));
    events.length = 0;
    await client.spaceDelete(work);
    expect(events).toEqual([changed(work), { type: "SpacesChanged" }]);
    expect((await client.securityStatus()).keyring).toMatchObject({ attachments: 0, tombstones: 0 });
    expect(transport.blobs.size).toBe(1);
    expect((await failure(client.attachmentFinish(uploading))).toApiError()).toEqual({
      code: "NotFound",
      message: "space not found",
    });
    expect(transport.blobs.size).toBe(0);
  });

  it("backups carry the attachments with their chunks and restore them", async () => {
    const files = new Map<string, Uint8Array>();
    const source = await setup({ files, security: { kitCreatedAt: T0, kitKey: KEY } });
    const image = demoImagePng();
    const big = pattern(CHUNK_SIZE + 7);
    const shown = await source.client.attachmentUpload(source.personal, "dot.png", "image/png", new Blob([image]));
    const loose = await source.client.attachmentUpload(source.work, "big.bin", "application/octet-stream", new Blob([big]));
    const gone = await source.client.attachmentUpload(source.work, "gone.txt", "text/plain", new Blob(["g"]));
    await source.client.attachmentDelete(gone.attachmentId);
    source.transport.changePage(source.alpha, (doc) => {
      appendLeaf(doc, "image", { id: blockId(1), attachmentId: shown.attachmentId, alt: "dot" });
    });
    const exported = await source.client.backupExport("/backups/b.atomikkb");
    expect(exported.summary).toMatchObject({ chunks: 3, chunkBytes: image.length + big.length });

    const target = await setup({ files, workspaceName: undefined, spaces: [] });
    target.clock.now = T0 + 50;
    const restored = await target.client.backupImport("/backups/b.atomikkb", (await kitWordsFromEntropy(KEY)).join(" "));
    expect(restored.summary).toMatchObject({ chunks: 3, chunkBytes: image.length + big.length });
    expect(await target.client.attachmentGet(shown.attachmentId)).toEqual(
      await source.client.attachmentGet(shown.attachmentId),
    );
    // The index is derived again: an unreferenced attachment is unreferenced since the restore.
    expect(await target.client.attachmentGet(loose.attachmentId)).toEqual({
      ...(await source.client.attachmentGet(loose.attachmentId)),
      unreferencedSince: T0 + 50,
    });
    expect(await target.client.attachmentRead(shown.attachmentId)).toEqual(image);
    const tail = await target.client.attachmentRead(loose.attachmentId, { start: CHUNK_SIZE, end: CHUNK_SIZE + 7 });
    expect(tail).toEqual(big.subarray(CHUNK_SIZE));
    expect((await target.client.securityStatus()).keyring).toMatchObject({ attachments: 2, tombstones: 1 });
    // New uploads after a restore do not collide with the restored chunks.
    await target.client.attachmentUpload(source.personal, "new.txt", "text/plain", new Blob(["n"]));
    expect(await target.client.attachmentRead(shown.attachmentId)).toEqual(image);
  });
});

describe("the demo image", () => {
  it("is seeded like the dev page does it, and reads back as a PNG", async () => {
    const attachmentId = id(60);
    const { client, personal } = await setup({
      spaces: [{ name: "Personal", pages: [{ title: "Welcome", content: imagePage(attachmentId, "A teal square") }] }],
      attachments: [{ attachmentId, name: "demo.png", mime: "image/png", bytes: demoImagePng() }],
    });
    const [info] = await client.attachmentList(personal);
    expect(info).toMatchObject({ attachmentId, mime: "image/png", references: 1 });
    const bytes = await client.attachmentRead(attachmentId);
    expect(Array.from(bytes.subarray(0, 8))).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    expect((await client.search("teal")).map((hit) => hit.blockId)).toEqual([blockId(2)]);
  });

  it("seeds are checked like uploads", async () => {
    await expect(setup({ attachments: [{ name: "a/b", mime: "text/plain", bytes: pattern(1) }] })).rejects.toThrow(
      CoreError,
    );
    await expect(
      setup({ spaces: [], attachments: [{ name: "a", mime: "text/plain", bytes: pattern(1) }] }),
    ).rejects.toThrow("MemoryTransport: seeded attachments need a space");
  });
});
