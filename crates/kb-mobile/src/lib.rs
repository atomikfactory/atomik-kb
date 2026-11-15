//! The Rust core on a phone (`apps/android/ARCHITECTURE.md`, Contract 1): a UniFFI `cdylib` whose [`KbEngine`] owns
//! `kb-core` behind one mutex, a tokio runtime, the sync driver and the pairing and invite tasks, and offers the app
//! one JSON door (`dispatch`), the raw page-document and attachment calls, the app lock and an event sink.
//!
//! The semantics mirror the desktop shell (`apps/desktop/src-tauri`: `state.rs`, `doc_ipc.rs`, `attachment_ipc.rs`,
//! `sync_host.rs`); this crate is its port, not a refactor of it. Events are queued under the core lock and delivered
//! to the [`EventSink`] by a thread of their own, outside the lock, in order. A panic never crosses the FFI boundary.
//!
//! The crate generates `extern "C"` glue, hence `unsafe_code` is allowed here (and only here).

#![allow(unsafe_code)]

mod host;
mod keystore;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use kb_core::{CoreDeps, Opened};
use kb_storage_native::{
    BLOB_DIR, DB_FILE, FsBlobStore, NativeDbOpener, NativeError, OsRng, SystemClock,
    load_or_create_install,
};
use kb_types::attachments::{CHUNK_SIZE, MAX_PREVIEW_BYTES};
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, ByteRange, ErrorCode, PageId, Request, Response,
};
use zeroize::Zeroizing;

use host::{Delivery, Host, STARTING};
use keystore::KeystoreAdapter;
pub use keystore::{KeystoreFfiError, PlatformKeystore};

uniffi::setup_scaffolding!();

/// What the app tells the engine about where it lives.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineConfig {
    /// The app-private directory for the database, the attachment chunks and `install.json`.
    pub data_dir: String,
    /// The platform name for diagnostics, `"android"`.
    pub platform: String,
}

/// Where the engine is in its life.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum EngineState {
    /// The keystore, database and migrations are still being set up.
    Opening,
    /// Requests are served.
    Ready,
    /// The app lock is engaged: nothing is open until `unlock`.
    Locked,
    /// Opening failed for good until the app restarts.
    Failed {
        /// What went wrong, safe to show.
        message: String,
    },
}

/// What a call that is not `dispatch` can answer instead of its value.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EngineError {
    /// The core is still opening.
    #[error("not ready")]
    NotReady,
    /// The workspace is locked.
    #[error("locked")]
    Locked,
    /// The core refused: `json` is a `kb_types::ApiError`.
    #[error("{json}")]
    Api {
        /// The `ApiError` as JSON.
        json: String,
    },
    /// An unexpected failure (a panic that was caught, an encoding error).
    #[error("{message}")]
    Internal {
        /// What happened, without user content.
        message: String,
    },
}

/// What the app implements to hear about the engine.
#[uniffi::export(with_foreign)]
pub trait EventSink: Send + Sync {
    /// One `kb_types::Event` as JSON (`{"type":..,"data":..}`), in emission order.
    fn on_event(&self, event_json: String);
    /// The engine's state changed (opening, ready, locked, failed, lock, unlock).
    fn on_state(&self, state: EngineState);
}

fn engine_error(error: &ApiError) -> EngineError {
    match error.code {
        ErrorCode::Locked => EngineError::Locked,
        ErrorCode::Internal if error.message == STARTING => EngineError::NotReady,
        _ => match serde_json::to_string(error) {
            Ok(json) => EngineError::Api { json },
            Err(_) => internal("the error could not be encoded"),
        },
    }
}

fn internal(message: &str) -> EngineError {
    EngineError::Internal {
        message: message.to_owned(),
    }
}

/// Runs `call`, turning a panic into [`EngineError::Internal`].
fn guarded<T>(call: impl FnOnce() -> Result<T, EngineError>) -> Result<T, EngineError> {
    catch_unwind(AssertUnwindSafe(call))
        .unwrap_or_else(|_| Err(internal("an internal error occurred")))
}

fn api<T>(result: Result<T, ApiError>) -> Result<T, EngineError> {
    result.map_err(|error| engine_error(&error))
}

fn page_id(text: &str) -> Result<PageId, EngineError> {
    api(text
        .parse::<PageId>()
        .map_err(|_| ApiError::invalid("pageId", "the page id must be UUID text")))
}

fn attachment_id(text: &str) -> Result<AttachmentId, EngineError> {
    api(text
        .parse::<AttachmentId>()
        .map_err(|_| ApiError::invalid("attachmentId", "the attachment id must be UUID text")))
}

fn limit_exceeded(field: &str, message: &str, limit: u64) -> ApiError {
    ApiError::invalid(field, message)
        .with_details(serde_json::json!({ "field": field, "limit": limit }))
}

/// The byte range an `attachment_read` asks for: both ends or neither (as the desktop's headers); non-empty and at
/// most `MAX_PREVIEW_BYTES` long.
fn byte_range(start: Option<u64>, end: Option<u64>) -> Result<Option<ByteRange>, ApiError> {
    let (start, end) = match (start, end) {
        (None, None) => return Ok(None),
        (Some(start), Some(end)) => (start, end),
        (Some(_), None) => {
            return Err(ApiError::invalid(
                "end",
                "the range end is missing: give both ends or neither",
            ));
        }
        (None, Some(_)) => {
            return Err(ApiError::invalid(
                "start",
                "the range start is missing: give both ends or neither",
            ));
        }
    };
    let length = match end.checked_sub(start) {
        Some(length) if length > 0 => length,
        _ => {
            return Err(ApiError::invalid(
                "range",
                "the range must end after it starts",
            ));
        }
    };
    if length > MAX_PREVIEW_BYTES {
        return Err(limit_exceeded(
            "range",
            "the range is longer than one read may answer; ask for it in pieces",
            MAX_PREVIEW_BYTES,
        ));
    }
    Ok(Some(ByteRange { start, end }))
}

/// The plaintext of an attachment, or of the half-open byte `range`, as one buffer (`attachments::read_bytes` of the
/// desktop shell): without a range, an attachment above `MAX_PREVIEW_BYTES` is refused before a byte is decrypted.
fn read_attachment(
    host: &Host,
    attachment_id: AttachmentId,
    range: Option<ByteRange>,
) -> Result<Vec<u8>, ApiError> {
    let announced = match range {
        Some(range) => range.end.saturating_sub(range.start),
        None => match host.dispatch(Request::AttachmentGet { attachment_id })? {
            Response::Attachment(AttachmentInfo { size, .. }) => {
                if size > MAX_PREVIEW_BYTES {
                    return Err(limit_exceeded(
                        "size",
                        "the attachment is too large to read in one piece: ask for a byte range",
                        MAX_PREVIEW_BYTES,
                    ));
                }
                size
            }
            _ => {
                return Err(ApiError::internal(
                    "the core answered with an unexpected response",
                ));
            }
        },
    };
    let capacity = usize::try_from(announced)
        .map_err(|_| ApiError::internal("the attachment is too large for this device"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| ApiError::internal("there is not enough memory to read the attachment"))?;
    let mut sink = |piece: &[u8]| -> Result<(), ApiError> {
        if bytes.len() + piece.len() > capacity {
            return Err(ApiError::internal(
                "the attachment is longer than the core announced",
            ));
        }
        bytes.extend_from_slice(piece);
        Ok(())
    };
    host.attachment_read(attachment_id, range, &mut sink)?;
    if bytes.len() != capacity {
        return Err(ApiError::internal(
            "the attachment is shorter than the core announced",
        ));
    }
    Ok(bytes)
}

/// What can go wrong while building the platform dependencies, as the API error the app shows.
fn api_error_from_native(error: &NativeError) -> ApiError {
    match error {
        NativeError::Keystore(_) => ApiError::keystore_unavailable(error.to_string()),
        NativeError::DataDir(_) => ApiError::internal(error.to_string()),
    }
}

/// Builds the platform pieces (the keystore is the app's) and opens the core: ready, or locked when the app lock
/// is on. Blocking: keystore access, database open and migrations.
fn open_core(
    config: &EngineConfig,
    keystore: Arc<dyn PlatformKeystore>,
) -> Result<Opened, ApiError> {
    let data_dir = Path::new(&config.data_dir);
    std::fs::create_dir_all(data_dir).map_err(|error| {
        ApiError::internal(format!("cannot create the data directory: {error}"))
    })?;
    let rng = OsRng;
    let install =
        load_or_create_install(data_dir, &rng).map_err(|error| api_error_from_native(&error))?;
    let blob_store = FsBlobStore::new(data_dir.join(BLOB_DIR)).map_err(|error| {
        ApiError::internal(format!("cannot open the attachment store: {error}"))
    })?;
    let deps = CoreDeps {
        install_id: install.install_id,
        platform: config.platform.clone(),
        data_dir: config.data_dir.clone(),
        keystore: Box::new(KeystoreAdapter(keystore)),
        opener: Box::new(NativeDbOpener {
            path: data_dir.join(DB_FILE),
        }),
        clock: Box::new(SystemClock),
        rng: Box::new(rng),
        lock_params: kb_crypto::Argon2Params::DEFAULT,
        blob_store: Box::new(blob_store),
    };
    kb_core::Core::open_or_locked(deps)
}

/// Calls the app's sink with what the host queued, one item at a time, in order, until the engine is gone. A
/// callback that panics (a foreign exception) is logged and skipped.
fn deliver(queue: &Receiver<Delivery>, sink: &Arc<dyn EventSink>) {
    while let Ok(delivery) = queue.recv() {
        let outcome = catch_unwind(AssertUnwindSafe(|| match delivery {
            Delivery::Event(json) => sink.on_event(json),
            Delivery::State(state) => sink.on_state(state),
        }));
        if outcome.is_err() {
            tracing::warn!("the event sink failed");
        }
    }
}

/// The engine: one per process. Calls are blocking; the app calls them off its main thread.
#[derive(uniffi::Object)]
pub struct KbEngine {
    host: Arc<Host>,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
}

#[uniffi::export]
impl KbEngine {
    /// Returns at once; opens the core on a background thread and reports through `sink.on_state`.
    #[uniffi::constructor]
    pub fn new(
        config: EngineConfig,
        keystore: Arc<dyn PlatformKeystore>,
        sink: Arc<dyn EventSink>,
    ) -> Arc<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("kb-engine")
            .enable_all()
            .build();
        // A runtime that cannot start leaves an engine that reports `Failed` for good.
        let (runtime, failure) = match runtime {
            Ok(runtime) => (Some(runtime), None),
            Err(error) => (
                None,
                Some(ApiError::internal(format!(
                    "cannot start the runtime: {error}"
                ))),
            ),
        };
        let (out, queue) = mpsc::channel();
        let host = Host::new(
            out,
            runtime.as_ref().map(|runtime| runtime.handle().clone()),
        );
        let spawned = std::thread::Builder::new()
            .name("kb-events".to_owned())
            .spawn({
                let sink = Arc::clone(&sink);
                move || deliver(&queue, &sink)
            });
        if spawned.is_err() {
            tracing::error!("the event thread could not start");
        }
        host.announce_opening();

        let engine = Arc::new(Self {
            host: Arc::clone(&host),
            runtime: Mutex::new(runtime),
        });
        match failure {
            Some(error) => host.install(Err(error)),
            None => {
                let opened = std::thread::Builder::new()
                    .name("kb-open".to_owned())
                    .spawn(move || {
                        let outcome =
                            catch_unwind(AssertUnwindSafe(|| open_core(&config, keystore)))
                                .unwrap_or_else(|_| {
                                    Err(ApiError::internal("the core could not be started"))
                                });
                        host.install(outcome);
                    });
                if let Err(error) = opened {
                    engine.host.install(Err(ApiError::internal(format!(
                        "cannot start the opening thread: {error}"
                    ))));
                }
            }
        }
        engine
    }

    /// Where the engine is now.
    pub fn state(&self) -> EngineState {
        self.host.state()
    }

    /// Request JSON in (`kb_types::Request`), `{"ok": Response}` or `{"err": ApiError}` out. Never panics across
    /// the FFI boundary.
    pub fn dispatch(&self, request_json: String) -> String {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.dispatch_json(&request_json)))
            .unwrap_or_else(|_| Err(ApiError::internal("an internal error occurred")));
        envelope(&outcome)
    }

    /// Disengages the app lock (Argon2id, about a second). A wrong passphrase is `Api` with `InvalidInput` for the
    /// field `passphrase`; the state stays locked.
    pub fn unlock(&self, passphrase: String) -> Result<(), EngineError> {
        let passphrase = Zeroizing::new(passphrase);
        guarded(|| api(self.host.unlock(&passphrase)))
    }

    /// Engages the app lock: the state becomes `Locked`. Does nothing when the lock is not enabled.
    pub fn lock(&self) {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.host.lock()));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::debug!(code = ?error.code, "lock refused"),
            Err(_) => tracing::error!("lock panicked"),
        }
    }

    /// Opens a page's document for the editor and returns its raw Loro snapshot.
    pub fn doc_open(&self, page_id_text: String) -> Result<Vec<u8>, EngineError> {
        guarded(|| api(self.host.doc_open(page_id(&page_id_text)?)))
    }

    /// Imports a raw Loro update from the editor's replica. An empty update is `InvalidInput`.
    pub fn doc_import(&self, page_id_text: String, update: Vec<u8>) -> Result<(), EngineError> {
        guarded(|| {
            let page = page_id(&page_id_text)?;
            if update.is_empty() {
                return api(Err(ApiError::invalid(
                    "body",
                    "an update must not be empty",
                )));
            }
            api(self.host.doc_import(page, &update))
        })
    }

    /// The raw update with everything the editor's replica lacks, given its encoded version vector (empty means
    /// everything); empty when there is nothing new.
    pub fn doc_export(
        &self,
        page_id_text: String,
        since_version: Vec<u8>,
    ) -> Result<Vec<u8>, EngineError> {
        guarded(|| {
            let page = page_id(&page_id_text)?;
            let since = (!since_version.is_empty()).then_some(since_version.as_slice());
            api(self.host.doc_export(page, since))
        })
    }

    /// Drops the core's in-memory replica of a page; its content stays persisted. Errors are ignored.
    pub fn doc_close(&self, page_id_text: String) {
        let _ = guarded(|| api(self.host.doc_close(page_id(&page_id_text)?)));
    }

    /// An attachment's plaintext, whole (at most 32 MiB) or the half-open byte range `start..end` (both or neither).
    pub fn attachment_read(
        &self,
        attachment_id_text: String,
        start: Option<u64>,
        end: Option<u64>,
    ) -> Result<Vec<u8>, EngineError> {
        guarded(|| {
            let id = attachment_id(&attachment_id_text)?;
            let range = api(byte_range(start, end))?;
            api(read_attachment(&self.host, id, range))
        })
    }

    /// Appends one plaintext piece (1 byte to 4 MiB) to an upload the core started with `AttachmentBegin`.
    pub fn attachment_write(
        &self,
        attachment_id_text: String,
        bytes: Vec<u8>,
    ) -> Result<(), EngineError> {
        guarded(|| {
            let id = attachment_id(&attachment_id_text)?;
            if bytes.is_empty() {
                return api(Err(ApiError::invalid("body", "a piece must not be empty")));
            }
            if bytes.len() > CHUNK_SIZE {
                return api(Err(limit_exceeded(
                    "body",
                    "a piece must not be larger than one chunk",
                    CHUNK_SIZE as u64,
                )));
            }
            api(self.host.attachment_write(id, &bytes))
        })
    }

    /// The app moved to the foreground or the background. Sync keeps running either way; coming to the foreground
    /// asks for a sync round so the notes are current when the app opens.
    pub fn set_foreground(&self, foreground: bool) {
        if foreground {
            let host = Arc::clone(&self.host);
            let _ = std::thread::Builder::new()
                .name("kb-foreground".to_owned())
                .spawn(move || {
                    // Not ready, locked or no server: nothing to ask for.
                    let _ = catch_unwind(AssertUnwindSafe(|| host.dispatch(Request::SyncNow)));
                });
        }
    }

    /// Asks for a sync round now and waits (blocking: call it off the main thread) until the driver is idle or
    /// `timeout_ms` passes. `true` when it is idle (or sync is not configured).
    pub fn sync_once(&self, timeout_ms: u64) -> bool {
        catch_unwind(AssertUnwindSafe(|| {
            self.host.sync_once(Duration::from_millis(timeout_ms))
        }))
        .unwrap_or(false)
    }
}

impl KbEngine {
    fn dispatch_json(&self, text: &str) -> Result<Response, ApiError> {
        // A parse failure names where it is, never what it found: a request can hold a passphrase.
        let request = serde_json::from_str::<Request>(text).map_err(|error| {
            ApiError::invalid(
                "request",
                format!(
                    "the request is not a valid request (line {}, column {})",
                    error.line(),
                    error.column()
                ),
            )
        })?;
        self.host.dispatch(request)
    }
}

impl Drop for KbEngine {
    fn drop(&mut self) {
        self.host.stop_background();
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(runtime) = runtime {
            runtime.shutdown_background();
        }
    }
}

/// `{"ok": Response}` or `{"err": ApiError}`.
fn envelope(outcome: &Result<Response, ApiError>) -> String {
    let encoded = match outcome {
        Ok(response) => serde_json::to_string(&serde_json::json!({ "ok": response })),
        Err(error) => serde_json::to_string(&serde_json::json!({ "err": error })),
    };
    encoded.unwrap_or_else(|_| {
        r#"{"err":{"code":"Internal","message":"the answer could not be encoded"}}"#.to_owned()
    })
}

#[cfg(test)]
mod tests;
