// Several sort keys (ADR-0058, `docs/architecture/database-power-ups.md` section 4): reading a list of keys (a usable key
// is an object with a `by`, none repeated, at most 5), and `sortPages` comparing by the first key, then the next, a missing
// value being last for each key and pages that tie on every key keeping the order they came in. Pure.

import type { PageInfo, PropertyDef, PropertyValue } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { MAX_SORTS, readSortList, readSorts, sortPages } from "./page-list-model";
import type { PageListSort } from "./page-list-model";

const STATUS = "p-status";
const PRIORITY = "p-priority";
const DUE = "p-due";

const schema: readonly PropertyDef[] = [
  {
    id: STATUS,
    name: "Status",
    kind: "select",
    options: [
      { id: "o-done", name: "Done", color: "green" },
      { id: "o-todo", name: "Todo", color: "gray" },
    ],
    createdAt: 0,
  },
  { id: PRIORITY, name: "Priority", kind: "number", options: [], createdAt: 0 },
  { id: DUE, name: "Due", kind: "date", options: [], createdAt: 0 },
];

function page(title: string, values: Record<string, PropertyValue> = {}, times: { createdAt?: number; updatedAt?: number } = {}): PageInfo {
  return {
    id: title,
    spaceId: "s",
    title,
    createdAt: times.createdAt ?? 0,
    updatedAt: times.updatedAt ?? 0,
    properties: Object.entries(values),
  };
}

const status = (option: "o-done" | "o-todo"): PropertyValue => ({ kind: "select", value: option });
const priority = (value: number): PropertyValue => ({ kind: "number", value });
const due = (day: string): PropertyValue => ({ kind: "date", value: { date: day } });
const asc = (by: string): PageListSort => ({ by, dir: "asc" });
const desc = (by: string): PageListSort => ({ by, dir: "desc" });
const order = (pages: readonly PageInfo[], ...sorts: PageListSort[]): string[] => sortPages(pages, sorts, schema).map((entry) => entry.title);

describe("reading the sort keys", () => {
  it("reads the usable keys of a list: an object with a `by`, none repeated, a direction that is not desc as asc", () => {
    expect(readSortList([asc("a"), desc("b"), { by: "c" }, { by: "d", dir: "sideways" }])).toEqual([asc("a"), desc("b"), asc("c"), asc("d")]);
    expect(readSortList([null, 3, "a", [], { by: "" }, { by: 4 }, { dir: "desc" }])).toEqual([]);
    expect(readSortList([asc("a"), desc("a"), asc("b")])).toEqual([asc("a"), asc("b")]);
    expect(readSortList("a")).toEqual([]);
    expect(readSortList(undefined)).toEqual([]);
    expect(readSortList({ 0: asc("a"), length: 1 })).toEqual([]);
  });

  it("reads at most 5 keys, counting the usable ones", () => {
    expect(MAX_SORTS).toBe(5);
    const six = ["a", "b", "c", "d", "e", "f"].map(asc);
    expect(readSortList(six)).toEqual(six.slice(0, 5));
    expect(readSortList([null, ...six, null])).toEqual(six.slice(0, 5));
  });

  it("reads `sorts` when it has a usable key, else the one key of `sort`, else the default", () => {
    expect(readSorts([desc("a"), asc("b")], asc("title"))).toEqual([desc("a"), asc("b")]);
    expect(readSorts([], desc("p-due"))).toEqual([desc("p-due")]);
    expect(readSorts([null], desc("p-due"))).toEqual([desc("p-due")]);
    expect(readSorts("no", desc("p-due"))).toEqual([desc("p-due")]);
    expect(readSorts(undefined, desc("p-due"))).toEqual([desc("p-due")]);
    expect(readSorts(undefined, { by: "", dir: "up" })).toEqual([asc("title")]);
    expect(readSorts(undefined)).toEqual([asc("title")]);
  });
});

describe("sorting by several keys", () => {
  const pages = [
    page("a", { [STATUS]: status("o-todo"), [PRIORITY]: priority(2) }),
    page("b", { [STATUS]: status("o-done"), [PRIORITY]: priority(1) }),
    page("c", { [STATUS]: status("o-todo"), [PRIORITY]: priority(1) }),
    page("d", { [STATUS]: status("o-done"), [PRIORITY]: priority(3) }),
    page("e", { [STATUS]: status("o-todo"), [PRIORITY]: priority(3) }),
  ];

  it("goes by the second key for the pages that tie on the first", () => {
    expect(order(pages, asc(STATUS), asc(PRIORITY))).toEqual(["b", "d", "c", "a", "e"]);
    expect(order(pages, asc(STATUS), desc(PRIORITY))).toEqual(["d", "b", "e", "a", "c"]);
    // The other way round: the priority first, then the status.
    expect(order(pages, asc(PRIORITY), asc(STATUS))).toEqual(["b", "c", "a", "d", "e"]);
  });

  it("has a direction for each key", () => {
    expect(order(pages, desc(STATUS), asc(PRIORITY))).toEqual(["c", "a", "e", "b", "d"]);
    expect(order(pages, desc(STATUS), desc(PRIORITY))).toEqual(["e", "a", "c", "d", "b"]);
  });

  it("goes by a third key, and keeps the order the pages came in when they tie on all of them", () => {
    const same = [page("z", { [PRIORITY]: priority(1) }), page("y", { [PRIORITY]: priority(1) }), page("x", { [PRIORITY]: priority(1) })];
    expect(order(same, asc(PRIORITY), asc(STATUS), asc("title"))).toEqual(["x", "y", "z"]);
    // Ties after the last key are the source order, in either direction.
    expect(order(same, asc(PRIORITY), asc(STATUS))).toEqual(["z", "y", "x"]);
    expect(order(same, desc(PRIORITY), desc(STATUS))).toEqual(["z", "y", "x"]);
    const three = [
      page("1", { [STATUS]: status("o-todo"), [PRIORITY]: priority(1), [DUE]: due("2026-10-02") }),
      page("2", { [STATUS]: status("o-todo"), [PRIORITY]: priority(1), [DUE]: due("2026-10-01") }),
      page("3", { [STATUS]: status("o-todo"), [PRIORITY]: priority(2), [DUE]: due("2026-09-30") }),
      page("4", { [STATUS]: status("o-done"), [PRIORITY]: priority(9), [DUE]: due("2026-12-01") }),
    ];
    expect(order(three, asc(STATUS), asc(PRIORITY), asc(DUE))).toEqual(["4", "2", "1", "3"]);
    expect(order(three, asc(STATUS), asc(PRIORITY), desc(DUE))).toEqual(["4", "1", "2", "3"]);
  });

  it("puts a page without a value last for each key, whichever way that key goes", () => {
    const rows = [
      page("none"),
      page("todoNone", { [STATUS]: status("o-todo") }),
      page("todo1", { [STATUS]: status("o-todo"), [PRIORITY]: priority(1) }),
      page("done1", { [STATUS]: status("o-done"), [PRIORITY]: priority(1) }),
      page("doneNone", { [STATUS]: status("o-done") }),
    ];
    expect(order(rows, asc(STATUS), asc(PRIORITY))).toEqual(["done1", "doneNone", "todo1", "todoNone", "none"]);
    // Descending puts the missing second value last inside each status, and the page with no status last of all.
    expect(order(rows, desc(STATUS), desc(PRIORITY))).toEqual(["todo1", "todoNone", "done1", "doneNone", "none"]);
    expect(order(rows, asc(STATUS), desc(PRIORITY))).toEqual(["done1", "doneNone", "todo1", "todoNone", "none"]);
  });

  it("sorts by the built-in keys and a property together", () => {
    const rows = [
      page("b", { [PRIORITY]: priority(1) }, { createdAt: 30, updatedAt: 3 }),
      page("a", { [PRIORITY]: priority(1) }, { createdAt: 10, updatedAt: 3 }),
      page("c", { [PRIORITY]: priority(2) }, { createdAt: 20, updatedAt: 1 }),
    ];
    expect(order(rows, asc(PRIORITY), asc("created"))).toEqual(["a", "b", "c"]);
    expect(order(rows, asc(PRIORITY), desc("created"))).toEqual(["b", "a", "c"]);
    expect(order(rows, desc("updated"), asc("title"))).toEqual(["a", "b", "c"]);
    expect(order(rows, asc("title"), desc("created"))).toEqual(["a", "b", "c"]);
  });

  it("sorts by the title for a property the space no longer has", () => {
    const rows = [page("b"), page("a"), page("c")];
    expect(order(rows, asc("p-gone"))).toEqual(["a", "b", "c"]);
    expect(order(rows, asc("p-gone"), desc("title"))).toEqual(["a", "b", "c"]);
  });

  it("uses only the first 5 keys, and leaves the order alone for none", () => {
    const rows = [page("b"), page("a")];
    expect(order(rows)).toEqual(["b", "a"]);
    const keys = [asc(PRIORITY), asc(PRIORITY), asc(PRIORITY), asc(PRIORITY), asc(PRIORITY), asc("title")];
    // The sixth key (the title) is not looked at: the pages tie on the first five and keep their order.
    expect(order(rows, ...keys)).toEqual(["b", "a"]);
    expect(order(rows, ...keys.slice(0, 4), asc("title"))).toEqual(["a", "b"]);
  });

  it("answers a new list and leaves the pages it was given as they were", () => {
    const source = [...pages];
    const sorted = sortPages(source, [asc(STATUS), asc(PRIORITY)], schema);
    expect(sorted).not.toBe(source);
    expect(source.map((entry) => entry.title)).toEqual(["a", "b", "c", "d", "e"]);
    expect(sorted.map((entry) => entry.title)).toEqual(["b", "d", "c", "a", "e"]);
  });
});
