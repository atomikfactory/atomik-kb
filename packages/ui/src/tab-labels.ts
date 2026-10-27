// What a tab of the strip shows (ADR-0034 section 4): the icon and the title of the page of its
// current entry, or "Home" for the Home of a space. The page of the open space comes from its
// tree, any other from the page lookup (asked for here, so the label fills in as the answer
// arrives); both are refreshed by the structure events, so a rename shows at once.

import type { PageInfo } from "@atomik-kb/core-client";
import { useEffect, useMemo } from "react";

import { t } from "./i18n";
import { useAppStore } from "./store-context";
import { currentEntry } from "./store-tabs";
import type { Tab, TabId } from "./store-tabs";
import { indexPages } from "./tree";

/** A tab, ready to show. */
export interface TabLabel {
  /** The title to show: "Home", "Untitled" for a page without a title, empty while the page is looked up. */
  title: string;
  /** What names the tab to a screen reader and in its tooltip (never empty). */
  name: string;
  /** The page's icon; `undefined` for none, and for a Home. */
  icon: string | undefined;
  /** True for the Home of a space (its own glyph shows, not a page's). */
  home: boolean;
}

/** The label of every tab, by tab id. */
export function useTabLabels(tabs: readonly Tab[]): ReadonlyMap<TabId, TabLabel> {
  const tree = useAppStore((state) => state.tree);
  const currentSpaceId = useAppStore((state) => state.currentSpaceId);
  const lookups = useAppStore((state) => state.pageLookup);
  const lookupPage = useAppStore((state) => state.lookupPage);

  const live = useMemo(() => (tree === null ? null : indexPages(tree.roots)), [tree]);

  // A page that the tree of the open space does not have (another space, the trash, a page the tree
  // does not hold yet) is asked for; asking again for one that was asked for does nothing.
  useEffect(() => {
    for (const tab of tabs) {
      const { pageId, spaceId } = currentEntry(tab);
      if (pageId === null) {
        continue;
      }
      const inOpenSpace = spaceId === currentSpaceId;
      if (inOpenSpace && live === null) {
        continue; // the tree is on its way
      }
      if (!inOpenSpace || live?.get(pageId) === undefined) {
        lookupPage(pageId);
      }
    }
  }, [tabs, currentSpaceId, live, lookupPage]);

  return useMemo(() => {
    const labels = new Map<TabId, TabLabel>();
    for (const tab of tabs) {
      const { pageId, spaceId } = currentEntry(tab);
      if (pageId === null) {
        const title = t("tabs.home");
        labels.set(tab.id, { title, name: title, icon: undefined, home: true });
        continue;
      }
      const lookup = lookups[pageId];
      let page: PageInfo | undefined;
      if (spaceId === currentSpaceId) {
        page = live?.get(pageId);
      }
      if (page === undefined && lookup?.status === "found") {
        page = lookup.page;
      }
      if (page === undefined) {
        // Not answered yet: an empty title and the default glyph, not a title that is not the page's.
        labels.set(tab.id, { title: "", name: t("page.untitled"), icon: undefined, home: false });
        continue;
      }
      const title = page.title === "" ? t("page.untitled") : page.title;
      labels.set(tab.id, { title, name: title, icon: page.icon, home: false });
    }
    return labels;
  }, [tabs, currentSpaceId, live, lookups]);
}
