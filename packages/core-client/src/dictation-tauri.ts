// The desktop implementation of `DictationApi` (ADR-0053, `docs/architecture/dictation.md` section 5): the
// commands `dictation_*` of the Tauri shell (`apps/desktop/src-tauri/src/speech_host.rs`) over `invoke`, with one
// `Channel` per session and per installation. Part of the `@atomik-kb/core-client/tauri` entry, so web builds do
// not pull in `@tauri-apps/api`.

import { Channel, invoke } from "@tauri-apps/api/core";

import {
  dictationEventOf,
  dictationStatusOf,
  installEventOf,
  newSessionId,
  type DictationApi,
  type DictationEvent,
  type DictationSession,
  type DictationStatus,
  type InstallEvent,
  type SpeechEngine,
} from "./dictation";
import { toCoreError } from "./errors";

/** The request header of `dictation_audio`: the session id (`apps/desktop/src-tauri/src/speech_host.rs`). */
const SESSION_HEADER = "x-kb-dictation";

/** Runs a command and turns whatever it rejects with into a `CoreError`. */
async function call<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (error) {
    throw toCoreError(error);
  }
}

/** A session of the shell. The audio goes out one `dictation_audio` at a time, so that it reaches the engine in order. */
class TauriDictationSession implements DictationSession {
  readonly id: string;
  #pushes: Promise<void> = Promise.resolve();
  #ended = false;

  constructor(id: string) {
    this.id = id;
  }

  push(pcm: Int16Array): void {
    if (this.#ended || pcm.length === 0) {
      return;
    }
    // Copy: the caller may reuse its buffer before the call goes out. Typed arrays are little-endian on every
    // platform the app runs on, which is what the shell reads.
    const bytes = new Uint8Array(pcm.slice().buffer);
    this.#pushes = this.#pushes.then(async () => {
      if (this.#ended) {
        return;
      }
      try {
        await invoke("dictation_audio", bytes, { headers: { [SESSION_HEADER]: this.id } });
      } catch {
        // The session ended in the shell (its `ended` event says so); the audio has nowhere to go.
      }
    });
  }

  async stop(): Promise<void> {
    // Everything pushed so far reaches the engine before it is told to finish.
    await this.#pushes;
    this.#ended = true;
    await call(() => invoke("dictation_stop", { sessionId: this.id }));
  }

  async cancel(): Promise<void> {
    this.#ended = true;
    await call(() => invoke("dictation_cancel", { sessionId: this.id }));
  }
}

/** Dictation through the desktop shell. */
export class TauriDictation implements DictationApi {
  async status(): Promise<DictationStatus> {
    return dictationStatusOf(await call(() => invoke<unknown>("dictation_status")), "dictation_status");
  }

  async select(engine: SpeechEngine): Promise<DictationStatus> {
    return dictationStatusOf(await call(() => invoke<unknown>("dictation_select", { engine })), "dictation_select");
  }

  async install(engine: SpeechEngine, onEvent: (event: InstallEvent) => void): Promise<void> {
    const channel = new Channel<unknown>((message) => {
      const event = installEventOf(message);
      if (event !== undefined) {
        onEvent(event);
      }
    });
    await call(() => invoke("dictation_install", { engine, onEvent: channel }));
  }

  async cancelInstall(): Promise<void> {
    await call(() => invoke("dictation_install_cancel"));
  }

  async importModel(engine: SpeechEngine): Promise<DictationStatus> {
    return dictationStatusOf(await call(() => invoke<unknown>("dictation_import", { engine })), "dictation_import");
  }

  async remove(engine: SpeechEngine): Promise<DictationStatus> {
    return dictationStatusOf(await call(() => invoke<unknown>("dictation_remove", { engine })), "dictation_remove");
  }

  async start(engine: SpeechEngine, onEvent: (event: DictationEvent) => void): Promise<DictationSession> {
    const sessionId = newSessionId();
    const session = new TauriDictationSession(sessionId);
    const channel = new Channel<unknown>((message) => {
      const event = dictationEventOf(message);
      if (event !== undefined) {
        onEvent(event);
      }
    });
    await call(() => invoke("dictation_start", { sessionId, engine, onEvent: channel }));
    return session;
  }
}
