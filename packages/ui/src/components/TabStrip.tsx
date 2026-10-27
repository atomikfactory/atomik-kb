import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { DragEvent, KeyboardEvent, MouseEvent } from "react";

import { t } from "../i18n";
import { BACK_SHORTCUT, FORWARD_SHORTCUT, MODIFIER } from "../shortcuts";
import { useAppStore } from "../store-context";
import { MAX_TABS, selectCanGoBack, selectCanGoForward } from "../store-tabs";
import type { Tab, TabId } from "../store-tabs";
import { useTabLabels } from "../tab-labels";
import type { TabLabel } from "../tab-labels";
import { anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { BackIcon, CloseIcon, ForwardIcon, HomeIcon, PlusIcon } from "./icons";
import { Menu } from "./Menu";
import type { MenuItem } from "./Menu";
import { PageIcon } from "./PageIcon";

/** Marks a drag that carries a tab of this strip (a drag from elsewhere does not). */
const DRAG_TYPE = "application/x-atomik-tab";

/** The padding of `.tab-list` (`styles/app.css`), room for the active tab's outline. */
const LIST_PADDING = 2;

/** The id of the area that shows the active tab, for the tabs' `aria-controls`. */
export const TAB_PANEL_ID = "tab-panel";

/** The DOM id of a tab, for the panel's `aria-labelledby`. */
export function tabElementId(tabId: TabId): string {
  return `tab-${tabId.replace(/[^A-Za-z0-9_-]/g, "_")}`;
}

type Zone = "before" | "after";

interface OpenMenu {
  tabId: TabId;
  anchor: Anchor;
}

interface DropHint {
  target: TabId;
  zone: Zone;
}

/** The tab the roving tab stop (the one tab Tab reaches) was last given to, for the tab that was active then. */
interface Roving {
  whileActive: TabId | null;
  tabId: TabId;
}

/**
 * The strip above the main area (ADR-0034 section 4), outside the scrolling content and shown on
 * every screen: Back and Forward (in the active tab's history), the tabs and a "New tab" button.
 *
 * A tab shows its page's icon and title (Home for the Home of a space) with a close button; a
 * pinned tab is its icon alone, named and given a tooltip by the title, and cannot be closed by
 * the button. Pressing a tab brings it to the front, also from Settings or Diagnostics; a middle
 * click closes it. Tabs shrink to a minimum width and then the strip scrolls sideways (the wheel
 * too), keeping the active tab in view. A tab can be dragged to a new place inside its group
 * (pinned or not), and has a menu (right click, the context menu key or Shift+F10): Pin or Unpin,
 * Close, Close other tabs, Close tabs to the right and Reopen closed tab. Double-clicking empty
 * space in the strip opens a new tab.
 *
 * It is a tab list: Left, Right, Home and End move between the tabs (one tab stop, the active tab
 * unless another was focused), Enter or Space bring the focused one to the front and Delete closes
 * it. The close buttons are not tab stops.
 */
export function TabStrip() {
  const tabs = useAppStore((state) => state.tabs);
  const activeTabId = useAppStore((state) => state.activeTabId);
  const hasClosedTabs = useAppStore((state) => state.closedTabs.length > 0);
  const canGoBack = useAppStore(selectCanGoBack);
  const canGoForward = useAppStore(selectCanGoForward);
  const route = useAppStore((state) => state.route);
  const navigate = useAppStore((state) => state.navigate);
  const newTab = useAppStore((state) => state.newTab);
  const activateTab = useAppStore((state) => state.activateTab);
  const closeTab = useAppStore((state) => state.closeTab);
  const closeOtherTabs = useAppStore((state) => state.closeOtherTabs);
  const closeTabsToRight = useAppStore((state) => state.closeTabsToRight);
  const reopenClosedTab = useAppStore((state) => state.reopenClosedTab);
  const moveTab = useAppStore((state) => state.moveTab);
  const setTabPinned = useAppStore((state) => state.setTabPinned);
  const goBack = useAppStore((state) => state.goBack);
  const goForward = useAppStore((state) => state.goForward);
  const labels = useTabLabels(tabs);

  const listRef = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const [roving, setRoving] = useState<Roving | null>(null);
  const [dragId, setDragId] = useState<TabId | null>(null);
  const [hint, setHint] = useState<DropHint | null>(null);
  // Set when the tab that has focus closes: where focus goes (a tab, or `null` for the active one).
  const refocus = useRef<{ tabId: TabId | null } | null>(null);

  const ids = tabs.map((tab) => tab.id);
  const hasTabs = tabs.length > 0;
  // One tab stop: the tab focus was last on while the same tab was active, else the active tab.
  const stopId =
    roving !== null && roving.whileActive === activeTabId && ids.includes(roving.tabId)
      ? roving.tabId
      : (activeTabId ?? ids[0] ?? null);

  /** Pressing something in the strip that changes what is in front shows the main screen. */
  function showMain(): void {
    if (route !== "home") {
      navigate("home");
    }
  }

  function openNewTab(): void {
    newTab();
    showMain();
  }

  function tabButtons(): HTMLElement[] {
    return Array.from(listRef.current?.querySelectorAll<HTMLElement>('[role="tab"]') ?? []);
  }

  function focusTab(tabId: TabId | null): void {
    const wanted = tabId ?? activeTabId;
    tabButtons()
      .find((button) => button.dataset.tabId === wanted)
      ?.focus();
  }

  /** Closes a tab; when focus is in the strip, it goes to the tab that takes the place of the closed one. */
  function close(tabId: TabId): void {
    const index = ids.indexOf(tabId);
    if (listRef.current?.contains(document.activeElement) === true || menu !== null) {
      refocus.current = { tabId: ids[index + 1] ?? ids[index - 1] ?? null };
    }
    closeTab(tabId);
  }

  // Focus does not get lost with the tab that had it.
  useLayoutEffect(() => {
    const target = refocus.current;
    if (target === null) {
      return;
    }
    refocus.current = null;
    const focus = document.activeElement;
    if (focus === null || focus === document.body || listRef.current?.contains(focus) !== true) {
      focusTab(target.tabId !== null && ids.includes(target.tabId) ? target.tabId : null);
    }
  });

  // The active tab stays in view when the strip scrolls.
  useLayoutEffect(() => {
    const list = listRef.current;
    const active = list?.querySelector<HTMLElement>(".tab.is-active");
    if (list === null || active === null || active === undefined || list.scrollWidth <= list.clientWidth) {
      return; // Nothing scrolls while every tab fits.
    }
    // With the list's padding, so the active tab's outline is not scrolled out of sight.
    const left = active.offsetLeft - LIST_PADDING;
    const right = active.offsetLeft + active.offsetWidth + LIST_PADDING;
    if (left < list.scrollLeft) {
      list.scrollLeft = left;
    } else if (right > list.scrollLeft + list.clientWidth) {
      list.scrollLeft = right - list.clientWidth;
    }
  }, [activeTabId, tabs]);

  // The wheel scrolls the strip sideways (native listener: a React one cannot cancel the page's scroll).
  useEffect(() => {
    const list = listRef.current;
    if (list === null) {
      return;
    }
    const onWheel = (event: WheelEvent): void => {
      if (event.ctrlKey || Math.abs(event.deltaY) <= Math.abs(event.deltaX) || list.scrollWidth <= list.clientWidth) {
        return;
      }
      list.scrollLeft += event.deltaMode === WheelEvent.DOM_DELTA_LINE ? event.deltaY * 16 : event.deltaY;
      event.preventDefault();
    };
    list.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      list.removeEventListener("wheel", onWheel);
    };
  }, []);

  // ----- the menu ----------------------------------------------------------------------

  function menuItems(tab: Tab): MenuItem[] {
    const index = ids.indexOf(tab.id);
    return [
      { id: "pin", label: tab.pinned ? t("tabs.unpin") : t("tabs.pin") },
      { id: "close", label: t("tabs.close") },
      { id: "closeOthers", label: t("tabs.closeOthers"), disabled: !tabs.some((other) => other !== tab && !other.pinned) },
      { id: "closeRight", label: t("tabs.closeRight"), disabled: !tabs.slice(index + 1).some((other) => !other.pinned) },
      { id: "reopen", label: t("tabs.reopen"), disabled: !hasClosedTabs },
    ];
  }

  function runCommand(tab: Tab, command: string): void {
    switch (command) {
      case "pin":
        setTabPinned(tab.id, !tab.pinned);
        break;
      case "close":
        close(tab.id);
        break;
      case "closeOthers":
        closeOtherTabs(tab.id);
        break;
      case "closeRight":
        closeTabsToRight(tab.id);
        break;
      case "reopen":
        reopenClosedTab();
        showMain();
        break;
      default:
    }
  }

  function openMenuAt(tabId: TabId, event: MouseEvent<HTMLElement>): void {
    event.preventDefault();
    const keyboard = event.clientX === 0 && event.clientY === 0; // the context menu key: no pointer position
    const tab = event.currentTarget.querySelector('[role="tab"]');
    setMenu({
      tabId,
      anchor: keyboard && tab !== null ? anchorBelow(tab) : { x: event.clientX, y: event.clientY },
    });
  }

  // ----- the keyboard ------------------------------------------------------------------

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    const button = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>('[role="tab"]') : null;
    const tabId = button?.dataset.tabId;
    if (button === null || tabId === undefined || event.ctrlKey || event.metaKey || event.altKey) {
      return;
    }
    if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
      event.preventDefault();
      setMenu({ tabId, anchor: anchorBelow(button) });
      return;
    }
    if (event.shiftKey) {
      return;
    }
    const buttons = tabButtons();
    const index = buttons.indexOf(button);
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
      case "Delete":
        event.preventDefault();
        close(tabId);
        return;
      default:
        return;
    }
    event.preventDefault();
    next?.focus();
  }

  // ----- drag and drop (HTML5 events) ----------------------------------------------------

  function zoneOf(event: DragEvent<HTMLElement>): Zone {
    const rect = event.currentTarget.getBoundingClientRect();
    return event.clientX - rect.left < rect.width / 2 ? "before" : "after";
  }

  /** Where the dragged tab ends up when dropped on the `zone` of `target`; `null` when it would not move or cannot go there. */
  function dropIndex(target: TabId, zone: Zone): number | null {
    const from = ids.indexOf(dragId ?? "");
    const to = ids.indexOf(target);
    const dragged = tabs[from];
    const over = tabs[to];
    if (dragged === undefined || over === undefined || dragged.pinned !== over.pinned) {
      return null; // a tab stays in its group
    }
    const slot = zone === "before" ? to : to + 1;
    const index = slot > from ? slot - 1 : slot;
    return index === from ? null : index;
  }

  function dragStart(event: DragEvent<HTMLElement>, tabId: TabId): void {
    event.dataTransfer.setData(DRAG_TYPE, tabId);
    event.dataTransfer.effectAllowed = "move";
    setDragId(tabId);
  }

  function dragOver(event: DragEvent<HTMLElement>, target: TabId): void {
    if (dragId === null) {
      return;
    }
    const zone = zoneOf(event);
    if (dropIndex(target, zone) === null) {
      setHint(null);
      return; // nothing would change: leaving the event alone shows the "no drop" cursor
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setHint((current) => (current?.target === target && current.zone === zone ? current : { target, zone }));
  }

  function dragLeave(event: DragEvent<HTMLElement>, target: TabId): void {
    const into = event.relatedTarget;
    if (into instanceof Node && event.currentTarget.contains(into)) {
      return; // moved onto a child of the same tab
    }
    setHint((current) => (current?.target === target ? null : current));
  }

  function drop(event: DragEvent<HTMLElement>, target: TabId): void {
    event.preventDefault();
    const id = dragId;
    const index = dropIndex(target, zoneOf(event));
    endDrag();
    if (id !== null && index !== null) {
      moveTab(id, index);
    }
  }

  function endDrag(): void {
    setDragId(null);
    setHint(null);
  }

  // ----- the tabs ----------------------------------------------------------------------

  function renderTab(tab: Tab, label: TabLabel) {
    const active = tab.id === activeTabId;
    const accessibleName = tab.pinned ? t("tabs.pinnedLabel", { title: label.name }) : label.name;
    const classes = ["tab"];
    if (active) {
      classes.push("is-active");
    }
    if (tab.pinned) {
      classes.push("is-pinned");
    }
    if (tab.id === dragId) {
      classes.push("is-dragging");
    }
    if (hint?.target === tab.id) {
      classes.push(`drop-${hint.zone}`);
    }
    return (
      <div
        key={tab.id}
        className={classes.join(" ")}
        role="presentation"
        draggable
        data-tab-id={tab.id}
        onContextMenu={(event) => {
          openMenuAt(tab.id, event);
        }}
        onDragStart={(event) => {
          dragStart(event, tab.id);
        }}
        onDragOver={(event) => {
          dragOver(event, tab.id);
        }}
        onDragLeave={(event) => {
          dragLeave(event, tab.id);
        }}
        onDrop={(event) => {
          drop(event, tab.id);
        }}
        onDragEnd={endDrag}
      >
        <button
          type="button"
          role="tab"
          id={tabElementId(tab.id)}
          className="tab-button"
          data-tab-id={tab.id}
          aria-selected={active}
          aria-controls={active && route === "home" ? TAB_PANEL_ID : undefined}
          aria-label={accessibleName}
          title={accessibleName}
          tabIndex={tab.id === stopId ? 0 : -1}
          onFocus={() => {
            setRoving((current) =>
              current?.tabId === tab.id && current.whileActive === activeTabId
                ? current
                : { whileActive: activeTabId, tabId: tab.id },
            );
          }}
          onClick={() => {
            activateTab(tab.id);
          }}
          onMouseDown={(event) => {
            if (event.button === 1) {
              event.preventDefault(); // no autoscroll cursor on the middle button
            }
          }}
          onAuxClick={(event) => {
            if (event.button === 1) {
              event.preventDefault();
              close(tab.id);
            }
          }}
        >
          {label.home ? <HomeIcon /> : <PageIcon icon={label.icon} className="tab-icon" />}
          {!tab.pinned && <span className="tab-title">{label.title}</span>}
        </button>
        {!tab.pinned && (
          <button
            type="button"
            className="tab-close"
            tabIndex={-1}
            aria-label={t("tabs.close")}
            title={t("tabs.closeTitle", { shortcut: `${MODIFIER}+W` })}
            onClick={(event) => {
              event.stopPropagation();
              close(tab.id);
            }}
          >
            <CloseIcon />
          </button>
        )}
      </div>
    );
  }

  const menuTab = menu === null ? undefined : tabs.find((tab) => tab.id === menu.tabId);

  return (
    <div className="tab-strip">
      <div className="tab-nav">
        <button
          type="button"
          className="icon-button"
          aria-label={t("tabs.back")}
          title={t("tabs.backTitle", { shortcut: BACK_SHORTCUT })}
          disabled={!canGoBack}
          onClick={() => {
            goBack();
            showMain();
          }}
        >
          <BackIcon />
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={t("tabs.forward")}
          title={t("tabs.forwardTitle", { shortcut: FORWARD_SHORTCUT })}
          disabled={!canGoForward}
          onClick={() => {
            goForward();
            showMain();
          }}
        >
          <ForwardIcon />
        </button>
      </div>
      <div
        ref={listRef}
        className="tab-list"
        role={hasTabs ? "tablist" : undefined}
        aria-label={hasTabs ? t("tabs.label") : undefined}
        onKeyDown={onKeyDown}
      >
        {tabs.map((tab) => {
          const label = labels.get(tab.id);
          return label === undefined ? null : renderTab(tab, label);
        })}
      </div>
      <button
        type="button"
        className="icon-button tab-new"
        aria-label={t("tabs.new")}
        title={t("tabs.newTitle", { shortcut: `${MODIFIER}+T` })}
        disabled={!hasTabs || tabs.length >= MAX_TABS}
        onClick={openNewTab}
      >
        <PlusIcon />
      </button>
      <div className="tab-strip-space" onDoubleClick={hasTabs && tabs.length < MAX_TABS ? openNewTab : undefined} />
      {menu !== null && menuTab !== undefined && (
        <Menu
          label={t("tabs.menu")}
          items={menuItems(menuTab)}
          anchor={menu.anchor}
          onSelect={(command) => {
            runCommand(menuTab, command);
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </div>
  );
}
