// The rows of a view (`use-view-rows.ts`, ADR-0054 and ADR-0058): the pages after the dashboard's filter and the view's own,
// ordered by the view's sort keys, with the viewer's own day for the relative days and periods of the filters, so that a view
// "of today" is worked out again when midnight passes. The clock is faked: the day is Wednesday 2026-10-14, one minute to midnight.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PageInfo, PropertyDef, PropertyValue } from "@atomik-kb/core-client";
import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { EMPTY_FILTER_GROUP } from "./page-list-filter-tree";
import type { FilterGroup } from "./page-list-filter-tree";
import type { PageListSort } from "./page-list-model";
import { useViewRows } from "./use-view-rows";

const DUE = "p-due";
const PRIORITY = "p-priority";
const DEFS: readonly PropertyDef[] = [
  { id: DUE, name: "Due", kind: "date", options: [], createdAt: 0 },
  { id: PRIORITY, name: "Priority", kind: "number", options: [], createdAt: 0 },
];

function page(title: string, day: string | null, priority: number | null): PageInfo {
  const properties: [string, PropertyValue][] = [];
  if (day !== null) {
    properties.push([DUE, { kind: "date", value: { date: day } }]);
  }
  if (priority !== null) {
    properties.push([PRIORITY, { kind: "number", value: priority }]);
  }
  return { id: title, spaceId: "s", title, createdAt: 0, updatedAt: 0, properties };
}

const PAGES: readonly PageInfo[] = [
  page("Alpha", "2026-10-14", 2),
  page("Beta", "2026-10-14", 1),
  page("Gamma", "2026-10-15", 1),
  page("Delta", "2026-10-14", null),
  page("Epsilon", null, 3),
];

const group = (...items: FilterGroup["items"][number][]): FilterGroup => ({ op: "and", items });
const day = (op: string, value: unknown): FilterGroup["items"][number] => ({ propertyId: DUE, op, value });
const asc = (by: string): PageListSort => ({ by, dir: "asc" });
const desc = (by: string): PageListSort => ({ by, dir: "desc" });

/** Draws the titles of the rows `useViewRows` answers. */
function Rows({ filter, sorts, outerFilter }: { filter: FilterGroup; sorts: readonly PageListSort[]; outerFilter?: FilterGroup }) {
  const { rows } = useViewRows(PAGES, { filter, sorts }, DEFS, null, outerFilter);
  return <p data-testid="rows">{rows.map((row) => row.title).join(",")}</p>;
}

function mount(props: { filter: FilterGroup; sorts?: readonly PageListSort[]; outerFilter?: FilterGroup }) {
  const memory = new MemoryTransport({ now: () => 0, workspaceName: "W", spaces: [{ name: "S", pages: [] }] });
  const store = createAppStore(new CoreClient(memory));
  const stop = store.getState().start();
  const result = render(
    <StoreContext value={store}>
      <Rows filter={props.filter} sorts={props.sorts ?? [asc("title")]} outerFilter={props.outerFilter} />
    </StoreContext>,
  );
  const rows = (): string => result.getByTestId("rows").textContent ?? "";
  return { rows, stop, ...result };
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
  vi.setSystemTime(new Date(2026, 9, 14, 23, 59, 0));
});

afterEach(() => {
  vi.useRealTimers();
});

describe("the rows of a view", () => {
  it("passes the viewer's day to the relative days of the filters", () => {
    const { rows, stop } = mount({ filter: group(day("equals", { relative: "today" })) });
    expect(rows()).toBe("Alpha,Beta,Delta");
    stop();
  });

  it("sorts by the keys of the view, a page that lacks a value being last for each", () => {
    const { rows, stop } = mount({ filter: group(day("equals", { relative: "today" })), sorts: [asc(PRIORITY), desc("title")] });
    expect(rows()).toBe("Beta,Alpha,Delta");
    stop();
  });

  it("keeps the order the pages came in for pages that tie on every key", () => {
    const { rows, stop } = mount({ filter: EMPTY_FILTER_GROUP, sorts: [asc(PRIORITY)] });
    expect(rows()).toBe("Beta,Gamma,Alpha,Epsilon,Delta");
    stop();
  });

  it("applies the dashboard's filter first, then the view's, both with the viewer's day", () => {
    const { rows, stop } = mount({
      filter: group({ propertyId: PRIORITY, op: "greater", value: 1 }),
      outerFilter: group(day("within", { relative: "nextDays", n: 1 })),
    });
    expect(rows()).toBe("Alpha");
    stop();
  });

  it("works the rows out again when midnight passes, with the new day", () => {
    const { rows, stop } = mount({ filter: group(day("equals", { relative: "today" })) });
    expect(rows()).toBe("Alpha,Beta,Delta");
    // Two minutes later it is Thursday: the timer of "today" wakes at midnight.
    act(() => {
      vi.advanceTimersByTime(2 * 60_000);
    });
    expect(rows()).toBe("Gamma");
    stop();
  });

  it("works a period out again at midnight too (the days counted from the new day)", () => {
    const { rows, stop } = mount({ filter: group(day("within", { relative: "pastDays", n: 1 })) });
    // Wednesday and Tuesday: the pages of 14 October.
    expect(rows()).toBe("Alpha,Beta,Delta");
    act(() => {
      vi.advanceTimersByTime(2 * 60_000);
    });
    // Thursday and Wednesday: the pages of 14 and 15 October.
    expect(rows()).toBe("Alpha,Beta,Delta,Gamma");
    stop();
  });
});
