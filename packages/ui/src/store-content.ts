// The content half of the app store (Milestone 3): open page documents, pages referenced
// from content, backlinks, search and navigation into a page. `store.ts` composes it
// with the rest.
//
// As in the structure slice, reads are "latest request wins" and events refresh what
// they name: `BacklinksChanged` the backlinks of the open page, `StructureChanged` the
// titles and icons of referenced pages (and the backlinks' titles). Page content itself
// is kept in sync by the open page's document session (`editor/page-session.ts`).

import { toCoreError } from "@atomik-kb/core-client";
import type {
  Backlink,
  BlockId,
  CoreClient,
  Event as CoreEvent,
  OptionId,
  PageId,
  PageInfo,
  PropertyId,
  SearchHit,
  SearchOptions,
  SpaceId,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { PageDocSessions } from "./editor/page-session";
import type { AppState } from "./store";
import type { OpenDisposition } from "./store-tabs";
import { withoutTemplates } from "./template-tree";
import { templatePageIds } from "./templates";
import { findPage, flattenVisible } from "./tree";

/** What the UI knows about a page referenced from content (a chip, a backlink). */
export type PageLookup = { status: "loading" } | { status: "found"; page: PageInfo } | { status: "missing" };

export interface BacklinksState {
  pageId: PageId;
  status: "loading" | "ready" | "error";
  /** One entry per referencing block, in the core's order. */
  items: readonly Backlink[];
}

/**
 * The tag filter of the search (ADR-0026): only the pages of one space that have this option
 * selected in a multi-select property (Tags). It limits the search to that space.
 */
export interface SearchTag {
  spaceId: SpaceId;
  propertyId: PropertyId;
  optionId: OptionId;
}

/** The Space chip of the search (ADR-0040): every space (the default) or the one that is open. */
export type SearchSpaceFilter = "all" | "current";

/** The Edited chip of the search (ADR-0040): any time, since local midnight, or the last 7 or 30 days. */
export type SearchEdited = "any" | "today" | "week" | "month";

/** The filter chips of the search other than the tag, which `setSearchFilters` changes. */
export interface SearchFilters {
  space: SearchSpaceFilter;
  edited: SearchEdited;
  /** The "Titles only" chip: the core's `Titles` scope (no page text, no properties). */
  titlesOnly: boolean;
}

export interface SearchState extends SearchFilters {
  open: boolean;
  /** The query as typed. */
  query: string;
  status: "idle" | "loading" | "ready" | "error";
  hits: readonly SearchHit[];
  /** The tag the results are limited to, `null` for none. With a tag, an empty query lists its pages. */
  tag: SearchTag | null;
}

/** A block to bring into view once its page shows (search results, backlinks). */
export interface ScrollTarget {
  pageId: PageId;
  blockId: BlockId;
  /** Distinguishes two requests for the same block. */
  seq: number;
}

/** Where `navigateTo` goes. */
export interface NavigationTarget {
  pageId: PageId;
  /** The page's space; switches space when it is not the current one. */
  spaceId?: SpaceId;
  /** A block to scroll to. */
  blockId?: BlockId;
  /** The active tab, a new tab behind it or a new tab in front (ADR-0034); default `"current"`. */
  disposition?: OpenDisposition;
}

/** A page offered by the `[[` picker. */
export interface PageChoice {
  pageId: PageId;
  spaceId: SpaceId;
  title: string;
  icon?: string;
}

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * The oldest last edit (Unix ms) the Edited chip lets through at `now`, `undefined` for any time: local
 * midnight for today, else exactly 7 or 30 days before now.
 */
export function editedAfterOf(edited: SearchEdited, now: Date): number | undefined {
  switch (edited) {
    case "any":
      return undefined;
    case "today":
      return new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
    case "week":
      return now.getTime() - 7 * DAY_MS;
    case "month":
      return now.getTime() - 30 * DAY_MS;
  }
}

/** Most results of one search; the core caps it at 100. */
export const SEARCH_LIMIT = 30;
/** Pages the `[[` picker offers. */
export const PICKER_LIMIT = 8;
/** The core accepts queries up to this many characters. */
const MAX_QUERY_CHARS = 200;

export interface ContentData {
  pageLookup: Readonly<Record<PageId, PageLookup>>;
  /** Backlinks of the open page; `null` with no page open. */
  backlinks: BacklinksState | null;
  search: SearchState;
  scrollTarget: ScrollTarget | null;
}

export interface ContentActions {
  /** The open page documents (one registry per app). */
  pageDocs: PageDocSessions;
  /** Makes sure `pageLookup` has the page (title, icon, trash state); a no-op once known. */
  lookupPage: (pageId: PageId) => void;
  /** Loads the backlinks of a page (the open one), or forgets them with `null`. */
  loadBacklinks: (pageId: PageId | null) => void;
  openSearch: () => void;
  /** Opens the search limited to the pages with a tag (a click on a tag of the properties panel). */
  openSearchWithTag: (tag: SearchTag) => void;
  /** Sets or clears the tag filter of the open search; the results wait for the next `runSearch`. */
  setSearchTag: (tag: SearchTag | null) => void;
  /**
   * Changes the Space, Edited and Titles-only chips of the open search (what is left out stays); the
   * results wait for the next `runSearch`, which asks the core with them.
   */
  setSearchFilters: (filters: Partial<SearchFilters>) => void;
  closeSearch: () => void;
  /**
   * Searches titles and bodies (with the filter chips: the open space only, pages edited since, titles
   * only); an empty query clears the results, unless a tag filter is set: then it lists the pages with the
   * tag, and a query searches among them.
   */
  runSearch: (query: string) => Promise<void>;
  /** Pages for the `[[` picker: titles matching `query`, or the first pages of the current space. */
  findPages: (query: string) => Promise<PageChoice[]>;
  /**
   * Opens a page (switching space when needed) and scrolls to a block of it, in the active tab,
   * a new tab behind it or a new tab in front (`disposition`, ADR-0034).
   */
  navigateTo: (target: NavigationTarget) => void;
  clearScrollTarget: () => void;
  /** The page's content as Markdown; rejects with a `CoreError`. */
  exportMarkdown: (pageId: PageId) => Promise<string>;
  /**
   * Opens an external http, https or mailto link through the shell, or in a new browser
   * tab without an opener where the shell cannot. Other schemes are ignored.
   */
  openExternal: (url: string) => Promise<void>;
}

export type ContentState = ContentData & ContentActions;

export const EMPTY_SEARCH: SearchState = {
  open: false,
  query: "",
  status: "idle",
  hits: [],
  tag: null,
  space: "all",
  edited: "any",
  titlesOnly: false,
};

/** No page looked up, no backlinks, no search: the content data after a lock. */
export const EMPTY_CONTENT: ContentData = { pageLookup: {}, backlinks: null, search: EMPTY_SEARCH, scrollTarget: null };

/** True for the link schemes the editor opens: http, https and mailto. */
export function isExternalHref(href: string): boolean {
  try {
    const { protocol } = new URL(href);
    return protocol === "http:" || protocol === "https:" || protocol === "mailto:";
  } catch {
    return false;
  }
}

export interface ContentSlice {
  initial: ContentState;
  onEvent: (event: CoreEvent) => void;
}

export function createContentSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): ContentSlice {
  const pageDocs = new PageDocSessions(client);
  const lookupTickets = new Map<PageId, number>();
  let backlinksTicket = 0;
  let searchTicket = 0;

  async function fetchLookup(pageId: PageId): Promise<void> {
    const ticket = (lookupTickets.get(pageId) ?? 0) + 1;
    lookupTickets.set(pageId, ticket);
    let lookup: PageLookup;
    try {
      lookup = { status: "found", page: await client.pageGet(pageId) };
    } catch (error) {
      if (toCoreError(error).code !== "NotFound") {
        throw error;
      }
      lookup = { status: "missing" };
    }
    if (lookupTickets.get(pageId) === ticket) {
      set((state) => ({ pageLookup: { ...state.pageLookup, [pageId]: lookup } }));
    }
  }

  async function fetchBacklinks(pageId: PageId): Promise<void> {
    const ticket = (backlinksTicket += 1);
    try {
      const items = await client.backlinksGet(pageId);
      if (ticket === backlinksTicket) {
        set({ backlinks: { pageId, status: "ready", items } });
      }
    } catch (error) {
      if (ticket === backlinksTicket) {
        set((state) => ({
          backlinks: { pageId, status: "error", items: state.backlinks?.pageId === pageId ? state.backlinks.items : [] },
        }));
      }
      throw error;
    }
  }

  /**
   * What the chips ask of the core besides the query and the limit: the open space only (`current`), the
   * `Titles` scope, and `editedAfter` from the Edited chip now. Nothing for a chip left at its default, so
   * the request is the plain one then.
   */
  function filterOptions(search: SearchFilters): Pick<SearchOptions, "spaceId" | "scope" | "editedAfter"> {
    const options: Pick<SearchOptions, "spaceId" | "scope" | "editedAfter"> = {};
    const spaceId = get().currentSpaceId;
    if (search.space === "current" && spaceId !== null) {
      options.spaceId = spaceId;
    }
    if (search.titlesOnly) {
      options.scope = "Titles";
    }
    const editedAfter = editedAfterOf(search.edited, new Date());
    if (editedAfter !== undefined) {
      options.editedAfter = editedAfter;
    }
    return options;
  }

  /**
   * The results of a search limited to a tag: the pages with the tag as title hits (in the core's
   * order) when nothing is typed, else the hits of the text search in the tag's space that are on
   * such a page. The Edited and Titles-only chips apply too; the Space chip does not, since the tag
   * belongs to one space.
   */
  async function taggedHits(tag: SearchTag, trimmed: string, search: SearchFilters): Promise<SearchHit[]> {
    const ids = await client.pagesByProperty(tag.spaceId, tag.propertyId, { type: "ContainsOption", data: tag.optionId });
    const { editedAfter } = filterOptions(search);
    if (trimmed !== "") {
      const tagged = new Set(ids);
      // The core's largest answer: the filter comes after the search, so ask for as many as it gives.
      const found = await client.search(trimmed, { ...filterOptions({ ...search, space: "all" }), spaceId: tag.spaceId, limit: 100 });
      return found.filter((hit) => tagged.has(hit.pageId)).slice(0, SEARCH_LIMIT);
    }
    const roots = get().tree?.spaceId === tag.spaceId ? get().tree?.roots : undefined;
    // The core lists the templates of a tag among its pages (`PagesByProperty`); a list of hits to open leaves them out (ADR-0056).
    const templates = get().tree?.spaceId === tag.spaceId ? templatePageIds(get().tree) : new Set<PageId>();
    const hits: SearchHit[] = [];
    for (const id of ids) {
      if (hits.length >= SEARCH_LIMIT) {
        break;
      }
      if (templates.has(id)) {
        continue;
      }
      const page = roots === undefined ? null : findPage(roots, id);
      const info = page ?? (await client.pageGet(id));
      if (editedAfter !== undefined && info.updatedAt < editedAfter) {
        continue;
      }
      const hit: SearchHit = { pageId: id, spaceId: tag.spaceId, title: info.title, snippet: info.title, highlights: [] };
      if (info.icon !== undefined) {
        hit.icon = info.icon;
      }
      hits.push(hit);
    }
    return hits;
  }

  const initial: ContentState = {
    ...EMPTY_CONTENT,
    pageDocs,

    lookupPage: (pageId) => {
      if (get().pageLookup[pageId] !== undefined) {
        return;
      }
      set((state) => ({ pageLookup: { ...state.pageLookup, [pageId]: { status: "loading" } } }));
      refreshQuietly(() => fetchLookup(pageId));
    },

    loadBacklinks: (pageId) => {
      if (pageId === null) {
        backlinksTicket += 1;
        set({ backlinks: null });
        return;
      }
      set((state) => ({
        backlinks:
          state.backlinks?.pageId === pageId ? state.backlinks : { pageId, status: "loading", items: [] },
      }));
      refreshQuietly(() => fetchBacklinks(pageId));
    },

    openSearch: () => {
      set((state) => ({ search: { ...state.search, open: true } }));
    },

    openSearchWithTag: (tag) => {
      searchTicket += 1;
      set({ search: { ...EMPTY_SEARCH, open: true, tag } });
    },

    setSearchTag: (tag) => {
      searchTicket += 1;
      set((state) => ({
        search: {
          ...state.search,
          tag,
          hits: [],
          status: tag === null && state.search.query.trim() === "" ? "idle" : "loading",
        },
      }));
    },

    setSearchFilters: (filters) => {
      searchTicket += 1;
      set((state) => {
        const next = { ...state.search, ...filters };
        return { search: { ...next, hits: [], status: next.tag === null && next.query.trim() === "" ? "idle" : "loading" } };
      });
    },

    closeSearch: () => {
      searchTicket += 1;
      set({ search: EMPTY_SEARCH });
    },

    runSearch: async (query) => {
      const ticket = (searchTicket += 1);
      const trimmed = query.trim().slice(0, MAX_QUERY_CHARS);
      const { tag, space, edited, titlesOnly } = get().search;
      if (trimmed === "" && tag === null) {
        set((state) => ({ search: { ...state.search, query, status: "idle", hits: [] } }));
        return;
      }
      set((state) => ({ search: { ...state.search, query, status: "loading" } }));
      try {
        const filters: SearchFilters = { space, edited, titlesOnly };
        const hits =
          tag === null
            ? await client.search(trimmed, { ...filterOptions(filters), limit: SEARCH_LIMIT })
            : await taggedHits(tag, trimmed, filters);
        if (ticket === searchTicket) {
          set((state) => ({ search: { ...state.search, status: "ready", hits } }));
        }
      } catch (error) {
        if (ticket === searchTicket) {
          set((state) => ({ search: { ...state.search, status: "error", hits: [] } }));
        }
        console.error("search failed", toCoreError(error).code);
      }
    },

    findPages: async (query) => {
      const trimmed = query.trim().slice(0, MAX_QUERY_CHARS);
      if (trimmed === "") {
        const { tree } = get();
        // The pages of the open space in tree order, without templates (ADR-0056); a search is filtered by the core.
        return flattenVisible(withoutTemplates(tree?.roots ?? []), new Set())
          .slice(0, PICKER_LIMIT)
          .map(({ page }) => ({ pageId: page.id, spaceId: page.spaceId, title: page.title, icon: page.icon }));
      }
      const hits = await client.search(trimmed, { scope: "Titles", limit: PICKER_LIMIT });
      const seen = new Set<PageId>();
      const choices: PageChoice[] = [];
      for (const hit of hits) {
        if (!seen.has(hit.pageId)) {
          seen.add(hit.pageId);
          choices.push({ pageId: hit.pageId, spaceId: hit.spaceId, title: hit.title, icon: hit.icon });
        }
      }
      return choices;
    },

    navigateTo: ({ pageId, spaceId, blockId, disposition }) => {
      // The tabs do the rest (ADR-0034): which tab shows the page, the space switch, the scroll target and the recent pages.
      const space = spaceId ?? get().currentSpaceId;
      if (space !== null) {
        get().openEntry({ spaceId: space, pageId, blockId }, disposition ?? "current");
      }
    },

    clearScrollTarget: () => {
      set({ scrollTarget: null });
    },

    exportMarkdown: (pageId) => client.pageExportMarkdown(pageId),

    openExternal: async (url) => {
      if (!isExternalHref(url)) {
        return;
      }
      let opened: boolean;
      try {
        opened = await client.openExternal(url);
      } catch (error) {
        // The shell refused (for example a scheme it does not allow): do not go around it.
        console.error("could not open the link", toCoreError(error).code);
        return;
      }
      if (!opened) {
        window.open(url, "_blank", "noopener,noreferrer");
      }
    },
  };

  /** Re-reads the referenced pages of a space (their title, icon or trash state changed). */
  function refreshLookups(spaceId: SpaceId): void {
    for (const [pageId, lookup] of Object.entries(get().pageLookup)) {
      if (lookup.status !== "found" || lookup.page.spaceId === spaceId) {
        refreshQuietly(() => fetchLookup(pageId));
      }
    }
  }

  return {
    initial,
    onEvent: (event) => {
      switch (event.type) {
        case "BacklinksChanged": {
          if (get().backlinks?.pageId === event.data.pageId) {
            refreshQuietly(() => fetchBacklinks(event.data.pageId));
          }
          break;
        }
        case "StructureChanged": {
          refreshLookups(event.data.spaceId);
          const open = get().backlinks?.pageId;
          if (open !== undefined) {
            refreshQuietly(() => fetchBacklinks(open)); // a referencing page may have been renamed
          }
          break;
        }
        case "PagesMovedSpace": {
          // What was looked up of a moved page names its old space (ADR-0038).
          for (const pageId of event.data.pageIds) {
            if (get().pageLookup[pageId] !== undefined) {
              refreshQuietly(() => fetchLookup(pageId));
            }
          }
          break;
        }
        // Content changes of the open page reach its document session directly.
      }
    },
  };
}
