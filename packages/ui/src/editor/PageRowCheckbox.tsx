// The checkbox that selects a row of a database block (ADR-0058, `docs/architecture/database-power-ups.md` section 5.1): one
// element for the list, the table and the board. It is a native checkbox, so Space ticks it and a screen reader announces its state.
// A click ticks the row and never opens it; Shift+click asks for a range from the last tick (`range`). Hiding it until the pointer
// or focus is on the row, or until something is selected, is the stylesheet's (`database-bulk.css`).

import { useEffect, useRef } from "react";
import type { ChangeEvent, MouseEvent } from "react";

import { t } from "../i18n";

/** Width of the table's checkbox column, in pixels. */
export const SELECT_COLUMN_WIDTH = 32;

export interface PageRowCheckboxProps {
  /** What the checkbox is for: the page's title (an empty title reads "Untitled" at the caller), or "Select all" for a header. */
  label: string;
  checked: boolean;
  /** Some, not all, of what the checkbox stands for is selected (the table's "Select all"). */
  mixed?: boolean;
  /** 0 where Tab reaches it, -1 where the row's other controls are the way in (the table's other rows). */
  tabIndex?: 0 | -1;
  /** Ticked or unticked; `range` is true for a Shift+click. */
  onToggle: (range: boolean) => void;
}

/** Whether the click that changed the checkbox had Shift held. */
function shiftOf(event: ChangeEvent<HTMLInputElement>): boolean {
  const native: unknown = event.nativeEvent;
  return typeof native === "object" && native !== null && "shiftKey" in native && native.shiftKey === true;
}

/** The checkbox of a row, or of a header (`mixed`). */
export function PageRowCheckbox({ label, checked, mixed = false, tabIndex, onToggle }: PageRowCheckboxProps) {
  const box = useRef<HTMLInputElement>(null);
  // `indeterminate` is a property of the element, not an attribute.
  useEffect(() => {
    if (box.current !== null) {
      box.current.indeterminate = mixed;
    }
  }, [mixed]);
  return (
    <input
      ref={box}
      type="checkbox"
      className="page-row-checkbox"
      data-row-select=""
      checked={checked}
      aria-label={label}
      tabIndex={tabIndex}
      onClick={(event: MouseEvent<HTMLInputElement>) => {
        // The click is for the selection, not for the row, the cell or the card around it.
        event.stopPropagation();
      }}
      onChange={(event) => {
        onToggle(shiftOf(event));
      }}
    />
  );
}

/** The accessible name of the checkbox of a row. */
export function selectLabel(title: string): string {
  return t("pageList.bulk.select", { title: title === "" ? t("page.untitled") : title });
}
