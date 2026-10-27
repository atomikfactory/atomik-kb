// The bytes of a PDF for the viewer (ADR-0031), behind one small interface so that the way they are
// read can be swapped: today the ranged `attachment_read` of the shell (every read at most
// `MAX_PREVIEW_BYTES`, only the chunks it touches are decrypted), later perhaps a custom URI scheme
// that the webview itself streams from.
//
// pdf.js asks for byte ranges of its own choosing (usually 64 KiB, the cross-reference table at the
// end first, then the objects of the pages in view). Asking the core for each of them would decrypt
// the same 4 MiB chunk over and over, so reads go through a small cache of aligned blocks, one block
// per attachment chunk: a block is read once, kept while it is among the last few used, and every
// answer is a copy (pdf.js hands the bytes it receives to its worker, which detaches them).

import { CHUNK_SIZE, MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import type { ByteRange } from "@atomik-kb/core-client";

/** What the viewer needs of a PDF's bytes: its size and any half-open range of them. */
export interface PdfByteSource {
  /** The size of the file in bytes. */
  readonly size: number;
  /**
   * The bytes `[start, end)`; `end` is clamped to the size. Rejects with the reader's error, and
   * with a `RangeError` for a range that starts past the end or is inverted. The array is the
   * caller's to keep.
   */
  read: (start: number, end: number) => Promise<Uint8Array>;
}

/** Reads one range of the attachment (the store's `readAttachmentRange`); at most `MAX_PREVIEW_BYTES` long. */
export type RangeReader = (range: ByteRange) => Promise<Uint8Array>;

export interface PdfSourceOptions {
  /** Bytes per cached block; at most `MAX_PREVIEW_BYTES`. Default: one attachment chunk. */
  blockSize?: number;
  /** How many blocks are kept. Default 4. */
  cachedBlocks?: number;
}

/** A source over `readRange` for a file of `size` bytes. */
export function createPdfSource(size: number, readRange: RangeReader, options: PdfSourceOptions = {}): PdfByteSource {
  const blockSize = Math.max(1, Math.min(options.blockSize ?? CHUNK_SIZE, MAX_PREVIEW_BYTES));
  const keep = Math.max(1, options.cachedBlocks ?? 4);
  /** Block index to its bytes (or the read under way), least recently used first (Map keeps insertion order). */
  const blocks = new Map<number, Promise<Uint8Array>>();

  function block(index: number): Promise<Uint8Array> {
    const cached = blocks.get(index);
    if (cached !== undefined) {
      // Most recently used goes last.
      blocks.delete(index);
      blocks.set(index, cached);
      return cached;
    }
    const start = index * blockSize;
    const pending = readRange({ start, end: Math.min(start + blockSize, size) });
    blocks.set(index, pending);
    // A failed read is not remembered: the next ask tries again.
    pending.catch(() => {
      if (blocks.get(index) === pending) {
        blocks.delete(index);
      }
    });
    while (blocks.size > keep) {
      const oldest = blocks.keys().next();
      if (oldest.done === true) {
        break;
      }
      blocks.delete(oldest.value);
    }
    return pending;
  }

  return {
    size,
    async read(start, end) {
      const last = Math.min(end, size);
      if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || start >= size || last <= start) {
        throw new RangeError("the byte range is outside the file");
      }
      const first = Math.floor(start / blockSize);
      const lastBlock = Math.floor((last - 1) / blockSize);
      const out = new Uint8Array(last - start);
      const parts: Promise<Uint8Array>[] = [];
      for (let index = first; index <= lastBlock; index += 1) {
        parts.push(block(index));
      }
      const read = await Promise.all(parts);
      read.forEach((bytes, offset) => {
        const blockStart = (first + offset) * blockSize;
        const from = Math.max(start, blockStart) - blockStart;
        const to = Math.min(last, blockStart + bytes.length) - blockStart;
        if (to > from) {
          out.set(bytes.subarray(from, to), Math.max(start, blockStart) - start);
        }
      });
      return out;
    },
  };
}
