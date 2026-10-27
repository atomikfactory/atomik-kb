// Page history in the store (ADR-0057, `docs/architecture/page-history.md` section 7): the calls the history dialog makes, and the
// restore that says so in a toast with "Undo". Nothing is kept here: the dialog holds the list it shows, the toast holds the version
// Undo goes back to.

import { toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, PageContent, PageHistory, PageId, PageInfo, PageVersionContent } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { t } from "./i18n";
import type { AppState } from "./store";

/** The history calls the dialog makes. */
export function historyActions(client: CoreClient) {
  return {
    list: (pageId: PageId, before?: string): Promise<PageHistory> =>
      client.pageHistoryList(pageId, before === undefined ? {} : { before }),
    get: (pageId: PageId, version: string): Promise<PageVersionContent> => client.pageHistoryGet(pageId, version),
    restore: (pageId: PageId, version: string): Promise<PageInfo> => client.pageHistoryRestore(pageId, version),
    /** The page's content now (what a version is compared with). */
    content: (pageId: PageId): Promise<PageContent> => client.pageContentGet(pageId),
  };
}

/** What the dialog asks of the store. */
export interface HistoryActions {
  /** The versions of a page, newest first; `before` resumes after a version id of an earlier answer. Rejects with a `CoreError`. */
  listPageVersions: (pageId: PageId, before?: string) => Promise<PageHistory>;
  /** The content (and title, when known) of a page at a version. Rejects with a `CoreError`. */
  getPageVersion: (pageId: PageId, version: string) => Promise<PageVersionContent>;
  /** The page's content now. Rejects with a `CoreError`. */
  getPageContent: (pageId: PageId) => Promise<PageContent>;
  /**
   * Makes the page what it was at `version` (a new change; the version before stays in the history), then says so in a toast
   * "Page restored" whose "Undo" restores the version that was current when this was asked: the one the core lists as current now,
   * else `knownCurrent` (the one the dialog listed). Rejects with a `CoreError` when the core refuses (nothing is said then).
   */
  restorePageVersion: (pageId: PageId, version: string, knownCurrent: string | null) => Promise<PageInfo>;
}

export type HistoryState = HistoryActions;

export interface HistorySlice {
  initial: HistoryState;
}

export function createHistorySlice(client: CoreClient, get: StoreApi<AppState>["getState"]): HistorySlice {
  const calls = historyActions(client);

  /** The version the core lists as the page's content now; `null` when it does not say (the list is not read). */
  async function currentVersion(pageId: PageId): Promise<string | null> {
    try {
      const history = await client.pageHistoryList(pageId, {});
      return history.versions.find((version) => version.current === true)?.id ?? null;
    } catch {
      return null;
    }
  }

  /** Undo of a restore: restores the version that was current before it; a refusal is said in an error toast. */
  async function undo(pageId: PageId, previous: string): Promise<void> {
    try {
      await calls.restore(pageId, previous);
      get().showToast(t("history.toast.undone"));
    } catch (error) {
      get().showToast(t("history.toast.undoError", { message: toCoreError(error).message }), "error");
    }
  }

  return {
    initial: {
      listPageVersions: calls.list,
      getPageVersion: calls.get,
      getPageContent: calls.content,
      restorePageVersion: async (pageId, version, knownCurrent) => {
        const previous = (await currentVersion(pageId)) ?? knownCurrent;
        const page = await calls.restore(pageId, version);
        const action =
          previous === null || previous === version
            ? undefined
            : {
                label: t("history.toast.undo"),
                run: () => {
                  void undo(pageId, previous);
                },
              };
        get().showToast(t("history.toast.restored"), "info", action);
        return page;
      },
    },
  };
}
