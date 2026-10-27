import type { ReactNode } from "react";

import { FloatingPanel } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";

export interface MenuItem {
  id: string;
  label: string;
  disabled?: boolean;
  /** Tooltip, for example why an item is disabled. */
  title?: string;
  /** Styles the item as destructive. */
  danger?: boolean;
  /**
   * The keys that do the same, as a label ("Ctrl+P"): shown at the item's right edge, announced as the item's
   * keyboard shortcut (`aria-keyshortcuts`) and not as a part of its name.
   */
  hint?: string;
}

/** A label of keys ("Ctrl+Shift+P", "Cmd+P") as `aria-keyshortcuts` writes them ("Control+Shift+P", "Meta+P"). */
function keyShortcuts(hint: string): string {
  return hint
    .split("+")
    .map((key) => (key === "Ctrl" ? "Control" : key === "Cmd" ? "Meta" : key))
    .join("+");
}

export interface MenuProps {
  items: readonly MenuItem[];
  anchor: Anchor;
  /** Accessible name of the menu. */
  label: string;
  /** Called with the id of the chosen item, after the menu asked to be closed. */
  onSelect: (id: string) => void;
  onClose: () => void;
  anchorElement?: Element | null;
  /**
   * Controls after the commands that do not close the menu (the page menu's font and switches): rendered below
   * a rule, in the same panel. The arrow keys also reach the ones marked `data-menu-nav`.
   */
  footer?: ReactNode;
}

/** A menu of commands: Up, Down, Home and End move between them, Escape closes it. */
export function Menu({ items, anchor, label, onSelect, onClose, anchorElement, footer }: MenuProps) {
  return (
    <FloatingPanel role="menu" label={label} anchor={anchor} onClose={onClose} anchorElement={anchorElement}>
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          role="menuitem"
          className={[
            "menu-item",
            item.danger === true ? "menu-item-danger" : "",
            item.hint === undefined ? "" : "menu-item-with-hint",
          ]
            .filter((name) => name !== "")
            .join(" ")}
          disabled={item.disabled}
          title={item.title}
          aria-keyshortcuts={item.hint === undefined ? undefined : keyShortcuts(item.hint)}
          onClick={() => {
            onClose();
            onSelect(item.id);
          }}
        >
          {item.hint === undefined ? (
            item.label
          ) : (
            <>
              <span>{item.label}</span>
              <span className="menu-item-hint" aria-hidden="true">
                {item.hint}
              </span>
            </>
          )}
        </button>
      ))}
      {footer}
    </FloatingPanel>
  );
}
