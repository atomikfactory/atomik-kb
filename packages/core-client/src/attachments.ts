// Attachments (ADR-0020, `docs/development/milestone-05.md`) on the TypeScript side: the
// shared limits and settings of `kb_types::attachments` (the download policy of Milestone 8,
// ADR-0023, included), the core's validation of names, MIME types and byte ranges (same codes,
// fields and messages, used by `CoreClient.attachmentUpload` before it starts an upload and by
// the in-memory mirror), and the slicing of an upload source into pieces of at most one chunk.

import { CoreError, internalError } from "./errors";
import type { ByteRange } from "./generated/ByteRange";
import { invalid } from "./validate";

/** Plaintext bytes per chunk (`kb_types::attachments::CHUNK_SIZE`, 4 MiB); also the largest piece one write sends. */
export const CHUNK_SIZE = 4 * 1024 * 1024;
/** Largest attachment accepted (`MAX_ATTACHMENT_BYTES`, 2 GiB, 512 chunks). */
export const MAX_ATTACHMENT_BYTES = 2 * 1024 * 1024 * 1024;
/** Longest attachment name, in UTF-8 bytes (`MAX_NAME_BYTES`). */
export const MAX_NAME_BYTES = 255;
/** Longest MIME type, in bytes of printable ASCII (`MAX_MIME_BYTES`). */
export const MAX_MIME_BYTES = 127;
/**
 * Largest attachment the UI previews (`MAX_PREVIEW_BYTES`, 32 MiB): a read without a byte
 * range answers at most this much, so larger images show a "too large to preview" card.
 */
export const MAX_PREVIEW_BYTES = 32 * 1024 * 1024;
/** Days an unreferenced attachment is kept before garbage collection when the setting is absent. */
export const DEFAULT_RETENTION_DAYS = 30;
/** Setting holding the retention in days (a non-negative integer; `0` keeps attachments forever). */
export const RETENTION_SETTING = "app.attachments.retention_days";
/**
 * Setting holding the download policy (`kb_types::attachments::DOWNLOAD_POLICY_SETTING`,
 * ADR-0023): {@link DOWNLOAD_POLICY_EAGER} (the default) or {@link DOWNLOAD_POLICY_ON_DEMAND}.
 */
export const DOWNLOAD_POLICY_SETTING = "app.attachments.download";
/** Download policy value: every chunk of every attachment that arrives is fetched at once (the default). */
export const DOWNLOAD_POLICY_EAGER = "eager";
/** Download policy value: an attachment is fetched when it is opened or `AttachmentDownload` asks for it. */
export const DOWNLOAD_POLICY_ON_DEMAND = "on_demand";

/** A value of {@link DOWNLOAD_POLICY_SETTING}. */
export type DownloadPolicy = typeof DOWNLOAD_POLICY_EAGER | typeof DOWNLOAD_POLICY_ON_DEMAND;

/**
 * The policy a stored value of {@link DOWNLOAD_POLICY_SETTING} means: the text `"on_demand"` is
 * on demand, anything else (absent, `null`, another text or type) is the default, eager.
 */
export function downloadPolicyOf(value: unknown): DownloadPolicy {
  return value === DOWNLOAD_POLICY_ON_DEMAND ? DOWNLOAD_POLICY_ON_DEMAND : DOWNLOAD_POLICY_EAGER;
}

const utf8 = new TextEncoder();

/**
 * An attachment name as the core accepts it (`kb-core`'s `attachment_name`, checked in this
 * order): not blank (`"the attachment needs a name"`), at most 255 UTF-8 bytes, no `/`, `\`
 * or NUL and neither `.` nor `..` (`"the name must not contain path separators"`). Taken as
 * given, never trimmed. Throws `InvalidInput` for the field `name`; returns the name.
 */
export function validateAttachmentName(name: string): string {
  if (/^\p{White_Space}*$/u.test(name)) {
    throw invalid("name", "the attachment needs a name");
  }
  if (utf8.encode(name).length > MAX_NAME_BYTES) {
    throw invalid("name", `the name must be at most ${String(MAX_NAME_BYTES)} bytes`);
  }
  if (name.includes("/") || name.includes("\\") || name.includes("\0") || name === "." || name === "..") {
    throw invalid("name", "the name must not contain path separators");
  }
  return name;
}

/**
 * A MIME type as the core accepts it (`kb-core`'s `mime_type`, checked in this order): 1 to
 * 127 bytes, printable ASCII without spaces (`!` to `~`), exactly one `/` and not at either
 * end (`"the MIME type must look like type/subtype"`). Throws `InvalidInput` for the field
 * `mime`; returns the type.
 */
export function validateMime(mime: string): string {
  const bytes = utf8.encode(mime).length;
  if (bytes === 0 || bytes > MAX_MIME_BYTES) {
    throw invalid("mime", `the MIME type must be 1 to ${String(MAX_MIME_BYTES)} bytes`);
  }
  if (!/^[\x21-\x7e]*$/u.test(mime)) {
    throw invalid("mime", "the MIME type must be printable ASCII");
  }
  if (mime.split("/").length !== 2 || mime.startsWith("/") || mime.endsWith("/")) {
    throw invalid("mime", "the MIME type must look like type/subtype");
  }
  return mime;
}

/** `InvalidInput` for the field `size`, with `details.limit`: an upload past {@link MAX_ATTACHMENT_BYTES}. */
export function sizeLimitError(): CoreError {
  return new CoreError({
    code: "InvalidInput",
    message: `an attachment is at most ${String(MAX_ATTACHMENT_BYTES)} bytes`,
    details: { field: "size", limit: MAX_ATTACHMENT_BYTES },
  });
}

/**
 * A byte range of an attachment of `size` bytes as the core accepts it: half-open, whole
 * numbers with `0 <= start <= end <= size` (an empty range reads nothing). Throws
 * `InvalidInput` for the field `range` ("the byte range is outside the attachment").
 */
export function validateRange(range: ByteRange, size: number): ByteRange {
  const { start, end } = range;
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start < 0 || start > end || end > size) {
    throw invalid("range", "the byte range is outside the attachment");
  }
  return { start, end };
}

/** What an upload reads from: a `Blob` (a `File` from a picker, a paste or a drop) or a stream of byte pieces. */
export type UploadSource = Blob | AsyncIterable<Uint8Array> | Iterable<Uint8Array>;

function isBlob(source: UploadSource): source is Blob {
  return typeof Blob !== "undefined" && source instanceof Blob;
}

/**
 * Cuts `source` into pieces of exactly `chunkSize` bytes (default {@link CHUNK_SIZE}) but the
 * last, which is shorter (nothing at all for an empty source), whatever sizes the source
 * yields: small pieces are gathered, large ones split. A `Blob` is read one slice at a time,
 * so memory stays bounded by one chunk (plus one piece of an iterable source). Yielded pieces
 * are never reused by this generator, but a piece may be a view into a piece of the source.
 * Throws an `Internal` error when the source yields something other than a `Uint8Array`.
 */
export async function* sliceIntoChunks(
  source: UploadSource,
  chunkSize: number = CHUNK_SIZE,
): AsyncGenerator<Uint8Array, void, undefined> {
  if (!Number.isSafeInteger(chunkSize) || chunkSize <= 0) {
    throw internalError("the chunk size must be a positive integer");
  }
  if (isBlob(source)) {
    for (let start = 0; start < source.size; start += chunkSize) {
      yield new Uint8Array(await source.slice(start, Math.min(source.size, start + chunkSize)).arrayBuffer());
    }
    return;
  }
  let buffer: Uint8Array | undefined;
  let filled = 0;
  for await (const piece of source) {
    if (!(piece instanceof Uint8Array)) {
      throw internalError("the upload source yielded something other than bytes");
    }
    let offset = 0;
    while (offset < piece.length) {
      if (filled === 0 && piece.length - offset >= chunkSize) {
        // A whole chunk of the piece: handed on without a copy.
        yield piece.subarray(offset, offset + chunkSize);
        offset += chunkSize;
        continue;
      }
      buffer ??= new Uint8Array(chunkSize);
      const take = Math.min(chunkSize - filled, piece.length - offset);
      buffer.set(piece.subarray(offset, offset + take), filled);
      filled += take;
      offset += take;
      if (filled === chunkSize) {
        yield buffer;
        buffer = undefined;
        filled = 0;
      }
    }
  }
  if (buffer !== undefined && filled > 0) {
    yield buffer.slice(0, filled);
  }
}
