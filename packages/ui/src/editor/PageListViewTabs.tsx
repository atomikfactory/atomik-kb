// The tab strip of a database block's views (ADR-0054, `docs/architecture/database-views.md` section 7): one tab
// per view with its layout's icon and its name, a "+" that adds a view (the person picks a layout), and a menu per
// tab (rename, duplicate, change layout, move left, move right, delete: not the last view). At most 20 views.
//
// It is a tab list (`role="tablist"`): Left, Right, Home and End move between the tabs (one tab stop, the active
// tab unless another was focused), Enter or Space show the focused one, F2 renames it, and the menu key or
// Shift+F10 opens its menu, as the page tabs of ADR-0034 do. Each tab also has a menu button, and a right click opens
// the menu too.
//
// Which view is active is the device's choice (`store-views.ts`, never synced), so the tabs switch in a read-only
// editor too; every other control appears only when `editable`.

import { useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, ReactNode } from "react";

import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { MoreIcon, PlusIcon } from "../components/icons";
import { Menu } from "../components/Menu";
import type { MenuItem } from "../components/Menu";
import { t } from "../i18n";
import type { PageListLayout } from "./page-list-model";
import { PAGE_LIST_LAYOUT_DEFS, layoutDef } from "./page-list-layouts";
import { canAddView, MAX_VIEW_NAME_CHARS, MAX_VIEWS, viewLabel } from "./page-list-views";
import type { PageListViewSpec } from "./page-list-views";

export interface PageListViewTabsProps {
  readonly views: readonly PageListViewSpec[];
  /** The id of the view shown (`resolveActiveView`). */
  readonly activeId: string;
  /** False in a read-only editor: tabs switch, nothing else is offered. */
  readonly editable: boolean;
  /** Shows a view (the device's choice: `setActiveView`, not a write to the document). */
  readonly onSelect: (id: string) => void;
  /** Adds a view of `layout` and shows it. */
  readonly onAdd: (layout: PageListLayout) => void;
  /** An empty name means the layout's own. */
  readonly onRename: (id: string, name: string) => void;
  readonly onDuplicate: (id: string) => void;
  readonly onChangeLayout: (id: string, layout: PageListLayout) => void;
  readonly onMove: (id: string, delta: -1 | 1) => void;
  /** Never called for the last view. */
  readonly onDelete: (id: string) => void;
  /**
   * What goes at the end of the row (the toolbar of the active view): on the same line as the tabs while there is room,
   * under them (still at the end) when there is not.
   */
  readonly trailing?: ReactNode;
}

/** The menu that is open: the "+" menu of layouts, a tab's menu, or the layouts to change a tab to. */
type OpenMenu =
  | { readonly kind: "add"; readonly anchor: Anchor; readonly trigger: Element | null }
  | { readonly kind: "tab"; readonly viewId: string; readonly anchor: Anchor; readonly trigger: Element | null }
  | { readonly kind: "layout"; readonly viewId: string; readonly anchor: Anchor; readonly trigger: Element | null };

/** The name being edited in place. */
interface Renaming {
  readonly viewId: string;
  readonly draft: string;
}

/** The tab the roving tab stop (the one tab Tab reaches) was last given to, for the view that was active then. */
interface Roving {
  readonly whileActive: string;
  readonly viewId: string;
}

const layoutName = (layout: PageListLayout): string => t(layoutDef(layout).labelKey);

/** The tab strip. */
export function PageListViewTabs({
  views,
  activeId,
  editable,
  onSelect,
  onAdd,
  onRename,
  onDuplicate,
  onChangeLayout,
  onMove,
  onDelete,
  trailing = null,
}: PageListViewTabsProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const [renaming, setRenamingState] = useState<Renaming | null>(null);
  // The name being edited, as the handlers see it right now (a blur that follows an Enter must not write it twice).
  const live = useRef<Renaming | null>(null);
  const [roving, setRoving] = useState<Roving | null>(null);
  // Set when a rename ends: the view whose tab takes focus back.
  const refocus = useRef<string | null>(null);

  const ids = views.map((view) => view.id);
  const fits = canAddView(views);
  // One tab stop: the tab focus was last on while the same view was active, else the active tab.
  const stopId = roving !== null && roving.whileActive === activeId && ids.includes(roving.viewId) ? roving.viewId : activeId;

  function tabs(): HTMLElement[] {
    return Array.from(listRef.current?.querySelectorAll<HTMLElement>('[role="tab"]') ?? []);
  }

  // Focus goes back to the tab whose name was edited, once the field is gone.
  useLayoutEffect(() => {
    const target = refocus.current;
    if (target === null || renaming !== null) {
      return;
    }
    refocus.current = null;
    tabs()
      .find((tab) => tab.dataset.viewId === target)
      ?.focus();
  });

  function setRenaming(next: Renaming | null): void {
    live.current = next;
    setRenamingState(next);
  }

  function startRename(viewId: string): void {
    const view = views.find((candidate) => candidate.id === viewId);
    if (view !== undefined) {
      setRenaming({ viewId, draft: view.name });
    }
  }

  /** Ends the rename: `keep` writes the name (an empty one restores the layout's), else the old one stays. */
  function endRename(keep: boolean): void {
    if (live.current === null) {
      return;
    }
    const { viewId, draft } = live.current;
    setRenaming(null);
    refocus.current = viewId;
    const view = views.find((candidate) => candidate.id === viewId);
    if (keep && view !== undefined && draft.trim() !== view.name) {
      onRename(viewId, draft.trim());
    }
  }

  function openMenuAt(viewId: string, event: MouseEvent<HTMLElement>): void {
    if (!editable) {
      return;
    }
    event.preventDefault();
    const keyboard = event.clientX === 0 && event.clientY === 0; // the context menu key: no pointer position
    const tab = event.currentTarget.querySelector('[role="tab"]');
    setMenu({
      kind: "tab",
      viewId,
      anchor: keyboard && tab !== null ? anchorBelow(tab) : { x: event.clientX, y: event.clientY },
      trigger: tab,
    });
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const tab = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>('[role="tab"]') : null;
    const viewId = tab?.dataset.viewId;
    if (tab === null || viewId === undefined || event.ctrlKey || event.metaKey || event.altKey) {
      return;
    }
    if (editable && (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey))) {
      event.preventDefault();
      setMenu({ kind: "tab", viewId, anchor: anchorBelow(tab), trigger: tab });
      return;
    }
    if (event.shiftKey) {
      return;
    }
    if (editable && event.key === "F2") {
      event.preventDefault();
      startRename(viewId);
      return;
    }
    const buttons = tabs();
    const index = buttons.indexOf(tab);
    let next: HTMLElement | undefined;
    switch (event.key) {
      case "ArrowRight":
        next = buttons[(index + 1) % buttons.length];
        break;
      case "ArrowLeft":
        next = buttons[(index - 1 + buttons.length) % buttons.length];
        break;
      case "Home":
        next = buttons[0];
        break;
      case "End":
        next = buttons[buttons.length - 1];
        break;
      default:
        return;
    }
    event.preventDefault();
    next?.focus();
  }

  // ----- the menus ---------------------------------------------------------------------------

  function tabMenuItems(view: PageListViewSpec): MenuItem[] {
    const index = ids.indexOf(view.id);
    return [
      { id: "rename", label: t("pageList.view.rename") },
      {
        id: "duplicate",
        label: t("pageList.view.duplicate"),
        disabled: !fits,
        title: fits ? undefined : t("pageList.views.limit", { max: MAX_VIEWS }),
      },
      { id: "changeLayout", label: t("pageList.view.changeLayout") },
      { id: "moveLeft", label: t("pageList.view.moveLeft"), disabled: index <= 0 },
      { id: "moveRight", label: t("pageList.view.moveRight"), disabled: index < 0 || index >= views.length - 1 },
      {
        id: "delete",
        label: t("pageList.view.delete"),
        danger: true,
        disabled: views.length <= 1,
        title: views.length <= 1 ? t("pageList.view.deleteLast") : undefined,
      },
    ];
  }

  function runTabCommand(current: OpenMenu & { kind: "tab" }, command: string): void {
    switch (command) {
      case "rename":
        // The menu returns focus to its opener as it closes; the field takes it a moment later.
        startRename(current.viewId);
        break;
      case "duplicate":
        onDuplicate(current.viewId);
        break;
      case "changeLayout":
        setMenu({ kind: "layout", viewId: current.viewId, anchor: current.anchor, trigger: current.trigger });
        break;
      case "moveLeft":
        onMove(current.viewId, -1);
        break;
      case "moveRight":
        onMove(current.viewId, 1);
        break;
      case "delete":
        onDelete(current.viewId);
        break;
      default:
    }
  }

  const layoutItems = (disabledLayout: PageListLayout | null): MenuItem[] =>
    PAGE_LIST_LAYOUT_DEFS.map((def) => ({ id: def.layout, label: t(def.labelKey), disabled: def.layout === disabledLayout }));

  const menuView = menu !== null && menu.kind !== "add" ? views.find((view) => view.id === menu.viewId) : undefined;

  function renderTab(view: PageListViewSpec) {
    const active = view.id === activeId;
    const label = viewLabel(view, layoutName);
    const { Icon } = layoutDef(view.layout);
    const editing = renaming?.viewId === view.id;
    return (
      <div
        key={view.id}
        className="page-list-tab-item"
        role="presentation"
        data-active={active ? "" : undefined}
        data-view-id={view.id}
        onContextMenu={(event) => {
          openMenuAt(view.id, event);
        }}
      >
        {editing ? (
          <input
            type="text"
            className="page-list-tab-input"
            aria-label={t("pageList.view.renameLabel")}
            maxLength={MAX_VIEW_NAME_CHARS}
            autoComplete="off"
            value={renaming.draft}
            placeholder={layoutName(view.layout)}
            ref={(element) => {
              // Focused and selected when it appears (a ref callback runs once per mount).
              if (element !== null && document.activeElement !== element) {
                element.focus();
                element.select();
              }
            }}
            onChange={(event) => {
              setRenaming({ viewId: view.id, draft: event.target.value });
            }}
            onBlur={() => {
              endRename(true);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                event.preventDefault();
                endRename(true);
              } else if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                endRename(false);
              }
            }}
          />
        ) : (
          <button
            type="button"
            role="tab"
            className="page-list-tab"
            data-view-id={view.id}
            aria-selected={active}
            title={label}
            tabIndex={view.id === stopId ? 0 : -1}
            onFocus={() => {
              setRoving((current) =>
                current?.viewId === view.id && current.whileActive === activeId ? current : { whileActive: activeId, viewId: view.id },
              );
            }}
            onClick={() => {
              onSelect(view.id);
            }}
            onDoubleClick={
              editable
                ? () => {
                    startRename(view.id);
                  }
                : undefined
            }
          >
            <Icon />
            <span className="page-list-tab-name">{label}</span>
          </button>
        )}
        {editable && !editing && (
          <button
            type="button"
            className="page-list-tab-menu"
            tabIndex={-1}
            aria-haspopup="menu"
            aria-label={t("pageList.view.menu", { name: label })}
            title={t("pageList.view.menu", { name: label })}
            onClick={(event) => {
              const opener = event.currentTarget;
              setMenu((current) =>
                current !== null && current.kind === "tab" && current.viewId === view.id
                  ? null
                  : { kind: "tab", viewId: view.id, anchor: anchorBelow(opener), trigger: opener },
              );
            }}
          >
            <MoreIcon />
          </button>
        )}
      </div>
    );
  }

  return (
    <div className="page-list-tabs-row">
      <div className="page-list-tabs-group">
        <div ref={listRef} className="page-list-tabs" role="tablist" aria-label={t("pageList.views.label")} onKeyDown={onKeyDown}>
          {views.map((view) => renderTab(view))}
        </div>
        {editable && (
          <button
            type="button"
            className="page-list-tab-add"
            aria-haspopup="menu"
            aria-expanded={menu?.kind === "add"}
            aria-label={t("pageList.views.add")}
            title={fits ? t("pageList.views.add") : t("pageList.views.limit", { max: MAX_VIEWS })}
            disabled={!fits}
            onClick={(event) => {
              const opener = event.currentTarget;
              setMenu((current) => (current?.kind === "add" ? null : { kind: "add", anchor: anchorBelow(opener), trigger: opener }));
            }}
          >
            <PlusIcon />
          </button>
        )}
        {editable && !fits && (
          <span className="page-list-tabs-limit" role="status">
            {t("pageList.views.limit", { max: MAX_VIEWS })}
          </span>
        )}
      </div>
      {trailing}
      {menu?.kind === "add" && (
        <Menu
          label={t("pageList.views.addMenu")}
          items={layoutItems(null)}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(id) => {
            const def = PAGE_LIST_LAYOUT_DEFS.find((candidate) => candidate.layout === id);
            if (def !== undefined) {
              onAdd(def.layout);
            }
          }}
          onClose={() => {
            setMenu((current) => (current?.kind === "add" ? null : current));
          }}
        />
      )}
      {menu?.kind === "tab" && menuView !== undefined && (
        <Menu
          label={t("pageList.view.menu", { name: viewLabel(menuView, layoutName) })}
          items={tabMenuItems(menuView)}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(command) => {
            runTabCommand(menu, command);
          }}
          onClose={() => {
            setMenu((current) => (current?.kind === "tab" ? null : current));
          }}
        />
      )}
      {menu?.kind === "layout" && menuView !== undefined && (
        <Menu
          label={t("pageList.view.changeLayout")}
          items={layoutItems(menuView.layout)}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(id) => {
            const def = PAGE_LIST_LAYOUT_DEFS.find((candidate) => candidate.layout === id);
            if (def !== undefined) {
              onChangeLayout(menu.viewId, def.layout);
            }
          }}
          onClose={() => {
            setMenu((current) => (current?.kind === "layout" ? null : current));
          }}
        />
      )}
    </div>
  );
}
