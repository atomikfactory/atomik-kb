// The microphone of dictation (ADR-0053): the chunks of 1,600 to 3,200 samples, and the capture of samples alone, which is
// the recorder's 16 kHz tap with no `MediaRecorder` (`createPcmCapture`).

import { floatToInt16 } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { browserCaptureEnv, browserPcmEnv, CaptureError, createPcmCapture } from "../ai/capture";
import { FakeMediaRecorder, fakeMicrophone, installMicrophone, removeMicrophone } from "../ai/recorder-kit";
import { MAX_PUSH_SAMPLES, MIN_PUSH_SAMPLES, PcmChunker } from "./chunker";

afterEach(() => {
  removeMicrophone();
  vi.restoreAllMocks();
});

describe("the chunker", () => {
  it("keeps its limits inside what the shell takes", () => {
    expect(MIN_PUSH_SAMPLES).toBe(1_600);
    expect(MAX_PUSH_SAMPLES).toBe(3_200);
    expect(MAX_PUSH_SAMPLES).toBeLessThanOrEqual(32_000);
  });

  it("holds small blocks until a chunk of 100 ms is there, then hands out all it has up to 200 ms", () => {
    const chunker = new PcmChunker();
    expect(chunker.push(new Float32Array(683))).toEqual([]);
    expect(chunker.push(new Float32Array(683))).toEqual([]);
    expect(chunker.pending).toBe(1_366);
    const out = chunker.push(new Float32Array(683));
    expect(out.map((chunk) => chunk.length)).toEqual([2_049]);
    expect(chunker.pending).toBe(0);
  });

  it("splits a block that is longer than a chunk, and keeps the rest for the next one", () => {
    const chunker = new PcmChunker();
    expect(chunker.push(new Float32Array(4_096)).map((chunk) => chunk.length)).toEqual([3_200]);
    expect(chunker.pending).toBe(896);
    // 896 + 4,096: one full chunk, and 1,792 is enough for another.
    expect(chunker.push(new Float32Array(4_096)).map((chunk) => chunk.length)).toEqual([3_200, 1_792]);
    expect(chunker.pending).toBe(0);
    expect(chunker.push(new Float32Array(30_000)).every((chunk) => chunk.length >= 1_600 && chunk.length <= 3_200)).toBe(true);
  });

  it("never loses or reorders a sample, and converts to 16-bit PCM", () => {
    const chunker = new PcmChunker();
    const input = Float32Array.from({ length: 10_000 }, (_, index) => ((index % 200) - 100) / 100);
    const pieces = [3_000, 1_234, 4_000, 1_766];
    let offset = 0;
    const out: number[] = [];
    for (const size of pieces) {
      for (const chunk of chunker.push(input.subarray(offset, offset + size))) {
        out.push(...chunk);
      }
      offset += size;
    }
    const rest = chunker.flush();
    out.push(...(rest ?? []));
    expect(out).toHaveLength(10_000);
    // -1 is -32768 and 0 is 0; every sample is where it was, as `floatToInt16` makes it.
    expect(out[0]).toBe(-32_768);
    expect(out[100]).toBe(0);
    expect(out).toEqual(Array.from(floatToInt16(input)));
  });

  it("flushes what is left, however short, once, and forgets on clear", () => {
    const chunker = new PcmChunker();
    chunker.push(new Float32Array(500));
    expect(chunker.flush()).toHaveLength(500);
    expect(chunker.flush()).toBeNull();
    chunker.push(new Float32Array(500));
    chunker.clear();
    expect(chunker.flush()).toBeNull();
    expect(chunker.push(new Float32Array(0))).toEqual([]);
  });
});

describe("the capture of samples alone", () => {
  it("needs a microphone API and Web Audio, and not a MediaRecorder", () => {
    expect(browserPcmEnv()).toBeNull(); // jsdom has none
    installMicrophone(fakeMicrophone());
    expect(browserPcmEnv()).not.toBeNull();
    expect(browserCaptureEnv()).not.toBeNull();
    vi.stubGlobal("MediaRecorder", undefined);
    expect(browserPcmEnv()).not.toBeNull(); // dictation does not care
    expect(browserCaptureEnv()).toBeNull(); // the recorder does
  });

  it("asks for the microphone like the recorder, hands out 16 kHz samples and a level, makes no file, and lets go", async () => {
    const microphone = fakeMicrophone();
    installMicrophone(microphone);
    const env = browserPcmEnv();
    if (env === null) {
      throw new Error("no environment");
    }
    FakeMediaRecorder.instances.length = 0;
    const blocks: number[] = [];
    const levels: number[] = [];
    const capture = createPcmCapture(
      {
        onPcm: (samples) => {
          blocks.push(samples.length);
        },
        onLevel: (level) => {
          levels.push(level);
        },
      },
      env,
    );
    await capture.start();
    expect(microphone.getUserMedia).toHaveBeenCalledWith({ audio: true });
    expect(FakeMediaRecorder.instances).toHaveLength(0);
    microphone.feed(0.5);
    expect(blocks.reduce((sum, length) => sum + length, 0)).toBe(8_000);
    expect(Math.max(...levels)).toBeGreaterThan(0);

    await capture.stop();
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalledTimes(1);
    expect(microphone.context().close).toHaveBeenCalled();
    expect(FakeMediaRecorder.instances).toHaveLength(0);
  });

  it("fails with the kind of the failure, as the recorder's capture does", async () => {
    for (const [name, reason] of [
      ["NotAllowedError", "denied"],
      ["NotFoundError", "noDevice"],
      ["AbortError", "failed"],
    ] as const) {
      const microphone = fakeMicrophone({ name });
      installMicrophone(microphone);
      const env = browserPcmEnv();
      if (env === null) {
        throw new Error("no environment");
      }
      const capture = createPcmCapture({ onPcm: () => undefined, onLevel: () => undefined }, env);
      const failure: unknown = await capture.start().catch((error: unknown) => error);
      expect(failure, name).toBeInstanceOf(CaptureError);
      expect((failure as CaptureError).reason, name).toBe(reason);
    }
  });

  it("lets go of a microphone that is granted after cancel", async () => {
    const microphone = fakeMicrophone(undefined, { pending: true });
    installMicrophone(microphone);
    const env = browserPcmEnv();
    if (env === null) {
      throw new Error("no environment");
    }
    const capture = createPcmCapture({ onPcm: () => undefined, onLevel: () => undefined }, env);
    const started = capture.start();
    capture.cancel();
    microphone.answer("allow");
    await started;
    expect(microphone.streams[0]?.tracks[0]?.stop).toHaveBeenCalled();
  });
});
