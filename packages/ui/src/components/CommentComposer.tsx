// The field a comment is written in (ADR-0048): a text area with the `@` people picker. Typing `@` and a few letters of a name lists
// the members of the space that match (`PeopleMentionable`, ADR-0049); Enter or a click puts `@Name` in the text and remembers whose
// account it is, and the body that goes out has a mention item in place of it (`bodyFromText`). A name edited after it was picked is
// plain text. Enter sends (Shift+Enter breaks the line), Escape closes the picker, then cancels.

import type { AccountId, CommentBodyItem, SpaceId } from "@atomik-kb/core-client";
import { useId, useLayoutEffect, useRef, useState } from "react";
import type { ChangeEvent, KeyboardEvent } from "react";

import { bodyFromText, initialOf, mentionQuery, refusalText } from "../comments";
import type { CommentAction } from "../comments";
import { usePeople } from "../comments-hooks";
import { personChoices } from "../editor/mention-menu";
import { t } from "../i18n";

export interface CommentComposerProps {
  spaceId: SpaceId;
  /** The accessible name of the field. */
  label: string;
  placeholder?: string;
  /** The text to start with (an edit), and the mentions in it. */
  initialText?: string;
  initialMentions?: ReadonlyMap<string, AccountId>;
  submitLabel: string;
  cancelLabel?: string;
  /** What sending does, for the words of a refusal: a new comment or a reply (`post`, the default) or an `edit`. */
  action?: CommentAction;
  autoFocus?: boolean;
  /** Sends the body; rejects with a `CoreError` (shown under the field). The field is emptied when it resolves. */
  onSubmit: (body: CommentBodyItem[]) => Promise<void>;
  /** Closes the composer without sending. */
  onCancel?: () => void;
}

interface Picker {
  start: number;
  query: string;
  active: number;
}

export function CommentComposer({
  spaceId,
  label,
  placeholder,
  initialText = "",
  initialMentions,
  submitLabel,
  cancelLabel,
  action = "post",
  autoFocus = false,
  onSubmit,
  onCancel,
}: CommentComposerProps) {
  const people = usePeople(spaceId);
  const [text, setText] = useState(initialText);
  const [mentions, setMentions] = useState<ReadonlyMap<string, AccountId>>(initialMentions ?? new Map());
  const [picker, setPicker] = useState<Picker | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  const caretAfter = useRef<number | null>(null);
  // A name was just picked: the caret moves behind it, which is no reason to open the picker again.
  const justPicked = useRef(false);
  const listId = useId();

  // After a pick the caret goes behind the name, once the text it is in has been drawn.
  useLayoutEffect(() => {
    const caret = caretAfter.current;
    if (caret !== null && field.current !== null) {
      field.current.setSelectionRange(caret, caret);
      caretAfter.current = null;
    }
  }, [text]);

  const choices = picker === null ? [] : personChoices(people ?? [], picker.query).flatMap((choice) => (choice.kind === "person" ? [choice.person] : []));
  const open = picker !== null && choices.length > 0;
  const active = picker === null ? 0 : Math.min(picker.active, Math.max(0, choices.length - 1));

  function track(value: string, caret: number): void {
    const found = mentionQuery(value, caret);
    setPicker(found === null ? null : { start: found.start, query: found.query, active: 0 });
  }

  function onChange(event: ChangeEvent<HTMLTextAreaElement>): void {
    justPicked.current = false;
    setText(event.target.value);
    setError(null);
    track(event.target.value, event.target.selectionStart);
  }

  function pick(index: number): void {
    const person = choices[index];
    const element = field.current;
    if (person === undefined || picker === null || element === null) {
      return;
    }
    const caret = element.selectionStart;
    const inserted = `@${person.name} `;
    const next = `${text.slice(0, picker.start)}${inserted}${text.slice(caret)}`;
    caretAfter.current = picker.start + inserted.length;
    justPicked.current = true;
    setText(next);
    setMentions((current) => new Map(current).set(person.name, person.accountId));
    setPicker(null);
  }

  async function send(): Promise<void> {
    const body = bodyFromText(text, mentions);
    if (body.length === 0 || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSubmit(body);
      setText("");
      setMentions(new Map());
      setPicker(null);
    } catch (failure) {
      setError(t("comments.error.save", { message: refusalText(failure, action) }));
    } finally {
      setBusy(false);
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (open && picker !== null) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const step = event.key === "ArrowDown" ? 1 : -1;
        setPicker({ ...picker, active: (active + step + choices.length) % choices.length });
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        pick(active);
        return;
      }
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (picker !== null) {
        setPicker(null);
      } else {
        onCancel?.();
      }
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void send();
    }
  }

  return (
    <form
      className="comment-composer"
      onSubmit={(event) => {
        event.preventDefault();
        void send();
      }}
    >
      <div className="comment-composer-field">
        <textarea
          ref={field}
          className="input comment-input"
          rows={2}
          aria-label={label}
          placeholder={placeholder}
          value={text}
          autoFocus={autoFocus}
          aria-controls={open ? listId : undefined}
          aria-autocomplete="list"
          aria-activedescendant={open ? `${listId}-${String(active)}` : undefined}
          readOnly={busy}
          onChange={onChange}
          onKeyDown={onKeyDown}
          onSelect={(event) => {
            if (!justPicked.current) {
              track(event.currentTarget.value, event.currentTarget.selectionStart);
            }
          }}
          onBlur={() => {
            setPicker(null);
          }}
        />
        {open && (
          <ul id={listId} role="listbox" aria-label={t("comments.mention.label")} className="comment-mention-list">
            {choices.map((person, index) => (
              <li
                key={person.accountId}
                id={`${listId}-${String(index)}`}
                role="option"
                aria-selected={index === active}
                className={index === active ? "menu-item is-active" : "menu-item"}
                onMouseDown={(event) => {
                  event.preventDefault();
                  pick(index);
                }}
              >
                <span className="person-mark" aria-hidden="true">
                  {initialOf(person.name)}
                </span>
                <span>{person.name}</span>
                {person.isMe && <span className="muted">{t("mention.person.you")}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      <div className="comment-composer-actions">
        <button type="submit" className="button" disabled={busy || text.trim() === ""}>
          {submitLabel}
        </button>
        {onCancel !== undefined && (
          <button type="button" className="link-button" onClick={onCancel}>
            {cancelLabel ?? t("comments.cancel")}
          </button>
        )}
      </div>
    </form>
  );
}
