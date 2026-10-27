// The toolbar of the list view (editable editors only): a slim row of four buttons, each opening a popover. Filter
// edits the view's filter tree (`PageListFilters.tsx`, AND and OR groups), Sort lists the view's sort keys (up to five: a
// property and a direction each, which can be added, removed and moved up or down, ADR-0058),
// Properties picks which properties each row shows, and the view settings button (sliders) holds the rest: the layout
// (list, table, board, calendar or gallery) and that layout's own settings (the registry's `Settings` section: the
// calendar's dates, the gallery's card size, the board's group-by and columns), where the pages come from (the chooser
// of `PageListSourceChooser.tsx`: this page's sub-pages, a tag, another page's sub-pages or the whole space), where a
// click on a row opens its page ("Open pages in", a setting of the view for every layout) and "Export CSV" (ADR-0036).
// A layout may have no use for Sort and Properties (the registry says so: a dashboard, ADR-0055): the bar then has Filter and the
// view settings only.
// Filter and Sort carry a badge in the accent colour while they change what the view shows (the number of filters; the
// number of sort keys when they are not just the default sort), so the state of the view is read from the bar without
// opening anything.
//
// The bar sits at the end of the row of the view tabs (`PageListViewTabs.tsx`) and wraps under them when there is no
// room; it never wraps itself, so it fits a 400 px column. It is shown while the pointer is over the block, the block
// is selected or focus is in it, and always while a badge says something (CSS).
//
// Each control changes one part of the active view through `onChange` (a patch of the view, plus the block's
// `source`), which the node view writes following the rule of `page-list-write.ts` (one attribute for a block that
// has no `views`), in one transaction, so the block keeps its `id` and the other settings.

import type { PageId, PropertyDef, PropertyId } from "@atomik-kb/core-client";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { PlusIcon, SlidersIcon } from "../components/icons";
import { ArrowDownIcon, ArrowUpIcon, CloseIcon } from "../components/property-icons";
import { t, tp } from "../i18n";
import type { MessageKey } from "../i18n";
import { countFilters } from "./page-list-filter-tree";
import { layoutDef, PAGE_LIST_LAYOUT_DEFS } from "./page-list-layouts";
import {
  BUILT_IN_SORTS,
  DEFAULT_PAGE_LIST_SORT,
  isKnownSort,
  MAX_SORTS,
  readLayout,
  SORT_BY_CREATED,
  SORT_BY_TITLE,
  SORT_BY_UPDATED,
} from "./page-list-model";
import type { PageListSort, PageListSource, SortDirection } from "./page-list-model";
import { DEFAULT_OPEN_IN, OPEN_IN_VALUES } from "./page-list-views";
import type { OpenIn, PageListViewPatch, PageListViewSpec } from "./page-list-views";
import { PageListField } from "./PageListField";
import { PageListFilters } from "./PageListFilters";
import { PageListSourceChooser } from "./PageListSourceChooser";

/** What a control changes: a part of the active view, or the block's `source` (shared by all its views). The rest is left alone. */
export interface PageListChange extends PageListViewPatch {
  source?: PageListSource;
}

/** The "Export CSV" entry of the view settings menu. */
export interface PageListCsvExport {
  /** Exports the rows the view shows. */
  onExport: () => void;
  /** Why there is nothing to export (shown on the disabled entry), or `null` when there is. */
  disabledReason: string | null;
}

export interface PageListToolbarProps {
  /** `null` for a source this version does not know. */
  source: PageListSource | null;
  /** The page that holds the block, left out of the pages the source chooser offers as another page (`null` or left out when unknown). */
  hostId?: PageId | null;
  /** The active view: its layout (an unknown one reads as the list), properties, sort, filter tree, board settings and `openIn`. */
  view: PageListViewSpec;
  /** The space's property schema, Tags first (empty while it loads). */
  defs: readonly PropertyDef[];
  onChange: (change: PageListChange) => void;
  /**
   * The active layout's own section (`layoutDef(layout).Settings`: the calendar's date source, the gallery's card
   * size, the board's group-by), rendered in the view settings menu after the layout and the source; `PageListView`
   * builds it, the toolbar only places it.
   */
  layoutSettings?: ReactNode;
  /** The CSV export; `null` (or left out) where the shell cannot save a file: the entry is not offered then. */
  csv?: PageListCsvExport | null;
}

const OPEN_IN_LABELS: Readonly<Record<OpenIn, MessageKey>> = {
  side: "pageList.openIn.side",
  center: "pageList.openIn.center",
  page: "pageList.openIn.page",
};

const SORT_LABELS: Readonly<Record<string, MessageKey>> = {
  [SORT_BY_TITLE]: "pageList.sort.title",
  [SORT_BY_UPDATED]: "pageList.sort.updated",
  [SORT_BY_CREATED]: "pageList.sort.created",
};

/** The width of the view settings popover (`.page-list-popover`), and its distance from the window's edge, in pixels. */
const SETTINGS_WIDTH = 260;
const SETTINGS_MARGIN = 8;

/** The popover that is open, from the button that opened it. */
type PanelKind = "filter" | "sort" | "properties" | "settings";

interface OpenPanel {
  readonly kind: PanelKind;
  readonly anchor: Anchor;
  readonly trigger: Element;
}

/** The number on a button (the filters, the sort): its accessible name is `label`, the digits are for the eye. */
function Badge({ label, children }: { label: string; children: ReactNode }) {
  return (
    <span className="page-list-badge" role="img" aria-label={label}>
      {children}
    </span>
  );
}

interface PanelProps {
  anchor: Anchor;
  anchorElement: Element;
  onClose: () => void;
}

interface PropertiesPanelProps extends PanelProps {
  defs: readonly PropertyDef[];
  shown: readonly PropertyId[];
  onToggle: (id: PropertyId, on: boolean) => void;
}

/** The properties popover: one checkbox per property of the space. It stays open while they are toggled. */
function PropertiesPanel({ anchor, anchorElement, defs, shown, onToggle, onClose }: PropertiesPanelProps) {
  return (
    <FloatingPanel
      anchor={anchor}
      anchorElement={anchorElement}
      label={t("pageList.properties.label")}
      className="page-list-props"
      onClose={onClose}
    >
      {defs.length === 0 ? (
        <p className="page-list-props-empty">{t("pageList.properties.none")}</p>
      ) : (
        defs.map((def) => (
          <label key={def.id} className="page-list-prop">
            <input
              type="checkbox"
              checked={shown.includes(def.id)}
              onChange={(event) => {
                onToggle(def.id, event.target.checked);
              }}
            />
            <span>{def.name}</span>
          </label>
        ))
      )}
    </FloatingPanel>
  );
}

interface SortPanelProps extends PanelProps {
  /** The properties the picker lists besides the built-in sorts. */
  sortable: readonly PropertyDef[];
  /** The sort keys as the view stores them (a property the space no longer has shows as removed). */
  sorts: readonly PageListSort[];
  onChange: (sorts: readonly PageListSort[]) => void;
}

/** The sort keys of a list, as the panel offers them: the title, the last edit, the creation, then the properties. */
function sortChoices(sortable: readonly PropertyDef[]): readonly { by: string; label: string }[] {
  return [
    ...BUILT_IN_SORTS.map((by) => ({ by, label: t(SORT_LABELS[by] ?? "pageList.sort.title") })),
    ...sortable.map((def) => ({ by: def.id, label: def.name })),
  ];
}

/**
 * The sort popover: one row per key (the property or title, last edit, creation; the direction; up, down and remove), and
 * "Add a sort" (at most 5 keys). A key is offered once: a row lists what the other rows do not use. It stays open while
 * the keys change. Removing the last key goes back to the default sort.
 */
function SortPanel({ anchor, anchorElement, sortable, sorts, onChange, onClose }: SortPanelProps) {
  const choices = sortChoices(sortable);
  const unused = choices.filter((choice) => !sorts.some((sort) => sort.by === choice.by));
  const isDefault = sorts.length === 1 && sorts[0]?.by === DEFAULT_PAGE_LIST_SORT.by && sorts[0].dir === DEFAULT_PAGE_LIST_SORT.dir;
  const full = sorts.length >= MAX_SORTS;
  const next = unused[0];
  // A key that moved keeps the focus on the button that moved it: the row it went to, the same control (the other one
  // when that one is disabled at an end of the list).
  const rows = useRef<(HTMLDivElement | null)[]>([]);
  const refocus = useRef<{ row: number; control: "up" | "down" } | null>(null);
  useEffect(() => {
    const target = refocus.current;
    refocus.current = null;
    const row = target === null ? null : rows.current[target.row];
    if (target !== null && row !== null && row !== undefined) {
      const same = row.querySelector<HTMLButtonElement>(`[data-control="${target.control}"]`);
      const other = row.querySelector<HTMLButtonElement>(`[data-control="${target.control === "up" ? "down" : "up"}"]`);
      (same?.disabled === false ? same : other)?.focus();
    }
  });
  return (
    <FloatingPanel
      anchor={anchor}
      anchorElement={anchorElement}
      label={t("pageList.sort")}
      className="page-list-popover page-list-sorts"
      onClose={onClose}
    >
      {sorts.map((sort, at) => {
        const known = isKnownSort(sort.by, sortable);
        const move = (to: number, control: "up" | "down"): void => {
          const moved = [...sorts];
          const [entry] = moved.splice(at, 1);
          if (entry !== undefined) {
            moved.splice(to, 0, entry);
            refocus.current = { row: to, control };
            onChange(moved);
          }
        };
        return (
          <div
            // A key has no id of its own (and changes when its property is chosen): its place in the list is its identity.
            key={at}
            ref={(element) => {
              rows.current[at] = element;
            }}
            className="page-sort-row"
            role="group"
            aria-label={t("pageList.sort.row", { n: at + 1 })}
          >
            <select
              className="page-list-select"
              aria-label={t("pageList.sort.label")}
              value={sort.by}
              onChange={(event) => {
                onChange(sorts.map((entry, index) => (index === at ? { by: event.target.value, dir: entry.dir } : entry)));
              }}
            >
              {!known && <option value={sort.by}>{t("pageList.sort.gone")}</option>}
              {choices
                .filter((choice) => choice.by === sort.by || !sorts.some((other) => other.by === choice.by))
                .map((choice) => (
                  <option key={choice.by} value={choice.by}>
                    {choice.label}
                  </option>
                ))}
            </select>
            <select
              className="page-list-select"
              aria-label={t("pageList.sort.dir")}
              value={sort.dir}
              onChange={(event) => {
                const dir: SortDirection = event.target.value === "desc" ? "desc" : "asc";
                onChange(sorts.map((entry, index) => (index === at ? { by: entry.by, dir } : entry)));
              }}
            >
              <option value="asc">{t("pageList.sort.asc")}</option>
              <option value="desc">{t("pageList.sort.desc")}</option>
            </select>
            <button
              type="button"
              className="icon-button"
              aria-label={t("pageList.sort.up")}
              title={t("pageList.sort.up")}
              data-control="up"
              disabled={at === 0}
              onClick={() => {
                move(at - 1, "up");
              }}
            >
              <ArrowUpIcon />
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label={t("pageList.sort.down")}
              title={t("pageList.sort.down")}
              data-control="down"
              disabled={at === sorts.length - 1}
              onClick={() => {
                move(at + 1, "down");
              }}
            >
              <ArrowDownIcon />
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label={t("pageList.sort.remove")}
              title={t("pageList.sort.remove")}
              disabled={sorts.length === 1 && isDefault}
              onClick={() => {
                onChange(sorts.length === 1 ? [DEFAULT_PAGE_LIST_SORT] : sorts.filter((_, index) => index !== at));
              }}
            >
              <CloseIcon />
            </button>
          </div>
        );
      })}
      <button
        type="button"
        className="page-filter-add"
        disabled={full || next === undefined}
        onClick={() => {
          if (next !== undefined) {
            onChange([...sorts, { by: next.by, dir: "asc" }]);
          }
        }}
      >
        <PlusIcon />
        <span>{t("pageList.sort.add")}</span>
      </button>
      {full && (
        <p className="page-list-props-empty" role="status">
          {t("pageList.sort.limit", { max: MAX_SORTS })}
        </p>
      )}
    </FloatingPanel>
  );
}

interface SettingsPanelProps extends PanelProps {
  source: PageListSource | null;
  /** The page that holds the block (`null` when unknown), left out of the pages "Sub-pages of another page" offers. */
  hostId: PageId | null;
  view: PageListViewSpec;
  defs: readonly PropertyDef[];
  onChange: (change: PageListChange) => void;
  layoutSettings: ReactNode;
  csv: PageListCsvExport | null;
}

/**
 * The view settings popover: the layout, where the pages come from, the layout's own section, where a click on a row
 * opens its page and the CSV export. Native selects (named by their visible labels); it stays open while they change.
 */
function SettingsPanel({ anchor, anchorElement, source, hostId, view, defs, onChange, layoutSettings, csv, onClose }: SettingsPanelProps) {
  // The page picker of "Sub-pages of another page…" is a popover over this one: a press inside it must not close this one.
  const picking = useRef(false);
  return (
    <FloatingPanel
      anchor={anchor}
      anchorElement={anchorElement}
      label={t("pageList.viewSettings")}
      className="page-list-popover page-list-settings"
      onClose={() => {
        if (!picking.current) {
          onClose();
        }
      }}
    >
      <PageListField label={t("pageList.layout.label")}>
        <select
          className="page-list-select"
          value={view.layout}
          onChange={(event) => {
            onChange({ layout: readLayout(event.target.value) });
          }}
        >
          {PAGE_LIST_LAYOUT_DEFS.map((def) => (
            <option key={def.layout} value={def.layout}>
              {t(def.labelKey)}
            </option>
          ))}
        </select>
      </PageListField>
      <PageListSourceChooser
        source={source}
        hostId={hostId}
        defs={defs}
        onChange={(next) => {
          onChange({ source: next });
        }}
        onPickerOpen={(open) => {
          picking.current = open;
        }}
      />
      {layoutSettings !== null && (
        <div role="group" aria-label={t("pageList.viewSettings.layout")} className="page-list-settings-section">
          <p className="page-list-settings-title">{t("pageList.viewSettings.layout")}</p>
          {layoutSettings}
        </div>
      )}
      <PageListField label={t("pageList.openIn.label")}>
        <select
          className="page-list-select"
          value={view.openIn ?? DEFAULT_OPEN_IN}
          onChange={(event) => {
            const next = OPEN_IN_VALUES.find((value) => value === event.target.value);
            if (next !== undefined) {
              onChange({ openIn: next });
            }
          }}
        >
          {OPEN_IN_VALUES.map((value) => (
            <option key={value} value={value}>
              {t(OPEN_IN_LABELS[value])}
            </option>
          ))}
        </select>
      </PageListField>
      {csv !== null && (
        <div role="group" aria-label={t("pageList.actions")} className="page-list-settings-actions">
          <button
            type="button"
            className="menu-item"
            disabled={csv.disabledReason !== null}
            title={csv.disabledReason ?? undefined}
            onClick={() => {
              onClose();
              csv.onExport();
            }}
          >
            {t("pageList.exportCsv")}
          </button>
        </div>
      )}
    </FloatingPanel>
  );
}

export function PageListToolbar({ source, hostId = null, view, defs, onChange, layoutSettings = null, csv = null }: PageListToolbarProps) {
  const { properties, sorts } = view;
  const filterCount = countFilters(view.filter);
  const [open, setOpen] = useState<OpenPanel | null>(null);
  const shown = properties.flatMap((id) => defs.find((def) => def.id === id) ?? []);
  // The sort picker lists the properties the rows show, and the ones the view sorts by when they are not shown
  // (so that the picker tells what the view does).
  const sortable = [...shown, ...sorts.flatMap((sort) => defs.find((def) => def.id === sort.by && !shown.includes(def)) ?? [])];
  // A sort by a property the space no longer has sorts like the title, and says so.
  const effective = sorts.map((sort) => (isKnownSort(sort.by, defs) ? sort : { by: SORT_BY_TITLE, dir: sort.dir }));
  // The default sort (by title, ascending) is how every view starts: the badge says there is another, or more.
  const [firstSort] = effective;
  const sortSet = effective.length > 1 || firstSort?.by !== DEFAULT_PAGE_LIST_SORT.by || firstSort.dir !== DEFAULT_PAGE_LIST_SORT.dir;
  const sortName = effective
    .map((sort) => defs.find((def) => def.id === sort.by)?.name ?? t(SORT_LABELS[sort.by] ?? "pageList.sort.title"))
    .join(", ");
  // A layout may have no use for Sort and Properties (a dashboard, ADR-0055: the registry says so).
  const { toolbar: layoutToolbar } = layoutDef(view.layout);
  const sortShown = layoutToolbar?.sort !== false;
  const propertiesShown = layoutToolbar?.properties !== false;

  /** The button that opens (or, pressed again, closes) the popover `kind`. */
  function toggle(kind: PanelKind) {
    return (event: { currentTarget: Element }): void => {
      const opener = event.currentTarget;
      setOpen((current) => {
        if (current?.kind === kind) {
          return null;
        }
        // The settings button is the last one of the bar: its popover grows to the left, under it.
        const anchor =
          kind === "settings"
            ? { x: Math.max(SETTINGS_MARGIN, opener.getBoundingClientRect().right - SETTINGS_WIDTH), y: anchorBelow(opener).y }
            : anchorBelow(opener);
        return { kind, anchor, trigger: opener };
      });
    };
  }

  function close(): void {
    setOpen(null);
  }

  return (
    <div role="toolbar" aria-label={t("pageList.toolbar")} className="page-list-toolbar" data-badged={filterCount > 0 || (sortShown && sortSet) ? "" : undefined}>
      <button
        type="button"
        className="page-list-button"
        aria-haspopup="dialog"
        aria-expanded={open?.kind === "filter"}
        onClick={toggle("filter")}
      >
        {t("pageList.filter")}
        {filterCount > 0 && (
          <>
            {" "}
            <Badge label={tp("pageList.filter.badge", filterCount)}>{String(filterCount)}</Badge>
          </>
        )}
      </button>
      {sortShown && (
        <button
          type="button"
          className="page-list-button"
          aria-haspopup="dialog"
          aria-expanded={open?.kind === "sort"}
          onClick={toggle("sort")}
        >
          {t("pageList.sort")}
          {sortSet && (
            <>
              {" "}
              <Badge label={t("pageList.sort.badge", { name: sortName })}>{String(effective.length)}</Badge>
            </>
          )}
        </button>
      )}
      {propertiesShown && (
        <button
          type="button"
          className="page-list-button"
          aria-haspopup="dialog"
          aria-expanded={open?.kind === "properties"}
          onClick={toggle("properties")}
        >
          {t("pageList.properties")}
          {shown.length > 0 && <span className="page-list-count">{` ${String(shown.length)}`}</span>}
        </button>
      )}
      <button
        type="button"
        className="page-list-button page-list-settings-button"
        aria-haspopup="dialog"
        aria-expanded={open?.kind === "settings"}
        aria-label={t("pageList.viewSettings")}
        title={t("pageList.viewSettings")}
        onClick={toggle("settings")}
      >
        <SlidersIcon />
      </button>
      {open?.kind === "filter" && (
        <PageListFilters
          key="filter"
          anchor={open.anchor}
          anchorElement={open.trigger}
          defs={defs}
          group={view.filter}
          onChange={(next) => {
            onChange({ filter: next });
          }}
          onClose={close}
        />
      )}
      {open?.kind === "sort" && sortShown && (
        <SortPanel
          key="sort"
          anchor={open.anchor}
          anchorElement={open.trigger}
          sortable={sortable}
          sorts={sorts}
          onChange={(next) => {
            onChange({ sorts: next });
          }}
          onClose={close}
        />
      )}
      {open?.kind === "properties" && propertiesShown && (
        <PropertiesPanel
          key="properties"
          anchor={open.anchor}
          anchorElement={open.trigger}
          defs={defs}
          shown={properties}
          onToggle={(id, on) => {
            const others = properties.filter((other) => other !== id);
            onChange({ properties: on ? [...others, id] : others });
          }}
          onClose={close}
        />
      )}
      {open?.kind === "settings" && (
        <SettingsPanel
          key="settings"
          anchor={open.anchor}
          anchorElement={open.trigger}
          source={source}
          hostId={hostId}
          view={view}
          defs={defs}
          onChange={onChange}
          layoutSettings={layoutSettings}
          csv={csv}
          onClose={close}
        />
      )}
    </div>
  );
}
