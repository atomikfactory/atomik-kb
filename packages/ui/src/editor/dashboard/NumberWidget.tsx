// The number widget of a dashboard (ADR-0055, `docs/architecture/dashboards.md` section 3): one calculation of the table
// footer (`page-table-calculations.ts`) over a column of the rows the widget's own filter leaves, drawn large and
// centred. The title (the one the widget was given, or "Count all", "Sum · Hours") is in the frame's header. A column whose
// property was deleted, and a calculation with nothing to say (the average of no numbers), show a dash.

import type { PageInfo, PropertyDef } from "@atomik-kb/core-client";

import { calculate, formatCalculation } from "../page-table-calculations";
import type { CalculationFunction } from "../page-table-calculations";

/** What a number widget shows when it has no value. */
export const NO_NUMBER = "—";

/** The text a calculation makes of `rows`: `null` when the column is gone or the function has nothing to say. */
export function numberText(
  calc: { readonly by: string; readonly fn: CalculationFunction },
  schema: readonly PropertyDef[],
  rows: readonly PageInfo[],
  today: string,
): string | null {
  const def = calc.by === "title" ? null : (schema.find((candidate) => candidate.id === calc.by) ?? undefined);
  if (def === undefined) {
    return null;
  }
  const result = calculate(calc.fn, def, rows);
  return result === undefined ? null : formatCalculation(calc.fn, result, today);
}

export interface NumberWidgetProps {
  readonly calc: { readonly by: string; readonly fn: CalculationFunction };
  readonly schema: readonly PropertyDef[];
  /** The pages after the dashboard's and the widget's filters. */
  readonly rows: readonly PageInfo[];
  readonly today: string;
}

/** The value of a number widget. */
export function NumberWidget({ calc, schema, rows, today }: NumberWidgetProps) {
  const text = numberText(calc, schema, rows, today);
  return (
    <div className="page-dashboard-number">
      <p className="page-dashboard-number-value" data-empty={text === null ? "" : undefined}>
        {text ?? NO_NUMBER}
      </p>
    </div>
  );
}
