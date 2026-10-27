// The ranged byte source of the PDF viewer (ADR-0031) against a fake reader.

import { MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import type { ByteRange } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { createPdfSource } from "./pdf-source";

/** A file of `size` bytes whose byte at `i` is `i % 251`, and a reader over it that records its asks. */
function fake(size: number) {
  const file = Uint8Array.from({ length: size }, (_, index) => index % 251);
  const asked: ByteRange[] = [];
  const reader = vi.fn((range: ByteRange) => {
    asked.push(range);
    return Promise.resolve(file.slice(range.start, range.end));
  });
  return { file, asked, reader };
}

describe("createPdfSource", () => {
  it("answers any range with the bytes of the file, across block borders", async () => {
    const { file, reader } = fake(1000);
    const source = createPdfSource(1000, reader, { blockSize: 64 });
    expect(source.size).toBe(1000);
    for (const [start, end] of [
      [0, 1],
      [0, 64],
      [60, 70],
      [63, 65],
      [10, 700],
      [999, 1000],
      [0, 1000],
    ] as const) {
      expect(await source.read(start, end), `${String(start)}-${String(end)}`).toEqual(file.slice(start, end));
    }
  });

  it("clamps the end to the size and refuses a range outside the file", async () => {
    const { file, reader } = fake(100);
    const source = createPdfSource(100, reader, { blockSize: 32 });
    expect(await source.read(90, 5000)).toEqual(file.slice(90));
    reader.mockClear();
    await expect(source.read(100, 101)).rejects.toThrow(RangeError);
    await expect(source.read(50, 50)).rejects.toThrow(RangeError);
    await expect(source.read(60, 50)).rejects.toThrow(RangeError);
    await expect(source.read(-1, 5)).rejects.toThrow(RangeError);
    await expect(source.read(1.5, 5)).rejects.toThrow(RangeError);
    expect(reader).not.toHaveBeenCalled();
  });

  it("asks the reader for aligned blocks of at most MAX_PREVIEW_BYTES, and only for those it touches", async () => {
    const { asked, reader } = fake(1000);
    const source = createPdfSource(1000, reader, { blockSize: 100 });
    await source.read(250, 260);
    expect(asked).toEqual([{ start: 200, end: 300 }]);
    await source.read(950, 1000);
    expect(asked.at(-1)).toEqual({ start: 900, end: 1000 });
    expect(asked).toHaveLength(2);

    // A block size beyond the limit is cut to it; the last block ends at the file's end.
    const big = fake(MAX_PREVIEW_BYTES + 10);
    const huge = createPdfSource(big.file.length, big.reader, { blockSize: MAX_PREVIEW_BYTES * 2 });
    await huge.read(MAX_PREVIEW_BYTES, MAX_PREVIEW_BYTES + 4);
    expect(big.asked).toEqual([{ start: MAX_PREVIEW_BYTES, end: MAX_PREVIEW_BYTES + 10 }]);
    expect(big.asked.every((range) => range.end - range.start <= MAX_PREVIEW_BYTES)).toBe(true);
  });

  it("reads a block once while it is cached, concurrent asks included", async () => {
    const { reader } = fake(1000);
    const source = createPdfSource(1000, reader, { blockSize: 100 });
    await Promise.all([source.read(0, 10), source.read(20, 30), source.read(90, 110)]);
    await source.read(5, 6);
    // Blocks 0 and 1, each once.
    expect(reader).toHaveBeenCalledTimes(2);
  });

  it("forgets the least recently used block past its capacity", async () => {
    const { reader } = fake(1000);
    const source = createPdfSource(1000, reader, { blockSize: 100, cachedBlocks: 2 });
    await source.read(0, 1); // block 0
    await source.read(100, 101); // block 1
    await source.read(0, 1); // block 0 again: now the most recent
    await source.read(200, 201); // block 2 pushes out block 1
    expect(reader).toHaveBeenCalledTimes(3);
    await source.read(0, 1);
    expect(reader).toHaveBeenCalledTimes(3);
    await source.read(100, 101);
    expect(reader).toHaveBeenCalledTimes(4);
  });

  it("hands out copies: bytes the caller gives away (pdf.js transfers them) do not damage the cache", async () => {
    const { file, reader } = fake(200);
    const source = createPdfSource(200, reader, { blockSize: 100 });
    const first = await source.read(10, 20);
    first.fill(0);
    new Uint8Array(first.buffer).fill(9);
    expect(await source.read(10, 20)).toEqual(file.slice(10, 20));
    expect(reader).toHaveBeenCalledTimes(1);
  });

  it("does not remember a failed read: the next ask tries again", async () => {
    const file = Uint8Array.from({ length: 100 }, (_, index) => index);
    const reader = vi
      .fn<(range: ByteRange) => Promise<Uint8Array>>()
      .mockRejectedValueOnce(new Error("disk"))
      .mockImplementation((range) => Promise.resolve(file.slice(range.start, range.end)));
    const source = createPdfSource(100, reader, { blockSize: 100 });
    await expect(source.read(0, 10)).rejects.toThrow("disk");
    expect(await source.read(0, 10)).toEqual(file.slice(0, 10));
    expect(reader).toHaveBeenCalledTimes(2);
  });
});
