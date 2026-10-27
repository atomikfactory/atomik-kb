// The calculations of the table's footer (ADR-0040 item 5, ADR-0016 amendment of 2026-10-14): which
// functions a column offers, how the attribute is read, what each family of functions makes of the rows
// (with the same arithmetic and formatting as a rollup), and that only the rows given count.

import { evaluateRollup } from "@atomik-kb/core-client";
import type { PageInfo, PropertyDef, PropertyKind, PropertyValue, RollupFunction } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { t } from "../i18n";
import { formatInstant, formatRollupNumber } from "../property-format";
import { formatDateMention } from "./dates";
import {
  calculate,
  CALCULATION_FUNCTIONS,
  CALCULATION_LABELS,
  calculationFor,
  calculationFunctionsFor,
  formatCalculation,
  isCalculationFunction,
  readCalculations,
} from "./page-table-calculations";
import type { CalculationFunction } from "./page-table-calculations";

const COUNTS: CalculationFunction[] = ["count_all", "count_values", "count_unique", "count_empty", "count_not_empty", "percent_empty", "percent_not_empty"];

function def(kind: PropertyKind, extra: Partial<PropertyDef> = {}): PropertyDef {
  return { id: `p-${kind}`, name: kind, kind, options: [], createdAt: 0, ...extra };
}

function rollupDef(fn: RollupFunction): PropertyDef {
  return def("rollup", { id: `p-rollup-${fn}`, config: { kind: "rollup", relation: "r", target: "t", function: fn } as never });
}

/** A page with the given values (by property id) and title. */
function page(title: string, values: Record<string, PropertyValue> = {}): PageInfo {
  return { id: title, spaceId: "s", title, createdAt: 0, updatedAt: 0, properties: Object.entries(values) };
}

const text = (value: string): PropertyValue => ({ kind: "text", value });
const num = (value: number): PropertyValue => ({ kind: "number", value });
const day = (date: string, time?: string): PropertyValue => ({ kind: "date", value: time === undefined ? { date } : { date, time } });
const box = (value: boolean): PropertyValue => ({ kind: "checkbox", value });
const many = (...value: string[]): PropertyValue => ({ kind: "multi_select", value });

/** The number a result holds (fails when it is not one). */
function numberOf(fn: CalculationFunction, column: PropertyDef | null, rows: readonly PageInfo[]): number | undefined {
  const result = calculate(fn, column, rows);
  if (result === undefined) {
    return undefined;
  }
  expect(result.type, fn).toBe("number");
  return result.data as number;
}

describe("the functions a column offers", () => {
  it("are the counts for the page column and for every kind that has nothing of its own", () => {
    expect(calculationFunctionsFor(null)).toEqual(COUNTS);
    for (const kind of ["text", "select", "multi_select", "status", "url", "email", "phone", "files", "checklist", "place", "relation", "created_by", "last_edited_by"] as const) {
      expect(calculationFunctionsFor(def(kind)), kind).toEqual(COUNTS);
    }
  });

  it("add sum, average, median, minimum, maximum and range for a number", () => {
    expect(calculationFunctionsFor(def("number"))).toEqual(["sum", "average", "median", "min", "max", "range", ...COUNTS]);
  });

  it("add the earliest, the latest and the range for a date, and for the time a page was created or edited", () => {
    const dates = ["earliest", "latest", "date_range", ...COUNTS];
    expect(calculationFunctionsFor(def("date"))).toEqual(dates);
    expect(calculationFunctionsFor(def("created_time"))).toEqual(dates);
    expect(calculationFunctionsFor(def("last_edited_time"))).toEqual(dates);
  });

  it("add checked, unchecked and the percent checked for a checkbox", () => {
    expect(calculationFunctionsFor(def("checkbox"))).toEqual(["count_checked", "count_unchecked", "percent_checked", ...COUNTS]);
  });

  it("follow what a rollup makes: numbers for a number, dates for a day, only the counts for a list or a progress", () => {
    expect(calculationFunctionsFor(rollupDef("sum"))).toEqual(["sum", "average", "median", "min", "max", "range", ...COUNTS]);
    expect(calculationFunctionsFor(rollupDef("count_all"))).toEqual(["sum", "average", "median", "min", "max", "range", ...COUNTS]);
    expect(calculationFunctionsFor(rollupDef("percent_checked"))).toEqual(["sum", "average", "median", "min", "max", "range", ...COUNTS]);
    expect(calculationFunctionsFor(rollupDef("earliest"))).toEqual(["earliest", "latest", "date_range", ...COUNTS]);
    expect(calculationFunctionsFor(rollupDef("latest"))).toEqual(["earliest", "latest", "date_range", ...COUNTS]);
    expect(calculationFunctionsFor(rollupDef("show_original"))).toEqual(COUNTS);
    expect(calculationFunctionsFor(rollupDef("checklist_progress"))).toEqual(COUNTS);
    expect(calculationFunctionsFor(def("rollup"))).toEqual(COUNTS); // no configuration
  });

  it("list no function twice, only known ones, and every one has a label", () => {
    for (const column of [null, def("number"), def("date"), def("checkbox"), def("created_time"), def("text")]) {
      const list = calculationFunctionsFor(column);
      expect(new Set(list).size).toBe(list.length);
      for (const fn of list) {
        expect(CALCULATION_FUNCTIONS).toContain(fn);
        expect(t(CALCULATION_LABELS[fn]).length).toBeGreaterThan(0);
      }
    }
    expect(new Set(CALCULATION_FUNCTIONS.map((fn) => CALCULATION_LABELS[fn])).size).toBe(CALCULATION_FUNCTIONS.length);
  });
});

describe("reading the attribute", () => {
  it("keeps the entries whose function is a text, and reads anything that is no object as none", () => {
    expect(readCalculations({ title: "count_all", a: "sum", b: "future_function", c: 3, d: null, e: "", "": "sum", f: ["sum"] })).toEqual({
      title: "count_all",
      a: "sum",
      b: "future_function",
    });
    for (const odd of [null, undefined, [], "sum", 3, true]) {
      expect(readCalculations(odd), JSON.stringify(odd)).toEqual({});
    }
    expect(Object.keys(readCalculations(JSON.parse('{"__proto__": "sum", "a": "sum"}')))).toEqual(["a"]);
  });

  it("knows the functions of this version", () => {
    expect(CALCULATION_FUNCTIONS.every(isCalculationFunction)).toBe(true);
    for (const odd of ["", "total", "SUM", null, undefined, 3, {}]) {
      expect(isCalculationFunction(odd), JSON.stringify(odd)).toBe(false);
    }
  });

  it("shows only a function of this version that the column's kind offers, and ignores the rest", () => {
    const stored = { title: "count_all", a: "sum", b: "sum", c: "future_function", d: "earliest" };
    expect(calculationFor(stored, "title", null)).toBe("count_all");
    expect(calculationFor(stored, "a", def("number"))).toBe("sum");
    expect(calculationFor(stored, "b", def("text"))).toBeNull(); // a sum of text
    expect(calculationFor(stored, "c", def("number"))).toBeNull(); // a function of a newer version
    expect(calculationFor(stored, "d", def("date"))).toBe("earliest");
    expect(calculationFor(stored, "missing", def("number"))).toBeNull();
    expect(calculationFor({}, "constructor", def("number"))).toBeNull();
    expect(calculationFor({ title: "sum" }, "title", null)).toBeNull(); // the page column has no sum
  });
});

describe("the counts", () => {
  const note = def("text");
  const rows = [
    page("Alpha", { [note.id]: text("x") }),
    page("beta", { [note.id]: text("y") }),
    page("Gamma"),
    page("delta", { [note.id]: text("x") }),
    page("eps", { [note.id]: text("") }),
  ];

  it("count the rows, the values, the unique values and the empty cells of a text column", () => {
    expect(numberOf("count_all", note, rows)).toBe(5);
    expect(numberOf("count_values", note, rows)).toBe(3);
    expect(numberOf("count_not_empty", note, rows)).toBe(3);
    expect(numberOf("count_unique", note, rows)).toBe(2);
    expect(numberOf("count_empty", note, rows)).toBe(2); // no value, and the empty text
  });

  it("give the share of the rows that are empty or not, as a percent", () => {
    expect(numberOf("percent_empty", note, rows)).toBe(40);
    expect(numberOf("percent_not_empty", note, rows)).toBe(60);
  });

  it("count a value once for a single-valued kind, and each element of a list for the values (not empty counts the rows)", () => {
    const areas = def("multi_select");
    const kinds = [
      page("a", { [areas.id]: many("home", "work") }),
      page("b", { [areas.id]: many("work") }),
      page("c", { [areas.id]: many() }),
      page("d"),
    ];
    expect(numberOf("count_values", areas, kinds)).toBe(3);
    expect(numberOf("count_not_empty", areas, kinds)).toBe(2);
    expect(numberOf("count_unique", areas, kinds)).toBe(2);
    expect(numberOf("count_empty", areas, kinds)).toBe(2);

    const tasks = def("relation");
    const related = [
      page("a", { [tasks.id]: { kind: "relation", value: ["x", "y"] } }),
      page("b", { [tasks.id]: { kind: "relation", value: ["y"] } }),
    ];
    expect(numberOf("count_values", tasks, related)).toBe(3);
    expect(numberOf("count_unique", tasks, related)).toBe(2);

    const files = def("files");
    expect(numberOf("count_values", files, [page("a", { [files.id]: { kind: "files", value: ["f1", "f2"] } }), page("b", { [files.id]: { kind: "files", value: ["f2"] } })])).toBe(3);

    const todo = def("checklist");
    const item = (text: string, done: boolean) => ({ id: `${text}-${String(done)}`, text, done });
    const lists = [
      page("a", { [todo.id]: { kind: "checklist", value: [item("one", true), item("two", false)] } }),
      page("b", { [todo.id]: { kind: "checklist", value: [item("one", false)] } }),
    ];
    expect(numberOf("count_values", todo, lists)).toBe(3);
    expect(numberOf("count_unique", todo, lists)).toBe(2);
  });

  it("tell a number of zero and a box that is not checked from no value: they are values", () => {
    const n = def("number");
    const c = def("checkbox");
    expect(numberOf("count_not_empty", n, [page("a", { [n.id]: num(0) }), page("b")])).toBe(1);
    expect(numberOf("count_not_empty", c, [page("a", { [c.id]: box(false) }), page("b")])).toBe(1);
  });

  it("count the page column: a blank title is empty, the same title twice is one", () => {
    const titles = [page("Alpha"), page("alpha"), page("Alpha"), page("  "), page("")];
    expect(numberOf("count_all", null, titles)).toBe(5);
    expect(numberOf("count_values", null, titles)).toBe(3);
    expect(numberOf("count_unique", null, titles)).toBe(2);
    expect(numberOf("count_empty", null, titles)).toBe(2);
    expect(numberOf("percent_empty", null, titles)).toBe(40);
  });

  it("count no rows as zero, and have no percent of nothing", () => {
    for (const fn of ["count_all", "count_values", "count_unique", "count_empty", "count_not_empty"] as const) {
      expect(numberOf(fn, note, []), fn).toBe(0);
    }
    expect(calculate("percent_empty", note, [])).toBeUndefined();
    expect(calculate("percent_not_empty", note, [])).toBeUndefined();
  });

  it("see the rows given and no others (the view passes the ones its filters keep)", () => {
    expect(numberOf("count_all", note, rows.slice(0, 2))).toBe(2);
    expect(numberOf("count_values", note, rows.slice(2))).toBe(1);
  });
});

describe("the numbers", () => {
  const cost = def("number");
  const rows = (...values: (number | undefined)[]): PageInfo[] =>
    values.map((value, at) => page(`r${String(at)}`, value === undefined ? {} : { [cost.id]: num(value) }));

  it("sum, average, median, minimum, maximum and range the numbers, leaving out the rows without one", () => {
    const some = rows(3, 10, undefined, 2, 0);
    expect(numberOf("sum", cost, some)).toBe(15);
    expect(numberOf("average", cost, some)).toBe(3.75);
    expect(numberOf("median", cost, some)).toBe(2.5);
    expect(numberOf("min", cost, some)).toBe(0);
    expect(numberOf("max", cost, some)).toBe(10);
    expect(numberOf("range", cost, some)).toBe(10);
    expect(numberOf("median", cost, rows(5, 1, 3))).toBe(3);
  });

  it("have nothing to say of no numbers, except a sum of zero", () => {
    for (const none of [[], rows(undefined, undefined)]) {
      expect(numberOf("sum", cost, none)).toBe(0);
      for (const fn of ["average", "median", "min", "max", "range"] as const) {
        expect(calculate(fn, cost, none), fn).toBeUndefined();
      }
    }
  });

  it("do the same arithmetic as the rollup functions, for every function the two share", () => {
    const column = def("number");
    const some = rows(7, 1.5, undefined, 4, 4);
    const values = some.map((entry) => entry.properties?.[0]?.[1]);
    for (const fn of ["sum", "average", "median", "min", "max", "range", "count_all", "count_empty", "count_unique", "percent_empty", "percent_not_empty"] as const) {
      expect(calculate(fn, column, some), fn).toEqual(evaluateRollup(fn, values));
    }
    expect(calculate("count_not_empty", column, some)).toEqual(evaluateRollup("count_values", values));
  });

  it("read a rollup that gives a number as that number", () => {
    const total = rollupDef("sum");
    const rollup = (value: number): PropertyValue => ({ kind: "rollup", value: { function: "sum", result: { type: "number", data: value } } });
    const some = [page("a", { [total.id]: rollup(7.5) }), page("b", { [total.id]: rollup(2) }), page("c")];
    expect(numberOf("sum", total, some)).toBe(9.5);
    expect(numberOf("max", total, some)).toBe(7.5);
    expect(numberOf("count_not_empty", total, some)).toBe(2);
  });
});

describe("the dates", () => {
  const due = def("date");
  const rows = [
    page("a", { [due.id]: day("2026-10-15") }),
    page("b", { [due.id]: day("2026-10-09", "09:30") }),
    page("c"),
    page("d", { [due.id]: day("2026-10-04") }),
    page("e", { [due.id]: day("2026-10-15", "08:00") }),
  ];

  it("find the earliest and the latest, a day with a time after the same day without, as a rollup does", () => {
    expect(calculate("earliest", due, rows)).toEqual({ type: "date", data: { date: "2026-10-04" } });
    expect(calculate("latest", due, rows)).toEqual({ type: "date", data: { date: "2026-10-15", time: "08:00" } });
    const values = rows.map((entry) => entry.properties?.[0]?.[1]);
    expect(calculate("earliest", due, rows)).toEqual(evaluateRollup("earliest", values));
    expect(calculate("latest", due, rows)).toEqual(evaluateRollup("latest", values));
  });

  it("count the days from the earliest to the latest (the time of day does not count)", () => {
    expect(calculate("date_range", due, rows)).toEqual({ type: "days", data: 11 });
    expect(calculate("date_range", due, [rows[0] as PageInfo])).toEqual({ type: "days", data: 0 });
    expect(calculate("date_range", due, [rows[0] as PageInfo, rows[4] as PageInfo])).toEqual({ type: "days", data: 0 });
  });

  it("have nothing to say of no dates", () => {
    for (const fn of ["earliest", "latest", "date_range"] as const) {
      expect(calculate(fn, due, []), fn).toBeUndefined();
      expect(calculate(fn, due, [rows[2] as PageInfo]), fn).toBeUndefined();
    }
  });

  it("read a rollup that gives a day as that day", () => {
    const first = rollupDef("earliest");
    const rollup = (date: string): PropertyValue => ({ kind: "rollup", value: { function: "earliest", result: { type: "date", data: { date } } } });
    const some = [page("a", { [first.id]: rollup("2026-10-20") }), page("b", { [first.id]: rollup("2026-10-02") })];
    expect(calculate("earliest", first, some)).toEqual({ type: "date", data: { date: "2026-10-02" } });
    expect(calculate("date_range", first, some)).toEqual({ type: "days", data: 18 });
  });

  it("work on the time a page was created or edited: the earliest and latest instants, the range in local days", () => {
    const created = def("created_time");
    const at = (month: number, dayOfMonth: number, hours: number): number => new Date(2026, month - 1, dayOfMonth, hours, 0).getTime();
    const made = (instant: number | undefined, name: string): PageInfo => page(name, instant === undefined ? {} : { [created.id]: { kind: "created_time", value: instant } });
    const some = [made(at(10, 9, 23), "a"), made(at(10, 4, 9), "b"), made(undefined, "c"), made(at(10, 12, 1), "d")];
    expect(calculate("earliest", created, some)).toEqual({ type: "instant", data: at(10, 4, 9) });
    expect(calculate("latest", created, some)).toEqual({ type: "instant", data: at(10, 12, 1) });
    expect(calculate("date_range", created, some)).toEqual({ type: "days", data: 8 });
    // Two moments of one local day are no days apart, however close to midnight.
    expect(calculate("date_range", created, [made(at(10, 4, 0), "x"), made(at(10, 4, 23), "y")])).toEqual({ type: "days", data: 0 });
    expect(calculate("earliest", created, [made(undefined, "z")])).toBeUndefined();
    expect(numberOf("count_not_empty", created, some)).toBe(3);
  });
});

describe("the checkboxes", () => {
  const done = def("checkbox");
  const rows = [page("a", { [done.id]: box(true) }), page("b", { [done.id]: box(false) }), page("c"), page("d", { [done.id]: box(true) })];

  it("count the checked ones, the unchecked ones (a box with no value is not checked) and the percent checked", () => {
    expect(numberOf("count_checked", done, rows)).toBe(2);
    expect(numberOf("count_unchecked", done, rows)).toBe(2);
    expect(numberOf("percent_checked", done, rows)).toBe(50);
    expect(numberOf("count_unchecked", done, [])).toBe(0);
    expect(calculate("percent_checked", done, [])).toBeUndefined();
  });

  it("do what the rollup functions do for the checked ones", () => {
    const values = rows.map((entry) => entry.properties?.[0]?.[1]);
    expect(calculate("count_checked", done, rows)).toEqual(evaluateRollup("count_checked", values));
    expect(calculate("percent_checked", done, rows)).toEqual(evaluateRollup("percent_checked", values));
  });
});

describe("the text of a result", () => {
  const today = "2026-10-14";

  it("writes a number and a day as a rollup does (the percent with its sign, at most two decimals)", () => {
    expect(formatCalculation("sum", { type: "number", data: 1234.5678 }, today)).toBe(formatRollupNumber(1234.5678, "sum"));
    expect(formatCalculation("percent_empty", { type: "number", data: 33.333 }, today)).toBe(formatRollupNumber(33.333, "percent_empty"));
    expect(formatCalculation("percent_empty", { type: "number", data: 33.333 }, today)).toContain("%");
    expect(formatCalculation("count_not_empty", { type: "number", data: 3 }, today)).toBe("3");
    expect(formatCalculation("earliest", { type: "date", data: { date: "2026-10-14" } }, today)).toBe(formatDateMention("2026-10-14", null, today));
    expect(formatCalculation("latest", { type: "date", data: { date: "2026-10-30", time: "08:00" } }, today)).toBe(formatDateMention("2026-10-30", "08:00", today));
  });

  it("writes an instant as the cell of a created time does, and a range in days", () => {
    const instant = new Date(2026, 9, 9, 9, 30).getTime();
    expect(formatCalculation("earliest", { type: "instant", data: instant }, today)).toBe(formatInstant(instant));
    expect(formatCalculation("date_range", { type: "days", data: 1 }, today)).toBe("1 day");
    expect(formatCalculation("date_range", { type: "days", data: 11 }, today)).toBe("11 days");
    expect(formatCalculation("date_range", { type: "days", data: 0 }, today)).toBe("0 days");
  });
});
