// The pure side of dictation (ADR-0053): the phrase-joining rule against the vectors it shares with
// `kb_speech::policy::join_phrase`, and the parsers of what the shell sends.

import { describe, expect, it } from "vitest";

import vectors from "../../../crates/kb-speech/testdata/join_phrase.json";
import {
  DICTATION_ERROR_CODES,
  dictationEventOf,
  dictationReasonOf,
  dictationStatusOf,
  installEventOf,
  joinPhrase,
  lastCharacterOf,
  newSessionId,
} from "./dictation";
import { CoreError } from "./errors";

describe("joinPhrase", () => {
  it("has the vectors of the Rust test", () => {
    expect(vectors.length).toBeGreaterThanOrEqual(100);
  });

  for (const vector of vectors) {
    it(vector.name, () => {
      expect(joinPhrase(vector.before, vector.text)).toBe(vector.expected);
    });
  }

  it("takes whitespace from Unicode White_Space, as Rust does (U+0085 is whitespace, U+FEFF is not)", () => {
    expect(joinPhrase("", "a\u0085b")).toBe("a b");
    expect(joinPhrase("", "a﻿b")).toBe("a﻿b");
    expect(joinPhrase("\u0085", "hello")).toBe("hello");
    expect(joinPhrase("﻿", "hello")).toBe(" hello");
  });
});

describe("lastCharacterOf", () => {
  it("is the last code point, or null for nothing", () => {
    expect(lastCharacterOf("abc")).toBe("c");
    expect(lastCharacterOf("a😀")).toBe("😀");
    expect(lastCharacterOf("")).toBeNull();
  });
});

describe("newSessionId", () => {
  it("is 1 to 64 of the characters the shell accepts, and new each time", () => {
    const ids = new Set([newSessionId(), newSessionId(), newSessionId()]);
    expect(ids.size).toBe(3);
    for (const id of ids) {
      expect(id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    }
  });
});

describe("dictationReasonOf", () => {
  it("reads the reason of the shell's refusal", () => {
    const error = new CoreError({
      code: "InvalidInput",
      message: "busy",
      details: { field: "sessionId", reason: "busy" },
    });
    expect(dictationReasonOf(error)).toBe("busy");
    for (const code of DICTATION_ERROR_CODES) {
      expect(dictationReasonOf(new CoreError({ code: "InvalidInput", message: "x", details: { reason: code } }))).toBe(code);
    }
  });

  it("is undefined for anything else", () => {
    expect(dictationReasonOf(new Error("x"))).toBeUndefined();
    expect(dictationReasonOf(new CoreError({ code: "Internal", message: "x" }))).toBeUndefined();
    expect(
      dictationReasonOf(new CoreError({ code: "InvalidInput", message: "x", details: { reason: "declined" } })),
    ).toBeUndefined();
    expect(dictationReasonOf(null)).toBeUndefined();
  });
});

describe("dictationStatusOf", () => {
  const engine = {
    engine: "accurate",
    installed: false,
    downloadBytes: 670_478_772,
    languages: ["en", "fr"],
    attribution: "NVIDIA",
    available: true,
  };

  it("accepts what the shell sends and copies the known fields", () => {
    const status = dictationStatusOf(
      {
        selected: null,
        engines: [engine, { ...engine, engine: "ollama", available: false, unavailableReason: "aiOff", extra: 1 }],
      },
      "dictation_status",
    );
    expect(status.selected).toBeNull();
    expect(status.engines).toHaveLength(2);
    expect(status.engines[0]).toEqual(engine);
    expect(status.engines[1]).toEqual({ ...engine, engine: "ollama", available: false, unavailableReason: "aiOff" });
  });

  it("accepts a selected engine and a missing one", () => {
    expect(dictationStatusOf({ selected: "light", engines: [] }, "x").selected).toBe("light");
    expect(dictationStatusOf({ engines: [] }, "x").selected).toBeNull();
  });

  it("refuses anything else with an Internal error naming the command", () => {
    for (const bad of [
      null,
      "status",
      {},
      { engines: "no" },
      { selected: "cloud", engines: [] },
      { engines: [{ ...engine, engine: "cloud" }] },
      { engines: [{ ...engine, installed: "yes" }] },
      { engines: [{ ...engine, downloadBytes: "1" }] },
      { engines: [{ ...engine, languages: [1] }] },
      { engines: [{ ...engine, unavailableReason: "tired" }] },
    ]) {
      expect(() => dictationStatusOf(bad, "dictation_status")).toThrow(/dictation_status/);
    }
  });
});

describe("dictationEventOf", () => {
  it("reads every event of the contract", () => {
    expect(dictationEventOf({ type: "state", state: "loading" })).toEqual({ type: "state", state: "loading" });
    expect(dictationEventOf({ type: "state", state: "recognising" })).toEqual({ type: "state", state: "recognising" });
    expect(dictationEventOf({ type: "phrase", text: "Hi." })).toEqual({ type: "phrase", text: "Hi." });
    expect(dictationEventOf({ type: "error", code: "engineCrashed" })).toEqual({ type: "error", code: "engineCrashed" });
    expect(dictationEventOf({ type: "ended" })).toEqual({ type: "ended" });
  });

  it("ignores what it does not know", () => {
    for (const bad of [
      null,
      "ended",
      {},
      { type: "state", state: "dancing" },
      { type: "phrase", text: "" },
      { type: "phrase", text: 3 },
      { type: "error", code: "nope" },
      { type: "future" },
    ]) {
      expect(dictationEventOf(bad)).toBeUndefined();
    }
  });
});

describe("installEventOf", () => {
  it("reads every event of the contract", () => {
    expect(installEventOf({ type: "progress", done: 5, total: 10 })).toEqual({ type: "progress", done: 5, total: 10 });
    expect(installEventOf({ type: "verifying" })).toEqual({ type: "verifying" });
    expect(installEventOf({ type: "done" })).toEqual({ type: "done" });
    expect(installEventOf({ type: "error", code: "hashMismatch" })).toEqual({ type: "error", code: "hashMismatch" });
  });

  it("ignores what it does not know", () => {
    for (const bad of [null, 1, {}, { type: "progress", done: "1", total: 2 }, { type: "error", code: "x" }, { type: "x" }]) {
      expect(installEventOf(bad)).toBeUndefined();
    }
  });
});
