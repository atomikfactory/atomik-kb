// The recorder (ADR-0037, `docs/architecture/ai.md` section 8.6): one per page editor. It records the
// microphone (capture.ts), transcribes the speech as it is heard when a transcription model is set (a
// queue of 30-second segments, transcription.ts), and when stopped saves the recording as an attachment
// named `Recording YYYY-MM-DD HH.mm.webm`, then inserts the audio block with the "Transcript" toggle in one
// transaction at the place it started, and the meeting summary under it in a second one. Cancel throws
// everything away before the page is touched; leaving the page does too.

import { toCoreError } from "@atomik-kb/core-client";
import type { AttachmentInfo, UploadProgress } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";

import { t } from "../../i18n";
import type { AiEditorClient } from "./ai-client";
import type { AiCapabilities } from "./availability";
import { browserCaptureEnv, CaptureError, createCapture } from "./capture";
import type { Capture, CaptureEnv } from "./capture";
import { GenerationCancelled } from "./generate";
import { insertRecording, insertSummary } from "./insert-transcript";
import { Observable } from "./observable";
import { summariseTranscript } from "./summary";
import { formatTimestamp, recordingName } from "./text";
import type { TranscriptSegment } from "./text";
import { StreamSegmenter, TranscriptionQueue } from "./transcription";
import type { QueueState } from "./transcription";

export type RecorderPhase = "idle" | "starting" | "recording" | "paused" | "finishing" | "error";
/** What the bar is waiting for once the recording is stopped. */
export type FinishStep = "transcribing" | "saving" | "summarising";

export interface RecorderState {
  phase: RecorderPhase;
  /** Seconds recorded (pauses do not count). */
  seconds: number;
  /** The loudness of the last block, 0 to 1. */
  level: number;
  /** The last two lines of the transcript so far. */
  lines: string[];
  /** The speech is being transcribed (a transcription model is set). */
  transcribing: boolean;
  step: FinishStep | null;
  /** Segments answered of segments given, while `step` is `transcribing`; parts done of parts, while `summarising`. */
  progress: { done: number; total: number } | null;
  /** A message the person should read (a denied microphone, a failure). */
  notice: string | null;
  /** The saving failed: Retry tries it again. */
  canRetry: boolean;
  /** The microphone has not answered for a few seconds: the person may have to answer a request. */
  permissionHint: boolean;
}

/** How long the bar waits for the microphone before it says a request may be open (WebView2 asks in its own window). */
export const START_HINT_MS = 3000;

export const IDLE_RECORDER: RecorderState = {
  phase: "idle",
  seconds: 0,
  level: 0,
  lines: [],
  transcribing: false,
  step: null,
  progress: null,
  notice: null,
  canRetry: false,
  permissionHint: false,
};

export interface RecorderDeps {
  editor: Editor;
  client: () => AiEditorClient | null;
  capabilities: () => AiCapabilities;
  /** Saves a file as an attachment of the page's space; `null` when the shell cannot. */
  upload: () => ((file: File, onProgress: UploadProgress) => Promise<AttachmentInfo>) | null;
  /** The browser objects (the page's by default); tests give a fake microphone. */
  env?: () => CaptureEnv | null;
  now?: () => Date;
  /** Tests: how long to wait before the permission hint shows (default {@link START_HINT_MS}). */
  startHintMs?: number;
  /** The microphone, for a test that does not use the fake environment. */
  capture?: (events: { onPcm: (samples: Float32Array) => void; onLevel: (level: number) => void }) => Capture;
  /** The recorder does not start while this is true (dictation holds the microphone, ADR-0053). */
  blocked?: () => boolean;
}

/** The id of the top-level block the selection is in (the recording goes after it), `null` for a page without one. */
function topBlockId(editor: Editor): string | null {
  const { $from } = editor.state.selection;
  const node = $from.depth >= 1 ? $from.node(1) : ($from.nodeBefore ?? $from.nodeAfter);
  const id: unknown = node?.attrs.id;
  return typeof id === "string" ? id : null;
}

/** The last two lines of a transcript, as `[mm:ss] text`. */
export function liveLines(segments: readonly TranscriptSegment[]): string[] {
  return segments
    .filter((segment) => segment.text.trim() !== "")
    .slice(-2)
    .map((segment) => `[${formatTimestamp(segment.startMs)}] ${segment.text.trim()}`);
}

export class RecorderController {
  readonly state = new Observable<RecorderState>(IDLE_RECORDER);
  readonly #deps: RecorderDeps;
  #capture: Capture | null = null;
  #segmenter = new StreamSegmenter();
  #queue: TranscriptionQueue | null = null;
  #anchorId: string | null = null;
  #file: File | null = null;
  #abort = new AbortController();
  /** Bumped by every start and cancel: what an older run still does after that is dropped. */
  #session = 0;
  #timer: ReturnType<typeof setInterval> | null = null;
  #hintTimer: ReturnType<typeof setTimeout> | null = null;
  #elapsedMs = 0;
  #since: number | null = null;

  constructor(deps: RecorderDeps) {
    this.#deps = deps;
  }

  /** Whether the page's browser can record at all (a microphone API and a recorder). */
  static supported(env: () => CaptureEnv | null = browserCaptureEnv): boolean {
    return env() !== null;
  }

  async start(): Promise<void> {
    const { phase } = this.state.get();
    if (phase !== "idle" && phase !== "error") {
      return;
    }
    const capabilities = this.#deps.capabilities();
    if (!capabilities.enabled || this.#deps.editor.isDestroyed || !this.#deps.editor.isEditable || this.#deps.blocked?.() === true) {
      return;
    }
    const env = (this.#deps.env ?? browserCaptureEnv)();
    if (env === null && this.#deps.capture === undefined) {
      this.state.set({ ...IDLE_RECORDER, phase: "error", notice: t("recorder.unsupported") });
      return;
    }
    const session = (this.#session += 1);
    this.#abort = new AbortController();
    this.#anchorId = topBlockId(this.#deps.editor);
    this.#segmenter = new StreamSegmenter();
    this.#file = null;
    this.#elapsedMs = 0;
    this.#since = null;
    const client = this.#deps.client();
    const transcribing = client !== null && capabilities.transcription;
    this.#queue = transcribing
      ? new TranscriptionQueue(client, capabilities.transcriptionModel ?? undefined, (queue) => {
          this.#onQueue(session, queue);
        })
      : null;
    this.state.set({ ...IDLE_RECORDER, phase: "starting", transcribing });
    this.#hintTimer = setTimeout(() => {
      this.#hintTimer = null;
      if (session === this.#session) {
        this.state.update((state) => (state.phase === "starting" ? { ...state, permissionHint: true } : state));
      }
    }, this.#deps.startHintMs ?? START_HINT_MS);
    const events = {
      onPcm: (samples: Float32Array) => {
        this.#onPcm(session, samples);
      },
      onLevel: (level: number) => {
        if (session === this.#session) {
          this.state.update((state) => (state.phase === "recording" ? { ...state, level } : state));
        }
      },
    };
    const capture = this.#deps.capture?.(events) ?? (env === null ? null : createCapture(events, env));
    if (capture === null) {
      return;
    }
    this.#capture = capture;
    try {
      await capture.start();
    } catch (error) {
      this.#clearHint();
      if (session === this.#session) {
        this.#capture = null;
        this.state.set({ ...IDLE_RECORDER, phase: "error", notice: this.#describeCaptureError(error) });
      }
      return;
    }
    if (session !== this.#session) {
      capture.cancel(); // cancelled while the microphone was being asked for
      return;
    }
    this.#clearHint();
    this.#since = Date.now();
    this.#timer = setInterval(() => {
      this.#tick(session);
    }, 250);
    this.state.update((state) => ({ ...state, phase: "recording", permissionHint: false }));
  }

  pause(): void {
    if (this.state.get().phase !== "recording" || this.#capture === null) {
      return;
    }
    this.#capture.pause();
    this.#freezeClock();
    this.state.update((state) => ({ ...state, phase: "paused", level: 0 }));
  }

  resume(): void {
    if (this.state.get().phase !== "paused" || this.#capture === null) {
      return;
    }
    this.#capture.resume();
    this.#since = Date.now();
    this.state.update((state) => ({ ...state, phase: "recording" }));
  }

  /** Ends the recording and puts it in the page: save, transcript, summary. */
  async stop(): Promise<void> {
    const { phase } = this.state.get();
    const capture = this.#capture;
    if ((phase !== "recording" && phase !== "paused") || capture === null) {
      return;
    }
    const session = this.#session;
    this.#freezeClock();
    this.#stopTimer();
    this.state.update((state) => ({ ...state, phase: "finishing", step: "saving", level: 0, notice: null }));
    let blob: Blob;
    try {
      blob = await capture.stop();
    } catch (error) {
      if (session === this.#session) {
        this.#capture = null;
        this.state.set({ ...IDLE_RECORDER, phase: "error", notice: this.#describeCaptureError(error) });
      }
      return;
    }
    this.#capture = null;
    if (session !== this.#session) {
      return;
    }
    for (const segment of this.#segmenter.finish()) {
      this.#queue?.enqueue(segment);
    }
    this.#file = new File([blob], recordingName((this.#deps.now ?? (() => new Date()))()), { type: "audio/webm" });
    await this.#finish(session);
  }

  /** Tries the saving again after it failed. */
  async retry(): Promise<void> {
    if (this.state.get().canRetry && this.#file !== null) {
      await this.#finish(this.#session);
    }
  }

  /** Throws the recording away (before the page is touched), or stops the summary that is being written. */
  cancel(): void {
    this.#session += 1;
    this.#abort.abort();
    this.#queue?.cancel();
    this.#capture?.cancel();
    this.#capture = null;
    this.#queue = null;
    this.#file = null;
    this.#stopTimer();
    this.#clearHint();
    this.state.set(IDLE_RECORDER);
  }

  /** Closes the bar's notice. */
  dismiss(): void {
    this.state.update((state) =>
      state.phase === "recording" || state.phase === "paused" || state.phase === "finishing"
        ? { ...state, notice: null }
        : { ...IDLE_RECORDER },
    );
  }

  /** Ends with the page: nothing of an unfinished recording is kept. */
  attach(): () => void {
    return () => {
      this.cancel();
    };
  }

  // ----- what happens after Stop ---------------------------------------------------------

  async #finish(session: number): Promise<void> {
    const file = this.#file;
    const queue = this.#queue;
    const upload = this.#deps.upload();
    if (file === null) {
      return;
    }
    if (upload === null) {
      this.state.set({ ...IDLE_RECORDER, phase: "error", notice: t("recorder.cannotSave"), canRetry: false });
      return;
    }
    this.state.update((state) => ({ ...state, phase: "finishing", notice: null, canRetry: false }));
    this.#showStep(queue?.state ?? null);
    let info: AttachmentInfo;
    try {
      [info] = await Promise.all([upload(file, () => undefined), queue?.idle()]);
    } catch (error) {
      if (session === this.#session) {
        this.state.update((state) => ({
          ...state,
          phase: "error",
          step: null,
          progress: null,
          canRetry: true,
          notice: t("recorder.saveFailed", { message: toCoreError(error).message }),
        }));
      }
      return;
    }
    if (session !== this.#session) {
      return;
    }
    const segments = queue?.segments ?? [];
    const inserted = insertRecording(this.#deps.editor, { anchorId: this.#anchorId, attachmentId: info.attachmentId, segments });
    const warning = this.#transcriptNotice(queue?.state ?? null, segments);
    this.#file = null;
    if (inserted === null) {
      this.state.set({ ...IDLE_RECORDER, notice: t("recorder.pageGone") });
      return;
    }
    const caps = this.#deps.capabilities();
    const client = this.#deps.client();
    if (inserted.toggleId === null || client === null || !caps.writing) {
      this.state.set({ ...IDLE_RECORDER, notice: warning });
      return;
    }
    const toggleId = inserted.toggleId;
    this.state.update((state) => ({ ...state, step: "summarising", progress: null }));
    let notice = warning;
    try {
      const summary = await summariseTranscript(client, segments, this.#abort.signal, (progress) => {
        if (session === this.#session) {
          this.state.update((state) => ({ ...state, progress }));
        }
      });
      if (session === this.#session && summary !== null) {
        insertSummary(this.#deps.editor, toggleId, summary);
      }
    } catch (error) {
      if (!(error instanceof GenerationCancelled)) {
        notice = t("recorder.summaryFailed", { message: toCoreError(error).message });
      }
    }
    if (session === this.#session) {
      this.state.set({ ...IDLE_RECORDER, notice });
    }
  }

  /** What to tell about a transcript that is missing parts, `null` when it is as complete as it could be. */
  #transcriptNotice(queue: QueueState | null, segments: readonly TranscriptSegment[]): string | null {
    if (queue === null || queue.failed === 0) {
      return null;
    }
    const message = queue.error ?? "";
    return segments.some((segment) => segment.text.trim() !== "")
      ? t("recorder.transcriptPartial", { message })
      : t("recorder.transcriptFailed", { message });
  }

  // ----- the live side ---------------------------------------------------------------------

  #onPcm(session: number, samples: Float32Array): void {
    if (session !== this.#session || this.#queue === null) {
      return;
    }
    for (const segment of this.#segmenter.push(samples)) {
      this.#queue.enqueue(segment);
    }
  }

  #onQueue(session: number, queue: QueueState): void {
    if (session !== this.#session) {
      return;
    }
    const lines = liveLines(this.#queue?.segments ?? []);
    this.state.update((state) => {
      if (state.phase === "finishing") {
        if (state.step === "summarising") {
          return { ...state, lines };
        }
        const pending = queue.done < queue.total;
        return {
          ...state,
          lines,
          step: pending ? "transcribing" : "saving",
          progress: pending ? { done: queue.done, total: queue.total } : null,
        };
      }
      if (queue.failed > 0 && state.notice === null) {
        return { ...state, lines, notice: t("recorder.transcriptLive", { message: queue.error ?? "" }) };
      }
      return { ...state, lines };
    });
  }

  #showStep(queue: QueueState | null): void {
    this.state.update((state) =>
      queue !== null && queue.done < queue.total
        ? { ...state, step: "transcribing", progress: { done: queue.done, total: queue.total } }
        : { ...state, step: "saving", progress: null },
    );
  }

  #describeCaptureError(error: unknown): string {
    if (error instanceof CaptureError) {
      switch (error.reason) {
        case "denied":
          return t("recorder.denied");
        case "noDevice":
          return t("recorder.noDevice");
        case "failed":
          return t("recorder.failed", { message: error.message });
      }
    }
    return t("recorder.failed", { message: toCoreError(error).message });
  }

  // ----- the clock -------------------------------------------------------------------------

  #freezeClock(): void {
    if (this.#since !== null) {
      this.#elapsedMs += Date.now() - this.#since;
      this.#since = null;
    }
  }

  #clearHint(): void {
    if (this.#hintTimer !== null) {
      clearTimeout(this.#hintTimer);
      this.#hintTimer = null;
    }
  }

  #stopTimer(): void {
    if (this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
  }

  #tick(session: number): void {
    if (session !== this.#session || this.#since === null) {
      return;
    }
    const seconds = Math.floor((this.#elapsedMs + (Date.now() - this.#since)) / 1000);
    this.state.update((state) => (state.seconds === seconds ? state : { ...state, seconds }));
  }
}
