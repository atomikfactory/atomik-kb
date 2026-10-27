// Public surface of @atomik-kb/core-client.
//
// The types below are generated from `kb-types` by ts-rs and re-exported from here;
// never import from `./generated` outside this package. `TauriTransport` lives in
// the `@atomik-kb/core-client/tauri` entry so that web builds do not pull in
// `@tauri-apps/api`.
//
// `Request`, `Response` and `Event` are the contract's names (kb-types) and shadow the DOM
// globals of the same name wherever they are imported; import them explicitly, or alias
// them (`import type { Event as CoreEvent }`) next to DOM code.
//
// `MemoryTransport` mirrors the core of Milestones 1 to 8 in memory (tests, `/dev.html`).
// The page document helpers (`portableToLoro`, `loroToPortable`, `normalizePortable`,
// `extractBlocks`, `extractLinks`, `renderMarkdown`, `configurePageDoc`) are what that
// mirror is built from; they are exported for tests and fixtures, not for UI features:
// the UI reads content through `CoreClient` and edits it through the editor binding.
// `parseMarkdown` is the inverse of `renderMarkdown` (Markdown text to portable block nodes
// without ids, `portableNodesToJson` for the plain JSON form); the editor's paste handler uses it.
//
// Recovery kit helpers: `BIP39_ENGLISH` and `resolveKitWord` serve the UI's word entry
// (prefix help, checking typed words against a shown kit); `kitWordsFromEntropy` and
// `parseKitWords` are the mirror's BIP-39 encoding, byte-compatible with the core's words
// and exported for tests. The mirror's code form is not compatible with the core's (its
// checksum is SHA-256, not BLAKE3), so no code helper is exported. `validatePassphrase` and
// `validateKitText` throw the `InvalidInput` errors the core would answer.
//
// Attachments (Milestone 5): the shared limits of `kb_types::attachments` (`CHUNK_SIZE`, ...),
// `validateAttachmentName` and `validateMime` (the core's `InvalidInput` errors, checked by
// `CoreClient.attachmentUpload` before it sends anything) and `sliceIntoChunks` (what the
// upload cuts its source with). `demoImagePng` is a tiny PNG for seeding the mirror
// (`MemoryTransportOptions.attachments`) on the dev page.
//
// Image from URL (ADR-0028): `CoreClient.canImportUrl` and `attachmentImportUrl` (the shell
// downloads the picture once; `MemoryTransportOptions.remoteImages` are the mirror's canned
// links), `parseImageUrl` (the shell's check of a link, with its `InvalidInput` for the field
// `url`, for forms that check as the user types) and the sniffing and naming rules the mirror
// shares with the shell (`sniffImage`, `imageName`, `imageMime`).
//
// Attachment sync (Milestone 8): `CoreClient.attachmentDownload`, the generated
// `AttachmentAvailability` and `TransferInfo`, the download policy setting
// (`DOWNLOAD_POLICY_SETTING` with `DOWNLOAD_POLICY_EAGER` and `DOWNLOAD_POLICY_ON_DEMAND`;
// `downloadPolicyOf` reads a stored value as the core does). The mirror seeds attachments that
// are on the simulated server but not here (`MemoryAttachmentsOptions`, as
// `MemoryTransportOptions.attachmentSync`; the dev page's `?attachments=downloading|waiting|
// on-demand`), and tests move their chunks with `MemoryTransport.advanceTransfers`.
//
// Sync (Milestone 6): `validateServerUrl` and `validateInviteToken` throw the `InvalidInput`
// errors `CoreClient.syncConfigure` checks before it sends anything (for forms that check as the
// user types). The mirror fakes the shell's sync driver on timers; `MemorySyncOptions` and
// `MemorySyncMode` pick the simulated server (`MemoryTransportOptions.sync`, the dev page's
// `?sync=`).
//
// Devices (Milestone 7): `validatePairingCode` (the `InvalidInput` for `code` that
// `CoreClient.pairingStart` checks) and `formatPairingCode` (three groups of four for display);
// `MemoryDevicesOptions` seeds the device log (`?devices=revoked|rotation`) and
// `DeviceLogChange` is what `MemoryTransport.receiveDeviceLog` delivers; the mirror's fixed code,
// digits and peer name (`MEMORY_PAIRING_CODE`, `MEMORY_PAIRING_SAS`, `MEMORY_JOINER_NAME`,
// `EXPIRED_PAIRING_CODE`) are exported for tests.
//
// Typed page properties (ADR-0026): the generated types (`PropertyDef`, `PropertyOption`,
// `PropertyValue`, `PropertyFilter`, `PropertyKind`, ...), the constants of `kb_types::properties`
// (`TAGS_PROPERTY_ID`, `PROPERTY_KINDS`, `OPTION_COLORS`, `STATUS_GROUPS`, `DEFAULT_STATUS_OPTIONS`,
// the `MAX_*` limits) and the pure rules of a value, so that a form can check what it is about to
// send (`isValidPropertyValue`, `normalisePropertyValue`, `propertyValueProblem`, `isValidUrl`);
// `CoreClient.propertySchemaGet`, `pagePropertySet`, `pagesByProperty` and the rest are the
// requests. The mirror seeds schemas and values through `SeedSpace.properties`, `SeedSpace.tagOptions`,
// `SeedPage.properties` and `SeedPage.tags` (see `SeedProperty`, `SeedOption`, `SeedValue`).
//
// Import and export (ADR-0036): the generated types (`ExportPlan`, `ExportBatch`, `ImportReport`, ...),
// `CoreClient.exportRun`, `importRun`, `exportCsv`, `saveTextFile`, `backupFolderChoose` and
// `backupRunNow` (each optional on the transport: `hasExport`, `hasImport`, ...). The mirror answers them
// with reduced fidelity (see `MemoryTransport`): `MemoryTransportOptions.importSources` are the canned
// sources its import dialogs pick from (`demoImportSources` is what the dev page offers), an export lands
// on the fake disk, as text for one page or as a JSON listing that `readMemoryArchive` turns back into
// the files of an import source.
//
// Local AI and MCP (ADR-0037, `docs/architecture/ai.md`): `CoreClient.hasAi()`, `aiStatus`, `aiGenerate`,
// `aiChat`, `aiSearch`, ... and `hasMcp()`, `mcpStatus`, `mcpGrantsSet` (each optional on the transport), the
// core requests `aiIndexStatus`, `aiRetrieve`, `mcpClient*` and `mcpAudit*`. `aiSettingsOf`, `mcpSettingsOf`,
// `defaultModels`, `citationParts` and `normaliseEndpoint` are the pure rules; `encodeWav`, `planSegments`
// and friends (`./audio`) prepare audio for transcription. The mirror answers all of it with
// `MemoryTransportOptions.ai` (`{ mode: "fake" }` or `{ mode: "ollama", baseUrl }`, see `MemoryAiOptions`);
// the hooks of its `ai` (`MemoryAiTransport`) hold, fail or cancel a stream in tests, and
// `MemoryTransport.recordMcpCall` writes a row of the audit log.

export type { AccountId } from "./generated/AccountId";
export type { AiAction } from "./generated/AiAction";
export type { AiChatInput } from "./generated/AiChatInput";
export type { AiChatMessage } from "./generated/AiChatMessage";
export type { AiChunkVector } from "./generated/AiChunkVector";
export type { AiEndpoint } from "./generated/AiEndpoint";
export type { AiGenerateInput } from "./generated/AiGenerateInput";
export type { AiIndexChunk } from "./generated/AiIndexChunk";
export type { AiIndexInfo } from "./generated/AiIndexInfo";
export type { AiIndexPage } from "./generated/AiIndexPage";
export type { AiIndexWork } from "./generated/AiIndexWork";
export type { AiIndexerPhase } from "./generated/AiIndexerPhase";
export type { AiIndexerState } from "./generated/AiIndexerState";
export type { AiModelInfo } from "./generated/AiModelInfo";
export type { AiRole } from "./generated/AiRole";
export type { AiScope } from "./generated/AiScope";
export type { AiSettings } from "./generated/AiSettings";
export type { AiSource } from "./generated/AiSource";
export type { AiStatus } from "./generated/AiStatus";
export type { AiStreamEvent } from "./generated/AiStreamEvent";
export type { AiTone } from "./generated/AiTone";
export type { AiTranscribed } from "./generated/AiTranscribed";
export type { McpAuditEntry } from "./generated/McpAuditEntry";
export type { McpClientCreated } from "./generated/McpClientCreated";
export type { McpClientInfo } from "./generated/McpClientInfo";
export type { McpGrant } from "./generated/McpGrant";
export type { McpOutcome } from "./generated/McpOutcome";
export type { McpReply } from "./generated/McpReply";
export type { McpSettings } from "./generated/McpSettings";
export type { McpStatus } from "./generated/McpStatus";
export type { ApiError } from "./generated/ApiError";
export type { AppLockInfo } from "./generated/AppLockInfo";
export type { AppStatus } from "./generated/AppStatus";
export type { AttachmentAvailability } from "./generated/AttachmentAvailability";
export type { AttachmentId } from "./generated/AttachmentId";
export type { AttachmentInfo } from "./generated/AttachmentInfo";
export type { BackupInterval } from "./generated/BackupInterval";
export type { BackupRunStatus } from "./generated/BackupRunStatus";
export type { BackupSchedule } from "./generated/BackupSchedule";
export type { BackupSummary } from "./generated/BackupSummary";
export type { CsvColumn } from "./generated/CsvColumn";
export type { ExportBatch } from "./generated/ExportBatch";
export type { ExportContent } from "./generated/ExportContent";
export type { ExportEntry } from "./generated/ExportEntry";
export type { ExportFormat } from "./generated/ExportFormat";
export type { ExportPlan } from "./generated/ExportPlan";
export type { ExportWarning } from "./generated/ExportWarning";
export type { ExportWarningCode } from "./generated/ExportWarningCode";
export type { ImportPlanInfo } from "./generated/ImportPlanInfo";
export type { ImportProgress } from "./generated/ImportProgress";
export type { ImportRemoteFile } from "./generated/ImportRemoteFile";
export type { ImportReport } from "./generated/ImportReport";
export type { ImportSpaceInfo } from "./generated/ImportSpaceInfo";
export type { ImportWarning } from "./generated/ImportWarning";
export type { ImportWarningCode } from "./generated/ImportWarningCode";
export type { Backlink } from "./generated/Backlink";
export type { BlockId } from "./generated/BlockId";
export type { ByteRange } from "./generated/ByteRange";
export type { ChangeSource } from "./generated/ChangeSource";
export type { ChecklistItem } from "./generated/ChecklistItem";
export type { ChecklistItemId } from "./generated/ChecklistItemId";
export type { ChunkId } from "./generated/ChunkId";
export type { DateValue } from "./generated/DateValue";
export type { DeviceEntry } from "./generated/DeviceEntry";
export type { DeviceId } from "./generated/DeviceId";
export type { DeviceInfo } from "./generated/DeviceInfo";
export type { DocId } from "./generated/DocId";
export type { ErrorCode } from "./generated/ErrorCode";
export type { Event } from "./generated/Event";
export type { GcSummary } from "./generated/GcSummary";
export type { InstallId } from "./generated/InstallId";
export type { KeyringInfo } from "./generated/KeyringInfo";
export type { OptionColor } from "./generated/OptionColor";
export type { OptionId } from "./generated/OptionId";
export type { PageContent } from "./generated/PageContent";
export type { PageFirstImage } from "./generated/PageFirstImage";
export type { PageExcerpt } from "./generated/PageExcerpt";
export type { BulkAction } from "./generated/BulkAction";
export type { PageFont } from "./generated/PageFont";
export type { PageId } from "./generated/PageId";
export type { PageHistory } from "./generated/PageHistory";
export type { PageInfo } from "./generated/PageInfo";
export type { PageVersion } from "./generated/PageVersion";
export type { PageVersionContent } from "./generated/PageVersionContent";
export type { PageVersionKind } from "./generated/PageVersionKind";
export type { VersionAuthor } from "./generated/VersionAuthor";
export type { PageNode } from "./generated/PageNode";
export type { PageProperties } from "./generated/PageProperties";
export type { PlaceValue } from "./generated/PlaceValue";
export type { PairingInfo } from "./generated/PairingInfo";
export type { PairingRole } from "./generated/PairingRole";
export type { PairingState } from "./generated/PairingState";
export type { FormulaBase } from "./generated/FormulaBase";
export type { FormulaResult } from "./generated/FormulaResult";
export type { FormulaType } from "./generated/FormulaType";
export type { PropertyConfig } from "./generated/PropertyConfig";
export type { PropertyDef } from "./generated/PropertyDef";
export type { PropertyFilter } from "./generated/PropertyFilter";
export type { PropertyId } from "./generated/PropertyId";
export type { PropertyKind } from "./generated/PropertyKind";
export type { PropertyOption } from "./generated/PropertyOption";
export type { PropertySchema } from "./generated/PropertySchema";
export type { PropertyValue } from "./generated/PropertyValue";
export type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
export type { RollupFunction } from "./generated/RollupFunction";
export type { RollupResult } from "./generated/RollupResult";
export type { RollupTarget } from "./generated/RollupTarget";
export type { RollupValue } from "./generated/RollupValue";
export type { RecoveryKit } from "./generated/RecoveryKit";
export type { RecoveryInfo } from "./generated/RecoveryInfo";
export type { RecoveryKitInfo } from "./generated/RecoveryKitInfo";
export type { RecoveryState } from "./generated/RecoveryState";
export type { ReminderDue } from "./generated/ReminderDue";
export type { Request } from "./generated/Request";
export type { Response } from "./generated/Response";
export type { SearchHit } from "./generated/SearchHit";
export type { SearchScope } from "./generated/SearchScope";
export type { SecurityInfo } from "./generated/SecurityInfo";
export type { SpaceId } from "./generated/SpaceId";
export type { SpaceInfo } from "./generated/SpaceInfo";
export type { SpaceMoveSummary } from "./generated/SpaceMoveSummary";
export type { StatusGroup } from "./generated/StatusGroup";
export type { SyncInfo } from "./generated/SyncInfo";
export type { SyncState } from "./generated/SyncState";
export type { TextRange } from "./generated/TextRange";
export type { TransferInfo } from "./generated/TransferInfo";
export type { TrashedPage } from "./generated/TrashedPage";
export type { TreeView } from "./generated/TreeView";
export type { UndoState } from "./generated/UndoState";
export type { Workspace } from "./generated/Workspace";
// Shared spaces (Milestone 13).
export type { ArchiveReason } from "./generated/ArchiveReason";
export type { DisputeInfo } from "./generated/DisputeInfo";
export type { FeedId } from "./generated/FeedId";
export type { FeedSyncInfo } from "./generated/FeedSyncInfo";
export type { InviteFlow } from "./generated/InviteFlow";
export type { InviteId } from "./generated/InviteId";
export type { InviteInfo } from "./generated/InviteInfo";
export type { InviteKind } from "./generated/InviteKind";
export type { InviteLinkInfo } from "./generated/InviteLinkInfo";
export type { InviteSessionInfo } from "./generated/InviteSessionInfo";
export type { InviteSessionState } from "./generated/InviteSessionState";
export type { InviteSide } from "./generated/InviteSide";
export type { InviteStatus } from "./generated/InviteStatus";
export type { MemberInfo } from "./generated/MemberInfo";
export type { MemberRole } from "./generated/MemberRole";
export type { FormerMemberInfo } from "./generated/FormerMemberInfo";
export type { MembersInfo } from "./generated/MembersInfo";
export type { PersonInfo } from "./generated/PersonInfo";
export type { ProfileInfo } from "./generated/ProfileInfo";
export type { SpaceSettings } from "./generated/SpaceSettings";
export type { SpaceSharePlan } from "./generated/SpaceSharePlan";
export type { SpaceShareState } from "./generated/SpaceShareState";
export type { WorkspaceId } from "./generated/WorkspaceId";
// Comments, person mentions and notifications (Milestone 13, slice 2).
export type { CommentAnchor } from "./generated/CommentAnchor";
export type { CommentBodyItem } from "./generated/CommentBodyItem";
export type { CommentId } from "./generated/CommentId";
export type { CommentInfo } from "./generated/CommentInfo";
export type { NotificationInfo } from "./generated/NotificationInfo";
export type { NotificationKind } from "./generated/NotificationKind";
export type { ThreadId } from "./generated/ThreadId";
export type { ThreadInfo } from "./generated/ThreadInfo";
export {
  COMMENT_BODY_MAX_BYTES,
  COMMENT_QUOTE_MAX_CHARS,
  NOTIFICATIONS_DEFAULT_LIMIT,
  NOTIFICATIONS_MAX_LIMIT,
  NOTIFICATIONS_MAX_ROWS,
  commentAnchorProblem,
  commentBodyMentions,
  commentBodyProblem,
  commentBodyText,
  isBase64,
} from "./comments";
export type {
  CommentChange,
  MemoryCommentBody,
  MemoryCommentSeed,
  MemoryNotificationSeed,
  MemoryThreadSeed,
} from "./comments";
// Realtime and presence (Milestone 13, slice 3). `PresenceMessage` is the wire form of the shell's channel (the update
// as an array of numbers); the editor gets `PresenceUpdate` (bytes) through `Transport.presence`.
export type { PresenceInfo } from "./generated/PresenceInfo";
export type { PresenceMessage } from "./generated/PresenceMessage";
export type { PresenceState } from "./generated/PresenceState";
export { PRESENCE_TIMEOUT_SECONDS, PRESENCE_UPDATES_PER_SECOND, PRESENCE_UPDATE_MAX_BYTES } from "./presence";
// View-only links (Milestone 13, slice 4): `CoreClient.viewLinkCreate`, `viewLinkList` and `viewLinkRevoke`, the generated
// `ViewLinkInfo` (its `url` holds the link's key) and `ViewLinkStatus`, and the limits and the policy of the viewer.
export type { LinkId } from "./generated/LinkId";
export type { ViewLinkInfo } from "./generated/ViewLinkInfo";
export type { ViewLinkStatus } from "./generated/ViewLinkStatus";
export {
  LINK_EXPIRY_MAX_DAYS,
  LINK_POLL_SECONDS,
  LINK_REFRESH_DEBOUNCE_MS,
  VIEWER_CSP,
  VIEWER_PATH,
} from "./link";
export type { JsonValue } from "./generated/serde_json/JsonValue";
export type { JsonValue as Json } from "./generated/serde_json/JsonValue";

export type { MemoryAttachmentSeed, MemoryAttachmentsOptions, MemoryRemoteAttachmentSeed } from "./attachment-mirror";
export {
  CHUNK_SIZE,
  DEFAULT_RETENTION_DAYS,
  DOWNLOAD_POLICY_EAGER,
  DOWNLOAD_POLICY_ON_DEMAND,
  DOWNLOAD_POLICY_SETTING,
  MAX_ATTACHMENT_BYTES,
  MAX_MIME_BYTES,
  MAX_NAME_BYTES,
  MAX_PREVIEW_BYTES,
  RETENTION_SETTING,
  downloadPolicyOf,
  sliceIntoChunks,
  validateAttachmentName,
  validateMime,
} from "./attachments";
export type { DownloadPolicy, UploadSource } from "./attachments";
export { fromBase64, toBase64 } from "./backup";
export { CoreClient } from "./client";
export {
  AI_ENDPOINT_SETTING,
  AI_SETTINGS_SETTING,
  DEFAULT_AI_ENDPOINT,
  DEFAULT_MCP_PORT,
  MAX_MCP_PORT,
  MCP_SETTINGS_SETTING,
  MIN_MCP_PORT,
  aiSettingsOf,
  citationParts,
  defaultModels,
  isLocalHost,
  isMcpPort,
  mcpSettingsOf,
  normaliseEndpoint,
} from "./ai";
export type { CitationPart } from "./ai";
export { FAKE_MODELS } from "./memory-ai";
export type { AiRecordedCall, MemoryAiOptions, MemoryAiTransport, MemoryAiTuning } from "./memory-ai";
export type { McpCallRecord } from "./memory-mcp";
export {
  SEGMENT_MAX_SECONDS,
  SEGMENT_MIN_SECONDS,
  TRANSCRIPTION_RATE,
  downmixAndResample,
  encodeWav,
  floatToInt16,
  planSegments,
} from "./audio";
export type { SegmentPlan } from "./audio";
export type {
  PageCreateInput,
  PairingStartOptions,
  PropertyOptionPatch,
  SearchOptions,
  UploadProgress,
} from "./client";
export {
  EXPIRED_PAIRING_CODE,
  MEMORY_JOINER_NAME,
  MEMORY_PAIRING_CODE,
  MEMORY_PAIRING_SAS,
} from "./devices-mirror";
export { DEMO_IMAGE_SIZE, demoImagePng } from "./demo-image";
export { demoPdf } from "./demo-pdf";
export { demoWav } from "./demo-audio";
export { demoImportSources } from "./demo-import";
export { CoreError, internalError, isApiError, toCoreError } from "./errors";
export { KIT_WORDS, kitWordsFromEntropy, parseKitWords, resolveKitWord } from "./kit";
export { mathInlineMarkdown, personMentionMarkdown, renderMarkdown } from "./markdown";
export type { MarkdownOptions } from "./markdown";
export { parseMarkdown, portableNodesToJson } from "./markdown-parse";
export { MemoryTransport } from "./memory";
export type { MemoryTransportOptions } from "./memory";
export { encodeMemoryArchive, readMemoryArchive } from "./memory-export";
export type { MemoryArchiveEntry } from "./memory-export";
export type { MemoryImportSource } from "./memory-import";
export type { RemoteImage } from "./memory-url-images";
export {
  DEFAULT_STATUS_OPTIONS,
  MAX_CHECKLIST_ITEMS,
  MAX_CHECKLIST_TEXT_CHARS,
  MAX_EMAIL_CHARS,
  MAX_FILES,
  MAX_FILTER_CHARS,
  MAX_NAME_CHARS,
  MAX_OPTIONS,
  MAX_PEOPLE,
  MAX_PHONE_CHARS,
  MAX_PLACE_NAME_CHARS,
  MAX_PROPERTIES,
  MAX_RELATED_PAGES,
  MAX_SELECTED_OPTIONS,
  MAX_TEXT_CHARS,
  MAX_URL_CHARS,
  OPTION_COLORS,
  PROPERTY_KINDS,
  STATUS_GROUPS,
  TAGS_NAME,
  TAGS_PROPERTY_ID,
  COMPUTED_PROPERTY_MESSAGE,
  checklistProgress,
  codePointLength,
  copyFormulaResult,
  copyPropertyValue,
  copyRollupResult,
  formulaResultsEqual,
  isComputedPropertyKind,
  isEmptyPropertyValue,
  isValidPropertyValue,
  isValidUrl,
  normalisePropertyValue,
  propertyKindHasOptions,
  propertyValueFiles,
  propertyValueKind,
  propertyValueOptions,
  propertyValuePages,
  propertyValueProblem,
  propertyValueText,
  propertyValuesEqual,
} from "./properties";
export type { DefaultStatusOption, SeedChecklistItem, SeedOption, SeedProperty, SeedValue } from "./properties";
export {
  ROLLUP_FUNCTIONS,
  ROLLUP_TITLE_TARGET,
  evaluateRollup,
  isRollupTargetKind,
  rollupFunctionAccepts,
  rollupFunctionsFor,
} from "./rollup";
export {
  FORMULA_FUNCTIONS,
  FORMULA_TITLE_ID,
  MAX_FORMULA_CHARS,
  MAX_FORMULA_DEPTH,
  MAX_FORMULA_LIST_ITEMS,
  MAX_FORMULA_STEPS,
  checkDisplayFormula,
  checkFormula,
  evaluateFormula,
  formatFormulaValue,
  formulaReferencedIds,
  formulaToDisplay,
  formulaToStored,
  isFormulaError,
  parseFormula,
  remapFormulaIds,
  tokenizeFormula,
} from "./formula";
export type {
  FormulaEnv,
  FormulaError,
  FormulaErrorCode,
  FormulaFunctionInfo,
  FormulaHost,
  FormulaProperty,
  FormulaSchema,
  FormulaToken,
  ParsedFormula,
} from "./formula";
export { formulaSchemaOf } from "./memory-formula";
export type { FormulaSource } from "./memory-formula";
export {
  CATCH_UP_SECONDS,
  FIRES_PER_TICK,
  LAST_CHECKED_SETTING as REMINDERS_LAST_CHECKED_SETTING,
  MAX_REMINDERS_DUE,
  localNowSeconds,
} from "./reminders";
export type { DeviceLogChange, MemoryDevicesOptions, MemorySecurityOptions } from "./security";
export {
  MAX_REMINDER_MINUTES,
  PAGE_MARKS,
  WHOLE_DAY_MOMENT_MINUTES,
  configurePageDoc,
  dateMentionText,
  extractAttachmentRefs,
  extractBlocks,
  extractDateMentions,
  extractLinks,
  extractPersonMentions,
  galleryItemsOf,
  isIsoDate,
  isIsoTime,
  loroToPortable,
  mathInlineSource,
  normalizePortable,
  personMentionLabel,
  portableToLoro,
  reminderFireAtLocal,
  reminderMinutesOf,
} from "./page-doc";
export {
  WHITEBOARDS,
  WHITEBOARD_ORIGIN,
  orphanWhiteboardKeys,
  readWhiteboard,
  sortZ as sortWhiteboardElements,
  splitWhiteboardKey,
  whiteboardElementText,
  whiteboardKey,
  whiteboardTexts,
  writeWhiteboardElements,
} from "./whiteboard";
export type { WhiteboardElement, WhiteboardValue } from "./whiteboard";
export type {
  GalleryItem,
  PageAttachmentRef,
  PageBlock,
  PageDateMention,
  PageInline,
  PageLink,
  PagePersonMention,
  PortableMark,
  PortableNode,
} from "./page-doc";
export type { SeedPage, SeedSpace, StoredPageFont } from "./structure";
export {
  DEAD_LINK_SECRET,
  EXPIRED_INVITE_CODE,
  INVITE_CODE_TTL_MS,
  INVITE_DEAD_MESSAGE,
  INVITE_LINK_DEFAULT_DAYS,
  INVITE_MAX_DAYS,
  MAX_MEMBERS,
  MAX_OPEN_INVITES,
  MAX_TRASH_RETENTION_DAYS,
  MEMORY_INVITEE_NAME,
  MEMORY_INVITE_CODE,
  MEMORY_INVITE_SAS,
  PROFILE_NAME_MAX_CHARS,
} from "./sharing-mirror";
export type {
  MemoryInvitation,
  MemorySeedInvite,
  MemorySeedMember,
  MemorySharedSpaceSeed,
  MemorySharingOptions,
  MemorySharingStart,
  SharingChange,
} from "./sharing-mirror";
// The in-memory presence (Milestone 13, slice 3): the simulated people of `?presence=demo` and the test hooks.
export { DEFAULT_PRESENCE_STEP_MS } from "./memory-presence";
export type {
  MemoryPresenceOptions,
  MemoryPresencePeer,
  MemoryPresenceStop,
  PresenceChange,
  PresenceSelect,
  SentPresence,
} from "./memory-presence";
// The in-memory view links (Milestone 13, slice 4): the links a dev page or a test starts with.
export { DEFAULT_LINK_PUBLISH_MS } from "./memory-viewlinks";
export type { MemoryViewLinkSeed, MemoryViewLinksOptions } from "./memory-viewlinks";
export type { MemorySyncMode, MemorySyncOptions } from "./sync-mirror";
export {
  DEFAULT_TRASH_RETENTION_DAYS,
  TRASH_EXPIRY_INTERVAL_MS,
  TRASH_RETENTION_SETTING,
  isTrashExpired,
  trashDaysLeft,
  trashRetentionDays,
  trashRetentionMs,
} from "./trash";
export type {
  AiTransport,
  AttachmentSaveResult,
  AttachmentTransport,
  BackupResult,
  CoreStateInfo,
  DeepLink,
  DocTransport,
  ExportRunOptions,
  ExportRunResult,
  ImportRunOptions,
  ImportSourceKind,
  McpTransport,
  MenuCommand,
  PresenceTransport,
  PresenceUpdate,
  ShellEvent,
  ShellLabels,
  ShortcutState,
  Transport,
} from "./transport";
export { MAX_IMAGE_URL_CHARS, imageMime, imageName, parseImageUrl, sniffImage } from "./url-images";
export type { ImageKind, UrlFailureReason } from "./url-images";
export {
  MAX_INVITE_TOKEN_CHARS,
  MAX_SERVER_URL_CHARS,
  PAIRING_CODE_LEN,
  PASSPHRASE_MAX_BYTES,
  PASSPHRASE_MIN_BYTES,
  formatPairingCode,
  inviteToken as validateInviteToken,
  kitText as validateKitText,
  pairingCode as validatePairingCode,
  passphrase as validatePassphrase,
  serverUrl as validateServerUrl,
} from "./validate";
// Local dictation (ADR-0053, `docs/architecture/dictation.md` section 5): `DictationApi` and its session, the
// types the desktop shell exchanges, `joinPhrase` (what a recognised phrase becomes at the cursor, the TypeScript
// mirror of `kb_speech::policy::join_phrase` with the same test vectors), the parsers of the shell's messages
// and the in-memory fake (`MemoryDictation`, the dev page's `?dictation=fake`). The desktop implementation
// `TauriDictation` is in the `@atomik-kb/core-client/tauri` entry. `Transport.dictation` is optional, and
// `CoreClient.hasDictation()` / `dictation()` hand it out (`MemoryTransportOptions.dictation`: the fake's options or `"fake"`).
export {
  DICTATION_CHUNK_MS,
  DICTATION_ERROR_CODES,
  DICTATION_MAX_PUSH_SAMPLES,
  DICTATION_RATE,
  PHRASE_CLOSERS,
  PHRASE_OPENERS,
  SPEECH_ENGINES,
  dictationEventOf,
  dictationReasonOf,
  dictationStatusOf,
  installEventOf,
  joinPhrase,
  lastCharacterOf,
  newSessionId,
} from "./dictation";
export type {
  DictationApi,
  DictationErrorCode,
  DictationEvent,
  DictationSession,
  DictationState,
  DictationStatus,
  EngineStatus,
  InstallEvent,
  SpeechEngine,
  UnavailableReason,
} from "./dictation";
export { FAKE_PHRASES, MemoryDictation } from "./dictation-memory";
// Google Drive backups (ADR-0059): `DriveApi`, its error reasons and the in-memory fake (`?drive=fake`); `TauriDrive` is in the
// `@atomik-kb/core-client/tauri` entry.
export { DRIVE_ERROR_REASONS, driveReasonOf } from "./drive";
export type { DriveApi, DriveErrorReason } from "./drive";
export { FAKE_DRIVE_ACCOUNT, FAKE_DRIVE_FOLDER_ID, FAKE_DRIVE_FOLDER_URL, FAKE_RESTORE_STAGING, MemoryDrive, driveFailure } from "./drive-memory";
export type {
  MemoryDriveBackup,
  MemoryDriveHost,
  MemoryDriveOperation,
  MemoryDriveOptions,
} from "./drive-memory";
export type { BackupFileInfo } from "./generated/BackupFileInfo";
export type { BackupFolderListing } from "./generated/BackupFolderListing";
export type { DriveClientInput } from "./generated/DriveClientInput";
export type { DriveClientSource } from "./generated/DriveClientSource";
export type { DriveConnection } from "./generated/DriveConnection";
export type { DriveProgress } from "./generated/DriveProgress";
export type { DriveStatus } from "./generated/DriveStatus";
export type { MemoryDictationOptions } from "./dictation-memory";
export { API_VERSION } from "./version";
export { BIP39_ENGLISH } from "./wordlist-english";
