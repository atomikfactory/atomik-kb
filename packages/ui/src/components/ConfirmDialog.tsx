import { useEffect, useEffectEvent, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { createPortal } from "react-dom";

import { t } from "../i18n";
import { focusableIn } from "./FloatingPanel";

export interface ConfirmDialogProps {
  title: string;
  body: string;
  /** Label of the confirming button; name the action ("Delete permanently"), not "OK". */
  confirmLabel: string;
  /**
   * Runs when the user confirms. The dialog stays up, busy, until it settles and then
   * closes. It is expected to report its own failures: a rejection is ignored here.
   */
  onConfirm: () => Promise<void>;
  onClose: () => void;
  /**
   * Where focus goes when the dialog closes. Defaults to the element that had focus when
   * it opened, which is wrong when the dialog replaces a menu: name the element that
   * opened the menu.
   */
  returnFocus?: Element | null;
  /**
   * Asks the user to type `phrase` (compared exactly, after trimming) before the confirming button
   * works; `label` names the field. For what cannot be taken back and takes much with it.
   */
  typeToConfirm?: { phrase: string; label: string };
}

/**
 * A modal question for a destructive action. Focus starts on the safe choice (cancel),
 * stays inside while the dialog is up, and returns to where it came from afterwards.
 * Escape and a click on the backdrop cancel.
 */
export function ConfirmDialog({ title, body, confirmLabel, onConfirm, onClose, returnFocus, typeToConfirm }: ConfirmDialogProps) {
  const titleId = useId();
  const bodyId = useId();
  const typedId = useId();
  const [busy, setBusy] = useState(false);
  const [typed, setTyped] = useState("");
  const confirmable = typeToConfirm === undefined || typed.trim() === typeToConfirm.phrase;
  // Read while rendering, before anything inside the dialog takes focus.
  const [opener] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));

  const restoreFocus = useEffectEvent(() => {
    const target = returnFocus instanceof HTMLElement && returnFocus.isConnected ? returnFocus : opener;
    if (target?.isConnected === true) {
      target.focus();
    }
  });

  // Focus is set here rather than with `autoFocus` so that it also holds when React runs
  // this effect twice (StrictMode) and the first cleanup hands focus back to the opener.
  const cancelButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    cancelButton.current?.focus();
    return () => {
      restoreFocus();
    };
  }, []);

  async function confirm(): Promise<void> {
    setBusy(true);
    try {
      await onConfirm();
    } catch {
      // The caller reports failures; the dialog closes either way.
    }
    onClose();
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.key === "Escape") {
      event.stopPropagation();
      if (!busy) {
        onClose();
      }
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const items = focusableIn(event.currentTarget);
    const first = items[0];
    const last = items[items.length - 1];
    if (first === undefined || last === undefined) {
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
        if (event.target === event.currentTarget && !busy) {
          onClose();
        }
      }}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        className="dialog"
        onKeyDown={onKeyDown}
      >
        <h2 className="subtitle" id={titleId}>
          {title}
        </h2>
        <p id={bodyId}>{body}</p>
        {typeToConfirm !== undefined && (
          <div className="dialog-field">
            <label htmlFor={typedId}>{typeToConfirm.label}</label>
            <input
              id={typedId}
              className="input"
              type="text"
              value={typed}
              autoComplete="off"
              spellCheck={false}
              disabled={busy}
              onChange={(event) => {
                setTyped(event.target.value);
              }}
            />
          </div>
        )}
        <div className="dialog-actions">
          <button ref={cancelButton} type="button" className="button" disabled={busy} onClick={onClose}>
            {t("dialog.cancel")}
          </button>
          <button
            type="button"
            className="button button-danger"
            disabled={busy || !confirmable}
            onClick={() => {
              void confirm();
            }}
          >
            {busy ? t("dialog.working") : confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
