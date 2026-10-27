// The frame of a widget of a dashboard (ADR-0055, `docs/architecture/dashboards.md` section 3): a header with the title (and a
// badge while the widget has a filter of its own) and, in an editable editor, a "…" menu (Edit…, Width, Height, Duplicate, Move
// earlier, Move later, Delete); the header is the handle the widget is dragged by (the grid, `PageDashboardView`, runs the drag
// and draws the drop indicator this frame is told about). The body is the widget's own content, at the height the widget asks.
// A read-only editor gets the title and the content: no menu, no handle.

import { useState } from "react";
import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";

import { anchorBelow, FloatingPanel } from "../../components/FloatingPanel";
import type { Anchor } from "../../components/FloatingPanel";
import { MoreIcon } from "../../components/icons";
import { t, tp } from "../../i18n";
import { WIDGET_HEIGHTS, WIDGET_WIDTHS } from "./dashboard-model";
import { MAX_DASHBOARD_WIDGETS, WIDGET_HEIGHT_PX } from "./dashboard-types";
import type { DashboardWidgetKind, WidgetHeight, WidgetWidth } from "./dashboard-types";

/** What the "…" menu of a widget asks for. */
export type FrameCommand =
  | { readonly type: "edit" }
  | { readonly type: "duplicate" }
  | { readonly type: "earlier" }
  | { readonly type: "later" }
  | { readonly type: "delete" }
  | { readonly type: "width"; readonly w: WidgetWidth }
  | { readonly type: "height"; readonly h: WidgetHeight };

export interface DashboardWidgetFrameProps {
  /** Names the frame to the grid (the widget's id, or the place of a raw entry). */
  readonly entryKey: string;
  readonly title: string;
  /** `"raw"`: a widget this version cannot read (only Move and Delete apply to it). */
  readonly kind: DashboardWidgetKind | "raw";
  readonly w: WidgetWidth;
  readonly h: WidgetHeight;
  /** How many filters the widget has of its own (0: no badge). */
  readonly filterCount: number;
  readonly editable: boolean;
  readonly index: number;
  readonly count: number;
  /** False at the limit of widgets (Duplicate is disabled). */
  readonly canDuplicate: boolean;
  /** This widget is being dragged. */
  readonly dragging: boolean;
  /** The drop indicator: a bar before or after this frame. */
  readonly drop: "before" | "after" | null;
  /** Buttons for the end of the header, before the menu ("Open view"). */
  readonly actions?: ReactNode;
  readonly onCommand: (command: FrameCommand) => void;
  /** The header was pressed (primary button, not on a button): a drag may begin. Not called in a read-only editor. */
  readonly onDragStart: (event: ReactPointerEvent<HTMLElement>) => void;
  readonly children: ReactNode;
}

interface MenuProps {
  readonly anchor: Anchor;
  readonly trigger: Element;
  readonly title: string;
  readonly raw: boolean;
  readonly w: WidgetWidth;
  readonly h: WidgetHeight;
  readonly index: number;
  readonly count: number;
  readonly canDuplicate: boolean;
  readonly onCommand: (command: FrameCommand) => void;
  readonly onClose: () => void;
}

/** The name of a width in the menu: a quarter, a half, three quarters or the full width. */
const WIDTH_KEYS = {
  1: "dashboard.width.1",
  2: "dashboard.width.2",
  3: "dashboard.width.3",
  4: "dashboard.width.4",
} as const;

const HEIGHT_KEYS = {
  s: "dashboard.height.s",
  m: "dashboard.height.m",
  l: "dashboard.height.l",
} as const;

/** The menu of a widget: commands, and the choices of width and height as radio items. */
function WidgetMenu({ anchor, trigger, title, raw, w, h, index, count, canDuplicate, onCommand, onClose }: MenuProps) {
  const run = (command: FrameCommand) => () => {
    onClose();
    onCommand(command);
  };
  return (
    <FloatingPanel
      role="menu"
      label={t("dashboard.widget.menu", { title })}
      anchor={anchor}
      anchorElement={trigger}
      className="page-dashboard-menu"
      onClose={onClose}
    >
      {!raw && (
        <button type="button" role="menuitem" className="menu-item" onClick={run({ type: "edit" })}>
          {t("dashboard.widget.edit")}
        </button>
      )}
      {!raw && (
        <div role="group" aria-label={t("dashboard.widget.width")} className="page-dashboard-menu-group">
          <span className="page-dashboard-menu-label" aria-hidden="true">
            {t("dashboard.widget.width")}
          </span>
          <div className="page-dashboard-menu-choices">
            {WIDGET_WIDTHS.map((choice) => (
              <button
                key={choice}
                type="button"
                role="menuitemradio"
                aria-checked={choice === w}
                data-menu-nav=""
                className="page-dashboard-menu-choice"
                onClick={run({ type: "width", w: choice })}
              >
                {t(WIDTH_KEYS[choice])}
              </button>
            ))}
          </div>
        </div>
      )}
      {!raw && (
        <div role="group" aria-label={t("dashboard.widget.height")} className="page-dashboard-menu-group">
          <span className="page-dashboard-menu-label" aria-hidden="true">
            {t("dashboard.widget.height")}
          </span>
          <div className="page-dashboard-menu-choices">
            {WIDGET_HEIGHTS.map((choice) => (
              <button
                key={choice}
                type="button"
                role="menuitemradio"
                aria-checked={choice === h}
                data-menu-nav=""
                className="page-dashboard-menu-choice"
                onClick={run({ type: "height", h: choice })}
              >
                {t(HEIGHT_KEYS[choice])}
              </button>
            ))}
          </div>
        </div>
      )}
      {!raw && (
        <button
          type="button"
          role="menuitem"
          className="menu-item"
          disabled={!canDuplicate}
          title={canDuplicate ? undefined : t("dashboard.add.limit", { max: MAX_DASHBOARD_WIDGETS })}
          onClick={run({ type: "duplicate" })}
        >
          {t("dashboard.widget.duplicate")}
        </button>
      )}
      <button type="button" role="menuitem" className="menu-item" disabled={index === 0} onClick={run({ type: "earlier" })}>
        {t("dashboard.widget.moveEarlier")}
      </button>
      <button type="button" role="menuitem" className="menu-item" disabled={index >= count - 1} onClick={run({ type: "later" })}>
        {t("dashboard.widget.moveLater")}
      </button>
      <button type="button" role="menuitem" className="menu-item menu-item-danger" onClick={run({ type: "delete" })}>
        {t("dashboard.widget.delete")}
      </button>
    </FloatingPanel>
  );
}

/** The frame of one widget. */
export function DashboardWidgetFrame({
  entryKey,
  title,
  kind,
  w,
  h,
  filterCount,
  editable,
  index,
  count,
  canDuplicate,
  dragging,
  drop,
  actions,
  onCommand,
  onDragStart,
  children,
}: DashboardWidgetFrameProps) {
  const [menu, setMenu] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  return (
    <section
      className="page-dashboard-widget"
      role="group"
      aria-label={title}
      data-widget-id={entryKey}
      data-kind={kind}
      data-w={w}
      data-h={h}
      data-dragging={dragging ? "" : undefined}
      data-drop={drop ?? undefined}
      style={{ height: WIDGET_HEIGHT_PX[h] }}
    >
      <header
        className="page-dashboard-widget-header"
        data-draggable={editable ? "" : undefined}
        onPointerDown={(event) => {
          if (!editable || event.button !== 0 || (event.target instanceof Element && event.target.closest("button") !== null)) {
            return;
          }
          // The editor must not take the press for its own (selecting the block, starting a drag of it).
          event.preventDefault();
          onDragStart(event);
        }}
      >
        <span className="page-dashboard-widget-title" role="heading" aria-level={3} title={title}>
          {title}
        </span>
        {filterCount > 0 && (
          <span className="page-list-badge" role="img" aria-label={tp("dashboard.widget.filtered", filterCount)}>
            {String(filterCount)}
          </span>
        )}
        <span className="page-dashboard-widget-actions">
          {actions}
          {editable && (
            <button
              type="button"
              className="icon-button page-dashboard-widget-menu-button"
              aria-haspopup="menu"
              aria-expanded={menu !== null}
              aria-label={t("dashboard.widget.menu", { title })}
              title={t("dashboard.widget.menu", { title })}
              onClick={(event) => {
                const trigger = event.currentTarget;
                setMenu((current) => (current === null ? { anchor: anchorBelow(trigger), trigger } : null));
              }}
            >
              <MoreIcon />
            </button>
          )}
        </span>
      </header>
      <div className="page-dashboard-widget-body">{children}</div>
      {menu !== null && (
        <WidgetMenu
          anchor={menu.anchor}
          trigger={menu.trigger}
          title={title}
          raw={kind === "raw"}
          w={w}
          h={h}
          index={index}
          count={count}
          canDuplicate={canDuplicate}
          onCommand={onCommand}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </section>
  );
}
