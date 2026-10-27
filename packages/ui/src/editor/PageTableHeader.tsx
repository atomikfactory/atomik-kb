// The header cell of a column of the table (ADR-0016, amendment of 2026-10-08): the name with the
// glyph of its kind, which opens a menu (sort ascending or descending, hide, move left or right) in
// an editable editor, and the handle on its right border that resizes it. The width follows the
// pointer while it is dragged and is written once, when the drag ends (`onResizeEnd`); the arrow
// keys on the handle resize by a step and write each time. The menu of a formula column also has "Edit formula…"
// (ADR-0055).

import type { PropertyDef } from "@atomik-kb/core-client";
import { TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import { useState } from "react";
import type { KeyboardEvent, PointerEvent as ReactPointerEvent } from "react";

import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { Menu } from "../components/Menu";
import type { MenuItem } from "../components/Menu";
import { KindGlyph, TagGlyph } from "../components/property-icons";
import { t } from "../i18n";
import { clampColumnWidth, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH } from "./page-list-model";
import type { SortDirection } from "./page-list-model";
import { dragWidth } from "./page-table-model";

/** How far an arrow key resizes a column, in pixels. */
const KEY_STEP = 16;

export type HeaderCommand = "asc" | "desc" | "hide" | "left" | "right" | "editFormula";

/**
 * The width a column is drawn with now, in whole pixels, from the header cell `element` is in (the page column
 * fills a wide block, so it can be wider than the width it keeps); `null` where there is no layout.
 */
export function drawnWidth(element: Element | null): number | null {
  const width = Math.round(element?.closest('[role="columnheader"]')?.getBoundingClientRect().width ?? 0);
  return width > 0 ? width : null;
}

export interface HeaderCellProps {
  /** The property of the column; `null` for the page (title) column. */
  def: PropertyDef | null;
  colIndex: number;
  /** The width as drawn (while dragging, the dragged width). */
  width: number;
  /** The direction the table is sorted in by this column as its first sort key, `null` when it is not (ADR-0058: a view may have several). */
  sorted: SortDirection | null;
  editable: boolean;
  canMoveLeft: boolean;
  canMoveRight: boolean;
  /** A menu command was chosen; `trigger` is the header button that opened the menu (where focus returns after a dialog). */
  onCommand: (command: HeaderCommand, trigger: Element) => void;
  /** The handle was pressed: the width the column is drawn with. */
  onResizeStart: (drawn: number) => void;
  /** The handle is being dragged: the width it would have. */
  onResize: (width: number) => void;
  /** The drag ended (`width`) or was cancelled (`null`). */
  onResizeEnd: (width: number | null) => void;
}

/** The header cell of one column. */
export function HeaderCell({
  def,
  colIndex,
  width,
  sorted,
  editable,
  canMoveLeft,
  canMoveRight,
  onCommand,
  onResizeStart,
  onResize,
  onResizeEnd,
}: HeaderCellProps) {
  const [menu, setMenu] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const name = def === null ? t("pageList.sort.title") : def.name;
  const glyph =
    def === null ? null : (
      <span className="property-glyph" aria-hidden="true">
        {def.id === TAGS_PROPERTY_ID ? <TagGlyph /> : <KindGlyph kind={def.kind} />}
      </span>
    );
  const label = (
    <>
      {glyph}
      <span className="page-table-header-name">{name}</span>
      {sorted !== null && (
        <span className="page-table-sort" aria-hidden="true">
          {sorted === "asc" ? "↑" : "↓"}
        </span>
      )}
    </>
  );

  const items: MenuItem[] = [
    { id: "asc", label: t("pageTable.sortAsc") },
    { id: "desc", label: t("pageTable.sortDesc") },
  ];
  if (def !== null) {
    items.push(
      { id: "hide", label: t("pageTable.hide") },
      { id: "left", label: t("pageTable.moveLeft"), disabled: !canMoveLeft },
      { id: "right", label: t("pageTable.moveRight"), disabled: !canMoveRight },
    );
    if (def.kind === "formula") {
      items.push({ id: "editFormula", label: t("formula.menu.edit") });
    }
  }

  function startResize(event: ReactPointerEvent<HTMLDivElement>): void {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const startX = event.clientX;
    // From the width drawn, not the stored one: the page column fills a wide block, and the edge must not jump.
    const start = drawnWidth(event.currentTarget) ?? width;
    let latest = start;
    onResizeStart(start);
    const move = (moved: PointerEvent): void => {
      latest = dragWidth(start + moved.clientX - startX, start);
      onResize(latest);
    };
    const stop = (): void => {
      document.removeEventListener("pointermove", move);
      document.removeEventListener("pointerup", up);
      document.removeEventListener("pointercancel", cancel);
    };
    const up = (): void => {
      stop();
      onResizeEnd(latest === start ? width : latest); // a drag that ends where it began changes nothing
    };
    const cancel = (): void => {
      stop();
      onResizeEnd(null);
    };
    document.addEventListener("pointermove", move);
    document.addEventListener("pointerup", up);
    document.addEventListener("pointercancel", cancel);
  }

  function resizeByKey(event: KeyboardEvent<HTMLDivElement>): void {
    const delta = event.key === "ArrowRight" ? KEY_STEP : event.key === "ArrowLeft" ? -KEY_STEP : 0;
    if (delta !== 0) {
      event.preventDefault();
      const next = clampColumnWidth((drawnWidth(event.currentTarget) ?? width) + delta);
      if (next !== width) {
        onResizeEnd(next);
      }
    }
  }

  return (
    <div
      role="columnheader"
      className="page-table-header-cell"
      aria-label={name}
      aria-colindex={colIndex}
      aria-sort={sorted === null ? undefined : sorted === "asc" ? "ascending" : "descending"}
      data-width={width}
    >
      {editable ? (
        <button
          type="button"
          className="page-table-header-button"
          aria-haspopup="menu"
          aria-expanded={menu !== null}
          onClick={(event) => {
            const trigger = event.currentTarget;
            setMenu((current) => (current === null ? { anchor: anchorBelow(trigger), trigger } : null));
          }}
        >
          {label}
        </button>
      ) : (
        <span className="page-table-header-button">{label}</span>
      )}
      {editable && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label={t("pageTable.resize", { name })}
          aria-valuemin={MIN_COLUMN_WIDTH}
          aria-valuemax={MAX_COLUMN_WIDTH}
          aria-valuenow={width}
          tabIndex={0}
          className="page-table-resize"
          onPointerDown={startResize}
          onKeyDown={resizeByKey}
        />
      )}
      {menu !== null && (
        <Menu
          label={t("pageTable.menu", { name })}
          items={items}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(command) => {
            onCommand(command as HeaderCommand, menu.trigger);
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </div>
  );
}
