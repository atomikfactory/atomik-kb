// The arithmetic of rollups (ADR-0027), ported from the tests of `kb-model`'s `properties/rollup.rs`
// and the rules of `kb-types`' `RollupFunction::accepts`.

import { describe, expect, it } from "vitest";

import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyValue } from "./generated/PropertyValue";
import type { RollupFunction } from "./generated/RollupFunction";
import {
  ROLLUP_FUNCTIONS,
  evaluateRollup,
  isRollupTargetKind,
  rollupFunctionAccepts,
  rollupFunctionsFor,
} from "./rollup";

const n = (value: number): PropertyValue => ({ kind: "number", value });
const day = (date: string, time?: string): PropertyValue => ({
  kind: "date",
  value: time === undefined ? { date } : { date, time },
});

function numberOf(fn: RollupFunction, values: (PropertyValue | undefined)[]): number | undefined {
  const result = evaluateRollup(fn, values);
  if (result === undefined) {
    return undefined;
  }
  expect(result.type).toBe("number");
  return result.type === "number" ? result.data : undefined;
}

describe("the functions a target accepts", () => {
  it("lists every function once and offers each target kind only what fits", () => {
    expect(new Set(ROLLUP_FUNCTIONS).size).toBe(18);
    const kinds: PropertyKind[] = ["text", "number", "date", "checkbox", "checklist", "select", "multi_select", "place"];
    const accepted = (kind: PropertyKind | undefined) => rollupFunctionsFor(kind);
    const counting = ["count_all", "count_values", "count_unique", "count_empty", "percent_empty", "percent_not_empty", "show_original"];
    expect(accepted(undefined)).toEqual(counting);
    expect(accepted("text")).toEqual(counting);
    expect(accepted("select")).toEqual(counting);
    expect(accepted("number")).toEqual([...counting.slice(0, 6), "sum", "average", "median", "min", "max", "range", "show_original"].sort(order));
    expect(accepted("date")).toEqual(
      ["count_all", "count_values", "count_unique", "count_empty", "percent_empty", "percent_not_empty", "earliest", "latest", "show_original"].sort(order),
    );
    expect(accepted("checkbox")).toContain("percent_checked");
    expect(accepted("checkbox")).toContain("count_checked");
    expect(accepted("checklist")).toContain("checklist_progress");
    for (const kind of kinds) {
      expect(isRollupTargetKind(kind)).toBe(true);
    }
    for (const kind of ["files", "relation", "rollup", "created_time", "last_edited_time", "created_by", "last_edited_by"] as const) {
      expect(isRollupTargetKind(kind)).toBe(false);
      expect(rollupFunctionAccepts("count_all", kind)).toBe(false);
    }
    expect(rollupFunctionAccepts("sum", "date")).toBe(false);
    expect(rollupFunctionAccepts("sum", undefined)).toBe(false);
  });
});

/** Orders function names as {@link ROLLUP_FUNCTIONS} does. */
function order(a: string, b: string): number {
  return ROLLUP_FUNCTIONS.indexOf(a as RollupFunction) - ROLLUP_FUNCTIONS.indexOf(b as RollupFunction);
}

describe("evaluateRollup", () => {
  it("counts include empty values and zero pages count zero", () => {
    const values = [n(1), undefined, n(1), n(2)];
    expect(numberOf("count_all", values)).toBe(4);
    expect(numberOf("count_values", values)).toBe(3);
    expect(numberOf("count_empty", values)).toBe(1);
    expect(numberOf("count_unique", values)).toBe(2);
    expect(numberOf("percent_empty", values)).toBe(25);
    expect(numberOf("percent_not_empty", values)).toBe(75);
    for (const fn of ["count_all", "count_values", "count_empty", "count_unique", "count_checked", "sum"] as const) {
      expect(numberOf(fn, [])).toBe(0);
    }
    for (const fn of [
      "percent_empty",
      "percent_not_empty",
      "percent_checked",
      "average",
      "median",
      "min",
      "max",
      "range",
      "earliest",
      "latest",
      "show_original",
      "checklist_progress",
    ] as const) {
      expect(evaluateRollup(fn, [])).toBeUndefined();
    }
  });

  it("summarises numbers and skips the pages without one", () => {
    const values = [n(4), undefined, n(1), n(10), n(5)];
    expect(numberOf("sum", values)).toBe(20);
    expect(numberOf("average", values)).toBe(5);
    expect(numberOf("median", values)).toBe(4.5);
    expect(numberOf("min", values)).toBe(1);
    expect(numberOf("max", values)).toBe(10);
    expect(numberOf("range", values)).toBe(9);
    expect(numberOf("median", [n(3), n(1), n(2)])).toBe(2);
    expect(numberOf("sum", [undefined, undefined])).toBe(0);
    expect(evaluateRollup("average", [undefined, undefined])).toBeUndefined();
    expect(numberOf("percent_not_empty", [n(1), undefined, undefined])).toBe(100 / 3);
  });

  it("sorts dates by day, then time", () => {
    const values = [day("2026-10-09"), day("2026-10-07", "14:30"), undefined, day("2026-10-07")];
    expect(evaluateRollup("earliest", values)).toEqual({ type: "date", data: { date: "2026-10-07" } });
    expect(evaluateRollup("latest", values)).toEqual({ type: "date", data: { date: "2026-10-09" } });
  });

  it("counts unset checkboxes as unchecked", () => {
    const values: (PropertyValue | undefined)[] = [
      { kind: "checkbox", value: true },
      { kind: "checkbox", value: false },
      undefined,
      { kind: "checkbox", value: true },
    ];
    expect(numberOf("count_checked", values)).toBe(2);
    expect(numberOf("percent_checked", values)).toBe(50);
    expect(numberOf("count_values", values)).toBe(3);
  });

  it("splits multi-selects into their options for unique counts, lists values and sums checklists", () => {
    const multi = (...ids: string[]): PropertyValue => ({ kind: "multi_select", value: ids });
    expect(numberOf("count_unique", [multi("a", "b"), multi("b", "c"), undefined])).toBe(3);
    const text = (value: string): PropertyValue => ({ kind: "text", value });
    expect(numberOf("count_unique", [text("a"), text("b"), text("a")])).toBe(2);
    expect(evaluateRollup("show_original", [text("b"), undefined, text("a")])).toEqual({
      type: "values",
      data: [text("b"), text("a")],
    });
    const list = (...done: boolean[]): PropertyValue => ({
      kind: "checklist",
      value: done.map((flag, at) => ({ id: String(at), text: "", done: flag })),
    });
    expect(evaluateRollup("checklist_progress", [list(true, false), undefined, list(true)])).toEqual({
      type: "progress",
      data: { done: 2, total: 3 },
    });
  });
});
