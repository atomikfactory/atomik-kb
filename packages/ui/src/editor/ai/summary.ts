// The summary of a transcript (ADR-0037, `docs/architecture/ai.md` sections 3.2 and 8.6): the
// `meetingSummary` action over the words of the transcript, collected (not streamed into the page). A
// transcript longer than one request takes (24,000 characters, the shell's own threshold) is summarised
// in parts, which one last request merges into a single summary with the same three sections.

import { generateText } from "./generate";
import type { AiEditorClient } from "./ai-client";
import { MAX_TEXT_CHARS, splitParts, SUMMARY_PART_CHARS, transcriptPlainText } from "./text";
import type { TranscriptSegment } from "./text";

/** The instruction that merges the summaries of the parts (`docs/architecture/ai.md` section 3.2). */
export const MERGE_PROMPT =
  "Merge these partial meeting summaries into one with the same three sections: ### Summary, ### Key points, ### Action items.";

/** How far a summary is: parts done of parts to do. */
export interface SummaryProgress {
  done: number;
  total: number;
}

/**
 * The Markdown summary of `segments`, `null` when they hold no words. Rejects with `GenerationCancelled`
 * when `signal` aborts, or with the shell's error.
 */
export async function summariseTranscript(
  client: Pick<AiEditorClient, "aiGenerate" | "aiCancel">,
  segments: readonly TranscriptSegment[],
  signal?: AbortSignal,
  onProgress?: (progress: SummaryProgress) => void,
): Promise<string | null> {
  const text = transcriptPlainText(segments);
  if (text === "") {
    return null;
  }
  const parts = splitParts(text, SUMMARY_PART_CHARS);
  if (parts.length === 1) {
    onProgress?.({ done: 0, total: 1 });
    return generateText(client, { action: "meetingSummary", text }, signal);
  }
  const total = parts.length + 1; // the parts, then the merge
  const summaries: string[] = [];
  for (const [index, part] of parts.entries()) {
    onProgress?.({ done: index, total });
    summaries.push(await generateText(client, { action: "meetingSummary", text: part }, signal));
  }
  onProgress?.({ done: parts.length, total });
  const joined = summaries.join("\n\n---\n\n");
  return generateText(
    client,
    { action: "custom", prompt: MERGE_PROMPT, text: joined.length > MAX_TEXT_CHARS ? joined.slice(0, MAX_TEXT_CHARS) : joined },
    signal,
  );
}
