import { useRef, useState } from "react";

export interface InlineEditProps {
  /** The text before the edit; leaving it unchanged is not a commit. */
  value: string;
  /** Accessible name of the input. */
  label: string;
  placeholder?: string;
  className?: string;
  /**
   * Saves the new text, on Enter or when the field loses focus. The edit ends when it
   * settles, also when it fails: the caller reports failures.
   */
  onCommit: (value: string) => Promise<void>;
  /** The edit is over: Escape, no change, or a finished commit. */
  onDone: () => void;
  /**
   * Enter was pressed (the commit is under way): the caller says where the focus goes, since the field is about to
   * disappear and would leave it on the page's body. Not called for the Enter of an input method's composition.
   */
  onEnter?: () => void;
}

/**
 * A text field that edits a name in place: Enter commits, Escape cancels, leaving the
 * field commits. It takes focus with the text selected when it appears.
 */
export function InlineEdit({ value, label, placeholder, className, onCommit, onDone, onEnter }: InlineEditProps) {
  const [draft, setDraft] = useState(value);
  const [busy, setBusy] = useState(false);
  // Enter, Escape and blur can follow each other (the field blurs as it disappears); only the first counts.
  const finished = useRef(false);

  async function commit(): Promise<void> {
    if (finished.current) {
      return;
    }
    finished.current = true;
    if (draft.trim() === value) {
      onDone();
      return;
    }
    setBusy(true);
    try {
      await onCommit(draft);
    } catch {
      // The caller reports failures; the edit ends either way.
    }
    onDone();
  }

  function cancel(): void {
    if (finished.current) {
      return;
    }
    finished.current = true;
    onDone();
  }

  return (
    <input
      className={className === undefined ? "inline-edit" : `inline-edit ${className}`}
      type="text"
      value={draft}
      readOnly={busy}
      aria-label={label}
      placeholder={placeholder}
      autoComplete="off"
      autoFocus
      onFocus={(event) => {
        event.currentTarget.select();
      }}
      onChange={(event) => {
        setDraft(event.target.value);
      }}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) {
          return; // Enter or Escape confirms or cancels the input method's composition, not the edit
        }
        if (event.key === "Enter") {
          event.preventDefault();
          void commit();
          // The commit has taken the field's blur (`finished`), so moving the focus here is not a second commit.
          onEnter?.();
        } else if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          cancel();
        }
      }}
      onBlur={() => {
        void commit();
      }}
      onClick={(event) => {
        event.stopPropagation();
      }}
      onDoubleClick={(event) => {
        event.stopPropagation();
      }}
    />
  );
}
