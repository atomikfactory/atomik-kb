// The popover that edits the LaTeX of an inline equation (opened by a click on the equation, Enter on
// it selected, or `/inline equation`): a text field for the source and a live preview under it, typeset
// the way the equation is. Nothing reaches the page while typing: Enter applies the source (one
// attribute update, one Loro change) and closes, Escape closes and drops it, a press outside applies it
// (a stray click must not lose a formula) and closes. Backspace in an empty field removes the equation.

import { useId, useState } from "react";
import type { KeyboardEvent } from "react";

import { FloatingPanel } from "../components/FloatingPanel";
import type { Anchor, CloseReason } from "../components/FloatingPanel";
import { t } from "../i18n";
import { useTypeset } from "./math-typeset";

export interface MathInlineEditorProps {
  anchor: Anchor;
  /** The element that opened the popover: pressing it again is not "outside", so it can close the popover. */
  anchorElement: Element;
  /** Where the focus goes when the popover closes: the page's editable text. */
  returnFocus: Element | null;
  /** The source the equation holds now. */
  latex: string;
  /** Called with the new source when it is applied (never with the unchanged one). */
  onApply: (latex: string) => void;
  /** Removes the equation from the page. */
  onRemove: () => void;
  onClose: () => void;
}

export function MathInlineEditor({ anchor, anchorElement, returnFocus, latex, onApply, onRemove, onClose }: MathInlineEditorProps) {
  const [draft, setDraft] = useState(latex);
  const typeset = useTypeset(draft);
  const previewId = useId();

  /** Applies what was typed (when it differs from the stored source) and closes. */
  function commit(): void {
    if (draft !== latex) {
      onApply(draft);
    }
    onClose();
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
    } else if (event.key === "Backspace" && draft === "") {
      event.preventDefault();
      onRemove();
    }
  }

  function onPanelClose(reason?: CloseReason): void {
    if (reason === "outside") {
      commit();
    } else {
      onClose(); // Escape: what was typed is dropped
    }
  }

  return (
    <FloatingPanel
      label={t("mathInline.label")}
      className="math-inline-editor"
      anchor={anchor}
      anchorElement={anchorElement}
      returnFocus={returnFocus}
      onClose={onPanelClose}
      initialFocus={(panel) => panel.querySelector("input")}
    >
      <input
        type="text"
        className="input math-inline-field"
        aria-label={t("mathInline.field")}
        aria-describedby={previewId}
        placeholder={t("mathInline.fieldPlaceholder")}
        value={draft}
        spellCheck={false}
        autoCapitalize="off"
        autoComplete="off"
        autoCorrect="off"
        onChange={(event) => {
          setDraft(event.target.value);
        }}
        onKeyDown={onKeyDown}
      />
      <div id={previewId} className="math-inline-preview" data-state={typeset.state} aria-live="polite">
        {typeset.state === "ready" ? (
          // KaTeX's own output (trust: false), never the document's text.
          <span className="math-inline-render" dangerouslySetInnerHTML={{ __html: typeset.markup }} />
        ) : typeset.state === "error" ? (
          <span className="math-inline-error" title={typeset.message}>
            {t("mathInline.error")}
          </span>
        ) : typeset.state === "loading" ? (
          <span className="math-inline-source">{draft}</span>
        ) : (
          <span className="math-inline-hint">{t("mathInline.previewEmpty")}</span>
        )}
      </div>
    </FloatingPanel>
  );
}
