//! The dictation commands of the shell (ADR-0053, `docs/architecture/dictation.md` sections 3 and 4): the status
//! of the engines, the engine chosen on this device, the model store and its downloads, and the sessions that
//! run the `kb-speech` sidecar. The commands in `lib.rs` are thin; the rules are here. The pure rules (the
//! catalog, the frame protocol, the phrase joining) are in `kb-speech`; the recognizer is in the sidecar
//! (`adapters/kb-speech-native`), a separate process the shell starts for one session.
//!
//! # As built
//!
//! The module is four files: this one (the shell's state, the commands and their checks), `session` (the sidecar
//! process, its three threads and the task that turns its messages into events), `models` (downloads, imports,
//! removal) and `process` (finding and starting the sidecar, the seam the tests replace).
//!
//! - **Locked or starting core**: every command except `dictation_stop` and `dictation_cancel` answers what
//!   `CoreHost::ensure_ready` answers (`Locked`, or "core is starting"). A lock ends the running session
//!   (`Ended`, the process killed) and the installation (`InstallEvent::Error` with `cancelled`)
//!   ([`SpeechShell::cancel_all`], through [`speech_starter`]).
//! - **One session at a time** (`InvalidInput` for `sessionId`, `details.reason = "busy"`) and one installation
//!   (or import) at a time (`InvalidInput` for `engine`, reason `busy`). A model is not removed or imported while
//!   a session of its engine or an installation of it runs (`busy`). A session id is 1 to 64 of `[A-Za-z0-9_-]`.
//! - **Errors**: a command that starts nothing answers `ApiError` `InvalidInput` whose `details.reason` is the
//!   [`DictationErrorCode::as_str`] name; what fails after a start is an event with the same code
//!   (`DictationEvent::Error`, `InstallEvent::Error`).
//! - **Sessions** (`start`, `audio`, `stop`, `cancel`): the checks run in the contract's order (core, id, busy,
//!   `modelMissing`, `engineMissing` including the model's SHA-256, `ollamaUnavailable`); then the sidecar is
//!   spawned (`CREATE_NO_WINDOW` on Windows) and the first events are `State(Loading)`... Audio goes to a writer
//!   thread that owns stdin (never waits for the sidecar; 60 s queued at most, over that `engineFailed`); a reader
//!   thread owns stdout; the stderr of the process is counted and dropped, except its own fixed
//!   `kb-speech: ended (...)` line. The sidecar's `Error` other than `recognizer_failed` is `engineFailed`; a
//!   process that ends on its own after `Ready` is `engineCrashed`, **before** `Ready` it is `engineFailed` (a
//!   damaged model file aborts the native library while it loads); no `Ready` within 60 s is `engineFailed`. A stop
//!   queues `Flush` behind the audio, waits for `Flushed` and the Ollama requests (20 s at most), then sends `Ended`
//!   and `Quit`; the process is killed 2 s later if it is still there. The session leaves the slot before `Ended` is
//!   sent. A webview channel that cannot be reached ends the session like a cancel.
//! - **The Ollama engine** runs the sidecar `VadOnly` and sends each `SegmentAudio` through
//!   `ai_host::transcribe` (one request at a time, in order). At most 8 segments wait; a person who talks faster than
//!   Ollama answers ends the session with `ollamaUnavailable`, and so do three failed requests in a row.
//! - **Installs**: the native consent dialog (`shell_labels`: `dialogDictationTitle`, `dialogDictationMessage` with
//!   `{model}`, `{size}` and `{host}`, `dialogDownload`), then `models::download_model`. A short body is
//!   `downloadFailed`; a body longer than the pinned size, a server that announces another size, or a wrong
//!   SHA-256 is `hashMismatch`. A failed or cancelled installation leaves no `.part` file. `dictation_install` of
//!   an installed model answers `Done` without a dialog. The SHA-256 is computed while the bytes arrive, so
//!   `Verifying` is the renaming and the marker.
//! - **Imports** take the same slot as installs, check every file's size, then copy and hash it in one pass
//!   (`models::import_folder`); a cancelled dialog is `InvalidInput` for `path`.
//! - **The engine chosen on this device** lives in `<app data>/speech/selection.json`, not in the settings
//!   table: a file the shell alone writes can be neither synced, nor backed up, nor set by a restore, nor by the
//!   webview's `SettingsSet` (see [`write_selection`]).
//! - **Model store**: `<app data>/speech/<model id>/` (the machine-local data directory, `KB_DATA_DIR` in debug
//!   builds). A model is installed when its `installed.json` names the pinned commit and every file has the
//!   pinned size ([`is_installed`]); it was hashed before the marker was written.
//! - **Privacy**: audio is held in memory only and dropped right after recognition; nothing the person said
//!   is logged (kind of request, engine, outcome, sizes and durations only); the model host is named in the
//!   consent dialog and contacted only after "Download"; the log never holds a host, a URL or a path.
//! - **Tests**: the pure helpers, the session and installation state machines against an in-process fake sidecar and
//!   an `axum` stub (`tests.rs`), and the commands through Tauri's mock runtime (`tests/speech_ipc.rs`).

mod models;
mod process;
mod session;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use kb_speech::api::{
    DictationErrorCode, DictationEvent, DictationStatus, Facts, InstallEvent, build_status,
};
use kb_speech::catalog::{ModelSpec, SpeechEngine, VAD_FILE};
use kb_speech::policy::MAX_PUSH_BYTES;
use kb_speech::protocol::bytes_to_pcm;
use kb_types::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::http::HeaderMap;
use tauri::{AppHandle, Runtime};
use tokio::sync::watch;

use crate::ai_host::{AiShell, allowed_url, core, read_ai_settings, read_endpoint};
use crate::blocking;
use crate::dialogs::{self, Confirm, Question};
use crate::shell_labels::{Labels, fill};
use crate::state::{BackgroundStarter, BackgroundTask, CoreHost};

pub use models::{
    DownloadPolicy, PROGRESS_PER_SECOND, Throttle, allowed_redirect, clean_partials,
    commit_download, download_model, import_folder, remove_model,
};
pub use process::{Launcher, Located, ProcessLauncher, Sidecar, SidecarProcess, sha256_of_file};
pub use session::{
    Driver, LOAD_TIMEOUT, MAX_QUEUED_SAMPLES, MAX_TRANSCRIBE_FAILURES, MAX_TRANSCRIBE_QUEUE,
    Ollama, QUIT_GRACE, SessionHandle, Timeouts,
};

/// The request header of `dictation_audio`: the session id.
pub const SESSION_HEADER: &str = "x-kb-dictation";

/// The folder of the model store under the machine-local data directory.
pub const STORE_DIR: &str = "speech";

/// The file holding the engine chosen on this device, in the store.
pub const SELECTION_FILE: &str = "selection.json";

/// The file that says a model folder is complete, inside it.
pub const INSTALLED_FILE: &str = "installed.json";

/// What a caller is told when the thread running a dictation command panicked.
const SPEECH_STOPPED: &str = "the dictation request could not be completed";

/// What a stream of events is sent through; `false` means the receiver is gone.
pub type Emit<'a, E> = &'a (dyn Fn(E) -> bool + Send + Sync);

// --- the shell's dictation state ---

/// The installation (or import) that runs.
struct InstallSlot {
    engine: SpeechEngine,
    /// Tells the installation to stop (`true`).
    cancel: watch::Sender<bool>,
    token: u64,
}

/// The running session and installation.
#[derive(Default)]
struct Slots {
    /// The session that runs, if any.
    session: Option<SessionHandle>,
    /// The installation that runs, if any.
    install: Option<InstallSlot>,
}

/// An installation's place in the shell; dropping it frees the place.
pub struct InstallGuard {
    shell: Arc<SpeechShell>,
    token: u64,
}

impl InstallGuard {
    /// Asks the installation to stop (the receiver of its events is gone).
    pub fn cancel(&self) {
        self.shell.cancel_install();
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        let mut slots = self.shell.lock_slots();
        if slots
            .install
            .as_ref()
            .is_some_and(|slot| slot.token == self.token)
        {
            slots.install = None;
        }
    }
}

/// What the dictation commands share: where the store is and what runs. Managed by Tauri inside an `Arc`;
/// the background starter ([`speech_starter`]) holds the same one.
pub struct SpeechShell {
    data_dir: OnceLock<PathBuf>,
    resource_dir: OnceLock<PathBuf>,
    slots: Mutex<Slots>,
    launcher: Mutex<Arc<dyn Launcher>>,
    downloads: Mutex<DownloadPolicy>,
    timeouts: Mutex<Timeouts>,
    tokens: AtomicU64,
}

impl std::fmt::Debug for SpeechShell {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpeechShell")
            .finish_non_exhaustive()
    }
}

impl Default for SpeechShell {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeechShell {
    /// A shell with nothing running and no store yet (`run` sets the directories at start-up), that starts the
    /// real sidecar and downloads from the catalog.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data_dir: OnceLock::new(),
            resource_dir: OnceLock::new(),
            slots: Mutex::new(Slots::default()),
            launcher: Mutex::new(Arc::new(ProcessLauncher)),
            downloads: Mutex::new(DownloadPolicy::production()),
            timeouts: Mutex::new(Timeouts::PRODUCTION),
            tokens: AtomicU64::new(1),
        }
    }

    fn lock_slots(&self) -> MutexGuard<'_, Slots> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn next_token(&self) -> u64 {
        self.tokens.fetch_add(1, Ordering::SeqCst)
    }

    /// Tells the shell the machine-local data directory (the store is `<dir>/speech`). Once.
    pub fn set_data_dir(&self, dir: &Path) {
        let _ = self.data_dir.set(dir.to_path_buf());
    }

    /// Tells the shell the app's resource directory (where the installer puts `silero_vad.onnx`). Once.
    pub fn set_resource_dir(&self, dir: &Path) {
        let _ = self.resource_dir.set(dir.to_path_buf());
    }

    /// Replaces what finds and starts the sidecar. For the tests: the app never calls this.
    pub fn set_launcher(&self, launcher: Arc<dyn Launcher>) {
        *self.launcher.lock().unwrap_or_else(PoisonError::into_inner) = launcher;
    }

    /// Replaces how models are downloaded. For the tests: the app never calls this.
    pub fn set_download_policy(&self, policy: DownloadPolicy) {
        *self
            .downloads
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = policy;
    }

    /// Replaces the waits of a session (loading, flushing, quitting). For the tests: the app never calls this.
    pub fn set_timeouts(&self, timeouts: Timeouts) {
        *self.timeouts.lock().unwrap_or_else(PoisonError::into_inner) = timeouts;
    }

    fn timeouts(&self) -> Timeouts {
        *self.timeouts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn launcher(&self) -> Arc<dyn Launcher> {
        Arc::clone(&self.launcher.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn download_policy(&self) -> DownloadPolicy {
        self.downloads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The model store, `<app data>/speech`.
    ///
    /// # Errors
    ///
    /// `Internal` before [`SpeechShell::set_data_dir`] (a test app that has no data directory).
    pub fn store_root(&self) -> Result<PathBuf, ApiError> {
        self.data_dir
            .get()
            .map(|dir| dir.join(STORE_DIR))
            .ok_or_else(|| ApiError::internal("the speech store is not set up"))
    }

    /// The sidecar executable next to the app's own, when it is there.
    #[must_use]
    pub fn sidecar(&self) -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let path = sidecar_path(exe.parent()?);
        path.is_file().then_some(path)
    }

    /// The voice activity model, when one of the places it may be holds the right file.
    #[must_use]
    pub fn vad(&self) -> Option<PathBuf> {
        let exe = std::env::current_exe().ok();
        let candidates = vad_candidates(
            self.resource_dir.get().map(PathBuf::as_path),
            exe.as_deref().and_then(Path::parent),
            std::env::var_os(VAD_ENV),
            cfg!(debug_assertions),
        );
        locate_vad(&candidates)
    }

    /// Whether a session runs.
    #[must_use]
    pub fn session_running(&self) -> bool {
        self.lock_slots().session.is_some()
    }

    /// Whether an installation or import runs.
    #[must_use]
    pub fn install_running(&self) -> bool {
        self.lock_slots().install.is_some()
    }

    /// The running session `id`, when that is the one that runs.
    fn session(&self, id: &str) -> Option<SessionHandle> {
        self.lock_slots()
            .session
            .as_ref()
            .filter(|session| session.id == id)
            .cloned()
    }

    /// Puts a started session in the slot.
    fn register_session(&self, handle: SessionHandle) -> Result<(), ApiError> {
        let mut slots = self.lock_slots();
        if slots.session.is_some() {
            return Err(busy("sessionId", "another dictation session is running"));
        }
        slots.session = Some(handle);
        Ok(())
    }

    /// Frees the slot of the session with this token (a later session keeps its own).
    fn release_session(&self, token: u64) {
        let mut slots = self.lock_slots();
        if slots
            .session
            .as_ref()
            .is_some_and(|session| session.token == token)
        {
            slots.session = None;
        }
    }

    /// Takes the installation slot for `engine`.
    fn begin_install(
        self: &Arc<Self>,
        engine: SpeechEngine,
    ) -> Result<(InstallGuard, watch::Receiver<bool>), ApiError> {
        let mut slots = self.lock_slots();
        if slots.install.is_some() {
            return Err(busy("engine", "another model is being installed"));
        }
        let (cancel, receiver) = watch::channel(false);
        let token = self.next_token();
        slots.install = Some(InstallSlot {
            engine,
            cancel,
            token,
        });
        Ok((
            InstallGuard {
                shell: Arc::clone(self),
                token,
            },
            receiver,
        ))
    }

    /// Cancels the installation that runs; nothing running is fine.
    fn cancel_install(&self) {
        if let Some(slot) = self.lock_slots().install.as_ref() {
            slot.cancel.send_replace(true);
        }
    }

    /// `busy` for `engine` while a session of it or an installation of it runs.
    fn refuse_while_in_use(&self, engine: SpeechEngine) -> Result<(), ApiError> {
        let slots = self.lock_slots();
        let in_use = slots
            .session
            .as_ref()
            .is_some_and(|session| session.engine == engine)
            || slots
                .install
                .as_ref()
                .is_some_and(|slot| slot.engine == engine);
        if in_use {
            Err(busy(
                "engine",
                "the model is in use by a dictation session or an installation",
            ))
        } else {
            Ok(())
        }
    }

    /// Ends the running session and installation (the core locked): the session ends with `Ended` and its process
    /// is killed, the installation ends with `Error { cancelled }`.
    pub fn cancel_all(&self) {
        let session = {
            let mut slots = self.lock_slots();
            if let Some(slot) = slots.install.as_ref() {
                slot.cancel.send_replace(true);
            }
            slots.session.take()
        };
        if let Some(session) = session {
            session.cancel();
        }
    }
}

/// The background work `lib.rs` registers with the others: it does nothing while the core is ready and ends the
/// session and the installation when it locks (the task is dropped then).
pub fn speech_starter(shell: Arc<SpeechShell>) -> BackgroundStarter {
    Box::new(move || {
        let task: BackgroundTask = Box::new(SpeechTask {
            shell: Arc::clone(&shell),
        });
        Some(task)
    })
}

struct SpeechTask {
    shell: Arc<SpeechShell>,
}

impl Drop for SpeechTask {
    fn drop(&mut self) {
        self.shell.cancel_all();
    }
}

// --- pure helpers ---

/// The debug-build variable that names the voice activity model (an absolute path; `scripts/dev-env.ps1`
/// sets it when `scripts/fetch-speech-deps.ps1` has run).
pub const VAD_ENV: &str = "KB_SPEECH_VAD";

/// The sidecar's file name in the directory of the app's executable.
#[must_use]
pub fn sidecar_path(exe_dir: &Path) -> PathBuf {
    exe_dir.join(if cfg!(windows) {
        "kb-speech.exe"
    } else {
        "kb-speech"
    })
}

/// Where the voice activity model may be, best first: the debug variable (debug builds, absolute paths only,
/// the way `KB_DATA_DIR` is taken), the resource directory, `resources/` under it, then next to the executable.
#[must_use]
pub fn vad_candidates(
    resource_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    env: Option<OsString>,
    debug_build: bool,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if debug_build
        && let Some(path) = env.map(PathBuf::from)
        && path.is_absolute()
    {
        candidates.push(path);
    }
    if let Some(dir) = resource_dir {
        candidates.push(dir.join(VAD_FILE.name));
        candidates.push(dir.join("resources").join(VAD_FILE.name));
    }
    if let Some(dir) = exe_dir {
        candidates.push(dir.join(VAD_FILE.name));
    }
    candidates
}

/// The first candidate that is a file of the voice activity model's size. (Its SHA-256 is checked when a
/// session starts: the file is 644 KB.)
#[must_use]
pub fn locate_vad(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|path| {
            std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == VAD_FILE.bytes)
        })
        .cloned()
}

/// The folder of a model in the store.
#[must_use]
pub fn model_dir(store_root: &Path, spec: &ModelSpec) -> PathBuf {
    store_root.join(spec.id)
}

/// What `installed.json` holds.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct InstalledMarker {
    id: String,
    commit: String,
}

/// Whether a model folder is complete: its marker names this model at the pinned commit and every file has
/// the pinned size. (A file that was altered on disk is not noticed here; the hashes were checked before the
/// marker was written.)
#[must_use]
pub fn is_installed(dir: &Path, spec: &ModelSpec) -> bool {
    let marker = std::fs::read(dir.join(INSTALLED_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<InstalledMarker>(&bytes).ok());
    marker.is_some_and(|marker| marker.id == spec.id && marker.commit == spec.commit)
        && spec.files.iter().all(|file| {
            std::fs::metadata(dir.join(file.name))
                .is_ok_and(|meta| meta.is_file() && meta.len() == file.bytes)
        })
}

/// Writes the marker of a model folder whose files were all checked against their SHA-256.
///
/// # Errors
///
/// `InvalidInput` for `engine`, reason `storeFailed`, when the file cannot be written.
pub fn write_installed_marker(dir: &Path, spec: &ModelSpec) -> Result<(), ApiError> {
    let marker = InstalledMarker {
        id: spec.id.to_owned(),
        commit: spec.commit.to_owned(),
    };
    let bytes = serde_json::to_vec(&marker).map_err(|_| store_failed())?;
    write_atomically(&dir.join(INSTALLED_FILE), &bytes)
}

/// What `selection.json` holds.
#[derive(Debug, Serialize, Deserialize)]
struct Selection {
    engine: SpeechEngine,
}

/// The engine chosen on this device; `None` before the first choice or when the file is not readable.
#[must_use]
pub fn read_selection(store_root: &Path) -> Option<SpeechEngine> {
    let bytes = std::fs::read(store_root.join(SELECTION_FILE)).ok()?;
    serde_json::from_slice::<Selection>(&bytes)
        .ok()
        .map(|selection| selection.engine)
}

/// Stores the engine chosen on this device in `selection.json`.
///
/// This is the device-local setting of ADR-0053. It is a file of the speech store and not a row of the
/// settings table on purpose: the table is what a backup file carries, what a restore writes and what the
/// webview's `SettingsSet` reaches, and none of those can touch a file the shell alone writes. The choice
/// belongs to the models on this machine, so it travels as little as they do.
///
/// # Errors
///
/// `InvalidInput` for `engine`, reason `storeFailed`, when the file cannot be written.
pub fn write_selection(store_root: &Path, engine: SpeechEngine) -> Result<(), ApiError> {
    let bytes = serde_json::to_vec(&Selection { engine }).map_err(|_| store_failed())?;
    write_atomically(&store_root.join(SELECTION_FILE), &bytes)
}

/// Writes `bytes` to `path` through a temporary file in the same folder, creating the folder.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), ApiError> {
    let parent = path.parent().ok_or_else(store_failed)?;
    std::fs::create_dir_all(parent).map_err(|_| store_failed())?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(|_| store_failed())?;
    std::fs::rename(&temporary, path).map_err(|_| {
        let _ = std::fs::remove_file(&temporary);
        store_failed()
    })
}

/// `InvalidInput` for `field` with `details.reason` the code's name.
#[must_use]
pub fn dictation_error(field: &str, code: DictationErrorCode, message: &str) -> ApiError {
    ApiError::invalid(field, message)
        .with_details(json!({ "field": field, "reason": code.as_str() }))
}

fn store_failed() -> ApiError {
    dictation_error(
        "engine",
        DictationErrorCode::StoreFailed,
        "the speech model store could not be written",
    )
}

/// `InvalidInput` for `field` with the reason `busy`.
fn busy(field: &str, message: &str) -> ApiError {
    dictation_error(field, DictationErrorCode::Busy, message)
}

/// `InvalidInput` for `engine` with the reason `modelMissing`: the engine has no model to download or to start.
fn no_model(message: &str) -> ApiError {
    dictation_error("engine", DictationErrorCode::ModelMissing, message)
}

/// A session id is 1 to 64 of `[A-Za-z0-9_-]`.
///
/// # Errors
///
/// `InvalidInput` for `sessionId`.
pub fn validate_session_id(id: &str) -> Result<(), ApiError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(())
    } else {
        Err(ApiError::invalid(
            "sessionId",
            "a session id is 1 to 64 letters, digits, dashes or underscores",
        ))
    }
}

/// The session id of a `dictation_audio` request: the header [`SESSION_HEADER`].
///
/// # Errors
///
/// `InvalidInput` for `sessionId` when the header is missing, is not text or is not a valid id.
pub fn session_from_header(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::invalid("sessionId", "the x-kb-dictation header is missing"))?;
    validate_session_id(value)?;
    Ok(value.to_owned())
}

/// The samples of a `dictation_audio` body: 16 kHz mono `i16` little-endian, at most
/// [`MAX_PUSH_BYTES`] (2 s), and not empty.
///
/// # Errors
///
/// `InvalidInput` for `audio`.
pub fn pcm_from_body(bytes: &[u8]) -> Result<Vec<i16>, ApiError> {
    if bytes.is_empty() || bytes.len() > MAX_PUSH_BYTES {
        return Err(ApiError::invalid(
            "audio",
            "an audio push is 1 byte to 2 seconds of 16 kHz 16-bit samples",
        ));
    }
    bytes_to_pcm(bytes)
        .map_err(|_| ApiError::invalid("audio", "an audio push holds whole 16-bit samples"))
}

/// The size of a download as the consent dialog says it: megabytes (decimal), rounded up.
#[must_use]
pub fn size_text(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1_000_000))
}

/// The question the person answers before a model is downloaded, in the words of `labels`.
#[must_use]
pub fn consent_question(labels: &Labels, spec: &ModelSpec) -> Question {
    Question {
        title: labels.dialog_dictation_title.clone(),
        message: fill(
            &labels.dialog_dictation_message,
            &[
                ("model", spec.name),
                ("size", &size_text(spec.total_bytes())),
                ("host", ModelSpec::HOST),
            ],
        ),
        accept: labels.dialog_download.clone(),
        cancel: labels.dialog_cancel.clone(),
    }
}

/// What the commands need to know about this machine and its files to describe the engines.
fn gather_facts(shell: &SpeechShell, store_root: &Path) -> Facts {
    let installed = |spec: &ModelSpec| is_installed(&model_dir(store_root, spec), spec);
    Facts {
        engine_present: shell.sidecar().is_some() && shell.vad().is_some(),
        accurate_installed: SpeechEngine::Accurate.model().is_some_and(installed),
        light_installed: SpeechEngine::Light.model().is_some_and(installed),
        // The AI facts are filled in by `status`, which reads them from the core.
        ai_enabled: false,
        transcription_model_set: false,
        endpoint_allowed: false,
    }
}

// --- the commands ---

/// The engines as Settings shows them (`dictation_status`): the engine chosen on this device, and for each
/// engine whether it is installed and available, its size, languages and attribution. Needs the core ready
/// (it reads the AI settings for the Ollama engine).
///
/// # Errors
///
/// `Locked`, or "core is starting", while the core is not ready; `Internal` before the store is set up.
pub async fn status(
    host: Arc<dyn CoreHost>,
    shell: Arc<SpeechShell>,
) -> Result<DictationStatus, ApiError> {
    let (enabled, model_set, allowed) = core(&host, |host| {
        host.ensure_ready()?;
        let settings = read_ai_settings(host)?;
        let allowed = allowed_url(&read_endpoint(host)?).is_ok();
        Ok((
            settings.enabled,
            settings
                .transcription_model
                .is_some_and(|model| !model.trim().is_empty()),
            allowed,
        ))
    })
    .await?;
    let root = shell.store_root()?;
    blocking(
        move || {
            let facts = Facts {
                ai_enabled: enabled,
                transcription_model_set: model_set,
                endpoint_allowed: allowed,
                ..gather_facts(&shell, &root)
            };
            Ok(build_status(read_selection(&root), &facts))
        },
        SPEECH_STOPPED,
    )
    .await
}

/// Chooses the engine of this device (`dictation_select`) and answers the new status. Any engine may be
/// chosen, installed or not: the first-use dialog chooses and then downloads.
///
/// # Errors
///
/// As [`status`], and `InvalidInput` (`storeFailed`) when the choice cannot be stored.
pub async fn select(
    host: Arc<dyn CoreHost>,
    shell: Arc<SpeechShell>,
    engine: SpeechEngine,
) -> Result<DictationStatus, ApiError> {
    core(&host, |host| host.ensure_ready()).await?;
    let root = shell.store_root()?;
    blocking(move || write_selection(&root, engine), SPEECH_STOPPED).await?;
    status(host, shell).await
}

/// Downloads and installs the model of `engine` after a native consent dialog (`dictation_install`): the dialog
/// names the model, its size and the host; "Download" streams every file (see `models`), sends
/// `InstallEvent::Progress` at most ten times a second, then `Verifying`, renames the files into
/// `<store>/<model id>/`, writes the marker and sends `Done`. A failure or a cancel is an `InstallEvent::Error`
/// and nothing partial is kept; the command answers `Ok` once the installation ended. An installed model is `Done`
/// at once, with no dialog. One installation at a time; `install_cancel` and a lock cancel it.
///
/// # Errors
///
/// Only when nothing started: the core is not ready, the engine has no download (`modelMissing`), another
/// installation runs (`busy`), the person declined (`consentDeclined`).
pub async fn install(
    host: Arc<dyn CoreHost>,
    shell: Arc<SpeechShell>,
    confirm: Arc<dyn Confirm>,
    engine: SpeechEngine,
    emit: Emit<'_, InstallEvent>,
) -> Result<(), ApiError> {
    core(&host, |host| host.ensure_ready()).await?;
    let spec = engine
        .model()
        .ok_or_else(|| no_model("this engine has no model to download"))?;
    install_model(&shell, confirm, spec, emit).await
}

/// [`install`] for a model: the catalog's, or a test's.
pub(crate) async fn install_model(
    shell: &Arc<SpeechShell>,
    confirm: Arc<dyn Confirm>,
    spec: &'static ModelSpec,
    emit: Emit<'_, InstallEvent>,
) -> Result<(), ApiError> {
    let root = shell.store_root()?;
    let dir = model_dir(&root, spec);
    let installed = {
        let dir = dir.clone();
        blocking(move || Ok(is_installed(&dir, spec)), SPEECH_STOPPED).await?
    };
    if installed {
        emit(InstallEvent::Done);
        return Ok(());
    }
    let (guard, mut cancel) = shell.begin_install(spec.engine)?;
    let question = consent_question(&confirm.labels(), spec);
    let accepted = blocking(move || Ok(confirm.ask(&question)), SPEECH_STOPPED).await?;
    if !accepted {
        return Err(dictation_error(
            "engine",
            DictationErrorCode::ConsentDeclined,
            "the download was declined",
        ));
    }
    let policy = shell.download_policy();
    let mut progress = |done, total| {
        if !emit(InstallEvent::Progress { done, total }) {
            // Nobody listens any more: stop downloading.
            guard.cancel();
        }
    };
    let downloaded = download_model(&policy, &dir, spec, &mut cancel, &mut progress).await;
    let outcome = match downloaded {
        Ok(()) => {
            emit(InstallEvent::Verifying);
            let dir = dir.clone();
            blocking(move || Ok(commit_download(&dir, spec)), SPEECH_STOPPED).await?
        }
        Err(code) => Err(code),
    };
    match outcome {
        Ok(()) => {
            tracing::info!(model = spec.id, "a speech model was installed");
            emit(InstallEvent::Done);
        }
        Err(code) => {
            tracing::info!(
                model = spec.id,
                code = code.as_str(),
                "a speech model was not installed"
            );
            let dir = dir.clone();
            blocking(
                move || {
                    clean_partials(&dir, spec);
                    Ok(())
                },
                SPEECH_STOPPED,
            )
            .await?;
            emit(InstallEvent::Error { code });
        }
    }
    drop(guard);
    Ok(())
}

/// Cancels the installation that runs (`dictation_install_cancel`); nothing running is fine.
pub fn install_cancel(shell: &SpeechShell) {
    shell.cancel_install();
}

/// Imports the model of `engine` from a folder the person chooses in a native folder dialog
/// (`dictation_import`, "Use model files..."), for a machine that is offline. Every file of the catalog must be in
/// the folder with the pinned size and SHA-256 (else `InvalidInput` for `engine` with reason `importInvalid` or
/// `hashMismatch`), the files are copied to `.part` names, renamed, and the marker written. Answers the new status.
///
/// # Errors
///
/// As [`status`], `modelMissing` for an engine with no model, `busy`, the reasons above; a cancelled dialog is
/// `InvalidInput` for `path`.
pub async fn import<R: Runtime>(
    app: AppHandle<R>,
    host: Arc<dyn CoreHost>,
    shell: Arc<SpeechShell>,
    engine: SpeechEngine,
) -> Result<DictationStatus, ApiError> {
    import_from(
        host,
        shell,
        engine,
        // A blocking dialog, made from the app handle; the webview cannot reach it.
        move || dialogs::choose_import_folder(&app),
    )
    .await
}

/// [`import`] with the folder chosen by `choose` (a native dialog; a test's closure).
pub(crate) async fn import_from(
    host: Arc<dyn CoreHost>,
    shell: Arc<SpeechShell>,
    engine: SpeechEngine,
    choose: impl FnOnce() -> Result<PathBuf, ApiError> + Send + 'static,
) -> Result<DictationStatus, ApiError> {
    core(&host, |host| host.ensure_ready()).await?;
    let spec = engine
        .model()
        .ok_or_else(|| no_model("this engine has no model files to use"))?;
    let root = shell.store_root()?;
    shell.refuse_while_in_use(engine)?;
    let (guard, _cancel) = shell.begin_install(engine)?;
    let dir = model_dir(&root, spec);
    blocking(
        move || {
            let source = choose()?;
            import_folder(&dir, spec, &source)
        },
        SPEECH_STOPPED,
    )
    .await?;
    drop(guard);
    status(host, shell).await
}

/// Deletes the model of `engine` from the store (`dictation_remove`) and answers the new status. A running
/// session of that engine, or an installation of it, is `busy`.
///
/// # Errors
///
/// As [`status`], `modelMissing` for an engine with no model, `busy`, `storeFailed`.
pub async fn remove(
    host: Arc<dyn CoreHost>,
    shell: Arc<SpeechShell>,
    engine: SpeechEngine,
) -> Result<DictationStatus, ApiError> {
    core(&host, |host| host.ensure_ready()).await?;
    let spec = engine
        .model()
        .ok_or_else(|| no_model("this engine has no model files to remove"))?;
    let root = shell.store_root()?;
    shell.refuse_while_in_use(engine)?;
    blocking(
        move || remove_model(&model_dir(&root, spec)),
        SPEECH_STOPPED,
    )
    .await?;
    status(host, shell).await
}

/// The checks of [`start`] that read the disk or start a process, in the contract's order, then the process and
/// its threads, registered as the running session. Blocking.
fn prepare_session(
    shell: &Arc<SpeechShell>,
    root: &Path,
    session_id: &str,
    engine: SpeechEngine,
    ai: Option<(bool, bool, bool)>,
) -> Result<
    (
        SessionHandle,
        tokio::sync::mpsc::UnboundedReceiver<session::Msg>,
    ),
    ApiError,
> {
    if shell.session_running() {
        return Err(busy("sessionId", "another dictation session is running"));
    }
    let model_folder = match engine.model() {
        Some(spec) => {
            let dir = model_dir(root, spec);
            if !is_installed(&dir, spec) {
                return Err(dictation_error(
                    "engine",
                    DictationErrorCode::ModelMissing,
                    "the model of this engine is not installed",
                ));
            }
            dir
        }
        None => root.to_path_buf(),
    };
    let launcher = shell.launcher();
    let located = launcher.locate(shell).ok_or_else(|| {
        dictation_error(
            "engine",
            DictationErrorCode::EngineMissing,
            "the dictation engine is not part of this installation",
        )
    })?;
    if let Some((enabled, model_set, allowed)) = ai
        && !(enabled && model_set && allowed)
    {
        return Err(dictation_error(
            "engine",
            DictationErrorCode::OllamaUnavailable,
            "AI transcription is not set up",
        ));
    }
    let failed = || {
        dictation_error(
            "engine",
            DictationErrorCode::EngineFailed,
            "the dictation engine could not be started",
        )
    };
    let config = engine.start_config(&model_folder, &located.vad);
    let sidecar = launcher.launch(&located.exe).map_err(|error| {
        tracing::info!(kind = ?error.kind(), "the dictation sidecar could not be started");
        failed()
    })?;
    let (handle, rx) = session::open(
        sidecar,
        config,
        session_id.to_owned(),
        engine,
        shell.next_token(),
        shell.timeouts().quit_grace,
    )
    .map_err(|()| failed())?;
    if let Err(error) = shell.register_session(handle.clone()) {
        // Another session got in while this one started.
        handle.cancel();
        return Err(error);
    }
    Ok((handle, rx))
}

/// Starts a dictation session (`dictation_start`): spawns the sidecar and sends it the first `Start` frame.
/// Returns once the session is registered and the process runs; then `DictationEvent`s flow to `emit`
/// (`State(Loading)`, `State(Listening)`, `Phrase`..., `Ended`). See the module documentation for the checks, the
/// threads and how a session ends.
///
/// # Errors
///
/// Only when nothing started: the core is not ready, the session id is not valid, another session runs (`busy`),
/// the engine's model is not installed (`modelMissing`), the sidecar or the voice activity model is missing
/// (`engineMissing`), AI transcription is not set up (`ollamaUnavailable`), the process cannot be started
/// (`engineFailed`).
pub async fn start(
    host: Arc<dyn CoreHost>,
    shell: Arc<SpeechShell>,
    ai: Arc<AiShell>,
    session_id: String,
    engine: SpeechEngine,
    emit: Arc<dyn Fn(DictationEvent) -> bool + Send + Sync>,
) -> Result<(), ApiError> {
    // The AI settings, read from the core only for the Ollama engine: (AI on, a transcription model, the endpoint
    // allowed).
    let ai_facts = core(&host, move |host| {
        host.ensure_ready()?;
        if engine != SpeechEngine::Ollama {
            return Ok(None);
        }
        let settings = read_ai_settings(host)?;
        let allowed = allowed_url(&read_endpoint(host)?).is_ok();
        Ok(Some((
            settings.enabled,
            settings
                .transcription_model
                .is_some_and(|model| !model.trim().is_empty()),
            allowed,
        )))
    })
    .await?;
    validate_session_id(&session_id)?;
    let root = shell.store_root()?;
    let (handle, rx) = {
        let shell = Arc::clone(&shell);
        blocking(
            move || prepare_session(&shell, &root, &session_id, engine, ai_facts),
            SPEECH_STOPPED,
        )
        .await?
    };
    let ollama = (engine == SpeechEngine::Ollama).then(|| Ollama { host, ai });
    let timeouts = shell.timeouts();
    let driver = Driver::new(
        handle.clone(),
        rx,
        shell,
        Arc::clone(&emit),
        ollama,
        timeouts,
    );
    // The first event, before the driver can send another.
    if !emit(DictationEvent::State {
        state: kb_speech::api::DictationState::Loading,
    }) {
        handle.cancel();
    }
    tauri::async_runtime::spawn(driver.run());
    Ok(())
}

/// Takes audio for a session (`dictation_audio`): the samples of the request body, in order, never blocking. They
/// are cut into `Audio` frames of at most `MAX_AUDIO_FRAME_SAMPLES` and queued for the sidecar.
///
/// # Errors
///
/// `InvalidInput` for `sessionId` (reason `ended`) for an unknown or finished session id.
pub fn audio(shell: &SpeechShell, session_id: &str, pcm: Vec<i16>) -> Result<(), ApiError> {
    shell
        .session(session_id)
        .ok_or_else(session::ended_error)?
        .audio(pcm)
}

/// Ends a session after its last phrase (`dictation_stop`): `Flush` is queued behind the audio, the sidecar sends
/// what is left and `Flushed`, the Ollama requests of the last segments are waited for (`FLUSH_TIMEOUT` at most),
/// then `Ended` and `Quit` follow. Returns at once; an unknown session is fine.
pub fn stop(shell: &SpeechShell, session_id: &str) {
    if let Some(session) = shell.session(session_id) {
        session.stop();
    }
}

/// Ends a session now (`dictation_cancel`): drops the queued audio, kills the sidecar and sends `Ended`.
/// An unknown session is fine.
pub fn cancel(shell: &SpeechShell, session_id: &str) {
    if let Some(session) = shell.session(session_id) {
        session.cancel();
    }
}

#[cfg(test)]
mod tests;
