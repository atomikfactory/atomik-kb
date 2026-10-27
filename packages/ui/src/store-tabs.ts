// The tabs of the main area (ADR-0034): several pages open, each tab a history of entries
// (a page, or the Home of a space), one of them in front. `store.ts` composes it with the rest.
//
// The tabs slice owns `currentSpaceId` and `selectedPageId` (they mirror the active tab's current
// entry, ADR-0034 invariant 2) and every way of opening a page ends in it (`navigateTo`,
// `selectPage`, `selectSpace`, `createPage`). The tab set is remembered in the device setting
// `ui.tabs`; histories and scroll positions live for the session only.
//
// The rules themselves are pure functions in `tabs-model.ts`. This file applies them to the store:
// every change of the tabs, of the mirrors, of the route and of the scroll target goes out in ONE
// `set`, and a store subscription (`onStateChange`) keeps the tabs honest as the rest of the state
// moves: pages and spaces that are gone leave the histories (ADR-0034 section 3), and the stored
// tab set is written when its stored part changed.

import type {
  BlockId,
  CoreClient,
  Event as CoreEvent,
  JsonValue,
  PageId,
  PageInfo,
  SpaceId,
  SpaceInfo,
  TreeView,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";
import type { ScrollTarget } from "./store-content";
import {
  MAX_CLOSED_TABS,
  MAX_TAB_HISTORY,
  MAX_TABS,
  NO_BOOKKEEPING,
  activateAdjacentTab,
  activateTab,
  activateTabAt,
  activeTabOf,
  appendHomeTab,
  bookkeepingOf,
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  currentEntry,
  mirrorOf,
  modelOf,
  movePagesToSpace,
  moveTab,
  openTarget,
  parseStoredTabs,
  removeEntries,
  reopenClosedTab,
  restoreTabs,
  setTabPinned,
  stepHistory,
  storedTabsToJson,
  tabInSpace,
  takePendingBlock,
  toStoredTabs,
} from "./tabs-model";
import type { OpenTarget, TabsBookkeeping, TabsModel } from "./tabs-model";
import { indexPages } from "./tree";

export { MAX_CLOSED_TABS, MAX_TAB_HISTORY, MAX_TABS, currentEntry, parseStoredTabs, toStoredTabs };

/** Setting that holds the tab set (ADR-0034 section 5). */
export const TABS_KEY = "ui.tabs";

export type TabId = string;

/** How a page is opened (ADR-0034 section 2). */
export type OpenDisposition = "current" | "background" | "foreground";

/** One place in a tab's history: a page, or the Home of a space (`pageId: null`). */
export interface TabEntry {
  /** Unique for the session; keys the remembered scroll position. */
  id: string;
  spaceId: SpaceId;
  pageId: PageId | null;
  /** A block to scroll to the first time the entry shows (a background open of a search hit, a block link). */
  blockId?: BlockId;
}

export interface Tab {
  id: TabId;
  pinned: boolean;
  /** Never empty. */
  entries: readonly TabEntry[];
  /** The current entry. */
  index: number;
}

/** One tab of the `ui.tabs` setting: the current entry of a tab. */
export interface StoredTab {
  spaceId: SpaceId;
  pageId: PageId | null;
  pinned: boolean;
}

export interface StoredTabs {
  tabs: readonly StoredTab[];
  /** Position of the active tab in `tabs`. */
  active: number;
}

export interface TabsData {
  /** Strip order, pinned tabs first. */
  tabs: readonly Tab[];
  /** `null` only while there are no tabs (no space, or locked). */
  activeTabId: TabId | null;
  /** Most recently closed first, at most {@link MAX_CLOSED_TABS}; for the session only. */
  closedTabs: readonly Tab[];
}

/** Where `openEntry` goes: a page, or the Home of a space (`pageId: null`). */
export type TabTarget = OpenTarget;

/** Actions on the tabs. None of them talks to the core but `ui.tabs` writes; none rejects. */
export interface TabsActions {
  /** Opens the Home of the current space in a new tab at the end and activates it (nothing at {@link MAX_TABS}). */
  newTab: () => void;
  /** Brings a tab to the front (switching space when its entry is in another one) and shows the main screen. */
  activateTab: (tabId: TabId) => void;
  /** Activates the tab at a strip position; `-1` is the last tab. Out of range does nothing. */
  activateTabAt: (position: number) => void;
  /** Activates the next (`1`) or previous (`-1`) tab in strip order, wrapping. */
  activateAdjacentTab: (delta: 1 | -1) => void;
  /** Closes a tab (pinned too); the tab to its right, else its left, takes the front if it was active. */
  closeTab: (tabId: TabId) => void;
  /** Closes every unpinned tab but this one. */
  closeOtherTabs: (tabId: TabId) => void;
  /** Closes the unpinned tabs to the right of this one. */
  closeTabsToRight: (tabId: TabId) => void;
  /** Reopens the last closed tab with its history at its former position, and activates it. */
  reopenClosedTab: () => void;
  /** Moves a tab to a strip position, clamped inside its group (pinned or not). */
  moveTab: (tabId: TabId, toIndex: number) => void;
  /** Pins (to the end of the pinned group) or unpins (to the start of the others) a tab. */
  setTabPinned: (tabId: TabId, pinned: boolean) => void;
  /** Back in the active tab's history; does nothing at its start. */
  goBack: () => void;
  /** Forward in the active tab's history; does nothing at its end. */
  goForward: () => void;
  /** Remembers the scroll position of an entry (not reactive: re-renders nothing). */
  saveScroll: (entryId: string, scrollTop: number) => void;
  /** The remembered scroll position of an entry, if any. */
  savedScroll: (entryId: string) => number | undefined;
  /**
   * What `navigateTo`, `selectPage` and `createPage` end in: opens a page, or the Home of a space
   * (`pageId: null`), as `disposition` says (default `"current"`, ADR-0034 section 2). A block is
   * scrolled to, now or when the tab it went to first shows.
   */
  openEntry: (target: TabTarget, disposition?: OpenDisposition) => void;
  /**
   * The space switcher (`selectSpace` ends here): the most recently activated tab that shows a page
   * or the Home of that space comes to the front; with none, the active tab (a new tab when it is
   * pinned) goes to the Home of the space. The current space again only shows the main screen.
   */
  switchSpace: (spaceId: SpaceId) => void;
  /**
   * Pages moved from one space to another (ADR-0038): the entries that show them, in every tab and in the closed
   * tabs, name the new space, so the tabs keep them open and the front one takes the main area into the new
   * space. Runs for the move this window made and for `PagesMovedSpace` (from another device or window too);
   * the second one finds nothing left to do.
   */
  followMovedPages: (pageIds: readonly PageId[], fromSpaceId: SpaceId, toSpaceId: SpaceId) => void;
}

export type TabsState = TabsData & TabsActions;

export const EMPTY_TABS: TabsData = { tabs: [], activeTabId: null, closedTabs: [] };

export const selectActiveTab = (state: TabsData): Tab | null =>
  state.tabs.find((tab) => tab.id === state.activeTabId) ?? null;

export const selectCanGoBack = (state: TabsData): boolean => {
  const tab = selectActiveTab(state);
  return tab !== null && tab.index > 0;
};

export const selectCanGoForward = (state: TabsData): boolean => {
  const tab = selectActiveTab(state);
  return tab !== null && tab.index < tab.entries.length - 1;
};

/** The tab data a boot puts in the store, with the mirrors that go with it. */
export interface RestoredTabs extends TabsData {
  currentSpaceId: SpaceId | null;
  selectedPageId: PageId | null;
}

export interface TabsSlice {
  /** The data and the actions, to be part of the store's initial state. */
  initial: TabsState;
  /** Reads `ui.tabs` for the boot sequence (`null` when unset or unreadable). */
  load: () => Promise<StoredTabs | null>;
  /**
   * The space whose tree the boot loads first: the one of the stored active tab, else the first
   * space (ADR-0034 section 5). `undefined` without spaces.
   */
  activeSpaceFor: (stored: StoredTabs | null, spaces: readonly SpaceInfo[]) => SpaceId | undefined;
  /**
   * The tabs a boot starts with, from the stored ones: one entry each, tabs of spaces that no longer
   * exist dropped, one Home tab of the first space when nothing is left. Forgets the session (closed
   * tabs, scroll positions, what was seen of the trees) as a launch or an unlock does.
   */
  restore: (stored: StoredTabs | null, spaces: readonly SpaceInfo[]) => RestoredTabs;
  /**
   * The store subscription: reacts to a change of the tabs, the page tree, the page lookups and the
   * spaces (removes what is gone, looks up the pages the strip needs, writes `ui.tabs`).
   */
  onStateChange: (state: AppState, previous: AppState) => void;
  /**
   * `PagesMovedSpace`: the entries of the moved pages get their new space (ADR-0038). `SettingChanged` for `ui.tabs`
   * is not acted on: the tab set is device-local and this window owns it.
   */
  onEvent: (event: CoreEvent) => void;
}

/** What `commit` does besides storing the tabs. */
interface CommitOptions {
  /** A block the caller asks to scroll to; it wins over one waiting on the entry. */
  block?: BlockId;
  /** The change opens a page (or a Home): a scroll target that nobody asked for is cleared. */
  opened?: boolean;
  /** Records the page that shows in the recent pages even when it was in front already. */
  record?: boolean;
  /** Shows the main screen: always (default), never, or when another entry came to the front. */
  route?: "home" | "keep" | "ifFront";
}

/** Passes over a chain of reconciliations at most this many times per notification. */
const RECONCILE_PASSES = 8;

let idCounter = 0;

/** A random string for a tab or an entry: they only need to be unique for the session. */
function randomId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  idCounter += 1;
  return `${Date.now().toString(36)}-${idCounter.toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function frontEntryId(data: TabsData): string | null {
  const active = activeTabOf(data);
  return active === null ? null : currentEntry(active).id;
}

export function createTabsSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
  /** Reads the tree of the current space again (the structure slice's `loadCurrent`). */
  loadTree: () => Promise<unknown>,
): TabsSlice {
  // Scroll positions of the entries; not state, so scrolling re-renders nothing.
  const scrolls = new Map<string, number>();
  // What a model keeps besides the tab data: the activation order, the background line, where closed tabs sat.
  let book: TabsBookkeeping = NO_BOOKKEEPING;
  let scrollSeq = 0;
  // The JSON of the stored part of the tabs that was last read or written.
  let lastWritten: string | null = null;
  // `ui.tabs` writes go one after the other, so the last one written is the last one made.
  let writes: Promise<void> = Promise.resolve();

  // What the reconciliation has seen of the trees and the spaces (ADR-0034 section 3). A page that
  // was live in a tree of its space and is not in the next one has vanished (trashed or purged); a
  // page that was never seen is not gone, it may just not be in the tree yet (a page that was just made).
  const seenLive = new Map<SpaceId, ReadonlySet<PageId>>();
  const vanished = new Set<PageId>();
  const vanishedSpaces = new Set<SpaceId>();
  let knownSpaces: ReadonlySet<SpaceId> | null = null;
  let observedSpaces: readonly SpaceInfo[] | null = null;
  let indexedTree: { tree: TreeView; live: ReadonlyMap<PageId, PageInfo> } | null = null;

  /** Forgets everything of the session: after a lock, and before a boot restores the tabs. */
  function resetSession(): void {
    scrolls.clear();
    book = NO_BOOKKEEPING;
    seenLive.clear();
    vanished.clear();
    vanishedSpaces.clear();
    knownSpaces = null;
    observedSpaces = null;
    indexedTree = null;
  }

  const snapshot = (state: AppState = get()): TabsModel => modelOf(state, book);

  /** Drops the scroll positions of entries that cannot show again: not in a tab, nor in a closed tab that can be reopened. */
  function pruneScrolls(model: TabsModel): void {
    if (scrolls.size === 0) {
      return;
    }
    const live = new Set<string>();
    for (const tab of [...model.tabs, ...model.closedTabs]) {
      for (const entry of tab.entries) {
        live.add(entry.id);
      }
    }
    for (const entryId of [...scrolls.keys()]) {
      if (!live.has(entryId)) {
        scrolls.delete(entryId);
      }
    }
  }

  /**
   * Stores a new tab set, with the mirrors (`currentSpaceId`, `selectedPageId`), the route and the
   * scroll target that go with it, in one `set`; loads the tree when the space changed; records the
   * page that came to the front in the recent pages.
   */
  function commit(next: TabsModel, options: CommitOptions = {}): void {
    const before = get();
    const wasFront = frontEntryId(before);
    const { model, blockId: pending } = takePendingBlock(next);
    const front = activeTabOf(model);
    const entry = front === null ? null : currentEntry(front);
    const frontChanged = (entry?.id ?? null) !== wasFront;
    const mirror = mirrorOf(model);
    const spaceChanged = mirror.currentSpaceId !== before.currentSpaceId;
    const block = options.block ?? pending;

    const patch: Partial<AppState> = {
      tabs: model.tabs,
      activeTabId: model.activeTabId,
      closedTabs: model.closedTabs,
      currentSpaceId: mirror.currentSpaceId,
      selectedPageId: mirror.selectedPageId,
    };
    if (spaceChanged) {
      patch.tree = null;
    }
    const route = options.route ?? "home";
    if (route === "home" || (route === "ifFront" && frontChanged)) {
      patch.route = "home";
    }
    if (entry !== null && entry.pageId !== null && block !== undefined) {
      scrollSeq += 1;
      const target: ScrollTarget = { pageId: entry.pageId, blockId: block, seq: scrollSeq };
      patch.scrollTarget = target;
    } else if (options.opened === true || frontChanged) {
      patch.scrollTarget = null;
    }
    book = bookkeepingOf(model);
    set((state) => {
      // A request to edit a title is for the page it names: another page in front withdraws it.
      const withdraw = (options.opened === true || frontChanged) && state.titleFocusPageId !== mirror.selectedPageId;
      return withdraw ? { ...patch, titleFocusPageId: null } : patch;
    });
    if (spaceChanged && mirror.currentSpaceId !== null) {
      refreshQuietly(loadTree);
    }
    pruneScrolls(model);
    if (entry !== null && entry.pageId !== null && (options.record === true || frontChanged)) {
      get().recordRecentPage(entry.pageId, entry.spaceId);
    }
  }

  // ---- ui.tabs ----

  /** Writes the stored part of the tabs when it changed since it was last read or written. */
  function persist(): void {
    const stored = toStoredTabs(get());
    if (stored === null) {
      return; // No tab (locked, or no space): what is stored stays for the next unlock.
    }
    const value = storedTabsToJson(stored);
    const text = JSON.stringify(value);
    if (text === lastWritten) {
      return;
    }
    lastWritten = text;
    set((state) => ({ settings: { ...state.settings, [TABS_KEY]: value } }));
    writes = writes
      .then(() => client.settingsSet(TABS_KEY, value))
      .catch((error: unknown) => {
        console.error("could not remember the open tabs", error);
      });
  }

  // ---- gone pages and spaces (ADR-0034 section 3) ----

  function observeSpaces(spaces: readonly SpaceInfo[]): void {
    if (spaces === observedSpaces) {
      return;
    }
    observedSpaces = spaces;
    const ids = new Set(spaces.map((space) => space.id));
    if (knownSpaces !== null) {
      for (const id of knownSpaces) {
        if (!ids.has(id)) {
          vanishedSpaces.add(id);
        }
      }
    }
    for (const id of vanishedSpaces) {
      if (ids.has(id)) {
        vanishedSpaces.delete(id);
      }
    }
    knownSpaces = ids;
  }

  /** The live pages of a tree, noting the pages that were live in the last tree of its space and are not now. */
  function observeTree(tree: TreeView | null): ReadonlyMap<PageId, PageInfo> | null {
    if (tree === null) {
      return null;
    }
    if (indexedTree !== null && indexedTree.tree === tree) {
      return indexedTree.live;
    }
    const live = indexPages(tree.roots);
    for (const id of seenLive.get(tree.spaceId) ?? []) {
      if (!live.has(id)) {
        vanished.add(id);
      }
    }
    for (const id of vanished) {
      if (live.has(id)) {
        vanished.delete(id); // restored since
      }
    }
    seenLive.set(tree.spaceId, new Set(live.keys()));
    indexedTree = { tree, live };
    return live;
  }

  /** Asks for every page of a tab that is not in the tree of the current space; the strip needs their titles anyway. */
  function lookUpPages(live: ReadonlyMap<PageId, PageInfo> | null): void {
    const state = get();
    const wanted = new Set<PageId>();
    for (const tab of state.tabs) {
      for (const entry of tab.entries) {
        const pageId = entry.pageId;
        if (pageId === null || live?.has(pageId) === true || state.pageLookup[pageId] !== undefined) {
          continue;
        }
        if (live === null && entry.spaceId === state.currentSpaceId) {
          continue; // The tree of its space is on the way.
        }
        wanted.add(pageId);
      }
    }
    for (const pageId of wanted) {
      state.lookupPage(pageId);
    }
  }

  function reconcileOnce(): void {
    const state = get();
    if (state.phase === "locked") {
      resetSession();
      return;
    }
    observeSpaces(state.spaces);
    const live = observeTree(state.tree);
    const isGone = (entry: { spaceId: SpaceId; pageId: PageId | null }): boolean => {
      if (vanishedSpaces.has(entry.spaceId)) {
        return true;
      }
      const pageId = entry.pageId;
      if (pageId === null || live?.has(pageId) === true) {
        return false; // The tree is the truth for a page it has, whatever an older lookup said.
      }
      if (vanished.has(pageId)) {
        return true;
      }
      const lookup = state.pageLookup[pageId];
      return lookup !== undefined && (lookup.status === "missing" || (lookup.status === "found" && lookup.page.trashedAt !== undefined));
    };
    // A Home tab appears when there is none: only in a session that runs (a late answer after a lock must not make one).
    const running = state.phase === "ready" || state.phase === "onboarding";
    const home = running
      ? [state.currentSpaceId, ...state.spaces.map((space) => space.id)].find(
          (id): id is SpaceId => id !== null && !vanishedSpaces.has(id),
        ) ?? null
      : null;
    const model = snapshot(state);
    const next = removeEntries(model, isGone, home, randomId);
    if (next !== model) {
      commit(next, { route: "keep" });
    }
    lookUpPages(observeTree(get().tree));
  }

  let reconciling = false;
  let again = false;

  /** Runs `reconcileOnce` until the state it changed settles; a change made on the way asks for one more pass. */
  function reconcile(): void {
    if (reconciling) {
      again = true;
      return;
    }
    reconciling = true;
    try {
      for (let pass = 0; pass === 0 || (again && pass < RECONCILE_PASSES); pass += 1) {
        again = false;
        reconcileOnce();
      }
    } finally {
      reconciling = false;
      again = false;
    }
  }

  function onStateChange(state: AppState, previous: AppState): void {
    const tabsChanged = state.tabs !== previous.tabs;
    if (
      tabsChanged ||
      state.tree !== previous.tree ||
      state.pageLookup !== previous.pageLookup ||
      state.spaces !== previous.spaces ||
      state.phase !== previous.phase
    ) {
      reconcile();
    }
    // The stored part is the current entry of each tab and the active position: activating a
    // tab changes only `activeTabId`.
    if (tabsChanged || state.activeTabId !== previous.activeTabId) {
      persist();
    }
  }

  const initial: TabsState = {
    ...EMPTY_TABS,

    newTab: () => {
      const spaceId = get().currentSpaceId;
      if (spaceId === null) {
        return;
      }
      const model = snapshot();
      const next = appendHomeTab(model, spaceId, randomId);
      if (next !== model) {
        commit(next, { opened: true });
      }
    },

    activateTab: (tabId) => {
      const model = snapshot();
      const next = activateTab(model, tabId);
      if (next !== model) {
        commit(next);
      }
    },

    activateTabAt: (position) => {
      const model = snapshot();
      const next = activateTabAt(model, position);
      if (next !== model) {
        commit(next);
      }
    },

    activateAdjacentTab: (delta) => {
      const model = snapshot();
      const next = activateAdjacentTab(model, delta);
      if (next !== model) {
        commit(next);
      }
    },

    closeTab: (tabId) => {
      const state = get();
      const model = snapshot(state);
      const next = closeTab(model, tabId, state.currentSpaceId ?? state.spaces[0]?.id ?? null, randomId);
      if (next !== model) {
        commit(next, { route: "ifFront" });
      }
    },

    closeOtherTabs: (tabId) => {
      const model = snapshot();
      const next = closeOtherTabs(model, tabId);
      if (next !== model) {
        commit(next, { route: "ifFront" });
      }
    },

    closeTabsToRight: (tabId) => {
      const model = snapshot();
      const next = closeTabsToRight(model, tabId);
      if (next !== model) {
        commit(next, { route: "ifFront" });
      }
    },

    reopenClosedTab: () => {
      const state = get();
      const model = snapshot(state);
      // What went while the tab was closed does not come back: a deleted space, a page known to be gone.
      const next = reopenClosedTab(model, (entry) => {
        if (vanishedSpaces.has(entry.spaceId) || (entry.pageId !== null && vanished.has(entry.pageId))) {
          return true;
        }
        const lookup = entry.pageId === null ? undefined : state.pageLookup[entry.pageId];
        return lookup?.status === "missing" || (lookup?.status === "found" && lookup.page.trashedAt !== undefined);
      });
      if (next !== model) {
        commit(next);
      }
    },

    moveTab: (tabId, toIndex) => {
      const model = snapshot();
      const next = moveTab(model, tabId, toIndex);
      if (next !== model) {
        commit(next, { route: "keep" });
      }
    },

    setTabPinned: (tabId, pinned) => {
      const model = snapshot();
      const next = setTabPinned(model, tabId, pinned);
      if (next !== model) {
        commit(next, { route: "keep" });
      }
    },

    goBack: () => {
      const model = snapshot();
      const next = stepHistory(model, -1);
      if (next !== model) {
        commit(next);
      }
    },

    goForward: () => {
      const model = snapshot();
      const next = stepHistory(model, 1);
      if (next !== model) {
        commit(next);
      }
    },

    saveScroll: (entryId, scrollTop) => {
      scrolls.set(entryId, scrollTop);
    },

    savedScroll: (entryId) => scrolls.get(entryId),

    openEntry: (target, disposition = "current") => {
      const model = snapshot();
      const { model: next, showed } = openTarget(model, target, disposition, randomId);
      if (!showed) {
        // A background open: the tab is there, the page in front stays; or nothing, the page being open already.
        if (next !== model) {
          commit(next, { route: "keep" });
        }
        return;
      }
      commit(next, { block: target.blockId, opened: true, record: true });
    },

    switchSpace: (spaceId) => {
      const state = get();
      if (state.currentSpaceId === spaceId) {
        set({ route: "home" });
        return;
      }
      const model = snapshot(state);
      const there = tabInSpace(model, spaceId);
      if (there !== null) {
        commit(activateTab(model, there.id));
        return;
      }
      commit(openTarget(model, { spaceId, pageId: null }, "current", randomId).model, { opened: true });
    },

    followMovedPages: (pageIds, fromSpaceId, toSpaceId) => {
      const moved = new Set(pageIds);
      // The tree of the old space stops listing them, which is not a page going away.
      for (const pageId of moved) {
        vanished.delete(pageId);
      }
      const seen = seenLive.get(fromSpaceId);
      if (seen !== undefined) {
        seenLive.set(fromSpaceId, new Set([...seen].filter((pageId) => !moved.has(pageId))));
      }
      const model = snapshot();
      const next = movePagesToSpace(model, moved, toSpaceId);
      if (next !== model) {
        commit(next, { route: "keep" });
      }
    },
  };

  return {
    initial,

    load: async () => {
      try {
        return parseStoredTabs(await client.settingsGet(TABS_KEY));
      } catch (error) {
        console.error("could not read the open tabs", error);
        return null;
      }
    },

    activeSpaceFor: (stored, spaces) =>
      mirrorOf(
        restoreTabs(
          stored,
          spaces.map((space) => space.id),
          randomId,
        ),
      ).currentSpaceId ?? undefined,

    restore: (stored, spaces) => {
      resetSession();
      const model = restoreTabs(
        stored,
        spaces.map((space) => space.id),
        randomId,
      );
      book = bookkeepingOf(model);
      // Nothing is written for what the stored tabs already say (or for the plain start of a first launch).
      const known = stored ?? toStoredTabs(model);
      lastWritten = known === null ? null : JSON.stringify(storedTabsToJson(known));
      return { tabs: model.tabs, activeTabId: model.activeTabId, closedTabs: [], ...mirrorOf(model) };
    },

    onStateChange,

    onEvent: (event) => {
      if (event.type === "PagesMovedSpace") {
        initial.followMovedPages(event.data.pageIds, event.data.fromSpaceId, event.data.toSpaceId);
      }
    },
  };
}

/** The stored tab set as the value of the `ui.tabs` setting, for the settings cache. */
export function storedTabsValue(stored: StoredTabs | null): JsonValue | null {
  return stored === null ? null : storedTabsToJson(stored);
}
