// An in-memory `DictationApi` (ADR-0053, `docs/architecture/dictation.md` section 5): what the dev page
// (`?dictation=fake`) and the UI tests dictate through. It behaves like the desktop shell as far as the contract
// goes: the same events in the same order, the same refusals (`busy`, `modelMissing`, `ollamaUnavailable`), an
// installation with progress, and scripted phrases instead of recognition: one phrase for every
// `samplesPerPhrase` samples of audio pushed (about 2 s), and one more on `stop` when audio came in since the
// last phrase. It stores and sends nothing.

import {
  DICTATION_RATE,
  SPEECH_ENGINES,
  type DictationApi,
  type DictationErrorCode,
  type DictationEvent,
  type DictationSession,
  type DictationStatus,
  type EngineStatus,
  type InstallEvent,
  type SpeechEngine,
  type UnavailableReason,
} from "./dictation";
import { CoreError } from "./errors";

/** The phrases the fake "recognises", in order, over and over. */
export const FAKE_PHRASES: readonly string[] = [
  "This is the first phrase.",
  "and here is a second one,",
  "then a third, without a capital",
  "Done.",
];

/** What the fake pretends to download, per engine: the sizes the catalog has (`kb-speech`), bytes. */
const DOWNLOAD_BYTES: Record<SpeechEngine, number> = {
  accurate: 670_478_772,
  light: 123_967_539,
  ollama: 0,
};

const LANGUAGES: Record<SpeechEngine, readonly string[]> = {
  accurate: [
    "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt", "pl", "pt", "ro",
    "sk", "sl", "es", "sv", "ru", "uk",
  ],
  light: ["en"],
  ollama: [],
};

const ATTRIBUTION: Record<SpeechEngine, string> = {
  accurate: "NVIDIA Parakeet TDT 0.6B v3, licensed CC-BY-4.0 (https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3); int8 conversion for sherpa-onnx by k2-fsa.",
  light: "Moonshine tiny by Useful Sensors, MIT licence (https://github.com/usefulsensors/moonshine); int8 conversion for sherpa-onnx by k2-fsa.",
  ollama: "",
};

/** How the fake starts. */
export interface MemoryDictationOptions {
  /** The phrases to "recognise"; default {@link FAKE_PHRASES}. */
  phrases?: readonly string[];
  /** Samples of audio per phrase; default 32,000 (2 s). */
  samplesPerPhrase?: number;
  /** Milliseconds from `start` to `listening` (the model loading); default 30. */
  loadMs?: number;
  /** Milliseconds between the progress events of an installation; default 30. It takes ten of them. */
  installStepMs?: number;
  /** Which engines start installed; default none. */
  installed?: readonly SpeechEngine[];
  /** The engine chosen on this "device"; default none. */
  selected?: SpeechEngine | null;
  /** Why the Ollama engine is unavailable; default `null` (it works). */
  ollamaUnavailable?: UnavailableReason | null;
  /** Makes every installation fail with this code (after its first progress event). */
  failInstallWith?: DictationErrorCode;
  /** Makes `start` reject with this code. */
  failStartWith?: DictationErrorCode;
  /** Whether the person agrees in the (pretend) native download dialog; default true. */
  consent?: boolean;
}

const INSTALL_STEPS = 10;

function refusal(reason: DictationErrorCode, field: string, message: string): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { field, reason } });
}

/** The fake dictation. */
export class MemoryDictation implements DictationApi {
  readonly #options: MemoryDictationOptions;
  readonly #installed: Set<SpeechEngine>;
  #selected: SpeechEngine | null;
  #session: FakeSession | undefined;
  #install: { engine: SpeechEngine; cancel: () => void } | undefined;

  constructor(options: MemoryDictationOptions = {}) {
    this.#options = options;
    this.#installed = new Set(options.installed ?? []);
    this.#selected = options.selected ?? null;
  }

  /** The engines that are installed now (for tests). */
  get installedEngines(): readonly SpeechEngine[] {
    return SPEECH_ENGINES.filter((engine) => this.#installed.has(engine));
  }

  /** The id of the session that runs, if any (for tests). */
  get runningSession(): string | undefined {
    return this.#session?.id;
  }

  #engineStatus(engine: SpeechEngine): EngineStatus {
    const unavailable = engine === "ollama" ? (this.#options.ollamaUnavailable ?? null) : null;
    const status: EngineStatus = {
      engine,
      installed: engine === "ollama" ? true : this.#installed.has(engine),
      downloadBytes: DOWNLOAD_BYTES[engine],
      languages: [...LANGUAGES[engine]],
      attribution: ATTRIBUTION[engine],
      available: unavailable === null,
    };
    if (unavailable !== null) {
      status.unavailableReason = unavailable;
    }
    return status;
  }

  #status(): DictationStatus {
    return { selected: this.#selected, engines: SPEECH_ENGINES.map((engine) => this.#engineStatus(engine)) };
  }

  status(): Promise<DictationStatus> {
    return Promise.resolve(this.#status());
  }

  select(engine: SpeechEngine): Promise<DictationStatus> {
    this.#selected = engine;
    return Promise.resolve(this.#status());
  }

  async install(engine: SpeechEngine, onEvent: (event: InstallEvent) => void): Promise<void> {
    if (engine === "ollama") {
      throw refusal("modelMissing", "engine", "this engine has nothing to download");
    }
    if (this.#install !== undefined) {
      throw refusal("busy", "engine", "another installation is running");
    }
    if (this.#options.consent === false) {
      throw refusal("consentDeclined", "engine", "the download was declined");
    }
    const total = DOWNLOAD_BYTES[engine];
    const step = this.#options.installStepMs ?? 30;
    let cancelled = false;
    let wake: (() => void) | undefined;
    this.#install = {
      engine,
      cancel: () => {
        cancelled = true;
        wake?.();
      },
    };
    const wait = () =>
      new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, step);
        wake = () => {
          clearTimeout(timer);
          resolve();
        };
      });
    try {
      for (let index = 1; index <= INSTALL_STEPS; index += 1) {
        await wait();
        if (cancelled) {
          onEvent({ type: "error", code: "cancelled" });
          return;
        }
        onEvent({ type: "progress", done: Math.floor((total * index) / INSTALL_STEPS), total });
        if (index === 1 && this.#options.failInstallWith !== undefined) {
          onEvent({ type: "error", code: this.#options.failInstallWith });
          return;
        }
      }
      onEvent({ type: "verifying" });
      await wait();
      if (cancelled) {
        onEvent({ type: "error", code: "cancelled" });
        return;
      }
      this.#installed.add(engine);
      onEvent({ type: "done" });
    } finally {
      this.#install = undefined;
    }
  }

  cancelInstall(): Promise<void> {
    this.#install?.cancel();
    return Promise.resolve();
  }

  importModel(engine: SpeechEngine): Promise<DictationStatus> {
    if (engine === "ollama") {
      return Promise.reject(refusal("importInvalid", "engine", "this engine has no model files"));
    }
    this.#installed.add(engine);
    return Promise.resolve(this.#status());
  }

  remove(engine: SpeechEngine): Promise<DictationStatus> {
    if (this.#session?.engine === engine) {
      return Promise.reject(refusal("busy", "engine", "a session uses this engine"));
    }
    this.#installed.delete(engine);
    return Promise.resolve(this.#status());
  }

  start(engine: SpeechEngine, onEvent: (event: DictationEvent) => void): Promise<DictationSession> {
    if (this.#session !== undefined) {
      return Promise.reject(refusal("busy", "sessionId", "another dictation session is running"));
    }
    if (this.#options.failStartWith !== undefined) {
      return Promise.reject(refusal(this.#options.failStartWith, "engine", "the session could not start"));
    }
    if (engine === "ollama") {
      if (this.#options.ollamaUnavailable !== null && this.#options.ollamaUnavailable !== undefined) {
        return Promise.reject(refusal("ollamaUnavailable", "engine", "Ollama cannot be used"));
      }
    } else if (!this.#installed.has(engine)) {
      return Promise.reject(refusal("modelMissing", "engine", "the model is not installed"));
    }
    const session = new FakeSession(
      engine,
      this.#options.phrases ?? FAKE_PHRASES,
      this.#options.samplesPerPhrase ?? DICTATION_RATE * 2,
      this.#options.loadMs ?? 30,
      onEvent,
      () => {
        if (this.#session === session) {
          this.#session = undefined;
        }
      },
    );
    this.#session = session;
    return Promise.resolve(session);
  }
}

/** A running fake session. */
class FakeSession implements DictationSession {
  readonly id: string;
  readonly engine: SpeechEngine;
  readonly #phrases: readonly string[];
  readonly #perPhrase: number;
  readonly #onEvent: (event: DictationEvent) => void;
  readonly #release: () => void;
  #pending = 0;
  #spoken = 0;
  #over = false;
  #ready = false;
  #loading: ReturnType<typeof setTimeout>;

  constructor(
    engine: SpeechEngine,
    phrases: readonly string[],
    perPhrase: number,
    loadMs: number,
    onEvent: (event: DictationEvent) => void,
    release: () => void,
  ) {
    this.id = `fake-${Math.random().toString(36).slice(2, 10)}`;
    this.engine = engine;
    this.#phrases = phrases;
    this.#perPhrase = Math.max(1, perPhrase);
    this.#onEvent = onEvent;
    this.#release = release;
    onEvent({ type: "state", state: "loading" });
    this.#loading = setTimeout(() => {
      this.#ready = true;
      if (!this.#over) {
        onEvent({ type: "state", state: "listening" });
        this.#phraseWhileDue();
      }
    }, loadMs);
  }

  #phrase(): void {
    const phrases = this.#phrases;
    const text = phrases[this.#spoken % Math.max(1, phrases.length)];
    this.#spoken += 1;
    if (text === undefined) {
      return;
    }
    this.#onEvent({ type: "state", state: "recognising" });
    this.#onEvent({ type: "phrase", text });
    this.#onEvent({ type: "state", state: "listening" });
  }

  #phraseWhileDue(): void {
    while (this.#pending >= this.#perPhrase && !this.#over) {
      this.#pending -= this.#perPhrase;
      this.#phrase();
    }
  }

  push(pcm: Int16Array): void {
    if (this.#over) {
      return;
    }
    this.#pending += pcm.length;
    // Audio pushed while the model "loads" is kept and recognised when it is ready, like in the shell.
    if (this.#ready) {
      this.#phraseWhileDue();
    }
  }

  stop(): Promise<void> {
    if (this.#over) {
      return Promise.resolve();
    }
    clearTimeout(this.#loading);
    if (!this.#ready) {
      this.#ready = true;
      this.#onEvent({ type: "state", state: "listening" });
    }
    this.#phraseWhileDue();
    if (this.#pending > 0) {
      this.#pending = 0;
      this.#phrase();
    }
    this.#finish();
    return Promise.resolve();
  }

  cancel(): Promise<void> {
    if (!this.#over) {
      clearTimeout(this.#loading);
      this.#finish();
    }
    return Promise.resolve();
  }

  #finish(): void {
    this.#over = true;
    this.#release();
    this.#onEvent({ type: "ended" });
  }
}
