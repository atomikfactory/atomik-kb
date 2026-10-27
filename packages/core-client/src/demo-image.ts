// A tiny picture for the in-memory demo (`/dev.html?seed=demo` shows it in an `image` block),
// built in code so that no binary file is checked in: a valid 16x16 RGB PNG whose image data
// is one stored (uncompressed) deflate block.

/** Width and height of {@link demoImagePng}, in pixels. */
export const DEMO_IMAGE_SIZE = 16;

/** CRC-32 (ISO 3309, as PNG chunks use it). */
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

/** Adler-32 (the checksum that ends a zlib stream). */
function adler32(bytes: Uint8Array): number {
  let a = 1;
  let b = 0;
  for (const byte of bytes) {
    a = (a + byte) % 65521;
    b = (b + a) % 65521;
  }
  return ((b << 16) | a) >>> 0;
}

function u32(value: number): number[] {
  return [(value >>> 24) & 0xff, (value >>> 16) & 0xff, (value >>> 8) & 0xff, value & 0xff];
}

/** One PNG chunk: length, type, data, CRC of type and data. */
function chunk(type: string, data: Uint8Array): number[] {
  const typed = new Uint8Array(4 + data.length);
  typed.set(Array.from(type, (char) => char.charCodeAt(0)));
  typed.set(data, 4);
  return [...u32(data.length), ...typed, ...u32(crc32(typed))];
}

/**
 * A 16x16 PNG (about 900 bytes): a diagonal teal-to-indigo gradient with a white dot in the
 * middle. A fresh copy on every call.
 */
export function demoImagePng(): Uint8Array<ArrayBuffer> {
  const size = DEMO_IMAGE_SIZE;
  // Scanlines: a filter byte (0, none) and RGB per pixel.
  const raw = new Uint8Array(size * (1 + size * 3));
  for (let y = 0; y < size; y += 1) {
    const row = y * (1 + size * 3);
    for (let x = 0; x < size; x += 1) {
      const t = (x + y) / (2 * (size - 1));
      const dot = (x - 7.5) ** 2 + (y - 7.5) ** 2 <= 9;
      const pixel = row + 1 + x * 3;
      raw[pixel] = dot ? 255 : Math.round(20 + t * 70);
      raw[pixel + 1] = dot ? 255 : Math.round(170 - t * 120);
      raw[pixel + 2] = dot ? 255 : Math.round(160 + t * 80);
    }
  }
  // zlib: header (deflate, 32 KiB window, no dictionary), one final stored block, Adler-32.
  const zlib = [0x78, 0x01, 0x01, raw.length & 0xff, raw.length >>> 8, ~raw.length & 0xff, (~raw.length >>> 8) & 0xff];
  const idat = new Uint8Array([...zlib, ...raw, ...u32(adler32(raw))]);
  // IHDR: width, height, bit depth 8, colour type 2 (RGB), deflate, adaptive filtering, no interlace.
  const ihdr = new Uint8Array([...u32(size), ...u32(size), 8, 2, 0, 0, 0]);
  return new Uint8Array([
    0x89,
    0x50,
    0x4e,
    0x47,
    0x0d,
    0x0a,
    0x1a,
    0x0a,
    ...chunk("IHDR", ihdr),
    ...chunk("IDAT", idat),
    ...chunk("IEND", new Uint8Array()),
  ]);
}
