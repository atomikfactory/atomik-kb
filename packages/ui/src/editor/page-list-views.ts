// The views of a database block (ADR-0054, amending ADR-0016): a `pageList` block holds a list of views, in its
// attribute `views`, that share the block's `source`. This file is the data model: the shape of a view
// (`PageListViewSpec`), reading it from a block's attributes without ever rewriting (`parseViews`), writing it
// back with the mirror that keeps older builds working (`writeViews`), and the pure list operations the tab strip
// uses (add, duplicate, remove, move, update). Nothing here touches the editor, the store or the page tree.
//
// **Stored shape.** `views` is a plain JSON list (set whole, like every attribute of the block: one Loro map
// write, last writer wins for the whole list); each entry is an object:
//
//   { "id": "k3f9a2bc", "name": "", "layout": "table", "properties": [...], "sort": { "by": "title", "dir": "asc" },
//     "sorts": [{ "by": "title", "dir": "asc" }, ...], "filter": { "op": "and", "items": [...] }, "columnWidths": {...},
//     "groupBy": "<property id>", "hiddenGroups": [...], "calculations": {...}, "openIn": "side",
//     "dateBy": "<property id>|created|updated", "endBy": "<property id>", "cardSize": "m", "defaultTemplate": "<page id>",
//     "widgets": [...] }
//
// `id`, `name`, `layout`, `properties` and `sort` are always written; the rest only when set (absent is the
// default). The entry is "the legacy attributes of the block, one per view" plus the new keys, with `filter` (a
// tree, `page-list-filter-tree.ts`) in place of the flat `filters`. **Sorts** (ADR-0058): a view orders by one to five
// keys, `sorts`; `sort` is always written as its first key (so a build that knows one key keeps the primary order) and
// `sorts` only when there are several. A reader uses `sorts` when it holds a usable key, else `[sort]`. A dashboard's
// `widgets` (ADR-0055) are read and written by `dashboard/dashboard-model.ts`.
//
// **The mirror.** The first view is also written into the block's own legacy attributes (`layout`, `properties`,
// `sort`, `filters`, `columnWidths`, `groupBy`, `hiddenGroups`, `calculations`), so that a build that does not
// know `views` shows that view, and keeps showing it if it drops `views` by editing the block. **A block without
// `views`** (every block written before ADR-0054) reads as one view made from the legacy attributes (id
// `"default"`, no name of its own: the label is the layout's name).
//
// **Reading is lenient** (the rule of the whole block): an entry that is no object or has no usable id is
// skipped, a duplicate id keeps the first, at most 20 views are read, a part of a view that is not usable reads as
// its default, and what this version does not understand is **kept** (`extra`, `layoutRaw`) and written back as it
// was, so that a view of a newer version survives an edit made here.

import type { PageId, PropertyId } from "@atomik-kb/core-client";

import { MAX_DASHBOARD_WIDGETS } from "./dashboard/dashboard-types";
import { readWidgets, widgetsToJson } from "./dashboard/dashboard-model";
import type { DashboardEntry } from "./dashboard/dashboard-model";
import { readGroupBy, readHiddenGroups } from "./page-board-model";
import {
  copyFilter,
  EMPTY_FILTER_GROUP,
  filterGroupToJson,
  isEmptyFilterGroup,
  legacyFilters,
  legacyFiltersToGroup,
  normaliseFilterGroup,
} from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import { readFilters } from "./page-list-filters";
import type { PageListFilter } from "./page-list-filters";
import {
  DEFAULT_PAGE_LIST_SORT,
  PAGE_LIST_LAYOUTS,
  readColumnWidths,
  readLayout,
  readProperties,
  readSortList,
  readSorts,
} from "./page-list-model";
import type { PageListLayout, PageListSort } from "./page-list-model";
import { readCalculations } from "./page-table-calculations";

/** Most views a block holds. */
export const MAX_VIEWS = 20;
/** Longest view name, in characters (an empty name shows the layout's own). */
export const MAX_VIEW_NAME_CHARS = 80;
/** Longest view id, in characters. */
export const MAX_VIEW_ID_CHARS = 64;
/** The id of the view a block without `views` reads as. */
export const DEFAULT_VIEW_ID = "default";

/** Where a click on a row opens its page: a side panel (the default), a dialog over the page, or the page itself. */
export type OpenIn = "side" | "center" | "page";
export const OPEN_IN_VALUES: readonly OpenIn[] = ["side", "center", "page"];
export const DEFAULT_OPEN_IN: OpenIn = "side";

/** The size of a gallery card. */
export type CardSize = "s" | "m" | "l";
export const CARD_SIZES: readonly CardSize[] = ["s", "m", "l"];
export const DEFAULT_CARD_SIZE: CardSize = "m";

/** The date sources of a calendar that are not properties (the page's own times), as the sorts name them. */
export const DATE_BY_CREATED = "created";
export const DATE_BY_UPDATED = "updated";

/** One view of a block. Immutable; `patchView` makes a changed copy. Optional parts are absent when they are the default. */
export interface PageListViewSpec {
  /** Unique among the block's views (1 to 64 characters; new ones are 8 random characters). */
  readonly id: string;
  /** The tab's label; empty means the layout's own name. At most 80 characters. */
  readonly name: string;
  /** The layout as it is drawn: a value this version does not know reads as `list` (and `layoutRaw` keeps it). */
  readonly layout: PageListLayout;
  /** The stored `layout` when this version does not know it; written back as it was until a control picks a layout. */
  readonly layoutRaw?: string;
  /** The properties shown on each row, card or chip (the columns of a table), in order. */
  readonly properties: readonly PropertyId[];
  /** The primary sort: always `sorts[0]` (what a build that knows one key reads). */
  readonly sort: PageListSort;
  /** The sort keys, first key first (1 to 5, never empty; a repeated `by` is dropped). */
  readonly sorts: readonly PageListSort[];
  /** The filter tree; `EMPTY_FILTER_GROUP` when there is none. */
  readonly filter: FilterGroup;
  readonly columnWidths?: Readonly<Record<string, number>>;
  /** The board's group property id. */
  readonly groupBy?: string;
  readonly hiddenGroups?: readonly string[];
  readonly calculations?: Readonly<Record<string, string>>;
  /** Absent means `DEFAULT_OPEN_IN`. */
  readonly openIn?: OpenIn;
  /** The calendar's date source: a Date property id, `"created"` or `"updated"`. Absent: not chosen yet. */
  readonly dateBy?: string;
  /** The calendar's optional end-date property id (a Date property). */
  readonly endBy?: string;
  /** Absent means `DEFAULT_CARD_SIZE`. */
  readonly cardSize?: CardSize;
  /**
   * The template "New" makes a page from (ADR-0056): the id of a template page. Used only while that page is one of the
   * templates of the block's source; otherwise (and when absent) "New" makes an empty page.
   */
  readonly defaultTemplate?: PageId;
  /** The dashboard's widgets (ADR-0055, `dashboard/dashboard-model.ts`); absent when there are none. Kept while the layout is another one. */
  readonly widgets?: readonly DashboardEntry[];
  /** What this version does not understand (unknown keys, a known key with a value it cannot use), kept as read. */
  readonly extra?: Readonly<Record<string, unknown>>;
}

/**
 * A change to a view, as the controls and the layouts make it. `undefined` leaves a part alone; `null` removes an
 * optional part (back to its default). Every part a patch names drops what was kept of it in `extra`.
 */
export interface PageListViewPatch {
  name?: string;
  layout?: PageListLayout;
  properties?: readonly PropertyId[];
  /** Replaces every sort key with this one (the table header's sort). */
  sort?: PageListSort;
  /** Replaces the sort keys (1 to 5; an empty list is the default sort). Wins over `sort`. */
  sorts?: readonly PageListSort[];
  filter?: FilterGroup;
  columnWidths?: Readonly<Record<string, number>> | null;
  groupBy?: string | null;
  hiddenGroups?: readonly string[] | null;
  calculations?: Readonly<Record<string, string>> | null;
  openIn?: OpenIn | null;
  dateBy?: string | null;
  endBy?: string | null;
  cardSize?: CardSize | null;
  /** `null` removes the default template (ADR-0056). */
  defaultTemplate?: PageId | null;
  /** An empty list (or `null`) removes the widgets. */
  widgets?: readonly DashboardEntry[] | null;
}

/** The keys of a stored view this version understands (everything else goes to `extra`). */
const KNOWN_KEYS: readonly string[] = [
  "id",
  "name",
  "layout",
  "properties",
  "sort",
  "sorts",
  "filter",
  "columnWidths",
  "groupBy",
  "hiddenGroups",
  "calculations",
  "openIn",
  "dateBy",
  "endBy",
  "cardSize",
  "defaultTemplate",
  "widgets",
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nonEmptyText(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined;
}

function isOpenIn(value: unknown): value is OpenIn {
  return typeof value === "string" && (OPEN_IN_VALUES as readonly string[]).includes(value);
}

function isCardSize(value: unknown): value is CardSize {
  return typeof value === "string" && (CARD_SIZES as readonly string[]).includes(value);
}

/** The layout text this version does not know (kept), `undefined` for a known one or no text. */
function unknownLayout(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" && !PAGE_LIST_LAYOUTS.includes(value) ? value : undefined;
}

/** The primary key of a list of sort keys (the default sort for a list with none, which a view never has). */
function firstSort(sorts: readonly PageListSort[]): PageListSort {
  return sorts[0] ?? DEFAULT_PAGE_LIST_SORT;
}

/** A new view of `layout`: default sort, no filters, no properties. */
export function emptyView(layout: PageListLayout, id: string, name = ""): PageListViewSpec {
  const sort = { by: DEFAULT_PAGE_LIST_SORT.by, dir: DEFAULT_PAGE_LIST_SORT.dir };
  return { id, name, layout, properties: [], sort, sorts: [sort], filter: EMPTY_FILTER_GROUP };
}

// ----- reading -------------------------------------------------------------------------------

/**
 * One stored view, or `null` when the entry is no object or has no usable id. Never throws.
 */
export function parseView(value: unknown): PageListViewSpec | null {
  if (!isRecord(value)) {
    return null;
  }
  const id = nonEmptyText(value.id);
  if (id === undefined || id.length > MAX_VIEW_ID_CHARS) {
    return null;
  }
  const extra: Record<string, unknown> = {};
  for (const [key, stored] of Object.entries(value)) {
    if (!KNOWN_KEYS.includes(key) && key !== "__proto__") {
      extra[key] = stored;
    }
  }
  const sorts = readSorts(value.sorts, value.sort);
  const view: { -readonly [K in keyof PageListViewSpec]: PageListViewSpec[K] } = {
    id,
    name: typeof value.name === "string" ? value.name.slice(0, MAX_VIEW_NAME_CHARS) : "",
    layout: readLayout(value.layout),
    properties: readProperties(value.properties),
    sort: firstSort(sorts),
    sorts,
    filter: normaliseFilterGroup(value.filter),
  };
  // A `sorts` this version cannot use (no list, or a list with no usable key) is kept as it was.
  if (value.sorts !== undefined && value.sorts !== null && readSortList(value.sorts).length === 0) {
    extra.sorts = value.sorts;
  }
  const raw = unknownLayout(value.layout);
  if (raw !== undefined) {
    view.layoutRaw = raw;
  }
  const widths = readColumnWidths(value.columnWidths);
  if (Object.keys(widths).length > 0) {
    view.columnWidths = widths;
  }
  const groupBy = readGroupBy(value.groupBy);
  if (groupBy !== null) {
    view.groupBy = groupBy;
  }
  const hidden = readHiddenGroups(value.hiddenGroups);
  if (hidden.length > 0) {
    view.hiddenGroups = hidden;
  }
  const calculations = readCalculations(value.calculations);
  if (Object.keys(calculations).length > 0) {
    view.calculations = calculations;
  }
  if (isOpenIn(value.openIn)) {
    view.openIn = value.openIn;
  } else if (value.openIn !== undefined && value.openIn !== null) {
    extra.openIn = value.openIn;
  }
  const dateBy = nonEmptyText(value.dateBy);
  if (dateBy !== undefined) {
    view.dateBy = dateBy;
  }
  const endBy = nonEmptyText(value.endBy);
  if (endBy !== undefined) {
    view.endBy = endBy;
  }
  if (isCardSize(value.cardSize)) {
    view.cardSize = value.cardSize;
  } else if (value.cardSize !== undefined && value.cardSize !== null) {
    extra.cardSize = value.cardSize;
  }
  const defaultTemplate = nonEmptyText(value.defaultTemplate);
  if (defaultTemplate !== undefined) {
    view.defaultTemplate = defaultTemplate;
  } else if (value.defaultTemplate !== undefined && value.defaultTemplate !== null) {
    extra.defaultTemplate = value.defaultTemplate;
  }
  if (Array.isArray(value.widgets)) {
    const widgets = readWidgets(value.widgets);
    if (widgets.length > 0) {
      view.widgets = widgets;
    }
  } else if (value.widgets !== undefined && value.widgets !== null) {
    extra.widgets = value.widgets;
  }
  if (Object.keys(extra).length > 0) {
    view.extra = extra;
  }
  return view;
}

/** The one view a block without `views` reads as: the legacy attributes (id `"default"`, no name of its own). */
export function legacyView(attrs: Readonly<Record<string, unknown>>): PageListViewSpec {
  const sorts = readSorts(undefined, attrs.sort);
  const view: { -readonly [K in keyof PageListViewSpec]: PageListViewSpec[K] } = {
    id: DEFAULT_VIEW_ID,
    name: "",
    layout: readLayout(attrs.layout),
    properties: readProperties(attrs.properties),
    sort: firstSort(sorts),
    sorts,
    filter: legacyFiltersToGroup(readFilters(attrs.filters)),
  };
  const raw = unknownLayout(attrs.layout);
  if (raw !== undefined) {
    view.layoutRaw = raw;
  }
  const widths = readColumnWidths(attrs.columnWidths);
  if (Object.keys(widths).length > 0) {
    view.columnWidths = widths;
  }
  const groupBy = readGroupBy(attrs.groupBy);
  if (groupBy !== null) {
    view.groupBy = groupBy;
  }
  const hidden = readHiddenGroups(attrs.hiddenGroups);
  if (hidden.length > 0) {
    view.hiddenGroups = hidden;
  }
  const calculations = readCalculations(attrs.calculations);
  if (Object.keys(calculations).length > 0) {
    view.calculations = calculations;
  }
  return view;
}

/**
 * The views of a block, from its attributes: the entries of `views` that can be read (at most 20, the first of
 * a repeated id), or, when there are none, the one view the legacy attributes make. Never empty, never throws,
 * never rewrites.
 */
export function parseViews(attrs: Readonly<Record<string, unknown>>): PageListViewSpec[] {
  const block = isRecord(attrs) ? attrs : {};
  const stored = block.views;
  if (Array.isArray(stored)) {
    const seen = new Set<string>();
    const views: PageListViewSpec[] = [];
    for (const entry of stored as readonly unknown[]) {
      const view = parseView(entry);
      if (view === null || seen.has(view.id)) {
        continue;
      }
      seen.add(view.id);
      views.push(view);
      if (views.length >= MAX_VIEWS) {
        break;
      }
    }
    if (views.length > 0) {
      return views;
    }
  }
  return [legacyView(block)];
}

// ----- writing -------------------------------------------------------------------------------

/** The attributes of a block that `writeViews` sets, ready for `updateAttributes`: `null` removes an attribute. */
export interface PageListAttrsWrite {
  views: Record<string, unknown>[];
  // The mirror of the first view (older builds read these).
  layout: string;
  properties: PropertyId[];
  sort: PageListSort;
  filters: PageListFilter[] | null;
  columnWidths: Record<string, number> | null;
  groupBy: string | null;
  hiddenGroups: string[] | null;
  calculations: Record<string, string> | null;
}

/** A view as the plain JSON that is stored: required parts always, optional parts when set, then what was kept in `extra`. */
export function viewToJson(view: PageListViewSpec): Record<string, unknown> {
  // `sorts` is the truth, `sort` its first key (the spec's `sort` only when a list has none).
  const first = view.sorts[0] ?? view.sort;
  const out: Record<string, unknown> = {
    id: view.id,
    name: view.name,
    layout: view.layoutRaw ?? view.layout,
    properties: [...view.properties],
    sort: { by: first.by, dir: first.dir },
  };
  if (view.sorts.length > 1) {
    out.sorts = view.sorts.map((sort) => ({ by: sort.by, dir: sort.dir }));
  }
  if (!isEmptyFilterGroup(view.filter)) {
    out.filter = filterGroupToJson(view.filter);
  }
  if (view.columnWidths !== undefined) {
    out.columnWidths = { ...view.columnWidths };
  }
  if (view.groupBy !== undefined) {
    out.groupBy = view.groupBy;
  }
  if (view.hiddenGroups !== undefined) {
    out.hiddenGroups = [...view.hiddenGroups];
  }
  if (view.calculations !== undefined) {
    out.calculations = { ...view.calculations };
  }
  if (view.openIn !== undefined) {
    out.openIn = view.openIn;
  }
  if (view.dateBy !== undefined) {
    out.dateBy = view.dateBy;
  }
  if (view.endBy !== undefined) {
    out.endBy = view.endBy;
  }
  if (view.cardSize !== undefined) {
    out.cardSize = view.cardSize;
  }
  if (view.defaultTemplate !== undefined) {
    out.defaultTemplate = view.defaultTemplate;
  }
  if (view.widgets !== undefined) {
    out.widgets = widgetsToJson(view.widgets);
  }
  for (const [key, value] of Object.entries(view.extra ?? {})) {
    if (!(key in out)) {
      out[key] = value;
    }
  }
  return out;
}

/**
 * What to write for a list of views: the `views` attribute and the mirror of the first view in the legacy
 * attributes (see the top of this file). At most 20 views are written; an empty list writes the one default view.
 * `filters` carries the plain filters of the first view's top-level group when it is AND and nothing (`null`, which
 * an older build reads as no filters) when it is OR or has none; the other optional mirrored attributes are
 * `null` when the first view does not set them.
 */
export function writeViews(views: readonly PageListViewSpec[]): PageListAttrsWrite {
  const kept = views.slice(0, MAX_VIEWS);
  const [first = emptyView("list", DEFAULT_VIEW_ID)] = kept;
  const list = kept.length === 0 ? [first] : kept;
  const filters = legacyFilters(first.filter);
  const primary = first.sorts[0] ?? first.sort;
  return {
    views: list.map(viewToJson),
    layout: first.layoutRaw ?? first.layout,
    properties: [...first.properties],
    sort: { by: primary.by, dir: primary.dir },
    filters: filters.length === 0 ? null : filters.map(copyFilter),
    columnWidths: first.columnWidths === undefined ? null : { ...first.columnWidths },
    groupBy: first.groupBy ?? null,
    hiddenGroups: first.hiddenGroups === undefined ? null : [...first.hiddenGroups],
    calculations: first.calculations === undefined ? null : { ...first.calculations },
  };
}

// ----- operations ----------------------------------------------------------------------------

/**
 * A view with `patch` applied. Every part the patch names replaces the old one (and drops what `extra` kept of it);
 * a `null` removes an optional part; `filter` is normalised (the limits apply); an empty map or list removes its
 * part; a name is cut to 80 characters. The same view when the patch changes nothing.
 */
export function patchView(view: PageListViewSpec, patch: PageListViewPatch): PageListViewSpec {
  const next: { -readonly [K in keyof PageListViewSpec]: PageListViewSpec[K] } = { ...view };
  const extra: Record<string, unknown> = { ...(view.extra ?? {}) };
  const take = (key: string): void => {
    Reflect.deleteProperty(extra, key);
  };
  if (patch.name !== undefined) {
    next.name = patch.name.slice(0, MAX_VIEW_NAME_CHARS);
  }
  if (patch.layout !== undefined) {
    next.layout = patch.layout;
    delete next.layoutRaw;
  }
  if (patch.properties !== undefined) {
    next.properties = [...patch.properties];
  }
  if (patch.sorts !== undefined || patch.sort !== undefined) {
    take("sorts");
    // `sorts` names every key, `sort` alone is the one key; the list is read as a stored one is (usable keys, no repeat, 5).
    const named = readSortList(patch.sorts ?? [patch.sort]);
    const sorts = named.length > 0 ? named : [{ ...DEFAULT_PAGE_LIST_SORT }];
    next.sorts = sorts;
    next.sort = firstSort(sorts);
  }
  if (patch.filter !== undefined) {
    next.filter = normaliseFilterGroup(patch.filter);
  }
  if (patch.columnWidths !== undefined) {
    const widths = patch.columnWidths === null ? {} : readColumnWidths(patch.columnWidths);
    if (Object.keys(widths).length === 0) {
      delete next.columnWidths;
    } else {
      next.columnWidths = widths;
    }
  }
  if (patch.groupBy !== undefined) {
    const group = readGroupBy(patch.groupBy);
    if (group === null) {
      delete next.groupBy;
    } else {
      next.groupBy = group;
    }
  }
  if (patch.hiddenGroups !== undefined) {
    const hidden = patch.hiddenGroups === null ? [] : readHiddenGroups(patch.hiddenGroups);
    if (hidden.length === 0) {
      delete next.hiddenGroups;
    } else {
      next.hiddenGroups = hidden;
    }
  }
  if (patch.calculations !== undefined) {
    const calculations = patch.calculations === null ? {} : readCalculations(patch.calculations);
    if (Object.keys(calculations).length === 0) {
      delete next.calculations;
    } else {
      next.calculations = calculations;
    }
  }
  if (patch.openIn !== undefined) {
    take("openIn");
    if (patch.openIn === null) {
      delete next.openIn;
    } else {
      next.openIn = patch.openIn;
    }
  }
  if (patch.dateBy !== undefined) {
    const date = patch.dateBy === null ? undefined : nonEmptyText(patch.dateBy);
    if (date === undefined) {
      delete next.dateBy;
    } else {
      next.dateBy = date;
    }
  }
  if (patch.endBy !== undefined) {
    const end = patch.endBy === null ? undefined : nonEmptyText(patch.endBy);
    if (end === undefined) {
      delete next.endBy;
    } else {
      next.endBy = end;
    }
  }
  if (patch.cardSize !== undefined) {
    take("cardSize");
    if (patch.cardSize === null) {
      delete next.cardSize;
    } else {
      next.cardSize = patch.cardSize;
    }
  }
  if (patch.defaultTemplate !== undefined) {
    take("defaultTemplate");
    const template = patch.defaultTemplate === null ? undefined : nonEmptyText(patch.defaultTemplate);
    if (template === undefined) {
      delete next.defaultTemplate;
    } else {
      next.defaultTemplate = template;
    }
  }
  if (patch.widgets !== undefined) {
    take("widgets");
    const widgets = patch.widgets === null ? [] : patch.widgets.slice(0, MAX_DASHBOARD_WIDGETS);
    if (widgets.length === 0) {
      delete next.widgets;
    } else {
      next.widgets = widgets;
    }
  }
  if (Object.keys(extra).length > 0) {
    next.extra = extra;
  } else {
    delete next.extra;
  }
  return JSON.stringify(viewToJson(next)) === JSON.stringify(viewToJson(view)) ? view : next;
}

/** The label of a view's tab: its own name, or (when empty) the name of its layout. */
export function viewLabel(view: PageListViewSpec, layoutName: (layout: PageListLayout) => string): string {
  return view.name.trim() === "" ? layoutName(view.layout) : view.name;
}

/** Whether another view fits (20 in all). */
export function canAddView(views: readonly PageListViewSpec[]): boolean {
  return views.length < MAX_VIEWS;
}

/**
 * The view to show: the one with id `activeId` (the device's choice, never synced), else the first. Never `undefined`
 * for a non-empty list (`parseViews` makes one).
 */
export function resolveActiveView(views: readonly PageListViewSpec[], activeId: string | null | undefined): PageListViewSpec {
  const [first = emptyView("list", DEFAULT_VIEW_ID)] = views;
  return (activeId === null || activeId === undefined ? undefined : views.find((view) => view.id === activeId)) ?? first;
}

/**
 * A fresh view id: 8 random lowercase hexadecimal characters, not in `taken`. (`crypto.randomUUID` where the
 * runtime has it, else `Math.random`: an id only has to differ from the block's other views.)
 */
export function newViewId(taken: Iterable<string>): string {
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

/** The list with a new empty view of `layout` at the end; the same list when 20 views are there. */
export function addView(views: readonly PageListViewSpec[], layout: PageListLayout, id: string, name = ""): PageListViewSpec[] {
  return canAddView(views) ? [...views, emptyView(layout, id, name)] : [...views];
}

/** The list with a copy of the view `id` (new id, `name`) right after it; the same list when 20 views are there or `id` is unknown. */
export function duplicateView(views: readonly PageListViewSpec[], id: string, newId: string, name: string): PageListViewSpec[] {
  const at = views.findIndex((view) => view.id === id);
  const source = views[at];
  if (source === undefined || !canAddView(views)) {
    return [...views];
  }
  const copy: PageListViewSpec = { ...source, id: newId, name: name.slice(0, MAX_VIEW_NAME_CHARS) };
  return [...views.slice(0, at + 1), copy, ...views.slice(at + 1)];
}

/** The list without the view `id`; the same list when it is the only view (a block keeps one) or `id` is unknown. */
export function removeView(views: readonly PageListViewSpec[], id: string): PageListViewSpec[] {
  if (views.length <= 1 || !views.some((view) => view.id === id)) {
    return [...views];
  }
  return views.filter((view) => view.id !== id);
}

/** The list with the view `id` one place earlier (`delta` -1) or later (1); the same list at either end or for an unknown id. */
export function moveView(views: readonly PageListViewSpec[], id: string, delta: -1 | 1): PageListViewSpec[] {
  const at = views.findIndex((view) => view.id === id);
  const to = at + delta;
  const moved = views[at];
  const other = views[to];
  if (moved === undefined || other === undefined) {
    return [...views];
  }
  const next = [...views];
  next[at] = other;
  next[to] = moved;
  return next;
}

/** The list with `patch` applied to the view `id`; the same list for an unknown id. */
export function updateView(views: readonly PageListViewSpec[], id: string, patch: PageListViewPatch): PageListViewSpec[] {
  return views.map((view) => (view.id === id ? patchView(view, patch) : view));
}
