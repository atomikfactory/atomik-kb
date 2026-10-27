// A short audio clip for the in-memory demo (`/dev.html?seed=demo` shows it as a player in a `file`
// block, ADR-0032), built in code so that no binary file is checked in: 16-bit mono PCM in a WAV
// container, a rising and falling tone that every browser plays.

const SAMPLE_RATE = 8000;

/** A WAV file of `seconds` seconds (at least a tenth) of a 440 Hz tone that fades in and out. */
export function demoWav(seconds = 3): Uint8Array<ArrayBuffer> {
  const samples = Math.max(SAMPLE_RATE / 10, Math.floor(seconds * SAMPLE_RATE));
  const dataBytes = samples * 2;
  const bytes = new Uint8Array(44 + dataBytes);
  const view = new DataView(bytes.buffer);
  const tag = (offset: number, text: string): void => {
    for (let index = 0; index < text.length; index += 1) {
      view.setUint8(offset + index, text.charCodeAt(index));
    }
  };
  tag(0, "RIFF");
  view.setUint32(4, 36 + dataBytes, true);
  tag(8, "WAVE");
  tag(12, "fmt ");
  view.setUint32(16, 16, true); // size of the fmt chunk
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, SAMPLE_RATE, true);
  view.setUint32(28, SAMPLE_RATE * 2, true); // bytes per second
  view.setUint16(32, 2, true); // bytes per frame
  view.setUint16(34, 16, true); // bits per sample
  tag(36, "data");
  view.setUint32(40, dataBytes, true);
  for (let index = 0; index < samples; index += 1) {
    // A triangle envelope over the whole clip keeps the ends quiet (no click).
    const position = index / samples;
    const envelope = 1 - Math.abs(2 * position - 1);
    const value = Math.sin((2 * Math.PI * 440 * index) / SAMPLE_RATE) * envelope * 0.3;
    view.setInt16(44 + index * 2, Math.round(value * 32767), true);
  }
  return bytes;
}
