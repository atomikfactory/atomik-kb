// The structure half of the app store: spaces, the page tree of the current space, the
// selected page and which pages are collapsed. `store.ts` composes it with the rest.
//
// Mutations only call the core; what changed comes back as events (`SpacesChanged`,
// `StructureChanged`) and the matching slice is re-read then, so nothing is fetched
// twice after a mutation. Reads are "latest request wins": a response is dropped when a
// newer request of its kind was sent meanwhile, so a slow answer never overwrites a
// fresher one.

import { CoreError, internalError } from "@atomik-kb/core-client";
import type {
  CoreClient,
  Event as CoreEvent,
  JsonValue,
  PageFont,
  PageId,
  PageInfo,
  PageNode,
  PropertyId,
  PropertyValue,
  SpaceId,
  SpaceInfo,
  SpaceMoveSummary,
  TemplatePlaceholder,
  TreeView,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { todayIso } from "./editor/dates";
import { activeLanguage } from "./i18n";
import { type SortKey, isSortedBy, sortedPageIds } from "./sort-order";
import { instantiateStarter } from "./starter-templates";
import type { AppState } from "./store";
import { findPage, locate } from "./tree";

/** Setting prefix: the pages collapsed in one space are stored as a JSON array under `<prefix>.<spaceId>`. */
export const COLLAPSED_KEY_PREFIX = "ui.sidebar.collapsed";

export function collapsedKey(spaceId: SpaceId): string {
  return `${COLLAPSED_KEY_PREFIX}.${spaceId}`;
}

/** Reads a stored list of collapsed page ids; anything unexpected means "nothing collapsed". */
export function parseCollapsed(value: JsonValue | null | undefined): ReadonlySet<PageId> {
  const ids = Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
  return new Set(ids);
}

/** What moving pages to another space does or would do (ADR-0038): the pages, and what cannot follow them. */
export type { SpaceMoveSummary };

export interface StructureData {
  /** Spaces in display order. */
  spaces: readonly SpaceInfo[];
  /** False until the spaces were read once, so the sidebar does not flash an empty state. */
  spacesLoaded: boolean;
  currentSpaceId: SpaceId | null;
  /** Tree, trash and undo state of the current space; `null` while it loads or without a space. */
  tree: TreeView | null;
  selectedPageId: PageId | null;
  /**
   * A page whose title the page view opens for editing as soon as it shows it: a page just
   * made from the text of another (`/page`), as the sidebar opens the rename field of a page
   * it just made. The page view clears it once it has taken it.
   */
  titleFocusPageId: PageId | null;
  /** Collapsed pages per space; a space is present once its setting was read. */
  collapsed: Readonly<Record<SpaceId, ReadonlySet<PageId>>>;
}

/** A move of a page and its sub-pages to another space, as the move dialog asks for it. */
export interface PageMoveToSpace {
  pageId: PageId;
  /** The space the page is in. */
  fromSpaceId: SpaceId;
  /** The space it goes to. */
  toSpaceId: SpaceId;
  /** Its parent there; `null` is the top level. */
  parentId: PageId | null;
  /** The pages of the target's tree to expand so that the page shows (the parent and its parents). */
  reveal?: readonly PageId[];
}

export interface CreatePageOptions {
  /** Parent page; leave out for the space root. */
  parentId?: PageId | null;
  /** Position among the siblings; leave out to append. */
  index?: number;
  /** Title; defaults to empty (shown as "Untitled"). */
  title?: string;
  /** Open the new page (default); `false` leaves the page that is open where it is. */
  select?: boolean;
}

/**
 * Every action returns a promise that rejects with a `CoreError` when the core refuses;
 * none of them refreshes the data itself, the events do.
 */
export interface StructureActions {
  /** Switches to a space and loads its tree. */
  selectSpace: (spaceId: SpaceId) => void;
  /** Creates a space and switches to it. */
  createSpace: (name: string, icon?: string) => Promise<SpaceInfo>;
  renameSpace: (spaceId: SpaceId, name: string) => Promise<void>;
  /** `null` clears the icon. */
  setSpaceIcon: (spaceId: SpaceId, icon: string | null) => Promise<void>;
  moveSpace: (spaceId: SpaceId, index: number) => Promise<void>;
  /** The core refuses while the space has live or trashed pages. */
  deleteSpace: (spaceId: SpaceId) => Promise<void>;

  /** Opens a page in the main area (`null` selects nothing). */
  selectPage: (pageId: PageId | null) => void;
  /** Asks the page view to open the title of `pageId` for editing when it shows it (`null` withdraws). */
  focusPageTitle: (pageId: PageId | null) => void;
  /** Creates a page in the current space, opens it and expands its parent. */
  createPage: (options?: CreatePageOptions) => Promise<PageInfo>;
  renamePage: (pageId: PageId, title: string) => Promise<void>;
  setPageIcon: (pageId: PageId, icon: string | null) => Promise<void>;
  /**
   * Sets, repositions or clears the cover of a page (ADR-0040): `cover` is `attachment:<id>`, `color:<name>` or
   * `gradient:<name>` and `coverY` the vertical position of its picture in percent; `cover` `null` with a position
   * moves the picture of the cover the page has, both `null` remove the cover. Rejects with a `CoreError`.
   */
  setPageCover: (pageId: PageId, cover: string | null, coverY?: number | null) => Promise<void>;
  /**
   * Sets or clears the font, the small text and the lock of a page (ADR-0040): a part left out stays as it is;
   * `font` `"default"`, `smallText` `false` and `locked` `false` clear. The open page follows through the tree.
   * Rejects with a `CoreError`.
   */
  setPageAppearance: (
    pageId: PageId,
    appearance: { font?: PageFont; smallText?: boolean; locked?: boolean },
  ) => Promise<void>;
  movePage: (pageId: PageId, parentId: PageId | null, index: number) => Promise<void>;
  /**
   * Sorts the direct subpages of a page (or the top-level pages of a space when `parentId` is `null`) once
   * (ADR-0052): `"title"` A to Z in the language of the app, `"created"` oldest first. One structure undo step; the
   * order stays editable by hand and nothing says the pages are sorted. The order is computed here (collation lives in
   * the UI) and the core applies it with `PageReorderChildren`; when the subpages changed meanwhile the core refuses
   * the order, and the tree is read again and the sort tried once more. Does nothing for fewer than two subpages or
   * when they already stand in that order. Rejects with a `CoreError`.
   */
  sortChildren: (spaceId: SpaceId, parentId: PageId | null, by: SortKey) => Promise<void>;
  /**
   * Duplicates a page with its sub-pages (ADR-0040): the copy goes right after the page, titled "... (copy)".
   * Then, as for a page just made, the copy's parent is expanded and the copy opens in the active tab. Answers the
   * copy; rejects with a `CoreError`. `open: false` leaves the page that is open where it is (the Templates panel's "Duplicate").
   */
  duplicatePage: (pageId: PageId, options?: { open?: boolean }) => Promise<PageInfo>;
  /** Makes a page a template or a page again (ADR-0056). Answers the page; rejects with a `CoreError`. */
  setPageTemplate: (pageId: PageId, template: boolean) => Promise<PageInfo>;
  /**
   * Sets (or clears, with `null`) a property's placeholder on a template page (ADR-0056): `today`/`now` for a date
   * property, `me` for a person property. Answers the page; rejects with a `CoreError`.
   */
  setTemplatePlaceholder: (pageId: PageId, propertyId: PropertyId, placeholder: TemplatePlaceholder | null) => Promise<PageInfo>;
  /**
   * Makes a page from a template (ADR-0056) under `parentId` (the space's top level when absent) at `index` (last when
   * absent) with `values` on the new page; `open` (default true) opens it in the active tab as a page just made, and
   * expands its parent. Answers the new page; rejects with a `CoreError`.
   */
  createFromTemplate: (
    templateId: PageId,
    options?: {
      parentId?: PageId;
      index?: number;
      values?: readonly (readonly [PropertyId, PropertyValue])[];
      open?: boolean;
    },
  ) => Promise<PageInfo>;
  /**
   * Makes a page of the current space from a built-in starter (ADR-0056, `starter-templates/`) in the language of the app, with
   * today's date and the time now, under `parentId` (the top level when absent); `open` (default true) opens it in the active tab
   * and expands its parent. Answers the new page; rejects with a `CoreError`.
   */
  createFromStarter: (starterId: string, options?: { parentId?: PageId; open?: boolean }) => Promise<PageInfo>;
  /**
   * "Add to my templates": makes a template page of the current space from a built-in starter (top level, its dates relative,
   * its title tokens kept); nothing opens. Answers the template; rejects with a `CoreError`.
   */
  addStarterAsTemplate: (starterId: string) => Promise<PageInfo>;
  trashPage: (pageId: PageId) => Promise<void>;
  restorePage: (pageId: PageId) => Promise<void>;
  purgePage: (pageId: PageId) => Promise<void>;
  /**
   * Adds a page to the favourites of its space (last) or removes it from them (ADR-0029). Does
   * nothing when it already is, or is not, a favourite.
   */
  setPageFavorite: (pageId: PageId, favorite: boolean) => Promise<void>;
  /** Moves a favourite in front of another favourite (`before`), or last when `before` is left out. */
  moveFavorite: (pageId: PageId, before?: PageId) => Promise<void>;
  /**
   * What moving a page and its sub-pages to another space would do (ADR-0038): the counts the confirmation
   * shows, nothing written. `parentId` `null` is the top level of `spaceId`. Rejects with a `CoreError`.
   */
  planPageMoveToSpace: (pageId: PageId, spaceId: SpaceId, parentId: PageId | null) => Promise<SpaceMoveSummary>;
  /**
   * Moves a page and its sub-pages to another space, last under `parentId` (`null` is the top level), keeping
   * their ids. Then shows them there: the pages of `reveal` (the parent and its parents) are expanded in the
   * target's tree, the sidebar switches to the target space and the page opens. Rejects with a `CoreError`.
   */
  movePageToSpace: (move: PageMoveToSpace) => Promise<SpaceMoveSummary>;
  /** The live tree of any space (the move dialog's picker); the tree of the current space in the store is not touched. */
  readTree: (spaceId: SpaceId) => Promise<TreeView>;
  /** Reads the spaces again (their page counts), for a dialog that reports on a change as soon as it is done. */
  refreshSpaces: () => Promise<void>;
  /** Undoes the last structural change in the current space. */
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  /** Collapses or expands a page in the current space and remembers it. */
  setPageCollapsed: (pageId: PageId, collapsed: boolean) => void;
}

export type StructureState = StructureData & StructureActions;

export const EMPTY_STRUCTURE: StructureData = {
  spaces: [],
  spacesLoaded: false,
  currentSpaceId: null,
  tree: null,
  selectedPageId: null,
  titleFocusPageId: null,
  collapsed: {},
};

export const selectCurrentSpace = (state: StructureData): SpaceInfo | null =>
  state.spaces.find((space) => space.id === state.currentSpaceId) ?? null;

const NO_FAVORITES: readonly PageId[] = [];

/** The live favourites of the current space in order (the tree carries them; ADR-0029). */
export const selectFavorites = (state: StructureData): readonly PageId[] => state.tree?.favorites ?? NO_FAVORITES;

/** Whether a live page of the current space is locked (ADR-0040): `false` for a page that is not in the tree. */
export const selectPageLocked = (state: StructureData, pageId: PageId | null): boolean => {
  if (state.tree === null || pageId === null) {
    return false;
  }
  return findPage(state.tree.roots, pageId)?.locked === true;
};

export interface StructureSlice {
  /** Initial data and the actions, to be part of the store's initial state. */
  initial: StructureState;
  /**
   * Reads the spaces and one space's tree, for the boot sequence: the space `chooseSpace` names
   * (the one of the active tab, ADR-0034), else the first. The selection stays empty: the tabs say what is open.
   */
  load: (chooseSpace?: (spaces: readonly SpaceInfo[]) => Promise<SpaceId | undefined> | SpaceId | undefined) => Promise<StructureData>;
  /** Reads the tree of the current space again (the tabs slice calls it when a tab brings another space in front). */
  loadCurrent: () => Promise<void>;
  /** Re-reads what an event says changed; other events are not its business. */
  onEvent: (event: CoreEvent) => void;
}

export function createStructureSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): StructureSlice {
  // Bumped by every request of its kind; only the response to the latest one is applied.
  let spacesTicket = 0;
  let treeTicket = 0;

  function requireSpaceId(): SpaceId {
    const spaceId = get().currentSpaceId;
    if (spaceId === null) {
      throw internalError("no space is selected");
    }
    return spaceId;
  }

  async function readCollapsed(spaceId: SpaceId): Promise<ReadonlySet<PageId>> {
    try {
      return parseCollapsed(await client.settingsGet(collapsedKey(spaceId)));
    } catch (error) {
      console.error("could not read which pages are collapsed", error);
      return new Set();
    }
  }

  /** Reads the tree of the current space and, the first time, its collapsed pages. */
  async function loadCurrent(): Promise<void> {
    const spaceId = get().currentSpaceId;
    if (spaceId === null) {
      return;
    }
    const ticket = (treeTicket += 1);
    const known = get().collapsed[spaceId];
    const [tree, collapsed] = await Promise.all([client.treeGet(spaceId), known ?? readCollapsed(spaceId)]);
    if (ticket !== treeTicket || get().currentSpaceId !== spaceId) {
      return;
    }
    set((state) => ({
      tree,
      collapsed:
        state.collapsed[spaceId] === undefined ? { ...state.collapsed, [spaceId]: collapsed } : state.collapsed,
    }));
  }

  async function refreshSpaces(): Promise<void> {
    const ticket = (spacesTicket += 1);
    const spaces = await client.spaceList();
    if (ticket !== spacesTicket) {
      return;
    }
    // The tabs slice reacts to the new list (ADR-0034): the entries of a deleted space leave the
    // histories, so the current space continues in the tab that is left, and a first space gets
    // its Home tab. Both move `currentSpaceId` and load the tree of the new current space.
    set({ spaces, spacesLoaded: true });
    const { currentSpaceId } = get();
    if (currentSpaceId !== null && spaces.some((space) => space.id === currentSpaceId)) {
      return;
    }
    if (currentSpaceId === null && spaces.length === 0) {
      return;
    }
    // No tab took it on (there is no tab slice around, or the session is not running): continue in the first space.
    set({ currentSpaceId: spaces[0]?.id ?? null, tree: null, selectedPageId: null });
    await loadCurrent();
  }

  async function load(
    chooseSpace?: (spaces: readonly SpaceInfo[]) => Promise<SpaceId | undefined> | SpaceId | undefined,
  ): Promise<StructureData> {
    const spaces = await client.spaceList();
    const chosen = await chooseSpace?.(spaces);
    const first = spaces.find((space) => space.id === chosen)?.id ?? spaces[0]?.id;
    if (first === undefined) {
      return { ...EMPTY_STRUCTURE, spaces, spacesLoaded: true };
    }
    const [tree, collapsed] = await Promise.all([client.treeGet(first), readCollapsed(first)]);
    return {
      spaces,
      spacesLoaded: true,
      currentSpaceId: first,
      tree,
      selectedPageId: null,
      titleFocusPageId: null,
      collapsed: { [first]: collapsed },
    };
  }

  /** The direct children of a page (the top-level pages for `null`): from the tree in the store when it is the open space's and `fresh` is off, else read from the core. */
  async function childrenOf(spaceId: SpaceId, parentId: PageId | null, fresh: boolean): Promise<readonly PageNode[]> {
    const state = get();
    const tree = !fresh && state.currentSpaceId === spaceId && state.tree !== null ? state.tree : await client.treeGet(spaceId);
    return parentId === null ? tree.roots : (locate(tree.roots, parentId)?.node.children ?? []);
  }

  function setPageCollapsed(pageId: PageId, collapsed: boolean): void {
    const spaceId = get().currentSpaceId;
    const current = spaceId === null ? undefined : get().collapsed[spaceId];
    if (spaceId === null || current === undefined || current.has(pageId) === collapsed) {
      return; // No space, nothing read yet (writing now would drop what is stored), or no change.
    }
    const next = new Set(current);
    if (collapsed) {
      next.add(pageId);
    } else {
      next.delete(pageId);
    }
    set((state) => ({ collapsed: { ...state.collapsed, [spaceId]: next } }));
    client.settingsSet(collapsedKey(spaceId), [...next]).catch((error: unknown) => {
      console.error("could not remember which pages are collapsed", error);
    });
  }

  /** Expands pages in a space (not only the current one) and remembers it, so that a page shows when it opens there. */
  async function expandPages(spaceId: SpaceId, pageIds: readonly PageId[]): Promise<void> {
    if (pageIds.length === 0) {
      return;
    }
    const known = get().collapsed[spaceId] ?? (await readCollapsed(spaceId));
    // Whatever was set while the setting was read wins.
    const latest = get().collapsed[spaceId] ?? known;
    const next = new Set(latest);
    for (const pageId of pageIds) {
      next.delete(pageId);
    }
    if (get().collapsed[spaceId] === undefined || next.size !== latest.size) {
      set((state) => ({ collapsed: { ...state.collapsed, [spaceId]: next } }));
    }
    if (next.size !== latest.size) {
      client.settingsSet(collapsedKey(spaceId), [...next]).catch((error: unknown) => {
        console.error("could not remember which pages are collapsed", error);
      });
    }
  }

  const initial: StructureState = {
    ...EMPTY_STRUCTURE,

    selectSpace: (spaceId) => {
      // The tabs decide (ADR-0034): a tab already in that space comes to the front, else the active tab goes to its Home.
      get().switchSpace(spaceId);
    },
    createSpace: async (name, icon) => {
      const space = await client.spaceCreate(name, icon);
      get().selectSpace(space.id);
      return space;
    },
    renameSpace: async (spaceId, name) => {
      await client.spaceRename(spaceId, name);
    },
    setSpaceIcon: async (spaceId, icon) => {
      await client.spaceSetIcon(spaceId, icon);
    },
    moveSpace: async (spaceId, index) => {
      await client.spaceMove(spaceId, index);
    },
    deleteSpace: async (spaceId) => {
      await client.spaceDelete(spaceId);
    },

    selectPage: (pageId) => {
      // A plain open in the current space (ADR-0034): the active tab navigates. `null` is the Home of the space.
      const spaceId = get().currentSpaceId;
      if (spaceId === null) {
        return;
      }
      if (pageId === null) {
        get().openEntry({ spaceId, pageId: null });
      } else {
        get().navigateTo({ pageId, spaceId });
      }
    },
    focusPageTitle: (pageId) => {
      set({ titleFocusPageId: pageId });
    },
    createPage: async (options = {}) => {
      const spaceId = requireSpaceId();
      const page = await client.pageCreate({
        spaceId,
        parentId: options.parentId ?? null,
        index: options.index,
        title: options.title ?? "",
      });
      if (options.parentId !== undefined && options.parentId !== null) {
        setPageCollapsed(options.parentId, false);
      }
      if (options.select !== false) {
        get().navigateTo({ pageId: page.id, spaceId });
      }
      return page;
    },
    renamePage: async (pageId, title) => {
      await client.pageRename(pageId, title);
    },
    setPageIcon: async (pageId, icon) => {
      await client.pageSetIcon(pageId, icon);
    },
    setPageCover: async (pageId, cover, coverY) => {
      await client.pageSetCover(pageId, cover, coverY);
    },
    setPageAppearance: async (pageId, appearance) => {
      await client.pageSetAppearance(pageId, appearance);
    },
    movePage: async (pageId, parentId, index) => {
      await client.pageMove(pageId, parentId, index);
    },
    sortChildren: async (spaceId, parentId, by) => {
      for (let attempt = 0; ; attempt += 1) {
        const pages = (await childrenOf(spaceId, parentId, attempt > 0)).map((node) => node.page);
        if (pages.length < 2 || isSortedBy(pages, by)) {
          return;
        }
        try {
          await client.pageReorderChildren(spaceId, parentId, sortedPageIds(pages, by));
          return;
        } catch (error) {
          // The subpages are not the ones the order was made from (a page came, went or moved meanwhile): once more
          // from what the core has now. Anything else, or a second miss, is the caller's.
          const stale =
            error instanceof CoreError &&
            error.code === "InvalidInput" &&
            typeof error.details === "object" &&
            error.details !== null &&
            "field" in error.details &&
            error.details.field === "order";
          if (!stale || attempt > 0) {
            throw error;
          }
        }
      }
    },
    duplicatePage: async (pageId, options = {}) => {
      const copy = await client.pageDuplicate(pageId);
      // The copy sits beside the page, whose parent is open when the page is on screen; a collapsed one opens.
      if (copy.parentId !== undefined) {
        setPageCollapsed(copy.parentId, false);
      }
      if (options.open !== false) {
        get().navigateTo({ pageId: copy.id, spaceId: copy.spaceId });
      }
      return copy;
    },
    setPageTemplate: async (pageId, template) => client.pageSetTemplate(pageId, template),
    setTemplatePlaceholder: async (pageId, propertyId, placeholder) =>
      client.pageSetTemplatePlaceholder(pageId, propertyId, placeholder ?? undefined),
    createFromTemplate: async (templateId, options = {}) => {
      const { open = true, ...rest } = options;
      const page = await client.pageCreateFromTemplate(templateId, rest);
      if (open) {
        if (page.parentId !== undefined) {
          setPageCollapsed(page.parentId, false);
        }
        get().navigateTo({ pageId: page.id, spaceId: page.spaceId });
      }
      return page;
    },
    createFromStarter: async (starterId, options = {}) => {
      const spaceId = requireSpaceId();
      const now = new Date();
      const page = await instantiateStarter(client, starterId, {
        spaceId,
        ...(options.parentId === undefined ? {} : { parentId: options.parentId }),
        language: activeLanguage(),
        today: todayIso(now),
        now: `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}`,
      });
      if (options.open !== false) {
        if (page.parentId !== undefined) {
          setPageCollapsed(page.parentId, false);
        }
        get().navigateTo({ pageId: page.id, spaceId: page.spaceId });
      }
      return page;
    },
    addStarterAsTemplate: async (starterId) => {
      const now = new Date();
      return instantiateStarter(client, starterId, {
        spaceId: requireSpaceId(),
        asTemplate: true,
        language: activeLanguage(),
        today: todayIso(now),
        now: `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}`,
      });
    },
    trashPage: async (pageId) => {
      await client.pageTrash(pageId);
    },
    restorePage: async (pageId) => {
      await client.pageRestore(pageId);
    },
    purgePage: async (pageId) => {
      await client.pagePurge(pageId);
    },
    setPageFavorite: async (pageId, favorite) => {
      await client.pageFavoriteSet(pageId, favorite);
    },
    moveFavorite: async (pageId, before) => {
      await client.pageFavoriteMove(pageId, before);
    },
    planPageMoveToSpace: (pageId, spaceId, parentId) => client.pageMoveToSpacePlan(pageId, spaceId, parentId),
    movePageToSpace: async ({ pageId, fromSpaceId, toSpaceId, parentId, reveal = [] }) => {
      const summary = await client.pageMoveToSpace(pageId, toSpaceId, parentId, null);
      // The tabs that show the pages follow them (the event that comes with the move says the same, and finds
      // nothing left to do), then the page opens in its new place wherever the menu was used (a page that was not open).
      get().followMovedPages([pageId, ...summary.pageIds], fromSpaceId, toSpaceId);
      await expandPages(toSpaceId, reveal);
      get().navigateTo({ pageId, spaceId: toSpaceId });
      return summary;
    },
    readTree: (spaceId) => client.treeGet(spaceId),
    refreshSpaces,
    undo: async () => {
      await client.structureUndo(requireSpaceId());
    },
    redo: async () => {
      await client.structureRedo(requireSpaceId());
    },
    setPageCollapsed,
  };

  return {
    initial,
    load,
    loadCurrent,
    onEvent: (event) => {
      switch (event.type) {
        case "WorkspaceCreated": // the workspace may come with its first space
        case "SpacesChanged":
          refreshQuietly(refreshSpaces);
          break;
        case "StructureChanged":
          if (event.data.spaceId === get().currentSpaceId) {
            refreshQuietly(loadCurrent);
          }
          break;
        case "PagesMovedSpace":
          // Both spaces changed (ADR-0038): their page counts, and the tree of the one that is open (the tabs
          // follow the pages and may bring the other one in front, which loads its own tree).
          refreshQuietly(refreshSpaces);
          if (event.data.fromSpaceId === get().currentSpaceId || event.data.toSpaceId === get().currentSpaceId) {
            refreshQuietly(loadCurrent);
          }
          break;
        // The other events are not about the structure.
      }
    },
  };
}
