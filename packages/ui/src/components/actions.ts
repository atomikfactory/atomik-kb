import { createContext, useContext } from "react";

import type { MessageKey } from "../i18n";

/**
 * Runs a store action the UI started and, when the core refuses it, shows the reason in
 * the sidebar's alert (`errorKey` is the message, with a `{message}` placeholder).
 * Never rejects, so callers can fire and forget.
 */
export type RunAction = (action: Promise<unknown>, errorKey: MessageKey) => Promise<void>;

export const ActionContext = createContext<RunAction>(() => Promise.resolve());

export function useRunAction(): RunAction {
  return useContext(ActionContext);
}
