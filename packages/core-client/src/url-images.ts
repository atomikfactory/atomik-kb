// "Image from URL" (ADR-0028, item 7) on the TypeScript side: the rules the desktop shell's
// `attachment_import_url` keeps (`apps/desktop/src-tauri/src/url_import.rs`), mirrored for
// `MemoryTransport` and for the UI's own early check of a link. The link must be `https`
// (no credentials, at most 2048 characters); the type of the picture comes from its bytes,
// never from the server (SVG only when the server says `image/svg+xml` and the body is an SVG
// document); the attachment is named after the last segment of the link's path, with the
// extension of the sniffed type. Failures are `InvalidInput` for the field `url`, with
// `details.reason` `status` (and `details.status`), `network`, `timeout`, `redirect` or
// `notImage`; no message ever repeats the link.

import { CoreError } from "./errors";
import { sanitisedName } from "./shell-names";
import { invalid } from "./validate";

/** The longest link accepted, in characters (the shell's `MAX_IMAGE_URL_CHARS`). */
export const MAX_IMAGE_URL_CHARS = 2048;

/** The image formats a link may lead to, as the bytes say. */
export type ImageKind = "png" | "jpeg" | "gif" | "webp" | "bmp" | "avif" | "svg";

/** The values of `details.reason` of a failed download (ADR-0028). */
export type UrlFailureReason = "status" | "network" | "timeout" | "redirect" | "notImage";

/** What the messages say; none of them names the link. */
export const URL_MESSAGES = {
  notImage: "the link is not an image",
  network: "the server could not be reached",
  timeout: "the server took too long to answer",
  redirect: "the link leads somewhere that is not allowed",
  tooLarge: "the image is larger than the app can show",
} as const;

/**
 * `InvalidInput` for the field `url` with `details.reason` (and the HTTP code for `status`), the
 * shell's shape.
 */
export function urlFailure(reason: UrlFailureReason, message: string, status?: number): CoreError {
  const details: { field: string; reason: UrlFailureReason; status?: number } = { field: "url", reason };
  if (status !== undefined) {
    details.status = status;
  }
  return new CoreError({ code: "InvalidInput", message, details });
}

/** The error for a status that is not a success: `status`, with the code in the message and in `details.status`. */
export function statusFailure(status: number): CoreError {
  return urlFailure("status", `the server answered with an error (HTTP ${String(status)})`, status);
}

/**
 * The link the person gave, checked like the shell does: surrounding white space dropped, not
 * empty, at most {@link MAX_IMAGE_URL_CHARS} characters, a well-formed absolute address with a host,
 * the scheme `https`, no user name or password. Throws `InvalidInput` for the field `url`
 * (messages without the link) and returns the parsed address.
 */
export function parseImageUrl(text: string): URL {
  const trimmed = text.trim();
  if (trimmed === "") {
    throw invalid("url", "the link is empty");
  }
  if (Array.from(trimmed).length > MAX_IMAGE_URL_CHARS) {
    throw invalid("url", `the link may be at most ${String(MAX_IMAGE_URL_CHARS)} characters`);
  }
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    throw invalid("url", "the link is not a valid web address");
  }
  if (url.protocol !== "https:") {
    throw invalid("url", "only https links can be used: the link must start with https://");
  }
  if (url.username !== "" || url.password !== "") {
    throw invalid("url", "the link must not contain a user name or a password");
  }
  if (url.hostname === "") {
    throw invalid("url", "the link has no host");
  }
  return url;
}

/** The extension a file of this kind is given (lowercase, no dot). */
export function imageExtension(kind: ImageKind): string {
  return kind === "jpeg" ? "jpg" : kind;
}

/** The MIME type stored with the attachment. */
export function imageMime(kind: ImageKind): string {
  return kind === "svg" ? "image/svg+xml" : `image/${kind}`;
}

/** The extensions that already name a kind, so that a name keeps its own. */
const NAMING_EXTENSIONS: Readonly<Record<ImageKind, readonly string[]>> = {
  png: ["png"],
  jpeg: ["jpg", "jpeg", "jpe", "jfif"],
  gif: ["gif"],
  webp: ["webp"],
  bmp: ["bmp"],
  avif: ["avif"],
  svg: ["svg"],
};

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/** The DIB header sizes a BMP file has (core, info, v2 to v5), at offset 14. */
const BMP_HEADER_SIZES = new Set([12, 40, 52, 56, 64, 108, 124]);

function startsWith(bytes: Uint8Array, prefix: readonly number[], at = 0): boolean {
  return bytes.length >= at + prefix.length && prefix.every((byte, index) => bytes[at + index] === byte);
}

function startsWithText(bytes: Uint8Array, text: string, at = 0): boolean {
  return startsWith(
    bytes,
    Array.from(text, (char) => char.charCodeAt(0)),
    at,
  );
}

function isBmp(bytes: Uint8Array): boolean {
  if (!startsWithText(bytes, "BM") || bytes.length < 18) {
    return false;
  }
  const size = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(14, true);
  return BMP_HEADER_SIZES.has(size);
}

function isAvif(bytes: Uint8Array): boolean {
  return startsWithText(bytes, "ftyp", 4) && (startsWithText(bytes, "avif", 8) || startsWithText(bytes, "avis", 8));
}

/** The kind of image the bytes are, from their magic numbers alone (the formats with a signature). */
export function sniffBinaryImage(bytes: Uint8Array): ImageKind | undefined {
  if (startsWith(bytes, PNG_SIGNATURE)) {
    return "png";
  }
  if (startsWith(bytes, [0xff, 0xd8, 0xff])) {
    return "jpeg";
  }
  if (startsWithText(bytes, "GIF87a") || startsWithText(bytes, "GIF89a")) {
    return "gif";
  }
  if (startsWithText(bytes, "RIFF") && startsWithText(bytes, "WEBP", 8)) {
    return "webp";
  }
  if (isBmp(bytes)) {
    return "bmp";
  }
  if (isAvif(bytes)) {
    return "avif";
  }
  return undefined;
}

/** Whether the server's `Content-Type` is `image/svg+xml` (any case, parameters ignored). */
export function isSvgContentType(contentType: string | null | undefined): boolean {
  return (contentType?.split(";")[0] ?? "").trim().toLowerCase() === "image/svg+xml";
}

const ASCII_WHITESPACE = new Set([0x09, 0x0a, 0x0c, 0x0d, 0x20]);

/** Index of `marker` in `bytes` from `from`, or -1. */
function indexOfText(bytes: Uint8Array, marker: string, from: number): number {
  const codes = Array.from(marker, (char) => char.charCodeAt(0));
  for (let at = from; at + codes.length <= bytes.length; at += 1) {
    if (codes.every((code, index) => bytes[at + index] === code)) {
      return at;
    }
  }
  return -1;
}

/**
 * Whether the bytes are an SVG document: after an optional UTF-8 byte order mark, white space,
 * processing instructions (the XML declaration), comments and a document type declaration (with
 * an internal subset), the first thing is an `<svg` element. Case-sensitive, as XML is.
 */
export function isSvgDocument(bytes: Uint8Array): boolean {
  let at = startsWith(bytes, [0xef, 0xbb, 0xbf]) ? 3 : 0;
  for (;;) {
    while (at < bytes.length && ASCII_WHITESPACE.has(bytes[at] ?? 0)) {
      at += 1;
    }
    if (startsWithText(bytes, "<?", at)) {
      const end = indexOfText(bytes, "?>", at + 2);
      if (end < 0) {
        return false;
      }
      at = end + 2;
    } else if (startsWithText(bytes, "<!--", at)) {
      const end = indexOfText(bytes, "-->", at + 4);
      if (end < 0) {
        return false;
      }
      at = end + 3;
    } else if (startsWithText(bytes, "<!DOCTYPE", at)) {
      let depth = 0;
      let cursor = at + 9;
      let closed = false;
      for (; cursor < bytes.length; cursor += 1) {
        const byte = bytes[cursor];
        if (byte === 0x5b) {
          depth += 1;
        } else if (byte === 0x5d) {
          depth = Math.max(0, depth - 1);
        } else if (byte === 0x3e && depth === 0) {
          closed = true;
          break;
        }
      }
      if (!closed) {
        return false;
      }
      at = cursor + 1;
    } else {
      if (!startsWithText(bytes, "<svg", at)) {
        return false;
      }
      const after = bytes[at + 4];
      return after !== undefined && (after === 0x3e || after === 0x2f || ASCII_WHITESPACE.has(after));
    }
  }
}

/**
 * The kind of image the bytes are, from their magic numbers alone; `undefined` for anything
 * else. SVG counts only when `contentType` (the server's header) is `image/svg+xml` **and** the
 * body is an SVG document, so a server cannot make a page pass for a picture and a mislabelled
 * picture is still read for what it is.
 */
export function sniffImage(bytes: Uint8Array, contentType?: string | null): ImageKind | undefined {
  const binary = sniffBinaryImage(bytes);
  if (binary !== undefined) {
    return binary;
  }
  return isSvgContentType(contentType) && isSvgDocument(bytes) ? "svg" : undefined;
}

/** A short word with a letter in it (`.php`, `.v2`): the extension a script or a version leaves in a path. */
function isReplaceableExtension(extension: string): boolean {
  return /^(?=[0-9a-z]{1,5}$)[0-9a-z]*[a-z][0-9a-z]*$/iu.test(extension);
}

/** The text with its `%XX` escapes decoded as UTF-8; a sequence that is not valid UTF-8 loses the characters it breaks. */
function percentDecode(text: string): string {
  const encoded = new TextEncoder().encode(text);
  const bytes: number[] = [];
  for (let at = 0; at < encoded.length; at += 1) {
    const byte = encoded[at] ?? 0;
    const hex = byte === 0x25 ? String.fromCharCode(encoded[at + 1] ?? 0, encoded[at + 2] ?? 0) : "";
    if (/^[0-9a-f]{2}$/iu.test(hex)) {
      bytes.push(Number.parseInt(hex, 16));
      at += 2;
    } else {
      bytes.push(byte);
    }
  }
  return new TextDecoder("utf-8", { ignoreBOM: true }).decode(Uint8Array.from(bytes)).replaceAll("�", "");
}

/**
 * The name the attachment gets: the last segment of the link's path, percent-decoded, with the
 * extension of the sniffed `kind` (kept when it already names the kind, in whatever case;
 * replaced when it is a short word with a letter in it, as `.php` or `.v2`; else added), then
 * sanitised like the name of a saved file (separators and control and invisible characters
 * removed, `< > : " | ? *` turned into `_`, device names prefixed, at most 255 bytes).
 * `image.<ext>` when nothing is left of the segment.
 */
export function imageName(url: URL, kind: ImageKind): string {
  const extension = imageExtension(kind);
  const segment = url.pathname.split("/").at(-1) ?? "";
  const decoded = percentDecode(segment).trim();
  const dot = decoded.lastIndexOf(".");
  let candidate: string;
  if (dot >= 0) {
    const stem = decoded.slice(0, dot);
    const existing = decoded.slice(dot + 1);
    if (stem.trim() === "") {
      candidate = ""; // `.png`, `.hidden`: an extension and no stem is not a name
    } else if (NAMING_EXTENSIONS[kind].includes(existing.toLowerCase())) {
      candidate = decoded;
    } else if (isReplaceableExtension(existing)) {
      candidate = `${stem.trimEnd()}.${extension}`;
    } else {
      candidate = `${decoded}.${extension}`;
    }
  } else {
    candidate = decoded === "" ? "" : `${decoded}.${extension}`;
  }
  const name = sanitisedName(candidate);
  // Nothing but the extension is left (the stem was all invisible characters).
  return name === undefined || name.startsWith(".") ? `image.${extension}` : name;
}
