// The toolbar of the table the selection is in (editable editors only): add and delete rows
// and columns, turn the header row on or off, delete the table. It floats above the table
// and follows it while the page scrolls; while text is selected inside the table, where the
// bubble menu (`FormatMenu`) sits above the text, it moves below the table instead. The
// buttons are Tiptap's table commands (prosemirror-tables), which keep the ids of the nodes
// they keep and leave the new ones to `UniqueID`.

import { autoUpdate, computePosition, flip, offset, shift } from "@floating-ui/dom";
import { findParentNodeClosestToPos } from "@tiptap/core";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { useEditorState } from "@tiptap/react";
import { useLayoutEffect, useState } from "react";
import { createPortal } from "react-dom";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";

type TableAction =
  | "addRowBefore"
  | "addRowAfter"
  | "deleteRow"
  | "addColumnBefore"
  | "addColumnAfter"
  | "deleteColumn"
  | "toggleHeaderRow"
  | "deleteTable";

interface TableButton {
  action: TableAction;
  /** The accessible name (and tooltip). */
  label: MessageKey;
  /** What the button shows (the group's caption completes it). */
  text: MessageKey;
  danger?: boolean;
}

interface TableGroup {
  id: string;
  /** The caption and accessible name of the group; none for the buttons that act on the table. */
  caption: MessageKey | null;
  buttons: readonly TableButton[];
}

const GROUPS: readonly TableGroup[] = [
  {
    id: "row",
    caption: "table.row",
    buttons: [
      { action: "addRowBefore", label: "table.addRowAbove", text: "table.above" },
      { action: "addRowAfter", label: "table.addRowBelow", text: "table.below" },
      { action: "deleteRow", label: "table.deleteRow", text: "table.delete", danger: true },
    ],
  },
  {
    id: "column",
    caption: "table.column",
    buttons: [
      { action: "addColumnBefore", label: "table.addColumnBefore", text: "table.before" },
      { action: "addColumnAfter", label: "table.addColumnAfter", text: "table.after" },
      { action: "deleteColumn", label: "table.deleteColumn", text: "table.delete", danger: true },
    ],
  },
  {
    id: "table",
    caption: null,
    buttons: [
      { action: "toggleHeaderRow", label: "table.headerRow", text: "table.headerRow" },
      { action: "deleteTable", label: "table.deleteTable", text: "table.deleteTable", danger: true },
    ],
  },
];

interface MenuState {
  /** Where the table the selection is in starts; `null` shows no toolbar. */
  pos: number | null;
  /** Every cell of the table's first row is a header cell. */
  headerRow: boolean;
  /** Text (a range) is selected: the bubble menu is above it. */
  textSelected: boolean;
  /** Which commands apply to the selection right now. */
  can: Record<TableAction, boolean>;
}

const NOTHING: Record<TableAction, boolean> = {
  addRowBefore: false,
  addRowAfter: false,
  deleteRow: false,
  addColumnBefore: false,
  addColumnAfter: false,
  deleteColumn: false,
  toggleHeaderRow: false,
  deleteTable: false,
};

function readState(editor: Editor, keepOpen: boolean): MenuState {
  const { selection } = editor.state;
  const hidden: MenuState = { pos: null, headerRow: false, textSelected: false, can: NOTHING };
  if (!editor.isEditable || !(editor.isFocused || keepOpen)) {
    return hidden;
  }
  const table = findParentNodeClosestToPos(selection.$from, (node) => node.type.name === "table");
  if (table === undefined) {
    return hidden;
  }
  const first = table.node.firstChild;
  let headerRow = first !== null && first.childCount > 0;
  first?.forEach((cell) => {
    headerRow &&= cell.type.name === "tableHeader";
  });
  const can = editor.can();
  return {
    pos: table.pos,
    headerRow,
    textSelected: selection instanceof TextSelection && !selection.empty,
    can: {
      addRowBefore: can.addRowBefore(),
      addRowAfter: can.addRowAfter(),
      deleteRow: can.deleteRow(),
      addColumnBefore: can.addColumnBefore(),
      addColumnAfter: can.addColumnAfter(),
      deleteColumn: can.deleteColumn(),
      toggleHeaderRow: can.toggleHeaderRow(),
      deleteTable: can.deleteTable(),
    },
  };
}

/** The element the toolbar is anchored to: what the table node renders as. */
function tableElement(editor: Editor, pos: number): HTMLElement | null {
  try {
    const dom = editor.view.nodeDOM(pos);
    return dom instanceof HTMLElement ? dom : null;
  } catch {
    return null; // the view is gone
  }
}

export function TableMenu({ editor }: { editor: Editor }) {
  const [hasFocus, setHasFocus] = useState(false);
  const [menu, setMenu] = useState<HTMLDivElement | null>(null);
  const state = useEditorState({ editor, selector: ({ editor: current }) => readState(current, hasFocus) });
  const reference = state.pos === null ? null : tableElement(editor, state.pos);
  const below = state.textSelected;

  // Placed next to the table, and again whenever the page scrolls or resizes.
  useLayoutEffect(() => {
    if (menu === null || reference === null) {
      return;
    }
    return autoUpdate(reference, menu, () => {
      void computePosition(reference, menu, {
        strategy: "fixed",
        placement: below ? "bottom-start" : "top-start",
        middleware: [offset(6), flip(), shift({ padding: 8 })],
      }).then(({ x, y }) => {
        menu.style.left = `${String(x)}px`;
        menu.style.top = `${String(y)}px`;
      });
    });
  }, [menu, reference, below]);

  if (state.pos === null) {
    return null;
  }

  return createPortal(
    <div
      ref={setMenu}
      role="toolbar"
      aria-label={t("table.label")}
      className="floating table-menu"
      onFocus={() => {
        setHasFocus(true);
      }}
      onBlur={(event) => {
        if (!(event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget))) {
          setHasFocus(false);
        }
      }}
      onMouseDown={(event) => {
        event.preventDefault(); // keep the editor's selection: the commands work on it
      }}
    >
      {GROUPS.map((group) => (
        <div
          key={group.id}
          role="group"
          className="table-menu-group"
          aria-label={group.caption === null ? t("table.label") : t(group.caption)}
        >
          {group.caption !== null && (
            <span className="table-menu-caption" aria-hidden="true">
              {t(group.caption)}
            </span>
          )}
          {group.buttons.map((button) => (
            <button
              key={button.action}
              type="button"
              className={button.danger === true ? "table-menu-button table-menu-danger" : "table-menu-button"}
              aria-label={t(button.label)}
              title={t(button.label)}
              aria-pressed={button.action === "toggleHeaderRow" ? state.headerRow : undefined}
              disabled={!state.can[button.action]}
              onClick={() => {
                editor.chain().focus()[button.action]().run();
              }}
            >
              {t(button.text)}
            </button>
          ))}
        </div>
      ))}
    </div>,
    document.body,
  );
}
