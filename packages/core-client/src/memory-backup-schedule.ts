// The pure helpers of scheduled backups (ADR-0036, `docs/architecture/import-export.md`, 5.4), the
// in-memory twin of `kb-core`'s `backup_schedule.rs`: the names of the backup files a schedule
// writes (`atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb`, local time) and which of the files in a
// folder to delete. The mirror has no timer: `MemoryTransport.backupRunNow` does one run.

import { AI_ENDPOINT_SETTING } from "./ai";
import type { BackupSchedule } from "./generated/BackupSchedule";
import type { JsonValue } from "./generated/serde_json/JsonValue";

/** The device setting holding the {@link BackupSchedule} (written by the UI). */
export const BACKUP_SCHEDULE_SETTING = "app.backup.schedule";
/** The device setting holding the folder of scheduled backups (written by the shell only). */
export const BACKUP_FOLDER_SETTING = "app.backup.folder";
/** The device setting holding the outcome of the last run (written by the shell only). */
export const BACKUP_LAST_SETTING = "app.backup.last";
/** The Google Drive target of this device (ADR-0059), written only by the shell (or `MemoryDrive`). */
export const BACKUP_DRIVE_SETTING = "app.backup.drive";
/**
 * The settings that belong to this device and to the shell's own dialogs, the twin of `kb-core`'s `DEVICE_BOUND_SETTINGS`: the
 * Ollama endpoint with its consent, the folder of scheduled backups, their last outcome and the Google Drive connection
 * (ADR-0059). A backup leaves them out and a restore ignores them.
 */
export const DEVICE_BOUND_SETTINGS: readonly string[] = [
  AI_ENDPOINT_SETTING,
  BACKUP_FOLDER_SETTING,
  BACKUP_LAST_SETTING,
  BACKUP_DRIVE_SETTING,
];
/** The prefix of the file names of scheduled backups. */
export const BACKUP_AUTO_PREFIX = "atomik-kb-auto-";
/** How many scheduled backups are kept unless the schedule says otherwise. */
export const DEFAULT_BACKUP_KEEP = 7;
/** The range `keep` may take. */
export const MIN_BACKUP_KEEP = 1;
export const MAX_BACKUP_KEEP = 100;

const AUTO_NAME = /^atomik-kb-auto-\d{4}-\d{2}-\d{2}-\d{4}\.atomikkb$/u;

/** The file name of a scheduled backup made at `ms`: `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb`, local time. */
export function autoBackupName(ms: number): string {
  const when = new Date(ms);
  const two = (n: number): string => String(n).padStart(2, "0");
  const date = `${String(when.getFullYear()).padStart(4, "0")}-${two(when.getMonth() + 1)}-${two(when.getDate())}`;
  return `${BACKUP_AUTO_PREFIX}${date}-${two(when.getHours())}${two(when.getMinutes())}.atomikkb`;
}

/** Whether `name` is exactly the form {@link autoBackupName} writes (nothing else is ever deleted). */
export function isAutoBackupName(name: string): boolean {
  return AUTO_NAME.test(name);
}

const AUTO_NAME_TIME = /^atomik-kb-auto-(\d{4})-(\d{2})-(\d{2})-(\d{2})(\d{2})\.atomikkb$/u;

/**
 * When (Unix ms, to the minute) the scheduled backup named `name` was made, local time, the inverse of
 * {@link autoBackupName} (`kb-core`'s `name_time_ms`); `undefined` for a name that is not a scheduled backup's, impossible
 * dates included.
 */
export function backupNameTime(name: string): number | undefined {
  const match = AUTO_NAME_TIME.exec(name);
  if (match === null) {
    return undefined;
  }
  const [year = 0, month = 0, day = 0, hour = 0, minute = 0] = match.slice(1).map(Number);
  const ms = new Date(year, month - 1, day, hour, minute).getTime();
  return Number.isNaN(ms) || autoBackupName(ms) !== name ? undefined : ms;
}

/** After a failed attempt nothing is tried for an hour; a backup is due a little before its period is over (`kb-core`). */
const PERIOD_SLACK_MS = 15 * 60_000;
const DAY_MS = 86_400_000;

/**
 * Whether the period of a backup made at `madeAt` is over at `now`: a run made then would be due again (`kb-core`'s `is_due`
 * for a success: a day or a week less 15 minutes, or a clock set back by more than a day).
 */
export function backupPeriodOver(madeAt: number, now: number, interval: BackupSchedule["interval"]): boolean {
  const age = now - madeAt;
  return age >= (interval === "weekly" ? 7 * DAY_MS : DAY_MS) - PERIOD_SLACK_MS || age < -DAY_MS;
}

/**
 * Which of `names` (the files of the folder) to delete so that the newest `keep` scheduled backups
 * stay: only names of the exact automatic form are listed, oldest first.
 */
export function backupsToPrune(names: readonly string[], keep: number): string[] {
  const own = names.filter(isAutoBackupName).sort();
  return own.slice(0, Math.max(0, own.length - Math.max(0, keep)));
}

/** The `keep` of a stored schedule (1 to 100, default 7). */
export function keepOf(stored: JsonValue | undefined): number {
  const keep = typeof stored === "object" && stored !== null && !Array.isArray(stored) ? stored.keep : undefined;
  return typeof keep === "number" && Number.isInteger(keep)
    ? Math.min(MAX_BACKUP_KEEP, Math.max(MIN_BACKUP_KEEP, keep))
    : DEFAULT_BACKUP_KEEP;
}

/** The schedule of the setting, with the defaults for what is missing (disabled, daily, keep 7). */
export function scheduleOf(stored: JsonValue | undefined): BackupSchedule {
  const raw = typeof stored === "object" && stored !== null && !Array.isArray(stored) ? stored : {};
  return {
    enabled: raw.enabled === true,
    interval: raw.interval === "weekly" ? "weekly" : "daily",
    keep: keepOf(stored),
  };
}
