// The registry of the layouts of a database block (ADR-0054, `docs/architecture/database-views.md` section 4): for
// each layout its name, its icon, the component that draws the rows and the settings section it adds to the
// toolbar. `PageListView` renders `layoutDef(view.layout).Component`, the toolbar renders its `Settings`, the
// layout select and the "add a view" menu list `PAGE_LIST_LAYOUT_DEFS` in order. Adding a layout means adding
// a row here and a component; nothing else of the block changes.
//
// Ownership: the calendar and the gallery are agents C and D's components and these two entries do not change; the
// list, the table and the board are adapters over the same props (`PageListRows`, `PageTableView`,
// `PageBoardView`), so every layout is drawn by the component its entry names. The settings sections (the board's
// group-by and columns, the calendar's dates, the gallery's card size) are drawn in the toolbar's view settings menu.
// A layout may also have an `initialPatch` (the feed's newest-first sort, ADR-0058): what a view of that layout starts with.

import type { ComponentType } from "react";

import {
  BoardViewIcon,
  CalendarViewIcon,
  DashboardViewIcon,
  FeedViewIcon,
  GalleryViewIcon,
  ListViewIcon,
  TableViewIcon,
} from "../components/icons";
import type { MessageKey } from "../i18n";
import { PageDashboardView } from "./dashboard/PageDashboardView";
import { PageBoardSettings } from "./PageBoardSettings";
import { PageBoardView } from "./PageBoardView";
import { PageCalendarSettings, PageCalendarView } from "./PageCalendarView";
import { PageFeedView } from "./PageFeedView";
import { PageGallerySettings, PageGalleryView } from "./PageGalleryView";
import type { PageListLayoutProps, PageListLayoutSettingsProps } from "./page-list-layout-types";
import { DEFAULT_PAGE_LIST_SORT } from "./page-list-model";
import type { PageListLayout } from "./page-list-model";
import { updateView } from "./page-list-views";
import type { PageListViewPatch, PageListViewSpec } from "./page-list-views";
import { PageListRows } from "./PageListRows";
import { PageTableView } from "./PageTableView";

/** What the registry knows about a layout. */
export interface PageListLayoutDef {
  readonly layout: PageListLayout;
  /** The name of the layout (the picker, the tab of an unnamed view). */
  readonly labelKey: MessageKey;
  readonly Icon: ComponentType;
  /** Draws the rows (and the layout's own controls inside the block: the table's header and footer, the board's columns). */
  readonly Component: ComponentType<PageListLayoutProps>;
  /** The layout's part of the view settings menu, shown while the layout is active; `null` when it has none. */
  readonly Settings: ComponentType<PageListLayoutSettingsProps> | null;
  /** Toolbar buttons the layout has no use for (ADR-0055: a dashboard has no Sort and no Properties); absent: all shown. */
  readonly toolbar?: { readonly sort?: boolean; readonly properties?: boolean };
  /**
   * What a view of this layout starts with besides the layout itself (ADR-0058: a feed sorts by created time, newest first).
   * Applied to a view added with this layout, and to a view whose layout is changed to it while it still has the default sort
   * (`layoutChangePatch`, `withInitialPatch`); absent: nothing.
   */
  readonly initialPatch?: PageListViewPatch;
}

/** Every layout in the order the pickers list them. */
export const PAGE_LIST_LAYOUT_DEFS: readonly PageListLayoutDef[] = [
  { layout: "list", labelKey: "pageList.layout.list", Icon: ListViewIcon, Component: PageListRows, Settings: null },
  { layout: "table", labelKey: "pageList.layout.table", Icon: TableViewIcon, Component: PageTableView, Settings: null },
  {
    layout: "board",
    labelKey: "pageList.layout.board",
    Icon: BoardViewIcon,
    Component: PageBoardView,
    Settings: PageBoardSettings,
  },
  {
    layout: "calendar",
    labelKey: "pageList.layout.calendar",
    Icon: CalendarViewIcon,
    Component: PageCalendarView,
    Settings: PageCalendarSettings,
  },
  {
    layout: "gallery",
    labelKey: "pageList.layout.gallery",
    Icon: GalleryViewIcon,
    Component: PageGalleryView,
    Settings: PageGallerySettings,
  },
  {
    layout: "feed",
    labelKey: "pageList.layout.feed",
    Icon: FeedViewIcon,
    Component: PageFeedView,
    Settings: null,
    initialPatch: { sorts: [{ by: "created", dir: "desc" }] },
  },
  {
    layout: "dashboard",
    labelKey: "pageList.layout.dashboard",
    Icon: DashboardViewIcon,
    Component: PageDashboardView,
    Settings: null,
    toolbar: { sort: false, properties: false },
  },
];

/** Only for the type checker: `PAGE_LIST_LAYOUT_DEFS` is never empty. */
const FALLBACK: PageListLayoutDef = {
  layout: "list",
  labelKey: "pageList.layout.list",
  Icon: ListViewIcon,
  Component: PageListRows,
  Settings: null,
};

/** The registry entry of a layout (the list's for a layout it does not have). */
export function layoutDef(layout: PageListLayout): PageListLayoutDef {
  return PAGE_LIST_LAYOUT_DEFS.find((def) => def.layout === layout) ?? PAGE_LIST_LAYOUT_DEFS[0] ?? FALLBACK;
}

/** Whether the view still orders its rows as a new view does (one key, the title, A to Z): its sort was never chosen. */
export function hasDefaultSort(view: Pick<PageListViewSpec, "sorts">): boolean {
  const [only, ...rest] = view.sorts;
  return rest.length === 0 && (only === undefined || (only.by === DEFAULT_PAGE_LIST_SORT.by && only.dir === DEFAULT_PAGE_LIST_SORT.dir));
}

/**
 * The patch that changes `view` to `layout`: the layout, and the layout's `initialPatch` when the view still has the default
 * sort (a sort the person chose is not replaced). The layout alone for a layout with no `initialPatch`.
 */
export function layoutChangePatch(view: Pick<PageListViewSpec, "sorts">, layout: PageListLayout): PageListViewPatch {
  const initial = PAGE_LIST_LAYOUT_DEFS.find((def) => def.layout === layout)?.initialPatch;
  return initial !== undefined && hasDefaultSort(view) ? { ...initial, layout } : { layout };
}

/**
 * `views` (the list `addView` made) with the view `id` given the `initialPatch` of its layout; the same list when the view is
 * not there or its layout has none.
 */
export function withInitialPatch(views: readonly PageListViewSpec[], id: string): PageListViewSpec[] {
  const added = views.find((view) => view.id === id);
  const initial = added === undefined ? undefined : PAGE_LIST_LAYOUT_DEFS.find((def) => def.layout === added.layout)?.initialPatch;
  return initial === undefined ? [...views] : updateView(views, id, initial);
}
