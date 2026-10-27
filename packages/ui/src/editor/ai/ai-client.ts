// How the editor's AI features reach the shell's AI commands (ADR-0037, `docs/architecture/ai.md`
// section 6.2): through the `CoreClient` of the app, which the store hands out as `aiClient` (store-ai.ts).
//
// `AiEditorClient` is the part of `CoreClient` the editor calls (so a test can give it a smaller
// object): whether the shell has AI at all, a streamed writing action, its cancellation and the
// transcription of one WAV segment.

import type { AiGenerateInput, AiStreamEvent, AiTranscribed } from "@atomik-kb/core-client";

import type { AppState } from "../../store";
import { useAppStore } from "../../store-context";

export interface AiEditorClient {
  /** The transport reaches Ollama through the shell. */
  hasAi(): boolean;
  /** Streams a writing action; resolves when the stream ended. */
  aiGenerate(requestId: string, input: AiGenerateInput, onEvent: (event: AiStreamEvent) => void): Promise<void>;
  /** Cancels a stream (its last event is an `error` with reason `cancelled`). */
  aiCancel(requestId: string): Promise<void>;
  /** Transcribes one 16 kHz mono WAV segment. */
  aiTranscribe(wav: Uint8Array, model?: string): Promise<AiTranscribed>;
}

/** What replaces the store's client in a test. */
let override: AiEditorClient | null | undefined;

/** Tests: use `client` in place of the store's `aiClient` (`undefined` goes back to the store). */
export function setAiClientForTests(client: AiEditorClient | null | undefined): void {
  override = client;
}

/** The AI client of the app, `null` when the shell has none to give. */
export function aiClientOf(state: AppState): AiEditorClient | null {
  return override !== undefined ? override : state.aiClient;
}

/** The AI client of the app, kept fresh (a stable object: the client itself). */
export function useAiClient(): AiEditorClient | null {
  return useAppStore(aiClientOf);
}

/** 1 to 64 characters of `[A-Za-z0-9_-]`: what the shell takes as the name of a stream. */
export function newRequestId(): string {
  return `ai-${crypto.randomUUID()}`;
}
