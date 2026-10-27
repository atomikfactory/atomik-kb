// From samples to transcript (ADR-0037, `docs/architecture/ai.md` section 8.6): 16 kHz mono samples are
// cut into segments of about 30 seconds (`planSegments` of core-client, at the quietest moment between
// 25 and 35 seconds), each segment is sent to `ai_transcribe` as a WAV file, one at a time and in order,
// and the answers make the transcript. The same code serves the recorder (the segments come as the
// microphone is heard: `StreamSegmenter`) and "Transcribe" on an existing file (all at once: `cutSegments`).

import {
  encodeWav,
  floatToInt16,
  planSegments,
  SEGMENT_MAX_SECONDS,
  toCoreError,
  TRANSCRIPTION_RATE,
} from "@atomik-kb/core-client";
import type { SegmentPlan } from "@atomik-kb/core-client";

import type { AiEditorClient } from "./ai-client";
import type { TranscriptSegment } from "./text";

/** A piece of audio to transcribe, with where it starts in the recording. */
export interface Segment {
  startMs: number;
  samples: Float32Array;
}

/** The longest a segment is, in samples. */
const MAX_SAMPLES = SEGMENT_MAX_SECONDS * TRANSCRIPTION_RATE;
/**
 * How much audio must wait before the first segment is cut: with 36 seconds after the start of a segment,
 * `planSegments` has seen the whole window its cut is chosen in, so the cut is final.
 */
const CUT_WHEN_SAMPLES = (SEGMENT_MAX_SECONDS + 1) * TRANSCRIPTION_RATE;
/** What is cut when `planSegments` gives nothing usable: windows of this length. */
const FALLBACK_SAMPLES = 30 * TRANSCRIPTION_RATE;
/** A segment shorter than this (half a second) holds no words worth asking about. */
const MIN_SAMPLES = TRANSCRIPTION_RATE / 2;

const toMs = (samples: number): number => Math.round((samples * 1000) / TRANSCRIPTION_RATE);

/** Fixed windows of 30 seconds, the last one joined to the one before when it is shorter than a second. */
function fixedPlans(length: number): SegmentPlan[] {
  const plans: SegmentPlan[] = [];
  for (let start = 0; start < length; start += FALLBACK_SAMPLES) {
    plans.push({ start, end: Math.min(length, start + FALLBACK_SAMPLES) });
  }
  const last = plans[plans.length - 1];
  const before = plans[plans.length - 2];
  if (last !== undefined && before !== undefined && last.end - last.start < TRANSCRIPTION_RATE) {
    before.end = last.end;
    plans.pop();
  }
  return plans;
}

/** Whether `plans` cut `length` samples into consecutive pieces that cover them. */
function usable(plans: readonly SegmentPlan[], length: number): boolean {
  let at = 0;
  for (const plan of plans) {
    if (plan.start !== at || plan.end <= plan.start || plan.end > length) {
      return false;
    }
    at = plan.end;
  }
  return at === length && plans.length > 0;
}

/** Where to cut `samples`: `planSegments`, or windows of 30 seconds where it gives nothing sound. */
export function cutPlans(samples: Float32Array): SegmentPlan[] {
  if (samples.length === 0) {
    return [];
  }
  const plans = planSegments(samples, TRANSCRIPTION_RATE);
  return usable(plans, samples.length) ? plans : fixedPlans(samples.length);
}

/** The segments of a whole recording (a file that was decoded), the silent or tiny ones left out. */
export function cutSegments(samples: Float32Array): Segment[] {
  return cutPlans(samples)
    .filter((plan) => plan.end - plan.start >= MIN_SAMPLES)
    .map((plan) => ({ startMs: toMs(plan.start), samples: samples.subarray(plan.start, plan.end) }));
}

/**
 * Cuts the samples of a recording as they arrive: whenever 36 seconds are waiting, the first segment is
 * cut where `planSegments` says (between 25 and 35 seconds in) and handed out; `finish` cuts the rest.
 */
export class StreamSegmenter {
  #chunks: Float32Array[] = [];
  #length = 0;
  /** Samples handed out so far (where the waiting ones start in the recording). */
  #emitted = 0;

  push(samples: Float32Array): Segment[] {
    if (samples.length === 0) {
      return [];
    }
    this.#chunks.push(samples);
    this.#length += samples.length;
    const out: Segment[] = [];
    while (this.#length >= CUT_WHEN_SAMPLES) {
      const all = this.#take();
      const plans = planSegments(all, TRANSCRIPTION_RATE);
      const first = plans[0];
      const cut =
        first !== undefined && first.start === 0 && first.end > 0 && first.end <= all.length && first.end <= MAX_SAMPLES
          ? first.end
          : FALLBACK_SAMPLES;
      out.push({ startMs: toMs(this.#emitted), samples: all.slice(0, cut) });
      this.#emitted += cut;
      const rest = all.slice(cut);
      this.#chunks = rest.length === 0 ? [] : [rest];
      this.#length = rest.length;
    }
    return out;
  }

  /** The segments of what is left. */
  finish(): Segment[] {
    const all = this.#take();
    this.#chunks = [];
    this.#length = 0;
    const base = this.#emitted;
    this.#emitted += all.length;
    return cutPlans(all)
      .filter((plan) => plan.end - plan.start >= MIN_SAMPLES)
      .map((plan) => ({ startMs: toMs(base + plan.start), samples: all.slice(plan.start, plan.end) }));
  }

  #take(): Float32Array {
    const all = new Float32Array(this.#length);
    let offset = 0;
    for (const chunk of this.#chunks) {
      all.set(chunk, offset);
      offset += chunk.length;
    }
    return all;
  }
}

/** Where a queue is: how many segments it was given and how many are answered. */
export interface QueueState {
  total: number;
  done: number;
  /** Segments whose transcription failed (they are missing from the transcript). */
  failed: number;
  /** The last failure's message. */
  error: string | null;
  /** Too many failures in a row: the rest is not tried. */
  halted: boolean;
}

/** After this many failures in a row (the model is gone, the shell cannot reach it) nothing more is tried. */
const MAX_CONSECUTIVE_FAILURES = 3;

/**
 * Sends segments to `ai_transcribe` one at a time, in the order they were given, and keeps the answers in
 * that order. A failed segment is left out of the transcript and counted; three in a row stop the queue.
 */
export class TranscriptionQueue {
  readonly #client: Pick<AiEditorClient, "aiTranscribe">;
  readonly #model: string | undefined;
  readonly #onChange: (state: QueueState) => void;
  #tail: Promise<void> = Promise.resolve();
  #results: TranscriptSegment[] = [];
  #state: QueueState = { total: 0, done: 0, failed: 0, error: null, halted: false };
  #consecutive = 0;
  #cancelled = false;

  constructor(client: Pick<AiEditorClient, "aiTranscribe">, model: string | undefined, onChange: (state: QueueState) => void) {
    this.#client = client;
    this.#model = model;
    this.#onChange = onChange;
  }

  get state(): QueueState {
    return this.#state;
  }

  /** The transcript so far: the segments that were answered, in order, silence included (empty text). */
  get segments(): TranscriptSegment[] {
    return [...this.#results];
  }

  enqueue(segment: Segment): void {
    if (this.#cancelled) {
      return;
    }
    this.#set({ ...this.#state, total: this.#state.total + 1 });
    this.#tail = this.#tail.then(() => this.#run(segment));
  }

  /** Resolves when every segment given so far is answered (or the queue stopped). */
  idle(): Promise<void> {
    return this.#tail;
  }

  /** Drops what has not been sent; the answer of the segment in flight is ignored. */
  cancel(): void {
    this.#cancelled = true;
  }

  async #run(segment: Segment): Promise<void> {
    if (this.#cancelled) {
      return;
    }
    if (this.#state.halted) {
      this.#set({ ...this.#state, done: this.#state.done + 1 }); // not tried: the progress still reaches its end
      return;
    }
    try {
      const wav = encodeWav(floatToInt16(segment.samples), TRANSCRIPTION_RATE);
      const answer = await this.#client.aiTranscribe(wav, this.#model);
      if (this.#cancelled) {
        return;
      }
      this.#results.push({ startMs: segment.startMs, text: answer.text });
      this.#consecutive = 0;
      this.#set({ ...this.#state, done: this.#state.done + 1 });
    } catch (error) {
      if (this.#cancelled) {
        return;
      }
      this.#consecutive += 1;
      this.#set({
        ...this.#state,
        done: this.#state.done + 1,
        failed: this.#state.failed + 1,
        error: toCoreError(error).message,
        halted: this.#consecutive >= MAX_CONSECUTIVE_FAILURES,
      });
    }
  }

  #set(state: QueueState): void {
    this.#state = state;
    this.#onChange(state);
  }
}
