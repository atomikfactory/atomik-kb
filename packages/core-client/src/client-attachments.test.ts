import { describe, expect, it, vi } from "vitest";

import { CHUNK_SIZE, MAX_ATTACHMENT_BYTES } from "./attachments";
import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { AttachmentTransport, DocTransport, Transport } from "./transport";

const SPACE = "22222222-2222-4222-8222-222222222222";
const ATTACHMENT = "33333333-3333-4333-8333-333333333333";

const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

const info: AttachmentInfo = {
  attachmentId: ATTACHMENT,
  spaceId: SPACE,
  name: "photo.png",
  mime: "image/png",
  size: 5,
  chunkCount: 1,
  createdAt: 1_700_000_000_000,
  references: 0,
  unreferencedSince: 1_700_000_000_000,
  localChunks: 1,
  availability: "Complete",
};

/** A core that answers the upload requests and records the requests and the bytes written. */
function uploadCore(options: { failWriteAt?: number; failFinish?: boolean } = {}) {
  const requests: Request[] = [];
  const written: Uint8Array[] = [];
  const dispatch = vi.fn((request: Request): Promise<Response> => {
    requests.push(request);
    switch (request.type) {
      case "AttachmentBegin":
        return Promise.resolve({ type: "AttachmentStarted", data: ATTACHMENT });
      case "AttachmentFinish":
        return options.failFinish === true
          ? Promise.reject(new CoreError({ code: "Locked", message: "the workspace is locked" }))
          : Promise.resolve({
              type: "Attachment",
              data: { ...info, size: written.reduce((sum, piece) => sum + piece.length, 0) },
            });
      case "AttachmentAbort":
        return Promise.resolve({ type: "Ok" });
      default:
        return Promise.reject(new Error(`unexpected ${request.type}`));
    }
  });
  // The spies themselves (asserting on methods of an `AttachmentTransport` would unbind them).
  const attachments = {
    write: vi.fn((attachmentId: string, bytes: Uint8Array) => {
      expect(attachmentId).toBe(ATTACHMENT);
      if (written.length === options.failWriteAt) {
        return Promise.reject(new CoreError({ code: "Storage", message: "disk full" }));
      }
      written.push(bytes.slice());
      return Promise.resolve();
    }),
    read: vi.fn(() => Promise.resolve(new Uint8Array([1, 2]))),
    importPath: vi.fn(() => Promise.resolve(info)),
    saveAs: vi.fn(() => Promise.resolve({ info, path: "C:/out/photo.png" })),
    open: vi.fn(() => Promise.resolve("C:/cache/open/photo.png")),
  } satisfies AttachmentTransport;
  const transport: Transport = { dispatch, subscribe: () => () => undefined, docs: noDocs, attachments };
  return { client: new CoreClient(transport), requests, written, attachments };
}

/** A transport that answers every request with one canned response. */
function stubTransport(response: Response, extras: Partial<Transport> = {}) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
    ...extras,
  };
  return { client: new CoreClient(transport), requests };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

describe("CoreClient attachment requests", () => {
  it("send the request kinds of the contract and return their payloads", async () => {
    let stub = stubTransport({ type: "AttachmentStarted", data: ATTACHMENT });
    await expect(stub.client.attachmentBegin(SPACE, "a.txt", "text/plain")).resolves.toBe(ATTACHMENT);
    expect(stub.requests).toEqual([
      { type: "AttachmentBegin", data: { spaceId: SPACE, name: "a.txt", mime: "text/plain" } },
    ]);

    stub = stubTransport({ type: "Attachment", data: info });
    await expect(stub.client.attachmentFinish(ATTACHMENT)).resolves.toEqual(info);
    await expect(stub.client.attachmentGet(ATTACHMENT)).resolves.toEqual(info);
    expect(stub.requests).toEqual([
      { type: "AttachmentFinish", data: { attachmentId: ATTACHMENT } },
      { type: "AttachmentGet", data: { attachmentId: ATTACHMENT } },
    ]);

    stub = stubTransport({ type: "Ok" });
    await expect(stub.client.attachmentAbort(ATTACHMENT)).resolves.toBeUndefined();
    await expect(stub.client.attachmentDelete(ATTACHMENT)).resolves.toBeUndefined();
    expect(stub.requests).toEqual([
      { type: "AttachmentAbort", data: { attachmentId: ATTACHMENT } },
      { type: "AttachmentDelete", data: { attachmentId: ATTACHMENT } },
    ]);

    stub = stubTransport({ type: "Attachments", data: [info] });
    await expect(stub.client.attachmentList(SPACE)).resolves.toEqual([info]);
    expect(stub.requests).toEqual([{ type: "AttachmentList", data: { spaceId: SPACE } }]);

    stub = stubTransport({ type: "AttachmentGc", data: { purged: 1, chunksDeleted: 3, bytesFreed: 9000 } });
    await expect(stub.client.attachmentGc()).resolves.toEqual({ purged: 1, chunksDeleted: 3, bytesFreed: 9000 });
    expect(stub.requests).toEqual([{ type: "AttachmentGc" }]);
  });

  it("attachmentDownload sends AttachmentDownload and returns the attachment, availability included", async () => {
    const downloading: AttachmentInfo = { ...info, chunkCount: 3, localChunks: 1, availability: "Downloading" };
    const stub = stubTransport({ type: "Attachment", data: downloading });
    await expect(stub.client.attachmentDownload(ATTACHMENT)).resolves.toEqual(downloading);
    expect(stub.requests).toEqual([{ type: "AttachmentDownload", data: { attachmentId: ATTACHMENT } }]);

    const wrong = stubTransport({ type: "Ok" });
    expect((await failure(wrong.client.attachmentDownload(ATTACHMENT))).toApiError()).toEqual({
      code: "Internal",
      message: "Unexpected response to AttachmentDownload: expected Attachment, got Ok",
    });
  });

  it("attachmentRead of an incomplete attachment rejects with the core's Storage error, unchanged", async () => {
    const { client, attachments } = uploadCore();
    const notHere = { code: "Storage", message: "the attachment is not complete on this device (1 of 3 chunks)" } as const;
    attachments.read.mockImplementationOnce(() => Promise.reject(new CoreError(notHere)));
    expect((await failure(client.attachmentRead(ATTACHMENT))).toApiError()).toEqual(notHere);
  });

  it("work on a transport without attachment bytes, and check the response variant", async () => {
    const { client } = stubTransport({ type: "Ok" });
    expect(client.hasAttachments()).toBe(false);
    await expect(client.attachmentGet(ATTACHMENT)).rejects.toMatchObject({
      code: "Internal",
      message: "Unexpected response to AttachmentGet: expected Attachment, got Ok",
    });
  });
});

describe("CoreClient attachment bytes", () => {
  it("hasAttachments wants all five methods", () => {
    const { attachments } = uploadCore();
    expect(stubTransport({ type: "Ok" }, { attachments }).client.hasAttachments()).toBe(true);
    for (const missing of ["write", "read", "importPath", "saveAs", "open"] as const) {
      const partial: Partial<AttachmentTransport> = { ...attachments, [missing]: undefined };
      const transport = { attachments: partial } as Partial<Transport>;
      expect(stubTransport({ type: "Ok" }, transport).client.hasAttachments(), missing).toBe(false);
    }
  });

  it("reject with Internal when the transport has no attachment methods, before anything is sent", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    const calls: [string, () => Promise<unknown>][] = [
      ["attachmentWrite", () => client.attachmentWrite(ATTACHMENT, new Uint8Array(1))],
      ["attachmentUpload", () => client.attachmentUpload(SPACE, "a.txt", "text/plain", new Blob(["a"]))],
      ["attachmentRead", () => client.attachmentRead(ATTACHMENT)],
      ["attachmentImportPath", () => client.attachmentImportPath(SPACE)],
      ["attachmentImportUrl", () => client.attachmentImportUrl(SPACE, "https://example.com/a.png")],
      ["attachmentSaveAs", () => client.attachmentSaveAs(ATTACHMENT)],
      ["attachmentOpen", () => client.attachmentOpen(ATTACHMENT)],
    ];
    for (const [method, call] of calls) {
      expect((await failure(call())).toApiError()).toEqual({
        code: "Internal",
        message: `${method} is not supported by this transport`,
      });
    }
    expect(requests).toEqual([]);
  });

  it("pass reads, writes and the file commands through to the transport", async () => {
    const { client, attachments } = uploadCore();
    await expect(client.attachmentRead(ATTACHMENT, { start: 1, end: 3 })).resolves.toEqual(new Uint8Array([1, 2]));
    expect(attachments.read).toHaveBeenCalledWith(ATTACHMENT, { start: 1, end: 3 });
    await client.attachmentRead(ATTACHMENT);
    expect(attachments.read).toHaveBeenLastCalledWith(ATTACHMENT, undefined);
    await client.attachmentWrite(ATTACHMENT, new Uint8Array([9]));
    expect(attachments.write).toHaveBeenCalledWith(ATTACHMENT, new Uint8Array([9]));
    await expect(client.attachmentImportPath(SPACE, "C:/in/photo.png")).resolves.toEqual(info);
    expect(attachments.importPath).toHaveBeenCalledWith(SPACE, "C:/in/photo.png");
    await expect(client.attachmentSaveAs(ATTACHMENT)).resolves.toEqual({ info, path: "C:/out/photo.png" });
    expect(attachments.saveAs).toHaveBeenCalledWith(ATTACHMENT, undefined);
    await expect(client.attachmentOpen(ATTACHMENT)).resolves.toBe("C:/cache/open/photo.png");
  });
});

describe("CoreClient image from a link (ADR-0028)", () => {
  const link = "https://example.com/photos/photo.png";

  it("canImportUrl is false without importUrl, and does not change what hasAttachments says", () => {
    const { attachments } = uploadCore();
    const without = stubTransport({ type: "Ok" }, { attachments }).client;
    expect(without.hasAttachments()).toBe(true);
    expect(without.canImportUrl()).toBe(false);

    const importUrl = vi.fn(() => Promise.resolve(info));
    const withIt = stubTransport({ type: "Ok" }, { attachments: { ...attachments, importUrl } }).client;
    expect(withIt.hasAttachments()).toBe(true);
    expect(withIt.canImportUrl()).toBe(true);

    // importUrl alone, without the rest of the attachment methods, is not enough.
    const partial: Partial<AttachmentTransport> = { importUrl };
    const alone = stubTransport({ type: "Ok" }, { attachments: partial } as Partial<Transport>).client;
    expect(alone.hasAttachments()).toBe(false);
    expect(alone.canImportUrl()).toBe(false);
    expect(stubTransport({ type: "Ok" }).client.canImportUrl()).toBe(false);
  });

  it("attachmentImportUrl passes the space and the link to the transport and returns its attachment", async () => {
    const { attachments } = uploadCore();
    const importUrl = vi.fn<(spaceId: string, url: string) => Promise<AttachmentInfo>>(() => Promise.resolve(info));
    const { client, requests } = stubTransport({ type: "Ok" }, { attachments: { ...attachments, importUrl } });
    await expect(client.attachmentImportUrl(SPACE, link)).resolves.toEqual(info);
    expect(importUrl).toHaveBeenCalledExactlyOnceWith(SPACE, link);
    // The link goes to the shell only: nothing is dispatched to the core.
    expect(requests).toEqual([]);
  });

  it("attachmentImportUrl passes the transport's failure on unchanged", async () => {
    const { attachments } = uploadCore();
    const refused = {
      code: "InvalidInput",
      message: "the link is not an image",
      details: { field: "url", reason: "notImage" },
    } as const;
    const importUrl = vi.fn(() => Promise.reject(new CoreError(refused)));
    const { client } = stubTransport({ type: "Ok" }, { attachments: { ...attachments, importUrl } });
    expect((await failure(client.attachmentImportUrl(SPACE, link))).toApiError()).toEqual(refused);
  });

  it("attachmentImportUrl is Internal 'not supported' on a transport that has attachments but no importUrl", async () => {
    const { attachments } = uploadCore();
    const { client } = stubTransport({ type: "Ok" }, { attachments });
    expect((await failure(client.attachmentImportUrl(SPACE, link))).toApiError()).toEqual({
      code: "Internal",
      message: "attachmentImportUrl is not supported by this transport",
    });
  });
});

describe("CoreClient.attachmentUpload", () => {
  it("begins, writes a Blob in CHUNK_SIZE pieces in order, finishes and reports progress", async () => {
    const { client, requests, written } = uploadCore();
    const content = new Uint8Array(2 * CHUNK_SIZE + 5).map((_, i) => (i * 7) % 256);
    const progress: [number, number | undefined][] = [];
    const result = await client.attachmentUpload(SPACE, "big.bin", "application/octet-stream", new Blob([content]), (sent, total) =>
      progress.push([sent, total]),
    );
    expect(result.size).toBe(content.length);
    expect(requests.map((request) => request.type)).toEqual(["AttachmentBegin", "AttachmentFinish"]);
    expect(requests[0]).toEqual({
      type: "AttachmentBegin",
      data: { spaceId: SPACE, name: "big.bin", mime: "application/octet-stream" },
    });
    expect(written.map((piece) => piece.length)).toEqual([CHUNK_SIZE, CHUNK_SIZE, 5]);
    expect(written.flatMap((piece) => [...piece.subarray(0, 3)])).toEqual([
      ...content.subarray(0, 3),
      ...content.subarray(CHUNK_SIZE, CHUNK_SIZE + 3),
      ...content.subarray(2 * CHUNK_SIZE, 2 * CHUNK_SIZE + 3),
    ]);
    const total = content.length;
    expect(progress).toEqual([
      [0, total],
      [CHUNK_SIZE, total],
      [2 * CHUNK_SIZE, total],
      [total, total],
    ]);
  });

  it("re-slices a stream whatever sizes it yields; the total is unknown", async () => {
    const { client, written } = uploadCore();
    async function* pieces(): AsyncGenerator<Uint8Array> {
      await Promise.resolve();
      yield new Uint8Array(10);
      yield new Uint8Array(CHUNK_SIZE);
      yield new Uint8Array(3);
    }
    const progress: [number, number | undefined][] = [];
    await client.attachmentUpload(SPACE, "s.bin", "application/octet-stream", pieces(), (sent, total) =>
      progress.push([sent, total]),
    );
    expect(written.map((piece) => piece.length)).toEqual([CHUNK_SIZE, 13]);
    expect(progress).toEqual([
      [0, undefined],
      [CHUNK_SIZE, undefined],
      [CHUNK_SIZE + 13, undefined],
    ]);
  });

  it("uploads an empty file as begin and finish without a write", async () => {
    const { client, requests, written } = uploadCore();
    await expect(client.attachmentUpload(SPACE, "empty.txt", "text/plain", new Blob([]))).resolves.toMatchObject({
      size: 0,
    });
    expect(written).toEqual([]);
    expect(requests.map((request) => request.type)).toEqual(["AttachmentBegin", "AttachmentFinish"]);
  });

  it("checks the name, the MIME type and a Blob's size before anything is sent", async () => {
    const { client, requests, written } = uploadCore();
    expect(await failure(client.attachmentUpload(SPACE, "a/b", "text/plain", new Blob(["x"])))).toMatchObject({
      code: "InvalidInput",
      details: { field: "name" },
    });
    expect(await failure(client.attachmentUpload(SPACE, "a.txt", "text", new Blob(["x"])))).toMatchObject({
      code: "InvalidInput",
      details: { field: "mime" },
    });
    // A 2 GiB + 1 byte Blob, without allocating it.
    const huge = Object.create(Blob.prototype) as Blob;
    Object.defineProperty(huge, "size", { value: MAX_ATTACHMENT_BYTES + 1 });
    expect((await failure(client.attachmentUpload(SPACE, "a.bin", "application/x-big", huge))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "an attachment is at most 2147483648 bytes",
      details: { field: "size", limit: MAX_ATTACHMENT_BYTES },
    });
    expect(requests).toEqual([]);
    expect(written).toEqual([]);
  });

  it("aborts when a write fails and rejects with that failure", async () => {
    const { client, requests, written } = uploadCore({ failWriteAt: 1 });
    const error = await failure(
      client.attachmentUpload(SPACE, "big.bin", "application/octet-stream", new Blob([new Uint8Array(CHUNK_SIZE + 1)])),
    );
    expect(error.toApiError()).toEqual({ code: "Storage", message: "disk full" });
    expect(written).toHaveLength(1);
    expect(requests.map((request) => request.type)).toEqual(["AttachmentBegin", "AttachmentAbort"]);
    expect(requests[1]).toEqual({ type: "AttachmentAbort", data: { attachmentId: ATTACHMENT } });
  });

  it("aborts when finish fails, when the source fails and when the progress callback throws", async () => {
    let core = uploadCore({ failFinish: true });
    expect(await failure(core.client.attachmentUpload(SPACE, "a.txt", "text/plain", new Blob(["abc"])))).toMatchObject({
      code: "Locked",
    });
    expect(core.requests.map((request) => request.type)).toEqual(["AttachmentBegin", "AttachmentFinish", "AttachmentAbort"]);

    core = uploadCore();
    async function* broken(): AsyncGenerator<Uint8Array> {
      await Promise.resolve();
      yield new Uint8Array(3);
      throw new Error("the file went away");
    }
    expect(
      (await failure(core.client.attachmentUpload(SPACE, "a.txt", "text/plain", broken()))).toApiError(),
    ).toEqual({ code: "Internal", message: "the file went away" });
    expect(core.requests.map((request) => request.type)).toEqual(["AttachmentBegin", "AttachmentAbort"]);

    core = uploadCore();
    const error = await failure(
      core.client.attachmentUpload(SPACE, "a.txt", "text/plain", new Blob(["abc"]), (sent) => {
        if (sent > 0) {
          throw new Error("the view is gone");
        }
      }),
    );
    expect(error.message).toBe("the view is gone");
    expect(core.requests.map((request) => request.type)).toEqual(["AttachmentBegin", "AttachmentAbort"]);
  });

  it("keeps the original failure when the abort fails too", async () => {
    const { client, requests } = uploadCore({ failWriteAt: 0 });
    const original = client.attachmentAbort.bind(client);
    const abort = vi.spyOn(client, "attachmentAbort").mockImplementation(async (id) => {
      await original(id);
      throw new CoreError({ code: "NotFound", message: "attachment not found" });
    });
    expect(await failure(client.attachmentUpload(SPACE, "a.txt", "text/plain", new Blob(["abc"])))).toMatchObject({
      code: "Storage",
    });
    expect(abort).toHaveBeenCalledOnce();
    expect(requests.map((request) => request.type)).toEqual(["AttachmentBegin", "AttachmentAbort"]);
  });

  it("does not abort when begin itself fails", async () => {
    const requests: Request[] = [];
    const { attachments } = uploadCore();
    const transport: Transport = {
      dispatch: (request) => {
        requests.push(request);
        return Promise.reject(new CoreError({ code: "NotFound", message: "space not found" }));
      },
      subscribe: () => () => undefined,
      docs: noDocs,
      attachments,
    };
    expect(
      await failure(new CoreClient(transport).attachmentUpload(SPACE, "a.txt", "text/plain", new Blob(["abc"]))),
    ).toMatchObject({ code: "NotFound" });
    expect(requests.map((request) => request.type)).toEqual(["AttachmentBegin"]);
    expect(attachments.write).not.toHaveBeenCalled();
  });
});
