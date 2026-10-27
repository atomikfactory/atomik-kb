// The device-local state of the database views (ADR-0054, `docs/architecture/database-views.md` section 7): which
// view of each database block is shown on this device.
//
// The views themselves are in the document (the block's `views` attribute) and sync with the page; **which one is
// active is not**: it is the setting `ui.database.active_views`, a JSON object `{ "<block id>": "<view id>" }` in
// the shared `settings` cache, like the layout preferences of `store-layout.ts` (ADR-0029): read once at boot so the
// first paint is the final one, updated in the cache first and written in the background, never synced
// (a backup of this device carries it with the other settings). Because it is local, a viewer or commenter (a read-only editor) can
// switch tabs too: nothing is written to the page.
//
// A block id that no longer names a view (the view was deleted on another device, the block is gone) is ignored by
// `resolveActiveView`, which shows the first view. The object keeps the 200 most recently changed entries, so it
// never grows without bound (a duplicated page makes new block ids; a deleted block leaves its entry until it is
// pushed out). A lock empties the cache with the rest of the settings.
//
// Owner: agent B.

import type { CoreClient, Event as CoreEvent, JsonValue } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

/** The setting that holds the active view of each block (device-local, never synced). */
export const ACTIVE_VIEWS_KEY = "ui.database.active_views";
/** Most entries the setting keeps. */
export const MAX_ACTIVE_VIEW_ENTRIES = 200;

/**
 * Reads the stored setting: an object of block id to view id. An entry whose value is not a non-empty text is
 * skipped, anything that is no object is no entry; at most the last 200 entries (the object's order) are kept.
 */
export function parseActiveViews(value: JsonValue | null | undefined): Readonly<Record<string, string>> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return {};
  }
  const entries = Object.entries(value).filter(
    (entry): entry is [string, string] => entry[0] !== "" && entry[0] !== "__proto__" && typeof entry[1] === "string" && entry[1] !== "",
  );
  return Object.fromEntries(entries.slice(-MAX_ACTIVE_VIEW_ENTRIES));
}

/**
 * The map after `blockId` shows `viewId`: that entry goes last (the most recent), and only the 200 most recent
 * entries stay. The map itself when the block already shows that view.
 */
export function withActiveView(
  active: Readonly<Record<string, string>>,
  blockId: string,
  viewId: string,
): Readonly<Record<string, string>> {
  if (active[blockId] === viewId) {
    return active;
  }
  const rest = Object.entries(active).filter(([id]) => id !== blockId);
  const next: [string, string][] = [...rest, [blockId, viewId]];
  return Object.fromEntries(next.slice(-MAX_ACTIVE_VIEW_ENTRIES));
}

export interface ViewsActions {
  /** Shows the view `viewId` of the block `blockId` on this device (the cache at once, the setting in the background). */
  setActiveView: (blockId: string, viewId: string) => void;
}

export type ViewsState = ViewsActions;

/** The view id this device shows for a block, `undefined` when it never chose (the first view shows). */
export const selectActiveViewId = (state: AppState, blockId: string): string | undefined =>
  parseActiveViews(state.settings[ACTIVE_VIEWS_KEY])[blockId];

export interface ViewsSlice {
  /** The actions, to be part of the store's initial state. */
  initial: ViewsState;
  /** Reads the setting for the boot sequence; a key that cannot be read is left out. */
  load: () => Promise<Record<string, JsonValue | null>>;
  /** Nothing of the views follows an event: the choice is this device's own. */
  onEvent: (event: CoreEvent) => void;
}

export function createViewsSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): ViewsSlice {
  return {
    initial: {
      setActiveView: (blockId, viewId) => {
        const current = parseActiveViews(get().settings[ACTIVE_VIEWS_KEY]);
        const next = withActiveView(current, blockId, viewId);
        if (next === current) {
          return;
        }
        set((state) => ({ settings: { ...state.settings, [ACTIVE_VIEWS_KEY]: { ...next } } }));
        client.settingsSet(ACTIVE_VIEWS_KEY, { ...next }).catch((error: unknown) => {
          console.error("could not remember the view that is shown", error);
        });
      },
    },
    load: async (): Promise<Record<string, JsonValue | null>> => {
      try {
        return { [ACTIVE_VIEWS_KEY]: await client.settingsGet(ACTIVE_VIEWS_KEY) };
      } catch (error) {
        console.error("could not read the views that are shown", error);
        return {};
      }
    },
    onEvent: () => undefined,
  };
}
