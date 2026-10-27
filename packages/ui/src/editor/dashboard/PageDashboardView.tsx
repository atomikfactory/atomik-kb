// The dashboard layout of a database block (ADR-0055, `docs/architecture/dashboards.md` section 3): a grid of number, chart and
// embedded-view widgets over the view's rows. A view with `layout: "dashboard"` keeps its widgets in `view.widgets` (`dashboard-model.ts`);
// every change of the grid is one `onUpdateView({ widgets })`.
//
// **The grid** is a CSS grid of four equal columns in list order; each widget takes one to four columns (`w`) and one of three heights
// (`h`). The dashboard is a container (`container-type: inline-size`): below 720 px it has two columns, below 420 px one, so it
// works in the side peek (`database-dashboard.css`). **The rows** are the pages after the dashboard view's filter; a number or a chart
// applies its own filter on top (`applyFilterTree`); an embedded view starts from the source's pages and runs its own pipeline.
//
// **Editing.** In an editable editor each widget has a frame with a "…" menu (`DashboardWidgetFrame.tsx`), its header is the handle it is
// dragged by to another place (pointer events: the drop indicator is a bar between the widgets, and the drop writes the new order in one
// change; Escape cancels), and an "Add widget" tile ends the grid (it is also the only content of an empty dashboard). Edit… opens the
// settings of the widget in a floating panel (`WidgetSettings.tsx`) under its header; a new widget opens them at once, under the tile. A read-only editor (a viewer, a
// locked page) draws the widgets without menus, handle or tile, and its embedded views are read-only too.
//
// A widget this version cannot read (a newer kind) is kept as it was and drawn as "This widget needs a newer version".

import type { PageInfo } from "@atomik-kb/core-client";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";

import { anchorBelow } from "../../components/FloatingPanel";
import type { Anchor } from "../../components/FloatingPanel";
import { PlusIcon } from "../../components/icons";
import { Menu } from "../../components/Menu";
import type { MenuItem } from "../../components/Menu";
import { activeLocale, t } from "../../i18n";
import { firstDayOfWeek } from "../dates";
import { applyFilterTree, countFilters } from "../page-list-filter-tree";
import type { FilterContext } from "../page-list-filter-tree";
import type { PageListLayoutProps } from "../page-list-layout-types";
import type { PageListLayout } from "../page-list-model";
import { useRowLookups } from "../use-view-rows";
import { ChartWidgetBody } from "./ChartWidget";
import { DashboardWidgetFrame } from "./DashboardWidgetFrame";
import type { FrameCommand } from "./DashboardWidgetFrame";
import {
  addWidget,
  automaticTitle,
  canAddWidget,
  defaultWidget,
  duplicateWidget,
  embeddableViews,
  entryKey,
  isRawEntry,
  moveWidget,
  moveWidgetTo,
  newWidgetId,
  removeWidget,
  sameEntries,
  updateWidget,
  widgetIds,
  widgetTitle,
  WIDGET_KINDS,
} from "./dashboard-model";
import type { DashboardEntry, TitleContext } from "./dashboard-model";
import { MAX_DASHBOARD_WIDGETS, WIDGET_HEIGHT_PX } from "./dashboard-types";
import type { DashboardWidget, DashboardWidgetKind } from "./dashboard-types";
import { EmbeddedViewWidget } from "./EmbeddedViewWidget";
import { NumberWidget } from "./NumberWidget";
import { useElementSize } from "./use-element-size";
import { WidgetSettings } from "./WidgetSettings";

const NO_ENTRIES: readonly DashboardEntry[] = [];

/** How far the pointer moves (pixels) before a press on a header becomes a drag. */
const DRAG_THRESHOLD = 4;
/** The height of a frame's header, in pixels (the chart is drawn in what is left). */
const HEADER_HEIGHT = 36;
/** The width a chart is drawn at before it has been measured. */
const CHART_FALLBACK_WIDTH = 320;

const layoutName = (layout: PageListLayout): string => t(`pageList.layout.${layout}`);

const ADD_KEYS = {
  number: "dashboard.add.number",
  chart: "dashboard.add.chart",
  view: "dashboard.add.view",
} as const;

/** The frame under the grid's children with this key. */
function frameOf(grid: HTMLElement | null, key: string): HTMLElement | undefined {
  return Array.from(grid?.querySelectorAll<HTMLElement>("[data-widget-id]") ?? []).find((frame) => frame.dataset.widgetId === key);
}

/**
 * The drop slot the pointer is at: before the frame it is over when it is in the frame's left half, after it otherwise; with the
 * pointer in a gap, the same for the frame whose centre is nearest. Slots count the frames as they are now (0 is before the first).
 */
function slotAt(grid: HTMLElement, x: number, y: number, fallback: number): number {
  const frames = Array.from(grid.querySelectorAll<HTMLElement>("[data-widget-id]"));
  let best: { index: number; distance: number; rect: DOMRect } | null = null;
  for (const [index, frame] of frames.entries()) {
    const rect = frame.getBoundingClientRect();
    if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
      return x < rect.left + rect.width / 2 ? index : index + 1;
    }
    const distance = Math.hypot(x - (rect.left + rect.width / 2), y - (rect.top + rect.height / 2));
    if (best === null || distance < best.distance) {
      best = { index, distance, rect };
    }
  }
  return best === null ? fallback : x < best.rect.left + best.rect.width / 2 ? best.index : best.index + 1;
}

interface ChartContentProps {
  readonly widget: DashboardWidget;
  readonly rows: readonly PageInfo[];
  readonly dashboard: PageListLayoutProps;
  readonly title: string;
}

/** The body of a chart widget: measured, so that the chart is drawn at its size. */
function ChartContent({ widget, rows, dashboard, title }: ChartContentProps) {
  const [ref, size] = useElementSize<HTMLDivElement>({
    width: CHART_FALLBACK_WIDTH,
    height: WIDGET_HEIGHT_PX[widget.h] - HEADER_HEIGHT,
  });
  return (
    <div ref={ref} className="page-dashboard-chart">
      {widget.chart !== undefined && (
        <ChartWidgetBody
          chart={widget.chart}
          rows={rows}
          schema={dashboard.schema}
          spaceId={dashboard.spaceId}
          width={size.width}
          height={size.height}
          today={dashboard.today}
          title={title}
          onOpen={dashboard.onOpen}
        />
      )}
    </div>
  );
}

interface ContentProps {
  readonly widget: DashboardWidget;
  readonly dashboard: PageListLayoutProps;
  readonly context: FilterContext;
  readonly title: string;
  readonly onRemove: () => void;
}

/** The content of a widget: its kind's body over the rows after the widget's own filter. */
function WidgetContent({ widget, dashboard, context, title, onRemove }: ContentProps) {
  const { rows, schema, today, view } = dashboard;
  const { filter } = widget;
  // Only a number and a chart read the rows (an embedded view starts from the source's pages).
  const widgetRows = useMemo(
    () => (widget.kind === "view" || filter === undefined ? rows : applyFilterTree(rows, filter, context)),
    [widget.kind, filter, rows, context],
  );
  switch (widget.kind) {
    case "number":
      return <NumberWidget calc={widget.calc ?? { by: "title", fn: "count_all" }} schema={schema} rows={widgetRows} today={today} />;
    case "chart":
      return <ChartContent widget={widget} rows={widgetRows} dashboard={dashboard} title={title} />;
    case "view": {
      const target = dashboard.views?.find((candidate) => candidate.id === widget.viewId);
      return (
        <EmbeddedViewWidget
          target={target === undefined || target.layout === "dashboard" || target.id === view.id ? undefined : target}
          dashboard={dashboard}
          onRemove={onRemove}
        />
      );
    }
  }
}

/** The dashboard: the layout of a view whose `layout` is `dashboard`. */
export function PageDashboardView(props: PageListLayoutProps) {
  const { view, schema, spaceId, editable, today, onUpdateView, onActivateView } = props;
  const entries = view.widgets ?? NO_ENTRIES;
  const views = useMemo(() => props.views ?? [view], [props.views, view]);
  const { me, lookups } = useRowLookups(spaceId === "" ? null : spaceId);
  // The widgets' filters resolve "today" and "this week" for the viewer, as the view's own filter does (`useViewRows`).
  const locale = activeLocale();
  const weekStart = useMemo(() => firstDayOfWeek(locale), [locale]);
  const context = useMemo<FilterContext>(() => ({ defs: schema, me, lookups, today, weekStart }), [schema, me, lookups, today, weekStart]);
  const titles = useMemo<TitleContext>(() => ({ schema, views, layoutName, t }), [schema, views]);

  const [settings, setSettings] = useState<{ id: string; anchor: Anchor } | null>(null);
  const [addMenu, setAddMenu] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const [drag, setDrag] = useState<{ from: number; slot: number } | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const stopDrag = useRef<(() => void) | null>(null);

  /** Writes a new list of widgets (one change of the view); nothing when the list is the very same. */
  function write(next: readonly DashboardEntry[]): void {
    if (!sameEntries(next, entries)) {
      onUpdateView({ widgets: next });
    }
  }

  // The handlers of a drag outlive the render that began it: they read the latest list and writer from here.
  const latest = useRef({ entries, write });
  useLayoutEffect(() => {
    latest.current = { entries, write };
  });
  // A drag in progress ends with the dashboard.
  useEffect(
    () => () => {
      stopDrag.current?.();
    },
    [],
  );

  function openSettings(id: string): void {
    const header = frameOf(gridRef.current, id)?.querySelector<HTMLElement>(".page-dashboard-widget-header");
    setSettings({ id, anchor: header === null || header === undefined ? { x: 0, y: 0 } : anchorBelow(header) });
  }

  /**
   * Adds a widget of `kind` at the end and opens its settings, below the tile it came from (where the widget appears): the panel is
   * drawn once the widget is in the list, whenever the write comes back.
   */
  function add(kind: DashboardWidgetKind, anchor: Anchor): void {
    if (!canAddWidget(entries)) {
      return;
    }
    const widget = defaultWidget(kind, newWidgetId(widgetIds(entries)), schema, views);
    write(addWidget(entries, widget));
    setSettings({ id: widget.id, anchor });
  }

  function runCommand(index: number, entry: DashboardEntry, command: FrameCommand): void {
    const id = isRawEntry(entry) ? null : entry.id;
    switch (command.type) {
      case "edit":
        if (id !== null) {
          openSettings(id);
        }
        break;
      case "duplicate":
        write(duplicateWidget(entries, index, newWidgetId(widgetIds(entries))));
        break;
      case "earlier":
        write(moveWidget(entries, index, -1));
        break;
      case "later":
        write(moveWidget(entries, index, 1));
        break;
      case "delete":
        write(removeWidget(entries, index));
        break;
      case "width":
        if (id !== null) {
          write(updateWidget(entries, id, { w: command.w }));
        }
        break;
      case "height":
        if (id !== null) {
          write(updateWidget(entries, id, { h: command.h }));
        }
        break;
    }
  }

  /** A press on the header of the widget at `from`: a drag begins once the pointer moves a few pixels. */
  function startDrag(from: number, event: ReactPointerEvent<HTMLElement>): void {
    const grid = gridRef.current;
    if (grid === null) {
      return;
    }
    stopDrag.current?.();
    const startX = event.clientX;
    const startY = event.clientY;
    let active = false;
    let slot = from;
    const end = (drop: boolean): void => {
      document.removeEventListener("pointermove", move);
      document.removeEventListener("pointerup", up);
      document.removeEventListener("pointercancel", cancel);
      document.removeEventListener("keydown", key, true);
      stopDrag.current = null;
      setDrag(null);
      if (drop && active) {
        latest.current.write(moveWidgetTo(latest.current.entries, from, slot));
      }
    };
    const move = (moved: PointerEvent): void => {
      if (!active && Math.hypot(moved.clientX - startX, moved.clientY - startY) < DRAG_THRESHOLD) {
        return;
      }
      active = true;
      slot = slotAt(grid, moved.clientX, moved.clientY, slot);
      setDrag({ from, slot });
    };
    const up = (): void => {
      end(true);
    };
    const cancel = (): void => {
      end(false);
    };
    const key = (pressed: KeyboardEvent): void => {
      if (pressed.key === "Escape") {
        pressed.stopPropagation();
        end(false);
      }
    };
    stopDrag.current = () => {
      end(false);
    };
    document.addEventListener("pointermove", move);
    document.addEventListener("pointerup", up);
    document.addEventListener("pointercancel", cancel);
    document.addEventListener("keydown", key, true);
  }

  /** The drop indicator of the frame at `index`: a bar before it, or after the last one. */
  function dropMark(index: number): "before" | "after" | null {
    if (drag === null || drag.slot === drag.from || drag.slot === drag.from + 1) {
      return null;
    }
    if (drag.slot === index) {
      return "before";
    }
    return drag.slot === entries.length && index === entries.length - 1 ? "after" : null;
  }

  const settingsWidget =
    settings === null ? undefined : entries.find((entry): entry is DashboardWidget => !isRawEntry(entry) && entry.id === settings.id);
  const addItems: MenuItem[] = WIDGET_KINDS.map((kind) => ({
    id: kind,
    label: t(ADD_KEYS[kind]),
    disabled: kind === "view" && embeddableViews(views).length === 0,
    title: kind === "view" && embeddableViews(views).length === 0 ? t("dashboard.add.viewNone") : undefined,
  }));
  const full = !canAddWidget(entries);

  return (
    <div className="page-dashboard" role="group" aria-label={t("dashboard.label")} data-dragging={drag === null ? undefined : ""}>
      {entries.length === 0 && (
        <p className="page-dashboard-empty">{editable ? t("dashboard.empty") : t("dashboard.emptyReadOnly")}</p>
      )}
      <div className="page-dashboard-grid" ref={gridRef}>
        {entries.map((entry, index) => {
          const key = entryKey(entry, index);
          if (isRawEntry(entry)) {
            return (
              <DashboardWidgetFrame
                key={key}
                entryKey={key}
                title={t("dashboard.widget.unknownTitle")}
                kind="raw"
                w={4}
                h="s"
                filterCount={0}
                editable={editable}
                index={index}
                count={entries.length}
                canDuplicate={false}
                dragging={drag?.from === index}
                drop={dropMark(index)}
                onCommand={(command) => {
                  runCommand(index, entry, command);
                }}
                onDragStart={(event) => {
                  startDrag(index, event);
                }}
              >
                <div className="page-dashboard-message">
                  <p>{t("dashboard.widget.unknown")}</p>
                  {editable && (
                    <button
                      type="button"
                      className="page-list-button"
                      onClick={() => {
                        write(removeWidget(entries, index));
                      }}
                    >
                      {t("dashboard.widget.remove")}
                    </button>
                  )}
                </div>
              </DashboardWidgetFrame>
            );
          }
          const title = widgetTitle(entry, titles);
          const target = entry.kind === "view" ? views.find((candidate) => candidate.id === entry.viewId) : undefined;
          const openable = target !== undefined && target.layout !== "dashboard" && target.id !== view.id && onActivateView !== undefined;
          return (
            <DashboardWidgetFrame
              key={key}
              entryKey={key}
              title={title}
              kind={entry.kind}
              w={entry.w}
              h={entry.h}
              filterCount={entry.filter === undefined ? 0 : countFilters(entry.filter)}
              editable={editable}
              index={index}
              count={entries.length}
              canDuplicate={!full}
              dragging={drag?.from === index}
              drop={dropMark(index)}
              actions={
                openable ? (
                  <button
                    type="button"
                    className="page-list-button page-dashboard-action"
                    aria-label={t("dashboard.view.openNamed", { name: title })}
                    onClick={() => {
                      onActivateView(target.id);
                    }}
                  >
                    {t("dashboard.view.open")}
                  </button>
                ) : undefined
              }
              onCommand={(command) => {
                runCommand(index, entry, command);
              }}
              onDragStart={(event) => {
                startDrag(index, event);
              }}
            >
              <WidgetContent
                widget={entry}
                dashboard={props}
                context={context}
                title={title}
                onRemove={() => {
                  write(removeWidget(entries, index));
                }}
              />
            </DashboardWidgetFrame>
          );
        })}
        {editable && (
          <button
            type="button"
            className="page-dashboard-add"
            aria-haspopup="menu"
            aria-expanded={addMenu !== null}
            disabled={full}
            title={full ? t("dashboard.add.limit", { max: MAX_DASHBOARD_WIDGETS }) : undefined}
            onClick={(event) => {
              const trigger = event.currentTarget;
              setAddMenu((current) => (current === null ? { anchor: anchorBelow(trigger), trigger } : null));
            }}
          >
            <PlusIcon />
            <span>{t("dashboard.add")}</span>
          </button>
        )}
      </div>
      {addMenu !== null && (
        <Menu
          items={addItems}
          anchor={addMenu.anchor}
          anchorElement={addMenu.trigger}
          label={t("dashboard.add.menu")}
          onSelect={(id) => {
            const kind = WIDGET_KINDS.find((candidate) => candidate === id);
            if (kind !== undefined) {
              add(kind, addMenu.anchor);
            }
          }}
          onClose={() => {
            setAddMenu(null);
          }}
        />
      )}
      {editable && settings !== null && settingsWidget !== undefined && (
        <WidgetSettings
          key={settingsWidget.id}
          widget={settingsWidget}
          schema={schema}
          views={views}
          layoutName={layoutName}
          automaticTitle={automaticTitle(settingsWidget, titles)}
          anchor={settings.anchor}
          onChange={(patch) => {
            write(updateWidget(entries, settingsWidget.id, patch));
          }}
          onClose={() => {
            setSettings(null);
          }}
        />
      )}
    </div>
  );
}
