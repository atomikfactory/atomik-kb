// The editor of a `checklist` property (ADR-0026, amendment of 2026-10-07): a list with a checkbox
// and an editable text for every item, a line "N of M done" with a thin bar under it, and "Add
// item". Enter in a text adds the next item below it, Backspace in an empty one removes it (and
// moves to the one above), Alt+Up and Alt+Down move the item, Escape leaves the field.
//
// Every change is one `pagePropertySet` with the whole list: a checkbox, an addition, a removal and
// a move at once, the text of an item 300 ms after the last key (and as soon as the field is left).
// Writes go one after the other, so the last one is what the core keeps. While edits are on their
// way the list is the one being edited: what the page holds is shown again once nothing is pending.
// Items without text that were added here and left empty are dropped when focus leaves the list.

import { MAX_CHECKLIST_ITEMS, MAX_CHECKLIST_TEXT_CHARS, checklistProgress } from "@atomik-kb/core-client";
import type { ChecklistItem, PropertyValue } from "@atomik-kb/core-client";
import { useEffect, useRef, useState } from "react";
import type { FocusEvent, KeyboardEvent } from "react";

import { t } from "../i18n";
import { PlusIcon } from "./icons";
import { ErrorLine, useCommit } from "./property-editor-parts";
import type { PropertyEditorProps } from "./property-editor-parts";
import { CloseIcon } from "./property-icons";

/** How long the text of an item may rest before it is saved. */
export const CHECKLIST_DEBOUNCE_MS = 300;

const NO_ITEMS: readonly ChecklistItem[] = [];

function copyItems(items: readonly ChecklistItem[]): ChecklistItem[] {
  return items.map((item) => ({ ...item }));
}

function newItem(): ChecklistItem {
  return { id: crypto.randomUUID(), text: "", done: false };
}

/** The value a list stands for; an empty list clears. */
function valueOf(items: readonly ChecklistItem[]): PropertyValue | null {
  return items.length === 0 ? null : { kind: "checklist", value: copyItems(items) };
}

function sameItems(a: readonly ChecklistItem[], b: readonly ChecklistItem[]): boolean {
  return (
    a.length === b.length &&
    a.every((item, at) => {
      const other = b[at];
      return other !== undefined && item.id === other.id && item.text === other.text && item.done === other.done;
    })
  );
}

/** Text as the core takes it: one line, no control characters. */
function cleaned(text: string): string {
  return text.replace(/\p{Cc}+/gu, " ");
}

export function ChecklistEditor({ def, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const stored: readonly ChecklistItem[] = value?.kind === "checklist" ? value.value : NO_ITEMS;
  const { run, error, clearError } = useCommit(commit);
  // A property that was just added starts with one empty item to type into.
  const [items, setItems] = useState<ChecklistItem[]>(() =>
    stored.length === 0 && autoFocus && !readOnly ? [newItem()] : copyItems(stored),
  );
  const [seen, setSeen] = useState(stored);
  const [pending, setPending] = useState(false);
  const [focus, setFocus] = useState<{ id: string | null; turn: number } | null>(() =>
    autoFocus && !readOnly ? { id: items[0]?.id ?? null, turn: 0 } : null,
  );
  const fields = useRef(new Map<string, HTMLInputElement>());
  const addButton = useRef<HTMLButtonElement>(null);
  const latest = useRef<ChecklistItem[]>(items);
  const sent = useRef<readonly ChecklistItem[]>(stored);
  const dirty = useRef(false);
  const inFlight = useRef(0);
  const timer = useRef<number | undefined>(undefined);
  const queue = useRef<Promise<void>>(Promise.resolve());
  const commitRef = useRef(commit);
  useEffect(() => {
    commitRef.current = commit;
  });

  // What the page holds changed (a save, or an edit from elsewhere): the list follows it unless
  // edits of this list are still on their way.
  if (seen !== stored) {
    const changed = !sameItems(seen, stored);
    setSeen(stored);
    if (changed && !pending) {
      setItems(copyItems(stored));
    }
  }
  // The refs the timers and the cleanup read follow the list that is shown and what the page holds.
  useEffect(() => {
    latest.current = items;
  });
  useEffect(() => {
    if (!pending) {
      sent.current = stored;
    }
  }, [pending, stored]);

  // Moves focus where an edit asked for it (a new item, the neighbour of a removed one).
  useEffect(() => {
    if (focus === null) {
      return;
    }
    const target = focus.id === null ? addButton.current : (fields.current.get(focus.id) ?? null);
    target?.focus();
  }, [focus]);

  // Edits that were not sent yet when the editor goes away (another page was opened) are sent.
  useEffect(
    () => () => {
      window.clearTimeout(timer.current);
      if (dirty.current) {
        dirty.current = false;
        void commitRef.current(valueOf(latest.current)).catch(() => undefined);
      }
    },
    [],
  );

  function settle(): void {
    setPending(dirty.current || inFlight.current > 0);
  }

  /** Sends the pending edits now (one write, after the ones before it). */
  function flush(): void {
    window.clearTimeout(timer.current);
    timer.current = undefined;
    if (!dirty.current) {
      settle();
      return;
    }
    dirty.current = false;
    const list = copyItems(latest.current);
    if (sameItems(list, sent.current)) {
      settle();
      return;
    }
    sent.current = list;
    inFlight.current += 1;
    settle();
    queue.current = queue.current.then(async () => {
      await run(valueOf(list));
      inFlight.current -= 1;
      settle();
    });
  }

  /** Takes `next` as the list, and saves it now or after the text rests. */
  function change(next: ChecklistItem[], now: boolean, focusId?: string | null): void {
    clearError();
    latest.current = next;
    setItems(next);
    dirty.current = true;
    if (focusId !== undefined) {
      setFocus((current) => ({ id: focusId, turn: (current?.turn ?? 0) + 1 }));
    }
    window.clearTimeout(timer.current);
    if (now) {
      flush();
    } else {
      timer.current = window.setTimeout(flush, CHECKLIST_DEBOUNCE_MS);
      settle();
    }
  }

  function insertAfter(index: number): void {
    const added = newItem();
    const next = [...items];
    next.splice(index + 1, 0, added);
    change(next, true, added.id);
  }

  function removeAt(index: number): void {
    const next = items.filter((_, at) => at !== index);
    const neighbour = next[Math.max(0, index - 1)];
    change(next, true, neighbour === undefined ? null : neighbour.id);
  }

  function moveBy(index: number, delta: -1 | 1): void {
    const to = index + delta;
    const moving = items[index];
    if (moving === undefined || to < 0 || to >= items.length) {
      return;
    }
    const next = [...items];
    next.splice(index, 1);
    next.splice(to, 0, moving);
    change(next, true, moving.id);
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>, index: number, item: ChecklistItem): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      if (items.length < MAX_CHECKLIST_ITEMS) {
        insertAfter(index);
      }
    } else if (event.key === "Backspace" && item.text === "") {
      event.preventDefault();
      removeAt(index);
    } else if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      event.preventDefault();
      moveBy(index, event.key === "ArrowUp" ? -1 : 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      event.currentTarget.blur();
    }
  }

  /** Focus left the list: the items nobody typed into are dropped, and what is pending is sent. */
  function onBlur(event: FocusEvent<HTMLDivElement>): void {
    if (event.currentTarget.contains(event.relatedTarget)) {
      return;
    }
    const kept = latest.current.filter((item) => item.text.trim() !== "");
    if (kept.length !== latest.current.length) {
      change(kept, true);
    } else {
      flush();
    }
  }

  const { done, total } = checklistProgress(items);
  const progress =
    total === 0 ? null : (
      <div className="checklist-progress">
        <span className="muted">{t("properties.checklist.progress", { done, total })}</span>
        <progress
          className="checklist-bar"
          max={total}
          value={done}
          aria-label={t("properties.checklist.progressLabel", { name: def.name })}
        />
      </div>
    );

  if (readOnly) {
    return (
      <div className="property-editor property-checklist">
        <ul className="checklist-items" aria-label={def.name}>
          {items.map((item) => (
            <li key={item.id} className="checklist-item" data-done={item.done ? "" : undefined}>
              <input type="checkbox" className="property-checkbox" checked={item.done} disabled aria-label={item.text} />
              <span className="checklist-text properties-text">{item.text}</span>
            </li>
          ))}
        </ul>
        {progress}
      </div>
    );
  }

  return (
    <div className="property-editor property-checklist" onBlur={onBlur}>
      <ul className="checklist-items" aria-label={def.name}>
        {items.map((item, index) => (
          <li key={item.id} className="checklist-item" data-done={item.done ? "" : undefined}>
            <input
              type="checkbox"
              className="property-checkbox"
              checked={item.done}
              aria-label={t("properties.checklist.done", { number: index + 1 })}
              onChange={(event) => {
                change(
                  items.map((entry) => (entry.id === item.id ? { ...entry, done: event.target.checked } : entry)),
                  true,
                );
              }}
            />
            <input
              ref={(element) => {
                if (element === null) {
                  fields.current.delete(item.id);
                } else {
                  fields.current.set(item.id, element);
                }
              }}
              type="text"
              className="property-input checklist-text"
              value={item.text}
              maxLength={MAX_CHECKLIST_TEXT_CHARS}
              autoComplete="off"
              spellCheck={false}
              placeholder={t("properties.checklist.placeholder")}
              aria-label={t("properties.checklist.item", { number: index + 1 })}
              onChange={(event) => {
                const text = cleaned(event.target.value);
                change(
                  items.map((entry) => (entry.id === item.id ? { ...entry, text } : entry)),
                  false,
                );
              }}
              onBlur={flush}
              onKeyDown={(event) => {
                onKeyDown(event, index, item);
              }}
            />
            <button
              type="button"
              className="icon-button checklist-remove"
              aria-label={t("properties.checklist.remove", { number: index + 1 })}
              title={t("properties.checklist.remove", { number: index + 1 })}
              onClick={() => {
                removeAt(index);
              }}
            >
              <CloseIcon />
            </button>
          </li>
        ))}
      </ul>
      {progress}
      <button
        ref={addButton}
        type="button"
        className={items.length === 0 ? "property-chip is-empty" : "property-chip property-chip-add"}
        disabled={items.length >= MAX_CHECKLIST_ITEMS}
        title={items.length >= MAX_CHECKLIST_ITEMS ? t("properties.checklist.limit", { max: MAX_CHECKLIST_ITEMS }) : undefined}
        onClick={() => {
          const added = newItem();
          change([...items, added], true, added.id);
        }}
      >
        <PlusIcon />
        <span>{t("properties.checklist.add")}</span>
      </button>
      <ErrorLine message={error} />
    </div>
  );
}
