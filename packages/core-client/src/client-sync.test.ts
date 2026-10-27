// `CoreClient`'s sync requests (Milestone 6): what each sends, what it returns, and the checks
// `syncConfigure` makes before anything reaches the transport.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SyncInfo } from "./generated/SyncInfo";
import type { DocTransport, Transport } from "./transport";

/** Page documents are not used by these tests; touching them is a test bug. */
const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response | Error) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return response instanceof Error ? Promise.reject(response) : Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
  };
  return { requests, client: new CoreClient(transport) };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

const connected: SyncInfo = {
  configured: true,
  serverUrl: "https://sync.example.com",
  state: "Idle",
  pendingEntries: 2,
  parkedItems: 0,
  transfers: { uploads: 0, downloads: 0, waiting: 0 },
  remoteCursor: 41,
  serverEpoch: "00112233445566778899aabbccddeeff",
  lastPushAt: 1_700_000_000_000,
  lastPullAt: 1_700_000_000_500,
  forkDetected: false,
  devicesSeen: 1,
  needsInviteToken: false,
};

const disabled: SyncInfo = {
  configured: false,
  state: "Disabled",
  pendingEntries: 0,
  parkedItems: 0,
  transfers: { uploads: 0, downloads: 0, waiting: 0 },
  remoteCursor: 0,
  forkDetected: false,
  devicesSeen: 0,
  needsInviteToken: false,
};

const BAD_URL = {
  code: "InvalidInput",
  message: "the server address must be http(s)://host[:port]",
  details: { field: "serverUrl" },
};

describe("CoreClient sync requests", () => {
  it("syncConfigure sends the trimmed address and returns the status", async () => {
    const { client, requests } = stubTransport({ type: "Sync", data: connected });
    await expect(client.syncConfigure("  https://sync.example.com ")).resolves.toEqual(connected);
    expect(requests).toEqual([{ type: "SyncConfigure", data: { serverUrl: "https://sync.example.com" } }]);
  });

  it("syncConfigure sends the trimmed invite token with the address", async () => {
    const { client, requests } = stubTransport({ type: "Sync", data: connected });
    await client.syncConfigure("http://localhost:8787", " invite-1 ");
    expect(requests).toEqual([
      { type: "SyncConfigure", data: { serverUrl: "http://localhost:8787", inviteToken: "invite-1" } },
    ]);
  });

  it("syncConfigure(null) sends no address, which disables sync", async () => {
    const { client, requests } = stubTransport({ type: "Sync", data: disabled });
    await expect(client.syncConfigure(null)).resolves.toEqual(disabled);
    expect(requests).toEqual([{ type: "SyncConfigure", data: {} }]);
  });

  it("syncConfigure refuses a bad address before sending anything", async () => {
    const { client, requests } = stubTransport({ type: "Sync", data: connected });
    for (const bad of ["sync.example.com", "ftp://sync.example.com", "https://me:pw@sync.example.com", "https://x.com?a=1"]) {
      expect((await failure(client.syncConfigure(bad))).toApiError(), bad).toEqual(BAD_URL);
    }
    expect(requests).toEqual([]);
  });

  it("syncConfigure refuses a bad invite token before sending anything, without quoting it", async () => {
    const { client, requests } = stubTransport({ type: "Sync", data: connected });
    for (const bad of ["", "x".repeat(129), "töken"]) {
      const error = await failure(client.syncConfigure("https://sync.example.com", bad));
      expect(error.toApiError()).toEqual({
        code: "InvalidInput",
        message: "the invite token must be 1 to 128 printable ASCII characters",
        details: { field: "inviteToken" },
      });
    }
    // The address is checked first.
    expect((await failure(client.syncConfigure("nope", ""))).toApiError()).toEqual(BAD_URL);
    expect(requests).toEqual([]);
  });

  it("syncStatus sends SyncStatus and returns the status", async () => {
    const { client, requests } = stubTransport({ type: "Sync", data: connected });
    await expect(client.syncStatus()).resolves.toEqual(connected);
    expect(requests).toEqual([{ type: "SyncStatus" }]);
  });

  it("syncNow sends SyncNow and resolves with nothing", async () => {
    const { client, requests } = stubTransport({ type: "Ok" });
    await expect(client.syncNow()).resolves.toBeUndefined();
    expect(requests).toEqual([{ type: "SyncNow" }]);
  });

  it("a response of the wrong variant is an Internal error", async () => {
    const { client } = stubTransport({ type: "Ok" });
    expect((await failure(client.syncStatus())).toApiError()).toEqual({
      code: "Internal",
      message: "Unexpected response to SyncStatus: expected Sync, got Ok",
    });
    const sync = stubTransport({ type: "Sync", data: connected });
    expect((await failure(sync.client.syncNow())).message).toBe("Unexpected response to SyncNow: expected Ok, got Sync");
    expect((await failure(sync.client.syncNow())).code).toBe("Internal");
  });

  it("failures of the transport reach the caller unchanged", async () => {
    const locked = new CoreError({ code: "Locked", message: "the workspace is locked" });
    const { client } = stubTransport(locked);
    await expect(client.syncStatus()).rejects.toBe(locked);
    await expect(client.syncNow()).rejects.toBe(locked);
    await expect(client.syncConfigure("https://sync.example.com")).rejects.toBe(locked);
  });
});
