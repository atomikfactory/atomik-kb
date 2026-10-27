import { Channel, invoke } from "@tauri-apps/api/core";

import { TauriDictation } from "./dictation-tauri";
import type { DictationApi } from "./dictation";
import type { DriveApi } from "./drive";
import { TauriDrive } from "./drive-tauri";
import type { BackupFolderListing } from "./generated/BackupFolderListing";
import { internalError, isApiError, toCoreError } from "./errors";
import type { AiChatInput } from "./generated/AiChatInput";
import type { AiGenerateInput } from "./generated/AiGenerateInput";
import type { AiIndexerPhase } from "./generated/AiIndexerPhase";
import type { AiIndexerState } from "./generated/AiIndexerState";
import type { AiIndexInfo } from "./generated/AiIndexInfo";
import type { AiModelInfo } from "./generated/AiModelInfo";
import type { AiScope } from "./generated/AiScope";
import type { AiSource } from "./generated/AiSource";
import type { AiStatus } from "./generated/AiStatus";
import type { AiStreamEvent } from "./generated/AiStreamEvent";
import type { AiTranscribed } from "./generated/AiTranscribed";
import type { AttachmentAvailability } from "./generated/AttachmentAvailability";
import type { AttachmentId } from "./generated/AttachmentId";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { BackupSummary } from "./generated/BackupSummary";
import type { ByteRange } from "./generated/ByteRange";
import type { Event } from "./generated/Event";
import type { ExportWarning } from "./generated/ExportWarning";
import type { ImportReport } from "./generated/ImportReport";
import type { McpClientInfo } from "./generated/McpClientInfo";
import type { McpGrant } from "./generated/McpGrant";
import type { McpStatus } from "./generated/McpStatus";
import type { PageId } from "./generated/PageId";
import type { PresenceState } from "./generated/PresenceState";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SpaceId } from "./generated/SpaceId";
import type {
  AttachmentSaveResult,
  AiTransport,
  AttachmentTransport,
  BackupResult,
  CoreStateInfo,
  DeepLink,
  DocTransport,
  ExportRunOptions,
  ExportRunResult,
  ImportRunOptions,
  McpTransport,
  MenuCommand,
  PresenceTransport,
  PresenceUpdate,
  ShellEvent,
  ShellLabels,
  ShortcutState,
  Transport,
} from "./transport";

// Local dictation (ADR-0053): the `dictation_*` commands live in their own module, which this entry exports so
// that web builds, which never import `@atomik-kb/core-client/tauri`, do not pull in `@tauri-apps/api` for it.
export { TauriDictation } from "./dictation-tauri";
// Google Drive backups (ADR-0059): the `drive_*` commands, in their own module like dictation's.
export { TauriDrive } from "./drive-tauri";

/**
 * Validates the JSON the shell's `core_state` command returns:
 * `"opening" | "ready" | "locked" | { failed: ApiError }`.
 */
function parseCoreState(raw: unknown): CoreStateInfo {
  if (raw === "opening" || raw === "ready" || raw === "locked") {
    return raw;
  }
  if (typeof raw === "object" && raw !== null && "failed" in raw && isApiError(raw.failed)) {
    return { failed: raw.failed };
  }
  throw internalError("The desktop shell returned an unrecognised core state");
}

/**
 * Validates a deep link, `{ pageId, blockId? }` with both ids text (the `data` of a `deepLink` message); anything
 * else is not a link and is dropped (the shell sends nothing else, and nothing else is passed on).
 */
function deepLinkOf(raw: unknown): DeepLink | undefined {
  if (typeof raw !== "object" || raw === null) {
    return undefined;
  }
  const { pageId, blockId } = raw as Record<string, unknown>;
  if (typeof pageId !== "string" || pageId === "") {
    return undefined;
  }
  if (blockId === undefined) {
    return { pageId };
  }
  return typeof blockId === "string" && blockId !== "" ? { pageId, blockId } : undefined;
}

const PRESENCE_STATES: ReadonlySet<string> = new Set<PresenceState>(["Viewing", "Editing", "Idle"]);

/**
 * Validates one message of the `presence_events` channel, a `PresenceMessage` (`{ info, update }`: `info` as
 * `PresenceInfo`, `update` an array of byte values), and turns the update into bytes (M13 slice 3, ADR-0050). A
 * message of another shape is not one of ours and is dropped; neither the update nor the name is ever logged.
 */
function presenceUpdateOf(raw: unknown): PresenceUpdate | undefined {
  if (typeof raw !== "object" || raw === null) {
    return undefined;
  }
  const { info, update } = raw as Record<string, unknown>;
  if (typeof info !== "object" || info === null || !Array.isArray(update)) {
    return undefined;
  }
  const fields = info as Record<string, unknown>;
  const { accountId, deviceId, name, color, pageId, state, updatedAt } = fields;
  if (
    typeof accountId !== "string" ||
    typeof deviceId !== "string" ||
    typeof name !== "string" ||
    typeof color !== "number" ||
    !Number.isInteger(color) ||
    color < 0 ||
    color > 255 ||
    (pageId !== undefined && typeof pageId !== "string") ||
    typeof state !== "string" ||
    !PRESENCE_STATES.has(state) ||
    typeof updatedAt !== "number"
  ) {
    return undefined;
  }
  const bytes = update as unknown[];
  if (!bytes.every((byte) => typeof byte === "number" && Number.isInteger(byte) && byte >= 0 && byte <= 255)) {
    return undefined;
  }
  return {
    info: {
      accountId,
      deviceId,
      name,
      color,
      ...(pageId === undefined ? {} : { pageId }),
      state: state as PresenceState,
      updatedAt,
    },
    update: Uint8Array.from(bytes as number[]),
  };
}

const SHORTCUT_STATES: ReadonlySet<string> = new Set<ShortcutState>(["disabled", "registered", "taken", "invalid"]);

const MENU_COMMANDS: ReadonlySet<string> = new Set<MenuCommand>([
  "import",
  "export",
  "print",
  "undo",
  "redo",
  "find",
  "toggleSidebar",
  "back",
  "forward",
  "about",
]);

/**
 * Validates one message of the `shell_events_subscribe` channel, `{ type, data? }`: a deep link, an invitation link, a capture, a lock,
 * the state of the capture shortcut, a choice in the menu bar or its visibility (ADR-0040). A message of another
 * type, or of a known type with the wrong data, is not one of ours and is dropped (the shell sends nothing else, and
 * nothing else is passed on).
 */
function shellEventOf(raw: unknown): ShellEvent | undefined {
  if (typeof raw !== "object" || raw === null) {
    return undefined;
  }
  const { type, data } = raw as Record<string, unknown>;
  switch (type) {
    case "deepLink": {
      const link = deepLinkOf(data);
      return link === undefined ? undefined : { type: "deepLink", data: link };
    }
    case "capture":
      return { type: "capture" };
    case "lock":
      return { type: "lock" };
    case "shortcut": {
      const state = typeof data === "object" && data !== null ? (data as Record<string, unknown>).state : undefined;
      return typeof state === "string" && SHORTCUT_STATES.has(state)
        ? { type: "shortcut", data: { state: state as ShortcutState } }
        : undefined;
    }
    case "menu": {
      const id = typeof data === "object" && data !== null ? (data as Record<string, unknown>).id : undefined;
      return typeof id === "string" && MENU_COMMANDS.has(id) ? { type: "menu", data: { id: id as MenuCommand } } : undefined;
    }
    case "menuBar": {
      const visible = typeof data === "object" && data !== null ? (data as Record<string, unknown>).visible : undefined;
      return typeof visible === "boolean" ? { type: "menuBar", data: { visible } } : undefined;
    }
    case "inviteLink": {
      // An invitation link (it carries the invite's secret): only its text is passed on, never logged here.
      const link = typeof data === "object" && data !== null ? (data as Record<string, unknown>).link : undefined;
      return typeof link === "string" && link !== "" ? { type: "inviteLink", data: { link } } : undefined;
    }
    default:
      return undefined;
  }
}

function isBackupSummary(value: unknown): value is BackupSummary {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const fields = value as Record<string, unknown>;
  return (
    ["bytes", "documents", "settings", "createdAt", "chunks", "chunkBytes"].every(
      (key) => typeof fields[key] === "number",
    ) &&
    typeof fields.accountId === "string"
  );
}

/**
 * Validates what `backup_export` and `backup_import` resolve with, `{ summary: BackupSummary,
 * path }`; anything else breaks the contract and is reported as `Internal`.
 */
function backupResultOf(raw: unknown, command: string): BackupResult {
  if (typeof raw === "object" && raw !== null && "summary" in raw && "path" in raw) {
    const { summary, path } = raw;
    if (typeof path === "string" && isBackupSummary(summary)) {
      return { summary, path };
    }
  }
  throw internalError(`The desktop shell answered ${command} without a backup summary and path`);
}

/**
 * Validates what `export_run` resolves with, `{ path, pages, attachments, warnings }`; anything
 * else breaks the contract and is reported as `Internal`.
 */
function exportRunResultOf(raw: unknown): ExportRunResult {
  if (typeof raw === "object" && raw !== null) {
    const { path, pages, attachments, warnings } = raw as Record<string, unknown>;
    if (
      typeof path === "string" &&
      typeof pages === "number" &&
      typeof attachments === "number" &&
      Array.isArray(warnings)
    ) {
      return { path, pages, attachments, warnings: warnings as ExportWarning[] };
    }
  }
  throw internalError("The desktop shell answered export_run without a path, counts and warnings");
}

/**
 * Validates what `import_run` resolves with, the core's `ImportReport` (the shell's own warnings
 * merged in); anything else breaks the contract and is reported as `Internal`.
 */
function importReportOf(raw: unknown): ImportReport {
  if (typeof raw === "object" && raw !== null) {
    const fields = raw as Record<string, unknown>;
    if (
      Array.isArray(fields.rootPageIds) &&
      typeof fields.pages === "number" &&
      typeof fields.attachments === "number" &&
      typeof fields.propertiesCreated === "number" &&
      typeof fields.moreWarnings === "number" &&
      // The spaces an import into new spaces made (ADR-0039): absent otherwise.
      (fields.spaceIds === undefined || fields.spaceIds === null || Array.isArray(fields.spaceIds))
    ) {
      return raw as ImportReport;
    }
  }
  throw internalError("The desktop shell answered import_run without an import report");
}

const AVAILABILITIES: readonly AttachmentAvailability[] = ["Complete", "Downloading", "Waiting", "NotDownloaded"];

/**
 * Validates an `AttachmentInfo` from the shell and copies its known fields (an absent or
 * `null` `unreferencedSince` means "referenced"); `undefined` when it is not one.
 */
function attachmentInfoOf(value: unknown): AttachmentInfo | undefined {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  const fields = value as Record<string, unknown>;
  const { attachmentId, spaceId, name, mime, size, chunkCount, createdAt, references, unreferencedSince } = fields;
  const { localChunks, availability } = fields;
  if (
    typeof attachmentId !== "string" ||
    typeof spaceId !== "string" ||
    typeof name !== "string" ||
    typeof mime !== "string" ||
    typeof size !== "number" ||
    typeof chunkCount !== "number" ||
    typeof createdAt !== "number" ||
    typeof references !== "number" ||
    (unreferencedSince !== undefined && unreferencedSince !== null && typeof unreferencedSince !== "number")
  ) {
    return undefined;
  }
  if (
    (localChunks !== undefined && typeof localChunks !== "number") ||
    (availability !== undefined && !AVAILABILITIES.includes(availability as AttachmentAvailability))
  ) {
    return undefined;
  }
  // A core older than Milestone 8 sends neither: its attachments are complete.
  const info: AttachmentInfo = {
    attachmentId,
    spaceId,
    name,
    mime,
    size,
    chunkCount,
    createdAt,
    references,
    localChunks: typeof localChunks === "number" ? localChunks : chunkCount,
    availability: availability === undefined ? "Complete" : (availability as AttachmentAvailability),
  };
  if (typeof unreferencedSince === "number") {
    info.unreferencedSince = unreferencedSince;
  }
  return info;
}

/**
 * Raw command results: Tauri resolves a `tauri::ipc::Response` with an `ArrayBuffer`, or,
 * through its `postMessage` fallback (macOS, or when the IPC channel cannot be used),
 * with the bytes as a JSON array of numbers. Anything else breaks the contract and is
 * reported as `Internal`.
 */
function bytesOf(raw: unknown, command: string): Uint8Array {
  if (raw instanceof ArrayBuffer) {
    return new Uint8Array(raw);
  }
  if (raw instanceof Uint8Array) {
    return raw;
  }
  if (Array.isArray(raw)) {
    const values: unknown[] = raw;
    if (values.every((value) => typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= 255)) {
      return Uint8Array.from(values as number[]);
    }
  }
  throw internalError(`The desktop shell answered ${command} without raw bytes`);
}

/**
 * The page document commands of the desktop shell (`docs/development/milestone-03.md`,
 * "Shell"): raw request bodies (always a `Uint8Array`, empty when the command takes none,
 * so the body is never JSON), the page id in the `x-kb-page` header, raw results.
 */
const tauriDocs: DocTransport = {
  async open(pageId: PageId): Promise<Uint8Array> {
    try {
      return bytesOf(
        await invoke<ArrayBuffer>("doc_open", new Uint8Array(), { headers: { "x-kb-page": pageId } }),
        "doc_open",
      );
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async import(pageId: PageId, update: Uint8Array): Promise<void> {
    try {
      await invoke("doc_import", update, { headers: { "x-kb-page": pageId } });
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async export(pageId: PageId, since?: Uint8Array): Promise<Uint8Array> {
    try {
      return bytesOf(
        await invoke<ArrayBuffer>("doc_export", since ?? new Uint8Array(), { headers: { "x-kb-page": pageId } }),
        "doc_export",
      );
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async close(pageId: PageId): Promise<void> {
    try {
      await invoke("doc_close", new Uint8Array(), { headers: { "x-kb-page": pageId } });
    } catch (error) {
      throw toCoreError(error);
    }
  },
};

/** The scheme of the shell's media protocol (`apps/desktop/src-tauri/src/media.rs`). */
export const MEDIA_SCHEME = "kbmedia";

/** The attachment id as the shell's media protocol accepts it: UUID text, nothing else. */
const MEDIA_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu;

/**
 * The URL the webview loads the media protocol from, the way Tauri forms custom protocol URLs:
 * `http://kbmedia.localhost/<id>` where the webview is WebView2 (Windows) or Android's, and
 * `kbmedia://localhost/<id>` elsewhere. `null` for text that is not an attachment id.
 */
export function mediaProtocolUrl(attachmentId: string, windowsLike: boolean): string | null {
  if (!MEDIA_ID.test(attachmentId)) {
    return null;
  }
  return windowsLike ? `http://${MEDIA_SCHEME}.localhost/${attachmentId}` : `${MEDIA_SCHEME}://localhost/${attachmentId}`;
}

/** Whether this webview sees custom protocols as `http://<scheme>.localhost` (Tauri's own test: Windows and Android). */
function mediaPlatformIsWindowsLike(): boolean {
  const agent = typeof navigator === "undefined" ? "" : navigator.userAgent;
  return agent.includes("Windows") || agent.includes("Android");
}

/**
 * The attachment commands of the desktop shell (`docs/development/milestone-05.md`, "Shell"):
 * `attachment_write` and `attachment_read` are raw (the attachment id in the
 * `x-kb-attachment` header, a byte range in `x-kb-range-start`/`x-kb-range-end` as decimal
 * text, raw bodies and results, never JSON); `attachment_import_path`, `attachment_import_url`,
 * `attachment_save_as` and `attachment_open` take JSON arguments (left out when undefined) and
 * answer JSON, which is validated. `attachment_import_url` (ADR-0028) has the shell download
 * one picture from an https link; the link travels in this one call and nowhere else.
 */
const tauriAttachments: AttachmentTransport = {
  async write(attachmentId: AttachmentId, bytes: Uint8Array): Promise<void> {
    try {
      await invoke("attachment_write", bytes, { headers: { "x-kb-attachment": attachmentId } });
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async read(attachmentId: AttachmentId, range?: ByteRange): Promise<Uint8Array> {
    const headers: Record<string, string> = { "x-kb-attachment": attachmentId };
    if (range !== undefined) {
      headers["x-kb-range-start"] = String(range.start);
      headers["x-kb-range-end"] = String(range.end);
    }
    try {
      return bytesOf(await invoke<ArrayBuffer>("attachment_read", new Uint8Array(), { headers }), "attachment_read");
    } catch (error) {
      throw toCoreError(error);
    }
  },

  mediaUrl(attachmentId: AttachmentId): string | null {
    return mediaProtocolUrl(attachmentId, mediaPlatformIsWindowsLike());
  },

  async importPath(spaceId: SpaceId, path?: string): Promise<AttachmentInfo> {
    try {
      const info = attachmentInfoOf(
        await invoke<unknown>("attachment_import_path", path === undefined ? { spaceId } : { spaceId, path }),
      );
      if (info === undefined) {
        throw internalError("The desktop shell answered attachment_import_path without an attachment");
      }
      return info;
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async importUrl(spaceId: SpaceId, url: string): Promise<AttachmentInfo> {
    try {
      const info = attachmentInfoOf(await invoke<unknown>("attachment_import_url", { spaceId, url }));
      if (info === undefined) {
        throw internalError("The desktop shell answered attachment_import_url without an attachment");
      }
      return info;
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async saveAs(attachmentId: AttachmentId, path?: string): Promise<AttachmentSaveResult> {
    try {
      const raw = await invoke<unknown>(
        "attachment_save_as",
        path === undefined ? { attachmentId } : { attachmentId, path },
      );
      if (typeof raw === "object" && raw !== null && "info" in raw && "path" in raw) {
        const info = attachmentInfoOf(raw.info);
        if (info !== undefined && typeof raw.path === "string") {
          return { info, path: raw.path };
        }
      }
      throw internalError("The desktop shell answered attachment_save_as without an attachment and path");
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async open(attachmentId: AttachmentId): Promise<string> {
    try {
      const path = await invoke<unknown>("attachment_open", { attachmentId });
      if (typeof path !== "string") {
        throw internalError("The desktop shell answered attachment_open without a path");
      }
      return path;
    } catch (error) {
      throw toCoreError(error);
    }
  },
};

// --- local AI and MCP (ADR-0037) ---

/** The request header of `ai_transcribe`: JSON `{ "model"?: string }` (`apps/desktop/src-tauri/src/ai_host.rs`). */
const AI_HEADER = "x-kb-ai";

/**
 * How long, in milliseconds, a stream command that resolved waits for its last event (`done` or `error`)
 * before the transport reports the missing one itself: the shell sends the last event before it answers,
 * but a channel and a command response travel separately.
 */
const STREAM_END_GRACE_MS = 2000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** `undefined` when the member is absent or `null` (the shell leaves out what is `None`). */
function absent(value: unknown): boolean {
  return value === undefined || value === null;
}

function isNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isOptionalString(value: unknown): value is string | undefined | null {
  return absent(value) || typeof value === "string";
}

function isOptionalNumber(value: unknown): value is number | undefined | null {
  return absent(value) || isNumber(value);
}

function aiModelOf(value: unknown): AiModelInfo | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const { name, family, parameterSize, capabilities, size } = value;
  if (
    typeof name !== "string" ||
    !isOptionalString(family) ||
    !isOptionalString(parameterSize) ||
    !Array.isArray(capabilities) ||
    !capabilities.every((capability) => typeof capability === "string") ||
    !isNumber(size)
  ) {
    return undefined;
  }
  const model: AiModelInfo = { name, capabilities: [...capabilities], size };
  if (typeof family === "string") {
    model.family = family;
  }
  if (typeof parameterSize === "string") {
    model.parameterSize = parameterSize;
  }
  return model;
}

const INDEXER_PHASES: readonly AiIndexerPhase[] = ["idle", "indexing", "error"];

function aiIndexerStateOf(value: unknown): AiIndexerState | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const { phase, error, lastRunAt } = value;
  if (!INDEXER_PHASES.includes(phase as AiIndexerPhase) || !isOptionalString(error) || !isOptionalNumber(lastRunAt)) {
    return undefined;
  }
  const state: AiIndexerState = { phase: phase as AiIndexerPhase };
  if (typeof error === "string") {
    state.error = error;
  }
  if (typeof lastRunAt === "number") {
    state.lastRunAt = lastRunAt;
  }
  return state;
}

function aiIndexInfoOf(value: unknown): AiIndexInfo | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const { model, pagesIndexed, pagesTotal, chunks, dirty, dimensions } = value;
  if (
    !isOptionalString(model) ||
    !isNumber(pagesIndexed) ||
    !isNumber(pagesTotal) ||
    !isNumber(chunks) ||
    !isNumber(dirty) ||
    !isOptionalNumber(dimensions)
  ) {
    return undefined;
  }
  const info: AiIndexInfo = { pagesIndexed, pagesTotal, chunks, dirty };
  if (typeof model === "string") {
    info.model = model;
  }
  if (typeof dimensions === "number") {
    info.dimensions = dimensions;
  }
  return info;
}

/**
 * Validates what `ai_status` and `ai_endpoint_set` resolve with, an `AiStatus`, and copies its known
 * fields (an absent or `null` optional member is left out); anything else breaks the contract and is
 * reported as `Internal`.
 */
function aiStatusOf(raw: unknown, command: string): AiStatus {
  if (isRecord(raw)) {
    const { endpoint, local, remoteConsent, reachable, version, models, error, index, indexer } = raw;
    const parsedModels = Array.isArray(models) ? models.map(aiModelOf) : undefined;
    const parsedIndexer = aiIndexerStateOf(indexer);
    const parsedIndex = absent(index) ? undefined : aiIndexInfoOf(index);
    if (
      typeof endpoint === "string" &&
      typeof local === "boolean" &&
      typeof remoteConsent === "boolean" &&
      typeof reachable === "boolean" &&
      isOptionalString(version) &&
      parsedModels !== undefined &&
      parsedModels.every((model) => model !== undefined) &&
      parsedIndexer !== undefined &&
      (absent(index) || parsedIndex !== undefined) &&
      (absent(error) || isApiError(error))
    ) {
      const status: AiStatus = {
        endpoint,
        local,
        remoteConsent,
        reachable,
        models: parsedModels,
        indexer: parsedIndexer,
      };
      if (typeof version === "string") {
        status.version = version;
      }
      if (isApiError(error)) {
        status.error = error;
      }
      if (parsedIndex !== undefined) {
        status.index = parsedIndex;
      }
      return status;
    }
  }
  throw internalError(`The desktop shell answered ${command} without an AI status`);
}

function aiSourceOf(value: unknown): AiSource | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const { n, pageId, spaceId, blockId, title, icon, text, score } = value;
  if (
    !isNumber(n) ||
    typeof pageId !== "string" ||
    typeof spaceId !== "string" ||
    !isOptionalString(blockId) ||
    typeof title !== "string" ||
    !isOptionalString(icon) ||
    typeof text !== "string" ||
    !isNumber(score)
  ) {
    return undefined;
  }
  const source: AiSource = { n, pageId, spaceId, title, text, score };
  if (typeof blockId === "string") {
    source.blockId = blockId;
  }
  if (typeof icon === "string") {
    source.icon = icon;
  }
  return source;
}

function aiSourcesOf(raw: unknown): AiSource[] | undefined {
  if (!Array.isArray(raw)) {
    return undefined;
  }
  const sources = raw.map(aiSourceOf);
  return sources.every((source) => source !== undefined) ? sources : undefined;
}

/** Validates one message of a stream's channel; `undefined` when it is not an `AiStreamEvent`. */
function aiStreamEventOf(raw: unknown): AiStreamEvent | undefined {
  if (!isRecord(raw)) {
    return undefined;
  }
  switch (raw.type) {
    case "sources": {
      const sources = aiSourcesOf(raw.sources);
      return sources === undefined ? undefined : { type: "sources", sources };
    }
    case "delta":
      return typeof raw.text === "string" ? { type: "delta", text: raw.text } : undefined;
    case "done":
      return typeof raw.text === "string" && typeof raw.model === "string"
        ? { type: "done", text: raw.text, model: raw.model }
        : undefined;
    case "error":
      return isApiError(raw.error) ? { type: "error", error: raw.error } : undefined;
    default:
      return undefined;
  }
}

/** Validates what `ai_transcribe` resolves with, `{ text, language? }`. */
function aiTranscribedOf(raw: unknown): AiTranscribed {
  if (isRecord(raw) && typeof raw.text === "string" && isOptionalString(raw.language)) {
    const transcribed: AiTranscribed = { text: raw.text };
    if (typeof raw.language === "string") {
      transcribed.language = raw.language;
    }
    return transcribed;
  }
  throw internalError("The desktop shell answered ai_transcribe without a transcript");
}

/** Validates what `mcp_status` resolves with, an `McpStatus`. */
function mcpStatusOf(raw: unknown): McpStatus {
  if (isRecord(raw)) {
    const { enabled, running, port, url, bridgePath, error } = raw;
    if (
      typeof enabled === "boolean" &&
      typeof running === "boolean" &&
      isNumber(port) &&
      typeof url === "string" &&
      isOptionalString(bridgePath) &&
      isOptionalString(error)
    ) {
      const status: McpStatus = { enabled, running, port, url };
      if (typeof bridgePath === "string") {
        status.bridgePath = bridgePath;
      }
      if (typeof error === "string") {
        status.error = error;
      }
      return status;
    }
  }
  throw internalError("The desktop shell answered mcp_status without an MCP status");
}

function mcpGrantOf(value: unknown): McpGrant | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const { spaceId, read, write } = value;
  return typeof spaceId === "string" && typeof read === "boolean" && typeof write === "boolean"
    ? { spaceId, read, write }
    : undefined;
}

/** Validates what `mcp_client_grants_set` resolves with, an `McpClientInfo`. */
function mcpClientInfoOf(raw: unknown): McpClientInfo {
  if (isRecord(raw)) {
    const { id, name, grants, createdAt, lastUsedAt } = raw;
    const parsed = Array.isArray(grants) ? grants.map(mcpGrantOf) : undefined;
    if (
      typeof id === "string" &&
      typeof name === "string" &&
      parsed !== undefined &&
      parsed.every((grant) => grant !== undefined) &&
      isNumber(createdAt) &&
      isOptionalNumber(lastUsedAt)
    ) {
      const client: McpClientInfo = { id, name, grants: parsed, createdAt };
      if (typeof lastUsedAt === "number") {
        client.lastUsedAt = lastUsedAt;
      }
      return client;
    }
  }
  throw internalError("The desktop shell answered mcp_client_grants_set without a client");
}

/**
 * The channel of one stream and a promise that settles when its last event (`done` or `error`) was
 * delivered. A message that is not an `AiStreamEvent` is logged and dropped; a listener that throws
 * does not break the stream.
 */
function openAiStream(onEvent: (event: AiStreamEvent) => void): { channel: Channel<AiStreamEvent>; ended: Promise<void> } {
  let end: () => void = () => undefined;
  const ended = new Promise<void>((resolve) => {
    end = resolve;
  });
  const channel = new Channel<AiStreamEvent>((message) => {
    const event = aiStreamEventOf(message);
    if (event === undefined) {
      console.error("the desktop shell sent an unrecognised AI stream event");
      return;
    }
    try {
      onEvent(event);
    } catch (error) {
      console.error("AI stream listener failed", error);
    }
    if (event.type === "done" || event.type === "error") {
      end();
    }
  });
  return { channel, ended };
}

/**
 * Called when a stream command resolved: waits for the last event, and when it does not come within
 * {@link STREAM_END_GRACE_MS} reports an `error` event itself (the stream must always end with one).
 */
async function finishAiStream(ended: Promise<void>, onEvent: (event: AiStreamEvent) => void): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<"late">((resolve) => {
    timer = setTimeout(() => {
      resolve("late");
    }, STREAM_END_GRACE_MS);
  });
  const outcome = await Promise.race([ended.then(() => "ended" as const), late]);
  clearTimeout(timer);
  if (outcome === "late") {
    try {
      onEvent({
        type: "error",
        error: internalError("The desktop shell ended the stream without a result").toApiError(),
      });
    } catch (error) {
      console.error("AI stream listener failed", error);
    }
  }
}

/** The JSON header of `ai_transcribe`, kept to ASCII (a header value is a byte string). */
function transcribeHeader(model: string | undefined): string {
  const json = JSON.stringify(model === undefined ? {} : { model });
  return json.replace(/[\u0080-￿]/gu, (character) => `\\u${character.charCodeAt(0).toString(16).padStart(4, "0")}`);
}

/**
 * The AI commands of the desktop shell (`docs/architecture/ai.md`, section 6): JSON arguments and
 * answers, which are validated; `ai_generate` and `ai_chat` stream their events over a channel per
 * request and resolve when the stream ended; `ai_transcribe` takes the WAV segment as its raw body
 * (never JSON) with the model in the `x-kb-ai` header. A rejection means nothing was streamed (a bad or
 * busy request id, AI off, no consent, no model); every other outcome is the stream's last event.
 */
const tauriAi: AiTransport = {
  async status(): Promise<AiStatus> {
    try {
      return aiStatusOf(await invoke<unknown>("ai_status"), "ai_status");
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async endpointSet(url: string): Promise<AiStatus> {
    try {
      return aiStatusOf(await invoke<unknown>("ai_endpoint_set", { url }), "ai_endpoint_set");
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async generate(requestId: string, input: AiGenerateInput, onEvent: (event: AiStreamEvent) => void): Promise<void> {
    const { channel, ended } = openAiStream(onEvent);
    try {
      await invoke("ai_generate", { requestId, input, onEvent: channel });
    } catch (error) {
      throw toCoreError(error);
    }
    await finishAiStream(ended, onEvent);
  },

  async chat(requestId: string, input: AiChatInput, onEvent: (event: AiStreamEvent) => void): Promise<void> {
    const { channel, ended } = openAiStream(onEvent);
    try {
      await invoke("ai_chat", { requestId, input, onEvent: channel });
    } catch (error) {
      throw toCoreError(error);
    }
    await finishAiStream(ended, onEvent);
  },

  async cancel(requestId: string): Promise<void> {
    try {
      await invoke("ai_cancel", { requestId });
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async search(query: string, scope: AiScope, limit?: number): Promise<AiSource[]> {
    try {
      const sources = aiSourcesOf(
        await invoke<unknown>("ai_search", limit === undefined ? { query, scope } : { query, scope, limit }),
      );
      if (sources === undefined) {
        throw internalError("The desktop shell answered ai_search without sources");
      }
      return sources;
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async transcribe(wav: Uint8Array, model?: string): Promise<AiTranscribed> {
    try {
      return aiTranscribedOf(await invoke<unknown>("ai_transcribe", wav, { headers: { [AI_HEADER]: transcribeHeader(model) } }));
    } catch (error) {
      throw toCoreError(error);
    }
  },
};

/**
 * The MCP commands of the desktop shell (`docs/architecture/ai.md`, sections 6.4 and 7): the status of
 * the loopback server, and the change of a client's grants, which asks the person in a native dialog
 * when it adds access (declining rejects with `InvalidInput` for `grants`, `details.reason` `declined`).
 */
const tauriMcp: McpTransport = {
  async status(): Promise<McpStatus> {
    try {
      return mcpStatusOf(await invoke<unknown>("mcp_status"));
    } catch (error) {
      throw toCoreError(error);
    }
  },

  async grantsSet(clientId: string, grants: McpGrant[]): Promise<McpClientInfo> {
    try {
      return mcpClientInfoOf(await invoke<unknown>("mcp_client_grants_set", { clientId, grants }));
    } catch (error) {
      throw toCoreError(error);
    }
  },
};

/**
 * Talks to the Rust core through the Tauri shell (ADR-0015):
 *
 * - requests: `invoke("dispatch", { request })`, JSON in and out;
 * - events: one `Channel<Event>` handed to `invoke("subscribe_events", { onEvent })`,
 *   fanned out to every listener registered through {@link TauriTransport.subscribe};
 * - lifecycle: `invoke("core_state")`;
 * - page documents ({@link TauriTransport.docs}): `doc_open`, `doc_import`, `doc_export`
 *   and `doc_close` with raw bytes in and out and the page id in the `x-kb-page` header;
 * - links: `invoke("open_external", { url })` (the shell accepts http, https and mailto);
 * - app lock (ADR-0018): `invoke("lock")` and `invoke("unlock", { passphrase })`; while locked,
 *   `core_state` is `"locked"` and every other command rejects with `Locked`;
 * - backups (ADR-0017): `invoke("backup_export", { path })` and `invoke("backup_import",
 *   { path, kit })`, both resolving `{ summary, path }`; without a path the shell opens its
 *   native file dialog, and cancelling it rejects with `InvalidInput` for the field `path`;
 * - attachments ({@link TauriTransport.attachments}, ADR-0020): `attachment_write` and
 *   `attachment_read` with raw bytes and the attachment id in the `x-kb-attachment` header,
 *   `attachment_import_path`, `attachment_import_url`, `attachment_save_as` and
 *   `attachment_open` with JSON;
 * - import and export (ADR-0036): `export_run { options }`, `import_run { options }`,
 *   `save_text_file { suggestedName, text }`, `backup_folder_choose` and `backup_run_now`; the
 *   shell opens the native dialogs (the webview never passes a path) and a cancelled one rejects
 *   with `InvalidInput` for the field `path`;
 * - local AI (ADR-0037, {@link TauriTransport.ai}): `ai_status`, `ai_endpoint_set { url }`, `ai_generate` and
 *   `ai_chat { requestId, input, onEvent }` (events over a channel), `ai_cancel { requestId }`,
 *   `ai_search { query, scope, limit? }` and `ai_transcribe` (a WAV as the raw body, the model in the
 *   `x-kb-ai` header);
 * - the MCP server ({@link TauriTransport.mcp}): `mcp_status` and `mcp_client_grants_set { clientId, grants }`;
 * - what the shell tells the page on its own account (ADR-0040, {@link ShellEvent}): one `Channel<{ type, data? }>`
 *   handed to `invoke("shell_events_subscribe", { onEvent })`, fanned out to every listener registered through
 *   {@link TauriTransport.onShellEvent}; and the words of the tray menu, `shell_labels_set { labels }`;
 * - presence in shared spaces (M13 slice 3, ADR-0050, {@link TauriTransport.presence}): `presence_send { spaceId,
 *   pageId?, state, update }` and one `Channel<PresenceMessage>` handed to `invoke("presence_events", { onEvent })`;
 * - local dictation (ADR-0053, {@link TauriTransport.dictation}): the `dictation_*` commands of `dictation-tauri.ts`.
 *
 * `invoke` rejects with the serialized `ApiError` when the core returned an error and
 * with a plain string for IPC-level problems; both are normalised into a `CoreError`.
 */
export class TauriTransport implements Transport {
  /** Raw-bytes page document sessions (never JSON); every method rejects with a `CoreError`. */
  readonly docs: DocTransport = tauriDocs;
  /** Attachment bytes (raw, never JSON) and files; every method rejects with a `CoreError`. */
  readonly attachments: AttachmentTransport = tauriAttachments;
  /** Local AI through Ollama (the shell makes every call); every method rejects with a `CoreError`. */
  readonly ai: AiTransport = tauriAi;
  /** The local MCP server's status and the confirmed change of a client's grants. */
  readonly mcp: McpTransport = tauriMcp;
  /** Local dictation through the shell's sidecar (ADR-0053); every method rejects with a `CoreError`. */
  readonly dictation: DictationApi = new TauriDictation();
  /** Google Drive backups through the shell (ADR-0059); every method rejects with a `CoreError`. */
  readonly drive: DriveApi = new TauriDrive();
  /**
   * Presence frames (M13 slice 3, ADR-0050): `presence_send { spaceId, pageId?, state, update }` with the update as
   * an array of byte values (at most 3 KiB), and one `Channel<PresenceMessage>` handed to
   * `invoke("presence_events", { onEvent })`, fanned out to every listener of `presence.subscribe`. Every method
   * rejects with a `CoreError`.
   */
  readonly presence: PresenceTransport = {
    send: async (spaceId, pageId, state, update) => {
      const bytes = Array.from(update);
      try {
        await invoke("presence_send", { spaceId, pageId, state, update: bytes });
      } catch (error) {
        throw toCoreError(error);
      }
    },
    subscribe: (listener) => {
      this.#presenceListeners.add(listener);
      this.#ensurePresenceChannel();
      return () => {
        this.#presenceListeners.delete(listener);
      };
    },
  };
  readonly #listeners = new Set<(event: Event) => void>();
  #channel: Channel<Event> | undefined;
  readonly #shellListeners = new Set<(event: ShellEvent) => void>();
  #shellChannel: Channel<unknown> | undefined;
  readonly #presenceListeners = new Set<(message: PresenceUpdate) => void>();
  #presenceChannel: Channel<unknown> | undefined;

  async dispatch(request: Request): Promise<Response> {
    try {
      return await invoke<Response>("dispatch", { request });
    } catch (error) {
      throw toCoreError(error);
    }
  }

  async coreState(): Promise<CoreStateInfo> {
    try {
      return parseCoreState(await invoke<unknown>("core_state"));
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Opens an `http`, `https` or `mailto` URL with the system handler (the shell refuses other schemes). */
  async openExternal(url: string): Promise<void> {
    try {
      await invoke("open_external", { url });
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Locks the workspace; `InvalidInput` when the app lock is not enabled. */
  async lock(): Promise<void> {
    try {
      await invoke("lock");
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Unlocks the workspace; a wrong passphrase is `InvalidInput` for the field `passphrase`. */
  async unlock(passphrase: string): Promise<void> {
    try {
      await invoke("unlock", { passphrase });
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Writes an encrypted backup to `path`, or to a file picked in the shell's save dialog. */
  async backupExport(path?: string): Promise<BackupResult> {
    try {
      return backupResultOf(
        await invoke<{ summary: BackupSummary; path: string }>("backup_export", { path }),
        "backup_export",
      );
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Restores a backup from `path`, or from a file picked in the shell's open dialog, with the kit text if needed. */
  async backupImport(path?: string, kit?: string): Promise<BackupResult> {
    try {
      return backupResultOf(
        await invoke<{ summary: BackupSummary; path: string }>("backup_import", { path, kit }),
        "backup_import",
      );
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /**
   * Exports a page, a page with its sub-pages or a whole space (ADR-0036): the shell plans the
   * export, asks where to save (a `.md` or `.html` file, or a `.zip`) and writes it. Cancelling the
   * dialog rejects with `InvalidInput` for the field `path`.
   */
  async exportRun(options: ExportRunOptions): Promise<ExportRunResult> {
    try {
      return exportRunResultOf(await invoke<unknown>("export_run", { options }));
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /**
   * Imports files, a folder or a `.zip` the person picks in the shell's dialog (ADR-0036) and
   * resolves with the report. Cancelling the dialog rejects with `InvalidInput` for the field `path`.
   */
  async importRun(options: ImportRunOptions): Promise<ImportReport> {
    try {
      // `destination: "newSpaces"` (ADR-0039) goes into one new space per space of the source: the
      // shell takes no destination space or page then, so neither is sent.
      const sent =
        options.destination === "newSpaces"
          ? { destination: options.destination, source: options.source }
          : options;
      return importReportOf(await invoke<unknown>("import_run", { options: sent }));
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Asks where to save a text file (a CSV) in the shell's save dialog and writes it; resolves with the path. */
  async saveTextFile(suggestedName: string, text: string): Promise<string> {
    try {
      const path = await invoke<unknown>("save_text_file", { suggestedName, text });
      if (typeof path !== "string") {
        throw internalError("The desktop shell answered save_text_file without a path");
      }
      return path;
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Opens the shell's folder dialog and stores the folder of scheduled backups; resolves with it. */
  async backupFolderChoose(): Promise<string> {
    try {
      const path = await invoke<unknown>("backup_folder_choose");
      if (typeof path !== "string") {
        throw internalError("The desktop shell answered backup_folder_choose without a path");
      }
      return path;
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Writes a scheduled backup into the chosen folder now, prunes old ones and records the outcome. */
  async backupRunNow(): Promise<BackupResult> {
    try {
      return backupResultOf(await invoke<{ summary: BackupSummary; path: string }>("backup_run_now"), "backup_run_now");
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** The backups of a folder for the restore wizard (ADR-0059); without `path`, the shell's folder dialog first. */
  async backupFolderList(path?: string): Promise<BackupFolderListing> {
    try {
      return await invoke<BackupFolderListing>("backup_folder_list", path === undefined ? {} : { path });
    } catch (error) {
      throw toCoreError(error);
    }
  }

  subscribe(onEvent: (event: Event) => void): () => void {
    this.#listeners.add(onEvent);
    this.#ensureChannel();
    return () => {
      this.#listeners.delete(onEvent);
    };
  }

  /** Registers a single channel with the shell, on first use, for all listeners. */
  #ensureChannel(): void {
    if (this.#channel !== undefined) {
      return;
    }
    const channel = new Channel<Event>((event) => {
      for (const listener of [...this.#listeners]) {
        try {
          listener(event);
        } catch (error) {
          // One misbehaving listener must not starve the others.
          console.error("core event listener failed", error);
        }
      }
    });
    this.#channel = channel;
    invoke("subscribe_events", { onEvent: channel }).catch((error: unknown) => {
      // Without the channel the UI still works, it just refreshes after its own calls.
      this.#channel = undefined;
      console.error("could not subscribe to core events", toCoreError(error));
    });
  }

  /**
   * Registers a listener for what the shell tells the page on its own account (ADR-0040): links of the operating
   * system, the quick capture, the tray's lock and the state of the capture shortcut. The shell sends what came
   * before the first listener at once, so the listener may be called before this returns.
   */
  onShellEvent(listener: (event: ShellEvent) => void): () => void {
    this.#shellListeners.add(listener);
    this.#ensureShellChannel();
    return () => {
      this.#shellListeners.delete(listener);
    };
  }

  /** Registers a single channel for the shell's messages with the shell, on first use, for all listeners. */
  #ensureShellChannel(): void {
    if (this.#shellChannel !== undefined) {
      return;
    }
    const channel = new Channel<unknown>((message) => {
      const event = shellEventOf(message);
      if (event === undefined) {
        return;
      }
      for (const listener of [...this.#shellListeners]) {
        try {
          listener(event);
        } catch (error) {
          // One misbehaving listener must not starve the others. A link is never logged.
          console.error("shell event listener failed", error);
        }
      }
    });
    this.#shellChannel = channel;
    invoke("shell_events_subscribe", { onEvent: channel }).catch((error: unknown) => {
      // Without the channel the shell's messages do nothing; the app works as before.
      this.#shellChannel = undefined;
      console.error("could not subscribe to the shell's events", toCoreError(error));
    });
  }

  /** Registers a single channel for the presence frames with the shell, on first use, for all listeners. */
  #ensurePresenceChannel(): void {
    if (this.#presenceChannel !== undefined) {
      return;
    }
    const channel = new Channel<unknown>((message) => {
      const update = presenceUpdateOf(message);
      if (update === undefined) {
        return;
      }
      for (const listener of [...this.#presenceListeners]) {
        try {
          listener(update);
        } catch (error) {
          // One misbehaving listener must not starve the others. A presence frame is never logged.
          console.error("presence listener failed", error);
        }
      }
    });
    this.#presenceChannel = channel;
    invoke("presence_events", { onEvent: channel }).catch((error: unknown) => {
      // Without the channel there are no remote cursors; editing and sync work as before.
      this.#presenceChannel = undefined;
      console.error("could not subscribe to presence", toCoreError(error));
    });
  }

  /**
   * Sets the words of the shell's own surfaces, the tray menu today; only the labels named change.
   * `InvalidInput` for the label that is not valid (nothing changes then).
   */
  async shellLabelsSet(labels: ShellLabels): Promise<void> {
    try {
      await invoke("shell_labels_set", { labels });
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Shows the native menu bar (a lone Alt press); it may already be shown. */
  async menuBarShow(): Promise<void> {
    try {
      await invoke("menu_bar_show");
    } catch (error) {
      throw toCoreError(error);
    }
  }

  /** Hides the native menu bar; it may already be hidden. */
  async menuBarHide(): Promise<void> {
    try {
      await invoke("menu_bar_hide");
    } catch (error) {
      throw toCoreError(error);
    }
  }
}
