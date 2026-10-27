// A formula property in the views (ADR-0055, `docs/architecture/formulas.md` section 9): what a result looks like in a cell, the
// filters by the type of the result, the sort by the result, and the footer's calculations by the type. The pages are built
// directly (`PageInfo` carrying `{ kind: "formula", value }`, as the core's values come), so nothing here needs the engine.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { FormulaResult, FormulaType, PageId, PageInfo, PropertyDef, PropertyValue } from "@atomik-kb/core-client";
import { render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, onTestFinished } from "vitest";

import { formatNumber } from "../property-format";
import { t } from "../i18n";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { formatDateMention } from "./dates";
import { applyFilterTree, matchFilterGroup } from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import { applyFilters, filterOps, isFilterOp, matchFilter, newFilter, opTakesValue } from "./page-list-filters";
import type { FilterOp, PageListFilter } from "./page-list-filters";
import { sortPages } from "./page-list-model";
import { calculate, calculationFor, calculationFunctionsFor, formatCalculation } from "./page-table-calculations";
import type { CalculationFunction } from "./page-table-calculations";
import { FormulaValue, PageListValue } from "./PageListValue";

// ----- fixtures -----------------------------------------------------------------------------------------------------------

function formulaDef(id: string, name: string, type: FormulaType | undefined): PropertyDef {
  return {
    id,
    name,
    kind: "formula",
    options: [],
    createdAt: 0,
    config: type === undefined ? { kind: "formula", expression: `prop("x")` } : { kind: "formula", expression: `prop("x")`, resultType: type },
  };
}

const NUMBER = formulaDef("f-number", "Total", { base: "number", list: false });
const TEXT = formulaDef("f-text", "Label", { base: "text", list: false });
const BOOLEAN = formulaDef("f-boolean", "Late", { base: "boolean", list: false });
const DATE = formulaDef("f-date", "Due soon", { base: "date", list: false });
const LIST = formulaDef("f-list", "Words", { base: "text", list: true });
const PAGES = formulaDef("f-pages", "Linked", { base: "page", list: true });
const PERSON = formulaDef("f-person", "Owner", { base: "person", list: false });
const BROKEN = formulaDef("f-broken", "Broken", undefined);
const PLAIN_NUMBER: PropertyDef = { id: "p-number", name: "Plain", kind: "number", options: [], createdAt: 0 };

const DEFS: readonly PropertyDef[] = [NUMBER, TEXT, BOOLEAN, DATE, LIST, PAGES, PERSON, BROKEN, PLAIN_NUMBER];

const num = (data: number): FormulaResult => ({ type: "number", data });
const str = (data: string): FormulaResult => ({ type: "text", data });
const bool = (data: boolean): FormulaResult => ({ type: "boolean", data });
const day = (date: string, time?: string): FormulaResult => ({ type: "date", data: time === undefined ? { date } : { date, time } });
const items = (...data: FormulaResult[]): FormulaResult => ({ type: "list", data });
const words = (...data: string[]): FormulaResult => items(...data.map(str));

/** A page with the results of formulas by property id (a property left out has no value on the page). */
function page(title: string, results: Record<string, FormulaResult> = {}, extra: Record<string, PropertyValue> = {}): PageInfo {
  return {
    id: title,
    spaceId: "s",
    title,
    createdAt: 0,
    updatedAt: 0,
    properties: [
      ...Object.entries(results).map(([id, value]): [string, PropertyValue] => [id, { kind: "formula", value }]),
      ...Object.entries(extra),
    ],
  };
}

/** Titles of the pages that pass one filter. */
function passing(pages: readonly PageInfo[], filter: PageListFilter, lookups = {}): string[] {
  return applyFilters(pages, [filter], DEFS, undefined, lookups).map((entry) => entry.title);
}

const filter = (def: PropertyDef, op: FilterOp, value?: unknown): PageListFilter =>
  value === undefined ? { propertyId: def.id, op } : { propertyId: def.id, op, value };

// ----- which conditions --------------------------------------------------------------------------------------------------

describe("the conditions of a formula", () => {
  it("are comparisons for a number", () => {
    expect(filterOps("formula", NUMBER)).toEqual(["equals", "notEquals", "greater", "less", "atLeast", "atMost", "isSet", "isEmpty"]);
  });

  it("are is, is not, contains and does not contain for a text", () => {
    expect(filterOps("formula", TEXT)).toEqual(["equals", "notEquals", "contains", "notContains", "isSet", "isEmpty"]);
  });

  it("are checked and not checked for a boolean", () => {
    expect(filterOps("formula", BOOLEAN)).toEqual(["checked", "unchecked"]);
  });

  it("are is, before, after and between for a date, and empty or not", () => {
    expect(filterOps("formula", DATE)).toEqual(["equals", "before", "after", "between", "isSet", "isEmpty"]);
  });

  it("are contains and empty or not for a list, a page and a person", () => {
    for (const def of [LIST, PAGES, PERSON]) {
      expect(filterOps("formula", def), def.name).toEqual(["contains", "isSet", "isEmpty"]);
    }
  });

  it("are empty and not empty for a formula that does not check, and when no definition is given", () => {
    expect(filterOps("formula", BROKEN)).toEqual(["isSet", "isEmpty"]);
    expect(filterOps("formula")).toEqual(["isSet", "isEmpty"]);
  });

  it("start a new filter on the first condition of the type, and know which take a value", () => {
    expect(newFilter(NUMBER)).toEqual({ propertyId: NUMBER.id, op: "equals" });
    expect(newFilter(BOOLEAN)).toEqual({ propertyId: BOOLEAN.id, op: "checked" });
    for (const op of ["notEquals", "greater", "less", "atLeast", "atMost", "notContains"]) {
      expect(opTakesValue(op), op).toBe(true);
    }
    for (const op of ["isSet", "isEmpty", "checked", "unchecked"]) {
      expect(opTakesValue(op), op).toBe(false);
    }
  });

  it("are not conditions of another kind (a text property has no \"is greater than\")", () => {
    expect(isFilterOp("text", "greater", PLAIN_NUMBER)).toBe(false);
    expect(isFilterOp("date", "contains", PLAIN_NUMBER)).toBe(false);
    expect(isFilterOp("formula", "greater", NUMBER)).toBe(true);
    expect(isFilterOp("formula", "greater", TEXT)).toBe(false);
  });
});

// ----- filtering ---------------------------------------------------------------------------------------------------------

describe("filtering by a number result", () => {
  const pages = [page("Low", { "f-number": num(2) }), page("Mid", { "f-number": num(5) }), page("High", { "f-number": num(12) }), page("None")];

  it("compares with the number", () => {
    expect(passing(pages, filter(NUMBER, "equals", 5))).toEqual(["Mid"]);
    expect(passing(pages, filter(NUMBER, "notEquals", 5))).toEqual(["Low", "High", "None"]);
    expect(passing(pages, filter(NUMBER, "greater", 5))).toEqual(["High"]);
    expect(passing(pages, filter(NUMBER, "less", 5))).toEqual(["Low"]);
    expect(passing(pages, filter(NUMBER, "atLeast", 5))).toEqual(["Mid", "High"]);
    expect(passing(pages, filter(NUMBER, "atMost", 5))).toEqual(["Low", "Mid"]);
  });

  it("keeps a page without a result out of every comparison but \"is not\" and \"is empty\"", () => {
    expect(matchFilter(pages[3] as PageInfo, filter(NUMBER, "greater", -100), DEFS)).toBe(false);
    expect(matchFilter(pages[3] as PageInfo, filter(NUMBER, "less", 100), DEFS)).toBe(false);
    expect(passing(pages, filter(NUMBER, "isEmpty"))).toEqual(["None"]);
    expect(passing(pages, filter(NUMBER, "isSet"))).toEqual(["Low", "Mid", "High"]);
  });

  it("ignores a comparison that has no number yet (and keeps it)", () => {
    for (const op of ["equals", "notEquals", "greater", "less", "atLeast", "atMost"] as const) {
      expect(matchFilter(pages[0] as PageInfo, filter(NUMBER, op), DEFS), op).toBeNull();
      expect(matchFilter(pages[0] as PageInfo, filter(NUMBER, op, "five"), DEFS), op).toBeNull();
    }
    expect(passing(pages, filter(NUMBER, "greater"))).toHaveLength(4);
  });

  it("is a condition of the filter tree like any other, with OR", () => {
    const group: FilterGroup = { op: "or", items: [filter(NUMBER, "less", 3), filter(NUMBER, "greater", 10)] };
    expect(applyFilterTree(pages, group, { defs: DEFS }).map((entry) => entry.title)).toEqual(["Low", "High"]);
    expect(matchFilterGroup(pages[1] as PageInfo, group, { defs: DEFS })).toBe(false);
  });
});

describe("filtering by a text result", () => {
  const pages = [page("A", { "f-text": str("Draft copy") }), page("B", { "f-text": str("Final") }), page("C")];

  it("compares ignoring case, and contains", () => {
    expect(passing(pages, filter(TEXT, "equals", "final"))).toEqual(["B"]);
    expect(passing(pages, filter(TEXT, "notEquals", "final"))).toEqual(["A", "C"]);
    expect(passing(pages, filter(TEXT, "contains", " COPY "))).toEqual(["A"]);
    expect(passing(pages, filter(TEXT, "notContains", "copy"))).toEqual(["B", "C"]);
  });

  it("is empty or not", () => {
    expect(passing(pages, filter(TEXT, "isEmpty"))).toEqual(["C"]);
    expect(passing(pages, filter(TEXT, "isSet"))).toEqual(["A", "B"]);
  });

  it("ignores a text condition that has no text yet", () => {
    expect(matchFilter(pages[0] as PageInfo, filter(TEXT, "contains", "  "), DEFS)).toBeNull();
    expect(matchFilter(pages[0] as PageInfo, filter(TEXT, "equals", ""), DEFS)).toBeNull();
  });
});

describe("filtering by a boolean result", () => {
  const pages = [page("Late", { "f-boolean": bool(true) }), page("OnTime", { "f-boolean": bool(false) }), page("Unknown")];

  it("is checked or not checked", () => {
    expect(passing(pages, filter(BOOLEAN, "checked"))).toEqual(["Late"]);
    expect(passing(pages, filter(BOOLEAN, "unchecked"))).toEqual(["OnTime", "Unknown"]);
  });
});

describe("filtering by a date result", () => {
  const pages = [
    page("Early", { "f-date": day("2026-10-01") }),
    page("Mid", { "f-date": day("2026-10-15", "09:30") }),
    page("Late", { "f-date": day("2026-11-01") }),
    page("None"),
  ];

  it("is a day, before a day, after a day, or between two", () => {
    expect(passing(pages, filter(DATE, "equals", "2026-10-15"))).toEqual(["Mid"]);
    expect(passing(pages, filter(DATE, "before", "2026-10-15"))).toEqual(["Early"]);
    expect(passing(pages, filter(DATE, "after", "2026-10-15"))).toEqual(["Late"]);
    expect(passing(pages, filter(DATE, "between", { from: "2026-10-01", to: "2026-10-31" }))).toEqual(["Early", "Mid"]);
    expect(passing(pages, filter(DATE, "between", { from: "2026-10-10" }))).toEqual(["Mid", "Late"]);
  });

  it("is empty or not", () => {
    expect(passing(pages, filter(DATE, "isEmpty"))).toEqual(["None"]);
    expect(passing(pages, filter(DATE, "isSet"))).toEqual(["Early", "Mid", "Late"]);
  });

  it("ignores a range with neither end", () => {
    expect(matchFilter(pages[0] as PageInfo, filter(DATE, "between", {}), DEFS)).toBeNull();
  });
});

describe("filtering by a list, a page or a person", () => {
  const pages = [
    page("Both", { "f-list": words("red", "Green"), "f-pages": items({ type: "page", data: "p-1" }, { type: "page", data: "p-2" }), "f-person": { type: "person", data: "acc-1" } }),
    page("Red", { "f-list": words("red") }),
    page("Nothing"),
  ];
  const lookups = {
    pageTitle: (id: PageId): string | null => (id === "p-1" ? "Roadmap" : id === "p-2" ? "Budget" : null),
    personName: (id: string): string => (id === "acc-1" ? "Ada" : ""),
  };

  it("contains a text in any item, ignoring case", () => {
    expect(passing(pages, filter(LIST, "contains", "green"))).toEqual(["Both"]);
    expect(passing(pages, filter(LIST, "contains", "RED"))).toEqual(["Both", "Red"]);
    expect(passing(pages, filter(LIST, "contains", "blue"))).toEqual([]);
  });

  it("reads the titles of pages and the names of people through the lookups", () => {
    expect(passing(pages, filter(PAGES, "contains", "budget"), lookups)).toEqual(["Both"]);
    expect(passing(pages, filter(PERSON, "contains", "ada"), lookups)).toEqual(["Both"]);
    // Without lookups those items have no text.
    expect(passing(pages, filter(PAGES, "contains", "budget"))).toEqual([]);
  });

  it("gets the lookups from the filter tree's context", () => {
    const group: FilterGroup = { op: "and", items: [filter(PAGES, "contains", "road")] };
    expect(applyFilterTree(pages, group, { defs: DEFS, lookups }).map((entry) => entry.title)).toEqual(["Both"]);
  });

  it("is empty or not", () => {
    expect(passing(pages, filter(LIST, "isEmpty"))).toEqual(["Nothing"]);
    expect(passing(pages, filter(LIST, "isSet"))).toEqual(["Both", "Red"]);
  });
});

describe("filtering by a formula that does not check", () => {
  it("has no value on any page, so it is empty, and any other condition is ignored", () => {
    const pages = [page("A", { "f-number": num(1) })];
    expect(passing(pages, filter(BROKEN, "isEmpty"))).toEqual(["A"]);
    expect(passing(pages, filter(BROKEN, "isSet"))).toEqual([]);
    expect(matchFilter(pages[0] as PageInfo, filter(BROKEN, "equals", 1), DEFS)).toBeNull();
  });

  it("is ignored when a formula changed type under a stored condition", () => {
    // "is greater than" was stored for a number; the formula now gives a text.
    expect(matchFilter(page("A", { "f-text": str("x") }), filter(TEXT, "greater", 1), DEFS)).toBeNull();
  });
});

// ----- sorting -----------------------------------------------------------------------------------------------------------

describe("sorting by a formula's result", () => {
  const order = (pages: readonly PageInfo[], by: string, dir: "asc" | "desc" = "asc", lookups = {}): string[] =>
    sortPages(pages, [{ by, dir }], DEFS, lookups).map((entry) => entry.title);

  it("orders numbers numerically, the pages without a result last in both directions", () => {
    const pages = [page("Ten", { "f-number": num(10) }), page("None"), page("Two", { "f-number": num(2) }), page("Five", { "f-number": num(5) })];
    expect(order(pages, "f-number")).toEqual(["Two", "Five", "Ten", "None"]);
    expect(order(pages, "f-number", "desc")).toEqual(["Ten", "Five", "Two", "None"]);
  });

  it("orders texts ignoring ASCII case", () => {
    const pages = [page("1", { "f-text": str("banana") }), page("2", { "f-text": str("Apple") }), page("3", { "f-text": str("cherry") })];
    expect(order(pages, "f-text")).toEqual(["2", "1", "3"]);
  });

  it("puts false before true", () => {
    const pages = [page("Yes", { "f-boolean": bool(true) }), page("No", { "f-boolean": bool(false) })];
    expect(order(pages, "f-boolean")).toEqual(["No", "Yes"]);
    expect(order(pages, "f-boolean", "desc")).toEqual(["Yes", "No"]);
  });

  it("orders dates by their ISO text, a day before the same day with a time", () => {
    const pages = [page("C", { "f-date": day("2026-10-02") }), page("B", { "f-date": day("2026-10-01", "08:00") }), page("A", { "f-date": day("2026-10-01") })];
    expect(order(pages, "f-date")).toEqual(["A", "B", "C"]);
  });

  it("orders a list by its items joined, and pages and people by the lookups", () => {
    const lists = [page("Z", { "f-list": words("zeta") }), page("A", { "f-list": words("alpha", "beta") })];
    expect(order(lists, "f-list")).toEqual(["A", "Z"]);
    const linked = [
      page("Second", { "f-pages": items({ type: "page", data: "p-b" }) }),
      page("First", { "f-pages": items({ type: "page", data: "p-a" }) }),
    ];
    const titles = (id: string): string | null => (id === "p-a" ? "Alpha" : id === "p-b" ? "Beta" : null);
    expect(order(linked, "f-pages", "asc", { pageTitle: titles })).toEqual(["First", "Second"]);
    const owners = [page("Bo", { "f-person": { type: "person", data: "b" } }), page("Al", { "f-person": { type: "person", data: "a" } })];
    expect(order(owners, "f-person", "asc", { personName: (id: string) => (id === "a" ? "Ada" : "Bob") })).toEqual(["Al", "Bo"]);
  });

  it("keeps the order of pages that tie", () => {
    const pages = [page("B", { "f-number": num(1) }), page("A", { "f-number": num(1) })];
    expect(order(pages, "f-number")).toEqual(["B", "A"]);
  });
});

// ----- calculations -------------------------------------------------------------------------------------------------------

const COUNTS: CalculationFunction[] = ["count_all", "count_values", "count_unique", "count_empty", "count_not_empty", "percent_empty", "percent_not_empty"];

describe("the calculations of a formula column", () => {
  it("are the number functions for a number result, the date functions for a date, the checkbox functions for a boolean", () => {
    expect(calculationFunctionsFor(NUMBER)).toEqual(["sum", "average", "median", "min", "max", "range", ...COUNTS]);
    expect(calculationFunctionsFor(DATE)).toEqual(["earliest", "latest", "date_range", ...COUNTS]);
    expect(calculationFunctionsFor(BOOLEAN)).toEqual(["count_checked", "count_unchecked", "percent_checked", ...COUNTS]);
  });

  it("are the counts only for a text, a list, a page, a person and a formula that does not check", () => {
    for (const def of [TEXT, LIST, PAGES, PERSON, BROKEN]) {
      expect(calculationFunctionsFor(def), def.name).toEqual(COUNTS);
    }
  });

  it("are the same as a plain property of that kind offers", () => {
    expect(calculationFunctionsFor(NUMBER)).toEqual(calculationFunctionsFor(PLAIN_NUMBER));
  });

  it("add up, average and range the numbers", () => {
    const rows = [page("A", { "f-number": num(2) }), page("B", { "f-number": num(4) }), page("C", { "f-number": num(9) }), page("D")];
    expect(calculate("sum", NUMBER, rows)).toEqual({ type: "number", data: 15 });
    expect(calculate("average", NUMBER, rows)).toEqual({ type: "number", data: 5 });
    expect(calculate("median", NUMBER, rows)).toEqual({ type: "number", data: 4 });
    expect(calculate("min", NUMBER, rows)).toEqual({ type: "number", data: 2 });
    expect(calculate("max", NUMBER, rows)).toEqual({ type: "number", data: 9 });
    expect(calculate("range", NUMBER, rows)).toEqual({ type: "number", data: 7 });
    expect(calculate("count_all", NUMBER, rows)).toEqual({ type: "number", data: 4 });
    expect(calculate("count_empty", NUMBER, rows)).toEqual({ type: "number", data: 1 });
    expect(calculate("count_not_empty", NUMBER, rows)).toEqual({ type: "number", data: 3 });
  });

  it("agree with a plain number property on the same numbers", () => {
    const values = [2, 4, 9];
    const formulaRows = values.map((value, at) => page(String(at), { "f-number": num(value) }));
    const plainRows = values.map((value, at) => page(String(at), {}, { "p-number": { kind: "number", value } }));
    for (const fn of ["sum", "average", "median", "min", "max", "range"] as const) {
      expect(calculate(fn, NUMBER, formulaRows), fn).toEqual(calculate(fn, PLAIN_NUMBER, plainRows));
    }
  });

  it("find the earliest and latest day and the days between", () => {
    const rows = [page("A", { "f-date": day("2026-10-05") }), page("B", { "f-date": day("2026-10-01") }), page("C", { "f-date": day("2026-10-20") }), page("D")];
    expect(calculate("earliest", DATE, rows)).toEqual({ type: "date", data: { date: "2026-10-01" } });
    expect(calculate("latest", DATE, rows)).toEqual({ type: "date", data: { date: "2026-10-20" } });
    expect(calculate("date_range", DATE, rows)).toEqual({ type: "days", data: 19 });
  });

  it("count the boxes of a boolean", () => {
    const rows = [page("A", { "f-boolean": bool(true) }), page("B", { "f-boolean": bool(false) }), page("C", { "f-boolean": bool(true) }), page("D", { "f-boolean": bool(false) })];
    expect(calculate("count_checked", BOOLEAN, rows)).toEqual({ type: "number", data: 2 });
    expect(calculate("count_unchecked", BOOLEAN, rows)).toEqual({ type: "number", data: 2 });
    expect(calculate("percent_checked", BOOLEAN, rows)).toEqual({ type: "number", data: 50 });
  });

  it("count the items of a list and the distinct texts of a text", () => {
    const lists = [page("A", { "f-list": words("x", "y") }), page("B", { "f-list": words("y") }), page("C")];
    expect(calculate("count_values", LIST, lists)).toEqual({ type: "number", data: 3 });
    expect(calculate("count_unique", LIST, lists)).toEqual({ type: "number", data: 2 });
    expect(calculate("count_not_empty", LIST, lists)).toEqual({ type: "number", data: 2 });
    const texts = [page("A", { "f-text": str("x") }), page("B", { "f-text": str("x") }), page("C", { "f-text": str("y") })];
    expect(calculate("count_unique", TEXT, texts)).toEqual({ type: "number", data: 2 });
  });

  it("are kept for a formula only while the type offers them", () => {
    expect(calculationFor({ "f-number": "sum" }, "f-number", NUMBER)).toBe("sum");
    // The formula's result became a text: a stored "sum" is no longer offered.
    expect(calculationFor({ "f-text": "sum" }, "f-text", TEXT)).toBeNull();
  });

  it("are written like a plain number's", () => {
    const result = calculate("sum", NUMBER, [page("A", { "f-number": num(1234.5) })]);
    expect(result).toBeDefined();
    if (result !== undefined) {
      expect(formatCalculation("sum", result, "2026-10-18")).toBe(formatNumber(1234.5));
    }
  });
});

// ----- what a result looks like -------------------------------------------------------------------------------------------

/** A store over a space with pages, started; the pages' ids by title. */
async function mountStore(titles: string[]) {
  const memory = new MemoryTransport({
    now: () => 0,
    workspaceName: "W",
    spaces: [{ name: "S", pages: titles.map((title) => ({ title })) }],
  });
  const store = createAppStore(new CoreClient(memory));
  onTestFinished(store.getState().start());
  await waitFor(() => {
    expect(store.getState().tree).not.toBeNull();
  });
  const ids = new Map<string, PageId>();
  for (const node of store.getState().tree?.roots ?? []) {
    ids.set(node.page.title, node.page.id);
  }
  return { store, ids };
}

describe("a result in a cell", () => {
  const TODAY = "2026-10-18";

  it("is a number as the locale writes it", () => {
    render(<FormulaValue result={num(1234.5)} today={TODAY} />);
    expect(screen.getByText(formatNumber(1234.5))).toBeInTheDocument();
  });

  it("is a text, cut to one line with the whole text as its tooltip", () => {
    render(<FormulaValue result={str(`${"long ".repeat(40)}end`)} today={TODAY} />);
    const cell = document.querySelector(".page-list-text");
    expect(cell?.textContent?.endsWith("…")).toBe(true);
    expect(cell).toHaveAttribute("title", expect.stringContaining("end"));
  });

  it("is a boolean as a box, checked or not, that cannot be changed", () => {
    const { unmount } = render(<FormulaValue result={bool(true)} today={TODAY} />);
    expect(screen.getByRole("img", { name: t("pageList.checked") })).toHaveAttribute("data-checked", "true");
    unmount();
    render(<FormulaValue result={bool(false)} today={TODAY} />);
    expect(screen.getByRole("img", { name: t("pageList.unchecked") })).toHaveAttribute("data-checked", "false");
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("is a date as a date property shows it", () => {
    render(<FormulaValue result={day("2026-10-19", "14:30")} today={TODAY} />);
    expect(screen.getByText(formatDateMention("2026-10-19", "14:30", TODAY))).toBeInTheDocument();
  });

  it("is a list as one chip per item", () => {
    render(<FormulaValue result={items(str("red"), num(3), day("2026-10-19"), bool(true))} today={TODAY} />);
    const chips = [...document.querySelectorAll(".rollup-item")].map((chip) => chip.textContent);
    expect(chips).toEqual(["red", formatNumber(3), formatDateMention("2026-10-19", null, TODAY), t("properties.rollup.yes")]);
  });

  it("is a page as a chip with the page's title from the tree", async () => {
    const { store, ids } = await mountStore(["Roadmap", "Budget"]);
    render(
      <StoreContext value={store}>
        <FormulaValue result={{ type: "page", data: ids.get("Roadmap") ?? "" }} today={TODAY} />
        <FormulaValue result={items({ type: "page", data: ids.get("Budget") ?? "" })} today={TODAY} />
      </StoreContext>,
    );
    expect(await screen.findByText("Roadmap")).toBeInTheDocument();
    expect(screen.getByText("Budget")).toBeInTheDocument();
    expect(document.querySelectorAll(".relation-chip.is-static")).toHaveLength(2);
  });

  it("is a person as a chip", async () => {
    const { store } = await mountStore(["Home"]);
    render(
      <StoreContext value={store}>
        <FormulaValue result={{ type: "person", data: "acc-1" }} today={TODAY} />
      </StoreContext>,
    );
    expect(document.querySelector(".person-chip.is-static")).not.toBeNull();
  });

  it("is the panel's variant: the whole text, and a box that is checked and disabled", () => {
    const { unmount } = render(<FormulaValue result={str("whole text")} today={TODAY} variant="panel" />);
    expect(document.querySelector(".properties-text")?.textContent).toBe("whole text");
    unmount();
    render(<FormulaValue result={bool(true)} today={TODAY} variant="panel" />);
    const box = screen.getByRole("checkbox", { name: t("pageList.checked") });
    expect(box).toBeChecked();
    expect(box).toBeDisabled();
  });

  it("is drawn by the view's value component for a formula value of any property", async () => {
    const { store } = await mountStore(["Home"]);
    render(
      <StoreContext value={store}>
        <PageListValue def={NUMBER} value={{ kind: "formula", value: num(7) }} today={TODAY} />
        <PageListValue def={TEXT} value={{ kind: "formula", value: str("hello") }} today={TODAY} />
      </StoreContext>,
    );
    const cells = screen.getAllByText(/^(7|hello)$/u);
    expect(cells.map((cell) => cell.textContent)).toEqual(["7", "hello"]);
    expect(within(document.body).queryByRole("textbox")).not.toBeInTheDocument();
  });
});
