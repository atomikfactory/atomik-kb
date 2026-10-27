import type { LoroDoc } from "loro-crdt";

import { AI_ENDPOINT_SETTING } from "./ai";
import { MemoryAttachments, attachmentHeader } from "./attachment-mirror";
import type { MemoryAttachmentSeed, MemoryAttachmentsOptions } from "./attachment-mirror";
import { CHUNK_SIZE, DOWNLOAD_POLICY_SETTING, MAX_ATTACHMENT_BYTES, MAX_PREVIEW_BYTES } from "./attachments";
import { backupSummary, encodeBackup, readBackupFile, readBackupState, toBase64 } from "./backup";
import type { BackupHeader, BackupState } from "./backup";
import { MemoryComments } from "./comments-mirror";
import type { CommentChange } from "./comments";
import { MemoryContent } from "./content";
import { MemoryDictation } from "./dictation-memory";
import type { MemoryDictationOptions } from "./dictation-memory";
import { MemoryDrive, driveFailure } from "./drive-memory";
import type { MemoryDriveOptions } from "./drive-memory";
import { DEFAULT_PAIRING_STEP_MS, MemoryPairing, MemoryRecovery } from "./devices-mirror";
import { CoreError, internalError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { ApiError } from "./generated/ApiError";
import type { AppStatus } from "./generated/AppStatus";
import type { AttachmentId } from "./generated/AttachmentId";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { BackupFolderListing } from "./generated/BackupFolderListing";
import type { BackupRunStatus } from "./generated/BackupRunStatus";
import type { BackupSchedule } from "./generated/BackupSchedule";
import type { ByteRange } from "./generated/ByteRange";
import type { ChangeSource } from "./generated/ChangeSource";
import type { ChunkId } from "./generated/ChunkId";
import type { DeviceInfo } from "./generated/DeviceInfo";
import type { Event } from "./generated/Event";
import type { AiScope } from "./generated/AiScope";
import type { ExportWarning } from "./generated/ExportWarning";
import type { ImportReport } from "./generated/ImportReport";
import type { McpAuditEntry } from "./generated/McpAuditEntry";
import type { McpGrant } from "./generated/McpGrant";
import type { ImportWarning } from "./generated/ImportWarning";
import type { PageId } from "./generated/PageId";
import type { PresenceState } from "./generated/PresenceState";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SpaceId } from "./generated/SpaceId";
import type { ThreadInfo } from "./generated/ThreadInfo";
import type { SyncInfo } from "./generated/SyncInfo";
import type { Workspace } from "./generated/Workspace";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { parseKit } from "./kit";
import {
  BACKUP_DRIVE_SETTING,
  BACKUP_FOLDER_SETTING,
  BACKUP_LAST_SETTING,
  BACKUP_SCHEDULE_SETTING,
  DEVICE_BOUND_SETTINGS,
  autoBackupName,
  backupNameTime,
  backupPeriodOver,
  backupsToPrune,
  isAutoBackupName,
  scheduleOf,
} from "./memory-backup-schedule";
import { MemoryAi } from "./memory-ai";
import type { MemoryAiOptions, MemoryAiTransport } from "./memory-ai";
import { blocksOfPortable } from "./memory-ai-text";
import type { AiPageText } from "./memory-ai-text";
import { MemoryBulk } from "./memory-bulk";
import { MemoryDuplicate } from "./memory-duplicate";
import { MemoryHistory } from "./memory-history";
import { MemoryExport, encodeMemoryArchive } from "./memory-export";
import type { MemoryArchiveEntry } from "./memory-export";
import { appFlowyPrefix } from "./memory-import-appflowy";
import {
  MAX_IMPORT_FILE_BYTES,
  MAX_REPORT_WARNINGS,
  MemoryImport,
  decodeSourceText,
  isSafeRelativePath,
  isTextFile,
  isSkippedPath,
} from "./memory-import";
import type { MemoryImportSource } from "./memory-import";
import { MemoryMcp } from "./memory-mcp";
import type { McpCallRecord } from "./memory-mcp";
import { MemoryMove } from "./memory-move";
import { MemoryPresence } from "./memory-presence";
import type { MemoryPresenceOptions, PresenceChange, SentPresence } from "./memory-presence";
import { MemoryProperties } from "./memory-properties";
import type { SeededSpace } from "./memory-properties";
import { MemoryViewLinks } from "./memory-viewlinks";
import type { MemoryViewLinksOptions } from "./memory-viewlinks";
import { loadRemoteImage } from "./memory-url-images";
import type { RemoteImage } from "./memory-url-images";
import { LAST_CHECKED_SETTING, MAX_REMINDERS_DUE, localNowSeconds, planTick, windowStart } from "./reminders";
import { MemorySecurity, lockNotEnabled, recoveryIdOf } from "./security";
import { DEFAULT_SHARING_STEP_MS, MemorySharing } from "./sharing-mirror";
import type { MemorySharingOptions, SharingChange } from "./sharing-mirror";
import type { DeviceLogChange, DevicesChange, MemoryDevicesOptions, MemorySecurityOptions } from "./security";
import { MemoryStructure } from "./structure";
import type { PageMeta, SeedSpace } from "./structure";
import { DEFAULT_SYNC_STEP_MS, MemorySync, isLocalEdit } from "./sync-mirror";
import type { MemorySyncMode, MemorySyncOptions } from "./sync-mirror";
import { TRASH_EXPIRY_INTERVAL_MS, TRASH_RETENTION_SETTING, isTrashExpired, trashRetentionMs } from "./trash";
import type {
  AttachmentSaveResult,
  AttachmentTransport,
  BackupResult,
  CoreStateInfo,
  DocTransport,
  ExportRunOptions,
  ExportRunResult,
  ImportRunOptions,
  ImportSourceKind,
  McpTransport,
  PresenceTransport,
  PresenceUpdate,
  Transport,
} from "./transport";
import { importName, mimeForName, pathSafeName, runsCode, sanitisedName } from "./shell-names";
import { URL_MESSAGES, imageMime, imageName, parseImageUrl, sniffImage, urlFailure } from "./url-images";
import { coverAttachment, displayName, iconAttachment, invalid } from "./validate";
import { API_VERSION } from "./version";

/** Largest accepted serialised setting value, as in `kb-core` (`MAX_SETTING_BYTES`). */
const MAX_SETTING_BYTES = 64 * 1024;
const MAX_SETTING_KEY_BYTES = 128;
const SETTING_NAMESPACES = ["ui", "app"];
const MAX_WORKSPACE_NAME_CHARS = 200;
const MAX_FIRST_SPACE_NAME_CHARS = 100;
const MAX_DEVICE_NAME_CHARS = 100;

const utf8 = new TextEncoder();

/** Largest text `saveTextFile` writes (the shell's limit, 64 MiB). */
const MAX_SAVED_TEXT_BYTES = 64 * 1024 * 1024;
/** The folder the simulated folder dialog picks (for scheduled backups and for the restore wizard's list), unless told otherwise. */
const SCHEDULED_BACKUP_FOLDER = "/backups/scheduled";
/** Where a scheduled run that has no folder writes its backup before it is delivered (the shell's `<app data>/backup-staging`). */
const BACKUP_STAGING_FOLDER = "/backup-staging";
/** The most backups `backupFolderList` answers (the shell's limit). */
const MAX_LISTED_BACKUPS = 200;

/** What `MemoryTransport` needs to know about the (fake) platform; all optional. */
export interface MemoryTransportOptions {
  /**
   * Overrides for the static fields of `AppStatus`. Defaults mimic the in-memory test
   * database of `kb-core`: schema 1, no encryption (`dbCipher: "none"`), the current
   * `API_VERSION`, platform `"memory"`. Set `apiVersion` to exercise the version
   * mismatch warning, `dbEncrypted` and `dbCipher` to look like a real desktop build.
   */
  status?: Partial<Omit<AppStatus, "workspaceInitialized" | "deviceId">>;
  /** Initial device name. Defaults like the core: `"<Platform> device"`. */
  deviceName?: string;
  /** Start with this workspace already created (onboarding done). */
  workspaceName?: string;
  /**
   * Start with these spaces (in this order) and their pages already created, without
   * events and without undo history, as if they had been there before the app started.
   * Pages with `content` (portable JSON) start with that document. A space can also start with
   * a property schema and Tags options, and its pages with values (`SeedSpace.properties`,
   * `SeedSpace.tagOptions`, `SeedPage.properties`, `SeedPage.tags`; ADR-0026): created last of
   * all seeds, so that they never shift the ids of spaces, pages, kits and attachments.
   */
  spaces?: readonly SeedSpace[];
  /** Settings present from the start. */
  settings?: Readonly<Record<string, JsonValue>>;
  /**
   * How far the device's wall clock is ahead of UTC, in seconds, at a time given in Unix ms (east positive, daylight
   * saving included): the offset the formulas' `today()` and the local day of a page's creation time are read with
   * (`Clock::local_offset_seconds`, ADR-0055). Default: the JavaScript date's own offset.
   */
  localOffsetSeconds?: (atMs: number) => number;
  /**
   * Initial core lifecycle state. Defaults to `"ready"`. `"locked"` (like
   * `security.startLocked`) needs `security.appLockPassphrase`, the passphrase that unlocks.
   */
  coreState?: CoreStateInfo;
  /**
   * Starting situation of the recovery kit, the app lock and the device log (Milestone 4):
   * a kit issued during onboarding, the app lock enabled, the app starting locked.
   */
  security?: MemorySecurityOptions;
  /**
   * The fake disk of `backupExport` and `backupImport`, and of the attachment transport's
   * `importPath` (reads a file) and `saveAs` (writes one): file path to bytes. Give two
   * transports the same map to restore one's backup into the other, as on a wiped machine.
   * Defaults to a new, empty map; the transport exposes it as `files`.
   */
  files?: Map<string, Uint8Array>;
  /**
   * Makes the shell's file dialogs (`backupExport()`, `backupImport()`, `attachments.importPath()`
   * and `attachments.saveAs()` without a path) behave as if the user cancelled them:
   * `InvalidInput` for the field `path`. Also switchable later with `setCancelDialogs`.
   */
  cancelDialogs?: boolean;
  /**
   * The folder the simulated folder dialog picks, for `backupFolderChoose()` and for `backupFolderList()` without a path
   * (default `/backups/scheduled`). Cancelled with `cancelDialogs` like the file dialogs.
   */
  dialogFolder?: string;
  /**
   * Attachments present from the start (finished uploads), created after the spaces, pages and
   * security seeds without events. Each goes to `spaceId` (default: the first space) under
   * `attachmentId` (default: the next `newId`), so that seeded page content can show it with an
   * `image` or `file` block; references are derived from that content.
   */
  attachments?: readonly MemoryAttachmentSeed[];
  /**
   * A stand-in for the desktop shell's media protocol (ADR-0032): `attachments.mediaUrl` answers
   * with what this returns. Absent (the default, like the web build), the transport has no such
   * URL and a player reads the bytes and makes a blob. The URL is not served by anything here.
   */
  mediaUrl?: (attachmentId: AttachmentId) => string | null;
  /**
   * What `attachments.importUrl` ("Image from URL", ADR-0028) finds at an address: the link as
   * typed (trimmed) or as parsed (`new URL(...).href`) to the bytes and the `Content-Type` the
   * server would send. Looked up first; an address that is not in the map goes to the browser's
   * `fetch`, with the shell's checks (which a web page can mostly not pass: CORS). Default: none.
   */
  remoteImages?: ReadonlyMap<string, RemoteImage>;
  /**
   * The attachment side of sync (Milestone 8, see `MemoryAttachmentsOptions`): the download
   * policy, attachments on the simulated server but not here (`remoteAttachments`, created after
   * `attachments`), and a timer for the transfers (`transferStepMs`). Default: the policy as the
   * settings say (eager when absent), no remote attachment, transfers only through
   * `advanceTransfers`.
   */
  attachmentSync?: MemoryAttachmentsOptions;
  /**
   * Starting situation of sync (Milestone 6) and the simulated server: connected and idle,
   * offline, forked or failing (see `MemorySyncOptions`). Default: sync disabled, and a healthy
   * server for a later `syncConfigure`.
   */
  sync?: MemorySyncOptions;
  /** Length of one step of the fake sync driver, in milliseconds (default 150). */
  syncStepMs?: number;
  /**
   * The device log's starting situation (Milestone 7): more devices, this device revoked, a
   * rotation pending (see `MemoryDevicesOptions`). Needs `security.kitCreatedAt`.
   */
  devices?: MemoryDevicesOptions;
  /** Length of one step of the fake pairing and recovery tasks, in milliseconds (default 150). */
  pairingStepMs?: number;
  /**
   * Shared spaces (Milestone 13, `./sharing-mirror`): spaces of the seeds that start shared with members of each
   * role (or as an archive), this account's profile name, what a code or link the imaginary inviter gives offers, and
   * a session that starts under way. Default: every space is personal.
   */
  sharing?: MemorySharingOptions;
  /**
   * Presence in shared spaces (Milestone 13, slice 3, `./memory-presence`): simulated members of the shared spaces who walk
   * routes of pages and move a caret through the words of the page they edit (the dev page: `?presence=demo`). Without it
   * nobody is present until a test says so (`MemoryTransport.receivePresence`).
   */
  presence?: MemoryPresenceOptions;
  /**
   * View-only links (Milestone 13, slice 4, `./memory-viewlinks`): the links that exist from the start (live, still publishing,
   * expiring or expired) and how long a new link's first upload takes (the dev page: `?links=`). Without it a space has no links
   * until a test or the UI makes one.
   */
  links?: MemoryViewLinksOptions;
  /**
   * What the simulated import dialogs (`importRun`, ADR-0036) can pick: source name to the files of
   * a source of a kind (`files`, `folder` or `zip`, as the person's choice names it). A dialog picks
   * the first source of the kind asked for; none of that kind (or `cancelDialogs`) is a cancelled
   * dialog (`InvalidInput` for `path`). A `zip` is named by its file name (`Notion export.zip`: the
   * source name for the container page is the name without `.zip`), a `folder` by its folder name,
   * `files` have no name. Files are paths relative to the source's root; text for the files the
   * importer reads as text (`.md`, `.markdown`, `.txt`, `.html`, `.htm`, `.csv`, `.excalidraw`,
   * `atomik-export.json`), bytes for the others (bytes are decoded as UTF-8, or UTF-16 with a byte
   * order mark, for a text file). The shell's skips apply: hidden entries (a segment starting with
   * `.`), `__MACOSX` and `node_modules`. Default: none.
   */
  importSources?: Readonly<Record<string, MemoryImportSource>>;
  /**
   * The shell's local AI (ADR-0037, `./memory-ai`): `{ mode: "fake" }` answers deterministically, streamed in a
   * few deltas, with a made-up list of models (the UI tests and `?ai=fake`); `{ mode: "ollama", baseUrl }` calls
   * a real Ollama from the browser (`?ai=ollama`). Gives the transport `ai` and `mcp` (the shell commands);
   * without it `CoreClient.hasAi()` and `hasMcp()` are false. The core's side (`AiIndexStatus`, `AiIndexClear`,
   * `AiRetrieve`, the MCP clients and audit log) works with or without it.
   */
  ai?: MemoryAiOptions;
  /**
   * Local dictation (ADR-0053, `./dictation-memory`): a `MemoryDictationOptions` for the fake (scripted phrases for the
   * audio pushed, the engines and what is installed, failures to provoke), or `"fake"` for its defaults (the dev page's
   * `?dictation=fake`). Gives the transport `dictation`; without it `CoreClient.hasDictation()` is false and the UI
   * hides `/microphone` and the Settings section.
   */
  dictation?: MemoryDictationOptions | "fake";
  /**
   * Google Drive as a backup target and a restore source (ADR-0059, `./drive-memory`): a `MemoryDriveOptions` for the fake (a
   * consent to give or refuse, an account, backups in the folder, failures to provoke), or `"fake"` for its defaults (the dev
   * page's `?drive=fake`). Gives the transport `drive`; without it `CoreClient.hasDrive()` is false and the UI hides the Drive
   * target and the wizard's Drive source. The scheduled run (`backupRunNow`) delivers to it when it is connected.
   */
  drive?: MemoryDriveOptions | "fake";
  /** Clock in Unix milliseconds. Defaults to `Date.now`. */
  now?: () => number;
  /** Id generator (UUID text). Defaults to random UUID v4. */
  newId?: () => string;
  /**
   * Becomes the transport's `openExternal` (for example a spy in tests). Without it the
   * transport has no `openExternal`, so the UI falls back to `window.open` as in a browser.
   */
  openExternal?: (url: string) => Promise<void>;
}

/**
 * An in-memory implementation of the core API (Milestones 1 to 8) for UI tests and for
 * developing the UI without the Tauri shell. It follows the semantics of `kb-core`:
 *
 * - names are trimmed, must be 1 to 200 characters (workspace), 1 to 100 (space, device)
 *   and must not contain control characters; page titles are trimmed, 0 to 500
 *   characters without control characters; icons are trimmed, 1 to 32 bytes, and `null`
 *   clears one;
 * - setting keys look like `namespace.name`, namespaces `ui` and `app`, lowercase
 *   `[a-z0-9_.-]`, at most 128 bytes; values are JSON, at most 64 KiB serialised;
 * - validation failures are `InvalidInput` errors whose `details.field` names the
 *   offending field; creating a second workspace is `AlreadyExists`; unknown spaces and
 *   pages are `NotFound`;
 * - spaces are ordered; each has a page tree with stable ids, a trash (`PageTrash`,
 *   `PageRestore`, `PagePurge`; a restored page returns to its previous parent when that
 *   page is live, else to the end of the root) and a structural undo history of 100
 *   steps (see `./structure`); `index` arguments clamp to the sibling count;
 * - a space has favourite pages (ADR-0029, `PageFavoriteSet`, `PageFavoriteMove`,
 *   `TreeView.favorites`, see `./structure`): kept in order, hidden while the page is in the
 *   trash, dropped by a purge, never on the undo history;
 * - requests emit the events the core emits, in the same order (`WorkspaceCreated`,
 *   `SettingChanged`, `DeviceRenamed`, `SpacesChanged`, `StructureChanged`), and events
 *   reach listeners before the `dispatch` promise resolves, as in the Tauri shell, which
 *   sends them first;
 * - every page has one Loro document (shape v1, `./page-doc`), created on first use like
 *   the core creates it (root plus one empty paragraph with a fresh id) or seeded from
 *   `SeedPage.content`; `docs` serves it as raw Loro bytes (`open` = snapshot, `import` =
 *   editor updates, refused when empty or above 16 MiB, `export` = what an encoded
 *   version vector lacks, empty when nothing, `close` keeps it); unknown and purged pages
 *   are `NotFound`, trashed pages open;
 * - an import emits `PageContentChanged { source: "Editor" }`, then `BacklinksChanged` for
 *   every page the page started or stopped referencing; `PageTrash` and `PageRestore` emit
 *   `BacklinksChanged` for the pages referenced from the moved subtree, `PagePurge` (and
 *   an undo or redo that removes pages) for the pages the removed ones referenced; a
 *   purged page's document is kept, so an undone purge brings the content back;
 * - `Search` (`./search`): the query is validated like a name (trimmed, 1 to 200
 *   characters), then cut into words like the core's FTS5 query (whitespace and
 *   `" * ( ) : ^ - +` separate, bare `AND`/`OR`/`NOT`/`NEAR` are dropped); matching is
 *   case- and diacritics-insensitive, the last token of each word a prefix, every word
 *   somewhere in the page (ADR-0040: its title, a block or a property); trashed pages are
 *   excluded, an unknown `spaceId` is `NotFound`; the rows that hold every word come first,
 *   title hits, then block hits, each in display order (no bm25 ranking), then one hit for
 *   each page whose words are spread over its rows, then the fuzzy title hits
 *   (`./search-fuzzy`) while there is room; `editedAfter` (Unix ms) keeps the pages whose
 *   `updatedAt` is that or later; `limit` clamps to 1..=100, default 20; snippets are chosen like FTS5's
 *   `snippet()` with 12 tokens, `highlights` are UTF-16 ranges; page references count as
 *   the referenced page's current title, hard breaks as spaces; property values count too
 *   (below): a hit on one is a page-level hit (no `blockId`, the property's text as the snippet),
 *   found by every word inside that one text, never by `scope: "Titles"`, listed after the block
 *   hits;
 * - `BacklinksGet` lists live source pages by page id, then block id, with the block's
 *   indexed text (at most 200 characters); `PageContentGet` is portable JSON,
 *   `PageExportMarkdown` the Markdown mirror (`./markdown`); `IndexRebuild` derives
 *   everything again (unknown space: `NotFound`);
 * - reminders on date mentions (ADR-0024, `./reminders`): `RemindersDue { until }` lists the
 *   reminders of live pages whose fire time (the mention's moment minus its `reminder`, on the
 *   local wall clock) is after the setting `app.reminders.last_checked` (seven days back at most)
 *   and not after `until`, oldest first; `remindersTick()` is the scheduler of the core (there
 *   is no shell around this core to run it): it emits `ReminderDue` once per reminder, at most
 *   25 per tick, and moves the mark;
 * - trash expiry (ADR-0040, `./trash`): a top-level trashed page is purged once its `trashedAt` is older than the
 *   setting `app.trash.retention_days` (days, fractions allowed, default 30, a value that is not a number is the
 *   default, `<= 0` = never; a page is still there at exactly the limit), with a page's subtree, as `PagePurge`
 *   would purge it (the events of a run come once each). It runs at every unlock (a seeded core has no trashed page
 *   to expire when it opens), and `remindersTick()` runs it once an hour of this transport's clock (the shell's loop
 *   does the same); `trashExpire()` runs it at once and answers how many top-level pages it purged (`0` while the
 *   core is not ready);
 * - typed page properties (ADR-0026, `./memory-properties`, the rules of a value in `./properties`):
 *   a space has a schema (Tags, a built-in multi-select with the fixed id `TAGS_PROPERTY_ID`,
 *   listed first before anything writes it, then up to 99 more) and a page at most one value per
 *   property; the ten kinds are text, number, select, multi-select, date, checkbox, URL, status
 *   (starts with "Not started", "In progress" and "Done") and email and phone. `PropertySchemaGet`,
 *   `PropertyCreate`, `PropertyRename`, `PropertyDelete` (Tags cannot go), `PropertyMove` (Tags
 *   stays first), `PropertyOptionAdd`, `PropertyOptionUpdate` and `PropertyOptionDelete` (which
 *   also takes the option out of every page's value) answer with the core's codes, fields and
 *   messages; `PagePropertySet` (live or trashed pages; the kind must be the property's, options
 *   must belong to it, an empty text, URL or list clears) and `PagePropertiesGet` answer the page's
 *   values in schema order, `PagesByProperty` lists the live pages that match one of the six
 *   filters, ordered by title (ASCII-only case folding, then exact, then page id). `PageInfo` (page
 *   reads, the tree, the trash) carries the same list as `properties`, left out when empty. A
 *   schema change emits `PropertySchemaChanged` then `StructureChanged`, a value change
 *   `StructureChanged` only (also when nothing changed); a real value change sets the page's
 *   `updatedAt`. Schema and values are not part of structural undo (a purged page's values come
 *   back with it), `SpaceDelete` drops the schema, a backup carries both. Deleting a property hides
 *   its values at once; they go for good with the next write of that page's values;
 * - keys and identity (`./security`, with the codes, fields and messages of `kb-core`'s
 *   `security.rs`): `RecoveryKitCreate` needs the workspace (`InvalidInput` for `workspace`)
 *   and no kit (`AlreadyExists`); it generates 32 random bytes and answers the 24 BIP-39
 *   words and the code form (`./kit`; the words are the real core's, the code's 2 checksum
 *   characters are not), sets the account id and puts this device in the device log; only a
 *   hash of the key is kept. `RecoveryKitVerify` (`NotFound` without a kit) takes either
 *   form (any case, 4-letter prefixes); a kit that does not decode or is not the current one
 *   is `InvalidInput` for `kit`. `RecoveryKitReissue` replaces the kit (the old one stops
 *   verifying), keeps the account, and wants the app-lock passphrase while the lock is on.
 *   `AppLockEnable` takes 8 to 256 UTF-8 bytes (`InvalidInput` for `passphrase`;
 *   `AlreadyExists` when on); `AppLockDisable` wants the passphrase (wrong: `InvalidInput`
 *   for `passphrase`); `AppLockChange` reports a bad new one on `new` and a wrong current one
 *   on `current`; both are `InvalidInput` for `lock` while the lock is off. `DeviceList` and
 *   `SecurityStatus` report the log, the app lock (Argon2id 65536 KiB, 3, 4) and keyring
 *   counts kept by the core's hooks (a space key per space, a document key per structure and
 *   page document, a tombstone per purged page). Every change emits `SecurityChanged`;
 * - app lock (`lock`, `unlock`, ADR-0018, as the shell does it): `lock()` needs the lock
 *   enabled (`InvalidInput` for `lock`) and moves the core to `"locked"`, where `dispatch`,
 *   `docs` and the backup methods reject with `Locked`; `unlock(passphrase)` returns to
 *   `"ready"`, a wrong passphrase is `InvalidInput` for `passphrase`. Locking a locked core
 *   succeeds; unlocking a ready one is `InvalidInput`;
 * - backups (`backupExport`, `backupImport`, ADR-0017) write and read `files`, this
 *   transport's fake disk, in the mirror's own JSON-based format (`./backup`, not the core's
 *   `KBB1` file, nothing encrypted). Explicit paths must be absolute (`InvalidInput` for
 *   `path`, like the shell). Without a path the shell's dialog is simulated: export proposes
 *   `/backups/atomik-kb-<yyyy-mm-dd>.atomikkb`, import picks the most recently written file,
 *   and with `cancelDialogs` both are cancelled (`InvalidInput` for `path`). Export needs the
 *   workspace and a kit (`InvalidInput` for `workspace` or `kit`). Import needs an
 *   installation without a workspace (`AlreadyExists`) and the backup's kit (`InvalidInput`
 *   for `kit` when missing or wrong; an installation of the same account restores without
 *   it); a damaged file is `Crypto` (a newer format `Storage`) and changes nothing. A restore
 *   brings back the workspace, settings, spaces and pages (same ids, trash included), page
 *   content, the kit, the device log (plus this device, "added via kit"), the attachments with
 *   their chunks (the summary's `chunks` and `chunkBytes`; plaintext bytes here) and the keyring
 *   tombstones, with the app lock off, and emits `WorkspaceCreated`, `SpacesChanged` and
 *   `SecurityChanged`;
 * - attachments (`./attachment-mirror`, ADR-0020, with `kb-core`'s codes, fields and
 *   messages): `AttachmentBegin` checks the name (not blank, at most 255 UTF-8 bytes, no `/`,
 *   `\` or NUL, not `.` or `..`), the MIME type (1 to 127 bytes of printable ASCII without
 *   spaces, `type/subtype`), the workspace (`InvalidInput` for `workspace`) and the space
 *   (`NotFound`); writes past 2 GiB are `InvalidInput` for `size` with `details.limit`; chunks
 *   are cut like the core cuts them, `floor(size / 4 MiB) + 1` of them (plaintext, unencrypted;
 *   `blobs` shows them); an unknown upload is `NotFound` ("no upload in progress for this
 *   attachment"). A finished attachment is created at begin and unreferenced since finish.
 *   References are the `image` and `file` blocks of the documents of live and trashed pages;
 *   when a change leaves an attachment unreferenced it is marked unreferenced since then, and
 *   referencing it again clears the mark. `AttachmentsChanged` follows finish, abort, every
 *   purge (`AttachmentDelete`, `AttachmentGc`, `SpaceDelete`) and every mark that changes
 *   (before the `PageContentChanged` of the edit, as in the core). `AttachmentGc` (also run at
 *   start and at every unlock) purges what has been unreferenced since before `now -
 *   retention` (`app.attachments.retention_days` as days, fractions allowed, default 30, `<= 0`
 *   = never), then deletes chunks nothing claims. The raw `attachments.write` and
 *   `attachments.read` check what the shell checks (the `x-kb-attachment` id, a piece of 1 byte
 *   to 4 MiB, a non-empty range of at most 32 MiB, a whole read of at most 32 MiB), then the
 *   core's (a range past the end is `InvalidInput` for `range`, a chunk missing from `blobs` is
 *   `Storage`, chunks that cannot back the size are `Crypto`). `importPath` and `saveAs` read
 *   and write `files` with the shell's naming rules (`./shell-names`; dialogs simulated like
 *   the backups': import picks the most recently written file, save proposes
 *   `/downloads/<name>`); `open` refuses files the system would run and answers a fake path.
 *   Markdown labels a `file` block with its attachment's name; search indexes an image's alt
 *   text, not a file's name, like the core;
 * - `attachments.importUrl` ("Image from URL", ADR-0028; `./url-images`, `./memory-url-images`):
 *   an `https` link (no credentials, at most 2048 characters) whose picture comes from
 *   `remoteImages`, else `fetch`; its type (PNG, JPEG, GIF, WebP, BMP, AVIF, SVG with the right
 *   `Content-Type`) is taken from the bytes, and the attachment is named after the last segment
 *   of the link's path with that type's extension. Failures are `InvalidInput` for the field
 *   `url` with `details.reason` (`status`, `network`, `timeout`, `redirect`, `notImage`), or for
 *   `size` (past 32 MiB), none naming the link;
 * - attachment sync (Milestone 8, ADR-0023; `./attachment-mirror`, `attachmentSync`): every
 *   `AttachmentInfo` reports `localChunks` and `availability`. Attachments made or seeded here
 *   are `Complete`; remote ones (`attachmentSync.remoteAttachments`) start with no chunk here,
 *   `Downloading` under the eager policy (`Waiting` when the simulated server does not hold their
 *   chunks), `NotDownloaded` under on demand. The policy is the setting `app.attachments.download`
 *   (`"on_demand"`, anything else eager; `attachmentSync.downloadPolicy` seeds it); setting it to
 *   eager makes every incomplete attachment wanted. `AttachmentDownload` makes an incomplete one
 *   wanted whatever the policy (`AttachmentsChanged` when that changes its availability, then
 *   `SyncRequested`) and answers its info; a complete one answers unchanged. Reads (`read`,
 *   `saveAs`, `open`) of an incomplete attachment are `Storage` ("the attachment is not complete
 *   on this device (n of m chunks)") and, under on demand, make it wanted. Chunks move only
 *   through `advanceTransfers` (or the timer of `attachmentSync.transferStepMs`), one per step,
 *   independently of the fake sync driver: uploads first, then downloads. `SyncStatus` reports
 *   `transfers`: `uploads` are the chunks of attachments finished while sync was configured
 *   (a step or the push of a healthy fake cycle sends them, disabling sync forgets them),
 *   `downloads` and `waiting` the missing chunks of wanted attachments;
 * - sync (`./sync-mirror`, ADR-0021; the core's status plus a fake of the shell's driver, no
 *   network): `SyncStatus` starts `Disabled` (or as `sync` seeds it). `SyncConfigure` checks the
 *   address and the token like the client (`InvalidInput` for `serverUrl` or a malformed
 *   `inviteToken`), answers `configured: true, state: "Idle"` (`"Forked"` while the same
 *   server's fork stands) and emits `SyncChanged` then `SyncRequested`; without a `serverUrl` it
 *   disables (cursor 0, no epoch, no fork) and emits `SyncChanged`. `SyncNow` emits
 *   `SyncRequested`. While configured, every local page, structure, space and attachment event
 *   counts as one pending entry and emits `SyncRequested` (settings are never synced; remote
 *   changes from `changePage` do not count). The fake driver answers `SyncRequested` after
 *   `syncStepMs` with a cycle of the simulated server (`Pushing` -> `Pulling` -> `Idle`, the
 *   cursor growing by the entries pushed; or `Offline`, `Error`, or `Pulling` -> `Forked`), one
 *   step per state, each emitting `SyncChanged`. A server that does not know the account
 *   (`sync.accountKnown: false`) ends a cycle without a token in `Error` with
 *   `needsInviteToken: true`; a token creates the account, except `"bad"`, which it refuses. A
 *   lock stops the driver (an unlock runs a cycle again). The first healthy push after a kit
 *   was issued uploads its recovery blob (`SecurityInfo.recoveryPublished`, `SecurityChanged`);
 * - devices (Milestone 7, ADR-0022; `./devices-mirror` and `./security`): pairing in both roles
 *   and the recovery with the kit, simulated on timers of `pairingStepMs` (see `MemoryPairing`
 *   and `MemoryRecovery`): a finished joiner or recovery gets a workspace, the account, its
 *   kit and device log and sync to the given server. `DeviceRevoke` revokes another device,
 *   rotates the root key (`rootGeneration` + 1) and answers a new kit, with `SecurityChanged`
 *   and `DevicesChanged`; `RotationApply` finishes a pending rotation (`devices` or
 *   `security.rotationPending` seed one, `receiveDeviceLog` delivers one); `DeviceList` reports
 *   who added and revoked each device. A lock cancels a pairing in progress;
 * - import and export (ADR-0036, `docs/architecture/import-export.md`; `./memory-export`,
 *   `./memory-import`, with REDUCED FIDELITY): the ten session requests answer like the core's.
 *   `ExportBegin` collects the live pages in tree order (a page, a page with its descendants, or
 *   the space), lays them out like `kb_export::layout` (Notion's layout, safe names de-duplicated
 *   per folder, attachments written once in the folder of the first page that uses them, one
 *   `.md` or `.html` file when the export is one page without attachments or whiteboards, else a
 *   `.zip`) and `ExportNext` renders them in batches with `ExportProgress` events: the body is the
 *   clipboard flavour of `renderMarkdown` under a front matter of the page (title, icon, times,
 *   tags and properties), with relative links to the pages and attachments of the export, or a
 *   compact HTML document, and a multi-file Markdown export ends with the sidecar
 *   `atomik-export.json`. `ExportCsv` follows the CSV rules of 2.8 (byte order mark, CRLF,
 *   quoting, the formula guard, `ExportValue::text`). `ImportBegin`, `ImportAddFile`,
 *   `ImportPlan`, `ImportAddAttachment`, `ImportCommit` and `ImportAbort` run an import
 *   session: the plan (`./memory-import-plan`) reads Markdown through `parseMarkdown` (front
 *   matter, relative and wiki links, relative image links to files of the source), text as
 *   paragraphs, HTML as its text in paragraphs and CSV as a database (a page with a table view
 *   and a child page per row, columns inferred as properties), folders as the page tree with
 *   Notion's `stem/` rule and ids stripped from names, a container page named after the source
 *   when it has several top-level pages; the commit creates the properties, then the pages, then
 *   their documents and values in batches with `ImportProgress` events, and answers the report.
 *   The five shell methods work on the fake disk and on the canned sources of `importSources`:
 *   `exportRun` (the save dialog proposes `/downloads/<suggested name>`; a single file is written
 *   as text, an archive as a JSON listing of its entries, see `encodeMemoryArchive`), `importRun`,
 *   `saveTextFile` (`/downloads/<name>`), `backupFolderChoose` (the folder `/backups/scheduled`,
 *   stored in `app.backup.folder`), `backupFolderList` (the `.atomikkb` files of a folder of the
 *   fake disk, newest first, for the restore wizard) and `backupRunNow` (one backup written into
 *   that folder under `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb`, older ones beyond the schedule's
 *   `keep` deleted, delivered to the fake Drive when it is connected, through a staging file when
 *   there is no folder; the outcome in `app.backup.last`; there is no timer). As in the shell,
 *   `SettingsSet` refuses the shell-owned keys (`InvalidInput` for `key`), a lock drops the sessions,
 *   and a backup leaves the device-bound settings (`DEVICE_BOUND_SETTINGS`: the folder, the last
 *   outcome, the Drive connection, the AI endpoint) out and a restore ignores them;
 * - local AI and MCP (ADR-0037, `docs/architecture/ai.md`, `./memory-ai`, `./memory-mcp`): the core's
 *   `AiIndexStatus` (a made-up index that fills as `ai.advanceIndex()` or the option's timer indexes pages),
 *   `AiIndexClear`, `AiRetrieve` (keyword scoring, any-word) and the MCP clients (`McpClientCreate` answers the
 *   token once), `McpClientDelete`, `McpAuditList` and `McpAuditClear` answer like the core; the requests that
 *   are the app's alone (`AiIndexNext`, `AiIndexStore`, `McpCall`, `McpClientGrantsSet`) and a webview write of
 *   `app.ai.endpoint` are `InvalidInput`. With the `ai` option the transport has `ai` and `mcp` (see
 *   `MemoryAiOptions`), whose hooks a test uses to hold or fail a stream;
 * - moving a page between spaces (ADR-0038, `docs/architecture/space-move.md`, `./memory-move`, with REDUCED
 *   FIDELITY: one replica, so no keys, no residue nodes and no reconciliation of concurrent moves):
 *   `PageMoveToSpacePlan` counts and `PageMoveToSpace` moves a live page and its live subtree to another space
 *   under a parent (a live page of the target, or its root) at an index; the checks and their fields are the
 *   core's (`NotFound`; `InvalidInput` for `pageId`, `spaceId`, `parentId`, `index`; `PageMove` still refuses a
 *   parent in another space). The pages keep their ids, titles, icons and times; values follow by property
 *   name (missing properties and options are created, kinds that differ go through the text form, what cannot
 *   follow is counted), relation values keep the pages that move along, the relation entries of the pages that
 *   stay are removed, favourites follow, saved views are remapped, and attachments that only moved pages use
 *   change space. It is not an undo step. Emits `PagesMovedSpace`, `StructureChanged` for both spaces,
 *   `SpacesChanged` and the schema, attachment and content events;
 * - reordering the children of a parent (ADR-0052): `PageReorderChildren` puts the live children of a page, or the live
 *   top-level pages, into the given order as one undo entry and one `StructureChanged`; `order` must be exactly those
 *   children (`InvalidInput` on `order`), a parent that is not a live page of the space is `InvalidInput` on `parentId`,
 *   an order they already have writes and emits nothing, and no page is stamped as edited;
 * - templates (ADR-0056, `docs/architecture/templates.md`, `./structure`): `PageSetTemplate` makes a live page a template (one undo
 *   step like the appearance; `InvalidInput` for `pageId` in the trash), `PageSetTemplatePlaceholder` sets or clears `today`, `now`
 *   (a date property) or `me` (a person property) on a page that is a template or inside one (the placeholder replaces the page's
 *   stored value in the same step, and `PagePropertySet` of that property removes the placeholder); `PageInfo.template` and
 *   `templatePlaceholders` (schema order, existing properties of a kind that takes them, only inside a template) report them. A
 *   template and the pages inside it stay in `TreeGet`, the counts and `PagesByProperty`, and are left out of search, backlinks,
 *   reminders and the AI index; the flag and the placeholders travel through export (front matter `template` and
 *   `templatePlaceholders`), import, a move to another space and a backup;
 * - duplicating a page with its sub-pages (ADR-0040, `./memory-duplicate`): `PageDuplicate` copies a live page and
 *   its live sub-pages (a trashed one is skipped) with new page and block ids, right after the page among its siblings;
 *   the top copy's title gets " (copy)" appended (English only, as in the core; cut to the 500 characters), the
 *   sub-pages keep theirs; icons and values are copied (a relation that lists a page of the subtree lists its copy,
 *   a two-way relation stays paired), the content is copied with its links into the subtree pointing at the copies
 *   and its drawings under the new block ids, attachments are shared by id, favourites are not copied. One undo step
 *   (the pages; values and documents come back with them). `NotFound` for an unknown page, `InvalidInput` for
 *   `pageId` for a page in the trash or a subtree of more than 1,000 pages. Answers the copy of the page and emits
 *   `StructureChanged`, `SpacesChanged`, `PageContentChanged` (source `Core`) for every new page and `BacklinksChanged`;
 * - bulk edit of rows (ADR-0058, `./memory-bulk`): `PageBulk` sets or clears one property of 1 to 1,000 pages of a space
 *   (`SetValue`, `SetValues`, through the value path of `PagePropertySet`, every value checked first), moves them last under a
 *   parent, duplicates them (each copy right after its page) or moves them to the trash, all or nothing; the structural
 *   actions drop the pages whose ancestor is listed and are ONE undo step. `PageExcerpts` answers the first 280 characters of
 *   the text of up to 500 pages, from the blocks' indexed text (`./content`);
 * - page history (ADR-0057, `docs/architecture/page-history.md`, `./memory-history`): every write to a page document (the
 *   editor's updates, the core's own writes, `changePage`) is a row of an in-memory journal (sessions of one author with a gap of
 *   at most ten minutes, times from `now()`, the title of the moment); `PageHistoryList` lists the rows newest first, then the
 *   older history as undated versions (runs of changes of one peer), a version being the document's Loro frontiers as
 *   `"<peer>@<counter>"`; `PageHistoryGet` answers the content (a fork of the document) and the title at a version;
 *   `PageHistoryRestore` (Editor) reverts the document to a version as one new change, records a `restore` row and renames
 *   the page when the version's title is known and differs. A change that arrived as `"Remote"` is a row without an author
 *   (this transport has no sync entries to name one);
 * - importing into new spaces (ADR-0039, `docs/architecture/appflowy-import.md`, `./memory-import`,
 *   `./memory-import-appflowy`, REDUCED FIDELITY): `ImportBeginSpaces` and `importRun({ destination:
 *   "newSpaces" })` plan an AppFlowy backup (the `.json` files of a backup are text, `collab_objects/` is
 *   skipped) with a reduced reader and create one space per AppFlowy space at the plan; the images the backup
 *   keeps on a server are listed (`remoteFiles`) and never downloaded (links with a `remoteImage` warning). The
 *   dialog picks the first canned `zip` that is an AppFlowy backup when the destination is new spaces;
 * - shared spaces (Milestone 13, ADR-0041 to ADR-0047, `./sharing-mirror`, with REDUCED FIDELITY: no server, no keys,
 *   simulated people): the 21 requests of the sharing contract answer like the core's, including the plan of the
 *   share dialog, `SpaceShare` (`Pending`, then `Shared` after a step), the member list, roles, removal, leave, close,
 *   settings, the per-account AI switch, the profile, verified people and the invites of both flows (a code session
 *   on timers, link creation, opening and preview). Every writing request is gated by this account's role
 *   (`Forbidden` below the needed role, and for any write to an archive; page documents `import` too), a shared
 *   space is left or closed, never deleted, its trash expires by its own settings on an Owner's device only, and
 *   AI and MCP skip a space whose AI is off. `receiveSharing` is the hook for what other people do. `SyncStatus`
 *   lists the `feeds` once a space is shared (the personal feed first);
 * - while the core is `"opening"`, `"locked"` or `"failed"`, `dispatch`, `docs` and
 *   `attachments` reject as the shell does.
 *
 * Nothing is persisted and nothing is encrypted.
 */
export class MemoryTransport implements Transport {
  /** Raw-bytes page document sessions; see the class documentation. */
  readonly docs: DocTransport;
  /** Attachment bytes and files (raw bytes, the fake disk `files`); see the class documentation. */
  readonly attachments: AttachmentTransport;
  /**
   * The fake blob store: chunk id to the chunk's bytes (plaintext). Tests read it, or delete or
   * add chunks to see a read fail with `Storage` or the GC sweep orphans.
   */
  readonly blobs: Map<ChunkId, Uint8Array>;
  /** Present only when `MemoryTransportOptions.openExternal` was given. */
  readonly openExternal?: (url: string) => Promise<void>;
  /** The shell's AI commands, with the mirror's hooks; present only when `MemoryTransportOptions.ai` was given. */
  readonly ai?: MemoryAiTransport;
  /** The shell's MCP commands; present only when `MemoryTransportOptions.ai` was given. */
  readonly mcp?: McpTransport;
  /** The fake dictation (ADR-0053); present only when `MemoryTransportOptions.dictation` was given. */
  readonly dictation?: MemoryDictation;
  /** The fake Google Drive (ADR-0059); present only when `MemoryTransportOptions.drive` was given. */
  readonly drive?: MemoryDrive;
  /**
   * Presence frames (Milestone 13, slice 3, `./memory-presence`): this device's frames go nowhere in a personal space or a
   * read-only copy (and that is no error), `sentPresence` lists what a shared space got, and the other people's frames come
   * from `receivePresence` and the scripts of `MemoryTransportOptions.presence`.
   */
  readonly presence: PresenceTransport;
  /**
   * The fake disk of `backupExport` and `backupImport` (path to file bytes); tests read,
   * copy or damage backups through it. Shared when `MemoryTransportOptions.files` is.
   */
  readonly files: Map<string, Uint8Array>;
  readonly #listeners = new Set<(event: Event) => void>();
  readonly #settings = new Map<string, string>();
  readonly #now: () => number;
  readonly #newId: () => string;
  readonly #status: Omit<AppStatus, "workspaceInitialized">;
  readonly #injected: ApiError[] = [];
  readonly #injectedDocs: ApiError[] = [];
  readonly #injectedAttachments: ApiError[] = [];
  readonly #remoteImages: ReadonlyMap<string, RemoteImage> | undefined;
  readonly #structure: MemoryStructure;
  readonly #properties: MemoryProperties;
  readonly #content: MemoryContent;
  readonly #attachments: MemoryAttachments;
  readonly #security: MemorySecurity;
  readonly #sync: MemorySync;
  readonly #pairing: MemoryPairing;
  readonly #recovery: MemoryRecovery;
  readonly #export: MemoryExport;
  readonly #import: MemoryImport;
  readonly #move: MemoryMove;
  readonly #duplicate: MemoryDuplicate;
  /** Bulk edit of rows (ADR-0058): `PageBulk` over the structure, the values and the duplication. */
  readonly #bulk: MemoryBulk;
  /** Page history (ADR-0057): the journal of editing sessions, the versions of a page, its content at one, restore. */
  readonly #history: MemoryHistory;
  readonly #ai: MemoryAi;
  readonly #mcp: MemoryMcp;
  readonly #sharing: MemorySharing;
  readonly #comments: MemoryComments;
  readonly #presence: MemoryPresence;
  readonly #links: MemoryViewLinks;
  readonly #importSources: Readonly<Record<string, MemoryImportSource>>;
  /** Events held back while an import commit runs (see `#collecting`); `undefined` outside one. */
  #collected: Event[] | undefined;
  #state: CoreStateInfo;
  /**
   * When `remindersTick` last expired the trash, by `now()`; `null` until the first tick after an open or an unlock
   * (the core has just expired it itself then, so the hour starts there).
   */
  #trashChecked: number | null = null;
  #workspace: Workspace | null = null;
  #device: DeviceInfo;
  #cancelDialogs: boolean;
  readonly #dialogFolder: string;

  constructor(options: MemoryTransportOptions = {}) {
    this.#now = options.now ?? Date.now;
    this.#newId = options.newId ?? randomUuidV4;
    const security = options.security ?? {};
    const startsLocked = security.startLocked === true || options.coreState === "locked";
    if (startsLocked && security.appLockPassphrase === undefined) {
      throw internalError("MemoryTransport: starting locked needs security.appLockPassphrase");
    }
    this.#state = startsLocked ? "locked" : (options.coreState ?? "ready");
    // Before anything can emit: every event passes by it (local edits count once sync is seeded).
    this.#sync = new MemorySync({
      now: this.#now,
      emit: (event) => {
        this.#emit(event);
      },
      ready: () => this.#state === "ready",
      stepMs: options.syncStepMs ?? DEFAULT_SYNC_STEP_MS,
      pushed: () => {
        this.#security.recoveryUploaded();
        this.#attachments.uploadsDone();
      },
      disconnected: () => {
        this.#attachments.uploadsDone();
      },
      transfers: () => this.#attachments.transfers(),
    });
    this.files = options.files ?? new Map<string, Uint8Array>();
    this.#remoteImages = options.remoteImages;
    this.#cancelDialogs = options.cancelDialogs ?? false;
    this.#dialogFolder = options.dialogFolder === undefined ? SCHEDULED_BACKUP_FOLDER : absolutePath(options.dialogFolder);
    this.#importSources = options.importSources ?? {};
    const platform = options.status?.platform ?? "memory";
    this.#status = {
      schemaVersion: 1,
      dbEncrypted: false,
      dbCipher: "none",
      dataDir: "(memory)",
      coreVersion: "0.0.0-memory",
      apiVersion: API_VERSION,
      platform,
      ...options.status,
      deviceId: this.#newId(),
    };
    this.#device = viaWire<DeviceInfo>({
      deviceId: this.#status.deviceId,
      name: options.deviceName ?? defaultDeviceName(platform),
      platform,
      createdAt: this.#now(),
    });
    if (options.workspaceName !== undefined) {
      this.#workspace = viaWire<Workspace>({
        id: this.#newId(),
        name: displayName(options.workspaceName, MAX_WORKSPACE_NAME_CHARS, "name"),
        createdAt: this.#now(),
      });
    }
    for (const [key, value] of Object.entries(options.settings ?? {})) {
      validateSettingKey(key);
      this.#settings.set(key, serialiseSettingValue(value));
    }
    const downloadPolicy = options.attachmentSync?.downloadPolicy;
    if (downloadPolicy !== undefined) {
      this.#settings.set(DOWNLOAD_POLICY_SETTING, serialiseSettingValue(downloadPolicy));
    }
    this.#structure = new MemoryStructure({
      now: this.#now,
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      pagesRemoved: (pageIds) => {
        this.#content.pagesRemoved(pageIds);
      },
      pageProperties: (spaceId, pageId) => this.#properties.valuesOf(spaceId, pageId),
      // The placeholders of a template page (ADR-0056) are listed by schema order and kind, and replace the stored value.
      propertyKinds: (spaceId) => this.#properties.kindsOf(spaceId),
      storedValue: (spaceId, pageId, propertyId) => this.#properties.storedValue(spaceId, pageId, propertyId),
      restoreValue: (spaceId, pageId, propertyId, value) => {
        this.#properties.restoreValue(spaceId, pageId, propertyId, value);
      },
      attachmentOf: (attachmentId) => this.#attachments.manifestOf(attachmentId),
      deviceId: () => this.#status.deviceId,
      // Spaces are listed with what sharing says about them (it does not exist yet while the seeds run).
      decorateSpace: (info) => (this.#sharing as MemorySharing | undefined)?.decorate(info) ?? info,
    });
    this.#properties = new MemoryProperties({
      now: this.#now,
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      structure: this.#structure,
      attachmentName: (attachmentId) => this.#attachments.nameOf(attachmentId),
      attachmentsChanged: (attachmentIds) => {
        this.#attachments.refresh(attachmentIds);
      },
      // The people of a Person value, "me" and the names are the sharing mirror's (it is built before the seeds run).
      people: (spaceId) => this.#sharing.peopleOf(spaceId).map((person) => person.accountId),
      me: () => this.#sharing.meId(),
      personName: (spaceId, accountId) => this.#sharing.nameOf(spaceId, accountId),
      account: (spaceId, name) => this.#sharing.accountByName(spaceId, name),
      ...(options.localOffsetSeconds === undefined ? {} : { localOffsetSeconds: options.localOffsetSeconds }),
    });
    this.#content = new MemoryContent({
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      structure: this.#structure,
      attachmentName: (attachmentId) => this.#attachments.nameOf(attachmentId),
      documentChanged: (touched) => {
        this.#attachments.refresh(touched);
      },
      propertyTexts: (spaceId, pageId) => this.#properties.searchTexts(spaceId, pageId),
      propertyReferences: () => this.#properties.attachmentReferenceCounts(),
      commentTexts: (spaceId, pageId) => this.#comments.searchTexts(spaceId, pageId),
      firesReminder: (spaceId, reminderFor) => this.#sharing.firesReminder(spaceId, reminderFor),
      iconReferences: () => this.#structure.iconReferenceCounts(),
      // Every write to a page document is a row of the history's journal (it does not exist yet while the seeds run).
      documentWritten: (pageId, source, before) => {
        (this.#history as MemoryHistory | undefined)?.record(pageId, source, before);
      },
      documentEdited: (pageId) => {
        // As the core: a local content edit stamps the page's last edit (throttled), then the
        // structure is announced as changed.
        const spaceId = this.#structure.pageMeta(pageId)?.spaceId;
        if (spaceId !== undefined && this.#structure.touchEdited(pageId, this.#now())) {
          this.#emit({ type: "StructureChanged", data: { spaceId } });
        }
      },
    });
    this.#attachments = new MemoryAttachments({
      now: this.#now,
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      structure: this.#structure,
      content: this.#content,
      hasWorkspace: () => this.#workspace !== null,
      setting: (key) => {
        const stored = this.#settings.get(key);
        return stored === undefined ? undefined : (JSON.parse(stored) as JsonValue);
      },
      // Chunks arriving and attachments becoming wanted change no synced document.
      emitTransfer: (event) => {
        this.#sync.remote(() => {
          this.#emit(event);
        });
      },
      requestSync: () => {
        this.#sync.syncNow();
      },
      syncConfigured: () => this.#sync.configured,
      ready: () => this.#state === "ready",
      transferStepMs: options.attachmentSync?.transferStepMs,
    });
    this.blobs = this.#attachments.blobs;
    const seeded: SeededSpace[] = [];
    this.#structure.seed(
      options.spaces ?? [],
      (page, seed) => {
        if (seed.content !== undefined) {
          this.#content.seed(page.id, seed.content);
        }
        seeded.find((entry) => entry.spaceId === page.spaceId)?.pages.push({ pageId: page.id, seed });
      },
      (space, seed) => {
        seeded.push({ spaceId: space.id, seed, pages: [] });
      },
    );
    const content = this.#content;
    this.docs = Object.freeze({
      open: (pageId: PageId) => this.#docCall(() => content.open(pageId)),
      import: (pageId: PageId, update: Uint8Array) =>
        this.#docCall(() => {
          // A Viewer's or Commenter's edit, and any edit of an archive, is refused (the gate of the content writes).
          this.#sharing.require(this.#structure.pageMeta(pageId)?.spaceId, "editor");
          content.import(pageId, update);
        }),
      export: (pageId: PageId, since?: Uint8Array) => this.#docCall(() => content.export(pageId, since)),
      // The core drops its in-memory replica; the mirror's document is its storage, so it stays.
      close: () => this.#docCall(() => undefined),
    });
    if (options.openExternal !== undefined) {
      this.openExternal = options.openExternal;
    }
    this.#security = new MemorySecurity({
      now: this.#now,
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      structure: this.#structure,
      content: this.#content,
      device: () => ({ ...this.#device }),
      hasWorkspace: () => this.#workspace !== null,
      attachments: () => this.#attachments.keyring(),
    });
    this.#sharing = new MemorySharing(
      {
        now: this.#now,
        newId: this.#newId,
        emit: (event) => {
          this.#emit(event);
        },
        ready: () => this.#state === "ready",
        stepMs: options.sharing?.stepMs ?? options.pairingStepMs ?? DEFAULT_SHARING_STEP_MS,
        hasWorkspace: () => this.#workspace !== null,
        hasSpace: (spaceId) => this.#structure.hasSpace(spaceId),
        findSpace: (name) => this.#structure.spaceList().find((space) => space.name === name)?.id,
        spaceInfo: (spaceId) => this.#structure.spaceInfo(spaceId),
        syncServer: () => this.#sync.info().serverUrl,
        accountId: () => this.#security.accountId,
        counts: (spaceId) => this.#sharePlanCounts(spaceId),
        dropSpace: (spaceId) => {
          this.#dropSpace(spaceId);
        },
        joinedSpace: (name) => this.#joinedSpace(name),
        copyUnsent: (pageIds) => this.#copyUnsent(pageIds),
        aiOff: (spaceId) => {
          // Opting out revokes the MCP grants of the space; the index skips it (`#aiPages`).
          this.#mcp.spaceDeleted(spaceId);
          this.#emit({ type: "McpChanged" });
          this.#emit({ type: "AiIndexChanged" });
        },
      },
      options.sharing,
    );
    // The mirror of comments, person mentions and notifications (Milestone 13, slice 2) sits on the people and roles of the
    // sharing mirror above; it draws no id while the seeds run.
    this.#comments = new MemoryComments({
      now: this.#now,
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      ready: () => this.#state === "ready",
      sharing: this.#sharing,
      page: (pageId) => {
        const meta = this.#structure.pageMeta(pageId);
        return meta === undefined ? undefined : { spaceId: meta.spaceId, title: meta.title };
      },
      hasSpace: (spaceId) => this.#structure.hasSpace(spaceId),
      pageByTitle: (spaceId, title) =>
        this.#structure.livePages(spaceId).find((page) => page.title === title)?.id,
      textAnchor: (pageId, quote) => this.#content.textAnchor(pageId, quote),
      personValue: (pageId, propertyId) => {
        const value = this.#properties.pageGet(pageId).find(([id]) => id === propertyId)?.[1];
        return value?.kind === "person" ? [...value.value] : [];
      },
      setPersonValue: (pageId, propertyId, accounts) => {
        this.#properties.pageSet(pageId, propertyId, { kind: "person", value: accounts });
      },
    });
    // Presence (Milestone 13, slice 3) sits on the members of the sharing mirror and the pages' documents; it draws no id from
    // the generator (the devices of the simulated people have ids of their own).
    this.#presence = new MemoryPresence({
      now: this.#now,
      emit: (event) => {
        this.#emit(event);
      },
      ready: () => this.#state === "ready",
      sharing: this.#sharing,
      hasSpace: (spaceId) => this.#structure.hasSpace(spaceId),
      findSpace: (name) => this.#structure.spaceList().find((space) => space.name === name)?.id,
      pageByTitle: (spaceId, title) => this.#structure.livePages(spaceId).find((page) => page.title === title)?.id,
      pageSpace: (pageId) => this.#structure.pageMeta(pageId)?.spaceId,
      doc: (pageId) => this.#content.docOf(pageId),
    });
    this.presence = Object.freeze({
      send: (spaceId: SpaceId, pageId: PageId | undefined, state: PresenceState, update: Uint8Array) =>
        this.#run(() => {
          this.#checkReady();
          this.#presence.send(spaceId, pageId, state, update);
        }),
      subscribe: (listener: (message: PresenceUpdate) => void) => this.#presence.subscribe(listener),
    });
    // View links (Milestone 13, slice 4) sit on the roles and settings of the sharing mirror and on the pages; they draw no id from the
    // generator (a link's id is derived from its key, not random).
    this.#links = new MemoryViewLinks(
      {
        now: this.#now,
        emit: (event) => {
          this.#emit(event);
        },
        ready: () => this.#state === "ready",
        sharing: this.#sharing,
        hasSpace: (spaceId) => this.#structure.hasSpace(spaceId),
        findSpace: (name) => this.#structure.spaceList().find((space) => space.name === name)?.id,
        pageByTitle: (spaceId, title) => this.#structure.livePages(spaceId).find((page) => page.title === title)?.id,
        page: (pageId) => {
          const meta = this.#structure.pageMeta(pageId);
          return meta === undefined ? undefined : { spaceId: meta.spaceId, trashed: meta.trashed };
        },
        syncServer: () => this.#sync.info().serverUrl,
        accountId: () => this.#security.accountId,
      },
      options.links,
    );
    // After the pages, so that a seeded kit's account id does not shift the ids of seeded
    // spaces and pages; attachments last, so that they shift nothing either.
    this.#security.seed(security, options.devices);
    this.#attachments.seed(options.attachments ?? []);
    this.#attachments.seedRemote(options.attachmentSync?.remoteAttachments ?? []);
    // Properties last of all that draw ids, so that they shift no other seeded id (see `SeedSpace.properties`).
    this.#properties.seed(seeded);
    // A file only a seeded property lists is referenced: the attachments were marked before it was seeded.
    this.#attachments.refresh();
    // The core collects at every open (no listener can hear it yet). It expires the trash too, but a
    // seeded workspace has no trashed page, so there is nothing for this open to expire.
    if (this.#state === "ready") {
      this.#attachments.gc();
    }
    // Last, so that nothing the seeds and the first collection did counts as a local edit. A
    // revoked device has forgotten its sync configuration.
    if (!this.#security.thisDeviceRevoked) {
      this.#sync.seed(options.sync ?? {});
    }
    const devices = {
      now: this.#now,
      emit: (event: Event) => {
        this.#emit(event);
      },
      ready: () => this.#state === "ready",
      stepMs: options.pairingStepMs ?? DEFAULT_PAIRING_STEP_MS,
      hasWorkspace: () => this.#workspace !== null,
    };
    this.#pairing = new MemoryPairing({
      ...devices,
      hasAccount: () => this.#security.status().recoveryKit !== undefined,
      syncServer: () => this.#sync.info().serverUrl,
      installBundle: (serverUrl) => {
        const sponsor = this.#security.otherDevice("Laptop", this.#now());
        this.#joinAccount("Paired workspace", serverUrl, {
          // The imaginary sponsor's kit: no typed kit matches it (its words are not known here).
          recoveryId: "",
          others: [sponsor],
          addedBy: sponsor.deviceId,
          viaKit: false,
        });
      },
      deviceAdded: (name) => {
        this.#security.deviceAdded(name);
      },
    });
    this.#recovery = new MemoryRecovery({
      ...devices,
      install: (serverUrl, recoveryId) => {
        this.#joinAccount("Recovered workspace", serverUrl, {
          recoveryId,
          others: [this.#security.otherDevice("Laptop", this.#now())],
          viaKit: true,
        });
      },
    });
    this.#export = new MemoryExport({
      now: this.#now,
      emit: (event) => {
        this.#emit(event);
      },
      structure: this.#structure,
      properties: this.#properties,
      content: this.#content,
      attachments: this.#attachments,
      deviceName: (deviceId) =>
        deviceId === this.#device.deviceId
          ? this.#device.name
          : this.#security.deviceList().find((entry) => entry.deviceId === deviceId)?.name,
      personName: (spaceId, accountId) => this.#sharing.nameOf(spaceId, accountId),
    });
    this.#import = new MemoryImport({
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      collect: (run) => this.#collecting(run),
      structure: this.#structure,
      properties: this.#properties,
      content: this.#content,
      attachments: this.#attachments,
      deleteEmptySpace: (spaceId) => {
        // What `SpaceDelete` does for a space without pages.
        this.#attachments.spaceDeleted(spaceId);
        this.#structure.spaceDelete(spaceId);
        this.#properties.spaceDeleted(spaceId);
        this.#mcp.spaceDeleted(spaceId);
        this.#security.spaceDeleted(spaceId);
      },
    });
    this.#move = new MemoryMove({
      collect: (run) => this.#collecting(run),
      structure: this.#structure,
      properties: this.#properties,
      content: this.#content,
      attachments: this.#attachments,
    });
    this.#duplicate = new MemoryDuplicate({
      collect: (run) => this.#collecting(run),
      newId: this.#newId,
      now: this.#now,
      // A page made from a template (ADR-0056) resolves "today", "now" and "me" with the clock, the offset and the account.
      me: () => this.#sharing.meId(),
      personName: (spaceId, accountId) => this.#sharing.nameOf(spaceId, accountId),
      people: (spaceId) => this.#sharing.peopleOf(spaceId).map((person) => person.accountId),
      ...(options.localOffsetSeconds === undefined ? {} : { localOffsetSeconds: options.localOffsetSeconds }),
      structure: this.#structure,
      properties: this.#properties,
      content: this.#content,
      attachments: this.#attachments,
    });
    this.#bulk = new MemoryBulk({
      collect: (run) => this.#collecting(run),
      structure: this.#structure,
      properties: this.#properties,
      duplicate: this.#duplicate,
      content: this.#content,
    });
    // The author of a local write is this device and, with a kit, this account; no id is drawn from the generator.
    this.#history = new MemoryHistory({
      now: this.#now,
      collect: (run) => this.#collecting(run),
      device: () => ({ deviceId: this.#device.deviceId, accountId: this.#security.accountId }),
      structure: this.#structure,
      content: this.#content,
    });
    this.#mcp = new MemoryMcp({
      now: this.#now,
      newId: this.#newId,
      emit: (event) => {
        this.#emit(event);
      },
      spaceExists: (spaceId) => this.#structure.spacePageIds(spaceId) !== undefined,
    });
    this.#ai = new MemoryAi(options.ai, {
      now: this.#now,
      emit: (event) => {
        this.#emit(event);
      },
      setting: (key) => this.#storedSetting(key),
      shellSetting: (key, value) => {
        this.#shellSetting(key, value);
      },
      pages: (scope) => this.#aiPages(scope),
      // A template and the pages inside it are not indexed (ADR-0056), so their chunks go when a page becomes one.
      livePageIds: () =>
        this.#structure
          .livePages()
          .filter((page) => !page.inTemplate && this.#sharing.aiEnabled(page.spaceId))
          .map((page) => page.id),
      ready: () => {
        this.#checkReady();
      },
    });
    if (options.dictation !== undefined) {
      this.dictation = new MemoryDictation(options.dictation === "fake" ? {} : options.dictation);
    }
    if (options.drive !== undefined) {
      // The connection lives in `app.backup.drive`, which only the shell writes: through the shell's path, so that
      // `SettingChanged` follows; the seed does not announce itself.
      this.drive = new MemoryDrive(options.drive === "fake" ? {} : options.drive, {
        files: this.files,
        now: this.#now,
        setting: (key) => this.#storedSetting(key),
        seedSetting: (key, value) => {
          this.#settings.set(key, serialiseSettingValue(value));
        },
        writeSetting: (key, value) => {
          this.#shellSetting(key, value);
        },
      });
    }
    if (options.ai !== undefined) {
      const ai = this.#ai;
      const mcp = this.#mcp;
      this.ai = ai;
      this.mcp = Object.freeze({
        status: () => this.#run(() => {
          this.#checkReady();
          return ai.mcpStatus();
        }),
        grantsSet: (clientId: string, grants: McpGrant[]) =>
          this.#run(() => {
            this.#checkReady();
            // A space whose AI is off cannot be granted (ADR-0047): the member opts in first.
            if (grants.some((grant) => (grant.read || grant.write) && !this.#sharing.aiEnabled(grant.spaceId))) {
              throw invalid("grants", "AI is off for a space in these grants: turn it on for the space first");
            }
            return mcp.grantsSet(clientId, grants);
          }),
      });
    }
    const attachments = this.#attachments;
    const mediaUrl = options.mediaUrl;
    this.attachments = Object.freeze({
      ...(mediaUrl === undefined ? {} : { mediaUrl }),
      write: (attachmentId: AttachmentId, bytes: Uint8Array) =>
        this.#attachmentCall(() => {
          attachments.write(attachmentHeader(attachmentId), shellPiece(bytes));
        }),
      read: (attachmentId: AttachmentId, range?: ByteRange) =>
        this.#attachmentCall(() => this.#rawRead(attachmentHeader(attachmentId), range)),
      importPath: (spaceId: SpaceId, path?: string) => this.#attachmentCall(() => this.#importPath(spaceId, path)),
      importUrl: (spaceId: SpaceId, url: string) => this.#attachmentCallAsync(() => this.#importUrl(spaceId, url)),
      saveAs: (attachmentId: AttachmentId, path?: string) => this.#attachmentCall(() => this.#saveAs(attachmentId, path)),
      open: (attachmentId: AttachmentId) => this.#attachmentCall(() => this.#openAttachment(attachmentId)),
    });
    // Shared spaces start last of all (after sync, whose server a session may need), drawing no ids.
    this.#sharing.seed(options.sharing ?? {});
    // Their comment threads and inbox rows come after it: the members they name exist by then (no ids drawn either).
    for (const seed of options.sharing?.spaces ?? []) {
      const spaceId = this.#structure.spaceList().find((space) => space.name === seed.space)?.id;
      if (spaceId !== undefined && (seed.comments !== undefined || seed.notifications !== undefined)) {
        this.#comments.seed(spaceId, seed.comments, seed.notifications);
      }
    }
    // The simulated people of presence come last: they are members of the shared spaces above and walk the pages seeded.
    this.#presence.seed(options.presence ?? {});
    // Their links last: they name pages and members of the spaces above.
    this.#links.seed(options.links ?? {});
  }

  dispatch(request: Request): Promise<Response> {
    // `async` semantics without an `async` function: every failure is a rejection, and the
    // events of a request that needs no hashing reach listeners before `dispatch` returns.
    return this.#run(() => this.#handle(request));
  }

  subscribe(onEvent: (event: Event) => void): () => void {
    this.#listeners.add(onEvent);
    return () => {
      this.#listeners.delete(onEvent);
    };
  }

  coreState(): Promise<CoreStateInfo> {
    const state = this.#state;
    return Promise.resolve(typeof state === "string" ? state : { failed: { ...state.failed } });
  }

  /**
   * One tick of the reminder scheduler, as `Core::reminders_tick` runs it (ADR-0024): emits an
   * `Event.ReminderDue` for every reminder that fell due since the last tick (oldest first, at
   * most 25 and what shares the moment of the last of them, the rest at the next tick) and
   * moves the mark `app.reminders.last_checked` (never back); resolves with how many fired. The
   * wall clock is `now()` plus the offset of this machine's time zone, so reminders fire when
   * the local clock reaches their moment. Nothing happens while the core is not ready (no
   * documents are open while locked, and the first tick after the unlock catches up).
   *
   * The shell around the real core ticks it with a timer; this transport has no shell, so the UI
   * store calls this every 30 seconds (`CoreClient.hasRemindersTick`).
   */
  remindersTick(): Promise<number> {
    return this.#run(() => {
      if (this.#state !== "ready") {
        return 0;
      }
      const nowMs = this.#now();
      // The shell's loop expires the trash about once an hour from the same tick.
      if (this.#trashChecked === null) {
        this.#trashChecked = nowMs;
      } else if (nowMs - this.#trashChecked >= TRASH_EXPIRY_INTERVAL_MS) {
        this.#trashChecked = nowMs;
        this.#expireTrash(nowMs);
      }
      const storedMark = this.#settings.get(LAST_CHECKED_SETTING);
      const parsed: unknown = storedMark === undefined ? null : JSON.parse(storedMark);
      const last = typeof parsed === "number" && Number.isInteger(parsed) ? parsed : null;
      const until = localNowSeconds(new Date(nowMs));
      const { fire, mark } = planTick(
        (after, upTo, limit) => this.#content.dueReminders(after, upTo, limit),
        last,
        until,
      );
      if (mark !== last) {
        this.#settings.set(LAST_CHECKED_SETTING, JSON.stringify(mark));
      }
      for (const reminder of fire) {
        this.#emit({ type: "ReminderDue", data: reminder });
      }
      return fire.length;
    });
  }

  /**
   * Expires the trash now, as `Core::trash_expire` does (ADR-0040): purges every top-level trashed page whose
   * `trashedAt` is strictly older than the setting `app.trash.retention_days` (30 days when absent, `0` = never),
   * subtree included, and resolves with how many top-level pages went. Nothing happens while the core is not ready.
   * The core runs this at open and unlock and the shell's loop once an hour; here `unlock` runs it, and
   * `remindersTick()` once an hour of `now()`.
   */
  trashExpire(): Promise<number> {
    return this.#run(() => (this.#state === "ready" ? this.#expireTrash() : 0));
  }

  /**
   * Test hook and dev page: records a call of an MCP client as the real server would (an audit row, the client's
   * last use, `McpChanged`). The mirror runs no MCP server (ADR-0037). `NotFound` for an unknown client.
   */
  recordMcpCall(call: McpCallRecord): McpAuditEntry {
    return this.#mcp.recordCall(call);
  }

  /** Test hook: moves the core to another lifecycle state (for example `"ready"`). */
  setCoreState(state: CoreStateInfo): void {
    this.#state = state;
  }

  /** Test hook: makes the next `times` calls to `dispatch` reject with `error`. */
  failNext(error: ApiError, times = 1): void {
    for (let i = 0; i < times; i += 1) {
      this.#injected.push({ ...error });
    }
  }

  /** Test hook: makes the next `times` calls to any `docs` method reject with `error`. */
  failNextDocs(error: ApiError, times = 1): void {
    for (let i = 0; i < times; i += 1) {
      this.#injectedDocs.push({ ...error });
    }
  }

  /**
   * Test hook: changes a page's document the way another writer would (the core itself, or
   * a remote device once sync exists): `edit` gets the mirror's `LoroDoc` (created first if
   * needed), the change is committed and indexed, and `PageContentChanged` with `source`
   * (default `"Remote"`) is emitted, followed by any `BacklinksChanged`. An editor then
   * catches up through `docs.export(pageId, doc.oplogVersion().encode())`. Throws
   * `NotFound` for unknown and purged pages. `actor` (a member of the page's space, by name) says who made a `"Remote"`
   * change: a mention of this account that it adds is then a `PageMention` notification (ADR-0049); without an actor a
   * remote change notifies nobody.
   */
  changePage(pageId: PageId, edit: (doc: LoroDoc) => void, source: ChangeSource = "Remote", actor?: string): void {
    const before = actor === undefined ? [] : [...this.#content.personMentions(pageId)];
    if (source === "Remote") {
      this.#sync.remote(() => {
        this.#content.change(pageId, edit, source);
      });
    } else {
      this.#content.change(pageId, edit, source);
    }
    // A change that another person made (`actor`, a member of the page's space by name) that adds a mention of this
    // account is a `PageMention` (ADR-0049); this account's own changes never notify.
    if (actor !== undefined && source === "Remote") {
      this.#comments.pageEdited(pageId, actor, before, this.#content.personMentions(pageId));
    }
  }

  /**
   * Test hook: another person acts on comments or a Person property, as it arrives through sync (see `CommentChange`): they
   * post a thread or a reply, edit, delete or resolve, or set a Person property. The rules of the comment stream's fold
   * apply to them as to this account (`Forbidden` for a Viewer's post, for an edit of someone else's comment, ...;
   * `InvalidInput` for `author` when the name is not a member of the space), and the notifications of this account come
   * out of these changes (a reply in a thread it wrote in, a mention of it in a comment, being added to a Person
   * property), as `NotificationRaised` then `NotificationsChanged`. Answers the thread the change touched. Nothing here is a
   * local edit.
   */
  receiveComments(change: CommentChange): ThreadInfo | undefined {
    return this.#sync.remote(() => this.#comments.receive(change));
  }

  /**
   * Test hook: makes the next `times` calls to any `attachments` method reject with `error`, and the
   * next `times` uploads of `importRun` (the shell's upload of a file an import needs) fail with it
   * (`attachmentFailed` in the report).
   */
  failNextAttachments(error: ApiError, times = 1): void {
    for (let i = 0; i < times; i += 1) {
      this.#injectedAttachments.push({ ...error });
    }
  }

  /**
   * Test hook: runs up to `count` (default 1) steps of the fake transfer step of the sync driver
   * and answers how many did something (`0` when nothing is due, and while the core is not
   * ready). One step moves one chunk: first a chunk to upload (an attachment finished while sync
   * was configured), else the next chunk of the oldest wanted attachment from the simulated
   * server into `blobs`, emitting `AttachmentsChanged { spaceId }` (`localChunks` grows; the
   * last chunk makes it `Complete`), or, when the server does not hold the chunk (a `waiting`
   * remote attachment), making it `Waiting`. Every step then emits `SyncChanged` (the counts of
   * `SyncInfo.transfers` changed). None of these events is a local edit. See
   * `MemoryAttachmentsOptions`.
   */
  advanceTransfers(count = 1): number {
    if (!Number.isSafeInteger(count) || count < 0) {
      throw internalError("MemoryTransport: advanceTransfers wants a whole number of steps");
    }
    return this.#state === "ready" ? this.#attachments.advance(count) : 0;
  }

  /** Test hook: makes the shell's file dialogs cancel (`true`) or pick a file again (`false`). */
  setCancelDialogs(cancel: boolean): void {
    this.#cancelDialogs = cancel;
  }

  /**
   * Test hook: switches the simulated sync server (see `MemorySyncMode`; `lastError` replaces the
   * default message of `"offline"` and `"error"`). The status changes with the next cycle, for
   * example after `syncNow()`.
   */
  setSyncMode(mode: MemorySyncMode, lastError?: string): void {
    this.#sync.setMode(mode, lastError);
  }

  /**
   * Test hook: device log entries arrive from other devices through sync (see `DeviceLogChange`):
   * devices added, other devices revoked, this device revoked (sync is then forgotten:
   * `SyncChanged`), the root key rotated (applied, or pending while the app lock is on). Emits
   * `SecurityChanged` and `DevicesChanged` as the core does, and answers the event's data.
   * Needs a kit (`Internal` otherwise).
   */
  receiveDeviceLog(change: DeviceLogChange): DevicesChange {
    const data = this.#security.receiveLog(change);
    if (data.thisDeviceRevoked && this.#sync.configured) {
      this.#sync.configure(undefined, undefined);
    }
    return data;
  }

  /**
   * Test hook: another person acts in a shared space, as it arrives through sync (see `SharingChange`): they join (by
   * a link this account created, with the invite rules: single use, expiry, revocation), change roles, leave, remove
   * this account (the space becomes an archive: `SpaceRemoved`, and `UnsentChangesCopied` for pages with unsent
   * changes), close the space, add a device with their kit, have changes refused, publish an incomplete snapshot. The
   * rules of the membership fold apply (the last Owner stays; `InvalidInput` for `role` otherwise), `NotFound` for a
   * dead invite. Answers the account of a join. Nothing here is a local edit.
   */
  receiveSharing(change: SharingChange): AccountId | undefined {
    const answer = this.#sync.remote(() => this.#sharing.receive(change));
    // A member who left or was removed, a space that closed: their presence goes with them, and a closed space's links.
    this.#presence.reconcile();
    this.#links.reconcile();
    return answer;
  }

  /**
   * Test hook: another person's device sends presence, as the socket delivers it (see `PresenceChange`): a frame (it appears,
   * moves to a page, edits with a cursor, goes idle) or leaves. The rules of an accepted frame apply (`InvalidInput` for a
   * space that is not live and shared, a member that is this account or none, a page of another space); `Event.PresenceChanged`
   * fires when a device appears, changes page or state, or goes, and the frame reaches `presence.subscribe` listeners as
   * `PresenceUpdate`. A device whose latest frame is 30 seconds old is gone (`presenceExpire`). Nothing here is a local edit.
   */
  receivePresence(change: PresenceChange): void {
    this.#presence.receive(change);
  }

  /** Test hook: takes `count` (default 1) steps of the scripted people of `MemoryTransportOptions.presence`. */
  presenceStep(count = 1): void {
    this.#presence.step(count);
  }

  /** Test hook: devices whose latest frame is 30 seconds old are gone now (the mirror also does it on a timer, and when asked). */
  presenceExpire(): void {
    this.#presence.expire();
  }

  /** The presence frames this device sent into live shared spaces, oldest first (`presence.send` of a personal space sends none). */
  sentPresence(): readonly SentPresence[] {
    return this.#presence.sent();
  }

  // ----- app lock (ADR-0018) --------------------------------------------------------

  /**
   * Closes the workspace: `coreState()` becomes `"locked"`, `dispatch` and `docs` reject with
   * `Locked` until `unlock`. `InvalidInput` for `lock` when the app lock is not enabled;
   * locking a locked workspace succeeds, as in the shell.
   */
  lock(): Promise<void> {
    return this.#run(() => {
      if (this.#state === "locked") {
        return;
      }
      this.#checkReady();
      if (!this.#security.lockEnabled) {
        throw lockNotEnabled();
      }
      this.#state = "locked";
      this.#sync.stop();
      this.#pairing.stop();
      this.#recovery.stop();
      this.#sharing.stop();
      // Presence is in memory only; the uploads of new view links finish.
      this.#presence.drop();
      this.#links.stop();
      // The sessions of an export and of an import are in memory only: a lock drops them.
      this.#export.drop();
      this.#import.drop();
      // Streams of the AI end, and its indexer stops.
      this.#ai.lock();
    });
  }

  /**
   * Reopens a locked workspace with the app-lock passphrase; a wrong one is `InvalidInput` for
   * `passphrase` and the core stays locked. A workspace that is not locked is `InvalidInput`
   * ("the workspace is not locked"), as in the shell.
   */
  unlock(passphrase: string): Promise<void> {
    return this.#run(async () => {
      if (this.#state === "ready") {
        throw new CoreError({ code: "InvalidInput", message: "the workspace is not locked" });
      }
      if (this.#state !== "locked") {
        this.#checkReady();
      }
      if (!(await this.#security.passphraseMatches(passphrase))) {
        throw invalid("passphrase", "wrong passphrase");
      }
      if (this.#state === "locked") {
        this.#state = "ready";
        // Unlocking opens the workspace again, and the core collects at every open and expires the trash.
        this.#attachments.gc();
        this.#expireTrash();
        this.#trashChecked = null;
        // The shell restarts the sync driver once the core is open again.
        this.#sync.resume();
        this.#ai.unlock();
      }
    });
  }

  // ----- backups (ADR-0017; the mirror's own file format, see `./backup`) ------------

  /**
   * Writes a backup of the whole workspace to `path` in `files` (absolute, as the shell wants
   * it), or, without a path, to the file the save dialog would propose
   * (`/backups/atomik-kb-<yyyy-mm-dd>.atomikkb`; cancelled with `cancelDialogs`: `InvalidInput`
   * for `path`). Needs the workspace (`InvalidInput` for `workspace`) and a recovery kit
   * (`InvalidInput` for `kit`).
   */
  async backupExport(path?: string): Promise<BackupResult> {
    this.#checkReady();
    const target =
      path === undefined
        ? this.#saveDialog(`/backups/atomik-kb-${new Date(this.#now()).toISOString().slice(0, 10)}.atomikkb`)
        : absolutePath(path);
    const workspace = this.#workspace;
    if (workspace === null) {
      throw invalid("workspace", "there is no workspace to back up");
    }
    const security = await this.#security.backupState();
    if (security === undefined) {
      throw invalid("kit", "create your recovery kit before exporting a backup");
    }
    const spaces = this.#properties.exportState(this.#structure.exportState());
    const state: BackupState = {
      workspace: { ...workspace },
      // The settings of this device (the folder, the Drive connection, the endpoint) never travel in a backup.
      settings: [...this.#settings]
        .filter(([key]) => !DEVICE_BOUND_SETTINGS.includes(key))
        .map(([key, value]) => ({ key, value: JSON.parse(value) as JsonValue })),
      spaces,
      documents: this.#content.exportDocuments(spaces.flatMap((space) => space.pages.map((page) => page.id))),
      security: security.state,
      attachments: this.#attachments.exportState(),
    };
    const header: BackupHeader = { ...security.identity, createdAt: this.#now() };
    const bytes = await encodeBackup(header, state);
    // Re-inserted, so that the open dialog finds it as the most recent file.
    this.files.delete(target);
    this.files.set(target, bytes);
    return { summary: backupSummary(header, state, bytes.length), path: target };
  }

  /**
   * Restores a backup from `path` in `files`, or, without a path, from the file the open
   * dialog would pick (the most recently written one; cancelled with `cancelDialogs` or when
   * there is none: `InvalidInput` for `path`). See the class documentation for the checks;
   * everything is read and checked before anything changes.
   */
  async backupImport(path?: string, kit?: string): Promise<BackupResult> {
    this.#checkReady();
    const source = path === undefined ? this.#openDialog() : absolutePath(path);
    const bytes = this.files.get(source);
    if (bytes === undefined) {
      throw invalid("path", "the backup file cannot be opened: no such file");
    }
    this.#requireNoWorkspace();
    const { header, body } = await readBackupFile(bytes);
    if (kit !== undefined) {
      const key = await parseKit(kit);
      const recoveryId = await recoveryIdOf(key);
      key.fill(0);
      if (recoveryId !== header.recoveryId) {
        throw invalid("kit", "this kit does not belong to this backup");
      }
    } else if (header.accountId !== this.#security.accountId) {
      throw invalid("kit", "the recovery kit is required to restore this backup");
    }
    const state = readBackupState(body);
    this.#checkReady();
    this.#requireNoWorkspace();
    let settings: [string, string][];
    let commits: (() => void)[];
    try {
      // A crafted backup must not carry another device's folder, consent or connection: those are ignored.
      settings = state.settings
        .filter(({ key }) => !DEVICE_BOUND_SETTINGS.includes(key))
        .map(({ key, value }): [string, string] => {
          validateSettingKey(key);
          return [key, serialiseSettingValue(value)];
        });
      commits = [
        this.#structure.prepareRestore(state.spaces),
        this.#properties.prepareRestore(state.spaces),
        this.#content.prepareRestore(state.documents),
        this.#security.prepareRestore(header, state.security, kit !== undefined),
        this.#attachments.prepareRestore(state.attachments),
      ];
    } catch (error) {
      throw new CoreError({ code: "Crypto", message: "not a valid backup file: records" }, { cause: error });
    }
    for (const commit of commits) {
      commit();
    }
    // The documents are new, so the journal's versions are gone (a journal is never in a backup).
    this.#history.reset();
    for (const [key, value] of settings) {
      this.#settings.set(key, value);
    }
    this.#workspace = { ...state.workspace };
    this.#emit({ type: "WorkspaceCreated", data: { id: state.workspace.id } });
    this.#emit({ type: "SpacesChanged" });
    this.#emit({ type: "SecurityChanged" });
    // The attachment index is derived: rebuilt from the restored manifests and documents.
    this.#attachments.refresh();
    return { summary: backupSummary(header, state, bytes.length), path: source };
  }

  // ----- import, export and scheduled backups (ADR-0036; the shell's commands) ---------

  /**
   * `export_run`: begins an export, asks where to save (the save dialog proposes
   * `/downloads/<suggested name>`; cancelled with `cancelDialogs`: `InvalidInput` for `path`, and the
   * session ends), drives `ExportNext` in batches of 50 entries and writes the result to `files`:
   * a single file as the text of its page, an archive as a JSON listing of its entries
   * ({@link encodeMemoryArchive}; attachments with their bytes in base64). An attachment that cannot
   * be read is left out with `attachmentUnreadable`. Answers the path, the pages and attachments
   * written and the warnings.
   */
  exportRun(options: ExportRunOptions): Promise<ExportRunResult> {
    return this.#run(() => this.#exportNow(options));
  }

  #exportNow(options: ExportRunOptions): ExportRunResult {
    this.#checkReady();
    const exporter = this.#export;
    const plan = exporter.begin(options.spaceId, options.pageId, options.includeChildren, options.format);
    try {
      const path = this.#saveDialog(`/downloads/${plan.suggestedName}`);
      const entries: MemoryArchiveEntry[] = [];
      const warnings: ExportWarning[] = [];
      let attachments = 0;
      for (let done = false; !done; ) {
        const batch = exporter.next(plan.exportId, 50);
        warnings.push(...(batch.warnings ?? []));
        for (const entry of batch.entries) {
          if (entry.content.type === "Text") {
            entries.push({ path: entry.path, text: entry.content.data.text });
            continue;
          }
          const { attachmentId, size } = entry.content.data;
          try {
            entries.push({
              path: entry.path,
              attachmentId,
              size,
              base64: toBase64(this.#attachments.read(attachmentId, undefined)),
            });
            attachments += 1;
          } catch (error) {
            if (!(error instanceof CoreError)) {
              throw error;
            }
            warnings.push({ code: "attachmentUnreadable", attachmentId });
          }
        }
        done = batch.done;
      }
      const first = entries[0];
      const bytes = plan.singleFile && first?.text !== undefined ? utf8.encode(first.text) : encodeMemoryArchive(entries);
      // Re-inserted, so that the open dialog finds it as the most recent file.
      this.files.delete(path);
      this.files.set(path, bytes);
      return { path, pages: plan.pages, attachments, warnings };
    } finally {
      exporter.end(plan.exportId);
    }
  }

  /**
   * `import_run`: picks a canned source of the kind asked for (`importSources`; none, or
   * `cancelDialogs`: `InvalidInput` for `path`), reads it like the shell does (hidden entries,
   * `__MACOSX` and `node_modules` skipped, unsafe paths left out with `unsupportedFile`, text files
   * decoded with `notUtf8` and `tooLarge` as needed), then runs the session: `ImportBegin`, the text
   * files, `ImportPlan`, an upload for every needed file (named after its last path segment; one that
   * fails is `attachmentFailed` and not reported), `ImportCommit` by 50 pages until done. A failure
   * aborts the session and is rethrown. Answers the core's report with the shell's warnings added.
   * With `destination: "newSpaces"` (ADR-0039) the session is `ImportBeginSpaces` (the source name is the
   * zip's name without `.zip`, a folder's name, none for files), each needed file is uploaded into the space
   * the plan gave it (`fileSpaces`), and the images the plan lists as remote files are not downloaded. For an
   * AppFlowy backup every `.json` file is read as text and `collab_objects/` is skipped silently.
   */
  importRun(options: ImportRunOptions): Promise<ImportReport> {
    return this.#run(() => this.#importNow(options));
  }

  #importNow(options: ImportRunOptions): ImportReport {
    this.#checkReady();
    const destination = options.destination ?? "page";
    if (destination === "page") {
      this.#sharing.require(options.spaceId, "editor");
    }
    const { name, source } = this.#pickImportSource(options.source, destination);
    const shell: ImportWarning[] = [];
    const texts = new Map<string, string>();
    const binary = new Map<string, Uint8Array>();
    // An AppFlowy backup (ADR-0039, 6): every `.json` file is text, `collab_objects/` is skipped silently.
    const backup = appFlowyPrefix(Object.keys(source.files).filter((path) => !isSkippedPath(path)));
    for (const [path, content] of Object.entries(source.files)) {
      if (isSkippedPath(path) || (backup !== undefined && path.startsWith(`${backup}collab_objects/`))) {
        continue;
      }
      if (!isSafeRelativePath(path)) {
        shell.push({ path, code: "unsupportedFile" });
      } else if (isTextFile(path) || (backup !== undefined && path.toLowerCase().endsWith(".json"))) {
        const bytes = typeof content === "string" ? utf8.encode(content) : content;
        if (bytes.length > MAX_IMPORT_FILE_BYTES) {
          shell.push({ path, code: "tooLarge" });
          continue;
        }
        const decoded = typeof content === "string" ? { text: content, lossy: false } : decodeSourceText(content);
        if (decoded.lossy) {
          shell.push({ path, code: "notUtf8" });
        }
        texts.set(path, decoded.text);
      } else {
        binary.set(path, typeof content === "string" ? utf8.encode(content) : content);
      }
    }
    const importer = this.#import;
    const sourceName = options.source === "files" ? "" : options.source === "zip" ? name.replace(/\.zip$/iu, "") : name;
    const importId =
      destination === "newSpaces"
        ? importer.beginSpaces(sourceName)
        : importer.begin(options.spaceId, options.parentId, sourceName);
    let report: ImportReport | undefined;
    try {
      for (const [path, text] of texts) {
        importer.addFile(importId, path, text);
      }
      const planned = importer.plan(importId, [...binary.keys()]);
      // The images the source keeps on a server (`planned.remoteFiles`) are not downloaded here: they stay links.
      for (const [at, path] of planned.neededFiles.entries()) {
        const bytes = binary.get(path);
        try {
          if (bytes === undefined || bytes.length > MAX_ATTACHMENT_BYTES) {
            throw invalid("path", "the file cannot be uploaded");
          }
          // Each file goes into the space the plan gave it (a destination of new spaces), else the destination's.
          const space = planned.fileSpaces?.[at] ?? options.spaceId;
          importer.addAttachment(importId, path, this.#uploadBytes(space, path, bytes).attachmentId);
        } catch (error) {
          if (!(error instanceof CoreError)) {
            throw error;
          }
          shell.push({ path, code: "attachmentFailed" });
        }
      }
      while (report === undefined) {
        report = importer.commit(importId, 50).report;
      }
    } catch (error) {
      importer.abort(importId);
      throw error;
    }
    const listed = [...(report.warnings ?? []), ...shell];
    const merged: ImportReport = {
      ...report,
      moreWarnings: report.moreWarnings + Math.max(0, listed.length - MAX_REPORT_WARNINGS),
    };
    if (listed.length > 0) {
      merged.warnings = listed.slice(0, MAX_REPORT_WARNINGS);
    }
    return merged;
  }

  /**
   * The canned source the open dialog picks for `kind`: the first of that kind; for a destination of new
   * spaces the first that is an AppFlowy backup when there is one (the dialog of "Import AppFlowy
   * workspace...", where the person picks a backup).
   */
  #pickImportSource(
    kind: ImportSourceKind,
    destination: "page" | "newSpaces",
  ): { name: string; source: MemoryImportSource } {
    if (!this.#cancelDialogs) {
      const candidates = Object.entries(this.#importSources).filter(([, source]) => source.kind === kind);
      const backup =
        destination === "newSpaces"
          ? candidates.find(([, source]) => appFlowyPrefix(Object.keys(source.files)) !== undefined)
          : undefined;
      const picked = backup ?? candidates[0];
      if (picked !== undefined) {
        return { name: picked[0], source: picked[1] };
      }
    }
    throw dialogCancelled();
  }

  /** The shell's upload of a file read from disk: begin, writes of 4 MiB, finish; an upload that fails aborts. */
  #uploadBytes(spaceId: SpaceId, path: string, bytes: Uint8Array): AttachmentInfo {
    // An injected failure (`failNextAttachments`) makes the upload fail, as for any attachment call.
    this.#checkReady(this.#injectedAttachments);
    this.#sharing.require(spaceId, "editor");
    const name = importName(path.split("/").at(-1) ?? "");
    const attachments = this.#attachments;
    const attachmentId = attachments.begin(spaceId, name, mimeForName(name));
    try {
      for (let start = 0; start < bytes.length; start += CHUNK_SIZE) {
        attachments.write(attachmentId, bytes.subarray(start, start + CHUNK_SIZE));
      }
      return attachments.finish(attachmentId);
    } catch (error) {
      attachments.abort(attachmentId);
      throw error;
    }
  }

  /**
   * `save_text_file`: writes `text` (at most 64 MiB) to the file the save dialog proposes,
   * `/downloads/<suggested name made safe for a path>` (cancelled with `cancelDialogs`: `InvalidInput`
   * for `path`); answers the path.
   */
  saveTextFile(suggestedName: string, text: string): Promise<string> {
    return this.#run(() => this.#saveTextNow(suggestedName, text));
  }

  #saveTextNow(suggestedName: string, text: string): string {
    this.#checkReady();
    const bytes = utf8.encode(text);
    if (bytes.length > MAX_SAVED_TEXT_BYTES) {
      throw limitExceeded("text", "the text is too large to save", MAX_SAVED_TEXT_BYTES);
    }
    const path = this.#saveDialog(`/downloads/${sanitisedName(suggestedName) ?? "Untitled"}`);
    // Re-inserted, so that the open dialog finds it as the most recent file.
    this.files.delete(path);
    this.files.set(path, bytes);
    return path;
  }

  /**
   * `backup_folder_choose`: the folder dialog picks `/backups/scheduled` (`MemoryTransportOptions.dialogFolder`; cancelled with
   * `cancelDialogs`: `InvalidInput` for `path`); the folder is stored in the setting
   * `app.backup.folder`, which only the shell writes (`SettingChanged` follows); answers it.
   */
  backupFolderChoose(): Promise<string> {
    return this.#run(() => {
      this.#checkReady();
      const folder = this.#saveDialog(this.#dialogFolder);
      this.#shellSetting(BACKUP_FOLDER_SETTING, folder);
      return folder;
    });
  }

  /**
   * `backup_folder_list` (ADR-0059, for the restore wizard): the backups of a folder of the fake disk, the `.atomikkb` files
   * directly in it (not in its sub-folders), newest first, at most 200, each `id` the file's path, which `backupImport` takes.
   * Without a path the folder dialog picks `/backups/scheduled` (`MemoryTransportOptions.dialogFolder`; cancelled with
   * `cancelDialogs`: `InvalidInput` for `path`, "no folder was chosen"). A path must be absolute (`InvalidInput` for `path`); a
   * folder with no backups lists none. The fake disk keeps no times: a file's time is the creation time in its header, `0`
   * for a file that is not a backup, and a tie goes to the file written later. Needs the core ready, like `backupImport`.
   */
  async backupFolderList(path?: string): Promise<BackupFolderListing> {
    this.#checkReady();
    let chosen: string;
    if (path === undefined) {
      if (this.#cancelDialogs) {
        throw invalid("path", "no folder was chosen");
      }
      chosen = this.#dialogFolder;
    } else {
      chosen = absolutePath(path);
    }
    // Without a trailing separator, whichever one the platform's paths use.
    const folder = chosen.replace(/[\\/]+$/u, "");
    const found: { id: string; name: string; size: number; modifiedAt: number; position: number }[] = [];
    let position = 0;
    for (const [filePath, bytes] of this.files) {
      position += 1;
      const prefix = [`${folder}/`, `${folder}\\`].find((candidate) => filePath.startsWith(candidate));
      if (prefix === undefined) {
        continue;
      }
      const name = filePath.slice(prefix.length);
      if (name === "" || /[\\/]/u.test(name) || !/\.atomikkb$/iu.test(name)) {
        continue;
      }
      let modifiedAt = 0;
      try {
        modifiedAt = (await readBackupFile(bytes)).header.createdAt;
      } catch {
        // Not a backup (or damaged): listed all the same, as the oldest; restoring it says what is wrong.
      }
      found.push({ id: filePath, name, size: bytes.length, modifiedAt, position });
    }
    found.sort((a, b) => b.modifiedAt - a.modifiedAt || b.position - a.position);
    return {
      folder: folder === "" ? chosen : folder,
      backups: found.slice(0, MAX_LISTED_BACKUPS).map(({ id, name, size, modifiedAt }) => ({ id, name, size, modifiedAt })),
    };
  }

  /**
   * `backup_run_now` (ADR-0036, ADR-0059): one export, named `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb` (local time), into the
   * chosen folder (the scheduled backups beyond the newest `keep` of the schedule, setting `app.backup.schedule`, default 7,
   * are deleted; only files of exactly that name form) or, with no folder, into a staging file of the fake disk that is
   * deleted afterwards; then delivered to the fake Drive when it is connected. The outcome is recorded in `app.backup.last`
   * (`BackupRunStatus`; a failure too, with its code and message, keeping the last success): a success only when every
   * target that is on succeeded, the error the first failure. No target (no folder, Drive not connected) is `InvalidInput` for
   * `folder`; the backup itself needs the workspace and a recovery kit, like `backupExport`. Answers the folder's file, or the
   * staging file's path. A person's press of "Back up now": always a fresh export, even right after a Drive failure
   * (see {@link MemoryTransport.runScheduledBackup} for the timer's run).
   */
  backupRunNow(): Promise<BackupResult> {
    return this.#runBackup(false);
  }

  /**
   * Test hook: the run the shell's timer makes when a backup is due (the mirror has no timer), which is `backupRunNow` except
   * for the retry of a Drive failure. After a run whose folder part worked and only Drive failed, the failed run's status
   * names the folder's file (`file`); while that file is still there, is newer than anything Drive has and its period is not
   * over, this run delivers it to Drive again and exports nothing, so that a Drive that stays down never pushes the folder's
   * older backups out through the pruning. With no folder, or in a new period, it exports as usual.
   */
  runScheduledBackup(): Promise<BackupResult> {
    return this.#runBackup(true);
  }

  async #runBackup(retry: boolean): Promise<BackupResult> {
    this.#checkReady();
    const at = this.#now();
    const previous = this.#backupStatus();
    // The scheduled backup the folder holds of this attempt (written, or delivered again), and when it was made.
    let folderFile: string | undefined;
    try {
      const stored = this.#storedSetting(BACKUP_FOLDER_SETTING);
      const folder = typeof stored === "string" && stored !== "" ? stored : undefined;
      const drive = this.drive?.connected === true ? this.drive : undefined;
      if (folder === undefined && drive === undefined) {
        throw invalid("folder", "no backup folder is chosen");
      }
      const schedule = scheduleOf(this.#storedSetting(BACKUP_SCHEDULE_SETTING));
      const keep = schedule.keep;
      const held =
        retry && folder !== undefined && drive !== undefined
          ? await this.#heldForRetry(folder, drive, previous, schedule.interval, at)
          : undefined;
      let result: BackupResult;
      let file = autoBackupName(at);
      let madeAt: number | undefined;
      let failure: CoreError | undefined;
      if (held !== undefined && drive !== undefined) {
        ({ result, name: file, madeAt } = held);
        folderFile = file;
        failure = await this.#deliverTo(drive, file, held.bytes, keep);
      } else {
        const staged = folder === undefined ? `${BACKUP_STAGING_FOLDER}/${file}` : undefined;
        try {
          result = await this.backupExport(staged ?? `${folder ?? ""}/${file}`);
          if (folder !== undefined) {
            folderFile = file;
            const prefix = `${folder}/`;
            const names = [...this.files.keys()]
              .filter((path) => path.startsWith(prefix) && !path.slice(prefix.length).includes("/"))
              .map((path) => path.slice(prefix.length));
            for (const name of backupsToPrune(names, keep)) {
              this.files.delete(`${prefix}${name}`);
            }
          }
          if (drive !== undefined) {
            const bytes = this.files.get(result.path);
            failure = bytes === undefined ? internalError("the backup file is not there") : await this.#deliverTo(drive, file, bytes, keep);
          }
        } finally {
          // The staging file goes whatever became of the delivery.
          if (staged !== undefined) {
            this.files.delete(staged);
          }
        }
      }
      if (failure !== undefined) {
        throw failure;
      }
      const now = this.#now();
      this.#shellSetting(BACKUP_LAST_SETTING, { at: now, ok: true, lastSuccessAt: madeAt ?? now, file });
      return result;
    } catch (error) {
      const known = error instanceof CoreError ? error : internalError("the backup failed");
      const failed: BackupRunStatus = { at: this.#now(), ok: false, error: `${known.code}: ${known.message}` };
      if (previous?.lastSuccessAt !== undefined) {
        failed.lastSuccessAt = previous.lastSuccessAt;
      }
      const file = folderFile ?? previous?.file;
      if (file !== undefined) {
        failed.file = file;
      }
      this.#shellSetting(BACKUP_LAST_SETTING, { ...failed });
      throw error;
    }
  }

  /** Delivers a file to the fake Drive; the failure (not a throw) when it did not go, `undefined` when it did. */
  async #deliverTo(drive: MemoryDrive, name: string, bytes: Uint8Array, keep: number): Promise<CoreError | undefined> {
    try {
      return (await drive.deliver(name, bytes, keep)) ? undefined : driveFailure("notConnected");
    } catch (error) {
      return error instanceof CoreError ? error : internalError("the delivery to Google Drive failed");
    }
  }

  /**
   * The backup a retry delivers to Drive again instead of exporting once more (`backupRunNow`): the file the previous, failed
   * run left in the folder, when it is still there and a backup, newer than anything Drive has, and its period is not over.
   */
  async #heldForRetry(
    folder: string,
    drive: MemoryDrive,
    previous: BackupRunStatus | undefined,
    interval: BackupSchedule["interval"],
    now: number,
  ): Promise<{ name: string; madeAt: number; bytes: Uint8Array; result: BackupResult } | undefined> {
    const name = previous?.ok === false ? previous.file : undefined;
    if (name === undefined || !isAutoBackupName(name)) {
      return undefined;
    }
    const path = `${folder}/${name}`;
    const bytes = this.files.get(path);
    const madeAt = backupNameTime(name);
    if (bytes === undefined || madeAt === undefined || backupPeriodOver(madeAt, now, interval)) {
      return undefined;
    }
    // The names are zero-padded times, so the text order is the time order.
    const newest = drive.newestDelivered;
    if (newest !== undefined && newest >= name) {
      return undefined;
    }
    try {
      const { header, body } = await readBackupFile(bytes);
      return { name, madeAt, bytes, result: { summary: backupSummary(header, readBackupState(body), bytes.length), path } };
    } catch {
      // Not a backup any more: a normal export.
      return undefined;
    }
  }

  /**
   * The live pages of a scope with the text of their blocks, for the AI (`./memory-ai-text`): a space's pages, a
   * page alone (none when it is in the trash) or every space; a template and the pages inside it are never among them
   * (ADR-0056). `NotFound` for an unknown space or page.
   */
  #aiPages(scope: AiScope): AiPageText[] {
    const structure = this.#structure;
    let metas: PageMeta[];
    switch (scope.type) {
      case "space":
        if (structure.spacePageIds(scope.spaceId) === undefined) {
          throw new CoreError({ code: "NotFound", message: "space not found" });
        }
        metas = structure.livePages(scope.spaceId);
        break;
      case "page": {
        const meta = structure.pageMeta(scope.pageId);
        if (meta === undefined) {
          throw new CoreError({ code: "NotFound", message: "page not found" });
        }
        metas = meta.trashed ? [] : [meta];
        break;
      }
      case "all":
        metas = structure.livePages();
        break;
    }
    const titleOf = (pageId: PageId): string | undefined => structure.pageMeta(pageId)?.title;
    // A space whose AI is off (a shared one until this account opts in) is neither indexed nor retrieved (ADR-0047), and
    // neither is a template or a page inside one (ADR-0056), whatever the scope.
    metas = metas.filter((meta) => !meta.inTemplate && this.#sharing.aiEnabled(meta.spaceId));
    return metas.map((meta): AiPageText => {
      const stored = this.#content.storedContent(meta.id);
      const page: AiPageText = {
        pageId: meta.id,
        spaceId: meta.spaceId,
        title: meta.title,
        blocks: stored === undefined ? [] : blocksOfPortable(stored, titleOf),
      };
      if (meta.icon !== undefined) {
        page.icon = meta.icon;
      }
      return page;
    });
  }

  /** A stored setting's value; `undefined` when it is not set. */
  #storedSetting(key: string): JsonValue | undefined {
    const stored = this.#settings.get(key);
    return stored === undefined ? undefined : (JSON.parse(stored) as JsonValue);
  }

  /** The outcome of the last scheduled backup, as `app.backup.last` holds it. */
  #backupStatus(): BackupRunStatus | undefined {
    const stored = this.#storedSetting(BACKUP_LAST_SETTING);
    return typeof stored === "object" && stored !== null && !Array.isArray(stored) ? (stored as unknown as BackupRunStatus) : undefined;
  }

  /** Writes a setting the shell owns (the webview's `SettingsSet` refuses it) and announces it. */
  #shellSetting(key: string, value: JsonValue): void {
    this.#settings.set(key, serialiseSettingValue(value));
    this.#emit({ type: "SettingChanged", data: { key } });
  }

  // ----- attachment files (the shell's commands, on `files`) --------------------------

  /**
   * The shell's raw `attachment_read`: a range must be non-empty and at most
   * `MAX_PREVIEW_BYTES` long, and without one an attachment above `MAX_PREVIEW_BYTES` is
   * refused (`InvalidInput` for `size`, with `details.limit`); then the core reads.
   */
  #rawRead(attachmentId: AttachmentId, range: ByteRange | undefined): Uint8Array {
    if (range === undefined) {
      if (this.#attachments.get(attachmentId).size > MAX_PREVIEW_BYTES) {
        throw limitExceeded(
          "size",
          "the attachment is too large to read in one piece: ask for a byte range",
          MAX_PREVIEW_BYTES,
        );
      }
    } else {
      for (const [field, value] of [
        ["x-kb-range-start", range.start],
        ["x-kb-range-end", range.end],
      ] as const) {
        if (!Number.isSafeInteger(value) || value < 0) {
          throw invalid(field, "the value must be a decimal number of bytes");
        }
      }
      if (range.end <= range.start) {
        throw invalid("range", "the range must end after it starts");
      }
      if (range.end - range.start > MAX_PREVIEW_BYTES) {
        throw limitExceeded("range", "the range is longer than one read may answer; ask for it in pieces", MAX_PREVIEW_BYTES);
      }
    }
    return this.#attachments.read(attachmentId, range);
  }

  /**
   * `attachment_import_path`: the file at `path` in `files` (absolute), or the one the open
   * dialog picks, uploaded through begin, writes of 4 MiB and finish, named after the file and
   * with a MIME type from its extension (the shell's rules, `./shell-names`); a file above
   * 2 GiB is refused before anything starts, and any failure after begin aborts the upload.
   */
  #importPath(spaceId: SpaceId, path: string | undefined): AttachmentInfo {
    this.#sharing.require(spaceId, "editor");
    const source = path === undefined ? this.#openDialog() : absolutePath(path);
    const bytes = this.files.get(source);
    if (bytes === undefined) {
      throw invalid("path", "the file cannot be opened: no such file");
    }
    if (bytes.length > MAX_ATTACHMENT_BYTES) {
      throw limitExceeded("size", "the file is larger than an attachment may be", MAX_ATTACHMENT_BYTES);
    }
    const name = importName(source.split(/[\\/]/u).at(-1) ?? "");
    const attachments = this.#attachments;
    const attachmentId = attachments.begin(spaceId, name, mimeForName(name));
    try {
      for (let start = 0; start < bytes.length; start += CHUNK_SIZE) {
        attachments.write(attachmentId, bytes.subarray(start, start + CHUNK_SIZE));
      }
      return attachments.finish(attachmentId);
    } catch (error) {
      attachments.abort(attachmentId);
      throw error;
    }
  }

  /**
   * `attachment_import_url` (ADR-0028): the shell's checks on the link ({@link parseImageUrl}),
   * the picture from `remoteImages` or `fetch`, its type from its bytes (`notImage` otherwise),
   * then begin, writes of 4 MiB and finish, named after the last segment of the link's path with
   * the extension of the type; any failure after begin aborts the upload. The shell's real work
   * is network access; here the network is the canned map.
   */
  async #importUrl(spaceId: SpaceId, typed: string): Promise<AttachmentInfo> {
    this.#sharing.require(spaceId, "editor");
    const url = parseImageUrl(typed);
    const { bytes, contentType } = await loadRemoteImage(url, typed, this.#remoteImages);
    const kind = sniffImage(bytes, contentType);
    if (kind === undefined) {
      throw urlFailure("notImage", URL_MESSAGES.notImage);
    }
    const attachments = this.#attachments;
    const attachmentId = attachments.begin(spaceId, imageName(url, kind), imageMime(kind));
    try {
      for (let start = 0; start < bytes.length; start += CHUNK_SIZE) {
        attachments.write(attachmentId, bytes.subarray(start, start + CHUNK_SIZE));
      }
      return attachments.finish(attachmentId);
    } catch (error) {
      attachments.abort(attachmentId);
      throw error;
    }
  }

  /**
   * `attachment_save_as`: the attachment's bytes written to `path` in `files` (absolute), or to
   * the file the save dialog proposes (`/downloads/<name made safe for a path>`).
   */
  #saveAs(attachmentId: AttachmentId, path: string | undefined): AttachmentSaveResult {
    const info = this.#attachments.get(attachmentId);
    const target =
      path === undefined ? this.#saveDialog(`/downloads/${pathSafeName(info.name, attachmentId)}`) : absolutePath(path);
    const bytes = this.#attachments.read(attachmentId, undefined);
    // Re-inserted, so that the open dialog finds it as the most recent file.
    this.files.delete(target);
    this.files.set(target, bytes);
    return { info, path: target };
  }

  /**
   * `attachment_open`: refuses files the system would run (`InvalidInput` for `attachment`),
   * reads the whole attachment like the shell's private copy does, and answers that copy's
   * (fake) path, `/cache/open/<attachment id>/<name made safe for a path>`.
   */
  #openAttachment(attachmentId: AttachmentId): string {
    const info = this.#attachments.get(attachmentId);
    const name = pathSafeName(info.name, attachmentId);
    if (runsCode(name)) {
      throw invalid("attachment", "files of this type are not opened from here: save the file and open it yourself");
    }
    this.#attachments.read(attachmentId, undefined);
    return `/cache/open/${attachmentId}/${name}`;
  }

  /** The shell's save dialog: the proposed file, unless cancelled. */
  #saveDialog(proposed: string): string {
    if (this.#cancelDialogs) {
      throw dialogCancelled();
    }
    return proposed;
  }

  /** The shell's open dialog: the most recently written file, unless cancelled or there is none. */
  #openDialog(): string {
    const latest = [...this.files.keys()].at(-1);
    if (this.#cancelDialogs || latest === undefined) {
      throw dialogCancelled();
    }
    return latest;
  }

  #requireNoWorkspace(): void {
    if (this.#workspace !== null) {
      throw new CoreError({
        code: "AlreadyExists",
        message: "this installation already has a workspace; restoring needs a fresh one",
      });
    }
  }

  // ----- plumbing -------------------------------------------------------------------

  /**
   * Rejects like the shell while the core is not ready (`Locked` while locked), and serves
   * injected failures first.
   */
  #checkReady(injected: ApiError[] = []): void {
    const failure = injected.shift();
    if (failure !== undefined) {
      throw new CoreError(failure);
    }
    if (this.#state === "opening") {
      throw internalError("core is starting");
    }
    if (this.#state === "locked") {
      throw new CoreError({ code: "Locked", message: "the workspace is locked" });
    }
    if (typeof this.#state !== "string") {
      throw new CoreError(this.#state.failed);
    }
  }

  /**
   * `async` semantics without an `async` function: `task` runs synchronously (so its events
   * are delivered before this returns), and every failure, thrown or rejected, is a rejection.
   */
  #run<T>(task: () => T | Promise<T>): Promise<T> {
    return new Promise((resolve, reject) => {
      try {
        resolve(task());
      } catch (error) {
        reject(error instanceof Error ? error : internalError("memory transport failed"));
      }
    });
  }

  /** Runs one `docs` operation with `dispatch`'s semantics. */
  #docCall<T>(run: () => T): Promise<T> {
    return this.#run(() => {
      this.#checkReady(this.#injectedDocs);
      return run();
    });
  }

  /** Runs one `attachments` operation with `dispatch`'s semantics. */
  #attachmentCall<T>(run: () => T): Promise<T> {
    return this.#run(() => {
      this.#checkReady(this.#injectedAttachments);
      return run();
    });
  }

  /** {@link MemoryTransport.#attachmentCall} for an operation that takes time (a download). */
  #attachmentCallAsync<T>(run: () => Promise<T>): Promise<T> {
    return this.#run(() => {
      this.#checkReady(this.#injectedAttachments);
      return run();
    });
  }

  #handle(request: Request): Response | Promise<Response> {
    this.#checkReady(this.#injected);
    this.#authorise(request);

    const structure = this.#structure;
    const security = this.#security;
    const attachments = this.#attachments;
    switch (request.type) {
      case "AppStatus":
        return { type: "AppStatus", data: this.#appStatus() };
      case "WorkspaceCreate":
        return this.#workspaceCreate(request.data.name, request.data.firstSpaceName);
      case "WorkspaceGet":
        return { type: "Workspace", data: this.#workspace === null ? null : { ...this.#workspace } };
      case "SettingsGet": {
        validateSettingKey(request.data.key);
        const stored = this.#settings.get(request.data.key);
        return { type: "Setting", data: stored === undefined ? null : (JSON.parse(stored) as JsonValue) };
      }
      case "SettingsSet": {
        const { key, value } = request.data;
        validateSettingKey(key);
        // The shell writes these itself; the webview may not (ADR-0036, ADR-0037, ADR-0059).
        if (
          key === BACKUP_FOLDER_SETTING ||
          key === BACKUP_LAST_SETTING ||
          key === AI_ENDPOINT_SETTING ||
          key === BACKUP_DRIVE_SETTING
        ) {
          throw invalid("key", "this setting is written by the app itself");
        }
        this.#settings.set(key, serialiseSettingValue(value));
        this.#emit({ type: "SettingChanged", data: { key } });
        // The core reconciles the chunk table when the download policy turns eager.
        attachments.settingChanged(key);
        return { type: "Ok" };
      }
      case "DeviceInfo":
        return { type: "DeviceInfo", data: { ...this.#device } };
      case "DeviceRename": {
        const name = displayName(request.data.name, MAX_DEVICE_NAME_CHARS, "name");
        this.#device = { ...this.#device, name };
        this.#emit({ type: "DeviceRenamed", data: { name } });
        return { type: "DeviceInfo", data: { ...this.#device } };
      }

      case "SpaceList":
        return { type: "Spaces", data: structure.spaceList() };
      case "SpaceCreate":
        return { type: "Space", data: structure.spaceCreate(request.data.name, request.data.icon) };
      case "SpaceRename":
        return { type: "Space", data: structure.spaceRename(request.data.spaceId, request.data.name) };
      case "SpaceSetIcon":
        return { type: "Space", data: structure.spaceSetIcon(request.data.spaceId, request.data.icon) };
      case "SpaceMove":
        return { type: "Spaces", data: structure.spaceMove(request.data.spaceId, request.data.index) };
      case "SpaceDelete":
        // A shared space is left or closed (an archive is removed from this device with `SpaceLeave`).
        if (this.#sharing.isShared(request.data.spaceId)) {
          throw invalid("spaceId", "leave or close a shared space instead of deleting it");
        }
        // A personal space goes with all its pages, live and trashed, for good (as a purge from the trash).
        this.#dropSpace(request.data.spaceId);
        return { type: "Ok" };
      case "TreeGet":
        return { type: "Tree", data: structure.treeGet(request.data.spaceId) };
      case "PageGet":
        return { type: "Page", data: structure.pageGet(request.data.pageId) };
      case "PageCreate": {
        const { spaceId, parentId, index, title } = request.data;
        return { type: "Page", data: structure.pageCreate(spaceId, parentId, index, title) };
      }
      case "PageRename":
        return { type: "Page", data: structure.pageRename(request.data.pageId, request.data.title) };
      case "PageSetIcon": {
        // An image icon (`attachment:<id>`) references its attachment (ADR-0029): the one the page
        // had and the one it has now go through the reference bookkeeping.
        const before = iconAttachment(structure.pageMeta(request.data.pageId)?.icon);
        const page = structure.pageSetIcon(request.data.pageId, request.data.icon);
        attachments.refresh(
          [before, iconAttachment(page.icon)].filter((id): id is string => id !== undefined),
        );
        return { type: "Page", data: page };
      }
      case "PageSetCover": {
        // An image cover (`attachment:<id>`) references its attachment like an image icon (ADR-0040):
        // the one the page had and the one it has now go through the reference bookkeeping.
        const before = coverAttachment(structure.pageMeta(request.data.pageId)?.cover);
        const page = structure.pageSetCover(request.data.pageId, request.data.cover, request.data.coverY);
        attachments.refresh(
          [before, coverAttachment(page.cover)].filter((id): id is string => id !== undefined),
        );
        return { type: "Page", data: page };
      }
      case "PageSetAppearance": {
        const { pageId, font, smallText, locked } = request.data;
        return { type: "Page", data: structure.pageSetAppearance(pageId, { font, smallText, locked }) };
      }
      case "PageMove": {
        const { pageId, parentId, index } = request.data;
        structure.pageMove(pageId, parentId, index);
        return { type: "Ok" };
      }
      case "PageReorderChildren": {
        const { spaceId, parentId, order } = request.data;
        structure.pageReorderChildren(spaceId, parentId, order);
        return { type: "Ok" };
      }
      case "PageDuplicate":
        return { type: "Page", data: this.#duplicate.duplicate(request.data.pageId) };
      case "PageBulk": {
        const { spaceId, pageIds, action } = request.data;
        return { type: "PageBulk", data: this.#bulk.bulk(spaceId, pageIds, action) };
      }
      case "PageSetTemplate":
        return { type: "Page", data: structure.pageSetTemplate(request.data.pageId, request.data.template) };
      case "PageSetTemplatePlaceholder": {
        const { pageId, propertyId, placeholder } = request.data;
        return {
          type: "Page",
          data: structure.pageSetTemplatePlaceholder(pageId, propertyId, placeholder, (id) =>
            this.#properties.schemaGet(this.#structure.pageMeta(pageId)?.spaceId ?? "").find((def) => def.id === id)?.kind,
          ),
        };
      }
      case "PageCreateFromTemplate": {
        const { templateId, parentId, index, values } = request.data;
        return { type: "Page", data: this.#duplicate.createFromTemplate(templateId, parentId, index, values ?? []) };
      }
      case "PageTrash":
        structure.pageTrash(request.data.pageId);
        this.#content.sourcesMoved(structure.subtreeIds(request.data.pageId));
        return { type: "Ok" };
      case "PageRestore": {
        const page = structure.pageRestore(request.data.pageId);
        this.#content.sourcesMoved(structure.subtreeIds(request.data.pageId));
        return { type: "Page", data: page };
      }
      case "PagePurge":
        this.#purgePage(request.data.pageId);
        return { type: "Ok" };
      case "PageFavoriteSet":
        structure.pageFavoriteSet(request.data.pageId, request.data.favorite);
        return { type: "Ok" };
      case "PageFavoriteMove":
        structure.pageFavoriteMove(request.data.pageId, request.data.before);
        return { type: "Ok" };
      case "StructureUndo": {
        // An undo or redo can remove pages or bring purged ones back, with their references.
        const tree = structure.structureUndo(request.data.spaceId);
        attachments.refresh();
        return { type: "Tree", data: tree };
      }
      case "StructureRedo": {
        const tree = structure.structureRedo(request.data.spaceId);
        attachments.refresh();
        return { type: "Tree", data: tree };
      }

      case "Search":
        return { type: "Search", data: this.#content.search(request.data) };
      case "BacklinksGet":
        return { type: "Backlinks", data: this.#content.backlinks(request.data.pageId) };
      case "PageContentGet":
        return { type: "PageContent", data: this.#content.content(request.data.pageId) };
      case "PageHistoryList":
        return {
          type: "PageHistory",
          data: this.#history.list(request.data.pageId, request.data.before, request.data.limit),
        };
      case "PageHistoryGet":
        return { type: "PageVersionContent", data: this.#history.get(request.data.pageId, request.data.version) };
      case "PageHistoryRestore":
        return { type: "Page", data: this.#history.restore(request.data.pageId, request.data.version) };
      case "PageFirstImages":
        return { type: "PageFirstImages", data: this.#content.firstImages(request.data.spaceId, request.data.pageIds) };
      case "PageExcerpts":
        return { type: "PageExcerpts", data: this.#content.excerpts(request.data.spaceId, request.data.pageIds) };
      case "PageExportMarkdown":
        return { type: "Markdown", data: this.#content.markdown(request.data.pageId) };
      case "IndexRebuild":
        this.#content.rebuild(request.data.spaceId);
        attachments.refresh();
        return { type: "Ok" };

      case "PropertySchemaGet":
        return { type: "PropertySchema", data: { properties: this.#properties.schemaGet(request.data.spaceId) } };
      case "PropertyCreate": {
        const { spaceId, name, kind, config } = request.data;
        return { type: "Property", data: this.#properties.create(spaceId, name, kind, config) };
      }
      case "PropertyConfigSet": {
        const { spaceId, propertyId, config } = request.data;
        return { type: "Property", data: this.#properties.setConfig(spaceId, propertyId, config) };
      }
      case "PropertyRename": {
        const { spaceId, propertyId, name } = request.data;
        return { type: "Property", data: this.#properties.rename(spaceId, propertyId, name) };
      }
      case "PropertyDelete":
        this.#properties.delete(request.data.spaceId, request.data.propertyId);
        return { type: "Ok" };
      case "PropertyMove": {
        const { spaceId, propertyId, before } = request.data;
        this.#properties.move(spaceId, propertyId, before);
        return { type: "Ok" };
      }
      case "PropertyOptionAdd": {
        const { spaceId, propertyId, name, color, group } = request.data;
        return { type: "PropertyOption", data: this.#properties.optionAdd(spaceId, propertyId, name, color, group) };
      }
      case "PropertyOptionUpdate": {
        const { spaceId, propertyId, optionId, name, color, group } = request.data;
        return {
          type: "PropertyOption",
          data: this.#properties.optionUpdate(spaceId, propertyId, optionId, { name, color, group }),
        };
      }
      case "PropertyOptionDelete": {
        const { spaceId, propertyId, optionId } = request.data;
        this.#properties.optionDelete(spaceId, propertyId, optionId);
        return { type: "Ok" };
      }
      case "PagePropertySet": {
        const { pageId, propertyId, value } = request.data;
        return { type: "PageProperties", data: { values: this.#properties.pageSet(pageId, propertyId, value) } };
      }
      case "PagePropertiesGet":
        return { type: "PageProperties", data: { values: this.#properties.pageGet(request.data.pageId) } };
      case "PagesByProperty": {
        const { spaceId, propertyId, filter } = request.data;
        return { type: "PageIds", data: this.#properties.pagesBy(spaceId, propertyId, filter) };
      }

      case "RemindersDue": {
        const stored = this.#settings.get(LAST_CHECKED_SETTING);
        const mark: unknown = stored === undefined ? null : JSON.parse(stored);
        const last = typeof mark === "number" && Number.isInteger(mark) ? mark : null;
        const { until } = request.data;
        return {
          type: "Reminders",
          data: this.#content.dueReminders(windowStart(last, until), until, MAX_REMINDERS_DUE),
        };
      }

      case "SecurityStatus":
        return { type: "Security", data: security.status() };
      case "RecoveryKitCreate":
        return security.kitCreate().then((data): Response => ({ type: "RecoveryKit", data }));
      case "RecoveryKitVerify":
        return security.kitVerify(request.data.kit).then(ok);
      case "RecoveryKitReissue":
        return security.kitReissue(request.data.passphrase).then((data): Response => ({ type: "RecoveryKit", data }));
      case "AppLockEnable":
        return security.lockEnable(request.data.passphrase).then(ok);
      case "AppLockDisable":
        return security.lockDisable(request.data.passphrase).then(ok);
      case "AppLockChange":
        return security.lockChange(request.data.current, request.data.new).then(ok);
      case "DeviceList":
        return { type: "Devices", data: security.deviceList() };

      case "AttachmentBegin": {
        const { spaceId, name, mime } = request.data;
        return { type: "AttachmentStarted", data: attachments.begin(spaceId, name, mime) };
      }
      case "AttachmentFinish":
        return { type: "Attachment", data: attachments.finish(request.data.attachmentId) };
      case "AttachmentAbort":
        attachments.abort(request.data.attachmentId);
        return { type: "Ok" };
      case "AttachmentList":
        return { type: "Attachments", data: attachments.list(request.data.spaceId) };
      case "AttachmentGet":
        return { type: "Attachment", data: attachments.get(request.data.attachmentId) };
      case "AttachmentDownload":
        return { type: "Attachment", data: attachments.download(request.data.attachmentId) };
      case "AttachmentDelete":
        attachments.delete(request.data.attachmentId);
        return { type: "Ok" };
      case "AttachmentGc":
        return { type: "AttachmentGc", data: attachments.gc() };

      case "SyncConfigure":
        return { type: "Sync", data: this.#withFeeds(this.#sync.configure(request.data.serverUrl, request.data.inviteToken)) };
      case "SyncStatus":
        return { type: "Sync", data: this.#withFeeds(this.#sync.info()) };
      case "SyncNow":
        this.#sync.syncNow();
        return { type: "Ok" };

      case "PairingStart": {
        const { role, serverUrl, code } = request.data;
        return { type: "Pairing", data: this.#pairing.start(role, serverUrl, code) };
      }
      case "PairingConfirm":
        return { type: "Pairing", data: this.#pairing.confirm() };
      case "PairingCancel":
        return { type: "Pairing", data: this.#pairing.cancel() };
      case "PairingStatus":
        return { type: "Pairing", data: this.#pairing.info() };
      case "DeviceRevoke":
        return security
          .revoke(request.data.deviceId, request.data.passphrase)
          .then((data): Response => ({ type: "RecoveryKit", data }));
      case "RotationApply":
        return security.rotationApply(request.data.passphrase).then((data): Response => ({ type: "Security", data }));
      case "RecoveryStart":
        return this.#recovery
          .start(request.data.serverUrl, request.data.kit)
          .then((data): Response => ({ type: "Recovery", data }));
      case "RecoveryStatus":
        return { type: "Recovery", data: this.#recovery.info() };
      case "RecoveryCancel":
        return { type: "Recovery", data: this.#recovery.cancel() };

      case "ExportBegin": {
        const { spaceId, pageId, includeChildren, format } = request.data;
        return { type: "ExportPlanned", data: this.#export.begin(spaceId, pageId, includeChildren, format) };
      }
      case "ExportNext":
        return { type: "ExportBatch", data: this.#export.next(request.data.exportId, request.data.maxEntries) };
      case "ExportEnd":
        this.#export.end(request.data.exportId);
        return { type: "Ok" };
      case "ExportCsv": {
        const { spaceId, pageIds, columns } = request.data;
        return { type: "Csv", data: this.#export.csv(spaceId, pageIds, columns) };
      }
      case "ImportBegin": {
        const { spaceId, parentId, sourceName } = request.data;
        return { type: "ImportStarted", data: this.#import.begin(spaceId, parentId, sourceName) };
      }
      case "ImportAddFile": {
        const { importId, path, text } = request.data;
        this.#import.addFile(importId, path, text);
        return { type: "Ok" };
      }
      case "ImportPlan":
        return { type: "ImportPlanned", data: this.#import.plan(request.data.importId, request.data.binaryPaths) };
      case "ImportAddAttachment": {
        const { importId, path, attachmentId } = request.data;
        this.#import.addAttachment(importId, path, attachmentId);
        return { type: "Ok" };
      }
      case "ImportCommit":
        return { type: "ImportProgress", data: this.#import.commit(request.data.importId, request.data.maxPages) };
      case "ImportAbort":
        this.#import.abort(request.data.importId);
        return { type: "Ok" };

      // Local AI and MCP (ADR-0037, `./memory-ai`, `./memory-mcp`).
      case "AiIndexStatus":
        return { type: "AiIndex", data: this.#ai.indexInfo() };
      case "AiIndexClear":
        this.#ai.indexClear();
        return ok();
      case "AiRetrieve": {
        const { query, vector, scope, limit } = request.data;
        return { type: "AiPassages", data: this.#ai.retrieve(query, scope, { vector, limit }) };
      }
      case "McpClientList":
        return { type: "McpClients", data: this.#mcp.list() };
      case "McpClientCreate":
        return { type: "McpClientCreated", data: this.#mcp.create(request.data.name) };
      case "McpClientDelete":
        this.#mcp.delete(request.data.clientId);
        return ok();
      case "McpAuditList": {
        const { clientId, before, limit } = request.data;
        return { type: "McpAudit", data: this.#mcp.auditList({ clientId, before, limit }) };
      }
      case "McpAuditClear":
        this.#mcp.auditClear();
        return ok();
      // Moves between spaces (ADR-0038, `./memory-move`).
      case "PageMoveToSpacePlan": {
        const { pageId, spaceId, parentId } = request.data;
        return { type: "SpaceMovePlanned", data: this.#move.plan(pageId, spaceId, parentId) };
      }
      case "PageMoveToSpace": {
        const { pageId, spaceId, parentId, index } = request.data;
        return { type: "PagesMovedToSpace", data: this.#move.move(pageId, spaceId, parentId, index) };
      }
      // Imports into new spaces (ADR-0039, `./memory-import`).
      case "ImportBeginSpaces":
        return { type: "ImportStarted", data: this.#import.beginSpaces(request.data.sourceName) };
      // Shared spaces (Milestone 13, ADR-0041 to ADR-0047, `./sharing-mirror`).
      case "SpaceSharePlan":
        return { type: "SpaceSharePlan", data: this.#sharing.plan(request.data.spaceId) };
      case "SpaceShare":
        return { type: "Space", data: this.#sharing.share(request.data.spaceId) };
      case "SpaceMembers":
        return { type: "Members", data: this.#sharing.members(request.data.spaceId) };
      case "MemberRoleSet": {
        const { spaceId, accountId, role } = request.data;
        return { type: "Members", data: this.#sharing.roleSet(spaceId, accountId, role) };
      }
      case "MemberRemove": {
        const members = this.#sharing.remove(request.data.spaceId, request.data.accountId);
        this.#presence.reconcile();
        return { type: "Members", data: members };
      }
      case "SpaceLeave":
        this.#sharing.leave(request.data.spaceId);
        this.#presence.reconcile();
        return ok();
      case "SpaceClose": {
        const space = this.#sharing.close(request.data.spaceId);
        this.#presence.reconcile();
        // The feed is deleted on the server, with the space's links.
        this.#links.reconcile();
        return { type: "Space", data: space };
      }
      case "SpaceSettingsSet": {
        const { spaceId, trashRetentionDays, editorsMayLink } = request.data;
        return { type: "Members", data: this.#sharing.settingsSet(spaceId, trashRetentionDays, editorsMayLink) };
      }
      case "SpaceAiSet":
        return { type: "Space", data: this.#sharing.aiSet(request.data.spaceId, request.data.enabled) };
      case "ProfileGet":
        return { type: "Profile", data: this.#sharing.profileGet() };
      case "ProfileSet":
        return { type: "Profile", data: this.#sharing.profileSet(request.data.displayName) };
      case "PersonGet":
        return { type: "Person", data: this.#sharing.personGet(request.data.accountId) };
      case "PersonVerifiedSet":
        return { type: "Person", data: this.#sharing.personVerifiedSet(request.data.accountId, request.data.verified) };
      case "InviteLinkCreate": {
        const { spaceId, role, expiresInDays } = request.data;
        return { type: "InviteLink", data: this.#sharing.linkCreate(spaceId, role, expiresInDays) };
      }
      case "InviteRevoke":
        return { type: "Members", data: this.#sharing.revoke(request.data.spaceId, request.data.inviteId) };
      case "InviteCodeStart":
        return { type: "Invite", data: this.#sharing.codeStart(request.data.spaceId, request.data.role) };
      case "InviteCodeJoin":
        return { type: "Invite", data: this.#sharing.codeJoin(request.data.serverUrl, request.data.code) };
      case "InviteLinkOpen":
        return { type: "Invite", data: this.#sharing.linkOpen(request.data.link) };
      case "InviteConfirm":
        return { type: "Invite", data: this.#sharing.confirm() };
      case "InviteCancel":
        return { type: "Invite", data: this.#sharing.cancel() };
      case "InviteStatus":
        return { type: "Invite", data: this.#sharing.status() };
      // Comments, person mentions and notifications (Milestone 13, slice 2, `./comments-mirror`): every member reads, the
      // writes follow the rules of the comment stream's fold (Commenter and up post, the author edits, the author or an
      // Owner deletes, the thread's author or an Editor resolves), the inbox is raised by the other people only.
      case "CommentList":
        return { type: "Comments", data: this.#comments.list(request.data.pageId) };
      case "CommentPost": {
        const { pageId, threadId, anchor, body } = request.data;
        return { type: "Thread", data: this.#comments.post(pageId, threadId, anchor, body) };
      }
      case "CommentEdit": {
        const { pageId, commentId, body } = request.data;
        return { type: "Thread", data: this.#comments.edit(pageId, commentId, body) };
      }
      case "CommentDelete":
        return { type: "Thread", data: this.#comments.delete(request.data.pageId, request.data.commentId) };
      case "CommentResolve": {
        const { pageId, threadId, resolved } = request.data;
        return { type: "Thread", data: this.#comments.resolve(pageId, threadId, resolved) };
      }
      case "NotificationList":
        return { type: "Notifications", data: this.#comments.notificationList(request.data) };
      case "NotificationsMarkRead":
        this.#comments.markRead(request.data.ids);
        return ok();
      case "PeopleMentionable":
        return { type: "People", data: this.#comments.people(request.data.spaceId) };
      // Realtime and presence (Milestone 13, slice 3, `./memory-presence`): who is where in a shared space now.
      case "PresenceList":
        return { type: "Presence", data: this.#presence.list(request.data.spaceId) };
      // View-only links (Milestone 13, slice 4, `./memory-viewlinks`): created by an Owner (or an Editor when the space allows it),
      // listed for every member without their URL (answered once, by the create), revoked by their creator or an Owner.
      case "ViewLinkCreate": {
        const { spaceId, pageId, expiresInDays } = request.data;
        return { type: "ViewLink", data: this.#links.create(spaceId, pageId, expiresInDays) };
      }
      case "ViewLinkList":
        return { type: "ViewLinks", data: this.#links.list(request.data.spaceId) };
      case "ViewLinkRevoke":
        return { type: "ViewLinks", data: this.#links.revoke(request.data.spaceId, request.data.linkId) };
      // The shell's own requests: its guard refuses them from the webview (`guard.rs`).
      case "AiIndexNext":
      case "AiIndexStore":
      case "McpCall":
      case "McpClientGrantsSet":
        throw invalid("type", "this request is not allowed from the page");

      default: {
        // Adding a variant to `Request` in kb-types stops compiling here until it is handled
        // above. At runtime this is only reachable from untyped callers; the real core fails
        // to deserialize such a request.
        const unhandled: never = request;
        throw internalError(`unknown request type: ${String((unhandled as { type?: unknown }).type)}`);
      }
    }
  }

  /**
   * A finished pairing or recovery: the workspace (`WorkspaceCreated`), the account with its kit
   * and device log (`SecurityChanged`), then sync to `serverUrl` (`SyncChanged`, `SyncRequested`).
   */
  #joinAccount(
    name: string,
    serverUrl: string,
    join: Omit<Parameters<MemorySecurity["joinAccount"]>[0], "accountId">,
  ): void {
    const workspace = viaWire<Workspace>({ id: this.#newId(), name, createdAt: this.#now() });
    this.#workspace = workspace;
    this.#emit({ type: "WorkspaceCreated", data: { id: workspace.id } });
    this.#security.joinAccount({ ...join, accountId: this.#newId() });
    this.#sync.configure(serverUrl, undefined);
  }

  /**
   * `SyncInfo.feeds` (Milestone 13): the personal feed first, then one per shared space, once sync is configured and
   * a space is shared (the mirror leaves the list out until then, so a status without sharing is what it always was).
   */
  #withFeeds(info: SyncInfo): SyncInfo {
    if (!info.configured) {
      return info;
    }
    const feeds = this.#sharing.feeds(info.pendingEntries);
    return feeds.length > 1 ? { ...info, feeds } : info;
  }

  /**
   * The gate of the writing requests (ADR-0043, `kb-core`'s `require_role`; the table "Role gating of requests" of
   * `milestone-13.md`, 1.5): Editor for the structure, content, properties, attachments, the name and icon of a space
   * and imports into it; Owner for purging, deleting attachments and moving pages out of a shared space; any write to
   * an archive is refused. Reads, `SpaceLeave`, `SpaceAiSet` and the favourites are open to every member.
   */
  #authorise(request: Request): void {
    const sharing = this.#sharing;
    const ofPage = (pageId: PageId): SpaceId | undefined => this.#structure.pageMeta(pageId)?.spaceId;
    switch (request.type) {
      case "SpaceRename":
      case "SpaceSetIcon":
      case "PageCreate":
      case "PageReorderChildren":
      case "PageBulk":
      case "StructureUndo":
      case "StructureRedo":
      case "PropertyCreate":
      case "PropertyConfigSet":
      case "PropertyRename":
      case "PropertyDelete":
      case "PropertyMove":
      case "PropertyOptionAdd":
      case "PropertyOptionUpdate":
      case "PropertyOptionDelete":
      case "AttachmentBegin":
      case "ImportBegin":
        sharing.require(request.data.spaceId, "editor");
        break;
      case "PageRename":
      case "PageSetIcon":
      case "PageSetCover":
      case "PageSetAppearance":
      case "PageMove":
      case "PageTrash":
      case "PageRestore":
      case "PageDuplicate":
      case "PageSetTemplate":
      case "PageSetTemplatePlaceholder":
      case "PageHistoryRestore":
      case "PagePropertySet":
        sharing.require(ofPage(request.data.pageId), "editor");
        break;
      case "PageCreateFromTemplate":
        sharing.require(ofPage(request.data.templateId), "editor");
        break;
      case "PagePurge":
        sharing.require(ofPage(request.data.pageId), "owner");
        break;
      case "AttachmentFinish":
      case "AttachmentAbort":
        sharing.require(this.#attachments.manifestOf(request.data.attachmentId)?.spaceId, "editor");
        break;
      case "AttachmentDelete":
        sharing.require(this.#attachments.manifestOf(request.data.attachmentId)?.spaceId, "owner");
        break;
      case "PageMoveToSpace":
        // Out of a shared space is Owner only; into one needs Editor.
        sharing.require(ofPage(request.data.pageId), "owner");
        sharing.require(request.data.spaceId, "editor");
        break;
      default:
        break;
    }
  }

  /** What the share dialog counts for a space (`SpaceSharePlan`). */
  #sharePlanCounts(spaceId: SpaceId): { pages: number; trashed: number; attachments: number; attachmentBytes: number } {
    const trashed = this.#structure.treeGet(spaceId).trashed.reduce((total, entry) => total + 1 + entry.descendantCount, 0);
    const files = this.#attachments.list(spaceId);
    return {
      pages: this.#structure.livePages(spaceId).length,
      trashed,
      attachments: files.length,
      attachmentBytes: files.reduce((total, file) => total + Number(file.size), 0),
    };
  }

  /** The local copy of a space goes with its pages, attachments, schema and grants (leaving, or removing an archive). */
  #dropSpace(spaceId: SpaceId): void {
    this.#attachments.spaceDeleted(spaceId);
    const pageIds = this.#structure.spaceDrop(spaceId);
    this.#content.pagesRemoved(pageIds);
    this.#attachments.refresh();
    this.#properties.spaceDeleted(spaceId);
    this.#mcp.spaceDeleted(spaceId);
    this.#security.spaceDeleted(spaceId);
    this.#sharing.spaceDeleted(spaceId);
    this.#links.spaceDeleted(spaceId);
  }

  /** The space an invitee receives with its history: its name and a couple of pages. */
  #joinedSpace(name: string): SpaceId {
    const space = this.#structure.spaceCreate(name, undefined);
    for (const title of ["Welcome", "Meeting notes"]) {
      this.#structure.pageCreate(space.id, undefined, undefined, title);
    }
    return space.id;
  }

  /**
   * Fork-out (ADR-0043): the unsent changes of pages become "Unsent changes - <title>" pages in a personal space (the
   * first one, else a new space named after the notice). The mirror copies the titles only.
   */
  #copyUnsent(pageIds: readonly PageId[]): { targetSpaceId: SpaceId; pageIds: PageId[] } {
    const targetSpaceId =
      this.#structure.spaceIds().find((id) => !this.#sharing.isShared(id)) ??
      this.#structure.spaceCreate("Unsent changes", undefined).id;
    const created = pageIds.map((pageId) => {
      const title = this.#structure.pageMeta(pageId)?.title ?? "";
      const copyTitle = Array.from(`Unsent changes - ${title}`).slice(0, 500).join("");
      return this.#structure.pageCreate(targetSpaceId, undefined, undefined, copyTitle).id;
    });
    return { targetSpaceId, pageIds: created };
  }

  #appStatus(): AppStatus {
    return { ...this.#status, workspaceInitialized: this.#workspace !== null };
  }

  /** The core validates both names before it looks for an existing workspace. */
  #workspaceCreate(rawName: string, rawFirstSpaceName: string | undefined): Response {
    const name = displayName(rawName, MAX_WORKSPACE_NAME_CHARS, "name");
    const firstSpaceName =
      rawFirstSpaceName === undefined
        ? undefined
        : displayName(rawFirstSpaceName, MAX_FIRST_SPACE_NAME_CHARS, "firstSpaceName");
    if (this.#workspace !== null) {
      throw new CoreError({ code: "AlreadyExists", message: "a workspace already exists" });
    }
    const workspace = viaWire<Workspace>({ id: this.#newId(), name, createdAt: this.#now() });
    this.#workspace = workspace;
    this.#emit({ type: "WorkspaceCreated", data: { id: workspace.id } });
    if (firstSpaceName !== undefined) {
      this.#structure.spaceCreate(firstSpaceName, undefined);
    }
    return { type: "Workspace", data: { ...workspace } };
  }

  /** `PagePurge`: the page and its subtree leave for good, with what the core cleans up after them. */
  #purgePage(pageId: PageId): void {
    const spaceId = this.#structure.pageMeta(pageId)?.spaceId;
    const purged = this.#structure.pagePurge(pageId);
    this.#attachments.refresh();
    this.#content.pagesRemoved(purged);
    if (spaceId !== undefined) {
      this.#security.pagesPurged(spaceId, purged);
    }
  }

  /**
   * `Core::trash_expire`: purges the top-level trashed pages past the limit; answers how many. Reads the clock only
   * when a page is in the trash (`nowMs` when the caller has read it already).
   */
  #expireTrash(nowMs?: number): number {
    const stored = this.#settings.get(TRASH_RETENTION_SETTING);
    const device = trashRetentionMs(stored === undefined ? undefined : (JSON.parse(stored) as JsonValue));
    // A shared space follows its own Settings entry, and only an Owner's device purges it (ADR-0047).
    const trashed = this.#structure.spaceIds().flatMap((spaceId) => {
      const policy = this.#sharing.trashPolicy(spaceId);
      const retention = policy === undefined ? device : policy === "skip" ? null : policy;
      return retention === null ? [] : this.#structure.treeGet(spaceId).trashed.map((entry) => ({ entry, retention }));
    });
    if (trashed.length === 0) {
      return 0;
    }
    const now = nowMs ?? this.#now();
    const expired = trashed
      .filter(({ entry, retention }) => isTrashExpired(entry.page.trashedAt, retention, now))
      .map(({ entry }) => entry.page.id);
    // The events of a run come once each, as the core drops the repeats.
    this.#collecting(() => {
      for (const pageId of expired) {
        this.#purgePage(pageId);
      }
    });
    return expired.length;
  }

  /**
   * Runs `run` with the events it causes held back, then delivers them once each, in the order of
   * their first occurrence (an import writes thousands of values and announces each change once),
   * also when `run` fails after writing something. Nested calls join the outer one.
   */
  #collecting<T>(run: () => T): T {
    if (this.#collected !== undefined) {
      return run();
    }
    const held: Event[] = [];
    this.#collected = held;
    try {
      return run();
    } finally {
      this.#collected = undefined;
      const seen = new Set<string>();
      for (const event of held) {
        const key = JSON.stringify(event);
        if (!seen.has(key)) {
          seen.add(key);
          this.#emit(event);
        }
      }
    }
  }

  #emit(event: Event): void {
    if (this.#collected !== undefined) {
      this.#collected.push(event);
      return;
    }
    for (const listener of [...this.#listeners]) {
      try {
        listener(event);
      } catch (error) {
        console.error("core event listener failed", error);
      }
    }
    // The core's persist hooks ask the sync driver for a cycle after every local change.
    if (isLocalEdit(event)) {
      this.#sync.localEdit();
    }
    // The semantic index sees what changed (it does not exist yet while the seeds emit nothing).
    (this.#ai as MemoryAi | undefined)?.onEvent(event);
  }
}

/** The answer of requests that return nothing. */
function ok(): Response {
  return { type: "Ok" };
}

/** `InvalidInput` for `field` that also names the limit (`details.limit`), as the shell words it. */
function limitExceeded(field: string, message: string, limit: number): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { field, limit } });
}

/** The body of the shell's raw `attachment_write`: 1 byte to one chunk (`InvalidInput` for `body` otherwise). */
function shellPiece(bytes: Uint8Array): Uint8Array {
  if (bytes.length === 0) {
    throw invalid("body", "a piece must not be empty");
  }
  if (bytes.length > CHUNK_SIZE) {
    throw limitExceeded("body", "a piece must not be larger than one chunk", CHUNK_SIZE);
  }
  return bytes;
}

/** What the shell answers when the user cancels a file dialog. */
function dialogCancelled(): CoreError {
  return invalid("path", "no file was chosen");
}

/**
 * An explicit backup path must be absolute, as in the shell: `/…` (POSIX), `C:\…` or `C:/…`
 * (a Windows drive) or `\\server\…`, whatever the platform the mirror runs on.
 */
function absolutePath(path: string): string {
  if (!/^(?:\/|[A-Za-z]:[\\/]|\\\\)/u.test(path)) {
    throw invalid("path", "the path must be absolute");
  }
  return path;
}

/** `kb-core` validate::setting_key. */
function validateSettingKey(key: string): void {
  if (key === "" || utf8.encode(key).length > MAX_SETTING_KEY_BYTES) {
    throw invalid("key", `must be 1 to ${String(MAX_SETTING_KEY_BYTES)} bytes`);
  }
  if (!/^[a-z0-9_.-]+$/.test(key)) {
    throw invalid("key", "may only contain a-z, 0-9, '_', '-' and '.'");
  }
  const [namespace = "", ...rest] = key.split(".");
  if (!SETTING_NAMESPACES.includes(namespace)) {
    throw invalid("key", `must start with one of: ${SETTING_NAMESPACES.join(", ")}`);
  }
  if (rest.length === 0) {
    throw invalid("key", "must have a name after the namespace");
  }
  if (rest.includes("")) {
    throw invalid("key", "must not contain empty segments");
  }
}

/**
 * Serialises a setting value as the wire would (JSON text) and enforces the size
 * limit. Storing text keeps stored values immune to later mutation by the caller.
 */
function serialiseSettingValue(value: JsonValue): string {
  let json: string | undefined;
  try {
    json = JSON.stringify(value);
  } catch (error) {
    throw invalid("value", `not serialisable: ${error instanceof Error ? error.message : "unknown error"}`);
  }
  if (json === undefined) {
    throw invalid("value", "not serialisable: undefined is not JSON");
  }
  if (utf8.encode(json).length > MAX_SETTING_BYTES) {
    throw invalid("value", `serialised value must be at most ${String(MAX_SETTING_BYTES)} bytes`);
  }
  return json;
}

/** The core names a new device after its platform, for example "Windows device". */
function defaultDeviceName(platform: string): string {
  const names: Record<string, string> = {
    windows: "Windows",
    macos: "macOS",
    linux: "Linux",
    android: "Android",
    web: "Web",
  };
  return `${names[platform] ?? platform} device`;
}

/**
 * Builds a record the way the wire delivers it: through JSON text, as Tauri IPC does.
 * That matters for timestamps (Unix milliseconds): ts-rs types Rust i64 as bigint
 * unless TS_RS_LARGE_INT says otherwise, but JSON can only carry a plain number, so a
 * real transport hands the UI a number whatever the declared type is. Going through
 * JSON gives this transport exactly the runtime values of a real one, and keeps this
 * file compiling for either setting of ts-rs. `draft` must have every key of `T`.
 */
function viaWire<T>(draft: Record<keyof T, unknown>): T {
  return JSON.parse(JSON.stringify(draft)) as T;
}

function randomUuidV4(): string {
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join("-");
}
