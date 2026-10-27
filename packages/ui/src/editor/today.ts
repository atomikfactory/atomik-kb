// "Today" for every date mention and date picker that is open. A mention shows "Today",
// "Tomorrow" or "Yesterday" for those days, so what it shows changes at local midnight: one
// timer, shared by everything that listens, wakes at the next midnight, and a window that gets
// the focus back (or becomes visible again after the machine slept) looks at the clock too.
// With nothing listening there is no timer and no event handler.

import { useSyncExternalStore } from "react";

import { todayIso } from "./dates";

const listeners = new Set<() => void>();
let current = todayIso();
let timer: ReturnType<typeof setTimeout> | undefined;

/** Milliseconds from `now` to the next local midnight, plus a moment so that the clock has crossed it. */
function untilMidnight(now: Date): number {
  const next = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, 0, 0, 0, 0);
  return Math.max(1000, next.getTime() - now.getTime() + 500);
}

function schedule(): void {
  clearTimeout(timer);
  timer = setTimeout(refresh, untilMidnight(new Date()));
}

/** Reads the clock; tells the listeners when the day changed, and waits for the next midnight. */
function refresh(): void {
  const next = todayIso();
  if (next !== current) {
    current = next;
    for (const listener of [...listeners]) {
      listener();
    }
  }
  if (listeners.size > 0) {
    schedule();
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    current = todayIso();
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    schedule();
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      clearTimeout(timer);
      timer = undefined;
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    }
  };
}

function getSnapshot(): string {
  // Nobody listens: nothing keeps `current` fresh, so read the clock.
  return listeners.size === 0 ? todayIso() : current;
}

/** The local day of this device ("2026-10-07"), redrawn when midnight passes or the window regains focus. */
export function useToday(): string {
  return useSyncExternalStore(subscribe, getSnapshot);
}
