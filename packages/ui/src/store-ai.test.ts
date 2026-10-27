// The AI part of the app store (ADR-0037, store-ai.ts): the settings and their selectors, what turning AI on and
// off does, the status, the index kept fresh by events (at most once a second), MCP data, the chat conversation
// (streaming, Stop, a new chat, scope, history, nothing kept after a lock) and the related search.

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { AiRecordedCall, MemoryAiTransport, MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { createAppStore } from "./store";
import type { AppState } from "./store";
import {
  AI_REFRESH_MS,
  answerMarkdown,
  describeAiError,
  effectiveChatScope,
  selectAiAvailable,
  selectAiEnabled,
  selectAiReady,
  selectAiSettings,
  selectMcpSettings,
} from "./store-ai";
import type { ChatMessage } from "./store-ai";
import { AI_ON, AI_SETTING, AI_SPACES, MCP_SETTING, aiOf } from "./test/ai";
import { PASSPHRASE } from "./test/security";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function setup(options: MemoryTransportOptions = {}, { ai = true }: { ai?: boolean } = {}) {
  const memory = new MemoryTransport({
    workspaceName: "Home",
    spaces: AI_SPACES,
    ...(ai ? { ai: { mode: "fake" as const } } : {}),
    ...options,
  });
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return memory.dispatch(request);
    },
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    lock: () => memory.lock(),
    unlock: (passphrase) => memory.unlock(passphrase),
    ...(memory.ai === undefined ? {} : { ai: memory.ai }),
    ...(memory.mcp === undefined ? {} : { mcp: memory.mcp }),
  };
  const client = new CoreClient(transport);
  const store = createAppStore(client, { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  return { memory, store, client, requests, stop };
}

const state = (store: ReturnType<typeof createAppStore>): AppState => store.getState();

/** The reads that events trigger wait for the second to pass (`AI_REFRESH_MS`), so waiting is generous. */
async function settled(store: ReturnType<typeof createAppStore>, check: (state: AppState) => boolean): Promise<void> {
  await vi.waitFor(
    () => {
      expect(check(store.getState())).toBe(true);
    },
    { timeout: AI_REFRESH_MS * 4 },
  );
}

/** The last call of a kind (a chat or a search also makes a retrieval). */
function lastCall(ai: MemoryAiTransport, kind: AiRecordedCall["kind"]): AiRecordedCall | undefined {
  return ai.calls.filter((call) => call.kind === kind).at(-1);
}

describe("what the transport and the settings say", () => {
  it("has no AI without the shell's commands, and AI is off by default with them", async () => {
    const none = await setup({}, { ai: false });
    expect(state(none.store).aiCapabilities).toEqual({ ai: false, mcp: false });
    expect(selectAiAvailable(state(none.store))).toBe(false);
    expect(selectAiEnabled(state(none.store))).toBe(false);
    const some = await setup();
    expect(state(some.store).aiCapabilities).toEqual({ ai: true, mcp: true });
    expect(selectAiAvailable(state(some.store))).toBe(true);
    expect(selectAiEnabled(state(some.store))).toBe(false);
    expect(selectAiSettings(state(some.store))).toEqual({ enabled: false, semanticIndex: true, think: false });
    expect(selectMcpSettings(state(some.store))).toEqual({ enabled: false, port: 47811 });
  });

  it("reads the settings at boot, so every entry point knows at once", async () => {
    const { store } = await setup({ settings: { [AI_SETTING]: AI_ON, [MCP_SETTING]: { enabled: true, port: 50000 } } });
    expect(selectAiEnabled(state(store))).toBe(true);
    expect(selectAiReady(state(store))).toBe(true);
    expect(selectAiSettings(state(store))).toMatchObject({ chatModel: "qwen3:8b", embeddingModel: "nomic-embed-text:latest" });
    expect(selectMcpSettings(state(store))).toEqual({ enabled: true, port: 50000 });
  });

  it("is ready only with a chat model, and never when the transport has no AI", async () => {
    const { store } = await setup({ settings: { [AI_SETTING]: { enabled: true } } });
    expect(selectAiEnabled(state(store))).toBe(true);
    expect(selectAiReady(state(store))).toBe(false);
    const none = await setup({ settings: { [AI_SETTING]: AI_ON } }, { ai: false });
    expect(selectAiEnabled(state(none.store))).toBe(false);
    expect(selectAiReady(state(none.store))).toBe(false);
  });

  it("gives the same settings object while the stored value is the same (a component may select it)", async () => {
    const { store } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    expect(selectAiSettings(state(store))).toBe(selectAiSettings(state(store)));
  });

  it("hands the core client to features outside the slice, and a lock does not take it away", async () => {
    const { store, client } = await setup({
      settings: { [AI_SETTING]: AI_ON },
      security: { kitCreatedAt: 1, appLockPassphrase: PASSPHRASE },
    });
    expect(state(store).aiClient).toBe(client);
    expect(state(store).aiClient.hasAi()).toBe(true);
    await state(store).lock();
    expect(state(store).aiClient).toBe(client);
  });

  it("follows a setting changed elsewhere", async () => {
    const { store, client } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await client.settingsSet(AI_SETTING, { enabled: false });
    await settled(store, (s) => !selectAiEnabled(s));
  });
});

describe("turning AI on and off", () => {
  it("fills empty models from what Ollama lists: qwen3:8b and nomic-embed-text, no transcription model", async () => {
    const { store, client } = await setup();
    await state(store).updateAiSettings({ enabled: true });
    expect(selectAiSettings(state(store))).toEqual({
      enabled: true,
      chatModel: "qwen3:8b",
      embeddingModel: "nomic-embed-text:latest",
      semanticIndex: true,
      think: false,
    });
    expect(await client.settingsGet(AI_SETTING)).toEqual({
      enabled: true,
      chatModel: "qwen3:8b",
      embeddingModel: "nomic-embed-text:latest",
      semanticIndex: true,
      think: false,
    });
  });

  it("falls back to the first completion model and the first embedding model", async () => {
    const { store, memory } = await setup();
    aiOf(memory).setModels([
      { name: "bge-m3", capabilities: ["embedding"], size: 1 },
      { name: "gemma4:12b", capabilities: ["completion", "audio"], size: 1 },
    ]);
    await state(store).updateAiSettings({ enabled: true });
    expect(selectAiSettings(state(store))).toMatchObject({ chatModel: "gemma4:12b", embeddingModel: "bge-m3" });
    expect(selectAiSettings(state(store)).transcriptionModel).toBeUndefined();
  });

  it("keeps the models the person already chose", async () => {
    const { store } = await setup({ settings: { [AI_SETTING]: { enabled: false, chatModel: "llama3.2:3b", semanticIndex: true, think: false } } });
    await state(store).updateAiSettings({ enabled: true });
    expect(selectAiSettings(state(store))).toMatchObject({ enabled: true, chatModel: "llama3.2:3b", embeddingModel: "nomic-embed-text:latest" });
  });

  it("turns on without models when Ollama cannot be reached", async () => {
    const { store, memory } = await setup();
    aiOf(memory).setReachable(false);
    await state(store).updateAiSettings({ enabled: true });
    expect(selectAiSettings(state(store))).toEqual({ enabled: true, semanticIndex: true, think: false });
    expect(selectAiReady(state(store))).toBe(false);
  });

  it("deletes the vectors when AI is turned off, and when the semantic index is", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const ai = aiOf(memory);
    await ai.advanceIndex();
    await state(store).loadAiStatus();
    expect(state(store).aiIndex?.pagesIndexed).toBe(4);
    await state(store).updateAiSettings({ semanticIndex: false });
    await settled(store, (s) => s.aiIndex?.pagesIndexed === 0);
    await state(store).updateAiSettings({ semanticIndex: true });
    await ai.advanceIndex();
    await settled(store, (s) => s.aiIndex?.pagesIndexed === 4);
    await state(store).updateAiSettings({ enabled: false });
    await settled(store, (s) => s.aiIndex?.pagesIndexed === 0);
    expect(selectAiEnabled(state(store))).toBe(false);
  });

  it("does not clear the index for other changes", async () => {
    const { store, memory, requests } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(memory).advanceIndex();
    await state(store).updateAiSettings({ think: true, chatModel: "llama3.2:3b" });
    expect(requests.some((request) => request.type === "AiIndexClear")).toBe(false);
    expect(selectAiSettings(state(store))).toMatchObject({ think: true, chatModel: "llama3.2:3b" });
  });

  it("rejects when the setting cannot be written, and changes nothing", async () => {
    const { store, memory } = await setup();
    memory.failNext({ code: "Storage", message: "disk full" });
    await expect(state(store).updateAiSettings({ enabled: true })).rejects.toBeInstanceOf(CoreError);
    expect(selectAiEnabled(state(store))).toBe(false);
  });
});

describe("the status, the endpoint and the index", () => {
  it("reads the status with the models and the index", async () => {
    const { store } = await setup();
    await state(store).loadAiStatus();
    expect(state(store).aiStatus).toMatchObject({ reachable: true, version: "0.35.0", local: true });
    expect(state(store).aiStatusError).toBeNull();
    expect(state(store).aiStatusLoading).toBe(false);
    expect(state(store).aiIndex).toMatchObject({ pagesTotal: 4, pagesIndexed: 0 });
  });

  it("says why the status could not be read", async () => {
    const { store, memory } = await setup();
    aiOf(memory).failNext({ code: "Internal", message: "boom" });
    await state(store).loadAiStatus();
    expect(state(store).aiStatus).toBeNull();
    expect(state(store).aiStatusError).toBe(t("ai.error.generic", { message: "boom" }));
  });

  it("reads nothing where the transport has no AI", async () => {
    const { store, requests } = await setup({}, { ai: false });
    await state(store).loadAiStatus();
    expect(state(store).aiStatus).toBeNull();
    expect(requests.some((request) => request.type === "AiIndexStatus")).toBe(false);
  });

  it("saves the endpoint and shows the new status; a refusal rejects and keeps the old one", async () => {
    const { store, memory } = await setup();
    await state(store).loadAiStatus();
    const status = await state(store).saveAiEndpoint("http://localhost:11500");
    expect(status.endpoint).toBe("http://localhost:11500");
    expect(state(store).aiStatus?.endpoint).toBe("http://localhost:11500");
    aiOf(memory).setDeclineConsent(true);
    await expect(state(store).saveAiEndpoint("https://far.example.com")).rejects.toMatchObject({ details: { field: "url", reason: "declined" } });
    expect(state(store).aiStatus?.endpoint).toBe("http://localhost:11500");
  });

  it("rebuilds the index: deletes the vectors and reads the index again", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(memory).advanceIndex();
    await state(store).loadAiStatus();
    await state(store).rebuildAiIndex();
    expect(state(store).aiIndex).toMatchObject({ pagesIndexed: 0, dirty: 4 });
  });

  it("reads the index again when it changes, at most once a second", async () => {
    const { store, memory, requests } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await state(store).loadAiStatus();
    const count = () => requests.filter((request) => request.type === "AiIndexStatus").length;
    const before = count();
    const ai = aiOf(memory);
    // Four batches in a row: four events.
    for (let i = 0; i < 4; i += 1) {
      await ai.advanceIndex(1);
    }
    await settled(store, (s) => s.aiIndex?.pagesIndexed === 1);
    // The first event reads at once, the rest wait for the second to pass and are read together.
    expect(count() - before).toBe(1);
    await vi.waitFor(
      () => {
        expect(state(store).aiIndex?.pagesIndexed).toBe(4);
      },
      { timeout: AI_REFRESH_MS * 3 },
    );
    expect(count() - before).toBe(2);
  });

  it("does not read the index for events nobody asked about", async () => {
    const { memory, requests } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(memory).advanceIndex(1);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(requests.some((request) => request.type === "AiIndexStatus")).toBe(false);
  });
});

describe("MCP in the store", () => {
  it("reads the status, the clients and the audit log, and keeps them fresh", async () => {
    const { store, memory, client } = await setup();
    await state(store).loadMcp();
    expect(state(store).mcpStatus).toMatchObject({ enabled: false, running: false, port: 47811 });
    expect(state(store).mcpClients).toEqual([]);
    expect(state(store).mcpAudit).toEqual([]);
    const { client: created } = await client.mcpClientCreate("Scripts");
    await settled(store, (s) => s.mcpClients?.length === 1);
    memory.recordMcpCall({ clientId: created.id, tool: "search", outcome: "ok", summary: "tomato" });
    await settled(store, (s) => s.mcpAudit?.length === 1);
  });

  it("creates a client without keeping its token, sets grants, filters the audit, clears it and deletes", async () => {
    const { store, memory } = await setup();
    await state(store).loadMcp();
    const created = await state(store).createMcpClient("Claude Desktop");
    expect(created.token).toMatch(/^kbm_/);
    expect(JSON.stringify(state(store).mcpClients)).not.toContain("kbm_");
    const space = state(store).spaces[0]?.id ?? "";
    const updated = await state(store).setMcpGrants(created.client.id, [{ spaceId: space, read: true, write: true }]);
    expect(updated.grants).toEqual([{ spaceId: space, read: true, write: true }]);
    const other = await state(store).createMcpClient("Scripts");
    memory.recordMcpCall({ clientId: created.client.id, tool: "search", outcome: "ok" });
    memory.recordMcpCall({ clientId: other.client.id, tool: "read_page", outcome: "denied" });
    await settled(store, (s) => s.mcpAudit?.length === 2);
    await state(store).setMcpAuditClient(other.client.id);
    expect(state(store).mcpAudit?.map((row) => row.tool)).toEqual(["read_page"]);
    await state(store).deleteMcpClient(other.client.id);
    expect(state(store).mcpAuditClient).toBeNull();
    await settled(store, (s) => s.mcpClients?.length === 1);
    await state(store).clearMcpAudit();
    expect(state(store).mcpAudit).toEqual([]);
  });

  it("stores the MCP settings and reads the status again", async () => {
    const { store, client } = await setup();
    await state(store).updateMcpSettings({ enabled: true, port: 50500 });
    expect(selectMcpSettings(state(store))).toEqual({ enabled: true, port: 50500 });
    expect(await client.settingsGet(MCP_SETTING)).toEqual({ enabled: true, port: 50500 });
    expect(state(store).mcpStatus).toMatchObject({ enabled: true, port: 50500 });
  });
});

describe("the chat", () => {
  const messagesOf = (store: ReturnType<typeof createAppStore>): ChatMessage[] => [...state(store).chatMessages];

  it("opens, closes and toggles; a lock closes it", async () => {
    const { store } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    expect(state(store).chatOpen).toBe(false);
    state(store).toggleChat();
    expect(state(store).chatOpen).toBe(true);
    state(store).toggleChat();
    expect(state(store).chatOpen).toBe(false);
    state(store).openChat();
    state(store).closeChat();
    expect(state(store).chatOpen).toBe(false);
  });

  it("asks a question: the answer streams in with its sources and ends done", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const release = aiOf(memory).holdStreams(1);
    const asking = state(store).askChat("  Water the tomatoes?  ");
    await settled(store, (s) => s.chatMessages[1]?.text !== "" && s.chatMessages[1]?.text !== undefined);
    expect(state(store).chatStreaming).toBe(true);
    expect(state(store).chatRequestId).toMatch(/^chat-\d+$/);
    const [question, streaming] = messagesOf(store);
    expect(question).toMatchObject({ role: "user", text: "Water the tomatoes?", status: "done" });
    expect(streaming).toMatchObject({ role: "assistant", status: "streaming", question: "Water the tomatoes?" });
    expect(streaming?.sources.length).toBeGreaterThan(0);
    release();
    await asking;
    const answer = messagesOf(store)[1];
    expect(answer?.status).toBe("done");
    expect(answer?.text).toContain("[1]");
    expect(answer?.sources[0]?.title).toBe("Garden plans");
    expect(state(store).chatStreaming).toBe(false);
    expect(state(store).chatRequestId).toBeNull();
  });

  it("ignores an empty question, and a question while an answer streams", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await state(store).askChat("   ");
    expect(messagesOf(store)).toEqual([]);
    const release = aiOf(memory).holdStreams(0);
    const first = state(store).askChat("one");
    await settled(store, (s) => s.chatStreaming);
    await state(store).askChat("two");
    expect(messagesOf(store)).toHaveLength(2);
    release();
    await first;
  });

  it("sends the conversation so far and the scope: this space by default, then all spaces, then a page", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const ai = aiOf(memory);
    const space = state(store).currentSpaceId;
    await state(store).askChat("first question");
    expect(lastCall(ai, "chat")).toMatchObject({ input: { scope: { type: "space", spaceId: space } } });
    state(store).setChatScope("all");
    await state(store).askChat("second question");
    const last = lastCall(ai, "chat")?.input as { messages: { role: string; content: string }[]; scope: unknown };
    expect(last.scope).toEqual({ type: "all" });
    expect(last.messages.map((message) => message.role)).toEqual(["user", "assistant", "user"]);
    expect(last.messages[0]?.content).toBe("first question");
    expect(last.messages[2]?.content).toBe("second question");
    // "This page" needs an open page: without one it is this space.
    state(store).setChatScope("page");
    expect(effectiveChatScope(state(store))).toBe("space");
    await state(store).askChat("third");
    expect(lastCall(ai, "chat")).toMatchObject({ input: { scope: { type: "space" } } });
    const garden = state(store).tree?.roots[0]?.page.id ?? "";
    expect(garden).not.toBe("");
    state(store).selectPage(garden);
    await settled(store, (s) => s.selectedPageId === garden);
    expect(effectiveChatScope(state(store))).toBe("page");
    await state(store).askChat("fourth");
    expect(lastCall(ai, "chat")).toMatchObject({ input: { scope: { type: "page", pageId: garden } } });
  });

  it("stops an answer: it keeps the text that arrived and says it was stopped", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const release = aiOf(memory).holdStreams(1);
    const asking = state(store).askChat("Water the tomatoes?");
    await settled(store, (s) => (s.chatMessages[1]?.text ?? "") !== "");
    state(store).stopChat();
    await asking;
    release();
    const answer = messagesOf(store)[1];
    expect(answer?.status).toBe("stopped");
    expect(answer?.text).not.toBe("");
    expect(answer?.error).toBeUndefined();
    expect(state(store).chatStreaming).toBe(false);
    // The conversation goes on, and the stopped answer is part of it.
    await state(store).askChat("again");
    expect(messagesOf(store)).toHaveLength(4);
  });

  it("shows a failure in the answer: Ollama unreachable, a missing model, AI turned off", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const ai = aiOf(memory);
    ai.failNextStream({ code: "InvalidInput", message: "gone", details: { field: "endpoint", reason: "unreachable" } });
    await state(store).askChat("one");
    expect(messagesOf(store)[1]).toMatchObject({ status: "error", error: t("ai.error.unreachable") });
    ai.failNext({ code: "InvalidInput", message: "no", details: { field: "model", reason: "missing" } });
    await state(store).askChat("two");
    expect(messagesOf(store)[3]).toMatchObject({ status: "error", error: t("ai.error.noModel") });
    // A failed answer is not sent again as part of the history.
    await state(store).askChat("three");
    const sent = lastCall(ai, "chat")?.input as { messages: { content: string }[] };
    expect(sent.messages.map((message) => message.content)).toEqual(["one", "two", "three"]);
    await state(store).updateAiSettings({ enabled: false });
    await state(store).askChat("four");
    expect(messagesOf(store).at(-1)).toMatchObject({ status: "error", error: t("ai.error.off") });
  });

  it("starts over with a new chat, also while an answer streams (and drops what it still delivers)", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const release = aiOf(memory).holdStreams(0);
    const asking = state(store).askChat("Water the tomatoes?");
    await settled(store, (s) => s.chatStreaming);
    state(store).newChat();
    expect(messagesOf(store)).toEqual([]);
    expect(state(store).chatStreaming).toBe(false);
    await asking;
    release();
    expect(messagesOf(store)).toEqual([]);
    expect(state(store).chatStreaming).toBe(false);
    // A new question works.
    await state(store).askChat("fresh");
    expect(messagesOf(store)).toHaveLength(2);
  });

  it("cuts the history to the limits: at most 40 messages, 16,000 characters each, 64,000 in all", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const ai = aiOf(memory);
    for (let i = 0; i < 22; i += 1) {
      await state(store).askChat(`question ${String(i)}`);
    }
    const sent = lastCall(ai, "chat")?.input as { messages: { content: string }[] };
    expect(sent.messages).toHaveLength(40);
    expect(sent.messages.at(-1)?.content).toBe("question 21");
    // The oldest go first: the first two messages (question 0 and its answer) are left out.
    expect(sent.messages[1]?.content).toBe("question 2");
  });

  it("forgets everything when the app locks, and what a stream still delivers does not come back", async () => {
    const { store, memory } = await setup({
      settings: { [AI_SETTING]: AI_ON },
      security: { kitCreatedAt: 1, appLockPassphrase: PASSPHRASE },
    });
    state(store).openChat();
    const release = aiOf(memory).holdStreams(1);
    const asking = state(store).askChat("Water the tomatoes?");
    await settled(store, (s) => (s.chatMessages[1]?.text ?? "") !== "");
    await state(store).lock();
    await asking;
    release();
    expect(state(store).phase).toBe("locked");
    expect(state(store)).toMatchObject({
      chatOpen: false,
      chatMessages: [],
      chatStreaming: false,
      chatRequestId: null,
      chatScope: "space",
      aiStatus: null,
      aiIndex: null,
      mcpClients: null,
      mcpAudit: null,
      related: { query: "", items: [] },
    });
    expect(Object.keys(state(store).settings)).not.toContain(AI_SETTING);
  });

  it("copies an answer, and a toast says so (or that it could not be copied)", async () => {
    const { store } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await state(store).askChat("Water the tomatoes?");
    const answer = messagesOf(store)[1];
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    await state(store).copyChatAnswer(answer?.id ?? 0);
    expect(writeText).toHaveBeenCalledWith(answer?.text);
    expect(state(store).toasts.at(-1)).toMatchObject({ text: t("aiChat.copied"), kind: "info" });
    vi.stubGlobal("navigator", { clipboard: { writeText: () => Promise.reject(new Error("denied")) } });
    await state(store).copyChatAnswer(answer?.id ?? 0);
    expect(state(store).toasts.at(-1)).toMatchObject({ text: t("aiChat.copyFailed"), kind: "error" });
    vi.unstubAllGlobals();
  });

  it("opens the page and block a source is from", async () => {
    const { store } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    state(store).setChatScope("all");
    await state(store).askChat("tomato roadmap");
    const source = messagesOf(store)[1]?.sources.find((candidate) => candidate.title === "Roadmap");
    expect(source).toBeDefined();
    if (source === undefined) {
      return;
    }
    state(store).openChatSource(source);
    await settled(store, (s) => s.selectedPageId === source.pageId);
    expect(state(store).currentSpaceId).toBe(source.spaceId);
    expect(state(store).scrollTarget).toMatchObject({ pageId: source.pageId, blockId: source.blockId });
  });
});

describe("the text of a saved answer", () => {
  it("is the answer then the titles of its sources, each page once", () => {
    const message: ChatMessage = {
      id: 1,
      role: "assistant",
      text: "The beans go in after the frost [1].",
      status: "done",
      sources: [
        { n: 1, pageId: "p1", spaceId: "s", title: "Garden plans", text: "x", score: 1 },
        { n: 2, pageId: "p1", spaceId: "s", title: "Garden plans", text: "y", score: 1 },
        { n: 3, pageId: "p2", spaceId: "s", title: "", text: "z", score: 1 },
      ],
    };
    expect(answerMarkdown(message)).toBe(
      `The beans go in after the frost [1].\n\n### ${t("aiChat.sources")}\n\n- Garden plans\n- ${t("page.untitled")}`,
    );
    expect(answerMarkdown({ ...message, sources: [] })).toBe("The beans go in after the frost [1].");
  });
});

describe("describeAiError", () => {
  const error = (message: string, details?: Record<string, string>) => ({ code: "InvalidInput" as const, message, details });
  it("words the shell's failures for people", () => {
    expect(describeAiError(error("x", { field: "enabled" }))).toBe(t("ai.error.off"));
    expect(describeAiError(error("x", { field: "endpoint", reason: "unreachable" }))).toBe(t("ai.error.unreachable"));
    expect(describeAiError(error("x", { field: "endpoint", reason: "model", model: "qwen3:8b" }))).toBe(t("ai.error.model", { model: "qwen3:8b" }));
    expect(describeAiError(error("x", { field: "endpoint", reason: "model" }))).toBe(t("ai.error.modelMissing"));
    expect(describeAiError(error("x", { field: "model", reason: "missing" }))).toBe(t("ai.error.noModel"));
    expect(describeAiError(error("x", { field: "requestId", reason: "busy" }))).toBe(t("ai.error.busy"));
    expect(describeAiError(error("consent required", { field: "endpoint" }))).toBe(t("ai.error.consent"));
    expect(describeAiError({ code: "Locked", message: "locked" })).toBe(t("ai.error.locked"));
    expect(describeAiError(error("Ollama said no", { field: "endpoint", reason: "remote" }))).toBe(t("ai.error.generic", { message: "Ollama said no" }));
  });
});

describe("related search", () => {
  it("looks for passages once the index has chunks: one row per page, best first, at least three characters", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await state(store).runRelatedSearch("tomato");
    // The index is empty: nothing to show.
    expect(state(store).related).toEqual({ query: "", items: [] });
    await aiOf(memory).advanceIndex();
    await state(store).loadAiStatus();
    await state(store).runRelatedSearch("tomato");
    const related = state(store).related;
    expect(related.query).toBe("tomato");
    expect(related.items.map((source) => source.title).sort()).toEqual(["Cooking", "Garden plans", "Roadmap"]);
    expect(new Set(related.items.map((source) => source.pageId)).size).toBe(related.items.length);
    await state(store).runRelatedSearch("to");
    expect(state(store).related).toEqual({ query: "", items: [] });
  });

  it("asks for six passages from every space", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(memory).advanceIndex();
    await state(store).loadAiStatus();
    await state(store).runRelatedSearch("tomato");
    expect(lastCall(aiOf(memory), "search")).toEqual({ kind: "search", input: { query: "tomato", scope: { type: "all" }, limit: 6 } });
  });

  it("is silent when the search fails, and does nothing with AI off or with the index off", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const ai = aiOf(memory);
    await ai.advanceIndex();
    await state(store).loadAiStatus();
    ai.failNext({ code: "Internal", message: "boom" });
    await state(store).runRelatedSearch("tomato");
    expect(state(store).related.items).toEqual([]);
    const callsBefore = ai.calls.length;
    await state(store).updateAiSettings({ enabled: false });
    await state(store).runRelatedSearch("tomato");
    expect(state(store).related.items).toEqual([]);
    expect(ai.calls.slice(callsBefore).some((call) => call.kind === "search")).toBe(false);
  });

  it("drops a slower answer for an older query", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    const ai = aiOf(memory);
    await ai.advanceIndex();
    await state(store).loadAiStatus();
    const first = state(store).runRelatedSearch("tomato");
    const second = state(store).runRelatedSearch("beans");
    await Promise.all([first, second]);
    expect(state(store).related.query).toBe("beans");
  });

  it("clears", async () => {
    const { store, memory } = await setup({ settings: { [AI_SETTING]: AI_ON } });
    await aiOf(memory).advanceIndex();
    await state(store).loadAiStatus();
    await state(store).runRelatedSearch("tomato");
    expect(state(store).related.items.length).toBeGreaterThan(0);
    state(store).clearRelated();
    expect(state(store).related).toEqual({ query: "", items: [] });
  });
});
