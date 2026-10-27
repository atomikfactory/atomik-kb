// Helpers for the tests of the tab strip, its keys and the scroll of its entries (ADR-0034): the
// whole layout over a real store, tab sets written straight into the store, and spies in place of
// the tabs actions, so that these tests hold whatever the tabs slice does with the actions.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, PageId, SpaceId } from "@atomik-kb/core-client";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { onTestFinished, vi } from "vitest";
import type { Mock } from "vitest";

import { t } from "../i18n";
import { MainLayout } from "../screens/MainLayout";
import { createAppStore } from "../store";
import type { AppState, AppStore } from "../store";
import { StoreContext } from "../store-context";
import type { Tab, TabEntry, TabId, TabsActions } from "../store-tabs";
import { idsOf } from "./page";
import { NOW, findMainNavigation } from "./render";
import { PERSONAL, WORK } from "./sidebar";

/** Names of the actions of the tabs that a test can replace with spies. */
export type TabActionName = Exclude<keyof TabsActions, "saveScroll" | "savedScroll" | "followMovedPages">;

const ACTIONS: readonly TabActionName[] = [
  "newTab",
  "activateTab",
  "activateTabAt",
  "activateAdjacentTab",
  "closeTab",
  "closeOtherTabs",
  "closeTabsToRight",
  "reopenClosedTab",
  "moveTab",
  "setTabPinned",
  "goBack",
  "goForward",
];

export type TabSpies = Record<TabActionName, Mock>;

/**
 * Renders the whole layout (sidebar, strip, main area) over a store on an in-memory core and waits
 * for the sidebar. The store is not the app's own (no `<App>`), so a test can reach it.
 */
export async function renderLayout(
  options: MemoryTransportOptions & { initialRoute?: "home" | "settings" | "diagnostics" } = {},
) {
  const { initialRoute, ...memoryOptions } = options;
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [PERSONAL, WORK], ...memoryOptions });
  const store = createAppStore(new CoreClient(memory), { corePollMs: 5, initialRoute });
  const stop = store.getState().start();
  onTestFinished(stop);
  const user = userEvent.setup();
  const view = render(
    <StoreContext value={store}>
      <MainLayout />
    </StoreContext>,
  );
  await findMainNavigation();
  const ids = await idsOf(memory);
  return { memory, store, user, ids, ...view };
}

/** A tab with one entry (a page, or a Home for `pageId: null`); `n` makes the ids readable. */
export function makeTab(
  n: string,
  spaceId: SpaceId,
  pageId: PageId | null,
  { pinned = false, entries }: { pinned?: boolean; entries?: readonly TabEntry[] } = {},
): Tab {
  const own: TabEntry = { id: `entry-${n}`, spaceId, pageId };
  return { id: `tab-${n}`, pinned, entries: entries ?? [own], index: (entries?.length ?? 1) - 1 };
}

/** A tab with a history of the given pages (all of one space), standing at `index`. */
export function historyTab(n: string, spaceId: SpaceId, pageIds: readonly (PageId | null)[], index: number): Tab {
  const entries = pageIds.map((pageId, position): TabEntry => ({ id: `entry-${n}-${String(position)}`, spaceId, pageId }));
  return { id: `tab-${n}`, pinned: false, entries, index };
}

/** Writes a tab set into the store: `active` is the id of the tab in front. */
export function seedTabs(store: AppStore, tabs: readonly Tab[], active: TabId | null, closedTabs: readonly Tab[] = []): void {
  store.setState({ tabs, activeTabId: active, closedTabs } satisfies Partial<AppState>);
}

/** Replaces the actions of the tabs with spies that do nothing and returns them. */
export function spyOnTabActions(store: AppStore): TabSpies {
  const spies = {} as TabSpies;
  const patch: Partial<AppState> = {};
  for (const name of ACTIONS) {
    spies[name] = vi.fn();
    (patch as Record<string, unknown>)[name] = spies[name];
  }
  store.setState(patch);
  return spies;
}

/** The tab strip's tab list. */
export function tabList(): HTMLElement {
  return screen.getByRole("tablist", { name: t("tabs.label") });
}

/** The tabs of the strip, in order. */
export function tabs(): HTMLElement[] {
  return within(tabList()).getAllByRole("tab");
}

/** The names of the tabs, in order. */
export function tabNames(): string[] {
  return tabs().map((tab) => tab.getAttribute("aria-label") ?? "");
}

/** One tab, by its accessible name. */
export function tabNamed(name: string): HTMLElement {
  return within(tabList()).getByRole("tab", { name });
}

/** The element that holds a tab and its close button. */
export function tabBox(tab: HTMLElement): HTMLElement {
  const box = tab.closest<HTMLElement>(".tab");
  if (box === null) {
    throw new Error("the tab has no box");
  }
  return box;
}

/** The close button of a tab, if it has one. */
export function closeButtonOf(tab: HTMLElement): HTMLElement | null {
  return within(tabBox(tab)).queryByRole("button", { name: t("tabs.close") });
}
