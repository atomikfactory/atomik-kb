// A writing action run to its end, without streaming into the page: the meeting summary of a
// transcript (ADR-0037, section 8.6). The stream is collected and the cleaned answer returned; a
// cancellation (the signal, or the shell's `cancelled` error) rejects with `GenerationCancelled`.

import { CoreError, toCoreError } from "@atomik-kb/core-client";
import type { AiGenerateInput, ApiError } from "@atomik-kb/core-client";

import { detailOf } from "../../errors";
import { newRequestId } from "./ai-client";
import type { AiEditorClient } from "./ai-client";
import { cleanOutput } from "./text";

/** The run was cancelled (by the person, or because the page went away). */
export class GenerationCancelled extends Error {
  constructor() {
    super("cancelled");
    this.name = "GenerationCancelled";
  }
}

/** Whether an error of the stream is the shell's answer to a cancellation. */
export function isCancelledError(error: ApiError): boolean {
  return detailOf(toCoreError(error), "reason") === "cancelled";
}

/** Runs `input` and returns the cleaned answer. */
export async function generateText(
  client: Pick<AiEditorClient, "aiGenerate" | "aiCancel">,
  input: AiGenerateInput,
  signal?: AbortSignal,
): Promise<string> {
  const aborted = (): boolean => signal?.aborted === true;
  if (aborted()) {
    throw new GenerationCancelled();
  }
  const requestId = newRequestId();
  const run: { streamed: string; final: string | null; failure: ApiError | null } = { streamed: "", final: null, failure: null };
  const cancel = (): void => {
    client.aiCancel(requestId).catch(() => undefined);
  };
  signal?.addEventListener("abort", cancel, { once: true });
  try {
    await client.aiGenerate(requestId, input, (event) => {
      switch (event.type) {
        case "delta":
          run.streamed += event.text;
          break;
        case "done":
          run.final = event.text;
          break;
        case "error":
          run.failure = event.error;
          break;
        case "sources":
          break;
      }
    });
  } finally {
    signal?.removeEventListener("abort", cancel);
  }
  if (aborted()) {
    throw new GenerationCancelled();
  }
  if (run.failure !== null) {
    throw isCancelledError(run.failure) ? new GenerationCancelled() : new CoreError(run.failure);
  }
  return cleanOutput(run.final ?? run.streamed);
}
