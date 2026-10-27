// The texts of an export (ADR-0036, `docs/architecture/import-export.md`, 2.4 to 2.8), the
// in-memory twin of `kb_export`'s `markdown`, `html`, `excalidraw`, `sidecar` and `csv` modules,
// with reduced fidelity: the Markdown body is the clipboard flavour of `renderMarkdown`
// (`./markdown`: no escaping, no annotations), the HTML document is a compact rendering of the
// common blocks, and the sidecar lists the schema by names. Pure functions over what
// `./memory-export` gathers.

import { formulaToDisplay } from "./formula";
import type { AttachmentId } from "./generated/AttachmentId";
import type { FormulaResult } from "./generated/FormulaResult";
import type { OptionId } from "./generated/OptionId";
import type { PageId } from "./generated/PageId";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyValue } from "./generated/PropertyValue";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { formulaSchemaOf } from "./memory-formula";
import { parsePortable } from "./page-doc";
import type { PortableNode } from "./page-doc";
import { TAGS_PROPERTY_ID, isComputedPropertyKind } from "./properties";
import { sortZ } from "./whiteboard";
import type { WhiteboardElement } from "./whiteboard";

// ----- values ---------------------------------------------------------------------------------

/** A property value of a page with the names it needs resolved (`kb_export::ExportValue`). */
export type ExportValue =
  | { kind: "text"; text: string }
  | { kind: "number"; value: number }
  | { kind: "checkbox"; value: boolean }
  | { kind: "date"; date: string; time?: string }
  | { kind: "options"; names: string[]; multi: boolean }
  | { kind: "pages"; titles: string[] }
  | { kind: "files"; names: string[]; paths: string[] }
  | { kind: "checklist"; items: { text: string; done: boolean }[] }
  | { kind: "place"; name: string; lat?: number; lon?: number }
  | { kind: "instant"; ms: number }
  | { kind: "device"; name: string }
  | { kind: "computed"; text: string };

/** What {@link exportValue} resolves ids with. */
export interface ValueLookups {
  /** The name of an option of the space's schema. */
  optionName: (optionId: OptionId) => string | undefined;
  /** The title of a page. */
  pageTitle: (pageId: PageId) => string | undefined;
  /** The stored file name of an attachment. */
  fileName: (attachmentId: AttachmentId) => string | undefined;
  /** The path of an attachment's file, relative to the page's file, when it is in the archive. */
  filePath: (attachmentId: AttachmentId) => string | undefined;
  /** The name of a device. */
  deviceName: (deviceId: string) => string | undefined;
  /** The current name of a member of the space; `undefined` for an account the space does not list (a former member). */
  personName?: (accountId: string) => string | undefined;
}

/** What an export writes for a person who is no longer a member (the core's text, English). */
export const FORMER_MEMBER = "Former member";

/** `ExportValue::text` of a date: `YYYY-MM-DD` or `YYYY-MM-DD HH:mm`. */
function dateText(date: string, time: string | undefined): string {
  return time === undefined ? date : `${date} ${time}`;
}

/** Rust's shortest `Display` of a number: plain decimal digits, never an exponent. */
export function rustNumber(value: number): string {
  const text = String(value);
  const match = /^(-?)(\d+)(?:\.(\d+))?e([+-]\d+)$/u.exec(text);
  if (match === null) {
    return text;
  }
  const [, sign = "", whole = "", fraction = "", exponent = "0"] = match;
  const digits = `${whole}${fraction}`;
  const point = whole.length + Number(exponent);
  if (point <= 0) {
    return `${sign}0.${"0".repeat(-point)}${digits}`;
  }
  if (point >= digits.length) {
    return `${sign}${digits}${"0".repeat(point - digits.length)}`;
  }
  return `${sign}${digits.slice(0, point)}.${digits.slice(point)}`;
}

/** The text of one value of a rollup result, as the CSV shows it. */
function plainValueText(value: PropertyValue, look: ValueLookups): string {
  const resolved = exportValue(value, look);
  return resolved === undefined ? "" : valueText(resolved, localMinute);
}

/**
 * Resolves a stored value: option ids to names, pages to titles, attachments to names and paths,
 * devices to names. `undefined` for a value that reads as nothing.
 */
export function exportValue(value: PropertyValue, look: ValueLookups): ExportValue | undefined {
  switch (value.kind) {
    case "text":
    case "url":
    case "email":
    case "phone":
      return value.value === "" ? undefined : { kind: "text", text: value.value };
    case "number":
      return { kind: "number", value: value.value };
    case "checkbox":
      return { kind: "checkbox", value: value.value };
    case "date":
      return value.value.time === undefined
        ? { kind: "date", date: value.value.date }
        : { kind: "date", date: value.value.date, time: value.value.time };
    case "select":
    case "status": {
      const name = look.optionName(value.value);
      return name === undefined ? undefined : { kind: "options", names: [name], multi: false };
    }
    case "multi_select": {
      const names = value.value.map((id) => look.optionName(id)).filter((name): name is string => name !== undefined);
      return names.length === 0 ? undefined : { kind: "options", names, multi: true };
    }
    case "relation": {
      const titles = value.value.map((id) => look.pageTitle(id)).filter((title): title is string => title !== undefined);
      return titles.length === 0 ? undefined : { kind: "pages", titles };
    }
    case "files": {
      const names: string[] = [];
      const paths: string[] = [];
      for (const id of value.value) {
        const name = look.fileName(id);
        if (name !== undefined) {
          names.push(name);
          paths.push(look.filePath(id) ?? name);
        }
      }
      return names.length === 0 ? undefined : { kind: "files", names, paths };
    }
    case "checklist":
      return value.value.length === 0
        ? undefined
        : { kind: "checklist", items: value.value.map((item) => ({ text: item.text, done: item.done })) };
    case "place":
      return value.value.lat !== undefined && value.value.lon !== undefined
        ? { kind: "place", name: value.value.name, lat: value.value.lat, lon: value.value.lon }
        : { kind: "place", name: value.value.name };
    case "person": {
      // The people's names as the options of a multi-select, like the core's export: "Former member" for an account the
      // space does not list, never an account id.
      const names = value.value.map((id) => look.personName?.(id) ?? FORMER_MEMBER);
      return names.length === 0 ? undefined : { kind: "options", names, multi: true };
    }
    case "created_time":
    case "last_edited_time":
      return { kind: "instant", ms: value.value };
    case "created_by":
    case "last_edited_by": {
      const name = look.deviceName(value.value);
      return name === undefined ? undefined : { kind: "device", name };
    }
    case "formula":
      return formulaExport(value.value, look);
    case "rollup": {
      const result = value.value.result;
      switch (result.type) {
        case "number":
          return { kind: "computed", text: rustNumber(result.data) };
        case "date":
          return { kind: "computed", text: dateText(result.data.date, result.data.time) };
        case "progress":
          return { kind: "computed", text: `${String(result.data.done)}/${String(result.data.total)}` };
        case "values":
          return { kind: "computed", text: result.data.map((item) => plainValueText(item, look)).filter((text) => text !== "").join(", ") };
        default:
          return undefined;
      }
    }
  }
}

/**
 * A formula's result as the exporters write it (ADR-0055, `kb-core`'s `export_formula`): a number, text, boolean (as a
 * checkbox) or date as the kind it is, a page by its title, a person by name ("Former member" for one the space no
 * longer lists); a list of pages or people as those kinds' lists, any other list joined with `, `.
 */
function formulaExport(result: FormulaResult, look: ValueLookups): ExportValue | undefined {
  const person = (account: string): string => look.personName?.(account) ?? FORMER_MEMBER;
  switch (result.type) {
    case "number":
      return { kind: "number", value: result.data };
    case "text":
      return { kind: "text", text: result.data };
    case "boolean":
      return { kind: "checkbox", value: result.data };
    case "date":
      return result.data.time === undefined
        ? { kind: "date", date: result.data.date }
        : { kind: "date", date: result.data.date, time: result.data.time };
    case "page": {
      const title = look.pageTitle(result.data);
      return title === undefined ? undefined : { kind: "pages", titles: [title] };
    }
    case "person":
      return { kind: "options", names: [person(result.data)], multi: true };
    case "list": {
      const first = result.data[0];
      if (first === undefined) {
        return undefined;
      }
      if (first.type === "page") {
        const titles = result.data.flatMap((item) => (item.type === "page" ? (look.pageTitle(item.data) ?? []) : []));
        return titles.length === 0 ? undefined : { kind: "pages", titles };
      }
      if (first.type === "person") {
        return { kind: "options", names: result.data.flatMap((item) => (item.type === "person" ? [person(item.data)] : [])), multi: true };
      }
      const texts = result.data.flatMap((item) => {
        const exported = formulaExport(item, look);
        return exported === undefined ? [] : [valueText(exported, localMinute)];
      });
      return texts.length === 0 ? undefined : { kind: "text", text: texts.join(", ") };
    }
  }
}

/** Local `YYYY-MM-DD HH:mm` with this device's offset (what CSV shows for created and edited times). */
export function localMinute(ms: number): string {
  const when = new Date(ms);
  const two = (n: number): string => String(n).padStart(2, "0");
  return `${String(when.getFullYear()).padStart(4, "0")}-${two(when.getMonth() + 1)}-${two(when.getDate())} ${two(when.getHours())}:${two(when.getMinutes())}`;
}

/** `ExportValue::text` (2.8): the text a CSV cell holds. */
export function valueText(value: ExportValue, instant: (ms: number) => string = localMinute): string {
  switch (value.kind) {
    case "text":
    case "computed":
      return value.text;
    case "number":
      return rustNumber(value.value);
    case "checkbox":
      return value.value ? "Yes" : "No";
    case "date":
      return dateText(value.date, value.time);
    case "options":
      return value.names.join(", ");
    case "pages":
      return value.titles.join(", ");
    case "files":
      return value.names.join(", ");
    case "checklist":
      return value.items.map((item) => `${item.done ? "[x]" : "[ ]"} ${item.text}`).join("; ");
    case "place":
      return value.lat !== undefined && value.lon !== undefined
        ? `${value.name} (${rustNumber(value.lat)}, ${rustNumber(value.lon)})`
        : value.name;
    case "instant":
      return instant(value.ms);
    case "device":
      return value.name;
  }
}

// ----- front matter ---------------------------------------------------------------------------

/** What the front matter of a page holds. */
export interface FrontMatterInput {
  title: string;
  /** The icon as written (an emoji or `icon:<name>`); left out when `undefined`. */
  icon: string | undefined;
  /**
   * The cover as written (ADR-0040): `color:<name>` or `gradient:<name>`, or the relative path of the
   * file of an uploaded one; left out when `undefined`.
   */
  cover?: string | undefined;
  /** The cover's vertical position in percent; written as `cover_y` unless it is the default 50. */
  coverY?: number | undefined;
  /** The page is a template (ADR-0056); written as `template: true`. */
  template?: boolean | undefined;
  /**
   * The placeholders of a page in a template (ADR-0056), by the property's name, in schema order; written as the mapping
   * `templatePlaceholders`. Left out when empty.
   */
  templatePlaceholders?: readonly { name: string; placeholder: TemplatePlaceholder }[] | undefined;
  createdAt: number;
  updatedAt: number;
  /** The values in schema order; the built-in Tags is the one with `TAGS_PROPERTY_ID`. */
  values: { property: PropertyDef; value: ExportValue | undefined }[];
}

/** A YAML double-quoted scalar written with JSON escapes (valid YAML). */
function quoted(text: string): string {
  return JSON.stringify(text);
}

/** An instant as RFC 3339 UTC without fractional seconds. */
function rfc3339(ms: number): string {
  return new Date(ms).toISOString().replace(/\.\d{3}Z$/u, "Z");
}

/** One property as the lines of its key: scalars on the key's line, lists and places below it. */
function yamlEntry(key: string, value: ExportValue): string[] | undefined {
  const list = (items: readonly string[]): string[] => [`${key}:`, ...items.map((item) => `  - ${quoted(item)}`)];
  switch (value.kind) {
    case "text":
      return [`${key}: ${quoted(value.text)}`];
    case "number":
      return [`${key}: ${rustNumber(value.value)}`];
    case "checkbox":
      return [`${key}: ${value.value ? "true" : "false"}`];
    case "date":
      return [`${key}: ${quoted(dateText(value.date, value.time))}`];
    case "options":
      return value.multi ? list(value.names) : [`${key}: ${quoted(value.names.join(", "))}`];
    case "pages":
      return list(value.titles.map((title) => `[[${title}]]`));
    case "files":
      return list(value.paths);
    case "checklist":
      return list(value.items.map((item) => `${item.done ? "[x]" : "[ ]"} ${item.text}`));
    case "place":
      return [
        `${key}:`,
        `  name: ${quoted(value.name)}`,
        ...(value.lat !== undefined && value.lon !== undefined
          ? [`  lat: ${rustNumber(value.lat)}`, `  lon: ${rustNumber(value.lon)}`]
          : []),
      ];
    default:
      // Computed values (times, devices, rollups) are never written.
      return undefined;
  }
}

/**
 * The keys of the front matter that are not properties, and the ones the importer ignores (`kb_export`'s `RESERVED_KEYS`,
 * lower case: they are compared in any case): a property of the same name is written as `<name> (2)`.
 */
const RESERVED_KEYS = [
  "title",
  "icon",
  "cover",
  "cover_y",
  "template",
  "templateplaceholders",
  "created",
  "updated",
  "tags",
  "aliases",
  "cssclasses",
  "cssclass",
  "publish",
  "permalink",
  "layout",
];

/** `name` or, when it is taken (in any case: the importer reads keys in lower case), `name (2)`, `name (3)`, ...; the key is claimed. */
function uniqueKey(used: Set<string>, name: string): string {
  let key = name;
  for (let number = 2; used.has(key.toLowerCase()); number += 1) {
    key = `${name} (${String(number)})`;
  }
  used.add(key.toLowerCase());
  return key;
}

/**
 * The front matter block of a page (2.4): `title` always, `icon` and `cover` (with `cover_y`) when set, `template: true`
 * and the mapping `templatePlaceholders` (property name to `today`, `now` or `me`) for a template (ADR-0056),
 * `created` and `updated`,
 * `tags`, then the other properties in schema order under their names (double-quoted keys; a name that is a key above, or
 * one written before it, becomes `<name> (2)`);
 * empty and computed values are not written. Ends with the closing `---` line and a newline.
 */
export function frontMatter(input: FrontMatterInput): string {
  const lines = ["---", `title: ${quoted(input.title)}`];
  if (input.icon !== undefined) {
    lines.push(`icon: ${quoted(input.icon)}`);
  }
  if (input.cover !== undefined && input.cover !== "") {
    lines.push(`cover: ${quoted(input.cover)}`);
    if (input.coverY !== undefined && Number.isInteger(input.coverY) && input.coverY >= 0 && input.coverY <= 100 && input.coverY !== 50) {
      lines.push(`cover_y: ${String(input.coverY)}`);
    }
  }
  if (input.template === true) {
    lines.push("template: true");
  }
  if (input.templatePlaceholders !== undefined && input.templatePlaceholders.length > 0) {
    lines.push("templatePlaceholders:", ...input.templatePlaceholders.map((entry) => `  ${quoted(entry.name)}: ${quoted(entry.placeholder)}`));
  }
  lines.push(`created: ${quoted(rfc3339(input.createdAt))}`, `updated: ${quoted(rfc3339(input.updatedAt))}`);
  const tags = input.values.find((entry) => entry.property.id === TAGS_PROPERTY_ID)?.value;
  if (tags?.kind === "options") {
    lines.push("tags:", ...tags.names.map((name) => `  - ${quoted(name)}`));
  }
  const used = new Set(RESERVED_KEYS);
  for (const { property, value } of input.values) {
    if (property.id === TAGS_PROPERTY_ID || value === undefined || isComputedPropertyKind(property.kind)) {
      continue;
    }
    lines.push(...(yamlEntry(quoted(uniqueKey(used, property.name)), value) ?? []));
  }
  lines.push("---");
  return `${lines.join("\n")}\n`;
}

// ----- HTML -----------------------------------------------------------------------------------

/** What the HTML renderer resolves links with. */
export interface HtmlLinks {
  /** The `href` of another page of the export, relative to this file; `undefined` outside the export. */
  pageHref: (pageId: PageId) => string | undefined;
  /** The `href` of an attachment's file in the archive; `undefined` when it is not there. */
  attachmentHref: (attachmentId: AttachmentId) => string | undefined;
  /** The stored name of an attachment. */
  attachmentName: (attachmentId: AttachmentId) => string | undefined;
  /** The title of a page. */
  pageTitle: (pageId: PageId) => string | undefined;
  /** The `href` of the `.excalidraw` file of the page's n-th whiteboard (0-based). */
  whiteboardHref: (index: number) => string | undefined;
}

const ENTITIES: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

/** Escapes text and attribute values: `& < > " '`. */
export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/gu, (char) => ENTITIES[char] ?? char);
}

/** The schemes of a `link` mark's href that are kept (anything else leaves the text alone). */
const SAFE_LINK = /^(?:https?:|mailto:|tel:)/iu;

function attrString(node: PortableNode, name: string): string | undefined {
  const value = node.attrs?.[name];
  return typeof value === "string" ? value : undefined;
}

function attrNumber(node: PortableNode, name: string): number | undefined {
  const value = node.attrs?.[name];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** The level of a toggle heading, the `level` of its summary when it is 1, 2 or 3; `undefined` for a plain toggle (ADR-0040). */
function summaryLevel(summary: PortableNode | undefined): number | undefined {
  const level = summary?.attrs?.level;
  return level === 1 || level === 2 || level === 3 ? level : undefined;
}

/** The plain text of a node's subtree. */
function plainText(node: PortableNode): string {
  if (node.type === "text") {
    return node.text ?? "";
  }
  if (node.type === "hardBreak") {
    return "\n";
  }
  return node.content.map(plainText).join("");
}

/** The headings of a page (toggle headings included), in order, with the ids their `<h>` tags carry. */
function headingsOf(node: PortableNode): { id: string; text: string }[] {
  const found: { id: string; text: string }[] = [];
  const visit = (current: PortableNode): void => {
    if (current.type === "heading" || (current.type === "detailsSummary" && summaryLevel(current) !== undefined)) {
      found.push({ id: `h-${attrString(current, "id") ?? String(found.length)}`, text: plainText(current) });
    }
    current.content.forEach(visit);
  };
  visit(node);
  return found;
}

function textHtml(node: PortableNode): string {
  let html = escapeHtml(node.text ?? "");
  const names = new Set(node.marks.map((mark) => mark.type));
  if (names.has("code")) {
    html = `<code>${html}</code>`;
  }
  if (names.has("strike")) {
    html = `<s>${html}</s>`;
  }
  if (names.has("underline")) {
    html = `<u>${html}</u>`;
  }
  if (names.has("italic")) {
    html = `<em>${html}</em>`;
  }
  if (names.has("bold")) {
    html = `<strong>${html}</strong>`;
  }
  for (const mark of node.marks) {
    const color = typeof mark.attrs?.color === "string" ? escapeHtml(mark.attrs.color) : undefined;
    if (mark.type === "highlight") {
      html = `<mark${color === undefined ? "" : ` data-color="${color}"`}>${html}</mark>`;
    } else if (mark.type === "textColor") {
      html = `<span class="text-color"${color === undefined ? "" : ` data-color="${color}"`}>${html}</span>`;
    }
  }
  const link = node.marks.find((mark) => mark.type === "link");
  const href = typeof link?.attrs?.href === "string" ? link.attrs.href : undefined;
  if (href !== undefined && SAFE_LINK.test(href)) {
    html = `<a href="${escapeHtml(href)}">${html}</a>`;
  }
  return html;
}

function inlineHtml(nodes: readonly PortableNode[], links: HtmlLinks): string {
  return nodes
    .map((node) => {
      switch (node.type) {
        case "text":
          return textHtml(node);
        case "hardBreak":
          return "<br>";
        case "pageRef": {
          const pageId = attrString(node, "pageId") ?? "";
          const title = escapeHtml(links.pageTitle(pageId) ?? "");
          const href = links.pageHref(pageId);
          return href === undefined
            ? `<span class="page-ref">${title}</span>`
            : `<a class="page-ref" href="${escapeHtml(href)}">${title}</a>`;
        }
        case "dateMention": {
          const date = attrString(node, "date") ?? "";
          const time = attrString(node, "time");
          const moment = time === undefined ? date : `${date}T${time}`;
          return `<time datetime="${escapeHtml(moment)}">${escapeHtml(dateText(date, time))}</time>`;
        }
        case "mathInline":
          return `<span class="math math-inline">\\(${escapeHtml(attrString(node, "latex") ?? "")}\\)</span>`;
        default:
          return inlineHtml(node.content, links);
      }
    })
    .join("");
}

/** Whether a node is inline content (text, a page reference, a date, an equation, a break). */
function isInlineNode(node: PortableNode): boolean {
  return ["text", "pageRef", "dateMention", "mathInline", "hardBreak"].includes(node.type);
}

/** Children as blocks: runs of inline nodes among blocks become paragraphs. */
function childrenHtml(children: readonly PortableNode[], links: HtmlLinks, root: PortableNode): string {
  let html = "";
  let run: PortableNode[] = [];
  const flush = (): void => {
    if (run.length > 0) {
      html += `<p>${inlineHtml(run, links)}</p>\n`;
      run = [];
    }
  };
  for (const child of children) {
    if (isInlineNode(child)) {
      run.push(child);
    } else {
      flush();
      html += blockHtml(child, links, root);
    }
  }
  flush();
  return html;
}

function figureHtml(
  attachmentId: string,
  alt: string,
  caption: string | undefined,
  layout: { align?: string | undefined; percent?: number | undefined },
  links: HtmlLinks,
): string {
  const href = links.attachmentHref(attachmentId);
  const picture = href === undefined ? "" : `<img src="${escapeHtml(href)}" alt="${escapeHtml(alt)}">`;
  const text = caption === undefined || caption === "" ? "" : `<figcaption>${escapeHtml(caption)}</figcaption>`;
  const align = layout.align === undefined ? "" : ` align-${escapeHtml(layout.align)}`;
  const style = layout.percent === undefined ? "" : ` style="width:${String(layout.percent)}%"`;
  return `<figure class="image${align}"${style}>${picture}${text}</figure>\n`;
}

const MEDIA_EXTENSIONS = /\.(mp4|webm|ogv|mov|m4v|mp3|wav|ogg|oga|m4a|flac)$/iu;

function blockHtml(node: PortableNode, links: HtmlLinks, root: PortableNode): string {
  switch (node.type) {
    case "paragraph":
      return `<p>${inlineHtml(node.content, links)}</p>\n`;
    case "heading": {
      const level = Math.min(3, Math.max(1, attrNumber(node, "level") ?? 1));
      const id = attrString(node, "id");
      return `<h${String(level)}${id === undefined ? "" : ` id="h-${escapeHtml(id)}"`}>${inlineHtml(node.content, links)}</h${String(level)}>\n`;
    }
    case "bulletList":
      return `<ul>\n${childrenHtml(node.content, links, root)}</ul>\n`;
    case "orderedList": {
      const start = attrNumber(node, "start");
      return `<ol${start === undefined || start === 1 ? "" : ` start="${String(start)}"`}>\n${childrenHtml(node.content, links, root)}</ol>\n`;
    }
    case "taskList":
      return `<ul class="task-list">\n${childrenHtml(node.content, links, root)}</ul>\n`;
    case "listItem":
      return `<li>${childrenHtml(node.content, links, root).trimEnd()}</li>\n`;
    case "taskItem": {
      const checked = node.attrs?.checked === true;
      return `<li class="task" data-checked="${String(checked)}"><input type="checkbox" disabled${checked ? " checked" : ""}> ${childrenHtml(node.content, links, root).trimEnd()}</li>\n`;
    }
    case "blockquote":
      return `<blockquote>\n${childrenHtml(node.content, links, root)}</blockquote>\n`;
    case "callout": {
      const color = attrString(node, "color");
      return `<aside class="callout"${color === undefined ? "" : ` data-color="${escapeHtml(color)}"`}><span class="callout-icon">${escapeHtml(attrString(node, "icon") ?? "💡")}</span><div>\n${childrenHtml(node.content, links, root)}</div></aside>\n`;
    }
    case "codeBlock": {
      const language = attrString(node, "language");
      return `<pre><code${language === undefined || language === "" ? "" : ` class="language-${escapeHtml(language)}"`}>${escapeHtml(plainText(node))}</code></pre>\n`;
    }
    case "mathBlock":
      return `<div class="math math-display">\\[${escapeHtml(plainText(node))}\\]</div>\n`;
    case "horizontalRule":
      return "<hr>\n";
    case "table":
      return `<table>\n${node.content.map((row) => blockHtml(row, links, root)).join("")}</table>\n`;
    case "tableRow":
      return `<tr>${node.content.map((cell) => blockHtml(cell, links, root)).join("")}</tr>\n`;
    case "tableCell":
    case "tableHeader": {
      const tag = node.type === "tableHeader" ? "th" : "td";
      return `<${tag}>${childrenHtml(node.content, links, root).trimEnd()}</${tag}>`;
    }
    case "details": {
      const summary = node.content.find((child) => child.type === "detailsSummary");
      const body = node.content.find((child) => child.type === "detailsContent");
      // A toggle heading puts the summary's text in a heading element, with the id the table of contents links to.
      const level = summaryLevel(summary);
      const text = escapeHtml(summary === undefined ? "" : plainText(summary));
      const id = summary === undefined ? undefined : attrString(summary, "id");
      const inside =
        level === undefined
          ? text
          : `<h${String(level)}${id === undefined ? "" : ` id="h-${escapeHtml(id)}"`}>${text}</h${String(level)}>`;
      return `<details${node.attrs?.open === true ? " open" : ""}><summary>${inside}</summary>\n${body === undefined ? "" : childrenHtml(body.content, links, root)}</details>\n`;
    }
    case "columnList":
      return `<div class="columns" style="--columns:${String(node.content.length)}">\n${node.content.map((column) => `<div class="column">\n${childrenHtml(column.content, links, root)}</div>\n`).join("")}</div>\n`;
    case "image": {
      return figureHtml(
        attrString(node, "attachmentId") ?? "",
        attrString(node, "alt") ?? "",
        attrString(node, "caption"),
        { align: attrString(node, "align"), percent: attrNumber(node, "widthPercent") },
        links,
      );
    }
    case "file": {
      const id = attrString(node, "attachmentId") ?? "";
      const name = links.attachmentName(id) ?? id;
      const href = links.attachmentHref(id);
      if (href !== undefined && attrString(node, "view") === "preview" && MEDIA_EXTENSIONS.test(name)) {
        const tag = /\.(mp3|wav|ogg|oga|m4a|flac)$/iu.test(name) ? "audio" : "video";
        return `<${tag} controls src="${escapeHtml(href)}"></${tag}>\n`;
      }
      return href === undefined
        ? `<p><span class="file">${escapeHtml(name)}</span></p>\n`
        : `<p><a class="file" href="${escapeHtml(href)}">${escapeHtml(name)}</a></p>\n`;
    }
    case "gallery": {
      const items = Array.isArray(node.attrs?.items) ? node.attrs.items : [];
      const pictures = items
        .map((item) =>
          typeof item === "object" && item !== null && !Array.isArray(item) && typeof item.attachmentId === "string"
            ? figureHtml(item.attachmentId, "", typeof item.caption === "string" ? item.caption : undefined, {}, links)
            : "",
        )
        .join("");
      return `<div class="gallery" style="--columns:${String(attrNumber(node, "columns") ?? 3)}">\n${pictures}</div>\n`;
    }
    case "whiteboard": {
      const href = links.whiteboardHref(whiteboardIndex(root, node));
      return `<figure class="whiteboard">${href === undefined ? "Whiteboard" : `<a href="${escapeHtml(href)}">Whiteboard</a>`}</figure>\n`;
    }
    case "pageLink":
      return `<p class="page-link">${inlineHtml([{ ...node, type: "pageRef" }], links)}</p>\n`;
    case "tableOfContents": {
      const items = headingsOf(root)
        .map((heading) => `<li><a href="#${escapeHtml(heading.id)}">${escapeHtml(heading.text)}</a></li>`)
        .join("\n");
      return `<nav class="toc"><ul>\n${items}\n</ul></nav>\n`;
    }
    case "breadcrumb":
    case "pageList":
      return "";
    case "templateButton":
      return ""; // template material (ADR-0056)
    default:
      return isInlineNode(node) ? `<p>${inlineHtml([node], links)}</p>\n` : childrenHtml(node.content, links, root);
  }
}

/** The position of a whiteboard block among the page's whiteboards, in document order. */
function whiteboardIndex(root: PortableNode, board: PortableNode): number {
  let index = 0;
  let found = -1;
  const visit = (node: PortableNode): void => {
    if (node.type === "whiteboard") {
      if (node === board) {
        found = index;
      }
      index += 1;
    }
    node.content.forEach(visit);
  };
  visit(root);
  return found;
}

const STYLE = `body{font:16px/1.6 system-ui,sans-serif;margin:2rem;color:#1f2328;background:#fff}
article.page{max-width:none}
.page-title{font-size:2rem;margin:0 0 1rem}
table{border-collapse:collapse}td,th{border:1px solid #d0d7de;padding:.3rem .6rem}
.properties th{text-align:left;font-weight:600}
pre{overflow:auto;background:#f6f8fa;padding:.8rem}
.callout{display:flex;gap:.6rem;padding:.8rem;background:#f6f8fa;border-radius:6px}
.page-cover{display:block;width:100%;height:clamp(160px,30vh,280px);object-fit:cover;margin:0 0 1.5rem;background:#f6f8fa}
.page-cover[data-color="gray"]{background:#d9d8d4}.page-cover[data-color="brown"]{background:#d8c3ba}.page-cover[data-color="orange"]{background:#f5c996}
.page-cover[data-color="yellow"]{background:#f1dc8f}.page-cover[data-color="green"]{background:#b4d9b9}.page-cover[data-color="blue"]{background:#a6cfe8}
.page-cover[data-color="purple"]{background:#cdbbe6}.page-cover[data-color="pink"]{background:#eab4cd}.page-cover[data-color="red"]{background:#f0aeb4}
.page-cover[data-gradient="sunset"]{background:linear-gradient(135deg,#f6d365,#fda085)}.page-cover[data-gradient="dawn"]{background:linear-gradient(135deg,#a18cd1,#fbc2eb)}
.page-cover[data-gradient="ocean"]{background:linear-gradient(135deg,#2193b0,#6dd5ed)}.page-cover[data-gradient="forest"]{background:linear-gradient(135deg,#134e5e,#71b280)}
.page-cover[data-gradient="lavender"]{background:linear-gradient(135deg,#cfd9df,#e2d1f9)}.page-cover[data-gradient="peach"]{background:linear-gradient(135deg,#ffecd2,#fcb69f)}
.page-cover[data-gradient="mint"]{background:linear-gradient(135deg,#84fab0,#8fd3f4)}.page-cover[data-gradient="slate"]{background:linear-gradient(135deg,#485563,#29323c)}
.page-cover[data-gradient="candy"]{background:linear-gradient(135deg,#ff9a9e,#fad0c4)}.page-cover[data-gradient="aurora"]{background:linear-gradient(135deg,#43e97b,#38f9d7)}
blockquote{margin-left:0;padding-left:1rem;border-left:3px solid #d0d7de}
figure img{max-width:100%}
.columns{display:grid;grid-template-columns:repeat(var(--columns),1fr);gap:1rem}
.gallery{display:grid;grid-template-columns:repeat(var(--columns),1fr);gap:.5rem}
@media (prefers-color-scheme:dark){body{color:#e6edf3;background:#0d1117}pre,.callout{background:#161b22}td,th{border-color:#30363d}}
@media print{body{margin:0}}`;

/** What {@link htmlPage} writes besides the blocks. */
export interface HtmlPageInput {
  title: string;
  icon: string | undefined;
  /** The cover (ADR-0040): `color:<name>`, `gradient:<name>` or the relative link to the file of a picture. */
  cover?: string | undefined;
  /** The cover's vertical position in percent (50 when absent). */
  coverY?: number | undefined;
  /** The page's values with their names, for the header table. */
  properties: { name: string; text: string }[];
  /** The page's portable JSON (an empty page is `{ "type": "doc" }`). */
  content: JsonValue;
}

/**
 * A standalone HTML document for a page (2.5, compact): `<!doctype html>`, an embedded style, and
 * `<article class="page">` with the icon, the title, the property table and the blocks. No script,
 * no external resource; every text and attribute value is escaped, and links of the `link` mark
 * are kept only for `http`, `https`, `mailto` and `tel`.
 */
export function htmlPage(input: HtmlPageInput, links: HtmlLinks): string {
  const root = parsePortable(input.content);
  const table =
    input.properties.length === 0
      ? ""
      : `<table class="properties">\n${input.properties.map((row) => `<tr><th>${escapeHtml(row.name)}</th><td>${escapeHtml(row.text)}</td></tr>`).join("\n")}\n</table>\n`;
  const icon = input.icon === undefined ? "" : `<div class="page-header-icon">${escapeHtml(input.icon)}</div>\n`;
  const cover = coverHtml(input.cover, input.coverY);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="atomik-kb">
<title>${escapeHtml(input.title)}</title>
<style>
${STYLE}
</style>
</head>
<body>
<article class="page">
${cover}<header>
${icon}<h1 class="page-title">${escapeHtml(input.title)}</h1>
${table}</header>
${childrenHtml(root.content, links, root)}</article>
</body>
</html>
`;
}

/** The cover banner of a page (ADR-0040); nothing when there is no cover or it links to nothing. */
function coverHtml(cover: string | undefined, coverY: number | undefined): string {
  if (cover === undefined || cover === "") {
    return "";
  }
  if (cover.startsWith("color:")) {
    return `<div class="page-cover" data-color="${escapeHtml(cover.slice("color:".length))}"></div>\n`;
  }
  if (cover.startsWith("gradient:")) {
    return `<div class="page-cover" data-gradient="${escapeHtml(cover.slice("gradient:".length))}"></div>\n`;
  }
  if (!cover.includes(":") && (cover.includes("/") || cover.includes("."))) {
    const y = coverY !== undefined && Number.isInteger(coverY) && coverY >= 0 && coverY <= 100 ? coverY : 50;
    return `<img class="page-cover" src="${escapeHtml(cover)}" alt="" style="object-position:50% ${String(y)}%">\n`;
  }
  return "";
}

// ----- whiteboards and the sidecar ---------------------------------------------------------------

/**
 * The `.excalidraw` file of a whiteboard (2.6): the live elements (not `isDeleted`) in z-order,
 * pretty-printed.
 */
export function excalidrawFile(elements: Record<string, JsonValue>): string {
  const list: WhiteboardElement[] = [];
  for (const [id, value] of Object.entries(elements)) {
    if (typeof value === "object" && value !== null && !Array.isArray(value) && value.isDeleted !== true) {
      list.push({ id, value });
    }
  }
  return `${JSON.stringify(
    {
      type: "excalidraw",
      version: 2,
      source: "atomik-kb",
      elements: sortZ(list).map((element) => element.value),
      appState: { viewBackgroundColor: "#ffffff" },
      files: {},
    },
    null,
    2,
  )}\n`;
}

/** The format of the sidecar file `atomik-export.json`. */
export const SIDECAR_NAME = "atomik-export.json";

/**
 * The sidecar (2.7): the whole schema of the space by names (kinds, options with colours and
 * groups, relations and rollups) and the page files in tree order.
 */
export function sidecarFile(input: {
  name: string;
  exportedAt: number;
  schema: readonly PropertyDef[];
  pages: readonly string[];
}): string {
  const nameOf = (id: string): string | undefined => input.schema.find((property) => property.id === id)?.name;
  const properties = input.schema.map((property) => {
    const entry: { [key: string]: JsonValue } = { name: property.name, kind: property.kind };
    if (property.options.length > 0) {
      entry.options = property.options.map((option) => ({
        name: option.name,
        color: option.color,
        ...(option.group === undefined ? {} : { group: option.group }),
      }));
    }
    const config = property.config;
    if (config?.kind === "relation") {
      const pair = config.pairedPropertyId === undefined ? undefined : nameOf(config.pairedPropertyId);
      entry.config = { twoWay: config.twoWay, ...(pair === undefined ? {} : { pairName: pair }) };
    } else if (config?.kind === "rollup") {
      const target = config.target === "title" ? "title" : { property: nameOf(config.target) ?? "" };
      entry.config = { relation: nameOf(config.relationPropertyId) ?? "", target, function: config.function };
    } else if (config?.kind === "formula") {
      // The expression as a person reads it: names for the ids of `prop("...")` (an id the schema lacks stays).
      entry.config = { expression: formulaToDisplay(config.expression, formulaSchemaOf(input.schema)) };
    }
    return entry;
  });
  return `${JSON.stringify(
    {
      format: 1,
      generator: "atomik-kb",
      name: input.name,
      exportedAt: rfc3339(input.exportedAt),
      properties,
      pages: input.pages,
    },
    null,
    2,
  )}\n`;
}
