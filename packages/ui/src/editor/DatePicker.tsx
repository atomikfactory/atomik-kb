// The popover that sets the day (and the time) of a date mention: a month grid, previous and next
// month, a button for today, a text field for typing the date as `YYYY-MM-DD`, an optional time
// of day and an optional reminder (a select of minutes before the moment, ADR-0024; it changes the
// `reminder` attribute alone). No dependency: the grid is plain buttons in a `role="grid"`, and the days come from
// `dates.ts`. Every change is applied at once (the mention shows it behind the popover), and the
// popover closes on Escape, on a press outside, on a click on a day or on Today, and on Enter in
// a field. In the grid the arrow keys move by a day or a week, PageUp and PageDown by a month
// (with Shift by a year), Home and End to the start and the end of the week, and Enter or Space
// pick the day.

import type { AccountId, PersonInfo, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { usePersonNames } from "../comments-hooks";
import { FloatingPanel } from "../components/FloatingPanel";
import type { Anchor, CloseReason } from "../components/FloatingPanel";
import { ChevronIcon } from "../components/icons";
import { PLACEHOLDER_LABELS } from "../components/TemplatePlaceholders";
import { t } from "../i18n";
import {
  REMINDER_CHOICES,
  addDays,
  addMonths,
  firstDayOfWeek,
  fullDate,
  isIsoDate,
  isIsoTime,
  monthGrid,
  monthTitle,
  reminderFireTime,
  reminderLabel,
  weekdayNames,
  weekdayOf,
} from "./dates";
import { useToday } from "./today";

export interface DatePickerProps {
  anchor: Anchor;
  /** The element that opened the picker: pressing it again is not "outside", so it can close the picker. */
  anchorElement: Element;
  /** The day the mention holds (`YYYY-MM-DD`; today is shown when it is not a real day). */
  date: string;
  /** The time of day the mention holds (`HH:mm`), or `null`. */
  time: string | null;
  /** Called with every new day and time (`null`: no time). */
  onChange: (date: string, time: string | null) => void;
  /** The reminder the mention holds: minutes before its moment, or `null` for none. */
  reminder?: number | null;
  /**
   * Called with the reminder the person picks (`null`: none); it changes that attribute alone.
   * Without it the picker has no "Reminder" field.
   */
  onReminderChange?: (reminder: number | null) => void;
  /**
   * In a shared space, who the reminder is for (ADR-0047): the members to choose from, and the account the mention names (`null`: nobody,
   * so the Owners' devices fire). Absent in a personal space, which has no "Remind" choice. The choice shows once there is a reminder.
   */
  remindFor?: { spaceId: SpaceId; people: readonly PersonInfo[]; value: AccountId | null };
  /** Called with the account the person picks (`null`: the space's Owners, the default); it changes `reminderFor` alone. */
  onRemindForChange?: (accountId: AccountId | null) => void;
  /**
   * On a page in a template (ADR-0056): "Today (when created)" and "Now (when created)", which make the mention relative (it
   * is filled in when a page is made from the template). `value` is what the mention is now (`null`: a fixed day); `onChange`
   * gets the choice, `null` to make it a fixed day again. Absent: no such field.
   */
  relative?: { value: "today" | "now" | null; onChange: (value: "today" | "now" | null) => void };
  /** Closes the picker: a pick, Enter and Today give no reason, Escape and a press outside say so (the panel's `CloseReason`). */
  onClose: (reason?: CloseReason) => void;
}

/** The reminders the "Reminder" field lists: the presets, and the mention's own when it is none of them. */
function reminderOptions(reminder: number | null): readonly (number | null)[] {
  return reminder === null || REMINDER_CHOICES.includes(reminder) ? REMINDER_CHOICES : [...REMINDER_CHOICES, reminder];
}

/** Whether the moment a reminder fires at is behind the clock: it will not fire (ADR-0024). */
function reminderHasPassed(date: string, time: string | null, reminder: number | null): boolean {
  if (reminder === null) {
    return false;
  }
  const fire = reminderFireTime(date, time, reminder);
  return fire !== null && fire.getTime() <= Date.now();
}

/** "Remind": the Owners of the space (nobody named: the default), this account, or another member; the account named stays listed when it left. */
function RemindWho({
  remindFor,
  onChange,
}: {
  remindFor: NonNullable<DatePickerProps["remindFor"]>;
  onChange: (accountId: AccountId | null) => void;
}) {
  const { spaceId, people, value } = remindFor;
  const nameOf = usePersonNames(spaceId, value === null ? [] : [value]);
  const me = people.find((person) => person.isMe);
  const others = people.filter((person) => !person.isMe);
  const listed = value === null || people.some((person) => person.accountId === value);
  return (
    <label className="date-picker-field">
      <span>{t("date.remindWho")}</span>
      <select
        className="input select"
        value={value ?? "owners"}
        onChange={(event) => {
          const next = event.target.value === "owners" ? null : event.target.value;
          if (next !== value) {
            onChange(next);
          }
        }}
      >
        <option value="owners">{t("date.remindWho.owners")}</option>
        {me !== undefined && <option value={me.accountId}>{t("date.remindWho.me")}</option>}
        {others.map((person) => (
          <option key={person.accountId} value={person.accountId}>
            {person.name}
          </option>
        ))}
        {value !== null && !listed && <option value={value}>{nameOf(value)}</option>}
      </select>
    </label>
  );
}

export function DatePicker({
  anchor,
  anchorElement,
  date,
  time,
  onChange,
  reminder = null,
  onReminderChange,
  remindFor,
  onRemindForChange,
  relative,
  onClose,
}: DatePickerProps) {
  const today = useToday();
  const current = isIsoDate(date) ? date : today;
  const [firstDay] = useState(() => firstDayOfWeek());
  // The day the keyboard is on (the grid shows its month), what the date field says, and the time
  // field's text (which is empty, or a valid time, or being typed).
  const [cursor, setCursor] = useState(current);
  const [typed, setTyped] = useState(date);
  const [timeText, setTimeText] = useState(time ?? "");
  // What the mention holds changed (a pick, or an edit from elsewhere): the fields follow it.
  const [seen, setSeen] = useState({ date, time });
  if (seen.date !== date || seen.time !== time) {
    setSeen({ date, time });
    if (seen.date !== date) {
      setTyped(date);
      setCursor(current);
    }
    if (seen.time !== time) {
      setTimeText(time ?? "");
    }
  }

  const titleId = useId();
  const grid = useRef<HTMLTableElement>(null);
  const timeField = useRef<HTMLInputElement>(null);
  // Set by a key that moved the cursor: the day it moved to takes the focus once it is drawn.
  const moveFocus = useRef(false);
  useEffect(() => {
    if (!moveFocus.current) {
      return;
    }
    moveFocus.current = false;
    grid.current?.querySelector<HTMLElement>(`[data-date="${cursor}"]`)?.focus();
  }, [cursor]);

  /** Applies a day and closes: a click on a day, Today, Enter. */
  function pick(day: string): void {
    if (day !== date) {
      onChange(day, time);
    }
    onClose();
  }

  function onGridKeyDown(event: KeyboardEvent<HTMLElement>): void {
    const weekday = (weekdayOf(cursor) - firstDay + 7) % 7;
    let next: string;
    switch (event.key) {
      case "ArrowLeft":
        next = addDays(cursor, -1);
        break;
      case "ArrowRight":
        next = addDays(cursor, 1);
        break;
      case "ArrowUp":
        next = addDays(cursor, -7);
        break;
      case "ArrowDown":
        next = addDays(cursor, 7);
        break;
      case "PageUp":
        next = addMonths(cursor, event.shiftKey ? -12 : -1);
        break;
      case "PageDown":
        next = addMonths(cursor, event.shiftKey ? 12 : 1);
        break;
      case "Home":
        next = addDays(cursor, -weekday);
        break;
      case "End":
        next = addDays(cursor, 6 - weekday);
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        pick(cursor);
        return;
      default:
        return;
    }
    event.preventDefault();
    if (next !== cursor && isIsoDate(next)) {
      moveFocus.current = true;
      setCursor(next);
    }
  }

  const names = weekdayNames(firstDay);
  const month = cursor.slice(0, 7);

  return (
    <FloatingPanel
      label={t("date.pickerTitle")}
      className="date-picker"
      anchor={anchor}
      anchorElement={anchorElement}
      onClose={onClose}
      initialFocus={(panel) => panel.querySelector<HTMLElement>('.date-picker-day[tabindex="0"]')}
    >
      <div className="date-picker-header">
        <button
          type="button"
          className="date-picker-nav is-previous"
          aria-label={t("date.prevMonth")}
          title={t("date.prevMonth")}
          onClick={() => {
            setCursor(addMonths(cursor, -1));
          }}
        >
          <ChevronIcon />
        </button>
        <div id={titleId} className="date-picker-title" aria-live="polite">
          {monthTitle(cursor)}
        </div>
        <button
          type="button"
          className="date-picker-nav"
          aria-label={t("date.nextMonth")}
          title={t("date.nextMonth")}
          onClick={() => {
            setCursor(addMonths(cursor, 1));
          }}
        >
          <ChevronIcon />
        </button>
      </div>
      <table ref={grid} role="grid" aria-labelledby={titleId} className="date-picker-grid" onKeyDown={onGridKeyDown}>
        <thead>
          <tr role="row">
            {names.map((name) => (
              <th key={name.long} role="columnheader" scope="col" abbr={name.long} aria-label={name.long}>
                {name.short}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {monthGrid(cursor, firstDay).map((week) => (
            <tr key={week[0]} role="row">
              {week.map((day) => (
                <td key={day} role="gridcell" aria-selected={day === date}>
                  <button
                    type="button"
                    tabIndex={day === cursor ? 0 : -1}
                    data-date={day}
                    aria-label={fullDate(day)}
                    disabled={!isIsoDate(day)}
                    aria-current={day === today ? "date" : undefined}
                    className={`date-picker-day${day.slice(0, 7) === month ? "" : " is-outside"}${day === date ? " is-selected" : ""}${day === today ? " is-today" : ""}`}
                    onClick={() => {
                      pick(day);
                    }}
                  >
                    {Number(day.slice(-2))}
                  </button>
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      <div className="date-picker-fields">
        <label className="date-picker-field">
          <span>{t("date.field")}</span>
          <input
            type="text"
            className="input"
            inputMode="numeric"
            autoComplete="off"
            spellCheck={false}
            maxLength={10}
            placeholder={t("date.fieldPlaceholder")}
            value={typed}
            aria-invalid={!isIsoDate(typed)}
            onChange={(event) => {
              const next = event.target.value;
              setTyped(next);
              // A real day applies as soon as it is complete; anything else waits.
              if (isIsoDate(next)) {
                setCursor(next);
                if (next !== date) {
                  onChange(next, time);
                }
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && isIsoDate(typed)) {
                event.preventDefault();
                onClose();
              }
            }}
          />
        </label>
        <button
          type="button"
          className="button date-picker-today"
          onClick={() => {
            pick(today);
          }}
        >
          {t("date.today")}
        </button>
      </div>
      <div className="date-picker-fields">
        <label className="date-picker-field">
          <span>{t("date.time")}</span>
          <input
            ref={timeField}
            type="time"
            className="input"
            value={timeText}
            onChange={(event) => {
              const next = event.target.value;
              setTimeText(next);
              if (next === "") {
                if (time !== null) {
                  onChange(current, null);
                }
              } else if (isIsoTime(next) && next !== time) {
                onChange(current, next);
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                onClose();
              }
            }}
          />
        </label>
        <button
          type="button"
          className="button date-picker-clear"
          disabled={time === null && timeText === ""}
          onClick={() => {
            setTimeText("");
            if (time !== null) {
              onChange(current, null);
            }
            timeField.current?.focus();
          }}
        >
          {t("date.clearTime")}
        </button>
      </div>
      {relative !== undefined && (
        <div className="date-picker-fields date-picker-relative" role="group" aria-label={t("template.picker.relativeLabel")}>
          {(["today", "now"] as const).map((choice) => (
            <button
              key={choice}
              type="button"
              className="button"
              aria-pressed={relative.value === choice}
              onClick={() => {
                relative.onChange(relative.value === choice ? null : choice);
                onClose();
              }}
            >
              {t(PLACEHOLDER_LABELS[choice])}
            </button>
          ))}
        </div>
      )}
      {onReminderChange !== undefined && (
        <div className="date-picker-fields">
          <label className="date-picker-field">
            <span>{t("date.reminder")}</span>
            <select
              className="input select"
              value={reminder === null ? "none" : String(reminder)}
              onChange={(event) => {
                const next = event.target.value === "none" ? null : Number(event.target.value);
                if (next !== reminder) {
                  onReminderChange(next);
                }
              }}
            >
              {reminderOptions(reminder).map((minutes) => (
                <option key={minutes ?? "none"} value={minutes === null ? "none" : String(minutes)}>
                  {reminderLabel(minutes, time)}
                </option>
              ))}
            </select>
          </label>
        </div>
      )}
      {onReminderChange !== undefined && reminder !== null && remindFor !== undefined && onRemindForChange !== undefined && (
        <div className="date-picker-fields">
          <RemindWho remindFor={remindFor} onChange={onRemindForChange} />
        </div>
      )}
      {onReminderChange !== undefined && reminderHasPassed(date, time, reminder) && (
        <p className="date-picker-note" role="status">
          {t("date.reminderPast")}
        </p>
      )}
    </FloatingPanel>
  );
}
