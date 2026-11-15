import { describe, expect, it } from "vitest";

import { AndroidTransport, base64ToBytes, bytesToBase64 } from "./android";
import type { KbHost } from "./android";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";

class FakeHost implements KbHost {
  sent: Record<string, unknown>[] = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  postMessage(message: string): void {
    this.sent.push(JSON.parse(message) as Record<string, unknown>);
  }
  say(message: unknown): void {
    this.onmessage?.({ data: typeof message === "string" ? message : JSON.stringify(message) });
  }
}

const setup = () => {
  const host = new FakeHost();
  return { host, transport: new AndroidTransport(host) };
};

describe("AndroidTransport", () => {
  it("dispatches a request and resolves with ok", async () => {
    const { host, transport } = setup();
    const pending = transport.dispatch({ type: "AppStatus" });
    expect(host.sent[0]).toMatchObject({ op: "dispatch", request: { type: "AppStatus" } });
    host.say({ id: host.sent[0]?.id, ok: { type: "Ok" } });
    await expect(pending).resolves.toEqual({ type: "Ok" });
  });

  it("rejects an error reply with a CoreError carrying code, message and details", async () => {
    const { host, transport } = setup();
    const pending = transport.dispatch({ type: "AppStatus" });
    host.say({ id: host.sent[0]?.id, err: { code: "NotFound", message: "gone", details: { page: 1 } } });
    const error = await pending.catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "NotFound", message: "gone", details: { page: 1 } });
  });

  it("matches replies to their ids when they arrive out of order", async () => {
    const { host, transport } = setup();
    const first = transport.dispatch({ type: "AppStatus" });
    const second = transport.dispatch({ type: "SpaceList" });
    host.say({ id: host.sent[1]?.id, ok: "second" });
    host.say({ id: host.sent[0]?.id, ok: "first" });
    await expect(first).resolves.toBe("first");
    await expect(second).resolves.toBe("second");
  });

  it("round-trips document bytes as base64", async () => {
    const { host, transport } = setup();
    const bytes = Uint8Array.from({ length: 70000 }, (_, index) => index % 256);
    const opened = transport.docs.open("p1");
    expect(host.sent[0]).toEqual({ id: host.sent[0]?.id, op: "docOpen", pageId: "p1" });
    host.say({ id: host.sent[0]?.id, ok: { b64: bytesToBase64(bytes) } });
    await expect(opened).resolves.toEqual(bytes);

    const imported = transport.docs.import("p1", bytes);
    expect(host.sent[1]).toMatchObject({ op: "docImport", pageId: "p1" });
    expect(base64ToBytes(host.sent[1]?.b64 as string)).toEqual(bytes);
    host.say({ id: host.sent[1]?.id, ok: null });
    await expect(imported).resolves.toBeUndefined();

    const exported = transport.docs.export("p1", Uint8Array.of(1, 2));
    expect(host.sent[2]).toMatchObject({ op: "docExport", b64: "AQI=" });
    host.say({ id: host.sent[2]?.id, ok: { b64: "" } });
    await expect(exported).resolves.toEqual(new Uint8Array());

    const closed = transport.docs.close("p1");
    expect(host.sent[3]).toMatchObject({ op: "docClose", pageId: "p1" });
    host.say({ id: host.sent[3]?.id, ok: null });
    await closed;
  });

  it("reads and writes attachments, with an optional range", async () => {
    const { host, transport } = setup();
    const whole = transport.attachments.read("a1");
    expect(host.sent[0]).toEqual({ id: host.sent[0]?.id, op: "attRead", attachmentId: "a1" });
    host.say({ id: host.sent[0]?.id, ok: { b64: "AQID" } });
    await expect(whole).resolves.toEqual(Uint8Array.of(1, 2, 3));

    const part = transport.attachments.read("a1", { start: 2, end: 9 });
    expect(host.sent[1]).toMatchObject({ op: "attRead", start: 2, end: 9 });
    host.say({ id: host.sent[1]?.id, ok: { b64: "AQ==" } });
    await part;

    const written = transport.attachments.write("a1", Uint8Array.of(7));
    expect(host.sent[2]).toMatchObject({ op: "attWrite", attachmentId: "a1", b64: "Bw==" });
    host.say({ id: host.sent[2]?.id, ok: null });
    await written;
    expect("mediaUrl" in transport.attachments).toBe(false);
  });

  it("delivers events to subscribers in order until they unsubscribe", () => {
    const { host, transport } = setup();
    const seen: string[] = [];
    const off = transport.subscribe((event) => seen.push(event.type));
    const a = { type: "WorkspaceCreated", data: { id: "x" } } as unknown as Event;
    const b = { type: "SettingChanged", data: { key: "k" } } as unknown as Event;
    host.say({ event: a });
    host.say({ event: b });
    off();
    host.say({ event: a });
    expect(seen).toEqual(["WorkspaceCreated", "SettingChanged"]);
  });

  it("surfaces host messages and sends notifications", () => {
    const { host, transport } = setup();
    const got: unknown[] = [];
    transport.onHostMessage((message) => got.push(message));
    host.say({ init: { pageId: "p" } });
    host.say({ theme: "dark" });
    host.say("not json");
    expect(got).toEqual([{ init: { pageId: "p" } }, { theme: "dark" }]);
    transport.notify({ host: "ready" });
    void transport.openExternal("https://example.com");
    expect(host.sent).toEqual([{ host: "ready" }, { host: "openExternal", url: "https://example.com" }]);
  });

  it("rejects a call when the host cannot be posted to", async () => {
    const host = new FakeHost();
    host.postMessage = () => {
      throw new Error("gone");
    };
    await expect(new AndroidTransport(host).dispatch({ type: "AppStatus" })).rejects.toBeInstanceOf(CoreError);
  });
});
