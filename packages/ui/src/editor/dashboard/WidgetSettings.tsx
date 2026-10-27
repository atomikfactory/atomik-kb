// The settings of a widget of a dashboard (ADR-0055, `docs/architecture/dashboards.md` section 3), in a floating panel
// anchored to the widget: its title (empty: the automatic one, shown as the hint), the settings of its kind (a number: the column
// and the function; a chart: `ChartWidgetSettings` of the charts; a view: which view of the block) and, for a number and a chart, the
// widget's own filter, in the filter editor of the toolbar (`PageListFilters`) over the widget's `filter`. Every control writes
// one `onChange(patch)`; the title writes when the field is left, on Enter or when the panel closes, not on every key.
//
// The filter editor is itself a floating panel: a press in it is outside this one, so this one ignores presses and Escape while it
// is open (the same care `PageListFilters` takes for its page picker).

import type { PropertyDef } from "@atomik-kb/core-client";
import { useRef, useState } from "react";

import { anchorBelow, FloatingPanel } from "../../components/FloatingPanel";
import type { Anchor, CloseReason } from "../../components/FloatingPanel";
import { t, tp } from "../../i18n";
import { countFilters, EMPTY_FILTER_GROUP } from "../page-list-filter-tree";
import type { PageListLayout } from "../page-list-model";
import type { PageListViewSpec } from "../page-list-views";
import { calculationFunctionsFor, CALCULATION_LABELS } from "../page-table-calculations";
import type { CalculationFunction } from "../page-table-calculations";
import { PageListField } from "../PageListField";
import { PageListFilters } from "../PageListFilters";
import { ChartWidgetSettings } from "./ChartWidget";
import { DEFAULT_CALC, embeddableViews } from "./dashboard-model";
import type { WidgetPatch } from "./dashboard-model";
import { MAX_WIDGET_TITLE_CHARS } from "./dashboard-types";
import type { DashboardWidget } from "./dashboard-types";

export interface WidgetSettingsProps {
  readonly widget: DashboardWidget;
  readonly schema: readonly PropertyDef[];
  /** Every view of the block (a view widget picks one that is not a dashboard). */
  readonly views: readonly PageListViewSpec[];
  readonly layoutName: (layout: PageListLayout) => string;
  /** The widget's automatic title, shown in the empty title field. */
  readonly automaticTitle: string;
  readonly anchor: Anchor;
  readonly onChange: (patch: WidgetPatch) => void;
  readonly onClose: () => void;
}

interface NumberSettingsProps {
  readonly calc: { readonly by: string; readonly fn: CalculationFunction };
  readonly schema: readonly PropertyDef[];
  readonly onChange: (patch: WidgetPatch) => void;
}

/** A number's column (the title or a property) and function (the ones the column's kind offers). */
function NumberSettings({ calc, schema, onChange }: NumberSettingsProps) {
  const def = calc.by === "title" ? null : (schema.find((candidate) => candidate.id === calc.by) ?? undefined);
  const offered = calculationFunctionsFor(def ?? null);
  return (
    <>
      <PageListField label={t("dashboard.settings.column")}>
        <select
          className="page-list-select"
          value={def === undefined ? "" : calc.by}
          onChange={(event) => {
            const by = event.target.value;
            const next = by === "title" ? null : schema.find((candidate) => candidate.id === by);
            if (by !== "title" && next === undefined) {
              return;
            }
            // The function stays when the new column offers it, else the count of pages (every column offers it).
            const fn = calculationFunctionsFor(next ?? null).includes(calc.fn) ? calc.fn : DEFAULT_CALC.fn;
            onChange({ calc: { by, fn } });
          }}
        >
          {def === undefined && (
            <option value="" disabled>
              {t("dashboard.settings.columnChoose")}
            </option>
          )}
          <option value="title">{t("pageList.sort.title")}</option>
          {schema.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </select>
      </PageListField>
      <PageListField label={t("dashboard.settings.function")}>
        <select
          className="page-list-select"
          value={calc.fn}
          onChange={(event) => {
            const fn = offered.find((candidate) => candidate === event.target.value);
            if (fn !== undefined) {
              onChange({ calc: { by: calc.by, fn } });
            }
          }}
        >
          {/* A function the column does not offer (its kind changed): listed, so the select says what is stored. */}
          {!offered.includes(calc.fn) && <option value={calc.fn}>{t(CALCULATION_LABELS[calc.fn])}</option>}
          {offered.map((fn) => (
            <option key={fn} value={fn}>
              {t(CALCULATION_LABELS[fn])}
            </option>
          ))}
        </select>
      </PageListField>
    </>
  );
}

interface FilterSettingProps {
  readonly widget: DashboardWidget;
  readonly schema: readonly PropertyDef[];
  /** Tells the panel that the filter editor is open (it ignores presses and Escape then). */
  readonly onOpenChange: (open: boolean) => void;
  readonly onChange: (patch: WidgetPatch) => void;
}

/** The widget's own filter: a button with the number of filters, opening the filter editor of the toolbar. */
function FilterSetting({ widget, schema, onOpenChange, onChange }: FilterSettingProps) {
  const [editor, setEditor] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const group = widget.filter ?? EMPTY_FILTER_GROUP;
  const count = countFilters(group);
  function open(next: { anchor: Anchor; trigger: Element } | null): void {
    onOpenChange(next !== null);
    setEditor(next);
  }
  return (
    <div className="page-list-field">
      <span className="page-list-field-label">{t("dashboard.settings.filter")}</span>
      <button
        type="button"
        className="page-list-select page-dashboard-filter-button"
        aria-haspopup="dialog"
        aria-expanded={editor !== null}
        onClick={(event) => {
          const trigger = event.currentTarget;
          open(editor === null ? { anchor: anchorBelow(trigger), trigger } : null);
        }}
      >
        <span>{t("dashboard.settings.filterEdit")}</span>
        {count > 0 && (
          <span className="page-list-badge" role="img" aria-label={tp("pageList.filter.badge", count)}>
            {String(count)}
          </span>
        )}
      </button>
      <span className="page-dashboard-hint">{t("dashboard.settings.filterHint")}</span>
      {editor !== null && (
        <PageListFilters
          anchor={editor.anchor}
          anchorElement={editor.trigger}
          defs={schema}
          group={group}
          onChange={(next) => {
            onChange({ filter: next });
          }}
          onClose={() => {
            open(null);
          }}
        />
      )}
    </div>
  );
}

/** The settings panel of a widget. */
export function WidgetSettings({ widget, schema, views, layoutName, automaticTitle, anchor, onChange, onClose }: WidgetSettingsProps) {
  const [title, setTitle] = useState(widget.title);
  const [seen, setSeen] = useState(widget.title);
  // What the document holds changed (an edit from elsewhere): the field follows it.
  if (seen !== widget.title) {
    setSeen(widget.title);
    setTitle(widget.title);
  }
  const filterOpen = useRef(false);

  function commitTitle(): void {
    if (title.trim() !== widget.title) {
      onChange({ title });
    }
  }

  function close(reason?: CloseReason): void {
    // The filter editor is a panel of its own: what closes it (a press outside, Escape) is not for this panel too.
    if (filterOpen.current) {
      return;
    }
    void reason;
    commitTitle();
    onClose();
  }

  const embeddable = embeddableViews(views);
  const shown = embeddable.find((view) => view.id === widget.viewId);
  return (
    <FloatingPanel anchor={anchor} label={t("dashboard.settings")} className="page-list-popover page-dashboard-settings" onClose={close}>
      <PageListField label={t("dashboard.settings.title")}>
        <input
          type="text"
          className="page-dashboard-input"
          value={title}
          placeholder={automaticTitle}
          maxLength={MAX_WIDGET_TITLE_CHARS}
          autoComplete="off"
          onChange={(event) => {
            setTitle(event.target.value);
          }}
          onBlur={commitTitle}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.nativeEvent.isComposing) {
              event.preventDefault();
              commitTitle();
            }
          }}
        />
      </PageListField>
      {widget.kind === "number" && <NumberSettings calc={widget.calc ?? DEFAULT_CALC} schema={schema} onChange={onChange} />}
      {widget.kind === "chart" && widget.chart !== undefined && (
        <ChartWidgetSettings
          chart={widget.chart}
          schema={schema}
          onChange={(chart) => {
            onChange({ chart });
          }}
        />
      )}
      {widget.kind === "view" &&
        (embeddable.length === 0 ? (
          <p className="page-dashboard-hint">{t("dashboard.settings.viewNone")}</p>
        ) : (
          <PageListField label={t("dashboard.settings.view")}>
            <select
              className="page-list-select"
              value={shown?.id ?? ""}
              onChange={(event) => {
                const chosen = embeddable.find((view) => view.id === event.target.value);
                if (chosen !== undefined) {
                  onChange({ viewId: chosen.id });
                }
              }}
            >
              {shown === undefined && (
                <option value="" disabled>
                  {t("dashboard.settings.viewChoose")}
                </option>
              )}
              {embeddable.map((view) => (
                <option key={view.id} value={view.id}>
                  {view.name.trim() === "" ? layoutName(view.layout) : view.name}
                </option>
              ))}
            </select>
          </PageListField>
        ))}
      {widget.kind !== "view" && (
        <FilterSetting
          widget={widget}
          schema={schema}
          onOpenChange={(open) => {
            filterOpen.current = open;
          }}
          onChange={onChange}
        />
      )}
    </FloatingPanel>
  );
}
