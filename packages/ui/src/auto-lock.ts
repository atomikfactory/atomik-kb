import { toCoreError } from "@atomik-kb/core-client";
import { useEffect } from "react";

import { ACTIVITY_EVENT } from "./activity";
import { useAppStore, useStoreApi } from "./store-context";
import { AUTO_LOCK_KEY, parseAutoLockMinutes } from "./store-security";

/**
 * What counts as activity in the window: each one starts the wait again. The last is the one the app reports itself
 * (`reportActivity`), for what is no input event of the window: a dictation that hears the person speak.
 */
export const ACTIVITY_EVENTS = ["mousemove", "keydown", "pointerdown", "focus", "wheel", ACTIVITY_EVENT] as const;

/**
 * Locks the app after `app.lock.auto_minutes` minutes without activity in the window, while
 * the app lock is on (ADR-0018: the timeout is a UI setting; locking is the core's).
 * Mounted with the main layout, so it only runs while the workspace is open.
 */
export function useAutoLock(): void {
  const store = useStoreApi();
  const active = useAppStore((state) => state.capabilities.appLock && state.security?.appLock.enabled === true);
  const minutes = useAppStore((state) => parseAutoLockMinutes(state.settings[AUTO_LOCK_KEY]));

  useEffect(() => {
    if (!active || minutes === 0) {
      return;
    }
    const wait = minutes * 60_000;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const expire = (): void => {
      store
        .getState()
        .lock()
        .catch((error: unknown) => {
          console.error("could not lock after inactivity", toCoreError(error).code);
        });
    };
    const restart = (): void => {
      clearTimeout(timer);
      timer = setTimeout(expire, wait);
    };
    restart();
    // Captured on the window: `focus` does not bubble, and nothing on the way can stop it.
    for (const type of ACTIVITY_EVENTS) {
      window.addEventListener(type, restart, { capture: true, passive: true });
    }
    return () => {
      clearTimeout(timer);
      for (const type of ACTIVITY_EVENTS) {
        window.removeEventListener(type, restart, { capture: true });
      }
    };
  }, [store, active, minutes]);
}
