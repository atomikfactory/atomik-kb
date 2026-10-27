import { useEffect, useEffectEvent, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { createPortal } from "react-dom";

import { t } from "../i18n";
import { focusableIn } from "./FloatingPanel";

export interface TextDialogProps {
  title: string;
  body: string;
  /** The text, shown read-only and selected so that it can be copied. */
  text: string;
  onClose: () => void;
}

/**
 * A modal that shows text to copy by hand (where the clipboard cannot be written). The
 * text starts selected; Escape, the button and a click on the backdrop close it.
 */
export function TextDialog({ title, body, text, onClose }: TextDialogProps) {
  const titleId = useId();
  const bodyId = useId();
  const textId = useId();
  const area = useRef<HTMLTextAreaElement>(null);
  const [opener] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));

  const restoreFocus = useEffectEvent(() => {
    if (opener?.isConnected === true) {
      opener.focus();
    }
  });

  useEffect(() => {
    area.current?.focus();
    area.current?.select();
    return () => {
      restoreFocus();
    };
  }, []);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.key === "Escape") {
      event.stopPropagation();
      onClose();
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
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        className="dialog text-dialog"
        onKeyDown={onKeyDown}
      >
        <h2 className="subtitle" id={titleId}>
          {title}
        </h2>
        <p id={bodyId}>{body}</p>
        <label className="visually-hidden" htmlFor={textId}>
          {title}
        </label>
        <textarea ref={area} id={textId} className="input text-dialog-text" readOnly value={text} rows={12} />
        <div className="dialog-actions">
          <button type="button" className="button" onClick={onClose}>
            {t("dialog.close")}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
