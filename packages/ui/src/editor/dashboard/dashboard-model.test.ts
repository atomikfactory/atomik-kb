// The widgets of a dashboard view (ADR-0055, `docs/architecture/dashboards.md` sections 2 and 3): reading the stored list leniently
// (defaults, limits, what is kept raw and in `extra`), writing it back, the defaults of each kind, the automatic titles, the
// operations of the grid (add, update, move, duplicate, remove) and the way the views model carries the widgets (`parseView`,
// `viewToJson`, `patchView`, the write rule). Pure: no React.

import type { PropertyDef } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { t } from "../../i18n";
import { fitsLegacyAttributes, writeForPatch } from "../page-list-write";
import { parseView, patchView, viewToJson } from "../page-list-views";
import type { PageListViewSpec } from "../page-list-views";
import {
  addWidget,
  automaticTitle,
  canAddWidget,
  DEFAULT_CALC,
  defaultWidget,
  duplicateWidget,
  embeddableViews,
  entryKey,
  isRawEntry,
  moveWidget,
  moveWidgetTo,
  newWidgetId,
  numberTitle,
  patchWidget,
  readWidgets,
  removeWidget,
  sameEntries,
  updateWidget,
  widgetIds,
  widgetsToJson,
  widgetTitle,
} from "./dashboard-model";
import type { DashboardEntry, TitleContext } from "./dashboard-model";
import { MAX_DASHBOARD_WIDGETS, MAX_WIDGET_TITLE_CHARS } from "./dashboard-types";
import type { DashboardWidget } from "./dashboard-types";

// ----- fixtures ----------------------------------------------------------------------------------

const HOURS: PropertyDef = { id: "p-hours", name: "Hours", kind: "number", options: [], createdAt: 1 };
const STATUS: PropertyDef = {
  id: "p-status",
  name: "Status",
  kind: "status",
  options: [{ id: "todo", name: "To do", color: "gray", group: "todo" }],
  createdAt: 1,
};
const SCHEMA = [STATUS, HOURS];

const CHART = { type: "bar", groupBy: "p-status", measure: { fn: "count" } };

function view(id: string, layout: PageListViewSpec["layout"], name = ""): PageListViewSpec {
  return { id, name, layout, properties: [], sort: { by: "title", dir: "asc" }, sorts: [{ by: "title", dir: "asc" }], filter: { op: "and", items: [] } };
}

const VIEWS = [view("v-dash", "dashboard", "Overview"), view("v-table", "table", "All tasks"), view("v-cal", "calendar")];

const CONTEXT: TitleContext = {
  schema: SCHEMA,
  views: VIEWS,
  layoutName: (layout) => t(`pageList.layout.${layout}`),
  t,
};

function widget(entry: DashboardEntry | undefined): DashboardWidget {
  if (entry === undefined || isRawEntry(entry)) {
    throw new Error("not a widget");
  }
  return entry;
}

const number = (id: string, extra: Partial<DashboardWidget> = {}): DashboardWidget => ({
  id,
  kind: "number",
  title: "",
  w: 1,
  h: "s",
  calc: DEFAULT_CALC,
  ...extra,
});

// ----- reading -----------------------------------------------------------------------------------

describe("reading the widgets", () => {
  it("reads anything that is no list as none, and skips what is no object", () => {
    expect(readWidgets(undefined)).toEqual([]);
    expect(readWidgets(null)).toEqual([]);
    expect(readWidgets({ id: "a1b2c3d4", kind: "number" })).toEqual([]);
    expect(readWidgets([1, "x", null, ["number"], { id: "a1b2c3d4", kind: "number" }])).toHaveLength(1);
  });

  it("reads a number widget with the defaults of what is missing", () => {
    const [entry] = readWidgets([{ id: "a1b2c3d4", kind: "number" }]);
    // Defaults of a widget that is *added* (section 2.2) are the add's; a stored widget reads its missing height as `m`.
    expect(entry).toEqual({ id: "a1b2c3d4", kind: "number", title: "", w: 1, h: "m", calc: { by: "title", fn: "count_all" } });
  });

  it("reads a width outside 1 to 4 as the width of the kind, and a height outside the three as `m`", () => {
    const entries = readWidgets([
      { id: "a0000001", kind: "number", w: 9, h: "xl" },
      { id: "a0000002", kind: "chart", chart: CHART, w: 0, h: 3 },
      { id: "a0000003", kind: "view", w: 2.5 },
      { id: "a0000004", kind: "view", w: 3, h: "l" },
      { id: "a0000005", kind: "number", w: "2" },
    ]);
    expect(entries.map((entry) => (isRawEntry(entry) ? null : [entry.w, entry.h]))).toEqual([
      [1, "m"],
      [2, "m"],
      [4, "m"],
      [3, "l"],
      [1, "m"],
    ]);
  });

  it("cuts a title past 80 characters and reads a title that is no text as empty", () => {
    const [long, odd] = readWidgets([
      { id: "a0000001", kind: "number", title: "x".repeat(200) },
      { id: "a0000002", kind: "number", title: 5 },
    ]);
    expect(widget(long).title).toHaveLength(MAX_WIDGET_TITLE_CHARS);
    expect(widget(odd).title).toBe("");
  });

  it("reads the calculation of a number, and a calculation that is not one as the count of the pages", () => {
    const entries = readWidgets([
      { id: "a0000001", kind: "number", calc: { by: "p-hours", fn: "sum" } },
      { id: "a0000002", kind: "number", calc: { by: "p-hours", fn: "median-ish" } },
      { id: "a0000003", kind: "number", calc: { fn: "count_unique" } },
      { id: "a0000004", kind: "number", calc: "sum" },
    ]);
    expect(entries.map((entry) => widget(entry).calc)).toEqual([
      { by: "p-hours", fn: "sum" },
      { by: "title", fn: "count_all" },
      { by: "title", fn: "count_unique" },
      { by: "title", fn: "count_all" },
    ]);
  });

  it("reads the own filter of a number and a chart with the limits, and leaves an empty one out", () => {
    const filter = { op: "or", items: [{ propertyId: "p-status", op: "hasOption", value: "todo" }, { op: "and", items: [] }] };
    const [withFilter, empty, junk] = readWidgets([
      { id: "a0000001", kind: "chart", chart: CHART, filter },
      { id: "a0000002", kind: "number", filter: { op: "and", items: [] } },
      { id: "a0000003", kind: "number", filter: "no" },
    ]);
    expect(widget(withFilter).filter).toEqual(filter);
    expect(widget(empty).filter).toBeUndefined();
    expect(widget(junk).filter).toBeUndefined();
  });

  it("reads the view a view widget shows (none when it names nothing)", () => {
    const [shown, nothing] = readWidgets([
      { id: "a0000001", kind: "view", viewId: "v-table" },
      { id: "a0000002", kind: "view", viewId: "" },
    ]);
    expect(widget(shown).viewId).toBe("v-table");
    expect(widget(nothing).viewId).toBeUndefined();
  });

  it("keeps a widget of an unknown kind, one with no usable id and a chart it cannot read as they were", () => {
    const stored = [
      { id: "a0000001", kind: "gauge", title: "Newer", value: 3 },
      { kind: "number" },
      { id: "", kind: "number" },
      { id: "a0000004", kind: "chart", chart: { type: "radar" } },
      { id: "a0000005", kind: "chart" },
      { id: "x".repeat(65), kind: "number" },
    ];
    const entries = readWidgets(stored);
    expect(entries).toHaveLength(6);
    expect(entries.every(isRawEntry)).toBe(true);
    expect(widgetsToJson(entries)).toEqual(stored);
  });

  it("keeps the first of a repeated id and reads at most 12", () => {
    const repeated = readWidgets([
      { id: "a0000001", kind: "number", title: "first" },
      { id: "a0000001", kind: "number", title: "second" },
    ]);
    expect(repeated.map((entry) => widget(entry).title)).toEqual(["first"]);
    const many = readWidgets(Array.from({ length: 20 }, (_, n) => ({ id: `a000000${n.toString(16)}-${String(n)}`, kind: "number" })));
    expect(many).toHaveLength(MAX_DASHBOARD_WIDGETS);
  });

  it("keeps the keys it does not know, and the keys of another kind", () => {
    const [entry] = readWidgets([
      { id: "a0000001", kind: "number", mood: "calm", chart: CHART, viewId: "v-table", filter: { op: "and", items: [] } },
    ]);
    expect(widget(entry).extra).toEqual({ mood: "calm", chart: CHART, viewId: "v-table" });
    expect(widgetsToJson([widget(entry)])).toEqual([
      { id: "a0000001", kind: "number", title: "", w: 1, h: "m", calc: { by: "title", fn: "count_all" }, mood: "calm", chart: CHART, viewId: "v-table" },
    ]);
  });

  it("leaves `__proto__` out of the kept keys", () => {
    const stored = JSON.parse('{"id":"a0000001","kind":"number","__proto__":{"polluted":true}}') as unknown;
    const [entry] = readWidgets([stored]);
    expect(widget(entry).extra).toBeUndefined();
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
  });
});

// ----- writing -----------------------------------------------------------------------------------

describe("writing the widgets", () => {
  it("writes the plain JSON of the contract: the common keys, then the kind's, then the kept ones", () => {
    const filter = { op: "and" as const, items: [{ propertyId: "p-status", op: "hasOption", value: "todo" }] };
    const entries: DashboardEntry[] = [
      number("a0000001", { title: "Open", calc: { by: "p-hours", fn: "sum" }, filter }),
      { id: "a0000002", kind: "chart", title: "", w: 2, h: "m", chart: { type: "bar", groupBy: "p-status", measure: { fn: "count" } } },
      { id: "a0000003", kind: "view", title: "Tasks", w: 4, h: "l", viewId: "v-table" },
    ];
    expect(widgetsToJson(entries)).toEqual([
      { id: "a0000001", kind: "number", title: "Open", w: 1, h: "s", filter, calc: { by: "p-hours", fn: "sum" } },
      { id: "a0000002", kind: "chart", title: "", w: 2, h: "m", chart: { type: "bar", groupBy: "p-status", measure: { fn: "count" } } },
      { id: "a0000003", kind: "view", title: "Tasks", w: 4, h: "l", viewId: "v-table" },
    ]);
  });

  it("reads back what it wrote", () => {
    const stored = [
      { id: "a0000001", kind: "number", title: "Open", w: 3, h: "l", calc: { by: "p-hours", fn: "average" }, extra1: [1, 2] },
      { id: "a0000002", kind: "chart", title: "", w: 2, h: "s", chart: { type: "donut", groupBy: "created", bucket: "week", measure: { fn: "sum", propertyId: "p-hours" } } },
      { id: "a0000003", kind: "view", title: "", w: 1, h: "m", viewId: "v-cal" },
      { id: "a0000004", kind: "gauge" },
    ];
    expect(widgetsToJson(readWidgets(stored))).toEqual(stored);
  });

  it("writes no more than 12 widgets, and no filter that says nothing", () => {
    const many = Array.from({ length: 15 }, (_, n) => number(`a000000${n.toString(16)}`));
    expect(widgetsToJson(many)).toHaveLength(MAX_DASHBOARD_WIDGETS);
    expect(widgetsToJson([number("a0000001", { filter: { op: "or", items: [] } })])).toEqual([
      { id: "a0000001", kind: "number", title: "", w: 1, h: "s", calc: { by: "title", fn: "count_all" } },
    ]);
  });

  it("writes the calculation of a number that has none", () => {
    const bare: DashboardWidget = { id: "a0000001", kind: "number", title: "", w: 1, h: "s" };
    expect(widgetsToJson([bare])).toEqual([{ id: "a0000001", kind: "number", title: "", w: 1, h: "s", calc: { by: "title", fn: "count_all" } }]);
  });
});

// ----- defaults ----------------------------------------------------------------------------------

describe("the defaults of a new widget", () => {
  it("is a one column, small count of the pages for a number", () => {
    expect(defaultWidget("number", "a0000001", SCHEMA, VIEWS)).toEqual({
      id: "a0000001",
      kind: "number",
      title: "",
      w: 1,
      h: "s",
      calc: { by: "title", fn: "count_all" },
    });
  });

  it("is a two column, medium chart of the count by the first status or select for a chart", () => {
    const chart = defaultWidget("chart", "a0000001", SCHEMA, VIEWS);
    expect([chart.w, chart.h]).toEqual([2, "m"]);
    expect(chart.chart).toMatchObject({ type: "bar", groupBy: "p-status", measure: { fn: "count" } });
    // No status or select: the month the pages were created.
    expect(defaultWidget("chart", "a0000001", [HOURS], VIEWS).chart).toMatchObject({ groupBy: "created", bucket: "month" });
  });

  it("is a full width, large view of the first view that is not a dashboard for a view", () => {
    const shown = defaultWidget("view", "a0000001", SCHEMA, VIEWS);
    expect([shown.w, shown.h, shown.viewId]).toEqual([4, "l", "v-table"]);
    expect(defaultWidget("view", "a0000001", SCHEMA, [VIEWS[0] as PageListViewSpec]).viewId).toBeUndefined();
    expect(embeddableViews(VIEWS).map((entry) => entry.id)).toEqual(["v-table", "v-cal"]);
  });

  it("makes ids of 8 hexadecimal characters that differ from the taken ones", () => {
    const taken = new Set<string>();
    for (let n = 0; n < 50; n += 1) {
      const id = newWidgetId(taken);
      expect(id).toMatch(/^[0-9a-f]{8}$/u);
      expect(taken.has(id)).toBe(false);
      taken.add(id);
    }
  });
});

// ----- titles ------------------------------------------------------------------------------------

describe("the titles", () => {
  it("names a number after its calculation, and the column when it is not the count of the pages", () => {
    expect(numberTitle({ by: "title", fn: "count_all" }, SCHEMA, t)).toBe("Count all");
    expect(numberTitle({ by: "p-hours", fn: "sum" }, SCHEMA, t)).toBe("Sum · Hours");
    expect(numberTitle({ by: "title", fn: "count_unique" }, SCHEMA, t)).toBe("Count unique values · Title");
    // A column that is gone: the calculation alone.
    expect(numberTitle({ by: "p-gone", fn: "sum" }, SCHEMA, t)).toBe("Sum");
  });

  it("names a chart after what it measures and groups by, and a view after the view it shows", () => {
    const chart = widget(readWidgets([{ id: "a0000001", kind: "chart", chart: CHART }])[0]);
    expect(automaticTitle(chart, CONTEXT)).toBe("Count by Status");
    expect(automaticTitle({ id: "a0000002", kind: "view", title: "", w: 4, h: "l", viewId: "v-table" }, CONTEXT)).toBe("All tasks");
    // An unnamed view takes its layout's name.
    expect(automaticTitle({ id: "a0000003", kind: "view", title: "", w: 4, h: "l", viewId: "v-cal" }, CONTEXT)).toBe("Calendar");
    // A view that is gone, and a dashboard, are named for the kind.
    expect(automaticTitle({ id: "a0000004", kind: "view", title: "", w: 4, h: "l", viewId: "v-gone" }, CONTEXT)).toBe("View");
    expect(automaticTitle({ id: "a0000005", kind: "view", title: "", w: 4, h: "l", viewId: "v-dash" }, CONTEXT)).toBe("View");
  });

  it("shows the title a widget was given, and the automatic one for an empty or blank title", () => {
    expect(widgetTitle(number("a0000001", { title: "Open tasks" }), CONTEXT)).toBe("Open tasks");
    expect(widgetTitle(number("a0000001", { title: "" }), CONTEXT)).toBe("Count all");
    expect(widgetTitle(number("a0000001", { title: "   " }), CONTEXT)).toBe("Count all");
  });
});

// ----- the operations ----------------------------------------------------------------------------

describe("the operations of the grid", () => {
  const a = number("a0000001", { title: "A" });
  const b = number("a0000002", { title: "B" });
  const c = number("a0000003", { title: "C" });
  const raw: DashboardEntry = { raw: { id: "a0000004", kind: "gauge" } };
  const titles = (entries: readonly DashboardEntry[]): string[] =>
    entries.map((entry) => (isRawEntry(entry) ? "raw" : entry.title));

  it("adds a widget at the end, and none past 12", () => {
    expect(titles(addWidget([a, b], c))).toEqual(["A", "B", "C"]);
    const full = Array.from({ length: MAX_DASHBOARD_WIDGETS }, (_, n) => number(`b000000${n.toString(16)}`));
    expect(canAddWidget(full)).toBe(false);
    expect(addWidget(full, c)).toHaveLength(MAX_DASHBOARD_WIDGETS);
    expect(canAddWidget([a])).toBe(true);
  });

  it("changes a widget by its id, and leaves the others and unknown ids alone", () => {
    const next = updateWidget([a, b, raw], "a0000002", { title: "  Renamed  ", w: 3, h: "l" });
    expect(next[0]).toBe(a);
    expect(next[2]).toBe(raw);
    expect(next[1]).toMatchObject({ title: "Renamed", w: 3, h: "l" });
    expect(sameEntries(updateWidget([a, b], "nope", { title: "x" }), [a, b])).toBe(true);
  });

  it("answers the same widget when a patch changes nothing", () => {
    expect(patchWidget(a, { title: "A", w: 1, h: "s" })).toBe(a);
    expect(patchWidget(a, {})).toBe(a);
    expect(patchWidget(a, { filter: null })).toBe(a);
  });

  it("sets and removes a widget's own filter (an empty one is none)", () => {
    const filter = { op: "and" as const, items: [{ propertyId: "p-status", op: "hasOption", value: "todo" }] };
    const filtered = patchWidget(a, { filter });
    expect(filtered.filter).toEqual(filter);
    expect(patchWidget(filtered, { filter: null }).filter).toBeUndefined();
    expect(patchWidget(filtered, { filter: { op: "or", items: [] } }).filter).toBeUndefined();
  });

  it("changes the calculation, the chart and the view", () => {
    expect(patchWidget(a, { calc: { by: "p-hours", fn: "sum" } }).calc).toEqual({ by: "p-hours", fn: "sum" });
    const chart = widget(readWidgets([{ id: "a0000009", kind: "chart", chart: CHART }])[0]);
    expect(patchWidget(chart, { chart: { type: "donut", groupBy: "p-status", measure: { fn: "count" } } }).chart?.type).toBe("donut");
    const shown = defaultWidget("view", "a0000008", SCHEMA, VIEWS);
    expect(patchWidget(shown, { viewId: "v-cal" }).viewId).toBe("v-cal");
  });

  it("moves an entry one place earlier or later, and stays put at the ends", () => {
    expect(titles(moveWidget([a, b, c], 1, -1))).toEqual(["B", "A", "C"]);
    expect(titles(moveWidget([a, b, c], 1, 1))).toEqual(["A", "C", "B"]);
    expect(titles(moveWidget([a, b, c], 0, -1))).toEqual(["A", "B", "C"]);
    expect(titles(moveWidget([a, b, c], 2, 1))).toEqual(["A", "B", "C"]);
    expect(titles(moveWidget([a, raw, b], 1, 1))).toEqual(["A", "B", "raw"]);
  });

  it("moves an entry to a drop slot (counted before the move), and changes nothing for the slots next to it", () => {
    expect(titles(moveWidgetTo([a, b, c], 0, 3))).toEqual(["B", "C", "A"]);
    expect(titles(moveWidgetTo([a, b, c], 0, 2))).toEqual(["B", "A", "C"]);
    expect(titles(moveWidgetTo([a, b, c], 2, 0))).toEqual(["C", "A", "B"]);
    expect(titles(moveWidgetTo([a, b, c], 2, 1))).toEqual(["A", "C", "B"]);
    for (const slot of [1, 2]) {
      expect(sameEntries(moveWidgetTo([a, b, c], 1, slot), [a, b, c])).toBe(true);
    }
    expect(sameEntries(moveWidgetTo([a, b, c], 1, 7), [a, b, c])).toBe(true);
    expect(sameEntries(moveWidgetTo([a, b, c], 5, 0), [a, b, c])).toBe(true);
  });

  it("duplicates a widget right after itself with a new id and the same settings, and not past 12 or a raw entry", () => {
    const filtered = number("a0000005", { title: "Hours", calc: { by: "p-hours", fn: "sum" } });
    const next = duplicateWidget([a, filtered, b], 1, "ffffffff");
    expect(next.map((entry) => entryKey(entry, 0))).toEqual(["a0000001", "a0000005", "ffffffff", "a0000002"]);
    expect(next[2]).toEqual({ ...filtered, id: "ffffffff" });
    expect(sameEntries(duplicateWidget([a, raw], 1, "ffffffff"), [a, raw])).toBe(true);
    const full = Array.from({ length: MAX_DASHBOARD_WIDGETS }, (_, n) => number(`b000000${n.toString(16)}`));
    expect(duplicateWidget(full, 0, "ffffffff")).toHaveLength(MAX_DASHBOARD_WIDGETS);
    expect(sameEntries(duplicateWidget([a], 4, "ffffffff"), [a])).toBe(true);
  });

  it("removes the entry at an index, a raw entry included", () => {
    expect(titles(removeWidget([a, b, c], 1))).toEqual(["A", "C"]);
    expect(titles(removeWidget([a, raw], 1))).toEqual(["A"]);
    expect(sameEntries(removeWidget([a, b], 5), [a, b])).toBe(true);
  });

  it("names the entries of a grid", () => {
    expect(widgetIds([a, raw, b])).toEqual(["a0000001", "a0000002"]);
    expect(entryKey(a, 0)).toBe("a0000001");
    expect(entryKey(raw, 3)).toBe("raw-3");
  });
});

// ----- the views model carries the widgets -------------------------------------------------------

describe("the widgets in a view", () => {
  const stored = {
    id: "k3f9a2bc",
    name: "",
    layout: "dashboard",
    properties: [],
    sort: { by: "title", dir: "asc" },
    widgets: [
      { id: "a0000001", kind: "number", title: "", w: 1, h: "s", calc: { by: "title", fn: "count_all" } },
      { id: "a0000002", kind: "gauge" },
    ],
  };

  it("reads `widgets` into the view, the unreadable one raw", () => {
    const parsed = parseView(stored);
    expect(parsed?.layout).toBe("dashboard");
    expect(parsed?.widgets).toHaveLength(2);
    expect(parsed?.widgets?.map(isRawEntry)).toEqual([false, true]);
    expect(parsed?.extra).toBeUndefined();
  });

  it("writes them back as they were read, and writes none for a view without", () => {
    const parsed = parseView(stored);
    expect(parsed === null ? null : viewToJson(parsed)).toEqual(stored);
    const bare = parseView({ id: "v1", layout: "dashboard" });
    expect(bare === null ? null : "widgets" in viewToJson(bare)).toBe(false);
    expect(parseView({ id: "v1", layout: "dashboard", widgets: [] })?.widgets).toBeUndefined();
  });

  it("keeps a `widgets` that is no list, written back as it was", () => {
    const parsed = parseView({ id: "v1", layout: "dashboard", widgets: { a: 1 } });
    expect(parsed?.widgets).toBeUndefined();
    expect(parsed?.extra).toEqual({ widgets: { a: 1 } });
    expect(parsed === null ? null : viewToJson(parsed).widgets).toEqual({ a: 1 });
  });

  it("changes them with a patch: the new list replaces the old, an empty one or null removes them, and a patch that says the same is no change", () => {
    const parsed = parseView(stored);
    if (parsed === null) {
      throw new Error("view not read");
    }
    const added = patchView(parsed, { widgets: [...(parsed.widgets ?? []), number("a0000003")] });
    expect(added.widgets).toHaveLength(3);
    expect(patchView(parsed, { widgets: parsed.widgets ?? [] })).toBe(parsed);
    expect(patchView(parsed, { widgets: [] }).widgets).toBeUndefined();
    expect(patchView(parsed, { widgets: null }).widgets).toBeUndefined();
    const odd = parseView({ id: "v1", layout: "dashboard", widgets: "nope" });
    if (odd === null) {
      throw new Error("view not read");
    }
    const replaced = patchView(odd, { widgets: [number("a0000001")] });
    expect(replaced.extra).toBeUndefined();
    expect(replaced.widgets).toHaveLength(1);
    // At most 12 are kept.
    const twenty = Array.from({ length: 20 }, (_, n) => number(`c000000${n.toString(16)}-${String(n)}`));
    expect(patchView(parsed, { widgets: twenty }).widgets).toHaveLength(MAX_DASHBOARD_WIDGETS);
  });

  it("is never a view the legacy attributes can hold, so a block with one always has `views`", () => {
    const dashboard = parseView(stored);
    if (dashboard === null) {
      throw new Error("view not read");
    }
    expect(fitsLegacyAttributes(dashboard)).toBe(false);
    // Widgets kept under another layout are not for the legacy attributes either.
    expect(fitsLegacyAttributes({ ...dashboard, layout: "table" })).toBe(false);
    expect(fitsLegacyAttributes({ ...dashboard, layout: "table", widgets: undefined })).toBe(true);
    const attrs = { source: { kind: "children" }, layout: "table", properties: [], sort: { by: "title", dir: "asc" } };
    const table = view("default", "table");
    const write = writeForPatch(attrs, [table], "default", { layout: "dashboard", widgets: [number("a0000001")] });
    expect(write).toMatchObject({ layout: "dashboard" });
    expect((write?.views as unknown[] | undefined)?.length).toBe(1);
  });
});
