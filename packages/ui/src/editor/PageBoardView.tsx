// The board layout of the list view block (ADR-0016, amendment of 2026-10-08, the board): one column
// per option of the block's group property (a select or a status) and a first column for the pages
// without a value; a card per page (icon, title that opens the page, the properties the block
// shows, read-only). Dragging a card to another column (or "Move to…" in its menu, for the keyboard)
// sets the page's value of the group property through `pagePropertySet`; the "No …" column clears
// it. The order inside a column is the view's sort. "New" at the foot of a column makes a page that
// is in the column (`onCreatePage` with the column's value), and "Add group" adds an option to the property. The
// layout takes the props every layout takes (`page-list-layout-types.ts`, ADR-0054): it asks to open a page through
// `onOpen` and writes no attribute of the block. In an editable editor each card has the checkbox of the selection in its corner
// (`PageRowCheckbox`, ADR-0058), and a column draws at most 200 cards until its "Show more" (`page-list-row-limit.ts`).

import { toCoreError } from "@atomik-kb/core-client";
import type { PageInfo } from "@atomik-kb/core-client";
import { useEffect, useMemo, useRef, useState } from "react";
import type { DragEvent, FormEvent } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { ChevronDownIcon, ChevronIcon, MoreIcon, PlusIcon } from "../components/icons";
import { OptionPill } from "../components/OptionPill";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { openGestures } from "../page-open";
import { GROUP_LABELS, hasOptionNamed, nextOptionColor } from "../property-format";
import { useAppStore } from "../store-context";
import { boardColumns, columnOf, groupRows, isGroupable, NO_GROUP, resolveGroup, valueForColumn } from "./page-board-model";
import type { BoardColumn } from "./page-board-model";
import { NewPageMenu } from "./NewPageMenu";
import type { NewPageInit, PageListLayoutProps } from "./page-list-layout-types";
import { valueOfPage } from "./page-list-model";
import { limitRows, ROW_LIMIT_STEP, useRowLimit } from "./page-list-row-limit";
import { PageListValue } from "./PageListValue";
import { PageRowCheckbox, selectLabel } from "./PageRowCheckbox";

const NO_KEYS: readonly string[] = [];

const DRAG_TYPE = "application/x-atomik-board-card";

interface CardMenu {
  page: PageInfo;
  anchor: Anchor;
  trigger: Element;
}

/** The board: the layout of a view whose `layout` is `board`, its columns the options of the view's `groupBy` property. */
export function PageBoardView({
  rows,
  view,
  schema,
  columns,
  spaceId,
  editable,
  today,
  onOpen,
  onUpdateView,
  onCreatePage,
  creating,
  selection,
}: PageListLayoutProps) {
  const group = useMemo(() => resolveGroup(view.groupBy ?? null, schema), [view.groupBy, schema]);
  // Whether the space has a select or status property to choose (for the hint).
  const canChoose = schema.some(isGroupable);
  const hiddenGroups = view.hiddenGroups ?? NO_KEYS;
  const setPageProperty = useAppStore((state) => state.setPageProperty);
  const addPropertyOption = useAppStore((state) => state.addPropertyOption);
  const [dragId, setDragId] = useState<string | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(false);
  const [menu, setMenu] = useState<CardMenu | null>(null);
  const [adding, setAdding] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const def = group.kind === "ok" ? group.def : null;
  const groups = useMemo(() => (def === null ? new Map<string, PageInfo[]>() : groupRows(rows, def)), [rows, def]);
  const root = useRef<HTMLDivElement>(null);
  // At most 200 cards are drawn per column, "Show 200 more" adds 200 to that column (ADR-0058); the counts and the CSV use every card.
  const limit = useRowLimit(view.id);
  const drawn = def !== null;
  // A card's drag is the board's own: the editor (its drop cursor, its drag of a selection) must not
  // see these events. React has handled them at the node view's element when they get there, so
  // they are stopped there, for the editor's listeners further up.
  useEffect(() => {
    const host = root.current?.closest(".page-list-node");
    if (host === null || host === undefined) {
      return;
    }
    const stop = (event: Event): void => {
      event.stopPropagation();
    };
    const types = ["dragstart", "dragenter", "dragover", "dragleave", "drop", "dragend"];
    for (const type of types) {
      host.addEventListener(type, stop);
    }
    return () => {
      for (const type of types) {
        host.removeEventListener(type, stop);
      }
    };
  }, [drawn]);

  if (def === null) {
    const text =
      group.kind === "missing"
        ? t("pageBoard.groupGone")
        : group.kind === "wrongKind"
          ? t("pageBoard.groupKind", { name: group.def.name })
          : canChoose
            ? t("pageBoard.choose")
            : t("pageBoard.noProperty");
    return (
      <p className="page-list-empty" role="status">
        {text}
      </p>
    );
  }

  const shown = boardColumns(def).filter((column) => !hiddenGroups.includes(column.key));
  const nameOf = (column: BoardColumn): string => column.option?.name ?? t("pageBoard.none", { name: def.name });
  /** What a page made in a column starts with: the column's value (the "No ..." column has none). */
  const initFor = (column: BoardColumn): NewPageInit | undefined => {
    const value = valueForColumn(def, column.key);
    return value === null ? undefined : { values: [[def.id, value]] };
  };

  async function move(pageId: string, key: string): Promise<void> {
    if (def === null) {
      return;
    }
    const page = rows.find((candidate) => candidate.id === pageId);
    if (page === undefined || columnOf(page, def) === key) {
      return; // dropped where it already is
    }
    setError(null);
    try {
      await setPageProperty(pageId, def.id, valueForColumn(def, key));
    } catch (cause) {
      setError(t("pageBoard.error.move", { message: toCoreError(cause).message }));
    }
  }

  function dragOver(event: DragEvent<HTMLElement>, key: string): void {
    if (dragId === null) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setOver(key);
  }

  function drop(event: DragEvent<HTMLElement>, key: string): void {
    event.preventDefault();
    const id = dragId;
    setDragId(null);
    setOver(null);
    if (id !== null) {
      void move(id, key);
    }
  }

  async function addGroup(event: FormEvent): Promise<void> {
    event.preventDefault();
    if (def === null || adding === null) {
      return;
    }
    const name = adding.trim();
    if (name === "") {
      return;
    }
    if (hasOptionNamed(def.options, name)) {
      setError(t("pageBoard.error.exists", { name }));
      return;
    }
    setError(null);
    try {
      await addPropertyOption(spaceId, def.id, name, nextOptionColor(def.options), def.kind === "status" ? "todo" : undefined);
      setAdding(null);
    } catch (cause) {
      setError(t("pageBoard.error.add", { message: toCoreError(cause).message }));
    }
  }

  return (
    <div ref={root} className="page-board" role="group" aria-label={t("pageBoard.label")}>
      <div className="page-board-columns">
        {shown.map((column) => {
          const pages = groups.get(column.key) ?? [];
          const cards = limitRows(pages, limit.limitOf(column.key));
          const isNone = column.key === NO_GROUP;
          const fold = isNone && collapsed;
          return (
            <section
              key={column.key}
              className="page-board-column"
              role="group"
              aria-label={nameOf(column)}
              data-key={column.key}
              data-over={over === column.key ? "" : undefined}
              data-collapsed={fold ? "" : undefined}
              onDragOver={editable ? (event) => dragOver(event, column.key) : undefined}
              onDragLeave={
                editable
                  ? (event) => {
                      const into = event.relatedTarget;
                      if (!(into instanceof Node && event.currentTarget.contains(into))) {
                        setOver((current) => (current === column.key ? null : current));
                      }
                    }
                  : undefined
              }
              onDrop={editable ? (event) => drop(event, column.key) : undefined}
            >
              <header className="page-board-header">
                {isNone && (
                  <button
                    type="button"
                    className="page-board-fold"
                    aria-expanded={!fold}
                    aria-label={t(fold ? "pageBoard.expand" : "pageBoard.collapse", { name: nameOf(column) })}
                    onClick={() => {
                      setCollapsed(!collapsed);
                    }}
                  >
                    {fold ? <ChevronIcon /> : <ChevronDownIcon />}
                  </button>
                )}
                {column.option === null ? (
                  <span className="page-board-none">{nameOf(column)}</span>
                ) : (
                  <OptionPill name={column.option.name} color={column.option.color} />
                )}
                {column.group !== null && <span className="page-board-group">{t(GROUP_LABELS[column.group])}</span>}
                <span className="page-board-count" aria-label={t("pageBoard.count", { count: pages.length })}>
                  {pages.length}
                </span>
              </header>
              {!fold && (
                <>
                  <ul className="page-board-cards">
                    {cards.map((page) => (
                      <li
                        key={page.id}
                        className="page-board-card"
                        data-page={page.id}
                        data-dragging={dragId === page.id ? "" : undefined}
                        data-selected={selection?.selected.has(page.id) === true ? "" : undefined}
                        draggable={editable}
                        onDragStart={
                          editable
                            ? (event) => {
                                event.dataTransfer.setData(DRAG_TYPE, page.id);
                                event.dataTransfer.effectAllowed = "move";
                                setDragId(page.id);
                              }
                            : undefined
                        }
                        onDragEnd={() => {
                          setDragId(null);
                          setOver(null);
                        }}
                      >
                        {selection !== undefined && (
                          <PageRowCheckbox
                            label={selectLabel(page.title)}
                            checked={selection.selected.has(page.id)}
                            onToggle={(range) => {
                              selection.onToggle(page.id, { range });
                            }}
                          />
                        )}
                        <button
                          type="button"
                          className="page-list-link page-board-link"
                          {...openGestures<HTMLButtonElement>((disposition) => {
                            onOpen(page, disposition);
                          })}
                        >
                          <PageIcon icon={page.icon} className="page-list-icon" />
                          <span className="page-list-name">{page.title === "" ? t("page.untitled") : page.title}</span>
                        </button>
                        {editable && (
                          <button
                            type="button"
                            className="page-board-more"
                            aria-haspopup="menu"
                            aria-label={t("pageBoard.cardMenu", { title: page.title === "" ? t("page.untitled") : page.title })}
                            onClick={(event) => {
                              const opener = event.currentTarget;
                              setMenu((current) =>
                                current?.page.id === page.id ? null : { page, anchor: anchorBelow(opener), trigger: opener },
                              );
                            }}
                          >
                            <MoreIcon />
                          </button>
                        )}
                        {columns.map((prop) => {
                          const value = valueOfPage(page, prop.id);
                          return value === undefined ? null : (
                            <div key={prop.id} className="page-board-prop" role="group" aria-label={prop.name} data-kind={prop.kind}>
                              <PageListValue def={prop} value={value} today={today} schema={schema} />
                            </div>
                          );
                        })}
                      </li>
                    ))}
                  </ul>
                  {cards.length < pages.length && (
                    <button
                      type="button"
                      className="page-list-more-button page-board-more-cards"
                      aria-label={t("pageList.rows.showMoreIn", {
                        name: nameOf(column),
                        count: Math.min(ROW_LIMIT_STEP, pages.length - cards.length),
                      })}
                      onClick={() => {
                        limit.showMore(column.key);
                      }}
                    >
                      {t("pageList.rows.showMore", { count: Math.min(ROW_LIMIT_STEP, pages.length - cards.length) })}
                    </button>
                  )}
                  {onCreatePage !== null && (
                    <div className="database-new-group">
                      <button
                        type="button"
                        className="page-board-new"
                        disabled={creating}
                        aria-label={t("pageBoard.newIn", { name: nameOf(column) })}
                        onClick={() => {
                          onCreatePage(initFor(column));
                        }}
                      >
                        <PlusIcon />
                        <span>{t("pageBoard.new")}</span>
                      </button>
                      <NewPageMenu
                        view={view}
                        onCreatePage={onCreatePage}
                        onUpdateView={onUpdateView}
                        init={initFor(column)}
                        disabled={creating}
                        label={t("databaseTemplate.menuIn", { name: nameOf(column) })}
                      />
                    </div>
                  )}
                </>
              )}
            </section>
          );
        })}
        {editable && (
          <div className="page-board-add">
            {adding === null ? (
              <button
                type="button"
                className="page-board-new"
                onClick={() => {
                  setAdding("");
                }}
              >
                <PlusIcon />
                <span>{t("pageBoard.addGroup")}</span>
              </button>
            ) : (
              <form onSubmit={(event) => void addGroup(event)}>
                <input
                  type="text"
                  className="page-board-input"
                  aria-label={t("pageBoard.addGroup.name")}
                  value={adding}
                  ref={(element) => element?.focus()}
                  onChange={(event) => {
                    setAdding(event.target.value);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") {
                      event.preventDefault();
                      setAdding(null);
                    }
                  }}
                />
              </form>
            )}
          </div>
        )}
      </div>
      {error !== null && (
        <p className="page-list-error" role="alert">
          {error}
        </p>
      )}
      {menu !== null && (
        <FloatingPanel
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          role="menu"
          label={t("pageBoard.moveMenu", { title: menu.page.title === "" ? t("page.untitled") : menu.page.title })}
          className="page-board-menu"
          onClose={() => {
            setMenu(null);
          }}
        >
          <p className="page-board-menu-title" aria-hidden="true">
            {t("pageBoard.moveTo")}
          </p>
          {boardColumns(def)
            .filter((column) => column.key !== columnOf(menu.page, def))
            .map((column) => (
              <button
                key={column.key}
                type="button"
                role="menuitem"
                className="page-board-menu-item"
                onClick={() => {
                  const id = menu.page.id;
                  setMenu(null);
                  void move(id, column.key);
                }}
              >
                {nameOf(column)}
              </button>
            ))}
        </FloatingPanel>
      )}
    </div>
  );
}
