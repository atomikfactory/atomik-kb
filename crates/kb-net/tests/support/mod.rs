//! The harness of the end-to-end tests: a [`Peer`] is a person's device, a real `kb_core::Core` over a plain
//! SQLite file that is the `SyncHost` and the `InviteHost` the v2 driver and the invite task run against
//! (the same single-writer rule as the shell's `AppState`: one call, then the events it queued go to the
//! listeners before the lock is released), and [`Env`] is the server between them: the reference
//! `MemoryRemote` or an in-process `kb-server` over HTTP on a loopback port. One `FixedClock` serves the
//! server and every core, so an invite's expiry means the same on both sides and a test can move time.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    dead_code
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kb_core::{Core, CoreDeps};
use kb_keys::{DbKey, MemoryKeystore};
use kb_net::invite::{InviteHost, InviteTasks};
use kb_net::v2::{SyncDriver, SyncHost, SyncRemote};
use kb_net::{DriverHandle, HttpRemote, RealtimeHandle, RecoveryUpload, RemoteFactory, TaskHandle};
use kb_storage::{DbOpener, Encryption, MemoryBlobStore, OpenedDb, StorageError};
use kb_sync::v2::{
    FeedChunkWork, FeedWork, InlineEntry, InlineOutcome, InviteOpenRequest, InviteOpened,
    InviteSignup, LinkOutcome, LinkWork, LogRequest, MembershipHead, MembershipPage, MemoryRemote,
    OutboundEntry, PendingMembership, SnapshotMark, SnapshotsApplied, SyncApplied,
};
use kb_sync::{ChangesPage, SnapshotsPage};
use kb_types::testing::{FixedClock, SeededRng};
use kb_types::{
    AccountId, ApiError, ChunkId, Clock, DeviceId, DocId, Event, FeedId, InstallId,
    InviteSessionInfo, InviteSessionState, InviteSide, LinkId, MemberRole, MembersInfo, PageId,
    PresenceMessage, PresenceState, Request, Response, SpaceId, SpaceInfo, SpaceShareState,
    SyncInfo, SyncState, TreeView,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// The time every test starts at (Unix ms).
pub const T0: i64 = 1_790_000_000_000;

/// The invite token of the test servers.
pub const TOKEN: &str = "dev";

/// A clock several holders share.
pub struct SharedClock(pub Arc<FixedClock>);

impl Clock for SharedClock {
    fn now_ms(&self) -> i64 {
        self.0.now_ms()
    }
}

/// A plain SQLite file: the tests need no SQLCipher key.
struct PlainFileOpener(PathBuf);

impl DbOpener for PlainFileOpener {
    fn open(&self, _key: &DbKey) -> Result<OpenedDb, StorageError> {
        let conn = kb_storage::rusqlite::Connection::open(&self.0)?;
        Ok(OpenedDb {
            conn,
            encryption: Encryption::None,
        })
    }
}

type Listener = Arc<dyn Fn(&Event) + Send + Sync>;

/// One person's device over a real core.
pub struct Peer {
    pub name: &'static str,
    core: Mutex<Core>,
    listeners: Arc<Mutex<Vec<(u64, Listener)>>>,
    next_listener: AtomicU64,
    /// Every event the core emitted, in order.
    events: Mutex<Vec<Event>>,
    /// What the core answered to each inline entry the driver offered, in order.
    inline: Mutex<Vec<InlineOutcome>>,
    /// Pages of changes the pull path handed the core (`sync_feed_apply` calls).
    pulled_pages: AtomicUsize,
    /// What `sync_push_debounce_ms` answered, in order.
    debounce: Mutex<Vec<u64>>,
    /// The presence frames the core accepted (what the shell sends its channel).
    presence: Mutex<Vec<PresenceMessage>>,
    _dir: tempfile::TempDir,
}

impl Peer {
    /// A device with a workspace named after the person (and a first space), the recovery kit and no
    /// sync configuration.
    pub fn open(name: &'static str, seed: u64, clock: &Arc<FixedClock>) -> Arc<Self> {
        let dir = tempfile::tempdir().unwrap();
        let mut install = [0u8; 16];
        install[..8].copy_from_slice(&seed.to_le_bytes());
        let deps = CoreDeps {
            install_id: InstallId::from_random_bytes(install),
            platform: "windows".into(),
            data_dir: dir.path().display().to_string(),
            keystore: Box::new(MemoryKeystore::new()),
            opener: Box::new(PlainFileOpener(dir.path().join("workspace.db"))),
            clock: Box::new(SharedClock(Arc::clone(clock))),
            rng: Box::new(SeededRng::new(seed)),
            lock_params: kb_crypto::Argon2Params {
                memory_kib: 8 * 1024,
                iterations: 1,
                parallelism: 1,
            },
            blob_store: Box::new(MemoryBlobStore::new()),
        };
        let core = Core::open(deps).unwrap();
        let peer = Arc::new(Self {
            name,
            core: Mutex::new(core),
            listeners: Arc::default(),
            next_listener: AtomicU64::new(0),
            events: Mutex::new(Vec::new()),
            inline: Mutex::new(Vec::new()),
            pulled_pages: AtomicUsize::new(0),
            debounce: Mutex::new(Vec::new()),
            presence: Mutex::new(Vec::new()),
            _dir: dir,
        });
        peer.request(Request::WorkspaceCreate {
            name: format!("{name}'s workspace"),
            first_space_name: Some("Personal".to_owned()),
        })
        .unwrap();
        peer.request(Request::RecoveryKitCreate).unwrap();
        peer
    }

    /// One call under the lock; the events it queued reach the listeners before the lock is released.
    pub fn with<T>(&self, call: impl FnOnce(&mut Core) -> T) -> T {
        let mut core = self.core.lock().unwrap();
        let out = call(&mut core);
        let events = core.drain_events();
        if !events.is_empty() {
            self.events.lock().unwrap().extend(events.iter().cloned());
            let listeners: Vec<Listener> = self
                .listeners
                .lock()
                .unwrap()
                .iter()
                .map(|(_, listener)| Arc::clone(listener))
                .collect();
            for event in &events {
                for listener in &listeners {
                    listener(event);
                }
            }
        }
        out
    }

    pub fn request(&self, request: Request) -> Result<Response, ApiError> {
        self.with(|core| core.dispatch(request))
    }

    /// The events the core emitted so far.
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    pub fn account(&self) -> AccountId {
        self.with(|core| core.sync_identity())
            .unwrap()
            .expect("a kit exists")
            .0
    }

    // ----- requests, answered --------------------------------------------------------------------

    pub fn set_profile(&self, name: &str) {
        self.request(Request::ProfileSet {
            display_name: name.to_owned(),
        })
        .unwrap();
    }

    pub fn configure(&self, url: &str, token: Option<&str>) {
        self.request(Request::SyncConfigure {
            server_url: Some(url.to_owned()),
            invite_token: token.map(str::to_owned),
        })
        .unwrap();
    }

    pub fn create_space(&self, name: &str) -> SpaceId {
        match self
            .request(Request::SpaceCreate {
                name: name.to_owned(),
                icon: None,
            })
            .unwrap()
        {
            Response::Space(space) => space.id,
            other => panic!("unexpected {other:?}"),
        }
    }

    pub fn create_page(&self, space: SpaceId, parent: Option<PageId>, title: &str) -> PageId {
        match self
            .request(Request::PageCreate {
                space_id: space,
                parent_id: parent,
                index: None,
                title: title.to_owned(),
            })
            .unwrap()
        {
            Response::Page(page) => page.id,
            other => panic!("unexpected {other:?}"),
        }
    }

    pub fn rename(&self, page: PageId, title: &str) -> Result<Response, ApiError> {
        self.request(Request::PageRename {
            page_id: page,
            title: title.to_owned(),
        })
    }

    pub fn spaces(&self) -> Vec<SpaceInfo> {
        match self.request(Request::SpaceList).unwrap() {
            Response::Spaces(spaces) => spaces,
            other => panic!("unexpected {other:?}"),
        }
    }

    pub fn space(&self, space: SpaceId) -> Option<SpaceInfo> {
        self.spaces().into_iter().find(|info| info.id == space)
    }

    pub fn tree(&self, space: SpaceId) -> TreeView {
        match self.request(Request::TreeGet { space_id: space }).unwrap() {
            Response::Tree(tree) => tree,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// The titles of the live tree, depth first, indented by depth.
    pub fn outline(&self, space: SpaceId) -> String {
        fn walk(nodes: &[kb_types::PageNode], depth: usize, out: &mut String) {
            for node in nodes {
                out.push_str(&"  ".repeat(depth));
                out.push_str(&node.page.title);
                out.push('\n');
                walk(&node.children, depth + 1, out);
            }
        }
        let mut out = String::new();
        walk(&self.tree(space).roots, 0, &mut out);
        out
    }

    pub fn members(&self, space: SpaceId) -> MembersInfo {
        match self
            .request(Request::SpaceMembers { space_id: space })
            .unwrap()
        {
            Response::Members(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    pub fn invite(&self) -> InviteSessionInfo {
        match self.request(Request::InviteStatus).unwrap() {
            Response::Invite(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    pub fn invite_state(&self) -> InviteSessionState {
        self.invite().state
    }

    pub fn sync_info(&self) -> SyncInfo {
        match self.request(Request::SyncStatus).unwrap() {
            Response::Sync(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// `SpaceShare`; the space reads `Pending` until the server took the `Create`.
    pub fn share(&self, space: SpaceId) {
        match self
            .request(Request::SpaceShare { space_id: space })
            .unwrap()
        {
            Response::Space(info) => assert_eq!(info.share_state, SpaceShareState::Pending),
            other => panic!("unexpected {other:?}"),
        }
    }

    pub fn role_set(&self, space: SpaceId, account: AccountId, role: MemberRole) {
        self.request(Request::MemberRoleSet {
            space_id: space,
            account_id: account,
            role,
        })
        .unwrap();
    }

    // ----- realtime and presence ------------------------------------------------------------------

    /// What the core answered to the inline entries the driver offered it, in order.
    pub fn inline_outcomes(&self) -> Vec<InlineOutcome> {
        self.inline.lock().unwrap().clone()
    }

    /// How many inline entries the core applied (a verified entry stored and applied in one go).
    pub fn inline_applied(&self) -> usize {
        self.inline
            .lock()
            .unwrap()
            .iter()
            .filter(
                |outcome| matches!(outcome, InlineOutcome::Applied(applied) if applied.applied > 0),
            )
            .count()
    }

    /// How many inline entries the core sent back to a pull.
    pub fn inline_pulls(&self) -> usize {
        self.inline
            .lock()
            .unwrap()
            .iter()
            .filter(|outcome| matches!(outcome, InlineOutcome::Pull))
            .count()
    }

    /// Pages of changes the pull path handed the core so far (every pull, a cycle's or a feed's alone,
    /// counts at least one, even an empty page).
    pub fn pulled_pages(&self) -> usize {
        self.pulled_pages.load(Ordering::SeqCst)
    }

    /// What `sync_push_debounce_ms` answered, in order.
    pub fn debounce_hints(&self) -> Vec<u64> {
        self.debounce.lock().unwrap().clone()
    }

    /// The presence frames the core accepted so far.
    pub fn presence_seen(&self) -> Vec<PresenceMessage> {
        self.presence.lock().unwrap().clone()
    }

    /// What the shell's `presence_send` does: the core seals this device's presence in `space` and the
    /// driver's socket sends it. `true` when the socket took it.
    pub fn presence_send(
        &self,
        realtime: &RealtimeHandle,
        space: SpaceId,
        page: Option<PageId>,
        state: PresenceState,
        update: &[u8],
    ) -> bool {
        let sealed = self
            .with(|core| core.presence_seal(space, page, state, update))
            .unwrap();
        sealed.is_some_and(|sealed| realtime.send_presence(sealed.feed, sealed.envelope))
    }

    /// Starts the network side of this device: the v2 driver and the invite tasks. Dropping the
    /// result stops them.
    pub fn start<R: SyncRemote>(self: &Arc<Self>, factory: &RemoteFactory<R>) -> Network {
        let runtime = tokio::runtime::Handle::current();
        let host: Arc<dyn SyncHost> = Arc::clone(self) as Arc<dyn SyncHost>;
        Network {
            _driver: SyncDriver::spawn(host, Arc::clone(factory), runtime.clone()),
            _invites: InviteTasks::spawn(Arc::clone(self), Arc::clone(factory), runtime),
        }
    }
}

/// The running network side of a device.
pub struct Network {
    _driver: DriverHandle,
    _invites: TaskHandle,
}

impl Network {
    /// The shell's handle onto the driver's realtime socket.
    pub fn realtime(&self) -> RealtimeHandle {
        self._driver.realtime()
    }
}

// ----- the hosts ---------------------------------------------------------------------------------

impl SyncHost for Peer {
    fn sync_info(&self) -> Result<SyncInfo, ApiError> {
        self.with(|core| core.sync_info())
    }
    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        self.with(|core| core.sync_identity())
    }
    fn sync_take_invite_token(&self) -> Result<Option<String>, ApiError> {
        self.with(|core| core.sync_take_invite_token())
    }
    fn sync_account_unknown(&self) -> Result<(), ApiError> {
        // What the shell does (`sync_host.rs`): an invitee that is about to get its account from the
        // invitation does not need an invite token.
        self.with(|core| {
            let joining = core.invite_info().is_ok_and(|info| {
                info.side == InviteSide::Invitee
                    && !matches!(
                        info.state,
                        InviteSessionState::Idle
                            | InviteSessionState::Done
                            | InviteSessionState::Failed
                            | InviteSessionState::Cancelled
                    )
            });
            if joining {
                Ok(())
            } else {
                core.sync_account_unknown()
            }
        })
    }
    fn sync_log_received(&self, entries: &[Vec<u8>]) -> Result<bool, ApiError> {
        self.with(|core| core.sync_log_received(entries))
    }
    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.with(|core| core.sync_account_log())
    }
    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.with(|core| core.sync_log_published(up_to_seq))
    }
    fn sync_sign(&self, message: &[u8]) -> Result<kb_crypto::Signature, ApiError> {
        self.with(|core| core.sync_sign(message))
    }
    fn sync_remote_epoch(&self, epoch: [u8; 16]) -> Result<bool, ApiError> {
        self.with(|core| core.sync_remote_epoch(epoch))
    }
    fn sync_set_state(&self, state: SyncState, error: Option<String>) -> Result<(), ApiError> {
        self.with(|core| core.sync_set_state(state, error))
    }
    fn sync_recovery_blob(&self) -> Result<Option<RecoveryUpload>, ApiError> {
        self.with(|core| core.sync_recovery_blob())
    }
    fn sync_recovery_published(&self) -> Result<(), ApiError> {
        self.with(|core| core.sync_recovery_published())
    }
    fn sync_recovery_obsolete(&self) -> Result<Vec<[u8; 32]>, ApiError> {
        self.with(|core| core.sync_recovery_obsolete())
    }
    fn sync_recovery_deleted(&self, id: [u8; 32]) -> Result<(), ApiError> {
        self.with(|core| core.sync_recovery_deleted(id))
    }
    fn sync_feeds(&self) -> Result<Vec<FeedWork>, ApiError> {
        self.with(|core| core.sync_feeds())
    }
    fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError> {
        self.with(|core| core.sync_needed_logs())
    }
    fn sync_foreign_log_received(
        &self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        self.with(|core| core.sync_foreign_log_received(account, entries))
    }
    fn sync_membership_pending(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, ApiError> {
        self.with(|core| core.sync_membership_pending(feed))
    }
    fn sync_membership_posted(&self, feed: FeedId, head: MembershipHead) -> Result<(), ApiError> {
        self.with(|core| core.sync_membership_posted(feed, head))
    }
    fn sync_membership_conflict(&self, feed: FeedId) -> Result<(), ApiError> {
        self.with(|core| core.sync_membership_conflict(feed))
    }
    fn sync_membership_wanted(&self, feed: FeedId) -> Result<Option<Option<u64>>, ApiError> {
        self.with(|core| core.sync_membership_wanted(feed))
    }
    fn sync_membership_fetched(&self, feed: FeedId, page: &MembershipPage) -> Result<(), ApiError> {
        self.with(|core| core.sync_membership_fetched(feed, page))
    }
    fn sync_feed_lost(&self, feed: FeedId) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_lost(feed))
    }
    fn sync_feed_refused(&self, feed: FeedId, message: &str) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_refused(feed, message))
    }
    fn sync_feed_rekey_needed(&self, feed: FeedId) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_rekey_needed(feed))
    }
    fn sync_feed_outbox(&self, feed: FeedId) -> Result<Vec<OutboundEntry>, ApiError> {
        self.with(|core| core.sync_feed_outbox(feed))
    }
    fn sync_feed_pushed(
        &self,
        feed: FeedId,
        doc_id: DocId,
        device_seq: u64,
        cursor: u64,
        up_to_seq: i64,
    ) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_pushed(feed, doc_id, device_seq, cursor, up_to_seq))
    }
    fn sync_feed_apply(&self, feed: FeedId, page: &ChangesPage) -> Result<SyncApplied, ApiError> {
        self.pulled_pages.fetch_add(1, Ordering::SeqCst);
        self.with(|core| core.sync_feed_apply(feed, page))
    }
    fn sync_feed_apply_inline(
        &self,
        feed: FeedId,
        inline: &InlineEntry,
    ) -> Result<InlineOutcome, ApiError> {
        let outcome = self.with(|core| core.sync_feed_apply_inline(feed, inline))?;
        self.inline.lock().unwrap().push(outcome);
        Ok(outcome)
    }
    fn sync_push_debounce_ms(&self) -> Result<u64, ApiError> {
        let hint = self.with(|core| core.sync_push_debounce_ms())?;
        self.debounce.lock().unwrap().push(hint);
        Ok(hint)
    }
    fn presence_received(&self, feed: FeedId, envelope: &[u8]) -> Result<(), ApiError> {
        if let Some(message) = self.with(|core| core.presence_open(feed, envelope))? {
            self.presence.lock().unwrap().push(message);
        }
        Ok(())
    }
    fn presence_expire(&self) -> Result<(), ApiError> {
        self.with(|core| core.presence_expire())
    }
    fn sync_feed_snapshot_candidates(&self, feed: FeedId) -> Result<Vec<DocId>, ApiError> {
        self.with(|core| core.sync_feed_snapshot_candidates(feed))
    }
    fn sync_feed_make_snapshot(
        &self,
        feed: FeedId,
        doc_id: DocId,
    ) -> Result<(Vec<u8>, u64), ApiError> {
        self.with(|core| core.sync_feed_make_snapshot(feed, doc_id))
    }
    fn sync_feed_snapshot_published(
        &self,
        feed: FeedId,
        doc_id: DocId,
        covers: u64,
    ) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_snapshot_published(feed, doc_id, covers))
    }
    fn sync_feed_apply_snapshots(
        &self,
        feed: FeedId,
        page: &SnapshotsPage,
    ) -> Result<SnapshotsApplied, ApiError> {
        self.with(|core| core.sync_feed_apply_snapshots(feed, page))
    }
    fn sync_snapshot_marks_wanted(&self, feed: FeedId) -> Result<Option<u64>, ApiError> {
        self.with(|core| core.sync_snapshot_marks_wanted(feed))
    }
    fn sync_snapshot_marks_received(
        &self,
        feed: FeedId,
        marks: &[SnapshotMark],
    ) -> Result<Vec<DocId>, ApiError> {
        self.with(|core| core.sync_snapshot_marks_received(feed, marks))
    }
    fn sync_snapshot_verified(
        &self,
        feed: FeedId,
        doc_id: DocId,
        ok: bool,
    ) -> Result<(), ApiError> {
        self.with(|core| core.sync_snapshot_verified(feed, doc_id, ok))
    }
    fn sync_feed_head(&self, feed: FeedId) -> Result<Vec<u8>, ApiError> {
        self.with(|core| core.sync_feed_head(feed))
    }
    fn sync_feed_heads_received(&self, feed: FeedId, heads: &[Vec<u8>]) -> Result<bool, ApiError> {
        self.with(|core| core.sync_feed_heads_received(feed, heads))
    }
    fn sync_feed_chunk_work(
        &self,
        feed: FeedId,
        limit: usize,
    ) -> Result<Vec<FeedChunkWork>, ApiError> {
        self.with(|core| core.sync_feed_chunk_work(feed, limit))
    }
    fn sync_feed_chunk_bytes(
        &self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, ApiError> {
        self.with(|core| core.sync_feed_chunk_bytes(feed, hash))
    }
    fn sync_feed_chunk_uploaded(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_chunk_uploaded(feed, hash))
    }
    fn sync_feed_chunk_received(
        &self,
        feed: FeedId,
        hash: ChunkId,
        bytes: Vec<u8>,
    ) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_chunk_received(feed, hash, bytes))
    }
    fn sync_feed_chunk_unavailable(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_chunk_unavailable(feed, hash))
    }
    fn sync_feed_chunk_deleted(&self, feed: FeedId, hash: ChunkId) -> Result<(), ApiError> {
        self.with(|core| core.sync_feed_chunk_deleted(feed, hash))
    }
    fn sync_link_work(&self) -> Result<Vec<LinkWork>, ApiError> {
        self.with(|core| core.sync_link_work())
    }
    fn sync_link_done(&self, link_id: LinkId, outcome: LinkOutcome) -> Result<(), ApiError> {
        self.with(|core| core.sync_link_done(link_id, outcome))
    }
    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

impl Peer {
    fn add_listener(&self, listener: Listener) -> Box<dyn FnOnce() + Send> {
        let id = self.next_listener.fetch_add(1, Ordering::Relaxed);
        self.listeners.lock().unwrap().push((id, listener));
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            listeners.lock().unwrap().retain(|(other, _)| *other != id);
        })
    }

    /// How many listeners the core's events reach (the driver and the tasks).
    pub fn listener_count(&self) -> usize {
        self.listeners.lock().unwrap().len()
    }
}

impl InviteHost for Peer {
    fn invite_info(&self) -> Result<InviteSessionInfo, ApiError> {
        self.with(|core| core.invite_info())
    }
    fn invite_mailbox_id(&self) -> Result<Option<[u8; 6]>, ApiError> {
        self.with(|core| core.invite_mailbox_id())
    }
    fn invite_mailbox_opened(&self, mailbox_id: [u8; 6], expires_at: i64) -> Result<(), ApiError> {
        self.with(|core| core.invite_mailbox_opened(mailbox_id, expires_at))
    }
    fn invite_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        self.with(|core| core.invite_outbox())
    }
    fn invite_posted(&self, seq: u32) -> Result<(), ApiError> {
        self.with(|core| core.invite_posted(seq))
    }
    fn invite_receive(&self, seq: u32, from: InviteSide, bytes: &[u8]) -> Result<(), ApiError> {
        self.with(|core| core.invite_receive(seq, from, bytes))
    }
    fn invite_failed(&self, message: &str) -> Result<(), ApiError> {
        self.with(|core| core.invite_failed(message))
    }
    fn invite_membership_pending(&self) -> Result<Option<PendingMembership>, ApiError> {
        self.with(|core| core.invite_membership_pending())
    }
    fn invite_membership_posted(&self, head: MembershipHead) -> Result<(), ApiError> {
        self.with(|core| core.invite_membership_posted(head))
    }
    fn invite_signup(&self, server_epoch: [u8; 16]) -> Result<Option<InviteSignup>, ApiError> {
        self.with(|core| core.invite_signup(server_epoch))
    }
    fn invite_open_request(
        &self,
        server_epoch: [u8; 16],
    ) -> Result<Option<InviteOpenRequest>, ApiError> {
        self.with(|core| core.invite_open_request(server_epoch))
    }
    fn invite_opened(&self, opened: &InviteOpened) -> Result<(), ApiError> {
        self.with(|core| core.invite_opened(opened))
    }
    fn invite_join(&self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError> {
        self.with(|core| core.invite_join())
    }
    fn invite_joined(&self, head: MembershipHead) -> Result<(), ApiError> {
        self.with(|core| core.invite_joined(head))
    }
    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        self.with(|core| core.sync_identity())
    }
    fn sync_sign(&self, message: &[u8]) -> Result<kb_crypto::Signature, ApiError> {
        self.with(|core| core.sync_sign(message))
    }
    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.with(|core| core.sync_account_log())
    }
    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.with(|core| core.sync_log_published(up_to_seq))
    }
    fn sync_needed_logs(&self) -> Result<Vec<LogRequest>, ApiError> {
        self.with(|core| core.sync_needed_logs())
    }
    fn sync_foreign_log_received(
        &self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<bool, ApiError> {
        self.with(|core| core.sync_foreign_log_received(account, entries))
    }
    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.add_listener(Arc::from(on_event))
    }
}

// ----- the server --------------------------------------------------------------------------------

/// The server between the people, and how to reach it.
pub struct Env<R: SyncRemote> {
    /// What the cores are configured with.
    pub url: String,
    /// One remote per call (a connection of its own, like the shell's `HttpRemote::new`).
    pub factory: RemoteFactory<R>,
    /// The clock of the server and of every core.
    pub clock: Arc<FixedClock>,
    /// The reference server itself, in the in-memory variant (to move its clock, to look at it).
    pub memory: Option<MemoryRemote>,
}

impl Env<MemoryRemote> {
    /// The reference remote, its clock set to the shared one.
    pub fn memory() -> Self {
        let clock = Arc::new(FixedClock::new(T0));
        let remote = MemoryRemote::new(&SeededRng::new(99));
        remote.set_now_ms(T0);
        remote.set_invite_tokens(&[TOKEN]);
        let handle = remote.clone();
        let factory: RemoteFactory<MemoryRemote> =
            Arc::new(move |_url: &str| Ok(Arc::new(remote.clone())));
        Self {
            url: "https://sync.example.test".to_owned(),
            factory,
            clock,
            memory: Some(handle),
        }
    }
}

/// An in-process `kb-server` on a loopback port.
pub struct HttpServer {
    pub base: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl HttpServer {
    pub async fn start(clock: &Arc<FixedClock>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = kb_server::Config {
            listen: ([127, 0, 0, 1], 0).into(),
            data_dir: dir.path().to_path_buf(),
            invite_tokens: vec![TOKEN.to_owned()],
            account_quota_bytes: 10 * 1024 * 1024 * 1024,
            prune_grace_days: 7,
            idle_device_days: None,
            trusted_proxies: Vec::new(),
            share_signups: 20,
            share_signups_per_account: 5,
            viewer_dir: None,
        };
        let shared: Arc<dyn Clock> = Arc::new(SharedClock(Arc::clone(clock)));
        let server = kb_server::Server::open_with_clock(&config, shared).unwrap();
        let app = server
            .router()
            .into_make_service_with_connect_info::<SocketAddr>();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            base: format!("http://{addr}"),
            shutdown: Some(shutdown),
            task: Some(task),
            _dir: dir,
        }
    }

    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Env<HttpRemote> {
    /// A real server on a loopback port, spoken to over HTTP.
    pub async fn http() -> (Self, HttpServer) {
        let clock = Arc::new(FixedClock::new(T0));
        let server = HttpServer::start(&clock).await;
        let factory: RemoteFactory<HttpRemote> =
            Arc::new(|url: &str| HttpRemote::new(url).map(Arc::new));
        (
            Self {
                url: server.base.clone(),
                factory,
                clock,
                memory: None,
            },
            server,
        )
    }
}

// ----- waiting -----------------------------------------------------------------------------------

/// Polls `condition` until it holds, on tokio's clock (real time in the HTTP tests, virtual in the
/// paused ones). `what` names it in the failure.
pub async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    // The conditions read a core (they are not free): the pause between looks grows, so that a wait of
    // a virtual minute (the pull timer, on the paused clock) costs a hundred looks, not thousands.
    let mut pause = Duration::from_millis(10);
    for _ in 0..4_000 {
        if condition() {
            return;
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(Duration::from_millis(500));
    }
    panic!("never happened: {what}");
}

/// Logs to stderr when `KB_TEST_LOG` holds a filter (`KB_TEST_LOG=kb_net=debug,kb_core=debug`).
pub fn init_logging() {
    if let Ok(filter) = std::env::var("KB_TEST_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_test_writer()
            .try_init();
    }
}

// ----- shared scenes -----------------------------------------------------------------------------

impl<R: SyncRemote> Env<R> {
    /// Alice with a profile, sync and a space `Project` of two pages (`Plan`, with `Notes` under it), and
    /// her network.
    pub fn alice_with_a_project(&self) -> (Arc<Peer>, Network, SpaceId, PageId) {
        let alice = Peer::open("alice", 1, &self.clock);
        alice.set_profile("Alice");
        alice.configure(&self.url, Some(TOKEN));
        let space = alice.create_space("Project");
        let plan = alice.create_page(space, None, "Plan");
        alice.create_page(space, Some(plan), "Notes");
        let network = alice.start(&self.factory);
        (alice, network, space, plan)
    }

    /// A person with a workspace, a kit and a profile name, and no sync configuration: the server
    /// creates the account through the share signup when the person joins.
    pub fn newcomer(&self, name: &'static str, seed: u64, profile: &str) -> (Arc<Peer>, Network) {
        let peer = Peer::open(name, seed, &self.clock);
        peer.set_profile(profile);
        let network = peer.start(&self.factory);
        (peer, network)
    }

    /// Whether the server holds the account's device log.
    pub async fn account_on_server(&self, account: AccountId) -> bool {
        let remote = (self.factory)(&self.url).unwrap();
        match remote.fetch_log(account, None).await {
            Ok(entries) => !entries.is_empty(),
            Err(kb_sync::SyncError::NotFound) => false,
            Err(other) => panic!("the server answered {other:?}"),
        }
    }

    /// Moves time on for the server and every core.
    pub fn advance(&self, delta_ms: i64) {
        self.clock.advance(delta_ms);
        if let Some(memory) = &self.memory {
            memory.advance_ms(delta_ms);
        }
    }
}

/// Alice shares `space` and the server takes the `Create`.
pub async fn shared(alice: &Arc<Peer>, space: SpaceId) {
    alice.share(space);
    until("the server took the Create", || {
        alice
            .space(space)
            .is_some_and(|info| info.share_state == SpaceShareState::Shared)
    })
    .await;
}

/// Alice made a link invite and the driver put its `Invite` on the server.
pub async fn link_on_the_server(
    alice: &Arc<Peer>,
    space: SpaceId,
    role: MemberRole,
    expires_in_days: Option<u32>,
) -> kb_types::InviteLinkInfo {
    let link = match alice
        .request(Request::InviteLinkCreate {
            space_id: space,
            role,
            expires_in_days,
        })
        .unwrap()
    {
        Response::InviteLink(link) => link,
        other => panic!("unexpected {other:?}"),
    };
    // The link is shared by hand in real life: by then the driver has put the `Invite` on the server
    // (the server cannot tell a link that was never written from a dead one: one 404 for both).
    until("the Invite is on the server", || {
        alice.members(space).invites.iter().any(|invite| {
            invite.invite_id == link.invite_id && invite.status == kb_types::InviteStatus::Open
        }) && alice.sync_info().pending_entries == 0
    })
    .await;
    link
}

/// How long `condition` takes to hold on tokio's clock (real time in the HTTP tests, virtual in the paused
/// ones), looked at every `step`; a failure when it takes more than `limit`.
pub async fn takes(
    what: &str,
    limit: Duration,
    step: Duration,
    mut condition: impl FnMut() -> bool,
) -> Duration {
    let start = tokio::time::Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < limit,
            "never happened within {limit:?}: {what}"
        );
        tokio::time::sleep(step).await;
    }
    start.elapsed()
}

/// Bob joins Alice's shared `space` through a link, as an Editor, and has its pages.
pub async fn join_by_link(alice: &Arc<Peer>, bob: &Arc<Peer>, space: SpaceId) {
    let link = link_on_the_server(alice, space, MemberRole::Editor, None).await;
    bob.request(Request::InviteLinkOpen {
        link: link.link.clone(),
    })
    .unwrap();
    until("Bob sees the preview", || {
        bob.invite_state() == InviteSessionState::Previewing
    })
    .await;
    bob.request(Request::InviteConfirm).unwrap();
    until("Bob joined", || {
        bob.invite_state() == InviteSessionState::Done
    })
    .await;
    until("Bob has the pages", || {
        bob.space(space)
            .is_some_and(|info| info.role == Some(MemberRole::Editor))
            && bob.outline(space) == alice.outline(space)
    })
    .await;
    until("both are idle", || {
        alice.sync_info().state == SyncState::Idle && bob.sync_info().state == SyncState::Idle
    })
    .await;
}
