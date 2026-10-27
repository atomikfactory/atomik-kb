// The local AI of `MemoryTransport` (ADR-0037, `docs/architecture/ai.md`): the core's index and retrieval, the
// shell's commands in `fake` mode (status, endpoint, writing, chat, search, transcription, the limits of section
// 10, streams that are cancelled, held or failed) and the MCP status.

import { afterEach, describe, expect, it, vi } from "vitest";

import { AI_ENDPOINT_SETTING, AI_SETTINGS_SETTING, MCP_SETTINGS_SETTING, normaliseEndpoint } from "./ai";
import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { AiChatInput } from "./generated/AiChatInput";
import type { AiGenerateInput } from "./generated/AiGenerateInput";
import type { AiStreamEvent } from "./generated/AiStreamEvent";
import type { Event } from "./generated/Event";
import type { Request } from "./generated/Request";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { FAKE_MODELS } from "./memory-ai";
import { encodeWav } from "./audio";

afterEach(() => {
  vi.useRealTimers();
});

const ON: JsonValue = {
  enabled: true,
  chatModel: "qwen3:8b",
  embeddingModel: "nomic-embed-text:latest",
  transcriptionModel: "r2t2:latest",
  semanticIndex: true,
  think: false,
};

function uuid(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

function doc(...paragraphs: string[]): JsonValue {
  return {
    type: "doc",
    content: paragraphs.map((text, index) => ({
      type: "paragraph",
      attrs: { id: uuid(900 + index) },
      content: [{ type: "text", text }],
    })),
  };
}

/** Two spaces: Personal (Garden, Cooking, a page in the trash later) and Work (Roadmap). */
function seeds(): MemoryTransportOptions {
  return {
    workspaceName: "Home",
    spaces: [
      {
        name: "Personal",
        pages: [
          {
            title: "Garden plans",
            content: {
              type: "doc",
              content: [
                { type: "paragraph", attrs: { id: uuid(901) }, content: [{ type: "text", text: "Tomatoes need full sun and regular watering." }] },
                { type: "paragraph", attrs: { id: uuid(902) }, content: [{ type: "text", text: "Plant the beans after the last frost." }] },
                { type: "paragraph", attrs: { id: uuid(903) }, content: [{ type: "text", text: "Compost the leaves in autumn." }] },
              ],
            },
          },
          { title: "Cooking", content: doc("Tomato soup needs ripe tomatoes and basil.") },
          { title: "Empty page" },
        ],
      },
      { name: "Work", pages: [{ title: "Roadmap", content: doc("Ship the tomato tracker in spring.", "Review the budget in May.") }] },
    ],
  };
}

async function setup(options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({ ...seeds(), ...options });
  const client = new CoreClient(memory);
  const events: Event[] = [];
  memory.subscribe((event) => events.push(event));
  const spaces = await client.spaceList();
  const personal = spaces[0]?.id ?? "";
  const work = spaces[1]?.id ?? "";
  const tree = await client.treeGet(personal);
  const pageId = (title: string): string => tree.roots.find((node) => node.page.title === title)?.page.id ?? "";
  return { memory, client, events, personal, work, garden: pageId("Garden plans"), cooking: pageId("Cooking"), empty: pageId("Empty page") };
}

async function aiOn(options: MemoryTransportOptions = {}, settings: JsonValue = ON) {
  const world = await setup({ ai: { mode: "fake" }, settings: { [AI_SETTINGS_SETTING]: settings }, ...options });
  if (world.memory.ai === undefined) {
    throw new Error("the AI option gives the transport an ai");
  }
  return { ...world, ai: world.memory.ai };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

async function stream(
  run: (onEvent: (event: AiStreamEvent) => void) => Promise<void>,
): Promise<AiStreamEvent[]> {
  const events: AiStreamEvent[] = [];
  await run((event) => events.push(event));
  return events;
}

function text(events: readonly AiStreamEvent[]): string {
  return events.map((event) => (event.type === "delta" ? event.text : "")).join("");
}

const improve = (value: string): AiGenerateInput => ({ action: "improve", text: value });
const ask = (question: string, scope: AiChatInput["scope"] = { type: "all" }): AiChatInput => ({
  messages: [{ role: "user", content: question }],
  scope,
});

describe("without the ai option", () => {
  it("has no AI or MCP transport, but the core's side answers", async () => {
    const { client, memory } = await setup();
    expect(client.hasAi()).toBe(false);
    expect(client.hasMcp()).toBe(false);
    expect(memory.ai).toBeUndefined();
    expect(memory.mcp).toBeUndefined();
    expect(await client.aiIndexStatus()).toEqual({ pagesIndexed: 0, pagesTotal: 4, chunks: 0, dirty: 4 });
    expect(await client.mcpClientList()).toEqual([]);
    expect((await client.aiRetrieve("tomato", { type: "all" })).length).toBeGreaterThan(0);
  });

  it("refuses the app's own requests and the webview's write of the endpoint", async () => {
    const { client, memory } = await setup();
    const requests: Request[] = [
      { type: "AiIndexNext", data: { maxChunks: 4 } },
      { type: "AiIndexStore", data: { pageId: "x", sourceHash: "", model: "m", vectors: [] } },
      { type: "McpCall", data: { token: "t", message: "{}" } },
      { type: "McpClientGrantsSet", data: { clientId: "c", grants: [] } },
    ];
    for (const request of requests) {
      const error = await failure(memory.dispatch(request));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "type" } });
    }
    const error = await failure(client.settingsSet(AI_ENDPOINT_SETTING, { url: "http://127.0.0.1:11434", remoteConsent: true }));
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "key" } });
    await expect(client.settingsSet(AI_SETTINGS_SETTING, ON)).resolves.toBeUndefined();
  });
});

describe("AiRetrieve (keyword scoring)", () => {
  it("finds blocks by any of the query's words, best first, numbered from 1", async () => {
    const { client, garden } = await setup();
    const found = await client.aiRetrieve("tomatoes watering", { type: "all" });
    expect(found[0]).toMatchObject({ n: 1, pageId: garden, title: "Garden plans", blockId: uuid(901) });
    expect(found[0]?.text).toBe("Tomatoes need full sun and regular watering.");
    expect(found.map((source) => source.n)).toEqual(found.map((_, index) => index + 1));
    // Any word, not all: the Cooking page has "tomatoes" only.
    expect(found.map((source) => source.title)).toContain("Cooking");
    for (let i = 1; i < found.length; i += 1) {
      expect(found[i - 1]?.score ?? 0).toBeGreaterThanOrEqual(found[i]?.score ?? 0);
    }
  });

  it("limits to a space, a page, or everything, and never returns trashed pages", async () => {
    const { client, personal, work, garden, cooking } = await setup();
    const inWork = await client.aiRetrieve("tomato", { type: "space", spaceId: work });
    expect(inWork.map((source) => source.title)).toEqual(["Roadmap"]);
    const inPersonal = await client.aiRetrieve("tomato", { type: "space", spaceId: personal });
    expect(inPersonal.every((source) => source.spaceId === personal)).toBe(true);
    const onPage = await client.aiRetrieve("tomatoes beans", { type: "page", pageId: garden });
    expect(onPage.every((source) => source.pageId === garden)).toBe(true);
    expect(onPage).toHaveLength(2);
    await client.pageTrash(cooking);
    const all = await client.aiRetrieve("tomato", { type: "all" });
    expect(all.some((source) => source.pageId === cooking)).toBe(false);
    expect(await client.aiRetrieve("tomato", { type: "page", pageId: cooking })).toEqual([]);
  });

  it("matches a title with no matching block as a passage of the title alone", async () => {
    const { client, empty } = await setup();
    const found = await client.aiRetrieve("empty", { type: "all" });
    expect(found).toHaveLength(1);
    expect(found[0]).toMatchObject({ pageId: empty, title: "Empty page", text: "Empty page" });
    expect(found[0]?.blockId).toBeUndefined();
  });

  it("drops words of two characters or fewer, and finds nothing for a query of such words", async () => {
    const { client } = await setup();
    expect(await client.aiRetrieve("is it in", { type: "all" })).toEqual([]);
    expect((await client.aiRetrieve("is the tomato", { type: "all" })).length).toBeGreaterThan(0);
  });

  it("answers at most 3 passages per page and at most `limit` in all (default 8, at most 20)", async () => {
    const lines = Array.from({ length: 6 }, (_, index) => `Tomato note number ${String(index)}.`);
    const { client } = await setup({ spaces: [{ name: "S", pages: [{ title: "Big", content: doc(...lines) }, { title: "Two", content: doc(...lines) }] }] });
    const found = await client.aiRetrieve("tomato", { type: "all" });
    expect(found).toHaveLength(6);
    expect(found.filter((source) => source.title === "Big")).toHaveLength(3);
    expect(await client.aiRetrieve("tomato", { type: "all" }, { limit: 2 })).toHaveLength(2);
    expect((await client.aiRetrieve("tomato", { type: "all" }, { limit: 500 })).length).toBeLessThanOrEqual(20);
  });

  it("cuts a passage at 1,500 characters", async () => {
    const long = `Tomato ${"word ".repeat(600)}`;
    const { client } = await setup({ spaces: [{ name: "S", pages: [{ title: "Long", content: doc(long) }] }] });
    const [source] = await client.aiRetrieve("tomato", { type: "all" });
    expect(source?.text).toHaveLength(1500);
  });

  it("validates the query, the vector and the scope", async () => {
    const { client } = await setup();
    expect(await failure(client.aiRetrieve("   ", { type: "all" }))).toMatchObject({ code: "InvalidInput", details: { field: "query" } });
    expect(await failure(client.aiRetrieve("x".repeat(2001), { type: "all" }))).toMatchObject({ details: { field: "query" } });
    expect(await failure(client.aiRetrieve("tomato", { type: "all" }, { vector: [1, 2, 3] }))).toMatchObject({ details: { field: "vector" } });
    expect(await failure(client.aiRetrieve("tomato", { type: "space", spaceId: "nope" }))).toMatchObject({ code: "NotFound" });
    expect(await failure(client.aiRetrieve("tomato", { type: "page", pageId: "nope" }))).toMatchObject({ code: "NotFound" });
  });

  it("finds property-less text through page references by the referenced title", async () => {
    const reference: JsonValue = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: uuid(950) },
          content: [{ type: "text", text: "See " }, { type: "pageRef", attrs: { pageId: "00000000-0000-4000-8000-000000000003" } }],
        },
      ],
    };
    const { client } = await setup({
      newId: (() => {
        let next = 0;
        return () => uuid((next += 1));
      })(),
      spaces: [{ name: "S", pages: [{ title: "Origin", content: reference }, { title: "Zucchini harvest" }, { title: "Hub" }] }],
    });
    const found = await client.aiRetrieve("zucchini", { type: "all" });
    expect(found.map((source) => source.title)).toContain("Zucchini harvest");
  });
});

describe("the semantic index (made up)", () => {
  it("counts live pages, and indexes pages by hand with advanceIndex", async () => {
    const { client, ai, events } = await aiOn();
    expect(await client.aiIndexStatus()).toEqual({ pagesIndexed: 0, pagesTotal: 4, chunks: 0, dirty: 4 });
    expect(await ai.advanceIndex(2)).toBe(2);
    const half = await client.aiIndexStatus();
    expect(half).toMatchObject({ pagesIndexed: 2, pagesTotal: 4, dirty: 2, model: "nomic-embed-text:latest", dimensions: 768 });
    expect(half.chunks).toBeGreaterThan(0);
    expect(events.filter((event) => event.type === "AiIndexChanged")).toHaveLength(1);
    expect(await ai.advanceIndex()).toBe(2);
    expect((await client.aiIndexStatus()).dirty).toBe(0);
    expect(await ai.advanceIndex()).toBe(0);
  });

  it("indexes nothing while AI, the semantic index or the embedding model is off", async () => {
    for (const settings of [
      { ...(ON as object), enabled: false },
      { ...(ON as object), semanticIndex: false },
      { ...(ON as object), embeddingModel: undefined },
    ] as JsonValue[]) {
      const { ai, client } = await aiOn({}, settings);
      expect(await ai.advanceIndex()).toBe(0);
      expect((await client.aiIndexStatus()).pagesIndexed).toBe(0);
    }
  });

  it("clears on AiIndexClear and says so", async () => {
    const { client, ai, events } = await aiOn();
    await ai.advanceIndex();
    events.length = 0;
    await client.aiIndexClear();
    expect(await client.aiIndexStatus()).toEqual({ pagesIndexed: 0, pagesTotal: 4, chunks: 0, dirty: 4 });
    expect(events).toContainEqual({ type: "AiIndexChanged" });
  });

  it("marks a page dirty when its content changes, and drops pages in the trash from the counts", async () => {
    const { client, ai, memory, garden, cooking } = await aiOn();
    await ai.advanceIndex();
    memory.changePage(garden, () => undefined);
    expect(await client.aiIndexStatus()).toMatchObject({ pagesIndexed: 3, pagesTotal: 4, dirty: 1 });
    await client.pageTrash(cooking);
    expect(await client.aiIndexStatus()).toMatchObject({ pagesIndexed: 2, pagesTotal: 3, dirty: 1 });
  });

  it("starts over when the embedding model changes", async () => {
    const { client, ai } = await aiOn();
    await ai.advanceIndex();
    await client.settingsSet(AI_SETTINGS_SETTING, { ...(ON as object), embeddingModel: "bge-m3" });
    expect((await client.aiIndexStatus()).pagesIndexed).toBe(0);
    await ai.advanceIndex();
    expect((await client.aiIndexStatus()).model).toBe("bge-m3");
  });

  it("indexes on a timer when the option gives a step", async () => {
    vi.useFakeTimers();
    const { client, events } = await aiOn({ ai: { mode: "fake", indexStepMs: 100, indexBatchPages: 3 } });
    expect((await client.aiIndexStatus()).pagesIndexed).toBe(0);
    await vi.advanceTimersByTimeAsync(100);
    expect((await client.aiIndexStatus()).pagesIndexed).toBe(3);
    await vi.advanceTimersByTimeAsync(100);
    expect((await client.aiIndexStatus()).pagesIndexed).toBe(4);
    expect(events.filter((event) => event.type === "AiIndexChanged")).toHaveLength(2);
    expect((await client.aiStatus()).indexer.phase).toBe("idle");
  });

  it("goes with the status: the indexer's last run", async () => {
    const { client, ai } = await aiOn({ now: () => 1234 });
    expect((await client.aiStatus()).indexer).toEqual({ phase: "idle" });
    await ai.advanceIndex(1);
    expect((await client.aiStatus()).indexer).toEqual({ phase: "idle", lastRunAt: 1234 });
    expect((await client.aiStatus()).index).toMatchObject({ pagesIndexed: 1 });
  });
});

describe("ai_status and ai_endpoint_set (fake)", () => {
  it("reports a reachable local Ollama with its models, and works with AI off", async () => {
    const { client } = await setup({ ai: { mode: "fake" } });
    expect(client.hasAi()).toBe(true);
    expect(client.hasMcp()).toBe(true);
    const status = await client.aiStatus();
    expect(status).toMatchObject({ endpoint: "http://127.0.0.1:11434", local: true, remoteConsent: false, reachable: true, version: "0.35.0" });
    expect(status.models.map((model) => model.name)).toEqual(FAKE_MODELS.map((model) => model.name));
    expect(status.models.find((model) => model.name === "qwen3:8b")?.capabilities).toContain("thinking");
    expect(status.error).toBeUndefined();
    expect(status.index?.pagesTotal).toBe(4);
  });

  it("says Ollama cannot be reached, with the message, until it can again", async () => {
    const { client, ai } = await aiOn();
    ai.setReachable(false, "connection refused");
    const down = await client.aiStatus();
    expect(down).toMatchObject({ reachable: false, models: [] });
    expect(down.error).toMatchObject({ code: "InvalidInput", message: "connection refused", details: { field: "endpoint", reason: "unreachable" } });
    ai.setReachable(true);
    expect((await client.aiStatus()).reachable).toBe(true);
  });

  it("lists the models a test installs", async () => {
    const { client, ai } = await aiOn();
    ai.setModels([{ name: "tiny", capabilities: ["completion"], size: 1 }]);
    expect((await client.aiStatus()).models.map((model) => model.name)).toEqual(["tiny"]);
  });

  it("stores a local endpoint without consent and announces the setting", async () => {
    const { client, events } = await aiOn();
    const status = await client.aiEndpointSet(" http://localhost:11500/ ");
    expect(status).toMatchObject({ endpoint: "http://localhost:11500", local: true, remoteConsent: false });
    expect(await client.settingsGet(AI_ENDPOINT_SETTING)).toEqual({ url: "http://localhost:11500", remoteConsent: false });
    expect(events).toContainEqual({ type: "SettingChanged", data: { key: AI_ENDPOINT_SETTING } });
  });

  it("allows a remote endpoint (the native dialog says Allow) and remembers the consent for that URL", async () => {
    const { client } = await aiOn();
    const status = await client.aiEndpointSet("https://ollama.example.com:8443");
    expect(status).toMatchObject({ endpoint: "https://ollama.example.com:8443", local: false, remoteConsent: true });
    expect(await client.settingsGet(AI_ENDPOINT_SETTING)).toEqual({ url: "https://ollama.example.com:8443", remoteConsent: true });
    // Back to a local one: no consent kept.
    expect((await client.aiEndpointSet("http://127.0.0.1:11434")).remoteConsent).toBe(false);
  });

  it("changes nothing when the native dialog is declined", async () => {
    const { client, ai } = await aiOn();
    ai.setDeclineConsent(true);
    const error = await failure(client.aiEndpointSet("http://192.168.1.5:11434"));
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "url", reason: "declined" } });
    expect((await client.aiStatus()).endpoint).toBe("http://127.0.0.1:11434");
    // A local one never asks.
    await expect(client.aiEndpointSet("http://localhost:11434")).resolves.toMatchObject({ local: true });
  });

  it("refuses endpoints that break the rules", async () => {
    const { client } = await aiOn();
    for (const url of [
      "",
      "ftp://host",
      "http://user:pass@host",
      "http://host/path",
      "http://host/?q=1",
      "http://host/#frag",
      "not a url",
      `http://${"a".repeat(600)}.com`,
    ]) {
      expect(await failure(client.aiEndpointSet(url))).toMatchObject({ code: "InvalidInput", details: { field: "url" } });
    }
  });

  it("classifies hosts: loopback is local, everything else (LAN, look-alikes) is remote", () => {
    for (const url of ["http://localhost:1", "http://127.0.0.1", "http://127.1.2.3:5", "http://[::1]:11434"]) {
      expect(normaliseEndpoint(url).local, url).toBe(true);
    }
    for (const url of ["http://localhost.evil.com", "http://192.168.1.2", "http://10.0.0.1", "https://example.com", "http://128.0.0.1"]) {
      expect(normaliseEndpoint(url).local, url).toBe(false);
    }
    expect(normaliseEndpoint("http://127.0.0.1:11434/").url).toBe("http://127.0.0.1:11434");
    expect(normaliseEndpoint("http://[::1]:11434").url).toBe("http://[::1]:11434");
  });

  it("starts from a stored endpoint, and refuses calls to a remote one without consent", async () => {
    const { client } = await aiOn({ settings: { [AI_SETTINGS_SETTING]: ON, [AI_ENDPOINT_SETTING]: { url: "https://far.example.com", remoteConsent: false } } });
    const status = await client.aiStatus();
    expect(status).toMatchObject({ endpoint: "https://far.example.com", local: false, remoteConsent: false, reachable: false });
    expect(status.error).toMatchObject({ details: { field: "endpoint" } });
    const error = await failure(client.aiGenerate("r1", improve("hello"), () => undefined));
    expect(error).toMatchObject({ code: "InvalidInput", message: "consent required", details: { field: "endpoint" } });
  });
});

describe("AI off", () => {
  it("answers InvalidInput for `enabled` to everything but the status and the endpoint", async () => {
    const { client } = await aiOn({}, { enabled: false });
    const refused = [
      client.aiGenerate("r1", improve("x"), () => undefined),
      client.aiChat("r2", ask("hello"), () => undefined),
      client.aiSearch("hello", { type: "all" }),
      client.aiTranscribe(encodeWav(new Int16Array(10))),
    ];
    for (const promise of refused) {
      expect(await failure(promise)).toMatchObject({ code: "InvalidInput", message: "AI is turned off", details: { field: "enabled" } });
    }
    await expect(client.aiStatus()).resolves.toMatchObject({ reachable: true });
    await expect(client.aiEndpointSet("http://127.0.0.1:11434")).resolves.toBeDefined();
  });
});

describe("ai_generate (fake)", () => {
  it("streams deltas then done, and the improved text is the input in capitals", async () => {
    const { client } = await aiOn();
    const events = await stream((onEvent) => client.aiGenerate("req-1", improve("fix this old text please"), onEvent));
    expect(events.length).toBeGreaterThanOrEqual(3);
    expect(events.slice(0, -1).every((event) => event.type === "delta")).toBe(true);
    expect(text(events)).toBe("FIX THIS OLD TEXT PLEASE");
    expect(events.at(-1)).toEqual({ type: "done", text: "FIX THIS OLD TEXT PLEASE", model: "qwen3:8b" });
  });

  it("answers each action deterministically", async () => {
    const { client } = await aiOn();
    const run = async (input: AiGenerateInput) => {
      const events = await stream((onEvent) => client.aiGenerate("r", input, onEvent));
      const done = events.at(-1);
      return done?.type === "done" ? done.text : "";
    };
    expect(await run({ action: "fixGrammar", text: "a  b   c" })).toBe("a b c");
    expect(await run({ action: "shorter", text: "one two three four" })).toBe("one two");
    expect(await run({ action: "longer", text: "x" })).toBe("x\n\nx");
    expect(await run({ action: "tone", text: "hi", tone: "casual" })).toBe("[casual] hi");
    expect(await run({ action: "translate", text: "hi", language: "French" })).toBe("[French] hi");
    expect(await run({ action: "custom", text: "abc", prompt: "Reverse" })).toBe("Reverse: abc");
    expect(await run({ action: "actionItems", text: "call Ann\nbuy milk" })).toBe("- [ ] call Ann\n- [ ] buy milk");
    expect(await run({ action: "continue", text: "Once" })).toBe("Once ... and so on.");
    expect(await run({ action: "continue", text: "" })).toBe("... and so on.");
    expect(await run({ action: "meetingSummary", text: "talk" })).toContain("### Action items");
    expect(await run({ action: "summarize", text: "long" })).toContain("- Key point one");
    expect(await run({ action: "explain", text: "it" })).toBe("This means: it");
    expect(await run({ action: "simplify", text: "it" })).toBe("Simply put: it");
  });

  it("checks the limits of section 10 before any stream", async () => {
    const { client } = await aiOn();
    const reject = (input: AiGenerateInput, id = "r1") => failure(client.aiGenerate(id, input, () => undefined));
    expect(await reject(improve("x".repeat(32_001)))).toMatchObject({ details: { field: "text", limit: 32_000 } });
    expect(await reject({ ...improve("x"), context: "c".repeat(8_001) })).toMatchObject({ details: { field: "context" } });
    expect(await reject({ action: "custom", text: "", prompt: "p".repeat(2_001) })).toMatchObject({ details: { field: "prompt" } });
    expect(await reject({ action: "custom", text: "x" })).toMatchObject({ details: { field: "prompt" } });
    expect(await reject({ action: "translate", text: "x" })).toMatchObject({ details: { field: "language" } });
    expect(await reject({ action: "tone", text: "x" })).toMatchObject({ details: { field: "tone" } });
    expect(await reject(improve("   "))).toMatchObject({ details: { field: "text" } });
    expect(await reject(improve("x"), "")).toMatchObject({ details: { field: "requestId" } });
    expect(await reject(improve("x"), "no spaces")).toMatchObject({ details: { field: "requestId" } });
    expect(await reject(improve("x"), "a".repeat(65))).toMatchObject({ details: { field: "requestId" } });
    await expect(client.aiGenerate("a".repeat(64), improve("x"), () => undefined)).resolves.toBeUndefined();
    await expect(client.aiGenerate("ok_id-9", { action: "continue", text: "" }, () => undefined)).resolves.toBeUndefined();
    // Exactly at the limit is fine.
    await expect(client.aiGenerate("r2", improve("x".repeat(32_000)), () => undefined)).resolves.toBeUndefined();
  });

  it("needs a chat model", async () => {
    const { client } = await aiOn({}, { enabled: true });
    const error = await failure(client.aiGenerate("r1", improve("x"), () => undefined));
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "model", reason: "missing" } });
  });

  it("cancels in the middle: the last event is an error with reason cancelled", async () => {
    const { client, ai } = await aiOn();
    const release = ai.holdStreams(1);
    const events: AiStreamEvent[] = [];
    const running = client.aiGenerate("stop-me", improve("one two three four five six seven eight"), (event) => events.push(event));
    await vi.waitFor(() => {
      expect(events.some((event) => event.type === "delta")).toBe(true);
    });
    await client.aiCancel("stop-me");
    await running;
    release();
    expect(events.at(-1)).toMatchObject({ type: "error", error: { code: "InvalidInput", details: { field: "endpoint", reason: "cancelled" } } });
    expect(events.filter((event) => event.type === "delta")).toHaveLength(1);
    expect(events.some((event) => event.type === "done")).toBe(false);
  });

  it("allows four streams at once and refuses the fifth as busy; an id is used once", async () => {
    const { client, ai } = await aiOn();
    const release = ai.holdStreams(0);
    const running = ["a", "b", "c", "d"].map((id) => client.aiGenerate(id, improve("hello world"), () => undefined));
    await vi.waitFor(async () => {
      // Wait until all four are registered: the fifth is refused.
      const error = await failure(client.aiGenerate("e", improve("x"), () => undefined));
      expect(error).toMatchObject({ details: { field: "requestId", reason: "busy" } });
    });
    expect(await failure(client.aiGenerate("a", improve("x"), () => undefined))).toMatchObject({ details: { field: "requestId" } });
    release();
    await Promise.all(running);
    await expect(client.aiGenerate("e", improve("x"), () => undefined)).resolves.toBeUndefined();
  });

  it("ends a stream with an error event a test injects, after the first delta", async () => {
    const { client, ai } = await aiOn();
    ai.failNextStream({ code: "InvalidInput", message: "model is gone", details: { field: "endpoint", reason: "model" } });
    const events = await stream((onEvent) => client.aiGenerate("r1", improve("alpha beta gamma delta"), onEvent));
    expect(events.map((event) => event.type)).toEqual(["delta", "error"]);
    // The next one is fine again.
    const again = await stream((onEvent) => client.aiGenerate("r2", improve("alpha"), onEvent));
    expect(again.at(-1)?.type).toBe("done");
  });

  it("rejects the next calls a test injects, before any stream", async () => {
    const { client, ai } = await aiOn();
    ai.failNext({ code: "Storage", message: "boom" });
    expect(await failure(client.aiGenerate("r1", improve("x"), () => undefined))).toMatchObject({ code: "Storage" });
    await expect(client.aiGenerate("r1", improve("x"), () => undefined)).resolves.toBeUndefined();
  });

  it("records the calls", async () => {
    const { client, ai } = await aiOn();
    await client.aiGenerate("r1", improve("hi"), () => undefined);
    expect(ai.calls.at(-1)).toEqual({ kind: "generate", input: improve("hi") });
  });
});

describe("ai_chat (fake)", () => {
  it("sends the sources first, then the answer that cites [1]", async () => {
    const { client, garden } = await aiOn();
    const events = await stream((onEvent) => client.aiChat("chat-1", ask("When should I water the tomatoes?"), onEvent));
    expect(events[0]?.type).toBe("sources");
    const sources = events[0]?.type === "sources" ? events[0].sources : [];
    expect(sources[0]).toMatchObject({ n: 1, pageId: garden });
    const done = events.at(-1);
    expect(done?.type).toBe("done");
    expect(done?.type === "done" ? done.text : "").toContain("[1]");
    expect(text(events)).toBe(done?.type === "done" ? done.text : "x");
    expect(events.filter((event) => event.type === "delta").length).toBeGreaterThanOrEqual(2);
  });

  it("answers plainly when nothing was found", async () => {
    const { client } = await aiOn();
    const events = await stream((onEvent) => client.aiChat("chat-1", ask("zebras quokkas"), onEvent));
    expect(events[0]).toEqual({ type: "sources", sources: [] });
    expect(text(events)).toContain("could not find");
  });

  it("limits retrieval to the scope, and a page scope gives that whole page as source 1", async () => {
    const { client, work, garden } = await aiOn();
    const inWork = await stream((onEvent) => client.aiChat("c1", ask("tomato", { type: "space", spaceId: work }), onEvent));
    const workSources = inWork[0]?.type === "sources" ? inWork[0].sources : [];
    expect(workSources.map((source) => source.title)).toEqual(["Roadmap"]);
    const onPage = await stream((onEvent) => client.aiChat("c2", ask("anything at all about plants", { type: "page", pageId: garden }), onEvent));
    const pageSources = onPage[0]?.type === "sources" ? onPage[0].sources : [];
    expect(pageSources[0]).toMatchObject({ n: 1, pageId: garden });
    expect(pageSources[0]?.text).toContain("Compost the leaves");
    expect(pageSources[0]?.text).toContain("Tomatoes need full sun");
  });

  it("ends with an error event for a scope that no longer exists", async () => {
    const { client } = await aiOn();
    const events = await stream((onEvent) => client.aiChat("c1", ask("tomato", { type: "page", pageId: "gone" }), onEvent));
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ type: "error", error: { code: "NotFound" } });
  });

  it("uses the previous question for a follow-up", async () => {
    const { client } = await aiOn();
    const events = await stream((onEvent) =>
      client.aiChat(
        "c1",
        {
          messages: [
            { role: "user", content: "Tell me about the roadmap" },
            { role: "assistant", content: "It ships in spring." },
            { role: "user", content: "and then?" },
          ],
          scope: { type: "all" },
        },
        onEvent,
      ),
    );
    const sources = events[0]?.type === "sources" ? events[0].sources : [];
    expect(sources[0]?.title).toBe("Roadmap");
  });

  it("checks the limits: messages, lengths, the last one from the person", async () => {
    const { client } = await aiOn();
    const reject = (input: AiChatInput) => failure(client.aiChat("r1", input, () => undefined));
    const user = (content: string) => ({ role: "user" as const, content });
    expect(await reject({ messages: [], scope: { type: "all" } })).toMatchObject({ details: { field: "messages" } });
    expect(await reject({ messages: Array.from({ length: 41 }, () => user("hi")), scope: { type: "all" } })).toMatchObject({ details: { field: "messages", limit: 40 } });
    expect(await reject({ messages: [user("hi"), { role: "assistant", content: "yo" }], scope: { type: "all" } })).toMatchObject({ details: { field: "messages" } });
    expect(await reject(ask("x".repeat(16_001)))).toMatchObject({ details: { field: "messages", limit: 16_000 } });
    expect(
      await reject({ messages: [user("x".repeat(16_000)), { role: "assistant", content: "y".repeat(16_000) }, user("z".repeat(16_000)), { role: "assistant", content: "w".repeat(16_000) }, user("v")], scope: { type: "all" } }),
    ).toMatchObject({ details: { field: "messages", limit: 64_000 } });
    expect(await reject(ask("   "))).toMatchObject({ details: { field: "messages" } });
  });
});

describe("ai_search (fake)", () => {
  it("returns related passages for the dialog, limited", async () => {
    const { client } = await aiOn();
    const found = await client.aiSearch("tomato", { type: "all" }, 2);
    expect(found).toHaveLength(2);
    expect(found[0]?.n).toBe(1);
    expect(await client.aiSearch("zebras", { type: "all" })).toEqual([]);
    expect(await failure(client.aiSearch("  ", { type: "all" }))).toMatchObject({ details: { field: "query" } });
  });
});

describe("ai_transcribe (fake)", () => {
  const wav = (seconds: number) => encodeWav(new Int16Array(Math.round(seconds * 16000)));

  it("answers a sentence naming the clip's length, and the language", async () => {
    const { client } = await aiOn();
    expect(await client.aiTranscribe(wav(2.5))).toEqual({ text: "Transcript of a 2.5 second clip.", language: "English" });
  });

  it("needs a model: the setting's or the argument", async () => {
    const { client } = await aiOn({}, { enabled: true, chatModel: "qwen3:8b" });
    expect(await failure(client.aiTranscribe(wav(1)))).toMatchObject({ details: { field: "model", reason: "missing" } });
    await expect(client.aiTranscribe(wav(1), "r2t2")).resolves.toBeDefined();
  });

  it("wants a WAV of at most 4 MiB", async () => {
    const { client } = await aiOn();
    expect(await failure(client.aiTranscribe(new Uint8Array(100)))).toMatchObject({ details: { field: "audio" } });
    const big = new Uint8Array(4 * 1024 * 1024 + 1);
    big.set(wav(0).subarray(0, 44));
    expect(await failure(client.aiTranscribe(big))).toMatchObject({ details: { field: "audio", limit: 4 * 1024 * 1024 } });
  });

  it("uses the transcriber of a test", async () => {
    const { client, ai } = await aiOn();
    ai.setTranscriber((_bytes, model) => ({ text: `heard by ${model ?? "default"}`, language: "French" }));
    expect(await client.aiTranscribe(wav(1), "m1")).toEqual({ text: "heard by m1", language: "French" });
    ai.setTranscriber(() => "just text");
    expect((await client.aiTranscribe(wav(1))).text).toBe("just text");
  });
});

describe("MCP status and the lock", () => {
  it("is not running, with the port and URL of the setting, and the bridge when the option gives one", async () => {
    const { client } = await aiOn({ settings: { [AI_SETTINGS_SETTING]: ON, [MCP_SETTINGS_SETTING]: { enabled: true, port: 50123 } }, ai: { mode: "fake", bridgePath: "C:\\Apps\\kb-mcp-bridge.exe" } });
    expect(await client.mcpStatus()).toEqual({
      enabled: true,
      running: false,
      port: 50123,
      url: "http://127.0.0.1:50123/mcp",
      bridgePath: "C:\\Apps\\kb-mcp-bridge.exe",
    });
  });

  it("defaults to off on port 47811, and a test can make it run or fail", async () => {
    const { client, ai } = await aiOn();
    expect(await client.mcpStatus()).toEqual({ enabled: false, running: false, port: 47811, url: "http://127.0.0.1:47811/mcp" });
    ai.setMcpStatus({ running: true });
    expect((await client.mcpStatus()).running).toBe(true);
    ai.setMcpStatus({ running: false, error: "port in use" });
    expect(await client.mcpStatus()).toMatchObject({ running: false, error: "port in use" });
  });

  it("rejects with Locked while the core is locked, and ends streams when it locks", async () => {
    const { client, memory, ai } = await aiOn({ security: { appLockPassphrase: "correct horse battery", kitCreatedAt: 1, kitKey: new Uint8Array(32).fill(3) } });
    const release = ai.holdStreams(1);
    const events: AiStreamEvent[] = [];
    const running = client.aiChat("c1", ask("tomato"), (event) => events.push(event));
    await vi.waitFor(() => {
      expect(events.some((event) => event.type === "delta")).toBe(true);
    });
    await memory.lock();
    await running;
    release();
    expect(events.at(-1)).toMatchObject({ type: "error", error: { details: { reason: "cancelled" } } });
    expect(await failure(client.aiStatus())).toMatchObject({ code: "Locked" });
    expect(await failure(client.mcpStatus())).toMatchObject({ code: "Locked" });
    expect(await failure(client.aiGenerate("r", improve("x"), () => undefined))).toMatchObject({ code: "Locked" });
    await memory.unlock("correct horse battery");
    await expect(client.aiStatus()).resolves.toMatchObject({ reachable: true });
  });
});
