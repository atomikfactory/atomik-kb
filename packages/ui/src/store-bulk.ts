// Bulk edit of database rows in the store (ADR-0058, `docs/architecture/database-power-ups.md` section 5): the four actions of the
// action bar (`editor/PageListBulkBar.tsx`) over the one core request `PageBulk`, each followed by a toast with "Undo".
//
// - Move, Duplicate and Trash are one structural undo step in the core, so their Undo is `StructureUndo` of the space.
// - A value change is not in structural undo (as a single `PagePropertySet`): its Undo sends the previous values back, one per
//   page, as `SetValues`. The previous values are read from the pages the caller passes (the rows' `PageInfo`) before the change.
//
// What changed comes back as events (`StructureChanged`) like for every other change, so nothing is re-read here.

import { toCoreError } from "@atomik-kb/core-client";
import type { BulkAction, CoreClient, PageId, PageInfo, PropertyId, PropertyValue, SpaceId } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { valueOfPage } from "./editor/page-list-model";
import { t, tp } from "./i18n";
import type { AppState } from "./store";
import type { ToastAction } from "./store-interop";

/** The most pages one `PageBulk` takes (the core refuses more with `InvalidInput` for `pageIds`): the bar's actions wait for fewer. */
export const BULK_LIMIT = 1000;

/**
 * Every action rejects with a `CoreError` when the core refuses (nothing was changed then, the request is all or nothing) and
 * answers the pages the core acted on; a toast says what was done.
 */
export interface BulkActions {
  /**
   * Sets a property of `pages` to `value`, or clears it with `null` (toast "Set Status on 3 pages", Undo puts each page's previous
   * value back). Answers the pages changed.
   */
  bulkSetValue: (spaceId: SpaceId, pages: readonly PageInfo[], propertyId: PropertyId, value: PropertyValue | null) => Promise<PageId[]>;
  /** Moves pages to the end of `parentId`'s children (`null`: the top level), in the order given. Undo is a structural undo. */
  bulkMovePages: (spaceId: SpaceId, pageIds: readonly PageId[], parentId: PageId | null) => Promise<PageId[]>;
  /** Duplicates pages, each copy right after its page; answers the top copies. Undo is a structural undo. */
  bulkDuplicatePages: (spaceId: SpaceId, pageIds: readonly PageId[]) => Promise<PageId[]>;
  /** Moves pages, with their sub-pages, to the trash. Undo is a structural undo. */
  bulkTrashPages: (spaceId: SpaceId, pageIds: readonly PageId[]) => Promise<PageId[]>;
}

export type BulkState = BulkActions;

export interface BulkSlice {
  /** The actions, to be part of the store's initial state. */
  initial: BulkState;
}

export function createBulkSlice(client: CoreClient, get: StoreApi<AppState>["getState"]): BulkSlice {
  /** The name of a property in a toast (the schema is read when the bar opens its menu; a property that is gone has none). */
  function propertyName(spaceId: SpaceId, propertyId: PropertyId): string {
    return get().propertySchemas[spaceId]?.find((def) => def.id === propertyId)?.name ?? "";
  }

  async function undoStructure(spaceId: SpaceId): Promise<void> {
    try {
      await client.structureUndo(spaceId);
      get().showToast(t("pageList.bulk.toast.undone"));
    } catch (error) {
      get().showToast(t("pageList.bulk.toast.undoError", { message: toCoreError(error).message }), "error");
    }
  }

  async function undoValues(spaceId: SpaceId, pageIds: readonly PageId[], propertyId: PropertyId, values: (PropertyValue | null)[]): Promise<void> {
    try {
      await client.pageBulk(spaceId, pageIds, { type: "SetValues", data: { propertyId, values } });
      get().showToast(t("pageList.bulk.toast.undone"));
    } catch (error) {
      get().showToast(t("pageList.bulk.toast.undoError", { message: toCoreError(error).message }), "error");
    }
  }

  /** A structural action: one request, then the toast whose Undo is the space's structural undo. */
  async function structural(
    spaceId: SpaceId,
    pageIds: readonly PageId[],
    action: BulkAction,
    text: (count: number) => string,
  ): Promise<PageId[]> {
    const done = await client.pageBulk(spaceId, pageIds, action);
    const undo: ToastAction = {
      label: t("pageList.bulk.toast.undo"),
      run: () => {
        void undoStructure(spaceId);
      },
    };
    get().showToast(text(done.length), "info", undo);
    return done;
  }

  return {
    initial: {
      bulkSetValue: async (spaceId, pages, propertyId, value) => {
        const pageIds = pages.map((page) => page.id);
        // What each page holds now, in the order of the request: Undo sends exactly this back.
        const previous = pages.map((page) => valueOfPage(page, propertyId) ?? null);
        const done = await client.pageBulk(spaceId, pageIds, { type: "SetValue", data: { propertyId, value } });
        const undo: ToastAction = {
          label: t("pageList.bulk.toast.undo"),
          run: () => {
            void undoValues(spaceId, pageIds, propertyId, previous);
          },
        };
        const property = propertyName(spaceId, propertyId);
        get().showToast(tp(value === null ? "pageList.bulk.toast.cleared" : "pageList.bulk.toast.set", done.length, { property }), "info", undo);
        return done;
      },
      bulkMovePages: (spaceId, pageIds, parentId) =>
        structural(spaceId, pageIds, { type: "Move", data: parentId === null ? {} : { parentId } }, (count) =>
          tp("pageList.bulk.toast.moved", count),
        ),
      bulkDuplicatePages: (spaceId, pageIds) =>
        structural(spaceId, pageIds, { type: "Duplicate" }, (count) => tp("pageList.bulk.toast.duplicated", count)),
      bulkTrashPages: (spaceId, pageIds) =>
        structural(spaceId, pageIds, { type: "Trash" }, (count) => tp("pageList.bulk.toast.trashed", count)),
    },
  };
}
