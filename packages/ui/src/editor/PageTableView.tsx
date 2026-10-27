// The table layout of the list view block (ADR-0016, amendment of 2026-10-08): a grid with the page
// as its first column (icon, title edited in place, a button that opens the page) and one column
// per property the block shows, a header row (name, kind glyph and a menu to sort, hide and move the
// column, a handle to resize it) and a "New page" row. The cells edit the values with the editors of
// the properties panel. A table of many rows draws only the ones in view (rows have a fixed height).
// The pages, their order and the filters are the view's (`PageListView.tsx`); this layout takes the props every
// layout takes (`page-list-layout-types.ts`, ADR-0054): it asks to open a page through `onOpen` and writes the
// columns, the sort, the widths and the footer through `onUpdateView`, never the block's attributes. It keeps only
// what is on screen: which cell is active, whether it is edited, the scroll position and the width
// of a column while it is dragged. Keys: the arrows move the active cell, Enter (or F2) edits it,
// Escape leaves it. A footer row (`PageTableFooter.tsx`, ADR-0040 item 5) shows a calculation under each
// column; it sits in the grid after the windowed body, so it is always drawn and keeps the columns' widths. In an editable editor a
// narrow first column holds the checkbox of the selection (a tri-state "Select all" in the header, `PageRowCheckbox`, ADR-0058) and
// at most 200 rows are drawn until "Show more" (`page-list-row-limit.ts`).

import { toCoreError } from "@atomik-kb/core-client";
import type { PageInfo, PropertyDef, PropertyOption } from "@atomik-kb/core-client";
import { useContext, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";

import { FormulaEditorHost } from "../components/FormulaEditor";
import { PlusIcon } from "../components/icons";
import { DeleteOptionConfirm } from "../components/PropertyConfirms";
import { t } from "../i18n";
import { dispositionFromKey } from "../page-open";
import { useAppStore } from "../store-context";
import { NewPageMenu } from "./NewPageMenu";
import { PageListEmptyContext } from "./page-list-empty";
import type { PageListLayoutProps } from "./page-list-layout-types";
import { TITLE_COLUMN } from "./page-list-model";
import { clampColumnWidth } from "./page-list-model";
import { limitRows, ROW_LIMIT_STEP, useRowLimit } from "./page-list-row-limit";
import type { ColumnDrag } from "./page-table-model";
import {
  gridLayout,
  isEditableKind,
  isWindowed,
  moveProperty,
  ROW_HEIGHT,
  scrollToReveal,
  visibleRows,
  WINDOW_HEIGHT,
  widthOf,
} from "./page-table-model";
import { drawnWidth, HeaderCell } from "./PageTableHeader";
import type { HeaderCommand } from "./PageTableHeader";
import { PropertyCell, TitleCell } from "./PageTableCell";
import type { CellActions, CellState } from "./PageTableCell";
import { calculationFor } from "./page-table-calculations";
import type { CalculationFunction } from "./page-table-calculations";
import { FooterCell } from "./PageTableFooter";
import { PageRowCheckbox, SELECT_COLUMN_WIDTH, selectLabel } from "./PageRowCheckbox";

const NO_WIDTHS: Readonly<Record<string, number>> = {};
const NO_CALCULATIONS: Readonly<Record<string, string>> = {};

interface ActiveCell {
  pageId: string;
  column: string;
}

/** The table: the layout of a view whose `layout` is `table`. */
export function PageTableView({
  rows,
  view,
  columns,
  spaceId,
  editable,
  today,
  onOpen,
  onUpdateView,
  onCreatePage,
  creating,
  selection,
}: PageListLayoutProps) {
  // What the view stores: the property ids (a moved column is written back into this list), the sort keys, the widths of the
  // columns and the footer's functions by column key (as stored: some may not apply).
  const { properties, sort } = view;
  const widths = view.columnWidths ?? NO_WIDTHS;
  const calculations = view.calculations ?? NO_CALCULATIONS;
  const emptyText = useContext(PageListEmptyContext);
  const renamePage = useAppStore((state) => state.renamePage);
  // Print mode (print.ts) draws every row, not only the ones in view.
  const printing = useAppStore((state) => state.printing);
  const [active, setActive] = useState<ActiveCell | null>(null);
  // The cell being edited (always the active one). Ending it names the cell, so that a cell whose editor lost focus to a click on another cell does not end the edit that click began.
  const [editing, setEditing] = useState<ActiveCell | null>(null);
  const [focusTick, setFocusTick] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [drag, setDrag] = useState<ColumnDrag | null>(null);
  const [deleting, setDeleting] = useState<{ def: PropertyDef; option: PropertyOption } | null>(null);
  // The formula column being edited: the editor previews on the first row of the table (the page in front holds the table).
  const [editingFormula, setEditingFormula] = useState<{ def: PropertyDef; returnTo: Element; pages: readonly PageInfo[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  // At most 200 rows are drawn, "Show 200 more" adds 200 (ADR-0058); the footer's calculations use every row. The checkbox column
  // (the selection, `database-power-ups.md` section 5.1) comes before the page column and shifts the column indexes by one.
  const limit = useRowLimit(view.id);
  const shownRows = limitRows(rows, limit.limitOf());
  const lead = selection === undefined ? 0 : 1;
  const ticked = selection === undefined ? 0 : rows.reduce((count, page) => count + (selection.selected.has(page.id) ? 1 : 0), 0);

  const keys = [TITLE_COLUMN, ...columns.map((def) => def.id)];
  const widthFor = (key: string): number => (drag?.column === key ? drag.width : widthOf(widths, key));
  const { template, minWidth } = gridLayout(keys, widths, drag);
  const shownIds = columns.map((def) => def.id);
  // The footer's function of each column (`null` for none); the footer is there to choose them, or to show the ones set.
  const footerFunctions = keys.map((key, at) => calculationFor(calculations, key, at === 0 ? null : (columns[at - 1] ?? null)));
  const showFooter = editable || footerFunctions.some((fn) => fn !== null);

  const windowed = !printing && isWindowed(shownRows.length);
  const { start, end } = windowed ? visibleRows(shownRows.length, scrollTop) : { start: 0, end: shownRows.length };
  const drawn = shownRows.slice(start, end);

  // The cell Tab reaches: the active one, or the first of the table while none is.
  const activeRow = active === null ? -1 : shownRows.findIndex((page) => page.id === active.pageId);
  const activeColumn = active === null ? -1 : keys.indexOf(active.column);
  const hasActive = activeRow >= 0 && activeColumn >= 0;
  const tabbableRow = hasActive ? activeRow : 0;
  const tabbableColumn = hasActive ? activeColumn : 0;

  function goTo(row: number, column: number): void {
    const page = shownRows[Math.min(shownRows.length - 1, Math.max(0, row))];
    const key = keys[Math.min(keys.length - 1, Math.max(0, column))];
    if (page === undefined || key === undefined) {
      return;
    }
    if (windowed) {
      const next = scrollToReveal(Math.min(shownRows.length - 1, Math.max(0, row)), scrollTop, showFooter ? ROW_HEIGHT : 0);
      if (next !== scrollTop && scroller.current !== null) {
        scroller.current.scrollTop = next;
        setScrollTop(next);
      }
    }
    setActive({ pageId: page.id, column: key });
    setEditing(null);
    setFocusTick((tick) => tick + 1);
  }

  function actionsFor(page: PageInfo, key: string, row: number, column: number, def: PropertyDef | null): CellActions {
    const cell = { pageId: page.id, column: key };
    return {
      onSelect: () => {
        setActive(cell);
        setEditing(null);
      },
      onEdit: () => {
        setActive(cell);
        setEditing(cell);
      },
      onStop: (refocus) => {
        setEditing((current) => (current?.pageId === cell.pageId && current.column === cell.column ? null : current));
        if (refocus) {
          setFocusTick((tick) => tick + 1);
        }
      },
      onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
        switch (event.key) {
          case "ArrowDown":
            event.preventDefault();
            goTo(row + 1, column);
            break;
          case "ArrowUp":
            event.preventDefault();
            goTo(row - 1, column);
            break;
          case "ArrowRight":
            event.preventDefault();
            goTo(row, column + 1);
            break;
          case "ArrowLeft":
            event.preventDefault();
            goTo(row, column - 1);
            break;
          case "Home":
            event.preventDefault();
            goTo(row, 0);
            break;
          case "End":
            event.preventDefault();
            goTo(row, keys.length - 1);
            break;
          case "Escape":
            event.preventDefault();
            setActive(null);
            break;
          case "Enter":
          case "F2":
            event.preventDefault();
            if (def === null) {
              if (editable) {
                setActive(cell);
                setEditing(cell);
              } else {
                onOpen(page, dispositionFromKey(event));
              }
            } else if (editable && isEditableKind(def.kind)) {
              if (def.kind === "checkbox") {
                event.currentTarget.querySelector<HTMLInputElement>('input[type="checkbox"]')?.click();
              } else {
                setActive(cell);
                setEditing(cell);
              }
            }
            break;
          case " ":
            if (def?.kind === "checkbox" && editable) {
              event.preventDefault();
              event.currentTarget.querySelector<HTMLInputElement>('input[type="checkbox"]')?.click();
            }
            break;
        }
      },
    };
  }

  function stateOf(page: PageInfo, key: string, row: number, column: number): CellState {
    const isActive = active?.pageId === page.id && active.column === key;
    return {
      active: isActive,
      editing: isActive && editing?.pageId === page.id && editing.column === key,
      tabbable: row === tabbableRow && column === tabbableColumn,
      focusTick,
    };
  }

  function command(def: PropertyDef | null, action: HeaderCommand, trigger: Element): void {
    const by = def === null ? TITLE_COLUMN : def.id;
    if (action === "editFormula") {
      if (def !== null) {
        setEditingFormula({ def, returnTo: trigger, pages: rows.slice(0, 1) });
      }
    } else if (action === "asc" || action === "desc") {
      // The header's sort is the view's only sort key (the Sort panel keeps several); the header marks the first one.
      onUpdateView({ sorts: [{ by, dir: action }] });
    } else if (def !== null && action === "hide") {
      onUpdateView({ properties: properties.filter((id) => id !== def.id) });
    } else if (def !== null) {
      const next = moveProperty(properties, shownIds, def.id, action === "left" ? -1 : 1);
      if (next !== null) {
        onUpdateView({ properties: next });
      }
    }
  }

  /** Writes the footer's function of a column (`null`: none) as the whole `calculations` map, one change of the view. */
  function calculateColumn(key: string, fn: CalculationFunction | null): void {
    const next = { ...calculations };
    if (fn === null) {
      if (!Object.hasOwn(next, key)) {
        return;
      }
      delete next[key];
    } else if (next[key] === fn) {
      return;
    } else {
      next[key] = fn;
    }
    onUpdateView({ calculations: Object.keys(next).length === 0 ? null : next });
  }

  async function rename(pageId: string, title: string): Promise<void> {
    setError(null);
    try {
      await renamePage(pageId, title);
    } catch (cause) {
      setError(t("pageTable.error.rename", { message: toCoreError(cause).message }));
    }
  }

  /** The width the page column is drawn with, `null` where there is no layout. */
  function titleWidth(): number | null {
    return drawnWidth(scroller.current?.querySelector(`[role="columnheader"][aria-colindex="${String(1 + lead)}"]`) ?? null);
  }

  function header(key: string, def: PropertyDef | null, index: number) {
    const at = def === null ? -1 : shownIds.indexOf(def.id);
    return (
      <HeaderCell
        key={key}
        def={def}
        colIndex={index + 1 + lead}
        width={widthFor(key)}
        sorted={sort.by === key ? sort.dir : null}
        editable={editable}
        canMoveLeft={at > 0}
        canMoveRight={at >= 0 && at < shownIds.length - 1}
        onCommand={(action, trigger) => {
          command(def, action, trigger);
        }}
        onResizeStart={(drawn) => {
          // The page column is frozen at the width it is drawn with for the length of the drag.
          setDrag({ column: key, width: drawn, title: key === TITLE_COLUMN ? drawn : titleWidth() });
        }}
        onResize={(width) => {
          setDrag((current) => ({ column: key, width, title: current?.title ?? null }));
        }}
        onResizeEnd={(width) => {
          setDrag(null);
          const kept = width === null ? null : clampColumnWidth(width);
          if (kept !== null && kept !== widthOf(widths, key)) {
            onUpdateView({ columnWidths: { ...widths, [key]: kept } });
          }
        }}
      />
    );
  }

  return (
    <div className="page-table">
      <div
        ref={scroller}
        className="page-table-scroll"
        data-windowed={windowed ? "" : undefined}
        style={windowed ? { maxHeight: WINDOW_HEIGHT } : undefined}
        onScroll={(event) => {
          if (windowed) {
            setScrollTop(event.currentTarget.scrollTop);
          }
        }}
      >
        <div
          role="grid"
          className="page-table-grid"
          aria-label={t("pageTable.grid")}
          aria-rowcount={rows.length + (showFooter ? 2 : 1)}
          aria-colcount={keys.length + lead}
          style={
            {
              "--page-table-columns": selection === undefined ? template : `${String(SELECT_COLUMN_WIDTH)}px ${template}`,
              minWidth: selection === undefined ? minWidth : minWidth + SELECT_COLUMN_WIDTH,
            } as CSSProperties
          }
        >
          <div role="rowgroup" className="page-table-head">
            <div role="row" className="page-table-row page-table-header-row" aria-rowindex={1}>
              {selection !== undefined && (
                <div role="columnheader" className="page-table-header-cell page-table-select-cell" aria-colindex={1} aria-label={t("pageList.bulk.selectAll")}>
                  <PageRowCheckbox
                    label={t("pageList.bulk.selectAll")}
                    checked={rows.length > 0 && ticked === rows.length}
                    mixed={ticked > 0 && ticked < rows.length}
                    onToggle={() => {
                      selection.onToggleAll();
                    }}
                  />
                </div>
              )}
              {header(TITLE_COLUMN, null, 0)}
              {columns.map((def, index) => header(def.id, def, index + 1))}
            </div>
          </div>
          <div
            role="rowgroup"
            className="page-table-body"
            style={windowed ? { paddingTop: start * ROW_HEIGHT, paddingBottom: (shownRows.length - end) * ROW_HEIGHT } : undefined}
          >
            {drawn.map((page, offset) => {
              const row = start + offset;
              return (
                <div
                  key={page.id}
                  role="row"
                  className="page-table-row"
                  aria-rowindex={row + 2}
                  data-page={page.id}
                  data-selected={selection?.selected.has(page.id) === true ? "" : undefined}
                >
                  {selection !== undefined && (
                    <div role="gridcell" className="page-table-select-cell" aria-colindex={1}>
                      <PageRowCheckbox
                        label={selectLabel(page.title)}
                        checked={selection.selected.has(page.id)}
                        tabIndex={row === tabbableRow ? 0 : -1}
                        onToggle={(range) => {
                          selection.onToggle(page.id, { range });
                        }}
                      />
                    </div>
                  )}
                  <TitleCell
                    page={page}
                    colIndex={1 + lead}
                    editable={editable}
                    state={stateOf(page, TITLE_COLUMN, row, 0)}
                    actions={actionsFor(page, TITLE_COLUMN, row, 0, null)}
                    onOpen={(disposition) => {
                      onOpen(page, disposition);
                    }}
                    onRename={(title) => rename(page.id, title)}
                  />
                  {columns.map((def, index) => (
                    <PropertyCell
                      key={def.id}
                      page={page}
                      def={def}
                      spaceId={spaceId}
                      colIndex={index + 2 + lead}
                      editable={editable}
                      today={today}
                      state={stateOf(page, def.id, row, index + 1)}
                      actions={actionsFor(page, def.id, row, index + 1, def)}
                      onDeleteOption={(owner, option) => {
                        setDeleting({ def: owner, option });
                      }}
                      onSaveError={setError}
                    />
                  ))}
                </div>
              );
            })}
          </div>
          {showFooter && (
            <div role="rowgroup" className="page-table-foot" data-empty={footerFunctions.every((fn) => fn === null) ? "" : undefined}>
              <div role="row" className="page-table-row page-table-footer-row" aria-rowindex={rows.length + 2}>
                {selection !== undefined && <div role="gridcell" className="page-table-select-cell" aria-colindex={1} aria-hidden="true" />}
                {keys.map((key, at) => {
                  const def = at === 0 ? null : (columns[at - 1] ?? null);
                  return (
                    <FooterCell
                      key={key}
                      def={def}
                      colIndex={at + 1 + lead}
                      rows={rows}
                      today={today}
                      fn={footerFunctions[at] ?? null}
                      editable={editable}
                      onChoose={(fn) => {
                        calculateColumn(key, fn);
                      }}
                    />
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>
      {shownRows.length < rows.length && (
        <div className="page-list-more">
          <span className="page-list-more-count">{t("pageList.rows.shown", { shown: shownRows.length, total: rows.length })}</span>
          <button
            type="button"
            className="page-list-more-button"
            onClick={() => {
              limit.showMore();
            }}
          >
            {t("pageList.rows.showMore", { count: Math.min(ROW_LIMIT_STEP, rows.length - shownRows.length) })}
          </button>
        </div>
      )}
      {onCreatePage !== null && (
        <div className="database-new-group" data-wide="">
          <button
            type="button"
            className="page-table-new"
            disabled={creating}
            onClick={() => {
              onCreatePage();
            }}
          >
            <PlusIcon />
            <span>{t("pageList.newPage")}</span>
          </button>
          <NewPageMenu view={view} onCreatePage={onCreatePage} onUpdateView={onUpdateView} disabled={creating} />
        </div>
      )}
      {rows.length === 0 && emptyText !== null && <p className="page-list-empty">{emptyText}</p>}
      {error !== null && (
        <p className="page-list-error" role="alert">
          {error}
        </p>
      )}
      {deleting !== null && (
        <DeleteOptionConfirm
          spaceId={spaceId}
          def={deleting.def}
          option={deleting.option}
          onError={setError}
          onClose={() => {
            setDeleting(null);
          }}
        />
      )}
      {editingFormula !== null && (
        <FormulaEditorHost
          spaceId={spaceId}
          def={editingFormula.def}
          previewPages={editingFormula.pages}
          returnFocus={editingFormula.returnTo}
          onSaved={() => {
            setEditingFormula(null);
          }}
          onClose={() => {
            setEditingFormula(null);
          }}
        />
      )}
    </div>
  );
}
