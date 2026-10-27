// The views of a database block (ADR-0054): reading a block's attributes as views (a block without `views`, a
// damaged list, what a newer version added), writing them back with the mirror that keeps older builds working,
// patching a view, and the list operations of the tab strip with their limits. Pure: no editor, no store.

import { describe, expect, it } from "vitest";

import { EMPTY_FILTER_GROUP } from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import {
  DEFAULT_VIEW_ID,
  MAX_VIEWS,
  MAX_VIEW_NAME_CHARS,
  addView,
  canAddView,
  duplicateView,
  emptyView,
  legacyView,
  moveView,
  newViewId,
  parseView,
  parseViews,
  patchView,
  removeView,
  resolveActiveView,
  updateView,
  viewLabel,
  viewToJson,
  writeViews,
} from "./page-list-views";
import type { PageListViewSpec } from "./page-list-views";

const isTodo = { propertyId: "p-status", op: "hasOption", value: "todo" };
const isHigh = { propertyId: "p-prio", op: "hasOption", value: "high" };

const orGroup: FilterGroup = { op: "or", items: [isTodo, isHigh] };
const andGroup: FilterGroup = { op: "and", items: [isTodo, { op: "or", items: [isHigh] }] };

/** The attributes of a block as the editor holds them, written by a version that knows only the flat attributes. */
const LEGACY = {
  id: "block-1",
  source: { kind: "children" },
  layout: "table",
  properties: ["p-status", "p-due"],
  sort: { by: "p-due", dir: "desc" },
  filters: [isTodo, isHigh],
  columnWidths: { title: 300 },
  groupBy: "p-status",
  hiddenGroups: ["none"],
  calculations: { title: "count_all" },
};

describe("a block without views", () => {
  it("reads as one view made from its attributes", () => {
    const [view, ...rest] = parseViews(LEGACY);
    expect(rest).toEqual([]);
    expect(view).toEqual({
      id: DEFAULT_VIEW_ID,
      name: "",
      layout: "table",
      properties: ["p-status", "p-due"],
      sort: { by: "p-due", dir: "desc" },
      sorts: [{ by: "p-due", dir: "desc" }],
      filter: { op: "and", items: [isTodo, isHigh] },
      columnWidths: { title: 300 },
      groupBy: "p-status",
      hiddenGroups: ["none"],
      calculations: { title: "count_all" },
    });
  });

  it("reads the defaults for a block that never set anything", () => {
    expect(parseViews({ id: "b" })).toEqual([emptyView("list", DEFAULT_VIEW_ID)]);
    expect(parseViews({})).toEqual([emptyView("list", DEFAULT_VIEW_ID)]);
  });

  it("keeps a layout of a newer version as it is while drawing the list", () => {
    const view = legacyView({ layout: "timeline" });
    expect(view.layout).toBe("list");
    expect(view.layoutRaw).toBe("timeline");
    expect(viewToJson(view).layout).toBe("timeline");
    expect(writeViews([view]).layout).toBe("timeline");
    // Picking a layout drops it.
    const picked = patchView(view, { layout: "board" });
    expect(picked.layoutRaw).toBeUndefined();
    expect(viewToJson(picked).layout).toBe("board");
  });

  it("falls back to the legacy attributes when `views` is no list, is empty or holds nothing usable", () => {
    for (const views of [null, undefined, 3, "x", {}, [], [null, 3, "x", {}, { id: "" }, { id: 4 }]]) {
      const [view] = parseViews({ ...LEGACY, views });
      expect(view?.id, JSON.stringify(views)).toBe(DEFAULT_VIEW_ID);
      expect(view?.layout).toBe("table");
    }
  });
});

describe("reading views", () => {
  it("reads every part of a view", () => {
    const stored = {
      id: "k3f9a2bc",
      name: "Open tasks",
      layout: "calendar",
      properties: ["p-status"],
      sort: { by: "p-due", dir: "asc" },
      filter: orGroup,
      columnWidths: { "p-status": 200 },
      groupBy: "p-status",
      hiddenGroups: ["none"],
      calculations: { "p-due": "date_range" },
      openIn: "center",
      dateBy: "p-due",
      endBy: "p-end",
      cardSize: "l",
    };
    expect(parseView(stored)).toEqual({ ...stored, sorts: [{ by: "p-due", dir: "asc" }], columnWidths: { "p-status": 200 } });
    // And writes exactly that back.
    expect(viewToJson(parseView(stored) as PageListViewSpec)).toEqual(stored);
  });

  it("reads an old view's missing parts as their defaults and adds none when it is written", () => {
    const stored = { id: "v1", name: "", layout: "list", properties: [], sort: { by: "title", dir: "asc" } };
    const view = parseView(stored) as PageListViewSpec;
    expect(view.filter).toBe(EMPTY_FILTER_GROUP);
    expect(view.openIn).toBeUndefined();
    expect(view.cardSize).toBeUndefined();
    expect(viewToJson(view)).toEqual(stored);
  });

  it("skips entries without a usable id, keeps the first of a repeated id and reads at most 20", () => {
    const entries = [
      { id: "a", layout: "table" },
      { id: "a", layout: "board" },
      null,
      7,
      { layout: "list" },
      { id: "x".repeat(65) },
      { id: "b" },
      ...Array.from({ length: 30 }, (_, i) => ({ id: `n${String(i)}` })),
    ];
    const views = parseViews({ views: entries });
    expect(views).toHaveLength(MAX_VIEWS);
    expect(views.slice(0, 3).map((view) => [view.id, view.layout])).toEqual([
      ["a", "table"],
      ["b", "list"],
      ["n0", "list"],
    ]);
  });

  it("cuts a long name and reads a name that is no text as empty", () => {
    expect(parseView({ id: "a", name: "n".repeat(200) })?.name).toHaveLength(MAX_VIEW_NAME_CHARS);
    expect(parseView({ id: "a", name: 4 })?.name).toBe("");
  });

  it("reads a part it cannot use as its default", () => {
    const view = parseView({
      id: "a",
      layout: 3,
      properties: "no",
      sort: { by: "", dir: "up" },
      filter: "no",
      columnWidths: { x: -1, y: "wide" },
      groupBy: "",
      hiddenGroups: "no",
      calculations: [],
      dateBy: 4,
      endBy: "",
    }) as PageListViewSpec;
    expect(view).toEqual({
      id: "a",
      name: "",
      layout: "list",
      properties: [],
      sort: { by: "title", dir: "asc" },
      sorts: [{ by: "title", dir: "asc" }],
      filter: EMPTY_FILTER_GROUP,
    });
  });

  it("keeps what it does not understand and writes it back", () => {
    const stored = { id: "a", name: "", layout: "list", properties: [], sort: { by: "title", dir: "asc" }, openIn: "tab", cardSize: "xl", future: { x: [1, 2] } };
    const view = parseView(stored) as PageListViewSpec;
    expect(view.openIn).toBeUndefined();
    expect(view.cardSize).toBeUndefined();
    expect(view.extra).toEqual({ openIn: "tab", cardSize: "xl", future: { x: [1, 2] } });
    expect(viewToJson(view)).toEqual(stored);
    // A control that sets the part replaces what was kept; an unrelated change leaves it.
    expect(viewToJson(patchView(view, { openIn: "page" }))).toEqual({ ...stored, openIn: "page" });
    expect(viewToJson(patchView(view, { cardSize: null }))).toEqual({ id: "a", name: "", layout: "list", properties: [], sort: { by: "title", dir: "asc" }, openIn: "tab", future: { x: [1, 2] } });
    expect(viewToJson(patchView(view, { name: "Renamed" }))).toEqual({ ...stored, name: "Renamed" });
  });

  it("reads the default template (ADR-0056) as an id, and keeps a value that is no id", () => {
    const base = { id: "a", name: "", layout: "table", properties: [], sort: { by: "title", dir: "asc" } };
    expect(parseView({ ...base, defaultTemplate: "page-1" })?.defaultTemplate).toBe("page-1");
    expect(parseView(base)?.defaultTemplate).toBeUndefined();
    expect(parseView({ ...base, defaultTemplate: null })?.defaultTemplate).toBeUndefined();
    const odd = parseView({ ...base, defaultTemplate: 7 }) as PageListViewSpec;
    expect(odd.defaultTemplate).toBeUndefined();
    expect(odd.extra).toEqual({ defaultTemplate: 7 });
    expect(viewToJson(odd)).toEqual({ ...base, defaultTemplate: 7 });
    // Choosing a default replaces what was kept; the id is written only when set.
    expect(viewToJson(patchView(odd, { defaultTemplate: "page-2" }))).toEqual({ ...base, defaultTemplate: "page-2" });
    expect(viewToJson(parseView({ ...base, defaultTemplate: "page-1" }) as PageListViewSpec)).toEqual({ ...base, defaultTemplate: "page-1" });
    expect(viewToJson(parseView(base) as PageListViewSpec)).not.toHaveProperty("defaultTemplate");
  });

  it("never reads a prototype key", () => {
    const stored = JSON.parse('{"id":"a","__proto__":{"polluted":true}}') as unknown;
    const view = parseView(stored) as PageListViewSpec;
    expect(view.extra).toBeUndefined();
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
  });

  it("does not throw on anything", () => {
    for (const odd of [undefined, null, 1, "a", [], [[]], { views: [{ id: "a", filter: { op: "and", items: [[], null, 1] } }] }]) {
      expect(() => parseViews(odd as Record<string, unknown>)).not.toThrow();
    }
  });
});

describe("writing views", () => {
  const first: PageListViewSpec = {
    id: "v1",
    name: "Board",
    layout: "board",
    properties: ["p-status"],
    sort: { by: "p-due", dir: "desc" },
    sorts: [{ by: "p-due", dir: "desc" }],
    filter: andGroup,
    columnWidths: { title: 320 },
    groupBy: "p-status",
    hiddenGroups: ["none"],
    calculations: { title: "count_all" },
    openIn: "center",
  };
  const second: PageListViewSpec = { ...emptyView("calendar", "v2", "Calendar"), dateBy: "p-due", endBy: "p-end", filter: orGroup };

  it("writes the views and mirrors the first into the legacy attributes", () => {
    const write = writeViews([first, second]);
    expect(write.views).toEqual([viewToJson(first), viewToJson(second)]);
    expect(write.layout).toBe("board");
    expect(write.properties).toEqual(["p-status"]);
    expect(write.sort).toEqual({ by: "p-due", dir: "desc" });
    // The plain filters of the top-level AND group; the nested OR group is left out for an older build.
    expect(write.filters).toEqual([isTodo]);
    expect(write.columnWidths).toEqual({ title: 320 });
    expect(write.groupBy).toBe("p-status");
    expect(write.hiddenGroups).toEqual(["none"]);
    expect(write.calculations).toEqual({ title: "count_all" });
  });

  it("tells an older build nothing about an OR group, and removes what the first view does not set", () => {
    const write = writeViews([second, first]);
    expect(write.layout).toBe("calendar");
    expect(write.filters).toBeNull(); // an OR cannot be a flat list: the older build shows every page
    expect(write.columnWidths).toBeNull();
    expect(write.groupBy).toBeNull();
    expect(write.hiddenGroups).toBeNull();
    expect(write.calculations).toBeNull();
    expect(write.properties).toEqual([]);
    expect(writeViews([emptyView("list", "x")]).filters).toBeNull();
  });

  it("reads back what it wrote", () => {
    const views = [first, second, { ...emptyView("gallery", "v3"), cardSize: "s" as const, properties: ["p-a", "p-b"] }];
    const write = writeViews(views);
    expect(parseViews(write as unknown as Record<string, unknown>)).toEqual(views);
  });

  it("leaves an older build that drops `views` with the first view as it was", () => {
    const write = writeViews([first, second]);
    // What is left when `views` is deleted: the mirror, read as the legacy block it is.
    const { views: _dropped, ...legacy } = write;
    void _dropped;
    const [view, ...rest] = parseViews(legacy);
    expect(rest).toEqual([]);
    expect(view).toMatchObject({ id: DEFAULT_VIEW_ID, layout: "board", properties: ["p-status"], sort: { by: "p-due", dir: "desc" }, groupBy: "p-status", hiddenGroups: ["none"], calculations: { title: "count_all" } });
    expect(view?.filter).toEqual({ op: "and", items: [isTodo] });
  });

  it("writes at most 20 views and at least one", () => {
    const many = Array.from({ length: 25 }, (_, i) => emptyView("list", `v${String(i)}`));
    expect(writeViews(many).views).toHaveLength(MAX_VIEWS);
    expect(writeViews([]).views).toEqual([viewToJson(emptyView("list", DEFAULT_VIEW_ID))]);
  });

  it("writes plain copies (a later change of a view cannot change what was written)", () => {
    const write = writeViews([first]);
    const json = write.views[0] as { properties: string[]; filter: { items: unknown[] } };
    expect(json.properties).not.toBe(first.properties);
    expect(json.filter.items).not.toBe(andGroup.items);
    expect(JSON.parse(JSON.stringify(write))).toEqual(write);
  });
});

describe("patching a view", () => {
  const base: PageListViewSpec = { ...emptyView("table", "v1"), columnWidths: { title: 300 }, calculations: { title: "count_all" }, groupBy: "g", hiddenGroups: ["none"] };

  it("replaces the parts it names and leaves the others", () => {
    const next = patchView(base, { name: "Tasks", sort: { by: "created", dir: "desc" }, properties: ["p"], openIn: "page", dateBy: "p-due", endBy: "p-end", cardSize: "s" });
    expect(next).toMatchObject({ id: "v1", name: "Tasks", layout: "table", sort: { by: "created", dir: "desc" }, properties: ["p"], openIn: "page", dateBy: "p-due", endBy: "p-end", cardSize: "s", columnWidths: { title: 300 } });
    expect(base.name).toBe("");
  });

  it("removes an optional part with null and when it becomes empty", () => {
    const next = patchView(base, { columnWidths: null, calculations: {}, groupBy: null, hiddenGroups: [] });
    expect(next.columnWidths).toBeUndefined();
    expect(next.calculations).toBeUndefined();
    expect(next.groupBy).toBeUndefined();
    expect(next.hiddenGroups).toBeUndefined();
    expect(patchView({ ...base, openIn: "page", dateBy: "d", endBy: "e", cardSize: "l" }, { openIn: null, dateBy: null, endBy: null, cardSize: null })).toEqual({
      ...base,
    });
  });

  it("sets the default template, removes it with null (and with an empty id), and leaves it alone otherwise", () => {
    const set = patchView(base, { defaultTemplate: "page-1" });
    expect(set.defaultTemplate).toBe("page-1");
    expect(patchView(set, { name: "Tasks" }).defaultTemplate).toBe("page-1");
    expect(patchView(set, { defaultTemplate: "page-1" })).toBe(set);
    expect(patchView(set, { defaultTemplate: null }).defaultTemplate).toBeUndefined();
    expect(patchView(set, { defaultTemplate: "" }).defaultTemplate).toBeUndefined();
    expect(patchView(base, { defaultTemplate: null })).toBe(base);
    // A duplicated view carries it.
    expect(duplicateView([set], "v1", "v2", "Copy")[1]?.defaultTemplate).toBe("page-1");
  });

  it("normalises a filter tree it is given and cuts a name", () => {
    const next = patchView(base, { filter: { op: "and", items: [isTodo, { op: "or", items: [isHigh, { op: "and", items: [isTodo] } as never] }] }, name: "n".repeat(500) });
    expect(next.filter).toEqual({ op: "and", items: [isTodo, { op: "or", items: [isHigh] }] });
    expect(next.name).toHaveLength(MAX_VIEW_NAME_CHARS);
  });

  it("answers the same view when nothing changes", () => {
    expect(patchView(base, {})).toBe(base);
    expect(patchView(base, { name: "", sort: { by: "title", dir: "asc" }, groupBy: "g" })).toBe(base);
  });
});

describe("the list of views", () => {
  const v = (id: string): PageListViewSpec => emptyView("list", id, id);
  const list = [v("a"), v("b"), v("c")];
  const idsOf = (views: readonly PageListViewSpec[]): string[] => views.map((view) => view.id);

  it("shows the device's view, else the first", () => {
    expect(resolveActiveView(list, "b").id).toBe("b");
    expect(resolveActiveView(list, "gone").id).toBe("a");
    expect(resolveActiveView(list, undefined).id).toBe("a");
    expect(resolveActiveView(list, null).id).toBe("a");
  });

  it("names a tab by its name, or by its layout when it has none", () => {
    expect(viewLabel(emptyView("board", "x", "Mine"), () => "Board")).toBe("Mine");
    expect(viewLabel(emptyView("board", "x"), (layout) => layout.toUpperCase())).toBe("BOARD");
    expect(viewLabel(emptyView("board", "x", "   "), () => "Board")).toBe("Board");
  });

  it("adds a view at the end, up to 20", () => {
    expect(idsOf(addView(list, "board", "d"))).toEqual(["a", "b", "c", "d"]);
    expect(addView(list, "board", "d")[3]).toEqual(emptyView("board", "d"));
    const full = Array.from({ length: MAX_VIEWS }, (_, i) => v(`v${String(i)}`));
    expect(canAddView(list)).toBe(true);
    expect(canAddView(full)).toBe(false);
    expect(idsOf(addView(full, "list", "x"))).toEqual(idsOf(full));
  });

  it("duplicates a view right after the source with a new id and name", () => {
    const copy = duplicateView([{ ...v("a"), sort: { by: "created", dir: "desc" } }, v("b")], "a", "a2", "a (copy)");
    expect(idsOf(copy)).toEqual(["a", "a2", "b"]);
    expect(copy[1]).toMatchObject({ id: "a2", name: "a (copy)", sort: { by: "created", dir: "desc" } });
    expect(idsOf(duplicateView(list, "zz", "n", "n"))).toEqual(["a", "b", "c"]);
    const full = Array.from({ length: MAX_VIEWS }, (_, i) => v(`v${String(i)}`));
    expect(idsOf(duplicateView(full, "v0", "n", "n"))).toEqual(idsOf(full));
  });

  it("removes a view but never the last one", () => {
    expect(idsOf(removeView(list, "b"))).toEqual(["a", "c"]);
    expect(idsOf(removeView(list, "zz"))).toEqual(["a", "b", "c"]);
    expect(idsOf(removeView([v("a")], "a"))).toEqual(["a"]);
  });

  it("moves a view one place, not past either end", () => {
    expect(idsOf(moveView(list, "b", -1))).toEqual(["b", "a", "c"]);
    expect(idsOf(moveView(list, "b", 1))).toEqual(["a", "c", "b"]);
    expect(idsOf(moveView(list, "a", -1))).toEqual(["a", "b", "c"]);
    expect(idsOf(moveView(list, "c", 1))).toEqual(["a", "b", "c"]);
    expect(idsOf(moveView(list, "zz", 1))).toEqual(["a", "b", "c"]);
  });

  it("updates one view and leaves the others the same objects", () => {
    const next = updateView(list, "b", { name: "Bee" });
    expect(next[1]?.name).toBe("Bee");
    expect(next[0]).toBe(list[0]);
    expect(next[2]).toBe(list[2]);
    expect(updateView(list, "zz", { name: "x" })).toEqual(list);
  });

  it("makes ids of 8 hexadecimal characters that differ from the taken ones", () => {
    const taken = new Set<string>();
    for (let i = 0; i < 200; i += 1) {
      const id = newViewId(taken);
      expect(id).toMatch(/^[0-9a-f]{8}$/u);
      expect(taken.has(id)).toBe(false);
      taken.add(id);
    }
  });
});

describe("the sort keys of a view (ADR-0058)", () => {
  const base = { id: "a", name: "", layout: "table", properties: [] };
  const asc = (by: string) => ({ by, dir: "asc" as const });
  const desc = (by: string) => ({ by, dir: "desc" as const });

  it("reads `sorts` over `sort`, the first key being the spec's sort", () => {
    const view = parseView({ ...base, sort: asc("title"), sorts: [desc("p-due"), asc("p-status")] }) as PageListViewSpec;
    expect(view.sorts).toEqual([desc("p-due"), asc("p-status")]);
    expect(view.sort).toEqual(desc("p-due"));
    expect(view.sort).toBe(view.sorts[0]);
  });

  it("reads `[sort]` when `sorts` is missing, no list, empty or holds nothing usable", () => {
    for (const sorts of [undefined, null, "no", {}, [], [null, 3, { by: "" }, { dir: "desc" }]]) {
      const view = parseView({ ...base, sort: desc("p-due"), sorts }) as PageListViewSpec;
      expect(view.sorts, JSON.stringify(sorts)).toEqual([desc("p-due")]);
      expect(view.sort).toEqual(desc("p-due"));
    }
    expect((parseView({ ...base }) as PageListViewSpec).sorts).toEqual([asc("title")]);
  });

  it("reads the usable keys only: no repeat of a property (the first stays), a direction that is not desc as asc, at most 5", () => {
    const view = parseView({
      ...base,
      sorts: [asc("a"), 7, { by: "" }, desc("b"), desc("a"), { by: "c" }, asc("d"), asc("e"), asc("f"), asc("g")],
    }) as PageListViewSpec;
    expect(view.sorts).toEqual([asc("a"), desc("b"), asc("c"), asc("d"), asc("e")]);
  });

  it("reads a block without views as the one key of its `sort`", () => {
    expect(legacyView({ sort: desc("p-due") }).sorts).toEqual([desc("p-due")]);
    expect(legacyView({}).sorts).toEqual([asc("title")]);
    // Whatever `sorts` the legacy attributes hold is not read: the mirror never writes it.
    expect(legacyView({ sort: desc("p-due"), sorts: [asc("p-status")] }).sorts).toEqual([desc("p-due")]);
  });

  it("writes `sorts` only for more than one key, and `sort` as the first key always", () => {
    const one = parseView({ ...base, sort: desc("p-due") }) as PageListViewSpec;
    expect(viewToJson(one)).not.toHaveProperty("sorts");
    expect(viewToJson(one).sort).toEqual(desc("p-due"));
    const three = [desc("p-due"), asc("p-status"), asc("title")];
    const many = parseView({ ...base, sort: asc("title"), sorts: three }) as PageListViewSpec;
    const json = viewToJson(many);
    expect(json.sorts).toEqual(three);
    expect(json.sort).toEqual(desc("p-due"));
    // A stored view round-trips, and a `sorts` with one key is written as the `sort` it is.
    expect(viewToJson(parseView(json) as PageListViewSpec)).toEqual(json);
    expect(viewToJson(parseView({ ...base, sort: asc("title"), sorts: [desc("p-due")] }) as PageListViewSpec)).toEqual({
      ...base,
      sort: desc("p-due"),
    });
  });

  it("keeps a `sorts` it cannot use as it was, until a control sets the sorts", () => {
    const stored = { ...base, sort: asc("title"), sorts: "later" };
    const view = parseView(stored) as PageListViewSpec;
    expect(view.sorts).toEqual([asc("title")]);
    expect(view.extra).toEqual({ sorts: "later" });
    expect(viewToJson(view)).toEqual(stored);
    expect(viewToJson(patchView(view, { name: "Renamed" }))).toEqual({ ...stored, name: "Renamed" });
    expect(viewToJson(patchView(view, { sorts: [asc("p-due")] }))).toEqual({ ...base, sort: asc("p-due") });
    expect(viewToJson(patchView(view, { sort: desc("title") }))).toEqual({ ...base, sort: desc("title") });
  });

  it("starts with the default sort, as one key", () => {
    expect(emptyView("table", "v1").sorts).toEqual([asc("title")]);
    expect(emptyView("table", "v1").sort).toEqual(asc("title"));
  });

  it("patches the keys: `sorts` names them all, `sort` is the one key, an empty list is the default sort", () => {
    const view = emptyView("table", "v1");
    const two = patchView(view, { sorts: [desc("p-due"), asc("p-status")] });
    expect(two.sorts).toEqual([desc("p-due"), asc("p-status")]);
    expect(two.sort).toEqual(desc("p-due"));
    // The table header's sort replaces every key with one.
    const one = patchView(two, { sort: asc("p-status") });
    expect(one.sorts).toEqual([asc("p-status")]);
    expect(one.sort).toEqual(asc("p-status"));
    expect(patchView(two, { sorts: [] }).sorts).toEqual([asc("title")]);
    // `sorts` wins over `sort`; a list is read as a stored one (no repeat, at most 5).
    expect(patchView(view, { sort: asc("x"), sorts: [desc("y")] }).sorts).toEqual([desc("y")]);
    expect(patchView(view, { sorts: ["a", "b", "a", "c", "d", "e", "f"].map(asc) }).sorts).toEqual(["a", "b", "c", "d", "e"].map(asc));
  });

  it("answers the same view when the keys do not change", () => {
    const two = patchView(emptyView("table", "v1"), { sorts: [desc("p-due"), asc("p-status")] });
    expect(patchView(two, { sorts: [desc("p-due"), asc("p-status")] })).toBe(two);
    expect(patchView(emptyView("table", "v1"), { sort: asc("title") }).sorts).toEqual([asc("title")]);
    const same = emptyView("table", "v1");
    expect(patchView(same, { sorts: [asc("title")] })).toBe(same);
  });

  it("mirrors the first key of the first view into the legacy `sort`, and never writes `sorts` there", () => {
    const first = patchView(emptyView("table", "v1"), { sorts: [desc("p-due"), asc("p-status")] });
    const second = patchView(emptyView("list", "v2"), { sorts: [desc("created"), asc("title")] });
    const write = writeViews([first, second]);
    expect(write.sort).toEqual(desc("p-due"));
    expect(write).not.toHaveProperty("sorts");
    expect(write.views).toEqual([viewToJson(first), viewToJson(second)]);
    expect(write.views[0]?.sorts).toEqual([desc("p-due"), asc("p-status")]);
    // It follows a reorder: the mirror is always the first view.
    expect(writeViews([second, first]).sort).toEqual(desc("created"));
    expect(writeViews([emptyView("list", "v3")]).sort).toEqual(asc("title"));
  });

  it("keeps the other views' keys when a view is duplicated, and reads back what was written", () => {
    const view = patchView(emptyView("table", "v1"), { sorts: [desc("p-due"), asc("p-status")] });
    const [, copy] = duplicateView([view], "v1", "v2", "Copy");
    expect(copy?.sorts).toEqual(view.sorts);
    const read = parseViews({ views: writeViews([view, copy as PageListViewSpec]).views });
    expect(read.map((entry) => entry.sorts)).toEqual([view.sorts, view.sorts]);
  });
});
