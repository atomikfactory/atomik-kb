// Helpers of the tests of the editor's AI features: the app over the in-memory core with the fake AI
// (`ai: { mode: "fake" }`), the settings that turn it on, and a client for the editor to call it through.

import { AI_SETTINGS_SETTING, CoreClient } from "@atomik-kb/core-client";
import type { AiSettings, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act } from "@testing-library/react";

import { renderPages } from "../../test/page";
import { setAiClientForTests } from "./ai-client";

/** AI on, with a chat model and a transcription model. */
export const AI_ON: AiSettings = {
  enabled: true,
  chatModel: "qwen3:8b",
  transcriptionModel: "r2t2:latest",
  semanticIndex: true,
  think: false,
};

/** Renders the app over `spaces` with the fake AI; `aiSettings` is what `app.ai.settings` holds (`null`: nothing stored). */
export async function renderAiPages(
  spaces: readonly SeedSpace[],
  { aiSettings = AI_ON, ...options }: { aiSettings?: AiSettings | null } & Parameters<typeof renderPages>[1] = {},
) {
  const app = await renderPages(spaces, { ai: { mode: "fake" }, ...options });
  const client = new CoreClient(app.memory);
  setAiClientForTests(client);
  if (aiSettings !== null) {
    await client.settingsSet(AI_SETTINGS_SETTING, aiSettings);
  }
  const ai = app.memory.ai;
  if (ai === undefined) {
    throw new Error("the in-memory transport has no AI");
  }
  return { ...app, client, ai };
}

/** Gives the editor focus and selects `from` to `to` (the bubble menu shows only for a focused selection). */
export function selectText(editor: Editor, from: number, to: number = from): void {
  editor.view.dom.tabIndex = 0; // jsdom only focuses a contenteditable that has one
  act(() => {
    editor.chain().focus().setTextSelection({ from, to }).run();
  });
}

/** Puts the cursor at `pos` without a selection. */
export function placeCursor(editor: Editor, pos: number): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
  });
}

/** Top-level blocks as `type:text`, for compact assertions. */
export function outline(editor: Editor): string[] {
  const lines: string[] = [];
  editor.state.doc.forEach((node) => {
    const level = node.type.name === "heading" ? String(node.attrs.level) : "";
    lines.push(`${node.type.name}${level}:${node.textContent}`);
  });
  return lines;
}

/** Ends what `renderAiPages` set up. */
export function resetAi(): void {
  setAiClientForTests(undefined);
}
