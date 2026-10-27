// The in-memory twin of the core's export sessions (ADR-0036, `docs/architecture/import-export.md`,
// sections 2 and 7): `ExportBegin`, `ExportNext`, `ExportEnd` and `ExportCsv` over the mirror's
// pages, with reduced fidelity. What is the same as in the core: the live pages in tree order, the
// layout and the names of `./memory-export-names` (Notion's layout, de-duplication, the single-file
// rule, shared attachments written once), the entries of a page in order (its file, its whiteboards,
// the attachments laid out for the first time with it), the warnings once per attachment, batches
// of at most `maxEntries` entries with `ExportProgress` events, the sidecar after the last page of
// a multi-file Markdown export, a page without a stored document exporting as an empty page (an
// export creates no document), and the CSV of a view (2.8). What is simpler: the Markdown body is
// the clipboard flavour of `renderMarkdown` (no escaping, no annotations; the only addition is
// that a reference to a page of the export is a relative link and an attachment's link its file's
// relative path), HTML is a compact document, and whiteboards are written as `.excalidraw` files
// that the Markdown does not point at (their texts stay in the quote `renderMarkdown` writes) and
// that an import does not read back.
//
// The shell side (`MemoryTransport.exportRun`) drives a session like the desktop shell does and
// writes the result to the fake disk; {@link encodeMemoryArchive} is the "zip" of that disk, a JSON
// listing of the entries.

import type { MemoryAttachments } from "./attachment-mirror";
import { fromBase64 } from "./backup";
import type { MemoryContent } from "./content";
import { CoreError } from "./errors";
import type { AttachmentId } from "./generated/AttachmentId";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { CsvColumn } from "./generated/CsvColumn";
import type { Event } from "./generated/Event";
import type { ExportBatch } from "./generated/ExportBatch";
import type { ExportEntry } from "./generated/ExportEntry";
import type { ExportFormat } from "./generated/ExportFormat";
import type { ExportPlan } from "./generated/ExportPlan";
import type { ExportWarning } from "./generated/ExportWarning";
import type { PageId } from "./generated/PageId";
import type { PageInfo } from "./generated/PageInfo";
import type { PageNode } from "./generated/PageNode";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyId } from "./generated/PropertyId";
import type { SpaceId } from "./generated/SpaceId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { renderMarkdown } from "./markdown";
import { formatCsv } from "./memory-csv";
import type { CsvCell } from "./memory-csv";
import { layoutExport, relativeHref } from "./memory-export-names";
import type { ExportLayout, LayoutAttachment, LayoutPage } from "./memory-export-names";
import {
  SIDECAR_NAME,
  excalidrawFile,
  exportValue,
  frontMatter,
  htmlPage,
  rustNumber,
  sidecarFile,
  valueText,
} from "./memory-export-text";
import type { ExportValue, ValueLookups } from "./memory-export-text";
import type { MemoryProperties } from "./memory-properties";
import { canonicalId, parsePortable } from "./page-doc";
import type { PortableNode } from "./page-doc";
import type { MemoryStructure } from "./structure";
import { coverAttachment, iconAttachment, invalid } from "./validate";

/** Most pages one export holds. */
export const MAX_EXPORT_PAGES = 50_000;
/** Most entries one `ExportNext` may ask for. */
export const MAX_EXPORT_BATCH = 500;
/** Most columns of a CSV export (the title and 100 properties). */
const MAX_CSV_COLUMNS = 101;

const utf8 = new TextEncoder();
const textDecoder = new TextDecoder();

/** What the export mirror needs from its owner. */
export interface ExportDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  structure: MemoryStructure;
  properties: MemoryProperties;
  content: MemoryContent;
  attachments: MemoryAttachments;
  /** The name of a device (this one, or one of the device log); `undefined` when unknown. */
  deviceName: (deviceId: string) => string | undefined;
  /** The current name of a member of a space (`undefined` for a former member, whom the export calls "Former member"). */
  personName?: (spaceId: SpaceId, accountId: string) => string | undefined;
}

/** `InvalidInput` for `field` that also names the limit. */
function limitExceeded(field: string, message: string, limit: number): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { field, limit } });
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** A page of the export with what the layout and the renderer need. */
interface SessionPage {
  info: PageInfo;
  /** The attachments it uses, in order, without repeats. */
  attachments: AttachmentId[];
  /** The ids of its whiteboard blocks, in document order. */
  whiteboards: string[];
}

interface Session {
  id: number;
  spaceId: SpaceId;
  format: ExportFormat;
  name: string;
  pages: SessionPage[];
  schema: PropertyDef[];
  layout: ExportLayout;
  /** What is known of every attachment the pages use. */
  known: Map<AttachmentId, AttachmentInfo | undefined>;
  /** Attachments already reported (or placed): one warning each. */
  handled: Set<AttachmentId>;
  /** Pages rendered so far. */
  rendered: number;
  /** Entries made and not delivered yet. */
  queue: ExportEntry[];
  sidecarQueued: boolean;
}

/** The pages of the walk in tree order, with their parent when it is in the set. */
function walk(
  nodes: readonly PageNode[],
  parentId: PageId | undefined,
  into: { info: PageInfo; parentId: PageId | undefined }[],
): void {
  for (const node of nodes) {
    into.push({ info: node.page, parentId });
    walk(node.children, node.page.id, into);
  }
}

function findNode(nodes: readonly PageNode[], pageId: PageId): PageNode | undefined {
  for (const node of nodes) {
    if (node.page.id === pageId) {
      return node;
    }
    const inside = findNode(node.children, pageId);
    if (inside !== undefined) {
      return inside;
    }
  }
  return undefined;
}

/** The attachments and whiteboard blocks of a page's content, in document order, without repeats. */
function referencesOf(content: JsonValue | undefined): { attachments: AttachmentId[]; whiteboards: string[] } {
  const attachments: AttachmentId[] = [];
  const whiteboards: string[] = [];
  if (content === undefined) {
    return { attachments, whiteboards };
  }
  const add = (raw: unknown): void => {
    const id = typeof raw === "string" ? canonicalId(raw) : undefined;
    if (id !== undefined && !attachments.includes(id)) {
      attachments.push(id);
    }
  };
  const visit = (node: PortableNode): void => {
    if (node.type === "image" || node.type === "file") {
      add(node.attrs?.attachmentId);
    } else if (node.type === "gallery") {
      const items = node.attrs?.items;
      for (const item of Array.isArray(items) ? (items as unknown[]) : []) {
        add(typeof item === "object" && item !== null ? (item as { attachmentId?: unknown }).attachmentId : undefined);
      }
    } else if (node.type === "whiteboard") {
      const id = node.attrs?.id;
      whiteboards.push(typeof id === "string" ? (canonicalId(id) ?? id) : "");
    }
    node.content.forEach(visit);
  };
  visit(parsePortable(content));
  return { attachments, whiteboards };
}

/**
 * Turns the page references of a document into links: an inline `pageRef` to a page of the export
 * becomes its title as text with a `link` mark (`[Title](relative/path.md)`), a `pageLink` card the
 * same alone in a paragraph. References outside the export are left alone (`[[Title]]`).
 */
function linkPages(json: JsonValue, linkOf: (pageId: PageId) => { title: string; href: string } | undefined): JsonValue {
  const visit = (value: JsonValue): JsonValue => {
    if (Array.isArray(value)) {
      return value.map(visit);
    }
    if (typeof value !== "object" || value === null) {
      return value;
    }
    const node: { [key: string]: JsonValue } = { ...value };
    const attrs = node.attrs;
    const pageId =
      (node.type === "pageRef" || node.type === "pageLink") && typeof attrs === "object" && attrs !== null && !Array.isArray(attrs)
        ? attrs.pageId
        : undefined;
    const link = typeof pageId === "string" ? linkOf(canonicalId(pageId) ?? pageId) : undefined;
    if (link !== undefined) {
      const text: JsonValue = {
        type: "text",
        text: link.title,
        marks: [{ type: "link", attrs: { href: link.href } }],
      };
      return node.type === "pageRef" ? text : { type: "paragraph", content: [text] };
    }
    if (Array.isArray(node.content)) {
      node.content = node.content.map(visit);
    }
    return node;
  };
  return visit(json);
}

/** The export sessions of one core: at most one at a time, ids counting up from 1. */
export class MemoryExport {
  readonly #deps: ExportDeps;
  #lastId = 0;
  #session: Session | undefined;

  constructor(deps: ExportDeps) {
    this.#deps = deps;
  }

  /** Drops the session (a lock does). */
  drop(): void {
    this.#session = undefined;
  }

  /**
   * `ExportBegin`: collects the live pages (one, a page with its live descendants, or the whole
   * space) in tree order with what the layout needs, lays them out and answers the plan. Nothing
   * is rendered yet, and no document is created. A new session replaces the old one.
   */
  begin(spaceId: SpaceId, pageId: PageId | undefined, includeChildren: boolean, format: ExportFormat): ExportPlan {
    const { structure, properties, content, attachments } = this.#deps;
    if (format !== "markdown" && format !== "html") {
      throw invalid("format", "must be markdown or html");
    }
    if (!structure.hasSpace(spaceId)) {
      throw notFound("space not found");
    }
    const space = structure.spaceList().find((candidate) => candidate.id === spaceId);
    const tree = structure.treeGet(spaceId);
    const collected: { info: PageInfo; parentId: PageId | undefined }[] = [];
    let name = space?.name ?? "";
    if (pageId === undefined) {
      walk(tree.roots, undefined, collected);
    } else {
      const meta = structure.pageMeta(pageId);
      if (meta === undefined) {
        throw notFound("page not found");
      }
      if (meta.spaceId !== spaceId) {
        throw invalid("pageId", "the page belongs to another space");
      }
      if (meta.trashed) {
        throw invalid("pageId", "the page is in the trash");
      }
      const node = findNode(tree.roots, pageId);
      if (node === undefined) {
        throw notFound("page not found");
      }
      name = node.page.title;
      if (includeChildren) {
        walk([node], undefined, collected);
      } else {
        collected.push({ info: node.page, parentId: undefined });
      }
    }
    if (collected.length > MAX_EXPORT_PAGES) {
      throw limitExceeded(pageId === undefined ? "spaceId" : "pageId", "the export has too many pages", MAX_EXPORT_PAGES);
    }
    const pages: SessionPage[] = collected.map(({ info }) => {
      const found = referencesOf(content.storedContent(info.id));
      const used = [...found.attachments];
      const add = (id: string | undefined): void => {
        const canonical = id === undefined ? undefined : canonicalId(id);
        if (canonical !== undefined && !used.includes(canonical)) {
          used.push(canonical);
        }
      };
      for (const [, value] of info.properties ?? []) {
        if (value.kind === "files") {
          value.value.forEach(add);
        }
      }
      add(iconAttachment(info.icon));
      add(coverAttachment(info.cover));
      return { info, attachments: used, whiteboards: found.whiteboards };
    });
    const known = new Map<AttachmentId, AttachmentInfo | undefined>();
    for (const page of pages) {
      for (const attachmentId of page.attachments) {
        if (!known.has(attachmentId)) {
          try {
            known.set(attachmentId, attachments.get(attachmentId));
          } catch {
            known.set(attachmentId, undefined);
          }
        }
      }
    }
    const layoutAttachments = new Map<AttachmentId, LayoutAttachment>();
    for (const [attachmentId, info] of known) {
      if (info !== undefined) {
        layoutAttachments.set(attachmentId, { name: info.name, available: info.availability === "Complete" });
      }
    }
    const layoutPages: LayoutPage[] = collected.map(({ info, parentId }, at) => ({
      id: info.id,
      parentId,
      title: info.title,
      attachments: pages[at]?.attachments ?? [],
      whiteboards: pages[at]?.whiteboards.length ?? 0,
    }));
    const layout = layoutExport({ pages: layoutPages, attachments: layoutAttachments, format, exportName: name });
    this.#lastId += 1;
    this.#session = {
      id: this.#lastId,
      spaceId,
      format,
      name,
      pages,
      schema: properties.schemaGet(spaceId),
      layout,
      known,
      handled: new Set(),
      rendered: 0,
      queue: [],
      sidecarQueued: false,
    };
    return {
      exportId: this.#lastId,
      pages: pages.length,
      attachments: layout.attachmentPaths.size,
      singleFile: layout.singleFile,
      suggestedName: layout.suggestedName,
    };
  }

  /**
   * `ExportNext`: renders pages until `maxEntries` entries are ready (1 to 500; a page's group may
   * continue in the next batch) and delivers them, with the warnings of the pages rendered by this
   * call, then emits `ExportProgress` (pages rendered so far). After the last page of a multi-file
   * Markdown export comes the sidecar. `NotFound` for an unknown session.
   */
  next(exportId: number, maxEntries: number): ExportBatch {
    const session = this.#require(exportId);
    if (!Number.isInteger(maxEntries) || maxEntries < 1 || maxEntries > MAX_EXPORT_BATCH) {
      throw limitExceeded("maxEntries", "must be 1 to 500", MAX_EXPORT_BATCH);
    }
    const warnings: ExportWarning[] = [];
    while (session.queue.length < maxEntries && session.rendered < session.pages.length) {
      this.#renderPage(session, warnings);
    }
    if (
      session.rendered >= session.pages.length &&
      !session.sidecarQueued &&
      session.format === "markdown" &&
      !session.layout.singleFile
    ) {
      session.sidecarQueued = true;
      session.queue.push({
        path: SIDECAR_NAME,
        content: {
          type: "Text",
          data: {
            text: sidecarFile({
              name: session.name,
              exportedAt: this.#deps.now(),
              schema: session.schema,
              pages: session.pages.flatMap((page) => session.layout.pagePaths.get(page.info.id) ?? []),
            }),
          },
        },
      });
    }
    const entries = session.queue.splice(0, maxEntries);
    const batch: ExportBatch = {
      entries,
      done: session.queue.length === 0 && session.rendered >= session.pages.length,
    };
    if (warnings.length > 0) {
      batch.warnings = warnings;
    }
    this.#deps.emit({ type: "ExportProgress", data: { exportId, done: session.rendered, total: session.pages.length } });
    return batch;
  }

  /** `ExportEnd`: drops the session (nothing to do for one that is already gone). */
  end(exportId: number): void {
    if (this.#session?.id === exportId) {
      this.#session = undefined;
    }
  }

  /**
   * `ExportCsv` (2.8): the rows (`pageIds` in the order given) and columns of a view as CSV text:
   * a byte order mark, CRLF, header `Name` for the title and the property names, fields quoted when
   * they need it, text cells that a spreadsheet could take for a formula prefixed with `'`
   * (numbers never are), values as `ExportValue::text` writes them.
   */
  csv(spaceId: SpaceId, pageIds: readonly PageId[], columns: readonly CsvColumn[]): string {
    const { structure, properties } = this.#deps;
    if (!structure.hasSpace(spaceId)) {
      throw notFound("space not found");
    }
    if (pageIds.length > MAX_EXPORT_PAGES) {
      throw limitExceeded("pageIds", "the view has too many rows", MAX_EXPORT_PAGES);
    }
    if (columns.length > MAX_CSV_COLUMNS) {
      throw limitExceeded("columns", "the view has too many columns", MAX_CSV_COLUMNS);
    }
    const schema = properties.schemaGet(spaceId);
    const chosen: (PropertyDef | "title")[] = columns.map((column) => {
      if (column.type === "Title") {
        return "title";
      }
      const def = schema.find((property) => property.id === column.data.propertyId);
      if (def === undefined) {
        throw notFound("property not found");
      }
      return def;
    });
    const metas = pageIds.map((pageId) => {
      const meta = structure.pageMeta(pageId);
      if (meta === undefined) {
        throw notFound("page not found");
      }
      if (meta.spaceId !== spaceId) {
        throw invalid("pageIds", "a page belongs to another space");
      }
      return meta;
    });
    const look = this.#lookups(spaceId, schema, undefined);
    const rows: CsvCell[][] = [chosen.map((column) => ({ text: column === "title" ? "Name" : column.name }))];
    for (const meta of metas) {
      const values = new Map<PropertyId, ExportValue>();
      for (const [propertyId, value] of properties.valuesOf(spaceId, meta.id)) {
        const resolved = exportValue(value, look);
        if (resolved !== undefined) {
          values.set(propertyId, resolved);
        }
      }
      rows.push(
        chosen.map((column): CsvCell => {
          if (column === "title") {
            return { text: meta.title };
          }
          const value = values.get(column.id);
          if (value === undefined) {
            return { text: "" };
          }
          return value.kind === "number" ? { text: rustNumber(value.value), number: true } : { text: valueText(value) };
        }),
      );
    }
    return formatCsv(rows);
  }

  // ----- plumbing -------------------------------------------------------------------------

  #require(exportId: number): Session {
    if (this.#session === undefined || this.#session.id !== exportId) {
      throw notFound("export session not found");
    }
    return this.#session;
  }

  #lookups(
    spaceId: SpaceId,
    schema: readonly PropertyDef[],
    pageFile: ((attachmentId: AttachmentId) => string | undefined) | undefined,
  ): ValueLookups {
    const { structure, attachments } = this.#deps;
    return {
      optionName: (optionId) => {
        for (const property of schema) {
          const option = property.options.find((candidate) => candidate.id === optionId);
          if (option !== undefined) {
            return option.name;
          }
        }
        return undefined;
      },
      pageTitle: (pageId) => structure.pageMeta(pageId)?.title,
      fileName: (attachmentId) => attachments.nameOf(attachmentId),
      filePath: (attachmentId) => pageFile?.(attachmentId),
      deviceName: this.#deps.deviceName,
      personName: (accountId) => this.#deps.personName?.(spaceId, accountId),
    };
  }

  /** Renders the next page into the queue: its file, its whiteboards, the attachments laid out with it. */
  #renderPage(session: Session, warnings: ExportWarning[]): void {
    const page = session.pages[session.rendered];
    session.rendered += 1;
    if (page === undefined) {
      return;
    }
    const { layout } = session;
    const pageId = page.info.id;
    const path = layout.pagePaths.get(pageId) ?? "";
    const portable = this.#deps.content.storedContent(pageId) ?? { type: "doc" };
    session.queue.push({ path, content: { type: "Text", data: { text: this.#pageText(session, page, path, portable) } } });
    const boards = layout.whiteboardPaths.get(pageId) ?? [];
    const drawn = typeof portable === "object" && !Array.isArray(portable) && portable !== null ? portable.whiteboards : undefined;
    page.whiteboards.forEach((blockId, at) => {
      const elements =
        typeof drawn === "object" && drawn !== null && !Array.isArray(drawn) ? drawn[blockId] : undefined;
      const board = boards[at];
      if (board !== undefined) {
        session.queue.push({
          path: board,
          content: {
            type: "Text",
            data: {
              text: excalidrawFile(
                typeof elements === "object" && elements !== null && !Array.isArray(elements) ? elements : {},
              ),
            },
          },
        });
      }
    });
    for (const attachmentId of page.attachments) {
      if (session.handled.has(attachmentId)) {
        continue;
      }
      session.handled.add(attachmentId);
      const info = session.known.get(attachmentId);
      const target = layout.attachmentPaths.get(attachmentId);
      if (info !== undefined && target !== undefined) {
        session.queue.push({ path: target, content: { type: "Attachment", data: { attachmentId, size: info.size } } });
      } else {
        warnings.push({
          code: info === undefined ? "attachmentMissing" : "attachmentNotDownloaded",
          pageId,
          attachmentId,
        });
      }
    }
  }

  /** The text of a page's file: front matter and Markdown body, or an HTML document. */
  #pageText(session: Session, page: SessionPage, path: string, portable: JsonValue): string {
    const { structure } = this.#deps;
    const { layout, schema } = session;
    const hrefOfPage = (target: PageId): string | undefined => {
      const targetPath = layout.pagePaths.get(target);
      return targetPath === undefined ? undefined : relativeHref(path, targetPath);
    };
    const hrefOfAttachment = (target: AttachmentId): string | undefined => {
      const targetPath = layout.attachmentPaths.get(target);
      return targetPath === undefined ? undefined : relativeHref(path, targetPath);
    };
    const look = this.#lookups(session.spaceId, schema, (attachmentId) => {
      const targetPath = layout.attachmentPaths.get(attachmentId);
      return targetPath === undefined ? undefined : relativeHref(path, targetPath, false);
    });
    const values = (page.info.properties ?? []).flatMap(([propertyId, value]) => {
      const property = schema.find((candidate) => candidate.id === propertyId);
      return property === undefined ? [] : [{ property, value: exportValue(value, look) }];
    });
    const iconTarget = iconAttachment(page.info.icon);
    const icon =
      iconTarget === undefined
        ? page.info.icon
        : layout.attachmentPaths.has(iconTarget)
          ? relativeHref(path, layout.attachmentPaths.get(iconTarget) ?? "", false)
          : undefined;
    // The cover is written the same way (ADR-0040): a colour or a gradient as stored, an uploaded one
    // as the link to its file, or left out with its position.
    const coverTarget = coverAttachment(page.info.cover);
    const cover =
      coverTarget === undefined
        ? page.info.cover
        : layout.attachmentPaths.has(coverTarget)
          ? relativeHref(path, layout.attachmentPaths.get(coverTarget) ?? "", false)
          : undefined;
    const coverY = cover === undefined ? undefined : page.info.coverY;
    if (session.format === "html") {
      return htmlPage(
        {
          title: page.info.title,
          icon: iconTarget === undefined ? page.info.icon : undefined,
          cover,
          coverY,
          properties: values.flatMap(({ property, value }) =>
            value === undefined ? [] : [{ name: property.name, text: valueText(value) }],
          ),
          content: portable,
        },
        {
          pageHref: hrefOfPage,
          attachmentHref: hrefOfAttachment,
          attachmentName: (attachmentId) => this.#deps.attachments.nameOf(attachmentId),
          pageTitle: (pageId) => structure.pageMeta(pageId)?.title,
          whiteboardHref: (index) => {
            const board = layout.whiteboardPaths.get(page.info.id)?.[index];
            return board === undefined ? undefined : relativeHref(path, board);
          },
        },
      );
    }
    const linked = linkPages(portable, (target) => {
      const href = hrefOfPage(target);
      const title = structure.pageMeta(target)?.title;
      return href === undefined || title === undefined ? undefined : { title, href };
    });
    const body = renderMarkdown(linked, (target) => structure.pageMeta(target)?.title, {
      resolveAttachmentName: (attachmentId) => this.#deps.attachments.nameOf(attachmentId),
    }).replace(/attachment:\/\/([0-9a-fA-F-]{32,36})/gu, (whole, raw: string) => {
      const attachmentId = canonicalId(raw);
      return (attachmentId === undefined ? undefined : hrefOfAttachment(attachmentId)) ?? whole;
    });
    // A template and its placeholders (ADR-0056): by property name, in the schema order `PageInfo` lists them in; only those
    // of a page in a template whose property still exists and takes the placeholder, which is what `PageInfo` holds.
    const templatePlaceholders = (page.info.templatePlaceholders ?? []).flatMap(([propertyId, placeholder]) => {
      const property = schema.find((candidate) => candidate.id === propertyId);
      return property === undefined ? [] : [{ name: property.name, placeholder }];
    });
    return `${frontMatter({
      title: page.info.title,
      icon,
      cover,
      coverY,
      template: page.info.template === true,
      templatePlaceholders,
      createdAt: page.info.createdAt,
      updatedAt: page.info.updatedAt,
      values,
    })}\n${body}`;
  }
}

// ----- the fake archive -----------------------------------------------------------------------

/** One entry of the fake archive {@link encodeMemoryArchive} writes. */
export interface MemoryArchiveEntry {
  path: string;
  /** A text file's text. */
  text?: string;
  /** An attachment's bytes in base64 (and its id and size). */
  base64?: string;
  attachmentId?: AttachmentId;
  size?: number;
}

/** The format marker of the fake archive. */
const ARCHIVE_FORMAT = "atomik-kb-memory-archive";

/**
 * The "zip" of the in-memory shell: the fake disk holds a JSON listing of the entries (each a text
 * file, or an attachment with its id, size and bytes in base64), not a real archive.
 */
export function encodeMemoryArchive(entries: readonly MemoryArchiveEntry[]): Uint8Array {
  return utf8.encode(JSON.stringify({ format: ARCHIVE_FORMAT, entries }, null, 2));
}

/**
 * Reads what {@link encodeMemoryArchive} wrote: the files as an import source takes them (text
 * for text entries, bytes for attachments). `undefined` when the bytes are not such a listing.
 */
export function readMemoryArchive(bytes: Uint8Array): Record<string, string | Uint8Array> | undefined {
  try {
    const parsed = JSON.parse(textDecoder.decode(bytes)) as { format?: unknown; entries?: unknown };
    if (parsed.format !== ARCHIVE_FORMAT || !Array.isArray(parsed.entries)) {
      return undefined;
    }
    const files: Record<string, string | Uint8Array> = {};
    for (const entry of parsed.entries as MemoryArchiveEntry[]) {
      if (entry.text !== undefined) {
        files[entry.path] = entry.text;
      } else if (entry.base64 !== undefined) {
        files[entry.path] = fromBase64(entry.base64);
      }
    }
    return files;
  } catch {
    return undefined;
  }
}
