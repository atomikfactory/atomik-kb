// The data of the list view (ADR-0016, amendment of 2026-10-07): the four attributes of the
// `pageList` node read leniently (a document written elsewhere may hold anything), which pages a
// view shows (the children of the page that holds it, in tree order, or the pages that carry a tag;
// `page-list-sources.ts` adds the linked sources) and the order they are shown in. Pure functions over the page tree and the space's property
// schema, so that the view (`PageListView.tsx`) and its tests share them.

import type {
  DeviceId,
  FormulaResult,
  OptionId,
  PageId,
  PageInfo,
  PropertyDef,
  PropertyId,
  PropertyValue,
  TreeView,
} from "@atomik-kb/core-client";

import { formulaPlainText } from "../formula-format";
import { rollupConfigOf, rollupItemText } from "../property-format";
import { locate } from "../tree";

/**
 * Where the pages of a view come from: the pages under the page that holds the block, the pages of
 * the space that carry a Tags option, the pages under another page of the space (a linked view,
 * ADR-0058) or every page of the space (`page-list-sources.ts` resolves them).
 */
export type PageListSource =
  | { readonly kind: "children" }
  | { readonly kind: "tag"; readonly optionId: OptionId }
  | { readonly kind: "page"; readonly pageId: PageId }
  | { readonly kind: "space" };

export type SortDirection = "asc" | "desc";

/**
 * How the rows are ordered: `by` is `"title"`, `"updated"`, `"created"` or the id of a property of
 * the space's schema.
 */
export interface PageListSort {
  readonly by: string;
  readonly dir: SortDirection;
}

/**
 * The layouts a view can have, in the order the layout picker and the "add a view" menu list them (ADR-0054 adds the
 * calendar and the gallery). One this version does not know shows as the list and is kept as it is.
 */
export const PAGE_LIST_LAYOUTS: readonly string[] = ["list", "table", "board", "calendar", "gallery", "feed", "dashboard"];
export const DEFAULT_PAGE_LIST_LAYOUT = "list";
export type PageListLayout = "list" | "table" | "board" | "calendar" | "gallery" | "feed" | "dashboard";

/** Narrowest a table column can be, in pixels. */
export const MIN_COLUMN_WIDTH = 80;
/** Widest a table column can be made, in pixels. */
export const MAX_COLUMN_WIDTH = 800;
/** The key of the page (title) column in `columnWidths`. */
export const TITLE_COLUMN = "title";

export const SORT_BY_TITLE = "title";
export const SORT_BY_UPDATED = "updated";
export const SORT_BY_CREATED = "created";
/** The sorts that are not properties, in the order the picker lists them. */
export const BUILT_IN_SORTS: readonly string[] = [SORT_BY_TITLE, SORT_BY_UPDATED, SORT_BY_CREATED];

export const DEFAULT_PAGE_LIST_SOURCE: PageListSource = { kind: "children" };
export const SPACE_PAGE_LIST_SOURCE: PageListSource = { kind: "space" };
export const DEFAULT_PAGE_LIST_SORT: PageListSort = { by: SORT_BY_TITLE, dir: "asc" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// ----- the attributes of the node ------------------------------------------------------------

/**
 * The source an attribute holds, or `null` for one this version does not know (another kind, or a
 * tag or page source without its option or page): the view says so and shows nothing, and never
 * rewrites it.
 */
export function readSource(value: unknown): PageListSource | null {
  if (!isRecord(value)) {
    return null;
  }
  if (value.kind === "children") {
    return DEFAULT_PAGE_LIST_SOURCE;
  }
  if (value.kind === "tag" && typeof value.optionId === "string" && value.optionId !== "") {
    return { kind: "tag", optionId: value.optionId };
  }
  if (value.kind === "page" && typeof value.pageId === "string" && value.pageId !== "") {
    return { kind: "page", pageId: value.pageId };
  }
  if (value.kind === "space") {
    return SPACE_PAGE_LIST_SOURCE;
  }
  return null;
}

/** The property ids an attribute lists, in order, without repeats; anything that is no list reads as none. */
export function readProperties(value: unknown): PropertyId[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const ids: PropertyId[] = [];
  for (const item of value as unknown[]) {
    if (typeof item === "string" && item !== "" && !ids.includes(item)) {
      ids.push(item);
    }
  }
  return ids;
}

/** The sort an attribute holds; a part that is not usable reads as its default (title, ascending). */
export function readSort(value: unknown): PageListSort {
  if (!isRecord(value)) {
    return DEFAULT_PAGE_LIST_SORT;
  }
  return {
    by: typeof value.by === "string" && value.by !== "" ? value.by : DEFAULT_PAGE_LIST_SORT.by,
    dir: value.dir === "desc" ? "desc" : "asc",
  };
}

/** Most sort keys a view can have (ADR-0058). */
export const MAX_SORTS = 5;

/**
 * The sort keys a list holds: the entries that are objects with a non-empty `by` (a direction that is not `"desc"` reads
 * as ascending), a repeated `by` keeping the first, at most 5. Anything that is no list reads as none.
 */
export function readSortList(value: unknown): PageListSort[] {
  const sorts: PageListSort[] = [];
  if (!Array.isArray(value)) {
    return sorts;
  }
  for (const item of value as unknown[]) {
    if (sorts.length >= MAX_SORTS) {
      break;
    }
    if (isRecord(item) && typeof item.by === "string" && item.by !== "" && !sorts.some((sort) => sort.by === item.by)) {
      sorts.push({ by: item.by, dir: item.dir === "desc" ? "desc" : "asc" });
    }
  }
  return sorts;
}

/**
 * The sort keys of a view (ADR-0058): `sorts` when it is a list with at least one usable key (`readSortList`), else the
 * one key `sort` holds (`readSort`). Never empty.
 */
export function readSorts(sorts: unknown, sort?: unknown): PageListSort[] {
  const list = readSortList(sorts);
  return list.length > 0 ? list : [readSort(sort)];
}

/** The layout an attribute holds; a value this version does not know (or no text) reads as the list. */
export function readLayout(value: unknown): PageListLayout {
  return value === "table" ||
    value === "board" ||
    value === "calendar" ||
    value === "gallery" ||
    value === "feed" ||
    value === "dashboard"
    ? value
    : "list";
}

/**
 * The widths an attribute holds, `{ "<property id>|title": px }`. Entries that are no finite number
 * above zero are dropped (they read as the default width); the others are kept as they are and only
 * clamped when a column is drawn.
 */
export function readColumnWidths(value: unknown): Record<string, number> {
  const widths: Record<string, number> = {};
  if (!isRecord(value)) {
    return widths;
  }
  for (const [key, width] of Object.entries(value)) {
    if (typeof width === "number" && Number.isFinite(width) && width > 0) {
      widths[key] = width;
    }
  }
  return widths;
}

/** A width as a column is drawn: a whole number of pixels between the minimum and the maximum. */
export function clampColumnWidth(width: number): number {
  return Math.min(MAX_COLUMN_WIDTH, Math.max(MIN_COLUMN_WIDTH, Math.round(width)));
}

/** The select value of a source (`children`, `tag:<option id>`, `page:<page id>` or `space`), and back. */
export function sourceChoice(source: PageListSource): string {
  switch (source.kind) {
    case "children":
      return "children";
    case "space":
      return "space";
    case "tag":
      return `tag:${source.optionId}`;
    case "page":
      return `page:${source.pageId}`;
  }
}

export function choiceSource(choice: string): PageListSource | null {
  if (choice === "children") {
    return DEFAULT_PAGE_LIST_SOURCE;
  }
  if (choice === "space") {
    return SPACE_PAGE_LIST_SOURCE;
  }
  if (choice.startsWith("tag:") && choice.length > 4) {
    return { kind: "tag", optionId: choice.slice(4) };
  }
  return choice.startsWith("page:") && choice.length > 5 ? { kind: "page", pageId: choice.slice(5) } : null;
}

// ----- which pages ---------------------------------------------------------------------------

/** The pages directly under `pageId`, in tree order; none while the tree is not loaded or lacks the page. */
export function childPages(tree: TreeView | null, pageId: PageId | null): PageInfo[] {
  if (tree === null || pageId === null) {
    return [];
  }
  return (locate(tree.roots, pageId)?.node.children ?? []).map((node) => node.page);
}

/** The pages of the tree whose id is in `ids`, in tree order (depth first). */
export function pagesIn(tree: TreeView | null, ids: ReadonlySet<PageId>): PageInfo[] {
  const pages: PageInfo[] = [];
  const visit = (nodes: TreeView["roots"]): void => {
    for (const node of nodes) {
      if (ids.has(node.page.id)) {
        pages.push(node.page);
      }
      visit(node.children);
    }
  };
  if (tree !== null) {
    visit(tree.roots);
  }
  return pages;
}

// ----- values as text ------------------------------------------------------------------------

/** The value a page holds for a property, `undefined` when it has none. */
export function valueOfPage(page: PageInfo, propertyId: PropertyId): PropertyValue | undefined {
  return page.properties?.find(([id]) => id === propertyId)?.[1];
}

/** The name of an option of a property, `undefined` when the property no longer lists it. */
export function optionName(def: PropertyDef, id: OptionId): string | undefined {
  return def.options.find((option) => option.id === id)?.name;
}

/**
 * The text of a value of a kind this version does not know (or any JSON): a string as it is, a
 * number or boolean written out, a list as its items joined with commas, an object as its `value`,
 * else its `name`, `text`, `title` or `label`; nothing otherwise.
 */
export function genericText(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (Array.isArray(value)) {
    return (value as unknown[])
      .map(genericText)
      .filter((text) => text !== "")
      .join(", ");
  }
  if (isRecord(value)) {
    if ("value" in value) {
      return genericText(value.value);
    }
    for (const key of ["name", "text", "title", "label"]) {
      const field = value[key];
      if (typeof field === "string" && field !== "") {
        return field;
      }
    }
  }
  return "";
}

// ----- sorting -------------------------------------------------------------------------------

/** What a page sorts by: nothing (a missing value, which sorts last), a number or a text. */
type SortKey = { kind: "none" } | { kind: "number"; value: number } | { kind: "text"; value: string };

const NONE: SortKey = { kind: "none" };

function numberKey(value: number): SortKey {
  return Number.isFinite(value) ? { kind: "number", value } : NONE;
}

function textKey(value: string): SortKey {
  return value === "" ? NONE : { kind: "text", value };
}

/**
 * What sorting needs to know that a page does not carry: the title of a page a relation points at
 * and the name a device is shown by. Either missing reads as nothing (the value sorts last / by id).
 */
export interface SortLookups {
  /** The title of a page, `null` when it is not known. */
  readonly pageTitle?: (id: PageId) => string | null;
  /** The name of a device as it is displayed. */
  readonly deviceName?: (id: DeviceId) => string;
  /** The name of a person of the space (a Person value sorts by the names), empty when it is not known. */
  readonly personName?: (id: string) => string;
}

/** A day as plain text, for ordering only. */
function plainDay(date: string, time: string | null): string {
  return time === null ? date : `${date} ${time}`;
}

/** The sort key of a rollup: a number or a progress by value, a day by ISO, a list by its texts. */
function rollupKey(def: PropertyDef, value: Extract<PropertyValue, { kind: "rollup" }>, schema: readonly PropertyDef[]): SortKey {
  const { result } = value.value;
  switch (result.type) {
    case "number":
      return numberKey(result.data);
    case "date":
      return textKey(plainDay(result.data.date, result.data.time ?? null));
    case "progress":
      return result.data.total === 0 ? NONE : numberKey(result.data.done / result.data.total);
    case "values": {
      const config = rollupConfigOf(def);
      const target = config === undefined ? undefined : schema.find((candidate) => candidate.id === config.target);
      return textKey(
        result.data
          .map((item) => rollupItemText(item, target, plainDay))
          .filter((text) => text !== "")
          .join(", "),
      );
    }
  }
}

/** The sort key of a formula's result: a number numerically, a boolean false before true, the rest by its plain text (a day by its ISO text). */
function formulaKey(result: FormulaResult, lookups: SortLookups): SortKey {
  switch (result.type) {
    case "number":
      return numberKey(result.data);
    case "boolean":
      return { kind: "number", value: result.data ? 1 : 0 };
    default:
      return textKey(formulaPlainText(result, lookups));
  }
}

/** The sort key of a value of a property: numbers numerically, dates by ISO, text and options by label, false before true. */
function propertyKey(def: PropertyDef, value: PropertyValue | undefined, schema: readonly PropertyDef[], lookups: SortLookups): SortKey {
  if (value === undefined) {
    return NONE;
  }
  switch (value.kind) {
    case "number":
      return numberKey(value.value);
    case "checkbox":
      return { kind: "number", value: value.value ? 1 : 0 };
    case "date":
      // ISO texts order like the days they name; a day without a time comes before the same day with one.
      return textKey(`${value.value.date} ${value.value.time ?? ""}`);
    case "select":
    case "status":
      return textKey(optionName(def, value.value) ?? "");
    case "multi_select":
      return textKey(
        value.value
          .map((id) => optionName(def, id))
          .filter((name): name is string => name !== undefined)
          .join(", "),
      );
    case "text":
    case "url":
    case "email":
    case "phone":
      return textKey(value.value);
    case "place":
      return textKey(value.value.name);
    case "created_time":
    case "last_edited_time":
      return numberKey(value.value);
    case "created_by":
    case "last_edited_by":
      return textKey(lookups.deviceName?.(value.value) ?? value.value);
    case "relation":
      return textKey(
        value.value
          .map((id) => lookups.pageTitle?.(id) ?? "")
          .filter((title) => title !== "")
          .join(", "),
      );
    case "person":
      return textKey(
        value.value
          .map((id) => lookups.personName?.(id) ?? "")
          .filter((name) => name !== "")
          .join(", "),
      );
    case "rollup":
      return rollupKey(def, value, schema);
    case "formula":
      return formulaKey(value.value, lookups);
    default:
      return textKey(genericText(value));
  }
}

function asciiLower(text: string): string {
  return text.replace(/[A-Z]/g, (letter) => letter.toLowerCase());
}

/** ASCII case-insensitive, then exact (so the order never depends on the locale). */
function compareText(a: string, b: string): number {
  const x = asciiLower(a);
  const y = asciiLower(b);
  if (x !== y) {
    return x < y ? -1 : 1;
  }
  return a === b ? 0 : a < b ? -1 : 1;
}

function compareKeys(a: SortKey, b: SortKey, dir: SortDirection): number {
  // A missing value is last whichever way the rows are sorted.
  if (a.kind === "none" || b.kind === "none") {
    return a.kind === b.kind ? 0 : a.kind === "none" ? 1 : -1;
  }
  let order: number;
  if (a.kind === "number" && b.kind === "number") {
    order = a.value - b.value;
  } else if (a.kind === "text" && b.kind === "text") {
    order = compareText(a.value, b.value);
  } else {
    order = a.kind === "number" ? -1 : 1; // (kinds that mix: numbers first)
  }
  return dir === "asc" ? order : -order;
}

/** The function that gives a page's sort key for `by`; a property the schema does not have sorts like the title. */
function keyOf(by: string, schema: readonly PropertyDef[], lookups: SortLookups): (page: PageInfo) => SortKey {
  switch (by) {
    case SORT_BY_UPDATED:
      return (page) => numberKey(Number(page.updatedAt));
    case SORT_BY_CREATED:
      return (page) => numberKey(Number(page.createdAt));
    case SORT_BY_TITLE:
      return (page) => ({ kind: "text", value: page.title });
    default: {
      const def = schema.find((candidate) => candidate.id === by);
      if (def === undefined) {
        return (page) => ({ kind: "text", value: page.title });
      }
      return (page) => propertyKey(def, valueOfPage(page, def.id), schema, lookups);
    }
  }
}

/**
 * `pages` ordered by `sort`: the title (ASCII case-insensitive), the time of the last change or of
 * the creation, or a property of the space by its value (numbers numerically, dates by ISO, text and
 * options by their label, a checkbox false before true; pages without a value come last whichever
 * the direction). The kinds of ADR-0026/0027 sort as the amendment of the list view says: a place by
 * its name, the times numerically, a device by the name it is displayed with, a relation by the
 * titles of its pages joined, a rollup by its number, day or progress ratio, or by the texts of its
 * values, and a formula (ADR-0055) by its result: a number numerically, a boolean false before true, a day by ISO, the
 * rest by its text. Several keys (ADR-0058, at most 5) compare in turn: a page that ties on a key goes by the next one (a
 * missing value is last for each key), and pages that tie on all of them keep the order they came in (the sort is
 * stable); no key leaves the order as it is.
 */
export function sortPages(
  pages: readonly PageInfo[],
  sorts: readonly PageListSort[],
  schema: readonly PropertyDef[],
  lookups: SortLookups = {},
): PageInfo[] {
  const keys = sorts.slice(0, MAX_SORTS).map((sort) => ({ key: keyOf(sort.by, schema, lookups), dir: sort.dir }));
  return pages
    .map((page) => ({ page, keys: keys.map(({ key }) => key(page)) }))
    .sort((a, b) => {
      for (const [at, { dir }] of keys.entries()) {
        const order = compareKeys(a.keys[at] ?? NONE, b.keys[at] ?? NONE, dir);
        if (order !== 0) {
          return order;
        }
      }
      return 0;
    })
    .map(({ page }) => page);
}

/** Whether a page list sorts by `by` as it stands (a built-in sort, or a property the schema has). */
export function isKnownSort(by: string, schema: readonly PropertyDef[]): boolean {
  return BUILT_IN_SORTS.includes(by) || schema.some((def) => def.id === by);
}
