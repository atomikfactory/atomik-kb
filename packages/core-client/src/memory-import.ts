// The in-memory twin of the core's import sessions (ADR-0036, `docs/architecture/import-export.md`,
// sections 3 and 7): `ImportBegin`, `ImportAddFile`, `ImportPlan`, `ImportAddAttachment`,
// `ImportCommit` and `ImportAbort` over the mirror's structure, properties, documents and
// attachments, with reduced fidelity (the plan is `./memory-import-plan`).
//
// Same as the core: one session at a time (a new one drops the old; ids count up from 1), the
// limits on paths and texts, files first and the plan once, the shell's uploads reported one by one
// (the attachment must be a complete one of the space), and the commit in batches: the first call
// matches or creates the properties (by name, case-folded; reused whatever their kind, missing
// options added, the 100-property limit turning the rest into warnings) and creates every page in
// the structure at once (appended after the destination's children, one `StructureChanged`), so
// every link can resolve; every call then writes up to `maxPages` documents (links, attachments
// and ids resolved) and the page's values (coerced to each property's kind, `InvalidValue` when
// they do not fit), and emits `ImportProgress`; the last answers the report (planning, resolving
// and coercion warnings in that order, at most 500) and closes the session. An abort leaves the
// pages already written.
//
// The events of a call are the ones the writes emit, deduplicated by the transport (`collect`),
// then the progress event.
//
// `ImportBeginSpaces` (ADR-0039, `docs/architecture/appflowy-import.md`, 5 and 8) opens a session whose
// destination is new spaces: `ImportPlan` creates the spaces (one per space root of the plan, an AppFlowy
// backup's spaces; one named after the source for a plan without), in order, and the space roots are not
// pages (their children are the root pages of their space); `ImportPlanInfo` lists the spaces, the space of
// each needed file and the images kept on a server (`remoteFiles`, never downloaded here: their blocks
// become links with a `remoteImage` warning unless the shell reports an attachment under their key). A
// commit creates properties per space (for the properties its pages use), pages per space in one change
// each, and resolves links across the session's spaces; `ImportAbort`, or a commit that fails, deletes the
// session's spaces that have no page, with their attachments. The report lists `spaceIds`. On an
// `ImportBegin` session the space roots are ordinary pages (containers).

import type { MemoryAttachments } from "./attachment-mirror";
import type { MemoryContent } from "./content";
import { CoreError } from "./errors";
import { MAX_FORMULA_CHARS, formulaReferencedIds, formulaToStored } from "./formula";
import type { AttachmentId } from "./generated/AttachmentId";
import type { Event } from "./generated/Event";
import type { ImportPlanInfo } from "./generated/ImportPlanInfo";
import type { ImportRemoteFile } from "./generated/ImportRemoteFile";
import type { ImportSpaceInfo } from "./generated/ImportSpaceInfo";
import type { ImportProgress } from "./generated/ImportProgress";
import type { ImportReport } from "./generated/ImportReport";
import type { ImportWarning } from "./generated/ImportWarning";
import type { ImportWarningCode } from "./generated/ImportWarningCode";
import type { PageId } from "./generated/PageId";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyValue } from "./generated/PropertyValue";
import type { SpaceId } from "./generated/SpaceId";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { FORMULA_TITLE_LABEL, formulaSchemaOf } from "./memory-formula";
import { MAX_BINARY_PATHS, MAX_PLAN_PAGES, planImport } from "./memory-import-plan";
import type { Cell, ImportPlan, PlannedPage, PlannedProperty } from "./memory-import-plan";
import { parseBool, parseDate, parseNumber } from "./memory-import-text";
import type { MemoryProperties } from "./memory-properties";
import { MAX_NAME_CHARS, OPTION_COLORS, TAGS_PROPERTY_ID, codePointLength, propertyKindHasOptions } from "./properties";
import { placeholderTakes } from "./structure";
import type { MemoryStructure } from "./structure";
import type { ImportSourceKind } from "./transport";
import { cover as validCover, coverY as validCoverY, icon as validIcon, invalid, spaceIcon as validSpaceIcon } from "./validate";

/** Most text files one import takes. */
export const MAX_IMPORT_FILES = 10_000;
/** Largest text file, in bytes. */
export const MAX_IMPORT_FILE_BYTES = 16 * 1024 * 1024;
/** Most bytes of text files in all. */
export const MAX_IMPORT_TOTAL_BYTES = 256 * 1024 * 1024;
/** Longest path, in bytes. */
export const MAX_IMPORT_PATH_BYTES = 1024;
/** Most warnings a report lists. */
export const MAX_REPORT_WARNINGS = 500;
/** Most pages one `ImportCommit` writes. */
const MAX_COMMIT_PAGES = 500;

const utf8 = new TextEncoder();

/** What the import mirror needs from its owner. */
export interface ImportDeps {
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Runs `run`, then delivers the events it caused once each (in order of first occurrence). */
  collect: <T>(run: () => T) => T;
  structure: MemoryStructure;
  properties: MemoryProperties;
  content: MemoryContent;
  attachments: MemoryAttachments;
  /** Deletes a space that has no page, with its attachments and everything else the space owns (`SpaceDelete`). */
  deleteEmptySpace: (spaceId: SpaceId) => void;
}

function limitExceeded(field: string, message: string, limit: number): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { field, limit } });
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/**
 * Whether `path` is a relative path of the kind the core accepts: `/`-separated, no empty, `.` or
 * `..` segment, no backslash, no NUL, no drive letter, at most 1,024 bytes.
 */
export function isSafeRelativePath(path: string): boolean {
  return (
    path !== "" &&
    utf8.encode(path).length <= MAX_IMPORT_PATH_BYTES &&
    !path.includes("\\") &&
    !path.includes("\0") &&
    !/^[A-Za-z]:/u.test(path) &&
    path.split("/").every((segment) => segment !== "" && segment !== "." && segment !== "..")
  );
}

/** A canned source of the in-memory import dialogs (`MemoryTransportOptions.importSources`). */
export interface MemoryImportSource {
  /** What the person picks in the dialog: some files, a folder, or a `.zip`. */
  kind: ImportSourceKind;
  /** The files by path relative to the source's root: text, or bytes (decoded when the file is a text file). */
  files: Readonly<Record<string, string | Uint8Array>>;
}

/** The extensions of the files the shell reads as text (the rest are binary paths). */
const TEXT_EXTENSIONS = [".md", ".markdown", ".txt", ".html", ".htm", ".csv", ".excalidraw"];

/** Whether the shell reads a file as text: the extensions above, and `atomik-export.json` at the root. */
export function isTextFile(path: string): boolean {
  const lower = path.toLowerCase();
  return path === "atomik-export.json" || TEXT_EXTENSIONS.some((extension) => lower.endsWith(extension));
}

/** Whether the shell skips an entry: a segment starting with `.`, `__MACOSX` or `node_modules`. */
export function isSkippedPath(path: string): boolean {
  return path.split("/").some((segment) => segment.startsWith(".") || segment === "__MACOSX" || segment === "node_modules");
}

/**
 * A text file's bytes as text: UTF-8 (a byte order mark removed), UTF-16 with a byte order mark
 * (either order), else UTF-8 with replacement characters, flagged `lossy` (`notUtf8`).
 */
export function decodeSourceText(bytes: Uint8Array): { text: string; lossy: boolean } {
  if (bytes[0] === 0xff && bytes[1] === 0xfe) {
    return { text: new TextDecoder("utf-16le").decode(bytes.subarray(2)), lossy: false };
  }
  if (bytes[0] === 0xfe && bytes[1] === 0xff) {
    return { text: new TextDecoder("utf-16be").decode(bytes.subarray(2)), lossy: false };
  }
  const body = bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf ? bytes.subarray(3) : bytes;
  try {
    return { text: new TextDecoder("utf-8", { fatal: true }).decode(body), lossy: false };
  } catch {
    return { text: new TextDecoder("utf-8").decode(body), lossy: true };
  }
}

function checkPath(path: string, field: string): void {
  if (path === "") {
    throw invalid(field, "must not be empty");
  }
  if (utf8.encode(path).length > MAX_IMPORT_PATH_BYTES) {
    throw limitExceeded(field, "the path is too long", MAX_IMPORT_PATH_BYTES);
  }
  if (!isSafeRelativePath(path)) {
    throw invalid(field, "must be a relative path of `/`-separated names (no empty, `.` or `..` segment, no backslash)");
  }
}

/** The node types that carry an `id` attribute (`fixtures/doc/block-schema-v1.json`, `idBearing`). */
const ID_BEARING = new Set([
  "paragraph",
  "heading",
  "bulletList",
  "orderedList",
  "listItem",
  "taskList",
  "taskItem",
  "blockquote",
  "codeBlock",
  "mathBlock",
  "horizontalRule",
  "details",
  "detailsSummary",
  "detailsContent",
  "callout",
  "pageRef",
  "pageLink",
  "image",
  "file",
  "gallery",
  "whiteboard",
  "table",
  "tableRow",
  "tableCell",
  "tableHeader",
  "columnList",
  "column",
  "tableOfContents",
  "breadcrumb",
  "pageList",
  "dateMention",
  "mathInline",
  "templateButton",
]);

type Obj = { [key: string]: JsonValue };

function isObj(value: JsonValue | undefined): value is Obj {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** A property of the plan as the space has it. */
interface Resolved {
  id: PropertyId;
  /** The property's current definition (options grow while values are coerced). */
  def: PropertyDef;
}

interface Session {
  id: number;
  /** The destination space of an `ImportBegin` session; `undefined` when the destination is new spaces. */
  spaceId: SpaceId | undefined;
  parentId: PageId | undefined;
  sourceName: string | undefined;
  files: Map<string, string>;
  bytes: number;
  plan: ImportPlan | undefined;
  /** The paths of the needed files and the keys of the remote files: what `ImportAddAttachment` may name. */
  needed: Set<string>;
  /** The space each of them is uploaded into. */
  fileSpaces: Map<string, SpaceId>;
  uploaded: Map<string, AttachmentId>;
  /** The spaces the session made at the plan (an `ImportBeginSpaces` session), in order. */
  made: SpaceId[];
  /** The space of each planned page (parallel to `plan.pages`). */
  pageSpaces: SpaceId[];
  // The commit:
  started: boolean;
  pageIds: PageId[];
  byKey: Map<number, PageId>;
  written: number;
  /** The planned properties as each space has them. */
  properties: Map<SpaceId, (Resolved | undefined)[]>;
  propertiesCreated: number;
  attachmentsUsed: Set<AttachmentId>;
  resolveWarnings: ImportWarning[];
  coerceWarnings: ImportWarning[];
  plannedTitles: Map<string, PageId> | undefined;
  /** The live pages of the space before the import, by exact title (what a link may find). */
  existingTitles: Map<string, PageId[]>;
}

/** The import sessions of one core. */
export class MemoryImport {
  readonly #deps: ImportDeps;
  #lastId = 0;
  #session: Session | undefined;

  constructor(deps: ImportDeps) {
    this.#deps = deps;
  }

  /** Drops the session (a lock does). */
  drop(): void {
    this.#session = undefined;
  }

  /**
   * `ImportBegin`: checks the space and the destination (a live page of the space), drops any
   * session and starts one; answers its id. An empty `sourceName` is none.
   */
  begin(spaceId: SpaceId, parentId: PageId | undefined, sourceName: string | undefined): number {
    const { structure } = this.#deps;
    if (!structure.hasSpace(spaceId)) {
      throw notFound("space not found");
    }
    if (parentId !== undefined) {
      const meta = structure.pageMeta(parentId);
      if (meta === undefined) {
        throw notFound("page not found");
      }
      if (meta.spaceId !== spaceId) {
        throw invalid("parentId", "the page belongs to another space");
      }
      if (meta.trashed) {
        throw invalid("parentId", "the parent page does not exist or is in the trash");
      }
    }
    return this.#open(spaceId, parentId, sourceName);
  }

  /**
   * `ImportBeginSpaces` (ADR-0039): drops any session and starts one whose destination is new spaces
   * (created by `ImportPlan`); answers its id. An empty `sourceName` is none.
   */
  beginSpaces(sourceName: string | undefined): number {
    return this.#open(undefined, undefined, sourceName);
  }

  #open(spaceId: SpaceId | undefined, parentId: PageId | undefined, sourceName: string | undefined): number {
    const old = this.#session;
    this.#session = undefined;
    if (old !== undefined) {
      this.#discard(old);
    }
    this.#lastId += 1;
    const name = sourceName?.trim();
    this.#session = {
      id: this.#lastId,
      spaceId,
      parentId,
      sourceName: name === undefined || name === "" ? undefined : name,
      files: new Map(),
      bytes: 0,
      plan: undefined,
      needed: new Set(),
      fileSpaces: new Map(),
      uploaded: new Map(),
      made: [],
      pageSpaces: [],
      started: false,
      pageIds: [],
      byKey: new Map(),
      written: 0,
      properties: new Map(),
      propertiesCreated: 0,
      attachmentsUsed: new Set(),
      resolveWarnings: [],
      coerceWarnings: [],
      plannedTitles: undefined,
      existingTitles: new Map(),
    };
    return this.#lastId;
  }

  /**
   * `ImportAddFile`: keeps one text file (a path given twice keeps the last). Paths are relative
   * (see {@link isSafeRelativePath}); at most 10,000 files, 16 MiB each, 256 MiB in all.
   */
  addFile(importId: number, path: string, text: string): void {
    const session = this.#require(importId);
    if (session.plan !== undefined) {
      throw invalid("importId", "the import is already planned; no more files can be added");
    }
    checkPath(path, "path");
    const size = utf8.encode(text).length;
    if (size > MAX_IMPORT_FILE_BYTES) {
      throw limitExceeded("text", "the file is too large", MAX_IMPORT_FILE_BYTES);
    }
    const replaced = session.files.get(path);
    const total = session.bytes - (replaced === undefined ? 0 : utf8.encode(replaced).length) + size;
    if (replaced === undefined && session.files.size >= MAX_IMPORT_FILES) {
      throw limitExceeded("path", "too many files", MAX_IMPORT_FILES);
    }
    if (total > MAX_IMPORT_TOTAL_BYTES) {
      throw limitExceeded("text", "the files are too large in all", MAX_IMPORT_TOTAL_BYTES);
    }
    session.files.set(path, text);
    session.bytes = total;
  }

  /**
   * `ImportPlan`: plans the import from the files given (and the paths of every other file of the
   * source), keeps the plan and drops the texts; answers the pages to create, the files they use and the
   * warnings found. On an `ImportBeginSpaces` session the spaces are created now (`SpacesChanged`): one
   * per space root of the plan, else one named after the source (or "Imported"); the space roots are not
   * pages, their children are the root pages of their space; the answer also lists the spaces and the
   * space each needed file goes into. The images the source keeps on a server are listed in both kinds.
   */
  plan(importId: number, binaryPaths: readonly string[]): ImportPlanInfo {
    const session = this.#require(importId);
    if (session.plan !== undefined) {
      throw invalid("importId", "the import is already planned");
    }
    if (binaryPaths.length > MAX_BINARY_PATHS) {
      throw limitExceeded("binaryPaths", "too many files", MAX_BINARY_PATHS);
    }
    for (const path of binaryPaths) {
      checkPath(path, "binaryPaths");
    }
    const spaces = session.spaceId === undefined;
    const raw = planImport({
      files: session.files,
      binaryPaths,
      // A source without spaces fills one new space: no container page named after it.
      sourceName: spaces ? undefined : session.sourceName,
      maxPages: MAX_PLAN_PAGES,
    });
    const plan = spaces ? this.#makeSpaces(session, raw) : this.#keepSpace(session, raw);
    session.plan = plan;
    session.files = new Map();
    session.bytes = 0;
    session.needed = new Set([...plan.neededFiles, ...plan.remoteFiles.map((file) => file.key)]);
    const info: ImportPlanInfo = { pages: plan.pages.length, neededFiles: [...plan.neededFiles] };
    if (plan.warnings.length > 0) {
      info.warnings = plan.warnings.map((warning) => ({ ...warning }));
    }
    if (spaces) {
      info.spaces = session.made.map((spaceId, at): ImportSpaceInfo => ({
        spaceId,
        name: this.#deps.structure.spaceList().find((space) => space.id === spaceId)?.name ?? "",
        pages: session.pageSpaces.filter((space) => space === session.made[at]).length,
      }));
      info.fileSpaces = plan.neededFiles.map((path) => session.fileSpaces.get(path) ?? session.made[0] ?? "");
    }
    if (plan.remoteFiles.length > 0) {
      info.remoteFiles = plan.remoteFiles.map((file): ImportRemoteFile => ({
        ...file,
        spaceId: session.fileSpaces.get(file.key) ?? "",
      }));
    }
    return info;
  }

  /** An `ImportBegin` session's plan as it is: every page and file belongs to the destination space. */
  #keepSpace(session: Session, plan: ImportPlan): ImportPlan {
    const space = session.spaceId ?? "";
    session.pageSpaces = plan.pages.map(() => space);
    for (const path of [...plan.neededFiles, ...plan.remoteFiles.map((file) => file.key)]) {
      session.fileSpaces.set(path, space);
    }
    return plan;
  }

  /**
   * The spaces of an `ImportBeginSpaces` session, created now, and the plan without the space roots: their
   * children become root pages (`parent: null`), every page is assigned the space of its root.
   */
  #makeSpaces(session: Session, raw: ImportPlan): ImportPlan {
    const roots = new Map(raw.spaceRoots.map((index, at) => [index, at]));
    const wanted =
      raw.spaceRoots.length === 0
        ? [{ name: session.sourceName ?? "Imported", icon: undefined as string | undefined }]
        : raw.spaceRoots.map((index) => ({ name: raw.pages[index]?.title ?? "", icon: raw.pages[index]?.icon }));
    session.made = this.#deps.collect(() => wanted.map((entry) => this.#makeSpace(entry.name, entry.icon)));
    // Pages parents first: a page is in the space of its parent, a root's child in the root's space.
    const ordinal: number[] = [];
    const kept = new Map<number, number>();
    const pages: PlannedPage[] = [];
    const pageSpaces: SpaceId[] = [];
    raw.pages.forEach((page, old) => {
      const root = roots.get(old);
      if (root !== undefined) {
        ordinal[old] = root;
        return;
      }
      const spaceNo = page.parent === null ? 0 : (ordinal[page.parent] ?? 0);
      ordinal[old] = spaceNo;
      kept.set(old, pages.length);
      const parent = page.parent === null || roots.has(page.parent) ? null : (kept.get(page.parent) ?? null);
      pages.push({ ...page, parent });
      pageSpaces.push(session.made[spaceNo] ?? session.made[0] ?? "");
    });
    session.pageSpaces = pageSpaces;
    const fileUsers = new Map<string, number>();
    for (const [file, old] of raw.fileUsers) {
      const index = kept.get(old);
      if (index !== undefined) {
        fileUsers.set(file, index);
      }
    }
    for (const path of [...raw.neededFiles, ...raw.remoteFiles.map((file) => file.key)]) {
      const user = fileUsers.get(path);
      session.fileSpaces.set(path, (user === undefined ? undefined : pageSpaces[user]) ?? session.made[0] ?? "");
    }
    return { ...raw, pages, spaceRoots: [], fileUsers };
  }

  /** One new space: the name as a space name (no control characters, 1 to 100 characters), the icon when it is valid. */
  #makeSpace(name: string, icon: string | undefined): SpaceId {
    const { structure } = this.#deps;
    const clean = Array.from(name.replace(/\p{Cc}/gu, " ").trim()).slice(0, 100).join("").trim();
    const spaceName = clean === "" ? "Imported" : clean;
    let valid: string | undefined;
    try {
      valid = validSpaceIcon(icon);
    } catch (error) {
      if (!(error instanceof CoreError)) {
        throw error;
      }
      valid = undefined;
    }
    return structure.spaceCreate(spaceName, valid).id;
  }

  /**
   * `ImportAddAttachment`: the shell uploaded a needed file (or downloaded a remote one: its key); the
   * attachment must be a complete one of the space the plan gave it (`NotFound` when unknown).
   */
  addAttachment(importId: number, path: string, attachmentId: AttachmentId): void {
    const session = this.#require(importId);
    if (session.plan === undefined) {
      throw invalid("importId", "the import is not planned yet");
    }
    if (!session.needed.has(path)) {
      throw invalid("path", "the file is not one the pages use");
    }
    const info = this.#deps.attachments.get(attachmentId);
    if (info.spaceId !== session.fileSpaces.get(path)) {
      throw invalid("attachmentId", "the attachment belongs to another space");
    }
    if (info.availability !== "Complete") {
      throw invalid("attachmentId", "the attachment is not complete on this device");
    }
    session.uploaded.set(path, info.attachmentId);
  }

  /**
   * `ImportAbort`: drops the session (nothing to do for one that is gone); the pages written stay, and
   * the spaces an `ImportBeginSpaces` session made that have no page are deleted with their attachments.
   */
  abort(importId: number): void {
    const session = this.#session;
    if (session?.id === importId) {
      this.#session = undefined;
      this.#discard(session);
    }
  }

  /** Deletes the spaces a session made that have no page (live or in the trash), with their attachments. */
  #discard(session: Session): void {
    const empty = session.made.filter((spaceId) => this.#deps.structure.spacePageIds(spaceId)?.length === 0);
    if (empty.length > 0) {
      this.#deps.collect(() => {
        for (const spaceId of empty) {
          this.#deps.deleteEmptySpace(spaceId);
        }
      });
    }
  }

  /**
   * `ImportCommit`: the first call writes the properties and creates every page; each call writes
   * up to `maxPages` documents and values, emits the events of the writes (once each) and then
   * `ImportProgress`; the call that finishes carries the report and closes the session. `maxPages`
   * is at least 1 and counts at most 500. A commit that fails ends an `ImportBeginSpaces` session as an
   * abort does.
   */
  commit(importId: number, maxPages: number): ImportProgress {
    const session = this.#require(importId);
    const plan = session.plan;
    if (plan === undefined) {
      throw invalid("importId", "the import is not planned yet");
    }
    if (!Number.isInteger(maxPages) || maxPages < 1) {
      throw invalid("maxPages", "must be at least 1");
    }
    const batch = Math.min(maxPages, MAX_COMMIT_PAGES);
    const total = plan.pages.length;
    try {
      this.#deps.collect(() => {
        if (!session.started) {
          session.started = true;
          this.#start(session, plan);
        }
        const stop = Math.min(total, session.written + batch);
        while (session.written < stop) {
          const index = session.written;
          const page = plan.pages[index];
          const pageId = session.pageIds[index];
          session.written += 1;
          if (page !== undefined && pageId !== undefined && pageId !== "") {
            this.#writePage(session, plan, index, page, pageId);
          }
        }
      });
    } catch (error) {
      if (session.made.length > 0) {
        this.#session = undefined;
        this.#discard(session);
      }
      throw error;
    }
    this.#deps.emit({ type: "ImportProgress", data: { importId, done: session.written, total } });
    if (session.written < total) {
      return { created: session.written, total, done: false };
    }
    this.#session = undefined;
    return { created: session.written, total, done: true, report: this.#report(session, plan) };
  }

  // ----- plumbing -------------------------------------------------------------------------

  #require(importId: number): Session {
    if (this.#session === undefined || this.#session.id !== importId) {
      throw notFound("import session not found");
    }
    return this.#session;
  }

  /** The spaces the pages of a session go to: its destination, or the ones it made. */
  #spacesOf(session: Session): SpaceId[] {
    return session.spaceId === undefined ? session.made : [session.spaceId];
  }

  #report(session: Session, plan: ImportPlan): ImportReport {
    const all = [...plan.warnings, ...session.resolveWarnings, ...session.coerceWarnings];
    const report: ImportReport = {
      rootPageIds: session.pageIds.filter((_, index) => plan.pages[index]?.parent === null),
      pages: plan.pages.length,
      attachments: session.attachmentsUsed.size,
      propertiesCreated: session.propertiesCreated,
      moreWarnings: Math.max(0, all.length - MAX_REPORT_WARNINGS),
    };
    if (all.length > 0) {
      report.warnings = all.slice(0, MAX_REPORT_WARNINGS);
    }
    if (session.spaceId === undefined) {
      report.spaceIds = [...session.made];
    }
    return report;
  }

  #warn(into: ImportWarning[], path: string, code: ImportWarningCode, detail?: string): void {
    into.push(detail === undefined ? { path, code } : { path, code, detail });
  }

  /**
   * The planned properties the pages of a space use: their values' properties, the ones their saved views
   * show or group by (`prop:<name>` markers), and the rollups over those relations.
   */
  #usedProperties(plan: ImportPlan, session: Session, space: SpaceId): Set<number> {
    const used = new Set<number>();
    const named = new Set<string>();
    const scan = (node: JsonValue): void => {
      if (!isObj(node)) {
        return;
      }
      if (node.type === "pageList" && isObj(node.attrs)) {
        const markers = [...(Array.isArray(node.attrs.properties) ? node.attrs.properties : []), node.attrs.groupBy];
        for (const marker of markers) {
          if (typeof marker === "string" && marker.startsWith("prop:")) {
            named.add(marker.slice(5).toLowerCase());
          }
        }
      }
      if (Array.isArray(node.content)) {
        node.content.forEach(scan);
      }
    };
    plan.pages.forEach((page, index) => {
      if (session.pageSpaces[index] === space) {
        page.values.forEach((value) => used.add(value.property));
        page.blocks.forEach(scan);
      }
    });
    plan.properties.forEach((property, index) => {
      if (named.has(property.name.toLowerCase())) {
        used.add(index);
      }
    });
    const names = new Set([...used].map((index) => plan.properties[index]?.name.toLowerCase()));
    plan.properties.forEach((property, index) => {
      if (property.rollup !== undefined && names.has(property.rollup.relation.toLowerCase())) {
        used.add(index);
      }
    });
    // A formula needs the properties it reads (by the names the plan has them under), formulas that read formulas too.
    for (let round = 0; round < 3; round += 1) {
      const before = used.size;
      const usedNames = new Set([...used].map((index) => plan.properties[index]?.name.toLowerCase()));
      for (const property of plan.properties) {
        if (property.formula === undefined || !usedNames.has(property.name.toLowerCase())) {
          continue;
        }
        for (const read of formulaReferencedIds(property.formula.expression)) {
          plan.properties.forEach((other, index) => {
            if (other.name.toLowerCase() === read.toLowerCase()) {
              used.add(index);
            }
          });
        }
      }
      if (used.size === before) {
        break;
      }
    }
    return used;
  }

  /** The first commit call: properties per space, then the pages of every space in the structure. */
  #start(session: Session, plan: ImportPlan): void {
    const { structure } = this.#deps;
    const spaces = this.#spacesOf(session);
    if (session.spaceId !== undefined) {
      for (const page of structure.livePages(session.spaceId)) {
        const same = session.existingTitles.get(page.title) ?? [];
        same.push(page.id);
        session.existingTitles.set(page.title, same);
      }
    }
    for (const space of spaces) {
      const used = session.spaceId === undefined ? this.#usedProperties(plan, session, space) : undefined;
      session.properties.set(space, this.#resolveProperties(session, plan, space, used));
    }
    session.pageIds = plan.pages.map(() => "");
    for (const space of spaces) {
      const indexes = plan.pages.flatMap((_, index) => (session.pageSpaces[index] === space ? [index] : []));
      const local = new Map(indexes.map((index, at) => [index, at]));
      const specs = indexes.map((index) => {
        const page = plan.pages[index];
        let icon = page?.icon;
        if (icon !== undefined) {
          try {
            validIcon(icon);
          } catch {
            this.#warn(session.resolveWarnings, page?.path ?? "", "unsupportedContent", "icon");
            icon = undefined;
          }
        }
        const { cover, coverY } = this.#coverOf(session, page, space);
        return {
          parent: page === undefined || page.parent === null ? null : (local.get(page.parent) ?? null),
          title: page?.title ?? "",
          icon,
          cover,
          coverY,
        };
      });
      const created = structure.pagesCreate(space, session.parentId, specs);
      indexes.forEach((index, at) => {
        session.pageIds[index] = created[at]?.id ?? "";
      });
    }
    plan.pages.forEach((page, index) => {
      const id = session.pageIds[index];
      if (id !== undefined && id !== "") {
        session.byKey.set(page.key, id);
      }
    });
  }

  /**
   * The cover a planned page gets and its position (ADR-0040), as the core's `import_cover` decides: a named cover as it
   * is, a file cover as `attachment:<id>` when the shell uploaded it into the page's space and it is an image. Anything
   * else leaves the page without a cover: `missingFile` for a file that was not uploaded (or went to another space),
   * `unsupportedContent` (detail `cover`) for a cover the core refuses.
   */
  #coverOf(session: Session, page: PlannedPage | undefined, space: SpaceId): { cover: string | undefined; coverY: number | undefined } {
    const none = { cover: undefined, coverY: undefined };
    if (page?.cover === undefined) {
      return none;
    }
    let text: string;
    let attachmentId: AttachmentId | undefined;
    if (page.cover.kind === "named") {
      text = page.cover.value;
    } else {
      attachmentId = session.uploaded.get(page.cover.path);
      if (attachmentId === undefined || this.#deps.attachments.manifestOf(attachmentId)?.spaceId !== space) {
        this.#warn(session.resolveWarnings, page.path, "missingFile", page.cover.path);
        return none;
      }
      text = `attachment:${attachmentId}`;
    }
    try {
      validCover(text);
      if (attachmentId !== undefined && this.#deps.attachments.manifestOf(attachmentId)?.mime.toLowerCase().startsWith("image/") !== true) {
        throw invalid("cover", "the attachment is not an image");
      }
    } catch (error) {
      if (!(error instanceof CoreError)) {
        throw error;
      }
      this.#warn(session.resolveWarnings, page.path, "unsupportedContent", "cover");
      return none;
    }
    if (attachmentId !== undefined) {
      session.attachmentsUsed.add(attachmentId);
    }
    let coverY: number | undefined;
    try {
      coverY = validCoverY(page.coverY);
    } catch (error) {
      if (!(error instanceof CoreError)) {
        throw error;
      }
    }
    return { cover: text, coverY };
  }

  /**
   * Matches each planned property by name (case-folded) in the space, or creates it; with `used`, only the
   * planned properties of that set (the ones a space's pages use). Answers them by index of the plan.
   */
  #resolveProperties(session: Session, plan: ImportPlan, space: SpaceId, used: ReadonlySet<number> | undefined): (Resolved | undefined)[] {
    const { properties } = this.#deps;
    const schema = (): PropertyDef[] => properties.schemaGet(space);
    const byName = (name: string): PropertyDef | undefined =>
      schema().find((def) => def.name.toLowerCase() === name.toLowerCase());
    const resolved: (Resolved | undefined)[] = plan.properties.map(() => undefined);
    const colorOf = (count: number): (typeof OPTION_COLORS)[number] => OPTION_COLORS[count % OPTION_COLORS.length] ?? "gray";

    const addOptions = (def: PropertyDef, planned: PlannedProperty): PropertyDef => {
      if (!propertyKindHasOptions(def.kind)) {
        return def;
      }
      for (const option of planned.options) {
        if (def.options.some((candidate) => candidate.name.toLowerCase() === option.name.toLowerCase())) {
          continue;
        }
        try {
          properties.optionAdd(
            space,
            def.id,
            option.name,
            option.color ?? colorOf(def.options.length),
            def.kind === "status" ? (option.group ?? "todo") : undefined,
          );
        } catch (error) {
          if (!(error instanceof CoreError)) {
            throw error;
          }
          this.#warn(session.resolveWarnings, "", "propertyLimit", planned.name);
          break;
        }
        def = byName(def.name) ?? def;
      }
      return def;
    };

    plan.properties.forEach((planned, index) => {
      if (planned.kind === "rollup" || planned.kind === "formula" || (used !== undefined && !used.has(index))) {
        return;
      }
      const name = planned.name.replace(/\p{Cc}/gu, " ").trim().slice(0, MAX_NAME_CHARS);
      const existing = planned.builtin ? schema().find((def) => def.id === TAGS_PROPERTY_ID) : byName(name);
      if (existing !== undefined) {
        const def = addOptions(existing, planned);
        resolved[index] = { id: def.id, def };
        return;
      }
      try {
        const before = schema().length;
        const config: PropertyConfig | undefined =
          planned.kind === "relation" ? { kind: "relation", twoWay: planned.relation?.twoWay === true } : undefined;
        let def = properties.create(space, name, planned.kind, config);
        session.propertiesCreated += schema().length - before;
        const pair = def.config?.kind === "relation" ? def.config.pairedPropertyId : undefined;
        if (pair !== undefined && planned.relation?.pairName !== undefined && planned.relation.pairName !== `${name} (back)`) {
          properties.rename(space, pair, planned.relation.pairName);
        }
        def = addOptions(def, planned);
        resolved[index] = { id: def.id, def };
      } catch (error) {
        if (!(error instanceof CoreError)) {
          throw error;
        }
        this.#warn(session.resolveWarnings, "", "propertyLimit", planned.name);
      }
    });

    // Rollups need their relation to exist.
    plan.properties.forEach((planned, index) => {
      const rollup = planned.rollup;
      if (planned.kind !== "rollup" || rollup === undefined || (used !== undefined && !used.has(index))) {
        return;
      }
      const existing = byName(planned.name);
      if (existing !== undefined) {
        resolved[index] = { id: existing.id, def: existing };
        return;
      }
      const relation = byName(rollup.relation);
      const target = rollup.target === "title" ? "title" : byName(rollup.target)?.id;
      if (relation === undefined || target === undefined) {
        this.#warn(session.resolveWarnings, "", "invalidValue", planned.name);
        return;
      }
      try {
        const before = schema().length;
        const def = properties.create(space, planned.name, "rollup", {
          kind: "rollup",
          relationPropertyId: relation.id,
          target,
          function: rollup.function,
        });
        session.propertiesCreated += schema().length - before;
        resolved[index] = { id: def.id, def };
        // Back to its place in the source's order: before the next property the plan has.
        const next = resolved.slice(index + 1).find((entry) => entry !== undefined);
        if (next !== undefined) {
          properties.move(space, def.id, next.id);
        }
      } catch (error) {
        if (!(error instanceof CoreError)) {
          throw error;
        }
        this.#warn(session.resolveWarnings, "", "invalidValue", planned.name);
      }
    });

    // Formulas read any property, formulas included: each is made after the formulas it reads (a loop, or a formula that
    // reads itself, is made in the file's order and stays broken).
    const makeFormula = (index: number): void => {
      const planned = plan.properties[index];
      if (planned?.formula === undefined) {
        return;
      }
      const existing = byName(planned.name);
      if (existing !== undefined) {
        resolved[index] = { id: existing.id, def: existing };
        return;
      }
      const written = planned.formula.expression;
      if (codePointLength(written) > MAX_FORMULA_CHARS) {
        this.#warn(session.resolveWarnings, "", "unsupportedContent", planned.name);
        return;
      }
      const before = schema().length;
      // The names of the file are the names of this space now; a text that does not resolve or check is kept as
      // written and stays broken (no value) until the person fixes it.
      const stored = formulaToStored(written, formulaSchemaOf(schema()), FORMULA_TITLE_LABEL);
      let def: PropertyDef | undefined;
      try {
        if (typeof stored === "string") {
          def = properties.create(space, planned.name, "formula", { kind: "formula", expression: stored });
        }
      } catch (error) {
        if (!(error instanceof CoreError)) {
          throw error;
        }
      }
      try {
        def ??= properties.createFormulaUnchecked(space, planned.name, written);
      } catch (error) {
        if (!(error instanceof CoreError)) {
          throw error;
        }
        this.#warn(session.resolveWarnings, "", "propertyLimit", planned.name);
        return;
      }
      session.propertiesCreated += schema().length - before;
      resolved[index] = { id: def.id, def };
    };
    let pending = plan.properties.flatMap((planned, index) =>
      planned.formula !== undefined && (used === undefined || used.has(index)) ? [index] : [],
    );
    while (pending.length > 0) {
      const waiting = new Set(pending.map((index) => plan.properties[index]?.name.toLowerCase()));
      const ready = pending.filter((index) => {
        const planned = plan.properties[index];
        const own = planned?.name.toLowerCase();
        return formulaReferencedIds(planned?.formula?.expression ?? "").every((read) => {
          const name = read.toLowerCase();
          return name === own || !waiting.has(name);
        });
      });
      if (ready.length === 0) {
        pending.forEach(makeFormula);
        break;
      }
      ready.forEach(makeFormula);
      pending = pending.filter((index) => !ready.includes(index));
    }
    return resolved;
  }

  /** One page's document and values. */
  #writePage(session: Session, plan: ImportPlan, index: number, page: PlannedPage, pageId: PageId): void {
    const { newId } = this.#deps;
    const space = session.pageSpaces[index] ?? session.spaceId ?? "";
    const properties = session.properties.get(space) ?? [];
    const blocks = this.#resolveBlocks(session, plan, page, page.blocks, properties);
    const doc: JsonValue = {
      type: "doc",
      content: (blocks.length > 0 ? blocks : [{ type: "paragraph" }]).map((block) => withIds(block, newId)),
    };
    this.#deps.content.createDocument(pageId, doc);
    for (const value of page.values) {
      this.#setValue(session, plan, page, pageId, space, properties[value.property], value.property, value.cell);
    }
    this.#setTemplate(plan, page, pageId, space, properties);
  }

  /**
   * A template page's flag and placeholders (ADR-0056), written after the page's content and values: the flag from the
   * front matter's `template`, each placeholder on the property its name matches (case-insensitively) among the properties
   * of the plan that were resolved or created, else among those the space already has. A name nothing matches, or a
   * property whose kind does not take the placeholder, is ignored. No undo entry of its own (the import is one step).
   */
  #setTemplate(
    plan: ImportPlan,
    page: PlannedPage,
    pageId: PageId,
    space: SpaceId,
    properties: readonly (Resolved | undefined)[],
  ): void {
    if (page.template !== true && (page.templatePlaceholders === undefined || page.templatePlaceholders.length === 0)) {
      return;
    }
    const schema = this.#deps.properties.schemaGet(space);
    const mapped = new Map<PropertyId, TemplatePlaceholder>();
    for (const { name, placeholder } of page.templatePlaceholders ?? []) {
      const lower = name.toLowerCase();
      const at = plan.properties.findIndex((planned) => planned.name.toLowerCase() === lower);
      const found = properties[at]?.def ?? schema.find((def) => def.name.toLowerCase() === lower);
      if (found !== undefined && placeholderTakes(placeholder, found.kind)) {
        mapped.set(found.id, placeholder);
      }
    }
    // A page inside a template carries the map without the flag (it is in the template of an ancestor).
    this.#deps.structure.templateSet(pageId, page.template === true, mapped);
    if (page.template === true || mapped.size > 0) {
      this.#deps.emit({ type: "StructureChanged", data: { spaceId: space } });
    }
  }

  // ----- documents ------------------------------------------------------------------------

  /** The title-to-page lookup of planned pages (first wins, case-insensitive). */
  #plannedTitle(session: Session, plan: ImportPlan, title: string): PageId | undefined {
    if (session.plannedTitles === undefined) {
      session.plannedTitles = new Map();
      plan.pages.forEach((page, index) => {
        const id = session.pageIds[index];
        if (id !== undefined && !session.plannedTitles?.has(page.title.toLowerCase())) {
          session.plannedTitles?.set(page.title.toLowerCase(), id);
        }
      });
    }
    return session.plannedTitles.get(title.toLowerCase());
  }

  /** A live page of the space with exactly this title, when exactly one has it; planned pages come first. */
  #pageByTitle(session: Session, plan: ImportPlan, title: string): PageId | undefined {
    const planned = this.#plannedTitle(session, plan, title);
    if (planned !== undefined) {
      return planned;
    }
    const found = session.existingTitles.get(title);
    return found?.length === 1 ? found[0] : undefined;
  }

  /** The page a marker names, or `undefined`. */
  #pageOf(session: Session, plan: ImportPlan, marker: string): PageId | undefined {
    if (marker.startsWith("plan:")) {
      return session.byKey.get(Number(marker.slice(5)));
    }
    if (marker.startsWith("title:")) {
      return this.#pageByTitle(session, plan, marker.slice(6));
    }
    if (marker.startsWith("existing:")) {
      const meta = this.#deps.structure.pageMeta(marker.slice(9));
      return meta !== undefined && this.#spacesOf(session).includes(meta.spaceId) && !meta.trashed ? meta.id : undefined;
    }
    return undefined;
  }

  /** The attachment a block's id names: a placeholder of a needed file, or an attachment of the space. */
  #attachmentOf(session: Session, plan: ImportPlan, id: string): { id: AttachmentId; path: string } | undefined {
    const path = plan.placeholders.get(id);
    if (path !== undefined) {
      const uploaded = session.uploaded.get(path);
      return uploaded === undefined ? undefined : { id: uploaded, path };
    }
    const space = this.#deps.attachments.manifestOf(id)?.spaceId;
    return space !== undefined && this.#spacesOf(session).includes(space) ? { id, path: "" } : undefined;
  }

  /** Markers in a page's blocks to the pages and attachments they name; what cannot be found becomes text. */
  #resolveBlocks(
    session: Session,
    plan: ImportPlan,
    page: PlannedPage,
    nodes: readonly JsonValue[],
    properties: readonly (Resolved | undefined)[],
  ): JsonValue[] {
    const warn = (code: ImportWarningCode, detail?: string): void => {
      this.#warn(session.resolveWarnings, page.path, code, detail);
    };
    const text = (value: string): JsonValue => ({ type: "text", text: value });
    const visit = (node: JsonValue): JsonValue[] => {
      if (!isObj(node)) {
        return [node];
      }
      const attrs = isObj(node.attrs) ? node.attrs : undefined;
      if (node.type === "pageRef" || node.type === "pageLink") {
        const marker = typeof attrs?.pageId === "string" ? attrs.pageId : "";
        const fallback = typeof attrs?.fallback === "string" ? attrs.fallback : "";
        const target = this.#pageOf(session, plan, marker);
        if (target !== undefined) {
          return [{ type: node.type, attrs: { pageId: target } }];
        }
        if (!marker.startsWith("existing:")) {
          warn("unresolvedLink", marker.startsWith("title:") ? marker.slice(6) : fallback);
        }
        return node.type === "pageRef" ? [text(fallback)] : [{ type: "paragraph", content: fallback === "" ? [] : [text(fallback)] }];
      }
      if (node.type === "image" || node.type === "file") {
        const given = typeof attrs?.attachmentId === "string" ? attrs.attachmentId : "";
        const found = this.#attachmentOf(session, plan, given);
        if (found !== undefined) {
          session.attachmentsUsed.add(found.id);
          return [{ ...node, attrs: { ...attrs, attachmentId: found.id } }];
        }
        const path = plan.placeholders.get(given) ?? given;
        // An image the source keeps on a server, not downloaded: a link to it.
        const remote = plan.remoteFiles.find((file) => file.key === path);
        if (remote !== undefined) {
          warn("remoteImage", remote.url);
          const label = typeof attrs?.alt === "string" && attrs.alt !== "" ? attrs.alt : remote.url;
          return [{ type: "paragraph", content: [{ type: "text", text: label, marks: [{ type: "link", attrs: { href: remote.url } }] }] }];
        }
        warn("missingFile", path);
        const name = typeof attrs?.alt === "string" && attrs.alt !== "" ? attrs.alt : (path.split("/").at(-1) ?? path);
        return [{ type: "paragraph", content: name === "" ? [] : [text(name)] }];
      }
      if (node.type === "pageList" && attrs !== undefined && (Array.isArray(attrs.properties) || typeof attrs.groupBy === "string")) {
        const idOf = (entry: JsonValue | undefined): string | undefined => {
          const name = typeof entry === "string" && entry.startsWith("prop:") ? entry.slice(5) : undefined;
          const index = name === undefined ? -1 : plan.properties.findIndex((property) => property.name.toLowerCase() === name.toLowerCase());
          return properties[index]?.id;
        };
        const next: Obj = { ...attrs };
        if (Array.isArray(attrs.properties)) {
          next.properties = attrs.properties.flatMap((entry) => idOf(entry) ?? []);
        }
        if (typeof attrs.groupBy === "string") {
          const group = idOf(attrs.groupBy);
          if (group === undefined) {
            delete next.groupBy;
          } else {
            next.groupBy = group;
          }
        }
        return [{ ...node, attrs: next }];
      }
      const copy: Obj = { ...node };
      if (Array.isArray(node.content)) {
        copy.content = node.content.flatMap(visit);
      }
      return [copy];
    };
    return nodes.flatMap(visit);
  }

  // ----- values ---------------------------------------------------------------------------

  #setValue(
    session: Session,
    plan: ImportPlan,
    page: PlannedPage,
    pageId: PageId,
    space: SpaceId,
    resolved: Resolved | undefined,
    index: number,
    cell: Cell,
  ): void {
    const planned = plan.properties[index];
    if (planned === undefined || resolved === undefined) {
      return;
    }
    const value = this.#coerce(session, plan, page, space, resolved, cell);
    if (value === undefined) {
      this.#warn(session.coerceWarnings, page.path, "invalidValue", planned.name);
      return;
    }
    if (value === null) {
      return;
    }
    try {
      this.#deps.properties.pageSet(pageId, resolved.id, value);
    } catch (error) {
      if (!(error instanceof CoreError) || error.code !== "InvalidInput") {
        throw error;
      }
      this.#warn(session.coerceWarnings, page.path, "invalidValue", planned.name);
      return;
    }
    if (value.kind === "files") {
      value.value.forEach((id) => session.attachmentsUsed.add(id));
    }
  }

  /** The option of `resolved` called `name` (case-insensitive), added when missing; `undefined` when it cannot be. */
  #optionId(space: SpaceId, resolved: Resolved, name: string): string | undefined {
    const wanted = name.trim();
    const found = resolved.def.options.find((option) => option.name.toLowerCase() === wanted.toLowerCase());
    if (found !== undefined) {
      return found.id;
    }
    if (wanted === "") {
      return undefined;
    }
    try {
      const option = this.#deps.properties.optionAdd(
        space,
        resolved.id,
        wanted,
        OPTION_COLORS[resolved.def.options.length % OPTION_COLORS.length] ?? "gray",
        resolved.def.kind === "status" ? "todo" : undefined,
      );
      resolved.def = { ...resolved.def, options: [...resolved.def.options, option] };
      return option.id;
    } catch (error) {
      if (!(error instanceof CoreError)) {
        throw error;
      }
      return undefined;
    }
  }

  /**
   * A cell made to fit the property's kind: the value, `null` for nothing to write, `undefined` for a
   * value that does not fit (`InvalidValue`).
   */
  #coerce(
    session: Session,
    plan: ImportPlan,
    page: PlannedPage,
    space: SpaceId,
    resolved: Resolved,
    cell: Cell,
  ): PropertyValue | null | undefined {
    const kind = resolved.def.kind;
    const text = cell.text;
    switch (kind) {
      case "text":
      case "url":
      case "email":
      case "phone":
        return text.trim() === "" ? null : { kind, value: text };
      case "number": {
        const number = cell.number ?? parseNumber(text);
        return number === undefined ? undefined : { kind, value: number };
      }
      case "checkbox": {
        const checked = cell.checked ?? parseBool(text);
        return checked === undefined ? undefined : { kind, value: checked };
      }
      case "date": {
        const date = parseDate(text);
        return date === undefined ? undefined : { kind, value: date };
      }
      case "select":
      case "status": {
        const name = cell.items?.[0] ?? text;
        if (name.trim() === "") {
          return null;
        }
        const id = this.#optionId(space, resolved, name);
        return id === undefined ? undefined : { kind, value: id };
      }
      case "multi_select": {
        const names = cell.items ?? text.split(", ");
        const ids: string[] = [];
        for (const name of names) {
          if (name.trim() === "") {
            continue;
          }
          const id = this.#optionId(space, resolved, name);
          if (id === undefined) {
            return undefined;
          }
          if (!ids.includes(id)) {
            ids.push(id);
          }
        }
        return ids.length === 0 ? null : { kind, value: ids };
      }
      case "checklist": {
        const items = cell.checklist ?? (cell.items ?? [text]).map((item) => ({ text: item, done: false }));
        return items.length === 0 || (items.length === 1 && items[0]?.text.trim() === "")
          ? null
          : { kind, value: items.map((item) => ({ id: this.#deps.newId(), text: item.text, done: item.done })) };
      }
      case "place": {
        const place = cell.place ?? { name: text };
        return place.name.trim() === "" ? null : { kind, value: place };
      }
      case "relation": {
        const ids: PageId[] = [];
        for (const target of cell.pages ?? []) {
          const id = "key" in target ? session.byKey.get(target.key) : this.#pageByTitle(session, plan, target.title);
          if (id === undefined) {
            this.#warn(session.coerceWarnings, page.path, "unresolvedLink", "title" in target ? target.title : undefined);
          } else if (this.#deps.structure.pageMeta(id)?.spaceId !== space) {
            // A relation keeps the pages of its own space: the others are left out.
            continue;
          } else if (!ids.includes(id)) {
            ids.push(id);
          }
        }
        return ids.length === 0 ? null : { kind, value: ids };
      }
      case "files": {
        const ids: AttachmentId[] = [];
        for (const path of cell.files ?? []) {
          const id = session.uploaded.get(path);
          if (id === undefined) {
            this.#warn(session.coerceWarnings, page.path, "missingFile", path);
          } else if (!ids.includes(id)) {
            ids.push(id);
          }
        }
        return ids.length === 0 ? null : { kind, value: ids };
      }
      default:
        return undefined;
    }
  }
}

/** A node and its descendants with an `id` attribute where the schema has one (a fresh id from `newId`). */
function withIds(node: JsonValue, newId: () => string): JsonValue {
  if (!isObj(node)) {
    return node;
  }
  const copy: Obj = { ...node };
  if (typeof node.type === "string" && ID_BEARING.has(node.type)) {
    const attrs = isObj(node.attrs) ? node.attrs : {};
    copy.attrs = typeof attrs.id === "string" ? attrs : { ...attrs, id: newId() };
  }
  if (Array.isArray(node.content)) {
    copy.content = node.content.map((child) => withIds(child, newId));
  }
  return copy;
}
