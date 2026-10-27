// The footer row of the table layout (ADR-0040 item 5, ADR-0016 amendment of 2026-10-14): one cell under
// each column, in the grid so that it has the columns' widths and scrolls sideways with them. A cell shows
// the function chosen for its column and what it makes of the rows the view shows; an editable editor lets
// the person choose (a "Calculate" button that shows on hover and focus while the cell is empty, and the
// current function afterwards, which opens the menu of the functions the column's kind offers and "None").
// A read-only editor shows the values only. The footer writes nothing itself: a choice is `onChoose`.

import type { PageInfo, PropertyDef } from "@atomik-kb/core-client";
import { useMemo, useState } from "react";

import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { Menu } from "../components/Menu";
import type { MenuItem } from "../components/Menu";
import { t } from "../i18n";
import { calculate, CALCULATION_LABELS, calculationFunctionsFor, formatCalculation, isCalculationFunction } from "./page-table-calculations";
import type { CalculationFunction } from "./page-table-calculations";

const NONE = "none";

export interface FooterCellProps {
  /** The property of the column; `null` for the page (title) column. */
  def: PropertyDef | null;
  colIndex: number;
  /** The rows the view shows (after the filters), which the function is applied to. */
  rows: readonly PageInfo[];
  /** The local day, for "Today" and "Tomorrow" in dates. */
  today: string;
  /** The function chosen for the column (one its kind offers), `null` for none. */
  fn: CalculationFunction | null;
  editable: boolean;
  /** The person chose a function, or `null` for "None". */
  onChoose: (fn: CalculationFunction | null) => void;
}

/** What the function made of the rows, as text; the dash of a result that is not there (the average of no numbers). */
function resultText(fn: CalculationFunction, def: PropertyDef | null, rows: readonly PageInfo[], today: string): string {
  const result = calculate(fn, def, rows);
  return result === undefined ? t("pageTable.calc.noResult") : formatCalculation(fn, result, today);
}

/** The footer cell of one column. */
export function FooterCell({ def, colIndex, rows, today, fn, editable, onChoose }: FooterCellProps) {
  const [menu, setMenu] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const name = def === null ? t("pageList.sort.title") : def.name;
  const value = useMemo(() => (fn === null ? null : resultText(fn, def, rows, today)), [fn, def, rows, today]);
  const label = fn === null ? null : t(CALCULATION_LABELS[fn]);

  const shown =
    label === null || value === null ? null : (
      <>
        <span className="page-table-calc-label">{label}</span>
        <span className="page-table-calc-value">{value}</span>
      </>
    );

  const items: MenuItem[] = [
    { id: NONE, label: t("pageTable.calc.none") },
    ...calculationFunctionsFor(def).map((candidate) => ({ id: candidate, label: t(CALCULATION_LABELS[candidate]) })),
  ];

  return (
    <div
      role="gridcell"
      className="page-table-footer-cell"
      aria-label={t("pageTable.calc.cell", { name })}
      aria-colindex={colIndex}
      data-kind={def === null ? "title" : def.kind}
    >
      {editable ? (
        <button
          type="button"
          className="page-table-calc-button"
          aria-haspopup="menu"
          aria-expanded={menu !== null}
          data-empty={shown === null ? "" : undefined}
          title={shown === null ? undefined : `${label ?? ""}: ${value ?? ""}`}
          onClick={(event) => {
            const trigger = event.currentTarget;
            setMenu((current) => (current === null ? { anchor: anchorBelow(trigger), trigger } : null));
          }}
        >
          {shown ?? <span className="page-table-calc-prompt">{t("pageTable.calc.calculate")}</span>}
        </button>
      ) : (
        shown !== null && (
          <span className="page-table-calc-button" title={`${label ?? ""}: ${value ?? ""}`}>
            {shown}
          </span>
        )
      )}
      {menu !== null && (
        <Menu
          label={t("pageTable.calc.menu", { name })}
          items={items}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(id) => {
            onChoose(isCalculationFunction(id) ? id : null);
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </div>
  );
}
