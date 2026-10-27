// The view-only links part of the app store (Milestone 13, slice 4; ADR-0051): the links of the spaces whose share dialog was opened,
// as the core lists them, and the three actions that make, list and revoke them.
//
// As in the other slices, reads are "latest request wins" and events refresh: `Event.ViewLinksChanged` re-reads the list of a space
// that was read before (the first upload turning a link `Live`, a link made or revoked on another device, a space closed).
//
// The URL of a link holds its key, so the store never keeps one: `createViewLink` hands it to its caller once, and the lists keep
// the links without it (`ViewLinkEntry`). A lock clears the lists (`EMPTY_LINKS`).

import type { CoreClient, Event as CoreEvent, LinkId, PageId, SpaceId, ViewLinkInfo } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

/** A view link as the lists keep it: everything but the URL, which holds the link's key. */
export type ViewLinkEntry = Omit<ViewLinkInfo, "url">;

export interface LinksData {
  /** The links of each space whose list was read, newest first; a space is present once its list was read. */
  viewLinks: Readonly<Record<SpaceId, readonly ViewLinkEntry[]>>;
}

export const EMPTY_LINKS: LinksData = { viewLinks: {} };

/** Every action rejects with a `CoreError` when the core refuses (`Forbidden`, `InvalidInput`); the lists follow the answers and the events. */
export interface LinksActions {
  /** Reads the links of a space and keeps them (without their URLs) in `viewLinks`. */
  loadViewLinks: (spaceId: SpaceId) => Promise<void>;
  /** Makes a link for a page and answers it, URL included: the caller shows the URL once, and nothing keeps it. */
  createViewLink: (spaceId: SpaceId, pageId: PageId, expiresInDays?: number) => Promise<ViewLinkInfo>;
  /** Revokes a link (its creator, or an Owner); the list of the space becomes what remains. */
  revokeViewLink: (spaceId: SpaceId, linkId: LinkId) => Promise<void>;
}

export type LinksState = LinksData & LinksActions;

export interface LinksSlice {
  initial: LinksState;
  onEvent: (event: CoreEvent) => void;
}

/** Drops the URL of a link: the form the lists keep. */
function entryOf(link: ViewLinkInfo): ViewLinkEntry {
  return {
    linkId: link.linkId,
    pageId: link.pageId,
    createdBy: link.createdBy,
    createdByName: link.createdByName,
    createdAt: link.createdAt,
    ...(link.expiresAt === undefined ? {} : { expiresAt: link.expiresAt }),
    ...(link.lastPublishedAt === undefined ? {} : { lastPublishedAt: link.lastPublishedAt }),
    status: link.status,
  };
}

export function createLinksSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): LinksSlice {
  // Bumped by every read or action that answers a space's list; only the latest applies.
  const tickets = new Map<SpaceId, number>();

  function keep(spaceId: SpaceId, links: readonly ViewLinkInfo[], mine: number): void {
    if (tickets.get(spaceId) === mine) {
      set((state) => ({ viewLinks: { ...state.viewLinks, [spaceId]: links.map(entryOf) } }));
    }
  }

  function begin(spaceId: SpaceId): number {
    const mine = (tickets.get(spaceId) ?? 0) + 1;
    tickets.set(spaceId, mine);
    return mine;
  }

  function drop(spaceId: SpaceId): void {
    begin(spaceId);
    if (get().viewLinks[spaceId] !== undefined) {
      set((state) => ({ viewLinks: Object.fromEntries(Object.entries(state.viewLinks).filter(([id]) => id !== spaceId)) }));
    }
  }

  async function loadViewLinks(spaceId: SpaceId): Promise<void> {
    const mine = begin(spaceId);
    keep(spaceId, await client.viewLinkList(spaceId), mine);
  }

  const initial: LinksState = {
    ...EMPTY_LINKS,
    loadViewLinks,
    createViewLink: async (spaceId, pageId, expiresInDays) => {
      const link = await client.viewLinkCreate(spaceId, pageId, expiresInDays);
      // The core's `ViewLinksChanged` re-reads a list that was read; one that was not is read now, so that a dialog sees the link.
      refreshQuietly(() => loadViewLinks(spaceId));
      return link;
    },
    revokeViewLink: async (spaceId, linkId) => {
      const mine = begin(spaceId);
      keep(spaceId, await client.viewLinkRevoke(spaceId, linkId), mine);
    },
  };

  return {
    initial,
    onEvent: (event) => {
      if (get().phase === "locked") {
        // The next dialog reads what it needs.
        return;
      }
      switch (event.type) {
        case "ViewLinksChanged": {
          const { spaceId } = event.data;
          if (get().viewLinks[spaceId] !== undefined) {
            refreshQuietly(() => loadViewLinks(spaceId));
          }
          break;
        }
        case "SpaceRemoved":
          drop(event.data.spaceId);
          break;
        // The other events are not about view links.
      }
    },
  };
}
