// The view of the list view block (ADR-0016, amendments of 2026-10-07 and 2026-10-08; ADR-0054, several views): the
// pages of its source drawn by the layout of the active view. The block's settings are read as *views*
// (`page-list-views.ts`: a block without `views` is one view made from its legacy attributes) and the one shown is
// this device's choice (`store-views.ts`, never synced: a tab switch writes nothing to the page, so a viewer switches
// tabs too). A strip of tabs above the block (`PageListViewTabs.tsx`) shows them, adds, renames, copies, moves,
// changes the layout of and deletes them; the toolbar, at the end of that row, edits the active one.
//
// The pages come from the app's page tree (the children of the page the editor shows, in tree order) or, for a tag
// source, from the pages of the space that carry the Tags option (`pagesWithOption`, asked again whenever the tree or
// the property schema changes, which the events `StructureChanged` and `PropertySchemaChanged` cause). Templates
// (ADR-0056) are told apart first: the pages that are templates, or inside one, are not rows (`database-templates.ts`),
// and the templates among them are what the menu next to each layout's "New" lists (`NewPageMenu.tsx`). The rows pass the
// view's filter tree (`applyFilterTree`, AND and OR, one level of groups) and are sorted as the view says (`use-view-rows.ts`,
// which the dashboard's embedded views run too); these `rows` go to the component the layout registry names for the view's layout (`page-list-layouts.tsx`: the list,
// the table, the board, the calendar, the gallery), with the props every layout takes
// (`page-list-layout-types.ts`). A layout never writes the block's attributes: it asks to change the view through
// `onUpdateView`, to open a page through `onOpen` (`openRow`: the side peek, the center peek, the page or another
// tab) and to make one through `onCreatePage`.
//
// Writing follows `page-list-write.ts`: a block that has no `views` keeps writing one legacy attribute at a time as
// long as the change fits them, anything else writes `views` and the mirror of the first view in one transaction. The
// view writes nothing on its own. The toolbar (Filter, Sort, Properties and the view settings menu, at the end of the row
// of the tabs) edits the active view; its settings menu exports the rows the view shows as CSV (`csv-export.ts`, ADR-0036;
// the table's calculation footer is not in it).

import { TAGS_PROPERTY_ID, toCoreError } from "@atomik-kb/core-client";
import type { OptionId, PageId, PageInfo, PropertyDef, PropertyId, PropertyValue, SpaceId, TreeView } from "@atomik-kb/core-client";
import { NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useEffect, useMemo, useState } from "react";
import type { KeyboardEvent } from "react";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { openGestures } from "../page-open";
import { useAppStore, useStoreApi } from "../store-context";
import { selectActiveViewId } from "../store-views";
import type { OpenDisposition } from "../store-tabs";
import { findPage } from "../tree";
import { useToday } from "./today";
import { resolveGroup } from "./page-board-model";
import { boardSelection, csvFileName, listSelection } from "./csv-export";
import { DashboardHostContext } from "./dashboard/dashboard-host";
import type { DashboardHost } from "./dashboard/dashboard-host";
import { DatabaseTemplatesContext, resolveDefaultTemplate, splitTemplates, tagValueFor } from "./database-templates";
import type { DatabaseTemplates } from "./database-templates";
import { PageListEmptyContext } from "./page-list-empty";
import { layoutChangePatch, layoutDef, withInitialPatch } from "./page-list-layouts";
import type { NewPageInit, RowSelectionProps } from "./page-list-layout-types";
import {
  addView,
  canAddView,
  duplicateView,
  moveView,
  newViewId,
  parseViews,
  removeView,
  resolveActiveView,
  viewLabel,
} from "./page-list-views";
import type { PageListViewPatch, PageListViewSpec } from "./page-list-views";
import { sameViews, writeForPatch, writeForViews } from "./page-list-write";
import { openRow } from "./open-row";
import { readSource } from "./page-list-model";
import type { PageListLayout, PageListSource } from "./page-list-model";
import { sourceParentId, sourcePages } from "./page-list-sources";
import type { SourcePages } from "./page-list-sources";
import {
  EMPTY_SELECTION,
  pruneSelection,
  selectAll,
  selectedInOrder,
  selectionShortcut,
  selectOnly,
  selectRange,
  toggleRow,
} from "./page-list-selection";
import type { RowSelection } from "./page-list-selection";
import { PageListBulkBar } from "./PageListBulkBar";
import { PageListSourceChooser } from "./PageListSourceChooser";
import { PageListToolbar } from "./PageListToolbar";
import type { PageListChange, PageListCsvExport } from "./PageListToolbar";
import { PageListViewTabs } from "./PageListViewTabs";
import { useDatabaseTemplateActions } from "./use-database-template-actions";
import { useViewRows } from "./use-view-rows";

const NO_DEFS: readonly PropertyDef[] = [];
const NO_KEYS: readonly string[] = [];
const NO_ROWS: readonly PageInfo[] = [];
const NO_SOURCE_PAGES: SourcePages = { pages: [], missing: false };

/** The `pageId` option of the list view extension, `null` when it has none. */
function pageIdOf(options: unknown): PageId | null {
  if (typeof options !== "object" || options === null || !("pageId" in options)) {
    return null;
  }
  return typeof options.pageId === "string" && options.pageId !== "" ? options.pageId : null;
}

interface Tagged {
  optionId: OptionId;
  ids: ReadonlySet<PageId>;
}

/**
 * The ids of the pages of the space that carry the Tags option `optionId` (`null` while the answer
 * for it is on its way, or for another source). Asked again when the page tree or the property schema
 * changes: the latest answer wins.
 */
function useTaggedPageIds(
  optionId: OptionId | null,
  spaceId: SpaceId | null,
  tree: TreeView | null,
  schema: readonly PropertyDef[],
): ReadonlySet<PageId> | null {
  const pagesWithOption = useAppStore((state) => state.pagesWithOption);
  const [tagged, setTagged] = useState<Tagged | null>(null);
  useEffect(() => {
    if (optionId === null || spaceId === null) {
      return;
    }
    let current = true;
    pagesWithOption(spaceId, TAGS_PROPERTY_ID, optionId).then(
      (ids) => {
        if (current) {
          setTagged({ optionId, ids: new Set(ids) });
        }
      },
      (error: unknown) => {
        console.error("could not list the pages with a tag", error);
      },
    );
    return () => {
      current = false;
    };
    // `tree` and `schema` are not read here: a change of either asks again.
  }, [optionId, spaceId, tree, schema, pagesWithOption]);
  return tagged !== null && tagged.optionId === optionId ? tagged.ids : null;
}

/**
 * What the heading of the view says about its source (`tagsDef` is the Tags property, once the schema is read; `sourcePage` the
 * source page of a linked view, `null` when it is not in the tree).
 */
function headingOf(source: PageListSource | null, tagsDef: PropertyDef | undefined, sourcePage: PageInfo | null): string {
  if (source === null || (source.kind === "tag" && tagsDef === undefined)) {
    return t("pageList.label");
  }
  switch (source.kind) {
    case "children":
      return t("pageList.source.children");
    case "space":
      return t("pageList.source.space");
    case "page":
      return sourcePage === null
        ? t("pageList.source.pageGone")
        : t("pageList.source.page", { title: sourcePage.title === "" ? t("page.untitled") : sourcePage.title });
    case "tag": {
      const name = tagsDef?.options.find((option) => option.id === source.optionId)?.name;
      return name === undefined ? t("pageList.source.tagGone") : t("pageList.source.tag", { name });
    }
  }
}

const layoutName = (layout: PageListLayout): string => t(layoutDef(layout).labelKey);

/** What a view with no pages at all says, by the kind of its source. */
const EMPTY_KEYS: Readonly<Record<PageListSource["kind"], MessageKey>> = {
  children: "pageList.empty.children",
  tag: "pageList.empty.tag",
  page: "pageList.empty.page",
  space: "pageList.empty.space",
};

/** The list view block of a page. */
export function PageListView({ node, editor, extension, updateAttributes }: ReactNodeViewProps) {
  const store = useStoreApi();
  const blockId = typeof node.attrs.id === "string" ? node.attrs.id : "";
  const views = useMemo(() => parseViews(node.attrs), [node.attrs]);
  const activeViewId = useAppStore((state) => selectActiveViewId(state, blockId));
  const view = resolveActiveView(views, activeViewId);
  const source = useMemo(() => readSource(node.attrs.source), [node.attrs.source]);
  // Everything but the source is the active view's (for a block without `views`, the legacy attributes read as one view).
  const { properties, layout } = view;
  const hiddenGroups = view.hiddenGroups ?? NO_KEYS;
  const pageId = pageIdOf(extension.options);
  // The binding can turn the editor read-only after the view was drawn (content from a newer version).
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const today = useToday();

  const tree = useAppStore((state) => state.tree);
  const schemas = useAppStore((state) => state.propertySchemas);
  const loadPropertySchema = useAppStore((state) => state.loadPropertySchema);
  const createPage = useAppStore((state) => state.createPage);
  const createFromTemplate = useAppStore((state) => state.createFromTemplate);
  const setActiveView = useAppStore((state) => state.setActiveView);
  const setPageProperty = useAppStore((state) => state.setPageProperty);
  const canSaveCsv = useAppStore((state) => state.interopCapabilities.saveTextFile);
  const exportViewCsv = useAppStore((state) => state.exportViewCsv);
  const spaceId = tree?.spaceId ?? null;
  const defs = (spaceId === null ? undefined : schemas[spaceId]) ?? NO_DEFS;
  useEffect(() => {
    if (spaceId !== null) {
      loadPropertySchema(spaceId);
    }
  }, [spaceId, loadPropertySchema]);

  const taggedIds = useTaggedPageIds(source?.kind === "tag" ? source.optionId : null, spaceId, tree, defs);
  const loading = source?.kind === "tag" && taggedIds === null;
  // The source's pages (`page-list-sources.ts`); a linked `page` source whose page is gone, trashed or a template is `missing`.
  const { pages: listedPages, missing } = useMemo(
    () => (source === null ? NO_SOURCE_PAGES : sourcePages(tree, source, pageId, taggedIds)),
    [source, tree, pageId, taggedIds],
  );
  // Where "New" makes a page: under the source page of a linked view, else under the page that holds the block.
  const parentId = source === null ? null : sourceParentId(source, pageId);
  const sourcePage = source?.kind === "page" && tree !== null ? findPage(tree.roots, source.pageId) : null;
  // The rows are the source's pages without templates (and without the pages inside one); the templates feed the menus of "New".
  const { rows: pages, templates } = useMemo(() => splitTemplates(tree, listedPages, pageId), [tree, listedPages, pageId]);
  const group = useMemo(() => resolveGroup(view.groupBy ?? null, defs), [view.groupBy, defs]);
  // The pipeline (ADR-0054; `use-view-rows.ts`, which the dashboard's embedded views run too): pages, the view's filter tree, its sort.
  const { rows, lookups } = useViewRows(pages, view, defs, spaceId);
  const columns = useMemo(
    () => properties.flatMap((id) => defs.find((def) => def.id === id) ?? []),
    [properties, defs],
  );

  // The rows and columns of the CSV: exactly what the layout in use draws (a board's cards column by column).
  const csvSelection = useMemo(
    () => (layout === "board" ? boardSelection(rows, group, hiddenGroups, columns) : listSelection(rows, columns)),
    [layout, rows, group, hiddenGroups, columns],
  );

  // ----- the selection of rows (ADR-0058, `database-power-ups.md` section 5.1) ---------------------------------------------
  // Offered in the list, the table and the board of an editable editor. The rows, in display order (a board: column by column),
  // are what a range and "all" follow; the selection is dropped when the view, the source or the layout changes and pruned of the
  // pages that leave the rows. The copies a Duplicate made are selected as soon as they are among the rows.
  const printing = useAppStore((state) => state.printing);
  const selectable =
    editable && !printing && (layout === "list" || layout === "table" || layout === "board") && source !== null && !missing;
  const displayIds = csvSelection?.pageIds ?? NO_KEYS;
  const displaySet = useMemo(() => new Set(displayIds), [displayIds]);
  const [selection, setSelection] = useState<RowSelection>(EMPTY_SELECTION);
  const [copies, setCopies] = useState<readonly PageId[] | null>(null);
  const selectionScope = `${view.id}|${layout}|${JSON.stringify(node.attrs.source ?? null)}|${String(selectable)}`;
  const [seenScope, setSeenScope] = useState(selectionScope);
  let ticked = selection;
  if (seenScope !== selectionScope) {
    setSeenScope(selectionScope);
    setSelection(EMPTY_SELECTION);
    setCopies(null);
    ticked = EMPTY_SELECTION;
  } else {
    const kept = pruneSelection(selection, displaySet);
    if (kept !== selection) {
      setSelection(kept);
      ticked = kept;
    }
    if (copies !== null && copies.every((id) => displaySet.has(id))) {
      setCopies(null);
      ticked = selectOnly(copies);
      setSelection(ticked);
    }
  }
  const selectedRows = useMemo(() => {
    if (ticked.ids.size === 0) {
      return NO_ROWS;
    }
    const byId = new Map(rows.map((page) => [page.id, page]));
    return selectedInOrder(ticked, displayIds).flatMap((id) => byId.get(id) ?? []);
  }, [ticked, rows, displayIds]);
  const barShown = selectable && selectedRows.length > 0;
  const rowSelection: RowSelectionProps | undefined = selectable
    ? {
        selected: ticked.ids,
        onToggle: (id, { range }) => {
          setCopies(null);
          setSelection((current) => (range ? selectRange(current, displayIds, id) : toggleRow(current, id)));
        },
        onToggleAll: () => {
          setCopies(null);
          setSelection((current) =>
            displayIds.length > 0 && current.ids.size === displayIds.length ? EMPTY_SELECTION : selectAll(displayIds, current.anchor),
          );
        },
      }
    : undefined;

  /** Ctrl+A selects every row of the view, Escape clears the selection, unless the key is for a field being typed in. */
  function onSelectionKey(event: KeyboardEvent<HTMLElement>): void {
    if (!selectable) {
      return;
    }
    const shortcut = selectionShortcut(event, ticked.ids.size > 0);
    if (shortcut === null || (shortcut === "selectAll" && displayIds.length === 0)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    setCopies(null);
    setSelection((current) => (shortcut === "clear" ? EMPTY_SELECTION : selectAll(displayIds, current.anchor)));
  }

  // What an empty view says: filters that hide every page, or that there are no pages.
  const emptyText = pages.length > 0 ? t("pageList.empty.filtered") : t(EMPTY_KEYS[source?.kind ?? "children"]);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The templates of the source and what the menu next to "New" does with them (edit, duplicate, delete, make one).
  const templateActions = useDatabaseTemplateActions({ hostId: pageId, source, onBusy: setBusy, onError: setError });
  const databaseTemplates = useMemo<DatabaseTemplates>(() => ({ templates, actions: templateActions }), [templates, templateActions]);

  /** Saves the rows the view shows as `<page title>.csv`; the person places the file. */
  async function exportCsv(): Promise<void> {
    if (spaceId === null || csvSelection === null) {
      return;
    }
    setError(null);
    try {
      const title = pageId === null ? "" : (lookups.pageTitle?.(pageId) ?? "");
      await exportViewCsv(spaceId, csvSelection.pageIds, csvSelection.columns, csvFileName(title));
    } catch (cause) {
      setError(t("pageList.exportCsv.error", { message: toCoreError(cause).message }));
    }
  }

  const csv: PageListCsvExport | null = canSaveCsv
    ? {
        onExport: () => {
          void exportCsv();
        },
        disabledReason:
          spaceId === null || csvSelection === null || csvSelection.pageIds.length === 0
            ? t("pageList.exportCsv.empty")
            : null,
      }
    : null;

  // ----- writing ----------------------------------------------------------------------------

  /** Writes what a change of the view `id` says (`page-list-write.ts`): one legacy attribute, or `views` and the mirror. */
  function writePatch(id: string, patch: PageListViewPatch): void {
    const attributes = writeForPatch(node.attrs, views, id, patch);
    if (attributes !== null) {
      updateAttributes(attributes);
    }
  }

  /** What the toolbar changes: the block's `source` alone (shared by the views), or a part of the active view. */
  function write(changed: PageListChange): void {
    const { source: changedSource, ...patch } = changed;
    if (changedSource !== undefined) {
      updateAttributes({ source: changedSource });
    }
    if (Object.keys(patch).length > 0) {
      // A new layout may bring what its views start with (the feed's newest-first sort, ADR-0058).
      writePatch(view.id, patch.layout === undefined ? patch : { ...patch, ...layoutChangePatch(view, patch.layout) });
    }
  }

  /** What a layout component changes in the active view. */
  function updateActiveView(patch: PageListViewPatch): void {
    writePatch(view.id, patch);
  }

  /** Writes a new list of views (a view added, copied, removed or moved): `views` and the mirror of the first. */
  function writeList(next: readonly PageListViewSpec[]): void {
    if (!sameViews(next, views)) {
      updateAttributes(writeForViews(next));
    }
  }

  /** The handlers of the tab strip. Choosing a tab is this device's own state: nothing is written to the page. */
  const tabActions = {
    onSelect: (id: string): void => {
      setActiveView(blockId, id);
    },
    onAdd: (layoutToAdd: PageListLayout): void => {
      if (!canAddView(views)) {
        return;
      }
      const id = newViewId(views.map((entry) => entry.id));
      writeList(withInitialPatch(addView(views, layoutToAdd, id), id));
      setActiveView(blockId, id);
    },
    onRename: (id: string, name: string): void => {
      writePatch(id, { name });
    },
    onDuplicate: (id: string): void => {
      const original = views.find((entry) => entry.id === id);
      if (original === undefined || !canAddView(views)) {
        return;
      }
      const copy = newViewId(views.map((entry) => entry.id));
      writeList(duplicateView(views, id, copy, t("pageList.view.copyName", { name: viewLabel(original, layoutName) })));
      setActiveView(blockId, copy);
    },
    onChangeLayout: (id: string, next: PageListLayout): void => {
      const changed = views.find((entry) => entry.id === id);
      writePatch(id, changed === undefined ? { layout: next } : layoutChangePatch(changed, next));
    },
    onMove: (id: string, delta: -1 | 1): void => {
      writeList(moveView(views, id, delta));
    },
    onDelete: (id: string): void => {
      const at = views.findIndex((entry) => entry.id === id);
      const next = removeView(views, id);
      if (sameViews(next, views)) {
        return;
      }
      writeList(next);
      if (id === view.id) {
        // The neighbour takes its place, on this device.
        const neighbour = next[Math.min(at, next.length - 1)];
        if (neighbour !== undefined) {
          setActiveView(blockId, neighbour.id);
        }
      }
    },
  };

  // ----- opening and making pages ------------------------------------------------------------

  /** Opens the page of a row the way the gesture and the view's `openIn` say (`open-row.ts`). */
  function openPage(page: PageInfo, disposition: OpenDisposition): void {
    openRow(store.getState(), { pageId: page.id, spaceId: page.spaceId }, view, disposition, pageId);
  }

  /** Opens the source page of a linked view as a page (or in another tab) the way the gesture on its link says. */
  function openSourcePage(disposition: OpenDisposition): void {
    if (sourcePage !== null) {
      store.getState().navigateTo({ pageId: sourcePage.id, spaceId: sourcePage.spaceId, disposition });
    }
  }

  // What a dashboard (ADR-0055) needs of the block that its props do not carry: the registry's components (it is itself an entry of the
  // registry and cannot import it) and how a page opens from another view of the block (that view's `openIn`).
  const dashboardHost = useMemo<DashboardHost>(
    () => ({
      layoutDef,
      openInView: (target, page, disposition) => {
        openRow(store.getState(), { pageId: page.id, spaceId: page.spaceId }, target, disposition, pageId);
      },
    }),
    [store, pageId],
  );

  /**
   * Makes a page under the page the editor shows (under the source page of a linked `page` source: `sourceParentId`), with the
   * tag of a tag source and the values a layout gives it (a calendar day's date, a board column's option), and opens it like
   * a row (`openRow`, a plain click). The page is made from a template (ADR-0056) when `init.templateId` names one, or when it
   * is absent and the view's `defaultTemplate` is one of the source's templates; `null` (and no default) makes an empty page.
   */
  async function createPageFor(init?: NewPageInit): Promise<void> {
    if (parentId === null || source === null || missing) {
      return;
    }
    const templateId = init?.templateId === undefined ? resolveDefaultTemplate(view, templates) : init.templateId;
    setBusy(true);
    setError(null);
    try {
      let page: PageInfo;
      if (templateId === null) {
        page = await createPage({ parentId, select: false });
        if (source.kind === "tag") {
          await setPageProperty(page.id, TAGS_PROPERTY_ID, { kind: "multi_select", value: [source.optionId] });
        }
        for (const [propertyId, value] of init?.values ?? []) {
          await setPageProperty(page.id, propertyId, value);
        }
      } else {
        // The tag of a tag source and the layout's values are written on the new page, over what the template holds.
        const tag: [PropertyId, PropertyValue][] =
          source.kind === "tag"
            ? [[TAGS_PROPERTY_ID, tagValueFor(templates.find((template) => template.id === templateId), source.optionId)]]
            : [];
        page = await createFromTemplate(templateId, { parentId, values: [...tag, ...(init?.values ?? [])], open: false });
      }
      openRow(store.getState(), { pageId: page.id, spaceId: page.spaceId }, view, "current", pageId);
    } catch (cause) {
      setError(t("pageList.error", { message: toCoreError(cause).message }));
    } finally {
      setBusy(false);
    }
  }

  // The component the registry names for the layout, and its section of the view settings menu.
  const { Component: LayoutComponent, Settings: LayoutSettings } = layoutDef(layout);
  // The strip is always there in an editable editor (its "+" adds a view); a read-only one shows it for several views.
  const tabsShown = editable || views.length > 1;
  // What the list and the table say when they have no row (nothing while a tag source is still being asked).
  const emptyShown = rows.length === 0 && !loading ? emptyText : null;

  return (
    <NodeViewWrapper
      className="page-list"
      role="group"
      aria-label={t("pageList.label")}
      data-layout={layout}
      data-selecting={barShown ? "" : undefined}
      onKeyDownCapture={onSelectionKey}
    >
      {barShown ? (
        <PageListBulkBar
          spaceId={spaceId ?? ""}
          selected={selectedRows}
          schema={defs}
          onClear={() => {
            setCopies(null);
            setSelection(EMPTY_SELECTION);
          }}
          onSelectOnly={(ids) => {
            // The copies are not among the rows yet: they are selected once they are (the originals stay selected until then).
            setCopies(ids);
          }}
        />
      ) : tabsShown && (
        <PageListViewTabs
          views={views}
          activeId={view.id}
          editable={editable}
          {...tabActions}
          trailing={
            editable ? (
              <PageListToolbar
                source={source}
                hostId={pageId}
                view={view}
                defs={defs}
                onChange={write}
                layoutSettings={
                  LayoutSettings === null ? null : <LayoutSettings view={view} schema={defs} onUpdateView={updateActiveView} />
                }
                csv={csv}
              />
            ) : null
          }
        />
      )}
      <div className="page-list-header">
        <span className="page-list-heading">{headingOf(source, defs.find((def) => def.id === TAGS_PROPERTY_ID), sourcePage)}</span>
        {sourcePage !== null && !missing && (
          // The source page of a linked view: a link to it, opened with the gestures of every link to a page (`page-open.ts`).
          <button
            type="button"
            className="page-list-source-link"
            aria-label={t("pageList.source.open", { title: sourcePage.title === "" ? t("page.untitled") : sourcePage.title })}
            {...openGestures<HTMLButtonElement>(openSourcePage)}
          >
            {`↗ ${sourcePage.title === "" ? t("page.untitled") : sourcePage.title}`}
          </button>
        )}
      </div>
      {source === null ? (
        <p className="page-list-empty">{t("pageList.unsupported")}</p>
      ) : missing ? (
        // A linked view whose page is gone, trashed or a template shows no other list, and lets an editor choose pages again.
        <div className="page-list-missing">
          <p className="page-list-empty">{t("pageList.source.missing")}</p>
          {editable && (
            <div className="page-list-missing-chooser">
              <PageListSourceChooser
                source={source}
                hostId={pageId}
                defs={defs}
                onChange={(next) => {
                  write({ source: next });
                }}
              />
            </div>
          )}
        </div>
      ) : (
        <PageListEmptyContext value={emptyShown}>
          <DashboardHostContext value={dashboardHost}>
            <DatabaseTemplatesContext value={databaseTemplates}>
              <LayoutComponent
                rows={rows}
                view={view}
                schema={defs}
                columns={columns}
                spaceId={spaceId ?? ""}
                editable={editable}
                today={today}
                onOpen={openPage}
                onUpdateView={updateActiveView}
                onCreatePage={editable && parentId !== null ? (init) => void createPageFor(init) : null}
                creating={busy}
                selection={rowSelection}
                views={views}
                sourcePages={pages}
                onUpdateOtherView={writePatch}
                onActivateView={tabActions.onSelect}
              />
            </DatabaseTemplatesContext>
          </DashboardHostContext>
        </PageListEmptyContext>
      )}
      {error !== null && (
        <p className="page-list-error" role="alert">
          {error}
        </p>
      )}
    </NodeViewWrapper>
  );
}
