// What `/dashboard` inserts (ADR-0055, `docs/architecture/dashboards.md` section 3): a `pageList` block whose only view is a
// dashboard with a number (the count of the pages) and a chart (`defaultChartSpec`). A dashboard never fits the legacy attributes of
// the block (`page-list-write.ts`), so the block is written as it is after any edit of a view: `views` and the mirror of the
// first view. The schema is not known where a slash command runs, so the chart is the one of a schema with no status or select
// (the count of the pages by the month they were created); its settings change that.

import { emptyView, newViewId, writeViews } from "../page-list-views";
import type { PageListViewSpec } from "../page-list-views";
import { defaultWidget, newWidgetId } from "./dashboard-model";

/** The attributes of a new dashboard block (everything but its `id` and `source`): `views` and the mirror of its view. */
export function dashboardBlockAttrs(): Record<string, unknown> {
  const number = defaultWidget("number", newWidgetId([]), [], []);
  const chart = defaultWidget("chart", newWidgetId([number.id]), [], []);
  const view: PageListViewSpec = { ...emptyView("dashboard", newViewId([])), widgets: [number, chart] };
  return { ...writeViews([view]) };
}
