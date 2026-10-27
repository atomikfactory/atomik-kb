// The widgets of a dashboard view (ADR-0055, `docs/architecture/dashboards.md` sections 2 and 3): reading the `widgets` a
// view stores (leniently, with the limits), writing them back, the operations of the grid (add, update, move, duplicate,
// remove), the defaults of each kind and the automatic titles. Pure: nothing here touches React, the store or the editor.
//
// **Reading is lenient** (the rule of the whole block): an entry that is no object is skipped, a repeated id keeps the first,
// at most 12 widgets are read, and a part of a widget that is not usable reads as its default (`w` outside 1..4 as the kind's
// width, `h` outside the three as `m`, a title past 80 characters is cut, a `calc` that is not a calculation as the count of
// pages). A widget this version cannot read at all (an unknown `kind`, no usable id, a chart `readChartSpec` refuses) is **kept
// as it was** (`{ raw }`), drawn as "This widget needs a newer version", and written back untouched. What a widget holds that
// this version does not know (unknown keys, the keys of another kind) is kept in `extra`.
//
// **Writing** (`widgetsToJson`) gives each widget the same plain JSON the contract documents: `id`, `kind`, `title`, `w` and `h`
// always; `filter` (number and chart) when it says something, `calc` (number), `chart` (chart), `viewId` (view); then `extra`.
//
// The operations take and answer a fresh list. Those that name a place take its **index** (a raw entry has no id to name).
// `sameEntries` tells a no-op from a change, so a control that changed nothing writes nothing.

import type { PropertyDef } from "@atomik-kb/core-client";

import type { MessageKey } from "../../i18n";
import { filterGroupToJson, normaliseFilterGroup } from "../page-list-filter-tree";
import type { FilterGroup } from "../page-list-filter-tree";
import type { PageListLayout } from "../page-list-model";
import type { PageListViewSpec } from "../page-list-views";
import { CALCULATION_LABELS, isCalculationFunction } from "../page-table-calculations";
import type { CalculationFunction } from "../page-table-calculations";
import { chartSpecToJson, chartTitle, defaultChartSpec, readChartSpec } from "./chart-model";
import { MAX_DASHBOARD_WIDGETS, MAX_WIDGET_TITLE_CHARS } from "./dashboard-types";
import type {
  ChartSpec,
  DashboardWidget,
  DashboardWidgetKind,
  RawDashboardWidget,
  WidgetHeight,
  WidgetWidth,
} from "./dashboard-types";

/** What a dashboard's `widgets` hold: a widget this version reads, or one it keeps as it was. */
export type DashboardEntry = DashboardWidget | RawDashboardWidget;

/** The kinds of widget, in the order the "Add widget" menu lists them. */
export const WIDGET_KINDS: readonly DashboardWidgetKind[] = ["number", "chart", "view"];
export const WIDGET_WIDTHS: readonly WidgetWidth[] = [1, 2, 3, 4];
export const WIDGET_HEIGHTS: readonly WidgetHeight[] = ["s", "m", "l"];

/** The width a new widget of each kind starts with, and what an unusable stored width reads as (section 2.2). */
export const DEFAULT_WIDTH: Readonly<Record<DashboardWidgetKind, WidgetWidth>> = { number: 1, chart: 2, view: 4 };
/** The height a new widget of each kind starts with. An unusable stored height reads as `m`. */
export const DEFAULT_HEIGHT: Readonly<Record<DashboardWidgetKind, WidgetHeight>> = { number: "s", chart: "m", view: "l" };

/** The calculation of a number widget that has none (or an unusable one): the count of the pages, by the title. */
export const DEFAULT_CALC: { readonly by: string; readonly fn: CalculationFunction } = { by: "title", fn: "count_all" };

/** Longest widget id, in characters (reading only; new ones are 8). */
const MAX_WIDGET_ID_CHARS = 64;

/** The keys every widget has; the others belong to one kind each. */
const COMMON_KEYS: readonly string[] = ["id", "kind", "title", "w", "h"];
/** The keys a kind uses besides the common ones; another kind's keys are kept in `extra`. */
const KIND_KEYS: Readonly<Record<DashboardWidgetKind, readonly string[]>> = {
  number: ["filter", "calc"],
  chart: ["filter", "chart"],
  view: ["viewId"],
};
const ALL_KIND_KEYS: readonly string[] = ["filter", "calc", "chart", "viewId"];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Whether a widget's filter says nothing (no filter in it, whatever its operator): such a filter is not kept. */
function saysNothing(filter: FilterGroup): boolean {
  return filter.items.length === 0;
}

/** Whether an entry is one this version could not read (kept as it was). */
export function isRawEntry(entry: DashboardEntry): entry is RawDashboardWidget {
  return "raw" in entry;
}

function isKind(value: unknown): value is DashboardWidgetKind {
  return typeof value === "string" && (WIDGET_KINDS as readonly string[]).includes(value);
}

function readWidth(value: unknown, kind: DashboardWidgetKind): WidgetWidth {
  return typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= 4
    ? (value as WidgetWidth)
    : DEFAULT_WIDTH[kind];
}

function readHeight(value: unknown): WidgetHeight {
  return value === "s" || value === "m" || value === "l" ? value : "m";
}

function readCalc(value: unknown): { readonly by: string; readonly fn: CalculationFunction } {
  if (!isRecord(value) || !isCalculationFunction(value.fn)) {
    return DEFAULT_CALC;
  }
  const by = typeof value.by === "string" && value.by !== "" ? value.by : DEFAULT_CALC.by;
  return { by, fn: value.fn };
}

/** One stored widget, or `null` when this version cannot read it (the caller keeps it raw). */
function readWidget(item: Record<string, unknown>): DashboardWidget | null {
  const { id, kind } = item;
  if (typeof id !== "string" || id === "" || id.length > MAX_WIDGET_ID_CHARS || !isKind(kind)) {
    return null;
  }
  const widget: { -readonly [K in keyof DashboardWidget]: DashboardWidget[K] } = {
    id,
    kind,
    title: typeof item.title === "string" ? item.title.slice(0, MAX_WIDGET_TITLE_CHARS) : "",
    w: readWidth(item.w, kind),
    h: readHeight(item.h),
  };
  if (kind !== "view") {
    const filter = normaliseFilterGroup(item.filter);
    if (!saysNothing(filter)) {
      widget.filter = filter;
    }
  }
  if (kind === "number") {
    widget.calc = readCalc(item.calc);
  }
  if (kind === "chart") {
    const chart = readChartSpec(item.chart);
    if (chart === null) {
      return null;
    }
    widget.chart = chart;
  }
  if (kind === "view" && typeof item.viewId === "string" && item.viewId !== "") {
    widget.viewId = item.viewId;
  }
  const extra: Record<string, unknown> = {};
  for (const [key, stored] of Object.entries(item)) {
    if (key === "__proto__" || COMMON_KEYS.includes(key)) {
      continue;
    }
    if (!ALL_KIND_KEYS.includes(key) || !KIND_KEYS[kind].includes(key)) {
      extra[key] = stored;
    }
  }
  if (Object.keys(extra).length > 0) {
    widget.extra = extra;
  }
  return widget;
}

/**
 * The widgets a view's `widgets` key holds: at most 12, the first of a repeated id, the ones this version cannot read kept raw.
 * Anything that is no list reads as none. Never throws, never rewrites.
 */
export function readWidgets(value: unknown): DashboardEntry[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const seen = new Set<string>();
  const entries: DashboardEntry[] = [];
  for (const item of value as readonly unknown[]) {
    if (entries.length >= MAX_DASHBOARD_WIDGETS) {
      break;
    }
    if (!isRecord(item)) {
      continue;
    }
    const widget = readWidget(item);
    if (widget === null) {
      entries.push({ raw: item });
    } else if (!seen.has(widget.id)) {
      seen.add(widget.id);
      entries.push(widget);
    }
  }
  return entries;
}

/** An entry as the plain JSON that is stored (a raw entry exactly as it was read). */
export function widgetToJson(entry: DashboardEntry): unknown {
  if (isRawEntry(entry)) {
    return entry.raw;
  }
  const out: Record<string, unknown> = {
    id: entry.id,
    kind: entry.kind,
    title: entry.title,
    w: entry.w,
    h: entry.h,
  };
  if (entry.kind !== "view" && entry.filter !== undefined && !saysNothing(entry.filter)) {
    out.filter = filterGroupToJson(entry.filter);
  }
  if (entry.kind === "number") {
    const calc = entry.calc ?? DEFAULT_CALC;
    out.calc = { by: calc.by, fn: calc.fn };
  }
  if (entry.kind === "chart" && entry.chart !== undefined) {
    out.chart = chartSpecToJson(entry.chart);
  }
  if (entry.kind === "view" && entry.viewId !== undefined) {
    out.viewId = entry.viewId;
  }
  for (const [key, value] of Object.entries(entry.extra ?? {})) {
    if (!(key in out)) {
      out[key] = value;
    }
  }
  return out;
}

/** The list as the plain JSON the view stores (at most 12 entries). */
export function widgetsToJson(entries: readonly DashboardEntry[]): unknown[] {
  return entries.slice(0, MAX_DASHBOARD_WIDGETS).map(widgetToJson);
}

// ----- ids and defaults ------------------------------------------------------------------------

/** A fresh widget id: 8 random lowercase hexadecimal characters, not in `taken`. */
export function newWidgetId(taken: Iterable<string>): string {
  const used = new Set(taken);
  for (;;) {
    const id =
      typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
        ? crypto.randomUUID().replaceAll("-", "").slice(0, 8)
        : Math.floor(Math.random() * 0x1_0000_0000)
            .toString(16)
            .padStart(8, "0");
    if (!used.has(id)) {
      return id;
    }
  }
}

/** The ids of the widgets of a list (raw entries have none). */
export function widgetIds(entries: readonly DashboardEntry[]): string[] {
  return entries.flatMap((entry) => (isRawEntry(entry) ? [] : [entry.id]));
}

/** The views a "view" widget can show: every view of the block that is not a dashboard. */
export function embeddableViews(views: readonly PageListViewSpec[]): PageListViewSpec[] {
  return views.filter((view) => view.layout !== "dashboard");
}

/** A new widget of `kind` with its defaults (section 2.2). A view widget shows the first view that is not a dashboard. */
export function defaultWidget(
  kind: DashboardWidgetKind,
  id: string,
  schema: readonly PropertyDef[],
  views: readonly PageListViewSpec[],
): DashboardWidget {
  const base = { id, title: "", w: DEFAULT_WIDTH[kind], h: DEFAULT_HEIGHT[kind] };
  switch (kind) {
    case "number":
      return { ...base, kind, calc: DEFAULT_CALC };
    case "chart":
      return { ...base, kind, chart: defaultChartSpec(schema) };
    case "view": {
      const [first] = embeddableViews(views);
      return first === undefined ? { ...base, kind } : { ...base, kind, viewId: first.id };
    }
  }
}

// ----- operations -------------------------------------------------------------------------------

/** What a control changes in a widget. `undefined` leaves a part alone; a `null` filter (or an empty one) removes it. */
export interface WidgetPatch {
  title?: string;
  w?: WidgetWidth;
  h?: WidgetHeight;
  filter?: FilterGroup | null;
  calc?: { readonly by: string; readonly fn: CalculationFunction };
  chart?: ChartSpec;
  viewId?: string;
}

/** A widget with `patch` applied; the same widget when the patch changes nothing. The title is trimmed and cut to 80 characters. */
export function patchWidget(widget: DashboardWidget, patch: WidgetPatch): DashboardWidget {
  const next: { -readonly [K in keyof DashboardWidget]: DashboardWidget[K] } = { ...widget };
  if (patch.title !== undefined) {
    next.title = patch.title.trim().slice(0, MAX_WIDGET_TITLE_CHARS);
  }
  if (patch.w !== undefined) {
    next.w = patch.w;
  }
  if (patch.h !== undefined) {
    next.h = patch.h;
  }
  if (patch.filter !== undefined) {
    const filter = patch.filter === null ? undefined : normaliseFilterGroup(patch.filter);
    if (filter === undefined || saysNothing(filter)) {
      delete next.filter;
    } else {
      next.filter = filter;
    }
  }
  if (patch.calc !== undefined) {
    next.calc = { by: patch.calc.by, fn: patch.calc.fn };
  }
  if (patch.chart !== undefined) {
    next.chart = patch.chart;
  }
  if (patch.viewId !== undefined) {
    next.viewId = patch.viewId;
  }
  return JSON.stringify(widgetToJson(next)) === JSON.stringify(widgetToJson(widget)) ? widget : next;
}

/** Whether another widget fits (12 in all). */
export function canAddWidget(entries: readonly DashboardEntry[]): boolean {
  return entries.length < MAX_DASHBOARD_WIDGETS;
}

/** The list with `widget` at the end; the same list when 12 widgets are there. */
export function addWidget(entries: readonly DashboardEntry[], widget: DashboardWidget): DashboardEntry[] {
  return canAddWidget(entries) ? [...entries, widget] : [...entries];
}

/** The list with `patch` applied to the widget `id`; the same list for an unknown id. */
export function updateWidget(entries: readonly DashboardEntry[], id: string, patch: WidgetPatch): DashboardEntry[] {
  return entries.map((entry) => (!isRawEntry(entry) && entry.id === id ? patchWidget(entry, patch) : entry));
}

/** The list with the entry at `index` one place earlier (`delta` -1) or later (1); the same list at either end. */
export function moveWidget(entries: readonly DashboardEntry[], index: number, delta: -1 | 1): DashboardEntry[] {
  const to = index + delta;
  const moved = entries[index];
  const other = entries[to];
  if (moved === undefined || other === undefined) {
    return [...entries];
  }
  const next = [...entries];
  next[index] = other;
  next[to] = moved;
  return next;
}

/**
 * The list with the entry at `from` put in the slot `slot` (0 is before the first entry, the length is after the last: the
 * places a drop indicator can stand, counted in the list as it is before the move). The same list when that is where it is.
 */
export function moveWidgetTo(entries: readonly DashboardEntry[], from: number, slot: number): DashboardEntry[] {
  const moved = entries[from];
  if (moved === undefined || slot < 0 || slot > entries.length || slot === from || slot === from + 1) {
    return [...entries];
  }
  const rest = entries.filter((_, at) => at !== from);
  const at = slot > from ? slot - 1 : slot;
  return [...rest.slice(0, at), moved, ...rest.slice(at)];
}

/** The list with a copy of the widget at `index` (id `newId`) right after it; the same list when 12 are there or the entry is raw. */
export function duplicateWidget(entries: readonly DashboardEntry[], index: number, newId: string): DashboardEntry[] {
  const source = entries[index];
  if (source === undefined || isRawEntry(source) || !canAddWidget(entries)) {
    return [...entries];
  }
  return [...entries.slice(0, index + 1), { ...source, id: newId }, ...entries.slice(index + 1)];
}

/** The list without the entry at `index`; the same list when there is none. */
export function removeWidget(entries: readonly DashboardEntry[], index: number): DashboardEntry[] {
  return entries.filter((_, at) => at !== index);
}

/** Whether two lists hold the very same entries (what the operations answer when they change nothing). */
export function sameEntries(a: readonly DashboardEntry[], b: readonly DashboardEntry[]): boolean {
  return a.length === b.length && a.every((entry, at) => entry === b[at]);
}

/** The key of an entry in the grid: its id, or its place for a raw entry (which has none). */
export function entryKey(entry: DashboardEntry, index: number): string {
  return isRawEntry(entry) ? `raw-${String(index)}` : entry.id;
}

// ----- titles -----------------------------------------------------------------------------------

/** The UI's translate function, as the titles take it. */
export type Translate = (key: MessageKey, params?: Record<string, string | number>) => string;

/** What the automatic titles need besides the widget. */
export interface TitleContext {
  readonly schema: readonly PropertyDef[];
  /** Every view of the block (a view widget is named after the one it shows). */
  readonly views: readonly PageListViewSpec[];
  /** The name of a layout (the title of an unnamed view). */
  readonly layoutName: (layout: PageListLayout) => string;
  readonly t: Translate;
}

/** The name of the column a calculation is over: "Title", the property's name, `null` when the property is gone. */
export function calcColumnName(by: string, schema: readonly PropertyDef[], t: Translate): string | null {
  if (by === "title") {
    return t("pageList.sort.title");
  }
  return schema.find((def) => def.id === by)?.name ?? null;
}

/** The automatic title of a number: the calculation's label and the column ("Count all", "Sum · Hours"). */
export function numberTitle(calc: { readonly by: string; readonly fn: CalculationFunction }, schema: readonly PropertyDef[], t: Translate): string {
  const label = t(CALCULATION_LABELS[calc.fn]);
  if (calc.by === "title" && calc.fn === "count_all") {
    return label;
  }
  const column = calcColumnName(calc.by, schema, t);
  return column === null ? label : t("dashboard.title.of", { function: label, column });
}

/** The automatic title of a widget (section 2.3). */
export function automaticTitle(widget: DashboardWidget, context: TitleContext): string {
  const { schema, views, layoutName, t } = context;
  switch (widget.kind) {
    case "number":
      return numberTitle(widget.calc ?? DEFAULT_CALC, schema, t);
    case "chart": {
      const title = widget.chart === undefined ? "" : chartTitle(widget.chart, schema, t);
      return title === "" ? t("dashboard.add.chart") : title;
    }
    case "view": {
      const view = views.find((candidate) => candidate.id === widget.viewId);
      if (view === undefined || view.layout === "dashboard") {
        return t("dashboard.add.view");
      }
      return view.name.trim() === "" ? layoutName(view.layout) : view.name;
    }
  }
}

/** The title a widget shows: the one it was given, else the automatic one. */
export function widgetTitle(widget: DashboardWidget, context: TitleContext): string {
  return widget.title.trim() === "" ? automaticTitle(widget, context) : widget.title;
}
