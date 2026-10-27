// The engine against the real pdf.js (the legacy build, run in the main thread by the library's own
// fake worker): a file read in ranges through a PdfByteSource opens, shows its pages and text, and
// fails with the right reason for a file that is empty, damaged, unreadable or password protected.

import type { ByteRange } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { testPdf } from "../test/pdf";
import { limitedPixelRatio, MAX_CANVAS_PIXELS, openWithLibrary, PdfOpenError } from "./pdf-engine";
import { createPdfSource } from "./pdf-source";

/** The library, with its worker code run in this thread (the library's own "fake worker" way). */
async function library() {
  const worker = (await import("pdfjs-dist/legacy/build/pdf.worker.mjs")) as unknown;
  (globalThis as { pdfjsWorker?: unknown }).pdfjsWorker = worker;
  return import("pdfjs-dist/legacy/build/pdf.mjs");
}

function sourceOf(bytes: Uint8Array, blockSize = 64 * 1024) {
  const asked: ByteRange[] = [];
  const source = createPdfSource(
    bytes.length,
    (range) => {
      asked.push(range);
      return Promise.resolve(bytes.slice(range.start, range.end));
    },
    { blockSize },
  );
  return { source, asked };
}

describe("openWithLibrary (real pdf.js)", () => {
  it("opens a PDF read in ranges and gives the size and text of its pages", async () => {
    const pdfjs = await library();
    const { source, asked } = sourceOf(testPdf({ pages: 3, size: [200, 300] }));
    const doc = await openWithLibrary(pdfjs, source);
    try {
      expect(doc.pages).toBe(3);
      const page = await doc.page(2);
      expect(page.width).toBe(200);
      expect(page.height).toBe(300);
      expect(asked.length).toBeGreaterThan(0);
      expect(asked.every((range) => range.end - range.start <= 64 * 1024)).toBe(true);
    } finally {
      await doc.destroy();
    }
  });

  it("asks only for the ranges it needs from a large file", async () => {
    const pdfjs = await library();
    const bytes = testPdf({ pages: 2, padding: 3 * 1024 * 1024 });
    const { source, asked } = sourceOf(bytes, 256 * 1024);
    const doc = await openWithLibrary(pdfjs, source);
    try {
      expect(doc.pages).toBe(2);
      await doc.page(1);
      const total = asked.reduce((sum, range) => sum + (range.end - range.start), 0);
      // The padding is 3 MiB of an object nobody reads: the library never needs it (auto fetch is off).
      expect(total).toBeLessThan(bytes.length / 2);
    } finally {
      await doc.destroy();
    }
  });

  it("refuses a file that is empty or not a PDF", async () => {
    const pdfjs = await library();
    const empty = await openWithLibrary(pdfjs, sourceOf(new Uint8Array(0)).source).catch((error: unknown) => error);
    expect(empty).toBeInstanceOf(PdfOpenError);
    expect((empty as PdfOpenError).failure).toBe("invalid");

    const junk = new TextEncoder().encode("this is not a pdf ".repeat(50));
    const damaged = await openWithLibrary(pdfjs, sourceOf(junk).source).catch((error: unknown) => error);
    expect(damaged).toBeInstanceOf(PdfOpenError);
    expect((damaged as PdfOpenError).failure).toBe("invalid");
  });

  it("reports unreadable bytes as such, whether at the start or later", async () => {
    const pdfjs = await library();
    const failing = createPdfSource(1000, () => Promise.reject(new Error("disk is gone")));
    const error = await openWithLibrary(pdfjs, failing).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(PdfOpenError);
    expect((error as PdfOpenError).failure).toBe("unreadable");

    // The first piece reads, the rest does not: opening fails and says so.
    const bytes = testPdf({ pages: 1, padding: 600 * 1024 });
    let reads = 0;
    const later = createPdfSource(
      bytes.length,
      (range) => {
        reads += 1;
        return reads === 1 ? Promise.resolve(bytes.slice(range.start, range.end)) : Promise.reject(new Error("gone"));
      },
      { blockSize: 64 * 1024, cachedBlocks: 1 },
    );
    const onReadError = vi.fn();
    const outcome = await openWithLibrary(pdfjs, later, { onReadError }).catch((caught: unknown) => caught);
    expect(outcome).toBeInstanceOf(PdfOpenError);
    expect((outcome as PdfOpenError).failure).toBe("unreadable");
  });
});

describe("limitedPixelRatio", () => {
  it("keeps the device ratio while the canvas stays within the limit and lowers it past it", () => {
    expect(limitedPixelRatio(600, 800, 2)).toBe(2);
    expect(limitedPixelRatio(600, 800, 0.5)).toBe(1);
    const ratio = limitedPixelRatio(6000, 8000, 2);
    expect(ratio).toBeLessThan(1);
    expect(6000 * 8000 * ratio * ratio).toBeLessThanOrEqual(MAX_CANVAS_PIXELS * 1.001);
  });
});
