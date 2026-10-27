// Reminders on date mentions (ADR-0024), the parts the TypeScript side shares with the core:
// the clock they run on, the window of reminders a tick looks at and what a tick fires. The
// in-memory core (`memory.ts`, `content.ts`) mirrors `kb-core`'s `reminders.rs` with them, and
// the UI reads the device's wall clock with `localNowSeconds` for `CoreClient.remindersDue`.
//
// A date mention is a floating local date (ADR-0016): "7 October 14:30" is that wall clock time
// wherever its reader is. So a reminder is due when this device's wall clock reaches its moment,
// and every time here is on that clock: seconds since 1970-01-01T00:00:00 as the local clock
// reads them (UTC plus the device's offset). No time zone data is needed, and a clock that
// jumps (daylight saving, a journey) skips nothing and repeats nothing: the mark of the last
// tick never moves back.

import type { ReminderDue } from "./generated/ReminderDue";

/**
 * Setting key of the per-device mark: the fire time (on the local wall clock, in seconds) up to
 * which reminders have been fired. Internal like the core's: nothing in the UI reads or writes it.
 */
export const LAST_CHECKED_SETTING = "app.reminders.last_checked";

/** How far back a tick looks for reminders it missed, in seconds: seven days. */
export const CATCH_UP_SECONDS = 7 * 24 * 60 * 60;

/** Most reminders `RemindersDue` returns. */
export const MAX_REMINDERS_DUE = 100;

/**
 * Most reminders one tick fires: more of them due at once (after a long absence) come one
 * batch per tick. The ones that fire at the same moment as the last of a batch go with it.
 */
export const FIRES_PER_TICK = 25;

/**
 * The device's wall clock at `now`: seconds since 1970-01-01T00:00:00 as the local clock reads
 * them, that is UTC plus the offset of the machine's time zone at that moment (daylight saving
 * included). This is the `until` of `CoreClient.remindersDue` and the "now" of a tick.
 */
export function localNowSeconds(now: Date = new Date()): number {
  return Math.floor(now.getTime() / 1000) - now.getTimezoneOffset() * 60;
}

/** Where the window of due reminders starts (exclusive): the mark, never more than seven days before `until`. */
export function windowStart(last: number | null, until: number): number {
  const floor = until - CATCH_UP_SECONDS;
  return last === null ? floor : Math.max(last, floor);
}

/** The reminders with a fire time after `after` and not after `until`, oldest first, at most `limit`. */
export type ReminderSource = (after: number, until: number, limit: number) => ReminderDue[];

/**
 * What one tick of the scheduler does: the reminders to fire (oldest first, at most
 * `FIRES_PER_TICK`, the ones that fire at the same moment as the last of them included) and the
 * mark to store (now, or the fire time of the last of them when some are left for the next tick;
 * never before the old mark). Mirrors `Core::reminders_tick`.
 */
export function planTick(
  source: ReminderSource,
  last: number | null,
  until: number,
): { fire: ReminderDue[]; mark: number } {
  const after = windowStart(last, until);
  const batch = source(after, until, FIRES_PER_TICK + 1);
  if (batch.length > FIRES_PER_TICK) {
    const cut = batch[FIRES_PER_TICK - 1]?.fireAt ?? until;
    return { fire: source(after, cut, MAX_REMINDERS_DUE), mark: cut };
  }
  return { fire: batch, mark: last === null ? until : Math.max(last, until) };
}
