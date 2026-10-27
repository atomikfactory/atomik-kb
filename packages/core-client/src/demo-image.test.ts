import { describe, expect, it } from "vitest";

import { DEMO_IMAGE_SIZE, demoImagePng } from "./demo-image";

function u32(bytes: Uint8Array, at: number): number {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(at);
}

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** The chunks of a PNG, checking every CRC on the way. */
function chunks(png: Uint8Array): { type: string; data: Uint8Array }[] {
  const found: { type: string; data: Uint8Array }[] = [];
  let at = 8;
  while (at < png.length) {
    const length = u32(png, at);
    const typed = png.subarray(at + 4, at + 8 + length);
    expect(u32(png, at + 8 + length)).toBe(crc32(typed));
    found.push({ type: String.fromCharCode(...typed.subarray(0, 4)), data: typed.subarray(4) });
    at += 12 + length;
  }
  expect(at).toBe(png.length);
  return found;
}

describe("demoImagePng", () => {
  it("is a valid 16x16 RGB PNG whose pixels inflate to the scanlines", async () => {
    const png = demoImagePng();
    expect(Array.from(png.subarray(0, 8))).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const parts = chunks(png);
    expect(parts.map((part) => part.type)).toEqual(["IHDR", "IDAT", "IEND"]);
    const [ihdr, idat] = parts;
    expect(u32(ihdr?.data ?? new Uint8Array(), 0)).toBe(DEMO_IMAGE_SIZE);
    expect(u32(ihdr?.data ?? new Uint8Array(), 4)).toBe(DEMO_IMAGE_SIZE);
    expect(Array.from(ihdr?.data.subarray(8) ?? [])).toEqual([8, 2, 0, 0, 0]);
    // The zlib stream inflates (the Adler-32 is checked by the decoder) to one filter byte and
    // three bytes per pixel on every row.
    const stream = new Blob([(idat?.data ?? new Uint8Array()).slice()])
      .stream()
      .pipeThrough(new DecompressionStream("deflate"));
    const raw = new Uint8Array(await new Response(stream).arrayBuffer());
    expect(raw).toHaveLength(DEMO_IMAGE_SIZE * (1 + DEMO_IMAGE_SIZE * 3));
    expect(Array.from({ length: DEMO_IMAGE_SIZE }, (_, row) => raw[row * (1 + DEMO_IMAGE_SIZE * 3)])).toEqual(
      new Array<number>(DEMO_IMAGE_SIZE).fill(0),
    );
  });

  it("returns a fresh copy every time", () => {
    const first = demoImagePng();
    first.fill(0);
    expect(demoImagePng()[1]).toBe(0x50);
  });
});
