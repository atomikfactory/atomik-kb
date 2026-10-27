// The local AI part of the app store (ADR-0037, `docs/architecture/ai.md` sections 2, 7, 8): the AI and MCP
// settings (stored in `app.ai.settings` and `app.mcp.settings`, read once at boot so every entry point knows
// whether AI is on), what the shell reports (the status of Ollama, the semantic index, the MCP server), the MCP
// clients and audit log, the chat panel's conversation and the "Related" results of the search dialog.
// `store.ts` composes it with the rest.
//
// Everything here is workspace content or derived from it, so a lock clears it all (`EMPTY_AI`) and ends the
// running stream (`onLocked`): messages, sources, tokens never come back. Nothing is persisted: the chat lives in
// memory only (ADR-0037, decision 4).
//
// How other code uses it: `selectAiAvailable(state)` is true when the transport has an AI at all,
// `selectAiEnabled(state)` when it is also switched on, `selectAiReady(state)` when a chat model is chosen too
// (the writing assistant and the chat need that); `selectAiSettings(state)` is the parsed `AiSettings`.

import {
  AI_SETTINGS_SETTING,
  DEFAULT_MCP_PORT,
  MCP_SETTINGS_SETTING,
  aiSettingsOf,
  defaultModels,
  mcpSettingsOf,
  toCoreError,
} from "@atomik-kb/core-client";
import type {
  AiChatInput,
  AiIndexInfo,
  AiScope,
  AiSettings,
  AiSource,
  AiStatus,
  AiStreamEvent,
  ApiError,
  CoreClient,
  Event as CoreEvent,
  JsonValue,
  McpAuditEntry,
  McpClientCreated,
  McpClientInfo,
  McpGrant,
  McpSettings,
  McpStatus,
  PageId,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { writeClipboard } from "./clipboard";
import { getActiveEditor, insertMarkdownAtEnd } from "./editor/ai/active-editor";
import { t } from "./i18n";
import type { AppState } from "./store";

// ----- types ------------------------------------------------------------------------------------------

/** Where the chat looks: the space of the active tab, every space, or the open page. */
export type ChatScopeChoice = "space" | "all" | "page";

/** One message of the chat: the person's question or the model's answer. */
export interface ChatMessage {
  /** Distinct for the life of the store. */
  id: number;
  role: "user" | "assistant";
  /** The text (Markdown for an answer; it grows while the answer streams). */
  text: string;
  /** The passages an answer is based on, numbered as its `[n]` citations. */
  sources: readonly AiSource[];
  /** `streaming` while the answer arrives, `stopped` after Stop (the text so far stays), `error` when it failed. */
  status: "done" | "streaming" | "stopped" | "error";
  /** Why an answer failed (a sentence for people). */
  error?: string;
  /** For an answer: the question it answers (the title of "Save as page"). */
  question?: string;
}

/** The "Related" results of the search dialog: one passage per page, best first. */
export interface RelatedResults {
  /** The query they are for. */
  query: string;
  items: readonly AiSource[];
}

/** What the transport can do (it does not change while the app runs). */
export interface AiCapabilities {
  /** `CoreClient.hasAi()`. */
  ai: boolean;
  /** `CoreClient.hasMcp()`. */
  mcp: boolean;
}

export interface AiData {
  /** The last `ai_status`; `null` before the first read and after a lock. */
  aiStatus: AiStatus | null;
  /** A status read is running. */
  aiStatusLoading: boolean;
  /** Why the last status read failed (a sentence); `null` when it did not. */
  aiStatusError: string | null;
  /** The semantic index (kept fresh by `AiIndexChanged`, at most once a second); `null` until it was read. */
  aiIndex: AiIndexInfo | null;
  /** The last `mcp_status`. */
  mcpStatus: McpStatus | null;
  /** The MCP clients; `null` until they were read. */
  mcpClients: readonly McpClientInfo[] | null;
  /** The last 100 audit rows (of the client in `mcpAuditClient`); `null` until they were read. */
  mcpAudit: readonly McpAuditEntry[] | null;
  /** The client the audit is limited to; `null` for all. */
  mcpAuditClient: string | null;
  /** The chat panel is open. */
  chatOpen: boolean;
  chatMessages: readonly ChatMessage[];
  /** An answer is streaming. */
  chatStreaming: boolean;
  /** The scope the person chose (see {@link effectiveChatScope}). */
  chatScope: ChatScopeChoice;
  /** The id of the running stream (what Stop cancels); `null` when none runs. */
  chatRequestId: string | null;
  /** The search dialog's "Related" group. */
  related: RelatedResults;
}

export const EMPTY_RELATED: RelatedResults = { query: "", items: [] };

/** What is left of the AI part after a lock: nothing from the workspace. */
export const EMPTY_AI: AiData = {
  aiStatus: null,
  aiStatusLoading: false,
  aiStatusError: null,
  aiIndex: null,
  mcpStatus: null,
  mcpClients: null,
  mcpAudit: null,
  mcpAuditClient: null,
  chatOpen: false,
  chatMessages: [],
  chatStreaming: false,
  chatScope: "space",
  chatRequestId: null,
  related: EMPTY_RELATED,
};

/** Every action may reject with a `CoreError`, except where it says it reports its own failures. */
export interface AiActions {
  aiCapabilities: AiCapabilities;
  /**
   * The core client the store was made with, for features outside this slice that call the shell's AI commands
   * themselves (the editor's writing assistant and recorder: `aiGenerate`, `aiCancel`, `aiTranscribe`, ...).
   * It never changes and is not cleared by a lock; check `selectAiReady` before using it.
   */
  aiClient: CoreClient;

  // Settings -> AI
  /** Reads `ai_status` (Ollama's reachability, its models, the index and the indexer). Reports its own failures in `aiStatusError`. */
  loadAiStatus: () => Promise<void>;
  /**
   * Changes the AI settings and stores them. Turning AI on fills an empty chat or embedding model from the
   * models Ollama lists (`defaultModels`); turning AI or the semantic index off deletes the vectors
   * (`AiIndexClear`).
   */
  updateAiSettings: (patch: Partial<AiSettings>) => Promise<void>;
  /** Stores the Ollama endpoint (a remote host asks for consent in a native dialog); resolves with the new status. */
  saveAiEndpoint: (url: string) => Promise<AiStatus>;
  /** Deletes every vector; the indexer builds them again. */
  rebuildAiIndex: () => Promise<void>;

  // Settings -> AI -> MCP
  /** Reads the MCP status, the clients and the audit log, and keeps them fresh (`McpChanged`). Reports its own failures in the console. */
  loadMcp: () => Promise<void>;
  /** Reads `mcp_status` again. */
  loadMcpStatus: () => Promise<void>;
  updateMcpSettings: (patch: Partial<McpSettings>) => Promise<void>;
  /** Creates a client; the answer carries its token, which is shown once and is not kept here. */
  createMcpClient: (name: string) => Promise<McpClientCreated>;
  /** Replaces a client's grants (adding access asks in a native dialog); resolves with the client. */
  setMcpGrants: (clientId: string, grants: McpGrant[]) => Promise<McpClientInfo>;
  deleteMcpClient: (clientId: string) => Promise<void>;
  /** Limits the audit to one client (`null`: all) and reads it again. */
  setMcpAuditClient: (clientId: string | null) => Promise<void>;
  clearMcpAudit: () => Promise<void>;

  // The chat panel
  openChat: () => void;
  closeChat: () => void;
  toggleChat: () => void;
  setChatScope: (scope: ChatScopeChoice) => void;
  /** Asks a question: adds it and the streaming answer to the conversation. Reports failures in the answer. */
  askChat: (question: string) => Promise<void>;
  /** Cancels the running answer (it keeps the text that arrived). */
  stopChat: () => void;
  /** Cancels any answer and forgets the conversation. */
  newChat: () => void;
  /** Copies an answer's Markdown. */
  copyChatAnswer: (messageId: number) => Promise<void>;
  /** Appends an answer at the end of the open page (one undo step); a toast says when no page can take it. */
  insertChatAnswer: (messageId: number) => void;
  /** Makes a page in the current space titled with the question, holding the answer and its sources, and opens it. */
  saveChatAnswerAsPage: (messageId: number) => Promise<void>;
  /** Opens the page (and block) a source is from. */
  openChatSource: (source: AiSource) => void;

  // The search dialog
  /** Looks for related passages (the "Related" group); silent when AI is off, the index is empty or it fails. */
  runRelatedSearch: (query: string) => Promise<void>;
  clearRelated: () => void;
}

export type AiState = AiData & AiActions;

export interface AiSlice {
  initial: AiState;
  /** Reads the settings the AI needs at boot (a key that cannot be read is left out). */
  load: () => Promise<Record<string, JsonValue | null>>;
  onEvent: (event: CoreEvent) => void;
  /** The workspace locked: the running stream ends, and nothing it still delivers is shown. */
  onLocked: () => void;
}

// ----- selectors -------------------------------------------------------------------------------------

const AI_OFF: AiSettings = { enabled: false, semanticIndex: true, think: false };
const MCP_OFF: McpSettings = { enabled: false, port: DEFAULT_MCP_PORT };

let lastAiRaw: JsonValue | null | undefined;
let lastAi: AiSettings = AI_OFF;
let lastMcpRaw: JsonValue | null | undefined;
let lastMcp: McpSettings = MCP_OFF;

/**
 * The AI settings as stored (`app.ai.settings`; AI is off when none are). The answer is the same object while the
 * stored value is, so a component may select it.
 */
export function selectAiSettings(state: AppState): AiSettings {
  const raw = state.settings[AI_SETTINGS_SETTING];
  if (raw !== lastAiRaw) {
    lastAiRaw = raw;
    lastAi = raw === undefined || raw === null ? AI_OFF : aiSettingsOf(raw);
  }
  return lastAi;
}

/** The MCP settings as stored (`app.mcp.settings`; off on port 47811 when none are); stable like {@link selectAiSettings}. */
export function selectMcpSettings(state: AppState): McpSettings {
  const raw = state.settings[MCP_SETTINGS_SETTING];
  if (raw !== lastMcpRaw) {
    lastMcpRaw = raw;
    lastMcp = raw === undefined || raw === null ? MCP_OFF : mcpSettingsOf(raw);
  }
  return lastMcp;
}

/** The transport has a local AI at all (Ollama through the shell, or the in-memory fake). */
export const selectAiAvailable = (state: AppState): boolean => state.aiCapabilities.ai;

/** AI is available and switched on: every AI entry point shows. */
export const selectAiEnabled = (state: AppState): boolean => state.aiCapabilities.ai && selectAiSettings(state).enabled;

/** AI is on and a chat model is chosen: the writing assistant and the chat can work. */
export const selectAiReady = (state: AppState): boolean =>
  selectAiEnabled(state) && selectAiSettings(state).chatModel !== undefined;

/**
 * The scope the next question uses: the person's choice, except that "This page" needs an open page (and so
 * falls back to "This space" when none is).
 */
export function effectiveChatScope(state: Pick<AppState, "chatScope" | "selectedPageId">): ChatScopeChoice {
  return state.chatScope === "page" && state.selectedPageId === null ? "space" : state.chatScope;
}

// ----- helpers ---------------------------------------------------------------------------------------

/** How often `AiIndexChanged` and `McpChanged` refresh what is shown, at most. */
export const AI_REFRESH_MS = 1000;
/** Most characters a question may have (the core takes 16,000; the box stops well short). */
export const MAX_QUESTION_CHARS = 4000;
/** Limits of a chat request (section 10). */
const MAX_CHAT_MESSAGES = 40;
const MAX_CHAT_MESSAGE_CHARS = 16_000;
const MAX_CHAT_TOTAL_CHARS = 64_000;
/** Most characters of a saved page's title. */
const MAX_TITLE_CHARS = 100;
/** The search dialog's "Related" group: shortest query and most rows. */
export const RELATED_MIN_CHARS = 3;
export const RELATED_LIMIT = 6;
/** How long "Save as page" waits for the new page's editor, and how often it looks. */
const EDITOR_WAIT_MS = 6000;
const EDITOR_POLL_MS = 50;

/** The reason a failure carries (`details.reason`), if there is one. */
function reasonOf(error: ApiError): string | null {
  const { details } = error;
  if (typeof details !== "object" || details === null || Array.isArray(details)) {
    return null;
  }
  const reason = details.reason;
  return typeof reason === "string" ? reason : null;
}

function detailString(error: ApiError, key: string): string | null {
  const { details } = error;
  if (typeof details !== "object" || details === null || Array.isArray(details)) {
    return null;
  }
  const value = details[key];
  return typeof value === "string" ? value : null;
}

/** Whether a failure is the cancelling of a stream (`error` event or rejection with reason `cancelled`). */
export function isCancelled(error: ApiError): boolean {
  return reasonOf(error) === "cancelled";
}

/** A sentence for people for a failure of an AI command (the shell's `details.reason` says what happened). */
export function describeAiError(error: ApiError): string {
  if (error.code === "Locked") {
    return t("ai.error.locked");
  }
  if (detailString(error, "field") === "enabled") {
    return t("ai.error.off");
  }
  switch (reasonOf(error)) {
    case "unreachable":
      return t("ai.error.unreachable");
    case "model": {
      const model = detailString(error, "model");
      return model === null ? t("ai.error.modelMissing") : t("ai.error.model", { model });
    }
    case "missing":
      return t("ai.error.noModel");
    case "busy":
      return t("ai.error.busy");
    default:
      break;
  }
  if (detailString(error, "field") === "endpoint" && error.message.toLowerCase().includes("consent")) {
    return t("ai.error.consent");
  }
  return t("ai.error.generic", { message: error.message });
}

/** The JSON stored for the AI settings: every field that has a value. */
function aiSettingsJson(settings: AiSettings): JsonValue {
  const json: { [key: string]: JsonValue } = {
    enabled: settings.enabled,
    semanticIndex: settings.semanticIndex,
    think: settings.think,
  };
  if (settings.chatModel !== undefined) {
    json.chatModel = settings.chatModel;
  }
  if (settings.embeddingModel !== undefined) {
    json.embeddingModel = settings.embeddingModel;
  }
  if (settings.transcriptionModel !== undefined) {
    json.transcriptionModel = settings.transcriptionModel;
  }
  return json;
}

/** Runs `run` now and, when asked again within `ms`, once more when the time is up. */
function throttle(ms: number, run: () => void): { call: () => void; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pending = false;
  const arm = (): void => {
    timer = setTimeout(() => {
      timer = null;
      if (pending) {
        pending = false;
        run();
        arm();
      }
    }, ms);
  };
  return {
    call: () => {
      if (timer === null) {
        run();
        arm();
      } else {
        pending = true;
      }
    },
    cancel: () => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      pending = false;
    },
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

/** The messages a chat request carries: the finished ones and the new question, cut to the limits of section 10. */
function requestMessages(history: readonly ChatMessage[], question: string): AiChatInput["messages"] {
  const messages = [
    ...history.filter((message) => message.status !== "error" && message.text !== "").map((message) => ({
      role: message.role,
      content: message.text.slice(0, MAX_CHAT_MESSAGE_CHARS),
    })),
    { role: "user" as const, content: question.slice(0, MAX_CHAT_MESSAGE_CHARS) },
  ];
  let kept = messages.slice(-MAX_CHAT_MESSAGES);
  let total = kept.reduce((sum, message) => sum + message.content.length, 0);
  // The oldest go first; the question stays.
  while (total > MAX_CHAT_TOTAL_CHARS && kept.length > 1) {
    total -= kept[0]?.content.length ?? 0;
    kept = kept.slice(1);
  }
  return kept;
}

/** The title of a page made from an answer: the first line of the question, cut. */
function titleOfQuestion(question: string): string {
  const first = question.trim().split(/\r?\n/)[0] ?? "";
  const chars = Array.from(first);
  return chars.length <= MAX_TITLE_CHARS ? first : `${chars.slice(0, MAX_TITLE_CHARS - 1).join("")}…`;
}

/** The Markdown of a saved answer: the answer, then the pages it is based on. */
export function answerMarkdown(message: ChatMessage): string {
  const titles: string[] = [];
  for (const source of message.sources) {
    const title = source.title.trim() === "" ? t("page.untitled") : source.title.trim();
    if (!titles.includes(title)) {
      titles.push(title);
    }
  }
  if (titles.length === 0) {
    return message.text;
  }
  return `${message.text}\n\n### ${t("aiChat.sources")}\n\n${titles.map((title) => `- ${title}`).join("\n")}`;
}

// ----- the slice -------------------------------------------------------------------------------------

export function createAiSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): AiSlice {
  const capabilities: AiCapabilities = { ai: client.hasAi(), mcp: client.hasMcp() };
  let nextMessage = 0;
  let nextRequest = 0;
  // Bumped when a conversation ends (a new chat, a lock): what a stream of an older one delivers is dropped.
  let chatEpoch = 0;
  let statusTicket = 0;
  let relatedTicket = 0;
  // True once the index or the MCP data were read: from then on their events keep them fresh.
  let indexWatched = false;
  let mcpWatched = false;

  async function fetchIndex(): Promise<AiIndexInfo | null> {
    try {
      const info = await client.aiIndexStatus();
      indexWatched = true;
      set({ aiIndex: info });
      return info;
    } catch (error) {
      console.error("could not read the semantic index", toCoreError(error).code);
      return null;
    }
  }

  async function fetchMcpData(): Promise<void> {
    const filter = get().mcpAuditClient;
    const [clients, audit] = await Promise.allSettled([
      client.mcpClientList(),
      client.mcpAuditList({ limit: 100, ...(filter === null ? {} : { clientId: filter }) }),
    ]);
    // The filter may have changed while the answer was on its way: then a newer read is.
    set({
      ...(clients.status === "fulfilled" ? { mcpClients: clients.value } : {}),
      ...(audit.status === "fulfilled" && get().mcpAuditClient === filter ? { mcpAudit: audit.value } : {}),
    });
    for (const result of [clients, audit]) {
      if (result.status === "rejected") {
        console.error("could not read the MCP clients or audit log", toCoreError(result.reason).code);
      }
    }
  }

  const refreshIndex = throttle(AI_REFRESH_MS, () => {
    void fetchIndex();
  });
  const refreshMcp = throttle(AI_REFRESH_MS, () => {
    void fetchMcpData();
  });

  async function loadStatus(): Promise<void> {
    if (!capabilities.ai) {
      return;
    }
    const ticket = (statusTicket += 1);
    set({ aiStatusLoading: true });
    try {
      const status = await client.aiStatus();
      if (ticket === statusTicket) {
        indexWatched = indexWatched || status.index !== undefined;
        set((state) => ({
          aiStatus: status,
          aiStatusLoading: false,
          aiStatusError: null,
          aiIndex: status.index ?? state.aiIndex,
        }));
      }
    } catch (error) {
      if (ticket === statusTicket) {
        set({ aiStatusLoading: false, aiStatusError: describeAiError(toCoreError(error).toApiError()) });
      }
    }
  }

  function messageById(id: number): ChatMessage | undefined {
    return get().chatMessages.find((message) => message.id === id);
  }

  function patchMessage(epoch: number, id: number, patch: (message: ChatMessage) => ChatMessage): void {
    if (epoch !== chatEpoch) {
      return;
    }
    set((state) => ({ chatMessages: state.chatMessages.map((message) => (message.id === id ? patch(message) : message)) }));
  }

  function chatScopeNow(): AiScope {
    const state = get();
    const choice = effectiveChatScope(state);
    if (choice === "all") {
      return { type: "all" };
    }
    if (choice === "page" && state.selectedPageId !== null) {
      return { type: "page", pageId: state.selectedPageId };
    }
    return state.currentSpaceId === null ? { type: "all" } : { type: "space", spaceId: state.currentSpaceId };
  }

  /** Waits (up to a few seconds) until the editor of `pageId` is mounted and editable. */
  async function editorOf(pageId: PageId): Promise<boolean> {
    for (let waited = 0; waited <= EDITOR_WAIT_MS; waited += EDITOR_POLL_MS) {
      if (getActiveEditor()?.pageId === pageId) {
        return true;
      }
      await delay(EDITOR_POLL_MS);
    }
    return false;
  }

  const initial: AiState = {
    ...EMPTY_AI,
    aiCapabilities: capabilities,
    aiClient: client,

    loadAiStatus: loadStatus,

    updateAiSettings: async (patch) => {
      const current = selectAiSettings(get());
      const next: AiSettings = { ...current, ...patch };
      if (patch.enabled === true && !current.enabled) {
        // Turned on: empty models are filled from what Ollama has (section 2).
        let models = get().aiStatus?.models;
        if (models === undefined) {
          await loadStatus();
          models = get().aiStatus?.models ?? [];
        }
        const defaults = defaultModels(models);
        if (next.chatModel === undefined && defaults.chatModel !== undefined) {
          next.chatModel = defaults.chatModel;
        }
        if (next.embeddingModel === undefined && defaults.embeddingModel !== undefined) {
          next.embeddingModel = defaults.embeddingModel;
        }
      }
      await get().setSetting(AI_SETTINGS_SETTING, aiSettingsJson(next));
      const turnedOff = (current.enabled && !next.enabled) || (current.semanticIndex && !next.semanticIndex);
      if (turnedOff) {
        // The vectors are as sensitive as the text: they do not outlive the feature.
        try {
          await client.aiIndexClear();
        } catch (error) {
          console.error("could not clear the semantic index", toCoreError(error).code);
        }
        set({ related: EMPTY_RELATED });
      }
      void fetchIndex();
    },

    saveAiEndpoint: async (url) => {
      const status = await client.aiEndpointSet(url);
      statusTicket += 1;
      set((state) => ({ aiStatus: status, aiStatusError: null, aiIndex: status.index ?? state.aiIndex }));
      return status;
    },

    rebuildAiIndex: async () => {
      await client.aiIndexClear();
      await fetchIndex();
    },

    loadMcp: async () => {
      if (!capabilities.mcp) {
        return;
      }
      mcpWatched = true;
      await Promise.all([get().loadMcpStatus(), fetchMcpData()]);
    },

    loadMcpStatus: async () => {
      if (!capabilities.mcp) {
        return;
      }
      try {
        set({ mcpStatus: await client.mcpStatus() });
      } catch (error) {
        console.error("could not read the MCP status", toCoreError(error).code);
      }
    },

    updateMcpSettings: async (patch) => {
      const next: McpSettings = { ...selectMcpSettings(get()), ...patch };
      await get().setSetting(MCP_SETTINGS_SETTING, { enabled: next.enabled, port: next.port });
      // The shell starts or stops the server on the setting; its status follows a moment later.
      await get().loadMcpStatus();
      setTimeout(() => {
        void get().loadMcpStatus();
      }, 800);
    },

    createMcpClient: async (name) => {
      const created = await client.mcpClientCreate(name);
      mcpWatched = true;
      refreshMcp.call();
      return created;
    },

    setMcpGrants: async (clientId, grants) => {
      const updated = await client.mcpGrantsSet(clientId, grants);
      refreshMcp.call();
      return updated;
    },

    deleteMcpClient: async (clientId) => {
      await client.mcpClientDelete(clientId);
      if (get().mcpAuditClient === clientId) {
        set({ mcpAuditClient: null });
      }
      refreshMcp.call();
    },

    setMcpAuditClient: async (clientId) => {
      set({ mcpAuditClient: clientId });
      await fetchMcpData();
    },

    clearMcpAudit: async () => {
      await client.mcpAuditClear();
      set({ mcpAudit: [] });
    },

    openChat: () => {
      if (!get().chatOpen) {
        set({ chatOpen: true });
        if (!indexWatched && selectAiEnabled(get())) {
          void fetchIndex();
        }
      }
    },

    closeChat: () => {
      if (get().chatOpen) {
        set({ chatOpen: false });
      }
    },

    toggleChat: () => {
      if (get().chatOpen) {
        get().closeChat();
      } else {
        get().openChat();
      }
    },

    setChatScope: (scope) => {
      set({ chatScope: scope });
    },

    askChat: async (rawQuestion) => {
      const question = rawQuestion.trim().slice(0, MAX_QUESTION_CHARS);
      if (question === "" || get().chatStreaming) {
        return;
      }
      const epoch = chatEpoch;
      nextMessage += 1;
      const userId = nextMessage;
      nextMessage += 1;
      const answerId = nextMessage;
      nextRequest += 1;
      const requestId = `chat-${String(nextRequest)}`;
      const messages = requestMessages(get().chatMessages, question);
      const scope = chatScopeNow();
      set((state) => ({
        chatMessages: [
          ...state.chatMessages,
          { id: userId, role: "user", text: question, sources: [], status: "done" },
          { id: answerId, role: "assistant", text: "", sources: [], status: "streaming", question },
        ],
        chatStreaming: true,
        chatRequestId: requestId,
      }));
      const onEvent = (event: AiStreamEvent): void => {
        switch (event.type) {
          case "sources":
            patchMessage(epoch, answerId, (message) => ({ ...message, sources: event.sources }));
            break;
          case "delta":
            patchMessage(epoch, answerId, (message) => ({ ...message, text: message.text + event.text }));
            break;
          case "done":
            patchMessage(epoch, answerId, (message) => ({ ...message, text: event.text, status: "done" }));
            break;
          case "error":
            patchMessage(epoch, answerId, (message) =>
              isCancelled(event.error)
                ? { ...message, status: "stopped" }
                : { ...message, status: "error", error: describeAiError(event.error) },
            );
            break;
        }
      };
      try {
        await client.aiChat(requestId, { messages, scope }, onEvent);
        // A stream that ended without a last event leaves the answer as it is.
        patchMessage(epoch, answerId, (message) =>
          message.status === "streaming" ? { ...message, status: message.text === "" ? "stopped" : "done" } : message,
        );
      } catch (error) {
        const failure = toCoreError(error).toApiError();
        patchMessage(epoch, answerId, (message) => ({ ...message, status: "error", error: describeAiError(failure) }));
      } finally {
        if (epoch === chatEpoch) {
          set({ chatStreaming: false, chatRequestId: null });
        }
      }
    },

    stopChat: () => {
      const requestId = get().chatRequestId;
      if (requestId !== null) {
        client.aiCancel(requestId).catch((error: unknown) => {
          console.error("could not cancel the answer", toCoreError(error).code);
        });
      }
    },

    newChat: () => {
      get().stopChat();
      chatEpoch += 1;
      set({ chatMessages: [], chatStreaming: false, chatRequestId: null });
    },

    copyChatAnswer: async (messageId) => {
      const message = messageById(messageId);
      if (message === undefined) {
        return;
      }
      try {
        await writeClipboard(message.text);
        get().showToast(t("aiChat.copied"));
      } catch {
        get().showToast(t("aiChat.copyFailed"), "error");
      }
    },

    insertChatAnswer: (messageId) => {
      const message = messageById(messageId);
      if (message === undefined) {
        return;
      }
      if (insertMarkdownAtEnd(message.text)) {
        get().showToast(t("aiChat.inserted"));
      } else {
        get().showToast(t("aiChat.insertNoPage"), "error");
      }
    },

    saveChatAnswerAsPage: async (messageId) => {
      const message = messageById(messageId);
      const spaceId = get().currentSpaceId;
      if (message === undefined || spaceId === null) {
        return;
      }
      try {
        const title = titleOfQuestion(message.question ?? message.text);
        const page = await get().createPage({ title, select: false });
        // The page opens in a tab of its own, and its editor takes the answer like any pasted Markdown.
        get().navigateTo({ pageId: page.id, spaceId, disposition: "foreground" });
        if (!(await editorOf(page.id)) || !insertMarkdownAtEnd(answerMarkdown(message))) {
          get().showToast(t("aiChat.saveIncomplete", { title: page.title }), "error");
          return;
        }
        get().showToast(t("aiChat.saved", { title: page.title }));
      } catch (error) {
        get().showToast(t("aiChat.saveFailed", { message: toCoreError(error).message }), "error");
      }
    },

    openChatSource: (source) => {
      get().navigateTo({ pageId: source.pageId, spaceId: source.spaceId, blockId: source.blockId });
    },

    runRelatedSearch: async (query) => {
      const ticket = (relatedTicket += 1);
      const trimmed = query.trim();
      const clear = (): void => {
        if (get().related.items.length > 0 || get().related.query !== "") {
          set({ related: EMPTY_RELATED });
        }
      };
      if (!selectAiEnabled(get()) || Array.from(trimmed).length < RELATED_MIN_CHARS) {
        clear();
        return;
      }
      try {
        const index = get().aiIndex ?? (await fetchIndex());
        if (ticket !== relatedTicket) {
          return;
        }
        if (index === null || index.chunks === 0) {
          clear();
          return;
        }
        const found = await client.aiSearch(trimmed, { type: "all" }, RELATED_LIMIT);
        if (ticket !== relatedTicket) {
          return;
        }
        const seen = new Set<PageId>();
        const items = found.filter((source) => {
          if (seen.has(source.pageId)) {
            return false;
          }
          seen.add(source.pageId);
          return true;
        });
        set({ related: { query: trimmed, items } });
      } catch {
        // A failure is silent (section 8.5).
        if (ticket === relatedTicket) {
          clear();
        }
      }
    },

    clearRelated: () => {
      relatedTicket += 1;
      if (get().related.items.length > 0 || get().related.query !== "") {
        set({ related: EMPTY_RELATED });
      }
    },
  };

  async function load(): Promise<Record<string, JsonValue | null>> {
    const loaded: Record<string, JsonValue | null> = {};
    if (!capabilities.ai && !capabilities.mcp) {
      return loaded;
    }
    await Promise.all(
      [AI_SETTINGS_SETTING, MCP_SETTINGS_SETTING].map(async (key) => {
        try {
          loaded[key] = await client.settingsGet(key);
        } catch (error) {
          console.error("could not read an AI setting", toCoreError(error).code);
        }
      }),
    );
    return loaded;
  }

  return {
    initial,
    load,
    onEvent: (event) => {
      switch (event.type) {
        case "AiIndexChanged":
          if (indexWatched) {
            refreshIndex.call();
          }
          break;
        case "McpChanged":
          if (mcpWatched) {
            refreshMcp.call();
          }
          break;
        // The other events are not about AI.
      }
    },
    onLocked: () => {
      // The stream ends with the core (it is cancelled there too); what it still delivers is dropped.
      chatEpoch += 1;
      statusTicket += 1;
      relatedTicket += 1;
      indexWatched = false;
      mcpWatched = false;
      refreshIndex.cancel();
      refreshMcp.cancel();
    },
  };
}
