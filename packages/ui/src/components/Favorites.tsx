import type { PageId, PageInfo } from "@atomik-kb/core-client";
import { useId, useMemo, useState } from "react";
import type { DragEvent } from "react";

import { t } from "../i18n";
import { openGestures } from "../page-open";
import { useAppStore } from "../store-context";
import { selectFavoritesCollapsed } from "../store-layout";
import { selectFavorites } from "../store-structure";
import type { OpenDisposition } from "../store-tabs";
import { templatePageIds } from "../templates";
import { indexPages } from "../tree";
import { useRunAction } from "./actions";
import { anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { ChevronIcon, MoreIcon } from "./icons";
import { Menu } from "./Menu";
import type { MenuItem } from "./Menu";
import { PageIcon } from "./PageIcon";
import { TemplateIcon } from "./template-icons";

/** Marks a drag that carries a favourite of this list (a drag from elsewhere does not). */
const DRAG_TYPE = "application/x-atomik-favorite";

type Zone = "before" | "after";

interface OpenMenu {
  id: PageId;
  anchor: Anchor;
  /** The button that opened it, if any, so pressing it again closes instead of reopening. */
  trigger: Element | null;
}

interface DropHint {
  target: PageId;
  zone: Zone;
}

/** The favourite a page dropped on the `zone` of `target` goes in front of; `null` is the end of the list. */
export function dropSlot(order: readonly PageId[], target: PageId, zone: Zone): PageId | null {
  return zone === "before" ? target : (order[order.indexOf(target) + 1] ?? null);
}

/** True when putting `id` in front of `before` (the end for `null`) leaves the order as it is. */
export function keepsOrder(order: readonly PageId[], id: PageId, before: PageId | null): boolean {
  const index = order.indexOf(id);
  if (index < 0 || before === id) {
    return true;
  }
  return before === null ? index === order.length - 1 : order[index + 1] === before;
}

/**
 * The favourites of the current space above the page tree (ADR-0029), in the order the user
 * chose; nothing at all while there are none. The title is a button that collapses the list
 * (remembered on this device). A row opens the page, has a menu (Move up, Move down, Remove from
 * favourites: the keyboard's way to reorder) and can be dragged to a new place. The page that
 * is open is highlighted. A favourite that is a template, or inside one (ADR-0056), has a template mark.
 */
export function Favorites() {
  const run = useRunAction();
  const tree = useAppStore((state) => state.tree);
  const order = useAppStore(selectFavorites);
  const collapsed = useAppStore(selectFavoritesCollapsed);
  const setCollapsed = useAppStore((state) => state.setFavoritesCollapsed);
  const selectedPageId = useAppStore((state) => state.selectedPageId);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const setPageFavorite = useAppStore((state) => state.setPageFavorite);
  const moveFavorite = useAppStore((state) => state.moveFavorite);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const [dragId, setDragId] = useState<PageId | null>(null);
  const [hint, setHint] = useState<DropHint | null>(null);
  const listId = useId();

  // A favourite whose page is not in the live tree (it cannot be, but the list is data) is left out.
  const pages = useMemo(() => {
    const index = tree === null ? new Map<PageId, PageInfo>() : indexPages(tree.roots);
    return order.flatMap((id) => {
      const page = index.get(id);
      return page === undefined ? [] : [page];
    });
  }, [tree, order]);
  const ids = useMemo(() => pages.map((page) => page.id), [pages]);
  // A favourite that is a template, or inside one (ADR-0056), stays here with a mark: the tree no longer shows it.
  const templateIds = useMemo(() => templatePageIds(tree), [tree]);

  if (pages.length === 0) {
    return null;
  }

  function open(page: PageInfo, disposition: OpenDisposition): void {
    navigateTo({ pageId: page.id, spaceId: page.spaceId, disposition });
  }

  function moveTo(id: PageId, before: PageId | null): void {
    void run(moveFavorite(id, before ?? undefined), "favorites.error.move");
  }

  function remove(id: PageId): void {
    void run(setPageFavorite(id, false), "favorites.error.set");
  }

  function menuItems(id: PageId): MenuItem[] {
    const index = ids.indexOf(id);
    return [
      { id: "open", label: t("page.openInNewTab") },
      { id: "up", label: t("favorites.moveUp"), disabled: index <= 0 },
      { id: "down", label: t("favorites.moveDown"), disabled: index < 0 || index >= ids.length - 1 },
      { id: "remove", label: t("favorites.remove") },
    ];
  }

  function runCommand(id: PageId, command: string): void {
    const index = ids.indexOf(id);
    const page = pages[index];
    if (command === "open" && page !== undefined) {
      open(page, "foreground");
    } else if (command === "up" && index > 0) {
      moveTo(id, ids[index - 1] ?? null);
    } else if (command === "down" && index >= 0 && index < ids.length - 1) {
      moveTo(id, ids[index + 2] ?? null);
    } else if (command === "remove") {
      remove(id);
    }
  }

  // ----- drag and drop (HTML5 events) --------------------------------------------------

  function zoneOf(event: DragEvent<HTMLElement>): Zone {
    const rect = event.currentTarget.getBoundingClientRect();
    return event.clientY - rect.top < rect.height / 2 ? "before" : "after";
  }

  function dragStart(event: DragEvent<HTMLElement>, id: PageId): void {
    event.dataTransfer.setData(DRAG_TYPE, id);
    event.dataTransfer.effectAllowed = "move";
    setDragId(id);
  }

  function dragOver(event: DragEvent<HTMLElement>, target: PageId): void {
    if (dragId === null) {
      return;
    }
    const zone = zoneOf(event);
    if (keepsOrder(ids, dragId, dropSlot(ids, target, zone))) {
      setHint(null);
      return; // nothing would change: leaving the event alone shows the "no drop" cursor
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setHint((current) => (current?.target === target && current.zone === zone ? current : { target, zone }));
  }

  function dragLeave(event: DragEvent<HTMLElement>, target: PageId): void {
    const into = event.relatedTarget;
    if (into instanceof Node && event.currentTarget.contains(into)) {
      return; // moved onto a child of the same row
    }
    setHint((current) => (current?.target === target ? null : current));
  }

  function drop(event: DragEvent<HTMLElement>, target: PageId): void {
    event.preventDefault();
    const id = dragId;
    const before = dropSlot(ids, target, zoneOf(event));
    endDrag();
    if (id !== null && !keepsOrder(ids, id, before)) {
      moveTo(id, before);
    }
  }

  function endDrag(): void {
    setDragId(null);
    setHint(null);
  }

  return (
    <div className="favorites">
      <button
        type="button"
        className="favorites-toggle"
        aria-expanded={!collapsed}
        aria-controls={collapsed ? undefined : listId}
        onClick={() => {
          setCollapsed(!collapsed);
        }}
      >
        <span className="favorites-toggle-chevron" data-expanded={!collapsed} aria-hidden="true">
          <ChevronIcon />
        </span>
        <span>{t("favorites.title")}</span>
      </button>
      {!collapsed && (
        <ul id={listId} className="favorites-list" aria-label={t("favorites.title")}>
          {pages.map((page) => {
            const untitled = page.title === "";
            const title = untitled ? t("page.untitled") : page.title;
            const classes = ["favorite-row"];
            if (page.id === selectedPageId) {
              classes.push("is-selected");
            }
            if (page.id === dragId) {
              classes.push("is-dragging");
            }
            if (hint?.target === page.id) {
              classes.push(`drop-${hint.zone}`);
            }
            return (
              <li
                key={page.id}
                className={classes.join(" ")}
                draggable
                data-page-id={page.id}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu({ id: page.id, anchor: { x: event.clientX, y: event.clientY }, trigger: null });
                }}
                onDragStart={(event) => {
                  dragStart(event, page.id);
                }}
                onDragOver={(event) => {
                  dragOver(event, page.id);
                }}
                onDragLeave={(event) => {
                  dragLeave(event, page.id);
                }}
                onDrop={(event) => {
                  drop(event, page.id);
                }}
                onDragEnd={endDrag}
              >
                <button
                  type="button"
                  className="favorite-open"
                  aria-current={page.id === selectedPageId ? "page" : undefined}
                  title={title}
                  {...openGestures<HTMLButtonElement>((disposition) => {
                    open(page, disposition);
                  })}
                >
                  <PageIcon icon={page.icon} className="tree-icon" />
                  <span className={untitled ? "tree-title is-untitled" : "tree-title"}>{title}</span>
                  {templateIds.has(page.id) && (
                    <span className="template-mark" role="img" aria-label={t("template.mark")} title={t("template.mark")}>
                      <TemplateIcon />
                    </span>
                  )}
                </button>
                <span className="favorite-actions">
                  <button
                    type="button"
                    className="icon-button"
                    aria-label={t("favorites.actions")}
                    title={t("favorites.actions")}
                    aria-haspopup="menu"
                    onClick={(event) => {
                      const trigger = event.currentTarget;
                      setMenu((current) =>
                        current?.trigger === trigger ? null : { id: page.id, anchor: anchorBelow(trigger), trigger },
                      );
                    }}
                  >
                    <MoreIcon />
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      )}
      {menu !== null && (
        <Menu
          label={t("favorites.actions")}
          items={menuItems(menu.id)}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(command) => {
            runCommand(menu.id, command);
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </div>
  );
}
