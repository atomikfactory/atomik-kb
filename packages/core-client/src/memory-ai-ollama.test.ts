// The `ollama` AI mode of `MemoryTransport` against a stub of Ollama's HTTP API (`fetch` is injected): the status
// with capabilities, streamed NDJSON (also split in the middle of a character), the request the prompts make,
// errors as the shell words them, cancelling, embeddings for the index and for retrieval, and transcription.

import { describe, expect, it } from "vitest";

import { AI_SETTINGS_SETTING } from "./ai";
import { encodeWav } from "./audio";
import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { AiStreamEvent } from "./generated/AiStreamEvent";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { FetchLike } from "./memory-ai-ollama";

const BASE = "http://127.0.0.1:11434";
const encoder = new TextEncoder();

interface Call {
  path: string;
  method: string;
  body: Record<string, unknown> | undefined;
}

interface Stub {
  fetch: FetchLike;
  calls: Call[];
}

type Handler = (call: Call, init: RequestInit | undefined) => Response | Promise<Response>;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

/** An NDJSON stream whose chunks are given as byte arrays (so a test can cut a character in two). */
function ndjson(pieces: Uint8Array[], options: { holdOpen?: AbortSignal } = {}): Response {
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const piece of pieces) {
        controller.enqueue(piece);
      }
      if (options.holdOpen === undefined) {
        controller.close();
      } else {
        options.holdOpen.addEventListener("abort", () => {
          controller.error(Object.assign(new Error("aborted"), { name: "AbortError" }));
        });
      }
    },
  });
  return new Response(stream, { status: 200, headers: { "Content-Type": "application/x-ndjson" } });
}

function lines(...objects: unknown[]): Uint8Array {
  return encoder.encode(objects.map((object) => JSON.stringify(object)).join("\n") + "\n");
}

const chunk = (content: string, done = false) => ({ message: { role: "assistant", content }, done });

function stub(handler: Handler): Stub {
  const calls: Call[] = [];
  const fetcher: FetchLike = async (input, init) => {
    const url = new URL(input);
    const raw = init?.body;
    const call: Call = {
      path: url.pathname,
      method: init?.method ?? "GET",
      body: typeof raw === "string" ? (JSON.parse(raw) as Record<string, unknown>) : undefined,
    };
    calls.push(call);
    return handler(call, init);
  };
  return { fetch: fetcher, calls };
}

const MODELS = [
  { name: "qwen3:8b", size: 5_000_000_000, details: { family: "qwen3", parameter_size: "8.2B" } },
  { name: "nomic-embed-text:latest", size: 274_000_000, details: { family: "nomic-bert", parameter_size: "137M" } },
  { name: "r2t2:latest", size: 1_000_000_000, capabilities: ["completion", "audio"] },
];

function ollamaDefaults(call: Call): Response | undefined {
  switch (call.path) {
    case "/api/version":
      return json({ version: "0.35.0" });
    case "/api/tags":
      return json({ models: MODELS });
    case "/api/show":
      return json({ capabilities: call.body?.model === "qwen3:8b" ? ["completion", "thinking", "tools"] : ["embedding"] });
    default:
      return undefined;
  }
}

const ON: JsonValue = {
  enabled: true,
  chatModel: "qwen3:8b",
  embeddingModel: "nomic-embed-text:latest",
  transcriptionModel: "r2t2:latest",
  semanticIndex: true,
  think: true,
};

function uuid(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

function paragraph(id: number, text: string): JsonValue {
  return { type: "paragraph", attrs: { id: uuid(id) }, content: [{ type: "text", text }] };
}

function setup(server: Stub, extra: MemoryTransportOptions = {}, settings: JsonValue = ON) {
  const memory = new MemoryTransport({
    workspaceName: "Home",
    spaces: [
      {
        name: "Personal",
        pages: [
          { title: "Garden", content: { type: "doc", content: [paragraph(901, "Tomatoes need full sun."), paragraph(902, "Water the beans daily.")] } },
          { title: "Cooking", content: { type: "doc", content: [paragraph(903, "Tomato soup needs basil.")] } },
        ],
      },
    ],
    ai: { mode: "ollama", baseUrl: BASE, fetch: server.fetch },
    settings: { [AI_SETTINGS_SETTING]: settings },
    ...extra,
  });
  const client = new CoreClient(memory);
  return { memory, client, ai: memory.ai };
}

async function collect(run: (onEvent: (event: AiStreamEvent) => void) => Promise<void>): Promise<AiStreamEvent[]> {
  const events: AiStreamEvent[] = [];
  await run((event) => events.push(event));
  return events;
}

const deltas = (events: readonly AiStreamEvent[]) => events.flatMap((event) => (event.type === "delta" ? [event.text] : []));

describe("the status", () => {
  it("reads the version and the models, with capabilities from the tags or from /api/show", async () => {
    const server = stub((call) => ollamaDefaults(call) ?? json({}, 404));
    const { client } = setup(server);
    const status = await client.aiStatus();
    expect(status).toMatchObject({ endpoint: BASE, local: true, reachable: true, version: "0.35.0" });
    expect(status.models).toEqual([
      { name: "qwen3:8b", family: "qwen3", parameterSize: "8.2B", capabilities: ["completion", "thinking", "tools"], size: 5_000_000_000 },
      { name: "nomic-embed-text:latest", family: "nomic-bert", parameterSize: "137M", capabilities: ["embedding"], size: 274_000_000 },
      { name: "r2t2:latest", capabilities: ["completion", "audio"], size: 1_000_000_000 },
    ]);
    // The capabilities of a model are asked for once.
    await client.aiStatus();
    expect(server.calls.filter((call) => call.path === "/api/show")).toHaveLength(2);
  });

  it("derives capabilities from the name when /api/show fails", async () => {
    const server = stub((call) => (call.path === "/api/show" ? json({ error: "nope" }, 500) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client } = setup(server);
    const models = (await client.aiStatus()).models;
    expect(models.find((model) => model.name === "qwen3:8b")?.capabilities).toEqual(["completion"]);
    expect(models.find((model) => model.name === "nomic-embed-text:latest")?.capabilities).toEqual(["embedding"]);
  });

  it("says Ollama is unreachable when the connection fails", async () => {
    const server = stub(() => {
      throw new TypeError("Failed to fetch");
    });
    const { client } = setup(server);
    const status = await client.aiStatus();
    expect(status).toMatchObject({ reachable: false, models: [] });
    expect(status.error).toMatchObject({ code: "InvalidInput", details: { field: "endpoint", reason: "unreachable" } });
  });

  it("reports a status error as the shell does, with the status", async () => {
    const server = stub(() => json({}, 503));
    const { client } = setup(server);
    expect((await client.aiStatus()).error).toMatchObject({ details: { field: "endpoint", reason: "status", status: 503 } });
  });

  it("uses the endpoint a person sets", async () => {
    const server = stub((call) => ollamaDefaults(call) ?? json({}, 404));
    const { client } = setup(server);
    const urls: string[] = [];
    const memory = new MemoryTransport({
      ai: { mode: "ollama", baseUrl: BASE, fetch: (input, init) => (urls.push(input), server.fetch(input, init)) },
    });
    const other = new CoreClient(memory);
    await other.aiEndpointSet("http://localhost:11500");
    expect(urls.every((url) => url.startsWith("http://localhost:11500/"))).toBe(true);
    expect(urls.length).toBeGreaterThan(0);
    expect(client.hasAi()).toBe(true);
  });
});

describe("writing", () => {
  it("streams the content of every line, then done with the cleaned text", async () => {
    const server = stub((call) => {
      if (call.path === "/api/chat") {
        return ndjson([lines(chunk("<think>"), chunk("hmm"), chunk("</think>"), chunk("Hello "), chunk("world"), chunk("", true))]);
      }
      return ollamaDefaults(call) ?? json({}, 404);
    });
    const { client } = setup(server);
    await client.aiStatus(); // learns that qwen3 can think
    const events = await collect((onEvent) => client.aiGenerate("r1", { action: "improve", text: "hello wrold" }, onEvent));
    expect(deltas(events)).toEqual(["<think>", "hmm", "</think>", "Hello ", "world"]);
    expect(events.at(-1)).toEqual({ type: "done", text: "Hello world", model: "qwen3:8b" });
  });

  it("builds the request from the prompt: model, messages, temperature, context size and think", async () => {
    const server = stub((call) => (call.path === "/api/chat" ? ndjson([lines(chunk("ok", true))]) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client } = setup(server);
    await client.aiStatus();
    await client.aiGenerate("r1", { action: "tone", tone: "casual", text: "Dear sir", context: "Letter" }, () => undefined);
    const request = server.calls.find((call) => call.path === "/api/chat")?.body;
    expect(request).toMatchObject({ model: "qwen3:8b", stream: true, think: true, options: { temperature: 0.4, num_ctx: 4096 } });
    const messages = request?.messages as { role: string; content: string }[];
    expect(messages[0]?.role).toBe("system");
    expect(messages[1]?.content).toContain("Rewrite in a casual tone");
    expect(messages[1]?.content).toContain("Dear sir");
  });

  it("sends `think` only to a model that can think, with the person's setting", async () => {
    const server = stub((call) => (call.path === "/api/chat" ? ndjson([lines(chunk("ok", true))]) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client } = setup(server, {}, { ...(ON as object), think: false, chatModel: "r2t2:latest" });
    await client.aiStatus();
    await client.aiGenerate("r1", { action: "improve", text: "x" }, () => undefined);
    expect(server.calls.find((call) => call.path === "/api/chat")?.body).not.toHaveProperty("think");
    const thinker = stub((call) => (call.path === "/api/chat" ? ndjson([lines(chunk("ok", true))]) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client: other } = setup(thinker, {}, { ...(ON as object), think: false });
    await other.aiStatus();
    await other.aiGenerate("r1", { action: "improve", text: "x" }, () => undefined);
    expect(thinker.calls.find((call) => call.path === "/api/chat")?.body).toMatchObject({ think: false });
  });

  it("reads lines cut in the middle, and a character cut in two", async () => {
    const whole = lines(chunk("héllo "), chunk("wörld ☃", true));
    const cut = 38; // inside a multi-byte character or a line
    const pieces = [whole.slice(0, cut), whole.slice(cut, cut + 1), whole.slice(cut + 1)];
    const server = stub((call) => (call.path === "/api/chat" ? ndjson(pieces) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client } = setup(server);
    const events = await collect((onEvent) => client.aiGenerate("r1", { action: "improve", text: "x" }, onEvent));
    expect(deltas(events).join("")).toBe("héllo wörld ☃");
    expect(events.at(-1)).toMatchObject({ type: "done", text: "héllo wörld ☃" });
  });

  it("reads a last line without a newline", async () => {
    const body = encoder.encode(JSON.stringify(chunk("a")) + "\n" + JSON.stringify(chunk("b", true)));
    const server = stub((call) => (call.path === "/api/chat" ? ndjson([body]) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client } = setup(server);
    const events = await collect((onEvent) => client.aiGenerate("r1", { action: "improve", text: "x" }, onEvent));
    expect(deltas(events)).toEqual(["a", "b"]);
  });

  it("ends with an error event when Ollama cannot be reached, a model is missing or it answers an error line", async () => {
    const cases: { handler: Handler; error: Record<string, unknown> }[] = [
      {
        handler: () => {
          throw new TypeError("Failed to fetch");
        },
        error: { details: { field: "endpoint", reason: "unreachable" } },
      },
      { handler: () => json({ error: "model 'qwen3:8b' not found" }, 404), error: { details: { field: "endpoint", reason: "model", model: "qwen3:8b" } } },
      { handler: () => json({ error: "out of memory" }, 500), error: { message: "out of memory", details: { reason: "remote", status: 500 } } },
      { handler: () => json({}, 502), error: { details: { reason: "status", status: 502 } } },
      { handler: () => ndjson([lines(chunk("a"), { error: "something broke" })]), error: { message: "something broke", details: { reason: "remote" } } },
      { handler: () => ndjson([encoder.encode("not json\n")]), error: { details: { reason: "remote" } } },
    ];
    for (const { handler, error } of cases) {
      const server = stub((call) => (call.path === "/api/chat" ? handler(call, undefined) : (ollamaDefaults(call) ?? json({}, 404))));
      const { client } = setup(server);
      const events = await collect((onEvent) => client.aiGenerate("r1", { action: "improve", text: "x" }, onEvent));
      expect(events.at(-1), JSON.stringify(error)).toMatchObject({ type: "error", error: { code: "InvalidInput", ...error } });
      expect(events.some((event) => event.type === "done")).toBe(false);
    }
  });

  it("cuts a long Ollama error message at 300 characters", async () => {
    const server = stub((call) => (call.path === "/api/chat" ? json({ error: "e".repeat(1000) }, 500) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client } = setup(server);
    const events = await collect((onEvent) => client.aiGenerate("r1", { action: "improve", text: "x" }, onEvent));
    const last = events.at(-1);
    expect(last?.type === "error" ? last.error.message : "").toHaveLength(300);
  });

  it("cancels: aborts the request and ends with a cancelled error", async () => {
    let aborted = false;
    const server = stub((call, init) => {
      if (call.path === "/api/chat") {
        init?.signal?.addEventListener("abort", () => {
          aborted = true;
        });
        return ndjson([lines(chunk("first "))], { holdOpen: init?.signal ?? undefined });
      }
      return ollamaDefaults(call) ?? json({}, 404);
    });
    const { client } = setup(server);
    const events: AiStreamEvent[] = [];
    const running = client.aiGenerate("stop", { action: "improve", text: "x" }, (event) => events.push(event));
    await expect.poll(() => events.length).toBeGreaterThan(0);
    await client.aiCancel("stop");
    await running;
    expect(aborted).toBe(true);
    expect(events.at(-1)).toMatchObject({ type: "error", error: { details: { reason: "cancelled" } } });
  });
});

describe("chat", () => {
  it("sends the sources, then streams an answer built from the numbered sources", async () => {
    const server = stub((call) => (call.path === "/api/chat" ? ndjson([lines(chunk("Water [1]."), chunk("", true))]) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client } = setup(server);
    const events = await collect((onEvent) =>
      client.aiChat("c1", { messages: [{ role: "user", content: "How often to water the beans?" }], scope: { type: "all" } }, onEvent),
    );
    expect(events[0]).toMatchObject({ type: "sources", sources: [{ n: 1, title: "Garden", text: "Water the beans daily." }] });
    expect(events.at(-1)).toEqual({ type: "done", text: "Water [1].", model: "qwen3:8b" });
    const request = server.calls.find((call) => call.path === "/api/chat")?.body;
    const messages = request?.messages as { role: string; content: string }[];
    expect(messages[0]?.content).toContain("[1] Garden\nWater the beans daily.");
    expect(messages.at(-1)).toEqual({ role: "user", content: "How often to water the beans?" });
    expect(request).toMatchObject({ options: { temperature: 0.3 } });
  });
});

describe("the index and semantic retrieval", () => {
  /** An embedding model that gives "tomato" texts one direction and the rest another. */
  function embedder(call: Call): Response | undefined {
    if (call.path !== "/api/embed") {
      return undefined;
    }
    const inputs = call.body?.input as string[];
    const vector = (text: string): number[] =>
      Array.from({ length: 16 }, (_, index) => (/tomato/i.test(text) ? (index === 0 ? 1 : 0) : index === 1 ? 1 : 0));
    return json({ embeddings: inputs.map(vector) });
  }

  it("embeds the pages' chunks with the document prefix in batches, and reports the dimension", async () => {
    const server = stub((call) => embedder(call) ?? ollamaDefaults(call) ?? json({}, 404));
    const { client, ai } = setup(server);
    expect(await ai?.advanceIndex()).toBe(2);
    const embeds = server.calls.filter((call) => call.path === "/api/embed");
    expect(embeds).toHaveLength(2);
    expect(embeds[0]?.body).toMatchObject({ model: "nomic-embed-text:latest", truncate: true });
    expect((embeds[0]?.body?.input as string[])[0]).toBe("search_document: Garden\nTomatoes need full sun.\nWater the beans daily.");
    expect(await client.aiIndexStatus()).toMatchObject({ pagesIndexed: 2, dimensions: 16, model: "nomic-embed-text:latest" });
  });

  it("finds a passage that shares no word with the question through the vectors, fused with the keywords", async () => {
    const server = stub((call) => embedder(call) ?? ollamaDefaults(call) ?? json({}, 404));
    const { client, ai } = setup(server);
    await ai?.advanceIndex();
    // "tomato" is the only way the question reaches the Garden chunk about watering.
    const found = await client.aiSearch("tomato", { type: "all" });
    expect(found.length).toBeGreaterThan(0);
    const queryCall = server.calls.filter((call) => call.path === "/api/embed").at(-1);
    expect((queryCall?.body?.input as string[])[0]).toBe("search_query: tomato");
    // A vector-only match: the Garden chunk is tomato-like, so it comes up for a question with no word in common.
    const retrieved = await client.aiRetrieve("zucchini", { type: "all" }, { vector: Array.from({ length: 16 }, (_, index) => (index === 0 ? 1 : 0)) });
    expect(retrieved.map((source) => source.title).sort()).toEqual(["Cooking", "Garden"]);
    expect(retrieved[0]?.blockId).toBeDefined();
  });

  it("falls back to keywords when embedding the question fails", async () => {
    let failing = false;
    const server = stub((call) => {
      if (call.path === "/api/embed" && failing) {
        return json({ error: "boom" }, 500);
      }
      return embedder(call) ?? ollamaDefaults(call) ?? json({}, 404);
    });
    const { client, ai } = setup(server);
    await ai?.advanceIndex();
    failing = true;
    const found = await client.aiSearch("tomato", { type: "all" });
    expect(found.map((source) => source.title)).toContain("Garden");
  });

  it("records an error in the indexer's state when embedding fails, and keeps the page waiting", async () => {
    const server = stub((call) => (call.path === "/api/embed" ? json({ error: "boom" }, 500) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client, ai } = setup(server);
    expect(await ai?.advanceIndex()).toBe(0);
    expect((await client.aiStatus()).indexer).toMatchObject({ phase: "error", error: "boom" });
    expect((await client.aiIndexStatus()).pagesIndexed).toBe(0);
  });

  it("refuses an embedding of an unusable size", async () => {
    const server = stub((call) => (call.path === "/api/embed" ? json({ embeddings: [[1, 2, 3]] }) : (ollamaDefaults(call) ?? json({}, 404))));
    const { client, ai } = setup(server);
    expect(await ai?.advanceIndex()).toBe(0);
    expect((await client.aiStatus()).indexer.phase).toBe("error");
  });
});

describe("transcription", () => {
  const wav = encodeWav(new Int16Array(1600));

  it("sends the WAV as the one image of a non-streamed request and cleans the answer", async () => {
    const server = stub((call) =>
      call.path === "/api/chat" ? json({ message: { role: "assistant", content: "language English<asr_text>Hello, this is a test.</asr_text>" }, done: true }) : (ollamaDefaults(call) ?? json({}, 404)),
    );
    const { client } = setup(server);
    expect(await client.aiTranscribe(wav)).toEqual({ text: "Hello, this is a test.", language: "English" });
    const request = server.calls.find((call) => call.path === "/api/chat")?.body;
    expect(request).toMatchObject({ model: "r2t2:latest", stream: false, options: { temperature: 0 } });
    const [message] = request?.messages as { content: string; images: string[] }[];
    expect(message?.images).toHaveLength(1);
    const decoded = atob(message?.images[0] ?? "");
    expect(decoded.slice(0, 4)).toBe("RIFF");
    expect(decoded).toHaveLength(wav.length);
  });

  it("rejects with the shell's error when Ollama cannot be reached", async () => {
    const server = stub((call) => {
      if (call.path === "/api/chat") {
        throw new TypeError("Failed to fetch");
      }
      return ollamaDefaults(call) ?? json({}, 404);
    });
    const { client } = setup(server);
    const error: unknown = await client.aiTranscribe(wav).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "endpoint", reason: "unreachable" } });
  });
});
