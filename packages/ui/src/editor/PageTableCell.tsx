// The cells of the table layout (ADR-0016, amendment of 2026-10-08). A cell shows its value as the
// list view does and turns into the editor of the properties panel (`PropertyEditors.tsx`, which
// writes through `pagePropertySet` like the panel) when it is edited: a text field in place, the
// editor's own picker for dates and options (opened for the person), a box that is always live, and
// a popover over the cell for files and checklists. The page column edits the title in place and has
// the button that opens the page. A kind the table cannot edit, and every cell of a read-only
// editor, only shows the value.
//
// Which cell is active, and whether it is being edited, is the table's (`PageTableView.tsx`); a cell
// tells it when the person leaves it.

import { checklistProgress, toCoreError } from "@atomik-kb/core-client";
import type { PageInfo, PropertyDef, PropertyOption, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import { InlineEdit } from "../components/InlineEdit";
import { PageIcon } from "../components/PageIcon";
import { PropertyEditor } from "../components/PropertyEditors";
import { t, tp } from "../i18n";
import { openGestures } from "../page-open";
import type { OpenDisposition } from "../store-tabs";
import { parseDraft } from "../property-format";
import { useAppStore } from "../store-context";
import { valueOfPage } from "./page-list-model";
import { isEditableKind, isPopoverKind, isTextKind } from "./page-table-model";
import { PageListValue } from "./PageListValue";

/** Where a cell stands in the table. */
export interface CellState {
  active: boolean;
  editing: boolean;
  /** The one cell of the table Tab reaches (roving focus). */
  tabbable: boolean;
  /** Raised when focus should go (back) to the active cell. */
  focusTick: number;
}

export interface CellActions {
  /** Makes this the active cell. */
  onSelect: () => void;
  /** Makes it active and starts editing it. */
  onEdit: () => void;
  /** Ends the editing; `refocus` says that nothing else took focus. */
  onStop: (refocus: boolean) => void;
  /** A key pressed on the cell itself (not in its editor). */
  onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => void;
}

const NO_SCHEMA: readonly PropertyDef[] = [];

/** The floating panels a cell's editor can open: focus inside one is still in the cell's editor. */
const PANELS = '[role="dialog"], [role="menu"], [role="listbox"]';

interface FrameProps {
  label: string;
  colIndex: number;
  kind?: string;
  state: CellState;
  actions: CellActions;
  /** Runs before the editor sees a key (to commit on Enter). */
  onEditorKeyDownCapture?: (event: KeyboardEvent<HTMLDivElement>) => void;
  /** A click on the cell: whether it starts editing. */
  clickEdits: boolean;
  children: ReactNode;
}

/** The cell element: roving focus, the keys of the cell, and the end of the editing when focus leaves it. */
function CellFrame({ label, colIndex, kind, state, actions, onEditorKeyDownCapture, clickEdits, children }: FrameProps) {
  const ref = useRef<HTMLDivElement>(null);
  const { active, editing, tabbable, focusTick } = state;

  useEffect(() => {
    if (active && !editing && focusTick > 0) {
      ref.current?.focus();
    }
  }, [active, editing, focusTick]);

  /** Focus left the editor: look again once the browser has settled where it went. */
  function leave(): void {
    window.setTimeout(() => {
      const cell = ref.current;
      if (cell === null) {
        return;
      }
      const focus = document.activeElement;
      const nowhere = focus === null || focus === document.body;
      if (!nowhere && (cell.contains(focus) || focus.closest(PANELS) !== null)) {
        return;
      }
      actions.onStop(nowhere);
    }, 0);
  }

  return (
    <div
      ref={ref}
      role="gridcell"
      className="page-table-cell"
      aria-label={label}
      aria-colindex={colIndex}
      data-kind={kind}
      data-active={active ? "" : undefined}
      data-editing={editing ? "" : undefined}
      tabIndex={tabbable ? 0 : -1}
      onClick={() => {
        if (clickEdits) {
          actions.onEdit();
        } else {
          actions.onSelect();
        }
      }}
      onDoubleClick={() => {
        actions.onEdit();
      }}
      onKeyDownCapture={onEditorKeyDownCapture}
      onKeyDown={(event) => {
        if (event.target === event.currentTarget) {
          actions.onKeyDown(event);
        } else if (editing && event.key === "Escape") {
          // An editor that does not take Escape itself (a picker's trigger): the editing ends.
          event.preventDefault();
          event.stopPropagation();
          actions.onStop(true);
        }
      }}
      onBlur={editing ? leave : undefined}
    >
      {children}
    </div>
  );
}

// ----- the page column -----------------------------------------------------------------------

export interface TitleCellProps {
  page: PageInfo;
  colIndex: number;
  editable: boolean;
  state: CellState;
  actions: CellActions;
  /** Opens the page as the gesture on the open button asks (ADR-0034). */
  onOpen: (disposition: OpenDisposition) => void;
  onRename: (title: string) => Promise<void>;
}

/** The page column: icon and title (edited in place) and the button that opens the page. */
export function TitleCell({ page, colIndex, editable, state, actions, onOpen, onRename }: TitleCellProps) {
  const name = page.title === "" ? t("page.untitled") : page.title;
  const gestures = openGestures<HTMLButtonElement>(onOpen);
  return (
    <CellFrame label={t("pageList.sort.title")} colIndex={colIndex} kind="title" state={state} actions={actions} clickEdits={false}>
      <PageIcon icon={page.icon} className="page-list-icon" />
      {state.editing && editable ? (
        <InlineEdit
          className="page-table-title-edit"
          value={page.title}
          label={t("pageTable.titleEdit")}
          onCommit={onRename}
          onDone={() => {
            // Focus goes back to the cell, unless the edit ended because it went to something outside the table.
            const focus = document.activeElement;
            actions.onStop(focus === null || focus === document.body || focus.closest('[role="gridcell"]') !== null);
          }}
        />
      ) : (
        <span className="page-table-title">{name}</span>
      )}
      <button
        type="button"
        className="page-table-open"
        aria-label={t("pageTable.open", { title: name })}
        title={t("pageTable.open", { title: name })}
        tabIndex={state.active ? 0 : -1}
        {...gestures}
        onClick={(event) => {
          // The click is for the page, not for the cell.
          event.stopPropagation();
          gestures.onClick(event);
        }}
      >
        <span aria-hidden="true">↗</span>
      </button>
    </CellFrame>
  );
}

// ----- a property ----------------------------------------------------------------------------

/** The value as a cell shows it: the list's rendering, with a count for files and the progress of a checklist. */
function CellValue({ def, page, today, schema }: { def: PropertyDef; page: PageInfo; today: string; schema: readonly PropertyDef[] }) {
  const value = valueOfPage(page, def.id);
  if (value === undefined) {
    return null;
  }
  if (value.kind === "files") {
    return value.value.length === 0 ? null : <span className="page-list-text">{tp("pageTable.files", value.value.length)}</span>;
  }
  if (value.kind === "checklist") {
    const { done, total } = checklistProgress(value.value);
    return total === 0 ? null : <span className="page-list-text">{t("pageTable.checklist", { done, total })}</span>;
  }
  return <PageListValue def={def} value={value} today={today} schema={schema} />;
}

export interface PropertyCellProps {
  page: PageInfo;
  def: PropertyDef;
  spaceId: SpaceId;
  colIndex: number;
  editable: boolean;
  today: string;
  state: CellState;
  actions: CellActions;
  onDeleteOption: (def: PropertyDef, option: PropertyOption) => void;
  /** Tells the table what went wrong when a value could not be saved (its editor may be gone by then), and `null` as a save starts. */
  onSaveError: (message: string | null) => void;
}

/** The cell of a property of a page. */
export function PropertyCell({ page, def, spaceId, colIndex, editable, today, state, actions, onDeleteOption, onSaveError }: PropertyCellProps) {
  const setPageProperty = useAppStore((store) => store.setPageProperty);
  const schema = useAppStore((store) => store.propertySchemas[spaceId]) ?? NO_SCHEMA;
  const editorBox = useRef<HTMLDivElement>(null);
  const live = editable && isEditableKind(def.kind);
  const isBox = def.kind === "checkbox";
  const inPlace = live && state.editing && !isPopoverKind(def.kind) && !isBox;
  const opensPicker = def.kind === "date" || def.kind === "select" || def.kind === "status" || def.kind === "multi_select";

  // A picker is what the person came for: open the editor's own as the cell starts to be edited.
  useEffect(() => {
    if (inPlace && opensPicker) {
      editorBox.current?.querySelector<HTMLButtonElement>("button[aria-haspopup]")?.click();
    }
  }, [inPlace, opensPicker]);

  const editor = (autoFocus: boolean): ReactNode => (
    <PropertyEditor
      def={def}
      spaceId={spaceId}
      value={valueOfPage(page, def.id)}
      readOnly={false}
      autoFocus={autoFocus}
      commit={async (next) => {
        onSaveError(null);
        try {
          await setPageProperty(page.id, def.id, next);
        } catch (cause) {
          onSaveError(t("properties.error.save", { message: toCoreError(cause).message }));
          throw cause;
        }
      }}
      onDeleteOption={(option) => {
        onDeleteOption(def, option);
      }}
    />
  );

  /** Enter in a text field commits it by leaving the field, unless the text cannot be saved (the editor then says why). */
  function commitOnEnter(event: KeyboardEvent<HTMLDivElement>): void {
    const field = event.target;
    if (!inPlace || !isTextKind(def.kind) || event.key !== "Enter" || event.nativeEvent.isComposing) {
      return;
    }
    if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) || (field instanceof HTMLTextAreaElement && event.shiftKey)) {
      return;
    }
    if (parseDraft(def.kind, field.value).ok) {
      event.preventDefault();
      event.stopPropagation();
      field.blur();
    }
  }

  const [valueBox, setValueBox] = useState<HTMLDivElement | null>(null);
  let content: ReactNode;
  if (!live) {
    content = <CellValue def={def} page={page} today={today} schema={schema} />;
  } else if (isBox) {
    content = editor(false);
  } else if (inPlace) {
    content = (
      <div ref={editorBox} className="page-table-editor">
        {editor(true)}
      </div>
    );
  } else {
    content = <CellValue def={def} page={page} today={today} schema={schema} />;
  }

  // A cell that cannot be edited (a computed kind, a read-only editor) is only ever selected.
  const frameActions: CellActions = live ? actions : { ...actions, onEdit: actions.onSelect };
  return (
    <CellFrame
      label={def.name}
      colIndex={colIndex}
      kind={def.kind}
      state={state}
      actions={frameActions}
      clickEdits={live && !isBox}
      onEditorKeyDownCapture={commitOnEnter}
    >
      <div ref={setValueBox} className="page-table-value">
        {content}
      </div>
      {live && state.editing && isPopoverKind(def.kind) && valueBox !== null && (
        <FloatingPanel
          anchor={anchorBelow(valueBox)}
          anchorElement={valueBox}
          label={def.name}
          className="page-table-popover"
          onClose={() => {
            actions.onStop(true);
          }}
        >
          {editor(true)}
        </FloatingPanel>
      )}
    </CellFrame>
  );
}

