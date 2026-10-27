// Settings -> Security -> Backup -> Scheduled backups (ADR-0036, `docs/architecture/import-export.md` 5.4):
// the desktop shell's timer writes the encrypted backup into a folder every day or week while the app runs
// and is unlocked, and keeps the newest N. This is the schedule (on or off, how often, how many to keep,
// the setting `app.backup.schedule`, written from here), the folder (chosen only through the shell's
// dialog, which stores it: `app.backup.folder`, read-only here), the outcome of the last run
// (`app.backup.last`, written by the shell; both are read again when a `SettingChanged` says they changed)
// and "Back up now". Where the shell has Google Drive (ADR-0059) the targets are two: the folder and Google Drive
// (`DriveBackupTarget`, the slice `store-drive.ts`); a run goes to each of them that is set. Without a target (no folder and
// no connected Drive) the controls are off; without a recovery kit they are off too, as the manual backup is: nothing
// could restore the file.

import { driveReasonOf, toCoreError } from "@atomik-kb/core-client";
import type { BackupInterval, BackupRunStatus, BackupSchedule } from "@atomik-kb/core-client";
import { useEffect, useId, useMemo, useState } from "react";

import { formatTimestamp } from "../format";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import {
  BACKUP_FOLDER_KEY,
  BACKUP_LAST_KEY,
  BACKUP_SCHEDULE_KEY,
  MAX_BACKUP_KEEP,
  MIN_BACKUP_KEEP,
  fileNameOf,
  parseBackupFolder,
  parseBackupLast,
  parseBackupSchedule,
} from "../store-interop";
import { DriveBackupTarget, driveReasonText } from "./DriveBackupTarget";

type Notice = { kind: "none" } | { kind: "info"; text: string } | { kind: "error"; text: string };

const NO_NOTICE: Notice = { kind: "none" };

/** Parses what was typed for "Backups to keep": a whole number from 1 to 100, else `null`. */
export function parseKeep(text: string): number | null {
  if (!/^\d+$/.test(text.trim())) {
    return null;
  }
  const keep = Number(text.trim());
  return keep >= MIN_BACKUP_KEEP && keep <= MAX_BACKUP_KEEP ? keep : null;
}

/** Why the schedule cannot be turned on: no recovery kit to restore with, or nowhere to write (no folder, no connected Drive). */
export type ScheduleBlocker = "kit" | "target";

/**
 * What stops the schedule and "Back up now", or `null` when neither does: a recovery kit must exist, and at least one target
 * must be set, the folder or Google Drive (connected, even while it asks to be reconnected: the target is set, it needs a
 * new sign-in). The kit comes first.
 */
export function scheduleBlocker(options: { hasKit: boolean; folder: string | null; driveConnected: boolean }): ScheduleBlocker | null {
  if (!options.hasKit) {
    return "kit";
  }
  return options.folder === null && !options.driveConnected ? "target" : null;
}

/** The outcome of the last scheduled backup: when and which file, or when and why it failed. */
function LastResult({ status }: { status: BackupRunStatus | null }) {
  if (status === null) {
    return <p className="muted">{t("security.scheduled.last.none")}</p>;
  }
  const when = formatTimestamp(status.at)?.text ?? "";
  if (status.ok) {
    return <p>{t("security.scheduled.last.ok", { date: when, file: status.file ?? "" })}</p>;
  }
  const good = status.lastSuccessAt === undefined ? null : formatTimestamp(status.lastSuccessAt);
  return (
    <>
      <p className="error-text">{t("security.scheduled.last.failed", { date: when, error: status.error ?? "" })}</p>
      {good !== null && <p className="muted">{t("security.scheduled.last.lastGood", { date: good.text })}</p>}
    </>
  );
}

/** The scheduled backups of the "Backup" part of Settings; not shown by a shell that cannot run them. */
export function ScheduledBackupSettings({ hasKit }: { hasKit: boolean }) {
  const available = useAppStore((state) => state.interopCapabilities.scheduledBackups);
  const scheduleSetting = useAppStore((state) => state.settings[BACKUP_SCHEDULE_KEY]);
  const folderSetting = useAppStore((state) => state.settings[BACKUP_FOLDER_KEY]);
  const lastSetting = useAppStore((state) => state.settings[BACKUP_LAST_KEY]);
  const loadSetting = useAppStore((state) => state.loadSetting);
  const setSetting = useAppStore((state) => state.setSetting);
  const driveAvailable = useAppStore((state) => state.interopCapabilities.drive);
  const driveConnected = useAppStore((state) => state.driveStatus?.connected === true);
  const loadDriveStatus = useAppStore((state) => state.loadDriveStatus);
  const chooseBackupFolder = useAppStore((state) => state.chooseBackupFolder);
  const runBackupNow = useAppStore((state) => state.runBackupNow);
  const schedule = useMemo(() => parseBackupSchedule(scheduleSetting), [scheduleSetting]);
  const folder = parseBackupFolder(folderSetting);
  const last = useMemo(() => parseBackupLast(lastSetting), [lastSetting]);
  // The text of the "keep" field while it is being typed; `null` shows the stored number.
  const [keepDraft, setKeepDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice>(NO_NOTICE);
  const titleId = useId();
  const noteId = useId();
  const intervalId = useId();
  const keepId = useId();
  const keepHintId = useId();

  // The three settings are read when the section opens (the shell writes two of them on its own).
  useEffect(() => {
    if (!available) {
      return;
    }
    for (const key of [BACKUP_SCHEDULE_KEY, BACKUP_FOLDER_KEY, BACKUP_LAST_KEY]) {
      loadSetting(key).catch((error: unknown) => {
        console.error("could not read a backup setting", toCoreError(error).code);
      });
    }
  }, [available, loadSetting]);

  // The Drive connection is read when the section opens; the core's `SettingChanged` keeps it fresh afterwards.
  useEffect(() => {
    if (available && driveAvailable) {
      loadDriveStatus().catch(() => undefined);
    }
  }, [available, driveAvailable, loadDriveStatus]);

  if (!available) {
    return null;
  }

  const blocker = scheduleBlocker({ hasKit, folder, driveConnected: driveAvailable && driveConnected });
  let unavailable: string | null = null;
  if (blocker === "kit") {
    unavailable = t("security.backup.needsKit");
  } else if (blocker === "target") {
    unavailable = driveAvailable ? t("security.scheduled.drive.needsTarget") : t("security.scheduled.needsFolder");
  }
  const off = unavailable !== null;

  async function save(next: BackupSchedule): Promise<void> {
    setNotice(NO_NOTICE);
    try {
      await setSetting(BACKUP_SCHEDULE_KEY, { enabled: next.enabled, interval: next.interval, keep: next.keep });
    } catch (failure) {
      setNotice({ kind: "error", text: t("security.scheduled.saveError", { message: toCoreError(failure).message }) });
    }
  }

  function commitKeep(): void {
    if (keepDraft === null) {
      return;
    }
    const keep = parseKeep(keepDraft);
    setKeepDraft(null);
    if (keep === null) {
      setNotice({ kind: "error", text: t("security.scheduled.keepError") });
    } else if (keep !== schedule.keep) {
      void save({ ...schedule, keep });
    }
  }

  async function choose(): Promise<void> {
    setNotice(NO_NOTICE);
    try {
      if ((await chooseBackupFolder()) === null) {
        setNotice({ kind: "info", text: t("security.scheduled.cancelled") });
      }
    } catch (failure) {
      setNotice({ kind: "error", text: t("security.scheduled.folderError", { message: toCoreError(failure).message }) });
    }
  }

  async function backUpNow(): Promise<void> {
    setNotice(NO_NOTICE);
    setBusy(true);
    try {
      const { path } = await runBackupNow();
      const file = fileNameOf(path);
      // The file went to each target that is set: the folder, Google Drive, or both.
      let text = t("security.scheduled.done", { file });
      if (driveAvailable && driveConnected) {
        text = t(folder === null ? "security.scheduled.drive.doneDrive" : "security.scheduled.drive.doneBoth", { file });
      }
      setNotice({ kind: "info", text });
    } catch (failure) {
      const reason = toCoreError(failure);
      const driveReason = driveReasonOf(reason);
      if (reason.code === "InvalidInput" && driveReason === undefined) {
        // The core refuses to write a backup of a workspace without a kit, as for the manual one.
        setNotice({ kind: "error", text: t("security.backup.needsKit") });
      } else {
        const message = driveReason === undefined ? reason.message : driveReasonText(driveReason);
        setNotice({ kind: "error", text: t("security.scheduled.error", { message }) });
      }
    }
    setBusy(false);
    if (driveAvailable) {
      // The shell recorded the delivery in `app.backup.drive`; reading it again does not wait for the event.
      loadDriveStatus().catch(() => undefined);
    }
  }

  return (
    <section className="subpanel scheduled-backups" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("security.scheduled.title")}
      </h3>
      <p className="muted">{t(driveAvailable ? "security.scheduled.drive.intro" : "security.scheduled.intro")}</p>
      {unavailable !== null && (
        <p className="note" id={noteId}>
          {unavailable}
        </p>
      )}

      <div className="field scheduled-backups-folder">
        <span className="scheduled-backups-label">{t("security.scheduled.folder")}</span>
        <div className="row scheduled-backups-folder-row">
          <span className={folder === null ? "muted" : "mono"}>{folder ?? t("security.scheduled.noFolder")}</span>
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => {
              void choose();
            }}
          >
            {t("security.scheduled.choose")}
          </button>
        </div>
      </div>

      {driveAvailable && <DriveBackupTarget />}

      <label className="checkbox-label scheduled-backups-enabled">
        <input
          type="checkbox"
          checked={schedule.enabled}
          disabled={off}
          aria-describedby={unavailable !== null ? noteId : undefined}
          onChange={(event) => {
            void save({ ...schedule, enabled: event.target.checked });
          }}
        />
        <span>{t("security.scheduled.enabled")}</span>
      </label>

      <div className="field scheduled-backups-interval">
        <label htmlFor={intervalId}>{t("security.scheduled.interval")}</label>
        <select
          id={intervalId}
          className="input select"
          value={schedule.interval}
          disabled={off}
          onChange={(event) => {
            const interval: BackupInterval = event.target.value === "weekly" ? "weekly" : "daily";
            void save({ ...schedule, interval });
          }}
        >
          <option value="daily">{t("security.scheduled.daily")}</option>
          <option value="weekly">{t("security.scheduled.weekly")}</option>
        </select>
      </div>

      <div className="field scheduled-backups-keep">
        <label htmlFor={keepId}>{t("security.scheduled.keep")}</label>
        <input
          id={keepId}
          className="input"
          type="number"
          inputMode="numeric"
          min={MIN_BACKUP_KEEP}
          max={MAX_BACKUP_KEEP}
          step={1}
          value={keepDraft ?? String(schedule.keep)}
          disabled={off}
          aria-describedby={keepHintId}
          onChange={(event) => {
            setKeepDraft(event.target.value);
          }}
          onBlur={commitKeep}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitKeep();
            } else if (event.key === "Escape") {
              setKeepDraft(null);
            }
          }}
        />
        <p className="muted scheduled-backups-hint" id={keepHintId}>
          {t("security.scheduled.keepHint")}
        </p>
      </div>

      <div className="scheduled-backups-last">
        <span className="scheduled-backups-label">{t("security.scheduled.last")}</span>
        <LastResult status={last} />
      </div>

      <div className="row-actions">
        <button
          type="button"
          className="button"
          disabled={off || busy}
          aria-describedby={unavailable !== null ? noteId : undefined}
          onClick={() => {
            void backUpNow();
          }}
        >
          {busy ? t("security.scheduled.running") : t("security.scheduled.now")}
        </button>
      </div>
      {notice.kind === "info" && (
        <p className="muted" role="status">
          {notice.text}
        </p>
      )}
      {notice.kind === "error" && (
        <p className="error-text" role="alert">
          {notice.text}
        </p>
      )}
    </section>
  );
}
