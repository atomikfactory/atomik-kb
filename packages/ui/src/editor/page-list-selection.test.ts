// The selection model of the rows of a database block and the rules around it that need no React (ADR-0058, section 5 of
// `docs/architecture/database-power-ups.md`): toggle, range from the anchor in display order, all, prune, the tri-state, the keys, the row
// limit's arithmetic, and which properties and pages the action bar offers.

import type { PageNode, PropertyDef } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { bulkEditableProperties, moveTargets } from "./PageListBulkBar";
import { limitAfter, limitRows, ROW_LIMIT_STEP } from "./page-list-row-limit";
import {
  clearSelection,
  EMPTY_SELECTION,
  isTextField,
  pruneSelection,
  selectAll,
  selectedInOrder,
  selectionShortcut,
  selectionState,
  selectOnly,
  selectRange,
  toggleRow,
} from "./page-list-selection";
import type { RowSelection, ShortcutKey } from "./page-list-selection";

const ORDER = ["a", "b", "c", "d", "e"];

function ids(selection: RowSelection): string[] {
  return ORDER.filter((id) => selection.ids.has(id));
}

describe("toggleRow", () => {
  it("ticks a row, unticks it again, and makes it the anchor either way", () => {
    const one = toggleRow(EMPTY_SELECTION, "b");
    expect(ids(one)).toEqual(["b"]);
    expect(one.anchor).toBe("b");
    const two = toggleRow(one, "d");
    expect(ids(two)).toEqual(["b", "d"]);
    const back = toggleRow(two, "b");
    expect(ids(back)).toEqual(["d"]);
    expect(back.anchor).toBe("b");
  });

  it("never changes the selection it was given", () => {
    const before = toggleRow(EMPTY_SELECTION, "a");
    toggleRow(before, "b");
    expect(ids(before)).toEqual(["a"]);
    expect(EMPTY_SELECTION.ids.size).toBe(0);
  });
});

describe("selectRange", () => {
  it("ticks every row from the anchor to the row, in display order, whichever comes first", () => {
    const from = toggleRow(EMPTY_SELECTION, "b");
    expect(ids(selectRange(from, ORDER, "d"))).toEqual(["b", "c", "d"]);
    const backwards = toggleRow(EMPTY_SELECTION, "d");
    expect(ids(selectRange(backwards, ORDER, "b"))).toEqual(["b", "c", "d"]);
  });

  it("keeps what was ticked, and the clicked row becomes the anchor of the next range", () => {
    const start = toggleRow(toggleRow(EMPTY_SELECTION, "a"), "e");
    const range = selectRange(start, ORDER, "c");
    expect(ids(range)).toEqual(["a", "c", "d", "e"]);
    expect(range.anchor).toBe("c");
    expect(ids(selectRange(range, ORDER, "b"))).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("follows the order it is given, not the alphabet (a board: column by column)", () => {
    const order = ["z", "m", "a", "k"];
    const from = toggleRow(EMPTY_SELECTION, "z");
    expect(selectRange(from, order, "a").ids).toEqual(new Set(["z", "m", "a"]));
  });

  it("ticks only the row when there is no anchor, or the anchor is no longer a row, and ignores a row that is not shown", () => {
    expect(ids(selectRange(EMPTY_SELECTION, ORDER, "c"))).toEqual(["c"]);
    const stale: RowSelection = { ids: new Set(["x"]), anchor: "x" };
    expect(selectRange(stale, ORDER, "c").ids).toEqual(new Set(["x", "c"]));
    expect(selectRange(toggleRow(EMPTY_SELECTION, "a"), ORDER, "nope")).toEqual(toggleRow(EMPTY_SELECTION, "a"));
  });
});

describe("selectAll, selectOnly and clearSelection", () => {
  it("ticks every row of the order, which is every row of the view, and keeps an anchor that is still a row", () => {
    expect(ids(selectAll(ORDER))).toEqual(ORDER);
    expect(selectAll(ORDER, "c").anchor).toBe("c");
    expect(selectAll(ORDER, "gone").anchor).toBeNull();
    expect(selectAll([]).ids.size).toBe(0);
  });

  it("ticks exactly the pages it is given (the copies of a Duplicate), the last one the anchor", () => {
    const only = selectOnly(["d", "b"]);
    expect(ids(only)).toEqual(["b", "d"]);
    expect(only.anchor).toBe("b");
    expect(selectOnly([])).toEqual({ ids: new Set(), anchor: null });
  });

  it("clears to nothing", () => {
    expect(clearSelection()).toBe(EMPTY_SELECTION);
    expect(clearSelection().ids.size).toBe(0);
  });
});

describe("pruneSelection", () => {
  it("drops the rows that left the view, and an anchor that did", () => {
    const selection = selectRange(toggleRow(EMPTY_SELECTION, "a"), ORDER, "c");
    const pruned = pruneSelection(selection, new Set(["a", "c", "d"]));
    expect(ids(pruned)).toEqual(["a", "c"]);
    expect(pruned.anchor).toBe("c");
    expect(pruneSelection(selection, new Set(["a", "b"])).anchor).toBeNull();
  });

  it("answers the very same object when nothing left, so that a render that changes nothing sets no state", () => {
    const selection = selectAll(["a", "b"]);
    expect(pruneSelection(selection, new Set(ORDER))).toBe(selection);
    expect(pruneSelection(EMPTY_SELECTION, new Set())).toBe(EMPTY_SELECTION);
  });
});

describe("selectionState and selectedInOrder", () => {
  it("says none, some or all of the rows are ticked (nothing is 'all' of no rows)", () => {
    expect(selectionState(EMPTY_SELECTION, ORDER)).toBe("none");
    expect(selectionState(toggleRow(EMPTY_SELECTION, "b"), ORDER)).toBe("some");
    expect(selectionState(selectAll(ORDER), ORDER)).toBe("all");
    expect(selectionState(selectAll(ORDER), [])).toBe("none");
  });

  it("lists the ticked rows in display order, whatever order they were ticked in", () => {
    const selection = toggleRow(toggleRow(toggleRow(EMPTY_SELECTION, "e"), "a"), "c");
    expect(selectedInOrder(selection, ORDER)).toEqual(["a", "c", "e"]);
    expect(selectedInOrder(selection, ["e", "c", "a"])).toEqual(["e", "c", "a"]);
  });
});

describe("the keys of the view", () => {
  const key = (overrides: Partial<ShortcutKey> & { key: string }): ShortcutKey => ({
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    defaultPrevented: false,
    target: document.body,
    ...overrides,
  });

  it("Ctrl+A and Cmd+A select every row, Escape clears a selection that exists", () => {
    expect(selectionShortcut(key({ key: "a", ctrlKey: true }), false)).toBe("selectAll");
    expect(selectionShortcut(key({ key: "A", metaKey: true }), true)).toBe("selectAll");
    expect(selectionShortcut(key({ key: "Escape" }), true)).toBe("clear");
    expect(selectionShortcut(key({ key: "Escape" }), false)).toBeNull();
  });

  it("leaves every other key alone, and A with another modifier", () => {
    expect(selectionShortcut(key({ key: "a" }), true)).toBeNull();
    expect(selectionShortcut(key({ key: "a", ctrlKey: true, shiftKey: true }), true)).toBeNull();
    expect(selectionShortcut(key({ key: "a", ctrlKey: true, altKey: true }), true)).toBeNull();
    expect(selectionShortcut(key({ key: "Enter" }), true)).toBeNull();
  });

  it("leaves a key to the field it is typed in, and to whoever took it already", () => {
    const text = document.createElement("input");
    const area = document.createElement("textarea");
    const select = document.createElement("select");
    const box = document.createElement("input");
    box.type = "checkbox";
    expect(isTextField(text)).toBe(true);
    expect(isTextField(area)).toBe(true);
    expect(isTextField(select)).toBe(true);
    expect(isTextField(box)).toBe(false);
    expect(isTextField(document.createElement("button"))).toBe(false);
    expect(isTextField(null)).toBe(false);
    expect(selectionShortcut(key({ key: "a", ctrlKey: true, target: text }), true)).toBeNull();
    expect(selectionShortcut(key({ key: "Escape", target: area }), true)).toBeNull();
    expect(selectionShortcut(key({ key: "a", ctrlKey: true, target: box }), false)).toBe("selectAll");
    expect(selectionShortcut(key({ key: "Escape", defaultPrevented: true }), true)).toBeNull();
  });
});

describe("the row limit", () => {
  it("is 200 and goes up by 200", () => {
    expect(ROW_LIMIT_STEP).toBe(200);
    expect([0, 1, 2].map(limitAfter)).toEqual([200, 400, 600]);
    expect(limitAfter(-3)).toBe(200);
  });

  it("keeps the first rows, and the very same array when they all fit", () => {
    const rows = Array.from({ length: 450 }, (_, index) => index);
    expect(limitRows(rows, 200)).toHaveLength(200);
    expect(limitRows(rows, 200)[199]).toBe(199);
    expect(limitRows(rows, 400)).toHaveLength(400);
    expect(limitRows(rows, 600)).toBe(rows);
    expect(limitRows(rows, Number.POSITIVE_INFINITY)).toBe(rows);
  });
});

describe("what the action bar offers", () => {
  const def = (id: string, kind: PropertyDef["kind"]): PropertyDef => ({ id, name: id, kind, options: [], createdAt: 0 });

  it("lists the properties that take a value, in schema order, never the computed ones", () => {
    const schema = [
      def("tags", "multi_select"),
      def("a", "text"),
      def("b", "formula"),
      def("c", "created_time"),
      def("d", "rollup"),
      def("e", "relation"),
      def("f", "person"),
      def("g", "last_edited_by"),
      def("h", "checkbox"),
    ];
    expect(bulkEditableProperties(schema).map((entry) => entry.id)).toEqual(["tags", "a", "e", "f", "h"]);
  });

  const page = (id: string, children: PageNode[] = [], template = false): PageNode => ({
    page: { id, spaceId: "s", title: id, createdAt: 0, updatedAt: 0, ...(template ? { template: true } : {}) },
    children,
  });
  const roots = [page("one", [page("one-a", [page("one-a-1")]), page("one-b")]), page("two"), page("tpl", [page("tpl-a")], true)];

  it("lists every live page of the tree in order, with its depth, never a template or what is inside one", () => {
    const rows = moveTargets(roots, new Set(), "");
    expect(rows.map((row) => row.id)).toEqual(["one", "one-a", "one-a-1", "one-b", "two"]);
    expect(rows.map((row) => row.depth)).toEqual([0, 1, 2, 1, 0]);
  });

  it("leaves out a selected page and every page inside it: a page cannot go into itself", () => {
    expect(moveTargets(roots, new Set(["one-a"]), "").map((row) => row.id)).toEqual(["one", "one-b", "two"]);
    expect(moveTargets(roots, new Set(["one"]), "").map((row) => row.id)).toEqual(["two"]);
    expect(moveTargets(roots, new Set(["one-b", "two"]), "").map((row) => row.id)).toEqual(["one", "one-a", "one-a-1"]);
  });

  it("filters by title, case-insensitively, and lists the matches flat", () => {
    const rows = moveTargets(roots, new Set(), "A-1");
    expect(rows.map((row) => row.id)).toEqual(["one-a-1"]);
    expect(rows[0]?.depth).toBe(0);
    expect(moveTargets(roots, new Set(), "nothing like it")).toEqual([]);
  });
});
