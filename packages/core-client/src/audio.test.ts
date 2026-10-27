import { describe, expect, it } from "vitest";

import {
  SEGMENT_MAX_SECONDS,
  SEGMENT_MIN_SECONDS,
  TRANSCRIPTION_RATE,
  downmixAndResample,
  encodeWav,
  floatToInt16,
  planSegments,
} from "./audio";

const RATE = TRANSCRIPTION_RATE;

function sine(seconds: number, rate: number, frequency: number, amplitude = 0.5): Float32Array {
  const samples = new Float32Array(Math.round(seconds * rate));
  for (let i = 0; i < samples.length; i += 1) {
    samples[i] = amplitude * Math.sin((2 * Math.PI * frequency * i) / rate);
  }
  return samples;
}

function rms(samples: Float32Array): number {
  let sum = 0;
  for (const sample of samples) {
    sum += sample * sample;
  }
  return Math.sqrt(sum / samples.length);
}

describe("floatToInt16", () => {
  it("scales to 16 bits and clamps", () => {
    const pcm = floatToInt16(Float32Array.of(0, 1, -1, 0.5, -0.5, 2, -3, Number.NaN));
    expect(Array.from(pcm)).toEqual([0, 32767, -32768, 16384, -16384, 32767, -32768, 0]);
  });

  it("keeps the length", () => {
    expect(floatToInt16(new Float32Array(123))).toHaveLength(123);
    expect(floatToInt16(new Float32Array(0))).toHaveLength(0);
  });
});

describe("downmixAndResample", () => {
  it("gives 16000 samples for one second at 48 kHz, 44.1 kHz and 8 kHz", () => {
    for (const rate of [48000, 44100, 8000, 22050, 32000]) {
      const out = downmixAndResample([new Float32Array(rate)], rate);
      expect(out.length).toBe(16000);
    }
  });

  it("floors the length when the rates do not divide", () => {
    expect(downmixAndResample([new Float32Array(44101)], 44100).length).toBe(16000);
    expect(downmixAndResample([new Float32Array(10)], 48000, 16000).length).toBe(3);
  });

  it("averages the channels", () => {
    const left = Float32Array.of(1, 0.5, 0, -1);
    const right = Float32Array.of(0, 0.5, 1, -1);
    expect(Array.from(downmixAndResample([left, right], 16000, 16000))).toEqual([0.5, 0.5, 0.5, -1]);
  });

  it("returns a copy of a mono signal at the target rate and the empty signal for no channels", () => {
    const mono = Float32Array.of(0.1, 0.2, 0.3);
    const out = downmixAndResample([mono], 16000);
    expect(Array.from(out)).toEqual(Array.from(mono));
    expect(out).not.toBe(mono);
    expect(downmixAndResample([], 48000)).toHaveLength(0);
  });

  it("cuts channels of unequal length to the shortest", () => {
    expect(downmixAndResample([new Float32Array(10), new Float32Array(6)], 16000, 16000)).toHaveLength(6);
  });

  it("keeps a constant signal constant and a tone's loudness", () => {
    const flat = downmixAndResample([new Float32Array(48000).fill(0.25)], 48000);
    expect(Math.max(...flat)).toBeCloseTo(0.25, 6);
    expect(Math.min(...flat)).toBeCloseTo(0.25, 6);
    const tone = downmixAndResample([sine(1, 44100, 440)], 44100);
    expect(rms(tone)).toBeCloseTo(0.5 / Math.SQRT2, 1);
    const up = downmixAndResample([sine(1, 8000, 440)], 8000);
    expect(up.length).toBe(16000);
    expect(rms(up)).toBeCloseTo(0.5 / Math.SQRT2, 1);
  });

  it("interpolates linearly when upsampling", () => {
    const out = downmixAndResample([Float32Array.of(0, 1, 0, 1)], 8000, 16000);
    expect(Array.from(out).map((x) => Number(x.toFixed(6)))).toEqual([0, 0.5, 1, 0.5, 0, 0.5, 1, 1]);
  });

  it("refuses rates that are not positive", () => {
    expect(() => downmixAndResample([new Float32Array(4)], 0)).toThrow(RangeError);
    expect(() => downmixAndResample([new Float32Array(4)], 48000, -1)).toThrow(RangeError);
    expect(() => downmixAndResample([new Float32Array(4)], Number.NaN)).toThrow(RangeError);
  });
});

describe("encodeWav", () => {
  it("writes a 44 byte RIFF/WAVE header for 16-bit mono PCM", () => {
    const wav = encodeWav(Int16Array.of(0, 1, -1, 32767, -32768), 16000);
    expect(wav.length).toBe(44 + 10);
    const text = (from: number, to: number) => String.fromCharCode(...wav.slice(from, to));
    expect(text(0, 4)).toBe("RIFF");
    expect(text(8, 12)).toBe("WAVE");
    expect(text(12, 16)).toBe("fmt ");
    expect(text(36, 40)).toBe("data");
    const view = new DataView(wav.buffer);
    expect(view.getUint32(4, true)).toBe(36 + 10);
    expect(view.getUint32(16, true)).toBe(16);
    expect(view.getUint16(20, true)).toBe(1); // PCM
    expect(view.getUint16(22, true)).toBe(1); // mono
    expect(view.getUint32(24, true)).toBe(16000);
    expect(view.getUint32(28, true)).toBe(32000);
    expect(view.getUint16(32, true)).toBe(2);
    expect(view.getUint16(34, true)).toBe(16);
    expect(view.getUint32(40, true)).toBe(10);
  });

  it("stores the samples little-endian", () => {
    const wav = encodeWav(Int16Array.of(1, -2, 0x1234));
    expect(Array.from(wav.slice(44))).toEqual([1, 0, 0xfe, 0xff, 0x34, 0x12]);
  });

  it("uses the given rate and defaults to 16 kHz; an empty clip is a bare header", () => {
    expect(new DataView(encodeWav(new Int16Array(0), 48000).buffer).getUint32(24, true)).toBe(48000);
    expect(new DataView(encodeWav(new Int16Array(0)).buffer).getUint32(24, true)).toBe(16000);
    expect(encodeWav(new Int16Array(0))).toHaveLength(44);
  });

  it("round-trips samples through floatToInt16", () => {
    const pcm = floatToInt16(sine(0.01, RATE, 440));
    const wav = encodeWav(pcm);
    const back = new Int16Array(wav.buffer.slice(44));
    expect(Array.from(back)).toEqual(Array.from(pcm));
  });
});

describe("planSegments", () => {
  const seconds = (n: number) => Math.round(n * RATE);

  it("gives no segments for no samples and one segment for a short clip, even under a second", () => {
    expect(planSegments(new Float32Array(0))).toEqual([]);
    expect(planSegments(new Float32Array(seconds(0.4)))).toEqual([{ start: 0, end: seconds(0.4) }]);
    expect(planSegments(new Float32Array(seconds(12)))).toEqual([{ start: 0, end: seconds(12) }]);
    expect(planSegments(new Float32Array(seconds(35)))).toEqual([{ start: 0, end: seconds(35) }]);
  });

  it("cuts at the quietest 200 ms between 25 s and 35 s", () => {
    // Noise everywhere, silence from 31.0 s to 31.3 s: the cut is in the middle of that gap.
    const samples = sine(60, RATE, 220, 0.4);
    samples.fill(0, seconds(31.0), seconds(31.3));
    const plan = planSegments(samples);
    expect(plan[0]?.start).toBe(0);
    const cut = plan[0]?.end ?? 0;
    expect(cut).toBeGreaterThanOrEqual(seconds(31.0));
    expect(cut).toBeLessThanOrEqual(seconds(31.3));
    expect(plan[1]?.start).toBe(cut);
    expect(plan.at(-1)?.end).toBe(samples.length);
  });

  it("covers the audio exactly, in order, with segments of at most 35 s", () => {
    const samples = sine(200, RATE, 300, 0.3);
    const plan = planSegments(samples);
    let at = 0;
    for (const segment of plan) {
      expect(segment.start).toBe(at);
      expect(segment.end).toBeGreaterThan(segment.start);
      expect(segment.end - segment.start).toBeLessThanOrEqual(seconds(SEGMENT_MAX_SECONDS));
      at = segment.end;
    }
    expect(at).toBe(samples.length);
    // Every segment but the last is at least 25 s long.
    for (const segment of plan.slice(0, -1)) {
      expect(segment.end - segment.start).toBeGreaterThanOrEqual(seconds(SEGMENT_MIN_SECONDS));
    }
  });

  it("never makes the last segment shorter than a second (the cut before it moves earlier)", () => {
    for (const total of [35.2, 35.5, 35.9, 36, 70.3, 70.9, 105.4]) {
      const samples = sine(total, RATE, 200, 0.3);
      const plan = planSegments(samples);
      const last = plan.at(-1);
      expect(last).toBeDefined();
      if (plan.length > 1 && last !== undefined) {
        expect(last.end - last.start).toBeGreaterThanOrEqual(seconds(1));
      }
      for (const segment of plan) {
        expect(segment.end - segment.start).toBeLessThanOrEqual(seconds(SEGMENT_MAX_SECONDS));
      }
      expect(plan.at(-1)?.end).toBe(samples.length);
    }
  });

  it("picks the earliest of equally quiet windows (silence)", () => {
    const plan = planSegments(new Float32Array(seconds(40)));
    // 25 s plus half a window.
    expect(plan[0]?.end).toBe(seconds(25) + seconds(0.1));
    expect(plan).toHaveLength(2);
  });

  it("scales the lengths with the rate", () => {
    const rate = 8000;
    const samples = new Float32Array(rate * 40);
    const plan = planSegments(samples, rate);
    expect(plan).toHaveLength(2);
    expect(plan[0]?.end).toBe(rate * 25 + rate * 0.1);
  });

  it("does not change a cut when more samples arrive (once 36 s follow the start)", () => {
    const samples = sine(90, RATE, 250, 0.3);
    samples.fill(0, seconds(29), seconds(29.4));
    const early = planSegments(samples.subarray(0, seconds(37)));
    const late = planSegments(samples);
    expect(early[0]).toEqual(late[0]);
  });
});
