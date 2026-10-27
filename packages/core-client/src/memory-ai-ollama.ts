// Ollama over HTTP from the browser, for `MemoryTransport`'s `ollama` AI mode (ADR-0037,
// `docs/architecture/ai.md` section 8.1): `/api/version`, `/api/tags` (with `/api/show` for a model's
// capabilities), `/api/chat` (streamed NDJSON, or one answer for a transcription) and `/api/embed`. The real
// shell does all this in Rust (`ai_client.rs`); a plain browser page can reach Ollama only because its default
// `OLLAMA_ORIGINS` allows `http://localhost:*`. Failures become the `ApiError` the shell would answer
// (`InvalidInput` for the field `endpoint` with a `details.reason`).
//
// `fetch` is injectable so tests run against a stub server.

import { CoreError } from "./errors";
import type { AiModelInfo } from "./generated/AiModelInfo";
import type { ApiError } from "./generated/ApiError";
import type { OllamaMessage } from "./memory-ai-text";

/** The part of `fetch` this module needs. */
export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

/** A request to `/api/chat`. */
export interface OllamaChatRequest {
  model: string;
  messages: OllamaMessage[];
  stream: boolean;
  think?: boolean;
  options: { temperature: number; num_ctx: number };
  keep_alive?: string;
}

/** What one line of a streamed `/api/chat` answer says. */
export interface OllamaChunk {
  content: string;
  thinking?: string;
  done: boolean;
}

/** Longest error text of Ollama that is passed on. */
const MAX_REMOTE_MESSAGE_CHARS = 300;
/** Longest NDJSON line (the shell's limit, 1 MiB). */
const MAX_LINE_CHARS = 1024 * 1024;

/** `InvalidInput` for the field `endpoint` with a reason, as the shell's Ollama client answers. */
export function endpointFailure(reason: string, message: string, extra: Record<string, string | number> = {}): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { field: "endpoint", reason, ...extra } });
}

/** The `ApiError` of {@link endpointFailure}, for a stream's `error` event. */
export function endpointError(reason: string, message: string, extra: Record<string, string | number> = {}): ApiError {
  return endpointFailure(reason, message, extra).toApiError();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The text of an Ollama error body (`{"error": "..."}`), cut to 300 characters. */
async function remoteMessage(response: Response): Promise<string> {
  try {
    const body: unknown = JSON.parse(await response.text());
    if (isRecord(body) && typeof body.error === "string") {
      return body.error.slice(0, MAX_REMOTE_MESSAGE_CHARS);
    }
  } catch {
    // Not JSON: the status says enough.
  }
  return "";
}

/** The model a "model not found" message of Ollama names, when it does. */
function missingModel(message: string): string | undefined {
  return /model ['"]?([^'" ]+)['"]? not found/i.exec(message)?.[1];
}

/** Whether `error` is the abort of a request this module started. */
function isAbort(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
}

/** The failure of a `fetch` that never got an answer, or of one whose status was not 2xx. */
function failureOf(error: unknown): CoreError {
  if (error instanceof CoreError) {
    return error;
  }
  if (isAbort(error)) {
    return endpointFailure("cancelled", "the request was cancelled");
  }
  return endpointFailure("unreachable", "Ollama could not be reached");
}

async function statusFailure(response: Response): Promise<CoreError> {
  const message = await remoteMessage(response);
  const model = missingModel(message);
  if (model !== undefined) {
    return endpointFailure("model", `the model ${model} is not installed`, { model });
  }
  if (message !== "") {
    return endpointFailure("remote", message, { status: response.status });
  }
  return endpointFailure("status", `Ollama answered with status ${String(response.status)}`, { status: response.status });
}

/** The capabilities Ollama lists for a model, or those its name suggests when it does not list any. */
function derivedCapabilities(name: string): string[] {
  return name.toLowerCase().includes("embed") ? ["embedding"] : ["completion"];
}

function stringsOf(value: unknown): string[] | undefined {
  return Array.isArray(value) && value.every((item) => typeof item === "string") ? value : undefined;
}

/** A client of one Ollama server. */
export class OllamaClient {
  readonly #fetch: FetchLike;
  readonly #base: string;
  readonly #capabilities = new Map<string, string[]>();

  constructor(baseUrl: string, fetcher: FetchLike) {
    this.#base = baseUrl.replace(/\/+$/, "");
    this.#fetch = fetcher;
  }

  async #request(path: string, init: RequestInit): Promise<Response> {
    let response: Response;
    try {
      response = await this.#fetch(`${this.#base}${path}`, init);
    } catch (error) {
      throw failureOf(error);
    }
    if (!response.ok) {
      throw await statusFailure(response);
    }
    return response;
  }

  async #json(path: string, init: RequestInit): Promise<unknown> {
    const response = await this.#request(path, init);
    try {
      return await response.json();
    } catch {
      throw endpointFailure("remote", "Ollama sent an answer that is not understood");
    }
  }

  static #post(body: unknown, signal?: AbortSignal): RequestInit {
    return {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      ...(signal === undefined ? {} : { signal }),
    };
  }

  /** Ollama's version (`/api/version`). */
  async version(): Promise<string> {
    const body = await this.#json("/api/version", { method: "GET" });
    return isRecord(body) && typeof body.version === "string" ? body.version : "";
  }

  async #capabilitiesOf(name: string, listed: string[] | undefined): Promise<string[]> {
    if (listed !== undefined) {
      return listed;
    }
    const known = this.#capabilities.get(name);
    if (known !== undefined) {
      return known;
    }
    let capabilities = derivedCapabilities(name);
    try {
      const shown = await this.#json("/api/show", OllamaClient.#post({ model: name }));
      const asked = isRecord(shown) ? stringsOf(shown.capabilities) : undefined;
      if (asked !== undefined) {
        capabilities = asked;
      }
    } catch {
      // The name's guess stands.
    }
    this.#capabilities.set(name, capabilities);
    return capabilities;
  }

  /** The installed models (`/api/tags`), each with its capabilities. */
  async models(): Promise<AiModelInfo[]> {
    const body = await this.#json("/api/tags", { method: "GET" });
    const listed = isRecord(body) && Array.isArray(body.models) ? (body.models as unknown[]) : [];
    const models: AiModelInfo[] = [];
    for (const item of listed) {
      if (!isRecord(item) || typeof item.name !== "string") {
        continue;
      }
      const details = isRecord(item.details) ? item.details : {};
      const capabilities = await this.#capabilitiesOf(item.name, stringsOf(item.capabilities));
      const model: AiModelInfo = {
        name: item.name,
        capabilities,
        size: typeof item.size === "number" ? item.size : 0,
      };
      if (typeof details.family === "string") {
        model.family = details.family;
      }
      if (typeof details.parameter_size === "string") {
        model.parameterSize = details.parameter_size;
      }
      models.push(model);
    }
    return models;
  }

  /**
   * Streams `/api/chat`: `onChunk` gets every line. Rejects with a `CoreError` (`cancelled` when `signal`
   * aborted, `unreachable`, `status`, `model`, `remote`; an `{"error": ...}` line is `remote`).
   */
  async chatStream(request: OllamaChatRequest, onChunk: (chunk: OllamaChunk) => void, signal?: AbortSignal): Promise<void> {
    const response = await this.#request("/api/chat", OllamaClient.#post({ ...request, stream: true }, signal));
    const reader = response.body?.getReader();
    if (reader === undefined) {
      throw endpointFailure("remote", "Ollama sent no stream");
    }
    const decoder = new TextDecoder();
    let pending = "";
    const handle = (line: string): void => {
      if (line.trim() === "") {
        return;
      }
      if (line.length > MAX_LINE_CHARS) {
        throw endpointFailure("remote", "Ollama sent a line that is too long");
      }
      let parsed: unknown;
      try {
        parsed = JSON.parse(line);
      } catch {
        throw endpointFailure("remote", "Ollama sent a line that is not understood");
      }
      if (!isRecord(parsed)) {
        return;
      }
      if (typeof parsed.error === "string") {
        const message = parsed.error.slice(0, MAX_REMOTE_MESSAGE_CHARS);
        const model = missingModel(message);
        throw model === undefined
          ? endpointFailure("remote", message)
          : endpointFailure("model", `the model ${model} is not installed`, { model });
      }
      const message = isRecord(parsed.message) ? parsed.message : {};
      const chunk: OllamaChunk = {
        content: typeof message.content === "string" ? message.content : "",
        done: parsed.done === true,
      };
      if (typeof message.thinking === "string") {
        chunk.thinking = message.thinking;
      }
      onChunk(chunk);
    };
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) {
          break;
        }
        pending += decoder.decode(value, { stream: true });
        let newline = pending.indexOf("\n");
        while (newline >= 0) {
          handle(pending.slice(0, newline));
          pending = pending.slice(newline + 1);
          newline = pending.indexOf("\n");
        }
        if (pending.length > MAX_LINE_CHARS) {
          throw endpointFailure("remote", "Ollama sent a line that is too long");
        }
      }
      pending += decoder.decode();
      handle(pending);
    } catch (error) {
      await reader.cancel().catch(() => undefined);
      throw failureOf(error);
    }
  }

  /** One non-streamed `/api/chat` answer (a transcription): the whole text. */
  async chatOnce(request: OllamaChatRequest, signal?: AbortSignal): Promise<string> {
    const body = await this.#json("/api/chat", OllamaClient.#post({ ...request, stream: false }, signal));
    const message = isRecord(body) && isRecord(body.message) ? body.message : {};
    return typeof message.content === "string" ? message.content : "";
  }

  /** Vectors for texts (`/api/embed`), one per input, in order. */
  async embed(model: string, input: string[]): Promise<number[][]> {
    const body = await this.#json("/api/embed", OllamaClient.#post({ model, input, truncate: true }));
    const vectors = isRecord(body) && Array.isArray(body.embeddings) ? (body.embeddings as unknown[]) : [];
    const out: number[][] = [];
    for (const vector of vectors) {
      if (!Array.isArray(vector) || !vector.every((x) => typeof x === "number")) {
        throw endpointFailure("remote", "Ollama sent an embedding that is not understood");
      }
      out.push(vector);
    }
    if (out.length !== input.length) {
      throw endpointFailure("remote", "Ollama sent the wrong number of embeddings");
    }
    return out;
  }
}
