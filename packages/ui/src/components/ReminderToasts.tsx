import type { ReminderDue } from "@atomik-kb/core-client";

import { t } from "../i18n";
import { useAppStore } from "../store-context";

/** "Reminder: <page title> — <text>" (the title of an unnamed page is "Untitled"; no text, no dash). */
export function reminderText({ pageTitle, text }: ReminderDue): string {
  const title = pageTitle.trim() === "" ? t("page.untitled") : pageTitle.trim();
  const body = text.trim();
  return body === "" ? t("reminders.toastNoText", { title }) : t("reminders.toast", { title, text: body });
}

/**
 * The in-app toasts of reminders (ADR-0024): one per `ReminderDue` event, newest last, each with
 * an "Open" button that goes to the page and scrolls to the block and a button that dismisses it;
 * they go by themselves after 15 seconds (`store-reminders.ts`). The container is always there, a
 * `log` (a polite live region for things that arrive one after another, and no `status`, which the
 * page's own notices are), so that a screen reader announces what appears in it. It is the only place a reminder shows in the
 * browser page, which has no OS notifications; on the desktop the shell shows one as well.
 */
export function ReminderToasts() {
  const toasts = useAppStore((state) => state.reminderToasts);
  const dismiss = useAppStore((state) => state.dismissReminderToast);
  const open = useAppStore((state) => state.openReminderToast);

  return (
    <div className="reminder-toasts" role="log" aria-label={t("reminders.label")}>
      {toasts.map(({ id, reminder }) => (
        <div key={id} className="reminder-toast">
          <p className="reminder-toast-text">{reminderText(reminder)}</p>
          <div className="reminder-toast-actions">
            <button
              type="button"
              className="button"
              onClick={() => {
                open(id);
              }}
            >
              {t("reminders.open")}
            </button>
            <button
              type="button"
              className="link-button"
              onClick={() => {
                dismiss(id);
              }}
            >
              {t("reminders.dismiss")}
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
