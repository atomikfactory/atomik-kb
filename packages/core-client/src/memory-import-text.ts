// Small readers for the import mirror (ADR-0036, `docs/architecture/import-export.md`, 3): names
// as Notion writes them, the order of siblings, the front matter of a Markdown file (a subset of
// YAML: `key: value` lines, quoted and plain scalars, inline and block lists, one level of maps),
// the value parsers a CSV column or a front matter key goes through (`values::parse_bool`,
// `parse_number`, `parse_date`), and the text of an HTML file. Pure functions; no DOM, so that
// they run in Node tests as in the browser.

import type { DateValue } from "./generated/DateValue";
import { isIsoDate, isIsoTime } from "./page-doc";

// ----- names and order ------------------------------------------------------------------------

/** A Notion id at the end of a name: a space and 32 lowercase hex digits. */
const NOTION_ID = / ([0-9a-f]{32})$/u;

/** A name with the Notion id removed, and the id (`undefined` when the name has none). */
export function cleanName(stem: string): { name: string; notionId: string | undefined } {
  const match = NOTION_ID.exec(stem);
  if (match === null) {
    return { name: stem, notionId: undefined };
  }
  return { name: stem.slice(0, stem.length - match[0].length), notionId: match[1] };
}

/** Natural order of display names: case-insensitive, runs of digits compared as numbers. */
export function naturalCompare(a: string, b: string): number {
  const left = a.toLowerCase().match(/\d+|\D+/gu) ?? [];
  const right = b.toLowerCase().match(/\d+|\D+/gu) ?? [];
  for (let at = 0; at < Math.min(left.length, right.length); at += 1) {
    const x = left[at] ?? "";
    const y = right[at] ?? "";
    if (x === y) {
      continue;
    }
    if (/^\d/u.test(x) && /^\d/u.test(y)) {
      const difference = BigInt(x) - BigInt(y);
      if (difference !== 0n) {
        return difference < 0n ? -1 : 1;
      }
      continue;
    }
    return x < y ? -1 : 1;
  }
  return left.length - right.length || (a < b ? -1 : a > b ? 1 : 0);
}

/** The parts of a path: its folder (without a trailing slash), the file's stem and its extension (with the dot, lowercase). */
export function splitPath(path: string): { dir: string; stem: string; ext: string; file: string } {
  const slash = path.lastIndexOf("/");
  const file = path.slice(slash + 1);
  const dir = slash < 0 ? "" : path.slice(0, slash);
  const dot = file.lastIndexOf(".");
  return dot <= 0
    ? { dir, stem: file, ext: "", file }
    : { dir, stem: file.slice(0, dot), ext: file.slice(dot).toLowerCase(), file };
}

/** The path of a link destination relative to the folder of the file it is in; `undefined` when it leaves the source. */
export function resolveRelative(dir: string, destination: string): string | undefined {
  const parts = dir === "" ? [] : dir.split("/");
  for (const segment of destination.split("/")) {
    if (segment === "" || segment === ".") {
      continue;
    }
    if (segment === "..") {
      if (parts.length === 0) {
        return undefined;
      }
      parts.pop();
    } else {
      parts.push(segment);
    }
  }
  return parts.join("/");
}

/** Percent-decoding that leaves a malformed sequence as it is. */
export function decodePath(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

// ----- text, titles and icons -------------------------------------------------------------------

/** A page title from source text: control characters are spaces, trimmed, at most 500 characters. */
export function titleOf(text: string): string {
  return Array.from(text.replace(/\p{Cc}/gu, " ").trim()).slice(0, 500).join("").trim();
}

/** Whether `text` is an emoji (or a short symbol run) fit for an icon: no letters or digits. */
export function isEmoji(text: string): boolean {
  return (
    text !== "" &&
    new TextEncoder().encode(text).length <= 32 &&
    /^[\p{Extended_Pictographic}\p{Regional_Indicator}\p{M}\p{Cf}\u{20E3}\u{FE0F}]+$/u.test(text) &&
    /[\p{Extended_Pictographic}\p{Regional_Indicator}]/u.test(text)
  );
}

// ----- value parsers ----------------------------------------------------------------------------

/** `parse_bool`: `true`, `false`, `yes`, `no` in any case (what Notion and our CSV write for a checkbox). */
export function parseBool(text: string): boolean | undefined {
  switch (text.trim().toLowerCase()) {
    case "true":
    case "yes":
      return true;
    case "false":
    case "no":
      return false;
    default:
      return undefined;
  }
}

/** `parse_number`: a plain decimal number (sign, digits, fraction, exponent); no thousands separators. */
export function parseNumber(text: string): number | undefined {
  const trimmed = text.trim();
  if (!/^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/u.test(trimmed)) {
    return undefined;
  }
  const value = Number(trimmed);
  return Number.isFinite(value) ? value : undefined;
}

const MONTHS = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

/**
 * `parse_date`: `YYYY-MM-DD`, optionally followed by a time (`HH:mm`, seconds, a `Z` or an offset
 * ignored), or Notion's `October 12, 2026` with an optional `9:30 AM`. `undefined` for anything else.
 */
export function parseDate(text: string): DateValue | undefined {
  const trimmed = text.trim();
  const iso = /^(\d{4}-\d{2}-\d{2})(?:[T ](\d{1,2}):(\d{2})(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?$/u.exec(trimmed);
  if (iso !== null) {
    return dateOf(iso[1] ?? "", iso[2], iso[3]);
  }
  const notion = /^([A-Za-z]+) (\d{1,2}), (\d{4})(?: (\d{1,2}):(\d{2}) ?(AM|PM))?$/u.exec(trimmed);
  if (notion !== null) {
    const month = MONTHS.findIndex((name) => name.startsWith((notion[1] ?? "").toLowerCase()) && (notion[1] ?? "").length >= 3);
    if (month < 0) {
      return undefined;
    }
    let hour = notion[4];
    if (hour !== undefined && notion[6] !== undefined) {
      const clock = Number(hour) % 12;
      hour = String(notion[6] === "PM" ? clock + 12 : clock);
    }
    const date = `${notion[3] ?? ""}-${String(month + 1).padStart(2, "0")}-${(notion[2] ?? "").padStart(2, "0")}`;
    return dateOf(date, hour, notion[5]);
  }
  return undefined;
}

function dateOf(date: string, hour: string | undefined, minute: string | undefined): DateValue | undefined {
  if (!isIsoDate(date)) {
    return undefined;
  }
  if (hour === undefined || minute === undefined) {
    return { date };
  }
  const time = `${hour.padStart(2, "0")}:${minute}`;
  return isIsoTime(time) ? { date, time } : undefined;
}

// ----- front matter -----------------------------------------------------------------------------

/** A YAML value of the subset: scalars, lists and one level of maps. */
export type YamlValue = string | number | boolean | null | YamlValue[] | { [key: string]: YamlValue };

/** The front matter split off a Markdown file. */
export interface FrontMatter {
  /** The YAML between the fences; `undefined` when the file has none. */
  yaml: string | undefined;
  /** The rest of the file. */
  body: string;
}

/**
 * A first line `---` (a byte order mark allowed) closes at the next `---` or `...` line. Without a
 * closing line the whole text is body.
 */
export function splitFrontMatter(text: string): FrontMatter {
  const lines = text.replace(/^\u{FEFF}/u, "").split(/\r\n|\r|\n/u);
  if ((lines[0] ?? "").trimEnd() !== "---") {
    return { yaml: undefined, body: text };
  }
  for (let at = 1; at < lines.length; at += 1) {
    const line = (lines[at] ?? "").trimEnd();
    if (line === "---" || line === "...") {
      return { yaml: lines.slice(1, at).join("\n"), body: lines.slice(at + 1).join("\n") };
    }
  }
  return { yaml: undefined, body: text };
}

/** A scalar: a double-quoted JSON string, a single-quoted string, or plain text typed as YAML types it. */
function scalarOf(raw: string): YamlValue | undefined {
  const text = raw.trim();
  if (text.startsWith('"')) {
    try {
      const parsed: unknown = JSON.parse(text);
      return typeof parsed === "string" ? parsed : undefined;
    } catch {
      return undefined;
    }
  }
  if (text.startsWith("'")) {
    return text.length >= 2 && text.endsWith("'") ? text.slice(1, -1).replaceAll("''", "'") : undefined;
  }
  const plain = text.replace(/\s+#.*$/u, "");
  if (plain === "" || plain === "~" || plain === "null") {
    return null;
  }
  if (plain === "true") {
    return true;
  }
  if (plain === "false") {
    return false;
  }
  if (/^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/u.test(plain)) {
    return Number(plain);
  }
  return plain;
}

/** An inline list `[a, "b, c", d]`; `undefined` when it does not close. */
function inlineList(raw: string): YamlValue[] | undefined {
  const text = raw.trim();
  if (!text.startsWith("[") || !text.endsWith("]")) {
    return undefined;
  }
  const items: string[] = [];
  let current = "";
  let quote: string | undefined;
  for (const char of text.slice(1, -1)) {
    if (quote !== undefined) {
      current += char;
      if (char === quote && !current.endsWith(`\\${quote}`)) {
        quote = undefined;
      }
    } else if (char === '"' || char === "'") {
      quote = char;
      current += char;
    } else if (char === ",") {
      items.push(current);
      current = "";
    } else {
      current += char;
    }
  }
  if (quote !== undefined) {
    return undefined;
  }
  if (current.trim() !== "" || items.length > 0) {
    items.push(current);
  }
  const values: YamlValue[] = [];
  for (const item of items) {
    const value = scalarOf(item);
    if (value === undefined) {
      return undefined;
    }
    values.push(value);
  }
  return values;
}

/** A key: a double- or single-quoted string, or plain text up to the colon. */
function keyOf(line: string): { key: string; rest: string } | undefined {
  const quoted = /^("(?:[^"\\]|\\.)*"|'(?:[^']|'')*')\s*:(?:\s+(.*)|\s*)$/u.exec(line);
  if (quoted !== null) {
    const key = scalarOf(quoted[1] ?? "");
    return typeof key === "string" ? { key, rest: quoted[2] ?? "" } : undefined;
  }
  const plain = /^([^\s:#'"[\]{}][^:#]*?)\s*:(?:\s+(.*)|\s*)$/u.exec(line);
  return plain === null ? undefined : { key: plain[1] ?? "", rest: plain[2] ?? "" };
}

/**
 * Reads the YAML of a front matter block: top-level `key: value` lines whose value is a scalar, an
 * inline list, or (on the lines below, indented) a block list or a map of scalars. Comments and
 * blank lines are skipped. `undefined` when a line is outside the subset (the caller drops the block).
 */
export function parseYaml(yaml: string): [string, YamlValue][] | undefined {
  const lines = yaml.split(/\r\n|\r|\n/u).filter((line) => line.trim() !== "" && !/^\s*#/u.test(line));
  const entries: [string, YamlValue][] = [];
  for (let at = 0; at < lines.length; at += 1) {
    const line = lines[at] ?? "";
    const entry = /^\S/u.test(line) ? keyOf(line) : undefined;
    if (entry === undefined) {
      return undefined;
    }
    const rest = entry.rest.trim();
    if (rest !== "") {
      const list = inlineList(rest);
      const value = list ?? scalarOf(rest);
      if (value === undefined) {
        return undefined;
      }
      entries.push([entry.key, value]);
      continue;
    }
    const below: string[] = [];
    while (at + 1 < lines.length && /^\s/u.test(lines[at + 1] ?? "")) {
      at += 1;
      below.push((lines[at] ?? "").trim());
    }
    if (below.length === 0) {
      entries.push([entry.key, null]);
    } else if (below.every((item) => item === "-" || item.startsWith("- "))) {
      const items: YamlValue[] = [];
      for (const item of below) {
        const value = scalarOf(item.slice(1));
        if (value === undefined) {
          return undefined;
        }
        items.push(value);
      }
      entries.push([entry.key, items]);
    } else {
      const map: { [key: string]: YamlValue } = {};
      for (const item of below) {
        const pair = keyOf(item);
        const value = pair === undefined ? undefined : scalarOf(pair.rest);
        if (pair === undefined || value === undefined) {
          return undefined;
        }
        map[pair.key] = value;
      }
      entries.push([entry.key, map]);
    }
  }
  return entries;
}

// ----- plain text and HTML ----------------------------------------------------------------------

/** Plain text: line ends normalised, blank lines separate paragraphs, single breaks stay (hard breaks). */
export function textParagraphs(text: string): string[][] {
  const paragraphs: string[][] = [];
  let current: string[] = [];
  for (const line of text.replace(/^\u{FEFF}/u, "").split(/\r\n|\r|\n/u)) {
    if (line.trim() === "") {
      if (current.length > 0) {
        paragraphs.push(current);
        current = [];
      }
    } else {
      current.push(line.trim());
    }
  }
  if (current.length > 0) {
    paragraphs.push(current);
  }
  return paragraphs;
}

const NAMED_ENTITIES: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };

/** Decodes the entities HTML text commonly holds: the named five, `&nbsp;` and numeric ones. */
export function decodeEntities(text: string): string {
  return text.replace(/&(?:#(\d+)|#x([0-9a-fA-F]+)|([a-zA-Z]+));/gu, (whole, decimal?: string, hex?: string, name?: string) => {
    if (name !== undefined) {
      return NAMED_ENTITIES[name.toLowerCase()] ?? whole;
    }
    const code = decimal === undefined ? Number.parseInt(hex ?? "", 16) : Number(decimal);
    return Number.isInteger(code) && code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
  });
}

/** What an HTML file reads as: its title and icon, and its text as paragraphs. */
export interface HtmlText {
  title: string | undefined;
  /** Notion's `.page-header-icon` text (the caller keeps it only when it is an emoji). */
  icon: string | undefined;
  paragraphs: string[];
  /** An element the page schema cannot hold was dropped (`script`, `style`, `iframe`, ...). */
  dropped: boolean;
}

/**
 * The text of an HTML file: the title (`h1.page-title`, else `<title>`, else the first `<h1>`; an
 * `<h1>` that gave it is not part of the text) and the icon (`.page-header-icon`), then the body's
 * text with block-level elements (and `<br>`) ending a paragraph. Scripts, styles and frames are
 * dropped with their content, other tags are removed and entities decoded.
 */
export function htmlText(html: string): HtmlText {
  let dropped = false;
  let source = html.replace(/<!--[\s\S]*?-->/gu, "");
  source = source.replace(/<(script|style|iframe|object|embed|noscript|template|svg|canvas|form)\b[\s\S]*?<\/\1\s*>/giu, () => {
    dropped = true;
    return "";
  });
  const plain = (markup: string | undefined): string | undefined =>
    markup === undefined ? undefined : decodeEntities(markup.replace(/<[^>]*>/gu, "")).replace(/\s+/gu, " ").trim();
  /** The inner text of the first match of `pattern` (its last group), which is taken out of the source. */
  const take = (pattern: RegExp): string | undefined => {
    const match = pattern.exec(source);
    if (match === null) {
      return undefined;
    }
    source = source.replace(match[0], "");
    return plain(match[match.length - 1]);
  };
  const icon = take(/<(\w+)\b[^>]*class="[^"]*\bpage-header-icon\b[^"]*"[^>]*>([\s\S]*?)<\/\1\s*>/iu);
  let title = take(/<h1\b[^>]*class="[^"]*\bpage-title\b[^"]*"[^>]*>([\s\S]*?)<\/h1\s*>/iu);
  title ??= plain(/<title\b[^>]*>([\s\S]*?)<\/title\s*>/iu.exec(source)?.[1]);
  title ??= take(/<h1\b[^>]*>([\s\S]*?)<\/h1\s*>/iu);
  let body = /<body\b[^>]*>([\s\S]*)<\/body\s*>/iu.exec(source)?.[1] ?? source;
  body = body.replace(/<head\b[\s\S]*?<\/head\s*>/giu, "").replace(/<title\b[\s\S]*?<\/title\s*>/giu, "");
  body = body
    .replace(/<br\s*\/?>/giu, "\n")
    .replace(
      /<\/?(?:p|div|h[1-6]|li|ul|ol|tr|table|blockquote|pre|section|article|header|footer|main|aside|details|summary|figure|figcaption|hr|nav)\b[^>]*>/giu,
      "\n",
    );
  const paragraphs = decodeEntities(body.replace(/<[^>]*>/gu, ""))
    .split(/\n/u)
    .map((line) => line.replace(/\s+/gu, " ").trim())
    .filter((line) => line !== "");
  return { title: title === "" ? undefined : title, icon: icon === "" ? undefined : icon, paragraphs, dropped };
}
