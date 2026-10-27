// What a change of a view writes to its block (ADR-0054, `docs/architecture/database-views.md` section 2.4, "who
// writes what"). Pure functions over the block's attributes and the views they hold, so the rule is tested here and
// `PageListView.tsx` only calls it.
//
// - A block that **has `views`** writes every change through them: `views` and the mirror of the first view, in one
//   `updateAttributes` (`writeViews`).
// - A block that **has none** (every block written before ADR-0054) keeps writing one legacy attribute at a time, as
//   before, **while the change is expressible in them**: a layout the older attributes name (list, table, board), a
//   flat AND list of filters, one sort key, no name, no `openIn`, `dateBy`, `endBy`, `cardSize` or `defaultTemplate`, no second view.
//   Older builds keep full fidelity and existing pages are not rewritten by a click.
// - Anything else (the first extra view, an OR or a filter group, a second sort key, a calendar or gallery setting, a changed
//   `openIn`, a name, a layout the older attributes cannot hold) upgrades the block: `views` and the mirror are written.
//
// The block's `source` is not part of a view and is written alone by the caller.

import { isFilterGroup, plainFilters } from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import { parseView, patchView, writeViews } from "./page-list-views";
import type { PageListViewPatch, PageListViewSpec } from "./page-list-views";

/** The layouts the attributes of a block written before ADR-0054 can hold. */
const LEGACY_LAYOUTS: readonly string[] = ["list", "table", "board"];

/** Whether a block's `views` attribute holds at least one view this version can read (so the block is read through them). */
export function hasViews(attrs: Readonly<Record<string, unknown>>): boolean {
  const stored = attrs.views;
  return Array.isArray(stored) && (stored as readonly unknown[]).some((entry) => parseView(entry) !== null);
}

/** Whether a filter group is what the flat `filters` attribute can say: AND, and no nested group. */
export function isFlatAnd(group: FilterGroup): boolean {
  return group.op === "and" && !group.items.some(isFilterGroup);
}

/** Whether everything the view says fits the legacy attributes of the block. */
export function fitsLegacyAttributes(view: PageListViewSpec): boolean {
  return (
    LEGACY_LAYOUTS.includes(view.layout) &&
    view.name === "" &&
    view.sorts.length <= 1 &&
    view.openIn === undefined &&
    view.dateBy === undefined &&
    view.endBy === undefined &&
    view.cardSize === undefined &&
    view.defaultTemplate === undefined &&
    view.widgets === undefined &&
    view.extra === undefined &&
    isFlatAnd(view.filter)
  );
}

/** The legacy attributes a patch names, with the values the patched view holds (`null` removes an optional one). */
function legacyAttributes(patch: PageListViewPatch, next: PageListViewSpec): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (patch.layout !== undefined) {
    out.layout = next.layout;
  }
  if (patch.properties !== undefined) {
    out.properties = [...next.properties];
  }
  if (patch.sort !== undefined || patch.sorts !== undefined) {
    // The legacy attribute is the first key (a second key does not fit it: `fitsLegacyAttributes`).
    out.sort = { by: next.sort.by, dir: next.sort.dir };
  }
  if (patch.filter !== undefined) {
    out.filters = plainFilters(next.filter).map((filter) => ({ ...filter }));
  }
  if (patch.columnWidths !== undefined) {
    out.columnWidths = next.columnWidths === undefined ? null : { ...next.columnWidths };
  }
  if (patch.groupBy !== undefined) {
    out.groupBy = next.groupBy ?? null;
  }
  if (patch.hiddenGroups !== undefined) {
    out.hiddenGroups = next.hiddenGroups === undefined ? null : [...next.hiddenGroups];
  }
  if (patch.calculations !== undefined) {
    out.calculations = next.calculations === undefined ? null : { ...next.calculations };
  }
  return out;
}

/**
 * The attributes to write for `patch` applied to the view `viewId`, or `null` when there is nothing to write (an
 * unknown view, or a patch that changes nothing). See the top of this file for which attributes.
 */
export function writeForPatch(
  attrs: Readonly<Record<string, unknown>>,
  views: readonly PageListViewSpec[],
  viewId: string,
  patch: PageListViewPatch,
): Record<string, unknown> | null {
  const view = views.find((candidate) => candidate.id === viewId);
  if (view === undefined) {
    return null;
  }
  const next = patchView(view, patch);
  if (next === view) {
    return null;
  }
  if (!hasViews(attrs) && views.length === 1 && fitsLegacyAttributes(next)) {
    return legacyAttributes(patch, next);
  }
  return { ...writeViews(views.map((candidate) => (candidate === view ? next : candidate))) };
}

/** The attributes to write for a new list of views (a view added, copied, removed, moved, renamed): `views` and the mirror. */
export function writeForViews(views: readonly PageListViewSpec[]): Record<string, unknown> {
  return { ...writeViews(views) };
}

/** Whether two lists hold the very same views (what the list operations answer when they change nothing). */
export function sameViews(a: readonly PageListViewSpec[], b: readonly PageListViewSpec[]): boolean {
  return a.length === b.length && a.every((view, at) => view === b[at]);
}
