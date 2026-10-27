// The recent pages (ADR-0029) as the UI shows them: the stored entries (`ui.recent_pages`)
// with the title and icon of each looked up live, trashed and unknown pages left out.

import type { PageId, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useMemo } from "react";

import { useAppStore } from "./store-context";
import { RECENT_PAGES_KEY, parseRecentPages } from "./store-layout";
import { templatePageIds } from "./templates";
import { indexPages } from "./tree";

/** A recent page, ready to show. */
export interface RecentPage {
  pageId: PageId;
  spaceId: SpaceId;
  /** When it was last opened, Unix ms. */
  openedAt: number;
  /** The page's title as it is now; empty for an untitled page. */
  title: string;
  /** The page's icon as it is now. */
  icon: string | undefined;
  /** The name of the page's space. */
  spaceName: string;
}

/**
 * The first `limit` recent pages, most recent first. Pages of the open space come from its tree,
 * the others from the page lookup (asked for here, so the list fills in as the answers arrive);
 * a page in the trash, one that is gone and one not answered yet are not in the list. The titles
 * and icons follow renames, because both sources are refreshed by the structure events. A template and the pages inside one
 * (ADR-0056) are not listed, as they are not in the sidebar.
 */
export function useRecentPages(limit: number): RecentPage[] {
  const stored = useAppStore((state) => state.settings[RECENT_PAGES_KEY]);
  const tree = useAppStore((state) => state.tree);
  const lookups = useAppStore((state) => state.pageLookup);
  const spaces = useAppStore((state) => state.spaces);
  const lookupPage = useAppStore((state) => state.lookupPage);

  const entries = useMemo(() => parseRecentPages(stored), [stored]);
  const live = useMemo(() => (tree === null ? null : indexPages(tree.roots)), [tree]);
  const templates = useMemo(() => templatePageIds(tree), [tree]);

  // A page that is not in the tree of the open space (another space, the trash, a purged page) is asked for.
  useEffect(() => {
    for (const entry of entries) {
      if (live?.get(entry.pageId) === undefined) {
        lookupPage(entry.pageId);
      }
    }
  }, [entries, live, lookupPage]);

  return useMemo(() => {
    const spaceNames = new Map(spaces.map((space) => [space.id, space.name]));
    const pages: RecentPage[] = [];
    for (const entry of entries) {
      if (pages.length >= limit) {
        break;
      }
      const inTree = live?.get(entry.pageId);
      const lookup = lookups[entry.pageId];
      const page = inTree ?? (lookup?.status === "found" ? lookup.page : undefined);
      if (page === undefined || page.trashedAt !== undefined || page.template === true || templates.has(page.id)) {
        continue;
      }
      pages.push({
        pageId: page.id,
        spaceId: page.spaceId,
        openedAt: entry.openedAt,
        title: page.title,
        icon: page.icon,
        spaceName: spaceNames.get(page.spaceId) ?? "",
      });
    }
    return pages;
  }, [entries, live, templates, lookups, spaces, limit]);
}
