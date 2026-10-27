// Cuts the microphone's blocks into the pushes of a dictation session (ADR-0053, `docs/architecture/dictation.md`
// sections 5 and 6): 1,600 to 3,200 samples (100 to 200 ms at 16 kHz) each, as 16-bit PCM. The worklet delivers about
// 680 samples at a time, the script processor 4,096, so a block is split or waits for the next one; the audio the
// session is stopped with goes out in a last, shorter push.

import { floatToInt16 } from "@atomik-kb/core-client";

/** The fewest samples of a push (100 ms), except for the last one. */
export const MIN_PUSH_SAMPLES = 1_600;

/** The most samples of a push (200 ms): well under what `dictation_audio` takes (`DICTATION_MAX_PUSH_SAMPLES`, 2 s). */
export const MAX_PUSH_SAMPLES = 3_200;

export class PcmChunker {
  #blocks: Float32Array[] = [];
  #held = 0;

  /** The samples waiting for more. */
  get pending(): number {
    return this.#held;
  }

  /** Adds a block; answers the pushes it completes, in order (none while fewer than {@link MIN_PUSH_SAMPLES} are held). */
  push(samples: Float32Array): Int16Array[] {
    if (samples.length === 0) {
      return [];
    }
    this.#blocks.push(samples);
    this.#held += samples.length;
    const pushes: Int16Array[] = [];
    while (this.#held >= MIN_PUSH_SAMPLES) {
      pushes.push(this.#take(Math.min(this.#held, MAX_PUSH_SAMPLES)));
    }
    return pushes;
  }

  /** What is left, shorter than a regular push; `null` when nothing is. */
  flush(): Int16Array | null {
    return this.#held === 0 ? null : this.#take(this.#held);
  }

  /** Forgets what is held. */
  clear(): void {
    this.#blocks = [];
    this.#held = 0;
  }

  /** The first `count` held samples as PCM. */
  #take(count: number): Int16Array {
    const out = new Float32Array(count);
    let filled = 0;
    while (filled < count) {
      const block = this.#blocks[0];
      if (block === undefined) {
        break;
      }
      const wanted = count - filled;
      if (block.length <= wanted) {
        out.set(block, filled);
        filled += block.length;
        this.#blocks.shift();
      } else {
        out.set(block.subarray(0, wanted), filled);
        filled += wanted;
        this.#blocks[0] = block.subarray(wanted);
      }
    }
    this.#held -= count;
    return floatToInt16(out);
  }
}
