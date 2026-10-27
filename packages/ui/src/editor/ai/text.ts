// Text rules of the AI features that need no editor: cleaning what a model streams, the timestamps and
// paragraphs of a transcript, splitting a long transcript for its summary, the name of a recording.

import type { JsonValue } from "@atomik-kb/core-client";

import { t } from "../../i18n";

/**
 * What a model's raw answer shows the person: `<think>...</think>` blocks removed (a block still open
 * hides the rest, as it is while the model thinks), one fence that wraps the whole answer removed
 * (```` ```markdown ```` or ```` ``` ````), the ends trimmed. The shell does the same to the `done` text
 * (`kb_ai::clean_output`); the preview of a stream in progress uses this.
 */
export function cleanOutput(raw: string): string {
  let text = raw.replace(/<think>[\s\S]*?<\/think>/giu, "");
  const open = text.search(/<think>/iu);
  if (open >= 0) {
    text = text.slice(0, open);
  }
  text = text.trim();
  const fenced = /^```[a-z]*[ \t]*\r?\n([\s\S]*?)\r?\n?```$/iu.exec(text);
  if (fenced !== null && !(fenced[1] ?? "").includes("```")) {
    text = (fenced[1] ?? "").trim();
  }
  return text;
}

/** One transcribed segment: when it starts (milliseconds into the recording) and what was said. */
export interface TranscriptSegment {
  startMs: number;
  text: string;
}

/** `mm:ss`, or `h:mm:ss` from one hour (`kb_ai::format_timestamp`). */
export function formatTimestamp(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  const two = (value: number): string => String(value).padStart(2, "0");
  return hours > 0 ? `${String(hours)}:${two(minutes)}:${two(rest)}` : `${two(minutes)}:${two(rest)}`;
}

/** The segments that hold speech (empty ones, silence, are left out). */
function spoken(segments: readonly TranscriptSegment[]): TranscriptSegment[] {
  return segments.filter((segment) => segment.text.trim() !== "");
}

/** The transcript as Markdown: one paragraph per segment, `**[mm:ss]** text` (`kb_ai::transcript_markdown`). */
export function transcriptMarkdown(segments: readonly TranscriptSegment[]): string {
  return spoken(segments)
    .map((segment) => `**[${formatTimestamp(segment.startMs)}]** ${segment.text.trim()}`)
    .join("\n\n");
}

/**
 * The transcript as portable paragraphs, a bold timestamp and the text. Built as nodes rather than
 * parsed from the Markdown above, so that what was said can never turn into formatting.
 */
export function transcriptBlocks(segments: readonly TranscriptSegment[]): JsonValue[] {
  return spoken(segments).map((segment): JsonValue => {
    const stamp: JsonValue = { type: "text", text: `[${formatTimestamp(segment.startMs)}]`, marks: [{ type: "bold" }] };
    const words: JsonValue = { type: "text", text: ` ${segment.text.trim()}` };
    return { type: "paragraph", content: [stamp, words] };
  });
}

/** What the summary is made from: the words, one paragraph per segment, without the timestamps. */
export function transcriptPlainText(segments: readonly TranscriptSegment[]): string {
  return spoken(segments)
    .map((segment) => segment.text.trim())
    .join("\n\n");
}

/** The most characters the shell takes as the text of one writing request. */
export const MAX_TEXT_CHARS = 32_000;
/** The most characters of context (the page title and what comes before). */
export const MAX_CONTEXT_CHARS = 8_000;
/** The most characters of a custom prompt, and of a language name. */
export const MAX_PROMPT_CHARS = 2_000;
export const MAX_LANGUAGE_CHARS = 60;
/** The longest transcript summarised in one request; longer ones go in parts. */
export const SUMMARY_PART_CHARS = 24_000;

/**
 * Splits `text` into parts of at most `max` characters, at paragraph boundaries (a paragraph longer
 * than `max` is cut at a space, or hard). One part when it fits.
 */
export function splitParts(text: string, max: number = SUMMARY_PART_CHARS): string[] {
  if (text.length <= max) {
    return [text];
  }
  const parts: string[] = [];
  let current = "";
  const flush = (): void => {
    if (current !== "") {
      parts.push(current);
      current = "";
    }
  };
  for (const paragraph of text.split(/\n{2,}/u)) {
    let rest = paragraph;
    while (rest.length > max) {
      flush();
      const space = rest.lastIndexOf(" ", max);
      const cut = space > max / 2 ? space : max;
      parts.push(rest.slice(0, cut).trim());
      rest = rest.slice(cut).trim();
    }
    if (rest === "") {
      continue;
    }
    if (current !== "" && current.length + 2 + rest.length > max) {
      flush();
    }
    current = current === "" ? rest : `${current}\n\n${rest}`;
  }
  flush();
  return parts;
}

/** `Recording YYYY-MM-DD HH.mm.webm`, from the local time `date`. */
export function recordingName(date: Date): string {
  const two = (value: number): string => String(value).padStart(2, "0");
  const day = `${String(date.getFullYear())}-${two(date.getMonth() + 1)}-${two(date.getDate())}`;
  return `${t("recorder.filePrefix")} ${day} ${two(date.getHours())}.${two(date.getMinutes())}.webm`;
}
