import { describe, expect, it } from "vitest";

import {
  CHUNK_SIZE,
  DEFAULT_RETENTION_DAYS,
  MAX_ATTACHMENT_BYTES,
  MAX_MIME_BYTES,
  MAX_NAME_BYTES,
  MAX_PREVIEW_BYTES,
  RETENTION_SETTING,
  sizeLimitError,
  sliceIntoChunks,
  validateAttachmentName,
  validateMime,
  validateRange,
} from "./attachments";
import type { UploadSource } from "./attachments";
import { CoreError } from "./errors";

function refusal(run: () => unknown): CoreError {
  try {
    run();
  } catch (error) {
    expect(error).toBeInstanceOf(CoreError);
    return error as CoreError;
  }
  throw new Error("expected the call to throw");
}

async function collect(source: UploadSource, chunkSize?: number): Promise<number[][]> {
  const pieces: number[][] = [];
  for await (const piece of sliceIntoChunks(source, chunkSize)) {
    expect(piece).toBeInstanceOf(Uint8Array);
    pieces.push(Array.from(piece));
  }
  return pieces;
}

function bytes(...values: number[]): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(values);
}

/** Byte equality without the cost of a deep comparison of megabytes. */
function sameBytes(a: Uint8Array | undefined, b: Uint8Array): boolean {
  return a !== undefined && a.length === b.length && a.every((value, index) => value === b[index]);
}

async function* stream(...pieces: Uint8Array[]): AsyncGenerator<Uint8Array> {
  for (const piece of pieces) {
    await Promise.resolve();
    yield piece;
  }
}

describe("shared limits", () => {
  it("are the values of kb_types::attachments", () => {
    expect(CHUNK_SIZE).toBe(4_194_304);
    expect(MAX_ATTACHMENT_BYTES).toBe(2_147_483_648);
    expect(MAX_NAME_BYTES).toBe(255);
    expect(MAX_MIME_BYTES).toBe(127);
    expect(MAX_PREVIEW_BYTES).toBe(33_554_432);
    expect(DEFAULT_RETENTION_DAYS).toBe(30);
    expect(RETENTION_SETTING).toBe("app.attachments.retention_days");
  });
});

describe("validateAttachmentName", () => {
  it("accepts names as given (never trimmed), up to 255 UTF-8 bytes", () => {
    for (const name of ["a", " photo.png ", ".hidden", "...", "é".repeat(127), "x".repeat(255), "日本語.txt"]) {
      expect(validateAttachmentName(name)).toBe(name);
    }
  });

  it("refuses blank, too long, separators, NUL, . and .. on the field name with the core's messages", () => {
    const separators = "the name must not contain path separators";
    const cases: [string, string][] = [
      ["", "the attachment needs a name"],
      ["  \t ", "the attachment needs a name"],
      ["x".repeat(256), "the name must be at most 255 bytes"],
      ["é".repeat(128), "the name must be at most 255 bytes"],
      ["a/b", separators],
      ["a\\b", separators],
      ["a\0b", separators],
      [".", separators],
      ["..", separators],
    ];
    for (const [name, message] of cases) {
      expect(refusal(() => validateAttachmentName(name)).toApiError(), JSON.stringify(name)).toEqual({
        code: "InvalidInput",
        message,
        details: { field: "name" },
      });
    }
  });

  it("checks blank first, then the length, then the separators", () => {
    expect(refusal(() => validateAttachmentName(`/${"x".repeat(255)}`)).message).toBe("the name must be at most 255 bytes");
  });
});

describe("validateMime", () => {
  it("accepts printable ASCII without spaces, exactly one inner slash, up to 127 bytes", () => {
    for (const mime of ["image/png", "application/octet-stream", "text/plain;charset=utf-8", `a/${"b".repeat(125)}`]) {
      expect(validateMime(mime)).toBe(mime);
    }
  });

  it("refuses the rest on the field mime with the core's messages, length first", () => {
    const length = "the MIME type must be 1 to 127 bytes";
    const printable = "the MIME type must be printable ASCII";
    const shape = "the MIME type must look like type/subtype";
    const cases: [string, string][] = [
      ["", length],
      [`a/${"b".repeat(126)}`, length],
      [`é/${"b".repeat(125)}`, length],
      ["image/pngé", printable],
      ["text/plain; charset=utf-8", printable],
      ["image/\tpng", printable],
      ["png", shape],
      ["image/svg/xml", shape],
      ["/png", shape],
      ["image/", shape],
    ];
    for (const [mime, message] of cases) {
      expect(refusal(() => validateMime(mime)).toApiError(), JSON.stringify(mime)).toEqual({
        code: "InvalidInput",
        message,
        details: { field: "mime" },
      });
    }
  });
});

describe("validateRange and sizeLimitError", () => {
  it("accept half-open ranges inside the attachment, empty ones included", () => {
    expect(validateRange({ start: 0, end: 10 }, 10)).toEqual({ start: 0, end: 10 });
    expect(validateRange({ start: 9, end: 10 }, 10)).toEqual({ start: 9, end: 10 });
    expect(validateRange({ start: 4, end: 4 }, 10)).toEqual({ start: 4, end: 4 });
    expect(validateRange({ start: 0, end: 0 }, 0)).toEqual({ start: 0, end: 0 });
  });

  it("refuse reversed, outside, negative and fractional ranges on the field range", () => {
    for (const range of [
      { start: 5, end: 4 },
      { start: 0, end: 11 },
      { start: 11, end: 11 },
      { start: -1, end: 2 },
      { start: 0.5, end: 2 },
    ]) {
      expect(refusal(() => validateRange(range, 10)).toApiError(), JSON.stringify(range)).toEqual({
        code: "InvalidInput",
        message: "the byte range is outside the attachment",
        details: { field: "range" },
      });
    }
  });

  it("the size error names the field and the limit", () => {
    expect(sizeLimitError().toApiError()).toEqual({
      code: "InvalidInput",
      message: "an attachment is at most 2147483648 bytes",
      details: { field: "size", limit: MAX_ATTACHMENT_BYTES },
    });
  });
});

describe("sliceIntoChunks", () => {
  it("gathers small pieces and splits large ones into full chunks, the last one shorter", async () => {
    expect(await collect([bytes(1), bytes(2, 3), bytes(4, 5, 6, 7, 8)], 3)).toEqual([
      [1, 2, 3],
      [4, 5, 6],
      [7, 8],
    ]);
    expect(await collect([bytes(1, 2, 3, 4, 5, 6, 7)], 3)).toEqual([[1, 2, 3], [4, 5, 6], [7]]);
    expect(await collect([bytes(1, 2, 3), bytes(4, 5, 6)], 3)).toEqual([
      [1, 2, 3],
      [4, 5, 6],
    ]);
    expect(await collect(stream(bytes(1), bytes(), bytes(2, 3, 4)), 2)).toEqual([
      [1, 2],
      [3, 4],
    ]);
  });

  it("yields nothing for an empty source", async () => {
    expect(await collect([], 3)).toEqual([]);
    expect(await collect([bytes(), bytes()], 3)).toEqual([]);
    expect(await collect(new Blob([]), 3)).toEqual([]);
  });

  it("reads a Blob one slice at a time", async () => {
    expect(await collect(new Blob([bytes(1, 2, 3, 4, 5)]), 2)).toEqual([[1, 2], [3, 4], [5]]);
  });

  it("cuts at CHUNK_SIZE by default and never reuses a yielded buffer", async () => {
    const source = new Uint8Array(CHUNK_SIZE + 10).map((_, i) => i % 251);
    const pieces: Uint8Array[] = [];
    for await (const piece of sliceIntoChunks([source.subarray(0, 7), source.subarray(7)])) {
      pieces.push(piece);
    }
    expect(pieces.map((piece) => piece.length)).toEqual([CHUNK_SIZE, 10]);
    expect(sameBytes(pieces[0], source.subarray(0, CHUNK_SIZE))).toBe(true);
    expect(sameBytes(pieces[1], source.subarray(CHUNK_SIZE))).toBe(true);
  });

  it("refuses a source that yields something other than bytes, and a bad chunk size", async () => {
    const wrong = [[1, 2, 3]] as unknown as Iterable<Uint8Array>;
    await expect(collect(wrong, 2)).rejects.toMatchObject({
      code: "Internal",
      message: "the upload source yielded something other than bytes",
    });
    await expect(collect([bytes(1)], 0)).rejects.toMatchObject({ code: "Internal" });
  });
});
