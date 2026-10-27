// The rules of the formula editor (ADR-0055, `docs/architecture/formulas.md` section 9) that need no React: the schema
// and the host the engine reads (built from the property schema and the page tree), the analysis of the text (display to
// stored, check, and where an error is in what the person sees), the highlighted mirror, the completion, the translated
// errors and the reference's descriptions. Positions that come from the engine count code points; the textarea counts
// UTF-16 units: `toUtf16` and `toCodePoints` convert.

import {
  checkFormula,
  formulaToStored,
  isFormulaError,
  tokenizeFormula,
} from "@atomik-kb/core-client";
import type {
  AccountId,
  FormulaEnv,
  FormulaError,
  FormulaFunctionInfo,
  FormulaHost,
  FormulaProperty,
  FormulaSchema,
  FormulaToken,
  FormulaType,
  PageId,
  PageInfo,
  PropertyDef,
  PropertyValue,
  TreeView,
} from "@atomik-kb/core-client";

import { t, tp } from "../i18n";
import type { MessageKey } from "../i18n";
import { KIND_LABELS } from "../property-format";

// ----- the schema and the host --------------------------------------------------------------------

/** The properties of a space as the engine reads them, in schema order (a rollup brings its function and its target's kind). */
export function formulaSchemaOf(defs: readonly PropertyDef[]): FormulaSchema {
  const properties: FormulaProperty[] = defs.map((def) => {
    const base = { id: def.id, name: def.name, kind: def.kind, options: def.options.map((option) => ({ id: option.id, name: option.name })) };
    if (def.config?.kind === "formula") {
      return { ...base, expression: def.config.expression };
    }
    if (def.config?.kind === "rollup") {
      const { target } = def.config;
      const targetKind = target === "title" ? null : defs.find((candidate) => candidate.id === target)?.kind;
      // A rollup whose target is gone has no function to read.
      return targetKind === undefined ? base : { ...base, rollup: { function: def.config.function, targetKind } };
    }
    return base;
  });
  const byId = new Map(properties.map((property) => [property.id, property] as const));
  return {
    property: (id) => byId.get(id),
    properties: () => properties,
  };
}

/** The live pages of a tree by id (the pages in the trash are not in `roots`). */
export function pagesOfTree(tree: TreeView | null): ReadonlyMap<PageId, PageInfo> {
  const pages = new Map<PageId, PageInfo>();
  const visit = (nodes: TreeView["roots"]): void => {
    for (const node of nodes) {
      pages.set(node.page.id, node.page);
      visit(node.children);
    }
  };
  if (tree !== null) {
    visit(tree.roots);
  }
  return pages;
}

/** What the preview reads besides the schema: the live pages of the space and the people it lists. */
export interface PreviewData {
  readonly pages: ReadonlyMap<PageId, PageInfo>;
  readonly people: readonly { readonly accountId: AccountId; readonly name: string }[];
}

/**
 * The host the editor's preview evaluates over: values, titles, times and relations from the pages the UI holds
 * (`PageInfo.properties` carries stored values and rollups; the engine computes formulas itself, so a formula's value is
 * left out), the people by the names the space gives them (their e-mail is not known here).
 */
export function buildFormulaHost(schema: FormulaSchema, data: PreviewData): FormulaHost {
  const valueOf = (page: PageId, property: string): PropertyValue | undefined =>
    data.pages.get(page)?.properties?.find(([id]) => id === property)?.[1];
  return {
    property: (id) => schema.property(id),
    properties: () => schema.properties(),
    title: (page) => data.pages.get(page)?.title,
    value: (page, property) => {
      const value = valueOf(page, property);
      return value?.kind === "formula" ? undefined : value;
    },
    related: (page, relation) => {
      const value = valueOf(page, relation);
      return value?.kind === "relation" ? value.value.filter((id) => data.pages.has(id)) : [];
    },
    createdMs: (page) => data.pages.get(page)?.createdAt,
    editedMs: (page) => data.pages.get(page)?.updatedAt,
    person: (account) => {
      const person = data.people.find((candidate) => candidate.accountId === account);
      return person === undefined ? undefined : { name: person.name, email: "" };
    },
    fileName: () => undefined,
  };
}

/** The time of a read, on this device's clock. */
export function previewEnv(now: Date = new Date()): FormulaEnv {
  return { nowMs: now.getTime(), offsetSeconds: -now.getTimezoneOffset() * 60 };
}

/**
 * The page the preview runs on: the first of `candidates` that is still in the tree (a table offers its rows), else the
 * page in front, else the first page of the space; `null` when the space has none.
 */
export function choosePreviewPage(
  pages: ReadonlyMap<PageId, PageInfo>,
  candidates: readonly PageInfo[],
  selectedPageId: PageId | null,
): PageInfo | null {
  for (const candidate of candidates) {
    const live = pages.get(candidate.id);
    if (live !== undefined) {
      return live;
    }
  }
  if (selectedPageId !== null) {
    const selected = pages.get(selectedPageId);
    if (selected !== undefined) {
      return selected;
    }
  }
  return pages.values().next().value ?? null;
}

// ----- code points and UTF-16 units -------------------------------------------------------------------

/** The UTF-16 index of each code point of `text` (and of its end): `offsets[n]` is where the n-th code point starts. */
function codePointOffsets(text: string): number[] {
  const offsets: number[] = [];
  let at = 0;
  for (const point of text) {
    offsets.push(at);
    at += point.length;
  }
  offsets.push(at);
  return offsets;
}

/** The UTF-16 index of the code point `position` of `text` (clamped to the text). */
export function toUtf16(text: string, position: number): number {
  const offsets = codePointOffsets(text);
  return offsets[Math.max(0, Math.min(position, offsets.length - 1))] ?? 0;
}

/** The code point position of the UTF-16 index `index` of `text` (an index inside a surrogate pair counts as the pair's start). */
export function toCodePoints(text: string, index: number): number {
  let position = 0;
  let at = 0;
  for (const point of text) {
    if (at + point.length > index) {
      return position;
    }
    at += point.length;
    position += 1;
  }
  return position;
}

function codePointCount(text: string): number {
  return codePointOffsets(text).length - 1;
}

// ----- the analysis ---------------------------------------------------------------------------------------

/** A range of the text the person sees, in code points, `end` exclusive. */
export interface TextRange {
  readonly start: number;
  readonly end: number;
}

/** What the text says: nothing yet, an error with where it is in the text shown, or a formula that checks. */
export type Analysis =
  | { readonly kind: "empty" }
  | { readonly kind: "error"; readonly error: FormulaError; readonly range: TextRange }
  | { readonly kind: "ok"; readonly stored: string; readonly type: FormulaType };

/**
 * Where a position of the stored text is in the display text. The two differ only in the text of the strings (a name
 * and an id), so their tokens pair up one to one; a position is carried over by its token and its distance from it.
 */
export function mapStoredPosition(stored: string, display: string, position: number): number {
  const from = tokenizeFormula(stored);
  const to = tokenizeFormula(display);
  const limit = codePointCount(display);
  if (from.length !== to.length) {
    return Math.min(position, limit);
  }
  for (let i = 0; i < from.length; i += 1) {
    const a = from[i];
    const b = to[i];
    if (a === undefined || b === undefined) {
      break;
    }
    if (position < a.start) {
      // In the white space before the token: its distance from the end of the token before.
      const previousA = from[i - 1]?.end ?? 0;
      const previousB = to[i - 1]?.end ?? 0;
      return Math.min(previousB + (position - previousA), b.start);
    }
    if (position <= a.end) {
      if (position === a.start) {
        return b.start;
      }
      return Math.min(b.start + (position - a.start), b.end);
    }
  }
  const lastA = from[from.length - 1]?.end ?? 0;
  const lastB = to[to.length - 1]?.end ?? 0;
  return Math.min(lastB + (position - lastA), limit);
}

/** An error's range made visible: a range with nothing in it covers the next character, or the last one at the end of the text. */
function visibleRange(text: string, start: number, end: number): TextRange {
  const length = codePointCount(text);
  const first = Math.max(0, Math.min(start, length));
  const last = Math.max(first, Math.min(end, length));
  if (last > first || length === 0) {
    return { start: first, end: last };
  }
  return first < length ? { start: first, end: first + 1 } : { start: length - 1, end: length };
}

/**
 * What a text written with property names makes: the stored expression (names to ids with `titleLabel` for the title
 * column) and its type, or the first error, placed in the text as it is written.
 */
export function analyseFormula(text: string, schema: FormulaSchema, selfId: string | undefined, titleLabel: string): Analysis {
  if (text.trim() === "") {
    return { kind: "empty" };
  }
  const stored = formulaToStored(text, schema, titleLabel);
  if (isFormulaError(stored)) {
    // The conversion read the text as it is written: its positions are the display text's.
    return { kind: "error", error: stored, range: visibleRange(text, stored.start, stored.end) };
  }
  const checked = checkFormula(stored, schema, selfId);
  if (isFormulaError(checked)) {
    const start = mapStoredPosition(stored, text, checked.start);
    const end = mapStoredPosition(stored, text, checked.end);
    return { kind: "error", error: checked, range: visibleRange(text, start, end) };
  }
  return { kind: "ok", stored, type: checked };
}

// ----- the mirror -------------------------------------------------------------------------------------------

/** One run of the highlighted mirror: its text, the kind of token it is (none for white space) and whether the error covers it. */
export interface Segment {
  readonly text: string;
  readonly kind: FormulaToken["kind"] | null;
  readonly error: boolean;
}

/**
 * The text cut into runs for the mirror under the textarea: one per token and per stretch of white space between
 * them, cut again where the error range starts and ends. The runs join to the text exactly.
 */
export function highlightSegments(text: string, tokens: readonly FormulaToken[], error: TextRange | null): Segment[] {
  const offsets = codePointOffsets(text);
  const at = (position: number): number => offsets[Math.max(0, Math.min(position, offsets.length - 1))] ?? text.length;
  const ranges = tokens
    .map((token) => ({ kind: token.kind, start: at(token.start), end: at(token.end) }))
    .filter((range) => range.end > range.start)
    .sort((a, b) => a.start - b.start);
  const errorStart = error === null ? -1 : at(error.start);
  const errorEnd = error === null ? -1 : at(error.end);
  const cuts = new Set<number>([0, text.length]);
  for (const range of ranges) {
    cuts.add(range.start);
    cuts.add(range.end);
  }
  if (error !== null) {
    cuts.add(errorStart);
    cuts.add(errorEnd);
  }
  const bounds = [...cuts].filter((cut) => cut >= 0 && cut <= text.length).sort((a, b) => a - b);
  const segments: Segment[] = [];
  let pointer = 0;
  for (let i = 0; i + 1 < bounds.length; i += 1) {
    const start = bounds[i];
    const end = bounds[i + 1];
    if (start === undefined || end === undefined || end <= start) {
      continue;
    }
    while (pointer < ranges.length && (ranges[pointer]?.end ?? 0) <= start) {
      pointer += 1;
    }
    const token = ranges[pointer];
    const kind = token !== undefined && token.start <= start && end <= token.end ? token.kind : null;
    segments.push({ text: text.slice(start, end), kind, error: error !== null && start >= errorStart && end <= errorEnd });
  }
  return segments;
}

// ----- completion ---------------------------------------------------------------------------------------------

/** One proposal of the completion list. */
export interface CompletionItem {
  readonly id: string;
  /** What the list shows. */
  readonly label: string;
  /** A short second line: the kind of a property, a function's signature. */
  readonly detail: string;
  readonly kind: "property" | "function" | "variable";
  /** What replaces the word. */
  readonly insert: string;
  /** How many characters before the end of `insert` the caret goes (inside the brackets of a function). */
  readonly caretBack: number;
}

/** What is being completed: the range of the text it replaces, the proposals, and an identity for "closed with Esc". */
export interface Completion {
  /** UTF-16 range of the text the chosen item replaces. */
  readonly from: number;
  readonly to: number;
  /** Characters after the insertion the caret skips (the closing quote of a string that was there). */
  readonly skip: number;
  readonly items: readonly CompletionItem[];
  readonly key: string;
}

/** The names and functions the completion proposes. */
export interface CompletionSources {
  /** The properties of the schema, in order. */
  readonly properties: readonly { readonly name: string; readonly kind: PropertyDef["kind"] }[];
  /** The name of the title column. */
  readonly titleLabel: string;
  readonly functions: readonly FormulaFunctionInfo[];
}

/** The most proposals the list shows. */
const MAX_PROPOSALS = 12;

const IDENT = /[A-Za-z0-9_]/u;

/** A name as the text of a string literal: `\` and `"` escaped. */
export function escapeFormulaString(name: string): string {
  return name.replace(/\\/gu, "\\\\").replace(/"/gu, '\\"');
}

/** Whether the code up to `caret` leaves it in code, in a string that began at `start`, or in a comment. */
function scanTo(text: string, caret: number): { mode: "code" | "string" | "comment"; start: number } {
  let mode: "code" | "string" | "comment" = "code";
  let start = 0;
  for (let i = 0; i < caret; i += 1) {
    const char = text[i];
    if (mode === "code") {
      if (char === '"') {
        mode = "string";
        start = i;
      } else if (char === "/" && text[i + 1] === "*" && i + 1 < caret) {
        mode = "comment";
        start = i;
        i += 1;
      }
    } else if (mode === "string") {
      if (char === "\\") {
        i += 1;
      } else if (char === '"') {
        mode = "code";
      }
    } else if (char === "*" && text[i + 1] === "/" && i + 1 < caret) {
      mode = "code";
      i += 1;
    }
  }
  return { mode, start };
}

/** Whether what comes before `index` (an opening quote or a position) is `prop(`, white space allowed. */
function followsPropOpen(text: string, index: number): boolean {
  let at = index;
  while (at > 0 && /\s/u.test(text.charAt(at - 1))) {
    at -= 1;
  }
  if (text.charAt(at - 1) !== "(") {
    return false;
  }
  at -= 1;
  while (at > 0 && /\s/u.test(text.charAt(at - 1))) {
    at -= 1;
  }
  return text.slice(Math.max(0, at - 4), at) === "prop" && !IDENT.test(text.charAt(at - 5));
}

/** The names of the properties a formula can read: the title column first, then the properties (not the two "by" kinds, which are devices). */
function readableNames(sources: CompletionSources): { name: string; detail: string }[] {
  const seen = new Set<string>();
  const names: { name: string; detail: string }[] = [];
  const add = (name: string, detail: string): void => {
    if (!seen.has(name)) {
      seen.add(name);
      names.push({ name, detail });
    }
  };
  add(sources.titleLabel, t("formula.completion.title"));
  for (const property of sources.properties) {
    if (property.kind !== "created_by" && property.kind !== "last_edited_by") {
      add(property.name, t(KIND_LABELS[property.kind]));
    }
  }
  return names;
}

/** The names that contain `query` (ignoring case), the ones that start with it first. */
function matchingNames(names: readonly { name: string; detail: string }[], query: string, allowInside: boolean) {
  const wanted = query.toLowerCase();
  const starts = names.filter((entry) => entry.name.toLowerCase().startsWith(wanted));
  const inside = allowInside ? names.filter((entry) => !starts.includes(entry) && entry.name.toLowerCase().includes(wanted)) : [];
  return [...starts, ...inside];
}

function propertyItem(entry: { name: string; detail: string }, insert: string): CompletionItem {
  return { id: `property:${entry.name}`, label: `prop("${escapeFormulaString(entry.name)}")`, detail: entry.detail, kind: "property", insert, caretBack: 0 };
}

function functionItem(fn: FormulaFunctionInfo, insertBrackets: boolean): CompletionItem {
  const signature = fn.signatures[0] ?? fn.name;
  const withoutArguments = signature.startsWith(`${fn.name}()`);
  return {
    id: `function:${fn.name}`,
    label: fn.name,
    detail: signature,
    kind: "function",
    insert: insertBrackets ? `${fn.name}()` : fn.name,
    caretBack: insertBrackets && !withoutArguments ? 1 : 0,
  };
}

/**
 * What the caret is completing, `null` when nothing: the name inside `prop("…")`, every property just after `prop(`, or the
 * word before the caret (a function, `current` or `index`, a property written as `prop("Name")`; after a dot only functions:
 * the method call). A word that starts with a digit is a number, and a comment is not completed.
 */
export function completionAt(text: string, caret: number, sources: CompletionSources): Completion | null {
  const { mode, start } = scanTo(text, caret);
  if (mode === "comment") {
    return null;
  }
  const names = readableNames(sources);
  if (mode === "string") {
    if (!followsPropOpen(text, start)) {
      return null;
    }
    const query = text.slice(start + 1, caret);
    let close = -1;
    for (let i = caret; i < text.length; i += 1) {
      if (text[i] === "\\") {
        i += 1;
      } else if (text[i] === '"') {
        close = i;
        break;
      }
    }
    const closed = close >= 0;
    const items = matchingNames(names, query, true)
      .slice(0, MAX_PROPOSALS)
      .map((entry) => propertyItem(entry, `${escapeFormulaString(entry.name)}${closed ? "" : '"'}`));
    return items.length === 0 ? null : { from: start + 1, to: closed ? close : caret, skip: closed ? 1 : 0, items, key: `s:${String(start)}:${query}` };
  }
  // Right after `prop(`: every property, written with its quotes.
  if (followsPropOpen(text, caret) && !/^\s*"/u.test(text.slice(caret))) {
    const items = names.slice(0, MAX_PROPOSALS).map((entry) => propertyItem(entry, `"${escapeFormulaString(entry.name)}"`));
    return items.length === 0 ? null : { from: caret, to: caret, skip: 0, items, key: `p:${String(caret)}` };
  }
  let wordStart = caret;
  while (wordStart > 0 && IDENT.test(text.charAt(wordStart - 1))) {
    wordStart -= 1;
  }
  let wordEnd = caret;
  while (wordEnd < text.length && IDENT.test(text.charAt(wordEnd))) {
    wordEnd += 1;
  }
  const prefix = text.slice(wordStart, caret);
  if (prefix === "" || /^[0-9]/u.test(prefix)) {
    return null;
  }
  const wanted = prefix.toLowerCase();
  const method = wordStart > 0 && text.charAt(wordStart - 1) === ".";
  const brackets = text.charAt(wordEnd) !== "(";
  const items: CompletionItem[] = sources.functions.filter((fn) => fn.name.toLowerCase().startsWith(wanted)).map((fn) => functionItem(fn, brackets));
  if (!method) {
    for (const variable of ["current", "index"]) {
      if (variable.startsWith(wanted)) {
        items.push({ id: `variable:${variable}`, label: variable, detail: t("formula.completion.variable"), kind: "variable", insert: variable, caretBack: 0 });
      }
    }
    for (const entry of matchingNames(names, prefix, prefix.length >= 2)) {
      items.push(propertyItem(entry, `prop("${escapeFormulaString(entry.name)}")`));
    }
  }
  const proposals = items.slice(0, MAX_PROPOSALS);
  return proposals.length === 0 ? null : { from: wordStart, to: wordEnd, skip: 0, items: proposals, key: `i:${String(wordStart)}:${wanted}` };
}

/** The text after the item replaced the word, and where the caret goes. */
export function applyCompletion(text: string, completion: Completion, item: CompletionItem): { text: string; caret: number } {
  const next = `${text.slice(0, completion.from)}${item.insert}${text.slice(completion.to)}`;
  return { text: next, caret: completion.from + item.insert.length + completion.skip - item.caretBack };
}

// ----- warnings ----------------------------------------------------------------------------------------------

/**
 * The names `prop("…")` reads in `text` that more than one property goes by (the formula reads the first one; the title
 * column counts when a property takes its name). In the order they first appear, without repeats.
 */
export function ambiguousNames(text: string, names: readonly string[], titleLabel: string): string[] {
  const found: string[] = [];
  for (const match of text.matchAll(/prop\(\s*"((?:[^"\\]|\\.)*)"\s*\)/gu)) {
    const name = (match[1] ?? "").replace(/\\(.)/gu, (_, char: string) => (char === "n" ? "\n" : char === "t" ? "\t" : char));
    const count = names.filter((candidate) => candidate === name).length + (name === titleLabel ? 1 : 0);
    if (count >= 2 && !found.includes(name)) {
      found.push(name);
    }
  }
  return found;
}

// ----- messages ----------------------------------------------------------------------------------------------

const BASE_TYPE_LABELS: Readonly<Record<string, MessageKey>> = {
  number: "formula.type.number",
  text: "formula.type.text",
  boolean: "formula.type.boolean",
  date: "formula.type.date",
  page: "formula.type.page",
  person: "formula.type.person",
};

/** The type names of the engine's messages (`number`, `list<text>`…) in the person's language, wherever they appear in `text`. */
export function translateTypeNames(text: string): string {
  return text.replace(/list<(\w+)>|\b(number|text|boolean|date|page|person)\b/gu, (_, inner: string | undefined, base: string | undefined) => {
    const name = inner ?? base ?? "";
    const label = BASE_TYPE_LABELS[name];
    const translated = label === undefined ? name : t(label);
    return inner === undefined ? translated : t("formula.type.list", { type: translated });
  });
}

/** The type of a formula's result as a person reads it: "number", "list of text". */
export function formulaTypeLabel(type: FormulaType): string {
  const base = t(BASE_TYPE_LABELS[type.base] ?? "formula.type.text");
  return type.list ? t("formula.type.list", { type: base }) : base;
}

/** An engine error as a sentence in the person's language, from its code and detail (the English message when the code is new). */
export function formulaErrorMessage(error: FormulaError): string {
  const { detail } = error;
  switch (error.code) {
    case "syntax":
      return detail === "" ? t("formula.error.syntax") : t("formula.error.syntax.expected", { expected: detail });
    case "too_long":
      return t("formula.error.too_long");
    case "unknown_property":
      return t("formula.error.unknown_property", { name: detail });
    case "unsupported_property":
      return t("formula.error.unsupported_property", { name: detail });
    case "unknown_function":
      return t("formula.error.unknown_function", { name: detail });
    case "unknown_variable":
      return t("formula.error.unknown_variable", { name: detail });
    case "arity": {
      const [name = "", min = "0", max = "-1"] = detail.split(":");
      const low = Number(min);
      const high = Number(max);
      if (high < 0) {
        return tp("formula.error.arity.atLeast", low, { name });
      }
      return low === high
        ? tp("formula.error.arity.exact", low, { name })
        : t("formula.error.arity.range", { name, min: low, max: high });
    }
    case "type": {
      const split = detail.indexOf(":");
      const expected = split < 0 ? detail : detail.slice(0, split);
      const found = split < 0 ? "" : detail.slice(split + 1);
      return t("formula.error.type", { expected: translateTypeNames(expected), found: translateTypeNames(found) });
    }
    case "cycle":
      return t("formula.error.cycle", { name: detail });
    default:
      return error.message;
  }
}

/** The short description of each function of the reference. A function this list lacks is shown without one. */
export const FUNCTION_DESCRIPTIONS: Readonly<Record<string, MessageKey>> = {
  if: "formula.fn.if",
  ifs: "formula.fn.ifs",
  and: "formula.fn.and",
  or: "formula.fn.or",
  not: "formula.fn.not",
  empty: "formula.fn.empty",
  equal: "formula.fn.equal",
  unequal: "formula.fn.unequal",
  add: "formula.fn.add",
  subtract: "formula.fn.subtract",
  multiply: "formula.fn.multiply",
  divide: "formula.fn.divide",
  mod: "formula.fn.mod",
  pow: "formula.fn.pow",
  abs: "formula.fn.abs",
  ceil: "formula.fn.ceil",
  floor: "formula.fn.floor",
  sqrt: "formula.fn.sqrt",
  cbrt: "formula.fn.cbrt",
  exp: "formula.fn.exp",
  ln: "formula.fn.ln",
  log10: "formula.fn.log10",
  log2: "formula.fn.log2",
  sign: "formula.fn.sign",
  pi: "formula.fn.pi",
  e: "formula.fn.e",
  round: "formula.fn.round",
  toNumber: "formula.fn.toNumber",
  min: "formula.fn.min",
  max: "formula.fn.max",
  sum: "formula.fn.sum",
  mean: "formula.fn.mean",
  median: "formula.fn.median",
  length: "formula.fn.length",
  substring: "formula.fn.substring",
  contains: "formula.fn.contains",
  test: "formula.fn.test",
  match: "formula.fn.match",
  replace: "formula.fn.replace",
  replaceAll: "formula.fn.replaceAll",
  split: "formula.fn.split",
  join: "formula.fn.join",
  lower: "formula.fn.lower",
  upper: "formula.fn.upper",
  trim: "formula.fn.trim",
  repeat: "formula.fn.repeat",
  padStart: "formula.fn.padStart",
  padEnd: "formula.fn.padEnd",
  format: "formula.fn.format",
  now: "formula.fn.now",
  today: "formula.fn.today",
  minute: "formula.fn.minute",
  hour: "formula.fn.hour",
  day: "formula.fn.day",
  date: "formula.fn.date",
  week: "formula.fn.week",
  month: "formula.fn.month",
  year: "formula.fn.year",
  dateAdd: "formula.fn.dateAdd",
  dateSubtract: "formula.fn.dateSubtract",
  dateBetween: "formula.fn.dateBetween",
  formatDate: "formula.fn.formatDate",
  parseDate: "formula.fn.parseDate",
  timestamp: "formula.fn.timestamp",
  fromTimestamp: "formula.fn.fromTimestamp",
  at: "formula.fn.at",
  first: "formula.fn.first",
  last: "formula.fn.last",
  slice: "formula.fn.slice",
  concat: "formula.fn.concat",
  sort: "formula.fn.sort",
  reverse: "formula.fn.reverse",
  unique: "formula.fn.unique",
  includes: "formula.fn.includes",
  flat: "formula.fn.flat",
  map: "formula.fn.map",
  filter: "formula.fn.filter",
  find: "formula.fn.find",
  findIndex: "formula.fn.findIndex",
  some: "formula.fn.some",
  every: "formula.fn.every",
  count: "formula.fn.count",
  prop: "formula.fn.prop",
  id: "formula.fn.id",
  name: "formula.fn.name",
  email: "formula.fn.email",
  let: "formula.fn.let",
  lets: "formula.fn.lets",
};

/** The categories of the reference, in order, with their headings. */
export const FUNCTION_CATEGORIES: readonly { readonly id: FormulaFunctionInfo["category"]; readonly label: MessageKey }[] = [
  { id: "logic", label: "formula.category.logic" },
  { id: "number", label: "formula.category.number" },
  { id: "text", label: "formula.category.text" },
  { id: "date", label: "formula.category.date" },
  { id: "list", label: "formula.category.list" },
  { id: "page", label: "formula.category.page" },
  { id: "special", label: "formula.category.special" },
];

/** The functions that match `query` (in their name, description or signatures, ignoring case), grouped by category. */
export function functionGroups(
  functions: readonly FormulaFunctionInfo[],
  query: string,
  describe: (name: string) => string,
): { id: FormulaFunctionInfo["category"]; label: MessageKey; items: FormulaFunctionInfo[] }[] {
  const wanted = query.trim().toLowerCase();
  const matches = (fn: FormulaFunctionInfo): boolean =>
    wanted === "" ||
    fn.name.toLowerCase().includes(wanted) ||
    describe(fn.name).toLowerCase().includes(wanted) ||
    fn.signatures.some((signature) => signature.toLowerCase().includes(wanted));
  return FUNCTION_CATEGORIES.map((category) => ({
    ...category,
    items: functions.filter((fn) => fn.category === category.id && matches(fn)),
  })).filter((group) => group.items.length > 0);
}
