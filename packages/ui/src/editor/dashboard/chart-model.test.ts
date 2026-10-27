// The data of a chart widget (ADR-0055, `docs/architecture/dashboards.md` section 4): reading and writing the spec, what can group and
// what can be measured, every kind of group (options, checkbox, person, relation, text, date buckets with their empty ones, formulas,
// rollups), the measures, the order, the caps and "Other", the cumulative line and the titles. Pure: no React, fixtures built by hand.

import type { PageInfo, PropertyDef, PropertyKind, PropertyValue } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { t } from "../../i18n";
import {
  CHART_MAX_BARS,
  CHART_MAX_BUCKETS,
  EMPTY_GROUP_KEY,
  OTHER_GROUP_KEY,
  chartData,
  chartSpecToJson,
  chartTitle,
  defaultChartSpec,
  groupableProperties,
  isDateSource,
  measurableProperties,
  readChartSpec,
  withGroupBy,
  withMeasureFunction,
  withMeasureProperty,
} from "./chart-model";
import type { ChartContext } from "./chart-model";
import type { ChartSpec } from "./dashboard-types";

const SPACE = "11111111-1111-4111-8111-111111111111";

// ----- fixtures ------------------------------------------------------------------------------------

function def(id: string, name: string, kind: PropertyKind, extra: Partial<PropertyDef> = {}): PropertyDef {
  return { id, name, kind, options: [], createdAt: 1, ...extra };
}

const STATUS = def("p-status", "Status", "status", {
  options: [
    { id: "todo", name: "To do", color: "gray", group: "todo" },
    { id: "doing", name: "Doing", color: "blue", group: "in_progress" },
    { id: "done", name: "Done", color: "green", group: "done" },
  ],
});
const PRIORITY = def("p-prio", "Priority", "select", {
  options: [
    { id: "hi", name: "High", color: "red" },
    { id: "lo", name: "Low", color: "purple" },
  ],
});
const TAGS = def("p-tags", "Tags", "multi_select", {
  options: [
    { id: "a", name: "Alpha", color: "orange" },
    { id: "b", name: "Beta", color: "pink" },
  ],
});
const DONE = def("p-done", "Finished", "checkbox");
const OWNER = def("p-owner", "Owner", "person");
const PROJECT = def("p-project", "Project", "relation", { config: { kind: "relation", twoWay: false } });
const NOTE = def("p-note", "Note", "text");
const SITE = def("p-site", "Site", "place");
const DUE = def("p-due", "Due", "date");
const HOURS = def("p-hours", "Hours", "number");
const MADE = def("p-made", "Made", "created_time");
const EDITED = def("p-edited", "Edited", "last_edited_time");
const FILES = def("p-files", "Files", "files");
const formula = (id: string, name: string, base: "number" | "text" | "boolean" | "date" | "page" | "person", list = false): PropertyDef =>
  def(id, name, "formula", { config: { kind: "formula", expression: "", resultType: { base, list } } });
const F_TOTAL = formula("f-total", "Total", "number");
const F_LABEL = formula("f-label", "Label", "text");
const F_FLAG = formula("f-flag", "Flag", "boolean");
const F_WHEN = formula("f-when", "When", "date", true);
const F_PAGE = formula("f-page", "Linked", "page", true);
const F_WHO = formula("f-who", "Who", "person");
const F_BROKEN = def("f-broken", "Broken", "formula", { config: { kind: "formula", expression: "" } });
const rollup = (id: string, name: string, target: string, fn: "sum" | "show_original" | "earliest"): PropertyDef =>
  def(id, name, "rollup", { config: { kind: "rollup", relationPropertyId: "p-project", target, function: fn } });
const R_SUM = rollup("r-sum", "Hours total", "p-hours", "sum");
const R_PRIO = rollup("r-prio", "Priorities", "p-prio", "show_original");
const R_TITLES = rollup("r-titles", "Titles", "title", "show_original");
const R_EARLIEST = rollup("r-earliest", "First due", "p-due", "earliest");
const R_NUMBERS = rollup("r-numbers", "Numbers", "p-hours", "show_original");

const SCHEMA: readonly PropertyDef[] = [
  STATUS,
  PRIORITY,
  TAGS,
  DONE,
  OWNER,
  PROJECT,
  NOTE,
  SITE,
  DUE,
  HOURS,
  MADE,
  EDITED,
  FILES,
  F_TOTAL,
  F_LABEL,
  F_FLAG,
  F_WHEN,
  F_PAGE,
  F_WHO,
  F_BROKEN,
  R_SUM,
  R_PRIO,
  R_TITLES,
  R_EARLIEST,
  R_NUMBERS,
];

const at = (year: number, month: number, day: number, hour = 12): number => new Date(year, month - 1, day, hour).getTime();

let counter = 0;
function page(values: [string, PropertyValue][] = [], extra: Partial<PageInfo> = {}): PageInfo {
  counter += 1;
  const id = `22222222-2222-4222-8222-${String(counter).padStart(12, "0")}`;
  return { id, spaceId: SPACE, title: `Page ${String(counter)}`, createdAt: at(2026, 10, 5), updatedAt: at(2026, 10, 6), properties: values, ...extra };
}

const status = (value: string): [string, PropertyValue] => ["p-status", { kind: "status", value }];
const hours = (value: number): [string, PropertyValue] => ["p-hours", { kind: "number", value }];
const due = (date: string, time?: string): [string, PropertyValue] => ["p-due", { kind: "date", value: time === undefined ? { date } : { date, time } }];
const note = (value: string): [string, PropertyValue] => ["p-note", { kind: "text", value }];

/** A context that does not depend on the machine: the US locale, weeks starting on Monday unless a test says otherwise. */
const ctx: ChartContext = { locale: "en-US", weekStart: 1 };

const bar = (groupBy: string | null, extra: Partial<ChartSpec> = {}): ChartSpec => ({ type: "bar", groupBy, measure: { fn: "count" }, ...extra });

const labels = (groups: readonly { label: string }[]): string[] => groups.map((group) => group.label);
const values = (groups: readonly { value: number | null }[]): (number | null)[] => groups.map((group) => group.value);

// ----- the spec --------------------------------------------------------------------------------------

describe("readChartSpec and chartSpecToJson", () => {
  it("reads every key of a full spec and writes it back as it was", () => {
    const json = {
      type: "line",
      groupBy: "p-due",
      bucket: "week",
      measure: { fn: "sum", propertyId: "p-hours" },
      cumulative: true,
      showEmpty: true,
    };
    const spec = readChartSpec(json);
    expect(spec).toEqual({
      type: "line",
      groupBy: "p-due",
      bucket: "week",
      measure: { fn: "sum", propertyId: "p-hours" },
      cumulative: true,
      showEmpty: true,
    });
    expect(spec === null ? null : chartSpecToJson(spec)).toEqual(json);
  });

  it("keeps the keys it does not know in `extra` and writes them back", () => {
    const json = { type: "bar", groupBy: null, measure: { fn: "count" }, horizontal: true, order: "value_desc", palette: "warm", nested: { a: [1, 2] } };
    const spec = readChartSpec(json);
    expect(spec?.extra).toEqual({ palette: "warm", nested: { a: [1, 2] } });
    expect(spec?.horizontal).toBe(true);
    expect(spec?.order).toBe("value_desc");
    expect(spec === null ? null : chartSpecToJson(spec)).toEqual(json);
  });

  it("writes the keys that differ from their default only", () => {
    expect(chartSpecToJson({ type: "donut", groupBy: "created", measure: { fn: "count" }, order: "default", horizontal: false })).toEqual({
      type: "donut",
      groupBy: "created",
      measure: { fn: "count" },
    });
  });

  it("reads a part that is unusable as its default", () => {
    expect(readChartSpec({ type: "bar", groupBy: 12, bucket: "decade", measure: "x", order: "random", horizontal: "yes" })).toEqual({
      type: "bar",
      groupBy: null,
      measure: { fn: "count" },
    });
    expect(readChartSpec({ type: "bar", groupBy: "", measure: { fn: "sum" } })).toEqual({ type: "bar", groupBy: null, measure: { fn: "count" } });
    expect(readChartSpec({ type: "bar", measure: { fn: "sum", propertyId: "" } })?.measure).toEqual({ fn: "count" });
  });

  it("is null for what is not a chart this version can read", () => {
    expect(readChartSpec(null)).toBeNull();
    expect(readChartSpec("bar")).toBeNull();
    expect(readChartSpec([])).toBeNull();
    expect(readChartSpec({ groupBy: "created" })).toBeNull();
    expect(readChartSpec({ type: "radar", groupBy: "created", measure: { fn: "count" } })).toBeNull();
    // A measure function of a newer version: the widget is kept as it was rather than read as a count.
    expect(readChartSpec({ type: "bar", groupBy: "created", measure: { fn: "range", propertyId: "p-hours" } })).toBeNull();
  });

  it("does not take a __proto__ key as an own setting", () => {
    const json = JSON.parse('{"type":"bar","groupBy":null,"measure":{"fn":"count"},"__proto__":{"x":1}}') as unknown;
    const spec = readChartSpec(json);
    expect(spec?.extra).toBeUndefined();
    expect(Object.getPrototypeOf(spec)).toBe(Object.prototype);
  });
});

describe("defaultChartSpec", () => {
  it("counts by the first status or select property", () => {
    expect(defaultChartSpec([NOTE, PRIORITY, STATUS])).toEqual({ type: "bar", groupBy: "p-prio", measure: { fn: "count" } });
    expect(defaultChartSpec([STATUS])).toEqual({ type: "bar", groupBy: "p-status", measure: { fn: "count" } });
  });

  it("counts by month created when there is none", () => {
    const spec = { type: "bar", groupBy: "created", bucket: "month", measure: { fn: "count" } };
    expect(defaultChartSpec([])).toEqual(spec);
    expect(defaultChartSpec([NOTE, TAGS, DUE])).toEqual(spec);
  });
});

// ----- what can group, what can be measured --------------------------------------------------------------

describe("which properties group and which are measured", () => {
  it("groups by options, checkboxes, people, relations, texts, dates, and by the results of formulas and rollups that make groups", () => {
    expect(groupableProperties(SCHEMA).map((candidate) => candidate.id)).toEqual([
      "p-status",
      "p-prio",
      "p-tags",
      "p-done",
      "p-owner",
      "p-project",
      "p-note",
      "p-site",
      "p-due",
      "p-made",
      "p-edited",
      "f-label",
      "f-flag",
      "f-when",
      "f-page",
      "f-who",
      "r-prio",
      "r-titles",
    ]);
  });

  it("measures numbers, rollups that make a number and formulas that make a number", () => {
    // Not "r-earliest" (a day) nor "r-numbers" (a list), not a text formula.
    expect(measurableProperties(SCHEMA).map((candidate) => candidate.id)).toEqual(["p-hours", "f-total", "r-sum"]);
  });

  it("knows a date source", () => {
    expect(isDateSource("created", SCHEMA)).toBe(true);
    expect(isDateSource("updated", SCHEMA)).toBe(true);
    expect(isDateSource("p-due", SCHEMA)).toBe(true);
    expect(isDateSource("p-made", SCHEMA)).toBe(true);
    expect(isDateSource("f-when", SCHEMA)).toBe(true);
    expect(isDateSource("p-status", SCHEMA)).toBe(false);
    expect(isDateSource("gone", SCHEMA)).toBe(false);
    expect(isDateSource(null, SCHEMA)).toBe(false);
  });
});

describe("changing a spec", () => {
  it("drops the order when the groups become dates", () => {
    const spec = bar("p-status", { order: "value_desc" });
    expect(withGroupBy(spec, "p-due", SCHEMA).order).toBeUndefined();
    expect(withGroupBy(spec, "p-prio", SCHEMA).order).toBe("value_desc");
    expect(withGroupBy(spec, "created", SCHEMA).groupBy).toBe("created");
  });

  it("gives a function of a number a property: the current one, else the first", () => {
    expect(withMeasureFunction(bar("p-status"), "sum", SCHEMA).measure).toEqual({ fn: "sum", propertyId: "p-hours" });
    expect(withMeasureFunction(bar("p-status", { measure: { fn: "max", propertyId: "r-sum" } }), "min", SCHEMA).measure).toEqual({
      fn: "min",
      propertyId: "r-sum",
    });
    expect(withMeasureFunction(bar("p-status", { measure: { fn: "max", propertyId: "r-sum" } }), "count", SCHEMA).measure).toEqual({ fn: "count" });
    // Nothing to measure: the spec stays.
    const spec = bar("p-status");
    expect(withMeasureFunction(spec, "sum", [STATUS])).toBe(spec);
    expect(withMeasureProperty(bar("p-status", { measure: { fn: "sum", propertyId: "p-hours" } }), "f-total").measure).toEqual({ fn: "sum", propertyId: "f-total" });
    expect(withMeasureProperty(spec, "f-total")).toBe(spec);
  });
});

describe("chartTitle", () => {
  it("names the measure and the group", () => {
    expect(chartTitle(bar("p-status"), SCHEMA, t)).toBe("Count by Status");
    expect(chartTitle(bar("p-prio", { measure: { fn: "average", propertyId: "p-hours" } }), SCHEMA, t)).toBe("Average of Hours by Priority");
    expect(chartTitle(bar("p-due", { bucket: "month", measure: { fn: "sum", propertyId: "p-hours" } }), SCHEMA, t)).toBe("Sum of Hours by Due (month)");
  });

  it("names a date source with its bucket, month when there is none", () => {
    expect(chartTitle(bar("created"), SCHEMA, t)).toBe("Count by Created time (month)");
    expect(chartTitle(bar("updated", { bucket: "week" }), SCHEMA, t)).toBe("Count by Last edited time (week)");
    expect(chartTitle(bar("p-due", { bucket: "quarter", measure: { fn: "median", propertyId: "p-hours" } }), SCHEMA, t)).toBe("Median of Hours by Due (quarter)");
  });

  it("falls back when the group or the property is gone", () => {
    expect(chartTitle(bar(null), SCHEMA, t)).toBe("Chart");
    expect(chartTitle(bar("p-gone"), SCHEMA, t)).toBe("Chart");
    expect(chartTitle(bar("p-status", { measure: { fn: "max", propertyId: "p-gone" } }), SCHEMA, t)).toBe("Maximum of Unknown property by Status");
  });
});

// ----- the groups ------------------------------------------------------------------------------------

describe("grouping by options", () => {
  it("makes one group per option, in the options' order, with its colour, and counts the pages", () => {
    const rows = [page([status("done")]), page([status("todo")]), page([status("done")])];
    const data = chartData(bar("p-status"), rows, SCHEMA, ctx);
    expect(data.status).toBe("ok");
    expect(data.dated).toBe(false);
    expect(labels(data.groups)).toEqual(["To do", "Doing", "Done"]);
    expect(values(data.groups)).toEqual([1, 0, 2]);
    expect(data.groups.map((group) => group.color)).toEqual(["gray", "blue", "green"]);
    expect(data.groups.map((group) => group.pages.length)).toEqual([1, 0, 2]);
    expect(data.groups[2]?.pages).toEqual([rows[0], rows[2]]);
    expect(data.groups.map((group) => group.key)).toEqual(["todo", "doing", "done"]);
  });

  it("has a group for pages without a value when asked, last, and not otherwise", () => {
    const rows = [page([status("todo")]), page(), page([["p-status", { kind: "status", value: "removed-option" }]])];
    const without = chartData(bar("p-status"), rows, SCHEMA, ctx);
    expect(labels(without.groups)).toEqual(["To do", "Doing", "Done"]);
    const withEmpty = chartData(bar("p-status", { showEmpty: true }), rows, SCHEMA, ctx);
    expect(labels(withEmpty.groups)).toEqual(["To do", "Doing", "Done", "No value"]);
    const empty = withEmpty.groups.at(-1);
    expect(empty).toMatchObject({ key: EMPTY_GROUP_KEY, kind: "empty", value: 2 });
    expect(empty?.pages).toEqual([rows[1], rows[2]]);
  });

  it("counts a page of a multi-select in each of its options, once", () => {
    const rows = [
      page([["p-tags", { kind: "multi_select", value: ["a", "b"] }]]),
      page([["p-tags", { kind: "multi_select", value: ["b", "b"] }]]),
      page([["p-tags", { kind: "multi_select", value: [] }]]),
    ];
    const data = chartData(bar("p-tags", { showEmpty: true }), rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["Alpha", "Beta", "No value"]);
    expect(values(data.groups)).toEqual([1, 2, 1]);
  });

  it("orders by value, the group without a value last whichever way", () => {
    const rows = [page([status("doing"), hours(5)]), page([status("todo"), hours(9)]), page([status("todo"), hours(1)]), page([status("done")])];
    const spec = (order: "value_desc" | "value_asc"): ChartSpec => bar("p-status", { order, measure: { fn: "sum", propertyId: "p-hours" } });
    const desc = chartData(spec("value_desc"), rows, SCHEMA, ctx);
    expect(labels(desc.groups)).toEqual(["To do", "Doing", "Done"]);
    expect(values(desc.groups)).toEqual([10, 5, null]);
    const asc = chartData(spec("value_asc"), rows, SCHEMA, ctx);
    expect(labels(asc.groups)).toEqual(["Doing", "To do", "Done"]);
  });

  it("keeps the natural order of groups that tie", () => {
    const rows = [page([status("done")]), page([status("todo")])];
    const data = chartData(bar("p-status", { order: "value_desc" }), rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["To do", "Done", "Doing"]);
  });
});

describe("grouping by a checkbox", () => {
  it("has Checked and Unchecked, a page without a value being unchecked", () => {
    const rows = [page([["p-done", { kind: "checkbox", value: true }]]), page([["p-done", { kind: "checkbox", value: false }]]), page(), page([["p-done", { kind: "checkbox", value: true }]])];
    const data = chartData(bar("p-done", { showEmpty: true }), rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["Checked", "Unchecked"]);
    expect(values(data.groups)).toEqual([2, 2]);
  });
});

describe("grouping by people", () => {
  const owners = (...ids: string[]): [string, PropertyValue] => ["p-owner", { kind: "person", value: ids }];
  const names: ChartContext = { ...ctx, personName: (id) => ({ "acc-b": "Bea", "acc-a": "Ada", "acc-z": "adam" })[id] ?? id };

  it("has one group per account named by the member's name, by name, and counts a page in each of its people", () => {
    const rows = [page([owners("acc-b", "acc-a")]), page([owners("acc-a")]), page([owners("acc-z")]), page([owners()])];
    const data = chartData(bar("p-owner", { showEmpty: true }), rows, SCHEMA, names);
    expect(labels(data.groups)).toEqual(["Ada", "adam", "Bea", "No value"]);
    expect(values(data.groups)).toEqual([2, 1, 1, 1]);
    expect(data.groups.map((group) => group.key)).toEqual(["acc-a", "acc-z", "acc-b", EMPTY_GROUP_KEY]);
  });

  it("shows the account when nothing names it", () => {
    const data = chartData(bar("p-owner"), [page([owners("acc-q")])], SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["acc-q"]);
  });
});

describe("grouping by a relation", () => {
  const related = (...ids: string[]): [string, PropertyValue] => ["p-project", { kind: "relation", value: ids }];
  const titles: ChartContext = { ...ctx, pageTitle: (id) => ({ "pg-1": "Website", "pg-2": "App", "pg-3": "  " })[id] ?? null };

  it("has one group per related page named by its title, by title, a page counting in each", () => {
    const rows = [page([related("pg-1", "pg-2")]), page([related("pg-1")]), page([related("pg-3")]), page([related("pg-9")])];
    const data = chartData(bar("p-project"), rows, SCHEMA, titles);
    expect(labels(data.groups)).toEqual(["App", "Unknown page", "Untitled", "Website"]);
    expect(values(data.groups)).toEqual([1, 1, 1, 2]);
  });
});

describe("grouping by text", () => {
  it("has one group per distinct trimmed text, ordered as the locale does, and skips empty ones", () => {
    const rows = [note("beta"), note("  Alpha "), note("beta"), note("   "), note("alpha"), note("item 10"), note("item 9")].map((value) => page([value]));
    const data = chartData(bar("p-note", { showEmpty: true }), [...rows, page()], SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["Alpha", "alpha", "beta", "item 9", "item 10", "No value"]);
    expect(values(data.groups)).toEqual([1, 1, 2, 1, 1, 2]);
  });

  it("groups a place by its name and reads url, email and phone as text", () => {
    const places = [page([["p-site", { kind: "place", value: { name: "Paris", lat: 48.85, lon: 2.35 } }]]), page([["p-site", { kind: "place", value: { name: "Paris" } }]])];
    expect(labels(chartData(bar("p-site"), places, SCHEMA, ctx).groups)).toEqual(["Paris"]);
    expect(values(chartData(bar("p-site"), places, SCHEMA, ctx).groups)).toEqual([2]);
    const urls = def("p-url", "Link", "url");
    const rows = [page([["p-url", { kind: "url", value: "https://a.example" }]]), page([["p-url", { kind: "url", value: "https://a.example" }]])];
    expect(values(chartData(bar("p-url"), rows, [urls], ctx).groups)).toEqual([2]);
  });
});

describe("grouping by a date", () => {
  const rows = [page([due("2026-10-05")]), page([due("2026-10-07", "09:30")]), page([due("2026-10-18")]), page([due("2026-10-20")]), page([due("2026-08-31")])];

  it("makes a bucket per month from the first date to the last, the empty ones included", () => {
    const data = chartData(bar("p-due", { bucket: "month" }), rows, SCHEMA, ctx);
    expect(data.dated).toBe(true);
    expect(labels(data.groups)).toEqual(["Aug 2026", "Sep 2026", "Oct 2026"]);
    expect(data.groups.map((group) => group.fullLabel)).toEqual(["August 2026", "September 2026", "October 2026"]);
    expect(values(data.groups)).toEqual([1, 0, 4]);
    expect(data.groups.map((group) => group.key)).toEqual(["2026-08-01", "2026-09-01", "2026-10-01"]);
  });

  it("is month when the spec has no bucket", () => {
    expect(labels(chartData(bar("p-due"), rows, SCHEMA, ctx).groups)).toEqual(["Aug 2026", "Sep 2026", "Oct 2026"]);
  });

  it("makes day buckets with the day's name", () => {
    const data = chartData(bar("p-due", { bucket: "day" }), [page([due("2026-10-05")]), page([due("2026-10-08")]), page([due("2026-10-08")])], SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["Oct 5", "Oct 6", "Oct 7", "Oct 8"]);
    expect(data.groups.map((group) => group.fullLabel)).toEqual(["Oct 5, 2026", "Oct 6, 2026", "Oct 7, 2026", "Oct 8, 2026"]);
    expect(values(data.groups)).toEqual([1, 0, 0, 2]);
  });

  it("makes week buckets from the locale's first day of the week", () => {
    const october = rows.slice(0, 4);
    const monday = chartData(bar("p-due", { bucket: "week" }), october, SCHEMA, { ...ctx, weekStart: 1 });
    expect(monday.groups.map((group) => group.key)).toEqual(["2026-10-05", "2026-10-12", "2026-10-19"]);
    expect(values(monday.groups)).toEqual([2, 1, 1]);
    expect(labels(monday.groups)).toEqual(["Oct 5", "Oct 12", "Oct 19"]);
    expect(monday.groups[0]?.fullLabel).toBe("Week of Oct 5, 2026");
    const sunday = chartData(bar("p-due", { bucket: "week" }), october, SCHEMA, { ...ctx, weekStart: 0 });
    expect(sunday.groups.map((group) => group.key)).toEqual(["2026-10-04", "2026-10-11", "2026-10-18"]);
    expect(values(sunday.groups)).toEqual([2, 0, 2]);
  });

  it("makes quarter and year buckets", () => {
    const quarters = chartData(bar("p-due", { bucket: "quarter" }), [...rows, page([due("2026-01-15")])], SCHEMA, ctx);
    expect(labels(quarters.groups)).toEqual(["Q1 2026", "Q2 2026", "Q3 2026", "Q4 2026"]);
    expect(values(quarters.groups)).toEqual([1, 0, 1, 4]);
    const years = chartData(bar("p-due", { bucket: "year" }), [...rows, page([due("2024-02-02")])], SCHEMA, ctx);
    expect(labels(years.groups)).toEqual(["2024", "2025", "2026"]);
    expect(values(years.groups)).toEqual([1, 0, 5]);
  });

  it("is chronological whatever the order says, and has a group for pages without a date when asked", () => {
    const data = chartData(bar("p-due", { bucket: "month", order: "value_desc", showEmpty: true }), [...rows, page()], SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["Aug 2026", "Sep 2026", "Oct 2026", "No value"]);
    expect(values(data.groups)).toEqual([1, 0, 4, 1]);
  });

  it("leaves out a date that is not a day, and is empty when no page has one", () => {
    const broken = [page([due("not-a-date")]), page([due("2026-13-40")])];
    expect(chartData(bar("p-due"), broken, SCHEMA, ctx).status).toBe("empty");
    // Asked for, they are the group without a value, alone: there is no bucket to put them after.
    const asked = chartData(bar("p-due", { showEmpty: true }), broken, SCHEMA, ctx);
    expect(asked.groups.map((group) => group.key)).toEqual([EMPTY_GROUP_KEY]);
    expect(values(asked.groups)).toEqual([2]);
  });

  it("buckets the time a page was created and last edited on the local day", () => {
    const made = [page([], { createdAt: at(2026, 10, 5, 0), updatedAt: at(2026, 11, 2, 23) }), page([], { createdAt: at(2026, 10, 5, 23), updatedAt: at(2026, 11, 3, 1) })];
    const created = chartData(bar("created", { bucket: "day" }), made, SCHEMA, ctx);
    expect(created.groups.map((group) => group.key)).toEqual(["2026-10-05"]);
    expect(values(created.groups)).toEqual([2]);
    const updated = chartData(bar("updated", { bucket: "day" }), made, SCHEMA, ctx);
    expect(updated.groups.map((group) => group.key)).toEqual(["2026-11-02", "2026-11-03"]);
  });

  it("reads a created or last-edited time property from its value, else from the page", () => {
    const rowsOfTimes = [
      page([["p-made", { kind: "created_time", value: at(2026, 3, 9) }]], { createdAt: at(2026, 1, 1) }),
      page([], { createdAt: at(2026, 1, 20), updatedAt: at(2026, 5, 5) }),
    ];
    expect(chartData(bar("p-made", { bucket: "month" }), rowsOfTimes, SCHEMA, ctx).groups.map((group) => group.key)).toEqual([
      "2026-01-01",
      "2026-02-01",
      "2026-03-01",
    ]);
    expect(chartData(bar("p-edited", { bucket: "month" }), rowsOfTimes, SCHEMA, ctx).groups.map((group) => group.key)).toContain("2026-05-01");
  });

  it("keeps the latest 400 buckets, counts the others, and a bar chart draws the latest 50", () => {
    const days = Array.from({ length: CHART_MAX_BUCKETS + 5 }, (_, offset) => page([due(new Date(Date.UTC(2025, 0, 1 + offset)).toISOString().slice(0, 10))]));
    const line = chartData({ type: "line", groupBy: "p-due", bucket: "day", measure: { fn: "count" } }, days, SCHEMA, ctx);
    expect(line.groups).toHaveLength(CHART_MAX_BUCKETS);
    expect(line.hidden).toBe(5);
    expect(line.groups[0]?.key).toBe("2025-01-06");
    expect(line.groups.at(-1)?.key).toBe("2026-02-09");
    const barData = chartData(bar("p-due", { bucket: "day" }), days, SCHEMA, ctx);
    expect(barData.groups).toHaveLength(CHART_MAX_BARS);
    expect(barData.hidden).toBe(5 + (CHART_MAX_BUCKETS - CHART_MAX_BARS));
    expect(barData.groups.at(-1)?.key).toBe("2026-02-09");
  });

  it("does not loop at the start of the calendar", () => {
    const data = chartData(bar("p-due", { bucket: "week" }), [page([due("0001-01-01")]), page([due("0001-01-09")])], SCHEMA, ctx);
    expect(data.groups.length).toBeGreaterThan(0);
    expect(data.groups.length).toBeLessThan(5);
  });
});

describe("grouping by a formula", () => {
  const result = (id: string, data: unknown): [string, PropertyValue] => [id, { kind: "formula", value: data as never }];

  it("groups by a text result, a list counting in each item once", () => {
    const rows = [
      page([result("f-label", { type: "text", data: "red" })]),
      page([result("f-label", { type: "list", data: [{ type: "text", data: "red" }, { type: "text", data: "blue" }, { type: "text", data: "red" }] })]),
      page([result("f-label", { type: "text", data: "" })]),
      page(),
    ];
    const data = chartData(bar("f-label", { showEmpty: true }), rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["blue", "red", "No value"]);
    expect(values(data.groups)).toEqual([1, 2, 2]);
  });

  it("groups by a boolean result as a checkbox, and a page without a result has no value", () => {
    const rows = [page([result("f-flag", { type: "boolean", data: true })]), page([result("f-flag", { type: "boolean", data: false })]), page()];
    const data = chartData(bar("f-flag", { showEmpty: true }), rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["Checked", "Unchecked", "No value"]);
    expect(values(data.groups)).toEqual([1, 1, 1]);
  });

  it("groups by a date result in buckets, a list result counting in each item's bucket", () => {
    const rows = [
      page([result("f-when", { type: "list", data: [{ type: "date", data: { date: "2026-08-02" } }, { type: "date", data: { date: "2026-08-20" } }, { type: "date", data: { date: "2026-10-01" } }] })]),
      page([result("f-when", { type: "date", data: { date: "2026-10-09" } })]),
    ];
    const data = chartData(bar("f-when", { bucket: "month" }), rows, SCHEMA, ctx);
    expect(data.dated).toBe(true);
    expect(labels(data.groups)).toEqual(["Aug 2026", "Sep 2026", "Oct 2026"]);
    expect(values(data.groups)).toEqual([1, 0, 2]);
  });

  it("groups by a page result by title and a person result by name", () => {
    const names: ChartContext = { ...ctx, pageTitle: (id) => (id === "pg-1" ? "Website" : null), personName: () => "Ada" };
    const pages = [page([result("f-page", { type: "list", data: [{ type: "page", data: "pg-1" }] })]), page([result("f-page", { type: "page", data: "pg-1" })])];
    expect(labels(chartData(bar("f-page"), pages, SCHEMA, names).groups)).toEqual(["Website"]);
    expect(values(chartData(bar("f-page"), pages, SCHEMA, names).groups)).toEqual([2]);
    const people = [page([result("f-who", { type: "person", data: "acc-1" })])];
    expect(labels(chartData(bar("f-who"), people, SCHEMA, names).groups)).toEqual(["Ada"]);
  });

  it("cannot group by a number result or by a formula that no longer checks", () => {
    const rows = [page([result("f-total", { type: "number", data: 4 })])];
    expect(chartData(bar("f-total"), rows, SCHEMA, ctx).status).toBe("no_group");
    expect(chartData(bar("f-broken"), rows, SCHEMA, ctx).status).toBe("no_group");
  });
});

describe("grouping by a rollup", () => {
  const original = (id: string, items: PropertyValue[]): [string, PropertyValue] => [id, { kind: "rollup", value: { function: "show_original", result: { type: "values", data: items } } }];

  it("groups the original values of a select as text, each once per page", () => {
    const rows = [
      page([original("r-prio", [{ kind: "select", value: "hi" }, { kind: "select", value: "lo" }, { kind: "select", value: "hi" }])]),
      page([original("r-prio", [{ kind: "select", value: "lo" }])]),
      page(),
    ];
    const data = chartData(bar("r-prio", { showEmpty: true }), rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["High", "Low", "No value"]);
    expect(values(data.groups)).toEqual([1, 2, 1]);
  });

  it("groups the titles of the related pages", () => {
    const rows = [page([original("r-titles", [{ kind: "text", value: "Alpha" }, { kind: "text", value: "Beta" }])]), page([original("r-titles", [{ kind: "text", value: "Beta" }])])];
    const data = chartData(bar("r-titles"), rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["Alpha", "Beta"]);
    expect(values(data.groups)).toEqual([1, 2]);
  });

  it("does not group by a rollup that makes a number, a day or a list of numbers", () => {
    expect(chartData(bar("r-sum"), [page()], SCHEMA, ctx).status).toBe("no_group");
    expect(chartData(bar("r-earliest"), [page()], SCHEMA, ctx).status).toBe("no_group");
    expect(chartData(bar("r-numbers"), [page()], SCHEMA, ctx).status).toBe("no_group");
  });
});

describe("what cannot be drawn", () => {
  it("has no group when none is chosen, the chosen property is gone or cannot group", () => {
    const rows = [page([status("todo")])];
    expect(chartData(bar(null), rows, SCHEMA, ctx)).toMatchObject({ status: "no_group", groups: [] });
    expect(chartData(bar("p-gone"), rows, SCHEMA, ctx).status).toBe("no_group");
    expect(chartData(bar("p-hours"), rows, SCHEMA, ctx).status).toBe("no_group");
    expect(chartData(bar("p-files"), rows, SCHEMA, ctx).status).toBe("no_group");
  });

  it("is empty when no page is left, and when no page has the number", () => {
    expect(chartData(bar("p-status"), [], SCHEMA, ctx)).toMatchObject({ status: "empty", groups: [] });
    const rows = [page([status("todo")])];
    expect(chartData(bar("p-status", { measure: { fn: "sum", propertyId: "p-hours" } }), rows, SCHEMA, ctx).status).toBe("empty");
    // Pages that have no value for the group and are not asked for are no data either.
    expect(chartData(bar("p-status"), [page()], SCHEMA, ctx).status).toBe("empty");
    expect(chartData(bar("p-status", { showEmpty: true }), [page()], SCHEMA, ctx).status).toBe("ok");
  });

  it("asks for another measure when its property is gone or holds no number", () => {
    const rows = [page([status("todo")])];
    expect(chartData(bar("p-status", { measure: { fn: "sum", propertyId: "p-gone" } }), rows, SCHEMA, ctx).status).toBe("no_measure");
    expect(chartData(bar("p-status", { measure: { fn: "sum", propertyId: "p-note" } }), rows, SCHEMA, ctx).status).toBe("no_measure");
    expect(chartData(bar("p-status", { measure: { fn: "max", propertyId: "r-earliest" } }), rows, SCHEMA, ctx).status).toBe("no_measure");
    expect(chartData(bar("p-status", { measure: { fn: "sum", propertyId: "f-label" } }), rows, SCHEMA, ctx).status).toBe("no_measure");
  });
});

// ----- the measures ------------------------------------------------------------------------------------

describe("measures", () => {
  const rows = [
    page([status("todo"), hours(2)]),
    page([status("todo"), hours(4)]),
    page([status("done"), hours(10)]),
    page([status("done")]),
    page([status("doing")]),
  ];
  const measured = (fn: "sum" | "average" | "median" | "min" | "max", propertyId = "p-hours"): (number | null)[] =>
    values(chartData(bar("p-status", { measure: { fn, propertyId } }), rows, SCHEMA, ctx).groups);

  it("counts the pages", () => {
    expect(values(chartData(bar("p-status"), rows, SCHEMA, ctx).groups)).toEqual([2, 1, 2]);
  });

  it("sums, averages, takes the median, the minimum and the maximum of a number, a group without a number having no value", () => {
    expect(measured("sum")).toEqual([6, null, 10]);
    expect(measured("average")).toEqual([3, null, 10]);
    expect(measured("median")).toEqual([3, null, 10]);
    expect(measured("min")).toEqual([2, null, 10]);
    expect(measured("max")).toEqual([4, null, 10]);
  });

  it("takes the median as the table footer does: the middle of an odd count, the mean of the two middles of an even one", () => {
    const median = { fn: "median", propertyId: "p-hours" } as const;
    const even = [1, 9, 4, 100].map((value) => page([status("todo"), hours(value)]));
    expect(values(chartData(bar("p-status", { measure: median }), even, SCHEMA, ctx).groups)[0]).toBe(6.5);
    const odd = [1, 9, 4].map((value) => page([status("todo"), hours(value)]));
    expect(values(chartData(bar("p-status", { measure: median }), odd, SCHEMA, ctx).groups)[0]).toBe(4);
  });

  it("measures a rollup that makes a number and a formula that makes a number", () => {
    const withRollup = [
      page([status("todo"), ["r-sum", { kind: "rollup", value: { function: "sum", result: { type: "number", data: 7 } } }]]),
      page([status("todo"), ["r-sum", { kind: "rollup", value: { function: "sum", result: { type: "number", data: 3 } } }]]),
    ];
    expect(values(chartData(bar("p-status", { measure: { fn: "sum", propertyId: "r-sum" } }), withRollup, SCHEMA, ctx).groups)[0]).toBe(10);
    const withFormula = [
      page([status("done"), ["f-total", { kind: "formula", value: { type: "number", data: 2.5 } }]]),
      page([status("done"), ["f-total", { kind: "formula", value: { type: "text", data: "x" } }]]),
      page([status("done")]),
    ];
    const data = chartData(bar("p-status", { measure: { fn: "average", propertyId: "f-total" } }), withFormula, SCHEMA, ctx);
    expect(values(data.groups)).toEqual([null, null, 2.5]);
  });

  it("measures the pages of the group without a value too", () => {
    const data = chartData(bar("p-status", { showEmpty: true, measure: { fn: "sum", propertyId: "p-hours" } }), [...rows, page([hours(7)])], SCHEMA, ctx);
    expect(data.groups.at(-1)).toMatchObject({ kind: "empty", value: 7 });
  });
});

describe("a cumulative line", () => {
  const line = (extra: Partial<ChartSpec> = {}): ChartSpec => ({ type: "line", groupBy: "p-due", bucket: "month", measure: { fn: "count" }, cumulative: true, ...extra });

  it("adds each point to the ones before, an empty bucket carrying the total", () => {
    const rows = [page([due("2026-08-10")]), page([due("2026-08-11")]), page([due("2026-10-01")])];
    expect(values(chartData(line(), rows, SCHEMA, ctx).groups)).toEqual([2, 2, 3]);
    expect(values(chartData(line({ cumulative: undefined }), rows, SCHEMA, ctx).groups)).toEqual([2, 0, 1]);
  });

  it("adds sums, a bucket without a number carrying the total once there is one", () => {
    const rows = [page([due("2026-08-10"), hours(2)]), page([due("2026-09-10")]), page([due("2026-10-01"), hours(5)])];
    expect(values(chartData(line({ measure: { fn: "sum", propertyId: "p-hours" } }), rows, SCHEMA, ctx).groups)).toEqual([2, 2, 7]);
    const late = [page([due("2026-08-10")]), page([due("2026-09-10"), hours(3)])];
    expect(values(chartData(line({ measure: { fn: "sum", propertyId: "p-hours" } }), late, SCHEMA, ctx).groups)).toEqual([null, 3]);
  });

  it("starts from the buckets the cap dropped", () => {
    const days = Array.from({ length: CHART_MAX_BUCKETS + 5 }, (_, offset) => page([due(new Date(Date.UTC(2025, 0, 1 + offset)).toISOString().slice(0, 10))]));
    const data = chartData(line({ bucket: "day" }), days, SCHEMA, ctx);
    expect(data.groups[0]?.value).toBe(6);
    expect(data.groups.at(-1)?.value).toBe(CHART_MAX_BUCKETS + 5);
  });

  it("only applies to a line and leaves out the group without a date, which has no place on a time axis", () => {
    const rows = [page([due("2026-08-10")]), page()];
    expect(values(chartData(bar("p-due", { cumulative: true }), rows, SCHEMA, ctx).groups)).toEqual([1]);
    const withEmpty = chartData(line({ showEmpty: true }), rows, SCHEMA, ctx);
    expect(labels(withEmpty.groups)).toEqual(["Aug 2026"]);
    // A line over options is not a time axis: its group without a value is a point.
    const options: ChartSpec = { type: "line", groupBy: "p-status", measure: { fn: "count" }, showEmpty: true };
    expect(labels(chartData(options, [page([status("todo")]), page()], SCHEMA, ctx).groups)).toEqual(["To do", "Doing", "Done", "No value"]);
  });
});

// ----- the caps ------------------------------------------------------------------------------------------

describe("caps", () => {
  const texts = (count: number): PageInfo[] => Array.from({ length: count }, (_, index) => page([note(`item ${String(index).padStart(3, "0")}`)]));

  it("draws at most 50 bars and counts the others", () => {
    const data = chartData(bar("p-note"), texts(60), SCHEMA, ctx);
    expect(data.groups).toHaveLength(CHART_MAX_BARS);
    expect(data.hidden).toBe(10);
    expect(data.groups[0]?.label).toBe("item 000");
    const none = chartData(bar("p-note"), texts(50), SCHEMA, ctx);
    expect(none.hidden).toBe(0);
  });

  it("keeps the biggest 50 when the bars are ordered by value", () => {
    const rows = [...texts(60), page([note("item 059")]), page([note("item 059")])];
    const data = chartData(bar("p-note", { order: "value_desc" }), rows, SCHEMA, ctx);
    expect(data.groups[0]?.label).toBe("item 059");
    expect(data.groups).toHaveLength(CHART_MAX_BARS);
  });

  it("names at most 8 slices of a donut, the biggest by value, and puts the rest in Other", () => {
    // item 1 has 10 pages, item 2 has 9, ... item 10 has 1.
    const rows = Array.from({ length: 10 }, (_, index) => Array.from({ length: 10 - index }, () => page([note(`t${String(index + 1)}`)]))).flat();
    const data = chartData({ type: "donut", groupBy: "p-note", measure: { fn: "count" } }, rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8", "Other"]);
    expect(values(data.groups)).toEqual([10, 9, 8, 7, 6, 5, 4, 3, 3]);
    const other = data.groups.at(-1);
    expect(other).toMatchObject({ key: OTHER_GROUP_KEY, kind: "other", value: 3 });
    expect(other?.pages).toHaveLength(3);
    expect(data.total).toBe(55);
    expect(data.hidden).toBe(0);
    expect(data.groups.slice(0, 8).map((group) => group.paletteIndex)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
  });

  it("picks the slices by value but draws them in the order asked for", () => {
    const rows = Array.from({ length: 10 }, (_, index) => Array.from({ length: index + 1 }, () => page([note(`t${String(index + 1)}`)]))).flat();
    const data = chartData({ type: "donut", groupBy: "p-note", measure: { fn: "count" }, order: "value_desc" }, rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["t10", "t9", "t8", "t7", "t6", "t5", "t4", "t3", "Other"]);
    expect(data.groups.at(-1)?.value).toBe(3);
  });

  it("leaves out of a donut the groups that have no value or a value that is not above 0", () => {
    const rows = [page([status("todo"), hours(3)]), page([status("doing")]), page([status("done"), hours(0)])];
    const data = chartData({ type: "donut", groupBy: "p-status", measure: { fn: "sum", propertyId: "p-hours" } }, rows, SCHEMA, ctx);
    expect(labels(data.groups)).toEqual(["To do"]);
    expect(data.total).toBe(3);
    const negative = chartData({ type: "donut", groupBy: "p-status", measure: { fn: "sum", propertyId: "p-hours" } }, [page([status("todo"), hours(-2)])], SCHEMA, ctx);
    expect(negative.status).toBe("empty");
  });

  it("keeps an option's colour in a donut and gives Other the neutral slot", () => {
    const data = chartData({ type: "donut", groupBy: "p-status", measure: { fn: "count" } }, [page([status("done")]), page([status("todo")])], SCHEMA, ctx);
    expect(data.groups.map((group) => group.color)).toEqual(["gray", "green"]);
  });

  it("takes the other slice of an average over its pages, and a total that is the measure over every page", () => {
    const rows = Array.from({ length: 10 }, (_, index) => page([note(`t${String(index + 1)}`), hours(index + 1)]));
    const data = chartData({ type: "donut", groupBy: "p-note", measure: { fn: "average", propertyId: "p-hours" } }, rows, SCHEMA, ctx);
    // t1 to t10 average 1..10; the two smallest values (1 and 2) are the Other slice: their average is 1.5.
    expect(data.groups.at(-1)).toMatchObject({ kind: "other", value: 1.5 });
    expect(data.total).toBe(5.5);
  });
});

describe("the pages of a group", () => {
  it("keep the order of the rows", () => {
    const rows = [page([status("todo")], { title: "B" }), page([status("done")]), page([status("todo")], { title: "A" })];
    const todo = chartData(bar("p-status"), rows, SCHEMA, ctx).groups[0];
    expect(todo?.pages.map((candidate) => candidate.title)).toEqual(["B", "A"]);
  });
});
