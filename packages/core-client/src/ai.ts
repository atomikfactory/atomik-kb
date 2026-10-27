// Local AI and MCP helpers of the webview (ADR-0037, `docs/architecture/ai.md` section 8.1): the
// setting keys and their parsers, the default model choice, and the split of an answer into text
// and `[n]` citations (so the UI never builds HTML from model output).

import type { AiModelInfo } from "./generated/AiModelInfo";
import type { AiSettings } from "./generated/AiSettings";
import type { McpSettings } from "./generated/McpSettings";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { invalid } from "./validate";

/** Shell-owned setting holding the `AiEndpoint` (the webview reads it, never writes it). */
export const AI_ENDPOINT_SETTING = "app.ai.endpoint";
/** Setting holding the `AiSettings`. */
export const AI_SETTINGS_SETTING = "app.ai.settings";
/** Setting holding the `McpSettings`. */
export const MCP_SETTINGS_SETTING = "app.mcp.settings";
/** The endpoint used when none is stored. */
export const DEFAULT_AI_ENDPOINT = "http://127.0.0.1:11434";
/** The MCP port used when none is stored. */
export const DEFAULT_MCP_PORT = 47811;
/** The lowest and highest port the MCP server accepts. */
export const MIN_MCP_PORT = 1024;
export const MAX_MCP_PORT = 65535;

function isObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** A model name from a stored value: a non-empty string (trimmed), else absent. */
function modelName(value: unknown): string | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  const name = value.trim();
  return name === "" ? undefined : name;
}

/**
 * The settings a stored value describes; defaults (AI off, semantic index on, no thinking) for anything
 * else. A model that is not a non-empty string is left out; `semanticIndex` is on unless it is `false`.
 */
export function aiSettingsOf(value: JsonValue | null | undefined): AiSettings {
  if (!isObject(value)) {
    return { enabled: false, semanticIndex: true, think: false };
  }
  const settings: AiSettings = {
    enabled: value.enabled === true,
    semanticIndex: value.semanticIndex !== false,
    think: value.think === true,
  };
  const chatModel = modelName(value.chatModel);
  if (chatModel !== undefined) {
    settings.chatModel = chatModel;
  }
  const embeddingModel = modelName(value.embeddingModel);
  if (embeddingModel !== undefined) {
    settings.embeddingModel = embeddingModel;
  }
  const transcriptionModel = modelName(value.transcriptionModel);
  if (transcriptionModel !== undefined) {
    settings.transcriptionModel = transcriptionModel;
  }
  return settings;
}

/** Whether `port` is a port the MCP server may bind (a whole number from 1024 to 65535). */
export function isMcpPort(port: unknown): port is number {
  return typeof port === "number" && Number.isInteger(port) && port >= MIN_MCP_PORT && port <= MAX_MCP_PORT;
}

/** The MCP settings a stored value describes; defaults (off, port 47811) for anything else. */
export function mcpSettingsOf(value: JsonValue | null | undefined): McpSettings {
  if (!isObject(value)) {
    return { enabled: false, port: DEFAULT_MCP_PORT };
  }
  return { enabled: value.enabled === true, port: isMcpPort(value.port) ? value.port : DEFAULT_MCP_PORT };
}

/** Whether Ollama lists `capability` for the model. */
function can(model: AiModelInfo, capability: string): boolean {
  return model.capabilities.includes(capability);
}

/**
 * The models to fill in when AI is turned on (section 2): chat `qwen3:8b` when installed, else the first
 * completion model that is not an embedding model; embedding `nomic-embed-text*` when installed, else the
 * first embedding model; transcription none (the person chooses a model that understands audio).
 */
export function defaultModels(models: readonly AiModelInfo[]): {
  chatModel?: string;
  embeddingModel?: string;
} {
  const chat =
    models.find((model) => model.name === "qwen3:8b") ??
    models.find((model) => can(model, "completion") && !can(model, "embedding"));
  const embedding =
    models.find((model) => model.name.startsWith("nomic-embed-text")) ?? models.find((model) => can(model, "embedding"));
  const chosen: { chatModel?: string; embeddingModel?: string } = {};
  if (chat !== undefined) {
    chosen.chatModel = chat.name;
  }
  if (embedding !== undefined) {
    chosen.embeddingModel = embedding.name;
  }
  return chosen;
}

/** Whether the host of a normalised endpoint is this machine: `localhost`, `127.0.0.0/8` or `::1`. */
export function isLocalHost(hostname: string): boolean {
  const host = hostname.toLowerCase();
  return host === "localhost" || host === "[::1]" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
}

/**
 * The endpoint rules of `kb_ai::endpoint`: `http` or `https`, no credentials, no query or fragment, a path of `/`
 * or none, at most 512 characters; the result has no trailing slash. `InvalidInput` for `url` otherwise.
 */
export function normaliseEndpoint(raw: string): { url: string; local: boolean } {
  const text = raw.trim();
  if (text === "" || text.length > 512) {
    throw invalid("url", "must be an http or https address of at most 512 characters");
  }
  let parsed: URL;
  try {
    parsed = new URL(text);
  } catch {
    throw invalid("url", "is not a web address");
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw invalid("url", "must start with http:// or https://");
  }
  if (parsed.username !== "" || parsed.password !== "") {
    throw invalid("url", "must not contain a user name or password");
  }
  if (parsed.search !== "" || parsed.hash !== "" || text.includes("?") || text.includes("#")) {
    throw invalid("url", "must not contain a query or a fragment");
  }
  if (parsed.pathname !== "/" && parsed.pathname !== "") {
    throw invalid("url", "must not contain a path");
  }
  const url = `${parsed.protocol}//${parsed.host}`;
  if (url.length > 512) {
    throw invalid("url", "must be at most 512 characters");
  }
  return { url, local: isLocalHost(parsed.hostname) };
}

/** A piece of an answer: plain text, or a citation of source `n`. */
export type CitationPart = { type: "text"; text: string } | { type: "cite"; n: number };

/**
 * A citation: one to three digits, or several separated by commas, in square brackets, and not the
 * text of a Markdown link (`[1](https://...)`). Matched with `matchAll`, so the `g` flag is needed.
 */
const CITATION = /\[(\d{1,2}(?:\s*,\s*\d{1,2})*)\](?!\()/g;

/**
 * Splits text at `[n]` citations (`[1]`, `[2][3]`, `[1, 2]`: each number is one citation); other brackets
 * stay text, and so does `[0]`. Adjacent text is joined, an empty text gives no parts. The UI renders a
 * citation as a chip when it knows source `n` and as the text `[n]` otherwise.
 */
export function citationParts(text: string): CitationPart[] {
  const parts: CitationPart[] = [];
  let plain = "";
  let at = 0;
  const flush = (): void => {
    if (plain !== "") {
      parts.push({ type: "text", text: plain });
      plain = "";
    }
  };
  for (const match of text.matchAll(CITATION)) {
    const numbers = (match[1] ?? "").split(",").map((part) => Number(part.trim()));
    if (numbers.some((n) => n < 1)) {
      continue; // `[0]` is not a source: it stays text
    }
    plain += text.slice(at, match.index);
    flush();
    for (const n of numbers) {
      parts.push({ type: "cite", n });
    }
    at = match.index + match[0].length;
  }
  plain += text.slice(at);
  flush();
  return parts;
}
