// The actions of the writing assistant's menu (ADR-0037, `docs/architecture/ai.md` sections 3.2 and
// 8.3): every action of the prompt table except `meetingSummary` (that one is the recorder's), with the
// choices of `tone` and `translate`.

import type { AiAction, AiTone } from "@atomik-kb/core-client";

import type { MessageKey } from "../../i18n";

export interface WriterAction {
  id: Exclude<AiAction, "custom" | "meetingSummary">;
  label: MessageKey;
  /**
   * What it needs: `text` (a selection or blocks), or `any`, which also works at the cursor (continue
   * the page, or act on the page up to the cursor).
   */
  needs: "text" | "any";
  /** The sub-choice the menu asks for. */
  choice?: "tone" | "language";
}

export const WRITER_ACTIONS: readonly WriterAction[] = [
  { id: "continue", label: "aiWriter.action.continue", needs: "any" },
  { id: "improve", label: "aiWriter.action.improve", needs: "text" },
  { id: "fixGrammar", label: "aiWriter.action.fixGrammar", needs: "text" },
  { id: "shorter", label: "aiWriter.action.shorter", needs: "text" },
  { id: "longer", label: "aiWriter.action.longer", needs: "text" },
  { id: "simplify", label: "aiWriter.action.simplify", needs: "text" },
  { id: "tone", label: "aiWriter.action.tone", needs: "text", choice: "tone" },
  { id: "translate", label: "aiWriter.action.translate", needs: "text", choice: "language" },
  { id: "summarize", label: "aiWriter.action.summarize", needs: "any" },
  { id: "explain", label: "aiWriter.action.explain", needs: "text" },
  { id: "actionItems", label: "aiWriter.action.actionItems", needs: "any" },
];

/** The tones of `tone`, in the order the sub-menu lists them. */
export const WRITER_TONES: readonly { id: AiTone; label: MessageKey }[] = [
  { id: "professional", label: "aiWriter.tone.professional" },
  { id: "casual", label: "aiWriter.tone.casual" },
  { id: "friendly", label: "aiWriter.tone.friendly" },
  { id: "confident", label: "aiWriter.tone.confident" },
  { id: "direct", label: "aiWriter.tone.direct" },
];

/** The languages `translate` offers; `name` is what the model is asked for. */
export const WRITER_LANGUAGES: readonly { name: string; label: MessageKey }[] = [
  { name: "English", label: "aiWriter.language.english" },
  { name: "French", label: "aiWriter.language.french" },
  { name: "Spanish", label: "aiWriter.language.spanish" },
  { name: "German", label: "aiWriter.language.german" },
  { name: "Italian", label: "aiWriter.language.italian" },
  { name: "Portuguese", label: "aiWriter.language.portuguese" },
  { name: "Chinese", label: "aiWriter.language.chinese" },
  { name: "Japanese", label: "aiWriter.language.japanese" },
];

/** What a run is asked to do. */
export interface WriterChoice {
  action: AiAction;
  tone?: AiTone;
  language?: string;
  prompt?: string;
}

/** The label of what a run does, for the panel's header. */
export function labelOfChoice(choice: WriterChoice): MessageKey {
  if (choice.action === "custom") {
    return "aiWriter.action.custom";
  }
  return WRITER_ACTIONS.find((entry) => entry.id === choice.action)?.label ?? "aiWriter.action.custom";
}
