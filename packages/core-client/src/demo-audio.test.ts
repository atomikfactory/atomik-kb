import { describe, expect, it } from "vitest";

import { demoWav } from "./demo-audio";

describe("demoWav", () => {
  it("is a well-formed 16-bit mono PCM WAV of the requested length", () => {
    const bytes = demoWav(2);
    const view = new DataView(bytes.buffer);
    const text = (offset: number, length: number): string => new TextDecoder().decode(bytes.subarray(offset, offset + length));
    expect(text(0, 4)).toBe("RIFF");
    expect(text(8, 4)).toBe("WAVE");
    expect(text(12, 4)).toBe("fmt ");
    expect(text(36, 4)).toBe("data");
    expect(view.getUint32(4, true)).toBe(bytes.length - 8);
    expect(view.getUint16(20, true)).toBe(1);
    expect(view.getUint16(22, true)).toBe(1);
    const rate = view.getUint32(24, true);
    expect(view.getUint32(40, true)).toBe(bytes.length - 44);
    expect((bytes.length - 44) / 2 / rate).toBeCloseTo(2, 5);
  });

  it("has sound in the middle and none at the start, and is never shorter than a tenth of a second", () => {
    const bytes = demoWav(1);
    const view = new DataView(bytes.buffer);
    const samples = (bytes.length - 44) / 2;
    let loudest = 0;
    for (let index = Math.floor(samples / 2) - 40; index < Math.floor(samples / 2) + 40; index += 1) {
      loudest = Math.max(loudest, Math.abs(view.getInt16(44 + index * 2, true)));
    }
    expect(loudest).toBeGreaterThan(1000);
    expect(view.getInt16(44, true)).toBe(0);
    expect(demoWav(0).length).toBeGreaterThan(44);
  });
});
