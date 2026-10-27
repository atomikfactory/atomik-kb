// The dictation of one page editor (ADR-0053, `docs/architecture/dictation.md` sections 5 and 6): it asks for the
// microphone, starts a session of the engine chosen on this device, pushes the microphone's samples to it in chunks of
// 100 to 200 ms, puts each phrase that comes back into the page at the cursor (`insert.ts`), and ends cleanly.
//
// - **Start.** The engine of this device must be installed and able to run; otherwise the controller waits in the phase
//   `setup` for the first-use dialog (`SetupDialog.tsx`), which installs or chooses one and calls {@link DictationController.proceed}.
//   The microphone is asked for before the session starts, so that a denied request loads no model.
// - **Stop.** Esc, the bar's Stop and a page that turns read-only end it with {@link DictationController.stop}: the audio
//   still held is pushed, the microphone is let go, the engine finishes what it heard, its last phrases arrive and the bar
//   goes away at `ended`. Phrases are never put into a page that is read-only. {@link DictationController.cancel} (the
//   editor is destroyed) drops everything at once, and so does {@link DictationController.leave} (another page or tab
//   comes to the front, the app locks): the editor goes away with the page, so a phrase still being recognised could go
//   nowhere, and the app says that it was dropped.
// - **Activity.** Each phrase and each `recognising` state is user activity for the auto-lock timer (`activity.ts`).
// - **Exclusion.** Dictation and the recorder both want the microphone and the cursor: neither starts while the other runs.

import type { DictationApi, DictationErrorCode, DictationEvent, DictationSession, SpeechEngine } from "@atomik-kb/core-client";
import { toCoreError } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";

import { reportActivity } from "../../activity";
import { t } from "../../i18n";
import { browserPcmEnv, CaptureError, createPcmCapture } from "../ai/capture";
import type { CaptureEvents, PcmCapture, PcmCaptureEnv } from "../ai/capture";
import { Observable } from "../ai/observable";
import { PcmChunker } from "./chunker";
import { insertPhrase } from "./insert";
import { readyEngine } from "./setup";
import { codeText, errorText } from "./text";

/**
 * Where a dictation is:
 * - `idle`: nothing runs (the bar shows nothing unless `notice` has something to say);
 * - `starting`: the engine of this device is being looked up and the microphone asked for;
 * - `setup`: no engine is ready, the first-use dialog is open;
 * - `loading`: the microphone runs and the engine loads its model (the audio is kept);
 * - `listening` and `recognising`: the engine's own states;
 * - `stopping`: Stop was pressed, the engine finishes (its last phrases still come in);
 * - `error`: it ended with a sentence the person should read (`notice`).
 */
export type DictationPhase = "idle" | "starting" | "setup" | "loading" | "listening" | "recognising" | "stopping" | "error";

export interface DictationUiState {
  phase: DictationPhase;
  /** The engine of the session (set once the microphone is being asked for). */
  engine: SpeechEngine | null;
  /** Seconds since the microphone started. */
  seconds: number;
  /** The loudness of the last block, 0 to 1. */
  level: number;
  /** A message the person should read (a denied microphone, a failure of the engine). */
  notice: string | null;
  /** The microphone has not answered for a few seconds: the person may have to answer a request. */
  permissionHint: boolean;
}

export const IDLE_DICTATION: DictationUiState = {
  phase: "idle",
  engine: null,
  seconds: 0,
  level: 0,
  notice: null,
  permissionHint: false,
};

/** How long the bar waits for the microphone before it says a request may be open (WebView2 asks in its own window). */
export const DICTATION_HINT_MS = 3000;

/** The longest the controller waits for `ended` after Stop: the shell's flush takes 20 s at most (section 4.4). */
export const STOP_WAIT_MS = 30_000;

/** Audio kept while the session is being registered (the shell holds the rest while the model loads), in pushes (about 60 s). */
const MAX_QUEUED_PUSHES = 300;

export interface DictationDeps {
  editor: Editor;
  /** The dictation of the transport, `null` when there is none. */
  api: () => DictationApi | null;
  /** Whether the editor's recorder runs (dictation does not start then). */
  recording: () => boolean;
  /** The browser objects (the page's by default); tests give a fake microphone. */
  env?: () => PcmCaptureEnv | null;
  /** The microphone, for a test that does not use the fake environment. */
  capture?: (events: CaptureEvents) => PcmCapture;
  /** Tests: how long to wait before the permission hint shows (default {@link DICTATION_HINT_MS}). */
  hintMs?: number;
  /** Tests: how long to wait for `ended` after Stop (default {@link STOP_WAIT_MS}). */
  stopWaitMs?: number;
}

function noop(): void {
  // nothing to do
}

/** Parts of the page whose Esc closes them: a menu, a list of choices, a dialog. */
const POPUP = '[role="menu"], [role="listbox"], [role="dialog"], [role="alertdialog"], [role="combobox"], [aria-modal="true"]';

/**
 * Whether an Esc that reached the window was meant for something other than the dictation: it came from inside a menu or a
 * dialog, or from a text field that is not the page (the find bar, a comment). The page's own keymaps (which take Esc as
 * well) do not count: with the cursor in the page, Esc is the way to stop.
 */
function closesSomethingElse(target: EventTarget | null, editor: Editor): boolean {
  if (!(target instanceof Element) || editor.view.dom.contains(target)) {
    return false;
  }
  return target.closest(POPUP) !== null || target.closest("input, textarea, select, [contenteditable]") !== null;
}

/** The sentence for a microphone that could not start. */
function describeCaptureError(error: unknown): string {
  if (error instanceof CaptureError) {
    switch (error.reason) {
      case "denied":
        return t("recorder.denied");
      case "noDevice":
        return t("recorder.noDevice");
      case "failed":
        return t("dictation.failed", { message: error.message });
    }
  }
  return t("dictation.failed", { message: toCoreError(error).message });
}

export class DictationController {
  readonly state = new Observable<DictationUiState>(IDLE_DICTATION);
  readonly #deps: DictationDeps;
  /** Bumped by every start, cancel and end: what an older run still does after that is dropped. */
  #run = 0;
  #capture: PcmCapture | null = null;
  #session: DictationSession | null = null;
  readonly #chunker = new PcmChunker();
  #queued: Int16Array[] = [];
  /** The code of the `error` event of this session, whose `ended` follows. */
  #errorCode: DictationErrorCode | null = null;
  #timer: ReturnType<typeof setInterval> | null = null;
  #hintTimer: ReturnType<typeof setTimeout> | null = null;
  #stopTimer: ReturnType<typeof setTimeout> | null = null;
  #since: number | null = null;
  #stopped: Promise<void> | null = null;
  #resolveStopped: (() => void) | null = null;

  constructor(deps: DictationDeps) {
    this.#deps = deps;
  }

  /** Whether the page's browser has a microphone API and Web Audio. */
  static supported(env: () => PcmCaptureEnv | null = browserPcmEnv): boolean {
    return env() !== null;
  }

  /** Dictation is under way, or its first-use dialog is open: another dictation and the recorder wait. */
  get busy(): boolean {
    const { phase } = this.state.get();
    return phase !== "idle" && phase !== "error";
  }

  /** Whether the editor's recorder runs. */
  get recording(): boolean {
    return this.#deps.recording();
  }

  /** Whether this device can capture the microphone at all. */
  get canCapture(): boolean {
    return this.#deps.capture !== undefined || DictationController.supported(this.#deps.env ?? browserPcmEnv);
  }

  /** The microphone is open and the engine is working: Esc and a page change stop it. */
  get #active(): boolean {
    const { phase } = this.state.get();
    return phase === "starting" || phase === "loading" || phase === "listening" || phase === "recognising";
  }

  // ----- starting ------------------------------------------------------------------------------------

  /** Starts dictating at the cursor: the first-use dialog when no engine is ready, else the microphone and the session. */
  async start(): Promise<void> {
    if (this.busy) {
      return;
    }
    const { editor } = this.#deps;
    const api = this.#deps.api();
    if (api === null || editor.isDestroyed || !editor.isEditable || this.#deps.recording()) {
      return;
    }
    if (!this.canCapture) {
      this.state.set({ ...IDLE_DICTATION, phase: "error", notice: t("recorder.unsupported") });
      return;
    }
    const run = (this.#run += 1);
    this.state.set({ ...IDLE_DICTATION, phase: "starting" });
    this.#hintTimer = setTimeout(() => {
      this.#hintTimer = null;
      if (run === this.#run) {
        this.state.update((state) => (state.phase === "starting" ? { ...state, permissionHint: true } : state));
      }
    }, this.#deps.hintMs ?? DICTATION_HINT_MS);
    let engine: SpeechEngine | null;
    try {
      engine = readyEngine(await api.status());
    } catch (error) {
      if (run === this.#run) {
        this.#fail(errorText(error));
      }
      return;
    }
    if (run !== this.#run) {
      return;
    }
    if (engine === null) {
      this.#clearHint();
      this.state.update((state) => ({ ...state, phase: "setup", permissionHint: false }));
      return;
    }
    await this.#begin(run, engine);
  }

  /** The first-use dialog is done: `engine` is installed and chosen. Starts the session with it. */
  async proceed(engine: SpeechEngine): Promise<void> {
    if (this.state.get().phase !== "setup") {
      return;
    }
    const run = this.#run;
    this.state.update((state) => ({ ...state, phase: "starting" }));
    await this.#begin(run, engine);
  }

  /** The first-use dialog was closed with nothing chosen. */
  dismissSetup(): void {
    if (this.state.get().phase === "setup") {
      this.cancel();
    }
  }

  async #begin(run: number, engine: SpeechEngine): Promise<void> {
    const api = this.#deps.api();
    const env = (this.#deps.env ?? browserPcmEnv)();
    const events: CaptureEvents = {
      onPcm: (samples) => {
        this.#onPcm(run, samples);
      },
      onLevel: (level) => {
        if (run === this.#run) {
          this.state.update((state) => (state.phase === "loading" || state.phase === "listening" || state.phase === "recognising" ? { ...state, level } : state));
        }
      },
    };
    const capture = this.#deps.capture?.(events) ?? (env === null ? null : createPcmCapture(events, env));
    if (api === null || capture === null) {
      this.#fail(t("recorder.unsupported"));
      return;
    }
    this.#capture = capture;
    this.state.update((state) => ({ ...state, engine }));
    try {
      await capture.start();
    } catch (error) {
      if (run === this.#run) {
        this.#fail(describeCaptureError(error));
      }
      return;
    }
    if (run !== this.#run) {
      capture.cancel(); // cancelled while the microphone was being asked for
      return;
    }
    this.#clearHint();
    this.#since = Date.now();
    this.#timer = setInterval(() => {
      this.#tick(run);
    }, 250);
    this.state.update((state) => ({ ...state, phase: "loading", permissionHint: false }));
    let session: DictationSession;
    try {
      session = await api.start(engine, (event) => {
        this.#onEvent(run, event);
      });
    } catch (error) {
      if (run === this.#run) {
        this.#fail(errorText(error));
      }
      return;
    }
    if (run !== this.#run) {
      session.cancel().catch(noop); // it ended (or was cancelled) while the shell was starting it
      return;
    }
    this.#session = session;
    for (const chunk of this.#queued.splice(0)) {
      session.push(chunk);
    }
  }

  // ----- stopping ------------------------------------------------------------------------------------

  /**
   * Ends the dictation the gentle way: the audio still held goes to the engine, the microphone is let go, the engine
   * finishes what it heard and its last phrases are put into the page. Resolves when the session has ended and the bar is down.
   */
  stop(): Promise<void> {
    const { phase } = this.state.get();
    switch (phase) {
      case "idle":
      case "error":
        return Promise.resolve();
      case "stopping":
        return this.#stopped ?? Promise.resolve();
      case "setup":
      case "starting":
        // Nothing has been heard yet (or the session is not registered): there is nothing to finish.
        this.cancel();
        return Promise.resolve();
      default:
        break;
    }
    const session = this.#session;
    if (session === null) {
      this.cancel();
      return Promise.resolve();
    }
    const run = this.#run;
    this.#stopClock();
    this.state.update((state) => ({ ...state, phase: "stopping", level: 0 }));
    const rest = this.#chunker.flush();
    if (rest !== null) {
      session.push(rest);
    }
    const capture = this.#capture;
    this.#capture = null;
    capture?.stop().catch(noop); // the microphone is let go now: no more audio is wanted
    const stopped = new Promise<void>((resolve) => {
      this.#resolveStopped = resolve;
    });
    this.#stopped = stopped;
    this.#stopTimer = setTimeout(() => {
      // The shell always ends a session; if its `ended` never comes, the bar must not stay up for good.
      if (run === this.#run) {
        session.cancel().catch(noop);
        this.#ended(run);
      }
    }, this.#deps.stopWaitMs ?? STOP_WAIT_MS);
    session.stop().catch((error: unknown) => {
      if (run === this.#run) {
        this.#ended(run, errorText(error));
      }
    });
    return stopped;
  }

  /** Ends everything at once, without the last phrases (the editor is destroyed, or the person gave up before it began). */
  cancel(): void {
    const session = this.#session;
    this.#teardown();
    session?.cancel().catch(noop);
    this.state.set(IDLE_DICTATION);
  }

  /**
   * The person left the page (another page or tab came to the front, or the app locked): the editor goes away with the
   * page, so a phrase still being recognised has no document to go into. Ends everything at once, like {@link cancel}, and
   * answers whether speech may have been lost: the microphone was open, so the person may have said something the engine had
   * not yet handed back. (The app says so, `EditorDictation.tsx`; nothing is written into another page.)
   */
  leave(): boolean {
    const { phase } = this.state.get();
    const lost = phase === "loading" || phase === "listening" || phase === "recognising" || phase === "stopping";
    this.cancel();
    return lost;
  }

  /** Closes the bar's notice. */
  dismiss(): void {
    this.state.update((state) => (state.phase === "error" || state.phase === "idle" ? IDLE_DICTATION : { ...state, notice: null }));
  }

  /**
   * Follows the editor while it is open: Esc stops, and so does a page that turns read-only. The function it answers ends
   * all of it (and cancels a dictation that runs: the editor is going away).
   */
  attach(): () => void {
    const { editor } = this.#deps;
    // A lock of the page is a transaction (`page-lock.ts`); `setEditable` says it with an `update`.
    const onChange = (): void => {
      if (this.#active && !editor.isEditable) {
        void this.stop();
      }
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape" && this.#active && !closesSomethingElse(event.target, editor)) {
        void this.stop();
      }
    };
    editor.on("transaction", onChange);
    editor.on("update", onChange);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      editor.off("transaction", onChange);
      editor.off("update", onChange);
      window.removeEventListener("keydown", onKeyDown);
      this.cancel();
    };
  }

  // ----- what the microphone and the engine say -----------------------------------------------------

  #onPcm(run: number, samples: Float32Array): void {
    if (run !== this.#run) {
      return;
    }
    for (const chunk of this.#chunker.push(samples)) {
      const session = this.#session;
      if (session !== null) {
        session.push(chunk);
      } else if (this.#queued.length < MAX_QUEUED_PUSHES) {
        this.#queued.push(chunk);
      }
    }
  }

  #onEvent(run: number, event: DictationEvent): void {
    if (run !== this.#run) {
      return;
    }
    // Speech heard is activity, as typing is: a long dictation must not run into the auto-lock. Only speech counts, not
    // the open microphone: a session nobody talks to for the auto-lock delay still locks the app (and so stops).
    if (event.type === "phrase" || (event.type === "state" && event.state === "recognising")) {
      reportActivity();
    }
    switch (event.type) {
      case "state": {
        // Once Stop was pressed the bar says so until the end, whatever the engine is doing.
        if (this.state.get().phase === "stopping") {
          return;
        }
        const phase = event.state === "loading" ? "loading" : event.state === "listening" ? "listening" : "recognising";
        this.state.update((state) => (state.phase === "starting" ? state : { ...state, phase }));
        break;
      }
      case "phrase":
        insertPhrase(this.#deps.editor, event.text);
        break;
      case "error":
        this.#errorCode = event.code;
        this.state.update((state) => ({ ...state, notice: codeText(event.code) }));
        break;
      case "ended":
        this.#ended(run);
        break;
    }
  }

  /** The session is over: the microphone and the bar come down, with the sentence of its error when it had one. */
  #ended(run: number, notice?: string): void {
    if (run !== this.#run) {
      return;
    }
    const code = this.#errorCode;
    this.#teardown(); // which also lets `stop()` resolve
    const sentence = notice ?? (code === null ? null : codeText(code));
    this.state.set(sentence === null ? IDLE_DICTATION : { ...IDLE_DICTATION, phase: "error", notice: sentence });
  }

  /** It could not start (or went wrong before a session existed): everything comes down and the bar says why. */
  #fail(notice: string): void {
    const session = this.#session;
    this.#teardown();
    session?.cancel().catch(noop);
    this.state.set({ ...IDLE_DICTATION, phase: "error", notice });
  }

  /** Lets the microphone and the clocks go and forgets the session; what the old run still delivers is dropped. */
  #teardown(): void {
    this.#run += 1;
    this.#capture?.cancel();
    this.#capture = null;
    this.#session = null;
    this.#chunker.clear();
    this.#queued = [];
    this.#errorCode = null;
    this.#stopClock();
    this.#clearHint();
    if (this.#stopTimer !== null) {
      clearTimeout(this.#stopTimer);
      this.#stopTimer = null;
    }
    const resolve = this.#resolveStopped;
    this.#resolveStopped = null;
    this.#stopped = null;
    resolve?.();
  }

  // ----- the clock ---------------------------------------------------------------------------------

  #stopClock(): void {
    if (this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
    this.#since = null;
  }

  #clearHint(): void {
    if (this.#hintTimer !== null) {
      clearTimeout(this.#hintTimer);
      this.#hintTimer = null;
    }
  }

  #tick(run: number): void {
    if (run !== this.#run || this.#since === null) {
      return;
    }
    const seconds = Math.floor((Date.now() - this.#since) / 1000);
    this.state.update((state) => (state.seconds === seconds ? state : { ...state, seconds }));
  }
}
