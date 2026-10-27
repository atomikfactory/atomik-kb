// `CoreClient`'s presence methods (Milestone 13, slice 3 contract): what `presenceList` sends and returns, how
// `presenceSend` and `onPresence` reach the transport's `presence` hook, and what a transport without it does. What the
// in-memory mirror's simulated people do (`?presence=...`) is `memory-presence.test.ts`, written with the mirror.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { PresenceInfo } from "./generated/PresenceInfo";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import { MemoryTransport } from "./memory";
import { PRESENCE_TIMEOUT_SECONDS, PRESENCE_UPDATES_PER_SECOND, PRESENCE_UPDATE_MAX_BYTES } from "./presence";
import type { DocTransport, PresenceTransport, PresenceUpdate, Transport } from "./transport";

/** Page documents are not used by these tests; touching them is a test bug. */
const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const ACCOUNT = "33333333-3333-4333-8333-333333333333";
const DEVICE = "44444444-4444-4444-8444-444444444444";

const bob: PresenceInfo = {
  accountId: ACCOUNT,
  deviceId: DEVICE,
  name: "Bob",
  color: 1,
  pageId: PAGE,
  state: "Editing",
  updatedAt: 1_791_383_400_000,
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response, presence?: PresenceTransport) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
    ...(presence === undefined ? {} : { presence }),
  };
  return { requests, client: new CoreClient(transport) };
}

describe("CoreClient presence", () => {
  it("lists who is in a space and sends the request as the contract says", async () => {
    const { client, requests } = stubTransport({ type: "Presence", data: [bob] });
    expect(await client.presenceList(SPACE)).toEqual([bob]);
    expect(requests).toEqual([{ type: "PresenceList", data: { spaceId: SPACE } }]);
  });

  it("refuses an answer of another type", async () => {
    const { client } = stubTransport({ type: "Ok" });
    await expect(client.presenceList(SPACE)).rejects.toBeInstanceOf(CoreError);
  });

  it("hands this device's presence to the transport and listens for the others'", async () => {
    const sent: unknown[][] = [];
    const listeners = new Set<(message: PresenceUpdate) => void>();
    const presence: PresenceTransport = {
      send: (...args) => {
        sent.push(args);
        return Promise.resolve();
      },
      subscribe: (listener) => {
        listeners.add(listener);
        return () => {
          listeners.delete(listener);
        };
      },
    };
    const { client } = stubTransport({ type: "Ok" }, presence);
    expect(client.hasPresence()).toBe(true);

    const update = Uint8Array.of(1, 2, 3);
    await client.presenceSend(SPACE, PAGE, "Viewing", update);
    await client.presenceSend(SPACE, undefined, "Idle", new Uint8Array());
    expect(sent).toEqual([
      [SPACE, PAGE, "Viewing", update],
      [SPACE, undefined, "Idle", new Uint8Array()],
    ]);

    const received: PresenceUpdate[] = [];
    const stop = client.onPresence((message) => received.push(message));
    expect(listeners.size).toBe(1);
    for (const listener of listeners) {
      listener({ info: bob, update });
    }
    expect(received).toEqual([{ info: bob, update }]);
    stop();
    expect(listeners.size).toBe(0);
  });

  it("a transport without presence says so, and listening to it does nothing", async () => {
    const { client } = stubTransport({ type: "Ok" });
    expect(client.hasPresence()).toBe(false);
    await expect(client.presenceSend(SPACE, PAGE, "Viewing", new Uint8Array())).rejects.toMatchObject({
      code: "Internal",
    });
    const stop = client.onPresence(() => {
      throw new Error("never called");
    });
    expect(stop()).toBeUndefined();
  });

  it("the in-memory mirror carries presence: an unknown space is NotFound, and a space without sharing sends nothing", async () => {
    const memory = new MemoryTransport({ spaces: [{ name: "Home", pages: [] }], workspaceName: "Home" });
    const client = new CoreClient(memory);
    expect(client.hasPresence()).toBe(true);
    await expect(client.presenceList(SPACE)).rejects.toMatchObject({ code: "NotFound" });
    const [home] = await client.spaceList();
    expect(await client.presenceList(home?.id ?? "")).toEqual([]);
    await client.presenceSend(home?.id ?? "", undefined, "Idle", new Uint8Array());
    expect(memory.sentPresence()).toEqual([]);
  });

  it("keeps the limits of the Rust constants", () => {
    // `kb_types::sharing`: PRESENCE_TIMEOUT_SECONDS, PRESENCE_UPDATES_PER_SECOND, PRESENCE_UPDATE_MAX_BYTES.
    expect(PRESENCE_TIMEOUT_SECONDS).toBe(30);
    expect(PRESENCE_UPDATES_PER_SECOND).toBe(10);
    expect(PRESENCE_UPDATE_MAX_BYTES).toBe(3072);
  });
});
