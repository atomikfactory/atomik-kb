import type { PageId, PageNode, SpaceId } from "@atomik-kb/core-client";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { DragEvent, KeyboardEvent } from "react";

import { t } from "../i18n";
import { useCurrentAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { selectFavorites } from "../store-structure";
import type { OpenDisposition } from "../store-tabs";
import { createFromChoice } from "../template-choices";
import type { TemplateChoice } from "../template-choices";
import { toFullTarget, withoutTemplates } from "../template-tree";
import {
  dropZone as zoneAt,
  findPage,
  flattenVisible,
  indentTarget,
  isNoop,
  moveDownTarget,
  moveUpTarget,
  neighborAfterRemoval,
  locate,
  outdentTarget,
  pathTo,
  resolveDrop,
  rootEndTarget,
} from "../tree";
import type { DropZone, FlatRow, MoveTarget } from "../tree";
import { useRunAction } from "./actions";
import { IconPicker } from "./IconPicker";
import { anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { PlusIcon } from "./icons";
import { Menu } from "./Menu";
import type { MenuItem } from "./Menu";
import { MoveToSpaceDialog } from "./MoveToSpaceDialog";
import { PageRow } from "./PageRow";
import type { RowHandlers } from "./PageRow";
import { PageFromTemplateIcon } from "./template-icons";
import { TemplatePicker } from "./TemplatePicker";

const NO_ROOTS: readonly PageNode[] = [];
const NO_COLLAPSED: ReadonlySet<PageId> = new Set();
/** Marks a drag that carries a page of this tree (a drag from elsewhere does not). */
const DRAG_TYPE = "application/x-atomik-page";

interface Opened {
  id: PageId;
  anchor: Anchor;
  /** The button that opened it, if any, so pressing it again closes instead of reopening. */
  trigger: Element | null;
  /** Where focus goes when whatever this opened is closed again: the button, else the row. */
  returnTo: Element | null;
}

/** The template picker, open for the place a page made from a template goes (`parentId` `null`: the top level). */
interface Picking {
  parentId: PageId | null;
  anchor: Anchor;
  anchorElement: Element | null;
  returnTo: Element | null;
}

interface DropHint {
  /** The row the drop would land on; `null` for the space below the last row (the end of the root). */
  target: PageId | null;
  zone: DropZone;
}

/**
 * The page tree of the current space: rows with expand toggles, inline rename, drag and
 * drop (before, after or inside a row), a context menu and keyboard navigation.
 *
 * The rows are a flat list of `treeitem`s with `aria-level`, in display order; the arrow
 * keys move focus (only one row is reachable with Tab), Enter opens, F2 renames, Delete
 * moves to the trash and the context menu key opens the menu.
 *
 * A click opens the page in the active tab, Ctrl/Cmd + click or a middle click in a background
 * tab, Ctrl/Cmd + Shift + click or "Open in new tab" in the menu in a tab in front (ADR-0034). The
 * tree has no multi-select, so those clicks mean nothing else here.
 *
 * The row menu also sorts a page's subpages once, A to Z or by date created (ADR-0052), when it has two or more.
 *
 * Templates (ADR-0056) and the pages inside them are not in the tree: it is drawn from `withoutTemplates`, and a move the person
 * makes is put back in terms of the whole tree (`toFullTarget`). The row menu turns a page into a template (it leaves the tree
 * for the Templates panel) and makes a sub-page from a template; "New page from template…" beside "New page" does the same at
 * the top level.
 *
 * A role that cannot edit the space (a Viewer or a Commenter, or a read-only copy; Milestone 13) gets a tree to read: no new page,
 * rename, drag, move or trash, only opening a page and the favourites, which are this account's own. An Editor cannot move a
 * page out of a shared space (that is an Owner's).
 *
 * Drag and drop uses the HTML5 drag events. In the Tauri desktop shell on Windows they
 * only reach the page when the window sets `dragDropEnabled: false` (Tauri's own drop
 * handler replaces WebView2's otherwise); the menu commands do the same moves without it.
 */
export function PageTree() {
  const run = useRunAction();
  const tree = useAppStore((state) => state.tree);
  const spaceId = useAppStore((state) => state.currentSpaceId);
  const access = useCurrentAccess();
  const canEdit = access.canEdit;
  // "Move to…" is offered when there is another space to move to (ADR-0038), and the role may move pages out of this one.
  const canMoveToSpace = useAppStore((state) => state.spaces.length > 1) && access.isOwner;
  const collapsed = useAppStore((state) =>
    state.currentSpaceId === null ? undefined : state.collapsed[state.currentSpaceId],
  );
  const selectedPageId = useAppStore((state) => state.selectedPageId);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const createPage = useAppStore((state) => state.createPage);
  const renamePage = useAppStore((state) => state.renamePage);
  const setPageIcon = useAppStore((state) => state.setPageIcon);
  const favorites = useAppStore(selectFavorites);
  const setPageFavorite = useAppStore((state) => state.setPageFavorite);
  const movePage = useAppStore((state) => state.movePage);
  const sortChildren = useAppStore((state) => state.sortChildren);
  const duplicatePage = useAppStore((state) => state.duplicatePage);
  const setPageTemplate = useAppStore((state) => state.setPageTemplate);
  const createFromTemplate = useAppStore((state) => state.createFromTemplate);
  const createFromStarter = useAppStore((state) => state.createFromStarter);
  const trashPage = useAppStore((state) => state.trashPage);
  const setPageCollapsed = useAppStore((state) => state.setPageCollapsed);

  const allRoots = tree?.roots ?? NO_ROOTS;
  // What the tree shows: no template and nothing inside one. Positions below are among these pages.
  const roots = useMemo(() => withoutTemplates(allRoots), [allRoots]);
  const rows = useMemo(() => flattenVisible(roots, collapsed ?? NO_COLLAPSED), [roots, collapsed]);

  const [focusedId, setFocusedId] = useState<PageId | null>(null);
  const [renamingId, setRenamingId] = useState<PageId | null>(null);
  const [menu, setMenu] = useState<Opened | null>(null);
  const [iconFor, setIconFor] = useState<Opened | null>(null);
  // The page being moved to another space, with its title and space as they were: it leaves this tree when the move is done.
  const [moveFor, setMoveFor] = useState<(Opened & { title: string; spaceId: SpaceId }) | null>(null);
  const [picking, setPicking] = useState<Picking | null>(null);
  const [dragId, setDragId] = useState<PageId | null>(null);
  const [hint, setHint] = useState<DropHint | null>(null);
  const elements = useRef(new Map<PageId, HTMLElement>());
  // A row that should take focus as soon as it is on screen (after a rename or a removal).
  const pendingFocus = useRef<PageId | null>(null);

  // Only one row is in the Tab order: the one last focused, else the open page, else the first.
  const tabbableId =
    focusedId !== null && rows.some((row) => row.id === focusedId)
      ? focusedId
      : (rows.find((row) => row.id === selectedPageId)?.id ?? rows[0]?.id ?? null);

  useLayoutEffect(() => {
    const id = pendingFocus.current;
    const element = id === null ? undefined : elements.current.get(id);
    if (element === undefined) {
      return; // not on screen yet: keep waiting
    }
    pendingFocus.current = null;
    // Only when focus is nowhere (the focused row or field just went away); never take it
    // from something the user moved to meanwhile.
    const active = document.activeElement;
    if (active === null || active === document.body) {
      element.focus();
    }
  });

  // Reveal: when another page comes to the front (a tab switch, a link into another space), its collapsed
  // ancestors open and its row scrolls into view, once per page and space so a later collapse stays.
  const revealed = useRef<string | null>(null);
  useEffect(() => {
    if (tree === null || spaceId === null || selectedPageId === null || tree.spaceId !== spaceId) {
      return;
    }
    const key = `${spaceId}:${selectedPageId}`;
    if (revealed.current === key) {
      return;
    }
    const path = pathTo(allRoots, selectedPageId);
    if (path === null || collapsed === undefined) {
      return; // not in the tree yet, or the collapsed set is not read yet
    }
    revealed.current = key;
    for (const page of path.slice(0, -1)) {
      if (collapsed.has(page.id)) {
        setPageCollapsed(page.id, false);
      }
    }
    elements.current.get(selectedPageId)?.scrollIntoView?.({ block: "nearest" });
  }, [tree, spaceId, selectedPageId, allRoots, collapsed, setPageCollapsed]);

  const register = useCallback((id: PageId, element: HTMLElement | null) => {
    if (element === null) {
      elements.current.delete(id);
    } else {
      elements.current.set(id, element);
    }
  }, []);

  function focusRow(id: PageId | undefined): void {
    if (id !== undefined) {
      elements.current.get(id)?.focus();
    }
  }

  function createIn(parentId: PageId | null): void {
    if (!canEdit) {
      return;
    }
    void run(
      createPage({ parentId }).then((page) => {
        setRenamingId(page.id);
      }),
      "tree.error.create",
    );
  }

  /** Makes a page from the template or starter the person picked, under `parentId` (the top level for `null`), and opens it. */
  function makeFrom(choice: TemplateChoice, parentId: PageId | null): void {
    if (!canEdit) {
      return;
    }
    void run(
      createFromChoice({ createFromTemplate, createFromStarter }, choice, parentId === null ? {} : { parentId }),
      "template.error.use",
    );
  }

  function trash(id: PageId): void {
    if (!canEdit) {
      return;
    }
    pendingFocus.current = neighborAfterRemoval(
      rows,
      rows.findIndex((row) => row.id === id),
    );
    void run(trashPage(id), "tree.error.trash");
  }

  function move(id: PageId, target: MoveTarget): void {
    if (!canEdit) {
      return;
    }
    const placed = toFullTarget(allRoots, id, target);
    void run(movePage(id, placed.parentId, placed.index), "tree.error.move");
    if (target.parentId !== null) {
      setPageCollapsed(target.parentId, false); // keep the moved page in view
    }
  }

  /** Opens a page of the tree (all of them are in the current space) as the gesture asks. */
  function open(id: PageId, disposition: OpenDisposition): void {
    navigateTo({ pageId: id, spaceId: spaceId ?? undefined, disposition });
  }

  function openMenuAtRow(id: PageId): void {
    const element = elements.current.get(id);
    setMenu({
      id,
      anchor: element === undefined ? { x: 0, y: 0 } : anchorBelow(element),
      trigger: null,
      returnTo: element ?? null,
    });
  }

  /** The commands of the context menu; a command that does not apply is disabled. */
  function menuItems(id: PageId): MenuItem[] {
    if (!canEdit) {
      return [
        { id: "open", label: t("page.openInNewTab") },
        { id: "favorite", label: favorites.includes(id) ? t("favorites.remove") : t("favorites.add") },
      ];
    }
    // Sorting needs something to put in order: two subpages at least (ADR-0052).
    const cannotSort = (locate(roots, id)?.node.children.length ?? 0) < 2;
    return [
      { id: "open", label: t("page.openInNewTab") },
      { id: "up", label: t("tree.menu.moveUp"), disabled: moveUpTarget(roots, id) === null },
      { id: "down", label: t("tree.menu.moveDown"), disabled: moveDownTarget(roots, id) === null },
      { id: "indent", label: t("tree.menu.indent"), disabled: indentTarget(roots, id) === null },
      { id: "outdent", label: t("tree.menu.outdent"), disabled: outdentTarget(roots, id) === null },
      { id: "sortTitle", label: t("tree.menu.sortTitle"), disabled: cannotSort },
      { id: "sortCreated", label: t("tree.menu.sortCreated"), disabled: cannotSort },
      { id: "duplicate", label: t("page.duplicate") },
      ...(canMoveToSpace ? [{ id: "moveToSpace", label: t("moveToSpace.menu") }] : []),
      { id: "rename", label: t("tree.menu.rename") },
      { id: "icon", label: t("tree.menu.setIcon") },
      { id: "favorite", label: favorites.includes(id) ? t("favorites.remove") : t("favorites.add") },
      { id: "newFromTemplate", label: t("template.newSubPageFrom") },
      { id: "turnIntoTemplate", label: t("template.turnInto") },
      { id: "trash", label: t("tree.menu.trash"), danger: true },
    ];
  }

  function runCommand(opened: Opened, command: string): void {
    const { id } = opened;
    const targets: Record<string, MoveTarget | null> = {
      up: moveUpTarget(roots, id),
      down: moveDownTarget(roots, id),
      indent: indentTarget(roots, id),
      outdent: outdentTarget(roots, id),
    };
    const target = targets[command];
    if (command === "open") {
      open(id, "foreground");
    } else if (target !== undefined && target !== null) {
      move(id, target);
    } else if (command === "sortTitle" || command === "sortCreated") {
      if (spaceId !== null) {
        void run(sortChildren(spaceId, id, command === "sortTitle" ? "title" : "created"), "tree.error.sort");
        setPageCollapsed(id, false); // keep the sorted subpages in view
      }
    } else if (command === "duplicate") {
      void run(duplicatePage(id), "page.duplicate.error");
    } else if (command === "moveToSpace") {
      if (spaceId !== null) {
        setMoveFor({ ...opened, title: findPage(roots, id)?.title ?? "", spaceId });
      }
    } else if (command === "rename") {
      setRenamingId(id);
    } else if (command === "icon") {
      setIconFor(opened);
    } else if (command === "favorite") {
      void run(setPageFavorite(id, !favorites.includes(id)), "favorites.error.set");
    } else if (command === "newFromTemplate") {
      setPicking({ parentId: id, anchor: opened.anchor, anchorElement: opened.trigger, returnTo: opened.returnTo });
    } else if (command === "turnIntoTemplate") {
      // The page leaves the tree: focus goes to its neighbour, as for a page sent to the trash.
      pendingFocus.current = neighborAfterRemoval(
        rows,
        rows.findIndex((row) => row.id === id),
      );
      void run(setPageTemplate(id, true), "template.error.change");
    } else if (command === "trash") {
      trash(id);
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLElement>, row: FlatRow): void {
    // Keys typed into the rename field, or with a modifier, are not for the tree.
    if (event.target !== event.currentTarget || event.ctrlKey || event.metaKey || event.altKey) {
      return;
    }
    const index = rows.findIndex((candidate) => candidate.id === row.id);
    switch (event.key) {
      case "ArrowDown":
        focusRow(rows[index + 1]?.id);
        break;
      case "ArrowUp":
        focusRow(rows[index - 1]?.id);
        break;
      case "Home":
        focusRow(rows[0]?.id);
        break;
      case "End":
        focusRow(rows[rows.length - 1]?.id);
        break;
      case "ArrowRight":
        if (row.hasChildren) {
          if (row.expanded) {
            focusRow(rows[index + 1]?.id);
          } else {
            setPageCollapsed(row.id, false);
          }
        }
        break;
      case "ArrowLeft":
        if (row.hasChildren && row.expanded) {
          setPageCollapsed(row.id, true);
        } else {
          focusRow(row.parentId ?? undefined);
        }
        break;
      case "Enter":
      case " ":
        open(row.id, "current");
        break;
      case "F2":
        if (!canEdit) {
          return;
        }
        setRenamingId(row.id);
        break;
      case "Delete":
        if (!canEdit) {
          return;
        }
        trash(row.id);
        break;
      case "ContextMenu":
        openMenuAtRow(row.id);
        break;
      case "F10":
        if (!event.shiftKey) {
          return;
        }
        openMenuAtRow(row.id);
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  // ----- drag and drop (HTML5 events) --------------------------------------------------

  /** The zone of the row under the pointer, from where in the row the pointer is. */
  function zoneOf(event: DragEvent<HTMLElement>): DropZone {
    const rect = event.currentTarget.getBoundingClientRect();
    return zoneAt(event.clientY - rect.top, rect.height);
  }

  function dragStart(event: DragEvent<HTMLElement>, row: FlatRow): void {
    if (renamingId !== null || !canEdit) {
      event.preventDefault();
      return;
    }
    event.dataTransfer.setData(DRAG_TYPE, row.id);
    if (spaceId !== null) {
      event.dataTransfer.setData("application/x-atomik-page-space", spaceId);
    }
    // "move" inside the tree; the editor takes the same drag as a link (page mention).
    event.dataTransfer.effectAllowed = "copyMove";
    setDragId(row.id);
  }

  function dragOver(event: DragEvent<HTMLElement>, row: FlatRow): void {
    if (dragId === null) {
      return;
    }
    const zone = zoneOf(event);
    if (resolveDrop(roots, dragId, row.id, zone) === null) {
      setHint(null);
      return; // not allowed: leaving the event alone shows the "no drop" cursor
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setHint((current) => (current?.target === row.id && current.zone === zone ? current : { target: row.id, zone }));
  }

  function dragLeave(event: DragEvent<HTMLElement>, row: FlatRow): void {
    const into = event.relatedTarget;
    if (into instanceof Node && event.currentTarget.contains(into)) {
      return; // moved onto a child of the same row
    }
    setHint((current) => (current?.target === row.id ? null : current));
  }

  function drop(event: DragEvent<HTMLElement>, row: FlatRow): void {
    event.preventDefault();
    const zone = zoneOf(event);
    dropOn(resolveDrop(roots, dragId ?? "", row.id, zone), zone === "inside" ? row.id : null);
  }

  /** Moves the dragged page to `target` (unless that changes nothing) and ends the drag. */
  function dropOn(target: MoveTarget | null, expandId: PageId | null): void {
    const id = dragId;
    setDragId(null);
    setHint(null);
    if (id === null || target === null || isNoop(roots, id, target)) {
      return;
    }
    move(id, target);
    if (expandId !== null) {
      setPageCollapsed(expandId, false);
    }
  }

  function dragOverEnd(event: DragEvent<HTMLElement>): void {
    if (dragId === null || isNoop(roots, dragId, rootEndTarget(roots, dragId))) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setHint((current) => (current !== null && current.target === null ? current : { target: null, zone: "inside" }));
  }

  function endDrag(): void {
    setDragId(null);
    setHint(null);
  }

  const handlers: RowHandlers = {
    register,
    open,
    focus: (id) => {
      pendingFocus.current = null;
      setFocusedId(id);
    },
    toggle: (id, expand) => {
      setPageCollapsed(id, !expand);
    },
    addChild: createIn,
    openMenu: (id, anchor, trigger) => {
      // The button that opened the menu also closes it.
      const returnTo = trigger ?? elements.current.get(id) ?? null;
      setMenu((current) =>
        trigger !== null && current?.trigger === trigger ? null : { id, anchor, trigger, returnTo },
      );
    },
    startRename: (id) => {
      if (canEdit) {
        setRenamingId(id);
      }
    },
    commitRename: (id, title) => run(renamePage(id, title), "tree.error.rename"),
    endRename: (id) => {
      setRenamingId((current) => (current === id ? null : current));
      pendingFocus.current = id;
    },
    keyDown: onKeyDown,
    dragStart,
    dragOver,
    dragLeave,
    drop,
    dragEnd: endDrag,
  };

  if (tree === null) {
    return spaceId === null ? null : (
      <p className="muted sidebar-note" role="status">
        {t("sidebar.loading")}
      </p>
    );
  }

  const hintFor = (id: PageId): DropZone | null => (hint?.target === id ? hint.zone : null);
  const iconPage = iconFor === null ? null : findPage(roots, iconFor.id);
  const overEnd = hint !== null && hint.target === null;

  return (
    <div className="page-tree">
      {rows.length > 0 ? (
        <div role="tree" aria-label={t("tree.label")} className="tree">
          {rows.map((row) => (
            <PageRow
              key={row.id}
              row={row}
              handlers={handlers}
              selected={row.id === selectedPageId}
              tabbable={row.id === tabbableId}
              renaming={row.id === renamingId}
              readOnly={!canEdit}
              dragging={row.id === dragId}
              dropZone={hintFor(row.id)}
            />
          ))}
        </div>
      ) : (
        <p className="muted sidebar-note">{t("tree.empty")}</p>
      )}
      <div
        className={overEnd ? "tree-end is-drop-target" : "tree-end"}
        onDragOver={dragOverEnd}
        onDragLeave={() => {
          setHint((current) => (current !== null && current.target === null ? null : current));
        }}
        onDrop={(event) => {
          event.preventDefault();
          dropOn(dragId === null ? null : rootEndTarget(roots, dragId), null);
        }}
      >
        {canEdit && (
          <button
            type="button"
            className="tree-new"
            onClick={() => {
              createIn(null);
            }}
          >
            <PlusIcon />
            <span>{t("tree.newPage")}</span>
          </button>
        )}
        {canEdit && (
          <button
            type="button"
            className="icon-button tree-new-template"
            aria-label={t("template.newPageFrom")}
            title={t("template.newPageFrom")}
            aria-haspopup="menu"
            aria-expanded={picking !== null && picking.parentId === null}
            onClick={(event) => {
              const trigger = event.currentTarget;
              setPicking((current) =>
                current !== null && current.parentId === null
                  ? null
                  : { parentId: null, anchor: anchorBelow(trigger), anchorElement: trigger, returnTo: trigger },
              );
            }}
          >
            <PageFromTemplateIcon />
          </button>
        )}
      </div>

      {menu !== null && (
        <Menu
          label={t("tree.actions")}
          items={menuItems(menu.id)}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(command) => {
            runCommand(menu, command);
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
      {picking !== null && (
        <TemplatePicker
          anchor={picking.anchor}
          anchorElement={picking.anchorElement}
          returnFocus={picking.returnTo}
          onPick={(choice) => {
            makeFrom(choice, picking.parentId);
          }}
          onClose={() => {
            setPicking(null);
          }}
        />
      )}
      {moveFor !== null && (
        <MoveToSpaceDialog
          pageId={moveFor.id}
          title={moveFor.title}
          spaceId={moveFor.spaceId}
          returnFocus={moveFor.returnTo}
          onClose={() => {
            setMoveFor(null);
          }}
        />
      )}
      {iconFor !== null && iconPage !== null && (
        <IconPicker
          anchor={iconFor.anchor}
          anchorElement={iconFor.trigger}
          returnFocus={iconFor.returnTo}
          current={iconPage.icon}
          allowUpload
          spaceId={iconPage.spaceId}
          onSelect={(icon) => setPageIcon(iconFor.id, icon)}
          onClose={() => {
            setIconFor(null);
          }}
        />
      )}
    </div>
  );
}
