// The dictation of one page editor (ADR-0053, `docs/architecture/dictation.md` section 6): a controller made once per
// editor and registered by editor (the `/` menu, which runs outside React, reads it), the bar and the first-use dialog it
// draws, and the app's own reasons to end it: a page or tab change (dropped, with a toast) and a lock (dropped). (Esc and a
// page that turns read-only are the controller's, which follows the editor.) Nothing is made where the transport cannot dictate.

import type { Editor } from "@tiptap/core";
import { useEffect, useMemo } from "react";
import type { ReactNode } from "react";

import { t } from "../../i18n";
import type { AppState } from "../../store";
import { useStoreApi } from "../../store-context";
import { editorAiOf } from "../ai/EditorAi";
import { useDictationApi } from "./api";
import { DictationController } from "./controller";
import { DictationBar } from "./DictationBar";
import { registerDictation, unregisterDictation } from "./registry";
import { SetupDialog } from "./SetupDialog";

/** What the app's state says about where the person is: the page and the tab in front, and whether the app is open. */
type Whereabouts = Pick<AppState, "selectedPageId" | "activeTabId" | "phase">;

/**
 * Whether the person left: another page or tab came to the front, or the app is no longer open (a lock, a failure). The
 * microphone does not stay on for a page nobody looks at, nor for a locked app.
 */
export function leftThePage(state: Whereabouts, previous: Whereabouts): boolean {
  return (
    state.selectedPageId !== previous.selectedPageId ||
    state.activeTabId !== previous.activeTabId ||
    (state.phase !== previous.phase && state.phase !== "ready")
  );
}

/** Whether the editor's recorder is recording, pausing or saving: it holds the microphone. */
function recorderRuns(editor: Editor): boolean {
  const phase = editorAiOf(editor)?.recorder.state.get().phase;
  return phase !== undefined && phase !== "idle" && phase !== "error";
}

interface ProviderProps {
  editor: Editor | null;
  children: ReactNode;
}

export function DictationEditorProvider({ editor, children }: ProviderProps) {
  const store = useStoreApi();
  const api = useDictationApi();

  const controller = useMemo(
    () =>
      editor === null || api === null
        ? null
        : new DictationController({ editor, api: () => api, recording: () => recorderRuns(editor) }),
    [editor, api],
  );

  useEffect(() => {
    if (controller === null || editor === null) {
      return;
    }
    registerDictation(editor, controller);
    const detach = controller.attach();
    const unwatch = store.subscribe((state, previous) => {
      if (!leftThePage(state, previous)) {
        return;
      }
      // The editor goes away with the page, so a phrase still being recognised has no document to go into: the dictation
      // is dropped, not finished. Another page or tab says so in a toast; a lock needs no word (and shows nothing).
      const lost = controller.leave();
      if (lost && state.phase === "ready") {
        state.showToast(t("dictation.leftPage"));
      }
    });
    return () => {
      unwatch();
      detach();
      unregisterDictation(editor, controller);
    };
  }, [controller, editor, store]);

  return (
    <>
      {children}
      {controller !== null && (
        <>
          <DictationBar controller={controller} />
          <SetupDialog controller={controller} />
        </>
      )}
    </>
  );
}
