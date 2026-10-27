// The peek of a database row (ADR-0054, `docs/architecture/database-views.md` section 8): the page of a row shown
// beside or over the page that holds the database, without leaving it. One peek at a time, per window, in memory:
// it is not in the tab's history, it closes when the tab, the page or the space in front changes (and on lock), and
// clicking another row swaps its page. `PeekHost` draws it; `openRow` (`editor/open-row.ts`) decides when a click
// opens it.
//
// Device-local layout: the width of the side peek is the setting `ui.peek.width` (pixels, 320 to 960, default 480),
// kept like the sidebar's (ADR-0029) and written once per resize, on release.
//
// What the peek never touches: `selectedPageId`, `activeTabId` and the tab histories (dictation and the tabs depend on
// them), and the single slots of the page in front (backlinks, comments, the active editor, presence): the page of a
// peek is drawn by `PeekHost` from the tree and holds none of them (section 8 of the design document).
//
// It closes when the tab, the page or the space in front changes, when the main screen is left (Settings, Diagnostics),
// when the app locks or fails, and when its page is trashed, purged or moved out of the space (read from the tree and from
// the page lookup). The focus handed back to the row that opened it is `PeekHost`'s.

import type { CoreClient, Event as CoreEvent, JsonValue, PageId, SpaceId } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";
import { findPage } from "./tree";

/** Setting that holds the width of the side peek in pixels. */
export const PEEK_WIDTH_KEY = "ui.peek.width";
export const PEEK_MIN_WIDTH = 320;
export const PEEK_MAX_WIDTH = 960;
export const PEEK_DEFAULT_WIDTH = 480;
/** What an arrow key on the resize handle changes the width by. */
export const PEEK_KEY_STEP = 16;

/** Reads a stored width; anything that is not an integer from 320 to 960 means the default. */
export function parsePeekWidth(value: JsonValue | null | undefined): number {
  return typeof value === "number" && Number.isInteger(value) && value >= PEEK_MIN_WIDTH && value <= PEEK_MAX_WIDTH
    ? value
    : PEEK_DEFAULT_WIDTH;
}

/** A width the person chose (a drag, a key): whole pixels between 320 and 960. */
export function clampPeekWidth(pixels: number): number {
  if (!Number.isFinite(pixels)) {
    return PEEK_DEFAULT_WIDTH;
  }
  return Math.min(PEEK_MAX_WIDTH, Math.max(PEEK_MIN_WIDTH, Math.round(pixels)));
}

/** How the peek shows: a resizable panel at the right of the main area, or a large dialog over the page. */
export type PeekMode = "side" | "center";

/** The page the peek shows. */
export interface PeekTarget {
  readonly pageId: PageId;
  readonly spaceId: SpaceId;
  readonly mode: PeekMode;
}

export interface PeekData {
  /** The page in the peek, `null` when none is open. */
  peek: PeekTarget | null;
}

export const EMPTY_PEEK: PeekData = { peek: null };

export interface PeekActions {
  /**
   * Opens the peek on a page, or swaps the page (and the mode) of the one that is open. A page is never edited twice,
   * and a peek lives in the space in front: the page in front, and a page of another space, are not peeked (nothing
   * happens; `openRow` opens them as pages).
   */
  openPeek: (target: PeekTarget) => void;
  /** Closes the peek; closing none is fine. */
  closePeek: () => void;
  /** Sets the width of the side peek (clamped to 320 to 960) and remembers it: one write per call. */
  setPeekWidth: (width: number) => void;
}

export type PeekState = PeekData & PeekActions;

export const selectPeekWidth = (state: AppState): number => parsePeekWidth(state.settings[PEEK_WIDTH_KEY]);

/** Whether the peeked page is gone: its lookup says so (it changed to that), or the tree lost it. */
function peekedPageGone(state: AppState, previous: AppState, pageId: PageId): boolean {
  const lookup = state.pageLookup[pageId];
  if (
    lookup !== undefined &&
    lookup !== previous.pageLookup[pageId] &&
    (lookup.status === "missing" || (lookup.status === "found" && lookup.page.trashedAt !== undefined))
  ) {
    return true;
  }
  // The same rule as the tabs' (ADR-0034 section 3): it was live in the tree before and is not after the tree was read again.
  // A page made a moment ago that the tree has not caught up with is not gone.
  return (
    state.tree !== previous.tree &&
    state.tree !== null &&
    previous.tree !== null &&
    findPage(previous.tree.roots, pageId) !== null &&
    findPage(state.tree.roots, pageId) === null
  );
}

export interface PeekSlice {
  /** The data and the actions, to be part of the store's initial state. */
  initial: PeekState;
  /** Reads the width setting for the boot sequence; a key that cannot be read is left out. */
  load: () => Promise<Record<string, JsonValue | null>>;
  /** Called by the store after every state change: closes the peek when the tab, the page or the space in front changed. */
  onStateChange: (state: AppState, previous: AppState) => void;
  onEvent: (event: CoreEvent) => void;
}

export function createPeekSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): PeekSlice {
  return {
    initial: {
      ...EMPTY_PEEK,
      openPeek: (target) => {
        const { peek, selectedPageId, currentSpaceId } = get();
        if (target.pageId === selectedPageId || target.spaceId !== currentSpaceId) {
          return;
        }
        if (peek?.pageId === target.pageId && peek.spaceId === target.spaceId && peek.mode === target.mode) {
          return;
        }
        set({ peek: target });
      },
      closePeek: () => {
        if (get().peek !== null) {
          set({ peek: null });
        }
      },
      setPeekWidth: (width) => {
        const next = clampPeekWidth(width);
        if (selectPeekWidth(get()) === next) {
          return;
        }
        set((state) => ({ settings: { ...state.settings, [PEEK_WIDTH_KEY]: next } }));
        client.settingsSet(PEEK_WIDTH_KEY, next).catch((error: unknown) => {
          console.error("could not remember the width of the peek", error);
        });
      },
    },
    load: async (): Promise<Record<string, JsonValue | null>> => {
      try {
        return { [PEEK_WIDTH_KEY]: await client.settingsGet(PEEK_WIDTH_KEY) };
      } catch (error) {
        console.error("could not read the width of the peek", error);
        return {};
      }
    },
    onStateChange: (state, previous) => {
      const open = state.peek;
      if (open === null) {
        return;
      }
      if (
        state.activeTabId !== previous.activeTabId ||
        state.selectedPageId !== previous.selectedPageId ||
        state.currentSpaceId !== previous.currentSpaceId ||
        state.route !== "home" ||
        state.phase !== "ready" ||
        peekedPageGone(state, previous, open.pageId)
      ) {
        set({ peek: null });
      }
    },
    onEvent: () => undefined,
  };
}
