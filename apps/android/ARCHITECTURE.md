# atomik-kb for Android: architecture

Status: accepted 2026-11-15 (Ben's interview). This file is the decision record and the
contract between the three parts below. Change it first when a contract changes.

## Decisions

| # | Decision | Why |
|---|---|---|
| A1 | Kotlin + Jetpack Compose for everything except the page editor. | Native navigation, lists, search, settings, system integration (share sheet, biometrics, speech). |
| A2 | The page editor is the existing Tiptap/Loro `PageEditor`, bundled by `apps/mobile-editor` and hosted in a `WebView`. | One editor, identical document semantics on every platform; a native rewrite would diverge. |
| A3 | The Rust core runs on the phone in `crates/kb-mobile`, a UniFFI `cdylib`. Requests and events cross as the same serde JSON the desktop uses; page docs and attachments cross as bytes. | The core already has one `dispatch(Request) -> Response` door; a JSON door keeps the FFI tiny and stable. |
| A4 | `kb-mobile` owns the core behind a `Mutex`, a tokio runtime, and ports the desktop's sync/pairing/invite hosts (`apps/desktop/src-tauri/src/sync_host.rs`). Desktop is not refactored in v1 (follow-up: move both shells onto one shared host crate). | No regression risk to the desktop. |
| A5 | The phone joins the account as a new device by pairing code with the desktop, on Ben's kb-server. | Same flow as a second desktop; zero-knowledge server unchanged. |
| A6 | Root keys live in an Android `Keystore` adapter: values encrypted with a non-exportable AES-256-GCM key in `AndroidKeyStore`, ciphertext in app-private files. App backup and device transfer are disabled. | The core's `Keystore` trait; nothing secret may reach cloud backups. |
| A7 | App lock = the core's existing passphrase lock (`AppLockEnable`...). Biometric unlock stores that passphrase encrypted under a Keystore key that requires biometric auth (BiometricPrompt + CryptoObject). | Reuses core semantics; biometrics never replace the secret, they guard a copy of it. |
| A8 | Quick capture, share-to and dictation insert content through the editor (host messages), not new core requests. | The editor owns Loro block structure; no second writer. |
| A9 | Dictation uses Android `SpeechRecognizer`, on-device only by default (`createOnDeviceSpeechRecognizer`, API 33+, else `EXTRA_PREFER_OFFLINE`); online recognition is an opt-in setting with a privacy note. | Privacy-first. |
| A10 | Sync runs while the process lives; WorkManager runs a periodic sync (15 min, network required) and on app start. | Notes are current when the app opens. |

v1 scope: pair, sync, spaces + page tree, read/edit pages (all blocks render), create/rename/move to trash, favorites,
search, quick capture, share-to (text, links, images), dictation, app lock + biometric, settings (server, sync status,
devices list, lock). Later waves (own PRs): databases/views polish for touch, sharing + comments, attachments + camera,
history, templates, import/export, AI.

## Layout

```
crates/kb-mobile/            Rust FFI (agent A)
apps/mobile-editor/          Vite bundle of the editor for the WebView (agent B)
packages/core-client/src/android.ts   AndroidTransport (agent B)
apps/android/                Gradle project (agent C)
  settings.gradle.kts, gradle/libs.versions.toml
  app/                       Compose app, Hilt, features by package
  engine/                    Android library: UniFFI Kotlin bindings + jniLibs + Engine wrapper
```

## Contract 1: Rust FFI (`crates/kb-mobile`, namespace `kb_mobile`)

UniFFI proc-macros (no UDL). The crate sets `#![allow(unsafe_code)]` at crate level only (workspace forbids it).
`crate-type = ["cdylib", "lib"]`. Kotlin package of the generated bindings: `io.atomik.kb.engine.ffi`.

```rust
#[uniffi::export(with_foreign)]
pub trait PlatformKeystore: Send + Sync {
    fn get(&self, name: String) -> Result<Option<Vec<u8>>, KeystoreFfiError>;
    fn set(&self, name: String, value: Vec<u8>) -> Result<(), KeystoreFfiError>;
    fn delete(&self, name: String) -> Result<(), KeystoreFfiError>;
}
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KeystoreFfiError { #[error("unavailable: {0}")] Unavailable(String), #[error("failed: {0}")] Failed(String) }

#[uniffi::export(with_foreign)]
pub trait EventSink: Send + Sync {
    /// One kb_types::Event as JSON ({"type":..,"data":..}), in emission order.
    fn on_event(&self, event_json: String);
    /// Engine state changed (opening -> ready/locked/failed, lock, unlock).
    fn on_state(&self, state: EngineState);
}

#[derive(uniffi::Record)] pub struct EngineConfig { pub data_dir: String, pub platform: String /* "android" */ }
#[derive(uniffi::Enum)] pub enum EngineState { Opening, Ready, Locked, Failed { message: String } }
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EngineError { #[error("not ready")] NotReady, #[error("locked")] Locked, #[error("{json}")] Api { json: String } /* ApiError JSON */, #[error("{message}")] Internal { message: String } }

#[derive(uniffi::Object)] pub struct KbEngine { .. }
#[uniffi::export]
impl KbEngine {
    /// Returns at once; opens the core on a background thread and reports through `sink.on_state`.
    #[uniffi::constructor] pub fn new(config: EngineConfig, keystore: Arc<dyn PlatformKeystore>, sink: Arc<dyn EventSink>) -> Arc<Self>;
    pub fn state(&self) -> EngineState;
    /// Request JSON in (kb_types::Request), {"ok": Response} or {"err": ApiError} out. Never panics across FFI.
    pub fn dispatch(&self, request_json: String) -> String;
    pub fn unlock(&self, passphrase: String) -> Result<(), EngineError>;
    pub fn lock(&self);
    pub fn doc_open(&self, page_id: String) -> Result<Vec<u8>, EngineError>;
    pub fn doc_import(&self, page_id: String, update: Vec<u8>) -> Result<(), EngineError>;
    pub fn doc_export(&self, page_id: String, since_version: Vec<u8>) -> Result<Vec<u8>, EngineError>;
    pub fn doc_close(&self, page_id: String);
    pub fn attachment_read(&self, attachment_id: String, start: Option<u64>, end: Option<u64>) -> Result<Vec<u8>, EngineError>;
    pub fn attachment_write(&self, attachment_id: String, bytes: Vec<u8>) -> Result<(), EngineError>;
    /// App foreground/background: keeps realtime presence only in foreground; sync keeps running.
    pub fn set_foreground(&self, foreground: bool);
    /// Asks for a sync round now and waits (blocking, call off the main thread) until idle or timeout. Returns true if idle.
    pub fn sync_once(&self, timeout_ms: u64) -> bool;
}
```

Semantics of `doc_*`, `attachment_*`, `unlock`, `lock` mirror the desktop shell exactly
(`doc_ipc.rs`, `attachment_ipc.rs`, `state.rs`); events are drained after every core call and
delivered to the sink outside the core lock, in order. Sync/pairing/invite tasks start when the
core becomes Ready and stop on lock (as `sync_host.rs::driver_starter`). Panics are caught and
turned into `Internal`.

Build: `cargo ndk -t arm64-v8a -t x86_64 --platform 28 -o apps/android/engine/src/main/jniLibs build -p kb-mobile [--release]`;
bindings: `cargo run -p kb-mobile --bin uniffi-bindgen generate --library <host .dll/.so> --language kotlin --out-dir apps/android/engine/build/generated/uniffi`
(wired as Gradle tasks in `engine/build.gradle.kts`; jniLibs and generated code are not committed).

## Contract 2: WebView bridge (JS <-> Kotlin)

The editor bundle is served from APK assets by `WebViewAssetLoader` at
`https://appassets.androidplatform.net/editor/index.html`. Kotlin registers
`WebViewCompat.addWebMessageListener(webView, "kbHost", setOf("https://appassets.androidplatform.net"), ..)`;
JS gets `window.kbHost` with `postMessage(string)` and `onmessage`. All messages are JSON strings; byte payloads are base64 (`b64`).

JS -> Kotlin, calls (Kotlin answers each with a reply carrying the same `id`):
```
{"id":n,"op":"dispatch","request":<Request>}
{"id":n,"op":"docOpen","pageId":s}                      -> {"b64":..}
{"id":n,"op":"docImport","pageId":s,"b64":..}
{"id":n,"op":"docExport","pageId":s,"b64":<version>}    -> {"b64":..}
{"id":n,"op":"docClose","pageId":s}
{"id":n,"op":"attRead","attachmentId":s,"start"?:n,"end"?:n} -> {"b64":..}
{"id":n,"op":"attWrite","attachmentId":s,"b64":..}
```
Reply: `{"id":n,"ok":<value or null>}` or `{"id":n,"err":<ApiError JSON>}`. For `dispatch`, `ok` is the Response.

JS -> Kotlin, notifications (no reply): `{"host":"ready"}` (bundle loaded, waiting for init),
`{"host":"opened","pageId":s,"title":s}`, `{"host":"titleChanged","title":s}`,
`{"host":"openPage","pageId":s}` (internal link tap), `{"host":"openExternal","url":s}`,
`{"host":"dictate"}` (toolbar mic tapped), `{"host":"error","message":s}`.

Kotlin -> JS: `{"event":<Event>}` (every core event while the page is open);
`{"init":{"pageId":s,"spaceId":s,"theme":"light"|"dark","locale":s,"readOnly":bool,"focus":"title"|"end"|null}}`;
`{"insert":{"text":s}}` (at the caret, or appended as a new paragraph when there is no selection);
`{"insert":{"attachmentId":s,"name":s,"mime":s}}` (image/file block after the caret);
`{"theme":"light"|"dark"}`; `{"dictation":{"state":"listening"|"idle","partial"?:s}}`.

The editor UI in the WebView: no sidebar or tabs; title + body; a formatting toolbar pinned above the keyboard
(bold, italic, list, checklist, heading, `/` menu, mic, undo/redo), tracking `visualViewport`; touch targets >= 44 px;
block drag handles replaced by long-press menu; respects safe-area insets and `prefers-color-scheme`.

## Contract 3: Kotlin engine API (`apps/android/engine`, package `io.atomik.kb.engine`)

```kotlin
interface Engine {
    val state: StateFlow<EngineState>          // Opening, Ready, Locked, Failed(message)
    val events: SharedFlow<CoreEvent>          // CoreEvent(type: String, data: JsonElement, raw: String)
    suspend fun dispatchRaw(requestJson: String): String
    suspend fun <T> dispatch(request: CoreRequest, response: KSerializer<T>): T   // throws CoreException(ApiError)
    suspend fun unlock(passphrase: String); fun lock()
    suspend fun docOpen(pageId: String): ByteArray; suspend fun docImport(pageId: String, update: ByteArray)
    suspend fun docExport(pageId: String, since: ByteArray): ByteArray; fun docClose(pageId: String)
    suspend fun attachmentRead(id: String, start: Long? = null, end: Long? = null): ByteArray
    suspend fun attachmentWrite(id: String, bytes: ByteArray)
    fun setForeground(foreground: Boolean); suspend fun syncOnce(timeoutMs: Long): Boolean
}
```
`RustEngine` implements it over `KbEngine` on a single-thread IO dispatcher; `FakeEngine` (in-memory) backs UI tests and
previews. `CoreRequest`/response models (kotlinx.serialization) cover the v1 subset only and must serialize exactly like
`kb_types` serde (check against `crates/kb-types/src/api.rs`; `ignoreUnknownKeys = true`).

## App (agent C)

- minSdk 28, target/compile 36, Kotlin 2.x, AGP current stable, version catalog, KSP, Hilt, Navigation Compose (type-safe routes),
  Material 3 + dynamic color, edge-to-edge, predictive back, `material3-adaptive` list-detail on wide screens.
- MVVM + unidirectional data flow: `ViewModel` exposes one immutable `UiState` `StateFlow`; repositories
  (`SpacesRepository`, `PagesRepository`, `SearchRepository`, `SyncRepository`, `SecurityRepository`) wrap `Engine` and refresh on events.
- Screens: Onboarding (welcome -> server URL -> pairing code + 6-digit SAS confirm, or "start fresh" workspace), Home (space switcher,
  favorites, page tree with expand/collapse, FAB = new page), Page (top bar + WebView editor, overflow: favorite, move to trash),
  Search (live, titles+bodies, snippets with highlights), Capture (new page in the capture space, opens editor focused), Share target
  (`ACTION_SEND`/`SEND_MULTIPLE` text and images -> choose space -> new page with content), Lock (passphrase + biometric), Settings
  (sync server + status + sync now, devices, app lock + biometric toggle + auto-lock delay, online dictation opt-in, about).
- Manifest: `allowBackup=false`, `dataExtractionRules` excluding everything, no cleartext except debug builds to `10.0.2.2`/`localhost`,
  `FLAG_SECURE` on Lock screen.
- Strings in `res/values` (en) + `values-fr` + `values-es` (desktop languages).
