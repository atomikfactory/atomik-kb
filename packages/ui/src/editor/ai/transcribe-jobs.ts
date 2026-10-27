// "Transcribe" on an existing audio or video block (ADR-0037, `docs/architecture/ai.md` section 8.6): the
// file is read in ranges (at most 512 MiB), decoded by the webview at 16 kHz mono, cut into segments of
// about 30 seconds and transcribed in order, with "Transcribing 3 of 42" in the block and a Cancel. The
// transcript goes into the page as a toggle right under the block, in one transaction, then the meeting
// summary under the toggle in a second one. One job per block; the jobs end with the page.

import { MAX_PREVIEW_BYTES, toCoreError } from "@atomik-kb/core-client";
import type { ByteRange } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";

import { t } from "../../i18n";
import type { AiEditorClient } from "./ai-client";
import type { AiCapabilities } from "./availability";
import { AudioDecodeError, decodeToMono16k } from "./audio-decode";
import { GenerationCancelled } from "./generate";
import { hasSpeech, insertSummary, insertTranscriptAfter } from "./insert-transcript";
import { Observable } from "./observable";
import { summariseTranscript } from "./summary";
import type { TranscriptSegment } from "./text";
import { cutSegments, TranscriptionQueue } from "./transcription";

/** The largest file transcribed: 512 MiB (`docs/architecture/ai.md` section 10). */
export const MAX_TRANSCRIBE_BYTES = 512 * 1024 * 1024;
/** How much of the file is asked for at once. */
const READ_STEP = Math.min(16 * 1024 * 1024, MAX_PREVIEW_BYTES);

export type JobStep = "reading" | "decoding" | "transcribing" | "summarising";

/** What a block shows about its job. */
export type JobView =
  | { status: "running"; step: JobStep; done: number; total: number }
  | { status: "error"; message: string };

/** What a file's transcription needs from the app. */
export interface FileJobDeps {
  client: Pick<AiEditorClient, "aiTranscribe">;
  model: string | undefined;
  /** The bytes of a range of the file. */
  readRange: (range: ByteRange) => Promise<Uint8Array>;
  /** The samples of the file's audio at 16 kHz mono (`decodeToMono16k` by default). */
  decode?: (bytes: Uint8Array) => Promise<Float32Array>;
  size: number;
  signal: AbortSignal;
  onProgress: (view: { step: JobStep; done: number; total: number }) => void;
}

/** The file's bytes, read in ranges. */
export async function readFileBytes(
  readRange: (range: ByteRange) => Promise<Uint8Array>,
  size: number,
  signal: AbortSignal,
  onProgress: (done: number, total: number) => void,
): Promise<Uint8Array> {
  const bytes = new Uint8Array(size);
  for (let start = 0; start < size; start += READ_STEP) {
    if (signal.aborted) {
      throw new GenerationCancelled();
    }
    const end = Math.min(size, start + READ_STEP);
    const part = await readRange({ start, end });
    bytes.set(part.subarray(0, end - start), start);
    onProgress(end, size);
  }
  return bytes;
}

/** The result of a file's transcription: what was heard, and how many segments could not be transcribed. */
export interface FileTranscript {
  segments: TranscriptSegment[];
  failed: number;
  error: string | null;
}

/** Reads, decodes, cuts and transcribes a file. Rejects with `GenerationCancelled` when `signal` aborts. */
export async function transcribeFile(deps: FileJobDeps): Promise<FileTranscript> {
  const bytes = await readFileBytes(deps.readRange, deps.size, deps.signal, (done, total) => {
    deps.onProgress({ step: "reading", done, total });
  });
  deps.onProgress({ step: "decoding", done: 0, total: 1 });
  const samples = await (deps.decode ?? decodeToMono16k)(bytes);
  if (deps.signal.aborted) {
    throw new GenerationCancelled();
  }
  const segments = cutSegments(samples);
  const total = segments.length;
  deps.onProgress({ step: "transcribing", done: 0, total });
  const queue = new TranscriptionQueue(deps.client, deps.model, (state) => {
    deps.onProgress({ step: "transcribing", done: state.done, total });
  });
  const abort = (): void => {
    queue.cancel();
  };
  deps.signal.addEventListener("abort", abort, { once: true });
  try {
    for (const segment of segments) {
      queue.enqueue(segment);
    }
    await queue.idle();
  } finally {
    deps.signal.removeEventListener("abort", abort);
  }
  if (deps.signal.aborted) {
    throw new GenerationCancelled();
  }
  return { segments: queue.segments, failed: queue.state.failed, error: queue.state.error };
}

/** What the jobs of an editor read from the app. */
export interface JobsDeps {
  editor: Editor;
  client: () => AiEditorClient | null;
  capabilities: () => AiCapabilities;
  readRange: (attachmentId: string, range: ByteRange) => Promise<Uint8Array>;
  /** Tests: another decoder. */
  decode?: (bytes: Uint8Array) => Promise<Float32Array>;
}

/** A block to transcribe. */
export interface JobTarget {
  blockId: string;
  attachmentId: string;
  size: number;
}

/** The transcription jobs of one page editor, by block id. */
export class TranscribeJobs {
  readonly views = new Observable<ReadonlyMap<string, JobView>>(new Map());
  readonly #deps: JobsDeps;
  readonly #aborts = new Map<string, AbortController>();

  constructor(deps: JobsDeps) {
    this.#deps = deps;
  }

  /** Ends with the page: every job is cancelled. */
  attach(): () => void {
    return () => {
      for (const abort of this.#aborts.values()) {
        abort.abort();
      }
      this.#aborts.clear();
      this.views.set(new Map());
    };
  }

  /** Whether a job runs for the block. */
  isRunning(blockId: string): boolean {
    return this.#aborts.has(blockId);
  }

  /** Transcribes a block's file and puts the transcript and the summary under it. */
  async start(target: JobTarget): Promise<void> {
    const client = this.#deps.client();
    const caps = this.#deps.capabilities();
    if (client === null || !caps.transcription || this.isRunning(target.blockId)) {
      return;
    }
    if (target.size > MAX_TRANSCRIBE_BYTES) {
      this.#view(target.blockId, { status: "error", message: t("transcribe.tooLarge") });
      return;
    }
    const abort = new AbortController();
    this.#aborts.set(target.blockId, abort);
    const progress = (view: { step: JobStep; done: number; total: number }): void => {
      if (this.#aborts.get(target.blockId) === abort) {
        this.#view(target.blockId, { status: "running", ...view });
      }
    };
    progress({ step: "reading", done: 0, total: target.size });
    let finalView: JobView | null = null;
    try {
      const transcript = await transcribeFile({
        client,
        model: caps.transcriptionModel ?? undefined,
        readRange: (range) => this.#deps.readRange(target.attachmentId, range),
        ...(this.#deps.decode === undefined ? {} : { decode: this.#deps.decode }),
        size: target.size,
        signal: abort.signal,
        onProgress: progress,
      });
      finalView = await this.#insert(target, transcript, client, caps, abort, progress);
    } catch (error) {
      if (!(error instanceof GenerationCancelled)) {
        finalView = {
          status: "error",
          message:
            error instanceof AudioDecodeError
              ? t("transcribe.decodeFailed", { message: error.message })
              : t("transcribe.failed", { message: toCoreError(error).message }),
        };
      }
    }
    if (this.#aborts.get(target.blockId) === abort) {
      this.#aborts.delete(target.blockId);
      this.#view(target.blockId, finalView);
    }
  }

  /** Stops the job of a block (nothing was written to the page unless it was already in the summary). */
  cancel(blockId: string): void {
    this.#aborts.get(blockId)?.abort();
    this.#aborts.delete(blockId);
    this.#view(blockId, null);
  }

  /** Clears the error a block shows. */
  dismiss(blockId: string): void {
    if (!this.isRunning(blockId)) {
      this.#view(blockId, null);
    }
  }

  async #insert(
    target: JobTarget,
    transcript: { segments: TranscriptSegment[]; failed: number; error: string | null },
    client: AiEditorClient,
    caps: AiCapabilities,
    abort: AbortController,
    progress: (view: { step: JobStep; done: number; total: number }) => void,
  ): Promise<JobView | null> {
    const { segments } = transcript;
    if (!hasSpeech(segments)) {
      return {
        status: "error",
        message:
          transcript.failed > 0
            ? t("transcribe.failed", { message: transcript.error ?? "" })
            : t("transcribe.noSpeech"),
      };
    }
    const inserted = insertTranscriptAfter(this.#deps.editor, target.blockId, segments);
    if (inserted === null) {
      return { status: "error", message: t("transcribe.pageGone") };
    }
    let message: string | null =
      transcript.failed > 0 ? t("transcribe.partial", { message: transcript.error ?? "" }) : null;
    if (caps.writing) {
      progress({ step: "summarising", done: 0, total: 1 });
      try {
        const summary = await summariseTranscript(client, segments, abort.signal, (parts) => {
          progress({ step: "summarising", done: parts.done, total: parts.total });
        });
        if (summary !== null && !abort.signal.aborted) {
          insertSummary(this.#deps.editor, inserted.toggleId, summary);
        }
      } catch (error) {
        if (!(error instanceof GenerationCancelled)) {
          message = t("transcribe.summaryFailed", { message: toCoreError(error).message });
        }
      }
    }
    return message === null ? null : { status: "error", message };
  }

  #view(blockId: string, view: JobView | null): void {
    this.views.update((views) => {
      const next = new Map(views);
      if (view === null) {
        next.delete(blockId);
      } else {
        next.set(blockId, view);
      }
      return next;
    });
  }
}
