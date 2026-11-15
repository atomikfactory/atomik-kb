//! The engine's host: the one `Core` behind a mutex (single writer), the events it emits, and the sync,
//! pairing, recovery and invite hosts the network tasks run against. A port of the desktop shell's
//! `state.rs` and `sync_host.rs` without Tauri; the desktop is not refactored (see ARCHITECTURE.md, A4).
//!
//! Every call into the core takes the lock, makes one core call, drains the events it emitted and hands them
//! to the Rust listeners (the driver and the tasks) and to the delivery queue before the lock is released, so
//! two calls never queue their events out of order. The delivery thread (see `lib.rs`) then calls the app's
//! `EventSink` outside the lock, one event at a time, in the order they were queued.
//!
//! Nothing here logs a page, an entry, a key, a code or an error message: method names, outcomes and event
//! counts only.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use kb_core::{AttachmentSink, Core, LockedCore, Opened};
use kb_crypto::Signature;
use kb_net::invite::{InviteHost, InviteTasks};
use kb_net::v2::{SyncDriver, SyncHost};
use kb_net::{
    DeviceTasks, DriverHandle, HttpRemote, PairingHost, RecoveryHost, RecoveryUpload,
    RemoteFactory, TaskHandle,
};
use kb_sync::v2::{
    FeedChunkWork, FeedWork, InlineEntry, InlineOutcome, InviteOpenRequest, InviteOpened,
    InviteSignup, LinkOutcome, LinkWork, LogRequest, MembershipHead, MembershipPage, OutboundEntry,
    PendingMembership, SnapshotMark, SnapshotsApplied, SyncApplied,
};
use kb_sync::{ChangesPage, SnapshotsPage};
use kb_types::devices::MAILBOX_ID_LEN;
use kb_types::{
    AccountId, ApiError, AttachmentId, AttachmentInfo, ByteRange, ChunkId, DeviceId, DocId, Event,
    FeedId, InviteSessionInfo, InviteSessionState, InviteSide, LinkId, PageId, PairingInfo,
    PairingRole, RecoveryInfo, Request, Response, SyncInfo, SyncState,
};

use crate::EngineState;

/// What a request is told while the core is still opening.
pub(crate) const STARTING: &str = "core is starting";

/// Where the core is in its life (ADR-0018): `Opening` becomes `Ready`, `Locked` or `Failed`; `Ready` becomes
/// `Locked` through `lock` and back through `unlock`.
enum Slot {
    Opening,
    Ready(Box<Core>),
    Locked(Box<LockedCore>),
    Failed(ApiError),
}

impl Slot {
    fn ready(&mut self) -> Result<&mut Core, ApiError> {
        match self {
            Self::Ready(core) => Ok(core),
            Self::Opening => Err(ApiError::internal(STARTING)),
            Self::Locked(_) => Err(ApiError::locked()),
            Self::Failed(error) => Err(error.clone()),
        }
    }

    fn state(&self) -> EngineState {
        match self {
            Self::Opening => EngineState::Opening,
            Self::Ready(_) => EngineState::Ready,
            Self::Locked(_) => EngineState::Locked,
            Self::Failed(error) => EngineState::Failed {
                message: error.message.clone(),
            },
        }
    }
}

/// What goes to the app, in order.
pub(crate) enum Delivery {
    /// One core event, as JSON.
    Event(String),
    /// The engine's state changed.
    State(EngineState),
}

type Listener = Arc<dyn Fn(&Event) + Send + Sync>;

#[derive(Default)]
struct Listeners {
    next_id: u64,
    entries: Vec<(u64, Listener)>,
}

/// Counts `SyncChanged` events, for `sync_once`.
#[derive(Default)]
struct SyncSignal {
    count: Mutex<u64>,
    changed: Condvar,
}

impl SyncSignal {
    fn bump(&self) {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.changed.notify_all();
    }

    fn count(&self) -> u64 {
        *self.count.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until the count is above `seen` or `timeout` passes.
    fn wait_past(&self, seen: u64, timeout: Duration) {
        let guard = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .changed
            .wait_timeout_while(guard, timeout, |count| *count <= seen);
    }
}

/// What runs in the background while the core is ready: dropping it stops all three.
struct NetworkWork {
    _driver: DriverHandle,
    _devices: TaskHandle,
    _invites: TaskHandle,
}

pub(crate) struct Host {
    core: Mutex<Slot>,
    listeners: Arc<Mutex<Listeners>>,
    background: Mutex<Option<NetworkWork>>,
    out: Sender<Delivery>,
    runtime: Option<tokio::runtime::Handle>,
    me: Weak<Host>,
    signal: SyncSignal,
}

fn poisoned() -> ApiError {
    ApiError::internal("the core stopped after an internal error; restart the app")
}

impl Host {
    pub(crate) fn new(out: Sender<Delivery>, runtime: Option<tokio::runtime::Handle>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            core: Mutex::new(Slot::Opening),
            listeners: Arc::default(),
            background: Mutex::new(None),
            out,
            runtime,
            me: me.clone(),
            signal: SyncSignal::default(),
        })
    }

    fn send(&self, delivery: Delivery) {
        // The delivery thread ends with the engine; a send after that has nobody to tell.
        let _ = self.out.send(delivery);
    }

    /// Tells the app the engine is opening (the first state it hears).
    pub(crate) fn announce_opening(&self) {
        self.send(Delivery::State(EngineState::Opening));
    }

    pub(crate) fn state(&self) -> EngineState {
        match self.core.lock() {
            Ok(slot) => slot.state(),
            Err(_) => EngineState::Failed {
                message: poisoned().message,
            },
        }
    }

    /// The core finished opening (or failed to): record it, tell the app, start the network work.
    pub(crate) fn install(&self, outcome: Result<Opened, ApiError>) {
        let slot = match outcome {
            Ok(Opened::Ready(core)) => Slot::Ready(Box::new(core)),
            Ok(Opened::Locked(locked)) => Slot::Locked(Box::new(locked)),
            Err(error) => {
                tracing::error!(code = ?error.code, "core failed to open");
                Slot::Failed(error)
            }
        };
        let ready = matches!(slot, Slot::Ready(_));
        {
            let mut guard = self.core.lock().unwrap_or_else(PoisonError::into_inner);
            *guard = slot;
            self.send(Delivery::State(guard.state()));
        }
        if ready {
            self.start_background();
        }
    }

    // ----- events -----

    /// Registers a Rust listener for every event from now on; the returned closure removes it.
    fn add_listener(&self, listener: Listener) -> Box<dyn FnOnce() + Send> {
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

    /// Hands events to the Rust listeners and queues them for the app, in order. Called under the core lock.
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
        for event in events {
            for listener in &listeners {
                listener(&event);
            }
            if matches!(event, Event::SyncChanged) {
                self.signal.bump();
            }
            match serde_json::to_string(&event) {
                Ok(json) => self.send(Delivery::Event(json)),
                Err(error) => tracing::warn!(%error, "an event could not be encoded"),
            }
        }
    }

    /// The one place the single-writer lock is taken for a core call: refuses while the core is not ready,
    /// runs `call`, drains the events it emitted and queues them before the lock is released.
    pub(crate) fn call<T>(
        &self,
        kind: &'static str,
        call: impl FnOnce(&mut Core) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let mut guard = self.core.lock().map_err(|_| poisoned())?;
        let core = guard.ready()?;
        let result = call(core);
        let events = core.drain_events();
        match &result {
            Ok(_) => tracing::trace!(kind, events = events.len(), "core call"),
            Err(error) => {
                tracing::debug!(kind, events = events.len(), code = ?error.code, "core call refused");
            }
        }
        self.publish(events);
        result
    }

    // ----- requests, documents, attachments -----

    pub(crate) fn dispatch(&self, request: Request) -> Result<Response, ApiError> {
        let kind = request.kind();
        self.call(kind, |core| core.dispatch(request))
    }

    pub(crate) fn doc_open(&self, page_id: PageId) -> Result<Vec<u8>, ApiError> {
        self.call("doc_open", |core| core.page_doc_open(page_id))
    }

    pub(crate) fn doc_import(&self, page_id: PageId, update: &[u8]) -> Result<(), ApiError> {
        self.call("doc_import", |core| core.page_doc_import(page_id, update))
    }

    pub(crate) fn doc_export(
        &self,
        page_id: PageId,
        since: Option<&[u8]>,
    ) -> Result<Vec<u8>, ApiError> {
        self.call("doc_export", |core| core.page_doc_export(page_id, since))
    }

    pub(crate) fn doc_close(&self, page_id: PageId) -> Result<(), ApiError> {
        self.call("doc_close", |core| core.page_doc_close(page_id))
    }

    pub(crate) fn attachment_write(
        &self,
        attachment_id: AttachmentId,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        self.call("attachment_write", |core| {
            core.attachment_write(attachment_id, bytes)
        })
    }

    pub(crate) fn attachment_read(
        &self,
        attachment_id: AttachmentId,
        range: Option<ByteRange>,
        sink: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        self.call("attachment_read", |core| {
            core.attachment_read(attachment_id, range, sink)
        })
    }

    // ----- lock and unlock -----

    /// Engages the app lock: `Ready` becomes `Locked` (the network work stops). A core that refuses (the lock
    /// is not enabled) stays ready and its error is returned; locking a locked workspace succeeds.
    pub(crate) fn lock(&self) -> Result<(), ApiError> {
        let (locked, result) = {
            let mut guard = self.core.lock().map_err(|_| poisoned())?;
            let current = std::mem::replace(&mut *guard, Slot::Opening);
            let (next, result) = match current {
                Slot::Ready(core) => {
                    let mut core = *core;
                    self.publish(core.drain_events());
                    match core.lock() {
                        Ok(locked) => (Slot::Locked(Box::new(locked)), Ok(())),
                        Err((mut core, error)) => {
                            self.publish(core.drain_events());
                            (Slot::Ready(Box::new(core)), Err(error))
                        }
                    }
                }
                Slot::Locked(locked) => (Slot::Locked(locked), Ok(())),
                Slot::Opening => (Slot::Opening, Err(ApiError::internal(STARTING))),
                Slot::Failed(error) => (Slot::Failed(error.clone()), Err(error)),
            };
            let locked = matches!(next, Slot::Locked(_));
            *guard = next;
            if locked && result.is_ok() {
                self.send(Delivery::State(EngineState::Locked));
            }
            (locked, result)
        };
        if locked {
            self.stop_background();
        }
        result
    }

    /// Disengages the app lock: `Locked` becomes `Ready` (the network work starts). A wrong passphrase leaves
    /// the state locked (`InvalidInput` for `passphrase`). Takes about a second (Argon2id).
    pub(crate) fn unlock(&self, passphrase: &str) -> Result<(), ApiError> {
        let ready = {
            let mut guard = self.core.lock().map_err(|_| poisoned())?;
            let current = std::mem::replace(&mut *guard, Slot::Opening);
            let (next, result) = match current {
                Slot::Locked(locked) => match (*locked).unlock(passphrase) {
                    Ok(mut core) => {
                        self.publish(core.drain_events());
                        (Slot::Ready(Box::new(core)), Ok(()))
                    }
                    Err((locked, error)) => (Slot::Locked(Box::new(locked)), Err(error)),
                },
                Slot::Ready(core) => (
                    Slot::Ready(core),
                    Err(ApiError::new(
                        kb_types::ErrorCode::InvalidInput,
                        "the workspace is not locked",
                    )),
                ),
                Slot::Opening => (Slot::Opening, Err(ApiError::internal(STARTING))),
                Slot::Failed(error) => (Slot::Failed(error.clone()), Err(error)),
            };
            let now_ready = result.is_ok() && matches!(next, Slot::Ready(_));
            *guard = next;
            if now_ready {
                self.send(Delivery::State(EngineState::Ready));
            }
            result.map(|()| now_ready)
        }?;
        if ready {
            self.start_background();
        }
        Ok(())
    }

    // ----- background work -----

    /// Starts the sync driver and the device and invite tasks unless they run already.
    pub(crate) fn start_background(&self) {
        let (Some(host), Some(runtime)) = (self.me.upgrade(), self.runtime.clone()) else {
            return;
        };
        let mut background = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if background.is_some() {
            return;
        }
        let factory: RemoteFactory<HttpRemote> =
            Arc::new(|url: &str| HttpRemote::new(url).map(Arc::new));
        let devices = DeviceTasks::spawn(Arc::clone(&host), Arc::clone(&factory), runtime.clone());
        let invites = InviteTasks::spawn(Arc::clone(&host), Arc::clone(&factory), runtime.clone());
        let sync: Arc<dyn SyncHost> = host;
        let driver = SyncDriver::spawn(sync, factory, runtime);
        *background = Some(NetworkWork {
            _driver: driver,
            _devices: devices,
            _invites: invites,
        });
        tracing::debug!("background work started");
    }

    /// Stops the background work (dropping it never waits for it).
    pub(crate) fn stop_background(&self) {
        let running = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if running.is_some() {
            drop(running);
            tracing::debug!("background work stopped");
        }
    }

    // ----- sync now -----

    /// Asks for a sync round and waits until the driver is idle again or `timeout` passes. `true` when the
    /// last state is `Idle` (or sync is disabled: nothing to wait for).
    pub(crate) fn sync_once(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let seen = self.signal.count();
        if self.dispatch(Request::SyncNow).is_err() {
            return false;
        }
        let mut was_active = false;
        loop {
            let Ok(info) = SyncHost::sync_info(self) else {
                return false;
            };
            if !info.configured || info.state == SyncState::Disabled {
                return true;
            }
            match info.state {
                SyncState::Pushing | SyncState::Pulling => was_active = true,
                state => {
                    if was_active || self.signal.count() > seen {
                        return state == SyncState::Idle;
                    }
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let step = (deadline - now).min(Duration::from_millis(100));
            self.signal.wait_past(self.signal.count(), step);
        }
    }
}

/// Forwards host methods to the core method of the same name, under the lock.
macro_rules! forward {
    ($( fn $name:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) -> $ret:ty; )*) => {
        $(
            fn $name(&self $(, $arg: $ty)*) -> $ret {
                self.call(stringify!($name), |core| core.$name($($arg),*))
            }
        )*
    };
}

/// Whether a session of the invitee side is running: the person is about to get an account on the server
/// from the invitation, or has just got it.
fn invitee_session_running(info: &InviteSessionInfo) -> bool {
    info.side == InviteSide::Invitee
        && !matches!(
            info.state,
            InviteSessionState::Idle
                | InviteSessionState::Done
                | InviteSessionState::Failed
                | InviteSessionState::Cancelled
        )
}

impl SyncHost for Host {
    forward! {
        fn sync_info(&self) -> Result<SyncInfo, ApiError>;
        fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError>;
        fn sync_take_invite_token(&self) -> Result<Option<String>, ApiError>;
        fn sync_log_received(&self, entries: &[Vec<u8>]) -> Result<bool, ApiError>;
        fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError>;
        fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError>;
        fn sync_sign(&self, message: &[u8]) -> Result<Signature, ApiError>;
        fn sync_remote_epoch(&self, epoch: [u8; 16]) -> Result<bool, ApiError>;
        fn sync_set_state(&self, state: SyncState, error: Option<String>) -> Result<(), ApiError>;
        fn sync_recovery_blob(&self) -> Result<Option<RecoveryUpload>, ApiError>;
        fn sync_recovery_published(&self) -> Result<(), ApiError>;
        fn sync_recovery_obsolete(&self) -> Result<Vec<[u8; 32]>, ApiError>;
        fn sync_recovery_deleted(&self, id: [u8; 32]) -> Result<(), ApiError>;
        fn sync_feeds(&self) -> Result<Vec<FeedWork>, ApiError>;
        fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError>;
        fn sync_foreign_log_received(&self, account: AccountId, entries: &[Vec<u8>]) -> Result<bool, ApiError>;
        fn sync_membership_pending(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError>;
        fn sync_membership_posted(&self, feed: FeedId, head: MembershipHead) -> Result<(), ApiError>;
        fn sync_membership_conflict(&self, feed: FeedId) -> Result<(), ApiError>;
        fn sync_membership_wanted(&self, feed: FeedId) -> Result<Option<Option<u64>>, ApiError>;
        fn sync_membership_fetched(&self, feed: FeedId, page: &MembershipPage) -> Result<(), ApiError>;
        fn sync_feed_lost(&self, feed: FeedId) -> Result<(), ApiError>;
        fn sync_feed_refused(&self, feed: FeedId, message: &str) -> Result<(), ApiError>;
        fn sync_feed_rekey_needed(&self, feed: FeedId) -> Result<(), ApiError>;
        fn sync_feed_outbox(&self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError>;
        fn sync_feed_pushed(&self, feed: FeedId, doc_id: DocId, device_seq: u64, cursor: u64, up_to_seq: i64) -> Result<(), ApiError>;
        fn sync_feed_apply(&self, feed: FeedId, page: &ChangesPage) -> Result<SyncApplied, ApiError>;
        fn sync_feed_snapshot_candidates(&self, feed: FeedId) -> Result<Vec<DocId>, ApiError>;
        fn sync_feed_make_snapshot(&self, feed: FeedId, doc_id: DocId) -> Result<(Vec<u8>, u64), ApiError>;
        fn sync_feed_snapshot_published(&self, feed: FeedId, doc_id: DocId, covers: u64) -> Result<(), ApiError>;
        fn sync_feed_apply_snapshots(&self, feed: FeedId, page: &SnapshotsPage) -> Result<SnapshotsApplied, ApiError>;
        fn sync_snapshot_marks_wanted(&self, feed: FeedId) -> Result<Option<u64>, ApiError>;
        fn sync_snapshot_marks_received(&self, feed: FeedId, marks: &[SnapshotMark]) -> Result<Vec<DocId>, ApiError>;
        fn sync_snapshot_verified(&self, feed: FeedId, doc_id: DocId, ok: bool) -> Result<(), ApiError>;
        fn sync_feed_head(&self, feed: FeedId) -> Result<Vec<u8>, ApiError>;
        fn sync_feed_heads_received(&self, feed: FeedId, heads: &[Vec<u8>]) -> Result<bool, ApiError>;
        fn sync_feed_chunk_work(&self, feed: FeedId, limit: usize) -> Result<Vec<FeedChunkWork>, ApiError>;
        fn sync_feed_chunk_bytes(&self, feed: FeedId, hash: ChunkId) -> Result<Option<Vec<u8>>, ApiError>;
        fn sync_feed_chunk_uploaded(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;
        fn sync_feed_chunk_received(&self, feed: FeedId, hash: ChunkId, bytes: Vec<u8>) -> Result<(), ApiError>;
        fn sync_feed_chunk_unavailable(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;
        fn sync_feed_chunk_deleted(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError>;
        fn sync_feed_apply_inline(&self, feed: FeedId, inline: &InlineEntry) -> Result<InlineOutcome, ApiError>;
        fn sync_push_debounce_ms(&self) -> Result<u64, ApiError>;
        fn presence_expire(&self) -> Result<(), ApiError>;
        fn sync_link_work(&self) -> Result<Vec<LinkWork>, ApiError>;
        fn sync_link_done(&self, link_id: LinkId, outcome: LinkOutcome) -> Result<(), ApiError>;
    }

    fn sync_account_unknown(&self) -> Result<(), ApiError> {
        self.call("sync_account_unknown", |core| {
            // An invitee that is about to get its account from the invitation does not need a token.
            if core
                .invite_info()
                .is_ok_and(|info| invitee_session_running(&info))
            {
                return Ok(());
            }
            core.sync_account_unknown()
        })
    }

    fn presence_received(&self, feed: FeedId, envelope: &[u8]) -> Result<(), ApiError> {
        // The core opens the frame under its lock (and emits `PresenceChanged` when someone appeared, moved or
        // left); the phone has no presence channel of its own in v1, so the accepted frame is dropped.
        self.call("presence_open", |core| core.presence_open(feed, envelope))
            .map(|_| ())
    }

    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

impl InviteHost for Host {
    forward! {
        fn invite_info(&self) -> Result<InviteSessionInfo, ApiError>;
        fn invite_mailbox_id(&self) -> Result<Option<[u8; 6]>, ApiError>;
        fn invite_mailbox_opened(&self, mailbox_id: [u8; 6], expires_at: i64) -> Result<(), ApiError>;
        fn invite_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError>;
        fn invite_posted(&self, seq: u32) -> Result<(), ApiError>;
        fn invite_receive(&self, seq: u32, from: InviteSide, bytes: &[u8]) -> Result<(), ApiError>;
        fn invite_failed(&self, message: &str) -> Result<(), ApiError>;
        fn invite_membership_pending(&self) -> Result<Option<PendingMembership>, ApiError>;
        fn invite_membership_posted(&self, head: MembershipHead) -> Result<(), ApiError>;
        fn invite_signup(&self, server_epoch: [u8; 16]) -> Result<Option<InviteSignup>, ApiError>;
        fn invite_open_request(&self, server_epoch: [u8; 16]) -> Result<Option<InviteOpenRequest>, ApiError>;
        fn invite_opened(&self, opened: &InviteOpened) -> Result<(), ApiError>;
        fn invite_join(&self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError>;
        fn invite_joined(&self, head: MembershipHead) -> Result<(), ApiError>;
        fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError>;
        fn sync_sign(&self, message: &[u8]) -> Result<Signature, ApiError>;
        fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError>;
        fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError>;
        fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError>;
        fn sync_foreign_log_received(&self, account: AccountId, entries: &[Vec<u8>]) -> Result<bool, ApiError>;
    }

    fn invite_generation(&self) -> Result<u64, ApiError> {
        self.call("invite_generation", |core| Ok(core.invite_generation()))
    }

    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

impl PairingHost for Host {
    forward! {
        fn pairing_mailbox_id(&self) -> Result<Option<[u8; MAILBOX_ID_LEN]>, ApiError>;
        fn pairing_mailbox_opened(&self, mailbox_id: [u8; MAILBOX_ID_LEN], expires_at: i64) -> Result<(), ApiError>;
        fn pairing_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError>;
        fn pairing_posted(&self, seq: u32) -> Result<(), ApiError>;
        fn pairing_receive(&self, seq: u32, role: PairingRole, bytes: &[u8]) -> Result<(), ApiError>;
        fn pairing_failed(&self, message: &str) -> Result<(), ApiError>;
        fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError>;
        fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError>;
        fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError>;
    }

    fn pairing_info(&self) -> Result<PairingInfo, ApiError> {
        self.call("pairing_info", |core| Ok(core.pairing_info()))
    }

    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

impl RecoveryHost for Host {
    forward! {
        fn recovery_id(&self) -> Result<Option<[u8; 32]>, ApiError>;
        fn recovery_blob_received(&self, bytes: &[u8]) -> Result<(), ApiError>;
        fn recovery_log_received(&self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError>;
        fn recovery_entry_published(&self) -> Result<(), ApiError>;
        fn recovery_failed(&self, message: &str) -> Result<(), ApiError>;
        fn recovery_account_id(&self) -> Result<Option<AccountId>, ApiError>;
    }

    fn recovery_info(&self) -> Result<RecoveryInfo, ApiError> {
        self.call("recovery_info", |core| Ok(core.recovery_info()))
    }
}
