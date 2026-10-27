import { useEffect, useEffectEvent, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode, RefObject } from "react";
import { createPortal } from "react-dom";

import { focusableIn } from "./FloatingPanel";

export interface ModalProps {
  /** Id of the element that names the dialog (its heading). */
  labelledBy: string;
  /** Id of the element that describes it, if any. */
  describedBy?: string;
  /**
   * Called on Escape and on a click on the backdrop. Leave it out while the dialog must
   * not be dismissed that way (it then closes only through its own buttons).
   */
  onDismiss?: () => void;
  className?: string;
  /** Takes focus when the dialog opens; by default the first focusable element inside. */
  initialFocus?: RefObject<HTMLElement | null>;
  /**
   * Gets focus back when the dialog closes. By default the element that had focus when it
   * opened, which is wrong when that element was disabled meanwhile (a busy button).
   */
  returnFocus?: RefObject<HTMLElement | null>;
  children: ReactNode;
}

/**
 * A modal dialog for content of its own (forms, the recovery kit). Focus moves in when
 * it opens, stays inside while it is up (Tab and Shift+Tab wrap around) and returns to
 * where it came from when it closes.
 */
export function Modal({
  labelledBy,
  describedBy,
  onDismiss,
  className,
  initialFocus,
  returnFocus,
  children,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  // Read while rendering, before anything inside the dialog takes focus.
  const [opener] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));

  const focusFirst = useEffectEvent(() => {
    const dialog = dialogRef.current;
    if (dialog !== null) {
      (initialFocus?.current ?? focusableIn(dialog)[0] ?? dialog).focus();
    }
  });

  const restoreFocus = useEffectEvent(() => {
    const preferred = returnFocus?.current;
    const target = preferred?.isConnected === true ? preferred : opener;
    if (target?.isConnected === true) {
      target.focus();
    }
  });

  // Set here rather than with `autoFocus` so that it also holds when React runs this effect
  // twice (StrictMode) and the first cleanup hands focus back to the opener.
  useEffect(() => {
    focusFirst();
    return () => {
      restoreFocus();
    };
  }, []);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    // Keys from a dialog opened on top (another portal) bubble here through React: theirs.
    if (!(event.target instanceof Node) || !event.currentTarget.contains(event.target)) {
      return;
    }
    if (event.key === "Escape") {
      event.stopPropagation();
      onDismiss?.();
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const items = focusableIn(event.currentTarget);
    const first = items[0];
    const last = items[items.length - 1];
    if (first === undefined || last === undefined) {
      event.preventDefault();
      return;
    }
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return createPortal(
    <div
      className="dialog-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) {
          onDismiss?.();
        }
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        tabIndex={-1}
        className={className === undefined ? "dialog" : `dialog ${className}`}
        onKeyDown={onKeyDown}
      >
        {children}
      </div>
    </div>,
    document.body,
  );
}
