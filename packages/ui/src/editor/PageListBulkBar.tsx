// The action bar of the selected rows of a database block (ADR-0058, `docs/architecture/database-power-ups.md` section 5.2). While
// at least one row is ticked it takes the place of the row of view tabs and toolbar: "{n} selected", "Edit property", "Move to…",
// "Duplicate", "Move to trash" and a button that clears the selection. Each action is one `PageBulk` request (`store-bulk.ts`), all
// or nothing, followed by a toast with "Undo".
//
// "Edit property" lists the properties that take a value (not the computed ones), then the bar shows the editor of the properties
// panel for the one chosen in place of the buttons (a picker an editor opens is a popover of its own, which a popover inside a
// popover could not hold), with "Apply" and "Clear". The editors commit what they hold to a draft; "Apply" sends the draft to every
// selected page (Enter in a text field does too) and "Clear" sends no value. After Duplicate the copies are selected (the owner
// does that: `onSelectOnly`), after Move and Trash the selection is cleared (`onClear`).

import { toCoreError } from "@atomik-kb/core-client";
import type { PageId, PageInfo, PageNode, PropertyDef, PropertyOption, PropertyValue, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { CloseIcon } from "../components/icons";
import { Menu } from "../components/Menu";
import { PageIcon } from "../components/PageIcon";
import { DeleteOptionConfirm } from "../components/PropertyConfirms";
import { PropertyEditor } from "../components/PropertyEditors";
import { activeLocale, t, tp } from "../i18n";
import { formatNumber } from "../property-format";
import { useAppStore } from "../store-context";
import { BULK_LIMIT } from "../store-bulk";
import { withoutTemplates } from "../template-tree";
import { flattenVisible } from "../tree";
import type { FlatRow } from "../tree";
import { isEditableKind } from "./page-table-model";

const NOTHING_COLLAPSED: ReadonlySet<PageId> = new Set();

export interface PageListBulkBarProps {
  spaceId: SpaceId;
  /** The ticked rows, in display order (their values are what Undo of a value change goes back to). */
  selected: readonly PageInfo[];
  /** The space's property schema, Tags first. */
  schema: readonly PropertyDef[];
  /** The close button: clears the selection. */
  onClear: () => void;
  /** Selects exactly these pages (the copies a Duplicate made). */
  onSelectOnly: (pageIds: readonly PageId[]) => void;
}

/** The properties a bulk edit can set: the kinds a cell edits, never the computed ones. */
export function bulkEditableProperties(schema: readonly PropertyDef[]): PropertyDef[] {
  return schema.filter((def) => isEditableKind(def.kind));
}

function shownTitle(title: string): string {
  return title === "" ? t("page.untitled") : title;
}

/**
 * The pages the selected ones can be moved under: every live page of the tree, in order, but not a template (nor what is in one),
 * not a selected page and not a page inside one (a page cannot go into itself), each with its depth (a filter lists the matches flat).
 */
export function moveTargets(roots: readonly PageNode[], selectedIds: ReadonlySet<PageId>, filter: string): FlatRow[] {
  const rows = flattenVisible(withoutTemplates(roots), NOTHING_COLLAPSED);
  const blocked = new Set<PageId>(selectedIds);
  const allowed: FlatRow[] = [];
  for (const row of rows) {
    // The tree is in preorder: a page's parent has been looked at before it.
    if (blocked.has(row.id) || (row.parentId !== null && blocked.has(row.parentId))) {
      blocked.add(row.id);
    } else {
      allowed.push(row);
    }
  }
  const query = filter.trim().toLocaleLowerCase(activeLocale());
  if (query === "") {
    return allowed;
  }
  return allowed
    .filter((row) => shownTitle(row.page.title).toLocaleLowerCase(activeLocale()).includes(query))
    .map((row) => ({ ...row, depth: 0 }));
}

interface MovePanelProps {
  anchor: Anchor;
  anchorElement: Element;
  selectedIds: ReadonlySet<PageId>;
  onPick: (parentId: PageId | null) => void;
  onClose: () => void;
}

/** "Move to…": the page picker of the space, with "Top level" first. A click on a page moves the selected rows under it at once. */
function MovePanel({ anchor, anchorElement, selectedIds, onPick, onClose }: MovePanelProps) {
  const tree = useAppStore((state) => state.tree);
  const [filter, setFilter] = useState("");
  const rows = useMemo(() => (tree === null ? [] : moveTargets(tree.roots, selectedIds, filter)), [tree, selectedIds, filter]);
  return (
    <FloatingPanel
      anchor={anchor}
      anchorElement={anchorElement}
      label={t("pageList.bulk.move.label")}
      className="page-list-bulk-move"
      onClose={onClose}
    >
      <input
        type="text"
        className="input page-list-bulk-filter"
        value={filter}
        aria-label={t("pageList.bulk.move.filter")}
        placeholder={t("pageList.bulk.move.filter")}
        autoComplete="off"
        onChange={(event) => {
          setFilter(event.target.value);
        }}
      />
      <ul className="page-list-bulk-targets">
        <li>
          <button
            type="button"
            className="page-list-bulk-target"
            onClick={() => {
              onPick(null);
            }}
          >
            {t("pageList.bulk.move.topLevel")}
          </button>
        </li>
        {rows.map((row) => (
          <li key={row.id}>
            <button
              type="button"
              className="page-list-bulk-target"
              style={{ "--depth": row.depth } as CSSProperties}
              onClick={() => {
                onPick(row.id);
              }}
            >
              <PageIcon icon={row.page.icon} className="tree-icon" />
              <span className="page-list-bulk-target-name">{shownTitle(row.page.title)}</span>
            </button>
          </li>
        ))}
        {rows.length === 0 && filter.trim() !== "" && <li className="page-list-bulk-empty">{t("pageList.bulk.move.empty")}</li>}
      </ul>
    </FloatingPanel>
  );
}

/** Whether Enter in `target` ends a line of text (a field that commits on Enter), as against a button or a checkbox. */
function isTextEntry(target: EventTarget, shift: boolean): boolean {
  if (target instanceof HTMLTextAreaElement) {
    return !shift;
  }
  return target instanceof HTMLInputElement && !["checkbox", "radio", "button", "submit"].includes(target.type);
}

interface ValueRowProps {
  def: PropertyDef;
  spaceId: SpaceId;
  busy: boolean;
  /** Sends the value to every selected page (`null` clears). Rejects when the core refuses (the bar says why). */
  onApply: (value: PropertyValue | null) => Promise<void>;
  onCancel: () => void;
}

/** The editor of one property in the bar, with "Apply", "Clear" and "Cancel". */
function ValueRow({ def, spaceId, busy, onApply, onCancel }: ValueRowProps) {
  // What the editor holds: the editors commit here, nothing is sent until "Apply" (or Enter in a text field).
  const [draft, setDraft] = useState<PropertyValue | undefined>(undefined);
  const latest = useRef<PropertyValue | undefined>(undefined);
  const [deleting, setDeleting] = useState<PropertyOption | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  function apply(value: PropertyValue | null): void {
    if (!busy) {
      onApply(value).catch(() => undefined);
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.key === "Enter" && !event.nativeEvent.isComposing && isTextEntry(event.target, event.shiftKey)) {
      // The field has already committed what it holds (its own handler runs first); a field left as it was commits nothing.
      if (latest.current !== undefined) {
        event.preventDefault();
        apply(latest.current);
      }
    }
  }

  return (
    <div className="page-list-bulk-edit" role="group" aria-label={t("pageList.bulk.editing", { property: def.name })} onKeyDown={onKeyDown}>
      <span className="page-list-bulk-property">{def.name}</span>
      <div className="page-list-bulk-editor">
        <PropertyEditor
          def={def}
          spaceId={spaceId}
          value={draft}
          readOnly={false}
          autoFocus
          commit={(next) => {
            latest.current = next ?? undefined;
            setDraft(next ?? undefined);
            return Promise.resolve();
          }}
          onDeleteOption={setDeleting}
        />
      </div>
      <button
        type="button"
        className="page-list-button page-list-bulk-apply"
        disabled={busy || draft === undefined}
        onClick={() => {
          if (draft !== undefined) {
            apply(draft);
          }
        }}
      >
        {t("pageList.bulk.apply")}
      </button>
      <button
        type="button"
        className="page-list-button"
        disabled={busy}
        onClick={() => {
          apply(null);
        }}
      >
        {t("pageList.bulk.clearValue")}
      </button>
      <button type="button" className="page-list-button" onClick={onCancel}>
        {t("pageList.bulk.cancel")}
      </button>
      {problem !== null && (
        <p className="page-list-error" role="alert">
          {problem}
        </p>
      )}
      {deleting !== null && (
        <DeleteOptionConfirm
          spaceId={spaceId}
          def={def}
          option={deleting}
          onError={setProblem}
          onClose={() => {
            setDeleting(null);
          }}
        />
      )}
    </div>
  );
}

/** The bar of the selected rows. */
export function PageListBulkBar({ spaceId, selected, schema, onClear, onSelectOnly }: PageListBulkBarProps) {
  const bulkSetValue = useAppStore((state) => state.bulkSetValue);
  const bulkMovePages = useAppStore((state) => state.bulkMovePages);
  const bulkDuplicatePages = useAppStore((state) => state.bulkDuplicatePages);
  const bulkTrashPages = useAppStore((state) => state.bulkTrashPages);
  const [menu, setMenu] = useState<{ kind: "properties" | "move"; anchor: Anchor; trigger: Element } | null>(null);
  const [editing, setEditing] = useState<PropertyDef | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const editButton = useRef<HTMLButtonElement>(null);
  const wasEditing = useRef(false);
  // A request takes at most 1,000 pages: a larger selection can be cleared, not acted on.
  const tooMany = selected.length > BULK_LIMIT;

  const editable = useMemo(() => bulkEditableProperties(schema), [schema]);
  const ids = useMemo(() => selected.map((page) => page.id), [selected]);
  const idSet = useMemo(() => new Set(ids), [ids]);

  // Leaving the editor (Apply, Clear, Cancel) puts focus back on the button that opened it.
  useEffect(() => {
    if (wasEditing.current && editing === null) {
      editButton.current?.focus();
    }
    wasEditing.current = editing !== null;
  }, [editing]);

  /** Runs one action; its failure is said in the bar and the selection stays. */
  async function run(action: () => Promise<void>): Promise<void> {
    if (busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(t("pageList.bulk.error", { message: toCoreError(cause).message }));
      throw cause;
    } finally {
      setBusy(false);
    }
  }

  function perform(action: () => Promise<void>): void {
    run(action).catch(() => undefined);
  }

  /** The bar goes with the selection: focus goes to the first checkbox of the block (the header's "Select all"), not to nowhere. */
  function clearSelection(): void {
    const host = root.current?.closest(".page-list") ?? null;
    onClear();
    window.requestAnimationFrame(() => {
      host?.querySelector<HTMLElement>("[data-row-select]")?.focus({ preventScroll: true });
    });
  }

  function toggleMenu(kind: "properties" | "move", trigger: Element): void {
    setMenu((current) => (current?.kind === kind ? null : { kind, anchor: anchorBelow(trigger), trigger }));
  }

  return (
    <div ref={root} className="page-list-bulk" role="group" aria-label={t("pageList.bulk.label")}>
      <span className="page-list-bulk-count" role="status">
        {tp("pageList.bulk.count", selected.length)}
      </span>
      {tooMany && (
        <span className="page-list-bulk-hint">{t("pageList.bulk.tooMany", { max: formatNumber(BULK_LIMIT) })}</span>
      )}
      {editing === null ? (
        <>
          <button
            ref={editButton}
            type="button"
            className="page-list-button"
            aria-haspopup="menu"
            aria-expanded={menu?.kind === "properties"}
            disabled={busy || tooMany || editable.length === 0}
            title={editable.length === 0 ? t("pageList.bulk.noProperties") : undefined}
            onClick={(event) => {
              toggleMenu("properties", event.currentTarget);
            }}
          >
            {t("pageList.bulk.editProperty")}
          </button>
          <button
            type="button"
            className="page-list-button"
            aria-haspopup="dialog"
            aria-expanded={menu?.kind === "move"}
            disabled={busy || tooMany}
            onClick={(event) => {
              toggleMenu("move", event.currentTarget);
            }}
          >
            {t("pageList.bulk.move")}
          </button>
          <button
            type="button"
            className="page-list-button"
            disabled={busy || tooMany}
            onClick={() => {
              perform(async () => {
                onSelectOnly(await bulkDuplicatePages(spaceId, ids));
              });
            }}
          >
            {t("pageList.bulk.duplicate")}
          </button>
          <button
            type="button"
            className="page-list-button page-list-bulk-danger"
            disabled={busy || tooMany}
            onClick={() => {
              perform(async () => {
                await bulkTrashPages(spaceId, ids);
                clearSelection();
              });
            }}
          >
            {t("pageList.bulk.trash")}
          </button>
        </>
      ) : (
        <ValueRow
          key={editing.id}
          def={editing}
          spaceId={spaceId}
          busy={busy}
          onApply={(value) =>
            run(async () => {
              await bulkSetValue(spaceId, selected, editing.id, value);
              setEditing(null);
            })
          }
          onCancel={() => {
            setEditing(null);
          }}
        />
      )}
      <button
        type="button"
        className="page-list-bulk-close"
        aria-label={t("pageList.bulk.close")}
        title={t("pageList.bulk.close")}
        onClick={clearSelection}
      >
        <CloseIcon />
      </button>
      {error !== null && (
        <p className="page-list-error page-list-bulk-error" role="alert">
          {error}
        </p>
      )}
      {menu?.kind === "properties" && (
        <Menu
          label={t("pageList.bulk.editProperty.menu")}
          items={editable.map((def) => ({ id: def.id, label: def.name }))}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(id) => {
            setEditing(editable.find((def) => def.id === id) ?? null);
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
      {menu?.kind === "move" && (
        <MovePanel
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          selectedIds={idSet}
          onPick={(parentId) => {
            setMenu(null);
            perform(async () => {
              await bulkMovePages(spaceId, ids, parentId);
              clearSelection();
            });
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </div>
  );
}
