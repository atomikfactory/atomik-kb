// The shapes of a dashboard (ADR-0055, `docs/architecture/dashboards.md` sections 2 and 4.5): the widgets a view with
// `layout: "dashboard"` keeps in `widgets`, the chart spec and the props between the dashboard (agent E) and the
// charts (agent F). Contract commit: change these only through the orchestrator.

import type { PageInfo, PropertyDef, SpaceId } from "@atomik-kb/core-client";

import type { OpenDisposition } from "../../store-tabs";
import type { FilterGroup } from "../page-list-filter-tree";
import type { CalculationFunction } from "../page-table-calculations";

/** Most widgets one dashboard holds. */
export const MAX_DASHBOARD_WIDGETS = 12;
/** Longest widget title, in characters. */
export const MAX_WIDGET_TITLE_CHARS = 80;

export type DashboardWidgetKind = "number" | "chart" | "view";
export type WidgetWidth = 1 | 2 | 3 | 4;
export type WidgetHeight = "s" | "m" | "l";

/** The pixel height of each widget height. */
export const WIDGET_HEIGHT_PX: Readonly<Record<WidgetHeight, number>> = { s: 140, m: 280, l: 420 };

/** A chart measure: a count of pages, or a summary of a number-like property. */
export type ChartMeasure =
  | { readonly fn: "count" }
  | { readonly fn: "sum" | "average" | "median" | "min" | "max"; readonly propertyId: string };

export type ChartType = "bar" | "line" | "donut";
export type ChartBucket = "day" | "week" | "month" | "quarter" | "year";
export type ChartOrder = "default" | "value_desc" | "value_asc";

/** A chart widget's settings (section 4.1). */
export interface ChartSpec {
  readonly type: ChartType;
  /** A property id, `"created"` or `"updated"`; `null` = not chosen yet. */
  readonly groupBy: string | null;
  /** For a date source; absent means `month`. */
  readonly bucket?: ChartBucket;
  readonly measure: ChartMeasure;
  /** Bar only. */
  readonly horizontal?: boolean;
  /** Line only. */
  readonly cumulative?: boolean;
  /** Not for a date source. */
  readonly order?: ChartOrder;
  /** A "No value" group for pages without a value. */
  readonly showEmpty?: boolean;
  /** Keys this version does not know, kept. */
  readonly extra?: Readonly<Record<string, unknown>>;
}

/** One widget of a dashboard (section 2.2). */
export interface DashboardWidget {
  /** 8 hexadecimal characters, unique in the view. */
  readonly id: string;
  readonly kind: DashboardWidgetKind;
  /** At most 80 characters; `""` = the automatic title. */
  readonly title: string;
  readonly w: WidgetWidth;
  readonly h: WidgetHeight;
  /** Number and chart: AND with the dashboard's filter. */
  readonly filter?: FilterGroup;
  /** Number: a column (`"title"` or a property id) and a table-footer function. */
  readonly calc?: { readonly by: string; readonly fn: CalculationFunction };
  /** Chart. */
  readonly chart?: ChartSpec;
  /** View: another view of the block (never a dashboard). */
  readonly viewId?: string;
  /** Keys this version does not know, kept. */
  readonly extra?: Readonly<Record<string, unknown>>;
}

/** A stored widget this version cannot read (an unknown kind): kept and written back as it was. */
export interface RawDashboardWidget {
  readonly raw: unknown;
}

/** The props of a chart widget's body (agent F's `ChartWidgetBody`). */
export interface ChartWidgetBodyProps {
  readonly chart: ChartSpec;
  /** The pages after the dashboard's and the widget's filters. */
  readonly rows: readonly PageInfo[];
  readonly schema: readonly PropertyDef[];
  readonly spaceId: SpaceId;
  /** The body's size in pixels (the frame measures it). */
  readonly width: number;
  readonly height: number;
  readonly today: string;
  /** The widget's title (set or automatic), for the accessible name. */
  readonly title: string;
  /** Opens a page of a mark's popover as the dashboard view says. */
  readonly onOpen: (page: PageInfo, disposition: OpenDisposition) => void;
}

/** The props of a chart widget's settings (agent F's `ChartWidgetSettings`). */
export interface ChartWidgetSettingsProps {
  readonly chart: ChartSpec;
  readonly schema: readonly PropertyDef[];
  readonly onChange: (chart: ChartSpec) => void;
}
