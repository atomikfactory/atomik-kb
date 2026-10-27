// Dictation wired into the transports and the client (ADR-0053, `docs/architecture/dictation.md` section 5):
// `Transport.dictation`, `CoreClient.hasDictation()` / `dictation()`, the `MemoryTransport` option (`"fake"` or the
// fake's options) and the `TauriTransport` member. The fake's own behaviour is `dictation-memory.test.ts`.

import { beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { MemoryDictation } from "./dictation-memory";
import { CoreError } from "./errors";
import { MemoryTransport } from "./memory";
import { TauriTransport } from "./tauri";
import type { Transport } from "./transport";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve(undefined)), Channel: class {} }));

beforeEach(() => {
  vi.clearAllMocks();
});

describe("CoreClient over a transport without dictation", () => {
  it("says so and refuses to hand out an API", () => {
    const client = new CoreClient(new MemoryTransport());
    expect(client.hasDictation()).toBe(false);
    expect(() => client.dictation()).toThrow(CoreError);
    expect(() => client.dictation()).toThrow(/dictation is not supported by this transport/);
  });
});

describe("MemoryTransport", () => {
  it("has no dictation unless the option asks for it", () => {
    expect(new MemoryTransport().dictation).toBeUndefined();
    expect(new MemoryTransport({ ai: { mode: "fake" } }).dictation).toBeUndefined();
  });

  it('gives the fake with its defaults for "fake"', async () => {
    const memory = new MemoryTransport({ dictation: "fake" });
    expect(memory.dictation).toBeInstanceOf(MemoryDictation);
    const client = new CoreClient(memory);
    expect(client.hasDictation()).toBe(true);
    const status = await client.dictation().status();
    expect(status.selected).toBeNull();
    expect(status.engines.map((engine) => [engine.engine, engine.installed])).toEqual([
      ["accurate", false],
      ["light", false],
      ["ollama", true],
    ]);
  });

  it("gives the fake the options it was handed", async () => {
    const memory = new MemoryTransport({ dictation: { installed: ["light"], selected: "light", ollamaUnavailable: "aiOff" } });
    const status = await new CoreClient(memory).dictation().status();
    expect(status.selected).toBe("light");
    expect(status.engines.find((engine) => engine.engine === "light")?.installed).toBe(true);
    expect(status.engines.find((engine) => engine.engine === "ollama")).toMatchObject({ available: false, unavailableReason: "aiOff" });
  });

  it("dictates through the client: the fake's session, its phrases, `ended` last", async () => {
    const memory = new MemoryTransport({ dictation: { installed: ["light"], loadMs: 0, samplesPerPhrase: 100, phrases: ["Hello."] } });
    const dictation = new CoreClient(memory).dictation();
    const events: string[] = [];
    const session = await dictation.start("light", (event) => {
      events.push(event.type === "state" ? `state ${event.state}` : event.type === "phrase" ? `phrase ${event.text}` : event.type);
    });
    await new Promise((resolve) => setTimeout(resolve, 5));
    session.push(new Int16Array(100));
    await session.stop();
    expect(events).toEqual(["state loading", "state listening", "state recognising", "phrase Hello.", "state listening", "ended"]);
  });
});

describe("a hand-made transport", () => {
  it("hands out the dictation it has", () => {
    const api = new MemoryDictation();
    const transport: Transport = {
      dispatch: () => Promise.reject(new Error("not needed")),
      subscribe: () => () => undefined,
      coreState: () => Promise.resolve("ready"),
      docs: new MemoryTransport().docs,
      lock: () => Promise.resolve(),
      unlock: () => Promise.resolve(),
      backupExport: () => Promise.reject(new Error("not needed")),
      backupImport: () => Promise.reject(new Error("not needed")),
      dictation: api,
    };
    const client = new CoreClient(transport);
    expect(client.hasDictation()).toBe(true);
    expect(client.dictation()).toBe(api);
  });
});

describe("TauriTransport", () => {
  it("has the shell's dictation", () => {
    const transport = new TauriTransport();
    expect(transport.dictation).toBeDefined();
    const client = new CoreClient(transport);
    expect(client.hasDictation()).toBe(true);
    expect(client.dictation()).toBe(transport.dictation);
  });
});
