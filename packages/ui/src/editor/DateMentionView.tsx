// The chip of a date mention: a calendar glyph and the day (and the time), in the words of the
// locale ("Today", "Tomorrow", "Yesterday" for those days, else the medium date), and a small bell
// when the mention has a reminder (ADR-0024). A click, or Enter on the selected chip, opens the
// date picker to change it; a read-only editor shows the chip and nothing more. The chip is redrawn
// when midnight passes (`today.ts`).

import { reminderMinutesOf } from "@atomik-kb/core-client";
import { NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useState, useSyncExternalStore } from "react";

import { usePeople } from "../comments-hooks";
import { anchorBelow } from "../components/FloatingPanel";
import { BellIcon, CalendarIcon } from "../components/icons";
import { PLACEHOLDER_LABELS } from "../components/TemplatePlaceholders";
import { FilledInIcon } from "../components/template-icons";
import { t } from "../i18n";
import { useSpaceAccess } from "../sharing-hooks";
import type { DateMentionOptions, DateMentionStorage } from "./date-mention";
import { formatDateMention, isIsoDate, isIsoTime, reminderLabel } from "./dates";
import { DatePicker } from "./DatePicker";
import { useInTemplatePage } from "./template-context";
import { useToday } from "./today";

export function DateMentionView({ node, editor, extension, updateAttributes }: ReactNodeViewProps) {
  const id = typeof node.attrs.id === "string" ? node.attrs.id : null;
  const date = typeof node.attrs.date === "string" ? node.attrs.date : "";
  const time = typeof node.attrs.time === "string" && isIsoTime(node.attrs.time) ? node.attrs.time : null;
  const reminder = reminderMinutesOf(node.attrs.reminder);
  const reminderFor = typeof node.attrs.reminderFor === "string" ? node.attrs.reminderFor : null;
  // A relative mention (ADR-0056) reads "Today" or "Now" with a mark: it is filled in when a page is made from its template.
  const relative: "today" | "now" | null =
    node.attrs.relative === "today" ? "today" : node.attrs.relative === "now" ? "now" : null;
  const inTemplate = useInTemplatePage();
  // In a shared space the reminder says who is reminded (the members of the space); a personal space has no such choice.
  const { spaceId } = extension.options as DateMentionOptions;
  const shared = useSpaceAccess(spaceId).shared;
  const people = usePeople(shared ? spaceId : null);
  const today = useToday();
  // The binding can turn the editor read-only after the view was drawn (content from a newer version).
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const requests = (extension.storage as DateMentionStorage).picker;
  const openFor = useSyncExternalStore(requests.subscribe, requests.getSnapshot);
  const [element, setElement] = useState<HTMLElement | null>(null);
  const open = editable && id !== null && openFor === id && element !== null;

  const label =
    relative === null ? formatDateMention(date, time, today) : relative === "today" ? t("date.today") : t("template.relative.now");
  // What the chip is called by: the relative ones say when they are filled in.
  const spoken = relative === null ? label : t(PLACEHOLDER_LABELS[relative]);
  const valid = isIsoDate(date);
  const reminderText = reminder === null ? null : reminderLabel(reminder, time);

  return (
    <NodeViewWrapper as="span" ref={setElement} className="date-mention-wrapper">
      <span
        className={`date-mention${valid ? "" : " is-invalid"}${relative === null ? "" : " is-relative"}`}
        role={editable ? "button" : undefined}
        aria-haspopup={editable ? "dialog" : undefined}
        aria-expanded={editable ? open : undefined}
        aria-label={
          editable
            ? reminderText === null
              ? t("date.change", { date: spoken })
              : t("date.changeWithReminder", { date: spoken, reminder: reminderText })
            : undefined
        }
        onClick={() => {
          if (editable && id !== null) {
            if (openFor === id) {
              requests.close(id);
            } else {
              requests.open(id);
            }
          }
        }}
      >
        <span className="date-mention-icon" aria-hidden="true">
          <CalendarIcon />
        </span>
        <time className="date-mention-text" dateTime={valid ? (time === null ? date : `${date}T${time}`) : undefined}>
          {label}
        </time>
        {relative !== null && (
          <span className="date-mention-relative" aria-hidden="true" title={t("template.relative.mark")}>
            <FilledInIcon />
          </span>
        )}
        {reminderText !== null && (
          <span
            className="date-mention-bell"
            role={editable ? undefined : "img"}
            aria-hidden={editable ? true : undefined}
            aria-label={editable ? undefined : t("date.reminderSet", { reminder: reminderText })}
            title={t("date.reminderSet", { reminder: reminderText })}
          >
            <BellIcon />
          </span>
        )}
      </span>
      {open && (
        <DatePicker
          anchor={anchorBelow(element)}
          anchorElement={element}
          date={date}
          time={time}
          onChange={(nextDate, nextTime) => {
            // A day picked by hand is a fixed day: the mention stops being relative.
            updateAttributes(relative === null ? { date: nextDate, time: nextTime } : { date: nextDate, time: nextTime, relative: null });
          }}
          relative={
            inTemplate || relative !== null
              ? {
                  value: relative,
                  onChange: (next) => {
                    updateAttributes({ relative: next });
                  },
                }
              : undefined
          }
          reminder={reminder}
          onReminderChange={(next) => {
            // Only the reminder: one attribute, one change of the document (with it goes who it was for when there is none now).
            updateAttributes(next === null && reminderFor !== null ? { reminder: null, reminderFor: null } : { reminder: next });
          }}
          remindFor={shared && spaceId !== null && people !== undefined ? { spaceId, people, value: reminderFor } : undefined}
          onRemindForChange={(accountId) => {
            updateAttributes({ reminderFor: accountId });
          }}
          onClose={() => {
            requests.close();
          }}
        />
      )}
    </NodeViewWrapper>
  );
}
