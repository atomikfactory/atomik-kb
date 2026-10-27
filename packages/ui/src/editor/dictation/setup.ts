// The engines of dictation as Settings and the first-use dialog show them (ADR-0053, `docs/architecture/dictation.md`
// section 6): the status of the shell, the choice of this device, and the installation of a model with its progress. One
// model per `DictationApi`, shared by the two places that use it (only one installation runs at a time, in the shell).
//
// Nothing here stores or sends anything: it calls the shell's commands and keeps what they answered. A refused
// download (the person said Cancel in the shell's own dialog) leaves no trace, and so does a cancelled one; every other
// failure is a sentence in `notice`.

import { dictationReasonOf, toCoreError } from "@atomik-kb/core-client";
import type { DictationApi, DictationErrorCode, DictationStatus, EngineStatus, SpeechEngine } from "@atomik-kb/core-client";

import { t } from "../../i18n";
import { Observable } from "../ai/observable";
import { codeText, errorText } from "./text";

/** An installation under way. */
export interface InstallProgress {
  engine: SpeechEngine;
  /** `starting` until the first byte (the shell's own dialog may be open), then `downloading`, then `verifying`. */
  phase: "starting" | "downloading" | "verifying";
  /** Bytes of the whole model so far. */
  done: number;
  /** Bytes of the whole model; 0 until the shell says. */
  total: number;
}

export interface SetupState {
  /** The last status of the shell; `null` before the first read. */
  status: DictationStatus | null;
  /** A status read is running. */
  loading: boolean;
  /** A sentence the person should read (the last failure); `null` when there is none. */
  notice: string | null;
  /** The installation that runs, `null` when none does. */
  install: InstallProgress | null;
  /** A choice, an import or a removal is running. */
  working: boolean;
}

/** How an installation ended. */
export type InstallOutcome =
  /** The model is installed. */
  | "done"
  /** The person said Cancel in the shell's dialog: nothing started, nothing to say. */
  | "declined"
  /** Cancelled with the Cancel button (or by a lock): the files are gone, nothing to say. */
  | "cancelled"
  /** It failed; `notice` says why. */
  | "failed";

const START: SetupState = { status: null, loading: false, notice: null, install: null, working: false };

/** The engine of `status` named `engine`. */
export function engineOf(status: DictationStatus | null, engine: SpeechEngine): EngineStatus | undefined {
  return status?.engines.find((candidate) => candidate.engine === engine);
}

/**
 * Whether an engine can start a session now: it is installed (Ollama always is, its model is the AI's) and can run
 * here. A downloadable engine that is not installed is "available" in the shell's words once it is.
 */
export function isReady(engine: EngineStatus | undefined): boolean {
  return engine !== undefined && engine.installed && engine.available;
}

/** The engine of this device when it can start a session now, else `null`. */
export function readyEngine(status: DictationStatus): SpeechEngine | null {
  return status.selected !== null && isReady(engineOf(status, status.selected)) ? status.selected : null;
}

export class DictationSetup {
  readonly state = new Observable<SetupState>(START);
  readonly #api: DictationApi;
  #readTicket = 0;

  constructor(api: DictationApi) {
    this.#api = api;
  }

  /** Reads the status again. Reports a failure in `notice`; answers the status, `null` when it could not be read. */
  async refresh(): Promise<DictationStatus | null> {
    const ticket = (this.#readTicket += 1);
    this.state.update((state) => ({ ...state, loading: true }));
    try {
      const status = await this.#api.status();
      if (ticket === this.#readTicket) {
        this.state.update((state) => ({ ...state, status, loading: false }));
      }
      return status;
    } catch (error) {
      if (ticket === this.#readTicket) {
        this.state.update((state) => ({
          ...state,
          loading: false,
          notice: t("settings.dictation.error", { message: errorText(error) }),
        }));
      }
      return null;
    }
  }

  /** Forgets the notice. */
  clearNotice(): void {
    this.state.update((state) => (state.notice === null ? state : { ...state, notice: null }));
  }

  /** Chooses the engine of this device. Answers whether the shell took it. */
  async select(engine: SpeechEngine): Promise<boolean> {
    return this.#work(async () => {
      const status = await this.#api.select(engine);
      this.#readTicket += 1;
      this.state.update((state) => ({ ...state, status }));
    });
  }

  /** "Use model files...": the shell's folder dialog, the checks and the copy. Answers whether the model is installed now. */
  async importModel(engine: SpeechEngine): Promise<boolean> {
    return this.#work(async () => {
      const status = await this.#api.importModel(engine);
      this.#readTicket += 1;
      this.state.update((state) => ({ ...state, status }));
    }, (error) => (isDialogCancelled(error) ? null : errorText(error)));
  }

  /** Deletes the model files of an engine. Answers whether they are gone. */
  async remove(engine: SpeechEngine): Promise<boolean> {
    return this.#work(async () => {
      const status = await this.#api.remove(engine);
      this.#readTicket += 1;
      this.state.update((state) => ({ ...state, status }));
    });
  }

  /**
   * Downloads the model of `engine` after the shell's consent dialog, with progress in `state.install`. It does not
   * choose the engine: call {@link DictationSetup.select} for that.
   */
  async install(engine: SpeechEngine): Promise<InstallOutcome> {
    if (this.state.get().install !== null) {
      return "failed";
    }
    this.state.update((state) => ({ ...state, notice: null, install: { engine, phase: "starting", done: 0, total: 0 } }));
    // The code of the `error` event the installation ended with (a holder: the callback below sets it).
    const ended: { code: DictationErrorCode | null } = { code: null };
    try {
      await this.#api.install(engine, (event) => {
        switch (event.type) {
          case "progress":
            this.state.update((state) =>
              state.install === null ? state : { ...state, install: { engine, phase: "downloading", done: event.done, total: event.total } },
            );
            break;
          case "verifying":
            this.state.update((state) => (state.install === null ? state : { ...state, install: { ...state.install, phase: "verifying" } }));
            break;
          case "error":
            ended.code = event.code;
            break;
          case "done":
            break;
        }
      });
    } catch (error) {
      this.state.update((state) => ({ ...state, install: null }));
      if (dictationReasonOf(error) === "consentDeclined") {
        return "declined";
      }
      this.state.update((state) => ({ ...state, notice: errorText(error) }));
      return "failed";
    }
    this.state.update((state) => ({ ...state, install: null }));
    const { code } = ended;
    if (code !== null && code !== "cancelled") {
      this.state.update((state) => ({ ...state, notice: codeText(code) }));
    }
    await this.refresh();
    return code === null ? "done" : code === "cancelled" ? "cancelled" : "failed";
  }

  /** Cancels the installation that runs; its last event is an error with the code `cancelled`. */
  cancelInstall(): void {
    this.#api.cancelInstall().catch((error: unknown) => {
      console.error("the download could not be cancelled", toCoreError(error).code);
    });
  }

  /** Runs a call of the shell that answers a status; failures become the notice. */
  async #work(run: () => Promise<void>, describe: (error: unknown) => string | null = errorText): Promise<boolean> {
    this.state.update((state) => ({ ...state, working: true, notice: null }));
    try {
      await run();
      this.state.update((state) => ({ ...state, working: false }));
      return true;
    } catch (error) {
      const notice = describe(error);
      this.state.update((state) => ({ ...state, working: false, notice }));
      return false;
    }
  }
}

/** The person closed the shell's folder dialog: `InvalidInput` for `path`, nothing to say. */
function isDialogCancelled(error: unknown): boolean {
  const details: unknown = toCoreError(error).details;
  return typeof details === "object" && details !== null && "field" in details && details.field === "path";
}

const models = new WeakMap<DictationApi, DictationSetup>();

/** The model of `api`, made once. */
export function setupOf(api: DictationApi): DictationSetup {
  let model = models.get(api);
  if (model === undefined) {
    model = new DictationSetup(api);
    models.set(api, model);
  }
  return model;
}
