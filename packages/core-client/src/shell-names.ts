// The desktop shell's file name rules (`apps/desktop/src-tauri/src/attachment_names.rs`),
// mirrored for `MemoryTransport`'s `importPath`, `saveAs` and `open`: the name an imported
// file gets, the MIME type its extension suggests, the name a stored attachment gets on disk,
// and the files "Open" refuses to hand to the system.

import { MAX_NAME_BYTES } from "./attachments";
import type { AttachmentId } from "./generated/AttachmentId";

/** What an imported file is called when its file name leaves nothing usable. */
const FALLBACK_NAME = "file";
/** An extension up to this many bytes (dot included) survives shortening. */
const KEPT_EXTENSION_BYTES = 16;
const UNKNOWN_MIME = "application/octet-stream";

/** Extension (lowercase, no dot) to MIME type: the shell's deliberately small table. */
const MIME_TYPES: Readonly<Record<string, string>> = {
  txt: "text/plain",
  md: "text/markdown",
  markdown: "text/markdown",
  csv: "text/csv",
  html: "text/html",
  htm: "text/html",
  css: "text/css",
  js: "text/javascript",
  json: "application/json",
  xml: "application/xml",
  pdf: "application/pdf",
  rtf: "application/rtf",
  epub: "application/epub+zip",
  doc: "application/msword",
  docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  xls: "application/vnd.ms-excel",
  xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  ppt: "application/vnd.ms-powerpoint",
  pptx: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  odt: "application/vnd.oasis.opendocument.text",
  ods: "application/vnd.oasis.opendocument.spreadsheet",
  odp: "application/vnd.oasis.opendocument.presentation",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  avif: "image/avif",
  bmp: "image/bmp",
  ico: "image/vnd.microsoft.icon",
  svg: "image/svg+xml",
  tif: "image/tiff",
  tiff: "image/tiff",
  heic: "image/heic",
  mp3: "audio/mpeg",
  wav: "audio/wav",
  ogg: "audio/ogg",
  oga: "audio/ogg",
  flac: "audio/flac",
  m4a: "audio/mp4",
  mp4: "video/mp4",
  m4v: "video/mp4",
  webm: "video/webm",
  ogv: "video/ogg",
  mov: "video/quicktime",
  avi: "video/x-msvideo",
  mkv: "video/x-matroska",
  zip: "application/zip",
  gz: "application/gzip",
  tar: "application/x-tar",
  "7z": "application/x-7z-compressed",
  rar: "application/vnd.rar",
  woff: "font/woff",
  woff2: "font/woff2",
  ttf: "font/ttf",
  otf: "font/otf",
};

/** Extensions of files a desktop "open" would run, install or load: `attachment_open` refuses them. */
const RUNS_CODE = new Set([
  "exe", "com", "scr", "pif", "bat", "cmd", "msi", "msp", "mst", "ps1", "ps1xml", "ps2", "ps2xml",
  "psc1", "psc2", "psd1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "wsc", "hta", "msc",
  "cpl", "inf", "reg", "dll", "sys", "jar", "py", "pyw", "msix", "msixbundle", "appx",
  "appxbundle", "appinstaller", "gadget", "application", "appref-ms", "lnk", "url", "scf",
  "library-ms", "search-ms", "sh", "bash", "zsh", "command", "terminal", "workflow", "app",
  "desktop", "run", "appimage",
]);

const utf8 = new TextEncoder();

function byteLength(text: string): number {
  return utf8.encode(text).length;
}

/** The longest prefix of `text` that fits `max` UTF-8 bytes, never cutting a character. */
function cut(text: string, max: number): string {
  let out = "";
  for (const char of text) {
    if (byteLength(out + char) > max) {
      break;
    }
    out += char;
  }
  return out;
}

/** `name` cut to at most `max` bytes, keeping a short extension by cutting the part before it. */
function capBytes(name: string, max: number): string {
  if (byteLength(name) <= max) {
    return name;
  }
  const dot = name.lastIndexOf(".");
  if (dot > 0) {
    const extension = name.slice(dot);
    if (byteLength(extension) <= KEPT_EXTENSION_BYTES) {
      return `${cut(name.slice(0, dot), max - byteLength(extension))}${extension}`;
    }
  }
  return cut(name, max);
}

/** The lowercase extension of a name, without the dot; `undefined` when there is none. */
function extension(name: string): string | undefined {
  const dot = name.lastIndexOf(".");
  const found = dot < 0 ? "" : name.slice(dot + 1);
  return found === "" ? undefined : found.toLowerCase();
}

/**
 * The name an imported file gets (`display_name`): control characters, `/` and `\` removed,
 * at most 255 bytes (a short extension kept), `file` when nothing usable is left.
 */
export function importName(fileName: string): string {
  const name = capBytes(fileName.replace(/[\p{Cc}/\\]/gu, ""), MAX_NAME_BYTES);
  return name === "" || name === "." || name === ".." ? FALLBACK_NAME : name;
}

/** The MIME type for a file name, from its extension (`mime_for_name`); `application/octet-stream` when unknown. */
export function mimeForName(name: string): string {
  const found = extension(name);
  return found !== undefined && Object.hasOwn(MIME_TYPES, found) ? (MIME_TYPES[found] ?? UNKNOWN_MIME) : UNKNOWN_MIME;
}

/** Whether Windows would read the name as a device (`CON`, `COM1.txt`, `lpt9 .log`, ...). */
function isDeviceName(name: string): boolean {
  const stem = (name.split(".")[0] ?? "").replace(/ +$/u, "").toUpperCase();
  return /^(?:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|(?:COM|LPT)[0-9¹²³])$/u.test(stem);
}

/**
 * A stored attachment name as the last component of a path (`path_safe_name`): control,
 * separator and invisible format characters removed, `< > : " | ? *` turned into `_`,
 * trailing dots and spaces dropped, Windows device names prefixed with `_`, at most 255
 * bytes; the attachment id when nothing is left.
 */
export function pathSafeName(stored: string, attachmentId: AttachmentId): string {
  return sanitisedName(stored) ?? attachmentId;
}

/**
 * The rules of {@link pathSafeName} without its fallback (`sanitised_name`): `undefined` when
 * nothing is left of the name. For a name that is not stored yet, such as the one an image
 * downloaded from a link is given.
 */
export function sanitisedName(name: string): string | undefined {
  const cleaned = name
    .replace(/[\p{Cc}/\\\u00ad\u061c\u180e\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u206f\ufeff]/gu, "")
    .replace(/[<>:"|?*]/gu, "_");
  const trimEnd = (text: string): string => text.replace(/[. ]+$/u, "");
  let safe = trimEnd(capBytes(trimEnd(cleaned), MAX_NAME_BYTES));
  if (safe === "") {
    return undefined;
  }
  if (isDeviceName(safe)) {
    safe = trimEnd(capBytes(`_${safe}`, MAX_NAME_BYTES));
  }
  return safe;
}

/** Whether the system would run a file with this name when it is opened (`runs_code`). */
export function runsCode(name: string): boolean {
  const found = extension(name);
  return found !== undefined && RUNS_CODE.has(found);
}
