// What a change of a view writes to its block (ADR-0054, `docs/architecture/database-views.md` section 2.4): the legacy
// attributes one at a time while a block has no `views` and the change fits them, `views` and the mirror of the first view
// otherwise. Pure rules, so no editor here (`page-list-tabs.test.tsx` and the older tests hold them through the editor).

import { describe, expect, it } from "vitest";

import { legacyFiltersToGroup } from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import { emptyView, parseViews } from "./page-list-views";
import { fitsLegacyAttributes, hasViews, isFlatAnd, sameViews, writeForPatch, writeForViews } from "./page-list-write";

const STATUS = "p-status";
const DUE = "p-due";
const todo = { propertyId: STATUS, op: "hasOption", value: "o-todo" };
const high = { propertyId: "p-priority", op: "hasOption", value: "o-high" };
const legacy = (extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  id: "b1",
  source: { kind: "children" },
  layout: "list",
  properties: [],
  sort: { by: "title", dir: "asc" },
  ...extra,
});

describe("hasViews", () => {
  it("is true when `views` holds at least one view this version can read", () => {
    expect(hasViews(legacy())).toBe(false);
    expect(hasViews(legacy({ views: null }))).toBe(false);
    expect(hasViews(legacy({ views: [] }))).toBe(false);
    expect(hasViews(legacy({ views: [3, "x", { name: "no id" }] }))).toBe(false);
    expect(hasViews(legacy({ views: [{ id: "k1", layout: "table" }] }))).toBe(true);
    expect(hasViews(legacy({ views: [null, { id: "k1" }] }))).toBe(true);
  });
});

describe("what fits the legacy attributes", () => {
  it("is a legacy layout, no name, no peek, calendar or gallery setting, a flat AND list of filters", () => {
    const view = emptyView("table", "default");
    expect(fitsLegacyAttributes(view)).toBe(true);
    expect(fitsLegacyAttributes({ ...view, layout: "board", groupBy: STATUS, hiddenGroups: ["none"] })).toBe(true);
    expect(fitsLegacyAttributes({ ...view, filter: legacyFiltersToGroup([todo, high]) })).toBe(true);
    expect(fitsLegacyAttributes({ ...view, layout: "calendar" })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, layout: "gallery" })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, name: "Mine" })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, openIn: "side" })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, dateBy: DUE })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, endBy: DUE })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, cardSize: "l" })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, defaultTemplate: "page-1" })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, extra: { later: 1 } })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, sorts: [{ by: DUE, dir: "desc" }] })).toBe(true);
    expect(fitsLegacyAttributes({ ...view, sorts: [{ by: DUE, dir: "desc" }, { by: STATUS, dir: "asc" }] })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, filter: { op: "or", items: [todo, high] } })).toBe(false);
    expect(fitsLegacyAttributes({ ...view, filter: { op: "and", items: [todo, { op: "or", items: [high] }] } })).toBe(false);
  });

  it("knows a flat AND group", () => {
    const flat: FilterGroup = { op: "and", items: [todo] };
    expect(isFlatAnd(flat)).toBe(true);
    expect(isFlatAnd({ op: "or", items: [] })).toBe(false);
    expect(isFlatAnd({ op: "and", items: [{ op: "and", items: [] }] })).toBe(false);
  });
});

describe("a block without views", () => {
  const attrs = legacy({ layout: "table" });

  it("keeps writing one legacy attribute at a time for a change the attributes can say", () => {
    const views = parseViews(attrs);
    expect(writeForPatch(attrs, views, "default", { sort: { by: DUE, dir: "desc" } })).toEqual({ sort: { by: DUE, dir: "desc" } });
    expect(writeForPatch(attrs, views, "default", { properties: [STATUS, DUE] })).toEqual({ properties: [STATUS, DUE] });
    expect(writeForPatch(attrs, views, "default", { layout: "board" })).toEqual({ layout: "board" });
    expect(writeForPatch(attrs, views, "default", { groupBy: STATUS })).toEqual({ groupBy: STATUS });
    expect(writeForPatch(attrs, views, "default", { columnWidths: { title: 300 } })).toEqual({ columnWidths: { title: 300 } });
    expect(writeForPatch(attrs, views, "default", { calculations: { [DUE]: "date_range" } })).toEqual({ calculations: { [DUE]: "date_range" } });
  });

  it("writes the one sort key as the legacy `sort`, the first key of several upgrading the block with the mirror", () => {
    const views = parseViews(attrs);
    expect(writeForPatch(attrs, views, "default", { sorts: [{ by: DUE, dir: "desc" }] })).toEqual({ sort: { by: DUE, dir: "desc" } });
    const written = writeForPatch(attrs, views, "default", { sorts: [{ by: DUE, dir: "desc" }, { by: STATUS, dir: "asc" }] });
    // The mirror: an older build keeps the primary order; `sorts` itself lives in the views only.
    expect(written?.sort).toEqual({ by: DUE, dir: "desc" });
    expect(written).not.toHaveProperty("sorts");
    expect((written?.views as { sorts?: unknown }[])[0]?.sorts).toEqual([{ by: DUE, dir: "desc" }, { by: STATUS, dir: "asc" }]);
    // And it reads back as both keys.
    const [view] = parseViews({ ...attrs, ...written });
    expect(view?.sorts).toEqual([{ by: DUE, dir: "desc" }, { by: STATUS, dir: "asc" }]);
    expect(view?.sort).toEqual({ by: DUE, dir: "desc" });
    // No change of the keys writes nothing.
    expect(writeForPatch(attrs, views, "default", { sorts: [{ by: "title", dir: "asc" }] })).toBeNull();
  });

  it("writes the flat filters, an empty list when the last one is taken away, and nothing of the view", () => {
    const views = parseViews(attrs);
    expect(writeForPatch(attrs, views, "default", { filter: legacyFiltersToGroup([todo]) })).toEqual({ filters: [todo] });
    const filtered = legacy({ layout: "table", filters: [todo] });
    expect(writeForPatch(filtered, parseViews(filtered), "default", { filter: { op: "and", items: [] } })).toEqual({ filters: [] });
  });

  it("removes an optional attribute with null, as a taken-away calculation did", () => {
    const withCalculations = legacy({ layout: "table", calculations: { [DUE]: "date_range" }, columnWidths: { title: 300 }, groupBy: STATUS, hiddenGroups: ["none"] });
    const views = parseViews(withCalculations);
    expect(writeForPatch(withCalculations, views, "default", { calculations: null })).toEqual({ calculations: null });
    expect(writeForPatch(withCalculations, views, "default", { columnWidths: null })).toEqual({ columnWidths: null });
    expect(writeForPatch(withCalculations, views, "default", { groupBy: null })).toEqual({ groupBy: null });
    expect(writeForPatch(withCalculations, views, "default", { hiddenGroups: null })).toEqual({ hiddenGroups: null });
  });

  it("writes nothing for a patch that changes nothing or names an unknown view", () => {
    const views = parseViews(attrs);
    expect(writeForPatch(attrs, views, "default", { sort: { by: "title", dir: "asc" } })).toBeNull();
    expect(writeForPatch(attrs, views, "default", { properties: [] })).toBeNull();
    expect(writeForPatch(attrs, views, "nope", { sort: { by: DUE, dir: "asc" } })).toBeNull();
  });

  it("leaves a layout it does not know as it is while another attribute changes", () => {
    const odd = legacy({ layout: "timeline" });
    const views = parseViews(odd);
    expect(views[0]).toMatchObject({ layout: "list", layoutRaw: "timeline" });
    expect(writeForPatch(odd, views, "default", { sort: { by: DUE, dir: "asc" } })).toEqual({ sort: { by: DUE, dir: "asc" } });
    // A layout the person picks replaces it.
    expect(writeForPatch(odd, views, "default", { layout: "table" })).toEqual({ layout: "table" });
  });

  it.each([
    ["a name", { name: "Mine" }],
    ["where pages open", { openIn: "center" as const }],
    ["a calendar layout", { layout: "calendar" as const }],
    ["a gallery layout", { layout: "gallery" as const }],
    ["a card size", { cardSize: "l" as const }],
    ["a default template", { defaultTemplate: "page-1" }],
    ["a date source", { dateBy: DUE }],
    ["a second sort key", { sorts: [{ by: DUE, dir: "desc" as const }, { by: STATUS, dir: "asc" as const }] }],
    ["an OR filter", { filter: { op: "or", items: [todo, high] } satisfies FilterGroup }],
    ["a filter group", { filter: { op: "and", items: [todo, { op: "or", items: [high] }] } satisfies FilterGroup }],
  ])("upgrades the block for %s: `views` and the mirror of the first view", (_name, patch) => {
    const views = parseViews(attrs);
    const written = writeForPatch(attrs, views, "default", patch);
    expect(written).not.toBeNull();
    expect(Array.isArray(written?.views)).toBe(true);
    expect((written?.views as unknown[]).length).toBe(1);
    // The mirror is there, whole: the older attributes follow the first view.
    for (const key of ["layout", "properties", "sort", "filters", "columnWidths", "groupBy", "hiddenGroups", "calculations"]) {
      expect(written, key).toHaveProperty(key);
    }
    // And it reads back as the patched view.
    const [view] = parseViews({ ...attrs, ...written });
    expect(view?.id).toBe("default");
  });

  it("mirrors an OR filter as no filters (an older build shows every page), and a flat AND group of the first view as its filters", () => {
    const views = parseViews(attrs);
    const or = writeForPatch(attrs, views, "default", { filter: { op: "or", items: [todo, high] } });
    expect(or?.filters).toBeNull();
    const nested = writeForPatch(attrs, views, "default", { filter: { op: "and", items: [todo, { op: "or", items: [high] }] } });
    expect(nested?.filters).toEqual([todo]);
  });
});

describe("a block that has views", () => {
  const two = (): Record<string, unknown> => {
    const first = { id: "k1", name: "", layout: "table", properties: [STATUS], sort: { by: "title", dir: "asc" } };
    const second = { id: "k2", name: "Board", layout: "board", properties: [], sort: { by: "title", dir: "asc" }, groupBy: STATUS };
    return legacy({ layout: "table", properties: [STATUS], views: [first, second] });
  };

  it("writes every change through the views, with the mirror of the first", () => {
    const attrs = two();
    const views = parseViews(attrs);
    const written = writeForPatch(attrs, views, "k1", { sort: { by: DUE, dir: "desc" } });
    expect(written?.sort).toEqual({ by: DUE, dir: "desc" });
    const list = written?.views as { id: string; sort: unknown }[];
    expect(list.map((entry) => entry.id)).toEqual(["k1", "k2"]);
    expect(list[0]?.sort).toEqual({ by: DUE, dir: "desc" });
    expect(list[1]?.sort).toEqual({ by: "title", dir: "asc" });
  });

  it("keeps the several keys of a view while it goes down to one (the views are written, not the legacy attribute), and mirrors only the first view", () => {
    const attrs = two();
    const keys = [{ by: DUE, dir: "desc" }, { by: STATUS, dir: "asc" }] as const;
    const sorted = writeForPatch(attrs, parseViews(attrs), "k1", { sorts: [...keys] });
    const next = { ...attrs, ...sorted };
    expect(parseViews(next)[0]?.sorts).toEqual(keys);
    const reduced = writeForPatch(next, parseViews(next), "k1", { sorts: [{ by: STATUS, dir: "asc" }] });
    expect(Array.isArray(reduced?.views)).toBe(true);
    expect(reduced?.sort).toEqual({ by: STATUS, dir: "asc" });
    expect((reduced?.views as { sorts?: unknown }[])[0]).not.toHaveProperty("sorts");
    // The second view's keys do not reach the mirror.
    const other = writeForPatch(next, parseViews(next), "k2", { sorts: keys });
    expect(other?.sort).toEqual({ by: DUE, dir: "desc" });
    expect((other?.views as { sorts?: unknown }[])[1]?.sorts).toEqual(keys);
    const third = writeForPatch(attrs, parseViews(attrs), "k2", { sorts: keys });
    expect(third?.sort).toEqual({ by: "title", dir: "asc" });
  });

  it("does not mirror a change of the second view", () => {
    const attrs = two();
    const written = writeForPatch(attrs, parseViews(attrs), "k2", { sort: { by: DUE, dir: "desc" } });
    expect(written?.sort).toEqual({ by: "title", dir: "asc" }); // the first view's
    expect(written?.layout).toBe("table");
    expect(written?.groupBy).toBeNull();
  });

  it("writes even a change a legacy attribute could say, and nothing for a patch that changes nothing", () => {
    const attrs = two();
    const written = writeForPatch(attrs, parseViews(attrs), "k1", { properties: [STATUS, DUE] });
    expect(Array.isArray(written?.views)).toBe(true);
    expect(writeForPatch(attrs, parseViews(attrs), "k1", { properties: [STATUS] })).toBeNull();
  });

  it("follows a reorder in the mirror, which is always the first view", () => {
    const attrs = two();
    const [first, second] = parseViews(attrs);
    if (first === undefined || second === undefined) {
      throw new Error("two views expected");
    }
    const written = writeForViews([second, first]);
    expect(written.layout).toBe("board");
    expect(written.groupBy).toBe(STATUS);
    expect(written.properties).toEqual([]);
  });
});

describe("a list of views", () => {
  it("is the same when it holds the very same views", () => {
    const a = emptyView("list", "k1");
    const b = emptyView("table", "k2");
    expect(sameViews([a, b], [a, b])).toBe(true);
    expect(sameViews([a, b], [b, a])).toBe(false);
    expect(sameViews([a], [a, b])).toBe(false);
    expect(sameViews([a], [{ ...a }])).toBe(false);
  });
});
