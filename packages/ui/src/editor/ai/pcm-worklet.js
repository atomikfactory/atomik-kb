/* global AudioWorkletProcessor, registerProcessor */
// The AudioWorklet of the recorder (ADR-0037, `docs/architecture/ai.md` section 8.6): it copies the
// microphone's samples to the page, which resamples them to 16 kHz for the transcription. It lives in a
// file of its own because a worklet is loaded as a module (`audioWorklet.addModule`) from the app's own
// origin, which the content security policy (`script-src 'self'`) allows; Vite serves it as an asset
// (`new URL("./pcm-worklet.js", import.meta.url)` in capture.ts).
//
// Samples are collected in blocks of 2048 frames per channel (about 40 ms at 48 kHz) before they are
// posted: one message per 128-frame render quantum would be thousands a second.

const BLOCK = 2048;

class PcmCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.channels = [];
    this.filled = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (input === undefined || input.length === 0) {
      return true;
    }
    if (this.channels.length !== input.length) {
      this.channels = input.map(() => new Float32Array(BLOCK));
      this.filled = 0;
    }
    const frames = input[0].length;
    for (let offset = 0; offset < frames; ) {
      const take = Math.min(frames - offset, BLOCK - this.filled);
      for (let channel = 0; channel < input.length; channel += 1) {
        this.channels[channel].set(input[channel].subarray(offset, offset + take), this.filled);
      }
      this.filled += take;
      offset += take;
      if (this.filled === BLOCK) {
        const block = this.channels;
        this.port.postMessage(block, block.map((samples) => samples.buffer));
        this.channels = input.map(() => new Float32Array(BLOCK));
        this.filled = 0;
      }
    }
    return true;
  }
}

registerProcessor("atomik-pcm-capture", PcmCapture);
