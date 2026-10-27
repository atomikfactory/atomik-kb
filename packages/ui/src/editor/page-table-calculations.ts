// The calculations of the table layout's footer (ADR-0040 item 5, ADR-0016 amendment of 2026-10-14): a
// `pageList` block with `layout: "table"` keeps one optional function per column in its `calculations`
// attribute (`{ "<property id>" | "title": "<function>" }`), and the footer row shows what the function
// makes of the rows the view shows (after the filters). This file holds the rules that need no React: which
// functions a column offers, how the attribute is read, and the arithmetic.
//
// The functions are the rollup functions of ADR-0027 that fit a column (`evaluateRollup` does the
// arithmetic, `rollupTexts` the number and date formatting, so that a footer reads like a rollup), with
// three additions a footer needs and a rollup does not: "count not empty" (the rows that have a value),
// "count unchecked" and "date range" (the days between the earliest and the latest date). "Count values"
// counts values rather than rows: an option of a multi-select, a page of a relation, a file or an item of
// a checklist each count once, and any other value counts once, so it differs from "count not empty" for
// the kinds that hold a list (a rollup's `count_values` counts the pages that have a value, which is what
// "count not empty" is here). A formula (ADR-0055) is read by the type of its result: a number gets the number functions,
// a day the date functions, a boolean the checkbox functions, anything else the counts.

import { evaluateRollup, isEmptyPropertyValue } from "@atomik-kb/core-client";
import type { DateValue, PageInfo, PropertyDef, PropertyValue, RollupFunction, RollupResult } from "@atomik-kb/core-client";

import { formulaCellValue, formulaResultOf, formulaShapeOf } from "../formula-format";
import { tp } from "../i18n";
import type { MessageKey } from "../i18n";
import { formatInstant, formatNumber, rollupConfigOf, rollupTexts } from "../property-format";
import { daysBetween, formatDateMention, todayIso } from "./dates";
import { valueOfPage } from "./page-list-model";

/** Every function a footer can show, in the order the menu lists them. */
export const CALCULATION_FUNCTIONS = [
  "count_all",
  "count_values",
  "count_unique",
  "count_empty",
  "count_not_empty",
  "percent_empty",
  "percent_not_empty",
  "sum",
  "average",
  "median",
  "min",
  "max",
  "range",
  "earliest",
  "latest",
  "date_range",
  "count_checked",
  "count_unchecked",
  "percent_checked",
] as const;

export type CalculationFunction = (typeof CALCULATION_FUNCTIONS)[number];

/** Whether `name` is a function of this version. */
export function isCalculationFunction(name: unknown): name is CalculationFunction {
  return typeof name === "string" && (CALCULATION_FUNCTIONS as readonly string[]).includes(name);
}

/** The name of each function in the menu and in the footer (the rollup's where it fits). */
export const CALCULATION_LABELS: Readonly<Record<CalculationFunction, MessageKey>> = {
  count_all: "properties.rollup.fn.count_all",
  count_values: "properties.rollup.fn.count_values",
  count_unique: "properties.rollup.fn.count_unique",
  count_empty: "properties.rollup.fn.count_empty",
  count_not_empty: "pageTable.calc.fn.count_not_empty",
  percent_empty: "properties.rollup.fn.percent_empty",
  percent_not_empty: "properties.rollup.fn.percent_not_empty",
  sum: "properties.rollup.fn.sum",
  average: "properties.rollup.fn.average",
  median: "properties.rollup.fn.median",
  min: "properties.rollup.fn.min",
  max: "properties.rollup.fn.max",
  range: "properties.rollup.fn.range",
  earliest: "properties.rollup.fn.earliest",
  latest: "properties.rollup.fn.latest",
  date_range: "pageTable.calc.fn.date_range",
  count_checked: "properties.rollup.fn.count_checked",
  count_unchecked: "pageTable.calc.fn.count_unchecked",
  percent_checked: "properties.rollup.fn.percent_checked",
};

/** The rollup function that does the arithmetic of a function, or that its number is formatted as. */
const ROLLUP_OF: Readonly<Record<CalculationFunction, RollupFunction>> = {
  count_all: "count_all",
  count_values: "count_values",
  count_unique: "count_unique",
  count_empty: "count_empty",
  count_not_empty: "count_values",
  percent_empty: "percent_empty",
  percent_not_empty: "percent_not_empty",
  sum: "sum",
  average: "average",
  median: "median",
  min: "min",
  max: "max",
  range: "range",
  earliest: "earliest",
  latest: "latest",
  date_range: "range",
  count_checked: "count_checked",
  count_unchecked: "count_checked",
  percent_checked: "percent_checked",
};

const COUNTS: readonly CalculationFunction[] = [
  "count_all",
  "count_values",
  "count_unique",
  "count_empty",
  "count_not_empty",
  "percent_empty",
  "percent_not_empty",
];
const NUMBERS: readonly CalculationFunction[] = ["sum", "average", "median", "min", "max", "range"];
const DATES: readonly CalculationFunction[] = ["earliest", "latest", "date_range"];
const CHECKBOXES: readonly CalculationFunction[] = ["count_checked", "count_unchecked", "percent_checked"];

/** What a column's values are, as far as the functions go. */
type ColumnKind = "number" | "date" | "time" | "checkbox" | "other";

/** The rollup functions whose result is not a number (a day, a list or a progress). */
const NON_NUMERIC_ROLLUPS: readonly RollupFunction[] = ["earliest", "latest", "show_original", "checklist_progress"];

/** The kind of a column: a rollup is a number or a day when its function makes one, a formula by its result, else nothing in particular. */
function columnKind(def: PropertyDef | null): ColumnKind {
  if (def === null) {
    return "other";
  }
  switch (def.kind) {
    case "number":
    case "date":
    case "checkbox":
      return def.kind;
    case "created_time":
    case "last_edited_time":
      return "time";
    case "rollup": {
      const fn = rollupConfigOf(def)?.function;
      if (fn === undefined) {
        return "other";
      }
      if (fn === "earliest" || fn === "latest") {
        return "date";
      }
      return NON_NUMERIC_ROLLUPS.includes(fn) ? "other" : "number";
    }
    case "formula": {
      const shape = formulaShapeOf(def);
      return shape === "number" || shape === "date" ? shape : shape === "boolean" ? "checkbox" : "other";
    }
    default:
      return "other";
  }
}

/**
 * The functions a column offers, in the order of its menu: the ones of its kind first (a number: sum to
 * range; a date, or the time a page was created or edited: earliest, latest and the range; a checkbox:
 * checked, unchecked and the percent), then the counts, which every kind has. The page (title) column,
 * `def === null`, has the counts only.
 */
export function calculationFunctionsFor(def: PropertyDef | null): CalculationFunction[] {
  switch (columnKind(def)) {
    case "number":
      return [...NUMBERS, ...COUNTS];
    case "date":
    case "time":
      return [...DATES, ...COUNTS];
    case "checkbox":
      return [...CHECKBOXES, ...COUNTS];
    default:
      return [...COUNTS];
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * The calculations an attribute holds, `{ "<property id>" | "title": "<function>" }`. Entries whose
 * function is not a non-empty text are dropped, and anything that is no object reads as none. A function
 * name this version does not know is kept as it is (a newer version may have written it, and writing
 * another column's calculation must not lose it): {@link calculationFor} is what ignores it.
 */
export function readCalculations(value: unknown): Record<string, string> {
  const found: Record<string, string> = {};
  if (!isRecord(value)) {
    return found;
  }
  for (const [key, name] of Object.entries(value)) {
    if (key !== "" && key !== "__proto__" && typeof name === "string" && name !== "") {
      found[key] = name;
    }
  }
  return found;
}

/**
 * The function the footer shows for a column: the stored one when this version knows it and the column's
 * kind offers it, `null` otherwise (no entry, an unknown function, a function of another kind).
 */
export function calculationFor(
  calculations: Readonly<Record<string, string>>,
  key: string,
  def: PropertyDef | null,
): CalculationFunction | null {
  const name = Object.hasOwn(calculations, key) ? calculations[key] : undefined;
  return isCalculationFunction(name) && calculationFunctionsFor(def).includes(name) ? name : null;
}

// ----- the arithmetic -------------------------------------------------------------------------------

/** What a function makes of the rows: a number, a day or an instant as the column shows them, or a count of days. */
export type CalculationResult =
  | { readonly type: "number"; readonly data: number }
  | { readonly type: "date"; readonly data: DateValue }
  | { readonly type: "instant"; readonly data: number }
  | { readonly type: "days"; readonly data: number };

/**
 * The value of a page in a column as the functions see it: `undefined` for no value (also an empty text or
 * list, and a blank title for the page column), a rollup that gives a number or a day as that number or day,
 * a formula's number, day, text or boolean as the value of that kind, and the value itself otherwise.
 */
function cellValue(def: PropertyDef | null, page: PageInfo): PropertyValue | undefined {
  if (def === null) {
    return page.title.trim() === "" ? undefined : { kind: "text", value: page.title };
  }
  const value = valueOfPage(page, def.id);
  if (value === undefined || isEmptyPropertyValue(value)) {
    return undefined;
  }
  if (value.kind === "rollup") {
    const { result } = value.value;
    if (result.type === "number") {
      return { kind: "number", value: result.data };
    }
    if (result.type === "date") {
      return { kind: "date", value: result.data };
    }
  }
  const formula = formulaResultOf(value);
  return formula === undefined ? value : formulaCellValue(formula);
}

/** What a value is made of, for the counts of values: an option, a page, a file or an item each, any other value as a whole. */
function atoms(value: PropertyValue): string[] {
  switch (value.kind) {
    case "multi_select":
    case "relation":
    case "person":
    case "files":
      return value.value.map(String);
    case "checklist":
      return value.value.map((item) => item.text);
    case "formula":
      // A list counts its items (the only formula value left as it is besides a page and a person).
      return value.value.type === "list" ? value.value.data.map((item) => JSON.stringify(item)) : [JSON.stringify(value.value)];
    default:
      return [JSON.stringify(value)];
  }
}

const number = (data: number): CalculationResult => ({ type: "number", data });

/** The whole days from the first to the second instant, counted on the person's local calendar. */
function daysBetweenInstants(first: number, last: number): number {
  return daysBetween(todayIso(new Date(first)), todayIso(new Date(last)));
}

/** What `evaluateRollup` made of the values, as a result (a number or a day: those are the only ones these functions make). */
function fromRollup(result: RollupResult | undefined): CalculationResult | undefined {
  return result?.type === "number" || result?.type === "date" ? result : undefined;
}

/**
 * What `fn` makes of `rows` for the column `def` (`null` is the page column). `rows` are the rows the view
 * shows: the pages of its source, filtered. `undefined` is a function with nothing to say (the average of no
 * numbers, a percent of no rows, the earliest of no dates).
 */
export function calculate(fn: CalculationFunction, def: PropertyDef | null, rows: readonly PageInfo[]): CalculationResult | undefined {
  const values = rows.map((page) => cellValue(def, page));
  const present = values.filter((value): value is PropertyValue => value !== undefined);
  switch (fn) {
    case "count_values":
      return number(present.reduce((total, value) => total + atoms(value).length, 0));
    case "count_unique":
      return number(new Set(present.flatMap(atoms)).size);
    case "count_unchecked":
      return number(rows.length - present.filter((value) => value.kind === "checkbox" && value.value).length);
    case "earliest":
    case "latest":
    case "date_range": {
      if (def?.kind === "created_time" || def?.kind === "last_edited_time") {
        const instants = present.flatMap((value) => (value.kind === "created_time" || value.kind === "last_edited_time" ? [value.value] : []));
        if (instants.length === 0) {
          return undefined;
        }
        const first = Math.min(...instants);
        const last = Math.max(...instants);
        if (fn === "date_range") {
          return { type: "days", data: daysBetweenInstants(first, last) };
        }
        return { type: "instant", data: fn === "earliest" ? first : last };
      }
      if (fn === "date_range") {
        const first = fromRollup(evaluateRollup("earliest", values));
        const last = fromRollup(evaluateRollup("latest", values));
        return first?.type === "date" && last?.type === "date"
          ? { type: "days", data: daysBetween(first.data.date, last.data.date) }
          : undefined;
      }
      return fromRollup(evaluateRollup(fn, values));
    }
    default:
      return fromRollup(evaluateRollup(ROLLUP_OF[fn], values));
  }
}

/** The result as the footer writes it: a number or a day as a rollup does, an instant as a created time is, a range in days. */
export function formatCalculation(fn: CalculationFunction, result: CalculationResult, today: string): string {
  const formatDay = (date: string, time: string | null): string => formatDateMention(date, time, today);
  switch (result.type) {
    case "number":
    case "date":
      return rollupTexts(ROLLUP_OF[fn], result, undefined, formatDay).join(", ");
    case "instant":
      return formatInstant(result.data);
    case "days":
      return tp("pageTable.calc.days", result.data, { count: formatNumber(result.data) });
  }
}
