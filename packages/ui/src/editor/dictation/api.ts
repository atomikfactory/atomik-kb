// How the page editor and Settings reach the dictation of the transport (ADR-0053): through the `CoreClient` of the app,
// which the store hands out (`aiClient`, the one client every slice is made with).

import type { DictationApi } from "@atomik-kb/core-client";

import type { AppState, AppStore } from "../../store";
import { useAppStore } from "../../store-context";

/** The dictation of the app's transport, `null` where it has none (the in-memory core without `?dictation=fake`, the web build). */
export function dictationApiOf(state: AppState): DictationApi | null {
  return state.aiClient.hasDictation() ? state.aiClient.dictation() : null;
}

/** The dictation of the app's transport, `null` when it has none. */
export function useDictationApi(): DictationApi | null {
  return useAppStore(dictationApiOf);
}

/** Opens Settings at its AI section: Ollama's transcription model is chosen there. */
export function openAiSettings(store: AppStore): void {
  if (store.getState().route !== "settings") {
    store.getState().navigate("settings");
  }
  // After the screen is drawn (it is, once the route changed, by the next task).
  setTimeout(() => {
    document.getElementById("settings-ai")?.scrollIntoView({ block: "start" });
  }, 0);
}
