// The in-memory twin of the local AI (ADR-0037, `docs/architecture/ai.md`): the core's semantic index and
// retrieval (`AiIndexStatus`, `AiIndexClear`, `AiRetrieve`) and, when `MemoryTransportOptions.ai` is given, the
// shell's AI commands (`status`, `endpointSet`, `generate`, `chat`, `cancel`, `search`, `transcribe`) and the MCP
// status, so the UI and its tests run without Ollama.
//
// Two modes (section 8.1):
//
// - `fake`: deterministic answers streamed in a few deltas (see `fakeGenerateText` and `fakeChatText`), a
//   made-up list of models and an index that "embeds" nothing (it counts chunks);
// - `ollama`: the browser calls a real Ollama (`./memory-ai-ollama`) for the status, writing, chat, embeddings
//   and transcription. Ollama's default `OLLAMA_ORIGINS` allows `http://localhost:*`, so the dev page works.
//
// Retrieval is keyword scoring in memory in both (plus the semantic part when the `ollama` mode embedded the
// pages). The rules of the settings (AI off refuses everything but the status and the endpoint, the endpoint's
// consent, the limits of section 10, at most four streams) are the shell's.

import {
  AI_ENDPOINT_SETTING,
  AI_SETTINGS_SETTING,
  DEFAULT_AI_ENDPOINT,
  MCP_SETTINGS_SETTING,
  aiSettingsOf,
  isLocalHost,
  mcpSettingsOf,
  normaliseEndpoint,
} from "./ai";
import { toBase64 } from "./backup";
import { CoreError } from "./errors";
import type { AiChatInput } from "./generated/AiChatInput";
import type { AiGenerateInput } from "./generated/AiGenerateInput";
import type { AiIndexInfo } from "./generated/AiIndexInfo";
import type { AiIndexerState } from "./generated/AiIndexerState";
import type { AiModelInfo } from "./generated/AiModelInfo";
import type { AiScope } from "./generated/AiScope";
import type { AiSettings } from "./generated/AiSettings";
import type { AiSource } from "./generated/AiSource";
import type { AiStatus } from "./generated/AiStatus";
import type { AiStreamEvent } from "./generated/AiStreamEvent";
import type { AiTranscribed } from "./generated/AiTranscribed";
import type { ApiError } from "./generated/ApiError";
import type { Event } from "./generated/Event";
import type { McpStatus } from "./generated/McpStatus";
import type { PageId } from "./generated/PageId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import {
  DEFAULT_RETRIEVE_LIMIT,
  MAX_CHAT_MESSAGES,
  MAX_CHAT_MESSAGE_CHARS,
  MAX_CHAT_TOTAL_CHARS,
  MAX_CUSTOM_PROMPT_CHARS,
  MAX_PASSAGES_PER_PAGE,
  MAX_PASSAGE_CHARS,
  MAX_RETRIEVE_LIMIT,
  MAX_RETRIEVE_QUERY_CHARS,
  MAX_WRITING_CONTEXT_CHARS,
  MAX_WRITING_TEXT_CHARS,
  buildChat,
  buildGenerate,
  buildTranscription,
  chunkPage,
  cleanOutput,
  cleanTranscript,
  contextSize,
  dot,
  embeddingInput,
  fakeChatText,
  fakeGenerateText,
  keywordPassages,
  normalized,
  rrf,
  splitDeltas,
  toSources,
} from "./memory-ai-text";
import type { AiChunk, AiPageText, Passage, PromptRequest } from "./memory-ai-text";
import { OllamaClient, endpointError, endpointFailure } from "./memory-ai-ollama";
import type { FetchLike, OllamaChatRequest } from "./memory-ai-ollama";
import type { AiTransport } from "./transport";
import { invalid } from "./validate";

/** Streams running at once (section 6.2). */
const MAX_STREAMS = 4;
/** Largest WAV `transcribe` takes (section 10). */
const MAX_WAV_BYTES = 4 * 1024 * 1024;
/** A page shorter than this goes whole into a page-scoped question (section 6.2). */
const WHOLE_PAGE_CHARS = 12_000;
/** The smallest cosine that counts as a semantic match (section 5). */
const MIN_SIMILARITY = 0.25;
/** Passages the semantic and the keyword part each contribute before fusion (section 5). */
const PER_LIST = 40;
/** Chunks per `/api/embed` call (section 10). */
const EMBED_BATCH = 16;
/** Dimension range of a vector (section 10). */
const MIN_DIMENSIONS = 16;
const MAX_DIMENSIONS = 8192;
/** What the fake mode reports for the vector dimension. */
const FAKE_DIMENSIONS = 768;
/** Default pages per batch of the timer that indexes (see `indexStepMs`). */
const DEFAULT_INDEX_PAGES = 2;

/** The models the `fake` mode lists unless `models` says otherwise. */
export const FAKE_MODELS: readonly AiModelInfo[] = [
  { name: "qwen3:8b", family: "qwen3", parameterSize: "8.2B", capabilities: ["completion", "thinking", "tools"], size: 5_225_000_000 },
  { name: "llama3.2:3b", family: "llama", parameterSize: "3.2B", capabilities: ["completion", "tools"], size: 2_019_000_000 },
  { name: "gemma4:12b", family: "gemma4", parameterSize: "12B", capabilities: ["completion", "vision", "audio", "thinking"], size: 8_100_000_000 },
  { name: "nomic-embed-text:latest", family: "nomic-bert", parameterSize: "137M", capabilities: ["embedding"], size: 274_000_000 },
  { name: "r2t2:latest", family: "r2t2", parameterSize: "2B", capabilities: ["completion", "audio"], size: 1_600_000_000 },
];

/** What the `fake` and `ollama` modes have in common that a test or the dev page may tune. */
export interface MemoryAiTuning {
  /** `ollama`: replaces the browser's `fetch` (tests give a stub server). */
  fetch?: FetchLike;
  /** `fake`: the installed models (default {@link FAKE_MODELS}). */
  models?: readonly AiModelInfo[];
  /** `fake`: Ollama's version (default `0.35.0`). */
  version?: string;
  /** `fake`: wait this long before each delta, in milliseconds (default none). */
  deltaDelayMs?: number;
  /**
   * Index `indexBatchPages` pages every this many milliseconds while AI and the semantic index are on and pages
   * wait (the dev page uses it to show the progress). Default: never; `advanceIndex` indexes by hand.
   */
  indexStepMs?: number;
  /** Pages per timer step (default 2). */
  indexBatchPages?: number;
  /** What `mcp_status` reports as the bridge's path (default none). */
  bridgePath?: string;
}

/** `MemoryTransportOptions.ai`: which AI the mirror answers with. */
export type MemoryAiOptions = ({ mode: "fake" } | { mode: "ollama"; baseUrl: string }) & MemoryAiTuning;

/** What {@link MemoryAi} needs from its owner. */
export interface AiDeps {
  now: () => number;
  emit: (event: Event) => void;
  /** A stored setting's value; `undefined` when it is not set. */
  setting: (key: string) => JsonValue | undefined;
  /** Writes a setting only the shell writes and announces it. */
  shellSetting: (key: string, value: JsonValue) => void;
  /** The live pages of a scope with their text (`NotFound` for an unknown space or page). */
  pages: (scope: AiScope) => AiPageText[];
  /** The ids of every live page. */
  livePageIds: () => PageId[];
  /** Throws what a call answers while the core is not ready (`Locked`, starting, failed). */
  ready: () => void;
}

/** One call a test may look at: what asked, with which input. */
export interface AiRecordedCall {
  kind: "generate" | "chat" | "search" | "transcribe" | "endpointSet" | "status" | "retrieve";
  /** The input of the call (the requests, the query, or the URL). */
  input: unknown;
}

/** The shell's AI commands plus the hooks of the mirror. */
export interface MemoryAiTransport extends AiTransport {
  /** Every `generate`, `chat`, `search`, `transcribe`, `endpointSet`, `status` and `AiRetrieve`, in order. */
  readonly calls: readonly AiRecordedCall[];
  /** Replaces the models the `fake` mode lists (and so what the status says is installed). */
  setModels(models: readonly AiModelInfo[]): void;
  /** Makes the status say Ollama cannot be reached (with `message`), or reachable again. */
  setReachable(reachable: boolean, message?: string): void;
  /**
   * Streams started from now on wait, after `afterDeltas` deltas (default 1), until the returned function is
   * called or the stream is cancelled; `Stop` can be tried in the middle of an answer.
   */
  holdStreams(afterDeltas?: number): () => void;
  /** Makes the next `times` calls (any command but `cancel`) reject with `error`. */
  failNext(error: ApiError, times?: number): void;
  /** Makes the next stream end with an `error` event with `error` after its first delta. */
  failNextStream(error: ApiError): void;
  /** Makes the native consent dialog of a remote endpoint answer "Cancel" (`true`) or "Allow" (`false`). */
  setDeclineConsent(decline: boolean): void;
  /** Replaces the answer of `transcribe` (default: a sentence naming the clip's length). */
  setTranscriber(transcriber: (wav: Uint8Array, model: string | undefined) => AiTranscribed | string): void;
  /**
   * Indexes up to `count` (default all) pages that wait, as the shell's indexer would, and answers how many
   * were indexed (`AiIndexChanged` follows). Nothing happens while AI, the semantic index or the embedding
   * model is off.
   */
  advanceIndex(count?: number): Promise<number>;
  /** Sets what `mcp_status` reports (the mirror has no server: by default it is not running). */
  setMcpStatus(status: Partial<Pick<McpStatus, "running" | "error" | "bridgePath">>): void;
}

interface Stream {
  cancelled: boolean;
  readonly abort: AbortController;
  /** Called when the stream is cancelled or a hold ends. */
  wake: (() => void) | undefined;
  /** The failure to end with after the first delta (`failNextStream`). */
  failure: ApiError | undefined;
}

interface Hold {
  after: number;
  released: boolean;
  release: () => void;
  promise: Promise<void>;
}

/** The vectors of one page's chunks. */
interface PageVectors {
  chunks: AiChunk[];
  vectors: Float32Array[];
}

interface Endpoint {
  url: string;
  remoteConsent: boolean;
}

const REQUEST_ID = /^[A-Za-z0-9_-]{1,64}$/;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

const CANCELLED = endpointError("cancelled", "the request was cancelled");

/**
 * The AI of the mirror: the core's index and retrieval, and the shell's commands when an `ai` option exists.
 * See the file's comment. Methods of {@link MemoryAiTransport} work only with the option.
 */
export class MemoryAi implements MemoryAiTransport {
  readonly #deps: AiDeps;
  readonly #options: MemoryAiOptions | undefined;
  readonly #fetch: FetchLike;
  readonly #calls: AiRecordedCall[] = [];
  readonly #streams = new Map<string, Stream>();
  readonly #injected: ApiError[] = [];
  readonly #indexed = new Map<PageId, { chunks: number; vectors?: PageVectors }>();
  #endpoint: Endpoint;
  #client: OllamaClient | undefined;
  #models: AiModelInfo[];
  #reachable = true;
  #unreachable = "Ollama could not be reached";
  #hold: Hold | undefined;
  #nextStreamFailure: ApiError | undefined;
  #declineConsent = false;
  #transcriber: ((wav: Uint8Array, model: string | undefined) => AiTranscribed | string) | undefined;
  #mcpStatus: Partial<Pick<McpStatus, "running" | "error" | "bridgePath">> = {};
  #indexModel: string | undefined;
  #dimensions: number | undefined;
  #indexTimer: ReturnType<typeof setTimeout> | undefined;
  #indexing = false;
  #indexError: string | undefined;
  #lastRunAt: number | undefined;
  #stopped = false;

  constructor(options: MemoryAiOptions | undefined, deps: AiDeps) {
    this.#deps = deps;
    this.#options = options;
    this.#fetch = options?.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.#models = [...(options?.models ?? FAKE_MODELS)];
    const stored = deps.setting(AI_ENDPOINT_SETTING);
    const first = options?.mode === "ollama" ? options.baseUrl : DEFAULT_AI_ENDPOINT;
    this.#endpoint = storedEndpoint(stored) ?? { url: normaliseEndpoint(first).url, remoteConsent: false };
    if (options?.mode === "ollama") {
      this.#client = new OllamaClient(this.#endpoint.url, this.#fetch);
      // The page that opened the dev page asked for this endpoint: it is the person's own choice.
      if (!isLocalHost(new URL(this.#endpoint.url).hostname)) {
        this.#endpoint.remoteConsent = true;
      }
      this.#models = [];
    }
    this.#scheduleIndex();
  }

  // ----- the core's side: index and retrieval ----------------------------------------------------------

  /** `AiIndexStatus`: what the semantic index holds (see the file's comment for how it fills). */
  indexInfo(): AiIndexInfo {
    const live = new Set(this.#deps.livePageIds());
    let indexed = 0;
    let chunks = 0;
    for (const [pageId, entry] of this.#indexed) {
      if (live.has(pageId)) {
        indexed += 1;
        chunks += entry.chunks;
      }
    }
    const info: AiIndexInfo = { pagesIndexed: indexed, pagesTotal: live.size, chunks, dirty: live.size - indexed };
    if (indexed > 0 && this.#indexModel !== undefined) {
      info.model = this.#indexModel;
      info.dimensions = this.#dimensions ?? FAKE_DIMENSIONS;
    }
    return info;
  }

  /** `AiIndexClear`: forgets every vector; the indexer (the timer, or `advanceIndex`) starts over. */
  indexClear(): void {
    this.#indexed.clear();
    this.#indexModel = undefined;
    this.#dimensions = undefined;
    this.#deps.emit({ type: "AiIndexChanged" });
    this.#scheduleIndex();
  }

  /**
   * `AiRetrieve`: passages for a question over a scope, best first, numbered from 1. Keyword scoring in any-word
   * mode, fused with the cosine of `vector` against the stored chunks when there is one and the pages were
   * embedded. `InvalidInput` for `query` (1 to 2,000 characters), `vector` (16 to 8,192 numbers) and `limit`
   * (clamped to 1 to 20, default 8); `NotFound` for an unknown space or page.
   */
  retrieve(query: string, scope: AiScope, options: { vector?: readonly number[]; limit?: number } = {}): AiSource[] {
    this.#calls.push({ kind: "retrieve", input: { query, scope } });
    const text = query.trim();
    if (text === "" || Array.from(text).length > MAX_RETRIEVE_QUERY_CHARS) {
      throw invalid("query", `must be 1 to ${String(MAX_RETRIEVE_QUERY_CHARS)} characters`);
    }
    const vector = options.vector;
    if (vector !== undefined && (vector.length < MIN_DIMENSIONS || vector.length > MAX_DIMENSIONS)) {
      throw invalid("vector", `must have ${String(MIN_DIMENSIONS)} to ${String(MAX_DIMENSIONS)} numbers`);
    }
    const limit = Math.min(MAX_RETRIEVE_LIMIT, Math.max(1, options.limit ?? DEFAULT_RETRIEVE_LIMIT));
    const pages = this.#deps.pages(scope);
    const keyword = keywordPassages(pages, text);
    if (vector === undefined || this.#indexed.size === 0) {
      return toSources(perPage(keyword), limit);
    }
    const query32 = normalized(vector);
    // The semantic part: chunks of the scope's pages whose vectors have the query's dimension.
    interface Hit {
      page: AiPageText;
      chunk: AiChunk;
      seq: number;
      score: number;
    }
    const semantic: Hit[] = [];
    for (const page of pages) {
      const stored = this.#indexed.get(page.pageId)?.vectors;
      stored?.vectors.forEach((chunkVector, seq) => {
        const chunk = stored.chunks[seq];
        if (chunk !== undefined && chunkVector.length === query32.length) {
          const score = dot(query32, chunkVector);
          if (score >= MIN_SIMILARITY) {
            semantic.push({ page, chunk, seq, score });
          }
        }
      });
    }
    semantic.sort((a, b) => b.score - a.score);
    const topSemantic = semantic.slice(0, PER_LIST);
    const keyOfChunk = (page: AiPageText, seq: number): string => `${page.pageId}:c${String(seq)}`;
    const keyOfPassage = (passage: Passage): string => {
      const chunks = this.#indexed.get(passage.page.pageId)?.vectors?.chunks ?? [];
      const seq = passage.blockId === undefined ? -1 : chunks.findIndex((chunk) => chunk.blockIds.includes(passage.blockId ?? ""));
      return seq >= 0 ? keyOfChunk(passage.page, seq) : `${passage.page.pageId}:b${passage.blockId ?? "title"}`;
    };
    const byKey = new Map<string, Passage>();
    const keywordKeys: string[] = [];
    for (const passage of keyword.slice(0, PER_LIST)) {
      const key = keyOfPassage(passage);
      if (!byKey.has(key)) {
        byKey.set(key, passage);
        keywordKeys.push(key);
      }
    }
    const semanticKeys: string[] = [];
    topSemantic.forEach((hit, order) => {
      const key = keyOfChunk(hit.page, hit.seq);
      semanticKeys.push(key);
      if (!byKey.has(key)) {
        const body = hit.chunk.text.slice(hit.chunk.text.indexOf("\n") + 1);
        byKey.set(key, { page: hit.page, blockId: hit.chunk.blockIds[0], text: body, score: hit.score, order: 1e9 + order });
      }
    });
    const fused = rrf([semanticKeys, keywordKeys]);
    const passages: Passage[] = [];
    for (const { key, score } of fused) {
      const passage = byKey.get(key);
      if (passage !== undefined) {
        passages.push({ ...passage, score });
      }
    }
    return toSources(perPage(passages), limit);
  }

  // ----- the shell's side: status and endpoint ---------------------------------------------------------

  /** `ai_status`. */
  async status(): Promise<AiStatus> {
    this.#enter({ kind: "status", input: null });
    return this.#statusNow();
  }

  async #statusNow(): Promise<AiStatus> {
    const { url, remoteConsent } = this.#endpoint;
    const local = isLocalHost(new URL(url).hostname);
    const status: AiStatus = {
      endpoint: url,
      local,
      remoteConsent,
      reachable: false,
      models: [],
      index: this.indexInfo(),
      indexer: this.#indexerState(),
    };
    if (!local && !remoteConsent) {
      status.error = endpointError("consent", "consent required");
      return status;
    }
    if (this.#options?.mode === "ollama" && this.#client !== undefined) {
      try {
        status.version = await this.#client.version();
        status.models = await this.#client.models();
        this.#models = [...status.models];
        status.reachable = true;
      } catch (error) {
        status.error = error instanceof CoreError ? error.toApiError() : endpointError("unreachable", this.#unreachable);
      }
      return status;
    }
    if (!this.#reachable) {
      status.error = endpointError("unreachable", this.#unreachable);
      return status;
    }
    status.reachable = true;
    status.version = this.#options?.version ?? "0.35.0";
    status.models = this.#models.map((model) => ({ ...model, capabilities: [...model.capabilities] }));
    return status;
  }

  /** `ai_endpoint_set`: stores the endpoint; a remote host is "allowed" in the native dialog unless declined. */
  async endpointSet(raw: string): Promise<AiStatus> {
    this.#enter({ kind: "endpointSet", input: raw });
    const { url, local } = normaliseEndpoint(raw);
    if (!local && this.#declineConsent) {
      throw new CoreError({ code: "InvalidInput", message: "the endpoint was not allowed", details: { field: "url", reason: "declined" } });
    }
    this.#endpoint = { url, remoteConsent: !local };
    this.#deps.shellSetting(AI_ENDPOINT_SETTING, { url, remoteConsent: !local });
    if (this.#options?.mode === "ollama") {
      this.#client = new OllamaClient(url, this.#fetch);
    }
    return this.#statusNow();
  }

  // ----- the shell's side: streams --------------------------------------------------------------------

  /** `ai_generate`. */
  async generate(requestId: string, input: AiGenerateInput, onEvent: (event: AiStreamEvent) => void): Promise<void> {
    this.#enter({ kind: "generate", input });
    const settings = this.#enabled();
    validateRequestId(requestId);
    validateGenerate(input);
    const model = chatModelOf(settings);
    this.#consent();
    const stream = this.#open(requestId);
    try {
      if (this.#options?.mode === "ollama") {
        await this.#ollamaStream(stream, buildGenerate(input), model, settings, onEvent);
      } else {
        await this.#fakeStream(stream, fakeGenerateText(input), model, onEvent);
      }
    } finally {
      this.#streams.delete(requestId);
    }
  }

  /** `ai_chat`: the sources first, then the answer. */
  async chat(requestId: string, input: AiChatInput, onEvent: (event: AiStreamEvent) => void): Promise<void> {
    this.#enter({ kind: "chat", input });
    const settings = this.#enabled();
    validateRequestId(requestId);
    validateChat(input);
    const model = chatModelOf(settings);
    this.#consent();
    const stream = this.#open(requestId);
    try {
      const users = input.messages.filter((message) => message.role === "user");
      const question = users[users.length - 1]?.content ?? "";
      const previous = users[users.length - 2]?.content;
      const asked = (previous === undefined ? question : `${previous}\n${question}`).slice(-MAX_RETRIEVE_QUERY_CHARS);
      let sources: AiSource[];
      try {
        sources = await this.#sourcesFor(asked, input.scope, settings);
      } catch (error) {
        if (error instanceof CoreError && error.code === "NotFound") {
          onEvent({ type: "error", error: error.toApiError() });
          return;
        }
        throw error;
      }
      onEvent({ type: "sources", sources });
      if (this.#options?.mode === "ollama") {
        await this.#ollamaStream(stream, buildChat(input, sources), model, settings, onEvent);
      } else {
        await this.#fakeStream(stream, fakeChatText(question.trim(), sources), model, onEvent);
      }
    } finally {
      this.#streams.delete(requestId);
    }
  }

  /** `ai_cancel`: the stream's last event is `error` with reason `cancelled`; an unknown id is fine. */
  cancel(requestId: string): Promise<void> {
    this.#deps.ready();
    const stream = this.#streams.get(requestId);
    if (stream !== undefined) {
      stream.cancelled = true;
      stream.abort.abort();
      stream.wake?.();
    }
    return Promise.resolve();
  }

  /** `ai_search`: related passages for the search dialog (semantic when the pages were embedded, else keyword). */
  async search(query: string, scope: AiScope, limit?: number): Promise<AiSource[]> {
    this.#enter({ kind: "search", input: { query, scope, limit } });
    const settings = this.#enabled();
    this.#consent();
    let vector: number[] | undefined;
    if (this.#options?.mode === "ollama" && this.#client !== undefined && this.#semanticOn(settings) && this.#indexed.size > 0) {
      try {
        const model = settings.embeddingModel ?? "";
        vector = (await this.#client.embed(model, [embeddingInput(model, query.trim(), "query")]))[0];
      } catch {
        vector = undefined; // keyword only, as the shell does when embedding fails
      }
    }
    return this.retrieve(query, scope, { ...(vector === undefined ? {} : { vector }), ...(limit === undefined ? {} : { limit }) });
  }

  /** `ai_transcribe`. */
  async transcribe(wav: Uint8Array, model?: string): Promise<AiTranscribed> {
    this.#enter({ kind: "transcribe", input: { bytes: wav.length, model } });
    const settings = this.#enabled();
    if (!isWav(wav)) {
      throw invalid("audio", "must be a RIFF/WAVE PCM file");
    }
    if (wav.length > MAX_WAV_BYTES) {
      throw new CoreError({ code: "InvalidInput", message: "the audio is too large", details: { field: "audio", limit: MAX_WAV_BYTES } });
    }
    const chosen = model ?? settings.transcriptionModel;
    if (chosen === undefined || chosen === "") {
      throw new CoreError({ code: "InvalidInput", message: "no transcription model is chosen", details: { field: "model", reason: "missing" } });
    }
    this.#consent();
    if (this.#options?.mode === "ollama" && this.#client !== undefined) {
      const prompt = buildTranscription(toBase64(wav));
      const answer = await this.#client.chatOnce(chatRequest(prompt, chosen, undefined), undefined);
      return cleanTranscript(answer);
    }
    await Promise.resolve();
    const made = this.#transcriber?.(wav, model) ?? `Transcript of a ${wavSeconds(wav).toFixed(1)} second clip.`;
    return typeof made === "string" ? { text: made, language: "English" } : made;
  }

  // ----- MCP status ------------------------------------------------------------------------------------

  /** `mcp_status`: not running (the mirror has no server) unless a test says otherwise. */
  mcpStatus(): McpStatus {
    const settings = mcpSettingsOf(this.#deps.setting(MCP_SETTINGS_SETTING));
    const status: McpStatus = {
      enabled: settings.enabled,
      running: this.#mcpStatus.running ?? false,
      port: settings.port,
      url: `http://127.0.0.1:${String(settings.port)}/mcp`,
    };
    const bridgePath = this.#mcpStatus.bridgePath ?? this.#options?.bridgePath;
    if (bridgePath !== undefined) {
      status.bridgePath = bridgePath;
    }
    if (this.#mcpStatus.error !== undefined) {
      status.error = this.#mcpStatus.error;
    }
    return status;
  }

  // ----- hooks of the mirror ---------------------------------------------------------------------------

  get calls(): readonly AiRecordedCall[] {
    return this.#calls;
  }

  setModels(models: readonly AiModelInfo[]): void {
    this.#models = models.map((model) => ({ ...model, capabilities: [...model.capabilities] }));
  }

  setReachable(reachable: boolean, message = "Ollama could not be reached"): void {
    this.#reachable = reachable;
    this.#unreachable = message;
  }

  holdStreams(afterDeltas = 1): () => void {
    let release: () => void = () => undefined;
    const promise = new Promise<void>((resolve) => {
      release = resolve;
    });
    const hold: Hold = { after: afterDeltas, released: false, release, promise };
    this.#hold = hold;
    return () => {
      hold.released = true;
      if (this.#hold === hold) {
        this.#hold = undefined;
      }
      hold.release();
    };
  }

  failNext(error: ApiError, times = 1): void {
    for (let i = 0; i < times; i += 1) {
      this.#injected.push({ ...error });
    }
  }

  failNextStream(error: ApiError): void {
    this.#nextStreamFailure = { ...error };
  }

  setDeclineConsent(decline: boolean): void {
    this.#declineConsent = decline;
  }

  setTranscriber(transcriber: (wav: Uint8Array, model: string | undefined) => AiTranscribed | string): void {
    this.#transcriber = transcriber;
  }

  setMcpStatus(status: Partial<Pick<McpStatus, "running" | "error" | "bridgePath">>): void {
    this.#mcpStatus = { ...this.#mcpStatus, ...status };
  }

  /** The semantic index is on: AI, the index and an embedding model are set. */
  #semanticOn(settings: AiSettings): boolean {
    return settings.enabled && settings.semanticIndex && settings.embeddingModel !== undefined && settings.embeddingModel !== "";
  }

  async advanceIndex(count = Number.POSITIVE_INFINITY): Promise<number> {
    const settings = aiSettingsOf(this.#deps.setting(AI_SETTINGS_SETTING));
    if (!this.#semanticOn(settings) || this.#stopped || this.#indexing) {
      return 0;
    }
    const model = settings.embeddingModel ?? "";
    if (this.#indexModel !== undefined && this.#indexModel !== model) {
      this.#indexed.clear();
    }
    const live = new Set(this.#deps.livePageIds());
    for (const pageId of [...this.#indexed.keys()]) {
      if (!live.has(pageId)) {
        this.#indexed.delete(pageId);
      }
    }
    const waiting = [...live].filter((pageId) => !this.#indexed.has(pageId)).slice(0, Math.max(0, count));
    if (waiting.length === 0) {
      return 0;
    }
    const pages = new Map(this.#deps.pages({ type: "all" }).map((page) => [page.pageId, page]));
    this.#indexing = true;
    let done = 0;
    try {
      for (const pageId of waiting) {
        const page = pages.get(pageId);
        if (page === undefined) {
          continue;
        }
        const chunks = chunkPage(page.title, page.blocks);
        const entry: { chunks: number; vectors?: PageVectors } = { chunks: chunks.length };
        if (this.#options?.mode === "ollama" && this.#client !== undefined && chunks.length > 0) {
          const vectors: Float32Array[] = [];
          for (let from = 0; from < chunks.length; from += EMBED_BATCH) {
            const batch = chunks.slice(from, from + EMBED_BATCH);
            const embedded = await this.#client.embed(model, batch.map((chunk) => embeddingInput(model, chunk.text, "document")));
            for (const vector of embedded) {
              if (vector.length < MIN_DIMENSIONS || vector.length > MAX_DIMENSIONS) {
                throw endpointFailure("remote", "Ollama sent an embedding of an unusable size");
              }
              vectors.push(normalized(vector));
            }
          }
          entry.vectors = { chunks, vectors };
          this.#dimensions = vectors[0]?.length ?? this.#dimensions;
        }
        // A page edited while it was embedded stays dirty: it is not recorded as done.
        this.#indexed.set(pageId, entry);
        this.#indexModel = model;
        done += 1;
      }
      this.#indexError = undefined;
    } catch (error) {
      this.#indexError = error instanceof CoreError ? error.message : "indexing failed";
    } finally {
      this.#indexing = false;
      if (done > 0) {
        this.#lastRunAt = this.#deps.now();
        this.#deps.emit({ type: "AiIndexChanged" });
      }
    }
    return done;
  }

  // ----- what the transport tells the mirror ------------------------------------------------------------

  /** Called for every event the transport emits: pages that changed wait for the index again; settings are reconciled. */
  onEvent(event: Event): void {
    switch (event.type) {
      case "PageContentChanged":
        if (this.#indexed.delete(event.data.pageId)) {
          this.#deps.emit({ type: "AiIndexChanged" });
        }
        this.#scheduleIndex();
        break;
      case "StructureChanged":
        this.#scheduleIndex();
        break;
      case "SettingChanged":
        if (event.data.key === AI_SETTINGS_SETTING) {
          this.#settingsChanged();
        }
        break;
      default:
    }
  }

  #settingsChanged(): void {
    const settings = aiSettingsOf(this.#deps.setting(AI_SETTINGS_SETTING));
    if (this.#indexModel !== undefined && settings.embeddingModel !== this.#indexModel) {
      // The index belongs to its model: a new one rebuilds it (the core clears it at the next work).
      this.#indexed.clear();
      this.#indexModel = undefined;
      this.#deps.emit({ type: "AiIndexChanged" });
    }
    this.#scheduleIndex();
  }

  /** The core locked: streams end, the timer stops. */
  lock(): void {
    this.#stopped = true;
    if (this.#indexTimer !== undefined) {
      clearTimeout(this.#indexTimer);
      this.#indexTimer = undefined;
    }
    for (const stream of this.#streams.values()) {
      stream.cancelled = true;
      stream.abort.abort();
      stream.wake?.();
    }
  }

  /** The core unlocked again. */
  unlock(): void {
    this.#stopped = false;
    this.#scheduleIndex();
  }

  // ----- private ---------------------------------------------------------------------------------------

  /** The start of every command of the shell: the core must be ready, an injected failure fires, the call is noted. */
  #enter(call: AiRecordedCall): void {
    this.#deps.ready();
    this.#calls.push(call);
    const failure = this.#injected.shift();
    if (failure !== undefined) {
      throw new CoreError(failure);
    }
  }

  #enabled(): AiSettings {
    const settings = aiSettingsOf(this.#deps.setting(AI_SETTINGS_SETTING));
    if (!settings.enabled) {
      throw invalid("enabled", "AI is turned off");
    }
    return settings;
  }

  /** Every call checks `remoteConsent || is_local(url)` (section 2). */
  #consent(): void {
    if (!this.#endpoint.remoteConsent && !isLocalHost(new URL(this.#endpoint.url).hostname)) {
      throw invalid("endpoint", "consent required");
    }
  }

  #open(requestId: string): Stream {
    if (this.#streams.has(requestId)) {
      throw invalid("requestId", "is already in use");
    }
    if (this.#streams.size >= MAX_STREAMS) {
      throw new CoreError({ code: "InvalidInput", message: "too many requests at once", details: { field: "requestId", reason: "busy" } });
    }
    const stream: Stream = { cancelled: false, abort: new AbortController(), wake: undefined, failure: this.#nextStreamFailure };
    this.#nextStreamFailure = undefined;
    this.#streams.set(requestId, stream);
    return stream;
  }

  /** Waits for the next delta: the configured delay, a hold, a cancel. `false` when the stream was cancelled. */
  async #pause(stream: Stream, delivered: number): Promise<boolean> {
    const delay = this.#options?.deltaDelayMs ?? 0;
    if (delay > 0) {
      await sleep(delay);
    } else {
      await Promise.resolve();
    }
    const hold = this.#hold;
    if (hold !== undefined && !hold.released && delivered >= hold.after && !stream.cancelled) {
      await new Promise<void>((resolve) => {
        stream.wake = resolve;
        void hold.promise.then(resolve);
      });
      stream.wake = undefined;
    }
    return !stream.cancelled;
  }

  async #fakeStream(stream: Stream, text: string, model: string, onEvent: (event: AiStreamEvent) => void): Promise<void> {
    const deltas = splitDeltas(text, 4);
    let delivered = 0;
    for (const delta of deltas) {
      if (!(await this.#pause(stream, delivered))) {
        onEvent({ type: "error", error: CANCELLED });
        return;
      }
      onEvent({ type: "delta", text: delta });
      delivered += 1;
      if (stream.failure !== undefined) {
        onEvent({ type: "error", error: stream.failure });
        return;
      }
    }
    if (!(await this.#pause(stream, delivered))) {
      onEvent({ type: "error", error: CANCELLED });
      return;
    }
    onEvent({ type: "done", text: cleanOutput(text), model });
  }

  async #ollamaStream(
    stream: Stream,
    prompt: PromptRequest,
    model: string,
    settings: AiSettings,
    onEvent: (event: AiStreamEvent) => void,
  ): Promise<void> {
    const client = this.#client;
    if (client === undefined) {
      onEvent({ type: "error", error: endpointError("unreachable", "Ollama could not be reached") });
      return;
    }
    const thinking = this.#models.find((candidate) => candidate.name === model)?.capabilities.includes("thinking") === true;
    let whole = "";
    try {
      await client.chatStream(
        chatRequest(prompt, model, thinking ? settings.think : undefined),
        (chunk) => {
          if (stream.cancelled) {
            return;
          }
          if (chunk.content !== "") {
            whole += chunk.content;
            onEvent({ type: "delta", text: chunk.content });
          }
        },
        stream.abort.signal,
      );
    } catch (error) {
      if (stream.cancelled) {
        onEvent({ type: "error", error: CANCELLED });
      } else {
        onEvent({ type: "error", error: error instanceof CoreError ? error.toApiError() : endpointError("unreachable", "Ollama could not be reached") });
      }
      return;
    }
    if (stream.cancelled) {
      onEvent({ type: "error", error: CANCELLED });
      return;
    }
    onEvent({ type: "done", text: cleanOutput(whole), model });
  }

  /**
   * The sources of a question (`ai_chat`): the retrieval over the scope, and for a page scope under 12,000
   * characters that page's whole text as the first source.
   */
  async #sourcesFor(question: string, scope: AiScope, settings: AiSettings): Promise<AiSource[]> {
    let vector: number[] | undefined;
    if (this.#options?.mode === "ollama" && this.#client !== undefined && this.#semanticOn(settings) && this.#indexed.size > 0) {
      try {
        const model = settings.embeddingModel ?? "";
        vector = (await this.#client.embed(model, [embeddingInput(model, question, "query")]))[0];
      } catch {
        vector = undefined;
      }
    }
    const found = this.retrieve(question, scope, { limit: 8, ...(vector === undefined ? {} : { vector }) });
    if (scope.type !== "page") {
      return found;
    }
    const page = this.#deps.pages(scope)[0];
    if (page === undefined) {
      return found;
    }
    const body = page.blocks.map((block) => block.text.trim()).filter((text) => text !== "").join("\n");
    if (body.length > WHOLE_PAGE_CHARS) {
      return found;
    }
    const whole: AiSource = {
      n: 1,
      pageId: page.pageId,
      spaceId: page.spaceId,
      title: page.title,
      text: body.slice(0, MAX_PASSAGE_CHARS),
      score: 1,
      ...(page.icon === undefined ? {} : { icon: page.icon }),
    };
    return [whole, ...found.filter((source) => source.pageId !== page.pageId)].slice(0, 8).map((source, index) => ({ ...source, n: index + 1 }));
  }

  #indexerState(): AiIndexerState {
    if (this.#indexError !== undefined) {
      const state: AiIndexerState = { phase: "error", error: this.#indexError };
      if (this.#lastRunAt !== undefined) {
        state.lastRunAt = this.#lastRunAt;
      }
      return state;
    }
    const settings = aiSettingsOf(this.#deps.setting(AI_SETTINGS_SETTING));
    const pending = this.indexInfo().dirty > 0;
    const state: AiIndexerState = {
      phase: this.#indexing || (this.#indexTimer !== undefined && pending && this.#semanticOn(settings)) ? "indexing" : "idle",
    };
    if (this.#lastRunAt !== undefined) {
      state.lastRunAt = this.#lastRunAt;
    }
    return state;
  }

  /** Starts the timer that indexes a few pages at a time, when the option asks for one and pages wait. */
  #scheduleIndex(): void {
    const step = this.#options?.indexStepMs;
    if (step === undefined || this.#indexTimer !== undefined || this.#stopped) {
      return;
    }
    if (!this.#semanticOn(aiSettingsOf(this.#deps.setting(AI_SETTINGS_SETTING)))) {
      return; // nothing to do until the settings say so (`SettingChanged` schedules again)
    }
    this.#indexTimer = setTimeout(() => {
      this.#indexTimer = undefined;
      void this.advanceIndex(this.#options?.indexBatchPages ?? DEFAULT_INDEX_PAGES).then((done) => {
        if (done > 0) {
          this.#scheduleIndex();
        }
      });
    }, step);
  }
}

// ----- helpers -----------------------------------------------------------------------------------------

function storedEndpoint(value: JsonValue | undefined): Endpoint | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }
  const { url, remoteConsent } = value;
  if (typeof url !== "string") {
    return undefined;
  }
  try {
    return { url: normaliseEndpoint(url).url, remoteConsent: remoteConsent === true };
  } catch {
    return undefined;
  }
}

function validateRequestId(requestId: string): void {
  if (!REQUEST_ID.test(requestId)) {
    throw invalid("requestId", "must be 1 to 64 characters of A-Z, a-z, 0-9, _ and -");
  }
}

function validateGenerate(input: AiGenerateInput): void {
  if (input.text.length > MAX_WRITING_TEXT_CHARS) {
    throw new CoreError({ code: "InvalidInput", message: "the text is too long", details: { field: "text", limit: MAX_WRITING_TEXT_CHARS } });
  }
  if (input.context !== undefined && input.context.length > MAX_WRITING_CONTEXT_CHARS) {
    throw new CoreError({ code: "InvalidInput", message: "the context is too long", details: { field: "context", limit: MAX_WRITING_CONTEXT_CHARS } });
  }
  if (input.prompt !== undefined && input.prompt.length > MAX_CUSTOM_PROMPT_CHARS) {
    throw new CoreError({ code: "InvalidInput", message: "the prompt is too long", details: { field: "prompt", limit: MAX_CUSTOM_PROMPT_CHARS } });
  }
  if (input.language !== undefined && input.language.length > 60) {
    throw invalid("language", "must be at most 60 characters");
  }
  if (input.action === "custom" && (input.prompt ?? "").trim() === "") {
    throw invalid("prompt", "must not be empty");
  }
  if (input.action === "translate" && (input.language ?? "").trim() === "") {
    throw invalid("language", "must not be empty");
  }
  if (input.action === "tone" && input.tone === undefined) {
    throw invalid("tone", "must be given");
  }
  if (input.text.trim() === "" && input.action !== "continue" && input.action !== "custom") {
    throw invalid("text", "must not be empty");
  }
}

function validateChat(input: AiChatInput): void {
  const { messages } = input;
  if (messages.length === 0 || messages.length > MAX_CHAT_MESSAGES) {
    throw new CoreError({ code: "InvalidInput", message: "a chat has 1 to 40 messages", details: { field: "messages", limit: MAX_CHAT_MESSAGES } });
  }
  if (messages[messages.length - 1]?.role !== "user") {
    throw invalid("messages", "the last message must be the person's");
  }
  let total = 0;
  for (const message of messages) {
    if (message.content.length > MAX_CHAT_MESSAGE_CHARS) {
      throw new CoreError({ code: "InvalidInput", message: "a message is too long", details: { field: "messages", limit: MAX_CHAT_MESSAGE_CHARS } });
    }
    total += message.content.length;
  }
  if (total > MAX_CHAT_TOTAL_CHARS) {
    throw new CoreError({ code: "InvalidInput", message: "the conversation is too long", details: { field: "messages", limit: MAX_CHAT_TOTAL_CHARS } });
  }
  if ((messages[messages.length - 1]?.content ?? "").trim() === "") {
    throw invalid("messages", "the question must not be empty");
  }
}

function chatModelOf(settings: AiSettings): string {
  const model = settings.chatModel;
  if (model === undefined || model === "") {
    throw new CoreError({ code: "InvalidInput", message: "no chat model is chosen", details: { field: "model", reason: "missing" } });
  }
  return model;
}

function chatRequest(prompt: PromptRequest, model: string, think: boolean | undefined): OllamaChatRequest {
  return {
    model,
    messages: prompt.messages,
    stream: true,
    ...(think === undefined ? {} : { think }),
    options: { temperature: prompt.temperature, num_ctx: contextSize(prompt.promptChars) },
    keep_alive: "5m",
  };
}

/** At most three passages of a page, keeping the order. */
function perPage(passages: readonly Passage[]): Passage[] {
  const counts = new Map<PageId, number>();
  return passages.filter((passage) => {
    const seen = counts.get(passage.page.pageId) ?? 0;
    counts.set(passage.page.pageId, seen + 1);
    return seen < MAX_PASSAGES_PER_PAGE;
  });
}

function isWav(bytes: Uint8Array): boolean {
  const tag = (from: number): string => String.fromCharCode(...bytes.slice(from, from + 4));
  return bytes.length >= 44 && tag(0) === "RIFF" && tag(8) === "WAVE";
}

/** The length of a mono or stereo PCM WAV in seconds, from its header. */
function wavSeconds(wav: Uint8Array): number {
  const view = new DataView(wav.buffer, wav.byteOffset, wav.byteLength);
  const byteRate = view.getUint32(28, true);
  return byteRate === 0 ? 0 : (wav.length - 44) / byteRate;
}

