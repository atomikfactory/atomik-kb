// The arithmetic and the rules of rollups (ADR-0027), mirrored from `kb-types`' `RollupFunction`
// (`accepts`) and `kb-model`'s `properties/rollup.rs` exactly, so that the in-memory core
// (`MemoryProperties`) computes what the real one does and a form offers only the functions a
// target accepts. Pure and dependency-free apart from the generated types.

import type { DateValue } from "./generated/DateValue";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyValue } from "./generated/PropertyValue";
import type { RollupFunction } from "./generated/RollupFunction";
import type { RollupResult } from "./generated/RollupResult";

/** The text a rollup's target has when it is the title of the related pages (`ROLLUP_TITLE_TARGET`). */
export const ROLLUP_TITLE_TARGET = "title";

/** Every function, in the order the UI lists them (`RollupFunction::ALL`). */
export const ROLLUP_FUNCTIONS: readonly RollupFunction[] = [
  "count_all",
  "count_values",
  "count_unique",
  "count_empty",
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
  "percent_checked",
  "count_checked",
  "show_original",
  "checklist_progress",
];

/** The kinds a rollup may summarise: not files, relations, rollups or the computed kinds (`PropertyKind::is_rollup_target`). */
const ROLLUP_TARGET_KINDS: readonly PropertyKind[] = [
  "text",
  "number",
  "select",
  "multi_select",
  "date",
  "checkbox",
  "url",
  "status",
  "email",
  "phone",
  "checklist",
  "place",
];

/** Whether a rollup may summarise a property of this kind. */
export function isRollupTargetKind(kind: PropertyKind): boolean {
  return ROLLUP_TARGET_KINDS.includes(kind);
}

/**
 * Whether `fn` can summarise a target of this kind (`undefined` is the title of the related pages):
 * the counting functions and `show_original` take any target, the number functions a `number`,
 * `earliest` and `latest` a `date`, the checked functions a `checkbox`, `checklist_progress` a
 * `checklist` (`RollupFunction::accepts`).
 */
export function rollupFunctionAccepts(fn: RollupFunction, target: PropertyKind | undefined): boolean {
  if (target !== undefined && !isRollupTargetKind(target)) {
    return false;
  }
  switch (fn) {
    case "count_all":
    case "count_values":
    case "count_unique":
    case "count_empty":
    case "percent_empty":
    case "percent_not_empty":
    case "show_original":
      return true;
    case "sum":
    case "average":
    case "median":
    case "min":
    case "max":
    case "range":
      return target === "number";
    case "earliest":
    case "latest":
      return target === "date";
    case "percent_checked":
    case "count_checked":
      return target === "checkbox";
    case "checklist_progress":
      return target === "checklist";
    default:
      return false;
  }
}

/** The functions a target accepts, in the order of {@link ROLLUP_FUNCTIONS}. */
export function rollupFunctionsFor(target: PropertyKind | undefined): RollupFunction[] {
  return ROLLUP_FUNCTIONS.filter((fn) => rollupFunctionAccepts(fn, target));
}

function numbers(values: readonly PropertyValue[]): number[] {
  const found: number[] = [];
  for (const value of values) {
    if (value.kind === "number") {
      found.push(value.value);
    }
  }
  return found;
}

function dates(values: readonly PropertyValue[]): DateValue[] {
  const found: DateValue[] = [];
  for (const value of values) {
    if (value.kind === "date") {
      found.push(value.value);
    }
  }
  return found;
}

/** A date sorts by day, then time; a day without a time comes before the same day with one. */
function compareDates(a: DateValue, b: DateValue): number {
  const day = a.date < b.date ? -1 : a.date > b.date ? 1 : 0;
  if (day !== 0) {
    return day;
  }
  const at = a.time ?? "";
  const bt = b.time ?? "";
  return at < bt ? -1 : at > bt ? 1 : 0;
}

function sum(all: readonly number[]): number {
  let total = 0;
  for (const n of all) {
    total += n;
  }
  return total;
}

function checked(values: readonly PropertyValue[]): number {
  return values.filter((value) => value.kind === "checkbox" && value.value).length;
}

/** Two values are the same for `count_unique` (a multi-select counts each option on its own). */
function uniqueKeys(value: PropertyValue): string[] {
  if (value.kind === "multi_select") {
    return value.value.map((option) => `option:${option}`);
  }
  return [JSON.stringify(value)];
}

/**
 * Evaluates `fn` over the target values of the related pages: one entry per related page (live and
 * existing), `undefined` for a page that has no value for the target (for the title: a blank one).
 * Counts of nothing are 0, a percent of nothing and a summary of no numbers or dates have no
 * result (`undefined`). (`rollup::evaluate`.)
 */
export function evaluateRollup(
  fn: RollupFunction,
  values: readonly (PropertyValue | undefined)[],
): RollupResult | undefined {
  const total = values.length;
  const present = values.filter((value): value is PropertyValue => value !== undefined);
  const withValue = present.length;
  const number = (data: number): RollupResult => ({ type: "number", data });
  const percent = (part: number): RollupResult | undefined => (total > 0 ? number((part * 100) / total) : undefined);
  switch (fn) {
    case "count_all":
      return number(total);
    case "count_values":
      return number(withValue);
    case "count_empty":
      return number(total - withValue);
    case "percent_empty":
      return percent(total - withValue);
    case "percent_not_empty":
      return percent(withValue);
    case "count_unique": {
      const seen = new Set<string>();
      for (const value of present) {
        for (const key of uniqueKeys(value)) {
          seen.add(key);
        }
      }
      return number(seen.size);
    }
    case "sum":
      return number(sum(numbers(present)));
    case "average": {
      const all = numbers(present);
      return all.length > 0 ? number(sum(all) / all.length) : undefined;
    }
    case "median": {
      const all = numbers(present).sort((a, b) => a - b);
      if (all.length === 0) {
        return undefined;
      }
      const middle = Math.floor(all.length / 2);
      const high = all[middle] ?? 0;
      const low = all[middle - 1] ?? 0;
      return number(all.length % 2 === 0 ? (low + high) / 2 : high);
    }
    case "min": {
      const all = numbers(present);
      return all.length > 0 ? number(Math.min(...all)) : undefined;
    }
    case "max": {
      const all = numbers(present);
      return all.length > 0 ? number(Math.max(...all)) : undefined;
    }
    case "range": {
      const all = numbers(present);
      return all.length > 0 ? number(Math.max(...all) - Math.min(...all)) : undefined;
    }
    case "earliest":
    case "latest": {
      const all = dates(present).sort(compareDates);
      const pick = fn === "earliest" ? all[0] : all[all.length - 1];
      if (pick === undefined) {
        return undefined;
      }
      const copy: DateValue = { date: pick.date };
      if (pick.time !== undefined) {
        copy.time = pick.time;
      }
      return { type: "date", data: copy };
    }
    case "count_checked":
      return number(checked(present));
    case "percent_checked":
      return percent(checked(present));
    case "show_original":
      return present.length > 0 ? { type: "values", data: present } : undefined;
    case "checklist_progress": {
      let done = 0;
      let items = 0;
      for (const value of present) {
        if (value.kind === "checklist") {
          items += value.value.length;
          done += value.value.filter((item) => item.done).length;
        }
      }
      return items > 0 ? { type: "progress", data: { done, total: items } } : undefined;
    }
    default:
      return undefined;
  }
}
