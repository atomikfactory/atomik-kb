// The filter tree of a view (ADR-0054, amending ADR-0016's flat AND list of 2026-10-08): a group that
// combines filters with AND or OR, one level deep. The top-level group holds filters and nested groups, a
// nested group holds filters only. Everything here is pure: the shapes, the lenient reading, the limits, the
// editing operations the filter editor uses and the evaluation, which reuses the per-filter matcher of
// `page-list-filters.ts` (`matchFilter`) and so cannot disagree with it about a single filter.
//
// Evaluation (the binding rules, `docs/architecture/database-views.md` section 3):
//
// - A **filter** that cannot be applied (its property is gone, its operator is not one of the property's
//   kind, a value it needs is missing) is *not applicable*, as `matchFilter` says (`null`).
// - A **group** is not applicable when none of its items is. Otherwise it is the AND (every applicable item)
//   or the OR (some applicable item) of the applicable items: **an item that cannot be applied is skipped,
//   it is neither true nor false**. So `A or <half-made filter>` is `A`, not "every page", and a filter
//   being typed never makes the list jump.
// - A page passes a tree unless the tree says `false`: a tree that is not applicable (an empty group, a group
//   of half-made filters) hides nothing, as an empty filter list never did.
// - Relative days and periods ("today", "this week", ADR-0058) are resolved at the context's `today` and `weekStart`
//   (the viewer's own day and first day of the week), so a shared view means "this week" for each reader.
//
// Reading is lenient and never rewrites (the rule of the whole `pageList` block): `normaliseFilterGroup` is
// what a stored value becomes in memory; the document is only replaced by a control the person uses.

import type { PageInfo, PropertyDef } from "@atomik-kb/core-client";

import type { FormulaLookups } from "../formula-format";
import { firstDayOfWeek, todayIso } from "./dates";
import { matchFilter, readFilters } from "./page-list-filters";
import type { PageListFilter } from "./page-list-filters";

export type FilterGroupOp = "and" | "or";

/** A group of filters. The top-level one may nest groups (one level); a nested one holds filters only. */
export interface FilterGroup {
  readonly op: FilterGroupOp;
  readonly items: readonly FilterNode[];
}

/** An item of a group: a filter, or (in the top-level group only) a group. */
export type FilterNode = PageListFilter | FilterGroup;

/** Most filters a view can hold, counted over the top-level group and every nested one. */
export const MAX_FILTERS = 50;
/** Most nested groups a view's top-level group can hold (bounds the size of the attribute; ADR-0054). */
export const MAX_FILTER_GROUPS = 10;

/** The group of a view without filters: AND of nothing. It is also the default, so it is never stored. */
export const EMPTY_FILTER_GROUP: FilterGroup = { op: "and", items: [] };

/** Where a filter or a group is in the tree: the index in the top-level group, then (inside a nested group) the index there. */
export type FilterPath = readonly [number] | readonly [number, number];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Whether a node is a group (by its shape: an `items` list and the operator `and` or `or`). */
export function isFilterGroup(node: unknown): node is FilterGroup {
  return isRecord(node) && Array.isArray(node.items) && (node.op === "and" || node.op === "or");
}

/** The number of filters of a group, nested groups included. */
export function countFilters(group: FilterGroup): number {
  return group.items.reduce((total, item) => total + (isFilterGroup(item) ? countFilters(item) : 1), 0);
}

/** The number of groups nested in a group. */
export function countGroups(group: FilterGroup): number {
  return group.items.filter(isFilterGroup).length;
}

/** Whether the group is exactly the default (AND of nothing): the only value a view does not store. */
export function isEmptyFilterGroup(group: FilterGroup): boolean {
  return group.op === "and" && group.items.length === 0;
}

/**
 * What a stored value becomes: a group stays a group; a list (the flat `filters` of a block written before
 * ADR-0054) becomes an AND group of its filters; anything else is the empty group. Entries that are no filter
 * and no group are dropped, a group inside a nested group is dropped (one level only), and the limits apply in
 * reading order: the first 50 filters and the first 10 groups are kept, the rest ignored (not rewritten).
 */
export function normaliseFilterGroup(value: unknown): FilterGroup {
  if (Array.isArray(value)) {
    return legacyFiltersToGroup(readFilters(value));
  }
  if (!isFilterGroup(value)) {
    return EMPTY_FILTER_GROUP;
  }
  let filters = 0;
  let groups = 0;
  const items: FilterNode[] = [];
  for (const item of value.items as readonly unknown[]) {
    if (isFilterGroup(item)) {
      if (groups >= MAX_FILTER_GROUPS) {
        continue;
      }
      groups += 1;
      const inner: PageListFilter[] = [];
      for (const filter of readFilters((item.items as readonly unknown[]).filter((entry) => !isFilterGroup(entry)))) {
        if (filters >= MAX_FILTERS) {
          break;
        }
        filters += 1;
        inner.push(filter);
      }
      items.push({ op: item.op, items: inner });
    } else {
      const [filter] = readFilters([item]);
      if (filter !== undefined && filters < MAX_FILTERS) {
        filters += 1;
        items.push(filter);
      }
    }
  }
  return { op: value.op, items };
}

/** The flat AND list of a block written before ADR-0054, as a group (the first 50 filters). */
export function legacyFiltersToGroup(filters: readonly PageListFilter[]): FilterGroup {
  return filters.length === 0 ? EMPTY_FILTER_GROUP : { op: "and", items: filters.slice(0, MAX_FILTERS) };
}

/** The filters that sit directly in the top-level group (the nested groups left out): what the legacy mirror carries. */
export function plainFilters(group: FilterGroup): PageListFilter[] {
  return group.items.filter((item): item is PageListFilter => !isFilterGroup(item));
}

/**
 * What an older build, which knows the flat `filters` only, is told: the plain filters of the top-level group
 * when it is AND (nested groups left out, so it may show more than the group does), nothing when it is OR (an OR
 * cannot be said as a flat list: the older build shows every page).
 */
export function legacyFilters(group: FilterGroup): PageListFilter[] {
  return group.op === "and" ? plainFilters(group) : [];
}

/** A filter as a fresh plain object: `propertyId`, `op` and, when there is one, `value`. */
export function copyFilter(filter: PageListFilter): PageListFilter {
  return "value" in filter ? { propertyId: filter.propertyId, op: filter.op, value: filter.value } : { propertyId: filter.propertyId, op: filter.op };
}

/** A group as the plain JSON that is stored: `op` and `items` (filters as fresh objects, nested groups the same way). */
export function filterGroupToJson(group: FilterGroup): Record<string, unknown> {
  return {
    op: group.op,
    items: group.items.map((item) => (isFilterGroup(item) ? filterGroupToJson(item) : copyFilter(item))),
  };
}

// ----- evaluation ----------------------------------------------------------------------------

/** What evaluating a filter needs besides the page: the space's schema, the viewer's own account and their calendar. */
export interface FilterContext {
  readonly defs: readonly PropertyDef[];
  /** The viewer's account in the page's space; `undefined` while it is not known (a "me" filter is then not applicable). */
  readonly me?: string;
  /** The titles of pages and names of people a text filter on a formula's result reads (they have no text without it). */
  readonly lookups?: FormulaLookups;
  /** The viewer's local day (`YYYY-MM-DD`) that relative days and periods are resolved at (ADR-0058); the device's own day when left out. */
  readonly today?: string;
  /** The first day of the viewer's week, 0 (Sunday) to 6, for "this week" and the like; the locale's first day when left out. */
  readonly weekStart?: number;
}

/**
 * Whether `page` passes the group: `true` or `false`, or `null` when nothing in the group can be applied (see the
 * rules at the top of this file).
 */
export function matchFilterGroup(page: PageInfo, group: FilterGroup, context: FilterContext): boolean | null {
  let any = false;
  let every = true;
  let some = false;
  for (const item of group.items) {
    const result = isFilterGroup(item)
      ? matchFilterGroup(page, item, context)
      : matchFilter(page, item, context.defs, context.me, context.lookups, { today: context.today, weekStart: context.weekStart });
    if (result === null) {
      continue;
    }
    any = true;
    every &&= result;
    some ||= result;
  }
  if (!any) {
    return null;
  }
  return group.op === "and" ? every : some;
}

/** The pages that pass the tree (everything when it says nothing), in their order. Today and the week start are read once for all the pages. */
export function applyFilterTree(pages: readonly PageInfo[], group: FilterGroup, context: FilterContext): PageInfo[] {
  if (group.items.length === 0) {
    return [...pages];
  }
  const resolved: FilterContext = { ...context, today: context.today ?? todayIso(), weekStart: context.weekStart ?? firstDayOfWeek() };
  return pages.filter((page) => matchFilterGroup(page, group, resolved) !== false);
}

// ----- editing -------------------------------------------------------------------------------
// The filter editor's operations, each answering a new group (or the same one when the rule forbids the change:
// the limits above, a nested group inside a nested group). Pure, so the rules are tested here and the editor
// only calls them.

/** Whether one more filter fits (50 in all). */
export function canAddFilter(group: FilterGroup): boolean {
  return countFilters(group) < MAX_FILTERS;
}

/** Whether one more nested group fits (10 in all). */
export function canAddGroup(group: FilterGroup): boolean {
  return countGroups(group) < MAX_FILTER_GROUPS;
}

/**
 * Adds `filter` at the end of the top-level group (`into` is `null`) or of the nested group at index `into` of
 * the top-level group. The same group when the 50 filters are used or `into` is not a nested group.
 */
export function addFilter(group: FilterGroup, filter: PageListFilter, into: number | null = null): FilterGroup {
  if (!canAddFilter(group)) {
    return group;
  }
  if (into === null) {
    return { op: group.op, items: [...group.items, filter] };
  }
  const target = group.items[into];
  if (!isFilterGroup(target)) {
    return group;
  }
  return { op: group.op, items: group.items.map((item, at) => (at === into ? { op: target.op, items: [...target.items, filter] } : item)) };
}

/** Adds an empty nested group at the end, with the operator opposite to the top-level one (an AND in an AND says nothing). */
export function addGroup(group: FilterGroup): FilterGroup {
  if (!canAddGroup(group)) {
    return group;
  }
  return { op: group.op, items: [...group.items, { op: group.op === "and" ? "or" : "and", items: [] }] };
}

/** Changes the operator of the top-level group (`into` is `null`) or of the nested group at index `into`. */
export function setGroupOp(group: FilterGroup, op: FilterGroupOp, into: number | null = null): FilterGroup {
  if (into === null) {
    return { op, items: group.items };
  }
  return { op: group.op, items: group.items.map((item, at) => (at === into && isFilterGroup(item) ? { op, items: item.items } : item)) };
}

/** Replaces the filter at `path`; the same group when `path` does not name a filter. */
export function replaceFilter(group: FilterGroup, path: FilterPath, filter: PageListFilter): FilterGroup {
  const [first, second] = path;
  const item = group.items[first];
  if (second === undefined) {
    return item === undefined || isFilterGroup(item) ? group : { op: group.op, items: group.items.map((entry, at) => (at === first ? filter : entry)) };
  }
  if (!isFilterGroup(item) || item.items[second] === undefined) {
    return group;
  }
  const inner = { op: item.op, items: item.items.map((entry, at) => (at === second ? filter : entry)) };
  return { op: group.op, items: group.items.map((entry, at) => (at === first ? inner : entry)) };
}

/** Removes the filter at `path`, or (a one-index path that names a nested group) the group with its filters. */
export function removeAt(group: FilterGroup, path: FilterPath): FilterGroup {
  const [first, second] = path;
  const item = group.items[first];
  if (item === undefined) {
    return group;
  }
  if (second === undefined) {
    return { op: group.op, items: group.items.filter((_, at) => at !== first) };
  }
  if (!isFilterGroup(item) || item.items[second] === undefined) {
    return group;
  }
  const inner = { op: item.op, items: item.items.filter((_, at) => at !== second) };
  return { op: group.op, items: group.items.map((entry, at) => (at === first ? inner : entry)) };
}
