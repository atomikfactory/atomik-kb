//! The shell's state: the one `Core` behind a mutex (ADR-0015, single writer) and the
//! channels events are pushed to. No business logic: requests go in, the core's answer
//! comes out, and whatever events the core emitted are forwarded in order.
//!
//! The core's life is a small state machine (ADR-0018):
//!
//! - `Opening` becomes `Ready`, `Locked` (the app lock was on at start-up) or `Failed`
//!   (for good, until the app restarts);
//! - `Ready` becomes `Locked` through `lock`, and `Locked` becomes `Ready` through
//!   `unlock`; a refused `lock` or a wrong passphrase changes nothing.
//!
//! While `Locked` the shell holds a [`LockedApi`], which has no key material and no
//! database; every command that needs the core (`dispatch`, the `doc_*`, the attachment and
//! the backup commands) is answered with `ErrorCode::Locked`. `lock`, `unlock` and `core_state` are the
//! lifecycle commands, and `subscribe_events` and `open_external` never needed the core. The
//! commands talk to the state through [`CoreHost`], an object-safe view of [`AppState`], so
//! the integration tests can run the real command handlers over a fake core.
//!
//! **Background work** (the sync driver, ADR-0021) lives as long as the core is ready: the
//! [`BackgroundStarter`] registered with [`AppState::set_background`] runs when the core
//! becomes `Ready` (it opened, or `unlock` succeeded) and what it returned is dropped, which
//! stops it, when the core leaves `Ready` (`lock`, a failure). Rust listeners
//! ([`AppState::add_listener`], what the driver subscribes with) receive every event the
//! webview channels receive, and survive page reloads.

use std::cell::Cell;
use std::sync::{Arc, Mutex, PoisonError};

use kb_net::RealtimeHandle;
use kb_sync::v2::SealedPresence;
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, BackupSummary, ByteRange, ErrorCode, Event, PageId,
    PresenceMessage, PresenceState, Request, Response, SpaceId,
};
use serde::Serialize;
use tauri::ipc::Channel;

use crate::presence::{PresenceShell, PresenceSink, check_update};

/// What a request is told while the core is still opening.
const STARTING: &str = "core is starting";

/// Where the core writes an encrypted backup: called once per piece, in order. An `Err`
/// (the I/O failure, as text) aborts the export.
pub type BackupSink<'a> = &'a mut dyn FnMut(&[u8]) -> Result<(), String>;

/// Where the core reads a backup from: fills (a prefix of) the buffer the way
/// [`std::io::Read::read`] does and returns how many bytes it wrote, `0` at the end of the
/// file. An `Err` (the I/O failure, as text) aborts the import.
pub type BackupSource<'a> = &'a mut dyn FnMut(&mut [u8]) -> Result<usize, String>;

// Where the core writes the plaintext of an attachment it reads: called once per piece, in
// order (the type is the core's own). An `Err` stops the read and is the core's answer.
pub use kb_core::AttachmentSink;

/// What the shell needs from the core. [`kb_core::Core`] is the implementation; tests
/// use a fake because the real core needs a keystore and a database.
pub trait CoreApi: Send + Sized + 'static {
    /// What this core becomes while the app lock is engaged ([`kb_core::LockedCore`]).
    type Locked: LockedApi<Core = Self>;

    /// Executes one request.
    fn dispatch(&mut self, request: Request) -> Result<Response, ApiError>;
    /// Returns and clears the events emitted since the last call.
    fn drain_events(&mut self) -> Vec<Event>;
    /// Opens a page's document for an editor and returns its raw Loro snapshot.
    fn page_doc_open(&mut self, page_id: PageId) -> Result<Vec<u8>, ApiError>;
    /// Imports a raw Loro update produced by the editor's replica.
    fn page_doc_import(&mut self, page_id: PageId, update: &[u8]) -> Result<(), ApiError>;
    /// Raw update with everything the editor's replica lacks, given its encoded version
    /// vector (`None` = everything). Empty when there is nothing new.
    fn page_doc_export(
        &mut self,
        page_id: PageId,
        since: Option<&[u8]>,
    ) -> Result<Vec<u8>, ApiError>;
    /// Drops the in-memory replica of a page; its content stays persisted.
    fn page_doc_close(&mut self, page_id: PageId) -> Result<(), ApiError>;
    /// Locks the workspace: consumes the core, which closes its database and zeroises its
    /// key material. When it refuses (the app lock is not enabled) the core comes back
    /// together with the error.
    fn lock(self) -> Result<Self::Locked, (Self, ApiError)>;
    /// Streams an encrypted backup of the whole workspace to `sink`.
    fn backup_export(&mut self, sink: BackupSink<'_>) -> Result<BackupSummary, ApiError>;
    /// Restores a backup read from `source`, with the recovery kit text when the backup
    /// belongs to another installation's account.
    fn backup_import(
        &mut self,
        source: BackupSource<'_>,
        kit: Option<&str>,
    ) -> Result<BackupSummary, ApiError>;
    /// Appends plaintext to an upload the core started (`AttachmentBegin`); at most one chunk
    /// per call.
    fn attachment_write(
        &mut self,
        attachment_id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), ApiError>;
    /// Streams an attachment's plaintext (whole, or the half-open byte `range`) into `sink`
    /// and answers its info.
    fn attachment_read(
        &mut self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError>;
    /// Seals this device's presence in a space for the sync driver's socket (M13 slice 3,
    /// `Core::presence_seal`); `None` when there is nothing to send (a personal space, an archive, no key
    /// yet). The default is a core with no shared spaces: nothing to send.
    fn presence_seal(
        &mut self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        state: PresenceState,
        update: &[u8],
    ) -> Result<Option<SealedPresence>, ApiError> {
        let _ = (space_id, page_id, state, update);
        Ok(None)
    }
}

/// A locked core: waiting for the app-lock passphrase, holding no key material
/// ([`kb_core::LockedCore`] is the implementation).
pub trait LockedApi: Send + Sized + 'static {
    /// The ready core a correct passphrase yields.
    type Core: CoreApi<Locked = Self>;

    /// Derives the key from `passphrase` and opens the workspace. A wrong passphrase
    /// returns the locked core unchanged with `InvalidInput` for the field `passphrase`.
    fn unlock(self, passphrase: &str) -> Result<Self::Core, (Self, ApiError)>;
}

impl CoreApi for kb_core::Core {
    type Locked = kb_core::LockedCore;

    fn dispatch(&mut self, request: Request) -> Result<Response, ApiError> {
        kb_core::Core::dispatch(self, request)
    }

    fn drain_events(&mut self) -> Vec<Event> {
        kb_core::Core::drain_events(self)
    }

    fn page_doc_open(&mut self, page_id: PageId) -> Result<Vec<u8>, ApiError> {
        kb_core::Core::page_doc_open(self, page_id)
    }

    fn page_doc_import(&mut self, page_id: PageId, update: &[u8]) -> Result<(), ApiError> {
        kb_core::Core::page_doc_import(self, page_id, update)
    }

    fn page_doc_export(
        &mut self,
        page_id: PageId,
        since: Option<&[u8]>,
    ) -> Result<Vec<u8>, ApiError> {
        kb_core::Core::page_doc_export(self, page_id, since)
    }

    fn page_doc_close(&mut self, page_id: PageId) -> Result<(), ApiError> {
        kb_core::Core::page_doc_close(self, page_id)
    }

    fn lock(self) -> Result<kb_core::LockedCore, (Self, ApiError)> {
        kb_core::Core::lock(self)
    }

    fn backup_export(&mut self, sink: BackupSink<'_>) -> Result<BackupSummary, ApiError> {
        kb_core::Core::backup_export(self, sink)
    }

    fn backup_import(
        &mut self,
        source: BackupSource<'_>,
        kit: Option<&str>,
    ) -> Result<BackupSummary, ApiError> {
        kb_core::Core::backup_import(self, source, kit)
    }

    fn attachment_write(
        &mut self,
        attachment_id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        kb_core::Core::attachment_write(self, attachment_id, bytes)
    }

    fn attachment_read(
        &mut self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        kb_core::Core::attachment_read(self, attachment_id, range, sink)
    }

    fn presence_seal(
        &mut self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        state: PresenceState,
        update: &[u8],
    ) -> Result<Option<SealedPresence>, ApiError> {
        kb_core::Core::presence_seal(self, space_id, page_id, state, update)
    }
}

impl LockedApi for kb_core::LockedCore {
    type Core = kb_core::Core;

    fn unlock(self, passphrase: &str) -> Result<kb_core::Core, (Self, ApiError)> {
        kb_core::LockedCore::unlock(self, passphrase)
    }
}

/// Where the core is in its life. It opens on a background thread after the window
/// exists, so the UI can already be asking questions while it is still `Opening`.
pub enum CoreState<C: CoreApi = kb_core::Core> {
    /// The keystore, database and migrations are still being set up.
    Opening,
    /// Requests are served.
    Ready(C),
    /// The app lock is engaged: nothing is open and no key is in memory until `unlock`.
    Locked(C::Locked),
    /// Opening failed for good; every request gets this error until the app restarts.
    Failed(ApiError),
}

impl<C: CoreApi> CoreState<C> {
    /// The ready core, or the error every request is answered with while there is none.
    fn ready(&mut self) -> Result<&mut C, ApiError> {
        match self {
            Self::Ready(core) => Ok(core),
            Self::Opening => Err(ApiError::internal(STARTING)),
            Self::Locked(_) => Err(ApiError::locked()),
            Self::Failed(error) => Err(error.clone()),
        }
    }

    /// The `lock` transition. `publish` receives the events the core emitted: those
    /// nobody has been told about yet go out *before* the core is consumed, and so do
    /// those of a lock that is refused (the core stays ready). Locking a locked
    /// workspace has nothing left to do and succeeds.
    fn lock(self, mut publish: impl FnMut(Vec<Event>)) -> (Self, Result<(), ApiError>) {
        match self {
            Self::Ready(mut core) => {
                publish(core.drain_events());
                match core.lock() {
                    Ok(locked) => (Self::Locked(locked), Ok(())),
                    Err((mut core, error)) => {
                        publish(core.drain_events());
                        (Self::Ready(core), Err(error))
                    }
                }
            }
            Self::Locked(locked) => (Self::Locked(locked), Ok(())),
            Self::Opening => (Self::Opening, Err(ApiError::internal(STARTING))),
            Self::Failed(error) => (Self::Failed(error.clone()), Err(error)),
        }
    }

    /// The `unlock` transition. A wrong passphrase puts the locked core back; a core that
    /// is not locked answers `InvalidInput`. `publish` receives what the freshly opened
    /// core emitted while it was opening.
    fn unlock(
        self,
        passphrase: &str,
        publish: impl FnOnce(Vec<Event>),
    ) -> (Self, Result<(), ApiError>) {
        match self {
            Self::Locked(locked) => match locked.unlock(passphrase) {
                Ok(mut core) => {
                    publish(core.drain_events());
                    (Self::Ready(core), Ok(()))
                }
                Err((locked, error)) => (Self::Locked(locked), Err(error)),
            },
            Self::Ready(core) => (
                Self::Ready(core),
                Err(ApiError::new(
                    ErrorCode::InvalidInput,
                    "the workspace is not locked",
                )),
            ),
            Self::Opening => (Self::Opening, Err(ApiError::internal(STARTING))),
            Self::Failed(error) => (Self::Failed(error.clone()), Err(error)),
        }
    }
}

/// What the `core_state` command returns. Serialises as `"opening"`, `"ready"`,
/// `"locked"` or `{ "failed": { "code": ..., "message": ... } }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreStateInfo {
    Opening,
    Ready,
    Locked,
    Failed(ApiError),
}

impl<C: CoreApi> From<&CoreState<C>> for CoreStateInfo {
    fn from(state: &CoreState<C>) -> Self {
        match state {
            CoreState::Opening => Self::Opening,
            CoreState::Ready(_) => Self::Ready,
            CoreState::Locked(_) => Self::Locked,
            CoreState::Failed(error) => Self::Failed(error.clone()),
        }
    }
}

/// Work that runs while the core is ready (the sync driver's handle); dropping it stops it.
pub type BackgroundTask = Box<dyn Send>;

/// Starts the background work; called each time the core becomes ready (`None`: nothing
/// to run). It must not call into the core synchronously: it runs right after a state
/// change, outside the core's lock but on the thread that changed the state.
pub type BackgroundStarter = Box<dyn Fn() -> Option<BackgroundTask> + Send + Sync>;

/// Several pieces of background work as one starter: the sync driver and the reminders'
/// timer. It starts each of `starters` (those that start nothing are left out) and holds what
/// they returned, so that dropping the result stops them all. `None` when none started.
pub fn all_of(starters: Vec<BackgroundStarter>) -> BackgroundStarter {
    Box::new(move || {
        let running: Vec<BackgroundTask> = starters.iter().filter_map(|start| start()).collect();
        if running.is_empty() {
            None
        } else {
            let all: BackgroundTask = Box::new(running);
            Some(all)
        }
    })
}

/// A Rust event listener (the sync driver's, the reminders'). Called under the core's lock: it
/// must not block or call into the core.
pub type Listener = Arc<dyn Fn(&Event) + Send + Sync>;

#[derive(Default)]
struct Listeners {
    next_id: u64,
    entries: Vec<(u64, Listener)>,
}

#[derive(Default)]
struct Background {
    starter: Option<BackgroundStarter>,
    running: Option<BackgroundTask>,
}

/// Managed by Tauri (inside an `Arc`, so blocking tasks can own a handle to it).
///
/// Lock order is always `core` then `subscribers` (or `listeners`, never both at once);
/// `background` is never held while `core` is taken, and may be held while `listeners` is
/// (a starter subscribes). `subscribe`, `clear_subscribers` and `state_info` each take only
/// one of them.
pub struct AppState<C: CoreApi = kb_core::Core> {
    core: Mutex<CoreState<C>>,
    subscribers: Mutex<Vec<Channel<Event>>>,
    listeners: Arc<Mutex<Listeners>>,
    background: Mutex<Background>,
    /// The channels of presence frames, the throttle of the webview's updates and the sync driver's socket
    /// (M13 slice 3); never held while `core` is taken, and each of its locks is held for a moment only.
    pub(crate) presence: PresenceShell,
}

impl<C: CoreApi> Default for AppState<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: CoreApi> AppState<C> {
    /// A state whose core is still opening and that has no subscribers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Mutex::new(CoreState::Opening),
            subscribers: Mutex::new(Vec::new()),
            listeners: Arc::default(),
            background: Mutex::default(),
            presence: PresenceShell::default(),
        }
    }

    /// The sync driver started: its socket is where presence frames go (see `PresenceShell::attach`).
    pub fn presence_attach(&self, handle: RealtimeHandle) {
        self.presence.attach(handle);
    }

    /// Sends a presence frame the core accepted to every channel the webview registered with
    /// `presence_events`; whether any heard it. What `SyncHost::presence_received` does with the core's answer.
    pub fn presence_deliver(&self, message: &PresenceMessage) -> bool {
        self.presence.deliver(message)
    }

    /// How many `presence_events` channels are registered.
    #[must_use]
    pub fn presence_subscribers(&self) -> usize {
        self.presence.subscribers()
    }

    /// The core finished opening: the background work starts.
    pub fn set_ready(&self, core: C) {
        *self.core.lock().unwrap_or_else(PoisonError::into_inner) = CoreState::Ready(core);
        self.start_background();
    }

    /// The core opened into the locked state: the app lock is on and nothing is open until
    /// the passphrase arrives (nor runs in the background).
    pub fn set_locked(&self, locked: C::Locked) {
        *self.core.lock().unwrap_or_else(PoisonError::into_inner) = CoreState::Locked(locked);
        self.stop_background();
    }

    /// The core could not be opened.
    pub fn set_failed(&self, error: ApiError) {
        *self.core.lock().unwrap_or_else(PoisonError::into_inner) = CoreState::Failed(error);
        self.stop_background();
    }

    /// Registers the background work (the sync driver): it starts now when the core is
    /// ready, else as soon as it is, and again after every unlock.
    pub fn set_background(&self, starter: BackgroundStarter) {
        let previous = {
            let mut background = self
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            background.starter = Some(starter);
            background.running.take()
        };
        drop(previous);
        if matches!(self.state_info(), CoreStateInfo::Ready) {
            self.start_background();
        }
    }

    /// Whether background work is running.
    #[must_use]
    pub fn background_running(&self) -> bool {
        self.background
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running
            .is_some()
    }

    /// Starts the background work unless it runs already.
    fn start_background(&self) {
        let mut background = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if background.running.is_some() {
            return;
        }
        background.running = background.starter.as_ref().and_then(|start| start());
        if background.running.is_some() {
            tracing::debug!("background work started");
        }
    }

    /// Stops the background work (dropping it never waits for it).
    fn stop_background(&self) {
        let running = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running
            .take();
        if running.is_some() {
            drop(running);
            tracing::debug!("background work stopped");
        }
    }

    /// Registers a Rust listener for every event from now on; the returned closure
    /// removes it.
    pub fn add_listener(&self, listener: Listener) -> Box<dyn FnOnce() + Send> {
        let id = {
            let mut listeners = self
                .listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let id = listeners.next_id;
            listeners.next_id += 1;
            listeners.entries.push((id, listener));
            id
        };
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            listeners
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entries
                .retain(|(other, _)| *other != id);
        })
    }

    /// How many Rust listeners are registered.
    #[must_use]
    pub fn listener_count(&self) -> usize {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .len()
    }

    /// Sends events to every channel in order, dropping channels that cannot be reached,
    /// and hands them to every Rust listener.
    fn publish(&self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        let listeners: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for event in events {
            for listener in &listeners {
                listener(&event);
            }
            subscribers.retain(|channel| channel.send(event.clone()).is_ok());
        }
    }

    /// The one place the single-writer lock is taken for a core call: refuses while the
    /// core is not ready (still opening, failed, or locked), runs `call`, collects the
    /// events it emitted, reports the result and the event count to `log`, and forwards
    /// the events before the lock is released.
    pub(crate) fn with_core<T>(
        &self,
        call: impl FnOnce(&mut C) -> Result<T, ApiError>,
        log: impl FnOnce(&Result<T, ApiError>, usize),
    ) -> Result<T, ApiError> {
        let mut guard = self.core.lock().map_err(|_| poisoned())?;
        let core = guard.ready()?;
        let result = call(core);
        let events = core.drain_events();
        log(&result, events.len());
        self.publish(events);
        result
    }
}

/// What the command handlers call: [`AppState`] seen through an object-safe interface, so
/// the managed state is one `Arc<dyn CoreHost>` whichever core is behind it (the real one
/// in the app, a fake in the integration tests).
///
/// Every method is blocking (it takes the single-writer lock and may touch the disk or
/// run Argon2id); call it from `spawn_blocking`. Whatever the core emitted is forwarded
/// to the subscribers before the lock is released, so two calls never deliver their
/// events out of order.
pub trait CoreHost: Send + Sync + 'static {
    /// Lifecycle summary for the UI.
    fn state_info(&self) -> CoreStateInfo;

    /// Registers a channel to receive every event from now on.
    fn subscribe(&self, channel: Channel<Event>);

    /// Forgets all channels; a page that is loading again has lost its callbacks.
    fn clear_subscribers(&self);

    /// Registers a Rust listener for every event from now on (the background tasks hear the core's
    /// events through it); the returned closure removes it. The listener is called under the core's
    /// lock: it must not block or call into the core.
    fn add_listener(&self, listener: Listener) -> Box<dyn FnOnce() + Send>;

    /// `Ok` when the core is ready; otherwise the error a request would get right now
    /// (`Internal` while starting, the failure, `Locked`). Lets a command refuse before it
    /// opens a dialog or a file, without being a promise: the call that follows checks
    /// again under the lock.
    fn ensure_ready(&self) -> Result<(), ApiError>;

    /// Runs one request on the core, then forwards the events it emitted.
    fn dispatch(&self, request: Request) -> Result<Response, ApiError>;

    /// Opens a page's document for an editor and returns its raw Loro snapshot.
    fn doc_open(&self, page_id: PageId) -> Result<Vec<u8>, ApiError>;

    /// Imports a raw update from the editor's replica. The core persists and indexes it
    /// and emits the content events, which are forwarded like any other.
    fn doc_import(&self, page_id: PageId, update: &[u8]) -> Result<(), ApiError>;

    /// The raw update with everything the editor's replica lacks (`since` is its encoded
    /// version vector, `None` for everything); empty when there is nothing new.
    fn doc_export(&self, page_id: PageId, since: Option<&[u8]>) -> Result<Vec<u8>, ApiError>;

    /// Drops the core's in-memory replica of a page.
    fn doc_close(&self, page_id: PageId) -> Result<(), ApiError>;

    /// Engages the app lock: `Ready` becomes `Locked`. A core that refuses (the lock is
    /// not enabled) stays ready and its error is returned; locking a locked workspace
    /// succeeds without doing anything.
    fn lock(&self) -> Result<(), ApiError>;

    /// Disengages the app lock: `Locked` becomes `Ready`. A wrong passphrase leaves the
    /// state locked and returns the core's error (`InvalidInput` for `passphrase`); a
    /// workspace that is not locked is `InvalidInput`. Takes about a second (Argon2id).
    /// The passphrase is never logged.
    fn unlock(&self, passphrase: &str) -> Result<(), ApiError>;

    /// Streams an encrypted backup of the workspace to `sink`; the core is called once,
    /// under the lock.
    fn backup_export(&self, sink: BackupSink<'_>) -> Result<BackupSummary, ApiError>;

    /// Restores a backup read from `source` into the workspace; the core is called once,
    /// under the lock, and the events it emitted are forwarded.
    fn backup_import(
        &self,
        source: BackupSource<'_>,
        kit: Option<&str>,
    ) -> Result<BackupSummary, ApiError>;

    /// Appends one plaintext piece to an upload the core started with `AttachmentBegin`.
    /// The core is called once, under the lock; the lock is released between pieces, so a
    /// long upload never blocks the editor for longer than one piece.
    fn attachment_write(&self, attachment_id: AttachmentId, bytes: &[u8]) -> Result<(), ApiError>;

    /// Streams an attachment's plaintext, whole or the half-open byte `range`, into `sink`
    /// and answers its info; the core is called once, under the lock, and the events it
    /// emitted are forwarded.
    fn attachment_read(
        &self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError>;

    /// `presence_send` (M13 slice 3, see `presence`): the core seals this device's presence under one call, and
    /// the sealed envelope goes to the sync driver's socket, best effort. `update` over
    /// `PRESENCE_UPDATE_MAX_BYTES` is `InvalidInput` for `update`; the updates past
    /// `PRESENCE_UPDATES_PER_SECOND` in a second are dropped with `Ok`; a personal space, an archive and a
    /// device with no socket send nothing, and that is `Ok` too. Blocking: call it from `spawn_blocking`.
    fn presence_send(
        &self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        state: PresenceState,
        update: &[u8],
    ) -> Result<(), ApiError>;

    /// `presence_events`: registers a channel for the presence frames the core accepted; it is forgotten
    /// when it cannot be reached, and with `clear_subscribers`.
    fn presence_subscribe(&self, sink: PresenceSink);
}

impl<C: CoreApi> CoreHost for AppState<C> {
    fn state_info(&self) -> CoreStateInfo {
        match self.core.lock() {
            Ok(core) => CoreStateInfo::from(&*core),
            Err(_) => CoreStateInfo::Failed(poisoned()),
        }
    }

    fn subscribe(&self, channel: Channel<Event>) {
        tracing::debug!("event subscriber registered");
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(channel);
    }

    fn clear_subscribers(&self) {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        // The page's presence channel went with its callbacks.
        self.presence.clear();
    }

    fn add_listener(&self, listener: Listener) -> Box<dyn FnOnce() + Send> {
        AppState::add_listener(self, listener)
    }

    fn ensure_ready(&self) -> Result<(), ApiError> {
        let mut guard = self.core.lock().map_err(|_| poisoned())?;
        guard.ready().map(|_| ())
    }

    fn dispatch(&self, request: Request) -> Result<Response, ApiError> {
        let kind = request.kind();
        self.with_core(
            |core| core.dispatch(request),
            // Variant names only: payloads may hold user content.
            |result, events| tracing::debug!(kind, ok = result.is_ok(), events, "dispatch"),
        )
    }

    fn doc_open(&self, page_id: PageId) -> Result<Vec<u8>, ApiError> {
        self.with_core(
            |core| core.page_doc_open(page_id),
            |result, events| {
                let bytes = result.as_ref().map_or(0, Vec::len);
                log_doc("doc_open", result.is_ok(), events, bytes);
            },
        )
    }

    fn doc_import(&self, page_id: PageId, update: &[u8]) -> Result<(), ApiError> {
        self.with_core(
            |core| core.page_doc_import(page_id, update),
            |result, events| log_doc("doc_import", result.is_ok(), events, update.len()),
        )
    }

    fn doc_export(&self, page_id: PageId, since: Option<&[u8]>) -> Result<Vec<u8>, ApiError> {
        self.with_core(
            |core| core.page_doc_export(page_id, since),
            |result, events| {
                let bytes = result.as_ref().map_or(0, Vec::len);
                log_doc("doc_export", result.is_ok(), events, bytes);
            },
        )
    }

    fn doc_close(&self, page_id: PageId) -> Result<(), ApiError> {
        self.with_core(
            |core| core.page_doc_close(page_id),
            |result, events| log_doc("doc_close", result.is_ok(), events, 0),
        )
    }

    fn lock(&self) -> Result<(), ApiError> {
        let (after, result) = {
            let mut guard = self.core.lock().map_err(|_| poisoned())?;
            let before = CoreStateInfo::from(&*guard);
            let current = std::mem::replace(&mut *guard, CoreState::Opening);
            let (next, result) = current.lock(|events| self.publish(events));
            let after = CoreStateInfo::from(&next);
            *guard = next;
            log_transition("lock", &before, &after, &result);
            (after, result)
        };
        // Outside the core's lock: nothing runs in the background of a locked workspace.
        if after == CoreStateInfo::Locked {
            self.stop_background();
        }
        result
    }

    fn unlock(&self, passphrase: &str) -> Result<(), ApiError> {
        let (after, result) = {
            let mut guard = self.core.lock().map_err(|_| poisoned())?;
            let before = CoreStateInfo::from(&*guard);
            let current = std::mem::replace(&mut *guard, CoreState::Opening);
            let (next, result) = current.unlock(passphrase, |events| self.publish(events));
            let after = CoreStateInfo::from(&next);
            *guard = next;
            log_transition("unlock", &before, &after, &result);
            (after, result)
        };
        if result.is_ok() && after == CoreStateInfo::Ready {
            self.start_background();
        }
        result
    }

    fn backup_export(&self, sink: BackupSink<'_>) -> Result<BackupSummary, ApiError> {
        self.with_core(
            |core| core.backup_export(sink),
            |result, events| log_backup("backup_export", result, events),
        )
    }

    fn backup_import(
        &self,
        source: BackupSource<'_>,
        kit: Option<&str>,
    ) -> Result<BackupSummary, ApiError> {
        self.with_core(
            |core| core.backup_import(source, kit),
            |result, events| log_backup("backup_import", result, events),
        )
    }

    fn attachment_write(&self, attachment_id: AttachmentId, bytes: &[u8]) -> Result<(), ApiError> {
        self.with_core(
            |core| core.attachment_write(attachment_id, bytes),
            |result, events| {
                let code = result.as_ref().err().map(|error| error.code);
                log_attachment("attachment_write", code, events, bytes.len() as u64);
            },
        )
    }

    fn attachment_read(
        &self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        // What was handed to the sink, for the log: a count, never the bytes.
        let delivered = Cell::new(0_u64);
        self.with_core(
            |core| {
                core.attachment_read(attachment_id, range, &mut |piece: &[u8]| {
                    delivered.set(delivered.get().saturating_add(piece.len() as u64));
                    sink(piece)
                })
            },
            |result, events| {
                let code = result.as_ref().err().map(|error| error.code);
                log_attachment("attachment_read", code, events, delivered.get());
            },
        )
    }

    fn presence_send(
        &self,
        space_id: SpaceId,
        page_id: Option<PageId>,
        state: PresenceState,
        update: &[u8],
    ) -> Result<(), ApiError> {
        check_update(update)?;
        // The second line of the editor's own limit; past it nothing is sealed and nothing is sent.
        if !self.presence.allow() {
            tracing::trace!("a presence update was dropped: too many in a second");
            return Ok(());
        }
        let sealed = self.with_core(
            |core| core.presence_seal(space_id, page_id, state, update),
            // Whether there was a frame, never what was in it.
            |result, events| {
                let sealed = result.as_ref().is_ok_and(Option::is_some);
                tracing::trace!(sealed, events, bytes = update.len(), "presence_send");
            },
        )?;
        if let Some(sealed) = sealed {
            let handed = self.presence.send(sealed.feed, sealed.envelope);
            tracing::trace!(handed, "presence frame");
        }
        Ok(())
    }

    fn presence_subscribe(&self, sink: PresenceSink) {
        tracing::debug!("presence subscriber registered");
        self.presence.subscribe(sink);
    }
}

/// One debug line per page-document call: which command, whether it worked, how many
/// events it emitted and how many payload bytes it moved. Never the payload itself.
fn log_doc(kind: &'static str, ok: bool, events: usize, bytes: usize) {
    tracing::debug!(kind, ok, events, bytes, "doc");
}

/// One debug line per backup call: which command, whether it worked, the size and the
/// error code. Never the file's content, the kit or a path (the command logs the path).
fn log_backup(kind: &'static str, result: &Result<BackupSummary, ApiError>, events: usize) {
    match result {
        Ok(summary) => tracing::debug!(
            kind,
            ok = true,
            events,
            bytes = summary.bytes,
            documents = summary.documents,
            "backup"
        ),
        Err(error) => tracing::debug!(kind, ok = false, events, code = ?error.code, "backup"),
    }
}

/// One debug line per binary attachment call: which command, whether it worked (the error
/// code when not), how many events it emitted and how many plaintext bytes it moved. Never
/// the bytes, a name or a MIME type.
fn log_attachment(kind: &'static str, error: Option<ErrorCode>, events: usize, bytes: u64) {
    match error {
        None => tracing::debug!(kind, ok = true, events, bytes, "attachment"),
        Some(code) => tracing::debug!(kind, ok = false, events, bytes, ?code, "attachment"),
    }
}

/// Logs what a `lock` or `unlock` did: a state change at `info`, a refusal with the error
/// code only. Neither the passphrase nor the error text (which a core could build from
/// user input) is ever written.
fn log_transition(
    command: &'static str,
    before: &CoreStateInfo,
    after: &CoreStateInfo,
    result: &Result<(), ApiError>,
) {
    match result {
        Ok(()) if before != after => {
            tracing::info!(command, from = ?before, to = ?after, "core state changed");
        }
        Ok(()) => tracing::debug!(command, state = ?after, "core state unchanged"),
        Err(error) => {
            tracing::info!(command, code = ?error.code, state = ?after, "state change refused");
        }
    }
}

fn poisoned() -> ApiError {
    ApiError::internal("the core stopped after an internal error; restart atomik-kb")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use kb_crypto::Signature;
    use kb_sync::v2::{
        FeedChunkKind, FeedChunkWork, FeedWork, InlineEntry, InlineOutcome, InviteOpenRequest,
        InviteOpened, InviteSignup, LinkOutcome, LinkPut, LinkWork, LogRequest, MembershipHead,
        MembershipPage, OutboundEntry, PendingMembership, SnapshotMark, SnapshotsApplied,
        SyncApplied,
    };
    use kb_sync::{ChangesPage, SnapshotsPage};
    use kb_types::{
        AccountId, ChangeSource, ChunkId, DeviceId, DocId, FeedId, InviteFlow, InviteSessionInfo,
        InviteSessionState, InviteSide, LinkId, PairingInfo, PairingRole, PairingState,
        PresenceInfo, PresenceMessage, RecoveryInfo, RecoveryState, SpaceId, SyncInfo, SyncState,
        WorkspaceId,
    };
    use tauri::ipc::InvokeResponseBody;

    use super::*;
    use crate::sync_host::{DevicesCoreApi, InviteCoreApi, SyncCoreApi, driver_starter};
    use crate::test_support::captured_logs;

    /// One call the fake core received on its page-document API.
    #[derive(Debug, Clone, PartialEq)]
    enum DocCall {
        Open(PageId),
        Import(PageId, Vec<u8>),
        Export(PageId, Option<Vec<u8>>),
        Close(PageId),
    }

    /// One call the fake core received on its attachment API.
    #[derive(Debug, Clone, PartialEq)]
    enum AttachmentCall {
        Write(AttachmentId, Vec<u8>),
        Read(AttachmentId, Option<ByteRange>),
    }

    /// Answers every call with a canned result, emits a canned list of events and records
    /// what it was asked.
    #[derive(Default)]
    struct FakeCore {
        seen: Vec<Request>,
        doc_calls: Vec<DocCall>,
        pending: Vec<Event>,
        emit: Vec<Event>,
        snapshot: Vec<u8>,
        update: Vec<u8>,
        fail_with: Option<ApiError>,
        panic_on_call: bool,
        /// The passphrase the fake app lock accepts.
        passphrase: String,
        /// What `lock` refuses with, when it does.
        lock_refusal: Option<ApiError>,
        panic_on_lock: bool,
        /// Queued on the core a successful `unlock` yields (what its open sequence emitted).
        unlock_events: Vec<Event>,
        /// What `backup_export` writes to the sink, piece by piece.
        backup_pieces: Vec<Vec<u8>>,
        /// What `backup_import` read from the source.
        imported: Vec<u8>,
        /// The kit text `backup_import` was given.
        kit: Option<String>,
        backup_calls: usize,
        /// What `attachment_read` streams to the sink, piece by piece.
        attachment_pieces: Vec<Vec<u8>>,
        attachment_calls: Vec<AttachmentCall>,
        /// The sync methods the driver (or a test) called, in order.
        sync_calls: Vec<&'static str>,
        /// The pairing session and the recovery the fake core runs (`None`: idle), and
        /// the server they go through.
        pairing: Option<(PairingRole, PairingState, Option<[u8; 6]>)>,
        recovery: Option<RecoveryState>,
        device_server: Option<String>,
        /// The invite session the fake core runs (`None`: idle).
        invite: Option<(InviteSide, InviteSessionState, Option<[u8; 6]>)>,
        /// What `sync_feed_chunk_work` hands out and what `sync_feed_chunk_bytes` reads.
        chunk_work: Vec<FeedChunkWork>,
        chunk_blob: Option<Vec<u8>>,
        /// The chunk calls with their hash (and the bytes `sync_feed_chunk_received` got).
        chunk_calls: Vec<(&'static str, ChunkId, Option<Vec<u8>>)>,
        /// What `presence_seal` answers (`None`: nothing to send) and the calls it got.
        sealed: Option<SealedPresence>,
        seal_calls: Vec<(SpaceId, Option<PageId>, PresenceState, Vec<u8>)>,
        /// What `presence_open` answers, call after call (an empty queue: the frame is dropped), and the frames
        /// it was given.
        opened: std::collections::VecDeque<PresenceMessage>,
        opened_frames: Vec<(FeedId, Vec<u8>)>,
        /// What `sync_link_work` hands out (once) and the outcomes `sync_link_done` was given.
        link_work: Vec<LinkWork>,
        link_done: Vec<(LinkId, LinkOutcome)>,
    }

    impl FakeCore {
        /// What every call has in common: maybe panic, queue the canned events, then
        /// answer with `ok` or the canned failure.
        fn answer<T>(&mut self, ok: T) -> Result<T, ApiError> {
            assert!(!self.panic_on_call, "the core has a bug");
            self.pending.extend(self.emit.iter().cloned());
            match &self.fail_with {
                Some(error) => Err(error.clone()),
                None => Ok(ok),
            }
        }
    }

    /// The locked fake: it keeps the core it was made from, so a test can see that
    /// unlocking gives back the very same core.
    struct FakeLocked {
        core: FakeCore,
        attempts: usize,
    }

    impl LockedApi for FakeLocked {
        type Core = FakeCore;

        fn unlock(mut self, passphrase: &str) -> Result<FakeCore, (Self, ApiError)> {
            self.attempts += 1;
            if passphrase != self.core.passphrase {
                let error = ApiError::invalid("passphrase", "the passphrase is wrong");
                return Err((self, error));
            }
            let queued = self.core.unlock_events.clone();
            self.core.pending.extend(queued);
            Ok(self.core)
        }
    }

    fn summary(bytes: u64) -> BackupSummary {
        BackupSummary {
            bytes,
            documents: 2,
            settings: 3,
            created_at: 1_700_000_000_000,
            account_id: AccountId::from_bytes([7; 16]),
            chunks: 0,
            chunk_bytes: 0,
        }
    }

    impl CoreApi for FakeCore {
        type Locked = FakeLocked;

        fn dispatch(&mut self, request: Request) -> Result<Response, ApiError> {
            self.seen.push(request);
            self.answer(Response::Ok)
        }

        fn drain_events(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }

        fn page_doc_open(&mut self, page_id: PageId) -> Result<Vec<u8>, ApiError> {
            self.doc_calls.push(DocCall::Open(page_id));
            let snapshot = self.snapshot.clone();
            self.answer(snapshot)
        }

        fn page_doc_import(&mut self, page_id: PageId, update: &[u8]) -> Result<(), ApiError> {
            self.doc_calls
                .push(DocCall::Import(page_id, update.to_vec()));
            self.answer(())
        }

        fn page_doc_export(
            &mut self,
            page_id: PageId,
            since: Option<&[u8]>,
        ) -> Result<Vec<u8>, ApiError> {
            self.doc_calls
                .push(DocCall::Export(page_id, since.map(<[u8]>::to_vec)));
            let update = self.update.clone();
            self.answer(update)
        }

        fn page_doc_close(&mut self, page_id: PageId) -> Result<(), ApiError> {
            self.doc_calls.push(DocCall::Close(page_id));
            self.answer(())
        }

        fn lock(self) -> Result<FakeLocked, (Self, ApiError)> {
            assert!(!self.panic_on_lock, "the core has a bug");
            match self.lock_refusal.clone() {
                Some(error) => Err((self, error)),
                None => Ok(FakeLocked {
                    core: self,
                    attempts: 0,
                }),
            }
        }

        fn backup_export(&mut self, sink: BackupSink<'_>) -> Result<BackupSummary, ApiError> {
            self.backup_calls += 1;
            let mut bytes = 0;
            for piece in &self.backup_pieces {
                sink(piece).map_err(ApiError::storage)?;
                bytes += piece.len() as u64;
            }
            self.answer(summary(bytes))
        }

        fn backup_import(
            &mut self,
            source: BackupSource<'_>,
            kit: Option<&str>,
        ) -> Result<BackupSummary, ApiError> {
            self.backup_calls += 1;
            self.kit = kit.map(str::to_owned);
            // A deliberately small buffer: the shell must cope with any size.
            let mut buffer = [0_u8; 4];
            let mut read = Vec::new();
            loop {
                let count = source(&mut buffer).map_err(ApiError::storage)?;
                if count == 0 {
                    break;
                }
                read.extend_from_slice(&buffer[..count]);
            }
            let bytes = read.len() as u64;
            self.imported = read;
            self.answer(summary(bytes))
        }

        fn attachment_write(
            &mut self,
            attachment_id: AttachmentId,
            bytes: &[u8],
        ) -> Result<(), ApiError> {
            self.attachment_calls
                .push(AttachmentCall::Write(attachment_id, bytes.to_vec()));
            self.answer(())
        }

        fn attachment_read(
            &mut self,
            attachment_id: AttachmentId,
            range: Option<ByteRange>,
            sink: AttachmentSink<'_>,
        ) -> Result<AttachmentInfo, ApiError> {
            self.attachment_calls
                .push(AttachmentCall::Read(attachment_id, range));
            let mut size = 0;
            for piece in &self.attachment_pieces {
                sink(piece)?;
                size += piece.len() as u64;
            }
            self.answer(attachment_info(attachment_id, size))
        }

        fn presence_seal(
            &mut self,
            space_id: SpaceId,
            page_id: Option<PageId>,
            state: PresenceState,
            update: &[u8],
        ) -> Result<Option<SealedPresence>, ApiError> {
            self.seal_calls
                .push((space_id, page_id, state, update.to_vec()));
            let sealed = self.sealed.clone();
            self.answer(sealed)
        }
    }

    type Received = Arc<Mutex<Vec<String>>>;

    /// A channel that records the JSON of every event, as the webview would receive it.
    fn recording_channel() -> (Channel<Event>, Received) {
        let received: Received = Arc::default();
        let sink = Arc::clone(&received);
        let channel = Channel::new(move |body| {
            if let InvokeResponseBody::Json(json) = body {
                sink.lock().unwrap().push(json);
            }
            Ok(())
        });
        (channel, received)
    }

    /// A channel whose webview is gone: every send fails.
    fn broken_channel() -> Channel<Event> {
        Channel::new(|_| Err(tauri::Error::Io(std::io::Error::other("webview closed"))))
    }

    fn changed(key: &str) -> Event {
        Event::SettingChanged { key: key.into() }
    }

    fn page(byte: u8) -> PageId {
        PageId::from_bytes([byte; 16])
    }

    fn attachment(byte: u8) -> AttachmentId {
        AttachmentId::from_bytes([byte; 16])
    }

    fn attachment_info(attachment_id: AttachmentId, size: u64) -> AttachmentInfo {
        AttachmentInfo {
            attachment_id,
            space_id: kb_types::SpaceId::from_bytes([3; 16]),
            name: "name.png".into(),
            mime: "image/png".into(),
            size,
            chunk_count: 1,
            created_at: 1_700_000_000_000,
            references: 0,
            unreferenced_since: None,
            local_chunks: 1,
            availability: kb_types::AttachmentAvailability::Complete,
        }
    }

    /// What the fake core was asked through its attachment API so far.
    fn attachment_calls(state: &AppState<FakeCore>) -> Vec<AttachmentCall> {
        with_ready(state, |core| core.attachment_calls.clone())
    }

    fn ready_state(core: FakeCore) -> AppState<FakeCore> {
        let state = AppState::new();
        state.set_ready(core);
        state
    }

    /// A state that is `Locked` after a real `lock` of a core that accepts `passphrase`.
    fn locked_state(core: FakeCore) -> AppState<FakeCore> {
        let state = ready_state(core);
        state.lock().unwrap();
        state
    }

    /// What the fake core was asked through its page-document API so far.
    fn doc_calls(state: &AppState<FakeCore>) -> Vec<DocCall> {
        match &*state.core.lock().unwrap() {
            CoreState::Ready(core) => core.doc_calls.clone(),
            _ => Vec::new(),
        }
    }

    /// Looks at the fake core inside a ready state.
    fn with_ready<T>(state: &AppState<FakeCore>, inspect: impl FnOnce(&FakeCore) -> T) -> T {
        let guard = state.core.lock().unwrap();
        let CoreState::Ready(core) = &*guard else {
            unreachable!("the state is not ready");
        };
        inspect(core)
    }

    /// How many passphrases the fake locked core has been tried with.
    fn attempts(state: &AppState<FakeCore>) -> usize {
        let guard = state.core.lock().unwrap();
        let CoreState::Locked(locked) = &*guard else {
            unreachable!("the state is not locked");
        };
        locked.attempts
    }

    fn json(info: &CoreStateInfo) -> serde_json::Value {
        serde_json::to_value(info).unwrap()
    }

    #[test]
    fn requests_are_refused_while_the_core_is_opening() {
        let state: AppState<FakeCore> = AppState::new();
        let error = state.dispatch(Request::AppStatus).unwrap_err();
        assert_eq!(error, ApiError::internal("core is starting"));
        assert_eq!(state.state_info(), CoreStateInfo::Opening);
    }

    #[test]
    fn a_failed_core_answers_every_request_with_the_opening_error() {
        let state: AppState<FakeCore> = AppState::new();
        let failure = ApiError::keystore_unavailable("no usable keystore: no secret service");
        state.set_failed(failure.clone());
        for request in [
            Request::AppStatus,
            Request::DeviceInfo,
            Request::WorkspaceGet,
        ] {
            assert_eq!(state.dispatch(request).unwrap_err(), failure);
        }
        assert_eq!(state.state_info(), CoreStateInfo::Failed(failure));
    }

    #[test]
    fn a_ready_core_serves_requests_and_reports_ready() {
        let state = ready_state(FakeCore::default());
        assert_eq!(state.dispatch(Request::DeviceInfo).unwrap(), Response::Ok);
        assert_eq!(state.state_info(), CoreStateInfo::Ready);
    }

    #[test]
    fn the_core_can_fail_after_it_was_ready() {
        let state = ready_state(FakeCore::default());
        state.set_failed(ApiError::internal("boom"));
        assert_eq!(
            state.state_info(),
            CoreStateInfo::Failed(ApiError::internal("boom"))
        );
    }

    #[test]
    fn errors_from_the_core_pass_through_untouched() {
        let error = ApiError::invalid("name", "must not be empty");
        let state = ready_state(FakeCore {
            fail_with: Some(error.clone()),
            ..FakeCore::default()
        });
        assert_eq!(state.dispatch(Request::AppStatus).unwrap_err(), error);
    }

    #[test]
    fn events_reach_every_subscriber_in_order_as_json() {
        let state = ready_state(FakeCore {
            emit: vec![
                Event::WorkspaceCreated {
                    id: WorkspaceId::from_bytes([1; 16]),
                },
                changed("ui.theme"),
                Event::DeviceRenamed {
                    name: "Desk".into(),
                },
            ],
            ..FakeCore::default()
        });
        let (first, first_seen) = recording_channel();
        let (second, second_seen) = recording_channel();
        state.subscribe(first);
        state.subscribe(second);

        state.dispatch(Request::AppStatus).unwrap();

        let expected: Vec<String> = [
            r#"{"type":"WorkspaceCreated","data":{"id":"01010101-0101-0101-0101-010101010101"}}"#,
            r#"{"type":"SettingChanged","data":{"key":"ui.theme"}}"#,
            r#"{"type":"DeviceRenamed","data":{"name":"Desk"}}"#,
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(*first_seen.lock().unwrap(), expected);
        assert_eq!(*second_seen.lock().unwrap(), expected);
    }

    #[test]
    fn events_are_not_repeated_by_the_next_request() {
        let state = ready_state(FakeCore {
            emit: vec![changed("ui.theme")],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        state.dispatch(Request::AppStatus).unwrap();
        state.dispatch(Request::AppStatus).unwrap();
        // The fake emits one event per request, so exactly two arrive, not three.
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn events_emitted_by_a_failing_request_are_still_delivered() {
        let state = ready_state(FakeCore {
            emit: vec![changed("ui.theme")],
            fail_with: Some(ApiError::storage("disk full")),
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        assert!(state.dispatch(Request::AppStatus).is_err());
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn nothing_is_sent_when_the_core_emitted_nothing() {
        let state = ready_state(FakeCore::default());
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        state.dispatch(Request::AppStatus).unwrap();
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn channels_that_fail_are_dropped_and_the_others_keep_working() {
        let state = ready_state(FakeCore {
            emit: vec![changed("a"), changed("b")],
            ..FakeCore::default()
        });
        let (healthy, seen) = recording_channel();
        state.subscribe(broken_channel());
        state.subscribe(healthy);

        state.dispatch(Request::AppStatus).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(state.subscribers.lock().unwrap().len(), 1);

        // The survivor still gets the next batch.
        state.dispatch(Request::AppStatus).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[test]
    fn events_do_not_reach_subscribers_cleared_by_a_page_load() {
        let state = ready_state(FakeCore {
            emit: vec![changed("a")],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        state.clear_subscribers();
        state.dispatch(Request::AppStatus).unwrap();
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_panicking_core_poisons_the_state_instead_of_taking_the_shell_down() {
        let state = ready_state(FakeCore {
            panic_on_call: true,
            ..FakeCore::default()
        });
        assert!(catch_unwind(AssertUnwindSafe(|| state.dispatch(Request::AppStatus))).is_err());

        let error = state.dispatch(Request::AppStatus).unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(
            matches!(state.state_info(), CoreStateInfo::Failed(e) if e.code == ErrorCode::Internal)
        );
    }

    #[test]
    fn core_state_info_serialises_as_the_ui_expects() {
        assert_eq!(json(&CoreStateInfo::Opening), serde_json::json!("opening"));
        assert_eq!(json(&CoreStateInfo::Ready), serde_json::json!("ready"));
        assert_eq!(json(&CoreStateInfo::Locked), serde_json::json!("locked"));
        assert_eq!(
            json(&CoreStateInfo::Failed(ApiError::keystore_unavailable(
                "no keystore"
            ))),
            serde_json::json!({ "failed": { "code": "KeystoreUnavailable", "message": "no keystore" } })
        );
    }

    // --- page documents (the binary API) ---

    #[test]
    fn page_documents_are_refused_while_the_core_is_opening() {
        let state: AppState<FakeCore> = AppState::new();
        let starting = ApiError::internal("core is starting");
        assert_eq!(state.doc_open(page(1)).unwrap_err(), starting);
        assert_eq!(state.doc_import(page(1), b"u").unwrap_err(), starting);
        assert_eq!(state.doc_export(page(1), None).unwrap_err(), starting);
        assert_eq!(state.doc_close(page(1)).unwrap_err(), starting);
    }

    #[test]
    fn a_failed_core_refuses_page_documents_with_its_opening_error() {
        let state: AppState<FakeCore> = AppState::new();
        let failure = ApiError::keystore_unavailable("no usable keystore");
        state.set_failed(failure.clone());
        assert_eq!(state.doc_open(page(1)).unwrap_err(), failure);
        assert_eq!(state.doc_import(page(1), b"u").unwrap_err(), failure);
        assert_eq!(state.doc_export(page(1), None).unwrap_err(), failure);
        assert_eq!(state.doc_close(page(1)).unwrap_err(), failure);
    }

    #[test]
    fn opening_a_page_document_returns_the_snapshot_the_core_produced() {
        let state = ready_state(FakeCore {
            snapshot: vec![0, 1, 2, 255],
            ..FakeCore::default()
        });
        assert_eq!(state.doc_open(page(7)).unwrap(), vec![0, 1, 2, 255]);
        assert_eq!(doc_calls(&state), vec![DocCall::Open(page(7))]);
    }

    #[test]
    fn importing_passes_the_update_bytes_through_untouched() {
        let state = ready_state(FakeCore::default());
        state.doc_import(page(3), &[0, 255, 7]).unwrap();
        assert_eq!(
            doc_calls(&state),
            vec![DocCall::Import(page(3), vec![0, 255, 7])]
        );
    }

    #[test]
    fn exporting_passes_the_version_vector_and_returns_the_update() {
        let state = ready_state(FakeCore {
            update: vec![5, 6],
            ..FakeCore::default()
        });
        assert_eq!(
            state.doc_export(page(4), Some(&[9, 9])).unwrap(),
            vec![5, 6]
        );
        assert_eq!(state.doc_export(page(4), None).unwrap(), vec![5, 6]);
        assert_eq!(
            doc_calls(&state),
            vec![
                DocCall::Export(page(4), Some(vec![9, 9])),
                DocCall::Export(page(4), None),
            ]
        );
    }

    #[test]
    fn an_empty_export_is_a_result_not_an_error() {
        let state = ready_state(FakeCore::default());
        assert_eq!(
            state.doc_export(page(4), Some(&[1])).unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn closing_a_page_document_reaches_the_core() {
        let state = ready_state(FakeCore::default());
        state.doc_close(page(9)).unwrap();
        assert_eq!(doc_calls(&state), vec![DocCall::Close(page(9))]);
    }

    #[test]
    fn errors_from_page_document_calls_pass_through_untouched() {
        let error = ApiError::not_found("no such page");
        let state = ready_state(FakeCore {
            fail_with: Some(error.clone()),
            ..FakeCore::default()
        });
        assert_eq!(state.doc_open(page(1)).unwrap_err(), error);
        assert_eq!(state.doc_import(page(1), b"u").unwrap_err(), error);
        assert_eq!(state.doc_export(page(1), None).unwrap_err(), error);
        assert_eq!(state.doc_close(page(1)).unwrap_err(), error);
    }

    #[test]
    fn page_document_events_reach_subscribers_in_order_as_json() {
        let state = ready_state(FakeCore {
            emit: vec![
                Event::PageContentChanged {
                    page_id: page(1),
                    source: ChangeSource::Editor,
                },
                Event::BacklinksChanged { page_id: page(2) },
            ],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);

        state.doc_import(page(1), b"update").unwrap();

        let expected: Vec<String> = [
            r#"{"type":"PageContentChanged","data":{"pageId":"01010101-0101-0101-0101-010101010101","source":"Editor"}}"#,
            r#"{"type":"BacklinksChanged","data":{"pageId":"02020202-0202-0202-0202-020202020202"}}"#,
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(*seen.lock().unwrap(), expected);
    }

    #[test]
    fn events_are_forwarded_after_every_kind_of_page_document_call() {
        let state = ready_state(FakeCore {
            emit: vec![changed("a")],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);

        state.doc_open(page(1)).unwrap();
        state.doc_import(page(1), b"u").unwrap();
        state.doc_export(page(1), None).unwrap();
        state.doc_close(page(1)).unwrap();

        // One event per call, none repeated.
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[test]
    fn events_emitted_by_a_failing_page_document_call_are_still_delivered() {
        let state = ready_state(FakeCore {
            emit: vec![changed("a")],
            fail_with: Some(ApiError::storage("disk full")),
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        assert!(state.doc_import(page(1), b"u").is_err());
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn requests_and_page_documents_run_on_the_same_core() {
        let state = ready_state(FakeCore::default());
        state.dispatch(Request::AppStatus).unwrap();
        state.doc_open(page(1)).unwrap();
        state.dispatch(Request::DeviceInfo).unwrap();
        with_ready(&state, |core| {
            assert_eq!(core.seen.len(), 2);
            assert_eq!(core.doc_calls, vec![DocCall::Open(page(1))]);
        });
    }

    #[test]
    fn a_panicking_core_poisons_the_state_during_a_page_document_call() {
        let state = ready_state(FakeCore {
            panic_on_call: true,
            ..FakeCore::default()
        });
        assert!(catch_unwind(AssertUnwindSafe(|| state.doc_import(page(1), b"u"))).is_err());

        assert_eq!(
            state.doc_open(page(1)).unwrap_err().code,
            ErrorCode::Internal
        );
        assert!(matches!(state.state_info(), CoreStateInfo::Failed(_)));
    }

    #[test]
    fn page_document_calls_log_their_size_and_never_their_content() {
        let state = ready_state(FakeCore {
            emit: vec![changed("a"), changed("b")],
            snapshot: b"SECRET snapshot".to_vec(),
            update: b"SECRET delta!".to_vec(),
            ..FakeCore::default()
        });
        let logs = captured_logs(|| {
            state.doc_open(page(1)).unwrap();
            state.doc_import(page(1), b"SECRET update").unwrap();
            state.doc_export(page(1), Some(b"SECRET vector")).unwrap();
            state.doc_close(page(1)).unwrap();
        });

        for kind in ["doc_open", "doc_import", "doc_export", "doc_close"] {
            assert!(logs.contains(&format!("kind=\"{kind}\"")), "{kind}: {logs}");
        }
        // 15, 13, 13 and 0 payload bytes, two events per call.
        for needle in ["bytes=15", "bytes=13", "bytes=0", "events=2", "ok=true"] {
            assert!(logs.contains(needle), "{needle}: {logs}");
        }
        assert!(
            !logs.contains("SECRET"),
            "content leaked into the log: {logs}"
        );
    }

    // --- the app lock ---

    #[test]
    fn locking_a_ready_core_makes_the_state_locked() {
        let state = ready_state(FakeCore::default());
        state.lock().unwrap();
        assert_eq!(state.state_info(), CoreStateInfo::Locked);
    }

    #[test]
    fn a_workspace_that_opens_locked_reports_locked() {
        let state: AppState<FakeCore> = AppState::new();
        state.set_locked(FakeLocked {
            core: FakeCore {
                passphrase: "open sesame".into(),
                ..FakeCore::default()
            },
            attempts: 0,
        });
        assert_eq!(state.state_info(), CoreStateInfo::Locked);
        state.unlock("open sesame").unwrap();
        assert_eq!(state.state_info(), CoreStateInfo::Ready);
    }

    #[test]
    fn every_command_answers_locked_while_locked() {
        let state = locked_state(FakeCore::default());
        let locked = ApiError::locked();
        assert_eq!(locked.code, ErrorCode::Locked);

        assert_eq!(state.dispatch(Request::AppStatus).unwrap_err(), locked);
        assert_eq!(state.doc_open(page(1)).unwrap_err(), locked);
        assert_eq!(state.doc_import(page(1), b"u").unwrap_err(), locked);
        assert_eq!(state.doc_export(page(1), None).unwrap_err(), locked);
        assert_eq!(state.doc_close(page(1)).unwrap_err(), locked);
        assert_eq!(state.ensure_ready().unwrap_err(), locked);

        let mut written = 0;
        let mut sink = |piece: &[u8]| -> Result<(), String> {
            written += piece.len();
            Ok(())
        };
        assert_eq!(state.backup_export(&mut sink).unwrap_err(), locked);
        assert_eq!(written, 0, "the sink must not be touched while locked");

        let mut reads = 0;
        let mut source = |_: &mut [u8]| -> Result<usize, String> {
            reads += 1;
            Ok(0)
        };
        assert_eq!(
            state.backup_import(&mut source, Some("kit")).unwrap_err(),
            locked
        );
        assert_eq!(reads, 0, "the source must not be touched while locked");

        // The state is still locked, and still answers `core_state`.
        assert_eq!(state.state_info(), CoreStateInfo::Locked);
    }

    #[test]
    fn unlocking_with_the_right_passphrase_gives_back_the_same_core() {
        let state = ready_state(FakeCore {
            passphrase: "correct horse".into(),
            ..FakeCore::default()
        });
        state.doc_open(page(5)).unwrap();
        state.lock().unwrap();

        state.unlock("correct horse").unwrap();

        assert_eq!(state.state_info(), CoreStateInfo::Ready);
        // The core that comes back is the one that was locked: it remembers its calls.
        assert_eq!(doc_calls(&state), vec![DocCall::Open(page(5))]);
        assert_eq!(state.dispatch(Request::AppStatus).unwrap(), Response::Ok);
    }

    #[test]
    fn a_wrong_passphrase_keeps_the_state_locked_and_returns_the_error() {
        let state = locked_state(FakeCore {
            passphrase: "correct horse".into(),
            ..FakeCore::default()
        });

        let error = state.unlock("battery staple").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(
            error.details.as_ref().and_then(|d| d.get("field")),
            Some(&serde_json::json!("passphrase"))
        );
        assert_eq!(state.state_info(), CoreStateInfo::Locked);
        assert_eq!(attempts(&state), 1);

        // Another wrong attempt, then the right one: nothing is lost on the way.
        assert!(state.unlock("again wrong").is_err());
        assert_eq!(attempts(&state), 2);
        state.unlock("correct horse").unwrap();
        assert_eq!(state.state_info(), CoreStateInfo::Ready);
    }

    #[test]
    fn a_ready_workspace_cannot_be_unlocked() {
        let state = ready_state(FakeCore::default());
        let error = state.unlock("anything at all").unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.message, "the workspace is not locked");
        // No field: the passphrase is not what is wrong.
        assert!(error.details.is_none());
        // And the core is still there, untouched.
        assert_eq!(state.state_info(), CoreStateInfo::Ready);
        assert_eq!(state.dispatch(Request::AppStatus).unwrap(), Response::Ok);
    }

    #[test]
    fn unlocking_while_the_core_opens_or_after_it_failed_answers_like_any_other_command() {
        let state: AppState<FakeCore> = AppState::new();
        assert_eq!(
            state.unlock("pw").unwrap_err(),
            ApiError::internal("core is starting")
        );
        assert_eq!(state.state_info(), CoreStateInfo::Opening);

        let failure = ApiError::keystore_unavailable("no keystore");
        state.set_failed(failure.clone());
        assert_eq!(state.unlock("pw").unwrap_err(), failure);
        assert_eq!(state.state_info(), CoreStateInfo::Failed(failure));
    }

    #[test]
    fn locking_while_the_core_opens_or_after_it_failed_answers_like_any_other_command() {
        let state: AppState<FakeCore> = AppState::new();
        assert_eq!(
            state.lock().unwrap_err(),
            ApiError::internal("core is starting")
        );
        assert_eq!(state.state_info(), CoreStateInfo::Opening);

        let failure = ApiError::keystore_unavailable("no keystore");
        state.set_failed(failure.clone());
        assert_eq!(state.lock().unwrap_err(), failure);
        assert_eq!(state.state_info(), CoreStateInfo::Failed(failure));
    }

    #[test]
    fn locking_a_locked_workspace_is_a_no_op() {
        let state = locked_state(FakeCore {
            passphrase: "pw pw pw pw".into(),
            ..FakeCore::default()
        });
        state.lock().unwrap();
        assert_eq!(state.state_info(), CoreStateInfo::Locked);
        // Still the same locked core: the passphrase still opens it.
        state.unlock("pw pw pw pw").unwrap();
    }

    #[test]
    fn a_core_that_refuses_to_lock_stays_ready_and_its_error_is_returned() {
        let refusal = ApiError::invalid("appLock", "the app lock is not enabled");
        let state = ready_state(FakeCore {
            lock_refusal: Some(refusal.clone()),
            ..FakeCore::default()
        });
        state.doc_open(page(2)).unwrap();

        assert_eq!(state.lock().unwrap_err(), refusal);

        assert_eq!(state.state_info(), CoreStateInfo::Ready);
        // The very core that refused is back in place, with its history.
        assert_eq!(doc_calls(&state), vec![DocCall::Open(page(2))]);
        assert_eq!(state.dispatch(Request::AppStatus).unwrap(), Response::Ok);
    }

    #[test]
    fn events_the_core_emitted_are_published_before_it_is_locked() {
        let state = ready_state(FakeCore {
            pending: vec![changed("emitted.before.lock")],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);

        state.lock().unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![r#"{"type":"SettingChanged","data":{"key":"emitted.before.lock"}}"#.to_string()]
        );
        assert_eq!(state.state_info(), CoreStateInfo::Locked);
    }

    #[test]
    fn events_of_a_refused_lock_are_published_too() {
        let state = ready_state(FakeCore {
            pending: vec![changed("a")],
            lock_refusal: Some(ApiError::invalid("appLock", "not enabled")),
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        assert!(state.lock().is_err());
        assert_eq!(seen.lock().unwrap().len(), 1);
        // Drained, not queued for later.
        with_ready(&state, |core| assert!(core.pending.is_empty()));
    }

    #[test]
    fn events_the_unlocked_core_emitted_while_opening_are_published() {
        let state = locked_state(FakeCore {
            passphrase: "pw".into(),
            unlock_events: vec![changed("opened.a"), changed("opened.b")],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);

        // A wrong passphrase publishes nothing.
        assert!(state.unlock("nope").is_err());
        assert!(seen.lock().unwrap().is_empty());

        state.unlock("pw").unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
        // And they are not delivered a second time by the next request.
        state.dispatch(Request::AppStatus).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_core_that_panics_while_locking_poisons_the_state() {
        let state = ready_state(FakeCore {
            panic_on_lock: true,
            ..FakeCore::default()
        });
        assert!(catch_unwind(AssertUnwindSafe(|| state.lock())).is_err());

        assert_eq!(
            state.dispatch(Request::AppStatus).unwrap_err().code,
            ErrorCode::Internal
        );
        assert_eq!(
            state.lock().unwrap_err().code,
            ErrorCode::Internal,
            "the state must not pretend to be locked"
        );
        assert!(matches!(state.state_info(), CoreStateInfo::Failed(_)));
    }

    #[test]
    fn state_changes_are_logged_and_the_passphrase_never_is() {
        let state = ready_state(FakeCore {
            passphrase: "swordfish-123".into(),
            ..FakeCore::default()
        });
        let logs = captured_logs(|| {
            state.lock().unwrap();
            let _ = state.unlock("hunter2-wrong").unwrap_err();
            state.unlock("swordfish-123").unwrap();
            let _ = state.unlock("swordfish-123").unwrap_err();
        });

        assert!(logs.contains("core state changed"), "{logs}");
        assert!(logs.contains("command=\"lock\""), "{logs}");
        assert!(logs.contains("command=\"unlock\""), "{logs}");
        assert!(logs.contains("from=Ready to=Locked"), "{logs}");
        assert!(logs.contains("from=Locked to=Ready"), "{logs}");
        assert!(logs.contains("state change refused"), "{logs}");
        assert!(!logs.contains("swordfish"), "passphrase in the log: {logs}");
        assert!(!logs.contains("hunter2"), "passphrase in the log: {logs}");
    }

    // --- backups ---

    #[test]
    fn a_backup_export_streams_the_core_s_pieces_to_the_sink_in_order() {
        let state = ready_state(FakeCore {
            backup_pieces: vec![b"KBB1".to_vec(), vec![1, 2, 3], vec![255]],
            ..FakeCore::default()
        });
        let mut written = Vec::new();
        let mut sink = |piece: &[u8]| -> Result<(), String> {
            written.extend_from_slice(piece);
            Ok(())
        };

        let summary = state.backup_export(&mut sink).unwrap();

        assert_eq!(written, b"KBB1\x01\x02\x03\xff");
        assert_eq!(summary.bytes, 8);
        assert_eq!(summary.documents, 2);
        with_ready(&state, |core| assert_eq!(core.backup_calls, 1));
    }

    #[test]
    fn a_failing_sink_aborts_the_export_with_the_core_s_error() {
        let state = ready_state(FakeCore {
            backup_pieces: vec![vec![1], vec![2], vec![3]],
            ..FakeCore::default()
        });
        let mut calls = 0;
        let mut sink = |_: &[u8]| -> Result<(), String> {
            calls += 1;
            Err("the disk is full".to_string())
        };

        let error = state.backup_export(&mut sink).unwrap_err();

        assert_eq!(error, ApiError::storage("the disk is full"));
        assert_eq!(calls, 1, "the core stops at the first failure");
    }

    #[test]
    fn a_backup_import_reads_the_source_to_the_end_and_passes_the_kit() {
        let state = ready_state(FakeCore::default());
        let data = b"a backup file longer than the fake core's four byte buffer".to_vec();
        let mut offset = 0;
        let mut source = |buffer: &mut [u8]| -> Result<usize, String> {
            let count = buffer.len().min(data.len() - offset);
            buffer[..count].copy_from_slice(&data[offset..offset + count]);
            offset += count;
            Ok(count)
        };

        let summary = state
            .backup_import(&mut source, Some("legal winner thank year"))
            .unwrap();

        assert_eq!(summary.bytes, data.len() as u64);
        with_ready(&state, |core| {
            assert_eq!(core.imported, data);
            assert_eq!(core.kit.as_deref(), Some("legal winner thank year"));
        });
    }

    #[test]
    fn a_backup_import_without_a_kit_passes_none() {
        let state = ready_state(FakeCore::default());
        let mut source = |_: &mut [u8]| -> Result<usize, String> { Ok(0) };
        state.backup_import(&mut source, None).unwrap();
        with_ready(&state, |core| assert_eq!(core.kit, None));
    }

    #[test]
    fn backup_calls_are_refused_while_the_core_is_opening_and_after_it_failed() {
        let state: AppState<FakeCore> = AppState::new();
        let mut sink = |_: &[u8]| -> Result<(), String> { Ok(()) };
        let mut source = |_: &mut [u8]| -> Result<usize, String> { Ok(0) };
        let starting = ApiError::internal("core is starting");
        assert_eq!(state.backup_export(&mut sink).unwrap_err(), starting);
        assert_eq!(
            state.backup_import(&mut source, None).unwrap_err(),
            starting
        );

        let failure = ApiError::keystore_unavailable("no keystore");
        state.set_failed(failure.clone());
        assert_eq!(state.backup_export(&mut sink).unwrap_err(), failure);
        assert_eq!(state.backup_import(&mut source, None).unwrap_err(), failure);
    }

    #[test]
    fn a_backup_import_forwards_the_events_the_core_emitted() {
        let state = ready_state(FakeCore {
            emit: vec![
                Event::WorkspaceCreated {
                    id: WorkspaceId::from_bytes([3; 16]),
                },
                Event::SecurityChanged,
            ],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        let mut source = |_: &mut [u8]| -> Result<usize, String> { Ok(0) };

        state.backup_import(&mut source, Some("kit")).unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                r#"{"type":"WorkspaceCreated","data":{"id":"03030303-0303-0303-0303-030303030303"}}"#
                    .to_string(),
                r#"{"type":"SecurityChanged"}"#.to_string(),
            ]
        );
    }

    #[test]
    fn errors_from_backup_calls_pass_through_untouched() {
        let error = ApiError::crypto("the backup is damaged or the kit is wrong");
        let state = ready_state(FakeCore {
            fail_with: Some(error.clone()),
            ..FakeCore::default()
        });
        let mut sink = |_: &[u8]| -> Result<(), String> { Ok(()) };
        let mut source = |_: &mut [u8]| -> Result<usize, String> { Ok(0) };
        assert_eq!(state.backup_export(&mut sink).unwrap_err(), error);
        assert_eq!(
            state.backup_import(&mut source, Some("kit")).unwrap_err(),
            error
        );
    }

    #[test]
    fn backup_calls_log_sizes_and_codes_and_never_the_kit_or_the_content() {
        let state = ready_state(FakeCore {
            backup_pieces: vec![b"SECRET payload".to_vec()],
            ..FakeCore::default()
        });
        let logs = captured_logs(|| {
            let mut sink = |_: &[u8]| -> Result<(), String> { Ok(()) };
            state.backup_export(&mut sink).unwrap();
            let mut source = |_: &mut [u8]| -> Result<usize, String> { Ok(0) };
            state
                .backup_import(&mut source, Some("SECRET kit words"))
                .unwrap();
        });
        assert!(logs.contains("kind=\"backup_export\""), "{logs}");
        assert!(logs.contains("kind=\"backup_import\""), "{logs}");
        assert!(logs.contains("bytes=14"), "{logs}");
        assert!(!logs.contains("SECRET"), "secret in the log: {logs}");
    }

    // --- attachments (the binary API) ---

    #[test]
    fn attachment_calls_are_refused_while_the_core_is_opening_and_after_it_failed() {
        let state: AppState<FakeCore> = AppState::new();
        let starting = ApiError::internal("core is starting");
        let mut sink = |_: &[u8]| Ok(());
        assert_eq!(
            state.attachment_write(attachment(1), b"x").unwrap_err(),
            starting
        );
        assert_eq!(
            state
                .attachment_read(attachment(1), None, &mut sink)
                .unwrap_err(),
            starting
        );

        let failure = ApiError::keystore_unavailable("no usable keystore");
        state.set_failed(failure.clone());
        assert_eq!(
            state.attachment_write(attachment(1), b"x").unwrap_err(),
            failure
        );
        assert_eq!(
            state
                .attachment_read(attachment(1), None, &mut sink)
                .unwrap_err(),
            failure
        );
    }

    #[test]
    fn a_locked_core_answers_attachment_calls_with_locked_and_sees_nothing() {
        let state = locked_state(FakeCore::default());
        let mut sink = |_: &[u8]| Ok(());
        assert_eq!(
            state.attachment_write(attachment(1), b"x").unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(
            state
                .attachment_read(
                    attachment(1),
                    Some(ByteRange { start: 0, end: 1 }),
                    &mut sink
                )
                .unwrap_err(),
            ApiError::locked()
        );
        // Unlocking gives back the very same core, which was never asked anything.
        state.unlock("").unwrap();
        assert!(attachment_calls(&state).is_empty());
    }

    #[test]
    fn writing_passes_the_id_and_the_bytes_through_untouched() {
        let state = ready_state(FakeCore::default());
        state.attachment_write(attachment(4), &[0, 255, 7]).unwrap();
        assert_eq!(
            attachment_calls(&state),
            vec![AttachmentCall::Write(attachment(4), vec![0, 255, 7])]
        );
    }

    #[test]
    fn reading_streams_the_core_s_pieces_to_the_sink_in_order_and_returns_the_info() {
        let state = ready_state(FakeCore {
            attachment_pieces: vec![vec![1, 2, 3], vec![4], vec![5, 6]],
            ..FakeCore::default()
        });
        let mut received = Vec::new();
        let mut sink = |piece: &[u8]| {
            received.push(piece.to_vec());
            Ok(())
        };

        let info = state
            .attachment_read(
                attachment(8),
                Some(ByteRange { start: 2, end: 8 }),
                &mut sink,
            )
            .unwrap();

        assert_eq!(received, vec![vec![1, 2, 3], vec![4], vec![5, 6]]);
        assert_eq!(info, attachment_info(attachment(8), 6));
        assert_eq!(
            attachment_calls(&state),
            vec![AttachmentCall::Read(
                attachment(8),
                Some(ByteRange { start: 2, end: 8 })
            )]
        );
    }

    #[test]
    fn a_whole_read_passes_no_range() {
        let state = ready_state(FakeCore::default());
        let mut sink = |_: &[u8]| Ok(());
        state
            .attachment_read(attachment(1), None, &mut sink)
            .unwrap();
        assert_eq!(
            attachment_calls(&state),
            vec![AttachmentCall::Read(attachment(1), None)]
        );
    }

    #[test]
    fn a_sink_that_fails_stops_the_read_with_its_error() {
        let state = ready_state(FakeCore {
            attachment_pieces: vec![vec![1], vec![2], vec![3]],
            ..FakeCore::default()
        });
        let mut pieces = 0;
        let mut sink = |_: &[u8]| {
            pieces += 1;
            Err(ApiError::storage("disk full"))
        };
        let error = state
            .attachment_read(attachment(1), None, &mut sink)
            .unwrap_err();
        assert_eq!(error, ApiError::storage("disk full"));
        assert_eq!(pieces, 1);
    }

    #[test]
    fn errors_from_attachment_calls_pass_through_untouched() {
        let error = ApiError::not_found("no such attachment");
        let state = ready_state(FakeCore {
            fail_with: Some(error.clone()),
            ..FakeCore::default()
        });
        let mut sink = |_: &[u8]| Ok(());
        assert_eq!(
            state.attachment_write(attachment(1), b"x").unwrap_err(),
            error
        );
        assert_eq!(
            state
                .attachment_read(attachment(1), None, &mut sink)
                .unwrap_err(),
            error
        );
    }

    #[test]
    fn attachment_events_reach_subscribers_in_order_as_json() {
        let state = ready_state(FakeCore {
            emit: vec![
                Event::AttachmentsChanged {
                    space_id: kb_types::SpaceId::from_bytes([1; 16]),
                },
                changed("a"),
            ],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        let mut sink = |_: &[u8]| Ok(());

        state.attachment_write(attachment(1), b"x").unwrap();
        state
            .attachment_read(attachment(1), None, &mut sink)
            .unwrap();

        let events = seen.lock().unwrap().clone();
        assert_eq!(events.len(), 4, "{events:?}");
        assert_eq!(
            events[0],
            r#"{"type":"AttachmentsChanged","data":{"spaceId":"01010101-0101-0101-0101-010101010101"}}"#
        );
        assert_eq!(events[1], r#"{"type":"SettingChanged","data":{"key":"a"}}"#);
    }

    #[test]
    fn events_of_a_failing_attachment_call_are_still_delivered() {
        let state = ready_state(FakeCore {
            emit: vec![changed("a")],
            fail_with: Some(ApiError::storage("disk full")),
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        assert!(state.attachment_write(attachment(1), b"x").is_err());
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn attachment_calls_log_sizes_and_codes_and_never_the_bytes() {
        let state = ready_state(FakeCore {
            attachment_pieces: vec![b"SECRET plaintext".to_vec()],
            ..FakeCore::default()
        });
        let logs = captured_logs(|| {
            state
                .attachment_write(attachment(1), b"SECRET piece")
                .unwrap();
            let mut sink = |_: &[u8]| Ok(());
            state
                .attachment_read(attachment(1), None, &mut sink)
                .unwrap();
            let mut failing = |_: &[u8]| Err(ApiError::storage("SECRET reason"));
            let _ = state.attachment_read(attachment(1), None, &mut failing);
        });
        assert!(logs.contains("kind=\"attachment_write\""), "{logs}");
        assert!(logs.contains("kind=\"attachment_read\""), "{logs}");
        assert!(logs.contains("bytes=12"), "{logs}");
        assert!(logs.contains("bytes=16"), "{logs}");
        assert!(logs.contains("code=Storage"), "{logs}");
        assert!(!logs.contains("SECRET"), "content in the log: {logs}");
    }

    // --- sync: the core's sync seam, the Rust listeners and the background driver ---

    impl FakeCore {
        /// Records a sync call; anything but the status and the state report is not
        /// scripted (the driver tests in `kb-net` cover the cycle).
        fn sync<T>(&mut self, name: &'static str) -> Result<T, ApiError> {
            self.sync_calls.push(name);
            Err(ApiError::internal("not scripted"))
        }
    }

    impl SyncCoreApi for FakeCore {
        fn sync_info(&mut self) -> Result<SyncInfo, ApiError> {
            self.sync_calls.push("sync_info");
            Ok(SyncInfo {
                configured: false,
                server_url: None,
                state: SyncState::Disabled,
                pending_entries: 0,
                parked_items: 0,
                remote_cursor: 0,
                server_epoch: None,
                last_push_at: None,
                last_pull_at: None,
                last_error: None,
                fork_detected: false,
                devices_seen: 0,
                needs_invite_token: false,
                transfers: kb_types::TransferInfo::default(),
                feeds: Vec::new(),
            })
        }
        fn sync_identity(&mut self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
            self.sync("sync_identity")
        }
        fn sync_take_invite_token(&mut self) -> Result<Option<String>, ApiError> {
            self.sync("sync_take_invite_token")
        }
        fn sync_account_unknown(&mut self) -> Result<(), ApiError> {
            self.sync("sync_account_unknown")
        }
        fn sync_log_received(&mut self, _: &[Vec<u8>]) -> Result<bool, ApiError> {
            self.sync("sync_log_received")
        }
        fn sync_account_log(&mut self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
            self.sync("sync_account_log")
        }
        fn sync_log_published(&mut self, _: u64) -> Result<(), ApiError> {
            self.sync("sync_log_published")
        }
        fn sync_sign(&mut self, _: &[u8]) -> Result<Signature, ApiError> {
            self.sync("sync_sign")
        }
        fn sync_remote_epoch(&mut self, _: [u8; 16]) -> Result<bool, ApiError> {
            self.sync("sync_remote_epoch")
        }
        fn sync_set_state(&mut self, _: SyncState, _: Option<String>) -> Result<(), ApiError> {
            // Like the core: every state change is announced.
            self.sync_calls.push("sync_set_state");
            self.pending.push(Event::SyncChanged);
            Ok(())
        }
        fn sync_recovery_blob(&mut self) -> Result<Option<([u8; 32], Vec<u8>)>, ApiError> {
            self.sync("sync_recovery_blob")
        }
        fn sync_recovery_published(&mut self) -> Result<(), ApiError> {
            self.sync("sync_recovery_published")
        }
        fn sync_recovery_obsolete(&mut self) -> Result<Vec<[u8; 32]>, ApiError> {
            self.sync("sync_recovery_obsolete")
        }
        fn sync_recovery_deleted(&mut self, _: [u8; 32]) -> Result<(), ApiError> {
            self.sync("sync_recovery_deleted")
        }
        fn sync_feeds(&mut self) -> Result<Vec<FeedWork>, ApiError> {
            self.sync("sync_feeds")
        }
        fn sync_needed_logs(&mut self) -> Result<Vec<LogRequest>, ApiError> {
            self.sync("sync_needed_logs")
        }
        fn sync_foreign_log_received(
            &mut self,
            _: AccountId,
            _: &[Vec<u8>],
        ) -> Result<bool, ApiError> {
            self.sync("sync_foreign_log_received")
        }
        fn sync_membership_pending(&mut self, _: FeedId) -> Result<Vec<Vec<u8>>, ApiError> {
            self.sync("sync_membership_pending")
        }
        fn sync_membership_posted(&mut self, _: FeedId, _: MembershipHead) -> Result<(), ApiError> {
            self.sync("sync_membership_posted")
        }
        fn sync_membership_conflict(&mut self, _: FeedId) -> Result<(), ApiError> {
            self.sync("sync_membership_conflict")
        }
        fn sync_membership_wanted(&mut self, _: FeedId) -> Result<Option<Option<u64>>, ApiError> {
            self.sync("sync_membership_wanted")
        }
        fn sync_membership_fetched(
            &mut self,
            _: FeedId,
            _: &MembershipPage,
        ) -> Result<(), ApiError> {
            self.sync("sync_membership_fetched")
        }
        fn sync_feed_lost(&mut self, _: FeedId) -> Result<(), ApiError> {
            self.sync("sync_feed_lost")
        }
        fn sync_feed_refused(&mut self, _: FeedId, _: &str) -> Result<(), ApiError> {
            self.sync("sync_feed_refused")
        }
        fn sync_feed_rekey_needed(&mut self, _: FeedId) -> Result<(), ApiError> {
            self.sync("sync_feed_rekey_needed")
        }
        fn sync_feed_outbox(&mut self, _: FeedId) -> Result<Vec<OutboundEntry>, ApiError> {
            self.sync("sync_feed_outbox")
        }
        fn sync_feed_pushed(
            &mut self,
            _: FeedId,
            _: DocId,
            _: u64,
            _: u64,
            _: i64,
        ) -> Result<(), ApiError> {
            self.sync("sync_feed_pushed")
        }
        fn sync_feed_apply(&mut self, _: FeedId, _: &ChangesPage) -> Result<SyncApplied, ApiError> {
            self.sync("sync_feed_apply")
        }
        fn sync_feed_snapshot_candidates(&mut self, _: FeedId) -> Result<Vec<DocId>, ApiError> {
            self.sync("sync_feed_snapshot_candidates")
        }
        fn sync_feed_make_snapshot(
            &mut self,
            _: FeedId,
            _: DocId,
        ) -> Result<(Vec<u8>, u64), ApiError> {
            self.sync("sync_feed_make_snapshot")
        }
        fn sync_feed_snapshot_published(
            &mut self,
            _: FeedId,
            _: DocId,
            _: u64,
        ) -> Result<(), ApiError> {
            self.sync("sync_feed_snapshot_published")
        }
        fn sync_feed_apply_snapshots(
            &mut self,
            _: FeedId,
            _: &SnapshotsPage,
        ) -> Result<SnapshotsApplied, ApiError> {
            self.sync("sync_feed_apply_snapshots")
        }
        fn sync_snapshot_marks_wanted(&mut self, _: FeedId) -> Result<Option<u64>, ApiError> {
            self.sync("sync_snapshot_marks_wanted")
        }
        fn sync_snapshot_marks_received(
            &mut self,
            _: FeedId,
            _: &[SnapshotMark],
        ) -> Result<Vec<DocId>, ApiError> {
            self.sync("sync_snapshot_marks_received")
        }
        fn sync_snapshot_verified(&mut self, _: FeedId, _: DocId, _: bool) -> Result<(), ApiError> {
            self.sync("sync_snapshot_verified")
        }
        fn sync_feed_head(&mut self, _: FeedId) -> Result<Vec<u8>, ApiError> {
            self.sync("sync_feed_head")
        }
        fn sync_feed_heads_received(&mut self, _: FeedId, _: &[Vec<u8>]) -> Result<bool, ApiError> {
            self.sync("sync_feed_heads_received")
        }
        fn sync_feed_chunk_work(
            &mut self,
            _: FeedId,
            limit: usize,
        ) -> Result<Vec<FeedChunkWork>, ApiError> {
            self.sync_calls.push("sync_feed_chunk_work");
            Ok(self.chunk_work.iter().take(limit).copied().collect())
        }
        fn sync_feed_chunk_bytes(
            &mut self,
            _: FeedId,
            hash: ChunkId,
        ) -> Result<Option<Vec<u8>>, ApiError> {
            self.sync_calls.push("sync_feed_chunk_bytes");
            self.chunk_calls.push(("bytes", hash, None));
            Ok(self.chunk_blob.clone())
        }
        fn sync_feed_chunk_uploaded(&mut self, _: FeedId, hash: ChunkId) -> Result<(), ApiError> {
            self.sync_calls.push("sync_feed_chunk_uploaded");
            self.chunk_calls.push(("uploaded", hash, None));
            Ok(())
        }
        fn sync_feed_chunk_received(
            &mut self,
            _: FeedId,
            hash: ChunkId,
            bytes: Vec<u8>,
        ) -> Result<(), ApiError> {
            self.sync_calls.push("sync_feed_chunk_received");
            self.chunk_calls.push(("received", hash, Some(bytes)));
            // Like the core: a chunk that arrives changes its attachment.
            self.pending.push(Event::AttachmentsChanged {
                space_id: SpaceId::from_bytes([7; 16]),
            });
            Ok(())
        }
        fn sync_feed_chunk_unavailable(
            &mut self,
            _: FeedId,
            hash: ChunkId,
        ) -> Result<(), ApiError> {
            self.sync_calls.push("sync_feed_chunk_unavailable");
            self.chunk_calls.push(("unavailable", hash, None));
            Ok(())
        }
        fn sync_feed_chunk_deleted(&mut self, _: FeedId, hash: ChunkId) -> Result<(), ApiError> {
            self.sync_calls.push("sync_feed_chunk_deleted");
            self.chunk_calls.push(("deleted", hash, None));
            // A refusal passes through like any other.
            Err(ApiError::crypto("SECRET reason"))
        }
        fn sync_feed_apply_inline(
            &mut self,
            _: FeedId,
            _: &InlineEntry,
        ) -> Result<InlineOutcome, ApiError> {
            self.sync_calls.push("sync_feed_apply_inline");
            Ok(InlineOutcome::Known)
        }
        fn sync_push_debounce_ms(&mut self) -> Result<u64, ApiError> {
            self.sync_calls.push("sync_push_debounce_ms");
            Ok(150)
        }
        fn presence_open(
            &mut self,
            feed: FeedId,
            envelope: &[u8],
        ) -> Result<Option<PresenceMessage>, ApiError> {
            self.sync_calls.push("presence_open");
            self.opened_frames.push((feed, envelope.to_vec()));
            // Like the core: a presence that appeared is announced.
            let accepted = self.opened.pop_front();
            if accepted.is_some() {
                self.pending.push(Event::PresenceChanged {
                    space_id: SpaceId::from_bytes([3; 16]),
                });
            }
            Ok(accepted)
        }
        fn presence_expire(&mut self) -> Result<(), ApiError> {
            self.sync_calls.push("presence_expire");
            Ok(())
        }
        fn sync_link_work(&mut self) -> Result<Vec<LinkWork>, ApiError> {
            self.sync_calls.push("sync_link_work");
            Ok(std::mem::take(&mut self.link_work))
        }
        fn sync_link_done(
            &mut self,
            link_id: LinkId,
            outcome: LinkOutcome,
        ) -> Result<(), ApiError> {
            self.sync_calls.push("sync_link_done");
            self.link_done.push((link_id, outcome));
            Ok(())
        }
    }

    impl InviteCoreApi for FakeCore {
        fn invite_info(&mut self) -> Result<InviteSessionInfo, ApiError> {
            self.sync_calls.push("invite_info");
            let (side, state, _) =
                self.invite
                    .unwrap_or((InviteSide::Invitee, InviteSessionState::Idle, None));
            Ok(InviteSessionInfo {
                flow: InviteFlow::Code,
                side,
                state,
                code: None,
                sas: None,
                peer_name: None,
                peer_safety_number: None,
                space_name: None,
                inviter_name: None,
                role: None,
                verified: false,
                server_url: self.device_server.clone(),
                error: None,
                expires_at: None,
            })
        }
        fn invite_mailbox_id(&mut self) -> Result<Option<[u8; 6]>, ApiError> {
            self.sync_calls.push("invite_mailbox_id");
            Ok(self.invite.and_then(|(_, _, mailbox)| mailbox))
        }
        fn invite_mailbox_opened(&mut self, _: [u8; 6], _: i64) -> Result<(), ApiError> {
            self.sync("invite_mailbox_opened")
        }
        fn invite_outbox(&mut self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
            self.sync_calls.push("invite_outbox");
            Ok(Vec::new())
        }
        fn invite_posted(&mut self, _: u32) -> Result<(), ApiError> {
            self.sync("invite_posted")
        }
        fn invite_receive(&mut self, _: u32, _: InviteSide, _: &[u8]) -> Result<(), ApiError> {
            self.sync("invite_receive")
        }
        fn invite_failed(&mut self, _: &str) -> Result<(), ApiError> {
            self.sync_calls.push("invite_failed");
            if let Some(invite) = self.invite.as_mut() {
                invite.1 = InviteSessionState::Failed;
            }
            Ok(())
        }
        fn invite_membership_pending(&mut self) -> Result<Option<PendingMembership>, ApiError> {
            self.sync("invite_membership_pending")
        }
        fn invite_membership_posted(&mut self, _: MembershipHead) -> Result<(), ApiError> {
            self.sync("invite_membership_posted")
        }
        fn invite_signup(&mut self, _: [u8; 16]) -> Result<Option<InviteSignup>, ApiError> {
            self.sync("invite_signup")
        }
        fn invite_open_request(
            &mut self,
            _: [u8; 16],
        ) -> Result<Option<InviteOpenRequest>, ApiError> {
            self.sync("invite_open_request")
        }
        fn invite_opened(&mut self, _: &InviteOpened) -> Result<(), ApiError> {
            self.sync("invite_opened")
        }
        fn invite_join(&mut self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError> {
            self.sync("invite_join")
        }
        fn invite_joined(&mut self, _: MembershipHead) -> Result<(), ApiError> {
            self.sync("invite_joined")
        }
    }

    impl DevicesCoreApi for FakeCore {
        fn pairing_info(&mut self) -> Result<PairingInfo, ApiError> {
            self.sync_calls.push("pairing_info");
            let (role, state, _) =
                self.pairing
                    .unwrap_or((PairingRole::Joiner, PairingState::Idle, None));
            Ok(PairingInfo {
                role,
                state,
                code: None,
                sas: None,
                peer_name: None,
                server_url: self.device_server.clone(),
                error: None,
                expires_at: None,
            })
        }
        fn pairing_mailbox_id(&mut self) -> Result<Option<[u8; 6]>, ApiError> {
            self.sync_calls.push("pairing_mailbox_id");
            Ok(self.pairing.and_then(|(_, _, mailbox)| mailbox))
        }
        fn pairing_mailbox_opened(&mut self, _: [u8; 6], _: i64) -> Result<(), ApiError> {
            self.sync("pairing_mailbox_opened")
        }
        fn pairing_outbox(&mut self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
            self.sync_calls.push("pairing_outbox");
            Ok(Vec::new())
        }
        fn pairing_posted(&mut self, _: u32) -> Result<(), ApiError> {
            self.sync("pairing_posted")
        }
        fn pairing_receive(&mut self, _: u32, _: PairingRole, _: &[u8]) -> Result<(), ApiError> {
            self.sync("pairing_receive")
        }
        fn pairing_failed(&mut self, _: &str) -> Result<(), ApiError> {
            self.sync_calls.push("pairing_failed");
            if let Some(pairing) = self.pairing.as_mut() {
                pairing.1 = PairingState::Failed;
            }
            Ok(())
        }
        fn recovery_info(&mut self) -> Result<RecoveryInfo, ApiError> {
            self.sync_calls.push("recovery_info");
            Ok(RecoveryInfo {
                state: self.recovery.unwrap_or(RecoveryState::Idle),
                server_url: self.device_server.clone(),
                error: None,
            })
        }
        fn recovery_id(&mut self) -> Result<Option<[u8; 32]>, ApiError> {
            self.sync_calls.push("recovery_id");
            Ok(None)
        }
        fn recovery_blob_received(&mut self, _: &[u8]) -> Result<(), ApiError> {
            self.sync("recovery_blob_received")
        }
        fn recovery_log_received(&mut self, _: &[Vec<u8>]) -> Result<Vec<u8>, ApiError> {
            self.sync("recovery_log_received")
        }
        fn recovery_entry_published(&mut self) -> Result<(), ApiError> {
            self.sync("recovery_entry_published")
        }
        fn recovery_failed(&mut self, _: &str) -> Result<(), ApiError> {
            self.sync_calls.push("recovery_failed");
            self.recovery = Some(RecoveryState::Failed);
            Ok(())
        }
        fn recovery_account_id(&mut self) -> Result<Option<AccountId>, ApiError> {
            self.sync("recovery_account_id")
        }
    }

    /// How often a fake background task was started and stopped.
    #[derive(Default)]
    struct Runs {
        started: AtomicUsize,
        stopped: AtomicUsize,
    }

    /// A fake background task: counts its stop when dropped.
    struct Running(Arc<Runs>);

    impl Drop for Running {
        fn drop(&mut self) {
            self.0.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_starter(runs: &Arc<Runs>) -> BackgroundStarter {
        let runs = Arc::clone(runs);
        Box::new(move || {
            runs.started.fetch_add(1, Ordering::SeqCst);
            Some(Box::new(Running(Arc::clone(&runs))) as BackgroundTask)
        })
    }

    /// `(started, stopped)`.
    fn counts(runs: &Runs) -> (usize, usize) {
        (
            runs.started.load(Ordering::SeqCst),
            runs.stopped.load(Ordering::SeqCst),
        )
    }

    fn feed() -> FeedId {
        FeedId::from_bytes([1; 16])
    }

    fn sync_calls(state: &AppState<FakeCore>) -> Vec<&'static str> {
        match &*state.core.lock().unwrap() {
            CoreState::Ready(core) => core.sync_calls.clone(),
            CoreState::Locked(locked) => locked.core.sync_calls.clone(),
            _ => Vec::new(),
        }
    }

    /// Waits (in real time: the driver runs on Tauri's runtime) for `condition`.
    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "never happened: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn background_work_runs_while_the_core_is_ready_and_stops_while_it_is_locked() {
        let runs = Arc::new(Runs::default());
        let state: AppState<FakeCore> = AppState::new();
        state.set_background(counting_starter(&runs));
        assert_eq!(counts(&runs), (0, 0), "nothing runs while the core opens");

        state.set_ready(FakeCore {
            passphrase: "pw".into(),
            ..FakeCore::default()
        });
        assert_eq!(counts(&runs), (1, 0));
        assert!(state.background_running());

        state.lock().unwrap();
        assert_eq!(counts(&runs), (1, 1));
        assert!(!state.background_running());

        // A wrong passphrase starts nothing; the right one starts it again.
        assert!(state.unlock("wrong").is_err());
        assert_eq!(counts(&runs), (1, 1));
        state.unlock("pw").unwrap();
        assert_eq!(counts(&runs), (2, 1));

        // Locking twice stops once.
        state.lock().unwrap();
        state.lock().unwrap();
        assert_eq!(counts(&runs), (2, 2));
    }

    #[test]
    fn a_refused_lock_keeps_the_background_work_running() {
        let runs = Arc::new(Runs::default());
        let state = ready_state(FakeCore {
            lock_refusal: Some(ApiError::invalid("appLock", "not enabled")),
            ..FakeCore::default()
        });
        state.set_background(counting_starter(&runs));
        assert_eq!(counts(&runs), (1, 0), "a ready core starts it at once");
        assert!(state.lock().is_err());
        assert_eq!(counts(&runs), (1, 0));
        assert!(state.background_running());
    }

    #[test]
    fn a_workspace_that_opens_locked_starts_its_background_work_at_unlock() {
        let runs = Arc::new(Runs::default());
        let state: AppState<FakeCore> = AppState::new();
        state.set_background(counting_starter(&runs));
        state.set_locked(FakeLocked {
            core: FakeCore {
                passphrase: "pw".into(),
                ..FakeCore::default()
            },
            attempts: 0,
        });
        assert_eq!(counts(&runs), (0, 0));
        state.unlock("pw").unwrap();
        assert_eq!(counts(&runs), (1, 0));
    }

    #[test]
    fn a_core_that_fails_stops_its_background_work() {
        let runs = Arc::new(Runs::default());
        let state = ready_state(FakeCore::default());
        state.set_background(counting_starter(&runs));
        state.set_failed(ApiError::internal("boom"));
        assert_eq!(counts(&runs), (1, 1));
    }

    #[test]
    fn registering_new_background_work_replaces_the_running_one() {
        let first = Arc::new(Runs::default());
        let second = Arc::new(Runs::default());
        let state = ready_state(FakeCore::default());
        state.set_background(counting_starter(&first));
        state.set_background(counting_starter(&second));
        assert_eq!(counts(&first), (1, 1));
        assert_eq!(counts(&second), (1, 0));
    }

    #[test]
    fn a_starter_with_nothing_to_run_leaves_nothing_running() {
        let state = ready_state(FakeCore::default());
        state.set_background(Box::new(|| None));
        assert!(!state.background_running());
        state.lock().unwrap();
        state.unlock("").unwrap();
        assert!(!state.background_running());
    }

    #[test]
    fn rust_listeners_hear_every_event_and_survive_page_reloads() {
        let state = ready_state(FakeCore {
            emit: vec![changed("a")],
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        let heard: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink = Arc::clone(&heard);
        let unsubscribe = state.add_listener(Arc::new(move |event: &Event| {
            sink.lock().unwrap().push(event.clone());
        }));
        assert_eq!(state.listener_count(), 1);

        state.dispatch(Request::AppStatus).unwrap();
        assert_eq!(*heard.lock().unwrap(), [changed("a")]);
        assert_eq!(seen.lock().unwrap().len(), 1);

        // A page load forgets the webview's channels, not the Rust listeners.
        state.clear_subscribers();
        state.dispatch(Request::AppStatus).unwrap();
        assert_eq!(heard.lock().unwrap().len(), 2);

        unsubscribe();
        assert_eq!(state.listener_count(), 0);
        state.dispatch(Request::AppStatus).unwrap();
        assert_eq!(heard.lock().unwrap().len(), 2);
    }

    #[test]
    fn sync_host_calls_take_the_lock_and_forward_what_the_core_emits() {
        let state = ready_state(FakeCore::default());
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        let heard: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink = Arc::clone(&heard);
        let _unsubscribe = kb_net::v2::SyncHost::subscribe(
            &state,
            Box::new(move |event: &Event| sink.lock().unwrap().push(event.clone())),
        );

        assert!(!kb_net::v2::SyncHost::sync_info(&state).unwrap().configured);
        kb_net::v2::SyncHost::sync_set_state(&state, SyncState::Idle, None).unwrap();
        assert_eq!(*heard.lock().unwrap(), [Event::SyncChanged]);
        assert_eq!(
            *seen.lock().unwrap(),
            [r#"{"type":"SyncChanged"}"#.to_string()]
        );
        // The core's own error passes through untouched.
        assert_eq!(
            kb_net::v2::SyncHost::sync_feed_outbox(&state, feed()).unwrap_err(),
            ApiError::internal("not scripted")
        );
        assert_eq!(
            sync_calls(&state),
            ["sync_info", "sync_set_state", "sync_feed_outbox"]
        );

        // A locked core is never reached.
        state.lock().unwrap();
        assert_eq!(
            kb_net::v2::SyncHost::sync_info(&state).unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(sync_calls(&state).len(), 3);
    }

    #[test]
    fn chunk_host_calls_take_the_lock_and_reach_the_core() {
        let hash = ChunkId::from_bytes([0xab; 32]);
        let item = FeedChunkWork {
            feed: feed(),
            hash,
            attachment_id: AttachmentId::from_bytes([3; 16]),
            kind: FeedChunkKind::Upload,
        };
        let state = ready_state(FakeCore {
            chunk_work: vec![item; 3],
            chunk_blob: Some(b"sealed".to_vec()),
            ..FakeCore::default()
        });
        let (channel, seen) = recording_channel();
        state.subscribe(channel);
        let heard: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink = Arc::clone(&heard);
        let _unsubscribe = kb_net::v2::SyncHost::subscribe(
            &state,
            Box::new(move |event: &Event| sink.lock().unwrap().push(event.clone())),
        );

        let logs = captured_logs(|| {
            assert_eq!(
                kb_net::v2::SyncHost::sync_feed_chunk_work(&state, feed(), 2).unwrap(),
                [item, item]
            );
            assert_eq!(
                kb_net::v2::SyncHost::sync_feed_chunk_bytes(&state, feed(), hash).unwrap(),
                Some(b"sealed".to_vec())
            );
            kb_net::v2::SyncHost::sync_feed_chunk_uploaded(&state, feed(), hash).unwrap();
            kb_net::v2::SyncHost::sync_feed_chunk_received(
                &state,
                feed(),
                hash,
                b"downloaded".to_vec(),
            )
            .unwrap();
            kb_net::v2::SyncHost::sync_feed_chunk_unavailable(&state, feed(), hash).unwrap();
            // The core's own error passes through untouched.
            assert_eq!(
                kb_net::v2::SyncHost::sync_feed_chunk_deleted(&state, feed(), hash).unwrap_err(),
                ApiError::crypto("SECRET reason")
            );
        });
        assert_eq!(
            sync_calls(&state),
            [
                "sync_feed_chunk_work",
                "sync_feed_chunk_bytes",
                "sync_feed_chunk_uploaded",
                "sync_feed_chunk_received",
                "sync_feed_chunk_unavailable",
                "sync_feed_chunk_deleted",
            ]
        );
        let chunk_calls = match &*state.core.lock().unwrap() {
            CoreState::Ready(core) => core.chunk_calls.clone(),
            _ => Vec::new(),
        };
        assert_eq!(
            chunk_calls,
            [
                ("bytes", hash, None),
                ("uploaded", hash, None),
                ("received", hash, Some(b"downloaded".to_vec())),
                ("unavailable", hash, None),
                ("deleted", hash, None),
            ]
        );
        // What the core emitted reaches the driver and the webview before the lock goes.
        let arrived = Event::AttachmentsChanged {
            space_id: SpaceId::from_bytes([7; 16]),
        };
        assert_eq!(*heard.lock().unwrap(), [arrived]);
        assert_eq!(seen.lock().unwrap().len(), 1);
        // Method names and codes only: no hash, no bytes, no message.
        assert!(logs.contains("kind=\"sync_feed_chunk_deleted\""), "{logs}");
        assert!(logs.contains("code=Crypto"), "{logs}");
        for secret in ["abababab", "SECRET", "sealed", "downloaded"] {
            assert!(!logs.contains(secret), "{secret} in the log: {logs}");
        }

        // A locked core is never reached.
        state.lock().unwrap();
        assert_eq!(
            kb_net::v2::SyncHost::sync_feed_chunk_work(&state, feed(), 8).unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(
            kb_net::v2::SyncHost::sync_feed_chunk_received(&state, feed(), hash, Vec::new())
                .unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(sync_calls(&state).len(), 6);
    }

    #[test]
    fn the_sync_driver_runs_while_the_core_is_ready_and_stops_when_it_locks() {
        let state: Arc<AppState<FakeCore>> = Arc::new(AppState::new());
        state.set_background(driver_starter(Arc::downgrade(&state)));
        assert!(!state.background_running());

        state.set_ready(FakeCore {
            passphrase: "pw".into(),
            ..FakeCore::default()
        });
        assert!(state.background_running());
        assert_eq!(
            state.listener_count(),
            3,
            "the driver, the device tasks and the invite tasks listen to the core"
        );
        wait_until("the driver reads the configuration", || {
            sync_calls(&state).contains(&"sync_info")
        });

        state.lock().unwrap();
        assert!(!state.background_running());
        assert_eq!(state.listener_count(), 0, "the driver stopped listening");
        // The stopped driver lets go of the state.
        wait_until("the driver released the state", || {
            Arc::strong_count(&state) == 1
        });

        state.unlock("pw").unwrap();
        assert!(state.background_running());
        assert_eq!(state.listener_count(), 3);
        wait_until("the new driver reads the configuration", || {
            sync_calls(&state)
                .iter()
                .filter(|call| **call == "sync_info")
                .count()
                >= 2
        });
        // Sync is not configured: the driver never got further.
        assert!(
            sync_calls(&state).iter().all(|call| *call == "sync_info"),
            "{:?}",
            sync_calls(&state)
        );
    }

    /// Queues `event` on the ready fake core and lets a request deliver it, as the core
    /// emits `PairingRequested` from `PairingStart`.
    fn core_emits(state: &AppState<FakeCore>, event: Event) {
        if let CoreState::Ready(core) = &mut *state.core.lock().unwrap() {
            core.pending.push(event);
        }
        state.dispatch(Request::AppStatus).unwrap();
    }

    /// An address where nothing listens.
    fn dead_server() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{address}")
    }

    #[test]
    fn the_device_tasks_start_when_the_core_asks_for_them() {
        let state: Arc<AppState<FakeCore>> = Arc::new(AppState::new());
        state.set_background(driver_starter(Arc::downgrade(&state)));
        state.set_ready(FakeCore {
            // A sponsor whose code named no mailbox, and a recovery whose kit names no
            // blob: both tasks give up before any network call.
            pairing: Some((PairingRole::Sponsor, PairingState::Starting, None)),
            recovery: Some(RecoveryState::Fetching),
            device_server: Some(dead_server()),
            ..FakeCore::default()
        });
        wait_until("the driver started", || {
            sync_calls(&state).contains(&"sync_info")
        });
        assert!(
            !sync_calls(&state).contains(&"pairing_info"),
            "nothing before a request"
        );

        core_emits(&state, Event::PairingRequested);
        wait_until("the pairing task ran", || {
            sync_calls(&state).contains(&"pairing_failed")
        });
        core_emits(&state, Event::RecoveryRequested);
        wait_until("the recovery task ran", || {
            sync_calls(&state).contains(&"recovery_failed")
        });
        // Both ended with their sessions: only the long-lived listeners are left (the driver, the
        // device tasks, the invite tasks).
        wait_until("the tasks ended", || state.listener_count() == 3);
    }

    #[test]
    fn a_running_pairing_task_stops_when_the_workspace_locks() {
        let state: Arc<AppState<FakeCore>> = Arc::new(AppState::new());
        state.set_background(driver_starter(Arc::downgrade(&state)));
        state.set_ready(FakeCore {
            passphrase: "pw".into(),
            // A sponsor polling a server that does not answer: it retries for seconds.
            pairing: Some((PairingRole::Sponsor, PairingState::Starting, Some([1; 6]))),
            device_server: Some(dead_server()),
            ..FakeCore::default()
        });
        core_emits(&state, Event::PairingRequested);
        wait_until("the pairing task listens", || state.listener_count() == 4);

        state.lock().unwrap();
        assert!(!state.background_running());
        wait_until("every task stopped listening", || {
            state.listener_count() == 0
        });
        wait_until("the tasks released the state", || {
            Arc::strong_count(&state) == 1
        });
        assert!(
            !sync_calls(&state).contains(&"pairing_failed"),
            "stopped, not failed: {:?}",
            sync_calls(&state)
        );
    }

    #[test]
    fn the_device_hosts_take_the_lock_like_every_other_call() {
        let state = ready_state(FakeCore {
            pairing: Some((PairingRole::Joiner, PairingState::Waiting, Some([2; 6]))),
            ..FakeCore::default()
        });
        assert_eq!(
            kb_net::PairingHost::pairing_info(&state).unwrap().state,
            PairingState::Waiting
        );
        assert_eq!(
            kb_net::PairingHost::pairing_mailbox_id(&state).unwrap(),
            Some([2; 6])
        );
        assert_eq!(
            kb_net::RecoveryHost::recovery_info(&state).unwrap().state,
            RecoveryState::Idle
        );
        state.lock().unwrap();
        assert_eq!(
            kb_net::PairingHost::pairing_info(&state).unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(
            kb_net::RecoveryHost::recovery_id(&state).unwrap_err(),
            ApiError::locked()
        );
    }

    #[test]
    fn the_invite_task_starts_when_the_core_asks_for_it() {
        let state: Arc<AppState<FakeCore>> = Arc::new(AppState::new());
        state.set_background(driver_starter(Arc::downgrade(&state)));
        state.set_ready(FakeCore {
            // An invitee whose code named no mailbox: the task gives up before any network call.
            invite: Some((InviteSide::Invitee, InviteSessionState::Starting, None)),
            device_server: Some(dead_server()),
            ..FakeCore::default()
        });
        wait_until("the driver started", || {
            sync_calls(&state).contains(&"sync_info")
        });
        assert!(
            !sync_calls(&state).contains(&"invite_info"),
            "nothing before a request"
        );

        core_emits(&state, Event::InviteRequested);
        wait_until("the invite task ran", || {
            sync_calls(&state).contains(&"invite_failed")
        });
        // The task ended with its session: only the long-lived listeners are left.
        wait_until("the task ended", || state.listener_count() == 3);
    }

    #[test]
    fn a_running_invite_task_stops_when_the_workspace_locks() {
        let state: Arc<AppState<FakeCore>> = Arc::new(AppState::new());
        state.set_background(driver_starter(Arc::downgrade(&state)));
        state.set_ready(FakeCore {
            passphrase: "pw".into(),
            invite: Some((
                InviteSide::Invitee,
                InviteSessionState::Starting,
                Some([1; 6]),
            )),
            device_server: Some(dead_server()),
            ..FakeCore::default()
        });
        core_emits(&state, Event::InviteRequested);
        wait_until("the invite task listens", || state.listener_count() == 4);

        state.lock().unwrap();
        assert!(!state.background_running());
        wait_until("every task stopped listening", || {
            state.listener_count() == 0
        });
        wait_until("the tasks released the state", || {
            Arc::strong_count(&state) == 1
        });
        assert!(
            !sync_calls(&state).contains(&"invite_failed"),
            "stopped, not failed: {:?}",
            sync_calls(&state)
        );
    }

    #[test]
    fn the_invite_host_takes_the_lock_like_every_other_call() {
        let state = ready_state(FakeCore {
            invite: Some((InviteSide::Inviter, InviteSessionState::Confirming, None)),
            ..FakeCore::default()
        });
        let info = kb_net::invite::InviteHost::invite_info(&state).unwrap();
        assert_eq!(info.state, InviteSessionState::Confirming);
        assert_eq!(info.side, InviteSide::Inviter);
        assert_eq!(
            kb_net::invite::InviteHost::invite_mailbox_id(&state).unwrap(),
            None
        );
        // The core's own error passes through untouched, and the call is logged by name and code.
        let logs = captured_logs(|| {
            assert_eq!(
                kb_net::invite::InviteHost::invite_joined(
                    &state,
                    MembershipHead {
                        seq: 0,
                        hash: [9; 32]
                    }
                )
                .unwrap_err(),
                ApiError::internal("not scripted")
            );
        });
        assert!(logs.contains("kind=\"invite_joined\""), "{logs}");
        assert!(logs.contains("code=Internal"), "{logs}");
        state.lock().unwrap();
        assert_eq!(
            kb_net::invite::InviteHost::invite_info(&state).unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(
            kb_net::invite::InviteHost::invite_outbox(&state).unwrap_err(),
            ApiError::locked()
        );
    }

    // --- presence (M13 slice 3): the commands' state, the throttle, the sealed frame's way out and in ---

    fn presence_feed() -> FeedId {
        FeedId::from_bytes([6; 16])
    }

    fn presence_space() -> SpaceId {
        SpaceId::from_bytes([3; 16])
    }

    fn presence_message(update: &[u8]) -> PresenceMessage {
        PresenceMessage {
            info: PresenceInfo {
                account_id: AccountId::from_bytes([1; 16]),
                device_id: DeviceId::from_bytes([2; 16]),
                name: "SECRET NAME".to_owned(),
                color: 1,
                page_id: Some(page(9)),
                state: PresenceState::Editing,
                updated_at: 5,
            },
            update: update.to_vec(),
        }
    }

    /// A ready state whose core seals every presence update into `sealed`, with a socket attached whose queue
    /// is the returned receiver.
    fn presence_state(
        sealed: Option<Vec<u8>>,
    ) -> (
        AppState<FakeCore>,
        tokio::sync::mpsc::Receiver<kb_net::RealtimeCommand>,
    ) {
        let state = ready_state(FakeCore {
            sealed: sealed.map(|envelope| SealedPresence {
                feed: presence_feed(),
                envelope,
            }),
            ..FakeCore::default()
        });
        let (commands, socket) = tokio::sync::mpsc::channel(64);
        state.presence_attach(RealtimeHandle::connected_to(commands));
        (state, socket)
    }

    fn seal_calls(state: &AppState<FakeCore>) -> Vec<(Option<PageId>, PresenceState, Vec<u8>)> {
        with_ready(state, |core| {
            core.seal_calls
                .iter()
                .map(|(_, page, presence, update)| (*page, *presence, update.clone()))
                .collect()
        })
    }

    /// Lets a test queue what the fake core answers `presence_open` with.
    fn queue_opened(state: &AppState<FakeCore>, messages: Vec<PresenceMessage>) {
        let mut guard = state.core.lock().unwrap();
        let CoreState::Ready(core) = &mut *guard else {
            unreachable!("the state is not ready");
        };
        core.opened.extend(messages);
    }

    #[test]
    fn presence_send_has_the_core_seal_the_update_and_hands_the_envelope_to_the_socket() {
        let (state, mut socket) = presence_state(Some(vec![7; 100]));
        CoreHost::presence_send(
            &state,
            presence_space(),
            Some(page(9)),
            PresenceState::Editing,
            b"cursor",
        )
        .unwrap();
        assert_eq!(
            seal_calls(&state),
            vec![(Some(page(9)), PresenceState::Editing, b"cursor".to_vec())]
        );
        assert_eq!(
            socket.try_recv().unwrap(),
            kb_net::RealtimeCommand::Presence(kb_sync::v2::PresenceFrame {
                feed: presence_feed(),
                envelope: vec![7; 100],
            })
        );
        assert!(socket.try_recv().is_err(), "one frame, once");
        // "In no page" and an idle device go the same way.
        CoreHost::presence_send(&state, presence_space(), None, PresenceState::Idle, b"").unwrap();
        assert_eq!(
            seal_calls(&state)[1],
            (None, PresenceState::Idle, Vec::new())
        );
        assert!(socket.try_recv().is_ok());
    }

    #[test]
    fn a_space_that_has_nothing_to_send_and_a_device_with_no_socket_are_not_errors() {
        // A personal space, an archive, a key not held yet: the core seals nothing.
        let (state, mut socket) = presence_state(None);
        CoreHost::presence_send(&state, presence_space(), None, PresenceState::Viewing, b"x")
            .unwrap();
        assert!(socket.try_recv().is_err());
        // No driver (the core is ready, sync is off): the frame has nowhere to go, and nothing fails.
        let state = ready_state(FakeCore {
            sealed: Some(SealedPresence {
                feed: presence_feed(),
                envelope: vec![1; 10],
            }),
            ..FakeCore::default()
        });
        CoreHost::presence_send(&state, presence_space(), None, PresenceState::Viewing, b"x")
            .unwrap();
        assert_eq!(seal_calls(&state).len(), 1);
        // A driver that stopped: its handle is dead.
        let (commands, socket) = tokio::sync::mpsc::channel(4);
        state.presence_attach(RealtimeHandle::connected_to(commands));
        drop(socket);
        CoreHost::presence_send(&state, presence_space(), None, PresenceState::Viewing, b"x")
            .unwrap();
    }

    #[test]
    fn at_most_ten_updates_a_second_are_sealed_and_the_rest_are_dropped_without_an_error() {
        let (state, mut socket) = presence_state(Some(vec![7; 100]));
        for i in 0..25_u8 {
            CoreHost::presence_send(&state, presence_space(), None, PresenceState::Editing, &[i])
                .unwrap();
        }
        // All within a few milliseconds: ten went through, the other fifteen never reached the core.
        assert_eq!(seal_calls(&state).len(), 10);
        let mut sent = 0;
        while socket.try_recv().is_ok() {
            sent += 1;
        }
        assert_eq!(sent, 10);
        assert_eq!(
            seal_calls(&state)
                .iter()
                .map(|(_, _, update)| update[0])
                .collect::<Vec<_>>(),
            (0..10).collect::<Vec<_>>(),
            "the first ten, in order"
        );
    }

    #[test]
    fn an_update_over_three_kib_is_refused_before_the_core_is_touched() {
        let (state, mut socket) = presence_state(Some(vec![7; 100]));
        let error = CoreHost::presence_send(
            &state,
            presence_space(),
            None,
            PresenceState::Editing,
            &vec![0; kb_types::sharing::PRESENCE_UPDATE_MAX_BYTES + 1],
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(error.details.unwrap()["field"], "update");
        assert!(seal_calls(&state).is_empty());
        assert!(socket.try_recv().is_err());
        // The largest update is fine.
        CoreHost::presence_send(
            &state,
            presence_space(),
            None,
            PresenceState::Editing,
            &vec![0; kb_types::sharing::PRESENCE_UPDATE_MAX_BYTES],
        )
        .unwrap();
        assert_eq!(seal_calls(&state).len(), 1);
    }

    #[test]
    fn a_core_that_is_locked_starting_or_refusing_answers_the_usual_error() {
        let state = locked_state(FakeCore {
            passphrase: "pw".into(),
            ..FakeCore::default()
        });
        assert_eq!(
            CoreHost::presence_send(&state, presence_space(), None, PresenceState::Idle, b"")
                .unwrap_err(),
            ApiError::locked()
        );
        let starting: AppState<FakeCore> = AppState::new();
        assert_eq!(
            CoreHost::presence_send(&starting, presence_space(), None, PresenceState::Idle, b"")
                .unwrap_err(),
            ApiError::internal(STARTING)
        );
        // The core's own refusal (an unknown space, a page that is not in it) passes through.
        let state = ready_state(FakeCore {
            fail_with: Some(ApiError::invalid("pageId", "the page is not in this space")),
            ..FakeCore::default()
        });
        let error = CoreHost::presence_send(
            &state,
            presence_space(),
            Some(page(1)),
            PresenceState::Idle,
            b"",
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn an_accepted_frame_goes_to_the_channels_and_a_dropped_one_to_nobody() {
        let state = ready_state(FakeCore::default());
        let heard: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let sink = Arc::clone(&heard);
        CoreHost::presence_subscribe(
            &state,
            Box::new(move |message| {
                sink.lock().unwrap().push(message.update.clone());
                true
            }),
        );
        // The core drops this one (nothing queued): the host answers Ok and nobody is told.
        kb_net::v2::SyncHost::presence_received(&state, presence_feed(), b"dropped").unwrap();
        assert!(heard.lock().unwrap().is_empty());
        queue_opened(
            &state,
            vec![presence_message(b"one"), presence_message(b"two")],
        );
        kb_net::v2::SyncHost::presence_received(&state, presence_feed(), b"a").unwrap();
        kb_net::v2::SyncHost::presence_received(&state, presence_feed(), b"b").unwrap();
        assert_eq!(
            *heard.lock().unwrap(),
            vec![b"one".to_vec(), b"two".to_vec()]
        );
        assert_eq!(
            with_ready(&state, |core| core.opened_frames.clone()),
            vec![
                (presence_feed(), b"dropped".to_vec()),
                (presence_feed(), b"a".to_vec()),
                (presence_feed(), b"b".to_vec()),
            ],
            "the envelope reaches the core as it came"
        );
    }

    #[test]
    fn the_core_events_of_an_opened_frame_reach_the_listeners_before_the_message_reaches_the_channel()
     {
        let state = ready_state(FakeCore::default());
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let events = Arc::clone(&order);
        let _unsubscribe = state.add_listener(Arc::new(move |event| {
            if matches!(event, Event::PresenceChanged { .. }) {
                events.lock().unwrap().push("event");
            }
        }));
        let messages = Arc::clone(&order);
        CoreHost::presence_subscribe(
            &state,
            Box::new(move |_| {
                messages.lock().unwrap().push("message");
                true
            }),
        );
        queue_opened(&state, vec![presence_message(b"x")]);
        kb_net::v2::SyncHost::presence_received(&state, presence_feed(), b"frame").unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["event", "message"]);
    }

    #[test]
    fn a_channel_that_cannot_be_reached_is_forgotten_and_a_reload_clears_the_rest() {
        let state = ready_state(FakeCore::default());
        CoreHost::presence_subscribe(&state, Box::new(|_| false));
        let heard = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&heard);
        CoreHost::presence_subscribe(
            &state,
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            }),
        );
        queue_opened(&state, vec![presence_message(b"x"), presence_message(b"y")]);
        kb_net::v2::SyncHost::presence_received(&state, presence_feed(), b"1").unwrap();
        assert_eq!(state.presence.subscribers(), 1);
        // The page reloads: the callbacks are gone with it.
        CoreHost::clear_subscribers(&state);
        assert_eq!(state.presence.subscribers(), 0);
        kb_net::v2::SyncHost::presence_received(&state, presence_feed(), b"2").unwrap();
        assert_eq!(heard.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_locked_core_hands_no_frame_to_anybody() {
        let state = locked_state(FakeCore {
            passphrase: "pw".into(),
            ..FakeCore::default()
        });
        let heard = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&heard);
        CoreHost::presence_subscribe(
            &state,
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            }),
        );
        assert_eq!(
            kb_net::v2::SyncHost::presence_received(&state, presence_feed(), b"x").unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(heard.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_driver_calls_of_slice_three_go_through_the_cores_lock() {
        let state = ready_state(FakeCore::default());
        let inline = InlineEntry {
            feed: presence_feed(),
            prev_cursor: 1,
            cursor: 2,
            chain_after: [0; 32],
            entry: vec![1, 2, 3],
        };
        assert_eq!(
            kb_net::v2::SyncHost::sync_feed_apply_inline(&state, presence_feed(), &inline).unwrap(),
            InlineOutcome::Known
        );
        assert_eq!(
            kb_net::v2::SyncHost::sync_push_debounce_ms(&state).unwrap(),
            150
        );
        kb_net::v2::SyncHost::presence_expire(&state).unwrap();
        assert_eq!(
            sync_calls(&state),
            vec![
                "sync_feed_apply_inline",
                "sync_push_debounce_ms",
                "presence_expire"
            ]
        );
        state.lock().unwrap();
        // Locked, the lot is refused without touching the core.
        assert_eq!(
            kb_net::v2::SyncHost::sync_push_debounce_ms(&state).unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(
            kb_net::v2::SyncHost::presence_expire(&state).unwrap_err(),
            ApiError::locked()
        );
    }

    #[test]
    fn the_link_work_of_slice_four_goes_through_the_cores_lock() {
        let link = LinkId::from_bytes([0xcd; 16]);
        let gone = LinkId::from_bytes([0xab; 16]);
        let put = LinkPut {
            feed: presence_feed(),
            expires_at: Some(9),
            chunks: vec![ChunkId::from_bytes([1; 32])],
            publication: vec![1, 2, 3],
        };
        let work = vec![
            LinkWork::Put { link_id: link, put },
            LinkWork::Delete { link_id: gone },
        ];
        let state = ready_state(FakeCore {
            link_work: work.clone(),
            ..FakeCore::default()
        });
        assert_eq!(kb_net::v2::SyncHost::sync_link_work(&state).unwrap(), work);
        kb_net::v2::SyncHost::sync_link_done(&state, link, LinkOutcome::Published { pub_seq: 3 })
            .unwrap();
        kb_net::v2::SyncHost::sync_link_done(&state, gone, LinkOutcome::Deleted).unwrap();
        assert!(
            kb_net::v2::SyncHost::sync_link_work(&state)
                .unwrap()
                .is_empty(),
            "the fake hands the work out once"
        );
        assert_eq!(
            with_ready(&state, |core| core.link_done.clone()),
            vec![
                (link, LinkOutcome::Published { pub_seq: 3 }),
                (gone, LinkOutcome::Deleted)
            ]
        );
        assert_eq!(
            sync_calls(&state),
            vec![
                "sync_link_work",
                "sync_link_done",
                "sync_link_done",
                "sync_link_work"
            ]
        );
        state.lock().unwrap();
        // Locked, both are refused without touching the core.
        assert_eq!(
            kb_net::v2::SyncHost::sync_link_work(&state).unwrap_err(),
            ApiError::locked()
        );
        assert_eq!(
            kb_net::v2::SyncHost::sync_link_done(&state, link, LinkOutcome::Refused).unwrap_err(),
            ApiError::locked()
        );
    }

    #[test]
    fn nothing_of_a_presence_frame_is_logged_not_the_update_the_name_or_the_envelope() {
        let (state, _socket) = presence_state(Some(b"ENVELOPE SECRET".to_vec()));
        let heard = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&heard);
        CoreHost::presence_subscribe(
            &state,
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            }),
        );
        queue_opened(&state, vec![presence_message(b"UPDATE SECRET")]);
        let logs = crate::test_support::captured_logs_through(
            tracing_subscriber::EnvFilter::new("trace"),
            || {
                CoreHost::presence_send(
                    &state,
                    presence_space(),
                    Some(page(9)),
                    PresenceState::Editing,
                    b"UPDATE SECRET",
                )
                .unwrap();
                kb_net::v2::SyncHost::presence_received(
                    &state,
                    presence_feed(),
                    b"ENVELOPE SECRET",
                )
                .unwrap();
            },
        );
        assert_eq!(heard.load(Ordering::SeqCst), 1);
        assert!(
            logs.contains("presence_send"),
            "something is logged: {logs}"
        );
        for secret in ["SECRET", "NAME", "UPDATE", "ENVELOPE"] {
            assert!(!logs.contains(secret), "{secret} in the log: {logs}");
        }
        assert!(!logs.contains(&presence_feed().to_string()), "{logs}");
    }
}
