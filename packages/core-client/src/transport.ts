import type { AiChatInput } from "./generated/AiChatInput";
import type { AiGenerateInput } from "./generated/AiGenerateInput";
import type { AiScope } from "./generated/AiScope";
import type { AiSource } from "./generated/AiSource";
import type { AiStatus } from "./generated/AiStatus";
import type { AiStreamEvent } from "./generated/AiStreamEvent";
import type { AiTranscribed } from "./generated/AiTranscribed";
import type { ApiError } from "./generated/ApiError";
import type { AttachmentId } from "./generated/AttachmentId";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { BackupSummary } from "./generated/BackupSummary";
import type { ByteRange } from "./generated/ByteRange";
import type { DictationApi } from "./dictation";
import type { DriveApi } from "./drive";
import type { BackupFolderListing } from "./generated/BackupFolderListing";
import type { Event } from "./generated/Event";
import type { McpGrant } from "./generated/McpGrant";
import type { McpClientInfo } from "./generated/McpClientInfo";
import type { McpStatus } from "./generated/McpStatus";
import type { ExportFormat } from "./generated/ExportFormat";
import type { ExportWarning } from "./generated/ExportWarning";
import type { ImportReport } from "./generated/ImportReport";
import type { PageId } from "./generated/PageId";
import type { PresenceInfo } from "./generated/PresenceInfo";
import type { PresenceState } from "./generated/PresenceState";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SpaceId } from "./generated/SpaceId";

/**
 * Lifecycle of the core behind a transport, as reported by the desktop shell's
 * `core_state` command (the shell opens the core asynchronously after the window
 * exists, so the UI may start before it is ready):
 *
 * - `"opening"`: keystore, database and migrations are still being set up;
 * - `"ready"`: requests are served;
 * - `"locked"`: the app lock is on and the workspace is closed (at start, or after
 *   `lock()`); every request rejects with `Locked` until `unlock(passphrase)` (ADR-0018);
 * - `{ failed }`: opening failed for good (for example the OS keystore is
 *   unavailable); every request returns that same error until the app restarts.
 */
export type CoreStateInfo = "opening" | "ready" | "locked" | { failed: ApiError };

/** A page, and optionally one of its blocks, that a link of the operating system names (ADR-0040). */
export interface DeepLink {
  /** The page. */
  pageId: PageId;
  /** The block to scroll to, as the page document stores its id. */
  blockId?: string;
}

/** The state of the quick-capture shortcut after the desktop shell registered it (ADR-0040). */
export type ShortcutState =
  /** The setting is empty: no shortcut. */
  | "disabled"
  /** Registered with the operating system and working. */
  | "registered"
  /** The operating system refused it: another program uses it, or it is not allowed. */
  | "taken"
  /** The setting is not an accelerator the shell accepts; nothing is registered. */
  | "invalid";

/**
 * What a choice in the desktop shell's native menu bar asks the page to run (ADR-0040): the `id` of a `menu` message.
 * The menu's new page and lock reach the page as `capture` and `lock`; its quit, minimize and close are the shell's.
 * Undo and redo are sent only where the platform's menu cannot run them itself (not Windows).
 */
export type MenuCommand =
  | "import"
  | "export"
  | "print"
  | "undo"
  | "redo"
  | "find"
  | "toggleSidebar"
  | "back"
  | "forward"
  | "about";

/**
 * What the desktop shell tells the page on its own account (ADR-0040), one message of the channel behind
 * `shell_events_subscribe`:
 *
 * - `deepLink`: the operating system opened an `atomik-kb://page/...` link the shell accepted;
 * - `capture`: the global shortcut, the tray's or the menu bar's "New page" asks for a page at the top level of the space in use;
 * - `lock`: the tray's or the menu bar's "Lock" asks the page to run its lock action;
 * - `shortcut`: the state of the capture shortcut, sent at the subscription and after each registration;
 * - `menu`: an item of the native menu bar that the page runs (see {@link MenuCommand}) was chosen;
 * - `menuBar`: the native menu bar was shown or hidden (a lone Alt, a choice, a loss of focus, a page load);
 * - `inviteLink`: the operating system opened an `atomik-kb://invite?...` invitation link the shell accepted (it carries
 *   the invite's secret: hand it to `CoreClient.inviteLinkOpen` and never log it).
 *
 * A message of another `type` (a newer shell) is dropped by the transport.
 */
export type ShellEvent =
  | { type: "deepLink"; data: DeepLink }
  | { type: "capture" }
  | { type: "lock" }
  | { type: "shortcut"; data: { state: ShortcutState } }
  | { type: "menu"; data: { id: MenuCommand } }
  | { type: "menuBar"; data: { visible: boolean } }
  | { type: "inviteLink"; data: { link: string } };

/**
 * The words of the shell's own surfaces, the tray menu, the native menu bar and the native dialogs (ADR-0040), by
 * label id. Each is optional: a message changes only the labels it names. A label is 1 to 64 characters without
 * control characters; the texts of the dialogs (`dialogAiMessage`, `dialogMcpRead`, ... `dialogImagesOther`) are up
 * to 400, may use the `{placeholders}` named beside them (the shell fills them, once, so a value never reads as
 * another placeholder) and are refused when a placeholder that carries the meaning of a consent (the computer's
 * address, the tool, the spaces) is missing.
 * The labels of the menu bar may carry an `&` before the letter that opens the item with Alt (`&File`; `&&` is a
 * literal `&`); the shell adds the shortcut hints itself.
 */
export interface ShellLabels {
  /** The tray menu's "Open". */
  open?: string;
  /** The tray menu's "New page". */
  newPage?: string;
  /** The tray menu's "Lock". */
  lock?: string;
  /** The tray menu's "Quit". */
  quit?: string;
  /** The menu bar's "File" menu. */
  menuFile?: string;
  /** The menu bar's "Edit" menu. */
  menuEdit?: string;
  /** The menu bar's "View" menu. */
  menuView?: string;
  /** The menu bar's "Window" menu. */
  menuWindow?: string;
  /** The menu bar's "Help" menu. */
  menuHelp?: string;
  /** File: new page. */
  menuNewPage?: string;
  /** File: import. */
  menuImport?: string;
  /** File: export. */
  menuExport?: string;
  /** File: print. */
  menuPrint?: string;
  /** File: lock. */
  menuLock?: string;
  /** File: quit. */
  menuQuit?: string;
  /** Edit: undo. */
  menuUndo?: string;
  /** Edit: redo. */
  menuRedo?: string;
  /** Edit: cut. */
  menuCut?: string;
  /** Edit: copy. */
  menuCopy?: string;
  /** Edit: paste. */
  menuPaste?: string;
  /** Edit: select all. */
  menuSelectAll?: string;
  /** Edit: find in the open page. */
  menuFind?: string;
  /** View: show or hide the sidebar. */
  menuToggleSidebar?: string;
  /** View: back in the active tab's history. */
  menuBack?: string;
  /** View: forward in the active tab's history. */
  menuForward?: string;
  /** Window: minimize. */
  menuMinimize?: string;
  /** Window: close the window (to the tray while the setting says so). */
  menuCloseWindow?: string;
  /** Help: about. */
  menuAbout?: string;
  /** The button that says yes in a consent dialog: "Allow". */
  dialogAllow?: string;
  /** The button that says no in a consent dialog: "Cancel". */
  dialogCancel?: string;
  /** The button that says yes in the image download dialog: "Yes". */
  dialogYes?: string;
  /** The button that says no in the image download dialog: "No". */
  dialogNo?: string;
  /** What a blank name is called in a dialog or in the notification of a reminder: "Untitled". */
  dialogUntitled?: string;
  /** The title of the dialog that asks to send text to another computer (an AI endpoint that is not this machine). */
  dialogAiTitle?: string;
  /** Its text; it must contain `{host}`, which the shell fills with the computer's address. */
  dialogAiMessage?: string;
  /** The title of the dialog that asks to let an AI tool (an MCP client) into spaces. */
  dialogMcpTitle?: string;
  /** Its text when the tool can only read; it must contain `{client}` and `{spaces}`. */
  dialogMcpRead?: string;
  /** Its text when the tool can only change; it must contain `{client}` and `{spaces}`. */
  dialogMcpWrite?: string;
  /** Its text when the tool can read some spaces and change others; it must contain `{client}`, `{read}` and `{write}`. */
  dialogMcpBoth?: string;
  /** How a list of names ends: `{names}` (joined with commas by the shell) and `{last}`; "A, B and C". */
  dialogListAnd?: string;
  /** The last item of a list that is cut: `{count}` more names; "2 more". */
  dialogListMore?: string;
  /** The title of the dialog that asks to download the images an import keeps on a server. */
  dialogImagesTitle?: string;
  /** Its text for exactly one image; it must contain `{stored}` (where it is kept), may use `{count}`. */
  dialogImagesOne?: string;
  /** Its text for several images; it must contain `{stored}` and `{count}`. */
  dialogImagesOther?: string;
  /** Where the images are kept when one other host is involved; `{host}` and `{count}`. */
  dialogHostsOne?: string;
  /** Where the images are kept when several other hosts are involved; `{host}` and `{count}`. */
  dialogHostsOther?: string;
  /** The body of the OS notification of a reply in a thread this account wrote in; it must contain `{actor}` (the person who replied) and may contain `{page}`. */
  dialogNotifyThreadReply?: string;
  /** The body of the OS notification of a mention in a comment; it must contain `{actor}` and may contain `{page}`. */
  dialogNotifyCommentMention?: string;
  /** The body of the OS notification of a mention in a page; it must contain `{actor}` and may contain `{page}`. */
  dialogNotifyPageMention?: string;
  /** The body of the OS notification of being added to a Person property; it must contain `{actor}` and may contain `{page}`. */
  dialogNotifyPersonAssigned?: string;
  /** The title of the dialog that asks to download a speech model for dictation (ADR-0053). */
  dialogDictationTitle?: string;
  /** Its text; it must contain `{size}` and `{host}`, and may use `{model}` (the shell fills them). */
  dialogDictationMessage?: string;
  /** The button that says yes in that dialog: "Download". */
  dialogDownload?: string;
  /** The title of the dialog that asks to send encrypted backups to Google Drive (ADR-0059). */
  dialogDriveTitle?: string;
  /** Its text, up to 400 characters, with no placeholder: what leaves this computer and what Google can see. */
  dialogDriveMessage?: string;
  /** The button that says yes in that dialog: "Connect". */
  dialogDriveAccept?: string;
  /** The button that says no in that dialog: "Cancel". */
  dialogDriveCancel?: string;
  /** What the page the browser lands on after the sign-in says (up to 400 characters, no placeholder): "You can close this tab and go back to atomik-kb." */
  dialogDriveDone?: string;
  /** The file type of a backup in the native file dialogs: "atomik-kb backup". */
  fileFilterBackup?: string;
  /** The file type of a zip archive in the native file dialogs. */
  fileFilterZip?: string;
  /** The file type of a plain text file in the native file dialogs. */
  fileFilterText?: string;
  /** The file type of the files an import accepts: "Markdown, text, HTML and CSV". */
  fileFilterImport?: string;
}

/**
 * How requests reach the core and events come back: Tauri `invoke` and a `Channel`
 * on desktop, a Worker `postMessage` on the web, an in-memory fake in tests.
 *
 * `dispatch` resolves with the `Response` or rejects with a `CoreError` (an `Error`
 * that is also an `ApiError`).
 */
export interface Transport {
  /** Sends one request to the core and resolves with its response. */
  dispatch(request: Request): Promise<Response>;

  /**
   * Registers a listener for events the core emits. Returns a function that removes
   * the listener again. Events are delivered in the order the core emitted them.
   */
  subscribe(onEvent: (event: Event) => void): () => void;

  /**
   * Reports whether the core is still opening, ready or failed to open. Optional:
   * a transport without this method is treated as always ready.
   */
  coreState?(): Promise<CoreStateInfo>;

  /**
   * The binary side of the core API (ADR-0015): page documents travel as raw Loro
   * bytes, never inside JSON. See `docs/development/milestone-03.md`, "Core" and
   * "Shell". Every method rejects with a `CoreError` on failure.
   */
  docs: DocTransport;

  /**
   * Runs one tick of the core's reminder scheduler (ADR-0024): the core emits an
   * `Event.ReminderDue` for every reminder of a date mention that fell due since the last tick
   * and resolves with how many fired. Optional: the desktop shell runs the scheduler of its
   * core itself (a timer, and an OS notification for each event), so only a transport with no
   * shell around its core has this (the in-memory one), and then the UI calls it on a timer.
   */
  remindersTick?(): Promise<number>;

  /**
   * Opens an external `http`, `https` or `mailto` URL in the system browser or mail
   * client. Optional: without it the UI falls back to `window.open` with `noopener`.
   */
  openExternal?(url: string): Promise<void>;

  /**
   * What the desktop shell tells the page on its own account (ADR-0040, see {@link ShellEvent}): the links to a
   * page that the operating system opened the app with (the shell registers the scheme `atomik-kb` and accepts
   * only `atomik-kb://page/<pageId>[#<blockId>]`), the quick capture, the tray's lock and the state of the capture
   * shortcut. A link or a capture that came before the first listener is sent at once, and so is the state of the
   * shortcut. A listener may run while the workspace is still opening or locked: it is the UI's to hold a link
   * or a capture until it can act on it. Returns the function that removes the listener. Optional: a transport
   * with no shell (the in-memory one, the web build) has none, and the UI listens to nothing.
   */
  onShellEvent?(listener: (event: ShellEvent) => void): () => void;

  /**
   * Sets the words of the shell's own surfaces, the tray menu today (ADR-0040): the UI sends its strings as soon
   * as it starts, and again when its language changes. Rejects with `InvalidInput` for the label that is not
   * valid and changes nothing then. Optional: only a desktop shell has such surfaces, which is also how the UI
   * knows it runs in one.
   */
  shellLabelsSet?(labels: ShellLabels): Promise<void>;

  /**
   * Shows the desktop shell's native menu bar (ADR-0040): the UI calls it for a lone Alt press while the bar is
   * hidden. Showing a bar that is shown is fine. The shell tells the page the result as a `menuBar` message.
   * Optional: only a desktop shell with a menu bar has it.
   */
  menuBarShow?(): Promise<void>;

  /** Hides the native menu bar (the UI's Escape, and the first key or click in the page while it is shown). Optional like `menuBarShow`. */
  menuBarHide?(): Promise<void>;

  /**
   * App lock (ADR-0018, `docs/development/milestone-04.md`). `lock` closes the
   * workspace and drops key material; afterwards `coreState()` is `"locked"` and every
   * other call rejects with `Locked`. `unlock` derives the key from the passphrase and
   * reopens the workspace; a wrong passphrase rejects with `InvalidInput` for the field
   * `passphrase`. Optional: a transport without them has no app lock.
   */
  lock?(): Promise<void>;
  unlock?(passphrase: string): Promise<void>;

  /**
   * Encrypted backup file (ADR-0017). `backupExport` writes the whole workspace to
   * `path`, or to a file the shell lets the user pick when `path` is omitted;
   * `backupImport` restores from `path` (or a picked file) into an installation that
   * has no workspace yet, with the recovery kit text unless the backup belongs to this
   * installation's own account. Both resolve with the summary and the file path.
   * Cancelling the picker rejects with `InvalidInput` for the field `path`. Optional:
   * the web shell has no files (yet).
   */
  backupExport?(path?: string): Promise<BackupResult>;
  backupImport?(path?: string, kit?: string): Promise<BackupResult>;

  /**
   * Import and export (ADR-0036, `docs/architecture/import-export.md`). The shell runs the core's
   * export session (`ExportBegin`, `ExportNext`, `ExportEnd`), asks where to save (a `.md` or
   * `.html` file for a single page without attachments, else a `.zip`) and writes the archive,
   * streaming attachments. Progress arrives as `Event.ExportProgress`. Cancelling the dialog
   * rejects with `InvalidInput` for the field `path`. Optional: the web shell has no files (yet).
   */
  exportRun?(options: ExportRunOptions): Promise<ExportRunResult>;

  /**
   * Lets the person pick the source (files, a folder or a `.zip`), runs the core's import session
   * (`ImportBegin` to `ImportCommit`), uploading the files the pages need as attachments, and
   * resolves with the report. Progress arrives as `Event.ImportProgress`. Cancelling the dialog
   * rejects with `InvalidInput` for the field `path`. Optional.
   */
  importRun?(options: ImportRunOptions): Promise<ImportReport>;

  /**
   * Asks where to save a text file (the CSV of a view) and writes it. `suggestedName` carries the
   * extension (`Tasks.csv`). Resolves with the path written; cancelling rejects with
   * `InvalidInput` for the field `path`. Optional.
   */
  saveTextFile?(suggestedName: string, text: string): Promise<string>;

  /**
   * Scheduled backups (ADR-0036): `backupFolderChoose` opens the folder dialog and stores the
   * chosen folder in the shell-owned setting `app.backup.folder` (resolves with it; cancelling
   * rejects with `InvalidInput` for `path`); `backupRunNow` writes a scheduled backup into that
   * folder at once, prunes old ones and records `app.backup.last`. The shell's own timer runs
   * the schedule. Optional: only the desktop shell has them.
   */
  backupFolderChoose?(): Promise<string>;
  backupRunNow?(): Promise<BackupResult>;
  /**
   * The backups of a folder for the restore wizard (ADR-0059): without `path`, the shell's folder dialog first (cancelling
   * rejects with `InvalidInput` for `path`); the `.atomikkb` files of that folder, newest first, at most 200, each `id` the
   * absolute path `backupImport` takes. Optional: only the desktop shell (and the in-memory fake) has it.
   */
  backupFolderList?(path?: string): Promise<BackupFolderListing>;

  /**
   * The binary side of attachments (ADR-0020, `docs/development/milestone-05.md`): bytes
   * travel as raw IPC bodies, never inside JSON, and files on disk are read and written by
   * the shell. Optional: without it the dispatch-only attachment requests still work, but
   * nothing can be uploaded or read. Every method rejects with a `CoreError`.
   */
  attachments?: AttachmentTransport;

  /**
   * Local AI through Ollama (ADR-0037, `docs/architecture/ai.md` sections 6 and 8): the shell does
   * every HTTP call. Optional: without it the UI hides every AI feature.
   */
  ai?: AiTransport;

  /**
   * The local MCP server (ADR-0037, section 7): its status and the grants change that needs a
   * native confirmation. Clients and the audit log are core requests. Optional.
   */
  mcp?: McpTransport;

  /**
   * Presence in shared spaces (M13 slice 3, ADR-0050): this device's presence goes out and other members'
   * arrives as binary frames the core seals and opens, so the webview never holds a key. The desktop shell
   * has it (`presence_send` and the channel of `presence_events`); the in-memory mirror simulates other
   * people. Optional: without it the UI shows no cursors and no avatars.
   */
  presence?: PresenceTransport;

  /**
   * Local dictation (ADR-0053, `docs/architecture/dictation.md` section 5): the desktop shell recognises speech on this
   * device (a sidecar process, models downloaded after a consent) and the in-memory mirror pretends to. Optional:
   * without it the UI hides `/microphone` and the Settings section.
   */
  dictation?: DictationApi;

  /**
   * Google Drive as a backup target and a restore source (ADR-0059, `docs/architecture/drive-backup.md`): the desktop shell
   * signs in, uploads and downloads; the in-memory mirror fakes it (`?drive=fake`). Optional: without it the UI hides the
   * Drive target and the wizard's Drive source.
   */
  drive?: DriveApi;
}

/**
 * One accepted presence frame as the editor gets it (M13 slice 3): who sent it and where (`info`: the name and
 * the colour index come from membership, never from the frame) and the sender's ephemeral-store update, the Loro
 * cursors of anchor and head (the generated `PresenceMessage` carries `update` as an array of numbers; the
 * transport turns it into bytes).
 */
export interface PresenceUpdate {
  /** Who and where. */
  info: PresenceInfo;
  /** The sender's `EphemeralStore` update, at most `PRESENCE_UPDATE_MAX_BYTES` bytes. */
  update: Uint8Array;
}

/**
 * The presence frames of the core API (ADR-0050): not requests, because the bytes are sealed and signed by the core
 * and relayed by the server without being read. Every method rejects with a `CoreError`.
 */
export interface PresenceTransport {
  /**
   * Sends this device's presence in a page of a shared space: the page (`undefined` for "in no page", an idle
   * window), what the device is doing and the editor's ephemeral-store update (at most `PRESENCE_UPDATE_MAX_BYTES`:
   * `InvalidInput` for `update`). The core seals it under the space key and the shell hands it to the socket;
   * nothing is sent for a personal space, an archive or a device that is offline, and that is not an error.
   * Presence is best effort and nothing waits for it. The UI sends at most `PRESENCE_UPDATES_PER_SECOND` updates a
   * second. Every member's device may send, Viewers included.
   */
  send(spaceId: SpaceId, pageId: PageId | undefined, state: PresenceState, update: Uint8Array): Promise<void>;
  /**
   * Registers a listener for the frames other members' devices sent (accepted by the core: right signature, a
   * current member, the current key epoch, a fresh counter). Returns the function that removes the listener.
   * Frames arrive as they were accepted; a device that goes quiet sends nothing more (the UI times it out after
   * `PRESENCE_TIMEOUT_SECONDS`, and `Event.PresenceChanged` / `CoreClient.presenceList` say who is where).
   */
  subscribe(listener: (message: PresenceUpdate) => void): () => void;
}

/**
 * The AI commands of the shell (ADR-0037). Every method rejects with a `CoreError`; `generate` and
 * `chat` resolve when the stream ended (its last event, `done` or `error`, was delivered first).
 */
export interface AiTransport {
  /** Endpoint, reachability, models, index and indexer state (works with AI off). */
  status(): Promise<AiStatus>;
  /**
   * Stores the Ollama endpoint; a remote host asks for consent in a native dialog (declining
   * rejects with `InvalidInput` for `url`, `details.reason = "declined"`).
   */
  endpointSet(url: string): Promise<AiStatus>;
  /** Streams a writing action; `requestId` (1 to 64 of `[A-Za-z0-9_-]`) names it for `cancel`. */
  generate(
    requestId: string,
    input: AiGenerateInput,
    onEvent: (event: AiStreamEvent) => void,
  ): Promise<void>;
  /** Streams a chat answer over retrieved sources (`sources` first). */
  chat(requestId: string, input: AiChatInput, onEvent: (event: AiStreamEvent) => void): Promise<void>;
  /** Cancels a stream (its last event is `error` with reason `cancelled`); unknown ids are fine. */
  cancel(requestId: string): Promise<void>;
  /** Related passages for a query (semantic when the index is on, else keyword). */
  search(query: string, scope: AiScope, limit?: number): Promise<AiSource[]>;
  /** Transcribes one 16 kHz mono PCM WAV segment (at most 4 MiB). */
  transcribe(wav: Uint8Array, model?: string): Promise<AiTranscribed>;
}

/** The MCP commands of the shell (ADR-0037). */
export interface McpTransport {
  /** Whether the loopback server runs, its URL and the bridge's path. */
  status(): Promise<McpStatus>;
  /**
   * Replaces a client's grants; adding access asks for confirmation in a native dialog (declining
   * rejects with `InvalidInput` for `grants`, `details.reason = "declined"`).
   */
  grantsSet(clientId: string, grants: McpGrant[]): Promise<McpClientInfo>;
}

/** What to export (ADR-0036). */
export interface ExportRunOptions {
  /** Space. */
  spaceId: SpaceId;
  /** Root page; omitted for the whole space. */
  pageId?: PageId;
  /** Include the root page's descendants (ignored without `pageId`). */
  includeChildren: boolean;
  /** Markdown or HTML. */
  format: ExportFormat;
}

/** What an export wrote. */
export interface ExportRunResult {
  /** The file written (`.md`, `.html` or `.zip`). */
  path: string;
  /** Pages written. */
  pages: number;
  /** Attachment files written. */
  attachments: number;
  /** What was left out (attachments not on this device, unreadable ones). */
  warnings: ExportWarning[];
}

/** Where an import reads from: some files, a folder, or a `.zip` (a Notion or atomik-kb export). */
export type ImportSourceKind = "files" | "folder" | "zip";

/** What to import, and where (ADR-0036). */
export interface ImportRunOptions {
  /**
   * Where the pages go (ADR-0039): `"page"` (the default) into `spaceId` under `parentId`;
   * `"newSpaces"` into one new space per space of the source (`ImportBeginSpaces`; `spaceId` and
   * `parentId` are ignored).
   */
  destination?: "page" | "newSpaces";
  /** Destination space (ignored for `"newSpaces"`). */
  spaceId: SpaceId;
  /** Destination page; omitted for the space root. */
  parentId?: PageId;
  /** What the dialog lets the person pick. */
  source: ImportSourceKind;
}

/** What a backup export or import did, with the file it used. */
export interface BackupResult {
  /** The core's summary. */
  summary: BackupSummary;
  /** The file written or read. */
  path: string;
}

/** Raw-bytes operations on one page's content document. */
export interface DocTransport {
  /**
   * Opens the page's document in the core (creating it with one empty paragraph when
   * the page has none yet) and resolves with the raw Loro snapshot to import into
   * the editor's own `LoroDoc`. Rejects with `NotFound` for unknown or purged pages.
   */
  open(pageId: PageId): Promise<Uint8Array>;
  /** Sends raw Loro update bytes from the editor replica; resolves once persisted and indexed. */
  import(pageId: PageId, update: Uint8Array): Promise<void>;
  /**
   * Resolves with a raw Loro update holding everything the core has that the given
   * encoded version vector (`doc.oplogVersion().encode()`) lacks; everything when
   * `since` is omitted; an empty array when there is nothing new.
   */
  export(pageId: PageId, since?: Uint8Array): Promise<Uint8Array>;
  /** Drops the core's in-memory replica of the page (its content stays persisted). */
  close(pageId: PageId): Promise<void>;
}

/** What saving an attachment to a file did: the attachment and the file written. */
export interface AttachmentSaveResult {
  /** The attachment saved. */
  info: AttachmentInfo;
  /** The file written. */
  path: string;
}

/**
 * Raw-bytes and file operations on attachments. An upload is `AttachmentBegin` (through
 * `dispatch`), then {@link AttachmentTransport.write} in order, then `AttachmentFinish` (or
 * `AttachmentAbort`); `CoreClient.attachmentUpload` does all of it.
 */
export interface AttachmentTransport {
  /**
   * Appends plaintext to an upload in progress; the core stores every chunk as it completes.
   * A piece is 1 byte to `CHUNK_SIZE` (the shell refuses others: `InvalidInput` for `body`).
   * `NotFound` for an unknown upload, `InvalidInput` for the field `size` (with
   * `details.limit`) past `MAX_ATTACHMENT_BYTES`.
   */
  write(attachmentId: AttachmentId, bytes: Uint8Array): Promise<void>;
  /**
   * The plaintext of an attachment, whole or the half-open byte `range`. One answer is at most
   * `MAX_PREVIEW_BYTES`: a larger attachment without a range is `InvalidInput` for `size`, a
   * longer range `InvalidInput` for `range` (both with `details.limit`); an empty or inverted
   * range, or one past the end, is `InvalidInput` for `range`. `NotFound` for an unknown or
   * purged attachment, `Storage` for an attachment not complete on this device ("the attachment
   * is not complete on this device (n of m chunks)", Milestone 8; `saveAs` and `open` too) and
   * for a missing chunk, `Crypto` for a damaged one.
   */
  read(attachmentId: AttachmentId, range?: ByteRange): Promise<Uint8Array>;
  /**
   * A URL a `<video>` or `<audio>` element can stream the attachment from, or `null` when this
   * transport has none (ADR-0032). Optional: only the desktop shell has one (its `kbmedia` URI
   * scheme answers Range requests by decrypting the chunks they need, whatever the size); the UI
   * then falls back to a blob made from {@link AttachmentTransport.read}. The URL carries the
   * attachment id and nothing else, and works only while the core is unlocked.
   */
  mediaUrl?(attachmentId: AttachmentId): string | null;
  /**
   * Uploads the file at `path` (or one the user picks in the shell's open dialog) into the
   * space, named after the file, with a MIME type guessed from its extension; a file above
   * `MAX_ATTACHMENT_BYTES` is `InvalidInput` for `size`. Cancelling the dialog rejects with
   * `InvalidInput` for the field `path` (release builds refuse explicit paths the same way:
   * only the dialog chooses there).
   */
  importPath(spaceId: SpaceId, path?: string): Promise<AttachmentInfo>;
  /**
   * Downloads the picture the `https` link `url` points to, once, and stores it as an attachment
   * of the space (ADR-0028): the shell fetches it (no cookies, no `Referer`, at most 5 redirects,
   * 60 s, at most `MAX_PREVIEW_BYTES`), takes the type from the bytes (PNG, JPEG, GIF, WebP, BMP,
   * AVIF, SVG) and names it after the last segment of the link's path. Optional: only a transport
   * with a shell that can reach the network has it (the web build will not), and the UI then
   * leaves "Image from URL" out (`CoreClient.canImportUrl`). Rejects with `InvalidInput` for the
   * field `url` and `details.reason` `status` (with `details.status`), `network`, `timeout`,
   * `redirect` or `notImage`, or for the field `size` (with `details.limit`) when the picture is
   * too large; a link that is not `https`, has credentials or is longer than 2048 characters is
   * `InvalidInput` for `url` without a reason. Messages never repeat the link.
   */
  importUrl?(spaceId: SpaceId, url: string): Promise<AttachmentInfo>;
  /**
   * Writes the attachment's plaintext to `path` (or to a file the user picks in the shell's
   * save dialog, which suggests the attachment's name made safe for a path). Cancelling
   * rejects with `InvalidInput` for the field `path`.
   */
  saveAs(attachmentId: AttachmentId, path?: string): Promise<AttachmentSaveResult>;
  /**
   * Writes a private temporary copy and hands it to the system's default application; resolves
   * with its path. Files the system would run (`.exe`, `.bat`, ...) are `InvalidInput` for
   * `attachment`.
   */
  open(attachmentId: AttachmentId): Promise<string>;
}
