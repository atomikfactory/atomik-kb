// "Move to…" of a page (row menu of the sidebar, page menu), ADR-0038: moving a page and its sub-pages to
// another space, keeping their ids. Three steps in one dialog: the other spaces (name, icon, how many pages), the
// target's live tree as a picker ("Top level" first, a filter box by title), and the confirmation the core's
// plan gives ("Move 12 pages to Work", then only the lines that are not zero: properties created, values that
// cannot follow, relation links removed, attachments that stay in the source). A refusal is shown in the dialog.
// When the move is done the sidebar is in the target space with the page open (`movePageToSpace`).

import { toCoreError } from "@atomik-kb/core-client";
import type { PageId, SpaceId, SpaceInfo, TreeView } from "@atomik-kb/core-client";
import { useEffect, useEffectEvent, useId, useMemo, useRef, useState } from "react";
import type { CSSProperties } from "react";

import { formatPageCount } from "../format";
import { activeLocale, t, tp } from "../i18n";
import { useAppStore } from "../store-context";
import type { SpaceMoveSummary } from "../store-structure";
import { withoutTemplates } from "../template-tree";
import { flattenVisible, pathTo } from "../tree";
import type { FlatRow } from "../tree";
import { Modal } from "./Modal";
import { PageIcon } from "./PageIcon";

export interface MoveToSpaceDialogProps {
  /** The page to move; its sub-pages go with it. */
  pageId: PageId;
  /** Its title as the sidebar shows it; empty for an untitled page. */
  title: string;
  /** The space the page is in now. */
  spaceId: SpaceId;
  /** Where focus goes when the dialog closes (the control that opened the menu). */
  returnFocus?: Element | null;
  onClose: () => void;
}

type Step = "space" | "place" | "confirm";

/** Something read from the core: on its way, there, or refused. */
type Read<T> = { status: "loading" } | { status: "ready"; value: T } | { status: "error"; message: string };

/** What `read` answers for `key`, read again whenever `key` changes; with `null` nothing is read (and the answer is "loading"). */
function useRead<T>(key: string | null, read: () => Promise<T>): Read<T> {
  const [answer, setAnswer] = useState<{ key: string; read: Read<T> } | null>(null);
  const ask = useEffectEvent(read);
  useEffect(() => {
    if (key === null) {
      return;
    }
    let current = true;
    ask().then(
      (value) => {
        if (current) {
          setAnswer({ key, read: { status: "ready", value } });
        }
      },
      (failure: unknown) => {
        if (current) {
          setAnswer({ key, read: { status: "error", message: toCoreError(failure).message } });
        }
      },
    );
    return () => {
      current = false;
    };
  }, [key]);
  return answer !== null && answer.key === key ? answer.read : { status: "loading" };
}

const NOTHING_COLLAPSED: ReadonlySet<PageId> = new Set();

/** The lines of the confirmation that are not zero, in the order the design gives them. */
export function moveNotes(summary: SpaceMoveSummary, target: string, source: string): string[] {
  const notes: string[] = [];
  if (summary.propertiesCreated > 0) {
    notes.push(tp("moveToSpace.properties", summary.propertiesCreated, { space: target }));
  }
  if (summary.valuesDropped > 0) {
    notes.push(tp("moveToSpace.values", summary.valuesDropped));
  }
  if (summary.relationLinksRemoved > 0) {
    notes.push(tp("moveToSpace.relations", summary.relationLinksRemoved));
  }
  if (summary.attachmentsLeft > 0) {
    notes.push(tp("moveToSpace.attachments", summary.attachmentsLeft, { space: source }));
  }
  return notes;
}

function shownTitle(title: string): string {
  return title === "" ? t("page.untitled") : title;
}

/** The pages of the target as picker rows: the whole tree in order, or the pages whose title has `filter`, each with where it is. */
function pickerRows(tree: TreeView | null, filter: string): { row: FlatRow; path: string }[] {
  if (tree === null) {
    return [];
  }
  // A template (and what is inside one) is no place to put a page (ADR-0056).
  const rows = flattenVisible(withoutTemplates(tree.roots), NOTHING_COLLAPSED);
  const query = filter.trim().toLocaleLowerCase(activeLocale());
  if (query === "") {
    return rows.map((row) => ({ row, path: "" }));
  }
  const byId = new Map(rows.map((row) => [row.id, row]));
  const pathOf = (row: FlatRow): string => {
    const names: string[] = [];
    for (let parent = row.parentId === null ? undefined : byId.get(row.parentId); parent !== undefined; ) {
      names.unshift(shownTitle(parent.page.title));
      parent = parent.parentId === null ? undefined : byId.get(parent.parentId);
    }
    return names.join(" / ");
  };
  return rows
    .filter((row) => shownTitle(row.page.title).toLocaleLowerCase(activeLocale()).includes(query))
    .map((row) => ({ row: { ...row, depth: 0 }, path: pathOf(row) }));
}

export function MoveToSpaceDialog({ pageId, title, spaceId, returnFocus, onClose }: MoveToSpaceDialogProps) {
  const spaces = useAppStore((state) => state.spaces);
  const readTree = useAppStore((state) => state.readTree);
  const planMove = useAppStore((state) => state.planPageMoveToSpace);
  const movePage = useAppStore((state) => state.movePageToSpace);
  const [step, setStep] = useState<Step>("space");
  const [target, setTarget] = useState<SpaceInfo | null>(null);
  const [parentId, setParentId] = useState<PageId | null>(null);
  const [filter, setFilter] = useState("");
  // How many times each of the steps that read from the core was entered: it is read again every time.
  const [visits, setVisits] = useState({ place: 0, confirm: 0 });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const stepId = useId();
  const groupName = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const filterBox = useRef<HTMLInputElement>(null);
  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);

  const sourceName = spaces.find((space) => space.id === spaceId)?.name ?? "";
  const others = spaces.filter((space) => space.id !== spaceId);
  // The tree of the target is read when the picker is entered and kept for the confirmation; the plan is asked
  // for each time the confirmation is entered.
  const treeRead = useRead<TreeView>(target === null ? null : `${target.id}:${String(visits.place)}`, () =>
    readTree(target?.id ?? ""),
  );
  const tree = treeRead.status === "ready" ? treeRead.value : null;
  // A parent that has gone since the tree was read is not a place any more.
  const parent = tree !== null && parentId !== null && pathTo(tree.roots, parentId) === null ? null : parentId;
  const planRead = useRead<SpaceMoveSummary>(
    step === "confirm" && target !== null ? `${target.id}:${parent ?? ""}:${String(visits.confirm)}` : null,
    () => planMove(pageId, target?.id ?? "", parent),
  );

  // The buttons of a step go with it: focus moves to the new step (the filter box, else the heading) so that it stays in the dialog.
  const shownStep = useRef(step);
  useEffect(() => {
    if (shownStep.current !== step) {
      shownStep.current = step;
      (step === "place" ? filterBox.current : heading.current)?.focus();
    }
  }, [step]);

  const rows = useMemo(() => pickerRows(tree, filter), [tree, filter]);
  const parentTitle = useMemo(() => {
    if (tree === null || parent === null) {
      return "";
    }
    const chain = pathTo(tree.roots, parent);
    return shownTitle(chain?.[chain.length - 1]?.title ?? "");
  }, [tree, parent]);
  const notes = planRead.status === "ready" && target !== null ? moveNotes(planRead.value, target.name, sourceName) : [];

  function go(next: Step): void {
    setError(null);
    if (next !== "space") {
      setVisits((before) => ({ ...before, [next]: before[next] + 1 }));
    }
    setStep(next);
  }

  function choose(space: SpaceInfo): void {
    setTarget(space);
    setParentId(null);
    setFilter("");
    go("place");
  }

  async function move(): Promise<void> {
    if (busy || target === null || planRead.status !== "ready") {
      return;
    }
    setError(null);
    setBusy(true);
    try {
      // The parent and its parents are expanded in the target's sidebar, so the page shows where it went.
      const reveal = parent === null || tree === null ? [] : (pathTo(tree.roots, parent) ?? []).map((page) => page.id);
      await movePage({ pageId, fromSpaceId: spaceId, toSpaceId: target.id, parentId: parent, reveal });
      onClose();
    } catch (failure) {
      setError(t("moveToSpace.error", { message: toCoreError(failure).message }));
      setBusy(false);
    }
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={stepId}
      onDismiss={busy ? undefined : onClose}
      className="move-dialog"
      returnFocus={returnRef}
    >
      <h2 className="subtitle" id={titleId} ref={heading} tabIndex={-1}>
        {t("moveToSpace.title", { name: shownTitle(title) })}
      </h2>

      {step === "space" && (
        <>
          <p id={stepId}>{t("moveToSpace.step.space")}</p>
          {others.length === 0 ? (
            <p className="muted">{t("moveToSpace.noSpaces")}</p>
          ) : (
            <ul className="space-list move-spaces" aria-label={t("moveToSpace.spacesLabel")}>
              {others.map((space) => (
                <li key={space.id} className="space-row">
                  <button
                    type="button"
                    className="space-item"
                    onClick={() => {
                      choose(space);
                    }}
                  >
                    <PageIcon icon={space.icon} kind="space" className="tree-icon" />
                    <span className="space-item-name" title={space.name}>
                      {space.name}
                    </span>
                    <span className="muted move-space-count">{formatPageCount(space.pageCount)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" onClick={onClose}>
              {t("dialog.cancel")}
            </button>
          </div>
        </>
      )}

      {step === "place" && target !== null && (
        <>
          <p id={stepId}>{t("moveToSpace.step.place", { space: target.name })}</p>
          <input
            ref={filterBox}
            type="text"
            className="input move-filter"
            value={filter}
            aria-label={t("moveToSpace.filterLabel")}
            placeholder={t("moveToSpace.filterPlaceholder")}
            autoComplete="off"
            onChange={(event) => {
              setFilter(event.target.value);
            }}
            onKeyDown={(event) => {
              // Enter in the filter box filters; it does not go on to the next step.
              if (event.key === "Enter") {
                event.preventDefault();
              }
            }}
          />
          {treeRead.status === "loading" && (
            <p className="muted" role="status">
              {t("moveToSpace.loadingTree", { space: target.name })}
            </p>
          )}
          {treeRead.status === "error" && (
            <p className="error-text" role="alert">
              {t("moveToSpace.treeError", { space: target.name, message: treeRead.message })}
            </p>
          )}
          {treeRead.status === "ready" && (
            <fieldset className="fieldset move-picker">
              <legend>{t("moveToSpace.placeLabel")}</legend>
              <div className="move-options">
                <div className="move-option">
                  <input
                    type="radio"
                    id={`${groupName}-top`}
                    name={groupName}
                    checked={parent === null}
                    onChange={() => {
                      setParentId(null);
                    }}
                  />
                  <label htmlFor={`${groupName}-top`}>{t("moveToSpace.topLevel")}</label>
                </div>
                {rows.map(({ row, path }) => {
                  const inputId = `${groupName}-${row.id}`;
                  const hintId = `${inputId}-path`;
                  return (
                    <div
                      key={row.id}
                      className="move-option"
                      data-depth={row.depth}
                      style={{ "--depth": row.depth } as CSSProperties}
                    >
                      <input
                        type="radio"
                        id={inputId}
                        name={groupName}
                        checked={parent === row.id}
                        aria-describedby={path === "" ? undefined : hintId}
                        onChange={() => {
                          setParentId(row.id);
                        }}
                      />
                      <label htmlFor={inputId}>
                        <PageIcon icon={row.page.icon} className="tree-icon" />
                        <span>{shownTitle(row.page.title)}</span>
                      </label>
                      {path !== "" && (
                        <span className="muted move-option-path" id={hintId}>
                          {path}
                        </span>
                      )}
                    </div>
                  );
                })}
                {rows.length === 0 && filter.trim() !== "" && <p className="muted">{t("moveToSpace.noMatches")}</p>}
              </div>
            </fieldset>
          )}
          <div className="dialog-actions">
            <button
              type="button"
              className="button"
              onClick={() => {
                go("space");
              }}
            >
              {t("moveToSpace.back")}
            </button>
            <button type="button" className="button" onClick={onClose}>
              {t("dialog.cancel")}
            </button>
            <button
              type="button"
              className="button button-primary"
              disabled={treeRead.status !== "ready"}
              onClick={() => {
                go("confirm");
              }}
            >
              {t("moveToSpace.next")}
            </button>
          </div>
        </>
      )}

      {step === "confirm" && target !== null && (
        <>
          {planRead.status === "ready" ? (
            <>
              <p id={stepId} className="move-summary">
                {tp("moveToSpace.confirm", planRead.value.pageIds.length, { space: target.name })}
              </p>
              <p className="muted">
                {parent === null ? t("moveToSpace.where.top") : t("moveToSpace.where.under", { parent: parentTitle })}
              </p>
              {notes.length > 0 && (
                <ul className="move-notes">
                  {notes.map((note) => (
                    <li key={note}>{note}</li>
                  ))}
                </ul>
              )}
            </>
          ) : (
            <p id={stepId} className="muted" role={planRead.status === "error" ? "alert" : "status"}>
              {planRead.status === "error"
                ? t("moveToSpace.planError", { message: planRead.message })
                : t("moveToSpace.checking")}
            </p>
          )}
          {error !== null && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button
              type="button"
              className="button"
              disabled={busy}
              onClick={() => {
                go("place");
              }}
            >
              {t("moveToSpace.back")}
            </button>
            <button type="button" className="button" disabled={busy} onClick={onClose}>
              {t("dialog.cancel")}
            </button>
            <button
              type="button"
              className="button button-primary"
              disabled={busy || planRead.status !== "ready"}
              onClick={() => {
                void move();
              }}
            >
              {busy ? t("moveToSpace.moving") : t("moveToSpace.move")}
            </button>
          </div>
        </>
      )}
    </Modal>
  );
}
