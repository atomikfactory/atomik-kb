// The rows of a view: the pipeline of the database block (ADR-0054, `docs/architecture/database-views.md` section 4;
// ADR-0055 for the dashboard's embedded views), taken out of `PageListView` so that an embedded view widget runs it too.
// The pages of the source pass the filter tree (`applyFilterTree`, AND and OR, one level of groups) and are sorted as the
// view says (its sort keys, ADR-0058); a dashboard's embedded view first passes the dashboard's own filter (`outerFilter`),
// then its own.
//
// What the filters and the sort need besides the pages is read here, from the app's store: the viewer's own account (a
// "me" filter is theirs, not the one who saved the view), the names of the people and of the devices, and the titles
// of the pages (the sort by a relation, the text filters on a formula), and the viewer's day and first day of the week
// ("today", "this week": `useToday` redraws the rows at midnight). `lookups` is returned for the block's other
// needs (the title of the page that holds it, for the name of the CSV file).

import type { PageInfo, PropertyDef, SpaceId } from "@atomik-kb/core-client";
import { useMemo } from "react";

import { useMyAccountId } from "../comments-hooks";
import { activeLocale } from "../i18n";
import { deviceLabel } from "../property-format";
import { useAppStore } from "../store-context";
import { firstDayOfWeek } from "./dates";
import { indexOf } from "./node-views";
import { applyFilterTree } from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import { sortPages } from "./page-list-model";
import type { SortLookups } from "./page-list-model";
import type { PageListViewSpec } from "./page-list-views";
import { useToday } from "./today";

/** What the filters and the sort read besides the pages. */
export interface RowLookups {
  /** The viewer's own account in the space; `undefined` while it is not known (a "me" filter is then not applicable). */
  readonly me: string | undefined;
  /** The titles of pages and the names of devices and people, as the sort and the filters read them. */
  readonly lookups: SortLookups;
}

/** The viewer's account and the lookups of the space `spaceId` (`null` before the tree is read), read from the app's store. */
export function useRowLookups(spaceId: SpaceId | null): RowLookups {
  const tree = useAppStore((state) => state.tree);
  const devices = useAppStore((state) => state.devices);
  const device = useAppStore((state) => state.device);
  // The viewer's own account (a "me" filter is theirs, not the one who saved the view) and the names of the people.
  const me = useMyAccountId(spaceId);
  const people = useAppStore((state) => (spaceId === null ? undefined : state.people[spaceId]));
  const lookups = useMemo<SortLookups>(() => {
    const index = tree === null ? null : indexOf(tree);
    const own = device === null ? null : { deviceId: device.deviceId, name: device.name };
    return {
      pageTitle: (id) => index?.get(id)?.title ?? null,
      deviceName: (id) => deviceLabel(id, devices, own),
      personName: (id) => people?.find((person) => person.accountId === id)?.name ?? "",
    };
  }, [tree, devices, device, people]);
  return useMemo(() => ({ me, lookups }), [me, lookups]);
}

/** What `useViewRows` answers. */
export interface ViewRows {
  /** The pages after the filters, sorted as the view says. */
  readonly rows: PageInfo[];
  /** The titles of pages and the names of devices and people, as the sort and the filters read them. */
  readonly lookups: SortLookups;
}

/**
 * The rows of a view: `pages` after `outerFilter` (when there is one: a dashboard's filter over an embedded view), after
 * the view's own filter, sorted by the view's sort keys. `defs` is the space's property schema, `spaceId` the space the
 * pages are in (`null` before the tree is read).
 */
export function useViewRows(
  pages: readonly PageInfo[],
  view: Pick<PageListViewSpec, "filter" | "sorts">,
  defs: readonly PropertyDef[],
  spaceId: SpaceId | null,
  outerFilter?: FilterGroup,
): ViewRows {
  const { me, lookups } = useRowLookups(spaceId);
  // The viewer's calendar, for the relative days and periods of the filters: the rows are worked out again at midnight.
  const today = useToday();
  const locale = activeLocale();
  const weekStart = useMemo(() => firstDayOfWeek(locale), [locale]);
  const { filter, sorts } = view;
  const rows = useMemo(() => {
    const context = { defs, me, lookups, today, weekStart };
    const outer = outerFilter === undefined ? pages : applyFilterTree(pages, outerFilter, context);
    return sortPages(applyFilterTree(outer, filter, context), sorts, defs, lookups);
  }, [pages, outerFilter, filter, sorts, defs, lookups, me, today, weekStart]);
  return { rows, lookups };
}
