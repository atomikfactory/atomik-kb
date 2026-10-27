// How a click on a row, card or chip of a database view opens its page (ADR-0054,
// `docs/architecture/database-views.md` section 8). One function decides, so that the list, the table, the board,
// the calendar and the gallery behave alike:
//
// - A gesture that asks for another tab (Ctrl/Cmd+click and middle click: `"background"`, Ctrl/Cmd+Shift+click:
//   `"foreground"`) keeps its meaning whatever the view's `openIn` says: the page opens in a tab (`navigateTo`).
// - A plain click (`"current"`) follows the view's `openIn`: `"page"` navigates the tab in front as before,
//   `"side"` (the default) opens the side peek and `"center"` the large dialog over the page, both through
//   `openPeek`, which swaps the page when a peek is already open.
// - **The page that holds the database cannot be peeked** (a tag source can list it): it opens as a page. Neither
//   can the page the tab in front shows (it is already on screen in the editor, and a page is never edited in two
//   places): it opens as a page, which closes the peek. A page of another space than the one in front opens as a
//   page too (the peek shows pages of the current space).
// - **Nothing opens for a page that is gone or a space that was left** (a row clicked in the moment between the page
//   being trashed, or the space being left, and the list catching up): there is no page to show and no tab to open.
//
// The row's own keyboard confirmation passes `dispositionFromKey` (Enter is `"current"`, Ctrl/Cmd+Enter
// `"foreground"`), the mouse passes what `openGestures` computes.

import type { PageId, SpaceId } from "@atomik-kb/core-client";

import type { AppState } from "../store";
import type { OpenDisposition } from "../store-tabs";
import { DEFAULT_OPEN_IN } from "./page-list-views";
import type { PageListViewSpec } from "./page-list-views";

/**
 * What `openRow` reads and calls on the store (so a test can pass a fake). The four members are needed; the others
 * make the guards (a fake without them is never guarded).
 */
export type OpenRowStore = Pick<AppState, "navigateTo" | "openPeek" | "closePeek" | "selectedPageId"> &
  Partial<Pick<AppState, "currentSpaceId" | "spaces" | "pageLookup">>;

/** The page a row stands for. */
export interface OpenRowTarget {
  readonly pageId: PageId;
  readonly spaceId: SpaceId;
}

/** False for a page the store knows is in the trash or purged, and for a space this account is no longer in. */
function openable(store: OpenRowStore, { pageId, spaceId }: OpenRowTarget): boolean {
  if (store.spaces !== undefined && store.spaces.length > 0 && !store.spaces.some((space) => space.id === spaceId)) {
    return false;
  }
  const lookup = store.pageLookup?.[pageId];
  return !(lookup?.status === "missing" || (lookup?.status === "found" && lookup.page.trashedAt !== undefined));
}

/**
 * Opens the page of a row. `hostPageId` is the page whose editor holds the database block (`null` when unknown),
 * which is never peeked.
 */
export function openRow(
  store: OpenRowStore,
  target: OpenRowTarget,
  view: Pick<PageListViewSpec, "openIn">,
  disposition: OpenDisposition,
  hostPageId: PageId | null,
): void {
  const { pageId, spaceId } = target;
  if (!openable(store, target)) {
    return;
  }
  if (disposition !== "current") {
    store.navigateTo({ pageId, spaceId, disposition });
    return;
  }
  const openIn = view.openIn ?? DEFAULT_OPEN_IN;
  const elsewhere = store.currentSpaceId !== undefined && store.currentSpaceId !== null && spaceId !== store.currentSpaceId;
  if (openIn === "page" || pageId === hostPageId || pageId === store.selectedPageId || elsewhere) {
    store.closePeek();
    store.navigateTo({ pageId, spaceId, disposition: "current" });
    return;
  }
  store.openPeek({ pageId, spaceId, mode: openIn });
}
