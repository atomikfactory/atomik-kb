// The in-memory dictation (ADR-0053): it keeps the shell's contract, so the UI can be built and tested against it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { dictationReasonOf, type DictationEvent, type InstallEvent } from "./dictation";
import { FAKE_PHRASES, MemoryDictation } from "./dictation-memory";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

function collect<T>(): { events: T[]; push: (event: T) => void } {
  const events: T[] = [];
  return { events, push: (event) => events.push(event) };
}

describe("MemoryDictation.status", () => {
  it("lists every engine with the catalog's sizes and languages", async () => {
    const status = await new MemoryDictation().status();
    expect(status.selected).toBeNull();
    expect(status.engines.map((engine) => engine.engine)).toEqual(["accurate", "light", "ollama"]);
    expect(status.engines.map((engine) => engine.installed)).toEqual([false, false, true]);
    expect(status.engines[0]?.downloadBytes).toBe(670_478_772);
    expect(status.engines[0]?.languages).toHaveLength(25);
    expect(status.engines[1]?.languages).toEqual(["en"]);
    expect(status.engines[2]?.downloadBytes).toBe(0);
    expect(status.engines.every((engine) => engine.available)).toBe(true);
  });

  it("keeps the choice and reports why Ollama cannot be used", async () => {
    const dictation = new MemoryDictation({ ollamaUnavailable: "aiOff", installed: ["light"] });
    const status = await dictation.select("light");
    expect(status.selected).toBe("light");
    expect(status.engines[1]?.installed).toBe(true);
    expect(status.engines[2]).toMatchObject({ available: false, unavailableReason: "aiOff" });
  });
});

describe("MemoryDictation.install", () => {
  it("reports progress, verifying and done, and then the engine is installed", async () => {
    const dictation = new MemoryDictation({ installStepMs: 10 });
    const { events, push } = collect<InstallEvent>();
    const finished = dictation.install("light", push);
    await vi.runAllTimersAsync();
    await finished;
    expect(events[0]).toEqual({ type: "progress", done: 12_396_753, total: 123_967_539 });
    const progress = events.filter((event) => event.type === "progress");
    expect(progress).toHaveLength(10);
    expect(progress.at(-1)).toEqual({ type: "progress", done: 123_967_539, total: 123_967_539 });
    expect(events.slice(-2)).toEqual([{ type: "verifying" }, { type: "done" }]);
    expect((await dictation.status()).engines[1]?.installed).toBe(true);
  });

  it("can be cancelled", async () => {
    const dictation = new MemoryDictation({ installStepMs: 10 });
    const { events, push } = collect<InstallEvent>();
    const finished = dictation.install("accurate", push);
    await vi.advanceTimersByTimeAsync(25);
    await dictation.cancelInstall();
    await finished;
    expect(events.at(-1)).toEqual({ type: "error", code: "cancelled" });
    expect((await dictation.status()).engines[0]?.installed).toBe(false);
  });

  it("refuses a second installation, a declined dialog and Ollama", async () => {
    const dictation = new MemoryDictation({ installStepMs: 10 });
    const running = dictation.install("light", () => undefined);
    await expect(dictation.install("accurate", () => undefined)).rejects.toSatisfy(
      (error) => dictationReasonOf(error) === "busy",
    );
    await vi.runAllTimersAsync();
    await running;
    await expect(new MemoryDictation({ consent: false }).install("light", () => undefined)).rejects.toSatisfy(
      (error) => dictationReasonOf(error) === "consentDeclined",
    );
    await expect(dictation.install("ollama", () => undefined)).rejects.toBeInstanceOf(Error);
  });

  it("can be made to fail", async () => {
    const dictation = new MemoryDictation({ installStepMs: 10, failInstallWith: "downloadFailed" });
    const { events, push } = collect<InstallEvent>();
    const finished = dictation.install("light", push);
    await vi.runAllTimersAsync();
    await finished;
    expect(events.at(-1)).toEqual({ type: "error", code: "downloadFailed" });
    expect(events.filter((event) => event.type === "progress")).toHaveLength(1);
    expect(dictation.installedEngines).toEqual([]);
  });
});

describe("MemoryDictation import and remove", () => {
  it("imports and removes the model files", async () => {
    const dictation = new MemoryDictation();
    expect((await dictation.importModel("accurate")).engines[0]?.installed).toBe(true);
    expect((await dictation.remove("accurate")).engines[0]?.installed).toBe(false);
    await expect(dictation.importModel("ollama")).rejects.toSatisfy((error) => dictationReasonOf(error) === "importInvalid");
  });
});

describe("MemoryDictation.start", () => {
  it("refuses an engine whose model is missing, Ollama when unavailable, and a second session", async () => {
    await expect(new MemoryDictation().start("light", () => undefined)).rejects.toSatisfy(
      (error) => dictationReasonOf(error) === "modelMissing",
    );
    await expect(new MemoryDictation({ ollamaUnavailable: "noTranscriptionModel" }).start("ollama", () => undefined)).rejects.toSatisfy(
      (error) => dictationReasonOf(error) === "ollamaUnavailable",
    );
    await expect(new MemoryDictation({ failStartWith: "engineMissing" }).start("ollama", () => undefined)).rejects.toSatisfy(
      (error) => dictationReasonOf(error) === "engineMissing",
    );
    const dictation = new MemoryDictation({ installed: ["light"] });
    await dictation.start("light", () => undefined);
    await expect(dictation.start("light", () => undefined)).rejects.toSatisfy((error) => dictationReasonOf(error) === "busy");
  });

  it("loads, listens, speaks one phrase for each two seconds of audio and ends", async () => {
    const dictation = new MemoryDictation({ installed: ["light"], loadMs: 20 });
    const { events, push } = collect<DictationEvent>();
    const session = await dictation.start("light", push);
    expect(events).toEqual([{ type: "state", state: "loading" }]);
    await vi.advanceTimersByTimeAsync(20);
    expect(events.at(-1)).toEqual({ type: "state", state: "listening" });
    for (let chunk = 0; chunk < 19; chunk += 1) {
      session.push(new Int16Array(1_600));
    }
    expect(events.filter((event) => event.type === "phrase")).toHaveLength(0);
    session.push(new Int16Array(1_600));
    expect(events.slice(-3)).toEqual([
      { type: "state", state: "recognising" },
      { type: "phrase", text: FAKE_PHRASES[0] },
      { type: "state", state: "listening" },
    ]);
    for (let chunk = 0; chunk < 20; chunk += 1) {
      session.push(new Int16Array(1_600));
    }
    expect(events.filter((event) => event.type === "phrase")).toEqual([
      { type: "phrase", text: FAKE_PHRASES[0] },
      { type: "phrase", text: FAKE_PHRASES[1] },
    ]);
    await session.stop();
    expect(events.at(-1)).toEqual({ type: "ended" });
    expect(dictation.runningSession).toBeUndefined();
  });

  it("speaks the audio of the last moments when it stops, but not when nothing came in", async () => {
    const dictation = new MemoryDictation({ installed: ["light"], loadMs: 5, phrases: ["one", "two"] });
    const first = collect<DictationEvent>();
    const session = await dictation.start("light", first.push);
    await vi.advanceTimersByTimeAsync(5);
    session.push(new Int16Array(100));
    await session.stop();
    expect(first.events.filter((event) => event.type === "phrase")).toEqual([{ type: "phrase", text: "one" }]);
    expect(first.events.at(-1)).toEqual({ type: "ended" });

    const second = collect<DictationEvent>();
    const quiet = await dictation.start("light", second.push);
    await vi.advanceTimersByTimeAsync(5);
    await quiet.stop();
    expect(second.events.filter((event) => event.type === "phrase")).toEqual([]);
    expect(second.events.at(-1)).toEqual({ type: "ended" });
  });

  it("keeps the audio pushed while the model loads and recognises it when ready", async () => {
    const dictation = new MemoryDictation({ installed: ["accurate"], loadMs: 50 });
    const { events, push } = collect<DictationEvent>();
    const session = await dictation.start("accurate", push);
    session.push(new Int16Array(32_000));
    expect(events.filter((event) => event.type === "phrase")).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(50);
    expect(events.filter((event) => event.type === "phrase")).toHaveLength(1);
  });

  it("ends at once on cancel, with no more phrases, and then allows a new session", async () => {
    const dictation = new MemoryDictation({ installed: ["light"], loadMs: 5 });
    const { events, push } = collect<DictationEvent>();
    const session = await dictation.start("light", push);
    await vi.advanceTimersByTimeAsync(5);
    session.push(new Int16Array(10_000));
    await session.cancel();
    expect(events.at(-1)).toEqual({ type: "ended" });
    session.push(new Int16Array(100_000));
    await vi.runAllTimersAsync();
    expect(events.filter((event) => event.type === "phrase")).toHaveLength(0);
    expect(events.filter((event) => event.type === "ended")).toHaveLength(1);
    await expect(dictation.start("light", () => undefined)).resolves.toBeDefined();
  });

  it("cannot remove a model while a session uses it", async () => {
    const dictation = new MemoryDictation({ installed: ["light"] });
    const session = await dictation.start("light", () => undefined);
    await expect(dictation.remove("light")).rejects.toSatisfy((error) => dictationReasonOf(error) === "busy");
    await session.cancel();
    await expect(dictation.remove("light")).resolves.toBeDefined();
  });

  it("can dictate with Ollama when it is available", async () => {
    const dictation = new MemoryDictation();
    const { events, push } = collect<DictationEvent>();
    const session = await dictation.start("ollama", push);
    await vi.advanceTimersByTimeAsync(30);
    session.push(new Int16Array(32_000));
    expect(events.some((event) => event.type === "phrase")).toBe(true);
    await session.stop();
  });
});
