// `CoreClient`'s device requests (Milestone 7): pairing, revocation, rotation and recovery —
// what each sends, what it returns, and the checks made before anything reaches the transport.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { PairingInfo } from "./generated/PairingInfo";
import type { RecoveryInfo } from "./generated/RecoveryInfo";
import type { RecoveryKit } from "./generated/RecoveryKit";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SecurityInfo } from "./generated/SecurityInfo";
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

const DEVICE = "22222222-2222-4222-8222-222222222222";
const URL_A = "https://sync.example.com";

const waiting: PairingInfo = {
  role: "Joiner",
  state: "Waiting",
  code: "ABCD EFGH JKMN",
  serverUrl: URL_A,
  expiresAt: 1_700_000_600_000,
};

const recovering: RecoveryInfo = { state: "Fetching", serverUrl: URL_A };

const kit: RecoveryKit = {
  words: Array.from({ length: 24 }, () => "abandon"),
  code: "AAAA-AAAA",
  accountId: "44444444-4444-4444-8444-444444444444",
  createdAt: 1_700_000_000_000,
};

const security: SecurityInfo = {
  appLock: { enabled: true, memoryKib: 65_536, iterations: 3, parallelism: 4 },
  thisDevice: "11111111-1111-4111-8111-111111111111",
  devices: 2,
  keyring: { spaces: 1, documents: 2, attachments: 0, tombstones: 0 },
  rootGeneration: 2,
  rotationPending: false,
  thisDeviceRevoked: false,
  recoveryPublished: true,
};

const BAD_URL = {
  code: "InvalidInput",
  message: "the server address must be http(s)://host[:port]",
  details: { field: "serverUrl" },
};
const BAD_CODE = { code: "InvalidInput", message: "the code is 12 letters and digits", details: { field: "code" } };

describe("CoreClient pairing requests", () => {
  it("pairingStart as the joiner sends the role and the trimmed server address", async () => {
    const { client, requests } = stubTransport({ type: "Pairing", data: waiting });
    await expect(client.pairingStart("Joiner", { serverUrl: ` ${URL_A} ` })).resolves.toEqual(waiting);
    expect(requests).toEqual([{ type: "PairingStart", data: { role: "Joiner", serverUrl: URL_A } }]);
  });

  it("pairingStart as the sponsor sends the code's 12 canonical characters", async () => {
    const { client, requests } = stubTransport({ type: "Pairing", data: waiting });
    await client.pairingStart("Sponsor", { code: "abcd-efgh-jkmo" });
    expect(requests).toEqual([{ type: "PairingStart", data: { role: "Sponsor", code: "ABCDEFGHJKM0" } }]);
  });

  it("pairingStart without options sends the role alone", async () => {
    const { client, requests } = stubTransport({ type: "Pairing", data: waiting });
    await client.pairingStart("Joiner");
    expect(requests).toEqual([{ type: "PairingStart", data: { role: "Joiner" } }]);
  });

  it("pairingStart refuses a bad address or code before sending anything", async () => {
    const { client, requests } = stubTransport({ type: "Pairing", data: waiting });
    expect((await failure(client.pairingStart("Joiner", { serverUrl: "sync.example.com" }))).toApiError()).toEqual(BAD_URL);
    expect((await failure(client.pairingStart("Sponsor", { code: "ABCD EFGH" }))).toApiError()).toEqual(BAD_CODE);
    expect((await failure(client.pairingStart("Sponsor", { code: "ABCD EFGH JKMU" }))).toApiError()).toEqual(BAD_CODE);
    expect(requests).toEqual([]);
  });

  it("pairingConfirm, pairingCancel and pairingStatus send their request and return the session", async () => {
    const { client, requests } = stubTransport({ type: "Pairing", data: waiting });
    await expect(client.pairingConfirm()).resolves.toEqual(waiting);
    await expect(client.pairingCancel()).resolves.toEqual(waiting);
    await expect(client.pairingStatus()).resolves.toEqual(waiting);
    expect(requests).toEqual([{ type: "PairingConfirm" }, { type: "PairingCancel" }, { type: "PairingStatus" }]);
  });

  it("a response of the wrong variant is an Internal error", async () => {
    const { client } = stubTransport({ type: "Ok" });
    expect((await failure(client.pairingStatus())).toApiError()).toEqual({
      code: "Internal",
      message: "Unexpected response to PairingStatus: expected Pairing, got Ok",
    });
  });
});

describe("CoreClient device requests", () => {
  it("deviceRevoke sends the device, with the passphrase only when given, and returns the new kit", async () => {
    const { client, requests } = stubTransport({ type: "RecoveryKit", data: kit });
    await expect(client.deviceRevoke(DEVICE)).resolves.toEqual(kit);
    await client.deviceRevoke(DEVICE, "correct horse");
    expect(requests).toEqual([
      { type: "DeviceRevoke", data: { deviceId: DEVICE } },
      { type: "DeviceRevoke", data: { deviceId: DEVICE, passphrase: "correct horse" } },
    ]);
  });

  it("rotationApply sends the passphrase only when given and returns the security status", async () => {
    const { client, requests } = stubTransport({ type: "Security", data: security });
    await expect(client.rotationApply()).resolves.toEqual(security);
    await client.rotationApply("correct horse");
    expect(requests).toEqual([
      { type: "RotationApply", data: {} },
      { type: "RotationApply", data: { passphrase: "correct horse" } },
    ]);
  });

  it("failures of the transport reach the caller unchanged", async () => {
    const locked = new CoreError({ code: "Locked", message: "the workspace is locked" });
    const { client } = stubTransport(locked);
    await expect(client.deviceRevoke(DEVICE)).rejects.toBe(locked);
    await expect(client.rotationApply()).rejects.toBe(locked);
    await expect(client.pairingConfirm()).rejects.toBe(locked);
  });
});

describe("CoreClient recovery requests", () => {
  it("recoveryStart sends the trimmed address and kit and returns the session", async () => {
    const { client, requests } = stubTransport({ type: "Recovery", data: recovering });
    await expect(client.recoveryStart(` ${URL_A}`, "  abandon ability able \n")).resolves.toEqual(recovering);
    expect(requests).toEqual([{ type: "RecoveryStart", data: { serverUrl: URL_A, kit: "abandon ability able" } }]);
  });

  it("recoveryStart refuses a bad address or an empty kit before sending anything", async () => {
    const { client, requests } = stubTransport({ type: "Recovery", data: recovering });
    expect((await failure(client.recoveryStart("ftp://x", "abandon"))).toApiError()).toEqual(BAD_URL);
    expect((await failure(client.recoveryStart(URL_A, "  \n "))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must not be empty",
      details: { field: "kit" },
    });
    expect(requests).toEqual([]);
  });

  it("recoveryStatus and recoveryCancel send their request and return the session", async () => {
    const { client, requests } = stubTransport({ type: "Recovery", data: { state: "Idle" } });
    await expect(client.recoveryStatus()).resolves.toEqual({ state: "Idle" });
    await expect(client.recoveryCancel()).resolves.toEqual({ state: "Idle" });
    expect(requests).toEqual([{ type: "RecoveryStatus" }, { type: "RecoveryCancel" }]);
  });

  it("a response of the wrong variant is an Internal error", async () => {
    const { client } = stubTransport({ type: "Pairing", data: waiting });
    expect((await failure(client.recoveryStatus())).message).toBe(
      "Unexpected response to RecoveryStatus: expected Recovery, got Pairing",
    );
  });
});
