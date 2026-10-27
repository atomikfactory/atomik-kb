//! atomik-kb desktop shell: a thin Tauri adapter over `kb-core` (ADR-0015).
//!
//! The shell owns exactly one [`kb_core::Core`] behind a mutex and exposes it to the
//! webview through a handful of commands. It contains no business logic: it opens the
//! core, passes requests and page-document bytes in, forwards the events the core emits,
//! and does the file and dialog work the sans-IO core cannot (backups, attachments).
//!
//! IPC contract (what `@atomik-kb/core-client`'s `TauriTransport` calls):
//!
//! | command            | arguments                                      | result                                             |
//! |--------------------|------------------------------------------------|----------------------------------------------------|
//! | `dispatch`         | `{ request: Request }`                         | `Response`, or rejects with the `ApiError` JSON    |
//! | `subscribe_events` | `{ onEvent: Channel }`                         | nothing; every `Event` is then sent to the channel |
//! | `core_state`       | none                                           | `"opening"`, `"ready"`, `"locked"` or `{ "failed": ApiError }` |
//! | `doc_open`         | header `x-kb-page`; no body                    | raw bytes: the page's Loro snapshot                |
//! | `doc_import`       | header `x-kb-page`; body: the update           | empty                                              |
//! | `doc_export`       | header `x-kb-page`; body: version vector or empty | raw bytes: the update, empty when nothing is new |
//! | `doc_close`        | header `x-kb-page`; no body                    | empty                                              |
//! | `open_external`    | `{ url: string }`                              | nothing; only `http`, `https`, `mailto` links      |
//! | `lock`             | none                                           | nothing; `Ready` becomes `Locked`                  |
//! | `unlock`           | `{ passphrase: string }`                       | nothing; `Locked` becomes `Ready`                  |
//! | `backup_export`    | `{ path?: string }`                            | `{ summary: BackupSummary, path: string }`         |
//! | `backup_import`    | `{ path?: string, kit?: string }`              | `{ summary: BackupSummary, path: string }`         |
//! | `attachment_write` | header `x-kb-attachment`; body: one plaintext piece, 1 byte to 4 MiB | empty                        |
//! | `attachment_read`  | header `x-kb-attachment`, optional `x-kb-range-start` + `x-kb-range-end`; no body | raw bytes: the plaintext |
//! | `attachment_import_path` | `{ spaceId: string, path?: string }`     | `AttachmentInfo`                                   |
//! | `attachment_import_url` | `{ spaceId: string, url: string }`        | `AttachmentInfo`                                   |
//! | `attachment_save_as` | `{ attachmentId: string, path?: string }`    | `{ info: AttachmentInfo, path: string }`           |
//! | `attachment_open`  | `{ attachmentId: string }`                     | the path of the private copy that was opened (string) |
//! | `export_run`       | `{ options: { spaceId, pageId?, includeChildren, format } }` | `{ path, pages, attachments, warnings }` |
//! | `import_run`       | `{ options: { destination?: "page" \| "newSpaces", spaceId?, parentId?, source: "files" \| "folder" \| "zip" } }` | `ImportReport` |
//! | `save_text_file`   | `{ suggestedName: string, text: string }`      | the path written (string)                          |
//! | `backup_folder_choose` | none                                       | the folder stored in `app.backup.folder` (string)  |
//! | `backup_run_now`   | none                                           | `{ summary: BackupSummary, path: string }`         |
//! | `ai_status`        | none                                           | `AiStatus`                                         |
//! | `ai_endpoint_set`  | `{ url: string }`                              | `AiStatus`                                         |
//! | `ai_generate`      | `{ requestId: string, input: AiGenerateInput, onEvent: Channel }` | nothing; `AiStreamEvent`s go to the channel |
//! | `ai_chat`          | `{ requestId: string, input: AiChatInput, onEvent: Channel }` | nothing; `AiStreamEvent`s go to the channel |
//! | `ai_cancel`        | `{ requestId: string }`                        | nothing                                            |
//! | `ai_search`        | `{ query: string, scope: AiScope, limit?: number }` | `AiSource[]`                                  |
//! | `ai_transcribe`    | header `x-kb-ai` (JSON `{ model?: string }`); body: a 16 kHz mono PCM WAV, at most 4 MiB | `AiTranscribed` |
//! | `mcp_status`       | none                                           | `McpStatus`                                        |
//! | `mcp_client_grants_set` | `{ clientId: string, grants: McpGrant[] }` | `McpClientInfo`                                   |
//! | `shell_events_subscribe` | `{ onEvent: Channel }`                    | nothing; what the shell tells the page on its own account is then sent to the channel as `{ type, data? }`: `deepLink` (`{ pageId, blockId? }`), `capture`, `lock`, `shortcut` (`{ state }`), `menu` (`{ id }`), `menuBar` (`{ visible }`) |
//! | `shell_labels_set` | `{ labels: { open?, newPage?, lock?, quit?, menuFile?, dialogAiTitle?, fileFilterZip?, ... } }` | nothing; changes the words of the tray menu, the menu bar, the native dialogs and the file types of the file dialogs |
//! | `menu_bar_show` | none                                           | nothing; shows the menu bar (it may already be shown) |
//! | `menu_bar_hide`  | none                                           | nothing; hides the menu bar (it may already be hidden) |
//! | `presence_send` | `{ spaceId: string, pageId?: string, state: "Viewing" \| "Editing" \| "Idle", update: number[] }` | nothing; seals this device's presence and sends it, best effort (M13 slice 3, see `presence`) |
//! | `presence_events` | `{ onEvent: Channel }`                       | nothing; every accepted presence frame is then sent to the channel as a `PresenceMessage` (`{ info, update: number[] }`) |
//! | `dictation_status` | none                                          | `DictationStatus` (ADR-0053: the engine chosen on this device and each engine's state) |
//! | `dictation_select` | `{ engine: SpeechEngine }`                    | `DictationStatus`; stores the choice of this device           |
//! | `dictation_install` | `{ engine: SpeechEngine, onEvent: Channel }` | nothing; a native consent dialog, then `InstallEvent`s go to the channel |
//! | `dictation_install_cancel` | none                                  | nothing; cancels the installation that runs                   |
//! | `dictation_import` | `{ engine: SpeechEngine }`                    | `DictationStatus`; a native folder dialog, then the model files are checked and copied |
//! | `dictation_remove` | `{ engine: SpeechEngine }`                    | `DictationStatus`; deletes the engine's model files           |
//! | `dictation_start`  | `{ sessionId: string, engine: SpeechEngine, onEvent: Channel }` | nothing; `DictationEvent`s go to the channel |
//! | `dictation_audio`  | header `x-kb-dictation` (the session id); body: 16 kHz mono 16-bit little-endian samples, at most 2 s | nothing       |
//! | `dictation_stop`   | `{ sessionId: string }`                       | nothing; flushes the last phrases, then `ended`               |
//! | `dictation_cancel` | `{ sessionId: string }`                       | nothing; ends the session now                                 |
//! | `drive_status`     | none                                           | `DriveStatus` (ADR-0059: the client in use, the connection, the last upload; no network) |
//! | `drive_connect`    | none                                           | `DriveStatus`; a native notice, the sign-in in the system browser, the "atomik-kb backups" folder found or made |
//! | `drive_disconnect` | none                                           | `DriveStatus`; revokes the token, forgets it; the files stay in Drive |
//! | `drive_client_set` | `{ client: { clientId, clientSecret? } \| null }` | `DriveStatus`; one's own Google client (`null`: the built-in one); disconnects |
//! | `drive_backups_list` | none                                         | `BackupFileInfo[]`: the `.atomikkb` files of the Drive folder, newest first, at most 200 |
//! | `drive_backup_download` | `{ fileId: string, onEvent: Channel }`    | the absolute path of the staging file holding the backup (string); `DriveProgress` goes to the channel |
//! | `drive_cancel`     | none                                           | nothing; ends a sign-in that waits for the browser or a download that runs |
//! | `backup_folder_list` | `{ path?: string }`                          | `BackupFolderListing`; a native folder dialog without `path`, then the `.atomikkb` files of the folder, newest first |
//!
//! `Request`, `Response`, `Event` and `ApiError` are the `kb-types` enums, adjacently
//! tagged JSON (`{ "type": ..., "data": ... }`). Every failure rejects with the `ApiError`
//! JSON. The core opens on a background thread after the window exists; until then
//! `dispatch`, the `doc_*` and the attachment commands answer `Internal: "core is starting"`
//! and the UI polls `core_state`.
//!
//! **App lock** (ADR-0018). When the app lock is on, the core opens as `Locked` and holds
//! no key material and no database; `core_state` says `"locked"`. While locked, `dispatch`,
//! the `doc_*`, attachment and backup commands answer `ErrorCode::Locked`; `lock`,
//! `unlock`, `core_state`, `subscribe_events` and `open_external` do not need the core and
//! still work. `unlock { passphrase }` takes about a second (Argon2id, on a blocking
//! thread): a wrong passphrase is `InvalidInput` for the field `passphrase` and the state
//! stays locked, and calling it on a workspace that is not locked is `InvalidInput`.
//! `lock` closes the workspace (the events the core had queued are delivered first) and
//! needs the app lock to be enabled, else the core refuses with `InvalidInput`; locking a
//! locked workspace succeeds. The passphrase and the recovery kit text are
//! `Zeroizing<String>` from the moment they are deserialised, are never logged, and are
//! handed to the core by reference.
//!
//! **Backups** (ADR-0017). `backup_export` and `backup_import` stream the core's encrypted
//! backup between the core and a file, on a blocking thread. Without `path` the shell opens
//! the native save (or open) dialog from Rust with `tauri-plugin-dialog`, filtered on
//! `.atomikkb`, the save dialog proposing `atomik-kb-<yyyy-mm-dd>.atomikkb`; cancelling is
//! `InvalidInput` for the field `path`, as is a `path` that is not absolute or cannot be
//! created, opened or moved. The export goes to `<path>.tmp` and is renamed into place
//! after `sync_all`, the temporary file being removed on any failure. `kit` is the recovery
//! kit text, needed unless the backup belongs to this installation's own account. Both
//! answer `Locked` (and open no dialog) while the workspace is locked.
//!
//! The `doc_*` commands are raw IPC (`tauri::ipc::Request` in, `tauri::ipc::Response`
//! out): binary payloads never travel inside JSON. The page id is the UUID text in the
//! `x-kb-page` header (missing or malformed is `InvalidInput`); the body is the
//! `Uint8Array` the call sends and the result is an `ArrayBuffer`. "No body" is an empty
//! `Uint8Array`: `doc_open` and `doc_close` ignore the body, `doc_import` refuses an empty
//! one (`InvalidInput`) and `doc_export` reads it as "everything". The calls run under the
//! same single-writer lock as `dispatch`, and the events the core emitted
//! (`PageContentChanged`, `BacklinksChanged`) are published once the call is done.
//!
//! **Attachments** (ADR-0020). The bytes of an attachment travel like those of a page
//! document: `attachment_write` and `attachment_read` are raw IPC. The attachment id is the
//! UUID text in the `x-kb-attachment` header (missing or malformed is `InvalidInput`).
//! `attachment_write` takes one plaintext piece of 1 byte to `CHUNK_SIZE` (4 MiB) as its
//! body (empty or larger is `InvalidInput` for the field `body`) and answers empty; the
//! upload itself is started and finished with `dispatch` (`AttachmentBegin`,
//! `AttachmentFinish`, `AttachmentAbort`), and the lock is released between pieces.
//! `attachment_read` answers the plaintext as an `ArrayBuffer`: the whole attachment (one
//! larger than `MAX_PREVIEW_BYTES`, 32 MiB, is `InvalidInput` for the field `size` with
//! `details.limit`), or, with both `x-kb-range-start` and `x-kb-range-end` (decimal bytes,
//! half-open, at most `MAX_PREVIEW_BYTES` long), just that range. The three JSON commands
//! do the file and dialog work: `attachment_import_path` streams a file into a new
//! attachment (the native open dialog, any file, without `path`; the name comes from the
//! file name and the MIME type from its extension; any failure aborts the upload),
//! `attachment_save_as` streams an attachment into a file the way `backup_export` does (the
//! save dialog proposes the sanitised name; `<path>.tmp`, renamed into place) and
//! `attachment_open` writes a private plaintext copy under
//! `<app cache dir>/open/<attachment id>/<sanitised name>` and hands it to the system
//! opener (the `open` directory is removed at start; files the system would run, such as
//! `.exe` or `.bat`, are refused with `InvalidInput` for the field `attachment`). The
//! `path` argument of `attachment_import_path` and `attachment_save_as` is honoured in debug
//! builds only (dev app, smoke scripts, tests), where it must be absolute and, on Windows,
//! on a drive; a release build answers any given `path` with `InvalidInput` for the field
//! `path` ("choose the file in the dialog") and touches nothing, because an explicit path
//! comes from the page and a compromised page could read or overwrite any file the user can.
//! No command ever uses a
//! stored name as a path unsanitised, and none logs a name, a MIME type or a path. All of
//! them answer `Locked` (and open no dialog, create no file) while the workspace is locked,
//! and `Internal: "core is starting"` while it opens.
//!
//! **Images from a link** (ADR-0028). `attachment_import_url` downloads the picture a link
//! points to, once, on the person's explicit action, and imports it like a picked file (the
//! same `AttachmentBegin`, `attachment_write` and `AttachmentFinish`, aborted on any failure),
//! so the page never holds the link and the webview never loads a remote image (its CSP does
//! not change). See `url_import`: `https` only, no credentials, at most 2048 characters; one
//! `GET` with no cookies and no `Referer`; at most 5 redirects, each `https`; 10 s to connect and
//! 60 s in all; at most `MAX_PREVIEW_BYTES` (32 MiB); the type is sniffed from the bytes (PNG,
//! JPEG, GIF, WebP, BMP, AVIF, and SVG only when the server says so and the body is an SVG
//! document); the name comes from the last segment of the link's path. Every failure is
//! `InvalidInput` for the field `url` (`size` with `details.limit` for a download that is too
//! large) with `details.reason` `status` (and `details.status`), `network`, `timeout`,
//! `redirect` or `notImage`; neither the link nor its host is ever in a message or a log line.
//! It answers `Locked`, or `Internal: "core is starting"`, before any request is made.
//!
//! **Import and export** (ADR-0036, `docs/architecture/import-export.md`). The converters live in
//! the core; the shell does the dialogs and the files, and the webview never passes a path.
//! `export_run` asks the core to plan an export (`ExportBegin`), shows the native save dialog (a
//! `.md` or `.html` file for one page and nothing else, else a `.zip`; cancelling ends the core's
//! session and is `InvalidInput` for `path`), then loops `ExportNext` and writes the entries
//! (`exporter`): text deflated, attachments stored and streamed from `attachment_read` one chunk
//! at a time with the core lock released between pieces, everything to `<path>.tmp` and renamed
//! into place. `import_run` shows the dialog its `source` names (files, a folder or a `.zip`), reads
//! the source (`import_source`: zip slip refused, one nested zip level unpacked to a temporary
//! file in the app cache folder, text decoded, limits), runs the core's import session and uploads
//! the files its pages need as attachments (`importer`), and answers the core's report with the
//! shell's own warnings appended. With `destination: "newSpaces"` (ADR-0039) the pages go into one new
//! space per space of the source (`ImportBeginSpaces`; an AppFlowy "Back up your workspace" archive,
//! recognised by `relation_map.json` and `metadata.json` at its root, brings its own: every `.json`
//! file of it is read as text and `collab_objects/` is skipped), each needed file is uploaded into the
//! space the plan names, and when the plan lists images the source keeps on a server a native Yes/No
//! dialog (owned by the main window) asks once before they are downloaded, at most four at a time,
//! with the rules of "Image from URL". `save_text_file` saves a text (the CSV of a view, at most 64 MiB)
//! through the save dialog (`text_file`). `backup_folder_choose` stores the folder of scheduled
//! backups in `app.backup.folder` and `backup_run_now` writes one into it at once; a timer started
//! with the sync driver (`backup_scheduler`) writes them when the core says one is due. The
//! `dispatch` command refuses a `SettingsSet` of `app.backup.folder` and `app.backup.last` from the
//! webview (`InvalidInput` for the field `key`): only the shell writes them. All five commands
//! answer `Locked` (and open no dialog) while the workspace is locked.
//!
//! **Local AI and MCP** (ADR-0037, `docs/architecture/ai.md`). The shell makes every call to Ollama
//! (`ai_client`: reqwest without a proxy; the webview's CSP still allows no network) and streams the
//! tokens back over a `tauri::ipc::Channel` (`ai_host`). `ai_status` and `ai_endpoint_set` work with AI
//! off; every other `ai_*` command answers `InvalidInput` for `enabled` while `app.ai.settings` says
//! off, and `InvalidInput` for `endpoint` ("consent required") for a remote endpoint the person did not
//! agree to. `ai_endpoint_set` stores `app.ai.endpoint` (a setting only the shell writes) after a native
//! dialog, made from Rust, when the host is not this machine. `ai_generate` and `ai_chat` take a
//! `requestId` and a channel: they reject only when nothing started, and otherwise report `sources`,
//! `delta`, `done` or `error` as events (at most four at once; `ai_cancel` and a lock stop them).
//! `ai_search` answers related passages (semantic when the question embeds in time, else full text) and
//! `ai_transcribe` takes a WAV segment as its raw body and answers its text. The semantic index is kept
//! by a background task (`ai_indexer`) that runs while the core is ready, asks the core for stale pages
//! (`AiIndexNext`), has Ollama embed them and stores the vectors (`AiIndexStore`); the core lock is never
//! held while Ollama is used. The MCP server (`mcp_host`) is another background task: it listens on
//! `127.0.0.1` only while the core is ready and `app.mcp.settings` is on, checks `Host`, `Origin`, the
//! size, the content type and the bearer token, and hands each message to the core (`McpCall`).
//! `mcp_status` reports it; `mcp_client_grants_set` asks the person in a native dialog before it gives
//! an MCP client more access. `dispatch` refuses `AiIndexNext`, `AiIndexStore`, `McpCall` and
//! `McpClientGrantsSet` from the webview (`InvalidInput` for the field `type`) and a `SettingsSet` of
//! `app.ai.endpoint` (field `key`).
//!
//! **Local dictation** (ADR-0053, `docs/architecture/dictation.md`). `/microphone` dictates speech into the editor,
//! phrase by phrase. The recognizer runs in the `kb-speech` sidecar, a process the shell starts for one session
//! (`speech_host`): the webview pushes 16 kHz audio with `dictation_audio`, the shell frames it to the sidecar
//! (`kb_speech::protocol`) and sends each recognised phrase back as a `DictationEvent` on the session's channel. The
//! engines are Accurate (Parakeet), Light (Moonshine) and Ollama (`ai_host::transcribe`); the choice of this device
//! lives in a file of the speech store. A model is downloaded after a native consent dialog, checked against
//! its SHA-256 and kept in `<app data>/speech`. Audio is never stored and no transcript text is logged. One session
//! runs at a time and a lock ends it. Every `dictation_*` command but `dictation_stop` and `dictation_cancel`
//! answers `Locked` while the workspace is locked.
//!
//! **Google Drive backups and the restore wizard** (ADR-0059, `docs/architecture/drive-backup.md`, `drive_host`,
//! `backup_listing`). Drive is a second target of the scheduled backups: the same encrypted `.atomikkb` file,
//! uploaded to a folder "atomik-kb backups" at the root of the person's Drive (scope `drive.file`: the app sees only
//! what it made). `kb-drive` is the protocol as values; the shell carries its requests over `reqwest` (`https`
//! only, the system proxy), opens the sign-in in the system browser with PKCE and a loopback redirect on
//! `127.0.0.1` (a random port, `Host` checked, five minutes at most), keeps the refresh token in the OS keystore
//! (`gdrive:<install id>`; an own client's secret in `gdrive-client:<install id>`) and the access token in memory,
//! and writes `app.backup.drive` (a setting only the shell writes, device-bound: never in a backup). The built-in
//! client is read when the shell is compiled from `KB_GOOGLE_CLIENT_ID` and `KB_GOOGLE_CLIENT_SECRET`; without it
//! `drive_connect` is `InvalidInput` with the reason `noClient`. `drive_connect` shows a native notice first
//! (what leaves this computer and what Google can see; the `dialogDrive*` labels; declined is `consentDeclined`).
//! Every failure is `InvalidInput` for the field `drive` with `details.reason` one of `consentDeclined`,
//! `cancelled`, `noClient`, `browserTimeout`, `denied`, `reconnect` (it also sets `needsReconnect`), `quota`,
//! `notConnected`, `notFound`, `network` and `server`; no log line holds a token, a code, an id, a name or an
//! address. `drive_backup_download` writes `<app data>/restore-staging/<random>.atomikkb` (emptied first), which
//! `backup_import` takes as an absolute path, with `DriveProgress` on the channel at most every 250 ms.
//! `backup_folder_list` lists the `.atomikkb` files of a folder for the wizard (a cancelled folder dialog is
//! `InvalidInput` for `path`, "no file was chosen"; an unreadable folder is another message). All of them but
//! `drive_status` and `drive_cancel` answer `Locked` (and open no dialog) while the workspace is locked, and "core is
//! starting" while it opens; none needs a workspace, because the wizard runs during onboarding.
//!
//! **Sync** (ADR-0021, ADR-0045). The shell hosts `kb-net`'s v2 `SyncDriver` (`sync_host`): it starts once
//! the core is `Ready` (opened, or unlocked) and is dropped, which stops it, when the core
//! locks; the state ([`AppState`]) is its `kb_net::v2::SyncHost`, every call taking the single-writer lock
//! like `dispatch`, and the driver hears the core's events through a Rust listener next to the
//! webview channels. One session per device serves every feed: the personal one and one per shared space (the
//! driver works them in the order of ADR-0045 section 9, each with its own transfer budget). The remote is
//! `HttpRemote` for the configured server. The same state is the `InviteHost` of `kb-net`'s `InviteTasks`,
//! which starts the invite task (both sides of the code flow, the invitee of the link flow) on the core's
//! `InviteRequested` event, and the host of the device tasks. Nothing is added to the IPC surface:
//! `SyncConfigure`, `SyncStatus`, `SyncNow` and the `Invite*` requests are ordinary `dispatch` requests,
//! and the driver and the tasks report through the core (`SyncChanged`, `InviteChanged`).
//!
//! **Reminders** (ADR-0024). A date mention may carry a reminder; the core derives the table of
//! them from the documents and decides which fell due (`Core::reminders_tick`, once each, within
//! seven days back). The shell runs the clock side (`reminders`): a thread that starts with the
//! sync driver when the core is ready, ticks the core at once and then every 30 seconds, and
//! stops when it locks. Each `ReminderDue` event reaches the webview like any event (the UI shows
//! its toast) and a Rust listener shows an OS notification through `tauri-plugin-notification`:
//! the title is the page's title, the body the block's text trimmed to 120 characters, shown only
//! from Rust (no `notification:*` permission is granted) and logged by nobody. The plugin has no
//! click hook on desktop, so a click on the notification does not bring the window to the front.
//! Nothing runs while the app is closed or locked: what fell due meanwhile fires at the first
//! tick afterwards. The IPC surface does not change. The same thread expires the trash
//! (ADR-0040, `Core::trash_expire`) when an hourly gate opens: the core does it at every open and
//! unlock, the loop about once an hour after, while the app runs and is unlocked.
//!
//! **Deep links and one running app** (ADR-0040, `deep_link`). `tauri-plugin-deep-link` registers the scheme
//! `atomik-kb` with the OS from `plugins.deep-link.desktop.schemes` in `tauri.conf.json`: the per-user NSIS
//! installer writes `HKCU\Software\Classes\atomik-kb` (a `URL Protocol` value, `DefaultIcon` and
//! `shell\open\command` = `"<install dir>\atomik-kb.exe" "%1"`) and removes it again when uninstalled, and the
//! `.deb` should list `x-scheme-handler/atomik-kb` in the `MimeType` of its `.desktop` file (not yet inspected;
//! `docs/development/packaging.md`). A debug build registers
//! nothing unless `KB_REGISTER_DEEP_LINK=1` is set, and then writes the same keys for the debug executable
//! (`register_all`; on Linux a `<exe>-handler.desktop` in the user's applications folder), which replaces the
//! installed app's handler until it is installed again or `HKCU\Software\Classes\atomik-kb` is deleted. Opening
//! a link starts the app with the URL as its only argument. `tauri-plugin-single-instance` (the first plugin on
//! the builder, with its `deep-link` feature) forwards that launch's arguments to the app that is running, which
//! shows, unminimises and focuses its main window, and the second process exits; the plugin identifies the app by
//! its identifier, so a debug build with `KB_DATA_DIR` (a second device, the sync demo) does not register it and
//! starts beside the first, unless `KB_SINGLE_INSTANCE=1` keeps it on (debug builds only; the real-app pass of the
//! deep links starts a second launch beside a running app on its own data directory). A link the app was started with is read from the deep-link plugin's `get_current`
//! once the window exists. Only `atomik-kb://page/<uuid>[#<uuid>]` is accepted (`deep_link::parse`: scheme in any
//! case, canonical lowercase UUIDs, no query, no extra path, at most 128 bytes) and the invitation link of a
//! shared space `atomik-kb://invite?server=<escaped address>#<52-character secret>` (`deep_link::parse_invite`,
//! equally strict); anything else, the device pairing link included, is ignored, and no URL, id or secret is ever
//! logged. The last accepted link of each kind is held until the
//! webview subscribes (`shell_events_subscribe`, once per page load, like `subscribe_events`) and then sent to it
//! as a `deepLink` or an `inviteLink` message; the UI keeps it until a workspace is open (an invitation then goes
//! to the core's `InviteLinkOpen`), so while the app is locked nothing opens until the passphrase is given. The webview is granted no permission of either plugin: the commands of the
//! deep-link plugin (`get_current`, `register`, `unregister`, `is_registered`) stay unreachable from it (a test
//! asserts it), and the plugin's `deep-link://new-url` event, which any webview could also emit, can only make
//! the UI open a page by id, which it can do anyway.
//!
//! **Tray, quick capture and the shell's own messages** (ADR-0040, `desktop`, `tray`, `window`, `shortcut`,
//! `shell_events`, `shell_labels`). The shell builds a tray icon from Rust (the app's icon, the tooltip
//! "atomik-kb", a menu of Open, New page, Lock and Quit; a left click shows, unminimises and focuses the main
//! window) and registers a global shortcut with `tauri-plugin-global-shortcut` (default `Ctrl+Alt+N`). Neither
//! needs a permission: the webview's capability gains nothing, and the plugin's commands stay unreachable from it
//! (a test asserts it). The device setting `app.tray.close_to_tray` (default true) makes the window's close
//! button hide the window instead of closing it, so the app, and the reminders (ADR-0024), keep running; Quit
//! closes the window through the normal path and the app ends with it. The device setting `app.shortcut.capture`
//! holds the accelerator (`""` turns it off; `accelerator::Accelerator` reads it strictly: at least one of Ctrl,
//! Alt or Super, one key). The shell learns of the two settings through the core's own `SettingChanged` event
//! (a Rust listener on the state, which hears every request, the webview's included) and by reading them
//! (`SettingsGet`) when the core becomes ready, which a worker thread does; while the core is locked the last
//! values stay in force, and before the first read the defaults do. The shortcut, the tray's "New page" and the
//! tray's "Lock" reach the page through `shell_events_subscribe` as `capture` and `lock` messages (a capture is
//! held until a page subscribes; a lock the page cannot hear is made by the shell, directly on the core), and the
//! state of the shortcut (`registered`, `disabled`, `taken` when the OS refuses it, `invalid`) as `shortcut`
//! messages, sent to every new subscriber. `shell_labels_set` sets the shell's words (English until the UI sends
//! its own at start and again whenever its language changes, ADR-0040 item 13): the tray's and the menu bar's (1 to 64
//! characters, no control characters), the three consent dialogs (an AI endpoint that is another computer, an AI
//! tool's access to spaces, the download of an import's images: texts of up to 400 characters with `{placeholders}`
//! the shell fills, refused when one that a consent depends on is missing), the file types of the file dialogs and
//! the title of a reminder's notification for an untitled page; never logged.
//!
//! **Native menu bar** (ADR-0040, `menu_bar`). The main window has a menu bar of File, Edit, View, Window and Help,
//! built in Rust with Tauri's menu API (the webview's capability gains nothing but the two commands below, which
//! need no permission entry) and **hidden** from the start (`Window::hide_menu`): the UI sends `menu_bar_show` for a
//! lone Alt press (Alt down and up with no other key, mouse button or composition between, see
//! `packages/ui/src/menu-bar.ts`) and the bar appears; it hides again after a choice of one of its items, on
//! `menu_bar_hide` (the UI's Escape and its first key or click in the page), when the main window loses focus and when
//! the page reloads, and each change is told to the page as a `menuBar` message. Edit's Cut, Copy, Paste and
//! Select all, and Undo and Redo on Windows, are muda's predefined items (muda sends the key to the focused window);
//! the other items are the tray's (new page, lock, quit), the shell's own (minimize, close the window, which
//! follows close to the tray) or a `menu` message the UI runs as its existing action (import, export, print, find,
//! toggle the sidebar, back, forward, about; undo and redo where they are not native). The labels are the UI's
//! (`shell_labels_set`, `&` marks a mnemonic) and the shortcut hints beside them are text after a tab, never
//! accelerators. A menu bar is not an always-visible fallback: see the module documentation of `menu_bar`.
//!
//! **Data directory.** The core lives in the app's machine-local data directory. Debug builds
//! honour `KB_DATA_DIR`, an absolute directory used instead, so two instances can run side by
//! side (the sync demo: two devices on one machine); a relative path is ignored with a
//! warning, and release builds ignore the variable.
//!
//! Webview guards (`guard`): the main window is declared in `tauri.conf.json` with
//! `"create": false` and built in `setup`, so its `on_navigation` handler can keep it on the
//! app's own origin (the dev server too in debug builds) and its `on_new_window` handler
//! can refuse every new window; a link in a page can never navigate the webview. Links the
//! UI wants opened go through `open_external`, which hands only `http`, `https` and `mailto`
//! URLs to `tauri-plugin-opener`. Both plugins are registered without handing the webview
//! anything: the opener without its click-interception script, the dialog plugin used from
//! Rust only. `capabilities/default.json` grants `core:default` and nothing else:
//! application commands need no permission entries, and the webview can call none of the
//! plugins' own commands (a permission only gates calls from the webview; the shell's own
//! Rust code uses the dialog and notification plugins without any).

#![forbid(unsafe_code)]

pub mod accelerator;
mod ai_client;
mod ai_host;
mod ai_indexer;
#[cfg(test)]
mod ai_test_support;
mod archive_path;
mod attachment_ipc;
mod attachment_names;
mod attachments;
mod backup;
mod backup_listing;
pub mod backup_scheduler;
pub mod deep_link;
pub mod desktop;
pub mod desktop_settings;
mod dialogs;
mod doc_ipc;
mod drive_host;
pub mod exporter;
mod guard;
pub mod import_source;
pub mod importer;
mod mcp_host;
mod media;
pub mod menu_bar;
pub mod presence;
mod reminders;
pub mod shell_events;
pub mod shell_labels;
pub mod shortcut;
pub mod speech_host;
mod state;
mod sync_host;
#[cfg(test)]
mod test_support;
pub mod text_file;
pub mod tray;
mod url_import;
pub mod window;

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use kb_speech::{DictationEvent, DictationStatus, InstallEvent, SpeechEngine};
use kb_storage_native::{NativeError, SystemClock};
use kb_types::ai::{
    AiChatInput, AiGenerateInput, AiScope, AiSource, AiStatus, AiStreamEvent, AiTranscribed,
    McpClientInfo, McpGrant, McpStatus,
};
use kb_types::interop::{
    BackupFileInfo, BackupFolderListing, DriveClientInput, DriveProgress, DriveStatus,
};
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, Event, ImportReport, PageId, PresenceMessage,
    PresenceState, Request, Response, SpaceId,
};
use tauri::ipc::Channel;
use tauri::webview::PageLoadEvent;
use tauri::{Manager, Runtime, State};
use tauri_plugin_opener::OpenerExt;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroizing;

pub use ai_host::AiShell;
pub use attachments::AttachmentSaved;
pub use backup::BackupResult;
pub use deep_link::{DeepLink, InviteLink};
pub use mcp_host::McpShell;
pub use reminders::{
    BODY_CHARS, HourlyGate, Notice, NotificationNotifier, Notifier, RemindersCoreApi,
    TICK_INTERVAL, TRASH_EXPIRY_INTERVAL, notice_of, notice_of_notification, notifications_starter,
    os_notification_notifier, os_notifier, reminders_starter, reminders_starter_with,
};
pub use shell_events::{ShellEvent, ShellEvents};
pub use shell_labels::LabelStore;
pub use speech_host::SpeechShell;
pub use state::{
    AppState, AttachmentSink, BackgroundStarter, BackgroundTask, BackupSink, BackupSource, CoreApi,
    CoreHost, CoreState, CoreStateInfo, Listener, LockedApi, all_of,
};
pub use sync_host::{DevicesCoreApi, InviteCoreApi, SyncCoreApi, driver_starter};
pub use window::DesktopPrefs;

/// Log filter used when `RUST_LOG` is not set.
const DEFAULT_LOG_FILTER: &str = "info";

/// Debug builds: an absolute directory that replaces the app's local data directory.
const DATA_DIR_VAR: &str = "KB_DATA_DIR";

/// Platform name the core reports in `AppStatus` and `DeviceInfo`.
const PLATFORM: &str = std::env::consts::OS;

/// Label of the app's only window, as declared in `tauri.conf.json` and named by
/// `capabilities/default.json`.
pub const MAIN_WINDOW: &str = "main";

/// What a caller is told when the thread running a core call panicked.
const CORE_STOPPED: &str = "the core stopped unexpectedly";

/// What a caller is told when the thread running a backup panicked.
const BACKUP_STOPPED: &str = "the backup could not be completed";

/// What a caller is told when the thread running an attachment import, save or open
/// panicked.
const ATTACHMENT_STOPPED: &str = "the attachment could not be processed";

/// What a caller is told when the thread running a Google Drive status read panicked.
const DRIVE_STOPPED: &str = "the Google Drive request could not be completed";

/// What a caller is told when the thread running an export panicked.
const EXPORT_STOPPED: &str = "the export could not be completed";

/// What a caller is told when the thread running an import panicked.
const IMPORT_STOPPED: &str = "the import could not be completed";

/// What a caller is told when the thread saving a text file panicked.
const TEXT_FILE_STOPPED: &str = "the file could not be saved";

/// What a caller is told when the thread running an AI request's core call panicked.
const AI_STOPPED: &str = "the AI request could not be completed";

/// What an MCP client is told when the thread running its request's core call panicked.
const MCP_STOPPED: &str = "the MCP request could not be completed";

/// The state every command shares: the core behind its lock and the event channels, seen
/// through [`CoreHost`] so the same handlers serve the real core and, in the integration
/// tests, a fake one.
pub type SharedState = Arc<dyn CoreHost>;

/// Runs a task that may block (the core touches the disk, the opener asks the OS) off the
/// async runtime. If the task panics the caller gets an `Internal` error saying
/// `on_panic` instead of the panic.
async fn blocking<T, F>(task: F, on_panic: &'static str) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(task).await {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(%error, "a blocking task failed");
            Err(ApiError::internal(on_panic))
        }
    }
}

/// The body of a raw response that has nothing to say.
fn empty_response() -> tauri::ipc::Response {
    tauri::ipc::Response::new(Vec::<u8>::new())
}

/// Runs one request on the core, off the main thread (the core is synchronous and may
/// touch the disk). A request that writes a setting only the shell may write (the folder of
/// scheduled backups, the outcome of the last one) is refused first: `InvalidInput` for the field
/// `key`, see `guard::webview_request_allowed`.
#[tauri::command]
async fn dispatch(state: State<'_, SharedState>, request: Request) -> Result<Response, ApiError> {
    guard::webview_request_allowed(&request)?;
    let state = Arc::clone(&state);
    blocking(move || state.dispatch(request), CORE_STOPPED).await
}

/// Registers the channel the core's events are sent to. Called once per page load.
#[tauri::command]
async fn subscribe_events(
    state: State<'_, SharedState>,
    on_event: Channel<Event>,
) -> Result<(), ApiError> {
    state.subscribe(on_event);
    Ok(())
}

/// Lets the UI show a loading, lock or error screen before its first `dispatch`.
#[tauri::command]
async fn core_state(state: State<'_, SharedState>) -> Result<CoreStateInfo, ApiError> {
    Ok(state.state_info())
}

/// Opens a page's document for an editor. Raw IPC: the page id is in the `x-kb-page`
/// header, any body is ignored, and the response body is the raw Loro snapshot.
#[tauri::command]
async fn doc_open(
    state: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> Result<tauri::ipc::Response, ApiError> {
    let page_id = doc_ipc::page_id(request.headers())?;
    let state = Arc::clone(&state);
    let snapshot = blocking(move || state.doc_open(page_id), CORE_STOPPED).await?;
    Ok(tauri::ipc::Response::new(snapshot))
}

/// Imports an update from the editor's replica. Raw IPC: the page id is in the `x-kb-page`
/// header, the body is the update (empty is `InvalidInput`), and the response is empty.
#[tauri::command]
async fn doc_import(
    state: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> Result<tauri::ipc::Response, ApiError> {
    let page_id = doc_ipc::page_id(request.headers())?;
    let update = doc_ipc::update(request.body())?.into_owned();
    let state = Arc::clone(&state);
    blocking(move || state.doc_import(page_id, &update), CORE_STOPPED).await?;
    Ok(empty_response())
}

/// Exports what the editor's replica lacks. Raw IPC: the page id is in the `x-kb-page`
/// header, the body is the replica's encoded version vector (empty for everything), and
/// the response body is the raw update, empty when there is nothing new.
#[tauri::command]
async fn doc_export(
    state: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> Result<tauri::ipc::Response, ApiError> {
    let page_id = doc_ipc::page_id(request.headers())?;
    let since = doc_ipc::since(request.body())?.map(Cow::into_owned);
    let state = Arc::clone(&state);
    let update = blocking(
        move || state.doc_export(page_id, since.as_deref()),
        CORE_STOPPED,
    )
    .await?;
    Ok(tauri::ipc::Response::new(update))
}

/// Drops the core's in-memory replica of a page. Raw IPC: the page id is in the `x-kb-page`
/// header, any body is ignored, and the response is empty.
#[tauri::command]
async fn doc_close(
    state: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> Result<tauri::ipc::Response, ApiError> {
    let page_id = doc_ipc::page_id(request.headers())?;
    let state = Arc::clone(&state);
    blocking(move || state.doc_close(page_id), CORE_STOPPED).await?;
    Ok(empty_response())
}

/// Opens a link in the system browser or mail client. Only `http`, `https` and `mailto`
/// URLs pass (anything else is `InvalidInput`), and the system is handed the normalised
/// URL, never the raw text.
#[tauri::command]
async fn open_external<R: Runtime>(app: tauri::AppHandle<R>, url: String) -> Result<(), ApiError> {
    let url = guard::external_url(&url)?;
    let scheme = url.scheme().to_owned();
    blocking(
        move || {
            app.opener()
                .open_url(url.as_str(), None::<&str>)
                .map_err(|error| {
                    // The OS error never contains the link; the link itself is not logged.
                    tracing::warn!(%scheme, %error, "the system could not open an external link");
                    ApiError::internal("the link could not be opened")
                })
        },
        "the link could not be opened",
    )
    .await
}

/// Engages the app lock (ADR-0018): the core closes its database and drops its key
/// material, `core_state` becomes `"locked"` and every command but `unlock` answers
/// `Locked` until the passphrase is given. The events the core had queued are delivered
/// first. The core refuses with `InvalidInput` when the app lock is not enabled (and stays
/// ready); locking a locked workspace succeeds.
#[tauri::command]
async fn lock(state: State<'_, SharedState>) -> Result<(), ApiError> {
    let state = Arc::clone(&state);
    blocking(move || state.lock(), CORE_STOPPED).await
}

/// Disengages the app lock: derives the key from `passphrase` (Argon2id, about a second, on
/// a blocking thread) and reopens the workspace. A wrong passphrase is `InvalidInput` for
/// the field `passphrase` and the state stays locked; a workspace that is not locked is
/// `InvalidInput`. The passphrase is wrapped in `Zeroizing` as it is deserialised, handed to
/// the core by reference and never logged.
#[tauri::command]
async fn unlock(
    state: State<'_, SharedState>,
    passphrase: Zeroizing<String>,
) -> Result<(), ApiError> {
    let state = Arc::clone(&state);
    blocking(move || state.unlock(&passphrase), CORE_STOPPED).await
}

/// Writes an encrypted backup of the whole workspace to a file and returns the core's
/// summary with the path. Without `path` the native save dialog asks for one (cancelling
/// is `InvalidInput` for `path`); an explicit `path` must be absolute. The core is called
/// once, under the single-writer lock, and its output goes to `<path>.tmp`, which replaces
/// `path` only when everything is on disk. Answers `Locked` (and shows no dialog) while the
/// workspace is locked.
#[tauri::command]
async fn backup_export<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    path: Option<String>,
) -> Result<BackupResult, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog or file: a locked or starting core answers at once.
            state.ensure_ready()?;
            let path = match path {
                Some(text) => backup::explicit_path(&text)?,
                None => dialogs::choose_export_path(&app)?,
            };
            backup::export_to_file(&*state, &path)
        },
        BACKUP_STOPPED,
    )
    .await
}

/// Restores a backup file into the workspace and returns the core's summary with the path.
/// Without `path` the native open dialog asks for one (cancelling is `InvalidInput` for
/// `path`); an explicit `path` must be absolute. `kit` is the recovery kit text, needed
/// unless the backup belongs to this installation's own account; it is `Zeroizing` and never
/// logged. The core is called once, under the single-writer lock, reading the file as it
/// goes, and the events it emits are published afterwards. Answers `Locked` (and shows no
/// dialog) while the workspace is locked.
#[tauri::command]
async fn backup_import<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    path: Option<String>,
    kit: Option<Zeroizing<String>>,
) -> Result<BackupResult, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog or file: a locked or starting core answers at once.
            state.ensure_ready()?;
            let path = match path {
                Some(text) => backup::explicit_path(&text)?,
                None => dialogs::choose_import_path(&app)?,
            };
            backup::import_from_file(&*state, &path, kit.as_ref().map(|kit| kit.as_str()))
        },
        BACKUP_STOPPED,
    )
    .await
}

/// Appends one plaintext piece to an upload (`AttachmentBegin` was dispatched). Raw IPC: the
/// attachment id is in the `x-kb-attachment` header, the body is the piece (1 byte to
/// `CHUNK_SIZE`; empty or larger is `InvalidInput` for the field `body`), and the response is
/// empty. The core is called once, under the single-writer lock, which is released between
/// pieces.
#[tauri::command]
async fn attachment_write(
    state: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> Result<tauri::ipc::Response, ApiError> {
    let attachment_id = attachment_ipc::attachment_id(request.headers())?;
    let piece = attachment_ipc::piece(request.body())?.into_owned();
    let state = Arc::clone(&state);
    blocking(
        move || state.attachment_write(attachment_id, &piece),
        CORE_STOPPED,
    )
    .await?;
    Ok(empty_response())
}

/// Reads an attachment's plaintext. Raw IPC: the attachment id is in the `x-kb-attachment`
/// header, a byte range in `x-kb-range-start` and `x-kb-range-end` (both or neither), any
/// body is ignored, and the response body is the raw plaintext. Without a range, an
/// attachment larger than `MAX_PREVIEW_BYTES` is `InvalidInput` for the field `size`.
#[tauri::command]
async fn attachment_read(
    state: State<'_, SharedState>,
    request: tauri::ipc::Request<'_>,
) -> Result<tauri::ipc::Response, ApiError> {
    let attachment_id = attachment_ipc::attachment_id(request.headers())?;
    let range = attachment_ipc::byte_range(request.headers())?;
    let state = Arc::clone(&state);
    let bytes = blocking(
        move || attachments::read_bytes(&*state, attachment_id, range),
        CORE_STOPPED,
    )
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Imports a file as an attachment of `space_id` and returns its `AttachmentInfo`. Without
/// `path` the native open dialog asks for a file (any file; cancelling is `InvalidInput` for
/// `path`). An explicit `path` is accepted in debug builds only (and must be absolute); a
/// release build refuses one with `InvalidInput` for `path` ("choose the file in the
/// dialog") and touches nothing. The file is read in `CHUNK_SIZE` pieces between
/// `AttachmentBegin` and `AttachmentFinish`, and the upload is aborted on any failure.
/// Answers `Locked` (and shows no dialog) while the workspace is locked.
#[tauri::command]
async fn attachment_import_path<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    space_id: SpaceId,
    path: Option<String>,
) -> Result<AttachmentInfo, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog or file: a locked or starting core answers at once.
            state.ensure_ready()?;
            attachments::import(&*state, space_id, path.as_deref(), || {
                dialogs::choose_attachment_import_path(&app)
            })
        },
        ATTACHMENT_STOPPED,
    )
    .await
}

/// Downloads the picture at `url` and imports it as an attachment of `space_id` (ADR-0028):
/// `https` only, at most 2048 characters, at most 32 MiB, the type taken from the bytes. Every
/// failure is `InvalidInput` for the field `url` (`size` for a download that is too large) with
/// `details.reason`; the link is never in a message or a log line. Answers `Locked`, or
/// `Internal: "core is starting"`, before any request is made.
#[tauri::command]
async fn attachment_import_url(
    state: State<'_, SharedState>,
    space_id: SpaceId,
    url: String,
) -> Result<AttachmentInfo, ApiError> {
    url_import::import_url(
        Arc::clone(&state),
        space_id,
        url,
        url_import::FetchPolicy::production(),
    )
    .await
}

/// Saves an attachment to a file and returns its `AttachmentInfo` with the path. Without
/// `path` the native save dialog asks where, proposing the sanitised name (cancelling is
/// `InvalidInput` for `path`). An explicit `path` is accepted in debug builds only (and must
/// be absolute); a release build refuses one with `InvalidInput` for `path` ("choose the
/// file in the dialog") and touches nothing. The plaintext goes to `<path>.tmp`, which
/// replaces `path` only when everything is on disk. Answers `Locked` (and shows no dialog)
/// while the workspace is locked.
#[tauri::command]
async fn attachment_save_as<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    attachment_id: AttachmentId,
    path: Option<String>,
) -> Result<AttachmentSaved, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog or file: a locked or starting core answers at once.
            state.ensure_ready()?;
            attachments::save_as(&*state, attachment_id, path.as_deref(), |name| {
                dialogs::choose_attachment_save_path(&app, name)
            })
        },
        ATTACHMENT_STOPPED,
    )
    .await
}

/// Opens an attachment with the system's default program and returns the path of the
/// private plaintext copy it opened: `<app cache dir>/open/<attachment id>/<sanitised
/// name>`, removed at the next start. Files the system would run (`.exe`, `.bat`, ...) are
/// refused with `InvalidInput` for the field `attachment`. Answers `Locked` while the
/// workspace is locked.
#[tauri::command]
async fn attachment_open<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    attachment_id: AttachmentId,
) -> Result<String, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any file: a locked or starting core answers at once.
            state.ensure_ready()?;
            let cache_dir = app.path().app_cache_dir().map_err(|error| {
                tracing::warn!(%error, "the app cache directory is unknown");
                ApiError::internal("the temporary directory is not available")
            })?;
            let path = attachments::open_copy(&*state, &cache_dir, attachment_id, |path| {
                tauri_plugin_opener::open_path(path, None::<&str>).map_err(|_| {
                    // The error can name the file, which is user content: not logged.
                    ApiError::internal("the file could not be opened")
                })
            })?;
            Ok(path.to_string_lossy().into_owned())
        },
        ATTACHMENT_STOPPED,
    )
    .await
}

/// Exports a page, a page with its sub-pages or a whole space (ADR-0036): the core plans the
/// export, the native save dialog asks where (a `.md` or `.html` file for one page and nothing
/// else, otherwise a `.zip`; cancelling is `InvalidInput` for `path` and ends the core's session),
/// and the entries are written to `<path>.tmp`, renamed into place at the end. Answers `{ path,
/// pages, attachments, warnings }`. Answers `Locked` (and shows no dialog) while the workspace is
/// locked.
#[tauri::command]
async fn export_run<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    options: exporter::ExportRunOptions,
) -> Result<exporter::ExportRunResult, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog or file: a locked or starting core answers at once.
            state.ensure_ready()?;
            let format = options.format;
            exporter::run(&*state, &options, |plan| {
                dialogs::choose_save_path(&app, &exporter::proposed_file_name(plan, format))
            })
        },
        EXPORT_STOPPED,
    )
    .await
}

/// Imports files, a folder or a `.zip` the person picks in the dialog `options.source` names
/// (ADR-0036; cancelling is `InvalidInput` for `path`) into `options.spaceId` under
/// `options.parentId` (or at the space root), or, with `options.destination` `"newSpaces"`, into
/// one new space per space of the source (ADR-0039), and answers the core's `ImportReport` with
/// the shell's own warnings appended. When the source keeps images on a server (an AppFlowy
/// workspace's uploads) a native Yes/No dialog asks once before anything is downloaded. Answers
/// `Locked` (and shows no dialog) while the workspace is locked.
#[tauri::command]
async fn import_run<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    options: importer::ImportRunOptions,
) -> Result<ImportReport, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog or file: a locked or starting core answers at once.
            state.ensure_ready()?;
            options.check()?;
            let source = match options.source {
                importer::ImportSourceKind::Files => {
                    import_source::ImportSource::from_files(&dialogs::choose_import_files(&app)?)?
                }
                importer::ImportSourceKind::Folder => {
                    import_source::ImportSource::from_folder(&dialogs::choose_import_folder(&app)?)?
                }
                importer::ImportSourceKind::Zip => {
                    let zip = dialogs::choose_import_zip(&app)?;
                    let cache_dir = app.path().app_cache_dir().map_err(|error| {
                        tracing::warn!(%error, "the app cache directory is unknown");
                        ApiError::internal("the temporary directory is not available")
                    })?;
                    import_source::ImportSource::from_zip(&zip, &cache_dir)?
                }
            };
            // The question about downloading images is a native dialog the page cannot fake,
            // owned by the main window.
            let remote = importer::DownloadAfterConsent::new(
                Arc::new(dialogs::NativeConfirm::new(app)),
                url_import::FetchPolicy::production(),
            );
            importer::run(&*state, &options, source, &remote)
        },
        IMPORT_STOPPED,
    )
    .await
}

/// Saves a text (the CSV of a view) to a file the person picks in the save dialog, which proposes
/// `suggested_name` made safe for a path; at most 64 MiB. The text is written to `<path>.tmp` and
/// renamed into place. Answers the path. Cancelling is `InvalidInput` for `path`. Answers
/// `Locked` (and shows no dialog) while the workspace is locked.
#[tauri::command]
async fn save_text_file<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    suggested_name: String,
    text: String,
) -> Result<String, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog or file: a locked or starting core answers at once.
            state.ensure_ready()?;
            text_file::save(&suggested_name, &text, |name| {
                dialogs::choose_save_path(&app, name)
            })
        },
        TEXT_FILE_STOPPED,
    )
    .await
}

/// Opens the native folder dialog and stores the folder in `app.backup.folder` (a setting only the
/// shell writes), where scheduled backups go. Answers the folder. Cancelling is `InvalidInput`
/// for `path`. Answers `Locked` (and shows no dialog) while the workspace is locked.
#[tauri::command]
async fn backup_folder_choose<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
) -> Result<String, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog: a locked or starting core answers at once.
            state.ensure_ready()?;
            backup_scheduler::choose_folder(&*state, || dialogs::choose_backup_folder(&app))
        },
        BACKUP_STOPPED,
    )
    .await
}

/// Writes a scheduled backup now (ADR-0036, ADR-0059): one export, named for the clock, into the
/// chosen folder (old scheduled backups pruned) or a staging file, delivered to Google Drive when it
/// is connected; the outcome recorded in `app.backup.last`, failures included. Answers
/// `{ summary, path }`, the path being the folder's file or the staging file's. Answers `Locked`
/// while the workspace is locked.
#[tauri::command]
async fn backup_run_now(
    state: State<'_, SharedState>,
    targets: State<'_, Arc<backup_scheduler::Targets>>,
) -> Result<BackupResult, ApiError> {
    let state = Arc::clone(&state);
    let targets = Arc::clone(&targets);
    blocking(
        move || backup_scheduler::run_now(&*state, &SystemClock, &targets),
        BACKUP_STOPPED,
    )
    .await
}

/// Where Ollama is, whether it answers, which models it has, the semantic index and the indexer
/// (ADR-0037). Works with AI off; a remote endpoint without consent is not contacted (the status says
/// so). Answers `Locked`, or "core is starting", while the core is not ready.
#[tauri::command]
async fn ai_status(
    state: State<'_, SharedState>,
    ai: State<'_, Arc<AiShell>>,
) -> Result<AiStatus, ApiError> {
    ai_host::status(Arc::clone(&state), Arc::clone(&ai)).await
}

/// Stores the Ollama endpoint in `app.ai.endpoint` and answers the new status. A URL that is not this
/// machine first asks the person in a native dialog (made from Rust, so the page cannot fake it);
/// declining is `InvalidInput` for `url` with `details.reason = "declined"` and changes nothing.
#[tauri::command]
async fn ai_endpoint_set<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    ai: State<'_, Arc<AiShell>>,
    url: String,
) -> Result<AiStatus, ApiError> {
    let confirm: Arc<dyn dialogs::Confirm> = Arc::new(dialogs::NativeConfirm::new(app));
    ai_host::endpoint_set(Arc::clone(&state), Arc::clone(&ai), confirm, url).await
}

/// Runs a writing action and streams its result over `on_event` (`delta`s, then `done` or `error`).
/// `request_id` (1 to 64 of `[A-Za-z0-9_-]`) names the stream for `ai_cancel`. Rejects, without any
/// event, when nothing started: a bad or busy `requestId` (at most four streams run), AI off, no
/// consent for the endpoint, no chat model chosen, a core that is locked or starting.
#[tauri::command]
async fn ai_generate(
    state: State<'_, SharedState>,
    ai: State<'_, Arc<AiShell>>,
    request_id: String,
    input: AiGenerateInput,
    on_event: Channel<AiStreamEvent>,
) -> Result<(), ApiError> {
    ai_host::generate(
        Arc::clone(&state),
        Arc::clone(&ai),
        request_id,
        input,
        &|event| on_event.send(event).is_ok(),
    )
    .await
}

/// Answers the last message of a conversation from the person's pages and streams the answer over
/// `on_event` (`sources` first, then `delta`s, then `done` or `error`). Rejects like `ai_generate`.
#[tauri::command]
async fn ai_chat(
    state: State<'_, SharedState>,
    ai: State<'_, Arc<AiShell>>,
    request_id: String,
    input: AiChatInput,
    on_event: Channel<AiStreamEvent>,
) -> Result<(), ApiError> {
    ai_host::chat(
        Arc::clone(&state),
        Arc::clone(&ai),
        request_id,
        input,
        &|event| on_event.send(event).is_ok(),
    )
    .await
}

/// Cancels a stream: its last event is `error` with `details.reason = "cancelled"`. An id that is not
/// running is fine.
#[tauri::command]
async fn ai_cancel(ai: State<'_, Arc<AiShell>>, request_id: String) -> Result<(), ApiError> {
    ai_host::cancel(&ai, &request_id);
    Ok(())
}

/// Related passages for a query: semantic when the query embeds in time and the index is on, full text
/// only otherwise. Rejects like `ai_generate` (without the stream's reasons).
#[tauri::command]
async fn ai_search(
    state: State<'_, SharedState>,
    ai: State<'_, Arc<AiShell>>,
    query: String,
    scope: AiScope,
    limit: Option<u32>,
) -> Result<Vec<AiSource>, ApiError> {
    ai_host::search(Arc::clone(&state), Arc::clone(&ai), query, scope, limit).await
}

/// Transcribes one WAV segment. Raw IPC: the body is a 16 kHz mono PCM WAV file of at most 4 MiB
/// (`InvalidInput` for `audio` otherwise), the optional header `x-kb-ai` is JSON `{ "model"?: string }`
/// (else the chosen transcription model; none is `InvalidInput` for `model`, reason `missing`), and the
/// answer is JSON: `{ text, language? }`.
#[tauri::command]
async fn ai_transcribe(
    state: State<'_, SharedState>,
    ai: State<'_, Arc<AiShell>>,
    request: tauri::ipc::Request<'_>,
) -> Result<AiTranscribed, ApiError> {
    let model = ai_host::model_from_header(request.headers())?;
    let bytes = doc_ipc::payload(request.body())?;
    // Before the copy: a body of any size is not read into a second buffer.
    ai_host::check_audio_size(&bytes)?;
    let wav = bytes.into_owned();
    ai_host::transcribe(Arc::clone(&state), Arc::clone(&ai), wav, model).await
}

/// The MCP server's state (ADR-0037): whether it listens, its URL and port, the bridge's path (next to
/// the app's executable, when it exists) and why it does not listen. Answers `Locked`, or "core is
/// starting", while the core is not ready.
#[tauri::command]
async fn mcp_status(
    state: State<'_, SharedState>,
    mcp: State<'_, Arc<McpShell>>,
) -> Result<McpStatus, ApiError> {
    mcp_host::status(Arc::clone(&state), Arc::clone(&mcp)).await
}

/// Replaces the grants of an MCP client. When they add read or write access to any space, the person
/// is asked in a native dialog first; declining is `InvalidInput` for `grants` with
/// `details.reason = "declined"` and changes nothing. Removing access asks nothing.
#[tauri::command]
async fn mcp_client_grants_set<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    client_id: String,
    grants: Vec<McpGrant>,
) -> Result<McpClientInfo, ApiError> {
    let confirm: Arc<dyn dialogs::Confirm> = Arc::new(dialogs::NativeConfirm::new(app));
    mcp_host::grants_set(Arc::clone(&state), confirm, client_id, grants).await
}

/// Registers the channel the shell's own messages are sent to (ADR-0040), `{ type, data? }` each: `deepLink` (every
/// `atomik-kb://page/<uuid>[#<uuid>]` link the app is opened with, as `{ pageId, blockId? }`), `capture` (the
/// global shortcut or the tray's "New page"), `lock` (the tray's "Lock") and `shortcut` (`{ state }`, the state of
/// the capture shortcut, sent at the subscription too). A link or a capture that came before the subscription is
/// sent at once. Called once per page load, like `subscribe_events`; the shell forgets the channel when the page
/// reloads. Anything else the OS hands the app is ignored and never reaches the channel.
#[tauri::command]
async fn shell_events_subscribe(
    events: State<'_, Arc<ShellEvents>>,
    on_event: Channel<ShellEvent>,
) -> Result<(), ApiError> {
    events.subscribe(Box::new(move |event| on_event.send(event.clone()).is_ok()));
    Ok(())
}

/// Sends this device's presence in a page of a shared space (M13 slice 3, ADR-0050): the core seals it under the space
/// key and the realtime socket relays it; `update` is the editor's ephemeral-store update as byte values, `pageId` is
/// left out for "in no page". Best effort: a personal space, an archive and a device with no socket send nothing, and
/// that is `Ok`; at most ten updates a second go through (the rest are dropped, also `Ok`). See `presence`.
#[tauri::command]
async fn presence_send(
    shared: State<'_, SharedState>,
    space_id: SpaceId,
    page_id: Option<PageId>,
    state: PresenceState,
    update: Vec<u8>,
) -> Result<(), ApiError> {
    let shared = Arc::clone(&shared);
    blocking(
        move || presence::send(&shared, space_id, page_id, state, update),
        CORE_STOPPED,
    )
    .await
}

/// Registers the channel the accepted presence frames of other members are sent to (M13 slice 3, ADR-0050), each a
/// `PresenceMessage`. Called once per page load, like `subscribe_events`; the shell forgets the channel when the page
/// reloads. See `presence`.
#[tauri::command]
async fn presence_events(
    shared: State<'_, SharedState>,
    on_event: Channel<PresenceMessage>,
) -> Result<(), ApiError> {
    presence::subscribe(&shared, on_event)
}

/// Sets the words of the shell's own surfaces, the tray menu, the menu bar, the native dialogs and the file types of
/// the file dialogs (ADR-0040): `labels` is a map of label ids (`open`, `newPage`, `lock`, `quit` for the tray;
/// `menuFile`, `menuNewPage`, ... for the menu bar; `dialogAiTitle`, `dialogAiMessage`, ... for the dialogs;
/// `fileFilterZip`, ... for the file types) to strings, each optional; only the ones named change. The UI sends its
/// strings as soon as it starts and again whenever its language changes. A label is 1 to 64 characters without control
/// characters, a dialog text up to 400 with the `{placeholders}` it needs; one that is not valid is `InvalidInput` for
/// its field and nothing changes. The strings are never logged.
#[tauri::command]
async fn shell_labels_set<R: Runtime>(
    app: tauri::AppHandle<R>,
    labels: shell_labels::ShellLabels,
) -> Result<(), ApiError> {
    desktop::set_labels(&app, &labels)
}

/// Shows the native menu bar (ADR-0040): the UI calls it for a lone Alt press in the webview while the bar is hidden.
/// Showing a bar that is shown changes nothing. Nothing happens, and nothing fails, where the shell has no menu bar
/// (macOS, or one that could not be built).
#[tauri::command]
async fn menu_bar_show<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), ApiError> {
    menu_bar::show(&app);
    Ok(())
}

/// Hides the native menu bar (ADR-0040): the UI calls it for Escape and for the first key or click in the page
/// while the bar is shown. It may already be hidden.
#[tauri::command]
async fn menu_bar_hide<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), ApiError> {
    menu_bar::set_visible(&app, false);
    Ok(())
}

/// The dictation engines and the engine chosen on this device (ADR-0053): for each, whether its model is
/// installed, whether it can run here, its download size, languages and attribution. Answers `Locked`, or "core
/// is starting", while the core is not ready.
#[tauri::command]
async fn dictation_status(
    state: State<'_, SharedState>,
    speech: State<'_, Arc<SpeechShell>>,
) -> Result<DictationStatus, ApiError> {
    speech_host::status(Arc::clone(&state), Arc::clone(&speech)).await
}

/// Chooses the engine of this device and answers the new status. Any engine may be chosen, installed or not.
#[tauri::command]
async fn dictation_select(
    state: State<'_, SharedState>,
    speech: State<'_, Arc<SpeechShell>>,
    engine: SpeechEngine,
) -> Result<DictationStatus, ApiError> {
    speech_host::select(Arc::clone(&state), Arc::clone(&speech), engine).await
}

/// Downloads the model of an engine after a native dialog (made from Rust, so the page cannot fake it) that
/// names its size and the host, and streams `InstallEvent`s over `on_event` until `done` or `error`. Rejects only
/// when nothing started: the core is locked, the engine has no download, another installation runs, the
/// person declined (`InvalidInput` for `engine`, reason `consentDeclined`).
#[tauri::command]
async fn dictation_install<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    speech: State<'_, Arc<SpeechShell>>,
    engine: SpeechEngine,
    on_event: Channel<InstallEvent>,
) -> Result<(), ApiError> {
    let confirm: Arc<dyn dialogs::Confirm> = Arc::new(dialogs::NativeConfirm::new(app));
    speech_host::install(
        Arc::clone(&state),
        Arc::clone(&speech),
        confirm,
        engine,
        &|event| on_event.send(event).is_ok(),
    )
    .await
}

/// Cancels the installation that runs; its last event is `error` with the code `cancelled`. Nothing running is
/// fine.
#[tauri::command]
async fn dictation_install_cancel(speech: State<'_, Arc<SpeechShell>>) -> Result<(), ApiError> {
    speech_host::install_cancel(&speech);
    Ok(())
}

/// "Use model files...": a native folder dialog, then every file of the engine's model is checked against its
/// pinned size and SHA-256 and copied into the store. Answers the new status.
#[tauri::command]
async fn dictation_import<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    speech: State<'_, Arc<SpeechShell>>,
    engine: SpeechEngine,
) -> Result<DictationStatus, ApiError> {
    speech_host::import(app, Arc::clone(&state), Arc::clone(&speech), engine).await
}

/// Deletes an engine's model files from the store and answers the new status.
#[tauri::command]
async fn dictation_remove(
    state: State<'_, SharedState>,
    speech: State<'_, Arc<SpeechShell>>,
    engine: SpeechEngine,
) -> Result<DictationStatus, ApiError> {
    speech_host::remove(Arc::clone(&state), Arc::clone(&speech), engine).await
}

/// Starts a dictation session: `session_id` (1 to 64 of `[A-Za-z0-9_-]`) names it for the other commands, and its
/// `DictationEvent`s go to `on_event` (`state`, `phrase`, `error`, and `ended` last). Resolves once the session is
/// registered and the engine process runs; rejects, with nothing started, when another session runs (`busy`),
/// the model is not installed, the engine's files are missing or Ollama cannot be used.
#[tauri::command]
async fn dictation_start(
    state: State<'_, SharedState>,
    speech: State<'_, Arc<SpeechShell>>,
    ai: State<'_, Arc<AiShell>>,
    session_id: String,
    engine: SpeechEngine,
    on_event: Channel<DictationEvent>,
) -> Result<(), ApiError> {
    speech_host::validate_session_id(&session_id)?;
    speech_host::start(
        Arc::clone(&state),
        Arc::clone(&speech),
        Arc::clone(&ai),
        session_id,
        engine,
        Arc::new(move |event| on_event.send(event).is_ok()),
    )
    .await
}

/// Takes audio for a session. Raw IPC: the body is 16 kHz mono 16-bit little-endian samples, 1 byte to 2 seconds
/// (`InvalidInput` for `audio` otherwise), the header `x-kb-dictation` is the session id (`InvalidInput` for
/// `sessionId` when it is missing, malformed or not a running session). Answers nothing, and never waits for the
/// engine.
#[tauri::command]
async fn dictation_audio(
    speech: State<'_, Arc<SpeechShell>>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), ApiError> {
    let session_id = speech_host::session_from_header(request.headers())?;
    let bytes = doc_ipc::payload(request.body())?;
    let pcm = speech_host::pcm_from_body(&bytes)?;
    speech_host::audio(&speech, &session_id, pcm)
}

/// Ends a session after its last phrase: the engine finishes what it heard, then `ended` follows. Returns at
/// once; an unknown session is fine.
#[tauri::command]
async fn dictation_stop(
    speech: State<'_, Arc<SpeechShell>>,
    session_id: String,
) -> Result<(), ApiError> {
    speech_host::stop(&speech, &session_id);
    Ok(())
}

/// Ends a session now, without its last phrases: `ended` follows. An unknown session is fine.
#[tauri::command]
async fn dictation_cancel(
    speech: State<'_, Arc<SpeechShell>>,
    session_id: String,
) -> Result<(), ApiError> {
    speech_host::cancel(&speech, &session_id);
    Ok(())
}

/// The Google Drive target's state for Settings (ADR-0059): the client in use (built in, the person's own, or none),
/// whether a connection is made and to which account, the folder's address, whether Google refused the stored
/// token, and the last upload. No network. Works while the core is locked or still opening (the answer is then
/// that of a Drive that is not connected).
#[tauri::command]
async fn drive_status(
    state: State<'_, SharedState>,
    drive: State<'_, Arc<drive_host::DriveShell>>,
) -> Result<DriveStatus, ApiError> {
    let (state, drive) = (Arc::clone(&state), Arc::clone(&drive));
    blocking(move || drive_host::status(&*state, &drive), DRIVE_STOPPED).await
}

/// Connects Google Drive: a native notice (made from Rust, so the page cannot fake it) says what goes to Google
/// and what Google can see; then the sign-in opens in the system browser and the shell waits on a loopback
/// address for its redirect (five minutes at most); the refresh token goes to the OS keystore; the
/// "atomik-kb backups" folder is found or made. Answers the new status. `InvalidInput` for the field `drive`
/// with `details.reason` `noClient`, `consentDeclined`, `cancelled`, `browserTimeout`, `denied`, `network` or `server`.
/// Answers `Locked` (and shows no dialog) while the workspace is locked.
#[tauri::command]
async fn drive_connect<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    drive: State<'_, Arc<drive_host::DriveShell>>,
) -> Result<DriveStatus, ApiError> {
    let confirm: Arc<dyn dialogs::Confirm> = Arc::new(dialogs::NativeConfirm::new(app.clone()));
    let browser: Arc<dyn drive_host::Browser> = Arc::new(drive_host::SystemBrowser::new(app));
    drive_host::connect(Arc::clone(&state), Arc::clone(&drive), confirm, browser).await
}

/// Disconnects Google Drive: revokes the refresh token at Google (errors ignored), deletes it from the keystore and
/// clears the connection; the files stay in Drive. Answers the new status.
#[tauri::command]
async fn drive_disconnect(
    state: State<'_, SharedState>,
    drive: State<'_, Arc<drive_host::DriveShell>>,
) -> Result<DriveStatus, ApiError> {
    drive_host::disconnect(Arc::clone(&state), Arc::clone(&drive)).await
}

/// Uses one's own Google client (`null`: the built-in one): the id goes to `app.backup.drive`, the secret to the OS
/// keystore. A token is bound to its client, so this disconnects first. Answers the new status. `InvalidInput` for
/// `clientId` or `clientSecret` when a text is not one.
#[tauri::command]
async fn drive_client_set(
    state: State<'_, SharedState>,
    drive: State<'_, Arc<drive_host::DriveShell>>,
    client: Option<DriveClientInput>,
) -> Result<DriveStatus, ApiError> {
    drive_host::client_set(Arc::clone(&state), Arc::clone(&drive), client).await
}

/// The `.atomikkb` files of the Drive folder, newest first, at most 200 (`id` is the Drive file id). The folder
/// is looked up again when its id is unknown or gone; with no folder the list is empty. `InvalidInput` for the
/// field `drive` with the reason `notConnected`, `reconnect`, `network` or `server`.
#[tauri::command]
async fn drive_backups_list(
    state: State<'_, SharedState>,
    drive: State<'_, Arc<drive_host::DriveShell>>,
) -> Result<Vec<BackupFileInfo>, ApiError> {
    drive_host::backups_list(Arc::clone(&state), Arc::clone(&drive)).await
}

/// Downloads a backup of the Drive folder into `<app data>/restore-staging/<random>.atomikkb` (the folder is
/// emptied first) and answers that absolute path, which `backup_import` takes. `DriveProgress` is sent to
/// `on_event` at most every 250 ms. A failed or cancelled download leaves no file. `InvalidInput` for
/// `fileId` when it is not a Drive file id, else for the field `drive` with the reason `notConnected`,
/// `reconnect`, `notFound`, `cancelled`, `network` or `server`.
#[tauri::command]
async fn drive_backup_download(
    state: State<'_, SharedState>,
    drive: State<'_, Arc<drive_host::DriveShell>>,
    file_id: String,
    on_event: Channel<DriveProgress>,
) -> Result<String, ApiError> {
    let emit: drive_host::Emit = Arc::new(move |progress| on_event.send(progress).is_ok());
    drive_host::download(Arc::clone(&state), Arc::clone(&drive), file_id, emit).await
}

/// Ends a sign-in that waits for the browser and a download that runs (each rejects with the reason `cancelled`);
/// with none running, nothing.
#[tauri::command]
async fn drive_cancel(drive: State<'_, Arc<drive_host::DriveShell>>) -> Result<(), ApiError> {
    drive_host::cancel(&drive);
    Ok(())
}

/// Lists the backups of a folder for the restore wizard: the `.atomikkb` files of the folder (not of its
/// sub-folders), newest first by modification time, at most 200, each named by its absolute path, which
/// `backup_import` takes. Without `path` the native folder dialog asks for the folder (cancelling is
/// `InvalidInput` for `path`, "no file was chosen"); an explicit `path` must be absolute. A folder that cannot be
/// read is `InvalidInput` for `path` with another message. Answers `Locked` (and shows no dialog) while the
/// workspace is locked.
#[tauri::command]
async fn backup_folder_list<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, SharedState>,
    path: Option<String>,
) -> Result<BackupFolderListing, ApiError> {
    let state = Arc::clone(&state);
    blocking(
        move || {
            // Before any dialog: a locked or starting core answers at once.
            state.ensure_ready()?;
            let folder = match path {
                Some(text) => PathBuf::from(text),
                None => dialogs::choose_backup_folder(&app)?,
            };
            backup_listing::list_folder(&folder)
        },
        BACKUP_STOPPED,
    )
    .await
}

/// Maps what can go wrong while building the platform dependencies to the API error
/// the UI shows.
fn api_error_from_native(error: &NativeError) -> ApiError {
    match error {
        NativeError::Keystore(_) => ApiError::keystore_unavailable(error.to_string()),
        NativeError::DataDir(_) => ApiError::internal(error.to_string()),
    }
}

/// Builds the platform pieces and opens the core: ready, or locked when the app lock is
/// on. Blocking: keystore access, database open and migrations.
fn open_core(data_dir: &Path) -> Result<kb_core::Opened, ApiError> {
    std::fs::create_dir_all(data_dir).map_err(|error| {
        ApiError::internal(format!("cannot create the data directory: {error}"))
    })?;
    let deps = kb_storage_native::native_deps(data_dir, PLATFORM)
        .map_err(|error| api_error_from_native(&error))?;
    kb_core::Core::open_or_locked(deps)
}

/// Opens the core without delaying the window, then records the outcome in the state.
fn open_core_in_background(state: Arc<AppState>, data_dir: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let outcome = tauri::async_runtime::spawn_blocking(move || open_core(&data_dir)).await;
        match outcome {
            Ok(Ok(kb_core::Opened::Ready(core))) => {
                tracing::info!("core ready");
                state.set_ready(core);
            }
            Ok(Ok(kb_core::Opened::Locked(locked))) => {
                tracing::info!("core locked: waiting for the app-lock passphrase");
                state.set_locked(locked);
            }
            Ok(Err(error)) => {
                tracing::error!(code = ?error.code, message = %error.message, "core failed to open");
                state.set_failed(error);
            }
            Err(error) => {
                tracing::error!(%error, "core opening task failed");
                state.set_failed(ApiError::internal("the core could not be started"));
            }
        }
    });
}

/// The data directory `KB_DATA_DIR` asks for, when it may be used: debug builds only, and
/// only an absolute path (a relative one would depend on the working directory). The path is
/// never logged.
fn data_dir_override(value: Option<OsString>, debug_build: bool) -> Option<PathBuf> {
    let value = value?;
    if !debug_build {
        tracing::warn!("{DATA_DIR_VAR} is ignored in release builds");
        return None;
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        tracing::warn!("{DATA_DIR_VAR} is not an absolute path: ignored");
        return None;
    }
    tracing::info!("data directory taken from {DATA_DIR_VAR}");
    Some(path)
}

/// The HTTP stack's targets that name the servers the shell talks to: `hyper_util` logs the
/// address of every connection it makes (at `debug`), `h2` the `:authority` of every request (at
/// `trace`), `reqwest` the destination of a new connection (at `debug`) and `rustls` the server name
/// of a handshake (at `debug`). With "Image from URL" (ADR-0028) those are the hosts of the person's
/// links, and with local AI (ADR-0037) the endpoint's host: the shell promises never to log either,
/// so they stay at `warn` whatever `RUST_LOG` asks for. The deep-link plugin is held at `error`: a debug
/// build of it warns with the text of a launch argument that is a URL of another scheme, and the shell
/// promises never to log a link (ADR-0040).
const QUIET_TARGETS: [&str; 6] = [
    "hyper_util=warn",
    "hyper=warn",
    "h2=warn",
    "reqwest=warn",
    "rustls=warn",
    "tauri_plugin_deep_link=error",
];

/// `filter` with the targets of [`QUIET_TARGETS`] held at the level each names.
fn log_filter(filter: EnvFilter) -> EnvFilter {
    QUIET_TARGETS.iter().fold(filter, |filter, text| match text
        .parse::<tracing_subscriber::filter::Directive>()
    {
        Ok(directive) => filter.add_directive(directive),
        Err(_) => filter,
    })
}

fn init_logging() {
    let filter = log_filter(
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER)),
    );
    // A second initialisation (tests, embedding) keeps the first subscriber.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Builds the main window from its `tauri.conf.json` entry. That entry says `"create":
/// false`, so Tauri does not build the window itself and this is the only place it comes
/// to exist, with the navigation guard attached before its first page loads. [`run`] calls
/// it from `setup`; it is public so the integration tests can build the window on the mock
/// runtime.
///
/// # Errors
///
/// When the configuration has no window labelled `main`, or the window cannot be built.
pub fn create_main_window<R: Runtime>(
    manager: &impl Manager<R>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = manager
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW)
        .ok_or("tauri.conf.json declares no window labelled \"main\"")?;
    tauri::WebviewWindowBuilder::from_config(manager, config)?
        .on_navigation(guard::allow_navigation)
        .on_new_window(guard::deny_new_window)
        .build()?;
    Ok(())
}

/// What the compiled app is made of: `tauri.conf.json`, the capabilities and the embedded
/// frontend. Public for the integration tests, which pair it with the mock runtime.
pub fn context<R: Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

/// Everything the shell adds to a Tauri builder: its state, the dialog and opener plugins
/// and the commands. [`run`] uses it with the real runtime and the integration tests with
/// the mock runtime (and a fake core behind the state), so the tests send their IPC
/// messages to the very handlers the app registers.
pub fn configure<R: Runtime>(builder: tauri::Builder<R>, state: SharedState) -> tauri::Builder<R> {
    // Google Drive as a backup target (ADR-0059): its shell is managed for its commands, and is the
    // Drive target of the scheduled backups, whose own managed state (`run` tells it the data
    // directory, and the timer takes the same `Arc`) is made from it.
    let drive = Arc::new(drive_host::DriveShell::new());
    let backup_targets = Arc::new(backup_scheduler::Targets::new(
        Arc::clone(&drive) as Arc<dyn backup_scheduler::DriveTarget>
    ));
    // The media protocol (ADR-0032): video and audio stream from the core, a range at a time.
    media::register(builder, Arc::clone(&state))
        .manage(state)
        // The AI features' client, streams and indexer state, and the MCP listener's (ADR-0037); the
        // background tasks that need them (`run`) take the same `Arc`s from here.
        .manage(Arc::new(AiShell::new()))
        .manage(Arc::new(McpShell::new()))
        // The dictation sidecar's sessions and the model store's location (ADR-0053); `run` tells it the
        // directories, and the background starter that ends a session on lock takes the same `Arc`.
        .manage(Arc::new(SpeechShell::new()))
        // What the shell tells the webview waits here for it (ADR-0040): the links of the OS, the quick capture,
        // the tray's lock and the shortcut's state; `run` and the tray feed it.
        .manage(Arc::new(ShellEvents::new()))
        // The inputs of the close button's decision and the words of the tray (ADR-0040); the settings worker
        // and `shell_labels_set` change them.
        .manage(Arc::new(DesktopPrefs::new()))
        .manage(Arc::new(LabelStore::new()))
        // Google Drive as a backup target (ADR-0059): the access token and a sign-in or download in progress.
        .manage(drive)
        .manage(backup_targets)
        // A close request of the main window hides it to the tray while the setting says so, and its loss of
        // focus hides the menu bar.
        .on_window_event(window::on_window_event)
        // A choice in the menu bar (the tray's own menu has its own handler and other ids).
        .on_menu_event(|app, event| menu_bar::on_menu_event(app, &event))
        // Used from Rust only (the backup commands): the capability grants no `dialog:*`
        // permission, so the webview cannot open a dialog itself.
        .plugin(tauri_plugin_dialog::init())
        // No click interception: links reach the system only through `open_external`, which
        // filters the scheme first. The plugin's own commands stay unreachable because
        // the capability grants no `opener:*` permission.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        // Used from Rust only (`run` reads the link the app was started with and listens for the next
        // ones): the capability grants no `deep-link:*` permission, so the webview can neither read
        // them, register the scheme nor unregister it. The single-instance plugin, which must come
        // before every other plugin, is registered by `run` on the builder it hands to `configure`.
        .plugin(tauri_plugin_deep_link::init())
        // Used from Rust only (the reminders' OS notification, `reminders.rs`): the capability
        // grants no `notification:*` permission, so the webview cannot show one itself.
        .plugin(tauri_plugin_notification::init())
        .on_page_load(|webview, payload| {
            // A page that starts loading has lost the callbacks of its old channels.
            if matches!(payload.event(), PageLoadEvent::Started) {
                webview.state::<SharedState>().clear_subscribers();
                webview.state::<Arc<ShellEvents>>().clear_subscribers();
                // The new page starts with the menu bar hidden, as the UI believes it is.
                menu_bar::set_visible(webview.app_handle(), false);
            }
        })
        .invoke_handler(tauri::generate_handler![
            dispatch,
            subscribe_events,
            core_state,
            doc_open,
            doc_import,
            doc_export,
            doc_close,
            open_external,
            lock,
            unlock,
            backup_export,
            backup_import,
            attachment_write,
            attachment_read,
            attachment_import_path,
            attachment_import_url,
            attachment_save_as,
            attachment_open,
            export_run,
            import_run,
            save_text_file,
            backup_folder_choose,
            backup_run_now,
            ai_status,
            ai_endpoint_set,
            ai_generate,
            ai_chat,
            ai_cancel,
            ai_search,
            ai_transcribe,
            mcp_status,
            mcp_client_grants_set,
            shell_events_subscribe,
            shell_labels_set,
            menu_bar_show,
            menu_bar_hide,
            presence_send,
            presence_events,
            dictation_status,
            dictation_select,
            dictation_install,
            dictation_install_cancel,
            dictation_import,
            dictation_remove,
            dictation_start,
            dictation_audio,
            dictation_stop,
            dictation_cancel,
            drive_status,
            drive_connect,
            drive_disconnect,
            drive_client_set,
            drive_backups_list,
            drive_backup_download,
            drive_cancel,
            backup_folder_list
        ])
}

/// Starts the app. Returns when the last window closes or the runtime fails.
pub fn run() -> tauri::Result<()> {
    init_logging();
    let app_state: Arc<AppState> = Arc::new(AppState::new());
    let shared = Arc::clone(&app_state) as SharedState;
    let desktop_state = Arc::clone(&shared);
    // Debug builds may point the data directory elsewhere with `KB_DATA_DIR` (below); the same choice
    // decides whether this launch is the one running app (a second device on its own directory is not).
    let data_dir_choice = data_dir_override(std::env::var_os(DATA_DIR_VAR), cfg!(debug_assertions));

    let mut builder = tauri::Builder::default();
    // A debug build with `KB_DATA_DIR` may still take part, with `KB_SINGLE_INSTANCE=1` (the deep-link pass).
    let keep_single = deep_link::keep_single_instance(
        std::env::var_os(deep_link::SINGLE_INSTANCE_VAR),
        cfg!(debug_assertions),
    );
    if deep_link::single_instance_applies(data_dir_choice.as_deref(), keep_single) {
        // The first plugin on the builder, as its documentation asks: a second launch must be
        // detected before anything else is set up. It hands that launch's arguments to the running
        // app, whose deep-link plugin raises the event for a link among them (the `deep-link`
        // feature), and the second process exits; here the running app comes to the front.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            deep_link::on_second_launch(app);
        }));
    }

    configure(builder, shared)
        .setup(move |app| {
            create_main_window(app)?;
            // The native menu bar, hidden until a lone Alt press (ADR-0040): on the window before its first paint.
            menu_bar::install(app.handle());
            // The links of the OS (ADR-0040), after the window exists: a link the app was started
            // with waits in `ShellEvents` for the webview to subscribe.
            deep_link::listen(app);
            #[cfg(debug_assertions)]
            deep_link::register_for_development(app);
            // The quick-capture shortcut's plugin, loaded here so that a failure to set it up does not end the
            // app (ADR-0040): the webview is granted none of its permissions, and the mock runtime of the tests
            // has no operating system's hotkey table, so it only loads it where a test asks.
            desktop::load_global_shortcut(app.handle());
            // The tray icon, and the worker that follows the settings of the close button and the quick-capture
            // shortcut (ADR-0040); it is told each time the core becomes ready, with the other background work.
            let desktop_starter = desktop::start(app.handle(), &desktop_state);
            // The background work runs while the core is ready: from the moment it opens (or is
            // unlocked) until it locks. The sync driver, the reminders' timer that ticks the
            // core's scheduler and shows an OS notification for each reminder that falls due
            // (it needs the app handle, which is why it is registered here), and the scheduled
            // backups' timer (ADR-0036).
            let weak_state = Arc::downgrade(&app_state);
            let host: Weak<dyn state::CoreHost> = weak_state;
            app_state.set_background(all_of(vec![
                desktop_starter,
                driver_starter(Arc::downgrade(&app_state)),
                reminders_starter(
                    Arc::downgrade(&app_state),
                    os_notifier(app.handle().clone()),
                    TICK_INTERVAL,
                ),
                // The OS notification of a reply, a mention or an assignment (ADR-0048): the page's title and a phrase
                // with the actor's name, never the words of a comment.
                notifications_starter(
                    Arc::downgrade(&app_state),
                    os_notification_notifier(app.handle().clone()),
                ),
                backup_scheduler::scheduler_starter(
                    host.clone(),
                    Arc::new(SystemClock),
                    Arc::clone(app.state::<Arc<backup_scheduler::Targets>>().inner()),
                    backup_scheduler::FIRST_RUN_DELAY,
                    backup_scheduler::CHECK_INTERVAL,
                ),
                // The semantic index's worker, which also cancels the AI streams when the core
                // locks, and the MCP server's loopback listener (ADR-0037).
                ai_indexer::indexer_starter(
                    host.clone(),
                    Arc::clone(app.state::<Arc<AiShell>>().inner()),
                    Arc::new(SystemClock),
                    ai_indexer::IndexerTiming::production(),
                ),
                mcp_host::mcp_starter(host, Arc::clone(app.state::<Arc<McpShell>>().inner())),
                // Ends the dictation session and the model installation when the core locks (ADR-0053).
                speech_host::speech_starter(Arc::clone(app.state::<Arc<SpeechShell>>().inner())),
            ]));
            // Plaintext copies made by "Open" last until the next start, never longer; nor do
            // the temporary files of an import's nested zips.
            match app.path().app_cache_dir() {
                Ok(cache_dir) => {
                    attachments::clear_open_dir(&cache_dir);
                    import_source::clear_import_dir(&cache_dir);
                }
                Err(error) => tracing::warn!(
                    %error,
                    "the app cache directory is unknown: temporary attachment copies are not cleared"
                ),
            }
            // The machine-local data directory, not `app_data_dir()`: on Windows the latter is
            // the roaming profile, which can be copied to other machines, while the key that
            // opens the database is stored with Local persistence and never leaves this one.
            // Debug builds may point elsewhere with `KB_DATA_DIR` (two instances side by
            // side); the keystore entries are named after the install id kept in that
            // directory, so the instances do not share keys either.
            let data_dir = match data_dir_choice {
                Some(dir) => dir,
                None => app.path().app_local_data_dir()?,
            };
            // The dictation models and the engine chosen on this device live under the same machine-local
            // directory (ADR-0053); the voice activity model is a resource of the installer.
            let speech = app.state::<Arc<SpeechShell>>();
            speech.set_data_dir(&data_dir);
            if let Ok(resources) = app.path().resource_dir() {
                speech.set_resource_dir(&resources);
            }
            // The staging files of the scheduled backups (ADR-0059) live under it too, and the timer
            // empties the staging folders each time the core becomes ready (just below).
            app.state::<Arc<backup_scheduler::Targets>>()
                .set_data_dir(&data_dir);
            // Google Drive (ADR-0059) finds the install id for its keystore entries, and the staging folder of a
            // download, in the same directory.
            app.state::<Arc<drive_host::DriveShell>>()
                .set_data_dir(&data_dir);
            open_core_in_background(app_state, data_dir);
            Ok(())
        })
        .run(context())
}

/// Unit tests of the shell's own logic. A test that builds a Tauri app, even on the mock
/// runtime, belongs in `tests/ipc.rs` instead: on Windows only integration tests get the
/// manifest that lets such an executable start (see `build.rs`).
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use kb_keys::KeystoreError;
    use kb_types::ErrorCode;

    use super::*;

    #[test]
    fn keystore_problems_are_reported_as_keystore_unavailable() {
        let unavailable =
            NativeError::Keystore(KeystoreError::Unavailable("no secret service".into()));
        let error = api_error_from_native(&unavailable);
        assert_eq!(error.code, ErrorCode::KeystoreUnavailable);
        assert!(
            error.message.contains("no secret service"),
            "{}",
            error.message
        );

        let other = NativeError::Keystore(KeystoreError::Other("access denied".into()));
        assert_eq!(
            api_error_from_native(&other).code,
            ErrorCode::KeystoreUnavailable
        );
    }

    #[test]
    fn data_directory_problems_are_internal_errors() {
        let error = api_error_from_native(&NativeError::DataDir("permission denied".into()));
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(
            error.message.contains("permission denied"),
            "{}",
            error.message
        );
    }

    #[test]
    fn the_platform_is_one_the_core_knows() {
        assert!(!PLATFORM.is_empty());
        if cfg!(target_os = "windows") {
            assert_eq!(PLATFORM, "windows");
        } else if cfg!(target_os = "macos") {
            assert_eq!(PLATFORM, "macos");
        } else if cfg!(target_os = "linux") {
            assert_eq!(PLATFORM, "linux");
        }
    }

    #[test]
    fn open_core_reports_an_unusable_data_directory() {
        let dir = std::env::temp_dir().join(format!("atomik-kb-shell-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the data directory should be.
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"x").unwrap();

        let error = open_core(&blocked).err().expect("opening must fail");
        assert_eq!(error.code, ErrorCode::Internal);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn kb_data_dir_replaces_the_data_directory_in_debug_builds_only() {
        let absolute = std::env::temp_dir().join("atomik-kb-second-instance");
        assert_eq!(
            data_dir_override(Some(absolute.clone().into_os_string()), true),
            Some(absolute.clone())
        );
        // Release builds ignore it, and nothing set means the default.
        assert_eq!(
            data_dir_override(Some(absolute.into_os_string()), false),
            None
        );
        assert_eq!(data_dir_override(None, true), None);
    }

    #[test]
    fn a_relative_or_empty_kb_data_dir_is_ignored() {
        for value in ["second-instance", "./data", ""] {
            assert_eq!(data_dir_override(Some(value.into()), true), None, "{value}");
        }
    }

    #[test]
    fn the_http_stack_never_logs_below_warn_whatever_the_filter_asks_for() {
        use crate::test_support::captured_logs_through;

        let emit = || {
            tracing::debug!(
                target: "hyper_util::client::legacy::connect::http",
                "connecting to HOSTSECRET"
            );
            tracing::trace!(target: "h2::codec::framed_write", "send frame HOSTSECRET");
            tracing::debug!(target: "hyper::proto::h1::io", "flushed HOSTSECRET");
            tracing::debug!(target: "reqwest::connect", "starting new connection: HOSTSECRET");
            tracing::debug!(target: "rustls::client::hs", "No cached session for HOSTSECRET");
            tracing::warn!(target: "hyper_util::client::legacy", "a warning is kept");
            tracing::debug!("the shell's own line");
        };
        for asked in [
            "debug",
            "trace",
            "debug,hyper_util=trace,h2=trace,hyper=trace,reqwest=trace,rustls=trace",
        ] {
            let logs = captured_logs_through(log_filter(EnvFilter::new(asked)), emit);
            assert!(!logs.contains("HOSTSECRET"), "{asked}: {logs}");
            assert!(logs.contains("a warning is kept"), "{asked}: {logs}");
            assert!(logs.contains("the shell's own line"), "{asked}: {logs}");
        }
        // Without the filter the same events do come through: the test means something.
        let logs = captured_logs_through(EnvFilter::new("trace"), emit);
        assert!(logs.contains("HOSTSECRET"), "{logs}");
    }

    #[test]
    fn the_deep_link_plugin_never_logs_the_argument_it_skips() {
        use crate::test_support::captured_logs_through;

        // A debug build of the plugin warns with the text of a launch argument that is a URL of another
        // scheme (a file path parses as one): a link is never logged (ADR-0040).
        let emit = || {
            tracing::warn!(
                target: "tauri_plugin_deep_link",
                "argument LINKSECRET does not match any configured deep link scheme; skipping it"
            );
            tracing::warn!("the shell's own warning is kept");
        };
        for asked in ["debug", "warn", "debug,tauri_plugin_deep_link=trace"] {
            let logs = captured_logs_through(log_filter(EnvFilter::new(asked)), emit);
            assert!(!logs.contains("LINKSECRET"), "{asked}: {logs}");
            assert!(logs.contains("the shell's own warning"), "{asked}: {logs}");
        }
        let logs = captured_logs_through(EnvFilter::new("debug"), emit);
        assert!(logs.contains("LINKSECRET"), "{logs}");
    }

    #[test]
    fn a_blocking_task_returns_its_result_untouched() {
        let ok = tauri::async_runtime::block_on(blocking(|| Ok(41 + 1), "unused"));
        assert_eq!(ok.unwrap(), 42);

        let failure = ApiError::not_found("no such page");
        let task_failure = failure.clone();
        let err =
            tauri::async_runtime::block_on(blocking(move || Err::<(), _>(task_failure), "unused"));
        assert_eq!(err.unwrap_err(), failure);
    }

    #[test]
    fn a_panicking_blocking_task_becomes_an_internal_error() {
        let result = tauri::async_runtime::block_on(blocking(
            || -> Result<(), ApiError> { unreachable!("this task panics on purpose") },
            CORE_STOPPED,
        ));
        assert_eq!(result.unwrap_err(), ApiError::internal(CORE_STOPPED));
    }

    #[test]
    fn an_empty_raw_response_is_an_empty_byte_body_not_json() {
        use tauri::ipc::{InvokeResponseBody, IpcResponse};

        let body = empty_response().body().unwrap();
        assert!(matches!(body, InvokeResponseBody::Raw(bytes) if bytes.is_empty()));
    }
}
