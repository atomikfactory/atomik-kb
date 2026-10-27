import type { PageId } from "@atomik-kb/core-client";
import type { CSSProperties, DragEvent, KeyboardEvent } from "react";

import { t } from "../i18n";
import { openGestures } from "../page-open";
import type { OpenDisposition } from "../store-tabs";
import type { DropZone, FlatRow } from "../tree";
import { InlineEdit } from "./InlineEdit";
import { anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { ChevronIcon, MoreIcon, PlusIcon } from "./icons";
import { PageIcon } from "./PageIcon";

/** What a row can ask of the tree that owns it. */
export interface RowHandlers {
  /** Remembers (or, with `null`, forgets) the element of a row, so focus can be moved to it. */
  register: (id: PageId, element: HTMLElement | null) => void;
  /** Opens the page: a click or Enter navigates the tab, a Ctrl/Cmd + click or a middle click opens another (ADR-0034). */
  open: (id: PageId, disposition: OpenDisposition) => void;
  focus: (id: PageId) => void;
  toggle: (id: PageId, expand: boolean) => void;
  addChild: (id: PageId) => void;
  openMenu: (id: PageId, anchor: Anchor, trigger: Element | null) => void;
  startRename: (id: PageId) => void;
  commitRename: (id: PageId, title: string) => Promise<void>;
  endRename: (id: PageId) => void;
  keyDown: (event: KeyboardEvent<HTMLElement>, row: FlatRow) => void;
  dragStart: (event: DragEvent<HTMLElement>, row: FlatRow) => void;
  dragOver: (event: DragEvent<HTMLElement>, row: FlatRow) => void;
  dragLeave: (event: DragEvent<HTMLElement>, row: FlatRow) => void;
  drop: (event: DragEvent<HTMLElement>, row: FlatRow) => void;
  dragEnd: () => void;
}

export interface PageRowProps {
  row: FlatRow;
  handlers: RowHandlers;
  selected: boolean;
  /** The one row that Tab reaches; the arrow keys move between the others. */
  tabbable: boolean;
  renaming: boolean;
  /** The space cannot be edited by this role: no drag, no rename, no new sub-page. */
  readOnly?: boolean;
  /** This row is the one being dragged. */
  dragging: boolean;
  /** Where a drag in progress would land relative to this row. */
  dropZone: DropZone | null;
}

/**
 * One row of the page tree: expand toggle, icon, title (or the rename field) and hover actions. A click
 * opens the page by its gesture (`openGestures`); the toggle and the buttons keep their own.
 */
export function PageRow({ row, handlers, selected, tabbable, renaming, readOnly = false, dragging, dropZone }: PageRowProps) {
  const { page } = row;
  const untitled = page.title === "";
  const title = untitled ? t("page.untitled") : page.title;
  const classes = ["tree-row"];
  if (dragging) {
    classes.push("is-dragging");
  }
  if (dropZone !== null) {
    classes.push(`drop-${dropZone}`);
  }

  return (
    <div
      ref={(element) => {
        handlers.register(row.id, element);
        return () => {
          handlers.register(row.id, null);
        };
      }}
      role="treeitem"
      aria-level={row.depth + 1}
      aria-posinset={row.index + 1}
      aria-setsize={row.siblingCount}
      aria-selected={selected}
      aria-expanded={row.hasChildren ? row.expanded : undefined}
      aria-label={title}
      tabIndex={tabbable ? 0 : -1}
      draggable={!renaming && !readOnly}
      className={classes.join(" ")}
      style={{ "--depth": row.depth } as CSSProperties}
      data-page-id={row.id}
      {...openGestures<HTMLDivElement>((disposition) => {
        handlers.open(row.id, disposition);
      })}
      onDoubleClick={(event) => {
        // A Ctrl or Cmd + double click is two opens in other tabs, not a rename.
        if (!event.ctrlKey && !event.metaKey) {
          handlers.startRename(row.id);
        }
      }}
      onFocus={() => {
        handlers.focus(row.id);
      }}
      onKeyDown={(event) => {
        handlers.keyDown(event, row);
      }}
      onContextMenu={(event) => {
        event.preventDefault();
        handlers.openMenu(row.id, { x: event.clientX, y: event.clientY }, null);
      }}
      onDragStart={(event) => {
        handlers.dragStart(event, row);
      }}
      onDragOver={(event) => {
        handlers.dragOver(event, row);
      }}
      onDragLeave={(event) => {
        handlers.dragLeave(event, row);
      }}
      onDrop={(event) => {
        handlers.drop(event, row);
      }}
      onDragEnd={handlers.dragEnd}
    >
      {row.hasChildren ? (
        <button
          type="button"
          tabIndex={-1}
          className="tree-chevron"
          data-expanded={row.expanded}
          aria-label={row.expanded ? t("tree.collapse") : t("tree.expand")}
          onClick={(event) => {
            event.stopPropagation();
            handlers.toggle(row.id, !row.expanded);
          }}
          onDoubleClick={(event) => {
            event.stopPropagation();
          }}
        >
          <ChevronIcon />
        </button>
      ) : (
        <span className="tree-chevron" aria-hidden="true" />
      )}
      <PageIcon icon={page.icon} className="tree-icon" />
      {renaming ? (
        <InlineEdit
          className="tree-rename"
          value={page.title}
          label={t("tree.renameLabel")}
          placeholder={t("page.untitled")}
          onCommit={(next) => handlers.commitRename(row.id, next)}
          onDone={() => {
            handlers.endRename(row.id);
          }}
        />
      ) : (
        <span className={untitled ? "tree-title is-untitled" : "tree-title"} title={title}>
          {title}
        </span>
      )}
      <span
        className="tree-actions"
        onDoubleClick={(event) => {
          event.stopPropagation();
        }}
      >
        {!readOnly && (
          <button
            type="button"
            tabIndex={-1}
            className="icon-button"
            aria-label={t("tree.addChild")}
            title={t("tree.addChild")}
            onClick={(event) => {
              event.stopPropagation();
              handlers.addChild(row.id);
            }}
          >
            <PlusIcon />
          </button>
        )}
        <button
          type="button"
          tabIndex={-1}
          className="icon-button"
          aria-label={t("tree.actions")}
          title={t("tree.actions")}
          aria-haspopup="menu"
          onClick={(event) => {
            event.stopPropagation();
            handlers.openMenu(row.id, anchorBelow(event.currentTarget), event.currentTarget);
          }}
        >
          <MoreIcon />
        </button>
      </span>
    </div>
  );
}
