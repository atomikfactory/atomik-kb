// From samples to transcript: segments cut as audio arrives and all at once, and the queue that sends
// them to `ai_transcribe` in order and survives a failing segment.

import { CoreError, TRANSCRIPTION_RATE } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { cutPlans, cutSegments, StreamSegmenter, TranscriptionQueue } from "./transcription";
import type { QueueState, Segment } from "./transcription";

const RATE = TRANSCRIPTION_RATE;

/** Samples of `seconds` seconds: a tone, with a silent gap `gapAt` seconds in (where the cut should go). */
function speech(seconds: number, gapAt?: number): Float32Array {
  const samples = new Float32Array(Math.round(seconds * RATE));
  for (let i = 0; i < samples.length; i += 1) {
    const at = i / RATE;
    const quiet = gapAt !== undefined && at >= gapAt - 0.15 && at <= gapAt + 0.15;
    samples[i] = quiet ? 0 : 0.2 * Math.sin((2 * Math.PI * 220 * i) / RATE);
  }
  return samples;
}

describe("cutSegments", () => {
  it("cuts a long recording at the quiet moments, in order, covering everything", () => {
    const samples = speech(80, 31);
    const plans = cutPlans(samples);
    expect(plans[0]?.end).toBeGreaterThan(30.5 * RATE);
    expect(plans[0]?.end).toBeLessThan(31.5 * RATE);
    expect(plans.at(-1)?.end).toBe(samples.length);
    for (const [index, plan] of plans.entries()) {
      expect(plan.end - plan.start).toBeLessThanOrEqual(35 * RATE);
      if (index > 0) {
        expect(plan.start).toBe(plans[index - 1]?.end);
      }
    }
    const segments = cutSegments(samples);
    expect(segments.map((segment) => segment.startMs)).toEqual(plans.map((plan) => Math.round((plan.start * 1000) / RATE)));
    expect(segments[0]?.startMs).toBe(0);
  });

  it("gives one segment for a short recording and none for silence of no length", () => {
    expect(cutSegments(speech(12))).toHaveLength(1);
    expect(cutSegments(new Float32Array(0))).toEqual([]);
  });

  it("leaves out a segment too short to hold words", () => {
    expect(cutSegments(speech(0.3))).toEqual([]);
    expect(cutSegments(speech(0.6))).toHaveLength(1);
  });
});

describe("StreamSegmenter", () => {
  /** Feeds `total` seconds in blocks of 40 ms and returns every segment handed out, then the rest. */
  function feed(total: number, gapAt?: number): { live: Segment[]; rest: Segment[]; all: Float32Array } {
    const all = speech(total, gapAt);
    const segmenter = new StreamSegmenter();
    const live: Segment[] = [];
    const block = Math.round(0.04 * RATE);
    for (let at = 0; at < all.length; at += block) {
      live.push(...segmenter.push(all.slice(at, Math.min(all.length, at + block))));
    }
    return { live, rest: segmenter.finish(), all };
  }

  it("hands out nothing for a short recording, until finish", () => {
    const { live, rest } = feed(20);
    expect(live).toEqual([]);
    expect(rest).toHaveLength(1);
    expect(rest[0]?.startMs).toBe(0);
    expect(rest[0]?.samples).toHaveLength(20 * RATE);
  });

  it("cuts a segment at the quiet moment once enough audio waits, and keeps the starts exact", () => {
    const { live, rest, all } = feed(100, 30);
    const segments = [...live, ...rest];
    expect(live.length).toBeGreaterThanOrEqual(2);
    expect(live[0]?.startMs).toBe(0);
    // The first cut falls in the quiet gap around 30 s.
    expect(live[0]?.samples.length).toBeGreaterThan(29.5 * RATE);
    expect(live[0]?.samples.length).toBeLessThan(30.5 * RATE);
    // Every segment starts where the one before ended, and together they are the whole recording.
    let at = 0;
    for (const segment of segments) {
      expect(segment.startMs).toBe(Math.round((at * 1000) / RATE));
      at += segment.samples.length;
    }
    expect(at).toBe(all.length);
    for (const segment of segments) {
      expect(segment.samples.length).toBeLessThanOrEqual(35 * RATE);
    }
  });
});

/** A queue over a fake `aiTranscribe`. */
function queueOver(transcribe: (wav: Uint8Array, model: string | undefined) => Promise<{ text: string }>) {
  const states: QueueState[] = [];
  const queue = new TranscriptionQueue({ aiTranscribe: transcribe }, "r2t2", (state) => {
    states.push(state);
  });
  return { queue, states };
}

const segment = (startMs: number): Segment => ({ startMs, samples: new Float32Array(RATE) });

describe("TranscriptionQueue", () => {
  it("transcribes one segment at a time, in order, as WAV files with the model", async () => {
    let running = 0;
    let most = 0;
    const seen: { bytes: number; model: string | undefined }[] = [];
    const { queue, states } = queueOver(async (wav, model) => {
      running += 1;
      most = Math.max(most, running);
      seen.push({ bytes: wav.length, model });
      await new Promise((resolve) => setTimeout(resolve, 5 - seen.length));
      running -= 1;
      return { text: `text ${String(seen.length)}` };
    });
    queue.enqueue(segment(0));
    queue.enqueue(segment(30_000));
    queue.enqueue(segment(60_000));
    await queue.idle();

    expect(most).toBe(1);
    expect(seen).toEqual(Array.from({ length: 3 }, () => ({ bytes: 44 + RATE * 2, model: "r2t2" })));
    expect(queue.segments).toEqual([
      { startMs: 0, text: "text 1" },
      { startMs: 30_000, text: "text 2" },
      { startMs: 60_000, text: "text 3" },
    ]);
    expect(states.at(-1)).toMatchObject({ total: 3, done: 3, failed: 0, halted: false });
  });

  it("leaves a failed segment out and goes on; three failures in a row stop it", async () => {
    let calls = 0;
    const { queue } = queueOver(() => {
      calls += 1;
      if (calls === 1) {
        return Promise.reject(new CoreError({ code: "InvalidInput", message: "model not installed" }));
      }
      return Promise.resolve({ text: "fine" });
    });
    queue.enqueue(segment(0));
    queue.enqueue(segment(30_000));
    await queue.idle();
    expect(queue.segments).toEqual([{ startMs: 30_000, text: "fine" }]);
    expect(queue.state).toMatchObject({ total: 2, done: 2, failed: 1, error: "model not installed", halted: false });

    const failing = queueOver(() => Promise.reject(new Error("down")));
    for (let index = 0; index < 5; index += 1) {
      failing.queue.enqueue(segment(index * 1000));
    }
    await failing.queue.idle();
    expect(failing.queue.state).toMatchObject({ total: 5, done: 5, failed: 3, halted: true });
    expect(failing.queue.segments).toEqual([]);
  });

  it("drops what was not sent when cancelled, and ignores the answer in flight", async () => {
    const transcribe = vi.fn(async () => {
      await new Promise((resolve) => setTimeout(resolve, 10));
      return { text: "late" };
    });
    const { queue } = queueOver(transcribe);
    queue.enqueue(segment(0));
    queue.enqueue(segment(1000));
    queue.cancel();
    await queue.idle();
    queue.enqueue(segment(2000));
    await queue.idle();
    expect(queue.segments).toEqual([]);
    expect(transcribe.mock.calls.length).toBeLessThanOrEqual(1);
  });
});
