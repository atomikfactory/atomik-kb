// Activity that is not an input event of the window but still means the person is using the app: it keeps the auto-lock
// timer (`auto-lock.ts`) from running out, the way typing does. Dictation reports each phrase it hears (ADR-0053,
// `docs/architecture/dictation.md` section 6): someone who talks for minutes without touching the keyboard is at work.

/** The event the window gets from {@link reportActivity}. */
export const ACTIVITY_EVENT = "atomik:activity";

/** Says that the person is active now: the auto-lock wait starts again. Does nothing outside a window. */
export function reportActivity(): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(ACTIVITY_EVENT));
  }
}
