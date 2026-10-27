// The contract between the database block and its layouts (ADR-0054, `docs/architecture/database-views.md`
// section 4): every layout component takes the same props, and may bring a settings section for the toolbar.
// `PageListView` builds the props once (the rows after filters and sort, the active view, the schema) and renders
// the component the registry (`page-list-layouts.tsx`) names for the view's layout, so a layout knows nothing of
// the block, the tabs, the filter editor or the peek: it draws `rows`, asks to open a page through `onOpen`, writes
// its own settings through `onUpdateView` and makes pages through `onCreatePage`.
//
// A layout never writes the block's attributes and never opens a page itself (`openRow` decides between the side
// peek, the center peek, the page and another tab from the view's `openIn` and the gesture). It may write the
// values of a page through the store (`setPageProperty`: a board moves a card, a calendar reschedules one).

import type { PageId, PageInfo, PropertyDef, PropertyId, PropertyValue, SpaceId } from "@atomik-kb/core-client";

import type { OpenDisposition } from "../store-tabs";
import type { PageListViewPatch, PageListViewSpec } from "./page-list-views";

/** What a page made by a layout starts with: the values of properties (a calendar day's date, a board column's value). */
export interface NewPageInit {
  readonly values?: readonly (readonly [PropertyId, PropertyValue])[];
  /**
   * Make the page from this template (ADR-0056) rather than empty; absent: the view's `defaultTemplate` when it has one
   * that still exists, else an empty page. `null`: an empty page whatever the default.
   */
  readonly templateId?: PageId | null;
}

/**
 * The selection of rows (ADR-0058, `docs/architecture/database-power-ups.md` section 5.1), offered to the list, the table and the
 * board of an editable editor; the other layouts ignore it. The block keeps the state (`PageListView`), the layout draws a checkbox
 * per row and reports the clicks.
 */
export interface RowSelectionProps {
  /** The rows ticked; a row the layout does not draw (beyond the row limit) can be in it. */
  readonly selected: ReadonlySet<PageId>;
  /** A row's checkbox was clicked; `range` is true for a Shift+click (from the last tick, in display order). */
  readonly onToggle: (pageId: PageId, options: { readonly range: boolean }) => void;
  /** The "Select all" of the table's header: every row of the view when not all are ticked, else none. */
  readonly onToggleAll: () => void;
}

/** The props of every layout component. */
export interface PageListLayoutProps {
  /** The pages of the block's source after the view's filters, sorted as the view says. */
  readonly rows: readonly PageInfo[];
  /** The active view (what the layout draws and the settings it reads). */
  readonly view: PageListViewSpec;
  /** The space's property schema, Tags first (empty while it loads). */
  readonly schema: readonly PropertyDef[];
  /** `view.properties` that the schema has, in order: the columns, or the values shown on a card. */
  readonly columns: readonly PropertyDef[];
  readonly spaceId: SpaceId;
  /** False in a read-only editor (a viewer, a locked page, content of a newer version): nothing may be changed. */
  readonly editable: boolean;
  /** The local day `YYYY-MM-DD` (`useToday`), for "Today" and "Tomorrow" in dates. */
  readonly today: string;
  /**
   * Opens a page the way the gesture asked: pass the disposition `openGestures` gives (a click is `"current"`,
   * Ctrl+click and a middle click `"background"`, Ctrl+Shift+click `"foreground"`; `dispositionFromKey` for Enter).
   * Allowed in read-only editors.
   */
  readonly onOpen: (page: PageInfo, disposition: OpenDisposition) => void;
  /** Changes the active view (one write of the block's attributes); read-only editors do not call it. */
  readonly onUpdateView: (patch: PageListViewPatch) => void;
  /**
   * Makes a page under the page that holds the block (carrying the tag of a tag source) with `init`'s values and
   * opens it like a row. `null` when "New page" is not offered (a read-only editor, no host page).
   */
  readonly onCreatePage: ((init?: NewPageInit) => void) | null;
  /** A page is being made: the "New page" controls are disabled. */
  readonly creating: boolean;
  /** The selection of rows; absent where it is not offered (a read-only editor, a layout without checkboxes). */
  readonly selection?: RowSelectionProps;
  // For the dashboard (ADR-0055, `docs/architecture/dashboards.md` section 3); the other layouts ignore them.
  /** Every view of the block, in tab order (the embedded view widgets draw another view). */
  readonly views?: readonly PageListViewSpec[];
  /** The pages of the block's source before any filter (an embedded view filters and sorts them itself). */
  readonly sourcePages?: readonly PageInfo[];
  /** Changes another view of the block (an embedded view's own settings); read-only editors do not call it. */
  readonly onUpdateOtherView?: (viewId: string, patch: PageListViewPatch) => void;
  /** Makes another view the active tab ("Open view"). */
  readonly onActivateView?: (viewId: string) => void;
}

/** The props of a layout's settings section in the toolbar (editable editors only). */
export interface PageListLayoutSettingsProps {
  readonly view: PageListViewSpec;
  readonly schema: readonly PropertyDef[];
  readonly onUpdateView: (patch: PageListViewPatch) => void;
}
