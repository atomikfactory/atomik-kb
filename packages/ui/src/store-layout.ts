// The device-local layout preferences of the shell (ADR-0029): the width of the sidebar and
// whether it is hidden, whether the favourites section is collapsed, and the recent pages.
// `store.ts` composes it with the rest.
//
// All of it lives in the shared `settings` cache (the keys below, `ui.*` namespace), is read
// once at boot so the first paint already has the right layout, and is never synced. The
// actions update the cache first and write the setting in the background, as the collapsed
// pages of the tree do: a failed write is logged and the layout keeps working for the session.

import type { CoreClient, Event as CoreEvent, JsonValue, PageId, SpaceId } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";
import { isInTemplate } from "./templates";

/** Settings of the sidebar layout (pixels, a boolean) and of the favourites section (a boolean). */
export const SIDEBAR_WIDTH_KEY = "ui.sidebar.width";
export const SIDEBAR_HIDDEN_KEY = "ui.sidebar.hidden";
export const FAVORITES_COLLAPSED_KEY = "ui.sidebar.favorites.collapsed";
/** Setting that holds the recent pages (a JSON array, most recent first). */
export const RECENT_PAGES_KEY = "ui.recent_pages";

export const SIDEBAR_MIN_WIDTH = 200;
export const SIDEBAR_MAX_WIDTH = 480;
export const SIDEBAR_DEFAULT_WIDTH = 272;
/** What an arrow key on the resize handle changes the width by. */
export const SIDEBAR_KEY_STEP = 16;

/** Most recent pages that are kept. */
export const RECENT_PAGES_MAX = 30;
/** Opening the page that is already first again within this time does not rewrite the setting. */
const RECENT_REFRESH_MS = 60_000;

/** Every key `load` reads at boot. */
const LAYOUT_KEYS = [SIDEBAR_WIDTH_KEY, SIDEBAR_HIDDEN_KEY, FAVORITES_COLLAPSED_KEY, RECENT_PAGES_KEY] as const;

/** Reads a stored width; anything that is not an integer from 200 to 480 means the default. */
export function parseSidebarWidth(value: JsonValue | null | undefined): number {
  return typeof value === "number" && Number.isInteger(value) && value >= SIDEBAR_MIN_WIDTH && value <= SIDEBAR_MAX_WIDTH
    ? value
    : SIDEBAR_DEFAULT_WIDTH;
}

/** A width the user chose (a drag, a key): rounded to whole pixels and held between 200 and 480. */
export function clampSidebarWidth(pixels: number): number {
  if (!Number.isFinite(pixels)) {
    return SIDEBAR_DEFAULT_WIDTH;
  }
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(pixels)));
}

/** One recent page. Titles and icons are not stored: they are looked up when shown. */
export interface RecentEntry {
  pageId: PageId;
  spaceId: SpaceId;
  /** When the page was last opened, Unix ms. */
  openedAt: number;
}

/** Reads the stored recent pages: a malformed entry is skipped, a page listed twice keeps its first place, at most 30. */
export function parseRecentPages(value: JsonValue | null | undefined): readonly RecentEntry[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const seen = new Set<PageId>();
  const entries: RecentEntry[] = [];
  for (const item of value) {
    if (typeof item !== "object" || item === null || Array.isArray(item)) {
      continue;
    }
    const { pageId, spaceId, openedAt } = item;
    if (
      typeof pageId !== "string" ||
      typeof spaceId !== "string" ||
      typeof openedAt !== "number" ||
      !Number.isFinite(openedAt) ||
      seen.has(pageId)
    ) {
      continue;
    }
    seen.add(pageId);
    entries.push({ pageId, spaceId, openedAt });
  }
  return entries.slice(0, RECENT_PAGES_MAX);
}

/**
 * The list after opening `opened`: it goes first (a page appears once), the pages `isGone`
 * names are dropped, and only the 30 most recent stay.
 */
export function withRecentPage(
  entries: readonly RecentEntry[],
  opened: RecentEntry,
  isGone: (pageId: PageId) => boolean,
): RecentEntry[] {
  const rest = entries.filter((entry) => entry.pageId !== opened.pageId && !isGone(entry.pageId));
  return [opened, ...rest].slice(0, RECENT_PAGES_MAX);
}

/**
 * The list after pages moved to another space (ADR-0038): their entries name the new space, in the same places.
 * The list itself when none of them is in it.
 */
export function withPagesInSpace(
  entries: readonly RecentEntry[],
  pageIds: ReadonlySet<PageId>,
  toSpaceId: SpaceId,
): readonly RecentEntry[] {
  const moves = (entry: RecentEntry): boolean => pageIds.has(entry.pageId) && entry.spaceId !== toSpaceId;
  return entries.some(moves) ? entries.map((entry) => (moves(entry) ? { ...entry, spaceId: toSpaceId } : entry)) : entries;
}

export interface LayoutActions {
  /** Sets the sidebar width (clamped to 200 to 480) and remembers it: one write per call. */
  setSidebarWidth: (width: number) => void;
  /** Hides or shows the sidebar and remembers it. */
  setSidebarHidden: (hidden: boolean) => void;
  toggleSidebar: () => void;
  /** Collapses or expands the favourites section and remembers it. */
  setFavoritesCollapsed: (collapsed: boolean) => void;
  /**
   * Notes that the page was opened: it moves to the front of the recent pages. Every way
   * of opening a page (`selectPage`, `createPage`, so also `navigateTo`) ends here. A template and a page inside one
   * (ADR-0056) are not noted: they are not in the sidebar, so they are not pages to come back to.
   */
  recordRecentPage: (pageId: PageId, spaceId: SpaceId) => void;
  /** Forgets every recent page; rejects with a `CoreError` when the write fails. */
  clearRecentPages: () => Promise<void>;
}

export type LayoutState = LayoutActions;

export const selectSidebarWidth = (state: AppState): number => parseSidebarWidth(state.settings[SIDEBAR_WIDTH_KEY]);
export const selectSidebarHidden = (state: AppState): boolean => state.settings[SIDEBAR_HIDDEN_KEY] === true;
export const selectFavoritesCollapsed = (state: AppState): boolean =>
  state.settings[FAVORITES_COLLAPSED_KEY] === true;

export interface LayoutSlice {
  /** The actions, to be part of the store's initial state. */
  initial: LayoutState;
  /** Reads the settings the layout needs, for the boot sequence; a key that cannot be read is left out. */
  load: () => Promise<Record<string, JsonValue | null>>;
  /** `PagesMovedSpace`: the recent pages that moved name their new space. */
  onEvent: (event: CoreEvent) => void;
}

export function createLayoutSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): LayoutSlice {
  /** Updates the cache at once and writes the setting in the background. */
  function remember(key: string, value: JsonValue, what: string): void {
    set((state) => ({ settings: { ...state.settings, [key]: value } }));
    client.settingsSet(key, value).catch((error: unknown) => {
      console.error(`could not remember ${what}`, error);
    });
  }

  const initial: LayoutState = {
    setSidebarWidth: (width) => {
      const next = clampSidebarWidth(width);
      if (selectSidebarWidth(get()) !== next) {
        remember(SIDEBAR_WIDTH_KEY, next, "the sidebar width");
      }
    },

    setSidebarHidden: (hidden) => {
      if (selectSidebarHidden(get()) !== hidden) {
        remember(SIDEBAR_HIDDEN_KEY, hidden, "whether the sidebar is hidden");
      }
    },

    toggleSidebar: () => {
      get().setSidebarHidden(!selectSidebarHidden(get()));
    },

    setFavoritesCollapsed: (collapsed) => {
      if (selectFavoritesCollapsed(get()) !== collapsed) {
        remember(FAVORITES_COLLAPSED_KEY, collapsed, "whether the favourites are collapsed");
      }
    },

    recordRecentPage: (pageId, spaceId) => {
      const state = get();
      // A template, and a page inside one, is not a page to come back to (ADR-0056): it is not in the sidebar either.
      if (isInTemplate(state.tree, pageId)) {
        return;
      }
      const current = parseRecentPages(state.settings[RECENT_PAGES_KEY]);
      const now = Date.now();
      // A page the core said it does not know any more (purged) is dropped with this write.
      const isGone = (id: PageId): boolean => state.pageLookup[id]?.status === "missing";
      const next = withRecentPage(current, { pageId, spaceId, openedAt: now }, isGone);
      const first = current[0];
      const unchanged =
        first !== undefined &&
        first.pageId === pageId &&
        first.spaceId === spaceId &&
        now - first.openedAt < RECENT_REFRESH_MS &&
        next.length === current.length;
      if (!unchanged) {
        remember(RECENT_PAGES_KEY, next.map((entry) => ({ ...entry })), "the recent pages");
      }
    },

    clearRecentPages: () => get().setSetting(RECENT_PAGES_KEY, []),
  };

  async function load(): Promise<Record<string, JsonValue | null>> {
    const loaded: Record<string, JsonValue | null> = {};
    await Promise.all(
      LAYOUT_KEYS.map(async (key) => {
        try {
          loaded[key] = await client.settingsGet(key);
        } catch (error) {
          console.error("could not read a layout setting", error);
        }
      }),
    );
    return loaded;
  }

  return {
    initial,
    load,
    onEvent: (event) => {
      if (event.type !== "PagesMovedSpace") {
        return;
      }
      const current = parseRecentPages(get().settings[RECENT_PAGES_KEY]);
      const next = withPagesInSpace(current, new Set(event.data.pageIds), event.data.toSpaceId);
      if (next !== current) {
        remember(RECENT_PAGES_KEY, next.map((entry) => ({ ...entry })), "the recent pages");
      }
    },
  };
}
