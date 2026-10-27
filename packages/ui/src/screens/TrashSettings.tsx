// Settings -> Data, trash (ADR-0040): how many days a page stays in the trash before it is deleted
// for good (`app.trash.retention_days`, a device setting; 30 when unset, 0 keeps trashed pages until
// someone deletes them). The number is saved when the field loses focus or Enter is pressed, not on
// every keystroke: a deletion limit should not pass through "1" on the way to "14".

import { TRASH_RETENTION_SETTING, toCoreError, trashRetentionDays } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";

import { t, tp } from "../i18n";
import { useAppStore } from "../store-context";

/** Parses what was typed for the limit: a whole number of days, 0 or more, else `null`. */
export function parseTrashDays(text: string): number | null {
  if (!/^\d+$/.test(text.trim())) {
    return null;
  }
  const days = Number(text.trim());
  return Number.isSafeInteger(days) ? days : null;
}

function hintFor(days: number): string {
  if (days === 0) {
    return t("settings.trash.hint.never");
  }
  return tp("settings.trash.hint", days);
}

/** The trash part of Settings -> Data. */
export function TrashSettings() {
  const stored = useAppStore((state) => state.settings[TRASH_RETENTION_SETTING]);
  const loadSetting = useAppStore((state) => state.loadSetting);
  const setSetting = useAppStore((state) => state.setSetting);
  // The text of the field while it is being typed; `null` shows the stored number.
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const fieldId = useId();
  const hintId = useId();
  const unread = stored === undefined;
  const days = trashRetentionDays(stored);

  useEffect(() => {
    if (unread) {
      loadSetting(TRASH_RETENTION_SETTING).catch((failure: unknown) => {
        console.error("could not read the trash limit", toCoreError(failure).code);
      });
    }
  }, [unread, loadSetting]);

  function commit(): void {
    if (draft === null) {
      return;
    }
    const next = parseTrashDays(draft);
    setDraft(null);
    if (next === null) {
      setError(t("settings.trash.invalid"));
      return;
    }
    setError(null);
    if (next !== days) {
      setSetting(TRASH_RETENTION_SETTING, next).catch((failure: unknown) => {
        setError(t("settings.trash.error", { message: toCoreError(failure).message }));
      });
    }
  }

  return (
    <section className="subpanel trash-settings" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("settings.trash.title")}
      </h3>
      <div className="field trash-retention">
        <label htmlFor={fieldId}>{t("settings.trash.retention")}</label>
        <input
          id={fieldId}
          className="input"
          type="number"
          inputMode="numeric"
          min={0}
          step={1}
          value={draft ?? String(days)}
          disabled={unread}
          aria-describedby={hintId}
          onChange={(event) => {
            setDraft(event.target.value);
          }}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commit();
            } else if (event.key === "Escape") {
              setDraft(null);
              setError(null);
            }
          }}
        />
        <p className="muted" id={hintId}>
          {hintFor(days)} {t("settings.trash.hint.sync")}
        </p>
        {error !== null && (
          <p className="error-text" role="alert">
            {error}
          </p>
        )}
      </div>
    </section>
  );
}
