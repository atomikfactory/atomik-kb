// The rules of the tabs (ADR-0034) as pure functions over the tab set, so that they can be tested
// without a store. `store-tabs.ts` applies them to the app state, reads and writes `ui.tabs`, and
// re-exports what the rest of the UI needs.
//
// A model is the reactive tab data (`tabs`, `activeTabId`, `closedTabs`) plus the bookkeeping that
// is not worth re-rendering for: which tab was activated last (for the "one tab per page" and the
// space switcher rules), where the last tab opened in the background went (so the next one lines up
// after it), and where each closed tab sat in the strip. Every function returns a new model, or
// the model it was given when there was nothing to do.

import type { BlockId, JsonValue, PageId, SpaceId } from "@atomik-kb/core-client";

import type { OpenDisposition, StoredTab, StoredTabs, Tab, TabEntry, TabId, TabsData } from "./store-tabs";

/** Most tabs open at once. */
export const MAX_TABS = 50;
/** Most entries in one tab's history; the oldest is dropped. */
export const MAX_TAB_HISTORY = 50;
/** Closed tabs that "Reopen closed tab" can bring back. */
export const MAX_CLOSED_TABS = 10;

/** Makes the random strings that name tabs and entries for the session. */
export type NewId = () => string;

/** The tabs opened in the background from `anchor` so far, `last` being the latest. */
export interface BackgroundChain {
  anchor: TabId;
  last: TabId;
}

export interface TabsBookkeeping {
  /** Tab ids, the most recently activated first (the active tab leads). May name tabs that are gone. */
  activation: readonly TabId[];
  /** Where the next background tab goes, `null` when there is no line to continue. */
  chain: BackgroundChain | null;
  /** The strip position each closed tab had when it was closed. */
  closedAt: Readonly<Record<TabId, number>>;
}

export interface TabsModel extends TabsData, TabsBookkeeping {}

export const NO_BOOKKEEPING: TabsBookkeeping = { activation: [], chain: null, closedAt: {} };

export const EMPTY_MODEL: TabsModel = { tabs: [], activeTabId: null, closedTabs: [], ...NO_BOOKKEEPING };

/** Where `openTarget` goes: a page, or the Home of a space (`pageId: null`). */
export interface OpenTarget {
  spaceId: SpaceId;
  pageId: PageId | null;
  /** A block to scroll to; a background open keeps it on the new entry until the entry first shows. */
  blockId?: BlockId;
}

/** What opening a page did. */
export interface OpenResult {
  model: TabsModel;
  /**
   * True when the page is now in front (a `current` or `foreground` open, or the tab that already
   * had it came to the front); false for a background open and for one that found the page already
   * open in a tab (then `model` is the very model that was given).
   */
  showed: boolean;
}

export function modelOf(data: TabsData, bookkeeping: TabsBookkeeping): TabsModel {
  return { tabs: data.tabs, activeTabId: data.activeTabId, closedTabs: data.closedTabs, ...bookkeeping };
}

export function bookkeepingOf(model: TabsBookkeeping): TabsBookkeeping {
  return { activation: model.activation, chain: model.chain, closedAt: model.closedAt };
}

/** The current entry of a tab. */
export function currentEntry(tab: Tab): TabEntry {
  const entry = tab.entries[tab.index] ?? tab.entries[tab.entries.length - 1];
  if (entry === undefined) {
    throw new Error("a tab has no entries");
  }
  return entry;
}

export function activeTabOf(data: TabsData): Tab | null {
  return data.tabs.find((tab) => tab.id === data.activeTabId) ?? null;
}

/** The space and page the main area shows (the mirrors `currentSpaceId` and `selectedPageId`, ADR-0034 invariant 2). */
export function mirrorOf(data: TabsData): { currentSpaceId: SpaceId | null; selectedPageId: PageId | null } {
  const active = activeTabOf(data);
  if (active === null) {
    return { currentSpaceId: null, selectedPageId: null };
  }
  const entry = currentEntry(active);
  return { currentSpaceId: entry.spaceId, selectedPageId: entry.pageId };
}

function indexOfTab(tabs: readonly Tab[], tabId: TabId | null): number {
  return tabId === null ? -1 : tabs.findIndex((tab) => tab.id === tabId);
}

/** How many tabs are pinned: pinned tabs come first. */
function pinnedCount(tabs: readonly Tab[]): number {
  const firstUnpinned = tabs.findIndex((tab) => !tab.pinned);
  return firstUnpinned < 0 ? tabs.length : firstUnpinned;
}

function inserted<T>(items: readonly T[], at: number, item: T): T[] {
  return [...items.slice(0, at), item, ...items.slice(at)];
}

function removedAt<T>(items: readonly T[], at: number): T[] {
  return [...items.slice(0, at), ...items.slice(at + 1)];
}

function replacedTab(model: TabsModel, tab: Tab): TabsModel {
  return { ...model, tabs: model.tabs.map((existing) => (existing.id === tab.id ? tab : existing)) };
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}

function homeTab(spaceId: SpaceId, newId: NewId): Tab {
  return { id: newId(), pinned: false, entries: [{ id: newId(), spaceId, pageId: null }], index: 0 };
}

function tabFor(target: OpenTarget, newId: NewId, keepBlock: boolean): Tab {
  const entry: TabEntry = { id: newId(), spaceId: target.spaceId, pageId: target.pageId };
  if (keepBlock && target.blockId !== undefined) {
    entry.blockId = target.blockId;
  }
  return { id: newId(), pinned: false, entries: [entry], index: 0 };
}

/** The most recently activated of some tabs; a tab never activated ranks after one that was, then strip order. */
function mostRecentlyActivated(model: TabsBookkeeping, candidates: readonly Tab[]): Tab | null {
  for (const tabId of model.activation) {
    const found = candidates.find((tab) => tab.id === tabId);
    if (found !== undefined) {
      return found;
    }
  }
  return candidates[0] ?? null;
}

/** The most recently activated tab whose current entry is in a space (the space switcher), if any. */
export function tabInSpace(model: TabsModel, spaceId: SpaceId): Tab | null {
  return mostRecentlyActivated(
    model,
    model.tabs.filter((tab) => currentEntry(tab).spaceId === spaceId),
  );
}

function activated(model: TabsModel, tabId: TabId): TabsModel {
  return {
    ...model,
    activeTabId: tabId,
    activation: [tabId, ...model.activation.filter((id) => id !== tabId)],
    // Opening tabs in the background lines them up after the active tab; another tab in front starts a new line.
    chain: tabId === model.activeTabId ? model.chain : null,
  };
}

/** A tab brought to the front; the model itself when there is no such tab. */
export function activateTab(model: TabsModel, tabId: TabId): TabsModel {
  return indexOfTab(model.tabs, tabId) < 0 ? model : activated(model, tabId);
}

/** The tab at a strip position; `-1` is the last tab, anything else out of range changes nothing. */
export function activateTabAt(model: TabsModel, position: number): TabsModel {
  const tab = model.tabs[position === -1 ? model.tabs.length - 1 : position];
  return tab === undefined ? model : activated(model, tab.id);
}

/** The next or previous tab in strip order, wrapping; nothing with fewer than two tabs. */
export function activateAdjacentTab(model: TabsModel, delta: 1 | -1): TabsModel {
  const at = indexOfTab(model.tabs, model.activeTabId);
  if (at < 0 || model.tabs.length < 2) {
    return model;
  }
  const next = model.tabs[(at + delta + model.tabs.length) % model.tabs.length];
  return next === undefined ? model : activated(model, next.id);
}

/** A new Home tab of a space at the end of the strip, in front (nothing at {@link MAX_TABS}). */
export function appendHomeTab(model: TabsModel, spaceId: SpaceId, newId: NewId): TabsModel {
  if (model.tabs.length >= MAX_TABS) {
    return model;
  }
  const tab = homeTab(spaceId, newId);
  return activated({ ...model, tabs: [...model.tabs, tab] }, tab.id);
}

/** A tab with the history `entry` goes after: the forward entries are dropped, and the oldest entries beyond the limit. */
function pushEntry(tab: Tab, entry: TabEntry): Tab {
  const entries = [...tab.entries.slice(0, tab.index + 1), entry].slice(-MAX_TAB_HISTORY);
  return { ...tab, entries, index: entries.length - 1 };
}

/**
 * Puts a new tab right after the active one, behind the tabs already opened from it in the
 * background (successive ones line up in order), and never among the pinned tabs.
 */
function insertNextToActive(model: TabsModel, active: Tab, tab: Tab): TabsModel {
  let after = indexOfTab(model.tabs, active.id);
  if (model.chain !== null && model.chain.anchor === active.id) {
    after = Math.max(after, indexOfTab(model.tabs, model.chain.last));
  }
  const at = Math.max(after + 1, pinnedCount(model.tabs));
  return { ...model, tabs: inserted(model.tabs, at, tab), chain: { anchor: active.id, last: tab.id } };
}

/**
 * Opens a page, or the Home of a space, as ADR-0034 section 2 says. A page that is the current page
 * of a tab is never opened twice: that tab comes to front (the active one if it is one of them,
 * else the most recently activated), and a background open then does nothing. A pinned active tab
 * keeps its page, so a `current` open from it becomes a `foreground` one; at {@link MAX_TABS} a
 * new tab is not made and the active tab navigates instead.
 */
export function openTarget(
  model: TabsModel,
  target: OpenTarget,
  disposition: OpenDisposition,
  newId: NewId,
): OpenResult {
  const active = activeTabOf(model);
  if (active === null) {
    const first = tabFor(target, newId, false);
    return {
      model: { ...model, tabs: [first], activeTabId: first.id, activation: [first.id], chain: null },
      showed: true,
    };
  }
  if (target.pageId !== null) {
    const holders = model.tabs.filter((tab) => currentEntry(tab).pageId === target.pageId);
    if (holders.length > 0) {
      if (disposition === "background") {
        return { model, showed: false };
      }
      const holder = holders.find((tab) => tab.id === active.id) ?? mostRecentlyActivated(model, holders) ?? active;
      return { model: activated(model, holder.id), showed: true };
    }
  }
  const here = currentEntry(active);
  if (disposition === "current" && target.pageId === null && here.pageId === null && here.spaceId === target.spaceId) {
    return { model: activated(model, active.id), showed: true }; // already on that Home
  }
  let how = disposition === "current" && active.pinned ? "foreground" : disposition;
  if (how !== "current" && model.tabs.length >= MAX_TABS) {
    how = "current";
  }
  if (how === "current") {
    const entry: TabEntry = { id: newId(), spaceId: target.spaceId, pageId: target.pageId };
    return { model: replacedTab(model, pushEntry(active, entry)), showed: true };
  }
  const tab = tabFor(target, newId, how === "background");
  const placed = insertNextToActive(model, active, tab);
  return how === "foreground" ? { model: activated(placed, tab.id), showed: true } : { model: placed, showed: false };
}

/**
 * Takes the block a background open left on the front entry (it scrolls there the first time the
 * entry shows): the model without it, and the block.
 */
export function takePendingBlock(model: TabsModel): { model: TabsModel; blockId: BlockId | undefined } {
  const active = activeTabOf(model);
  const entry = active === null ? undefined : active.entries[active.index];
  if (active === null || entry === undefined || entry.blockId === undefined) {
    return { model, blockId: undefined };
  }
  const shown: TabEntry = { id: entry.id, spaceId: entry.spaceId, pageId: entry.pageId };
  const entries = active.entries.map((existing, at) => (at === active.index ? shown : existing));
  return { model: replacedTab(model, { ...active, entries }), blockId: entry.blockId };
}

/** Back (`-1`) or forward (`1`) in the active tab; the model itself at either end of its history. */
export function stepHistory(model: TabsModel, delta: 1 | -1): TabsModel {
  const active = activeTabOf(model);
  if (active === null) {
    return model;
  }
  const index = active.index + delta;
  if (index < 0 || index >= active.entries.length) {
    return model;
  }
  return replacedTab(model, { ...active, index });
}

/** Keeps the positions of closed tabs that are still on the stack. */
function closedPositions(closedTabs: readonly Tab[], closedAt: Readonly<Record<TabId, number>>): Record<TabId, number> {
  const kept: Record<TabId, number> = {};
  for (const tab of closedTabs) {
    const position = closedAt[tab.id];
    if (position !== undefined) {
      kept[tab.id] = position;
    }
  }
  return kept;
}

/** Takes the tab at a position out of the strip; the tab to its right, else its left, takes the front if it had it. */
function removeTab(model: TabsModel, position: number, remember: boolean): TabsModel {
  const tab = model.tabs[position];
  if (tab === undefined) {
    return model;
  }
  const tabs = removedAt(model.tabs, position);
  let activeTabId = model.activeTabId;
  let activation = model.activation.filter((id) => id !== tab.id);
  if (model.activeTabId === tab.id) {
    activeTabId = (tabs[position] ?? tabs[position - 1])?.id ?? null;
    if (activeTabId !== null) {
      const next = activeTabId;
      activation = [next, ...activation.filter((id) => id !== next)];
    }
  }
  const chain =
    model.chain !== null &&
    activeTabId === model.activeTabId &&
    model.chain.anchor !== tab.id &&
    model.chain.last !== tab.id
      ? model.chain
      : null;
  const closedTabs = remember ? [tab, ...model.closedTabs].slice(0, MAX_CLOSED_TABS) : model.closedTabs;
  const closedAt = remember ? { ...model.closedAt, [tab.id]: position } : model.closedAt;
  return { tabs, activeTabId, activation, chain, closedTabs, closedAt: closedPositions(closedTabs, closedAt) };
}

/** When the last tab is gone and there is a space, a Home tab of it takes its place (invariant 1). */
function withHomeWhenEmpty(model: TabsModel, homeSpaceId: SpaceId | null, newId: NewId): TabsModel {
  if (model.tabs.length > 0 || homeSpaceId === null) {
    return model;
  }
  const tab = homeTab(homeSpaceId, newId);
  return { ...model, tabs: [tab], activeTabId: tab.id, activation: [tab.id], chain: null };
}

/**
 * Closes a tab (a pinned one too) and keeps it for "Reopen closed tab". Closing the last tab
 * leaves a Home tab of `homeSpaceId` (none when it is `null`).
 */
export function closeTab(model: TabsModel, tabId: TabId, homeSpaceId: SpaceId | null, newId: NewId): TabsModel {
  const position = indexOfTab(model.tabs, tabId);
  return position < 0 ? model : withHomeWhenEmpty(removeTab(model, position, true), homeSpaceId, newId);
}

/**
 * Closes tabs from the left, each at the position it had at that moment, so that reopening them
 * newest first puts every one back where it was. When the active tab was among them, `keepId` takes the front.
 */
function closeMany(model: TabsModel, keepId: TabId, tabIds: readonly TabId[]): TabsModel {
  if (tabIds.length === 0) {
    return model;
  }
  const activeClosed = model.activeTabId !== null && tabIds.includes(model.activeTabId);
  let next = model;
  for (const tabId of tabIds) {
    next = removeTab(next, indexOfTab(next.tabs, tabId), true);
  }
  return activeClosed ? activated(next, keepId) : next;
}

/** Closes every unpinned tab but one. */
export function closeOtherTabs(model: TabsModel, keepId: TabId): TabsModel {
  if (indexOfTab(model.tabs, keepId) < 0) {
    return model;
  }
  const ids = model.tabs.filter((tab) => tab.id !== keepId && !tab.pinned).map((tab) => tab.id);
  return closeMany(model, keepId, ids);
}

/** Closes the unpinned tabs to the right of one. */
export function closeTabsToRight(model: TabsModel, tabId: TabId): TabsModel {
  const at = indexOfTab(model.tabs, tabId);
  if (at < 0) {
    return model;
  }
  const ids = model.tabs
    .slice(at + 1)
    .filter((tab) => !tab.pinned)
    .map((tab) => tab.id);
  return closeMany(model, tabId, ids);
}

/** Where a tab goes in a strip that has room for it: inside its own group (pinned, or not). */
function placeInGroup(tabs: readonly Tab[], pinned: boolean, position: number): number {
  const count = pinnedCount(tabs);
  return clamp(Math.trunc(position), pinned ? 0 : count, pinned ? count : tabs.length);
}

/**
 * Brings back the most recently closed tab, with its history and its current entry, at its former
 * strip position (clamped inside its group) and in front. `isGone` names the entries that went
 * meanwhile; a tab left with none is skipped. When the page the tab shows is open in a tab now,
 * that tab comes to the front and the closed one is dropped. Nothing happens at {@link MAX_TABS}.
 */
export function reopenClosedTab(model: TabsModel, isGone: (entry: TabEntry) => boolean): TabsModel {
  let rest = model.closedTabs;
  while (rest.length > 0) {
    if (model.tabs.length >= MAX_TABS) {
      return model;
    }
    const [closed, ...others] = rest;
    if (closed === undefined) {
      break;
    }
    rest = others;
    const tab = pruneTab(closed, isGone);
    if (tab === null) {
      continue;
    }
    const base: TabsModel = { ...model, closedTabs: rest, closedAt: closedPositions(rest, model.closedAt) };
    const pageId = currentEntry(tab).pageId;
    const holder = pageId === null ? undefined : model.tabs.find((other) => currentEntry(other).pageId === pageId);
    if (holder !== undefined) {
      return activated(base, holder.id);
    }
    const position = placeInGroup(model.tabs, tab.pinned, model.closedAt[closed.id] ?? model.tabs.length);
    return activated({ ...base, tabs: inserted(model.tabs, position, tab) }, tab.id);
  }
  return rest === model.closedTabs ? model : { ...model, closedTabs: rest, closedAt: closedPositions(rest, model.closedAt) };
}

/** Moves a tab so that it ends up at a strip position, held inside its group (pinned tabs first). */
export function moveTab(model: TabsModel, tabId: TabId, toIndex: number): TabsModel {
  const from = indexOfTab(model.tabs, tabId);
  const tab = model.tabs[from];
  if (tab === undefined || !Number.isFinite(toIndex)) {
    return model;
  }
  const others = removedAt(model.tabs, from);
  const to = placeInGroup(others, tab.pinned, toIndex);
  return to === from ? model : { ...model, tabs: inserted(others, to, tab) };
}

/** Pins a tab (to the end of the pinned group) or unpins it (to the start of the others). */
export function setTabPinned(model: TabsModel, tabId: TabId, pinned: boolean): TabsModel {
  const from = indexOfTab(model.tabs, tabId);
  const tab = model.tabs[from];
  if (tab === undefined || tab.pinned === pinned) {
    return model;
  }
  const others = removedAt(model.tabs, from);
  return { ...model, tabs: inserted(others, pinnedCount(others), { ...tab, pinned }) };
}

/**
 * The tab without the entries `isGone` names: the forward and back entries around them stay, entries
 * that become neighbours and show the same place merge into one, and the position moves to the
 * entry before the current one when that went (else the one after). `null` when no entry is left;
 * the tab itself when nothing went.
 */
export function pruneTab(tab: Tab, isGone: (entry: TabEntry) => boolean): Tab | null {
  if (!tab.entries.some(isGone)) {
    return tab;
  }
  const kept = tab.entries.flatMap((entry, at) => (isGone(entry) ? [] : [{ entry, at }]));
  if (kept.length === 0) {
    return null;
  }
  const landing = Math.max(
    0,
    kept.findLastIndex(({ at }) => at <= tab.index),
  );
  const entries: TabEntry[] = [];
  let index = 0;
  kept.forEach(({ entry }, position) => {
    const last = entries[entries.length - 1];
    if (last !== undefined && last.spaceId === entry.spaceId && last.pageId === entry.pageId) {
      if (position === landing) {
        entries[entries.length - 1] = entry;
        index = entries.length - 1;
      }
      return;
    }
    entries.push(entry);
    if (position === landing) {
      index = entries.length - 1;
    }
  });
  return { ...tab, entries, index };
}

/**
 * Removes the entries `isGone` names from every tab's history (a gone page, a deleted space). A tab
 * left empty closes without being kept (there is nothing to reopen), and the neighbour to its right,
 * else to its left, takes the front when it was the active one. With no tab left, a Home tab of
 * `homeSpaceId` appears; it also appears when there was no tab at all (invariant 1).
 */
export function removeEntries(
  model: TabsModel,
  isGone: (entry: TabEntry) => boolean,
  homeSpaceId: SpaceId | null,
  newId: NewId,
): TabsModel {
  let changed = false;
  const survivors: { tab: Tab; at: number }[] = [];
  model.tabs.forEach((tab, at) => {
    const pruned = pruneTab(tab, isGone);
    changed ||= pruned !== tab;
    if (pruned !== null) {
      survivors.push({ tab: pruned, at });
    }
  });
  if (!changed && (model.tabs.length > 0 || homeSpaceId === null)) {
    return model;
  }
  const tabs = survivors.map(({ tab }) => tab);
  let activeTabId = model.activeTabId;
  if (!tabs.some((tab) => tab.id === activeTabId)) {
    const activeAt = Math.max(0, indexOfTab(model.tabs, model.activeTabId));
    activeTabId = (survivors.find(({ at }) => at > activeAt) ?? survivors[survivors.length - 1])?.tab.id ?? null;
  }
  const ids = new Set(tabs.map((tab) => tab.id));
  let activation = model.activation.filter((id) => ids.has(id));
  if (activeTabId !== null && activation[0] !== activeTabId && activeTabId !== model.activeTabId) {
    const next = activeTabId;
    activation = [next, ...activation.filter((id) => id !== next)];
  }
  const chain =
    model.chain !== null &&
    activeTabId === model.activeTabId &&
    ids.has(model.chain.anchor) &&
    ids.has(model.chain.last)
      ? model.chain
      : null;
  return withHomeWhenEmpty({ ...model, tabs, activeTabId, activation, chain }, homeSpaceId, newId);
}

/** A tab whose history has no two neighbours showing the same place; the current entry survives a merge. */
function withMergedNeighbours(tab: Tab): Tab {
  const entries: TabEntry[] = [];
  let index = 0;
  tab.entries.forEach((entry, at) => {
    const last = entries[entries.length - 1];
    if (last !== undefined && last.spaceId === entry.spaceId && last.pageId === entry.pageId) {
      if (at === tab.index) {
        entries[entries.length - 1] = entry;
      }
    } else {
      entries.push(entry);
    }
    if (at === tab.index) {
      index = entries.length - 1;
    }
  });
  return entries.length === tab.entries.length ? tab : { ...tab, entries, index };
}

/** A tab with every entry that shows one of `pageIds` in another space than `toSpaceId` moved there; the tab itself when none does. */
function withPagesIn(tab: Tab, pageIds: ReadonlySet<PageId>, toSpaceId: SpaceId): Tab {
  const shown = (entry: TabEntry): boolean =>
    entry.pageId !== null && pageIds.has(entry.pageId) && entry.spaceId !== toSpaceId;
  if (!tab.entries.some(shown)) {
    return tab;
  }
  // The entry keeps its id: its remembered scroll position stays.
  return withMergedNeighbours({
    ...tab,
    entries: tab.entries.map((entry) => (shown(entry) ? { ...entry, spaceId: toSpaceId } : entry)),
  });
}

/**
 * The tabs after pages moved to another space (ADR-0038): every entry of a tab, or of a closed tab, that shows
 * one of `pageIds` now names `toSpaceId` (the page is the same page, so the tab keeps showing it, and when it
 * is the front one the main area follows it into the other space), and neighbours in a history that then show
 * the same place merge into one. The model itself when no entry shows one of the pages.
 */
export function movePagesToSpace(model: TabsModel, pageIds: ReadonlySet<PageId>, toSpaceId: SpaceId): TabsModel {
  if (pageIds.size === 0) {
    return model;
  }
  let changed = false;
  const follow = (tab: Tab): Tab => {
    const next = withPagesIn(tab, pageIds, toSpaceId);
    changed ||= next !== tab;
    return next;
  };
  const tabs = model.tabs.map(follow);
  const closedTabs = model.closedTabs.map(follow);
  return changed ? { ...model, tabs, closedTabs } : model;
}

/** A tab set read back from `ui.tabs` (ADR-0034 section 5): one entry per tab, the stored active one in front. */
export function restoreTabs(stored: StoredTabs | null, spaceIds: readonly SpaceId[], newId: NewId): TabsModel {
  const known = new Set(spaceIds);
  const kept = (stored?.tabs ?? []).flatMap((tab, at) => (known.has(tab.spaceId) ? [{ tab, at }] : []));
  if (stored === null || kept.length === 0) {
    const first = spaceIds[0];
    if (first === undefined) {
      return EMPTY_MODEL;
    }
    const tab = homeTab(first, newId);
    return { ...EMPTY_MODEL, tabs: [tab], activeTabId: tab.id, activation: [tab.id] };
  }
  const tabs: Tab[] = kept.map(({ tab }) => ({
    id: newId(),
    pinned: tab.pinned,
    entries: [{ id: newId(), spaceId: tab.spaceId, pageId: tab.pageId }],
    index: 0,
  }));
  // The stored active tab, else (its space is gone) the first one left.
  const active = tabs[Math.max(0, kept.findIndex(({ at }) => at === stored.active))] ?? tabs[0];
  return { ...EMPTY_MODEL, tabs, activeTabId: active?.id ?? null, activation: active === undefined ? [] : [active.id] };
}

function parseStoredTab(item: JsonValue): StoredTab | null {
  if (typeof item !== "object" || item === null || Array.isArray(item)) {
    return null;
  }
  const { spaceId, pageId, pinned } = item;
  if (typeof spaceId !== "string" || spaceId === "" || typeof pinned !== "boolean") {
    return null;
  }
  if (pageId === null) {
    return { spaceId, pageId: null, pinned };
  }
  return typeof pageId === "string" && pageId !== "" ? { spaceId, pageId, pinned } : null;
}

/**
 * Reads the stored tab set (ADR-0034 section 5): anything unexpected means no stored tabs, a
 * malformed entry is skipped, a page listed twice keeps its first place, pinned tabs come first,
 * at most {@link MAX_TABS} are kept, an out-of-range `active` means the first tab.
 */
export function parseStoredTabs(value: JsonValue | null | undefined): StoredTabs | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  const { tabs, active } = value;
  if (!Array.isArray(tabs)) {
    return null;
  }
  const firstOfPage = new Map<PageId, number>();
  const parsed: { tab: StoredTab; at: number }[] = [];
  // The position (in the stored list) a skipped duplicate answers to: the first tab with its page.
  const duplicateOf = new Map<number, number>();
  tabs.forEach((item, at) => {
    const tab = parseStoredTab(item);
    if (tab === null) {
      return;
    }
    if (tab.pageId !== null) {
      const first = firstOfPage.get(tab.pageId);
      if (first !== undefined) {
        duplicateOf.set(at, first);
        return;
      }
      firstOfPage.set(tab.pageId, at);
    }
    parsed.push({ tab, at });
  });
  const ordered = [...parsed.filter(({ tab }) => tab.pinned), ...parsed.filter(({ tab }) => !tab.pinned)].slice(
    0,
    MAX_TABS,
  );
  if (ordered.length === 0) {
    return null;
  }
  const wanted = typeof active === "number" && Number.isInteger(active) && active >= 0 && active < tabs.length ? active : 0;
  const found = ordered.findIndex(({ at }) => at === (duplicateOf.get(wanted) ?? wanted));
  return { tabs: ordered.map(({ tab }) => tab), active: Math.max(0, found) };
}

/** The stored part of the tab set (the current entry of each tab and the active position); `null` with no tab. */
export function toStoredTabs(state: TabsData): StoredTabs | null {
  if (state.tabs.length === 0) {
    return null;
  }
  return {
    tabs: state.tabs.map((tab) => {
      const entry = currentEntry(tab);
      return { spaceId: entry.spaceId, pageId: entry.pageId, pinned: tab.pinned };
    }),
    active: Math.max(
      0,
      state.tabs.findIndex((tab) => tab.id === state.activeTabId),
    ),
  };
}

/** The stored tab set as the JSON value of the setting. */
export function storedTabsToJson(stored: StoredTabs): JsonValue {
  return {
    tabs: stored.tabs.map((tab) => ({ spaceId: tab.spaceId, pageId: tab.pageId, pinned: tab.pinned })),
    active: stored.active,
  };
}
