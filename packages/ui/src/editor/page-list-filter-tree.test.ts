// The filter tree of a view (ADR-0054): the shapes, the lenient reading and the limits, the evaluation (AND, OR, an
// empty group, a filter that cannot be applied, one level), the flat list of an older block, the mirror an older
// build reads, and the editing operations of the filter editor. Pure: no editor, no store.

import type { PageInfo, PropertyDef, PropertyValue } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import {
  EMPTY_FILTER_GROUP,
  MAX_FILTERS,
  MAX_FILTER_GROUPS,
  addFilter,
  addGroup,
  applyFilterTree,
  canAddFilter,
  canAddGroup,
  countFilters,
  countGroups,
  isEmptyFilterGroup,
  isFilterGroup,
  legacyFilters,
  legacyFiltersToGroup,
  matchFilterGroup,
  normaliseFilterGroup,
  plainFilters,
  removeAt,
  replaceFilter,
  setGroupOp,
} from "./page-list-filter-tree";
import type { FilterContext, FilterGroup, FilterNode } from "./page-list-filter-tree";
import { applyFilters } from "./page-list-filters";
import type { PageListFilter } from "./page-list-filters";

const STATUS = "p-status";
const PRIORITY = "p-priority";
const COST = "p-cost";
const GONE = "p-gone";

const defs: readonly PropertyDef[] = [
  {
    id: STATUS,
    name: "Status",
    kind: "select",
    options: [
      { id: "todo", name: "To do", color: "gray" },
      { id: "done", name: "Done", color: "green" },
    ],
    createdAt: 0,
  },
  {
    id: PRIORITY,
    name: "Priority",
    kind: "select",
    options: [
      { id: "low", name: "Low", color: "gray" },
      { id: "high", name: "High", color: "red" },
    ],
    createdAt: 0,
  },
  { id: COST, name: "Cost", kind: "number", options: [], createdAt: 0 },
];

const context: FilterContext = { defs };

function page(id: string, values: [string, PropertyValue][] = []): PageInfo {
  return { id, spaceId: "space", title: id, createdAt: 0, updatedAt: 0, ...(values.length === 0 ? {} : { properties: values }) };
}

const select = (id: string, option: string): [string, PropertyValue] => [id, { kind: "select", value: option }];

// A small space: every combination of status and priority, and a page with neither.
const pages: readonly PageInfo[] = [
  page("a", [select(STATUS, "todo"), select(PRIORITY, "high")]),
  page("b", [select(STATUS, "todo"), select(PRIORITY, "low")]),
  page("c", [select(STATUS, "done"), select(PRIORITY, "high")]),
  page("d", [select(STATUS, "done"), select(PRIORITY, "low")]),
  page("e"),
];

const isTodo: PageListFilter = { propertyId: STATUS, op: "hasOption", value: "todo" };
const isDone: PageListFilter = { propertyId: STATUS, op: "hasOption", value: "done" };
const isHigh: PageListFilter = { propertyId: PRIORITY, op: "hasOption", value: "high" };
const isLow: PageListFilter = { propertyId: PRIORITY, op: "hasOption", value: "low" };

const group = (op: "and" | "or", ...items: FilterNode[]): FilterGroup => ({ op, items });
const ids = (list: readonly PageInfo[]): string[] => list.map((entry) => entry.id);
const pass = (tree: FilterGroup): string[] => ids(applyFilterTree(pages, tree, context));

describe("evaluating a tree", () => {
  it("passes every page for an empty group, whatever its operator", () => {
    expect(pass(EMPTY_FILTER_GROUP)).toEqual(["a", "b", "c", "d", "e"]);
    expect(pass(group("or"))).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("is the flat AND list of before when the group is AND of filters", () => {
    const filters = [isTodo, isHigh];
    expect(pass(group("and", ...filters))).toEqual(ids(applyFilters(pages, filters, defs)));
    expect(pass(group("and", ...filters))).toEqual(["a"]);
  });

  it("combines filters with OR", () => {
    expect(pass(group("or", isTodo, isHigh))).toEqual(["a", "b", "c"]);
  });

  it("nests one group: A and (B or C), and A or (B and C)", () => {
    expect(pass(group("and", isTodo, group("or", isHigh, isLow)))).toEqual(["a", "b"]);
    expect(pass(group("or", isDone, group("and", isTodo, isLow)))).toEqual(["b", "c", "d"]);
  });

  it("keeps a page that a group says nothing about out of neither side: an unapplicable filter is skipped", () => {
    const halfMade: PageListFilter = { propertyId: STATUS, op: "hasOption" }; // no option yet
    const gone: PageListFilter = { propertyId: GONE, op: "isSet" };
    const wrongOp: PageListFilter = { propertyId: COST, op: "contains", value: "x" }; // not an operator of a number
    expect(matchFilterGroup(pages[0] as PageInfo, group("and", halfMade), context)).toBeNull();
    // AND: skipped, so only the other filter counts (as the flat list always did).
    expect(pass(group("and", isTodo, halfMade, gone, wrongOp))).toEqual(["a", "b"]);
    // OR: skipped too, so `A or <half-made>` is `A`, not "every page".
    expect(pass(group("or", isTodo, halfMade))).toEqual(["a", "b"]);
    expect(pass(group("or", halfMade, gone))).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("skips a nested group that has nothing to apply, and passes everything when only such groups are left", () => {
    const emptyInside = group("or");
    const halfMade: PageListFilter = { propertyId: PRIORITY, op: "hasOption" };
    expect(pass(group("or", isDone, emptyInside))).toEqual(["c", "d"]);
    expect(pass(group("and", isDone, group("or", halfMade)))).toEqual(["c", "d"]);
    expect(pass(group("and", emptyInside, group("and", halfMade)))).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("returns a new list in the order it was given and never changes its input", () => {
    const input = [...pages].reverse();
    const out = applyFilterTree(input, group("or", isTodo, isDone), context);
    expect(ids(out)).toEqual(["d", "c", "b", "a"]);
    expect(out).not.toBe(input);
    expect(ids(input)).toEqual(["e", "d", "c", "b", "a"]);
  });

  it("gives the viewer's account to a `me` filter", () => {
    const person: PropertyDef = { id: "p-owner", name: "Owner", kind: "person", options: [], createdAt: 0 };
    const mine = page("mine", [["p-owner", { kind: "person", value: ["me-account"] }]]);
    const theirs = page("theirs", [["p-owner", { kind: "person", value: ["other"] }]]);
    const tree = group("and", { propertyId: "p-owner", op: "isMe" });
    expect(ids(applyFilterTree([mine, theirs], tree, { defs: [person], me: "me-account" }))).toEqual(["mine"]);
    // Without an account the filter cannot be applied: it hides nothing.
    expect(ids(applyFilterTree([mine, theirs], tree, { defs: [person] }))).toEqual(["mine", "theirs"]);
  });
});

describe("reading a stored tree", () => {
  it("takes a group as it is and a legacy list as an AND group", () => {
    const stored = { op: "or", items: [isTodo, { op: "and", items: [isHigh, isLow] }] };
    expect(normaliseFilterGroup(stored)).toEqual(stored);
    expect(normaliseFilterGroup([isTodo, isHigh])).toEqual(group("and", isTodo, isHigh));
    expect(normaliseFilterGroup([])).toEqual(EMPTY_FILTER_GROUP);
  });

  it("reads anything else as the empty group", () => {
    for (const odd of [undefined, null, 3, "x", true, {}, { op: "xor", items: [] }, { op: "and" }, { op: "and", items: "no" }]) {
      expect(normaliseFilterGroup(odd), JSON.stringify(odd)).toEqual(EMPTY_FILTER_GROUP);
    }
  });

  it("drops entries that are no filter and no group, and keeps a filter it cannot apply", () => {
    const stored = { op: "and", items: [isTodo, 3, null, "x", { propertyId: "", op: "isSet" }, { propertyId: GONE, op: "weird", value: 4 }, { op: "and" }] };
    expect(normaliseFilterGroup(stored)).toEqual(group("and", isTodo, { propertyId: GONE, op: "weird", value: 4 }));
  });

  it("allows one level: a group inside a nested group is dropped, its sibling filters stay", () => {
    const stored = { op: "and", items: [{ op: "or", items: [isHigh, { op: "and", items: [isLow] }, isLow] }] };
    expect(normaliseFilterGroup(stored)).toEqual(group("and", group("or", isHigh, isLow)));
  });

  it("keeps the first 50 filters in reading order across top level and nested groups", () => {
    const many = (n: number, id: string): PageListFilter[] => Array.from({ length: n }, (_, i) => ({ propertyId: `${id}${String(i)}`, op: "isSet" }));
    const stored = { op: "and", items: [...many(30, "top"), { op: "or", items: many(30, "in") }, ...many(5, "late")] };
    const read = normaliseFilterGroup(stored);
    expect(countFilters(read)).toBe(MAX_FILTERS);
    const nested = read.items.find(isFilterGroup);
    expect(nested === undefined ? 0 : countFilters(nested)).toBe(20);
    expect(read.items).toHaveLength(31);
    // The legacy list is cut the same way.
    expect(normaliseFilterGroup(many(60, "x")).items).toHaveLength(MAX_FILTERS);
  });

  it("keeps the first 10 nested groups", () => {
    const stored = { op: "and", items: Array.from({ length: 15 }, () => ({ op: "or", items: [isTodo] })) };
    expect(countGroups(normaliseFilterGroup(stored))).toBe(MAX_FILTER_GROUPS);
  });

  it("does not share objects with what it read (a later edit of the input cannot change the tree)", () => {
    const stored = { op: "and", items: [{ propertyId: STATUS, op: "hasOption", value: "todo", extra: 1 }] };
    const read = normaliseFilterGroup(stored);
    expect(read.items[0]).toEqual({ propertyId: STATUS, op: "hasOption", value: "todo" });
    expect(read.items[0]).not.toBe(stored.items[0]);
  });
});

describe("the default group and the mirror", () => {
  it("knows the one value that is not stored", () => {
    expect(isEmptyFilterGroup(EMPTY_FILTER_GROUP)).toBe(true);
    expect(isEmptyFilterGroup(group("and"))).toBe(true);
    expect(isEmptyFilterGroup(group("or"))).toBe(false);
    expect(isEmptyFilterGroup(group("and", isTodo))).toBe(false);
  });

  it("turns a legacy list into a group and back", () => {
    expect(legacyFiltersToGroup([])).toBe(EMPTY_FILTER_GROUP);
    expect(legacyFiltersToGroup([isTodo, isHigh])).toEqual(group("and", isTodo, isHigh));
    expect(legacyFilters(legacyFiltersToGroup([isTodo, isHigh]))).toEqual([isTodo, isHigh]);
  });

  it("gives an older build the plain filters of an AND group, and nothing for an OR group", () => {
    const and = group("and", isTodo, group("or", isHigh, isLow), isDone);
    expect(plainFilters(and)).toEqual([isTodo, isDone]);
    expect(legacyFilters(and)).toEqual([isTodo, isDone]);
    const or = group("or", isTodo, isDone);
    expect(plainFilters(or)).toEqual([isTodo, isDone]);
    expect(legacyFilters(or)).toEqual([]);
  });
});

describe("the editing operations", () => {
  it("adds a filter to the top level or to a nested group", () => {
    const start = group("and", isTodo, group("or", isHigh));
    expect(addFilter(start, isDone)).toEqual(group("and", isTodo, group("or", isHigh), isDone));
    expect(addFilter(start, isLow, 1)).toEqual(group("and", isTodo, group("or", isHigh, isLow)));
    expect(addFilter(start, isLow, 0)).toBe(start); // index 0 is a filter, not a group
    expect(addFilter(start, isLow, 7)).toBe(start);
    expect(start).toEqual(group("and", isTodo, group("or", isHigh)));
  });

  it("stops at 50 filters, nested ones counted", () => {
    const fill = (n: number): PageListFilter[] => Array.from({ length: n }, (_, i) => ({ propertyId: `p${String(i)}`, op: "isSet" }));
    const nearly = group("and", ...fill(25), group("or", ...fill(24)));
    expect(canAddFilter(nearly)).toBe(true);
    const full = addFilter(nearly, isTodo);
    expect(countFilters(full)).toBe(MAX_FILTERS);
    expect(canAddFilter(full)).toBe(false);
    expect(addFilter(full, isDone)).toBe(full);
    expect(addFilter(full, isDone, 25)).toBe(full);
  });

  it("adds an empty group with the opposite operator, up to 10", () => {
    expect(addGroup(group("and", isTodo))).toEqual(group("and", isTodo, group("or")));
    expect(addGroup(group("or", isTodo))).toEqual(group("or", isTodo, group("and")));
    let tree = group("and");
    for (let i = 0; i < MAX_FILTER_GROUPS; i += 1) {
      tree = addGroup(tree);
    }
    expect(canAddGroup(tree)).toBe(false);
    expect(addGroup(tree)).toBe(tree);
    expect(countGroups(tree)).toBe(MAX_FILTER_GROUPS);
  });

  it("changes the operator of the top group or of a nested group", () => {
    const start = group("and", isTodo, group("or", isHigh));
    expect(setGroupOp(start, "or")).toEqual(group("or", isTodo, group("or", isHigh)));
    expect(setGroupOp(start, "and", 1)).toEqual(group("and", isTodo, group("and", isHigh)));
    expect(setGroupOp(start, "and", 0)).toEqual(start); // not a group: nothing changes
  });

  it("replaces a filter at a path and nothing else", () => {
    const start = group("and", isTodo, group("or", isHigh, isLow));
    expect(replaceFilter(start, [0], isDone)).toEqual(group("and", isDone, group("or", isHigh, isLow)));
    expect(replaceFilter(start, [1, 1], isDone)).toEqual(group("and", isTodo, group("or", isHigh, isDone)));
    expect(replaceFilter(start, [1], isDone)).toBe(start); // a group is not replaced by a filter
    expect(replaceFilter(start, [0, 0], isDone)).toBe(start); // a filter has no items
    expect(replaceFilter(start, [5], isDone)).toBe(start);
    expect(replaceFilter(start, [1, 5], isDone)).toBe(start);
  });

  it("removes a filter, or a whole group with its filters", () => {
    const start = group("and", isTodo, group("or", isHigh, isLow), isDone);
    expect(removeAt(start, [0])).toEqual(group("and", group("or", isHigh, isLow), isDone));
    expect(removeAt(start, [1])).toEqual(group("and", isTodo, isDone));
    expect(removeAt(start, [1, 0])).toEqual(group("and", isTodo, group("or", isLow), isDone));
    expect(removeAt(start, [1, 1])).toEqual(group("and", isTodo, group("or", isHigh), isDone));
    expect(removeAt(start, [9])).toBe(start);
    expect(removeAt(start, [0, 0])).toBe(start);
    expect(removeAt(start, [1, 9])).toBe(start);
    // Removing the last filter of a group leaves the empty group (it is removed on its own).
    expect(removeAt(group("and", group("or", isHigh)), [0, 0])).toEqual(group("and", group("or")));
  });
});

describe("relative days and periods in a tree (ADR-0058)", () => {
  const DUE = "p-due";
  const dated: readonly PropertyDef[] = [...defs, { id: DUE, name: "Due", kind: "date", options: [], createdAt: 0 }];
  const due = (day: string): [string, PropertyValue] => [DUE, { kind: "date", value: { date: day } }];
  // Wednesday 2026-10-14; its week is 11 to 17 October from a Sunday and 12 to 18 from a Monday.
  const days: readonly PageInfo[] = [
    page("sunday", [due("2026-10-11")]),
    page("monday", [due("2026-10-12")]),
    page("today", [due("2026-10-14")]),
    page("sunday2", [due("2026-10-18")]),
    page("none"),
  ];
  const today = (filter: PageListFilter, more: Partial<FilterContext> = {}): string[] =>
    ids(applyFilterTree(days, group("and", filter), { defs: dated, today: "2026-10-14", weekStart: 0, ...more }));
  const thisWeek: PageListFilter = { propertyId: DUE, op: "within", value: { relative: "thisWeek" } };

  it("resolves a relative day at the context's today", () => {
    const isToday: PageListFilter = { propertyId: DUE, op: "equals", value: { relative: "today" } };
    expect(today(isToday)).toEqual(["today"]);
    expect(today(isToday, { today: "2026-10-12" })).toEqual(["monday"]);
    expect(today({ propertyId: DUE, op: "before", value: { relative: "today" } })).toEqual(["sunday", "monday"]);
  });

  it("resolves a period at the context's first day of the week, so a shared view means \"this week\" for each reader", () => {
    expect(today(thisWeek, { weekStart: 0 })).toEqual(["sunday", "monday", "today"]);
    expect(today(thisWeek, { weekStart: 1 })).toEqual(["monday", "today", "sunday2"]);
  });

  it("takes the context to nested groups and to every OR item", () => {
    const tree = group("and", group("or", { propertyId: DUE, op: "equals", value: { relative: "today" } }, { propertyId: DUE, op: "equals", value: { relative: "yesterday" } }));
    expect(ids(applyFilterTree(days, tree, { defs: dated, today: "2026-10-13", weekStart: 1 }))).toEqual(["monday"]);
    expect(ids(applyFilterTree(days, tree, { defs: dated, today: "2026-10-12", weekStart: 1 }))).toEqual(["sunday", "monday"]);
    expect(matchFilterGroup(days[2] as PageInfo, tree, { defs: dated, today: "2026-10-14" })).toBe(true);
    expect(matchFilterGroup(days[2] as PageInfo, tree, { defs: dated, today: "2026-10-20" })).toBe(false);
  });

  it("falls back to the device's day and week when the context says nothing", () => {
    const now = new Date();
    const day = (offset: number): string => {
      const next = new Date(now.getFullYear(), now.getMonth(), now.getDate() + offset);
      return `${String(next.getFullYear()).padStart(4, "0")}-${String(next.getMonth() + 1).padStart(2, "0")}-${String(next.getDate()).padStart(2, "0")}`;
    };
    const mine = [page("mine", [due(day(0))]), page("tomorrow", [due(day(1))])];
    const tree = group("and", { propertyId: DUE, op: "equals", value: { relative: "today" } });
    expect(ids(applyFilterTree(mine, tree, { defs: dated }))).toEqual(["mine"]);
    // A week holds today whatever day it starts on.
    expect(ids(applyFilterTree(mine, group("and", thisWeek), { defs: dated }))).toContain("mine");
  });

  it("skips a filter whose relative value is not one (kept, and hiding nothing)", () => {
    const broken: PageListFilter = { propertyId: DUE, op: "within", value: { relative: "pastDays" } };
    expect(today(broken)).toEqual(["sunday", "monday", "today", "sunday2", "none"]);
  });

  it("combines a relative filter with the others like any filter", () => {
    const tree = group("and", thisWeek, { propertyId: DUE, op: "onOrAfter", value: { relative: "today" } });
    expect(ids(applyFilterTree(days, tree, { defs: dated, today: "2026-10-14", weekStart: 1 }))).toEqual(["today", "sunday2"]);
  });
});
