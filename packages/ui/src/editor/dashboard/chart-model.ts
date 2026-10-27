// The data of a chart widget (ADR-0055, `docs/architecture/dashboards.md` section 4): reading and writing the spec,
// grouping pages by a property or a date bucket (section 4.2), measuring each group (4.3), ordering and capping. Pure
// functions over `PageInfo` and the space's schema; `ChartWidget.tsx` and `charts/` draw what `chartData` answers.
//
// Grouping follows the table of section 4.2: an option property has one group per option (in the options' order, with
// the option's colour), a checkbox two, a person, a relation or a text value one per distinct value, a date one per
// bucket of the local day from the first to the last date (the empty buckets included, the latest 400 kept), and a
// formula or a "show original" rollup groups by what it holds. A page with a list of values counts in each of its
// items (once). A page without a value goes in a "No value" group when `showEmpty` asks for it.
//
// Measuring: `count` counts pages; the other functions summarise a number (a number property, a rollup whose result is
// a number, a formula whose result is a number), as the table footer's `evaluateRollup` does; a group without a number
// has no value (`null`). `cumulative` (a line) adds each point to the ones before, the pages of the buckets that the
// cap dropped included. The cap is per drawing: 50 bars, 400 points, 8 donut slices and the rest in "Other".

import { evaluateRollup } from "@atomik-kb/core-client";
import type {
  AccountId,
  FormulaResult,
  OptionColor,
  PageId,
  PageInfo,
  PropertyDef,
  PropertyOption,
  PropertyValue,
} from "@atomik-kb/core-client";

import { activeLocale, t as translateNow } from "../../i18n";
import type { MessageKey } from "../../i18n";
import { optionColorOf, rollupConfigOf, rollupItemText } from "../../property-format";
import { addDays, addMonths, daysBetween, firstDayOfWeek, isIsoDate, partsOf, todayIso, weekdayOf } from "../dates";
import { valueOfPage } from "../page-list-model";
import type { ChartBucket, ChartMeasure, ChartOrder, ChartSpec, ChartType } from "./dashboard-types";

// ----- constants ----------------------------------------------------------------------------------

export const CHART_TYPES: readonly ChartType[] = ["bar", "line", "donut"];
export const CHART_BUCKETS: readonly ChartBucket[] = ["day", "week", "month", "quarter", "year"];
export const CHART_ORDERS: readonly ChartOrder[] = ["default", "value_desc", "value_asc"];
/** The functions of a measure, in the order the settings list them. */
export const MEASURE_FUNCTIONS = ["count", "sum", "average", "median", "min", "max"] as const;
export type MeasureFunction = (typeof MEASURE_FUNCTIONS)[number];

/** `groupBy` of the time a page was created, and of the time it was last edited. */
export const GROUP_BY_CREATED = "created";
export const GROUP_BY_UPDATED = "updated";

/** Most bars one bar chart draws (the rest are left out and counted: "+k more"). */
export const CHART_MAX_BARS = 50;
/** Most points one line chart draws. */
export const CHART_MAX_POINTS = 400;
/** Most buckets a date source makes: a wider range keeps the latest ones. */
export const CHART_MAX_BUCKETS = 400;
/** Most slices a donut names; the rest are one "Other" slice. */
export const CHART_MAX_SLICES = 8;
/** How many palette colours there are (`--chart-1` to `--chart-8`). */
export const CHART_PALETTE_SIZE = 8;
/** Most pages a mark's popover lists (then "and k more"). */
export const CHART_POPOVER_PAGES = 50;

/** The key of the group of pages without a value, and of the donut's "Other" slice. */
export const EMPTY_GROUP_KEY = "~empty";
export const OTHER_GROUP_KEY = "~other";

type Translate = (key: MessageKey, params?: Record<string, string | number>) => string;

// ----- reading and writing the spec ----------------------------------------------------------------

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const KNOWN_KEYS: readonly string[] = ["type", "groupBy", "bucket", "measure", "horizontal", "cumulative", "order", "showEmpty"];

function isChartType(value: unknown): value is ChartType {
  return typeof value === "string" && (CHART_TYPES as readonly string[]).includes(value);
}

function isBucket(value: unknown): value is ChartBucket {
  return typeof value === "string" && (CHART_BUCKETS as readonly string[]).includes(value);
}

function isOrder(value: unknown): value is ChartOrder {
  return typeof value === "string" && (CHART_ORDERS as readonly string[]).includes(value);
}

/**
 * The measure a stored value holds: a count when it is missing or cannot be read (an incomplete summary reads as the
 * count), `null` for a function this version does not know (a newer version wrote it: the widget is kept as it was).
 */
function readMeasure(value: unknown): ChartMeasure | null {
  if (!isRecord(value) || typeof value.fn !== "string" || value.fn === "count") {
    return { fn: "count" };
  }
  const fn = value.fn;
  if (fn !== "sum" && fn !== "average" && fn !== "median" && fn !== "min" && fn !== "max") {
    return null;
  }
  return typeof value.propertyId === "string" && value.propertyId !== "" ? { fn, propertyId: value.propertyId } : { fn: "count" };
}

/**
 * The spec a stored `chart` value holds; `null` when it is not a chart this version can read (not an object, a type or a
 * measure function of a newer version): the widget is kept raw. A part that is unusable reads as its default (no group
 * chosen, the count, no option), keys this version does not know are kept in `extra`.
 */
export function readChartSpec(value: unknown): ChartSpec | null {
  if (!isRecord(value) || !isChartType(value.type)) {
    return null;
  }
  const measure = readMeasure(value.measure);
  if (measure === null) {
    return null;
  }
  const extra: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (!KNOWN_KEYS.includes(key) && key !== "__proto__") {
      extra[key] = entry;
    }
  }
  return {
    type: value.type,
    groupBy: typeof value.groupBy === "string" && value.groupBy !== "" ? value.groupBy : null,
    ...(isBucket(value.bucket) ? { bucket: value.bucket } : {}),
    measure,
    ...(value.horizontal === true ? { horizontal: true } : {}),
    ...(value.cumulative === true ? { cumulative: true } : {}),
    ...(isOrder(value.order) && value.order !== "default" ? { order: value.order } : {}),
    ...(value.showEmpty === true ? { showEmpty: true } : {}),
    ...(Object.keys(extra).length > 0 ? { extra } : {}),
  };
}

/** The JSON a spec is stored as: the keys that differ from their default, and the unknown keys of `extra` as they were. */
export function chartSpecToJson(spec: ChartSpec): Record<string, unknown> {
  const json: Record<string, unknown> = { ...(spec.extra ?? {}) };
  json.type = spec.type;
  json.groupBy = spec.groupBy;
  if (spec.bucket !== undefined) {
    json.bucket = spec.bucket;
  }
  json.measure = spec.measure.fn === "count" ? { fn: "count" } : { fn: spec.measure.fn, propertyId: spec.measure.propertyId };
  if (spec.horizontal === true) {
    json.horizontal = true;
  }
  if (spec.cumulative === true) {
    json.cumulative = true;
  }
  if (spec.order !== undefined && spec.order !== "default") {
    json.order = spec.order;
  }
  if (spec.showEmpty === true) {
    json.showEmpty = true;
  }
  return json;
}

/** The spec of a new chart widget (section 2.2): a bar chart of the count by the first status or select, else by month created. */
export function defaultChartSpec(schema: readonly PropertyDef[]): ChartSpec {
  const first = schema.find((def) => def.kind === "status" || def.kind === "select");
  if (first !== undefined) {
    return { type: "bar", groupBy: first.id, measure: { fn: "count" } };
  }
  return { type: "bar", groupBy: GROUP_BY_CREATED, bucket: "month", measure: { fn: "count" } };
}

// ----- what can group and what can be measured ------------------------------------------------------

/** What a group-by source reads its groups from. */
type GroupSourceKind = "options" | "checkbox" | "people" | "pages" | "text" | "dates";

/** The kinds a "show original" rollup can group by, as text. */
const TEXT_TARGETS: readonly string[] = ["select", "status", "multi_select", "text", "url", "email", "phone", "place"];

function groupSourceKind(def: PropertyDef, schema: readonly PropertyDef[]): GroupSourceKind | null {
  switch (def.kind) {
    case "select":
    case "status":
    case "multi_select":
      return "options";
    case "checkbox":
      return "checkbox";
    case "person":
      return "people";
    case "relation":
      return "pages";
    case "text":
    case "url":
    case "email":
    case "phone":
    case "place":
      return "text";
    case "date":
    case "created_time":
    case "last_edited_time":
      return "dates";
    case "formula": {
      const type = def.config?.kind === "formula" ? def.config.resultType : undefined;
      switch (type?.base) {
        case "text":
          return "text";
        case "boolean":
          return "checkbox";
        case "date":
          return "dates";
        case "page":
          return "pages";
        case "person":
          return "people";
        default:
          return null; // a number result is measured, not grouped by; no type: the formula no longer checks
      }
    }
    case "rollup": {
      const config = rollupConfigOf(def);
      if (config?.function !== "show_original") {
        return null;
      }
      if (config.target === "title") {
        return "text";
      }
      const target = schema.find((candidate) => candidate.id === config.target);
      return target !== undefined && TEXT_TARGETS.includes(target.kind) ? "text" : null;
    }
    default:
      return null;
  }
}

/** The properties a chart can group by (the created and edited times are not properties: `GROUP_BY_CREATED`, `GROUP_BY_UPDATED`). */
export function groupableProperties(schema: readonly PropertyDef[]): PropertyDef[] {
  return schema.filter((def) => groupSourceKind(def, schema) !== null);
}

/** Whether the groups of `groupBy` are dates (buckets): a date property, a time, a formula that makes a date. */
export function isDateSource(groupBy: string | null, schema: readonly PropertyDef[]): boolean {
  if (groupBy === GROUP_BY_CREATED || groupBy === GROUP_BY_UPDATED) {
    return true;
  }
  const def = schema.find((candidate) => candidate.id === groupBy);
  return def !== undefined && groupSourceKind(def, schema) === "dates";
}

/** The rollup functions whose result is not a number. */
const NON_NUMERIC_ROLLUPS: readonly string[] = ["earliest", "latest", "show_original", "checklist_progress"];

/** Whether a property holds a number a measure can summarise: a number, a rollup whose result is one, a formula that makes one. */
export function isMeasurable(def: PropertyDef): boolean {
  switch (def.kind) {
    case "number":
      return true;
    case "rollup": {
      const fn = rollupConfigOf(def)?.function;
      return fn !== undefined && !NON_NUMERIC_ROLLUPS.includes(fn);
    }
    case "formula": {
      const type = def.config?.kind === "formula" ? def.config.resultType : undefined;
      return type?.base === "number" && !type.list;
    }
    default:
      return false;
  }
}

/** The properties a measure can summarise. */
export function measurableProperties(schema: readonly PropertyDef[]): PropertyDef[] {
  return schema.filter(isMeasurable);
}

/** The number a page holds for a measurable property, `undefined` for none. */
function numberOf(page: PageInfo, def: PropertyDef): number | undefined {
  const value = valueOfPage(page, def.id);
  switch (value?.kind) {
    case "number":
      return Number.isFinite(value.value) ? value.value : undefined;
    case "rollup":
      return value.value.result.type === "number" ? value.value.result.data : undefined;
    case "formula":
      return value.value.type === "number" ? value.value.data : undefined;
    default:
      return undefined;
  }
}

// ----- changing a spec (the settings) ---------------------------------------------------------------

/** `spec` grouped by `groupBy`; a date source is chronological, so it drops the order. */
export function withGroupBy(spec: ChartSpec, groupBy: string, schema: readonly PropertyDef[]): ChartSpec {
  return isDateSource(groupBy, schema) ? { ...spec, groupBy, order: undefined } : { ...spec, groupBy };
}

/** `spec` measured with `fn`; a function of a number needs a property: the current one if it is measurable, else the first. */
export function withMeasureFunction(spec: ChartSpec, fn: MeasureFunction, schema: readonly PropertyDef[]): ChartSpec {
  if (fn === "count") {
    return { ...spec, measure: { fn: "count" } };
  }
  const current = spec.measure.fn === "count" ? undefined : spec.measure.propertyId;
  const measurable = measurableProperties(schema);
  const property = measurable.find((def) => def.id === current) ?? measurable[0];
  return property === undefined ? spec : { ...spec, measure: { fn, propertyId: property.id } };
}

/** `spec` measured over another property (the function stays). */
export function withMeasureProperty(spec: ChartSpec, propertyId: string): ChartSpec {
  return spec.measure.fn === "count" ? spec : { ...spec, measure: { fn: spec.measure.fn, propertyId } };
}

// ----- titles --------------------------------------------------------------------------------------

const MEASURE_KEYS = {
  count: "chart.measure.count",
  sum: "chart.measure.sum",
  average: "chart.measure.average",
  median: "chart.measure.median",
  min: "chart.measure.min",
  max: "chart.measure.max",
} as const satisfies Record<MeasureFunction, MessageKey>;

/** What a measure is called: "Count", "Sum of Hours". */
export function chartMeasureLabel(measure: ChartMeasure, schema: readonly PropertyDef[], t: Translate): string {
  if (measure.fn === "count") {
    return t(MEASURE_KEYS.count);
  }
  const def = schema.find((candidate) => candidate.id === measure.propertyId);
  return t(MEASURE_KEYS[measure.fn], { property: def?.name ?? t("chart.unknownProperty") });
}

/** What the groups are called: the property's name, "Created", "Last edited", a date source with its bucket: "Due (month)". `null` without a group. */
function groupLabelOf(spec: ChartSpec, schema: readonly PropertyDef[], t: Translate): string | null {
  const { groupBy } = spec;
  if (groupBy === null) {
    return null;
  }
  const def = schema.find((candidate) => candidate.id === groupBy);
  const name = groupBy === GROUP_BY_CREATED ? t("chart.group.created") : groupBy === GROUP_BY_UPDATED ? t("chart.group.updated") : def?.name;
  if (name === undefined) {
    return null;
  }
  return isDateSource(groupBy, schema) ? t("chart.group.bucket", { name, bucket: t(`chart.bucket.${spec.bucket ?? "month"}`) }) : name;
}

/** The automatic title of a chart ("Count by Status", "Sum of Hours by Due (month)"), through the UI's translate function. */
export function chartTitle(spec: ChartSpec, schema: readonly PropertyDef[], t: Translate): string {
  const group = groupLabelOf(spec, schema, t);
  return group === null ? t("chart.title.none") : t("chart.title", { measure: chartMeasureLabel(spec.measure, schema, t), group });
}

// ----- the data ------------------------------------------------------------------------------------

/** What a chart's groups are made of and how they are named. Every part has a default: the UI's language and the first day of its week. */
export interface ChartContext {
  /** The locale of the bucket labels and of the order of texts (the UI's). */
  readonly locale?: string;
  /** The first day of the week, 0 for Sunday to 6 for Saturday (the locale's). */
  readonly weekStart?: number;
  /** The title of a page a relation links to; `null` when it is not known. */
  readonly pageTitle?: (id: PageId) => string | null;
  /** The name a person of the space is shown by. */
  readonly personName?: (id: AccountId) => string;
  readonly t?: Translate;
}

interface ResolvedContext {
  readonly locale: string;
  readonly weekStart: number;
  readonly pageTitle: (id: PageId) => string | null;
  readonly personName: (id: AccountId) => string;
  readonly t: Translate;
}

function resolveContext(context: ChartContext): ResolvedContext {
  const locale = context.locale ?? activeLocale();
  return {
    locale,
    weekStart: context.weekStart ?? firstDayOfWeek(locale),
    pageTitle: context.pageTitle ?? (() => null),
    personName: context.personName ?? ((id) => id),
    t: context.t ?? translateNow,
  };
}

/** What kind of group a mark stands for. */
export type ChartGroupKind = "value" | "empty" | "other";

/** One group of a chart: a bar, a point, a slice. */
export interface ChartGroup {
  /** Stable: an option id, a bucket's first day, an account or page id, a text (or `EMPTY_GROUP_KEY`, `OTHER_GROUP_KEY`). */
  readonly key: string;
  readonly label: string;
  /** The label in full (a bucket's whole date), for the tooltip and the accessible name. */
  readonly fullLabel: string;
  readonly kind: ChartGroupKind;
  /** The colour of the option of an option group; absent: the palette. */
  readonly color?: OptionColor;
  /** The palette slot, 0 to 7 (`--chart-1` to `--chart-8`). */
  readonly paletteIndex: number;
  /** The measure over the group's pages; `null` when none of them has the number. */
  readonly value: number | null;
  /** The group's pages, in the order of the rows. */
  readonly pages: readonly PageInfo[];
}

/**
 * What a chart has to draw: `ok`, or why nothing: no group chosen (or the chosen one is gone or cannot group), the measure's
 * property is gone, or no page is left.
 */
export type ChartStatus = "ok" | "no_group" | "no_measure" | "empty";

export interface ChartData {
  readonly status: ChartStatus;
  readonly groups: readonly ChartGroup[];
  /** How many groups the cap of the drawing left out (the "+k more" of a bar chart). */
  readonly hidden: number;
  /** The groups are dates (buckets, in time order, in one series colour). */
  readonly dated: boolean;
  /** The sum of a donut's slices for a count or a sum, else the measure over every page of the slices; `null` without one. */
  readonly total: number | null;
}

const NOTHING: Omit<ChartData, "status"> = { groups: [], hidden: 0, dated: false, total: null };

/** One entry a page has in a source: a group's key and its name. */
interface Item {
  readonly key: string;
  readonly label: string;
}

/** Where the groups of a chart come from, and how a page says which it is in. */
type GroupSource =
  | { readonly kind: "options"; readonly options: readonly PropertyOption[]; readonly read: (page: PageInfo) => readonly string[] }
  | { readonly kind: "checkbox"; readonly read: (page: PageInfo) => readonly boolean[] }
  | { readonly kind: "labels"; readonly read: (page: PageInfo) => readonly Item[] }
  | { readonly kind: "dates"; readonly read: (page: PageInfo) => readonly string[] };

/** The local day of an instant (Unix ms), `[]` for one that is not a time. */
function dayOfInstant(ms: number): readonly string[] {
  return Number.isFinite(ms) ? [todayIso(new Date(ms))] : [];
}

/** The results a formula holds for a page: the items of a list, the result itself otherwise, none without a value. */
function formulaItems(page: PageInfo, def: PropertyDef): readonly FormulaResult[] {
  const value = valueOfPage(page, def.id);
  if (value?.kind !== "formula") {
    return [];
  }
  return value.value.type === "list" ? value.value.data : [value.value];
}

function textItem(text: string): readonly Item[] {
  const trimmed = text.trim();
  return trimmed === "" ? [] : [{ key: trimmed, label: trimmed }];
}

function resolveGroupSource(groupBy: string | null, schema: readonly PropertyDef[], ctx: ResolvedContext): GroupSource | null {
  if (groupBy === GROUP_BY_CREATED) {
    return { kind: "dates", read: (page) => dayOfInstant(page.createdAt) };
  }
  if (groupBy === GROUP_BY_UPDATED) {
    return { kind: "dates", read: (page) => dayOfInstant(page.updatedAt) };
  }
  const def = groupBy === null ? undefined : schema.find((candidate) => candidate.id === groupBy);
  if (def === undefined) {
    return null;
  }
  const kind = groupSourceKind(def, schema);
  const pageItem = (id: PageId): Item => {
    const title = ctx.pageTitle(id);
    return { key: id, label: title === null ? ctx.t("properties.relation.unknown") : title.trim() === "" ? ctx.t("page.untitled") : title };
  };
  const personItem = (id: AccountId): Item => ({ key: id, label: ctx.personName(id) });
  const isFormula = def.kind === "formula";
  switch (kind) {
    case "options":
      return {
        kind,
        options: def.options,
        read: (page) => {
          const value = valueOfPage(page, def.id);
          return value?.kind === "select" || value?.kind === "status" ? [value.value] : value?.kind === "multi_select" ? value.value : [];
        },
      };
    case "checkbox":
      return {
        kind,
        read: (page) => {
          if (isFormula) {
            return formulaItems(page, def).flatMap((item) => (item.type === "boolean" ? [item.data] : []));
          }
          const value = valueOfPage(page, def.id);
          return [value?.kind === "checkbox" && value.value]; // no value is unchecked
        },
      };
    case "people":
      return {
        kind: "labels",
        read: (page) => {
          if (isFormula) {
            return formulaItems(page, def).flatMap((item) => (item.type === "person" ? [personItem(item.data)] : []));
          }
          const value = valueOfPage(page, def.id);
          return value?.kind === "person" ? value.value.map(personItem) : [];
        },
      };
    case "pages":
      return {
        kind: "labels",
        read: (page) => {
          if (isFormula) {
            return formulaItems(page, def).flatMap((item) => (item.type === "page" ? [pageItem(item.data)] : []));
          }
          const value = valueOfPage(page, def.id);
          return value?.kind === "relation" ? value.value.map(pageItem) : [];
        },
      };
    case "text":
      return { kind: "labels", read: (page) => textsOf(page, def, schema) };
    case "dates":
      return {
        kind,
        read: (page) => {
          if (def.kind === "created_time") {
            const value = valueOfPage(page, def.id);
            return dayOfInstant(value?.kind === "created_time" ? value.value : page.createdAt);
          }
          if (def.kind === "last_edited_time") {
            const value = valueOfPage(page, def.id);
            return dayOfInstant(value?.kind === "last_edited_time" ? value.value : page.updatedAt);
          }
          if (isFormula) {
            return formulaItems(page, def).flatMap((item) => (item.type === "date" && isIsoDate(item.data.date) ? [item.data.date] : []));
          }
          const value = valueOfPage(page, def.id);
          return value?.kind === "date" && isIsoDate(value.value.date) ? [value.value.date] : [];
        },
      };
    default:
      return null;
  }
}

/** The texts a page holds in a text-like property, a text formula or a "show original" rollup, trimmed, empty ones left out. */
function textsOf(page: PageInfo, def: PropertyDef, schema: readonly PropertyDef[]): readonly Item[] {
  const value: PropertyValue | undefined = valueOfPage(page, def.id);
  switch (value?.kind) {
    case "text":
    case "url":
    case "email":
    case "phone":
      return textItem(value.value);
    case "place":
      return textItem(value.value.name);
    case "formula": {
      const items = value.value.type === "list" ? value.value.data : [value.value];
      return items.flatMap((item) => (item.type === "text" ? textItem(item.data) : []));
    }
    case "rollup": {
      const { result } = value.value;
      if (result.type !== "values") {
        return [];
      }
      const config = rollupConfigOf(def);
      const target = config === undefined ? undefined : schema.find((candidate) => candidate.id === config.target);
      return result.data.flatMap((item) => textItem(rollupItemText(item, target, (date) => date)));
    }
    default:
      return [];
  }
}

// ----- date buckets ---------------------------------------------------------------------------------

/** The first day of the bucket of `day`. */
function bucketStart(day: string, bucket: ChartBucket, weekStart: number): string {
  switch (bucket) {
    case "day":
      return day;
    case "week":
      return addDays(day, -((weekdayOf(day) - weekStart + 7) % 7));
    case "month":
      return `${day.slice(0, 7)}-01`;
    case "quarter": {
      const month = Math.floor((Number(day.slice(5, 7)) - 1) / 3) * 3 + 1;
      return `${day.slice(0, 4)}-${String(month).padStart(2, "0")}-01`;
    }
    case "year":
      return `${day.slice(0, 4)}-01-01`;
  }
}

/** The first day of the bucket before (`-1`) the one starting on `start`. */
function previousBucket(start: string, bucket: ChartBucket): string {
  switch (bucket) {
    case "day":
      return addDays(start, -1);
    case "week":
      return addDays(start, -7);
    case "month":
      return addMonths(start, -1);
    case "quarter":
      return addMonths(start, -3);
    case "year":
      return addMonths(start, -12);
  }
}

function monthIndex(iso: string): number {
  return Number(iso.slice(0, 4)) * 12 + Number(iso.slice(5, 7));
}

/** How many buckets there are from the one starting on `first` to the one starting on `last`, both included. */
function countBuckets(first: string, last: string, bucket: ChartBucket): number {
  switch (bucket) {
    case "day":
      return daysBetween(first, last) + 1;
    case "week":
      return Math.round(daysBetween(first, last) / 7) + 1;
    case "month":
      return monthIndex(last) - monthIndex(first) + 1;
    case "quarter":
      return Math.round((monthIndex(last) - monthIndex(first)) / 3) + 1;
    case "year":
      return Math.round((monthIndex(last) - monthIndex(first)) / 12) + 1;
  }
}

/** The starts of the buckets from `first` to `last`, every one of them (empty ones included), the latest `limit` at most. */
function bucketRange(first: string, last: string, bucket: ChartBucket, limit: number): { starts: string[]; total: number } {
  const starts: string[] = [];
  let current = last;
  while (starts.length < limit && current >= first) {
    starts.push(current);
    const previous = previousBucket(current, bucket);
    if (previous >= current) {
      break; // the start of the calendar
    }
    current = previous;
  }
  return { starts: starts.reverse(), total: countBuckets(first, last, bucket) };
}

function utcDate(iso: string): Date | null {
  const parts = partsOf(iso);
  if (parts === null) {
    return null;
  }
  const date = new Date(0);
  date.setUTCFullYear(parts.year, parts.month - 1, parts.day);
  date.setUTCHours(0, 0, 0, 0);
  return date;
}

/** The short and the full name of a bucket, from its first day (the key's own text for a day outside the calendar). */
function bucketLabeller(bucket: ChartBucket, ctx: ResolvedContext): (start: string) => { label: string; full: string } {
  const format = (options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat =>
    new Intl.DateTimeFormat(ctx.locale, { ...options, timeZone: "UTC" });
  const dayShort = format({ month: "short", day: "numeric" });
  const dayFull = format({ dateStyle: "medium" });
  const monthShort = format({ month: "short", year: "numeric" });
  const monthFull = format({ month: "long", year: "numeric" });
  return (start) => {
    const date = utcDate(start);
    if (date === null) {
      return { label: start, full: start };
    }
    const year = String(date.getUTCFullYear());
    switch (bucket) {
      case "day":
        return { label: dayShort.format(date), full: dayFull.format(date) };
      case "week":
        return { label: dayShort.format(date), full: ctx.t("chart.weekOf", { date: dayFull.format(date) }) };
      case "month":
        return { label: monthShort.format(date), full: monthFull.format(date) };
      case "quarter": {
        const text = ctx.t("chart.quarter", { quarter: Math.floor(date.getUTCMonth() / 3) + 1, year });
        return { label: text, full: text };
      }
      case "year":
        return { label: year, full: year };
    }
  };
}

// ----- measures --------------------------------------------------------------------------------------

/** What a measure reads: the count, or a function over the number each page holds. */
type MeasureReader =
  | { readonly fn: "count" }
  | { readonly fn: "sum" | "average" | "median" | "min" | "max"; readonly read: (page: PageInfo) => number | undefined };

/** The measure against the schema; `null` when its property is gone or holds no number. */
function resolveMeasure(measure: ChartMeasure, schema: readonly PropertyDef[]): MeasureReader | null {
  if (measure.fn === "count") {
    return { fn: "count" };
  }
  const def = schema.find((candidate) => candidate.id === measure.propertyId);
  if (def === undefined || !isMeasurable(def)) {
    return null;
  }
  return { fn: measure.fn, read: (page) => numberOf(page, def) };
}

/** The measure over `pages`: the count, or the function over the numbers they hold (`null` when none holds one). */
function measureValue(measure: MeasureReader, pages: readonly PageInfo[]): number | null {
  if (measure.fn === "count") {
    return pages.length;
  }
  const numbers: PropertyValue[] = [];
  for (const page of pages) {
    const value = measure.read(page);
    if (value !== undefined) {
      numbers.push({ kind: "number", value });
    }
  }
  if (numbers.length === 0) {
    return null;
  }
  const result = evaluateRollup(measure.fn, numbers);
  return result?.type === "number" ? result.data : null;
}

/** What one page adds to a running total: 1 for a count, its number otherwise (nothing without one). */
function pageAmount(measure: MeasureReader, page: PageInfo): number {
  return measure.fn === "count" ? 1 : (measure.read(page) ?? 0);
}

// ----- grouping ---------------------------------------------------------------------------------------

/** A group while the pages are being sorted into it. */
interface Draft {
  readonly key: string;
  readonly label: string;
  readonly fullLabel: string;
  readonly color?: OptionColor;
  readonly pages: PageInfo[];
}

/** What the source made of the rows: the groups in their natural order, the pages without a value, the buckets dropped. */
interface Grouped {
  readonly drafts: readonly Draft[];
  readonly empty: readonly PageInfo[];
  /** The pages of the buckets that a date source's cap left out, and how many buckets that is. */
  readonly dropped: { readonly pages: readonly PageInfo[]; readonly buckets: number };
}

function unique<T>(items: readonly T[]): T[] {
  return [...new Set(items)];
}

function groupRows(source: GroupSource, rows: readonly PageInfo[], spec: ChartSpec, ctx: ResolvedContext): Grouped {
  const none = { pages: [], buckets: 0 };
  switch (source.kind) {
    case "options": {
      const drafts: Draft[] = source.options.map((option) => ({
        key: option.id,
        label: option.name,
        fullLabel: option.name,
        color: optionColorOf(option.color),
        pages: [],
      }));
      const byKey = new Map(drafts.map((draft) => [draft.key, draft]));
      const empty: PageInfo[] = [];
      for (const page of rows) {
        const found = unique(source.read(page)).flatMap((id) => byKey.get(id) ?? []);
        if (found.length === 0) {
          empty.push(page);
        }
        for (const draft of found) {
          draft.pages.push(page);
        }
      }
      return { drafts, empty, dropped: none };
    }
    case "checkbox": {
      const checked: Draft = { key: "checked", label: ctx.t("chart.checked"), fullLabel: ctx.t("chart.checked"), pages: [] };
      const unchecked: Draft = { key: "unchecked", label: ctx.t("chart.unchecked"), fullLabel: ctx.t("chart.unchecked"), pages: [] };
      const empty: PageInfo[] = [];
      for (const page of rows) {
        const values = unique(source.read(page));
        if (values.length === 0) {
          empty.push(page);
        }
        if (values.includes(true)) {
          checked.pages.push(page);
        }
        if (values.includes(false)) {
          unchecked.pages.push(page);
        }
      }
      return { drafts: [checked, unchecked], empty, dropped: none };
    }
    case "labels": {
      const byKey = new Map<string, Draft>();
      const empty: PageInfo[] = [];
      for (const page of rows) {
        const items = source.read(page);
        if (items.length === 0) {
          empty.push(page);
        }
        for (const item of items) {
          let draft = byKey.get(item.key);
          if (draft === undefined) {
            draft = { key: item.key, label: item.label, fullLabel: item.label, pages: [] };
            byKey.set(item.key, draft);
          }
          if (!draft.pages.includes(page)) {
            draft.pages.push(page);
          }
        }
      }
      let collator: Intl.Collator;
      try {
        collator = new Intl.Collator(ctx.locale, { numeric: true, sensitivity: "base" });
      } catch {
        collator = new Intl.Collator();
      }
      const drafts = [...byKey.values()].sort((a, b) => collator.compare(a.label, b.label) || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
      return { drafts, empty, dropped: none };
    }
    case "dates": {
      const bucket = spec.bucket ?? "month";
      const placed: { page: PageInfo; keys: string[] }[] = [];
      const empty: PageInfo[] = [];
      let first = null as string | null;
      let last = null as string | null;
      for (const page of rows) {
        const keys = unique(source.read(page).map((day) => bucketStart(day, bucket, ctx.weekStart)));
        if (keys.length === 0) {
          empty.push(page);
          continue;
        }
        placed.push({ page, keys });
        for (const key of keys) {
          first = first === null || key < first ? key : first;
          last = last === null || key > last ? key : last;
        }
      }
      if (first === null || last === null) {
        return { drafts: [], empty, dropped: none };
      }
      const range = bucketRange(first, last, bucket, CHART_MAX_BUCKETS);
      const label = bucketLabeller(bucket, ctx);
      const drafts: Draft[] = range.starts.map((start) => {
        const names = label(start);
        return { key: start, label: names.label, fullLabel: names.full, pages: [] };
      });
      const byKey = new Map(drafts.map((draft) => [draft.key, draft]));
      const droppedPages: PageInfo[] = [];
      for (const { page, keys } of placed) {
        for (const key of keys) {
          const draft = byKey.get(key);
          if (draft === undefined) {
            droppedPages.push(page);
          } else {
            draft.pages.push(page);
          }
        }
      }
      return { drafts, empty, dropped: { pages: droppedPages, buckets: range.total - range.starts.length } };
    }
  }
}

/** Orders groups by value (`null` last, ties in their natural order). */
function orderByValue(groups: readonly ChartGroup[], order: ChartOrder): ChartGroup[] {
  if (order === "default") {
    return [...groups];
  }
  const sign = order === "value_desc" ? -1 : 1;
  return groups
    .map((group, index) => ({ group, index }))
    .sort((a, b) => {
      if (a.group.value === null || b.group.value === null) {
        return a.group.value === b.group.value ? a.index - b.index : a.group.value === null ? 1 : -1;
      }
      return sign * (a.group.value - b.group.value) || a.index - b.index;
    })
    .map(({ group }) => group);
}

/** The pages of several groups, each once, in the order they come in. */
function unionOf(groups: readonly ChartGroup[]): PageInfo[] {
  const seen = new Set<PageInfo>();
  for (const group of groups) {
    for (const page of group.pages) {
      seen.add(page);
    }
  }
  return [...seen];
}

/** The donut's slices: the positive values, the biggest `CHART_MAX_SLICES` named and the rest in "Other". */
function sliceGroups(groups: readonly ChartGroup[], measure: MeasureReader, ctx: ResolvedContext): ChartGroup[] {
  const positive = groups.filter((group) => group.value !== null && group.value > 0);
  let named = positive;
  let rest: ChartGroup[] = [];
  if (positive.length > CHART_MAX_SLICES) {
    const biggest = new Set(
      positive
        .map((group, index) => ({ group, index }))
        .sort((a, b) => (b.group.value ?? 0) - (a.group.value ?? 0) || a.index - b.index)
        .slice(0, CHART_MAX_SLICES)
        .map(({ group }) => group),
    );
    named = positive.filter((group) => biggest.has(group));
    rest = positive.filter((group) => !biggest.has(group));
  }
  // Slices with no option colour take the palette in order, so that no two of the (at most) eight share one.
  const slices = named.map((group, index) => (group.color === undefined ? { ...group, paletteIndex: index % CHART_PALETTE_SIZE } : group));
  if (rest.length > 0) {
    const pages = unionOf(rest);
    const additive = measure.fn === "count" || measure.fn === "sum";
    slices.push({
      key: OTHER_GROUP_KEY,
      label: ctx.t("chart.other"),
      fullLabel: ctx.t("chart.other"),
      kind: "other",
      paletteIndex: 0,
      value: additive ? rest.reduce((total, group) => total + (group.value ?? 0), 0) : measureValue(measure, pages),
      pages,
    });
  }
  return slices;
}

/**
 * What a chart draws for `rows` (the pages after the dashboard's and the widget's filters): the groups of the spec's source
 * with their values, ordered and capped for the chart's type (section 4). Never throws: a source or a measure that cannot
 * be read answers a status and no groups.
 */
export function chartData(spec: ChartSpec, rows: readonly PageInfo[], schema: readonly PropertyDef[], context: ChartContext = {}): ChartData {
  const ctx = resolveContext(context);
  const source = resolveGroupSource(spec.groupBy, schema, ctx);
  if (source === null) {
    return { status: "no_group", ...NOTHING };
  }
  const measure = resolveMeasure(spec.measure, schema);
  if (measure === null) {
    return { status: "no_measure", ...NOTHING };
  }
  const dated = source.kind === "dates";
  const grouped = groupRows(source, rows, spec, ctx);

  // The groups in their natural order, then ordered, then cumulated, then the group of pages without a value.
  let groups: ChartGroup[] = grouped.drafts.map((draft, index): ChartGroup => ({
    key: draft.key,
    label: draft.label,
    fullLabel: draft.fullLabel,
    kind: "value",
    ...(draft.color === undefined ? {} : { color: draft.color }),
    paletteIndex: index % CHART_PALETTE_SIZE,
    value: measureValue(measure, draft.pages),
    pages: draft.pages,
  }));
  if (!dated) {
    groups = orderByValue(groups, spec.order ?? "default");
  }
  if (spec.type === "line" && spec.cumulative === true) {
    const baseline = grouped.dropped.pages.reduce((total, page) => total + pageAmount(measure, page), 0);
    let running: number | null = grouped.dropped.pages.length > 0 ? baseline : null;
    groups = groups.map((group) => {
      if (group.value !== null) {
        running = (running ?? 0) + group.value;
      }
      return { ...group, value: running };
    });
  }
  // A line through a time axis has no place for "no date".
  if (spec.showEmpty === true && grouped.empty.length > 0 && !(spec.type === "line" && dated)) {
    groups.push({
      key: EMPTY_GROUP_KEY,
      label: ctx.t("chart.noValue"),
      fullLabel: ctx.t("chart.noValue"),
      kind: "empty",
      paletteIndex: 0,
      value: measureValue(measure, grouped.empty),
      pages: grouped.empty,
    });
  }

  let hidden = grouped.dropped.buckets;
  if (spec.type === "donut") {
    groups = sliceGroups(groups, measure, ctx);
  } else {
    const cap = spec.type === "bar" ? CHART_MAX_BARS : CHART_MAX_POINTS;
    if (groups.length > cap) {
      hidden += groups.length - cap;
      groups = dated ? groups.slice(-cap) : groups.slice(0, cap);
    }
  }

  if (!groups.some((group) => group.pages.length > 0 && group.value !== null)) {
    return { status: "empty", ...NOTHING, dated };
  }
  const additive = measure.fn === "count" || measure.fn === "sum";
  const total =
    spec.type !== "donut"
      ? null
      : additive
        ? groups.reduce((sum, group) => sum + (group.value ?? 0), 0)
        : measureValue(measure, unionOf(groups));
  return { status: "ok", groups, hidden, dated, total };
}
