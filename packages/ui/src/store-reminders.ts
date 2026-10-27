// The reminders part of the app store (ADR-0024, reminders on date mentions): the in-app toasts
// and, for a core with no shell around it, the timer that drives the core's scheduler.
//
// The core emits `ReminderDue` once per reminder, when the scheduler ticks (the desktop shell ticks
// its core every 30 seconds and shows an OS notification as well; the in-memory core of the
// browser page has no shell, so `startScheduler` ticks it from here, every 15 seconds). Each event
// becomes a toast, "Reminder: <page title> - <text>", with an "Open" button that goes to the page
// and scrolls to the block, dismissed by itself after 15 seconds; several stack, at most five
// (the oldest go first: the OS notification centre keeps the rest on the desktop). Nothing is
// kept while the workspace is locked: the toasts hold page titles and text.

import { toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, Event as CoreEvent, ReminderDue } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

/** How long a toast stays when nobody dismisses it, in milliseconds. */
export const REMINDER_TOAST_MS = 15_000;

/** Most toasts on screen at once; a newer one pushes the oldest out. */
export const MAX_REMINDER_TOASTS = 5;

/** How often the UI ticks a core that has no scheduler of its own, in milliseconds. */
export const REMINDER_TICK_MS = 15_000;

/** One reminder on screen. */
export interface ReminderToast {
  /** Distinct for the life of the store. */
  id: number;
  reminder: ReminderDue;
}

export interface RemindersData {
  /** The toasts, oldest first. */
  reminderToasts: readonly ReminderToast[];
}

export const EMPTY_REMINDERS: RemindersData = { reminderToasts: [] };

export interface RemindersActions {
  /** Removes a toast (a click on its close button, or its 15 seconds). */
  dismissReminderToast: (id: number) => void;
  /** Opens the page of a toast at the block that holds the mention, and removes the toast. */
  openReminderToast: (id: number) => void;
}

export type RemindersState = RemindersData & RemindersActions;

export interface RemindersSlice {
  initial: RemindersState;
  onEvent: (event: CoreEvent) => void;
  /**
   * Runs one tick of the core's scheduler when the transport wants the UI to (the in-memory
   * core); a no-op for the desktop shell, which ticks its core itself. Only while the app is
   * ready, so nothing runs for an onboarding, a lock screen or a fatal error.
   */
  tick: () => void;
  /**
   * Ticks every `REMINDER_TICK_MS` for as long as the returned function has not been called;
   * nothing happens (and nothing is scheduled) for a transport that has its own scheduler.
   */
  startScheduler: () => () => void;
}

export function createRemindersSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): RemindersSlice {
  let nextId = 0;
  const timers = new Map<number, ReturnType<typeof setTimeout>>();

  function forget(id: number): void {
    const timer = timers.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      timers.delete(id);
    }
  }

  function dismiss(id: number): void {
    forget(id);
    set((state) =>
      state.reminderToasts.some((toast) => toast.id === id)
        ? { reminderToasts: state.reminderToasts.filter((toast) => toast.id !== id) }
        : state,
    );
  }

  function show(reminder: ReminderDue): void {
    const existing = get().reminderToasts;
    // The same reminder twice (a replayed event) is one toast.
    if (existing.some((toast) => toast.reminder.nodeId === reminder.nodeId && toast.reminder.fireAt === reminder.fireAt)) {
      return;
    }
    nextId += 1;
    const toast: ReminderToast = { id: nextId, reminder };
    const kept = [...existing, toast];
    const dropped = kept.slice(0, Math.max(0, kept.length - MAX_REMINDER_TOASTS));
    for (const old of dropped) {
      forget(old.id);
    }
    set({ reminderToasts: kept.slice(dropped.length) });
    timers.set(
      toast.id,
      setTimeout(() => {
        dismiss(toast.id);
      }, REMINDER_TOAST_MS),
    );
  }

  const initial: RemindersState = {
    ...EMPTY_REMINDERS,

    dismissReminderToast: dismiss,

    openReminderToast: (id) => {
      const toast = get().reminderToasts.find((candidate) => candidate.id === id);
      if (toast === undefined) {
        return;
      }
      const { pageId, spaceId, blockId } = toast.reminder;
      get().navigateTo({ pageId, spaceId, blockId });
      dismiss(id);
    },
  };

  function tick(): void {
    if (!client.hasRemindersTick() || get().phase !== "ready") {
      return;
    }
    client.remindersTick().catch((error: unknown) => {
      console.error("could not tick the reminder scheduler", toCoreError(error).code);
    });
  }

  return {
    initial,
    tick,
    startScheduler: () => {
      if (!client.hasRemindersTick()) {
        return () => undefined;
      }
      const timer = setInterval(tick, REMINDER_TICK_MS);
      return () => {
        clearInterval(timer);
      };
    },
    onEvent: (event) => {
      if (event.type === "ReminderDue" && get().phase !== "locked") {
        show(event.data);
      }
    },
  };
}
