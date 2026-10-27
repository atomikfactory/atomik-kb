// The toolbar of the column layout the selection is in (editable editors only): add a column,
// remove the current one, unwrap the layout. It floats above the layout, at its right edge
// (the table toolbar, `TableMenu`, takes the left one), and follows it while the page scrolls;
// while text is selected inside the layout, where the bubble menu (`FormatMenu`) sits above
// the text, it moves below the layout instead. The buttons are the commands of `columns.ts`.

import { autoUpdate, computePosition, flip, offset, shift } from "@floating-ui/dom";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { useEditorState } from "@tiptap/react";
import { useLayoutEffect, useState } from "react";
import { createPortal } from "react-dom";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { enclosingColumn } from "./columns";

type ColumnsAction = "addColumn" | "removeColumn" | "unwrapColumns";

interface ColumnsButton {
  action: ColumnsAction;
  /** The accessible name (and tooltip). */
  label: MessageKey;
  /** What the button shows. */
  text: MessageKey;
  danger?: boolean;
}

const BUTTONS: readonly ColumnsButton[] = [
  { action: "addColumn", label: "columns.addLabel", text: "columns.add" },
  { action: "removeColumn", label: "columns.removeLabel", text: "columns.remove", danger: true },
  { action: "unwrapColumns", label: "columns.unwrapLabel", text: "columns.unwrap" },
];

interface MenuState {
  /** Where the column list the selection is in starts; `null` shows no toolbar. */
  pos: number | null;
  /** Text (a range) is selected: the bubble menu is above it. */
  textSelected: boolean;
  /** Which commands apply to the selection right now. */
  can: Record<ColumnsAction, boolean>;
}

const NOTHING: Record<ColumnsAction, boolean> = { addColumn: false, removeColumn: false, unwrapColumns: false };

function readState(editor: Editor, keepOpen: boolean): MenuState {
  const { selection } = editor.state;
  const hidden: MenuState = { pos: null, textSelected: false, can: NOTHING };
  if (!editor.isEditable || !(editor.isFocused || keepOpen)) {
    return hidden;
  }
  const column = enclosingColumn(editor.state);
  if (column === null) {
    return hidden;
  }
  const can = editor.can();
  return {
    pos: column.list.pos,
    textSelected: selection instanceof TextSelection && !selection.empty,
    can: { addColumn: can.addColumn(), removeColumn: can.removeColumn(), unwrapColumns: can.unwrapColumns() },
  };
}

/** The element the toolbar is anchored to: what the column list renders as. */
function listElement(editor: Editor, pos: number): HTMLElement | null {
  try {
    const dom = editor.view.nodeDOM(pos);
    return dom instanceof HTMLElement ? dom : null;
  } catch {
    return null; // the view is gone
  }
}

export function ColumnsMenu({ editor }: { editor: Editor }) {
  const [hasFocus, setHasFocus] = useState(false);
  const [menu, setMenu] = useState<HTMLDivElement | null>(null);
  const state = useEditorState({ editor, selector: ({ editor: current }) => readState(current, hasFocus) });
  const reference = state.pos === null ? null : listElement(editor, state.pos);
  const below = state.textSelected;

  // Placed next to the layout, and again whenever the page scrolls or resizes. At its left end, as the table's
  // toolbar is: a column list is as wide as the page (ADR-0035), and its right end can be far from the text.
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
      aria-label={t("columns.label")}
      className="floating columns-menu"
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
      {BUTTONS.map((button) => (
        <button
          key={button.action}
          type="button"
          className={button.danger === true ? "columns-menu-button columns-menu-danger" : "columns-menu-button"}
          aria-label={t(button.label)}
          title={t(button.label)}
          disabled={!state.can[button.action]}
          onClick={() => {
            editor.chain().focus()[button.action]().run();
          }}
        >
          {t(button.text)}
        </button>
      ))}
    </div>,
    document.body,
  );
}
