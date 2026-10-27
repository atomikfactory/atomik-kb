// The row limit of the list, the table and the board (ADR-0058, `docs/architecture/database-power-ups.md` section 5.3): a view
// over a whole space can list thousands of pages, so these layouts draw at most 200 rows (the board 200 cards per column) and
// a "Show 200 more" button raises the limit for that block until its view changes. The limit is about drawing only: the
// calculations, the counts, the dashboard and the CSV export use every row, and print mode draws every row too.

import { useState } from "react";

import { useAppStore } from "../store-context";

/** How many rows are drawn at first, and how many more each "Show more" adds. */
export const ROW_LIMIT_STEP = 200;

const NO_STEPS: Readonly<Record<string, number>> = {};

/** The first `limit` of `rows` (all of them for an infinite limit). */
export function limitRows<T>(rows: readonly T[], limit: number): readonly T[] {
  return rows.length <= limit ? rows : rows.slice(0, limit);
}

/** The limit after `steps` presses of "Show more". */
export function limitAfter(steps: number): number {
  return ROW_LIMIT_STEP * (1 + Math.max(0, steps));
}

export interface RowLimit {
  /** How many rows of `part` (a board column's key; the whole list or table when left out) are drawn. */
  limitOf: (part?: string) => number;
  /** Raises the limit of `part` by one step. */
  showMore: (part?: string) => void;
}

/**
 * The limits of one layout: each part (the list and the table are one, a board has one per column) starts at 200 and goes up
 * by 200 per press of "Show more". They all start again when `resetKey` changes (the id of the active view). Printing lifts
 * the limit, so a printed page holds every row.
 */
export function useRowLimit(resetKey: string): RowLimit {
  const printing = useAppStore((state) => state.printing);
  const [raised, setRaised] = useState<{ key: string; steps: Readonly<Record<string, number>> }>({ key: resetKey, steps: NO_STEPS });
  const steps = raised.key === resetKey ? raised.steps : NO_STEPS;
  return {
    limitOf: (part = "") => (printing ? Number.POSITIVE_INFINITY : limitAfter(steps[part] ?? 0)),
    showMore: (part = "") => {
      setRaised((current) => {
        const before = current.key === resetKey ? current.steps : NO_STEPS;
        return { key: resetKey, steps: { ...before, [part]: (before[part] ?? 0) + 1 } };
      });
    },
  };
}
