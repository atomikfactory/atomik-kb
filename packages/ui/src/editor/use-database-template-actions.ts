// What the menu next to "New" does with the templates of a database block (ADR-0056, `docs/architecture/templates.md`
// section 7): edit one (the side peek), duplicate one, move one to the trash, and make a new one under the page that holds
// the block. `PageListView` provides them through `DatabaseTemplatesContext`; a failure is reported through `onError` (the
// block's own error line), never thrown, and the slow ones tell `onBusy` so that the "New" controls wait. A new template is
// made under the page "New" makes rows under (`sourceParentId`: the source page of a linked `page` source, else the host page).

import { TAGS_PROPERTY_ID, toCoreError } from "@atomik-kb/core-client";
import type { PageId, PageInfo } from "@atomik-kb/core-client";
import { useMemo } from "react";

import { t } from "../i18n";
import { useAppStore, useStoreApi } from "../store-context";
import { tagValueFor } from "./database-templates";
import type { DatabaseTemplateActions } from "./database-templates";
import { openRow } from "./open-row";
import type { PageListSource } from "./page-list-model";
import { sourceParentId } from "./page-list-sources";

export interface DatabaseTemplateContext {
  /** The page that holds the block; no template can be made without it, except under the source page of a `page` source. */
  readonly hostId: PageId | null;
  readonly source: PageListSource | null;
  readonly onBusy: (busy: boolean) => void;
  readonly onError: (message: string | null) => void;
}

/** The actions of the menu for the block whose host page, source and report channels are `context`. */
export function useDatabaseTemplateActions({ hostId, source, onBusy, onError }: DatabaseTemplateContext): DatabaseTemplateActions {
  const store = useStoreApi();
  const createPage = useAppStore((state) => state.createPage);
  const setPageProperty = useAppStore((state) => state.setPageProperty);
  const setPageTemplate = useAppStore((state) => state.setPageTemplate);
  const duplicatePage = useAppStore((state) => state.duplicatePage);
  const trashPage = useAppStore((state) => state.trashPage);

  return useMemo(() => {
    /** Runs `work` as a change of the templates: the block's controls wait for it, and a refusal is its error line. */
    async function run(work: () => Promise<void>): Promise<void> {
      onBusy(true);
      onError(null);
      try {
        await work();
      } catch (cause) {
        onError(t("databaseTemplate.error", { message: toCoreError(cause).message }));
      } finally {
        onBusy(false);
      }
    }

    function edit(template: PageInfo): void {
      // Always the side peek, whatever the view's `openIn` says; `openRow` keeps its guards (the page that holds the block, or the
      // page in front, opens as a page instead: it cannot be edited in two places).
      openRow(store.getState(), { pageId: template.id, spaceId: template.spaceId }, { openIn: "side" }, "current", hostId);
    }

    return {
      edit,
      duplicate: (template) =>
        run(async () => {
          await duplicatePage(template.id, { open: false });
        }),
      remove: (template) =>
        run(async () => {
          await trashPage(template.id);
        }),
      create: () =>
        run(async () => {
          // Under the source page of a `page` source, the page that holds the block otherwise.
          const parentId = source === null ? null : sourceParentId(source, hostId);
          if (parentId === null || source === null) {
            return;
          }
          const page = await createPage({ parentId, title: t("databaseTemplate.templateTitle"), select: false });
          if (source.kind === "tag") {
            await setPageProperty(page.id, TAGS_PROPERTY_ID, tagValueFor(undefined, source.optionId));
          }
          await setPageTemplate(page.id, true);
          edit(page);
        }),
    };
  }, [store, hostId, source, onBusy, onError, createPage, setPageProperty, setPageTemplate, duplicatePage, trashPage]);
}
