// Audio helpers for transcription (ADR-0037, `docs/architecture/ai.md` section 8.1): pure functions
// over PCM samples, tested in node. Decoding and capture (Web Audio, MediaRecorder) live in the UI.

/** The sample rate sent to the transcription model. */
export const TRANSCRIPTION_RATE = 16000;
/** Segments are cut between these lengths, seconds. */
export const SEGMENT_MIN_SECONDS = 25;
export const SEGMENT_MAX_SECONDS = 35;
/** The length of the quiet window a cut is made in, milliseconds. */
const QUIET_WINDOW_MS = 200;
/** How far apart the windows that are compared start, milliseconds. */
const QUIET_HOP_MS = 10;
/** The shortest last segment, seconds. */
const MIN_LAST_SECONDS = 1;

/** A planned segment: `[start, end)` in samples. */
export interface SegmentPlan {
  start: number;
  end: number;
}

/** Float samples in [-1, 1] to 16-bit PCM (clamped; NaN is silence). */
export function floatToInt16(samples: Float32Array): Int16Array {
  const pcm = new Int16Array(samples.length);
  for (let i = 0; i < samples.length; i += 1) {
    const sample = samples[i] ?? 0;
    if (Number.isNaN(sample)) {
      continue;
    }
    const clamped = Math.max(-1, Math.min(1, sample));
    pcm[i] = Math.round(clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff);
  }
  return pcm;
}

/**
 * Mixes channels down to mono (the mean of the channels) and resamples to `targetRate`. Upsampling
 * interpolates linearly; downsampling averages the source samples each output sample covers (a box
 * filter, so frequencies above the new Nyquist are not folded back as loudly). The result has
 * `floor(length * targetRate / sourceRate)` samples; channels of unequal length are cut to the shortest.
 */
export function downmixAndResample(
  channels: readonly Float32Array[],
  sourceRate: number,
  targetRate: number = TRANSCRIPTION_RATE,
): Float32Array {
  if (!(sourceRate > 0) || !(targetRate > 0) || !Number.isFinite(sourceRate) || !Number.isFinite(targetRate)) {
    throw new RangeError("the sample rates must be positive numbers");
  }
  const first = channels[0];
  if (first === undefined) {
    return new Float32Array(0);
  }
  let length = first.length;
  for (const channel of channels) {
    length = Math.min(length, channel.length);
  }
  const mono = new Float32Array(length);
  for (const channel of channels) {
    for (let i = 0; i < length; i += 1) {
      mono[i] = (mono[i] ?? 0) + (channel[i] ?? 0);
    }
  }
  if (channels.length > 1) {
    for (let i = 0; i < length; i += 1) {
      mono[i] = (mono[i] ?? 0) / channels.length;
    }
  }
  if (sourceRate === targetRate) {
    return mono;
  }
  const ratio = sourceRate / targetRate;
  const outLength = Math.floor(length / ratio);
  const out = new Float32Array(outLength);
  if (ratio > 1) {
    // Prefix sums make each box average O(1).
    const sums = new Float64Array(length + 1);
    for (let i = 0; i < length; i += 1) {
      sums[i + 1] = (sums[i] ?? 0) + (mono[i] ?? 0);
    }
    for (let i = 0; i < outLength; i += 1) {
      const from = Math.floor(i * ratio);
      const to = Math.min(length, Math.max(from + 1, Math.floor((i + 1) * ratio)));
      out[i] = ((sums[to] ?? 0) - (sums[from] ?? 0)) / (to - from);
    }
    return out;
  }
  for (let i = 0; i < outLength; i += 1) {
    const position = i * ratio;
    const index = Math.floor(position);
    const fraction = position - index;
    const a = mono[index] ?? 0;
    const b = mono[Math.min(index + 1, length - 1)] ?? a;
    out[i] = a + (b - a) * fraction;
  }
  return out;
}

/** A RIFF/WAVE file of 16-bit mono PCM (a 44 byte header, then the samples, little-endian). */
export function encodeWav(pcm: Int16Array, rate: number = TRANSCRIPTION_RATE): Uint8Array {
  const dataBytes = pcm.length * 2;
  const bytes = new Uint8Array(44 + dataBytes);
  const view = new DataView(bytes.buffer);
  const tag = (offset: number, text: string): void => {
    for (let i = 0; i < text.length; i += 1) {
      view.setUint8(offset + i, text.charCodeAt(i));
    }
  };
  tag(0, "RIFF");
  view.setUint32(4, 36 + dataBytes, true);
  tag(8, "WAVE");
  tag(12, "fmt ");
  view.setUint32(16, 16, true); // size of the fmt chunk
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, rate, true);
  view.setUint32(28, rate * 2, true); // bytes per second
  view.setUint16(32, 2, true); // bytes per sample frame
  view.setUint16(34, 16, true); // bits per sample
  tag(36, "data");
  view.setUint32(40, dataBytes, true);
  for (let i = 0; i < pcm.length; i += 1) {
    view.setInt16(44 + i * 2, pcm[i] ?? 0, true);
  }
  return bytes;
}

/**
 * The cut inside `[from, to]`: the middle of the 200 ms window with the lowest RMS (the earliest of equals),
 * the windows starting every 10 ms. A range too short for one window is cut in its middle.
 */
function quietestCut(samples: Float32Array, from: number, to: number, rate: number): number {
  const window = Math.max(1, Math.round((rate * QUIET_WINDOW_MS) / 1000));
  const hop = Math.max(1, Math.round((rate * QUIET_HOP_MS) / 1000));
  if (to - from < window) {
    return Math.floor((from + to) / 2);
  }
  // Prefix sums of the squares over the range make each window's energy O(1).
  const squares = new Float64Array(to - from + 1);
  for (let i = from; i < to; i += 1) {
    const sample = samples[i] ?? 0;
    squares[i - from + 1] = (squares[i - from] ?? 0) + sample * sample;
  }
  let best = from;
  let bestEnergy = Number.POSITIVE_INFINITY;
  for (let start = from; start + window <= to; start += hop) {
    const energy = (squares[start - from + window] ?? 0) - (squares[start - from] ?? 0);
    if (energy < bestEnergy) {
      bestEnergy = energy;
      best = start;
    }
  }
  return best + Math.floor(window / 2);
}

/**
 * Cuts mono samples into segments of at most 35 s: while more than 35 s remain, a segment ends at the
 * quietest 200 ms window (lowest RMS) between 25 s and 35 s into it. The last segment is what is left, and
 * is never shorter than 1 s unless the audio itself is (the cut before it is then made early enough, so
 * that no segment is longer than 35 s). No samples give no segments.
 *
 * A cut depends only on the 35 s that follow its segment's start, once at least 36 s of audio are there:
 * with a stream of samples, a segment that is not the last one is final when 36 s follow its start.
 */
export function planSegments(samples: Float32Array, rate: number = TRANSCRIPTION_RATE): SegmentPlan[] {
  const total = samples.length;
  if (total === 0) {
    return [];
  }
  const max = Math.round(SEGMENT_MAX_SECONDS * rate);
  const min = Math.round(SEGMENT_MIN_SECONDS * rate);
  const minLast = Math.round(MIN_LAST_SECONDS * rate);
  const plan: SegmentPlan[] = [];
  let start = 0;
  while (total - start > max) {
    // The cut leaves at least `minLast` for the next segment, and stays within `max` of the start.
    const latest = Math.min(start + max, total - minLast);
    const cut = quietestCut(samples, start + min, latest, rate);
    plan.push({ start, end: cut });
    start = cut;
  }
  plan.push({ start, end: total });
  return plan;
}
