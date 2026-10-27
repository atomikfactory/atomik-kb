// The filters of a view (ADR-0016, amendment of 2026-10-08; ADR-0058): a list of `{ propertyId, op, value? }`
// combined with AND, evaluated here over the pages the view already has (the children of the page, the pages of a tag, a
// linked page's children or the space), for the list and the table layout alike. The operators mirror the core's
// `PropertyFilter` (has a value, equals, contains an option, text contains, a date between, checked,
// a checklist complete, lists a person, lists me) and add what a person asks of a view: is empty, "is not" and "does not
// contain" for texts, numbers, options, pages and people, a number greater or less than, a text that starts or ends with,
// a date on, before, after, on or before, on or after a day, or within a period. A formula (ADR-0055) is computed on read and filtered
// here, by the type of its result: a number is compared (is, is not, greater, less, at least, at most), a text is, is not,
// contains or does not contain, a boolean is checked or not, a day is, before, after or between, a list, a page or a person
// contains a text; every one is empty or not. "Me" (a Person property, ADR-0049) is a condition of its own, not a person
// picked: a saved view is shared and who "me" is differs per viewer, so it is worked out here from the viewer's own
// account (`matchFilter`'s `me`).
//
// A "not" operator is true for a page without a value ("does not contain x" lists the empty pages too, as Notion does),
// except "is not" on a number, which needs a number. A day operator takes a day (`YYYY-MM-DD`) or a relative day
// (`{ relative: "today" | "yesterday" | "tomorrow" }`), `within` a relative period (`{ relative: "thisWeek" … "nextDays", n? }`);
// relative values are resolved at the viewer's own `today` and first day of the week (`FilterClock`), so a shared view
// means "this week" for each reader. A filter this version cannot apply (its property is gone, its operator is not one of
// that kind, or it is still missing its value, or holds a value of another shape) is **ignored, and kept**: it hides
// nothing and is only ever replaced by a control the person uses.

import { isEmptyPropertyValue } from "@atomik-kb/core-client";
import type {
  FormulaResult,
  PageInfo,
  PropertyDef,
  PropertyId,
  PropertyKind,
  PropertyValue,
  RollupFunction,
} from "@atomik-kb/core-client";

import { formulaItems, formulaPlainText, formulaResultOf, formulaShapeOf } from "../formula-format";
import type { FormulaLookups, FormulaShape } from "../formula-format";
import { rollupConfigOf } from "../property-format";
import { addDays, daysInMonth, firstDayOfWeek, isIsoDate, isoOf, partsOf, todayIso, weekdayOf } from "./dates";

/** The operators. Which kinds take which is `filterOps`. */
export type FilterOp =
  | "isSet"
  | "isEmpty"
  | "equals"
  | "contains"
  | "hasOption"
  | "before"
  | "after"
  | "between"
  | "checked"
  | "unchecked"
  | "complete"
  | "incomplete"
  | "relatesTo"
  | "isMe"
  | "hasPerson"
  | "notEquals"
  | "greater"
  | "less"
  | "atLeast"
  | "atMost"
  | "notContains"
  | "startsWith"
  | "endsWith"
  | "lacksOption"
  | "notRelatesTo"
  | "notMe"
  | "lacksPerson"
  | "onOrBefore"
  | "onOrAfter"
  | "within";

/** A day range of a `between` filter, `YYYY-MM-DD`, each end optional and inclusive. */
export interface DateRange {
  from?: string;
  to?: string;
}

/** The relative days a day operator takes instead of a fixed day. */
export type RelativeDay = "today" | "yesterday" | "tomorrow";
export const RELATIVE_DAYS: readonly RelativeDay[] = ["today", "yesterday", "tomorrow"];

/** The relative periods `within` takes. */
export type RelativePeriodName =
  | "thisWeek"
  | "lastWeek"
  | "nextWeek"
  | "thisMonth"
  | "lastMonth"
  | "nextMonth"
  | "pastDays"
  | "nextDays";
export const RELATIVE_PERIODS: readonly RelativePeriodName[] = [
  "thisWeek",
  "lastWeek",
  "nextWeek",
  "thisMonth",
  "lastMonth",
  "nextMonth",
  "pastDays",
  "nextDays",
];
/** The periods that count days (`n`). */
const COUNTED_PERIODS: readonly string[] = ["pastDays", "nextDays"];
/** Most days `n` of `pastDays` and `nextDays` can be (and 1 the fewest). */
export const MAX_RELATIVE_DAYS = 365;
/** The `n` a freshly chosen "past days" or "next days" starts with. */
export const DEFAULT_RELATIVE_DAYS = 7;

/** A relative period as stored: `n` only for the periods that count days. */
export interface RelativePeriod {
  readonly relative: RelativePeriodName;
  readonly n?: number;
}

/**
 * What relative values are resolved at: the viewer's local day (`YYYY-MM-DD`) and the first day of their week (0 for
 * Sunday to 6 for Saturday). Each defaults to the device's clock and locale when left out.
 */
export interface FilterClock {
  readonly today?: string;
  readonly weekStart?: number;
}

/** One filter as stored: `op` and `value` are read as they come (`readFilters`), a stored filter may hold anything. */
export interface PageListFilter {
  readonly propertyId: PropertyId;
  readonly op: string;
  readonly value?: unknown;
}

const OPTION_KINDS: readonly string[] = ["select", "status", "multi_select"];
const TEXT_KINDS: readonly string[] = ["text", "url", "email", "phone"];

const TIME_KINDS: readonly string[] = ["created_time", "last_edited_time"];
const DEVICE_KINDS: readonly string[] = ["created_by", "last_edited_by"];
/** The rollup functions whose result is a day, a list or a progress rather than a number. */
const NON_NUMERIC_ROLLUPS: readonly RollupFunction[] = ["earliest", "latest", "show_original", "checklist_progress"];

/** The operators of a number, a numeric rollup and a formula's number. */
const NUMBER_OPS: readonly FilterOp[] = ["equals", "notEquals", "greater", "less", "atLeast", "atMost", "isSet", "isEmpty"];
/** The operators that compare a number with the filter's number. */
type NumberComparison = "greater" | "less" | "atLeast" | "atMost";

/** Whether the rollup `def` gives a number (so that "is" can compare it); without a definition, the answer is no. */
function isNumericRollup(def: PropertyDef | undefined): boolean {
  const config = def === undefined ? undefined : rollupConfigOf(def);
  return config !== undefined && !NON_NUMERIC_ROLLUPS.includes(config.function);
}

/** The operators of a formula, by the shape of its result (one that does not check has no value to compare). */
function formulaOps(shape: FormulaShape): readonly FilterOp[] {
  switch (shape) {
    case "number":
      return NUMBER_OPS;
    case "text":
      return ["equals", "notEquals", "contains", "notContains", "isSet", "isEmpty"];
    case "boolean":
      return ["checked", "unchecked"];
    case "date":
      return ["equals", "before", "after", "between", "isSet", "isEmpty"];
    case "list":
    case "page":
    case "person":
      return ["contains", "isSet", "isEmpty"];
    case "none":
      return ["isSet", "isEmpty"];
  }
}

/**
 * The operators a property of `kind` offers, in the order the editor lists them (a kind this
 * version does not know: only the two generic ones). `def` tells a numeric rollup (which also
 * offers "is") from the other rollups, and the type of a formula's result.
 */
export function filterOps(kind: PropertyKind, def?: PropertyDef): readonly FilterOp[] {
  if (kind === "formula") {
    return formulaOps(def === undefined ? "none" : formulaShapeOf(def));
  }
  if (TIME_KINDS.includes(kind)) {
    return ["equals", "before", "after", "onOrBefore", "onOrAfter", "between", "within", "isSet"];
  }
  if (DEVICE_KINDS.includes(kind)) {
    return ["equals", "isSet"];
  }
  if (kind === "rollup") {
    return isNumericRollup(def) ? NUMBER_OPS : ["isSet", "isEmpty"];
  }
  if (kind === "relation") {
    return ["relatesTo", "notRelatesTo", "isSet", "isEmpty"];
  }
  if (kind === "person") {
    return ["isMe", "notMe", "hasPerson", "lacksPerson", "isSet", "isEmpty"];
  }
  if (kind === "place") {
    return ["contains", "notContains", "isSet", "isEmpty"];
  }
  if (TEXT_KINDS.includes(kind)) {
    return ["contains", "notContains", "equals", "notEquals", "startsWith", "endsWith", "isSet", "isEmpty"];
  }
  if (OPTION_KINDS.includes(kind)) {
    return ["hasOption", "lacksOption", "isSet", "isEmpty"];
  }
  switch (kind) {
    case "number":
      return NUMBER_OPS;
    case "date":
      return ["equals", "before", "after", "onOrBefore", "onOrAfter", "between", "within", "isSet", "isEmpty"];
    case "checkbox":
      return ["checked", "unchecked"];
    case "checklist":
      return ["complete", "incomplete", "isSet", "isEmpty"];
    default:
      return ["isSet", "isEmpty"];
  }
}

/** Whether `op` is one of the operators of `kind`. */
export function isFilterOp(kind: PropertyKind, op: string, def?: PropertyDef): op is FilterOp {
  return (filterOps(kind, def) as readonly string[]).includes(op);
}

/** The operators that need a value (the others stand alone). */
export function opTakesValue(op: string): boolean {
  return [
    "equals",
    "contains",
    "hasOption",
    "before",
    "after",
    "between",
    "relatesTo",
    "hasPerson",
    "notEquals",
    "greater",
    "less",
    "atLeast",
    "atMost",
    "notContains",
    "startsWith",
    "endsWith",
    "lacksOption",
    "notRelatesTo",
    "lacksPerson",
    "onOrBefore",
    "onOrAfter",
    "within",
  ].includes(op);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The filters an attribute holds: the entries that are objects with a property id and an operator; anything else reads as none. */
export function readFilters(value: unknown): PageListFilter[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const filters: PageListFilter[] = [];
  for (const item of value as unknown[]) {
    if (isRecord(item) && typeof item.propertyId === "string" && item.propertyId !== "" && typeof item.op === "string") {
      filters.push("value" in item ? { propertyId: item.propertyId, op: item.op, value: item.value } : { propertyId: item.propertyId, op: item.op });
    }
  }
  return filters;
}

/** A new filter for `def`: its first operator, no value yet (so it hides nothing until one is given). */
export function newFilter(def: PropertyDef): PageListFilter {
  const [op] = filterOps(def.kind, def);
  return { propertyId: def.id, op: op ?? "isSet" };
}

/**
 * A filter on `propertyId` for a condition the person just chose: no value yet (the old one may mean something else),
 * except `within`, whose select always shows a period, so it starts at "this week" (the value it shows).
 */
export function filterForOp(propertyId: PropertyId, op: string): PageListFilter {
  return op === "within" ? { propertyId, op, value: { relative: "thisWeek" } } : { propertyId, op };
}

/** The range a `between` value holds (each end only when it is a day). */
export function readDateRange(value: unknown): DateRange {
  const range: DateRange = {};
  if (isRecord(value)) {
    if (typeof value.from === "string" && value.from !== "") {
      range.from = value.from;
    }
    if (typeof value.to === "string" && value.to !== "") {
      range.to = value.to;
    }
  }
  return range;
}

/** The relative day a value holds, `null` for a fixed day or anything else. */
export function readRelativeDay(value: unknown): RelativeDay | null {
  return isRecord(value) ? (RELATIVE_DAYS.find((day) => day === value.relative) ?? null) : null;
}

/** Whether `n` is a number of days a period may count: an integer from 1 to 365. */
export function isRelativeCount(n: unknown): n is number {
  return typeof n === "number" && Number.isInteger(n) && n >= 1 && n <= MAX_RELATIVE_DAYS;
}

/** Whether a period counts days (`n`): the past and the next days. */
export function periodCountsDays(period: RelativePeriodName): boolean {
  return COUNTED_PERIODS.includes(period);
}

/**
 * The relative period a value holds (`null` for anything else); `n` is kept only for the periods that count days and only
 * when it is a usable count (without one the period cannot be applied: `periodRange`).
 */
export function readRelativePeriod(value: unknown): RelativePeriod | null {
  if (!isRecord(value)) {
    return null;
  }
  const relative = RELATIVE_PERIODS.find((period) => period === value.relative);
  if (relative === undefined) {
    return null;
  }
  return periodCountsDays(relative) && isRelativeCount(value.n) ? { relative, n: value.n } : { relative };
}

// ----- days ----------------------------------------------------------------------------------

/** The viewer's day: the clock's, else the device's (a text that is no day reads as the device's). */
function clockToday(clock: FilterClock): string {
  return clock.today !== undefined && isIsoDate(clock.today) ? clock.today : todayIso();
}

/** The first day of the viewer's week, 0 (Sunday) to 6. */
function clockWeekStart(clock: FilterClock): number {
  const first = clock.weekStart;
  return typeof first === "number" && Number.isInteger(first) ? ((first % 7) + 7) % 7 : firstDayOfWeek();
}

/** The day a day operator's value names: a `YYYY-MM-DD` day or a relative day; `null` for anything else. */
export function resolveDay(value: unknown, clock: FilterClock = {}): string | null {
  if (typeof value === "string") {
    return isIsoDate(value) ? value : null;
  }
  switch (readRelativeDay(value)) {
    case "today":
      return clockToday(clock);
    case "yesterday":
      return addDays(clockToday(clock), -1);
    case "tomorrow":
      return addDays(clockToday(clock), 1);
    case null:
      return null;
  }
}

/** The first and the last day (inclusive) of the calendar month `offset` months from the one `today` is in. */
function monthRange(today: string, offset: number): readonly [string, string] | null {
  const parts = partsOf(today);
  if (parts === null) {
    return null;
  }
  const index = parts.year * 12 + (parts.month - 1) + offset;
  const year = Math.floor(index / 12);
  const month = (index % 12) + 1;
  return [isoOf({ year, month, day: 1 }), isoOf({ year, month, day: daysInMonth(year, month) })];
}

/**
 * The first and the last day (inclusive) a `within` value names, `null` when it is not a relative period or lacks the
 * count of days it needs (1 to 365). Weeks start on the clock's first day, months are calendar months, `pastDays` is
 * `[today − n, today]` and `nextDays` `[today, today + n]`.
 */
export function periodRange(value: unknown, clock: FilterClock = {}): readonly [string, string] | null {
  const period = readRelativePeriod(value);
  if (period === null) {
    return null;
  }
  const today = clockToday(clock);
  switch (period.relative) {
    case "thisWeek":
    case "lastWeek":
    case "nextWeek": {
      const shift = period.relative === "thisWeek" ? 0 : period.relative === "lastWeek" ? -7 : 7;
      const start = addDays(today, shift - ((weekdayOf(today) - clockWeekStart(clock) + 7) % 7));
      return [start, addDays(start, 6)];
    }
    case "thisMonth":
      return monthRange(today, 0);
    case "lastMonth":
      return monthRange(today, -1);
    case "nextMonth":
      return monthRange(today, 1);
    case "pastDays":
      return period.n === undefined ? null : [addDays(today, -period.n), today];
    case "nextDays":
      return period.n === undefined ? null : [today, addDays(today, period.n)];
  }
}

/**
 * Whether the day `day` (`undefined`: the page has none, so no day condition holds) passes the day operator `op` with
 * `argument`; `null` when the value the condition needs is missing or is not one. Days are compared as their
 * `YYYY-MM-DD` texts, which order like the days they name.
 */
function matchDay(op: string, day: string | undefined, argument: unknown, clock: FilterClock): boolean | null {
  if (op === "between") {
    const { from, to } = readDateRange(argument);
    if (from === undefined && to === undefined) {
      return null;
    }
    if ((from !== undefined && !isIsoDate(from)) || (to !== undefined && !isIsoDate(to))) {
      return null;
    }
    return day !== undefined && (from === undefined || day >= from) && (to === undefined || day <= to);
  }
  if (op === "within") {
    const range = periodRange(argument, clock);
    return range === null ? null : day !== undefined && day >= range[0] && day <= range[1];
  }
  const target = resolveDay(argument, clock);
  if (target === null) {
    return null;
  }
  switch (op) {
    case "equals":
      return day === target;
    case "before":
      return day !== undefined && day < target;
    case "after":
      return day !== undefined && day > target;
    case "onOrBefore":
      return day !== undefined && day <= target;
    case "onOrAfter":
      return day !== undefined && day >= target;
    default:
      return null;
  }
}

// ----- values --------------------------------------------------------------------------------

function valueOf(page: PageInfo, propertyId: PropertyId): PropertyValue | undefined {
  return page.properties?.find(([id]) => id === propertyId)?.[1];
}

function hasValue(value: PropertyValue | undefined): boolean {
  return value !== undefined && !isEmptyPropertyValue(value);
}

function asciiLower(text: string): string {
  return text.replace(/[A-Z]/g, (letter) => letter.toLowerCase());
}

/** The text of a text-like value (a place is its name). */
function textOf(value: PropertyValue | undefined): string {
  if (value?.kind === "place") {
    return value.value.name;
  }
  return value !== undefined && TEXT_KINDS.includes(value.kind) ? String((value as { value: string }).value) : "";
}

/** The instant of a time property (created / last edited), `undefined` when the page has none. */
function instantOf(value: PropertyValue | undefined): number | undefined {
  return value?.kind === "created_time" || value?.kind === "last_edited_time" ? value.value : undefined;
}

/** The day a date property holds, or the viewer's local day a time property's instant falls on; `undefined` when the page has none. */
function dayOf(kind: string, value: PropertyValue | undefined): string | undefined {
  if (TIME_KINDS.includes(kind)) {
    const instant = instantOf(value);
    return instant !== undefined && Number.isFinite(instant) ? todayIso(new Date(instant)) : undefined;
  }
  return value?.kind === "date" ? value.value.date : undefined;
}

/** The number a number property or a numeric rollup holds; `undefined` when the page has none. */
function numberOf(kind: string, value: PropertyValue | undefined): number | undefined {
  if (kind === "rollup") {
    return value?.kind === "rollup" && value.value.result.type === "number" ? value.value.result.data : undefined;
  }
  return value?.kind === "number" ? value.value : undefined;
}

/** `data` compared with the filter's `argument` by one of the four comparisons. */
function compareNumber(op: NumberComparison, data: number, argument: number): boolean {
  return op === "greater" ? data > argument : op === "less" ? data < argument : op === "atLeast" ? data >= argument : data <= argument;
}

/** Whether `argument` is a number a comparison can use. */
function isNumberArgument(argument: unknown): argument is number {
  return typeof argument === "number" && Number.isFinite(argument);
}

/** Whether any item of a formula's result contains `needle` (ASCII case-insensitive), items read as plain text. */
function resultContains(result: FormulaResult | undefined, needle: string, lookups: FormulaLookups): boolean {
  const wanted = asciiLower(needle.trim());
  return result !== undefined && formulaItems(result).some((item) => asciiLower(formulaPlainText(item, lookups)).includes(wanted));
}

/**
 * Whether the result of a formula (`undefined`: it has none) passes `op` with `argument`; `null` when the value the
 * condition needs is missing or is not one. The operators are the ones of `formulaOps` for the shape of the result.
 */
function matchFormula(
  result: FormulaResult | undefined,
  shape: FormulaShape,
  op: string,
  argument: unknown,
  lookups: FormulaLookups,
  clock: FilterClock,
): boolean | null {
  switch (op) {
    case "isSet":
      return result !== undefined;
    case "isEmpty":
      return result === undefined;
    case "checked":
      return result?.type === "boolean" && result.data;
    case "unchecked":
      return !(result?.type === "boolean" && result.data);
    case "equals":
    case "notEquals": {
      if (shape === "date") {
        return matchDay("equals", result?.type === "date" ? result.data.date : undefined, argument, clock);
      }
      let same: boolean;
      if (shape === "number") {
        if (!isNumberArgument(argument)) {
          return null;
        }
        same = result?.type === "number" && result.data === argument;
      } else {
        if (typeof argument !== "string" || argument === "") {
          return null;
        }
        same = result !== undefined && asciiLower(formulaPlainText(result, lookups)) === asciiLower(argument);
      }
      return op === "equals" ? same : !same;
    }
    case "greater":
    case "less":
    case "atLeast":
    case "atMost": {
      if (!isNumberArgument(argument)) {
        return null;
      }
      return result?.type === "number" && compareNumber(op, result.data, argument);
    }
    case "contains":
    case "notContains": {
      if (typeof argument !== "string" || argument.trim() === "") {
        return null;
      }
      const found = resultContains(result, argument, lookups);
      return op === "contains" ? found : !found;
    }
    case "before":
    case "after":
    case "between":
      return matchDay(op, result?.type === "date" ? result.data.date : undefined, argument, clock);
    default:
      return null;
  }
}

/**
 * Whether `page` passes `filter`; `null` when the filter cannot be applied (its property is not in
 * `defs`, the operator is not one of the property's kind, or the value it needs is missing). `me` is the viewer's own
 * account in the page's space (`undefined` while it is not known: a "me" filter then cannot be applied). `lookups` give
 * the titles and names a text filter on a formula's pages and people reads (without them those have no text). `clock` is
 * the day and the first day of the week relative values are resolved at (the device's own when left out).
 */
export function matchFilter(
  page: PageInfo,
  filter: PageListFilter,
  defs: readonly PropertyDef[],
  me?: string,
  lookups: FormulaLookups = {},
  clock: FilterClock = {},
): boolean | null {
  const def = defs.find((candidate) => candidate.id === filter.propertyId);
  if (def === undefined || !isFilterOp(def.kind, filter.op, def)) {
    return null;
  }
  const value = valueOf(page, def.id);
  const argument = filter.value;
  if (def.kind === "formula") {
    return matchFormula(formulaResultOf(value), formulaShapeOf(def), filter.op, argument, lookups, clock);
  }
  switch (filter.op) {
    case "isSet":
      return hasValue(value);
    case "isEmpty":
      return !hasValue(value);
    case "checked":
      return value?.kind === "checkbox" && value.value;
    case "unchecked":
      return !(value?.kind === "checkbox" && value.value);
    case "complete":
      return value?.kind === "checklist" && value.value.length > 0 && value.value.every((item) => item.done);
    case "incomplete":
      return value?.kind === "checklist" && value.value.some((item) => !item.done);
    case "hasOption":
    case "lacksOption": {
      if (typeof argument !== "string" || argument === "") {
        return null;
      }
      const has =
        value?.kind === "multi_select"
          ? value.value.includes(argument)
          : (value?.kind === "select" || value?.kind === "status") && value.value === argument;
      return filter.op === "hasOption" ? has : !has;
    }
    case "contains":
    case "notContains":
    case "startsWith":
    case "endsWith": {
      if (typeof argument !== "string" || argument.trim() === "") {
        return null;
      }
      const text = asciiLower(textOf(value));
      const wanted = asciiLower(argument.trim());
      switch (filter.op) {
        case "startsWith":
          return text.trimStart().startsWith(wanted);
        case "endsWith":
          return text.trimEnd().endsWith(wanted);
        case "contains":
          return text.includes(wanted);
        default:
          return !text.includes(wanted);
      }
    }
    case "equals":
    case "notEquals": {
      if (filter.op === "equals" && DEVICE_KINDS.includes(def.kind)) {
        if (typeof argument !== "string" || argument === "") {
          return null;
        }
        return (value?.kind === "created_by" || value?.kind === "last_edited_by") && value.value === argument;
      }
      if (def.kind === "date" || TIME_KINDS.includes(def.kind)) {
        return matchDay("equals", dayOf(def.kind, value), argument, clock);
      }
      if (def.kind === "number" || def.kind === "rollup") {
        if (!isNumberArgument(argument)) {
          return null;
        }
        const data = numberOf(def.kind, value);
        // "Is not" needs a number: a page without one is not listed.
        return filter.op === "equals" ? data === argument : data !== undefined && data !== argument;
      }
      if (typeof argument !== "string" || argument === "") {
        return null;
      }
      const same = asciiLower(textOf(value)) === asciiLower(argument);
      return filter.op === "equals" ? same : !same;
    }
    case "greater":
    case "less":
    case "atLeast":
    case "atMost": {
      if (!isNumberArgument(argument)) {
        return null;
      }
      const data = numberOf(def.kind, value);
      return data !== undefined && compareNumber(filter.op, data, argument);
    }
    case "relatesTo":
    case "notRelatesTo": {
      if (typeof argument !== "string" || argument === "") {
        return null;
      }
      const related = value?.kind === "relation" && value.value.includes(argument);
      return filter.op === "relatesTo" ? related : !related;
    }
    case "isMe":
    case "notMe": {
      if (me === undefined) {
        return null;
      }
      const mine = value?.kind === "person" && value.value.includes(me);
      return filter.op === "isMe" ? mine : !mine;
    }
    case "hasPerson":
    case "lacksPerson": {
      if (typeof argument !== "string" || argument === "") {
        return null;
      }
      const listed = value?.kind === "person" && value.value.includes(argument);
      return filter.op === "hasPerson" ? listed : !listed;
    }
    case "before":
    case "after":
    case "onOrBefore":
    case "onOrAfter":
    case "between":
    case "within":
      return matchDay(filter.op, dayOf(def.kind, value), argument, clock);
    default:
      // The operators of a formula's result: only a formula offers them, and its filters were answered above.
      return null;
  }
}

/** The pages that pass every filter that can be applied (AND), in their order; `me` is the viewer's own account. */
export function applyFilters(
  pages: readonly PageInfo[],
  filters: readonly PageListFilter[],
  defs: readonly PropertyDef[],
  me?: string,
  lookups?: FormulaLookups,
  clock?: FilterClock,
): PageInfo[] {
  if (filters.length === 0) {
    return [...pages];
  }
  return pages.filter((page) => filters.every((filter) => matchFilter(page, filter, defs, me, lookups, clock) !== false));
}
