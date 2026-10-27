// The text side of `MemoryTransport`'s local AI (ADR-0037, `docs/architecture/ai.md` sections 3, 5 and 8.1):
// the prompts of `kb-ai` in TypeScript (for the `ollama` mode, where the browser calls Ollama itself), the
// cleanup of model output and transcripts, the canned answers of the `fake` mode, the chunking that makes the
// index plausible and the keyword scoring that stands in for the core's retrieval.
//
// Reduced fidelity: the real rules live in Rust (`kb-ai`, `kb-core`); this mirrors their shape closely enough
// for the UI and its tests. Pure: no I/O and no clock.

import type { AiAction } from "./generated/AiAction";
import type { AiChatInput } from "./generated/AiChatInput";
import type { AiGenerateInput } from "./generated/AiGenerateInput";
import type { AiSource } from "./generated/AiSource";
import type { PageId } from "./generated/PageId";
import type { SpaceId } from "./generated/SpaceId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { foldText } from "./search";

// ----- limits (section 10) -------------------------------------------------------------------------

export const MAX_WRITING_TEXT_CHARS = 32_000;
export const MAX_WRITING_CONTEXT_CHARS = 8_000;
export const MAX_CUSTOM_PROMPT_CHARS = 2_000;
export const MAX_CHAT_MESSAGES = 40;
export const MAX_CHAT_MESSAGE_CHARS = 16_000;
export const MAX_CHAT_TOTAL_CHARS = 64_000;
export const MAX_RETRIEVE_QUERY_CHARS = 2_000;
export const MAX_PASSAGE_CHARS = 1_500;
export const DEFAULT_RETRIEVE_LIMIT = 8;
export const MAX_RETRIEVE_LIMIT = 20;
export const MAX_PASSAGES_PER_PAGE = 3;

// ----- what the mirror knows of a page --------------------------------------------------------------

/** A block with text: its id and its indexed text. */
export interface AiBlockText {
  id: string;
  text: string;
}

/** A live page with its title and the text of its blocks, in document order. */
export interface AiPageText {
  pageId: PageId;
  spaceId: SpaceId;
  title: string;
  icon?: string;
  blocks: AiBlockText[];
}

function isObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The text of a node's direct inline children: text runs, page references (their title), breaks, dates, equations. */
function inlineTextOf(node: Readonly<Record<string, unknown>>, titleOf: (pageId: PageId) => string | undefined): string {
  const children = Array.isArray(node.content) ? (node.content as unknown[]) : [];
  let text = "";
  for (const child of children) {
    if (!isObject(child)) {
      continue;
    }
    const attrs = isObject(child.attrs) ? child.attrs : {};
    switch (child.type) {
      case "text":
        text += typeof child.text === "string" ? child.text : "";
        break;
      case "pageRef":
        text += typeof attrs.pageId === "string" ? (titleOf(attrs.pageId) ?? "") : "";
        break;
      case "hardBreak":
        text += " ";
        break;
      case "dateMention":
        text += [attrs.date, attrs.time].filter((part): part is string => typeof part === "string").join(" ");
        break;
      case "mathInline":
        text += typeof attrs.latex === "string" ? attrs.latex : "";
        break;
      default:
    }
  }
  return text;
}

/**
 * The blocks of a page's portable JSON with their text, in document order: every node that has an `id` attribute
 * contributes the text of its direct inline children (an image: its caption and alt text). A reduced version of
 * the text index the core builds (`extractBlocks`), without building a Loro document. The blocks inside a template button
 * (ADR-0056) are listed with no text: template material is not indexed.
 */
export function blocksOfPortable(content: JsonValue, titleOf: (pageId: PageId) => string | undefined): AiBlockText[] {
  const blocks: AiBlockText[] = [];
  const visit = (node: unknown, inButton: boolean): void => {
    if (!isObject(node)) {
      return;
    }
    const attrs = isObject(node.attrs) ? node.attrs : {};
    if (typeof attrs.id === "string") {
      let text = inButton ? "" : inlineTextOf(node, titleOf);
      if (node.type === "image" && !inButton) {
        text = [attrs.caption, attrs.alt].filter((part): part is string => typeof part === "string" && part !== "").join(" ");
      }
      blocks.push({ id: attrs.id, text });
    }
    if (Array.isArray(node.content)) {
      const insideButton = inButton || node.type === "templateButton";
      for (const child of node.content as unknown[]) {
        visit(child, insideButton);
      }
    }
  };
  visit(content, false);
  return blocks;
}

// ----- prompts (section 3.2) ------------------------------------------------------------------------

/** One message of an Ollama chat request. */
export interface OllamaMessage {
  role: "system" | "user" | "assistant";
  content: string;
  images?: string[];
}

/** The body of an `/api/chat` request, before the model and the options are added. */
export interface PromptRequest {
  messages: OllamaMessage[];
  temperature: number;
  /** Characters of the whole prompt, for the context size. */
  promptChars: number;
}

export const WRITING_SYSTEM_PROMPT =
  "You are a writing assistant inside a notes app. Reply with the result only, in Markdown, without preamble, without explanations, without code fences around the whole answer. Keep the language of the input unless asked to translate.";

export const CHAT_SYSTEM_PROMPT =
  "You answer questions about the user's notes. Use only the numbered sources below. Cite them inline as [1], [2]. If the sources do not contain the answer, say so plainly. Answer in the language of the question.";

export const TRANSCRIPTION_PROMPT =
  "Transcribe the speech in this audio clip verbatim. Output only the transcript.";

/** The instruction and temperature of each writing action (the table of section 3.2). */
function instructionOf(input: AiGenerateInput): { instruction: string; temperature: number } {
  const table: Record<AiAction, { instruction: string; temperature: number }> = {
    continue: {
      instruction: "Continue the text naturally for one or two paragraphs. Context precedes. Do not repeat it.",
      temperature: 0.7,
    },
    improve: {
      instruction: "Improve the writing: clarity, flow, word choice. Keep the meaning and the formatting.",
      temperature: 0.4,
    },
    fixGrammar: { instruction: "Fix spelling, grammar and punctuation only. Change nothing else.", temperature: 0.1 },
    shorter: { instruction: "Make it about half as long, keeping the key points.", temperature: 0.3 },
    longer: { instruction: "Expand it to about twice the length with relevant detail. Invent no facts.", temperature: 0.6 },
    simplify: { instruction: "Rewrite in plain, simple language.", temperature: 0.3 },
    tone: {
      instruction: `Rewrite in a ${input.tone ?? "professional"} tone (professional, casual, friendly, confident, direct).`,
      temperature: 0.4,
    },
    translate: { instruction: `Translate into ${input.language ?? "English"}.`, temperature: 0.2 },
    summarize: {
      instruction: "Summarise as a short paragraph followed by key points as a bulleted list.",
      temperature: 0.2,
    },
    explain: { instruction: "Explain what this means in simple terms.", temperature: 0.3 },
    actionItems: {
      instruction:
        'List the action items as a Markdown task list (`- [ ] ...`), with owners and dates when stated. Write "No action items." if none.',
      temperature: 0.1,
    },
    custom: { instruction: input.prompt ?? "", temperature: 0.6 },
    meetingSummary: {
      instruction:
        'From this meeting transcript write `### Summary` (2 to 4 sentences), `### Key points` (bullets), `### Action items` (a Markdown task list: one `- [ ] ` line per item, with the owner and the date when stated; write "None." when there are none).',
      temperature: 0.2,
    },
  };
  return table[input.action];
}

/** The chat request of a writing action: the system prompt, then the instruction with the context and the text. */
export function buildGenerate(input: AiGenerateInput): PromptRequest {
  const { instruction, temperature } = instructionOf(input);
  const parts = [instruction];
  if (input.context !== undefined && input.context !== "") {
    parts.push(`Context (the page title and the text before the selection):\n${input.context}`);
  }
  if (input.text !== "") {
    parts.push(`Text:\n${input.text}`);
  }
  const user = parts.join("\n\n");
  return {
    messages: [
      { role: "system", content: WRITING_SYSTEM_PROMPT },
      { role: "user", content: user },
    ],
    temperature,
    promptChars: WRITING_SYSTEM_PROMPT.length + user.length,
  };
}

/** The chat request of a question: the sources in the system prompt, then the conversation (the newest 40 messages). */
export function buildChat(input: AiChatInput, sources: readonly AiSource[]): PromptRequest {
  const list = sources.map((source) => `[${String(source.n)}] ${source.title}\n${source.text}`).join("\n\n");
  const system = list === "" ? CHAT_SYSTEM_PROMPT : `${CHAT_SYSTEM_PROMPT}\n\n${list}`;
  const conversation = input.messages.slice(-MAX_CHAT_MESSAGES);
  const messages: OllamaMessage[] = [
    { role: "system", content: system },
    ...conversation.map((message): OllamaMessage => ({ role: message.role, content: message.content })),
  ];
  return {
    messages,
    temperature: 0.3,
    promptChars: messages.reduce((sum, message) => sum + message.content.length, 0),
  };
}

/** The chat request of a transcription: the instruction and the WAV (base64) as the one image. */
export function buildTranscription(wavBase64: string): PromptRequest {
  return {
    messages: [{ role: "user", content: TRANSCRIPTION_PROMPT, images: [wavBase64] }],
    temperature: 0,
    promptChars: TRANSCRIPTION_PROMPT.length,
  };
}

/** The context size to ask for: about a third of a token per character plus 1,024, in steps of 1,024, from 4,096 to 16,384. */
export function contextSize(promptChars: number, max = 16_384): number {
  const wanted = Math.ceil((promptChars / 3 + 1024) / 1024) * 1024;
  return Math.max(4096, Math.min(wanted, Math.min(16_384, max)));
}

/**
 * Removes `<think>...</think>` blocks (an unfinished one too), one fence that wraps the whole answer
 * (```` ```markdown ```` or ```` ``` ````) and surrounding space.
 */
export function cleanOutput(text: string): string {
  let cleaned = text.replace(/<think>[\s\S]*?<\/think>/g, "").replace(/<think>[\s\S]*$/, "").trim();
  const fenced = /^```(?:markdown|md)?[ \t]*\r?\n([\s\S]*?)\r?\n```$/.exec(cleaned);
  if (fenced !== null && !(fenced[1] ?? "").includes("```")) {
    cleaned = (fenced[1] ?? "").trim();
  }
  return cleaned;
}

/** What a transcription answer says: the speech and the language, when the model reported one. */
export interface Transcribed {
  text: string;
  language?: string;
}

/**
 * Cleans a transcription answer (section 3.5): an ASR header `language <Name><asr_text>` is removed (and read for
 * the language), so is a closing `</asr_text>`, a leading "Transcript:" line, surrounding quotes and fences;
 * "[silence]", "(no speech)" and an empty answer are empty text.
 */
export function cleanTranscript(raw: string): Transcribed {
  let text = cleanOutput(raw);
  let language: string | undefined;
  const header = /^language\s+([^<\n]+?)\s*<asr_text>/i.exec(text);
  if (header !== null) {
    language = header[1]?.trim();
    text = text.slice(header[0].length);
  }
  text = text.replace(/<\/asr_text>\s*$/i, "").replace(/^<asr_text>/i, "");
  text = text.replace(/^(?:here is the transcript|transcript)\s*:\s*\n?/i, "").trim();
  const fenced = /^```[a-z]*\r?\n([\s\S]*?)\r?\n```$/.exec(text);
  if (fenced !== null) {
    text = (fenced[1] ?? "").trim();
  }
  if (/^["“](.*)["”]$/s.test(text)) {
    text = text.slice(1, -1).trim();
  }
  text = text.replace(/[ \t]{2,}/g, " ").trim();
  if (/^(?:\[silence\]|\(no speech\)|\[no speech\])\.?$/i.test(text)) {
    text = "";
  }
  return language === undefined ? { text } : { text, language };
}

// ----- the fake mode's canned text -----------------------------------------------------------------

function words(text: string): string[] {
  return text.split(/\s+/).filter((word) => word !== "");
}

/**
 * The fake mode's answer to a writing action, deterministic:
 *
 * - `improve`: the text in capitals; `fixGrammar`: spaces collapsed; `shorter`: the first half of the words;
 *   `longer`: the text twice; `simplify`: the text with "Simply put: " before it; `tone`: `[<tone>] <text>`;
 *   `translate`: `[<language>] <text>`; `summarize`: a sentence and two bullets; `explain`: "This means: <text>";
 *   `actionItems`: the lines as `- [ ] <line>`; `custom`: `<prompt>: <text>`; `continue`: the text then
 *   " ... and so on."; `meetingSummary`: the three sections with one line each.
 */
export function fakeGenerateText(input: AiGenerateInput): string {
  const text = input.text;
  switch (input.action) {
    case "improve":
      return text.toUpperCase();
    case "fixGrammar":
      return text.replace(/[ \t]{2,}/g, " ").trim();
    case "shorter": {
      const all = words(text);
      return all.slice(0, Math.max(1, Math.ceil(all.length / 2))).join(" ");
    }
    case "longer":
      return `${text}\n\n${text}`;
    case "simplify":
      return `Simply put: ${text}`;
    case "tone":
      return `[${input.tone ?? "professional"}] ${text}`;
    case "translate":
      return `[${input.language ?? "English"}] ${text}`;
    case "summarize":
      return `A short summary of the text.\n\n- Key point one\n- Key point two`;
    case "explain":
      return `This means: ${text}`;
    case "actionItems": {
      const lines = text.split(/\r?\n/).filter((line) => line.trim() !== "");
      return lines.length === 0 ? "No action items." : lines.map((line) => `- [ ] ${line.trim()}`).join("\n");
    }
    case "custom":
      return `${input.prompt ?? ""}: ${text}`.trim();
    case "continue":
      return `${text} ... and so on.`;
    case "meetingSummary":
      return "### Summary\nThe meeting covered the topics in the transcript.\n\n### Key points\n- A decision was made\n\n### Action items\n- [ ] Follow up on the decision";
  }
}

/**
 * The fake mode's answer to a question: the title of the first source cited as [1] (and the second as [2]), or
 * a plain "not found" when nothing was retrieved.
 */
export function fakeChatText(question: string, sources: readonly AiSource[]): string {
  const first = sources[0];
  if (first === undefined) {
    return `I could not find anything about "${question}" in your notes.`;
  }
  let text = `According to your notes, **${first.title}** covers "${question}" [1].`;
  const second = sources[1];
  if (second !== undefined) {
    text += ` See also ${second.title} [2].`;
  }
  return text;
}

/** `text` in at most `count` pieces cut after whitespace, so that every piece but the last ends in a space. */
export function splitDeltas(text: string, count: number): string[] {
  if (text === "") {
    return [];
  }
  const pieces = text.match(/\S+\s*|\s+/g) ?? [text];
  const per = Math.max(1, Math.ceil(pieces.length / Math.max(1, count)));
  const out: string[] = [];
  for (let i = 0; i < pieces.length; i += per) {
    out.push(pieces.slice(i, i + per).join(""));
  }
  return out;
}

// ----- chunks (section 3.3, reduced) ---------------------------------------------------------------

const CHUNK_CHARS = 1_000;
const LONG_BLOCK_CHARS = 1_500;
const MAX_CHUNKS_PER_PAGE = 400;
const MAX_TITLE_CHARS = 200;

/** One chunk of a page: the ids of its blocks and the text to embed (the title line, then the text). */
export interface AiChunk {
  blockIds: string[];
  text: string;
}

/** Splits a long block at sentence ends into pieces of at most 1,000 characters, or cuts it at 1,500. */
function splitBlock(text: string): string[] {
  if (text.length <= LONG_BLOCK_CHARS) {
    return [text];
  }
  const pieces: string[] = [];
  let rest = text;
  while (rest.length > LONG_BLOCK_CHARS) {
    const window = rest.slice(0, CHUNK_CHARS);
    const end = Math.max(window.lastIndexOf(". "), window.lastIndexOf("? "), window.lastIndexOf("! "), window.lastIndexOf("\n"));
    const cut = end > 0 ? end + 1 : LONG_BLOCK_CHARS;
    pieces.push(rest.slice(0, cut).trim());
    rest = rest.slice(cut).trim();
  }
  if (rest !== "") {
    pieces.push(rest);
  }
  return pieces;
}

/**
 * Chunks a page like `kb_ai::chunk_page`: empty blocks are skipped, consecutive blocks are joined with a newline
 * until the next would pass 1,000 characters, a long block is split; every chunk's text starts with the title;
 * a page without body text has one chunk, its title (no chunk when the title is empty too); at most 400 chunks.
 */
export function chunkPage(title: string, blocks: readonly AiBlockText[]): AiChunk[] {
  const head = title.slice(0, MAX_TITLE_CHARS);
  const chunks: AiChunk[] = [];
  let ids: string[] = [];
  let body = "";
  const close = (): void => {
    if (body !== "") {
      chunks.push({ blockIds: ids, text: `${head}\n${body}` });
    }
    ids = [];
    body = "";
  };
  for (const block of blocks) {
    const text = block.text.trim();
    if (text === "") {
      continue;
    }
    for (const piece of splitBlock(text)) {
      if (body !== "" && body.length + 1 + piece.length > CHUNK_CHARS) {
        close();
      }
      body = body === "" ? piece : `${body}\n${piece}`;
      ids.push(block.id);
    }
  }
  close();
  if (chunks.length === 0 && head !== "") {
    chunks.push({ blockIds: [], text: head });
  }
  return chunks.slice(0, MAX_CHUNKS_PER_PAGE);
}

// ----- keyword retrieval (section 5, reduced) --------------------------------------------------------

const TOKEN = /[\p{L}\p{N}\p{M}]+/gu;

/** The words of a query in any-word mode: folded, words of 2 or fewer characters dropped, no repeats. */
export function queryWords(query: string): string[] {
  const found = foldText(query).match(TOKEN) ?? [];
  return [...new Set(found.filter((word) => word.length > 2))];
}

/** How many of `wanted` (folded words) a text contains, a word matching a token it is a prefix of. */
function matches(text: string, wanted: readonly string[]): number {
  const tokens = foldText(text).match(TOKEN) ?? [];
  return wanted.filter((word) => tokens.some((token) => token.startsWith(word))).length;
}

/** A passage before it is numbered: the page and block it comes from, its text and its score. */
export interface Passage {
  page: AiPageText;
  blockId?: string;
  text: string;
  score: number;
  /** Position among the pages and the blocks, for a stable order of equals. */
  order: number;
}

/**
 * Passages of `pages` for a question: every block is scored by the share of the query's words it contains (the
 * title's words count a little for each of its blocks), at most 3 passages per page; a page whose title matches
 * and no block does gives its title alone. Best first, equals in document order.
 */
export function keywordPassages(pages: readonly AiPageText[], query: string): Passage[] {
  const wanted = queryWords(query);
  if (wanted.length === 0) {
    return [];
  }
  const found: Passage[] = [];
  let order = 0;
  for (const page of pages) {
    const titleShare = matches(page.title, wanted) / wanted.length;
    const scored: Passage[] = [];
    for (const block of page.blocks) {
      order += 1;
      const text = block.text.trim();
      if (text === "") {
        continue;
      }
      const share = matches(text, wanted) / wanted.length;
      if (share > 0) {
        scored.push({ page, blockId: block.id, text, score: share + titleShare * 0.25, order });
      }
    }
    scored.sort((a, b) => b.score - a.score || a.order - b.order);
    const best = scored.slice(0, MAX_PASSAGES_PER_PAGE);
    if (best.length === 0 && titleShare > 0) {
      order += 1;
      best.push({ page, text: page.title, score: titleShare, order });
    }
    found.push(...best);
  }
  return found.sort((a, b) => b.score - a.score || a.order - b.order);
}

/** Numbers passages from 1 as sources: the text cut at 1,500 characters, the score kept (only the order matters). */
export function toSources(passages: readonly Passage[], limit: number): AiSource[] {
  return passages.slice(0, limit).map((passage, index): AiSource => {
    const source: AiSource = {
      n: index + 1,
      pageId: passage.page.pageId,
      spaceId: passage.page.spaceId,
      title: passage.page.title,
      text: passage.text.slice(0, MAX_PASSAGE_CHARS),
      score: passage.score,
    };
    if (passage.blockId !== undefined) {
      source.blockId = passage.blockId;
    }
    if (passage.page.icon !== undefined) {
      source.icon = passage.page.icon;
    }
    return source;
  });
}

// ----- vectors (section 3.4) -----------------------------------------------------------------------

/** Scales a vector to length 1; a zero vector stays zero. */
export function normalized(vector: readonly number[]): Float32Array {
  const out = Float32Array.from(vector);
  let sum = 0;
  for (const x of out) {
    sum += x * x;
  }
  if (sum > 0) {
    const length = Math.sqrt(sum);
    for (let i = 0; i < out.length; i += 1) {
      out[i] = (out[i] ?? 0) / length;
    }
  }
  return out;
}

/** The dot product of two vectors of one length. */
export function dot(a: Float32Array, b: Float32Array): number {
  let sum = 0;
  for (let i = 0; i < a.length; i += 1) {
    sum += (a[i] ?? 0) * (b[i] ?? 0);
  }
  return sum;
}

/** Reciprocal rank fusion: `sum 1 / (k + rank + 1)` over the lists, best first, equals by first appearance. */
export function rrf(lists: readonly (readonly string[])[], k = 60): { key: string; score: number }[] {
  const scores = new Map<string, number>();
  for (const list of lists) {
    list.forEach((key, rank) => {
      scores.set(key, (scores.get(key) ?? 0) + 1 / (k + rank + 1));
    });
  }
  const ranked = [...scores].map(([key, score]) => ({ key, score }));
  // `Array.prototype.sort` is stable: equal scores stay in order of first appearance.
  return ranked.sort((a, b) => b.score - a.score);
}

/** The prefix a document or a query needs before it is embedded (section 3.3). */
export function embeddingInput(model: string, text: string, role: "document" | "query"): string {
  if (model.includes("nomic-embed")) {
    return `${role === "document" ? "search_document" : "search_query"}: ${text}`;
  }
  if (model.includes("qwen3-embedding") && role === "query") {
    return `Instruct: Given a question, retrieve passages of the user's notes that answer it\nQuery: ${text}`;
  }
  return text;
}
