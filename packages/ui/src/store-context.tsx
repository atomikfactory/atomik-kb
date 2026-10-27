import { createContext, useContext } from "react";
import { useStore } from "zustand";

import type { AppState, AppStore } from "./store";

export const StoreContext = createContext<AppStore | null>(null);

/** Reads from the app's store; only valid below `<App>`. */
export function useAppStore<T>(selector: (state: AppState) => T): T {
  const store = useContext(StoreContext);
  if (store === null) {
    throw new Error("useAppStore must be used inside <App>");
  }
  return useStore(store, selector);
}

/** The store itself, for code that reads the latest state outside of rendering (event handlers). */
export function useStoreApi(): AppStore {
  const store = useContext(StoreContext);
  if (store === null) {
    throw new Error("useStoreApi must be used inside <App>");
  }
  return store;
}
