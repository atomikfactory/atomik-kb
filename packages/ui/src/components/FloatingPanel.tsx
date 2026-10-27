import { useEffect, useEffectEvent, useLayoutEffect, useRef } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import { createPortal } from "react-dom";

/** A point in viewport coordinates where a panel opens (its top left corner). */
export interface Anchor {
  x: number;
  y: number;
}

/** The point just below an element's left edge: where a popover or menu for it opens. */
export function anchorBelow(element: Element): Anchor {
  const rect = element.getBoundingClientRect();
  return { x: rect.left, y: rect.bottom + 4 };
}

const FOCUSABLE =
  'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])';

/** The elements inside `container` that Tab can reach, in document order. */
export function focusableIn(container: HTMLElement): HTMLElement[] {
  // Sorted explicitly: some DOM implementations return the matches of a selector list
  // grouped by selector instead of in document order.
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE)).sort((a, b) =>
    a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1,
  );
}

/** Why a panel asks to be closed by itself: Escape, or a press outside it (a menu's Tab gives none). */
export type CloseReason = "escape" | "outside";

export interface FloatingPanelProps {
  anchor: Anchor;
  /** Accessible name of the panel. */
  label: string;
  /** `"menu"` adds arrow key navigation between its `menuitem`s (and the controls marked `data-menu-nav`). */
  role?: "dialog" | "menu";
  /** Closes the panel; `reason` says what asked for it (a popover that commits or cancels tells them apart). */
  onClose: (reason?: CloseReason) => void;
  /**
   * The element that opened the panel. Pressing it does not count as pressing outside,
   * so its own click handler can toggle the panel instead of reopening it.
   */
  anchorElement?: Element | null;
  /**
   * Where focus goes when the panel closes. Defaults to the element that had focus when
   * the panel opened, which is wrong when this panel replaces another one (a menu item
   * that opens a popover): name the element that opened the first of them.
   */
  returnFocus?: Element | null;
  /**
   * The element that takes focus when the panel opens, given the panel. The first element Tab can
   * reach is the default (and the panel itself when `initialFocus` finds none).
   */
  initialFocus?: (panel: HTMLElement) => HTMLElement | null;
  className?: string;
  children: ReactNode;
}

/**
 * A panel floating above the page at a fixed position (menus, popovers). It takes focus
 * when it opens and gives it back when it closes, and asks to be closed on Escape or on
 * a press outside. It is rendered into the document body, so the sidebar's scrolling
 * and clipping never cut it off.
 *
 * Events inside a portal still bubble through the React tree: open the panel from a
 * component that is not itself inside a clickable or key-handling element.
 */
export function FloatingPanel({
  anchor,
  label,
  role = "dialog",
  onClose,
  anchorElement,
  returnFocus,
  initialFocus,
  className,
  children,
}: FloatingPanelProps) {
  const panelRef = useRef<HTMLDivElement>(null);

  const focusInitial = useEffectEvent((panel: HTMLElement) => {
    (initialFocus?.(panel) ?? focusableIn(panel)[0] ?? panel).focus();
  });

  const closeIfOutside = useEffectEvent((event: PointerEvent) => {
    const target = event.target;
    if (!(target instanceof Node)) {
      return;
    }
    if (panelRef.current?.contains(target) === true || anchorElement?.contains(target) === true) {
      return;
    }
    onClose("outside");
  });

  const closeOnEscape = useEffectEvent((event: globalThis.KeyboardEvent) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      onClose("escape");
    }
  });

  // Gives focus back, unless it already went somewhere else on purpose. Without scrolling: an editor
  // that gets focus back would otherwise jump to its caret and leave the next panel away from its anchor.
  const restoreFocus = useEffectEvent((opener: HTMLElement | null, panel: HTMLElement) => {
    const active = document.activeElement;
    if (active !== null && active !== document.body && !panel.contains(active)) {
      return;
    }
    const target = returnFocus instanceof HTMLElement && returnFocus.isConnected ? returnFocus : opener;
    if (target?.isConnected === true) {
      target.focus({ preventScroll: true });
    }
  });

  useEffect(() => {
    const panel = panelRef.current;
    if (panel === null) {
      return;
    }
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    focusInitial(panel);
    document.addEventListener("pointerdown", closeIfOutside, true);
    document.addEventListener("keydown", closeOnEscape, true);
    return () => {
      document.removeEventListener("pointerdown", closeIfOutside, true);
      document.removeEventListener("keydown", closeOnEscape, true);
      restoreFocus(opener, panel);
    };
  }, []);

  // Keep the panel inside the window.
  useLayoutEffect(() => {
    const panel = panelRef.current;
    if (panel === null) {
      return;
    }
    const margin = 8;
    const rect = panel.getBoundingClientRect();
    if (rect.width > 0 && rect.right > window.innerWidth - margin) {
      panel.style.left = `${String(Math.max(margin, window.innerWidth - margin - rect.width))}px`;
    }
    if (rect.height > 0 && rect.bottom > window.innerHeight - margin) {
      panel.style.top = `${String(Math.max(margin, window.innerHeight - margin - rect.height))}px`;
    }
  }, []);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (role !== "menu") {
      return;
    }
    // The menu's commands, and the controls it carries that are not commands (`data-menu-nav`).
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled), [data-menu-nav]:not(:disabled)'),
    );
    const current = items.findIndex((item) => item === document.activeElement);
    let next: HTMLElement | undefined;
    switch (event.key) {
      case "ArrowDown":
        next = items[(current + 1) % items.length];
        break;
      case "ArrowUp":
        next = items[current <= 0 ? items.length - 1 : current - 1];
        break;
      case "Home":
        next = items[0];
        break;
      case "End":
        next = items[items.length - 1];
        break;
      case "Tab":
        onClose();
        return;
      default:
        return;
    }
    event.preventDefault();
    next?.focus();
  }

  return createPortal(
    <div
      ref={panelRef}
      role={role}
      aria-label={label}
      tabIndex={-1}
      className={className === undefined ? "floating" : `floating ${className}`}
      style={{ left: anchor.x, top: anchor.y }}
      onKeyDown={onKeyDown}
    >
      {children}
    </div>,
    document.body,
  );
}
