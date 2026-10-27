// A chart widget's body and settings (ADR-0055, `docs/architecture/dashboards.md` section 4): the dashboard frame (agent E)
// mounts these and never draws a chart itself.
//
// The body groups and measures the rows (`chart-model.ts`) and draws the bar, line or donut of the spec's type in SVG
// (`charts/`). The drawing is one tab stop with a tooltip; a click or Enter on a mark opens a popover with that group's pages
// (at most 50, then "and k more"), and a click on a page opens it the way the dashboard says (`onOpen`: Ctrl+click and Ctrl+Enter a
// tab). The chart's accessible name lists its groups and values, and a visually hidden table says the same; a chart with nothing
// to draw says why (no group chosen, no number to measure, no data). Page titles and people's names come from the app's store when
// there is one (a chart over a relation or a person property), and read as "Unknown page" and the account otherwise.
//
// The settings are the chart's part of the widget's settings panel: labelled fields like the toolbar's view settings.

import { useCallback, useContext, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { KeyboardEvent } from "react";
import type { PageInfo, PropertyDef, SpaceId } from "@atomik-kb/core-client";

import { personName } from "../../comments";
import { FloatingPanel } from "../../components/FloatingPanel";
import type { Anchor } from "../../components/FloatingPanel";
import { PageIcon } from "../../components/PageIcon";
import { activeLocale, t, tp } from "../../i18n";
import type { MessageKey } from "../../i18n";
import { dispositionFromKey, openGestures } from "../../page-open";
import { formatRollupNumber } from "../../property-format";
import { StoreContext } from "../../store-context";
import type { OpenDisposition } from "../../store-tabs";
import { indexOf } from "../node-views";
import { PageListField } from "../PageListField";
import {
  CHART_BUCKETS,
  CHART_ORDERS,
  CHART_POPOVER_PAGES,
  CHART_TYPES,
  GROUP_BY_CREATED,
  GROUP_BY_UPDATED,
  MEASURE_FUNCTIONS,
  chartData,
  chartMeasureLabel,
  groupableProperties,
  isDateSource,
  measurableProperties,
  withGroupBy,
  withMeasureFunction,
  withMeasureProperty,
} from "./chart-model";
import type { ChartGroup, MeasureFunction } from "./chart-model";
import type { ChartBucket, ChartOrder, ChartSpec, ChartType, ChartWidgetBodyProps, ChartWidgetSettingsProps } from "./dashboard-types";
import { BarChart } from "./charts/BarChart";
import { DonutChart } from "./charts/DonutChart";
import { LineChart } from "./charts/LineChart";
import type { ChartDescription, ChartViewProps } from "./charts/chart-parts";

/** A number as the table footer writes it: the locale's, at most two decimals. */
function formatValue(value: number): string {
  return formatRollupNumber(value, "sum");
}

// ----- names ---------------------------------------------------------------------------------------

const noop = (): void => undefined;

/**
 * What a chart needs from the app to name its groups: a page's title (a relation) and a person's name. Read from the store of the
 * app when there is one (the tree for the titles, the space's people, asked for when a person property is the source); without a
 * store the titles are unknown and a person is its account.
 */
function useChartNames(spaceId: SpaceId, needsPeople: boolean) {
  const store = useContext(StoreContext);
  const subscribe = useCallback((listener: () => void) => (store === null ? noop : store.subscribe(listener)), [store]);
  const tree = useSyncExternalStore(subscribe, () => store?.getState().tree ?? null);
  const people = useSyncExternalStore(subscribe, () => store?.getState().people[spaceId]);
  const missing = needsPeople && people === undefined;
  useEffect(() => {
    if (store !== null && missing) {
      store
        .getState()
        .ensurePeople(spaceId)
        .catch((error: unknown) => {
          console.error("could not read the people of the space", error instanceof Error ? error.name : "unknown");
        });
    }
  }, [store, missing, spaceId]);
  return useMemo(
    () => ({
      pageTitle: (id: string): string | null => (tree === null ? null : (indexOf(tree).get(id)?.title ?? null)),
      personName: (id: string): string => (store === null ? id : people === undefined ? "…" : personName(people, id)),
    }),
    [tree, people, store],
  );
}

// ----- the popover of a mark -----------------------------------------------------------------------

/** One page of a mark's popover: a button that opens it as the gesture says. */
function PagesPopoverButton({ page, onOpen }: { page: PageInfo; onOpen: (page: PageInfo, disposition: OpenDisposition) => void }) {
  const open = (disposition: OpenDisposition): void => {
    onOpen(page, disposition);
  };
  return (
    <button
      type="button"
      className="chart-popover-page"
      {...openGestures<HTMLButtonElement>(open)}
      onKeyDown={(event: KeyboardEvent<HTMLButtonElement>) => {
        // Ctrl+Enter opens a tab in front; the browser would send a click with Ctrl held, which means a background tab.
        if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
          event.preventDefault();
          open(dispositionFromKey(event));
        }
      }}
    >
      <PageIcon icon={page.icon} className="page-list-icon" />
      <span className="chart-popover-page-title">{page.title === "" ? t("page.untitled") : page.title}</span>
    </button>
  );
}

/** The pages of a group, in a small popover at the mark: at most `CHART_POPOVER_PAGES`, then "and k more". */
function PagesPopover({
  group,
  anchor,
  anchorElement,
  valueText,
  onOpen,
  onClose,
}: {
  group: ChartGroup;
  anchor: Anchor;
  anchorElement: Element | null;
  valueText: string;
  onOpen: (page: PageInfo, disposition: OpenDisposition) => void;
  onClose: () => void;
}) {
  const shown = group.pages.slice(0, CHART_POPOVER_PAGES);
  const more = group.pages.length - shown.length;
  return (
    <FloatingPanel
      anchor={anchor}
      label={t("chart.popover.label", { group: group.fullLabel })}
      className="chart-popover"
      anchorElement={anchorElement}
      onClose={onClose}
    >
      <p className="chart-popover-title">
        <span className="chart-popover-group">{group.fullLabel}</span>
        <span className="chart-popover-value">{valueText}</span>
      </p>
      {shown.length === 0 ? (
        <p className="chart-popover-empty">{t("chart.popover.none")}</p>
      ) : (
        <ul className="chart-popover-pages">
          {shown.map((page) => (
            <li key={page.id}>
              <PagesPopoverButton
                page={page}
                onOpen={(opened, disposition) => {
                  onOpen(opened, disposition);
                  if (disposition !== "background") {
                    onClose();
                  }
                }}
              />
            </li>
          ))}
        </ul>
      )}
      {more > 0 && <p className="chart-popover-more">{t("chart.popover.more", { count: more })}</p>}
    </FloatingPanel>
  );
}

// ----- the body ------------------------------------------------------------------------------------

/** The room the "+k more" line takes under a chart, in pixels. */
const MORE_HEIGHT = 18;

/** Draws the chart of a widget. */
export function ChartWidgetBody({ chart, rows, schema, spaceId, width, height, title, onOpen }: ChartWidgetBodyProps) {
  const groupKind = schema.find((def) => def.id === chart.groupBy)?.kind;
  const names = useChartNames(spaceId, groupKind === "person" || groupKind === "formula");
  const locale = activeLocale();
  const data = useMemo(
    () => chartData(chart, rows, schema, { locale, pageTitle: names.pageTitle, personName: names.personName }),
    [chart, rows, schema, locale, names],
  );
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  const [selected, setSelected] = useState<{ key: string; anchor: Anchor } | null>(null);

  if (width <= 0 || height <= 0) {
    return <div className="chart-body" ref={setRoot} />;
  }
  if (data.status !== "ok") {
    const message: MessageKey =
      data.status === "no_group" ? "chart.chooseGroup" : data.status === "no_measure" ? "chart.chooseMeasure" : "chart.empty";
    return (
      <div className="chart-body" ref={setRoot} style={{ width, height }}>
        <p className="chart-message">{t(message)}</p>
      </div>
    );
  }

  const measureLabel = chartMeasureLabel(chart.measure, schema, t);
  const additive = chart.measure.fn === "count" || chart.measure.fn === "sum";
  const shares = chart.type === "donut" && additive && data.total !== null && data.total > 0;
  const sliceTotal = data.total ?? 0;
  const percent = (value: number): string =>
    new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: value / sliceTotal < 0.1 ? 1 : 0 }).format(value / sliceTotal);
  const valueText = (group: ChartGroup): string => (group.value === null ? "—" : formatValue(group.value));

  const describe = (group: ChartGroup): ChartDescription => {
    const value = group.value !== null && shares ? `${valueText(group)} (${percent(group.value)})` : valueText(group);
    const lines = [t("chart.tooltip.value", { measure: measureLabel, value })];
    if (chart.measure.fn !== "count") {
      lines.push(tp("chart.pages", group.pages.length, { count: group.pages.length }));
    }
    return { title: group.fullLabel, lines };
  };

  const items = data.groups.map((group) => `${group.fullLabel}: ${valueText(group)}`);
  if (data.hidden > 0 && chart.type !== "donut") {
    items.push(t("chart.more", { count: data.hidden }));
  }
  const label = t("chart.aria", { title, items: items.join("; ") });

  const select = (index: number, anchor: Anchor): void => {
    const group = data.groups[index];
    if (group !== undefined) {
      setSelected((current) => (current?.key === group.key ? null : { key: group.key, anchor }));
    }
  };

  const showMore = data.hidden > 0 && chart.type !== "donut";
  const view: ChartViewProps = {
    groups: data.groups,
    width,
    height: showMore ? Math.max(40, height - MORE_HEIGHT) : height,
    dated: data.dated,
    label,
    formatValue,
    describe,
    onSelect: select,
  };
  const selectedGroup = selected === null ? undefined : data.groups.find((group) => group.key === selected.key);

  return (
    <div className="chart-body" ref={setRoot} style={{ width, height }} data-type={chart.type}>
      {chart.type === "bar" && <BarChart {...view} horizontal={chart.horizontal === true} />}
      {chart.type === "line" && <LineChart {...view} />}
      {chart.type === "donut" && (
        <DonutChart {...view} total={data.total} totalLabel={measureLabel} shares={shares} locale={locale} />
      )}
      {showMore && <p className="chart-more">{t("chart.more", { count: data.hidden })}</p>}
      <table className="visually-hidden">
        <caption>{title}</caption>
        <thead>
          <tr>
            <th scope="col">{t("chart.table.group")}</th>
            <th scope="col">{measureLabel}</th>
          </tr>
        </thead>
        <tbody>
          {data.groups.map((group) => (
            <tr key={group.key}>
              <th scope="row">{group.fullLabel}</th>
              <td>{valueText(group)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {selected !== null && selectedGroup !== undefined && (
        <PagesPopover
          key={selectedGroup.key}
          group={selectedGroup}
          anchor={selected.anchor}
          anchorElement={root}
          valueText={`${measureLabel}: ${valueText(selectedGroup)}`}
          onOpen={onOpen}
          onClose={() => {
            setSelected(null);
          }}
        />
      )}
    </div>
  );
}

// ----- the settings --------------------------------------------------------------------------------

const TYPE_LABELS: Readonly<Record<ChartType, MessageKey>> = {
  bar: "chart.type.bar",
  line: "chart.type.line",
  donut: "chart.type.donut",
};

const BUCKET_LABELS: Readonly<Record<ChartBucket, MessageKey>> = {
  day: "chart.bucket.option.day",
  week: "chart.bucket.option.week",
  month: "chart.bucket.option.month",
  quarter: "chart.bucket.option.quarter",
  year: "chart.bucket.option.year",
};

const ORDER_LABELS: Readonly<Record<ChartOrder, MessageKey>> = {
  default: "chart.order.default",
  value_desc: "chart.order.value_desc",
  value_asc: "chart.order.value_asc",
};

const MEASURE_LABELS: Readonly<Record<MeasureFunction, MessageKey>> = {
  count: "chart.measure.count",
  sum: "properties.rollup.fn.sum",
  average: "properties.rollup.fn.average",
  median: "properties.rollup.fn.median",
  min: "properties.rollup.fn.min",
  max: "properties.rollup.fn.max",
};

function isMeasureFunction(value: string): value is MeasureFunction {
  return (MEASURE_FUNCTIONS as readonly string[]).includes(value);
}

function isBucket(value: string): value is ChartBucket {
  return (CHART_BUCKETS as readonly string[]).includes(value);
}

function isOrder(value: string): value is ChartOrder {
  return (CHART_ORDERS as readonly string[]).includes(value);
}

/** A checkbox of the settings, in the row style of the view settings' lists. */
function SettingsCheckbox({ label, checked, onChange }: { label: string; checked: boolean; onChange: (checked: boolean) => void }) {
  return (
    <label className="page-list-prop">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => {
          onChange(event.target.checked);
        }}
      />
      <span>{label}</span>
    </label>
  );
}

/** The chart part of a widget's settings. */
export function ChartWidgetSettings({ chart, schema, onChange }: ChartWidgetSettingsProps) {
  const groupable = groupableProperties(schema);
  const measurable = measurableProperties(schema);
  const groupBy = chart.groupBy;
  const groupKnown = groupBy === GROUP_BY_CREATED || groupBy === GROUP_BY_UPDATED || groupable.some((def) => def.id === groupBy);
  const dated = groupKnown && isDateSource(groupBy, schema);
  const propertyId = chart.measure.fn === "count" ? "" : chart.measure.propertyId;
  const propertyKnown = measurable.some((def) => def.id === propertyId);
  const set = (patch: Partial<ChartSpec>): void => {
    onChange({ ...chart, ...patch });
  };

  return (
    <>
      <div role="group" aria-label={t("chart.settings.type")} className="page-list-field">
        <span className="page-list-field-label">{t("chart.settings.type")}</span>
        <div className="chart-type-buttons">
          {CHART_TYPES.map((type) => (
            <button
              key={type}
              type="button"
              className="chart-type-button"
              aria-pressed={chart.type === type}
              onClick={() => {
                set({ type });
              }}
            >
              {t(TYPE_LABELS[type])}
            </button>
          ))}
        </div>
      </div>
      <PageListField label={t("chart.settings.groupBy")}>
        <select
          className="page-list-select"
          value={groupKnown && groupBy !== null ? groupBy : ""}
          onChange={(event) => {
            if (event.target.value !== "") {
              onChange(withGroupBy(chart, event.target.value, schema));
            }
          }}
        >
          <option value="" disabled>
            {t("chart.settings.groupBy.choose")}
          </option>
          {groupable.map((def: PropertyDef) => (
            <option key={def.id} value={def.id}>
              {def.name}
            </option>
          ))}
          <option value={GROUP_BY_CREATED}>{t("chart.group.created")}</option>
          <option value={GROUP_BY_UPDATED}>{t("chart.group.updated")}</option>
        </select>
      </PageListField>
      {dated && (
        <PageListField label={t("chart.settings.bucket")}>
          <select
            className="page-list-select"
            value={chart.bucket ?? "month"}
            onChange={(event) => {
              if (isBucket(event.target.value)) {
                set({ bucket: event.target.value });
              }
            }}
          >
            {CHART_BUCKETS.map((bucket) => (
              <option key={bucket} value={bucket}>
                {t(BUCKET_LABELS[bucket])}
              </option>
            ))}
          </select>
        </PageListField>
      )}
      <PageListField label={t("chart.settings.measure")}>
        <select
          className="page-list-select"
          value={chart.measure.fn}
          onChange={(event) => {
            if (isMeasureFunction(event.target.value)) {
              onChange(withMeasureFunction(chart, event.target.value, schema));
            }
          }}
        >
          {MEASURE_FUNCTIONS.map((fn) => (
            <option key={fn} value={fn} disabled={fn !== "count" && measurable.length === 0}>
              {t(MEASURE_LABELS[fn])}
            </option>
          ))}
        </select>
      </PageListField>
      {chart.measure.fn !== "count" && (
        <PageListField label={t("chart.settings.property")}>
          <select
            className="page-list-select"
            value={propertyKnown ? propertyId : ""}
            onChange={(event) => {
              if (event.target.value !== "") {
                onChange(withMeasureProperty(chart, event.target.value));
              }
            }}
          >
            <option value="" disabled>
              {t("chart.settings.property.choose")}
            </option>
            {measurable.map((def) => (
              <option key={def.id} value={def.id}>
                {def.name}
              </option>
            ))}
          </select>
        </PageListField>
      )}
      {!dated && (
        <PageListField label={t("chart.settings.order")}>
          <select
            className="page-list-select"
            value={chart.order ?? "default"}
            onChange={(event) => {
              if (isOrder(event.target.value)) {
                set({ order: event.target.value === "default" ? undefined : event.target.value });
              }
            }}
          >
            {CHART_ORDERS.map((order) => (
              <option key={order} value={order}>
                {t(ORDER_LABELS[order])}
              </option>
            ))}
          </select>
        </PageListField>
      )}
      {chart.type === "bar" && (
        <SettingsCheckbox
          label={t("chart.settings.horizontal")}
          checked={chart.horizontal === true}
          onChange={(checked) => {
            set({ horizontal: checked ? true : undefined });
          }}
        />
      )}
      {chart.type === "line" && (
        <SettingsCheckbox
          label={t("chart.settings.cumulative")}
          checked={chart.cumulative === true}
          onChange={(checked) => {
            set({ cumulative: checked ? true : undefined });
          }}
        />
      )}
      {!(chart.type === "line" && dated) && (
        <SettingsCheckbox
          label={t("chart.settings.showEmpty")}
          checked={chart.showEmpty === true}
          onChange={(checked) => {
            set({ showEmpty: checked ? true : undefined });
          }}
        />
      )}
    </>
  );
}
