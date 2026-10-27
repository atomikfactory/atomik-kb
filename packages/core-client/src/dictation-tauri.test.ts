// `TauriDictation` (ADR-0053 contract): the commands `dictation_*` with the argument names the shell reads
// (`apps/desktop/src-tauri/src/lib.rs`; `apps/desktop/src/ipc-contract.test.ts` keeps the two in step), the
// channels of a session and of an installation with their messages validated, and the audio pushed in order.

import { beforeEach, describe, expect, it, vi } from "vitest";

import { dictationReasonOf, type DictationEvent, type InstallEvent } from "./dictation";
import { TauriDictation } from "./dictation-tauri";
import { CoreError } from "./errors";

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
  return {
    invoke: vi.fn<(command: string, args?: unknown, options?: unknown) => Promise<unknown>>(),
    MockChannel,
  };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke, Channel: tauri.MockChannel }));

const STATUS = {
  selected: "light",
  engines: [
    { engine: "accurate", installed: false, downloadBytes: 1, languages: ["en"], attribution: "a", available: true },
    { engine: "light", installed: true, downloadBytes: 2, languages: ["en"], attribution: "b", available: true },
    { engine: "ollama", installed: true, downloadBytes: 0, languages: [], attribution: "", available: false, unavailableReason: "aiOff" },
  ],
};

beforeEach(() => {
  tauri.invoke.mockReset();
  tauri.invoke.mockResolvedValue(undefined);
  tauri.MockChannel.instances.length = 0;
});

describe("TauriDictation status, select, import and remove", () => {
  it("invokes the commands with the engine and validates the status", async () => {
    const dictation = new TauriDictation();
    tauri.invoke.mockResolvedValue(STATUS);
    expect((await dictation.status()).selected).toBe("light");
    expect(tauri.invoke).toHaveBeenLastCalledWith("dictation_status");
    expect((await dictation.select("accurate")).engines).toHaveLength(3);
    expect(tauri.invoke).toHaveBeenLastCalledWith("dictation_select", { engine: "accurate" });
    await dictation.importModel("light");
    expect(tauri.invoke).toHaveBeenLastCalledWith("dictation_import", { engine: "light" });
    await dictation.remove("light");
    expect(tauri.invoke).toHaveBeenLastCalledWith("dictation_remove", { engine: "light" });
  });

  it("rejects with a CoreError when the shell does", async () => {
    tauri.invoke.mockRejectedValue({ code: "Locked", message: "the workspace is locked" });
    await expect(new TauriDictation().status()).rejects.toBeInstanceOf(CoreError);
  });

  it("refuses an answer that is not a status", async () => {
    tauri.invoke.mockResolvedValue({ nope: true });
    await expect(new TauriDictation().status()).rejects.toThrow(/dictation_status/);
  });
});

describe("TauriDictation.install", () => {
  it("hands the shell a channel that passes the installation's events on, and resolves when the command does", async () => {
    const events: InstallEvent[] = [];
    tauri.invoke.mockImplementation((command) => {
      if (command === "dictation_install") {
        const channel = tauri.MockChannel.instances[0];
        channel?.onmessage({ type: "progress", done: 1, total: 4 });
        channel?.onmessage({ type: "who knows" });
        channel?.onmessage({ type: "done" });
      }
      return Promise.resolve(undefined);
    });
    await new TauriDictation().install("accurate", (event) => events.push(event));
    expect(tauri.invoke).toHaveBeenCalledWith("dictation_install", { engine: "accurate", onEvent: tauri.MockChannel.instances[0] });
    expect(events).toEqual([{ type: "progress", done: 1, total: 4 }, { type: "done" }]);
  });

  it("rejects with the reason when nothing started, and cancels with its own command", async () => {
    tauri.invoke.mockRejectedValueOnce({
      code: "InvalidInput",
      message: "declined",
      details: { field: "engine", reason: "consentDeclined" },
    });
    const dictation = new TauriDictation();
    await expect(dictation.install("light", () => undefined)).rejects.toSatisfy(
      (error) => dictationReasonOf(error) === "consentDeclined",
    );
    await dictation.cancelInstall();
    expect(tauri.invoke).toHaveBeenLastCalledWith("dictation_install_cancel");
  });
});

describe("TauriDictation.start", () => {
  it("starts a session under a fresh id and passes the events of the channel on", async () => {
    const events: DictationEvent[] = [];
    const session = await new TauriDictation().start("light", (event) => events.push(event));
    const call = tauri.invoke.mock.calls[0];
    expect(call?.[0]).toBe("dictation_start");
    expect(call?.[1]).toEqual({ sessionId: session.id, engine: "light", onEvent: tauri.MockChannel.instances[0] });
    expect(session.id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    const channel = tauri.MockChannel.instances[0];
    channel?.onmessage({ type: "state", state: "listening" });
    channel?.onmessage({ type: "phrase", text: "Hello." });
    channel?.onmessage({ type: "phrase", text: "" });
    channel?.onmessage({ type: "ended" });
    expect(events).toEqual([
      { type: "state", state: "listening" },
      { type: "phrase", text: "Hello." },
      { type: "ended" },
    ]);
  });

  it("rejects with the reason when the shell refuses to start", async () => {
    tauri.invoke.mockRejectedValue({ code: "InvalidInput", message: "busy", details: { reason: "busy" } });
    await expect(new TauriDictation().start("light", () => undefined)).rejects.toSatisfy(
      (error) => dictationReasonOf(error) === "busy",
    );
  });

  it("sends the audio one call at a time, in order, as little-endian bytes with the session id in the header", async () => {
    const session = await new TauriDictation().start("light", () => undefined);
    const releases: (() => void)[] = [];
    let running = 0;
    let busiest = 0;
    tauri.invoke.mockImplementation((command) => {
      if (command !== "dictation_audio") {
        return Promise.resolve(undefined);
      }
      running += 1;
      busiest = Math.max(busiest, running);
      return new Promise<undefined>((resolve) => {
        releases.push(() => {
          running -= 1;
          resolve(undefined);
        });
      });
    });
    const first = Int16Array.of(1, -2);
    session.push(first);
    session.push(Int16Array.of(3));
    first[0] = 99; // the push copied its samples
    await vi.waitFor(() => expect(releases).toHaveLength(1));
    releases[0]?.();
    await vi.waitFor(() => expect(releases).toHaveLength(2));
    releases[1]?.();
    await vi.waitFor(() => expect(running).toBe(0));
    expect(busiest).toBe(1);
    const audio = tauri.invoke.mock.calls.filter((call) => call[0] === "dictation_audio");
    expect(audio).toHaveLength(2);
    expect([...(audio[0]?.[1] as Uint8Array)]).toEqual([1, 0, 0xfe, 0xff]);
    expect([...(audio[1]?.[1] as Uint8Array)]).toEqual([3, 0]);
    expect(audio[0]?.[2]).toEqual({ headers: { "x-kb-dictation": session.id } });
  });

  it("stops after the pushed audio went out, cancels at once, and drops pushes after the end", async () => {
    const session = await new TauriDictation().start("light", () => undefined);
    session.push(Int16Array.of(1));
    session.push(new Int16Array(0));
    await session.stop();
    const commands = tauri.invoke.mock.calls.map((call) => call[0]);
    expect(commands).toEqual(["dictation_start", "dictation_audio", "dictation_stop"]);
    expect(tauri.invoke).toHaveBeenLastCalledWith("dictation_stop", { sessionId: session.id });
    session.push(Int16Array.of(2));
    await Promise.resolve();
    expect(tauri.invoke).toHaveBeenCalledTimes(3);

    const other = await new TauriDictation().start("light", () => undefined);
    await other.cancel();
    expect(tauri.invoke).toHaveBeenLastCalledWith("dictation_cancel", { sessionId: other.id });
  });

  it("does not fail a push when the session has ended in the shell", async () => {
    const session = await new TauriDictation().start("light", () => undefined);
    tauri.invoke.mockRejectedValue({ code: "InvalidInput", message: "ended", details: { field: "sessionId" } });
    session.push(Int16Array.of(1));
    await expect(session.stop()).rejects.toBeInstanceOf(CoreError);
  });
});
