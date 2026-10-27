// File names and the layout of an export (ADR-0036, `docs/architecture/import-export.md`, 2.2),
// the in-memory twin of `kb_export::layout`: names made safe for Windows, macOS and Linux,
// de-duplicated per folder (ASCII case-insensitive), the pages laid out the way Notion lays
// them out (a page is `Title.md` and its children, attachments and whiteboards are in the
// folder `Title/` next to it), the single-file rule, and relative links between the files.
// Pure functions; `./memory-export` feeds them the pages of one export session.

import type { AttachmentId } from "./generated/AttachmentId";
import type { PageId } from "./generated/PageId";
import type { ExportFormat } from "./generated/ExportFormat";

/** The stem of a name is cut to this many characters (code points). */
export const MAX_STEM_CHARS = 100;
/** What an empty name becomes. */
export const UNTITLED = "Untitled";

/** Device names Windows reserves, with or without an extension. */
const RESERVED = new Set([
  "CON",
  "PRN",
  "AUX",
  "NUL",
  ...Array.from({ length: 9 }, (_, index) => `COM${String(index + 1)}`),
  ...Array.from({ length: 9 }, (_, index) => `LPT${String(index + 1)}`),
]);

/**
 * `safe_file_name` for a name without an extension (a page title): every character of
 * `<>:"/\|?*` and every control character becomes a space, runs of white space one space, leading
 * and trailing spaces and trailing dots go, a reserved device name gets a `_` appended, the result
 * is cut to {@link MAX_STEM_CHARS} characters, and an empty one is `Untitled`. Emoji stay.
 */
export function safeStem(name: string): string {
  let text = name
    .replace(/[<>:"/\\|?*\p{Cc}]/gu, " ")
    .replace(/\s+/gu, " ")
    .trim();
  text = text.replace(/[. ]+$/u, "");
  if (text === "") {
    return UNTITLED;
  }
  const dot = text.indexOf(".");
  const base = dot < 0 ? text : text.slice(0, dot);
  if (RESERVED.has(base.toUpperCase())) {
    text = `${base}_${dot < 0 ? "" : text.slice(dot)}`;
  }
  const chars = Array.from(text);
  if (chars.length > MAX_STEM_CHARS) {
    text = chars.slice(0, MAX_STEM_CHARS).join("").replace(/[. ]+$/u, "");
  }
  return text === "" ? UNTITLED : text;
}

/** An extension of a file name: a dot and 1 to 16 letters or digits at the end. */
const EXTENSION = /\.[A-Za-z0-9]{1,16}$/u;

/** A file name split at its extension (the extension includes its dot; empty when there is none). */
export function splitExtension(name: string): { stem: string; ext: string } {
  const ext = EXTENSION.exec(name)?.[0] ?? "";
  const stem = name.slice(0, name.length - ext.length);
  return stem === "" ? { stem: name, ext: "" } : { stem, ext };
}

/**
 * `safe_file_name` for a name that may carry an extension (an attachment): the stem is made safe
 * and cut, the extension is kept (and made safe too).
 */
export function safeFileName(name: string): string {
  const { stem, ext } = splitExtension(name.trim());
  const safeExt = ext.replace(/[^A-Za-z0-9.]/gu, "");
  return `${safeStem(stem)}${safeExt}`;
}

/** The printable characters a link segment percent-encodes (ASCII controls are encoded too). */
const SEGMENT_ESCAPES = " \"#%'()<>?[\\]^`{|}";

/** One link segment, percent-encoded; non-ASCII stays as it is (UTF-8). */
export function encodeSegment(segment: string): string {
  let encoded = "";
  for (const char of segment) {
    const code = char.charCodeAt(0);
    encoded +=
      code <= 0x1f || code === 0x7f || SEGMENT_ESCAPES.includes(char)
        ? `%${code.toString(16).toUpperCase().padStart(2, "0")}`
        : char;
  }
  return encoded;
}

/**
 * `relative_href(from_file, to_file)`: the path of `toFile` relative to the folder of `fromFile`
 * (`../` as needed), every segment percent-encoded unless `encode` is false (front matter lists
 * file paths as they are).
 */
export function relativeHref(fromFile: string, toFile: string, encode = true): string {
  const from = fromFile.split("/").slice(0, -1);
  const to = toFile.split("/");
  let common = 0;
  while (common < from.length && common < to.length - 1 && from[common] === to[common]) {
    common += 1;
  }
  const segments = [...Array.from({ length: from.length - common }, () => ".."), ...to.slice(common)];
  return segments.map((segment) => (encode ? encodeSegment(segment) : segment)).join("/");
}

/** A page of the export, in tree order. */
export interface LayoutPage {
  id: PageId;
  /** The parent when it is in the export; `undefined` for the root(s). */
  parentId: PageId | undefined;
  title: string;
  /** The attachments the page uses, in order, without repeats (all of them, available or not). */
  attachments: readonly AttachmentId[];
  /** How many whiteboard blocks the page has. */
  whiteboards: number;
}

/** What the layout needs to know about an attachment. */
export interface LayoutAttachment {
  /** The stored file name. */
  name: string;
  /** Its bytes are on this device. */
  available: boolean;
}

/** The result of {@link layoutExport}. */
export interface ExportLayout {
  /** The path of every page's file. */
  pagePaths: Map<PageId, string>;
  /** The path of every available attachment, once (in the folder of the first page that uses it). */
  attachmentPaths: Map<AttachmentId, string>;
  /** The attachments placed with each page (first use), in order. */
  placed: Map<PageId, AttachmentId[]>;
  /** The paths of each page's whiteboards, in document order. */
  whiteboardPaths: Map<PageId, string[]>;
  /** One text file and nothing else. */
  singleFile: boolean;
  /** The name to offer in the save dialog: the page's file, or `<export name>.zip`. */
  suggestedName: string;
}

/** What {@link layoutExport} takes. */
export interface LayoutInput {
  /** The pages in tree order (parents before children). */
  pages: readonly LayoutPage[];
  /** Attachments by id; ids that are not here are unknown or deleted (no file). */
  attachments: ReadonlyMap<AttachmentId, LayoutAttachment>;
  format: ExportFormat;
  /** The root page's title, or the space's name. */
  exportName: string;
}

/** The file extension of a page for `format`. */
export function pageExtension(format: ExportFormat): string {
  return format === "markdown" ? ".md" : ".html";
}

/**
 * The layout of an export (2.2): names made safe, de-duplicated per folder (pages first in tree
 * order, then attachments, then whiteboards; ASCII case-insensitive: the second `Notes.md` is
 * `Notes (2).md` and its folder `Notes (2)/`), shared attachments written once in the folder of the
 * first page (tree order) that uses them, and the single-file rule (exactly one page, no
 * available attachment and no whiteboard). A top-level page is at the top level (no root folder).
 */
export function layoutExport(input: LayoutInput): ExportLayout {
  const ext = pageExtension(input.format);
  const placed = new Map<PageId, AttachmentId[]>();
  const taken = new Set<AttachmentId>();
  for (const page of input.pages) {
    const mine: AttachmentId[] = [];
    for (const attachmentId of page.attachments) {
      if (!taken.has(attachmentId) && input.attachments.get(attachmentId)?.available === true) {
        taken.add(attachmentId);
        mine.push(attachmentId);
      }
    }
    placed.set(page.id, mine);
  }
  const children = new Map<PageId | undefined, LayoutPage[]>();
  for (const page of input.pages) {
    const siblings = children.get(page.parentId) ?? [];
    siblings.push(page);
    children.set(page.parentId, siblings);
  }
  const layout: ExportLayout = {
    pagePaths: new Map(),
    attachmentPaths: new Map(),
    placed,
    whiteboardPaths: new Map(),
    singleFile: false,
    suggestedName: "",
  };
  const onlyPage = input.pages.length === 1 ? input.pages[0] : undefined;
  if (onlyPage !== undefined && taken.size === 0 && onlyPage.whiteboards === 0) {
    const path = `${safeStem(onlyPage.title)}${ext}`;
    layout.pagePaths.set(onlyPage.id, path);
    layout.singleFile = true;
    layout.suggestedName = path;
    return layout;
  }
  layout.suggestedName = `${safeStem(input.exportName)}.zip`;

  const hasFolder = (page: LayoutPage): boolean =>
    (children.get(page.id)?.length ?? 0) > 0 || (placed.get(page.id)?.length ?? 0) > 0 || page.whiteboards > 0;
  const folderOf = new Map<PageId, string>();

  // `dir` is the folder the pages' children, attachments and whiteboards go in ('' at the top).
  const fill = (parent: LayoutPage | undefined, dir: string): void => {
    const used = new Set<string>();
    const claim = (name: string): void => {
      used.add(name.toLowerCase());
    };
    for (const page of children.get(parent?.id) ?? []) {
      const stem = safeStem(page.title);
      const folder = hasFolder(page);
      for (let n = 1; ; n += 1) {
        const base = n === 1 ? stem : `${stem} (${String(n)})`;
        if (used.has(`${base}${ext}`.toLowerCase()) || (folder && used.has(base.toLowerCase()))) {
          continue;
        }
        claim(`${base}${ext}`);
        layout.pagePaths.set(page.id, `${dir}${base}${ext}`);
        if (folder) {
          claim(base);
          folderOf.set(page.id, `${dir}${base}/`);
        }
        break;
      }
    }
    if (parent !== undefined) {
      for (const attachmentId of placed.get(parent.id) ?? []) {
        const { stem, ext: fileExt } = splitExtension(safeFileName(input.attachments.get(attachmentId)?.name ?? ""));
        for (let n = 1; ; n += 1) {
          const name = `${n === 1 ? stem : `${stem} (${String(n)})`}${fileExt}`;
          if (used.has(name.toLowerCase())) {
            continue;
          }
          claim(name);
          layout.attachmentPaths.set(attachmentId, `${dir}${name}`);
          break;
        }
      }
      const boards: string[] = [];
      for (let board = 0; board < parent.whiteboards; board += 1) {
        for (let n = 1; ; n += 1) {
          const name = `${n === 1 ? "Whiteboard" : `Whiteboard (${String(n)})`}.excalidraw`;
          if (used.has(name.toLowerCase())) {
            continue;
          }
          claim(name);
          boards.push(`${dir}${name}`);
          break;
        }
      }
      layout.whiteboardPaths.set(parent.id, boards);
    }
    for (const page of children.get(parent?.id) ?? []) {
      const folder = folderOf.get(page.id);
      if (folder !== undefined) {
        fill(page, folder);
      }
    }
  };
  fill(undefined, "");
  return layout;
}
