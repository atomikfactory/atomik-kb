// The dictation of each page editor, by editor, for code that runs outside React (the `/` menu reads it; the recorder
// asks whether dictation runs). `EditorDictation.tsx` registers a controller while the editor is open and the transport can
// dictate; an editor with none (a schema-only editor, a transport without dictation) offers no `/microphone`.

import type { Editor } from "@tiptap/core";

import type { MessageKey } from "../../i18n";
import type { DictationController } from "./controller";

const registry = new WeakMap<Editor, DictationController>();

/** Makes `controller` the dictation of `editor`. */
export function registerDictation(editor: Editor, controller: DictationController): void {
  registry.set(editor, controller);
}

/** Forgets the dictation of `editor`, unless another controller was registered since. */
export function unregisterDictation(editor: Editor, controller: DictationController): void {
  if (registry.get(editor) === controller) {
    registry.delete(editor);
  }
}

/** The dictation of an editor, `null` for an editor without one. */
export function dictationOf(editor: Editor): DictationController | null {
  return registry.get(editor) ?? null;
}

/** Whether `/microphone` is on offer in `editor`: the transport can dictate (the `/` menu hides the entry when it cannot). */
export function dictationOn(editor: Editor): boolean {
  return registry.has(editor);
}

/** Whether dictation runs in `editor` (or its first-use dialog is open): the recorder does not start then. */
export function dictationRunning(editor: Editor): boolean {
  return registry.get(editor)?.busy === true;
}

/**
 * Why `/microphone` cannot run in `editor` now (shown in place of its hint), `null` when it can: the page is read-only,
 * dictation runs already, the recorder holds the microphone, or this device has no microphone API.
 */
export function dictationUnavailable(editor: Editor): MessageKey | null {
  const controller = registry.get(editor);
  if (!editor.isEditable) {
    return "slash.microphone.readOnly";
  }
  if (controller === undefined) {
    return "slash.microphone.unsupported";
  }
  if (controller.busy) {
    return "slash.microphone.busy";
  }
  if (controller.recording) {
    return "slash.microphone.recording";
  }
  return controller.canCapture ? null : "slash.microphone.unsupported";
}
