// The operators of a view's filters (ADR-0058, `docs/architecture/database-power-ups.md` section 3): the table of
// operators per kind, what each operator says of a page with and without a value (a "not" operator is true for an empty
// value, except "is not" on a number), the day operators and `within` with relative values resolved at a fixed day and
// either first day of the week, and the values that make a filter not applicable. Pure: the pages are built directly.

import type { FormulaType, PageInfo, PropertyDef, PropertyValue } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { todayIso } from "./dates";
import { applyFilters, filterForOp, filterOps, isFilterOp, matchFilter, newFilter, opTakesValue, periodRange, readRelativeDay, readRelativePeriod, resolveDay } from "./page-list-filters";
import type { FilterClock, FilterOp, PageListFilter } from "./page-list-filters";

// ----- fixtures ----------------------------------------------------------------------------------------------------------

const def = (id: string, kind: PropertyDef["kind"], options: PropertyDef["options"] = []): PropertyDef => ({ id, name: id, kind, options, createdAt: 0 });
const rollupDef = (id: string, fn: string): PropertyDef => ({
  id,
  name: id,
  kind: "rollup",
  options: [],
  createdAt: 0,
  config: { kind: "rollup", relationPropertyId: "rel", target: "title", function: fn } as never,
});
const formulaDef = (id: string, type: FormulaType): PropertyDef => ({
  id,
  name: id,
  kind: "formula",
  options: [],
  createdAt: 0,
  config: { kind: "formula", expression: `prop("x")`, resultType: type },
});

const OPTIONS = [
  { id: "o-a", name: "A", color: "gray" as const },
  { id: "o-b", name: "B", color: "blue" as const },
];

const DEFS: readonly PropertyDef[] = [
  def("text", "text"),
  def("url", "url"),
  def("email", "email"),
  def("phone", "phone"),
  def("place", "place"),
  def("num", "number"),
  rollupDef("sum", "sum"),
  rollupDef("late", "latest"),
  def("sel", "select", OPTIONS),
  def("stat", "status", OPTIONS),
  def("multi", "multi_select", OPTIONS),
  def("rel", "relation"),
  def("who", "person"),
  def("day", "date"),
  def("made", "created_time"),
  def("edited", "last_edited_time"),
  def("by", "created_by"),
  def("box", "checkbox"),
  def("todo", "checklist"),
  formulaDef("f-date", { base: "date", list: false }),
];

function page(id: string, values: Record<string, unknown> = {}): PageInfo {
  return {
    id,
    spaceId: "s",
    title: id,
    createdAt: 0,
    updatedAt: 0,
    properties: Object.entries(values).map(([key, value]): [string, PropertyValue] => [key, value as PropertyValue]),
  };
}

const filter = (propertyId: string, op: FilterOp, value?: unknown): PageListFilter => (value === undefined ? { propertyId, op } : { propertyId, op, value });
const check = (p: PageInfo, f: PageListFilter, me?: string, clock: FilterClock = {}): boolean | null => matchFilter(p, f, DEFS, me, {}, clock);
/** The ids of the pages that pass one filter. */
const passing = (pages: readonly PageInfo[], f: PageListFilter, me?: string, clock: FilterClock = {}): string[] =>
  applyFilters(pages, [f], DEFS, me, {}, clock).map((entry) => entry.id);

const text = (value: string): PropertyValue => ({ kind: "text", value });
const num = (value: number): PropertyValue => ({ kind: "number", value });
const date = (day: string, time?: string): PropertyValue => ({ kind: "date", value: time === undefined ? { date: day } : { date: day, time } });
/** A local instant: the days of a filter are the person's days. */
const at = (y: number, m: number, d: number, h = 12, min = 0): number => new Date(y, m - 1, d, h, min).getTime();

/** Wednesday 2026-10-14: its week is 11 to 17 October when it starts on Sunday, 12 to 18 when it starts on Monday. */
const TODAY = "2026-10-14";
const SUNDAY: FilterClock = { today: TODAY, weekStart: 0 };
const MONDAY: FilterClock = { today: TODAY, weekStart: 1 };

// ----- the table ---------------------------------------------------------------------------------------------------------

describe("the operators of each kind", () => {
  it("lists the operators of the text kinds in the editor's order", () => {
    for (const kind of ["text", "url", "email", "phone"] as const) {
      expect(filterOps(kind), kind).toEqual(["contains", "notContains", "equals", "notEquals", "startsWith", "endsWith", "isSet", "isEmpty"]);
    }
    expect(filterOps("place")).toEqual(["contains", "notContains", "isSet", "isEmpty"]);
  });

  it("lists the comparisons of a number and of a numeric rollup, and only the two generic ones for another rollup", () => {
    const numeric = ["equals", "notEquals", "greater", "less", "atLeast", "atMost", "isSet", "isEmpty"];
    expect(filterOps("number")).toEqual(numeric);
    expect(filterOps("rollup", rollupDef("x", "sum"))).toEqual(numeric);
    expect(filterOps("rollup", rollupDef("x", "latest"))).toEqual(["isSet", "isEmpty"]);
    expect(filterOps("rollup")).toEqual(["isSet", "isEmpty"]);
  });

  it("lists \"does not have\", \"does not link to\" and the people's \"not\" operators", () => {
    for (const kind of ["select", "status", "multi_select"] as const) {
      expect(filterOps(kind), kind).toEqual(["hasOption", "lacksOption", "isSet", "isEmpty"]);
    }
    expect(filterOps("relation")).toEqual(["relatesTo", "notRelatesTo", "isSet", "isEmpty"]);
    expect(filterOps("person")).toEqual(["isMe", "notMe", "hasPerson", "lacksPerson", "isSet", "isEmpty"]);
  });

  it("lists the day operators and \"within\" for a date, and for the created and edited times (which are never empty)", () => {
    expect(filterOps("date")).toEqual(["equals", "before", "after", "onOrBefore", "onOrAfter", "between", "within", "isSet", "isEmpty"]);
    for (const kind of ["created_time", "last_edited_time"] as const) {
      expect(filterOps(kind), kind).toEqual(["equals", "before", "after", "onOrBefore", "onOrAfter", "between", "within", "isSet"]);
    }
  });

  it("leaves the other kinds and the formulas as they were", () => {
    expect(filterOps("checkbox")).toEqual(["checked", "unchecked"]);
    expect(filterOps("checklist")).toEqual(["complete", "incomplete", "isSet", "isEmpty"]);
    expect(filterOps("created_by")).toEqual(["equals", "isSet"]);
    expect(filterOps("last_edited_by")).toEqual(["equals", "isSet"]);
    expect(filterOps("files")).toEqual(["isSet", "isEmpty"]);
    expect(filterOps("hologram" as never)).toEqual(["isSet", "isEmpty"]);
    expect(filterOps("formula", formulaDef("f", { base: "date", list: false }))).toEqual(["equals", "before", "after", "between", "isSet", "isEmpty"]);
    expect(filterOps("formula", formulaDef("f", { base: "number", list: false }))).toEqual([
      "equals",
      "notEquals",
      "greater",
      "less",
      "atLeast",
      "atMost",
      "isSet",
      "isEmpty",
    ]);
  });

  it("knows which operators are the kind's, and which need a value", () => {
    expect(isFilterOp("text", "startsWith")).toBe(true);
    expect(isFilterOp("number", "startsWith")).toBe(false);
    expect(isFilterOp("number", "greater")).toBe(true);
    expect(isFilterOp("date", "within")).toBe(true);
    expect(isFilterOp("select", "within")).toBe(false);
    expect(isFilterOp("person", "notMe")).toBe(true);
    for (const op of ["startsWith", "endsWith", "lacksOption", "notRelatesTo", "lacksPerson", "onOrBefore", "onOrAfter", "within"]) {
      expect(opTakesValue(op), op).toBe(true);
    }
    expect(opTakesValue("notMe")).toBe(false);
    expect(opTakesValue("isSet")).toBe(false);
  });

  it("starts a new filter with the first operator, and \"within\" with a period (its select always shows one)", () => {
    expect(newFilter(def("p", "date"))).toEqual({ propertyId: "p", op: "equals" });
    expect(newFilter(def("p", "text"))).toEqual({ propertyId: "p", op: "contains" });
    expect(filterForOp("p", "startsWith")).toEqual({ propertyId: "p", op: "startsWith" });
    expect(filterForOp("p", "within")).toEqual({ propertyId: "p", op: "within", value: { relative: "thisWeek" } });
  });
});

// ----- texts -------------------------------------------------------------------------------------------------------------

describe("the text operators", () => {
  const pages = [
    page("hello", { text: text("Hello World") }),
    page("blank", { text: text("") }),
    page("none"),
    page("other", { text: text("Goodbye") }),
    page("spaced", { text: text("  Hello there  ") }),
  ];

  it("contains and does not contain: ASCII case-insensitive, the text to find trimmed, a page without a value listed by the second", () => {
    expect(passing(pages, filter("text", "contains", "  WORLD "))).toEqual(["hello"]);
    expect(passing(pages, filter("text", "notContains", "world"))).toEqual(["blank", "none", "other", "spaced"]);
    expect(passing(pages, filter("text", "contains", "zzz"))).toEqual([]);
    expect(passing(pages, filter("text", "notContains", "zzz"))).toEqual(["hello", "blank", "none", "other", "spaced"]);
  });

  it("is and is not: the whole text, ASCII case-insensitive, a page without a value listed by \"is not\"", () => {
    expect(passing(pages, filter("text", "equals", "HELLO world"))).toEqual(["hello"]);
    expect(passing(pages, filter("text", "notEquals", "hello world"))).toEqual(["blank", "none", "other", "spaced"]);
    expect(passing(pages, filter("text", "equals", "hello"))).toEqual([]);
  });

  it("starts with and ends with: the text to find trimmed, the text's own spaces ignored at the end that is looked at", () => {
    expect(passing(pages, filter("text", "startsWith", "hell"))).toEqual(["hello", "spaced"]);
    expect(passing(pages, filter("text", "startsWith", " GOOD "))).toEqual(["other"]);
    expect(passing(pages, filter("text", "startsWith", "world"))).toEqual([]);
    expect(passing(pages, filter("text", "endsWith", "WORLD"))).toEqual(["hello"]);
    expect(passing(pages, filter("text", "endsWith", "there"))).toEqual(["spaced"]);
    expect(passing(pages, filter("text", "endsWith", "hello"))).toEqual([]);
  });

  it("answers false (never true) to starts with and ends with for a page without text", () => {
    expect(check(page("none"), filter("text", "startsWith", "a"))).toBe(false);
    expect(check(page("blank", { text: text("") }), filter("text", "endsWith", "a"))).toBe(false);
  });

  it("works the same on a URL, an e-mail address and a phone number", () => {
    const url = page("u", { url: { kind: "url", value: "https://Example.com/docs" } });
    const email = page("e", { email: { kind: "email", value: "ada@example.com" } });
    const phone = page("p", { phone: { kind: "phone", value: "+33 1 23" } });
    expect(check(url, filter("url", "startsWith", "HTTPS://"))).toBe(true);
    expect(check(url, filter("url", "endsWith", ".org"))).toBe(false);
    expect(check(email, filter("email", "notContains", "grace"))).toBe(true);
    expect(check(email, filter("email", "notEquals", "ADA@example.com"))).toBe(false);
    expect(check(phone, filter("phone", "startsWith", "+33"))).toBe(true);
    expect(check(page("none"), filter("phone", "notContains", "1"))).toBe(true);
  });

  it("applies contains and does not contain to the name of a place", () => {
    const cafe = page("cafe", { place: { kind: "place", value: { name: "Café de Flore", lat: 1, lon: 2 } } });
    expect(check(cafe, filter("place", "contains", "FLORE"))).toBe(true);
    expect(check(cafe, filter("place", "notContains", "flore"))).toBe(false);
    expect(check(cafe, filter("place", "notContains", "paris"))).toBe(true);
    expect(check(page("none"), filter("place", "notContains", "paris"))).toBe(true);
  });

  it("cannot be applied without the text, with a blank one or with a value that is no text", () => {
    for (const op of ["contains", "notContains", "startsWith", "endsWith"] as const) {
      expect(check(pages[0] as PageInfo, filter("text", op)), op).toBeNull();
      expect(check(pages[0] as PageInfo, filter("text", op, "   ")), op).toBeNull();
      expect(check(pages[0] as PageInfo, filter("text", op, 4)), op).toBeNull();
    }
    for (const op of ["equals", "notEquals"] as const) {
      expect(check(pages[0] as PageInfo, filter("text", op)), op).toBeNull();
      expect(check(pages[0] as PageInfo, filter("text", op, "")), op).toBeNull();
      expect(check(pages[0] as PageInfo, filter("text", op, 4)), op).toBeNull();
    }
  });
});

// ----- numbers -----------------------------------------------------------------------------------------------------------

describe("the number operators", () => {
  const pages = [page("two", { num: num(2) }), page("three", { num: num(3) }), page("five", { num: num(5) }), page("zero", { num: num(0) }), page("none")];

  it("compares with the number, a zero being a value", () => {
    expect(passing(pages, filter("num", "equals", 3))).toEqual(["three"]);
    expect(passing(pages, filter("num", "greater", 3))).toEqual(["five"]);
    expect(passing(pages, filter("num", "less", 3))).toEqual(["two", "zero"]);
    expect(passing(pages, filter("num", "atLeast", 3))).toEqual(["three", "five"]);
    expect(passing(pages, filter("num", "atMost", 3))).toEqual(["two", "three", "zero"]);
    expect(passing(pages, filter("num", "equals", 0))).toEqual(["zero"]);
    expect(passing(pages, filter("num", "less", 1))).toEqual(["zero"]);
  });

  it("lists, for \"is not\", the pages that have another number, and not the ones without a number", () => {
    expect(passing(pages, filter("num", "notEquals", 3))).toEqual(["two", "five", "zero"]);
    expect(check(page("none"), filter("num", "notEquals", 3))).toBe(false);
    expect(check(page("none"), filter("num", "equals", 3))).toBe(false);
  });

  it("keeps a page without a number out of every comparison", () => {
    for (const op of ["greater", "less", "atLeast", "atMost"] as const) {
      expect(check(page("none"), filter("num", op, 1)), op).toBe(false);
    }
  });

  it("cannot be applied without a number (or with a text or a number that is not finite)", () => {
    for (const op of ["equals", "notEquals", "greater", "less", "atLeast", "atMost"] as const) {
      expect(check(pages[0] as PageInfo, filter("num", op)), op).toBeNull();
      expect(check(pages[0] as PageInfo, filter("num", op, "3")), op).toBeNull();
      expect(check(pages[0] as PageInfo, filter("num", op, Number.NaN)), op).toBeNull();
    }
  });

  it("compares a rollup that is a number the same way, and no other result", () => {
    const rollup = (value: number): PageInfo => page(`r${String(value)}`, { sum: { kind: "rollup", value: { function: "sum", result: { type: "number", data: value } } } });
    const rows = [rollup(1), rollup(7.5), page("none")];
    expect(passing(rows, filter("sum", "greater", 1))).toEqual(["r7.5"]);
    expect(passing(rows, filter("sum", "atMost", 1))).toEqual(["r1"]);
    expect(passing(rows, filter("sum", "notEquals", 7.5))).toEqual(["r1"]);
    expect(passing(rows, filter("sum", "equals", 7.5))).toEqual(["r7.5"]);
    const later = page("d", { sum: { kind: "rollup", value: { function: "sum", result: { type: "date", data: { date: TODAY } } } } });
    expect(check(later, filter("sum", "greater", 1))).toBe(false);
    expect(check(later, filter("sum", "notEquals", 1))).toBe(false);
    // The other rollups have no comparison.
    expect(check(later, filter("late", "greater", 1))).toBeNull();
  });
});

// ----- options, pages and people -----------------------------------------------------------------------------------------

describe("the \"not\" operators of options, pages and people", () => {
  it("does not have an option: a select or a status that is another one or none, a multi-select that does not list it", () => {
    const sel = [page("a", { sel: { kind: "select", value: "o-a" } }), page("b", { sel: { kind: "select", value: "o-b" } }), page("none")];
    expect(passing(sel, filter("sel", "lacksOption", "o-a"))).toEqual(["b", "none"]);
    expect(passing(sel, filter("sel", "hasOption", "o-a"))).toEqual(["a"]);
    const stat = [page("a", { stat: { kind: "status", value: "o-a" } }), page("none")];
    expect(passing(stat, filter("stat", "lacksOption", "o-a"))).toEqual(["none"]);
    const multi = [
      page("both", { multi: { kind: "multi_select", value: ["o-a", "o-b"] } }),
      page("b", { multi: { kind: "multi_select", value: ["o-b"] } }),
      page("empty", { multi: { kind: "multi_select", value: [] } }),
      page("none"),
    ];
    expect(passing(multi, filter("multi", "lacksOption", "o-a"))).toEqual(["b", "empty", "none"]);
    expect(passing(multi, filter("multi", "hasOption", "o-a"))).toEqual(["both"]);
  });

  it("does not link to a page: the pages whose relation lacks it or is empty", () => {
    const pages = [page("a", { rel: { kind: "relation", value: ["p-1", "p-2"] } }), page("b", { rel: { kind: "relation", value: ["p-2"] } }), page("none")];
    expect(passing(pages, filter("rel", "notRelatesTo", "p-1"))).toEqual(["b", "none"]);
    expect(passing(pages, filter("rel", "relatesTo", "p-1"))).toEqual(["a"]);
  });

  it("is not me and does not include a person: the pages without that person, those without people too", () => {
    const pages = [
      page("me", { who: { kind: "person", value: ["acc-me", "acc-ada"] } }),
      page("ada", { who: { kind: "person", value: ["acc-ada"] } }),
      page("none"),
    ];
    expect(passing(pages, filter("who", "isMe"), "acc-me")).toEqual(["me"]);
    expect(passing(pages, filter("who", "notMe"), "acc-me")).toEqual(["ada", "none"]);
    expect(passing(pages, filter("who", "hasPerson", "acc-ada"))).toEqual(["me", "ada"]);
    expect(passing(pages, filter("who", "lacksPerson", "acc-ada"))).toEqual(["none"]);
  });

  it("cannot be applied while who \"me\" is is not known, or without the option, page or person", () => {
    const p = page("a");
    expect(check(p, filter("who", "notMe"))).toBeNull();
    expect(check(p, filter("who", "isMe"))).toBeNull();
    expect(check(p, filter("sel", "lacksOption"))).toBeNull();
    expect(check(p, filter("sel", "lacksOption", ""))).toBeNull();
    expect(check(p, filter("rel", "notRelatesTo"))).toBeNull();
    expect(check(p, filter("who", "lacksPerson", 3))).toBeNull();
  });
});

// ----- days --------------------------------------------------------------------------------------------------------------

describe("the day operators on a date", () => {
  const pages = [
    page("old", { day: date("2026-10-01") }),
    page("yesterday", { day: date("2026-10-13") }),
    page("today", { day: date("2026-10-14", "23:30") }),
    page("tomorrow", { day: date("2026-10-15") }),
    page("later", { day: date("2026-11-01") }),
    page("none"),
  ];
  const on = (op: FilterOp, value: unknown, clock: FilterClock = SUNDAY): string[] => passing(pages, filter("day", op, value), undefined, clock);

  it("is on a day, the time of day not counting", () => {
    expect(on("equals", "2026-10-14")).toEqual(["today"]);
    expect(on("equals", "2026-10-02")).toEqual([]);
  });

  it("is before and after a day (the day itself excluded), on or before and on or after (included)", () => {
    expect(on("before", "2026-10-14")).toEqual(["old", "yesterday"]);
    expect(on("after", "2026-10-14")).toEqual(["tomorrow", "later"]);
    expect(on("onOrBefore", "2026-10-14")).toEqual(["old", "yesterday", "today"]);
    expect(on("onOrAfter", "2026-10-14")).toEqual(["today", "tomorrow", "later"]);
  });

  it("takes a relative day: today, yesterday and tomorrow, at the clock's day", () => {
    expect(on("equals", { relative: "today" })).toEqual(["today"]);
    expect(on("equals", { relative: "yesterday" })).toEqual(["yesterday"]);
    expect(on("equals", { relative: "tomorrow" })).toEqual(["tomorrow"]);
    expect(on("before", { relative: "today" })).toEqual(["old", "yesterday"]);
    expect(on("onOrAfter", { relative: "tomorrow" })).toEqual(["tomorrow", "later"]);
    expect(on("onOrBefore", { relative: "yesterday" })).toEqual(["old", "yesterday"]);
    expect(on("after", { relative: "yesterday" })).toEqual(["today", "tomorrow", "later"]);
    // Another day on the clock moves them; the first day of the week does not matter to a day.
    const later = { today: "2026-10-15", weekStart: 3 };
    expect(on("equals", { relative: "today" }, later)).toEqual(["tomorrow"]);
    expect(on("equals", { relative: "yesterday" }, later)).toEqual(["today"]);
  });

  it("resolves \"today\" over a month and a year end", () => {
    expect(resolveDay({ relative: "tomorrow" }, { today: "2026-12-31" })).toBe("2027-01-01");
    expect(resolveDay({ relative: "yesterday" }, { today: "2026-03-01" })).toBe("2026-02-28");
    expect(resolveDay({ relative: "yesterday" }, { today: "2028-03-01" })).toBe("2028-02-29");
    expect(resolveDay({ relative: "today" }, { today: "2026-10-14" })).toBe("2026-10-14");
  });

  it("uses the device's own day when the clock has none", () => {
    const now = page("now", { day: date(todayIso()) });
    expect(check(now, filter("day", "equals", { relative: "today" }))).toBe(true);
    expect(check(now, filter("day", "equals", { relative: "today" }), undefined, { today: "not a day" })).toBe(true);
    expect(check(now, filter("day", "equals", { relative: "tomorrow" }))).toBe(false);
  });

  it("keeps a page without a date out of every day operator", () => {
    for (const op of ["equals", "before", "after", "onOrBefore", "onOrAfter"] as const) {
      expect(check(page("none"), filter("day", op, "2026-10-14")), op).toBe(false);
      expect(check(page("none"), filter("day", op, { relative: "today" }), undefined, SUNDAY), op).toBe(false);
    }
  });

  it("cannot be applied without a day, with a text that is no day, or with a value of another shape (and is kept)", () => {
    const p = pages[2] as PageInfo;
    for (const op of ["equals", "before", "after", "onOrBefore", "onOrAfter"] as const) {
      for (const value of [undefined, "", "yesterday", "2026-13-40", "2026-02-30", 20261014, { relative: "someday" }, { relative: "thisWeek" }, { date: TODAY }, ["2026-10-14"]]) {
        expect(check(p, filter("day", op, value), undefined, SUNDAY), `${op} ${JSON.stringify(value)}`).toBeNull();
      }
    }
  });

  it("is between two days, inclusive, with one or both ends (relative days are not ends of a range)", () => {
    const between = (value: unknown): string[] => on("between", value);
    expect(between({ from: "2026-10-13", to: "2026-10-15" })).toEqual(["yesterday", "today", "tomorrow"]);
    expect(between({ from: "2026-10-14" })).toEqual(["today", "tomorrow", "later"]);
    expect(between({ to: "2026-10-13" })).toEqual(["old", "yesterday"]);
    expect(check(pages[0] as PageInfo, filter("day", "between", {}))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("day", "between", { from: { relative: "today" } }))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("day", "between", { from: "soon" }))).toBeNull();
  });
});

describe("the day operators on the created and edited times", () => {
  const pages = [
    page("early", { made: { kind: "created_time", value: at(2026, 10, 13, 0, 0) }, edited: { kind: "last_edited_time", value: at(2026, 10, 13, 23, 59) } }),
    page("midnight", { made: { kind: "created_time", value: at(2026, 10, 14, 0, 0) }, edited: { kind: "last_edited_time", value: at(2026, 10, 14, 0, 0) } }),
    page("night", { made: { kind: "created_time", value: at(2026, 10, 14, 23, 59) }, edited: { kind: "last_edited_time", value: at(2026, 10, 14, 23, 59) } }),
    page("next", { made: { kind: "created_time", value: at(2026, 10, 15, 0, 0) }, edited: { kind: "last_edited_time", value: at(2026, 10, 15, 0, 0) } }),
    page("none"),
  ];
  const on = (op: FilterOp, value: unknown, property = "made"): string[] => passing(pages, filter(property, op, value), undefined, SUNDAY);

  it("is on a day, from its first minute to its last (the viewer's own calendar)", () => {
    expect(on("equals", "2026-10-14")).toEqual(["midnight", "night"]);
    expect(on("equals", { relative: "today" })).toEqual(["midnight", "night"]);
    expect(on("equals", "2026-10-13", "edited")).toEqual(["early"]);
  });

  it("is before and after a day, on or before and on or after it", () => {
    expect(on("before", "2026-10-14")).toEqual(["early"]);
    expect(on("after", "2026-10-14")).toEqual(["next"]);
    expect(on("onOrBefore", "2026-10-14")).toEqual(["early", "midnight", "night"]);
    expect(on("onOrAfter", "2026-10-14")).toEqual(["midnight", "night", "next"]);
    expect(on("onOrBefore", { relative: "yesterday" })).toEqual(["early"]);
    expect(on("onOrAfter", { relative: "tomorrow" })).toEqual(["next"]);
  });

  it("is between two days, inclusive, and within a period", () => {
    expect(on("between", { from: "2026-10-14", to: "2026-10-14" })).toEqual(["midnight", "night"]);
    expect(on("within", { relative: "thisWeek" })).toEqual(["early", "midnight", "night", "next"]);
    expect(passing(pages, filter("made", "within", { relative: "nextDays", n: 1 }), undefined, SUNDAY)).toEqual(["midnight", "night", "next"]);
    expect(passing(pages, filter("made", "within", { relative: "pastDays", n: 1 }), undefined, SUNDAY)).toEqual(["early", "midnight", "night"]);
  });

  it("keeps a page without the time out, and cannot be applied to a day that is none", () => {
    expect(check(page("none"), filter("made", "equals", "2026-10-14"))).toBe(false);
    expect(check(page("none"), filter("made", "within", { relative: "thisWeek" }), undefined, SUNDAY)).toBe(false);
    expect(check(pages[0] as PageInfo, filter("made", "onOrBefore", "tomorrow"))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("made", "equals"))).toBeNull();
  });
});

describe("the day operators on a formula that gives a day", () => {
  const pages = [page("a", { "f-date": { kind: "formula", value: { type: "date", data: { date: "2026-10-14" } } } }), page("none")];

  it("takes a relative day for \"is\", \"before\" and \"after\", and keeps its operators", () => {
    expect(passing(pages, filter("f-date", "equals", { relative: "today" }), undefined, SUNDAY)).toEqual(["a"]);
    expect(passing(pages, filter("f-date", "before", { relative: "tomorrow" }), undefined, SUNDAY)).toEqual(["a"]);
    expect(passing(pages, filter("f-date", "after", { relative: "today" }), undefined, SUNDAY)).toEqual([]);
    expect(passing(pages, filter("f-date", "equals", "2026-10-14"))).toEqual(["a"]);
    expect(check(pages[0] as PageInfo, filter("f-date", "onOrBefore", "2026-10-14"))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("f-date", "within", { relative: "thisWeek" }))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("f-date", "equals", { relative: "someday" }))).toBeNull();
  });
});

// ----- periods -----------------------------------------------------------------------------------------------------------

describe("the periods of \"within\"", () => {
  const range = (relative: string, clock: FilterClock, n?: unknown): readonly [string, string] | null =>
    periodRange(n === undefined ? { relative } : { relative, n }, clock);

  it("is the week of the clock's first day: Sunday 11 to 17 October, or Monday 12 to 18 October", () => {
    expect(range("thisWeek", SUNDAY)).toEqual(["2026-10-11", "2026-10-17"]);
    expect(range("thisWeek", MONDAY)).toEqual(["2026-10-12", "2026-10-18"]);
    expect(range("lastWeek", SUNDAY)).toEqual(["2026-10-04", "2026-10-10"]);
    expect(range("lastWeek", MONDAY)).toEqual(["2026-10-05", "2026-10-11"]);
    expect(range("nextWeek", SUNDAY)).toEqual(["2026-10-18", "2026-10-24"]);
    expect(range("nextWeek", MONDAY)).toEqual(["2026-10-19", "2026-10-25"]);
  });

  it("starts the week on the day of the week the clock says, whatever today is", () => {
    // Sunday 11 October and Saturday 17 October, with a Sunday or a Monday start.
    expect(range("thisWeek", { today: "2026-10-11", weekStart: 0 })).toEqual(["2026-10-11", "2026-10-17"]);
    expect(range("thisWeek", { today: "2026-10-11", weekStart: 1 })).toEqual(["2026-10-05", "2026-10-11"]);
    expect(range("thisWeek", { today: "2026-10-17", weekStart: 0 })).toEqual(["2026-10-11", "2026-10-17"]);
    expect(range("thisWeek", { today: "2026-10-17", weekStart: 1 })).toEqual(["2026-10-12", "2026-10-18"]);
    // Monday 12 October itself, and a Saturday start.
    expect(range("thisWeek", { today: "2026-10-12", weekStart: 1 })).toEqual(["2026-10-12", "2026-10-18"]);
    expect(range("thisWeek", { today: "2026-10-12", weekStart: 6 })).toEqual(["2026-10-10", "2026-10-16"]);
    // A week across a month and a year end.
    expect(range("thisWeek", { today: "2026-12-31", weekStart: 1 })).toEqual(["2026-12-28", "2027-01-03"]);
    expect(range("nextWeek", { today: "2026-12-31", weekStart: 1 })).toEqual(["2027-01-04", "2027-01-10"]);
  });

  it("reads a first day of the week outside 0 to 6 as the week of that day (7 is Sunday)", () => {
    expect(range("thisWeek", { today: TODAY, weekStart: 7 })).toEqual(["2026-10-11", "2026-10-17"]);
    expect(range("thisWeek", { today: TODAY, weekStart: -6 })).toEqual(["2026-10-12", "2026-10-18"]);
  });

  it("is the calendar month, the last month and the next, over a year end and a leap February", () => {
    expect(range("thisMonth", SUNDAY)).toEqual(["2026-10-01", "2026-10-31"]);
    expect(range("lastMonth", SUNDAY)).toEqual(["2026-09-01", "2026-09-30"]);
    expect(range("nextMonth", SUNDAY)).toEqual(["2026-11-01", "2026-11-30"]);
    expect(range("lastMonth", { today: "2026-01-15" })).toEqual(["2025-12-01", "2025-12-31"]);
    expect(range("nextMonth", { today: "2026-12-20" })).toEqual(["2027-01-01", "2027-01-31"]);
    expect(range("lastMonth", { today: "2028-03-10" })).toEqual(["2028-02-01", "2028-02-29"]);
    expect(range("thisMonth", { today: "2026-02-28" })).toEqual(["2026-02-01", "2026-02-28"]);
  });

  it("counts the past and the next days from today, both ends included", () => {
    expect(range("pastDays", SUNDAY, 7)).toEqual(["2026-10-07", "2026-10-14"]);
    expect(range("nextDays", SUNDAY, 7)).toEqual(["2026-10-14", "2026-10-21"]);
    expect(range("pastDays", SUNDAY, 1)).toEqual(["2026-10-13", "2026-10-14"]);
    expect(range("nextDays", { today: "2026-12-30" }, 3)).toEqual(["2026-12-30", "2027-01-02"]);
    expect(range("pastDays", SUNDAY, 365)).toEqual(["2025-10-14", "2026-10-14"]);
  });

  it("is not a period without its count (an integer from 1 to 365) for the past and the next days", () => {
    for (const relative of ["pastDays", "nextDays"]) {
      for (const n of [undefined, 0, -1, 366, 1.5, "7", null, Number.NaN]) {
        expect(range(relative, SUNDAY, n === undefined ? undefined : n), `${relative} ${String(n)}`).toBeNull();
      }
    }
    // The other periods ignore a count.
    expect(range("thisWeek", SUNDAY, 400)).toEqual(["2026-10-11", "2026-10-17"]);
  });

  it("is not a period for anything else", () => {
    for (const value of [undefined, null, "thisWeek", 7, [], {}, { relative: "today" }, { relative: "thisYear" }, { n: 7 }]) {
      expect(periodRange(value, SUNDAY), JSON.stringify(value)).toBeNull();
    }
  });

  it("reads a period and a relative day leniently (a count kept only for the days periods and only when usable)", () => {
    expect(readRelativePeriod({ relative: "pastDays", n: 7 })).toEqual({ relative: "pastDays", n: 7 });
    expect(readRelativePeriod({ relative: "pastDays" })).toEqual({ relative: "pastDays" });
    expect(readRelativePeriod({ relative: "nextDays", n: 400 })).toEqual({ relative: "nextDays" });
    expect(readRelativePeriod({ relative: "thisWeek", n: 5 })).toEqual({ relative: "thisWeek" });
    expect(readRelativePeriod({ relative: "later" })).toBeNull();
    expect(readRelativePeriod("thisWeek")).toBeNull();
    expect(readRelativeDay({ relative: "today" })).toBe("today");
    expect(readRelativeDay({ relative: "thisWeek" })).toBeNull();
    expect(readRelativeDay("today")).toBeNull();
  });
});

describe("\"within\" on a date", () => {
  const pages = [
    page("before", { day: date("2026-10-10") }),
    page("sunday", { day: date("2026-10-11") }),
    page("monday", { day: date("2026-10-12") }),
    page("today", { day: date("2026-10-14", "10:00") }),
    page("saturday", { day: date("2026-10-17") }),
    page("nextSunday", { day: date("2026-10-18") }),
    page("none"),
  ];
  const within = (value: unknown, clock: FilterClock): string[] => passing(pages, filter("day", "within", value), undefined, clock);

  it("lists the week from its first to its last day, which depends on the first day of the week", () => {
    expect(within({ relative: "thisWeek" }, SUNDAY)).toEqual(["sunday", "monday", "today", "saturday"]);
    expect(within({ relative: "thisWeek" }, MONDAY)).toEqual(["monday", "today", "saturday", "nextSunday"]);
    expect(within({ relative: "lastWeek" }, SUNDAY)).toEqual(["before"]);
    expect(within({ relative: "nextWeek" }, SUNDAY)).toEqual(["nextSunday"]);
    expect(within({ relative: "nextWeek" }, MONDAY)).toEqual([]);
  });

  it("lists the past and the next days with both bounds in", () => {
    expect(within({ relative: "pastDays", n: 4 }, SUNDAY)).toEqual(["before", "sunday", "monday", "today"]);
    expect(within({ relative: "pastDays", n: 3 }, SUNDAY)).toEqual(["sunday", "monday", "today"]);
    expect(within({ relative: "nextDays", n: 3 }, SUNDAY)).toEqual(["today", "saturday"]);
    expect(within({ relative: "nextDays", n: 2 }, SUNDAY)).toEqual(["today"]);
    expect(within({ relative: "nextDays", n: 4 }, SUNDAY)).toEqual(["today", "saturday", "nextSunday"]);
  });

  it("lists the month", () => {
    expect(within({ relative: "thisMonth" }, SUNDAY)).toEqual(["before", "sunday", "monday", "today", "saturday", "nextSunday"]);
    expect(within({ relative: "lastMonth" }, SUNDAY)).toEqual([]);
    expect(within({ relative: "nextMonth" }, { today: "2026-09-20" })).toEqual(["before", "sunday", "monday", "today", "saturday", "nextSunday"]);
  });

  it("never lists a page without a date, and cannot be applied to a value that is no period", () => {
    expect(check(page("none"), filter("day", "within", { relative: "thisMonth" }), undefined, SUNDAY)).toBe(false);
    expect(check(pages[0] as PageInfo, filter("day", "within", "thisWeek"))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("day", "within", { relative: "pastDays" }))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("day", "within"))).toBeNull();
    expect(check(pages[0] as PageInfo, filter("day", "within", { relative: "today" }))).toBeNull();
  });
});
