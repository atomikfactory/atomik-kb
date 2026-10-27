// Local dictation (ADR-0053, `docs/architecture/dictation.md` section 5): the interface the UI dictates through,
// the parsers of what the desktop shell sends (checked, never trusted), the phrase-joining rule (a mirror of
// `kb_speech::policy::join_phrase`, tested with the same vectors) and a few constants. The Tauri
// implementation is `dictation-tauri.ts` (the `@atomik-kb/core-client/tauri` entry), the in-memory fake
// `dictation-memory.ts`.
//
// The types come from `kb-speech` by ts-rs (`cargo test -p kb-speech --features ts`); import them from here.
//
// Wired in: `Transport.dictation?: DictationApi`, `CoreClient.hasDictation()` / `dictation()`,
// `TauriTransport.dictation` (a `TauriDictation`), the `MemoryTransport` option `dictation` (a
// `MemoryDictationOptions`, or `"fake"`) and the dev page's `?dictation=fake`. The UI is in `packages/ui`
// (`editor/dictation/`, `screens/DictationSettings.tsx`; docs/architecture/dictation.md section 6).

import { CoreError } from "./errors";
import type { DictationErrorCode } from "./generated/DictationErrorCode";
import type { DictationEvent } from "./generated/DictationEvent";
import type { DictationState } from "./generated/DictationState";
import type { DictationStatus } from "./generated/DictationStatus";
import type { EngineStatus } from "./generated/EngineStatus";
import type { InstallEvent } from "./generated/InstallEvent";
import type { SpeechEngine } from "./generated/SpeechEngine";
import type { UnavailableReason } from "./generated/UnavailableReason";

export type {
  DictationErrorCode,
  DictationEvent,
  DictationState,
  DictationStatus,
  EngineStatus,
  InstallEvent,
  SpeechEngine,
  UnavailableReason,
};

/** Every engine, in the order Settings lists them (`SpeechEngine::ALL`). */
export const SPEECH_ENGINES: readonly SpeechEngine[] = ["accurate", "light", "ollama"];

/** The sample rate of the audio a session takes, Hz. */
export const DICTATION_RATE = 16_000;

/** How much audio one `push` should hold, milliseconds (100 to 200 are fine). */
export const DICTATION_CHUNK_MS = 100;

/** The most samples one `push` may hold: 2 s, what `dictation_audio` accepts (`MAX_PUSH_BYTES`). */
export const DICTATION_MAX_PUSH_SAMPLES = 32_000;

/** Every error code, for guards (`DictationErrorCode`). */
export const DICTATION_ERROR_CODES: readonly DictationErrorCode[] = [
  "modelMissing",
  "engineMissing",
  "engineFailed",
  "engineCrashed",
  "ollamaUnavailable",
  "busy",
  "downloadFailed",
  "hashMismatch",
  "cancelled",
  "consentDeclined",
  "importInvalid",
  "storeFailed",
];

/** A running dictation session. */
export interface DictationSession {
  /** The id the shell knows the session by. */
  readonly id: string;
  /**
   * Sends 16 kHz mono audio (about {@link DICTATION_CHUNK_MS} of it per call, never more than
   * {@link DICTATION_MAX_PUSH_SAMPLES} samples). Fire and forget: the pushes reach the engine in the order they
   * were made, and a push to a session that has ended is dropped.
   */
  push(pcm: Int16Array): void;
  /** The person stopped: the engine finishes what it heard, its last phrases arrive, then `ended`. */
  stop(): Promise<void>;
  /** Ends the session now, without the last phrases; `ended` follows. */
  cancel(): Promise<void>;
}

/** What the UI dictates through. */
export interface DictationApi {
  /** The engines and the engine chosen on this device. */
  status(): Promise<DictationStatus>;
  /** Chooses the engine of this device; any engine, installed or not. */
  select(engine: SpeechEngine): Promise<DictationStatus>;
  /**
   * Downloads the model of an engine after the shell's native consent dialog. `onEvent` gets `progress`,
   * `verifying`, then `done` or `error`; the promise resolves when the installation has ended either way, and
   * rejects (a {@link CoreError} with the reason in `details.reason`, see {@link dictationReasonOf}) when
   * nothing started: the dialog was declined, another installation runs, the core is locked.
   */
  install(engine: SpeechEngine, onEvent: (event: InstallEvent) => void): Promise<void>;
  /** Cancels the installation that runs; its last event is `error` with the code `cancelled`. */
  cancelInstall(): Promise<void>;
  /** "Use model files...": a native folder dialog, then the files are checked and copied. */
  importModel(engine: SpeechEngine): Promise<DictationStatus>;
  /** Deletes the model files of an engine. */
  remove(engine: SpeechEngine): Promise<DictationStatus>;
  /**
   * Starts a session. Resolves once it is registered and the engine runs, then `onEvent` gets `state`, `phrase`
   * and `error` events and, last, `ended`. Rejects with a {@link CoreError} (see {@link dictationReasonOf}) when
   * nothing started: `busy`, `modelMissing`, `engineMissing`, `ollamaUnavailable`.
   */
  start(engine: SpeechEngine, onEvent: (event: DictationEvent) => void): Promise<DictationSession>;
}

/** The reason of a failure to start or install: the `details.reason` of the shell's `ApiError`, when it is a code. */
export function dictationReasonOf(error: unknown): DictationErrorCode | undefined {
  if (!(error instanceof CoreError)) {
    return undefined;
  }
  const details: unknown = error.details;
  if (typeof details !== "object" || details === null || !("reason" in details)) {
    return undefined;
  }
  const reason = details.reason;
  return DICTATION_ERROR_CODES.find((code) => code === reason);
}

/** A session id: 1 to 64 of `[A-Za-z0-9_-]`, what `dictation_start` takes. */
export function newSessionId(): string {
  const uuid = (globalThis.crypto as Crypto | undefined)?.randomUUID?.();
  const body = uuid ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;
  return `dict-${body}`.slice(0, 64);
}

// ----- joining a phrase to the text before the cursor ---------------------------------------------

/** Unicode `White_Space`, the set Rust's `char::is_whitespace` uses (JavaScript's `\s` differs at U+0085 and U+FEFF). */
const WHITESPACE = "\\u0009-\\u000d\\u0020\\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const WHITESPACE_RUN = new RegExp(`[${WHITESPACE}]+`, "u");
const WHITESPACE_CHAR = new RegExp(`^[${WHITESPACE}]$`, "u");

/** Marks that stay attached to the text before them when a phrase starts with one. */
export const PHRASE_CLOSERS = [",", ".", ";", ":", "!", "?", ")", "]", "}", "…"] as const;

/** Marks after which no space is added. */
export const PHRASE_OPENERS = ["(", "[", "{", "¿", "¡"] as const;

/** Line breaks (`\n`, `\v`, `\f`, `\r`, NEL, LINE SEPARATOR, PARAGRAPH SEPARATOR): a phrase after one starts a line. */
const LINE_BREAKS = "\\u000a-\\u000d\\u0085\\u2028\\u2029";
const LINE_BREAK_CHAR = new RegExp(`^[${LINE_BREAKS}]$`, "u");
/** A letter with case or a digit: what the text before the cursor may end with when its sentence goes on. */
const CASED_LETTER_OR_DIGIT = /^[\p{Lowercase}\p{Uppercase}\p{N}]$/u;
const UPPERCASE = /^\p{Uppercase}$/u;
const LOWERCASE = /^\p{Lowercase}$/u;

/**
 * Whether the text before the cursor leaves its sentence open, so that a capital at the cursor is not a sentence's start:
 * the last mark that is not white space is a lowercase or uppercase letter, a digit, a comma or a semicolon, and no line
 * break lies between it and the cursor. Nothing before the cursor, only white space, a full stop or any other end mark, a
 * closing quote or bracket, a dash, a colon, a letter of a script without case and an inline atom all say no.
 */
function continuesSentence(before: string): boolean {
  for (const character of [...before].reverse()) {
    if (LINE_BREAK_CHAR.test(character)) {
      return false;
    }
    if (WHITESPACE_CHAR.test(character)) {
      continue;
    }
    return CASED_LETTER_OR_DIGIT.test(character) || character === "," || character === ";";
  }
  return false;
}

/**
 * `word` with its capital lowered, `word` itself when it is not one to lower: its first character must be an uppercase
 * letter, its second a lowercase letter, and no later character of the word an uppercase letter (so a single letter, `I'm`,
 * `NASA`, `McDonald` and `Jean-Pierre` stay).
 */
function lowerInitial(word: string): string {
  const [first, second, ...rest] = [...word];
  if (
    first === undefined ||
    second === undefined ||
    !UPPERCASE.test(first) ||
    !LOWERCASE.test(second) ||
    rest.some((character) => UPPERCASE.test(character))
  ) {
    return word;
  }
  return first.toLowerCase() + word.slice(first.length);
}

/**
 * What to insert at the cursor for a recognised phrase: `text` trimmed, with every run of whitespace collapsed to
 * one space, and a leading space unless the phrase is at the start of a block (`before` is empty), follows
 * whitespace or an opening mark, or begins with a closing mark. `null` when `text` is only whitespace.
 *
 * `before` is the text before the cursor in the same text block, `""` at its start (only its end matters).
 *
 * **Mid-sentence capital.** Parakeet starts every phrase with a capital, even when the speaker only paused. When the text
 * before the cursor leaves its sentence open (it ends, past white space, in a letter with case, a digit, a comma or a
 * semicolon, with no line break between), the first letter of the phrase is lowered, but only when it is an uppercase letter
 * followed by a lowercase one and the rest of the first word has no capital: `The`, `Then`, `Élan` become `the`, `then`, `élan`,
 * while `I`, `I'm`, `NASA`, `OK`, `McDonald` and `Jean-Pierre` are kept. A name such as `Paris` looks like any sentence's
 * first word and is lowered too (a limit). The phrase's full stop stays.
 *
 * A mirror of `kb_speech::policy::join_phrase`; `crates/kb-speech/testdata/join_phrase.json` holds the vectors of both.
 */
export function joinPhrase(before: string, text: string): string | null {
  const words = text.split(WHITESPACE_RUN).filter((word) => word !== "");
  const first = words[0];
  if (first === undefined) {
    return null;
  }
  const firstMark = [...first][0] ?? "";
  const prev = lastCharacterOf(before);
  const attached =
    prev === null ||
    WHITESPACE_CHAR.test(prev) ||
    (PHRASE_OPENERS as readonly string[]).includes(prev) ||
    (PHRASE_CLOSERS as readonly string[]).includes(firstMark);
  words[0] = continuesSentence(before) ? lowerInitial(first) : first;
  return (attached ? "" : " ") + words.join(" ");
}

/** The last code point of `text`, `null` when it is empty: the `prev` of {@link joinPhrase} from the text before the cursor. */
export function lastCharacterOf(text: string): string | null {
  const characters = [...text];
  return characters[characters.length - 1] ?? null;
}

// ----- parsing what the shell sends -----------------------------------------------------------------

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isEngine(value: unknown): value is SpeechEngine {
  return SPEECH_ENGINES.some((engine) => engine === value);
}

function isErrorCode(value: unknown): value is DictationErrorCode {
  return DICTATION_ERROR_CODES.some((code) => code === value);
}

const UNAVAILABLE_REASONS: readonly UnavailableReason[] = ["engineMissing", "aiOff", "noTranscriptionModel", "consentRequired"];

function protocolError(command: string, what: string): CoreError {
  return new CoreError({ code: "Internal", message: `The desktop shell answered ${command} with ${what}` });
}

/** Validates what `dictation_status`, `dictation_select`, `dictation_import` and `dictation_remove` resolve with. */
export function dictationStatusOf(value: unknown, command: string): DictationStatus {
  if (!isRecord(value) || !Array.isArray(value.engines)) {
    throw protocolError(command, "something that is not a dictation status");
  }
  const selected = value.selected;
  if (selected !== null && selected !== undefined && !isEngine(selected)) {
    throw protocolError(command, "an unknown selected engine");
  }
  const engines: EngineStatus[] = (value.engines as unknown[]).map((item) => {
    if (
      !isRecord(item) ||
      !isEngine(item.engine) ||
      typeof item.installed !== "boolean" ||
      typeof item.downloadBytes !== "number" ||
      !Array.isArray(item.languages) ||
      !item.languages.every((language) => typeof language === "string") ||
      typeof item.attribution !== "string" ||
      typeof item.available !== "boolean"
    ) {
      throw protocolError(command, "an engine status that is not one");
    }
    const engine: EngineStatus = {
      engine: item.engine,
      installed: item.installed,
      downloadBytes: item.downloadBytes,
      languages: [...item.languages],
      attribution: item.attribution,
      available: item.available,
    };
    const reason = item.unavailableReason;
    if (reason !== undefined && reason !== null) {
      const known = UNAVAILABLE_REASONS.find((candidate) => candidate === reason);
      if (known === undefined) {
        throw protocolError(command, "an unknown unavailable reason");
      }
      engine.unavailableReason = known;
    }
    return engine;
  });
  return { selected: selected ?? null, engines };
}

const SESSION_STATES: readonly DictationState[] = ["loading", "listening", "recognising"];

/** Reads one message of a session's channel; `undefined` for anything that is not a known event. */
export function dictationEventOf(message: unknown): DictationEvent | undefined {
  if (!isRecord(message)) {
    return undefined;
  }
  switch (message.type) {
    case "state": {
      const state = SESSION_STATES.find((candidate) => candidate === message.state);
      return state === undefined ? undefined : { type: "state", state };
    }
    case "phrase":
      return typeof message.text === "string" && message.text !== "" ? { type: "phrase", text: message.text } : undefined;
    case "error":
      return isErrorCode(message.code) ? { type: "error", code: message.code } : undefined;
    case "ended":
      return { type: "ended" };
    default:
      return undefined;
  }
}

/** Reads one message of an installation's channel; `undefined` for anything that is not a known event. */
export function installEventOf(message: unknown): InstallEvent | undefined {
  if (!isRecord(message)) {
    return undefined;
  }
  switch (message.type) {
    case "progress":
      return typeof message.done === "number" && typeof message.total === "number"
        ? { type: "progress", done: message.done, total: message.total }
        : undefined;
    case "verifying":
      return { type: "verifying" };
    case "done":
      return { type: "done" };
    case "error":
      return isErrorCode(message.code) ? { type: "error", code: message.code } : undefined;
    default:
      return undefined;
  }
}
