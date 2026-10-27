// The plan of an import (ADR-0036, `docs/architecture/import-export.md`, 3.3 to 3.8), the in-memory
// twin of `kb_import::plan`, with reduced fidelity. Source files in, a tree of pages out, with
// their content as portable blocks (no ids yet, markers where a link waits for the pages to
// exist), the property values they carry, the properties those need and the files they use.
//
// Same as the core: the single wrapping folder is stripped; names lose their Notion id; the tree
// follows `stem/` folders (a folder without a page of its name is a page of its own); a Notion
// database (`X <id>_all.csv`, its rows matched to their pages by title) is a page with a table view
// and one child per row; siblings follow the sidecar, then the order of the links, then natural
// order of names; a container page named after the source when there are several top-level
// pages; titles come from the front matter, a first level-1 heading or the file name; front
// matter, tags and CSV columns become properties with inferred kinds; relative links become page
// references, relative image links to files of the source become image blocks (the files are
// "needed": the shell uploads them), nothing is fetched.
//
// Reduced: Markdown goes through `parseMarkdown` (`./markdown-parse`: no footnotes, no
// annotations of an atomik archive but the callout icons, no HTML blocks beyond `<aside>`, a link
// to a file inside a line of text becomes a file block after the line), text is paragraphs with hard
// breaks, HTML is its text in paragraphs, whiteboards (`.excalidraw`) are not read, YAML front
// matter is the subset of `./memory-import-text`, Notion relation cells are the only relation
// inference. Markers: `plan:<key>` (a page of the import), `title:<Title>` (a page of the space,
// looked up when the pages are written), `existing:<id>` (an atomik-kb link), and placeholder
// attachment ids for the needed files; `./memory-import` resolves them.

import type { ImportWarning } from "./generated/ImportWarning";
import type { ImportWarningCode } from "./generated/ImportWarningCode";
import type { OptionColor } from "./generated/OptionColor";
import type { PropertyKind } from "./generated/PropertyKind";
import type { RollupFunction } from "./generated/RollupFunction";
import type { StatusGroup } from "./generated/StatusGroup";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { parseMarkdown, portableNodesToJson } from "./markdown-parse";
import { parseCsv } from "./memory-csv";
import { isAppFlowyArchive, planAppFlowy } from "./memory-import-appflowy";
import {
  cleanName,
  decodePath,
  htmlText,
  isEmoji,
  naturalCompare,
  parseBool,
  parseDate,
  parseNumber,
  parseYaml,
  resolveRelative,
  splitFrontMatter,
  splitPath,
  textParagraphs,
  titleOf,
} from "./memory-import-text";
import type { YamlValue } from "./memory-import-text";
import { canonicalId } from "./page-doc";
import { OPTION_COLORS, PROPERTY_KINDS, STATUS_GROUPS, TAGS_NAME } from "./properties";
import { ROLLUP_FUNCTIONS } from "./rollup";

/** Most pages one import plans. */
export const MAX_PLAN_PAGES = 10_000;
/** Most binary paths one import is told about. */
export const MAX_BINARY_PATHS = 100_000;

/** The name of the sidecar file of an atomik-kb archive. */
const SIDECAR_PATH = "atomik-export.json";

/** A page a value points at: a page of the import (by its key), or one found by title. */
export type CellPage = { key: number } | { title: string };

/** What a source holds for one property of one page, before it is made to fit the property's kind. */
export interface Cell {
  /** The value as text. */
  text: string;
  /** The items of a list. */
  items?: string[];
  number?: number;
  checked?: boolean;
  pages?: CellPage[];
  /** Paths (as the shell gave them) of the files of a `files` value. */
  files?: string[];
  checklist?: { text: string; done: boolean }[];
  place?: { name: string; lat?: number; lon?: number };
}

/** One value of a planned page: the planned property (index into `ImportPlan.properties`) and its cell. */
export interface PlannedValue {
  property: number;
  cell: Cell;
}

/** An option a planned property starts with. */
export interface PlannedOption {
  name: string;
  color?: OptionColor;
  group?: StatusGroup;
}

/** A property the import needs: matched by name in the space, created when missing. */
export interface PlannedProperty {
  name: string;
  kind: PropertyKind;
  options: PlannedOption[];
  /** The built-in Tags property (reused whatever the source says). */
  builtin: boolean;
  /** The sidecar defined it: its kind is kept whatever the values look like. */
  fixed: boolean;
  relation?: { twoWay: boolean; pairName?: string };
  rollup?: { relation: string; target: string; function: RollupFunction };
  /** A formula (ADR-0055): the expression as the sidecar wrote it, in display form (the properties it reads by name). */
  formula?: { expression: string };
}

/**
 * The cover a source gives a page (ADR-0040): a named one (`color:<name>`, `gradient:<name>`, already in the
 * grammar of the core) or a file of the source, uploaded by the shell and used when it is an image (`path`:
 * a path of `ImportPlan.neededFiles`, or a key of `ImportPlan.remoteFiles`).
 */
export type PlannedCover = { kind: "named"; value: string } | { kind: "file"; path: string };

/** The nine palette names a `color:` cover takes. */
const COVER_COLORS = ["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"];

/** `text` when it is a named cover in the grammar of the core (`color:<palette name>` or `gradient:<[a-z0-9-]{1,32}>`). */
export function namedCover(text: string): string | undefined {
  const trimmed = text.trim();
  if (trimmed.startsWith("color:")) {
    return COVER_COLORS.includes(trimmed.slice("color:".length)) ? trimmed : undefined;
  }
  return trimmed.startsWith("gradient:") && /^gradient:[a-z0-9-]{1,32}$/u.test(trimmed) ? trimmed : undefined;
}

/** A cover position: an integer from 0 to 100 written as text. */
function coverPosition(text: string): number | undefined {
  return /^\d{1,3}$/u.test(text.trim()) && Number(text.trim()) <= 100 ? Number(text.trim()) : undefined;
}

/** A page the import creates. */
export interface PlannedPage {
  /** Stable id of the page in the plan: what `plan:<key>` markers name. */
  key: number;
  /** Index of the parent in `ImportPlan.pages` (always earlier); `null` under the destination. */
  parent: number | null;
  title: string;
  icon: string | undefined;
  /** The cover (ADR-0040); a file cover is also one of `ImportPlan.neededFiles` or `remoteFiles`. */
  cover?: PlannedCover | undefined;
  /** The cover's vertical position in percent (0 to 100); the default 50 when absent. */
  coverY?: number | undefined;
  /** The page is a template (ADR-0056): front matter `template: true`; set after its content and values are written. */
  template?: boolean | undefined;
  /**
   * The placeholders of a template page (ADR-0056): front matter `templatePlaceholders`, property names as written (matched
   * case-insensitively when the commit resolves them), only the placeholders this version knows.
   */
  templatePlaceholders?: readonly TemplatePlaceholderEntry[] | undefined;
  /** Portable blocks without ids, markers inside (see the module comment). */
  blocks: JsonValue[];
  values: PlannedValue[];
  /** The source path (for warnings); a folder page has its folder's. */
  path: string;
}

/** One entry of `templatePlaceholders` in a page's front matter: the property's name and its placeholder. */
export interface TemplatePlaceholderEntry {
  name: string;
  placeholder: TemplatePlaceholder;
}

/** The outcome of {@link planImport}. */
export interface ImportPlan {
  pages: PlannedPage[];
  properties: PlannedProperty[];
  /** The files the pages use, as the shell gave their paths. */
  neededFiles: string[];
  /** Placeholder attachment id (in `blocks`) to the file's path as given. */
  placeholders: Map<string, string>;
  warnings: ImportWarning[];
  /**
   * The indexes (into `pages`) of the pages that stand for the spaces of the source, in order (an
   * AppFlowy backup's spaces, ADR-0039); none for the other sources. An `ImportBeginSpaces` session
   * makes a space of each and no page; an `ImportBegin` session keeps them as container pages.
   */
  spaceRoots: number[];
  /** Images the source keeps on a server: `placeholders` name them by their key (`remote/<n>`). */
  remoteFiles: RemoteFile[];
  /** The index (into `pages`) of the first page that uses each needed file (by path as given) or remote file (by key). */
  fileUsers: Map<string, number>;
}

/** A file of the source that lives on a server (`ImportRemoteFile` without its space). */
export interface RemoteFile {
  key: string;
  url: string;
  host: string;
}

/** What {@link planImport} reads. */
export interface PlanInput {
  /** The text files by path. */
  files: ReadonlyMap<string, string>;
  /** Every other file of the source. */
  binaryPaths: readonly string[];
  /** The source's name, used as the container's title when there are several top-level pages. */
  sourceName: string | undefined;
  maxPages: number;
}

// ----- internals ----------------------------------------------------------------------------

type Obj = { [key: string]: JsonValue };

function isObj(value: unknown): value is Obj {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

type SourceKind = "markdown" | "text" | "html" | "csv" | "folder" | "row" | "container";

/** A page in the making. */
interface Source {
  key: number;
  kind: SourceKind;
  /** The normalised path of its file (a folder page's folder); `""` for the container. */
  path: string;
  dir: string;
  /** The name of the folder it owns (`stem`, without a CSV's `_all`). */
  folderKey: string;
  /** The display name (Notion id and `_all` removed). */
  name: string;
  notionId: string | undefined;
  text: string | undefined;
  parent: Source | null;
  children: Source[];
  // Filled by the conversion:
  title: string;
  icon: string | undefined;
  cover: PlannedCover | undefined;
  coverY: number | undefined;
  template?: boolean | undefined;
  templatePlaceholders?: readonly TemplatePlaceholderEntry[] | undefined;
  blocks: JsonValue[];
  values: PlannedValue[];
  /** The sources its content links to, in order of first link. */
  links: Source[];
  /** A CSV database as read: its column names, the title column and the records. */
  csv: { names: string[]; titleAt: number; data: string[][] } | undefined;
  /** The names (lower case) of a CSV database's property columns: what Notion's property lines start with. */
  columnNames: Set<string> | undefined;
  /** A row's CSV title, and the page file that matched it. */
  rowTitle: string | undefined;
  file: Source | undefined;
}

function newSource(key: number, kind: SourceKind, path: string, name: string): Source {
  const split = splitPath(path);
  return {
    key,
    kind,
    path,
    dir: split.dir,
    folderKey: split.stem,
    name,
    notionId: undefined,
    text: undefined,
    parent: null,
    children: [],
    title: name,
    icon: undefined,
    cover: undefined,
    coverY: undefined,
    blocks: [],
    values: [],
    links: [],
    csv: undefined,
    columnNames: undefined,
    rowTitle: undefined,
    file: undefined,
  };
}

/** How the planners register properties (one per name over a whole import). */
export class Registry {
  readonly properties: PlannedProperty[] = [];
  readonly #index = new Map<string, number>();

  /** The planned property called `name`, added when missing; kinds that differ meet as text. */
  add(name: string, kind: PropertyKind, options: readonly string[] = []): number {
    const key = name.toLowerCase();
    let at = this.#index.get(key);
    if (at === undefined) {
      at = this.properties.length;
      this.#index.set(key, at);
      this.properties.push({ name, kind, options: [], builtin: key === TAGS_NAME.toLowerCase(), fixed: false });
    }
    const property = this.properties[at];
    if (property === undefined) {
      return at;
    }
    if (property.builtin) {
      property.kind = "multi_select";
    } else if (property.kind !== kind && !property.fixed) {
      property.kind = "text";
    }
    for (const option of options) {
      if (!property.options.some((candidate) => candidate.name === option)) {
        property.options.push({ name: option });
      }
    }
    return at;
  }

  /** Adds a property the sidecar defines (its kind is kept). */
  addFixed(property: PlannedProperty): void {
    const key = property.name.toLowerCase();
    if (this.#index.has(key)) {
      return;
    }
    this.#index.set(key, this.properties.length);
    this.properties.push(property);
  }
}

/** Everything the conversions share. */
interface Context {
  byPath: Map<string, Source>;
  byPathLower: Map<string, Source>;
  byNotion: Map<string, Source>;
  byTitle: Map<string, Source>;
  /** Binary paths by normalised path, and by lower-case one. */
  binaries: Map<string, string>;
  binariesLower: Map<string, string>;
  /** Needed files: normalised path to placeholder. */
  needed: Map<string, string>;
  placeholders: Map<string, string>;
  registry: Registry;
  /** The next page key (keys are unique in a plan). */
  nextKey: () => number;
  warn: (path: string, code: ImportWarningCode, detail?: string) => void;
}

/** The placeholder attachment id of a needed file (a valid id the parser accepts as an attachment). */
function placeholderFor(ctx: Context, normalised: string): string {
  let placeholder = ctx.needed.get(normalised);
  if (placeholder === undefined) {
    placeholder = `ffffffff-ffff-4fff-8fff-${ctx.needed.size.toString(16).padStart(12, "0")}`;
    ctx.needed.set(normalised, placeholder);
    ctx.placeholders.set(placeholder, ctx.binaries.get(normalised) ?? normalised);
  }
  return placeholder;
}

/** What a link destination points at. */
type Target =
  | { type: "external"; url: string }
  | { type: "attachment"; url: string }
  | { type: "atomik"; pageId: string }
  | { type: "page"; source: Source }
  | { type: "binary"; path: string }
  | { type: "whiteboard" }
  | { type: "data" }
  | { type: "anchor" }
  | { type: "unresolved"; detail: string };

const SCHEME = /^([a-zA-Z][a-zA-Z0-9+.-]*):/u;

/** The planned page a notion.so URL names (its last path segment ends in the 32-hex id). */
function notionPage(url: string, ctx: Context): Source | undefined {
  try {
    const parsed = new URL(url);
    if (!/(^|\.)notion\.so$/iu.test(parsed.hostname)) {
      return undefined;
    }
    const id = /([0-9a-f]{32})$/iu.exec(parsed.pathname)?.[1]?.toLowerCase();
    return id === undefined ? undefined : ctx.byNotion.get(id);
  } catch {
    return undefined;
  }
}

function classify(destination: string, dir: string, ctx: Context): Target {
  let text = destination.trim();
  if (text.startsWith("<") && text.endsWith(">")) {
    text = text.slice(1, -1).trim();
  }
  if (text.startsWith("#")) {
    return { type: "anchor" };
  }
  const scheme = SCHEME.exec(text)?.[1]?.toLowerCase();
  if (scheme !== undefined && scheme.length > 1) {
    if (scheme === "http" || scheme === "https") {
      const page = notionPage(text, ctx);
      return page === undefined ? { type: "external", url: text } : { type: "page", source: page };
    }
    if (scheme === "mailto" || scheme === "tel") {
      return { type: "external", url: text };
    }
    if (scheme === "attachment") {
      return { type: "attachment", url: text };
    }
    if (scheme === "atomik-kb") {
      const id = /^atomik-kb:\/\/page\/([0-9a-fA-F-]{32,36})/u.exec(text)?.[1];
      const canonical = id === undefined ? undefined : canonicalId(id);
      return canonical === undefined ? { type: "unresolved", detail: text } : { type: "atomik", pageId: canonical };
    }
    if (scheme === "data") {
      return { type: "data" };
    }
    return { type: "unresolved", detail: text };
  }
  const cut = text.replace(/[#?].*$/u, "");
  const resolved = resolveRelative(dir, decodePath(cut));
  if (resolved === undefined || resolved === "") {
    return { type: "unresolved", detail: text };
  }
  const page = ctx.byPath.get(resolved) ?? ctx.byPathLower.get(resolved.toLowerCase());
  if (page !== undefined) {
    return { type: "page", source: page };
  }
  const binary = ctx.binaries.has(resolved) ? resolved : ctx.binariesLower.get(resolved.toLowerCase());
  if (binary !== undefined) {
    return { type: "binary", path: binary };
  }
  if (resolved.toLowerCase().endsWith(".excalidraw")) {
    return { type: "whiteboard" };
  }
  return { type: "unresolved", detail: text };
}

const IMAGE_EXTENSIONS = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".avif", ".svg"]);

// ----- Markdown ----------------------------------------------------------------------------------

const LINK =
  /(!?)\[((?:[^[\]\\]|\\.)*)\]\(\s*(<[^<>]*>|(?:[^()\s]|\([^()\s]*\))*)(?:\s+"((?:[^"\\]|\\.)*)")?\s*\)/gu;

/** Applies `change` to the parts of a line outside inline code spans. */
function outsideCode(line: string, change: (part: string) => string): string {
  let out = "";
  let last = 0;
  for (const match of line.matchAll(/(`+)(?:(?!\1)[^`]|`(?!`))*?\1(?!`)/gu)) {
    out += change(line.slice(last, match.index)) + match[0];
    last = match.index + match[0].length;
  }
  return out + change(line.slice(last));
}

/**
 * Rewrites a line's links to files of the source: an image (or a plain link to a picture or file)
 * becomes an `attachment://` line of its own for the parser, a remote image a link, a `data:` image
 * its alt text. Everything else is left for the walk over the parsed blocks (`postNode`).
 */
function rewriteLinks(line: string, source: Source, ctx: Context): string {
  return outsideCode(line, (part) => {
    const whole = part.trim();
    return part.replace(LINK, (match: string, bang: string, label: string, destination: string, title: string | undefined) => {
      const target = classify(destination, source.dir, ctx);
      const image = bang === "!";
      const alt = label.replace(/\\(.)/gu, "$1");
      const alone = whole === match;
      const quoted = title === undefined ? "" : ` "${title}"`;
      if (target.type === "binary") {
        const placeholder = placeholderFor(ctx, target.path);
        const isImage = image || IMAGE_EXTENSIONS.has(splitPath(target.path).ext);
        const block = `${isImage ? "!" : ""}[${label}](attachment://${placeholder}${quoted})`;
        // A plain link inside text keeps its words and adds the file after the line.
        if (alone) {
          return block;
        }
        return image ? `\n\n${block}\n\n` : `${label}\n\n${block}\n\n`;
      }
      if (!image) {
        if (target.type === "whiteboard") {
          ctx.warn(source.path, "unsupportedContent", "whiteboard");
          return label;
        }
        return match;
      }
      switch (target.type) {
        case "external":
          ctx.warn(source.path, "remoteImage", target.url);
          return `[${alt === "" ? target.url : label}](${target.url})`;
        case "data":
          ctx.warn(source.path, "embeddedImage");
          return alt;
        case "attachment":
          return alone ? match : `\n\n${match}\n\n`;
        case "page":
          return `[${label}](${destination})`;
        default:
          ctx.warn(source.path, "missingFile", target.type === "unresolved" ? target.detail : destination);
          return alt;
      }
    });
  });
}

/**
 * The text a Markdown body is parsed from: annotation comments and `<aside>` wrappers dealt with
 * (an aside becomes a quote, so that its leading emoji makes a callout), links rewritten.
 */
function preprocess(body: string, source: Source, ctx: Context): string {
  const lines = body.replace(/^\u{FEFF}/u, "").split(/\r\n|\r|\n/u);
  const out: string[] = [];
  let fence: string | undefined;
  let aside = false;
  for (const raw of lines) {
    const fenced = /^ {0,3}(`{3,}|~{3,})/u.exec(raw)?.[1];
    if (fenced !== undefined) {
      if (fence === undefined) {
        fence = fenced.charAt(0);
      } else if (fenced.charAt(0) === fence) {
        fence = undefined;
      }
      out.push(raw);
      continue;
    }
    if (fence !== undefined) {
      out.push(raw);
      continue;
    }
    const trimmed = raw.trim();
    if (/^<!--.*-->$/u.test(trimmed)) {
      continue;
    }
    if (/^<aside>$/iu.test(trimmed)) {
      aside = true;
      continue;
    }
    if (/^<\/aside>$/iu.test(trimmed)) {
      aside = false;
      out.push("");
      continue;
    }
    const rewritten = rewriteLinks(raw, source, ctx);
    out.push(aside ? `> ${rewritten}` : rewritten);
  }
  return out.join("\n");
}

const ALERT_ICONS: Record<string, string> = { NOTE: "ℹ️", TIP: "💡", IMPORTANT: "❗", WARNING: "⚠️", CAUTION: "🛑" };

/** The state of the post-processing of one page's blocks. */
interface Post {
  source: Source;
  ctx: Context;
  /** Page references that came from a link (a paragraph holding only one becomes a card). */
  cards: WeakSet<object>;
}

/** A page reference node carrying the marker of its target and the text to show if it cannot be found. */
function referenceTo(marker: string, fallback: string, post: Post, card: boolean): Obj {
  const node: Obj = { type: "pageRef", attrs: { pageId: marker, fallback } };
  if (card) {
    post.cards.add(node);
  }
  return node;
}

function withoutLink(node: Obj): Obj {
  const marks = Array.isArray(node.marks) ? node.marks.filter((mark) => !(isObj(mark) && mark.type === "link")) : [];
  const copy: Obj = { ...node };
  if (marks.length > 0) {
    copy.marks = marks;
  } else {
    delete copy.marks;
  }
  return copy;
}

function linkNodes(node: Obj, href: string, post: Post): JsonValue[] {
  const { source, ctx } = post;
  const target = classify(href, source.dir, ctx);
  switch (target.type) {
    case "external":
    case "attachment":
      return [node];
    case "page":
      if (!source.links.includes(target.source)) {
        source.links.push(target.source);
      }
      return [referenceTo(`plan:${String(target.source.key)}`, target.source.name, post, true)];
    case "atomik":
      return [referenceTo(`existing:${target.pageId}`, typeof node.text === "string" ? node.text : "", post, true)];
    case "anchor":
      return [withoutLink(node)];
    default:
      if (target.type === "unresolved") {
        ctx.warn(source.path, "unresolvedLink", target.detail);
      }
      return [withoutLink(node)];
  }
}

const WIKI = /\[\[([^[\]|\n]+)(?:\|([^[\]\n]*))?\]\]/gu;

/** The last segment of a wiki link's target, without a `#heading`, as the title it names. */
function wikiTitle(target: string): string {
  const base = target.split("#")[0] ?? "";
  return (base.split("/").at(-1) ?? base).trim();
}

function wikiNodes(node: Obj, text: string, post: Post): JsonValue[] {
  const { source, ctx } = post;
  if (!text.includes("[[")) {
    return [node];
  }
  const out: JsonValue[] = [];
  let last = 0;
  const piece = (value: string): void => {
    if (value !== "") {
      out.push({ ...node, text: value });
    }
  };
  for (const match of text.matchAll(WIKI)) {
    piece(text.slice(last, match.index));
    last = match.index + match[0].length;
    const title = wikiTitle(match[1] ?? "");
    const planned = ctx.byTitle.get(title.toLowerCase());
    if (title === "") {
      piece(match[0]);
    } else if (planned !== undefined) {
      if (!source.links.includes(planned)) {
        source.links.push(planned);
      }
      out.push(referenceTo(`plan:${String(planned.key)}`, title, post, false));
    } else {
      out.push(referenceTo(`title:${title}`, `[[${title}]]`, post, false));
    }
  }
  piece(text.slice(last));
  return out;
}

function processInlines(children: readonly JsonValue[], post: Post): JsonValue[] {
  const out: JsonValue[] = [];
  for (const child of children) {
    if (!isObj(child) || child.type !== "text" || typeof child.text !== "string") {
      out.push(child);
      continue;
    }
    const marks = Array.isArray(child.marks) ? child.marks : [];
    if (marks.some((mark) => isObj(mark) && mark.type === "code")) {
      out.push(child);
      continue;
    }
    const link = marks.find((mark) => isObj(mark) && mark.type === "link");
    const href = isObj(link) && isObj(link.attrs) && typeof link.attrs.href === "string" ? link.attrs.href : undefined;
    out.push(...(href === undefined ? wikiNodes(child, child.text, post) : linkNodes(child, href, post)));
  }
  return out;
}

/** The plain text of inline content (a reference reads as its fallback). */
function plainOf(nodes: readonly JsonValue[]): string {
  return nodes
    .map((node) => {
      if (!isObj(node)) {
        return "";
      }
      if (node.type === "text") {
        return typeof node.text === "string" ? node.text : "";
      }
      if (node.type === "pageRef" && isObj(node.attrs) && typeof node.attrs.fallback === "string") {
        return node.attrs.fallback;
      }
      return Array.isArray(node.content) ? plainOf(node.content) : "";
    })
    .join("");
}

/** A quote whose first line is a GitHub alert (`[!NOTE]`, ...) is a callout with that icon. */
function alertOf(quote: Obj): Obj {
  const children = Array.isArray(quote.content) ? [...quote.content] : [];
  const first = children[0];
  const text = isObj(first) && first.type === "paragraph" && Array.isArray(first.content) ? first.content[0] : undefined;
  const marker = isObj(text) && text.type === "text" && typeof text.text === "string" ? /^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\][ \t]*/iu.exec(text.text) : null;
  if (marker === null || !isObj(first) || !Array.isArray(first.content) || !isObj(text) || typeof text.text !== "string") {
    return quote;
  }
  const rest = text.text.slice(marker[0].length);
  const content = [...first.content];
  if (rest === "") {
    content.shift();
  } else {
    content[0] = { ...text, text: rest };
  }
  if (content.length > 0) {
    children[0] = { ...first, content };
  } else {
    children.shift();
  }
  return {
    type: "callout",
    attrs: { icon: ALERT_ICONS[(marker[1] ?? "").toUpperCase()] ?? "💡" },
    content: children.length > 0 ? children : [{ type: "paragraph" }],
  };
}

function postNode(node: JsonValue, post: Post): JsonValue {
  if (!isObj(node) || node.type === "codeBlock" || node.type === "mathBlock") {
    return node;
  }
  if (node.type === "paragraph" || node.type === "heading") {
    const inlines = processInlines(Array.isArray(node.content) ? node.content : [], post);
    const only = inlines.length === 1 ? inlines[0] : undefined;
    if (node.type === "paragraph" && isObj(only) && only.type === "pageRef" && post.cards.has(only)) {
      return { type: "pageLink", attrs: only.attrs ?? {} };
    }
    const copy: Obj = { ...node };
    if (inlines.length > 0) {
      copy.content = inlines;
    } else {
      delete copy.content;
    }
    return copy;
  }
  let copy: Obj = { ...node };
  if (Array.isArray(node.content)) {
    copy.content = node.content.map((child) => postNode(child, post));
  }
  if (copy.type === "blockquote") {
    copy = alertOf(copy);
  }
  return copy;
}

/** The blocks of a Markdown body: links to the source's files become blocks, the rest is `parseMarkdown`'s. */
function markdownBlocks(body: string, source: Source, ctx: Context): JsonValue[] {
  const nodes = portableNodesToJson(parseMarkdown(preprocess(body, source, ctx)));
  const post: Post = { source, ctx, cards: new WeakSet() };
  return nodes.map((node) => postNode(node, post));
}

// ----- front matter and values -----------------------------------------------------------------

const IGNORED_KEYS = new Set([
  "title",
  "icon",
  "cover_y",
  "created",
  "updated",
  "aliases",
  "cssclasses",
  "cssclass",
  "publish",
  "permalink",
  "layout",
]);

/**
 * `template: true` (the YAML boolean, or the text `true` in any case, trimmed): the page is a template (ADR-0056). Any other
 * value of the key is not the flag: the key stays an ordinary property, as it was before templates.
 */
function templateFlag(value: YamlValue): boolean {
  return value === true || (typeof value === "string" && value.trim().toLowerCase() === "true");
}

/** A mapping of the front matter (not a scalar, a list or nothing). */
function isMapping(value: YamlValue): value is { [key: string]: YamlValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * The entries of the mapping `templatePlaceholders` of a page's front matter (ADR-0056): property name to `today`, `now` or
 * `me` (any case, trimmed). An entry with an unknown placeholder or an empty name is skipped, and so is one whose name
 * repeats an earlier entry's in another case. A value that is not a mapping is not this key's: it stays an ordinary property.
 */
function templatePlaceholdersOf(value: { [key: string]: YamlValue }): TemplatePlaceholderEntry[] {
  const seen = new Set<string>();
  return Object.entries(value).flatMap(([name, raw]): TemplatePlaceholderEntry[] => {
    const placeholder = typeof raw === "string" ? raw.trim().toLowerCase() : "";
    const known = placeholder === "today" || placeholder === "now" || placeholder === "me";
    const trimmed = name.trim();
    if (!known || trimmed === "" || seen.has(trimmed.toLowerCase())) {
      return [];
    }
    seen.add(trimmed.toLowerCase());
    return [{ name: trimmed, placeholder }];
  });
}

/** The `[[Title]]` a text is, when it is only that. */
function wikiOnly(text: string): string | undefined {
  return /^\[\[([^[\]|]+)(?:\|[^[\]]*)?\]\]$/u.exec(text.trim())?.[1];
}

const CHECKLIST_ITEM = /^\[( |x|X)\] ?(.*)$/u;

/** The paths of a list of strings when every one is a file of the source (relative to `dir`). */
function filesOf(items: readonly string[], dir: string, ctx: Context): string[] | undefined {
  const paths: string[] = [];
  for (const item of items) {
    const resolved = resolveRelative(dir, decodePath(item));
    const found =
      resolved === undefined ? undefined : ctx.binaries.has(resolved) ? resolved : ctx.binariesLower.get(resolved.toLowerCase());
    if (found === undefined) {
      return undefined;
    }
    paths.push(found);
  }
  return paths.length === 0 ? undefined : paths;
}

/** A front matter value as a planned value: its kind, the options it names and its cell. */
function frontMatterValue(
  yaml: YamlValue,
  source: Source,
  ctx: Context,
): { kind: PropertyKind; cell: Cell; options: string[] } | undefined {
  const pageOf = (title: string): CellPage => {
    const planned = ctx.byTitle.get(wikiTitle(title).toLowerCase());
    return planned === undefined ? { title: wikiTitle(title) } : { key: planned.key };
  };
  if (typeof yaml === "boolean") {
    return { kind: "checkbox", cell: { text: yaml ? "Yes" : "No", checked: yaml }, options: [] };
  }
  if (typeof yaml === "number") {
    return { kind: "number", cell: { text: String(yaml), number: yaml }, options: [] };
  }
  if (typeof yaml === "string") {
    if (yaml.trim() === "") {
      return undefined;
    }
    const wiki = wikiOnly(yaml);
    if (wiki !== undefined) {
      return { kind: "relation", cell: { text: yaml, pages: [pageOf(wiki)] }, options: [] };
    }
    return { kind: parseDate(yaml) === undefined ? "text" : "date", cell: { text: yaml }, options: [] };
  }
  if (Array.isArray(yaml)) {
    const items = yaml.filter((item) => item !== null).map((item) => (typeof item === "string" ? item : JSON.stringify(item)));
    if (items.length === 0) {
      return undefined;
    }
    const text = items.join(", ");
    const wikis = items.map(wikiOnly);
    if (wikis.every((wiki) => wiki !== undefined)) {
      return { kind: "relation", cell: { text, items, pages: wikis.map((wiki) => pageOf(wiki ?? "")) }, options: [] };
    }
    const checks = items.map((item) => CHECKLIST_ITEM.exec(item));
    if (checks.every((check) => check !== null)) {
      const checklist = checks.map((check) => ({ done: (check?.[1] ?? " ") !== " ", text: check?.[2] ?? "" }));
      return { kind: "checklist", cell: { text, items, checklist }, options: [] };
    }
    const files = filesOf(items, source.dir, ctx);
    if (files !== undefined) {
      // The shell uploads them for the property, as it does for an image.
      files.forEach((path) => placeholderFor(ctx, path));
      return {
        kind: "files",
        cell: { text, items, files: files.map((path) => ctx.binaries.get(path) ?? path) },
        options: [],
      };
    }
    return { kind: "multi_select", cell: { text, items }, options: items };
  }
  if (typeof yaml === "object" && yaml !== null) {
    const name = yaml.name;
    if (typeof name === "string" && name.trim() !== "") {
      const place: { name: string; lat?: number; lon?: number } = { name };
      if (typeof yaml.lat === "number" && typeof yaml.lon === "number") {
        place.lat = yaml.lat;
        place.lon = yaml.lon;
      }
      return { kind: "place", cell: { text: name, place }, options: [] };
    }
    return { kind: "text", cell: { text: JSON.stringify(yaml) }, options: [] };
  }
  return undefined;
}

/** The tags of a `tags` value: a list, or a string split on commas and spaces, `#` removed. */
function tagsOf(yaml: YamlValue): string[] {
  const raw = Array.isArray(yaml) ? yaml.map((item) => (typeof item === "string" ? item : JSON.stringify(item))) : typeof yaml === "string" ? yaml.split(/[,\s]+/u) : [];
  const tags: string[] = [];
  for (const item of raw) {
    const tag = item.trim().replace(/^#+/u, "");
    if (tag !== "" && !tags.includes(tag)) {
      tags.push(tag);
    }
  }
  return tags;
}

/** The properties of Notion row pages: the first paragraph's `Key: Value` lines of a database's columns are left out. */
function withoutPropertyLines(body: string, columns: ReadonlySet<string>): string {
  const lines = body.split(/\r\n|\r|\n/u);
  let at = 0;
  while (at < lines.length && (lines[at] ?? "").trim() === "") {
    at += 1;
  }
  if (/^#\s/u.test(lines[at] ?? "")) {
    at += 1;
  }
  while (at < lines.length && (lines[at] ?? "").trim() === "") {
    at += 1;
  }
  let end = at;
  while (end < lines.length && (lines[end] ?? "").trim() !== "") {
    end += 1;
  }
  const block = lines.slice(at, end);
  const allColumns =
    block.length > 0 &&
    block.every((line) => {
      const key = /^([^:]+): ?/u.exec(line)?.[1];
      return key !== undefined && columns.has(key.trim().toLowerCase());
    });
  return allColumns ? [...lines.slice(0, at), ...lines.slice(end)].join("\n") : body;
}

/** What one page's conversion found. */
interface Converted {
  title: string | undefined;
  icon: string | undefined;
  cover?: PlannedCover | undefined;
  coverY?: number | undefined;
  template?: boolean | undefined;
  templatePlaceholders?: readonly TemplatePlaceholderEntry[] | undefined;
  blocks: JsonValue[];
  values: PlannedValue[];
}

/**
 * The cover of a page from front matter (ADR-0040): a named one as atomik-kb writes it, or the relative path of a
 * picture of the source (the file is recorded as needed). `undefined` for anything else, which leaves the key to be
 * read as a property.
 */
function coverOf(value: YamlValue, source: Source, ctx: Context): PlannedCover | undefined {
  if (typeof value !== "string" || value.trim() === "") {
    return undefined;
  }
  const named = namedCover(value);
  if (named !== undefined) {
    return { kind: "named", value: named };
  }
  const text = value.trim();
  if (/^[a-z][a-z0-9+.-]+:/iu.test(text)) {
    return undefined;
  }
  const resolved = resolveRelative(source.dir, decodePath(text));
  const binary =
    resolved === undefined ? undefined : ctx.binaries.has(resolved) ? resolved : ctx.binariesLower.get(resolved.toLowerCase());
  if (binary === undefined || !IMAGE_EXTENSIONS.has(extensionOf(binary))) {
    return undefined;
  }
  placeholderFor(ctx, binary);
  return { kind: "file", path: ctx.binaries.get(binary) ?? binary };
}

/** The lower-case extension of a path with its dot; `""` when it has none. */
function extensionOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? "" : name.slice(dot).toLowerCase();
}

function convertMarkdown(source: Source, text: string, ctx: Context, columns?: ReadonlySet<string>): Converted {
  const { yaml, body: afterMatter } = splitFrontMatter(text);
  let entries: [string, YamlValue][] = [];
  if (yaml !== undefined) {
    const parsed = parseYaml(yaml);
    if (parsed === undefined) {
      ctx.warn(source.path, "unsupportedContent", "front matter");
    } else {
      entries = parsed;
    }
  }
  let title: string | undefined;
  let icon: string | undefined;
  let cover: PlannedCover | undefined;
  let coverY: number | undefined;
  let template: boolean | undefined;
  let templatePlaceholders: TemplatePlaceholderEntry[] | undefined;
  const values: PlannedValue[] = [];
  for (const [key, value] of entries) {
    const lower = key.toLowerCase();
    if (lower === "cover" && cover === undefined) {
      const found = coverOf(value, source, ctx);
      if (found !== undefined) {
        cover = found;
        continue;
      }
    }
    if (lower === "cover_y") {
      coverY = typeof value === "string" || typeof value === "number" ? coverPosition(String(value)) : undefined;
      continue;
    }
    // A template and its placeholders (ADR-0056): a `template` that is `true` and a `templatePlaceholders` that is a mapping are
    // read as such and are not properties; any other value of those keys is an ordinary property.
    if (lower === "template" && templateFlag(value)) {
      template = true;
      continue;
    }
    if (lower === "templateplaceholders" && isMapping(value)) {
      templatePlaceholders = templatePlaceholdersOf(value);
      continue;
    }
    if (lower === "title") {
      title = typeof value === "string" && value.trim() !== "" ? titleOf(value) : undefined;
    } else if (lower === "icon") {
      if (typeof value === "string" && (isEmoji(value.trim()) || /^icon:[a-z0-9-]+(?::[a-z]+)?$/u.test(value.trim()))) {
        icon = value.trim();
      } else if (typeof value === "string" && value.trim() !== "") {
        ctx.warn(source.path, "unsupportedContent", "icon");
      }
    } else if (lower === "tags") {
      const tags = tagsOf(value);
      if (tags.length > 0) {
        values.push({ property: ctx.registry.add(TAGS_NAME, "multi_select", tags), cell: { text: tags.join(", "), items: tags } });
      }
    } else if (!IGNORED_KEYS.has(lower)) {
      const found = frontMatterValue(value, source, ctx);
      if (found !== undefined && key.trim() !== "") {
        values.push({ property: ctx.registry.add(key.trim(), found.kind, found.options), cell: found.cell });
      }
    }
  }
  const body = columns === undefined ? afterMatter : withoutPropertyLines(afterMatter, columns);
  const blocks = markdownBlocks(body, source, ctx);
  // A first level-1 heading is the title when it says the file's name, or the file has a Notion id.
  const first = blocks[0];
  if (title === undefined && isObj(first) && first.type === "heading" && isObj(first.attrs) && first.attrs.level === 1) {
    const heading = titleOf(plainOf(Array.isArray(first.content) ? first.content : []));
    if (heading !== "" && (source.notionId !== undefined || heading.toLowerCase() === source.name.trim().toLowerCase())) {
      title = heading;
      blocks.shift();
    }
  }
  return { title, icon, cover, coverY, template, templatePlaceholders, blocks, values };
}

function paragraphOf(lines: readonly string[]): JsonValue {
  const content: JsonValue[] = [];
  lines.forEach((line, at) => {
    if (at > 0) {
      content.push({ type: "hardBreak" });
    }
    content.push({ type: "text", text: line });
  });
  return { type: "paragraph", content };
}

function convertText(text: string): Converted {
  return { title: undefined, icon: undefined, blocks: textParagraphs(text).map(paragraphOf), values: [] };
}

function convertHtml(source: Source, text: string, ctx: Context): Converted {
  const html = htmlText(text);
  if (html.dropped) {
    ctx.warn(source.path, "unsupportedContent", "html");
  }
  return {
    title: html.title === undefined ? undefined : titleOf(html.title),
    icon: html.icon !== undefined && isEmoji(html.icon) ? html.icon : undefined,
    blocks: html.paragraphs.map((line) => paragraphOf([line])),
    values: [],
  };
}

// ----- CSV --------------------------------------------------------------------------------------

const URL_CELL = /^(?:https?:\/\/|mailto:)\S+$/iu;
const EMAIL_CELL = /^[^\s@,]+@[^\s@,]+\.[^\s@,]+$/u;
/** One item of a Notion relation cell: `Title (path.md)`. */
const RELATION_ITEM = /(.+?) \(([^()]*?\.(?:md|markdown|html|htm|csv|txt))\)(?:, |$)/gu;

/** The pages a Notion relation cell names, when it is made of nothing else. */
function relationPages(cell: string, dir: string, ctx: Context): Source[] | undefined {
  const found: Source[] = [];
  let consumed = 0;
  for (const match of cell.matchAll(RELATION_ITEM)) {
    if (match.index !== consumed) {
      return undefined;
    }
    consumed = match.index + match[0].length;
    const path = resolveRelative(dir, decodePath(match[2] ?? ""));
    const page = path === undefined ? undefined : (ctx.byPath.get(path) ?? ctx.byPathLower.get(path.toLowerCase()));
    if (page === undefined) {
      return undefined;
    }
    found.push(page);
  }
  return found.length > 0 && consumed === cell.length ? found : undefined;
}

/** The kind a CSV column has, from its non-empty cells (3.6). */
function inferKind(cells: readonly string[], rows: number, dir: string, ctx: Context): PropertyKind {
  if (cells.length === 0) {
    return "text";
  }
  if (cells.every((cell) => parseBool(cell) !== undefined)) {
    return "checkbox";
  }
  if (cells.every((cell) => parseNumber(cell) !== undefined)) {
    return "number";
  }
  if (cells.every((cell) => parseDate(cell) !== undefined)) {
    return "date";
  }
  if (cells.every((cell) => URL_CELL.test(cell))) {
    return "url";
  }
  if (cells.every((cell) => EMAIL_CELL.test(cell))) {
    return "email";
  }
  if (cells.every((cell) => relationPages(cell, dir, ctx) !== undefined)) {
    return "relation";
  }
  const split = cells.map((cell) => cell.split(", "));
  const items = new Set(split.flat());
  if (split.some((parts) => parts.length >= 2) && items.size <= 50 && [...items].every((item) => item.length <= 60)) {
    return "multi_select";
  }
  const distinct = new Set(cells);
  if (
    distinct.size <= 50 &&
    [...distinct].every((cell) => cell.length <= 60 && !/[\r\n]/u.test(cell)) &&
    (distinct.size < cells.length || rows <= 3)
  ) {
    return "select";
  }
  return "text";
}

/** Header names made unique (` (2)`) and non-empty (`Column N`). */
function headerNames(header: readonly string[]): string[] {
  const seen = new Set<string>();
  return header.map((raw, at) => {
    const base = raw.trim() === "" ? `Column ${String(at + 1)}` : raw.trim();
    let name = base;
    for (let n = 2; seen.has(name.toLowerCase()); n += 1) {
      name = `${base} (${String(n)})`;
    }
    seen.add(name.toLowerCase());
    return name;
  });
}

function cellOf(kind: PropertyKind, text: string, dir: string, ctx: Context): Cell {
  switch (kind) {
    case "checkbox":
      return { text, checked: parseBool(text) ?? false };
    case "number":
      return { text, number: parseNumber(text) ?? 0 };
    case "select":
      return { text, items: [text] };
    case "multi_select":
      return { text, items: text.split(", ") };
    case "relation":
      return { text, pages: (relationPages(text, dir, ctx) ?? []).map((page) => ({ key: page.key })) };
    default:
      return { text };
  }
}

/**
 * First pass over a CSV database (3.6): reads its records, makes a child page for every row and
 * hands each row the page file of its title (Notion's row pages), so that links to those files,
 * and wiki links to their titles, reach the row. The file's own children move to the row.
 */
function matchRows(source: Source, text: string, ctx: Context): void {
  const rows = parseCsv(text).rows;
  const names = headerNames(rows[0] ?? []);
  const lowered = names.map((name) => name.toLowerCase());
  const titleAt = Math.max(
    0,
    lowered.findIndex((name) => name === "name" || name === "title"),
  );
  const data = rows.slice(1).map((row) => names.map((_, at) => row[at] ?? ""));
  source.csv = { names, titleAt, data };
  source.columnNames = new Set(lowered.filter((_, at) => at !== titleAt));
  const candidates = source.children.filter(
    (child) => child.kind === "markdown" || child.kind === "text" || child.kind === "html",
  );
  const matched = new Set<Source>();
  const made: Source[] = [];
  // The page files by title, in order: a row takes the first one that is left.
  const waiting = new Map<string, Source[]>();
  for (const candidate of candidates) {
    const same = waiting.get(candidate.name.trim()) ?? [];
    same.push(candidate);
    waiting.set(candidate.name.trim(), same);
  }
  for (const row of data) {
    const rowTitle = titleOf(row[titleAt] ?? "") || "Untitled";
    const page = waiting.get(rowTitle)?.shift();
    const item = newSource(ctx.nextKey(), "row", page?.path ?? "", rowTitle);
    item.rowTitle = rowTitle;
    item.parent = source;
    if (page !== undefined) {
      matched.add(page);
      item.file = page;
      item.notionId = page.notionId;
      item.folderKey = page.folderKey;
      item.children = page.children;
      for (const child of item.children) {
        child.parent = item;
      }
      ctx.byPath.set(page.path, item);
      ctx.byPathLower.set(page.path.toLowerCase(), item);
      if (page.notionId !== undefined) {
        ctx.byNotion.set(page.notionId, item);
      }
      for (const [title, target] of ctx.byTitle) {
        if (target === page) {
          ctx.byTitle.set(title, item);
        }
      }
    }
    made.push(item);
  }
  source.children = [...made, ...source.children.filter((child) => !matched.has(child))];
}

/**
 * Second pass over a CSV database: every column but the title's becomes a planned property (its
 * kind inferred from its cells), every row its values, and the page itself one `pageList` block (a
 * table of its children with the columns in order).
 */
function convertCsv(source: Source, ctx: Context): void {
  const csv = source.csv;
  if (csv === undefined) {
    return;
  }
  const { names, titleAt, data } = csv;
  const columns: { name: string; at: number; kind: PropertyKind; property: number }[] = [];
  names.forEach((name, at) => {
    if (at === titleAt) {
      return;
    }
    const cells = data.map((row) => (row[at] ?? "").trim()).filter((cell) => cell !== "");
    const kind = inferKind(cells, data.length, source.dir, ctx);
    const options =
      kind === "select" ? [...new Set(cells)] : kind === "multi_select" ? [...new Set(cells.flatMap((cell) => cell.split(", ")))] : [];
    columns.push({ name, at, kind, property: ctx.registry.add(name, kind, options) });
  });
  source.blocks = [
    {
      type: "pageList",
      attrs: { source: { kind: "children" }, layout: "table", properties: columns.map((column) => `prop:${column.name}`) },
    },
  ];
  const rows = source.children.filter((child) => child.kind === "row");
  data.forEach((record, index) => {
    const row = rows[index];
    for (const column of columns) {
      const raw = (record[column.at] ?? "").trim();
      if (row !== undefined && raw !== "") {
        row.values.push({ property: column.property, cell: cellOf(column.kind, raw, source.dir, ctx) });
      }
    }
  });
}

// ----- the planner -------------------------------------------------------------------------------

const PAGE_EXTENSIONS: Record<string, SourceKind> = {
  ".md": "markdown",
  ".markdown": "markdown",
  ".txt": "text",
  ".html": "html",
  ".htm": "html",
  ".csv": "csv",
};

function parentDir(dir: string): string {
  const slash = dir.lastIndexOf("/");
  return slash < 0 ? "" : dir.slice(0, slash);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The properties and the page order an atomik-kb archive's sidecar carries; nothing for text that is not one. */
function readSidecar(text: string | undefined): { properties: PlannedProperty[]; pages: string[] } {
  const none = { properties: [], pages: [] };
  if (text === undefined) {
    return none;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return none;
  }
  if (!isRecord(parsed)) {
    return none;
  }
  const properties: PlannedProperty[] = [];
  for (const entry of Array.isArray(parsed.properties) ? (parsed.properties as unknown[]) : []) {
    if (!isRecord(entry)) {
      continue;
    }
    const kind = PROPERTY_KINDS.find((candidate) => candidate === entry.kind);
    const name = typeof entry.name === "string" ? entry.name.trim() : "";
    if (name === "" || kind === undefined) {
      continue;
    }
    const planned: PlannedProperty = {
      name,
      kind,
      options: [],
      builtin: name.toLowerCase() === TAGS_NAME.toLowerCase(),
      fixed: true,
    };
    for (const option of Array.isArray(entry.options) ? (entry.options as unknown[]) : []) {
      if (!isRecord(option) || typeof option.name !== "string" || option.name.trim() === "") {
        continue;
      }
      const color = OPTION_COLORS.find((candidate) => candidate === option.color);
      const group = STATUS_GROUPS.find((candidate) => candidate === option.group);
      planned.options.push({
        name: option.name.trim(),
        ...(color === undefined ? {} : { color }),
        ...(group === undefined ? {} : { group }),
      });
    }
    const config = entry.config;
    if (kind === "relation" && isRecord(config)) {
      planned.relation = {
        twoWay: config.twoWay === true,
        ...(typeof config.pairName === "string" ? { pairName: config.pairName } : {}),
      };
    } else if (kind === "rollup" && isRecord(config)) {
      const fn = ROLLUP_FUNCTIONS.find((candidate) => candidate === config.function);
      const target = config.target === "title" ? "title" : isRecord(config.target) ? config.target.property : undefined;
      if (typeof config.relation === "string" && typeof target === "string" && fn !== undefined) {
        planned.rollup = { relation: config.relation, target, function: fn };
      }
    } else if (kind === "formula") {
      // Kept as written: the core makes the names the ids of the target space, and keeps a text that does not resolve.
      const expression = isRecord(config) ? config.expression : undefined;
      if (typeof expression !== "string" || expression.trim() === "") {
        continue;
      }
      planned.formula = { expression };
    }
    properties.push(planned);
  }
  const pages = Array.isArray(parsed.pages) ? (parsed.pages as unknown[]).filter((page): page is string => typeof page === "string") : [];
  return { properties, pages };
}

/**
 * Plans an import (see the module comment). Never throws: what cannot be read is left out with a
 * warning, and the plan is as complete as the source allows.
 */
export function planImport(input: PlanInput): ImportPlan {
  const warnings: ImportWarning[] = [];

  // Normalise: when every path is inside one folder, that folder is stripped.
  const everyPath = [...input.files.keys(), ...input.binaryPaths];
  const first = everyPath[0]?.split("/")[0];
  const wrapper =
    first !== undefined && everyPath.every((path) => path.includes("/") && path.split("/")[0] === first) ? first : "";
  const normalise = (path: string): string => (wrapper === "" ? path : path.slice(wrapper.length + 1));
  const given = new Map<string, string>();
  const binaries = new Map<string, string>();
  for (const path of input.binaryPaths) {
    given.set(normalise(path), path);
    binaries.set(normalise(path), path);
  }
  const texts = new Map<string, string>();
  for (const [path, text] of input.files) {
    given.set(normalise(path), path);
    texts.set(normalise(path), text);
  }
  const shown = (path: string): string => given.get(path) ?? path;
  const warn = (path: string, code: ImportWarningCode, detail?: string): void => {
    warnings.push(detail === undefined ? { path: shown(path), code } : { path: shown(path), code, detail });
  };

  // An AppFlowy backup (ADR-0039) has its own reader.
  if (isAppFlowyArchive(texts)) {
    return planAppFlowy({ texts, binaries, shown, registry: new Registry(), maxPages: input.maxPages });
  }

  let nextKey = 0;
  const ctx: Context = {
    byPath: new Map(),
    byPathLower: new Map(),
    byNotion: new Map(),
    byTitle: new Map(),
    binaries,
    binariesLower: new Map([...binaries.keys()].map((path) => [path.toLowerCase(), path])),
    needed: new Map(),
    placeholders: new Map(),
    registry: new Registry(),
    nextKey: () => {
      nextKey += 1;
      return nextKey - 1;
    },
    warn,
  };

  const sidecar = readSidecar(texts.get(SIDECAR_PATH));
  for (const property of sidecar.properties) {
    ctx.registry.addFixed(property);
  }
  const rank = new Map(sidecar.pages.map((path, at) => [path, at]));

  // The pages: one per file the importer reads.
  const sources: Source[] = [];
  const aliases: [string, string][] = [];
  const paths = [...texts.keys()].sort();
  const csvPaths = new Set(paths.filter((path) => path.toLowerCase().endsWith(".csv")));
  for (const path of paths) {
    const split = splitPath(path);
    const kind = PAGE_EXTENSIONS[split.ext];
    if (path === SIDECAR_PATH || split.ext === ".excalidraw") {
      continue;
    }
    if (kind === undefined) {
      warn(path, "unsupportedFile");
      continue;
    }
    const base = path.slice(0, path.length - split.ext.length);
    // Notion writes a database twice (`X <id>.csv` and `X <id>_all.csv`): the `_all` one is read.
    if (kind === "csv" && !split.stem.endsWith("_all") && csvPaths.has(`${base}_all${path.slice(base.length)}`)) {
      // Links to the plain file reach the database all the same.
      aliases.push([path, `${base}_all${path.slice(base.length)}`]);
      continue;
    }
    const stem = kind === "csv" && split.stem.endsWith("_all") ? split.stem.slice(0, -4) : split.stem;
    const cleaned = cleanName(stem);
    const source = newSource(ctx.nextKey(), kind, path, cleaned.name);
    source.folderKey = stem;
    source.notionId = cleaned.notionId;
    source.text = texts.get(path);
    sources.push(source);
    ctx.byPath.set(path, source);
    ctx.byPathLower.set(path.toLowerCase(), source);
    if (cleaned.notionId !== undefined) {
      ctx.byNotion.set(cleaned.notionId, source);
    }
  }
  for (const [path, target] of aliases) {
    const database = ctx.byPath.get(target);
    if (database !== undefined) {
      ctx.byPath.set(path, database);
      ctx.byPathLower.set(path.toLowerCase(), database);
    }
  }
  // The titles a wiki link may name: the front matter's title, and the name.
  for (const source of sources) {
    const names = [source.name];
    if (source.kind === "markdown" && source.text !== undefined) {
      const { yaml } = splitFrontMatter(source.text);
      const named = yaml === undefined ? undefined : parseYaml(yaml)?.find(([key]) => key.toLowerCase() === "title")?.[1];
      if (typeof named === "string" && named.trim() !== "") {
        names.unshift(named.trim());
      }
    }
    for (const name of names) {
      if (!ctx.byTitle.has(name.toLowerCase())) {
        ctx.byTitle.set(name.toLowerCase(), source);
      }
    }
  }

  // The tree: a page owns the folder of its name; a folder without such a page is a page.
  const owners = new Map<string, Source>();
  for (const source of sources) {
    const folder = `${source.dir === "" ? "" : `${source.dir}/`}${source.folderKey}`;
    if (!owners.has(folder)) {
      owners.set(folder, source);
    }
  }
  const folders = new Map<string, Source>();
  const top: Source[] = [];
  const attach = (child: Source, parent: Source | null): void => {
    child.parent = parent;
    if (parent === null) {
      top.push(child);
    } else {
      parent.children.push(child);
    }
  };
  const parentOf = (dir: string): Source | null => {
    if (dir === "") {
      return null;
    }
    const owner = owners.get(dir);
    if (owner !== undefined) {
      return owner;
    }
    let folder = folders.get(dir);
    if (folder === undefined) {
      folder = newSource(ctx.nextKey(), "folder", dir, cleanName(dir.split("/").at(-1) ?? dir).name);
      folder.dir = parentDir(dir);
      folders.set(dir, folder);
      attach(folder, parentOf(parentDir(dir)));
    }
    return folder;
  };
  for (const source of sources) {
    attach(source, parentOf(source.dir));
  }

  // CSV databases: the rows take their page files first, then the columns and values are read;
  // then the content of every page.
  const databases = sources.filter((source) => source.kind === "csv" && source.text !== undefined);
  for (const database of databases) {
    matchRows(database, database.text ?? "", ctx);
  }
  for (const database of databases) {
    convertCsv(database, ctx);
  }
  const everything: Source[] = [];
  const collect = (source: Source): void => {
    everything.push(source);
    source.children.forEach(collect);
  };
  top.forEach(collect);
  for (const source of everything) {
    // A row reads its page file (when Notion exported one); everything else reads itself.
    const holder = source.kind === "row" ? source.file : source;
    let converted: Converted | undefined;
    if (holder?.text !== undefined) {
      if (holder.kind === "markdown") {
        converted = convertMarkdown(source, holder.text, ctx, source.kind === "row" ? source.parent?.columnNames : undefined);
      } else if (holder.kind === "text") {
        converted = convertText(holder.text);
      } else if (holder.kind === "html") {
        converted = convertHtml(source, holder.text, ctx);
      }
    }
    source.title = source.kind === "row" ? (source.rowTitle ?? source.name) : (converted?.title ?? source.name);
    if (converted !== undefined) {
      source.blocks = converted.blocks;
      source.icon = converted.icon;
      source.cover = converted.cover;
      source.coverY = converted.coverY;
      source.template = converted.template;
      source.templatePlaceholders = converted.templatePlaceholders;
      source.values = [...source.values, ...converted.values];
    }
  }

  // Several top-level pages of a named source go under a container named after it.
  let roots = top;
  if (input.sourceName !== undefined && top.length > 1) {
    const container = newSource(ctx.nextKey(), "container", "", cleanName(wrapper === "" ? input.sourceName : wrapper).name);
    container.children = top;
    for (const child of top) {
      child.parent = container;
    }
    roots = [container];
  }

  // Siblings in order, parents before children.
  const ranked = sidecar.pages.length > 0;
  const ordered = (children: readonly Source[], parent: Source | null): Source[] => {
    const position = new Map(children.map((child, at) => [child, at]));
    const linked = new Map((parent?.links ?? []).map((target, at) => [target, at]));
    return [...children].sort((a, b) => {
      if (parent?.kind === "csv") {
        const left = a.kind === "row" ? 0 : 1;
        const right = b.kind === "row" ? 0 : 1;
        if (left !== right) {
          return left - right;
        }
        if (left === 0) {
          return (position.get(a) ?? 0) - (position.get(b) ?? 0);
        }
      }
      if (ranked) {
        const x = rank.get(a.path) ?? Number.POSITIVE_INFINITY;
        const y = rank.get(b.path) ?? Number.POSITIVE_INFINITY;
        if (x !== y) {
          return x < y ? -1 : 1;
        }
      } else if (linked.size > 0) {
        const x = linked.get(a) ?? -1;
        const y = linked.get(b) ?? -1;
        if (x >= 0 || y >= 0) {
          return x < 0 ? 1 : y < 0 ? -1 : x - y;
        }
      }
      return naturalCompare(a.name, b.name) || a.key - b.key;
    });
  };
  const flat: Source[] = [];
  const visit = (source: Source): void => {
    flat.push(source);
    ordered(source.children, source).forEach(visit);
  };
  ordered(roots, null).forEach(visit);

  let kept = flat;
  if (flat.length > input.maxPages) {
    kept = flat.slice(0, input.maxPages);
    warn(flat[input.maxPages]?.path ?? "", "truncated");
  }
  const at = new Map(kept.map((source, index) => [source, index]));
  const pages: PlannedPage[] = kept.map((source) => ({
    key: source.key,
    parent: source.parent === null ? null : (at.get(source.parent) ?? null),
    title: titleOf(source.title) || "Untitled",
    icon: source.icon,
    cover: source.cover,
    coverY: source.coverY,
    template: source.template,
    templatePlaceholders: source.templatePlaceholders,
    blocks: source.blocks,
    values: source.values,
    path: shown(source.path),
  }));
  return {
    pages,
    properties: ctx.registry.properties,
    neededFiles: [...ctx.needed.keys()].map((path) => binaries.get(path) ?? path),
    placeholders: ctx.placeholders,
    warnings,
    spaceRoots: [],
    remoteFiles: [],
    fileUsers: new Map(),
  };
}
