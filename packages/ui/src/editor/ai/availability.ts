// Which AI features the editor offers (ADR-0037, `docs/architecture/ai.md` sections 2 and 8): the
// shell must have AI (`hasAi`), the person must have turned it on (`app.ai.settings`, `enabled`), and
// each feature needs its model: the writing assistant a chat model, the transcription a model that
// understands audio. While AI is off every entry point is hidden.

import { AI_SETTINGS_SETTING, aiSettingsOf } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { useMemo } from "react";

import { useAppStore } from "../../store-context";
import { useAiClient } from "./ai-client";
import type { AiEditorClient } from "./ai-client";

export interface AiCapabilities {
  /** The shell has AI and it is turned on (the recorder and the settings-dependent entries). */
  enabled: boolean;
  /** The writing assistant: AI on and a chat model chosen. */
  writing: boolean;
  /** Transcription: AI on and a model that understands audio chosen. */
  transcription: boolean;
  /** The chat model (the writing assistant and the summaries), `null` when none is chosen. */
  chatModel: string | null;
  /** The transcription model, `null` when none is chosen. */
  transcriptionModel: string | null;
}

/** Nothing: what AI off, or a shell without AI, gives. */
export const NO_AI: AiCapabilities = {
  enabled: false,
  writing: false,
  transcription: false,
  chatModel: null,
  transcriptionModel: null,
};

function modelOf(value: string | undefined): string | null {
  return value === undefined || value.trim() === "" ? null : value;
}

/** What the editor offers for a client and the stored `app.ai.settings` (absent: AI off). */
export function aiCapabilitiesOf(
  client: Pick<AiEditorClient, "hasAi"> | null,
  stored: JsonValue | null | undefined,
): AiCapabilities {
  if (client === null || !client.hasAi()) {
    return NO_AI;
  }
  const settings = aiSettingsOf(stored);
  if (!settings.enabled) {
    return NO_AI;
  }
  const chatModel = modelOf(settings.chatModel);
  const transcriptionModel = modelOf(settings.transcriptionModel);
  return {
    enabled: true,
    writing: chatModel !== null,
    transcription: transcriptionModel !== null,
    chatModel,
    transcriptionModel,
  };
}

/** The AI features on offer now, kept fresh as the settings change. */
export function useAiCapabilities(): AiCapabilities {
  const client = useAiClient();
  const stored = useAppStore((state) => state.settings[AI_SETTINGS_SETTING]);
  return useMemo(() => aiCapabilitiesOf(client, stored), [client, stored]);
}
