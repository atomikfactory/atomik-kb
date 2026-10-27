// `TauriTransport.presence` (Milestone 13, slice 3 contract): the command `presence_send` with the update as an array of
// byte values, and the channel of `presence_events` with its messages validated and turned into bytes. The shell's side
// of both is `apps/desktop/src-tauri/src/presence.rs`; `apps/desktop/src/ipc-contract.test.ts` keeps the two in step.

import { beforeEach, describe, expect, it, vi } from "vitest";

import { CoreError } from "./errors";
import { TauriTransport } from "./tauri";
import type { PresenceUpdate } from "./transport";

// Stand-ins for `@tauri-apps/api/core`, as in `tauri.test.ts`.
const tauri = vi.hoisted(() => {
  class MockChannel<T> {
    static instances: MockChannel<unknown>[] = [];
    readonly onmessage: (message: T) => void;
    constructor(onmessage: (message: T) => void) {
      this.onmessage = onmessage;
      MockChannel.instances.push(this as MockChannel<unknown>);
    }
  }
  return { invoke: vi.fn<(command: string, args?: unknown) => Promise<unknown>>(), MockChannel };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke, Channel: tauri.MockChannel }));

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const ACCOUNT = "33333333-3333-4333-8333-333333333333";
const DEVICE = "44444444-4444-4444-8444-444444444444";

const info = {
  accountId: ACCOUNT,
  deviceId: DEVICE,
  name: "Bob",
  color: 3,
  pageId: PAGE,
  state: "Editing",
  updatedAt: 1_791_383_400_000,
};

beforeEach(() => {
  tauri.invoke.mockReset();
  tauri.invoke.mockResolvedValue(undefined);
  tauri.MockChannel.instances.length = 0;
  vi.restoreAllMocks();
});

describe("TauriTransport.presence.send", () => {
  it("invokes presence_send with the ids, the state and the update as an array of numbers", async () => {
    const transport = new TauriTransport();
    await transport.presence.send(SPACE, PAGE, "Viewing", Uint8Array.of(0, 7, 255));
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("presence_send", {
      spaceId: SPACE,
      pageId: PAGE,
      state: "Viewing",
      update: [0, 7, 255],
    });
  });

  it("leaves the page out for a device in no page and rejects with a CoreError", async () => {
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "the update is too large" });
    const error: unknown = await new TauriTransport()
      .presence.send(SPACE, undefined, "Idle", new Uint8Array())
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("presence_send", {
      spaceId: SPACE,
      pageId: undefined,
      state: "Idle",
      update: [],
    });
  });
});

describe("TauriTransport.presence.subscribe", () => {
  it("registers one channel with presence_events and reuses it for every listener", () => {
    const transport = new TauriTransport();
    transport.presence.subscribe(() => undefined);
    transport.presence.subscribe(() => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(1);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("presence_events", { onEvent: tauri.MockChannel.instances[0] });
  });

  it("does not touch the shell until someone listens", () => {
    new TauriTransport();
    expect(tauri.invoke).not.toHaveBeenCalled();
  });

  it("delivers an accepted frame as bytes to every listener until it unsubscribes", () => {
    const transport = new TauriTransport();
    const first: PresenceUpdate[] = [];
    const second: PresenceUpdate[] = [];
    const stopFirst = transport.presence.subscribe((message) => first.push(message));
    transport.presence.subscribe((message) => second.push(message));
    const channel = tauri.MockChannel.instances[0];

    channel?.onmessage({ info, update: [1, 2, 3] });
    expect(first).toEqual([{ info, update: Uint8Array.of(1, 2, 3) }]);
    expect(first[0]?.update).toBeInstanceOf(Uint8Array);
    expect(second).toEqual(first);

    stopFirst();
    channel?.onmessage({ info, update: [] });
    expect(first).toHaveLength(1);
    expect(second).toHaveLength(2);
  });

  it("reads a device in no page (the page id is left out)", () => {
    const transport = new TauriTransport();
    const received: PresenceUpdate[] = [];
    transport.presence.subscribe((message) => received.push(message));
    const idle = { accountId: ACCOUNT, deviceId: DEVICE, name: "Bob", color: 3, state: "Idle", updatedAt: 1_791_383_400_000 };
    tauri.MockChannel.instances[0]?.onmessage({ info: idle, update: [] });
    expect(received).toEqual([{ info: idle, update: new Uint8Array() }]);
    expect(received[0]?.info).not.toHaveProperty("pageId");
  });

  it("drops what is not a presence message", () => {
    const transport = new TauriTransport();
    const received: PresenceUpdate[] = [];
    transport.presence.subscribe((message) => received.push(message));
    const channel = tauri.MockChannel.instances[0];
    for (const bad of [
      null,
      "text",
      {},
      { info },
      { update: [] },
      { info, update: "AQID" },
      { info, update: [256] },
      { info, update: [-1] },
      { info, update: [1.5] },
      { info, update: ["1"] },
      { info: { ...info, state: "Away" }, update: [] },
      { info: { ...info, state: "viewing" }, update: [] },
      { info: { ...info, color: 256 }, update: [] },
      { info: { ...info, color: -1 }, update: [] },
      { info: { ...info, name: 7 }, update: [] },
      { info: { ...info, accountId: undefined }, update: [] },
      { info: { ...info, deviceId: 1 }, update: [] },
      { info: { ...info, pageId: 5 }, update: [] },
      { info: { ...info, updatedAt: "now" }, update: [] },
    ]) {
      channel?.onmessage(bad);
    }
    expect(received).toEqual([]);
  });

  it("one misbehaving listener does not starve the others", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const transport = new TauriTransport();
    const received: PresenceUpdate[] = [];
    transport.presence.subscribe(() => {
      throw new Error("boom");
    });
    transport.presence.subscribe((message) => received.push(message));
    tauri.MockChannel.instances[0]?.onmessage({ info, update: [9] });
    expect(received).toHaveLength(1);
  });

  it("forgets the channel when the shell refuses it, so the next listener tries again", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    tauri.invoke.mockRejectedValueOnce({ code: "Internal", message: "presence_events is not implemented yet" });
    const transport = new TauriTransport();
    transport.presence.subscribe(() => undefined);
    await vi.waitFor(() => {
      expect(console.error).toHaveBeenCalled();
    });
    transport.presence.subscribe(() => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(2);
  });
});
